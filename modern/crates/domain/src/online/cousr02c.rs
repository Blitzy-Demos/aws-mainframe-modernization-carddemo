//! COUSR02C, user update: the rules of endpoint 18, `GET /api/v1/users/{user_id}`, the read
//! shared with COUSR03C, and of endpoint 19, `PUT /api/v1/users/{user_id}` (transaction `CU02`,
//! map `COUSR2A`; both admin-only, AAP 0.3.4.1).
//!
//! This module ports the `server` and `both` rows of `modern/audit/online/COUSR02C.md`: the
//! shared read (COUSR02C-010 to COUSR02C-012, COUSR02C-015, COUSR02C-016, which COUSR03C-010,
//! COUSR03C-013 and COUSR03C-014 reference), the update's empty-field edits (COUSR02C-018 to
//! COUSR02C-023), its re-read (COUSR02C-024, COUSR02C-025), the change test (COUSR02C-026 to
//! COUSR02C-028) and the rewrite's outcomes (COUSR02C-029 to COUSR02C-032). The token and admin
//! gate (COUSR02C-001, COUSR02C-002) belong to the API's extractors, and the `client` rows, F3,
//! F4, F12 and the read's prompt among them (COUSR02C-003 to COUSR02C-009, COUSR02C-013,
//! COUSR02C-014, COUSR02C-017, COUSR02C-033, COUSR02C-034), to the screen.
//!
//! # Flow
//!
//! `api::services::cousr02c` drives both endpoints; this module does no I/O.
//!
//! - Endpoint 18: [`edit_user_id`], the store's read of that key as a [`UserRead`], then
//!   [`read`], which answers with a [`UserView`] or the read's failure. COUSR03C's fetch is this
//!   same endpoint, because both programs' read paragraphs carry identical failure literals
//!   (AAP 0.2.3).
//! - Endpoint 19: [`edit_update`], the locked re-read of the row as a [`UserRead`] and
//!   [`reread`], then [`modified`] against the row and its `password_hash`, which yields the
//!   [`UserChanges`] to store or `Please modify to update ...`. The service hashes a changed
//!   password with `crate::password::hash`, rewrites the row and answers with
//!   [`rewrite_outcome`].
//!
//! Every outcome a request can reach is a value here, so the service only maps store results
//! into [`UserRead`] and [`RewriteResult`] and the handler only converts what comes back (Thin
//! Request Handlers). A failure is a [`UserMaintFailure`]: the literal moved to `WS-MESSAGE`,
//! the field the cursor goes to and the status class. Where one task sends the map twice, the
//! response is the last send, so endpoint 19 never returns the read's prompt (COUSR02C-024).
//!
//! # Input
//!
//! Every request string is read as an alphanumeric `MOVE` into its map field: its bytes
//! left-justified and padded with spaces to the `PIC X(n)` width, cut on the right beyond it,
//! never trimmed or upper-cased. A field is blank when, so padded, it is all spaces or all NUL
//! bytes, the `= SPACES OR LOW-VALUES` test; any other byte, a leading space included, makes it
//! non-blank. The values are compared and stored as they stand: no edit tests their content
//! beyond blankness (COUSR02C-023). The request DTO has already rejected a value that does not
//! fit its `PIC X(n)` field as printable ASCII bytes, `LOW-VALUES` aside, before this module
//! sees it, so the `MOVE` never cuts a byte of a request and a supplied password reaches
//! [`Password`] exactly as sent; this module does not repeat that check.
//!
//! # Password (`E27`)
//!
//! The COBOL pre-fills the dark password field from the record and compares the field with the
//! stored value to detect a change. The replacement never returns the password: an omitted one
//! stands for the pre-filled field, so it passes the empty edit and is no change; a supplied one
//! is held only as a [`Password`] and verified against the stored argon2id hash
//! (COUSR02C-021, COUSR02C-027). No [`UserView`], message, failure or `Debug` output here carries
//! a password or a hash (Cardholder Data Redaction), and the hash is only read, never returned.
//!
//! # Candidates awaiting review
//!
//! - `X-COUSR02C-1`: the read's prompt differs between COUSR02C and COUSR03C, so the shared
//!   endpoint returns none and each screen shows its own ([`UPDATE_PROMPT`]).
//! - Halt candidate `X-COUSR02C-H1`: an update whose re-read fails has no single COBOL result;
//!   [`reread`] answers with the read's own failure, which is not a parity result.
//! - `X-COUSR02C-4`: a stored hash that cannot be verified has no COBOL counterpart; [`modified`]
//!   treats it as the read failure.
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a value
//! or a [`UserMaintFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input).

use core::fmt;

use copybook::PicX;

use crate::password::verify;
use crate::records::user_security::UserSecurityRecord;
use crate::sensitive::Password;

/// The info prompt COUSR02C's read sends on success, in the neutral colour (`DFHNEUTR`;
/// COUSR02C-014, `COUSR02C.cbl:336-339`).
///
/// Endpoint 18 does not return it: the endpoint is shared with COUSR03C, whose read sends
/// `Press PF5 key to delete this user ...` instead, so the response carries no message
/// (COUSR02C-012) and the update screen shows this prompt after a 200 (`X-COUSR02C-1`).
pub const UPDATE_PROMPT: &str = "Press PF5 key to save your updates ...";

/// Blank user ID, on ENTER and on PF5 (COUSR02C-010, COUSR02C-018).
const USER_ID_EMPTY: &str = "User ID can NOT be empty...";

