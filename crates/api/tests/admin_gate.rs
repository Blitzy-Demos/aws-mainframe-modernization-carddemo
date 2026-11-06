//! The admin gate of every `/admin/**` operation, proved through the HTTP
//! layer: a request without a bearer token is 401 `Authentication required`,
//! the token of the regular user `USER0001` (type `U`) is 403 `Admin access
//! required`, and the token of the admin `ADMIN001` (type `A`) with valid input
//! is the operation's 2xx. The reward-rule operations are held to the same
//! gate as the user and transaction-type operations.
//!
//! The COBOL keeps a `U` user out of the admin functions only through menu
//! routing. The user type is `CDEMO-USER-TYPE`, whose 88 levels are `A` and
//! `U` [app/cpy/COCOM01Y.cpy:L26-28]. COSGN00C transfers an `A` user to
//! COADM01C [app/cbl/COSGN00C.cbl:L230-232], whose six options are the user
//! and transaction-type programs [app/cpy/COADM02Y.cpy:L22-53] and which
//! transfers to the chosen one without testing the user type again
//! [app/cbl/COADM01C.cbl:L141-146]. COMEN01C refuses an admin-only option to a
//! `U` user [app/cbl/COMEN01C.cbl:L136-143]. On the server, `AdminUser` takes
//! the place of that routing on every admin route: it authenticates first, so
//! a request without a token is 401 there too, and checks the role second.
//!
//! The operations under test are read from the `OpenAPI` document the server
//! is built from, so an admin route added without a case in [`CASES`] fails
//! `the_case_table_is_every_admin_operation_of_the_openapi_document`, which
//! needs no container.
//!
//! In the matrix test, each operation's two refused requests carry exactly
//! the input the admin's request then uses successfully, so a refusal can come
//! only from the gate, never from a missing resource or a rejected body. The
//! table each operation reads or writes is read in full before and after the
//! refused requests, proving they wrote nothing, and after the admin's write,
//! proving that one took effect. Docker must be running: the matrix starts one
//! `postgres:16.15` container.

pub mod common;

use std::collections::BTreeSet;

use carddemo_domain::messages;
use carddemo_testkit::block_on;
use common::{Resp, TestApp, assert_error};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The `code` of a 401 for a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The `code` of a 403 for a valid token that is not an admin's.
const FORBIDDEN: &str = "FORBIDDEN";

/// The path prefix of the operations under test.
const ADMIN_PREFIX: &str = "/admin";

/// The `OpenAPI` path-item keys read as operations.
const DOCUMENTED_METHODS: [&str; 5] = ["get", "post", "put", "delete", "patch"];

/// The last line of a table-versus-document failure.
const REMEDY: &str = "add a valid-input case to admin_gate.rs for each documented operation \
     without one, and remove each case whose operation is no longer documented";

/// The user the matrix adds, updates and deletes; no seeded user has this ID.
const GATE_USER: &str = "GATEUSR1";

/// The transaction type the matrix adds, updates and deletes: the seed holds
/// types `01` to `07`, and no category references `08`.
const GATE_TYPE: &str = "08";

/// Every `users` row, in key order, read by the user operations' snapshots.
const USERS_TABLE: &str = "SELECT * FROM users ORDER BY user_id";

/// Every Db2 master `transaction_types` row, in key order.
const TYPES_TABLE: &str = "SELECT * FROM transaction_types ORDER BY tr_type";

/// Every `reward_rules` row, active and soft-deleted, in key order.
const RULES_TABLE: &str = "SELECT * FROM reward_rules ORDER BY rule_id";

/// The HTTP method of one admin operation. A POST or PUT carries the builder
/// of the valid JSON body every request of the operation is sent with.
#[derive(Clone, Copy)]
enum Verb {
    Get,
    Post(fn() -> Value),
    Put(fn() -> Value),
    Delete,
}

