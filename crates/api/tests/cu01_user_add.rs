//! CU01, `POST /admin/users`: the `COUSR01C` user add proved through the HTTP
//! layer on a seeded database, from the rows of `docs/parity/validation-audit.md`
//! and `docs/parity/parity-oracle-audit.md` for `COUSR01C`.
//!
//! - `PROCESS-ENTER-KEY` edits the five `COUSR1A` fields for emptiness in one
//!   `EVALUATE`, in screen order [app/cbl/COUSR01C.cbl:L117-147]: a blank
//!   first name, last name, user ID, password or user type is 400 with its
//!   message on its BMS field, and the first blank field wins whatever follows
//!   it. An absent member, `""`, spaces and the field's `LOW-VALUES` are all
//!   blank. A member longer than its BMS field has no `COUSR01C` path (a 3270
//!   field never sends more than its width), so the DTO answers it as a
//!   no-message 400 `INVALID` on that field (E-49).
//! - `WRITE-USER-SEC-FILE` [app/cbl/COUSR01C.cbl:L238-274]: a new user is 201
//!   with `User <id> has been added ...`; an existing user ID, the seeded
//!   `ADMIN001` included, is 409 `User ID already exist...` on `USERID` and
//!   rewrites nothing; any other write failure, injected here with a trigger,
//!   is 500 `Unable to Add User...` on `FNAME` and writes nothing.
//! - Every field is stored as entered [app/cbl/COUSR01C.cbl:L154-158]: the
//!   user ID, names and type keep their case, and the password is the argon2id
//!   hash of the 8 space-padded bytes as entered, so the upper-casing sign-on
//!   check does not accept a mixed-case one (E-20). No response carries a
//!   password or its hash.
//! - The 3270 mechanics of `COUSR01C` and their REST replacements: ENTER is
//!   `POST /admin/users`, PF3, PF4 and the invalid-key answer send no
//!   request, and a body naming another AID key is answered as ENTER; an
//!   add without a session is 401 and writes nothing; the body replaces the
//!   received map and every answer the sent one, with no 3270 header field;
//!   after a write only the added text shows.
//!
//! Docker must be running: the add test starts one `postgres:16.15` container
//! and runs every step on it, so the steps share one seeded state.

pub mod common;

use std::collections::BTreeSet;

use carddemo_api::dto::admin_users::{UserAddField, UserMessageResponse, UserSummary};
use carddemo_audit::{
    Document, FUNCTION_MAP_PATH, IdKind, PARITY_AUDIT_PATH, Repo, VALIDATION_AUDIT_PATH,
    cell_code_spans, code_span, id_tokens, key_values,
};
use carddemo_domain::auth::{verify_as_entered, verify_signon};
use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use carddemo_testkit::{block_on, fixtures};
use common::{Resp, TestApp, assert_error, parity_audit, validation_audit, validation_ids};
use reqwest::StatusCode;
use serde_json::{Map, Value};

/// The program whose validation rows this file proves.
const PROGRAM: &str = "COUSR01C";

/// The CU01 add route, also the CU00 list route.
const ADMIN_USERS: &str = "/admin/users";

/// The user the baseline body adds: `USERIDI` at its full `PIC X(8)` width.
const NEW_USER: &str = "NEWUSR01";

/// The password of the baseline body, at its full `PIC X(8)` width and upper
/// case, so the added user signs on.
const NEW_PASSWORD: &str = "PASSWORD";

/// The seeded admin user, the first `DUSRSECJ` row (P-CSUSR01Y-008).
const SEEDED_ADMIN: &str = "ADMIN001";

/// The user added in lower and mixed case, and its mixed-case password (E-20).
const MIXED_USER: &str = "mixUsr01";

/// The password of [`MIXED_USER`], in mixed case.
const MIXED_PASSWORD: &str = "PaSs1234";

/// The user whose insert the injected trigger refuses. [`REFUSE_INSERT`]
/// names the same ID.
const FAILING_USER: &str = "FAILUSR1";

/// The `message` of a member longer than its BMS field: the DTO width check
/// answers it as a no-message path, 400 `INVALID` with the field named and an
/// empty text, because no `COUSR01C` path receives such a value (E-49).
const NO_MESSAGE: &str = "";

/// Every stored user, as `fetch_text_rows` reads it: `SEC-USR-ID`,
/// `SEC-USR-FNAME`, `SEC-USR-LNAME`, `SEC-USR-TYPE` and the hash that replaces
/// `SEC-USR-PWD` (`CSUSR01Y` [app/cpy/CSUSR01Y.cpy:L17-23]). Tests pick a user
/// by exact ID, as the `COLLATE "C"` key compares it.
const STORED_USERS: &str = "SELECT user_id, sec_usr_fname, sec_usr_lname, sec_usr_type, \
                            password_hash FROM users ORDER BY user_id";

/// A trigger that makes the insert of [`FAILING_USER`] fail with an error that
/// is not a duplicate key: the `WHEN OTHER` outcome of the `WRITE`.
const REFUSE_INSERT: &str = "CREATE FUNCTION cu01_refuse_insert() RETURNS trigger \
     LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'CU01 test: user insert refused'; END $$; \
     CREATE TRIGGER cu01_refuse_insert BEFORE INSERT ON users FOR EACH ROW \
     WHEN (NEW.user_id = 'FAILUSR1') EXECUTE FUNCTION cu01_refuse_insert();";

/// Removes [`REFUSE_INSERT`] again.
const ALLOW_INSERT: &str =
    "DROP TRIGGER cu01_refuse_insert ON users; DROP FUNCTION cu01_refuse_insert();";

