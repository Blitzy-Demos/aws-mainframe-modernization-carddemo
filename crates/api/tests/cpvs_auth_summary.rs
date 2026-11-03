//! CPVS, `COPAUS0C` with map `COPAU00`: the pending authorizations of one account,
//! `GET /authorizations?account_id=&start_key=&direction=&page_size=`, served by
//! `list_authorizations`.
//!
//! The IMS tables `pending_auth_summaries` and `pending_auth_details` start empty, because
//! only CP00, CPVD and `cbpaup0j` write them, so each test that lists authorizations writes
//! one `PAUTSUM0` summary and seven `PAUTDTL1` details for account 1 first. The details are
//! inserted out of key order, so a page that follows the `PAUT9CTS` key cannot be the
//! identity order by accident.
//!
//! Expected texts come from `carddemo_domain::messages`, the `STRING`-built ones through its
//! composers, with the `RESP`, `RESP2` and IMS status values E-44 sets. Each error's status
//! and BMS field are read from its `COPAUS0C` row of the validation audit, and each literal
//! text is checked against the row's quoted text. A test cites every audit row it proves as
//! a string literal; [`every_copaus0c_audit_row_is_exercised_here`] holds both tables of
//! cited rows to the audits and to their passing reviews.
//!
//! Docker must be running for the three tests that start a [`TestApp`].

pub mod common;

use std::collections::BTreeSet;

use axum::extract::State;
use carddemo_api::AppState;
use carddemo_api::auth::{AuthUser, UserType};
use carddemo_api::dto::authorizations::{AuthSummaryPageQuery, AuthSummaryQuery};
use carddemo_api::error::ApiQuery;
use carddemo_api::handlers::authorizations::list_authorizations;
use carddemo_audit::{Document, IdKind, code_span, ids_with_prefix};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages::{self, AuthLookupFailure};
use carddemo_persistence::Db;
use carddemo_runtime::config::Settings;
use carddemo_testkit::block_on;
use common::{Resp, TestApp, assert_error, parity_audit, validation_audit, validation_ids};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The processing clock of the API suites (AAP 0.9.3).
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// The CPVS route.
const ROUTE: &str = "/authorizations";

/// The program whose audit rows this file proves.
const PROGRAM: &str = "COPAUS0C";

/// The start of every `COPAUS0C` parity row ID.
const PARITY_PREFIX: &str = "P-COPAUS0C-";

/// Account 1 of the fixture as `ACCTIDI` holds it: card `9680294154603697` and customer 1
/// [app/data/ASCII/cardxref.txt:L49]. The tests give it the pending authorizations.
const ACCOUNT_1: &str = "00000000001";

/// Customer 1, the cross-reference customer of account 1, as `CUST-ID 9(09)` displays it.
const CUSTOMER_1: &str = "000000001";

/// Account 2 of the fixture, which keeps no pending authorization summary.
const ACCOUNT_2: &str = "00000000002";

/// An account no cross-reference names [app/data/ASCII/cardxref.txt].
const UNKNOWN_ACCOUNT: &str = "99999999999";

/// The account of [`ORPHAN_XREFS`] whose cross-reference names no account record.
const XREF_WITHOUT_ACCOUNT: &str = "99999999998";

/// The account of [`ORPHAN_XREFS`] whose cross-reference names no customer record.
const XREF_WITHOUT_CUSTOMER: &str = "99999999997";

/// The customer [`XREF_WITHOUT_CUSTOMER`]'s cross-reference names, which no record holds.
const MISSING_CUSTOMER: &str = "999999997";

/// The `RESP` and `RESP2` of a `READ` that ends `NOTFND` (E-44).
const RESP_NOTFND: (u32, u32) = (13, 80);

/// The `RESP` (`IOERR`) and `RESP2` E-44 sets for any other database failure.
const RESP_IOERR: (u32, u32) = (17, 0);

/// The IMS status E-44 sets for any failed authorization-store access.
const IMS_STATUS_OTHER: &str = "AO";

/// `PA-AUTH-RESP-CODE` of an approved authorization, which `POPULATE-AUTH-LIST` lists as
/// [`APPROVED_FLAG`], every other code as [`DECLINED_FLAG`]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L536-540].
const APPROVED_RESPONSE: &str = "00";

/// `MOVE 'A' TO WS-AUTH-APRV-STAT`.
const APPROVED_FLAG: &str = "A";

/// `MOVE 'D' TO WS-AUTH-APRV-STAT`.
const DECLINED_FLAG: &str = "D";

/// The rows of one `COPAU00` page: `SEL0001` to `SEL0005`.
const SCREEN_ROWS: usize = 5;

/// The `PAUTSUM0` root of account 1. Its own credit and cash limits differ from the account
/// record's, which the screen shows instead (`CREDLIMO`, `CASHLIMO`), and the cash balance is
/// negative, so the sign crosses the wire. The counts and amounts sum the details below:
/// four approved, three declined.
const INSERT_SUMMARY: &str = "
    INSERT INTO pending_auth_summaries (
        pa_acct_id, pa_cust_id, pa_auth_status, pa_account_status, pa_credit_limit,
        pa_cash_limit, pa_credit_balance, pa_cash_balance, pa_approved_auth_cnt,
        pa_declined_auth_cnt, pa_approved_auth_amt, pa_declined_auth_amt)
    VALUES (1, 1, 'A', ARRAY['Y', '', '', '', ''], 9999.00, 8888.00, 1234.56, -78.90, 4, 3,
            475.49, 5250.24)";

