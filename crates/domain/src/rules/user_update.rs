//! `COUSR02C` (CU02): show a user for update and rewrite it, served as
//! `GET /admin/users/{id}` ([`lookup`], the ENTER fetch) and
//! `PUT /admin/users/{id}` ([`update`], [`UserUpdateCheck::compare`] and
//! [`rewrite`], PF5) to administrators only.
//!
//! [`lookup`] ports `PROCESS-ENTER-KEY` [app/cbl/COUSR02C.cbl:L143-172] with
//! `READ-USER-SEC-FILE` [app/cbl/COUSR02C.cbl:L320-353]; the update steps port
//! `UPDATE-USER-INFO` [app/cbl/COUSR02C.cbl:L177-245] with the same read and
//! `UPDATE-USER-SEC-FILE` [app/cbl/COUSR02C.cbl:L358-390]. The record is
//! `CSUSR01Y` `SEC-USER-DATA` [app/cpy/CSUSR01Y.cpy:L17-23]; the edited
//! fields are the `COUSR2AI` inputs `USRIDINI X(8)`, `FNAMEI X(20)`,
//! `LNAMEI X(20)`, `PASSWDI X(8)` and `USRTYPEI X(1)` [app/cpy-bms/COUSR02.CPY],
//! whose BMS names the errors carry.
//!
//! # Three update steps
//!
//! An update runs in three steps, because argon2id verification and hashing
//! are memory-hard and CPU-bound and never yield:
//!
//! 1. [`update`], asynchronous: the empty checks and the `READ … UPDATE`. It
//!    does no argon2 work and answers with a [`UserUpdateCheck`] holding the
//!    record read and the fields entered.
//! 2. [`UserUpdateCheck::compare`], synchronous and pure: the field compare,
//!    the password verification and, for a changed password, its hash. It
//!    answers with a [`UserRewrite`].
//! 3. [`rewrite`], asynchronous: the `REWRITE` when a field changed, or
//!    [`PLEASE_MODIFY_TO_UPDATE`] when none did. It does no argon2 work.
//!
//! The adapter awaits [`update`] inside its unit of work and keeps that unit
//! open through the compare, so the row the `READ … UPDATE` locked stays
//! locked, as the COBOL holds it until its `REWRITE`. An asynchronous adapter
//! runs [`UserUpdateCheck::compare`] on a blocking thread pool, such as tokio's
//! `spawn_blocking`, never inline on the executor, where one argon2 run would
//! hold a worker thread and delay every other task on it. When the compare
//! fails, nothing has been written: the adapter rolls its unit of work back
//! and answers the error. Otherwise it awaits [`rewrite`] in the same unit.
//! [`rewrite`] takes only a [`UserRewrite`], which only the compare yields, so
//! no record is rewritten without it.
//!
//! # The password (E-21, E-20)
//!
//! The COBOL moves the stored `SEC-USR-PWD` into the edit field when it shows a
//! user [app/cbl/COUSR02C.cbl:L166-170] and counts the password as changed only
//! when the field then differs from it [app/cbl/COUSR02C.cbl:L227-230]. A
//! stored argon2id hash cannot be shown, so [`lookup`] reads the record
//! without its password, and in an update an absent password stands for that
//! reloaded value: unchanged, never verified or re-hashed. A password that is
//! present goes through the empty edit of [`update`], then
//! [`UserUpdateCheck::compare`] compares it with the stored value by
//! [`verify_as_entered`], case kept, and stores a different one with
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
use crate::entities::{UserProfile, UserSecurity};
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

/// An update whose fields passed the empty checks and whose user was read for
/// update, still to be compared: what [`update`] answers and
/// [`UserUpdateCheck::compare`] completes.
///
/// Every field is owned, so the check is `Send + 'static` and an asynchronous
/// adapter can move it into a blocking thread pool to compare it. `Debug`
/// shows the user ID only: neither the password entered nor the stored hash
/// reaches a log line or a tracing span.
#[must_use = "the fields are compared only by `UserUpdateCheck::compare`"]
pub struct UserUpdateCheck {
    /// The `USRSEC` record read for update by the entered `USRIDINI`.
    record: UserSecurity,
    /// The `COUSR2AI` fields PF5 sent, the password included.
    input: UserUpdateInput,
}

impl fmt::Debug for UserUpdateCheck {
    /// The user ID read only, ended with `..` to show that the fields entered
    /// and the rest of the record are left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserUpdateCheck")
            .field("user_id", &self.record.sec_usr_id)
            .finish_non_exhaustive()
    }
}

