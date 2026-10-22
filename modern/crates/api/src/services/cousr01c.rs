//! COUSR01C, user add: the service of endpoint 17, `POST /api/v1/users`, admin only (transaction
//! `CU01`, map `COUSR1A`; AAP 0.3.4.1).
//!
//! It ports the store side of ENTER: the record `PROCESS-ENTER-KEY` builds and the
//! `WRITE-USER-SEC-FILE` that stores it (`app/cbl/COUSR01C.cbl:153-160,238-273`). The edits are
//! `domain::online::cousr01c`'s; the handler only extracts, calls [`add`] and maps the result
//! (Thin Request Handlers).
//!
//! # Flow
//!
//! 1. The five request members, moved out of the [`UserAddRequest`] by value, are lent to
//!    [`cousr01c::edit`] as its [`UserAddInput`]. The first empty field in `EVALUATE` order is
//!    the answer, a 400 naming the field (COUSR01C-010 to COUSR01C-014); a value no `PIC X(n)`
//!    can hold is rejected first (X-COUSR01C-3).
//! 2. The record of the [`NewUser`](cousr01c::NewUser) is `SEC-USER-DATA` with every field as
//!    typed (COUSR01C-015, COUSR01C-016). Its 8 password bytes are hashed with argon2id by
//!    [`password::hash`], exactly as typed and never upper-cased (COUSR01C-018, E25): sign-on
//!    upper-cases what it is given, so a password with a lower-case letter never verifies, as in
//!    the COBOL (COUSR01C-019, E26, preserved).
//! 3. The row is inserted through [`UserRepository::insert`] on the pool, one autocommitted
//!    statement, as the CICS `WRITE` is one request. The outcome follows the `EVALUATE
//!    WS-RESP-CD` of `WRITE-USER-SEC-FILE`:
//!
//! | Store outcome | COBOL `RESP` | Answer | Rule |
//! |---|---|---|---|
//! | row inserted | `NORMAL` | 200, [`cousr01c::added_message`]: `User <id> has been added ...` | COUSR01C-021 |
//! | [`PersistenceError::is_duplicate`], the `users_pkey` violation | `DUPKEY`, `DUPREC` | [`cousr01c::duplicate_user_id`]: `User ID already exist...` on `user_id`, 400 by its class | COUSR01C-023 |
//! | any other store error, or a hash that could not be computed | `WHEN OTHER` | [`cousr01c::write_failure`]: `Unable to Add User...` on `first_name`, 500 | COUSR01C-024 |
//!
//! Every outcome is the program's own answer, a [`ServiceError::Rule`]; none is the unaudited
//! [`ServiceError::Store`] or [`ServiceError::Internal`].
//!
//! # Filler
//!
//! `SEC-USR-FILLER PIC X(23)` is never assigned by COUSR01C, so the bytes the COBOL writes are
//! initial storage (halt candidate X-COUSR01C-H1). The row carries the 23 spaces of
//! `NewUser::into_record`, which claim no parity (COUSR01C-017).
//!
//! # Identity
//!
//! The caller is the verified token's [`AdminUser`], whose gate has already refused a missing
//! token with 401 and a non-`A` user type with 403 (COUSR01C-001, COUSR01C-002). The body's
//! `user_id` and `user_type` only describe the row being added; nothing here changes who acts
//! (COUSR01C-026, AAP 0.5.1).
//!
//! # Redaction
//!
//! The password lives only in the request's `password` string and the record's
//! [`Password`](domain::Password), whose `Debug` is redacted; the hash lives only in the
//! [`UserEntry`], whose `Debug` prints `<redacted>` in its place. Neither is logged, traced,
//! formatted or returned, the success message names only the user ID, and no failure carries a
//! request value, an argon2 error or a store detail (Cardholder Data Redaction, COUSR01C-018).
//! A `WHEN OTHER` failure is logged once before it is answered: a store error by its `Display`,
//! an SQLSTATE and a constraint or column at most; a hash failure by the fixed class text of
//! [`PasswordError`]; a hashing task that did not complete by whether it panicked, never its
//! payload. No event names the user, the password or the hash.

use domain::online::cousr01c::{self, UserAddFailure, UserAddInput};
use domain::password::{self, PasswordError};
use domain::records::UserEntry;
use persistence::{PersistenceError, UserRepository};
use tokio::task::JoinError;

use crate::app::AppState;
use crate::auth::extract::AdminUser;
use crate::dto::cousr01c::UserAddRequest;
use crate::error::ServiceError;

/// The program the tracing events of a failed add name.
const PROGRAM: &str = "COUSR01C";

