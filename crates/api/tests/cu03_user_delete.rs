//! CU03, `COUSR03C`: `DELETE /admin/users/{id}`, the Confirm of the delete
//! dialog (PF5, `DELETE-USER-INFO`), and the `GET /admin/users/{id}` fetch
//! that fills the dialog (ENTER, `PROCESS-ENTER-KEY`), proved through the
//! HTTP layer.
//!
//! Every status, BMS field and kind is read from the `COUSR03C` rows of
//! `docs/parity/validation-audit.md`, and every expected text comes from
//! `carddemo_domain::messages`, checked against the text the audit row quotes.
//! The `COUSR03C` rows of section B of `docs/parity/parity-oracle-audit.md`
//! are checked to quote the same catalog texts.
//!
//! `DELETE-USER-INFO` performs `DELETE-USER-SEC-FILE` whatever
//! `READ-USER-SEC-FILE` answered [app/cbl/COUSR03C.cbl:L188-191]. After a
//! failed `READ … UPDATE` the `DELETE` holds no record and fails `INVREQ`, so
//! an unknown user ends with the 500 update-failure text of the `DELETE`, not
//! the read's not-found text (E-46). That defect-like outcome is the audited
//! behaviour and is asserted as such, a 404 nowhere on the delete of an
//! unknown user.
//!
//! Failures the fixture corpus cannot produce are injected with test-only
//! database objects, one test function each: a `BEFORE DELETE` trigger that
//! raises (the delete fails), one that returns `NULL` (the delete finds no
//! row after the read found it), and a view that refuses `FOR UPDATE` (the
//! read for update fails while the bearer token's plain subject read works).
//!
//! The 3270 mechanics of `COUSR03C` have their REST replacements proved in
//! [`the_delete_keys_and_the_header_have_their_rest_replacements`]: ENTER is
//! the fetch and PF5 the delete, PF3, PF4, PF12 and the invalid-key answer
//! send no request, and a request naming another AID key is answered as
//! without it; a request without a session is 401 and deletes nothing; the
//! `{id}` the dialog is bound to replaces the received key; no answer
//! carries the 3270 header.
//!
//! Docker must be running: each test function starts its own seeded
//! `postgres:16.15` container.

pub mod common;

use std::collections::BTreeSet;

use carddemo_api::dto::admin_users::UserDeleteField;
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, PARITY_AUDIT_PATH, Repo, cell_code_spans, code_span,
    id_tokens, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::{block_on, fixtures};
use common::{Resp, TestApp, assert_error, parity_audit, validation_audit};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The user administration route, `POST /admin/users` (CU01).
const USERS_PATH: &str = "/admin/users";

/// The `{id}` path segment of one space: `USRIDINI` entered as `SPACES`.
const BLANK_USER_PATH: &str = "/admin/users/%20";

/// A user ID no seeded `USRSEC` row holds [app/jcl/DUSRSECJ.jcl:L35-44].
const UNKNOWN_USER_PATH: &str = "/admin/users/NOSUCHUS";

/// The user added through CU01 and deleted through CU03.
const ADDED_USER: &str = "DELUSR01";

/// The route of [`ADDED_USER`].
const ADDED_USER_PATH: &str = "/admin/users/DELUSR01";

/// The seeded regular user `USER0005` (`LEE TING`, type `U`)
/// [app/jcl/DUSRSECJ.jcl:L44], the target of the injected failures.
const SEEDED_USER_PATH: &str = "/admin/users/USER0005";

/// The stored row of [`ADDED_USER`], without its password hash.
const ADDED_USER_ROW: &str = "SELECT user_id, sec_usr_fname, sec_usr_lname, sec_usr_type \
                              FROM users WHERE user_id = 'DELUSR01'";

/// The stored row of `USER0005`, without its password hash.
const SEEDED_USER_ROW: &str = "SELECT user_id, sec_usr_fname, sec_usr_lname, sec_usr_type \
                               FROM users WHERE user_id = 'USER0005'";

/// The number of stored users.
const USER_COUNT: &str = "SELECT count(*) FROM users";

/// The number of `DUSRSECJ` `SYSUT1` rows the seed loads
/// [app/jcl/DUSRSECJ.jcl:L35-44].
const SEEDED_USERS: &str = "10";

/// A test-only trigger that makes every user delete fail on the server: the
/// `WHEN OTHER` response of the CICS `DELETE`.
const FAIL_USER_DELETE: &str = "\
    CREATE FUNCTION cu03_fail_user_delete() RETURNS trigger LANGUAGE plpgsql AS $$ \
    BEGIN RAISE EXCEPTION 'cu03 test: the user delete fails'; END $$; \
    CREATE TRIGGER cu03_fail_user_delete BEFORE DELETE ON users \
    FOR EACH ROW EXECUTE FUNCTION cu03_fail_user_delete()";

/// A test-only trigger that skips every user delete, so it affects no row:
/// the `NOTFND` response of a CICS `DELETE` after a successful `READ … UPDATE`.
const SKIP_USER_DELETE: &str = "\
    CREATE FUNCTION cu03_skip_user_delete() RETURNS trigger LANGUAGE plpgsql AS $$ \
    BEGIN RETURN NULL; END $$; \
    CREATE TRIGGER cu03_skip_user_delete BEFORE DELETE ON users \
    FOR EACH ROW EXECUTE FUNCTION cu03_skip_user_delete()";

