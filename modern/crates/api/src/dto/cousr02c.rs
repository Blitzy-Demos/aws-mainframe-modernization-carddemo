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
/// Only the map `LENGTH` of each field is validated here, the server half of its `both` row of
/// `modern/audit/screens.md`: `first_name` and `last_name` 20 (COUSR02-017, COUSR02-020),
/// `password` 8 (COUSR02-023) and `user_type` 1 (COUSR02-026). A name or type that is absent is
/// the empty text, so the program's own blank-field edits answer it with their messages, in its
/// order (COUSR02C-019 to COUSR02C-022); no value edit applies to the type (COUSR02C-023).
///
/// The password is optional and write-only: omitted or `null`, the stored hash is kept (`E27`).
/// It never leaves through `Serialize`, so a client that sends one writes the JSON itself, and
/// `Debug` shows a supplied one as `********`.
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
    /// `FNAME`, `PIC X(20)`.
    #[serde(default)]
    #[validate(length(max = 20))]
    pub first_name: String,
    /// `LNAME`, `PIC X(20)`.
    #[serde(default)]
    #[validate(length(max = 20))]
    pub last_name: String,
    /// `PASSWD`, `PIC X(8)`, dark on the map: the new password, or absent to keep the stored
    /// one. Never returned by any endpoint.
    // `skip_serializing_if` with an always-true predicate, not `skip_serializing`: utoipa 5.5
    // reads `skip_serializing` as `skip` and drops the property from the OpenAPI document,
    // which would take the write-only `password` out of the generated client type.
    #[serde(default, skip_serializing_if = "never_serialized")]
    #[validate(length(max = 8))]
    #[schema(write_only, example = "********")]
    pub password: Option<String>,
    /// `USRTYPE`, `PIC X(1)`: the type to store for the target user.
    #[serde(default)]
    #[validate(length(max = 1))]
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
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};
    use validator::Validate;

    use super::{UserUpdateRequest, UserView, rules};
    use crate::dto::{FIELD_LENGTH_MESSAGE, ScreenResult, ValidJson};

    type TestResult = Result<(), Box<dyn StdError>>;

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
