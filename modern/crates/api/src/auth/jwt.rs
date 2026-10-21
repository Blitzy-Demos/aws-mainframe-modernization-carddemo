//! The API's session token: the COMMAREA identity of a successful sign-on as a signed HS256 JWT,
//! and its verification (AAP 0.3.4.6, 0.5.1).
//!
//! COSGN00C sets `CDEMO-USER-ID PIC X(08)` and `CDEMO-USER-TYPE PIC X(01)` (88-levels
//! `CDEMO-USRTYP-ADMIN 'A'` and `CDEMO-USRTYP-USER 'U'`, `COCOM01Y.cpy:25-28`) only after the
//! password check succeeds, copying `SEC-USR-TYPE` from the `USRSEC` record
//! (`COSGN00C.cbl:221-240`; COSGN00C-014, COCOM01Y-005, COCOM01Y-006), and no program ever takes
//! them from screen input. [`JwtKeys::issue`] turns that identity into a token whose [`Claims`]
//! are `sub`, `usr_type`, `iat` and `exp`; [`JwtKeys::verify`] is the only way the API reads an
//! identity back, so the verified claims are the only acting identity it trusts.
//!
//! # What this module does not do
//!
//! - It never puts the password (`SEC-USR-PWD X(08)`, CSUSR01Y-004) or any other protected value
//!   into a token, and its `Debug` and `Display` output never shows key material, token text or
//!   claim content (Cardholder Data Redaction). It does not log or trace.
//! - It reads no environment variable: the secret and the lifetime arrive as parameters from
//!   `runtime::config::Config` (`CARDDEMO_JWT_SECRET`, `CARDDEMO_JWT_TTL_SECONDS`), which also
//!   refuses an empty secret (Local-Only Runtime).
//! - It does not rebuild [`PicX`] or [`UserType`] values from verified claims; that is
//!   `api::auth::extract`'s job.
//!
//! A token is untrusted input: [`JwtKeys::verify`] returns a [`JwtError`] for any string and never
//! panics, and the lifetime is added with checked arithmetic (Total Parsers Over Untrusted Input,
//! Checked Integer Narrowing).

use core::fmt;
use std::sync::Arc;

use domain::copybook::PicX;
use domain::online::cosgn00c::UserType;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};

/// The one signing algorithm: tokens are issued with it and verification accepts no other.
const ALGORITHM: Algorithm = Algorithm::HS256;

/// The claims every verified token must carry before its payload is trusted.
const REQUIRED_CLAIMS: [&str; 2] = ["exp", "sub"];

/// The payload of an API token: the COMMAREA identity sign-on sets, and the token's lifetime
/// (COSGN00C-014).
///
/// It holds no password, card number, SSN or CVV, so the derived `Debug` shows only the user ID,
/// the user type and the two times.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// `CDEMO-USER-ID`: the 8-byte upper-cased user ID without its trailing spaces, such as
    /// `ADMIN001` or `USER0001` (COCOM01Y-005).
    pub sub: String,
    /// `CDEMO-USER-TYPE`: the stored `SEC-USR-TYPE` byte as a 1-character string, never trimmed,
    /// so a type that is neither `A` nor `U`, a space included, is carried unchanged
    /// (COCOM01Y-006).
    pub usr_type: String,
    /// Issue time, in seconds since the Unix epoch.
    pub iat: u64,
    /// Expiry time, in seconds since the Unix epoch: `iat` plus the configured lifetime.
    pub exp: u64,
}

/// The HS256 signing and verification keys of the API, with the token lifetime.
///
/// Cloning shares one allocation, so the per-request copy the extractors take from the
/// application state is cheap. `Debug` shows the lifetime and never the key material.
#[derive(Clone)]
pub struct JwtKeys(Arc<Keys>);

/// What [`JwtKeys`] shares between its clones.
struct Keys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    ttl_seconds: u64,
}

impl JwtKeys {
    /// The keys derived from `secret`, issuing tokens valid for `ttl_seconds`.
    ///
    /// `secret` may be a `&str`, a `&String` or a `&[u8]`. Verification accepts HS256 only,
    /// requires `exp` and `sub`, and rejects a token from the second after its `exp` (no
    /// leeway).
    #[must_use]
    pub fn new<S: AsRef<[u8]> + ?Sized>(secret: &S, ttl_seconds: u64) -> Self {
        let secret = secret.as_ref();
        let mut validation = Validation::new(ALGORITHM);
        validation.validate_exp = true;
        validation.leeway = 0;
        validation.set_required_spec_claims(&REQUIRED_CLAIMS);
        Self(Arc::new(Keys {
            encoding: EncodingKey::from_secret(secret),
            decoding: DecodingKey::from_secret(secret),
            validation,
            ttl_seconds,
        }))
    }

