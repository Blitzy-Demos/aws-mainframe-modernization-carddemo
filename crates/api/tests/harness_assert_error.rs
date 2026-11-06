//! The contract of `common::assert_error`, the error-body check every HTTP
//! error test of this crate relies on.
//!
//! The helper must pass a response only when its status, `code`, untrimmed
//! `message` and ordered `(field, message)` entries all equal the expectation,
//! and must panic, naming the part that differs and showing the raw body, on
//! anything else. A helper that passed a wrong code or a wrong field text would
//! let every caller miss a malformed envelope, so this file pins both sides.
//!
//! It needs no container, database or runtime: the responses are built in
//! memory as `(StatusCode, JSON value, raw text)`, the shape `common::send`
//! returns. The texts are `carddemo_domain::messages` constants used as
//! arbitrary strings, and the codes come from `ErrorKind::code`.

pub mod common;

use std::panic::{self, AssertUnwindSafe};

use carddemo_domain::error::ErrorKind;
use carddemo_domain::messages;
use common::{Resp, assert_error};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// The status every case expects.
const STATUS: StatusCode = StatusCode::BAD_REQUEST;

/// The `code` every case expects.
const CODE: &str = ErrorKind::Empty.code();

/// The top-level `message` every case expects.
const MESSAGE: &str = messages::AUTHENTICATION_REQUIRED;

/// The `(field, message)` entries every case expects, in body order.
const FIELDS: [(&str, &str); 2] = [
    ("USERID", messages::AUTHENTICATION_REQUIRED),
    ("PASSWD", messages::ADMIN_ACCESS_REQUIRED),
];

/// One response `assert_error` must reject against the expectation above.
struct Rejected {
    /// What the case changes, for the failure text.
    case: &'static str,
    /// The response handed to `assert_error`.
    resp: Resp,
    /// Text the panic must contain, naming the part that differs.
    names: &'static str,
}

/// A response with `status` and the JSON `body`, whose raw text is the body's
/// serialization.
fn resp(status: StatusCode, body: Value) -> Resp {
    let raw = body.to_string();
    (status, body, raw)
}

/// The `fields` entry `{field, message}`.
fn entry(field: &str, message: &str) -> Value {
    json!({ "field": field, "message": message })
}

/// The `fields` array of the expected entries.
fn expected_entries() -> Value {
    Value::Array(
        FIELDS
            .iter()
            .map(|(field, message)| entry(field, message))
            .collect(),
    )
}

/// The expected `code` and `message` with `fields` as given.
fn body_with_fields(fields: Value) -> Value {
    let mut body = json!({ "code": CODE, "message": MESSAGE });
    body["fields"] = fields;
    body
}

/// `assert_error` against the expectation every case shares.
fn assert_expected(resp: &Resp) {
    assert_error(resp, STATUS, CODE, MESSAGE, &FIELDS);
}

#[test]
fn an_exact_body_passes_and_absent_null_or_empty_fields_equal_no_fields() {
    assert_expected(&resp(STATUS, body_with_fields(expected_entries())));

    let without_fields = [
        json!({ "code": CODE, "message": MESSAGE }),
        body_with_fields(Value::Null),
        body_with_fields(json!([])),
    ];
    for body in without_fields {
        assert_error(&resp(STATUS, body), STATUS, CODE, MESSAGE, &[]);
    }
}

/// The responses `assert_error` must reject against the shared expectation.
fn rejected_cases() -> Vec<Rejected> {
    let [(first_field, first_text), (second_field, second_text)] = FIELDS;
    let first = entry(first_field, first_text);
    let second = entry(second_field, second_text);
    let exact = body_with_fields(expected_entries());
    let replaced = |key: &str, value: Value| {
        let mut body = exact.clone();
        body[key] = value;
        body
    };
    let pairs = "`fields` of the error body, as (field, message) pairs";
    vec![
        Rejected {
            case: "a different status",
            resp: resp(StatusCode::NOT_FOUND, exact.clone()),
            names: "status of the error answer",
        },
        Rejected {
            case: "a wrong but non-empty code",
            resp: resp(STATUS, replaced("code", json!(ErrorKind::Invalid.code()))),
            names: "`code` of the error body",
        },
        Rejected {
            case: "no code",
            resp: resp(
                STATUS,
                json!({ "message": MESSAGE, "fields": expected_entries() }),
            ),
            names: "`code` of the error body",
        },
        Rejected {
            case: "a message one trailing space longer",
            resp: resp(
                STATUS,
                replaced("message", Value::String([MESSAGE, " "].concat())),
            ),
            names: "`message` of the error body",
        },
        Rejected {
            case: "an entry without a message",
            resp: resp(
                STATUS,
                body_with_fields(json!([{ "field": first_field }, second])),
            ),
            names: "`fields` entry 0 of the error body has no string `message`",
        },
        Rejected {
            case: "an entry whose field is not a string",
            resp: resp(
                STATUS,
                body_with_fields(json!([{ "field": 7, "message": first_text }, second])),
            ),
            names: "`fields` entry 0 of the error body has no string `field`",
        },
        Rejected {
            case: "an entry with a wrong message",
            resp: resp(
                STATUS,
                body_with_fields(json!([entry(first_field, second_text), second])),
            ),
            names: pairs,
        },
        Rejected {
            case: "the entries in swapped order",
            resp: resp(STATUS, body_with_fields(json!([second, first]))),
            names: pairs,
        },
        Rejected {
            case: "an extra entry",
            resp: resp(STATUS, body_with_fields(json!([first, second, first]))),
            names: pairs,
        },
        Rejected {
            case: "a missing entry",
            resp: resp(STATUS, body_with_fields(json!([first]))),
            names: pairs,
        },
        Rejected {
            case: "fields that are not an array",
            resp: resp(STATUS, body_with_fields(first.clone())),
            names: "`fields` of the error body is not an array",
        },
        Rejected {
            case: "an entry that is not an object",
            resp: resp(STATUS, body_with_fields(json!([first, second_field]))),
            names: "`fields` entry 1 of the error body is not an object",
        },
    ]
}

#[test]
fn a_differing_or_malformed_body_panics_naming_the_part() {
    for Rejected {
        case,
        resp: response,
        names,
    } in rejected_cases()
    {
        let raw = &response.2;
        let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| assert_expected(&response)))
        else {
            panic!("assert_error accepted {case}: {raw}");
        };
        let text = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_else(|| panic!("the panic for {case} carries no text"));
        assert!(
            text.contains(names),
            "the panic for {case} must name {names:?}: {text}"
        );
        assert!(
            text.contains(raw.as_str()),
            "the panic for {case} must show the raw body {raw}: {text}"
        );
    }
}
