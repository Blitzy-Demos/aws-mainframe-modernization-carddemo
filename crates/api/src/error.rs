//! The HTTP error mapping (AAP 0.6.4): the only place in the workspace where a
//! domain [`ErrorKind`] becomes an HTTP status, and the only place the error
//! body `{code, message, fields?}` is built.
//!
//! Every handler returns `Result<_, ApiError>`. An [`ApiError`] comes from a
//! [`DomainError`] (the rule's kind, its COBOL message and the BMS fields the
//! message concerns), from one of the request-written failures (no valid
//! token, a non-admin caller, an unexpected failure), or from a request whose
//! body, query or path does not deserialize.
//!
//! # Status table
//!
//! | `ErrorKind` | Status |
//! |---|---|
//! | `Invalid`, `Empty` | 400 Bad Request |
//! | `NotFound` | 404 Not Found |
//! | `Duplicate`, `Conflict` | 409 Conflict |
//! | `SignonFailed` | 401 Unauthorized |
//! | `Unexpected` | 500 Internal Server Error |
//!
//! Outside the kinds, a missing, malformed, expired or badly signed bearer
//! token is 401 with code `UNAUTHENTICATED`, a `usr_type` `U` token on
//! `/admin/**` is 403 with code `FORBIDDEN`, and a request that does not
//! deserialize is 400 with code `INVALID`. The router's own failures take the
//! same body: a path no route serves is 404 with the `NotFound` code
//! `NOT_FOUND`, and a method a routed path does not serve is 405 with code
//! `METHOD_NOT_ALLOWED`, beside the `Allow` header axum sets.
//!
//! Every 401, the `UNAUTHENTICATED` answer and the `SignonFailed` kind alike,
//! carries the challenge `WWW-Authenticate: Bearer` (RFC 9110 §15.5.2); no
//! other status carries a `WWW-Authenticate` header. The body is the same with
//! or without it.
//!
//! The table is the "Status mapping" of `docs/parity/validation-audit.md`, and
//! every row's `Status` column follows from the row's kind through it. Where a
//! row and its kind disagree, the rule's kind is corrected in
//! `carddemo_domain`; this module never special-cases a program or a message.
//!
//! # Messages
//!
//! `message` is the domain message unchanged: byte-identical to the audited
//! COBOL text, trailing spaces included, never trimmed. A 500 carries the COBOL
//! message the program shows for that failure when the rule supplies one, else
//! [`messages::UNEXPECTED_ERROR`]. The detail of a database, port or reward
//! store failure goes to the log through `tracing`, never to the body, and a
//! rule's `Unexpected` error is logged at ERROR with its code, message and
//! fields in the span of the handler answering it. An input class a rule holds
//! at `halt` is answered through [`ApiError::halted`], whose one ERROR event
//! also carries `cause` `halt` and the escalation ID that holds the class, the
//! cause the answer's [`messages::UNEXPECTED_ERROR`] does not state. Every 500
//! answering a rule's error is logged by exactly one event.
//!
//! # Extractors
//!
//! axum's own `Json`, `Query` and `Path` answer a rejected request with a
//! plain-text body. [`ApiJson`], [`ApiQuery`] and [`ApiPath`] wrap them and
//! answer with the error body instead: status 400, code `INVALID`, and the
//! rejection's text as `message`. That text describes the malformed input and
//! holds no server detail. A request body must be a JSON object, the
//! `type: object` every request DTO's schema declares: [`ApiJson`] rejects any
//! other JSON value, an array included, with 400 `INVALID`
//! (`invalid type: sequence, expected a JSON object at line 1 column 0`), so
//! no field is ever bound by position. A JSON body that parses but does not
//! fit its type is the one rejection whose text serde may word after a Rust
//! type (`expected struct <Name>`, `expected tuple struct <Name> with 2
//! elements`): its `message` keeps the JSON path, the value received and the
//! line and column, and names the JSON that type accepts in place of the type
//! (`expected a JSON object`, `expected a JSON array of 2 elements`). Handlers
//! declare their request body and parameters explicitly in
//! `#[utoipa::path(request_body = .., params(..))]`, so the
//! wrappers leave the `OpenAPI` document unchanged.
//!
//! A refused request never reaches its handler, so no handler span opens for
//! it. Each wrapper therefore records the refusal on the request span through
//! `crate::record_refusal`: the `transaction` of the operation the request
//! asks for, `outcome` `rejected`, and the `user_id` of the user an earlier
//! [`AuthUser`](crate::auth::AuthUser) or [`AdminUser`](crate::auth::AdminUser)
//! extractor verified and kept in the request's extensions. A request whose
//! token did not verify never reaches these wrappers, and a request to the
//! public `POST /auth/login` has no verified user, so neither records one.
//! The rejection's text is neither recorded nor logged, because it can quote
//! what the client wrote, such as a body value.
//!
//! # Success statuses
//!
//! [`Created`] answers a stored new resource with 201 and the resource as its
//! JSON body, so a handler that returns it names the kind of success and
//! chooses no status itself. Handlers still declare the 201 in
//! `#[utoipa::path(responses(..))]`.

use std::fmt;
use std::marker::PhantomData;

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::header::WWW_AUTHENTICATE;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use carddemo_domain::error::DomainError;
use carddemo_domain::messages;
use carddemo_domain::rules::{ErrorKind, FieldError, PortError, RewardError};
use carddemo_persistence::tx::PersistenceError;
use serde::de::value::MapAccessDeserializer;
use serde::de::{DeserializeOwned, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::auth::extract::verified_user;

/// The `code` of a request without a valid bearer token.
const AUTH_REQUIRED_CODE: &str = "UNAUTHENTICATED";

/// The `code` of a caller whose token is not an admin's, on `/admin/**`.
const FORBIDDEN_CODE: &str = "FORBIDDEN";

/// The `code` of a method a routed path does not serve.
const METHOD_NOT_ALLOWED_CODE: &str = "METHOD_NOT_ALLOWED";

/// The `WWW-Authenticate` challenge of every 401: the `Bearer` scheme the API
/// authenticates requests with (RFC 6750 §3), without parameters.
const BEARER_CHALLENGE: &str = "Bearer";

/// One field a failure concerns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorFieldBody {
    /// The BMS symbolic field name the COBOL program places the cursor on,
    /// without its suffix (for example `ACCTSID`, `USERID`, `PASSWD`), or the
    /// request's own field name for a reward rule (`reward_rate`,
    /// `min_transaction_amount`, `active_to`). The `<Dto>Field` enum of each
    /// request DTO in `crate::dto` lists the legal values.
    pub field: String,
    /// The text shown for this field.
    pub message: String,
}

impl From<FieldError> for ErrorFieldBody {
    fn from(entry: FieldError) -> Self {
        Self {
            field: entry.field.to_owned(),
            message: entry.message,
        }
    }
}