/// Blank first name (COUSR02C-019).
const FIRST_NAME_EMPTY: &str = "First Name can NOT be empty...";

/// Blank last name (COUSR02C-020).
const LAST_NAME_EMPTY: &str = "Last Name can NOT be empty...";

/// A supplied password that is blank (COUSR02C-021).
const PASSWORD_EMPTY: &str = "Password can NOT be empty...";

/// Blank user type (COUSR02C-022).
const USER_TYPE_EMPTY: &str = "User Type can NOT be empty...";

/// The read or the rewrite found no row (COUSR02C-015, COUSR02C-031).
const USER_NOT_FOUND: &str = "User ID NOT found...";

/// The read failed for any other reason (COUSR02C-016).
const LOOKUP_FAILED: &str = "Unable to lookup User...";

/// Nothing differs from the stored row (COUSR02C-028).
const NOT_MODIFIED: &str = "Please modify to update ...";

/// The rewrite failed for any reason but not-found (COUSR02C-032).
const UPDATE_FAILED: &str = "Unable to Update User...";

/// Head and tail of the success message around the user ID (COUSR02C-029).
const UPDATED_HEAD: &str = "User ";
const UPDATED_TAIL: &str = " has been updated ...";

/// The byte `STRING … DELIMITED BY SPACE` stops at (COUSR02C-030).
const SPACE: u8 = b' ';

/// The `LOW-VALUES` byte.
const LOW_VALUE: u8 = 0;

/// The request field a failure names: where the COBOL puts the cursor (`MOVE -1 TO <field>L`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserField {
    /// `USRIDIN`, the path `user_id`: a blank ID and a row not found (COUSR02C-010,
    /// COUSR02C-015, COUSR02C-018, COUSR02C-031).
    UserId,
    /// `FNAME`: a blank first name, and the cursor of a store failure and of the no-change
    /// message (COUSR02C-016, COUSR02C-019, COUSR02C-028, COUSR02C-032).
    FirstName,
    /// `LNAME`: a blank last name (COUSR02C-020).
    LastName,
    /// `PASSWD`: a blank supplied password (COUSR02C-021).
    Password,
    /// `USRTYPE`: a blank user type (COUSR02C-022).
    UserType,
}

impl UserField {
    /// The request field name the `ScreenError` `field` carries: `user_id`, `first_name`,
    /// `last_name`, `password` or `user_type`, as the audit's map-to-DTO correspondence names
    /// them.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::UserId => "user_id",
            Self::FirstName => "first_name",
            Self::LastName => "last_name",
            Self::Password => "password",
            Self::UserType => "user_type",
        }
    }
}

/// The status class of a [`UserMaintFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An edit of the request failed, or nothing was modified: 400.
    Input,
    /// The read or the store's write found no row: 404.
    NotFound,
    /// The read or the store's write failed: 500.
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400 for [`FailureClass::Input`], 404 for
    /// [`FailureClass::NotFound`], 500 for [`FailureClass::Store`].
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Store => 500,
        }
    }
}

/// A screen COUSR02C or COUSR03C sends instead of its result: the message, the field the
/// cursor goes to and the status class.
///
/// The message is the program's literal exactly as moved to `WS-MESSAGE`, trailing dots kept;
/// every one fits the 78-byte message row. No failure holds any request value, so none can
/// carry a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserMaintFailure {
    message: &'static str,
    field: UserField,
    class: FailureClass,
}

impl UserMaintFailure {
    /// A failed edit of the request or the no-change message: status class
    /// [`FailureClass::Input`].
    const fn input(message: &'static str, field: UserField) -> Self {
        Self {
            message,
            field,
            class: FailureClass::Input,
        }
    }

    /// `User ID NOT found...` on `user_id`, status class [`FailureClass::NotFound`]: the read
    /// found no row (COUSR02C-015, COUSR02C-025; COUSR03C-013), or the store's rewrite or delete
    /// affected none (COUSR02C-031; COUSR03C-021).
    #[must_use]
    pub const fn not_found() -> Self {
        Self {
            message: USER_NOT_FOUND,
            field: UserField::UserId,
            class: FailureClass::NotFound,
        }
    }

    /// `Unable to Update User...` on `first_name`, status class [`FailureClass::Store`]: the
    /// rewrite failed (COUSR02C-032), and COUSR03C's failed delete, which keeps the update
    /// literal (COUSR03C-018, COUSR03C-022).
    #[must_use]
    pub const fn update_failed() -> Self {
        Self {
            message: UPDATE_FAILED,
            field: UserField::FirstName,
            class: FailureClass::Store,
        }
    }

