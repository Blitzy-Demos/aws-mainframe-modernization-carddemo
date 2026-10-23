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
//!   JSON, a missing JSON content type, a body whose top-level value is not a JSON object (an
//!   array, even one whose elements would fill the members in order, `null`, a string, a number
//!   or a boolean), the same in a nested request member or list item whose schema is an object,
//!   which reads through `object`, `objects` or `objects_or_null` (`fields` and `snapshot`
//!   of endpoints 6 and 10, `fields` of endpoint 13, and the `rows` items, `state`, and the
//!   `state.rows` and `state.screen_rows` items of endpoint 25, where `null` stays an empty
//!   position), a path segment whose percent-encoding is not UTF-8, a
//!   value of the wrong type (text where an integer belongs, for instance) and an unknown enum
//!   value. An attention key whose program answers unserved keys itself (`cotrtupc`,
//!   `cotrn02c`) reads every other key name as a hidden variant instead, so that the program's
//!   invalid-key message, not this rejection, answers it.
//! - A rule a DTO declares: 400 naming the first failing field in key order, as a path such as
//!   `user_id`, `fields.zip` or `rows[3].description`, with the text of the rule's code
//!   ([`FIELD_LENGTH_MESSAGE`] for `length` and [`PIC_X_CODE`], [`FIELD_NUMERIC_MESSAGE`] or
//!   [`FIELD_RANGE_MESSAGE`]).
//! - A `page_size` that is not an integer from [`MIN_PAGE_SIZE`] to [`MAX_PAGE_SIZE`], be it out
//!   of range, beyond `i64`, beyond the range of the JSON parser or no integer at all: 422 with
//!   [`PAGE_SIZE_MESSAGE`] and field `page_size`, whatever other rule failed. It is the only 422
//!   of the API.
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
//! `#[param(minimum = 1, maximum = 100)]`, and reads it with `#[serde(default, deserialize_with
//! = ..)]` through `page_size_from_query` on a query string or `page_size_from_json` on
//! endpoint 25's body. Neither refuses a value, so every `page_size` that is not an integer from
//! 1 to 100 reaches the range rule as an `i64` outside it, which answers 422: `0`, a negative
//! value and `101` as themselves, and an integer beyond `i64` and anything else that is no
//! integer (text such as `abc`, an empty value or `1.5`, and in a body also a string, a boolean,
//! an array or an object) as a value outside the range. So is a number in the body beyond the
//! range the JSON parser represents, such as an integer of 310 digits or `1e400`, which
//! `serde_json` refuses before `page_size_from_json` sees it: [`ValidJson`] reads the body again
//! with that number replaced by a size outside the range. The field is `i64` rather than an
//! unsigned type for the same reason. A JSON `null` is an absent size. Another member that does
//! not deserialise still makes the request the 400 above, whatever its `page_size`. The screens
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

use std::fmt;
use std::marker::PhantomData;

use axum::Json;
use axum::body::{Body, Bytes};
use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Path, Query, Request};
use axum::http::request::Parts;
use domain::copybook::{PicU, PicX};
use serde::de::value::MapAccessDeserializer;
use serde::de::{DeserializeOwned, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use serde_json::error::Category;
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

/// The `page_size` a value that is not an integer at all reads as: one below [`MIN_PAGE_SIZE`],
/// so that the range rule answers it with the same 422 as every other size outside the range.
const NON_INTEGER_PAGE_SIZE: i64 = MIN_PAGE_SIZE - 1;

/// `page_size` from a query string, for `#[serde(default, deserialize_with =
/// "super::page_size_from_query")]` on the member of a collection query.
///
/// It never fails, so that every value reaches the range rule, which answers anything outside
/// [`MIN_PAGE_SIZE`] to [`MAX_PAGE_SIZE`] with the 422 of Bounded Pagination rather than the 400
/// of a query that does not deserialise. The text is read as [`page_size_from_text`] reads it
/// and is not kept, so no rejection can echo it.
fn page_size_from_query<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.map(|text| page_size_from_text(&text)))
}

/// The integer a query-string `page_size` holds, as `str::parse::<i64>` reads it, so that `7`,
/// `+7` and `007` are 7 and `0101` is 101. An integer beyond `i64`, digits after an optional
/// sign, saturates to `i64::MAX` or `i64::MIN`; any other text, the empty value included, is
/// [`NON_INTEGER_PAGE_SIZE`]. Each of those lies outside the range.
fn page_size_from_text(text: &str) -> i64 {
    text.parse::<i64>()
        .unwrap_or_else(|error| match error.kind() {
            std::num::IntErrorKind::PosOverflow => i64::MAX,
            std::num::IntErrorKind::NegOverflow => i64::MIN,
            _ => NON_INTEGER_PAGE_SIZE,
        })
}

