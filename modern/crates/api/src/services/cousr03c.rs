//! COUSR03C, user delete: the service of endpoint 20, `DELETE /api/v1/users/{user_id}`, admin
//! only (transaction `CU03`, map `COUSR3A`; AAP 0.3.4.1). The `AdminUser` argument is the gate,
//! already passed (COUSR03C-001, COUSR03C-002).
//!
//! It ports PF5, `DELETE-USER-INFO` with its `READ-USER-SEC-FILE` and `DELETE-USER-SEC-FILE`
//! (`app/cbl/COUSR03C.cbl:174-192,267-336`). Every rule lives in `domain::online::cousr03c`;
//! this module only reads and deletes the row through `UserRepository`, maps each store answer
//! into the domain's `UserRead` or `DeleteResult`, and returns the domain's outcome (Thin Request
//! Handlers). ENTER's fetch is endpoint 18, the read COUSR03C shares with COUSR02C, served by
//! `services::cousr02c::fetch`.
//!
//! # Flow
//!
//! One `PgTx`, as the CICS `READ … UPDATE` holds the record until the `DELETE`:
//!
//! 1. The path ID goes through `edit_delete`: a blank one is `User ID can NOT be empty...`
//!    (400) before any read (COUSR03C-016).
//! 2. The row of that exact key, padded to 8 bytes and neither upper-cased nor trimmed, is read
//!    `FOR UPDATE` (COUSR03C-017). `held_for_delete` yields the key of the row it found.
//! 3. A read that found no row or failed holds nothing. The COBOL still issues its `DELETE`,
//!    which names no key and so acts only on a held record; `held_for_delete` answers with that
//!    `DELETE`'s `OTHER` branch, `Unable to Update User...` (500), and no delete statement runs.
//!    HALTED(`X-COUSR03C-H1`): that answer is not claimed as parity; only its store effect, no
//!    change to `users`, is (COUSR03C-018).
//! 4. The held row is deleted, the store's answer becomes a `DeleteResult`, and
//!    `delete_outcome` answers: `User <id> has been deleted ...` (COUSR03C-019, COUSR03C-020),
//!    `User ID NOT found...` (404) for a delete that affects no row (COUSR03C-021), or
//!    `Unable to Update User...` (500), the program's text for a failed delete, `Update` kept
//!    (COUSR03C-022, `X-COUSR03C-1`).
//! 5. The transaction commits only on the success message; every failure rolls back, so the row
//!    stays.
//!
//! The read's prompt, `Press PF5 key to delete this user ...`, is replaced in the same task by
//! the delete's message, so this endpoint never returns it (COUSR03C-017).
//!
//! # Store failures
//!
//! A failed read or delete is answered with the program's own message, as its `WHEN OTHER`
//! branch does. In place of the `DISPLAY` of the CICS `RESP` and `RESP2` codes, which have no
//! counterpart (halt H4), a tracing event names the paragraph and the error, which carries only
//! an SQLSTATE, a constraint or a column name. A failed `begin`, `commit` or `rollback` has no
//! audited text and is `ServiceError::Store`.
//!
//! # Identity and data (AAP 0.5.1, Cardholder Data Redaction)
//!
//! The path ID names the target user and is validated input. The caller acts only as the
//! verified token's `AdminUser`, and nothing compares the two, so an admin may delete any user,
//! their own and the last admin's included (COUSR03C-024). The row's hash is dropped unread by
//! `StoredUserView::from`; no value of the row is logged, and the success message names
//! only the user ID.

use domain::copybook::PicX;
use domain::online::cousr02c::{StoredUserView, UserMaintFailure, UserRead};
use domain::online::cousr03c::{self as rules, DeleteResult};
use domain::records::UserEntry;
use persistence::{PersistenceError, UserRepository};

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::error::ServiceError;

/// The program the tracing events of a store failure name.
const PROGRAM: &str = "COUSR03C";

/// The paragraph of PF5's `READ … UPDATE` (COUSR03C-018).
const READ_PARAGRAPH: &str = "READ-USER-SEC-FILE";

/// The paragraph of the `DELETE` (COUSR03C-022).
const DELETE_PARAGRAPH: &str = "DELETE-USER-SEC-FILE";

