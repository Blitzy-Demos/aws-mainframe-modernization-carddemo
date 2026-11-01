//! `COUSR02C` (CU02): show a user for update and rewrite it, served as
//! `GET /admin/users/{id}` ([`lookup`], the ENTER fetch) and
//! `PUT /admin/users/{id}` ([`update`], PF5) to administrators only.
//!
//! [`lookup`] ports `PROCESS-ENTER-KEY` [app/cbl/COUSR02C.cbl:L143-172] with
//! `READ-USER-SEC-FILE` [app/cbl/COUSR02C.cbl:L320-353]; [`update`] ports
//! `UPDATE-USER-INFO` [app/cbl/COUSR02C.cbl:L177-245] with the same read and
//! `UPDATE-USER-SEC-FILE` [app/cbl/COUSR02C.cbl:L358-390]. The record is
//! `CSUSR01Y` `SEC-USER-DATA` [app/cpy/CSUSR01Y.cpy:L17-23]; the edited
//! fields are the `COUSR2AI` inputs `USRIDINI X(8)`, `FNAMEI X(20)`,
//! `LNAMEI X(20)`, `PASSWDI X(8)` and `USRTYPEI X(1)` [app/cpy-bms/COUSR02.CPY],
//! whose BMS names the errors carry.
//!
//! # The password (E-21, E-20)
//!
//! The COBOL moves the stored `SEC-USR-PWD` into the edit field when it shows a
//! user [app/cbl/COUSR02C.cbl:L166-170] and counts the password as changed only
//! when the field then differs from it [app/cbl/COUSR02C.cbl:L227-230]. A
//! stored argon2id hash cannot be shown, so [`lookup`]'s record goes out
//! without its password, and in [`update`] an absent password stands for that
//! reloaded value: unchanged, never re-hashed. A password that is present goes
//! through the empty edit, then is compared with the stored value by
//! [`verify_as_entered`], case kept; a different one is stored with
//! [`hash_stored_password`]. The one remaining difference, the current password
//! never displayed, is the known difference P-COUSR02C-007 records.
//!
//! # Messages
//!
//! `SEND-USRUPD-SCREEN` does not end the task, so the last message the
//! program sets is the one shown (P-COUSR02C-009). A successful fetch ends on
//! the 3270 prompt 'Press PF5 key to save your updates ...', which the message
//! catalog leaves out with every other input prompt, so [`lookup`] returns the
//! record alone.

use core::fmt;

use crate::auth::{SEC_USR_ID_LEN, SEC_USR_PWD_LEN, hash_stored_password, verify_as_entered};
use crate::entities::UserSecurity;
use crate::error::DomainError;
use crate::messages::{
    FIRST_NAME_CAN_NOT_BE_EMPTY, LAST_NAME_CAN_NOT_BE_EMPTY, PASSWORD_CAN_NOT_BE_EMPTY,
    PLEASE_MODIFY_TO_UPDATE, UNABLE_TO_LOOKUP_USER, UNABLE_TO_UPDATE_USER,
    USER_ID_CAN_NOT_BE_EMPTY, USER_ID_NOT_FOUND, USER_TYPE_CAN_NOT_BE_EMPTY, user_updated_message,
};
use crate::numeric::{Alnum, UserId};
use crate::ports::{PortError, UserStore};

/// BMS field `USRIDIN` of `COUSR2A`, the user ID.
const USRIDIN: &str = "USRIDIN";

/// BMS field `FNAME` of `COUSR2A`, the first name; also where `COUSR02C`
/// puts the cursor after a read or rewrite failure.
const FNAME: &str = "FNAME";

/// BMS field `LNAME` of `COUSR2A`, the last name.
const LNAME: &str = "LNAME";

/// BMS field `PASSWD` of `COUSR2A`, the password.
const PASSWD: &str = "PASSWD";

/// BMS field `USRTYPE` of `COUSR2A`, the user type.
const USRTYPE: &str = "USRTYPE";

/// Width of `FNAMEI`/`SEC-USR-FNAME` and `LNAMEI`/`SEC-USR-LNAME`, `X(20)`.
const NAME_LEN: usize = 20;