impl UserUpdateCheck {
    /// COUSR02C UPDATE-USER-INFO — P-COUSR02C-005, P-COUSR02C-007,
    /// V-COUSR02C-010, V-COUSR02C-012.
    ///
    /// The second step of an update: compares each entered field with the
    /// record read and moves every one that differs into it
    /// [app/cbl/COUSR02C.cbl:L219-234], setting `USR-MODIFIED-YES`. A present
    /// password is verified against the stored hash, case kept, and hashed
    /// when it differs; an absent one is neither verified nor re-hashed
    /// (E-21). [`rewrite`] writes the [`UserRewrite`] this answers.
    ///
    /// Synchronous: argon2id verification and hashing are memory-hard and
    /// CPU-bound and never yield. An asynchronous adapter calls this on a
    /// blocking thread pool, such as tokio's `spawn_blocking`, never inline on
    /// the executor, while it keeps the unit of work of [`update`] open with
    /// the row read for update still locked. On an error it rolls that unit
    /// back; nothing has been written.
    ///
    /// # Errors
    ///
    /// [`UNABLE_TO_UPDATE_USER`], kind `Unexpected`, field `FNAME`, when the
    /// stored hash cannot be verified or the new password cannot be hashed
    /// (V-COUSR02C-012).
    pub fn compare(self) -> Result<UserRewrite, DomainError> {
        let Self { mut record, input } = self;
        let modified = apply_changes(&mut record, &input)?;
        Ok(UserRewrite { record, modified })
    }
}

/// A compared update, still to be written: what
/// [`UserUpdateCheck::compare`] answers and [`rewrite`] completes.
///
/// `Debug` shows the user ID and whether a field changed: the stored hash
/// never reaches a log line or a tracing span.
#[must_use = "the record is rewritten only by `rewrite`"]
pub struct UserRewrite {
    /// The record read for update, every changed field moved into it.
    record: UserSecurity,
    /// `USR-MODIFIED-YES`: whether any entered field differed.
    modified: bool,
}

