//! CU03, `COUSR03C`: the delete of a `USRSEC` user, `DELETE /admin/users/{id}`.
//!
//! The 3270 transaction is a fetch screen: ENTER (`PROCESS-ENTER-KEY`) reads
//! the typed user and shows the names and type with the prompt to press PF5,
//! and PF5 (`DELETE-USER-INFO`) deletes it. The web replacement is a confirm
//! dialog on the user list: the row action fetches the user and opens the
//! dialog, and the dialog's Confirm is PF5 (E-58, F-COUSR03-001,
//! F-COUSR03-004). The fetch outcomes, V-COUSR03C-001 to V-COUSR03C-003, are
//! those of the `COUSR02C` fetch (V-COUSR02C-001 to V-COUSR02C-003), so
//! `GET /admin/users/{id}` is served by `rules::user_update::lookup`; this
//! module ports the PF5 path, [`delete`].
//!
//! # Outcomes
//!
//! | Request or store result | Outcome | Rows |
//! |---|---|---|
//! | user ID absent, `SPACES` or `LOW-VALUES` | [`ErrorKind::Empty`](crate::ErrorKind::Empty), `User ID can NOT be empty...`, field `USRIDIN`; nothing read | P-COUSR03C-004, V-COUSR03C-004 |
//! | `READ … UPDATE` answers `NOTFND` or fails | [`ErrorKind::Unexpected`](crate::ErrorKind::Unexpected), `Unable to Update User...`, field `FNAME`; nothing deleted | P-COUSR03C-004, P-COUSR03C-005, V-COUSR03C-005, E-46 |
//! | `DELETE` answers `NORMAL` | [`UserDeleted`] with `User <id> has been deleted ...` | P-COUSR03C-005, P-COUSR03C-011 |
//! | `DELETE` answers `NOTFND` | [`ErrorKind::NotFound`](crate::ErrorKind::NotFound), `User ID NOT found...`, field `USRIDIN` | P-COUSR03C-005, V-COUSR03C-006 |
//! | `DELETE` fails otherwise | [`ErrorKind::Unexpected`](crate::ErrorKind::Unexpected), `Unable to Update User...`, field `FNAME` | P-COUSR03C-005, V-COUSR03C-007 |
//!
//! `COUSR03C` sends the screen once per message it moves and never returns
//! between them, so a later text replaces an earlier one (P-COUSR03C-007): the
//! read's `Press PF5 key to delete this user ...`, `User ID NOT found...` or
//! `Unable to lookup User...` never reaches the response of a delete, because
//! `DELETE-USER-SEC-FILE` always runs after the read and moves its own text.
//!
//! The delete is one legacy write. The rule commits nothing: the adapter that
//! calls it owns the database transaction (AAP 0.6.4).

use crate::error::DomainError;
use crate::messages::{
    UNABLE_TO_UPDATE_USER, USER_ID_CAN_NOT_BE_EMPTY, USER_ID_NOT_FOUND, user_deleted_message,
};
use crate::numeric::UserId;
use crate::ports::{PortError, UserStore};

/// The width of `USRIDINI PIC X(8)` of map `COUSR3A` [app/cpy-bms/COUSR03.CPY:L60],
/// moved to the key `SEC-USR-ID PIC X(08)` [app/cpy/CSUSR01Y.cpy:L18].
const USRIDIN_LEN: usize = 8;

/// The BMS field `USRIDIN` of map `COUSR3A`: the cursor of the user-ID edit
/// and of the `NOTFND` delete (`MOVE -1 TO USRIDINL`).
const USRIDIN: &str = "USRIDIN";

/// The BMS field `FNAME` of map `COUSR3A`: the cursor of the `WHEN OTHER`
/// delete branch (`MOVE -1 TO FNAMEL` [app/cbl/COUSR03C.cbl:L334]).
const FNAME: &str = "FNAME";

/// The figurative constant `SPACE`.
const SPACE: u8 = b' ';

/// The figurative constant `LOW-VALUE`.
const LOW_VALUE: u8 = 0x00;

/// A deleted user: `DELETE-USER-SEC-FILE` answered `NORMAL`.
///
/// COUSR03C DELETE-USER-SEC-FILE — P-COUSR03C-005, P-COUSR03C-011
/// [app/cbl/COUSR03C.cbl:L313-322]: `INITIALIZE-ALL-FIELDS` empties the form
/// before the text is built, so the message is all the screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserDeleted {
    /// `SEC-USR-ID` of the deleted record, as stored.
    pub user_id: UserId,
    /// `User <id> has been deleted ...`, the `STRING` of
    /// [app/cbl/COUSR03C.cbl:L318-321] built by
    /// [`user_deleted_message`].
    pub message: String,
}

