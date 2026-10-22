//! COUSR02C's file statements for endpoint 18, `GET /api/v1/users/{user_id}`, the read COUSR02C
//! (`CU02`) shares with COUSR03C (`CU03`), and endpoint 19, `PUT /api/v1/users/{user_id}`,
//! COUSR02C's PF5 update. Both are admin-only; the `AdminUser` argument is the gate, already
//! passed (COUSR02C-001, COUSR02C-002).
//!
//! Every rule lives in `domain::online::cousr02c`; this module only reads and writes `users`
//! through `UserRepository`, maps each store answer into the domain's `UserRead` or
//! `RewriteResult`, and returns the domain's outcome (Thin Request Handlers).
//!
//! # Endpoint 18 (`fetch`)
//!
//! `PROCESS-ENTER-KEY` and `READ-USER-SEC-FILE`: the path ID goes through `edit_user_id`
//! (COUSR02C-010, COUSR03C-010), then one read of that exact key (COUSR02C-011). A row is the
//! domain's `UserView` without a password or a hash (COUSR02C-012); no row is `User ID NOT
//! found...` (COUSR02C-015, COUSR03C-013); any other failure is `Unable to lookup User...`
//! (COUSR02C-016, COUSR03C-014). The COBOL reads `UPDATE`, but that hold ends with the task, so
//! the read runs on a pooled connection, takes no lock and writes nothing.
//!
//! # Endpoint 19 (`update`)
//!
//! `UPDATE-USER-INFO` and `UPDATE-USER-SEC-FILE`, in one `PgTx`:
//!
//! 1. The empty-field edits, in the program's order, before any read (COUSR02C-018 to
//!    COUSR02C-023).
//! 2. The re-read `FOR UPDATE` of the path ID, which holds the row until the rewrite
//!    (COUSR02C-024). No row or a failed read rolls back with the domain's answer, and nothing
//!    is written (COUSR02C-025, halt candidate `X-COUSR02C-H1`).
//! 3. The change test against the row as read now: names and type byte for byte, a supplied
//!    password verified against the stored argon2id hash; nothing changed is `Please modify to
//!    update ...` (COUSR02C-026 to COUSR02C-028).
//! 4. The rewrite of the whole row: ID and `FILLER` as read, the request's names and type, and
//!    a new hash only when the password changed, else the stored one (COUSR02C-029, `E27`).
//! 5. Commit and `User <id> has been updated ...` (COUSR02C-029, COUSR02C-030); a rewrite that
//!    finds no row (COUSR02C-031) or fails (COUSR02C-032) rolls back with its message.
//!
//! # Password (`E27`, Cardholder Data Redaction)
//!
//! The COBOL pre-fills the dark field with `SEC-USR-PWD` and compares it with the record. Here
//! the password is never returned: an omitted one means unchanged, and a supplied one stays a
//! `Password` that only the domain's `verify` and `hash` see. The hash never leaves `UserEntry`
//! and `Held`, neither of which this module formats, and no value of either is logged.
//!
//! argon2id costs milliseconds of CPU per call, so the change test and the hashing run on
//! Tokio's blocking pool rather than on the thread that drives other requests. Their failures
//! are logged once the task is awaited, on the request's own thread and span: a stored hash that
//! cannot be verified, a new password that cannot be hashed, by the fixed class text of
//! `PasswordError`, and a task that did not complete, by whether it panicked, never its payload.
//! No event names the password, the hash or the user.
//!
//! # Store failures
//!
//! A failed read or rewrite is answered with the program's own message, as its `WHEN OTHER`
//! branch does. In place of the `DISPLAY` of the CICS `RESP` and `RESP2` codes, which have no
//! counterpart (halt H4), a tracing event names the paragraph and the error, which carries only
//! an SQLSTATE, a constraint or a column name (COUSR02C-016, COUSR02C-032). A failed `begin`,
//! `commit` or `rollback` has no audited text and is `ServiceError::Store`.
//!
//! # Identity (AAP 0.5.1)
//!
//! The path ID, names, type and password name the target user and are validated input. The
//! caller's authority is the verified token alone, which neither selects nor restricts the
//! target.

