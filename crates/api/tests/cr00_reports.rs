//! CR00 · `CORPT00C` · map `CORPT00` → `GET /reports/transactions`.
//!
//! Seven tests in six parts, each but the one reading the published document
//! on its own seeded database with the processing clock pinned to [`CLOCK`]:
//!
//! 1. The request edits of `PROCESS-ENTER-KEY` and `SUBMIT-JOB-TO-INTRDR`
//!    [app/cbl/CORPT00C.cbl:L208-494]: one input per `V-CORPT00C` row of
//!    `docs/parity/validation-audit.md`, answered with the row's status, BMS
//!    field and message, then inputs failing several edits at once, answered
//!    by the row the COBOL reaches first.
//! 2. The monthly and yearly ranges `FUNCTION CURRENT-DATE` gives
//!    [app/cbl/CORPT00C.cbl:L213-255] (P-CORPT00C-003, P-CORPT00C-004).
//! 3. The report lines of `CBTRN03C` over the state section D.6 of
//!    `docs/parity/parity-oracle-audit.md` derives them from (`batch seed`,
//!    then `batch posttran` with the clock pinned), equal to its expected lines
//!    P-CBTRN03C-016 to P-CBTRN03C-466; then an admin edit of a Db2 master type
//!    that leaves them unchanged, because the report reads only the processing
//!    copies `trantype_file` and `trancatg_file`.
//! 4. The 3270 mechanics the one GET replaces: the published operation, its
//!    query and its report, then requests standing for a MAPFAIL, the AID keys
//!    other than ENTER, a missing session, each report with each class of
//!    confirmation, and the form `INITIALIZE-ALL-FIELDS` empties
//!    (P-CORPT00C-001, P-CORPT00C-009 to P-CORPT00C-011, P-CORPT00C-018 to
//!    P-CORPT00C-021).
//! 5. The custom date parts: the empty checks and the value edits in screen
//!    order, the `CSUTLDTC` checks, the `NUMVAL-C` conversions, and malformed
//!    parts (P-CORPT00C-005 to P-CORPT00C-008, P-CORPT00C-015,
//!    P-CORPT00C-016).
//! 6. The TDQ write the synchronous report replaces: a run whose live master
//!    or lookup table is missing answers 500 `Unexpected error`, never the
//!    `WRITEQ TD` failure text (P-CORPT00C-013, E-48).
//!
//! Expected texts come from `carddemo_domain::messages` and are checked
//! against the audit's quoted texts; statuses, fields, orders, ranges and lines
//! come from reviewed audit rows. Docker must be running; the harness
//! conventions are those of `common`.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs;
use std::ops::Range;
use std::str::FromStr;

use carddemo_api::dto::reports::{ReportRequestField, ReportResponse};
use carddemo_audit::{Document, Row, Table};
use carddemo_domain::entities::DailyTran;
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_domain::numeric::{Alnum, CardNumber, Money, TranId, TranTypeCode};
use carddemo_domain::ports::TransactionWriter;
use carddemo_persistence::Db;
use carddemo_testkit::fixtures;
use reqwest::StatusCode;
use serde_json::{Value, json};

use common::{Resp, TestApp};

/// The processing clock of every test: the instant the D.6 derivation pins
/// `CARDDEMO_CLOCK_OVERRIDE` to (P-CBTRN03C-015), inside the TRANREPT range
/// 2022-01-01 to 2022-07-06.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The route under test.
const REPORT_PATH: &str = "/reports/transactions";

/// The program whose `V-` rows the edit test exercises.
const PROGRAM: &str = "CORPT00C";

// The audit column headers this file reads.
const ID: &str = "ID";
const EXPECTED_RESULT: &str = "Expected result";
const INPUT_CONDITION: &str = "Input condition";
const TRIGGER_CONDITION: &str = "Trigger condition";
const MESSAGE_TEXT: &str = "Message text";
const STATUS: &str = "Status";
const BMS_FIELD: &str = "BMS field";
const ORDER: &str = "Order";
const ROW_ID: &str = "Row ID";
const VERDICT: &str = "Verdict";

/// The review verdict a row must carry before a test relies on it.
const PASS: &str = "pass";

/// The `D.1 POSTTRAN reject set` subsection: one row per rejected daily
/// record, with its `record_no` and `tran_id`.
const REJECT_SET: &str = "D.1 ";

/// The `D.6 TRANREPT` subsection: P-CBTRN03C-015, then one row per report
/// line, P-CBTRN03C-016 to P-CBTRN03C-466.
const TRANREPT_LINES: &str = "D.6 ";

/// The TRANREPT run: the range in its input condition, and
/// `records_in_range`, `lines` and `width` in its expected result.
const TRANREPT_RUN: &str = "P-CBTRN03C-015";

/// The POSTTRAN counts over the whole fixture: `read`, `posted`, `rejected`.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// `Z-GET-DB2-FORMAT-TIMESTAMP`: the `pinned` processing timestamp
/// CBTRN02C stamps on every posted record under [`CLOCK`].
const PROC_TS_ROW: &str = "P-CBTRN02C-220";

/// The `CVTRA06Y DALYTRAN-RECORD` fields in copybook order, through the
/// trailing `FILLER` [app/cpy/CVTRA06Y.cpy:L5-18]: each row's `Offset` and
/// `Length` place the field in the 350-byte record.
const DALYTRAN_FIELDS: [&str; 14] = [
    "P-CVTRA06Y-001",
    "P-CVTRA06Y-002",
    "P-CVTRA06Y-003",
    "P-CVTRA06Y-004",
    "P-CVTRA06Y-005",
    "P-CVTRA06Y-006",
    "P-CVTRA06Y-007",
    "P-CVTRA06Y-008",
    "P-CVTRA06Y-009",
    "P-CVTRA06Y-010",
    "P-CVTRA06Y-011",
    "P-CVTRA06Y-012",
    "P-CVTRA06Y-013",
    "P-CVTRA06Y-014",
];

/// The seeded Db2 master type `01` and its description `PURCHASE`.
const MASTER_TYPE_ROW: &str = "P-TRNTYPE-004";

/// The seeded processing copy of type `01` and its description `Purchase`.
const PROCESSING_TYPE_ROW: &str = "P-CVTRA03Y-005";

/// The Db2 master description of type `01`.
const MASTER_TYPE_01: &str = "SELECT tr_description FROM transaction_types WHERE tr_type = '01'";

/// The processing-copy description of type `01`.
const PROCESSING_TYPE_01: &str = "SELECT tran_type_desc FROM trantype_file WHERE tran_type = '01'";

/// The new description of type `01`: letters and a space, so the `COTRTUPC`
/// alphanumeric edit passes, and unequal to `PURCHASE` after upper-casing, so
/// the update is a change (V-COTRTUPC-006).
const NEW_TYPE_DESCRIPTION: &str = "Retail purchase";

/// The 401 code of a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

// ---------------------------------------------------------------------------
// 1. The request edits (V-CORPT00C-001 to V-CORPT00C-018)
// ---------------------------------------------------------------------------

/// The custom query part every date edit accepts: the TRANREPT range
/// 2022-01-01 to 2022-07-06 the `V-CORPT00C` confirmation rows name.
const VALID_RANGE: &str = "report_type=custom&start_date=2022-01-01&end_date=2022-07-06";

/// One input of the edit test: the audit row it reaches first, the query
/// carrying the row's example, and the text the message catalog gives for it.
struct EditCase {
    /// The `V-CORPT00C` row.
    id: &'static str,
    /// The query string, without its `?`.
    query: String,
    /// The `carddemo_domain::messages` text of the row for this input; empty
    /// for the row that sets no message.
    message: String,
    /// The `STRING` variable the audit text shows as `{<COBOL-FIELD>}`, and
    /// the value this input gives it.
    variable: Option<(&'static str, &'static str)>,
}

impl EditCase {
    /// A case whose message is a catalog literal.
    fn literal(id: &'static str, query: &str, message: &str) -> Self {
        Self {
            id,
            query: query.to_owned(),
            message: message.to_owned(),
            variable: None,
        }
    }

    /// A custom report over [`VALID_RANGE`] with `confirm`, whose message the
    /// catalog composes, the audit showing `variable` as `value`.
    fn confirmation(
        id: &'static str,
        confirm: &str,
        message: String,
        variable: Option<(&'static str, &'static str)>,
    ) -> Self {
        Self {
            id,
            query: format!("{VALID_RANGE}&confirm={confirm}"),
            message,
            variable,
        }
    }
}

/// A custom report with the dates `start` and `end`, confirmed with `Y`.
fn custom(start: &str, end: &str) -> String {
    confirmed(start, end, "Y")
}

/// A custom report with the dates `start` and `end` and the confirmation
/// value `confirm`.
fn confirmed(start: &str, end: &str, confirm: &str) -> String {
    format!("report_type=custom&start_date={start}&end_date={end}&confirm={confirm}")
}

/// The first and last dates of [`VALID_RANGE`], for date inputs that leave
/// one of the two dates valid.
const VALID_START: &str = "2022-01-01";
const VALID_END: &str = "2022-07-06";

