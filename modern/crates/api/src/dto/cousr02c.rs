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
//! The path `user_id` of endpoints 18 to 20 is a [`UserIdPath`], which the handlers extract with
//! the parent module's `ValidPath`. It revalidates the map `LENGTH` of `USRIDIN`, 8 (COUSR02-013,
//! COUSR03-013), and the `PIC X(8)` key the ID is moved into: a longer segment, or one holding a
//! byte outside printable ASCII, is a 400 naming `user_id`, never a key cut to its first 8 bytes.
//! The text that passes reaches the service raw, which edits it with `rules::edit_user_id` or
//! `cousr03c::edit_delete`. F3's save calls endpoint 19 as PF5 does (COUSR02C-006); F4 (clear),
//! F12 (cancel) and the navigation after F3 send no request. A failure of any of the three
//! endpoints is a `rules::UserMaintFailure`, which `crate::error` maps to a `ScreenError`.
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
use core::marker::PhantomData;

use domain::online::cousr02c as rules;
use domain::online::cousr03c;
use serde::de::{self, Deserializer, IgnoredAny, MapAccess, Visitor};
use serde::ser::{SerializeStruct, Serializer};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::text;

/// What `Debug` shows in place of a supplied password.
const PASSWORD_SHOWN: &str = "********";

/// The path of endpoints 18, 19 and 20, `/api/v1/users/{user_id}`: the user ID input of the
/// update and delete maps (`USRIDIN`).
///
/// The segment is validated against the map `LENGTH`, the server half of the `both` rows
/// COUSR02-013 and COUSR03-013 of `modern/audit/screens.md`, and against the `PIC X(8)` key
/// the programs move it into: at most 8 bytes, each printable ASCII (`0x20`–`0x7E`), or
/// `LOW-VALUES`. A segment that passes, a blank one included, goes to the domain's edit as
/// typed, so a blank ID gets the program's own `User ID can NOT be empty...` (COUSR02C-010,
/// COUSR02C-018, COUSR03C-016).
#[derive(Debug, Clone, IntoParams, Validate)]
#[into_params(parameter_in = Path)]
pub struct UserIdPath {
    /// User ID as typed on the screen: at most 8 bytes of printable ASCII, neither upper-cased
    /// nor trimmed, so `admin001` does not find `ADMIN001` (COUSR02C-011, COUSR03C-017).
    #[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]
    #[param(max_length = 8)]
    pub user_id: String,
}

/// The answer of endpoint 18 for a user that has a row: the row's ID, names and type, and the
/// prompt each of the two sharing programs sends after its read (COUSR02C-012, COUSR02C-014,
/// COUSR03C-011, COUSR03C-012).
///
/// Each stored value is shown without its trailing spaces, the trimmed view of `PicX`, as the
/// map field displays it. No password is carried (`E27`).
// The schema name `UserView` is the frontend's `components["schemas"]["UserView"]` (Generated
// API Types Only).
#[derive(Debug, Clone, PartialEq, Eq, ToSchema)]
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
#[derive(Clone, ToSchema, Validate)]
#[schema(example = json!({
    "first_name": "LAWRENCE",
    "last_name": "THOMAS",
    "password": "********",
    "user_type": "U"
}))]
pub struct UserUpdateRequest {
    /// `FNAME`, `PIC X(20)`: at most 20 bytes of printable ASCII (`0x20`–`0x7E`).
    #[validate(length(max = 20), custom(function = "super::pic_x::<20>"))]
    #[schema(max_length = 20, required = false)]
    pub first_name: String,
    /// `LNAME`, `PIC X(20)`: at most 20 bytes of printable ASCII (`0x20`–`0x7E`).
    #[validate(length(max = 20), custom(function = "super::pic_x::<20>"))]
    #[schema(max_length = 20, required = false)]
    pub last_name: String,
    /// `PASSWD`, `PIC X(8)`, dark on the map: the new password, at most 8 bytes of printable
    /// ASCII (`0x20`–`0x7E`), or absent to keep the stored one. Never returned by any endpoint.
    // A write-only property of the schema, which the generated client type must carry; the
    // hand-written `Serialize` below leaves it out all the same.
    #[validate(length(max = 8), custom(function = "super::pic_x::<8>"))]
    #[schema(write_only, max_length = 8, example = "********")]
    pub password: Option<String>,
    /// `USRTYPE`, `PIC X(1)`: the type to store for the target user, one byte of printable
    /// ASCII (`0x20`–`0x7E`).
    #[validate(length(max = 1), custom(function = "super::pic_x::<1>"))]
    #[schema(max_length = 1, required = false)]
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

// Wire forms, written by hand: serde_derive wraps every implementation it generates in a block
// carrying `allow` lint suppressions, and no source of this workspace, generated or written,
// carries one. Each keeps the form the derive gave. A struct is a map of its members
// under their Rust names, in declaration order, the password never written (Cardholder Data
// Redaction); it reads its members in any order, refuses one sent twice or a required one
// missing, takes the empty text for an absent request text and `None` for an absent or `null`
// password, and skips an unknown member unread as `IgnoredAny`.

impl<'de> Deserialize<'de> for UserIdPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        read_struct(deserializer)
    }
}

