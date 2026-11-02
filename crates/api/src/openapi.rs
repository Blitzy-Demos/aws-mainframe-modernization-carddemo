//! The `OpenAPI` 3.1 document of the REST API, built from the route registry
//! the server runs.
//!
//! # Pipeline
//!
//! 1. [`spec`] merges this module's document root (title, version, tags, the
//!    bearer security scheme and the schemas no path refers to) with
//!    `crate::handlers::router()`, the same `OpenApiRouter` whose axum half
//!    `crate::app` serves. A route therefore cannot be served without being
//!    documented, or documented without being served.
//! 2. `cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json`
//!    writes [`spec_json`] to the committed `web/openapi/openapi.json`.
//! 3. `pnpm --dir web gen:api` turns that file into `web/src/api/schema.d.ts`,
//!    the SPA's only view of the wire types. Both generated files are
//!    committed.
//! 4. `crates/api/tests/openapi_spec.rs` regenerates [`spec_json`] in memory
//!    and fails on any byte difference from the committed file, so a handler
//!    or DTO change that is not regenerated fails the build.
//!
//! # Byte determinism
//!
//! Step 4 compares bytes, so the document is a function of the source alone:
//!
//! - utoipa keeps paths, component schemas, security schemes and schema
//!   properties in `BTreeMap`s, which serialize in key order. Its
//!   `preserve_order` and `preserve_path_order` features, which would switch
//!   them to insertion order, stay disabled.
//! - Tags keep the fixed order declared here; operations keep the order the
//!   registry declares them in.
//! - Nothing reads the environment, the clock or a random source at run time.
//!   The version is the package version, fixed when the crate compiles, and
//!   there is no `servers` entry: the SPA calls through the Vite `/api` proxy,
//!   which strips the prefix, and the routes are served at their root paths.
//!
//! # Schemas no path refers to
//!
//! An error body's `fields[].field` is a plain string, so the `<Dto>Field`
//! enums that list each request DTO's legal BMS field names are reached by no
//! operation. They are registered here explicitly; without them
//! `web/src/schemas/*` could not type its `bmsFieldMap` against
//! `components["schemas"]["<Dto>Field"]`.

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{self, Components};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;

use crate::dto::accounts::{AccountUpdateField, AccountViewField};
use crate::dto::admin_reward_rules::RewardRuleField;
use crate::dto::admin_transaction_types::{TranTypeListField, TranTypeUpsertField};
use crate::dto::admin_users::{UserAddField, UserDeleteField, UserListField, UserUpdateField};
use crate::dto::auth::LoginField;
use crate::dto::authorizations::{AuthDetailField, AuthSummaryField};
use crate::dto::cards::{CardListField, CardUpdateField, CardViewField};
use crate::dto::payments::BillPaymentField;
use crate::dto::reports::ReportRequestField;
use crate::dto::transactions::{TransactionAddField, TransactionListField, TransactionViewField};
use crate::error::{ErrorBody, ErrorFieldBody};
use crate::handlers;
use crate::state::AppState;

/// The name of the bearer security scheme. Every operation except
/// `POST /auth/login` and `POST /auth/logout` names it in
/// `#[utoipa::path(security(("bearer_auth" = [])))]`; the attribute takes
/// only a literal, so the handlers spell the name out.
const BEARER_AUTH: &str = "bearer_auth";

