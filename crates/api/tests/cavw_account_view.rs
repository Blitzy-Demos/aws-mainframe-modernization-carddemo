//! CAVW · `COACTVWC` · map `COACTVW`: `GET /accounts/{id}`, the account view.
//!
//! Three tests, each on its own seeded database:
//!
//! - Every `COACTVWC` row of `docs/parity/validation-audit.md`. A local table
//!   of failing requests, each keyed by the V-ID whose row gives its answer,
//!   must exercise every `COACTVWC` row the audit defines, and each request
//!   must answer with that row's status, the `code` of its kind, its BMS field
//!   and the whole text the row records for the injected key and `RESP`
//!   codes, as its message and as the message of its field. States the
//!   fixture corpus does not hold are set up with SQL on the test database: a
//!   table renamed away fails its read (`SQLSTATE` `42P01`), a deleted row is
//!   not found.
//! - Account `00000000001`: the values section D of
//!   `docs/parity/parity-oracle-audit.md` records, every shown field compared
//!   with the fixture bytes at the offsets section A records, the version
//!   token, and the insta snapshot of the body without the token.
//! - The rows of the 3270 dialogue around the reads (AID keys, first entry,
//!   `COMMON-RETURN`, the screen header, information line, attributes and
//!   send, `SEND-PLAIN-TEXT`, `SEND-LONG-TEXT`, `ABEND-ROUTINE` and the
//!   unreachable `IF INPUT-ERROR`): each request is one stateless JSON answer
//!   that carries none of the screen's texts, names or attributes, and a
//!   failure outside the reads, provoked by closing the pool the server reads
//!   through, is the HTTP mapping's 500 unexpected-error response.
//!
//! A fourth test reads only `docs/parity/function-map.md` and the published
//! `OpenAPI` document: the keys of map `COACTVW` become the one
//! `GET /accounts/{id}`, whose only input is the account number, or a
//! navigation link with no request.
//!
//! Statuses, kinds, BMS fields and record values come from the audits through
//! `carddemo_audit`. A literal message text is the constant of
//! `carddemo_domain::messages`, checked to equal the audit's message cell. A
//! `STRING`-built text is expanded by `common::audit_text` from the reviewed
//! template of its row, with the injected key and the `RESP` and `RESP2` codes
//! E-44 sets, 13 and 80 for `NOTFND`, 17 and 0 for a database failure, each
//! moved into its COBOL picture; it is never composed by
//! `carddemo_domain::messages`. Docker must be running.

pub mod common;

use std::collections::BTreeSet;
use std::fs;

use axum::extract::State;
use carddemo_api::AppState;
use carddemo_api::auth::{AuthUser, UserType};
use carddemo_api::error::{ApiError, ApiPath};
use carddemo_api::handlers::accounts::get_account;
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, KeyValue, Repo, Row, Table, cell_code_spans, code_span,
    id_tokens, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_domain::rules::account_view::edit_account_id;
use carddemo_persistence::Db;
use carddemo_runtime::config::Settings;
use carddemo_testkit::{block_on, fixtures};
use common::audit_text::{Operand, Template};
use common::{Resp, TestApp, assert_error, parity_audit, validation_audit, validation_ids};
use reqwest::StatusCode;
use reqwest::header::{CONTENT_TYPE, SET_COOKIE};
use serde_json::Value;

/// The program whose validation rows the first test exercises.
const PROGRAM: &str = "COACTVWC";

/// `WS-RETURN-MSG PIC X(75)`, the receiver every `COACTVWC` text is cut to
/// [app/cbl/COACTVWC.cbl:L117].
const RETURN_MSG_LEN: usize = 75;

/// `MOVE 'READ' TO ERROR-OPNAME` of the `WHEN OTHER` branch of each read
/// [app/cbl/COACTVWC.cbl:L762].
const ERROR_OPNAME_READ: &str = "READ";

/// `ERROR-OPNAME PIC X(8)` of `WS-FILE-ERROR-MESSAGE`
/// [app/cbl/COACTVWC.cbl:L89].
const ERROR_OPNAME_LEN: usize = 8;

/// `ERROR-FILE PIC X(9)` of `WS-FILE-ERROR-MESSAGE`
/// [app/cbl/COACTVWC.cbl:L93].
const ERROR_FILE_LEN: usize = 9;

/// The digits of `WS-RESP-CD` and `WS-REAS-CD`, both `PIC S9(09) COMP`
/// [app/cbl/COACTVWC.cbl:L40-42], which `MOVE` sends to `ERROR-RESP` and
/// `ERROR-RESP2` as their 9-digit display.
const RESP_DIGITS: usize = 9;

/// `ERROR-RESP PIC X(10)` and `ERROR-RESP2 PIC X(10)`
/// [app/cbl/COACTVWC.cbl:L98-103].
const ERROR_RESP_LEN: usize = 10;

/// `WS-CARD-RID-ACCT-ID-X PIC X(11)`, the account number of the
/// cross-reference and account not-found texts [app/cbl/COACTVWC.cbl:L79-80].
const ACCT_ID_X_LEN: usize = 11;

/// `WS-CARD-RID-CUST-ID-X PIC X(09)`, the customer number of the customer
/// not-found text [app/cbl/COACTVWC.cbl:L76-77].
const CUST_ID_X_LEN: usize = 9;

/// The CICS `RESP` of `NOTFND` on `READ` (E-44).
const RESP_NOTFND: u64 = 13;

/// The `RESP2` of `NOTFND` on `READ` (E-44).
const RESP2_NOTFND: u64 = 80;

/// The `RESP` E-44 sets for a database failure: `IOERR`.
const RESP_IOERR: u64 = 17;

/// The `RESP2` E-44 sets alongside [`RESP_IOERR`].
const RESP2_IOERR: u64 = 0;

/// The `Expected result` column of the parity audit.
const EXPECTED_RESULT: &str = "Expected result";

/// Section D: account `00000000001`, with its cross-reference, account and
/// customer records.
const EXISTING_ACCOUNT_ROW: &str = "P-CVACT01Y-015";

/// Section D: an account number no cross-reference holds.
const MISSING_ACCOUNT_ROW: &str = "P-CVACT01Y-016";

/// Section D: the first cross-reference, of account `00000000050`, whose
/// account and customer records exist.
const EXISTING_XREF_ROW: &str = "P-CVACT03Y-006";

/// Section D: customer `000000001`, the customer of account `00000000001`.
const EXISTING_CUSTOMER_ROW: &str = "P-CVCUS01Y-021";

/// The ASCII fixture of `ACCTDAT` (CVACT01Y).
const ACCOUNT_FILE: &str = "acctdata.txt";

/// The ASCII fixture of `CCXREF`, read through `CXACAIX` (CVACT03Y).
const XREF_FILE: &str = "cardxref.txt";

/// The ASCII fixture of `CUSTDAT` (CVCUS01Y).
const CUSTOMER_FILE: &str = "custdata.txt";

/// The length of the version token, 16 lower-case hex digits, as the
/// `AccountResponse` schema publishes it (`^[0-9a-f]{16}$`).
const VERSION_LEN: usize = 16;

/// The account numbers 2210-EDIT-ACCOUNT reads as not entered: one space, a
/// segment of spaces and `*` [app/cbl/COACTVWC.cbl:L628-630].
const BLANK_KEYS: [&str; 3] = ["%20", "%20%20%20", "*"];

/// Account numbers 2210-EDIT-ACCOUNT rejects as not 11 digits or zero
/// [app/cbl/COACTVWC.cbl:L666-667]: letters, zeros, a trailing letter, one
/// digit padded with spaces by the `MOVE`, and 12 digits, more than the 11 of
/// `ACCTSID`.
const INVALID_KEYS: [&str; 5] = [
    "ABCDEFGHIJK",
    "00000000000",
    "0000000000A",
    "1",
    "000000000001",
];

/// Hides the cross-reference: the `CXACAIX` read then fails on the server.
const HIDE_XREF: &str = "ALTER TABLE card_xref RENAME TO card_xref_offline";

/// Puts the cross-reference back.
const RESTORE_XREF: &str = "ALTER TABLE card_xref_offline RENAME TO card_xref";

/// Hides the account master: the `ACCTDAT` read then fails on the server.
const HIDE_ACCOUNTS: &str = "ALTER TABLE accounts RENAME TO accounts_offline";

/// Puts the account master back.
const RESTORE_ACCOUNTS: &str = "ALTER TABLE accounts_offline RENAME TO accounts";

/// Hides the customer master: the `CUSTDAT` read then fails on the server.
const HIDE_CUSTOMERS: &str = "ALTER TABLE customers RENAME TO customers_offline";

/// Puts the customer master back.
const RESTORE_CUSTOMERS: &str = "ALTER TABLE customers_offline RENAME TO customers";

