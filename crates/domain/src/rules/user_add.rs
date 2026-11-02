//! `COUSR01C` (CU01): add a user to the `USRSEC` file, `POST /admin/users`.
//!
//! Ports `PROCESS-ENTER-KEY` [app/cbl/COUSR01C.cbl:L115-160] and
//! `WRITE-USER-SEC-FILE` [app/cbl/COUSR01C.cbl:L238-274]: one `EVALUATE` of
//! five empty-field edits in screen order, the `MOVE`s of the entered values
//! into `SEC-USER-DATA` (`CSUSR01Y` [app/cpy/CSUSR01Y.cpy:L17-23]) and the
//! `WRITE` with its three outcomes. [`edit`] is the `EVALUATE`.
//!
//! # Two steps
//!
//! The rule runs in two steps, because argon2id hashing is memory-hard and
//! CPU-bound and never yields:
//!
//! 1. [`prepare`], synchronous and pure: [`edit`], the `MOVE`s and the hash of
//!    the password. It reads no store and answers with a [`PreparedUser`].
//! 2. [`add`], asynchronous: the `WRITE` of that record and the message its
//!    outcome selects. It does no argon2 work.
//!
//! An asynchronous adapter runs [`prepare`] on a blocking thread pool, such as
//! tokio's `spawn_blocking`, never inline on the executor, where one hash
//! would hold a worker thread and delay every other task on it. The step
//! needs no store, so the adapter runs it before it opens its unit of work,
//! then awaits [`add`] inside that unit. [`add`] takes only a
//! [`PreparedUser`], which only [`prepare`] yields, so no user is written
//! without the edits.
//!
//! # Values as entered (E-20)
//!
//! The program moves every field as entered (P-COUSR01C-003): it upper-cases
//! neither the user ID nor the password, and it accepts any non-blank user
//! type, not only `A` and `U`. Sign-on (`COSGN00C`) upper-cases what is typed
//! before it reads the user and compares the password, so a user ID or
//! password stored here in lower or mixed case never signs on
//! (P-COSGN00C-013). That is reproduced, not repaired.
//!
//! The one departure is the password itself: the record holds the argon2id
//! hash of the exact `PIC X(08)` value the COBOL stores in plaintext, which
//! [`hash_stored_password`] computes.
//!
//! # What stays with the client
//!
//! PF3, PF4, the screen header and the map send and receive
//! (P-COUSR01C-001, P-COUSR01C-006 to P-COUSR01C-011) become navigation and
//! form state in the web client, so this module has no counterpart for them.
//! The COBOL reached this program only through the admin menu; the API admits
//! only administrators to the endpoint.

use std::fmt;

use crate::auth::{SEC_USR_ID_LEN, SEC_USR_PWD_LEN, hash_stored_password};
use crate::entities::UserSecurity;
use crate::error::DomainError;
use crate::messages::{
    FIRST_NAME_CAN_NOT_BE_EMPTY, LAST_NAME_CAN_NOT_BE_EMPTY, PASSWORD_CAN_NOT_BE_EMPTY,
    UNABLE_TO_ADD_USER, USER_ID_ALREADY_EXIST, USER_ID_CAN_NOT_BE_EMPTY,
    USER_TYPE_CAN_NOT_BE_EMPTY, user_added_message,
};
use crate::numeric::{Alnum, UserId};
use crate::ports::{PortError, UserStore};

/// Width of the name fields: `FNAMEI` and `LNAMEI PIC X(20)` of the map
/// [app/cpy-bms/COUSR01.CPY:L60, L66], the widths of `SEC-USR-FNAME` and
/// `SEC-USR-LNAME` (P-CSUSR01Y-002, P-CSUSR01Y-003).
const NAME_LEN: usize = 20;

/// Width of `USRTYPEI PIC X(1)` [app/cpy-bms/COUSR01.CPY:L84] and of
/// `SEC-USR-TYPE` (P-CSUSR01Y-005).
const USRTYPE_LEN: usize = 1;

/// BMS field of the first-name edit and of the write failure (V-COUSR01C-001,
/// V-COUSR01C-007).
const FIELD_FNAME: &str = "FNAME";

/// BMS field of the last-name edit (V-COUSR01C-002).
const FIELD_LNAME: &str = "LNAME";

/// BMS field of the user-ID edit and of the duplicate key (V-COUSR01C-003,
/// V-COUSR01C-006).
const FIELD_USERID: &str = "USERID";

/// BMS field of the password edit (V-COUSR01C-004).
const FIELD_PASSWD: &str = "PASSWD";