/// Seven `PAUTDTL1` twins under account 1's summary, inserted in `ord` order, which is not
/// their key order. COPAUA0C keys a detail `99999 - YYDDD` and `999999999 - HHMMSSmmm`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L874-875], so ascending keys run from
/// the newest authorization (`220705` `093015`, `TRN000000000001`) to the oldest (`220701`
/// `120000`, `TRN000000000007`). Responses `00` alternate with `05`.
const INSERT_DETAILS: &str = "
    INSERT INTO pending_auth_details (
        pa_acct_id, pa_auth_date_9c, pa_auth_time_9c, pa_auth_orig_date, pa_auth_orig_time,
        pa_card_num, pa_auth_type, pa_card_expiry_date, pa_message_type, pa_message_source,
        pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason, pa_processing_code,
        pa_transaction_amt, pa_approved_amt, pa_merchant_catagory_code, pa_acqr_country_code,
        pa_pos_entry_mode, pa_merchant_id, pa_merchant_name, pa_merchant_city,
        pa_merchant_state, pa_merchant_zip, pa_transaction_id, pa_match_status,
        pa_auth_fraud, pa_fraud_rpt_date)
    SELECT 1, date_9c, time_9c, orig_date, orig_time, '9680294154603697', auth_type, '1225',
           '0100', 'POS', id_code, resp_code, resp_reason, 0, tran_amt, approved_amt, '5411',
           '840', 1, 'CDMERCH00000001', 'CARDDEMO MARKET', 'SPRINGFIELD', 'IL', '62701',
           tran_id, match_status, '', ''
      FROM (VALUES
            (1, 77817, 879999999, '220701', '120000', 'SALE', 'A00007', '00', '0000',
             300.00, 300.00, 'TRN000000000007', 'P'),
            (2, 77814, 829999999, '220704', '170000', 'SALE', 'A00003', '00', '0000',
             45.00, 45.00, 'TRN000000000003', 'M'),
            (3, 77813, 906984999, '220705', '093015', 'SALE', 'A00001', '00', '0000',
             120.50, 120.50, 'TRN000000000001', 'P'),
            (4, 77816, 859999999, '220702', '140000', 'SALE', '', '05', '4100',
             999.99, 0.00, 'TRN000000000006', 'D'),
            (5, 77815, 889499999, '220703', '110500', 'CASH', '', '05', '4200',
             2750.25, 0.00, 'TRN000000000004', 'D'),
            (6, 77813, 918799999, '220705', '081200', 'SALE', '', '05', '4100',
             1500.00, 0.00, 'TRN000000000002', 'D'),
            (7, 77815, 919999999, '220703', '080000', 'SALE', 'A00005', '00', '0000',
             9.99, 9.99, 'TRN000000000005', 'P'))
        AS fixture (ord, date_9c, time_9c, orig_date, orig_time, auth_type, id_code,
                    resp_code, resp_reason, tran_amt, approved_amt, tran_id, match_status)
     ORDER BY ord";

/// The `PA-TRANSACTION-ID`s of [`INSERT_DETAILS`], newest authorization first.
const NEWEST_FIRST: [&str; 7] = [
    "TRN000000000001",
    "TRN000000000002",
    "TRN000000000003",
    "TRN000000000004",
    "TRN000000000005",
    "TRN000000000006",
    "TRN000000000007",
];

/// Account 1's details in `PAUT9CTS` order, ascending (`pa_auth_date_9c`,
/// `pa_auth_time_9c`), with the stored fields a listed line carries; read by
/// [`listed_line`].
const DETAILS_IN_KEY_ORDER: &str = "
    SELECT auth_id, pa_transaction_id, pa_auth_orig_date, pa_auth_orig_time, pa_auth_type,
           pa_auth_resp_code, pa_match_status, pa_approved_amt
      FROM pending_auth_details
     WHERE pa_acct_id = 1
     ORDER BY pa_auth_date_9c, pa_auth_time_9c";

/// The header records of accounts 1 and 2, in the wire form of [`HEADER_FIELDS`]: the
/// cross-reference by account (`CXACAIX`), its account and its customer.
const HEADER_RECORDS: &str = "
    SELECT lpad(x.xref_acct_id::text, 11, '0'), lpad(c.customer_id::text, 9, '0'),
           c.cust_first_name, c.cust_middle_name, c.cust_last_name, c.cust_addr_line_1,
           c.cust_addr_line_2, c.cust_addr_line_3, c.cust_addr_state_cd, c.cust_addr_zip,
           c.cust_phone_num_1, a.acct_credit_limit, a.acct_cash_credit_limit
      FROM card_xref x
      JOIN accounts a ON a.account_id = x.xref_acct_id
      JOIN customers c ON c.customer_id = x.xref_cust_id
     WHERE x.xref_acct_id IN (1, 2)
     ORDER BY x.xref_acct_id";

/// The response fields [`HEADER_RECORDS`] gives, column by column, as
/// `GATHER-ACCOUNT-DETAILS` fills `CUSTIDO`, `CNAMEO`, `ADDR001O`, `ADDR002O`, `PHONE1O`,
/// `CREDLIMO` and `CASHLIMO` [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L757-783].
const HEADER_FIELDS: [&str; 13] = [
    "account_id",
    "customer_id",
    "cust_first_name",
    "cust_middle_name",
    "cust_last_name",
    "cust_addr_line_1",
    "cust_addr_line_2",
    "cust_addr_line_3",
    "cust_addr_state_cd",
    "cust_addr_zip",
    "cust_phone_num_1",
    "acct_credit_limit",
    "acct_cash_credit_limit",
];

/// Account 1's summary counters in the order of [`SUMMARY_FIELDS`].
const SUMMARY_COUNTERS: &str = "
    SELECT pa_approved_auth_cnt, pa_declined_auth_cnt, pa_credit_balance, pa_cash_balance,
           pa_approved_auth_amt, pa_declined_auth_amt
      FROM pending_auth_summaries
     WHERE pa_acct_id = 1";

/// The response fields of the summary segment: `APPRCNTO`, `DECLCNTO`, `CREDBALO`,
/// `CASHBALO`, `APPRAMTO` and `DECLAMTO`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L787-807].
const SUMMARY_FIELDS: [&str; 6] = [
    "pa_approved_auth_cnt",
    "pa_declined_auth_cnt",
    "pa_credit_balance",
    "pa_cash_balance",
    "pa_approved_auth_amt",
    "pa_declined_auth_amt",
];

/// [`SUMMARY_FIELDS`] of an account without a summary segment, `MOVE ZERO TO APPRCNTO …
/// DECLAMTO` [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L800-806].
const NO_SUMMARY: [&str; 6] = ["0", "0", "0.00", "0.00", "0.00", "0.00"];