/// Deletes customer `000000001` ([`EXISTING_CUSTOMER_ROW`]), leaving the
/// cross-reference and the account of account `00000000001`.
const DELETE_CUSTOMER: &str = "DELETE FROM customers WHERE customer_id = '000000001'";

/// Deletes account `00000000001` ([`EXISTING_ACCOUNT_ROW`]), leaving its
/// cross-reference.
const DELETE_ACCOUNT: &str = "DELETE FROM accounts WHERE account_id = '00000000001'";

/// Deletes account `00000000050` ([`EXISTING_XREF_ROW`]), leaving its
/// cross-reference and its customer.
const DELETE_XREF_ACCOUNT: &str = "DELETE FROM accounts WHERE account_id = '00000000050'";

/// The fixture keys the failing requests address, as section D of the parity
/// audit records them.
struct Keys {
    /// `ACCT-ID` of an account with all three records.
    account: String,
    /// An `ACCT-ID` without a cross-reference.
    missing_account: String,
    /// `CUST-ID` of the customer of [`Keys::account`].
    customer: String,
    /// `XREF-ACCT-ID` of another account with all three records.
    xref_account: String,
}

impl Keys {
    /// The keys of the section D rows, after checking that each delete
    /// statement names the record its row keys.
    fn from_audit(parity: &Document) -> Self {
        let keys = Self {
            account: expected_text(parity, EXISTING_ACCOUNT_ROW, "value"),
            missing_account: expected_text(parity, MISSING_ACCOUNT_ROW, "value"),
            customer: expected_text(parity, EXISTING_CUSTOMER_ROW, "value"),
            xref_account: expected_text(parity, EXISTING_XREF_ROW, "account_id"),
        };
        for (sql, key) in [
            (DELETE_ACCOUNT, &keys.account),
            (DELETE_CUSTOMER, &keys.customer),
            (DELETE_XREF_ACCOUNT, &keys.xref_account),
        ] {
            assert!(
                sql.ends_with(&format!("'{key}'")),
                "`{sql}` must delete the record the parity audit keys {key}"
            );
        }
        keys
    }
}

/// The message a failing request must answer with: a literal of the
/// `carddemo_domain::messages` catalog, checked to equal the audit's text, or
/// a `STRING`-built text, expanded from the row's reviewed audit template with
/// the injected key and the E-44 `RESP` codes.
enum Text {
    /// A literal message constant of the catalog, equal to the row's text.
    Literal(&'static str),
    /// The 9200-GETCARDXREF-BYACCT `NOTFND` text of an account number: the
    /// template with `WS-CARD-RID-ACCT-ID-X` and the `NOTFND` codes.
    XrefNotFound(String),
    /// The 9300-GETACCTDATA-BYACCT `NOTFND` text of an account number: the
    /// template with `WS-CARD-RID-ACCT-ID-X` and the `NOTFND` codes.
    AccountNotFound(String),
    /// The 9400-GETCUSTDATA-BYCUST `NOTFND` text of a customer number: the
    /// template with `WS-CARD-RID-CUST-ID-X` and the `NOTFND` codes.
    CustomerNotFound(String),
    /// The `WS-FILE-ERROR-MESSAGE` of a failed `READ` of the file the audit
    /// row's `ERROR-FILE` names: the template with `ERROR-OPNAME`,
    /// `ERROR-FILE` and the `IOERR` codes.
    FileError,
}

/// One failing request of the case table.
struct Case {
    /// The V-ID whose row gives the answer.
    id: &'static str,
    /// The parity rows and the precedence the case proves, shown on failure.
    proves: &'static str,
    /// Statements run on the test database before the request.
    before: &'static [&'static str],
    /// Statements run on the test database after the request.
    after: &'static [&'static str],
    /// The request path.
    path: String,
    /// The message of the answer.
    text: Text,
}

/// The answer one case must receive.
struct Expected {
    status: StatusCode,
    code: &'static str,
    field: String,
    message: String,
}

/// The router path of the account number `key` as entered.
fn account_path(key: &str) -> String {
    format!("/accounts/{key}")
}

/// A key-edit case: the request is answered by 2210-EDIT-ACCOUNT while the
/// cross-reference is hidden, so any read would answer 500 instead.
fn key_case(id: &'static str, proves: &'static str, key: &str, text: Text) -> Case {
    Case {
        id,
        proves,
        before: &[HIDE_XREF],
        after: &[RESTORE_XREF],
        path: account_path(key),
        text,
    }
}

/// The failing requests, in the order they run on one database: the key
/// edits, the missing cross-reference and the three read failures first, the
/// deletions last, since they persist.
fn cases(keys: &Keys) -> Vec<Case> {
    let account = account_path(&keys.account);
    let xref_account = account_path(&keys.xref_account);
    let blank = BLANK_KEYS.iter().map(|key| {
        key_case(
            "V-COACTVWC-001",
            "P-COACTVWC-020, P-COACTVWC-021: a blank segment is no input and the cross edit \
             replaces the P-COACTVWC-004 prompt; P-COACTVWC-019: no read",
            key,
            Text::Literal(messages::NO_INPUT_RECEIVED),
        )
    });
    let invalid = INVALID_KEYS.iter().map(|key| {
        key_case(
            "V-COACTVWC-002",
            "P-COACTVWC-005: not 11 digits or zero; P-COACTVWC-019: no read",
            key,
            Text::Literal(messages::ACCOUNT_FILTER_MUST_BE_NON_ZERO_11_DIGIT),
        )
    });
    let reads = [
        Case {
            id: "V-COACTVWC-003",
            proves: "P-COACTVWC-007: the cross-reference is not on file",
            before: &[],
            after: &[],
            path: account_path(&keys.missing_account),
            text: Text::XrefNotFound(keys.missing_account.clone()),
        },
        Case {
            id: "V-COACTVWC-004",
            proves: "P-COACTVWC-008: the cross-reference read fails and ends the reads",
            before: &[HIDE_XREF],
            after: &[RESTORE_XREF],
            path: account.clone(),
            text: Text::FileError,
        },
        Case {
            id: "V-COACTVWC-006",
            proves: "P-COACTVWC-006: the account read fails, the customer read finds its record",
            before: &[HIDE_ACCOUNTS],
            after: &[RESTORE_ACCOUNTS],
            path: account.clone(),
            text: Text::FileError,
        },
        Case {
            id: "V-COACTVWC-008",
            proves: "P-COACTVWC-006: the customer read fails after the account was found",
            before: &[HIDE_CUSTOMERS],
            after: &[RESTORE_CUSTOMERS],
            path: account.clone(),
            text: Text::FileError,
        },
        Case {
            id: "V-COACTVWC-007",
            proves: "P-COACTVWC-010: the customer of the cross-reference is not on file",
            before: &[DELETE_CUSTOMER],
            after: &[],
            path: account.clone(),
            text: Text::CustomerNotFound(keys.customer.clone()),
        },
        Case {
            id: "V-COACTVWC-005",
            proves: "P-COACTVWC-009: the cross-reference's account is not on file",
            before: &[DELETE_XREF_ACCOUNT],
            after: &[],
            path: xref_account.clone(),
            text: Text::AccountNotFound(keys.xref_account.clone()),
        },
        Case {
            id: "V-COACTVWC-008",
            proves: "P-COACTVWC-030: after the V-COACTVWC-005 text a customer read failure \
                     replaces it (E-45)",
            before: &[HIDE_CUSTOMERS],
            after: &[RESTORE_CUSTOMERS],
            path: xref_account,
            text: Text::FileError,
        },
        Case {
            id: "V-COACTVWC-005",
            proves: "P-COACTVWC-030: after the V-COACTVWC-005 text a customer not found \
                     keeps it (E-45)",
            before: &[DELETE_ACCOUNT],
            after: &[],
            path: account,
            text: Text::AccountNotFound(keys.account.clone()),
        },
    ];
    blank.chain(invalid).chain(reads).collect()
}

/// The `key=value` pairs of the `Expected result` cell of the parity row `id`.
fn expected_values(parity: &Document, id: &str) -> Vec<KeyValue> {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    let cell = table
        .cell(row, EXPECTED_RESULT)
        .unwrap_or_else(|| panic!("{id} has no `{EXPECTED_RESULT}` cell"));
    key_values(cell)
}

/// The text the first `key=` pair of the parity row `id` records.
fn expected_text(parity: &Document, id: &str, key: &str) -> String {
    let pair = expected_values(parity, id)
        .into_iter()
        .find(|pair| pair.key == key)
        .unwrap_or_else(|| panic!("{id} records no {key}="));
    unquote(&pair.value)
}

/// The text an audit value stands for: the string between the double quotes
/// of its code span, as in `` `"Immanuel"` width=25``, else the value as
/// written, as in `194.00`.
fn unquote(value: &str) -> String {
    let Some(span) = code_span(value) else {
        return value.to_owned();
    };
    span.text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .map_or_else(|| span.text.clone(), str::to_owned)
}