/// Width of `USRTYPEI`/`SEC-USR-TYPE`, `X(1)`.
const USER_TYPE_LEN: usize = 1;

/// The fields of a `PUT /admin/users/{id}`: the `COUSR2AI` inputs PF5 sends.
///
/// Each text is the value entered in its screen field. It is moved into the
/// `X(n)` field as the COBOL moves it: cut on the right at the field width and
/// space-padded, case unchanged.
///
/// `Debug` leaves out the password, so a plaintext password never reaches a
/// log line or a tracing span.
#[derive(Clone, PartialEq, Eq)]
pub struct UserUpdateInput {
    /// `USRIDINI X(8)`: the ID of the user to update, the route's `{id}`.
    pub user_id: String,
    /// `FNAMEI X(20)`: the first name.
    pub first_name: String,
    /// `LNAMEI X(20)`: the last name.
    pub last_name: String,
    /// `PASSWDI X(8)`: the new password, or `None` for the current one
    /// (E-21). `Some` with a blank value is the empty-field error.
    pub password: Option<String>,
    /// `USRTYPEI X(1)`: the user type.
    pub usr_type: String,
}

impl fmt::Debug for UserUpdateInput {
    /// Every field but the password, ended with `..` to show that one is left
    /// out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserUpdateInput")
            .field("user_id", &self.user_id)
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("usr_type", &self.usr_type)
            .finish_non_exhaustive()
    }
}

/// How an update that passed its edits and found its user ended.
///
/// Both outcomes leave `WS-ERR-FLG` off, so neither is an error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserUpdateOutcome {
    /// A field differed, and the `REWRITE` answered `NORMAL` — P-COUSR02C-006.
    Updated {
        /// The record as rewritten, its password hash included.
        user: UserSecurity,
        /// `User <id> has been updated ...` [app/cbl/COUSR02C.cbl:L370-375].
        message: String,
    },
    /// Every field equals the stored value: nothing is written and the
    /// program shows [`PLEASE_MODIFY_TO_UPDATE`] without setting the error
    /// flag [app/cbl/COUSR02C.cbl:L236-243] — P-COUSR02C-005, V-COUSR02C-010.
    NotModified {
        /// [`PLEASE_MODIFY_TO_UPDATE`].
        message: &'static str,
    },
}

/// The COBOL condition `= SPACES OR LOW-VALUES` on the `X(width)` field that
/// receives `text`.
///
/// Only the first `width` bytes reach the field. They are blank when they are
/// all spaces or all NUL characters, an empty text included: the bytes a field
/// left unentered holds, as [`Confirm::parse`](super::Confirm::parse) reads
/// them.
fn is_blank(text: &str, width: usize) -> bool {
    let field = text.as_bytes().get(..width).unwrap_or(text.as_bytes());
    field.iter().all(|&byte| byte == b' ') || field.iter().all(|&byte| byte == 0)
}

/// The error of an empty field: its message, with the cursor on `field`.
fn empty(message: &'static str, field: &'static str) -> DomainError {
    DomainError::empty(message).with_field(field)
}

/// `UPDATE-USER-SEC-FILE` `WHEN OTHER`: 'Unable to Update User...' with the
/// cursor on `FNAME` [app/cbl/COUSR02C.cbl:L383-389] — V-COUSR02C-012.
fn unable_to_update() -> DomainError {
    DomainError::unexpected(UNABLE_TO_UPDATE_USER).with_field(FNAME)
}

