//! Argon2id password storage and verification for the `USRSEC` user file.
//!
//! The COBOL compares the stored and entered passwords in plaintext
//! (`COSGN00C` `READ-USER-SEC-FILE`, `IF SEC-USR-PWD = WS-USER-PWD`
//! [app/cbl/COSGN00C.cbl:L223]). Here the stored value is an argon2id PHC
//! string, a mandated deviation (E-20). The bytes hashed and the bytes
//! verified are exactly the `PIC X(08)` values the COBOL stores and compares,
//! so its case behaviour is kept: storage keeps the case as entered, sign-on
//! upper-cases. A password or user ID stored in lower or mixed case therefore
//! never signs on, as in the COBOL.
//!
//! This module imports nothing from the rest of the crate, which keeps it
//! outside the parity-audit gate. Input edits and their messages ('Please
//! enter Password ...', 'Password can NOT be empty...') belong to the rules
//! and the message catalog, not to this module.

use argon2::Argon2;
use password_hash::rand_core::OsRng;
use password_hash::{
    Error as PhcError, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};

/// Width of `SEC-USR-PWD PIC X(08)` in `CSUSR01Y` [app/cpy/CSUSR01Y.cpy:L21].
pub const SEC_USR_PWD_LEN: usize = 8;

/// Width of `SEC-USR-ID PIC X(08)` in `CSUSR01Y` [app/cpy/CSUSR01Y.cpy:L18].
pub const SEC_USR_ID_LEN: usize = 8;

/// Failure to compute a password hash or to use a stored one.
///
/// No variant carries the entered or padded password: the payload is the
/// `password_hash` error text, which holds none.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PasswordError {
    /// Salt handling or hash computation failed while storing a password.
    #[error("password hashing failed: {0}")]
    Hash(String),
    /// The stored PHC string does not parse, lacks a salt or hash output, or
    /// names an algorithm, version or parameters that argon2 rejects.
    #[error("stored password hash is malformed: {0}")]
    MalformedHash(String),
}

/// Stores a password the way the admin programs move it into `SEC-USR-PWD`.
///
/// `COUSR01C` `PROCESS-ENTER-KEY` moves `PASSWDI` to `SEC-USR-PWD` as entered
/// [app/cbl/COUSR01C.cbl:L154-158]; `COUSR02C` `UPDATE-USER-INFO` does the same
/// for a changed password [app/cbl/COUSR02C.cbl:L227-229]; the `DUSRSECJ` seed
/// rows hold the value as stored [app/jcl/DUSRSECJ.jcl:L35-44]. The password
/// is therefore padded and truncated to `PIC X(08)` with its case unchanged,
/// then hashed with `Argon2::default()` (argon2id, v=19, m=19456, t=2, p=1) and
/// a fresh `OsRng` salt. E-20.
///
/// Returns the PHC string `$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`.
///
/// # Errors
///
/// [`PasswordError::Hash`] when argon2 cannot compute the hash.
pub fn hash_stored_password(entered: &str) -> Result<String, PasswordError> {
    let padded = move_pic_x(entered, SEC_USR_PWD_LEN);
    let salt = SaltString::generate(&mut OsRng);
    let phc = Argon2::default()
        .hash_password(padded.as_bytes(), &salt)
        .map_err(|err| PasswordError::Hash(err.to_string()))?;
    Ok(phc.to_string())
}

/// Normalises a sign-on user ID into the `USRSEC` key `WS-USER-ID`.
///
/// `COSGN00C` `PROCESS-ENTER-KEY` moves `FUNCTION UPPER-CASE(USERIDI)` to
/// `WS-USER-ID PIC X(08)` [app/cbl/COSGN00C.cbl:L132-134], which is the key of
/// the `USRSEC` read [app/cbl/COSGN00C.cbl:L211-219]. E-20.
///
/// The result is exactly [`SEC_USR_ID_LEN`] bytes, space-padded. A caller
/// whose user-ID column or newtype is right-trimmed must trim the trailing
/// spaces the same way before the lookup. Upper-casing covers the ASCII range
/// only; other characters pass through unchanged.
#[must_use]
pub fn normalize_signon_user_id(entered: &str) -> String {
    move_pic_x(&entered.to_ascii_uppercase(), SEC_USR_ID_LEN)
}

/// Checks a sign-on password against the stored hash.
///
/// `COSGN00C` `PROCESS-ENTER-KEY` moves `FUNCTION UPPER-CASE(PASSWDI)` to
/// `WS-USER-PWD PIC X(08)` [app/cbl/COSGN00C.cbl:L135-136], and
/// `READ-USER-SEC-FILE` compares it with `SEC-USR-PWD`
/// [app/cbl/COSGN00C.cbl:L223]. The entered password is upper-cased (ASCII
/// range only), padded to `PIC X(08)` and verified. E-20.
///
/// `Ok(false)` is a wrong password.
///
/// # Errors
///
/// [`PasswordError::MalformedHash`] when the stored hash cannot be used.
pub fn verify_signon(entered: &str, phc: &str) -> Result<bool, PasswordError> {
    let padded = move_pic_x(&entered.to_ascii_uppercase(), SEC_USR_PWD_LEN);
    verify_padded(padded.as_bytes(), phc)
}