    /// `Unable to lookup User...` on `first_name`, status class [`FailureClass::Store`]
    /// (COUSR02C-016, COUSR02C-025; COUSR03C-014), and a stored hash that cannot be verified
    /// (`X-COUSR02C-4`).
    const fn lookup_failed() -> Self {
        Self {
            message: LOOKUP_FAILED,
            field: UserField::FirstName,
            class: FailureClass::Store,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field the cursor goes to.
    #[must_use]
    pub const fn field(&self) -> UserField {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for UserMaintFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for UserMaintFailure {}

/// Whether a padded field is `SPACES` or `LOW-VALUES`: all spaces or all NUL bytes.
fn is_blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == SPACE) || bytes.iter().all(|&byte| byte == LOW_VALUE)
}

/// `raw` moved into a `PIC X(N)` map field, or `message` on `field` when the field is blank.
fn required<const N: usize>(
    raw: &str,
    message: &'static str,
    field: UserField,
) -> Result<PicX<N>, UserMaintFailure> {
    let value = PicX::<N>::moved_from(raw.as_bytes());
    if is_blank(value.as_bytes()) {
        Err(UserMaintFailure::input(message, field))
    } else {
        Ok(value)
    }
}

/// The user ID of the path, `USRIDINI PIC X(8)`, as the key of the read: the received bytes
/// padded with spaces to 8, neither upper-cased nor trimmed of leading spaces, so `admin001`
/// does not find `ADMIN001` (COUSR02C-010, COUSR02C-011, COUSR02C-018; COUSR03C-010,
/// COUSR03C-016).
///
/// It is the first edit of ENTER and of PF5 in both programs, so a blank ID is rejected before
/// any other field is looked at and before any read.
///
/// # Errors
///
/// `User ID can NOT be empty...` on `user_id`, status class [`FailureClass::Input`], when the
/// padded ID is blank.
pub fn edit_user_id(raw: &str) -> Result<PicX<8>, UserMaintFailure> {
    required(raw, USER_ID_EMPTY, UserField::UserId)
}

/// The stored user as the update and the shared read see it: `SEC-USER-DATA` without
/// `SEC-USR-PWD` (CSUSR01Y-001 to CSUSR01Y-003, CSUSR01Y-005).
///
/// The service builds one from the row it read; the row's `password_hash` travels apart, to
/// [`modified`] only, so no value of this type can carry a password or a hash (`E27`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredUserView {
    /// `SEC-USR-ID PIC X(08)`, the key `users.usr_id`.
    pub user_id: PicX<8>,
    /// `SEC-USR-FNAME PIC X(20)`, padding kept.
    pub first_name: PicX<20>,
    /// `SEC-USR-LNAME PIC X(20)`, padding kept.
    pub last_name: PicX<20>,
    /// `SEC-USR-TYPE PIC X(01)`.
    pub user_type: PicX<1>,
}

/// The view of a `USRSEC` record: its ID, names and type, its password and `FILLER` left out
/// (CSUSR01Y-001 to CSUSR01Y-005).
impl From<&UserSecurityRecord> for StoredUserView {
    fn from(record: &UserSecurityRecord) -> Self {
        Self {
            user_id: record.id.clone(),
            first_name: record.fname.clone(),
            last_name: record.lname.clone(),
            user_type: record.usr_type.clone(),
        }
    }
}

/// What endpoint 18 answers for a user that has a row: `user_id`, `first_name`, `last_name` and
/// `user_type` from the row, and nothing else (COUSR02C-012).
///
/// It has no password, although COUSR02C moves `SEC-USR-PWD` into its dark field
/// (`COUSR02C.cbl:169`), and no message: each screen supplies its own program's prompt
/// ([`UPDATE_PROMPT`], `X-COUSR02C-1`). The response shows each value without its trailing
/// spaces, the trimmed view of [`PicX`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserView {
    /// `SEC-USR-ID`.
    pub user_id: PicX<8>,
    /// `SEC-USR-FNAME`.
    pub first_name: PicX<20>,
    /// `SEC-USR-LNAME`.
    pub last_name: PicX<20>,
    /// `SEC-USR-TYPE`.
    pub user_type: PicX<1>,
}

impl From<&StoredUserView> for UserView {
    fn from(stored: &StoredUserView) -> Self {
        Self {
            user_id: stored.user_id.clone(),
            first_name: stored.first_name.clone(),
            last_name: stored.last_name.clone(),
            user_type: stored.user_type.clone(),
        }
    }
}

/// The store's answer to a read of `users` by key: the `RESP` of `READ-USER-SEC-FILE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRead<'a> {
    /// `NORMAL`: the row, without its hash.
    Found(&'a StoredUserView),
    /// `NOTFND`: no row has the key.
    NotFound,
    /// `OTHER`: the read failed for any other reason, the database unreachable among them.
    Error,
}

/// The shared read of endpoint 18: the [`UserView`] of a found row (COUSR02C-012), or the read's
/// failure (COUSR02C-015, COUSR02C-016; COUSR03C-013, COUSR03C-014).
///
/// The COBOL reads `UPDATE`, but that hold ends with the task, so the read takes no lock and
/// writes nothing (COUSR02C-011).
///
/// # Errors
///
/// - `User ID NOT found...` on `user_id`, status class [`FailureClass::NotFound`], for
///   [`UserRead::NotFound`].
/// - `Unable to lookup User...` on `first_name`, status class [`FailureClass::Store`], for
///   [`UserRead::Error`]. The CICS codes the COBOL displays have no counterpart (halt H4).
pub fn read(result: UserRead<'_>) -> Result<UserView, UserMaintFailure> {
    reread(result).map(UserView::from)
}

/// The re-read of PF5, which holds the row for the rewrite: the stored row the update compares
/// with, as read now and not as the screen fetched it, so no changed-by-another-user check
/// exists (COUSR02C-024).
///
/// A failed re-read has no single COBOL result: the paragraph goes on to compare with record
/// fields the read never filled (halt candidate `X-COUSR02C-H1`, COUSR02C-025). The replacement
/// answers with the read's own failure, which is not claimed as parity, and writes nothing.
///
/// # Errors
///
/// The failures of [`read`]: `User ID NOT found...` (404) or `Unable to lookup User...` (500).
pub fn reread(result: UserRead<'_>) -> Result<&StoredUserView, UserMaintFailure> {
    match result {
        UserRead::Found(stored) => Ok(stored),
        UserRead::NotFound => Err(UserMaintFailure::not_found()),
        UserRead::Error => Err(UserMaintFailure::lookup_failed()),
    }
}

