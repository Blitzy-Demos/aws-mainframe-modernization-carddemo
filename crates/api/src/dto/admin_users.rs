//! The wire types of user administration, CU00 to CU03: the `COUSR0nC`
//! programs over the `USRSEC` file (`CSUSR01Y` `SEC-USER-DATA`
//! [app/cpy/CSUSR01Y.cpy:L17-23]), served to administrators only.
//!
//! | Endpoint | Program | Request | Response | Error fields |
//! |---|---|---|---|---|
//! | `GET /admin/users` | `COUSR00C` (CU00) | [`UserListQuery`] | `Page<UserSummary>` | [`UserListField`] |
//! | `GET /admin/users/{id}` | `COUSR02C` and `COUSR03C` `PROCESS-ENTER-KEY` | path | [`UserSummary`] | [`UserUpdateField`] |
//! | `POST /admin/users` | `COUSR01C` (CU01) | [`UserAddRequest`] | [`UserMessageResponse`] | [`UserAddField`] |
//! | `PUT /admin/users/{id}` | `COUSR02C` (CU02) | [`UserUpdateRequest`] | [`UserMessageResponse`] | [`UserUpdateField`] |
//! | `DELETE /admin/users/{id}` | `COUSR03C` (CU03) | path | [`UserMessageResponse`] | [`UserDeleteField`] |
//!
//! The fetch rows of `COUSR03C` (V-COUSR03C-001 to V-COUSR03C-003) name the
//! same fields as those of `COUSR02C` (V-COUSR02C-001 to V-COUSR02C-003), so
//! one fetch serves both screens.
//!
//! # Passwords (E-20, E-21)
//!
//! No response type has a password member, so neither a password nor its
//! argon2id hash can be serialized: `COUSR00C` never lists `SEC-USR-PWD`, and
//! the hash that replaces it cannot be shown where `COUSR02C` moves the stored
//! password into its edit field (P-COUSR02C-007). The request types carry the
//! password exactly as entered, case kept and untrimmed; the rules pad it to
//! `PIC X(08)` and hash it. Their `Debug` output leaves the password out, and
//! they implement no `Serialize`.
//!
//! # Lengths and encoding
//!
//! The only edits made here are the BMS field width and its encoding.
//! `USRIDINI`, `USERIDI` and `PASSWDI` are `PIC X(8)`, `FNAMEI` and `LNAMEI`
//! `PIC X(20)` and `USRTYPEI` `PIC X(1)` [app/cpy-bms/COUSR01.CPY:L60-84]
//! [app/cpy-bms/COUSR02.CPY:L60-84] [app/cpy-bms/COUSR03.CPY:L60]. Every
//! member and key is also held to ASCII ([`crate::dto::ascii_text`]): the
//! `COUSR0nC` rules `MOVE` the UTF-8 bytes into those fields and into
//! `SEC-USER-DATA`, so a non-ASCII character would take two to four bytes of
//! the width, be cut at it, and read back one Latin-1 character per byte (`é`
//! stored as `Ã©`), and a user ID added that way could not be addressed again.
//! Neither edit has a `COUSR0nC` path or text, so both are answered alike: 400
//! `Invalid` on the field with an empty message. A blank or missing field is
//! ASCII and reaches the rule unchanged, which answers it with the audited
//! empty-field message.

use std::fmt;

use carddemo_domain::entities::UserSecurity;
use carddemo_domain::error::ErrorKind;
use carddemo_domain::rules::user_add::UserAddInput;
use carddemo_domain::rules::user_list::UserListPage;
use carddemo_domain::rules::user_update::UserUpdateInput;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::Validate;

use super::{BmsField, ascii_text, bms_errors, id_string, reject_invalid, text};
use crate::error::ApiError;
use crate::pagination::{Page, PageDirection, PageQuery, PageSize};

/// The width of a user ID: `USRIDINI` of maps `COUSR0A`, `COUSR2A` and
/// `COUSR3A` and `USERIDI` of `COUSR1A`, `PIC X(8)`
/// [app/cpy-bms/COUSR00.CPY:L66] [app/cpy-bms/COUSR01.CPY:L72], the width of
/// the key `SEC-USR-ID PIC X(08)` [app/cpy/CSUSR01Y.cpy:L18].
const USER_ID_LEN: usize = 8;

/// The `message` of a field or key longer than its BMS field, or holding a
/// character outside ASCII.
///
/// A 3270 field never sends more than its `PIC X(n)` width, and holds one
/// byte per character, so no `COUSR0nC` path or message exists for a longer
/// value or for a character the `MOVE` of its UTF-8 bytes would split, and
/// the validation audit has no row for either: the request is answered as a
/// no-message path, 400 `Invalid` with the field named and an empty text, as
/// E-49 answers the REST inputs that have no COBOL counterpart.
const OVER_LENGTH: &str = "";

/// The BMS fields a `GET /admin/users` error names.
///
/// Every `COUSR00C` path, page-boundary texts and browse failures alike,
/// moves -1 to `USRIDINL`, so the cursor stays on the search field
/// (V-COUSR00C-001 to V-COUSR00C-009).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum UserListField {
    /// `USRIDINI PIC X(8)` of `COUSR0A` [app/cpy-bms/COUSR00.CPY:L66], the
    /// user ID the page starts from: the `start_key` parameter.
    #[serde(rename = "USRIDIN")]
    UsrIdIn,
}

impl UserListField {
    /// The BMS field name, the `fields[].field` value of an error.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsrIdIn => "USRIDIN",
        }
    }
}

impl BmsField for UserListField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    fn rank(self) -> u16 {
        match self {
            Self::UsrIdIn => 1,
        }
    }
}

