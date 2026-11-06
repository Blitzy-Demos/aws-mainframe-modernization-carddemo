//! HS256 bearer tokens: the signed-on user on every API request.
//!
//! COSGN00C hands the signed-on user to the next program in the COMMAREA. It
//! moves the upper-cased user ID to `CDEMO-USER-ID PIC X(08)` and the `USRSEC`
//! record's `SEC-USR-TYPE` to `CDEMO-USER-TYPE PIC X(01)`, whose 88 levels are
//! `CDEMO-USRTYP-ADMIN` (`'A'`) and `CDEMO-USRTYP-USER` (`'U'`)
//! [app/cpy/COCOM01Y.cpy:L25-28]. It then transfers control with
//! `IF CDEMO-USRTYP-ADMIN XCTL COADM01C ELSE XCTL COMEN01C`
//! [app/cbl/COSGN00C.cbl:L224-240], so every code other than `'A'` takes the
//! user path.
//!
//! A token issued by `POST /auth/login` carries the same two values as the
//! claims `sub` and `usr_type`. The `usr_type` claim is the 88-level value of
//! the type, `A` or `U` ([`UserType`]): a stored `SEC-USR-TYPE` other than
//! exactly `A` is issued as `U`, the user path it takes in the COBOL. The
//! token is signed with HMAC-SHA256 under `CARDDEMO_JWT_SECRET` and expires
//! `CARDDEMO_JWT_TTL_SECS` seconds after it is issued. Verification accepts
//! HS256 only, allows no leeway on `exp`, requires `exp` and `sub`, and
//! rejects a `usr_type` claim that is not exactly `A` or `U`. No server-side
//! session exists: a token is valid until it expires, and logout only
//! discards it in the client.
//!
//! # Time
//!
//! `iat` and `exp` come from the system clock, never from
//! `carddemo_domain::clock`. `jsonwebtoken` checks `exp` against system time,
//! so a token stamped from a pinned `CARDDEMO_CLOCK_OVERRIDE` instant (such as
//! 2022-07-01) would arrive already expired. The clock override governs
//! business outputs only.
//!
//! # Secrets
//!
//! Neither the signing secret nor an issued token is ever printed. The `Debug`
//! output of [`JwtKeys`] and [`IssuedToken`] redacts them, and the text of a
//! [`JwtError`] names only its fixed category, [`JwtError::reason`].

use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::error::ApiError;

/// The only signing algorithm issued and accepted.
const ALGORITHM: Algorithm = Algorithm::HS256;

/// The registered claims a token must carry to be accepted.
const REQUIRED_CLAIMS: [&str; 2] = ["exp", "sub"];

/// Text `Debug` prints in place of a token.
const REDACTED: &str = "<redacted>";

/// `CDEMO-USRTYP-ADMIN VALUE 'A'` [app/cpy/COCOM01Y.cpy:L27].
const ADMIN_CODE: &str = "A";

/// `CDEMO-USRTYP-USER VALUE 'U'` [app/cpy/COCOM01Y.cpy:L28].
const USER_CODE: &str = "U";

/// The signed-on user's type, `CDEMO-USER-TYPE PIC X(01)`
/// [app/cpy/COCOM01Y.cpy:L26-28].
///
/// As the `usr_type` claim it is the 88-level value [`UserType::as_code`]:
/// `"A"` or `"U"`. Decoding accepts only the string `"A"` or `"U"`; any other
/// string and every other JSON shape (map, array, `null`, number, boolean)
/// fails, so a token whose claim lies outside that domain never verifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum UserType {
    /// `CDEMO-USRTYP-ADMIN`, the code `A`: COSGN00C transfers to the admin
    /// menu COADM01C, and the API admits the user to `/admin/**`.
    #[serde(rename = "A")]
    Admin,
    /// Every other code, `CDEMO-USRTYP-USER` (`U`) included: COSGN00C's `ELSE`
    /// branch transfers to the main menu COMEN01C. Its claim value is `U`.
    #[serde(rename = "U")]
    User,
}

