//! CU02, `COUSR02C`: the fetch and the update of one user through the HTTP
//! layer, with the E-21 password rules.
//!
//! `GET /admin/users/{id}` is the ENTER fetch, `PROCESS-ENTER-KEY` with
//! `READ-USER-SEC-FILE` [app/cbl/COUSR02C.cbl:L143-176]
//! [app/cbl/COUSR02C.cbl:L320-353], and `PUT /admin/users/{id}` is PF5,
//! `UPDATE-USER-INFO` with `UPDATE-USER-SEC-FILE`
//! [app/cbl/COUSR02C.cbl:L177-245] [app/cbl/COUSR02C.cbl:L358-390], over the
//! `CSUSR01Y` users the seed loads from the `DUSRSECJ` rows
//! [app/jcl/DUSRSECJ.jcl:L35-44]. The tests prove:
//!
//! - the fetch: the user's ID, names and type, never its password or hash
//!   (P-COUSR02C-002, P-COUSR02C-007), and the blank and unknown IDs;
//! - the PF5 empty edits, one `EVALUATE` in screen order whose first true
//!   `WHEN` wins (P-COUSR02C-004), and the unknown user whose failed read the
//!   rewrite text replaces (E-46, P-COUSR02C-009);
//! - the five E-21 password cases: an absent password is the current one,
//!   never re-hashed; an empty one is rejected; a present one is compared by
//!   argon2 verification and re-hashed only when it differs
//!   (P-COUSR02C-005, P-COUSR02C-006, P-COUSR02C-013), after which sign-on
//!   takes the new password and refuses the old one (E-20);
//! - the read and rewrite failures, injected through SQL on the test
//!   database: a stored row no `PIC X(20)` field can hold, and a trigger that
//!   skips or fails the `UPDATE` (P-COUSR02C-003, P-COUSR02C-006);
//! - the 3270 mechanics and their REST replacements: ENTER is the fetch, PF3
//!   and PF5 the update, PF4, PF12 and the invalid-key answer send no
//!   request, and a request naming another AID key is answered as without
//!   it; a request without a session is 401 and writes nothing; the route
//!   parameter and the body replace the received map; no answer carries the
//!   3270 header (P-COUSR02C-001, P-COUSR02C-008, P-COUSR02C-010 to
//!   P-COUSR02C-012).
//!
//! Every expected status, error kind, BMS field and order is read from the
//! `COUSR02C` rows of `docs/parity/validation-audit.md`, and every text is a
//! `carddemo_domain::messages` item, checked equal to the row's quoted text.
//! Only rows with a `pass` review verdict are used, and
//! [`every_cousr02c_validation_row_is_exercised`] fails when the audit holds a
//! `COUSR02C` row no test here sends. The seeded users' IDs, names, types and
//! stored password are read through `carddemo_audit` from the reviewed
//! `CSUSR01Y` rows of `docs/parity/parity-oracle-audit.md`: the record of
//! `USER0001`, and the `DUSRSECJ` records the users' count row cites, decoded
//! with the section A layout.
//!
//! Docker must be running: each test that starts a [`TestApp`] starts one
//! `postgres:16.15` container. The two audit checks need none.

pub mod common;

use std::collections::BTreeSet;