/// The `code` the error body publishes for the audit kind `kind` of row `id`.
fn error_code(id: &str, kind: &str) -> &'static str {
    let kind = match kind {
        "Invalid" => ErrorKind::Invalid,
        "Empty" => ErrorKind::Empty,
        "NotFound" => ErrorKind::NotFound,
        "Duplicate" => ErrorKind::Duplicate,
        "Conflict" => ErrorKind::Conflict,
        "SignonFailed" => ErrorKind::SignonFailed,
        "Unexpected" => ErrorKind::Unexpected,
        other => panic!("{id} names no error kind: kind={other}"),
    };
    kind.code()
}

/// The `ERROR-FILE` literal a message cell names: the quoted text after its
/// last `ERROR-FILE`, for example `CXACAIX ` in `ERROR-FILE 'CXACAIX ', E-44`.
fn error_file<'a>(id: &str, cell: &'a str) -> &'a str {
    let (file, _) = cell
        .rsplit_once("ERROR-FILE")
        .and_then(|(_, after)| after.split_once('\''))
        .and_then(|(_, quoted)| quoted.split_once('\''))
        .unwrap_or_else(|| panic!("{id} names no quoted ERROR-FILE: {cell}"));
    file
}

/// `ERROR-RESP PIC X(10)` or `ERROR-RESP2 PIC X(10)`, as `name` says
/// [app/cbl/COACTVWC.cbl:L98-103], after `MOVE WS-RESP-CD` or
/// `MOVE WS-REAS-CD`, `PIC S9(09) COMP` [app/cbl/COACTVWC.cbl:L40-42],
/// holding `code`: its 9-digit display and a space.
fn resp(name: &str, code: u64) -> Operand {
    Operand::unsigned_to_alphanumeric(name, code, RESP_DIGITS, ERROR_RESP_LEN)
}

/// The `NOTFND` bindings of the not-found templates of V-COACTVWC-003,
/// V-COACTVWC-005 and V-COACTVWC-007: the injected `key`, then `ERROR-RESP`
/// and `ERROR-RESP2` of the `NOTFND` codes E-44 sets.
fn not_found(key: Operand) -> Vec<Operand> {
    vec![
        key,
        resp("ERROR-RESP", RESP_NOTFND),
        resp("ERROR-RESP2", RESP2_NOTFND),
    ]
}

/// The bindings the template of row `id`, whose message cell is `cell`, is
/// expanded with for `text`: each injected key or status moved into the
/// COBOL picture of the field the template names, and none for a literal.
fn operands(id: &str, cell: &str, text: &Text) -> Vec<Operand> {
    match text {
        Text::Literal(_) => Vec::new(),
        Text::XrefNotFound(account) | Text::AccountNotFound(account) => not_found(
            Operand::alphanumeric("WS-CARD-RID-ACCT-ID-X", account, ACCT_ID_X_LEN),
        ),
        Text::CustomerNotFound(customer) => not_found(Operand::alphanumeric(
            "WS-CARD-RID-CUST-ID-X",
            customer,
            CUST_ID_X_LEN,
        )),
        Text::FileError => vec![
            Operand::alphanumeric("ERROR-OPNAME", ERROR_OPNAME_READ, ERROR_OPNAME_LEN),
            Operand::alphanumeric("ERROR-FILE", error_file(id, cell), ERROR_FILE_LEN),
            resp("ERROR-RESP", RESP_IOERR),
            resp("ERROR-RESP2", RESP2_IOERR),
        ],
    }
}

/// The answer the validation row of `case` gives: its status, the code of its
/// kind, its BMS field, and its whole message: the row's reviewed template
/// expanded with the bindings of the case into the 75 bytes of
/// `WS-RETURN-MSG`, which a literal of the catalog must equal.
fn expected(validation: &Document, case: &Case) -> Expected {
    let id = case.id;
    let (table, row) = validation
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
    let kind = key_values(cell("Trigger condition"))
        .into_iter()
        .rfind(|pair| pair.key == "kind")
        .unwrap_or_else(|| panic!("{id} records no kind="))
        .value;
    let bindings = operands(id, cell("Message text"), &case.text);
    let expanded = Template::load(validation, id).expand(&bindings, RETURN_MSG_LEN);
    let message = if let Text::Literal(text) = &case.text {
        assert_eq!(
            *text, expanded,
            "{id}: the catalog constant is the audit's literal"
        );
        (*text).to_owned()
    } else {
        expanded
    };
    Expected {
        status,
        code: error_code(id, &kind),
        field: cell("BMS field").to_owned(),
        message,
    }
}

/// Asserts that `answer` is the error `expected`, its `fields` naming the BMS
/// field with the same text.
fn assert_answer(case: &Case, expected: &Expected, answer: &Resp) {
    assert_eq!(
        answer.0, expected.status,
        "{} GET {} ({}): {}",
        case.id, case.path, case.proves, answer.2
    );
    assert_error(
        answer,
        expected.status,
        expected.code,
        &expected.message,
        &[(expected.field.as_str(), expected.message.as_str())],
    );
}

#[test]
fn every_coactvwc_validation_row_answers_as_the_audit_records() {
    let validation = validation_audit();
    let cases = cases(&Keys::from_audit(&parity_audit()));
    let defined: BTreeSet<String> = validation_ids(PROGRAM).into_iter().collect();
    let exercised: BTreeSet<String> = cases.iter().map(|case| case.id.to_owned()).collect();
    assert_eq!(
        exercised, defined,
        "the case table must exercise every {PROGRAM} row of the validation audit, and no other"
    );
    let expected: Vec<Expected> = cases
        .iter()
        .map(|case| expected(&validation, case))
        .collect();
    let fields: BTreeSet<&str> = expected
        .iter()
        .map(|answer| answer.field.as_str())
        .collect();
    assert_eq!(
        fields.len(),
        1,
        "P-COACTVWC-017: every {PROGRAM} answer marks the one field the cursor goes to: {fields:?}"
    );
    let app = TestApp::start();
    block_on(async {
        let token = app.user_token().await;
        for (case, expected) in cases.iter().zip(&expected) {
            for sql in case.before {
                app.stack().exec_sql(sql).await;
            }
            let answer = app.get(&case.path, Some(&token)).await;
            for sql in case.after {
                app.stack().exec_sql(sql).await;
            }
            assert_answer(case, expected, &answer);
        }
    });
}

/// How a section A field is stored in its fixture and shown in the body.
#[derive(Clone, Copy, Debug)]
enum Form {
    /// Alphanumeric: the bytes, shown right-trimmed as stored.
    Text,
    /// Unsigned display digits: the bytes, shown as they are.
    Digits,
    /// Signed zoned with a trailing overpunch and `scale` implied decimals:
    /// shown as a decimal string.
    Signed { scale: usize },
}

/// One stored field of a section A record layout.
#[derive(Debug)]
struct Field {
    /// The column the field maps to, which the body uses as its key.
    column: String,
    /// The 0-based offset of the field in the record.
    offset: usize,
    /// The length of the field in bytes.
    length: usize,
    /// How it is stored and shown.
    form: Form,
}

/// The implied decimals of the PIC clause ending `pic_cell`, for example 2
/// for `ACCT-CURR-BAL S9(10)V99`.
fn scale(id: &str, pic_cell: &str) -> usize {
    let pic = pic_cell.split_whitespace().last().unwrap_or_default();
    pic.split_once('V').map_or(0, |(_, decimals)| {
        match decimals
            .strip_prefix("9(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            Some(count) => count
                .parse()
                .unwrap_or_else(|_| panic!("{id}: `{pic}` has no decimal count")),
            None if decimals.bytes().all(|byte| byte == b'9') => decimals.len(),
            None => panic!("{id}: `{pic}` has unreadable decimals"),
        }
    })
}

/// The field a section A row records, or `None` for a `FILLER`, which is not
/// stored.
fn layout_field(table: &Table, row: &Row) -> Option<Field> {
    let id = table.cell(row, "ID").unwrap_or_default();
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
    };
    let column = key_values(cell(EXPECTED_RESULT))
        .into_iter()
        .find(|pair| pair.key == "column")
        .unwrap_or_else(|| panic!("{id} records no column="))
        .value;
    if column == "—" {
        return None;
    }
    let number = |name: &str| {
        cell(name)
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("{id} has no numeric `{name}`"))
    };
    let condition = cell("Input condition");
    let form = if condition.starts_with("alphanumeric") {
        Form::Text
    } else if condition.starts_with("unsigned display") {
        Form::Digits
    } else if condition.starts_with("signed") {
        Form::Signed {
            scale: scale(id, cell("Copybook field or PIC clause")),
        }
    } else {
        panic!("{id}: no fixture form for `{condition}`")
    };
    Some(Field {
        column,
        offset: number("Offset"),
        length: number("Length"),
        form,
    })
}

