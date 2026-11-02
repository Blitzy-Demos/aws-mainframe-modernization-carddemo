//! CC00 sign-on: `COSGN00C` `PROCESS-ENTER-KEY` and `READ-USER-SEC-FILE`
//! [app/cbl/COSGN00C.cbl:L108-140] [app/cbl/COSGN00C.cbl:L209-257].
//!
//! The rule edits the two map fields of `COSGN00` (`USERIDI`, `PASSWDI`, both
//! `PIC X(8)` [app/cpy-bms/COSGN00.CPY:L72, L78]), upper-cases them, reads the
//! `USRSEC` record (`CSUSR01Y`) by the upper-cased user ID and compares the
//! password. A signed-on user carries its user type (`CDEMO-USER-TYPE`,
//! `COCOM01Y`) and the menu the COBOL transfers to: `COADM01C` for an
//! administrator, `COMEN01C` otherwise.
//!
//! The plaintext compare of the COBOL is replaced by argon2id verification of
//! the stored hash, with storage and sign-on normalising differently, as in the
//! COBOL (E-20): the admin programs store the password with its case kept,
//! sign-on upper-cases the entered user ID and password. A user ID or password
//! stored in lower or mixed case therefore never signs on (P-COSGN00C-013).
//! Normalisation and verification belong to [`crate::auth`]; this module only
//! sequences them.
//!
//! The rule runs in two steps, because argon2id verification is memory-hard
//! and CPU-bound and never yields:
//!
//! 1. [`sign_on`], asynchronous: the blank edits, the upper-casing and the
//!    `USRSEC` read. It does no argon2 work and answers with a
//!    [`SignonCheck`] holding the record read and the password typed.
//! 2. [`SignonCheck::verify`], synchronous and pure: the password compare and
//!    the outcome it selects.
//!
//! An adapter awaits [`sign_on`], ends its unit of work, then runs
//! [`SignonCheck::verify`] where blocking work belongs. An asynchronous
//! adapter runs it on a blocking thread pool, such as tokio's
//! `spawn_blocking`, never inline on the executor, where one verification
//! would hold a worker thread and delay every other task on it. The token
//! needs a [`SignonOutcome`], which only [`SignonCheck::verify`] yields, so no
//! sign-on succeeds without the second step.
//!
//! `POST /auth/login` adapts the rule and issues the token; the token, the
//! logout text (P-COSGN00C-003) and the 3270 screen mechanics (first entry,
//! AID keys, header) are not this module's concern. The COBOL keeps no session
//! and counts no failed attempts, so neither does this rule.

use std::fmt;

use crate::auth::{normalize_signon_user_id, verify_signon};
use crate::entities::UserSecurity;
use crate::error::DomainError;
use crate::messages::{
    PLEASE_ENTER_PASSWORD, PLEASE_ENTER_USER_ID, UNABLE_TO_VERIFY_THE_USER,
    USER_NOT_FOUND_TRY_AGAIN, WRONG_PASSWORD_TRY_AGAIN,
};
use crate::numeric::{Alnum, UserId};
use crate::ports::{PortError, UserStore};

/// The BMS field of the user ID: `USERIDL OF COSGN0AI`, where the COBOL puts
/// the cursor with `MOVE -1` [app/cpy-bms/COSGN00.CPY:L67].
const USERID: &str = "USERID";

/// The BMS field of the password: `PASSWDL OF COSGN0AI`
/// [app/cpy-bms/COSGN00.CPY:L73].
const PASSWD: &str = "PASSWD";

/// The width of `USERIDI PIC X(8)` [app/cpy-bms/COSGN00.CPY:L72].
const USERIDI_LEN: usize = 8;

/// The width of `PASSWDI PIC X(8)` [app/cpy-bms/COSGN00.CPY:L78].
const PASSWDI_LEN: usize = 8;