/// Puts a `DISTINCT` view in place of the user table. A plain read through it
/// works, so the bearer token's subject read still passes, while every
/// `SELECT … FOR UPDATE`, the `READ … UPDATE` of the fetch and of the delete,
/// fails on the server.
const UNLOCKABLE_USERS: &str = "\
    ALTER TABLE users RENAME TO users_offline; \
    CREATE VIEW users AS SELECT DISTINCT * FROM users_offline";

/// Puts the user table back.
const RESTORE_USERS: &str = "DROP VIEW users; ALTER TABLE users_offline RENAME TO users";

/// `MAIN-PARA`'s `EVALUATE EIBAID`: ENTER, PF3, PF4, PF5, PF12 and any other
/// key, each with its action.
const AID_ROW: &str = "P-COUSR03C-001";

/// `RETURN-TO-PREV-SCREEN`: PF3, PF12, and the entry without a COMMAREA that
/// the bearer token replaces; nothing is deleted.
const RETURN_ROW: &str = "P-COUSR03C-006";

/// `RECEIVE-USRDEL-SCREEN`: the user row the dialog is bound to replaces the
/// received key.
const RECEIVE_ROW: &str = "P-COUSR03C-008";

/// `POPULATE-HEADER-INFO`: the 3270 header no answer carries.
const HEADER_ROW: &str = "P-COUSR03C-009";

/// `CLEAR-CURRENT-SCREEN`: PF4, which closes the dialog.
const CLEAR_ROW: &str = "P-COUSR03C-010";

/// The `COUSR03C` screen, as the function map's `BMS map` cell names it.
const MAP: &str = "COUSR03";

/// The route of the fetch, the update and the delete, as published.
const ID_PATH: &str = "/admin/users/{id}";

/// The key [`AID_ROW`] names for every AID key it answers with the
/// invalid-key text.
const OTHER_KEY: &str = "other";

/// AID keys `MAIN-PARA` answers with the invalid-key text.
const OTHER_KEYS: [&str; 3] = ["PF9", "PA1", "CLEAR"];

/// The query parameter a request would name an AID key in: one neither the
/// fetch nor the delete publishes.
const AID_MEMBER: &str = "aid";

/// The bearer of a request without a session: none, or one that does not
/// verify.
const NO_SESSION: [Option<&str>; 2] = [None, Some("not-a-token")];

/// The `code` of the 401 a request without a session answers.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The HTTP methods an `OpenAPI` path item can hold an operation under.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The schemas of the bodies the fetch and the delete answer.
const RESPONSE_SCHEMAS: [&str; 3] = ["UserSummary", "UserMessageResponse", "ErrorBody"];

/// The answer one `COUSR03C` row of the validation audit gives.
struct Audited {
    /// The row ID, `V-COUSR03C-nnn`.
    id: &'static str,
    /// The `Status` cell.
    status: StatusCode,
    /// The `BMS field` cell, the `fields[].field` value.
    field: String,
    /// The text the `Message text` cell quotes, without its quotes.
    message: String,
    /// The `kind=` of the `Trigger condition` cell, a `DomainError` kind name.
    kind: String,
}

/// The status, field, text and kind of validation audit row `id`.
///
/// # Panics
///
/// Panics, naming the row, when the audit has no such row or one of its cells
/// does not hold the value expected.
fn audited(doc: &Document, id: &'static str) -> Audited {
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
    let message = code_span(cell("Message text"))
        .and_then(|span| unquoted(&span.text).map(str::to_owned))
        .unwrap_or_else(|| panic!("{id} quotes no message text: {}", cell("Message text")));
    let kind = key_values(cell("Trigger condition"))
        .into_iter()
        .rfind(|pair| pair.key == "kind")
        .map_or_else(
            || panic!("{id} names no kind: {}", cell("Trigger condition")),
            |pair| pair.value,
        );
    Audited {
        id,
        status,
        field: cell("BMS field").to_owned(),
        message,
        kind,
    }
}

/// The text between the double quotes that open and close `text`, the form
/// the audits quote a COBOL literal in.
fn unquoted(text: &str) -> Option<&str> {
    text.strip_prefix('"')?.strip_suffix('"')
}

/// Asserts that `resp` is the error answer of audit row `row`: its status,
/// the code of `kind`, the catalog text `message` and the one field entry
/// `(row.field, message)`.
///
/// # Panics
///
/// Panics, naming the row, when the row's kind is not `kind` or its quoted
/// text is not `message`, and as `assert_error` does when the answer differs.
fn assert_audited(resp: &Resp, row: &Audited, kind: ErrorKind, message: &str) {
    assert_eq!(row.kind, format!("{kind:?}"), "the kind of {}", row.id);
    assert_eq!(
        row.message, message,
        "{} quotes another text than the message catalog",
        row.id
    );
    assert_error(
        resp,
        row.status,
        kind.code(),
        message,
        &[(row.field.as_str(), message)],
    );
}