use domain::copybook::PicX;
use domain::online::cousr02c::{
    self as rules, FailureClass, RewriteResult, StoredUserView, UserMaintFailure, UserRead,
    UserUpdateInput, UserView, ValidatedUpdate,
};
use domain::password::{self, PasswordError};
use domain::records::UserEntry;
use persistence::{PersistenceError, PgTx, UserRepository};
use tokio::task::JoinError;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cousr02c::UserUpdateRequest;
use crate::error::ServiceError;

/// The program the tracing events of a store failure name.
const PROGRAM: &str = "COUSR02C";

/// The paragraph of the read, on ENTER and before the rewrite (COUSR02C-016).
const READ_PARAGRAPH: &str = "READ-USER-SEC-FILE";

/// The paragraph of the rewrite (COUSR02C-032).
const REWRITE_PARAGRAPH: &str = "UPDATE-USER-SEC-FILE";

/// The paragraph of the change test, whose password compare the replacement runs as argon2id
/// verification and hashing (COUSR02C-026 to COUSR02C-029, `E27`).
const CHANGE_PARAGRAPH: &str = "UPDATE-USER-INFO";

/// What both endpoints answer with when they do not succeed.
type Refusal = ServiceError<UserMaintFailure>;

/// Endpoint 18: the user stored under the path ID, for the update and the delete screen alike
/// (COUSR02C-010 to COUSR02C-012, COUSR02C-015, COUSR02C-016; COUSR03C-010, COUSR03C-013,
/// COUSR03C-014).
///
/// # Errors
///
/// `ServiceError::Rule` with the domain's failure: `User ID can NOT be empty...` (400) for a
/// blank ID, `User ID NOT found...` (404) when no row has the key, `Unable to lookup User...`
/// (500) when the read fails.
pub(crate) async fn fetch(
    state: &AppState,
    _admin: &AdminUser,
    user_id: &str,
) -> Result<UserView, Refusal> {
    let id = rules::edit_user_id(user_id).map_err(ServiceError::Rule)?;
    let mut store = state.store.clone();
    let looked =
        looked_up(UserRepository::find_by_id(&mut store, &id).await).map(StoredUserView::from);
    let read = looked
        .as_ref()
        .map_or_else(|&absent| absent, UserRead::Found);
    rules::read(read).map_err(ServiceError::Rule)
}

/// Endpoint 19: the PF5 update of the user under the path ID, with the request's names, type
/// and, when supplied, password (COUSR02C-018 to COUSR02C-032, `E27`). Returns the success
/// message, `User <id> has been updated ...`.
///
/// # Errors
///
/// - `ServiceError::Rule` with the domain's failure: an empty-field edit (400), `Please modify
///   to update ...` (400), `User ID NOT found...` (404) from the re-read or the rewrite, and
///   `Unable to lookup User...` or `Unable to Update User...` (500) when the re-read, the
///   stored hash's verification or the rewrite fails.
/// - `ServiceError::Store` when the transaction cannot begin, commit or roll back.
/// - `ServiceError::Internal` when the new password cannot be hashed or the blocking task that
///   runs argon2id does not complete.
pub(crate) async fn update(
    state: &AppState,
    _admin: &AdminUser,
    user_id: &str,
    request: UserUpdateRequest,
) -> Result<String, Refusal> {
    let UserUpdateRequest {
        first_name,
        last_name,
        password,
        user_type,
    } = request;
    let input = UserUpdateInput {
        first_name: &first_name,
        last_name: &last_name,
        password: password.as_deref(),
        user_type: &user_type,
    };
    let update = rules::edit_update(user_id, &input).map_err(ServiceError::Rule)?;

    let mut tx = state.store.begin().await?;
    let looked = looked_up(UserRepository::lock_by_id(&mut tx, update.user_id()).await);
    match looked {
        Ok(entry) => rewrite(tx, update, Held::from(entry)).await,
        Err(absent) => abandon(tx, reread_refusal(absent)).await,
    }
}

/// The row the locked re-read holds, split as the change test and the rewrite take it: the
/// stored view the domain compares with, and the hash and `FILLER` that the view leaves out and
/// the rewrite keeps.
///
/// Neither `Debug` nor any other formatting exists, so the hash cannot be printed.
struct Held {
    /// ID, names and type as stored.
    stored: StoredUserView,
    /// The argon2id PHC string standing for `SEC-USR-PWD` (`E25`).
    password_hash: String,
    /// `SEC-USR-FILLER PIC X(23)`, rewritten unchanged (COUSR02C-029).
    filler: PicX<23>,
}

