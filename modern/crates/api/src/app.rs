//! The assembly of the `CardDemo` REST API: the state every handler receives, the router of the 27
//! operations under `/api/v1`, and the one `OpenAPI` document built from those same operations
//! (AAP 0.3.4.1).
//!
//! - [`AppState`] holds what the services reach: the [`PgStore`], their only path to
//!   `PostgreSQL` (R4), the [`JwtKeys`] that sign and verify the sign-on token, the injected
//!   [`Clock`], their only time source, and the job-deck directory of endpoint 14
//!   (`CARDDEMO_INTRDR_DIR`), the counterpart of CORPT00C's `JOBS` transient data queue.
//! - [`router`] registers every handler through `utoipa_axum::routes!` and splits the result into
//!   the axum router and the document, so each operation in the document is exactly one handler
//!   and each handler is exactly one operation. [`openapi`] is that document alone. Building it
//!   opens no connection and reads no environment, so `carddemo-openapi` and
//!   `scripts/regen-api.sh` run offline, and one build always emits the same bytes: the document
//!   is the frontend's only source of payload types (Generated API Types Only).
//! - [`app`] adds [`DOCUMENT_ROUTE`] after the split, so the document is served without being one
//!   of its own operations, wraps every route in the request trace layer and binds the state.
//!
//! # Request spans
//!
//! Every request, an unmatched one included, runs inside the `http.request` span of
//! `runtime::telemetry::request_span`. The span records the method and the `MatchedPath` route
//! template, such as `/api/v1/accounts/{acct_id}/view`, or `runtime::telemetry::UNMATCHED_ROUTE`
//! when no route matched; `runtime::telemetry::record_response` then adds the status and the
//! latency. The URI, the query string, the headers and the body are never recorded, because path
//! segments and query values carry account IDs and card numbers (Cardholder Data Redaction, AAP
//! 0.3.4.7, 0.3.4.8). tower-http's default request and failure events are kept: neither names
//! the URI.
//!
//! # What is deliberately absent
//!
//! - No CORS layer: the Vite dev server proxies `/api`, so the browser sees a single origin.
//! - No authentication middleware: the gate is the `SignedOnUser` or `AdminUser` extractor each
//!   handler declares, which is also what its operation's security requirement documents.
//! - No schema change and no broker: the schema is applied by `carddemo-migrate` alone, and no
//!   online transaction sends a message (Single Schema Executor, AAP 0.5.3).
//!
//! Sign-on (COSGN00C) is the one operation the token does not guard, since it issues the token,
//! and CORPT00C's job deck is the one file the API writes; both are services' concerns, reached
//! only through the handlers registered here.

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRef, MatchedPath};
use axum::http::{Request, Response};
use axum::routing::get;
use axum::{Json, Router};
use domain::clock::Clock;
use persistence::PgStore;
use runtime::config::{Config, EnvError};
use runtime::telemetry::{UNMATCHED_ROUTE, record_response, request_span};
use tower_http::trace::{MakeSpan, OnResponse, TraceLayer};
use tracing::Span;
use utoipa::OpenApi as _;
use utoipa::openapi::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::jwt::JwtKeys;
use crate::handlers;
use crate::openapi::ApiDoc;

/// Where [`app`] serves the `OpenAPI` document as JSON. It is added after the split, so it is not
/// an operation of the document it serves.
pub const DOCUMENT_ROUTE: &str = "/api-docs/openapi.json";

/// What every handler receives through `axum::extract::State`, and its extractors through
/// [`FromRef`].
///
/// Cloning is cheap: the store clones a pool handle, the keys and the clock an `Arc`.
#[derive(Clone)]
pub struct AppState {
    /// The executor of every repository trait, the services' only path to `PostgreSQL` (R4). A
    /// service calls the traits on it, or on the transaction `store.begin()` opens.
    pub store: PgStore,
    /// The HS256 keys that sign the sign-on token and verify every bearer token.
    pub jwt: JwtKeys,
    /// The only time source of the services: `SystemClock` in `carddemo-api`, `FixedClock` in
    /// tests (AAP 0.5.6).
    pub clock: Arc<dyn Clock>,
    /// `CARDDEMO_INTRDR_DIR`: where endpoint 14 writes each job deck as the next `TRNRPT00`
    /// generation.
    pub intrdr_dir: PathBuf,
}