/// The stored fields of the section A layout of `copybook`, for example
/// `CVACT01Y`, in record order.
fn layout(parity: &Document, copybook: &str) -> Vec<Field> {
    let fields: Vec<Field> = parity
        .section_tables("A.")
        .filter(|table| {
            table.is_data_table()
                && table
                    .subsection
                    .as_deref()
                    .is_some_and(|subsection| subsection.starts_with(copybook))
        })
        .flat_map(|table| table.rows.iter().filter_map(|row| layout_field(table, row)))
        .collect();
    assert!(!fields.is_empty(), "section A records no {copybook} layout");
    fields
}

/// The decimal text of the signed zoned field `bytes`, whose last byte holds
/// the sign as an ASCII overpunch (`{` and `A` to `I` for +0 to +9, `}` and
/// `J` to `R` for -0 to -9), with `scale` implied decimals, written as the API
/// writes a `Decimal`: `00000001940{` with scale 2 is `194.00`.
fn overpunch(bytes: &str, scale: usize) -> String {
    let Some((&last, head)) = bytes.as_bytes().split_last() else {
        panic!("an empty signed field");
    };
    let (digit, negative) = match last {
        b'{' => (b'0', false),
        b'A'..=b'I' => (b'1' + (last - b'A'), false),
        b'}' => (b'0', true),
        b'J'..=b'R' => (b'1' + (last - b'J'), true),
        other => panic!("{bytes:?} ends in {other:?}, not an overpunch"),
    };
    let mut digits = String::from_utf8_lossy(head).into_owned();
    digits.push(char::from(digit));
    assert!(
        digits.bytes().all(|byte| byte.is_ascii_digit()) && digits.len() > scale,
        "{bytes:?} is not a zoned decimal with {scale} decimals"
    );
    let (whole, fraction) = digits.split_at(digits.len() - scale);
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let sign = if negative && digits.bytes().any(|byte| byte != b'0') {
        "-"
    } else {
        ""
    };
    if scale == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    }
}

/// The value `field` holds in the fixture record `line` of `file`, as the
/// body shows it.
fn decode(file: &str, line: &str, field: &Field) -> String {
    let bytes = line
        .get(field.offset..field.offset + field.length)
        .unwrap_or_else(|| panic!("{file} record {line:?} does not reach `{}`", field.column));
    match field.form {
        Form::Text => bytes.trim_end_matches(' ').to_owned(),
        Form::Digits => bytes.to_owned(),
        Form::Signed { scale } => overpunch(bytes, scale),
    }
}

/// The records of the ASCII fixture `file` whose `column` field is `key`.
fn fixture_records(file: &str, layout: &[Field], column: &str, key: &str) -> Vec<String> {
    let field = layout
        .iter()
        .find(|field| field.column == column)
        .unwrap_or_else(|| panic!("the {file} layout has no `{column}`"));
    let path = fixtures::ascii_dir().join(file);
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
    text.lines()
        .filter(|line| decode(file, line, field) == key)
        .map(str::to_owned)
        .collect()
}

/// The one record of `file` whose `column` is `key`.
fn fixture_record(file: &str, layout: &[Field], column: &str, key: &str) -> String {
    let mut records = fixture_records(file, layout, column, key);
    assert_eq!(
        records.len(),
        1,
        "{file} must hold one record with {column} {key}"
    );
    records.remove(0)
}

/// The value `column` of `line`, a record of `file` with `layout`.
fn fixture_value(file: &str, line: &str, layout: &[Field], column: &str) -> String {
    let field = layout
        .iter()
        .find(|field| field.column == column)
        .unwrap_or_else(|| panic!("the {file} layout has no `{column}`"));
    decode(file, line, field)
}

/// The string `name` of the body.
fn text<'a>(body: &'a Value, name: &str) -> &'a str {
    body[name]
        .as_str()
        .unwrap_or_else(|| panic!("the body has no string `{name}`: {body}"))
}

/// Asserts that the body shows every value the section D row `id` records,
/// its `value=` under `value_field`.
fn assert_section_d(parity: &Document, body: &Value, id: &str, value_field: &str) {
    for pair in expected_values(parity, id) {
        if pair.key == "kind" {
            continue;
        }
        let field = if pair.key == "value" {
            value_field
        } else {
            pair.key.as_str()
        };
        assert_eq!(
            body[field].as_str(),
            Some(unquote(&pair.value).as_str()),
            "{id} records {}={} as `{field}`: {body}",
            pair.key,
            pair.value
        );
    }
}

/// Asserts that the body shows the fixture records of its account: the
/// `ACCTDAT` record of `account_id`, the account's first cross-reference in
/// card-number order, and the `CUSTDAT` record of that cross-reference's
/// customer, each field decoded at its section A offset. Every stored field
/// but `ACCT-ADDR-ZIP` is shown, and the body holds nothing else but the card
/// number and the version token.
fn assert_fixture_records(parity: &Document, body: &Value) {
    let account_layout = layout(parity, "CVACT01Y");
    let xref_layout = layout(parity, "CVACT03Y");
    let customer_layout = layout(parity, "CVCUS01Y");
    let account_id = text(body, "account_id");
    let account = fixture_record(ACCOUNT_FILE, &account_layout, "account_id", account_id);
    let xref = fixture_records(XREF_FILE, &xref_layout, "xref_acct_id", account_id)
        .into_iter()
        .min_by_key(|line| fixture_value(XREF_FILE, line, &xref_layout, "xref_card_num"))
        .unwrap_or_else(|| panic!("{XREF_FILE} holds no cross-reference of {account_id}"));
    let customer_id = fixture_value(XREF_FILE, &xref, &xref_layout, "xref_cust_id");
    assert_eq!(
        text(body, "card_number"),
        fixture_value(XREF_FILE, &xref, &xref_layout, "xref_card_num"),
        "P-COACTVWC-007: the card of the cross-reference the customer is read through"
    );
    let customer = fixture_record(CUSTOMER_FILE, &customer_layout, "customer_id", &customer_id);
    let mut shown = BTreeSet::from(["card_number".to_owned(), "version".to_owned()]);
    let mut unshown = Vec::new();
    for (file, line, fields) in [
        (ACCOUNT_FILE, &account, &account_layout),
        (CUSTOMER_FILE, &customer, &customer_layout),
    ] {
        for field in fields {
            let value = decode(file, line, field);
            if body.get(&field.column).is_some() {
                assert_eq!(
                    text(body, &field.column),
                    value,
                    "P-COACTVWC-014, P-COACTVWC-015: `{}` shows {file} bytes {} to {}",
                    field.column,
                    field.offset,
                    field.offset + field.length
                );
                shown.insert(field.column.clone());
            } else {
                unshown.push((field.column.clone(), value));
            }
        }
    }
    let [(column, zip)] = unshown.as_slice() else {
        panic!("P-COACTVWC-014: every stored field but ACCT-ADDR-ZIP is shown, not {unshown:?}");
    };
    assert_eq!(
        column, "acct_addr_zip",
        "P-COACTVWC-014 moves no ACCT-ADDR-ZIP"
    );
    assert!(
        !zip.is_empty() && text(body, "acct_group_id") != zip,
        "E-06: ACCT-GROUP-ID is read at its own offset, not from the ZIP bytes {zip:?} before it"
    );
    let keys: BTreeSet<String> = body
        .as_object()
        .map(|record| record.keys().cloned().collect())
        .unwrap_or_default();
    assert_eq!(
        keys, shown,
        "P-COACTVWC-011, P-COACTVWC-025: the body is the account and customer fields, the card \
         number and the version token, with no message"
    );
}

/// Asserts that `version` is a token of 16 lower-case hex digits.
fn assert_version(version: &str) {
    assert!(
        version.len() == VERSION_LEN
            && version
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "the version token must be {VERSION_LEN} lower-case hex digits: {version:?}"
    );
}

#[test]
fn account_00000000001_shows_its_records_and_a_stable_version() {
    let parity = parity_audit();
    let path = account_path(&expected_text(&parity, EXISTING_ACCOUNT_ROW, "value"));
    let app = TestApp::start();
    let (first, second) = block_on(async {
        let token = app.user_token().await;
        let first = app.get(&path, Some(&token)).await;
        let second = app.get(&path, Some(&token)).await;
        (first, second)
    });
    for (status, _, raw) in [&first, &second] {
        assert_eq!(
            *status,
            StatusCode::OK,
            "P-COACTVWC-011: all three reads find their record: {raw}"
        );
    }
    let (_, mut body, _) = first;
    assert_eq!(
        second.1, body,
        "P-COACTVWC-011: the view writes nothing, so a second GET answers the same body and token"
    );
    assert_section_d(&parity, &body, EXISTING_ACCOUNT_ROW, "account_id");
    assert_section_d(&parity, &body, EXISTING_CUSTOMER_ROW, "customer_id");
    assert_fixture_records(&parity, &body);
    assert_version(text(&body, "version"));
    // The token is a digest keyed with the server's `CARDDEMO_JWT_SECRET`, not a
    // record value, so the snapshot holds the records only.
    if let Some(record) = body.as_object_mut() {
        record.remove("version");
    }
    insta::assert_yaml_snapshot!("account_00000000001", body);
}

