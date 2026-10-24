//! The base `OpenAPI` document: what the 27 operations do not contribute themselves.
//!
//! `api::app::router()` starts from `OpenApiRouter::with_openapi(ApiDoc::openapi())`, and every
//! handler it registers then adds its operation to a path item, with the request-body and
//! response schemas the handler declares. This module adds only the rest:
//!
//! - **`info`**: title, version and a one-line description, all fixed at compile time, so the
//!   emitted `modern/openapi/openapi.json` is the same bytes on every run and needs no
//!   environment (Generated API Types Only).
//! - **The security scheme [`BEARER_AUTH`]**: HTTP bearer with format `JWT`, the token
//!   `POST /api/v1/signon` issues. [`SecurityAddon`] declares it. No global security requirement
//!   is set: each handler's `#[utoipa::path(security(("bearer_auth" = [])))]` declares its own,
//!   and sign-on declares none.
//! - **Schemas no operation body references**: [`ScreenError`](crate::error::ScreenError),
//!   [`ScreenResult`](crate::dto::ScreenResult), [`PageDirection`](crate::dto::PageDirection)
//!   and the seven query DTOs. A handler takes a query as `params(T)`, and
//!   `IntoParams` emits parameters, not a `components.schemas` entry, so without this list the
//!   frontend's `components["schemas"]["CardListQuery"]` and the `PageDirection` reference of a
//!   query parameter would resolve to nothing. Registering a type also registers every schema it
//!   references (`TranListEntryKey`, for one), each under its own `ToSchema` name.
//!
//! The document lists no paths, so the operations are exactly those `router()` adds, and no
//! `servers`, so it points at no hosted service (Local-Only Runtime).
//!
//! # Built by the derive
//!
//! [`ApiDoc`] derives `utoipa::OpenApi`: its expansion compiles under the workspace's pedantic
//! lints with warnings denied, so the hand-built `OpenApiBuilder` fallback is not needed. A
//! caller brings the `utoipa::OpenApi` trait into scope to call `ApiDoc::openapi()`.
//!
//! # Redaction
//!
//! This module writes no example of its own, and every schema it registers documents protected
//! values only in redacted form: a card number as `************5740`, never a full number, and
//! no SSN, password or CVV property at all (Cardholder Data Redaction, AAP 0.3.4.8).

use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::openapi::{Components, OpenApi as Document};
use utoipa::{Modify, OpenApi};

/// The name of the bearer-token security scheme under `components.securitySchemes`, the name a
/// handler's `#[utoipa::path(security(("bearer_auth" = [])))]` refers to.
pub const BEARER_AUTH: &str = "bearer_auth";

/// The base document [`ApiDoc::openapi()`](OpenApi::openapi) returns: `info`, the
/// [`BEARER_AUTH`] scheme and the schemas no operation body references.
///
/// The version is the contract's, `1.0.0` for the `/api/v1` prefix, not the crate's: the member
/// manifests carry no version of their own.
#[derive(Debug, Clone, Copy, Default, OpenApi)]
#[openapi(
    info(
        title = "CardDemo API",
        version = "1.0.0",
        description = concat!(
            "REST replacement of the CardDemo online CICS transactions; every operation is under ",
            "/api/v1, and every one but POST /api/v1/signon takes the bearer JWT sign-on returns."
        )
    ),
    components(schemas(
        crate::error::ScreenError,
        crate::dto::ScreenResult,
        crate::dto::PageDirection,
        crate::dto::cocrdlic::CardListQuery,
        crate::dto::cocrdslc::CardLookupQuery,
        crate::dto::cotrn00c::TranListQuery,
        crate::dto::cousr00c::UserListQuery,
        crate::dto::copaus0c::AuthSummaryQuery,
        crate::dto::copaus1c::AuthDetailQuery,
        crate::dto::cotrtlic::TranTypeListQuery,
    )),
    modifiers(&SecurityAddon)
)]
pub struct ApiDoc;