impl UserType {
    /// The type a stored `SEC-USR-TYPE` code stands for.
    ///
    /// This is the 88-level test `IF CDEMO-USRTYP-ADMIN` on a one-byte field:
    /// only exactly `A` is [`UserType::Admin`]. The compare is case-sensitive,
    /// so `a`, an empty code and any other byte are [`UserType::User`], as the
    /// `ELSE` branch of [app/cbl/COSGN00C.cbl:L230-240] treats them.
    #[must_use]
    pub fn from_code(code: &str) -> Self {
        if code == ADMIN_CODE {
            Self::Admin
        } else {
            Self::User
        }
    }

    /// The 88-level value of the type: `A` for [`UserType::Admin`], `U` for
    /// [`UserType::User`].
    #[must_use]
    pub const fn as_code(self) -> &'static str {
        match self {
            Self::Admin => ADMIN_CODE,
            Self::User => USER_CODE,
        }
    }

    /// Whether the type is [`UserType::Admin`].
    #[must_use]
    pub const fn is_admin(self) -> bool {
        matches!(self, Self::Admin)
    }
}

impl<'de> Deserialize<'de> for UserType {
    /// Decodes the 88-level value from a string only. A derived decoder would
    /// also read the externally tagged form `{"A": null}` as `A`, so the
    /// claim's domain would not be the two strings alone.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(UserTypeVisitor)
    }
}

/// Reads a [`UserType`] from exactly [`ADMIN_CODE`] or [`USER_CODE`].
struct UserTypeVisitor;

impl de::Visitor<'_> for UserTypeVisitor {
    type Value = UserType;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the string \"{ADMIN_CODE}\" or \"{USER_CODE}\"")
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<UserType, E> {
        match value {
            ADMIN_CODE => Ok(UserType::Admin),
            USER_CODE => Ok(UserType::User),
            _ => Err(E::invalid_value(de::Unexpected::Str(value), &self)),
        }
    }
}

/// The claims every token carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// The signed-on user ID, `CDEMO-USER-ID`, exactly as the issuer passed
    /// it. The sign-on handler passes the upper-cased ID that COSGN00C reads
    /// `USRSEC` with [app/cbl/COSGN00C.cbl:L132-134].
    pub sub: String,
    /// The user's type, carried as its 88-level value `A` or `U`. The sign-on
    /// handler maps the stored `SEC-USR-TYPE PIC X(01)` code
    /// [app/cpy/CSUSR01Y.cpy:L22] to it with [`UserType::from_code`], so a
    /// legacy code such as `X` or `a` is issued as `U`.
    pub usr_type: UserType,
    /// Issue time, in whole seconds since the Unix epoch, from the system
    /// clock.
    pub iat: u64,
    /// Expiry, in whole seconds since the Unix epoch: `iat` plus the
    /// configured lifetime. The token is rejected once system time passes it.
    pub exp: u64,
}

/// A token issued by [`JwtKeys::issue`].
#[derive(Clone)]
pub struct IssuedToken {
    /// The compact `header.payload.signature` form, which the client sends
    /// back as `Authorization: Bearer <token>`.
    pub token: String,
    /// The instant the token expires: its `exp` claim.
    pub expires_at: DateTime<Utc>,
}

impl fmt::Debug for IssuedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Exhaustive destructuring: a field added to `IssuedToken` fails to
        // compile here until it is listed below.
        let Self {
            token: _,
            expires_at,
        } = self;
        f.debug_struct("IssuedToken")
            .field("token", &REDACTED)
            .field("expires_at", expires_at)
            .finish()
    }
}

/// The [`JwtError::reason`] of a token that is not a decodable JWT: not three
/// segments, a segment that is not base64url, or a header that is not the
/// JSON of a known algorithm (`alg` `none` included).
const REASON_MALFORMED: &str = "malformed";

/// The [`JwtError::reason`] of a token whose signature does not match the
/// secret.
const REASON_BAD_SIGNATURE: &str = "bad_signature";

/// The [`JwtError::reason`] of a token signed with an algorithm other than
/// HS256.
const REASON_WRONG_ALGORITHM: &str = "wrong_algorithm";

/// The [`JwtError::reason`] of a token whose `exp` system time has passed.
const REASON_EXPIRED: &str = "expired";