/// V-COUSR01C-006: a user ID that already exists.
const DUPLICATE_ROW: &str = "V-COUSR01C-006";

/// V-COUSR01C-007: any other write failure.
const WRITE_FAILURE_ROW: &str = "V-COUSR01C-007";

/// The empty-field checks run in screen order and the first blank field wins.
const EMPTY_ORDER_ROW: &str = "P-COUSR01C-002";

/// User ID, names, password and type are stored exactly as entered.
const AS_ENTERED_ROW: &str = "P-COUSR01C-003";

/// A `WRITE` that answers `NORMAL` is 201 with the added text.
const ADDED_ROW: &str = "P-COUSR01C-004";

/// A `WRITE` that answers `DUPKEY` or `DUPREC` is 409; any other answer is the
/// failure text.
const WRITE_OUTCOME_ROW: &str = "P-COUSR01C-005";

/// The `CSUSR01Y` columns the stored rows are read from: `user_id`,
/// `sec_usr_fname`, `sec_usr_lname`, `password_hash` and `sec_usr_type`.
const COLUMN_ROWS: [&str; 5] = [
    "P-CSUSR01Y-001",
    "P-CSUSR01Y-002",
    "P-CSUSR01Y-003",
    "P-CSUSR01Y-004",
    "P-CSUSR01Y-005",
];

/// The seeded admin `ADMIN001`, `MARGARET GOLD`, type `A`.
const SEEDED_ADMIN_ROW: &str = "P-CSUSR01Y-008";

/// `MAIN-PARA`'s `EVALUATE EIBAID`: ENTER, PF3, PF4 and any other key, each
/// with its action.
const AID_ROW: &str = "P-COUSR01C-001";

/// `RETURN-TO-PREV-SCREEN`: PF3, and the entry without a COMMAREA that the
/// bearer token replaces; nothing is written.
const RETURN_ROW: &str = "P-COUSR01C-006";

/// `SEND-USRADD-SCREEN`: every answer replaces the sent `COUSR1A` map.
const SEND_ROW: &str = "P-COUSR01C-007";

/// `RECEIVE-USRADD-SCREEN`: the request body replaces the received map.
const RECEIVE_ROW: &str = "P-COUSR01C-008";

/// `POPULATE-HEADER-INFO`: the 3270 header no answer carries.
const HEADER_ROW: &str = "P-COUSR01C-009";

/// `CLEAR-CURRENT-SCREEN`: PF4, the form's Reset control.
const CLEAR_ROW: &str = "P-COUSR01C-010";

/// `INITIALIZE-ALL-FIELDS`: after the write only the added text shows.
const INITIALIZE_ROW: &str = "P-COUSR01C-011";

/// The `COUSR01C` screen, as the function map's `BMS map` cell names it.
const MAP: &str = "COUSR01";

/// The key [`AID_ROW`] names for every AID key it answers with the
/// invalid-key text.
const OTHER_KEY: &str = "other";

/// AID keys `MAIN-PARA` answers with the invalid-key text.
const OTHER_KEYS: [&str; 3] = ["PF9", "PA1", "CLEAR"];

/// The body member a request would name an AID key in: one the add does not
/// publish.
const AID_MEMBER: &str = "aid";

/// The user an add naming another AID key adds, as ENTER does.
const AID_USER: &str = "AIDUSR01";

/// The user the [`INITIALIZE_ROW`] step adds right after rejecting it.
const INIT_USER: &str = "INITUSR1";

/// The user an add without a session names, and must not write.
const NO_SESSION_USER: &str = "NOSESS01";

/// The bearer of a request without a session: none, or one that does not
/// verify.
const NO_SESSION: [Option<&str>; 2] = [None, Some("not-a-token")];

/// The `code` of the 401 a request without a session answers.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The HTTP methods an `OpenAPI` path item can hold an operation under.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The schemas of every body this file reads: the add, the fetch, the list
/// and the errors.
const RESPONSE_SCHEMAS: [&str; 4] = [
    "UserMessageResponse",
    "UserSummary",
    "Page_UserSummary",
    "ErrorBody",
];

/// One `COUSR1A` input: its JSON member, its BMS field and width
/// [app/cpy-bms/COUSR01.CPY:L60-84], and the validation row and message of its
/// empty-field check.
struct Member {
    json: &'static str,
    field: UserAddField,
    width: usize,
    empty_row: &'static str,
    empty_message: &'static str,
}

/// The five inputs in the order `PROCESS-ENTER-KEY` edits them.
const MEMBERS: [Member; 5] = [
    Member {
        json: "first_name",
        field: UserAddField::FName,
        width: 20,
        empty_row: "V-COUSR01C-001",
        empty_message: messages::FIRST_NAME_CAN_NOT_BE_EMPTY,
    },
    Member {
        json: "last_name",
        field: UserAddField::LName,
        width: 20,
        empty_row: "V-COUSR01C-002",
        empty_message: messages::LAST_NAME_CAN_NOT_BE_EMPTY,
    },
    Member {
        json: "user_id",
        field: UserAddField::UserId,
        width: 8,
        empty_row: "V-COUSR01C-003",
        empty_message: messages::USER_ID_CAN_NOT_BE_EMPTY,
    },
    Member {
        json: "password",
        field: UserAddField::Passwd,
        width: 8,
        empty_row: "V-COUSR01C-004",
        empty_message: messages::PASSWORD_CAN_NOT_BE_EMPTY,
    },
    Member {
        json: "usr_type",
        field: UserAddField::UsrType,
        width: 1,
        empty_row: "V-COUSR01C-005",
        empty_message: messages::USER_TYPE_CAN_NOT_BE_EMPTY,
    },
];

