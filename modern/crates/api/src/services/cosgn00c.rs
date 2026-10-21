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
//! | Decide | `EVALUATE WS-RESP-CD` and `SEC-USR-PWD = WS-USER-PWD` | `cosgn00c::decide` | COSGN00C-013, COSGN00C-018 to COSGN00C-020 |
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
//!   User ...` (COSGN00C-020). The error is reported as a warning event carrying the
//!   `PersistenceError` text alone, which names at most an SQLSTATE and a constraint, never the
//!   database's detail text or the key.
//!
//! The password is verified inside `cosgn00c::decide`, never here.
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

use domain::online::cosgn00c::{self, SignedOn, SignonFailure, UserLookup};
use persistence::rows::UserEntry;
use persistence::{PersistenceError, UserRepository};

use crate::app::AppState;
use crate::auth::jwt::JwtKeys;
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
/// - `ServiceError::Internal` when the token cannot be issued; it carries no detail.
pub(crate) async fn signon(
    state: &AppState,
    input: SignonRequest,
) -> Result<(String, SignedOn), ServiceError<SignonFailure>> {
    let SignonRequest { user_id, password } = input;
    let key = cosgn00c::edit(&user_id, &password).map_err(ServiceError::Rule)?;
    let mut store = state.store.clone();
    let read = UserRepository::find_by_id(&mut store, key.user_id()).await;
    let signed_on = cosgn00c::decide(&key, lookup(&read))
        .into_result()
        .map_err(ServiceError::Rule)?;
    let token = issue_token(&state.jwt, &signed_on)?;
    Ok((token, signed_on))
}

/// The `EVALUATE WS-RESP-CD` of `READ-USER-SEC-FILE` as a `UserLookup`: RESP 0, RESP 13 or
/// `WHEN OTHER` (COSGN00C-013, COSGN00C-019, COSGN00C-020).
///
/// The found row lends its hash to the lookup, so `read` outlives the decision.
fn lookup(read: &Result<Option<UserEntry>, PersistenceError>) -> UserLookup<'_> {
    match read {
        Ok(Some(entry)) => UserLookup::Found(entry.as_stored_user()),
        Ok(None) => UserLookup::NotFound,
        Err(error) => {
            // `PersistenceError` displays an SQLSTATE and a constraint name at most, never a
            // bound value, so the event names neither the user ID nor the row.
            tracing::warn!(error = %error, "sign-on user read failed");
            UserLookup::ReadError
        }
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
            tracing::error!(error = %error, "sign-on token could not be issued");
            ServiceError::Internal
        })
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::PathBuf;
    use std::sync::Arc;

    use domain::SystemClock;
    use domain::copybook::PicX;
    use domain::online::cosgn00c::{SignonFailure, SignonField, UserType};
    use domain::password;
    use persistence::rows::UserEntry;
    use persistence::{PgStore, UserRepository};
    use test_support::postgres::start_postgres;
    use test_support::trace_capture;

    use super::signon;
    use crate::app::AppState;
    use crate::auth::jwt::JwtKeys;
    use crate::dto::cosgn00c::SignonRequest;
    use crate::error::ServiceError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A local-only signing secret for these tests.
    const SECRET: &str = "services-cosgn00c-test-secret";

    /// The password every seeded user except `LOWER001` and `BADHASH1` is stored with.
    const TYPED_PASSWORD: &str = "TESTPW01";

    /// The password `LOWER001` is stored with, as typed into COUSR01C (E26).
    const LOWER_PASSWORD: &[u8; 8] = b"lowerpw1";

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

    #[tokio::test]
    async fn sign_on_outcomes_follow_the_read_and_the_stored_user() -> TestResult {
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
        let outcome = signon(&overflowing, request("ADMIN001", TYPED_PASSWORD)).await;
        assert_eq!(outcome, Err(ServiceError::Internal));
        Ok(())
    }

    #[tokio::test]
    async fn a_failed_read_is_unable_to_verify_and_reports_no_value() -> TestResult {
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
    }
}