impl AppState {
    /// The state over `store`, signing and verifying tokens with `jwt`, reading time from
    /// `clock` and writing job decks under `intrdr_dir`.
    #[must_use]
    pub fn new(store: PgStore, jwt: JwtKeys, clock: Arc<dyn Clock>, intrdr_dir: PathBuf) -> Self {
        Self {
            store,
            jwt,
            clock,
            intrdr_dir,
        }
    }

    /// The state of the `carddemo-api` process: keys from `config.jwt_secret` with lifetime
    /// `config.jwt_ttl_seconds`, and job decks under `config.intrdr_dir`. The environment is not
    /// read again.
    ///
    /// # Errors
    ///
    /// [`EnvError::MissingJwtSecret`] when `CARDDEMO_JWT_SECRET` is empty or unset: the API
    /// cannot sign a token, so it refuses to start (AAP 0.5.6).
    pub fn from_config(
        store: PgStore,
        config: &Config,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, EnvError> {
        let secret = config.require_jwt_secret()?;
        Ok(Self::new(
            store,
            JwtKeys::new(secret, config.jwt_ttl_seconds),
            clock,
            config.intrdr_dir.clone(),
        ))
    }
}

/// Names every field but shows only the job-deck directory: the keys are secret, and neither the
/// store nor the clock has anything a diagnostic needs.
impl fmt::Debug for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AppState")
            .field("store", &format_args!(".."))
            .field("jwt", &format_args!(".."))
            .field("clock", &format_args!(".."))
            .field("intrdr_dir", &self.intrdr_dir)
            .finish()
    }
}

/// The keys the `SignedOnUser` and `AdminUser` extractors verify a bearer token with.
impl FromRef<AppState> for JwtKeys {
    fn from_ref(state: &AppState) -> Self {
        state.jwt.clone()
    }
}

/// The router of the 27 operations, still awaiting its state, and the `OpenAPI` document they
/// make: the base document of [`ApiDoc`] with one path item per route and every schema the
/// handlers declare.
///
/// `routes!` registers each handler under the path its `#[utoipa::path]` declares, and gives one
/// method router to every path its handlers name, so the handlers of one path share one
/// `routes!` call and no call mixes paths. That makes 22 calls for the 27 operations. No route is
/// added outside `routes!` before the split, so the router serves exactly the document's
/// operations. The static segments `/cards/view`, `/cards/edit` and
/// `/transaction-types/list-actions` take precedence over the `{param}` segment beside them.
#[must_use = "the router serves nothing and the document is emitted nowhere until both are used"]
pub fn router() -> (Router<AppState>, OpenApi) {
    OpenApiRouter::<AppState>::with_openapi(ApiDoc::openapi())
        // 1: CC00 / COSGN00C.
        .routes(routes!(handlers::cosgn00c::signon))
        // 2: CM00 / COMEN01C.
        .routes(routes!(handlers::comen01c::main_menu))
        // 3: CA00 / COADM01C.
        .routes(routes!(handlers::coadm01c::admin_menu))
        // 4: CAVW / COACTVWC.
        .routes(routes!(handlers::coactvwc::view))
        // 5 and 6: CAUP / COACTUPC, fetch and save.
        .routes(routes!(handlers::coactupc::fetch, handlers::coactupc::save))
        // 7: CCLI / COCRDLIC.
        .routes(routes!(handlers::cocrdlic::list))
        // 8: CCDL / COCRDSLC.
        .routes(routes!(handlers::cocrdslc::view))
        // 9 and 10: CCUP / COCRDUPC, fetch and save, on two paths.
        .routes(routes!(handlers::cocrdupc::fetch))
        .routes(routes!(handlers::cocrdupc::save))
        // 11 and 13: CT00 / COTRN00C's list and CT02 / COTRN02C's add share a path.
        .routes(routes!(handlers::cotrn00c::list, handlers::cotrn02c::add))
        // 12: CT01 / COTRN01C.
        .routes(routes!(handlers::cotrn01c::view))
        // 14: CR00 / CORPT00C.
        .routes(routes!(handlers::corpt00c::submit))
        // 15: CB00 / COBIL00C.
        .routes(routes!(handlers::cobil00c::pay))
        // 16 and 17: CU00 / COUSR00C's list and CU01 / COUSR01C's add share a path.
        .routes(routes!(handlers::cousr00c::list, handlers::cousr01c::add))
        // 18 to 20: the user read COUSR02C and COUSR03C share, COUSR02C's update and COUSR03C's
        // delete.
        .routes(routes!(
            handlers::cousr02c::fetch,
            handlers::cousr02c::update,
            handlers::cousr03c::delete
        ))
        // 21: CPVS / COPAUS0C.
        .routes(routes!(handlers::copaus0c::summary))
        // 22 and 23: CPVD / COPAUS1C, the detail and the fraud toggle COPAUS2C performs.
        .routes(routes!(handlers::copaus1c::detail))
        .routes(routes!(handlers::copaus1c::toggle_fraud))
        // 24 and 25: CTLI / COTRTLIC, paging and the ENTER and F10 workflow.
        .routes(routes!(handlers::cotrtlic::list))
        .routes(routes!(handlers::cotrtlic::list_actions))
        // 26 and 27: CTTU / COTRTUPC, fetch and the save and delete actions.
        .routes(routes!(handlers::cotrtupc::fetch))
        .routes(routes!(handlers::cotrtupc::action))
        .split_for_parts()
}

