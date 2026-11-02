//! `carddemo-api`: the REST API that replaces the `CardDemo` CICS online
//! transactions [README.md:L271-294]. It is an axum 0.8 and utoipa 5 adapter
//! over the ported COBOL rules in `carddemo_domain::rules`, which reach
//! `PostgreSQL` only through the repositories and units of work of
//! `carddemo_persistence`.
//!
//! The CICS region that served those transactions is the `CARDDEMO` CSD group,
//! one `DEFINE TRANSACTION` per transaction code
//! [app/csd/CARDDEMO.CSD:L306-487]. Here the region is [`app`]: the router that
//! serves every route of [`handlers::router()`] over one shared [`AppState`].
//! Five online transactions have no route: the CM00 and CA00 menus became the
//! SPA's sidebar, and the MQ-triggered CP00, CDRD and CDRA are the consumers of
//! `carddemo-messaging`.
//!
//! # Modules
//!
//! - [`handlers`]: one module per resource family, and [`handlers::router()`],
//!   the only route registry.
//! - [`dto`]: every type that crosses the HTTP boundary. `carddemo_domain`
//!   names no serialization crate, so each request, response and query type is
//!   a DTO here that converts to and from the domain's rule inputs and entities.
//! - [`auth`]: the bearer token that stands for the signed-on user, and the
//!   [`auth::AuthUser`] and [`auth::AdminUser`] extractors that verify it.
//! - [`error`]: the only place a domain error kind becomes an HTTP status, and
//!   the only builder of the `{code, message, fields}` error body.
//! - [`pagination`]: the query every list endpoint takes and the page it
//!   returns.
//! - [`state`]: [`AppState`], built once per process from the database handle
//!   and the settings.
//! - [`openapi`]: the `OpenAPI` document of the registry, [`spec`] and its
//!   committed byte form [`spec_json`].
//!
//! # Import boundary
//!
//! The crate names three workspace crates only: `carddemo_domain`,
//! `carddemo_persistence` and `carddemo_runtime`. It never names `sqlx`,
//! `carddemo_codec`, `carddemo_batch` or `carddemo_messaging`; the manifest
//! scan of `crates/testkit/tests/hygiene.rs` fails the build if one appears.
//!
//! # Serving
//!
//! Routes are served at their root paths (`/auth/login`, `/accounts/{id}`, …),
//! with no `/api` prefix: the SPA's Vite dev server maps `/api/*` to
//! `CARDDEMO_API_PROXY_TARGET` and strips the prefix. The `api` binary passes
//! [`app`] to `axum::serve`; the `openapi` binary writes [`spec_json`] to
//! `web/openapi/openapi.json`.

pub mod auth;
pub mod dto;
pub mod error;
pub mod handlers;
pub mod openapi;
pub mod pagination;
pub mod state;

pub use openapi::{spec, spec_json};
pub use state::{AppState, StateError};

