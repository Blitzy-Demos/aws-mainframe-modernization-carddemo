//! COUSR01C, user add: the rules of endpoint 17, `POST /api/v1/users`, admin only (transaction
//! `CU01`, map `COUSR1A`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COUSR01C.md` that a pure function
//! decides: the five empty-field edits of `PROCESS-ENTER-KEY`, in its order (COUSR01C-010 to
//! COUSR01C-014); the record the add writes, every field exactly as typed (COUSR01C-015,
//! COUSR01C-016, COUSR01C-019, COUSR01C-020); and the messages of the three outcomes of
//! `WRITE-USER-SEC-FILE` (COUSR01C-021, COUSR01C-023, COUSR01C-024). The admin gate
//! (COUSR01C-001, COUSR01C-002) is the `AdminUser` extractor's, and the caller's identity never
//! changes (COUSR01C-026). The `client` rows, F3, F4 and every other key among them
//! (COUSR01C-007 to COUSR01C-009), send no request and belong to the screen.
//!
//! # Flow
//!
//! `api::services::cousr01c` calls [`edit`], hashes the 8 password bytes of the [`NewUser`] with
//! `crate::password::hash` over [`Password::expose`] (COUSR01C-018, E25) and inserts the row of
//! [`NewUser::into_record`]. It answers a stored row with [`added_message`], the unique violation
//! of the `users` primary key with [`duplicate_user_id`], and every other store failure, a failed
//! hash included, with [`write_failure`]. This module does no I/O and no hashing.
//!
//! The COBOL sends one screen per ENTER: the first empty field in `EVALUATE` order, or the
//! outcome of the write. A failure is a [`UserAddFailure`]: the literal moved to `WS-MESSAGE`,
//! the request field the cursor goes to and the status class.
//!
//! # Input
//!
//! The five inputs enter as the raw text of their map fields ([`UserAddInput`]). Each becomes its
//! `PIC X(n)` record field unchanged, left-justified and padded with spaces to 20, 20, 8, 8 and 1
//! bytes: no trimming and no case change, so leading spaces and lower-case letters are stored
//! (COUSR01C-016). The password is stored as typed while sign-on upper-cases what it is given, so
//! a password with a lower-case letter never verifies (E26, preserved; COUSR01C-019); the same
//! holds for the user ID and the sign-on read (X-COUSR01C-1, COUSR01C-020). The user type is any
//! non-blank character, although the screen prompts `(A=Admin, U=User)` (X-COUSR01C-2,
//! COUSR01C-015). A field is empty when its padded value is all spaces: absent, empty or only
//! spaces.
//!
//! # Totality
//!
//! [`edit`] takes any text and returns a value or a [`UserAddFailure`]; nothing indexes, slices,
//! unwraps or panics (Total Parsers Over Untrusted Input). A value longer than its map `LENGTH`
//! or holding a byte outside printable ASCII fits no `PIC X(n)` field (PIC-001, PIC-002). The
//! request DTO rejects it first (COUSR01-013, COUSR01-016, COUSR01-019, COUSR01-022,
//! COUSR01-025), and [`edit`] rejects it too, before the COBOL edits, naming the field with a
//! message of this replacement's that claims no parity (X-COUSR01C-3).
//!
//! # Halts
//!
//! `SEC-USR-FILLER PIC X(23)` is never assigned, so the 23 bytes the COBOL writes are initial
//! storage: halt candidate X-COUSR01C-H1, proposed to join `HALTED(H2)` (COUSR01C-017).
//! [`NewUser::into_record`] writes 23 spaces, the value the user load stores (CSUSR01Y-006), and
//! that value is no parity result.

use core::fmt;

use copybook::PicX;

use crate::records::user_security::UserSecurityRecord;
use crate::sensitive::Password;

