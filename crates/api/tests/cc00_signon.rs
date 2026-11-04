//! CC00 · COSGN00C · COSGN00 — `POST /auth/login` and `POST /auth/logout`,
//! driven over HTTP through the real router (AAP 0.3.1, 0.6.1).
//!
//! - The seeded admin and user, typed in any case, sign on as the upper-cased
//!   user ID and land on the menu of their user type
//!   [app/cbl/COSGN00C.cbl:L132-136] [app/cbl/COSGN00C.cbl:L211-240]. Logout
//!   answers the `CCDA-MSG-THANK-YOU` text with or without a token and ends
//!   no server-side session, because none exists.
//! - Every `COSGN00C` row of `docs/parity/validation-audit.md` answers its
//!   audited rejection, the user read failure included, and a field longer
//!   than its `PIC X(8)` map field is refused by the length rule of
//!   `LoginRequest`.
//! - E-20: a password or user ID an admin stores in lower or mixed case never
//!   signs on, because the add keeps the case [app/cbl/COUSR01C.cbl:L154-158]
//!   while sign-on upper-cases what is typed.
//! - The 3270 mechanics and their REST replacements: ENTER is
//!   `POST /auth/login`, any other AID key sends no request and a sign-on
//!   naming one is answered as ENTER, PF3's plain text is the logout's, the
//!   first entry and PF3 read no file, and no answer carries the sign-on
//!   header or its clock.
//!
//! Each rejection takes its status, `code`, BMS field and order from the audit
//! row it proves, and that row must have passed its review. Its message is the
//! `carddemo_domain::messages` constant, checked byte for byte against the
//! text the row quotes. Docker must be running: each test function starts one
//! `postgres:16.15` container.

pub mod common;

use std::collections::BTreeSet;

use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, PARITY_AUDIT_PATH, Repo, Row, Table,
    VALIDATION_AUDIT_PATH, code_span, id_tokens, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::format::move_alnum;
use carddemo_domain::messages;
use carddemo_runtime::config::Settings;
use carddemo_testkit::{block_on, fixtures};
use chrono::{DateTime, Utc};
use common::{Resp, SeededUser, TestApp, assert_error};
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The sign-on route, ENTER on the `COSGN00` map.
const LOGIN: &str = "/auth/login";

/// The logout route, PF3 on the `COSGN00` map.
const LOGOUT: &str = "/auth/logout";

/// The CU00 list and CU01 add route.
const ADMIN_USERS: &str = "/admin/users";

/// The program whose validation rows this file proves.
const PROGRAM: &str = "COSGN00C";

/// The password every `DUSRSECJ` user row stores, upper case.
const SEED_PASSWORD: &str = "PASSWORD";

/// The SPA route of the admin menu `COADM01C`, the `XCTL` target for type `A`.
const ADMIN_LANDING: &str = "/admin";

/// The `CDEMO-USER-TYPE` value of the 88 level `CDEMO-USRTYP-ADMIN`
/// [app/cpy/COCOM01Y.cpy:L27], the one type `COSGN00C` sends to `COADM01C`.
const ADMIN_TYPE: &str = "A";

/// The SPA route of the main menu `COMEN01C`, the `XCTL` target for any other
/// user type.
const USER_LANDING: &str = "/";

/// The members of a successful sign-on body, and no others.
const LOGIN_MEMBERS: [&str; 5] = ["expires_at", "landing", "token", "user_id", "usr_type"];

/// The members of an error body, and no others.
const ERROR_MEMBERS: [&str; 3] = ["code", "fields", "message"];

/// The blank user ID, the first `PROCESS-ENTER-KEY` edit.
const BLANK_ID: &str = "V-COSGN00C-001";

/// The blank password, the second `PROCESS-ENTER-KEY` edit.
const BLANK_PASSWORD: &str = "V-COSGN00C-002";

/// No `USRSEC` record for the upper-cased user ID.
const NOT_FOUND: &str = "V-COSGN00C-003";

/// The user read failure, proved by its own test function: renaming `users`
/// leaves its stack unable to sign anyone on.
const READ_ERROR_ROW: &str = "V-COSGN00C-004";

/// The upper-cased password differs from the stored one.
const WRONG_PASSWORD: &str = "V-COSGN00C-005";

/// Every `COSGN00C` validation row: its V-ID, the parity row of the same
/// `PROCESS-ENTER-KEY` or `READ-USER-SEC-FILE` branch, and the `messages`
/// constant whose text both rows quote.
const ROWS: [(&str, &str, &str); 5] = [
    (BLANK_ID, "P-COSGN00C-005", messages::PLEASE_ENTER_USER_ID),
    (
        BLANK_PASSWORD,
        "P-COSGN00C-006",
        messages::PLEASE_ENTER_PASSWORD,
    ),
    (
        NOT_FOUND,
        "P-COSGN00C-011",
        messages::USER_NOT_FOUND_TRY_AGAIN,
    ),
    (
        READ_ERROR_ROW,
        "P-COSGN00C-012",
        messages::UNABLE_TO_VERIFY_THE_USER,
    ),
    (
        WRONG_PASSWORD,
        "P-COSGN00C-010",
        messages::WRONG_PASSWORD_TRY_AGAIN,
    ),
];

/// The seeded admin: its user ID, stored password and type, read through the
/// audit [app/jcl/DUSRSECJ.jcl:L35].
const SEEDED_ADMIN_ROW: &str = "P-CSUSR01Y-008";

/// The seeded regular user: its user ID, stored password and type, read
/// through the audit [app/jcl/DUSRSECJ.jcl:L40].
const SEEDED_USER_ROW: &str = "P-CSUSR01Y-009";

/// The parity rows a successful sign-on proves: the upper-casing, the read by
/// the upper-cased key, the signed-on outcome with its landing, and the two
/// seeded users signed on.
const SIGNON_ROWS: &[&str] = &[
    "P-COSGN00C-007",
    "P-COSGN00C-008",
    "P-COSGN00C-009",
    SEEDED_ADMIN_ROW,
    SEEDED_USER_ROW,
];

/// The parity row of PF3: the `CCDA-MSG-THANK-YOU` text, replaced by
/// `POST /auth/logout` with 200.
const LOGOUT_ROWS: &[&str] = &["P-COSGN00C-003"];

/// The parity rows of the rejections beside those [`ROWS`] pairs: the error
/// body that replaces `SEND-SIGNON-SCREEN`, with the field the cursor marks,
/// and the missing user of the seeded state.
const REJECTION_ROWS: &[&str] = &["P-COSGN00C-015", "P-CSUSR01Y-010"];

/// The parity rows of E-20: the add stores the user ID and password as typed
/// and answers its success text; sign-on upper-cases what is typed, so a
/// value stored in lower or mixed case never signs on.
const E20_ROWS: &[&str] = &[
    "P-COSGN00C-007",
    "P-COSGN00C-013",
    "P-COUSR01C-003",
    "P-COUSR01C-004",
];

