//! Password hashing and verification with argon2id, the replacement for the plaintext
//! `SEC-USR-PWD PIC X(08)` field of `USRSEC` (`CSUSR01Y.cpy:21`; copybook row CSUSR01Y-004).
//!
//! The COBOL keeps the 8 password bytes in the record and compares them with `=`. The replacement
//! stores, in `users.password_hash TEXT`, only the argon2id PHC string [`hash`] computes over
//! those same 8 bytes, and [`verify`] answers the comparison against it (escalation `E25`).
//! Callers pass the bytes of [`Password::expose`](crate::sensitive::Password::expose): exactly
//! the field's 8 bytes, space padding included.
//!
//! | Caller | Rule | Use |
//! |---|---|---|
//! | `dusrsecj` load | LOADS-033 | [`hash`] over the 8 bytes at offset 48 of each in-stream record; every run draws a new salt, so a rerun stores a different string that verifies the same bytes |
//! | User add (COUSR01C) | COUSR01C-018 | [`hash`] over the password as typed, padded with spaces |
//! | User update (COUSR02C) | COUSR02C-027 | [`verify`] decides the "modified?" test for a supplied password; a mismatch is hashed with [`hash`] to replace the stored string (`E27`) |
//! | Sign-on (COSGN00C) | COSGN00C-013, COSGN00C-018, COSGN00C-020 | [`verify`] over the upper-cased input stands in for `SEC-USR-PWD = WS-USER-PWD` |
//!
//! # No normalisation
//!
//! Nothing here folds case, trims or pads: the 8 bytes are hashed and compared exactly as given.
//! COUSR01C stores the password as typed (`COUSR01C.cbl:157`) while sign-on upper-cases what is
//! entered before comparing (`COSGN00C.cbl:132-136`, COSGN00C-009), and that upper-casing belongs
//! to `online::cosgn00c`. A stored password that holds a lower-case letter therefore never
//! verifies at sign-on, as in the COBOL (`E26`, COUSR01C-019).
//!
//! # Outcomes
//!
//! [`verify`] separates a mismatch from a stored string that cannot be verified. `Ok(false)` is
//! the COBOL's unequal compare, which sign-on reports as `Wrong Password. Try again ...`
//! (COSGN00C-018). A [`PasswordError`] has no COBOL counterpart, because an 8-byte compare cannot
//! fail to run; sign-on treats it as the read failure `Unable to verify the User ...`
//! (COSGN00C-020, X-COSGN00C-2), never as a wrong password, which would invite retries of a
//! credential that cannot verify. Both messages are decided by the online rules and the API
//! service from this result, not here.
//!
//! # Redaction
//!
//! No password, salt or hash text leaves this module except the PHC string [`hash`] returns for
//! storage (Cardholder Data Redaction, PIC-024). [`PasswordError`] carries no data and its
//! messages name the failure class only; argon2's own error values are discarded rather than
//! wrapped. Callers never log, trace or return the PHC string either.
//!
//! # Cost
//!
//! [`verify`] runs argon2 with the memory, time and lane parameters written inside the stored
//! string, so its cost is whatever the writer chose. Only [`hash`] writes that column, with
//! argon2's defaults.

use core::fmt;

use argon2::Argon2;
use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::{
    self, PasswordHash, PasswordHasher, PasswordVerifier, Salt, SaltString,
};

/// Why a password could not be hashed or a stored hash could not be verified.
///
/// The variants carry no data and the messages name the failure class only, so neither can
/// disclose a password, a salt or a hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordError {
    /// The salt could not be drawn from the operating-system RNG, or argon2 failed to hash.
    Hash,
    /// The stored text is not a PHC string, or it lacks the salt or the hash output that a
    /// stored password hash always has.
    MalformedHash,
    /// The stored PHC string parsed but names an algorithm, version or parameter set argon2 does
    /// not support, so the password could not be checked against it.
    Verify,
}

impl fmt::Display for PasswordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Hash => "password hashing failed",
            Self::MalformedHash => "stored password hash is not a complete PHC string",
            Self::Verify => "stored password hash could not be verified",
        })
    }
}

impl std::error::Error for PasswordError {}

/// Hashes the 8 bytes of a `SEC-USR-PWD` value with argon2id (version 0x13, argon2's default
/// parameters) and a fresh 16-byte salt, and returns the PHC string `users.password_hash` stores
/// (CSUSR01Y-004, LOADS-033, COUSR01C-018, COUSR02C-027; `E25`).
///
/// The bytes are hashed exactly as given, space padding included, with no case change (`E26`).
/// Two calls over the same bytes return different strings, each of which [`verify`] accepts for
/// those bytes.
///
/// # Errors
///
/// [`PasswordError::Hash`] when the operating-system RNG cannot supply the salt or argon2 fails
/// to compute the hash. User add reports this as its store failure (COUSR01C-024).
pub fn hash(password: &[u8; 8]) -> Result<String, PasswordError> {
    // `SaltString::generate` panics when the OS RNG fails; drawing the bytes with
    // `try_fill_bytes` turns that failure into an error instead.
    let mut salt_bytes = [0_u8; Salt::RECOMMENDED_LENGTH];
    OsRng
        .try_fill_bytes(&mut salt_bytes)
        .map_err(|_| PasswordError::Hash)?;
    let salt = SaltString::encode_b64(&salt_bytes).map_err(|_| PasswordError::Hash)?;
    Argon2::default()
        .hash_password(password, &salt)
        .map(|phc| phc.to_string())
        .map_err(|_| PasswordError::Hash)
}