/// The `OpenAPI` document of the 27 operations, as `carddemo-openapi` prints it and
/// [`DOCUMENT_ROUTE`] serves it. Building it opens no connection and reads no environment.
#[must_use]
pub fn openapi() -> OpenApi {
    router().1
}

/// The complete API: the operations of [`router`], the document at [`DOCUMENT_ROUTE`], one
/// `http.request` span per request (see the module documentation) and `state`.
#[must_use = "the API answers no request until the router is served"]
pub fn app(state: AppState) -> Router {
    let (router, document) = router();
    router
        .route(DOCUMENT_ROUTE, get(move || async move { Json(document) }))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(RouteSpan)
                .on_response(ResponseFields),
        )
        .with_state(state)
}

/// Makes the `http.request` span of a request from its method and its `MatchedPath` route
/// template alone. tower-http's `DefaultMakeSpan` is not used, because it records the URI.
#[derive(Debug, Clone, Copy, Default)]
struct RouteSpan;

impl<B> MakeSpan<B> for RouteSpan {
    fn make_span(&mut self, request: &Request<B>) -> Span {
        let route = request
            .extensions()
            .get::<MatchedPath>()
            .map_or(UNMATCHED_ROUTE, MatchedPath::as_str);
        request_span(request.method().as_str(), route)
    }
}

/// Records the status and the latency of a response on its request's span, and nothing else:
/// no header and no body.
#[derive(Debug, Clone, Copy, Default)]
struct ResponseFields;