/// The body of endpoint 19, `UserUpdateRequest`, each value the raw text of its map field.
///
/// `password` is `None` when the request omits it: the dark field left as pre-filled, which
/// means unchanged (`E27`). An absent name or type is the empty text. `Debug` shows a supplied
/// password as `********`.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct UserUpdateInput<'a> {
    /// `FNAME`, `PIC X(20)`.
    pub first_name: &'a str,
    /// `LNAME`, `PIC X(20)`.
    pub last_name: &'a str,
    /// `PASSWD`, `PIC X(8)`, when supplied.
    pub password: Option<&'a str>,
    /// `USRTYPE`, `PIC X(1)`.
    pub user_type: &'a str,
}

impl fmt::Debug for UserUpdateInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserUpdateInput")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("password", &self.password.map(|_| "********"))
            .field("user_type", &self.user_type)
            .finish()
    }
}

/// An update whose fields passed the empty edits, padded to their map widths: what
/// `UPDATE-USER-INFO` compares with the stored row.
///
/// Only [`edit_update`] builds one, so every field is non-blank. A supplied password is a
/// [`Password`], whose `Debug` is redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedUpdate {
    user_id: PicX<8>,
    first_name: PicX<20>,
    last_name: PicX<20>,
    password: Option<Password>,
    user_type: PicX<1>,
}

impl ValidatedUpdate {
    /// The key the row is re-read and rewritten by (COUSR02C-024).
    #[must_use]
    pub const fn user_id(&self) -> &PicX<8> {
        &self.user_id
    }

    /// The first name, padded to 20.
    #[must_use]
    pub const fn first_name(&self) -> &PicX<20> {
        &self.first_name
    }

    /// The last name, padded to 20.
    #[must_use]
    pub const fn last_name(&self) -> &PicX<20> {
        &self.last_name
    }

    /// The supplied password, padded to 8, or `None` when it was omitted (`E27`).
    #[must_use]
    pub const fn password(&self) -> Option<&Password> {
        self.password.as_ref()
    }

    /// The user type.
    #[must_use]
    pub const fn user_type(&self) -> &PicX<1> {
        &self.user_type
    }
}

/// The empty-field edits of PF5, in the program's order: user ID, first name, last name,
/// password, user type; the first that fails is the answer, before any read (COUSR02C-018 to
/// COUSR02C-022).
///
/// An omitted password passes its edit, because it stands for the pre-filled dark field, which
/// holds the stored password, and a stored password is never blank (COUSR02C-021, `E27`). No
/// edit tests what a non-blank value holds: a user type other than `A` or `U`, or names in
/// lower case, go on to the comparison as received (COUSR02C-023).
///
/// # Errors
///
/// Status class [`FailureClass::Input`], on the field that is blank:
///
/// - `User ID can NOT be empty...` on `user_id`;
/// - `First Name can NOT be empty...` on `first_name`;
/// - `Last Name can NOT be empty...` on `last_name`;
/// - `Password can NOT be empty...` on `password`, for a supplied password only;
/// - `User Type can NOT be empty...` on `user_type`.
pub fn edit_update(
    user_id: &str,
    input: &UserUpdateInput<'_>,
) -> Result<ValidatedUpdate, UserMaintFailure> {
    let user_id = edit_user_id(user_id)?;
    let first_name = required(input.first_name, FIRST_NAME_EMPTY, UserField::FirstName)?;
    let last_name = required(input.last_name, LAST_NAME_EMPTY, UserField::LastName)?;
    let password = input
        .password
        .map(|raw| required(raw, PASSWORD_EMPTY, UserField::Password).map(Password::new))
        .transpose()?;
    let user_type = required(input.user_type, USER_TYPE_EMPTY, UserField::UserType)?;
    Ok(ValidatedUpdate {
        user_id,
        first_name,
        last_name,
        password,
        user_type,
    })
}

/// What the rewrite stores: the row's new ID, names and type, and the new password when it
/// changed (COUSR02C-026, COUSR02C-027, COUSR02C-029).
///
/// Only [`modified`] builds one, and only when at least one value differs from the stored row.
/// The names and type are the request's, which equal the stored ones where they did not change.
/// The service keeps the row's `filler` and, without a new password, its `password_hash`; with
/// one, it stores `crate::password::hash` over [`Password::expose`] in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserChanges {
    user_id: PicX<8>,
    first_name: PicX<20>,
    last_name: PicX<20>,
    user_type: PicX<1>,
    new_password: Option<Password>,
}

impl UserChanges {
    /// The key of the row to rewrite.
    #[must_use]
    pub const fn user_id(&self) -> &PicX<8> {
        &self.user_id
    }

    /// The first name to store.
    #[must_use]
    pub const fn first_name(&self) -> &PicX<20> {
        &self.first_name
    }

    /// The last name to store.
    #[must_use]
    pub const fn last_name(&self) -> &PicX<20> {
        &self.last_name
    }

    /// The user type to store.
    #[must_use]
    pub const fn user_type(&self) -> &PicX<1> {
        &self.user_type
    }

    /// The supplied password when it does not verify against the stored hash, to be hashed in
    /// its place; `None` when the password was omitted or matches, so the stored hash is kept.
    #[must_use]
    pub const fn new_password(&self) -> Option<&Password> {
        self.new_password.as_ref()
    }
}