/// What the request's user ID puts in `USRIDINI`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnteredUserId {
    /// `USRIDINI = SPACES OR LOW-VALUES`.
    Blank,
    /// The key `MOVE USRIDINI OF COUSR3AI TO SEC-USR-ID` reads.
    Key(UserId),
    /// Text with non-space bytes past the eighth: no `SEC-USR-ID PIC X(08)`
    /// holds it.
    Unkeyable,
}

/// The `USRIDINI` field a request's user ID fills, by the convention the rules
/// share with [`Confirm::parse`](super::Confirm::parse): absent or empty text,
/// spaces only, or NUL characters only are the unentered field.
///
/// Any other text is moved into the `X(8)` field, left-justified and
/// space-padded, its case kept (the key is compared byte for byte). The map
/// field holds 8 bytes, so a 3270 never sends more; trailing spaces past the
/// eighth byte are padding the `MOVE` drops, but text with any other byte
/// there is [`EnteredUserId::Unkeyable`] rather than cut, so a request never
/// reaches the user whose ID is its first 8 bytes.
fn entered_user_id(user_id: Option<&str>) -> EnteredUserId {
    let Some(text) = user_id else {
        return EnteredUserId::Blank;
    };
    if text.bytes().all(|byte| byte == LOW_VALUE) {
        return EnteredUserId::Blank;
    }
    let key = UserId::move_from(text);
    if key.as_alnum().is_blank() {
        return EnteredUserId::Blank;
    }
    let overflow = text.as_bytes().get(USRIDIN_LEN..).unwrap_or_default();
    if overflow.iter().any(|&byte| byte != SPACE) {
        return EnteredUserId::Unkeyable;
    }
    EnteredUserId::Key(key)
}

/// `Unable to Update User...` with the cursor on `FNAME`: the `WHEN OTHER`
/// branch of `DELETE-USER-SEC-FILE`.
///
/// COUSR03C DELETE-USER-SEC-FILE — P-COUSR03C-005, V-COUSR03C-005,
/// V-COUSR03C-007 [app/cbl/COUSR03C.cbl:L329-335]. The text says Update
/// although the program deletes; it is reproduced as written.
fn unable_to_update_user() -> DomainError {
    DomainError::unexpected(UNABLE_TO_UPDATE_USER).with_field(FNAME)
}

/// Deletes the user `user_id` names: PF5 on the CU03 screen, the Confirm of
/// the delete dialog.
///
/// COUSR03C DELETE-USER-INFO — P-COUSR03C-004, V-COUSR03C-004,
/// V-COUSR03C-005 [app/cbl/COUSR03C.cbl:L174-192]: the user-ID edit, then
/// `READ-USER-SEC-FILE` (a `READ … UPDATE`, P-COUSR03C-003) and
/// `DELETE-USER-SEC-FILE`, which runs whatever the read answered. The read
/// leaves out the password, which a delete never compares.
///
/// After a failed read the COBOL `DELETE` has no record held for update and
/// no `RIDFLD`, so CICS answers `INVREQ` and the delete's `WHEN OTHER` text
/// replaces the read's (E-46). The rule therefore issues no delete then and
/// answers that outcome directly: nothing is deleted. A user ID no `X(08)` key
/// can hold (see `entered_user_id`) names no user on file, so it gets the same
/// outcome without a read.
///
/// # Errors
///
/// - [`ErrorKind::Empty`](crate::ErrorKind::Empty), `User ID can NOT be
///   empty...` on `USRIDIN`, when `user_id` is absent, `SPACES` or
///   `LOW-VALUES` (V-COUSR03C-004).
/// - [`ErrorKind::Unexpected`](crate::ErrorKind::Unexpected), `Unable to
///   Update User...` on `FNAME`, when the read for update fails for any reason,
///   `NOTFND` included (V-COUSR03C-005), or the delete fails other than with
///   `NOTFND` (V-COUSR03C-007).
/// - [`ErrorKind::NotFound`](crate::ErrorKind::NotFound), `User ID NOT
///   found...` on `USRIDIN`, when the delete answers `NOTFND`
///   (V-COUSR03C-006).
pub async fn delete<S: UserStore>(
    store: &mut S,
    user_id: Option<String>,
) -> Result<UserDeleted, DomainError> {
    let key = match entered_user_id(user_id.as_deref()) {
        EnteredUserId::Blank => {
            return Err(DomainError::empty(USER_ID_CAN_NOT_BE_EMPTY).with_field(USRIDIN));
        }
        EnteredUserId::Unkeyable => return Err(unable_to_update_user()),
        EnteredUserId::Key(key) => key,
    };
    let Ok(user) = store.read_user_profile_for_update(key).await else {
        return Err(unable_to_update_user());
    };
    delete_user_sec_file(store, user.sec_usr_id).await
}

