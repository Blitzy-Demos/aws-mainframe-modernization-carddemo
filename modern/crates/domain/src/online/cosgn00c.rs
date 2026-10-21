//! COSGN00C, sign-on: the rules of endpoint 1, `POST /api/v1/signon` (transaction `CC00`, map
//! `COSGN0A`; AAP 0.3.4.1, 0.3.4.6).
//!
//! This module ports the `server` rows of `modern/audit/online/COSGN00C.md`:
//!
//! | Rows | Paragraph | Implemented by |
//! |---|---|---|
//! | COSGN00C-003 | `PROCESS-ENTER-KEY` | [`edit`]: each input read as its 8-byte map field |
//! | COSGN00C-006 | `MAIN-PARA` | the whole module: a request is decided from its own credentials, and nothing is kept between requests, so there is no attempt count, lockout or delay |
//! | COSGN00C-007, COSGN00C-008 | `PROCESS-ENTER-KEY` | [`edit`]: the blank-field edits, user ID first |
//! | COSGN00C-009, COSGN00C-010 | `PROCESS-ENTER-KEY` | [`edit`]: both values upper-cased before the read (`E26`) |
//! | COSGN00C-013, COSGN00C-018 to COSGN00C-020 | `READ-USER-SEC-FILE` | [`decide`]: the password check and the read outcomes |
//! | COSGN00C-014 to COSGN00C-016 | `READ-USER-SEC-FILE` | [`SignedOn`]: the COMMAREA identity and the `XCTL` target |
//!
//! The `client` rows send no request and belong to the `COSGN00` screen: the first entry, F3 and
//! every other key, the redisplay after an error, the header and the navigation after success
//! (COSGN00C-001, COSGN00C-002, COSGN00C-004, COSGN00C-005, COSGN00C-011, COSGN00C-012,
//! COSGN00C-017).
//!
//! # Flow
//!
//! `api::services::cosgn00c` calls [`edit`] on the two request fields, reads the `users` row
//! whose `usr_id` is [`SignonKey::user_id`] (the COBOL `READ DATASET('USRSEC  ')
//! RIDFLD(WS-USER-ID) KEYLENGTH(8)`), states the read's result as a [`UserLookup`], calls
//! [`decide`], and issues the token, claims `sub` and `usr_type`, from the [`SignedOn`] value.
//! This module does no I/O and holds no token logic.
//!
//! Every outcome is complete: a [`SignonFailure`] names its message literal, the request field
//! the COBOL puts the cursor on and its status class; a [`SignedOn`] carries the identity and the
//! route. The API maps them without deciding anything (Thin Request Handlers).
//!
//! # Identity
//!
//! [`SignedOn`] is the only acting identity the API ever trusts (AAP 0.5.1): `CDEMO-USER-ID`
//! becomes the upper-cased user ID that was read, and `CDEMO-USER-TYPE` the stored
//! `SEC-USR-TYPE`, unchanged (COSGN00C-014, COCOM01Y-005, COCOM01Y-006). Nothing here takes an
//! identity from a request field other than the two credentials. [`UserType`] is the workspace's
//! single user-type type.
//!
//! # Input outside a 3270 field
//!
//! A JSON string can carry what an 8-byte `USERIDI` or `PASSWDI` cannot: more than 8 characters,
//! control characters, or characters outside ASCII. Following escalation candidate
//! `X-COSGN00C-3`, each field accepts at most 8 bytes of printable ASCII (`0x20` to `0x7E`, the
//! bytes a stored `USRSEC` field holds: PIC-001, PIC-002), and any other value is rejected as a
//! request-validation failure naming its field, before the program's own edits run, as the
//! request DTO rejects an over-length value (COSGN00-027, COSGN00-031). That rejection is no
//! COBOL result and claims no parity. An empty field is the unmodified field (`LOW-VALUES`) and
//! reaches the blank-field edits.
//!
//! # Redaction and totality
//!
//! The password is held only as a [`Password`], reached through [`Password::expose`] solely for
//! `crate::password::verify`. No failure, message or `Debug` output carries the password, the
//! stored hash or any input text (Cardholder Data Redaction, PIC-024). [`edit`] and [`decide`]
//! are total: any text gives a value or a [`SignonFailure`], with no indexing, unwrapping or panic
//! (Total Parsers Over Untrusted Input).