/// The `Copybook field or PIC clause` column of the parity audit.
const FIELD_COLUMN: &str = "Copybook field or PIC clause";

/// The `BMS map` column of the function map.
const BMS_MAP: &str = "BMS map";

/// The `Key or field` column of the function map.
const KEY_OR_FIELD: &str = "Key or field";

/// The `COBOL action` column of the function map.
const COBOL_ACTION: &str = "COBOL action";

/// The `UI control` column of the function map.
const UI_CONTROL: &str = "UI control";

/// The `Endpoint` column of the function map.
const ENDPOINT: &str = "Endpoint";

/// The BMS map `COACTVWC` sends, as the function map names it.
const MAP: &str = "COACTVW";

/// The operation `COACTVWC` becomes, as the function map writes it.
const VIEW_ENDPOINT: &str = "GET /accounts/{id}";

/// The `Endpoint` cell of a control that only moves to another route.
const NAVIGATION_ONLY: &str = "— (navigation)";

/// The `UI control` and `Endpoint` cell of a key without a control.
const NO_CONTROL: &str = "—";

/// AID keys named in the query string. The view operation takes no key, so
/// each request must be answered as its ENTER.
const AID_QUERIES: [&str; 3] = ["?aid=PF5", "?aid=PF12&key=PA1", "?eibaid=CLEAR"];

/// Request paths that name no account, as the first entry of `COACTVWC`
/// holds none: it sends the empty map before any key is entered.
const KEYLESS_PATHS: [&str; 2] = ["/accounts", "/accounts/"];

/// The words after the status of the response the abend rows name.
const UNEXPECTED_RESPONSE: &str = " unexpected-error response";

/// The keys an error body may hold.
const ERROR_KEYS: [&str; 3] = ["code", "message", "fields"];

/// The keys of each `fields` entry of an error body.
const FIELD_KEYS: [&str; 2] = ["field", "message"];

/// The media type of every answer.
const JSON_MEDIA_TYPE: &str = "application/json";

/// The user `TestApp::user_token` signs on, `USER0001` of type `U`: the
/// identity the handler is called with directly.
const SIGNED_ON_USER: &str = "USER0001";

/// One HTTP answer of the dialogue, with the headers its rows read.
struct Answer {
    /// The request, as failure messages show it.
    request: String,
    /// The status.
    status: StatusCode,
    /// The `Content-Type` header, when present and readable.
    content_type: Option<String>,
    /// Whether the answer sets a cookie, which would carry state to the next
    /// request.
    sets_cookie: bool,
    /// The body parsed as JSON, `Value::Null` when it is not JSON.
    body: Value,
    /// The raw body.
    raw: String,
}

impl Answer {
    /// Whether the body holds `text` anywhere.
    fn carries(&self, text: &str) -> bool {
        self.raw.contains(text)
    }

    /// Whether the answer is declared JSON.
    fn is_json(&self) -> bool {
        self.content_type
            .as_deref()
            .is_some_and(|media| media.starts_with(JSON_MEDIA_TYPE))
    }

    /// The `message` of the body, then the `message` of each `fields` entry.
    fn messages(&self) -> Vec<&str> {
        let entries = self.body["fields"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        self.body["message"]
            .as_str()
            .into_iter()
            .chain(entries.iter().filter_map(|entry| entry["message"].as_str()))
            .collect()
    }
}

/// The answers of one sign-on's requests, in the order they are sent.
struct Dialogue {
    /// The account of [`EXISTING_ACCOUNT_ROW`], the first request after the
    /// sign-on.
    first: Answer,
    /// The same request with each of [`AID_QUERIES`].
    with_aid: Vec<Answer>,
    /// The first of [`BLANK_KEYS`].
    blank: Answer,
    /// The first of [`BLANK_KEYS`] with the first of [`AID_QUERIES`].
    blank_with_aid: Answer,
    /// Every key of [`BLANK_KEYS`] and [`INVALID_KEYS`] as the edit receives
    /// it, with its answer.
    key_errors: Vec<(String, Answer)>,
    /// An account without a cross-reference ([`MISSING_ACCOUNT_ROW`]).
    missing: Answer,
    /// The account of [`EXISTING_ACCOUNT_ROW`] without a bearer token.
    unauthenticated: Answer,
    /// Each of [`KEYLESS_PATHS`].
    keyless: Vec<Answer>,
    /// The account of [`EXISTING_ACCOUNT_ROW`] again, after every request
    /// above.
    again: Answer,
    /// The account of [`EXISTING_ACCOUNT_ROW`] once the pool is closed.
    closed: Answer,
    /// The answer `get_account` itself gives over the closed pool.
    closed_handler: Resp,
}

impl Dialogue {
    /// Every HTTP answer.
    fn answers(&self) -> Vec<&Answer> {
        let mut answers = vec![
            &self.first,
            &self.blank,
            &self.blank_with_aid,
            &self.missing,
            &self.unauthenticated,
            &self.again,
            &self.closed,
        ];
        answers.extend(&self.with_aid);
        answers.extend(self.key_errors.iter().map(|(_, answer)| answer));
        answers.extend(&self.keyless);
        answers
    }
}

/// Sends `GET path`, with `token` as the bearer token when given, and keeps
/// what the dialogue rows read of the answer.
async fn exchange(app: &TestApp, path: &str, token: Option<&str>) -> Answer {
    let builder = app.client().get(app.url(path));
    let (builder, request) = match token {
        Some(token) => (builder.bearer_auth(token), format!("GET {path}")),
        None => (builder, format!("GET {path} without a token")),
    };
    let response = builder
        .send()
        .await
        .unwrap_or_else(|error| panic!("{request} failed: {error}"));
    let status = response.status();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let sets_cookie = response.headers().contains_key(SET_COOKIE);
    let raw = response
        .text()
        .await
        .unwrap_or_else(|error| panic!("the body of {request} cannot be read: {error}"));
    let body = serde_json::from_str(&raw).unwrap_or(Value::Null);
    Answer {
        request,
        status,
        content_type,
        sets_cookie,
        body,
        raw,
    }
}

/// The JSON body of `error`, as the router writes it.
fn error_body(error: &ApiError) -> Value {
    serde_json::to_value(error.body())
        .unwrap_or_else(|failure| panic!("the error body does not serialize: {failure}"))
}

/// Closes the pool `app` serves from, then calls `get_account` for `account`
/// directly, with the signed-on user the `AuthUser` extractor would give, and
/// returns its answer. No later request reaches the database.
async fn closed_pool_handler_answer(app: &TestApp, account: &str) -> Resp {
    let db = Db::from_pool(app.stack().pool().clone());
    let state = AppState::new(db, Settings::default())
        .unwrap_or_else(|error| panic!("the default settings were rejected: {error}"));
    state.db().close().await;
    let user = AuthUser {
        user_id: SIGNED_ON_USER.to_owned(),
        usr_type: UserType::User,
    };
    let Err(error) = get_account(user, State(state), ApiPath(account.to_owned())).await else {
        panic!("get_account showed account {account} over a closed pool");
    };
    let body = error_body(&error);
    let raw = body.to_string();
    (error.status(), body, raw)
}

/// Signs on and sends the dialogue's requests in order, the pool closed last.
async fn converse(app: &TestApp, keys: &Keys) -> Dialogue {
    let bearer = app.user_token().await;
    let token = Some(bearer.as_str());
    let account = account_path(&keys.account);
    let first = exchange(app, &account, token).await;
    let mut with_aid = Vec::new();
    for query in AID_QUERIES {
        with_aid.push(exchange(app, &format!("{account}{query}"), token).await);
    }
    let blank_path = account_path(BLANK_KEYS[0]);
    let blank = exchange(app, &blank_path, token).await;
    let aid = AID_QUERIES[0];
    let blank_with_aid = exchange(app, &format!("{blank_path}{aid}"), token).await;
    let mut key_errors = Vec::new();
    for key in BLANK_KEYS.iter().chain(&INVALID_KEYS) {
        // The segments of `BLANK_KEYS` encode their spaces as `%20`.
        let entered = key.replace("%20", " ");
        key_errors.push((entered, exchange(app, &account_path(key), token).await));
    }
    let missing = exchange(app, &account_path(&keys.missing_account), token).await;
    let unauthenticated = exchange(app, &account, None).await;
    let mut keyless = Vec::new();
    for path in KEYLESS_PATHS {
        keyless.push(exchange(app, path, token).await);
    }
    let again = exchange(app, &account, token).await;
    let closed_handler = closed_pool_handler_answer(app, &keys.account).await;
    let closed = exchange(app, &account, token).await;
    Dialogue {
        first,
        with_aid,
        blank,
        blank_with_aid,
        key_errors,
        missing,
        unauthenticated,
        keyless,
        again,
        closed,
        closed_handler,
    }
}

/// The `column` cell of the parity row `id`.
fn parity_cell<'a>(parity: &'a Document, id: &str, column: &str) -> &'a str {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    table
        .cell(row, column)
        .unwrap_or_else(|| panic!("{id} has no `{column}` cell"))
}

