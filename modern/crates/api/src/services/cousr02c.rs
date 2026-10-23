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
//! `UPDATE-USER-INFO` and `UPDATE-USER-SEC-FILE`. The COBOL runs its change test against the
//! row its `READ … UPDATE` holds. Here the change test's argon2id work runs while no
//! transaction is open, against a basis read of the row, and the locked re-read then confirms
//! that the row is still the one it was run against:
//!
//! 1. The empty-field edits, in the program's order, before any read (COUSR02C-018 to
//!    COUSR02C-023).
//! 2. An unlocked read of the path ID, the basis of the change test. No row is the domain's
//!    answer to the re-read, and nothing is written (COUSR02C-025, halt candidate
//!    `X-COUSR02C-H1`). A failed read is recovered by step 4's locked re-read, in a `PgTx` of
//!    its own that rolls back as soon as the row is read, so that a store failure keeps the
//!    answer of the COBOL's order, the locked read first: `ServiceError::Store` when the
//!    transaction cannot begin, the domain's answer when the re-read finds no row or fails too
//!    (COUSR02C-025). The row it read is then the basis.
//! 3. The change test against the basis, in no transaction: names and type byte for byte, a
//!    supplied password verified against the stored argon2id hash, and the new hash computed
//!    when the password changed; nothing changed is `Please modify to update ...`
//!    (COUSR02C-026 to COUSR02C-028). A refusal writes nothing, so it answers as of the basis
//!    read and opens no transaction.
//! 4. In one `PgTx`, the re-read `FOR UPDATE` of the path ID, which holds the row until the
//!    rewrite (COUSR02C-024). No row or a failed read rolls back with the domain's answer
//!    (COUSR02C-025). A row that differs from the basis, rewritten and committed by another
//!    request in between, rolls back and becomes the basis of the next attempt, from step 3.
//!    Every such row is another request's commit, so each round of contention ends with one
//!    contender's rewrite: updates of one user that arrive together all succeed, one after
//!    another, and the last committed values stand, as when the COBOL's `READ … UPDATE`
//!    serializes them. Only an update whose row is still changing at a lock after
//!    [`CONFIRM_DEADLINE`] gives up: it rolls back and answers as a failed rewrite, `Unable to
//!    Update User...` (COUSR02C-032), with an `ERROR` event. The COBOL cannot reach that case,
//!    because its `READ … UPDATE` holds the record from the change test to the `REWRITE`, so it
//!    adds no message. No lock, in the database or in the process, is held across a change
//!    test: the locked re-read alone decides, so the rule holds between processes too. So every
//!    rewrite is decided against the row its own transaction holds, and every refusal against
//!    the row the last read or lock read.
//! 5. The rewrite of the whole row: ID and `FILLER` as read, the request's names and type, and
//!    a new hash only when the password changed, else the stored one (COUSR02C-029, `E27`).
//! 6. Commit and `User <id> has been updated ...` (COUSR02C-029, COUSR02C-030); a rewrite that
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
//! Tokio's blocking pool, within the API's password-work budget (`crate::app::PasswordWork`),
//! rather than on the thread that drives other requests; and they run only while no
//! transaction is open, on every path, so that no pooled connection or row lock is held while
//! they compute or wait for a worker. Their failures are logged once the task is awaited, on
//! the request's own thread and span: a stored hash that cannot be verified, a new password that
//! cannot be hashed, by the fixed class text of `PasswordError`, and a task that did not
//! complete, by whether it panicked, never its payload. A test the saturated budget refuses,
//! because no place came free within its wait, is `ServiceError::Saturated`, a 503 with
//! `Retry-After` that writes nothing, and is logged by the budget. No event names the password,
//! the hash or the user.
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

use std::time::{Duration, Instant};

use domain::copybook::PicX;
use domain::online::cousr02c::{
    self as rules, FailureClass, RewriteResult, StoredUserView, UserMaintFailure, UserRead,
    UserUpdateInput, UserView, ValidatedUpdate,
};
use domain::password::{self, PasswordError};
use domain::records::UserEntry;
use persistence::{PersistenceError, PgTx, UserRepository};
use tracing::{Level, Metadata, field};

use crate::app::{AppState, EventSite, PasswordWork, PasswordWorkError, emit, event_metadata};
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