/// The date rows of `CORPT00C`, V-CORPT00C-002 to V-CORPT00C-015: each row,
/// the custom start and end dates of its audit example, and its catalog text.
/// V-CORPT00C-010 and V-CORPT00C-013 are reached by a malformed year, whose
/// rejection with the field's value message is the E-28 proposal the rows
/// record. V-CORPT00C-014 and V-CORPT00C-015 use a day beyond the month
/// length, a `CSUTLDTC` class the audit pins (P-CSUTLDTC-003).
const DATE_CASES: [(&str, &str, &str, &str); 14] = [
    (
        "V-CORPT00C-002",
        "2022--01",
        VALID_END,
        messages::START_DATE_MONTH_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-CORPT00C-003",
        "2022-01-",
        VALID_END,
        messages::START_DATE_DAY_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-CORPT00C-004",
        "-01-01",
        VALID_END,
        messages::START_DATE_YEAR_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-CORPT00C-005",
        VALID_START,
        "2022--06",
        messages::END_DATE_MONTH_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-CORPT00C-006",
        VALID_START,
        "2022-07-",
        messages::END_DATE_DAY_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-CORPT00C-007",
        VALID_START,
        "-07-06",
        messages::END_DATE_YEAR_CAN_NOT_BE_EMPTY,
    ),
    (
        "V-CORPT00C-008",
        "2022-13-01",
        VALID_END,
        messages::START_DATE_NOT_A_VALID_MONTH,
    ),
    (
        "V-CORPT00C-009",
        "2022-01-32",
        VALID_END,
        messages::START_DATE_NOT_A_VALID_DAY,
    ),
    (
        "V-CORPT00C-010",
        "20A2-01-01",
        VALID_END,
        messages::START_DATE_NOT_A_VALID_YEAR,
    ),
    (
        "V-CORPT00C-011",
        VALID_START,
        "2022-13-06",
        messages::END_DATE_NOT_A_VALID_MONTH,
    ),
    (
        "V-CORPT00C-012",
        VALID_START,
        "2022-07-32",
        messages::END_DATE_NOT_A_VALID_DAY,
    ),
    (
        "V-CORPT00C-013",
        VALID_START,
        "20A2-07-06",
        messages::END_DATE_NOT_A_VALID_YEAR,
    ),
    (
        "V-CORPT00C-014",
        "2022-02-30",
        VALID_END,
        messages::START_DATE_NOT_A_VALID_DATE,
    ),
    (
        "V-CORPT00C-015",
        VALID_START,
        "2022-04-31",
        messages::END_DATE_NOT_A_VALID_DATE,
    ),
];

/// The edit inputs, one per `V-CORPT00C` row in audit order: each the row's
/// example, every edit before it passing.
fn edit_cases() -> Vec<EditCase> {
    use messages as m;
    let mut cases = vec![EditCase::literal(
        "V-CORPT00C-001",
        "confirm=Y",
        m::SELECT_A_REPORT_TYPE_TO_PRINT_REPORT,
    )];
    cases.extend(
        DATE_CASES
            .into_iter()
            .map(|(id, start, end, message)| EditCase::literal(id, &custom(start, end), message)),
    );
    cases.extend([
        EditCase::confirmation(
            "V-CORPT00C-016",
            "",
            m::confirm_print_report_message(m::REPORT_NAME_CUSTOM),
            Some(("{WS-REPORT-NAME}", m::REPORT_NAME_CUSTOM)),
        ),
        // CONFIRM N clears the form and sets no message (E-49).
        EditCase::confirmation("V-CORPT00C-017", "N", String::new(), None),
        EditCase::confirmation(
            "V-CORPT00C-018",
            "X",
            m::invalid_confirm_value_message("X"),
            Some(("{CONFIRMI}", "X")),
        ),
    ]);
    cases
}

/// Inputs failing more than one edit, each with every row whose condition it
/// meets. The answer is the row with the lowest (`Order`, `ID`):
/// `SEND-TRNRPT-SCREEN` ends the task at the first failing edit
/// [app/cbl/CORPT00C.cbl:L556-580] (P-CORPT00C-002), the six empty checks
/// form one `EVALUATE` in screen order, the value edits follow them, the two
/// `CSUTLDTC` checks come next and the confirmation last.
fn precedence_cases() -> Vec<(String, &'static [&'static str])> {
    vec![
        (
            "report_type=custom&confirm=Y".to_owned(),
            &[
                "V-CORPT00C-002",
                "V-CORPT00C-003",
                "V-CORPT00C-004",
                "V-CORPT00C-005",
                "V-CORPT00C-006",
                "V-CORPT00C-007",
            ],
        ),
        (
            custom("2022-13-01", "2022--06"),
            &["V-CORPT00C-005", "V-CORPT00C-008"],
        ),
        (
            custom("2022-01-32", "2022-13-06"),
            &[
                "V-CORPT00C-009",
                "V-CORPT00C-011",
                "V-CORPT00C-014",
                "V-CORPT00C-015",
            ],
        ),
        (
            custom("2022-02-30", "2022-13-06"),
            &["V-CORPT00C-011", "V-CORPT00C-014", "V-CORPT00C-015"],
        ),
        (
            custom("2022-02-30", "2022-04-31"),
            &["V-CORPT00C-014", "V-CORPT00C-015"],
        ),
        (
            confirmed("2022-02-30", VALID_END, "X"),
            &["V-CORPT00C-014", "V-CORPT00C-018"],
        ),
        (
            "confirm=X".to_owned(),
            &["V-CORPT00C-001", "V-CORPT00C-018"],
        ),
    ]
}

/// What a reviewed `V-CORPT00C` row records for its rejection.
struct AuditedEdit {
    /// `Status`.
    status: StatusCode,
    /// The body `code` of the row's `kind`.
    code: &'static str,
    /// `BMS field`: the `fields[].field` of the answer.
    field: String,
    /// `Order`: the rank at which the COBOL reaches the rejection.
    order: u16,
    /// `Message text`, as written.
    message_cell: String,
}

/// The rejection row `id` of the validation audit records.
///
/// # Panics
///
/// Panics, naming the row, if a cell is missing or malformed.
fn audited_edit(doc: &Document, id: &str) -> AuditedEdit {
    let (table, row) = audit_row(doc, id);
    let status = parse_number::<u16>(cell(table, row, STATUS), id);
    let kind = key_value(cell(table, row, TRIGGER_CONDITION), "kind")
        .unwrap_or_else(|| panic!("validation row {id} names no kind="));
    let code = match kind.as_str() {
        "Empty" => ErrorKind::Empty.code(),
        "Invalid" => ErrorKind::Invalid.code(),
        other => panic!("validation row {id}: kind={other} is no CORPT00C edit kind"),
    };
    AuditedEdit {
        status: StatusCode::from_u16(status)
            .unwrap_or_else(|error| panic!("validation row {id}: status {status}: {error}")),
        code,
        field: cell(table, row, BMS_FIELD).to_owned(),
        order: parse_number(cell(table, row, ORDER), id),
        message_cell: cell(table, row, MESSAGE_TEXT).to_owned(),
    }
}

/// Asserts that the catalog text of `case` is the audit's message text: the
/// quoted literal, with its `STRING` variable given the input's value, or no
/// message linking the escalation that decides its status.
fn assert_catalog_matches(case: &EditCase, audited: &AuditedEdit) {
    let id = case.id;
    if carddemo_audit::is_no_message(&audited.message_cell) {
        assert_eq!(
            carddemo_audit::no_message_escalation(&audited.message_cell).as_deref(),
            Some("E-49"),
            "validation row {id} must be the no-message path E-49 decides"
        );
        assert!(case.message.is_empty(), "{id} sets no message");
        return;
    }
    let span = carddemo_audit::code_span(&audited.message_cell)
        .unwrap_or_else(|| panic!("validation row {id} quotes no message"));
    let mut text = unquote(&span.text, id);
    if let Some((variable, value)) = case.variable {
        assert!(
            text.contains(variable),
            "{id}: the audit text names no {variable}"
        );
        text = text.replace(variable, value);
    }
    assert_eq!(text, case.message, "{id}: catalog text against the audit");
}

/// The path of a report request with the query string `query`.
fn report_path(query: &str) -> String {
    format!("{REPORT_PATH}?{query}")
}