/// COUSR02C PROCESS-ENTER-KEY, READ-USER-SEC-FILE — P-COUSR02C-002,
/// P-COUSR02C-003, P-COUSR02C-007, V-COUSR02C-001, V-COUSR02C-002,
/// V-COUSR02C-003.
///
/// Fetches the user `user_id` names for update, as the ENTER key and the
/// entry from the user list do [app/cbl/COUSR02C.cbl:L143-172]: a blank or
/// absent ID is rejected before any file access, and otherwise the record is
/// read with `READ … UPDATE` [app/cbl/COUSR02C.cbl:L322-331]. The record
/// returned holds the password hash, which the caller never shows (E-21).
///
/// # Errors
///
/// - [`USER_ID_CAN_NOT_BE_EMPTY`], kind `Empty`, field `USRIDIN`, when the ID
///   is absent, empty, spaces or NULs (V-COUSR02C-001).
/// - [`USER_ID_NOT_FOUND`], kind `NotFound`, field `USRIDIN`, when no user has
///   the ID (V-COUSR02C-002).
/// - [`UNABLE_TO_LOOKUP_USER`], kind `Unexpected`, field `FNAME`, on any other
///   read failure (V-COUSR02C-003).
pub async fn lookup<S: UserStore>(
    store: &mut S,
    user_id: Option<String>,
) -> Result<UserSecurity, DomainError> {
    let user_id = user_id.unwrap_or_default();
    if is_blank(&user_id, SEC_USR_ID_LEN) {
        return Err(empty(USER_ID_CAN_NOT_BE_EMPTY, USRIDIN));
    }
    store
        .read_user_for_update(UserId::move_from(&user_id))
        .await
        .map_err(|error| match error {
            PortError::NotFound => DomainError::not_found(USER_ID_NOT_FOUND).with_field(USRIDIN),
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => {
                DomainError::unexpected(UNABLE_TO_LOOKUP_USER).with_field(FNAME)
            }
        })
}

/// COUSR02C UPDATE-USER-INFO — P-COUSR02C-004, V-COUSR02C-004,
/// V-COUSR02C-005, V-COUSR02C-006, V-COUSR02C-007, V-COUSR02C-008.
///
/// The empty checks, one `EVALUATE` in screen order whose first failing branch
/// wins [app/cbl/COUSR02C.cbl:L179-213]: user ID, first name, last name,
/// password, user type. An absent password stands for the current one, which
/// is not blank, so only a password that is present is checked (E-21).
fn edit(input: &UserUpdateInput) -> Result<(), DomainError> {
    let password_blank = input
        .password
        .as_deref()
        .is_some_and(|password| is_blank(password, SEC_USR_PWD_LEN));
    let checks = [
        (
            is_blank(&input.user_id, SEC_USR_ID_LEN),
            USER_ID_CAN_NOT_BE_EMPTY,
            USRIDIN,
        ),
        (
            is_blank(&input.first_name, NAME_LEN),
            FIRST_NAME_CAN_NOT_BE_EMPTY,
            FNAME,
        ),
        (
            is_blank(&input.last_name, NAME_LEN),
            LAST_NAME_CAN_NOT_BE_EMPTY,
            LNAME,
        ),
        (password_blank, PASSWORD_CAN_NOT_BE_EMPTY, PASSWD),
        (
            is_blank(&input.usr_type, USER_TYPE_LEN),
            USER_TYPE_CAN_NOT_BE_EMPTY,
            USRTYPE,
        ),
    ];
    match checks.into_iter().find(|(failed, _, _)| *failed) {
        Some((_, message, field)) => Err(empty(message, field)),
        None => Ok(()),
    }
}

/// COUSR02C UPDATE-USER-INFO — P-COUSR02C-005, P-COUSR02C-007.
///
/// Compares each entered field with the record read for update and moves
/// every one that differs into it [app/cbl/COUSR02C.cbl:L219-234]. Names and
/// user type compare as the `X(n)` fields they are moved into: space-padded,
/// byte for byte, case included. The password is compared only when present,
/// by [`verify_as_entered`], and re-hashed only when it differs (E-21).
/// Returns whether any field differed: `USR-MODIFIED-YES`.
///
/// # Errors
///
/// [`UNABLE_TO_UPDATE_USER`] when the stored hash cannot be verified or the new
/// password cannot be hashed: the update fails before its `REWRITE`, as the
/// rewrite's `WHEN OTHER` does (V-COUSR02C-012).
fn apply_changes(record: &mut UserSecurity, input: &UserUpdateInput) -> Result<bool, DomainError> {
    let mut modified = false;
    let first_name = Alnum::move_from(&input.first_name);
    if first_name != record.sec_usr_fname {
        record.sec_usr_fname = first_name;
        modified = true;
    }
    let last_name = Alnum::move_from(&input.last_name);
    if last_name != record.sec_usr_lname {
        record.sec_usr_lname = last_name;
        modified = true;
    }
    if let Some(password) = input.password.as_deref() {
        let unchanged =
            verify_as_entered(password, &record.password_hash).map_err(|_| unable_to_update())?;
        if !unchanged {
            record.password_hash =
                hash_stored_password(password).map_err(|_| unable_to_update())?;
            modified = true;
        }
    }
    let usr_type = Alnum::move_from(&input.usr_type);
    if usr_type != record.sec_usr_type {
        record.sec_usr_type = usr_type;
        modified = true;
    }
    Ok(modified)
}