/// `MAIN-PARA` with `EIBCALEN = 0`: the first entry sends the sign-on screen
/// and reads no file.
const FIRST_ENTRY_ROW: &str = "P-COSGN00C-001";

/// `MAIN-PARA` `WHEN DFHENTER`: `PROCESS-ENTER-KEY`.
const ENTER_ROW: &str = "P-COSGN00C-002";

/// `MAIN-PARA` `WHEN OTHER`: any other AID key, the invalid-key text.
const OTHER_KEY_ROW: &str = "P-COSGN00C-004";

/// Section F: the header date and time, which no API field carries.
const CLOCK_ROW: &str = "P-COSGN00C-014";

/// `SEND-PLAIN-TEXT`: PF3's thank-you text in the 80-byte `WS-MESSAGE`.
const PLAIN_TEXT_ROW: &str = "P-COSGN00C-016";

/// `POPULATE-HEADER-INFO`: the sign-on header, which no API field carries.
const HEADER_ROW: &str = "P-COSGN00C-017";

/// The parity rows of the 3270 mechanics, proved by
/// [`cosgn00c_screen_mechanics_have_their_rest_replacements`].
const MECHANICS_ROWS: &[&str] = &[
    FIRST_ENTRY_ROW,
    ENTER_ROW,
    OTHER_KEY_ROW,
    CLOCK_ROW,
    PLAIN_TEXT_ROW,
    HEADER_ROW,
];

/// The `COSGN00C` screen, as the function map's `BMS map` cell names it.
const MAP: &str = "COSGN00";

/// The members of the sign-on body: `USERIDI` and `PASSWDI` of `COSGN0A`.
const LOGIN_INPUTS: [&str; 2] = ["password", "user_id"];

/// AID keys `MAIN-PARA` answers with the invalid-key text.
const OTHER_KEYS: [&str; 3] = ["PF9", "PA1", "CLEAR"];

/// The body member a request would name an AID key in: one the sign-on does
/// not publish.
const AID_MEMBER: &str = "aid";

/// The `code` of the 405 a method no operation serves answers.
const METHOD_NOT_ALLOWED_CODE: &str = "METHOD_NOT_ALLOWED";

/// Renames `users`, the last action on a stack: no request can read a user
/// after it.
const HIDE_USERS: &str = "ALTER TABLE users RENAME TO users_unavailable";

/// The HTTP methods an `OpenAPI` path item can hold an operation under.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The schemas of the sign-on and logout bodies.
const AUTH_SCHEMAS: [&str; 4] = [
    "LoginRequest",
    "LoginResponse",
    "MessageResponse",
    "ErrorBody",
];

/// The E-20 user ID stored in lower case, as `COUSR01C` stores it as typed.
const LOWER_USER_ID: &str = "lowusr01";

/// The users the E-20 cases add through CU01, type `U`, each with the
/// password as the admin typed it.
const E20_USERS: [(&str, &str); 4] = [
    ("QAPWLOW1", "pass1234"),
    ("QAPWMIX1", "Pass1234"),
    ("QAPWUPP1", "PASS1234"),
    (LOWER_USER_ID, SEED_PASSWORD),
];

/// The rows of `users` whose ID upper-cases to the lower-case E-20 user ID:
/// the one row the add stored, with its case kept.
const LOWER_ID_ROWS_SQL: &str = "SELECT user_id FROM users WHERE upper(user_id) = 'LOWUSR01'";

/// The audit kinds, by the name the `kind=` of a trigger condition gives.
const KINDS: [(&str, ErrorKind); 7] = [
    ("Invalid", ErrorKind::Invalid),
    ("Empty", ErrorKind::Empty),
    ("NotFound", ErrorKind::NotFound),
    ("Duplicate", ErrorKind::Duplicate),
    ("Conflict", ErrorKind::Conflict),
    ("SignonFailed", ErrorKind::SignonFailed),
    ("Unexpected", ErrorKind::Unexpected),
];

/// Text of a database failure that an error body must never carry.
const INTERNAL_DETAIL: [&str; 5] = [
    "users_unavailable",
    "relation",
    "does not exist",
    "database",
    "postgres",
];

/// One sign-on that must succeed: what is typed, and the parity row of the
/// seeded user it signs on.
struct SignOn {
    typed_id: &'static str,
    typed_password: &'static str,
    seeded: &'static str,
}

/// The seeded admin and user, typed as stored, in lower case and in mixed
/// case: `COSGN00C` upper-cases both inputs before the read.
const SIGN_ONS: [SignOn; 4] = [
    SignOn {
        typed_id: "ADMIN001",
        typed_password: SEED_PASSWORD,
        seeded: SEEDED_ADMIN_ROW,
    },
    SignOn {
        typed_id: "USER0001",
        typed_password: SEED_PASSWORD,
        seeded: SEEDED_USER_ROW,
    },
    SignOn {
        typed_id: "admin001",
        typed_password: "password",
        seeded: SEEDED_ADMIN_ROW,
    },
    SignOn {
        typed_id: "User0001",
        typed_password: "PassWord",
        seeded: SEEDED_USER_ROW,
    },
];

/// The SPA route `COSGN00C` lands a user of type `usr_type` on:
/// `IF CDEMO-USRTYP-ADMIN` it sends the `COMMAREA` to `COADM01C`, else to
/// `COMEN01C` [app/cbl/COSGN00C.cbl:L230-240].
fn landing(usr_type: &str) -> &'static str {
    if usr_type == ADMIN_TYPE {
        ADMIN_LANDING
    } else {
        USER_LANDING
    }
}

/// What a sign-on of `user` must answer: its user ID, its type, and the
/// landing of that type.
fn signed_on_as(user: &SeededUser) -> (&str, &str, &str) {
    (&user.user_id, &user.usr_type, landing(&user.usr_type))
}

