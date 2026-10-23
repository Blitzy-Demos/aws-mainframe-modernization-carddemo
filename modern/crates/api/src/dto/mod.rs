//! The request and response contracts of the 27 REST operations (AAP 0.3.4.1), and what all of
//! them share: the [`ScreenResult`] and [`PageDirection`] schemas, the `page_size` rule of
//! Bounded Pagination, the validating extractors [`ValidJson`], [`ValidQuery`] and
//! [`ValidPath`], and the raw path extractor [`RawPath`].
//!
//! # Folder contract
//!
//! - **One module per online program**, named after it: 21 modules for the 22 programs behind
//!   the endpoints. COPAUS2C has none, because the fraud toggle it performs for COPAUS1C
//!   (endpoint 23) answers through `copaus1c`; the user read COUSR02C and COUSR03C share
//!   (endpoint 18) lives in `cousr02c`.
//! - **Schema names are fixed.** Every request and response type derives `utoipa::ToSchema`
//!   under the name AAP 0.3.4.1 and 0.6.2 give it. Those names are the `components.schemas`
//!   keys of `modern/openapi/openapi.json` and the only payload types of the frontend's
//!   generated `schema.d.ts` (Generated API Types Only), so renaming one changes the contract.
//! - **Conversions only.** A module holds its DTOs, their `validator` rules (the map `LENGTH`
//!   and `NUM` attributes, the server half of the `both` rows of `modern/audit/screens.md`, and
//!   the [`pic_x`] storage of a text field) and `From` conversions from domain outcomes. It
//!   holds no business rule, reads no store and names neither `persistence` nor `sqlx`: every
//!   program edit and message stays in `domain::online` (Thin Request Handlers).
//! - **A DTO field keeps its Rust name on the wire.** `validator` reports a failing field under
//!   its Rust name, so no DTO field is renamed through `serde`; the `field` of a rejection is
//!   then the name the client sent.
//!
//! # Extractors
//!
//! A handler takes a body as [`ValidJson<T>`], a query string as [`ValidQuery<T>`], path
//! parameters that carry a map field as [`ValidPath<T>`] and path parameters whose raw text the
//! program's own edit checks as [`RawPath<T>`], which checks no rule. utoipa cannot see through
//! any of them, so a handler taking `ValidJson<T>` declares `request_body = T`, one taking
//! `ValidQuery<T>` or `ValidPath<T>` declares `params(T)`, and one taking `RawPath<T>` declares
//! each segment inline in the `params` of its `#[utoipa::path]`.
//!
//! Every rejection is a [`ScreenError`]:
//!
//! - A body, query string or path that does not deserialise into `T`: 400 with
//!   [`INVALID_BODY`], [`INVALID_QUERY`] or [`INVALID_PATH`] and no field. That covers malformed
//!   JSON, a missing JSON content type, a path segment whose percent-encoding is not UTF-8, a
//!   value of the wrong type (text where an integer belongs, for instance) and an unknown enum
//!   value. An attention key whose program answers unserved keys itself (`cotrtupc`,
//!   `cotrn02c`) reads every other key name as a hidden variant instead, so that the program's
//!   invalid-key message, not this rejection, answers it.
//! - A rule a DTO declares: 400 naming the first failing field in key order, as a path such as
//!   `user_id`, `fields.zip` or `rows[3].description`, with the text of the rule's code
//!   ([`FIELD_LENGTH_MESSAGE`] for `length` and [`PIC_X_CODE`], [`FIELD_NUMERIC_MESSAGE`] or
//!   [`FIELD_RANGE_MESSAGE`]).
//! - `page_size` outside [`MIN_PAGE_SIZE`] to [`MAX_PAGE_SIZE`]: 422 with [`PAGE_SIZE_MESSAGE`]
//!   and field `page_size`, whatever else failed. It is the only 422 of the API.
//!
//! # Redaction
//!
//! A rejection never carries the input. It holds neither the deserialiser's error text, which
//! quotes the offending value, nor the URI, nor `validator`'s `params` and `message`, where
//! `validator` keeps the rejected value; any of them could hold a card number, an SSN or a
//! password (Cardholder Data Redaction, AAP 0.3.4.8). Nothing in this module logs.
//!
//! # `page_size`
//!
//! Every collection DTO (`CardListQuery`, `TranListQuery`, `UserListQuery`, `AuthSummaryQuery`,
//! `TranTypeListQuery` and `TranTypeListActionRequest`) declares `page_size: Option<i64>` with
//! `#[validate(range(min = 1, max = 100))]` and `#[schema(minimum = 1, maximum = 100)]` or
//! `#[param(minimum = 1, maximum = 100)]`. The field is `i64` rather than an unsigned type so
//! that `0`, a negative value and `101` all deserialise and reach the range rule, which answers
//! 422; a value that is not an integer at all fails deserialisation and is a 400. The screens
//! never send it, and [`resolve_page_size`] gives the program's screen size when it is absent.

// In endpoint order. Each declaration stands alone because rustfmt sorts a run of adjacent `mod`
// items, which would undo the order; the trailing comment names the module's endpoints.
pub mod cosgn00c; // endpoint 1

pub mod comen01c; // endpoint 2

pub mod coadm01c; // endpoint 3

pub mod coactvwc; // endpoint 4