/// Every `V-CORPT00C` row, each sent once with its example, answered as the
/// audit records it: status, code, message and BMS field; then the
/// first-failure order across rows, and the bearer-token requirement.
#[test]
fn report_request_edits_answer_the_audited_rows() {
    let doc = common::validation_audit();
    let cases = edit_cases();
    let case_ids: Vec<&str> = cases.iter().map(|case| case.id).collect();
    assert_eq!(
        case_ids,
        common::validation_ids(PROGRAM),
        "one edit case per {PROGRAM} validation row, in audit order"
    );
    assert_reviewed(&doc, case_ids.iter().copied());
    let audited: BTreeMap<&str, AuditedEdit> = cases
        .iter()
        .map(|case| (case.id, audited_edit(&doc, case.id)))
        .collect();
    for case in &cases {
        assert_catalog_matches(case, &audited[case.id]);
        let field = &audited[case.id].field;
        // Each audited field is one the error body's published enum declares.
        serde_json::from_value::<ReportRequestField>(json!(field)).unwrap_or_else(|error| {
            panic!("{}: {field} is no ReportRequestField: {error}", case.id)
        });
    }

    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let token = app.user_token().await;
        for case in &cases {
            let resp = app.get(&report_path(&case.query), Some(&token)).await;
            assert_edit_answer(&resp, &audited[case.id], &case.message, case.id);
        }
        for (query, failing) in precedence_cases() {
            let first = failing
                .iter()
                .copied()
                .min_by_key(|id| (audited[id].order, *id))
                .unwrap_or_else(|| panic!("{query}: no failing row listed"));
            let case = cases
                .iter()
                .find(|case| case.id == first)
                .unwrap_or_else(|| panic!("{first} has no edit case"));
            let resp = app.get(&report_path(&query), Some(&token)).await;
            assert_edit_answer(&resp, &audited[first], &case.message, &query);
        }
        let resp = app.get(&report_path(VALID_RANGE), None).await;
        common::assert_error(
            &resp,
            StatusCode::UNAUTHORIZED,
            UNAUTHENTICATED,
            messages::AUTHENTICATION_REQUIRED,
            &[],
        );
    });
}

/// Asserts that `resp` is the rejection `audited` records, carrying `message`
/// on its BMS field.
fn assert_edit_answer(resp: &Resp, audited: &AuditedEdit, message: &str, case: &str) {
    assert_eq!(resp.0, audited.status, "{case}: status: {}", resp.2);
    common::assert_error(
        resp,
        audited.status,
        audited.code,
        message,
        &[(audited.field.as_str(), message)],
    );
}

// ---------------------------------------------------------------------------
// 2. The monthly and yearly ranges (P-CORPT00C-003, P-CORPT00C-004)
// ---------------------------------------------------------------------------

/// The monthly and yearly reports of [`CLOCK`]: the report name, the first
/// and last processing dates, and the confirmation value sent.
///
/// Monthly runs from the first to the last day of the current month
/// (P-CORPT00C-003), yearly from 1 January to 31 December of the current year
/// (P-CORPT00C-004); `Y` and `y` both confirm (P-CORPT00C-012).
const RANGES: [(&str, &str, &str, &str, &str); 3] = [
    (
        "monthly",
        messages::REPORT_NAME_MONTHLY,
        "2022-07-01",
        "2022-07-31",
        "Y",
    ),
    (
        "yearly",
        messages::REPORT_NAME_YEARLY,
        "2022-01-01",
        "2022-12-31",
        "Y",
    ),
    (
        "yearly",
        messages::REPORT_NAME_YEARLY,
        "2022-01-01",
        "2022-12-31",
        "y",
    ),
];

/// The rows the range test proves: the two ranges, the confirmation that runs
/// the report, the submitted text, and the date format of section F.
const RANGE_ROWS: [&str; 5] = [
    "P-CORPT00C-003",
    "P-CORPT00C-004",
    "P-CORPT00C-012",
    "P-CORPT00C-014",
    "P-CORPT00C-017",
];

/// A confirmed monthly or yearly report answers 200 with the range of the
/// pinned clock and `<name> report submitted for printing ...`.
#[test]
fn monthly_and_yearly_ranges_follow_the_pinned_clock() {
    assert_reviewed(&common::parity_audit(), RANGE_ROWS);
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let token = app.user_token().await;
        for (report_type, name, start, end, confirm) in RANGES {
            let query = format!("report_type={report_type}&confirm={confirm}");
            let report = report_of(&app.get(&report_path(&query), Some(&token)).await, &query);
            assert_eq!(report.report_name, name, "{query}: report name");
            assert_eq!(
                (report.start_date.as_str(), report.end_date.as_str()),
                (start, end),
                "{query}: the range of {CLOCK}"
            );
            for date in [&report.start_date, &report.end_date] {
                assert!(is_iso_date(date), "{query}: {date:?} is not YYYY-MM-DD");
            }
            assert_eq!(
                report.message,
                messages::report_submitted_message(name),
                "{query}: submitted text"
            );
        }
    });
}

/// The 200 report of `resp`, the answer to `query`.
///
/// # Panics
///
/// Panics, showing the raw body, unless `resp` is 200 with a `ReportResponse`.
fn report_of(resp: &Resp, query: &str) -> ReportResponse {
    let (status, body, raw) = resp;
    assert_eq!(*status, StatusCode::OK, "{query}: {raw}");
    serde_json::from_value(body.clone())
        .unwrap_or_else(|error| panic!("{query}: the body is no ReportResponse ({error}): {raw}"))
}

/// The format P-CORPT00C-017 pins for a report date: four digits, `-`, two
/// digits, `-`, two digits.
fn is_iso_date(text: &str) -> bool {
    text.len() == 10
        && text.bytes().enumerate().all(|(at, byte)| match at {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        })
}

// ---------------------------------------------------------------------------
// 3. The report lines over the processing copies (D.6, AAP 0.3.2)
// ---------------------------------------------------------------------------

/// The custom report of the posted fixture prints the D.6 lines exactly, and
/// still prints them after the Db2 master type `01` is edited: the report
/// reads `trantype_file`, which only `batch seed --only tranextr` refreshes.
#[test]
fn custom_report_prints_the_tranrept_lines_from_the_processing_copies() {
    let doc = common::parity_audit();
    let expected = TranreptExpectation::from_audit(&doc);
    let posted = posted_records(&doc);
    assert_eq!(
        posted.len(),
        expected.records_in_range,
        "{TRANREPT_RUN}: every posted record is in the range"
    );
    let query = custom(&expected.start, &expected.end);

    let app = TestApp::start_with_clock(CLOCK);
    let lines = carddemo_testkit::block_on(async {
        write_posted(&app, &posted, expected.proc_ts).await;
        let token = app.user_token().await;
        let report = report_of(&app.get(&report_path(&query), Some(&token)).await, &query);
        assert_eq!(
            report.report_name,
            messages::REPORT_NAME_CUSTOM,
            "report name"
        );
        assert_eq!(
            (report.start_date.as_str(), report.end_date.as_str()),
            (expected.start.as_str(), expected.end.as_str()),
            "the requested range"
        );
        assert_eq!(
            report.message,
            messages::report_submitted_message(messages::REPORT_NAME_CUSTOM),
            "submitted text"
        );
        assert_lines(&report.lines, &expected.lines);

        edit_master_type_01(&app, &doc).await;
        let again = report_of(&app.get(&report_path(&query), Some(&token)).await, &query);
        assert_lines(&again.lines, &report.lines);
        report.lines
    });
    insta::assert_yaml_snapshot!("report_lines", lines);
}

/// The TRANREPT run section D.6 records, with the processing timestamp of
/// the records it reports.
struct TranreptExpectation {
    /// The first processing date of the range, `start=` of [`TRANREPT_RUN`].
    start: String,
    /// The last processing date of the range, `end=`.
    end: String,
    /// `records_in_range`: the posted records the range selects.
    records_in_range: usize,
    /// Every report line in order, right-padded to `width`.
    lines: Vec<String>,
    /// The `TRAN-PROC-TS` POSTTRAN stamps under [`CLOCK`] ([`PROC_TS_ROW`]).
    proc_ts: Alnum<26>,
}

impl TranreptExpectation {
    /// The expectation of the reviewed D.6 rows: the run's range and counts,
    /// then the `line_no`, `text` and `width` of P-CBTRN03C-016 to
    /// P-CBTRN03C-466, in `line_no` order.
    ///
    /// # Panics
    ///
    /// Panics, naming the row, if a row is unreviewed or malformed, if the line
    /// numbers do not run from 1 without a gap, or if the count or a width
    /// differs from the run's.
    fn from_audit(doc: &Document) -> Self {
        let input = row_cell(doc, TRANREPT_RUN, INPUT_CONDITION);
        let run = expected_cell(doc, TRANREPT_RUN);
        let count = |key: &str| -> usize {
            let value =
                key_value(run, key).unwrap_or_else(|| panic!("{TRANREPT_RUN} expects no {key}="));
            parse_number(&value, TRANREPT_RUN)
        };
        let width = count("width");
        let mut numbered = BTreeMap::new();
        let mut ids = vec![TRANREPT_RUN, PROC_TS_ROW];
        for (table, row) in subsection_rows(doc, TRANREPT_LINES) {
            let id = cell(table, row, ID);
            let result = cell(table, row, EXPECTED_RESULT);
            let Some(line_no) = key_value(result, "line_no") else {
                continue;
            };
            let text = exact_text(result, "text", id);
            assert_eq!(text.len(), width, "{id}: line width");
            numbered.insert(parse_number::<usize>(&line_no, id), text);
            ids.push(id);
        }
        assert_reviewed(doc, ids);
        let line_numbers: Vec<usize> = numbered.keys().copied().collect();
        assert_eq!(
            line_numbers,
            (1..=count("lines")).collect::<Vec<usize>>(),
            "{TRANREPT_LINES}lines: one row per report line, numbered from 1"
        );
        Self {
            start: key_value(input, "start")
                .unwrap_or_else(|| panic!("{TRANREPT_RUN} names no start=")),
            end: key_value(input, "end").unwrap_or_else(|| panic!("{TRANREPT_RUN} names no end=")),
            records_in_range: count("records_in_range"),
            lines: numbered.into_values().collect(),
            proc_ts: Alnum::move_from(&exact_text(
                expected_cell(doc, PROC_TS_ROW),
                "pinned",
                PROC_TS_ROW,
            )),
        }
    }
}