/// Endpoint 20: the PF5 delete of the user under the path ID. Returns the success message,
/// `User <id> has been deleted ...` (COUSR03C-016 to COUSR03C-022, COUSR03C-024).
///
/// # Errors
///
/// - `ServiceError::Rule` with the domain's failure: `User ID can NOT be empty...` (400) for a
///   blank ID; `User ID NOT found...` (404) for a delete that affects no row; `Unable to Update
///   User...` (500) for a failed delete, and for a key with no row or a failed read, which leave
///   nothing held for the delete.
/// - `ServiceError::Store` when the transaction cannot begin, commit or roll back.
pub(crate) async fn delete(
    state: &AppState,
    _admin: &AdminUser,
    user_id: &str,
) -> Result<String, ServiceError<UserMaintFailure>> {
    let id = rules::edit_delete(user_id).map_err(ServiceError::Rule)?;
    let mut tx = state.store.begin().await?;
    match delete_held(&mut tx, &id).await {
        Ok(message) => {
            tx.commit().await?;
            Ok(message)
        }
        Err(failure) => {
            tx.rollback().await?;
            Err(ServiceError::Rule(failure))
        }
    }
}

/// The locked read and the delete of `id` on any `users` executor: the open transaction in
/// service, a test double in the unit tests below (COUSR03C-017 to COUSR03C-022).
///
/// No delete statement runs unless the read found the row.
async fn delete_held<R: UserRepository>(
    store: &mut R,
    id: &PicX<8>,
) -> Result<String, UserMaintFailure> {
    let looked = looked_up(UserRepository::lock_by_id(store, id).await);
    let read = looked
        .as_ref()
        .map_or_else(|&absent| absent, UserRead::Found);
    let held = rules::held_for_delete(read)?;
    let result = delete_result(UserRepository::delete(store, held).await);
    rules::delete_outcome(held, result)
}

/// The store's answer to the locked read: the row without its hash, or the domain's `UserRead`
/// for its absence, `NotFound` for no row and `Error` for any other failure, which is traced.
fn looked_up(
    result: Result<Option<UserEntry>, PersistenceError>,
) -> Result<StoredUserView, UserRead<'static>> {
    match result {
        Ok(Some(entry)) => Ok(StoredUserView::from(entry)),
        Ok(None) => Err(UserRead::NotFound),
        Err(error) => {
            traced(READ_PARAGRAPH, &error);
            Err(UserRead::Error)
        }
    }
}

/// The store's answer to the delete: `Deleted`, `NotFound` when no row was removed, or `Error`
/// for any other failure, which is traced (COUSR03C-019, COUSR03C-021, COUSR03C-022).
fn delete_result(result: Result<bool, PersistenceError>) -> DeleteResult {
    match result {
        Ok(true) => DeleteResult::Deleted,
        Ok(false) => DeleteResult::NotFound,
        Err(error) => {
            traced(DELETE_PARAGRAPH, &error);
            DeleteResult::Error
        }
    }
}