/// Asserts that the `Expected result` of parity audit row `id` exists and
/// quotes each catalog text of `texts`, the texts this file asserts for it.
///
/// # Panics
///
/// Panics, naming the row, when the audit has no such row or the cell lacks
/// one of `texts`.
fn assert_parity_quotes(doc: &Document, id: &str, texts: &[&str]) {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    let cell = table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no `Expected result` cell"));
    let quoted: Vec<String> = cell_code_spans(cell)
        .iter()
        .filter_map(|span| unquoted(&span.text).map(str::to_owned))
        .collect();
    for text in texts {
        assert!(
            quoted.iter().any(|candidate| candidate == text),
            "{id} does not quote {text:?}: {cell}"
        );
    }
}

/// The single text column of the single row `sql` reads.
async fn scalar(app: &TestApp, sql: &str) -> Option<String> {
    let rows = app.stack().fetch_text_rows(sql).await;
    match rows.as_slice() {
        [row] => match row.as_slice() {
            [value] => value.clone(),
            _ => panic!("{sql} read {} columns, not one", row.len()),
        },
        _ => panic!("{sql} read {} rows, not one", rows.len()),
    }
}

/// The CU01 body of [`ADDED_USER`], a regular user.
fn added_user_body() -> Value {
    json!({
        "user_id": ADDED_USER,
        "first_name": "DELETE",
        "last_name": "CANDIDATE",
        "password": "PASSWORD",
        "usr_type": "U",
    })
}

/// The blank and unknown user IDs, of the fetch and of the delete; nothing is
/// deleted.
async fn blank_and_unknown_ids(app: &TestApp, token: &str, doc: &Document) {
    // P-COUSR03C-002: a blank ENTER fetch is the empty-field error.
    let fetched = app.get(BLANK_USER_PATH, Some(token)).await;
    let row = audited(doc, "V-COUSR03C-001");
    assert_audited(
        &fetched,
        &row,
        ErrorKind::Empty,
        messages::USER_ID_CAN_NOT_BE_EMPTY,
    );
    // P-COUSR03C-004: a blank PF5 is refused before any read.
    let deleted = app.delete(BLANK_USER_PATH, Some(token)).await;
    let row = audited(doc, "V-COUSR03C-004");
    assert_audited(
        &deleted,
        &row,
        ErrorKind::Empty,
        messages::USER_ID_CAN_NOT_BE_EMPTY,
    );
    // P-COUSR03C-003: the fetch's READ answers NOTFND.
    let fetched = app.get(UNKNOWN_USER_PATH, Some(token)).await;
    let row = audited(doc, "V-COUSR03C-002");
    assert_audited(
        &fetched,
        &row,
        ErrorKind::NotFound,
        messages::USER_ID_NOT_FOUND,
    );
    // P-COUSR03C-004, P-COUSR03C-007: the DELETE still runs after the NOTFND
    // read, fails INVREQ, and its text replaces the read's (E-46). The
    // reviewed audit row decides this answer: an unknown user's delete is the
    // reproduced 500, not the read's 404.
    let deleted = app.delete(UNKNOWN_USER_PATH, Some(token)).await;
    let row = audited(doc, "V-COUSR03C-005");
    assert_audited(
        &deleted,
        &row,
        ErrorKind::Unexpected,
        messages::UNABLE_TO_UPDATE_USER,
    );
    assert_eq!(
        scalar(app, USER_COUNT).await.as_deref(),
        Some(SEEDED_USERS),
        "a refused delete must delete nothing"
    );
}

/// A user added through CU01, fetched, deleted, then gone: the fetch answers
/// `NOTFND` and a repeat delete the audited `INVREQ` text.
async fn delete_an_added_user(app: &TestApp, token: &str, doc: &Document) {
    let (status, _, raw) = app.post(USERS_PATH, Some(token), &added_user_body()).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the CU01 add of {ADDED_USER}: {raw}"
    );

    // P-COUSR03C-002, P-COUSR03C-003: READ NORMAL shows the names and type,
    // never the password.
    let (status, fetched, raw) = app.get(ADDED_USER_PATH, Some(token)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the ENTER fetch of {ADDED_USER}: {raw}"
    );
    let expected_user = json!({
        "user_id": ADDED_USER,
        "first_name": "DELETE",
        "last_name": "CANDIDATE",
        "usr_type": "U",
    });
    assert_eq!(
        fetched, expected_user,
        "the fetched user, without a password"
    );

    // P-COUSR03C-005, P-COUSR03C-011: DELETE NORMAL; the form is emptied and
    // only the composed deleted text shows, beside the user as read before
    // the delete.
    let (status, deleted, raw) = app.delete(ADDED_USER_PATH, Some(token)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the PF5 delete of {ADDED_USER}: {raw}"
    );
    assert_eq!(
        deleted["message"].as_str(),
        Some(messages::user_deleted_message(ADDED_USER).as_str()),
        "the deleted text: {raw}"
    );
    assert_eq!(
        deleted["user"], fetched,
        "the user as read before the delete: {raw}"
    );

    assert!(
        app.stack().fetch_text_rows(ADDED_USER_ROW).await.is_empty(),
        "{ADDED_USER} must be gone from users"
    );
    assert_eq!(
        scalar(app, USER_COUNT).await.as_deref(),
        Some(SEEDED_USERS),
        "only {ADDED_USER} may be deleted"
    );

    // P-COUSR03C-003: the fetch of the deleted user answers NOTFND.
    let fetched = app.get(ADDED_USER_PATH, Some(token)).await;
    let not_found = audited(doc, "V-COUSR03C-002");
    assert_audited(
        &fetched,
        &not_found,
        ErrorKind::NotFound,
        messages::USER_ID_NOT_FOUND,
    );
    // P-COUSR03C-004, P-COUSR03C-007: a repeat delete reads NOTFND, and the
    // DELETE's INVREQ text replaces it (E-46).
    let deleted = app.delete(ADDED_USER_PATH, Some(token)).await;
    let update_failed = audited(doc, "V-COUSR03C-005");
    assert_audited(
        &deleted,
        &update_failed,
        ErrorKind::Unexpected,
        messages::UNABLE_TO_UPDATE_USER,
    );
}

