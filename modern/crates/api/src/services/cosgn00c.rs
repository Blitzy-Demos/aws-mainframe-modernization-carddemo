//! COSGN00C, sign-on: the service of endpoint 1, `POST /api/v1/signon` (transaction `CC00`;
//! AAP 0.3.4.1, 0.3.4.6).
//!
//! It ports the file statement and the outcome orchestration of `PROCESS-ENTER-KEY` and
//! `READ-USER-SEC-FILE` (`COSGN00C.cbl:117-136,209-257`). Every rule, from the blank-field edits
//! to the password check and the transfer target, lives in `domain::online::cosgn00c`; this
//! module reads the user, states the read's result in the domain's terms and issues the token.
//!
//! | Step | COBOL | Here | Rules |
//! |---|---|---|---|
//! | Edit | `EVALUATE TRUE` on `USERIDI` and `PASSWDI`, then `FUNCTION UPPER-CASE` of both | `cosgn00c::edit` | COSGN00C-003, COSGN00C-007 to COSGN00C-010 |
//! | Read | `EXEC CICS READ DATASET('USRSEC  ') RIDFLD(WS-USER-ID) KEYLENGTH(8)` | `UserRepository::find_by_id` on the upper-cased, space-padded user ID | COSGN00C-013, COSGN00C-019, COSGN00C-020 |
//! | Decide | `EVALUATE WS-RESP-CD` and `SEC-USR-PWD = WS-USER-PWD` | `cosgn00c::decide`, on Tokio's blocking pool within the API's password-work budget | COSGN00C-013, COSGN00C-018 to COSGN00C-020 |
//! | Transfer | COMMAREA identity, then `XCTL` to COADM01C or COMEN01C | the token of `JwtKeys::issue` and `SignedOn::next_route` | COSGN00C-014 to COSGN00C-016 |
//!
//! A blank-field failure returns before the read, as `IF NOT ERR-FLG-ON` skips
//! `READ-USER-SEC-FILE` (COSGN00C-010).
//!
//! # The read
//!
//! Each `WS-RESP-CD` the COBOL evaluates has a message of its own, so the read's result is never
//! propagated with `?`; it becomes a `UserLookup`:
//!
//! - a row (RESP 0) is `UserLookup::Found`, with the stored type and the argon2id hash that
//!   stands for `SEC-USR-PWD` (COSGN00C-013, E25);
//! - no row (RESP 13, `NOTFND`) is `UserLookup::NotFound`, answered `User not found. Try again
//!   ...` (COSGN00C-019);
//! - any store error (`WHEN OTHER`) is `UserLookup::ReadError`, answered `Unable to verify the
//!   User ...` (COSGN00C-020). The error is reported, inside the request span, as an error event
//!   carrying the `PersistenceError` text alone, which names at most an SQLSTATE and a
//!   constraint, never the database's detail text or the key.
//!
//! # The decision
//!
//! The password is verified inside `cosgn00c::decide`, never here. argon2id is deliberately
//! CPU- and memory-hard, so the decision takes the owned key and read to Tokio's blocking pool
//! and the request awaits it there: concurrent sign-ons for known users never hold a runtime
//! worker that unrelated requests need. The whole decision, an unknown user's included, runs
//! within the `AppState::password_work` budget, so a burst of sign-ons, which anyone can send,
//! runs a bounded number of verifications at once and queues a bounded number more
//! (`crate::app::PasswordWork`); a sign-on that finds every place taken waits for one, for a
//! bounded time. A decision the budget refuses because no place came free within that wait is
//! `ServiceError::Saturated`, a 503 with `Retry-After`, and one whose task does not complete is
//! `ServiceError::Internal`; no verification is skipped and no ordinary answer changes. The
//! token is issued only after a decision that signs on.
//!
//! # Identity
//!
//! The token's `sub` and `usr_type` come only from the `SignedOn` value: the upper-cased user ID
//! the row was read by and the stored `SEC-USR-TYPE` (COSGN00C-014, AAP 0.5.1). Nothing is kept
//! between requests, so every sign-on is decided from its own credentials (COSGN00C-006).
//!
//! # Redaction
//!
//! The password stays inside the `Password` of the `SignonKey`. The request, the stored row, the
//! hash and the token are never logged, formatted into an error or placed in a `ServiceError`;
//! a token that cannot be issued is `ServiceError::Internal`, which carries nothing (Cardholder
//! Data Redaction, AAP 0.3.4.8).