/// The `Expected result` cell of the parity row `id`.
fn expected_cell<'a>(parity: &'a Document, id: &str) -> &'a str {
    parity_cell(parity, id, EXPECTED_RESULT)
}

/// The double-quoted texts of the code spans of the `Expected result` of the
/// parity row `id`, in order and without their quotes.
fn row_texts(parity: &Document, id: &str) -> Vec<String> {
    let texts: Vec<String> = cell_code_spans(expected_cell(parity, id))
        .into_iter()
        .filter_map(|span| {
            span.text
                .strip_prefix('"')
                .and_then(|text| text.strip_suffix('"'))
                .map(str::to_owned)
        })
        .collect();
    assert!(!texts.is_empty(), "{id} quotes no text");
    texts
}

/// The COBOL literals of `text`, right-trimmed: each run between two single
/// quotes made only of upper-case letters, digits, spaces and hyphens. So
/// `('COACTVW ' cut to 'COACTVW')` gives `COACTVW`, and the run an apostrophe
/// of the prose opens gives nothing.
fn cobol_literals(text: &str) -> BTreeSet<String> {
    text.split('\'')
        .skip(1)
        .step_by(2)
        .map(str::trim_end)
        .filter(|literal| {
            !literal.is_empty()
                && literal.bytes().all(|byte| {
                    byte.is_ascii_uppercase()
                        || byte.is_ascii_digit()
                        || byte == b' '
                        || byte == b'-'
                })
        })
        .map(str::to_owned)
        .collect()
}

/// The header forms a cell writes, such as `MM/DD/YY`: words of three
/// two-letter groups joined by one `/` or `:`.
fn header_shapes(cell: &str) -> Vec<String> {
    cell.split(|ch: char| ch.is_whitespace() || ch == '=' || ch == ';')
        .filter(|word| {
            let bytes = word.as_bytes();
            bytes.len() == 8
                && [0, 1, 3, 4, 6, 7]
                    .iter()
                    .all(|&at| bytes[at].is_ascii_uppercase())
                && matches!(bytes[2], b'/' | b':')
                && bytes[5] == bytes[2]
        })
        .map(str::to_owned)
        .collect()
}

/// Whether `text` has the form `shape`: a digit for each letter, the
/// separators as they are.
fn fits_shape(text: &str, shape: &str) -> bool {
    text.len() == shape.len()
        && text.bytes().zip(shape.bytes()).all(|(have, want)| {
            if want.is_ascii_uppercase() {
                have.is_ascii_digit()
            } else {
                have == want
            }
        })
}

/// The length `n` of the alphanumeric `name X(n)` a field cell records.
fn pic_length(id: &str, cell: &str, name: &str) -> usize {
    cell.split_once(format!("{name} X(").as_str())
        .and_then(|(_, rest)| rest.split_once(')'))
        .and_then(|(digits, _)| digits.parse().ok())
        .unwrap_or_else(|| panic!("{id} records no `{name} X(n)`: {cell}"))
}

/// The HTTP status a cell writes right before `words`, as in
/// `500 unexpected-error response`.
fn status_before(id: &str, cell: &str, words: &str) -> StatusCode {
    cell.split_once(words)
        .and_then(|(before, _)| before.rsplit(' ').next())
        .and_then(|code| code.parse::<u16>().ok())
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} names no status before `{words}`: {cell}"))
}

/// Every string of `value`, at any depth.
fn strings(value: &Value) -> Vec<&str> {
    match value {
        Value::String(text) => vec![text.as_str()],
        Value::Array(items) => items.iter().flat_map(strings).collect(),
        Value::Object(record) => record.values().flat_map(strings).collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) => Vec::new(),
    }
}