/// What one E-20 sign-on must answer.
enum Answer {
    /// 200 for the upper-cased user ID given, type `U`.
    SignedOn(&'static str),
    /// The rejection of the validation row given.
    Rejected(&'static str),
}

/// The E-20 sign-ons: typed user ID, typed password, answer.
const E20_SIGN_ONS: [(&str, &str, Answer); 9] = [
    ("QAPWLOW1", "pass1234", Answer::Rejected(WRONG_PASSWORD)),
    ("QAPWLOW1", "PASS1234", Answer::Rejected(WRONG_PASSWORD)),
    ("QAPWMIX1", "Pass1234", Answer::Rejected(WRONG_PASSWORD)),
    ("QAPWMIX1", "PASS1234", Answer::Rejected(WRONG_PASSWORD)),
    ("QAPWUPP1", "pass1234", Answer::SignedOn("QAPWUPP1")),
    ("QAPWUPP1", "PASS1234", Answer::SignedOn("QAPWUPP1")),
    ("qapwupp1", "Pass1234", Answer::SignedOn("QAPWUPP1")),
    (LOWER_USER_ID, SEED_PASSWORD, Answer::Rejected(NOT_FOUND)),
    ("LOWUSR01", SEED_PASSWORD, Answer::Rejected(NOT_FOUND)),
];

/// Eight spaces: a blank 8-byte map field.
const SPACES: &str = "        ";

/// Eight NUL characters: `LOW-VALUES` in an 8-byte map field.
const LOW_VALUES: &str = "\0\0\0\0\0\0\0\0";

/// The inputs each validation row must reject, keyed by V-ID: the row, the
/// case, then the user ID and password sent, `None` leaving the member out.
///
/// Blank is absent, empty, spaces or `LOW-VALUES`. The `EVALUATE` tests the
/// user ID first [app/cbl/COSGN00C.cbl:L117-130], so the inputs blank in both
/// fields belong to the user-ID row; every blank edit precedes the `USRSEC`
/// read, so an unknown user with a blank password belongs to the password row.
/// The read failure, V-COSGN00C-004, has its own test function.
const REJECTIONS: [(&str, &str, Option<&str>, Option<&str>); 18] = [
    (BLANK_ID, "user ID absent", None, Some(SEED_PASSWORD)),
    (BLANK_ID, "user ID empty", Some(""), Some(SEED_PASSWORD)),
    (
        BLANK_ID,
        "user ID of spaces",
        Some(SPACES),
        Some(SEED_PASSWORD),
    ),
    (
        BLANK_ID,
        "user ID of LOW-VALUES",
        Some(LOW_VALUES),
        Some(SEED_PASSWORD),
    ),
    (BLANK_ID, "both fields absent", None, None),
    (BLANK_ID, "both fields empty", Some(""), Some("")),
    (
        BLANK_ID,
        "both fields of spaces",
        Some(SPACES),
        Some(SPACES),
    ),
    (
        BLANK_ID,
        "spaces, then LOW-VALUES",
        Some(SPACES),
        Some(LOW_VALUES),
    ),
    (BLANK_PASSWORD, "password absent", Some("ADMIN001"), None),
    (BLANK_PASSWORD, "password empty", Some("ADMIN001"), Some("")),
    (
        BLANK_PASSWORD,
        "password of spaces",
        Some("ADMIN001"),
        Some(SPACES),
    ),
    (
        BLANK_PASSWORD,
        "password of LOW-VALUES",
        Some("USER0001"),
        Some(LOW_VALUES),
    ),
    (
        BLANK_PASSWORD,
        "unknown user, empty password",
        Some("USER0099"),
        Some(""),
    ),
    (
        NOT_FOUND,
        "unknown user USER0099",
        Some("USER0099"),
        Some(SEED_PASSWORD),
    ),
    (
        NOT_FOUND,
        "unknown user in lower case",
        Some("nosuchus"),
        Some(SEED_PASSWORD),
    ),
    (
        WRONG_PASSWORD,
        "ADMIN001, wrong password",
        Some("ADMIN001"),
        Some("WRONGPWD"),
    ),
    (
        WRONG_PASSWORD,
        "USER0001, wrong password",
        Some("USER0001"),
        Some("WRONGPWD"),
    ),
    (
        WRONG_PASSWORD,
        "user0001, 7-character password",
        Some("user0001"),
        Some("passwor"),
    ),
];

/// One validation row as the audit records it, with its `messages` constant.
struct Expected {
    id: &'static str,
    parity: &'static str,
    status: StatusCode,
    code: &'static str,
    field: String,
    order: u16,
    message: &'static str,
}

/// `ADMIN001` and `USER0001`, typed as stored and in lower and mixed case,
/// sign on as the upper-cased user and land on the menu of their type, with a
/// token whose claims carry that user and type. The user ID, stored password
/// and type each case expects are those its seeded-user row records, and
/// what is typed upper-cases to that user ID and password. Logout then
/// answers the thank-you text, and the token still opens a route afterwards.
#[test]
fn signon_lands_the_upper_cased_user_on_its_menu_and_logout_is_stateless() {
    let parity = common::parity_audit();
    assert_reviewed(&parity, SIGNON_ROWS, PARITY_AUDIT_PATH);
    assert_reviewed(&parity, LOGOUT_ROWS, PARITY_AUDIT_PATH);
    assert_eq!(
        parity_text(&parity, LOGOUT_ROWS[0], "text"),
        quoted(messages::CCDA_MSG_THANK_YOU),
        "{LOGOUT_ROWS:?}: CCDA_MSG_THANK_YOU differs from the audited text"
    );
    let cases: Vec<(&SignOn, SeededUser)> = SIGN_ONS
        .iter()
        .map(|case| (case, common::seeded_user(&parity, case.seeded)))
        .collect();
    for (case, seeded) in &cases {
        let id = case.seeded;
        assert_eq!(
            case.typed_id.to_ascii_uppercase(),
            seeded.user_id,
            "{id}: {:?} upper-cases to the seeded user ID",
            case.typed_id
        );
        assert_eq!(
            case.typed_password.to_ascii_uppercase(),
            seeded.password,
            "{id}: {:?} upper-cases to the stored password",
            case.typed_password
        );
    }
    let app = TestApp::start();
    block_on(async {
        for (case, seeded) in &cases {
            let label = format!(
                "{SIGNON_ROWS:?} {:?} / {:?}",
                case.typed_id, case.typed_password
            );
            let before = Utc::now();
            let resp = sign_on(&app, case.typed_id, case.typed_password).await;
            assert_signed_on(&resp, signed_on_as(seeded), before, &label);
        }
        logout_is_public_and_stateless(&app).await;
    });
}

/// Every `COSGN00C` validation row but the read failure answers its audited
/// status, `code`, message and BMS field for each input of its trigger, the
/// rows taken in their audited order; an over-length field is refused by the
/// request's length rule. E-20 then runs on the same stack.
#[test]
fn every_cosgn00c_row_answers_its_audited_rejection_and_stored_case_never_signs_on() {
    let validation = common::validation_audit();
    let parity = common::parity_audit();
    let rows: Vec<Expected> = ROWS
        .iter()
        .map(|&(id, _, _)| expected(&validation, &parity, id))
        .collect();
    assert_every_row_exercised();
    assert_evaluate_order(&rows);
    assert_reviewed(&parity, REJECTION_ROWS, PARITY_AUDIT_PATH);
    assert_reviewed(&parity, E20_ROWS, PARITY_AUDIT_PATH);
    let app = TestApp::start();
    block_on(async {
        for (id, case, user_id, password) in REJECTIONS {
            let resp = app.post(LOGIN, None, &login_body(user_id, password)).await;
            let label = format!("{REJECTION_ROWS:?} {case}");
            assert_rejected(&resp, row_of(&rows, id), &label);
        }
        over_length_fields_are_refused(&app).await;
        stored_case_never_signs_on(&app, &rows).await;
    });
}

/// V-COSGN00C-004: the `USRSEC` read fails with a `RESP` other than `NOTFND`
/// [app/cbl/COSGN00C.cbl:L252-256]. With `users` renamed, every sign-on read
/// fails, and the answer is the audited text on `USERID`, naming neither the
/// table nor the database error.
#[test]
fn a_failed_user_read_answers_unable_to_verify_the_user_without_internal_detail() {
    let audited = expected(
        &common::validation_audit(),
        &common::parity_audit(),
        READ_ERROR_ROW,
    );
    let app = TestApp::start();
    block_on(async {
        // The last action on this stack: no sign-on can read a user after it.
        app.stack()
            .exec_sql("ALTER TABLE users RENAME TO users_unavailable")
            .await;
        let resp = sign_on(&app, "ADMIN001", SEED_PASSWORD).await;
        assert_rejected(&resp, &audited, "the users table is unavailable");
        let (_, body, raw) = &resp;
        assert_eq!(
            members(body),
            BTreeSet::from(ERROR_MEMBERS),
            "{READ_ERROR_ROW}: the error body holds only its own members: {raw}"
        );
        let text = raw.to_lowercase();
        for detail in INTERNAL_DETAIL {
            assert!(
                !text.contains(detail),
                "{READ_ERROR_ROW}: the error body shows internal detail {detail:?}: {raw}"
            );
        }
    });
}

/// The 3270 mechanics of `COSGN00C` and their REST replacements, on one
/// seeded database: ENTER, the other AID keys and PF3's plain text, then,
/// with the user file gone, the first entry and PF3 reading no file; over
/// every answer, the absent sign-on header and clock. Each row has passed
/// review.
#[test]
fn cosgn00c_screen_mechanics_have_their_rest_replacements() {
    let validation = common::validation_audit();
    let parity = common::parity_audit();
    assert_reviewed(&parity, MECHANICS_ROWS, PARITY_AUDIT_PATH);
    let map_doc = function_map();
    let rows: Vec<Expected> = [BLANK_ID, WRONG_PASSWORD, READ_ERROR_ROW]
        .into_iter()
        .map(|id| expected(&validation, &parity, id))
        .collect();
    let admin = common::seeded_user(&parity, SEEDED_ADMIN_ROW);
    let app = TestApp::start();
    let bodies = block_on(async {
        let mut bodies = Vec::new();
        enter_key(&app, &map_doc, &rows, &admin, &mut bodies).await;
        other_keys(&app, &parity, &map_doc, &rows, &admin, &mut bodies).await;
        plain_text(&app, &parity, &mut bodies).await;
        first_entry_reads_no_file(&app, &rows, &mut bodies).await;
        bodies
    });
    assert_no_header(&bodies, &parity);
    assert_no_clock(&bodies, &parity);
}

/// The request a function map `Endpoint` cell names: the lower-case method
/// and the path.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Endpoint {
    method: String,
    path: String,
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

/// The function map rows of the [`MAP`] screen whose `COBOL action` names
/// the parity row `id`, each with its request.
fn rows_naming(map_doc: &Document, id: &str) -> Vec<(String, Option<Endpoint>)> {
    let named: Vec<(String, Option<Endpoint>)> = map_doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
        .filter(|(table, row)| {
            table.cell(row, "BMS map") == Some(MAP)
                && table.cell(row, "COBOL action").is_some_and(|action| {
                    id_tokens(action)
                        .iter()
                        .any(|(kind, named)| *kind == IdKind::Parity && named == id)
                })
        })
        .map(|(table, row)| {
            let text = |name: &str| table.cell(row, name).unwrap_or_default();
            (text("ID").to_owned(), endpoint(text("Endpoint")))
        })
        .collect();
    assert!(
        !named.is_empty(),
        "no {MAP} row of {FUNCTION_MAP_PATH} names {id}"
    );
    named
}

/// The cell `name` of the parity row `id`.
fn parity_cell<'d>(parity: &'d Document, id: &str, name: &str) -> &'d str {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("{PARITY_AUDIT_PATH} defines no row {id}"));
    cell(table, row, id, name)
}