impl From<UserEntry> for Held {
    fn from(mut entry: UserEntry) -> Self {
        let password_hash = core::mem::take(&mut entry.password_hash);
        let filler = entry.filler.clone();
        Self {
            stored: StoredUserView::from(entry),
            password_hash,
            filler,
        }
    }
}

/// The store's answer to a read by key: the row, or the domain's `UserRead` for its absence,
/// `NotFound` for no row and `Error` for any other failure, which is traced.
fn looked_up(
    result: Result<Option<UserEntry>, PersistenceError>,
) -> Result<UserEntry, UserRead<'static>> {
    match result {
        Ok(Some(entry)) => Ok(entry),
        Ok(None) => Err(UserRead::NotFound),
        Err(error) => {
            traced(READ_PARAGRAPH, &error);
            Err(UserRead::Error)
        }
    }
}

/// The domain's answer to a re-read that found no row or failed: `User ID NOT found...` or
/// `Unable to lookup User...` (COUSR02C-025).
///
/// `rules::reread` returns a row only for `UserRead::Found`, which `looked_up` never yields as
/// an absence, so that arm is the 500 of an inconsistent answer rather than a success.
fn reread_refusal(absent: UserRead<'_>) -> Refusal {
    rules::reread(absent).map_or_else(ServiceError::Rule, |_| ServiceError::Internal)
}

/// The change test, the rewrite and its outcome, inside the transaction that holds the row
/// (COUSR02C-026 to COUSR02C-032). Commits only on the domain's success.
async fn rewrite(
    mut tx: PgTx<'static>,
    update: ValidatedUpdate,
    held: Held,
) -> Result<String, Refusal> {
    let entry = match changed_entry(update, held).await {
        Ok(entry) => entry,
        Err(refusal) => return abandon(tx, refusal).await,
    };
    let result = rewrite_result(UserRepository::update(&mut tx, &entry).await);
    match rules::rewrite_outcome(&entry.id, result) {
        Ok(message) => {
            tx.commit().await?;
            Ok(message)
        }
        Err(failure) => abandon(tx, ServiceError::Rule(failure)).await,
    }
}

/// The row to rewrite, from the change test on Tokio's blocking pool, where argon2id verifies
/// a supplied password and hashes a changed one; [`answered`] logs its failures once it is
/// awaited.
async fn changed_entry(update: ValidatedUpdate, held: Held) -> Result<UserEntry, Refusal> {
    let joined = tokio::task::spawn_blocking(move || changed(&update, held)).await;
    answered(joined)
}

/// Why the change test yields no row to rewrite.
enum Unchanged {
    /// The domain's answer: `Please modify to update ...` (COUSR02C-028), or `Unable to lookup
    /// User...` for a stored hash that cannot be verified (`X-COUSR02C-4`).
    Refused(UserMaintFailure),
    /// argon2id could not hash the changed password.
    NotHashed(PasswordError),
}

/// The change test's answer, on the thread that awaited its task: the row, the domain's
/// refusal, or `ServiceError::Internal` when the new password cannot be hashed or the task did
/// not complete.
///
/// Each operational failure is logged here, once, rather than on the blocking pool, so that the
/// event falls in the request's span: the unverifiable stored hash as `WARN`, since the
/// program's own message answers it; the hash failure, by the fixed class text of
/// [`PasswordError`], and the task failure, by whether it panicked and never its payload, as
/// `ERROR`, since both answer the unaudited 500. A refusal for nothing modified is the user's
/// input and logs nothing.
fn answered(joined: Result<Result<UserEntry, Unchanged>, JoinError>) -> Result<UserEntry, Refusal> {
    match joined {
        Ok(Ok(entry)) => Ok(entry),
        Ok(Err(Unchanged::Refused(failure))) => {
            // `rules::modified` refuses with a store-class failure only when the stored hash
            // cannot be verified; its other refusal is the input class's no-change message.
            if failure.class() == FailureClass::Store {
                tracing::warn!(
                    program = PROGRAM,
                    paragraph = CHANGE_PARAGRAPH,
                    "stored password hash could not be verified; answering Unable to lookup User"
                );
            }
            Err(ServiceError::Rule(failure))
        }
        Ok(Err(Unchanged::NotHashed(error))) => {
            tracing::error!(
                program = PROGRAM,
                paragraph = CHANGE_PARAGRAPH,
                error = %error,
                "new password not hashed; answering 500"
            );
            Err(ServiceError::Internal)
        }
        Err(error) => {
            tracing::error!(
                program = PROGRAM,
                paragraph = CHANGE_PARAGRAPH,
                panicked = error.is_panic(),
                "password task did not complete; answering 500"
            );
            Err(ServiceError::Internal)
        }
    }
}

