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
//!   (card number, SSN, CVV) into a token. It does not log or trace. The `Debug` of [`JwtKeys`]
//!   shows only the lifetime, never key material, and the `Debug` and `Display` of [`JwtError`]
//!   give only a fixed variant name or text, never the token, its claims or the `jsonwebtoken`
//!   error (Cardholder Data Redaction). The derived `Debug` of [`Claims`] shows its ordinary
//!   identity and lifetime fields (`sub`, `usr_type`, `iat`, `exp`), none a protected value.
//! - It reads no environment variable: the secret and the lifetime arrive as parameters from
//!   `runtime::config::Config` (`CARDDEMO_JWT_SECRET`, `CARDDEMO_JWT_TTL_SECONDS`), which also
//!   refuses a blank secret and one shorter than 32 bytes (Local-Only Runtime).
//! - It does not rebuild [`PicX`] or [`UserType`] values from verified claims; that is
//!   `api::auth::extract`'s job.
//!
//! A token is untrusted input: [`JwtKeys::verify`] returns a [`JwtError`] for any string and never
//! panics, and the lifetime is added with checked arithmetic (Total Parsers Over Untrusted Input,
//! Checked Integer Narrowing). The payload is read by the hand-written `Deserialize` of
//! [`Claims`], which accepts only a JSON object carrying each claim once and refuses an `nbf`
//! that is not a non-negative integer; a token whose `nbf` is still in the future is refused
//! as well.

use core::fmt;
use std::sync::Arc;

use domain::copybook::PicX;
use domain::online::cosgn00c::UserType;
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::de::{self, IgnoredAny, MapAccess, Visitor};
use serde::ser::SerializeStruct;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The one signing algorithm: tokens are issued with it and verification accepts no other.
const ALGORITHM: Algorithm = Algorithm::HS256;

/// The claims every verified token must carry before its payload is trusted. `nbf` is not among
/// them: issued tokens never carry it, and verification checks it only when it is present.
const REQUIRED_CLAIMS: [&str; 2] = ["exp", "sub"];

/// The members of a [`Claims`] payload, in the order they are serialised.
const CLAIM_NAMES: &[&str] = &["sub", "usr_type", "iat", "exp"];

/// The payload of an API token: the COMMAREA identity sign-on sets, and the token's lifetime
/// (COSGN00C-014).
///
/// It holds no password, card number, SSN or CVV, so the derived `Debug` shows only the user ID,
/// the user type and the two times. Its JSON form is the object
/// `{"sub":…,"usr_type":…,"iat":…,"exp":…}`, members in that order; the `Serialize` and
/// `Deserialize` impls are written out so that no generated code carries lint attributes.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// The four claims as one JSON object, members in the order `sub`, `usr_type`, `iat`, `exp`:
/// the payload of every token [`JwtKeys::issue_at`] signs.
impl Serialize for Claims {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut object = serializer.serialize_struct("Claims", CLAIM_NAMES.len())?;
        object.serialize_field("sub", &self.sub)?;
        object.serialize_field("usr_type", &self.usr_type)?;
        object.serialize_field("iat", &self.iat)?;
        object.serialize_field("exp", &self.exp)?;
        object.end()
    }
}

/// The claims of a token payload, which must be a JSON object; an array, a string, a number or
/// `null` is refused.
///
/// `sub`, `usr_type`, `iat` and `exp` are each required exactly once: a missing one is a
/// `missing_field` error and a repeated one a `duplicate_field` error. Member names are compared
/// after JSON unescaping, so `"s\u0075b"` is `sub`. An `nbf` member may appear at most once and
/// must be a non-negative integer that fits `u64`; its value is not kept, since
/// [`JwtKeys::verify`] checks it against the clock. Every other member is skipped, whatever its
/// value.
impl<'de> Deserialize<'de> for Claims {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_struct("Claims", CLAIM_NAMES, ClaimsVisitor)
    }
}

/// Reads a token payload object into [`Claims`].
struct ClaimsVisitor;