/// The first code span of the `key=` value of the parity row `id`'s expected
/// result, with its text unquoted and its `width=`.
fn parity_span(parity: &Document, id: &str, key: &str) -> (String, Option<usize>) {
    key_values(parity_cell(parity, id, "Expected result"))
        .into_iter()
        .find(|pair| pair.key == key)
        .and_then(|pair| code_span(&pair.value))
        .and_then(|span| {
            let text = span.text.strip_prefix('"')?.strip_suffix('"')?.to_owned();
            Some((text, span.width))
        })
        .unwrap_or_else(|| panic!("{id}: the expected result quotes no `{key}=` text"))
}

/// The member names of the 3270 output fields a copybook cell lists, such as
/// `TITLE01O` or `CURDATEO`: each upper-case name ending in `O`, without it,
/// in lower case.
fn output_members(cell: &str) -> BTreeSet<String> {
    cell.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|word| {
            word.len() > 1
                && word.ends_with('O')
                && word
                    .bytes()
                    .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        })
        .map(|word| word[..word.len() - 1].to_ascii_lowercase())
        .collect()
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
        Value::Object(object) => {
            for (name, member) in object {
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

/// `body`, a JSON object, with an [`AID_MEMBER`] naming `aid`.
fn with_aid(body: Value, aid: &str) -> Value {
    let Value::Object(mut members) = body else {
        panic!("a sign-on body is a JSON object: {body}");
    };
    members.insert(AID_MEMBER.to_owned(), Value::from(aid));
    Value::Object(members)
}

/// `MAIN-PARA` `WHEN DFHENTER` — P-COSGN00C-002.
///
/// ENTER is `PROCESS-ENTER-KEY`: each function map row naming the row is the
/// published `POST /auth/login`, the only operation of its path, whose body
/// holds the two `COSGN0A` inputs. The seeded `admin` signs on and lands on
/// its menu, and an empty body is answered by the first edit of
/// `PROCESS-ENTER-KEY`.
async fn enter_key(
    app: &TestApp,
    map_doc: &Document,
    rows: &[Expected],
    admin: &SeededUser,
    bodies: &mut Vec<String>,
) {
    let login = Endpoint {
        method: "post".to_owned(),
        path: LOGIN.to_owned(),
    };
    for (id, request) in rows_naming(map_doc, ENTER_ROW) {
        assert_eq!(request.as_ref(), Some(&login), "{ENTER_ROW}: {id}");
    }
    let spec = spec();
    assert_eq!(operations(&spec, LOGIN), BTreeSet::from(["post"]));
    let inputs: BTreeSet<String> = LOGIN_INPUTS.into_iter().map(str::to_owned).collect();
    assert_eq!(schema_members(&spec, "LoginRequest"), inputs, "{ENTER_ROW}");
    let before = Utc::now();
    let signed_on = sign_on(app, "ADMIN001", SEED_PASSWORD).await;
    assert_signed_on(&signed_on, signed_on_as(admin), before, ENTER_ROW);
    let blank = app.post(LOGIN, None, &json!({})).await;
    assert_rejected(&blank, row_of(rows, BLANK_ID), ENTER_ROW);
    bodies.extend([signed_on.2, blank.2]);
}

/// `MAIN-PARA` `WHEN OTHER` — P-COSGN00C-004.
///
/// Any other AID key has no REST counterpart: each function map row naming
/// the row sends no request, the row's error is the invalid-key text, whose
/// audited 49 bytes fit its `X(50)`, and the sign-on body publishes no
/// member a key could travel in. A sign-on naming another key is answered as
/// ENTER, the seeded `admin` signed on or rejected as without it, never with
/// the invalid-key text.
async fn other_keys(
    app: &TestApp,
    parity: &Document,
    map_doc: &Document,
    rows: &[Expected],
    admin: &SeededUser,
    bodies: &mut Vec<String>,
) {
    for (id, request) in rows_naming(map_doc, OTHER_KEY_ROW) {
        assert_eq!(request, None, "{OTHER_KEY_ROW}: {id} sends no request");
    }
    let (error, width) = parity_span(parity, OTHER_KEY_ROW, "error");
    assert_eq!(error, messages::CCDA_MSG_INVALID_KEY, "{OTHER_KEY_ROW}");
    assert!(width >= Some(error.len()), "{OTHER_KEY_ROW}: X({width:?})");
    let invalid = messages::CCDA_MSG_INVALID_KEY.trim_end();
    for aid in OTHER_KEYS {
        let label = format!("{OTHER_KEY_ROW} {aid}");
        let before = Utc::now();
        let body = with_aid(login_body(Some("ADMIN001"), Some(SEED_PASSWORD)), aid);
        let signed_on = app.post(LOGIN, None, &body).await;
        assert_signed_on(&signed_on, signed_on_as(admin), before, &label);
        let body = with_aid(login_body(Some("ADMIN001"), Some("WRONGPWD")), aid);
        let wrong = app.post(LOGIN, None, &body).await;
        assert_rejected(&wrong, row_of(rows, WRONG_PASSWORD), &label);
        for raw in [signed_on.2, wrong.2] {
            assert!(
                !raw.contains(invalid),
                "{label}: the invalid-key text: {raw}"
            );
            bodies.push(raw);
        }
    }
}

/// `SEND-PLAIN-TEXT` — P-COSGN00C-016.
///
/// PF3 is the request the row's `replacement=` names, `POST /auth/logout`,
/// published with no body and no security, answering the status it names
/// with the `CCDA-MSG-THANK-YOU` constant alone: moved to the `X(80)`
/// `WS-MESSAGE` the row's copybook cell gives, it is the audited text in its
/// width, the `X(50)` constant then 30 spaces.
async fn plain_text(app: &TestApp, parity: &Document, bodies: &mut Vec<String>) {
    let id = PLAIN_TEXT_ROW;
    let result = parity_cell(parity, id, "Expected result");
    let replacement = key_values(result)
        .into_iter()
        .find(|pair| pair.key == "replacement")
        .unwrap_or_else(|| panic!("{id} gives no replacement="))
        .value;
    let words: Vec<&str> = replacement.split_whitespace().take(3).collect();
    let [method, path, status] = words.as_slice() else {
        panic!("{id}: the replacement is not `METHOD /path status`: {replacement}");
    };
    assert_eq!((*method, *path), ("POST", LOGOUT), "{id}");
    let (text, width) = parity_span(parity, id, "text");
    let widths: Vec<usize> = parity_cell(parity, id, "Copybook field or PIC clause")
        .split("X(")
        .skip(1)
        .filter_map(|rest| rest.split_once(')')?.0.parse().ok())
        .collect();
    let [message_width, constant_width] = widths.as_slice() else {
        panic!("{id}: the copybook cell gives {widths:?}, not WS-MESSAGE and the constant");
    };
    assert_eq!(
        width,
        Some(*message_width),
        "{id}: the text fills WS-MESSAGE"
    );

    let spec = spec();
    assert_eq!(operations(&spec, LOGOUT), BTreeSet::from(["post"]));
    let operation = &spec["paths"][LOGOUT]["post"];
    assert!(operation["requestBody"].is_null(), "{id}: no body");
    assert!(operation["security"].is_null(), "{id}: no token");
    let (answered, body, raw) = common::send(app.client().post(app.url(LOGOUT)), None).await;
    assert_eq!(answered.as_str(), *status, "{id}: {raw}");
    assert_eq!(members(&body), BTreeSet::from(["message"]), "{id}: {raw}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.len() <= *message_width, "{id}: {raw}");
    let shown = move_alnum(message, *message_width);
    assert_eq!(shown, move_alnum(&text, *message_width), "{id}: {raw}");
    let constant = move_alnum(messages::CCDA_MSG_THANK_YOU, *constant_width);
    assert_eq!(shown, move_alnum(&constant, *message_width), "{id}: {raw}");
    bodies.push(raw);
}

/// `MAIN-PARA` with `EIBCALEN = 0` — P-COSGN00C-001 — and PF3, with the user
/// file gone.
///
/// The first entry only sends the sign-on screen, which the SPA's `/login`
/// page replaces, its cursor on `USERID` being the page's focus that no
/// answer carries: `/auth/login` publishes no operation but the ENTER
/// `POST`, so a `GET` of it is 405. With `users` renamed, that answer and the
/// logout's thank-you text are unchanged, so neither reads the user file,
/// while a sign-on, which reads it, fails with the read-failure text.
async fn first_entry_reads_no_file(app: &TestApp, rows: &[Expected], bodies: &mut Vec<String>) {
    let spec = spec();
    let published = operations(&spec, LOGIN);
    assert_eq!(published, BTreeSet::from(["post"]), "{FIRST_ENTRY_ROW}");
    app.stack().exec_sql(HIDE_USERS).await;
    let entry = common::send(app.client().get(app.url(LOGIN)), None).await;
    assert_error(
        &entry,
        StatusCode::METHOD_NOT_ALLOWED,
        METHOD_NOT_ALLOWED_CODE,
        messages::METHOD_NOT_ALLOWED,
        &[],
    );
    let logout = common::send(app.client().post(app.url(LOGOUT)), None).await;
    assert_eq!(logout.0, StatusCode::OK, "{PLAIN_TEXT_ROW}: {}", logout.2);
    let thank_you = json!({ "message": messages::CCDA_MSG_THANK_YOU });
    assert_eq!(logout.1, thank_you, "{PLAIN_TEXT_ROW}: no file access");
    let read = sign_on(app, "ADMIN001", SEED_PASSWORD).await;
    assert_rejected(&read, row_of(rows, READ_ERROR_ROW), FIRST_ENTRY_ROW);
    bodies.extend([entry.2, logout.2, read.2]);
}

/// `POPULATE-HEADER-INFO` — P-COSGN00C-017: no body `bodies` holds, and no
/// schema of the sign-on and logout, carries the sign-on header: no member
/// of its output fields, applid and sysid included, and none of its texts
/// (titles, transaction and program name).
fn assert_no_header(bodies: &[String], parity: &Document) {
    let id = HEADER_ROW;
    let header = output_members(parity_cell(parity, id, "Copybook field or PIC clause"));
    assert!(
        header.contains("applid") && header.contains("sysid"),
        "{id}: {header:?}"
    );
    let result = parity_cell(parity, id, "Expected result");
    let first = |key: &str| {
        key_values(result)
            .into_iter()
            .find(|pair| pair.key == key)
            .and_then(|pair| pair.value.split_whitespace().next().map(str::to_owned))
            .unwrap_or_else(|| panic!("{id} gives no {key}="))
    };
    let texts = BTreeSet::from([
        parity_span(parity, id, "title01").0.trim().to_owned(),
        parity_span(parity, id, "title02").0.trim().to_owned(),
        first("trnname"),
        first("pgmname"),
    ]);
    for raw in bodies {
        let body: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        let (mut names, mut strings) = (Vec::new(), Vec::new());
        walk(&body, &mut names, &mut strings);
        for name in names.iter().map(|name| name.to_ascii_lowercase()) {
            assert!(!header.contains(&name), "{id}: member {name}: {raw}");
        }
        for text in strings.iter().map(|text| text.trim()) {
            assert!(!texts.contains(text), "{id}: text {text:?}: {raw}");
        }
    }
    let spec = spec();
    for schema in AUTH_SCHEMAS {
        let members = schema_members(&spec, schema);
        let shown: Vec<&String> = members.intersection(&header).collect();
        assert!(shown.is_empty(), "{id}: {schema} carries {shown:?}");
    }
}

/// One element of the `format_regex` patterns of [`CLOCK_ROW`]: `[0-9]{n}`
/// or a literal character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Atom {
    Digits(usize),
    Char(char),
}

/// The atoms of `pattern`, anchored `^…$`, built only from `[0-9]{n}` and
/// literal characters.
///
/// # Panics
///
/// Panics on any other regular-expression syntax, so a row the matcher
/// cannot read fails instead of matching nothing.
fn atoms(pattern: &str) -> Vec<Atom> {
    let body = pattern
        .strip_prefix('^')
        .and_then(|rest| rest.strip_suffix('$'))
        .unwrap_or_else(|| panic!("{CLOCK_ROW}: {pattern} is not anchored ^…$"));
    let mut atoms = Vec::new();
    let mut rest = body;
    while let Some(ch) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix("[0-9]{") {
            let (count, tail) = after
                .split_once('}')
                .unwrap_or_else(|| panic!("{CLOCK_ROW}: {pattern} leaves a count open"));
            let count = count
                .parse()
                .unwrap_or_else(|error| panic!("{CLOCK_ROW}: {pattern}: {error}"));
            atoms.push(Atom::Digits(count));
            rest = tail;
        } else {
            assert!(
                !"[](){}*+?.\\|^$".contains(ch),
                "{CLOCK_ROW}: {pattern} uses syntax this matcher does not read"
            );
            atoms.push(Atom::Char(ch));
            rest = &rest[ch.len_utf8()..];
        }
    }
    atoms
}

/// Whether the whole of `text` matches `atoms`.
fn matches(atoms: &[Atom], text: &str) -> bool {
    let mut chars = text.chars();
    let all = atoms.iter().all(|atom| match *atom {
        Atom::Digits(count) => {
            (0..count).all(|_| chars.next().is_some_and(|ch| ch.is_ascii_digit()))
        }
        Atom::Char(expected) => chars.next() == Some(expected),
    });
    all && chars.next().is_none()
}

/// The header clock — P-COSGN00C-014.
///
/// The row's two `format_regex` patterns, read from the audit, match a date
/// and a time formatted as `FUNCTION CURRENT-DATE` gives them to
/// `CURDATEO` and `CURTIMEO`, so the scan is not vacuous; then no body
/// `bodies` holds carries a value either matches or a member of the two
/// fields, and no schema of the sign-on and logout publishes one.
fn assert_no_clock(bodies: &[String], parity: &Document) {
    let id = CLOCK_ROW;
    let (regex, _) = parity_span(parity, id, "format_regex");
    let patterns: Vec<Vec<Atom>> = regex.split(" and ").map(atoms).collect();
    let now = Utc::now();
    let samples = [
        now.format("%m/%d/%y").to_string(),
        now.format("%H:%M:%S").to_string(),
    ];
    assert_eq!(patterns.len(), samples.len(), "{id}: a date and a time");
    for (pattern, sample) in patterns.iter().zip(&samples) {
        assert!(matches(pattern, sample), "{id}: {sample} is a header value");
    }
    let fields = output_members(parity_cell(parity, id, "Copybook field or PIC clause"));
    assert_eq!(
        fields,
        BTreeSet::from(["curdate", "curtime"].map(str::to_owned))
    );
    for raw in bodies {
        let body: Value = serde_json::from_str(raw).unwrap_or(Value::Null);
        let (mut names, mut strings) = (Vec::new(), Vec::new());
        walk(&body, &mut names, &mut strings);
        for name in names.iter().map(|name| name.to_ascii_lowercase()) {
            assert!(!fields.contains(&name), "{id}: member {name}: {raw}");
        }
        for text in strings {
            let shaped = patterns.iter().any(|pattern| matches(pattern, text.trim()));
            assert!(!shaped, "{id}: {text:?} is a header clock value: {raw}");
        }
    }
    let spec = spec();
    for schema in AUTH_SCHEMAS {
        let members = schema_members(&spec, schema);
        let shown: Vec<&String> = members.intersection(&fields).collect();
        assert!(shown.is_empty(), "{id}: {schema} carries {shown:?}");
    }
}

/// `POST /auth/logout` without a token, with each seeded user's token and
/// with a bearer that does not verify: 200 with exactly the
/// `CCDA-MSG-THANK-YOU` text, untrimmed. No session ends, so the admin token
/// still opens `/admin/users` afterwards (AAP 0.6.1).
async fn logout_is_public_and_stateless(app: &TestApp) {
    let admin = app.admin_token().await;
    let user = app.user_token().await;
    let expected = json!({ "message": messages::CCDA_MSG_THANK_YOU });
    let tokens = [
        ("no token", None),
        ("the ADMIN001 token", Some(admin.as_str())),
        ("the USER0001 token", Some(user.as_str())),
        ("a bearer that does not verify", Some("not-a-token")),
    ];
    for (case, token) in tokens {
        let (status, body, raw) = common::send(app.client().post(app.url(LOGOUT)), token).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "{LOGOUT_ROWS:?} logout with {case}: {raw}"
        );
        assert_eq!(
            body, expected,
            "{LOGOUT_ROWS:?} logout with {case}: the untrimmed thank-you text: {raw}"
        );
    }
    let (status, _, raw) = app.get(ADMIN_USERS, Some(&admin)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "{LOGOUT_ROWS:?} the ADMIN001 token after logout: {raw}"
    );
}