use core::fmt;

use copybook::PicX;

use crate::password;
use crate::sensitive::Password;

/// Width of the two inputs: `USERIDI` and `PASSWDI PIC X(8)` (`COSGN00.CPY`), `WS-USER-ID` and
/// `WS-USER-PWD PIC X(08)`, and the key and password of `SEC-USER-DATA` (CSUSR01Y-001,
/// CSUSR01Y-004).
const FIELD_WIDTH: usize = 8;

/// `CDEMO-USRTYP-ADMIN VALUE 'A'` (COCOM01Y-006).
const ADMIN_CODE: u8 = b'A';

/// `CDEMO-USRTYP-USER VALUE 'U'` (COCOM01Y-006).
const USER_CODE: u8 = b'U';

/// The user ID is blank (COSGN00C-007).
pub const ENTER_USER_ID: &str = "Please enter User ID ...";

/// The user ID is given and the password is blank (COSGN00C-008).
pub const ENTER_PASSWORD: &str = "Please enter Password ...";

/// The user exists and the password does not verify (COSGN00C-018).
pub const WRONG_PASSWORD: &str = "Wrong Password. Try again ...";

/// No user has the upper-cased ID, the read's RESP 13 (COSGN00C-019).
pub const USER_NOT_FOUND: &str = "User not found. Try again ...";

/// Any other read outcome, or a stored hash that cannot be verified (COSGN00C-020).
pub const UNABLE_TO_VERIFY: &str = "Unable to verify the User ...";

/// A user ID that no 8-byte map field can carry (COSGN00C-003, `X-COSGN00C-3`). The request
/// validation's text, not a COBOL literal; the reviewer settles it with that candidate.
pub const USER_ID_REJECTED: &str = "User ID must be at most 8 printable ASCII characters";

/// A password that no 8-byte map field can carry (COSGN00C-003, `X-COSGN00C-3`). The request
/// validation's text, not a COBOL literal; the reviewer settles it with that candidate.
pub const PASSWORD_REJECTED: &str = "Password must be at most 8 printable ASCII characters";

/// The user type of a signed-on user, `CDEMO-USER-TYPE PIC X(01)` (COCOM01Y-006), which sign-on
/// copies from the stored `SEC-USR-TYPE` (CSUSR01Y-005; COSGN00C-014).
///
/// The two 88-levels are tested exactly: [`UserType::is_admin`] is `CDEMO-USRTYP-ADMIN VALUE 'A'`
/// and [`UserType::is_user`] is `CDEMO-USRTYP-USER VALUE 'U'`. A stored type that is neither, a
/// space or a lower-case `a` for instance, is carried unchanged and is neither; sign-on still
/// routes it to the main menu, because COSGN00C tests only `CDEMO-USRTYP-ADMIN` (COSGN00C-016).
///
/// This is the one user-type type of the workspace: the JWT claim `usr_type`, the `api::auth`
/// extractors and the online rules that depend on the user type use it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserType(PicX<1>);

impl UserType {
    /// `A`, the type `CDEMO-USRTYP-ADMIN` names.
    pub const ADMIN: Self = Self(PicX::from_array([ADMIN_CODE]));

    /// `U`, the type `CDEMO-USRTYP-USER` names.
    pub const USER: Self = Self(PicX::from_array([USER_CODE]));

    /// The user type held in a stored `SEC-USR-TYPE` or a verified `usr_type` claim, its byte
    /// kept verbatim.
    #[must_use]
    pub const fn from_code(code: PicX<1>) -> Self {
        Self(code)
    }

    /// The `PIC X(01)` value: what the `usr_type` claim and the sign-on response carry.
    #[must_use]
    pub const fn code(&self) -> &PicX<1> {
        &self.0
    }

    /// `CDEMO-USRTYP-ADMIN`: the type is `A`, which transfers sign-on to the admin menu
    /// (COSGN00C-015) and passes the API's admin gate.
    #[must_use]
    pub const fn is_admin(&self) -> bool {
        matches!(self.0.as_bytes(), [ADMIN_CODE])
    }

