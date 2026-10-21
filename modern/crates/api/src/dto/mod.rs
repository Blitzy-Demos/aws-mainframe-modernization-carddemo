//! The request and response contracts of the 27 REST operations (AAP 0.3.4.1), and what all of
//! them share: the [`ScreenResult`] and [`PageDirection`] schemas, the `page_size` rule of
//! Bounded Pagination, and the validating extractors [`ValidJson`] and [`ValidQuery`].
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
//!   and `NUM` attributes, the server half of the `both` rows of `modern/audit/screens.md`) and
//!   `From` conversions from domain outcomes. It holds no business rule, reads no store and
//!   names neither `persistence` nor `sqlx`: every program edit and message stays in
//!   `domain::online` (Thin Request Handlers).
//! - **A DTO field keeps its Rust name on the wire.** `validator` reports a failing field under
//!   its Rust name, so no DTO field is renamed through `serde`; the `field` of a rejection is
//!   then the name the client sent.
//!
//! # Extractors
//!
//! A handler takes a body as [`ValidJson<T>`] and a query string as [`ValidQuery<T>`]. utoipa
//! cannot see through either, so a handler taking `ValidJson<T>` declares `request_body = T` and
//! one taking `ValidQuery<T>` declares `params(T)` in its `#[utoipa::path]`.
//!
//! Every rejection is a [`ScreenError`]:
//!
//! - A body or query string that does not deserialise into `T`: 400 with [`INVALID_BODY`] or
//!   [`INVALID_QUERY`] and no field. That covers malformed JSON, a missing JSON content type, a
//!   value of the wrong type (text where an integer belongs, for instance) and an unknown enum
//!   value.
//! - A rule a DTO declares: 400 naming the first failing field in key order, as a path such as
//!   `user_id`, `fields.zip` or `rows[3].description`, with the text of the rule's code
//!   ([`FIELD_LENGTH_MESSAGE`], [`FIELD_NUMERIC_MESSAGE`] or [`FIELD_RANGE_MESSAGE`]).
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
use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use chrono::{NaiveDate, NaiveDateTime};
use domain::copybook::{PicDate, PicTimestamp, PicU, PicX};
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

/// The message of a field longer than its map `LENGTH`, and of every rule code without a text
/// of its own: input the 3270 field would not have admitted.
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
/// [`FIELD_RANGE_MESSAGE`] for `range`, and [`FIELD_LENGTH_MESSAGE`] for `length` and every
/// other code.
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

/// A `PIC X(n)` field as shown: its bytes without the trailing spaces, leading spaces kept,
/// converted lossily (the stored bytes are printable ASCII).
#[must_use]
pub(crate) fn text<const N: usize>(field: &PicX<N>) -> String {
    field.to_text()
}

// The `Copy` PIC types below are taken by value and `PicX`, which is not `Copy`, by reference:
// the workspace denies clippy's `trivially_copy_pass_by_ref`, which a reference to the
// eight-byte `PicU` or the four-byte `PicDate` trips, and lint suppressions are not allowed.

/// A `PIC 9(n)` field as shown: its `N` digits with the leading zeros, the text a COBOL `MOVE`
/// of the field to `X(n)` gives, from the field's own encoder.
#[must_use]
pub(crate) fn digits<const N: u32>(field: PicU<N>) -> String {
    let mut out = Vec::new();
    field.encode_into(&mut out);
    String::from_utf8_lossy(&out).into_owned()
}

/// A `PIC X(10)` date field as its typed date, `None` for a blank field (Typed Dates and
/// Timestamps).
#[must_use]
pub(crate) const fn date(field: PicDate) -> Option<NaiveDate> {
    field.value()
}

/// A `PIC X(26)` timestamp field as its typed value, `None` for a blank field (Typed Dates and
/// Timestamps). The layout the COBOL wrote it in is a storage concern and is not sent.
#[must_use]
pub(crate) fn timestamp(field: PicTimestamp) -> Option<NaiveDateTime> {
    field.value().map(|stamp| stamp.value())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::Body;
    use axum::extract::{FromRequest, FromRequestParts, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use chrono::{NaiveDate, NaiveDateTime};
    use domain::copybook::{CobolTimestamp, PicDate, PicTimestamp, PicU, PicX, TsForm};
    use domain::online::{cocrdlic, copaus0c, cotrn00c, cotrtlic, cousr00c};
    use serde::Deserialize;
    use utoipa::ToSchema;
    use validator::Validate;

    use super::{
        FIELD_LENGTH_MESSAGE, FIELD_NUMERIC_MESSAGE, FIELD_RANGE_MESSAGE, INVALID_BODY,
        INVALID_QUERY, MAX_PAGE_SIZE, MIN_PAGE_SIZE, PAGE_SIZE_FIELD, PAGE_SIZE_MESSAGE,
        PageDirection, ScreenResult, ValidJson, ValidQuery, date, digits, numeric,
        resolve_page_size, text, timestamp, validation_error,
    };
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

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
        #[validate(length(max = 8))]
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
    fn pic_fields_render_as_the_screen_shows_them() -> TestResult {
        assert_eq!(text(&PicX::<8>::from_text(" AB C")?), " AB C");
        assert_eq!(text(&PicX::<4>::spaces()), "");
        assert_eq!(digits(PicU::<11>::new(50)?), "00000000050");
        assert_eq!(digits(PicU::<9>::ZERO), "000000000");

        let day = NaiveDate::from_ymd_opt(2022, 7, 18).ok_or("invalid date")?;
        assert_eq!(date(PicDate::new(Some(day))?), Some(day));
        assert_eq!(date(PicDate::BLANK), None);

        let at: NaiveDateTime = day
            .and_hms_micro_opt(13, 5, 9, 120_000)
            .ok_or("invalid time")?;
        for form in [TsForm::Iso, TsForm::Db2] {
            let stamp = PicTimestamp::new(Some(CobolTimestamp::new(at, form)?));
            assert_eq!(timestamp(stamp), Some(at));
        }
        assert_eq!(timestamp(PicTimestamp::BLANK), None);
        Ok(())
    }
}
