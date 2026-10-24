//! COUSR03C, user delete: the contract of endpoint 20, `DELETE /api/v1/users/{user_id}`, admin
//! only (`[A]`; transaction `CU03`, map `COUSR3A`; AAP 0.3.4.1).
//!
//! F5 (`F5=Delete`) is the only key the endpoint serves (COUSR03C-023). The request has no body:
//! the path `user_id` is the text of the `USRIDIN` input, the
//! [`UserIdPath`](super::cousr02c::UserIdPath) endpoints 18 to 20 share, which rejects a segment
//! longer than the map's 8 bytes (COUSR03-013). The handler passes the text that fits unchanged
//! to `api::services::cousr03c`. There `domain::online::cousr03c::edit_delete` pads it to its
//! `PIC X(8)` key, neither upper-cased nor trimmed, or rejects a blank one with
//! `User ID can NOT be empty...` before any read (COUSR03C-016, COUSR03C-017).
//!
//! ENTER's fetch is endpoint 18, the read COUSR03C shares with COUSR02C, so its contract lives
//! in [`super::cousr02c`]: its [`UserView`](super::cousr02c::UserView) carries this program's
//! `delete_prompt`, `Press PF5 key to delete this user ...` (COUSR03C-012). F3, F4 and F12 send
//! no request and belong to the screen (COUSR03C-006 to COUSR03C-008).
//!
//! # Response
//!
//! A deleted user answers 200 with a [`ScreenResult`](super::ScreenResult) whose message is the
//! one `domain::online::cousr03c::delete_outcome` renders: `User `, the user ID up to its first
//! space, and ` has been deleted ...`, such as `User USER0001 has been deleted ...`
//! (COUSR03C-019, COUSR03C-020). That message is a `String`, which `ScreenResult`'s
//! `From<String>` in the parent module takes unchanged, so the handler ends in
//! `.map(ScreenResult::from)` and this module declares no conversion of its own; a second
//! `From<String> for ScreenResult` would conflict with the parent's.
//!
//! A failure is a `domain::online::cousr02c::UserMaintFailure`, the type COUSR03C shares with
//! COUSR02C, which `crate::error` turns into a `ScreenError` with its audited status:
//!
//! - 400 `User ID can NOT be empty...` on `user_id` for a blank path ID (COUSR03C-016);
//! - 404 `User ID NOT found...` on `user_id` for a delete that affects no row (COUSR03C-021);
//! - 500 `Unable to Update User...` on `first_name` for a failed delete: the program's text,
//!   `Update` included and not corrected (COUSR03C-022, `X-COUSR03C-1`);
//! - the same 500 for a path ID with no row, or a failed read, which leaves nothing held for
//!   the delete. That answer is not claimed as parity while halt candidate `X-COUSR03C-H1` is
//!   open (COUSR03C-018).
//!
//! # Why no schema is defined here
//!
//! Endpoint 20 takes no body, and its 200 is the shared `ScreenResult` schema, the frontend's
//! `components["schemas"]["ScreenResult"]`. A delete-specific request or result type would only
//! restate it in the generated `schema.d.ts` (Generated API Types Only).
//!
//! # Identity and data
//!
//! The path `user_id` names the user to delete. The caller acts only as the verified token's
//! `AdminUser`, and nothing compares the two, so an admin may delete their own user
//! (COUSR03C-024, AAP 0.5.1). The map has no password field, and no answer of this endpoint
//! carries a password or a hash (Cardholder Data Redaction).

#[cfg(test)]
mod tests {
    use std::error::Error;

    use axum::http::StatusCode;
    use domain::copybook::PicX;
    use domain::online::cousr02c::UserMaintFailure;
    use domain::online::cousr03c::{DeleteResult, delete_outcome, edit_delete};
    use serde_json::json;

    use crate::dto::ScreenResult;
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn Error>>;

    /// A user ID as the 8-byte key `edit_delete` makes of it.
    fn key(id: &str) -> PicX<8> {
        PicX::moved_from(id.as_bytes())
    }

    /// The answer endpoint 20 gives once the store has answered the delete of `id` with
    /// `result`: the handler's `.map(ScreenResult::from)` and `.map_err(ScreenError::from)`.
    fn answer(id: &str, result: DeleteResult) -> Result<ScreenResult, ScreenError> {
        delete_outcome(&key(id), result)
            .map(ScreenResult::from)
            .map_err(ScreenError::from)
    }

    #[test]
    fn a_delete_answers_with_the_programs_message_verbatim() -> TestResult {
        // COUSR03C-019, COUSR03C-020, COUSR03C-024: every byte of the message, the two spaces
        // of an ID that starts with one and the trailing ` ...` included.
        for (id, message) in [
            ("USER0001", "User USER0001 has been deleted ..."),
            ("ADMIN001", "User ADMIN001 has been deleted ..."),
            ("AB CD", "User AB has been deleted ..."),
            (" AB", "User  has been deleted ..."),
        ] {
            let result = answer(id, DeleteResult::Deleted).map_err(|e| e.message().to_owned())?;
            assert_eq!(
                result,
                ScreenResult {
                    message: message.to_owned()
                },
                "{id:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn the_wire_body_is_the_shared_message_row_only() -> TestResult {
        // COUSR03C-019: `{ "message" }` and nothing else, the shape of `ScreenResult`.
        let result =
            answer("USER0001", DeleteResult::Deleted).map_err(|e| e.message().to_owned())?;
        assert_eq!(
            serde_json::to_value(&result)?,
            json!({ "message": "User USER0001 has been deleted ..." })
        );
        Ok(())
    }

    #[test]
    fn each_failure_answers_with_its_audited_status_message_and_field() {
        // COUSR03C-016, COUSR03C-021, COUSR03C-022 (`X-COUSR03C-1`).
        let id = key("USER0001");
        let cases: [(Option<UserMaintFailure>, StatusCode, &str, &str); 3] = [
            (
                edit_delete("        ").err(),
                StatusCode::BAD_REQUEST,
                "User ID can NOT be empty...",
                "user_id",
            ),
            (
                delete_outcome(&id, DeleteResult::NotFound).err(),
                StatusCode::NOT_FOUND,
                "User ID NOT found...",
                "user_id",
            ),
            (
                delete_outcome(&id, DeleteResult::Error).err(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "Unable to Update User...",
                "first_name",
            ),
        ];
        for (failure, status, message, field) in cases {
            let error = failure.map(ScreenError::from);
            assert_eq!(
                error
                    .as_ref()
                    .map(|error| (error.status(), error.message(), error.field())),
                Some((status, message, Some(field))),
                "{message}"
            );
        }
    }
}