/// Width of `FNAMEI`/`LNAMEI PIC X(20)` and of `SEC-USR-FNAME`/`SEC-USR-LNAME PIC X(20)`
/// (CSUSR01Y-002, CSUSR01Y-003).
const NAME_WIDTH: usize = 20;

/// Width of `USERIDI PIC X(8)` and of `SEC-USR-ID PIC X(08)` (CSUSR01Y-001).
const USER_ID_WIDTH: usize = 8;

/// Width of `PASSWDI PIC X(8)` and of `SEC-USR-PWD PIC X(08)` (CSUSR01Y-004).
const PASSWORD_WIDTH: usize = 8;

/// Width of `USRTYPEI PIC X(1)` and of `SEC-USR-TYPE PIC X(01)` (CSUSR01Y-005).
const USER_TYPE_WIDTH: usize = 1;

/// What the `Debug` of a [`UserAddInput`] prints in place of the password.
const PASSWORD_REDACTED: &str = "********";

/// Head of the success message, `'User '` `DELIMITED BY SIZE` (COUSR01C-021).
const ADDED_HEAD: &str = "User ";

/// Tail of the success message, `' has been added ...'` `DELIMITED BY SIZE` (COUSR01C-021).
const ADDED_TAIL: &str = " has been added ...";

/// `DFHRESP(DUPKEY)` or `DFHRESP(DUPREC)` of the write (COUSR01C-023).
const DUPLICATE_USER_ID: &str = "User ID already exist...";

/// Any other response of the write (COUSR01C-024).
const WRITE_FAILURE: &str = "Unable to Add User...";

/// The longest success message, an 8-byte ID between head and tail, fits `WS-MESSAGE PIC X(80)`
/// and the 78-byte message row, so the `STRING` never overflows (COUSR01C-021).
const _: () = assert!(ADDED_HEAD.len() + USER_ID_WIDTH + ADDED_TAIL.len() <= 78);

/// The request field a failure names: where the COBOL puts the cursor (`MOVE -1 TO <field>L`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserAddField {
    /// `FNAME`: its empty edit (COUSR01C-010) and a failed write (COUSR01C-024).
    FirstName,
    /// `LNAME`: its empty edit (COUSR01C-011).
    LastName,
    /// `USERID`: its empty edit (COUSR01C-012) and a duplicate ID (COUSR01C-023).
    UserId,
    /// `PASSWD`: its empty edit (COUSR01C-013).
    Password,
    /// `USRTYPE`: its empty edit (COUSR01C-014).
    UserType,
}

impl UserAddField {
    /// The `UserAddRequest` member the `field` of the `ScreenError` names: `first_name`,
    /// `last_name`, `user_id`, `password` or `user_type` (COUSR01C-010 to COUSR01C-014,
    /// COUSR01C-023, COUSR01C-024).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FirstName => "first_name",
            Self::LastName => "last_name",
            Self::UserId => "user_id",
            Self::Password => "password",
            Self::UserType => "user_type",
        }
    }
}

/// The status class of a [`UserAddFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An empty field (COUSR01C-010 to COUSR01C-014), or a value no `PIC X(n)` field can hold
    /// (X-COUSR01C-3): 400.
    Input,
    /// The user ID is taken (COUSR01C-023): 400, not 409. The audit keeps 409 for a record
    /// changed by another user, and a taken ID is input the operator must change.
    Duplicate,
    /// The store could not add the user (COUSR01C-024): 500.
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400 for [`FailureClass::Input`] and
    /// [`FailureClass::Duplicate`], 500 for [`FailureClass::Store`].
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input | Self::Duplicate => 400,
            Self::Store => 500,
        }
    }
}

/// A screen COUSR01C sends instead of adding the user: the message, the field in error and the
/// status class.
///
/// The message is the literal moved to `WS-MESSAGE PIC X(80)` without its trailing spaces, as
/// the response carries it, or for X-COUSR01C-3 this replacement's text. None carries a request
/// value, so no failure can disclose a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserAddFailure {
    message: &'static str,
    field: UserAddField,
    class: FailureClass,
}