/// Checks whether an edited password equals the stored one, case kept.
///
/// `COUSR02C` `UPDATE-USER-INFO` treats the password as modified when
/// `PASSWDI NOT = SEC-USR-PWD` [app/cbl/COUSR02C.cbl:L227]. The entered value
/// is padded to `PIC X(08)` without a case change and verified. `Ok(true)`
/// means unchanged (keep the stored hash); `Ok(false)` means modified (store
/// it with [`hash_stored_password`]). The COBOL also reloads `SEC-USR-PWD`
/// into the edit field [app/cbl/COUSR02C.cbl:L166-170], which a hash cannot
/// reproduce. E-21.
///
/// # Errors
///
/// [`PasswordError::MalformedHash`] when the stored hash cannot be used.
pub fn verify_as_entered(entered: &str, phc: &str) -> Result<bool, PasswordError> {
    let padded = move_pic_x(entered, SEC_USR_PWD_LEN);
    verify_padded(padded.as_bytes(), phc)
}

/// COBOL alphanumeric `MOVE` into `PIC X(width)`: left-justified, truncated on
/// the right and space-padded to exactly `width` bytes.
///
/// NUL and other low-value bytes get no special treatment; the empty-field
/// edits live in the rules. A multibyte character straddling the boundary is
/// dropped whole, which keeps the result valid UTF-8 at `width` bytes.
///
/// Written here rather than taken from the crate's formatting module so this
/// module stays free of intra-crate imports and outside the audit gate.
fn move_pic_x(entered: &str, width: usize) -> String {
    let mut end = entered.len().min(width);
    while !entered.is_char_boundary(end) {
        end -= 1;
    }
    let mut field = String::with_capacity(width);
    field.push_str(&entered[..end]);
    field.extend(std::iter::repeat_n(' ', width - end));
    field
}

/// Verifies padded password bytes against a stored PHC string, using the
/// algorithm, version and parameters the string encodes.
///
/// A PHC string without a salt or hash output would make the blanket verifier
/// report a plain mismatch, so a corrupt row would look like a wrong password;
/// it is rejected as malformed first.
fn verify_padded(padded: &[u8], phc: &str) -> Result<bool, PasswordError> {
    let parsed =
        PasswordHash::new(phc).map_err(|err| PasswordError::MalformedHash(err.to_string()))?;
    if parsed.salt.is_none() || parsed.hash.is_none() {
        return Err(PasswordError::MalformedHash(
            "missing salt or hash output".to_owned(),
        ));
    }
    match Argon2::default().verify_password(padded, &parsed) {
        Ok(()) => Ok(true),
        Err(PhcError::Password) => Ok(false),
        Err(err) => Err(PasswordError::MalformedHash(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PasswordError, hash_stored_password, normalize_signon_user_id, verify_as_entered,
        verify_signon,
    };

    #[test]
    fn storage_keeps_case_signon_upper_cases() {
        let lower = hash_stored_password("pass1234").expect("hash pass1234");
        assert!(!verify_signon("pass1234", &lower).expect("verify lower"));
        assert!(verify_as_entered("pass1234", &lower).expect("verify lower as entered"));

        let mixed = hash_stored_password("Pass1234").expect("hash Pass1234");
        assert!(!verify_signon("Pass1234", &mixed).expect("verify mixed"));

        let upper = hash_stored_password("PASS1234").expect("hash PASS1234");
        assert!(verify_signon("pass1234", &upper).expect("verify upper"));
    }

    #[test]
    fn seed_password_signs_on() {
        let seeded = hash_stored_password("PASSWORD").expect("hash seed password");
        assert!(seeded.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert!(verify_signon("PASSWORD", &seeded).expect("verify as typed"));
        assert!(verify_signon("password", &seeded).expect("verify lower-case typing"));
        assert!(!verify_signon("PASSWORX", &seeded).expect("verify wrong password"));

        let reseeded = hash_stored_password("PASSWORD").expect("hash seed password again");
        assert_ne!(seeded, reseeded);
    }

    #[test]
    fn padding_and_truncation() {
        let short = hash_stored_password("abc").expect("hash abc");
        assert!(verify_as_entered("abc     ", &short).expect("verify padded"));
        assert!(!verify_as_entered("ABC", &short).expect("verify other case"));

        let long = hash_stored_password("PASSWORD9").expect("hash PASSWORD9");
        assert!(verify_as_entered("PASSWORD", &long).expect("verify truncated"));
    }

    #[test]
    fn malformed_hash_is_error() {
        assert!(matches!(
            verify_signon("PASSWORD", "not-a-phc"),
            Err(PasswordError::MalformedHash(_))
        ));
        assert!(matches!(
            verify_as_entered("x", ""),
            Err(PasswordError::MalformedHash(_))
        ));
        assert!(matches!(
            verify_signon(
                "PASSWORD",
                "$pbkdf2-sha256$i=1000$c2FsdHNhbHQ$aGFzaGhhc2hoYXNo"
            ),
            Err(PasswordError::MalformedHash(_))
        ));
        assert!(matches!(
            verify_signon("PASSWORD", "$argon2id$v=19$m=19456,t=2,p=1"),
            Err(PasswordError::MalformedHash(_))
        ));
    }

    #[test]
    fn signon_user_id_normalisation() {
        assert_eq!(normalize_signon_user_id("admin001"), "ADMIN001");
        assert_eq!(normalize_signon_user_id("user1"), "USER1   ");
        assert_eq!(normalize_signon_user_id("abcdefghij"), "ABCDEFGH");
        assert_eq!(normalize_signon_user_id(""), "        ");

        let accented = normalize_signon_user_id("us\u{e9}r01");
        assert_eq!(accented, "US\u{e9}R01 ");
        assert_eq!(accented.len(), 8);

        let straddling = normalize_signon_user_id("abcdefg\u{e9}");
        assert_eq!(straddling, "ABCDEFG ");
        assert_eq!(straddling.len(), 8);
    }
}