/// The paragraph that builds the record, whose password the replacement hashes (COUSR01C-018).
const ENTER_PARAGRAPH: &str = "PROCESS-ENTER-KEY";

/// The paragraph of the insert (COUSR01C-024).
const WRITE_PARAGRAPH: &str = "WRITE-USER-SEC-FILE";

/// ENTER on `COUSR1A`: edits the five fields and adds the user, answering with the message of
/// the stored row (COUSR01C-016, COUSR01C-021).
///
/// `_admin` is the gate's proof: the handler can only call this once [`AdminUser`] has accepted
/// the token. The new user's fields are request input and grant the caller nothing
/// (COUSR01C-026).
///
/// # Errors
///
/// A [`ServiceError::Rule`] carrying the program's [`UserAddFailure`]:
///
/// - the first empty field, or a value its `PIC X(n)` cannot hold, from
///   [`cousr01c::edit`] (COUSR01C-010 to COUSR01C-014);
/// - [`cousr01c::duplicate_user_id`] when a stored row has the same 8 user ID bytes
///   (COUSR01C-023);
/// - [`cousr01c::write_failure`] when the hash cannot be computed or the insert fails for any
///   other reason (COUSR01C-024).
pub(crate) async fn add(
    state: &AppState,
    _admin: &AdminUser,
    request: UserAddRequest,
) -> Result<String, ServiceError<UserAddFailure>> {
    // Cloning copies the pool handle only; repository calls take the executor by `&mut`.
    let mut store = state.store.clone();
    insert_user(&mut store, request).await
}

/// The add of [`add`] on any `users` executor: the pool in service, a test double in the unit
/// tests below.
async fn insert_user<R: UserRepository>(
    store: &mut R,
    request: UserAddRequest,
) -> Result<String, ServiceError<UserAddFailure>> {
    let UserAddRequest {
        first_name,
        last_name,
        user_id,
        password,
        user_type,
    } = request;
    let input = UserAddInput {
        first_name: &first_name,
        last_name: &last_name,
        user_id: &user_id,
        password: &password,
        user_type: &user_type,
    };
    let record = cousr01c::edit(&input)
        .map_err(ServiceError::Rule)?
        .into_record();

    // argon2id is deliberately CPU- and memory-hard, so it runs on the blocking pool rather than
    // stalling a runtime worker. A hash that fails, or a hashing task that cannot finish, is the
    // `WHEN OTHER` answer `Unable to Add User...` that COUSR01C-024 assigns to it, not the
    // unaudited `ServiceError::Internal`; `hashed` logs its class here, after the await, and the
    // answer never echoes it.
    let joined = tokio::task::spawn_blocking(move || {
        let hashed = password::hash(record.pwd.expose());
        (record, hashed)
    })
    .await;
    let (record, password_hash) = hashed(joined)?;

    // `record.pwd` stops here: the row holds the hash in its place (E25).
    let entry = UserEntry {
        id: record.id,
        fname: record.fname,
        lname: record.lname,
        password_hash,
        usr_type: record.usr_type,
        filler: record.filler,
    };
    UserRepository::insert(store, &entry)
        .await
        .map(|()| cousr01c::added_message(&entry.id))
        .map_err(|error| ServiceError::Rule(write_answer(&error)))
}

/// The record and its password hash from the hashing task (COUSR01C-018), or `Unable to Add
/// User...` when the task did not complete or argon2id could not hash (COUSR01C-024).
///
/// Either failure is logged first: the task's as whether it panicked, never its payload, which
/// the `JoinError`'s `Display` could carry; the hash's as the fixed class text of
/// [`PasswordError`]. Neither event names the user, the password or the hash.
fn hashed<T>(
    joined: Result<(T, Result<String, PasswordError>), JoinError>,
) -> Result<(T, String), ServiceError<UserAddFailure>> {
    match joined {
        Ok((record, Ok(password_hash))) => Ok((record, password_hash)),
        Ok((_, Err(error))) => {
            tracing::warn!(
                program = PROGRAM,
                paragraph = ENTER_PARAGRAPH,
                error = %error,
                "password not hashed; answering Unable to Add User"
            );
            Err(ServiceError::Rule(cousr01c::write_failure()))
        }
        Err(error) => {
            tracing::warn!(
                program = PROGRAM,
                paragraph = ENTER_PARAGRAPH,
                panicked = error.is_panic(),
                "password hashing task did not complete; answering Unable to Add User"
            );
            Err(ServiceError::Rule(cousr01c::write_failure()))
        }
    }
}