/// How long endpoint 19 keeps confirming its change tests, each run outside any transaction and
/// each confirmed by one transaction's locked re-read, counted from the start of the update
/// (see "Endpoint 19" above). Once a lock after it still finds the row changed, the update
/// answers `Unable to Update User...` rather than test again.
///
/// A basis goes stale only when another request has committed a rewrite of the same user
/// between the read and the lock, so each round of contention ends with a commit and updates
/// that arrive together confirm within as many rounds as there are of them. Four seconds cover
/// some thirty updates of one user that each set a new password, a round costing one change
/// test of two argon2id runs in a release build, and, with one last change test's
/// [`PASSWORD_WAIT`](crate::app::PASSWORD_WAIT), keep the update within the 8 s that
/// `carddemo-api` gives the requests in flight at a stop.
const CONFIRM_DEADLINE: Duration = Duration::from_secs(4);

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
///   to update ...` (400), `User ID NOT found...` (404) from the read, the re-read or the
///   rewrite, and `Unable to lookup User...` or `Unable to Update User...` (500) when the read,
///   the re-read, the stored hash's verification or the rewrite fails, or, for `Unable to
///   Update User...`, when the row still changed before a lock after [`CONFIRM_DEADLINE`].
/// - `ServiceError::Store` when a transaction cannot begin, commit or roll back.
/// - `ServiceError::Internal` when the new password cannot be hashed or the blocking task that
///   runs argon2id does not complete.
/// - `ServiceError::Saturated` when the password-work budget finds no place for a change test
///   within its wait; nothing is written.
pub(crate) async fn update(
    state: &AppState,
    _admin: &AdminUser,
    user_id: &str,
    request: UserUpdateRequest,
) -> Result<String, Refusal> {
    updated(state, user_id, request, CONFIRM_DEADLINE).await
}

/// The update of [`update`], which keeps confirming its change tests for `deadline` from its
/// start; a test passes a shorter one than [`CONFIRM_DEADLINE`] to reach the end of it quickly.
///
/// # Errors
///
/// As [`update`], with `deadline` in place of [`CONFIRM_DEADLINE`].
async fn updated(
    state: &AppState,
    user_id: &str,
    request: UserUpdateRequest,
    deadline: Duration,
) -> Result<String, Refusal> {
    let started = Instant::now();
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

    let mut basis = basis_read(state, update.user_id()).await?;
    let work = &state.password_work;
    let mut attempts: usize = 1;
    loop {
        // No transaction is open: the change test computes and waits for a worker holding no
        // connection and no lock, and a refusal returns from here and writes nothing.
        let entry = changed_entry(work, update.clone(), Held::from(basis.clone())).await?;
        let mut tx = state.store.begin().await?;
        let locked = match looked_up(UserRepository::lock_by_id(&mut tx, update.user_id()).await) {
            Ok(locked) => locked,
            Err(absent) => return abandon(tx, reread_refusal(absent)).await,
        };
        if locked == basis {
            return rewritten(tx, &entry).await;
        }
        // Another request committed a rewrite since the basis was read: test against that row,
        // until the deadline.
        if started.elapsed() >= deadline {
            return abandon(tx, kept_changing(attempts, deadline)).await;
        }
        tx.rollback().await?;
        basis = locked;
        attempts = attempts.saturating_add(1);
    }
}

/// The first basis of the change test: the unlocked read of `id` on a pooled connection, or,
/// when that read fails, the locked re-read of a transaction that rolls back as soon as it has
/// read the row, so that no transaction stays open into the change test (COUSR02C-024,
/// COUSR02C-025).
///
/// The recovery keeps every answer of the COBOL's own order, which reads `UPDATE` first:
/// `ServiceError::Store` when the transaction cannot begin or roll back, `User ID NOT
/// found...` when the re-read finds no row and `Unable to lookup User...` when it fails too.
///
/// # Errors
///
/// As above; and `User ID NOT found...` when the unlocked read finds no row.
async fn basis_read(state: &AppState, id: &PicX<8>) -> Result<UserEntry, Refusal> {
    let mut store = state.store.clone();
    match looked_up(UserRepository::find_by_id(&mut store, id).await) {
        Ok(basis) => Ok(basis),
        Err(UserRead::Error) => {
            let mut tx = state.store.begin().await?;
            match looked_up(UserRepository::lock_by_id(&mut tx, id).await) {
                Ok(locked) => {
                    tx.rollback().await?;
                    Ok(locked)
                }
                Err(absent) => abandon(tx, reread_refusal(absent)).await,
            }
        }
        Err(absent) => Err(reread_refusal(absent)),
    }
}

/// The answer once the row has still changed before a lock after the update's `deadline`, the
/// bound, when its change tests have run `attempts` times: the rewrite's own failure, `Unable
/// to Update User...` on `first_name` (COUSR02C-032), with one `ERROR` event, since the answer
/// is a 500, that names the program, the paragraph, the bound and the attempts, and no value of
/// the request or the row.
///
/// The COBOL cannot reach this case: its `READ … UPDATE` holds the record from the change test
/// to the `REWRITE`, so no other task rewrites it in between. The replacement runs the change
/// test with no lock held, so another request can rewrite the row each time; the update then
/// gives up without rewriting, as a rewrite that failed, and adds no message of its own.
fn kept_changing(attempts: usize, deadline: Duration) -> Refusal {
    static SITE: EventSite<5> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "kept_changing",
        module_path!(),
        Level::ERROR,
        &["message", "program", "paragraph", "deadline", "attempts"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"user row kept changing before its lock; answering Unable to Update User"),
            Some(&PROGRAM),
            Some(&REWRITE_PARAGRAPH),
            Some(&field::debug(deadline)),
            Some(&attempts),
        ],
    );
    ServiceError::Rule(UserMaintFailure::update_failed())
}