/// The [`JwtError::reason`] of a correctly signed token whose claims are not
/// [`Claims`]: a claim missing or of the wrong type, or a `usr_type` other
/// than `A` or `U`.
const REASON_INVALID_CLAIMS: &str = "invalid_claims";

/// The [`JwtError::reason`] of every other failure of the decoder, such as a
/// key or cryptographic error.
const REASON_VERIFICATION_FAILED: &str = "verification_failed";

/// A token that failed verification: malformed, signed with another key or
/// algorithm, expired, missing a required claim, or with a `usr_type` claim
/// other than `A` or `U`.
///
/// Its text is `token rejected: ` and the [`JwtError::reason`], so it never
/// quotes the token, a claim value or the decoder's own message, which can
/// quote claim values; the decoder's error is not its `source` either.
#[derive(Debug, thiserror::Error)]
#[error("token rejected: {reason}")]
pub struct JwtError {
    /// The decoder's error; only its [`ErrorKind`] is exposed.
    cause: jsonwebtoken::errors::Error,
    /// The fixed category [`JwtError::reason`] returns.
    reason: &'static str,
}

impl JwtError {
    /// The rejection of `token`, which the decoder refused with `cause`.
    ///
    /// The decoder reads the header before it checks the signature, and the
    /// claims only once the signature matches. A JSON failure while the
    /// header of `token` decodes therefore lies in the claims of a correctly
    /// signed token, and any other JSON failure in a header no one signed.
    fn new(cause: jsonwebtoken::errors::Error, token: &str) -> Self {
        let reason = match cause.kind() {
            ErrorKind::ExpiredSignature => REASON_EXPIRED,
            ErrorKind::InvalidSignature => REASON_BAD_SIGNATURE,
            ErrorKind::InvalidAlgorithm
            | ErrorKind::InvalidAlgorithmName
            | ErrorKind::MissingAlgorithm => REASON_WRONG_ALGORITHM,
            ErrorKind::Json(_) if jsonwebtoken::decode_header(token).is_ok() => {
                REASON_INVALID_CLAIMS
            }
            ErrorKind::MissingRequiredClaim(_)
            | ErrorKind::InvalidIssuer
            | ErrorKind::InvalidAudience
            | ErrorKind::InvalidSubject
            | ErrorKind::ImmatureSignature => REASON_INVALID_CLAIMS,
            ErrorKind::InvalidToken
            | ErrorKind::Base64(_)
            | ErrorKind::Utf8(_)
            | ErrorKind::Json(_) => REASON_MALFORMED,
            _ => REASON_VERIFICATION_FAILED,
        };
        Self { cause, reason }
    }

    /// Why the token was rejected, for example
    /// [`ErrorKind::InvalidSignature`], [`ErrorKind::InvalidAlgorithm`] or
    /// [`ErrorKind::ExpiredSignature`].
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        self.cause.kind()
    }

    /// The fixed category of the rejection, safe to log: `malformed`,
    /// `bad_signature`, `wrong_algorithm`, `expired`, `invalid_claims` (a
    /// correctly signed token whose claims are not [`Claims`], a `usr_type`
    /// other than `A` or `U` included) or `verification_failed`.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

/// The signing and verification keys, built once from `CARDDEMO_JWT_SECRET`
/// and `CARDDEMO_JWT_TTL_SECS` and shared by every request.
pub struct JwtKeys {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    ttl_secs: u64,
}

impl fmt::Debug for JwtKeys {
    /// Shows the algorithm and the lifetime only: the keys hold the secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JwtKeys")
            .field("algorithm", &ALGORITHM)
            .field("ttl_secs", &self.ttl_secs)
            .finish_non_exhaustive()
    }
}

impl JwtKeys {
    /// Keys that sign and verify HS256 tokens with `secret` and issue them
    /// with a lifetime of `ttl_secs` seconds.
    ///
    /// Verification accepts HS256 only, so a token signed with another
    /// algorithm, or with `alg` `none`, is rejected. It allows no leeway on
    /// `exp` and requires the claims `exp` and `sub`.
    #[must_use]
    pub fn new(secret: &str, ttl_secs: u64) -> Self {
        let mut validation = Validation::new(ALGORITHM);
        validation.leeway = 0;
        validation.validate_exp = true;
        validation.set_required_spec_claims(&REQUIRED_CLAIMS);
        Self {
            encoding: EncodingKey::from_secret(secret.as_bytes()),
            decoding: DecodingKey::from_secret(secret.as_bytes()),
            validation,
            ttl_secs,
        }
    }