#[test]
fn blank_unknown_and_deleted_users_answer_the_audited_outcomes() {
    let validation = validation_audit();
    let parity = parity_audit();
    assert_parity_quotes(
        &parity,
        "P-COUSR03C-002",
        &[messages::USER_ID_CAN_NOT_BE_EMPTY],
    );
    assert_parity_quotes(
        &parity,
        "P-COUSR03C-003",
        &[messages::USER_ID_NOT_FOUND, messages::UNABLE_TO_LOOKUP_USER],
    );
    assert_parity_quotes(
        &parity,
        "P-COUSR03C-004",
        &[messages::USER_ID_CAN_NOT_BE_EMPTY],
    );
    assert_parity_quotes(
        &parity,
        "P-COUSR03C-005",
        &[
            messages::USER_LABEL,
            messages::HAS_BEEN_DELETED,
            messages::USER_ID_NOT_FOUND,
            messages::UNABLE_TO_UPDATE_USER,
        ],
    );
    assert_parity_quotes(&parity, "P-COUSR03C-007", &[]);
    assert_parity_quotes(
        &parity,
        "P-COUSR03C-011",
        &[messages::USER_LABEL, messages::HAS_BEEN_DELETED],
    );
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        blank_and_unknown_ids(&app, &token, &validation).await;
        delete_an_added_user(&app, &token, &validation).await;
    });
}

/// Asserts that `USER0005` is still stored as `before` holds it and that the
/// fetch still reads it.
async fn assert_seeded_user_kept(app: &TestApp, token: &str, before: &[Vec<Option<String>>]) {
    let after = app.stack().fetch_text_rows(SEEDED_USER_ROW).await;
    assert_eq!(after, before, "the failed delete must keep USER0005");
    let (status, _, raw) = app.get(SEEDED_USER_PATH, Some(token)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the fetch of the kept USER0005: {raw}"
    );
}

#[test]
fn a_failed_delete_answers_unable_to_update_user_and_keeps_the_row() {
    let row = audited(&validation_audit(), "V-COUSR03C-007");
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let before = app.stack().fetch_text_rows(SEEDED_USER_ROW).await;
        assert_eq!(before.len(), 1, "USER0005 is seeded");
        app.stack().exec_sql(FAIL_USER_DELETE).await;

        // P-COUSR03C-005: the DELETE fails with another RESP; the text says
        // Update although the program deletes, reproduced as written.
        let deleted = app.delete(SEEDED_USER_PATH, Some(&token)).await;
        assert_audited(
            &deleted,
            &row,
            ErrorKind::Unexpected,
            messages::UNABLE_TO_UPDATE_USER,
        );
        assert_seeded_user_kept(&app, &token, &before).await;
    });
}

#[test]
fn a_delete_that_finds_no_row_answers_user_id_not_found() {
    let row = audited(&validation_audit(), "V-COUSR03C-006");
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let before = app.stack().fetch_text_rows(SEEDED_USER_ROW).await;
        assert_eq!(before.len(), 1, "USER0005 is seeded");
        app.stack().exec_sql(SKIP_USER_DELETE).await;

        // P-COUSR03C-005: the user was read for update and the DELETE answers
        // NOTFND.
        let deleted = app.delete(SEEDED_USER_PATH, Some(&token)).await;
        assert_audited(
            &deleted,
            &row,
            ErrorKind::NotFound,
            messages::USER_ID_NOT_FOUND,
        );
        assert_seeded_user_kept(&app, &token, &before).await;
    });
}