/// Every object key of `value`, at any depth.
fn object_keys(value: &Value) -> Vec<&str> {
    match value {
        Value::Array(items) => items.iter().flat_map(object_keys).collect(),
        Value::Object(record) => record
            .iter()
            .flat_map(|(key, item)| std::iter::once(key.as_str()).chain(object_keys(item)))
            .collect(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => Vec::new(),
    }
}

/// The keys of the object `value`, none for any other value.
fn top_keys(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .map(|record| record.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// P-COACTVWC-001: a key named in the query is no input of the view
/// operation, so the request is answered as its ENTER, byte for byte, and no
/// answer carries the invalid-key text.
fn assert_aid_keys_answer_as_enter(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-001";
    let rule = expected_text(parity, id, "rule");
    let pairs = dialogue
        .with_aid
        .iter()
        .map(|keyed| (keyed, &dialogue.first))
        .chain([(&dialogue.blank_with_aid, &dialogue.blank)]);
    for (keyed, enter) in pairs {
        assert_eq!(
            (keyed.status, &keyed.raw),
            (enter.status, &enter.raw),
            "{id} ({rule}): {} must answer as {}",
            keyed.request,
            enter.request
        );
    }
    let invalid_key = messages::CCDA_MSG_INVALID_KEY.trim_end();
    for answer in dialogue.answers() {
        assert!(
            !answer.carries(invalid_key),
            "{id} ({rule}): {} carries the invalid-key text",
            answer.request
        );
    }
}

/// P-COACTVWC-003: the first entry sends the empty map and reads nothing. A
/// request that names no account is no turn of `COACTVWC`: it is answered as
/// a path no route serves, not as a blank key entered with ENTER.
fn assert_no_request_reads_without_a_key(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-003";
    let action = expected_text(parity, id, "action");
    let unrouted = ApiError::route_not_found();
    let unrouted_body = error_body(&unrouted);
    for answer in &dialogue.keyless {
        assert_eq!(
            (answer.status, &answer.body),
            (unrouted.status(), &unrouted_body),
            "{id} ({action}): {}: {}",
            answer.request,
            answer.raw
        );
        assert_ne!(
            answer.raw, dialogue.blank.raw,
            "{id}: {} is not the blank key",
            answer.request
        );
    }
}

/// P-COACTVWC-012: `COMMON-RETURN` keeps the dialogue in the COMMAREA, and its
/// stateless replacement keeps nothing. The account answers the same before
/// and after every other request, a repeated error answers the same, no answer
/// sets a cookie, and a request without the bearer token is refused although
/// the client signed on before.
fn assert_no_conversation_state(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-012";
    let replacement = expected_text(parity, id, "replacement");
    assert!(
        replacement.contains(VIEW_ENDPOINT),
        "{id} names {VIEW_ENDPOINT} as its replacement: {replacement}"
    );
    assert_eq!(
        dialogue.again.raw, dialogue.first.raw,
        "{id} ({replacement}): the account answers as on the first request"
    );
    let (_, repeated) = &dialogue.key_errors[0];
    assert_eq!(
        repeated.raw, dialogue.blank.raw,
        "{id} ({replacement}): a repeated blank key answers as the first time"
    );
    for answer in dialogue.answers() {
        assert!(
            !answer.sets_cookie,
            "{id} ({replacement}): {} sets a cookie",
            answer.request
        );
    }
    let refused = ApiError::unauthenticated();
    let unauthenticated = &dialogue.unauthenticated;
    assert_eq!(
        (unauthenticated.status, &unauthenticated.body),
        (refused.status(), &error_body(&refused)),
        "{id} ({replacement}): {}",
        unauthenticated.request
    );
}

/// P-COACTVWC-013: no answer carries the screen header: neither the
/// transaction and program names the row records, nor a date or time in the
/// header's `MM/DD/YY` and `HH:MM:SS` forms.
fn assert_no_screen_header(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-013";
    let cell = expected_cell(parity, id);
    let names: Vec<String> = key_values(cell)
        .into_iter()
        .filter(|pair| pair.key.ends_with("NAMEO"))
        .flat_map(|pair| cobol_literals(&pair.value))
        .collect();
    assert_eq!(
        names.len(),
        2,
        "{id} records TRNNAMEO and PGMNAMEO: {names:?}"
    );
    let shapes = header_shapes(cell);
    assert_eq!(
        shapes.len(),
        2,
        "{id} records the CURDATEO and CURTIMEO forms: {shapes:?}"
    );
    for answer in dialogue.answers() {
        for name in &names {
            assert!(
                !answer.carries(name),
                "{id}: {} carries the header name {name}",
                answer.request
            );
        }
        for text in strings(&answer.body) {
            assert!(
                !shapes.iter().any(|shape| fits_shape(text, shape)),
                "{id}: {} carries {text:?}, a header date or time",
                answer.request
            );
        }
    }
}

/// P-COACTVWC-016: the information line is not carried. A shown account holds
/// no message, neither the prompt every send shows nor the never-set
/// `WS-INFORM-OUTPUT` text, and an error's message is `WS-RETURN-MSG`, the one
/// text its body and each of its `fields` entries carry.
fn assert_no_information_line(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-016";
    let texts = row_texts(parity, id);
    assert_eq!(
        texts.len(),
        2,
        "{id} quotes the prompt and the WS-INFORM-OUTPUT text: {texts:?}"
    );
    for answer in dialogue.answers() {
        for text in &texts {
            assert!(
                !answer.carries(text),
                "{id}: {} carries the information text {text:?}",
                answer.request
            );
        }
        let messages = answer.messages();
        if answer.status.is_success() {
            assert!(
                messages.is_empty(),
                "{id}: {} shows a message: {}",
                answer.request,
                answer.raw
            );
        } else {
            let (top, entries) = messages.split_first().unwrap_or_else(|| {
                panic!("{id}: {} has no message: {}", answer.request, answer.raw)
            });
            assert!(
                entries.iter().all(|entry| entry == top),
                "{id}: every `fields` entry of {} carries WS-RETURN-MSG {top:?}",
                answer.request
            );
        }
    }
}

/// P-COACTVWC-018: `1400-SEND-SCREEN` becomes the HTTP response. Every turn is
/// answered with one JSON body, the first request after the sign-on is
/// processed as input already (the re-entry `1400` sets), and no answer names
/// the map or the mapset the row records.
fn assert_one_json_response(parity: &Document, keys: &Keys, dialogue: &Dialogue) {
    let id = "P-COACTVWC-018";
    let replacement = expected_text(parity, id, "replacement");
    let names = cobol_literals(expected_cell(parity, id));
    assert_eq!(
        names.len(),
        2,
        "{id} records the map and the mapset: {names:?}"
    );
    let first = &dialogue.first;
    assert_eq!(
        (first.status, first.body["account_id"].as_str()),
        (StatusCode::OK, Some(keys.account.as_str())),
        "{id} ({replacement}): the first request reads the account it names: {}",
        first.raw
    );
    for answer in dialogue.answers() {
        assert!(
            answer.is_json() && answer.body.is_object(),
            "{id} ({replacement}): {} answers {:?}: {}",
            answer.request,
            answer.content_type,
            answer.raw
        );
        for name in &names {
            assert!(
                !answer.carries(name),
                "{id}: {} names {name}",
                answer.request
            );
        }
    }
}

/// P-COACTVWC-022: `SEND-PLAIN-TEXT` and its text are reached only from a
/// COMMAREA context that is neither first entry nor re-entry, which no request
/// carries; no answer holds the text, the handler's own over a closed pool
/// included.
fn assert_no_plain_text_scenario(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-022";
    let texts = row_texts(parity, id);
    let [text] = texts.as_slice() else {
        panic!("{id} quotes one text: {texts:?}");
    };
    for answer in dialogue.answers() {
        assert!(
            !answer.carries(text),
            "{id}: {} carries {text:?}",
            answer.request
        );
    }
    assert!(
        !dialogue.closed_handler.2.contains(text.as_str()),
        "{id}: get_account answers {text:?}"
    );
}

/// P-COACTVWC-023, P-COACTVWC-027: `SEND-LONG-TEXT` is not carried. It would
/// send `WS-LONG-MSG` as plain text of the length the row's field cell
/// records; every answer is JSON, and each of its messages fits the 75 bytes
/// of `WS-RETURN-MSG`, which is shorter.
fn assert_no_long_text(parity: &Document, id: &str, dialogue: &Dialogue) {
    let long = pic_length(id, parity_cell(parity, id, FIELD_COLUMN), "WS-LONG-MSG");
    assert!(
        RETURN_MSG_LEN < long,
        "{id}: WS-LONG-MSG X({long}) is longer than WS-RETURN-MSG"
    );
    for answer in dialogue.answers() {
        assert!(
            answer.is_json(),
            "{id}: {} is sent as {:?}",
            answer.request,
            answer.content_type
        );
        for message in answer.messages() {
            assert!(
                message.len() <= RETURN_MSG_LEN,
                "{id}: {} carries {} bytes: {message:?}",
                answer.request,
                message.len()
            );
        }
    }
}

/// P-COACTVWC-024, P-COACTVWC-026: an abend is answered with the HTTP
/// mapping's unexpected-error response, of the status the row names, with
/// neither `UNEXPECTED ABEND OCCURRED.` nor the `ABEND-DATA` literals, over
/// HTTP and from the handler itself, once the pool both read through is
/// closed.
fn assert_abend_is_the_unexpected_error(parity: &Document, id: &str, dialogue: &Dialogue) {
    let cell = expected_cell(parity, id);
    let status = status_before(id, cell, UNEXPECTED_RESPONSE);
    let texts = row_texts(parity, id);
    let literals = cobol_literals(cell);
    let unexpected = ApiError::unexpected();
    let expected = error_body(&unexpected);
    assert_eq!(
        (unexpected.status(), expected["message"].as_str()),
        (status, Some(messages::UNEXPECTED_ERROR)),
        "{id}: the unexpected-error response of the HTTP mapping"
    );
    let closed = &dialogue.closed;
    let (handler_status, handler_body, handler_raw) = &dialogue.closed_handler;
    let answers = [
        (
            closed.request.as_str(),
            closed.status,
            &closed.body,
            &closed.raw,
        ),
        (
            "get_account over the closed pool",
            *handler_status,
            handler_body,
            handler_raw,
        ),
    ];
    for (source, answer_status, body, raw) in answers {
        assert_eq!(
            (answer_status, body),
            (status, &expected),
            "{id}: {source}: {raw}"
        );
        for text in texts.iter().chain(&literals) {
            assert!(
                !raw.contains(text.as_str()),
                "{id}: {source} carries {text:?}"
            );
        }
    }
}

/// P-COACTVWC-028: the `IF INPUT-ERROR` after the `EVALUATE` is never reached,
/// every branch having answered. A key the edit refuses is answered once, with
/// exactly the error of `2210-EDIT-ACCOUNT` and its one field, which nothing
/// after the edit changes.
fn assert_input_error_answered_once(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-028";
    let disposition = expected_text(parity, id, "disposition");
    for (entered, answer) in &dialogue.key_errors {
        let Err(error) = edit_account_id(Some(entered.as_str())) else {
            panic!("{id}: the edit accepts {entered:?}");
        };
        let refused = ApiError::from(error);
        assert_eq!(
            (answer.status, &answer.body),
            (refused.status(), &error_body(&refused)),
            "{id} ({disposition}): {}",
            answer.request
        );
        let fields = answer.body["fields"].as_array().map_or(0, Vec::len);
        assert_eq!(
            fields, 1,
            "{id}: {} is answered with one field: {}",
            answer.request, answer.raw
        );
    }
}

/// P-COACTVWC-029: `INFOMSGC`, the attribute byte of the information line, has
/// no API counterpart. No answer names the field, an error body holds only
/// `code`, `message` and `fields`, and each `fields` entry only its field and
/// message, with no colour or intensity.
fn assert_no_screen_attributes(parity: &Document, dialogue: &Dialogue) {
    let id = "P-COACTVWC-029";
    let cell = parity_cell(parity, id, FIELD_COLUMN);
    let field = cell
        .split_whitespace()
        .next()
        .and_then(|attribute| attribute.strip_suffix('C'))
        .unwrap_or_else(|| panic!("{id} names no colour attribute: {cell}"));
    let field_keys = BTreeSet::from(FIELD_KEYS);
    for answer in dialogue.answers() {
        for key in object_keys(&answer.body) {
            assert!(
                !key.to_ascii_uppercase().contains(field),
                "{id}: {} has the key {key}",
                answer.request
            );
        }
        if !answer.status.is_success() {
            let keys = top_keys(&answer.body);
            assert!(
                keys.iter().all(|key| ERROR_KEYS.contains(key)),
                "{id}: {} holds {keys:?}",
                answer.request
            );
            let entries = answer.body["fields"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default();
            for entry in entries {
                assert_eq!(top_keys(entry), field_keys, "{id}: {}", answer.request);
            }
        }
    }
}

#[test]
fn each_coactvwc_turn_is_one_stateless_json_answer_without_screen_texts() {
    let parity = parity_audit();
    let keys = Keys::from_audit(&parity);
    let app = TestApp::start();
    let dialogue = block_on(converse(&app, &keys));
    assert_aid_keys_answer_as_enter(&parity, &dialogue);
    assert_no_request_reads_without_a_key(&parity, &dialogue);
    assert_no_conversation_state(&parity, &dialogue);
    assert_no_screen_header(&parity, &dialogue);
    assert_no_information_line(&parity, &dialogue);
    assert_one_json_response(&parity, &keys, &dialogue);
    assert_no_plain_text_scenario(&parity, &dialogue);
    for id in ["P-COACTVWC-023", "P-COACTVWC-027"] {
        assert_no_long_text(&parity, id, &dialogue);
    }
    for id in ["P-COACTVWC-024", "P-COACTVWC-026"] {
        assert_abend_is_the_unexpected_error(&parity, id, &dialogue);
    }
    assert_input_error_answered_once(&parity, &dialogue);
    assert_no_screen_attributes(&parity, &dialogue);
}

/// The parsed `docs/parity/function-map.md` (`F-` rows).
fn function_map() -> Document {
    Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("{FUNCTION_MAP_PATH} cannot be read: {error}"))
}

/// The `column` cell of the function-map row `row` of `table`.
fn map_cell<'a>(table: &'a Table, row: &'a Row, column: &str) -> &'a str {
    table.cell(row, column).unwrap_or_else(|| {
        panic!(
            "the {FUNCTION_MAP_PATH} row of line {} has no `{column}` cell",
            row.line
        )
    })
}

/// The value of the `key=` pair of the `UI control` cell of `row`.
fn control_value(table: &Table, row: &Row, key: &str) -> String {
    let pair = key_values(map_cell(table, row, UI_CONTROL))
        .into_iter()
        .find(|pair| pair.key == key)
        .unwrap_or_else(|| {
            panic!(
                "the {FUNCTION_MAP_PATH} row of line {} records no {key}=",
                row.line
            )
        });
    pair.value
}

/// The key rows of the function-map table of BMS map `map`.
fn map_rows<'a>(functions: &'a Document, map: &str) -> Vec<(&'a Table, &'a Row)> {
    let rows: Vec<(&Table, &Row)> = functions
        .tables()
        .iter()
        .filter(|table| table.is_data_table() && !table.is_escalations_raised())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| table.cell(row, BMS_MAP) == Some(map))
        .collect();
    assert!(!rows.is_empty(), "{FUNCTION_MAP_PATH} has no {map} row");
    rows
}

/// The one row of BMS map `map` whose COBOL action cites the parity row `id`.
fn map_row_citing<'a>(functions: &'a Document, map: &str, id: &str) -> (&'a Table, &'a Row) {
    let citing: Vec<(&Table, &Row)> = map_rows(functions, map)
        .into_iter()
        .filter(|(table, row)| {
            id_tokens(map_cell(table, row, COBOL_ACTION))
                .iter()
                .any(|(kind, token)| *kind == IdKind::Parity && token == id)
        })
        .collect();
    let [found] = citing.as_slice() else {
        panic!(
            "one {map} row of {FUNCTION_MAP_PATH} must cite {id}, not {}",
            citing.len()
        );
    };
    *found
}