    /// The lifetime of an issued token, in seconds.
    #[must_use]
    pub const fn ttl_secs(&self) -> u64 {
        self.ttl_secs
    }

    /// Whether a token issued now, by the system clock, gets an expiry
    /// [`JwtKeys::issue`] can represent: `iat` plus [`JwtKeys::ttl_secs`]
    /// fits a `u64` and is an instant `chrono` represents.
    ///
    /// Start-up calls this once, so a lifetime no token could be issued with
    /// fails before the server listens rather than on every sign-on. A system
    /// clock before the Unix epoch counts as `iat` 0 here; [`JwtKeys::issue`]
    /// still rejects such a clock, and an expiry that stops being
    /// representable later, on each request.
    #[must_use]
    pub fn expiry_is_representable(&self) -> bool {
        let iat = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        expiry(iat, self.ttl_secs).is_some()
    }

    /// Issues a token for `user_id` with type `usr_type`: the `sub` claim is
    /// `user_id` exactly as passed, and the `usr_type` claim is the 88-level
    /// value of `usr_type`, `A` or `U`. It is valid from now, by the system
    /// clock, for [`JwtKeys::ttl_secs`] seconds.
    ///
    /// # Errors
    ///
    /// [`ApiError::unexpected`] (500) when the system clock is before the Unix
    /// epoch, when the expiry overflows or cannot be represented as a
    /// timestamp, or when signing fails. The cause is logged; the response
    /// carries no detail.
    pub fn issue(&self, user_id: &str, usr_type: UserType) -> Result<IssuedToken, ApiError> {
        let iat = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| {
                tracing::error!(error = %err, "system clock is before the Unix epoch");
                ApiError::unexpected()
            })?
            .as_secs();
        let (exp, expires_at) = expiry(iat, self.ttl_secs).ok_or_else(|| {
            tracing::error!(
                iat,
                ttl_secs = self.ttl_secs,
                "token expiry overflows a 64-bit timestamp or is not a representable instant"
            );
            ApiError::unexpected()
        })?;

        let claims = Claims {
            sub: user_id.to_owned(),
            usr_type,
            iat,
            exp,
        };
        let token = jsonwebtoken::encode(&Header::new(ALGORITHM), &claims, &self.encoding)
            .map_err(|err| {
                tracing::error!(error = %err, "token signing failed");
                ApiError::unexpected()
            })?;
        Ok(IssuedToken { token, expires_at })
    }

    /// Verifies `token` and returns its claims.
    ///
    /// Nothing is logged here; the caller decides what a rejection records.
    ///
    /// # Errors
    ///
    /// [`JwtError`] when the token is malformed, has a signature that does not
    /// match this secret, names an algorithm other than HS256 (`none`
    /// included), has an `exp` that system time has passed, lacks a claim
    /// [`Claims`] or the validation requires, or has a `usr_type` claim other
    /// than exactly `A` or `U`.
    pub fn verify(&self, token: &str) -> Result<Claims, JwtError> {
        jsonwebtoken::decode::<Claims>(token, &self.decoding, &self.validation)
            .map(|data| data.claims)
            .map_err(|cause| JwtError::new(cause, token))
    }
}