impl UserAddFailure {
    /// A failed edit of the request: status class [`FailureClass::Input`].
    const fn input(message: &'static str, field: UserAddField) -> Self {
        Self {
            message,
            field,
            class: FailureClass::Input,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field the cursor goes to.
    #[must_use]
    pub const fn field(&self) -> UserAddField {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for UserAddFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for UserAddFailure {}

/// The five inputs of `COUSR1A`, each the raw text of its map field (`UserAddRequest` of
/// endpoint 17; COUSR01C-006). An absent field is the empty text.
///
/// `Debug` prints `********` in place of the password (Cardholder Data Redaction).
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct UserAddInput<'a> {
    /// `FNAME`, `PIC X(20)`.
    pub first_name: &'a str,
    /// `LNAME`, `PIC X(20)`.
    pub last_name: &'a str,
    /// `USERID`, `PIC X(8)`.
    pub user_id: &'a str,
    /// `PASSWD`, `PIC X(8)`, a dark field.
    pub password: &'a str,
    /// `USRTYPE`, `PIC X(1)`.
    pub user_type: &'a str,
}

impl fmt::Debug for UserAddInput<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserAddInput")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("user_id", &self.user_id)
            .field("password", &format_args!("{PASSWORD_REDACTED}"))
            .field("user_type", &self.user_type)
            .finish()
    }
}

/// The user an add writes, once [`edit`] has passed every field: `SEC-USER-DATA` as
/// `PROCESS-ENTER-KEY` moves the five map fields into it (COUSR01C-016).
///
/// Only [`edit`] builds one, so no field is blank and every value is exactly as typed. `Debug`
/// prints `********` in place of the password, which is a [`Password`] (Cardholder Data
/// Redaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    user_id: PicX<USER_ID_WIDTH>,
    first_name: PicX<NAME_WIDTH>,
    last_name: PicX<NAME_WIDTH>,
    password: Password,
    user_type: PicX<USER_TYPE_WIDTH>,
}

impl NewUser {
    /// `SEC-USR-ID PIC X(08)`, the key of the write and of the success message (COUSR01C-016,
    /// COUSR01C-021, COUSR01C-023).
    #[must_use]
    pub const fn user_id(&self) -> &PicX<USER_ID_WIDTH> {
        &self.user_id
    }

    /// `SEC-USR-FNAME PIC X(20)` (COUSR01C-016).
    #[must_use]
    pub const fn first_name(&self) -> &PicX<NAME_WIDTH> {
        &self.first_name
    }

    /// `SEC-USR-LNAME PIC X(20)` (COUSR01C-016).
    #[must_use]
    pub const fn last_name(&self) -> &PicX<NAME_WIDTH> {
        &self.last_name
    }

    /// `SEC-USR-PWD PIC X(08)`, as typed: the 8 bytes the service hashes (COUSR01C-018, E26).
    #[must_use]
    pub const fn password(&self) -> &Password {
        &self.password
    }

    /// `SEC-USR-TYPE PIC X(01)`, as typed: `A` is an administrator, any other character a
    /// regular user (COUSR01C-015).
    #[must_use]
    pub const fn user_type(&self) -> &PicX<USER_TYPE_WIDTH> {
        &self.user_type
    }

    /// The 80-byte `SEC-USER-DATA` record `WRITE-USER-SEC-FILE` writes (COUSR01C-016).
    ///
    /// `SEC-USR-FILLER` is 23 spaces. The COBOL never assigns it, so its bytes have no single
    /// value: `HALTED(H2)` through candidate X-COUSR01C-H1, and these spaces are no parity result
    /// (COUSR01C-017).
    #[must_use]
    pub fn into_record(self) -> UserSecurityRecord {
        UserSecurityRecord {
            id: self.user_id,
            fname: self.first_name,
            lname: self.last_name,
            pwd: self.password,
            usr_type: self.user_type,
            filler: PicX::spaces(),
        }
    }
}