/// Asserts that `actual` equals `expected` line for line, untrimmed, naming
/// the first line that differs.
fn assert_lines(actual: &[String], expected: &[String]) {
    for (index, (got, want)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(got, want, "report line {}", index + 1);
    }
    assert_eq!(actual.len(), expected.len(), "number of report lines");
}

/// The daily records `batch posttran` posts from `app/data/ASCII/dailytran.txt`
/// in the seeded state, in file order: every record but the 38 of the D.1
/// reject set, checked against the counts of [`POSTTRAN_COUNTS`].
///
/// # Panics
///
/// Panics if the fixture cannot be read, a record is not 350 bytes or does
/// not decode, a reject row names a record with another TRAN-ID, a row is
/// unreviewed, or a count differs from the audit's.
fn posted_records(doc: &Document) -> Vec<DailyTran> {
    let layout = dalytran_layout(doc);
    let record_len = layout[layout.len() - 1].end;
    let path = fixtures::ascii_dir().join("dailytran.txt");
    let bytes =
        fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let records: Vec<&[u8]> = bytes
        .split(|&byte| byte == b'\n')
        .filter(|record| !record.is_empty())
        .collect();
    let mut rejected = BTreeSet::new();
    let mut ids = vec![POSTTRAN_COUNTS];
    for (table, row) in subsection_rows(doc, REJECT_SET) {
        let id = cell(table, row, ID);
        let result = cell(table, row, EXPECTED_RESULT);
        let Some(record_no) = key_value(result, "record_no") else {
            continue;
        };
        let record_no: usize = parse_number(&record_no, id);
        let tran_id = key_value(result, "tran_id").unwrap_or_else(|| panic!("{id}: no tran_id="));
        let record = record_no
            .checked_sub(1)
            .and_then(|index| records.get(index))
            .unwrap_or_else(|| panic!("{id}: dailytran.txt has no record {record_no}"));
        assert_eq!(
            &record[layout[0].clone()],
            tran_id.as_bytes(),
            "{id}: rejected record"
        );
        rejected.insert(record_no);
        ids.push(id);
    }
    assert_reviewed(doc, ids);
    let counts = expected_cell(doc, POSTTRAN_COUNTS);
    let count = |key: &str| -> usize {
        let value =
            key_value(counts, key).unwrap_or_else(|| panic!("{POSTTRAN_COUNTS}: no {key}="));
        parse_number(&value, POSTTRAN_COUNTS)
    };
    assert_eq!(records.len(), count("read"), "daily records read");
    assert_eq!(rejected.len(), count("rejected"), "daily records rejected");
    let posted: Vec<DailyTran> = records
        .iter()
        .enumerate()
        .filter(|(index, _)| !rejected.contains(&(index + 1)))
        .map(|(index, record)| {
            assert_eq!(
                record.len(),
                record_len,
                "dailytran.txt record {}",
                index + 1
            );
            daily_tran(record, &layout)
        })
        .collect();
    assert_eq!(posted.len(), count("posted"), "daily records posted");
    posted
}

/// Writes `posted` into the live master as `CBTRN02C 2000-POST-TRANSACTION`
/// does (P-CBTRN02C-013, P-CBTRN02C-014): each daily record moved field by
/// field, `TRAN-PROC-TS` set to `proc_ts`, in file order, in one unit of work.
///
/// The records go through the posting job's own write, the `TransactionWriter`
/// port of `2900-WRITE-TRANSACTION-FILE` (P-CBTRN02C-019), not through
/// `POST /transactions` or SQL text: CT02 cannot produce the fixture TRAN-IDs
/// or the `CBTRN02C` processing timestamp, and the test SQL of
/// `TestStack::exec_sql` is static text, never built from values. The report
/// reads nothing else `batch posttran` changes (accounts and category
/// balances), so this is the D.6 state the report sees.
async fn write_posted(app: &TestApp, posted: &[DailyTran], proc_ts: Alnum<26>) {
    let db = Db::from_pool(app.stack().pool().clone());
    let mut uow = db
        .begin()
        .await
        .unwrap_or_else(|error| panic!("opening the posting unit of work failed: {error}"));
    {
        let mut store = uow.store();
        for record in posted {
            store
                .write_transaction(record.to_new_transaction(proc_ts))
                .await
                .unwrap_or_else(|error| {
                    panic!("posting {} failed: {error}", record.dalytran_id.trimmed())
                });
        }
    }
    uow.commit()
        .await
        .unwrap_or_else(|error| panic!("committing the posted records failed: {error}"));
}

/// Changes the Db2 master description of type `01` through
/// `PUT /admin/transaction-types/01` (CTTU, no `origin`), and asserts that
/// only `transaction_types` changed: `trantype_file` keeps the fixture text.
async fn edit_master_type_01(app: &TestApp, doc: &Document) {
    assert_reviewed(doc, [MASTER_TYPE_ROW, PROCESSING_TYPE_ROW]);
    let master = exact_text(
        expected_cell(doc, MASTER_TYPE_ROW),
        "tr_description",
        MASTER_TYPE_ROW,
    );
    let processing = exact_text(
        expected_cell(doc, PROCESSING_TYPE_ROW),
        "tran_type_desc",
        PROCESSING_TYPE_ROW,
    );
    // VARCHAR columns hold the text right-trimmed of the X(n) padding.
    let processing = processing.trim_end_matches(' ');
    assert_eq!(
        description(app, MASTER_TYPE_01).await,
        master,
        "seeded master"
    );
    assert_eq!(
        description(app, PROCESSING_TYPE_01).await,
        processing,
        "seeded copy"
    );

    let admin = app.admin_token().await;
    let body = json!({ "description": NEW_TYPE_DESCRIPTION });
    let (status, answer, raw) = app
        .put("/admin/transaction-types/01", Some(&admin), &body)
        .await;
    assert_eq!(status, StatusCode::OK, "type 01 update: {raw}");
    assert_eq!(
        answer["message"],
        Value::from(messages::CHANGES_COMMITTED_TO_DATABASE),
        "type 01 update: {raw}"
    );
    assert_eq!(
        description(app, MASTER_TYPE_01).await,
        NEW_TYPE_DESCRIPTION,
        "the Db2 master after the update"
    );
    assert_eq!(
        description(app, PROCESSING_TYPE_01).await,
        processing,
        "the processing copy after the update"
    );
}