/// BMS field of the user-type edit (V-COUSR01C-005).
const FIELD_USRTYPE: &str = "USRTYPE";

/// The five input fields of the `COUSR1A` map, as the request body carries
/// them: `FNAMEI`, `LNAMEI`, `USERIDI`, `PASSWDI` and `USRTYPEI`
/// [app/cpy-bms/COUSR01.CPY:L60-84].
///
/// Each value is taken as entered; the rule moves it into its `PIC X(n)` field.
/// A field the caller leaves out is the empty string. `Debug` omits the
/// password, so it never reaches a log line or a tracing span.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct UserAddInput {
    /// `FNAMEI PIC X(20)`, moved to `SEC-USR-FNAME`.
    pub first_name: String,
    /// `LNAMEI PIC X(20)`, moved to `SEC-USR-LNAME`.
    pub last_name: String,
    /// `USERIDI PIC X(8)`, moved to the key `SEC-USR-ID` with its case kept.
    pub user_id: String,
    /// `PASSWDI PIC X(8)`, stored as the argon2id hash of `SEC-USR-PWD` with
    /// its case kept.
    pub password: String,
    /// `USRTYPEI PIC X(1)`, moved to `SEC-USR-TYPE`; its value is not edited.
    pub usr_type: String,
}

impl fmt::Debug for UserAddInput {
    /// Every field but the password, ended with `..` to show that one is left
    /// out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserAddInput")
            .field("first_name", &self.first_name)
            .field("last_name", &self.last_name)
            .field("user_id", &self.user_id)
            .field("usr_type", &self.usr_type)
            .finish_non_exhaustive()
    }
}

/// A user written to `USRSEC`: the record as stored and the success text the
/// screen shows, `User <id> has been added ...` (P-COUSR01C-004).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserAdded {
    /// The record written: every field as entered, moved to its `CSUSR01Y`
    /// width, and the hash of the password.
    pub user: UserSecurity,
    /// [`user_added_message`] of the stored `SEC-USR-ID`.
    pub message: String,
}

/// COUSR01C PROCESS-ENTER-KEY — P-COUSR01C-002, V-COUSR01C-001,
/// V-COUSR01C-002, V-COUSR01C-003, V-COUSR01C-004, V-COUSR01C-005
///
/// The empty-field edits [app/cbl/COUSR01C.cbl:L117-151]: first name, last
/// name, user ID, password and user type, in that order. A field fails when,
/// moved into its map field, it is all spaces or all low-values
/// (`= SPACES OR LOW-VALUES`). The edits are one `EVALUATE`, so only the first
/// failing field is reported, with the cursor on it. The user type's value is
/// not checked.
///
/// # Errors
///
/// An [`ErrorKind::Empty`](crate::error::ErrorKind::Empty) error carrying the
/// message and BMS field of the first blank field: `First Name can NOT be
/// empty...` (`FNAME`), `Last Name can NOT be empty...` (`LNAME`), `User ID
/// can NOT be empty...` (`USERID`), `Password can NOT be empty...` (`PASSWD`)
/// or `User Type can NOT be empty...` (`USRTYPE`).
pub fn edit(input: &UserAddInput) -> Result<(), DomainError> {
    let edits = [
        (
            blank::<NAME_LEN>(&input.first_name),
            FIRST_NAME_CAN_NOT_BE_EMPTY,
            FIELD_FNAME,
        ),
        (
            blank::<NAME_LEN>(&input.last_name),
            LAST_NAME_CAN_NOT_BE_EMPTY,
            FIELD_LNAME,
        ),
        (
            blank::<SEC_USR_ID_LEN>(&input.user_id),
            USER_ID_CAN_NOT_BE_EMPTY,
            FIELD_USERID,
        ),
        (
            blank::<SEC_USR_PWD_LEN>(&input.password),
            PASSWORD_CAN_NOT_BE_EMPTY,
            FIELD_PASSWD,
        ),
        (
            blank::<USRTYPE_LEN>(&input.usr_type),
            USER_TYPE_CAN_NOT_BE_EMPTY,
            FIELD_USRTYPE,
        ),
    ];
    let first_blank = edits
        .into_iter()
        .find_map(|(is_blank, message, field)| is_blank.then_some((message, field)));
    match first_blank {
        Some((message, field)) => Err(DomainError::empty(message).with_field(field)),
        None => Ok(()),
    }
}