    /// `CDEMO-USRTYP-USER`: the type is `U`. A type that is neither `A` nor `U` is not a user by
    /// this test, and is still no administrator (COSGN00C-016).
    #[must_use]
    pub const fn is_user(&self) -> bool {
        matches!(self.0.as_bytes(), [USER_CODE])
    }
}

/// Where a successful sign-on goes: the program `READ-USER-SEC-FILE` transfers to with `XCTL`,
/// as the frontend route of its screen (`COSGN00C.cbl:230-240`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NextRoute {
    /// `XCTL PROGRAM('COADM01C')`, the admin menu (COSGN00C-015).
    Admin,
    /// `XCTL PROGRAM('COMEN01C')`, the main menu (COSGN00C-016).
    Menu,
}

impl NextRoute {
    /// [`NextRoute::Admin`] when `user_type` is `CDEMO-USRTYP-ADMIN`, [`NextRoute::Menu`] for
    /// every other type, `U`, a space or any other byte (COSGN00C-015, COSGN00C-016).
    #[must_use]
    pub const fn for_user_type(user_type: &UserType) -> Self {
        if user_type.is_admin() {
            Self::Admin
        } else {
            Self::Menu
        }
    }

    /// The route of the screen: `/admin` (COADM01) or `/menu` (COMEN01).
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Admin => "/admin",
            Self::Menu => "/menu",
        }
    }
}

/// The request field a failure names: where the COBOL puts the cursor (`MOVE -1 TO USERIDL` or
/// `MOVE -1 TO PASSWDL`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignonField {
    /// `USERID`, request field `user_id`.
    UserId,
    /// `PASSWD`, request field `password`.
    Password,
}

impl SignonField {
    /// The request field's name, as `ScreenError.field` carries it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::UserId => "user_id",
            Self::Password => "password",
        }
    }
}

/// The status class of a [`SignonFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// The request's input was rejected: 400 (COSGN00C-003, COSGN00C-007, COSGN00C-008).
    Input,
    /// The credentials did not sign on: 401 (COSGN00C-018, COSGN00C-019).
    Signon,
    /// The user could not be read or verified: 500 (COSGN00C-020).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns: 400, 401 or 500.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::Signon => 401,
            Self::Store => 500,
        }
    }
}

/// A sign-on screen COSGN00C sends instead of transferring: which message, which field and
/// which status class. No variant holds any part of the input, the password or the stored hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignonFailure {
    /// The field holds more than 8 bytes or a byte outside printable ASCII, which no 3270 field
    /// carries (COSGN00C-003, `X-COSGN00C-3`). No COBOL result and no parity claim.
    Rejected(SignonField),
    /// [`ENTER_USER_ID`], field `user_id`, 400 (COSGN00C-007).
    UserIdBlank,
    /// [`ENTER_PASSWORD`], field `password`, 400 (COSGN00C-008).
    PasswordBlank,
    /// [`WRONG_PASSWORD`], field `password`, 401 (COSGN00C-018).
    WrongPassword,
    /// [`USER_NOT_FOUND`], field `user_id`, 401 (COSGN00C-019).
    UserNotFound,
    /// [`UNABLE_TO_VERIFY`], field `user_id`, 500 (COSGN00C-020).
    UnableToVerify,
}

impl SignonFailure {
    /// The message, exactly as the program moves it to `WS-MESSAGE`, without the field's
    /// trailing pad spaces.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Rejected(SignonField::UserId) => USER_ID_REJECTED,
            Self::Rejected(SignonField::Password) => PASSWORD_REJECTED,
            Self::UserIdBlank => ENTER_USER_ID,
            Self::PasswordBlank => ENTER_PASSWORD,
            Self::WrongPassword => WRONG_PASSWORD,
            Self::UserNotFound => USER_NOT_FOUND,
            Self::UnableToVerify => UNABLE_TO_VERIFY,
        }
    }

    /// The field the cursor goes to, which `ScreenError.field` names.
    #[must_use]
    pub const fn field(self) -> SignonField {
        match self {
            Self::Rejected(field) => field,
            Self::UserIdBlank | Self::UserNotFound | Self::UnableToVerify => SignonField::UserId,
            Self::PasswordBlank | Self::WrongPassword => SignonField::Password,
        }
    }

    /// The status class.
    #[must_use]
    pub const fn class(self) -> FailureClass {
        match self {
            Self::Rejected(_) | Self::UserIdBlank | Self::PasswordBlank => FailureClass::Input,
            Self::WrongPassword | Self::UserNotFound => FailureClass::Signon,
            Self::UnableToVerify => FailureClass::Store,
        }
    }
}

