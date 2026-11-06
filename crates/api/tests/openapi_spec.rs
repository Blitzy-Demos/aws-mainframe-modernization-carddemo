//! Stale-client gate of the committed `OpenAPI` document (AAP 0.5.4.2, 0.10.1 o7).
//!
//! `web/openapi/openapi.json` is the input of the SPA's type generator, so it must be exactly the
//! document the server's route registry describes: [`spec_json`], byte for byte. A handler or DTO
//! change whose document is not regenerated fails here, with the line the two first differ on and
//! the two regeneration commands. The structure tests pin the wire contract the SPA's generated
//! types and form schemas rely on: the bearer scheme, the 19 resource paths, the error body, money
//! and reward points as strings, and the `<Dto>Field` enums.
//!
//! It needs no container, database or async runtime: only `std`, `serde_json` and
//! [`carddemo_api::openapi`]. Run it alone with `cargo test -p carddemo-api --test openapi_spec`.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use carddemo_api::openapi::spec_json;
use serde_json::Value;

/// The last part of every staleness failure: the two commands, run from the repository root,
/// that bring `web/openapi/openapi.json` and `web/src/api/schema.d.ts` back in line.
const REMEDY: &str = "regenerate it from the repository root with \
    `cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json`, then \
    `pnpm --dir web gen:api`, and commit both generated files";

/// The most characters of a line a staleness failure shows, so a long description line cannot
/// flood the output.
const LINE_PREVIEW: usize = 120;

/// Every path `carddemo_api::handlers::router()` registers: the 19 REST resources of the online
/// transactions served over HTTP and of the rewards system, in axum 0.8 brace-capture syntax.
const PATHS: [&str; 19] = [
    "/auth/login",
    "/auth/logout",
    "/accounts/{id}",
    "/cards",
    "/cards/{id}",
    "/transactions",
    "/transactions/{id}",
    "/reports/transactions",
    "/payments",
    "/authorizations",
    "/authorizations/{id}",
    "/admin/users",
    "/admin/users/{id}",
    "/admin/transaction-types",
    "/admin/transaction-types/{id}",
    "/rewards/balance/{account_id}",
    "/rewards/history/{account_id}",
    "/admin/reward-rules",
    "/admin/reward-rules/{id}",
];

/// The `<Dto>Field` enums: the BMS field names, or the reward-rule field names, an error body's
/// `fields[].field` may carry for each request DTO. `web/src/schemas/*` types its `bmsFieldMap`
/// against them.
const FIELD_ENUMS: [&str; 20] = [
    "LoginField",
    "AccountViewField",
    "AccountUpdateField",
    "CardListField",
    "CardViewField",
    "CardUpdateField",
    "TransactionListField",
    "TransactionViewField",
    "TransactionAddField",
    "ReportRequestField",
    "BillPaymentField",
    "AuthSummaryField",
    "AuthDetailField",
    "UserListField",
    "UserAddField",
    "UserUpdateField",
    "UserDeleteField",
    "TranTypeListField",
    "TranTypeUpsertField",
    "RewardRuleField",
];

/// The `Decimal` money fields of `AccountResponse`, each a `S9(10)V99` field of `CVACT01Y`.
const ACCOUNT_MONEY: [&str; 5] = [
    "acct_curr_bal",
    "acct_credit_limit",
    "acct_cash_credit_limit",
    "acct_curr_cyc_credit",
    "acct_curr_cyc_debit",
];

/// The reward-point properties, `(schema, property)`. Points are a decimal integer string on
/// the wire, because a JSON number above 2^53 − 1 loses precision in the browser.
const POINTS: [(&str, &str); 4] = [
    ("RewardBalanceResponse", "total_points_earned"),
    ("RewardBalanceResponse", "total_points_redeemed"),
    ("RewardBalanceResponse", "available_points"),
    ("RewardAccrualResponse", "points_earned"),
];

/// The committed document, located from this crate's manifest so the test runs from any
/// working directory.
fn committed_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/openapi/openapi.json")
}

/// Where two documents first differ.
struct Difference {
    /// The 1-based number of the line holding the first differing byte.
    line: usize,
    /// The byte offset that line starts at, the same in both documents because they agree
    /// before it.
    line_start: usize,
}

/// The first difference between `committed` and `generated`, or `None` when they are
/// byte-identical. When one is a prefix of the other, the difference is where the shorter ends.
fn first_difference(committed: &[u8], generated: &[u8]) -> Option<Difference> {
    let shared = committed.len().min(generated.len());
    let index = committed
        .iter()
        .zip(generated)
        .position(|(left, right)| left != right)
        .or((committed.len() != generated.len()).then_some(shared))?;
    let agreed = &committed[..index];
    Some(Difference {
        // The agreed bytes split into one piece per line they touch, the last one partial.
        line: agreed.split(|&byte| byte == b'\n').count(),
        line_start: agreed
            .iter()
            .rposition(|&byte| byte == b'\n')
            .map_or(0, |newline| newline + 1),
    })
}