use carddemo_api::dto::admin_users::UserUpdateField;
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, PARITY_AUDIT_PATH, Repo, code_span, id_tokens, key_values,
};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::{block_on, fixtures};
use common::{
    Resp, SeededUser, TestApp, assert_error, parity_audit, seeded_user, seeded_users,
    validation_audit, validation_ids,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The program whose validation rows this file proves.
const PROGRAM: &str = "COUSR02C";

/// `GET /admin/users/{id}` with a blank ID — P-COUSR02C-002.
const FETCH_EMPTY_ID: &str = "V-COUSR02C-001";

/// `GET /admin/users/{id}` of an unknown user, `READ` `NOTFND` —
/// P-COUSR02C-003.
const FETCH_NOT_FOUND: &str = "V-COUSR02C-002";

/// `GET /admin/users/{id}` whose `READ` fails otherwise — P-COUSR02C-003.
const FETCH_READ_FAILED: &str = "V-COUSR02C-003";

/// `PUT /admin/users/{id}` with a blank ID — P-COUSR02C-004.
const UPDATE_EMPTY_ID: &str = "V-COUSR02C-004";

/// `PUT` with a blank first name — P-COUSR02C-004.
const UPDATE_EMPTY_FIRST_NAME: &str = "V-COUSR02C-005";

/// `PUT` with a blank last name — P-COUSR02C-004.
const UPDATE_EMPTY_LAST_NAME: &str = "V-COUSR02C-006";

/// `PUT` with a present, blank password — P-COUSR02C-004.
const UPDATE_EMPTY_PASSWORD: &str = "V-COUSR02C-007";

/// `PUT` with a blank user type — P-COUSR02C-004.
const UPDATE_EMPTY_USER_TYPE: &str = "V-COUSR02C-008";

/// `PUT` of an unknown user: the read text replaced by the rewrite's (E-46) —
/// P-COUSR02C-009.
const UPDATE_UNKNOWN_USER: &str = "V-COUSR02C-009";

/// `PUT` that changes no field: message only, nothing written —
/// P-COUSR02C-005.
const UPDATE_NOT_MODIFIED: &str = "V-COUSR02C-010";

/// The `REWRITE` answers `NOTFND` — P-COUSR02C-006.
const REWRITE_NOT_FOUND: &str = "V-COUSR02C-011";

/// The `REWRITE` fails otherwise — P-COUSR02C-006.
const REWRITE_FAILED: &str = "V-COUSR02C-012";

/// Every `COUSR02C` row of the validation audit, in audit order, with the
/// catalog text its answer carries.
const COUSR02C_ROWS: [(&str, &str); 12] = [
    (FETCH_EMPTY_ID, messages::USER_ID_CAN_NOT_BE_EMPTY),
    (FETCH_NOT_FOUND, messages::USER_ID_NOT_FOUND),
    (FETCH_READ_FAILED, messages::UNABLE_TO_LOOKUP_USER),
    (UPDATE_EMPTY_ID, messages::USER_ID_CAN_NOT_BE_EMPTY),
    (
        UPDATE_EMPTY_FIRST_NAME,
        messages::FIRST_NAME_CAN_NOT_BE_EMPTY,
    ),
    (UPDATE_EMPTY_LAST_NAME, messages::LAST_NAME_CAN_NOT_BE_EMPTY),
    (UPDATE_EMPTY_PASSWORD, messages::PASSWORD_CAN_NOT_BE_EMPTY),
    (UPDATE_EMPTY_USER_TYPE, messages::USER_TYPE_CAN_NOT_BE_EMPTY),
    (UPDATE_UNKNOWN_USER, messages::UNABLE_TO_UPDATE_USER),
    (UPDATE_NOT_MODIFIED, messages::PLEASE_MODIFY_TO_UPDATE),
    (REWRITE_NOT_FOUND, messages::USER_ID_NOT_FOUND),
    (REWRITE_FAILED, messages::UNABLE_TO_UPDATE_USER),
];

/// The sign-on row the replaced password meets: 'Wrong Password. Try
/// again ...' (E-20).
const SIGNON_WRONG_PASSWORD: &str = "V-COSGN00C-005";

/// The `COUSR02C` parity rows of the fetch and the update; the 3270 mechanics
/// rows are [`SCREEN_ROWS`].
const PARITY_ROWS: [&str; 8] = [
    "P-COUSR02C-002",
    "P-COUSR02C-003",
    "P-COUSR02C-004",
    "P-COUSR02C-005",
    "P-COUSR02C-006",
    "P-COUSR02C-007",
    "P-COUSR02C-009",
    "P-COUSR02C-013",
];

/// `MAIN-PARA`'s `EVALUATE EIBAID`: ENTER, PF3, PF4, PF5, PF12 and any other
/// key, each with its action.
const AID_ROW: &str = "P-COUSR02C-001";

/// `RETURN-TO-PREV-SCREEN`: PF3 after the update, PF12, and the entry
/// without a COMMAREA that the bearer token replaces.
const RETURN_ROW: &str = "P-COUSR02C-008";

/// `RECEIVE-USRUPD-SCREEN`: the route parameter and the body replace the
/// received map.
const RECEIVE_ROW: &str = "P-COUSR02C-010";

/// `POPULATE-HEADER-INFO`: the 3270 header no answer carries.
const HEADER_ROW: &str = "P-COUSR02C-011";

/// `CLEAR-CURRENT-SCREEN`: PF4, the form's Reset control.
const CLEAR_ROW: &str = "P-COUSR02C-012";

/// The `COUSR02C` parity rows of the 3270 mechanics, proved by
/// [`the_screen_keys_and_the_header_have_their_rest_replacements`].
const SCREEN_ROWS: [&str; 5] = [AID_ROW, RETURN_ROW, RECEIVE_ROW, HEADER_ROW, CLEAR_ROW];

/// The `COUSR02C` screen, as the function map's `BMS map` cell names it.
const MAP: &str = "COUSR02";

/// The route of the fetch, the update and the delete, as published.
const ID_PATH: &str = "/admin/users/{id}";

/// The `COUSR2A` inputs, in screen order: `USRIDINI` is the `{id}` path
/// segment, the others are body members.
const UPDATE_FIELDS: [UserUpdateField; 5] = [
    UserUpdateField::UsrIdIn,
    UserUpdateField::FName,
    UserUpdateField::LName,
    UserUpdateField::Passwd,
    UserUpdateField::UsrType,
];

/// The key [`AID_ROW`] names for every AID key it answers with the
/// invalid-key text.
const OTHER_KEY: &str = "other";

/// AID keys `MAIN-PARA` answers with the invalid-key text.
const OTHER_KEYS: [&str; 3] = ["PF9", "PA1", "CLEAR"];

/// The query parameter or body member a request would name an AID key in:
/// one neither operation publishes.
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

/// The schemas of the bodies the fetch and the update answer.
const RESPONSE_SCHEMAS: [&str; 3] = ["UserSummary", "UserMessageResponse", "ErrorBody"];

/// The PF5 edits of the body members, in screen order, each with its row.
const MEMBER_EDITS: [(&str, &str); 4] = [
    ("first_name", UPDATE_EMPTY_FIRST_NAME),
    ("last_name", UPDATE_EMPTY_LAST_NAME),
    ("password", UPDATE_EMPTY_PASSWORD),
    ("usr_type", UPDATE_EMPTY_USER_TYPE),
];

/// The blank forms of an entered field: empty, a space and `LOW-VALUES`, each
/// within the one-byte `USRTYPEI` width.
const BLANKS: [&str; 3] = ["", " ", "\u{0}"];

/// `{id}` segments the edits read as blank: one space, the 8 spaces of
/// `USRIDINI X(8)`, and `LOW-VALUES`.
const BLANK_ID_PATHS: [&str; 3] = [
    "/admin/users/%20",
    "/admin/users/%20%20%20%20%20%20%20%20",
    "/admin/users/%00",
];

/// Sets of members one `PUT` blanks, after a blank `{id}` when `true`. Each is
/// answered by the row with the lowest (Order, ID) among its blank fields:
/// the first true `WHEN` of the one `EVALUATE` [app/cbl/COUSR02C.cbl:L179-213].
const BLANK_SETS: [(bool, &[&str]); 5] = [
    (true, &["first_name", "last_name", "password", "usr_type"]),
    (false, &["first_name", "last_name", "password", "usr_type"]),
    (false, &["last_name", "password", "usr_type"]),
    (false, &["password", "usr_type"]),
    (true, &["usr_type"]),
];

/// The regular user of the seed, `USER0001` [app/jcl/DUSRSECJ.jcl:L40]: the
/// user ID, names, type and stored password the fetch, the edits, the E-21
/// cases and the screen keys act on and expect.
const REGULAR_USER_ROW: &str = "P-CSUSR01Y-009";

/// The count row of the `DUSRSECJ` users [app/jcl/DUSRSECJ.jcl:L35-44],
/// whose cited records, decoded with the reviewed `CSUSR01Y` layout, give
/// the seeded users the failure cases act on and expect.
const SEEDED_USERS_ROW: &str = "P-CSUSR01Y-007";

/// The first name the E-21 cases change `USER0001` to.
const CHANGED_FIRST_NAME: &str = "LARRY";

/// The password the last E-21 case sets, upper case as sign-on compares it.
const NEW_PASSWORD: &str = "NEWPASS1";

/// A user ID no seeded user holds.
const UNKNOWN_USER: &str = "NOSUCHUS";

/// The fetch and update route of [`UNKNOWN_USER`].
const UNKNOWN_PATH: &str = "/admin/users/NOSUCHUS";

/// The seeded user whose stored row the read failure corrupts
/// [app/jcl/DUSRSECJ.jcl:L41]. [`CORRUPT_USER0002`] names the same ID.
const USER0002: &str = "USER0002";

/// The fetch route of [`USER0002`].
const USER0002_PATH: &str = "/admin/users/USER0002";

/// The seeded user whose rewrite the trigger skips or fails
/// [app/jcl/DUSRSECJ.jcl:L42]. [`SKIP_REWRITE`] names the same ID.
const USER0003: &str = "USER0003";

/// The fetch and update route of [`USER0003`].
const USER0003_PATH: &str = "/admin/users/USER0003";

/// The first name the rewrite failures try to give [`USER0003`].
const RENAMED_USER0003: &str = "LAURA";

/// The sign-on route.
const LOGIN_PATH: &str = "/auth/login";

/// The head of every argon2 PHC string: no answer may carry it.
const ARGON2_MARK: &str = "$argon2";

/// Every stored user in key order, each column in its `PostgreSQL` text form:
/// ID, first name, last name, password hash, type.
///
/// One user is picked from these rows in Rust rather than by a `WHERE`
/// built for its ID, because the hygiene gate refuses SQL built with
/// `format!`.
const USER_ROWS: &str = "SELECT user_id, sec_usr_fname, sec_usr_lname, password_hash, \
     sec_usr_type FROM users ORDER BY user_id";

/// Drops the Latin-1 `CHECK` of `sec_usr_lname` and stores `KUMAR` and the
/// euro sign (`chr(8364)`, above `U+00FF`) as the last name of `USER0002`. No
/// `PIC X(20)` field holds that character, so reading the row into
/// `SEC-USER-DATA` fails: a `READ` that answers neither `NORMAL` nor `NOTFND`.
const CORRUPT_USER0002: &str = "ALTER TABLE users DROP CONSTRAINT users_sec_usr_lname_check; \
     UPDATE users SET sec_usr_lname = 'KUMAR' || chr(8364) WHERE user_id = 'USER0002'";

/// Puts the seeded last name of `USER0002` back.
const REPAIR_USER0002: &str = "UPDATE users SET sec_usr_lname = 'KUMAR' WHERE user_id = 'USER0002'";

/// A `BEFORE UPDATE` trigger on the row of `USER0003` that returns `NULL`, so
/// its `UPDATE` changes no row: the `REWRITE` `NOTFND`.
const SKIP_REWRITE: &str = r"
CREATE FUNCTION cu02_rewrite_fault() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RETURN NULL;
END
$$;
CREATE TRIGGER cu02_rewrite_fault BEFORE UPDATE ON users
    FOR EACH ROW WHEN (OLD.user_id = 'USER0003') EXECUTE FUNCTION cu02_rewrite_fault();
";

/// The same trigger raising an error instead: a `REWRITE` that fails with any
/// other response.
const FAIL_REWRITE: &str = r"
CREATE OR REPLACE FUNCTION cu02_rewrite_fault() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'injected USRSEC rewrite failure';
END
$$;
";

/// Removes the rewrite fault.
const CLEAR_REWRITE_FAULT: &str =
    "DROP TRIGGER cu02_rewrite_fault ON users; DROP FUNCTION cu02_rewrite_fault()";

/// One validation audit row as an answer is compared with it.
struct Audited {
    /// The row ID.
    id: &'static str,
    /// The HTTP status of the answer.
    status: StatusCode,
    /// The `DomainError` kind of an error row; `None` for a message-only row
    /// (`kind=none`).
    kind: Option<ErrorKind>,
    /// The catalog text, equal to the row's quoted message text.
    message: &'static str,
    /// The BMS field the error names.
    field: String,
    /// The rank at which the COBOL reaches the row for one input.
    order: u16,
}

impl Audited {
    /// The row `id` of `doc`, whose quoted message text must equal the catalog
    /// `message`.
    ///
    /// # Panics
    ///
    /// Panics, naming the row, when it is missing, a cell is missing or
    /// unreadable, or its text differs from `message`.
    fn read(doc: &Document, id: &'static str, message: &'static str) -> Self {
        let (table, row) = doc
            .find_row(id)
            .unwrap_or_else(|| panic!("the validation audit has no {id} row"));
        let cell = |name: &str| {
            table
                .cell(row, name)
                .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
                .trim()
        };
        let quoted = code_span(cell("Message text"))
            .unwrap_or_else(|| panic!("{id} quotes no message text"))
            .text;
        assert_eq!(
            quoted
                .strip_prefix('"')
                .and_then(|text| text.strip_suffix('"')),
            Some(message),
            "{id}: the catalog text differs from the audited text {quoted}"
        );
        let status = cell("Status")
            .parse::<u16>()
            .ok()
            .and_then(|code| StatusCode::from_u16(code).ok())
            .unwrap_or_else(|| panic!("{id} has no HTTP status: {}", cell("Status")));
        let kind = key_values(cell("Trigger condition"))
            .into_iter()
            .rev()
            .find(|pair| pair.key == "kind")
            .unwrap_or_else(|| panic!("{id} names no kind"));
        let kind = kind_named(&kind.value);
        let order = cell("Order")
            .parse()
            .unwrap_or_else(|error| panic!("{id} has no numeric order: {error}"));
        Self {
            id,
            status,
            kind,
            message,
            field: cell("BMS field").to_owned(),
            order,
        }
    }

    /// The `COUSR02C` row `id`, with its text from [`COUSR02C_ROWS`].
    fn cousr02c(doc: &Document, id: &'static str) -> Self {
        let message = COUSR02C_ROWS
            .iter()
            .find_map(|&(row, message)| (row == id).then_some(message))
            .unwrap_or_else(|| panic!("{id} is not a COUSR02C row of this file"));
        Self::read(doc, id, message)
    }

    /// Asserts that `resp` is this row's error answer: its status, the code of
    /// its kind, its text, and its BMS field carrying the same text.
    fn assert_answer(&self, resp: &Resp) {
        let kind = self
            .kind
            .unwrap_or_else(|| panic!("{} is a message-only row, not an error", self.id));
        assert_eq!(
            resp.0, self.status,
            "{}: status of the answer: {}",
            self.id, resp.2
        );
        assert_error(
            resp,
            self.status,
            kind.code(),
            self.message,
            &[(self.field.as_str(), self.message)],
        );
    }
}

/// The error kind the audit names `name`, `None` for `none`.
fn kind_named(name: &str) -> Option<ErrorKind> {
    match name {
        "none" => None,
        "Invalid" => Some(ErrorKind::Invalid),
        "Empty" => Some(ErrorKind::Empty),
        "NotFound" => Some(ErrorKind::NotFound),
        "Duplicate" => Some(ErrorKind::Duplicate),
        "Conflict" => Some(ErrorKind::Conflict),
        "SignonFailed" => Some(ErrorKind::SignonFailed),
        "Unexpected" => Some(ErrorKind::Unexpected),
        other => panic!("the validation audit names an unknown kind {other:?}"),
    }
}

/// Whether the review logs of `doc` record `id` with a `pass` verdict and no
/// other verdict.
fn reviewed(doc: &Document, id: &str) -> bool {
    let verdicts: Vec<&str> = doc
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| {
            table
                .rows
                .iter()
                .filter(move |row| table.cell(row, "Row ID") == Some(id))
                .filter_map(move |row| table.cell(row, "Verdict"))
        })
        .collect();
    !verdicts.is_empty() && verdicts.iter().all(|verdict| *verdict == "pass")
}