    /// The lifetime, in seconds, of every token these keys issue.
    #[must_use]
    pub fn ttl_seconds(&self) -> u64 {
        self.0.ttl_seconds
    }

    /// A token for the identity of a successful sign-on, issued now (COSGN00C-014).
    ///
    /// # Errors
    ///
    /// As [`JwtKeys::issue_at`]: [`JwtError::Lifetime`] when the current time plus the lifetime
    /// overflows `u64`, [`JwtError::Sign`] when a value is not UTF-8 or signing fails.
    pub fn issue(&self, user_id: &PicX<8>, user_type: &UserType) -> Result<String, JwtError> {
        // The system clock `verify` checks `exp` against, not the injected `domain::Clock`:
        // `iat` is no COBOL field a rule writes, and both ends must read the same clock.
        self.issue_at(user_id, user_type, jsonwebtoken::get_current_timestamp())
    }

    /// A token for `user_id` and `user_type` issued at `iat` seconds since the Unix epoch and
    /// expiring `ttl_seconds` later.
    ///
    /// `sub` is the user ID without its trailing spaces and `usr_type` the user type's single
    /// byte, unchanged.
    ///
    /// # Errors
    ///
    /// - [`JwtError::Lifetime`] when `iat` plus the lifetime overflows `u64`.
    /// - [`JwtError::Sign`] when the user ID or the user type holds a byte sequence that is not
    ///   UTF-8, or the encoder fails.
    pub fn issue_at(
        &self,
        user_id: &PicX<8>,
        user_type: &UserType,
        iat: u64,
    ) -> Result<String, JwtError> {
        let exp = iat
            .checked_add(self.0.ttl_seconds)
            .ok_or(JwtError::Lifetime)?;
        let claims = Claims {
            sub: claim_text(user_id.trim_end())?,
            usr_type: claim_text(user_type.code().as_bytes())?,
            iat,
            exp,
        };
        jsonwebtoken::encode(&Header::new(ALGORITHM), &claims, &self.0.encoding)
            .map_err(|_| JwtError::Sign)
    }

    /// The claims of `token` once its signature, algorithm and expiry are verified.
    ///
    /// Any header algorithm other than HS256 is rejected, `none` included, as is any text that is
    /// not a well-formed token carrying every [`Claims`] field.
    ///
    /// # Errors
    ///
    /// - [`JwtError::Expired`] for a correctly signed token whose `exp` is in the past.
    /// - [`JwtError::Invalid`] for every other failure: a bad signature, another algorithm, a
    ///   malformed token or a missing claim.
    pub fn verify(&self, token: &str) -> Result<Claims, JwtError> {
        jsonwebtoken::decode::<Claims>(token, &self.0.decoding, &self.0.validation)
            .map(|data| data.claims)
            .map_err(|error| match error.kind() {
                ErrorKind::ExpiredSignature => JwtError::Expired,
                _ => JwtError::Invalid,
            })
    }
}

/// The lifetime only.
impl fmt::Debug for JwtKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtKeys")
            .field("ttl_seconds", &self.0.ttl_seconds)
            .finish_non_exhaustive()
    }
}

/// A claim's text from PIC bytes. The decoding error is dropped, since the result must carry
/// nothing of the value.
fn claim_text(bytes: &[u8]) -> Result<String, JwtError> {
    core::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| JwtError::Sign)
}

/// Why a token could not be issued or verified. No variant holds the token, its claims or the
/// underlying `jsonwebtoken` error, whose text could quote them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtError {
    /// Signing failed, or the user ID or user type is not UTF-8.
    Sign,
    /// The issue time plus the lifetime overflows `u64`.
    Lifetime,
    /// A bad signature, another algorithm, a malformed token or a missing claim.
    Invalid,
    /// The token's `exp` is in the past.
    Expired,
}

/// A fixed text per variant.
impl fmt::Display for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Sign => "token signing failed",
            Self::Lifetime => "token lifetime overflow",
            Self::Invalid => "invalid token",
            Self::Expired => "token expired",
        })
    }
}

impl std::error::Error for JwtError {}

#[cfg(test)]
mod tests {
    use domain::copybook::PicX;
    use domain::online::cosgn00c::UserType;
    use jsonwebtoken::{Algorithm, EncodingKey, Header};
    use serde_json::json;

    use super::{Claims, JwtError, JwtKeys};