pub mod coactupc; // endpoints 5 and 6

pub mod cocrdlic; // endpoint 7

pub mod cocrdslc; // endpoint 8

pub mod cocrdupc; // endpoints 9 and 10

pub mod cotrn00c; // endpoint 11

pub mod cotrn01c; // endpoint 12

pub mod cotrn02c; // endpoint 13

pub mod corpt00c; // endpoint 14

pub mod cobil00c; // endpoint 15

pub mod cousr00c; // endpoint 16

pub mod cousr01c; // endpoint 17

pub mod cousr02c; // endpoints 18 and 19

pub mod cousr03c; // endpoint 20

pub mod copaus0c; // endpoint 21

pub mod copaus1c; // endpoints 22 and 23

pub mod cotrtlic; // endpoints 24 and 25

pub mod cotrtupc; // endpoints 26 and 27

use axum::Json;
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::request::Parts;
use domain::copybook::{PicU, PicX};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError, ValidationErrors, ValidationErrorsKind};

use crate::error::ScreenError;

/// The body of a 200 that carries only the message row: user add (endpoint 17, COUSR01C), user
/// update (endpoint 19, COUSR02C) and user delete (endpoint 20, COUSR03C).
// The schema name `ScreenResult` is the frontend's `components["schemas"]["ScreenResult"]`
// (Generated API Types Only); `api::openapi::ApiDoc` registers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({"message": "User NEWUSR01 has been added ..."}))]
pub struct ScreenResult {
    /// The message row exactly as the program words it, such as `User NEWUSR01 has been added
    /// ...` (COUSR01C-021), `User USER0001 has been updated ...` (COUSR02C-029) or
    /// `User USER0001 has been deleted ...` (COUSR03C-019).
    pub message: String,
}

/// The program's success message, as `cousr01c::added_message`, `cousr02c::updated_message`
/// and `cousr03c::delete_outcome` render it.
impl From<String> for ScreenResult {
    fn from(message: String) -> Self {
        Self { message }
    }
}

/// The paging key of a collection request: `forward` (F8) or `backward` (F7). Absent, the
/// request is ENTER or a first entry.
// The schema name `PageDirection` is fixed (Generated API Types Only); each program's own
// `Direction` comes from it through the conversions below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PageDirection {
    /// `forward`: F8, the page after the one shown.
    Forward,
    /// `backward`: F7, the page before the one shown.
    Backward,
}

/// One `From<PageDirection>` per program direction type: each program declares its own
/// `Direction` with the same two variants, and none re-exports another's. The path is spelled
/// in full because this module's own program modules carry the same names.
macro_rules! direction_from_page_direction {
    ($($program:ident),+ $(,)?) => {$(
        #[doc = concat!("F8 and F7 as `domain::online::", stringify!($program), "::Direction`.")]
        impl From<PageDirection> for domain::online::$program::Direction {
            fn from(direction: PageDirection) -> Self {
                match direction {
                    PageDirection::Forward => Self::Forward,
                    PageDirection::Backward => Self::Backward,
                }
            }
        }
    )+};
}

direction_from_page_direction!(cocrdlic, cotrn00c, cousr00c, copaus0c, cotrtlic);

/// The smallest `page_size` a collection endpoint admits (Bounded Pagination).
pub const MIN_PAGE_SIZE: i64 = 1;

/// The largest `page_size` a collection endpoint admits (Bounded Pagination).
pub const MAX_PAGE_SIZE: i64 = 100;

/// The wire name of the page-size member of every collection DTO, and the field of its 422.
pub const PAGE_SIZE_FIELD: &str = "page_size";

/// The rows a collection request reads: `requested`, or `default` when it is absent.
///
/// Callers pass their program's `DEFAULT_PAGE_SIZE`, the screen size: 7 cards (`cocrdlic`),
/// 10 transactions (`cotrn00c`), 10 users (`cousr00c`), 5 authorizations (`copaus0c`) and 7
/// transaction types (`cotrtlic`). Those constants are `usize`, so a service converts them
/// with the checked `u32::try_from`, never by a cast. Validation has already rejected a value
/// outside [`MIN_PAGE_SIZE`] to [`MAX_PAGE_SIZE`] with 422, so `requested` fits; a value that
/// does not still gives `default` and never wraps.
#[must_use]
pub fn resolve_page_size(requested: Option<i64>, default: u32) -> u32 {
    // TRUNCATION: checked, never lossy. The range rule admits only 1 to 100, which `u32` holds
    // exactly, and a value the conversion refuses falls back to the default.
    requested
        .and_then(|size| u32::try_from(size).ok())
        .unwrap_or(default)
}

/// The message of a body that does not deserialise into its DTO.
///
/// A fixed text of this crate: it never includes the deserialiser's error, which quotes the
/// offending value, or any other part of the request.
pub const INVALID_BODY: &str = "Invalid request body...";

/// The message of a query string that does not deserialise into its DTO.
///
/// A fixed text of this crate: it never includes the deserialiser's error, the URI or any
/// value of the query.
pub const INVALID_QUERY: &str = "Invalid query parameters...";

/// The message of path parameters that do not deserialise into their DTO, such as a segment
/// whose percent-encoding is not UTF-8.
///
/// A fixed text of this crate: it never includes the deserialiser's error, the URI or any
/// segment of the path.
pub const INVALID_PATH: &str = "Invalid path parameters...";