/// The basis of a change test, the row read before it, split as the change test and the
/// rewrite take it: the stored view the domain compares with, and the hash and `FILLER` that
/// the view leaves out and the rewrite keeps.
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

/// The domain's answer to a read or re-read that found no row or failed: `User ID NOT
/// found...` or `Unable to lookup User...` (COUSR02C-025).
///
/// `rules::reread` returns a row only for `UserRead::Found`, which `looked_up` never yields as
/// an absence, so that arm is the 500 of an inconsistent answer rather than a success.
fn reread_refusal(absent: UserRead<'_>) -> Refusal {
    rules::reread(absent).map_or_else(ServiceError::Rule, |_| ServiceError::Internal)
}

/// The rewrite of `entry` and its outcome, inside the transaction that holds its row
/// (COUSR02C-029 to COUSR02C-032). Commits only on the domain's success.
async fn rewritten(mut tx: PgTx<'static>, entry: &UserEntry) -> Result<String, Refusal> {
    let result = rewrite_result(UserRepository::update(&mut tx, entry).await);
    match rules::rewrite_outcome(&entry.id, result) {
        Ok(message) => {
            tx.commit().await?;
            Ok(message)
        }
        Err(failure) => abandon(tx, ServiceError::Rule(failure)).await,
    }
}

/// The row to rewrite, from the change test on Tokio's blocking pool within `work`, where
/// argon2id verifies a supplied password and hashes a changed one; [`answered`] logs its
/// failures once it is awaited. [`update`] awaits it only while no transaction is open.
async fn changed_entry(
    work: &PasswordWork,
    update: ValidatedUpdate,
    held: Held,
) -> Result<UserEntry, Refusal> {
    let computed = work.run(move || changed(&update, held)).await;
    answered(computed)
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
/// refusal, `ServiceError::Internal` when the new password cannot be hashed or the task did not
/// complete, or `ServiceError::Saturated` when the password-work budget refused the test.
///
/// Each operational failure is logged here, once, rather than on the blocking pool, so that the
/// event falls in the request's span, and as `ERROR`, since each answers a 500: the unverifiable
/// stored hash with the program's own `Unable to lookup User...`; the hash failure, by the fixed
/// class text of [`PasswordError`], and the task failure, by whether it panicked and never its
/// payload, with the unaudited 500. A refusal by the saturated budget is logged by the budget
/// itself, and a refusal for nothing modified is the user's input; neither logs here.
fn answered(
    computed: Result<Result<UserEntry, Unchanged>, PasswordWorkError>,
) -> Result<UserEntry, Refusal> {
    match computed {
        Ok(Ok(entry)) => Ok(entry),
        Ok(Err(Unchanged::Refused(failure))) => {
            // `rules::modified` refuses with a store-class failure only when the stored hash
            // cannot be verified; its other refusal is the input class's no-change message.
            if failure.class() == FailureClass::Store {
                hash_unverified();
            }
            Err(ServiceError::Rule(failure))
        }
        Ok(Err(Unchanged::NotHashed(error))) => {
            new_password_not_hashed(error);
            Err(ServiceError::Internal)
        }
        Err(PasswordWorkError::Saturated) => Err(ServiceError::Saturated),
        Err(PasswordWorkError::Interrupted { panicked }) => {
            password_task_incomplete(panicked);
            Err(ServiceError::Internal)
        }
    }
}

/// The `ERROR` event of a stored hash that could not be verified: the program and the
/// paragraph, and nothing of the row or the request.
fn hash_unverified() {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "hash_unverified",
        module_path!(),
        Level::ERROR,
        &["message", "program", "paragraph"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"stored password hash could not be verified; answering Unable to lookup User"),
            Some(&PROGRAM),
            Some(&CHANGE_PARAGRAPH),
        ],
    );
}

/// The `ERROR` event of a new password argon2id could not hash: the program, the paragraph and
/// the fixed class text of `error`.
fn new_password_not_hashed(error: PasswordError) {
    static SITE: EventSite<4> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "new_password_not_hashed",
        module_path!(),
        Level::ERROR,
        &["message", "program", "paragraph", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"new password not hashed; answering 500"),
            Some(&PROGRAM),
            Some(&CHANGE_PARAGRAPH),
            Some(&field::display(error)),
        ],
    );
}