/// Every field of a page answer that shows no boundary text. No title, transaction name,
/// program name, date or time of the 3270 header is among them.
const PAGE_FIELDS: [&str; 22] = [
    "account_id",
    "customer_id",
    "cust_first_name",
    "cust_middle_name",
    "cust_last_name",
    "cust_addr_line_1",
    "cust_addr_line_2",
    "cust_addr_line_3",
    "cust_addr_state_cd",
    "cust_addr_zip",
    "cust_phone_num_1",
    "acct_credit_limit",
    "acct_cash_credit_limit",
    "pa_approved_auth_cnt",
    "pa_declined_auth_cnt",
    "pa_credit_balance",
    "pa_cash_balance",
    "pa_approved_auth_amt",
    "pa_declined_auth_amt",
    "items",
    "next_key",
    "prev_key",
];

/// Everything stored in the IMS and fraud tables, for the check that CPVS writes nothing.
const IMS_STATE: &str = "
    SELECT * FROM pending_auth_summaries ORDER BY pa_acct_id;
    SELECT * FROM pending_auth_details ORDER BY auth_id;
    SELECT count(*) FROM auth_fraud";

/// Two cross-references the fixture lacks: one whose account has no record, and one whose
/// account record (a copy of account 1) names a customer no record holds. The tables carry
/// no foreign keys, as the VSAM files carry none.
const ORPHAN_XREFS: &str = "
    INSERT INTO card_xref (xref_card_num, xref_cust_id, xref_acct_id)
    VALUES ('0000000000009998', 1, 99999999998),
           ('0000000000009997', 999999997, 99999999997);
    INSERT INTO accounts (
        account_id, acct_active_status, acct_curr_bal, acct_credit_limit,
        acct_cash_credit_limit, acct_open_date, acct_expiraion_date, acct_reissue_date,
        acct_curr_cyc_credit, acct_curr_cyc_debit, acct_addr_zip, acct_group_id)
    SELECT 99999999997, acct_active_status, acct_curr_bal, acct_credit_limit,
           acct_cash_credit_limit, acct_open_date, acct_expiraion_date, acct_reissue_date,
           acct_curr_cyc_credit, acct_curr_cyc_debit, acct_addr_zip, acct_group_id
      FROM accounts
     WHERE account_id = 1";

/// Hides the card cross-reference file: its read then fails on the server (SQLSTATE `42P01`).
const HIDE_XREF: &str = "ALTER TABLE card_xref RENAME TO card_xref_offline";

/// Puts the card cross-reference file back.
const RESTORE_XREF: &str = "ALTER TABLE card_xref_offline RENAME TO card_xref";

/// Hides the account master.
const HIDE_ACCOUNTS: &str = "ALTER TABLE accounts RENAME TO accounts_offline";

/// Puts the account master back.
const RESTORE_ACCOUNTS: &str = "ALTER TABLE accounts_offline RENAME TO accounts";

/// Hides the customer master.
const HIDE_CUSTOMERS: &str = "ALTER TABLE customers RENAME TO customers_offline";

/// Puts the customer master back.
const RESTORE_CUSTOMERS: &str = "ALTER TABLE customers_offline RENAME TO customers";

/// Hides the `PAUTSUM0` segments.
const HIDE_SUMMARIES: &str =
    "ALTER TABLE pending_auth_summaries RENAME TO pending_auth_summaries_offline";

/// Puts the `PAUTSUM0` segments back.
const RESTORE_SUMMARIES: &str =
    "ALTER TABLE pending_auth_summaries_offline RENAME TO pending_auth_summaries";

/// Hides the `PAUTDTL1` segments.
const HIDE_DETAILS: &str =
    "ALTER TABLE pending_auth_details RENAME TO pending_auth_details_offline";

/// Puts the `PAUTDTL1` segments back.
const RESTORE_DETAILS: &str =
    "ALTER TABLE pending_auth_details_offline RENAME TO pending_auth_details";

/// The signed-on user of the request [`closed_store_answer`] hands the handler.
const SIGNED_ON_USER: &str = "USER0001";

/// [`cpvs_lists_the_account_header_and_pages_its_authorizations`].
const SUMMARY_TEST: &str = "cpvs_lists_the_account_header_and_pages_its_authorizations";

/// [`cpvs_key_edits_and_failed_lookups_answer_on_acctid`].
const EDITS_TEST: &str = "cpvs_key_edits_and_failed_lookups_answer_on_acctid";

/// [`cpvs_failed_reads_answer_with_the_system_error_text`].
const FAULTS_TEST: &str = "cpvs_failed_reads_answer_with_the_system_error_text";

/// [`cpvs_publishes_no_row_selector_or_aid_key_parameter`].
const SURFACE_TEST: &str = "cpvs_publishes_no_row_selector_or_aid_key_parameter";

/// Every `COPAUS0C` validation row with the test that sends its input. The selector row has
/// no API input (E-54), so its test asserts the published surface declares no selector.
const VALIDATION_ROWS: [(&str, &str); 15] = [
    ("V-COPAUS0C-001", EDITS_TEST),
    ("V-COPAUS0C-002", EDITS_TEST),
    ("V-COPAUS0C-003", SURFACE_TEST),
    ("V-COPAUS0C-004", EDITS_TEST),
    ("V-COPAUS0C-005", FAULTS_TEST),
    ("V-COPAUS0C-006", EDITS_TEST),
    ("V-COPAUS0C-007", FAULTS_TEST),
    ("V-COPAUS0C-008", EDITS_TEST),
    ("V-COPAUS0C-009", FAULTS_TEST),
    ("V-COPAUS0C-010", FAULTS_TEST),
    ("V-COPAUS0C-011", FAULTS_TEST),
    ("V-COPAUS0C-012", FAULTS_TEST),
    ("V-COPAUS0C-013", SUMMARY_TEST),
    ("V-COPAUS0C-014", SUMMARY_TEST),
    ("V-COPAUS0C-015", FAULTS_TEST),
];

