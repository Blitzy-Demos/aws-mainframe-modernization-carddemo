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
//! - [`state`]: [`AppState`], built once per process from the settings,
//!   checked first as a [`PreparedState`], and the database handle.
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
pub use state::{AppState, PreparedState, StateError};

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use axum::Router;
use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::{HeaderMap, HeaderValue, Method, Request, Response, header};
use error::ApiError;
use tower::{Layer, Service};
use tower_http::cors::{AllowOrigin, Cors, CorsLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::{Level, Span};

/// The HTTP server: every route of [`handlers::router()`] with `state`, behind
/// the request trace layer and the CORS layer.
///
/// The routes come from the same registry [`spec`] documents, so the server
/// serves exactly the documented operations, and no route serves the document
/// itself. A path the registry does not hold answers 404 with
/// [`error::ApiError::route_not_found`], and a method a path does not serve
/// answers 405 with [`error::ApiError::method_not_allowed`] and axum's `Allow`
/// header listing the methods it does, both in the `{code, message, fields?}`
/// error body every other failure has.
///
/// Building the router opens no connection and reads no environment variable:
/// everything it needs arrives in `state`, already validated by
/// [`PreparedState::new`].
///
/// # Trace layer
///
/// Each request gets the `tower-http` `request` span, made by
/// `request_span` with the method, the matched route template and the HTTP
/// version, and, when the response is ready, a `finished processing request`
/// event with the status and latency. The handler spans, with `transaction`,
/// `user_id`, the resource keys and `outcome`, open inside it. Both the span
/// and the event are at INFO, not `tower-http`'s default DEBUG, because the
/// default `RUST_LOG=info` filter would otherwise drop them, and with them the
/// request span every API write must emit as a JSON line through
/// `carddemo_runtime::telemetry`. Neither records the URI or a header, so no
/// card number, query text or bearer token reaches the logs from them.
///
/// # CORS layer
///
/// The CORS layer admits one browser origin, [`AppState::cors_origin`]
/// (`CARDDEMO_CORS_ORIGIN`, by default the Vite dev server
/// `http://localhost:5173`), for `GET`, `POST`, `PUT` and `DELETE` with the
/// `Authorization` and `Content-Type` request headers. It sits on the routed
/// paths only, so a path the registry does not hold answers 404 to every
/// method, `OPTIONS` included. On a routed path it answers a CORS preflight
/// itself, and only that: an `OPTIONS` request without both `Origin` and
/// `Access-Control-Request-Method` gets the path's 405.
// A reason string rather than a bare `#[must_use]`: `Router` is already
// `#[must_use]`, and clippy's `double_must_use` (denied) rejects a bare one here.
#[must_use = "the router answers no request until it is served, for example by `axum::serve`"]
pub fn app(state: AppState) -> Router {
    let (router, _doc) = handlers::router().split_for_parts();
    let cors_origin = state.cors_origin().clone();
    with_layers(router.with_state(state), cors_origin)
}

/// Gives `router` the 404 and 405 answers of [`app`], puts the CORS layer on
/// its routes, then wraps everything in the trace layer.
///
/// The 405 answer must be set before any layer: it replaces the default
/// fallback of each route's method router, which a layer would already have
/// wrapped. The CORS layer is a route layer, so it wraps each route's
/// operations and 405 answer but not the 404 of the router's fallback. The
/// trace layer wraps the routes and the fallback alike, so every request,
/// preflights included, opens a request span.
///
/// Kept apart from [`app`] so the layers are tested over a stand-in router,
/// without the database handle an [`AppState`] holds.
fn with_layers(router: Router, cors_origin: HeaderValue) -> Router {
    router
        .fallback(|| async { ApiError::route_not_found() })
        .method_not_allowed_fallback(|| async { ApiError::method_not_allowed() })
        .route_layer(RoutedCorsLayer(cors_layer(cors_origin)))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(request_span)
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}

/// The `route` of a request span whose path no route matched. A route
/// template starts with `/`, so the marker is never mistaken for one.
const UNMATCHED_ROUTE: &str = "unmatched";

/// The `method` of a request span whose method is no standard HTTP method.
const OTHER_METHOD: &str = "_OTHER";

/// The request span of [`app`]'s trace layer: `request` at INFO, with fields
/// whose values come from closed sets, never from text the client wrote:
///
/// - `method`: the request method when it is a standard HTTP method, else
///   [`OTHER_METHOD`];
/// - `route`: the template of the matched route, such as `/accounts/{id}`,
///   else [`UNMATCHED_ROUTE`];
/// - `version`: the HTTP version, such as `HTTP/1.1`.
///
/// The URI is never recorded: its path holds the resource keys, card numbers
/// among them, and its query any text at all, before any authentication or
/// validation. Nor are the headers, so a bearer token never reaches the logs.
fn request_span(request: &Request<Body>) -> Span {
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED_ROUTE, MatchedPath::as_str);
    tracing::info_span!(
        "request",
        method = bounded_method(request.method()),
        route,
        version = ?request.version(),
    )
}