/// The program's answer to a failed insert: `DUPKEY`/`DUPREC` for the `users_pkey` violation,
/// the only unique constraint of `users` (COUSR01C-023), and `WHEN OTHER` for anything else
/// (COUSR01C-024).
///
/// A `WHEN OTHER` failure is logged first with its store error, whose `Display` names an SQLSTATE
/// and a constraint or column at most. A duplicate is the user's own input, a taken ID, and is
/// answered without an event.
fn write_answer(error: &PersistenceError) -> UserAddFailure {
    if error.is_duplicate() {
        cousr01c::duplicate_user_id()
    } else {
        tracing::warn!(
            program = PROGRAM,
            paragraph = WRITE_PARAGRAPH,
            error = %error,
            "USRSEC write failed; answering Unable to Add User"
        );
        cousr01c::write_failure()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::future::{Future, ready};

    use domain::batch::loads::HashedUser;
    use domain::copybook::PicX;
    use domain::online::cousr01c::{self, UserAddFailure};
    use domain::password::{self, PasswordError};
    use domain::records::{UserEntry, UserListEntry};
    use persistence::{DecodeKind, PersistenceError, PgStore, UserRepository};
    use test_support::trace_capture;

    use super::{hashed, insert_user};
    use crate::dto::cousr01c::UserAddRequest;
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn Error>>;

    /// A fake password the tests submit, with a lower-case letter and shorter than its 8 bytes.
    const PASSWORD: &str = "fakePw";

    /// [`PASSWORD`] as `SEC-USR-PWD` holds it: as typed, padded with spaces to 8 bytes.
    const STORED_PASSWORD: &[u8; 8] = b"fakePw  ";

    /// [`STORED_PASSWORD`] upper-cased, as sign-on would compare it (E26).
    const SIGNON_PASSWORD: &[u8; 8] = b"FAKEPW  ";

    /// A `users` executor that records every insert and answers it with `answer`. It serves no
    /// other access: each one reports the store unavailable, so a test reaching one fails.
    struct InsertOnly {
        answer: Result<(), PersistenceError>,
        inserted: Vec<UserEntry>,
    }

    impl InsertOnly {
        fn answering(answer: Result<(), PersistenceError>) -> Self {
            Self {
                answer,
                inserted: Vec::new(),
            }
        }
    }

    impl UserRepository for InsertOnly {
        fn find_by_id(
            &mut self,
            _id: &PicX<8>,
        ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn lock_by_id(
            &mut self,
            _id: &PicX<8>,
        ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
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
            entry: &UserEntry,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            self.inserted.push(entry.clone());
            ready(self.answer.clone())
        }

        fn update(
            &mut self,
            _entry: &UserEntry,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn delete(
            &mut self,
            _id: &PicX<8>,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }

        fn replace_all(
            &mut self,
            _users: &[HashedUser],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            ready(Err(PersistenceError::Unavailable))
        }
    }

    /// A request with every member filled and the fields typed with leading spaces and
    /// lower-case letters, which the add keeps.
    fn request() -> UserAddRequest {
        UserAddRequest {
            first_name: " jane".to_owned(),
            last_name: "o'Neil".to_owned(),
            user_id: "NEWUSR01".to_owned(),
            password: PASSWORD.to_owned(),
            user_type: "a".to_owned(),
        }
    }

    /// The program's failure inside `outcome`, or why there is none.
    fn rule(
        outcome: Result<String, ServiceError<UserAddFailure>>,
    ) -> Result<UserAddFailure, String> {
        match outcome {
            Err(ServiceError::Rule(failure)) => Ok(failure),
            Err(other) => Err(format!("not the program's answer: {other}")),
            Ok(message) => Err(format!("added: {message}")),
        }
    }

    // audit: COUSR01C-015
    // audit: COUSR01C-016
    // audit: COUSR01C-017
    // audit: COUSR01C-018
    // audit: COUSR01C-019
    // audit: COUSR01C-021
    #[tokio::test]
    async fn a_valid_request_stores_every_field_as_typed_with_the_hash_of_its_password()
    -> TestResult {
        let mut store = InsertOnly::answering(Ok(()));
        let message = insert_user(&mut store, request()).await?;
        assert_eq!(message, "User NEWUSR01 has been added ...");

        let [entry] = store.inserted.as_slice() else {
            return Err(format!("{} inserts", store.inserted.len()).into());
        };
        assert_eq!(entry.id, PicX::from_array(*b"NEWUSR01"));
        assert_eq!(entry.fname, PicX::from_text(" jane")?);
        assert_eq!(entry.lname, PicX::from_text("o'Neil")?);
        assert_eq!(entry.usr_type, PicX::from_array(*b"a"));
        assert!(entry.filler.is_spaces());

        assert!(entry.password_hash.starts_with("$argon2id$"));
        assert!(!entry.password_hash.contains(PASSWORD));
        assert_eq!(
            password::verify(STORED_PASSWORD, &entry.password_hash),
            Ok(true)
        );
        // Hashed as typed, never upper-cased: sign-on's upper-cased compare fails (E26).
        assert_eq!(
            password::verify(SIGNON_PASSWORD, &entry.password_hash),
            Ok(false)
        );
        Ok(())
    }

    // audit: COUSR01C-010
    // audit: COUSR01C-011
    // audit: COUSR01C-012
    // audit: COUSR01C-013
    // audit: COUSR01C-014
    #[tokio::test]
    async fn the_first_empty_field_answers_and_nothing_is_written() -> TestResult {
        type Blank = fn(&mut UserAddRequest);
        let cases: [(Blank, &str, &str); 5] = [
            (
                |r| r.first_name.clear(),
                "First Name can NOT be empty...",
                "first_name",
            ),
            (
                |r| r.last_name = "   ".to_owned(),
                "Last Name can NOT be empty...",
                "last_name",
            ),
            (
                |r| r.user_id.clear(),
                "User ID can NOT be empty...",
                "user_id",
            ),
            (
                |r| r.password = " ".to_owned(),
                "Password can NOT be empty...",
                "password",
            ),
            (
                |r| r.user_type.clear(),
                "User Type can NOT be empty...",
                "user_type",
            ),
        ];
        for (position, &(blank, message, field)) in cases.iter().enumerate() {
            // The field alone, then with every later field empty too: the earliest one answers.
            let mut alone = request();
            blank(&mut alone);
            let mut with_later = request();
            for &(later, _, _) in cases.iter().skip(position) {
                later(&mut with_later);
            }
            for case in [alone, with_later] {
                let mut store = InsertOnly::answering(Ok(()));
                let failure = rule(insert_user(&mut store, case).await)?;
                assert_eq!(
                    (failure.message(), failure.field().name()),
                    (message, field)
                );
                assert_eq!(failure.class().http_status(), 400, "{message}");
                assert!(store.inserted.is_empty(), "{message}");
            }
        }
        Ok(())
    }

    // audit: COUSR01C-023
    #[tokio::test]
    async fn a_taken_user_id_is_the_duplicate_answer() -> TestResult {
        let traces = trace_capture::capture();
        for constraint in [Some("users_pkey".to_owned()), None] {
            let mut store = InsertOnly::answering(Err(PersistenceError::Duplicate { constraint }));
            let failure = rule(insert_user(&mut store, request()).await)?;
            assert_eq!(failure, cousr01c::duplicate_user_id());
            assert_eq!(
                (failure.message(), failure.field().name()),
                ("User ID already exist...", "user_id")
            );
            assert_eq!(store.inserted.len(), 1);
        }
        // A taken ID is the user's input, not an operational failure: nothing is logged.
        assert!(traces.text().is_empty(), "{}", traces.text());
        Ok(())
    }

    // audit: COUSR01C-024
    #[tokio::test]
    async fn any_other_store_error_is_unable_to_add_user() -> TestResult {
        let errors = [
            PersistenceError::Unavailable,
            PersistenceError::NotFound,
            PersistenceError::Database {
                sqlstate: Some("57014".to_owned()),
                constraint: None,
            },
            PersistenceError::Check {
                constraint: Some("users_check".to_owned()),
            },
            PersistenceError::Restrict { constraint: None },
            PersistenceError::decode("usr_id", DecodeKind::InvalidByte),
        ];
        let traces = trace_capture::capture();
        for error in errors {
            traces.clear();
            let shown = format!("{error:?}");
            let cause = error.to_string();
            let mut store = InsertOnly::answering(Err(error));
            let failure = rule(insert_user(&mut store, request()).await)?;
            assert_eq!(failure, cousr01c::write_failure(), "{shown}");
            assert_eq!(
                (failure.message(), failure.field().name()),
                ("Unable to Add User...", "first_name"),
                "{shown}"
            );
            assert_eq!(failure.class().http_status(), 500, "{shown}");

            // The cause is logged once, before the answer, and the event names no request value.
            let lines = traces.lines();
            let [line] = lines.as_slice() else {
                return Err(format!("{shown}: {} lines", lines.len()).into());
            };
            for expected in [
                r#""level":"WARN""#,
                r#""program":"COUSR01C""#,
                r#""paragraph":"WRITE-USER-SEC-FILE""#,
                cause.as_str(),
            ] {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
            for value in [PASSWORD, "NEWUSR01", "$argon2id$"] {
                assert!(!line.contains(value), "{shown}: the event holds {value}");
            }
        }
        Ok(())
    }

    // audit: COUSR01C-024
    #[tokio::test]
    async fn a_hash_or_task_failure_is_logged_and_answers_unable_to_add_user() -> TestResult {
        let task = tokio::spawn(std::future::pending::<()>());
        task.abort();
        let cancelled = task.await.err().ok_or("an aborted task completed")?;
        let traces = trace_capture::capture();
        let cases = [
            (
                Err(cancelled),
                [
                    "password hashing task did not complete",
                    r#""panicked":false"#,
                ],
            ),
            (
                Ok(((), Err(PasswordError::Hash))),
                ["password not hashed", "password hashing failed"],
            ),
        ];
        for (joined, texts) in cases {
            traces.clear();
            assert_eq!(
                hashed(joined),
                Err(ServiceError::Rule(cousr01c::write_failure()))
            );
            let lines = traces.lines();
            let [line] = lines.as_slice() else {
                return Err(format!("{texts:?}: {} lines", lines.len()).into());
            };
            for expected in [
                r#""level":"WARN""#,
                r#""program":"COUSR01C""#,
                r#""paragraph":"PROCESS-ENTER-KEY""#,
            ]
            .into_iter()
            .chain(texts)
            {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
        }

        traces.clear();
        let computed = hashed(Ok(((), Ok("$argon2id$v=19$stand-in".to_owned()))));
        assert_eq!(computed, Ok(((), "$argon2id$v=19$stand-in".to_owned())));
        assert!(traces.text().is_empty(), "{}", traces.text());
        Ok(())
    }

    // audit: COUSR01C-018
    #[tokio::test]
    async fn no_answer_or_stored_value_shows_the_password_or_its_hash() -> TestResult {
        let mut store = InsertOnly::answering(Ok(()));
        let message = insert_user(&mut store, request()).await?;
        assert!(!message.contains(PASSWORD));

        let [entry] = store.inserted.as_slice() else {
            return Err(format!("{} inserts", store.inserted.len()).into());
        };
        let shown = format!("{entry:?}");
        assert!(!shown.contains(PASSWORD), "{shown}");
        assert!(!shown.contains(&entry.password_hash), "{shown}");

        let answers = [
            Err(PersistenceError::Duplicate { constraint: None }),
            Err(PersistenceError::Unavailable),
        ];
        for answer in answers {
            let mut store = InsertOnly::answering(answer);
            let error = match insert_user(&mut store, request()).await {
                Ok(message) => return Err(format!("added: {message}").into()),
                Err(error) => error,
            };
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains(PASSWORD), "{shown}");
        }
        Ok(())
    }

    // audit: COUSR01C-016
    // audit: COUSR01C-023
    #[tokio::test]
    async fn the_store_keeps_the_added_user_and_refuses_its_id_twice() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let key = PicX::from_array(*b"NEWUSR01");

        let message = insert_user(&mut store, request()).await?;
        assert_eq!(message, "User NEWUSR01 has been added ...");
        let stored = UserRepository::find_by_id(&mut store, &key)
            .await?
            .ok_or("the added user is not stored")?;
        assert_eq!(stored.fname, PicX::from_text(" jane")?);
        assert_eq!(stored.usr_type, PicX::from_array(*b"a"));
        assert!(stored.filler.is_spaces());
        assert_eq!(
            password::verify(STORED_PASSWORD, &stored.password_hash),
            Ok(true)
        );

        // The same 8 bytes again: the existing row stays as it was.
        let again = UserAddRequest {
            first_name: "OTHER".to_owned(),
            ..request()
        };
        let failure = rule(insert_user(&mut store, again).await)?;
        assert_eq!(failure, cousr01c::duplicate_user_id());
        let kept = UserRepository::find_by_id(&mut store, &key)
            .await?
            .ok_or("the added user is gone")?;
        assert!(kept == stored, "the stored row changed");

        // Keys compare byte for byte, so the lower-case ID is another user.
        let lower = UserAddRequest {
            user_id: "newusr01".to_owned(),
            ..request()
        };
        let message = insert_user(&mut store, lower).await?;
        assert_eq!(message, "User newusr01 has been added ...");
        Ok(())
    }
}