/// Every `COPAUS0C` parity row with the test that proves it. The rows whose disposition is
/// 3270 navigation (PF3, the AID dispatch, the received map, the selectors) are proved on
/// the API side by the published surface: no operation or parameter carries them.
const PARITY_ROWS: [(&str, &str); 21] = [
    ("P-COPAUS0C-001", SUMMARY_TEST),
    ("P-COPAUS0C-002", SURFACE_TEST),
    ("P-COPAUS0C-003", EDITS_TEST),
    ("P-COPAUS0C-004", SUMMARY_TEST),
    ("P-COPAUS0C-005", SURFACE_TEST),
    ("P-COPAUS0C-006", SUMMARY_TEST),
    ("P-COPAUS0C-007", EDITS_TEST),
    ("P-COPAUS0C-008", EDITS_TEST),
    ("P-COPAUS0C-009", SUMMARY_TEST),
    ("P-COPAUS0C-010", SUMMARY_TEST),
    ("P-COPAUS0C-011", SUMMARY_TEST),
    ("P-COPAUS0C-012", SUMMARY_TEST),
    ("P-COPAUS0C-013", SUMMARY_TEST),
    ("P-COPAUS0C-014", FAULTS_TEST),
    ("P-COPAUS0C-015", SUMMARY_TEST),
    ("P-COPAUS0C-016", SUMMARY_TEST),
    ("P-COPAUS0C-017", SURFACE_TEST),
    ("P-COPAUS0C-018", SURFACE_TEST),
    ("P-COPAUS0C-019", SUMMARY_TEST),
    ("P-COPAUS0C-020", FAULTS_TEST),
    ("P-COPAUS0C-021", FAULTS_TEST),
];

/// One injected read failure and the validation row whose answer it must give.
struct Fault {
    /// The validation row.
    id: &'static str,
    /// The statement that makes the read fail.
    hide: &'static str,
    /// The statement that undoes [`Fault::hide`].
    restore: &'static str,
    /// Whether the request continues from a key (PF7), whose first DL/I call repositions.
    keyed: bool,
    /// The text the answer carries.
    message: String,
}

#[test]
fn cpvs_lists_the_account_header_and_pages_its_authorizations() {
    let validation = validation_audit();
    let parity = parity_audit();
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        insert_authorizations(&app).await;
        let token = app.user_token().await;
        let before = app.stack().fetch_text_rows(IMS_STATE).await;

        let first = first_page(&app, &token, &parity).await;
        later_pages(&app, &token, &first, &parity).await;
        boundaries(&app, &token, &validation, &parity).await;
        detail_link(&app, &token, &first, &parity).await;
        account_without_summary(&app, &token, &parity).await;

        // SEND-PAULST-SCREEN only releases the PSB: CPVS writes nothing.
        proves(&parity, &["P-COPAUS0C-015"]);
        let after = app.stack().fetch_text_rows(IMS_STATE).await;
        assert_eq!(
            after, before,
            "CPVS must leave the IMS and fraud tables unchanged"
        );
    });
}

/// ENTER with account 1: the header records, the summary counters and the first five
/// details in `PAUT9CTS` order, newest first, with the key of the next page.
async fn first_page(app: &TestApp, token: &str, parity: &Document) -> Value {
    proves(
        parity,
        &[
            "P-COPAUS0C-001",
            "P-COPAUS0C-006",
            "P-COPAUS0C-010",
            "P-COPAUS0C-013",
            "P-COPAUS0C-019",
        ],
    );
    let lines = listed_lines(app).await;
    let body = page_ok(app, token, &summary_path(ACCOUNT_1)).await;
    assert_eq!(
        field_names(&body),
        PAGE_FIELDS.into_iter().collect::<BTreeSet<_>>(),
        "a page carries the screen's data fields and no 3270 header field: {body}"
    );
    assert_header(app, &body, ACCOUNT_1).await;
    let counters = app.stack().fetch_text_rows(SUMMARY_COUNTERS).await;
    let counters: Vec<&str> = counters
        .first()
        .unwrap_or_else(|| panic!("account 1 has no summary segment"))
        .iter()
        .map(|cell| cell.as_deref().unwrap_or_default())
        .collect();
    assert_summary(&body, &counters);
    assert_eq!(
        body["items"],
        Value::from(lines[..SCREEN_ROWS].to_vec()),
        "the first page lists the five newest authorizations in key order"
    );
    let flags: BTreeSet<String> = lines[..SCREEN_ROWS]
        .iter()
        .map(|line| text_of(&line["approved"]))
        .collect();
    assert_eq!(
        flags,
        BTreeSet::from([APPROVED_FLAG.to_owned(), DECLINED_FLAG.to_owned()]),
        "the first page shows both approval flags"
    );
    assert_eq!(
        body["next_key"],
        lines[SCREEN_ROWS - 1]["auth_id"],
        "PF8 key"
    );
    assert_eq!(body["prev_key"], Value::Null, "no page precedes the first");
    body
}

/// PF8 from the first page's `next_key`, then PF7 from the second page's `prev_key`.
async fn later_pages(app: &TestApp, token: &str, first: &Value, parity: &Document) {
    proves(
        parity,
        &["P-COPAUS0C-010", "P-COPAUS0C-011", "P-COPAUS0C-016"],
    );
    let lines = listed_lines(app).await;
    let next_key = text_of(&first["next_key"]);
    let second = page_ok(app, token, &keyed_path(&next_key, "forward")).await;
    assert_eq!(
        second["items"],
        Value::from(lines[SCREEN_ROWS..].to_vec()),
        "the second page lists only the two authorizations left, no row of the first"
    );
    assert_eq!(second["next_key"], Value::Null, "no page follows the last");
    assert_eq!(second["prev_key"], lines[SCREEN_ROWS]["auth_id"], "PF7 key");
    assert_eq!(
        second.get("message"),
        None,
        "a page with rows shows no text"
    );
    assert_header(app, &second, ACCOUNT_1).await;

    let prev_key = text_of(&second["prev_key"]);
    let back = page_ok(app, token, &keyed_path(&prev_key, "backward")).await;
    assert_eq!(
        back["items"], first["items"],
        "PF7 shows the first page again"
    );
    assert_eq!(
        back["next_key"], first["next_key"],
        "PF8 key of the first page"
    );
    assert_eq!(back["prev_key"], Value::Null, "no page precedes the first");
    assert_eq!(back.get("message"), None, "a page with rows shows no text");
}

