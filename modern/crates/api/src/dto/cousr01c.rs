//! COUSR01C, user add: the request of endpoint 17, `POST /api/v1/users`, admin only (transaction
//! `CU01`, map `COUSR1A`; AAP 0.3.4.1).
//!
//! ENTER is the only key the endpoint serves, so [`UserAddRequest`] carries no `aid`; F3 and F4
//! send no request and belong to the screen (COUSR01C-007, COUSR01C-008). A stored user answers
//! 200 with a [`ScreenResult`](super::ScreenResult) whose message is the one
//! `domain::online::cousr01c::added_message` renders, such as `User NEWUSR01 has been added ...`
//! (COUSR01C-021). That message is a `String`, which `ScreenResult`'s `From<String>` in the
//! parent module takes unchanged, so this module declares no conversion of its own; a
//! `UserAddFailure` becomes a `ScreenError` through `api::error`.
//!
//! # Fields
//!
//! The five members are the raw text of the five map fields (COUSR01C-006): no trimming and no
//! case change, so leading spaces and lower-case letters reach the record as typed
//! (COUSR01C-016, COUSR01C-019, COUSR01C-020). `api::services::cousr01c` moves them out by value
//! and lends them to `domain::online::cousr01c::edit` as its `UserAddInput`, which pads each to
//! its `PIC X(n)` field and runs the program's empty-field edits in `EVALUATE` order. An absent
//! member is the empty text, so an empty field reaches the program's own message and never a
//! deserialisation error (COUSR01C-010 to COUSR01C-014).
//!
//! The rules declared here are each map field's `LENGTH`, the server half of the `both` rows of
//! `modern/audit/screens.md`: `first_name` 20 (COUSR01-013), `last_name` 20 (COUSR01-016),
//! `user_id` 8 (COUSR01-019), `password` 8 (COUSR01-022) and `user_type` 1 (COUSR01-025), and
//! the `PIC X(n)` field each is moved into, through the parent module's `pic_x`: at most that
//! many bytes, each printable ASCII (`0x20`–`0x7E`). A longer value, or one that counts few
//! enough characters but holds a byte outside printable ASCII, such as an accented letter or a
//! TAB, is the 400 the parent module's `ValidJson` gives, naming the member (X-COUSR01C-3).
//! `LOW-VALUES`, a value of the field's width in NUL bytes, which `pic_x` admits for the
//! programs that test it, passes on to `domain::online::cousr01c::edit`, which rejects it
//! naming the member, because an erased field arrives as an empty value here. `user_type` is
//! not narrowed to `A` and `U`: COUSR01C stores any non-blank character (COUSR01C-015).
//!
//! # Redaction
//!
//! `password` is the dark `PASSWD` field. It is write-only in the `OpenAPI` document, whose
//! example shows `********`; it is never serialised, and the hand-written `Debug` prints
//! `********` in its place. The rejection of an over-length password, like every rejection of
//! `ValidJson`, names the field and never the value (Cardholder Data Redaction, COUSR01C-018).
//!
//! # Identity
//!
//! `user_id` and `user_type` describe the user being added. They grant the caller nothing and
//! never change who acts: that is the verified token's `AdminUser` alone (AAP 0.5.1,
//! COUSR01C-026).

use core::fmt;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// What the `Debug` of a [`UserAddRequest`] prints in place of the password, and the password's
/// `OpenAPI` example.
const PASSWORD_REDACTED: &str = "********";

/// The body of endpoint 17: the five input fields of `COUSR1A`, each the raw text of its map
/// field (COUSR01C-006).
///
/// Every member is optional on the wire and empty when absent. `Debug` prints `********` in
/// place of the password, which is never serialised (Cardholder Data Redaction).
// The schema name `UserAddRequest` is the frontend's `components["schemas"]["UserAddRequest"]`
// (Generated API Types Only). `Debug` is written by hand so the password cannot reach a log.
#[derive(Clone, Serialize, Deserialize, ToSchema, Validate)]
pub struct UserAddRequest {
    /// `FNAME`, `PIC X(20)`: the first name, as typed, at most 20 bytes of printable ASCII
    /// (`0x20`–`0x7E`; COUSR01-013).
    #[serde(default)]
    #[validate(length(max = 20), custom(function = "super::pic_x::<20>"))]
    #[schema(max_length = 20, example = "ALEX")]
    pub first_name: String,

    /// `LNAME`, `PIC X(20)`: the last name, as typed, at most 20 bytes of printable ASCII
    /// (`0x20`–`0x7E`; COUSR01-016).
    #[serde(default)]
    #[validate(length(max = 20), custom(function = "super::pic_x::<20>"))]
    #[schema(max_length = 20, example = "MORGAN")]
    pub last_name: String,