/// The message of a field longer than its map `LENGTH`, of a text that does not fit its
/// `PIC X(n)` field byte for byte as printable ASCII ([`pic_x`], code [`PIC_X_CODE`]), and of
/// every rule code without a text of its own: input the 3270 field would not have admitted.
///
/// The server half of the `LENGTH` `both` rows of `modern/audit/screens.md`, such as
/// `COSGN00-027` (`USERID`, 8), `COSGN00-031` (`PASSWD`, 8), `COACTUP-013` (`ACCTSID`, 11) and
/// `CORPT00-012` (`MONTHLY`, 1). `screens.md` records no message for those rows, so this is a
/// fixed text of this crate and not a COBOL literal.
pub const FIELD_LENGTH_MESSAGE: &str = "Input does not fit the screen field...";

/// The message of a field with the map `NUM` attribute that holds a character other than a
/// digit.
///
/// The server half of the `NUM` `both` rows of `modern/audit/screens.md`: `CORPT00-022`,
/// `CORPT00-024`, `CORPT00-026`, `CORPT00-030`, `CORPT00-032` and `CORPT00-034`, the six date
/// parts of the report screen (X-CORPT00C-5). A fixed text of this crate, as `screens.md`
/// records no message for them.
pub const FIELD_NUMERIC_MESSAGE: &str = "Only digits can be entered in this field...";

/// The message of a numeric request member outside its declared range, such as a page number
/// below 1. A fixed text of this crate.
pub const FIELD_RANGE_MESSAGE: &str = "Value is out of the allowed range...";

/// The message of the 422 a `page_size` outside [`MIN_PAGE_SIZE`] to [`MAX_PAGE_SIZE`] gets
/// (Bounded Pagination). A fixed text of this crate.
pub const PAGE_SIZE_MESSAGE: &str = "page_size must be from 1 to 100...";

/// The code of the [`numeric`] rule.
pub const NUMERIC_CODE: &str = "numeric";

/// The code of the [`pic_x`] rule.
pub const PIC_X_CODE: &str = "pic_x";

/// The `LOW-VALUES` byte, NUL.
const LOW_VALUE: u8 = 0;

/// `validator`'s code of a `range` rule.
const RANGE_CODE: &str = "range";

/// A JSON body deserialised into `T` and checked against `T`'s `validator` rules.
///
/// A handler that takes it declares `request_body = T` in its `#[utoipa::path]`. The rejection
/// is a [`ScreenError`]: 400 with [`INVALID_BODY`] when the body does not deserialise (the JSON
/// content type is required), otherwise the error of the first failing rule, or the 422 of an
/// out-of-range `page_size`.
#[derive(Debug, Clone)]
pub struct ValidJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ScreenError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // The rejection's own text quotes the offending input, so it is dropped unread.
        let Json(value) = Json::<T>::from_request(req, state)
            .await
            .map_err(|_| ScreenError::input(INVALID_BODY, None))?;
        value
            .validate()
            .map_err(|errors| validation_error(&errors))?;
        Ok(Self(value))
    }
}

/// A query string deserialised into `T` and checked against `T`'s `validator` rules.
///
/// A handler that takes it declares `params(T)` in its `#[utoipa::path]`, `T` deriving
/// `utoipa::IntoParams`. The rejection is a [`ScreenError`]: 400 with [`INVALID_QUERY`] when
/// the query does not deserialise, otherwise the error of the first failing rule, or the 422
/// of an out-of-range `page_size`.
#[derive(Debug, Clone)]
pub struct ValidQuery<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidQuery<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ScreenError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // The rejection's own text quotes the offending input, so it is dropped unread.
        let Query(value) = Query::<T>::from_request_parts(parts, state)
            .await
            .map_err(|_| ScreenError::input(INVALID_QUERY, None))?;
        value
            .validate()
            .map_err(|errors| validation_error(&errors))?;
        Ok(Self(value))
    }
}

/// Path parameters deserialised into `T` and checked against `T`'s `validator` rules, for a
/// path segment that carries the text of a map field, such as the transaction ID of endpoint 12
/// (`TRNIDIN`, 16) and the user ID of endpoints 18 to 20 (`USRIDIN`, 8).
///
/// A handler that takes it declares `params(T)` in its `#[utoipa::path]`, `T` deriving
/// `utoipa::IntoParams` with `parameter_in = Path`. The rejection is a [`ScreenError`]: 400 with
/// [`INVALID_PATH`] when the path does not deserialise, otherwise the error of the first failing
/// rule, such as [`FIELD_LENGTH_MESSAGE`] naming a segment longer than its map `LENGTH`.
#[derive(Debug, Clone)]
pub struct ValidPath<T>(pub T);

impl<T, S> FromRequestParts<S> for ValidPath<T>
where
    T: DeserializeOwned + Validate + Send,
    S: Send + Sync,
{
    type Rejection = ScreenError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // The rejection's own text quotes the offending segment, so it is dropped unread.
        let Path(value) = Path::<T>::from_request_parts(parts, state)
            .await
            .map_err(|_| ScreenError::input(INVALID_PATH, None))?;
        value
            .validate()
            .map_err(|errors| validation_error(&errors))?;
        Ok(Self(value))
    }
}