/// PF8 from the last authorization and PF7 from the first: nothing is browsed, and the
/// boundary text comes with a 200 page.
async fn boundaries(app: &TestApp, token: &str, validation: &Document, parity: &Document) {
    proves(parity, &["P-COPAUS0C-011", "P-COPAUS0C-012"]);
    let lines = listed_lines(app).await;
    let oldest = text_of(&lines[lines.len() - 1]["auth_id"]);
    let bottom = page_ok(app, token, &keyed_path(&oldest, "forward")).await;
    assert_boundary(
        &bottom,
        validation,
        "V-COPAUS0C-013",
        messages::YOU_ARE_ALREADY_AT_THE_BOTTOM_OF_THE_PAGE,
    );
    let newest = text_of(&lines[0]["auth_id"]);
    let top = page_ok(app, token, &keyed_path(&newest, "backward")).await;
    assert_boundary(
        &top,
        validation,
        "V-COPAUS0C-014",
        messages::YOU_ARE_ALREADY_AT_THE_TOP_OF_THE_PAGE,
    );
}

/// The `S` selector's replacement: each listed `auth_id` addresses the CPVD detail that
/// `XCTL COPAUS1C` would show for that row.
async fn detail_link(app: &TestApp, token: &str, first: &Value, parity: &Document) {
    proves(parity, &["P-COPAUS0C-004"]);
    let items = first["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the first page lists no items: {first}"));
    for item in items {
        let auth_id = text_of(&item["auth_id"]);
        let detail = page_ok(app, token, &format!("{ROUTE}/{auth_id}")).await;
        assert_eq!(detail["auth_id"], item["auth_id"], "{detail}");
        assert_eq!(detail["account_id"], ACCOUNT_1, "{detail}");
        assert_eq!(
            detail["pa_transaction_id"], item["pa_transaction_id"],
            "{detail}"
        );
    }
}

/// ENTER with account 2, which has no `PAUTSUM0` segment: `GU` ends `GE`, the header shows
/// zero counters and amounts, and nothing is listed, with no text.
async fn account_without_summary(app: &TestApp, token: &str, parity: &Document) {
    proves(parity, &["P-COPAUS0C-006", "P-COPAUS0C-009"]);
    let body = page_ok(app, token, &summary_path(ACCOUNT_2)).await;
    assert_eq!(
        field_names(&body),
        PAGE_FIELDS.into_iter().collect::<BTreeSet<_>>(),
        "an account without a summary shows no text: {body}"
    );
    assert_header(app, &body, ACCOUNT_2).await;
    assert_summary(&body, &NO_SUMMARY);
    assert_eq!(body["items"], json!([]), "{body}");
    assert_eq!(body["next_key"], Value::Null, "{body}");
    assert_eq!(body["prev_key"], Value::Null, "{body}");
}

#[test]
fn cpvs_key_edits_and_failed_lookups_answer_on_acctid() {
    let validation = validation_audit();
    let parity = parity_audit();
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        let token = app.user_token().await;
        key_edits(&app, &token, &validation, &parity).await;
        failed_lookups(&app, &token, &validation, &parity).await;
    });
}

/// `PROCESS-ENTER-KEY`: a blank account ID, then one that is not eleven digits.
async fn key_edits(app: &TestApp, token: &str, validation: &Document, parity: &Document) {
    proves(parity, &["P-COPAUS0C-003"]);
    let blank = [
        ROUTE.to_owned(),
        format!("{ROUTE}?account_id="),
        format!("{ROUTE}?account_id=%20%20%20"),
    ];
    for path in &blank {
        let resp = app.get(path, Some(token)).await;
        assert_row_error(
            &resp,
            validation,
            "V-COPAUS0C-001",
            ErrorKind::Empty,
            messages::PLEASE_ENTER_ACCT_ID,
        );
    }
    for entered in ["ABC", "0000000000A", "123", "000000000012"] {
        let resp = app.get(&summary_path(entered), Some(token)).await;
        assert_row_error(
            &resp,
            validation,
            "V-COPAUS0C-002",
            ErrorKind::Invalid,
            messages::ACCT_ID_MUST_BE_NUMERIC,
        );
    }
}

/// `GATHER-ACCOUNT-DETAILS` reads that find no record: the cross-reference of an unknown
/// account, then the account and the customer of two cross-references the fixture lacks.
async fn failed_lookups(app: &TestApp, token: &str, validation: &Document, parity: &Document) {
    proves(parity, &["P-COPAUS0C-007", "P-COPAUS0C-008"]);
    app.stack().exec_sql(ORPHAN_XREFS).await;
    let cases = [
        (
            "V-COPAUS0C-004",
            UNKNOWN_ACCOUNT,
            AuthLookupFailure::XrefNotFound,
            UNKNOWN_ACCOUNT,
        ),
        (
            "V-COPAUS0C-006",
            XREF_WITHOUT_ACCOUNT,
            AuthLookupFailure::AccountNotFound,
            XREF_WITHOUT_ACCOUNT,
        ),
        (
            "V-COPAUS0C-008",
            XREF_WITHOUT_CUSTOMER,
            AuthLookupFailure::CustomerNotFound,
            MISSING_CUSTOMER,
        ),
    ];
    for (id, account, failure, key) in cases {
        let message = messages::auth_lookup_message(failure, key, RESP_NOTFND.0, RESP_NOTFND.1);
        let resp = app.get(&summary_path(account), Some(token)).await;
        assert_row_error(&resp, validation, id, ErrorKind::NotFound, &message);
    }
}

