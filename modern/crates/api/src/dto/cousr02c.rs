//! The contracts of endpoints 18 and 19, both admin-only (`[A]`, AAP 0.3.4.1):
//!
//! - Endpoint 18, `GET /api/v1/users/{user_id}`, the read COUSR02C (`CU02`) and COUSR03C
//!   (`CU03`) share, because their read paragraphs carry identical failure literals (`User ID
//!   NOT found...`, `Unable to lookup User...`; AAP 0.2.3). It answers with a [`UserView`].
//! - Endpoint 19, `PUT /api/v1/users/{user_id}`, COUSR02C's PF5 update, takes a
//!   [`UserUpdateRequest`] and answers with [`super::ScreenResult`], built by
//!   `ScreenResult::from` over the message `rules::rewrite_outcome` returns
//!   (`User USER0001 has been updated ...`, COUSR02C-029).
//!
//! The path `user_id` is a raw `String` the handler passes to the service, which edits it with
//! `rules::edit_user_id`; its map `LENGTH` (`USRIDIN`, 8, COUSR02-013) is the domain's
//! alphanumeric `MOVE`, so no DTO here carries it. F3, F4 and F12 are client-side. A failure of
//! either endpoint is a `rules::UserMaintFailure`, which `crate::error` maps to a
//! `ScreenError`.
//!
//! # Prompts
//!
//! The two programs' reads differ only in the prompt they send on success:
//! `Press PF5 key to save your updates ...` in COUSR02C and `Press PF5 key to delete this user
//! ...` in COUSR03C (COUSR02C-014, COUSR03C-012). One shared response cannot carry one message
//! for both, so [`UserView`] carries both, each verbatim from its own program's domain constant,
//! under its own key: the update screen shows `update_prompt` and the delete screen
//! `delete_prompt`. The server owns every message and the client composes neither text (AAP
//! 0.3.5); neither is the `message` of a [`super::ScreenResult`].
//!
//! # Password (`E27`)
//!
//! COUSR02C moves the stored password into its dark field (`COUSR02C.cbl:169`) and compares the
//! field with it to detect a change (`COUSR02C.cbl:227`). The replacement never returns it:
//! [`UserView`] has no password property, so the screen never pre-fills one (COUSR02C-013). On
//! the update the password is optional and write-only: omitted or `null` means unchanged
//! (COUSR02C-021), and a supplied value is verified against the stored hash for the change test
//! and re-hashed when it differs, both in the service (COUSR02C-027). No value of this module
//! serialises or debug-prints a password (Cardholder Data Redaction).
//!
//! # Identity
//!
//! The path `user_id` and the body name the target user. The body's `user_type` is the value to
//! store for that user and never alters the caller's authority, which comes from the verified
//! token alone (AAP 0.5.1).

use core::fmt;

use domain::online::cousr02c as rules;
use domain::online::cousr03c;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use super::text;

/// What `Debug` shows in place of a supplied password.
const PASSWORD_SHOWN: &str = "********";

/// The answer of endpoint 18 for a user that has a row: the row's ID, names and type, and the
/// prompt each of the two sharing programs sends after its read (COUSR02C-012, COUSR02C-014,
/// COUSR03C-011, COUSR03C-012).
///
/// Each stored value is shown without its trailing spaces, the trimmed view of `PicX`, as the
/// map field displays it. No password is carried (`E27`).
// The schema name `UserView` is the frontend's `components["schemas"]["UserView"]` (Generated
// API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "user_id": "USER0001",
    "first_name": "LAWRENCE",
    "last_name": "THOMAS",
    "user_type": "U",
    "update_prompt": "Press PF5 key to save your updates ...",
    "delete_prompt": "Press PF5 key to delete this user ..."
}))]
pub struct UserView {
    /// `SEC-USR-ID PIC X(08)`, the key the user was read by.
    pub user_id: String,
    /// `SEC-USR-FNAME PIC X(20)`.
    pub first_name: String,
    /// `SEC-USR-LNAME PIC X(20)`.
    pub last_name: String,
    /// `SEC-USR-TYPE PIC X(01)`: `A` for an admin, `U` for a user, or any other character
    /// COUSR01C or COUSR02C stored as typed.
    pub user_type: String,
    /// COUSR02C's read prompt, `Press PF5 key to save your updates ...`, which the update screen
    /// shows in the message row (COUSR02C-014).
    pub update_prompt: String,
    /// COUSR03C's read prompt, `Press PF5 key to delete this user ...`, which the delete screen
    /// shows in the message row (COUSR03C-012).
    pub delete_prompt: String,
}