/// The message only.
impl fmt::Display for SignonFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for SignonFailure {}

/// The two inputs after the edits of `PROCESS-ENTER-KEY`: what `READ-USER-SEC-FILE` reads by and
/// compares (`WS-USER-ID` and `WS-USER-PWD`).
///
/// Only [`edit`] builds one, so both values are 8 bytes of printable ASCII, neither is blank and
/// both are upper-cased (COSGN00C-003, COSGN00C-009). `Debug` shows the user ID and `********`
/// for the password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignonKey {
    user_id: PicX<FIELD_WIDTH>,
    password: Password,
}

impl SignonKey {
    /// `WS-USER-ID`: the upper-cased user ID, space-padded to 8 bytes and never trimmed, the key
    /// of the `users` read (`RIDFLD(WS-USER-ID) KEYLENGTH(8)`) and the `CDEMO-USER-ID` of a
    /// successful sign-on (COSGN00C-009, COSGN00C-014).
    #[must_use]
    pub const fn user_id(&self) -> &PicX<FIELD_WIDTH> {
        &self.user_id
    }

    /// `WS-USER-PWD`: the upper-cased password, space-padded to 8 bytes, which [`decide`]
    /// verifies against the stored hash (COSGN00C-009, COSGN00C-013).
    #[must_use]
    pub const fn password(&self) -> &Password {
        &self.password
    }
}

/// The edits `PROCESS-ENTER-KEY` applies to the user ID and password of an ENTER, in program
/// order (COSGN00C-003, COSGN00C-007 to COSGN00C-010).
///
/// 1. Each value is read as its 8-byte map field: left-justified and padded with spaces, never
///    trimmed, so a leading space stays part of it (COSGN00C-003). A value of more than 8 bytes,
///    or with a byte outside printable ASCII, is rejected first, the user ID before the password,
///    as the request validation rejects it before the program runs (`X-COSGN00C-3`).
/// 2. A blank user ID (empty or all spaces) is [`SignonFailure::UserIdBlank`] whatever the
///    password holds; then a blank password is [`SignonFailure::PasswordBlank`] (COSGN00C-007,
///    COSGN00C-008).
/// 3. Both values are upper-cased, `a` to `z` becoming `A` to `Z` and every other byte left
///    unchanged (`FUNCTION UPPER-CASE`, COSGN00C-009). The password is compared upper-cased
///    while COUSR01C stores it as typed, so a stored password holding a lower-case letter never
///    verifies; that is preserved (`E26`). After a blank-field error the COBOL still upper-cases,
///    to no effect, and reads nothing (COSGN00C-010).
///
/// # Errors
///
/// [`SignonFailure::Rejected`] for a value no 8-byte field carries, then
/// [`SignonFailure::UserIdBlank`] or [`SignonFailure::PasswordBlank`]; each is class
/// [`FailureClass::Input`].
pub fn edit(user_id: &str, password: &str) -> Result<SignonKey, SignonFailure> {
    // Both fields are checked before either blank edit: the request validation that rejects a
    // value no 3270 field carries runs before the program does (X-COSGN00C-3).
    let user_id = map_field(user_id, SignonField::UserId)?;
    let password = map_field(password, SignonField::Password)?;
    if user_id.is_spaces() {
        return Err(SignonFailure::UserIdBlank);
    }
    if password.is_spaces() {
        return Err(SignonFailure::PasswordBlank);
    }
    Ok(SignonKey {
        user_id: upper_case(&user_id),
        password: Password::new(upper_case(&password)),
    })
}

/// `text` as the 8-byte input field it was typed into, or the rejection of `field` when it is
/// longer than 8 bytes or holds a byte outside printable ASCII. The copybook error is dropped,
/// since the failure must carry nothing of the text.
fn map_field(text: &str, field: SignonField) -> Result<PicX<FIELD_WIDTH>, SignonFailure> {
    PicX::from_text(text).map_err(|_| SignonFailure::Rejected(field))
}