/// The row of the PF5 edit of `member`.
fn member_row(member: &str) -> &'static str {
    MEMBER_EDITS
        .iter()
        .find_map(|&(name, id)| (name == member).then_some(id))
        .unwrap_or_else(|| panic!("{member} has no PF5 edit"))
}

/// One stored user, each column as `PostgreSQL` text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredUser {
    user_id: String,
    first_name: String,
    last_name: String,
    password_hash: String,
    usr_type: String,
}

/// The [`StoredUser`] of one [`USER_ROWS`] row.
fn to_stored(row: &[Option<String>]) -> StoredUser {
    let column = |index: usize| {
        row.get(index)
            .cloned()
            .flatten()
            .unwrap_or_else(|| panic!("users column {index} is missing or null: {row:?}"))
    };
    StoredUser {
        user_id: column(0),
        first_name: column(1),
        last_name: column(2),
        password_hash: column(3),
        usr_type: column(4),
    }
}

/// The stored row of `user_id`, `None` when no user has that ID.
async fn stored(app: &TestApp, user_id: &str) -> Option<StoredUser> {
    app.stack()
        .fetch_text_rows(USER_ROWS)
        .await
        .iter()
        .map(Vec::as_slice)
        .map(to_stored)
        .find(|user| user.user_id == user_id)
}