impl fmt::Debug for UserRewrite {
    /// The user ID and the modified flag, ended with `..` to show that the
    /// rest of the record is left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserRewrite")
            .field("user_id", &self.record.sec_usr_id)
            .field("modified", &self.modified)
            .finish_non_exhaustive()
    }
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
/// read with `READ … UPDATE` [app/cbl/COUSR02C.cbl:L322-331]. The record is
/// read without its password: the stored hash cannot be shown (E-21), and
/// nothing here compares it.
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
) -> Result<UserProfile, DomainError> {
    let user_id = user_id.unwrap_or_default();
    if is_blank(&user_id, SEC_USR_ID_LEN) {
        return Err(empty(USER_ID_CAN_NOT_BE_EMPTY, USRIDIN));
    }
    store
        .read_user_profile_for_update(UserId::move_from(&user_id))
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

/// COUSR02C UPDATE-USER-INFO, READ-USER-SEC-FILE — P-COUSR02C-004,
/// V-COUSR02C-004, V-COUSR02C-005, V-COUSR02C-006, V-COUSR02C-007,
/// V-COUSR02C-008, V-COUSR02C-009.
///
/// The first step of an update by PF5 [app/cbl/COUSR02C.cbl:L177-213]: runs
/// the empty checks, then reads the user for update with `READ … UPDATE`
/// [app/cbl/COUSR02C.cbl:L320-353]. The record read and the fields entered
/// come back as a [`UserUpdateCheck`], uncompared: this function does no
/// argon2 work. The adapter awaits it inside its unit of work, keeps that unit
/// open, and runs [`UserUpdateCheck::compare`] where blocking work belongs —
/// for an asynchronous adapter a blocking thread pool, such as tokio's
/// `spawn_blocking`, never inline on the executor. The rule commits nothing:
/// the adapter's caller owns the transaction.
///
/// # Errors
///
/// - The empty-field message of the first blank field, kind `Empty`, on its
///   BMS field (V-COUSR02C-004 to V-COUSR02C-008); nothing is read.
/// - [`UNABLE_TO_UPDATE_USER`], kind `Unexpected`, field `FNAME`, when the
///   read for update fails, unknown user included (V-COUSR02C-009, E-46).
///
/// A password that cannot be verified or hashed is answered by
/// [`UserUpdateCheck::compare`], and a failed `REWRITE` by [`rewrite`].
pub async fn update<S: UserStore>(
    store: &mut S,
    input: UserUpdateInput,
) -> Result<UserUpdateCheck, DomainError> {
    edit(&input)?;
    // A failed READ UPDATE is not answered with the read's own text, as the
    // fetch answers it: COUSR02C sets that text, still compares against the
    // unread record, finds the entered fields different and issues a REWRITE
    // with no READ UPDATE before it, whose INVREQ replaces the text with
    // 'Unable to Update User...' (E-46, V-COUSR02C-009). Nothing is written.
    let record = store
        .read_user_for_update(UserId::move_from(&input.user_id))
        .await
        .map_err(|_| unable_to_update())?;
    Ok(UserUpdateCheck { record, input })
}

/// COUSR02C UPDATE-USER-INFO, UPDATE-USER-SEC-FILE — P-COUSR02C-005,
/// P-COUSR02C-006, V-COUSR02C-010, V-COUSR02C-011, V-COUSR02C-012.
///
/// The third step of an update: when no field differed, nothing is written
/// and the program shows [`PLEASE_MODIFY_TO_UPDATE`]
/// [app/cbl/COUSR02C.cbl:L236-243]; otherwise the record
/// [`UserUpdateCheck::compare`] changed is rewritten
/// [app/cbl/COUSR02C.cbl:L358-390], and `NORMAL` answers
/// `User <id> has been updated ...`, the ID cut at its first space. It does no
/// argon2 work, and it commits nothing: the adapter's caller owns the
/// transaction.
///
/// # Errors
///
/// - [`USER_ID_NOT_FOUND`], kind `NotFound`, field `USRIDIN`, when the
///   `REWRITE` finds no user (V-COUSR02C-011).
/// - [`UNABLE_TO_UPDATE_USER`], kind `Unexpected`, field `FNAME`, when the
///   `REWRITE` fails otherwise (V-COUSR02C-012).
pub async fn rewrite<S: UserStore>(
    store: &mut S,
    rewrite: UserRewrite,
) -> Result<UserUpdateOutcome, DomainError> {
    let UserRewrite { record, modified } = rewrite;
    if !modified {
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
    use std::thread;

    use super::{
        UserRewrite, UserUpdateCheck, UserUpdateInput, UserUpdateOutcome, lookup, rewrite, update,
    };
    use crate::auth::{hash_stored_password, verify_as_entered};
    use crate::entities::{UserProfile, UserSecurity};
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

    /// The update's first step only: the empty checks and the read for
    /// update.
    fn check(
        store: &mut FakeStore,
        input: UserUpdateInput,
    ) -> Result<UserUpdateCheck, DomainError> {
        block_on(update(store, input))
    }

    /// Updates with `input`: all three steps, the read, the compare and the
    /// rewrite.
    fn run(
        store: &mut FakeStore,
        input: UserUpdateInput,
    ) -> Result<UserUpdateOutcome, DomainError> {
        let rewritten = check(store, input)?.compare()?;
        block_on(rewrite(store, rewritten))
    }

    /// What a compare answers, a re-hashed password replaced by a marker once
    /// it is checked to verify `entered`: each hash draws its own salt, so two
    /// runs never store the same bytes. A hash kept from `stored` is kept as
    /// it is.
    fn compared(
        result: Result<UserRewrite, DomainError>,
        stored: &UserSecurity,
        entered: Option<&str>,
    ) -> Result<(UserSecurity, bool), DomainError> {
        result.map(|rewritten| {
            let UserRewrite {
                mut record,
                modified,
            } = rewritten;
            if record.password_hash != stored.password_hash {
                let entered = entered.expect("only an entered password is re-hashed");
                let verified = verify_as_entered(entered, &record.password_hash);
                assert_eq!(verified, Ok(true), "the new hash of {entered:?}");
                "re-hashed".clone_into(&mut record.password_hash);
            }
            (record, modified)
        })
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
        // P-COUSR02C-002, P-COUSR02C-003: READ UPDATE, then the record shown,
        // read without its password (E-21).
        let mut store = seeded();
        let result = block_on(lookup(&mut store, Some("USER0001".to_owned())));
        assert_eq!(result, Ok(UserProfile::from(&user0001())));
        assert_eq!(store.calls, ["read_user_profile_for_update"]);
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
                store.fail_next("read_user_profile_for_update", failure);
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
            let result = run(&mut store, input.clone());
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
            let result = run(&mut store, input.clone());
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
        let (user, message) = updated(run(&mut store, input));

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
        assert_eq!(run(&mut store, input), Ok(not_modified()));
        assert_eq!(stored(&store, "USER0001"), Some(user0001()));

        let input = input_with(|input| {
            input.password = Some(SEED_PASSWORD.to_owned());
            input.last_name = "THOMSON".to_owned();
        });
        let (user, _) = updated(run(&mut store, input));
        assert_eq!(user.sec_usr_lname, Alnum::move_from("THOMSON"));
        assert_eq!(user.password_hash, seed_hash(), "hash not replaced");
    }

    #[test]
    fn new_password_is_re_hashed_with_its_case_kept() {
        // P-COUSR02C-005, E-21, E-20: stored as entered, case unchanged.
        let mut store = seeded();
        let input = input_with(|input| input.password = Some("pass1234".to_owned()));
        let (user, message) = updated(run(&mut store, input));

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
            let (user, _) = updated(run(&mut store, input.clone()));
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
        let (_, message) = updated(run(&mut store, input));
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
            let result = run(&mut store, input);
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
            let result = run(&mut store, input);
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
            run(&mut store, input),
            Err(error(ErrorKind::Unexpected, UNABLE_TO_UPDATE_USER, "FNAME"))
        );
        assert_eq!(store.calls, ["read_user_for_update"]);

        // E-21: an absent password is never verified, so the name change goes
        // through with the stored value untouched.
        let input = input_with(|input| input.first_name = "LARRY".to_owned());
        let (user, _) = updated(run(&mut store, input));
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
    fn the_read_step_leaves_the_password_uncompared() {
        // V-COUSR02C-012 before the REWRITE: the read step answers with a
        // check even though the stored hash is unusable, so it verified
        // nothing; only the compare rejects, and nothing is rewritten.
        let broken = UserSecurity {
            password_hash: UNUSABLE_HASH.to_owned(),
            ..user0001()
        };
        let mut store = FakeStore::default().with_user(broken.clone());
        let input = input_with(|input| input.password = Some(SEED_PASSWORD.to_owned()));

        let pending = check(&mut store, input)
            .unwrap_or_else(|error| panic!("USER0001 is read for update: {error:?}"));

        assert_eq!(store.calls, ["read_user_for_update"]);
        assert_eq!(
            pending.compare().err(),
            Some(error(ErrorKind::Unexpected, UNABLE_TO_UPDATE_USER, "FNAME"))
        );
        assert_eq!(store.calls, ["read_user_for_update"], "no write");
        assert_eq!(stored(&store, "USER0001"), Some(broken));
    }

    #[test]
    fn a_compare_on_another_thread_answers_as_inline() {
        // An asynchronous adapter compares on a blocking thread pool. The
        // bounds of `thread::spawn` check that the check and the rewrite are
        // Send + 'static, and the answer there is the one given inline.
        let broken = UserSecurity {
            password_hash: UNUSABLE_HASH.to_owned(),
            ..user0001()
        };
        let cases: [(UserSecurity, InputChange); 4] = [
            (user0001(), |input| input.first_name = "LARRY".to_owned()),
            (user0001(), |input| {
                input.password = Some(SEED_PASSWORD.to_owned());
            }),
            (user0001(), |input| {
                input.password = Some("pass1234".to_owned());
            }),
            (broken, |input| {
                input.password = Some(SEED_PASSWORD.to_owned());
            }),
        ];
        for (record, change) in cases {
            let input = input_with(change);
            let case = format!("{record:?} {input:?}");
            let entered = input.password.clone();
            let mut store = FakeStore::default().with_user(record.clone());
            let inline = compared(
                check(&mut store, input.clone()).and_then(UserUpdateCheck::compare),
                &record,
                entered.as_deref(),
            );
            let pending = check(&mut store, input)
                .unwrap_or_else(|error| panic!("{case}: read for update: {error:?}"));
            let elsewhere = thread::spawn(move || pending.compare())
                .join()
                .unwrap_or_else(|_| panic!("{case}: the comparing thread panicked"));
            assert_eq!(
                compared(elsewhere, &record, entered.as_deref()),
                inline,
                "{case}"
            );
        }
    }

    #[test]
    fn debug_of_the_check_and_the_rewrite_omits_the_password_and_the_hash() {
        let mut store = seeded();
        let input = input_with(|input| input.password = Some("FAKEPW99".to_owned()));
        let pending = check(&mut store, input)
            .unwrap_or_else(|error| panic!("USER0001 is read for update: {error:?}"));
        let rendered_check = format!("{pending:?}");
        let rewritten = pending
            .compare()
            .unwrap_or_else(|error| panic!("FAKEPW99 is compared: {error:?}"));
        let rendered_rewrite = format!("{rewritten:?}");
        let key = user_id("USER0001");

        assert_eq!(
            rendered_check,
            format!("UserUpdateCheck {{ user_id: {key:?}, .. }}")
        );
        assert_eq!(
            rendered_rewrite,
            format!("UserRewrite {{ user_id: {key:?}, modified: true, .. }}")
        );
        let hidden = [
            "FAKEPW99",
            "argon2",
            seed_hash(),
            rewritten.record.password_hash.as_str(),
        ];
        for rendered in [&rendered_check, &rendered_rewrite] {
            for text in hidden {
                assert!(!rendered.contains(text), "{text} in {rendered}");
            }
        }
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
        {
            let read = update(&mut store, unchanged_input());
            assert_send(&read);
        }
        let rewritten = check(&mut store, unchanged_input())
            .and_then(UserUpdateCheck::compare)
            .unwrap_or_else(|error| panic!("USER0001 is read and compared: {error:?}"));
        let write = rewrite(&mut store, rewritten);
        assert_send(&write);
    }
}