/// The two fields of the sign-on map as typed: `USERIDI` and `PASSWDI` of
/// `COSGN0AI`, before any case change.
///
/// The request DTO limits each to the 8 characters of its map field; the rule
/// reads at most that many bytes of each. `Debug` omits the password, so it
/// never reaches a log line or a tracing span.
#[derive(Clone, PartialEq, Eq)]
pub struct SignonInput {
    /// `USERIDI`, the user ID as typed.
    pub user_id: String,
    /// `PASSWDI`, the password as typed.
    pub password: String,
}

impl fmt::Debug for SignonInput {
    /// The user ID only, ended with `..` to show that the password is left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignonInput")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

/// Where a signed-on user lands: the program `COSGN00C` transfers control to
/// with `XCTL` [app/cbl/COSGN00C.cbl:L230-240].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    /// `CDEMO-USRTYP-ADMIN` (`SEC-USR-TYPE` is `A` [app/cpy/COCOM01Y.cpy:L27]):
    /// the admin menu `COADM01C`, the SPA route `/admin`.
    AdminHome,
    /// Any other user type: the main menu `COMEN01C`, the SPA route `/`.
    UserHome,
}

/// A successful sign-on: the COMMAREA fields `COSGN00C` sets before its
/// `XCTL` [app/cbl/COSGN00C.cbl:L224-228].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignonOutcome {
    /// `CDEMO-USER-ID`: the upper-cased, space-padded user ID typed, which is
    /// the key of the `USRSEC` record read.
    pub user_id: UserId,
    /// `CDEMO-USER-TYPE`, moved from `SEC-USR-TYPE` of the record read.
    pub usr_type: Alnum<1>,
    /// The menu the user type selects.
    pub landing: Landing,
}

/// A sign-on whose user record has been read and whose password is still to
/// be verified: what [`sign_on`] answers and [`SignonCheck::verify`] completes.
///
/// Every field is owned, so the check is `Send + 'static` and an asynchronous
/// adapter can move it into a blocking thread pool to verify it. `Debug` shows
/// the user ID only: neither the password typed nor the stored hash reaches a
/// log line or a tracing span.
#[must_use = "the password is verified only by `SignonCheck::verify`"]
pub struct SignonCheck {
    /// `WS-USER-ID`: the upper-cased, space-padded user ID typed, the key of
    /// the `USRSEC` record read.
    user_id: UserId,
    /// The `USRSEC` record read by that key.
    user: UserSecurity,
    /// `PASSWDI` as typed; it is upper-cased and padded when it is verified.
    password: String,
}

