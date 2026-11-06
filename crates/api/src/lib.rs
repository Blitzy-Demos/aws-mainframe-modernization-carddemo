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
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, Request, Response, Uri, header};
use axum::middleware::map_response;
use error::ApiError;
use tower::{Layer, Service};
use tower_http::cors::{AllowOrigin, Cors, CorsLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::field::Empty;
use tracing::{Level, Span};

/// The HTTP server: every route of [`handlers::router()`] with `state`, behind
/// the request trace layer, the response header layer and the CORS layer.
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
/// `user_id`, the resource keys and `outcome`, open inside it. A handler
/// whose request is dropped before it answers, its client having gone away,
/// records `cancelled` as its `outcome` through `cancellable`. A request an
/// extractor refuses, with 401, 403, or 400 for a body, query or path that
/// does not deserialize, never reaches its handler, so the request span
/// records the handler's `transaction` and `outcome` itself, through
/// `record_refusal`, with the `user_id` of its token once that token has
/// verified. A request whose verified token's user cannot be read is
/// answered 500 before its handler too, and recorded the same way through
/// `record_failure`, with that token's `user_id`. Both the span and the
/// event are at INFO, not
/// `tower-http`'s default DEBUG, because the default `RUST_LOG=info` filter
/// would otherwise drop them, and with them the request span every API write
/// must emit as a JSON line through `carddemo_runtime::telemetry`. Neither
/// records the URI or a header, so no card number, query text or bearer token
/// reaches the logs from them.
///
/// # Response header layer
///
/// Every answer, handler errors, the 404 and 405 answers and CORS preflights
/// included, carries `Cache-Control: no-store`, `X-Content-Type-Options:
/// nosniff`, `X-Frame-Options: DENY` and `Content-Security-Policy:
/// frame-ancestors 'none'`. The answers hold per-account and admin data
/// (balances, card numbers, reward points, user records, reward rules), so no
/// browser or intermediary may keep a copy, no browser may read a body as any
/// type other than its declared `Content-Type`, and no page may frame an
/// answer: `frame-ancestors` forbids it to browsers that apply Content
/// Security Policy, and `X-Frame-Options` to browsers that predate it. Each
/// header is added only to an answer that does not carry it already: a
/// handler that sets its own value keeps it, and no answer carries any of
/// them twice.
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
/// its routes, then wraps everything in the response header layer and that in
/// the trace layer.
///
/// The 405 answer must be set before any layer: it replaces the default
/// fallback of each route's method router, which a layer would already have
/// wrapped. The CORS layer is a route layer, so it wraps each route's
/// operations and 405 answer but not the 404 of the router's fallback. The
/// response header layer, [`add_response_headers`], wraps the routes, outside
/// the CORS layer, and the fallback alike, so the preflight, 404 and 405
/// answers get its headers as the operations' answers do, and the CORS layer
/// answers exactly as it would without it. The trace layer is outermost and
/// wraps the routes and the fallback alike, so every request, preflights
/// included, opens a request span.
///
/// Kept apart from [`app`] so the layers are tested over a stand-in router,
/// without the database handle an [`AppState`] holds.
fn with_layers(router: Router, cors_origin: HeaderValue) -> Router {
    router
        .fallback(|| async { ApiError::route_not_found() })
        .method_not_allowed_fallback(|| async { ApiError::method_not_allowed() })
        .route_layer(RoutedCorsLayer(cors_layer(cors_origin)))
        .layer(map_response(add_response_headers))
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
/// It also declares `transaction`, `user_id` and `outcome`, empty. They are
/// recorded only by [`record_refusal`], for a request an extractor refuses
/// before its handler span opens, and by [`record_failure`], for one an
/// extractor answers with a 5xx status then. `user_id` is recorded only when
/// the request's token verified before the refusal or the failure. A field
/// never recorded is not exported, so the span of every other request
/// carries the three fields above only.
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
        transaction = Empty,
        user_id = Empty,
        outcome = Empty,
    )
}

/// The `outcome` of a request an extractor refused: the word the handler
/// spans record for a request they answer with a 4xx status.
const OUTCOME_REJECTED: &str = "rejected";

/// Records on the current span, at extraction the request span of
/// [`request_span`], that an extractor refused the request, so the request
/// span carries what the handler span would have:
///
/// - `transaction`: `transaction`, the code [`transaction_of`] gives the
///   request, when it has one;
/// - `user_id`: `user_id`, the verified token's `sub`, when the request's
///   token verified before the refusal: the admin check's 403, or a body,
///   query or path refused after [`auth::AuthUser`] or [`auth::AdminUser`]
///   kept the verified user in the request's extensions. A token that did not
///   verify, and the public `POST /auth/login`, give none;
/// - `outcome`: [`OUTCOME_REJECTED`].
///
/// Extraction runs inside the trace layer's span and before a handler opens
/// its own, and axum stops at the first extractor that refuses, so each
/// refused request records them once, on its request span. An extractor
/// records them before it logs the refusal, so its event carries them in its
/// span context. The refused input is never recorded: a rejection's text can
/// quote the body, query or path the client wrote.
pub(crate) fn record_refusal(transaction: Option<&'static str>, user_id: Option<&str>) {
    record_unanswered(transaction, user_id, OUTCOME_REJECTED);
}