use domain::online::cosgn00c::{
    self, SignedOn, SignonFailure, SignonKey, SignonOutcome, StoredUser, UserLookup,
};
use domain::records::UserEntry;
use persistence::{PersistenceError, UserRepository};
use tracing::{Level, Metadata, field};

use crate::app::{AppState, EventSite, PasswordWork, PasswordWorkError, emit, event_metadata};
use crate::auth::jwt::{JwtError, JwtKeys};
use crate::dto::cosgn00c::SignonRequest;
use crate::error::ServiceError;

/// ENTER on the sign-on screen: edits the two inputs, reads the user by the upper-cased ID,
/// decides the outcome and, on success, issues the session token (COSGN00C-003 to COSGN00C-020,
/// `server` rows).
///
/// Returns the token with the `SignedOn` identity and route it was issued for, which
/// `SignonResponse::from` maps field for field.
///
/// # Errors
///
/// - `ServiceError::Rule` with the program's `SignonFailure`: a rejected or blank input (400),
///   a wrong password or an unknown user (401), and a failed read or an unverifiable stored hash
///   (500, `Unable to verify the User ...`).
/// - `ServiceError::Saturated` when the password-work budget finds no place for the decision
///   within its wait (503 with `Retry-After`).
/// - `ServiceError::Internal` when the verification task does not complete or the token cannot
///   be issued.
///
/// Neither carries any detail.
pub(crate) async fn signon(
    state: &AppState,
    input: SignonRequest,
) -> Result<(String, SignedOn), ServiceError<SignonFailure>> {
    let SignonRequest { user_id, password } = input;
    let key = cosgn00c::edit(&user_id, &password).map_err(ServiceError::Rule)?;
    let mut store = state.store.clone();
    let read = UserRepository::find_by_id(&mut store, key.user_id()).await;
    if let Err(error) = &read {
        // Reported here, inside the request span, before the decision leaves the runtime
        // worker. `PersistenceError` displays an SQLSTATE and a constraint name at most, never a
        // bound value, so the event names neither the user ID nor the row.
        read_failed(error);
    }
    let signed_on = decided(&state.password_work, key, read)
        .await?
        .into_result()
        .map_err(ServiceError::Rule)?;
    let token = issue_token(&state.jwt, &signed_on)?;
    Ok((token, signed_on))
}

/// The `ERROR` event of a failed user read (`WHEN OTHER`, COSGN00C-020): the store's `error`
/// alone, by its `Display`.
fn read_failed(error: &PersistenceError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "read_failed",
        module_path!(),
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"sign-on user read failed"),
            Some(&field::display(error)),
        ],
    );
}

/// `cosgn00c::decide` over the read's `UserLookup`, run on Tokio's blocking pool within `work`:
/// argon2id is deliberately CPU- and memory-hard, so verifying a known user's password inline
/// would stall the runtime worker and every request it serves, and verifying any number at once
/// would exhaust the host (COSGN00C-013, COSGN00C-018 to COSGN00C-020).
///
/// # Errors
///
/// - `ServiceError::Saturated` when the budget refuses the decision, which `work` has already
///   logged.
/// - `ServiceError::Internal` when the verification task does not complete. The task is
///   reported only as whether it panicked, never by its payload.
async fn decided(
    work: &PasswordWork,
    key: SignonKey,
    read: Result<Option<UserEntry>, PersistenceError>,
) -> Result<SignonOutcome, ServiceError<SignonFailure>> {
    work.run(move || cosgn00c::decide(&key, lookup(&read)))
        .await
        .map_err(|error| match error {
            PasswordWorkError::Saturated => ServiceError::Saturated,
            PasswordWorkError::Interrupted { panicked } => {
                verification_incomplete(panicked);
                ServiceError::Internal
            }
        })
}