impl<'de> Visitor<'de> for ClaimsVisitor {
    type Value = Claims;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JWT claims set object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Claims, A::Error> {
        let mut sub = None;
        let mut usr_type = None;
        let mut iat = None;
        let mut exp = None;
        let mut not_before: Option<u64> = None;
        while let Some(member) = map.next_key()? {
            match member {
                Member::Sub => read_once(&mut map, &mut sub, "sub")?,
                Member::UsrType => read_once(&mut map, &mut usr_type, "usr_type")?,
                Member::Iat => read_once(&mut map, &mut iat, "iat")?,
                Member::Exp => read_once(&mut map, &mut exp, "exp")?,
                Member::NotBefore => read_once(&mut map, &mut not_before, "nbf")?,
                Member::Other => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(Claims {
            sub: sub.ok_or_else(|| de::Error::missing_field("sub"))?,
            usr_type: usr_type.ok_or_else(|| de::Error::missing_field("usr_type"))?,
            iat: iat.ok_or_else(|| de::Error::missing_field("iat"))?,
            exp: exp.ok_or_else(|| de::Error::missing_field("exp"))?,
        })
    }
}

/// Reads the value of member `name` into `slot`, refusing a member whose slot is already filled.
fn read_once<'de, A, T>(
    map: &mut A,
    slot: &mut Option<T>,
    name: &'static str,
) -> Result<(), A::Error>
where
    A: MapAccess<'de>,
    T: Deserialize<'de>,
{
    if slot.is_some() {
        return Err(de::Error::duplicate_field(name));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

/// The name of one member of a token payload, as [`Claims`] reads it.
enum Member {
    Sub,
    UsrType,
    Iat,
    Exp,
    /// The registered not-before claim, checked for its type and not kept.
    NotBefore,
    /// Any other name, whose value is skipped.
    Other,
}

/// A member name, whether the format lends it borrowed, copied after unescaping or owned.
impl<'de> Deserialize<'de> for Member {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_identifier(MemberVisitor)
    }
}

/// Reads one member name into a [`Member`].
struct MemberVisitor;

impl Visitor<'_> for MemberVisitor {
    type Value = Member;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a claim name")
    }