/// A user that passed the edits, moved into `SEC-USER-DATA` with its password
/// hashed, and not yet written: what [`prepare`] answers and [`add`] writes.
///
/// Its one field is owned, so the prepared user is `Send + 'static` and an
/// asynchronous adapter can move it out of the blocking thread pool that
/// prepared it. `Debug` shows the user ID only: neither the hash nor the other
/// fields reach a log line or a tracing span.
#[must_use = "the user is written only by `add`"]
pub struct PreparedUser {
    /// The record to write: every field as entered, moved to its `CSUSR01Y`
    /// width, and the argon2id hash of the password.
    user: UserSecurity,
}

impl fmt::Debug for PreparedUser {
    /// The user ID only, ended with `..` to show that the rest of the record
    /// is left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedUser")
            .field("user_id", &self.user.sec_usr_id)
            .finish_non_exhaustive()
    }
}

/// COUSR01C PROCESS-ENTER-KEY — P-COUSR01C-002, P-COUSR01C-003,
/// V-COUSR01C-001 to V-COUSR01C-005, V-COUSR01C-007
///
/// The first step of an add by the ENTER key [app/cbl/COUSR01C.cbl:L115-160]:
/// runs [`edit`]; when every field is entered, moves the fields into
/// `SEC-USER-DATA` as entered [app/cbl/COUSR01C.cbl:L153-158]. Each `MOVE`
/// left-justifies its value in the `CSUSR01Y` field, pads it with spaces and
/// cuts it on the right; no case is changed (E-20). The password is hashed by
/// [`hash_stored_password`], which performs its `PIC X(08)` move. [`add`]
/// writes the [`PreparedUser`] this answers.
///
/// Synchronous: argon2id hashing is memory-hard and CPU-bound and never
/// yields. The step reads no store, so an asynchronous adapter calls it on a
/// blocking thread pool, such as tokio's `spawn_blocking`, before it opens its
/// unit of work, never inline on the executor.
///
/// # Errors
///
/// - The [`edit`] error of the first blank field; nothing is hashed.
/// - `Unable to Add User...` with field `FNAME`, an
///   [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) error,
///   when the password cannot be hashed.
pub fn prepare(input: UserAddInput) -> Result<PreparedUser, DomainError> {
    edit(&input)?;
    let UserAddInput {
        first_name,
        last_name,
        user_id,
        password,
        usr_type,
    } = input;
    // Hashing stands in for `MOVE PASSWDI TO SEC-USR-PWD`, which cannot fail
    // in the COBOL. Its failure leaves the user unwritten, so it answers with
    // the program's own message for a user that could not be added.
    let password_hash = hash_stored_password(&password).map_err(|_| unable_to_add())?;
    Ok(PreparedUser {
        user: UserSecurity {
            sec_usr_id: UserId::move_from(&user_id),
            sec_usr_fname: Alnum::move_from(&first_name),
            sec_usr_lname: Alnum::move_from(&last_name),
            password_hash,
            sec_usr_type: Alnum::move_from(&usr_type),
        },
    })
}

/// COUSR01C WRITE-USER-SEC-FILE — P-COUSR01C-004, P-COUSR01C-005,
/// V-COUSR01C-006, V-COUSR01C-007
///
/// The second step of an add: `WRITE`s the record [`prepare`] built
/// [app/cbl/COUSR01C.cbl:L238-274]. It does no argon2 work. On `NORMAL` the
/// result carries the stored record and `User <id> has been added ...`, the
/// ID cut at its first space (`DELIMITED BY SPACE`).
///
/// # Errors
///
/// - `User ID already exist...` with field `USERID`, an
///   [`ErrorKind::Duplicate`](crate::error::ErrorKind::Duplicate) error, when
///   the write answers `DUPKEY` or `DUPREC` ([`PortError::Duplicate`]).
/// - `Unable to Add User...` with field `FNAME`, an
///   [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) error, for
///   any other write outcome.
pub async fn add<S: UserStore>(
    store: &mut S,
    user: PreparedUser,
) -> Result<UserAdded, DomainError> {
    let PreparedUser { user } = user;
    match store.insert_user(user.clone()).await {
        Ok(()) => {
            let message = user_added_message(&user.sec_usr_id.as_alnum().as_text());
            Ok(UserAdded { user, message })
        }
        Err(PortError::Duplicate) => {
            Err(DomainError::duplicate(USER_ID_ALREADY_EXIST).with_field(FIELD_USERID))
        }
        Err(_) => Err(unable_to_add()),
    }
}