/// `FUNCTION UPPER-CASE` of an 8-byte field: `a` to `z` become `A` to `Z`, every other byte is
/// kept.
fn upper_case(field: &PicX<FIELD_WIDTH>) -> PicX<FIELD_WIDTH> {
    PicX::from_array(field.as_bytes().map(|byte| byte.to_ascii_uppercase()))
}

/// What sign-on needs of a `users` row: the stored `SEC-USR-TYPE` and the argon2id PHC string
/// that stands for `SEC-USR-PWD` (CSUSR01Y-004, CSUSR01Y-005; `E25`).
///
/// `Debug` prints the user type and never the hash.
#[derive(Clone, PartialEq, Eq)]
pub struct StoredUser<'a> {
    user_type: UserType,
    password_hash: &'a str,
}

impl<'a> StoredUser<'a> {
    /// The stored user type and password hash of the row read by [`SignonKey::user_id`].
    #[must_use]
    pub const fn new(user_type: UserType, password_hash: &'a str) -> Self {
        Self {
            user_type,
            password_hash,
        }
    }
}

impl fmt::Debug for StoredUser<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoredUser")
            .field("user_type", &self.user_type)
            .finish_non_exhaustive()
    }
}

/// The result of reading `users` by [`SignonKey::user_id`], the `EVALUATE WS-RESP-CD` of
/// `READ-USER-SEC-FILE` (`COSGN00C.cbl:211-257`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserLookup<'a> {
    /// RESP 0: the row exists (COSGN00C-013).
    Found(StoredUser<'a>),
    /// RESP 13, `NOTFND`: no row has the ID (COSGN00C-019).
    NotFound,
    /// Any other RESP: the read failed for another reason, such as an unavailable database
    /// (COSGN00C-020).
    ReadError,
}

/// A successful sign-on: the COMMAREA identity `READ-USER-SEC-FILE` sets and the program it
/// transfers to (COSGN00C-013 to COSGN00C-016).
///
/// It holds no password and no hash. The API issues its token from `user_id` and `user_type`
/// alone, and returns them with `next_route`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedOn {
    /// `CDEMO-USER-ID`: the upper-cased user ID the row was read by, the claim `sub`
    /// (COSGN00C-014, COCOM01Y-005).
    pub user_id: PicX<FIELD_WIDTH>,
    /// `CDEMO-USER-TYPE`: the stored `SEC-USR-TYPE`, unchanged, the claim `usr_type`
    /// (COSGN00C-014, COCOM01Y-006).
    pub user_type: UserType,
    /// The `XCTL` target: the admin menu for `A`, the main menu otherwise (COSGN00C-015,
    /// COSGN00C-016).
    pub next_route: NextRoute,
}

/// What [`decide`] concludes for a [`SignonKey`] and its [`UserLookup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignonOutcome {
    /// The password verified: transfer with this identity (COSGN00C-013 to COSGN00C-016).
    SignedOn(SignedOn),
    /// The sign-on screen is sent again with this failure (COSGN00C-018 to COSGN00C-020).
    Failed(SignonFailure),
}

impl SignonOutcome {
    /// The outcome as a `Result`, for a caller that propagates the failure.
    ///
    /// # Errors
    ///
    /// The [`SignonFailure`] of [`SignonOutcome::Failed`].
    pub fn into_result(self) -> Result<SignedOn, SignonFailure> {
        match self {
            Self::SignedOn(signed_on) => Ok(signed_on),
            Self::Failed(failure) => Err(failure),
        }
    }
}