/// The `DELETE` of the user record read for update, and its message.
///
/// COUSR03C DELETE-USER-SEC-FILE — P-COUSR03C-005, P-COUSR03C-011,
/// V-COUSR03C-006, V-COUSR03C-007 [app/cbl/COUSR03C.cbl:L305-336]. The
/// deleted text names `SEC-USR-ID` as the read left it, cut at its first
/// space (`DELIMITED BY SPACE`).
async fn delete_user_sec_file<S: UserStore>(
    store: &mut S,
    user_id: UserId,
) -> Result<UserDeleted, DomainError> {
    match store.delete_user(user_id).await {
        Ok(()) => Ok(UserDeleted {
            user_id,
            message: user_deleted_message(&user_id.as_alnum().as_text()),
        }),
        Err(PortError::NotFound) => {
            Err(DomainError::not_found(USER_ID_NOT_FOUND).with_field(USRIDIN))
        }
        Err(_) => Err(unable_to_update_user()),
    }
}

#[cfg(test)]
mod tests {
    use super::{EnteredUserId, UserDeleted, delete, entered_user_id};
    use crate::entities::UserSecurity;
    use crate::error::{DomainError, ErrorKind, FieldError};
    use crate::messages::{UNABLE_TO_UPDATE_USER, USER_ID_CAN_NOT_BE_EMPTY, USER_ID_NOT_FOUND};
    use crate::numeric::{Alnum, UserId};
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// A made-up PHC string; the rule never reads the hash.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$ZmFrZXNhbHQ$ZmFrZWhhc2g";

    fn user(id: &str, kind: &str) -> UserSecurity {
        UserSecurity {
            sec_usr_id: UserId::move_from(id),
            sec_usr_fname: Alnum::move_from("MARGARET"),
            sec_usr_lname: Alnum::move_from("GOLD"),
            password_hash: HASH.to_owned(),
            sec_usr_type: Alnum::move_from(kind),
        }
    }

    /// The seeded admin and user rows (P-CSUSR01Y-008, P-CSUSR01Y-009) and a
    /// short ID, which is stored space-padded.
    fn seeded() -> FakeStore {
        FakeStore::default()
            .with_user(user("ADMIN001", "A"))
            .with_user(user("USER0001", "U"))
            .with_user(user("ABC", "U"))
    }

    fn ids(store: &FakeStore) -> Vec<String> {
        store
            .users
            .keys()
            .map(|id| id.trimmed().into_owned())
            .collect()
    }

    fn error(kind: ErrorKind, message: &str, field: &'static str) -> DomainError {
        DomainError {
            kind,
            code: kind.code(),
            message: message.to_owned(),
            fields: vec![FieldError {
                field,
                message: message.to_owned(),
            }],
        }
    }

    fn unable_to_update() -> DomainError {
        error(ErrorKind::Unexpected, UNABLE_TO_UPDATE_USER, "FNAME")
    }

    #[test]
    fn deletes_the_user_read_for_update_and_names_it() {
        // P-COUSR03C-004, P-COUSR03C-005, P-COUSR03C-011: READ UPDATE, DELETE
        // NORMAL, then only the deleted text, the ID cut at its first space.
        let cases = [
            ("USER0001", "User USER0001 has been deleted ..."),
            ("ABC", "User ABC has been deleted ..."),
            ("ABC     ", "User ABC has been deleted ..."),
            ("ADMIN001  ", "User ADMIN001 has been deleted ..."),
        ];
        for (entered, text) in cases {
            let mut store = seeded();
            let deleted = block_on(delete(&mut store, Some(entered.to_owned())));
            let expected = UserDeleted {
                user_id: UserId::move_from(entered),
                message: text.to_owned(),
            };
            assert_eq!(deleted, Ok(expected), "{entered:?}");
            assert_eq!(store.calls, ["read_user_profile_for_update", "delete_user"]);
            assert!(
                !store.users.contains_key(&UserId::move_from(entered)),
                "{entered:?}"
            );
            assert_eq!(store.users.len(), 2, "only {entered:?} is deleted");
        }
    }

