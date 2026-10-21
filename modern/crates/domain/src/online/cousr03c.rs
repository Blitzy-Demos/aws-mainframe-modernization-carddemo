//! COUSR03C, user delete: the rules of endpoint 20, `DELETE /api/v1/users/{user_id}` (transaction
//! `CU03`, map `COUSR3A`; admin only, AAP 0.3.4.1).
//!
//! This module ports the `server` and `both` rows of `modern/audit/online/COUSR03C.md` that a
//! pure function decides: the blank-ID edit of `DELETE-USER-INFO` (COUSR03C-016), the key and
//! the held row of its `READ … UPDATE` (COUSR03C-017, COUSR03C-018), and the outcomes of
//! `DELETE-USER-SEC-FILE` (COUSR03C-019 to COUSR03C-022). The token and admin gate
//! (COUSR03C-001, COUSR03C-002) belong to the API's extractors. The fetch the screen's ENTER
//! sends is endpoint 18, the read COUSR03C shares with COUSR02C, so its rules live in
//! [`super::cousr02c`] and are not repeated here (COUSR03C-010, COUSR03C-013, COUSR03C-014). The
//! `client` rows, F3, F4, F12, the fetch's display and the read's prompt among them
//! (COUSR03C-003 to COUSR03C-009, COUSR03C-011, COUSR03C-012, COUSR03C-015, COUSR03C-023,
//! COUSR03C-025, COUSR03C-026), belong to the screen.
//!
//! # Flow
//!
//! `api::services::cousr03c` drives endpoint 20; this module does no I/O.
//!
//! 1. [`edit_delete`] turns the path's user ID into the 8-byte key, or rejects a blank one
//!    before any read (COUSR03C-016).
//! 2. The service reads the row by that key and holds it for the delete, as the COBOL's
//!    `READ … UPDATE` does, and passes the store's answer to [`held_for_delete`]. A found row
//!    yields the key to delete; a row not found or a failed read yields the answer the delete
//!    would give with no record held (COUSR03C-017, COUSR03C-018), and no delete is issued.
//! 3. The service deletes the held row, maps the store's answer into a [`DeleteResult`] and
//!    answers with [`delete_outcome`]: the success message or the delete's failure
//!    (COUSR03C-019 to COUSR03C-022).
//!
//! Every outcome a request can reach is a value here, so the service only maps store results
//! into [`UserRead`] and [`DeleteResult`] and the handler only converts what comes back (Thin
//! Request Handlers). A failure is a [`UserMaintFailure`], shared with COUSR02C: the literal
//! moved to `WS-MESSAGE`, the field the cursor goes to and the status class. Where the task sends
//! the map twice, the response is the last send, so endpoint 20 never returns the read's prompt
//! [`DELETE_PROMPT`] (COUSR03C-017).
//!
//! No check compares the target with the caller: COUSR03C never tests `CDEMO-USER-ID` and counts
//! no admins, so an admin may delete any user, themselves and the last admin included, and the
//! acting identity stays the verified token's (COUSR03C-024).
//!
//! # Behaviour kept
//!
//! - The delete's failure says `Unable to Update User...`, COUSR02C's rewrite literal, and the
//!   text is not corrected (`X-COUSR03C-1`, COUSR03C-022).
//! - F5 deletes the user ID in the input without a prior fetch: the path names the user, and
//!   the names the screen shows take no part (COUSR03C-017, COUSR03C-023).
//!
//! # Halt candidate `X-COUSR03C-H1`
//!
//! `DELETE-USER-INFO` issues the `DELETE` without testing `WS-ERR-FLG`, so after a read that
//! found no row or failed, a `DELETE` with no record held decides the final message. Its outcome
//! is defined by CICS, outside the repository. [`held_for_delete`] answers with the `OTHER`
//! branch IBM documents for that `DELETE`, `Unable to Update User...` on `first_name`, and
//! deletes nothing; that answer is not claimed as parity until the candidate is resolved
//! (COUSR03C-018).
//!
//! # Data
//!
//! COUSR03C's map has no password field and this module handles none: no value, message or
//! failure here carries a password or a hash (Cardholder Data Redaction, CSUSR01Y-004). The user
//! ID is validated input naming the target, never the caller's authority (AAP 0.5.1).
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a value
//! or a [`UserMaintFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input).