/// A field longer than its `PIC X(8)` map field [app/cpy-bms/COSGN00.CPY:L72]
/// [app/cpy-bms/COSGN00.CPY:L78] is refused by the `max_length = 8` rule of
/// `LoginRequest` as `INVALID`, naming `USERID` then `PASSWD`, before any
/// read. A 3270 field never holds more, so no `COSGN00C` row or escalation
/// records the case, and its message is not asserted.
async fn over_length_fields_are_refused(app: &TestApp) {
    let cases: [(&str, Value, &[&str]); 3] = [
        (
            "a 9-character user ID",
            json!({ "user_id": "ADMIN0011", "password": SEED_PASSWORD }),
            &["USERID"],
        ),
        (
            "a 9-character password",
            json!({ "user_id": "ADMIN001", "password": "PASSWORD1" }),
            &["PASSWD"],
        ),
        (
            "both fields too long",
            json!({ "user_id": "ADMIN0011", "password": "PASSWORD1" }),
            &["USERID", "PASSWD"],
        ),
    ];
    for (case, body, fields) in cases {
        let (status, json, raw) = app.post(LOGIN, None, &body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}: {raw}");
        assert_eq!(json["code"], ErrorKind::Invalid.code(), "{case}: {raw}");
        let named: Vec<&str> = json["fields"].as_array().map_or_else(Vec::new, |entries| {
            entries
                .iter()
                .filter_map(|entry| entry["field"].as_str())
                .collect()
        });
        assert_eq!(named, fields, "{case}: the fields named: {raw}");
    }
}