    fn visit_str<E: de::Error>(self, name: &str) -> Result<Member, E> {
        Ok(match name {
            "sub" => Member::Sub,
            "usr_type" => Member::UsrType,
            "iat" => Member::Iat,
            "exp" => Member::Exp,
            "nbf" => Member::NotBefore,
            _ => Member::Other,
        })
    }
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
    /// `secret` may be a `&str`, a `&String` or a `&[u8]`. Verification accepts HS256 only and
    /// requires `exp` and `sub`. Neither time has leeway: a token is rejected from the second
    /// after its `exp` and, when it carries an `nbf`, before the second its `nbf` names. Issued
    /// tokens carry no `nbf`, so the claim stays optional.
    #[must_use]
    pub fn new<S: AsRef<[u8]> + ?Sized>(secret: &S, ttl_seconds: u64) -> Self {
        let secret = secret.as_ref();
        let mut validation = Validation::new(ALGORITHM);
        validation.validate_exp = true;
        validation.validate_nbf = true;
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

    /// The claims of `token` once its signature, algorithm, expiry and not-before time are
    /// verified.
    ///
    /// Any header algorithm other than HS256 is rejected, `none` included, as is any text that is
    /// not a well-formed token whose payload the `Deserialize` of [`Claims`] reads.
    ///
    /// # Errors
    ///
    /// - [`JwtError::Expired`] for a correctly signed token whose `exp` is in the past.
    /// - [`JwtError::Invalid`] for every other failure: a bad signature, another algorithm, a
    ///   malformed token, a payload that is not an object, a missing, repeated or mistyped claim,
    ///   or an `nbf` still in the future.
    pub fn verify(&self, token: &str) -> Result<Claims, JwtError> {
        jsonwebtoken::decode::<Claims>(token, &self.0.decoding, &self.0.validation)
            .map(|data| data.claims)
            .map_err(|error| match error.kind() {
                ErrorKind::ExpiredSignature => JwtError::Expired,
                // `ImmatureSignature`, an `nbf` after the current second, is one of these.
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
    /// A bad signature, another algorithm, a malformed token, a payload that is not an object, a
    /// missing, repeated or mistyped claim, or an `nbf` still in the future.
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

    /// The URL-safe base64 alphabet (RFC 4648 section 5) of every JWT segment.
    const BASE64URL: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    /// The header of [`signed_text`] tokens.
    const HS256_HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

    /// `bytes` in unpadded URL-safe base64, as a JWT segment carries them.
    fn base64url(bytes: &[u8]) -> String {
        let mut text = String::new();
        let mut buffer = 0_u32;
        let mut bits = 0_u32;
        for &byte in bytes {
            buffer = (buffer << 8) | u32::from(byte);
            bits += 8;
            while bits >= 6 {
                bits -= 6;
                text.push(base64url_digit(buffer >> bits));
            }
            buffer &= (1 << bits) - 1;
        }
        if bits > 0 {
            text.push(base64url_digit(buffer << (6 - bits)));
        }
        text
    }

    /// The [`BASE64URL`] digit of the low six bits of `value`.
    fn base64url_digit(value: u32) -> char {
        usize::try_from(value & 0x3F)
            .ok()
            .and_then(|index| BASE64URL.get(index))
            .map(|&digit| char::from(digit))
            .expect("a six-bit index into the alphabet")
    }

    /// The JSON text `payload`, byte for byte, signed with [`SECRET`] under HS256: a payload may
    /// repeat a member or escape its name, which no JSON value holds.
    fn signed_text(payload: &str) -> String {
        let message = format!(
            "{}.{}",
            base64url(HS256_HEADER.as_bytes()),
            base64url(payload.as_bytes())
        );
        let signature = jsonwebtoken::crypto::sign(
            message.as_bytes(),
            &EncodingKey::from_secret(SECRET.as_bytes()),
            Algorithm::HS256,
        )
        .expect("an HMAC signature");
        format!("{message}.{signature}")
    }

    /// The payload text of `claims` as issued tokens carry it, with `extra` members appended.
    fn payload_text(claims: &Claims, extra: &str) -> String {
        format!(
            r#"{{"sub":"{}","usr_type":"{}","iat":{},"exp":{}{extra}}}"#,
            claims.sub, claims.usr_type, claims.iat, claims.exp
        )
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
    fn an_issued_payload_is_exactly_the_four_claims() {
        // The encoder reproduces both segments of a token whose text is known.
        let none_header = base64url(br#"{"alg":"none","typ":"JWT"}"#);
        let none_payload =
            base64url(br#"{"sub":"ADMIN001","usr_type":"A","iat":1700000000,"exp":4102444800}"#);
        assert_eq!(format!("{none_header}.{none_payload}."), ALG_NONE);

        let iat = now();
        let token = keys()
            .issue_at(&user_id("ADMIN001"), &UserType::ADMIN, iat)
            .expect("a signed token");
        let exp = iat.checked_add(TTL).expect("a lifetime within u64");
        let text = format!(r#"{{"sub":"ADMIN001","usr_type":"A","iat":{iat},"exp":{exp}}}"#);
        let payload = token.split('.').nth(1).expect("a payload segment");
        assert!(
            payload == base64url(text.as_bytes()),
            "the issued payload is not {text}"
        );

        let claims = Claims {
            sub: "ADMIN001".to_owned(),
            usr_type: "A".to_owned(),
            iat,
            exp,
        };
        assert_eq!(
            serde_json::to_string(&claims).expect("serialisable claims"),
            text
        );
        assert_eq!(
            serde_json::from_str::<Claims>(&text).expect("readable claims"),
            claims
        );
    }

    #[test]
    fn a_not_before_claim_is_checked_when_present() {
        let claims = admin_claims();
        let soon = claims
            .iat
            .checked_add(60)
            .expect("a small offset")
            .to_string();
        let earlier = claims
            .iat
            .checked_sub(60)
            .expect("a time after the epoch")
            .to_string();
        let issued = claims.iat.to_string();
        // (case, the `nbf` member's JSON value, whether the token verifies)
        let cases = [
            ("one minute ahead", soon.as_str(), false),
            ("in 2100", "4102444800", false),
            ("the last u64 second", "18446744073709551615", false),
            ("one minute ago", earlier.as_str(), true),
            ("the issue second", issued.as_str(), true),
            ("the epoch", "0", true),
            ("a string of a future time", r#""4102444800""#, false),
            ("a string of a past time", r#""0""#, false),
            ("a fraction", "1.5", false),
            ("a whole float of a past time", "1000000000.0", false),
            ("an exponent", "1e3", false),
            ("a negative time", "-1", false),
            ("a time beyond u64", "18446744073709551616", false),
            ("null", "null", false),
            ("a boolean", "true", false),
            ("an object", "{}", false),
            ("an array", "[0]", false),
        ];
        let keys = keys();
        for (case, nbf, verifies) in cases {
            let token = signed_text(&payload_text(&claims, &format!(r#","nbf":{nbf}"#)));
            let expected = if verifies {
                Ok(claims.clone())
            } else {
                Err(JwtError::Invalid)
            };
            assert_eq!(keys.verify(&token), expected, "nbf {case}");
        }
    }

    #[test]
    fn payload_members_are_read_once_by_name() {
        let claims = admin_claims();
        let (iat, exp) = (claims.iat, claims.exp);
        let with = |extra: &str| payload_text(&claims, extra);
        // (case, the payload's JSON text, whether the token verifies)
        let cases = [
            (
                "unknown members",
                with(r#","jti":"abc","scope":{"menu":[1,2]},"role":null"#),
                true,
            ),
            (
                "members in another order",
                format!(r#"{{"exp":{exp},"iat":{iat},"usr_type":"A","sub":"ADMIN001"}}"#),
                true,
            ),
            (
                "escaped member names",
                format!(r#"{{"s\u0075b":"ADMIN001","usr_\u0074ype":"A","iat":{iat},"exp":{exp}}}"#),
                true,
            ),
            ("a repeated sub", with(r#","sub":"ADMIN001""#), false),
            ("a repeated usr_type", with(r#","usr_type":"A""#), false),
            ("a repeated iat", with(&format!(r#","iat":{iat}"#)), false),
            ("a repeated exp", with(&format!(r#","exp":{exp}"#)), false),
            ("a repeated nbf", with(r#","nbf":0,"nbf":0"#), false),
            (
                "a sub repeated under an escaped name",
                with(r#","s\u0075b":"ADMIN001""#),
                false,
            ),
            (
                "an array",
                format!(r#"["ADMIN001","A",{iat},{exp}]"#),
                false,
            ),
            ("a string", r#""ADMIN001""#.to_owned(), false),
            ("a number", iat.to_string(), false),
            ("null", "null".to_owned(), false),
        ];
        let keys = keys();
        for (case, payload, verifies) in &cases {
            let expected = if *verifies {
                Ok(claims.clone())
            } else {
                Err(JwtError::Invalid)
            };
            assert_eq!(keys.verify(&signed_text(payload)), expected, "{case}");
        }
    }

    #[test]
    fn a_refused_payload_says_why() {
        let claims = admin_claims();
        let (iat, exp) = (claims.iat, claims.exp);
        let cases = [
            (
                format!(r#"{{"usr_type":"A","iat":{iat},"exp":{exp}}}"#),
                "missing field `sub`",
            ),
            (
                format!(r#"{{"sub":"ADMIN001","iat":{iat},"exp":{exp}}}"#),
                "missing field `usr_type`",
            ),
            (
                format!(r#"{{"sub":"ADMIN001","usr_type":"A","exp":{exp}}}"#),
                "missing field `iat`",
            ),
            (
                format!(r#"{{"sub":"ADMIN001","usr_type":"A","iat":{iat}}}"#),
                "missing field `exp`",
            ),
            (
                payload_text(&claims, r#","usr_type":"U""#),
                "duplicate field `usr_type`",
            ),
            (
                payload_text(&claims, r#","nbf":1,"nbf":2"#),
                "duplicate field `nbf`",
            ),
            (
                payload_text(&claims, r#","nbf":"0""#),
                "invalid type: string \"0\", expected u64",
            ),
            (
                payload_text(&claims, r#","nbf":-1"#),
                "invalid value: integer `-1`, expected u64",
            ),
            (
                format!(r#"["ADMIN001","A",{iat},{exp}]"#),
                "invalid type: sequence, expected a JWT claims set object",
            ),
        ];
        for (payload, reason) in &cases {
            let error = serde_json::from_str::<Claims>(payload).expect_err("a refused payload");
            assert!(error.to_string().starts_with(reason), "{payload}: {error}");
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