    /// A test-only signing secret.
    const SECRET: &str = "unit-test-signing-key";

    /// The default `CARDDEMO_JWT_TTL_SECONDS`.
    const TTL: u64 = 3_600;

    /// Header `{"alg":"none","typ":"JWT"}`, payload
    /// `{"sub":"ADMIN001","usr_type":"A","iat":1700000000,"exp":4102444800}`, empty signature.
    const ALG_NONE: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.eyJzdWIiOiJBRE1JTjAwMSIsInVzcl90eXBlIjoiQSIsImlhdCI6MTcwMDAwMDAwMCwiZXhwIjo0MTAyNDQ0ODAwfQ.";

    fn keys() -> JwtKeys {
        JwtKeys::new(SECRET, TTL)
    }

    fn user_id(text: &str) -> PicX<8> {
        PicX::from_text(text).expect("a user ID of at most 8 printable ASCII bytes")
    }

    fn now() -> u64 {
        jsonwebtoken::get_current_timestamp()
    }

    /// Claims for `ADMIN001`, type `A`, issued now and valid for [`TTL`].
    fn admin_claims() -> Claims {
        let iat = now();
        Claims {
            sub: "ADMIN001".to_owned(),
            usr_type: "A".to_owned(),
            iat,
            exp: iat.checked_add(TTL).expect("a lifetime within u64"),
        }
    }