#[test]
fn a_failed_read_for_update_answers_the_audited_texts_and_deletes_nothing() {
    let doc = validation_audit();
    let fetch_row = audited(&doc, "V-COUSR03C-003");
    let delete_row = audited(&doc, "V-COUSR03C-005");
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let before = app.stack().fetch_text_rows(SEEDED_USER_ROW).await;
        assert_eq!(before.len(), 1, "USER0005 is seeded");
        app.stack().exec_sql(UNLOCKABLE_USERS).await;

        // P-COUSR03C-003: the fetch's READ … UPDATE fails with another RESP.
        let fetched = app.get(SEEDED_USER_PATH, Some(&token)).await;
        assert_audited(
            &fetched,
            &fetch_row,
            ErrorKind::Unexpected,
            messages::UNABLE_TO_LOOKUP_USER,
        );
        // P-COUSR03C-004, P-COUSR03C-007: the delete's read fails, the DELETE
        // still runs and fails INVREQ, and its text replaces the read's lookup
        // text (E-46).
        let deleted = app.delete(SEEDED_USER_PATH, Some(&token)).await;
        assert_audited(
            &deleted,
            &delete_row,
            ErrorKind::Unexpected,
            messages::UNABLE_TO_UPDATE_USER,
        );

        app.stack().exec_sql(RESTORE_USERS).await;
        assert_seeded_user_kept(&app, &token, &before).await;
    });
}

/// The request a function map `Endpoint` cell names: the lower-case method
/// and the path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    method: String,
    path: String,
}

/// One AID key of [`AID_ROW`]: the key its input condition names, the action
/// its expected result gives the key, and the request the function map
/// replaces the key with, `None` for a key no request replaces.
#[derive(Debug)]
struct AidKey {
    key: String,
    action: String,
    endpoint: Option<Endpoint>,
}

impl AidKey {
    /// Whether the row's action for the key is screen handling that no
    /// request replaces: an `XCTL` to another program, the screen clear, or
    /// the invalid-key error.
    fn sends_no_request(&self) -> bool {
        ["XCTL", "clear", "error"]
            .iter()
            .any(|word| self.action.starts_with(word))
    }
}

/// The parsed `docs/parity/function-map.md` (`F-` rows).
///
/// # Panics
///
/// Panics, naming the path, if the document cannot be read.
fn function_map() -> Document {
    Repo::new(fixtures::repo_root())
        .function_map()
        .unwrap_or_else(|error| panic!("{FUNCTION_MAP_PATH} cannot be read: {error}"))
}

/// The request of a function map `Endpoint` cell: `None` for `—` and
/// `— (navigation)`, else the method and path of `METHOD /path`.
fn endpoint(cell: &str) -> Option<Endpoint> {
    if cell.starts_with('—') {
        return None;
    }
    let (method, target) = cell
        .split_once(' ')
        .unwrap_or_else(|| panic!("a function map endpoint is not `METHOD /path`: {cell}"));
    let path = target.split_once('?').map_or(target, |(path, _)| path);
    Some(Endpoint {
        method: method.to_ascii_lowercase(),
        path: path.to_owned(),
    })
}

/// The cell `name` of the parity row `id`.
fn parity_cell<'d>(parity: &'d Document, id: &str, name: &str) -> &'d str {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("{PARITY_AUDIT_PATH} defines no row {id}"));
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
}

/// The request of the one function map row of the [`MAP`] screen whose `Key
/// or field` cell starts with `key`.
fn key_endpoint(map_doc: &Document, key: &str) -> Option<Endpoint> {
    let cells: Vec<&str> = map_doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| {
            table.cell(row, "BMS map") == Some(MAP)
                && table
                    .cell(row, "Key or field")
                    .and_then(|text| text.split_whitespace().next())
                    == Some(key)
        })
        .filter_map(|(table, row)| table.cell(row, "Endpoint"))
        .collect();
    let [cell] = cells.as_slice() else {
        panic!(
            "{FUNCTION_MAP_PATH}: {} rows for {key} on {MAP}",
            cells.len()
        );
    };
    endpoint(cell)
}

/// The keys of [`AID_ROW`] in the order its input condition lists them, each
/// with its action and the request the function map gives it.
fn aid_keys(parity: &Document, map_doc: &Document) -> Vec<AidKey> {
    let keys: Vec<&str> = parity_cell(parity, AID_ROW, "Input condition")
        .split(" / ")
        .map(str::trim)
        .collect();
    let actions = key_values(parity_cell(parity, AID_ROW, "Expected result"))
        .into_iter()
        .find(|pair| pair.key == "action")
        .unwrap_or_else(|| panic!("{AID_ROW} gives no action= result"))
        .value;
    let actions: Vec<&str> = actions.split(" / ").map(str::trim).collect();
    assert_eq!(keys.len(), actions.len(), "{AID_ROW}: one action per key");
    keys.iter()
        .zip(actions)
        .map(|(key, action)| AidKey {
            key: (*key).to_owned(),
            action: action.to_owned(),
            endpoint: key_endpoint(map_doc, key),
        })
        .collect()
}

/// The function map rows the expected result of the parity row `id` names,
/// each with its request.
fn named_requests(
    parity: &Document,
    map_doc: &Document,
    id: &str,
) -> Vec<(String, Option<Endpoint>)> {
    let named: Vec<(String, Option<Endpoint>)> =
        id_tokens(parity_cell(parity, id, "Expected result"))
            .into_iter()
            .filter(|(kind, _)| *kind == IdKind::Function)
            .map(|(_, function)| {
                let (table, row) = map_doc
                    .find_row(&function)
                    .unwrap_or_else(|| panic!("{FUNCTION_MAP_PATH} defines no {function} row"));
                let cell = table
                    .cell(row, "Endpoint")
                    .unwrap_or_else(|| panic!("{function} has no `Endpoint` cell"));
                (function, endpoint(cell))
            })
            .collect();
    assert!(!named.is_empty(), "{id} names no function map row");
    named
}