#[test]
fn cpvs_failed_reads_answer_with_the_system_error_text() {
    let validation = validation_audit();
    let parity = parity_audit();
    proves(
        &parity,
        &[
            "P-COPAUS0C-007",
            "P-COPAUS0C-008",
            "P-COPAUS0C-009",
            "P-COPAUS0C-014",
        ],
    );
    let app = TestApp::start_with_clock(CLOCK);
    block_on(async {
        insert_authorizations(&app).await;
        let token = app.user_token().await;
        let healthy = page_ok(&app, &token, &summary_path(ACCOUNT_1)).await;
        let start_key = text_of(&healthy["items"][0]["auth_id"]);
        for fault in faults() {
            let path = if fault.keyed {
                keyed_path(&start_key, "backward")
            } else {
                summary_path(ACCOUNT_1)
            };
            app.stack().exec_sql(fault.hide).await;
            let resp = app.get(&path, Some(&token)).await;
            app.stack().exec_sql(fault.restore).await;
            assert_row_error(
                &resp,
                &validation,
                fault.id,
                ErrorKind::Unexpected,
                &fault.message,
            );
            let restored = page_ok(&app, &token, &summary_path(ACCOUNT_1)).await;
            assert_eq!(restored, healthy, "the page once {} is restored", fault.id);
        }

        // Last, because it closes the pool the app serves from.
        proves(&parity, &["P-COPAUS0C-020", "P-COPAUS0C-021"]);
        let closed = closed_store_answer(&app).await;
        assert_row_error(
            &closed,
            &validation,
            "V-COPAUS0C-010",
            ErrorKind::Unexpected,
            &ims(messages::SYSTEM_ERROR_WHILE_SCHEDULING_PSB),
        );
    });
}

/// The read failures of CPVS, each injected by hiding the table the read names: the three
/// `READ` failures of `GATHER-ACCOUNT-DETAILS` and the three DL/I failures.
fn faults() -> [Fault; 6] {
    let lookup = |failure: AuthLookupFailure, key: &str| {
        messages::auth_lookup_message(failure, key, RESP_IOERR.0, RESP_IOERR.1)
    };
    [
        Fault {
            id: "V-COPAUS0C-005",
            hide: HIDE_XREF,
            restore: RESTORE_XREF,
            keyed: false,
            message: lookup(AuthLookupFailure::XrefReadError, ACCOUNT_1),
        },
        Fault {
            id: "V-COPAUS0C-007",
            hide: HIDE_ACCOUNTS,
            restore: RESTORE_ACCOUNTS,
            keyed: false,
            message: lookup(AuthLookupFailure::AccountReadError, ACCOUNT_1),
        },
        Fault {
            id: "V-COPAUS0C-009",
            hide: HIDE_CUSTOMERS,
            restore: RESTORE_CUSTOMERS,
            keyed: false,
            message: lookup(AuthLookupFailure::CustomerReadError, CUSTOMER_1),
        },
        Fault {
            id: "V-COPAUS0C-011",
            hide: HIDE_SUMMARIES,
            restore: RESTORE_SUMMARIES,
            keyed: false,
            message: ims(messages::SYSTEM_ERROR_WHILE_READING_AUTH_SUMMARY_COPAUS0C),
        },
        Fault {
            id: "V-COPAUS0C-012",
            hide: HIDE_DETAILS,
            restore: RESTORE_DETAILS,
            keyed: false,
            message: ims(messages::SYSTEM_ERROR_WHILE_READING_AUTH_DETAILS_COPAUS0C),
        },
        Fault {
            id: "V-COPAUS0C-015",
            hide: HIDE_DETAILS,
            restore: RESTORE_DETAILS,
            keyed: true,
            message: ims(messages::SYSTEM_ERROR_WHILE_REPOS_AUTH_DETAILS),
        },
    ]
}

/// The answer of a CPVS request whose authorization store cannot be opened, the
/// `SCHEDULE-PSB` failure (V-COPAUS0C-010, E-71).
///
/// No HTTP request reaches it: the `AuthUser` extractor opens a unit of work of its own on
/// the same pool to read the signed-on user before the handler opens the store, so a pool
/// that cannot begin a transaction fails that read first. The handler is therefore called
/// directly, with the signed-on user the extractor would have produced, over a state whose
/// pool is closed. This closes the pool `app` serves from.
async fn closed_store_answer(app: &TestApp) -> Resp {
    let db = Db::from_pool(app.stack().pool().clone());
    let state = AppState::new(db, Settings::default())
        .unwrap_or_else(|error| panic!("the default settings were rejected: {error}"));
    state.db().close().await;
    let user = AuthUser {
        user_id: SIGNED_ON_USER.to_owned(),
        usr_type: UserType::User,
    };
    let query = AuthSummaryQuery {
        account_id: Some(ACCOUNT_1.to_owned()),
    };
    let page = AuthSummaryPageQuery::default();
    let outcome = list_authorizations(State(state), user, ApiQuery(query), ApiQuery(page)).await;
    let Err(error) = outcome else {
        panic!("a request over a closed store answered a page");
    };
    let body = serde_json::to_value(error.body())
        .unwrap_or_else(|failure| panic!("the error body does not serialize: {failure}"));
    let raw = body.to_string();
    (error.status(), body, raw)
}

/// The CPVD route the S selector's replacement addresses.
const DETAIL_ROUTE: &str = "/authorizations/{id}";

/// The keys of an `OpenAPI` path item that name operations.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The query parameters of the list operation, all `in: query`.
const LIST_PARAMETERS: [&str; 4] = ["account_id", "start_key", "direction", "page_size"];