/// The COBOL condition `= SPACES OR LOW-VALUES` on `entered` moved into an
/// `X(N)` map field.
fn blank<const N: usize>(entered: &str) -> bool {
    Alnum::<N>::move_from(entered).is_blank()
}

/// `WRITE-USER-SEC-FILE` `WHEN OTHER`: `Unable to Add User...` with the cursor
/// on the first name — V-COUSR01C-007 [app/cbl/COUSR01C.cbl:L267-273].
fn unable_to_add() -> DomainError {
    DomainError::unexpected(UNABLE_TO_ADD_USER).with_field(FIELD_FNAME)
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::{PreparedUser, UserAddInput, UserAdded, add, edit, prepare};
    use crate::auth::{verify_as_entered, verify_signon};
    use crate::entities::UserSecurity;
    use crate::error::{DomainError, ErrorKind};
    use crate::messages::{
        FIRST_NAME_CAN_NOT_BE_EMPTY, LAST_NAME_CAN_NOT_BE_EMPTY, PASSWORD_CAN_NOT_BE_EMPTY,
        UNABLE_TO_ADD_USER, USER_ID_ALREADY_EXIST, USER_ID_CAN_NOT_BE_EMPTY,
        USER_TYPE_CAN_NOT_BE_EMPTY,
    };
    use crate::numeric::{Alnum, UserId};
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The text of a field of `width` bytes holding only low-values.
    fn low_values(width: usize) -> String {
        "\0".repeat(width)
    }

    /// A new user, absent from the seeded `USRSEC` rows (P-CSUSR01Y-010).
    fn new_user() -> UserAddInput {
        UserAddInput {
            first_name: "Test".to_owned(),
            last_name: "User".to_owned(),
            user_id: "TEST0001".to_owned(),
            password: "Pass1234".to_owned(),
            usr_type: "U".to_owned(),
        }
    }

    /// The seeded `ADMIN001` row (P-CSUSR01Y-008) with a stand-in hash.
    fn admin001() -> UserSecurity {
        UserSecurity {
            sec_usr_id: UserId::move_from("ADMIN001"),
            sec_usr_fname: Alnum::move_from("MARGARET"),
            sec_usr_lname: Alnum::move_from("GOLD"),
            password_hash: "argon2id-fixture-hash".to_owned(),
            sec_usr_type: Alnum::move_from("A"),
        }
    }

    /// The input with field `index` (0 first name … 4 user type) set to
    /// `value`.
    fn with_field(mut input: UserAddInput, index: usize, value: &str) -> UserAddInput {
        let field = match index {
            0 => &mut input.first_name,
            1 => &mut input.last_name,
            2 => &mut input.user_id,
            3 => &mut input.password,
            _ => &mut input.usr_type,
        };
        value.clone_into(field);
        input
    }

    /// Adds `input`: both steps, the preparation and then the write.
    fn run(store: &mut FakeStore, input: UserAddInput) -> Result<UserAdded, DomainError> {
        let user = prepare(input)?;
        block_on(add(store, user))
    }

    fn added(store: &mut FakeStore, input: UserAddInput) -> UserAdded {
        run(store, input).expect("the user is added")
    }

    /// What a preparation answers, once its hash is checked to verify
    /// `password` as entered and then cleared: two hashes of one password
    /// never match byte for byte, because each draws its own salt.
    fn prepared_outcome(
        result: Result<PreparedUser, DomainError>,
        password: &str,
    ) -> Result<UserSecurity, DomainError> {
        result.map(|PreparedUser { mut user }| {
            let verified = verify_as_entered(password, &user.password_hash);
            assert_eq!(verified, Ok(true), "the hash of {password:?} as entered");
            user.password_hash.clear();
            user
        })
    }

    #[test]
    fn first_blank_field_wins_in_screen_order() {
        // V-COUSR01C-001 to V-COUSR01C-005, P-COUSR01C-002: field `index` is
        // blank, every earlier field is entered and every later field is blank
        // too, so only the one EVALUATE branch for `index` answers.
        let expected = [
            (FIRST_NAME_CAN_NOT_BE_EMPTY, "FNAME", 20),
            (LAST_NAME_CAN_NOT_BE_EMPTY, "LNAME", 20),
            (USER_ID_CAN_NOT_BE_EMPTY, "USERID", 8),
            (PASSWORD_CAN_NOT_BE_EMPTY, "PASSWD", 8),
            (USER_TYPE_CAN_NOT_BE_EMPTY, "USRTYPE", 1),
        ];
        for (index, (message, field, width)) in expected.into_iter().enumerate() {
            for blank in [String::new(), " ".repeat(width), low_values(width)] {
                let input = (index..expected.len())
                    .fold(new_user(), |input, later| with_field(input, later, &blank));
                let mut store = FakeStore::default();

                let error = run(&mut store, input.clone()).expect_err("a blank field");

                assert_eq!(error, DomainError::empty(message).with_field(field));
                assert_eq!(error.kind, ErrorKind::Empty);
                assert_eq!(edit(&input), Err(error), "{field} {blank:?}");
                assert!(store.calls.is_empty(), "{field}: nothing is written");
            }
        }
    }

    #[test]
    fn every_field_entered_passes_the_edits() {
        // P-COUSR01C-002 WHEN OTHER; P-COUSR01C-003: the user type is any
        // non-blank byte, and a value is blank only when its whole field is.
        assert_eq!(edit(&new_user()), Ok(()));
        assert_eq!(edit(&with_field(new_user(), 4, "x")), Ok(()));
        assert_eq!(edit(&with_field(new_user(), 0, "   Ada")), Ok(()));
    }

    #[test]
    fn adds_the_user_with_the_success_text() {
        // P-COUSR01C-003, P-COUSR01C-004.
        let mut store = FakeStore::default().with_user(admin001());

        let result = added(&mut store, new_user());

        assert_eq!(result.message, "User TEST0001 has been added ...");
        assert_eq!(store.calls, ["insert_user"]);
        assert_eq!(store.users.get(&result.user.sec_usr_id), Some(&result.user));
        assert_eq!(store.users.len(), 2);
        let user = &result.user;
        assert_eq!(user.sec_usr_id.as_bytes(), b"TEST0001");
        assert_eq!(user.sec_usr_fname.as_bytes(), b"Test                ");
        assert_eq!(user.sec_usr_lname.as_bytes(), b"User                ");
        assert_eq!(user.sec_usr_type.as_bytes(), b"U");
        assert!(!user.is_admin());
        assert!(verify_as_entered("Pass1234", &user.password_hash).expect("a usable hash"));
        assert!(!verify_as_entered("PASS1234", &user.password_hash).expect("a usable hash"));
    }

    #[test]
    fn case_is_kept_so_lower_case_entries_never_sign_on() {
        // P-COUSR01C-003, E-20: the ID and the password are stored as typed.
        let mut store = FakeStore::default();
        let input = UserAddInput {
            user_id: "test0002".to_owned(),
            password: "pass1234".to_owned(),
            ..new_user()
        };

        let result = added(&mut store, input);

        let stored = &result.user;
        assert_eq!(stored.sec_usr_id.as_bytes(), b"test0002");
        assert_ne!(stored.sec_usr_id, UserId::move_from("TEST0002"));
        assert!(!store.users.contains_key(&UserId::move_from("TEST0002")));
        assert_eq!(result.message, "User test0002 has been added ...");
        assert!(verify_as_entered("pass1234", &stored.password_hash).expect("a usable hash"));
        assert!(!verify_signon("pass1234", &stored.password_hash).expect("a usable hash"));
    }

    #[test]
    fn moves_pad_and_cut_to_the_record_widths() {
        // P-COUSR01C-003, P-COUSR01C-004: each MOVE cuts on the right at the
        // CSUSR01Y width, and the success text stops at the ID's first space.
        let mut store = FakeStore::default();
        let input = UserAddInput {
            first_name: "Bartholomew-Alexander-Smith".to_owned(),
            user_id: "LONGUSERID".to_owned(),
            password: "Longpass99".to_owned(),
            usr_type: "AU".to_owned(),
            ..new_user()
        };

        let result = added(&mut store, input);

        let user = &result.user;
        assert_eq!(user.sec_usr_fname.as_bytes(), b"Bartholomew-Alexande");
        assert_eq!(user.sec_usr_id.as_bytes(), b"LONGUSER");
        assert_eq!(user.sec_usr_type.as_bytes(), b"A");
        assert_eq!(result.message, "User LONGUSER has been added ...");
        assert!(verify_as_entered("Longpass", &user.password_hash).expect("a usable hash"));

        let spaced = added(
            &mut store,
            UserAddInput {
                user_id: "AB CD".to_owned(),
                ..new_user()
            },
        );
        assert_eq!(spaced.user.sec_usr_id.as_bytes(), b"AB CD   ");
        assert_eq!(spaced.message, "User AB has been added ...");
    }

    #[test]
    fn duplicate_user_id_is_rejected() {
        // V-COUSR01C-006, P-COUSR01C-005: DUPKEY/DUPREC on the WRITE.
        let mut store = FakeStore::default().with_user(admin001());
        let input = UserAddInput {
            user_id: "ADMIN001".to_owned(),
            ..new_user()
        };

        let error = run(&mut store, input).expect_err("a duplicate key");

        assert_eq!(
            error,
            DomainError::duplicate(USER_ID_ALREADY_EXIST).with_field("USERID")
        );
        assert_eq!(error.kind, ErrorKind::Duplicate);
        assert_eq!(
            store.users.get(&UserId::move_from("ADMIN001")),
            Some(&admin001())
        );
        assert_eq!(store.users.len(), 1);
    }

    #[test]
    fn any_other_write_failure_is_unable_to_add() {
        // V-COUSR01C-007, P-COUSR01C-005: WHEN OTHER on the WRITE.
        let failures = [
            PortError::Unavailable("connection reset".to_owned()),
            PortError::Contention,
            PortError::Restricted,
            PortError::NotFound,
        ];
        for failure in failures {
            let mut store = FakeStore::default();
            store.fail_next("insert_user", failure.clone());

            let error = run(&mut store, new_user()).expect_err("a failed write");

            assert_eq!(
                error,
                DomainError::unexpected(UNABLE_TO_ADD_USER).with_field("FNAME"),
                "{failure:?}"
            );
            assert_eq!(error.kind, ErrorKind::Unexpected);
            assert!(store.users.is_empty(), "{failure:?}");
            assert_eq!(store.calls, ["insert_user"]);
        }
    }

    #[test]
    fn the_write_step_stores_the_prepared_record_as_is() {
        // P-COUSR01C-004: the WRITE is the step's only store call, and it
        // stores the hash the preparation computed, so it does no argon2 work.
        let prepared = prepare(new_user()).expect("every field is entered");
        let expected = prepared.user.clone();
        let mut store = FakeStore::default();

        let result = block_on(add(&mut store, prepared)).expect("the user is added");

        assert_eq!(store.calls, ["insert_user"]);
        assert_eq!(result.user, expected, "the prepared hash is stored as is");
        assert_eq!(store.users.get(&expected.sec_usr_id), Some(&expected));
    }

    #[test]
    fn a_preparation_on_another_thread_answers_as_inline() {
        // An asynchronous adapter prepares on a blocking thread pool. The
        // bounds of `thread::spawn` check that the input and the prepared user
        // are Send + 'static, and the answer there is the one given inline.
        let cases = [
            new_user(),
            UserAddInput {
                user_id: "test0002".to_owned(),
                password: "pass1234".to_owned(),
                ..new_user()
            },
            with_field(new_user(), 3, ""),
        ];
        for input in cases {
            let case = format!("{input:?}");
            let password = input.password.clone();
            let inline = prepared_outcome(prepare(input.clone()), &password);
            let elsewhere = thread::spawn(move || prepare(input))
                .join()
                .unwrap_or_else(|_| panic!("{case}: the preparing thread panicked"));
            assert_eq!(prepared_outcome(elsewhere, &password), inline, "{case}");
        }
    }

    #[test]
    fn add_is_send_for_a_send_store() {
        // The API awaits the write on the multi-threaded runtime.
        fn assert_send<T: Send>(value: T) -> T {
            value
        }

        let mut store = FakeStore::default();
        let user = prepare(new_user()).expect("every field is entered");
        let result = block_on(assert_send(add(&mut store, user)));

        assert!(result.is_ok());
    }

    #[test]
    fn debug_of_the_prepared_user_omits_the_password_and_the_hash() {
        let prepared = prepare(new_user()).expect("every field is entered");
        let rendered = format!("{prepared:?}");
        let user_id = UserId::move_from("TEST0001");

        assert_eq!(
            rendered,
            format!("PreparedUser {{ user_id: {user_id:?}, .. }}")
        );
        for hidden in ["Pass1234", "argon2", prepared.user.password_hash.as_str()] {
            assert!(!rendered.contains(hidden), "{hidden} in {rendered}");
        }
    }

    #[test]
    fn debug_omits_the_password() {
        let rendered = format!("{:?}", new_user());

        assert!(!rendered.contains("Pass1234"), "{rendered}");
        assert!(!rendered.contains("password"), "{rendered}");
        assert!(rendered.contains("\"TEST0001\""), "{rendered}");
        assert!(rendered.ends_with(", .. }"), "{rendered}");
    }
}