impl Verb {
    /// The method as the operation table prints it.
    const fn name(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post(_) => "POST",
            Self::Put(_) => "PUT",
            Self::Delete => "DELETE",
        }
    }

    /// The status of the admin's request with valid input: 201 for a create,
    /// 200 for a read, an update or a delete.
    const fn admitted(self) -> StatusCode {
        match self {
            Self::Post(_) => StatusCode::CREATED,
            Self::Get | Self::Put(_) | Self::Delete => StatusCode::OK,
        }
    }

    /// Whether the admin's request writes, so that its table must change.
    const fn writes(self) -> bool {
        !matches!(self, Self::Get)
    }

    /// Sends the request to `path`, with `token` as the bearer token when
    /// given, through the harness.
    async fn send(self, app: &TestApp, path: &str, token: Option<&str>) -> Resp {
        match self {
            Self::Get => app.get(path, token).await,
            Self::Post(body) => app.post(path, token, &body()).await,
            Self::Put(body) => app.put(path, token, &body()).await,
            Self::Delete => app.delete(path, token).await,
        }
    }
}

/// What an earlier case's admitted request returned that a later case
/// addresses: the `rule_id` of the reward rule the matrix creates.
#[derive(Default)]
struct Captured {
    rule_id: Option<String>,
}

impl Captured {
    /// The created rule's `rule_id`.
    ///
    /// # Panics
    ///
    /// Panics if no case has captured it yet, which means [`CASES`] puts an
    /// update or delete of the rule before its create.
    fn rule_id(&self) -> String {
        self.rule_id.clone().unwrap_or_else(|| {
            panic!("the POST /admin/reward-rules case must precede the cases that address its rule")
        })
    }
}

/// One admin operation and its valid input.
struct Case {
    /// The method, with the body of a POST or PUT.
    verb: Verb,
    /// The documented path, with an `{id}` capture for one resource.
    template: &'static str,
    /// The `{id}` the requests address; `None` for a collection path.
    key: Option<fn(&Captured) -> String>,
    /// The statement reading, in full, the table the operation reads or writes.
    table: &'static str,
    /// Records what a later case needs from the admitted response.
    capture: Option<fn(&mut Captured, &Resp)>,
}

impl Case {
    /// The operation as the document names it, such as `PUT /admin/users/{id}`.
    fn label(&self) -> String {
        format!("{} {}", self.verb.name(), self.template)
    }

    /// The request path: the template with its `{id}` replaced by the key.
    fn path(&self, captured: &Captured) -> String {
        match self.key {
            Some(key) => self.template.replace("{id}", &key(captured)),
            None => self.template.to_owned(),
        }
    }
}