/// The document root the registry's paths are merged into.
///
/// `info.version` is left to utoipa's default, the package version
/// (`CARGO_PKG_VERSION`, the workspace version), and `info.license` to the
/// package licence. Each tag names one handler module: first the user
/// functions in the order of the online-transaction table
/// [README.md:L271-294], then the rewards resources, then the admin
/// functions. Each handler's `#[utoipa::path(tag = ..)]` uses one of these
/// names.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "CardDemo API",
        description = "REST replacement of the CardDemo CICS online transactions \
            (sign-on, accounts, cards, transactions, reports, bill payment, pending \
            authorizations, user and transaction-type administration) plus the rewards \
            system. Every operation except POST /auth/login and POST /auth/logout needs \
            a bearer token from POST /auth/login; every /admin operation needs the token \
            of an admin user (usr_type A)."
    ),
    modifiers(&SecurityAddon),
    components(schemas(
        ErrorBody,
        ErrorFieldBody,
        LoginField,
        AccountViewField,
        AccountUpdateField,
        CardListField,
        CardViewField,
        CardUpdateField,
        TransactionListField,
        TransactionViewField,
        TransactionAddField,
        ReportRequestField,
        BillPaymentField,
        AuthSummaryField,
        AuthDetailField,
        UserListField,
        UserAddField,
        UserUpdateField,
        UserDeleteField,
        TranTypeListField,
        TranTypeUpsertField,
        RewardRuleField,
    )),
    tags(
        (name = "auth", description = "CC00 (COSGN00C): sign-on and sign-off"),
        (name = "accounts", description = "CAVW (COACTVWC), CAUP (COACTUPC): account view \
            and update"),
        (name = "cards", description = "CCLI (COCRDLIC), CCDL (COCRDSLC), CCUP (COCRDUPC): \
            card list, view and update"),
        (name = "transactions", description = "CT00 (COTRN00C), CT01 (COTRN01C), CT02 \
            (COTRN02C): transaction list, view and add"),
        (name = "reports", description = "CR00 (CORPT00C): transaction reports"),
        (name = "payments", description = "CB00 (COBIL00C): bill payment"),
        (name = "authorizations", description = "CPVS (COPAUS0C), CPVD (COPAUS1C with \
            COPAUS2C): pending authorization summary, detail and fraud marking"),
        (name = "rewards", description = "Reward balance and accrual history per account"),
        (name = "admin-users", description = "CU00-CU03 (COUSR00C-COUSR03C): user list, \
            add, update and delete; admin only"),
        (name = "admin-transaction-types", description = "CTLI (COTRTLIC), CTTU \
            (COTRTUPC): transaction type list, add, update and delete; admin only"),
        (name = "admin-reward-rules", description = "Reward rule list, add, update and \
            soft delete; admin only"),
    )
)]
struct ApiDoc;

/// Registers the HTTP bearer scheme the protected operations name: a JWT
/// that `POST /auth/login` issues, sent as `Authorization: Bearer <token>`.
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, doc: &mut openapi::OpenApi) {
        doc.components
            .get_or_insert_with(Components::default)
            .add_security_scheme(
                BEARER_AUTH,
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .build(),
                ),
            );
    }
}

/// The `OpenAPI` document of every route the server serves.
///
/// The paths and the schemas they reach come from `crate::handlers::router()`,
/// the registry `crate::app` serves; the title, version, tags, security scheme
/// and the `<Dto>Field` enums come from this module. Building it opens no
/// connection and reads no settings: the router is never served here.
#[must_use]
pub fn spec() -> openapi::OpenApi {
    OpenApiRouter::<AppState>::with_openapi(ApiDoc::openapi())
        .merge(handlers::router())
        .split_for_parts()
        .1
}