/// The published `OpenAPI` document.
fn spec() -> Value {
    serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"))
}

/// The HTTP methods `spec` publishes an operation under on `path`.
fn operations<'s>(spec: &'s Value, path: &str) -> BTreeSet<&'s str> {
    spec["paths"][path]
        .as_object()
        .map(|item| {
            item.keys()
                .map(String::as_str)
                .filter(|name| HTTP_METHODS.contains(name))
                .collect()
        })
        .unwrap_or_default()
}

/// The `(name, in)` pairs of the parameters `spec` publishes for `method` on
/// `path`.
fn parameters<'s>(spec: &'s Value, path: &str, method: &str) -> BTreeSet<(&'s str, &'s str)> {
    spec["paths"][path][method]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("{method} {path} publishes no parameters"))
        .iter()
        .map(|parameter| {
            let text = |name: &str| parameter[name].as_str().unwrap_or_default();
            (text("name"), text("in"))
        })
        .collect()
}

/// The property names of the component schema `name` of `spec`.
fn schema_members(spec: &Value, name: &str) -> BTreeSet<String> {
    spec["components"]["schemas"][name]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document has no {name} schema properties"))
        .keys()
        .cloned()
        .collect()
}

/// Every member name and every string of `value`, at any depth.
fn walk<'v>(value: &'v Value, names: &mut Vec<&'v str>, strings: &mut Vec<&'v str>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                names.push(name);
                walk(member, names, strings);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, names, strings);
            }
        }
        Value::String(text) => strings.push(text),
        _ => {}
    }
}

/// Whether `text` has the shape of the date or time `picture` (`MM/DD/YY`,
/// `HH:MM:SS`): a digit for each picture letter and the picture's own
/// separators.
fn matches_picture(text: &str, picture: &str) -> bool {
    text.chars().count() == picture.chars().count()
        && text.chars().zip(picture.chars()).all(|(ch, pic)| {
            if pic.is_ascii_uppercase() {
                ch.is_ascii_digit()
            } else {
                ch == pic
            }
        })
}

/// The 3270 header [`HEADER_ROW`] gives: the member names of its output
/// fields (`TITLE01O` is `title01`), the texts it shows (titles, transaction
/// and program name) and the pictures of its date and time.
struct Header {
    members: BTreeSet<String>,
    texts: BTreeSet<String>,
    pictures: Vec<String>,
}

impl Header {
    /// The header of [`HEADER_ROW`].
    fn read(parity: &Document) -> Self {
        let id = HEADER_ROW;
        let fields = parity_cell(parity, id, "Copybook field or PIC clause");
        let shown = fields.split(" of ").next().unwrap_or_default();
        let members = shown
            .split(", ")
            .map(|name| {
                let stem = name.trim().strip_suffix('O');
                stem.unwrap_or_else(|| panic!("{id}: {name} is not an output field"))
                    .to_ascii_lowercase()
            })
            .collect();
        let result = parity_cell(parity, id, "Expected result");
        let pairs = key_values(result);
        let value = |key: &str| {
            pairs.iter().find(|pair| pair.key == key).map_or_else(
                || panic!("{id} gives no {key}="),
                |pair| pair.value.as_str(),
            )
        };
        let quoted = |key: &str| {
            code_span(value(key))
                .and_then(|span| unquoted(&span.text).map(|text| text.trim().to_owned()))
                .unwrap_or_else(|| panic!("{id} quotes no {key} text"))
        };
        let first = |key: &str| {
            value(key)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        let picture = |key: &str| {
            let needle = format!("{key}=");
            let at = result
                .find(&needle)
                .unwrap_or_else(|| panic!("{id} gives no {needle}"));
            result[at + needle.len()..]
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned()
        };
        Self {
            members,
            texts: BTreeSet::from([
                quoted("title01"),
                quoted("title02"),
                first("trnname"),
                first("pgmname"),
            ]),
            pictures: vec![picture("curdate"), picture("curtime")],
        }
    }

    /// Asserts that no body of `bodies` carries a header member, a header
    /// text or a value shaped as the header date or time.
    fn assert_absent(&self, bodies: &[String]) {
        let id = HEADER_ROW;
        assert!(!bodies.is_empty(), "{id}: no response body was kept");
        for raw in bodies {
            let body: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
            let (mut names, mut strings) = (Vec::new(), Vec::new());
            walk(&body, &mut names, &mut strings);
            for name in names {
                let name = name.to_ascii_lowercase();
                assert!(!self.members.contains(&name), "{id}: member {name}: {raw}");
            }
            for text in strings.iter().map(|text| text.trim()) {
                assert!(!self.texts.contains(text), "{id}: text {text:?}: {raw}");
                for picture in &self.pictures {
                    assert!(
                        !matches_picture(text, picture),
                        "{id}: {picture} {text:?}: {raw}"
                    );
                }
            }
        }
    }
}

/// Adds [`ADDED_USER`] through CU01 and keeps the answer in `bodies`.
async fn add_user(app: &TestApp, token: &str, bodies: &mut Vec<String>) {
    let (status, _, raw) = app.post(USERS_PATH, Some(token), &added_user_body()).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "the add of {ADDED_USER}: {raw}"
    );
    bodies.push(raw);
}