/// The tracing event that replaces the `DISPLAY 'RESP:' … 'REAS:'` of a `WHEN OTHER` branch.
///
/// The error's `Display` holds only an SQLSTATE, a constraint name or a column name, never a
/// stored or request value (Cardholder Data Redaction).
fn traced(paragraph: &'static str, error: &PersistenceError) {
    tracing::warn!(
        program = PROGRAM,
        paragraph,
        error = %error,
        "USRSEC access failed"
    );
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::future::{Future, ready};
    use std::path::PathBuf;
    use std::sync::Arc;

    use domain::SystemClock;
    use domain::batch::loads::HashedUser;
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use domain::online::cousr02c::UserMaintFailure;
    use domain::online::cousr03c::edit_delete;
    use domain::records::{UserEntry, UserListEntry};
    use persistence::{PersistenceError, PgStore, UserRepository};
    use test_support::postgres::start_postgres;
    use test_support::trace_capture;

    use super::{delete, delete_held};
    use crate::app::AppState;
    use crate::auth::extract::{AdminUser, SignedOnUser};
    use crate::auth::jwt::JwtKeys;
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A failure as the response shows it: message, field name and HTTP status.
    type Shown = (&'static str, &'static str, u16);

    /// A local-only signing secret for these tests.
    const SECRET: &str = "services-cousr03c-test-secret";

    /// The audited answers, written out apart from the domain's constants.
    const USER_ID_EMPTY: Shown = ("User ID can NOT be empty...", "user_id", 400);
    const NOT_FOUND: Shown = ("User ID NOT found...", "user_id", 404);
    const UPDATE_FAILED: Shown = ("Unable to Update User...", "first_name", 500);

    /// The SQLSTATE of the failed delete the double reports, `57014` (query canceled).
    const CANCELED: &str = "57014";

    /// A `users` executor that answers the locked read with `lock` and the delete with `delete`,
    /// recording the key of each. Every other access reports the store unavailable, so a test
    /// reaching one fails.
    struct Scripted {
        lock: Result<Option<UserEntry>, PersistenceError>,
        delete: Result<bool, PersistenceError>,
        locked: Vec<PicX<8>>,
        deleted: Vec<PicX<8>>,
    }

    impl Scripted {
        fn answering(
            lock: Result<Option<UserEntry>, PersistenceError>,
            delete: Result<bool, PersistenceError>,
        ) -> Self {
            Self {
                lock,
                delete,
                locked: Vec::new(),
                deleted: Vec::new(),
            }
        }
    }

    impl UserRepository for Scripted {
        fn find_by_id(
            &mut self,
            _id: &PicX<8>,
        ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn lock_by_id(
            &mut self,
            id: &PicX<8>,
        ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
            self.locked.push(id.clone());
            ready(self.lock.clone())
        }

        fn page_after(
            &mut self,
            _start: &PicX<8>,
            _limit: i64,
        ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn page_before(
            &mut self,
            _start: &PicX<8>,
            _limit: i64,
        ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn insert(
            &mut self,
            _entry: &UserEntry,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn update(
            &mut self,
            _entry: &UserEntry,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn delete(
            &mut self,
            id: &PicX<8>,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            self.deleted.push(id.clone());
            ready(self.delete.clone())
        }

        fn replace_all(
            &mut self,
            _users: &[HashedUser],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }
    }

    /// A `users` row of `LAWRENCE THOMAS`, type `U`, under `id`. Nothing here verifies the
    /// hash, so it is a fixed text rather than an argon2id string.
    fn entry(id: &str) -> Result<UserEntry, Box<dyn StdError>> {
        Ok(UserEntry {
            id: PicX::from_text(id)?,
            fname: PicX::from_text("LAWRENCE")?,
            lname: PicX::from_text("THOMAS")?,
            password_hash: "unverified-test-hash".to_owned(),
            usr_type: PicX::from_array([b'U']),
            filler: PicX::from_array([b' '; 23]),
        })
    }

    /// The 8-byte key `edit_delete` makes of `raw`.
    fn key(raw: &str) -> Result<PicX<8>, Box<dyn StdError>> {
        edit_delete(raw).map_err(|failure| failure.message().into())
    }

    fn shown_failure(failure: &UserMaintFailure) -> Shown {
        (
            failure.message(),
            failure.field().name(),
            failure.class().http_status(),
        )
    }

    /// The domain failure of `result`, as the response shows it; `None` for anything else.
    fn shown<T>(result: &Result<T, ServiceError<UserMaintFailure>>) -> Option<Shown> {
        match result {
            Err(ServiceError::Rule(failure)) => Some(shown_failure(failure)),
            _ => None,
        }
    }

    fn app_state(store: PgStore) -> AppState {
        AppState::new(
            store,
            JwtKeys::new(SECRET, 3600),
            Arc::new(SystemClock),
            PathBuf::from("var/jobs"),
        )
    }

    fn admin() -> Result<AdminUser, Box<dyn StdError>> {
        Ok(AdminUser(SignedOnUser {
            user_id: PicX::from_text("ADMIN001")?,
            user_type: UserType::ADMIN,
        }))
    }

    async fn stored(store: &PgStore, id: &str) -> Result<Option<UserEntry>, Box<dyn StdError>> {
        let mut store = store.clone();
        Ok(UserRepository::find_by_id(&mut store, &PicX::from_text(id)?).await?)
    }

    // audit: COUSR03C-017
    // audit: COUSR03C-019
    // audit: COUSR03C-020
    #[tokio::test]
    async fn the_held_row_is_deleted_under_the_received_key() -> TestResult {
        for (raw, message) in [
            ("USER0001", "User USER0001 has been deleted ..."),
            ("user0001", "User user0001 has been deleted ..."),
            ("AB CD", "User AB has been deleted ..."),
            (" ADMIN", "User  has been deleted ..."),
        ] {
            let id = key(raw)?;
            let mut store = Scripted::answering(Ok(Some(entry(raw)?)), Ok(true));
            let answer = delete_held(&mut store, &id)
                .await
                .map_err(|f| f.message())?;
            assert_eq!(answer, message, "{raw:?}");
            // One locked read and one delete, both of the padded key as received.
            assert_eq!(store.locked, [id.clone()], "{raw:?}");
            assert_eq!(store.deleted, [id], "{raw:?}");
        }
        Ok(())
    }

    // audit: COUSR03C-021
    // audit: COUSR03C-022
    #[tokio::test]
    async fn a_delete_that_removes_nothing_or_fails_is_refused() -> TestResult {
        let traces = trace_capture::capture();
        let failed = PersistenceError::Database {
            sqlstate: Some(CANCELED.to_owned()),
            constraint: None,
        };
        for (answer, expected) in [(Ok(false), NOT_FOUND), (Err(failed), UPDATE_FAILED)] {
            let mut store = Scripted::answering(Ok(Some(entry("USER0001")?)), answer);
            let outcome = delete_held(&mut store, &key("USER0001")?).await;
            assert_eq!(
                outcome.as_ref().err().map(shown_failure),
                Some(expected),
                "{}",
                expected.0
            );
        }
        // The `DISPLAY` of the codes becomes an event naming the paragraph and the SQLSTATE.
        assert!(traces.contains("DELETE-USER-SEC-FILE"));
        assert!(traces.contains(CANCELED));
        assert!(!traces.contains("unverified-test-hash"));
        Ok(())
    }

    // audit: COUSR03C-018
    #[tokio::test]
    async fn a_read_that_holds_nothing_issues_no_delete() -> TestResult {
        // No row, or a failed read: halt candidate X-COUSR03C-H1 leaves only the store effect
        // assertable, so the answer is checked to be no success and no delete to have run.
        let traces = trace_capture::capture();
        for lock in [Ok(None), Err(PersistenceError::Unavailable)] {
            let mut store = Scripted::answering(lock, Ok(true));
            let outcome = delete_held(&mut store, &key("NOBODY01")?).await;
            assert!(outcome.is_err());
            assert_eq!(store.locked.len(), 1);
            assert!(store.deleted.is_empty());
        }
        assert!(traces.contains("READ-USER-SEC-FILE"));
        Ok(())
    }

    // audit: COUSR03C-016
    // audit: COUSR03C-017
    // audit: COUSR03C-018
    // audit: COUSR03C-019
    // audit: COUSR03C-020
    // audit: COUSR03C-024
    #[tokio::test]
    async fn the_delete_removes_only_the_named_row() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        for id in ["USER0001", "ADMIN001", "AB CD"] {
            UserRepository::insert(&mut store, &entry(id)?).await?;
        }
        let state = app_state(store.clone());
        let admin = admin()?;

        // A blank ID is refused before any read, and nothing changes.
        let blank = delete(&state, &admin, "        ").await;
        assert_eq!(shown(&blank), Some(USER_ID_EMPTY));

        let message = delete(&state, &admin, "USER0001")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(message, "User USER0001 has been deleted ...");
        assert!(stored(&store, "USER0001").await?.is_none());
        assert!(stored(&store, "ADMIN001").await?.is_some());

        let spaced = delete(&state, &admin, "AB CD")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(spaced, "User AB has been deleted ...");
        assert!(stored(&store, "AB CD").await?.is_none());

        // No row has the key, the lower-case form of a stored ID included: only the store
        // effect is asserted (halt candidate X-COUSR03C-H1), and the stored admin remains.
        for missing in ["NOBODY01", "admin001", "USER0001"] {
            assert!(delete(&state, &admin, missing).await.is_err(), "{missing}");
        }
        let kept = stored(&store, "ADMIN001").await?;
        assert_eq!(
            kept.map(|row| (row.id, row.fname, row.password_hash)),
            Some((
                PicX::from_text("ADMIN001")?,
                PicX::from_text("LAWRENCE")?,
                "unverified-test-hash".to_owned()
            ))
        );

        // The caller may delete their own user: nothing compares the target with the token.
        let own = delete(&state, &admin, "ADMIN001")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(own, "User ADMIN001 has been deleted ...");
        assert!(stored(&store, "ADMIN001").await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn a_transaction_that_cannot_begin_is_a_store_error() -> TestResult {
        let guard = start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let state = app_state(store.clone());
        store.pool().close().await;
        assert_eq!(
            delete(&state, &admin()?, "USER0001").await,
            Err(ServiceError::Store(PersistenceError::Unavailable))
        );
        Ok(())
    }
}
