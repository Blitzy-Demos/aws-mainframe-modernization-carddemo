//! CR00 reports queued behind the API's per-process report admission, so that
//! overlapping reports never hold the whole connection pool, and each one
//! still answered whole.
//!
//! A report holds one pooled connection and one read-only snapshot while it
//! runs and streams its answer. The API therefore admits reports before they
//! open their unit of work: a quarter of the pool's connections, and at least
//! 1, which on the testkit pool of 5 connections is 1. Here a gate transaction
//! holds `ACCESS EXCLUSIVE` on `transactions`, the table under the live
//! master, so an admitted report waits in the database on its first page
//! while more reports arrive than the admission admits. Exactly the admitted
//! ones wait in the database, never more, the rest queue in the API without a
//! connection, and `GET /accounts/{id}` still answers at once. Once the gate
//! commits, every report answers 200 with the body a lone report gave before
//! the gate, byte for byte, and a report after them answers the same: every
//! report permit is back once the bodies are read.
//!
//! A client that requests a report far larger than the server's and the
//! sockets' buffers, and then never reads, keeps the one report permit only
//! until the answer has waited `REPORT_SEND_DEADLINE` for it to take a
//! chunk. While its connection stays open, the next report answers 200
//! whole within that deadline and a margin, no session is left idle in its
//! transaction, and the stalled answer, once read, ends short of its
//! `Content-Length`.
//!
//! Docker must be running; the harness conventions are those of `common`.

pub mod common;

use std::io::ErrorKind;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use carddemo_api::dto::reports::ReportResponse;
use carddemo_api::handlers::reports::REPORT_SEND_DEADLINE;
use carddemo_domain::messages;
use reqwest::StatusCode;
use tokio::net::{TcpSocket, TcpStream};
use tokio::task::JoinHandle;

use common::{Resp, TestApp};

/// The processing clock, after the report range.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The live transactions [`LIVE_ROWS`] writes, all in the report range.
const ROWS: usize = 30;

/// [`ROWS`] live purchases on card `9680294154603697` of account 1, on
/// 2022-06-15, with type `01` and category `0001`, which the seeded
/// `card_xref`, `trantype_file` and `trancatg_file` hold. The seeded live
/// master is empty (E-07).
const LIVE_ROWS: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     SELECT lpad(n::text, 16, '0'), '01', 1, 'POS TERM', 'Report admission row', \
      (10 + n)::numeric(11, 2), n, 'Report merchant', 'Report city', '12345', \
      '9680294154603697', '2022-06-15 12:00:00.000000', '2022-06-15 12:00:00.000000' \
     FROM generate_series(1, 30) AS n";

/// The custom report of June 2022, confirmed: every row of [`LIVE_ROWS`].
const REPORT: &str =
    "/reports/transactions?report_type=custom&start_date=2022-06-01&end_date=2022-06-30&confirm=Y";

/// The table whose one row releases the gate.
const CREATE_RELEASE: &str = "CREATE TABLE report_gate_release (released BOOLEAN NOT NULL)";

/// The row that releases the gate.
const RELEASE: &str = "INSERT INTO report_gate_release VALUES (TRUE)";

/// The gate: one transaction that takes `ACCESS EXCLUSIVE` on `transactions`,
/// which every read of the live master waits for, and keeps it until
/// [`RELEASE`] commits its row, then commits. It fails after 60 seconds
/// without the row, so a test that never releases it fails instead of
/// hanging.
const GATE: &str = "BEGIN; \
     LOCK TABLE transactions IN ACCESS EXCLUSIVE MODE; \
     DO $$ \
     DECLARE deadline timestamptz := clock_timestamp() + interval '60 seconds'; \
     BEGIN \
         WHILE NOT EXISTS (SELECT 1 FROM report_gate_release) LOOP \
             IF clock_timestamp() > deadline THEN \
                 RAISE EXCEPTION 'the report gate was never released'; \
             END IF; \
             PERFORM pg_sleep(0.01); \
         END LOOP; \
     END $$; \
     COMMIT;";

/// The granted `ACCESS EXCLUSIVE` locks on `transactions`: 1 once the gate
/// holds its lock.
const GATE_HELD: &str = "SELECT count(*)::text FROM pg_locks AS l \
     JOIN pg_class AS c ON c.oid = l.relation \
     WHERE c.relname = 'transactions' AND l.mode = 'AccessExclusiveLock' AND l.granted";