/// The one text column of the one row `sql` reads.
async fn description(app: &TestApp, sql: &str) -> String {
    let rows = app.stack().fetch_text_rows(sql).await;
    match rows.as_slice() {
        [row] => match row.as_slice() {
            [Some(text)] => text.clone(),
            other => panic!("{sql}: expected one non-null column, got {other:?}"),
        },
        other => panic!("{sql}: expected one row, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. The 3270 mechanics the one GET replaces (P-CORPT00C-001,
//    P-CORPT00C-009 to P-CORPT00C-011, P-CORPT00C-018 to P-CORPT00C-021)
// ---------------------------------------------------------------------------

/// The keys of an `OpenAPI` path item that name operations.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The query parameters of the CR00 operation, all `in: query`: the report
/// type and the two dates that replace the received map (P-CORPT00C-019,
/// E-48), and `confirm`, the `CONFIRM` field V-CORPT00C-016 to V-CORPT00C-018
/// are reached through.
const REPORT_PARAMETERS: [&str; 4] = ["report_type", "start_date", "end_date", "confirm"];

/// The schema of a 200 report in the published document.
const REPORT_SCHEMA: &str = "ReportResponse";

/// The members of a 200 report: the report, its range and the message line;
/// no `CORPT0AO` header field and no form field (P-CORPT00C-020,
/// P-CORPT00C-021).
const REPORT_MEMBERS: [&str; 5] = ["end_date", "lines", "message", "report_name", "start_date"];

/// The members of every error body.
const ERROR_MEMBERS: [&str; 3] = ["code", "fields", "message"];

/// A request every edit accepts: the monthly report, confirmed.
const MONTHLY_CONFIRMED: &str = "report_type=monthly&confirm=Y";

/// The AID keys other than ENTER, each sent in a query parameter the
/// operation does not declare: PF3, which `RETURN-TO-PREV-SCREEN` serves, and
/// keys `WHEN OTHER` answers (P-CORPT00C-001).
const AID_KEYS: [&str; 4] = ["PF3", "PF4", "CLEAR", "PA1"];

/// The three report selections, each with every custom edit passing, and the
/// `WS-REPORT-NAME` each sets.
const SELECTIONS: [(&str, &str); 3] = [
    ("report_type=monthly", messages::REPORT_NAME_MONTHLY),
    ("report_type=yearly", messages::REPORT_NAME_YEARLY),
    (VALID_RANGE, messages::REPORT_NAME_CUSTOM),
];

/// What a request could write: the transaction rows, the account balances and
/// the reward accruals.
const WRITTEN_STATE: &str = "SELECT (SELECT count(*) FROM transactions)::text, \
     (SELECT sum(acct_curr_bal) FROM accounts)::text, \
     (SELECT count(*) FROM reward_accruals)::text";

/// The rows the published document proves: ENTER and the other AID keys, the
/// session the bearer token stands for, the received map and the header.
const SURFACE_ROWS: [&str; 4] = [
    "P-CORPT00C-001",
    "P-CORPT00C-018",
    "P-CORPT00C-019",
    "P-CORPT00C-020",
];

/// The rows the mechanics test proves through the running API.
const MECHANICS_ROWS: [&str; 8] = [
    "P-CORPT00C-001",
    "P-CORPT00C-009",
    "P-CORPT00C-010",
    "P-CORPT00C-011",
    "P-CORPT00C-018",
    "P-CORPT00C-019",
    "P-CORPT00C-020",
    "P-CORPT00C-021",
];

/// CR00 publishes one operation, the GET of ENTER: PF3 and every other AID
/// key have none, the query replaces the received map and carries no key, the
/// operation needs the bearer token that stands for the COMMAREA, and its
/// report carries no 3270 header field.
#[test]
fn cr00_publishes_one_get_whose_query_replaces_the_map() {
    let doc = common::parity_audit();
    assert_reviewed(&doc, SURFACE_ROWS);
    // WHEN OTHER moves CCDA-MSG-INVALID-KEY, the catalog literal in its X(50)
    // field; no request reaches it (E-56).
    let invalid_key = exact_text(
        expected_cell(&doc, "P-CORPT00C-001"),
        "action",
        "P-CORPT00C-001",
    );
    let width = invalid_key.len();
    let catalog = messages::CCDA_MSG_INVALID_KEY;
    assert_eq!(
        format!("{catalog:<width$}"),
        invalid_key,
        "P-CORPT00C-001: the WHEN OTHER text"
    );
    assert_replacement(&doc, "P-CORPT00C-018", "no token gives 401");
    assert_replacement(
        &doc,
        "P-CORPT00C-019",
        "query parameters of GET /reports/transactions",
    );
    assert_replacement(&doc, "P-CORPT00C-020", "no API field carries the header");

    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    let item = &spec["paths"][REPORT_PATH];
    assert_eq!(
        operations(item),
        BTreeSet::from(["get"]),
        "P-CORPT00C-001, P-CORPT00C-018: ENTER is the one GET; PF3 and the other AID keys \
         have no operation: {item}"
    );
    let get = &item["get"];
    let parameters: BTreeSet<(&str, &str)> = get["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("the CR00 operation declares no parameters: {get}"))
        .iter()
        .map(|parameter| {
            (
                parameter["name"].as_str().unwrap_or_default(),
                parameter["in"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    let expected: BTreeSet<(&str, &str)> = REPORT_PARAMETERS
        .iter()
        .map(|name| (*name, "query"))
        .collect();
    assert_eq!(
        parameters, expected,
        "P-CORPT00C-019, P-CORPT00C-001: the query replaces the received map, and no \
         parameter carries an AID key"
    );
    assert_eq!(
        get["security"],
        json!([{ "bearer_auth": [] }]),
        "P-CORPT00C-018: a request without a session is refused"
    );
    assert_eq!(
        get["responses"]["200"]["content"]["application/json"]["schema"]["$ref"],
        Value::from(format!("#/components/schemas/{REPORT_SCHEMA}")),
        "P-CORPT00C-020: the 200 answer is the report"
    );
    assert_eq!(
        members(&spec["components"]["schemas"][REPORT_SCHEMA]["properties"]),
        BTreeSet::from(REPORT_MEMBERS),
        "P-CORPT00C-020: no member carries the title, transaction, program, date or time \
         of the header"
    );
}

/// The screen mechanics through the running API: a MAPFAIL and the AID keys
/// other than ENTER, a missing session, a missing report type, each class of
/// confirmation for each report, and the emptied form; none writes.
#[test]
fn cr00_screen_mechanics_answer_through_the_one_get() {
    let parity = common::parity_audit();
    let validation = common::validation_audit();
    assert_reviewed(&parity, MECHANICS_ROWS);
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let before = app.stack().fetch_text_rows(WRITTEN_STATE).await;
        let token = app.user_token().await;
        received_map_and_keys(&app, &token, &validation).await;
        without_a_session(&app).await;
        no_report_type(&app, &token, &parity, &validation).await;
        confirmations(&app, &token, &parity, &validation).await;
        initialized_form(&app, &token, &parity, &validation).await;
        assert_eq!(
            app.stack().fetch_text_rows(WRITTEN_STATE).await,
            before,
            "P-CORPT00C-018, P-CORPT00C-021: CR00 writes nothing"
        );
    });
}

/// P-CORPT00C-019: a request without a query, the received map of a MAPFAIL,
/// still runs `PROCESS-ENTER-KEY`, whose first edit answers (V-CORPT00C-001).
/// P-CORPT00C-001: a parameter naming an AID key is no input of the
/// operation, so every key answers as ENTER does with the same query, never
/// with the invalid-key text. P-CORPT00C-020: neither answer carries a header
/// field.
async fn received_map_and_keys(app: &TestApp, token: &str, validation: &Document) {
    let first = audited_edit(validation, "V-CORPT00C-001");
    let empty = app.get(REPORT_PATH, Some(token)).await;
    assert_edit_answer(
        &empty,
        &first,
        messages::SELECT_A_REPORT_TYPE_TO_PRINT_REPORT,
        "P-CORPT00C-019: no query",
    );
    assert_eq!(
        members(&empty.1),
        BTreeSet::from(ERROR_MEMBERS),
        "P-CORPT00C-020: the error body: {}",
        empty.2
    );
    let report = app.get(&report_path(MONTHLY_CONFIRMED), Some(token)).await;
    assert_eq!(
        report.0,
        StatusCode::OK,
        "{MONTHLY_CONFIRMED}: {}",
        report.2
    );
    assert_eq!(
        members(&report.1),
        BTreeSet::from(REPORT_MEMBERS),
        "P-CORPT00C-020: the report: {}",
        report.2
    );
    for (query, enter) in [("", &empty), (MONTHLY_CONFIRMED, &report)] {
        for key in AID_KEYS {
            let keyed = app
                .get(&report_path(&format!("{query}&aid={key}")), Some(token))
                .await;
            assert_eq!(
                (keyed.0, &keyed.1),
                (enter.0, &enter.1),
                "P-CORPT00C-001: {key} with {query:?} answers as ENTER: {}",
                keyed.2
            );
        }
    }
}

/// P-CORPT00C-018: `EIBCALEN = 0` sends the terminal to the sign-on, so a
/// request without a valid bearer token answers 401 with the authentication
/// text (P-COMEN01C-001) and runs no edit.
async fn without_a_session(app: &TestApp) {
    for token in [None, Some("not-a-bearer-token")] {
        let resp = app.get(&report_path(MONTHLY_CONFIRMED), token).await;
        common::assert_error(
            &resp,
            StatusCode::UNAUTHORIZED,
            UNAUTHENTICATED,
            messages::AUTHENTICATION_REQUIRED,
            &[],
        );
    }
}

/// P-CORPT00C-009: no selector entered, absent, empty, `SPACES` or
/// `LOW-VALUES`, answers the audited prompt, every date valid and the request
/// confirmed.
async fn no_report_type(app: &TestApp, token: &str, parity: &Document, validation: &Document) {
    let text = exact_text(
        expected_cell(parity, "P-CORPT00C-009"),
        "error",
        "P-CORPT00C-009",
    );
    assert_eq!(
        text,
        messages::SELECT_A_REPORT_TYPE_TO_PRINT_REPORT,
        "P-CORPT00C-009: the catalog text"
    );
    let audited = audited_edit(validation, "V-CORPT00C-001");
    for selector in ["", "report_type=", "report_type=%20", "report_type=%00"] {
        let query = format!("{selector}&start_date={VALID_START}&end_date={VALID_END}&confirm=Y");
        let resp = app.get(&report_path(&query), Some(token)).await;
        assert_edit_answer(&resp, &audited, &text, &format!("P-CORPT00C-009: {query}"));
    }
}

/// P-CORPT00C-010 and P-CORPT00C-011: for each report, a blank confirmation
/// (absent, empty, `SPACES` or `LOW-VALUES`) asks for it, naming the report;
/// `N` or `n` clears the form with no message; any other value is quoted back.
/// None runs the report.
async fn confirmations(app: &TestApp, token: &str, parity: &Document, validation: &Document) {
    let [head, tail] = quoted_texts::<2>(parity, "P-CORPT00C-010");
    let [open, close] = quoted_texts::<2>(parity, "P-CORPT00C-011");
    let prompt = audited_edit(validation, "V-CORPT00C-016");
    let cleared = audited_edit(validation, "V-CORPT00C-017");
    let invalid = audited_edit(validation, "V-CORPT00C-018");
    for (selection, name) in SELECTIONS {
        let text = format!("{head}{name}{tail}");
        assert_eq!(
            text,
            messages::confirm_print_report_message(name),
            "P-CORPT00C-010: the {name} prompt"
        );
        for confirm in ["", "&confirm=", "&confirm=%20", "&confirm=%00"] {
            let query = format!("{selection}{confirm}");
            let resp = app.get(&report_path(&query), Some(token)).await;
            assert_edit_answer(&resp, &prompt, &text, &format!("P-CORPT00C-010: {query}"));
        }
        for confirm in ["N", "n"] {
            let query = format!("{selection}&confirm={confirm}");
            let resp = app.get(&report_path(&query), Some(token)).await;
            assert_edit_answer(&resp, &cleared, "", &format!("P-CORPT00C-011: {query}"));
        }
        for value in ["X", "x", "1", "*"] {
            let text = format!("{open}{value}{close}");
            assert_eq!(
                text,
                messages::invalid_confirm_value_message(value),
                "P-CORPT00C-011: {value} quoted back"
            );
            let query = format!("{selection}&confirm={value}");
            let resp = app.get(&report_path(&query), Some(token)).await;
            assert_edit_answer(&resp, &invalid, &text, &format!("P-CORPT00C-011: {query}"));
        }
    }
}

/// P-CORPT00C-021: `INITIALIZE-ALL-FIELDS` empties the form. Once the job is
/// written it runs before the submitted text is built, so a confirmed report
/// answers that text alone and returns no form field; on `N` it leaves no
/// message, the cursor on `MONTHLY`, and no report.
async fn initialized_form(app: &TestApp, token: &str, parity: &Document, validation: &Document) {
    assert_replacement(
        parity,
        "P-CORPT00C-021",
        "a confirmed report is GET /reports/transactions",
    );
    let cleared = audited_edit(validation, "V-CORPT00C-017");
    for (selection, name) in SELECTIONS {
        let query = format!("{selection}&confirm=Y");
        let resp = app.get(&report_path(&query), Some(token)).await;
        let report = report_of(&resp, &query);
        assert_eq!(
            report.message,
            messages::report_submitted_message(name),
            "P-CORPT00C-021: {query}: the message holds the submitted text alone"
        );
        assert_eq!(
            members(&resp.1),
            BTreeSet::from(REPORT_MEMBERS),
            "P-CORPT00C-021: {query}: no form field comes back: {}",
            resp.2
        );
        let query = format!("{selection}&confirm=N");
        let resp = app.get(&report_path(&query), Some(token)).await;
        assert_edit_answer(&resp, &cleared, "", &format!("P-CORPT00C-021: {query}"));
        assert_eq!(
            members(&resp.1),
            BTreeSet::from(ERROR_MEMBERS),
            "P-CORPT00C-021: {query}: no report: {}",
            resp.2
        );
    }
}

/// The operations `item`, a published path item, declares.
fn operations(item: &Value) -> BTreeSet<&str> {
    item.as_object()
        .unwrap_or_else(|| panic!("the path is not published: {item}"))
        .keys()
        .map(String::as_str)
        .filter(|key| HTTP_METHODS.contains(key))
        .collect()
}

/// The member names of the JSON object `value`.
fn members(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{value} is not a JSON object"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Asserts that the `replacement=` of row `id`, which runs to the end of its
/// cell across `;`, names `replacement`: the REST counterpart a test proves is
/// the one the reviewed row records.
fn assert_replacement(doc: &Document, id: &str, replacement: &str) {
    let Some((_, named)) = expected_cell(doc, id).split_once("replacement=") else {
        panic!("{id} names no replacement=");
    };
    assert!(
        named.contains(replacement),
        "{id}: replacement={named} does not name {replacement:?}"
    );
}

/// The exact texts of the `N` code spans of the expected result of row `id`,
/// unquoted, in order.
///
/// # Panics
///
/// Panics, naming the row, if it quotes another number of texts.
fn quoted_texts<const N: usize>(doc: &Document, id: &str) -> [String; N] {
    let texts: Vec<String> = carddemo_audit::cell_code_spans(expected_cell(doc, id))
        .iter()
        .map(|span| unquote(&span.text, id))
        .collect();
    <[String; N]>::try_from(texts)
        .unwrap_or_else(|texts| panic!("{id} quotes {} texts, not {N}: {texts:?}", texts.len()))
}

// ---------------------------------------------------------------------------
// 5. The custom date parts (P-CORPT00C-005 to P-CORPT00C-008,
//    P-CORPT00C-015, P-CORPT00C-016)
// ---------------------------------------------------------------------------

/// The six date parts in screen order, `SDTMM`, `SDTDD`, `SDTYYYY`, `EDTMM`,
/// `EDTDD` and `EDTYYYY`, of [`VALID_RANGE`].
const VALID_PARTS: [&str; 6] = ["01", "01", "2022", "07", "06", "2022"];

/// The validation rows of the six empty checks, in screen order.
const EMPTY_ROWS: [&str; 6] = [
    "V-CORPT00C-002",
    "V-CORPT00C-003",
    "V-CORPT00C-004",
    "V-CORPT00C-005",
    "V-CORPT00C-006",
    "V-CORPT00C-007",
];

/// The validation rows of the six value edits, in screen order.
const VALUE_ROWS: [&str; 6] = [
    "V-CORPT00C-008",
    "V-CORPT00C-009",
    "V-CORPT00C-010",
    "V-CORPT00C-011",
    "V-CORPT00C-012",
    "V-CORPT00C-013",
];

/// The validation rows of the start and the end `CSUTLDTC` check.
const DATE_ROWS: [&str; 2] = ["V-CORPT00C-014", "V-CORPT00C-015"];

/// Blank parts as the query carries them: empty, and `SPACES` (`%20`) up to
/// the width of each map field.
const BLANK_PARTS: [[&str; 6]; 2] = [
    ["", "", "", "", "", ""],
    [
        "%20",
        "%20%20",
        "%20%20%20%20",
        "%20%20",
        "%20",
        "%20%20%20",
    ],
];

/// Parts each failing its value edit after the conversion: a month above
/// `'12'`, a day above `'31'`, and a year `NUMVAL-C` does not read, which
/// only the E-28 proposal rejects.
const INVALID_PARTS: [&str; 6] = ["13", "32", "20A2", "13", "32", "20A2"];

/// Malformed `NUMVAL-C` arguments, one per part (E-28).
const MALFORMED_PARTS: [&str; 6] = ["1A", "B1", "2O22", "A", "1B", "Z"];

/// Custom dates as entered, and the dates the conversion and the move back
/// give them: digits short of the field, leading spaces, a leading `+` and a
/// trailing `-` whose sign the unsigned receiver drops, and fractions that
/// round up to 1 (P-CORPT00C-006, P-CORPT00C-015).
const CONVERTED: [(&str, &str, &str, &str); 5] = [
    ("2022-1-5", "2022-7-6", "2022-01-05", "2022-07-06"),
    (
        "2022-%201-%205",
        "2022-%207-%206",
        "2022-01-05",
        "2022-07-06",
    ),
    (
        "2022-%2B1-%2B5",
        "2022-%2B7-%2B6",
        "2022-01-05",
        "2022-07-06",
    ),
    ("2022-01-5-", "2022-07-6-", "2022-01-05", "2022-07-06"),
    ("2022-.6-05", "2022-07-.6", "2022-01-05", "2022-07-01"),
];

/// The rows the custom date test proves, with the `CSUTLDTC` row of the
/// dates before 1582-10-15 it relies on.
const CUSTOM_ROWS: [&str; 7] = [
    "P-CORPT00C-005",
    "P-CORPT00C-006",
    "P-CORPT00C-007",
    "P-CORPT00C-008",
    "P-CORPT00C-015",
    "P-CORPT00C-016",
    "P-CSUTLDTC-004",
];

/// The API under test and the validation audit, for the date part cases.
struct DateParts<'a> {
    /// The running API.
    app: &'a TestApp,
    /// A signed-on user's bearer token.
    token: &'a str,
    /// The `V-CORPT00C` rows.
    validation: &'a Document,
}

impl DateParts<'_> {
    /// Asserts that the confirmed custom report of `parts`, in screen order,
    /// is refused as validation row `id` records, with `message`.
    async fn refused(&self, parts: [&str; 6], id: &str, message: &str, case: &str) {
        let [
            start_month,
            start_day,
            start_year,
            end_month,
            end_day,
            end_year,
        ] = parts;
        let query = custom(
            &format!("{start_year}-{start_month}-{start_day}"),
            &format!("{end_year}-{end_month}-{end_day}"),
        );
        let resp = self.app.get(&report_path(&query), Some(self.token)).await;
        let audited = audited_edit(self.validation, id);
        assert_edit_answer(&resp, &audited, message, &format!("{case}: {query}"));
    }

    /// Asserts that the confirmed custom report of `start` to `end`, as
    /// entered, runs over the range `range` with the submitted text.
    async fn reported(&self, (start, end): (&str, &str), range: (&str, &str), case: &str) {
        let query = custom(start, end);
        let resp = self.app.get(&report_path(&query), Some(self.token)).await;
        let report = report_of(&resp, &query);
        assert_eq!(
            (report.start_date.as_str(), report.end_date.as_str()),
            range,
            "{case}: {query}: the range"
        );
        assert_eq!(
            report.message,
            messages::report_submitted_message(messages::REPORT_NAME_CUSTOM),
            "{case}: {query}: the submitted text"
        );
    }
}

/// Input `k` of a staircase over `bad`: the parts before `k` valid, part `k`
/// and every later part from `bad`, so the edit of part `k` fails first.
fn staircase(bad: [&'static str; 6], k: usize) -> [&'static str; 6] {
    std::array::from_fn(|at| if at < k { VALID_PARTS[at] } else { bad[at] })
}

/// `VALID_PARTS` with part `at` replaced by `part`.
fn with_part(at: usize, part: &'static str) -> [&'static str; 6] {
    let mut parts = VALID_PARTS;
    parts[at] = part;
    parts
}

/// A custom report's six date parts through the empty checks, the
/// `NUMVAL-C` conversion, the value edits and `CSUTLDTC`, each answered as
/// the reviewed rows record.
#[test]
fn custom_date_parts_convert_and_edit_in_the_cobol_order() {
    let parity = common::parity_audit();
    let validation = common::validation_audit();
    assert_reviewed(&parity, CUSTOM_ROWS);
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let token = app.user_token().await;
        let parts = DateParts {
            app: &app,
            token: &token,
            validation: &validation,
        };
        empty_checks(&parts, &parity).await;
        value_edits(&parts, &parity).await;
        date_checks(&parts, &parity).await;
        conversions(&parts, &parity).await;
    });
}

/// P-CORPT00C-005: the six empty checks run in screen order, an empty part
/// and `SPACES` alike blank, so the first blank part answers whatever follows
/// it; and every empty check precedes every value edit.
async fn empty_checks(parts: &DateParts<'_>, parity: &Document) {
    let texts = quoted_texts::<6>(parity, "P-CORPT00C-005");
    for blank in BLANK_PARTS {
        for (k, (id, text)) in EMPTY_ROWS.iter().zip(&texts).enumerate() {
            parts
                .refused(staircase(blank, k), id, text, "P-CORPT00C-005")
                .await;
        }
    }
    let mut late_blank = with_part(0, INVALID_PARTS[0]);
    late_blank[5] = "";
    parts
        .refused(
            late_blank,
            EMPTY_ROWS[5],
            &texts[5],
            "P-CORPT00C-005: a blank end year before an invalid start month",
        )
        .await;
}

/// P-CORPT00C-007: after the conversion the six value edits run in screen
/// order, the first failing part answering; a month or day of `00` passes
/// them and reaches `CSUTLDTC`, which rejects its date (P-CORPT00C-008).
async fn value_edits(parts: &DateParts<'_>, parity: &Document) {
    let texts = quoted_texts::<6>(parity, "P-CORPT00C-007");
    for (k, (id, text)) in VALUE_ROWS.iter().zip(&texts).enumerate() {
        parts
            .refused(staircase(INVALID_PARTS, k), id, text, "P-CORPT00C-007")
            .await;
    }
    let [start, end] = quoted_texts::<2>(parity, "P-CORPT00C-008");
    for (at, id, text) in [
        (0, DATE_ROWS[0], &start),
        (1, DATE_ROWS[0], &start),
        (3, DATE_ROWS[1], &end),
        (4, DATE_ROWS[1], &end),
    ] {
        parts
            .refused(
                with_part(at, "00"),
                id,
                text,
                "P-CORPT00C-007: 00 passes the value edit",
            )
            .await;
    }
}

/// P-CORPT00C-008: `CSUTLDTC` checks the start date, then the end date, and
/// rejects a date that is not on the calendar; a date before 1582-10-15
/// passes on message 2513 (P-CSUTLDTC-004), and a start after the end is
/// never checked.
async fn date_checks(parts: &DateParts<'_>, parity: &Document) {
    let [start, end] = quoted_texts::<2>(parity, "P-CORPT00C-008");
    let rejected = [
        (
            ["02", "30", "2022", "07", "06", "2022"],
            DATE_ROWS[0],
            &start,
        ),
        (["01", "01", "2022", "04", "31", "2022"], DATE_ROWS[1], &end),
        (
            ["02", "30", "2022", "04", "31", "2022"],
            DATE_ROWS[0],
            &start,
        ),
    ];
    for (dates, id, text) in rejected {
        parts.refused(dates, id, text, "P-CORPT00C-008").await;
    }
    let accepted = [("2022-07-06", "2022-01-01"), ("1500-01-01", "1500-12-31")];
    for range in accepted {
        parts.reported(range, range, "P-CORPT00C-008").await;
    }
}

/// P-CORPT00C-006 and P-CORPT00C-015: `NUMVAL-C` converts every part before
/// any numeric test, digits exactly, a sign dropped by the unsigned receiver
/// and a fraction rounded, and the move back zero-fills the map field.
/// P-CORPT00C-016: malformed text is the E-28 halt, and its proposal rejects
/// the part with its own value message.
async fn conversions(parts: &DateParts<'_>, parity: &Document) {
    for (start, end, converted_start, converted_end) in CONVERTED {
        parts
            .reported(
                (start, end),
                (converted_start, converted_end),
                "P-CORPT00C-006, P-CORPT00C-015",
            )
            .await;
    }
    let anomaly = expected_cell(parity, "P-CORPT00C-016");
    assert_eq!(
        (
            key_value(anomaly, "escalation").as_deref(),
            key_value(anomaly, "disposition").as_deref(),
        ),
        (Some("E-28"), Some("halt")),
        "P-CORPT00C-016: the malformed arguments are the E-28 halt"
    );
    let texts = quoted_texts::<6>(parity, "P-CORPT00C-007");
    for (at, (id, text)) in VALUE_ROWS.iter().zip(&texts).enumerate() {
        parts
            .refused(
                with_part(at, MALFORMED_PARTS[at]),
                id,
                text,
                "P-CORPT00C-016: the E-28 proposal",
            )
            .await;
    }
}

// ---------------------------------------------------------------------------
// 6. The TDQ write the synchronous report replaces (P-CORPT00C-013)
// ---------------------------------------------------------------------------

/// `WIRTE-JOBSUB-TDQ`: the `WRITEQ TD` failure and its text.
const TDQ_ROW: &str = "P-CORPT00C-013";

/// Each relation the report reads, hidden and then put back: the live master
/// the `STEP05R` selection reads, and the cross-reference, the type and the
/// category processing copies of the `CBTRN03C` lookups.
const REPORT_READ_FAULTS: [(&str, &str); 4] = [
    (
        "ALTER VIEW transaction_master RENAME TO transaction_master_cr00_hidden",
        "ALTER VIEW transaction_master_cr00_hidden RENAME TO transaction_master",
    ),
    (
        "ALTER TABLE card_xref RENAME TO card_xref_cr00_hidden",
        "ALTER TABLE card_xref_cr00_hidden RENAME TO card_xref",
    ),
    (
        "ALTER TABLE trantype_file RENAME TO trantype_file_cr00_hidden",
        "ALTER TABLE trantype_file_cr00_hidden RENAME TO trantype_file",
    ),
    (
        "ALTER TABLE trancatg_file RENAME TO trancatg_file_cr00_hidden",
        "ALTER TABLE trancatg_file_cr00_hidden RENAME TO trancatg_file",
    ),
];

/// A confirmed request writes no TDQ `JOBS`: it runs the report synchronously
/// (P-CORPT00C-012, E-48), so the `WRITEQ TD` failure and its text have no
/// counterpart. A synchronous run that fails instead, the live master or a
/// table its lookups read being missing, answers 500 `Unexpected error` with
/// no fields, never the TDQ text; once the relation is back, the same request
/// prints the D.6 report again.
#[test]
fn a_failed_synchronous_report_answers_unexpected_error_not_the_tdq_text() {
    let doc = common::parity_audit();
    assert_reviewed(&doc, [TDQ_ROW, "P-CORPT00C-012"]);
    let tdq = exact_text(expected_cell(&doc, TDQ_ROW), "error", TDQ_ROW);
    assert_replacement(
        &doc,
        TDQ_ROW,
        "GET /reports/transactions runs the report synchronously and writes no TDQ",
    );
    let unexpected = messages::UNEXPECTED_ERROR;
    assert_replacement(
        &doc,
        TDQ_ROW,
        &format!("500 `\"{unexpected}\"` with no fields"),
    );
    let expected = TranreptExpectation::from_audit(&doc);
    let posted = posted_records(&doc);
    let query = custom(&expected.start, &expected.end);

    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        write_posted(&app, &posted, expected.proc_ts).await;
        let token = app.user_token().await;
        let path = report_path(&query);
        let report = report_of(&app.get(&path, Some(&token)).await, &query);
        assert_lines(&report.lines, &expected.lines);
        for (hide, restore) in REPORT_READ_FAULTS {
            app.stack().exec_sql(hide).await;
            let failed = app.get(&path, Some(&token)).await;
            app.stack().exec_sql(restore).await;
            assert!(
                !failed.2.contains(&tdq),
                "{TDQ_ROW}: {hide}: the answer carries the TDQ text: {}",
                failed.2
            );
            common::assert_error(
                &failed,
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorKind::Unexpected.code(),
                unexpected,
                &[],
            );
            let again = report_of(&app.get(&path, Some(&token)).await, &query);
            assert_lines(&again.lines, &report.lines);
        }
    });
}

// ---------------------------------------------------------------------------
// The daily record layout (P-CVTRA06Y-001 to P-CVTRA06Y-014)
// ---------------------------------------------------------------------------

/// The byte ranges of the [`DALYTRAN_FIELDS`], from the `Offset` and
/// `Length` of their reviewed section A rows.
fn dalytran_layout(doc: &Document) -> [Range<usize>; 14] {
    assert_reviewed(doc, DALYTRAN_FIELDS);
    DALYTRAN_FIELDS.map(|id| {
        let offset: usize = parse_number(row_cell(doc, id, "Offset"), id);
        let length: usize = parse_number(row_cell(doc, id, "Length"), id);
        offset..offset + length
    })
}

/// The `DALYTRAN-RECORD` held by `record`, each field decoded by its PIC
/// clause: `X(n)` copied byte for byte, `9(n)` read as its digits, and the
/// `S9(09)V99` amount by its trailing overpunch.
fn daily_tran(record: &[u8], layout: &[Range<usize>; 14]) -> DailyTran {
    let [
        id,
        type_cd,
        cat_cd,
        source,
        desc,
        amt,
        merchant_id,
        merchant_name,
        merchant_city,
        merchant_zip,
        card_num,
        orig_ts,
        proc_ts,
        _filler,
    ] = layout;
    DailyTran {
        dalytran_id: TranId::from_alnum(alnum(record, id)),
        dalytran_type_cd: TranTypeCode::from_alnum(alnum(record, type_cd)),
        dalytran_cat_cd: digits(record, cat_cd),
        dalytran_source: alnum(record, source),
        dalytran_desc: alnum(record, desc),
        dalytran_amt: Money::from_cents(overpunch_cents(&record[amt.clone()]))
            .unwrap_or_else(|error| panic!("DALYTRAN-AMT out of S9(09)V99: {error}")),
        dalytran_merchant_id: digits(record, merchant_id),
        dalytran_merchant_name: alnum(record, merchant_name),
        dalytran_merchant_city: alnum(record, merchant_city),
        dalytran_merchant_zip: alnum(record, merchant_zip),
        dalytran_card_num: CardNumber::from_alnum(alnum(record, card_num)),
        dalytran_orig_ts: alnum(record, orig_ts),
        dalytran_proc_ts: alnum(record, proc_ts),
    }
}

/// The `X(N)` field at `range` of `record`.
fn alnum<const N: usize>(record: &[u8], range: &Range<usize>) -> Alnum<N> {
    let bytes = &record[range.clone()];
    assert_eq!(bytes.len(), N, "an X({N}) field of {} bytes", bytes.len());
    Alnum::move_from_bytes(bytes)
}

/// The unsigned `9(n)` field at `range` of `record`, read as its digits.
fn digits<T>(record: &[u8], range: &Range<usize>) -> T
where
    T: FromStr,
    T::Err: Debug,
{
    let bytes = &record[range.clone()];
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or_else(|| panic!("the 9(n) field {bytes:?} holds no digits"))
}

/// The cents of a signed zoned `S9(n)V99` field whose last byte carries the
/// sign as an ASCII overpunch: `{` and `A` to `I` are +0 to +9, `}` and `J` to
/// `R` are -0 to -9 (AAP 0.5.3.2).
fn overpunch_cents(field: &[u8]) -> i64 {
    let Some((&last, leading)) = field.split_last() else {
        panic!("an empty signed field");
    };
    let (digit, negative) = match last {
        b'{' => (0, false),
        b'A'..=b'I' => (last - b'A' + 1, false),
        b'}' => (0, true),
        b'J'..=b'R' => (last - b'J' + 1, true),
        other => panic!("{:?} carries no overpunch sign", char::from(other)),
    };
    let zoned = leading.iter().copied().chain(std::iter::once(b'0' + digit));
    let magnitude = zoned.fold(0_i64, |value, byte| {
        assert!(byte.is_ascii_digit(), "{field:?} is not zoned decimal");
        value * 10 + i64::from(byte - b'0')
    });
    if negative { -magnitude } else { magnitude }
}

// ---------------------------------------------------------------------------
// Audit reading
// ---------------------------------------------------------------------------

/// The data row `id` of `doc`, with its table.
fn audit_row<'d>(doc: &'d Document, id: &str) -> (&'d Table, &'d Row) {
    doc.find_row(id)
        .unwrap_or_else(|| panic!("the audit has no row {id}"))
}

/// The cell of `row` under `column`.
fn cell<'d>(table: &'d Table, row: &'d Row, column: &str) -> &'d str {
    table
        .cell(row, column)
        .unwrap_or_else(|| panic!("audit row at line {} has no {column:?} cell", row.line))
}

/// The cell of row `id` under `column`.
fn row_cell<'d>(doc: &'d Document, id: &str, column: &str) -> &'d str {
    let (table, row) = audit_row(doc, id);
    cell(table, row, column)
}