/// The domain's view of the found row, with both programs' prompts from their constants.
impl From<rules::UserView> for UserView {
    fn from(view: rules::UserView) -> Self {
        Self {
            user_id: text(&view.user_id),
            first_name: text(&view.first_name),
            last_name: text(&view.last_name),
            user_type: text(&view.user_type),
            update_prompt: rules::UPDATE_PROMPT.to_owned(),
            delete_prompt: cousr03c::DELETE_PROMPT.to_owned(),
        }
    }
}

/// The body of endpoint 19: the four input fields of the update map, each the raw text of its
/// map field (`FNAME`, `LNAME`, `PASSWD`, `USRTYPE`).
///
/// Each field is validated against its map `LENGTH`, the server half of its `both` row of
/// `modern/audit/screens.md`: `first_name` and `last_name` 20 (COUSR02-017, COUSR02-020),
/// `password` 8 (COUSR02-023) and `user_type` 1 (COUSR02-026), and against the `PIC X(n)`
/// field it is stored in: at most that many bytes, each printable ASCII (`0x20`–`0x7E`), or
/// `LOW-VALUES`, the field's width in NUL bytes. A value that fails either is a 400 naming the
/// field, before any program edit, so a password is stored with exactly the bytes sent.
///
/// A name or type that is absent is the empty text, and an empty, all-space or `LOW-VALUES`
/// value passes both rules, so the program's own blank-field edits answer it with their
/// messages, in its order (COUSR02C-019 to COUSR02C-022); no value edit applies to the type
/// (COUSR02C-023).
///
/// The password is optional and write-only: omitted or `null`, the stored hash is kept (`E27`).
/// It never leaves through `Serialize`, so a client that sends one writes the JSON itself, and
/// `Debug` shows a supplied one as `********`.
// The byte rule is the parent module's `pic_x` beside each `length`, whose rejection text is
// `FIELD_LENGTH_MESSAGE`. `length` counts characters, and the domain reads each value as an
// alphanumeric `MOVE` that cuts on the right, so without `pic_x` a password such as `ééééAAAA`,
// eight characters in twelve bytes, would be cut to its first eight bytes before hashing.
// The schema name `UserUpdateRequest` is the frontend's
// `components["schemas"]["UserUpdateRequest"]` (Generated API Types Only).
#[derive(Clone, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({
    "first_name": "LAWRENCE",
    "last_name": "THOMAS",
    "password": "********",
    "user_type": "U"
}))]
pub struct UserUpdateRequest {
    /// `FNAME`, `PIC X(20)`: at most 20 bytes of printable ASCII (`0x20`–`0x7E`).
    #[serde(default)]
    #[validate(length(max = 20), custom(function = "super::pic_x::<20>"))]
    #[schema(max_length = 20)]
    pub first_name: String,
    /// `LNAME`, `PIC X(20)`: at most 20 bytes of printable ASCII (`0x20`–`0x7E`).
    #[serde(default)]
    #[validate(length(max = 20), custom(function = "super::pic_x::<20>"))]
    #[schema(max_length = 20)]
    pub last_name: String,
    /// `PASSWD`, `PIC X(8)`, dark on the map: the new password, at most 8 bytes of printable
    /// ASCII (`0x20`–`0x7E`), or absent to keep the stored one. Never returned by any endpoint.
    // `skip_serializing_if` with an always-true predicate, not `skip_serializing`: utoipa 5.5
    // reads `skip_serializing` as `skip` and drops the property from the OpenAPI document,
    // which would take the write-only `password` out of the generated client type.
    #[serde(default, skip_serializing_if = "never_serialized")]
    #[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]
    #[schema(write_only, max_length = 8, example = "********")]
    pub password: Option<String>,
    /// `USRTYPE`, `PIC X(1)`: the type to store for the target user, one byte of printable
    /// ASCII (`0x20`–`0x7E`).
    #[serde(default)]
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    #[schema(max_length = 1)]
    pub user_type: String,
}