impl<'de> FromMembers<'de> for UserIdPath {
    const NAME: &'static str = "UserIdPath";
    const FIELDS: &'static [&'static str] = &["user_id"];

    fn from_members<A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let mut user_id = None;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "user_id" => member(&mut map, &mut user_id, "user_id")?,
                _ => skip(&mut map)?,
            }
        }
        Ok(Self {
            user_id: required(user_id, "user_id")?,
        })
    }
}

impl Serialize for UserView {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct(<Self as FromMembers<'_>>::NAME, 6)?;
        out.serialize_field("user_id", &self.user_id)?;
        out.serialize_field("first_name", &self.first_name)?;
        out.serialize_field("last_name", &self.last_name)?;
        out.serialize_field("user_type", &self.user_type)?;
        out.serialize_field("update_prompt", &self.update_prompt)?;
        out.serialize_field("delete_prompt", &self.delete_prompt)?;
        out.end()
    }
}

impl<'de> Deserialize<'de> for UserView {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        read_struct(deserializer)
    }
}

impl<'de> FromMembers<'de> for UserView {
    const NAME: &'static str = "UserView";
    const FIELDS: &'static [&'static str] = &[
        "user_id",
        "first_name",
        "last_name",
        "user_type",
        "update_prompt",
        "delete_prompt",
    ];

    fn from_members<A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let (mut user_id, mut first_name, mut last_name) = (None, None, None);
        let (mut user_type, mut update_prompt, mut delete_prompt) = (None, None, None);
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "user_id" => member(&mut map, &mut user_id, "user_id")?,
                "first_name" => member(&mut map, &mut first_name, "first_name")?,
                "last_name" => member(&mut map, &mut last_name, "last_name")?,
                "user_type" => member(&mut map, &mut user_type, "user_type")?,
                "update_prompt" => member(&mut map, &mut update_prompt, "update_prompt")?,
                "delete_prompt" => member(&mut map, &mut delete_prompt, "delete_prompt")?,
                _ => skip(&mut map)?,
            }
        }
        Ok(Self {
            user_id: required(user_id, "user_id")?,
            first_name: required(first_name, "first_name")?,
            last_name: required(last_name, "last_name")?,
            user_type: required(user_type, "user_type")?,
            update_prompt: required(update_prompt, "update_prompt")?,
            delete_prompt: required(delete_prompt, "delete_prompt")?,
        })
    }
}

impl Serialize for UserUpdateRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct(<Self as FromMembers<'_>>::NAME, 3)?;
        out.serialize_field("first_name", &self.first_name)?;
        out.serialize_field("last_name", &self.last_name)?;
        out.skip_field("password")?;
        out.serialize_field("user_type", &self.user_type)?;
        out.end()
    }
}

impl<'de> Deserialize<'de> for UserUpdateRequest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        read_struct(deserializer)
    }
}

impl<'de> FromMembers<'de> for UserUpdateRequest {
    const NAME: &'static str = "UserUpdateRequest";
    const FIELDS: &'static [&'static str] = &["first_name", "last_name", "password", "user_type"];

    fn from_members<A: MapAccess<'de>>(mut map: A) -> Result<Self, A::Error> {
        let (mut first_name, mut last_name) = (None, None);
        let (mut password, mut user_type) = (None, None);
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "first_name" => member(&mut map, &mut first_name, "first_name")?,
                "last_name" => member(&mut map, &mut last_name, "last_name")?,
                "password" => member(&mut map, &mut password, "password")?,
                "user_type" => member(&mut map, &mut user_type, "user_type")?,
                _ => skip(&mut map)?,
            }
        }
        Ok(Self {
            first_name: first_name.unwrap_or_default(),
            last_name: last_name.unwrap_or_default(),
            password: password.unwrap_or_default(),
            user_type: user_type.unwrap_or_default(),
        })
    }
}

/// A struct of this module read from the members of a map.
trait FromMembers<'de>: Sized {
    /// The Rust name, which a format may record.
    const NAME: &'static str;
    /// The members' wire names, in declaration order.
    const FIELDS: &'static [&'static str];

    /// The struct from `map`'s members.
    ///
    /// # Errors
    ///
    /// A member sent twice, a required member missing, or a value that does not deserialise.
    fn from_members<A: MapAccess<'de>>(map: A) -> Result<Self, A::Error>;
}