impl<B> OnResponse<B> for ResponseFields {
    fn on_response(self, response: &Response<B>, latency: Duration, span: &Span) {
        record_response(span, response.status().as_u16(), latency);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;
    use std::path::PathBuf;
    use std::sync::Arc;

    use axum::body::{self, Body};
    use axum::extract::FromRef;
    use axum::http::{Request, StatusCode, header};
    use chrono::NaiveDate;
    use domain::clock::{Clock, FixedClock};
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use persistence::PgStore;
    use runtime::config::{Config, EnvError};
    use serde_json::Value;
    use test_support::postgres::{PostgresGuard, start_postgres};
    use test_support::trace_capture;
    use tower::ServiceExt as _;

    use super::{AppState, DOCUMENT_ROUTE, app, openapi};
    use crate::auth::jwt::JwtKeys;

    type TestResult = Result<(), Box<dyn StdError>>;

    const SECRET: &str = "app-unit-test-secret";

    /// The 27 operations of AAP 0.3.4.1 in endpoint order: method, path and the operation ID
    /// `<module>_<handler>` of the handler registered for them.
    const OPERATIONS: [(&str, &str, &str); 27] = [
        ("post", "/api/v1/signon", "cosgn00c_signon"),
        ("get", "/api/v1/menus/main", "comen01c_main_menu"),
        ("get", "/api/v1/menus/admin", "coadm01c_admin_menu"),
        ("get", "/api/v1/accounts/{acct_id}/view", "coactvwc_view"),
        ("get", "/api/v1/accounts/{acct_id}", "coactupc_fetch"),
        ("put", "/api/v1/accounts/{acct_id}", "coactupc_save"),
        ("get", "/api/v1/cards", "cocrdlic_list"),
        ("get", "/api/v1/cards/view", "cocrdslc_view"),
        ("get", "/api/v1/cards/edit", "cocrdupc_fetch"),
        ("put", "/api/v1/cards/{card_ref}", "cocrdupc_save"),
        ("get", "/api/v1/transactions", "cotrn00c_list"),
        ("get", "/api/v1/transactions/{tran_id}", "cotrn01c_view"),
        ("post", "/api/v1/transactions", "cotrn02c_add"),
        ("post", "/api/v1/reports/transactions", "corpt00c_submit"),
        ("post", "/api/v1/bill-payments", "cobil00c_pay"),
        ("get", "/api/v1/users", "cousr00c_list"),
        ("post", "/api/v1/users", "cousr01c_add"),
        ("get", "/api/v1/users/{user_id}", "cousr02c_fetch"),
        ("put", "/api/v1/users/{user_id}", "cousr02c_update"),
        ("delete", "/api/v1/users/{user_id}", "cousr03c_delete"),
        (
            "get",
            "/api/v1/accounts/{acct_id}/authorizations",
            "copaus0c_summary",
        ),
        (
            "get",
            "/api/v1/authorizations/{acct_id}/{auth_key}",
            "copaus1c_detail",
        ),
        (
            "post",
            "/api/v1/authorizations/{acct_id}/{auth_key}/fraud",
            "copaus1c_toggle_fraud",
        ),
        ("get", "/api/v1/transaction-types", "cotrtlic_list"),
        (
            "post",
            "/api/v1/transaction-types/list-actions",
            "cotrtlic_list_actions",
        ),
        (
            "get",
            "/api/v1/transaction-types/{type_code}",
            "cotrtupc_fetch",
        ),
        (
            "post",
            "/api/v1/transaction-types/{type_code}/actions",
            "cotrtupc_action",
        ),
    ];

    /// The keys of a path item that name an operation.
    const METHODS: [&str; 8] = [
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];

    /// Every operation of the document: method, path, operation ID and whether it declares a
    /// security requirement.
    fn operations(doc: &Value) -> Result<Vec<(String, String, String, bool)>, String> {
        let paths = doc
            .get("paths")
            .and_then(Value::as_object)
            .ok_or("the document has no paths object")?;
        let mut found = Vec::new();
        for (path, item) in paths {
            let item = item
                .as_object()
                .ok_or_else(|| format!("{path} is not an object"))?;
            for (method, operation) in item {
                if !METHODS.contains(&method.as_str()) {
                    continue;
                }
                let id = operation
                    .get("operationId")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{method} {path} has no operation ID"))?;
                found.push((
                    method.clone(),
                    path.clone(),
                    id.to_owned(),
                    operation.get("security").is_some(),
                ));
            }
        }
        Ok(found)
    }

    #[test]
    fn the_document_holds_exactly_the_27_operations_on_22_paths() -> TestResult {
        let doc = openapi();
        assert_eq!(doc.paths.paths.len(), 22);
        assert!(!doc.paths.paths.contains_key(DOCUMENT_ROUTE));

        let found = operations(&serde_json::to_value(&doc)?)?;
        assert_eq!(found.len(), 27);
        let found_set: BTreeSet<(&str, &str, &str)> = found
            .iter()
            .map(|(method, path, id, _)| (method.as_str(), path.as_str(), id.as_str()))
            .collect();
        let expected: BTreeSet<(&str, &str, &str)> = OPERATIONS.into_iter().collect();
        assert_eq!(found_set, expected);

        // Sign-on issues the token, so it alone declares no bearer requirement.
        for (method, path, _, secured) in &found {
            assert_eq!(*secured, path != "/api/v1/signon", "{method} {path}");
        }
        Ok(())
    }

    #[test]
    fn the_document_is_the_same_bytes_on_every_build() -> TestResult {
        let first = openapi().to_pretty_json()?;
        assert_eq!(first, openapi().to_pretty_json()?);
        assert_eq!(
            serde_json::from_str::<Value>(&first)?.pointer("/info/title"),
            Some(&Value::from("CardDemo API"))
        );
        Ok(())
    }

    /// A state over a closed pool, so that a request that reached the store would fail; every
    /// request below is answered before any query.
    async fn closed_store() -> Result<(PgStore, PostgresGuard), Box<dyn StdError>> {
        let guard = start_postgres().await?;
        guard.pool().close().await;
        Ok((PgStore::new(guard.pool().clone()), guard))
    }

    /// The fixed instant of the test clock.
    fn clock() -> Result<Arc<dyn Clock>, Box<dyn StdError>> {
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_opt(10, 30, 0))
            .ok_or("an invalid test instant")?;
        Ok(Arc::new(FixedClock::new(at)))
    }