impl UserUpdateRequest {
    /// The request as the domain's update input, borrowing every value: the input
    /// `rules::edit_update` edits.
    #[must_use]
    pub fn as_input(&self) -> rules::UserUpdateInput<'_> {
        rules::UserUpdateInput {
            first_name: &self.first_name,
            last_name: &self.last_name,
            password: self.password.as_deref(),
            user_type: &self.user_type,
        }
    }
}

/// Shows a supplied password as `********` and an omitted one as `None`.
impl fmt::Debug for UserUpdateRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserUpdateRequest")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("password", &self.password.as_ref().map(|_| PASSWORD_SHOWN))
            .field("user_type", &self.user_type)
            .finish()
    }
}

/// The `skip_serializing_if` predicate of [`UserUpdateRequest::password`]: always true, so a
/// password is never serialised.
fn never_serialized<T>(_: &T) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::Body;
    use axum::extract::{FromRequest, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use domain::copybook::PicX;
    use domain::online::cosgn00c;
    use domain::password;
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};
    use validator::Validate;

    use super::{UserUpdateRequest, UserView, rules};
    use crate::dto::{FIELD_LENGTH_MESSAGE, ScreenResult, ValidJson};
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The members a case blanks, each with the width of its `PIC X(n)` field.
    type Blanks = &'static [(&'static str, usize)];

    // The audited literals, written out apart from the domain constants.
    const UPDATE_PROMPT: &str = "Press PF5 key to save your updates ...";
    const DELETE_PROMPT: &str = "Press PF5 key to delete this user ...";

    /// A synthetic password the tests submit; no output may contain it.
    const SECRET: &str = "NEWPW123";

    fn field<const N: usize>(value: &str) -> PicX<N> {
        PicX::moved_from(value.as_bytes())
    }

    /// `USER0001` of the in-stream records as the domain's read returns it.
    fn user0001() -> rules::UserView {
        rules::UserView {
            user_id: field("USER0001"),
            first_name: field("LAWRENCE"),
            last_name: field("THOMAS"),
            user_type: field("U"),
        }
    }

    fn request(password: Option<&str>) -> UserUpdateRequest {
        UserUpdateRequest {
            first_name: "LAWRENCE".to_owned(),
            last_name: "THOMAS".to_owned(),
            password: password.map(str::to_owned),
            user_type: "U".to_owned(),
        }
    }

    /// The JSON keys of `value`, sorted.
    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .map(|object| object.keys().map(String::as_str).collect())
            .unwrap_or_default();
        keys.sort_unstable();
        keys
    }

    /// The `OpenAPI` schema of `T` as JSON.
    fn schema_of<T: PartialSchema>() -> Result<Value, serde_json::Error> {
        serde_json::to_value(T::schema())
    }

    fn json_request(body: &str) -> Result<Request, axum::http::Error> {
        Request::builder()
            .method("PUT")
            .uri("/api/v1/users/USER0001")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
    }

    /// `body` through the extractor endpoint 19's handler takes: the request, or its rejection.
    async fn extract(
        body: &Value,
    ) -> Result<Result<UserUpdateRequest, ScreenError>, Box<dyn StdError>> {
        Ok(
            ValidJson::<UserUpdateRequest>::from_request(json_request(&body.to_string())?, &())
                .await
                .map(|ValidJson(request)| request),
        )
    }

    /// A body every rule and every edit admits.
    fn valid_body() -> Value {
        json!({
            "first_name": "LAWRENCE",
            "last_name": "THOMAS",
            "password": SECRET,
            "user_type": "U",
        })
    }

    /// [`valid_body`] with `member` set to `value`.
    fn body_with(member: &str, value: Value) -> Value {
        let mut body = valid_body();
        body[member] = value;
        body
    }

    #[test]
    fn the_view_shows_the_row_trimmed_with_both_programs_prompts() {
        // COUSR02C-012, COUSR02C-014, COUSR03C-012.
        let view = UserView::from(user0001());
        assert_eq!(
            view,
            UserView {
                user_id: "USER0001".to_owned(),
                first_name: "LAWRENCE".to_owned(),
                last_name: "THOMAS".to_owned(),
                user_type: "U".to_owned(),
                update_prompt: UPDATE_PROMPT.to_owned(),
                delete_prompt: DELETE_PROMPT.to_owned(),
            }
        );
        assert_eq!(view.update_prompt, rules::UPDATE_PROMPT);
        assert_eq!(view.delete_prompt, super::cousr03c::DELETE_PROMPT);

        // Trailing spaces go, leading spaces and case stay, as the map field shows them.
        let padded = rules::UserView {
            user_id: field("AB CD"),
            first_name: field(" lawrence"),
            last_name: field(""),
            user_type: field("a"),
        };
        let view = UserView::from(padded);
        assert_eq!(
            (
                view.user_id.as_str(),
                view.first_name.as_str(),
                view.last_name.as_str(),
                view.user_type.as_str()
            ),
            ("AB CD", " lawrence", "", "a")
        );
    }

    #[test]
    fn the_view_has_no_password_on_the_wire_or_in_its_schema() -> TestResult {
        // COUSR02C-012, COUSR02C-013 (E27).
        let wire = serde_json::to_value(UserView::from(user0001()))?;
        assert_eq!(
            keys(&wire),
            [
                "delete_prompt",
                "first_name",
                "last_name",
                "update_prompt",
                "user_id",
                "user_type"
            ]
        );
        let schema = schema_of::<UserView>()?;
        assert_eq!(
            keys(&schema["properties"]),
            [
                "delete_prompt",
                "first_name",
                "last_name",
                "update_prompt",
                "user_id",
                "user_type"
            ]
        );
        assert_eq!(keys(&schema["example"]), keys(&wire));
        assert_eq!(UserView::name(), "UserView");
        Ok(())
    }

    #[test]
    fn the_request_password_is_optional_and_write_only_in_the_schema() -> TestResult {
        let schema = schema_of::<UserUpdateRequest>()?;
        let password = &schema["properties"]["password"];
        assert_eq!(password["writeOnly"], json!(true), "{schema}");
        assert_eq!(password["example"], json!("********"), "{schema}");
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(!required.contains(&json!("password")), "{schema}");
        assert_eq!(
            keys(&schema["properties"]),
            ["first_name", "last_name", "password", "user_type"]
        );
        // Each property publishes the map `LENGTH` its rules enforce (COUSR02-017, COUSR02-020,
        // COUSR02-023, COUSR02-026), and no pattern.
        for (member, length) in [
            ("first_name", 20),
            ("last_name", 20),
            ("password", 8),
            ("user_type", 1),
        ] {
            let property = &schema["properties"][member];
            assert_eq!(property["maxLength"], json!(length), "{member}: {schema}");
            assert!(property.get("pattern").is_none(), "{member}: {schema}");
        }
        assert_eq!(UserUpdateRequest::name(), "UserUpdateRequest");
        Ok(())
    }

    #[test]
    fn absent_fields_are_empty_and_an_omitted_or_null_password_is_none() -> TestResult {
        // COUSR02C-021 (E27).
        let empty: UserUpdateRequest = serde_json::from_str("{}")?;
        assert_eq!(
            (
                empty.first_name.as_str(),
                empty.last_name.as_str(),
                empty.password.as_deref(),
                empty.user_type.as_str()
            ),
            ("", "", None, "")
        );
        let null: UserUpdateRequest = serde_json::from_str(r#"{"password":null}"#)?;
        assert_eq!(null.password, None);
        let supplied: UserUpdateRequest = serde_json::from_str(&format!(
            r#"{{"first_name":"Ann","last_name":"Lee","password":"{SECRET}","user_type":"A"}}"#
        ))?;
        assert_eq!(supplied.password.as_deref(), Some(SECRET));
        assert_eq!(supplied.user_type, "A");
        Ok(())
    }

    #[test]
    fn a_password_never_leaves_through_serialize_or_debug() -> TestResult {
        let supplied = request(Some(SECRET));
        let wire = serde_json::to_value(&supplied)?;
        assert_eq!(keys(&wire), ["first_name", "last_name", "user_type"]);
        assert!(!wire.to_string().contains(SECRET));

        let shown = format!("{supplied:?}");
        assert!(shown.contains(r#"password: Some("********")"#), "{shown}");
        assert!(!shown.contains(SECRET), "{shown}");
        assert!(format!("{:?}", request(None)).contains("password: None"));
        Ok(())
    }

    #[test]
    fn every_field_admits_its_map_length() {
        let at_limit = UserUpdateRequest {
            first_name: "F".repeat(20),
            last_name: "L".repeat(20),
            password: Some("P".repeat(8)),
            user_type: "U".to_owned(),
        };
        assert!(at_limit.validate().is_ok());
        assert!(request(None).validate().is_ok());
        assert!(
            UserUpdateRequest {
                first_name: String::new(),
                last_name: String::new(),
                password: Some(String::new()),
                user_type: String::new(),
            }
            .validate()
            .is_ok(),
            "blank values are the domain's edits, not the DTO's"
        );
    }

    #[tokio::test]
    async fn a_field_beyond_its_map_length_is_a_400_naming_it() -> TestResult {
        // COUSR02-017, COUSR02-020, COUSR02-023, COUSR02-026.
        let cases = [
            (json!({ "first_name": "F".repeat(21) }), "first_name"),
            (json!({ "last_name": "L".repeat(21) }), "last_name"),
            (json!({ "password": format!("{SECRET}9") }), "password"),
            (json!({ "user_type": "UA" }), "user_type"),
        ];
        for (body, name) in cases {
            let error =
                ValidJson::<UserUpdateRequest>::from_request(json_request(&body.to_string())?, &())
                    .await
                    .err()
                    .ok_or("an over-long field was accepted")?;
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(name)),
                "{name}"
            );
            assert!(!serde_json::to_string(&error)?.contains(SECRET));
        }
        Ok(())
    }

    // audit: COUSR02-017
    // audit: COUSR02-020
    // audit: COUSR02-023
    // audit: COUSR02-026
    #[tokio::test]
    async fn a_value_its_pic_x_field_cannot_hold_is_a_400_naming_it() -> TestResult {
        let wide_name = "é".repeat(20);
        let short_nul = "\0".repeat(3);
        let cases = [
            // Eight characters, within `length`; twelve bytes, beyond `PIC X(8)`.
            ("password", "ééééAAAA"),
            ("password", "NEWPW12\u{7f}"),
            ("password", short_nul.as_str()),
            ("first_name", wide_name.as_str()),
            ("first_name", "LAW\tRENCE"),
            ("last_name", wide_name.as_str()),
            ("last_name", "THO\0MAS"),
            ("user_type", "é"),
            ("user_type", "\n"),
        ];
        for (member, value) in cases {
            let error = extract(&body_with(member, json!(value)))
                .await?
                .err()
                .ok_or("a value its field cannot hold was accepted")?;
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(member)),
                "{member} {value:?}"
            );
            let wire = serde_json::to_string(&error)?;
            let shown = format!("{error:?}");
            for output in [&wire, &shown] {
                assert!(!output.contains(value), "{output}");
                assert!(!output.contains(SECRET), "{output}");
            }
        }
        Ok(())
    }

    // audit: COUSR02C-019
    // audit: COUSR02C-020
    // audit: COUSR02C-021
    // audit: COUSR02C-022
    #[tokio::test]
    async fn blank_and_low_values_reach_the_programs_blank_edits_in_its_order() -> TestResult {
        let fills: [fn(usize) -> String; 3] = [
            |_| String::new(),
            |width| " ".repeat(width),
            |width| "\0".repeat(width),
        ];
        // Each case blanks the members it lists, at their full widths, in a valid body; the
        // first blank one in the program's order is the answer.
        let cases: [(Blanks, &str, &str); 4] = [
            (
                &[
                    ("first_name", 20),
                    ("last_name", 20),
                    ("password", 8),
                    ("user_type", 1),
                ],
                "First Name can NOT be empty...",
                "first_name",
            ),
            (
                &[("last_name", 20), ("password", 8), ("user_type", 1)],
                "Last Name can NOT be empty...",
                "last_name",
            ),
            (
                &[("password", 8), ("user_type", 1)],
                "Password can NOT be empty...",
                "password",
            ),
            (
                &[("user_type", 1)],
                "User Type can NOT be empty...",
                "user_type",
            ),
        ];
        for fill in fills {
            for (blanks, message, member) in cases {
                let mut body = valid_body();
                for &(blank, width) in blanks {
                    body[blank] = json!(fill(width));
                }
                let request = extract(&body)
                    .await?
                    .map_err(|error| format!("{member}: {error}"))?;
                let failure = rules::edit_update("USER0001", &request.as_input())
                    .err()
                    .ok_or("a blank field passed the edits")?;
                assert_eq!(
                    (failure.message(), failure.field().name()),
                    (message, member),
                    "{body}"
                );
            }
        }
        Ok(())
    }

    // audit: COUSR02C-021
    #[tokio::test]
    async fn an_omitted_or_null_password_passes_and_keeps_the_stored_one() -> TestResult {
        // An omitted or null password means unchanged (E27).
        let mut omitted = valid_body();
        omitted
            .as_object_mut()
            .ok_or("the body is not an object")?
            .remove("password");
        for body in [omitted, body_with("password", Value::Null)] {
            let request = extract(&body).await?.map_err(|error| error.to_string())?;
            assert_eq!(request.password, None, "{body}");
            let update = rules::edit_update("USER0001", &request.as_input())?;
            assert!(update.password().is_none(), "{body}");
        }
        Ok(())
    }

    // audit: COUSR02-023
    // audit: COUSR02C-027
    // audit: COSGN00C-009
    #[tokio::test]
    async fn an_accepted_password_reaches_the_update_byte_for_byte_and_signs_on() -> TestResult {
        // Upper case, because sign-on upper-cases what it is given while the update stores the
        // password as sent (E26).
        let cases: [(&str, &[u8; 8]); 3] = [
            (SECRET, b"NEWPW123"),
            ("PW 12", b"PW 12   "),
            (" ~!@Z#9", b" ~!@Z#9 "),
        ];
        for (typed, stored) in cases {
            let request = extract(&body_with("password", json!(typed)))
                .await?
                .map_err(|error| error.to_string())?;
            let update = rules::edit_update("USER0001", &request.as_input())?;
            let supplied = update
                .password()
                .ok_or("the supplied password was dropped")?;
            assert!(supplied.expose() == stored, "{typed:?}");
            let signon = cosgn00c::edit("USER0001", typed)?;
            assert!(signon.password().expose() == stored, "{typed:?}");
            if typed == SECRET {
                // The hash the rewrite stores over those bytes verifies what sign-on is given.
                let hash = password::hash(supplied.expose())?;
                assert!(password::verify(signon.password().expose(), &hash)?);
            }
        }
        Ok(())
    }

    #[test]
    fn the_request_feeds_the_programs_own_edits() -> TestResult {
        // COUSR02C-019, COUSR02C-021.
        let supplied = request(Some(SECRET));
        let input = supplied.as_input();
        assert_eq!(
            (
                input.first_name,
                input.last_name,
                input.password,
                input.user_type
            ),
            ("LAWRENCE", "THOMAS", Some(SECRET), "U")
        );
        assert!(rules::edit_update("USER0001", &input).is_ok());
        assert_eq!(request(None).as_input().password, None);

        let blank: UserUpdateRequest = serde_json::from_str(r#"{"last_name":"THOMAS"}"#)?;
        let failure = rules::edit_update("USER0001", &blank.as_input())
            .err()
            .ok_or("a blank first name passed the edit")?;
        assert_eq!(failure.message(), "First Name can NOT be empty...");
        Ok(())
    }

    #[test]
    fn a_rewrite_answers_with_the_programs_message() -> TestResult {
        // COUSR02C-029.
        let message = rules::rewrite_outcome(&field("USER0001"), rules::RewriteResult::Rewritten)?;
        assert_eq!(
            ScreenResult::from(message),
            ScreenResult {
                message: "User USER0001 has been updated ...".to_owned()
            }
        );
        Ok(())
    }
}