    #[test]
    fn a_blank_user_id_is_rejected_before_any_read() {
        // V-COUSR03C-004, P-COUSR03C-004: USRIDINI = SPACES OR LOW-VALUES.
        let blanks = [
            None,
            Some(""),
            Some(" "),
            Some("        "),
            Some("            "),
            Some("\0"),
            Some("\0\0\0\0\0\0\0\0"),
        ];
        for entered in blanks {
            let mut store = seeded();
            let result = block_on(delete(&mut store, entered.map(str::to_owned)));
            assert_eq!(
                result,
                Err(error(ErrorKind::Empty, USER_ID_CAN_NOT_BE_EMPTY, "USRIDIN")),
                "{entered:?}"
            );
            assert!(store.calls.is_empty(), "{entered:?}");
            assert_eq!(store, seeded(), "{entered:?}");
        }
    }

    #[test]
    fn an_unknown_user_ends_with_the_delete_text_and_deletes_nothing() {
        // V-COUSR03C-005, E-46: the READ UPDATE answers NOTFND, the DELETE
        // without a held record fails INVREQ and its text replaces the read's.
        // The key is compared byte for byte, so a lower-case ID is unknown.
        for entered in ["USER0099", "user0001", " USER001", "AB C"] {
            let mut store = seeded();
            let result = block_on(delete(&mut store, Some(entered.to_owned())));
            assert_eq!(result, Err(unable_to_update()), "{entered:?}");
            assert_eq!(store.calls, ["read_user_profile_for_update"], "{entered:?}");
            assert_eq!(ids(&store), ["ABC", "ADMIN001", "USER0001"]);
        }
    }

    #[test]
    fn a_failed_read_for_update_ends_with_the_delete_text_and_deletes_nothing() {
        // V-COUSR03C-005, P-COUSR03C-003 WHEN OTHER, E-46.
        let failures = [
            PortError::Contention,
            PortError::Unavailable("connection reset".to_owned()),
        ];
        for failure in failures {
            let mut store = seeded();
            store.fail_next("read_user_profile_for_update", failure.clone());
            let result = block_on(delete(&mut store, Some("USER0001".to_owned())));
            assert_eq!(result, Err(unable_to_update()), "{failure:?}");
            assert_eq!(store.calls, ["read_user_profile_for_update"], "{failure:?}");
            assert!(store.users.contains_key(&UserId::move_from("USER0001")));
        }
    }

    #[test]
    fn the_delete_outcomes_follow_its_resp() {
        // V-COUSR03C-006 (NOTFND) and V-COUSR03C-007 (any other RESP), after
        // a successful read for update.
        let cases = [
            (
                PortError::NotFound,
                error(ErrorKind::NotFound, USER_ID_NOT_FOUND, "USRIDIN"),
            ),
            (
                PortError::Unavailable("disk full".to_owned()),
                unable_to_update(),
            ),
            (PortError::Contention, unable_to_update()),
        ];
        for (failure, expected) in cases {
            let mut store = seeded();
            store.fail_next("delete_user", failure.clone());
            let result = block_on(delete(&mut store, Some("USER0001".to_owned())));
            assert_eq!(result, Err(expected), "{failure:?}");
            assert_eq!(store.calls, ["read_user_profile_for_update", "delete_user"]);
            assert!(
                store.users.contains_key(&UserId::move_from("USER0001")),
                "a failed delete changes nothing"
            );
        }
    }

    #[test]
    fn text_past_the_eighth_byte_names_no_user_and_reads_nothing() {
        // USRIDINI is X(8): a request whose ninth byte on is not padding names
        // no SEC-USR-ID, so it never reaches the user its first 8 bytes name.
        for entered in ["USER00012", "ADMIN001X", "USER0001  x"] {
            let mut store = seeded();
            let result = block_on(delete(&mut store, Some(entered.to_owned())));
            assert_eq!(result, Err(unable_to_update()), "{entered:?}");
            assert!(store.calls.is_empty(), "{entered:?}");
            assert_eq!(store, seeded(), "{entered:?}");
        }
    }

    #[test]
    fn the_entered_field_is_the_x8_move_of_the_text() {
        let cases = [
            (None, EnteredUserId::Blank),
            (Some("\0\0"), EnteredUserId::Blank),
            (Some(" \0"), EnteredUserId::Key(UserId::move_from(" \0"))),
            (Some("ab"), EnteredUserId::Key(UserId::move_from("ab"))),
            (
                Some("USER0001   "),
                EnteredUserId::Key(UserId::move_from("USER0001")),
            ),
            (Some("USER0001\0"), EnteredUserId::Unkeyable),
        ];
        for (entered, expected) in cases {
            assert_eq!(entered_user_id(entered), expected, "{entered:?}");
        }
    }

    #[test]
    fn the_delete_future_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = seeded();
        let future = delete(&mut store, Some("USER0001".to_owned()));
        assert_send(&future);
        assert!(block_on(future).is_ok());
    }
}