/// Checks the 8 bytes of a password against a stored argon2 PHC string, the counterpart of the
/// COBOL compare `SEC-USR-PWD = WS-USER-PWD` (COSGN00C-013, COSGN00C-018, COUSR02C-027).
///
/// Returns `Ok(true)` when the bytes are the ones the string was computed over and `Ok(false)`
/// when they are not. The bytes are compared exactly as given: sign-on upper-cases its input
/// before calling, the update check does not (`E26`, `E27`).
///
/// Total over `stored_hash`: any text gives a value or an error, never a panic.
///
/// # Errors
///
/// - [`PasswordError::MalformedHash`] when `stored_hash` is not a PHC string, or is one without
///   a salt or a hash output. argon2 would report the missing parts as a mismatch, which sign-on
///   would show as a wrong password rather than as a hash it cannot verify (COSGN00C-020).
/// - [`PasswordError::Verify`] when the string names an algorithm, version or parameters argon2
///   cannot run.
pub fn verify(password: &[u8; 8], stored_hash: &str) -> Result<bool, PasswordError> {
    let parsed = PasswordHash::new(stored_hash).map_err(|_| PasswordError::MalformedHash)?;
    if parsed.salt.is_none() || parsed.hash.is_none() {
        return Err(PasswordError::MalformedHash);
    }
    match Argon2::default().verify_password(password, &parsed) {
        Ok(()) => Ok(true),
        Err(password_hash::Error::Password) => Ok(false),
        Err(_) => Err(PasswordError::Verify),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::{PasswordError, hash, verify};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The upper-case 8-byte password of the in-stream `USRSEC` records (`DUSRSECJ.jcl:35-44`).
    const UPPER: &[u8; 8] = b"PASSWORD";

    /// The same letters in lower case: what COUSR01C would store as typed (`E26`).
    const LOWER: &[u8; 8] = b"password";

    // audit: LOADS-033
    #[test]
    fn hash_is_an_argon2id_phc_string_with_default_parameters() -> TestResult {
        let phc = hash(UPPER)?;
        assert!(phc.starts_with("$argon2id$"));
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        Ok(())
    }

    // audit: COSGN00C-013
    // audit: COUSR01C-018
    #[test]
    fn the_hashed_bytes_verify() -> TestResult {
        assert_eq!(verify(UPPER, &hash(UPPER)?), Ok(true));
        Ok(())
    }

    // audit: COSGN00C-018
    // audit: COUSR01C-019
    #[test]
    fn other_bytes_do_not_verify_and_case_is_not_folded() -> TestResult {
        let upper = hash(UPPER)?;
        assert_eq!(verify(LOWER, &upper), Ok(false));
        assert_eq!(verify(UPPER, &hash(LOWER)?), Ok(false));
        assert_eq!(verify(b"PASSWOR ", &upper), Ok(false));
        Ok(())
    }

    // audit: LOADS-033
    #[test]
    fn each_hash_draws_a_fresh_salt() -> TestResult {
        let first = hash(UPPER)?;
        let second = hash(UPPER)?;
        assert_ne!(first, second);
        assert_eq!(verify(UPPER, &second), Ok(true));
        Ok(())
    }

    // audit: COSGN00C-020
    #[test]
    fn unverifiable_stored_text_is_an_error_not_a_mismatch() -> TestResult {
        let phc = hash(UPPER)?;
        let (without_output, output) = phc.rsplit_once('$').ok_or("PHC string has no output")?;
        let (head, salt) = without_output
            .rsplit_once('$')
            .ok_or("PHC string has no salt")?;
        let cases = [
            ("not-a-phc".to_owned(), PasswordError::MalformedHash),
            (String::new(), PasswordError::MalformedHash),
            ("$".to_owned(), PasswordError::MalformedHash),
            (head.to_owned(), PasswordError::MalformedHash),
            (format!("{head}${salt}"), PasswordError::MalformedHash),
            (format!("{phc}$extra"), PasswordError::MalformedHash),
            (
                format!("$md5$v=19$m=19456,t=2,p=1${salt}${output}"),
                PasswordError::Verify,
            ),
            (
                format!("$argon2id$v=99$m=19456,t=2,p=1${salt}${output}"),
                PasswordError::Verify,
            ),
            (
                format!("$argon2id$v=19$m=1,t=2,p=1${salt}${output}"),
                PasswordError::Verify,
            ),
        ];
        for (stored, expected) in cases {
            assert_eq!(verify(UPPER, &stored), Err(expected), "{stored}");
        }
        Ok(())
    }

    #[test]
    fn error_messages_name_the_class_only() {
        for (error, message) in [
            (PasswordError::Hash, "password hashing failed"),
            (
                PasswordError::MalformedHash,
                "stored password hash is not a complete PHC string",
            ),
            (
                PasswordError::Verify,
                "stored password hash could not be verified",
            ),
        ] {
            assert_eq!(error.to_string(), message);
        }
    }
}