/// `method`'s name when it is one of the nine standard HTTP methods, else
/// [`OTHER_METHOD`]: an extension method is a token of the client's choosing.
fn bounded_method(method: &Method) -> &str {
    match *method {
        Method::GET
        | Method::HEAD
        | Method::POST
        | Method::PUT
        | Method::DELETE
        | Method::OPTIONS
        | Method::PATCH
        | Method::CONNECT
        | Method::TRACE => method.as_str(),
        _ => OTHER_METHOD,
    }
}

/// The CORS policy of [`app`] for the one admitted `origin`.
///
/// The origin is a one-entry list rather than an exact value: `tower-http`
/// sends an exact value to every requester, whereas a list names the origin
/// only to the requester it matches. A response to any other origin therefore
/// carries no `Access-Control-Allow-Origin`, so the browser refuses it and the
/// admitted origin is not disclosed.
///
/// [`AppState`] guarantees that [`AppState::cors_origin`] is one concrete
/// serialized origin, so the list never holds the wildcard `*`, on which
/// `AllowOrigin::list` panics, and never a spelling no browser sends.
fn cors_layer(origin: HeaderValue) -> CorsLayer {
    CorsLayer::new()
        .allow_origin(AllowOrigin::list([origin]))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
}

/// [`cors_layer`] as [`app`] puts it on each route: it answers a CORS
/// preflight and adds its response headers to every other request, except an
/// `OPTIONS` request that is no preflight.
///
/// `tower-http`'s CORS layer answers every `OPTIONS` request as a preflight.
/// This layer hands it an `OPTIONS` request only when [`is_preflight`] holds;
/// any other `OPTIONS` request goes straight to the route's method router,
/// which answers 405 with the path's `Allow` header.
#[derive(Debug, Clone)]
struct RoutedCorsLayer(CorsLayer);

impl<S> Layer<S> for RoutedCorsLayer {
    type Service = RoutedCors<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RoutedCors(self.0.layer(inner))
    }
}

/// The service of [`RoutedCorsLayer`]: the CORS layer around one route's
/// method handler or 405 answer.
#[derive(Debug, Clone)]
struct RoutedCors<S>(Cors<S>);

/// The response future of [`RoutedCors`]: the CORS layer's, or the wrapped
/// service's own for an `OPTIONS` request that is no preflight.
type RoutedCorsFuture<R, E> = Pin<Box<dyn Future<Output = Result<R, E>> + Send>>;

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RoutedCors<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    S::Future: Send + 'static,
    ResBody: Default + Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = RoutedCorsFuture<Self::Response, Self::Error>;

    /// Readiness is the wrapped service's, polled through the CORS layer.
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, request: Request<ReqBody>) -> Self::Future {
        if request.method() == Method::OPTIONS && !is_preflight(request.headers()) {
            Box::pin(self.0.get_mut().call(request))
        } else {
            Box::pin(self.0.call(request))
        }
    }
}