/// The `outcome` of a request an extractor answered with a 5xx status: the
/// word the handler spans record for a 5xx answer.
const OUTCOME_FAILED: &str = "failed";

/// Records on the current span, as [`record_refusal`] does, that an extractor
/// answered the request with a 5xx status before its handler span opened:
/// [`auth::AuthUser`]'s read of the token's user failed, or its unit of work
/// could not be opened or rolled back. `outcome` is [`OUTCOME_FAILED`] and
/// `user_id` is `user_id`, the `sub` of the request's verified token, so the
/// 500 names the user it was answered to. The token's signature is verified
/// by then, so the value is a claim signed with the server's key, never a
/// path key or other text the client wrote.
///
/// It is recorded before the failure's detail is logged, so that event
/// carries it in its span context. A rollback that fails after a refusal was
/// recorded records `failed` over `rejected`, the 500 being the answer.
pub(crate) fn record_failure(transaction: Option<&'static str>, user_id: &str) {
    record_unanswered(transaction, Some(user_id), OUTCOME_FAILED);
}

/// Records `transaction` and `user_id`, when given, and `outcome` on the
/// current span, the request span while extraction runs.
fn record_unanswered(
    transaction: Option<&'static str>,
    user_id: Option<&str>,
    outcome: &'static str,
) {
    let span = Span::current();
    if let Some(transaction) = transaction {
        span.record("transaction", transaction);
    }
    if let Some(user_id) = user_id {
        span.record("user_id", user_id);
    }
    span.record("outcome", outcome);
}

/// The `outcome` of a request dropped before its handler answered: its client
/// went away, for example by giving up waiting, and the server dropped the
/// handler's future.
const OUTCOME_CANCELLED: &str = "cancelled";

/// Awaits `work`, the work of the handler whose span is `span`, and records
/// [`OUTCOME_CANCELLED`] as `outcome` on `span` when this future is dropped
/// before `work` completes.
///
/// The client abandoned the request before the handler answered: hyper drops
/// the future of a request whose connection closes, so the handler stops at
/// the await it had reached and never records an outcome of its own. A unit
/// of work open at that point is rolled back as it is dropped, sqlx queueing
/// the `ROLLBACK` on its connection, unless its `COMMIT` had already reached
/// the server. The word is therefore `cancelled`, not `rolled_back`: the drop
/// can come before any unit of work opens, or while its `COMMIT` is in flight.
///
/// Once `work` has completed nothing is recorded here: `work`, or its caller
/// after it, records the outcome, and no handler awaits anything after that,
/// so `outcome` is recorded once per request. A future dropped while its
/// thread unwinds a panic records nothing, a panic being no cancellation. The
/// guard is set when this future is first polled, and every handler awaits
/// it at once. `work` runs in whatever span its caller instrumented it with:
/// `span` is only recorded on, never entered.
pub(crate) async fn cancellable<F: Future>(span: Span, work: F) -> F::Output {
    let mut guard = CancelGuard(Some(span));
    let output = work.await;
    guard.0 = None;
    output
}

/// The guard of [`cancellable`]: dropped while it still holds the handler
/// span, it records [`OUTCOME_CANCELLED`] there, unless a panic is unwinding.
struct CancelGuard(Option<Span>);

impl Drop for CancelGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        if let Some(span) = self.0.take() {
            span.record("outcome", OUTCOME_CANCELLED);
        }
    }
}

/// The `transaction` of the operation that serves a request with `method`,
/// `uri` and `extensions`: [`handlers::transaction_code`] of its matched
/// route template, or `None` when no route matched it.
pub(crate) fn transaction_of(
    method: &Method,
    uri: &Uri,
    extensions: &Extensions,
) -> Option<&'static str> {
    let route = extensions.get::<MatchedPath>()?;
    handlers::transaction_code(method, route.as_str(), uri)
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