/// The BMS fields a `POST /admin/users` error names: the `COUSR1A` inputs
/// [app/cpy-bms/COUSR01.CPY:L60-84].
///
/// `COUSR01C` `PROCESS-ENTER-KEY` edits the five fields in one `EVALUATE`
/// (V-COUSR01C-001 to V-COUSR01C-005, all of Order 1), and
/// `WRITE-USER-SEC-FILE` puts the cursor on `USERID` for a duplicate key and
/// on `FNAME` for any other write failure (V-COUSR01C-006, V-COUSR01C-007).
// The ranks follow the audit's (Order, ID) order, which its conventions
// define as complete: the five edits share Order 1 and the first true WHEN
// wins, so the screen order FNAME, LNAME, USERID, PASSWD, USRTYPE decides
// rather than the field names `bms_errors` breaks equal ranks by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum UserAddField {
    /// `FNAMEI PIC X(20)`, the first name: `first_name`.
    #[serde(rename = "FNAME")]
    FName,
    /// `LNAMEI PIC X(20)`, the last name: `last_name`.
    #[serde(rename = "LNAME")]
    LName,
    /// `USERIDI PIC X(8)`, the new user's ID: `user_id`.
    #[serde(rename = "USERID")]
    UserId,
    /// `PASSWDI PIC X(8)`, the password: `password`.
    #[serde(rename = "PASSWD")]
    Passwd,
    /// `USRTYPEI PIC X(1)`, the user type: `usr_type`.
    #[serde(rename = "USRTYPE")]
    UsrType,
}

impl UserAddField {
    /// The BMS field name, the `fields[].field` value of an error.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FName => "FNAME",
            Self::LName => "LNAME",
            Self::UserId => "USERID",
            Self::Passwd => "PASSWD",
            Self::UsrType => "USRTYPE",
        }
    }

    /// The field of a [`UserAddRequest`] member, by the JSON name `validator`
    /// reports.
    fn of_request_member(name: &str) -> Option<Self> {
        match name {
            "first_name" => Some(Self::FName),
            "last_name" => Some(Self::LName),
            "user_id" => Some(Self::UserId),
            "password" => Some(Self::Passwd),
            "usr_type" => Some(Self::UsrType),
            _ => None,
        }
    }
}

impl BmsField for UserAddField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    fn rank(self) -> u16 {
        match self {
            Self::FName => 1,
            Self::LName => 2,
            Self::UserId => 3,
            Self::Passwd => 4,
            Self::UsrType => 5,
        }
    }
}

/// The BMS fields a `GET` or `PUT /admin/users/{id}` error names: the
/// `COUSR2A` inputs [app/cpy-bms/COUSR02.CPY:L60-84].
///
/// The fetch, `COUSR02C` `PROCESS-ENTER-KEY`, marks `USRIDIN` for a blank or
/// unknown ID and `FNAME` for a failed read (V-COUSR02C-001 to
/// V-COUSR02C-003). The update (`UPDATE-USER-INFO`) edits the five fields in
/// one `EVALUATE` (V-COUSR02C-004 to V-COUSR02C-008, all of Order 1) and puts
/// the cursor on `FNAME` or `USRIDIN` after its read and rewrite
/// (V-COUSR02C-009 to V-COUSR02C-012).
// The ranks follow the audit's (Order, ID) order, as for `UserAddField`: the
// screen order USRIDIN, FNAME, LNAME, PASSWD, USRTYPE of the update's
// EVALUATE, which the fetch rows (USRIDIN before FNAME) agree with.
// The doc text reaches web/src/api/schema.d.ts, whose hygiene scan reads a word
// `fetch` followed by `(` as a call; its wording keeps them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum UserUpdateField {
    /// `USRIDINI PIC X(8)`, the ID of the user fetched or updated: the
    /// `{id}` path segment.
    #[serde(rename = "USRIDIN")]
    UsrIdIn,
    /// `FNAMEI PIC X(20)`, the first name: `first_name`.
    #[serde(rename = "FNAME")]
    FName,
    /// `LNAMEI PIC X(20)`, the last name: `last_name`.
    #[serde(rename = "LNAME")]
    LName,
    /// `PASSWDI PIC X(8)`, the password: `password`.
    #[serde(rename = "PASSWD")]
    Passwd,
    /// `USRTYPEI PIC X(1)`, the user type: `usr_type`.
    #[serde(rename = "USRTYPE")]
    UsrType,
}

impl UserUpdateField {
    /// The BMS field name, the `fields[].field` value of an error.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsrIdIn => "USRIDIN",
            Self::FName => "FNAME",
            Self::LName => "LNAME",
            Self::Passwd => "PASSWD",
            Self::UsrType => "USRTYPE",
        }
    }

    /// The field of a [`UserUpdateRequest`] member, by the JSON name
    /// `validator` reports.
    fn of_request_member(name: &str) -> Option<Self> {
        match name {
            "first_name" => Some(Self::FName),
            "last_name" => Some(Self::LName),
            "password" => Some(Self::Passwd),
            "usr_type" => Some(Self::UsrType),
            _ => None,
        }
    }
}

impl BmsField for UserUpdateField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    fn rank(self) -> u16 {
        match self {
            Self::UsrIdIn => 1,
            Self::FName => 2,
            Self::LName => 3,
            Self::Passwd => 4,
            Self::UsrType => 5,
        }
    }
}

/// The BMS fields a `DELETE /admin/users/{id}` error names: `USRIDINI` and
/// `FNAMEI` of `COUSR3A` [app/cpy-bms/COUSR03.CPY:L60-66].
///
/// `COUSR03C` `DELETE-USER-INFO` marks `USRIDIN` for a blank ID
/// (V-COUSR03C-004) and a delete that answers `NOTFND` (V-COUSR03C-006), and
/// `FNAME` for the `WHEN OTHER` text 'Unable to Update User...' of
/// `DELETE-USER-SEC-FILE` (V-COUSR03C-005, V-COUSR03C-007).
// The variants are the BMS fields of the audit's COUSR03C rows, which are
// authoritative: FNAME is among them, so an error body can carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum UserDeleteField {
    /// `USRIDINI PIC X(8)`, the ID of the user deleted: the `{id}` path
    /// segment.
    #[serde(rename = "USRIDIN")]
    UsrIdIn,
    /// `FNAMEI PIC X(20)`, where the cursor goes when the read for update or
    /// the delete fails.
    #[serde(rename = "FNAME")]
    FName,
}

impl UserDeleteField {
    /// The BMS field name, the `fields[].field` value of an error.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UsrIdIn => "USRIDIN",
            Self::FName => "FNAME",
        }
    }
}

impl BmsField for UserDeleteField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The delete's Order: the user-ID edit (1) comes before the read and
    /// delete (2 and 3) that mark `FNAME`.
    fn rank(self) -> u16 {
        match self {
            Self::UsrIdIn => 1,
            Self::FName => 2,
        }
    }
}