use copybook::PicX;

use super::cousr02c::{UserMaintFailure, UserRead, edit_user_id, reread};

/// The info prompt COUSR03C's read sends on success, in the neutral colour (`DFHNEUTR`;
/// COUSR03C-012, `COUSR03C.cbl:280-286`).
///
/// No endpoint returns it. Endpoint 18 is shared with COUSR02C, whose read sends
/// `Press PF5 key to save your updates ...` instead, so the response carries no message and the
/// delete screen shows this prompt after a 200 (`X-COUSR02C-1`). On F5 the same task sends the
/// delete's message after it, so endpoint 20 answers with that message only (COUSR03C-017).
pub const DELETE_PROMPT: &str = "Press PF5 key to delete this user ...";

/// Head and tail of the success message around the user ID (COUSR03C-019).
const DELETED_HEAD: &str = "User ";
const DELETED_TAIL: &str = " has been deleted ...";

/// The byte `STRING … DELIMITED BY SPACE` stops at (COUSR03C-020).
const SPACE: u8 = b' ';

/// The user ID of the path, `USRIDINI PIC X(8)`, as the key of F5's read and delete: the received
/// bytes padded with spaces to 8, neither upper-cased nor trimmed of leading spaces, so
/// `user0001` does not find `USER0001` (COUSR03C-016, COUSR03C-017).
///
/// It is the first edit of `DELETE-USER-INFO`, so a blank ID is rejected before any read or
/// delete. The edit and its literal are those of ENTER in both programs, so this is
/// [`edit_user_id`] (COUSR02C-010, COUSR02C-018).
///
/// # Errors
///
/// `User ID can NOT be empty...` on `user_id`, status class
/// [`super::cousr02c::FailureClass::Input`] (400), when the padded ID is all spaces or all NUL
/// bytes (COUSR03C-016).
pub fn edit_delete(raw: &str) -> Result<PicX<8>, UserMaintFailure> {
    edit_user_id(raw)
}

/// The row F5's `READ … UPDATE` holds for the delete: the key of the found row, the
/// `SEC-USR-ID` the delete removes and its message names (COUSR03C-017).
///
/// The service reads the row `FOR UPDATE` in the transaction that deletes it, so no other
/// request can remove or change it in between; the names the screen showed take no part.
///
/// A read that found no row or failed holds nothing, yet the COBOL still issues its `DELETE`,
/// which names no key and so can act only on a held record. The answer is the one that `DELETE`
/// gives, the `OTHER` branch of `DELETE-USER-SEC-FILE`, and the service deletes nothing. The
/// read's own `User ID NOT found...` or `Unable to lookup User...` is replaced in the same task
/// and never reaches the response. HALTED(`X-COUSR03C-H1`): this answer rests on IBM's CICS
/// documentation of `DELETE` and is not claimed as parity until the candidate is resolved; only
/// its store effect, no change to `users`, is (COUSR03C-018).
///
/// # Errors
///
/// `Unable to Update User...` on `first_name`, status class
/// [`super::cousr02c::FailureClass::Store`] (500), for [`UserRead::NotFound`] and
/// [`UserRead::Error`].
pub fn held_for_delete(result: UserRead<'_>) -> Result<&PicX<8>, UserMaintFailure> {
    reread(result)
        .map(|stored| &stored.user_id)
        .map_err(|_| UserMaintFailure::update_failed())
}