/// The change test of `UPDATE-USER-INFO`: the edited update against the row the re-read holds
/// and that row's `password_hash` (COUSR02C-026 to COUSR02C-028).
///
/// - First name, last name and user type, each padded to its width, are compared byte for byte
///   with the stored field, case and leading spaces significant (COUSR02C-026).
/// - An omitted password is no change. A supplied one is verified against `password_hash` with
///   `crate::password::verify`, exactly as given, not upper-cased: a match is no change, a
///   mismatch is a change whose bytes [`UserChanges::new_password`] carries (COUSR02C-027,
///   `E26`, `E27`).
///
/// # Errors
///
/// - `Please modify to update ...` on `first_name`, status class [`FailureClass::Input`], when
///   no value differs (COUSR02C-028).
/// - `Unable to lookup User...` on `first_name`, status class [`FailureClass::Store`], when a
///   password is supplied and `password_hash` cannot be verified (`X-COUSR02C-4`).
pub fn modified(
    update: &ValidatedUpdate,
    stored: &StoredUserView,
    password_hash: &str,
) -> Result<UserChanges, UserMaintFailure> {
    let names_or_type_changed = update.first_name != stored.first_name
        || update.last_name != stored.last_name
        || update.user_type != stored.user_type;
    let new_password = match &update.password {
        None => None,
        Some(password) => match verify(password.expose(), password_hash) {
            Ok(true) => None,
            Ok(false) => Some(password.clone()),
            // X-COUSR02C-4: an 8-byte COBOL compare cannot fail to run, so no COBOL path
            // corresponds. As sign-on does (X-COSGN00C-2), an unverifiable stored hash is the
            // row that could not be read, never a changed password, so the admin learns the row
            // is unusable instead of having it silently overwritten.
            Err(_) => return Err(UserMaintFailure::lookup_failed()),
        },
    };
    if !names_or_type_changed && new_password.is_none() {
        return Err(UserMaintFailure::input(NOT_MODIFIED, UserField::FirstName));
    }
    Ok(UserChanges {
        user_id: update.user_id.clone(),
        first_name: update.first_name.clone(),
        last_name: update.last_name.clone(),
        user_type: update.user_type.clone(),
        new_password,
    })
}

/// The success message of a rewrite: `User `, the user ID up to its first space, and
/// ` has been updated ...`, shown in green (COUSR02C-029, COUSR02C-030).
///
/// `STRING … DELIMITED BY SPACE` takes the ID's bytes only up to its first space, so `AB CD`
/// gives `User AB has been updated ...` and an ID that starts with a space gives
/// `User  has been updated ...`, with two spaces.
#[must_use]
pub fn updated_message(user_id: &PicX<8>) -> String {
    let mut message = String::from(UPDATED_HEAD);
    message.extend(
        user_id
            .as_bytes()
            .iter()
            .take_while(|&&byte| byte != SPACE)
            .map(|&byte| char::from(byte)),
    );
    message.push_str(UPDATED_TAIL);
    message
}

/// The store's answer to the rewrite of a row: the `RESP` of `UPDATE-USER-SEC-FILE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RewriteResult {
    /// `NORMAL`: the row was rewritten and the transaction commits.
    Rewritten,
    /// `NOTFND`: the write affected no row.
    NotFound,
    /// `OTHER`: the write failed for any other reason; the transaction rolls back.
    Error,
}