/// `READ-USER-SEC-FILE` after the read: the password check and the outcome of each response
/// (COSGN00C-013 to COSGN00C-016, COSGN00C-018 to COSGN00C-020).
///
/// - [`UserLookup::Found`]: the 8 upper-cased password bytes are verified with argon2id against
///   the stored hash, the counterpart of `SEC-USR-PWD = WS-USER-PWD` (COSGN00C-013). A match
///   signs on as [`SignedOn`]; a mismatch is [`SignonFailure::WrongPassword`] (COSGN00C-018). A
///   stored hash that cannot be verified at all, malformed or naming parameters argon2 cannot
///   run, has no COBOL counterpart and takes the `WHEN OTHER` branch,
///   [`SignonFailure::UnableToVerify`], never a wrong password (COSGN00C-020, `X-COSGN00C-2`).
/// - [`UserLookup::NotFound`]: [`SignonFailure::UserNotFound`]; the password is not examined
///   (COSGN00C-019).
/// - [`UserLookup::ReadError`]: [`SignonFailure::UnableToVerify`] (COSGN00C-020).
///
/// The decision rests on this request alone, so a correct sign-on succeeds after any number of
/// failed ones (COSGN00C-006).
#[must_use]
pub fn decide(key: &SignonKey, lookup: UserLookup<'_>) -> SignonOutcome {
    match lookup {
        UserLookup::Found(stored) => {
            match password::verify(key.password.expose(), stored.password_hash) {
                Ok(true) => SignonOutcome::SignedOn(SignedOn {
                    next_route: NextRoute::for_user_type(&stored.user_type),
                    user_id: key.user_id.clone(),
                    user_type: stored.user_type,
                }),
                Ok(false) => SignonOutcome::Failed(SignonFailure::WrongPassword),
                // A stored hash argon2 cannot check has no COBOL counterpart: it takes the
                // `WHEN OTHER` branch and is never reported as a wrong password (X-COSGN00C-2).
                Err(_) => SignonOutcome::Failed(SignonFailure::UnableToVerify),
            }
        }
        UserLookup::NotFound => SignonOutcome::Failed(SignonFailure::UserNotFound),
        UserLookup::ReadError => SignonOutcome::Failed(SignonFailure::UnableToVerify),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::sync::OnceLock;

    use copybook::PicX;

    use super::{
        FailureClass, NextRoute, SignonFailure, SignonField, StoredUser, UserLookup, UserType,
        decide, edit,
    };
    use crate::password::{self, PasswordError};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The upper-case password of the in-stream `USRSEC` records (`DUSRSECJ.jcl:35-44`), the
    /// demo value `README.md:203-204` publishes.
    const DEMO_PASSWORD: &[u8; 8] = b"PASSWORD";

    /// The argon2id hash of [`DEMO_PASSWORD`], computed once, because argon2 with its default
    /// cost is slow in an unoptimised test build.
    fn demo_hash() -> Result<String, PasswordError> {
        static HASH: OnceLock<Result<String, PasswordError>> = OnceLock::new();
        HASH.get_or_init(|| password::hash(DEMO_PASSWORD)).clone()
    }

    /// The user type whose `PIC X(01)` byte is `code`.
    fn user_type(code: u8) -> UserType {
        UserType::from_code(PicX::from_array([code]))
    }

    // audit: COSGN00C-007
    #[test]
    fn a_blank_user_id_is_reported_whatever_the_password_holds() {
        for (user_id, password) in [
            ("", "PASSWORD"),
            ("        ", "PASSWORD"),
            ("", ""),
            (" ", "  "),
        ] {
            assert_eq!(
                edit(user_id, password).err(),
                Some(SignonFailure::UserIdBlank)
            );
        }
        let failure = SignonFailure::UserIdBlank;
        assert_eq!(failure.message(), "Please enter User ID ...");
        assert_eq!(failure.field(), SignonField::UserId);
        assert_eq!(failure.field().name(), "user_id");
        assert_eq!(failure.class(), FailureClass::Input);
        assert_eq!(failure.class().http_status(), 400);
    }

    // audit: COSGN00C-008
    #[test]
    fn a_blank_password_is_reported_after_the_user_id() {
        for (user_id, password) in [("ADMIN001", ""), ("ADMIN001", "        "), (" A", " ")] {
            assert_eq!(
                edit(user_id, password).err(),
                Some(SignonFailure::PasswordBlank)
            );
        }
        let failure = SignonFailure::PasswordBlank;
        assert_eq!(failure.message(), "Please enter Password ...");
        assert_eq!(failure.field().name(), "password");
        assert_eq!(failure.class().http_status(), 400);
    }

    // audit: COSGN00C-003
    // audit: COSGN00C-009
    #[test]
    fn inputs_are_padded_8_byte_fields_upper_cased_a_to_z_only() -> TestResult {
        let key = edit("admin001", "password")?;
        assert_eq!(key.user_id().as_bytes(), b"ADMIN001");
        assert!(key.password().expose() == DEMO_PASSWORD, "COSGN00C-009");

        let key = edit(" user1", "pa55 w-z")?;
        assert_eq!(key.user_id().as_bytes(), b" USER1  ");
        assert!(key.password().expose() == b"PA55 W-Z", "COSGN00C-003");

        let key = edit("a{|}~`z0", "x")?;
        assert_eq!(key.user_id().as_bytes(), b"A{|}~`Z0");
        assert!(key.password().expose() == b"X       ", "COSGN00C-003");
        Ok(())
    }

    // audit: COSGN00C-003
    #[test]
    fn a_value_no_8_byte_field_carries_is_rejected_before_the_edits() {
        let user_id = SignonField::UserId;
        let password = SignonField::Password;
        for (typed_id, typed_password, field) in [
            ("ADMIN0011", "PASSWORD", user_id),
            ("ADMIN001", "PASSWORD1", password),
            ("USER\u{e9}", "PASSWORD", user_id),
            ("\u{e9}\u{e9}\u{e9}\u{e9}", "PASSWORD", user_id),
            ("\0", "PASSWORD", user_id),
            ("ADMIN001", "PASS\tWD", password),
            ("ADMIN001", "\u{7f}", password),
            ("", "PASSWORD12", password),
            ("ADMIN0011", "", user_id),
            ("ADMIN0011", "PASSWORD12", user_id),
        ] {
            let failure = edit(typed_id, typed_password).err();
            assert_eq!(failure, Some(SignonFailure::Rejected(field)));
        }
        for (field, message) in [
            (
                user_id,
                "User ID must be at most 8 printable ASCII characters",
            ),
            (
                password,
                "Password must be at most 8 printable ASCII characters",
            ),
        ] {
            let failure = SignonFailure::Rejected(field);
            assert_eq!(failure.message(), message);
            assert_eq!(failure.field(), field);
            assert_eq!(failure.class().http_status(), 400);
        }
    }

    // audit: COSGN00C-013
    // audit: COSGN00C-014
    // audit: COSGN00C-015
    #[test]
    fn an_admin_signs_on_with_the_upper_cased_id_and_goes_to_the_admin_menu() -> TestResult {
        let hash = demo_hash()?;
        let key = edit("admin001", "password")?;
        let lookup = UserLookup::Found(StoredUser::new(UserType::ADMIN, &hash));
        let signed_on = decide(&key, lookup).into_result()?;
        assert_eq!(signed_on.user_id.as_bytes(), b"ADMIN001");
        assert_eq!(signed_on.user_type, UserType::ADMIN);
        assert_eq!(signed_on.next_route, NextRoute::Admin);
        assert_eq!(signed_on.next_route.path(), "/admin");
        Ok(())
    }

    // audit: COSGN00C-014
    // audit: COSGN00C-016
    #[test]
    fn every_other_type_signs_on_to_the_main_menu_with_its_type_unchanged() -> TestResult {
        let hash = demo_hash()?;
        let key = edit("USER0001", "PASSWORD")?;
        let lookup = UserLookup::Found(StoredUser::new(UserType::USER, &hash));
        let signed_on = decide(&key, lookup).into_result()?;
        assert_eq!(signed_on.user_id.as_bytes(), b"USER0001");
        assert_eq!(signed_on.user_type.code().as_bytes(), b"U");
        assert_eq!(signed_on.next_route.path(), "/menu");

        for code in [b'U', b' ', b'a', b'X'] {
            assert_eq!(NextRoute::for_user_type(&user_type(code)), NextRoute::Menu);
        }
        assert_eq!(NextRoute::for_user_type(&UserType::ADMIN), NextRoute::Admin);
        assert_eq!(NextRoute::Menu.path(), "/menu");
        Ok(())
    }

    // audit: COSGN00C-018
    // audit: COUSR01C-019
    #[test]
    fn a_password_stored_in_lower_case_never_verifies() -> TestResult {
        let stored_as_typed = password::hash(b"password")?;
        let key = edit("USER0001", "password")?;
        let lookup = UserLookup::Found(StoredUser::new(UserType::USER, &stored_as_typed));
        assert_eq!(
            decide(&key, lookup).into_result(),
            Err(SignonFailure::WrongPassword)
        );
        let failure = SignonFailure::WrongPassword;
        assert_eq!(failure.message(), "Wrong Password. Try again ...");
        assert_eq!(failure.field().name(), "password");
        assert_eq!(failure.class(), FailureClass::Signon);
        assert_eq!(failure.class().http_status(), 401);
        Ok(())
    }

    // audit: COSGN00C-019
    #[test]
    fn an_unknown_user_is_not_found() -> TestResult {
        let key = edit("nobody", "PASSWORD")?;
        assert_eq!(
            decide(&key, UserLookup::NotFound).into_result(),
            Err(SignonFailure::UserNotFound)
        );
        let failure = SignonFailure::UserNotFound;
        assert_eq!(failure.message(), "User not found. Try again ...");
        assert_eq!(failure.field().name(), "user_id");
        assert_eq!(failure.class().http_status(), 401);
        Ok(())
    }

    // audit: COSGN00C-020
    #[test]
    fn a_failed_read_or_an_unverifiable_hash_cannot_verify_the_user() -> TestResult {
        let key = edit("ADMIN001", "PASSWORD")?;
        let unable = Err(SignonFailure::UnableToVerify);
        assert_eq!(decide(&key, UserLookup::ReadError).into_result(), unable);
        for stored in ["", "not-a-phc", "$argon2id$v=19$m=19456,t=2,p=1"] {
            let lookup = UserLookup::Found(StoredUser::new(UserType::ADMIN, stored));
            assert_eq!(decide(&key, lookup).into_result(), unable);
        }
        let failure = SignonFailure::UnableToVerify;
        assert_eq!(failure.message(), "Unable to verify the User ...");
        assert_eq!(failure.field().name(), "user_id");
        assert_eq!(failure.class(), FailureClass::Store);
        assert_eq!(failure.class().http_status(), 500);
        Ok(())
    }

    // audit: COCOM01Y-006
    #[test]
    fn user_types_follow_the_88_levels_exactly() {
        assert!(UserType::ADMIN.is_admin());
        assert!(!UserType::ADMIN.is_user());
        assert!(UserType::USER.is_user());
        assert!(!UserType::USER.is_admin());
        assert_eq!(user_type(b'A'), UserType::ADMIN);
        for code in [b' ', b'a', b'u', b'X'] {
            let other = user_type(code);
            assert!(!other.is_admin() && !other.is_user());
            assert_eq!(other.code().as_bytes(), &[code]);
        }
    }

    // audit: PIC-024
    #[test]
    fn no_output_shows_the_password_or_the_stored_hash() -> TestResult {
        let key = edit("admin001", "hidden01")?;
        let text = format!("{key:?}");
        assert!(
            text.contains("ADMIN001") && text.contains("********"),
            "{text}"
        );
        assert!(
            !text.contains("HIDDEN01") && !text.contains("hidden01"),
            "PIC-024"
        );

        let hash = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2g";
        let text = format!(
            "{:?}",
            UserLookup::Found(StoredUser::new(UserType::USER, hash))
        );
        assert!(
            text.contains("StoredUser") && text.contains("user_type"),
            "{text}"
        );
        assert!(
            !text.contains("argon2id") && !text.contains("aGFzaGhhc2g"),
            "{text}"
        );

        for failure in [
            SignonFailure::Rejected(SignonField::UserId),
            SignonFailure::Rejected(SignonField::Password),
            SignonFailure::UserIdBlank,
            SignonFailure::PasswordBlank,
            SignonFailure::WrongPassword,
            SignonFailure::UserNotFound,
            SignonFailure::UnableToVerify,
        ] {
            assert_eq!(failure.to_string(), failure.message());
            assert!(
                !failure
                    .to_string()
                    .to_ascii_uppercase()
                    .contains("HIDDEN01")
            );
        }
        Ok(())
    }
}