/// E-20: the users of [`E20_USERS`] are added as typed, the lower-case user ID
/// is stored with its case kept, and each sign-on of [`E20_SIGN_ONS`] answers
/// as listed: a password stored in lower or mixed case never matches the
/// upper-cased entry, and a lower-case user ID is never found.
async fn stored_case_never_signs_on(app: &TestApp, rows: &[Expected]) {
    let admin = app.admin_token().await;
    for (user_id, password) in E20_USERS {
        add_user(app, &admin, user_id, password).await;
    }
    let stored = app.stack().fetch_text_rows(LOWER_ID_ROWS_SQL).await;
    assert_eq!(
        stored,
        vec![vec![Some(LOWER_USER_ID.to_owned())]],
        "E-20 {E20_ROWS:?}: the user ID is stored as typed"
    );
    for (typed_id, typed_password, answer) in E20_SIGN_ONS {
        let label = format!("E-20 {E20_ROWS:?}: {typed_id:?} / {typed_password:?}");
        let before = Utc::now();
        let resp = sign_on(app, typed_id, typed_password).await;
        match answer {
            Answer::SignedOn(user_id) => {
                assert_signed_on(&resp, (user_id, "U", USER_LANDING), before, &label);
            }
            Answer::Rejected(id) => assert_rejected(&resp, row_of(rows, id), &label),
        }
    }
}