/// Adds the [`BEARER_AUTH`] security scheme, HTTP `bearer` with bearer format `JWT`, to the
/// document's components, creating them when the document has none.
///
/// It adds no security requirement: each operation declares its own.
#[derive(Debug, Clone, Copy, Default)]
pub struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut Document) {
        openapi
            .components
            .get_or_insert_with(Components::new)
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;
    use std::fs;
    use std::path::Path;

    use serde_json::{Map, Value, json};
    use utoipa::openapi::OpenApiBuilder;
    use utoipa::{Modify, OpenApi};

    use super::{ApiDoc, BEARER_AUTH, SecurityAddon};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The schemas this module registers, by their `ToSchema` names.
    const REGISTERED: [&str; 10] = [
        "ScreenError",
        "ScreenResult",
        "PageDirection",
        "CardListQuery",
        "CardLookupQuery",
        "TranListQuery",
        "UserListQuery",
        "AuthSummaryQuery",
        "AuthDetailQuery",
        "TranTypeListQuery",
    ];

    /// The prefix of every schema reference.
    const SCHEMA_REF: &str = "#/components/schemas/";

    /// The base document as JSON.
    fn document() -> Result<Value, serde_json::Error> {
        serde_json::to_value(ApiDoc::openapi())
    }

    /// The `components.schemas` object of `doc`.
    fn schemas(doc: &Value) -> Result<&Map<String, Value>, &'static str> {
        doc.pointer("/components/schemas")
            .and_then(Value::as_object)
            .ok_or("the document has no components.schemas")
    }

    /// Every object key anywhere in `value`, and every `$ref` target.
    fn walk(value: &Value, keys: &mut BTreeSet<String>, refs: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                for (key, member) in map {
                    if key == "$ref" {
                        if let Some(target) = member.as_str() {
                            refs.insert(target.to_owned());
                        }
                    }
                    keys.insert(key.clone());
                    walk(member, keys, refs);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, keys, refs);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn the_bearer_scheme_is_http_bearer_with_jwt_format() -> TestResult {
        let doc = document()?;
        let schemes = doc
            .pointer("/components/securitySchemes")
            .and_then(Value::as_object)
            .ok_or("the document has no security schemes")?;
        assert_eq!(schemes.len(), 1);
        assert_eq!(
            schemes.get(BEARER_AUTH),
            Some(&json!({"type": "http", "scheme": "bearer", "bearerFormat": "JWT"}))
        );
        Ok(())
    }

    #[test]
    fn the_addon_creates_components_when_the_document_has_none() -> TestResult {
        let mut doc = OpenApiBuilder::new().build();
        assert!(doc.components.is_none());
        SecurityAddon.modify(&mut doc);
        let json = serde_json::to_value(&doc)?;
        assert_eq!(
            json.pointer("/components/securitySchemes/bearer_auth/scheme"),
            Some(&json!("bearer"))
        );
        Ok(())
    }

    #[test]
    fn every_unreferenced_schema_is_registered_under_its_own_name() -> TestResult {
        let doc = document()?;
        let schemas = schemas(&doc)?;
        for name in REGISTERED {
            assert!(schemas.contains_key(name), "{name} is not registered");
        }
        // Referenced by `TranListQuery` only, and registered with it.
        assert!(schemas.contains_key("TranListEntryKey"));
        Ok(())
    }

    #[test]
    fn every_schema_reference_resolves() -> TestResult {
        let doc = document()?;
        let schemas = schemas(&doc)?;
        let (mut keys, mut refs) = (BTreeSet::new(), BTreeSet::new());
        walk(&doc, &mut keys, &mut refs);
        assert!(refs.contains("#/components/schemas/PageDirection"));
        for target in &refs {
            let name = target
                .strip_prefix(SCHEMA_REF)
                .ok_or("a reference outside components.schemas")?;
            assert!(schemas.contains_key(name), "{target} resolves to nothing");
        }
        Ok(())
    }

    #[test]
    fn the_document_has_no_path_no_server_and_no_global_requirement() -> TestResult {
        let doc = document()?;
        assert_eq!(doc.get("paths"), Some(&json!({})));
        assert_eq!(doc.get("servers"), None);
        assert_eq!(doc.get("security"), None);
        Ok(())
    }

    #[test]
    fn the_info_is_fixed_at_compile_time() -> TestResult {
        let doc = document()?;
        assert_eq!(doc.pointer("/info/title"), Some(&json!("CardDemo API")));
        assert_eq!(doc.pointer("/info/version"), Some(&json!("1.0.0")));
        let description = doc
            .pointer("/info/description")
            .and_then(Value::as_str)
            .ok_or("the document has no description")?;
        assert!(description.contains("/api/v1"));
        assert!(!description.contains('\n'));
        assert_eq!(
            ApiDoc::openapi().to_pretty_json()?,
            ApiDoc::openapi().to_pretty_json()?
        );
        Ok(())
    }

    #[test]
    fn the_document_names_no_cvv_and_holds_no_fixture_card_number_or_ssn() -> TestResult {
        let doc = document()?;
        let (mut keys, mut refs) = (BTreeSet::new(), BTreeSet::new());
        walk(&doc, &mut keys, &mut refs);
        let cvv = keys
            .iter()
            .find(|key| key.to_ascii_lowercase().contains("cvv"));
        assert_eq!(cvv, None);

        let text = doc.to_string();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII");
        let cards = fs::read_to_string(fixtures.join("carddata.txt"))?;
        let customers = fs::read_to_string(fixtures.join("custdata.txt"))?;
        // `CARD-NUM PIC X(16)` opens `CVACT02Y`; `CUST-SSN PIC 9(09)` starts at offset 279 of
        // `CVCUS01Y`.
        let card_numbers: Vec<&str> = cards.lines().filter_map(|line| line.get(..16)).collect();
        let ssns: Vec<&str> = customers
            .lines()
            .filter_map(|line| line.get(279..288))
            .collect();
        assert_eq!((card_numbers.len(), ssns.len()), (50, 50));
        for value in card_numbers.iter().chain(&ssns) {
            assert!(!text.contains(value), "the document holds a fixture value");
        }
        for ssn in &ssns {
            let dashed = match (ssn.get(..3), ssn.get(3..5), ssn.get(5..)) {
                (Some(area), Some(group), Some(serial)) => format!("{area}-{group}-{serial}"),
                _ => return Err("an SSN shorter than nine digits".into()),
            };
            assert!(!text.contains(&dashed), "the document holds a fixture SSN");
        }
        Ok(())
    }
}