/// The `ERROR` event of a change-test task that did not complete: the program, the paragraph
/// and whether it `panicked`, never its payload.
fn password_task_incomplete(panicked: bool) {
    static SITE: EventSite<4> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "password_task_incomplete",
        module_path!(),
        Level::ERROR,
        &["message", "program", "paragraph", "panicked"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"password task did not complete; answering 500"),
            Some(&PROGRAM),
            Some(&CHANGE_PARAGRAPH),
            Some(&panicked),
        ],
    );
}

/// The change test against the held row, its basis, and the row it yields: the request's names
/// and type, the held ID's key and `FILLER`, and a new hash only for a changed password
/// (COUSR02C-026 to COUSR02C-029, `E27`).
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
    static SITE: EventSite<4> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "usrsec_access_failed",
        module_path!(),
        Level::ERROR,
        &["message", "program", "paragraph", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"USRSEC access failed"),
            Some(&PROGRAM),
            Some(&paragraph),
            Some(&field::display(error)),
        ],
    );
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::future::poll_fn;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex, PoisonError};
    use std::task::Poll;
    use std::time::{Duration, Instant};

    use domain::SystemClock;
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use domain::online::cousr02c::{self as rules, RewriteResult, UserMaintFailure, UserRead};
    use domain::password::{self, PasswordError};
    use domain::records::UserEntry;
    use persistence::{PersistenceError, PgStore, UserRepository};
    use test_support::postgres::{PostgresGuard, start_postgres};
    use test_support::trace_capture;

    use super::{
        Refusal, Unchanged, answered, fetch, reread_refusal, rewrite_result, update, updated,
    };
    use crate::app::worker_hold::{WorkerHold, is_saturated, until_saturated};
    use crate::app::{AppState, PasswordWork, PasswordWorkError};
    use crate::auth::extract::{AdminUser, SignedOnUser};
    use crate::auth::jwt::JwtKeys;
    use crate::dto::cousr02c::UserUpdateRequest;
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread runtime, the one Tokio's test attribute
    /// builds, without the lint attributes that attribute's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// Runs `first` and `second` at once and answers both outputs once both have finished, the
    /// concurrent progress Tokio's join macro gives without the unchecked pinning and lint
    /// attributes its expansion carries: each future is pinned on the heap with `Box::pin`, the
    /// pin that needs no pinning macro, and every wake polls each one that has not yet finished.
    async fn join<A: Future, B: Future>(first: A, second: B) -> (A::Output, B::Output) {
        let (mut first, mut second) = (Box::pin(first), Box::pin(second));
        let (mut first_output, mut second_output) = (None, None);
        poll_fn(|cx| {
            if first_output.is_none() {
                if let Poll::Ready(output) = first.as_mut().poll(cx) {
                    first_output = Some(output);
                }
            }
            if second_output.is_none() {
                if let Poll::Ready(output) = second.as_mut().poll(cx) {
                    second_output = Some(output);
                }
            }
            match (first_output.take(), second_output.take()) {
                (Some(from_first), Some(from_second)) => Poll::Ready((from_first, from_second)),
                (from_first, from_second) => {
                    first_output = from_first;
                    second_output = from_second;
                    Poll::Pending
                }
            }
        })
        .await
    }

    /// A failure as the response shows it: message, field name and HTTP status.
    type Shown = (&'static str, &'static str, u16);

    /// A local-only signing secret for these tests.
    const SECRET: &str = "services-cousr02c-test-secret";

    /// The synthetic password the seeded users are stored with.
    const STORED_PASSWORD: &str = "TESTPW01";

    /// The synthetic password the update tests submit.
    const NEW_PASSWORD: &str = "NEWPW123";

    /// The synthetic password another admin stores while an update is in progress.
    const OTHER_PASSWORD: &str = "OTHERPW1";

    /// How long a test lets an update or a wait run before it counts it as stuck, as behind a
    /// row lock that is never released.
    const STUCK: Duration = Duration::from_secs(60);

    /// The `FILLER` every seeded row holds, so a rewrite that loses it shows.
    const FILLER: [u8; 23] = [b'Z'; 23];

    /// The `lock_timeout` of every session of an [`own_store`]: a free row is locked at once, so
    /// a lock wait the server ends after this long, SQLSTATE `55P03`, shows a row held by
    /// another transaction.
    const LOCK_TIMEOUT: &str = "1s";

    /// How long the first connection of an [`own_store`] that withholds it keeps its statement
    /// waiting before the pool gives up, as the database unavailable. Every later connection
    /// is ordinary and opens well within it.
    const WITHHELD: Duration = Duration::from_secs(2);

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

    /// A store on `guard`'s database with a pool of its own, whose every session ends a lock
    /// wait after [`LOCK_TIMEOUT`].
    ///
    /// With `withhold_first`, the pool's first connection never becomes usable, so the first
    /// statement through the store fails as the database unavailable once its acquire has
    /// waited [`WITHHELD`], and every later statement runs on an ordinary connection. For
    /// endpoint 19 that makes the unlocked read fail and the reads after it succeed, with no
    /// hook in the service or the store.
    fn own_store(guard: &PostgresGuard, withhold_first: bool) -> PgStore {
        let pool = guard.pool();
        let connect = pool
            .connect_options()
            .as_ref()
            .clone()
            .options([("lock_timeout", LOCK_TIMEOUT)]);
        let mut options = pool.options().clone().min_connections(0);
        if withhold_first {
            let first = AtomicBool::new(true);
            options = options
                .acquire_timeout(WITHHELD)
                .after_connect(move |_connection, _meta| {
                    let withhold = first.swap(false, Ordering::SeqCst);
                    Box::pin(async move {
                        if withhold {
                            std::future::pending::<()>().await;
                        }
                        Ok(())
                    })
                });
        }
        PgStore::new(options.connect_lazy_with(connect))
    }

    /// Locks `id` in a transaction of `store` and rolls it back, failing when the row is held
    /// elsewhere for longer than the store's lock wait.
    async fn locks_at_once(store: &PgStore, id: &PicX<8>) -> TestResult {
        let mut tx = store.begin().await?;
        UserRepository::lock_by_id(&mut tx, id)
            .await?
            .ok_or("no row to lock")?;
        tx.rollback().await?;
        Ok(())
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

    /// Endpoint 19 for `body` on `USER0001`, with `meanwhile` run while its first change test
    /// waits for a worker: after the update's basis read and before its transaction begins.
    ///
    /// `state`'s budget must have one worker and one waiting place. A held computation keeps
    /// the worker, so the update's change test waits in the place, which it takes only after its
    /// read; `meanwhile` runs once every place is taken, and the worker is then released.
    async fn interleaved_update(
        state: &AppState,
        body: UserUpdateRequest,
        meanwhile: impl Future<Output = TestResult>,
    ) -> Result<Result<String, Refusal>, Box<dyn StdError>> {
        let admin = admin()?;
        let hold = WorkerHold::start(&state.password_work).await?;
        let interleaved = async {
            tokio::time::timeout(STUCK, until_saturated(&state.password_work)).await?;
            meanwhile.await?;
            hold.release().await
        };
        let (outcome, released) = join(update(state, &admin, "USER0001", body), interleaved).await;
        assert!(released?, "the held computation was not released");
        Ok(outcome)
    }

    #[test]
    fn the_shared_read_answers_the_row_or_its_failure() -> TestResult {
        block_on(async {
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
        })
    }

    #[test]
    fn the_edits_run_in_order_before_any_read_and_write_nothing() -> TestResult {
        block_on(async {
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
        })
    }

    #[test]
    fn a_change_rewrites_the_whole_held_row() -> TestResult {
        block_on(async {
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
        })
    }

    #[test]
    fn store_failures_answer_with_the_programs_messages_and_trace_no_value() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store, &["USER0001"]).await?;
            UserRepository::insert(
                &mut store,
                &entry("BADHASH1", "not-a-phc-string".to_owned())?,
            )
            .await?;
            let state = app_state(store.clone())
                .with_password_work(PasswordWork::new(1, 1).with_wait(STUCK));
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

            // The store goes away after the read and before the transaction, which then cannot
            // begin: that has no audited text.
            let outcome = interleaved_update(&state, request("A", "B", None, "U"), async {
                store.pool().close().await;
                Ok(())
            })
            .await?;
            assert_eq!(
                outcome,
                Err(ServiceError::Store(PersistenceError::Unavailable))
            );

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
        })
    }

    /// Another admin rewrites the row after the update's unlocked read: the lock finds a row
    /// other than the basis, so the change test runs again against that row, and the rewrite is
    /// the request's change to that row, its hash and `FILLER` kept.
    #[test]
    fn a_row_rewritten_before_the_lock_is_tested_again_as_it_now_is() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store, &["USER0001"]).await?;
            let state = app_state(store.clone())
                .with_password_work(PasswordWork::new(1, 1).with_wait(STUCK));
            let original = stored(&store, "USER0001").await?;
            let larry = PicX::from_text("LARRY")?;

            let cases = [
                // A new password and `FILLER`: the rename lands on that row.
                (
                    UserEntry {
                        password_hash: password::hash(password_bytes(OTHER_PASSWORD)?)?,
                        filler: PicX::from_array([b'Y'; 23]),
                        ..original.clone()
                    },
                    Ok("User USER0001 has been updated ..."),
                ),
                // The very rename this request makes: against that row nothing is modified.
                (
                    UserEntry {
                        fname: larry.clone(),
                        ..original.clone()
                    },
                    Err(NOT_MODIFIED),
                ),
            ];
            for (theirs, expected) in cases {
                assert!(UserRepository::update(&mut store, &original).await?);
                let mut other = store.clone();
                let written = theirs.clone();
                let body = request("LARRY", "THOMAS", None, "U");
                let outcome = interleaved_update(&state, body, async move {
                    assert!(UserRepository::update(&mut other, &written).await?);
                    Ok(())
                })
                .await?;

                let answer = match &outcome {
                    Ok(message) => Ok(message.as_str()),
                    Err(_) => Err(shown(&outcome)),
                };
                assert_eq!(answer, expected.map_err(Some));
                let after = stored(&store, "USER0001").await?;
                assert_eq!(
                    after,
                    UserEntry {
                        fname: larry.clone(),
                        ..theirs
                    }
                );
            }
            Ok(())
        })
    }

    /// The unlocked read fails, so the update recovers its basis by the locked re-read of a
    /// transaction it rolls back at once: while the change test then waits for a worker, the row
    /// is free for another transaction to lock, and the rewrite lands afterwards. The recovery
    /// keeps the answers of the COBOL's order, the locked read first: nothing modified against
    /// the row it read, no row under the key, and a re-read that fails too, here a lock wait the
    /// server ends while another transaction holds the row; none of them writes. A transaction
    /// that cannot begin is `ServiceError::Store`, which the closed pool of
    /// `store_failures_answer_with_the_programs_messages_and_trace_no_value` shows.
    #[test]
    fn a_failed_unlocked_read_is_recovered_by_a_lock_released_before_the_change_test() -> TestResult
    {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store, &["USER0001"]).await?;
            let other = own_store(&guard, false);
            let key = PicX::from_text("USER0001")?;
            let admin = admin()?;
            let traces = trace_capture::capture();

            let state = app_state(own_store(&guard, true))
                .with_password_work(PasswordWork::new(1, 1).with_wait(STUCK));
            let body = request("LARRY", "THOMAS", None, "U");
            let outcome = interleaved_update(&state, body, locks_at_once(&other, &key)).await?;
            assert_eq!(outcome, Ok("User USER0001 has been updated ...".to_owned()));
            assert!(
                traces.contains("READ-USER-SEC-FILE"),
                "the unlocked read did not fail"
            );
            let renamed = stored(&store, "USER0001").await?;
            assert_eq!(
                (&renamed.fname, &renamed.filler),
                (&PicX::from_text("LARRY")?, &PicX::from_array(FILLER))
            );

            let cases = [
                (
                    "USER0001",
                    request("LARRY", "THOMAS", None, "U"),
                    false,
                    NOT_MODIFIED,
                ),
                (
                    "NOBODY01",
                    request("ANN", "LEE", None, "U"),
                    false,
                    NOT_FOUND,
                ),
                (
                    "USER0001",
                    request("ANN", "LEE", None, "U"),
                    true,
                    LOOKUP_FAILED,
                ),
            ];
            for (user_id, body, held, expected) in cases {
                let state = app_state(own_store(&guard, true));
                let mut holder = store.begin().await?;
                if held {
                    UserRepository::lock_by_id(&mut holder, &key)
                        .await?
                        .ok_or("no row to lock")?;
                }
                let outcome =
                    tokio::time::timeout(STUCK, update(&state, &admin, user_id, body)).await?;
                holder.rollback().await?;
                assert_eq!(shown(&outcome), Some(expected), "{user_id} held: {held}");
            }
            assert_eq!(stored(&store, "USER0001").await?, renamed);
            Ok(())
        })
    }

    /// Another admin rewrites the row while each change test waits for a worker, so every lock
    /// finds a row other than its basis. The update tests again against the row as it now is,
    /// and only once its deadline has passed does it roll back and answer as a failed rewrite,
    /// `Unable to Update User...`, with one event that names the bound and the attempts and
    /// holds no value: the row is as the other admin last left it, and at every wait the other
    /// admin locks it at once, since the update holds no lock while its change test waits.
    #[test]
    fn a_row_still_changing_after_the_deadline_is_refused() -> TestResult {
        /// The update's deadline, short so that the test reaches it in a few rounds.
        const DEADLINE: Duration = Duration::from_millis(300);
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store, &["USER0001"]).await?;
            let state = app_state(store.clone())
                .with_password_work(PasswordWork::new(1, 1).with_wait(STUCK));
            let work = &state.password_work;
            let other = own_store(&guard, false);
            let key = PicX::from_text("USER0001")?;
            let traces = trace_capture::capture();

            let mut hold = WorkerHold::start(work).await?;
            let started = Instant::now();
            let updating = tokio::spawn({
                let state = state.clone();
                let body = request("LARRY", "THOMAS", None, "U");
                async move { updated(&state, "USER0001", body, DEADLINE).await }
            });
            let mut rewrites: usize = 0;
            let mut theirs = None;
            loop {
                // Until the update's next change test waits for the held worker, or it answered.
                tokio::time::timeout(STUCK, async {
                    while !updating.is_finished() && !is_saturated(work) {
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                })
                .await?;
                if updating.is_finished() {
                    break;
                }
                // The lock is granted within the store's lock wait, or this read fails with
                // `55P03`: the waiting update holds none.
                let mut tx = other.begin().await?;
                let row = UserRepository::lock_by_id(&mut tx, &key)
                    .await?
                    .ok_or("no row to lock")?;
                rewrites += 1;
                let written = UserEntry {
                    filler: PicX::from_text(&format!("OTHER ADMIN {rewrites}"))?,
                    ..row
                };
                assert!(UserRepository::update(&mut tx, &written).await?);
                tx.commit().await?;
                theirs = Some(written);
                // The waiting change test takes the worker; the next one waits behind a new hold,
                // which runs once this change test has returned.
                assert!(
                    hold.release().await?,
                    "the held computation was not released"
                );
                hold = WorkerHold::start(work).await?;
            }
            let outcome = updating.await?;
            let answered_after = started.elapsed();
            assert!(hold.release().await?);

            assert_eq!(
                shown(&outcome),
                Some(("Unable to Update User...", "first_name", 500))
            );
            assert!(
                answered_after >= DEADLINE,
                "refused after {answered_after:?}, before its deadline"
            );
            let theirs = theirs.ok_or("the other admin never rewrote the row")?;
            assert_eq!(stored(&store, "USER0001").await?, theirs);
            let logged: Vec<String> = traces
                .lines()
                .into_iter()
                .filter(|line| line.contains("kept changing"))
                .collect();
            let [line] = logged.as_slice() else {
                return Err(format!("{} events: {}", logged.len(), traces.text()).into());
            };
            let event: serde_json::Value = serde_json::from_str(line)?;
            assert_eq!(
                event.get("level").and_then(serde_json::Value::as_str),
                Some("ERROR")
            );
            assert_eq!(
                event.get("target").and_then(serde_json::Value::as_str),
                Some("api::services::cousr02c")
            );
            // One change test per rewrite of the other admin, each confirmed against a newer row.
            assert_eq!(
                event.get("fields"),
                Some(&serde_json::json!({
                    "message": concat!(
                        "user row kept changing before its lock; ",
                        "answering Unable to Update User"
                    ),
                    "program": "COUSR02C",
                    "paragraph": "UPDATE-USER-SEC-FILE",
                    "deadline": "300ms",
                    "attempts": rewrites,
                }))
            );
            for value in ["USER0001", "LARRY", "LAWRENCE", "OTHER ADMIN", "$argon2id$"] {
                assert!(!line.contains(value), "the event holds {value}");
            }
            assert!(!traces.contains("$argon2id$"), "the trace holds a hash");
            Ok(())
        })
    }

    /// Six admins update the same user at once: every update reads the row before any of them
    /// commits, and their change tests run one after another on one worker, so each lock after
    /// a commit finds the row changed. Each update tests again against the row as it now is
    /// and commits in its turn, as the COBOL's `READ … UPDATE` serializes them: all six answer
    /// `User USER0001 has been updated ...`, and the row holds the names of the update that
    /// committed last, its hash and `FILLER` kept.
    #[test]
    fn concurrent_updates_of_one_user_all_succeed_and_the_last_commit_stands() -> TestResult {
        /// How many admins update the user at once.
        const UPDATES: usize = 6;
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store, &["USER0001"]).await?;
            let state = app_state(store.clone())
                .with_password_work(PasswordWork::new(1, UPDATES).with_wait(STUCK));
            let before = stored(&store, "USER0001").await?;
            let admin = admin()?;
            let committed = Arc::new(Mutex::new(Vec::new()));

            let hold = WorkerHold::start(&state.password_work).await?;
            let updates: Vec<_> = (1..=UPDATES)
                .map(|number| {
                    let state = state.clone();
                    let admin = admin.clone();
                    let committed = Arc::clone(&committed);
                    let body = request(
                        &format!("FIRST{number}"),
                        &format!("LAST{number}"),
                        None,
                        "U",
                    );
                    tokio::spawn(async move {
                        let outcome = update(&state, &admin, "USER0001", body).await;
                        // On this one thread, right after its commit, before any later commit.
                        if outcome.is_ok() {
                            committed
                                .lock()
                                .unwrap_or_else(PoisonError::into_inner)
                                .push(number);
                        }
                        outcome
                    })
                })
                .collect();
            // Every update has read the row and waits for the held worker: no change test ran.
            tokio::time::timeout(STUCK, until_saturated(&state.password_work)).await?;
            assert!(hold.release().await?);
            for (number, task) in (1_usize..).zip(updates) {
                let outcome = tokio::time::timeout(STUCK, task).await??;
                assert_eq!(
                    outcome,
                    Ok("User USER0001 has been updated ...".to_owned()),
                    "update {number}"
                );
            }

            let order = committed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            assert_eq!(order.len(), UPDATES, "{order:?}");
            let last = order.last().ok_or("no update committed")?;
            assert_eq!(
                stored(&store, "USER0001").await?,
                UserEntry {
                    fname: PicX::from_text(&format!("FIRST{last}"))?,
                    lname: PicX::from_text(&format!("LAST{last}"))?,
                    ..before
                }
            );
            Ok(())
        })
    }

    /// A refusal the change test decides opens no transaction, so it answers while another
    /// transaction holds the row `FOR UPDATE`, which any locked re-read would wait for.
    #[test]
    fn refusals_answer_without_the_row_lock_and_write_nothing() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store, &["USER0001"]).await?;
            UserRepository::insert(
                &mut store,
                &entry("BADHASH1", "not-a-phc-string".to_owned())?,
            )
            .await?;
            let state = app_state(store.clone());
            let saturated = PasswordWork::new(1, 0).with_wait(Duration::from_millis(50));
            let full = app_state(store.clone()).with_password_work(saturated);
            let admin = admin()?;
            let before = [
                stored(&store, "USER0001").await?,
                stored(&store, "BADHASH1").await?,
            ];

            let mut holder = store.begin().await?;
            for id in ["USER0001", "BADHASH1"] {
                UserRepository::lock_by_id(&mut holder, &PicX::from_text(id)?)
                    .await?
                    .ok_or("no row to lock")?;
            }
            let hold = WorkerHold::start(&full.password_work).await?;
            let cases = [
                // Nothing modified, with and without the stored password.
                (
                    &state,
                    "USER0001",
                    request("LAWRENCE", "THOMAS", None, "U"),
                    Some(NOT_MODIFIED),
                ),
                (
                    &state,
                    "USER0001",
                    request("LAWRENCE", "THOMAS", Some(STORED_PASSWORD), "U"),
                    Some(NOT_MODIFIED),
                ),
                // A stored hash that cannot be verified (`X-COUSR02C-4`).
                (
                    &state,
                    "BADHASH1",
                    request("LAWRENCE", "THOMAS", Some(NEW_PASSWORD), "U"),
                    Some(LOOKUP_FAILED),
                ),
                // A change the full budget refuses: the 503 of a test that found no place.
                (
                    &full,
                    "USER0001",
                    request("LARRY", "THOMAS", None, "U"),
                    None,
                ),
            ];
            for (state, user_id, body, expected) in cases {
                let outcome =
                    tokio::time::timeout(STUCK, update(state, &admin, user_id, body)).await?;
                assert_eq!(shown(&outcome), expected, "{user_id}");
                if expected.is_none() {
                    assert_eq!(outcome, Err(ServiceError::Saturated));
                }
            }
            assert!(hold.release().await?);
            holder.rollback().await?;
            let after = [
                stored(&store, "USER0001").await?,
                stored(&store, "BADHASH1").await?,
            ];
            assert_eq!(after, before);
            Ok(())
        })
    }

    /// The change test's failures are logged once, after the task is awaited, with their class
    /// and never a value, and keep their answers: the unverifiable stored hash its `Unable to
    /// lookup User...` (`X-COUSR02C-4`), the hash and task failures their 500.
    #[test]
    fn change_test_failures_are_logged_once_and_keep_their_answers() -> TestResult {
        block_on(async {
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
                    Err(PasswordWorkError::Interrupted { panicked: false }),
                    Err(ServiceError::Internal),
                    Some(("ERROR", r#""panicked":false"#)),
                ),
                // The saturated budget has logged its own refusal.
                (
                    Err(PasswordWorkError::Saturated),
                    Err(ServiceError::Saturated),
                    None,
                ),
                (
                    Ok(Err(Unchanged::NotHashed(PasswordError::Hash))),
                    Err(ServiceError::Internal),
                    Some(("ERROR", "password hashing failed")),
                ),
                (
                    Ok(Err(Unchanged::Refused(unverified))),
                    Err(ServiceError::Rule(unverified)),
                    Some(("ERROR", "stored password hash could not be verified")),
                ),
                (
                    Ok(Err(Unchanged::Refused(unmodified))),
                    Err(ServiceError::Rule(unmodified)),
                    None,
                ),
                (Ok(Ok(row.clone())), Ok(row), None),
            ];
            for (computed, expected, logged) in cases {
                traces.clear();
                let answer = answered(computed);
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
        })
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