/// The change test against the held row and the row it yields: the request's names and type,
/// the held ID's key and `FILLER`, and a new hash only for a changed password (COUSR02C-026 to
/// COUSR02C-029, `E27`).
///
/// It runs on the blocking pool and logs nothing: [`answered`] logs what it returns.
fn changed(update: &ValidatedUpdate, held: Held) -> Result<UserEntry, Unchanged> {
    let changes =
        rules::modified(update, &held.stored, &held.password_hash).map_err(Unchanged::Refused)?;
    let password_hash = match changes.new_password() {
        Some(new) => password::hash(new.expose()).map_err(Unchanged::NotHashed)?,
        None => held.password_hash,
    };
    Ok(UserEntry {
        id: changes.user_id().clone(),
        fname: changes.first_name().clone(),
        lname: changes.last_name().clone(),
        password_hash,
        usr_type: changes.user_type().clone(),
        filler: held.filler,
    })
}

/// The store's answer to the rewrite: `Rewritten`, `NotFound` when no row has the key, or
/// `Error` for any other failure, which is traced (COUSR02C-029, COUSR02C-031, COUSR02C-032).
fn rewrite_result(result: Result<bool, PersistenceError>) -> RewriteResult {
    match result {
        Ok(true) => RewriteResult::Rewritten,
        Ok(false) => RewriteResult::NotFound,
        Err(error) => {
            traced(REWRITE_PARAGRAPH, &error);
            RewriteResult::Error
        }
    }
}