/// `page_size` from a JSON body, for `#[serde(default, deserialize_with =
/// "super::page_size_from_json")]` on the member of endpoint 25's request.
///
/// It fails only where `serde_json` cannot read the member's value at all, so that every value
/// reaches the range rule and its 422, as in [`page_size_from_query`]. A number beyond the range
/// of a 64-bit float, which the parser refuses before this function runs, reaches it through
/// `read_past_unrepresentable_page_size` as [`NON_INTEGER_PAGE_SIZE`]. `null` is an absent size
/// and an integer `i64` holds is itself. One above `i64::MAX` that `u64` holds saturates to
/// `i64::MAX`. Every other value is [`NON_INTEGER_PAGE_SIZE`]: a number with a fraction or an
/// exponent, an integer below `i64::MIN` or above `u64::MAX`, which `serde_json` reads as such
/// a number, a string (`"7"` included, as the member is a JSON integer), a boolean, an array
/// and an object. The value is not kept, so no rejection can echo it.
fn page_size_from_json<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Null => None,
        serde_json::Value::Number(number) => Some(
            number
                .as_i64()
                .or_else(|| number.as_u64().map(|_| i64::MAX))
                .unwrap_or(NON_INTEGER_PAGE_SIZE),
        ),
        serde_json::Value::Bool(_)
        | serde_json::Value::String(_)
        | serde_json::Value::Array(_)
        | serde_json::Value::Object(_) => Some(NON_INTEGER_PAGE_SIZE),
    })
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
/// digit, apart from spaces before and after the digits ([`numeric`]).
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
/// content type is required, and so is a JSON object as the top-level value and in every member
/// or list item whose schema is an object: an array, `null`, a string, a number or a boolean is
/// refused there before any rule runs, even an array whose elements would fill the members in
/// order, and `null` stays an empty position where the list admits one), otherwise the error of
/// the first failing rule, or the 422 of an out-of-range `page_size`.
///
/// A top-level `page_size` that holds a JSON number beyond the range the JSON parser represents,
/// such as an integer of 310 digits or `1e400`, is read as a size outside the range too, so it
/// gets the same 422. `serde_json` refuses such a number while parsing it, before any rule or
/// `deserialize_with` function sees it, so the body is buffered under the route's body limit
/// and read again, as `read_past_unrepresentable_page_size` sets out. A body that does not
/// deserialise for any other reason, beside that number or without it, stays the 400.
#[derive(Debug, Clone)]
pub struct ValidJson<T>(pub T);

impl<T, S> FromRequest<S> for ValidJson<T>
where
    T: DeserializeOwned + Validate,
    S: Send + Sync,
{
    type Rejection = ScreenError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        // Buffered once, under the route's body limit, so that a body can be read a second time
        // below; axum's own extractor then reads the copy, its content-type rule included.
        let (parts, body) = req.into_parts();
        let bytes = Bytes::from_request(Request::from_parts(parts.clone(), body), state)
            .await
            .map_err(|_| ScreenError::input(INVALID_BODY, None))?;
        let read = Json::<ObjectBody<T>>::from_request(
            Request::from_parts(parts, Body::from(bytes.clone())),
            state,
        )
        .await;
        // A rejection's own text quotes the offending input, so it is dropped unread.
        let value = match read {
            Ok(Json(ObjectBody(value))) => value,
            Err(JsonRejection::JsonSyntaxError(_)) => {
                read_past_unrepresentable_page_size(&bytes)
                    .ok_or_else(|| ScreenError::input(INVALID_BODY, None))?
            }
            Err(_) => return Err(ScreenError::input(INVALID_BODY, None)),
        };
        value
            .validate()
            .map_err(|errors| validation_error(&errors))?;
        Ok(Self(value))
    }
}

/// `T` read from `bytes` when the one reason it does not deserialise is a top-level `page_size`
/// holding a JSON number beyond the range `serde_json` represents; `None` otherwise.
///
/// `serde_json`, built without arbitrary precision, refuses a number beyond the 64-bit float range
/// with a syntax error while it parses the number, so `page_size_from_json` never receives it.
/// Such a number lies outside [`MIN_PAGE_SIZE`] to [`MAX_PAGE_SIZE`] whatever its sign, so it is
/// replaced by [`NON_INTEGER_PAGE_SIZE`], a size the range rule answers with 422, and the body
/// is read again. Each step below must hold, or the body stays the 400 of a body that does not
/// deserialise:
///
/// 1. The body is well-formed JSON. `IgnoredAny` checks the grammar of every number without
///    converting it, so a malformed number such as `1.e5`, a truncated body and text after the
///    value fail here.
/// 2. `T` stops at a syntax error, and [`PageSizeMember`], which reads the top-level `page_size`
///    alone and skips every other member, stops at the same line and column with a syntax
///    error: the fault lies inside that member's value, and no member before it failed.
/// 3. That position, just past the last byte the parser consumed, falls inside a run of number
///    bytes that is, on its own, a well-formed JSON number the parser cannot represent.
/// 4. `T` deserialises from the body with that number replaced. A member that does not
///    deserialise after `page_size` still fails here, as one before it fails `T`'s first
///    reading, so the answer does not depend on member order.
///
/// Only error positions and categories are read, never an error's text, which quotes the input.
fn read_past_unrepresentable_page_size<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    serde_json::from_slice::<IgnoredAny>(bytes).ok()?;
    let failure = serde_json::from_slice::<ObjectBody<T>>(bytes).err()?;
    let probe = serde_json::from_slice::<PageSizeMember>(bytes).err()?;
    let same_fault = failure.classify() == Category::Syntax
        && probe.classify() == Category::Syntax
        && (probe.line(), probe.column()) == (failure.line(), failure.column());
    if !same_fault {
        return None;
    }
    let offset = byte_offset(bytes, failure.line(), failure.column())?;
    let (start, end) = unrepresentable_number_at(bytes, offset)?;
    let mut patched = Vec::with_capacity(bytes.len());
    patched.extend_from_slice(bytes.get(..start)?);
    patched.extend_from_slice(NON_INTEGER_PAGE_SIZE.to_string().as_bytes());
    patched.extend_from_slice(bytes.get(end..)?);
    Json::<ObjectBody<T>>::from_bytes(&patched)
        .ok()
        .map(|Json(ObjectBody(value))| value)
}