/// Every admin operation, in an order where each create precedes the reads,
/// updates and deletes of what it created.
const CASES: [Case; 13] = [
    // CU00 (COUSR00C)
    Case {
        verb: Verb::Get,
        template: "/admin/users",
        key: None,
        table: USERS_TABLE,
        capture: None,
    },
    // CU01 (COUSR01C)
    Case {
        verb: Verb::Post(|| {
            json!({
                "first_name": "GATE",
                "last_name": "KEEPER",
                "user_id": GATE_USER,
                "password": "PASSWORD",
                "usr_type": "U",
            })
        }),
        template: "/admin/users",
        key: None,
        table: USERS_TABLE,
        capture: None,
    },
    // CU02 (COUSR02C) fetch
    Case {
        verb: Verb::Get,
        template: "/admin/users/{id}",
        key: Some(|_| GATE_USER.to_owned()),
        table: USERS_TABLE,
        capture: None,
    },
    // CU02 (COUSR02C) update: a changed first name, the password left as stored
    Case {
        verb: Verb::Put(|| {
            json!({
                "first_name": "GATED",
                "last_name": "KEEPER",
                "usr_type": "U",
            })
        }),
        template: "/admin/users/{id}",
        key: Some(|_| GATE_USER.to_owned()),
        table: USERS_TABLE,
        capture: None,
    },
    // CU03 (COUSR03C)
    Case {
        verb: Verb::Delete,
        template: "/admin/users/{id}",
        key: Some(|_| GATE_USER.to_owned()),
        table: USERS_TABLE,
        capture: None,
    },
    // CTLI (COTRTLIC) list
    Case {
        verb: Verb::Get,
        template: "/admin/transaction-types",
        key: None,
        table: TYPES_TABLE,
        capture: None,
    },
    // CTTU (COTRTUPC) add
    Case {
        verb: Verb::Post(|| json!({ "type_code": GATE_TYPE, "description": "Gate test type" })),
        template: "/admin/transaction-types",
        key: None,
        table: TYPES_TABLE,
        capture: None,
    },
    // CTTU (COTRTUPC) update, without `origin`
    Case {
        verb: Verb::Put(|| json!({ "description": "Gate test type renamed" })),
        template: "/admin/transaction-types/{id}",
        key: Some(|_| GATE_TYPE.to_owned()),
        table: TYPES_TABLE,
        capture: None,
    },
    // CTLI (COTRTLIC) delete
    Case {
        verb: Verb::Delete,
        template: "/admin/transaction-types/{id}",
        key: Some(|_| GATE_TYPE.to_owned()),
        table: TYPES_TABLE,
        capture: None,
    },
    // Reward rules: list, active and soft-deleted
    Case {
        verb: Verb::Get,
        template: "/admin/reward-rules",
        key: None,
        table: RULES_TABLE,
        capture: None,
    },
    // Reward rules: create, open-ended and unbounded
    Case {
        verb: Verb::Post(|| reward_rule("2.0000")),
        template: "/admin/reward-rules",
        key: None,
        table: RULES_TABLE,
        capture: Some(capture_rule_id),
    },
    // Reward rules: update of the created rule's rate
    Case {
        verb: Verb::Put(|| reward_rule("2.5000")),
        template: "/admin/reward-rules/{id}",
        key: Some(Captured::rule_id),
        table: RULES_TABLE,
        capture: None,
    },
    // Reward rules: soft delete of the created rule
    Case {
        verb: Verb::Delete,
        template: "/admin/reward-rules/{id}",
        key: Some(Captured::rule_id),
        table: RULES_TABLE,
        capture: None,
    },
];

/// A reward-rule body for type `01` Purchase and category `0001` Regular
/// Sales Draft [app/data/ASCII/trancatg.txt:L1] at `rate`, a decimal string,
/// without amount bounds, active from 2022-01-01 with no end.
fn reward_rule(rate: &str) -> Value {
    json!({
        "transaction_type_cd": "01",
        "transaction_category_cd": "1",
        "reward_rate": rate,
        "active_from": "2022-01-01T00:00:00Z",
    })
}

/// Keeps the `rule_id` of the created rule for the update and delete cases.
///
/// # Panics
///
/// Panics, showing the raw body, if the response has no string `rule_id`.
fn capture_rule_id(captured: &mut Captured, resp: &Resp) {
    let (_, body, raw) = resp;
    let rule_id = body["rule_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the created reward rule has no string rule_id: {raw}"));
    captured.rule_id = Some(rule_id.to_owned());
}

/// Every `(METHOD, path)` of the `OpenAPI` document whose path starts with
/// [`ADMIN_PREFIX`], the method upper case.
///
/// # Panics
///
/// Panics if the document is not JSON or has no `paths` object.
fn documented_admin_operations() -> BTreeSet<(String, String)> {
    let document: Value = serde_json::from_str(&carddemo_api::openapi::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    let paths = document["paths"]
        .as_object()
        .unwrap_or_else(|| panic!("the OpenAPI document has no paths object"));
    paths
        .iter()
        .filter(|(path, _)| path.starts_with(ADMIN_PREFIX))
        .flat_map(|(path, item)| {
            DOCUMENTED_METHODS
                .iter()
                .filter(|method| item.get(**method).is_some())
                .map(|method| (method.to_ascii_uppercase(), path.clone()))
        })
        .collect()
}

/// `operations` one per line as `METHOD path`, or `none`.
fn listed<'a>(operations: impl Iterator<Item = &'a (String, String)>) -> String {
    let lines: Vec<String> = operations
        .map(|(method, path)| format!("  {method} {path}"))
        .collect();
    if lines.is_empty() {
        "  none".to_owned()
    } else {
        lines.join("\n")
    }
}