/// The stored row of `user_id`, which must exist.
async fn stored_user(app: &TestApp, user_id: &str) -> StoredUser {
    stored(app, user_id)
        .await
        .unwrap_or_else(|| panic!("no stored user {user_id}"))
}

/// The stored argon2id hash of `user_id`.
async fn hash_of(app: &TestApp, user_id: &str) -> String {
    stored_user(app, user_id).await.password_hash
}

/// A user as the fetch and the update answers show it.
fn summary(user_id: &str, first_name: &str, last_name: &str, usr_type: &str) -> Value {
    json!({
        "user_id": user_id,
        "first_name": first_name,
        "last_name": last_name,
        "usr_type": usr_type,
    })
}

/// A `PUT` body: the names and type, and `password` only when given, an
/// absent one standing for the current password (E-21).
fn form(first_name: &str, last_name: &str, usr_type: &str, password: Option<&str>) -> Value {
    let mut body = json!({
        "first_name": first_name,
        "last_name": last_name,
        "usr_type": usr_type,
    });
    if let Some(password) = password {
        body["password"] = Value::from(password);
    }
    body
}

/// The `PUT` body of the seeded `user` with its seeded last name and type.
fn user0001_form(user: &SeededUser, first_name: &str, password: Option<&str>) -> Value {
    form(first_name, &user.last_name, &user.usr_type, password)
}

/// The seeded `user` as the fetch and the update answers show it, with the
/// first name `first_name`.
fn seeded_summary(user: &SeededUser, first_name: &str) -> Value {
    summary(&user.user_id, first_name, &user.last_name, &user.usr_type)
}

/// The fetch and update route of the user `user_id`.
fn user_path(user_id: &str) -> String {
    format!("/admin/users/{user_id}")
}

/// The seeded user `user_id` among the decoded `users`.
fn seeded_record<'u>(users: &'u [SeededUser], user_id: &str) -> &'u SeededUser {
    users
        .iter()
        .find(|user| user.user_id == user_id)
        .unwrap_or_else(|| panic!("{SEEDED_USERS_ROW}: no seeded user {user_id}"))
}

/// `body` with `member` set to `value`, or removed when `value` is `None`.
fn with_member(body: &Value, member: &str, value: Option<&str>) -> Value {
    let mut body = body.clone();
    let members = body.as_object_mut().expect("a PUT body is a JSON object");
    match value {
        Some(value) => {
            members.insert(member.to_owned(), Value::from(value));
        }
        None => {
            members.remove(member);
        }
    }
    body
}

/// Every member name of `value`, at any depth.
fn member_names(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                names.push(name.clone());
                member_names(member, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                member_names(item, names);
            }
        }
        _ => {}
    }
}

/// Asserts that the answer `resp` carries no password: no member name holds
/// `password`, and no text holds an argon2 hash (E-21, P-COUSR02C-007).
fn assert_no_password(resp: &Resp) {
    let (_, body, raw) = resp;
    assert!(
        !raw.contains(ARGON2_MARK),
        "the answer shows a password hash: {raw}"
    );
    let mut names = Vec::new();
    member_names(body, &mut names);
    assert!(
        names
            .iter()
            .all(|name| !name.to_ascii_lowercase().contains("password")),
        "the answer names a password member: {raw}"
    );
}

/// Asserts the 200 of a rewrite of `user`: `User <id> has been updated ...` in
/// its `X(80)` receiver and the user as written, the fields kept on the screen
/// (P-COUSR02C-006, P-COUSR02C-013).
fn assert_updated(resp: &Resp, user: &Value) {
    let (status, body, raw) = resp;
    let user_id = user["user_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the expected user has no ID: {user}"));
    assert_eq!(*status, StatusCode::OK, "the rewrite of {user_id}: {raw}");
    assert_eq!(
        body["message"].as_str(),
        Some(messages::user_updated_message(user_id).as_str()),
        "the success text of the rewrite of {user_id}: {raw}"
    );
    assert_eq!(&body["user"], user, "the user as written: {raw}");
    assert_no_password(resp);
}