/// The body of every error response: `{code, message, fields?}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    /// The failure class: an error kind code (`INVALID`, `EMPTY`,
    /// `NOT_FOUND`, `DUPLICATE`, `CONFLICT`, `SIGNON_FAILED`, `UNEXPECTED`),
    /// `UNAUTHENTICATED`, `FORBIDDEN` or `METHOD_NOT_ALLOWED`. A path no route
    /// serves is `NOT_FOUND`.
    pub code: String,
    /// The message text, byte-identical to the audited COBOL message or the
    /// request-written text. Empty for a path that sets no message and whose
    /// escalation names no text.
    pub message: String,
    /// The fields the message concerns, in the order the COBOL program places
    /// the cursor on them. Omitted when the message concerns no field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<ErrorFieldBody>,
}

/// A failed request: the HTTP status and the error body sent with it.
///
/// Displays as the body's `message`, so `?` composes in tests.
#[derive(Debug, thiserror::Error)]
#[error("{}", .body.message)]
pub struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

/// The status of each [`ErrorKind`]: the table in the module documentation.
const fn status_for(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::Invalid | ErrorKind::Empty => StatusCode::BAD_REQUEST,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Duplicate | ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::SignonFailed => StatusCode::UNAUTHORIZED,
        ErrorKind::Unexpected => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl ApiError {
    fn new(status: StatusCode, code: &str, message: String, fields: Vec<ErrorFieldBody>) -> Self {
        Self {
            status,
            body: ErrorBody {
                code: code.to_owned(),
                message,
                fields,
            },
        }
    }

    /// 401 for a missing, malformed, expired or badly signed bearer token,
    /// with [`messages::AUTHENTICATION_REQUIRED`] (AAP 0.6.1).
    #[must_use]
    pub fn unauthenticated() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            AUTH_REQUIRED_CODE,
            messages::AUTHENTICATION_REQUIRED.to_owned(),
            Vec::new(),
        )
    }

    /// 403 for a `usr_type` `U` token on `/admin/**`, with
    /// [`messages::ADMIN_ACCESS_REQUIRED`] (AAP 0.6.1).
    #[must_use]
    pub fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            FORBIDDEN_CODE,
            messages::ADMIN_ACCESS_REQUIRED.to_owned(),
            Vec::new(),
        )
    }

    /// 404 for a path no route serves, with the `NotFound` kind's status and
    /// code and [`messages::ROUTE_NOT_FOUND`]: the router's fallback answer.
    /// It names neither the path nor the method requested.
    #[must_use]
    pub fn route_not_found() -> Self {
        Self::new(
            status_for(ErrorKind::NotFound),
            ErrorKind::NotFound.code(),
            messages::ROUTE_NOT_FOUND.to_owned(),
            Vec::new(),
        )
    }

    /// 405 for a method a routed path does not serve, with
    /// [`messages::METHOD_NOT_ALLOWED`]: the method routers' fallback answer,
    /// to which axum adds the `Allow` header of the path. It names neither the
    /// path nor the method requested.
    #[must_use]
    pub fn method_not_allowed() -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            METHOD_NOT_ALLOWED_CODE,
            messages::METHOD_NOT_ALLOWED.to_owned(),
            Vec::new(),
        )
    }

    /// 500 with [`messages::UNEXPECTED_ERROR`] and no fields: a failure for
    /// which no rule chose a COBOL message. It never carries internal detail.
    #[must_use]
    pub fn unexpected() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            ErrorKind::Unexpected.code(),
            messages::UNEXPECTED_ERROR.to_owned(),
            Vec::new(),
        )
    }

    /// The response to a path that `docs/parity/validation-audit.md` records
    /// as `no-message (E-nn)`, such as the missing `version` token of
    /// `PUT /accounts/{id}` and `PUT /cards/{id}` (E-24).
    ///
    /// `message` is the `carddemo_domain::messages` constant the escalation
    /// decision names, or `""` when it names none. The status comes from
    /// `kind` through the status table.
    #[must_use]
    pub fn no_message(kind: ErrorKind, message: &'static str, fields: Vec<ErrorFieldBody>) -> Self {
        Self::new(status_for(kind), kind.code(), message.to_owned(), fields)
    }

    /// The answer to `err`, a rule's answer to an input class it holds at
    /// `halt`, with `escalation_id`, the ID of the `docs/parity/escalations.md`
    /// entry that holds it, for example `E-26`.
    ///
    /// The status, code, message and fields are those [`From<DomainError>`]
    /// gives `err`, usually 500 with [`messages::UNEXPECTED_ERROR`] and no
    /// fields; the escalation never reaches the body. The answer is logged by
    /// one ERROR event, `unexpected failure answered`, inside the span of the
    /// handler converting it, with the fields of the event [`From<DomainError>`]
    /// writes (`code`, `reason`, `fields`) plus `cause` `halt` and
    /// `escalation_id`, whatever `err`'s kind, because the answer never states
    /// the halt's cause.
    #[must_use]
    pub fn halted(err: DomainError, escalation_id: &'static str) -> Self {
        tracing::error!(
            code = err.kind.code(),
            reason = err.message.trim_end_matches(' '),
            fields = field_names(&err.fields).as_str(),
            cause = "halt",
            escalation_id,
            "unexpected failure answered"
        );
        Self::answer(err)
    }

    /// `err` answered with its kind's status and code, and its message and
    /// fields unchanged, without a log event.
    fn answer(err: DomainError) -> Self {
        let DomainError {
            kind,
            message,
            fields,
            ..
        } = err;
        // The code is taken from the kind, as the status is, so the two can
        // never disagree; `DomainError::code` holds the same text.
        Self::new(
            status_for(kind),
            kind.code(),
            message,
            fields.into_iter().map(ErrorFieldBody::from).collect(),
        )
    }

    /// 400 for a request body, query or path that does not deserialize, with
    /// the extractor's rejection text as `message` and no fields.
    ///
    /// Every such request is an `Invalid` input, so it is 400 whatever status
    /// axum would give the rejection itself, such as 415 for a body without a
    /// JSON content type or 422 for a body of the wrong shape.
    #[must_use]
    pub fn rejection(text: String) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            ErrorKind::Invalid.code(),
            text,
            Vec::new(),
        )
    }

    /// The HTTP status of the response.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }

    /// The error body of the response.
    #[must_use]
    pub const fn body(&self) -> &ErrorBody {
        &self.body
    }
}

/// A rule's error answered with its kind's status and code, and its message
/// and fields unchanged.
///
/// An [`ErrorKind::Unexpected`] error is a failure the request's input did not
/// cause, so, as the adapter failures below are, it is also logged at ERROR,
/// inside the span of the handler converting it: `code`, `reason` (the message
/// without its trailing padding) and `fields` (the BMS names of its fields,
/// joined with commas). The message is a `carddemo_domain::messages` text the
/// response body carries as well, never request input, so the event holds
/// nothing the answer does not. Every other kind answers the request's own
/// input and is not logged. An input class held at `halt` is answered through
/// [`ApiError::halted`] instead, which writes the one event of its answer.
impl From<DomainError> for ApiError {
    fn from(err: DomainError) -> Self {
        if err.kind == ErrorKind::Unexpected {
            tracing::error!(
                code = err.kind.code(),
                reason = err.message.trim_end_matches(' '),
                fields = field_names(&err.fields).as_str(),
                "unexpected failure answered"
            );
        }
        Self::answer(err)
    }
}