/// Every validation row this file proves.
fn proven_validation_rows() -> BTreeSet<&'static str> {
    MEMBERS
        .iter()
        .map(|member| member.empty_row)
        .chain([DUPLICATE_ROW, WRITE_FAILURE_ROW])
        .collect()
}

/// Every parity row this file proves.
fn proven_parity_rows() -> BTreeSet<&'static str> {
    [
        EMPTY_ORDER_ROW,
        AS_ENTERED_ROW,
        ADDED_ROW,
        WRITE_OUTCOME_ROW,
        SEEDED_ADMIN_ROW,
        AID_ROW,
        RETURN_ROW,
        SEND_ROW,
        RECEIVE_ROW,
        HEADER_ROW,
        CLEAR_ROW,
        INITIALIZE_ROW,
    ]
    .into_iter()
    .chain(COLUMN_ROWS)
    .collect()
}

/// A way a member reaches `PROCESS-ENTER-KEY` as `SPACES OR LOW-VALUES`.
#[derive(Debug, Clone, Copy)]
enum Blank {
    /// The member left out of the body: the empty field.
    Absent,
    /// `""`.
    Empty,
    /// The field's width of spaces.
    Spaces,
    /// The field's width of NUL characters, its `LOW-VALUES`.
    LowValues,
}

impl Blank {
    const ALL: [Self; 4] = [Self::Absent, Self::Empty, Self::Spaces, Self::LowValues];

    /// Makes `member` of `body` blank this way.
    fn apply(self, body: &mut Map<String, Value>, member: &Member) {
        let value = match self {
            Self::Absent => {
                body.remove(member.json);
                return;
            }
            Self::Empty => String::new(),
            Self::Spaces => " ".repeat(member.width),
            Self::LowValues => "\0".repeat(member.width),
        };
        body.insert(member.json.to_owned(), Value::from(value));
    }
}

/// The add body of a new user `user_id` with every other member valid: first
/// name `JANE`, last name `DOE`, password [`NEW_PASSWORD`] and type `U`.
fn new_user_body(user_id: &str) -> Map<String, Value> {
    [
        ("first_name", "JANE"),
        ("last_name", "DOE"),
        ("user_id", user_id),
        ("password", NEW_PASSWORD),
        ("usr_type", "U"),
    ]
    .into_iter()
    .map(|(member, value)| (member.to_owned(), Value::from(value)))
    .collect()
}

/// The user [`new_user_body`] adds, as the API shows it.
fn new_user_summary(user_id: &str) -> UserSummary {
    UserSummary {
        user_id: user_id.to_owned(),
        first_name: "JANE".to_owned(),
        last_name: "DOE".to_owned(),
        usr_type: "U".to_owned(),
    }
}

/// What the run proved: the audit rows exercised, every response body read,
/// for the credential and header scans, and the bodies that answered an add,
/// for the [`SEND_ROW`] check.
#[derive(Default)]
struct Proof {
    validation_rows: BTreeSet<&'static str>,
    parity_rows: BTreeSet<&'static str>,
    bodies: Vec<String>,
    posts: Vec<String>,
}

impl Proof {
    /// Sends `GET path` as the admin and keeps the body.
    async fn get(&mut self, app: &TestApp, token: &str, path: &str) -> Resp {
        let resp = app.get(path, Some(token)).await;
        self.bodies.push(resp.2.clone());
        resp
    }

    /// Sends `POST /admin/users` with `body` as the admin and keeps the
    /// answer's body.
    async fn add(&mut self, app: &TestApp, token: &str, body: Map<String, Value>) -> Resp {
        let resp = app
            .post(ADMIN_USERS, Some(token), &Value::Object(body))
            .await;
        self.bodies.push(resp.2.clone());
        self.posts.push(resp.2.clone());
        resp
    }
}

/// One stored `users` row, every column in its text form.
#[derive(Debug, Clone, PartialEq, Eq)]
struct StoredUser {
    user_id: String,
    first_name: String,
    last_name: String,
    usr_type: String,
    password_hash: String,
}

impl StoredUser {
    /// The row as the API shows the user.
    fn summary(&self) -> UserSummary {
        UserSummary {
            user_id: self.user_id.clone(),
            first_name: self.first_name.clone(),
            last_name: self.last_name.clone(),
            usr_type: self.usr_type.clone(),
        }
    }
}

/// The stored row whose `user_id` is exactly `user_id`, if any.
async fn stored_user(app: &TestApp, user_id: &str) -> Option<StoredUser> {
    let rows = app.stack().fetch_text_rows(STORED_USERS).await;
    rows.into_iter()
        .map(|row| {
            let column = |index: usize| {
                row.get(index)
                    .cloned()
                    .flatten()
                    .unwrap_or_else(|| panic!("users column {index} is null or missing: {row:?}"))
            };
            StoredUser {
                user_id: column(0),
                first_name: column(1),
                last_name: column(2),
                usr_type: column(3),
                password_hash: column(4),
            }
        })
        .find(|stored| stored.user_id == user_id)
}

/// The stored row of `user_id`, which must exist.
async fn existing_user(app: &TestApp, user_id: &str) -> StoredUser {
    stored_user(app, user_id)
        .await
        .unwrap_or_else(|| panic!("no users row holds {user_id}"))
}

