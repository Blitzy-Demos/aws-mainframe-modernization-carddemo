//! A failed account master read keeps its own file in the answer:
//! `GET /accounts/{id}` (CAVW) and `PUT /accounts/{id}` (CAUP).
//!
//! `COACTVWC` and `COACTUPC` `9000-READ-ACCT` read the cross-reference, then
//! `ACCTDAT`, then `CUSTDAT` (P-COACTVWC-006). An `ACCTDAT` read failure moves
//! the `ACCTDAT` file-error text (V-COACTVWC-006, V-COACTUPC-007), and only a
//! failure of the customer read itself replaces it with the `CUSTDAT` text
//! (V-COACTVWC-008, E-45).
//!
//! `PostgreSQL` aborts the whole transaction on any server-side error, so a
//! failed account read on the request's transaction would make the healthy
//! customer read after it fail too and answer with the `CUSTDAT` text. Each
//! store statement therefore runs under its own savepoint. Here the account
//! table alone is hidden (SQLSTATE `42P01` on the account read) while the
//! customer table stays readable, and both routes must answer with the
//! `ACCTDAT` text the audit rows give.
//!
//! The expected texts are built by `carddemo_domain::messages` from the audit
//! rows' `ERROR-FILE` literal, with the `READ` operation name of the COBOL
//! `WHEN OTHER` branch and the RESP and RESP2 values E-44 sets for a database
//! failure.

pub mod common;

use carddemo_audit::Document;
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::block_on;
use common::{TestApp, assert_error, validation_audit};
use reqwest::StatusCode;
use serde_json::{Map, Value};

/// Account 1 of the fixture, as `ACCTSID` holds it.
const ACCOUNT_PATH: &str = "/accounts/00000000001";

/// Hides the account master: the account read then fails on the server with
/// SQLSTATE `42P01`, while the cross-reference and customer tables stay readable.
const HIDE_ACCOUNTS: &str = "ALTER TABLE accounts RENAME TO accounts_offline";

/// Puts the account master back.
const RESTORE_ACCOUNTS: &str = "ALTER TABLE accounts_offline RENAME TO accounts";

/// `MOVE 'READ' TO ERROR-OPNAME` of the `WHEN OTHER` branch of
/// `9300-GETACCTDATA-BYACCT` [app/cbl/COACTVWC.cbl:L812]
/// [app/cbl/COACTUPC.cbl:L3739].
const ERROR_OPNAME_READ: &str = "READ";

/// The `RESP` E-44 sets for a database failure: `IOERR`.
const RESP_IOERR: u32 = 17;

/// The `RESP2` E-44 sets alongside [`RESP_IOERR`].
const RESP2_IOERR: u32 = 0;

/// The account group the update body changes, `AADDGRP X(10)`, a field no
/// edit checks.
const CHANGED_GROUP: &str = "ISOLATED";

/// The update body fields that repeat a fetched field of the same name.
const COPIED_FIELDS: [&str; 21] = [
    "acct_active_status",
    "acct_credit_limit",
    "acct_cash_credit_limit",
    "acct_curr_bal",
    "acct_curr_cyc_credit",
    "acct_curr_cyc_debit",
    "customer_id",
    "cust_fico_credit_score",
    "cust_first_name",
    "cust_middle_name",
    "cust_last_name",
    "cust_addr_line_1",
    "cust_addr_state_cd",
    "cust_addr_line_2",
    "cust_addr_zip",
    "cust_addr_line_3",
    "cust_addr_country_cd",
    "cust_govt_issued_id",
    "cust_eft_account_id",
    "cust_pri_card_holder_ind",
    "version",
];

/// The update body fields that are a part of a fetched field, as the
/// `COACTUP` map splits it: (body field, fetched field, first byte, end byte).
/// Dates are `YYYY-MM-DD`, the SSN is 9 digits and phones are `(aaa)bbb-cccc`.
const SPLIT_FIELDS: [(&str, &str, usize, usize); 21] = [
    ("acct_open_year", "acct_open_date", 0, 4),
    ("acct_open_mon", "acct_open_date", 5, 7),
    ("acct_open_day", "acct_open_date", 8, 10),
    ("acct_exp_year", "acct_expiraion_date", 0, 4),
    ("acct_exp_mon", "acct_expiraion_date", 5, 7),
    ("acct_exp_day", "acct_expiraion_date", 8, 10),
    ("acct_reissue_year", "acct_reissue_date", 0, 4),
    ("acct_reissue_mon", "acct_reissue_date", 5, 7),
    ("acct_reissue_day", "acct_reissue_date", 8, 10),
    ("cust_ssn_1", "cust_ssn", 0, 3),
    ("cust_ssn_2", "cust_ssn", 3, 5),
    ("cust_ssn_3", "cust_ssn", 5, 9),
    ("cust_dob_year", "cust_dob_yyyy_mm_dd", 0, 4),
    ("cust_dob_mon", "cust_dob_yyyy_mm_dd", 5, 7),
    ("cust_dob_day", "cust_dob_yyyy_mm_dd", 8, 10),
    ("cust_phone_num_1a", "cust_phone_num_1", 1, 4),
    ("cust_phone_num_1b", "cust_phone_num_1", 5, 8),
    ("cust_phone_num_1c", "cust_phone_num_1", 9, 13),
    ("cust_phone_num_2a", "cust_phone_num_2", 1, 4),
    ("cust_phone_num_2b", "cust_phone_num_2", 5, 8),
    ("cust_phone_num_2c", "cust_phone_num_2", 9, 13),
];