/// Asserts the message-only answer of `audited` (V-COUSR02C-010): its status and
/// text, and the seeded `user` as stored with the first name `first_name`.
fn assert_not_modified(resp: &Resp, audited: &Audited, user: &SeededUser, first_name: &str) {
    let id = audited.id;
    assert!(audited.kind.is_none(), "{id} must be a message-only row");
    let (status, body, raw) = resp;
    assert_eq!(*status, audited.status, "{id}: status of the answer: {raw}");
    assert_eq!(
        body["message"].as_str(),
        Some(audited.message),
        "{id}: text of the answer: {raw}"
    );
    assert_eq!(
        body["user"],
        seeded_summary(user, first_name),
        "{id}: the user as stored: {raw}"
    );
    assert_no_password(resp);
}

/// Each PF5 edit alone: every blank form of the member, and an absent name or
/// type, which is the empty field too, get the member's row; a blank `{id}`
/// gets [`UPDATE_EMPTY_ID`]. An absent password is not blank: it stands for
/// the current one (E-21), which the E-21 test covers.
async fn edit_each_field(app: &TestApp, token: &str, doc: &Document, user: &SeededUser) {
    let path = user_path(&user.user_id);
    let seeded = user0001_form(user, &user.first_name, None);
    for (member, id) in MEMBER_EDITS {
        let row = Audited::cousr02c(doc, id);
        for blank in BLANKS {
            let body = with_member(&seeded, member, Some(blank));
            row.assert_answer(&app.put(&path, Some(token), &body).await);
        }
        if member != "password" {
            let body = with_member(&seeded, member, None);
            row.assert_answer(&app.put(&path, Some(token), &body).await);
        }
    }
    let empty_id = Audited::cousr02c(doc, UPDATE_EMPTY_ID);
    for path in BLANK_ID_PATHS {
        empty_id.assert_answer(&app.put(path, Some(token), &seeded).await);
    }
}

/// Several blank fields in one `PUT`: the row with the lowest (Order, ID)
/// among them answers (P-COUSR02C-004).
async fn edit_blank_sets(app: &TestApp, token: &str, doc: &Document, user: &SeededUser) {
    let user0001_path = user_path(&user.user_id);
    for (blank_id, members) in BLANK_SETS {
        let mut rows: Vec<Audited> = members
            .iter()
            .map(|member| Audited::cousr02c(doc, member_row(member)))
            .collect();
        let body = members.iter().fold(
            user0001_form(user, &user.first_name, None),
            |body, member| with_member(&body, member, Some("")),
        );
        let path = if blank_id {
            rows.push(Audited::cousr02c(doc, UPDATE_EMPTY_ID));
            BLANK_ID_PATHS[0]
        } else {
            user0001_path.as_str()
        };
        let first = rows
            .iter()
            .min_by_key(|row| (row.order, row.id))
            .expect("every set blanks a field");
        first.assert_answer(&app.put(path, Some(token), &body).await);
    }
}

/// The ENTER fetch: `USER0001` as seeded, without its password, then the
/// unknown and the blank IDs. The user its reviewed row records is the one
/// its `DUSRSECJ` record decodes to.
#[test]
fn the_fetch_shows_the_user_without_its_password() {
    let doc = validation_audit();
    let parity = parity_audit();
    let user0001 = seeded_user(&parity, REGULAR_USER_ROW);
    let decoded = seeded_users(&parity, SEEDED_USERS_ROW);
    assert_eq!(
        seeded_record(&decoded, &user0001.user_id),
        &user0001,
        "{REGULAR_USER_ROW} records the user its {SEEDED_USERS_ROW} record decodes to"
    );
    let not_found = Audited::cousr02c(&doc, FETCH_NOT_FOUND);
    let empty_id = Audited::cousr02c(&doc, FETCH_EMPTY_ID);
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        // P-COUSR02C-002, P-COUSR02C-003: READ UPDATE NORMAL moves the names
        // and type to the screen; P-COUSR02C-007: never the password.
        let path = user_path(&user0001.user_id);
        let fetched = app.get(&path, Some(&token)).await;
        assert_eq!(fetched.0, StatusCode::OK, "the fetch: {}", fetched.2);
        assert_eq!(
            fetched.1,
            seeded_summary(&user0001, &user0001.first_name),
            "the fetched user: {}",
            fetched.2
        );
        assert_no_password(&fetched);
        not_found.assert_answer(&app.get(UNKNOWN_PATH, Some(&token)).await);
        for path in BLANK_ID_PATHS {
            empty_id.assert_answer(&app.get(path, Some(&token)).await);
        }
    });
}

/// The PF5 empty edits in screen order, which write nothing, then the unknown
/// user, whose edits pass and whose failed read for update ends in the
/// rewrite's text (E-46).
#[test]
fn the_update_edits_each_field_in_screen_order() {
    let doc = validation_audit();
    let user0001 = seeded_user(&parity_audit(), REGULAR_USER_ROW);
    let unknown = Audited::cousr02c(&doc, UPDATE_UNKNOWN_USER);
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let before = stored_user(&app, &user0001.user_id).await;
        edit_each_field(&app, &token, &doc, &user0001).await;
        edit_blank_sets(&app, &token, &doc, &user0001).await;
        assert_eq!(
            stored_user(&app, &user0001.user_id).await,
            before,
            "a rejected update must write nothing"
        );
        let body = user0001_form(&user0001, &user0001.first_name, Some(&user0001.password));
        unknown.assert_answer(&app.put(UNKNOWN_PATH, Some(&token), &body).await);
        assert_eq!(
            stored(&app, UNKNOWN_USER).await,
            None,
            "the update of an unknown user must write nothing"
        );
    });
}

/// E-21 case 1: no password and the stored names and type: the message-only
/// answer, nothing written.
async fn no_password_and_no_change(app: &TestApp, token: &str, doc: &Document, user: &SeededUser) {
    let before = stored_user(app, &user.user_id).await;
    let body = user0001_form(user, &user.first_name, None);
    let resp = app.put(&user_path(&user.user_id), Some(token), &body).await;
    assert_not_modified(
        &resp,
        &Audited::cousr02c(doc, UPDATE_NOT_MODIFIED),
        user,
        &user.first_name,
    );
    assert_eq!(
        stored_user(app, &user.user_id).await,
        before,
        "an update that changes nothing must write nothing"
    );
}