/// Adds the type `U` user `user_id` with `password` through `POST /admin/users`
/// and asserts the 201 answer with the `COUSR01C` success text.
async fn add_user(app: &TestApp, admin: &str, user_id: &str, password: &str) {
    let body = json!({
        "first_name": "CASE",
        "last_name": "CHECK",
        "user_id": user_id,
        "password": password,
        "usr_type": "U",
    });
    let (status, json, raw) = app.post(ADMIN_USERS, Some(admin), &body).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "{E20_ROWS:?} the add of {user_id:?}: {raw}"
    );
    assert_eq!(
        json["message"],
        messages::user_added_message(user_id),
        "{E20_ROWS:?} the add of {user_id:?}: {raw}"
    );
    assert_eq!(
        json["user"]["user_id"], user_id,
        "{E20_ROWS:?} the add of {user_id:?} keeps the case: {raw}"
    );
}

/// The `POST /auth/login` body holding `user_id` and `password`, each left
/// out when `None`.
fn login_body(user_id: Option<&str>, password: Option<&str>) -> Value {
    let mut body = serde_json::Map::new();
    if let Some(user_id) = user_id {
        body.insert("user_id".to_owned(), Value::from(user_id));
    }
    if let Some(password) = password {
        body.insert("password".to_owned(), Value::from(password));
    }
    Value::Object(body)
}

/// Sends `POST /auth/login` for `user_id` and `password` as typed.
async fn sign_on(app: &TestApp, user_id: &str, password: &str) -> Resp {
    app.post(LOGIN, None, &login_body(Some(user_id), Some(password)))
        .await
}

/// Asserts that `resp` signed on `(user_id, usr_type, landing)`: 200 with
/// exactly the members of [`LOGIN_MEMBERS`], a non-empty token whose claims
/// carry the user and type, and an RFC 3339 `expires_at` later than `before`
/// that is the token's `exp`.
fn assert_signed_on(
    resp: &Resp,
    (user_id, usr_type, landing): (&str, &str, &str),
    before: DateTime<Utc>,
    case: &str,
) {
    let (status, body, raw) = resp;
    assert_eq!(*status, StatusCode::OK, "{case}: {raw}");
    assert_eq!(
        members(body),
        BTreeSet::from(LOGIN_MEMBERS),
        "{case}: the sign-on body members: {raw}"
    );
    assert_eq!(body["user_id"], user_id, "{case}: CDEMO-USER-ID: {raw}");
    assert_eq!(body["usr_type"], usr_type, "{case}: CDEMO-USER-TYPE: {raw}");
    assert_eq!(body["landing"], landing, "{case}: the XCTL target: {raw}");
    let Some(token) = body["token"].as_str().filter(|token| !token.is_empty()) else {
        panic!("{case}: the sign-on issued no token: {raw}");
    };
    let Some(expires_at) = body["expires_at"]
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
    else {
        panic!("{case}: `expires_at` is not an RFC 3339 instant: {raw}");
    };
    assert!(
        expires_at > before,
        "{case}: the token expires at {expires_at}, before the sign-on: {raw}"
    );
    let claims = token_claims(token, case);
    assert_eq!(claims["sub"], user_id, "{case}: the token subject");
    assert_eq!(claims["usr_type"], usr_type, "{case}: the token user type");
    assert_eq!(
        claims["exp"].as_i64(),
        Some(expires_at.timestamp()),
        "{case}: `expires_at` is the token's `exp`"
    );
}

/// The claims of `token`, verified HS256 with the `CARDDEMO_JWT_SECRET` the
/// test app runs with.
fn token_claims(token: &str, case: &str) -> Value {
    let key = DecodingKey::from_secret(Settings::default().jwt_secret.as_bytes());
    jsonwebtoken::decode::<Value>(token, &key, &Validation::new(Algorithm::HS256))
        .unwrap_or_else(|error| panic!("{case}: the issued token does not verify: {error}"))
        .claims
}