/// The `ERROR` event of a verification task that did not complete: whether it `panicked`, never
/// its payload, and nothing of the user or the password.
fn verification_incomplete(panicked: bool) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "verification_incomplete",
        module_path!(),
        Level::ERROR,
        &["message", "panicked"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"sign-on verification task did not complete"),
            Some(&panicked),
        ],
    );
}

/// The `EVALUATE WS-RESP-CD` of `READ-USER-SEC-FILE` as a `UserLookup`: RESP 0, RESP 13 or
/// `WHEN OTHER` (COSGN00C-013, COSGN00C-019, COSGN00C-020).
///
/// The found row lends its hash to the lookup, so `read` outlives the decision. A failed read
/// has already been reported by `signon`, so this mapping emits nothing.
fn lookup(read: &Result<Option<UserEntry>, PersistenceError>) -> UserLookup<'_> {
    match read {
        Ok(Some(entry)) => UserLookup::Found(StoredUser::from(entry)),
        Ok(None) => UserLookup::NotFound,
        Err(_) => UserLookup::ReadError,
    }
}

/// The session token for the identity `READ-USER-SEC-FILE` sets before its `XCTL`
/// (COSGN00C-014).
///
/// # Errors
///
/// `ServiceError::Internal` when signing fails. The `JwtError` is reported only as its fixed
/// text in an error event, never returned.
fn issue_token(jwt: &JwtKeys, signed_on: &SignedOn) -> Result<String, ServiceError<SignonFailure>> {
    jwt.issue(&signed_on.user_id, &signed_on.user_type)
        .map_err(|error| {
            token_not_issued(error);
            ServiceError::Internal
        })
}