/// Rejects a user ID key longer than `PIC X(8)` or holding a character
/// outside ASCII: the `{id}` path segment of `GET`, `PUT` and
/// `DELETE /admin/users/{id}` (`USRIDINI`) and the `start_key` of
/// `GET /admin/users`.
///
/// `COUSR00C` `PROCESS-PAGE-FORWARD`, `COUSR02C` and `COUSR03C`
/// `PROCESS-ENTER-KEY` and `COUSR03C` `DELETE-USER-INFO` move the key into
/// `SEC-USR-ID PIC X(08)` byte for byte (P-COUSR00C-003, P-COUSR02C-002,
/// P-COUSR03C-002, P-COUSR03C-004). Within ASCII,
/// characters and bytes agree, so the 8 characters `maxLength` counts are the
/// 8 bytes the key holds; a non-ASCII character would be stored as two to
/// four bytes and address another key. A key of 8 ASCII characters or fewer
/// passes, blank, empty and low-values included: the rule answers a blank key
/// with its own message (V-COUSR02C-001, V-COUSR02C-004, V-COUSR03C-004) or,
/// for the list, starts at the top of the file.
///
/// # Errors
///
/// 400 with code `INVALID`, an empty message and `field` as the one error
/// field, when `value` holds more than 8 characters or a character above
/// `U+007F`: no `COUSR0nC` path receives such a key, so no row of the audit
/// answers it.
// Crate-private, as `BmsField` is: a public function bounded by a
// crate-private trait would trip the `private_bounds` lint.
pub(crate) fn check_user_id_key(value: &str, field: impl BmsField) -> Result<(), ApiError> {
    if value.chars().count() > USER_ID_LEN || ascii_text(value).is_err() {
        return Err(ApiError::no_message(
            ErrorKind::Invalid,
            OVER_LENGTH,
            bms_errors([field], OVER_LENGTH),
        ));
    }
    Ok(())
}

/// The query of `GET /admin/users` (CU00): `start_key`, `direction` and
/// `page_size`.
///
/// `COUSR00C` `PROCESS-ENTER-KEY`, `PROCESS-PF7-KEY` and `PROCESS-PF8-KEY` —
/// P-COUSR00C-003, P-COUSR00C-004. The parameters are those of
/// [`PageQuery`], declared here so that the published `start_key` carries
/// the `USRIDINI PIC X(8)` width and the ASCII pattern that
/// [`check_user_id_key`] holds it to. `direction` and `page_size` are
/// published and read exactly as `PageQuery` declares them, and the query
/// converts into one, so [`PageQuery::resolve`] gives the browse its
/// direction and its 10-row default. An unknown `direction` or a `page_size`
/// outside 1 to 100 fails to deserialize, so
/// [`ApiQuery`](crate::error::ApiQuery) answers it 400 with code `INVALID`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UserListQuery {
    /// The user ID the page starts from, `USRIDINI` `X(8)` in ASCII, case
    /// kept: the search field, or a key a page returned. Forward, the page
    /// starts at the first user ID at or after it; backward, it holds the
    /// users before that one. Absent, empty, all spaces or all low-values is
    /// the top of the file: the first page forward, and backward an empty
    /// page with `You are already at the top of the page...`.
    #[param(max_length = 8, pattern = "^[\\x00-\\x7F]*$")]
    pub start_key: Option<String>,
    /// `forward` (ENTER and PF8, the default) or `backward` (PF7).
    // Inlined, as `PageQuery` inlines it: query parameters register no
    // component schema, so a reference to `PageDirection` would dangle.
    #[param(inline)]
    pub direction: Option<PageDirection>,
    /// Rows on the page, from 1 to 100. Defaults to 10, the rows of the
    /// `COUSR0A` screen.
    #[param(value_type = Option<u16>, minimum = 1, maximum = 100)]
    pub page_size: Option<PageSize>,
}

/// The shared pagination query of the user list, each parameter unchanged.
impl From<UserListQuery> for PageQuery {
    fn from(query: UserListQuery) -> Self {
        let UserListQuery {
            start_key,
            direction,
            page_size,
        } = query;
        Self {
            start_key,
            direction,
            page_size,
        }
    }
}

/// One user as the user list and the fetch show it.
///
/// The list rows are `USRIDnnI`, `FNAMEnnI`, `LNAMEnnI` and `UTYPEnnI` of
/// `COUSR0A` [app/cpy-bms/COUSR00.CPY:L78-96], filled by `COUSR00C`
/// `POPULATE-USER-DATA` (P-COUSR00C-010, P-COUSR00C-011); the fetch fills
/// `FNAMEI`, `LNAMEI` and `USRTYPEI` of `COUSR2A` and `COUSR3A` in `COUSR02C`
/// and `COUSR03C` `PROCESS-ENTER-KEY`. There is no password member (E-21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UserSummary {
    /// `SEC-USR-ID PIC X(08)`, the user ID as stored, case kept, without
    /// trailing spaces.
    #[schema(max_length = 8)]
    pub user_id: String,
    /// `SEC-USR-FNAME PIC X(20)`, the first name, without trailing spaces.
    #[schema(max_length = 20)]
    pub first_name: String,
    /// `SEC-USR-LNAME PIC X(20)`, the last name, without trailing spaces.
    #[schema(max_length = 20)]
    pub last_name: String,
    /// `SEC-USR-TYPE PIC X(01)`, the user type: `A` for an administrator, `U`
    /// for a regular user, or the other value it was added with.
    #[schema(max_length = 1)]
    pub usr_type: String,
}

impl From<&UserSecurity> for UserSummary {
    fn from(user: &UserSecurity) -> Self {
        Self {
            user_id: id_string(&user.sec_usr_id),
            first_name: text(&user.sec_usr_fname),
            last_name: text(&user.sec_usr_lname),
            usr_type: text(&user.sec_usr_type),
        }
    }
}