/// The answer to endpoint 19 once the store has rewritten the row or failed to: the success
/// message (COUSR02C-029, COUSR02C-030), or the rewrite's failure (COUSR02C-031, COUSR02C-032).
///
/// # Errors
///
/// - `User ID NOT found...` on `user_id`, status class [`FailureClass::NotFound`], for
///   [`RewriteResult::NotFound`].
/// - `Unable to Update User...` on `first_name`, status class [`FailureClass::Store`], for
///   [`RewriteResult::Error`]. The CICS codes the COBOL displays have no counterpart (halt H4).
pub fn rewrite_outcome(
    user_id: &PicX<8>,
    result: RewriteResult,
) -> Result<String, UserMaintFailure> {
    match result {
        RewriteResult::Rewritten => Ok(updated_message(user_id)),
        RewriteResult::NotFound => Err(UserMaintFailure::not_found()),
        RewriteResult::Error => Err(UserMaintFailure::update_failed()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use copybook::PicX;

    use super::{
        FailureClass, RewriteResult, StoredUserView, UPDATE_PROMPT, UserChanges, UserField,
        UserMaintFailure, UserRead, UserUpdateInput, UserView, edit_update, edit_user_id, modified,
        read, reread, rewrite_outcome, updated_message,
    };
    use crate::password::{PasswordError, hash};
    use crate::records::user_security::UserSecurityRecord;
    use crate::sensitive::Password;

    type TestResult = Result<(), String>;

    /// A failure as the response shows it: message, field name and HTTP status.
    type Shown = (&'static str, &'static str, u16);

    // The audited literals, written out apart from the module's own constants.
    const USER_ID_EMPTY: &str = "User ID can NOT be empty...";
    const FIRST_NAME_EMPTY: &str = "First Name can NOT be empty...";
    const LAST_NAME_EMPTY: &str = "Last Name can NOT be empty...";
    const PASSWORD_EMPTY: &str = "Password can NOT be empty...";
    const USER_TYPE_EMPTY: &str = "User Type can NOT be empty...";
    const USER_NOT_FOUND: &str = "User ID NOT found...";
    const LOOKUP_FAILED: &str = "Unable to lookup User...";
    const NOT_MODIFIED: &str = "Please modify to update ...";
    const UPDATE_FAILED: &str = "Unable to Update User...";

    /// Twenty `LOW-VALUES` bytes: a name field that is blank without a single space.
    const NUL_NAME: &str = "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";

    /// The synthetic password the stored hash is computed over, as text and as its 8 bytes.
    const STORED_TEXT: &str = "TESTPW01";
    const STORED_BYTES: &[u8; 8] = b"TESTPW01";

    fn shown(failure: &UserMaintFailure) -> Shown {
        (
            failure.message(),
            failure.field().name(),
            failure.class().http_status(),
        )
    }

    fn failure_of<T>(result: Result<T, UserMaintFailure>) -> Option<Shown> {
        result.err().as_ref().map(shown)
    }

    fn text<const N: usize>(value: &str) -> PicX<N> {
        PicX::moved_from(value.as_bytes())
    }

    /// `USER0001` of the in-stream records (FIXUSR-006), as the store holds it.
    fn user0001() -> StoredUserView {
        StoredUserView {
            user_id: text("USER0001"),
            first_name: text("LAWRENCE"),
            last_name: text("THOMAS"),
            user_type: text("U"),
        }
    }

    /// The update of `USER0001` that repeats every stored value, with this password.
    fn unchanged(password: Option<&str>) -> UserUpdateInput<'_> {
        UserUpdateInput {
            first_name: "LAWRENCE",
            last_name: "THOMAS",
            password,
            user_type: "U",
        }
    }

    /// One argon2id hash of [`STORED_BYTES`], shared by every test because argon2 is slow in
    /// debug builds.
    fn stored_hash() -> Result<&'static str, String> {
        static HASH: OnceLock<Result<String, PasswordError>> = OnceLock::new();
        HASH.get_or_init(|| hash(STORED_BYTES))
            .as_deref()
            .map_err(ToString::to_string)
    }

    /// The edits and the change test of PF5 for `USER0001` against [`user0001`].
    fn changes(
        input: &UserUpdateInput<'_>,
        password_hash: &str,
    ) -> Result<UserChanges, UserMaintFailure> {
        edit_update("USER0001", input)
            .and_then(|update| modified(&update, &user0001(), password_hash))
    }

    // audit: COUSR02C-010
    // audit: COUSR02C-018
    // audit: COUSR03C-010
    // audit: COUSR03C-016
    #[test]
    fn a_blank_user_id_is_rejected_before_any_other_field() {
        let blank = (USER_ID_EMPTY, "user_id", 400);
        for raw in ["", " ", "        ", "\0\0\0\0\0\0\0\0"] {
            assert_eq!(failure_of(edit_user_id(raw)), Some(blank), "{raw:?}");
            let every_field_blank = edit_update(raw, &UserUpdateInput::default());
            assert_eq!(failure_of(every_field_blank), Some(blank), "{raw:?}");
        }
    }

    // audit: COUSR02C-011
    #[test]
    fn the_key_is_the_received_id_padded_to_eight() -> TestResult {
        let cases: [(&str, &[u8; 8]); 5] = [
            ("USER0001", b"USER0001"),
            ("admin001", b"admin001"),
            (" ADMIN", b" ADMIN  "),
            ("\0", b"\0       "),
            ("ADMIN0012", b"ADMIN001"),
        ];
        for (raw, key) in cases {
            let id = edit_user_id(raw).map_err(|failure| failure.to_string())?;
            assert_eq!(id.as_bytes(), key, "{raw:?}");
        }
        Ok(())
    }

    // audit: COUSR02C-012
    // audit: COUSR02C-015
    // audit: COUSR02C-016
    // audit: COUSR03C-013
    // audit: COUSR03C-014
    #[test]
    fn the_shared_read_answers_with_the_view_or_the_read_failure() -> TestResult {
        let stored = user0001();
        let view = read(UserRead::Found(&stored)).map_err(|failure| failure.to_string())?;
        let expected = UserView {
            user_id: text("USER0001"),
            first_name: text("LAWRENCE"),
            last_name: text("THOMAS"),
            user_type: text("U"),
        };
        assert_eq!(view, expected);
        assert_eq!(view.first_name.to_text(), "LAWRENCE");
        assert_eq!(
            failure_of(read(UserRead::NotFound)),
            Some((USER_NOT_FOUND, "user_id", 404))
        );
        assert_eq!(
            failure_of(read(UserRead::Error)),
            Some((LOOKUP_FAILED, "first_name", 500))
        );
        Ok(())
    }

    // audit: COUSR02C-012
    // audit: CSUSR01Y-004
    #[test]
    fn the_view_of_a_record_leaves_its_password_out() -> TestResult {
        let record = UserSecurityRecord {
            id: text("ADMIN001"),
            fname: text("MARGARET"),
            lname: text("GOLD"),
            pwd: Password::new(PicX::from_array(*STORED_BYTES)),
            usr_type: text("A"),
            filler: PicX::spaces(),
        };
        let stored = StoredUserView::from(&record);
        assert_eq!(stored.user_id.as_bytes(), b"ADMIN001");
        assert_eq!(stored.first_name, text::<20>("MARGARET"));
        assert_eq!(stored.last_name, text::<20>("GOLD"));
        assert_eq!(stored.user_type.as_bytes(), b"A");
        let view = read(UserRead::Found(&stored)).map_err(|failure| failure.to_string())?;
        assert!(!format!("{stored:?} {view:?}").contains(STORED_TEXT));
        Ok(())
    }

    // audit: COUSR02C-014
    // audit: COUSR03C-012
    #[test]
    fn the_update_prompt_is_cousr02c_s_own_literal() {
        assert_eq!(UPDATE_PROMPT, "Press PF5 key to save your updates ...");
    }

    // audit: COUSR02C-019
    // audit: COUSR02C-020
    // audit: COUSR02C-021
    // audit: COUSR02C-022
    #[test]
    fn the_empty_field_edits_run_in_program_order() {
        let full = unchanged(Some(STORED_TEXT));
        let first = (FIRST_NAME_EMPTY, "first_name", 400);
        let last = (LAST_NAME_EMPTY, "last_name", 400);
        let password = (PASSWORD_EMPTY, "password", 400);
        let user_type = (USER_TYPE_EMPTY, "user_type", 400);
        let cases: [(UserUpdateInput<'_>, Shown); 7] = [
            (UserUpdateInput::default(), first),
            (
                UserUpdateInput {
                    first_name: NUL_NAME,
                    ..full
                },
                first,
            ),
            (
                UserUpdateInput {
                    last_name: "   ",
                    password: Some(""),
                    user_type: "",
                    ..full
                },
                last,
            ),
            (
                UserUpdateInput {
                    password: Some("        "),
                    user_type: "",
                    ..full
                },
                password,
            ),
            (
                UserUpdateInput {
                    password: Some(""),
                    ..full
                },
                password,
            ),
            (
                UserUpdateInput {
                    user_type: " ",
                    ..full
                },
                user_type,
            ),
            (
                UserUpdateInput {
                    user_type: "",
                    ..unchanged(None)
                },
                user_type,
            ),
        ];
        for (input, expected) in cases {
            let result = edit_update("USER0001", &input);
            assert_eq!(failure_of(result), Some(expected), "{input:?}");
        }
    }

    // audit: COUSR02C-021
    // audit: COUSR02C-023
    #[test]
    fn values_pass_as_received_and_an_omitted_password_passes() -> TestResult {
        let omitted = edit_update("USER0001", &unchanged(None)).map_err(|f| f.to_string())?;
        assert_eq!(omitted.password(), None);

        let odd = UserUpdateInput {
            first_name: "lawrence 2",
            last_name: " thomas",
            password: Some("ab"),
            user_type: "x",
        };
        let update = edit_update("user0001", &odd).map_err(|f| f.to_string())?;
        assert_eq!(update.user_id().as_bytes(), b"user0001");
        assert_eq!(update.first_name().as_bytes(), b"lawrence 2          ");
        assert_eq!(update.last_name().as_bytes(), b" thomas             ");
        assert_eq!(update.user_type().as_bytes(), b"x");
        assert!(update.password().map(Password::expose) == Some(b"ab      "));

        let long = UserUpdateInput {
            password: Some("TESTPW0123"),
            user_type: "AU",
            ..odd
        };
        let cut = edit_update("user0001", &long).map_err(|f| f.to_string())?;
        assert!(cut.password().map(Password::expose) == Some(STORED_BYTES));
        assert_eq!(cut.user_type().as_bytes(), b"A");
        Ok(())
    }

    // audit: COUSR02C-027
    // audit: COUSR02C-028
    #[test]
    fn an_omitted_or_matching_password_is_no_change() -> TestResult {
        let password_hash = stored_hash()?;
        let nothing = (NOT_MODIFIED, "first_name", 400);
        let omitted = changes(&unchanged(None), password_hash);
        assert_eq!(failure_of(omitted), Some(nothing));
        let matching = changes(&unchanged(Some(STORED_TEXT)), password_hash);
        assert_eq!(failure_of(matching), Some(nothing));
        Ok(())
    }

    // audit: COUSR02C-027
    #[test]
    fn a_password_that_does_not_verify_is_a_change_carrying_its_bytes() -> TestResult {
        let password_hash = stored_hash()?;
        // `testpw01` differs only in case: it is kept as supplied, not upper-cased (E26).
        let supplied: [(&str, &[u8; 8]); 2] =
            [("TESTPW02", b"TESTPW02"), ("testpw01", b"testpw01")];
        for (raw, bytes) in supplied {
            let changed =
                changes(&unchanged(Some(raw)), password_hash).map_err(|f| f.to_string())?;
            assert!(
                changed.new_password().map(Password::expose) == Some(bytes),
                "the new password is the supplied bytes"
            );
            assert_eq!(changed.user_id(), &text::<8>("USER0001"));
            assert_eq!(changed.first_name(), &text::<20>("LAWRENCE"));
            assert_eq!(changed.last_name(), &text::<20>("THOMAS"));
            assert_eq!(changed.user_type(), &text::<1>("U"));
            assert!(!format!("{changed:?}").contains(raw));
        }
        Ok(())
    }

    // audit: COUSR02C-026
    #[test]
    fn names_and_type_are_compared_byte_for_byte() -> TestResult {
        // No password is supplied, so the hash is never verified and its text plays no part.
        let unverifiable = "not-a-phc";
        let cases = [
            UserUpdateInput {
                first_name: "MARGARET",
                ..unchanged(None)
            },
            UserUpdateInput {
                first_name: "lawrence",
                ..unchanged(None)
            },
            UserUpdateInput {
                first_name: " LAWRENCE",
                ..unchanged(None)
            },
            UserUpdateInput {
                last_name: "THOMAS-SMITH",
                ..unchanged(None)
            },
            UserUpdateInput {
                user_type: "A",
                ..unchanged(None)
            },
        ];
        for input in cases {
            let changed = changes(&input, unverifiable).map_err(|f| f.to_string())?;
            assert_eq!(changed.user_id(), &text::<8>("USER0001"));
            assert_eq!(changed.first_name(), &text::<20>(input.first_name));
            assert_eq!(changed.last_name(), &text::<20>(input.last_name));
            assert_eq!(changed.user_type(), &text::<1>(input.user_type));
            assert_eq!(changed.new_password(), None);
        }
        // Trailing spaces are the field's padding, so they change nothing.
        let padded = UserUpdateInput {
            first_name: "LAWRENCE   ",
            ..unchanged(None)
        };
        assert_eq!(
            failure_of(changes(&padded, unverifiable)),
            Some((NOT_MODIFIED, "first_name", 400))
        );
        Ok(())
    }

    // audit: COUSR02C-027
    #[test]
    fn an_unverifiable_stored_hash_is_the_read_failure() {
        let lookup = (LOOKUP_FAILED, "first_name", 500);
        let renamed = UserUpdateInput {
            first_name: "MARGARET",
            ..unchanged(Some(STORED_TEXT))
        };
        for stored in ["", "not-a-phc", "$argon2id$v=19$m=19456,t=2,p=1"] {
            let same = changes(&unchanged(Some(STORED_TEXT)), stored);
            assert_eq!(failure_of(same), Some(lookup), "{stored:?}");
            assert_eq!(
                failure_of(changes(&renamed, stored)),
                Some(lookup),
                "{stored:?}"
            );
        }
    }

    // audit: COUSR02C-024
    // audit: COUSR02C-025
    #[test]
    fn the_re_read_yields_the_row_or_the_read_failure() -> TestResult {
        let stored = user0001();
        let row = reread(UserRead::Found(&stored)).map_err(|f| f.to_string())?;
        assert!(core::ptr::eq(row, &stored));
        assert_eq!(
            failure_of(reread(UserRead::NotFound)),
            Some((USER_NOT_FOUND, "user_id", 404))
        );
        assert_eq!(
            failure_of(reread(UserRead::Error)),
            Some((LOOKUP_FAILED, "first_name", 500))
        );
        Ok(())
    }

    // audit: COUSR02C-029
    // audit: COUSR02C-031
    // audit: COUSR02C-032
    #[test]
    fn the_rewrite_answers_with_the_message_or_its_failure() {
        let id = text::<8>("USER0001");
        assert_eq!(
            rewrite_outcome(&id, RewriteResult::Rewritten),
            Ok("User USER0001 has been updated ...".to_owned())
        );
        assert_eq!(
            failure_of(rewrite_outcome(&id, RewriteResult::NotFound)),
            Some((USER_NOT_FOUND, "user_id", 404))
        );
        assert_eq!(
            failure_of(rewrite_outcome(&id, RewriteResult::Error)),
            Some((UPDATE_FAILED, "first_name", 500))
        );
    }

    // audit: COUSR02C-029
    // audit: COUSR02C-030
    #[test]
    fn the_message_takes_the_id_up_to_its_first_space() {
        for (id, message) in [
            ("ADMIN001", "User ADMIN001 has been updated ..."),
            ("ADMIN", "User ADMIN has been updated ..."),
            ("AB CD", "User AB has been updated ..."),
            (" AB", "User  has been updated ..."),
        ] {
            assert_eq!(updated_message(&text(id)), message, "{id:?}");
        }
    }

    // audit: COUSR03C-021
    // audit: COUSR03C-022
    #[test]
    fn the_shared_store_failures_carry_their_literals() {
        assert_eq!(
            shown(&UserMaintFailure::not_found()),
            (USER_NOT_FOUND, "user_id", 404)
        );
        assert_eq!(
            shown(&UserMaintFailure::update_failed()),
            (UPDATE_FAILED, "first_name", 500)
        );
    }

    #[test]
    fn fields_statuses_and_display() {
        for (field, name) in [
            (UserField::UserId, "user_id"),
            (UserField::FirstName, "first_name"),
            (UserField::LastName, "last_name"),
            (UserField::Password, "password"),
            (UserField::UserType, "user_type"),
        ] {
            assert_eq!(field.name(), name);
        }
        assert_eq!(FailureClass::Input.http_status(), 400);
        assert_eq!(FailureClass::NotFound.http_status(), 404);
        assert_eq!(FailureClass::Store.http_status(), 500);
        assert_eq!(UserMaintFailure::not_found().to_string(), USER_NOT_FOUND);
        let error: &dyn std::error::Error = &UserMaintFailure::update_failed();
        assert!(error.source().is_none());
    }

    // audit: CSUSR01Y-004
    #[test]
    fn debug_output_never_shows_a_password() -> TestResult {
        let input = UserUpdateInput {
            password: Some("TESTPW02"),
            ..unchanged(None)
        };
        let update = edit_update("USER0001", &input).map_err(|f| f.to_string())?;
        let rendered = format!("{input:?} {update:?}");
        assert!(!rendered.contains("TESTPW02"));
        assert!(rendered.contains("********"));
        assert!(format!("{:?}", unchanged(None)).contains("password: None"));
        Ok(())
    }
}