/// One map field's edits: the field the failure names, the literal of its empty edit, and the
/// text rejecting a value its `PIC X(n)` cannot hold.
struct FieldSpec {
    field: UserAddField,
    empty: &'static str,
    unrepresentable: &'static str,
}

const FIRST_NAME: FieldSpec = FieldSpec {
    field: UserAddField::FirstName,
    empty: "First Name can NOT be empty...",
    unrepresentable: "First Name must be at most 20 printable ASCII characters",
};

const LAST_NAME: FieldSpec = FieldSpec {
    field: UserAddField::LastName,
    empty: "Last Name can NOT be empty...",
    unrepresentable: "Last Name must be at most 20 printable ASCII characters",
};

const USER_ID: FieldSpec = FieldSpec {
    field: UserAddField::UserId,
    empty: "User ID can NOT be empty...",
    unrepresentable: "User ID must be at most 8 printable ASCII characters",
};

const PASSWORD: FieldSpec = FieldSpec {
    field: UserAddField::Password,
    empty: "Password can NOT be empty...",
    unrepresentable: "Password must be at most 8 printable ASCII characters",
};

const USER_TYPE: FieldSpec = FieldSpec {
    field: UserAddField::UserType,
    empty: "User Type can NOT be empty...",
    unrepresentable: "User Type must be at most 1 printable ASCII character",
};

/// `text` as its `PIC X(N)` field, left-justified and padded with spaces; the cause of a
/// rejection is dropped, because it may describe the password.
fn field<const N: usize>(text: &str, spec: &FieldSpec) -> Result<PicX<N>, UserAddFailure> {
    PicX::from_text(text).map_err(|_| UserAddFailure::input(spec.unrepresentable, spec.field))
}

/// The ENTER key of `COUSR1A`: the edits of `PROCESS-ENTER-KEY` and the record it builds.
///
/// The five values are first taken as their `PIC X(n)` fields, in screen order (X-COUSR01C-3).
/// The empty edits then run in the program's `EVALUATE` order, and the first empty field is the
/// answer: First Name (COUSR01C-010), Last Name (COUSR01C-011), User ID (COUSR01C-012), Password
/// (COUSR01C-013), User Type (COUSR01C-014). With none empty the fields are moved into the record
/// as typed (COUSR01C-015, COUSR01C-016, COUSR01C-019, COUSR01C-020).
///
/// # Errors
///
/// A [`UserAddFailure`] of class [`FailureClass::Input`] naming its field:
///
/// - a value longer than its map `LENGTH` or holding a byte outside `0x20`–`0x7E`: this
///   replacement's `<Field> must be at most <n> printable ASCII characters`, `character` for
///   the one-byte User Type (X-COUSR01C-3);
/// - the first empty field: `First Name can NOT be empty...`, `Last Name can NOT be empty...`,
///   `User ID can NOT be empty...`, `Password can NOT be empty...` or
///   `User Type can NOT be empty...` (COUSR01C-010 to COUSR01C-014).
pub fn edit(input: &UserAddInput<'_>) -> Result<NewUser, UserAddFailure> {
    let first_name = field::<NAME_WIDTH>(input.first_name, &FIRST_NAME)?;
    let last_name = field::<NAME_WIDTH>(input.last_name, &LAST_NAME)?;
    let user_id = field::<USER_ID_WIDTH>(input.user_id, &USER_ID)?;
    let password = field::<PASSWORD_WIDTH>(input.password, &PASSWORD)?;
    let user_type = field::<USER_TYPE_WIDTH>(input.user_type, &USER_TYPE)?;

    let empty = [
        (first_name.is_spaces(), &FIRST_NAME),
        (last_name.is_spaces(), &LAST_NAME),
        (user_id.is_spaces(), &USER_ID),
        (password.is_spaces(), &PASSWORD),
        (user_type.is_spaces(), &USER_TYPE),
    ]
    .into_iter()
    .find_map(|(blank, spec)| blank.then_some(spec));
    if let Some(spec) = empty {
        return Err(UserAddFailure::input(spec.empty, spec.field));
    }

    Ok(NewUser {
        user_id,
        first_name,
        last_name,
        password: Password::new(password),
        user_type,
    })
}