/// A JSON object read for its top-level `page_size` member alone, as `page_size_from_json`
/// reads that member's value, every other member skipped unread: the reading
/// `read_past_unrepresentable_page_size` compares a DTO's failure with. Anything but an object
/// fails at once, as [`ObjectBody`] does.
struct PageSizeMember;

impl<'de> Deserialize<'de> for PageSizeMember {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(PageSizeMember)
    }
}

impl<'de> Visitor<'de> for PageSizeMember {
    type Value = Self;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self, A::Error>
    where
        A: MapAccess<'de>,
    {
        while let Some(key) = map.next_key::<String>()? {
            if key == PAGE_SIZE_FIELD {
                map.next_value::<serde_json::Value>()?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(self)
    }
}

/// The byte offset in `bytes` of the position a `serde_json` error over a slice reports: `line`
/// counts from 1, and `column` is the reader's index less the offset at which its line starts.
/// `None` for line 0, which a syntax error never reports, or for a position beyond the body.
fn byte_offset(bytes: &[u8], line: usize, column: usize) -> Option<usize> {
    let line_start = match line {
        0 => return None,
        1 => 0,
        _ => {
            let (newline, _) = bytes
                .iter()
                .enumerate()
                .filter(|&(_, &byte)| byte == b'\n')
                .nth(line.checked_sub(2)?)?;
            newline.checked_add(1)?
        }
    };
    line_start
        .checked_add(column)
        .filter(|&offset| offset <= bytes.len())
}

/// The bounds of the JSON number that `offset`, the parser's index just past the last byte it
/// consumed, falls inside: the run of number bytes before `offset` and on after it, as the
/// parser stops inside the exponent of a number whose exponent overflows. `None` unless a
/// number byte precedes `offset` and the run, on its own, is a well-formed JSON number that
/// `serde_json` cannot represent: `IgnoredAny` accepts it and `Value` refuses it.
fn unrepresentable_number_at(bytes: &[u8], offset: usize) -> Option<(usize, usize)> {
    let (before, after) = bytes.split_at_checked(offset)?;
    let start = match before.iter().rposition(|&byte| !is_number_byte(byte)) {
        Some(delimiter) => delimiter.checked_add(1)?,
        None => 0,
    };
    let run_after = after
        .iter()
        .position(|&byte| !is_number_byte(byte))
        .unwrap_or(after.len());
    let end = offset.checked_add(run_after)?;
    let token = bytes.get(start..end)?;
    let unrepresentable = start < offset
        && serde_json::from_slice::<IgnoredAny>(token).is_ok()
        && serde_json::from_slice::<serde_json::Value>(token).is_err();
    unrepresentable.then_some((start, end))
}

/// Whether `byte` can belong to a JSON number: a digit, a sign, a decimal point or an exponent
/// marker.
fn is_number_byte(byte: u8) -> bool {
    byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.' | b'e' | b'E')
}

/// A JSON object deserialised into `T`: a request body, or through [`object`], [`objects`] and
/// [`objects_or_null`] a request member or list item whose schema is an object.
///
/// serde's derived `Deserialize` of a struct also reads a JSON array, element by element in
/// member order, so `["ADMIN001","PASSWORD"]` would read as a sign-on request, `[]` as `{}`, and
/// `"fields":["Y","2014"]` as an account update's fields. Every request body is a `type: object`
/// schema, and so is every struct it nests, so this wrapper asks the deserialiser for a map and
/// nothing else: an array, a string, a number, a boolean or `null` in its place fails to
/// deserialise, at the top level and at every depth whose member reads through these helpers.
/// The object's members then reach `T`'s own derived visitor exactly as they arrive, which keeps
/// its rejection of a duplicate member, its defaults for omitted members, its reading of unknown
/// ones and the `deserialize_with` rules of its own members.
struct ObjectBody<T>(T);

impl<'de, T> Deserialize<'de> for ObjectBody<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        object(deserializer).map(Self)
    }
}

/// A request member whose schema is an object, for `#[serde(deserialize_with =
/// "super::object")]` on a struct-typed member of a request DTO.
///
/// # Errors
///
/// The deserialiser's invalid-type error when the member holds anything but a JSON object, an
/// array whose elements would fill `T`'s members in order included, or `T`'s own error for the
/// object's members. Neither reaches a response: [`ValidJson`] answers any of them with
/// [`INVALID_BODY`] and no field.
pub(crate) fn object<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    deserializer.deserialize_map(ObjectVisitor(PhantomData))
}

/// A request member whose schema is a list of objects, for `#[serde(deserialize_with =
/// "super::objects")]` on a `Vec<T>` member of a request DTO, or for a `deserialize_with` rule
/// that reads the list through it before checking it further.
///
/// # Errors
///
/// As [`object`], for the member when it is not a JSON array and for each item that is not a
/// JSON object, `null` included.
pub(crate) fn objects<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Vec::<ObjectBody<T>>::deserialize(deserializer)?
        .into_iter()
        .map(|ObjectBody(item)| item)
        .collect())
}