    /// `payload` signed with [`SECRET`] under `algorithm`, whatever its shape.
    fn signed<T: serde::Serialize>(algorithm: Algorithm, payload: &T) -> String {
        jsonwebtoken::encode(
            &Header::new(algorithm),
            payload,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .expect("an encodable payload")
    }

    #[test]
    fn an_issued_token_verifies_to_its_identity() {
        let odd_type = UserType::from_code(PicX::from_array([b' ']));
        let cases = [
            ("ADMIN001", UserType::ADMIN, "ADMIN001", "A"),
            ("USER0001", UserType::USER, "USER0001", "U"),
            // A short ID loses its pad spaces; only trailing spaces are padding.
            ("USER1", UserType::USER, "USER1", "U"),
            (" AB", UserType::USER, " AB", "U"),
            // A type that is neither `A` nor `U` is carried untrimmed.
            ("ODD00001", odd_type, "ODD00001", " "),
        ];
        let keys = keys();
        for (id, user_type, sub, usr_type) in cases {
            let before = now();
            let token = keys
                .issue(&user_id(id), &user_type)
                .expect("a signed token");
            let claims = keys.verify(&token).expect("a verified token");
            assert_eq!(claims.sub, sub, "{id}");
            assert_eq!(claims.usr_type, usr_type, "{id}");
            assert!((before..=now()).contains(&claims.iat), "{id}");
            assert_eq!(claims.exp.checked_sub(claims.iat), Some(TTL), "{id}");
        }
    }

    #[test]
    fn issue_at_signs_the_given_times() {
        let keys = keys();
        let iat = now();
        let token = keys
            .issue_at(&user_id("ADMIN001"), &UserType::ADMIN, iat)
            .expect("a signed token");
        let claims = keys.verify(&token).expect("a verified token");
        assert_eq!(
            claims,
            Claims {
                sub: "ADMIN001".to_owned(),
                usr_type: "A".to_owned(),
                iat,
                exp: iat.checked_add(TTL).expect("a lifetime within u64"),
            }
        );
        assert_eq!(keys.ttl_seconds(), TTL);
    }

    #[test]
    fn every_secret_form_derives_the_same_keys() {
        let token = keys()
            .issue(&user_id("USER0001"), &UserType::USER)
            .expect("a signed token");
        let owned = String::from(SECRET);
        let forms = [
            JwtKeys::new(SECRET, TTL),
            JwtKeys::new(&owned, TTL),
            JwtKeys::new(SECRET.as_bytes(), TTL),
        ];
        for keys in &forms {
            assert_eq!(
                keys.verify(&token).map(|claims| claims.sub),
                Ok("USER0001".to_owned())
            );
        }
        // A clone shares the keys of its original.
        let original = keys();
        let copy = original.clone();
        let token = original
            .issue(&user_id("ADMIN001"), &UserType::ADMIN)
            .expect("a signed token");
        assert!(copy.verify(&token).is_ok());
    }

    #[test]
    fn a_past_expiry_is_expired_without_leeway() {
        let keys = keys();
        let one_second_late = TTL.checked_add(1).expect("a small offset");
        let two_lifetimes = TTL.checked_mul(2).expect("a small offset");
        for offset in [one_second_late, two_lifetimes] {
            let iat = now().checked_sub(offset).expect("a time after the epoch");
            let token = keys
                .issue_at(&user_id("ADMIN001"), &UserType::ADMIN, iat)
                .expect("a signed token");
            assert_eq!(keys.verify(&token), Err(JwtError::Expired), "{offset}");
        }
    }

    #[test]
    fn a_token_of_another_secret_is_invalid() {
        let token = JwtKeys::new("other-secret", TTL)
            .issue(&user_id("ADMIN001"), &UserType::ADMIN)
            .expect("a signed token");
        assert_eq!(keys().verify(&token), Err(JwtError::Invalid));
    }

    #[test]
    fn only_hs256_is_accepted() {
        let keys = keys();
        assert_eq!(keys.verify(ALG_NONE), Err(JwtError::Invalid));
        for algorithm in [Algorithm::HS384, Algorithm::HS512] {
            let token = signed(algorithm, &admin_claims());
            assert_eq!(keys.verify(&token), Err(JwtError::Invalid), "{algorithm:?}");
        }
        let claims = admin_claims();
        let token = signed(Algorithm::HS256, &claims);
        assert_eq!(keys.verify(&token), Ok(claims));
    }

    #[test]
    fn a_token_missing_a_claim_is_invalid() {
        let claims = admin_claims();
        let payloads = [
            json!({ "usr_type": "A", "iat": claims.iat, "exp": claims.exp }),
            json!({ "sub": "ADMIN001", "usr_type": "A", "iat": claims.iat }),
            json!({ "sub": "ADMIN001", "iat": claims.iat, "exp": claims.exp }),
            json!({ "sub": "ADMIN001", "usr_type": "A", "exp": claims.exp }),
            json!({ "sub": "ADMIN001", "usr_type": "A", "iat": claims.iat, "exp": "never" }),
            json!({ "sub": 1, "usr_type": "A", "iat": claims.iat, "exp": claims.exp }),
        ];
        let keys = keys();
        for payload in &payloads {
            let token = signed(Algorithm::HS256, payload);
            assert_eq!(keys.verify(&token), Err(JwtError::Invalid), "{payload}");
        }
    }

    #[test]
    fn malformed_text_is_invalid() {
        let keys = keys();
        let valid = keys
            .issue(&user_id("ADMIN001"), &UserType::ADMIN)
            .expect("a signed token");
        let truncated = valid.get(..valid.len().saturating_sub(1)).unwrap_or("");
        let doubled = format!("{valid}.{valid}");
        for token in [
            "",
            "abc",
            "a.b.c",
            "..",
            "\u{e9}.\u{e9}.\u{e9}",
            truncated,
            &doubled,
        ] {
            assert_eq!(keys.verify(token), Err(JwtError::Invalid), "{token:?}");
        }
    }

    #[test]
    fn an_overflowing_lifetime_is_refused() {
        let keys = JwtKeys::new(SECRET, u64::MAX);
        assert_eq!(
            keys.issue_at(&user_id("ADMIN001"), &UserType::ADMIN, 1),
            Err(JwtError::Lifetime)
        );
        assert_eq!(
            keys.issue(&user_id("ADMIN001"), &UserType::ADMIN),
            Err(JwtError::Lifetime)
        );
    }

    #[test]
    fn bytes_that_are_not_utf8_are_not_signed() {
        let keys = keys();
        assert_eq!(
            keys.issue(&PicX::from_array([0xFF; 8]), &UserType::ADMIN),
            Err(JwtError::Sign)
        );
        assert_eq!(
            keys.issue(
                &user_id("ADMIN001"),
                &UserType::from_code(PicX::from_array([0xC3]))
            ),
            Err(JwtError::Sign)
        );
    }

    #[test]
    fn nothing_secret_is_formatted() {
        let keys = keys();
        let shown = format!("{keys:?}");
        assert!(!shown.contains(SECRET), "{shown}");
        assert_eq!(shown, "JwtKeys { ttl_seconds: 3600, .. }");

        let token = keys
            .issue(&user_id("ADMIN001"), &UserType::ADMIN)
            .expect("a signed token");
        let texts = [
            (JwtError::Sign, "token signing failed"),
            (JwtError::Lifetime, "token lifetime overflow"),
            (JwtError::Invalid, "invalid token"),
            (JwtError::Expired, "token expired"),
        ];
        for (error, text) in texts {
            let message = error.to_string();
            assert_eq!(message, text);
            assert!(!message.contains(&token));
            assert!(!format!("{error:?}").contains(&token));
        }
    }
}