/// Asserts that `resp` is the 201 of an add of `user`: the body is exactly
/// `{user, message}`, `user` is exactly the four members of [`UserSummary`]
/// (no password or hash member) and equals `user`, and `message` is the
/// composed `User <id> has been added ...` (P-COUSR01C-004).
fn assert_added(resp: &Resp, user: UserSummary) {
    let (status, json, raw) = resp;
    assert_eq!(*status, StatusCode::CREATED, "status of the add: {raw}");
    let keys = |value: &Value| -> BTreeSet<String> {
        value
            .as_object()
            .unwrap_or_else(|| panic!("not a JSON object: {raw}"))
            .keys()
            .cloned()
            .collect()
    };
    let expected_keys =
        |names: &[&str]| -> BTreeSet<String> { names.iter().copied().map(str::to_owned).collect() };
    assert_eq!(keys(json), expected_keys(&["message", "user"]), "{raw}");
    assert_eq!(
        keys(&json["user"]),
        expected_keys(&["first_name", "last_name", "user_id", "usr_type"]),
        "the added user's members: {raw}"
    );
    let body: UserMessageResponse = serde_json::from_value(json.clone()).unwrap_or_else(|error| {
        panic!("the add body is not a UserMessageResponse: {error}: {raw}")
    });
    let expected = UserMessageResponse {
        message: messages::user_added_message(&user.user_id),
        user,
    };
    assert_eq!(body, expected, "the add body: {raw}");
}

/// Asserts the 409 of an add whose user ID already exists.
fn assert_duplicate(resp: &Resp) {
    assert_error(
        resp,
        StatusCode::CONFLICT,
        ErrorKind::Duplicate.code(),
        messages::USER_ID_ALREADY_EXIST,
        &[(
            UserAddField::UserId.as_str(),
            messages::USER_ID_ALREADY_EXIST,
        )],
    );
}

/// `PROCESS-ENTER-KEY` [app/cbl/COUSR01C.cbl:L117-147]: each member blank in
/// each way, alone and with every later member blank too, is that member's
/// empty-field 400; an empty object is the first-name 400.
async fn empty_fields(app: &TestApp, token: &str, proof: &mut Proof) {
    for (rank, member) in MEMBERS.iter().enumerate() {
        let later = &MEMBERS[rank + 1..];
        for blank in Blank::ALL {
            let mut alone = new_user_body(NEW_USER);
            blank.apply(&mut alone, member);
            let mut bodies = vec![alone.clone()];
            if !later.is_empty() {
                let mut with_later = alone;
                for next in later {
                    blank.apply(&mut with_later, next);
                }
                bodies.push(with_later);
            }
            for body in bodies {
                let resp = proof.add(app, token, body).await;
                assert_eq!(
                    resp.0,
                    StatusCode::BAD_REQUEST,
                    "{} with {} {blank:?}: {}",
                    member.empty_row,
                    member.json,
                    resp.2
                );
                assert_error(
                    &resp,
                    StatusCode::BAD_REQUEST,
                    ErrorKind::Empty.code(),
                    member.empty_message,
                    &[(member.field.as_str(), member.empty_message)],
                );
            }
        }
        proof.validation_rows.insert(member.empty_row);
    }
    let first = &MEMBERS[0];
    let resp = proof.add(app, token, Map::new()).await;
    assert_error(
        &resp,
        StatusCode::BAD_REQUEST,
        ErrorKind::Empty.code(),
        first.empty_message,
        &[(first.field.as_str(), first.empty_message)],
    );
    proof.parity_rows.insert(EMPTY_ORDER_ROW);
}

/// A member one character longer than its BMS field is a no-message 400
/// `INVALID` on that field; all five over at once name the five fields in
/// screen order. Nothing is edited further or written.
async fn over_length(app: &TestApp, token: &str, proof: &mut Proof) {
    let mut all_over = new_user_body(NEW_USER);
    for member in &MEMBERS {
        let over = Value::from("X".repeat(member.width + 1));
        let mut body = new_user_body(NEW_USER);
        body.insert(member.json.to_owned(), over.clone());
        all_over.insert(member.json.to_owned(), over);
        let resp = proof.add(app, token, body).await;
        assert_error(
            &resp,
            StatusCode::BAD_REQUEST,
            ErrorKind::Invalid.code(),
            NO_MESSAGE,
            &[(member.field.as_str(), NO_MESSAGE)],
        );
    }
    let resp = proof.add(app, token, all_over).await;
    let every_field: Vec<(&str, &str)> = MEMBERS
        .iter()
        .map(|member| (member.field.as_str(), NO_MESSAGE))
        .collect();
    assert_error(
        &resp,
        StatusCode::BAD_REQUEST,
        ErrorKind::Invalid.code(),
        NO_MESSAGE,
        &every_field,
    );
}

/// `WRITE-USER-SEC-FILE` `NORMAL` [app/cbl/COUSR01C.cbl:L250-259]: the baseline
/// body is 201, the row holds every member as entered and the hash of the
/// password, and the new user signs on with it.
async fn added(app: &TestApp, token: &str, proof: &mut Proof) {
    let resp = proof.add(app, token, new_user_body(NEW_USER)).await;
    assert_added(&resp, new_user_summary(NEW_USER));
    let stored = existing_user(app, NEW_USER).await;
    assert_eq!(stored.summary(), new_user_summary(NEW_USER), "{stored:?}");
    assert!(
        stored.password_hash.starts_with("$argon2id$"),
        "{NEW_USER}: the stored password is not an argon2id hash"
    );
    // The password was entered in upper case, so sign-on, which upper-cases
    // what is typed, accepts it.
    app.login(NEW_USER, NEW_PASSWORD).await;
    let fetched = proof
        .get(app, token, &format!("{ADMIN_USERS}/{NEW_USER}"))
        .await;
    assert_eq!(fetched.0, StatusCode::OK, "{}", fetched.2);
    proof.parity_rows.insert(ADDED_ROW);
    proof.parity_rows.extend(COLUMN_ROWS);
}