/// Path parameters deserialised into `T` and passed on unchecked, for a path segment that
/// carries a key the program's own edit checks: the account number of endpoints 4 to 6 and 21,
/// the card handle of endpoint 10, and the account and authorization key of endpoints 22 and
/// 23.
///
/// No rule is checked and nothing is parsed, trimmed or normalised here, so the text reaches the
/// service as the client sent it, once percent-decoded, and the program's total domain edit
/// answers a malformed key with its own message. utoipa cannot see through it, so a handler that
/// takes it declares each segment inline, as `("acct_id" = String, Path, ..)`, in the `params`
/// of its `#[utoipa::path]`. The rejection is a [`ScreenError`]: 400 with [`INVALID_PATH`] and
/// no field when the path does not deserialise into `T`, such as a segment whose percent-encoding
/// is not UTF-8.
#[derive(Debug, Clone)]
pub struct RawPath<T>(pub T);

impl<T, S> FromRequestParts<S> for RawPath<T>
where
    T: DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = ScreenError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // The rejection's own text quotes the offending segment, so it is dropped unread.
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| ScreenError::input(INVALID_PATH, None))
    }
}

/// The [`ScreenError`] of a failed `validate()`.
///
/// A top-level `page_size` error decides it: 422 with [`PAGE_SIZE_MESSAGE`], whatever else
/// failed. Otherwise 400 naming the first failing field in key order, with the text of its
/// first error's code. Only the field names and the codes are read, never an error's `params`
/// or `message`, where `validator` keeps the rejected value.
fn validation_error(errors: &ValidationErrors) -> ScreenError {
    if errors.errors().contains_key(PAGE_SIZE_FIELD) {
        return ScreenError::page_size(PAGE_SIZE_MESSAGE);
    }
    match first_failure(errors) {
        Some((field, code)) => ScreenError::input(code_message(code), Some(field)),
        None => ScreenError::input(FIELD_LENGTH_MESSAGE, None),
    }
}

/// The text of a rule code: [`FIELD_NUMERIC_MESSAGE`] for [`NUMERIC_CODE`],
/// [`FIELD_RANGE_MESSAGE`] for `range`, and [`FIELD_LENGTH_MESSAGE`] for `length`,
/// [`PIC_X_CODE`] and every other code.
fn code_message(code: &str) -> &'static str {
    match code {
        NUMERIC_CODE => FIELD_NUMERIC_MESSAGE,
        RANGE_CODE => FIELD_RANGE_MESSAGE,
        _ => FIELD_LENGTH_MESSAGE,
    }
}

/// The path and the first error code of the first failing field of `errors`.
///
/// Keys are visited in ascending order and list items in ascending index, so the answer never
/// depends on `HashMap` order. A nested struct adds `parent.child` to the path and a list item
/// `parent[index].child`; an entry with no error is passed over.
fn first_failure(errors: &ValidationErrors) -> Option<(String, &str)> {
    let mut entries: Vec<(&str, &ValidationErrorsKind)> = errors
        .errors()
        .iter()
        .map(|(&name, kind)| (name, kind))
        .collect();
    entries.sort_unstable_by_key(|&(name, _)| name);
    entries.into_iter().find_map(|(name, kind)| match kind {
        ValidationErrorsKind::Field(list) => {
            list.first().map(|error| (name.to_owned(), code(error)))
        }
        ValidationErrorsKind::Struct(inner) => {
            first_failure(inner).map(|(path, code)| (format!("{name}.{path}"), code))
        }
        ValidationErrorsKind::List(items) => items.iter().find_map(|(index, item)| {
            first_failure(item).map(|(path, code)| (format!("{name}[{index}].{path}"), code))
        }),
    })
}

/// The code of `error`.
fn code(error: &ValidationError) -> &str {
    &error.code
}

/// The `NUM` attribute of a map field, as a `validator` custom rule:
/// `#[validate(custom(function = "super::numeric"))]`.
///
/// Admits a value of ASCII digits only, the characters `modern/audit/screens.md` says the
/// numeric field admits (`CORPT00-022` to `CORPT00-034`, X-CORPT00C-5), and a blank value,
/// empty or all spaces, which the program's own blank-field edit answers with its message.
///
/// # Errors
///
/// A [`ValidationError`] with code [`NUMERIC_CODE`] for any other value, a sign, a decimal
/// point or a space beside a digit included. The error carries no message.
pub fn numeric(value: &str) -> Result<(), ValidationError> {
    let bytes = value.as_bytes();
    if bytes.iter().all(u8::is_ascii_digit) || bytes.iter().all(|&byte| byte == b' ') {
        Ok(())
    } else {
        Err(ValidationError::new(NUMERIC_CODE))
    }
}