/// The sessions waiting for a heavyweight lock: here, the admitted reports
/// waiting for the gate's lock.
const LOCK_WAITERS: &str =
    "SELECT count(*)::text FROM pg_stat_activity WHERE wait_event_type = 'Lock'";

/// The reports sent while the gate holds its lock, more than [`ADMITTED`].
/// With the gate's connection, as many reports each holding a connection
/// would take the whole testkit pool of 5, and no bearer check could run.
const REPORTS: usize = 4;

/// The reports the API admits at once on the testkit pool of 5 connections:
/// `max(1, 5 / 4)`.
const ADMITTED: u64 = 1;

/// How long the gate and the admitted reports may take to reach their
/// lock states.
const LOCK_DEADLINE: Duration = Duration::from_secs(30);

/// How long the lock waiters are watched once the admitted reports wait.
const OBSERVATION: Duration = Duration::from_secs(1);

/// The bound on the account read while the reports queue. Without the
/// admission the reports would hold every pooled connection and its bearer
/// check would wait for the pool's 30-second acquire timeout.
const READ_DEADLINE: Duration = Duration::from_secs(5);

/// The pause between two reads of a lock count.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 80,000 live purchases on the card of [`LIVE_ROWS`], on 2022-05-15, keyed
/// after its rows: a report of some 15 MB, several times what the server's
/// write buffer, the two sockets' buffers and the report's channel hold
/// together, so a client that never reads stalls the answer long before its
/// end.
const STALLED_ROWS: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     SELECT lpad(n::text, 16, '0'), '01', 1, 'POS TERM', 'Stalled report row', \
      (10 + n % 100)::numeric(11, 2), n, 'Report merchant', 'Report city', '12345', \
      '9680294154603697', '2022-05-15 12:00:00.000000', '2022-05-15 12:00:00.000000' \
     FROM generate_series(1001, 81000) AS n";

/// The custom report of May 2022, confirmed: every row of [`STALLED_ROWS`].
const STALLED_REPORT: &str =
    "/reports/transactions?report_type=custom&start_date=2022-05-01&end_date=2022-05-31&confirm=Y";

/// The receive buffer the never-reading client asks for, which the kernel
/// keeps small: what it takes before it stops reading is a few kilobytes.
const STALLED_RECEIVE_BUFFER: u32 = 4 * 1024;

/// The sessions holding a snapshot between two statements of their
/// transaction for 2 seconds or more: the stalled report's, once its second
/// pass waits for the client to take a chunk.
const STALLED_SESSION: &str = "SELECT count(*)::text FROM pg_stat_activity \
     WHERE state = 'idle in transaction' AND backend_xmin IS NOT NULL \
     AND state_change < clock_timestamp() - interval '2 seconds'";

/// The sessions between two statements of an open transaction: none once
/// every report has rolled back.
const OPEN_TRANSACTIONS: &str =
    "SELECT count(*)::text FROM pg_stat_activity WHERE state = 'idle in transaction'";

/// How long the stalled report may take to run its first pass and fill the
/// buffers in an unoptimised build on a loaded host.
const STALL_DEADLINE: Duration = Duration::from_secs(120);

/// What the report after the stalled one may take beyond
/// [`REPORT_SEND_DEADLINE`]. It is sent once the stalled pass has waited 2
/// seconds of that deadline, so the margin covers its own run, which a
/// stalled report still holding the permit would never start.
const FOLLOW_UP_MARGIN: Duration = Duration::from_secs(30);

/// How long reading the rest of the stalled answer may take.
const DRAIN_DEADLINE: Duration = Duration::from_secs(30);

/// The bytes one read of the stalled connection takes at most.
const READ_CHUNK: usize = 64 * 1024;