/// A request member whose schema is a list of positions, each an object or `null`, for
/// `#[serde(deserialize_with = "super::objects_or_null")]` on a `Vec<Option<T>>` member of a
/// request DTO: `null` is an empty position, as before.
///
/// # Errors
///
/// As [`object`], for the member when it is not a JSON array and for each item that is neither
/// a JSON object nor `null`.
pub(crate) fn objects_or_null<'de, D, T>(deserializer: D) -> Result<Vec<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Vec::<Option<ObjectBody<T>>>::deserialize(deserializer)?
        .into_iter()
        .map(|item| item.map(|ObjectBody(item)| item))
        .collect())
}

/// The visitor of [`ObjectBody`] and [`object`]: it accepts a map alone and hands it to `T`, so
/// a sequence or a scalar is the deserialiser's invalid-type error.
struct ObjectVisitor<T>(PhantomData<T>);

impl<'de, T> Visitor<'de> for ObjectVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<A>(self, map: A) -> Result<T, A::Error>
    where
        A: MapAccess<'de>,
    {
        T::deserialize(MapAccessDeserializer::new(map))
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

/// The `NUM` attribute of a map field, as a `validator` custom rule after the field's `length`
/// and [`pic_x`]: `#[validate(length(max = 2), custom(function = "super::pic_x::<2>"),
/// custom(function = "super::numeric"))]`, so that text the field cannot hold is answered with
/// [`FIELD_LENGTH_MESSAGE`] before this rule is reported.
///
/// Admits ASCII digits with any number of spaces before and after them, `^ *[0-9]* *$`: the
/// characters `modern/audit/screens.md` says the numeric field admits (`CORPT00-022` to
/// `CORPT00-034`, X-CORPT00C-5), which `NUMVAL-C` reads as the value of the digits
/// (CORPT00C-017: `" 5"` and `"5 "` are both `05`). A blank value, empty or all spaces, is
/// admitted too, and the program's own blank-field edit answers it with its message.
///
/// # Errors
///
/// A [`ValidationError`] with code [`NUMERIC_CODE`] for any other value: a space between two
/// digits, a sign, a decimal point, a comma, a letter, a non-ASCII digit or a control byte such
/// as NUL, `LOW-VALUES` that [`pic_x`] admits included. The error carries no message.
pub fn numeric(value: &str) -> Result<(), ValidationError> {
    let digits = value.trim_matches(' ');
    if digits.bytes().all(|byte| byte.is_ascii_digit()) {
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
    use domain::online::{coactupc, cocrdlic, copaus0c, cotrn00c, cotrtlic, cousr00c};
    use serde::Deserialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use tower::ServiceExt as _;
    use utoipa::ToSchema;
    use validator::{Validate, ValidationError};

    use super::coactupc::{AccountUpdateRequest, AccountUpdateSnapshot};
    use super::cobil00c::BillPaymentRequest;
    use super::cocrdupc::CardUpdateRequest;
    use super::corpt00c::ReportRequest;
    use super::cosgn00c::SignonRequest;
    use super::cotrn02c::TranAddRequest;
    use super::cotrtlic::TranTypeListActionRequest;
    use super::cotrtupc::TranTypeActionRequest;
    use super::cousr01c::UserAddRequest;
    use super::cousr02c::UserUpdateRequest;
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
        #[serde(default, deserialize_with = "super::page_size_from_json")]
        #[validate(range(min = 1, max = 100))]
        page_size: Option<i64>,
        #[validate(range(min = 1))]
        page_no: Option<i64>,
        #[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]
        user_id: String,
        #[validate(length(max = 2), custom(function = "super::numeric"))]
        start_month: String,
        #[serde(deserialize_with = "super::object")]
        #[validate(nested)]
        inner: Inner,
        #[serde(deserialize_with = "super::objects")]
        #[validate(nested)]
        rows: Vec<Row>,
    }

    /// A query-string request: page size, page number and one text filter.
    #[derive(Debug, Deserialize, Validate)]
    struct QueryProbe {
        #[serde(default, deserialize_with = "super::page_size_from_query")]
        #[validate(range(min = 1, max = 100))]
        page_size: Option<i64>,
        #[validate(range(min = 0))]
        page_no: Option<i64>,
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

    /// Asserts that `body`, posted to a `ValidJson<T>` extractor, is the 400 of a body that does
    /// not deserialise: [`INVALID_BODY`] and no field.
    async fn assert_unreadable<T: DeserializeOwned + Validate>(body: &str) -> TestResult {
        let dto = std::any::type_name::<T>();
        let error = ValidJson::<T>::from_request(json_request(body)?, &())
            .await
            .err()
            .ok_or_else(|| format!("{dto} accepted {body}"))?;
        assert_eq!(
            parts(&error),
            (StatusCode::BAD_REQUEST, INVALID_BODY, None),
            "{dto}: {body}"
        );
        Ok(())
    }

    /// Asserts that `body` reads through `ValidJson<T>`, then replaces the object at each of
    /// `members`, a JSON pointer into `body` with the array that fills that object's members in
    /// order, by that array, an empty array, a string, a number and a boolean in turn. Asserts
    /// that each refusal is the 400 of a body that does not deserialise, and returns every
    /// replacement `T` accepted instead, as `type pointer = value`.
    async fn object_members_accepted<T: DeserializeOwned + Validate>(
        body: &Value,
        members: &[(&str, Value)],
    ) -> Result<Vec<String>, Box<dyn StdError>> {
        let dto = std::any::type_name::<T>();
        ValidJson::<T>::from_request(json_request(&body.to_string())?, &())
            .await
            .map_err(|error| format!("{dto} refused its object body: {error}"))?;
        let mut accepted = Vec::new();
        for (pointer, positional) in members {
            if !body.pointer(pointer).is_some_and(Value::is_object) {
                return Err(format!("{dto}: no object at {pointer}").into());
            }
            for value in [
                positional.clone(),
                json!([]),
                json!("x"),
                json!(7),
                json!(true),
            ] {
                let mut altered = body.clone();
                *altered.pointer_mut(pointer).ok_or(*pointer)? = value.clone();
                match ValidJson::<T>::from_request(json_request(&altered.to_string())?, &()).await {
                    Ok(_) => accepted.push(format!("{dto} {pointer} = {value}")),
                    Err(error) => assert_eq!(
                        parts(&error),
                        (StatusCode::BAD_REQUEST, INVALID_BODY, None),
                        "{dto} {pointer} = {value}"
                    ),
                }
            }
        }
        Ok(accepted)
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
    fn numeric_admits_digits_with_surrounding_spaces_and_blank_values_only() {
        for value in [
            "",
            "  ",
            "    ",
            "0",
            "07",
            "2022",
            "0123456789",
            " 5",
            "5 ",
            " 22 ",
            "  22",
            "22  ",
        ] {
            assert!(numeric(value).is_ok(), "{value:?}");
        }
        for value in [
            "A", "AB", "1A", "1 2", " 1 2 ", "-3", "1.", "+1", "1,0", "\u{663}", "\t", " 5\t",
            "\0", "\0\0", "5\0",
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

        // Out of range, beyond `i64` or no integer at all: the 422, which echoes nothing.
        for size in [
            "101".to_owned(),
            format!("{CARD}{CARD}"),
            format!("\"{CARD}\""),
            "1.5".to_owned(),
            "true".to_owned(),
            format!("[{CARD}]"),
        ] {
            let body = format!(r#"{{"page_size":{size},"user_id":"USER00010"}}"#);
            let error = ValidJson::<Probe>::from_request(json_request(&body)?, &())
                .await
                .err()
                .ok_or("page_size accepted")?;
            assert_eq!(
                parts(&error),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    PAGE_SIZE_MESSAGE,
                    Some(PAGE_SIZE_FIELD)
                )
            );
            assert!(!serde_json::to_string(&error)?.contains("4111"));
        }
        Ok(())
    }

    #[tokio::test]
    async fn unreadable_bodies_are_a_400_that_echoes_nothing() -> TestResult {
        let mut requests = vec![
            json_request(&format!(r#"{{"page_no":"{CARD}"}}"#))?,
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
    async fn a_body_that_is_not_a_json_object_is_a_400_before_any_rule() -> TestResult {
        let bodies = [
            "[]",
            // Positional arrays that would fill the members in order: a sign-on, the same with a
            // user ID too long for its rule, a bill payment and a confirmed Monthly report.
            r#"["ADMIN001","PASSWORD"]"#,
            r#"["USER00010","PASSWORD"]"#,
            r#"["00000000033","Y"]"#,
            r#"["X","","","","","","","","","Y"]"#,
            "null",
            r#""x""#,
            "1",
            "true",
        ];
        for body in bodies {
            assert_unreadable::<Probe>(body).await?;
            assert_unreadable::<SignonRequest>(body).await?;
            assert_unreadable::<AccountUpdateRequest>(body).await?;
            assert_unreadable::<CardUpdateRequest>(body).await?;
            assert_unreadable::<TranAddRequest>(body).await?;
            assert_unreadable::<ReportRequest>(body).await?;
            assert_unreadable::<BillPaymentRequest>(body).await?;
            assert_unreadable::<UserAddRequest>(body).await?;
            assert_unreadable::<UserUpdateRequest>(body).await?;
            assert_unreadable::<TranTypeListActionRequest>(body).await?;
            assert_unreadable::<TranTypeActionRequest>(body).await?;
        }
        Ok(())
    }

    /// Object bodies of endpoints 6, 10, 13 and 25, in that order, that read and pass every rule,
    /// with every nested object present.
    fn nested_object_bodies() -> [Value; 4] {
        let row = json!({"type_code": "01", "description": "PURCHASE"});
        let blank_snapshot = AccountUpdateSnapshot::from(coactupc::Snapshot::from_fields(
            &coactupc::SnapshotFields::<&str>::default(),
        ));
        [
            json!({
                "aid": "ENTER",
                "fields": {"active_status": "Y", "open_year": "2014"},
                "snapshot": blank_snapshot
            }),
            json!({
                "aid": "ENTER",
                "fields": {
                    "embossed_name": "TEST HOLDER",
                    "status": "N",
                    "expiry_month": "06",
                    "expiry_year": "2030"
                },
                "snapshot": {
                    "state": "show_details",
                    "acct_id": "00000000050",
                    "embossed_name": "TEST HOLDER",
                    "expiration_date": "2025-04-30",
                    "status": "Y"
                }
            }),
            json!({
                "aid": "ENTER",
                "acct_id": "00000000050",
                "fields": {"type_cd": "01", "cat_cd": "0001"},
                "confirm": "Y"
            }),
            json!({
                "aid": "ENTER",
                "rows": [{"selection": "U", "description": "NEW"}],
                "state": {
                    "type_filter": "",
                    "desc_filter": "",
                    "rows": [row, null],
                    "selected_row": null,
                    "first_key": "01",
                    "last_key": "01",
                    "page_no": 1,
                    "next_page_exists": false,
                    "last_page_shown": false,
                    "pending_action": "none",
                    "screen_rows": [row, null],
                    "selectors": "open",
                    "description_entry": null
                }
            }),
        ]
    }

    #[tokio::test]
    async fn a_nested_member_whose_schema_is_an_object_refuses_anything_else() -> TestResult {
        let [account, card, transaction, list_action] = nested_object_bodies();
        // `AccountUpdateSnapshot`'s members in declaration order: six texts, three dates,
        // seventeen texts, the date of birth and three texts.
        let texts = |count| std::iter::repeat_n(json!(""), count);
        let positional_snapshot: Vec<Value> = texts(6)
            .chain(std::iter::repeat_n(Value::Null, 3))
            .chain(texts(17))
            .chain([Value::Null])
            .chain(texts(3))
            .collect();
        // Each nested object with the array that fills its members in declaration order.
        let mut accepted = object_members_accepted::<AccountUpdateRequest>(
            &account,
            &[
                ("/fields", json!(["Y", "2014"])),
                ("/snapshot", Value::Array(positional_snapshot)),
            ],
        )
        .await?;
        accepted.extend(
            object_members_accepted::<CardUpdateRequest>(
                &card,
                &[
                    ("/fields", json!(["TEST HOLDER", "N", "06", "2030"])),
                    (
                        "/snapshot",
                        json!([
                            "show_details",
                            "00000000050",
                            "TEST HOLDER",
                            "2025-04-30",
                            "Y"
                        ]),
                    ),
                ],
            )
            .await?,
        );
        accepted.extend(
            object_members_accepted::<TranAddRequest>(
                &transaction,
                &[("/fields", json!(["01", "0001"]))],
            )
            .await?,
        );
        accepted.extend(
            object_members_accepted::<TranTypeListActionRequest>(
                &list_action,
                &[
                    ("/rows/0", json!(["U", "NEW"])),
                    (
                        "/state",
                        json!([
                            "",
                            "",
                            [],
                            null,
                            null,
                            null,
                            1,
                            false,
                            false,
                            "none",
                            [],
                            "open",
                            null
                        ]),
                    ),
                    ("/state/rows/0", json!(["01", "PURCHASE"])),
                    ("/state/screen_rows/0", json!(["01", "PURCHASE"])),
                ],
            )
            .await?,
        );
        assert!(
            accepted.is_empty(),
            "read in place of an object: {accepted:#?}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn an_object_body_reads_as_it_always_has() -> TestResult {
        // Unknown members are skipped and omitted ones take their defaults.
        let body = r#" {"rows":[{"description":"X"}],"unknown":[1],"user_id":"USER0001"} "#;
        let ValidJson(probe) = ValidJson::<Probe>::from_request(json_request(body)?, &())
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(
            (
                probe.user_id.as_str(),
                probe.rows.len(),
                probe.inner.code.as_str()
            ),
            ("USER0001", 1, "")
        );
        // The same in a nested object and in each object of a list, which keep their own rules.
        let body =
            r#"{"inner":{"code":"ABC","unknown":[1]},"rows":[{},{"description":"X","x":[]}]}"#;
        let probe = serde_json::from_str::<Probe>(body)?;
        assert_eq!(
            (
                probe.inner.code.as_str(),
                probe.rows.first().map(|row| row.description.as_str()),
                probe.rows.get(1).map(|row| row.description.as_str())
            ),
            ("ABC", Some(""), Some("X"))
        );
        let error = ValidJson::<Probe>::from_request(json_request(body)?, &())
            .await
            .err()
            .ok_or("over-long nested code accepted")?;
        assert_eq!(
            parts(&error),
            (
                StatusCode::BAD_REQUEST,
                FIELD_LENGTH_MESSAGE,
                Some("inner.code")
            )
        );
        // A duplicate member at any depth and text after the object are still unreadable,
        // before any rule.
        for body in [
            r#"{"user_id":"USER0001","user_id":"USER0002"}"#,
            r#"{"user_id":"USER00010","user_id":"USER0001"}"#,
            r#"{"inner":{"code":"A","code":"B"}}"#,
            r#"{"rows":[{"description":"A","description":"B"}]}"#,
            r#"{"user_id":"USER0001"} []"#,
        ] {
            assert_unreadable::<Probe>(body).await?;
        }
        Ok(())
    }

    /// The JSON object text of `object` with `first` as its first member and `last` as its last,
    /// each the raw text of one member or empty for none, so that a member can hold a number no
    /// `Value` holds.
    fn members_around(
        first: &str,
        object: &Value,
        last: &str,
    ) -> Result<String, Box<dyn StdError>> {
        let text = object.to_string();
        let inner = text
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
            .ok_or("not a JSON object")?;
        let members: Vec<&str> = [first, inner, last]
            .into_iter()
            .filter(|member| !member.is_empty())
            .collect();
        Ok(format!("{{{}}}", members.join(",")))
    }

    /// `object` with a last `page_size` member of the raw value `size`, laid out first, last and
    /// spread over lines: in the third body the member follows blank CRLF lines, and its value
    /// starts a line of its own after every other member's line.
    fn page_size_layouts(object: &Value, size: &str) -> Result<[String; 3], Box<dyn StdError>> {
        let member = format!(r#""page_size":{size}"#);
        let pretty = serde_json::to_string_pretty(object)?;
        let members = pretty.strip_suffix("\n}").ok_or("not a JSON object")?;
        Ok([
            members_around(&member, object, "")?,
            members_around("", object, &member)?,
            format!("{members},\r\n\r\n\t  \"page_size\":\n{size}\n}}"),
        ])
    }

    /// The rejection `body` gets from a `ValidJson<T>` extractor.
    async fn json_rejection<T: DeserializeOwned + Validate>(
        body: &str,
    ) -> Result<ScreenError, Box<dyn StdError>> {
        let dto = std::any::type_name::<T>();
        ValidJson::<T>::from_request(json_request(body)?, &())
            .await
            .err()
            .ok_or_else(|| format!("{dto} accepted the body").into())
    }

    /// A top-level `page_size` holding a JSON number beyond the range `serde_json` represents is
    /// the 422 of every size outside the range, wherever the member stands and however the body
    /// is laid out: on the probe, whose user ID also breaks its rule, and on endpoint 25's body.
    #[tokio::test]
    async fn a_page_size_beyond_the_parsers_range_is_the_422() -> TestResult {
        let nines = "9".repeat(310);
        let sizes = [
            nines.clone(),
            format!("-{nines}"),
            "1e400".to_owned(),
            "-1e400".to_owned(),
            // 400 digits, which the rejection must not echo.
            CARD.repeat(25),
            // The exponent overflows, so the parser stops inside it.
            "1E99999999999999999999".to_owned(),
            format!("{nines}.5e3"),
            // An array is outside the range whatever it holds.
            format!("[7,{nines}]"),
        ];
        let probe = json!({"user_id": "USER00010"});
        let [.., list_action] = nested_object_bodies();
        // A failure names the case by its indexes: some bodies hold card numbers.
        for (size, value) in sizes.iter().enumerate() {
            for (layout, body) in page_size_layouts(&probe, value)?.iter().enumerate() {
                let error = json_rejection::<Probe>(body).await?;
                assert_eq!(
                    parts(&error),
                    (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        PAGE_SIZE_MESSAGE,
                        Some(PAGE_SIZE_FIELD)
                    ),
                    "probe, size {size}, layout {layout}"
                );
                assert!(
                    !serde_json::to_string(&error)?.contains("4111"),
                    "probe, size {size}, layout {layout}"
                );
            }
            for (layout, body) in page_size_layouts(&list_action, value)?.iter().enumerate() {
                let error = json_rejection::<TranTypeListActionRequest>(body).await?;
                assert_eq!(
                    parts(&error),
                    (
                        StatusCode::UNPROCESSABLE_ENTITY,
                        PAGE_SIZE_MESSAGE,
                        Some(PAGE_SIZE_FIELD)
                    ),
                    "endpoint 25, size {size}, layout {layout}"
                );
                assert!(
                    !serde_json::to_string(&error)?.contains("4111"),
                    "endpoint 25, size {size}, layout {layout}"
                );
            }
        }
        Ok(())
    }

    /// A body that does not deserialise for any other reason stays the 400, beside a
    /// `page_size` beyond the parser's range or in its place: a malformed number, a truncated
    /// body, text after the object, and a member that does not deserialise or holds such a
    /// number itself, before `page_size` or after it, at the top level or nested.
    #[tokio::test]
    async fn a_body_unreadable_beside_a_page_size_beyond_the_parsers_range_is_a_400() -> TestResult
    {
        let nines = "9".repeat(310);
        let huge = format!(r#""page_size":{nines}"#);
        let huge_page_no = format!(r#""page_no":{nines}"#);
        let probe = json!({"user_id": "USER0001"});
        let probe_last = members_around("", &probe, &huge)?;
        for body in [
            members_around("", &probe, r#""page_size":1.e5"#)?,
            members_around("", &probe, r#""page_size":01"#)?,
            members_around("", &probe, r#""page_size":-"#)?,
            probe_last.trim_end_matches('}').to_owned(),
            members_around(&huge, &probe, "")?
                .trim_end_matches('}')
                .to_owned(),
            format!("{probe_last} x"),
            members_around(r#""page_no":"x""#, &probe, &huge)?,
            members_around(&huge, &probe, r#""page_no":"x""#)?,
            members_around(&huge_page_no, &probe, &huge)?,
            members_around(&huge, &probe, &huge_page_no)?,
            members_around(&huge_page_no, &probe, "")?,
        ] {
            assert_unreadable::<Probe>(&body).await?;
        }

        let [.., list_action] = nested_object_bodies();
        let mut without_aid = list_action.clone();
        without_aid
            .as_object_mut()
            .ok_or("the endpoint 25 body is not an object")?
            .remove("aid");
        // `state.page_no`, nested, holds the number as well.
        let nested = |text: String| {
            let nested = text.replacen(r#""page_no":1"#, &huge_page_no, 1);
            (nested != text)
                .then_some(nested)
                .ok_or("no state.page_no to replace")
        };
        for body in [
            members_around(r#""aid":"PF7""#, &without_aid, &huge)?,
            members_around(&huge, &without_aid, r#""aid":"PF7""#)?,
            nested(members_around("", &list_action, &huge)?)?,
            nested(members_around(&huge, &list_action, "")?)?,
            members_around("", &list_action, r#""page_size":1.e5"#)?,
            members_around("", &list_action, &huge)?
                .trim_end_matches('}')
                .to_owned(),
        ] {
            assert_unreadable::<TranTypeListActionRequest>(&body).await?;
        }
        Ok(())
    }

    /// A DTO without a `page_size` member skips an unknown one unread, as it always has, whatever
    /// number it holds; a malformed number there is still unreadable.
    #[tokio::test]
    async fn an_unknown_page_size_beyond_the_parsers_range_is_skipped_as_before() -> TestResult {
        let nines = "9".repeat(310);
        let sign_on = json!({"user_id": "ADMIN001", "password": "PASSWORD"});
        for body in [
            members_around("", &sign_on, &format!(r#""page_size":{nines}"#))?,
            members_around(&format!(r#""page_size":-{nines}"#), &sign_on, "")?,
            members_around("", &sign_on, r#""page_size":1e400"#)?,
        ] {
            let ValidJson(request) =
                ValidJson::<SignonRequest>::from_request(json_request(&body)?, &())
                    .await
                    .map_err(|error| error.to_string())?;
            assert_eq!(request.user_id, "ADMIN001");
        }
        assert_unreadable::<SignonRequest>(&members_around("", &sign_on, r#""page_size":1.e5"#)?)
            .await
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

        // Out of range, beyond `i64` or no integer at all, whatever other rule fails: the 422,
        // which echoes nothing. A failure names the case by its index: two of the URIs hold a
        // card number.
        for (case, uri) in [
            "/users?page_size=101".to_owned(),
            "/users?page_size=0".to_owned(),
            "/users?page_size=-1".to_owned(),
            "/users?page_size=101&start_user_id=USER00010".to_owned(),
            format!("/users?page_size={CARD}{CARD}"),
            format!("/users?page_size={CARD}x"),
            "/users?page_size=7.5".to_owned(),
            "/users?page_size=".to_owned(),
            "/users?page_size=abc&start_user_id=USER00010".to_owned(),
        ]
        .into_iter()
        .enumerate()
        {
            let error = query(&uri)
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
                "case {case}"
            );
            assert!(
                !serde_json::to_string(&error)?.contains("4111"),
                "case {case}"
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

        // Another member that does not deserialise, beside a valid `page_size`.
        for uri in [
            format!("/users?page_size=7&page_no={CARD}x"),
            format!("/users?page_size=7&page_no={CARD}{CARD}"),
            "/users?page_size=7&page_no=7.5".to_owned(),
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

    /// Every `page_size` value as the two forms read it: an integer `i64` holds as itself, one
    /// beyond it saturated where the form holds it as an integer, and any other value as
    /// `NON_INTEGER_PAGE_SIZE`. None fails to deserialise, so the range rule judges each.
    #[test]
    fn page_size_reads_every_value_as_an_integer_for_the_range_rule() -> TestResult {
        let other = super::NON_INTEGER_PAGE_SIZE;
        assert!(!(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&other));
        // The query value as sent, percent-encoded, and the size it reads as.
        let queries = [
            ("7", Some(7)),
            ("%2B7", Some(7)),
            ("007", Some(7)),
            ("0101", Some(101)),
            ("0", Some(0)),
            ("-1", Some(-1)),
            ("9223372036854775807", Some(i64::MAX)),
            ("9223372036854775808", Some(i64::MAX)),
            ("99999999999999999999", Some(i64::MAX)),
            ("-9223372036854775808", Some(i64::MIN)),
            ("-9223372036854775809", Some(i64::MIN)),
            ("", Some(other)),
            ("abc", Some(other)),
            ("1.5", Some(other)),
            ("7.0", Some(other)),
            ("1e2", Some(other)),
            // A `+` in a query string is a space.
            ("+7", Some(other)),
            ("7%20", Some(other)),
            ("--1", Some(other)),
            ("%D9%A3", Some(other)),
        ];
        for (value, expected) in queries {
            let uri = format!("/users?page_size={value}").parse()?;
            let axum::extract::Query(probe) =
                axum::extract::Query::<QueryProbe>::try_from_uri(&uri)?;
            assert_eq!(probe.page_size, expected, "{value:?}");
        }
        let axum::extract::Query(absent) =
            axum::extract::Query::<QueryProbe>::try_from_uri(&"/users".parse()?)?;
        assert_eq!(absent.page_size, None);

        // The JSON value of the member, and the size it reads as.
        let bodies = [
            ("7", Some(7)),
            ("-1", Some(-1)),
            ("9223372036854775807", Some(i64::MAX)),
            ("-9223372036854775808", Some(i64::MIN)),
            ("9223372036854775808", Some(i64::MAX)),
            ("18446744073709551615", Some(i64::MAX)),
            ("null", None),
            ("99999999999999999999", Some(other)),
            ("-9223372036854775809", Some(other)),
            ("1.5", Some(other)),
            ("7.0", Some(other)),
            ("1e2", Some(other)),
            (r#""7""#, Some(other)),
            (r#""""#, Some(other)),
            ("true", Some(other)),
            ("false", Some(other)),
            ("[7]", Some(other)),
            (r#"{"page_size":7}"#, Some(other)),
        ];
        for (value, expected) in bodies {
            let probe = serde_json::from_str::<Probe>(&format!(r#"{{"page_size":{value}}}"#))?;
            assert_eq!(probe.page_size, expected, "{value}");
        }
        assert_eq!(serde_json::from_str::<Probe>("{}")?.page_size, None);
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