/// `RETURN-TO-PREV-SCREEN` and `CLEAR-CURRENT-SCREEN` — P-COUSR03C-006,
/// P-COUSR03C-010.
///
/// PF3 and PF12 leave and PF4 clears: each function map row the two rows
/// name is the dialog's Cancel, which sends no request. The entry without a
/// COMMAREA (`EIBCALEN = 0`) is a request without a session: the fetch and
/// the delete answer 401 `Authentication required` with no field, and
/// nothing is deleted.
async fn return_and_clear_keys(
    app: &TestApp,
    parity: &Document,
    map_doc: &Document,
    bodies: &mut Vec<String>,
) {
    for row in [RETURN_ROW, CLEAR_ROW] {
        for (id, request) in named_requests(parity, map_doc, row) {
            assert_eq!(request, None, "{row}: {id} sends no request");
        }
    }
    let before = app.stack().fetch_text_rows(SEEDED_USER_ROW).await;
    assert_eq!(before.len(), 1, "USER0005 is seeded");
    for token in NO_SESSION {
        let fetched = common::send(app.client().get(app.url(SEEDED_USER_PATH)), token).await;
        let request = app.client().delete(app.url(SEEDED_USER_PATH));
        let deleted = common::send(request, token).await;
        for resp in [fetched, deleted] {
            bodies.push(resp.2.clone());
            assert_error(
                &resp,
                StatusCode::UNAUTHORIZED,
                UNAUTHENTICATED,
                messages::AUTHENTICATION_REQUIRED,
                &[],
            );
        }
    }
    let after = app.stack().fetch_text_rows(SEEDED_USER_ROW).await;
    assert_eq!(after, before, "{RETURN_ROW}: USER0005 was deleted");
    assert_eq!(
        scalar(app, USER_COUNT).await.as_deref(),
        Some(SEEDED_USERS),
        "{RETURN_ROW}: a request without a session deleted a user"
    );
}

/// `RECEIVE-USRDEL-SCREEN` — P-COUSR03C-008.
///
/// The user row the dialog is bound to replaces the received `USRIDINI`: the
/// function map rows the row names are the fetch the row action runs, and
/// the fetch and the delete each publish exactly the `id` path parameter,
/// the delete no body. The delete removes the user of its `{id}` and no
/// other, answering with the user as fetched.
async fn received_key(
    app: &TestApp,
    token: &str,
    parity: &Document,
    map_doc: &Document,
    bodies: &mut Vec<String>,
) {
    let read = parity_cell(parity, RECEIVE_ROW, "Expected result");
    let input = format!("{}I", UserDeleteField::UsrIdIn.as_str());
    assert!(read.contains(&input), "{RECEIVE_ROW} names {input}");
    let fetch = Endpoint {
        method: "get".to_owned(),
        path: ID_PATH.to_owned(),
    };
    for (id, request) in named_requests(parity, map_doc, RECEIVE_ROW) {
        assert_eq!(request.as_ref(), Some(&fetch), "{RECEIVE_ROW}: {id}");
    }
    let spec = spec();
    for method in ["get", "delete"] {
        let published = parameters(&spec, ID_PATH, method);
        assert_eq!(published, BTreeSet::from([("id", "path")]), "{method}");
    }
    let body = &spec["paths"][ID_PATH]["delete"]["requestBody"];
    assert!(body.is_null(), "{RECEIVE_ROW}: the delete takes no body");

    add_user(app, token, bodies).await;
    let fetched = app.get(ADDED_USER_PATH, Some(token)).await;
    assert_eq!(fetched.0, StatusCode::OK, "{}", fetched.2);
    let deleted = app.delete(ADDED_USER_PATH, Some(token)).await;
    assert_eq!(deleted.0, StatusCode::OK, "{}", deleted.2);
    assert_eq!(deleted.1["user"], fetched.1, "{RECEIVE_ROW}: {}", deleted.2);
    assert!(app.stack().fetch_text_rows(ADDED_USER_ROW).await.is_empty());
    assert_eq!(scalar(app, USER_COUNT).await.as_deref(), Some(SEEDED_USERS));
    bodies.extend([fetched.2, deleted.2]);
}