use axum::Router;
use axum::http::{HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

/// The HTTP server: every route of [`handlers::router()`] with `state`, behind
/// the request trace layer and the CORS layer.
///
/// The routes come from the same registry [`spec`] documents, so the server
/// serves exactly the documented operations. A path the registry does not hold
/// gets axum's empty 404, and a method a path does not serve gets 405: there is
/// no fallback handler and no route for the document itself.
///
/// Building the router opens no connection and reads no environment variable:
/// everything it needs arrives in `state`, already validated by
/// [`AppState::new`].
///
/// # Trace layer
///
/// Each request gets the `tower-http` `request` span (method, URI, HTTP
/// version) and, when the response is ready, a `finished processing request`
/// event with the status and latency. The handler spans, with `transaction`,
/// `user_id`, the resource keys and `outcome`, open inside it. Both the span
/// and the event are at INFO, not `tower-http`'s default DEBUG, because the
/// default `RUST_LOG=info` filter would otherwise drop them, and with them the
/// request span every API write must emit as a JSON line through
/// `carddemo_runtime::telemetry`. Request and response headers are not
/// recorded, so a bearer token never reaches the logs.
///
/// # CORS layer
///
/// The outermost layer admits one browser origin, [`AppState::cors_origin`]
/// (`CARDDEMO_CORS_ORIGIN`, by default the Vite dev server
/// `http://localhost:5173`), for `GET`, `POST`, `PUT` and `DELETE` with the
/// `Authorization` and `Content-Type` request headers. It answers preflight
/// requests itself.
// A reason string rather than a bare `#[must_use]`: `Router` is already
// `#[must_use]`, and clippy's `double_must_use` (denied) rejects a bare one here.
#[must_use = "the router answers no request until it is served, for example by `axum::serve`"]
pub fn app(state: AppState) -> Router {
    let (router, _doc) = handlers::router().split_for_parts();
    let cors_origin = state.cors_origin().clone();
    with_layers(router.with_state(state), cors_origin)
}

/// Wraps `router` in the trace layer and then the CORS layer of [`app`].
///
/// Kept apart from [`app`] so the layers are tested over a stand-in router,
/// without the database handle an [`AppState`] holds.
fn with_layers(router: Router, cors_origin: HeaderValue) -> Router {
    router
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(cors_layer(cors_origin))
}

/// The CORS policy of [`app`] for the one admitted `origin`.
///
/// The origin is a one-entry list rather than an exact value: `tower-http`
/// sends an exact value to every requester, whereas a list names the origin
/// only to the requester it matches. A response to any other origin therefore
/// carries no `Access-Control-Allow-Origin`, so the browser refuses it and the
/// admitted origin is not disclosed.
fn cors_layer(origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::list([origin]))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex, PoisonError};

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
    use axum::routing::get;
    use tower::ServiceExt;
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::with_layers;

    /// The admitted origin: the default `CARDDEMO_CORS_ORIGIN`.
    const ORIGIN: &str = "http://localhost:5173";

    /// A stand-in for the registry: one path with a GET and a PUT, as
    /// `/accounts/{id}` has.
    fn layered() -> Router {
        let routes = Router::new().route(
            "/accounts/{id}",
            get(|| async { "account" }).put(|| async { "updated" }),
        );
        with_layers(routes, HeaderValue::from_static(ORIGIN))
    }

    fn request(method: Method, uri: &str, headers: &[(HeaderName, &str)]) -> Request<Body> {
        headers
            .iter()
            .fold(
                Request::builder().method(method).uri(uri),
                |builder, (name, value)| builder.header(name, *value),
            )
            .body(Body::empty())
            .expect("a valid request")
    }

    fn preflight(origin: &str) -> Request<Body> {
        request(
            Method::OPTIONS,
            "/accounts/00000000001",
            &[
                (header::ORIGIN, origin),
                (header::ACCESS_CONTROL_REQUEST_METHOD, "PUT"),
                (
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization,content-type",
                ),
            ],
        )
    }

    async fn send(request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        let response = layered()
            .oneshot(request)
            .await
            .expect("the router is infallible");
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is readable");
        (status, headers, body.to_vec())
    }

    /// The comma-separated values of every `name` header, lower-cased.
    fn values(headers: &HeaderMap, name: &HeaderName) -> BTreeSet<String> {
        headers
            .get_all(name)
            .iter()
            .flat_map(|value| value.to_str().expect("an ASCII header").split(','))
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| !value.is_empty())
            .collect()
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[tokio::test]
    async fn preflight_from_the_admitted_origin_gets_the_allow_headers() {
        let (status, headers, body) = send(preflight(ORIGIN)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(body.is_empty());
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            Some(&HeaderValue::from_static(ORIGIN))
        );
        assert_eq!(
            values(&headers, &header::ACCESS_CONTROL_ALLOW_METHODS),
            set(&["delete", "get", "post", "put"])
        );
        assert_eq!(
            values(&headers, &header::ACCESS_CONTROL_ALLOW_HEADERS),
            set(&["authorization", "content-type"])
        );
        assert!(
            values(&headers, &header::VARY).contains("origin"),
            "the answer depends on the origin, so caches must key on it"
        );
        assert!(
            headers
                .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .is_none()
        );
    }

    #[tokio::test]
    async fn only_the_admitted_origin_is_named_in_the_allow_origin_header() {
        for (origin, admitted) in [
            (ORIGIN, true),
            ("http://localhost:5174", false),
            ("https://localhost:5173", false),
            ("http://127.0.0.1:5173", false),
            ("http://localhost:5173.example.com", false),
            ("null", false),
        ] {
            let expected = admitted.then(|| HeaderValue::from_static(ORIGIN));

            let (_, headers, _) = send(preflight(origin)).await;
            assert_eq!(
                headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                expected.as_ref(),
                "preflight from {origin}"
            );

            let (status, headers, body) = send(request(
                Method::GET,
                "/accounts/00000000001",
                &[(header::ORIGIN, origin)],
            ))
            .await;
            assert_eq!(status, StatusCode::OK, "GET from {origin}");
            assert_eq!(body, b"account", "GET from {origin}");
            assert_eq!(
                headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                expected.as_ref(),
                "GET from {origin}"
            );
        }
    }

    #[tokio::test]
    async fn layers_leave_unknown_paths_and_methods_to_axum() {
        let cases = [
            (Method::GET, "/no-such-path", StatusCode::NOT_FOUND),
            (
                Method::GET,
                "/api/accounts/00000000001",
                StatusCode::NOT_FOUND,
            ),
            (
                Method::DELETE,
                "/accounts/00000000001",
                StatusCode::METHOD_NOT_ALLOWED,
            ),
        ];
        for (method, uri, expected) in cases {
            let (status, headers, body) =
                send(request(method.clone(), uri, &[(header::ORIGIN, ORIGIN)])).await;
            assert_eq!(status, expected, "{method} {uri}");
            assert!(body.is_empty(), "{method} {uri}");
            assert_eq!(
                headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
                Some(&HeaderValue::from_static(ORIGIN)),
                "{method} {uri}"
            );
        }
    }

    /// A span or event the recording subscriber saw.
    #[derive(Debug, Clone)]
    struct Seen {
        span: bool,
        name: &'static str,
        target: &'static str,
        level: Level,
        fields: BTreeSet<&'static str>,
    }

    /// A subscriber that records the metadata of every span and event.
    #[derive(Clone, Default)]
    struct Recorder(Arc<Mutex<Vec<Seen>>>);

    impl Recorder {
        fn push(&self, span: bool, metadata: &'static Metadata<'static>) -> usize {
            let mut seen = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            seen.push(Seen {
                span,
                name: metadata.name(),
                target: metadata.target(),
                level: *metadata.level(),
                fields: metadata.fields().iter().map(|field| field.name()).collect(),
            });
            seen.len()
        }

        fn seen(&self) -> Vec<Seen> {
            self.0
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        }
    }

    impl Subscriber for Recorder {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, span: &Attributes<'_>) -> Id {
            let count = self.push(true, span.metadata());
            Id::from_u64(u64::try_from(count).expect("fewer spans than u64::MAX"))
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            self.push(false, event.metadata());
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[tokio::test]
    async fn request_span_and_response_event_are_info_and_record_no_headers() {
        let recorder = Recorder::default();
        let guard = tracing::subscriber::set_default(recorder.clone());
        let (status, _, _) = send(request(
            Method::PUT,
            "/accounts/00000000001",
            &[
                (header::ORIGIN, ORIGIN),
                (header::AUTHORIZATION, "Bearer fake-token-for-tests"),
            ],
        ))
        .await;
        drop(guard);
        assert_eq!(status, StatusCode::OK);

        let seen = recorder.seen();
        let spans: Vec<&Seen> = seen
            .iter()
            .filter(|seen| seen.span && seen.name == "request")
            .collect();
        assert_eq!(spans.len(), 1, "{seen:?}");
        assert_eq!(spans[0].level, Level::INFO);
        assert!(spans[0].target.starts_with("tower_http::trace"), "{seen:?}");
        assert_eq!(
            spans[0].fields,
            BTreeSet::from(["method", "uri", "version"]),
            "the span records no header, the bearer token included"
        );

        let responses: Vec<&Seen> = seen
            .iter()
            .filter(|seen| !seen.span && seen.target == "tower_http::trace::on_response")
            .collect();
        assert_eq!(responses.len(), 1, "{seen:?}");
        assert_eq!(responses[0].level, Level::INFO);
        assert!(responses[0].fields.contains("status"), "{seen:?}");
        assert!(!responses[0].fields.contains("headers"), "{seen:?}");
    }
}