/// The `COUSR00C` page as `GET /admin/users` returns it: each user as a
/// [`UserSummary`], the neighbouring start keys as right-trimmed user IDs, and
/// the page-boundary text unchanged (E-54).
impl From<UserListPage> for Page<UserSummary> {
    fn from(page: UserListPage) -> Self {
        let UserListPage {
            items,
            next_key,
            prev_key,
            message,
        } = page;
        Page::new(
            items.iter().map(UserSummary::from).collect(),
            next_key.map(|key| id_string(&key)),
            prev_key.map(|key| id_string(&key)),
            message.map(str::to_owned),
        )
    }
}

/// The answer to an add, update or delete: the user and the text the screen
/// shows.
///
/// `message` is the rule's text unchanged: `User <id> has been added ...`
/// (`COUSR01C` `WRITE-USER-SEC-FILE`, P-COUSR01C-004), `User <id> has been
/// updated ...` (`COUSR02C` `UPDATE-USER-SEC-FILE`, P-COUSR02C-006), `Please
/// modify to update ...` when no field changed (V-COUSR02C-010), or `User <id>
/// has been deleted ...` (`COUSR03C` `DELETE-USER-SEC-FILE`, P-COUSR03C-005).
/// The success texts fill the `WS-MESSAGE PIC X(80)` field, trailing spaces
/// included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct UserMessageResponse {
    /// The user as written, or, for a delete, as read before it was deleted.
    pub user: UserSummary,
    /// The text the `COUSR0nC` program shows on success.
    pub message: String,
}

impl UserMessageResponse {
    /// The response for `user` with the rule's `message`.
    #[must_use]
    pub fn new(user: &UserSecurity, message: impl Into<String>) -> Self {
        Self {
            user: UserSummary::from(user),
            message: message.into(),
        }
    }
}

/// The body of `POST /admin/users`: the five `COUSR1A` inputs
/// [app/cpy-bms/COUSR01.CPY:L60-84] that `COUSR01C` `PROCESS-ENTER-KEY`
/// edits and moves into `SEC-USER-DATA` [app/cbl/COUSR01C.cbl:L115-160].
///
/// Every member is taken as entered: no trimming and no case change, so a
/// user ID or password stored in lower or mixed case is kept that way and
/// never signs on, as in the COBOL (E-20, P-COUSR01C-003). A missing member is
/// the empty field, which the rule rejects with its empty-field message
/// (V-COUSR01C-001 to V-COUSR01C-005).
#[derive(Clone, Default, PartialEq, Eq, Deserialize, ToSchema, Validate)]
pub struct UserAddRequest {
    /// `FNAMEI PIC X(20)`, the first name, in ASCII.
    #[serde(default)]
    #[schema(max_length = 20, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 20), custom(function = "crate::dto::ascii_text"))]
    pub first_name: Option<String>,
    /// `LNAMEI PIC X(20)`, the last name, in ASCII.
    #[serde(default)]
    #[schema(max_length = 20, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 20), custom(function = "crate::dto::ascii_text"))]
    pub last_name: Option<String>,
    /// `USERIDI PIC X(8)`, the new user's ID in ASCII, stored with its case
    /// kept.
    #[serde(default)]
    #[schema(max_length = 8, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 8), custom(function = "crate::dto::ascii_text"))]
    pub user_id: Option<String>,
    /// `PASSWDI PIC X(8)`, the password in ASCII, stored as the argon2id hash
    /// of the value as entered, case kept and space-padded to 8 characters.
    #[serde(default)]
    #[schema(max_length = 8, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 8), custom(function = "crate::dto::ascii_text"))]
    pub password: Option<String>,
    /// `USRTYPEI PIC X(1)`, the user type: `A` for an administrator, `U` for a
    /// regular user. Any other non-blank ASCII value is stored as entered.
    #[serde(default)]
    #[schema(max_length = 1, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 1), custom(function = "crate::dto::ascii_text"))]
    pub usr_type: Option<String>,
}

impl fmt::Debug for UserAddRequest {
    /// Every member but the password, ended with `..` to show that one is
    /// left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserAddRequest")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("user_id", &self.user_id)
            .field("usr_type", &self.usr_type)
            .finish_non_exhaustive()
    }
}

impl UserAddRequest {
    /// Checks every member against its BMS field width and for ASCII, before
    /// `COUSR01C` `PROCESS-ENTER-KEY` moves it into `SEC-USER-DATA`
    /// (P-COUSR01C-002, P-COUSR01C-003); blank members pass, for the
    /// empty-field edits (V-COUSR01C-001 to V-COUSR01C-005).
    ///
    /// # Errors
    ///
    /// 400 with code `INVALID`, an empty message and one [`UserAddField`] per
    /// member longer than its field or holding a character above `U+007F`,
    /// in screen order.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate()
            .map_err(|errors| reject_invalid(&errors, UserAddField::of_request_member, OVER_LENGTH))
    }

    /// The `COUSR1AI` fields of the rule, each member as entered and a missing
    /// one as the empty field.
    #[must_use]
    pub fn into_input(self) -> UserAddInput {
        UserAddInput {
            first_name: self.first_name.unwrap_or_default(),
            last_name: self.last_name.unwrap_or_default(),
            user_id: self.user_id.unwrap_or_default(),
            password: self.password.unwrap_or_default(),
            usr_type: self.usr_type.unwrap_or_default(),
        }
    }
}