#[test]
fn queued_reports_wait_only_as_admitted_and_each_answers_whole() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(LIVE_ROWS).await;
        app.stack().exec_sql(CREATE_RELEASE).await;
        let token = app.user_token().await;
        let lone = app.get(REPORT, Some(&token)).await;
        assert_whole_report(&lone);

        let gate = app.stack().exec_sql(GATE);
        let queued = async {
            wait_for_count(&app, GATE_HELD, 1).await;
            let reports: Vec<_> = (0..REPORTS).map(|_| spawn_report(&app, &token)).collect();
            wait_for_count(&app, LOCK_WAITERS, ADMITTED).await;
            let until = Instant::now() + OBSERVATION;
            while Instant::now() < until {
                let waiting = count(&app, LOCK_WAITERS).await;
                assert!(
                    waiting <= ADMITTED,
                    "{waiting} sessions wait for a lock; the API admits {ADMITTED} report"
                );
                pause(POLL_INTERVAL).await;
            }

            let read = app
                .client()
                .get(app.url("/accounts/00000000001"))
                .timeout(READ_DEADLINE);
            let (status, _, raw) = common::send(read, Some(&token)).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "GET /accounts/00000000001 while the reports queue: {raw}"
            );
            assert!(
                reports.iter().all(|report| !report.is_finished()),
                "no report can answer while the gate holds its lock"
            );

            app.stack().exec_sql(RELEASE).await;
            let mut answers = Vec::with_capacity(REPORTS);
            for report in reports {
                answers.push(
                    report
                        .await
                        .unwrap_or_else(|error| panic!("a report's request task failed: {error}")),
                );
            }
            answers
        };
        let ((), answers) = tokio::join!(gate, queued);

        for (at, (status, _, raw)) in answers.iter().enumerate() {
            assert_eq!(*status, StatusCode::OK, "queued report {at}: {raw}");
            assert!(
                *raw == lone.2,
                "queued report {at} differs from the lone report: {raw}"
            );
        }
        let after = app.get(REPORT, Some(&token)).await;
        assert_eq!(
            after.0,
            StatusCode::OK,
            "the report after the queue: {}",
            after.2
        );
        assert!(
            after.2 == lone.2,
            "the report after the queue differs from the lone report: {}",
            after.2
        );
    });
}

#[test]
fn a_client_that_never_reads_gives_back_the_report_permit_after_the_send_deadline() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(LIVE_ROWS).await;
        app.stack().exec_sql(STALLED_ROWS).await;
        let token = app.user_token().await;
        let lone = app.get(REPORT, Some(&token)).await;
        assert_whole_report(&lone);

        let stalled = request_unread(&app, STALLED_REPORT, &token).await;
        wait_for_count_within(&app, STALLED_SESSION, 1, STALL_DEADLINE).await;

        // The testkit pool admits one report at once, and the stalled report
        // holds that permit: this one is answered only once it is back.
        let bound = REPORT_SEND_DEADLINE + FOLLOW_UP_MARGIN;
        let request = app.client().get(app.url(REPORT)).timeout(bound);
        let after = common::send(request, Some(&token)).await;
        assert_eq!(
            after.0,
            StatusCode::OK,
            "the report after the stalled one: {}",
            after.2
        );
        assert!(
            after.2 == lone.2,
            "the report after the stalled one differs from the lone report: {}",
            after.2
        );
        assert_eq!(
            count(&app, OPEN_TRANSACTIONS).await,
            0,
            "a session is still idle in its transaction after the stalled report"
        );

        let answer = tokio::time::timeout(DRAIN_DEADLINE, read_to_end(&stalled))
            .await
            .unwrap_or_else(|_| {
                panic!("the stalled answer did not end within {DRAIN_DEADLINE:?} of reading it")
            });
        let (status, length, body) = split_response(&answer);
        assert_eq!(
            status, "HTTP/1.1 200 OK",
            "the stalled report's status line"
        );
        assert!(
            body < length,
            "the stalled answer sent {body} bytes of its Content-Length {length}"
        );
    });
}

/// Asserts that `resp` is the 200 custom report of the [`ROWS`] live rows:
/// its name, its submission text, and more lines than rows, the headers and
/// totals around one detail line per row.
fn assert_whole_report(resp: &Resp) {
    let (status, body, raw) = resp;
    assert_eq!(*status, StatusCode::OK, "the lone report: {raw}");
    let report: ReportResponse = serde_json::from_value(body.clone())
        .unwrap_or_else(|error| panic!("the body is no ReportResponse ({error}): {raw}"));
    assert_eq!(report.report_name, messages::REPORT_NAME_CUSTOM, "{raw}");
    assert_eq!(
        report.message,
        messages::report_submitted_message(messages::REPORT_NAME_CUSTOM),
        "{raw}"
    );
    assert!(
        report.lines.len() > ROWS,
        "{} lines for {ROWS} rows: {raw}",
        report.lines.len()
    );
}

/// Sends `GET` [`REPORT`] with `token` as the bearer token on a task of its
/// own, so the request can queue while the test goes on.
fn spawn_report(app: &TestApp, token: &str) -> JoinHandle<Resp> {
    let request = app.client().get(app.url(REPORT)).bearer_auth(token);
    tokio::spawn(common::send(request, None))
}