#[test]
fn cpvs_publishes_no_row_selector_or_aid_key_parameter() {
    let validation = validation_audit();
    let parity = parity_audit();
    proves(
        &parity,
        &[
            "P-COPAUS0C-002",
            "P-COPAUS0C-004",
            "P-COPAUS0C-005",
            "P-COPAUS0C-017",
            "P-COPAUS0C-018",
        ],
    );
    // E-54: no request carries a row selector, so the text of a selector other than S stays
    // in the catalog with its 200 status, and this test cites the row by asserting that the
    // list operation declares no selector parameter.
    assert_kind(&validation, "V-COPAUS0C-003", None);
    assert_eq!(
        audited_status(&validation, "V-COPAUS0C-003"),
        StatusCode::OK
    );
    assert_eq!(
        audited_literal(&validation, "V-COPAUS0C-003"),
        messages::INVALID_SELECTION_VALID_VALUE_IS_S
    );

    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    let list = &spec["paths"][ROUTE];
    assert_eq!(
        operations(list),
        BTreeSet::from(["get"]),
        "PF3 and the other AID keys have no operation: {list}"
    );
    let parameters: BTreeSet<(String, String)> = list["get"]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("the list operation declares no parameters: {list}"))
        .iter()
        .map(|parameter| (text_of(&parameter["name"]), text_of(&parameter["in"])))
        .collect();
    let expected: BTreeSet<(String, String)> = LIST_PARAMETERS
        .iter()
        .map(|name| ((*name).to_owned(), "query".to_owned()))
        .collect();
    assert_eq!(
        parameters, expected,
        "the query replaces the received map: no selector and no AID key parameter"
    );
    let detail = &spec["paths"][DETAIL_ROUTE];
    assert!(
        operations(detail).contains("get"),
        "a listed auth_id addresses the detail the S selector showed: {detail}"
    );
}

#[test]
fn every_copaus0c_audit_row_is_exercised_here() {
    let source = include_str!("cpvs_auth_summary.rs");
    let validation = validation_audit();
    let parity = parity_audit();
    let defined: BTreeSet<String> = validation_ids(PROGRAM).into_iter().collect();
    assert_eq!(
        row_ids(&VALIDATION_ROWS),
        defined,
        "VALIDATION_ROWS must list exactly the COPAUS0C rows of the validation audit"
    );
    let defined: BTreeSet<String> = ids_with_prefix(&parity, IdKind::Parity)
        .into_iter()
        .filter(|id| id.starts_with(PARITY_PREFIX))
        .collect();
    assert_eq!(
        row_ids(&PARITY_ROWS),
        defined,
        "PARITY_ROWS must list exactly the COPAUS0C rows of the parity audit"
    );
    let audits = [
        (&validation, &VALIDATION_ROWS[..]),
        (&parity, &PARITY_ROWS[..]),
    ];
    for (doc, rows) in audits {
        for (id, test) in rows {
            assert_eq!(
                last_verdict(doc, id),
                Some("pass"),
                "{id} must have passed its review before a test relies on it"
            );
            assert!(
                source.contains(&format!("fn {test}()")),
                "{id} names {test}, which is no test of this file"
            );
            assert!(
                source.matches(&format!("\"{id}\"")).count() >= 2,
                "{id} is listed here but cited by no assertion"
            );
        }
    }
}

/// Writes account 1's summary segment and its seven details.
async fn insert_authorizations(app: &TestApp) {
    app.stack().exec_sql(INSERT_SUMMARY).await;
    app.stack().exec_sql(INSERT_DETAILS).await;
}

/// Account 1's details as listed lines, in `PAUT9CTS` order. Also checks the fixture: the
/// key order is newest first, and differs from the identity order of the inserts.
async fn listed_lines(app: &TestApp) -> Vec<Value> {
    let rows = app.stack().fetch_text_rows(DETAILS_IN_KEY_ORDER).await;
    let lines: Vec<Value> = rows.iter().map(|row| listed_line(row.as_slice())).collect();
    let order: Vec<String> = lines
        .iter()
        .map(|line| text_of(&line["pa_transaction_id"]))
        .collect();
    assert_eq!(
        order, NEWEST_FIRST,
        "ascending keys run from the newest authorization"
    );
    let ids: Vec<i64> = lines
        .iter()
        .map(|line| {
            let auth_id = text_of(&line["auth_id"]);
            auth_id
                .parse()
                .unwrap_or_else(|error| panic!("auth_id {auth_id:?} is no integer: {error}"))
        })
        .collect();
    assert!(
        ids.windows(2).any(|pair| pair[0] > pair[1]),
        "the details must not have been inserted in key order: {ids:?}"
    );
    lines
}

/// One line of the `COPAU00` list as `POPULATE-AUTH-LIST` fills it from a row of
/// [`DETAILS_IN_KEY_ORDER`]: the stored fields, and `A` or `D` from the response code. The
/// API sends the date, time and amount as stored; their `MM/DD/YY`, `HH:MM:SS` and
/// `-zzzzzzz9.99` editing is the SPA's.
fn listed_line(row: &[Option<String>]) -> Value {
    let cell = |index: usize| -> String {
        row.get(index)
            .cloned()
            .flatten()
            .unwrap_or_else(|| panic!("column {index} of a detail row is missing: {row:?}"))
    };
    let approved = if cell(5) == APPROVED_RESPONSE {
        APPROVED_FLAG
    } else {
        DECLINED_FLAG
    };
    json!({
        "auth_id": cell(0),
        "pa_transaction_id": cell(1),
        "pa_auth_orig_date": cell(2),
        "pa_auth_orig_time": cell(3),
        "pa_auth_type": cell(4),
        "approved": approved,
        "pa_match_status": cell(6),
        "pa_approved_amt": cell(7),
    })
}

/// The 200 page `GET path` answers.
async fn page_ok(app: &TestApp, token: &str, path: &str) -> Value {
    let (status, body, raw) = app.get(path, Some(token)).await;
    assert_eq!(status, StatusCode::OK, "GET {path}: {raw}");
    body
}

/// ENTER with `account` in `ACCTIDI`.
fn summary_path(account: &str) -> String {
    format!("{ROUTE}?account_id={account}")
}

/// PF8 (`forward`) or PF7 (`backward`) on account 1 from the authorization `start_key`.
fn keyed_path(start_key: &str, direction: &str) -> String {
    format!("{ROUTE}?account_id={ACCOUNT_1}&start_key={start_key}&direction={direction}")
}

