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
//! Docker must be running; the harness conventions are those of `common`.

pub mod common;

use std::time::{Duration, Instant};

use carddemo_api::dto::reports::ReportResponse;
use carddemo_domain::messages;
use reqwest::StatusCode;
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
    let until = Instant::now() + LOCK_DEADLINE;
    loop {
        let counted = count(app, sql).await;
        if counted == expected {
            return;
        }
        assert!(
            Instant::now() < until,
            "expected {expected} within {LOCK_DEADLINE:?}, last counted {counted}: {sql}"
        );
        pause(POLL_INTERVAL).await;
    }
}

/// Sleeps for `period` on a blocking thread, leaving the runtime's workers to
/// the server, the gate and the request tasks.
async fn pause(period: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(period))
        .await
        .unwrap_or_else(|error| panic!("the pause task failed: {error}"));
}