/// The line of `document` starting at `start`, quoted and cut to [`LINE_PREVIEW`] characters,
/// or `<end of file>` when the document ends there.
fn line_at(document: &[u8], start: usize) -> String {
    let Some(rest) = document.get(start..).filter(|rest| !rest.is_empty()) else {
        return "<end of file>".to_owned();
    };
    let line = rest.split(|&byte| byte == b'\n').next().unwrap_or_default();
    let text = String::from_utf8_lossy(line);
    let mut shown: String = text.chars().take(LINE_PREVIEW).collect();
    if text.chars().count() > LINE_PREVIEW {
        shown.push('…');
    }
    format!("{shown:?}")
}

/// The failure message when the committed document at `path`, as read, is not `generated`;
/// `None` when it is. Only the first differing line of each document is shown, never the
/// documents themselves.
fn staleness(
    path: &Path,
    committed: Result<&[u8], &io::Error>,
    generated: &[u8],
) -> Option<String> {
    let shown = path.display();
    let committed = match committed {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Some(format!("{shown} is missing; {REMEDY}"));
        }
        Err(error) => return Some(format!("{shown} cannot be read ({error}); {REMEDY}")),
    };
    let Difference { line, line_start } = first_difference(committed, generated)?;
    Some(format!(
        "{shown} is stale: it first differs from `spec_json()` on line {line} \
         (committed {} bytes, generated {} bytes)\n  committed: {}\n  generated: {}\n{REMEDY}",
        committed.len(),
        generated.len(),
        line_at(committed, line_start),
        line_at(generated, line_start),
    ))
}

/// The generated document, parsed.
fn document() -> Value {
    serde_json::from_str(&spec_json()).expect("spec_json() is JSON")
}

/// The component schema `name` of `document`; panics naming it when it is absent.
fn schema<'a>(document: &'a Value, name: &str) -> &'a Value {
    document
        .pointer(&format!("/components/schemas/{name}"))
        .unwrap_or_else(|| panic!("components.schemas has no {name}"))
}

/// The JSON pointer of every node of `value` that declares a floating-point JSON type: a
/// `type` of `number` (alone or in a type list) or a `format` of `float` or `double`.
fn floating_point_nodes(value: &Value, pointer: &str, found: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            let numeric_type = match map.get("type") {
                Some(Value::String(kind)) => kind == "number",
                Some(Value::Array(kinds)) => kinds.iter().any(|kind| kind == "number"),
                _ => false,
            };
            let float_format = map
                .get("format")
                .and_then(Value::as_str)
                .is_some_and(|format| matches!(format, "float" | "double"));
            if numeric_type || float_format {
                found.push(pointer.to_owned());
            }
            for (key, child) in map {
                let escaped = key.replace('~', "~0").replace('/', "~1");
                floating_point_nodes(child, &format!("{pointer}/{escaped}"), found);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                floating_point_nodes(child, &format!("{pointer}/{index}"), found);
            }
        }
        _ => {}
    }
}

#[test]
fn committed_document_is_byte_identical_to_spec_json() {
    let path = committed_path();
    let committed = fs::read(&path);
    if let Some(message) = staleness(&path, committed.as_deref(), spec_json().as_bytes()) {
        panic!("{message}");
    }
}

#[test]
fn a_stale_or_missing_document_fails_with_its_first_line_and_the_remedy() {
    let path = Path::new("web/openapi/openapi.json");
    let generated: &[u8] = b"{\n  \"openapi\": \"3.1.0\",\n  \"info\": {}\n}\n";
    assert_eq!(staleness(path, Ok(generated), generated), None);

    // (committed bytes, first differing line, committed preview, generated preview)
    let cases: [(&[u8], usize, &str, &str); 3] = [
        (
            b"{\n  \"openapi\": \"3.1.1\",\n  \"info\": {}\n}\n",
            2,
            r#""  \"openapi\": \"3.1.1\",""#,
            r#""  \"openapi\": \"3.1.0\",""#,
        ),
        (
            b"{\n  \"openapi\": \"3.1.0\",\n  \"info\": {}\n}\n\n",
            5,
            r#""""#,
            "<end of file>",
        ),
        (
            b"{\n  \"openapi\": \"3.1.0\",\n",
            3,
            "<end of file>",
            r#""  \"info\": {}""#,
        ),
    ];
    for (committed, line, committed_line, generated_line) in cases {
        let message = staleness(path, Ok(committed), generated).expect("the documents differ");
        let expected = [
            "web/openapi/openapi.json is stale".to_owned(),
            format!("on line {line} "),
            format!("committed: {committed_line}\n"),
            format!("generated: {generated_line}\n"),
            REMEDY.to_owned(),
        ];
        for part in expected {
            assert!(message.contains(&part), "{part:?} in {message}");
        }
    }

    let long = format!("{}\n", "x".repeat(LINE_PREVIEW + 1));
    let message = staleness(path, Ok(long.as_bytes()), generated).expect("the documents differ");
    let preview = format!("\"{}…\"", "x".repeat(LINE_PREVIEW));
    assert!(message.contains(&preview), "{message}");

    let missing = io::Error::from(io::ErrorKind::NotFound);
    let message = staleness(path, Err(&missing), generated).expect("a missing file fails");
    assert_eq!(
        message,
        format!("web/openapi/openapi.json is missing; {REMEDY}")
    );
    for command in [
        "`cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json`",
        "`pnpm --dir web gen:api`",
    ] {
        assert!(REMEDY.contains(command), "{command}");
    }
}