/// COUSR02C UPDATE-USER-INFO, READ-USER-SEC-FILE, UPDATE-USER-SEC-FILE —
/// P-COUSR02C-004, P-COUSR02C-005, P-COUSR02C-006, P-COUSR02C-007,
/// V-COUSR02C-004, V-COUSR02C-005, V-COUSR02C-006, V-COUSR02C-007,
/// V-COUSR02C-008, V-COUSR02C-009, V-COUSR02C-010, V-COUSR02C-011,
/// V-COUSR02C-012.
///
/// Runs the empty checks, reads the user for update, compares and moves the
/// changed fields, and rewrites the record when one changed
/// [app/cbl/COUSR02C.cbl:L177-245] [app/cbl/COUSR02C.cbl:L358-390]. The rule
/// commits nothing: the adapter's caller owns the transaction.
///
/// # Errors
///
/// - The empty-field message of the first blank field, kind `Empty`, on its
///   BMS field (V-COUSR02C-004 to V-COUSR02C-008).
/// - [`UNABLE_TO_UPDATE_USER`], kind `Unexpected`, field `FNAME`, when the
///   read for update fails, unknown user included (V-COUSR02C-009, E-46),
///   when the password cannot be verified or hashed, or when the `REWRITE`
///   fails other than `NOTFND` (V-COUSR02C-012).
/// - [`USER_ID_NOT_FOUND`], kind `NotFound`, field `USRIDIN`, when the
///   `REWRITE` finds no user (V-COUSR02C-011).
pub async fn update<S: UserStore>(
    store: &mut S,
    input: UserUpdateInput,
) -> Result<UserUpdateOutcome, DomainError> {
    edit(&input)?;
    // A failed READ UPDATE is not answered with the read's own text, as the
    // fetch answers it: COUSR02C sets that text, still compares against the
    // unread record, finds the entered fields different and issues a REWRITE
    // with no READ UPDATE before it, whose INVREQ replaces the text with
    // 'Unable to Update User...' (E-46, V-COUSR02C-009). Nothing is written.
    let mut record = store
        .read_user_for_update(UserId::move_from(&input.user_id))
        .await
        .map_err(|_| unable_to_update())?;
    if !apply_changes(&mut record, &input)? {
        return Ok(UserUpdateOutcome::NotModified {
            message: PLEASE_MODIFY_TO_UPDATE,
        });
    }
    match store.update_user(record.clone()).await {
        Ok(()) => {
            let message = user_updated_message(&record.sec_usr_id.as_alnum().as_text());
            Ok(UserUpdateOutcome::Updated {
                user: record,
                message,
            })
        }
        Err(PortError::NotFound) => {
            Err(DomainError::not_found(USER_ID_NOT_FOUND).with_field(USRIDIN))
        }
        Err(
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_),
        ) => Err(unable_to_update()),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use super::{UserUpdateInput, UserUpdateOutcome, lookup, update};
    use crate::auth::{hash_stored_password, verify_as_entered};
    use crate::entities::UserSecurity;
    use crate::error::{DomainError, ErrorKind};
    use crate::messages::{
        FIRST_NAME_CAN_NOT_BE_EMPTY, LAST_NAME_CAN_NOT_BE_EMPTY, PASSWORD_CAN_NOT_BE_EMPTY,
        PLEASE_MODIFY_TO_UPDATE, UNABLE_TO_LOOKUP_USER, UNABLE_TO_UPDATE_USER,
        USER_ID_CAN_NOT_BE_EMPTY, USER_ID_NOT_FOUND, USER_TYPE_CAN_NOT_BE_EMPTY,
        user_updated_message,
    };
    use crate::numeric::{Alnum, UserId};
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The password every `DUSRSECJ` user is seeded with — P-CSUSR01Y-009.
    const SEED_PASSWORD: &str = "PASSWORD";

    /// A stored value that is no PHC string, so it cannot be verified.
    const UNUSABLE_HASH: &str = "not-a-phc-string";

    /// The argon2id hash of [`SEED_PASSWORD`], computed once for every test:
    /// each argon2 run is slow in an unoptimised build.
    fn seed_hash() -> &'static str {
        static HASH: OnceLock<String> = OnceLock::new();
        HASH.get_or_init(|| hash_stored_password(SEED_PASSWORD).expect("hash the seed password"))
    }

    fn user_id(text: &str) -> UserId {
        UserId::move_from(text)
    }

    /// `USER0001` as `DUSRSECJ` seeds it — P-CSUSR01Y-009.
    fn user0001() -> UserSecurity {
        UserSecurity {
            sec_usr_id: user_id("USER0001"),
            sec_usr_fname: Alnum::move_from("LAWRENCE"),
            sec_usr_lname: Alnum::move_from("THOMAS"),
            password_hash: seed_hash().to_owned(),
            sec_usr_type: Alnum::move_from("U"),
        }
    }

    fn seeded() -> FakeStore {
        FakeStore::default().with_user(user0001())
    }

    /// `USER0001`'s stored values as the edit form sends them back, the
    /// password absent (E-21).
    fn unchanged_input() -> UserUpdateInput {
        UserUpdateInput {
            user_id: "USER0001".to_owned(),
            first_name: "LAWRENCE".to_owned(),
            last_name: "THOMAS".to_owned(),
            password: None,
            usr_type: "U".to_owned(),
        }
    }

    /// An edit a test applies to the request.
    type InputChange = fn(&mut UserUpdateInput);

    /// An edit a test applies to the expected record.
    type RecordChange = fn(&mut UserSecurity);

    /// [`unchanged_input`] after `change`.
    fn input_with(change: InputChange) -> UserUpdateInput {
        let mut input = unchanged_input();
        change(&mut input);
        input
    }

    fn error(kind: ErrorKind, message: &str, field: &'static str) -> DomainError {
        DomainError::new(kind, message).with_field(field)
    }

    fn not_modified() -> UserUpdateOutcome {
        UserUpdateOutcome::NotModified {
            message: PLEASE_MODIFY_TO_UPDATE,
        }
    }

    fn stored(store: &FakeStore, id: &str) -> Option<UserSecurity> {
        store.users.get(&user_id(id)).cloned()
    }

    /// The updated record and message, or a panic naming the outcome.
    fn updated(outcome: Result<UserUpdateOutcome, DomainError>) -> (UserSecurity, String) {
        match outcome {
            Ok(UserUpdateOutcome::Updated { user, message }) => (user, message),
            other => panic!("expected an update, got {other:?}"),
        }
    }

    #[test]
    fn lookup_rejects_a_blank_user_id_before_any_read() {
        // V-COUSR02C-001: blank or LOW-VALUES, a path segment of spaces.
        let blanks = [
            None,
            Some(""),
            Some(" "),
            Some("        "),
            Some("\0\0\0\0"),
        ];
        for user in blanks {
            let mut store = seeded();
            let result = block_on(lookup(&mut store, user.map(str::to_owned)));
            assert_eq!(
                result,
                Err(error(ErrorKind::Empty, USER_ID_CAN_NOT_BE_EMPTY, "USRIDIN")),
                "{user:?}"
            );
            assert!(store.calls.is_empty(), "{user:?}: {:?}", store.calls);
        }
    }

    #[test]
    fn lookup_reads_the_user_for_update() {
        // P-COUSR02C-002, P-COUSR02C-003: READ UPDATE, then the record shown.
        let mut store = seeded();
        let result = block_on(lookup(&mut store, Some("USER0001".to_owned())));
        assert_eq!(result, Ok(user0001()));
        assert_eq!(store.calls, ["read_user_for_update"]);
    }

    #[test]
    fn lookup_maps_each_read_failure_to_its_message() {
        let not_found = error(ErrorKind::NotFound, USER_ID_NOT_FOUND, "USRIDIN");
        let unexpected = error(ErrorKind::Unexpected, UNABLE_TO_LOOKUP_USER, "FNAME");
        let cases = [
            // V-COUSR02C-002 with P-CSUSR01Y-010, and the key compared with
            // its case kept (E-20).
            ("USER0099", None, not_found.clone()),
            ("user0001", None, not_found),
            // V-COUSR02C-003.
            (
                "USER0001",
                Some(PortError::Unavailable("connection reset".to_owned())),
                unexpected.clone(),
            ),
            ("USER0001", Some(PortError::Contention), unexpected),
        ];
        for (id, failure, expected) in cases {
            let mut store = seeded();
            if let Some(failure) = failure.clone() {
                store.fail_next("read_user_for_update", failure);
            }
            let result = block_on(lookup(&mut store, Some(id.to_owned())));
            assert_eq!(result, Err(expected), "{id} {failure:?}");
        }
    }

    #[test]
    fn update_rejects_the_first_blank_field_in_screen_order() {
        // V-COUSR02C-004 to V-COUSR02C-008, P-COUSR02C-004: one EVALUATE.
        let cases: [(InputChange, &str, &str); 11] = [
            (
                |input| input.user_id.clear(),
                USER_ID_CAN_NOT_BE_EMPTY,
                "USRIDIN",
            ),
            (
                |input| {
                    input.user_id = "        ".to_owned();
                    input.first_name.clear();
                },
                USER_ID_CAN_NOT_BE_EMPTY,
                "USRIDIN",
            ),
            (
                |input| input.first_name.clear(),
                FIRST_NAME_CAN_NOT_BE_EMPTY,
                "FNAME",
            ),
            (
                |input| input.first_name = "\0\0\0".to_owned(),
                FIRST_NAME_CAN_NOT_BE_EMPTY,
                "FNAME",
            ),
            (
                |input| {
                    input.last_name = "   ".to_owned();
                    input.usr_type.clear();
                },
                LAST_NAME_CAN_NOT_BE_EMPTY,
                "LNAME",
            ),
            // E-21: a password present and blank.
            (
                |input| input.password = Some(String::new()),
                PASSWORD_CAN_NOT_BE_EMPTY,
                "PASSWD",
            ),
            (
                |input| input.password = Some("        ".to_owned()),
                PASSWORD_CAN_NOT_BE_EMPTY,
                "PASSWD",
            ),
            (
                |input| {
                    input.password = Some(" ".to_owned());
                    input.usr_type.clear();
                },
                PASSWORD_CAN_NOT_BE_EMPTY,
                "PASSWD",
            ),
            (
                |input| input.usr_type.clear(),
                USER_TYPE_CAN_NOT_BE_EMPTY,
                "USRTYPE",
            ),
            (
                |input| input.usr_type = " ".to_owned(),
                USER_TYPE_CAN_NOT_BE_EMPTY,
                "USRTYPE",
            ),
            // An absent password is the current one, never blank: the user
            // type is the field the EVALUATE stops at.
            (
                |input| {
                    input.password = None;
                    input.usr_type.clear();
                },
                USER_TYPE_CAN_NOT_BE_EMPTY,
                "USRTYPE",
            ),
        ];
        for (change, message, field) in cases {
            let mut store = seeded();
            let input = input_with(change);
            let result = block_on(update(&mut store, input.clone()));
            assert_eq!(
                result,
                Err(error(ErrorKind::Empty, message, field)),
                "{input:?}"
            );
            assert!(store.calls.is_empty(), "{input:?}: {:?}", store.calls);
            assert_eq!(stored(&store, "USER0001"), Some(user0001()));
        }
    }

    #[test]
    fn absent_password_and_no_other_change_is_not_modified() {
        // V-COUSR02C-010, E-21. Entered values compare as their X(n) fields,
        // so trailing spaces change nothing.
        let inputs = [
            unchanged_input(),
            input_with(|input| {
                input.first_name = "LAWRENCE            ".to_owned();
                input.last_name = "THOMAS ".to_owned();
            }),
        ];
        for input in inputs {
            let mut store = seeded();
            let result = block_on(update(&mut store, input.clone()));
            assert_eq!(result, Ok(not_modified()), "{input:?}");
            assert_eq!(store.calls, ["read_user_for_update"], "no write");
            assert_eq!(stored(&store, "USER0001"), Some(user0001()));
        }
    }

    #[test]
    fn absent_password_with_a_changed_name_keeps_the_stored_hash() {
        // P-COUSR02C-005, P-COUSR02C-006, E-21: not re-hashed.
        let mut store = seeded();
        let input = input_with(|input| input.first_name = "LARRY".to_owned());
        let (user, message) = updated(block_on(update(&mut store, input)));

        assert_eq!(message, user_updated_message("USER0001"));
        assert_eq!(user.sec_usr_fname, Alnum::move_from("LARRY"));
        assert_eq!(user.password_hash, seed_hash(), "hash byte-identical");
        assert_eq!(store.calls, ["read_user_for_update", "update_user"]);
        assert_eq!(stored(&store, "USER0001"), Some(user));
    }

    #[test]
    fn same_password_re_entered_is_unchanged() {
        // P-COUSR02C-005, E-21: equal by argon2 verification, hash kept.
        let mut store = seeded();
        let input = input_with(|input| input.password = Some(SEED_PASSWORD.to_owned()));
        assert_eq!(block_on(update(&mut store, input)), Ok(not_modified()));
        assert_eq!(stored(&store, "USER0001"), Some(user0001()));

        let input = input_with(|input| {
            input.password = Some(SEED_PASSWORD.to_owned());
            input.last_name = "THOMSON".to_owned();
        });
        let (user, _) = updated(block_on(update(&mut store, input)));
        assert_eq!(user.sec_usr_lname, Alnum::move_from("THOMSON"));
        assert_eq!(user.password_hash, seed_hash(), "hash not replaced");
    }

    #[test]
    fn new_password_is_re_hashed_with_its_case_kept() {
        // P-COUSR02C-005, E-21, E-20: stored as entered, case unchanged.
        let mut store = seeded();
        let input = input_with(|input| input.password = Some("pass1234".to_owned()));
        let (user, message) = updated(block_on(update(&mut store, input)));

        assert_eq!(message, user_updated_message("USER0001"));
        assert_ne!(user.password_hash, seed_hash());
        let verify = |entered: &str| verify_as_entered(entered, &user.password_hash);
        assert_eq!(verify("pass1234"), Ok(true));
        assert_eq!(verify("PASS1234"), Ok(false));
        assert_eq!(verify(SEED_PASSWORD), Ok(false));
        assert_eq!(stored(&store, "USER0001"), Some(user));
    }

    #[test]
    fn names_and_type_compare_byte_for_byte() {
        // P-COUSR02C-005: case-sensitive compare, each differing field moved.
        let cases: [(InputChange, RecordChange); 3] = [
            (
                |input| input.first_name = "lawrence".to_owned(),
                |user| user.sec_usr_fname = Alnum::move_from("lawrence"),
            ),
            (
                |input| input.last_name = "THOMAS-SMITH-JONES-WILLIAMS".to_owned(),
                |user| user.sec_usr_lname = Alnum::move_from("THOMAS-SMITH-JONES-W"),
            ),
            (
                |input| input.usr_type = "A".to_owned(),
                |user| user.sec_usr_type = Alnum::move_from("A"),
            ),
        ];
        for (change, expect) in cases {
            let mut store = seeded();
            let input = input_with(change);
            let (user, _) = updated(block_on(update(&mut store, input.clone())));
            let mut expected = user0001();
            expect(&mut expected);
            assert_eq!(user, expected, "{input:?}");
            assert_eq!(stored(&store, "USER0001"), Some(expected));
        }
    }

    #[test]
    fn success_message_cuts_the_id_at_its_first_space() {
        // P-COUSR02C-006: SEC-USR-ID DELIMITED BY SPACE.
        let short = UserSecurity {
            sec_usr_id: user_id("ADMIN1"),
            ..user0001()
        };
        let mut store = FakeStore::default().with_user(short);
        let input = input_with(|input| {
            input.user_id = "ADMIN1".to_owned();
            input.usr_type = "A".to_owned();
        });
        let (_, message) = updated(block_on(update(&mut store, input)));
        assert_eq!(message, user_updated_message("ADMIN1"));
    }

    #[test]
    fn a_failed_read_for_update_ends_with_the_rewrite_text() {
        // V-COUSR02C-009, E-46: nothing is written.
        let unexpected = error(ErrorKind::Unexpected, UNABLE_TO_UPDATE_USER, "FNAME");
        let cases = [
            ("USER0099", None),
            (
                "USER0001",
                Some(PortError::Unavailable("timeout".to_owned())),
            ),
            ("USER0001", Some(PortError::Contention)),
        ];
        for (id, failure) in cases {
            let mut store = seeded();
            if let Some(failure) = failure.clone() {
                store.fail_next("read_user_for_update", failure);
            }
            let input = UserUpdateInput {
                user_id: id.to_owned(),
                first_name: "NEW".to_owned(),
                ..unchanged_input()
            };
            let result = block_on(update(&mut store, input));
            assert_eq!(result, Err(unexpected.clone()), "{id} {failure:?}");
            assert_eq!(store.calls, ["read_user_for_update"], "{id} {failure:?}");
            assert_eq!(stored(&store, "USER0001"), Some(user0001()));
            assert_eq!(stored(&store, "USER0099"), None);
        }
    }

    #[test]
    fn rewrite_failures_map_to_their_messages() {
        let cases = [
            // V-COUSR02C-011.
            (
                PortError::NotFound,
                error(ErrorKind::NotFound, USER_ID_NOT_FOUND, "USRIDIN"),
            ),
            // V-COUSR02C-012.
            (
                PortError::Unavailable("disk full".to_owned()),
                error(ErrorKind::Unexpected, UNABLE_TO_UPDATE_USER, "FNAME"),
            ),
        ];
        for (failure, expected) in cases {
            let mut store = seeded();
            store.fail_next("update_user", failure.clone());
            let input = input_with(|input| input.first_name = "LARRY".to_owned());
            let result = block_on(update(&mut store, input));
            assert_eq!(result, Err(expected), "{failure:?}");
            assert_eq!(store.calls, ["read_user_for_update", "update_user"]);
            assert_eq!(stored(&store, "USER0001"), Some(user0001()));
        }
    }

    #[test]
    fn an_unusable_stored_hash_fails_only_a_password_compare() {
        // V-COUSR02C-012: the update stops before its REWRITE.
        let broken = UserSecurity {
            password_hash: UNUSABLE_HASH.to_owned(),
            ..user0001()
        };
        let mut store = FakeStore::default().with_user(broken.clone());
        let input = input_with(|input| input.password = Some(SEED_PASSWORD.to_owned()));
        assert_eq!(
            block_on(update(&mut store, input)),
            Err(error(ErrorKind::Unexpected, UNABLE_TO_UPDATE_USER, "FNAME"))
        );
        assert_eq!(store.calls, ["read_user_for_update"]);

        // E-21: an absent password is never verified, so the name change goes
        // through with the stored value untouched.
        let input = input_with(|input| input.first_name = "LARRY".to_owned());
        let (user, _) = updated(block_on(update(&mut store, input)));
        assert_eq!(user.password_hash, UNUSABLE_HASH);
    }

    #[test]
    fn debug_of_the_input_leaves_out_the_password() {
        let input = input_with(|input| input.password = Some("FAKEPW99".to_owned()));
        let rendered = format!("{input:?}");
        assert!(!rendered.contains("FAKEPW99"), "{rendered}");
        assert!(!rendered.contains("password"), "{rendered}");
        assert!(rendered.contains("\"USER0001\""), "{rendered}");
        assert!(rendered.ends_with(", .. }"), "{rendered}");
    }

    #[test]
    fn rule_futures_are_send_with_a_send_store() {
        // The adapters run the rules on the multi-threaded tokio runtime.
        fn assert_send<T: Send>(_: &T) {}
        let mut store = seeded();
        {
            let fetch = lookup(&mut store, Some("USER0001".to_owned()));
            assert_send(&fetch);
        }
        let write = update(&mut store, unchanged_input());
        assert_send(&write);
    }
}