/// The `PIC X(n)` field a map input is moved into, as a `validator` custom rule beside the
/// field's `length`: `#[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]`.
///
/// `length` counts characters, but the field holds `N` bytes of printable ASCII (`0x20`–`0x7E`,
/// PIC-001, PIC-002), and a program reads the text into it as an alphanumeric `MOVE`, which cuts
/// it on the right. This rule admits a value only when it fits the field byte for byte, as
/// `PicX::<N>::from_text` takes it, so that `MOVE` never drops a byte and a password is hashed
/// exactly as typed: at most `N` bytes, each printable ASCII. An empty or all-space value is
/// printable text and reaches the program's blank edit. So does `LOW-VALUES`, exactly `N` NUL
/// bytes, the field as the terminal sends it unmodified, which the `= SPACES OR LOW-VALUES`
/// edits answer with their own messages.
///
/// # Errors
///
/// A [`ValidationError`] with code [`PIC_X_CODE`] for any other value: more than `N` bytes, a
/// character outside ASCII however few characters the value counts, a control byte such as TAB
/// or `0x7F`, or NUL bytes that do not fill the field, which are not `LOW-VALUES` and which no
/// field stores. The error carries no message; its text is [`FIELD_LENGTH_MESSAGE`].
pub fn pic_x<const N: usize>(value: &str) -> Result<(), ValidationError> {
    let bytes = value.as_bytes();
    let low_values = bytes.len() == N && bytes.iter().all(|&byte| byte == LOW_VALUE);
    if low_values || PicX::<N>::from_text(value).is_ok() {
        Ok(())
    } else {
        Err(ValidationError::new(PIC_X_CODE))
    }
}

/// A `PIC X(n)` field as shown: its bytes without the trailing spaces, leading spaces kept,
/// converted lossily (the stored bytes are printable ASCII).
#[must_use]
pub(crate) fn text<const N: usize>(field: &PicX<N>) -> String {
    field.to_text()
}

/// A `PIC X(n)` field as its map field holds it: all `N` bytes, trailing spaces included,
/// converted lossily (the stored bytes are printable ASCII). For the response fields whose
/// audit row carries the stored or moved value unchanged.
#[must_use]
pub(crate) fn full_text<const N: usize>(field: &PicX<N>) -> String {
    String::from_utf8_lossy(field.as_bytes()).into_owned()
}

// `digits` below takes the `Copy` `PicU` by value, and the helpers above take `PicX`, which is
// not `Copy`, by reference: the workspace denies clippy's `trivially_copy_pass_by_ref`, which a
// reference to the eight-byte `PicU` trips, and lint suppressions are not allowed.