/// `WRITE-USER-SEC-FILE` `DUPKEY`/`DUPREC` [app/cbl/COUSR01C.cbl:L260-266]:
/// the same body again, the same ID with other values, and the seeded
/// `ADMIN001` are each 409 on `USERID`, and the stored rows are unchanged.
async fn duplicates(app: &TestApp, token: &str, proof: &mut Proof) {
    let before = existing_user(app, NEW_USER).await;
    let resp = proof.add(app, token, new_user_body(NEW_USER)).await;
    assert_duplicate(&resp);
    let mut other_values = new_user_body(NEW_USER);
    for (member, value) in [
        ("first_name", "OTHER"),
        ("last_name", "NAMES"),
        ("password", "SECRET01"),
        ("usr_type", "A"),
    ] {
        other_values.insert(member.to_owned(), Value::from(value));
    }
    let resp = proof.add(app, token, other_values).await;
    assert_duplicate(&resp);
    assert_eq!(
        existing_user(app, NEW_USER).await,
        before,
        "{NEW_USER} was rewritten"
    );

    let admin_before = existing_user(app, SEEDED_ADMIN).await;
    let seeded = UserSummary {
        user_id: SEEDED_ADMIN.to_owned(),
        first_name: "MARGARET".to_owned(),
        last_name: "GOLD".to_owned(),
        usr_type: "A".to_owned(),
    };
    assert_eq!(admin_before.summary(), seeded, "{SEEDED_ADMIN_ROW}");
    let resp = proof.add(app, token, new_user_body(SEEDED_ADMIN)).await;
    assert_duplicate(&resp);
    assert_eq!(
        existing_user(app, SEEDED_ADMIN).await,
        admin_before,
        "{SEEDED_ADMIN} was rewritten"
    );
    // Its password is still the seeded one.
    app.admin_token().await;
    proof.validation_rows.insert(DUPLICATE_ROW);
    proof.parity_rows.insert(SEEDED_ADMIN_ROW);
}

/// `PROCESS-ENTER-KEY` moves every field as entered
/// [app/cbl/COUSR01C.cbl:L154-158] (E-20): a user ID, names, type and
/// password in lower and mixed case are stored with their case kept, the
/// hash verifies the password exactly as entered and not the upper-cased
/// value sign-on compares, and the user is addressed by the ID as entered.
async fn mixed_case(app: &TestApp, token: &str, proof: &mut Proof) {
    let entered = UserSummary {
        user_id: MIXED_USER.to_owned(),
        first_name: "Mixed".to_owned(),
        last_name: "Case".to_owned(),
        usr_type: "u".to_owned(),
    };
    let body: Map<String, Value> = [
        ("first_name", entered.first_name.as_str()),
        ("last_name", entered.last_name.as_str()),
        ("user_id", MIXED_USER),
        ("password", MIXED_PASSWORD),
        ("usr_type", entered.usr_type.as_str()),
    ]
    .into_iter()
    .map(|(member, value)| (member.to_owned(), Value::from(value)))
    .collect();
    let resp = proof.add(app, token, body).await;
    assert_added(&resp, entered.clone());

    let stored = existing_user(app, MIXED_USER).await;
    assert_eq!(stored.summary(), entered, "{AS_ENTERED_ROW}: {stored:?}");
    let upper_id = MIXED_USER.to_ascii_uppercase();
    assert_eq!(
        stored_user(app, &upper_id).await,
        None,
        "{AS_ENTERED_ROW}: the user ID was upper-cased"
    );
    let hash = &stored.password_hash;
    assert!(
        hash.starts_with("$argon2id$"),
        "{MIXED_USER}: the stored password is not an argon2id hash"
    );
    for plaintext in [
        MIXED_PASSWORD.to_owned(),
        MIXED_PASSWORD.to_ascii_uppercase(),
    ] {
        assert!(
            !hash.contains(&plaintext),
            "{MIXED_USER}: the stored hash holds the password text"
        );
    }
    assert_eq!(
        verify_as_entered(MIXED_PASSWORD, hash),
        Ok(true),
        "{AS_ENTERED_ROW}: the hash is not that of the password as entered"
    );
    assert_eq!(
        verify_signon(MIXED_PASSWORD, hash),
        Ok(false),
        "E-20: the upper-cased sign-on password matched a mixed-case one"
    );

    let fetched = proof
        .get(app, token, &format!("{ADMIN_USERS}/{MIXED_USER}"))
        .await;
    assert_eq!(fetched.0, StatusCode::OK, "{}", fetched.2);
    let summary: UserSummary = serde_json::from_value(fetched.1.clone())
        .unwrap_or_else(|error| panic!("not a UserSummary: {error}: {}", fetched.2));
    assert_eq!(summary, entered, "the fetched user: {}", fetched.2);
    proof.parity_rows.insert(AS_ENTERED_ROW);
}