/// `MAIN-PARA`'s `EVALUATE EIBAID` — P-COUSR03C-001.
///
/// ENTER is the published fetch and PF5 the published delete the function
/// map names for them; PF3's and PF12's `XCTL`, PF4's clear and the
/// invalid-key error send no request. Neither operation takes key input, so
/// a fetch naming any other AID key answers as without it, a delete naming
/// one deletes as PF5 does, and none answers the invalid-key text, whose
/// audited 49 bytes fit its `X(50)`.
async fn aid_keys_map_to_requests(
    app: &TestApp,
    token: &str,
    parity: &Document,
    map_doc: &Document,
    bodies: &mut Vec<String>,
) {
    let spec = spec();
    let published = operations(&spec, ID_PATH);
    assert_eq!(published, BTreeSet::from(["delete", "get", "put"]));
    let keys = aid_keys(parity, map_doc);
    for key in &keys {
        match (&key.endpoint, key.sends_no_request()) {
            (None, true) => {}
            (Some(Endpoint { method, path }), false) => {
                assert_eq!(path, ID_PATH, "{AID_ROW} {}", key.key);
                let method = method.as_str();
                assert!(["get", "delete"].contains(&method), "{AID_ROW} {method}");
            }
            (request, _) => panic!("{AID_ROW} {}: {:?} but {request:?}", key.key, key.action),
        }
    }
    let other = keys.iter().find(|key| key.key == OTHER_KEY);
    let invalid = other.and_then(|key| code_span(&key.action));
    let invalid = invalid.unwrap_or_else(|| panic!("{AID_ROW} quotes no invalid-key text"));
    assert_eq!(
        invalid.text,
        format!("\"{}\"", messages::CCDA_MSG_INVALID_KEY)
    );
    assert!(invalid.width >= Some(messages::CCDA_MSG_INVALID_KEY.len()));

    let fetched = app.get(SEEDED_USER_PATH, Some(token)).await;
    assert_eq!(fetched.0, StatusCode::OK, "{}", fetched.2);
    let mut answers = Vec::new();
    for aid in OTHER_KEYS {
        let path = format!("{SEEDED_USER_PATH}?{AID_MEMBER}={aid}");
        let fetch = app.get(&path, Some(token)).await;
        assert_eq!(
            (fetch.0, &fetch.1),
            (fetched.0, &fetched.1),
            "{AID_ROW} {aid}"
        );
        answers.push(fetch);
    }
    add_user(app, token, bodies).await;
    let path = format!("{ADDED_USER_PATH}?{AID_MEMBER}={}", OTHER_KEYS[0]);
    let deleted = app.delete(&path, Some(token)).await;
    assert_eq!(deleted.0, StatusCode::OK, "{AID_ROW}: {}", deleted.2);
    assert_eq!(
        deleted.1["message"].as_str(),
        Some(messages::user_deleted_message(ADDED_USER).as_str()),
        "{AID_ROW}: {}",
        deleted.2
    );
    assert_eq!(scalar(app, USER_COUNT).await.as_deref(), Some(SEEDED_USERS));
    answers.extend([fetched, deleted]);
    for (_, _, raw) in answers {
        let text = messages::CCDA_MSG_INVALID_KEY.trim_end();
        assert!(
            !raw.contains(text),
            "{AID_ROW}: the invalid-key text: {raw}"
        );
        bodies.push(raw);
    }
}

/// `POPULATE-HEADER-INFO` — P-COUSR03C-009: no body `bodies` holds, and no
/// schema of the fetch and the delete, carries the 3270 header: no member of
/// its output fields, none of its texts, no value shaped as its date or
/// time.
fn assert_no_header(bodies: &[String], parity: &Document) {
    let header = Header::read(parity);
    header.assert_absent(bodies);
    let spec = spec();
    for schema in RESPONSE_SCHEMAS {
        let members = schema_members(&spec, schema);
        let shown: Vec<&String> = members.intersection(&header.members).collect();
        assert!(shown.is_empty(), "{HEADER_ROW}: {schema} carries {shown:?}");
    }
}

/// The 3270 mechanics of `COUSR03C` and their REST replacements, on one
/// seeded database: the return and clear keys with the entry without a
/// session, the received key, the AID keys of `MAIN-PARA`, and, over every
/// answer they read, the absent header. Each of the rows has passed review.
#[test]
fn the_delete_keys_and_the_header_have_their_rest_replacements() {
    let parity = parity_audit();
    for id in [AID_ROW, RETURN_ROW, RECEIVE_ROW, HEADER_ROW, CLEAR_ROW] {
        assert_reviewed(&parity, id);
    }
    let map_doc = function_map();
    let app = TestApp::start();
    let bodies = block_on(async {
        let token = app.admin_token().await;
        let mut bodies = Vec::new();
        return_and_clear_keys(&app, &parity, &map_doc, &mut bodies).await;
        received_key(&app, &token, &parity, &map_doc, &mut bodies).await;
        aid_keys_map_to_requests(&app, &token, &parity, &map_doc, &mut bodies).await;
        bodies
    });
    assert_no_header(&bodies, &parity);
}

/// Asserts that `parity` defines the `COUSR03C` row `id` and that its last
/// review-log verdict is `pass`: only reviewed rows gate a test.
fn assert_reviewed(parity: &Document, id: &str) {
    assert_eq!(
        parity_cell(parity, id, "Program"),
        "COUSR03C",
        "{id} is a COUSR03C row"
    );
    let verdict = parity
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| table.cell(row, "Row ID") == Some(id))
        .filter_map(|(table, row)| table.cell(row, "Verdict"))
        .next_back();
    assert_eq!(verdict, Some("pass"), "the last review of {id} passed it");
}