/// A `PIC 9(n)` field as shown: its `N` digits with the leading zeros, the text a COBOL `MOVE`
/// of the field to `X(n)` gives, from the field's own encoder.
#[must_use]
pub(crate) fn digits<const N: u32>(field: PicU<N>) -> String {
    let mut out = Vec::new();
    field.encode_into(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::{Body, to_bytes};
    use axum::extract::{FromRequest, FromRequestParts, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use axum::routing::get;
    use axum::{Json, Router};
    use domain::copybook::{PicU, PicX};
    use domain::online::{cocrdlic, copaus0c, cotrn00c, cotrtlic, cousr00c};
    use serde::Deserialize;
    use serde_json::{Value, json};
    use tower::ServiceExt as _;
    use utoipa::ToSchema;
    use validator::{Validate, ValidationError};

    use super::{
        FIELD_LENGTH_MESSAGE, FIELD_NUMERIC_MESSAGE, FIELD_RANGE_MESSAGE, INVALID_BODY,
        INVALID_PATH, INVALID_QUERY, MAX_PAGE_SIZE, MIN_PAGE_SIZE, PAGE_SIZE_FIELD,
        PAGE_SIZE_MESSAGE, PIC_X_CODE, PageDirection, RawPath, ScreenResult, ValidJson, ValidQuery,
        digits, full_text, numeric, pic_x, resolve_page_size, text, validation_error,
    };
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// One width of the [`pic_x`] rule, so that one table holds several widths.
    type PicXRule = fn(&str) -> Result<(), ValidationError>;

    /// A card number the rejection texts must never echo.
    const CARD: &str = "4111111111111111";

    #[derive(Debug, Default, Deserialize, Validate)]
    #[serde(default)]
    struct Row {
        #[validate(length(max = 50))]
        description: String,
    }

    #[derive(Debug, Default, Deserialize, Validate)]
    #[serde(default)]
    struct Inner {
        #[validate(length(max = 2))]
        code: String,
    }

    /// A collection request with every kind of rule the DTO modules declare.
    #[derive(Debug, Default, Deserialize, Validate)]
    #[serde(default)]
    struct Probe {
        #[validate(range(min = 1, max = 100))]
        page_size: Option<i64>,
        #[validate(range(min = 1))]
        page_no: Option<i64>,
        #[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]
        user_id: String,
        #[validate(length(max = 2), custom(function = "super::numeric"))]
        start_month: String,
        #[validate(nested)]
        inner: Inner,
        #[validate(nested)]
        rows: Vec<Row>,
    }

    /// A query-string request: page size and one text filter.
    #[derive(Debug, Deserialize, Validate)]
    struct QueryProbe {
        #[validate(range(min = 1, max = 100))]
        page_size: Option<i64>,
        #[validate(length(max = 8))]
        start_user_id: Option<String>,
    }

    /// The status, message and field of `error`.
    fn parts(error: &ScreenError) -> (StatusCode, &str, Option<&str>) {
        (error.status(), error.message(), error.field())
    }

    /// The [`ScreenError`] `probe` fails validation with.
    fn rejection(probe: &Probe) -> Result<ScreenError, &'static str> {
        probe
            .validate()
            .err()
            .map(|errors| validation_error(&errors))
            .ok_or("the probe passed validation")
    }

    fn json_request(body: &str) -> Result<Request, axum::http::Error> {
        Request::builder()
            .method("POST")
            .uri("/probe")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
    }

    async fn query(uri: &str) -> Result<Result<QueryProbe, ScreenError>, axum::http::Error> {
        let (mut head, ()) = Request::builder().uri(uri).body(())?.into_parts();
        Ok(ValidQuery::<QueryProbe>::from_request_parts(&mut head, &())
            .await
            .map(|ValidQuery(value)| value))
    }

    /// The status and JSON body `uri` answers on a router with one [`RawPath`] route of a single
    /// segment and one of two, each echoing the text it extracted.
    async fn through_raw_path(uri: &str) -> Result<(StatusCode, Value), Box<dyn StdError>> {
        let router = Router::new()
            .route(
                "/probe/{key}",
                get(|RawPath(key): RawPath<String>| async move { Json(json!({ "key": key })) }),
            )
            .route(
                "/probe/{acct_id}/{auth_key}",
                get(
                    |RawPath((acct_id, auth_key)): RawPath<(String, String)>| async move {
                        Json(json!({ "acct_id": acct_id, "auth_key": auth_key }))
                    },
                ),
            );
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty())?)
            .await?;
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    #[test]
    fn page_size_resolves_to_the_request_or_the_default() {
        assert_eq!(resolve_page_size(None, 7), 7);
        assert_eq!(resolve_page_size(Some(25), 7), 25);
        assert_eq!(resolve_page_size(Some(MAX_PAGE_SIZE), 10), 100);
        assert_eq!(resolve_page_size(Some(-1), 5), 5);
        assert_eq!(resolve_page_size(Some(i64::MAX), 5), 5);
    }

    #[test]
    fn page_size_outside_one_to_one_hundred_is_the_only_422() -> TestResult {
        for size in [MAX_PAGE_SIZE + 1, MIN_PAGE_SIZE - 1, -1, i64::MAX, i64::MIN] {
            let probe = Probe {
                page_size: Some(size),
                ..Probe::default()
            };
            assert_eq!(
                parts(&rejection(&probe)?),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    PAGE_SIZE_MESSAGE,
                    Some(PAGE_SIZE_FIELD)
                ),
                "{size}"
            );
        }
        for size in [None, Some(MIN_PAGE_SIZE), Some(7), Some(MAX_PAGE_SIZE)] {
            let probe = Probe {
                page_size: size,
                ..Probe::default()
            };
            assert!(probe.validate().is_ok(), "{size:?}");
        }
        Ok(())
    }

    #[test]
    fn page_size_decides_over_every_other_failure() -> TestResult {
        let probe = Probe {
            page_size: Some(101),
            page_no: Some(0),
            user_id: "ABCDEFGHI".to_owned(),
            start_month: "AB".to_owned(),
            ..Probe::default()
        };
        assert_eq!(
            rejection(&probe)?.status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        Ok(())
    }

    #[test]
    fn a_failing_rule_is_a_400_naming_the_field_with_its_code_text() -> TestResult {
        let cases = [
            (
                Probe {
                    user_id: "ABCDEFGHI".to_owned(),
                    ..Probe::default()
                },
                "user_id",
                FIELD_LENGTH_MESSAGE,
            ),
            (
                // Eight characters within `length`, twelve bytes beyond the `PIC X(8)` field.
                Probe {
                    user_id: "ééééAAAA".to_owned(),
                    ..Probe::default()
                },
                "user_id",
                FIELD_LENGTH_MESSAGE,
            ),
            (
                Probe {
                    start_month: "1A".to_owned(),
                    ..Probe::default()
                },
                "start_month",
                FIELD_NUMERIC_MESSAGE,
            ),
            (
                Probe {
                    page_no: Some(0),
                    ..Probe::default()
                },
                "page_no",
                FIELD_RANGE_MESSAGE,
            ),
            (
                Probe {
                    inner: Inner {
                        code: "ABC".to_owned(),
                    },
                    ..Probe::default()
                },
                "inner.code",
                FIELD_LENGTH_MESSAGE,
            ),
            (
                Probe {
                    rows: vec![
                        Row::default(),
                        Row {
                            description: "X".repeat(51),
                        },
                    ],
                    ..Probe::default()
                },
                "rows[1].description",
                FIELD_LENGTH_MESSAGE,
            ),
        ];
        for (probe, field, message) in cases {
            assert_eq!(
                parts(&rejection(&probe)?),
                (StatusCode::BAD_REQUEST, message, Some(field)),
                "{probe:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn the_first_failing_field_in_key_order_is_reported() -> TestResult {
        let probe = Probe {
            user_id: "ABCDEFGHI".to_owned(),
            start_month: "123".to_owned(),
            rows: vec![
                Row::default(),
                Row {
                    description: "X".repeat(51),
                },
                Row {
                    description: "Y".repeat(60),
                },
            ],
            ..Probe::default()
        };
        // `rows` sorts before `start_month` and `user_id`, and item 1 before item 2.
        for _ in 0..16 {
            assert_eq!(rejection(&probe)?.field(), Some("rows[1].description"));
        }
        let probe = Probe {
            user_id: "ABCDEFGHI".to_owned(),
            start_month: "AB1".to_owned(),
            ..Probe::default()
        };
        // Both rules of `start_month` fail; the first error recorded decides the text.
        assert_eq!(
            parts(&rejection(&probe)?),
            (
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("start_month")
            )
        );
        Ok(())
    }

    #[test]
    fn numeric_admits_digits_and_blank_values_only() {
        for value in ["", "  ", "0", "07", "2022", "0123456789"] {
            assert!(numeric(value).is_ok(), "{value:?}");
        }
        for value in [
            "A", "1A", " 5", "5 ", "-3", "1.", "+1", "1,0", "\u{663}", "\t",
        ] {
            let error = numeric(value).err();
            assert_eq!(
                error.as_ref().map(|error| error.code.as_ref()),
                Some("numeric"),
                "{value:?}"
            );
            assert!(error.is_some_and(|error| error.message.is_none() && error.params.is_empty()));
        }
    }

    // audit: PIC-002
    #[test]
    fn pic_x_admits_what_its_field_holds_byte_for_byte_and_low_values() {
        let low_values = "\0".repeat(8);
        let nul_over_width = "\0".repeat(9);
        let wide_name = "é".repeat(10);
        let cases: &[(PicXRule, &str, bool)] = &[
            // Printable ASCII up to the width, blank values included.
            (pic_x::<8>, "PASSWORD", true),
            (pic_x::<8>, "PW", true),
            (pic_x::<8>, "", true),
            (pic_x::<8>, "        ", true),
            (pic_x::<8>, " !~AZaz9", true),
            (pic_x::<1>, "U", true),
            (pic_x::<20>, "LAWRENCE", true),
            // `LOW-VALUES`: NUL in every byte of the field.
            (pic_x::<8>, &low_values, true),
            (pic_x::<1>, "\0", true),
            // One byte more than the field.
            (pic_x::<8>, "PASSWORD9", false),
            (pic_x::<1>, "UA", false),
            // Within the width in characters or bytes, but not ASCII.
            (pic_x::<8>, "ééééAAAA", false),
            (pic_x::<8>, "é", false),
            (pic_x::<1>, "é", false),
            (pic_x::<20>, &wide_name, false),
            // Control bytes.
            (pic_x::<8>, "PASS\tWD", false),
            (pic_x::<8>, "PASS\u{7f}", false),
            // NUL bytes that are not `LOW-VALUES`.
            (pic_x::<8>, &nul_over_width, false),
            (pic_x::<8>, "\0\0\0", false),
            (pic_x::<8>, "AB\0\0\0\0\0\0", false),
        ];
        for &(rule, value, admitted) in cases {
            let error = rule(value).err();
            if admitted {
                assert!(error.is_none(), "{value:?}");
            } else {
                assert_eq!(
                    error.as_ref().map(|error| error.code.as_ref()),
                    Some(PIC_X_CODE),
                    "{value:?}"
                );
                assert!(
                    error.is_some_and(|error| error.message.is_none() && error.params.is_empty()),
                    "{value:?}"
                );
            }
        }
    }

    #[test]
    fn page_direction_travels_as_forward_and_backward() -> TestResult {
        for (direction, wire) in [
            (PageDirection::Forward, "\"forward\""),
            (PageDirection::Backward, "\"backward\""),
        ] {
            assert_eq!(serde_json::to_string(&direction)?, wire);
            assert_eq!(serde_json::from_str::<PageDirection>(wire)?, direction);
        }
        assert!(serde_json::from_str::<PageDirection>("\"Forward\"").is_err());
        assert_eq!(PageDirection::name(), "PageDirection");
        Ok(())
    }

    #[test]
    fn page_direction_maps_onto_every_program_direction() {
        use PageDirection::{Backward, Forward};
        macro_rules! assert_maps {
            ($($program:ident),+) => {$(
                assert_eq!($program::Direction::from(Forward), $program::Direction::Forward);
                assert_eq!($program::Direction::from(Backward), $program::Direction::Backward);
            )+};
        }
        assert_maps!(cocrdlic, cotrn00c, cousr00c, copaus0c, cotrtlic);
    }

    #[test]
    fn screen_result_carries_the_message_only() -> TestResult {
        let result = ScreenResult::from("User NEWUSR01 has been added ...".to_owned());
        assert_eq!(
            serde_json::to_value(&result)?,
            serde_json::json!({"message": "User NEWUSR01 has been added ..."})
        );
        assert_eq!(ScreenResult::name(), "ScreenResult");
        Ok(())
    }

    #[tokio::test]
    async fn json_bodies_are_deserialised_then_validated() -> TestResult {
        let ValidJson(probe) =
            ValidJson::<Probe>::from_request(json_request(r#"{"user_id":"USER0001"}"#)?, &())
                .await
                .map_err(|error| error.to_string())?;
        assert_eq!(probe.user_id, "USER0001");

        let too_long =
            ValidJson::<Probe>::from_request(json_request(r#"{"user_id":"USER00010"}"#)?, &())
                .await
                .err()
                .ok_or("over-long user ID accepted")?;
        assert_eq!(
            parts(&too_long),
            (
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("user_id")
            )
        );

        let oversized =
            ValidJson::<Probe>::from_request(json_request(r#"{"page_size":101}"#)?, &())
                .await
                .err()
                .ok_or("page_size 101 accepted")?;
        assert_eq!(oversized.status(), StatusCode::UNPROCESSABLE_ENTITY);
        Ok(())
    }

    #[tokio::test]
    async fn unreadable_bodies_are_a_400_that_echoes_nothing() -> TestResult {
        let mut requests = vec![
            json_request(&format!(r#"{{"page_size":"{CARD}"}}"#))?,
            json_request(&format!(r#"{{"user_id":{CARD}}}"#))?,
            json_request(&format!(r#"{{"user_id":"{CARD}""#))?,
        ];
        requests.push(
            Request::builder()
                .method("POST")
                .uri("/probe")
                .body(Body::from(format!(r#"{{"user_id":"{CARD}"}}"#)))?,
        );
        for request in requests {
            let error = ValidJson::<Probe>::from_request(request, &())
                .await
                .err()
                .ok_or("unreadable body accepted")?;
            assert_eq!(parts(&error), (StatusCode::BAD_REQUEST, INVALID_BODY, None));
            assert!(!error.message().contains("4111"));
        }

        // A value that deserialises but breaks a rule is not echoed either.
        let error = ValidJson::<Probe>::from_request(
            json_request(&format!(r#"{{"user_id":"{CARD}"}}"#))?,
            &(),
        )
        .await
        .err()
        .ok_or("over-long card number accepted")?;
        let body = serde_json::to_string(&error)?;
        assert!(!body.contains("4111"), "{body}");
        Ok(())
    }

    #[tokio::test]
    async fn query_strings_are_deserialised_then_validated() -> TestResult {
        let probe = query("/users?page_size=7&start_user_id=USER0001")
            .await?
            .map_err(|error| error.to_string())?;
        assert_eq!(probe.page_size, Some(7));
        assert_eq!(probe.start_user_id.as_deref(), Some("USER0001"));

        let probe = query("/users").await?.map_err(|error| error.to_string())?;
        assert_eq!(probe.page_size, None);

        for uri in [
            "/users?page_size=101",
            "/users?page_size=0",
            "/users?page_size=-1",
        ] {
            let error = query(uri)
                .await?
                .err()
                .ok_or("out-of-range page_size accepted")?;
            assert_eq!(
                parts(&error),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    PAGE_SIZE_MESSAGE,
                    Some(PAGE_SIZE_FIELD)
                ),
                "{uri}"
            );
        }

        let error = query("/users?start_user_id=USER00010")
            .await?
            .err()
            .ok_or("over-long filter accepted")?;
        assert_eq!(
            parts(&error),
            (
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("start_user_id")
            )
        );

        for uri in [
            format!("/users?page_size={CARD}x"),
            format!("/users?page_size={CARD}{CARD}"),
            "/users?page_size=7.5".to_owned(),
        ] {
            let error = query(&uri)
                .await?
                .err()
                .ok_or("unreadable query accepted")?;
            assert_eq!(
                parts(&error),
                (StatusCode::BAD_REQUEST, INVALID_QUERY, None)
            );
            assert!(!error.message().contains("4111"));
        }
        Ok(())
    }

    #[test]
    fn raw_path_segments_pass_as_decoded_or_are_a_400_that_echoes_nothing() -> TestResult {
        let rejected = || (StatusCode::BAD_REQUEST, json!({ "message": INVALID_PATH }));
        let cases = [
            // Decodable text passes untrimmed and unparsed, for the program's own edit to judge.
            (
                "/probe/00000000001".to_owned(),
                (StatusCode::OK, json!({ "key": "00000000001" })),
            ),
            (
                "/probe/%20%201A%20".to_owned(),
                (StatusCode::OK, json!({ "key": "  1A " })),
            ),
            (
                "/probe/%C3%A9x".to_owned(),
                (StatusCode::OK, json!({ "key": "éx" })),
            ),
            (
                "/probe/00000000001/0778000000000001".to_owned(),
                (
                    StatusCode::OK,
                    json!({ "acct_id": "00000000001", "auth_key": "0778000000000001" }),
                ),
            ),
            // A segment that is not UTF-8 once decoded, in either position, is refused without
            // its text.
            ("/probe/%FF".to_owned(), rejected()),
            (format!("/probe/{CARD}%FF"), rejected()),
            ("/probe/%FF/0778000000000001".to_owned(), rejected()),
            (format!("/probe/00000000001/{CARD}%FE"), rejected()),
        ];
        // A failure names the case by its index: two of the URIs hold a card number.
        block_on(async {
            for (case, (uri, expected)) in cases.into_iter().enumerate() {
                assert_eq!(through_raw_path(&uri).await?, expected, "case {case}");
            }
            Ok(())
        })
    }

    #[test]
    fn pic_fields_render_as_the_screen_shows_them() -> TestResult {
        assert_eq!(text(&PicX::<8>::from_text(" AB C")?), " AB C");
        assert_eq!(text(&PicX::<4>::spaces()), "");
        assert_eq!(full_text(&PicX::<8>::from_text(" AB C")?), " AB C   ");
        assert_eq!(full_text(&PicX::<4>::spaces()), "    ");
        assert_eq!(full_text(&PicX::<3>::from_text("XYZ")?), "XYZ");
        assert_eq!(digits(PicU::<11>::new(50)?), "00000000050");
        assert_eq!(digits(PicU::<9>::ZERO), "000000000");
        Ok(())
    }
}