/// The field names of a JSON object answer.
fn field_names(body: &Value) -> BTreeSet<&str> {
    body.as_object()
        .unwrap_or_else(|| panic!("the answer is no JSON object: {body}"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Asserts the header of `body` holds `account`'s cross-reference, customer and account
/// records as `GATHER-ACCOUNT-DETAILS` moves them.
async fn assert_header(app: &TestApp, body: &Value, account: &str) {
    let records = app.stack().fetch_text_rows(HEADER_RECORDS).await;
    let record = records
        .iter()
        .find(|record| record.first().and_then(Option::as_deref) == Some(account))
        .unwrap_or_else(|| panic!("account {account} has no cross-reference record"));
    assert_eq!(record.len(), HEADER_FIELDS.len(), "{record:?}");
    for (field, cell) in HEADER_FIELDS.iter().zip(record) {
        let stored = cell
            .as_deref()
            .unwrap_or_else(|| panic!("{field} of account {account} is null"));
        assert_eq!(
            wire_text(&body[*field]),
            stored,
            "{field} of account {account}: {body}"
        );
    }
}

/// Asserts the summary counters of `body` are `expected`, in [`SUMMARY_FIELDS`] order.
fn assert_summary(body: &Value, expected: &[&str]) {
    assert_eq!(expected.len(), SUMMARY_FIELDS.len());
    for (field, value) in SUMMARY_FIELDS.iter().zip(expected) {
        assert_eq!(wire_text(&body[*field]), *value, "{field}: {body}");
    }
}

/// Asserts `body` is the page of a browse that read nothing on account 1: no rows and no
/// keys, with the boundary text `message` of the audit row `id`, whose status is 200.
fn assert_boundary(body: &Value, validation: &Document, id: &str, message: &str) {
    assert_kind(validation, id, None);
    assert_eq!(audited_status(validation, id), StatusCode::OK, "{id}");
    assert_eq!(audited_literal(validation, id), message, "{id}");
    assert_eq!(body["message"], message, "{id}: {body}");
    assert_eq!(body["items"], json!([]), "{id}: {body}");
    assert_eq!(body["next_key"], Value::Null, "{id}: {body}");
    assert_eq!(body["prev_key"], Value::Null, "{id}: {body}");
    assert_eq!(body["account_id"], ACCOUNT_1, "{id}: {body}");
}

/// Asserts `resp` is the error answer of the validation row `id`: its status and BMS field,
/// the error code of `kind`, which the row names, and `message`, which must be the row's
/// quoted text or, for a `STRING`-built text, start with the literal before its first slot.
fn assert_row_error(resp: &Resp, doc: &Document, id: &str, kind: ErrorKind, message: &str) {
    assert_kind(doc, id, Some(kind));
    let literal = audited_literal(doc, id);
    match literal.split_once('{') {
        Some((head, _)) => assert!(
            message.starts_with(head),
            "{id}: {message:?} does not start with the audit's text {head:?}"
        ),
        None => assert_eq!(
            message, literal,
            "{id}: the catalog text differs from the audit"
        ),
    }
    let field = audit_cell(doc, id, "BMS field");
    assert_error(
        resp,
        audited_status(doc, id),
        kind.code(),
        message,
        &[(field, message)],
    );
}

/// Asserts the trigger condition of the validation row `id` names `kind`, `none` for a row
/// that sets no error.
fn assert_kind(doc: &Document, id: &str, kind: Option<ErrorKind>) {
    let expected = kind.map_or_else(|| "kind=none".to_owned(), |kind| format!("kind={kind:?}"));
    let trigger = audit_cell(doc, id, "Trigger condition");
    assert!(
        trigger
            .split(';')
            .map(str::trim)
            .any(|part| part == expected),
        "{id} does not name {expected}: {trigger}"
    );
}

/// The HTTP status the validation row `id` maps.
fn audited_status(doc: &Document, id: &str) -> StatusCode {
    let cell = audit_cell(doc, id, "Status");
    cell.parse::<u16>()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} has no HTTP status: {cell}"))
}

/// The quoted text of the validation row `id`: its first code span without the quotes.
fn audited_literal(doc: &Document, id: &str) -> String {
    let cell = audit_cell(doc, id, "Message text");
    code_span(cell)
        .and_then(|span| {
            span.text
                .strip_prefix('"')
                .and_then(|text| text.strip_suffix('"'))
                .map(str::to_owned)
        })
        .unwrap_or_else(|| panic!("{id} quotes no message text: {cell}"))
}

/// The cell `column` of the data row `id`, trimmed.
fn audit_cell<'a>(doc: &'a Document, id: &str, column: &str) -> &'a str {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the audit has no {id} row"));
    table
        .cell(row, column)
        .unwrap_or_else(|| panic!("{id} has no `{column}` cell"))
        .trim()
}

/// Asserts each of `ids` is a `COPAUS0C` row of the parity audit that passed its review,
/// so a test relies on reviewed rows only.
fn proves(parity: &Document, ids: &[&str]) {
    for id in ids {
        assert_eq!(audit_cell(parity, id, "Program"), PROGRAM, "{id}");
        assert_eq!(
            last_verdict(parity, id),
            Some("pass"),
            "{id} has not passed its review"
        );
    }
}

/// The verdict of the last review-log row on `id`, if any.
fn last_verdict<'a>(doc: &'a Document, id: &str) -> Option<&'a str> {
    let mut verdict = None;
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            if table.cell(row, "Row ID").map(str::trim) == Some(id) {
                verdict = table.cell(row, "Verdict").map(str::trim);
            }
        }
    }
    verdict
}

/// A failed authorization-store access: the IMS system-error `head` with the status E-44
/// sets.
fn ims(head: &str) -> String {
    messages::ims_status_message(head, IMS_STATUS_OTHER)
}

/// The JSON string `value`.
fn text_of(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON string, got {value}"))
        .to_owned()
}

/// A JSON scalar as the stored text it stands for: a string as is, a number in decimal.
fn wire_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => panic!("expected a JSON string or number, got {other}"),
    }
}

/// The operations an `OpenAPI` path item declares.
fn operations(item: &Value) -> BTreeSet<&str> {
    item.as_object()
        .unwrap_or_else(|| panic!("the path is not published: {item}"))
        .keys()
        .map(String::as_str)
        .filter(|key| HTTP_METHODS.contains(key))
        .collect()
}

/// The IDs of a table of audit rows and their tests.
fn row_ids(rows: &[(&str, &str)]) -> BTreeSet<String> {
    rows.iter().map(|(id, _)| (*id).to_owned()).collect()
}
