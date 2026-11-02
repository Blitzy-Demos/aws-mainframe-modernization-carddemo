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
//! store failure goes to the log through `tracing`, never to the body.
//!
//! # Extractors
//!
//! axum's own `Json`, `Query` and `Path` answer a rejected request with a
//! plain-text body. [`ApiJson`], [`ApiQuery`] and [`ApiPath`] wrap them and
//! answer with the error body instead: status 400, code `INVALID`, and the
//! rejection's text as `message`. That text describes the malformed input and
//! holds no server detail. A JSON body that parses but does not fit the
//! request DTO is the one rejection whose text serde words after a Rust type
//! (`expected struct LoginRequest`): its `message` keeps the JSON path, the
//! value received and the line and column, and names the JSON that type
//! accepts in place of the type (`expected a JSON object`, `expected a JSON
//! array of 7 elements`). Handlers declare their request body and parameters
//! explicitly in `#[utoipa::path(request_body = .., params(..))]`, so the
//! wrappers leave the `OpenAPI` document unchanged.
//!
//! # Success statuses
//!
//! [`Created`] answers a stored new resource with 201 and the resource as its
//! JSON body, so a handler that returns it names the kind of success and
//! chooses no status itself. Handlers still declare the 201 in
//! `#[utoipa::path(responses(..))]`.

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};
use carddemo_domain::error::{DomainError, ErrorKind, FieldError};
use carddemo_domain::messages;
use carddemo_domain::ports::PortError;
use carddemo_domain::rewards::RewardError;
use carddemo_persistence::PersistenceError;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// The `code` of a request without a valid bearer token.
const AUTH_REQUIRED_CODE: &str = "UNAUTHENTICATED";

/// The `code` of a caller whose token is not an admin's, on `/admin/**`.
const FORBIDDEN_CODE: &str = "FORBIDDEN";

/// The `code` of a method a routed path does not serve.
const METHOD_NOT_ALLOWED_CODE: &str = "METHOD_NOT_ALLOWED";

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

impl From<DomainError> for ApiError {
    fn from(err: DomainError) -> Self {
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
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
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
/// 400 error body.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiJson<T>(pub T);

impl<T, S> FromRequest<S> for ApiJson<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(req, state).await?;
        Ok(Self(value))
    }
}

/// A query string, extracted as `axum::extract::Query` does, whose rejection
/// is the 400 error body.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiQuery<T>
where
    T: DeserializeOwned,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Query(value) = Query::<T>::from_request_parts(parts, state).await?;
        Ok(Self(value))
    }
}

/// Path parameters, extracted as `axum::extract::Path` does, whose rejection
/// is the 400 error body.
#[derive(Debug, Clone, Copy, Default)]
pub struct ApiPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ApiPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let Path(value) = Path::<T>::from_request_parts(parts, state).await?;
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
    use axum::extract::{Json, Path, Query};
    use axum::http::{Request, StatusCode, header};
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
                (
                    "[1]".to_owned(),
                    "[0]: invalid type: integer `1`, expected a string at line 1 column 2"
                        .to_owned(),
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
            // refuses an empty array or object.
            if dto.type_name == "RewardRuleRequest" {
                cases.push((
                    "[]".to_owned(),
                    "invalid length 0, expected a JSON array of 7 elements at line 1 column 2"
                        .to_owned(),
                ));
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