/// `T` read from `deserializer` as a struct: a map, through [`MembersVisitor`].
fn read_struct<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: FromMembers<'de>,
{
    deserializer.deserialize_struct(T::NAME, T::FIELDS, MembersVisitor(PhantomData))
}

/// The visitor of [`read_struct`]: it accepts a map alone and hands its members to `T`.
struct MembersVisitor<T>(PhantomData<T>);

impl<'de, T: FromMembers<'de>> Visitor<'de> for MembersVisitor<T> {
    type Value = T;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "struct {}", T::NAME)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<T, A::Error> {
        T::from_members(map)
    }
}

/// Reads the value of the member `name` into `slot`. A second occurrence is the duplicate-field
/// error, raised before its value is read.
fn member<'de, A, T>(map: &mut A, slot: &mut Option<T>, name: &'static str) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

/// Skips the value of a member no struct of this module names, unread.
fn skip<'de, A: MapAccess<'de>>(map: &mut A) -> Result<(), A::Error> {
    map.next_value::<IgnoredAny>().map(|IgnoredAny| ())
}

/// The value read for the required member `name`, or the missing-field error.
fn required<T, E: de::Error>(slot: Option<T>, name: &'static str) -> Result<T, E> {
    slot.ok_or_else(|| E::missing_field(name))
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::{self, Body};
    use axum::extract::{FromRequest, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use axum::routing::get;
    use axum::{Json, Router};
    use domain::copybook::PicX;
    use domain::online::cosgn00c;
    use domain::password;
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use tower::ServiceExt as _;
    use utoipa::{IntoParams, PartialSchema, ToSchema};
    use validator::Validate;

    use super::{UserIdPath, UserUpdateRequest, UserView, rules};
    use crate::dto::{FIELD_LENGTH_MESSAGE, INVALID_PATH, ScreenResult, ValidJson, ValidPath};
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread runtime, the one Tokio's test attribute
    /// builds, without the lint attributes that attribute's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

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

    #[test]
    fn a_field_beyond_its_map_length_is_a_400_naming_it() -> TestResult {
        block_on(async {
            // COUSR02-017, COUSR02-020, COUSR02-023, COUSR02-026.
            let cases = [
                (json!({ "first_name": "F".repeat(21) }), "first_name"),
                (json!({ "last_name": "L".repeat(21) }), "last_name"),
                (json!({ "password": format!("{SECRET}9") }), "password"),
                (json!({ "user_type": "UA" }), "user_type"),
            ];
            for (body, name) in cases {
                let error = ValidJson::<UserUpdateRequest>::from_request(
                    json_request(&body.to_string())?,
                    &(),
                )
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
        })
    }

    // audit: COUSR02-017
    // audit: COUSR02-020
    // audit: COUSR02-023
    // audit: COUSR02-026
    #[test]
    fn a_value_its_pic_x_field_cannot_hold_is_a_400_naming_it() -> TestResult {
        block_on(async {
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
        })
    }

    // audit: COUSR02C-019
    // audit: COUSR02C-020
    // audit: COUSR02C-021
    // audit: COUSR02C-022
    #[test]
    fn blank_and_low_values_reach_the_programs_blank_edits_in_its_order() -> TestResult {
        block_on(async {
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
        })
    }

    // audit: COUSR02C-021
    #[test]
    fn an_omitted_or_null_password_passes_and_keeps_the_stored_one() -> TestResult {
        block_on(async {
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
        })
    }

    // audit: COUSR02-023
    // audit: COUSR02C-027
    // audit: COSGN00C-009
    #[test]
    fn an_accepted_password_reaches_the_update_byte_for_byte_and_signs_on() -> TestResult {
        block_on(async {
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
        })
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

    /// The status and body a route of endpoints 18 to 20 gives `uri` once `ValidPath` has
    /// extracted its [`UserIdPath`]: the user ID it would pass on, or the rejection.
    async fn through_path(uri: &str) -> Result<(StatusCode, Value), Box<dyn StdError>> {
        let router = Router::new().route(
            "/api/v1/users/{user_id}",
            get(|ValidPath(path): ValidPath<UserIdPath>| async move {
                Json(json!({ "user_id": path.user_id }))
            }),
        );
        let response = router
            .oneshot(Request::builder().uri(uri).body(Body::empty())?)
            .await?;
        let status = response.status();
        let bytes = body::to_bytes(response.into_body(), usize::MAX).await?;
        Ok((status, serde_json::from_slice(&bytes)?))
    }

    #[test]
    fn the_path_user_id_is_revalidated_against_its_map_length() -> TestResult {
        block_on(async {
            // COUSR02-013, COUSR03-013: within the map `LENGTH` the text passes as typed, blank,
            // spaced and lower-case IDs included, to the programs' own edits (COUSR02C-010,
            // COUSR02C-011, COUSR03C-016, COUSR03C-017).
            for (uri, passed) in [
                ("/api/v1/users/USER0001", "USER0001"),
                ("/api/v1/users/admin001", "admin001"),
                ("/api/v1/users/AB%20CD", "AB CD"),
                ("/api/v1/users/%20AB", " AB"),
                ("/api/v1/users/%20%20%20", "   "),
            ] {
                assert_eq!(
                    through_path(uri).await?,
                    (StatusCode::OK, json!({ "user_id": passed })),
                    "{uri}"
                );
            }
            // Beyond 8 bytes, or a byte the `PIC X(8)` key cannot hold: rejected naming the
            // segment, never moved into the key cut to its first 8 bytes.
            for uri in [
                "/api/v1/users/USER00019",
                "/api/v1/users/%C3%A9",
                "/api/v1/users/AB%09CD",
            ] {
                assert_eq!(
                    through_path(uri).await?,
                    (
                        StatusCode::BAD_REQUEST,
                        json!({ "message": FIELD_LENGTH_MESSAGE, "field": "user_id" })
                    ),
                    "{uri}"
                );
            }
            // A segment that is not UTF-8 does not deserialise.
            assert_eq!(
                through_path("/api/v1/users/%FF").await?,
                (StatusCode::BAD_REQUEST, json!({ "message": INVALID_PATH }))
            );
            Ok(())
        })
    }

    #[test]
    fn the_path_parameter_publishes_its_map_length() -> TestResult {
        let parameters = serde_json::to_value(UserIdPath::into_params(|| None))?;
        assert_eq!(parameters[0]["name"], json!("user_id"), "{parameters}");
        assert_eq!(parameters[0]["in"], json!("path"), "{parameters}");
        assert_eq!(parameters[0]["required"], json!(true), "{parameters}");
        assert_eq!(
            parameters[0]["schema"]["maxLength"],
            json!(8),
            "{parameters}"
        );
        assert_eq!(parameters.as_array().map(Vec::len), Some(1), "{parameters}");
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

    #[test]
    fn every_body_keeps_its_wire_form_byte_for_byte() -> TestResult {
        // Members in declaration order; the password is no member of either.
        let view = UserView::from(user0001());
        let wire = format!(
            r#"{{"user_id":"USER0001","first_name":"LAWRENCE","last_name":"THOMAS","user_type":"U","update_prompt":"{UPDATE_PROMPT}","delete_prompt":"{DELETE_PROMPT}"}}"#
        );
        assert_eq!(serde_json::to_string(&view)?, wire);
        assert_eq!(serde_json::from_str::<UserView>(&wire)?, view);
        assert_eq!(
            serde_json::to_string(&request(Some(SECRET)))?,
            r#"{"first_name":"LAWRENCE","last_name":"THOMAS","user_type":"U"}"#
        );
        Ok(())
    }

    #[test]
    fn a_member_sent_twice_is_refused_and_an_unknown_one_skipped() -> TestResult {
        let view = format!(
            r#""user_id":"U1","first_name":"F","last_name":"L","user_type":"U","update_prompt":"{UPDATE_PROMPT}","delete_prompt":"{DELETE_PROMPT}""#
        );
        let cases: [ReadCase; 7] = [
            (reads::<UserView>, format!("{{{view}}}"), true),
            (
                reads::<UserView>,
                format!(r#"{{{view},"user_id":"U2"}}"#),
                false,
            ),
            (
                reads::<UserView>,
                format!(r#"{{"password":"{SECRET}",{view}}}"#),
                true,
            ),
            (
                reads::<UserView>,
                r#"{"user_id":"U1","first_name":"F"}"#.to_owned(),
                false,
            ),
            (
                reads::<UserUpdateRequest>,
                format!(r#"{{"password":"{SECRET}","password":null}}"#),
                false,
            ),
            (
                reads::<UserUpdateRequest>,
                r#"{"user_type":"U","user_type":"A"}"#.to_owned(),
                false,
            ),
            (
                reads::<UserUpdateRequest>,
                r#"{"user_type":"A","x":[1e400],"first_name":"F"}"#.to_owned(),
                true,
            ),
        ];
        for (read, json, accepted) in cases {
            assert_eq!(read(&json), accepted, "{json}");
        }
        let reordered: UserUpdateRequest =
            serde_json::from_str(r#"{"user_type":"A","x":[1e400],"first_name":"F"}"#)?;
        assert_eq!(
            (
                reordered.first_name.as_str(),
                reordered.last_name.as_str(),
                reordered.password,
                reordered.user_type.as_str()
            ),
            ("F", "", None, "A")
        );
        Ok(())
    }

    /// A reading case: whether a type reads the JSON text, the text, and the expected answer.
    type ReadCase = (fn(&str) -> bool, String, bool);

    /// Whether `json` deserialises into `T`.
    fn reads<T: DeserializeOwned>(json: &str) -> bool {
        serde_json::from_str::<T>(json).is_ok()
    }
}