/// The `ERROR` event of a token that could not be signed: the fixed text of the `JwtError`,
/// never the claims or the key.
fn token_not_issued(error: JwtError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "token_not_issued",
        module_path!(),
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"sign-on token could not be issued"),
            Some(&field::display(error)),
        ],
    );
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::future::poll_fn;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::task::Poll;
    use std::time::{Duration, Instant};

    use domain::SystemClock;
    use domain::copybook::PicX;
    use domain::online::cosgn00c::{SignonFailure, SignonField, UserType};
    use domain::password;
    use domain::records::UserEntry;
    use persistence::{PgStore, UserRepository};
    use test_support::postgres::start_postgres;
    use test_support::trace_capture;

    use super::{signon, verification_incomplete};
    use crate::app::worker_hold::WorkerHold;
    use crate::app::{AppState, PasswordWork};
    use crate::auth::jwt::JwtKeys;
    use crate::dto::cosgn00c::SignonRequest;
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

    /// Runs four futures at once and answers their outputs in order once all have finished, as
    /// a [`join`] of two [`join`]s: every wake still polls each future not yet finished.
    async fn join4<A: Future, B: Future, C: Future, D: Future>(
        first: A,
        second: B,
        third: C,
        fourth: D,
    ) -> (A::Output, B::Output, C::Output, D::Output) {
        let ((first, second), (third, fourth)) =
            join(join(first, second), join(third, fourth)).await;
        (first, second, third, fourth)
    }

    /// A local-only signing secret for these tests.
    const SECRET: &str = "services-cosgn00c-test-secret";

    /// The password every seeded user except `LOWER001` and `BADHASH1` is stored with.
    const TYPED_PASSWORD: &str = "TESTPW01";

    /// The password `LOWER001` is stored with, as typed into COUSR01C (E26).
    const LOWER_PASSWORD: &[u8; 8] = b"lowerpw1";

    /// The period of the heartbeat that watches the runtime thread while sign-ons verify.
    const HEARTBEAT: Duration = Duration::from_millis(5);

    /// A `users` row with the given key, type and stored hash, names and `FILLER` filled.
    fn entry(
        id: &str,
        user_type: u8,
        password_hash: String,
    ) -> Result<UserEntry, Box<dyn StdError>> {
        Ok(UserEntry {
            id: PicX::from_text(id)?,
            fname: PicX::from_text("Test")?,
            lname: PicX::from_text("Signon")?,
            password_hash,
            usr_type: PicX::from_array([user_type]),
            filler: PicX::from_array([b' '; 23]),
        })
    }

    /// Stores the users the cases below sign on as.
    async fn seed(store: &mut PgStore) -> TestResult {
        let typed = password::hash(<&[u8; 8]>::try_from(TYPED_PASSWORD.as_bytes())?)?;
        let users = [
            entry("ADMIN001", b'A', typed.clone())?,
            entry("USER0001", b'U', typed)?,
            entry("LOWER001", b'U', password::hash(LOWER_PASSWORD)?)?,
            entry("BADHASH1", b'U', "not-an-argon2-phc-string".to_owned())?,
        ];
        for user in &users {
            UserRepository::insert(store, user).await?;
        }
        Ok(())
    }

    fn app_state(store: PgStore, ttl_seconds: u64) -> AppState {
        AppState::new(
            store,
            JwtKeys::new(SECRET, ttl_seconds),
            Arc::new(SystemClock),
            PathBuf::from("var/jobs"),
        )
    }

    fn request(user_id: &str, password: &str) -> SignonRequest {
        SignonRequest {
            user_id: user_id.to_owned(),
            password: password.to_owned(),
        }
    }

    #[test]
    fn sign_on_outcomes_follow_the_read_and_the_stored_user() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store).await?;
            let state = app_state(store.clone(), 3600);

            // audit: COSGN00C-009
            // audit: COSGN00C-013
            // audit: COSGN00C-014
            // audit: COSGN00C-015
            // audit: COSGN00C-016
            let successes = [
                (
                    "ADMIN001",
                    TYPED_PASSWORD,
                    "ADMIN001",
                    UserType::ADMIN,
                    "/admin",
                ),
                ("user0001", "testpw01", "USER0001", UserType::USER, "/menu"),
            ];
            for (user_id, typed, expected_id, expected_type, route) in successes {
                let (token, signed_on) = signon(&state, request(user_id, typed))
                    .await
                    .map_err(|error| error.to_string())?;
                assert_eq!(
                    signed_on.user_id.as_bytes(),
                    expected_id.as_bytes(),
                    "{user_id}"
                );
                assert_eq!(signed_on.user_type, expected_type, "{user_id}");
                assert_eq!(signed_on.next_route.path(), route, "{user_id}");
                let claims = state.jwt.verify(&token)?;
                assert_eq!(claims.sub, expected_id, "{user_id}");
                assert_eq!(claims.usr_type.as_bytes(), expected_type.code().as_bytes());
                assert!(!token.contains(TYPED_PASSWORD), "{user_id}");
            }

            // audit: COSGN00C-003
            // audit: COSGN00C-007
            // audit: COSGN00C-008
            // audit: COSGN00C-018
            // audit: COSGN00C-019
            // audit: COSGN00C-020
            let failures = [
                (
                    "ADMIN0011",
                    TYPED_PASSWORD,
                    SignonFailure::Rejected(SignonField::UserId),
                ),
                ("", TYPED_PASSWORD, SignonFailure::UserIdBlank),
                ("        ", "", SignonFailure::UserIdBlank),
                ("ADMIN001", "", SignonFailure::PasswordBlank),
                ("ADMIN001", "WRONGPW1", SignonFailure::WrongPassword),
                ("NOBODY01", TYPED_PASSWORD, SignonFailure::UserNotFound),
                ("LOWER001", "lowerpw1", SignonFailure::WrongPassword),
                ("BADHASH1", TYPED_PASSWORD, SignonFailure::UnableToVerify),
            ];
            for (user_id, typed, expected) in failures {
                let outcome = signon(&state, request(user_id, typed)).await;
                assert_eq!(outcome, Err(ServiceError::Rule(expected)), "{user_id:?}");
            }

            // A token that cannot be issued answers without detail: here the lifetime overflows.
            let overflowing = app_state(store, u64::MAX);
            let traces = trace_capture::capture();
            let outcome = signon(&overflowing, request("ADMIN001", TYPED_PASSWORD)).await;
            assert_eq!(outcome, Err(ServiceError::Internal));
            // It is logged as one `ERROR` of this module carrying the `JwtError::Lifetime` text
            // alone. The read writes lines of other targets, so only this module's are counted.
            let lines = traces.lines_with_target("api::services::cosgn00c");
            let [line] = lines.as_slice() else {
                return Err(format!("token not issued: {} lines", lines.len()).into());
            };
            let event: serde_json::Value = serde_json::from_str(line)?;
            assert_eq!(
                event.get("level").and_then(serde_json::Value::as_str),
                Some("ERROR")
            );
            assert_eq!(
                event.get("target").and_then(serde_json::Value::as_str),
                Some("api::services::cosgn00c")
            );
            assert_eq!(
                event.get("fields"),
                Some(&serde_json::json!({
                    "message": "sign-on token could not be issued",
                    "error": "token lifetime overflow",
                }))
            );
            for value in ["ADMIN001", TYPED_PASSWORD] {
                assert!(!traces.contains(value), "the trace holds {value}");
            }
            Ok(())
        })
    }

    #[test]
    fn a_failed_read_is_unable_to_verify_and_reports_no_value() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store).await?;
            store.pool().close().await;
            let state = app_state(store, 3600);

            let traces = trace_capture::capture();
            // audit: COSGN00C-020
            let outcome = signon(&state, request("ADMIN001", TYPED_PASSWORD)).await;
            assert_eq!(
                outcome,
                Err(ServiceError::Rule(SignonFailure::UnableToVerify))
            );
            assert!(traces.contains("sign-on user read failed"));
            for value in ["ADMIN001", TYPED_PASSWORD] {
                assert!(!traces.contains(value), "the trace holds {value}");
            }
            Ok(())
        })
    }

    #[test]
    fn a_full_password_budget_answers_saturated_until_a_place_frees() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store).await?;
            let budget = PasswordWork::new(1, 0).with_wait(Duration::from_millis(50));
            let state = app_state(store, 3600).with_password_work(budget);
            let hold = WorkerHold::start(&state.password_work).await?;
            let traces = trace_capture::capture();

            // The whole decision runs within the budget, so a refusal answers alike for a right
            // password, a wrong one and an unknown user, and tells nothing about the user ID.
            for (user_id, typed) in [
                ("ADMIN001", TYPED_PASSWORD),
                ("ADMIN001", "WRONGPW1"),
                ("NOBODY01", TYPED_PASSWORD),
            ] {
                let outcome = signon(&state, request(user_id, typed)).await;
                assert_eq!(outcome, Err(ServiceError::Saturated), "{user_id} {typed}");
            }
            assert!(traces.contains("password work budget saturated"));
            for value in [
                "ADMIN001",
                "NOBODY01",
                TYPED_PASSWORD,
                "WRONGPW1",
                "$argon2id$",
            ] {
                assert!(!traces.contains(value), "the trace holds {value}");
            }

            // Once the place frees, sign-on answers as it always does.
            assert!(hold.release().await?);
            let (_, signed_on) = signon(&state, request("ADMIN001", TYPED_PASSWORD))
                .await
                .map_err(|error| error.to_string())?;
            assert_eq!(signed_on.next_route.path(), "/admin");
            assert_eq!(
                signon(&state, request("ADMIN001", "WRONGPW1")).await,
                Err(ServiceError::Rule(SignonFailure::WrongPassword))
            );
            Ok(())
        })
    }

    /// A verification task that did not complete is logged as the `ERROR` a `tracing` macro of
    /// this module would write: its target, its message and whether the task panicked, and no
    /// other field.
    #[test]
    fn an_incomplete_verification_is_logged_as_an_error_of_this_module() -> TestResult {
        let traces = trace_capture::capture();
        for panicked in [true, false] {
            traces.clear();
            verification_incomplete(panicked);
            let lines = traces.lines();
            let [line] = lines.as_slice() else {
                return Err(format!("panicked {panicked}: {} lines", lines.len()).into());
            };
            let event: serde_json::Value = serde_json::from_str(line)?;
            assert_eq!(
                event.get("level").and_then(serde_json::Value::as_str),
                Some("ERROR")
            );
            assert_eq!(
                event.get("target").and_then(serde_json::Value::as_str),
                Some("api::services::cosgn00c")
            );
            assert_eq!(
                event.get("fields"),
                Some(&serde_json::json!({
                    "message": "sign-on verification task did not complete",
                    "panicked": panicked,
                }))
            );
        }
        Ok(())
    }

    /// Verification leaves the runtime serving: on this test's single runtime thread, a
    /// heartbeat keeps ticking while four known-user sign-ons verify their passwords at once.
    ///
    /// The bound is one argon2id verification measured on this host just before, halved. A
    /// decision run on the runtime thread holds it for at least one verification per sign-on,
    /// so the longest heartbeat gap would reach that duration; on the blocking pool the gap is
    /// a tick plus scheduling delay, far below it even on a loaded host, where the
    /// verification slows as much as the heartbeat.
    #[test]
    fn verification_runs_off_the_runtime_thread() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            seed(&mut store).await?;
            let state = app_state(store, 3600);

            let typed = <&[u8; 8]>::try_from(TYPED_PASSWORD.as_bytes())?;
            let hash = password::hash(typed)?;
            let started = Instant::now();
            assert!(password::verify(typed, &hash)?);
            let one_verification = started.elapsed();

            // Unknown users verify nothing; signing them on at once opens the pool's connections
            // first, so connection setup stays out of the measured window.
            let unknown = join4(
                signon(&state, request("NOBODY01", TYPED_PASSWORD)),
                signon(&state, request("NOBODY02", TYPED_PASSWORD)),
                signon(&state, request("NOBODY03", TYPED_PASSWORD)),
                signon(&state, request("NOBODY04", TYPED_PASSWORD)),
            )
            .await;
            let not_found = || Err(ServiceError::Rule(SignonFailure::UserNotFound));
            assert_eq!(
                unknown,
                (not_found(), not_found(), not_found(), not_found())
            );

            let mut signons = Box::pin(join4(
                signon(&state, request("ADMIN001", TYPED_PASSWORD)),
                signon(&state, request("USER0001", TYPED_PASSWORD)),
                signon(&state, request("ADMIN001", "WRONGPW1")),
                signon(&state, request("USER0001", "WRONGPW1")),
            ));
            // Every poll of the sign-ons happens inside one iteration, so a decision that held the
            // thread would lengthen that iteration's gap, including the iteration that finishes.
            let mut largest_gap = Duration::ZERO;
            let (admin, user, wrong_admin, wrong_user) = loop {
                let tick = Instant::now();
                // The sign-ons are polled first, then a fresh heartbeat, whose tick ends the
                // iteration while they are still pending.
                let mut heartbeat = Box::pin(tokio::time::sleep(HEARTBEAT));
                let finished = poll_fn(|cx| match signons.as_mut().poll(cx) {
                    Poll::Ready(outcomes) => Poll::Ready(Some(outcomes)),
                    Poll::Pending => heartbeat.as_mut().poll(cx).map(|()| None),
                })
                .await;
                largest_gap = largest_gap.max(tick.elapsed());
                if let Some(outcomes) = finished {
                    break outcomes;
                }
            };

            for (outcome, route) in [(admin, "/admin"), (user, "/menu")] {
                let (token, signed_on) = outcome.map_err(|error| error.to_string())?;
                assert_eq!(signed_on.next_route.path(), route);
                assert_eq!(
                    state.jwt.verify(&token)?.sub.as_bytes(),
                    signed_on.user_id.as_bytes()
                );
            }
            for outcome in [wrong_admin, wrong_user] {
                assert_eq!(
                    outcome,
                    Err(ServiceError::Rule(SignonFailure::WrongPassword))
                );
            }
            assert!(
                largest_gap < one_verification / 2,
                "the runtime thread stalled {largest_gap:?}; one verification takes \
                 {one_verification:?}"
            );
            Ok(())
        })
    }
}