/// The `exp` claim and the expiry instant of a token issued at `iat`, in
/// whole seconds since the Unix epoch, for `ttl_secs` seconds.
///
/// `None` when the sum overflows a `u64`, does not fit the `i64` of a
/// timestamp, or lies past the last instant `chrono` represents.
fn expiry(iat: u64, ttl_secs: u64) -> Option<(u64, DateTime<Utc>)> {
    let exp = iat.checked_add(ttl_secs)?;
    let expires_at = DateTime::<Utc>::from_timestamp(i64::try_from(exp).ok()?, 0)?;
    Some((exp, expires_at))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    use chrono::{DateTime, Utc};
    use jsonwebtoken::errors::ErrorKind;
    use jsonwebtoken::{Algorithm, Header};
    use serde::Serialize;
    use serde_json::json;

    use super::{Claims, JwtError, JwtKeys, UserType, expiry};
    use crate::error::ApiError;

    const SECRET: &str = "test-secret";
    const TTL_SECS: u64 = 3600;

    /// base64url of `{"alg":"none","typ":"JWT"}`.
    const NONE_HEADER: &str = "eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0";

    /// base64url of `{"alg":"FOO"}`: JSON, but no algorithm the decoder knows.
    const UNKNOWN_ALG_HEADER: &str = "eyJhbGciOiJGT08ifQ";

    fn assert_send_sync<T: Send + Sync>() {}

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the system clock is after the epoch")
            .as_secs()
    }

    fn keys() -> JwtKeys {
        JwtKeys::new(SECRET, TTL_SECS)
    }

    fn issue(keys: &JwtKeys, user_id: &str, usr_type: UserType) -> String {
        keys.issue(user_id, usr_type).expect("issues").token
    }

    /// The claims of a token the keys verify, as JSON rather than [`Claims`].
    fn wire_claims(keys: &JwtKeys, token: &str) -> serde_json::Value {
        jsonwebtoken::decode::<serde_json::Value>(token, &keys.decoding, &keys.validation)
            .expect("verifies")
            .claims
    }

    /// Signs `claims` with the keys' own secret under `algorithm`.
    fn sign(keys: &JwtKeys, algorithm: Algorithm, claims: &impl Serialize) -> String {
        jsonwebtoken::encode(&Header::new(algorithm), claims, &keys.encoding).expect("signs")
    }

    fn claims(iat: u64, exp: u64) -> Claims {
        Claims {
            sub: "ADMIN001".to_owned(),
            usr_type: UserType::Admin,
            iat,
            exp,
        }
    }

    /// `token` split at its last `.` into `header.payload` and the signature.
    fn split_signature(token: &str) -> (&str, &str) {
        token.rsplit_once('.').expect("a compact token")
    }

    fn rejection(keys: &JwtKeys, token: &str) -> JwtError {
        keys.verify(token).expect_err("the token is rejected")
    }

    #[test]
    fn keys_are_shareable_across_threads() {
        assert_send_sync::<JwtKeys>();
    }

    #[test]
    fn issued_token_verifies_with_the_claims_as_passed() {
        let keys = keys();
        assert_eq!(keys.ttl_secs(), TTL_SECS);
        // (user ID, type, the `usr_type` claim on the wire)
        for (user_id, usr_type, claim) in [
            ("ADMIN001", UserType::Admin, "A"),
            ("USER0001", UserType::User, "U"),
            ("user0001", UserType::User, "U"),
        ] {
            let before = now();
            let issued = keys.issue(user_id, usr_type).expect("issues");
            let after = now();
            let verified = keys.verify(&issued.token).expect("verifies");
            assert_eq!(verified.sub, user_id);
            assert_eq!(verified.usr_type, usr_type);
            assert_eq!(
                wire_claims(&keys, &issued.token)["usr_type"],
                json!(claim),
                "{usr_type:?}"
            );
            assert!((before..=after).contains(&verified.iat), "{verified:?}");
            assert_eq!(verified.exp - verified.iat, TTL_SECS);
            assert_eq!(
                issued.expires_at.timestamp(),
                i64::try_from(verified.exp).expect("exp fits i64")
            );
            let header = jsonwebtoken::decode_header(&issued.token).expect("a header");
            assert_eq!(header.alg, Algorithm::HS256);
        }
    }

    #[test]
    fn token_signed_with_another_secret_is_rejected() {
        let token = issue(
            &JwtKeys::new("other", TTL_SECS),
            "ADMIN001",
            UserType::Admin,
        );
        assert_eq!(
            rejection(&keys(), &token).kind(),
            &ErrorKind::InvalidSignature
        );
    }

    #[test]
    fn tampered_token_is_rejected() {
        let keys = keys();
        let admin_token = issue(&keys, "ADMIN001", UserType::Admin);
        let user_token = issue(&keys, "ADMIN001", UserType::User);

        // The admin claims under the signature of the user claims.
        let (admin_message, _) = split_signature(&admin_token);
        let (user_message, user_signature) = split_signature(&user_token);
        let spliced = format!("{admin_message}.{user_signature}");

        // One character changed in the middle of the signature. The last
        // character is avoided: its low bits are padding and may decode to the
        // same bytes.
        let middle = user_signature.len() / 2;
        let replacement = if user_signature[middle..].starts_with('A') {
            'B'
        } else {
            'A'
        };
        let flipped = format!(
            "{user_message}.{}{replacement}{}",
            &user_signature[..middle],
            &user_signature[middle + 1..]
        );

        for token in [spliced, flipped] {
            assert_ne!(token, user_token);
            assert_eq!(
                rejection(&keys, &token).kind(),
                &ErrorKind::InvalidSignature,
                "{token}"
            );
        }
    }

    #[test]
    fn token_with_another_algorithm_is_rejected() {
        let keys = keys();
        let now = now();
        for algorithm in [Algorithm::HS384, Algorithm::HS512] {
            let token = sign(&keys, algorithm, &claims(now, now + 60));
            assert_eq!(
                rejection(&keys, &token).kind(),
                &ErrorKind::InvalidAlgorithm,
                "{algorithm:?}"
            );
        }
    }

    #[test]
    fn unsigned_alg_none_token_is_rejected() {
        let keys = keys();
        let token = issue(&keys, "ADMIN001", UserType::Admin);
        let mut segments = token.split('.');
        let payload = segments.nth(1).expect("a payload segment");
        let signature = segments.next().expect("a signature segment");
        for unsigned in [
            format!("{NONE_HEADER}.{payload}."),
            format!("{NONE_HEADER}.{payload}.{signature}"),
        ] {
            assert!(keys.verify(&unsigned).is_err(), "{unsigned}");
        }
    }

    #[test]
    fn expired_token_is_rejected_without_leeway() {
        let keys = keys();
        let now = now();
        for age in [1, 10] {
            let exp = now - age;
            let token = sign(&keys, Algorithm::HS256, &claims(exp - TTL_SECS, exp));
            assert_eq!(
                rejection(&keys, &token).kind(),
                &ErrorKind::ExpiredSignature,
                "expired {age}s ago"
            );
        }
        let current = sign(&keys, Algorithm::HS256, &claims(now, now + 60));
        assert_eq!(
            keys.verify(&current).expect("verifies"),
            claims(now, now + 60)
        );
    }

    #[test]
    fn token_missing_a_claim_is_rejected() {
        let keys = keys();
        let now = now();
        let incomplete = [
            json!({"usr_type": "A", "iat": now, "exp": now + 60}),
            json!({"sub": "ADMIN001", "usr_type": "A", "iat": now}),
            json!({"sub": "ADMIN001", "iat": now, "exp": now + 60}),
            json!({"sub": "ADMIN001", "usr_type": "A", "exp": now + 60}),
            json!({"sub": "ADMIN001", "usr_type": "A", "iat": now, "exp": "never"}),
        ];
        for payload in incomplete {
            let token = sign(&keys, Algorithm::HS256, &payload);
            assert!(keys.verify(&token).is_err(), "{payload}");
        }
    }

    #[test]
    fn token_with_a_role_claim_outside_a_and_u_is_rejected() {
        let keys = keys();
        let now = now();
        let payload = |role: &serde_json::Value| {
            let mut claims = json!({"sub": "ADMIN001", "iat": now, "exp": now + 60});
            claims["usr_type"] = role.clone();
            claims
        };
        // Correctly signed with the right secret: only the role is wrong. The
        // map, array and other non-string shapes include the externally
        // tagged enum forms a derived decoder would read as `A` or `U`.
        let outside = [
            json!("X"),
            json!("a"),
            json!("u"),
            json!(""),
            json!("AA"),
            json!("A "),
            json!(" A"),
            json!(null),
            json!(1),
            json!(true),
            json!({"A": null}),
            json!({"U": null}),
            json!({"A": {}}),
            json!(["A"]),
        ];
        for role in outside {
            let token = sign(&keys, Algorithm::HS256, &payload(&role));
            let err = rejection(&keys, &token);
            assert!(matches!(err.kind(), ErrorKind::Json(_)), "{role}");
            assert_eq!(err.reason(), "invalid_claims", "{role}");
        }
        for (role, usr_type) in [("A", UserType::Admin), ("U", UserType::User)] {
            let token = sign(&keys, Algorithm::HS256, &payload(&json!(role)));
            assert_eq!(
                keys.verify(&token).expect("verifies").usr_type,
                usr_type,
                "{role}"
            );
        }
    }

    #[test]
    fn malformed_token_is_rejected() {
        let keys = keys();
        for token in ["", "not-a-token", "a.b", "a.b.c", "..", "Bearer a.b.c"] {
            assert!(keys.verify(token).is_err(), "{token:?}");
        }
    }

    #[test]
    fn rejection_text_never_quotes_the_token() {
        let token = issue(
            &JwtKeys::new("other", TTL_SECS),
            "ADMIN001",
            UserType::Admin,
        );
        let err = rejection(&keys(), &token);
        let (message, signature) = split_signature(&token);
        for text in [err.to_string(), format!("{err:?}")] {
            assert!(!text.contains(signature), "{text}");
            assert!(!text.contains(message), "{text}");
        }
        assert!(err.to_string().starts_with("token rejected: "));
    }

    #[test]
    fn rejection_reason_is_a_fixed_category() {
        let keys = keys();
        let now = now();
        let current = claims(now, now + 60);
        let issued = issue(&keys, "ADMIN001", UserType::Admin);
        let mut segments = issued.split('.').skip(1);
        let payload = segments.next().expect("a payload segment");
        let signature = segments.next().expect("a signature segment");
        let other_secret = issue(
            &JwtKeys::new("other", TTL_SECS),
            "ADMIN001",
            UserType::Admin,
        );
        let missing_sub = json!({"usr_type": "A", "iat": now, "exp": now + 60});
        let role_x = json!({"sub": "ADMIN001", "usr_type": "X", "iat": now, "exp": now + 60});
        // (case, token, reason)
        let cases = [
            ("another secret", other_secret, "bad_signature"),
            (
                "HS384",
                sign(&keys, Algorithm::HS384, &current),
                "wrong_algorithm",
            ),
            (
                "expired",
                sign(&keys, Algorithm::HS256, &claims(now - TTL_SECS, now - 1)),
                "expired",
            ),
            (
                "missing sub",
                sign(&keys, Algorithm::HS256, &missing_sub),
                "invalid_claims",
            ),
            (
                "role X",
                sign(&keys, Algorithm::HS256, &role_x),
                "invalid_claims",
            ),
            ("garbage", "not-a-token".to_owned(), "malformed"),
            ("garbage segments", "a.b.c".to_owned(), "malformed"),
            ("alg none", format!("{NONE_HEADER}.{payload}."), "malformed"),
            (
                "unknown alg",
                format!("{UNKNOWN_ALG_HEADER}.{payload}.{signature}"),
                "malformed",
            ),
        ];
        for (case, token, reason) in cases {
            let err = rejection(&keys, &token);
            assert_eq!(err.reason(), reason, "{case}: {:?}", err.kind());
            assert_eq!(
                err.to_string(),
                format!("token rejected: {reason}"),
                "{case}"
            );
        }
    }

    #[test]
    fn expiry_that_cannot_be_represented_is_a_500() {
        let beyond_i64 = u64::try_from(i64::MAX).expect("i64::MAX fits u64");
        // Fits an `i64` but lies past the last instant `chrono` represents.
        let beyond_chrono = 10_u64.pow(16);
        for ttl_secs in [u64::MAX, beyond_i64, beyond_chrono] {
            let err = JwtKeys::new("s", ttl_secs)
                .issue("U1", UserType::User)
                .expect_err("the expiry cannot be represented");
            assert_eq!(err.body(), ApiError::unexpected().body(), "{ttl_secs}");
            assert_eq!(
                err.into_response().status(),
                StatusCode::INTERNAL_SERVER_ERROR,
                "{ttl_secs}"
            );
        }
    }

    #[test]
    fn startup_check_agrees_with_issue_on_which_lifetimes_work() {
        let beyond_i64 = u64::try_from(i64::MAX).expect("i64::MAX fits u64");
        for (ttl_secs, representable) in [
            (1, true),
            (TTL_SECS, true),
            (10_u64.pow(11), true),
            (10_u64.pow(16), false),
            (beyond_i64, false),
            (u64::MAX, false),
        ] {
            let keys = JwtKeys::new("s", ttl_secs);
            assert_eq!(keys.expiry_is_representable(), representable, "{ttl_secs}");
            assert_eq!(
                keys.issue("U1", UserType::User).is_ok(),
                representable,
                "{ttl_secs}"
            );
        }
    }

    #[test]
    fn expiry_is_none_exactly_past_the_last_representable_instant() {
        let last =
            u64::try_from(DateTime::<Utc>::MAX_UTC.timestamp()).expect("a positive timestamp");
        assert_eq!(expiry(0, 0), Some((0, DateTime::<Utc>::UNIX_EPOCH)));
        assert_eq!(
            expiry(1, last - 1).map(|(exp, at)| (exp, at.timestamp())),
            Some((last, DateTime::<Utc>::MAX_UTC.timestamp()))
        );
        for (iat, ttl_secs) in [
            (1, last),
            (0, last + 1),
            (1, u64::try_from(i64::MAX).expect("i64::MAX fits u64")),
            (u64::MAX, 1),
        ] {
            assert_eq!(expiry(iat, ttl_secs), None, "{iat} + {ttl_secs}");
        }
    }

    #[test]
    fn only_code_a_is_admin() {
        assert_eq!(UserType::from_code("A"), UserType::Admin);
        assert!(UserType::Admin.is_admin());
        for code in ["U", "", "a", "X", "A ", " A", "AA"] {
            assert_eq!(UserType::from_code(code), UserType::User, "{code:?}");
        }
        assert!(!UserType::User.is_admin());
        assert_eq!(UserType::Admin.as_code(), "A");
        assert_eq!(UserType::User.as_code(), "U");
        for user_type in [UserType::Admin, UserType::User] {
            assert_eq!(UserType::from_code(user_type.as_code()), user_type);
        }
    }

    #[test]
    fn user_type_is_encoded_and_decoded_as_the_string_a_or_u_only() {
        for (user_type, code) in [(UserType::Admin, "A"), (UserType::User, "U")] {
            assert_eq!(
                serde_json::to_value(user_type).expect("serializes"),
                json!(code)
            );
            assert_eq!(
                serde_json::from_value::<UserType>(json!(code)).expect("decodes"),
                user_type
            );
        }
        let refused = [
            json!("X"),
            json!("a"),
            json!("u"),
            json!(""),
            json!("A "),
            json!(null),
            json!(1),
            json!(true),
            json!({"A": null}),
            json!({"U": null}),
            json!({"A": {}}),
            json!(["A"]),
        ];
        for value in refused {
            assert!(
                serde_json::from_value::<UserType>(value.clone()).is_err(),
                "{value}"
            );
            assert!(
                serde_json::from_str::<UserType>(&value.to_string()).is_err(),
                "{value}"
            );
        }
    }

    #[test]
    fn debug_output_redacts_the_secret_and_the_token() {
        let keys = JwtKeys::new("super-secret-value", TTL_SECS);
        let shown = format!("{keys:?}");
        assert!(!shown.contains("super-secret-value"), "{shown}");
        assert_eq!(shown, "JwtKeys { algorithm: HS256, ttl_secs: 3600, .. }");

        let issued = keys.issue("ADMIN001", UserType::Admin).expect("issues");
        let shown = format!("{issued:?}");
        assert!(!shown.contains(&issued.token), "{shown}");
        let (_, signature) = split_signature(&issued.token);
        assert!(!shown.contains(signature), "{shown}");
        assert!(shown.contains("<redacted>"), "{shown}");
    }
}