/// The `Expected result` cell of row `id`.
fn expected_cell<'d>(doc: &'d Document, id: &str) -> &'d str {
    row_cell(doc, id, EXPECTED_RESULT)
}

/// Every row of the section D data tables whose subsection starts with
/// `subsection`, in document order, with its table.
fn subsection_rows<'d>(
    doc: &'d Document,
    subsection: &'d str,
) -> impl Iterator<Item = (&'d Table, &'d Row)> {
    doc.section_tables("D.")
        .filter(move |table| {
            table.is_data_table()
                && table
                    .subsection
                    .as_deref()
                    .is_some_and(|name| name.starts_with(subsection))
        })
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
}

/// The value of the first `key=` pair of a structured cell, backticks kept.
fn key_value(cell: &str, key: &str) -> Option<String> {
    carddemo_audit::key_values(cell)
        .into_iter()
        .find(|pair| pair.key == key)
        .map(|pair| pair.value)
}

/// The exact text of the code span of the `key=` pair of `cell`, unquoted and
/// right-padded with spaces to its `width=<n>` when it has one.
fn exact_text(cell: &str, key: &str, id: &str) -> String {
    let value = key_value(cell, key).unwrap_or_else(|| panic!("{id}: no {key}="));
    let span = carddemo_audit::code_span(&value)
        .unwrap_or_else(|| panic!("{id}: {key}={value} holds no code span"));
    let text = unquote(&span.text, id);
    match span.width {
        Some(width) => {
            assert!(
                text.len() <= width,
                "{id}: {key} is longer than width={width}"
            );
            format!("{text:<width$}")
        }
        None => text,
    }
}