/// The message of a stored user (`DFHRESP(NORMAL)`, COUSR01C-021): `User ` + the user ID up to
/// its first space + ` has been added ...`, the `STRING` with `SEC-USR-ID DELIMITED BY SPACE`.
///
/// `NEWUSR01` gives `User NEWUSR01 has been added ...`; an ID with an embedded space keeps only
/// the part before it; an ID that starts with a space contributes nothing, giving two spaces
/// between `User` and `has`.
#[must_use]
pub fn added_message(user_id: &PicX<USER_ID_WIDTH>) -> String {
    let id: String = user_id
        .as_bytes()
        .iter()
        .take_while(|&&byte| byte != b' ')
        .copied()
        .map(char::from)
        .collect();
    format!("{ADDED_HEAD}{id}{ADDED_TAIL}")
}

/// The answer to a write the `users` primary key rejects, the `DFHRESP(DUPKEY)` or
/// `DFHRESP(DUPREC)` of the COBOL: `User ID already exist...` on the user ID, class
/// [`FailureClass::Duplicate`] (COUSR01C-023).
#[must_use]
pub const fn duplicate_user_id() -> UserAddFailure {
    UserAddFailure {
        message: DUPLICATE_USER_ID,
        field: UserAddField::UserId,
        class: FailureClass::Duplicate,
    }
}