/// E-21 case 2: no password and a changed first name: rewritten, with the
/// stored hash kept, never re-hashed (P-COUSR02C-005).
async fn no_password_and_a_new_name(
    app: &TestApp,
    token: &str,
    user: &SeededUser,
    seeded_hash: &str,
) {
    let path = user_path(&user.user_id);
    let body = user0001_form(user, CHANGED_FIRST_NAME, None);
    let resp = app.put(&path, Some(token), &body).await;
    let renamed = seeded_summary(user, CHANGED_FIRST_NAME);
    assert_updated(&resp, &renamed);
    let after = stored_user(app, &user.user_id).await;
    assert_eq!(
        after.first_name, CHANGED_FIRST_NAME,
        "the rewritten first name"
    );
    assert_eq!(
        after.password_hash, seeded_hash,
        "an absent password must leave the stored hash as it was"
    );
    let fetched = app.get(&path, Some(token)).await;
    assert_eq!(
        fetched.1, renamed,
        "the fetch after the rewrite: {}",
        fetched.2
    );
}

/// E-21 case 3: a present, empty password is the blank field.
async fn an_empty_password(
    app: &TestApp,
    token: &str,
    doc: &Document,
    user: &SeededUser,
    seeded_hash: &str,
) {
    let body = user0001_form(user, CHANGED_FIRST_NAME, Some(""));
    let resp = app.put(&user_path(&user.user_id), Some(token), &body).await;
    Audited::cousr02c(doc, UPDATE_EMPTY_PASSWORD).assert_answer(&resp);
    assert_eq!(
        hash_of(app, &user.user_id).await,
        seeded_hash,
        "a rejected password must leave the stored hash as it was"
    );
}

/// E-21 case 4: the stored password entered again, nothing else changed: the
/// argon2 verification finds it equal, so the answer is message-only and
/// nothing is written.
async fn the_same_password(
    app: &TestApp,
    token: &str,
    doc: &Document,
    user: &SeededUser,
    seeded_hash: &str,
) {
    let before = stored_user(app, &user.user_id).await;
    let body = user0001_form(user, CHANGED_FIRST_NAME, Some(&user.password));
    let resp = app.put(&user_path(&user.user_id), Some(token), &body).await;
    assert_not_modified(
        &resp,
        &Audited::cousr02c(doc, UPDATE_NOT_MODIFIED),
        user,
        CHANGED_FIRST_NAME,
    );
    let after = stored_user(app, &user.user_id).await;
    assert_eq!(
        after.password_hash, seeded_hash,
        "an equal password must not be re-hashed"
    );
    assert_eq!(after, before, "an equal password must write nothing");
}

/// E-21 case 5: a new password is re-hashed and rewritten; sign-on then takes
/// it and refuses the replaced one (E-20).
async fn a_new_password(
    app: &TestApp,
    token: &str,
    doc: &Document,
    user: &SeededUser,
    seeded_hash: &str,
) {
    let body = user0001_form(user, CHANGED_FIRST_NAME, Some(NEW_PASSWORD));
    let resp = app.put(&user_path(&user.user_id), Some(token), &body).await;
    let renamed = seeded_summary(user, CHANGED_FIRST_NAME);
    assert_updated(&resp, &renamed);
    let hash = hash_of(app, &user.user_id).await;
    assert_ne!(hash, seeded_hash, "a changed password must be re-hashed");
    assert!(
        hash.starts_with(ARGON2_MARK),
        "the new hash is an argon2 PHC string"
    );
    let new_token = app.login(&user.user_id, NEW_PASSWORD).await;
    assert!(!new_token.is_empty(), "the new password signs on");
    let old = json!({ "user_id": user.user_id, "password": user.password });
    let refused = app.post(LOGIN_PATH, None, &old).await;
    Audited::read(
        doc,
        SIGNON_WRONG_PASSWORD,
        messages::WRONG_PASSWORD_TRY_AGAIN,
    )
    .assert_answer(&refused);
}

/// The five E-21 cases of PF5, in order, on `USER0001`: its seeded hash is
/// recorded first and compared after each.
#[test]
fn the_password_follows_the_e21_rules() {
    let doc = validation_audit();
    let user = seeded_user(&parity_audit(), REGULAR_USER_ROW);
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        let seeded_hash = hash_of(&app, &user.user_id).await;
        no_password_and_no_change(&app, &token, &doc, &user).await;
        no_password_and_a_new_name(&app, &token, &user, &seeded_hash).await;
        an_empty_password(&app, &token, &doc, &user, &seeded_hash).await;
        the_same_password(&app, &token, &doc, &user, &seeded_hash).await;
        a_new_password(&app, &token, &doc, &user, &seeded_hash).await;
    });
}