/// `WRITE-USER-SEC-FILE` `WHEN OTHER` [app/cbl/COUSR01C.cbl:L267-273]: an
/// insert that fails with an error other than a duplicate key is 500 on
/// `FNAME` and writes nothing; once the failure is removed, the same add is
/// 201.
async fn write_failure(app: &TestApp, token: &str, proof: &mut Proof) {
    app.stack().exec_sql(REFUSE_INSERT).await;
    let resp = proof.add(app, token, new_user_body(FAILING_USER)).await;
    assert_error(
        &resp,
        StatusCode::INTERNAL_SERVER_ERROR,
        ErrorKind::Unexpected.code(),
        messages::UNABLE_TO_ADD_USER,
        &[(UserAddField::FName.as_str(), messages::UNABLE_TO_ADD_USER)],
    );
    assert_eq!(
        stored_user(app, FAILING_USER).await,
        None,
        "{WRITE_FAILURE_ROW}: the failed add wrote {FAILING_USER}"
    );
    app.stack().exec_sql(ALLOW_INSERT).await;
    let resp = proof.add(app, token, new_user_body(FAILING_USER)).await;
    assert_added(&resp, new_user_summary(FAILING_USER));
    proof.validation_rows.insert(WRITE_FAILURE_ROW);
    proof.parity_rows.insert(WRITE_OUTCOME_ROW);
}

/// Reads the CU00 page from [`FAILING_USER`], which lists every user added
/// here, so its body joins the credential scan.
async fn list_added_users(app: &TestApp, token: &str, proof: &mut Proof) {
    let resp = proof
        .get(
            app,
            token,
            &format!("{ADMIN_USERS}?start_key={FAILING_USER}"),
        )
        .await;
    assert_eq!(resp.0, StatusCode::OK, "{}", resp.2);
    let listed: BTreeSet<&str> = resp.1["items"]
        .as_array()
        .unwrap_or_else(|| panic!("the user page has no items: {}", resp.2))
        .iter()
        .filter_map(|item| item["user_id"].as_str())
        .collect();
    for user_id in [FAILING_USER, NEW_USER, MIXED_USER] {
        assert!(
            listed.contains(user_id),
            "{user_id} is not listed: {}",
            resp.2
        );
    }
}