/// The BMS names of `fields`, in order, joined with commas: the `fields` of
/// an `unexpected failure answered` event.
fn field_names(fields: &[FieldError]) -> String {
    let names: Vec<&str> = fields.iter().map(|entry| entry.field).collect();
    names.join(",")
}

impl From<PersistenceError> for ApiError {
    fn from(err: PersistenceError) -> Self {
        tracing::error!(error = %err, "persistence operation failed");
        Self::unexpected()
    }
}

impl From<PortError> for ApiError {
    fn from(err: PortError) -> Self {
        tracing::error!(error = %err, "port call failed");
        Self::unexpected()
    }
}

/// A reward accrual failure rolls back the CT02 unit of work and answers 500
/// (AAP 0.3.5).
impl From<RewardError> for ApiError {
    fn from(err: RewardError) -> Self {
        tracing::error!(error = %err, "reward accrual failed");
        Self::unexpected()
    }
}

/// The words before the expectation in serde's `invalid type`, `invalid
/// value` and `invalid length` messages.
const EXPECTED: &str = ", expected ";

/// The expectations serde's derive writes after a Rust type: the form that
/// precedes the type's name (`Type`, or `Type::Variant` for a variant), and
/// the JSON that type accepts, which replaces the whole expectation. The
/// positional form of a struct, tuple or variant (`… with 7 elements`)
/// becomes `a JSON array of 7 elements` instead.
///
/// An externally tagged enum's `enum Type` is absent: `serde_json` answers
/// such an enum given neither a string nor an object with the syntax error
/// `expected value`, so that expectation never reaches a rejection.
const TYPE_EXPECTATIONS: [(&str, &str); 7] = [
    ("struct ", "a JSON object"),
    ("struct variant ", "a JSON object"),
    ("tuple struct ", "a JSON array"),
    ("tuple variant ", "a JSON array"),
    ("unit struct ", "null"),
    (
        "internally tagged enum ",
        "a JSON object with a variant tag",
    ),
    (
        "adjacently tagged enum ",
        "a JSON object with a variant tag and content",
    ),
];

/// The whole messages serde writes around a Rust type's name: the text
/// before and after the name, and the message that replaces them.
const TYPE_MESSAGES: [(&str, &str, &str); 2] = [
    (
        "data did not match any variant of untagged enum ",
        "",
        "data did not match any variant",
    ),
    (
        "no variant of enum ",
        " found in flattened data",
        "no variant found in flattened data",
    ),
];

/// The text of a JSON body that parses but does not fit the request DTO,
/// with the Rust type name serde put in it replaced by the JSON that type
/// accepts.
///
/// axum writes the text as `<preamble>: [<JSON path>: ]<serde message> at
/// line L column C`. Everything but a type name at the end of the serde
/// message is kept: the JSON path of wire names, the value received, wire
/// field and variant names and the position. A message that names no type,
/// such as `expected a string` or a rule's own text, is kept whole.
fn public_data_error(text: &str) -> String {
    let (message, position) = split_position(text);
    let public = public_message(message)
        .or_else(|| {
            let (head, expected) = message.rsplit_once(EXPECTED)?;
            public_expectation(expected).map(|json| format!("{head}{EXPECTED}{json}"))
        })
        .unwrap_or_else(|| message.to_owned());
    format!("{public}{position}")
}

/// `text` split before the ` at line L column C` that ends a located
/// `serde_json` error, or whole with an empty position.
fn split_position(text: &str) -> (&str, &str) {
    text.rsplit_once(" at line ")
        .filter(|(_, location)| {
            location
                .split_once(" column ")
                .is_some_and(|(line, column)| is_digits(line) && is_digits(column))
        })
        .map_or((text, ""), |(message, _)| text.split_at(message.len()))
}

/// `message` with a [`TYPE_MESSAGES`] form at its end replaced, if it ends
/// with one around a type name.
fn public_message(message: &str) -> Option<String> {
    TYPE_MESSAGES.iter().find_map(|&(before, after, public)| {
        let (head, rest) = message.rsplit_once(before)?;
        let name = rest.strip_suffix(after)?;
        is_type_name(name).then(|| format!("{head}{public}"))
    })
}

/// The JSON a [`TYPE_EXPECTATIONS`] expectation stands for, or `None` for an
/// expectation that names no type.
fn public_expectation(expected: &str) -> Option<String> {
    let (named, elements) = expected
        .rsplit_once(" with ")
        .filter(|&(_, count)| {
            count == "1 element" || count.strip_suffix(" elements").is_some_and(is_digits)
        })
        .map_or((expected, None), |(named, count)| (named, Some(count)));
    TYPE_EXPECTATIONS.iter().find_map(|&(form, json)| {
        let name = named.strip_prefix(form)?;
        is_type_name(name).then(|| {
            elements.map_or_else(
                || json.to_owned(),
                |count| format!("a JSON array of {count}"),
            )
        })
    })
}

/// Whether `text` is one or more ASCII digits.
fn is_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `name` is a type name as serde's derive writes it: a Rust
/// identifier, or `Type::Variant`.
fn is_type_name(name: &str) -> bool {
    name.split_once("::").map_or_else(
        || is_identifier(name),
        |(type_name, variant)| is_identifier(type_name) && is_identifier(variant),
    )
}

/// Whether `text` is a Rust identifier, raw (`r#type`) or not.
fn is_identifier(text: &str) -> bool {
    let mut chars = text.strip_prefix("r#").unwrap_or(text).chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_alphabetic())
        && chars.all(|next| next == '_' || next.is_alphanumeric())
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        let text = rejection.body_text();
        // Only a body that parses but does not fit the DTO is worded after a
        // Rust type. A syntax error, a missing JSON content type and a body
        // that cannot be read are described without one.
        if matches!(rejection, JsonRejection::JsonDataError(_)) {
            Self::rejection(public_data_error(&text))
        } else {
            Self::rejection(text)
        }
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::rejection(rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::rejection(rejection.body_text())
    }
}

impl IntoResponse for ApiError {
    /// The status and the JSON error body, plus the `WWW-Authenticate:
    /// Bearer` challenge on a 401, which RFC 9110 §15.5.2 requires of every
    /// 401 response.
    fn into_response(self) -> Response {
        let Self { status, body } = self;
        let mut response = (status, Json(body)).into_response();
        if status == StatusCode::UNAUTHORIZED {
            response
                .headers_mut()
                .insert(WWW_AUTHENTICATE, HeaderValue::from_static(BEARER_CHALLENGE));
        }
        response
    }
}

/// The answer to a request that stored a new resource: 201 Created with the
/// resource as its JSON body, serialized as `axum::Json` serializes it.
///
/// A handler returns `Created(resource)`, so it chooses no status itself.
#[derive(Debug, Clone, Copy)]
pub struct Created<T>(pub T);

impl<T: Serialize> IntoResponse for Created<T> {
    fn into_response(self) -> Response {
        (StatusCode::CREATED, Json(self.0)).into_response()
    }
}