/// The success message of a delete: `User `, the user ID up to its first space, and
/// ` has been deleted ...`, shown in green (COUSR03C-019, COUSR03C-020).
///
/// `STRING … DELIMITED BY SPACE` takes the ID's bytes only up to its first space, so `AB CD`
/// gives `User AB has been deleted ...` and an ID that starts with a space gives
/// `User  has been deleted ...`, with two spaces. The longest message, for an 8-byte ID, has 34
/// characters and fits the 78-byte message row.
#[must_use]
pub fn deleted_message(user_id: &PicX<8>) -> String {
    let mut message = String::from(DELETED_HEAD);
    message.extend(
        user_id
            .as_bytes()
            .iter()
            .take_while(|&&byte| byte != SPACE)
            .map(|&byte| char::from(byte)),
    );
    message.push_str(DELETED_TAIL);
    message
}

/// The store's answer to the delete of the held row: the `RESP` of `DELETE-USER-SEC-FILE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeleteResult {
    /// `NORMAL`: the row was removed whole, `password_hash` and `filler` with it, and the
    /// transaction commits (COUSR03C-019).
    Deleted,
    /// `NOTFND`: the delete affected no row (COUSR03C-021).
    NotFound,
    /// `OTHER`: the delete failed for any other reason; the transaction rolls back, so the row
    /// stays (COUSR03C-022).
    Error,
}

/// The answer to endpoint 20 once the store has deleted the held row or failed to: the success
/// message (COUSR03C-019, COUSR03C-020), or the delete's failure (COUSR03C-021, COUSR03C-022).
///
/// `user_id` is the key [`held_for_delete`] returned, the `SEC-USR-ID` the message names.
///
/// # Errors
///
/// - `User ID NOT found...` on `user_id`, status class
///   [`super::cousr02c::FailureClass::NotFound`] (404), for [`DeleteResult::NotFound`]. The
///   held row makes this unreachable after a successful read; the outcome is kept for a store
///   that reports it (COUSR03C-021).
/// - `Unable to Update User...` on `first_name`, status class
///   [`super::cousr02c::FailureClass::Store`] (500), for [`DeleteResult::Error`]: the program's
///   text for a failed delete, `Update` included (`X-COUSR03C-1`). The CICS codes the COBOL
///   displays have no counterpart (halt H4; COUSR03C-022).
pub fn delete_outcome(user_id: &PicX<8>, result: DeleteResult) -> Result<String, UserMaintFailure> {
    match result {
        DeleteResult::Deleted => Ok(deleted_message(user_id)),
        DeleteResult::NotFound => Err(UserMaintFailure::not_found()),
        DeleteResult::Error => Err(UserMaintFailure::update_failed()),
    }
}

#[cfg(test)]
mod tests {
    use copybook::PicX;

    use super::{
        DELETE_PROMPT, DeleteResult, delete_outcome, deleted_message, edit_delete, held_for_delete,
    };
    use crate::online::cousr02c::{StoredUserView, UPDATE_PROMPT, UserMaintFailure, UserRead};

    type TestResult = Result<(), String>;

    /// A failure as the response shows it: message, field name and HTTP status.
    type Shown = (&'static str, &'static str, u16);

    // The audited literals, written out apart from the module's own constants.
    const USER_ID_EMPTY: &str = "User ID can NOT be empty...";
    const USER_NOT_FOUND: &str = "User ID NOT found...";
    const UPDATE_FAILED: &str = "Unable to Update User...";

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

    // audit: COUSR03C-016
    #[test]
    fn a_blank_user_id_is_rejected_before_any_read() {
        let blank = (USER_ID_EMPTY, "user_id", 400);
        for raw in ["", " ", "        ", "         ", "\0\0\0\0\0\0\0\0"] {
            assert_eq!(failure_of(edit_delete(raw)), Some(blank), "{raw:?}");
        }
    }