impl fmt::Debug for SignonCheck {
    /// The user ID only, ended with `..` to show that the password typed and
    /// the record read are left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SignonCheck")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

impl SignonCheck {
    /// COSGN00C READ-USER-SEC-FILE — P-COSGN00C-009, P-COSGN00C-010,
    /// P-COSGN00C-012, P-COSGN00C-013, V-COSGN00C-004, V-COSGN00C-005.
    ///
    /// `IF SEC-USR-PWD = WS-USER-PWD` on the record read and the transfer it
    /// selects [app/cbl/COSGN00C.cbl:L221-246]: the password typed is
    /// upper-cased, padded to 8 (E-20) and verified against the stored hash. A
    /// matching password signs the user on with the user type of the record
    /// and the landing it selects. A password stored in lower or mixed case
    /// never matches the upper-cased entry (P-COSGN00C-013).
    ///
    /// Synchronous: argon2id verification is memory-hard and CPU-bound and
    /// never yields. An asynchronous adapter calls this on a blocking thread
    /// pool, such as tokio's `spawn_blocking`, never inline on the executor.
    ///
    /// # Errors
    ///
    /// A [`DomainError`] with one field:
    ///
    /// - `SignonFailed`, [`WRONG_PASSWORD_TRY_AGAIN`], field `PASSWD`: the
    ///   password does not match (V-COSGN00C-005).
    /// - `Unexpected`, [`UNABLE_TO_VERIFY_THE_USER`], field `USERID`: the
    ///   stored hash is unusable (V-COSGN00C-004).
    pub fn verify(self) -> Result<SignonOutcome, DomainError> {
        let Self {
            user_id,
            user,
            password,
        } = self;
        match verify_signon(&password, &user.password_hash) {
            Ok(true) => Ok(SignonOutcome {
                user_id,
                usr_type: user.sec_usr_type,
                landing: landing_of(&user),
            }),
            Ok(false) => {
                Err(DomainError::signon_failed(WRONG_PASSWORD_TRY_AGAIN).with_field(PASSWD))
            }
            // A stored hash argon2 cannot use has no COBOL counterpart, since
            // the COBOL compares plaintext. It is a failure to verify the user
            // record, so it answers as the WHEN OTHER branch of the read does
            // (V-COSGN00C-004), never as a wrong password.
            Err(_) => Err(unable_to_verify()),
        }
    }
}

/// COSGN00C PROCESS-ENTER-KEY — P-COSGN00C-002, P-COSGN00C-005,
/// P-COSGN00C-006, P-COSGN00C-007, V-COSGN00C-001, V-COSGN00C-002; then
/// READ-USER-SEC-FILE — P-COSGN00C-008, P-COSGN00C-011, P-COSGN00C-012,
/// V-COSGN00C-003, V-COSGN00C-004.
///
/// The first step of a sign-on by the ENTER key of the sign-on screen
/// [app/cbl/COSGN00C.cbl:L108-140], up to and including the user read:
///
/// 1. A user ID of `SPACES OR LOW-VALUES` is rejected first, then a password of
///    `SPACES OR LOW-VALUES` [app/cbl/COSGN00C.cbl:L117-130]. Either rejection
///    sends the screen and ends the task's work, so no record is read.
/// 2. The user ID is upper-cased and padded to 8, giving the `USRSEC` key
///    [app/cbl/COSGN00C.cbl:L132-134]; the password is upper-cased and padded
///    when it is verified [app/cbl/COSGN00C.cbl:L135-136] (E-20).
/// 3. The user record is read by that key (`READ-USER-SEC-FILE`
///    [app/cbl/COSGN00C.cbl:L209-219, L247-256]).
///
/// The found record and the password typed come back as a [`SignonCheck`],
/// unverified: this function does no argon2 work. The adapter awaits it, ends
/// its unit of work, then runs [`SignonCheck::verify`] where blocking work
/// belongs — for an asynchronous adapter a blocking thread pool, such as
/// tokio's `spawn_blocking`, never inline on the executor, because argon2id
/// verification is memory-hard and never yields. Only that second step yields
/// the [`SignonOutcome`] that signs the user on.
///
/// # Errors
///
/// A [`DomainError`] with one field, in the order the COBOL tests them:
///
/// - `Empty`, [`PLEASE_ENTER_USER_ID`], field `USERID`: the user ID is blank
///   (V-COSGN00C-001).
/// - `Empty`, [`PLEASE_ENTER_PASSWORD`], field `PASSWD`: the password is blank
///   (V-COSGN00C-002).
/// - `SignonFailed`, [`USER_NOT_FOUND_TRY_AGAIN`], field `USERID`: no user has
///   the upper-cased ID (V-COSGN00C-003).
/// - `Unexpected`, [`UNABLE_TO_VERIFY_THE_USER`], field `USERID`: the read
///   failed otherwise (V-COSGN00C-004).
///
/// A wrong password and an unusable stored hash are answered by
/// [`SignonCheck::verify`].
pub async fn sign_on<S: UserStore>(
    store: &mut S,
    input: SignonInput,
) -> Result<SignonCheck, DomainError> {
    let SignonInput { user_id, password } = input;
    if is_spaces_or_low_values(&user_id, USERIDI_LEN) {
        return Err(DomainError::empty(PLEASE_ENTER_USER_ID).with_field(USERID));
    }
    if is_spaces_or_low_values(&password, PASSWDI_LEN) {
        return Err(DomainError::empty(PLEASE_ENTER_PASSWORD).with_field(PASSWD));
    }
    let key = UserId::move_from(&normalize_signon_user_id(&user_id));
    let user = read_user_sec_file(store, key).await?;
    Ok(SignonCheck {
        user_id: key,
        user,
        password,
    })
}

/// COSGN00C PROCESS-ENTER-KEY — P-COSGN00C-005, P-COSGN00C-006,
/// V-COSGN00C-001, V-COSGN00C-002.
///
/// The condition `<field> = SPACES OR LOW-VALUES` [app/cbl/COSGN00C.cbl:L118,
/// L123] on the text received into a `PIC X(width)` map field: the first
/// `width` bytes of `entered` are all spaces, or all NUL. Empty text satisfies
/// both. A mix of spaces and NUL satisfies neither figurative constant, so it
/// is not blank.
fn is_spaces_or_low_values(entered: &str, width: usize) -> bool {
    let bytes = entered.as_bytes();
    let field = bytes.get(..width).unwrap_or(bytes);
    field.iter().all(|&byte| byte == b' ') || field.iter().all(|&byte| byte == 0)
}

/// COSGN00C READ-USER-SEC-FILE — P-COSGN00C-008, P-COSGN00C-011,
/// P-COSGN00C-012, P-COSGN00C-013, V-COSGN00C-003, V-COSGN00C-004.
///
/// `EXEC CICS READ DATASET(USRSEC) RIDFLD(WS-USER-ID)` and the `RESP`
/// branches of its `EVALUATE` that end before the password compare
/// [app/cbl/COSGN00C.cbl:L211-219, L247-256]. The key is compared byte for
/// byte, so a user ID stored in lower or mixed case is never found (E-20).
///
/// # Errors
///
/// As [`sign_on`] lists for V-COSGN00C-003 and V-COSGN00C-004.
async fn read_user_sec_file<S: UserStore>(
    store: &mut S,
    key: UserId,
) -> Result<UserSecurity, DomainError> {
    match store.read_user(key).await {
        Ok(user) => Ok(user),
        Err(PortError::NotFound) => {
            Err(DomainError::signon_failed(USER_NOT_FOUND_TRY_AGAIN).with_field(USERID))
        }
        Err(_) => Err(unable_to_verify()),
    }
}

/// COSGN00C READ-USER-SEC-FILE — P-COSGN00C-012, V-COSGN00C-004.
///
/// `WHEN OTHER`: 'Unable to verify the User ...' with the cursor on the user
/// ID [app/cbl/COSGN00C.cbl:L252-256].
fn unable_to_verify() -> DomainError {
    DomainError::unexpected(UNABLE_TO_VERIFY_THE_USER).with_field(USERID)
}

/// COSGN00C READ-USER-SEC-FILE — P-COSGN00C-009.
///
/// `IF CDEMO-USRTYP-ADMIN` `XCTL PROGRAM('COADM01C')` `ELSE`
/// `XCTL PROGRAM('COMEN01C')` [app/cbl/COSGN00C.cbl:L230-240].
fn landing_of(user: &UserSecurity) -> Landing {
    if user.is_admin() {
        Landing::AdminHome
    } else {
        Landing::UserHome
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::OnceLock;
    use std::thread;

    use super::{Landing, SignonCheck, SignonInput, SignonOutcome, sign_on};
    use crate::auth::hash_stored_password;
    use crate::entities::UserSecurity;
    use crate::error::{DomainError, ErrorKind, FieldError};
    use crate::messages::{
        PLEASE_ENTER_PASSWORD, PLEASE_ENTER_USER_ID, UNABLE_TO_VERIFY_THE_USER,
        USER_NOT_FOUND_TRY_AGAIN, WRONG_PASSWORD_TRY_AGAIN,
    };
    use crate::numeric::{Alnum, UserId};
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// A `USRSEC` record whose password is stored as `password` is entered,
    /// as `COUSR01C` and the `DUSRSECJ` seed store it (E-20).
    fn user(id: &str, first: &str, last: &str, password: &str, kind: &str) -> UserSecurity {
        UserSecurity {
            sec_usr_id: UserId::move_from(id),
            sec_usr_fname: Alnum::move_from(first),
            sec_usr_lname: Alnum::move_from(last),
            password_hash: hash_stored_password(password).expect("an argon2 hash"),
            sec_usr_type: Alnum::move_from(kind),
        }
    }

    /// The users every test signs on against. Hashing is slow, so the store
    /// is built once and cloned.
    fn seeded() -> FakeStore {
        static SEEDED: OnceLock<FakeStore> = OnceLock::new();
        SEEDED
            .get_or_init(|| {
                let mut broken = user("BADHASH1", "BROKEN", "HASH", "PASSWORD", "U");
                broken.password_hash = "not-a-phc-string".to_owned();
                FakeStore::default()
                    // The seeded rows [app/jcl/DUSRSECJ.jcl:L35, L40].
                    .with_user(user("ADMIN001", "MARGARET", "GOLD", "PASSWORD", "A"))
                    .with_user(user("USER0001", "LAWRENCE", "THOMAS", "PASSWORD", "U"))
                    // Passwords an admin stored in lower, mixed and upper case.
                    .with_user(user("LOWERPWD", "LOWER", "CASE", "pass1234", "U"))
                    .with_user(user("MIXEDPWD", "MIXED", "CASE", "Pass1234", "U"))
                    .with_user(user("UPPERPWD", "UPPER", "CASE", "PASS1234", "U"))
                    // A user ID an admin stored in lower case.
                    .with_user(user("user0009", "LOWER", "ID", "PASSWORD", "U"))
                    .with_user(broken)
            })
            .clone()
    }

    /// `future`, unchanged, once the compiler has checked that it is `Send`, as
    /// the multi-threaded runtime of the API adapter requires.
    fn send<F: Future + Send>(future: F) -> F {
        future
    }

    /// The first step only: the edits and the user read for the map fields
    /// typed as `user_id` and `password`.
    fn look_up(
        store: &mut FakeStore,
        user_id: &str,
        password: &str,
    ) -> Result<SignonCheck, DomainError> {
        let input = SignonInput {
            user_id: user_id.to_owned(),
            password: password.to_owned(),
        };
        block_on(send(sign_on(store, input)))
    }

    /// Signs on with the map fields typed as `user_id` and `password`: both
    /// steps, the read and then the verification.
    fn run(
        store: &mut FakeStore,
        user_id: &str,
        password: &str,
    ) -> Result<SignonOutcome, DomainError> {
        look_up(store, user_id, password).and_then(SignonCheck::verify)
    }

    /// Asserts that `result` is the rejection of `kind` with `message`, whose
    /// one field is `field`.
    fn assert_rejected(
        result: Result<SignonOutcome, DomainError>,
        kind: ErrorKind,
        message: &str,
        field: &'static str,
        case: &str,
    ) {
        let error = result.expect_err(case);
        assert_eq!(error.kind, kind, "{case}");
        assert_eq!(error.code, kind.code(), "{case}");
        assert_eq!(error.message, message, "{case}");
        let expected = [FieldError {
            field,
            message: message.to_owned(),
        }];
        assert_eq!(error.fields, expected, "{case}");
    }

    #[test]
    fn valid_credentials_sign_on_and_land_by_user_type() {
        // P-COSGN00C-007 (upper-casing), P-COSGN00C-008 (read by the
        // upper-cased key), P-COSGN00C-009 (outcome and landing), E-20.
        let cases = [
            ("ADMIN001", "PASSWORD", "ADMIN001", "A", Landing::AdminHome),
            ("USER0001", "PASSWORD", "USER0001", "U", Landing::UserHome),
            ("admin001", "password", "ADMIN001", "A", Landing::AdminHome),
            ("uSeR0001", "PassWord", "USER0001", "U", Landing::UserHome),
            ("upperpwd", "pass1234", "UPPERPWD", "U", Landing::UserHome),
        ];
        for (typed_id, typed_password, key, kind, landing) in cases {
            let mut store = seeded();
            let outcome = run(&mut store, typed_id, typed_password)
                .unwrap_or_else(|error| panic!("{typed_id}: {error:?}"));
            assert_eq!(
                outcome,
                SignonOutcome {
                    user_id: UserId::move_from(key),
                    usr_type: Alnum::move_from(kind),
                    landing,
                },
                "{typed_id}"
            );
            assert_eq!(store.calls, ["read_user"], "{typed_id}");
        }
    }

    #[test]
    fn blank_fields_are_rejected_before_any_read() {
        // V-COSGN00C-001 is tested before V-COSGN00C-002 (orders 1 and 2), and
        // either ends the task's work (P-COSGN00C-005, P-COSGN00C-006).
        let empty = ErrorKind::Empty;
        let (no_id, no_password) = (PLEASE_ENTER_USER_ID, PLEASE_ENTER_PASSWORD);
        let cases = [
            ("", "PASSWORD", no_id, "USERID"),
            ("        ", "PASSWORD", no_id, "USERID"),
            ("\0\0\0\0\0\0\0\0", "PASSWORD", no_id, "USERID"),
            ("\0", "PASSWORD", no_id, "USERID"),
            ("   ", "", no_id, "USERID"),
            ("ADMIN001", "", no_password, "PASSWD"),
            ("ADMIN001", "        ", no_password, "PASSWD"),
            ("ADMIN001", "\0\0\0", no_password, "PASSWD"),
        ];
        for (typed_id, typed_password, message, field) in cases {
            let mut store = seeded();
            let case = format!("{typed_id:?} {typed_password:?}");
            let result = run(&mut store, typed_id, typed_password);
            assert_rejected(result, empty, message, field, &case);
            assert!(store.calls.is_empty(), "{case}");
        }
    }

    #[test]
    fn failed_reads_and_compares_are_rejected_with_their_message() {
        let failed = ErrorKind::SignonFailed;
        let (not_found, wrong) = (USER_NOT_FOUND_TRY_AGAIN, WRONG_PASSWORD_TRY_AGAIN);
        let cases = [
            // V-COSGN00C-003: RESP 13 for the upper-cased key.
            ("USER0099", "PASSWORD", failed, not_found, "USERID"),
            // P-COSGN00C-013 (E-20): an ID stored in lower case is never found.
            ("user0009", "PASSWORD", failed, not_found, "USERID"),
            // Spaces mixed with LOW-VALUES are neither figurative constant, so
            // the ID passes the blank edit and is read.
            ("\0 ", "PASSWORD", failed, not_found, "USERID"),
            // V-COSGN00C-005.
            ("ADMIN001", "WRONGPWD", failed, wrong, "PASSWD"),
            ("ADMIN001", "\0 ", failed, wrong, "PASSWD"),
            // P-COSGN00C-013 (E-20): a password stored in lower or mixed case
            // never matches the upper-cased entry, typed either way.
            ("LOWERPWD", "pass1234", failed, wrong, "PASSWD"),
            ("MIXEDPWD", "Pass1234", failed, wrong, "PASSWD"),
            ("MIXEDPWD", "PASS1234", failed, wrong, "PASSWD"),
            // V-COSGN00C-004: a stored hash that cannot be verified.
            (
                "BADHASH1",
                "PASSWORD",
                ErrorKind::Unexpected,
                UNABLE_TO_VERIFY_THE_USER,
                "USERID",
            ),
        ];
        for (typed_id, typed_password, kind, message, field) in cases {
            let mut store = seeded();
            let case = format!("{typed_id:?} {typed_password:?}");
            let result = run(&mut store, typed_id, typed_password);
            assert_rejected(result, kind, message, field, &case);
            assert_eq!(store.calls, ["read_user"], "{case}");
        }
    }

    #[test]
    fn any_other_read_failure_is_unable_to_verify() {
        // V-COSGN00C-004, P-COSGN00C-012: the WHEN OTHER branch of the RESP.
        let failures = [
            PortError::Unavailable("connection reset".to_owned()),
            PortError::Contention,
        ];
        for failure in failures {
            let mut store = seeded();
            let case = format!("{failure:?}");
            store.fail_next("read_user", failure);
            let result = run(&mut store, "ADMIN001", "PASSWORD");
            assert_rejected(
                result,
                ErrorKind::Unexpected,
                UNABLE_TO_VERIFY_THE_USER,
                "USERID",
                &case,
            );
            assert_eq!(store.calls, ["read_user"], "{case}");
        }
    }

    #[test]
    fn the_read_step_leaves_the_password_unverified() {
        // V-COSGN00C-005 and the unusable-hash case of V-COSGN00C-004: the
        // read step answers with a check once the record is found, and only
        // the verification step rejects.
        let cases = [
            (
                "ADMIN001",
                "WRONGPWD",
                ErrorKind::SignonFailed,
                WRONG_PASSWORD_TRY_AGAIN,
                "PASSWD",
            ),
            (
                "BADHASH1",
                "PASSWORD",
                ErrorKind::Unexpected,
                UNABLE_TO_VERIFY_THE_USER,
                "USERID",
            ),
        ];
        for (typed_id, typed_password, kind, message, field) in cases {
            let mut store = seeded();
            let case = format!("{typed_id:?} {typed_password:?}");
            let check = look_up(&mut store, typed_id, typed_password)
                .unwrap_or_else(|error| panic!("{case}: {error:?}"));
            assert_eq!(store.calls, ["read_user"], "{case}");
            assert_rejected(check.verify(), kind, message, field, &case);
        }
    }

    #[test]
    fn a_check_verified_on_another_thread_answers_as_inline() {
        // An asynchronous adapter verifies on a blocking thread pool. The
        // bounds of `thread::spawn` check that the check is Send + 'static,
        // and the outcome there is the one the same entry gives inline.
        let cases = [
            ("ADMIN001", "PASSWORD"),
            ("uSeR0001", "PassWord"),
            ("ADMIN001", "WRONGPWD"),
            ("BADHASH1", "PASSWORD"),
        ];
        for (typed_id, typed_password) in cases {
            let case = format!("{typed_id:?} {typed_password:?}");
            let inline = run(&mut seeded(), typed_id, typed_password);
            let check = look_up(&mut seeded(), typed_id, typed_password)
                .unwrap_or_else(|error| panic!("{case}: {error:?}"));
            let elsewhere = thread::spawn(move || check.verify())
                .join()
                .unwrap_or_else(|_| panic!("{case}: the verifying thread panicked"));
            assert_eq!(elsewhere, inline, "{case}");
        }
    }

    #[test]
    fn debug_of_the_check_omits_the_password_and_the_hash() {
        let mut store = seeded();
        let check = look_up(&mut store, "admin001", "notshown")
            .unwrap_or_else(|error| panic!("ADMIN001 is read: {error:?}"));
        let rendered = format!("{check:?}");
        let user_id = UserId::move_from("ADMIN001");
        assert_eq!(
            rendered,
            format!("SignonCheck {{ user_id: {user_id:?}, .. }}")
        );
        for hidden in ["notshown", "NOTSHOWN", "argon2"] {
            assert!(!rendered.contains(hidden), "{hidden} in {rendered}");
        }
    }

    #[test]
    fn debug_of_the_input_omits_the_password() {
        let input = SignonInput {
            user_id: "ADMIN001".to_owned(),
            password: "NOTSHOWN".to_owned(),
        };
        let rendered = format!("{input:?}");
        assert_eq!(rendered, "SignonInput { user_id: \"ADMIN001\", .. }");
    }
}