/// The answer to any other failure of the add, the `WHEN OTHER` of the COBOL, a failed hash
/// included: `Unable to Add User...` on the first name, class [`FailureClass::Store`]
/// (COUSR01C-024). No response code or store detail reaches the message.
#[must_use]
pub const fn write_failure() -> UserAddFailure {
    UserAddFailure {
        message: WRITE_FAILURE,
        field: UserAddField::FirstName,
        class: FailureClass::Store,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use copybook::PicX;

    use super::{
        FailureClass, UserAddFailure, UserAddField, UserAddInput, added_message, duplicate_user_id,
        edit, write_failure,
    };

    type TestResult = Result<(), Box<dyn Error>>;

    /// What a failure shows: message, field and status class.
    type Shown = (&'static str, UserAddField, FailureClass);

    /// The five fields in `EVALUATE` order, each with the literal of its empty edit.
    const EMPTY_EDITS: [(UserAddField, &str); 5] = [
        (UserAddField::FirstName, "First Name can NOT be empty..."),
        (UserAddField::LastName, "Last Name can NOT be empty..."),
        (UserAddField::UserId, "User ID can NOT be empty..."),
        (UserAddField::Password, "Password can NOT be empty..."),
        (UserAddField::UserType, "User Type can NOT be empty..."),
    ];

    /// Five non-blank fields, each inside its width.
    const FILLED: UserAddInput<'static> = UserAddInput {
        first_name: "Jane",
        last_name: "Doe",
        user_id: "NEWUSR01",
        password: "fakepw1",
        user_type: "U",
    };

    /// `input` with `field` set to `value`.
    fn with<'a>(input: UserAddInput<'a>, field: UserAddField, value: &'a str) -> UserAddInput<'a> {
        let mut changed = input;
        match field {
            UserAddField::FirstName => changed.first_name = value,
            UserAddField::LastName => changed.last_name = value,
            UserAddField::UserId => changed.user_id = value,
            UserAddField::Password => changed.password = value,
            UserAddField::UserType => changed.user_type = value,
        }
        changed
    }

    fn failed(input: &UserAddInput<'_>) -> Option<Shown> {
        edit(input)
            .err()
            .map(|failure| (failure.message(), failure.field(), failure.class()))
    }

    #[test]
    fn the_first_empty_field_in_evaluate_order_is_the_answer() {
        for (position, &(field, message)) in EMPTY_EDITS.iter().enumerate() {
            for blank in ["", " "] {
                let alone = with(FILLED, field, blank);
                let with_later = EMPTY_EDITS
                    .iter()
                    .skip(position)
                    .fold(FILLED, |input, &(later, _)| with(input, later, blank));
                for input in [alone, with_later] {
                    assert_eq!(
                        failed(&input),
                        Some((message, field, FailureClass::Input)),
                        "{message}"
                    );
                }
            }
        }
        let all_blank = EMPTY_EDITS
            .iter()
            .fold(FILLED, |input, &(field, _)| with(input, field, ""));
        assert_eq!(
            failed(&all_blank),
            Some((
                "First Name can NOT be empty...",
                UserAddField::FirstName,
                FailureClass::Input
            ))
        );
    }

    #[test]
    fn every_field_is_stored_as_typed_and_padded() -> TestResult {
        let input = UserAddInput {
            first_name: " jane",
            last_name: "o'Neil",
            user_id: "user0001",
            password: "pass",
            user_type: "a",
        };
        let new = edit(&input)?;
        assert_eq!(
            new.first_name(),
            &PicX::from_array(*b" jane               ")
        );
        assert_eq!(new.last_name(), &PicX::from_array(*b"o'Neil              "));
        assert_eq!(new.user_id(), &PicX::from_array(*b"user0001"));
        assert!(new.password().expose() == b"pass    ", "password as typed");
        assert_eq!(new.user_type(), &PicX::from_array(*b"a"));

        let record = new.clone().into_record();
        assert_eq!(&record.id, new.user_id());
        assert_eq!(&record.fname, new.first_name());
        assert_eq!(&record.lname, new.last_name());
        assert!(&record.pwd == new.password(), "password moved unchanged");
        assert_eq!(&record.usr_type, new.user_type());
        assert!(record.filler.is_spaces());
        assert!(!record.is_admin());
        Ok(())
    }

    #[test]
    fn full_width_values_and_any_non_blank_type_are_accepted() -> TestResult {
        let full = UserAddInput {
            first_name: "ABCDEFGHIJKLMNOPQRST",
            last_name: "abcdefghijklmnopqrst",
            user_id: "ADMIN002",
            password: "FAKEPW08",
            user_type: "A",
        };
        let new = edit(&full)?;
        assert_eq!(new.first_name(), &PicX::from_text(full.first_name)?);
        assert_eq!(new.last_name(), &PicX::from_text(full.last_name)?);
        assert!(
            new.password().expose() == b"FAKEPW08",
            "full-width password"
        );
        assert!(new.into_record().is_admin());

        for user_type in ["A", "U", "a", "X", "9", "*"] {
            let new = edit(&with(FILLED, UserAddField::UserType, user_type))?;
            assert_eq!(new.user_type(), &PicX::from_text(user_type)?);
        }
        Ok(())
    }

    #[test]
    fn a_value_no_field_can_hold_is_rejected_before_the_empty_edits() {
        let cases = [
            (UserAddField::FirstName, "ABCDEFGHIJKLMNOPQRSTU"),
            (UserAddField::FirstName, "Ren\u{e9}e"),
            (UserAddField::LastName, "abcdefghijklmnopqrstu"),
            (UserAddField::LastName, "Doe\t"),
            (UserAddField::UserId, "NEWUSR012"),
            (UserAddField::UserId, "\0\0\0\0\0\0\0\0"),
            (UserAddField::Password, "fakepw123"),
            (UserAddField::Password, "pa\u{df}"),
            (UserAddField::UserType, "AU"),
            (UserAddField::UserType, "\u{7f}"),
        ];
        let messages = [
            (
                UserAddField::FirstName,
                "First Name must be at most 20 printable ASCII characters",
            ),
            (
                UserAddField::LastName,
                "Last Name must be at most 20 printable ASCII characters",
            ),
            (
                UserAddField::UserId,
                "User ID must be at most 8 printable ASCII characters",
            ),
            (
                UserAddField::Password,
                "Password must be at most 8 printable ASCII characters",
            ),
            (
                UserAddField::UserType,
                "User Type must be at most 1 printable ASCII character",
            ),
        ];
        for (field, value) in cases {
            let message = messages
                .iter()
                .find_map(|&(f, m)| (f == field).then_some(m))
                .unwrap_or_default();
            let expected = Some((message, field, FailureClass::Input));
            assert_eq!(failed(&with(FILLED, field, value)), expected, "{message}");
            // An earlier empty field does not take precedence.
            let earlier_blank = with(with(FILLED, field, value), UserAddField::FirstName, "");
            if field != UserAddField::FirstName {
                assert_eq!(failed(&earlier_blank), expected, "{message}");
            }
        }
    }

    #[test]
    fn the_added_message_keeps_the_id_up_to_its_first_space() -> TestResult {
        let cases = [
            ("NEWUSR01", "User NEWUSR01 has been added ..."),
            ("ab", "User ab has been added ..."),
            ("AB CD", "User AB has been added ..."),
            (" ABC", "User  has been added ..."),
        ];
        for (id, message) in cases {
            assert_eq!(added_message(&PicX::from_text(id)?), message);
        }
        let new = edit(&FILLED)?;
        assert_eq!(
            added_message(new.user_id()),
            "User NEWUSR01 has been added ..."
        );
        Ok(())
    }

    #[test]
    fn write_outcomes_carry_their_literal_field_and_status() {
        let duplicate = duplicate_user_id();
        assert_eq!(
            (duplicate.message(), duplicate.field(), duplicate.class()),
            (
                "User ID already exist...",
                UserAddField::UserId,
                FailureClass::Duplicate
            )
        );
        assert_eq!(duplicate.class().http_status(), 400);

        let store = write_failure();
        assert_eq!(
            (store.message(), store.field(), store.class()),
            (
                "Unable to Add User...",
                UserAddField::FirstName,
                FailureClass::Store
            )
        );
        assert_eq!(store.class().http_status(), 500);
        assert_eq!(store.to_string(), "Unable to Add User...");
        assert!(store.source().is_none());

        assert_eq!(FailureClass::Input.http_status(), 400);
    }

    #[test]
    fn fields_name_their_request_members() {
        let names = [
            (UserAddField::FirstName, "first_name"),
            (UserAddField::LastName, "last_name"),
            (UserAddField::UserId, "user_id"),
            (UserAddField::Password, "password"),
            (UserAddField::UserType, "user_type"),
        ];
        for (field, name) in names {
            assert_eq!(field.name(), name);
        }
    }

    #[test]
    fn no_debug_output_or_failure_shows_the_password() -> TestResult {
        let shown_input = format!("{FILLED:?}");
        assert!(shown_input.contains("********"), "input redacts");
        assert!(!shown_input.contains(FILLED.password), "input redacts");
        assert!(shown_input.contains(FILLED.user_id));

        let new = edit(&FILLED)?;
        assert!(
            !format!("{new:?}").contains(FILLED.password),
            "new user redacts"
        );
        assert!(
            !format!("{:?}", new.into_record()).contains(FILLED.password),
            "record redacts"
        );

        let too_long: Result<_, UserAddFailure> =
            edit(&with(FILLED, UserAddField::Password, "fakepw123"));
        let failure = too_long
            .err()
            .map(|f| format!("{f} {f:?}"))
            .unwrap_or_default();
        assert!(!failure.contains("fakepw"), "failure redacts");
        Ok(())
    }
}