/// The answer one validation audit row gives.
struct Expected {
    status: StatusCode,
    message: String,
    field: String,
}

/// The status, the file-error text and the BMS field of the audit row `id`.
fn expected(doc: &Document, id: &str) -> Expected {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the validation audit has no {id} row"));
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
            .trim()
    };
    let status = cell("Status")
        .parse::<u16>()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} has no HTTP status: {}", cell("Status")));
    Expected {
        status,
        message: messages::file_error_message(
            ERROR_OPNAME_READ,
            error_file(cell("Message text")),
            RESP_IOERR,
            RESP2_IOERR,
        ),
        field: cell("BMS field").to_owned(),
    }
}

/// The `ERROR-FILE` literal a message cell names: the quoted text after its
/// last `ERROR-FILE`, for example `ACCTDAT ` in `(ERROR-FILE 'ACCTDAT ', E-44)`.
fn error_file(cell: &str) -> &str {
    let (file, _) = cell
        .rsplit_once("ERROR-FILE")
        .and_then(|(_, after)| after.split_once('\''))
        .and_then(|(_, quoted)| quoted.split_once('\''))
        .unwrap_or_else(|| panic!("no quoted ERROR-FILE in the message cell: {cell}"));
    file
}

/// The string `name` of the fetched record.
fn text<'a>(fetched: &'a Value, name: &str) -> &'a str {
    fetched[name]
        .as_str()
        .unwrap_or_else(|| panic!("GET answered no string `{name}`: {fetched}"))
}

/// The bytes `from..to` of the fetched string `name`.
fn part<'a>(fetched: &'a Value, name: &str, from: usize, to: usize) -> &'a str {
    let value = text(fetched, name);
    value
        .get(from..to)
        .unwrap_or_else(|| panic!("`{name}` {value:?} has no bytes {from}..{to}"))
}

/// The `PUT` body of the fetched record as the `COACTUP` fields show it, its
/// version token included, with only the account group changed.
fn update_body(fetched: &Value) -> Value {
    let copied = COPIED_FIELDS
        .iter()
        .map(|name| (*name, text(fetched, name)));
    let split = SPLIT_FIELDS
        .iter()
        .map(|(name, source, from, to)| (*name, part(fetched, source, *from, *to)));
    let body: Map<String, Value> = copied
        .chain(split)
        .chain([("acct_group_id", CHANGED_GROUP)])
        .map(|(name, value)| (name.to_owned(), Value::String(value.to_owned())))
        .collect();
    Value::Object(body)
}

#[test]
fn a_failed_account_read_answers_with_the_account_file_error() {
    // P-COACTVWC-006: CXACAIX, then ACCTDAT, then CUSTDAT.
    let doc = validation_audit();
    let view_error = expected(&doc, "V-COACTVWC-006");
    let update_error = expected(&doc, "V-COACTUPC-007");
    let customer_error = expected(&doc, "V-COACTVWC-008");
    assert_ne!(
        view_error.message, customer_error.message,
        "V-COACTVWC-006 and V-COACTVWC-008 must name different files"
    );
    let app = TestApp::start();
    block_on(async {
        let token = app.user_token().await;
        let (status, fetched, raw) = app.get(ACCOUNT_PATH, Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "the fetch before the fault: {raw}");
        let body = update_body(&fetched);
        app.stack().exec_sql(HIDE_ACCOUNTS).await;

        // V-COACTVWC-006: the ACCTDAT text, not the V-COACTVWC-008 CUSTDAT text.
        let viewed = app.get(ACCOUNT_PATH, Some(&token)).await;
        assert_error(
            &viewed,
            view_error.status,
            ErrorKind::Unexpected.code(),
            &view_error.message,
            &[(view_error.field.as_str(), view_error.message.as_str())],
        );
        // V-COACTUPC-007: the fetch of the update reads as the view does.
        let updated = app.put(ACCOUNT_PATH, Some(&token), &body).await;
        assert_error(
            &updated,
            update_error.status,
            ErrorKind::Unexpected.code(),
            &update_error.message,
            &[(update_error.field.as_str(), update_error.message.as_str())],
        );

        app.stack().exec_sql(RESTORE_ACCOUNTS).await;
        let (status, restored, raw) = app.get(ACCOUNT_PATH, Some(&token)).await;
        assert_eq!(status, StatusCode::OK, "the fetch after the fault: {raw}");
        assert_eq!(
            restored, fetched,
            "the failed update must have written nothing"
        );
    });
}