/// The published `OpenAPI` document.
fn openapi() -> Value {
    serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"))
}

/// The `OpenAPI` operation of [`VIEW_ENDPOINT`], with its path.
fn view_operation(spec: &Value) -> (&Value, &'static str) {
    let (method, path) = VIEW_ENDPOINT
        .split_once(' ')
        .unwrap_or_else(|| panic!("{VIEW_ENDPOINT} is not a method and a path"));
    let method = method.to_ascii_lowercase();
    let operation = &spec["paths"][path][method.as_str()];
    assert!(
        operation.is_object(),
        "the OpenAPI document has no {VIEW_ENDPOINT}"
    );
    (operation, path)
}

/// P-COACTVWC-001: the AID keys other than ENTER and PF3 have neither a
/// control nor a request, and the view operation's only input is the account
/// number, so no request names a key.
fn assert_other_keys_have_no_control(parity: &Document, functions: &Document, spec: &Value) {
    let id = "P-COACTVWC-001";
    let rule = expected_text(parity, id, "rule");
    let (table, row) = map_row_citing(functions, MAP, id);
    assert!(
        map_cell(table, row, KEY_OR_FIELD).starts_with("other AID keys"),
        "{id}: the {MAP} row citing it is the other-keys row"
    );
    for column in [UI_CONTROL, ENDPOINT] {
        assert_eq!(
            map_cell(table, row, column),
            NO_CONTROL,
            "{id} ({rule}): no `{column}`"
        );
    }
    let (operation, _) = view_operation(spec);
    let inputs: Vec<(&str, &str)> = operation["parameters"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|parameter| {
            (
                parameter["name"].as_str().unwrap_or_default(),
                parameter["in"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        inputs,
        [("id", "path")],
        "{id} ({rule}): {VIEW_ENDPOINT} takes the account number and no key"
    );
    assert!(
        operation.get("requestBody").is_none(),
        "{id} ({rule}): {VIEW_ENDPOINT} takes no body"
    );
}

/// P-COACTVWC-002: PF3 is a navigation link without a request, back to the
/// route that renders the menu program the row names (`COMEN01C`, whose map
/// is `COMEN01`): the route of that map's PF3 control.
fn assert_pf3_links_back_to_the_menu(parity: &Document, functions: &Document) {
    let id = "P-COACTVWC-002";
    let action = expected_text(parity, id, "action");
    let menu_program = action
        .rsplit_once('(')
        .and_then(|(_, rest)| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("{id} names no menu program: {action}"));
    let menu_map = menu_program
        .strip_suffix('C')
        .unwrap_or_else(|| panic!("{id}: {menu_program} is no online program name"));
    let (table, row) = map_row_citing(functions, MAP, id);
    assert!(
        map_cell(table, row, KEY_OR_FIELD).starts_with("PF3"),
        "{id}: the {MAP} row citing it is the PF3 row"
    );
    assert_eq!(
        map_cell(table, row, ENDPOINT),
        NAVIGATION_ONLY,
        "{id} ({action}): PF3 sends no request"
    );
    let (menu_table, menu_row) = map_rows(functions, menu_map)
        .into_iter()
        .find(|(table, row)| map_cell(table, row, KEY_OR_FIELD).starts_with("PF3"))
        .unwrap_or_else(|| panic!("{FUNCTION_MAP_PATH} has no PF3 row of {menu_map}"));
    let menu_route = control_value(menu_table, menu_row, "route");
    assert_eq!(
        control_value(table, row, "target"),
        menu_route,
        "{id} ({action}): the link returns to the route that renders {menu_program}"
    );
}

/// P-COACTVWC-003: the empty map of the first entry is the account lookup
/// form: one `COACTVW` control takes the account number, in an empty textbox,
/// and sends [`VIEW_ENDPOINT`] once a number is typed, and no operation shows
/// an account without one.
fn assert_the_key_is_entered_before_any_request(
    parity: &Document,
    functions: &Document,
    spec: &Value,
) {
    let id = "P-COACTVWC-003";
    let action = expected_text(parity, id, "action");
    let entries: Vec<(&Table, &Row)> = map_rows(functions, MAP)
        .into_iter()
        .filter(|(table, row)| {
            key_values(map_cell(table, row, UI_CONTROL))
                .iter()
                .any(|pair| pair.key == "input")
        })
        .collect();
    let [(table, row)] = entries.as_slice() else {
        panic!(
            "{id} ({action}): one {MAP} control takes the account number, not {}",
            entries.len()
        );
    };
    assert!(
        map_cell(table, row, KEY_OR_FIELD).starts_with("ENTER")
            && control_value(table, row, "input").starts_with("textbox"),
        "{id} ({action}): the account number is typed in a textbox and sent with ENTER"
    );
    assert_eq!(
        map_cell(table, row, ENDPOINT),
        VIEW_ENDPOINT,
        "{id} ({action}): the number typed is sent as the view request"
    );
    let (_, path) = view_operation(spec);
    let keyless = path.rsplit_once('/').map_or(path, |(parent, _)| parent);
    let published = spec["paths"]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document has no paths"));
    assert!(
        !published.contains_key(keyless),
        "{id} ({action}): no operation shows an account without its number"
    );
}

#[test]
fn the_coactvw_keys_become_the_view_operation_or_navigation() {
    let parity = parity_audit();
    let functions = function_map();
    let spec = openapi();
    assert_other_keys_have_no_control(&parity, &functions, &spec);
    assert_pf3_links_back_to_the_menu(&parity, &functions);
    assert_the_key_is_entered_before_any_request(&parity, &functions, &spec);
}