/// The read failure of the fetch, then the `REWRITE` `NOTFND` and the
/// `REWRITE` failure, each injected through SQL and each writing nothing; the
/// same update succeeds once the fault is removed. The two users are those
/// their `DUSRSECJ` records decode to.
#[test]
fn read_and_rewrite_failures_answer_with_their_audited_texts() {
    let doc = validation_audit();
    let seeded = seeded_users(&parity_audit(), SEEDED_USERS_ROW);
    let user0002 = seeded_record(&seeded, USER0002);
    let user0003 = seeded_record(&seeded, USER0003);
    let read_failed = Audited::cousr02c(&doc, FETCH_READ_FAILED);
    let rewrite_not_found = Audited::cousr02c(&doc, REWRITE_NOT_FOUND);
    let rewrite_failed = Audited::cousr02c(&doc, REWRITE_FAILED);
    let app = TestApp::start();
    block_on(async {
        let token = app.admin_token().await;
        // P-COUSR02C-003, the `other` response of the READ.
        app.stack().exec_sql(CORRUPT_USER0002).await;
        read_failed.assert_answer(&app.get(USER0002_PATH, Some(&token)).await);
        app.stack().exec_sql(REPAIR_USER0002).await;
        let repaired = app.get(USER0002_PATH, Some(&token)).await;
        assert_eq!(
            repaired.0,
            StatusCode::OK,
            "the fetch once repaired: {}",
            repaired.2
        );
        assert_eq!(
            repaired.1,
            seeded_summary(user0002, &user0002.first_name),
            "{SEEDED_USERS_ROW}: the repair restores {USER0002} as seeded: {}",
            repaired.2
        );

        // P-COUSR02C-006, the NOTFND and `other` responses of the REWRITE.
        let before = stored_user(&app, USER0003).await;
        let body = form(
            RENAMED_USER0003,
            &user0003.last_name,
            &user0003.usr_type,
            None,
        );
        app.stack().exec_sql(SKIP_REWRITE).await;
        rewrite_not_found.assert_answer(&app.put(USER0003_PATH, Some(&token), &body).await);
        assert_eq!(
            stored_user(&app, USER0003).await,
            before,
            "a rewrite that finds no row must write nothing"
        );
        app.stack().exec_sql(FAIL_REWRITE).await;
        rewrite_failed.assert_answer(&app.put(USER0003_PATH, Some(&token), &body).await);
        assert_eq!(
            stored_user(&app, USER0003).await,
            before,
            "a failed rewrite must write nothing"
        );
        app.stack().exec_sql(CLEAR_REWRITE_FAULT).await;
        let written = app.put(USER0003_PATH, Some(&token), &body).await;
        let renamed = seeded_summary(user0003, RENAMED_USER0003);
        assert_updated(&written, &renamed);
        assert_eq!(
            stored_user(&app, USER0003).await.first_name,
            RENAMED_USER0003,
            "the update once the fault is removed"
        );
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

/// The property names of the component schema `name` of `spec`.
fn schema_members(spec: &Value, name: &str) -> BTreeSet<String> {
    spec["components"]["schemas"][name]["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document has no {name} schema properties"))
        .keys()
        .cloned()
        .collect()
}

/// The text between the double quotes that open and close `text`, the form
/// the audits quote a COBOL literal in.
fn unquoted(text: &str) -> Option<&str> {
    text.strip_prefix('"')?.strip_suffix('"')
}

/// Every string of `value`, at any depth; its member names are collected by
/// [`member_names`].
fn strings_of<'v>(value: &'v Value, strings: &mut Vec<&'v str>) {
    match value {
        Value::Object(members) => {
            for member in members.values() {
                strings_of(member, strings);
            }
        }
        Value::Array(items) => {
            for item in items {
                strings_of(item, strings);
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
            let mut names = Vec::new();
            member_names(&body, &mut names);
            for name in names {
                let name = name.to_ascii_lowercase();
                assert!(!self.members.contains(&name), "{id}: member {name}: {raw}");
            }
            let mut strings = Vec::new();
            strings_of(&body, &mut strings);
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

/// `RECEIVE-USRUPD-SCREEN` — P-COUSR02C-010.
///
/// The route parameter and the `PUT` body replace the received `COUSR2A`
/// map: the update publishes exactly the `id` path parameter (`USRIDINI`)
/// and a body of the four other inputs, each of which the row names as the
/// `I` field `UPDATE-USER-INFO` reads. `RESP` is never tested, so a body that
/// delivers no field still reaches `UPDATE-USER-INFO`: its first-name edit
/// answers, not a receive failure.
async fn received_map(
    app: &TestApp,
    token: &str,
    validation: &Document,
    parity: &Document,
    user: &SeededUser,
    bodies: &mut Vec<String>,
) {
    let read = parity_cell(parity, RECEIVE_ROW, "Expected result");
    for field in UPDATE_FIELDS {
        let input = format!("{}I", field.as_str());
        assert!(read.contains(&input), "{RECEIVE_ROW} names {input}");
    }
    let spec = spec();
    let parameters: BTreeSet<(&str, &str)> = spec["paths"][ID_PATH]["put"]["parameters"]
        .as_array()
        .unwrap_or_else(|| panic!("{RECEIVE_ROW}: PUT {ID_PATH} publishes no parameters"))
        .iter()
        .map(|parameter| {
            let text = |name: &str| parameter[name].as_str().unwrap_or_default();
            (text("name"), text("in"))
        })
        .collect();
    assert_eq!(
        parameters,
        BTreeSet::from([("id", "path")]),
        "{RECEIVE_ROW}"
    );
    let members: BTreeSet<String> = MEMBER_EDITS
        .iter()
        .map(|(member, _)| (*member).to_owned())
        .collect();
    assert_eq!(
        schema_members(&spec, "UserUpdateRequest"),
        members,
        "{RECEIVE_ROW}: the update body holds the four COUSR2A inputs after USRIDINI"
    );
    let resp = app
        .put(&user_path(&user.user_id), Some(token), &json!({}))
        .await;
    bodies.push(resp.2.clone());
    Audited::cousr02c(validation, UPDATE_EMPTY_FIRST_NAME).assert_answer(&resp);
}

/// `RETURN-TO-PREV-SCREEN` and `CLEAR-CURRENT-SCREEN` — P-COUSR02C-008,
/// P-COUSR02C-012.
///
/// Of the function map rows the return row names, PF3's is the `PUT` that
/// `UPDATE-USER-INFO` becomes before the screen is left, whatever it
/// answers, and PF12's sends no request; the rows the clear row names (PF4)
/// send none either. The entry without a COMMAREA (`EIBCALEN = 0`) is a
/// request without a session: the fetch and the update answer 401
/// `Authentication required` with no field, and the seeded `user` stays as
/// stored.
async fn return_and_clear_keys(
    app: &TestApp,
    parity: &Document,
    map_doc: &Document,
    user: &SeededUser,
    bodies: &mut Vec<String>,
) {
    let update = Endpoint {
        method: "put".to_owned(),
        path: ID_PATH.to_owned(),
    };
    let returns = named_requests(parity, map_doc, RETURN_ROW);
    for (id, request) in &returns {
        assert!(
            request.as_ref().is_none_or(|request| *request == update),
            "{RETURN_ROW}: {id} sends {request:?}"
        );
    }
    assert!(returns.iter().any(|(_, request)| request.is_none()));
    assert!(returns.iter().any(|(_, request)| request.is_some()));
    for (id, request) in named_requests(parity, map_doc, CLEAR_ROW) {
        assert_eq!(request, None, "{CLEAR_ROW}: {id} sends no request");
    }
    let path = user_path(&user.user_id);
    let before = stored_user(app, &user.user_id).await;
    let renamed = user0001_form(user, CHANGED_FIRST_NAME, None);
    for token in NO_SESSION {
        let fetched = common::send(app.client().get(app.url(&path)), token).await;
        let request = app.client().put(app.url(&path)).json(&renamed);
        let updated = common::send(request, token).await;
        for resp in [fetched, updated] {
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
    assert_eq!(
        stored_user(app, &user.user_id).await,
        before,
        "{RETURN_ROW}: a request without a session wrote {}",
        user.user_id
    );
}

/// `MAIN-PARA`'s `EVALUATE EIBAID` — P-COUSR02C-001.
///
/// ENTER is the published fetch and PF3 and PF5 the published update the
/// function map names for them; PF4's clear, PF12's `XCTL COADM01C` and the
/// invalid-key error send no request. Neither operation takes key input, so
/// a fetch or an update naming any other AID key answers as without it, and
/// never with the invalid-key text, whose audited 49 bytes fit its `X(50)`.
/// The requests act on the seeded `user`.
async fn aid_keys_map_to_requests(
    app: &TestApp,
    token: &str,
    (validation, parity, map_doc): (&Document, &Document, &Document),
    user: &SeededUser,
    bodies: &mut Vec<String>,
) {
    let spec = spec();
    assert_eq!(
        operations(&spec, ID_PATH),
        BTreeSet::from(["delete", "get", "put"])
    );
    let keys = aid_keys(parity, map_doc);
    for key in &keys {
        match (&key.endpoint, key.sends_no_request()) {
            (None, true) => {}
            (Some(Endpoint { method, path }), false) => {
                assert_eq!(path, ID_PATH, "{AID_ROW} {}", key.key);
                assert!(
                    ["get", "put"].contains(&method.as_str()),
                    "{AID_ROW} {}",
                    key.key
                );
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

    let user0001_path = user_path(&user.user_id);
    let unchanged = user0001_form(user, &user.first_name, None);
    let fetched = app.get(&user0001_path, Some(token)).await;
    let kept = app.put(&user0001_path, Some(token), &unchanged).await;
    let not_modified = Audited::cousr02c(validation, UPDATE_NOT_MODIFIED);
    assert_not_modified(&kept, &not_modified, user, &user.first_name);
    let mut answers = vec![fetched.clone(), kept.clone()];
    for aid in OTHER_KEYS {
        let path = format!("{user0001_path}?{AID_MEMBER}={aid}");
        let fetch = app.get(&path, Some(token)).await;
        assert_eq!(
            (fetch.0, &fetch.1),
            (fetched.0, &fetched.1),
            "{AID_ROW} {aid}"
        );
        let body = with_member(&unchanged, AID_MEMBER, Some(aid));
        let update = app.put(&user0001_path, Some(token), &body).await;
        assert_eq!((update.0, &update.1), (kept.0, &kept.1), "{AID_ROW} {aid}");
        answers.extend([fetch, update]);
    }
    let renamed = user0001_form(user, CHANGED_FIRST_NAME, None);
    let body = with_member(&renamed, AID_MEMBER, Some(OTHER_KEYS[0]));
    let updated = app.put(&user0001_path, Some(token), &body).await;
    assert_updated(&updated, &seeded_summary(user, CHANGED_FIRST_NAME));
    answers.push(updated);
    for (_, _, raw) in answers {
        let text = messages::CCDA_MSG_INVALID_KEY.trim_end();
        assert!(
            !raw.contains(text),
            "{AID_ROW}: the invalid-key text: {raw}"
        );
        bodies.push(raw);
    }
}

/// `POPULATE-HEADER-INFO` — P-COUSR02C-011: no body `bodies` holds, and no
/// schema of the fetch and the update, carries the 3270 header: no member of
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

/// The 3270 mechanics of `COUSR02C` and their REST replacements, on one
/// seeded database: the received map, the return and clear keys with the
/// entry without a session, the AID keys of `MAIN-PARA`, and, over every
/// answer they read, the absent header.
#[test]
fn the_screen_keys_and_the_header_have_their_rest_replacements() {
    let validation = validation_audit();
    let parity = parity_audit();
    let user = seeded_user(&parity, REGULAR_USER_ROW);
    let map_doc = function_map();
    let app = TestApp::start();
    let bodies = block_on(async {
        let token = app.admin_token().await;
        let mut bodies = Vec::new();
        received_map(&app, &token, &validation, &parity, &user, &mut bodies).await;
        return_and_clear_keys(&app, &parity, &map_doc, &user, &mut bodies).await;
        let docs = (&validation, &parity, &map_doc);
        aid_keys_map_to_requests(&app, &token, docs, &user, &mut bodies).await;
        bodies
    });
    assert_no_header(&bodies, &parity);
}

/// Every `COUSR02C` row of the validation audit is one this file sends, in
/// audit order, with a `pass` review verdict and the catalog text; the
/// sign-on row the replaced password meets passed review too.
#[test]
fn every_cousr02c_validation_row_is_exercised() {
    let doc = validation_audit();
    let exercised: Vec<&str> = COUSR02C_ROWS.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        validation_ids(PROGRAM),
        exercised,
        "the {PROGRAM} rows of the validation audit and the rows this file exercises"
    );
    for (id, message) in COUSR02C_ROWS {
        let row = Audited::read(&doc, id, message);
        assert!(
            reviewed(&doc, row.id),
            "{id} has no pass verdict in the review log; only reviewed rows gate a test"
        );
    }
    Audited::read(
        &doc,
        SIGNON_WRONG_PASSWORD,
        messages::WRONG_PASSWORD_TRY_AGAIN,
    );
    assert!(
        reviewed(&doc, SIGNON_WRONG_PASSWORD),
        "{SIGNON_WRONG_PASSWORD} has no pass verdict in the review log"
    );
}

/// Every parity row the tests here cite is a reviewed `COUSR02C` row.
#[test]
fn the_cited_parity_rows_are_reviewed() {
    let doc = parity_audit();
    for id in PARITY_ROWS.into_iter().chain(SCREEN_ROWS) {
        let (table, row) = doc
            .find_row(id)
            .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
        assert_eq!(
            table.cell(row, "Program"),
            Some(PROGRAM),
            "the program of {id}"
        );
        assert!(
            reviewed(&doc, id),
            "{id} has no pass verdict in the review log of the parity audit"
        );
    }
}