/// Whether an `OPTIONS` request with `headers` is a CORS preflight: it names
/// both the `Origin` it comes from and the `Access-Control-Request-Method` it
/// asks for, as the Fetch standard's CORS-preflight request does.
fn is_preflight(headers: &HeaderMap) -> bool {
    headers.contains_key(header::ORIGIN)
        && headers.contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::BTreeSet;
    use std::fmt;
    use std::sync::Once;
    use std::sync::atomic::{AtomicU64, Ordering};

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
    use axum::routing::get;
    use tower::ServiceExt;
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::Interest;
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::error::{ApiError, ErrorBody};
    use super::{OTHER_METHOD, UNMATCHED_ROUTE, with_layers};

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

    /// Sends `request` through [`layered`] and returns the answer's status,
    /// headers and body, after making [`Recorder`] the process's subscriber.
    async fn send(request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        install_recorder();
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
        // The preflight is answered in place of the path's 405, so the method
        // router adds the path's `Allow` header to it.
        assert_eq!(
            headers.get(header::ALLOW),
            Some(&HeaderValue::from_static("GET,HEAD,PUT"))
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

    /// Sends `request` and asserts that the answer is `expected`'s status and
    /// error body (no body for `HEAD`) with `allow` as its `Allow` header,
    /// that it names the admitted origin only when `names_origin`, and that it
    /// carries no preflight header.
    async fn assert_error_answer(
        request: Request<Body>,
        expected: &ApiError,
        allow: Option<&str>,
        names_origin: bool,
    ) {
        let case = format!(
            "{} {} {:?}",
            request.method(),
            request.uri(),
            request.headers()
        );
        let head = request.method() == Method::HEAD;
        let (status, headers, body) = send(request).await;
        assert_eq!(status, expected.status(), "{case}");
        assert_eq!(
            headers.get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("application/json")),
            "{case}"
        );
        assert_eq!(
            headers
                .get(header::ALLOW)
                .map(|value| value.to_str().expect("an ASCII header")),
            allow,
            "{case}"
        );
        assert_eq!(
            headers.get(header::ACCESS_CONTROL_ALLOW_ORIGIN),
            names_origin
                .then(|| HeaderValue::from_static(ORIGIN))
                .as_ref(),
            "{case}"
        );
        for preflight_header in [
            header::ACCESS_CONTROL_ALLOW_METHODS,
            header::ACCESS_CONTROL_ALLOW_HEADERS,
        ] {
            assert!(
                headers.get(&preflight_header).is_none(),
                "{case}: {preflight_header}"
            );
        }
        if head {
            assert!(body.is_empty(), "{case}");
        } else {
            assert_eq!(
                serde_json::from_slice::<ErrorBody>(&body).expect("an error body"),
                *expected.body(),
                "{case}"
            );
        }
    }

    #[tokio::test]
    async fn requests_no_operation_serves_answer_the_error_body_and_no_preflight() {
        const ACCOUNT: &str = "/accounts/00000000001";
        const ALLOW: Option<&str> = Some("GET,HEAD,PUT");
        let not_found = ApiError::route_not_found();
        let not_allowed = ApiError::method_not_allowed();
        let none: &[(HeaderName, &str)] = &[];
        let admitted: &[(HeaderName, &str)] = &[(header::ORIGIN, ORIGIN)];
        let foreign: &[(HeaderName, &str)] = &[(header::ORIGIN, "http://evil.example")];
        let preflight: &[(HeaderName, &str)] = &[
            (header::ORIGIN, ORIGIN),
            (header::ACCESS_CONTROL_REQUEST_METHOD, "GET"),
        ];
        let method_only: &[(HeaderName, &str)] = &[(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")];

        // A path the registry does not hold: 404, outside the CORS layer.
        for (method, uri) in [
            (Method::GET, "/no-such-path"),
            (Method::GET, "/api/accounts/00000000001"),
            (Method::POST, "/accounts"),
            (Method::HEAD, "/no-such-path"),
        ] {
            assert_error_answer(request(method, uri, admitted), &not_found, None, false).await;
        }
        // A method a routed path does not serve: 405, through the CORS layer.
        for method in [Method::DELETE, Method::POST] {
            let wrong = request(method, ACCOUNT, admitted);
            assert_error_answer(wrong, &not_allowed, ALLOW, true).await;
        }
        // An `OPTIONS` request without both preflight headers is no preflight,
        // so a routed path answers it with its 405 and no CORS header.
        for headers in [none, admitted, method_only] {
            let options = request(Method::OPTIONS, ACCOUNT, headers);
            assert_error_answer(options, &not_allowed, ALLOW, false).await;
        }
        // A path the registry does not hold answers 404 to `OPTIONS` as to
        // every other method, preflight or not.
        for uri in [
            "/health",
            "/metrics",
            "/v1/accounts/00000000001",
            "/rewards/redeem",
        ] {
            for headers in [none, preflight, foreign] {
                let options = request(Method::OPTIONS, uri, headers);
                assert_error_answer(options, &not_found, None, false).await;
            }
        }
    }

    /// A span or event the recording subscriber saw.
    #[derive(Debug)]
    struct Seen {
        /// The ID [`Recorder`] gave the span; `None` for an event.
        span: Option<u64>,
        name: &'static str,
        target: &'static str,
        level: Level,
        fields: BTreeSet<&'static str>,
        /// Every value recorded, when the span or event was made or later, as
        /// text: a string as it is, any other value in its `Debug` form.
        values: Vec<(&'static str, String)>,
    }

    impl Seen {
        /// The span with ID `span`, or the event when `span` is `None`, that
        /// `metadata` describes, with its `values` so far.
        fn new(
            span: Option<u64>,
            metadata: &'static Metadata<'static>,
            values: Vec<(&'static str, String)>,
        ) -> Self {
            Self {
                span,
                name: metadata.name(),
                target: metadata.target(),
                level: *metadata.level(),
                fields: metadata.fields().iter().map(|field| field.name()).collect(),
                values,
            }
        }
    }

    /// A visitor that appends each recorded value to a list, as text.
    struct Values<'a>(&'a mut Vec<(&'static str, String)>);

    impl Visit for Values<'_> {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.push((field.name(), value.to_owned()));
        }

        fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
            self.0.push((field.name(), format!("{value:?}")));
        }
    }

    thread_local! {
        /// What [`Recorder`] has seen on this thread since [`capturing`]
        /// began here, or `None` while no capture runs on this thread.
        static CAPTURE: RefCell<Option<Vec<Seen>>> = const { RefCell::new(None) };
    }

    /// The ID [`Recorder`] gives its next span, unique across threads.
    static NEXT_SPAN: AtomicU64 = AtomicU64::new(1);

    /// The process's subscriber: it records the metadata and the values of
    /// every span and event made on a thread while [`capturing`] runs there,
    /// and enables nothing on any other thread.
    ///
    /// It is the global default, not a thread's, because the interest a
    /// callsite caches is global: a callsite first hit on a thread with no
    /// subscriber caches `never`, and a recorder scoped to another thread then
    /// misses it until the next subscriber is made. As the one subscriber, the
    /// recorder answers `sometimes` for every callsite, so each hit asks
    /// [`Subscriber::enabled`] on the thread it happens on.
    struct Recorder;

    /// Makes [`Recorder`] the process's subscriber, once, before any request
    /// reaches the trace layer.
    ///
    /// The subscriber becomes visible to other threads only after the cached
    /// interests are rebuilt for it, so a callsite first hit in between would
    /// cache `never`. Every request goes through [`send`], which calls this
    /// first, and [`Once`] holds each caller until the recorder is installed:
    /// no thread reaches the trace layer's callsites before then.
    fn install_recorder() {
        static INSTALL: Once = Once::new();
        INSTALL.call_once(|| {
            tracing::subscriber::set_global_default(Recorder)
                .expect("no other subscriber is the process's default");
        });
    }

    /// Awaits `future` with this thread capturing, and returns its output with
    /// every span and event [`Recorder`] saw meanwhile.
    ///
    /// `#[tokio::test]` runs its test on a current-thread runtime, so the
    /// future, and every span and event it makes, stay on this thread.
    async fn capturing<F: Future>(future: F) -> (F::Output, Vec<Seen>) {
        CAPTURE.set(Some(Vec::new()));
        let output = future.await;
        let seen = CAPTURE.take().expect("the capture this call began");
        (output, seen)
    }

    /// Runs `f` on this thread's capture, when one runs here.
    fn in_capture(f: impl FnOnce(&mut Vec<Seen>)) {
        // The storage is gone only while the thread exits, when nothing
        // captures, so there is nothing to record then.
        let _ = CAPTURE.try_with(|capture| {
            if let Some(seen) = capture.borrow_mut().as_mut() {
                f(seen);
            }
        });
    }

    impl Subscriber for Recorder {
        /// `sometimes` for every callsite: whether a hit is recorded depends
        /// on its thread, so [`Self::enabled`] decides at each one.
        fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
            Interest::sometimes()
        }

        /// Whether this thread captures. A thread whose storage is gone is
        /// exiting, and captures nothing.
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            CAPTURE
                .try_with(|capture| capture.borrow().is_some())
                .unwrap_or(false)
        }

        fn new_span(&self, span: &Attributes<'_>) -> Id {
            let id = NEXT_SPAN.fetch_add(1, Ordering::Relaxed);
            let mut values = Vec::new();
            span.record(&mut Values(&mut values));
            in_capture(|seen| seen.push(Seen::new(Some(id), span.metadata(), values)));
            Id::from_u64(id)
        }

        /// Appends `values` to the span `span` names. A capture holds every
        /// span made on its thread while it runs, so a span it lacks fails
        /// the test.
        fn record(&self, span: &Id, values: &Record<'_>) {
            let id = span.into_u64();
            let mut recorded = Vec::new();
            values.record(&mut Values(&mut recorded));
            in_capture(|seen| {
                seen.iter_mut()
                    .find(|entry| entry.span == Some(id))
                    .expect("a span made during this capture")
                    .values
                    .extend(recorded);
            });
        }

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut values = Vec::new();
            event.record(&mut Values(&mut values));
            in_capture(|seen| seen.push(Seen::new(None, event.metadata(), values)));
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[tokio::test]
    async fn request_span_records_the_method_route_and_version_only() {
        /// Client-written text of the requests below: the path's account key,
        /// the query's names and values, and the bearer token.
        const CLIENT_TEXT: [&str; 7] = [
            "41234567890123456",
            "4000123412341234",
            "TopSecret99",
            "card_number",
            "secret",
            "/accounts/4",
            "fake-token-for-tests",
        ];
        let account = "/accounts/41234567890123456?card_number=4000123412341234&secret=TopSecret99";
        let unrouted = "/cards-by-number/4000123412341234?secret=TopSecret99";
        let extension = Method::from_bytes(b"X-4000123412341234").expect("a method token");
        // (method, URI, the status, the span's `method`, the span's `route`)
        let cases = [
            (
                Method::PUT,
                account,
                StatusCode::OK,
                "PUT",
                "/accounts/{id}",
            ),
            (
                Method::GET,
                unrouted,
                StatusCode::NOT_FOUND,
                "GET",
                UNMATCHED_ROUTE,
            ),
            (
                extension,
                account,
                StatusCode::METHOD_NOT_ALLOWED,
                OTHER_METHOD,
                "/accounts/{id}",
            ),
        ];
        for (method, uri, status, span_method, span_route) in cases {
            let case = format!("{method} {uri}");
            let ((answered, _, _), seen) = capturing(send(request(
                method,
                uri,
                &[
                    (header::ORIGIN, ORIGIN),
                    (header::AUTHORIZATION, "Bearer fake-token-for-tests"),
                ],
            )))
            .await;
            assert_eq!(answered, status, "{case}");

            let spans: Vec<&Seen> = seen
                .iter()
                .filter(|seen| seen.span.is_some() && seen.name == "request")
                .collect();
            assert_eq!(spans.len(), 1, "{case}: {seen:?}");
            assert_eq!(spans[0].level, Level::INFO, "{case}");
            assert_eq!(spans[0].target, env!("CARGO_CRATE_NAME"), "{case}");
            assert_eq!(
                spans[0].fields,
                BTreeSet::from(["method", "route", "version"]),
                "{case}"
            );
            assert_eq!(
                spans[0].values,
                [
                    ("method", span_method.to_owned()),
                    ("route", span_route.to_owned()),
                    ("version", "HTTP/1.1".to_owned()),
                ],
                "{case}"
            );

            let responses: Vec<&Seen> = seen
                .iter()
                .filter(|seen| {
                    seen.span.is_none() && seen.target == "tower_http::trace::on_response"
                })
                .collect();
            assert_eq!(responses.len(), 1, "{case}: {seen:?}");
            assert_eq!(responses[0].level, Level::INFO, "{case}");
            assert!(responses[0].fields.contains("status"), "{case}: {seen:?}");

            for entry in &seen {
                for field in ["uri", "path", "query", "headers"] {
                    assert!(!entry.fields.contains(field), "{case}: {entry:?}");
                }
                for (field, value) in &entry.values {
                    for text in CLIENT_TEXT {
                        assert!(!value.contains(text), "{case}: {field} = {value:?}");
                    }
                }
            }
        }
    }
}