    // audit: COUSR03C-017
    #[test]
    fn the_key_is_the_received_id_padded_to_eight() -> TestResult {
        let cases: [(&str, &[u8; 8]); 5] = [
            ("USER0001", b"USER0001"),
            ("user0001", b"user0001"),
            (" ADMIN", b" ADMIN  "),
            ("AB CD", b"AB CD   "),
            ("ADMIN0012", b"ADMIN001"),
        ];
        for (raw, key) in cases {
            let id = edit_delete(raw).map_err(|failure| failure.to_string())?;
            assert_eq!(id.as_bytes(), key, "{raw:?}");
        }
        Ok(())
    }

    // audit: COUSR03C-017
    #[test]
    fn a_found_row_is_held_for_the_delete_by_its_key() -> TestResult {
        let stored = user0001();
        let held = held_for_delete(UserRead::Found(&stored)).map_err(|f| f.to_string())?;
        assert_eq!(held.as_bytes(), b"USER0001");
        assert_eq!(
            delete_outcome(held, DeleteResult::Deleted),
            Ok("User USER0001 has been deleted ...".to_owned())
        );
        Ok(())
    }

    // HALTED(X-COUSR03C-H1): the replacement's stated answer for a `DELETE` issued with no
    // record held, not a parity result, so this test carries no audit tag.
    #[test]
    fn a_read_that_holds_nothing_answers_as_the_delete_would() {
        let held_nothing = Some((UPDATE_FAILED, "first_name", 500));
        assert_eq!(
            failure_of(held_for_delete(UserRead::NotFound)),
            held_nothing
        );
        assert_eq!(failure_of(held_for_delete(UserRead::Error)), held_nothing);
    }

    // audit: COUSR03C-019
    // audit: COUSR03C-021
    // audit: COUSR03C-022
    #[test]
    fn the_delete_answers_with_the_message_or_its_failure() {
        let id = text::<8>("USER0001");
        assert_eq!(
            delete_outcome(&id, DeleteResult::Deleted),
            Ok("User USER0001 has been deleted ...".to_owned())
        );
        assert_eq!(
            failure_of(delete_outcome(&id, DeleteResult::NotFound)),
            Some((USER_NOT_FOUND, "user_id", 404))
        );
        assert_eq!(
            failure_of(delete_outcome(&id, DeleteResult::Error)),
            Some((UPDATE_FAILED, "first_name", 500))
        );
    }

    // audit: COUSR03C-019
    // audit: COUSR03C-020
    #[test]
    fn the_message_takes_the_id_up_to_its_first_space() {
        for (id, message) in [
            ("USER0001", "User USER0001 has been deleted ..."),
            ("ADMIN", "User ADMIN has been deleted ..."),
            ("AB CD", "User AB has been deleted ..."),
            (" AB", "User  has been deleted ..."),
        ] {
            assert_eq!(deleted_message(&text(id)), message, "{id:?}");
        }
        assert!(deleted_message(&text("ADMIN001")).len() <= 78);
    }

    // audit: COUSR03C-024
    #[test]
    fn an_admin_may_delete_their_own_user() -> TestResult {
        let own = edit_delete("ADMIN001").map_err(|failure| failure.to_string())?;
        assert_eq!(
            delete_outcome(&own, DeleteResult::Deleted),
            Ok("User ADMIN001 has been deleted ...".to_owned())
        );
        Ok(())
    }

    // audit: COUSR03C-012
    #[test]
    fn the_delete_prompt_is_cousr03c_s_own_literal() {
        assert_eq!(DELETE_PROMPT, "Press PF5 key to delete this user ...");
        assert_ne!(DELETE_PROMPT, UPDATE_PROMPT);
    }

    #[test]
    fn delete_results_compare_and_display() {
        assert_ne!(DeleteResult::Deleted, DeleteResult::NotFound);
        assert_ne!(DeleteResult::NotFound, DeleteResult::Error);
        assert_eq!(format!("{:?}", DeleteResult::Error), "Error");
        let failure = delete_outcome(&text("USER0001"), DeleteResult::Error).err();
        assert_eq!(
            failure.map(|f| f.to_string()),
            Some(UPDATE_FAILED.to_owned())
        );
    }
}