/// [`spec`] as the exact bytes of the committed `web/openapi/openapi.json`:
/// serde's pretty JSON (two-space indentation, LF line ends) followed by one
/// newline.
///
/// Byte-deterministic: two calls, in one process or in two builds of the same
/// source, return the same string.
///
/// # Panics
///
/// Panics if the document does not serialize. It is an in-memory tree of
/// strings, numbers and string-keyed maps built from derived schemas, which
/// `serde_json` always serializes, so a panic means a schema implementation
/// broke that contract.
#[must_use]
pub fn spec_json() -> String {
    let mut json = spec()
        .to_pretty_json()
        .expect("the OpenAPI document serializes to JSON");
    json.push('\n');
    json
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use serde_json::{Value, json};

    use super::{BEARER_AUTH, spec, spec_json};
    use crate::handlers;

    /// The tags, in the order the document declares them.
    const TAGS: [&str; 11] = [
        "auth",
        "accounts",
        "cards",
        "transactions",
        "reports",
        "payments",
        "authorizations",
        "rewards",
        "admin-users",
        "admin-transaction-types",
        "admin-reward-rules",
    ];

    /// The `<Dto>Field` enums `crate::dto` defines, each with one BMS field
    /// name it must list.
    const FIELD_ENUMS: [(&str, &str); 20] = [
        ("LoginField", "USERID"),
        ("AccountViewField", "ACCTSID"),
        ("AccountUpdateField", "ACCTSID"),
        ("CardListField", "ACCTSID"),
        ("CardViewField", "CARDSID"),
        ("CardUpdateField", "CRDNAME"),
        ("TransactionListField", "TRNIDIN"),
        ("TransactionViewField", "TRNIDIN"),
        ("TransactionAddField", "ACTIDIN"),
        ("ReportRequestField", "CONFIRM"),
        ("BillPaymentField", "ACTIDIN"),
        ("AuthSummaryField", "ACCTID"),
        ("AuthDetailField", "CARDNUM"),
        ("UserListField", "USRIDIN"),
        ("UserAddField", "USERID"),
        ("UserUpdateField", "USRIDIN"),
        ("UserDeleteField", "USRIDIN"),
        ("TranTypeListField", "TRTYPE"),
        ("TranTypeUpsertField", "TRTYPCD"),
        ("RewardRuleField", "reward_rate"),
    ];

    const HTTP_METHODS: [&str; 8] = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];

    fn document() -> Value {
        serde_json::from_str(&spec_json()).expect("spec_json is JSON")
    }

    /// Every operation object of the document, with its path and method.
    fn operations(document: &Value) -> Vec<(String, &'static str, Value)> {
        let paths = document["paths"].as_object().expect("paths is an object");
        paths
            .iter()
            .flat_map(|(path, item)| {
                HTTP_METHODS.iter().filter_map(move |method| {
                    item.get(*method)
                        .map(|operation| (path.clone(), *method, operation.clone()))
                })
            })
            .collect()
    }

    #[test]
    fn spec_json_is_byte_deterministic_and_ends_with_one_newline() {
        let first = spec_json();
        let second = spec_json();
        assert_eq!(first, second, "two calls give the same bytes");
        assert!(
            first.ends_with("}\n"),
            "one trailing newline after the root"
        );
        assert!(!first.ends_with("\n\n"), "exactly one trailing newline");
        assert!(!first.contains('\r'), "LF line ends only");
        assert!(
            first.starts_with("{\n  \""),
            "serde's two-space pretty form"
        );
    }

    #[test]
    fn spec_json_is_the_pretty_form_of_spec() {
        let pretty = spec().to_pretty_json().expect("the document serializes");
        assert_eq!(spec_json(), format!("{pretty}\n"));
        assert_eq!(
            document(),
            serde_json::to_value(spec()).expect("the document serializes")
        );
    }

    #[test]
    fn info_is_the_package_and_there_is_no_server_entry() {
        let document = document();
        assert_eq!(document["openapi"], "3.1.0");
        assert_eq!(document["info"]["title"], "CardDemo API");
        assert_eq!(document["info"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            document["info"]["license"]["name"],
            env!("CARGO_PKG_LICENSE")
        );
        let description = document["info"]["description"]
            .as_str()
            .expect("a description");
        assert!(description.contains("CICS"), "{description}");
        assert!(description.contains("rewards"), "{description}");
        assert!(document.get("servers").is_none(), "no servers entry");
    }

    #[test]
    fn bearer_scheme_is_an_http_jwt_bearer() {
        let document = document();
        assert_eq!(
            document["components"]["securitySchemes"],
            json!({
                BEARER_AUTH: { "type": "http", "scheme": "bearer", "bearerFormat": "JWT" }
            })
        );
    }

    #[test]
    fn every_security_requirement_names_the_declared_scheme() {
        let document = document();
        let declared: BTreeSet<&str> = document["components"]["securitySchemes"]
            .as_object()
            .expect("security schemes")
            .keys()
            .map(String::as_str)
            .collect();
        let mut protected = 0_usize;
        for (path, method, operation) in operations(&document) {
            let Some(requirements) = operation.get("security") else {
                assert!(path.starts_with("/auth/"), "{method} {path} is public");
                continue;
            };
            protected += 1;
            for requirement in requirements.as_array().expect("a requirement list") {
                for name in requirement.as_object().expect("a requirement").keys() {
                    assert!(declared.contains(name.as_str()), "{method} {path}: {name}");
                }
            }
        }
        assert_eq!(protected, 28, "every operation but sign-on and sign-off");
    }

    #[test]
    fn error_body_and_every_field_enum_are_components() {
        let document = document();
        let schemas = &document["components"]["schemas"];

        let error = &schemas["ErrorBody"]["properties"];
        for property in ["code", "message", "fields"] {
            assert!(error.get(property).is_some(), "ErrorBody.{property}");
        }
        let field = &schemas["ErrorFieldBody"]["properties"];
        for property in ["field", "message"] {
            assert!(field.get(property).is_some(), "ErrorFieldBody.{property}");
        }

        for (name, member) in FIELD_ENUMS {
            let schema = &schemas[name];
            assert_eq!(schema["type"], "string", "{name}: {schema}");
            let values = schema["enum"].as_array().expect("an enum list");
            assert!(!values.is_empty(), "{name}");
            assert!(values.iter().all(Value::is_string), "{name}: {schema}");
            assert!(
                values.contains(&json!(member)),
                "{name} lists {member}: {schema}"
            );
        }
        let names: BTreeSet<&str> = FIELD_ENUMS.iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names.len(),
            FIELD_ENUMS.len(),
            "the enum names are distinct"
        );
    }

    #[test]
    fn tags_are_declared_in_order_and_every_operation_uses_one() {
        let document = document();
        let declared: Vec<&str> = document["tags"]
            .as_array()
            .expect("a tag list")
            .iter()
            .map(|tag| tag["name"].as_str().expect("a tag name"))
            .collect();
        assert_eq!(declared, TAGS);
        for tag in document["tags"].as_array().expect("a tag list") {
            assert!(tag["description"].is_string(), "{tag}");
        }

        let mut used = BTreeSet::new();
        for (path, method, operation) in operations(&document) {
            let tags = operation["tags"].as_array().expect("operation tags");
            assert_eq!(tags.len(), 1, "{method} {path}: {tags:?}");
            let tag = tags[0].as_str().expect("a tag name");
            assert!(TAGS.contains(&tag), "{method} {path}: {tag}");
            used.insert(tag.to_owned());
        }
        assert_eq!(used.len(), TAGS.len(), "every declared tag is used");
    }

    #[test]
    fn document_describes_exactly_the_registry_paths() {
        let (_router, registry) = handlers::router().split_for_parts();
        let registry = serde_json::to_value(registry.paths).expect("the registry paths serialize");
        assert_eq!(document()["paths"], registry);
    }

    #[test]
    fn every_path_uses_axum_brace_captures() {
        let document = document();
        let paths = document["paths"].as_object().expect("paths is an object");
        assert_eq!(paths.len(), 19);
        for path in paths.keys() {
            assert!(path.starts_with('/'), "{path} is a root path");
            assert!(!path.starts_with("/api/"), "{path} has no /api prefix");
            for segment in path.split('/').skip(1) {
                assert!(!segment.starts_with(':'), "{path} has no colon capture");
                assert!(!segment.is_empty(), "{path} has no empty segment");
                if let Some(name) = segment.strip_prefix('{') {
                    let name = name.strip_suffix('}').expect("a closed capture");
                    assert!(
                        !name.is_empty()
                            && name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'),
                        "{path}: capture {name:?}"
                    );
                } else {
                    assert!(!segment.contains(['{', '}', ':']), "{path}: {segment}");
                }
            }
        }
    }
}