/// The member names of the JSON object `body`, empty for any other value.
fn members(body: &Value) -> BTreeSet<&str> {
    body.as_object().map_or_else(BTreeSet::new, |object| {
        object.keys().map(String::as_str).collect()
    })
}

/// Asserts that `resp` is the rejection the validation row `audited` records:
/// its status and `code`, its message, and that message on its BMS field
/// alone.
fn assert_rejected(resp: &Resp, audited: &Expected, case: &str) {
    let (status, body, raw) = resp;
    let (id, parity) = (audited.id, audited.parity);
    assert_eq!(*status, audited.status, "{id} {parity} {case}: {raw}");
    assert_eq!(body["code"], audited.code, "{id} {parity} {case}: {raw}");
    assert_eq!(
        body["message"], audited.message,
        "{id} {parity} {case}: {raw}"
    );
    assert_error(
        resp,
        audited.status,
        audited.code,
        audited.message,
        &[(audited.field.as_str(), audited.message)],
    );
}

/// The row `id` of [`ROWS`] as the validation audit records it.
///
/// Both the validation row and its parity row must have passed their review,
/// and both must quote the row's `messages` constant byte for byte.
fn expected(validation: &Document, parity: &Document, id: &'static str) -> Expected {
    let Some(&(_, parity_id, message)) = ROWS.iter().find(|(row, _, _)| *row == id) else {
        panic!("{id} is not a {PROGRAM} row of ROWS");
    };
    assert_reviewed(validation, &[id], VALIDATION_AUDIT_PATH);
    assert_reviewed(parity, &[parity_id], PARITY_AUDIT_PATH);
    let Some((table, row)) = validation.find_row(id) else {
        panic!("{VALIDATION_AUDIT_PATH} defines no row {id}");
    };
    assert_eq!(
        cell(table, row, id, "Program"),
        PROGRAM,
        "{id}: the program"
    );
    let Some(text) = code_span(cell(table, row, id, "Message text")) else {
        panic!("{id}: the message text is not a code span");
    };
    assert_eq!(
        text.text,
        quoted(message),
        "{id}: the messages constant differs from the audited text"
    );
    assert_eq!(
        parity_text(parity, parity_id, "error"),
        quoted(message),
        "{parity_id}: the messages constant differs from the audited error"
    );
    let Some(status) = cell(table, row, id, "Status")
        .parse::<u16>()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
    else {
        panic!("{id}: the status is not an HTTP status code");
    };
    let Ok(order) = cell(table, row, id, "Order").parse::<u16>() else {
        panic!("{id}: the order is not a number");
    };
    Expected {
        id,
        parity: parity_id,
        status,
        code: kind_code(cell(table, row, id, "Trigger condition"), id),
        field: cell(table, row, id, "BMS field").to_owned(),
        order,
        message,
    }
}

/// The cell `name` of the row `id`.
fn cell<'a>(table: &'a Table, row: &'a Row, id: &str, name: &str) -> &'a str {
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id}: the row has no `{name}` cell"))
}

/// The error `code` of the kind a trigger condition names in its `kind=`.
fn kind_code(trigger: &str, id: &str) -> &'static str {
    let Some(named) = key_values(trigger)
        .into_iter()
        .find(|pair| pair.key == "kind")
    else {
        panic!("{id}: the trigger condition names no kind");
    };
    let Some(&(_, kind)) = KINDS.iter().find(|(name, _)| *name == named.value) else {
        panic!("{id}: {:?} is not an error kind", named.value);
    };
    kind.code()
}

/// The row `id` among `rows`.
fn row_of<'a>(rows: &'a [Expected], id: &str) -> &'a Expected {
    rows.iter()
        .find(|row| row.id == id)
        .unwrap_or_else(|| panic!("{id} is not a {PROGRAM} row of ROWS"))
}

/// [`ROWS`] lists exactly the `COSGN00C` rows the validation audit defines,
/// and [`REJECTIONS`] with the read-failure test exercises every one of them.
fn assert_every_row_exercised() {
    let defined: BTreeSet<String> = common::validation_ids(PROGRAM).into_iter().collect();
    let listed: BTreeSet<String> = ROWS.iter().map(|(id, _, _)| (*id).to_owned()).collect();
    assert_eq!(
        listed, defined,
        "ROWS must list every {PROGRAM} row of {VALIDATION_AUDIT_PATH}"
    );
    let exercised: BTreeSet<String> = REJECTIONS
        .iter()
        .map(|(id, _, _, _)| *id)
        .chain([READ_ERROR_ROW])
        .map(str::to_owned)
        .collect();
    assert_eq!(
        exercised, defined,
        "every {PROGRAM} row of {VALIDATION_AUDIT_PATH} needs an input"
    );
}

/// The audited `Order` the inputs of [`REJECTIONS`] rely on: the blank user
/// ID, then the blank password, then the read, then the password compare
/// [app/cbl/COSGN00C.cbl:L117-130] [app/cbl/COSGN00C.cbl:L211-256].
fn assert_evaluate_order(rows: &[Expected]) {
    let sequence = [BLANK_ID, BLANK_PASSWORD, NOT_FOUND, WRONG_PASSWORD];
    for pair in sequence.windows(2) {
        let (first, then) = (row_of(rows, pair[0]), row_of(rows, pair[1]));
        assert!(
            first.order < then.order,
            "{} (order {}) must come before {} (order {})",
            first.id,
            first.order,
            then.id,
            then.order
        );
    }
}

/// Asserts that `doc`, read from `path`, defines each of `ids` and that the
/// last review-log verdict on each is `pass`: assertions rest on reviewed
/// rows only.
fn assert_reviewed(doc: &Document, ids: &[&str], path: &str) {
    for &id in ids {
        assert!(doc.find_row(id).is_some(), "{path} defines no row {id}");
        let verdict = doc
            .tables()
            .iter()
            .rev()
            .filter(|table| table.is_review_log())
            .find_map(|table| {
                table
                    .rows
                    .iter()
                    .rev()
                    .find(|row| table.cell(row, "Row ID") == Some(id))
                    .and_then(|row| table.cell(row, "Verdict"))
            });
        assert_eq!(
            verdict,
            Some("pass"),
            "{path}: row {id} has not passed its review"
        );
    }
}

/// The quoted text that the `key=` of the parity row `id` gives in its
/// expected result, for example the `error=` of a rejection.
fn parity_text(parity: &Document, id: &str, key: &str) -> String {
    let Some((table, row)) = parity.find_row(id) else {
        panic!("{PARITY_AUDIT_PATH} defines no row {id}");
    };
    let result = cell(table, row, id, "Expected result");
    let Some(span) = key_values(result)
        .into_iter()
        .find(|pair| pair.key == key)
        .and_then(|pair| code_span(&pair.value))
    else {
        panic!("{id}: the expected result quotes no `{key}=` text");
    };
    span.text
}

/// `text` as an audit code span quotes it: in double quotes, with `\"` for a
/// double quote and `\\` for a backslash.
fn quoted(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}