/// No response body names a password member or the hash column, or holds an
/// argon2 hash or the mixed-case password.
fn assert_no_credentials(bodies: &[String]) {
    assert!(!bodies.is_empty(), "no response body was kept");
    for raw in bodies {
        for needle in ["\"password\"", "password_hash", "$argon2", MIXED_PASSWORD] {
            assert!(!raw.contains(needle), "a response holds {needle:?}: {raw}");
        }
    }
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

/// The keys of [`AID_ROW`] in the order its input condition lists them, each
/// with its action and the request of the one function map row of the
/// [`MAP`] screen whose `Key or field` cell starts with the key.
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
        .map(|(key, action)| {
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
                            == Some(*key)
                })
                .filter_map(|(table, row)| table.cell(row, "Endpoint"))
                .collect();
            let [cell] = cells.as_slice() else {
                panic!(
                    "{FUNCTION_MAP_PATH}: {} rows for {key} on {MAP}",
                    cells.len()
                );
            };
            AidKey {
                key: (*key).to_owned(),
                action: action.to_owned(),
                endpoint: endpoint(cell),
            }
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

/// The JSON names of [`MEMBERS`], the five `COUSR1A` inputs.
fn member_names() -> BTreeSet<String> {
    MEMBERS
        .iter()
        .map(|member| member.json.to_owned())
        .collect()
}

/// The text between the double quotes that open and close `text`, the form
/// the audits quote a COBOL literal in.
fn unquoted(text: &str) -> Option<&str> {
    text.strip_prefix('"')?.strip_suffix('"')
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

/// `RECEIVE-USRADD-SCREEN` — P-COUSR01C-008.
///
/// The add body replaces the received `COUSR1A` map: its published members
/// are exactly the five inputs, and the row names each one's BMS field as the
/// `I` field `PROCESS-ENTER-KEY` reads. `RESP` is never tested, so a receive
/// that delivers no field still reaches `PROCESS-ENTER-KEY`: an empty body is
/// answered by the first-name edit, not as a receive failure.
async fn received_map(app: &TestApp, token: &str, proof: &mut Proof, parity: &Document) {
    let read = parity_cell(parity, RECEIVE_ROW, "Expected result");
    for member in &MEMBERS {
        let input = format!("{}I", member.field.as_str());
        assert!(read.contains(&input), "{RECEIVE_ROW} names {input}");
    }
    assert_eq!(
        schema_members(&spec(), "UserAddRequest"),
        member_names(),
        "{RECEIVE_ROW}: the add body holds the five COUSR1A inputs"
    );
    let first = &MEMBERS[0];
    let resp = proof.add(app, token, Map::new()).await;
    assert_error(
        &resp,
        StatusCode::BAD_REQUEST,
        ErrorKind::Empty.code(),
        first.empty_message,
        &[(first.field.as_str(), first.empty_message)],
    );
    proof.parity_rows.insert(RECEIVE_ROW);
}

/// `RETURN-TO-PREV-SCREEN` — P-COUSR01C-006.
///
/// PF3's `XCTL COADM01C` is navigation: each function map row the audit row
/// names sends no request. The entry without a COMMAREA (`EIBCALEN = 0`) is
/// an add without a session: 401 `Authentication required` with no field,
/// and nothing is written.
async fn no_session(app: &TestApp, proof: &mut Proof, parity: &Document, map_doc: &Document) {
    for (id, request) in named_requests(parity, map_doc, RETURN_ROW) {
        assert_eq!(request, None, "{RETURN_ROW}: {id} sends no request");
    }
    let body = Value::Object(new_user_body(NO_SESSION_USER));
    for token in NO_SESSION {
        let request = app.client().post(app.url(ADMIN_USERS)).json(&body);
        let resp = common::send(request, token).await;
        proof.bodies.push(resp.2.clone());
        proof.posts.push(resp.2.clone());
        assert_error(
            &resp,
            StatusCode::UNAUTHORIZED,
            UNAUTHENTICATED,
            messages::AUTHENTICATION_REQUIRED,
            &[],
        );
    }
    assert_eq!(
        stored_user(app, NO_SESSION_USER).await,
        None,
        "{RETURN_ROW}: an add without a session wrote {NO_SESSION_USER}"
    );
    proof.parity_rows.insert(RETURN_ROW);
}

/// `INITIALIZE-ALL-FIELDS` after `WRITE NORMAL` — P-COUSR01C-011.
///
/// An add right after a rejected one: the rejection's text is gone, and the
/// 201 shows only `User <id> has been added ...`, built from the literals the
/// row quotes, beside the user as written.
async fn initialized_after_the_write(
    app: &TestApp,
    token: &str,
    proof: &mut Proof,
    parity: &Document,
) {
    let first = &MEMBERS[0];
    let mut rejected = new_user_body(INIT_USER);
    Blank::Empty.apply(&mut rejected, first);
    let resp = proof.add(app, token, rejected).await;
    assert_eq!(resp.1["message"], first.empty_message, "{}", resp.2);
    let resp = proof.add(app, token, new_user_body(INIT_USER)).await;
    assert_added(&resp, new_user_summary(INIT_USER));
    let texts: Vec<String> =
        cell_code_spans(parity_cell(parity, INITIALIZE_ROW, "Expected result"))
            .iter()
            .filter_map(|span| unquoted(&span.text).map(str::to_owned))
            .collect();
    let [label, tail] = texts.as_slice() else {
        panic!(
            "{INITIALIZE_ROW} quotes {} texts, not the label and the tail",
            texts.len()
        );
    };
    assert_eq!(
        resp.1["message"],
        format!("{label}{INIT_USER}{tail}"),
        "{INITIALIZE_ROW}: only the added text shows: {}",
        resp.2
    );
    proof.parity_rows.insert(INITIALIZE_ROW);
}

/// `MAIN-PARA`'s `EVALUATE EIBAID` and `CLEAR-CURRENT-SCREEN` —
/// P-COUSR01C-001, P-COUSR01C-010.
///
/// ENTER is the published `POST /admin/users` the function map names for
/// it; PF3's `XCTL COADM01C`, PF4's clear (the function map rows the clear
/// row names) and the invalid-key error send no request, and the add body
/// publishes no member a key could travel in. A body naming any other AID
/// key is answered as ENTER, by its edits and then its write, and never with
/// the invalid-key text, whose audited 49 bytes fit its `X(50)`.
async fn aid_keys_map_to_requests(
    app: &TestApp,
    token: &str,
    proof: &mut Proof,
    parity: &Document,
    map_doc: &Document,
) {
    let spec = spec();
    let keys = aid_keys(parity, map_doc);
    for key in &keys {
        match (&key.endpoint, key.sends_no_request()) {
            (None, true) => {}
            (Some(Endpoint { method, path }), false) => {
                assert_eq!((method.as_str(), path.as_str()), ("post", ADMIN_USERS));
                assert!(operations(&spec, path).contains(method.as_str()));
            }
            (request, _) => panic!("{AID_ROW} {}: {:?} but {request:?}", key.key, key.action),
        }
    }
    for (id, request) in named_requests(parity, map_doc, CLEAR_ROW) {
        assert_eq!(request, None, "{CLEAR_ROW}: {id} sends no request");
    }
    assert_eq!(schema_members(&spec, "UserAddRequest"), member_names());
    let other = keys.iter().find(|key| key.key == OTHER_KEY);
    let invalid = other.and_then(|key| code_span(&key.action));
    let invalid = invalid.unwrap_or_else(|| panic!("{AID_ROW} quotes no invalid-key text"));
    assert_eq!(
        invalid.text,
        format!("\"{}\"", messages::CCDA_MSG_INVALID_KEY)
    );
    assert!(invalid.width >= Some(messages::CCDA_MSG_INVALID_KEY.len()));
    let first = &MEMBERS[0];
    let mut answers = Vec::new();
    for aid in OTHER_KEYS {
        let mut body = new_user_body(AID_USER);
        body.insert(AID_MEMBER.to_owned(), Value::from(aid));
        Blank::Empty.apply(&mut body, first);
        let resp = proof.add(app, token, body).await;
        let fields = [(first.field.as_str(), first.empty_message)];
        let code = ErrorKind::Empty.code();
        assert_error(
            &resp,
            StatusCode::BAD_REQUEST,
            code,
            first.empty_message,
            &fields,
        );
        answers.push(resp);
    }
    let mut body = new_user_body(AID_USER);
    body.insert(AID_MEMBER.to_owned(), Value::from(OTHER_KEYS[0]));
    let resp = proof.add(app, token, body).await;
    assert_added(&resp, new_user_summary(AID_USER));
    answers.push(resp);
    for (_, _, raw) in &answers {
        let text = messages::CCDA_MSG_INVALID_KEY.trim_end();
        assert!(
            !raw.contains(text),
            "{AID_ROW}: the invalid-key text: {raw}"
        );
    }
    proof.parity_rows.extend([AID_ROW, CLEAR_ROW]);
}

/// `SEND-USRADD-SCREEN` — P-COUSR01C-007.
///
/// Every answer to `POST /admin/users` replaces the sent map: the 201 body
/// `{user, message}`, or the error body `{code, message, fields}`, whose
/// fields name `COUSR1A` inputs and whose `message` is the text of its first
/// field, the one `WS-MESSAGE` moved to `ERRMSGO`. Each text fits the
/// `WS-MESSAGE` field [`INITIALIZE_ROW`] gives.
fn assert_sends(posts: &[String], parity: &Document) {
    let fields = parity_cell(parity, INITIALIZE_ROW, "Copybook field or PIC clause");
    let width: usize = fields
        .split_once("WS-MESSAGE X(")
        .and_then(|(_, rest)| rest.split_once(')'))
        .and_then(|(digits, _)| digits.parse().ok())
        .unwrap_or_else(|| panic!("{INITIALIZE_ROW} gives no WS-MESSAGE width: {fields}"));
    let inputs: BTreeSet<&str> = MEMBERS.iter().map(|member| member.field.as_str()).collect();
    let added = BTreeSet::from(["message", "user"]);
    let failed = BTreeSet::from(["code", "fields", "message"]);
    assert!(!posts.is_empty(), "{SEND_ROW}: no add was answered");
    for raw in posts {
        let body: Value = serde_json::from_str(raw)
            .unwrap_or_else(|error| panic!("{SEND_ROW}: an answer is not JSON: {error}: {raw}"));
        let members: BTreeSet<&str> = body
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        let message = body["message"].as_str();
        let message = message.unwrap_or_else(|| panic!("{SEND_ROW}: no text: {raw}"));
        assert!(message.len() <= width, "{SEND_ROW}: over X({width}): {raw}");
        if members == added {
            continue;
        }
        assert!(
            members.contains("code") && members.is_subset(&failed),
            "{SEND_ROW}: neither the added nor the error body: {raw}"
        );
        let entries = body["fields"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        for entry in entries {
            let field = entry["field"].as_str().unwrap_or_default();
            assert!(
                inputs.contains(field),
                "{SEND_ROW}: {field} is no COUSR1A input: {raw}"
            );
        }
        if let Some(entry) = entries.first() {
            assert_eq!(
                entry["message"], body["message"],
                "{SEND_ROW}: ERRMSGO: {raw}"
            );
        }
    }
}

/// `POPULATE-HEADER-INFO` — P-COUSR01C-009: no body this file read, and no
/// schema of them, carries the 3270 header: no member of its output fields,
/// none of its texts, no value shaped as its date or time.
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

/// Asserts that `doc`, read from `path`, defines a data row for every ID of
/// `ids` and that the last review-log verdict of each is `pass`.
fn assert_reviewed(doc: &Document, path: &str, ids: &BTreeSet<&str>) {
    for &id in ids {
        assert!(doc.find_row(id).is_some(), "{path} defines no row {id}");
        let verdict = doc
            .tables()
            .iter()
            .filter(|table| table.is_review_log())
            .flat_map(|table| {
                table
                    .rows
                    .iter()
                    .filter(move |row| table.cell(row, "Row ID").map(str::trim) == Some(id))
                    .map(move |row| table.cell(row, "Verdict").map(str::trim))
            })
            .last()
            .flatten();
        assert_eq!(verdict, Some("pass"), "{path}: the review of {id}");
    }
}

/// The rows this file proves are every `COUSR01C` row of the validation
/// audit, and each one, with every parity row it proves, has passed review.
#[test]
fn proven_rows_are_the_reviewed_cousr01c_rows() {
    let defined: BTreeSet<String> = validation_ids(PROGRAM).into_iter().collect();
    let proven: BTreeSet<String> = proven_validation_rows()
        .into_iter()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        proven, defined,
        "the {PROGRAM} rows of {VALIDATION_AUDIT_PATH}"
    );
    assert_reviewed(
        &validation_audit(),
        VALIDATION_AUDIT_PATH,
        &proven_validation_rows(),
    );
    assert_reviewed(&parity_audit(), PARITY_AUDIT_PATH, &proven_parity_rows());
}

/// CU01 end to end on one seeded database: the rejected adds first, so that
/// none can be masked by a user an earlier step added, with the empty body of
/// the received map and the adds without a session, then the add, the
/// duplicates, the mixed-case add, the injected write failure, the add after
/// a rejection, the AID keys and the list. Every validation and parity row
/// of [`proven_rows_are_the_reviewed_cousr01c_rows`] is exercised, every add
/// answer replaces the sent map, and no response carries a credential or the
/// 3270 header.
#[test]
fn post_admin_users_adds_as_cousr01c_does() {
    let parity = parity_audit();
    let map_doc = function_map();
    let app = TestApp::start();
    let mut proof = block_on(async {
        let token = app.admin_token().await;
        let mut proof = Proof::default();
        empty_fields(&app, &token, &mut proof).await;
        over_length(&app, &token, &mut proof).await;
        received_map(&app, &token, &mut proof, &parity).await;
        no_session(&app, &mut proof, &parity, &map_doc).await;
        assert_eq!(
            stored_user(&app, NEW_USER).await,
            None,
            "a rejected add wrote {NEW_USER}"
        );
        added(&app, &token, &mut proof).await;
        duplicates(&app, &token, &mut proof).await;
        mixed_case(&app, &token, &mut proof).await;
        write_failure(&app, &token, &mut proof).await;
        initialized_after_the_write(&app, &token, &mut proof, &parity).await;
        aid_keys_map_to_requests(&app, &token, &mut proof, &parity, &map_doc).await;
        list_added_users(&app, &token, &mut proof).await;
        proof
    });
    assert_no_credentials(&proof.bodies);
    assert_sends(&proof.posts, &parity);
    assert_no_header(&proof.bodies, &parity);
    proof.parity_rows.extend([SEND_ROW, HEADER_ROW]);
    assert_eq!(proof.validation_rows, proven_validation_rows());
    assert_eq!(proof.parity_rows, proven_parity_rows());
}