/// The body of `PUT /admin/users/{id}`: the `COUSR2A` inputs
/// [app/cpy-bms/COUSR02.CPY:L66-84] that `COUSR02C` `UPDATE-USER-INFO` edits
/// and compares with the stored user [app/cbl/COUSR02C.cbl:L177-245]. The
/// user ID, `USRIDINI`, is the `{id}` path segment.
///
/// Every member is taken as entered, case kept and untrimmed (E-20). A missing
/// name or user type is the empty field, which the rule rejects with its
/// empty-field message (V-COUSR02C-005, V-COUSR02C-006, V-COUSR02C-008); a
/// missing password is not (E-21, see `password`).
#[derive(Clone, Default, PartialEq, Eq, Deserialize, ToSchema, Validate)]
pub struct UserUpdateRequest {
    /// `FNAMEI PIC X(20)`, the first name, in ASCII.
    #[serde(default)]
    #[schema(max_length = 20, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 20), custom(function = "crate::dto::ascii_text"))]
    pub first_name: Option<String>,
    /// `LNAMEI PIC X(20)`, the last name, in ASCII.
    #[serde(default)]
    #[schema(max_length = 20, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 20), custom(function = "crate::dto::ascii_text"))]
    pub last_name: Option<String>,
    /// `PASSWDI PIC X(8)`, the password in ASCII, as entered, case kept. Absent or
    /// `null`: the current password, unchanged and not re-hashed (E-21). An
    /// empty string is a password left blank, rejected with `Password can NOT
    /// be empty...` (V-COUSR02C-007). Any other value is compared with the
    /// stored password by argon2 verification and re-hashed when it differs.
    ///
    /// The current password is never returned, because `COUSR02C` would show
    /// it in this field [app/cbl/COUSR02C.cbl:L166-170] and a stored hash
    /// cannot be shown. An absent or `null` password therefore stands for that
    /// reloaded value, while an empty string is a field the user emptied; the
    /// two are never merged.
    // utoipa 5.5 has no field-level `description` attribute: this doc comment
    // is the published description of the member.
    #[serde(default)]
    #[schema(max_length = 8, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 8), custom(function = "crate::dto::ascii_text"))]
    pub password: Option<String>,
    /// `USRTYPEI PIC X(1)`, the user type: `A` for an administrator, `U` for a
    /// regular user. Any other non-blank ASCII value is stored as entered.
    #[serde(default)]
    #[schema(max_length = 1, pattern = "^[\\x00-\\x7F]*$")]
    #[validate(length(max = 1), custom(function = "crate::dto::ascii_text"))]
    pub usr_type: Option<String>,
}

impl fmt::Debug for UserUpdateRequest {
    /// Every member but the password, ended with `..` to show that one is
    /// left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserUpdateRequest")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("usr_type", &self.usr_type)
            .finish_non_exhaustive()
    }
}