/// Rolls the transaction back, so nothing is written, and answers with `refusal`.
///
/// # Errors
///
/// `refusal`; `ServiceError::Store` when the rollback itself fails.
async fn abandon<T>(tx: PgTx<'_>, refusal: Refusal) -> Result<T, Refusal> {
    tx.rollback().await?;
    Err(refusal)
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
    use std::path::PathBuf;
    use std::sync::Arc;

    use domain::SystemClock;
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use domain::online::cousr02c::{self as rules, RewriteResult, UserMaintFailure, UserRead};
    use domain::password::{self, PasswordError};
    use domain::records::UserEntry;
    use persistence::{PersistenceError, PgStore, UserRepository};
    use test_support::postgres::start_postgres;
    use test_support::trace_capture;

    use super::{Unchanged, answered, fetch, reread_refusal, rewrite_result, update};
    use crate::app::AppState;
    use crate::auth::extract::{AdminUser, SignedOnUser};
    use crate::auth::jwt::JwtKeys;
    use crate::dto::cousr02c::UserUpdateRequest;
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A failure as the response shows it: message, field name and HTTP status.
    type Shown = (&'static str, &'static str, u16);

    /// A local-only signing secret for these tests.
    const SECRET: &str = "services-cousr02c-test-secret";

    /// The synthetic password the seeded users are stored with.
    const STORED_PASSWORD: &str = "TESTPW01";

    /// The synthetic password the update tests submit.
    const NEW_PASSWORD: &str = "NEWPW123";

    /// The `FILLER` every seeded row holds, so a rewrite that loses it shows.
    const FILLER: [u8; 23] = [b'Z'; 23];

    /// The audited answers, written out apart from the domain's constants.
    const NOT_FOUND: Shown = ("User ID NOT found...", "user_id", 404);
    const LOOKUP_FAILED: Shown = ("Unable to lookup User...", "first_name", 500);
    const NOT_MODIFIED: Shown = ("Please modify to update ...", "first_name", 400);

    fn password_bytes(text: &str) -> Result<&[u8; 8], Box<dyn StdError>> {
        Ok(<&[u8; 8]>::try_from(text.as_bytes())?)
    }

    /// A `users` row: `LAWRENCE THOMAS`, type `U`, with the given key and hash.
    fn entry(id: &str, password_hash: String) -> Result<UserEntry, Box<dyn StdError>> {
        Ok(UserEntry {
            id: PicX::from_text(id)?,
            fname: PicX::from_text("LAWRENCE")?,
            lname: PicX::from_text("THOMAS")?,
            password_hash,
            usr_type: PicX::from_array([b'U']),
            filler: PicX::from_array(FILLER),
        })
    }

    /// Stores `ids`, each with the hash of [`STORED_PASSWORD`].
    async fn seed(store: &mut PgStore, ids: &[&str]) -> TestResult {
        for id in ids {
            let hash = password::hash(password_bytes(STORED_PASSWORD)?)?;
            UserRepository::insert(store, &entry(id, hash)?).await?;
        }
        Ok(())
    }

    async fn stored(store: &PgStore, id: &str) -> Result<UserEntry, Box<dyn StdError>> {
        let mut store = store.clone();
        UserRepository::find_by_id(&mut store, &PicX::from_text(id)?)
            .await?
            .ok_or_else(|| format!("no row for {id:?}").into())
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

    fn request(first: &str, last: &str, password: Option<&str>, kind: &str) -> UserUpdateRequest {
        UserUpdateRequest {
            first_name: first.to_owned(),
            last_name: last.to_owned(),
            password: password.map(str::to_owned),
            user_type: kind.to_owned(),
        }
    }

    /// The domain failure of `result`, as the response shows it; `None` for anything else.
    fn shown<T>(result: &Result<T, ServiceError<UserMaintFailure>>) -> Option<Shown> {
        match result {
            Err(ServiceError::Rule(failure)) => Some((
                failure.message(),
                failure.field().name(),
                failure.class().http_status(),
            )),
            _ => None,
        }
    }

    #[tokio::test]
    async fn the_shared_read_answers_the_row_or_its_failure() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        seed(&mut store, &["USER0001"]).await?;
        let state = app_state(store);
        let admin = admin()?;

        // audit: COUSR02C-011
        // audit: COUSR02C-012
        let view = fetch(&state, &admin, "USER0001")
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(
            (
                view.user_id,
                view.first_name,
                view.last_name,
                view.user_type
            ),
            (
                PicX::from_text("USER0001")?,
                PicX::from_text("LAWRENCE")?,
                PicX::from_text("THOMAS")?,
                PicX::from_array([b'U']),
            )
        );

        // audit: COUSR02C-010
        // audit: COUSR03C-010
        // audit: COUSR02C-015
        // audit: COUSR03C-013
        let blank = ("User ID can NOT be empty...", "user_id", 400);
        let cases = [
            ("", blank),
            ("        ", blank),
            ("NOBODY01", NOT_FOUND),
            ("user0001", NOT_FOUND),
            (" USER0001", NOT_FOUND),
        ];
        for (user_id, expected) in cases {
            let outcome = fetch(&state, &admin, user_id).await;
            assert_eq!(shown(&outcome), Some(expected), "{user_id:?}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn the_edits_run_in_order_before_any_read_and_write_nothing() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        seed(&mut store, &["USER0001"]).await?;
        let state = app_state(store.clone());
        let admin = admin()?;
        let before = stored(&store, "USER0001").await?;

        // audit: COUSR02C-018
        // audit: COUSR02C-019
        // audit: COUSR02C-020
        // audit: COUSR02C-021
        // audit: COUSR02C-022
        let cases = [
            (
                "  ",
                request("", "", None, ""),
                ("User ID can NOT be empty...", "user_id"),
            ),
            (
                "USER0001",
                request("", "", None, ""),
                ("First Name can NOT be empty...", "first_name"),
            ),
            (
                "USER0001",
                request("A", " ", None, ""),
                ("Last Name can NOT be empty...", "last_name"),
            ),
            (
                "USER0001",
                request("A", "B", Some(" "), ""),
                ("Password can NOT be empty...", "password"),
            ),
            (
                "USER0001",
                request("A", "B", None, ""),
                ("User Type can NOT be empty...", "user_type"),
            ),
            (
                "NOBODY01",
                request("", "B", None, "U"),
                ("First Name can NOT be empty...", "first_name"),
            ),
        ];
        for (user_id, body, (message, field)) in cases {
            let outcome = update(&state, &admin, user_id, body).await;
            assert_eq!(shown(&outcome), Some((message, field, 400)), "{message}");
        }

        // audit: COUSR02C-026
        // audit: COUSR02C-027
        // audit: COUSR02C-028
        for password in [None, Some(STORED_PASSWORD)] {
            let body = request("LAWRENCE", "THOMAS", password, "U");
            let outcome = update(&state, &admin, "USER0001", body).await;
            assert_eq!(
                shown(&outcome),
                Some(NOT_MODIFIED),
                "{}",
                password.is_some()
            );
        }
        assert_eq!(stored(&store, "USER0001").await?, before);
        Ok(())
    }

    #[tokio::test]
    async fn a_change_rewrites_the_whole_held_row() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        seed(&mut store, &["USER0001", "AB CD"]).await?;
        let state = app_state(store.clone());
        let admin = admin()?;
        let before = stored(&store, "USER0001").await?;

        // audit: COUSR02C-024
        // audit: COUSR02C-026
        // audit: COUSR02C-029
        let renamed = update(
            &state,
            &admin,
            "USER0001",
            request("LARRY", "THOMAS", None, "u"),
        )
        .await
        .map_err(|error| error.to_string())?;
        assert_eq!(renamed, "User USER0001 has been updated ...");
        let after = stored(&store, "USER0001").await?;
        assert_eq!(
            (&after.fname, &after.usr_type, &after.filler),
            (
                &PicX::from_text("LARRY")?,
                &PicX::from_array([b'u']),
                &before.filler
            )
        );
        assert!(
            after.password_hash == before.password_hash,
            "an omitted password is kept"
        );

        // audit: COUSR02C-027
        let body = request("LARRY", "THOMAS", Some(NEW_PASSWORD), "u");
        let message = update(&state, &admin, "USER0001", body)
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(message, "User USER0001 has been updated ...");
        let after = stored(&store, "USER0001").await?;
        assert!(password::verify(
            password_bytes(NEW_PASSWORD)?,
            &after.password_hash
        )?);
        assert!(!password::verify(
            password_bytes(STORED_PASSWORD)?,
            &after.password_hash
        )?);
        assert_eq!(
            (&after.fname, &after.filler),
            (&PicX::from_text("LARRY")?, &before.filler)
        );

        // audit: COUSR02C-030
        let spaced = update(&state, &admin, "AB CD", request("ANN", "LEE", None, "U"))
            .await
            .map_err(|error| error.to_string())?;
        assert_eq!(spaced, "User AB has been updated ...");

        // audit: COUSR02C-025
        // No row has the key: the store effect alone is asserted (halt candidate
        // X-COUSR02C-H1), and no row appears.
        let missing = update(&state, &admin, "NOBODY01", request("A", "B", None, "U")).await;
        assert!(missing.is_err());
        let mut probe = store.clone();
        let key = PicX::from_text("NOBODY01")?;
        assert!(
            UserRepository::find_by_id(&mut probe, &key)
                .await?
                .is_none()
        );
        Ok(())
    }

    #[tokio::test]
    async fn store_failures_answer_with_the_programs_messages_and_trace_no_value() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        seed(&mut store, &["USER0001"]).await?;
        UserRepository::insert(
            &mut store,
            &entry("BADHASH1", "not-a-phc-string".to_owned())?,
        )
        .await?;
        let state = app_state(store.clone());
        let admin = admin()?;
        let traces = trace_capture::capture();
        let before = stored(&store, "USER0001").await?;

        // audit: COUSR02C-032
        // A first name codec B cannot store fails the rewrite, which rolls back.
        let body = request("\u{c9}MILE", "THOMAS", Some(NEW_PASSWORD), "U");
        let outcome = update(&state, &admin, "USER0001", body).await;
        assert_eq!(
            shown(&outcome),
            Some(("Unable to Update User...", "first_name", 500))
        );
        assert_eq!(stored(&store, "USER0001").await?, before);
        assert!(traces.contains("UPDATE-USER-SEC-FILE"));

        // A stored hash that cannot be verified is the read's failure (X-COUSR02C-4).
        let body = request("X", "Y", Some(NEW_PASSWORD), "U");
        let outcome = update(&state, &admin, "BADHASH1", body).await;
        assert_eq!(shown(&outcome), Some(LOOKUP_FAILED));
        assert_eq!(
            stored(&store, "BADHASH1").await?.fname,
            PicX::from_text("LAWRENCE")?
        );
        assert!(traces.contains("stored password hash could not be verified"));

        store.pool().close().await;
        // audit: COUSR02C-016
        // audit: COUSR03C-014
        assert_eq!(
            shown(&fetch(&state, &admin, "USER0001").await),
            Some(LOOKUP_FAILED)
        );
        assert!(traces.contains("READ-USER-SEC-FILE"));
        let outcome = update(&state, &admin, "USER0001", request("A", "B", None, "U")).await;
        assert_eq!(
            outcome,
            Err(ServiceError::Store(PersistenceError::Unavailable))
        );

        for value in [
            STORED_PASSWORD,
            NEW_PASSWORD,
            "$argon2id$",
            "not-a-phc-string",
        ] {
            assert!(!traces.contains(value), "the trace holds {value}");
        }
        Ok(())
    }

    /// The change test's failures are logged once, after the task is awaited, with their class
    /// and never a value, and keep their answers: the unverifiable stored hash its `Unable to
    /// lookup User...` (`X-COUSR02C-4`), the hash and task failures their 500.
    #[tokio::test]
    async fn change_test_failures_are_logged_once_and_keep_their_answers() -> TestResult {
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        let cancelled = task.await.err().ok_or("an aborted task completed")?;
        let unverified = rules::read(UserRead::Error)
            .err()
            .ok_or("a failed read gave a view")?;
        let unmodified = rules::edit_user_id("")
            .err()
            .ok_or("a blank ID passed the edit")?;
        let row = entry("USER0001", "$argon2id$v=19$stand-in".to_owned())?;
        let traces = trace_capture::capture();
        let cases = [
            (
                Err(cancelled),
                Err(ServiceError::Internal),
                Some(("ERROR", r#""panicked":false"#)),
            ),
            (
                Ok(Err(Unchanged::NotHashed(PasswordError::Hash))),
                Err(ServiceError::Internal),
                Some(("ERROR", "password hashing failed")),
            ),
            (
                Ok(Err(Unchanged::Refused(unverified))),
                Err(ServiceError::Rule(unverified)),
                Some(("WARN", "stored password hash could not be verified")),
            ),
            (
                Ok(Err(Unchanged::Refused(unmodified))),
                Err(ServiceError::Rule(unmodified)),
                None,
            ),
            (Ok(Ok(row.clone())), Ok(row), None),
        ];
        for (joined, expected, logged) in cases {
            traces.clear();
            let answer = answered(joined);
            assert!(answer == expected, "{logged:?}");
            let lines = traces.lines();
            match logged {
                None => assert!(lines.is_empty(), "{}", traces.text()),
                Some((level, text)) => {
                    let [line] = lines.as_slice() else {
                        return Err(format!("{text}: {} lines", lines.len()).into());
                    };
                    let level = format!(r#""level":"{level}""#);
                    for part in [
                        level.as_str(),
                        r#""program":"COUSR02C""#,
                        r#""paragraph":"UPDATE-USER-INFO""#,
                        text,
                    ] {
                        assert!(line.contains(part), "{part} missing from {line}");
                    }
                }
            }
        }
        for value in ["USER0001", "$argon2id$", STORED_PASSWORD, NEW_PASSWORD] {
            assert!(!traces.contains(value), "the trace holds {value}");
        }
        Ok(())
    }

    #[test]
    fn store_answers_become_the_domains_values() {
        // audit: COUSR02C-031
        assert_eq!(rewrite_result(Ok(true)), RewriteResult::Rewritten);
        assert_eq!(rewrite_result(Ok(false)), RewriteResult::NotFound);
        assert_eq!(
            rewrite_result(Err(PersistenceError::Unavailable)),
            RewriteResult::Error
        );

        // The re-read's failures are the read's own, not a parity result (X-COUSR02C-H1).
        assert_eq!(
            reread_refusal(UserRead::NotFound),
            ServiceError::Rule(UserMaintFailure::not_found())
        );
        let failed: Result<(), _> = Err(reread_refusal(UserRead::Error));
        assert_eq!(shown(&failed), Some(LOOKUP_FAILED));
    }
}