    /// `USERID`, `PIC X(8)`: the ID of the user to add, as typed and compared byte for byte
    /// with the stored IDs, at most 8 bytes of printable ASCII (`0x20`–`0x7E`; COUSR01-019,
    /// COUSR01C-023).
    #[serde(default)]
    #[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]
    #[schema(max_length = 8, example = "NEWUSR01")]
    pub user_id: String,

    /// `PASSWD`, `PIC X(8)`, a dark field: the new user's password, stored as typed and hashed,
    /// at most 8 bytes of printable ASCII (`0x20`–`0x7E`; COUSR01-022, COUSR01C-018).
    /// Write-only: never sent back.
    // `skip_serializing_if` with a predicate that is always true, not `skip_serializing`:
    // utoipa drops a `skip_serializing` member from the schema, and the request body must
    // declare the password as a write-only property. Serialisation omits it all the same.
    #[serde(default, skip_serializing_if = "never_serialised")]
    #[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]
    #[schema(write_only, max_length = 8, example = "********")]
    pub password: String,

    /// `USRTYPE`, `PIC X(1)`: `A` for an administrator, any other non-blank character of
    /// printable ASCII (`0x20`–`0x7E`) for a regular user (COUSR01-025, COUSR01C-015).
    #[serde(default)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    #[schema(max_length = 1, example = "U")]
    pub user_type: String,
}

/// The `skip_serializing_if` predicate of [`UserAddRequest::password`]: true for every value,
/// so no serialisation of a request carries the password (Cardholder Data Redaction).
const fn never_serialised(_password: &str) -> bool {
    true
}