impl UserUpdateRequest {
    /// Checks every member against its BMS field width and for ASCII, before
    /// `COUSR02C` `UPDATE-USER-INFO` compares it with the stored user and
    /// moves it into `SEC-USER-DATA` (P-COUSR02C-004 to P-COUSR02C-006);
    /// blank members pass, for the empty-field edits (V-COUSR02C-005 to
    /// V-COUSR02C-008). The `{id}` segment is checked apart, by
    /// `check_user_id_key` with [`UserUpdateField::UsrIdIn`].
    ///
    /// # Errors
    ///
    /// 400 with code `INVALID`, an empty message and one [`UserUpdateField`]
    /// per member longer than its field or holding a character above
    /// `U+007F`, in screen order.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate().map_err(|errors| {
            reject_invalid(&errors, UserUpdateField::of_request_member, OVER_LENGTH)
        })
    }

    /// The `COUSR2AI` fields of the rule for the user `user_id`, the `{id}`
    /// path segment: the names and type as entered, a missing one as the
    /// empty field, and the password as sent, `None` when absent (E-21).
    #[must_use]
    pub fn into_input(self, user_id: String) -> UserUpdateInput {
        UserUpdateInput {
            user_id,
            first_name: self.first_name.unwrap_or_default(),
            last_name: self.last_name.unwrap_or_default(),
            password: self.password,
            usr_type: self.usr_type.unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fmt::Debug;

    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use carddemo_domain::entities::UserSecurity;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{Alnum, UserId};
    use carddemo_domain::ports::Direction;
    use carddemo_domain::rules::user_add::UserAddInput;
    use carddemo_domain::rules::user_list::{self, UserListPage};
    use serde::Serialize;
    use serde::de::DeserializeOwned;
    use serde_json::{Map, Value, json};
    use utoipa::PartialSchema;

    use super::{
        UserAddField, UserAddRequest, UserDeleteField, UserListField, UserListQuery,
        UserMessageResponse, UserSummary, UserUpdateField, UserUpdateRequest, check_user_id_key,
    };
    use crate::dto::BmsField;
    use crate::error::ApiError;
    use crate::pagination::{Page, PageQuery};

    /// A made-up argon2id PHC string standing for a stored hash.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2g";

    /// A password text that must never reach `Debug` output.
    const REDACTED_TEXT: &str = "FAKEPW01";

    /// A `USRSEC` record holding [`HASH`], each text moved to its field.
    fn user(id: &str, first_name: &str, last_name: &str, usr_type: &str) -> UserSecurity {
        UserSecurity {
            sec_usr_id: UserId::move_from(id),
            sec_usr_fname: Alnum::move_from(first_name),
            sec_usr_lname: Alnum::move_from(last_name),
            password_hash: HASH.to_owned(),
            sec_usr_type: Alnum::move_from(usr_type),
        }
    }

    /// The error body as JSON.
    fn body(error: &ApiError) -> Value {
        serde_json::to_value(error.body()).expect("the error body serializes")
    }

    /// The 400 no-message body that names `fields`, in that order.
    fn over_length(fields: &[&str]) -> Value {
        let fields: Vec<Value> = fields
            .iter()
            .map(|field| json!({"field": field, "message": ""}))
            .collect();
        json!({"code": ErrorKind::Invalid.code(), "message": "", "fields": fields})
    }

    /// A request body with the one member `name` set to `value`.
    fn one_member(name: &str, value: String) -> Value {
        Value::Object(Map::from_iter([(name.to_owned(), Value::String(value))]))
    }

    /// The keys of a JSON object.
    fn keys(value: &Value) -> BTreeSet<String> {
        value
            .as_object()
            .expect("a JSON object")
            .keys()
            .cloned()
            .collect()
    }

    /// The outcome of `check` as an error body: `None` when it passes.
    fn rejection(outcome: Result<(), ApiError>) -> Option<Value> {
        outcome.err().map(|error| {
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            body(&error)
        })
    }

    /// Each variant serializes and deserializes as its BMS name, the schema
    /// lists exactly those names, and the ranks order them as `expected`.
    fn assert_bms_enum<F>(variants: &[F], expected: &[&str])
    where
        F: BmsField + Serialize + DeserializeOwned + PartialSchema + PartialEq + Debug,
    {
        for &field in variants {
            let name = field.as_str();
            assert_eq!(
                serde_json::to_value(field).expect("serializes"),
                json!(name)
            );
            let parsed: F = serde_json::from_value(json!(name)).expect("deserializes");
            assert_eq!(parsed, field);
        }
        let mut ranked = variants.to_vec();
        ranked.sort_by_key(|field| field.rank());
        let names: Vec<&str> = ranked.iter().map(|field| field.as_str()).collect();
        assert_eq!(names, expected);
        let ranks: BTreeSet<u16> = variants.iter().map(|field| field.rank()).collect();
        assert_eq!(ranks.len(), variants.len(), "ranks are distinct");

        let schema = serde_json::to_value(F::schema()).expect("the schema serializes");
        let listed: BTreeSet<&str> = schema["enum"]
            .as_array()
            .expect("a string enum")
            .iter()
            .map(|name| name.as_str().expect("a name"))
            .collect();
        assert_eq!(listed, expected.iter().copied().collect());
    }

    #[test]
    fn field_enums_carry_the_audit_bms_fields_in_screen_order() {
        assert_bms_enum(&[UserListField::UsrIdIn], &["USRIDIN"]);
        assert_bms_enum(
            &[
                UserAddField::UsrType,
                UserAddField::Passwd,
                UserAddField::UserId,
                UserAddField::LName,
                UserAddField::FName,
            ],
            &["FNAME", "LNAME", "USERID", "PASSWD", "USRTYPE"],
        );
        assert_bms_enum(
            &[
                UserUpdateField::UsrType,
                UserUpdateField::Passwd,
                UserUpdateField::LName,
                UserUpdateField::FName,
                UserUpdateField::UsrIdIn,
            ],
            &["USRIDIN", "FNAME", "LNAME", "PASSWD", "USRTYPE"],
        );
        assert_bms_enum(
            &[UserDeleteField::FName, UserDeleteField::UsrIdIn],
            &["USRIDIN", "FNAME"],
        );
        assert_eq!(UserAddField::UserId.as_str(), "USERID");
        assert_eq!(UserUpdateField::UsrIdIn.as_str(), "USRIDIN");
    }

    #[test]
    fn user_summary_has_exactly_the_four_columns_and_no_password() {
        let summary = UserSummary::from(&user("ADMIN001", "MARGARET", "GOLD", "A"));
        let json = serde_json::to_value(&summary).expect("serializes");
        assert_eq!(
            json,
            json!({
                "user_id": "ADMIN001",
                "first_name": "MARGARET",
                "last_name": "GOLD",
                "usr_type": "A",
            })
        );
        let columns: BTreeSet<String> = ["user_id", "first_name", "last_name", "usr_type"]
            .map(str::to_owned)
            .into();
        assert_eq!(keys(&json), columns);
        let schema = serde_json::to_value(UserSummary::schema()).expect("the schema serializes");
        assert_eq!(keys(&schema["properties"]), columns);
        let text = serde_json::to_string(&summary).expect("serializes");
        assert!(!text.contains(HASH), "{text}");
        assert!(!text.contains("argon2"), "{text}");
    }

    #[test]
    fn user_summary_trims_trailing_spaces_and_keeps_case() {
        let summary = UserSummary::from(&user("user1", " Ann", "Lee", "u"));
        assert_eq!(summary.user_id, "user1");
        assert_eq!(summary.first_name, " Ann");
        assert_eq!(summary.last_name, "Lee");
        assert_eq!(summary.usr_type, "u");
    }

    #[test]
    fn update_password_absent_null_and_empty_stay_distinct() {
        let cases = [
            (json!({"first_name": "A"}), None),
            (json!({"password": null}), None),
            (json!({"password": ""}), Some("")),
            (json!({"password": "pass1234"}), Some("pass1234")),
        ];
        for (sent, expected) in cases {
            let request: UserUpdateRequest =
                serde_json::from_value(sent.clone()).expect("deserializes");
            assert_eq!(request.password.as_deref(), expected, "{sent}");
            assert!(request.check().is_ok(), "{sent}");
            let input = request.into_input("USER0001".to_owned());
            assert_eq!(input.password.as_deref(), expected, "{sent}");
        }
    }

    #[test]
    fn add_rejects_each_member_over_its_width() {
        // (member, BMS width, BMS field)
        let members = [
            ("first_name", 20, "FNAME"),
            ("last_name", 20, "LNAME"),
            ("user_id", 8, "USERID"),
            ("password", 8, "PASSWD"),
            ("usr_type", 1, "USRTYPE"),
        ];
        for (member, width, field) in members {
            let at_width: UserAddRequest =
                serde_json::from_value(one_member(member, "x".repeat(width))).expect("parses");
            assert_eq!(rejection(at_width.check()), None, "{member}");
            let over: UserAddRequest =
                serde_json::from_value(one_member(member, "x".repeat(width + 1))).expect("parses");
            assert_eq!(
                rejection(over.check()),
                Some(over_length(&[field])),
                "{member}"
            );
        }

        let all_over = UserAddRequest {
            first_name: Some("F".repeat(21)),
            last_name: Some("L".repeat(21)),
            user_id: Some("USER00999".to_owned()),
            password: Some("pass12345".to_owned()),
            usr_type: Some("AU".to_owned()),
        };
        assert_eq!(
            rejection(all_over.check()),
            Some(over_length(&[
                "FNAME", "LNAME", "USERID", "PASSWD", "USRTYPE"
            ]))
        );
    }

    #[test]
    fn update_rejects_each_member_over_its_width() {
        // (member, BMS width, BMS field)
        let members = [
            ("first_name", 20, "FNAME"),
            ("last_name", 20, "LNAME"),
            ("password", 8, "PASSWD"),
            ("usr_type", 1, "USRTYPE"),
        ];
        for (member, width, field) in members {
            let at_width: UserUpdateRequest =
                serde_json::from_value(one_member(member, "x".repeat(width))).expect("parses");
            assert_eq!(rejection(at_width.check()), None, "{member}");
            let over: UserUpdateRequest =
                serde_json::from_value(one_member(member, "x".repeat(width + 1))).expect("parses");
            assert_eq!(
                rejection(over.check()),
                Some(over_length(&[field])),
                "{member}"
            );
        }

        let all_over = UserUpdateRequest {
            first_name: Some("F".repeat(21)),
            last_name: Some("L".repeat(21)),
            password: Some("pass12345".to_owned()),
            usr_type: Some("AU".to_owned()),
        };
        assert_eq!(
            rejection(all_over.check()),
            Some(over_length(&["FNAME", "LNAME", "PASSWD", "USRTYPE"]))
        );
    }

    /// Values within `width` characters that hold a character above `U+007F`:
    /// two-, three- and four-byte UTF-8 characters, alone and after ASCII.
    fn non_ascii_values(width: usize) -> Vec<String> {
        let mut values = vec![
            "\u{e9}".repeat(width),
            "\u{80}".to_owned(),
            "\u{20ac}".to_owned(),
            "\u{1f600}".to_owned(),
        ];
        if width > 1 {
            values.push(format!("{}\u{e9}", "A".repeat(width - 1)));
        }
        values
    }

    /// Values every member's check passes: blank in each COBOL sense (empty,
    /// spaces, low-values) and ASCII text at the width, so the rule still
    /// answers the blank ones with its empty-field message.
    fn ascii_values(width: usize) -> [String; 5] {
        [
            String::new(),
            " ".repeat(width),
            "\0".repeat(width),
            "x".repeat(width),
            "\u{7f}".to_owned(),
        ]
    }

    #[test]
    fn non_ascii_members_are_rejected_on_their_fields_and_ascii_ones_pass() {
        // (member, BMS width, field of the add, field of the update)
        let members = [
            ("first_name", 20, "FNAME", Some("FNAME")),
            ("last_name", 20, "LNAME", Some("LNAME")),
            ("user_id", 8, "USERID", None),
            ("password", 8, "PASSWD", Some("PASSWD")),
            ("usr_type", 1, "USRTYPE", Some("USRTYPE")),
        ];
        for (member, width, add_field, update_field) in members {
            for value in non_ascii_values(width) {
                let add: UserAddRequest =
                    serde_json::from_value(one_member(member, value.clone())).expect("parses");
                assert_eq!(
                    rejection(add.check()),
                    Some(over_length(&[add_field])),
                    "{member} {value:?}"
                );
                let update: UserUpdateRequest =
                    serde_json::from_value(one_member(member, value.clone())).expect("parses");
                assert_eq!(
                    rejection(update.check()),
                    update_field.map(|field| over_length(&[field])),
                    "{member} {value:?}"
                );
            }
            for value in ascii_values(width) {
                let add: UserAddRequest =
                    serde_json::from_value(one_member(member, value.clone())).expect("parses");
                assert_eq!(rejection(add.check()), None, "{member} {value:?}");
                let update: UserUpdateRequest =
                    serde_json::from_value(one_member(member, value.clone())).expect("parses");
                assert_eq!(rejection(update.check()), None, "{member} {value:?}");
            }
        }

        let add = UserAddRequest {
            first_name: Some("Ren\u{e9}e".to_owned()),
            last_name: Some("L".repeat(21)),
            user_id: Some("\u{e9}".repeat(8)),
            password: Some("p\u{e4}ssw\u{f6}rd".to_owned()),
            usr_type: Some("\u{c9}".to_owned()),
        };
        assert_eq!(
            rejection(add.check()),
            Some(over_length(&[
                "FNAME", "LNAME", "USERID", "PASSWD", "USRTYPE"
            ]))
        );
        let update = UserUpdateRequest {
            first_name: Some("Ren\u{e9}e".to_owned()),
            last_name: Some("Lee".to_owned()),
            password: Some("p\u{e4}ssw\u{f6}rd".to_owned()),
            usr_type: Some("\u{c9}".to_owned()),
        };
        assert_eq!(
            rejection(update.check()),
            Some(over_length(&["FNAME", "PASSWD", "USRTYPE"]))
        );
    }

    #[test]
    fn missing_members_pass_the_check_and_reach_the_rule_empty() {
        let add: UserAddRequest = serde_json::from_value(json!({})).expect("parses");
        assert!(add.check().is_ok());
        assert_eq!(add.into_input(), UserAddInput::default());

        let update: UserUpdateRequest = serde_json::from_value(json!({})).expect("parses");
        assert!(update.check().is_ok());
        let input = update.into_input("USER0001".to_owned());
        assert_eq!(input.user_id, "USER0001");
        assert_eq!(
            (
                input.first_name.as_str(),
                input.last_name.as_str(),
                input.usr_type.as_str()
            ),
            ("", "", "")
        );
        assert_eq!(input.password, None);
    }

    #[test]
    fn inputs_keep_every_member_as_entered() {
        let add: UserAddRequest = serde_json::from_value(json!({
            "first_name": " ann ",
            "last_name": "Lee",
            "user_id": "user0099",
            "password": "pass1234",
            "usr_type": "u",
        }))
        .expect("parses");
        let input = add.into_input();
        assert_eq!(input.first_name, " ann ");
        assert_eq!(input.last_name, "Lee");
        assert_eq!(input.user_id, "user0099");
        assert_eq!(input.password, "pass1234");
        assert_eq!(input.usr_type, "u");

        let update: UserUpdateRequest = serde_json::from_value(json!({
            "first_name": "Ann ",
            "last_name": " lee",
            "password": "Pass1234",
            "usr_type": "U",
        }))
        .expect("parses");
        let input = update.into_input("user0099".to_owned());
        assert_eq!(input.user_id, "user0099");
        assert_eq!(input.first_name, "Ann ");
        assert_eq!(input.last_name, " lee");
        assert_eq!(input.password.as_deref(), Some("Pass1234"));
        assert_eq!(input.usr_type, "U");
    }

    #[test]
    fn debug_leaves_the_password_out() {
        let add = UserAddRequest {
            user_id: Some("USER0099".to_owned()),
            password: Some(REDACTED_TEXT.to_owned()),
            ..UserAddRequest::default()
        };
        let update = UserUpdateRequest {
            first_name: Some("ANN".to_owned()),
            password: Some(REDACTED_TEXT.to_owned()),
            ..UserUpdateRequest::default()
        };
        let rendered = [format!("{add:?}"), format!("{update:?}")];
        for text in &rendered {
            assert!(!text.contains(REDACTED_TEXT), "{text}");
            assert!(!text.contains("password"), "{text}");
            assert!(text.ends_with(", .. }"), "{text}");
        }
        assert!(rendered[0].contains("\"USER0099\""), "{}", rendered[0]);
        assert!(rendered[1].contains("\"ANN\""), "{}", rendered[1]);
    }

    /// `check_user_id_key` for `key` with each enum's `USRIDIN`.
    fn key_outcomes(key: &str) -> [Option<Value>; 3] {
        [
            rejection(check_user_id_key(key, UserListField::UsrIdIn)),
            rejection(check_user_id_key(key, UserUpdateField::UsrIdIn)),
            rejection(check_user_id_key(key, UserDeleteField::UsrIdIn)),
        ]
    }

    #[test]
    fn user_id_key_over_eight_characters_or_not_ascii_is_rejected_on_the_given_field() {
        let accepted = [
            "",
            "        ",
            "\0\0\0\0\0\0\0\0",
            "ADMIN001",
            "user0001",
            "\u{7f}",
        ];
        for key in accepted {
            assert_eq!(key_outcomes(key), [None, None, None], "{key:?}");
        }
        let rejected = [
            "ADMIN0012",
            "         ",
            "\u{c9}\u{c9}\u{c9}\u{c9}\u{c9}\u{c9}\u{c9}\u{c9}\u{c9}",
            "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
            "USER000\u{e9}",
            "\u{80}",
            "\u{20ac}",
            "\u{1f600}",
        ];
        for key in rejected {
            let expected = Some(over_length(&["USRIDIN"]));
            assert_eq!(
                key_outcomes(key),
                [expected.clone(), expected.clone(), expected],
                "{key:?}"
            );
        }
        assert_eq!(
            rejection(check_user_id_key("USER00099", UserAddField::UserId)),
            Some(over_length(&["USERID"]))
        );
    }

    /// The query of `GET /admin/users?{text}` as `T`, as the list's query
    /// extractor reads it, or the 400 it answers.
    fn list_query<T: DeserializeOwned>(text: &str) -> Result<T, ApiError> {
        let uri: Uri = format!("/admin/users?{text}").parse().expect("a valid URI");
        Query::<T>::try_from_uri(&uri)
            .map(|Query(query)| query)
            .map_err(ApiError::from)
    }

    #[test]
    fn list_query_reads_and_resolves_as_the_shared_page_query() {
        // (query text, start key, direction, rows)
        let cases = [
            ("", None, Direction::Forward, user_list::SCREEN_ROWS),
            (
                "start_key=ADMIN001",
                Some("ADMIN001"),
                Direction::Forward,
                10,
            ),
            (
                "start_key=USER0001&direction=backward&page_size=5",
                Some("USER0001"),
                Direction::Backward,
                5,
            ),
            (
                "start_key=&direction=forward&page_size=100",
                Some(""),
                Direction::Forward,
                100,
            ),
        ];
        for (text, start_key, direction, rows) in cases {
            let query: UserListQuery = list_query(text).expect("deserializes");
            let shared: PageQuery = list_query(text).expect("deserializes");
            assert_eq!(PageQuery::from(query.clone()), shared, "{text}");
            assert_eq!(
                PageQuery::from(query).resolve(user_list::SCREEN_ROWS),
                (start_key.map(str::to_owned), direction, rows),
                "{text}"
            );
        }
        let malformed = [
            "direction=up",
            "page_size=0",
            "page_size=101",
            "page_size=x",
        ];
        for text in malformed {
            let error = list_query::<UserListQuery>(text).expect_err("rejected");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST, "{text}");
            assert_eq!(error.body().code, ErrorKind::Invalid.code(), "{text}");
        }
        // A percent-encoded non-ASCII key arrives decoded and is refused on
        // the search field.
        let query: UserListQuery =
            list_query("start_key=%C3%A9%C3%A9%C3%A9%C3%A9").expect("deserializes");
        let key = query.start_key.expect("a start key");
        assert_eq!(key, "\u{e9}\u{e9}\u{e9}\u{e9}");
        assert_eq!(
            rejection(check_user_id_key(&key, UserListField::UsrIdIn)),
            Some(over_length(&["USRIDIN"]))
        );
    }

    #[test]
    fn message_response_carries_the_summary_and_the_text_unchanged() {
        let record = user("user0099", "Ann", "Lee", "U");
        let message = messages::user_added_message("user0099");
        let response = UserMessageResponse::new(&record, message.clone());
        assert_eq!(
            serde_json::to_value(&response).expect("serializes"),
            json!({
                "user": {
                    "user_id": "user0099",
                    "first_name": "Ann",
                    "last_name": "Lee",
                    "usr_type": "U",
                },
                "message": message,
            })
        );
        assert!(
            response
                .message
                .starts_with("User user0099 has been added ...")
        );

        let unchanged = UserMessageResponse::new(&record, messages::PLEASE_MODIFY_TO_UPDATE);
        assert_eq!(unchanged.message, messages::PLEASE_MODIFY_TO_UPDATE);
        let text = serde_json::to_string(&unchanged).expect("serializes");
        assert!(!text.contains(HASH), "{text}");
    }

    #[test]
    fn list_page_converts_rows_keys_and_message() {
        let page = UserListPage {
            items: vec![
                user("ADMIN001", "MARGARET", "GOLD", "A"),
                user("USER1", "ANN", "LEE", "U"),
            ],
            next_key: Some(UserId::move_from("USER2")),
            prev_key: Some(UserId::move_from("ADMIN001")),
            message: Some(messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE),
        };
        assert_eq!(
            serde_json::to_value(Page::<UserSummary>::from(page)).expect("serializes"),
            json!({
                "items": [
                    {"user_id": "ADMIN001", "first_name": "MARGARET", "last_name": "GOLD", "usr_type": "A"},
                    {"user_id": "USER1", "first_name": "ANN", "last_name": "LEE", "usr_type": "U"},
                ],
                "next_key": "USER2",
                "prev_key": "ADMIN001",
                "message": messages::YOU_HAVE_REACHED_THE_BOTTOM_OF_THE_PAGE,
            })
        );

        let empty = UserListPage {
            items: Vec::new(),
            next_key: None,
            prev_key: None,
            message: None,
        };
        assert_eq!(
            serde_json::to_value(Page::<UserSummary>::from(empty)).expect("serializes"),
            json!({"items": [], "next_key": null, "prev_key": null})
        );
    }
}