/// The response header layer of [`app`]: `response` with
/// `X-Content-Type-Options: nosniff`, `Cache-Control: no-store`,
/// `X-Frame-Options: DENY` and `Content-Security-Policy: frame-ancestors
/// 'none'`, each added only when `response` does not carry that header
/// already, so a value its handler set wins and no header is ever sent twice.
///
/// The two frame headers keep every page from framing an answer:
/// `frame-ancestors` is the control of browsers that apply Content Security
/// Policy, and `X-Frame-Options` that of browsers that predate it.
async fn add_response_headers(mut response: Response<Body>) -> Response<Body> {
    let headers = response.headers_mut();
    headers
        .entry(header::X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    headers
        .entry(header::X_FRAME_OPTIONS)
        .or_insert(HeaderValue::from_static("DENY"));
    headers
        .entry(header::CONTENT_SECURITY_POLICY)
        .or_insert(HeaderValue::from_static("frame-ancestors 'none'"));
    response
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
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, Once};
    use std::task::{Context, Poll, Waker};
    use std::{fmt, io};

    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::FromRequestParts;
    use axum::http::request::Parts;
    use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
    use axum::routing::{get, post, put};
    use carddemo_runtime::telemetry::subscriber_with_writer;
    use serde_json::Value;
    use tower::ServiceExt;
    use tracing::field::{Empty, Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::Interest;
    use tracing::{Event, Level, Metadata, Span, Subscriber};

    use super::auth::extract::{authenticate, require_admin};
    use super::auth::jwt::{JwtKeys, UserType};
    use super::dto::KeySegment;
    use super::dto::accounts::AccountUpdateRequest;
    use super::dto::admin_transaction_types::{TranTypeUpdateQuery, TranTypeUpdateRequest};
    use super::dto::auth::LoginRequest;
    use super::dto::authorizations::AuthIdPath;
    use super::error::{ApiError, ApiJson, ApiPath, ApiQuery, ErrorBody};
    use super::pagination::PageQuery;
    use super::{OTHER_METHOD, UNMATCHED_ROUTE, cancellable, with_layers};

    /// The admitted origin: the default `CARDDEMO_CORS_ORIGIN`.
    const ORIGIN: &str = "http://localhost:5173";

    /// The path of [`layered`]'s route whose GET answer sets its own
    /// `Cache-Control`.
    const CACHED: &str = "/cached";

    /// The path of [`layered`]'s route whose GET handler answers with an
    /// error.
    const DENIED: &str = "/denied";

    /// A stand-in for the registry: one path with a GET and a PUT, as
    /// `/accounts/{id}` has, [`CACHED`], whose GET sets its own
    /// `Cache-Control: max-age=60`, and [`DENIED`], whose GET answers
    /// [`ApiError::unauthenticated`].
    fn layered() -> Router {
        let routes = Router::new()
            .route(
                "/accounts/{id}",
                get(|| async { "account" }).put(|| async { "updated" }),
            )
            .route(
                CACHED,
                get(|| async { ([(header::CACHE_CONTROL, "max-age=60")], "cached") }),
            )
            .route(DENIED, get(|| async { ApiError::unauthenticated() }));
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

    /// Sends `request` through [`layered`], as [`send_to`] does.
    async fn send(request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        send_to(layered(), request).await
    }

    /// Sends `request` through `router` and returns the answer's status,
    /// headers and body, after making [`Recorder`] the process's subscriber.
    async fn send_to(router: Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
        install_recorder();
        let response = router
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

    #[tokio::test]
    async fn every_answer_is_no_store_nosniff_and_unframeable_unless_its_handler_sets_its_own() {
        const ACCOUNT: &str = "/accounts/00000000001";
        const NO_STORE: &str = "no-store";
        let admitted: &[(HeaderName, &str)] = &[(header::ORIGIN, ORIGIN)];
        // (the request, its status, the one `Cache-Control` value it gets)
        let cases = [
            (
                request(Method::GET, ACCOUNT, admitted),
                StatusCode::OK,
                NO_STORE,
            ),
            (
                request(Method::HEAD, ACCOUNT, admitted),
                StatusCode::OK,
                NO_STORE,
            ),
            (
                request(Method::PUT, ACCOUNT, admitted),
                StatusCode::OK,
                NO_STORE,
            ),
            (
                request(Method::GET, DENIED, admitted),
                StatusCode::UNAUTHORIZED,
                NO_STORE,
            ),
            (preflight(ORIGIN), StatusCode::OK, NO_STORE),
            (preflight("http://evil.example"), StatusCode::OK, NO_STORE),
            (
                request(Method::OPTIONS, ACCOUNT, admitted),
                StatusCode::METHOD_NOT_ALLOWED,
                NO_STORE,
            ),
            (
                request(Method::DELETE, ACCOUNT, admitted),
                StatusCode::METHOD_NOT_ALLOWED,
                NO_STORE,
            ),
            (
                request(Method::GET, "/no-such-path", admitted),
                StatusCode::NOT_FOUND,
                NO_STORE,
            ),
            (
                request(Method::HEAD, "/no-such-path", admitted),
                StatusCode::NOT_FOUND,
                NO_STORE,
            ),
            (
                request(Method::GET, CACHED, admitted),
                StatusCode::OK,
                "max-age=60",
            ),
        ];
        for (request, status, cache_control) in cases {
            let case = format!(
                "{} {} {:?}",
                request.method(),
                request.uri(),
                request.headers()
            );
            let (answered, headers, _) = send(request).await;
            assert_eq!(answered, status, "{case}");
            for (name, expected) in [
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                (header::CACHE_CONTROL, cache_control),
                (header::X_FRAME_OPTIONS, "DENY"),
                (header::CONTENT_SECURITY_POLICY, "frame-ancestors 'none'"),
            ] {
                let sent: Vec<&str> = headers
                    .get_all(&name)
                    .iter()
                    .map(|value| value.to_str().expect("an ASCII header"))
                    .collect();
                assert_eq!(sent, [expected], "{case}: {name}");
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
    /// cache `never`. Every request goes through [`send_to`], which calls this
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
            // `transaction`, `user_id` and `outcome` are declared for a refusal
            // and stay unrecorded here.
            assert_eq!(
                spans[0].fields,
                BTreeSet::from([
                    "method",
                    "route",
                    "version",
                    "transaction",
                    "user_id",
                    "outcome"
                ]),
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

    /// Stand-ins for registry operations that take [`ApiJson`], [`ApiPath`]
    /// and [`ApiQuery`], on the registry's route templates and with the
    /// operations' DTOs, so a refusal names the operation the registry serves
    /// there.
    fn extracting() -> Router {
        let routes = Router::new()
            .route(
                "/auth/login",
                post(|ApiJson(_): ApiJson<LoginRequest>| async { "signed on" }),
            )
            .route(
                "/accounts/{id}",
                get(|ApiPath(_): ApiPath<String>| async { "account" }).put(
                    |ApiPath(_): ApiPath<String>, ApiJson(_): ApiJson<AccountUpdateRequest>| async {
                        "updated"
                    },
                ),
            )
            .route(
                "/cards",
                get(|ApiQuery(_): ApiQuery<PageQuery>| async { "cards" }),
            )
            .route(
                "/authorizations/{id}",
                get(|ApiPath(_): ApiPath<AuthIdPath>| async { "authorization" }),
            )
            .route(
                "/admin/transaction-types/{id}",
                put(
                    |ApiPath(_): ApiPath<KeySegment>,
                     ApiQuery(_): ApiQuery<TranTypeUpdateQuery>,
                     ApiJson(_): ApiJson<TranTypeUpdateRequest>| async {
                        "type updated"
                    },
                ),
            );
        with_layers(routes, HeaderValue::from_static(ORIGIN))
    }

    /// The `Content-Type` of a JSON body.
    const JSON: Option<&str> = Some("application/json");

    /// A sign-on body that deserializes.
    const LOGIN: &str = r#"{"user_id":"USER0001","password":"PASSW0RD"}"#;

    /// A transaction type update body that deserializes.
    const DESCRIPTION: &str = r#"{"description":"PURCHASE"}"#;

    /// The type update path with the CTLI query.
    const TYPE_LIST: &str = "/admin/transaction-types/01?origin=list";

    /// The client-written text of [`EXTRACTED`]: a numeric password, a
    /// numeric SSN part, a query value, a password and an account key.
    const EXTRACTED_TEXT: [&str; 4] = ["918273645", "TopSecret99", "PASSW0RD", "00000000001"];

    /// One request through [`extracting`]: (method, URI, content type, body,
    /// status, the refused request's `transaction`, or `None` when every
    /// extractor passes).
    type Extracted = (
        &'static str,
        &'static str,
        Option<&'static str>,
        &'static str,
        u16,
        Option<&'static str>,
    );

    /// The requests of
    /// `an_extractor_refusal_is_recorded_on_the_request_span_and_a_pass_is_not`.
    const EXTRACTED: [Extracted; 14] = [
        // A numeric password, malformed JSON, no JSON content type.
        (
            "POST",
            "/auth/login",
            JSON,
            r#"{"user_id":"USER0001","password":918273645}"#,
            400,
            Some("CC00"),
        ),
        (
            "POST",
            "/auth/login",
            JSON,
            r#"{"user_id":"#,
            400,
            Some("CC00"),
        ),
        ("POST", "/auth/login", None, LOGIN, 400, Some("CC00")),
        // A numeric SSN part.
        (
            "PUT",
            "/accounts/00000000001",
            JSON,
            r#"{"cust_ssn_1":918273645}"#,
            400,
            Some("CAUP"),
        ),
        // A path segment that is no UTF-8, into a string and into a struct.
        ("GET", "/accounts/%FF", None, "", 400, Some("CAVW")),
        ("GET", "/authorizations/%FF", None, "", 400, Some("CPVD")),
        // A query value that does not deserialize.
        (
            "GET",
            "/cards?page_size=TopSecret99",
            None,
            "",
            400,
            Some("CCLI"),
        ),
        // The shared type update: CTLI with `origin=list`, else CTTU.
        ("PUT", TYPE_LIST, JSON, "{", 400, Some("CTLI")),
        (
            "PUT",
            "/admin/transaction-types/01",
            JSON,
            "{",
            400,
            Some("CTTU"),
        ),
        (
            "PUT",
            "/admin/transaction-types/01?origin=TopSecret99",
            JSON,
            DESCRIPTION,
            400,
            Some("CTTU"),
        ),
        // Every extractor passes.
        ("POST", "/auth/login", JSON, LOGIN, 200, None),
        ("PUT", TYPE_LIST, JSON, DESCRIPTION, 200, None),
        ("GET", "/cards?page_size=7", None, "", 200, None),
        ("GET", "/accounts/00000000001", None, "", 200, None),
    ];

    #[tokio::test]
    async fn an_extractor_refusal_is_recorded_on_the_request_span_and_a_pass_is_not() {
        for (method, uri, content_type, body, status, refusal) in EXTRACTED {
            let case = format!("{method} {uri} {body}");
            let request = content_type
                .into_iter()
                .fold(
                    Request::builder().method(method).uri(uri),
                    |builder, value| builder.header(header::CONTENT_TYPE, value),
                )
                .body(Body::from(body))
                .expect("a valid request");
            let ((answered, _, answer), lines) = send_logged(extracting(), request).await;
            assert_eq!(answered.as_u16(), status, "{case}");
            if refusal.is_some() {
                let error: ErrorBody = serde_json::from_slice(&answer).expect("an error body");
                assert_eq!(error.code, "INVALID", "{case}");
            }

            // The stand-ins open no span, so the request span is the only one.
            let spans: Vec<&Value> = lines.iter().filter(|line| line["type"] == "span").collect();
            let [span] = spans.as_slice() else {
                panic!("{case}: one span expected, got {lines:?}");
            };
            assert_eq!(span["name"], "request", "{case}: {span}");
            let attributes = &span["attributes"];
            assert_eq!(attributes["method"], method, "{case}: {span}");
            assert_eq!(attributes["version"], "HTTP/1.1", "{case}: {span}");
            assert!(attributes["route"].is_string(), "{case}: {span}");
            assert!(attributes.get("user_id").is_none(), "{case}: {span}");
            if let Some(transaction) = refusal {
                assert_eq!(attributes["transaction"], transaction, "{case}: {span}");
                assert_eq!(attributes["outcome"], "rejected", "{case}: {span}");
            } else {
                assert!(attributes.get("transaction").is_none(), "{case}: {span}");
                assert!(attributes.get("outcome").is_none(), "{case}: {span}");
            }

            // No event but the trace layer's is written, and nothing the
            // client wrote reaches a line.
            for line in &lines {
                if line["type"] != "span" {
                    let target = line["target"].as_str().unwrap_or_default();
                    assert!(target.starts_with("tower_http::"), "{case}: {line}");
                }
                let text = line.to_string();
                for withheld in EXTRACTED_TEXT {
                    assert!(!text.contains(withheld), "{case}: {text}");
                }
            }
        }
    }

    /// The secret of the token keys [`signed_on_extracting`] verifies with.
    const SECRET: &str = "test-secret";

    /// The lifetime of the tokens the tests issue, in seconds.
    const TTL_SECS: u64 = 3600;

    /// `AuthUser`'s extraction as its `FromRequestParts` impl runs it,
    /// `authenticate`, on a router whose state is the token keys: an
    /// `AppState` cannot be built without a database handle.
    struct SignedOn;

    impl FromRequestParts<Arc<JwtKeys>> for SignedOn {
        type Rejection = ApiError;

        async fn from_request_parts(
            parts: &mut Parts,
            keys: &Arc<JwtKeys>,
        ) -> Result<Self, Self::Rejection> {
            authenticate(parts, keys).map(|_| Self)
        }
    }

    /// `AdminUser`'s extraction, as [`SignedOn`] stands for `AuthUser`'s:
    /// `authenticate`, then `require_admin`.
    struct SignedOnAdmin;

    impl FromRequestParts<Arc<JwtKeys>> for SignedOnAdmin {
        type Rejection = ApiError;

        async fn from_request_parts(
            parts: &mut Parts,
            keys: &Arc<JwtKeys>,
        ) -> Result<Self, Self::Rejection> {
            let user = authenticate(parts, keys)?;
            require_admin(parts, user).map(|_| Self)
        }
    }

    /// [`extracting`]'s operations with their extractors in the order the
    /// registry's handlers declare them: the signed-on user first, then the
    /// path, query and body. The public sign-on takes its body only.
    fn signed_on_extracting(keys: Arc<JwtKeys>) -> Router {
        let routes = Router::new()
            .route(
                "/auth/login",
                post(|ApiJson(_): ApiJson<LoginRequest>| async { "signed on" }),
            )
            .route(
                "/accounts/{id}",
                get(|_: SignedOn, ApiPath(_): ApiPath<String>| async { "account" }).put(
                    |_: SignedOn,
                     ApiPath(_): ApiPath<String>,
                     ApiJson(_): ApiJson<AccountUpdateRequest>| async {
                        "updated"
                    },
                ),
            )
            .route(
                "/cards",
                get(|_: SignedOn, ApiQuery(_): ApiQuery<PageQuery>| async { "cards" }),
            )
            .route(
                "/authorizations/{id}",
                get(|_: SignedOn, ApiPath(_): ApiPath<AuthIdPath>| async { "authorization" }),
            )
            .route(
                "/admin/transaction-types/{id}",
                put(
                    |_: SignedOnAdmin,
                     ApiPath(_): ApiPath<KeySegment>,
                     ApiQuery(_): ApiQuery<TranTypeUpdateQuery>,
                     ApiJson(_): ApiJson<TranTypeUpdateRequest>| async {
                        "type updated"
                    },
                ),
            )
            .with_state(keys);
        with_layers(routes, HeaderValue::from_static(ORIGIN))
    }

    /// The bearer token a request of [`SIGNED_ON`] presents.
    #[derive(Debug, Clone, Copy)]
    enum Token {
        /// USER0001's, of type `U`.
        User,
        /// ADMIN001's, of type `A`.
        Admin,
        /// ADMIN001's, signed with another secret.
        Forged,
    }

    /// An account update body with a numeric SSN part.
    const NUMERIC_SSN: &str = r#"{"cust_ssn_1":918273645}"#;

    /// One request through [`signed_on_extracting`]: (the token it presents,
    /// if any, method, URI, JSON body, empty for none, status, and the refused
    /// request's `transaction` and `user_id`, or `None` when every extractor
    /// passes).
    type SignedOnRequest = (
        Option<Token>,
        &'static str,
        &'static str,
        &'static str,
        u16,
        Option<(&'static str, Option<&'static str>)>,
    );

    /// The requests of
    /// `a_refusal_after_a_verified_token_records_its_user_id_and_no_other_does`.
    const SIGNED_ON: [SignedOnRequest; 15] = [
        // A body, path or query refused after the token verified.
        (
            Some(Token::User),
            "PUT",
            "/accounts/00000000001",
            NUMERIC_SSN,
            400,
            Some(("CAUP", Some("USER0001"))),
        ),
        (
            Some(Token::User),
            "GET",
            "/accounts/%FF",
            "",
            400,
            Some(("CAVW", Some("USER0001"))),
        ),
        (
            Some(Token::User),
            "GET",
            "/authorizations/%FF",
            "",
            400,
            Some(("CPVD", Some("USER0001"))),
        ),
        (
            Some(Token::User),
            "GET",
            "/cards?page_size=TopSecret99",
            "",
            400,
            Some(("CCLI", Some("USER0001"))),
        ),
        (
            Some(Token::Admin),
            "PUT",
            TYPE_LIST,
            "{",
            400,
            Some(("CTLI", Some("ADMIN001"))),
        ),
        (
            Some(Token::Admin),
            "PUT",
            "/admin/transaction-types/01?origin=TopSecret99",
            DESCRIPTION,
            400,
            Some(("CTTU", Some("ADMIN001"))),
        ),
        (
            Some(Token::Admin),
            "PUT",
            "/admin/transaction-types/%FF?origin=list",
            DESCRIPTION,
            400,
            Some(("CTLI", Some("ADMIN001"))),
        ),
        // The admin check refuses a user before the body is read.
        (
            Some(Token::User),
            "PUT",
            TYPE_LIST,
            "{",
            403,
            Some(("CTLI", Some("USER0001"))),
        ),
        // A token that does not verify, or none: refused before the body.
        (
            Some(Token::Forged),
            "PUT",
            "/accounts/00000000001",
            NUMERIC_SSN,
            401,
            Some(("CAUP", None)),
        ),
        (
            None,
            "GET",
            "/cards?page_size=TopSecret99",
            "",
            401,
            Some(("CCLI", None)),
        ),
        // The public sign-on verifies no token, not even a valid one.
        (
            Some(Token::User),
            "POST",
            "/auth/login",
            r#"{"user_id":"USER0001","password":918273645}"#,
            400,
            Some(("CC00", None)),
        ),
        // Every extractor passes.
        (
            Some(Token::User),
            "PUT",
            "/accounts/00000000001",
            "{}",
            200,
            None,
        ),
        (
            Some(Token::User),
            "GET",
            "/cards?page_size=7",
            "",
            200,
            None,
        ),
        (Some(Token::Admin), "PUT", TYPE_LIST, DESCRIPTION, 200, None),
        (Some(Token::User), "POST", "/auth/login", LOGIN, 200, None),
    ];

    /// The token `keys` issue for `user_id` of type `usr_type`.
    fn issued(keys: &JwtKeys, user_id: &str, usr_type: UserType) -> String {
        keys.issue(user_id, usr_type).expect("issues").token
    }

    #[tokio::test]
    async fn a_refusal_after_a_verified_token_records_its_user_id_and_no_other_does() {
        let keys = Arc::new(JwtKeys::new(SECRET, TTL_SECS));
        let user = issued(&keys, "USER0001", UserType::User);
        let admin = issued(&keys, "ADMIN001", UserType::Admin);
        let other_secret = JwtKeys::new("other-secret", TTL_SECS);
        let forged = issued(&other_secret, "ADMIN001", UserType::Admin);
        for (token, method, uri, body, status, refusal) in SIGNED_ON {
            let case = format!("{token:?} {method} {uri} {body}");
            let presented = token.map(|token| match token {
                Token::User => &user,
                Token::Admin => &admin,
                Token::Forged => &forged,
            });
            let mut builder = Request::builder().method(method).uri(uri);
            if let Some(presented) = presented {
                builder = builder.header(header::AUTHORIZATION, format!("Bearer {presented}"));
            }
            if !body.is_empty() {
                builder = builder.header(header::CONTENT_TYPE, "application/json");
            }
            let request = builder.body(Body::from(body)).expect("a valid request");
            let router = signed_on_extracting(Arc::clone(&keys));
            let ((answered, _, answer), lines) = send_logged(router, request).await;
            assert_eq!(answered.as_u16(), status, "{case}");
            if refusal.is_some() {
                let error: ErrorBody = serde_json::from_slice(&answer).expect("an error body");
                let code = match status {
                    401 => "UNAUTHENTICATED",
                    403 => "FORBIDDEN",
                    _ => "INVALID",
                };
                assert_eq!(error.code, code, "{case}");
            }

            // The stand-ins open no span, so the request span is the only one.
            let spans: Vec<&Value> = lines.iter().filter(|line| line["type"] == "span").collect();
            let [span] = spans.as_slice() else {
                panic!("{case}: one span expected, got {lines:?}");
            };
            assert_eq!(span["name"], "request", "{case}: {span}");
            let attributes = &span["attributes"];
            if let Some((transaction, user_id)) = refusal {
                assert_eq!(attributes["transaction"], transaction, "{case}: {span}");
                assert_eq!(attributes["outcome"], "rejected", "{case}: {span}");
                assert_eq!(
                    attributes.get("user_id").and_then(Value::as_str),
                    user_id,
                    "{case}: {span}"
                );
            } else {
                for field in ["transaction", "user_id", "outcome"] {
                    assert!(attributes.get(field).is_none(), "{case}: {span}");
                }
            }

            // Only a 401 or 403 writes an application event, its warning, and
            // no token, secret or client-written text reaches a line.
            let warned = lines
                .iter()
                .filter(|line| line["type"] != "span")
                .filter(|line| {
                    let target = line["target"].as_str().unwrap_or_default();
                    !target.starts_with("tower_http::")
                })
                .count();
            assert_eq!(warned, usize::from(matches!(status, 401 | 403)), "{case}");
            for line in &lines {
                let text = line.to_string();
                let tokens = [user.as_str(), admin.as_str(), forged.as_str()];
                for withheld in EXTRACTED_TEXT.into_iter().chain(tokens) {
                    assert!(!text.contains(withheld), "{case}: {text}");
                }
                for withheld in [SECRET, "other-secret"] {
                    assert!(!text.contains(withheld), "{case}: {text}");
                }
            }
        }
    }

    /// A shared in-memory sink standing in for standard output.
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("sink buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// Sends `request` through `router`, as [`send_to`] does, with the API's
    /// telemetry subscriber under the default filter `info` as this thread's
    /// subscriber, and returns the answer with the JSON lines it wrote.
    ///
    /// [`Recorder`] keeps no current span, so it cannot show what an
    /// extractor records on the span it runs in; this subscriber's registry
    /// does, and its lines are what the `api` binary writes to stdout.
    async fn send_logged(
        router: Router,
        request: Request<Body>,
    ) -> ((StatusCode, HeaderMap, Vec<u8>), Vec<Value>) {
        let sink = Sink::default();
        let writer = sink.clone();
        let (subscriber, guard) = subscriber_with_writer("api", "info", move || writer.clone())
            .expect("valid directives");
        let answer = {
            // `#[tokio::test]` polls the request on this thread only, so the
            // thread's default covers every span and event it makes.
            let _default = tracing::subscriber::set_default(subscriber);
            send_to(router, request).await
        };
        guard.shutdown().expect("the tracer provider shuts down");
        let bytes = sink.0.lock().expect("the sink buffer").clone();
        let text = String::from_utf8(bytes).expect("JSON lines are UTF-8");
        let lines = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect();
        (answer, lines)
    }

    /// What the work a handler hands to [`cancellable`] does in a case of
    /// `a_dropped_handler_records_cancelled_and_a_finished_one_its_own_outcome`.
    #[derive(Debug, Clone, Copy)]
    enum Work {
        /// Still waiting, on a row lock for example, when its client goes
        /// away and the future is dropped.
        Pending,
        /// Records this `outcome` on the handler span, then completes.
        Records(&'static str),
        /// Completes without recording; its caller then records this
        /// `outcome`.
        Answers(&'static str),
        /// Panics while it runs.
        Panics,
    }

    /// A handler span declaring `outcome`, as every handler span does.
    fn handler_span() -> Span {
        tracing::info_span!("handler", transaction = "CB00", outcome = Empty)
    }

    /// Runs `work` through [`cancellable`] on the handler span `span` as a
    /// handler does, then drops it as hyper drops the future of a request
    /// whose client went away: one poll, the outcome of work that answered
    /// without one recorded by the caller, then the drop, complete or not.
    fn run_cancellable(span: &Span, work: Work) {
        let recording = span.clone();
        let mut future = Box::pin(cancellable(span.clone(), async move {
            match work {
                Work::Pending => std::future::pending().await,
                Work::Records(outcome) => {
                    recording.record("outcome", outcome);
                }
                Work::Answers(_) => {}
                Work::Panics => panic!("the handler's work panicked"),
            }
        }));
        let mut context = Context::from_waker(Waker::noop());
        let polled = catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(&mut context)));
        match (&polled, work) {
            (Ok(Poll::Pending), Work::Pending)
            | (Ok(Poll::Ready(())), Work::Records(_))
            | (Err(_), Work::Panics) => {}
            (Ok(Poll::Ready(())), Work::Answers(outcome)) => {
                span.record("outcome", outcome);
            }
            _ => panic!("{work:?} polled to {polled:?}"),
        }
        drop(future);
    }

    /// The JSON lines the API's telemetry subscriber, under the default
    /// filter `info`, wrote while `f` ran with it as this thread's subscriber.
    fn logged(f: impl FnOnce()) -> Vec<Value> {
        let sink = Sink::default();
        let writer = sink.clone();
        let (subscriber, guard) = subscriber_with_writer("api", "info", move || writer.clone())
            .expect("valid directives");
        tracing::subscriber::with_default(subscriber, f);
        guard.shutdown().expect("the tracer provider shuts down");
        let bytes = sink.0.lock().expect("the sink buffer").clone();
        let text = String::from_utf8(bytes).expect("JSON lines are UTF-8");
        text.lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect()
    }

    /// What [`Recorder`] saw on this thread while `f` ran, as [`capturing`]
    /// gives it for a future.
    fn captured(f: impl FnOnce()) -> Vec<Seen> {
        install_recorder();
        CAPTURE.set(Some(Vec::new()));
        f();
        CAPTURE.take().expect("the capture this call began")
    }

    #[test]
    fn a_dropped_handler_records_cancelled_and_a_finished_one_its_own_outcome() {
        // (the handler's work, the `outcome` its span ends with)
        let cases = [
            (Work::Pending, Some("cancelled")),
            (Work::Records("committed"), Some("committed")),
            (Work::Answers("ok"), Some("ok")),
            // A panic is no cancellation, and its span records no outcome.
            (Work::Panics, None),
        ];
        for (work, expected) in cases {
            // The span the exporter writes, as the `api` binary writes it to
            // stdout once the dropped future has closed it.
            let lines = logged(|| run_cancellable(&handler_span(), work));
            let spans: Vec<&Value> = lines.iter().filter(|line| line["type"] == "span").collect();
            let [span] = spans.as_slice() else {
                panic!("{work:?}: one span expected, got {lines:?}");
            };
            assert_eq!(span["name"], "handler", "{work:?}: {span}");
            assert_eq!(
                span["attributes"]["transaction"], "CB00",
                "{work:?}: {span}"
            );
            assert_eq!(
                span["attributes"].get("outcome").and_then(Value::as_str),
                expected,
                "{work:?}: {span}"
            );

            // Every value recorded on the span: the exporter keeps only the
            // last value of a field, so `outcome` must be recorded once.
            let seen = captured(|| run_cancellable(&handler_span(), work));
            let recorded: Vec<&str> = seen
                .iter()
                .filter(|entry| entry.span.is_some() && entry.name == "handler")
                .flat_map(|entry| &entry.values)
                .filter(|(field, _)| *field == "outcome")
                .map(|(_, value)| value.as_str())
                .collect();
            assert_eq!(recorded, expected.as_slice(), "{work:?}: {seen:?}");
        }
    }
}