/// The value of the double-quoted audit string `content`, whose only escapes
/// are `\"` and `\\`.
fn unquote(content: &str, id: &str) -> String {
    let inner = content
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id}: {content:?} is not a double-quoted exact text"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                _ => panic!("{id}: {content:?} holds a backslash escaping no quote or backslash"),
            },
            '"' => panic!("{id}: {content:?} holds an unescaped quote"),
            _ => text.push(ch),
        }
    }
    text
}

/// `text` read as a number, the cell of row `id`.
fn parse_number<T>(text: &str, id: &str) -> T
where
    T: FromStr,
    T::Err: Debug,
{
    text.trim()
        .parse()
        .unwrap_or_else(|error| panic!("{id}: {text:?} is no number: {error:?}"))
}

/// Asserts that the last review verdict of each of `ids` is `pass`: no test
/// relies on a row its COBOL-informed review has not passed.
fn assert_reviewed<'a>(doc: &Document, ids: impl IntoIterator<Item = &'a str>) {
    let mut verdicts = BTreeMap::new();
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            verdicts.insert(cell(table, row, ROW_ID), cell(table, row, VERDICT));
        }
    }
    for id in ids {
        assert_eq!(
            verdicts.get(id).copied(),
            Some(PASS),
            "audit row {id} has no passing review"
        );
    }
}