/// The one count `sql` reads now.
async fn count(app: &TestApp, sql: &str) -> u64 {
    let rows = app.stack().fetch_text_rows(sql).await;
    let [row] = rows.as_slice() else {
        panic!("one count row expected, got {rows:?}");
    };
    row.first()
        .and_then(Option::as_deref)
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| panic!("a count expected, got {row:?}"))
}

/// Waits until `sql` counts exactly `expected`, panicking when that does not
/// happen within [`LOCK_DEADLINE`].
async fn wait_for_count(app: &TestApp, sql: &str, expected: u64) {
    wait_for_count_within(app, sql, expected, LOCK_DEADLINE).await;
}

/// Waits until `sql` counts exactly `expected`, panicking when that does not
/// happen within `deadline`.
async fn wait_for_count_within(app: &TestApp, sql: &str, expected: u64, deadline: Duration) {
    let until = Instant::now() + deadline;
    loop {
        let counted = count(app, sql).await;
        if counted == expected {
            return;
        }
        assert!(
            Instant::now() < until,
            "expected {expected} within {deadline:?}, last counted {counted}: {sql}"
        );
        pause(POLL_INTERVAL).await;
    }
}

/// Sends `GET path` with `token` as the bearer token over a connection of
/// its own, whose receive buffer is [`STALLED_RECEIVE_BUFFER`] bytes, and
/// returns the connection without reading any of the answer.
async fn request_unread(app: &TestApp, path: &str, token: &str) -> TcpStream {
    let base = app.url("");
    let addr: SocketAddr = base
        .strip_prefix("http://")
        .and_then(|host| host.parse().ok())
        .unwrap_or_else(|| panic!("the test server URL {base} names no socket address"));
    let socket = TcpSocket::new_v4()
        .unwrap_or_else(|error| panic!("creating the unread client socket failed: {error}"));
    socket
        .set_recv_buffer_size(STALLED_RECEIVE_BUFFER)
        .unwrap_or_else(|error| panic!("shrinking the unread client's buffer failed: {error}"));
    let bound = common::CONNECT_TIMEOUT;
    let stream = tokio::time::timeout(bound, socket.connect(addr))
        .await
        .unwrap_or_else(|_| panic!("no connection to {addr} within {bound:?}"))
        .unwrap_or_else(|error| panic!("connecting the unread client to {addr} failed: {error}"));
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\n\r\n");
    let mut unsent = request.as_bytes();
    while !unsent.is_empty() {
        stream
            .writable()
            .await
            .unwrap_or_else(|error| panic!("the unread client cannot write: {error}"));
        match stream.try_write(unsent) {
            Ok(written) => unsent = &unsent[written..],
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) => panic!("sending the unread client's request failed: {error}"),
        }
    }
    stream
}

/// Every byte `stream` receives until the server ends the connection, by a
/// close or a reset.
async fn read_to_end(stream: &TcpStream) -> Vec<u8> {
    let mut received = Vec::new();
    let mut buffer = vec![0; READ_CHUNK];
    loop {
        stream
            .readable()
            .await
            .unwrap_or_else(|error| panic!("the stalled connection cannot be read: {error}"));
        match stream.try_read(&mut buffer) {
            Ok(0) => return received,
            Ok(read) => received.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {}
            Err(error) if error.kind() == ErrorKind::ConnectionReset => return received,
            Err(error) => panic!("reading the stalled connection failed: {error}"),
        }
    }
}

/// The status line, the `Content-Length` and the length of the body of
/// `answer`, one HTTP/1.1 response as received.
fn split_response(answer: &[u8]) -> (String, usize, usize) {
    let Some(end) = answer.windows(4).position(|window| window == b"\r\n\r\n") else {
        panic!(
            "the stalled answer holds no whole head: {} bytes",
            answer.len()
        );
    };
    let head = String::from_utf8_lossy(&answer[..end]);
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default().to_owned();
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse().ok())
        .unwrap_or_else(|| panic!("the stalled answer has no numeric Content-Length: {head}"));
    (status, length, answer.len() - end - 4)
}

/// Sleeps for `period` on a blocking thread, leaving the runtime's workers to
/// the server, the gate and the request tasks.
async fn pause(period: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(period))
        .await
        .unwrap_or_else(|error| panic!("the pause task failed: {error}"));
}