#[test]
fn spec_json_is_deterministic_and_ends_with_a_newline() {
    let first = spec_json();
    assert_eq!(first, spec_json(), "two calls give the same bytes");
    assert!(first.ends_with("}\n"), "the root object then one newline");
    assert!(!first.ends_with("\n\n"), "exactly one trailing newline");
}

#[test]
fn bearer_scheme_is_an_http_jwt_bearer() {
    let document = document();
    let scheme = document
        .pointer("/components/securitySchemes/bearer_auth")
        .expect("components.securitySchemes.bearer_auth");
    assert_eq!(scheme["type"], "http", "{scheme}");
    assert_eq!(scheme["scheme"], "bearer", "{scheme}");
    assert_eq!(scheme["bearerFormat"], "JWT", "{scheme}");
}

#[test]
fn paths_are_the_registered_resources_with_brace_captures() {
    let document = document();
    let documented: BTreeSet<&str> = document["paths"]
        .as_object()
        .expect("paths is an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(documented, BTreeSet::from(PATHS));
    for path in documented {
        assert!(!path.contains(':'), "{path} has no colon capture");
        for segment in path.split('/').skip(1) {
            match segment.strip_prefix('{') {
                Some(capture) => {
                    let name = capture.strip_suffix('}').expect("a closed capture");
                    assert!(
                        !name.is_empty() && !name.contains(['{', '}']),
                        "{path}: {segment}"
                    );
                }
                None => assert!(!segment.contains(['{', '}']), "{path}: {segment}"),
            }
        }
    }
}

#[test]
fn error_body_is_code_message_and_optional_fields() {
    let document = document();
    let body = schema(&document, "ErrorBody");
    let properties: BTreeSet<&str> = body["properties"]
        .as_object()
        .expect("ErrorBody.properties")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(properties, BTreeSet::from(["code", "message", "fields"]));
    assert_eq!(body["required"], serde_json::json!(["code", "message"]));
    assert_eq!(
        body["properties"]["fields"]["items"]["$ref"],
        "#/components/schemas/ErrorFieldBody"
    );

    let field = schema(&document, "ErrorFieldBody");
    for property in ["field", "message"] {
        assert_eq!(
            field["properties"][property]["type"], "string",
            "ErrorFieldBody.{property}"
        );
    }
    assert_eq!(field["required"], serde_json::json!(["field", "message"]));
}

#[test]
fn money_is_a_string_and_no_schema_is_a_json_number() {
    let document = document();
    let account = schema(&document, "AccountResponse");
    for property in ACCOUNT_MONEY {
        assert_eq!(
            account["properties"][property]["type"], "string",
            "AccountResponse.{property}: {}",
            account["properties"][property]
        );
    }

    let mut found = Vec::new();
    floating_point_nodes(&document, "", &mut found);
    assert!(
        found.is_empty(),
        "money, rates and points cross the API as decimal strings; floating-point JSON types \
         at {found:?}"
    );
}

#[test]
fn reward_points_are_decimal_integer_strings() {
    let document = document();
    for (name, property) in POINTS {
        let points = &schema(&document, name)["properties"][property];
        assert_eq!(points["type"], "string", "{name}.{property}: {points}");
        assert_eq!(points["pattern"], "^[0-9]+$", "{name}.{property}: {points}");
    }
}

#[test]
fn every_field_enum_is_a_non_empty_string_enum() {
    let document = document();
    for name in FIELD_ENUMS {
        let field = schema(&document, name);
        assert_eq!(field["type"], "string", "{name}: {field}");
        let values = field["enum"].as_array().expect("an enum list");
        assert!(!values.is_empty(), "{name}: {field}");
        assert!(values.iter().all(Value::is_string), "{name}: {field}");
    }

    let login: BTreeSet<&str> = schema(&document, "LoginField")["enum"]
        .as_array()
        .expect("an enum list")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        login.is_superset(&BTreeSet::from(["USERID", "PASSWD"])),
        "{login:?}"
    );

    let declared: BTreeSet<&str> = document["components"]["schemas"]
        .as_object()
        .expect("components.schemas")
        .keys()
        .map(String::as_str)
        .filter(|name| name.ends_with("Field"))
        .collect();
    assert_eq!(
        declared,
        BTreeSet::from(FIELD_ENUMS),
        "a new `<Dto>Field` enum belongs in FIELD_ENUMS"
    );
}