#[test]
fn the_case_table_is_every_admin_operation_of_the_openapi_document() {
    let documented = documented_admin_operations();
    let tabled: BTreeSet<(String, String)> = CASES
        .iter()
        .map(|case| (case.verb.name().to_owned(), case.template.to_owned()))
        .collect();
    assert_eq!(
        tabled.len(),
        CASES.len(),
        "CASES lists an admin operation twice"
    );
    assert!(
        documented == tabled,
        "the CASES table of admin_gate.rs differs from the admin operations of the OpenAPI \
         document\ndocumented without a case:\n{}\ncases the document does not list:\n{}\n{REMEDY}",
        listed(documented.difference(&tabled)),
        listed(tabled.difference(&documented)),
    );
    for case in &CASES {
        assert_eq!(
            case.key.is_some(),
            case.template.contains("{id}"),
            "{}: a case addresses an `{{id}}` exactly when its template has one",
            case.label()
        );
    }
}

/// One seeded database serves the whole matrix: every case runs in [`CASES`]
/// order, so each create precedes the cases that use what it created.
#[test]
fn every_admin_operation_is_401_without_a_token_403_for_a_user_and_2xx_for_an_admin() {
    let app = TestApp::start();
    block_on(async {
        let admin = app.admin_token().await;
        let user = app.user_token().await;
        let mut captured = Captured::default();
        for case in &CASES {
            run_case(&app, case, &admin, &user, &mut captured).await;
        }
    });
}

/// Sends `case` without a token, with the `U` token `user` and with the `A`
/// token `admin`, all with the same valid input, and asserts the gate's answer
/// to each and the table around them.
async fn run_case(app: &TestApp, case: &Case, admin: &str, user: &str, captured: &mut Captured) {
    let label = case.label();
    let path = case.path(captured);
    let before = app.stack().fetch_text_rows(case.table).await;

    let anonymous = case.verb.send(app, &path, None).await;
    assert_eq!(
        anonymous.0,
        StatusCode::UNAUTHORIZED,
        "{label} at {path} without a token: {}",
        anonymous.2
    );
    assert_error(
        &anonymous,
        StatusCode::UNAUTHORIZED,
        UNAUTHENTICATED,
        messages::AUTHENTICATION_REQUIRED,
        &[],
    );

    let refused = case.verb.send(app, &path, Some(user)).await;
    assert_eq!(
        refused.0,
        StatusCode::FORBIDDEN,
        "{label} at {path} with the USER0001 token: {}",
        refused.2
    );
    assert_error(
        &refused,
        StatusCode::FORBIDDEN,
        FORBIDDEN,
        messages::ADMIN_ACCESS_REQUIRED,
        &[],
    );
    assert_eq!(
        app.stack().fetch_text_rows(case.table).await,
        before,
        "{label} at {path}: the refused requests changed `{}`",
        case.table
    );

    let admitted = case.verb.send(app, &path, Some(admin)).await;
    let expected = case.verb.admitted();
    assert!(
        admitted.0.is_success() && admitted.0 == expected,
        "{label} at {path} with the ADMIN001 token: expected {expected}, got {}: {}",
        admitted.0,
        admitted.2
    );
    if case.verb.writes() {
        assert_ne!(
            app.stack().fetch_text_rows(case.table).await,
            before,
            "{label} at {path}: the admitted write left `{}` unchanged",
            case.table
        );
    }
    if let Some(capture) = case.capture {
        capture(captured, &admitted);
    }
}