/// A JSON request body, extracted as `axum::Json` does, whose rejection is the
/// 400 error body, recorded first on the request span
/// (`crate::record_refusal`).
///
/// The body must be a JSON object, the `type: object` every request DTO's
/// schema declares. Any other JSON value, an array included, is rejected with
/// 400 `INVALID` (`invalid type: sequence, expected a JSON object`), so a
/// struct's fields are never bound by position.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    /// The request's operation and verified user are read before
    /// `axum::Json` consumes the request, so a refusal can still name them.
    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let transaction = crate::transaction_of(req.method(), req.uri(), req.extensions());
        let user_id = verified_user(req.extensions()).map(|user| user.user_id.clone());
        Json::<ObjectBody<T>>::from_request(req, state)
            .await
            .map(|Json(ObjectBody(value))| Self(value))
            .map_err(|rejection| {
                crate::record_refusal(transaction, user_id.as_deref());
                rejection.into()
            })
    }
}

/// A `T` deserialized from a JSON object only.
///
/// serde's derive also accepts a struct as a sequence of its field values in
/// declaration order. This wrapper asks the deserializer for a map, so any
/// other JSON value fails as a data error that expects `a JSON object`, and a
/// map is handed to `T` whole. The JSON path, value and position of an error
/// inside the object are the ones `T` itself reports.
struct ObjectBody<T>(T);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ObjectBody<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer
            .deserialize_map(ObjectVisitor(PhantomData))
            .map(Self)
    }
}

/// The visitor of [`ObjectBody`]: it accepts a map and nothing else.
struct ObjectVisitor<T>(PhantomData<T>);

impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        T::deserialize(MapAccessDeserializer::new(map))
    }
}

/// A query string, extracted as `axum::extract::Query` does, whose rejection
/// is the 400 error body, recorded first on the request span
/// (`crate::record_refusal`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|rejection| {
                crate::record_refusal(
                    crate::transaction_of(&parts.method, &parts.uri, &parts.extensions),
                    verified_user(&parts.extensions).map(|user| user.user_id.as_str()),
                );
                rejection.into()
            })
    }
}