impl fmt::Debug for UserAddRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserAddRequest")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("user_id", &self.user_id)
            .field("password", &format_args!("{PASSWORD_REDACTED}"))
            .field("user_type", &self.user_type)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::body::Body;
    use axum::extract::{FromRequest, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use domain::online::cousr01c::{FailureClass, UserAddInput, edit};
    use serde_json::{Map, Value, json};
    use utoipa::{PartialSchema, ToSchema};
    use validator::Validate;

    use super::{PASSWORD_REDACTED, UserAddRequest};
    use crate::dto::{FIELD_LENGTH_MESSAGE, ValidJson};
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn Error>>;

    /// A password the tests submit, which nothing but the request itself may hold.
    const PASSWORD: &str = "s3cRetPw";

    /// Each member with its map `LENGTH`.
    const LENGTHS: [(&str, usize); 5] = [
        ("first_name", 20),
        ("last_name", 20),
        ("user_id", 8),
        ("password", 8),
        ("user_type", 1),
    ];

    /// `body` through the extractor the handler uses: the request, or its rejection.
    async fn extract(body: &Value) -> Result<Result<UserAddRequest, ScreenError>, Box<dyn Error>> {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/users")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(body)?))?;
        Ok(ValidJson::<UserAddRequest>::from_request(request, &())
            .await
            .map(|ValidJson(value)| value))
    }

    /// A request with every member filled, `password` included.
    fn full() -> UserAddRequest {
        UserAddRequest {
            first_name: "ALEX".to_owned(),
            last_name: "MORGAN".to_owned(),
            user_id: "NEWUSR01".to_owned(),
            password: PASSWORD.to_owned(),
            user_type: "U".to_owned(),
        }
    }

    /// The JSON object of the schema `UserAddRequest` derives.
    fn schema_json() -> Result<Value, serde_json::Error> {
        serde_json::to_value(UserAddRequest::schema())
    }

    // audit: COUSR01C-010
    // audit: COUSR01C-014
    #[tokio::test]
    async fn absent_members_are_empty_so_the_program_edits_answer() -> TestResult {
        let request = extract(&json!({}))
            .await?
            .map_err(|e| e.message().to_owned())?;
        for member in [
            &request.first_name,
            &request.last_name,
            &request.user_id,
            &request.password,
            &request.user_type,
        ] {
            assert!(member.is_empty());
        }
        let schema = schema_json()?;
        assert!(schema.get("required").is_none(), "{schema}");
        Ok(())
    }

    // audit: COUSR01C-016
    // audit: COUSR01C-019
    #[tokio::test]
    async fn values_reach_the_service_exactly_as_typed() -> TestResult {
        let body = json!({
            "first_name": "  alex",
            "last_name": "Morgan ",
            "user_id": "usr 0001",
            "password": "pA ss wd",
            "user_type": "a",
        });
        let request = extract(&body).await?.map_err(|e| e.message().to_owned())?;
        assert_eq!(request.first_name, "  alex");
        assert_eq!(request.last_name, "Morgan ");
        assert_eq!(request.user_id, "usr 0001");
        assert_eq!(request.password, "pA ss wd");
        assert_eq!(request.user_type, "a");
        Ok(())
    }

    // audit: COUSR01-013
    // audit: COUSR01-016
    // audit: COUSR01-019
    // audit: COUSR01-022
    // audit: COUSR01-025
    #[tokio::test]
    async fn each_member_admits_its_map_length_and_rejects_one_more() -> TestResult {
        for (member, length) in LENGTHS {
            let fits = json!({ member: "P".repeat(length) });
            assert!(extract(&fits).await?.is_ok(), "{member} at {length}");

            let mut body = Map::new();
            let long = format!("{PASSWORD}{}", "Q".repeat(length));
            body.insert(member.to_owned(), Value::String(long.clone()));
            let rejection = match extract(&Value::Object(body)).await? {
                Ok(_) => return Err(format!("{member} longer than {length} passed").into()),
                Err(rejection) => rejection,
            };
            assert_eq!(
                (rejection.status(), rejection.message(), rejection.field()),
                (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(member)),
            );
            assert!(!rejection.message().contains(&long));
        }
        Ok(())
    }

    // audit: COUSR01-013
    // audit: COUSR01-016
    // audit: COUSR01-019
    // audit: COUSR01-022
    // audit: COUSR01-025
    #[tokio::test]
    async fn each_member_admits_only_what_its_field_holds_byte_for_byte() -> TestResult {
        for (member, length) in LENGTHS {
            // As many characters as the map `LENGTH`, so within the `length` rule, yet bytes no
            // `PIC X(n)` field holds: a two-byte letter or a control byte in every position.
            for value in ["é".repeat(length), "\t".repeat(length)] {
                let rejection = match extract(&json!({ member: value })).await? {
                    Ok(_) => return Err(format!("{member} {value:?} passed").into()),
                    Err(rejection) => rejection,
                };
                assert_eq!(
                    (rejection.status(), rejection.message(), rejection.field()),
                    (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(member)),
                    "{member} {value:?}"
                );
                let wire = serde_json::to_string(&rejection)?;
                assert!(!wire.contains(&value), "{wire}");
            }

            // `LOW-VALUES` passes the request rules, and `edit` answers it as a value no
            // `PIC X(n)` field stores, naming the member (X-COUSR01C-3).
            let request = extract(&json!({ member: "\0".repeat(length) }))
                .await?
                .map_err(|e| e.message().to_owned())?;
            let input = UserAddInput {
                first_name: &request.first_name,
                last_name: &request.last_name,
                user_id: &request.user_id,
                password: &request.password,
                user_type: &request.user_type,
            };
            let failure = edit(&input).err().ok_or("LOW-VALUES passed the edits")?;
            assert_eq!(
                (failure.field().name(), failure.class()),
                (member, FailureClass::Input)
            );
        }
        Ok(())
    }

    // audit: COUSR01C-015
    #[test]
    fn any_one_character_user_type_passes_the_request_rules() {
        for user_type in ["A", "U", "a", "X", "9", " ", ""] {
            let request = UserAddRequest {
                user_type: user_type.to_owned(),
                ..full()
            };
            assert!(request.validate().is_ok(), "{user_type:?}");
        }
    }

    // audit: COUSR01C-018
    #[test]
    fn debug_and_serialisation_never_show_the_password() -> TestResult {
        let request = full();

        let debug = format!("{request:?}");
        assert!(!debug.contains(PASSWORD), "{debug}");
        assert!(debug.contains(PASSWORD_REDACTED), "{debug}");
        assert!(debug.contains("NEWUSR01"), "{debug}");

        let serialised = serde_json::to_value(&request)?;
        assert!(serialised.get("password").is_none(), "{serialised}");
        assert_eq!(serialised.get("user_id"), Some(&json!("NEWUSR01")));
        assert!(!serialised.to_string().contains(PASSWORD));
        Ok(())
    }

    // audit: COUSR01C-018
    #[test]
    fn the_schema_is_named_and_its_password_is_write_only() -> TestResult {
        assert_eq!(UserAddRequest::name(), "UserAddRequest");
        let schema = schema_json()?;
        let properties = schema
            .get("properties")
            .and_then(Value::as_object)
            .ok_or("no properties")?;
        let mut names: Vec<&str> = properties.keys().map(String::as_str).collect();
        names.sort_unstable();
        let mut expected: Vec<&str> = LENGTHS.iter().map(|&(member, _)| member).collect();
        expected.sort_unstable();
        assert_eq!(names, expected);

        let password = properties.get("password").ok_or("no password")?;
        assert_eq!(password.get("writeOnly"), Some(&json!(true)), "{password}");
        assert!(
            password.to_string().contains(PASSWORD_REDACTED),
            "{password}"
        );
        for (member, length) in LENGTHS {
            let property = properties.get(member).ok_or(member)?;
            assert_eq!(property.get("maxLength"), Some(&json!(length)), "{member}");
        }
        Ok(())
    }
}