    /// A bearer token for `USER0001`, a user of type `U`, signed with `keys`.
    fn user_token(keys: &JwtKeys) -> Result<String, Box<dyn StdError>> {
        Ok(keys.issue(&PicX::from_text("USER0001")?, &UserType::USER)?)
    }

    /// The status of `request` sent to `app(state)`, with the body read to its end so that the
    /// request's span closes.
    async fn send(
        state: &AppState,
        request: Request<Body>,
    ) -> Result<(StatusCode, Vec<u8>), Box<dyn StdError>> {
        let response = app(state.clone()).oneshot(request).await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, bytes.to_vec()))
    }

    /// A `GET` of `uri`, with `token` as bearer token when one is given.
    fn get(uri: &str, token: Option<&str>) -> Result<Request<Body>, axum::http::Error> {
        let builder = Request::get(uri);
        let builder = match token {
            Some(token) => builder.header(header::AUTHORIZATION, format!("Bearer {token}")),
            None => builder,
        };
        builder.body(Body::empty())
    }

    #[tokio::test]
    async fn the_state_comes_from_the_configuration_and_shows_no_secret() -> TestResult {
        let (store, _guard) = closed_store().await?;

        let unset = AppState::from_config(store.clone(), &Config::default(), clock()?);
        assert_eq!(unset.err(), Some(EnvError::MissingJwtSecret));

        let config = Config {
            jwt_secret: SECRET.to_owned(),
            jwt_ttl_seconds: 120,
            intrdr_dir: PathBuf::from("var/test-jobs"),
            ..Config::default()
        };
        let state = AppState::from_config(store, &config, clock()?)?;
        assert_eq!(state.intrdr_dir, PathBuf::from("var/test-jobs"));
        assert_eq!(JwtKeys::from_ref(&state).ttl_seconds(), 120);

        let shown = format!("{state:?}");
        assert_eq!(
            shown,
            r#"AppState { store: .., jwt: .., clock: .., intrdr_dir: "var/test-jobs" }"#
        );
        assert!(!shown.contains(SECRET));

        // The extractors verify with the configured secret: a token of these keys is accepted,
        // one of another secret is refused before the handler runs.
        let token = user_token(&JwtKeys::new(SECRET, 60))?;
        let (status, _) = send(&state, get("/api/v1/menus/main", Some(&token))?).await?;
        assert_eq!(status, StatusCode::OK);
        let foreign = user_token(&JwtKeys::new("another-secret", 60))?;
        let (status, _) = send(&state, get("/api/v1/menus/main", Some(&foreign))?).await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        // A `U` token reaches no admin operation.
        let (status, _) = send(&state, get("/api/v1/menus/admin", Some(&token))?).await?;
        assert_eq!(status, StatusCode::FORBIDDEN);
        Ok(())
    }

    #[tokio::test]
    async fn the_document_is_served_and_spans_carry_route_templates_only() -> TestResult {
        let (store, _guard) = closed_store().await?;
        let state = AppState::new(
            store,
            JwtKeys::new(SECRET, 3600),
            clock()?,
            PathBuf::from("var/jobs"),
        );

        let traces = trace_capture::capture();

        let (status, bytes) = send(&state, get(DOCUMENT_ROUTE, None)?).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            serde_json::to_value(openapi())?
        );
        assert!(traces.contains(r#""route":"/api-docs/openapi.json""#));

        // No token: the extractor answers 401 before the path or the store is read.
        let (status, _) = send(&state, get("/api/v1/accounts/00000000050/view", None)?).await?;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert!(traces.contains(r#""route":"/api/v1/accounts/{acct_id}/view""#));
        assert!(traces.contains(r#""method":"GET""#));
        assert!(traces.contains(r#""status":401"#));
        assert!(traces.contains(r#""latency_ms":"#));

        let (status, _) = send(
            &state,
            get(
                "/api/v1/no-such-route/0500024453765740?card=0500024453765740",
                None,
            )?,
        )
        .await?;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(traces.contains(r#""route":"<unmatched>""#));
        assert!(traces.contains(r#""status":404"#));

        let text = traces.text();
        for withheld in ["00000000050", "0500024453765740", "no-such-route", "card="] {
            assert!(
                !text.contains(withheld),
                "a span or event recorded {withheld}"
            );
        }
        Ok(())
    }
}