/// Path parameters, extracted as `axum::extract::Path` does, whose rejection
/// is the 400 error body, recorded first on the request span
/// (`crate::record_refusal`).
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|rejection| {
                crate::record_refusal(
                    crate::transaction_of(&parts.method, &parts.uri, &parts.extensions),
                    verified_user(&parts.extensions).map(|user| user.user_id.as_str()),
                );
                rejection.into()
            })
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
    use axum::extract::{Json, Path, Query};
    use axum::http::{HeaderValue, Request, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{get, post};
    use carddemo_domain::error::{DomainError, ErrorKind};
    use carddemo_domain::messages;
    use carddemo_domain::ports::PortError;
    use carddemo_domain::rewards::RewardError;
    use carddemo_persistence::PersistenceError;
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tower::ServiceExt;
    use utoipa::PartialSchema;

    use super::{ApiError, ApiJson, ApiPath, ApiQuery, Created, ErrorBody, ErrorFieldBody};

    /// The status table, written out independently of `status_for`.
    const KIND_STATUSES: [(ErrorKind, StatusCode); 7] = [
        (ErrorKind::Invalid, StatusCode::BAD_REQUEST),
        (ErrorKind::Empty, StatusCode::BAD_REQUEST),
        (ErrorKind::NotFound, StatusCode::NOT_FOUND),
        (ErrorKind::Duplicate, StatusCode::CONFLICT),
        (ErrorKind::Conflict, StatusCode::CONFLICT),
        (ErrorKind::SignonFailed, StatusCode::UNAUTHORIZED),
        (ErrorKind::Unexpected, StatusCode::INTERNAL_SERVER_ERROR),
    ];

    /// Adapter detail that must reach the log only.
    const DETAIL: &str = "connection reset by 10.0.0.7:5432 while reading acct_curr_bal";

    fn field(name: &str, message: &str) -> ErrorFieldBody {
        ErrorFieldBody {
            field: name.to_owned(),
            message: message.to_owned(),
        }
    }

    async fn read_body(response: Response) -> (StatusCode, Option<String>, Vec<u8>) {
        let status = response.status();
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("the body is buffered");
        (status, content_type, bytes.to_vec())
    }

    #[test]
    fn every_kind_maps_to_its_status_and_code() {
        for (kind, status) in KIND_STATUSES {
            let error = ApiError::from(DomainError::new(kind, messages::CCDA_MSG_INVALID_KEY));
            assert_eq!(error.status(), status, "{kind:?}");
            assert_eq!(error.body().code, kind.code(), "{kind:?}");
            assert_eq!(error.body().message, messages::CCDA_MSG_INVALID_KEY);
            assert!(error.body().fields.is_empty());

            let silent = ApiError::no_message(kind, "", vec![field("ACTIDIN", "")]);
            assert_eq!(silent.status(), status, "{kind:?}");
            assert_eq!(silent.body().code, kind.code(), "{kind:?}");
            assert_eq!(silent.body().message, "");
            assert_eq!(silent.body().fields, vec![field("ACTIDIN", "")]);
        }
    }

    #[test]
    fn domain_message_and_fields_pass_through_byte_identical() {
        let error = ApiError::from(
            DomainError::signon_failed(messages::CCDA_MSG_THANK_YOU)
                .with_field("USERID")
                .with_field_message("PASSWD", messages::CCDA_MSG_INVALID_KEY),
        );
        assert_eq!(error.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(error.body().message, messages::CCDA_MSG_THANK_YOU);
        assert!(error.body().message.ends_with("      "));
        assert_eq!(
            error.body().fields,
            vec![
                field("USERID", messages::CCDA_MSG_THANK_YOU),
                field("PASSWD", messages::CCDA_MSG_INVALID_KEY),
            ]
        );
        assert_eq!(error.to_string(), messages::CCDA_MSG_THANK_YOU);

        let unexpected = ApiError::from(DomainError::unexpected(
            messages::UNABLE_TO_ADD_BILL_PAY_TRANSACTION,
        ));
        assert_eq!(unexpected.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            unexpected.body().message,
            messages::UNABLE_TO_ADD_BILL_PAY_TRANSACTION
        );
    }

    #[test]
    fn fields_are_omitted_from_json_only_when_empty() {
        let bare = ErrorBody {
            code: ErrorKind::Conflict.code().to_owned(),
            message: messages::RECORD_CHANGED_BY_SOME_ONE_ELSE.to_owned(),
            fields: Vec::new(),
        };
        let bare_json = json!({
            "code": "CONFLICT",
            "message": messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
        });
        assert_eq!(serde_json::to_value(&bare).expect("serializes"), bare_json);
        assert_eq!(
            serde_json::from_value::<ErrorBody>(bare_json).expect("deserializes"),
            bare
        );

        let with_field = ErrorBody {
            fields: vec![field("ACCTSID", messages::RECORD_CHANGED_BY_SOME_ONE_ELSE)],
            ..bare
        };
        assert_eq!(
            serde_json::to_value(&with_field).expect("serializes"),
            json!({
                "code": "CONFLICT",
                "message": messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
                "fields": [{
                    "field": "ACCTSID",
                    "message": messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
                }],
            })
        );
    }

    #[test]
    fn openapi_schema_requires_code_and_message_only() {
        let schema = serde_json::to_value(ErrorBody::schema()).expect("serializes");
        assert_eq!(schema["required"], json!(["code", "message"]));
        assert_eq!(schema["properties"]["fields"]["type"], json!("array"));
        let field_schema = serde_json::to_value(ErrorFieldBody::schema()).expect("serializes");
        assert_eq!(field_schema["required"], json!(["field", "message"]));
    }

    #[test]
    fn request_written_failures_have_their_status_code_and_text() {
        let cases = [
            (
                ApiError::unauthenticated(),
                StatusCode::UNAUTHORIZED,
                "UNAUTHENTICATED",
                messages::AUTHENTICATION_REQUIRED,
            ),
            (
                ApiError::forbidden(),
                StatusCode::FORBIDDEN,
                "FORBIDDEN",
                messages::ADMIN_ACCESS_REQUIRED,
            ),
            (
                ApiError::route_not_found(),
                StatusCode::NOT_FOUND,
                "NOT_FOUND",
                messages::ROUTE_NOT_FOUND,
            ),
            (
                ApiError::method_not_allowed(),
                StatusCode::METHOD_NOT_ALLOWED,
                "METHOD_NOT_ALLOWED",
                messages::METHOD_NOT_ALLOWED,
            ),
            (
                ApiError::unexpected(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "UNEXPECTED",
                messages::UNEXPECTED_ERROR,
            ),
            (
                ApiError::rejection(DETAIL.to_owned()),
                StatusCode::BAD_REQUEST,
                "INVALID",
                DETAIL,
            ),
        ];
        for (error, status, code, message) in cases {
            assert_eq!(error.status(), status, "{code}");
            assert_eq!(
                error.body(),
                &ErrorBody {
                    code: code.to_owned(),
                    message: message.to_owned(),
                    fields: Vec::new(),
                }
            );
        }
    }

    #[test]
    fn adapter_failures_answer_unexpected_without_their_detail() {
        let unavailable = || PortError::Unavailable(DETAIL.to_owned());
        let errors = [
            ApiError::from(unavailable()),
            ApiError::from(PortError::NotFound),
            ApiError::from(PersistenceError::Port(unavailable())),
            ApiError::from(PersistenceError::Corrupt {
                table: "accounts",
                column: "acct_curr_bal",
                reason: DETAIL.to_owned(),
            }),
            ApiError::from(RewardError::Store(unavailable())),
            ApiError::from(RewardError::PointsOverflow),
        ];
        let expected = ApiError::unexpected();
        for error in errors {
            assert_eq!(error.status(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(error.body(), expected.body());
            let text = serde_json::to_string(error.body()).expect("serializes");
            assert!(!text.contains(DETAIL), "{text}");
            assert!(!error.to_string().contains(DETAIL));
        }
    }

    /// A shared in-memory sink standing in for standard output.
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| std::io::Error::other("capture buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// `error` converted inside a `CAUP` handler span under the API's
    /// telemetry subscriber and its default filter `info`, with every JSON line
    /// the subscriber wrote: the event lines and the span line.
    fn converted_in_handler_span(error: DomainError) -> (ApiError, Vec<Value>) {
        answered_in_handler_span("info", || ApiError::from(error))
    }

    /// The error `answer` builds inside a `CAUP` handler span under the API's
    /// telemetry subscriber and the filter `directives`, with every JSON line
    /// the subscriber wrote: the event lines and any span line.
    fn answered_in_handler_span(
        directives: &str,
        answer: impl FnOnce() -> ApiError,
    ) -> (ApiError, Vec<Value>) {
        let capture = Capture::default();
        let sink = capture.clone();
        let (subscriber, guard) =
            carddemo_runtime::telemetry::subscriber_with_writer("api", directives, move || {
                sink.clone()
            })
            .expect("valid directives");
        let converted = tracing::subscriber::with_default(subscriber, || {
            tracing::info_span!(
                "update_account",
                transaction = "CAUP",
                user_id = "ADMIN001",
                account_id = "00000000001",
            )
            .in_scope(answer)
        });
        guard.shutdown().expect("the tracer provider shuts down");
        let bytes = capture.0.lock().expect("capture buffer").clone();
        let text = String::from_utf8(bytes).expect("JSON lines are UTF-8");
        let lines = text
            .lines()
            .map(|line| serde_json::from_str(line).expect("a JSON line"))
            .collect();
        (converted, lines)
    }

    /// The lines of `lines` that are log events, not span lines.
    fn events(lines: &[Value]) -> Vec<&Value> {
        lines.iter().filter(|line| line["type"] != "span").collect()
    }

    /// The status the span line of `lines` carries.
    fn span_status(lines: &[Value]) -> &Value {
        let spans: Vec<&Value> = lines.iter().filter(|line| line["type"] == "span").collect();
        let [span] = spans.as_slice() else {
            panic!("one span line expected, got {lines:?}");
        };
        &span["status"]
    }

    #[test]
    fn an_unexpected_domain_error_is_logged_at_error_in_its_handler_span_with_its_reason() {
        // A read failure with its composed, padded file-error text on one
        // field, a TRAN-ID failure on one field, and a halt answer (E-26)
        // that names no field.
        let file_error = messages::file_error_message("READ", "ACCTDAT", 13, 0);
        let cases = [
            (
                DomainError::unexpected(file_error.clone()).with_field("ACCTSID"),
                file_error.trim_end_matches(' ').to_owned(),
                "ACCTSID",
            ),
            (
                DomainError::unexpected(messages::UNABLE_TO_LOOKUP_TRANSACTION)
                    .with_field("ACTIDIN")
                    .with_field_message("CARDNIN", messages::UNEXPECTED_ERROR),
                messages::UNABLE_TO_LOOKUP_TRANSACTION
                    .trim_end_matches(' ')
                    .to_owned(),
                "ACTIDIN,CARDNIN",
            ),
            (
                DomainError::unexpected(messages::UNEXPECTED_ERROR),
                messages::UNEXPECTED_ERROR.to_owned(),
                "",
            ),
        ];
        for (error, reason, fields) in cases {
            let expected = ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorKind::Unexpected.code(),
                error.message.clone(),
                error
                    .fields
                    .iter()
                    .cloned()
                    .map(ErrorFieldBody::from)
                    .collect(),
            );
            let (converted, lines) = converted_in_handler_span(error);
            assert_eq!(converted.status(), expected.status(), "{reason}");
            assert_eq!(converted.body(), expected.body(), "{reason}");
            let events = events(&lines);
            let [event] = events.as_slice() else {
                panic!("{reason}: one event expected, got {lines:?}");
            };
            assert_eq!(event["level"], "ERROR", "{reason}");
            assert_eq!(event["target"], "carddemo_api::error", "{reason}");
            assert_eq!(
                event["fields"],
                json!({
                    "message": "unexpected failure answered",
                    "code": "UNEXPECTED",
                    "reason": reason,
                    "fields": fields,
                }),
                "{reason}"
            );
            assert_eq!(
                event["span"],
                json!({
                    "name": "update_account",
                    "transaction": "CAUP",
                    "user_id": "ADMIN001",
                    "account_id": "00000000001",
                }),
                "{reason}"
            );
            assert_eq!(span_status(&lines)["code"], "error", "{reason}");
        }
    }

    #[test]
    fn a_halt_is_answered_unchanged_and_logged_once_with_its_escalation_in_its_handler_span() {
        // The CAUP answer to a change held at the E-26 halt, under the default
        // filter, which exports the handler span, and under `warn`, which does
        // not: the event still carries the span's fields.
        let expected = ApiError::unexpected();
        for (directives, exported) in [("info", 1), ("warn", 0)] {
            let (answered, lines) = answered_in_handler_span(directives, || {
                ApiError::halted(DomainError::unexpected(messages::UNEXPECTED_ERROR), "E-26")
            });
            assert_eq!(answered.status(), expected.status(), "{directives}");
            assert_eq!(answered.body(), expected.body(), "{directives}");
            let events = events(&lines);
            let [event] = events.as_slice() else {
                panic!("{directives}: one event expected, got {lines:?}");
            };
            assert_eq!(event["level"], "ERROR", "{directives}");
            assert_eq!(event["target"], "carddemo_api::error", "{directives}");
            assert_eq!(
                event["fields"],
                json!({
                    "message": "unexpected failure answered",
                    "code": "UNEXPECTED",
                    "reason": messages::UNEXPECTED_ERROR,
                    "fields": "",
                    "cause": "halt",
                    "escalation_id": "E-26",
                }),
                "{directives}"
            );
            assert_eq!(
                event["span"],
                json!({
                    "name": "update_account",
                    "transaction": "CAUP",
                    "user_id": "ADMIN001",
                    "account_id": "00000000001",
                }),
                "{directives}"
            );
            let spans: Vec<&Value> = lines.iter().filter(|line| line["type"] == "span").collect();
            assert_eq!(spans.len(), exported, "{directives}: {lines:?}");
            if let [span] = spans.as_slice() {
                assert_eq!(span["status"]["code"], "error", "{directives}");
            }
        }
    }

    #[test]
    fn a_domain_error_of_any_other_kind_is_answered_without_a_log_event() {
        for (kind, status) in KIND_STATUSES {
            if kind == ErrorKind::Unexpected {
                continue;
            }
            let error =
                DomainError::new(kind, messages::CCDA_MSG_INVALID_KEY).with_field("ACCTSID");
            let (converted, lines) = converted_in_handler_span(error);
            assert_eq!(converted.status(), status, "{kind:?}");
            assert!(events(&lines).is_empty(), "{kind:?}: {lines:?}");
            assert_eq!(span_status(&lines)["code"], "unset", "{kind:?}");
        }
    }

    #[tokio::test]
    async fn into_response_writes_the_status_and_the_json_body() {
        let error = ApiError::from(
            DomainError::conflict(messages::RECORD_CHANGED_BY_SOME_ONE_ELSE).with_field("ACCTSID"),
        );
        let (status, content_type, bytes) = read_body(error.into_response()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(content_type.as_deref(), Some("application/json"));
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes).expect("a JSON body"),
            json!({
                "code": "CONFLICT",
                "message": messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
                "fields": [{
                    "field": "ACCTSID",
                    "message": messages::RECORD_CHANGED_BY_SOME_ONE_ELSE,
                }],
            })
        );
    }

    #[tokio::test]
    async fn only_a_401_carries_the_bearer_challenge_beside_an_unchanged_body() {
        let kinds = KIND_STATUSES.map(|(kind, status)| {
            let error = ApiError::from(
                DomainError::new(kind, messages::WRONG_PASSWORD_TRY_AGAIN).with_field("PASSWD"),
            );
            (format!("{kind:?}"), error, status)
        });
        // The request-written failures and the sign-on rejection's own
        // constructor, named by their codes.
        let named = [
            (ApiError::unauthenticated(), StatusCode::UNAUTHORIZED),
            (
                ApiError::from(
                    DomainError::signon_failed(messages::WRONG_PASSWORD_TRY_AGAIN)
                        .with_field("PASSWD"),
                ),
                StatusCode::UNAUTHORIZED,
            ),
            (ApiError::forbidden(), StatusCode::FORBIDDEN),
            (ApiError::route_not_found(), StatusCode::NOT_FOUND),
            (
                ApiError::method_not_allowed(),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (ApiError::unexpected(), StatusCode::INTERNAL_SERVER_ERROR),
            (
                ApiError::rejection(DETAIL.to_owned()),
                StatusCode::BAD_REQUEST,
            ),
        ]
        .map(|(error, status)| (error.body().code.clone(), error, status));
        for (case, error, status) in kinds.into_iter().chain(named) {
            let body = serde_json::to_vec(error.body()).expect("the body serializes");
            let response = error.into_response();
            let challenges: Vec<&[u8]> = response
                .headers()
                .get_all(header::WWW_AUTHENTICATE)
                .iter()
                .map(HeaderValue::as_bytes)
                .collect();
            let expected: &[&[u8]] = if status == StatusCode::UNAUTHORIZED {
                &[b"Bearer"]
            } else {
                &[]
            };
            assert_eq!(challenges, expected, "{case}");
            let (actual, content_type, bytes) = read_body(response).await;
            assert_eq!(actual, status, "{case}");
            assert_eq!(content_type.as_deref(), Some("application/json"), "{case}");
            assert_eq!(bytes, body, "{case}: the body, byte for byte");
        }
    }

    #[tokio::test]
    async fn created_writes_201_and_the_resource_as_the_json_body() {
        let resource = json!({
            "rule_id": "9007199254740993",
            "reward_rate": "0.0150",
            "active_to": null,
        });
        let (status, content_type, bytes) =
            read_body(Created(resource.clone()).into_response()).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(content_type.as_deref(), Some("application/json"));
        assert_eq!(
            bytes,
            serde_json::to_vec(&resource).expect("the resource serializes")
        );
    }

    #[derive(Debug, Deserialize)]
    struct Probe {
        name: String,
    }

    async fn wrapped_json(ApiJson(probe): ApiJson<Probe>) -> String {
        probe.name
    }

    async fn plain_json(probe: Result<Json<Probe>, JsonRejection>) -> String {
        probe.map_or_else(|rejection| rejection.body_text(), |Json(probe)| probe.name)
    }

    async fn wrapped_query(ApiQuery(probe): ApiQuery<Probe>) -> String {
        probe.name
    }

    async fn plain_query(probe: Result<Query<Probe>, QueryRejection>) -> String {
        probe.map_or_else(|rejection| rejection.body_text(), |Query(probe)| probe.name)
    }

    async fn wrapped_path(ApiPath(id): ApiPath<u32>) -> String {
        id.to_string()
    }

    async fn plain_path(id: Result<Path<u32>, PathRejection>) -> String {
        id.map_or_else(|rejection| rejection.body_text(), |Path(id)| id.to_string())
    }

    /// Each wrapper at `/wrapped/…`, and the axum extractor it wraps at
    /// `/plain/…`, where the handler answers with the rejection's own text.
    fn router() -> Router {
        Router::new()
            .route("/wrapped/json", post(wrapped_json))
            .route("/plain/json", post(plain_json))
            .route("/wrapped/query", get(wrapped_query))
            .route("/plain/query", get(plain_query))
            .route("/wrapped/path/{id}", get(wrapped_path))
            .route("/plain/path/{id}", get(plain_path))
    }

    fn request(method: &str, uri: &str, content_type: Option<&str>, body: &str) -> Request<Body> {
        let builder = Request::builder().method(method).uri(uri);
        let builder = match content_type {
            Some(value) => builder.header(header::CONTENT_TYPE, value),
            None => builder,
        };
        builder
            .body(Body::from(body.to_owned()))
            .expect("a valid request")
    }

    async fn send(request: Request<Body>) -> (StatusCode, Option<String>, Vec<u8>) {
        let response = router()
            .oneshot(request)
            .await
            .expect("the router is infallible");
        read_body(response).await
    }

    #[tokio::test]
    async fn extractors_pass_valid_input_and_answer_malformed_input_with_the_error_body() {
        const JSON: Option<&str> = Some("application/json");
        // (method, path after /wrapped or /plain, content type, body, extracted value)
        let valid = [
            ("POST", "/json", JSON, r#"{"name":"ADMIN001"}"#, "ADMIN001"),
            ("GET", "/query?name=USER0001", None, "", "USER0001"),
            ("GET", "/path/42", None, "", "42"),
        ];
        for (method, path, content_type, body, value) in valid {
            let wrapped = format!("/wrapped{path}");
            let (status, _, bytes) = send(request(method, &wrapped, content_type, body)).await;
            assert_eq!(status, StatusCode::OK, "{wrapped}");
            assert_eq!(bytes, value.as_bytes(), "{wrapped}");
        }

        // One byte beyond axum's default 2 MiB body limit: a body that cannot
        // be read.
        let oversized = " ".repeat(2 * 1024 * 1024 + 1);
        // (method, path after /wrapped or /plain, content type, body)
        let malformed = [
            ("POST", "/json", JSON, r#"{"name":"#),
            ("POST", "/json", JSON, r#"{"name":5}"#),
            ("POST", "/json", None, r#"{"name":"ADMIN001"}"#),
            ("POST", "/json", JSON, oversized.as_str()),
            ("GET", "/query?other=1", None, ""),
            ("GET", "/path/abc", None, ""),
            ("GET", "/path/99999999999", None, ""),
        ];
        for (method, path, content_type, body) in malformed {
            let plain = format!("/plain{path}");
            let (_, _, text) = send(request(method, &plain, content_type, body)).await;
            let text = String::from_utf8(text).expect("UTF-8 rejection text");
            assert!(!text.is_empty(), "{plain}");

            let wrapped = format!("/wrapped{path}");
            let (status, response_type, bytes) =
                send(request(method, &wrapped, content_type, body)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{wrapped}");
            assert_eq!(response_type.as_deref(), JSON, "{wrapped}");
            assert_eq!(
                serde_json::from_slice::<ErrorBody>(&bytes).expect("an error body"),
                ErrorBody {
                    code: ErrorKind::Invalid.code().to_owned(),
                    message: text,
                    fields: Vec::new(),
                },
                "{wrapped}"
            );
        }
    }

    /// The text axum puts before the description of a JSON body that does not
    /// fit its type.
    const DATA_ERROR: &str = "Failed to deserialize the JSON body into the target type: ";

    /// The description of a JSON array given for a request DTO: the position
    /// is the one before the `[`, which is never read.
    const SEQUENCE: &str = "invalid type: sequence, expected a JSON object at line 1 column 0";

    /// The smallest `RewardRuleRequest` object: its four fields without a
    /// default.
    const REWARD_RULE_OBJECT: &str = r#"{"transaction_type_cd":"01","transaction_category_cd":"0001","reward_rate":"1.0000","active_from":"2022-01-01T00:00:00Z"}"#;

    /// Answers 204 to a body that deserializes as `T`.
    async fn accept<T>(ApiJson(_): ApiJson<T>) -> StatusCode {
        StatusCode::NO_CONTENT
    }

    /// One request DTO a handler extracts with [`ApiJson`], behind a probe
    /// route.
    struct DtoRoute {
        /// The probe route's path.
        path: &'static str,
        /// `POST`, extracting the DTO through [`ApiJson`].
        handler: axum::routing::MethodRouter,
        /// The DTO's Rust type name, which no rejection may show.
        type_name: &'static str,
        /// One of the DTO's string fields.
        field: &'static str,
    }

    impl DtoRoute {
        fn new<T>(path: &'static str, field: &'static str) -> Self
        where
            T: serde::de::DeserializeOwned + Send + 'static,
        {
            let type_name = std::any::type_name::<T>();
            Self {
                path,
                handler: post(accept::<T>),
                type_name: type_name.rsplit("::").next().unwrap_or(type_name),
                field,
            }
        }
    }

    /// Every request DTO the handlers extract with [`ApiJson`].
    fn dto_routes() -> [DtoRoute; 10] {
        use crate::dto::{
            accounts, admin_reward_rules, admin_transaction_types, admin_users, auth, cards,
            payments, transactions,
        };
        [
            DtoRoute::new::<auth::LoginRequest>("/auth/login", "user_id"),
            DtoRoute::new::<accounts::AccountUpdateRequest>("/accounts", "acct_active_status"),
            DtoRoute::new::<cards::CardUpdateRequest>("/cards", "embossed_name"),
            DtoRoute::new::<transactions::TransactionAddRequest>("/transactions", "tran_amt"),
            DtoRoute::new::<payments::PaymentRequest>("/payments", "account_id"),
            DtoRoute::new::<admin_users::UserAddRequest>("/admin/users", "user_id"),
            DtoRoute::new::<admin_users::UserUpdateRequest>("/admin/users/update", "usr_type"),
            DtoRoute::new::<admin_transaction_types::TranTypeCreateRequest>(
                "/admin/transaction-types",
                "type_code",
            ),
            DtoRoute::new::<admin_transaction_types::TranTypeUpdateRequest>(
                "/admin/transaction-types/update",
                "description",
            ),
            DtoRoute::new::<admin_reward_rules::RewardRuleRequest>(
                "/admin/reward-rules",
                "reward_rate",
            ),
        ]
    }

    #[tokio::test]
    async fn bodies_that_do_not_fit_a_request_dto_answer_without_its_type_name() {
        const JSON: Option<&str> = Some("application/json");
        let dtos = dto_routes();
        let type_names: Vec<&str> = dtos.iter().map(|dto| dto.type_name).collect();
        let router = dtos.iter().fold(Router::new(), |router, dto| {
            router.route(dto.path, dto.handler.clone())
        });
        for dto in &dtos {
            let (path, field) = (dto.path, dto.field);
            // The smallest object the DTO accepts: every field has a default
            // but the reward rule's type, category, rate and start.
            let object = if dto.type_name == "RewardRuleRequest" {
                REWARD_RULE_OBJECT
            } else {
                "{}"
            };
            let response = router
                .clone()
                .oneshot(request("POST", path, JSON, object))
                .await
                .expect("the router is infallible");
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{path} {object}");

            // `{"<field>":` ends at column field.len() + 4: a map value is
            // reported there, a number once it is read.
            let (map_column, number_column) = (field.len() + 4, field.len() + 5);
            // (body, description after the preamble)
            let mut cases = vec![
                (
                    "1".to_owned(),
                    "invalid type: integer `1`, expected a JSON object at line 1 column 1"
                        .to_owned(),
                ),
                (
                    r#""x""#.to_owned(),
                    r#"invalid type: string "x", expected a JSON object at line 1 column 3"#
                        .to_owned(),
                ),
                (
                    "true".to_owned(),
                    "invalid type: boolean `true`, expected a JSON object at line 1 column 4"
                        .to_owned(),
                ),
                (
                    "null".to_owned(),
                    "invalid type: null, expected a JSON object at line 1 column 4".to_owned(),
                ),
                // An array is refused before its first element is read,
                // whatever it holds, so no field is bound by position.
                ("[]".to_owned(), SEQUENCE.to_owned()),
                ("[1]".to_owned(), SEQUENCE.to_owned()),
                (r#"["ADMIN001","PASSWORD"]"#.to_owned(), SEQUENCE.to_owned()),
                (
                    r#"["01","0001","2.5",null,null,"2099-01-01T00:00:00Z",null]"#.to_owned(),
                    SEQUENCE.to_owned(),
                ),
                (
                    format!(r#"{{"{field}":{{}}}}"#),
                    format!(
                        "{field}: invalid type: map, expected a string at line 1 column {map_column}"
                    ),
                ),
                (
                    format!(r#"{{"{field}":5}}"#),
                    format!(
                        "{field}: invalid type: integer `5`, expected a string \
                         at line 1 column {number_column}"
                    ),
                ),
            ];
            // Only the reward rule has fields without a default, so only it
            // refuses an empty object.
            if dto.type_name == "RewardRuleRequest" {
                cases.push((
                    "{}".to_owned(),
                    "missing field `transaction_type_cd` at line 1 column 2".to_owned(),
                ));
            }
            for (body, description) in cases {
                let response = router
                    .clone()
                    .oneshot(request("POST", path, JSON, &body))
                    .await
                    .expect("the router is infallible");
                let (status, content_type, bytes) = read_body(response).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{path} {body}");
                assert_eq!(content_type.as_deref(), JSON, "{path} {body}");
                let error = serde_json::from_slice::<ErrorBody>(&bytes).expect("an error body");
                for type_name in &type_names {
                    assert!(
                        !error.message.contains(type_name),
                        "{path} {body}: {}",
                        error.message
                    );
                }
                assert!(
                    !error.message.contains("JSON array"),
                    "{path} {body}: {}",
                    error.message
                );
                assert_eq!(
                    error,
                    ErrorBody {
                        code: ErrorKind::Invalid.code().to_owned(),
                        message: format!("{DATA_ERROR}{description}"),
                        fields: Vec::new(),
                    },
                    "{path} {body}"
                );
            }
        }
    }

    /// A unit struct, which serde words `unit struct Marker`.
    #[derive(Deserialize)]
    struct Marker;

    /// A tuple struct, which serde words `tuple struct Pair`.
    #[derive(Deserialize)]
    struct Pair((), ());

    /// An externally tagged enum with a tuple and a struct variant.
    #[derive(Deserialize)]
    enum Shape {
        Line((), ()),
        Square { _side: () },
    }

    /// An internally tagged enum.
    #[derive(Deserialize)]
    #[serde(tag = "kind")]
    enum Tagged {
        Dot,
    }

    /// An adjacently tagged enum.
    #[derive(Deserialize)]
    #[serde(tag = "kind", content = "data")]
    enum Adjacent {
        Dot,
    }

    /// An untagged enum.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Untagged {
        Dot,
    }

    /// A struct with a flattened enum.
    #[derive(Deserialize)]
    struct Flattened {
        #[serde(flatten)]
        _shape: Shape,
    }

    /// The text axum gives `body` as a `T`, and the error [`ApiJson`]
    /// answers it with.
    fn reject<T: serde::de::DeserializeOwned>(body: &str) -> (String, ApiError) {
        let Err(rejection) = Json::<T>::from_bytes(body.as_bytes()) else {
            panic!("{body} deserializes");
        };
        (rejection.body_text(), ApiError::from(rejection))
    }

    /// [`reject`] for one of the types above.
    type Reject = fn(&str) -> (String, ApiError);

    /// The Rust names of the types above, which no rejection may show.
    const TYPE_NAMES: [&str; 7] = [
        "Marker",
        "Pair",
        "Shape",
        "Tagged",
        "Adjacent",
        "Untagged",
        "Flattened",
    ];

    #[test]
    fn data_errors_word_each_derived_type_as_the_json_it_accepts() {
        // (type, body, serde's wording, the wording answered)
        let cases: [(Reject, &str, &str, &str); 12] = [
            (
                reject::<Marker>,
                "1",
                "expected unit struct Marker",
                "expected null",
            ),
            (
                reject::<Pair>,
                "1",
                "expected tuple struct Pair",
                "expected a JSON array",
            ),
            (
                reject::<Pair>,
                "[null]",
                "expected tuple struct Pair with 2 elements",
                "expected a JSON array of 2 elements",
            ),
            (
                reject::<Shape>,
                r#"{"Line":1}"#,
                "Line: invalid type: integer `1`, expected tuple variant Shape::Line",
                "Line: invalid type: integer `1`, expected a JSON array",
            ),
            (
                reject::<Shape>,
                r#"{"Line":[]}"#,
                "expected tuple variant Shape::Line with 2 elements",
                "expected a JSON array of 2 elements",
            ),
            (
                reject::<Shape>,
                r#"{"Square":1}"#,
                "expected struct variant Shape::Square",
                "expected a JSON object",
            ),
            (
                reject::<Shape>,
                r#"{"Square":[]}"#,
                "expected struct variant Shape::Square with 1 element",
                "expected a JSON array of 1 element",
            ),
            // The syntax error that stands for `enum Shape`, kept whole.
            (
                reject::<Shape>,
                "1",
                "expected value at line 1 column 1",
                "expected value at line 1 column 1",
            ),
            (
                reject::<Tagged>,
                "1",
                "expected internally tagged enum Tagged",
                "expected a JSON object with a variant tag",
            ),
            (
                reject::<Adjacent>,
                "1",
                "expected adjacently tagged enum Adjacent",
                "expected a JSON object with a variant tag and content",
            ),
            // Written once the body is buffered, so without a position.
            (
                reject::<Untagged>,
                "1",
                "data did not match any variant of untagged enum Untagged",
                "data did not match any variant",
            ),
            (
                reject::<Flattened>,
                "{}",
                "no variant of enum Shape found in flattened data",
                "no variant found in flattened data",
            ),
        ];
        for (reject, body, serde_wording, answered) in cases {
            let (text, error) = reject(body);
            assert!(text.contains(serde_wording), "{body}: {text}");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{body}");
            assert_eq!(
                error.body(),
                &ErrorBody {
                    code: ErrorKind::Invalid.code().to_owned(),
                    message: text.replacen(serde_wording, answered, 1),
                    fields: Vec::new(),
                },
                "{body}"
            );
            for type_name in TYPE_NAMES {
                assert!(
                    !error.body().message.contains(type_name),
                    "{body}: {}",
                    error.body().message
                );
            }
        }
    }
}
