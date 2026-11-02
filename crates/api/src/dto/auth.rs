//! CC00 sign-on and logout: the wire types of `POST /auth/login` and
//! `POST /auth/logout`, replacing the `COSGN00` map (`COSGN0AI`/`COSGN0AO`
//! [app/cpy-bms/COSGN00.CPY]) that `COSGN00C` sends and receives.
//!
//! [`LoginRequest`] is the only request type of the API that carries
//! credentials (AAP 0.6.1). It holds the two map fields as typed; the
//! upper-casing and padding of `COSGN00C` (E-20) belong to
//! `carddemo_domain::rules::signon` and `carddemo_domain::auth`, never to the
//! wire type. [`LoginResponse`] carries the token and what the SPA needs to
//! navigate; the server decides access from the token's claims alone.

use std::fmt;

use carddemo_domain::rules::signon::{Landing, SignonInput, SignonOutcome};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

use crate::dto::{BmsField, id_string, reject_invalid, text};
use crate::error::ApiError;

/// The `message` of a sign-on request whose fields exceed their map lengths.
///
/// The `COSGN00C` section of `docs/parity/validation-audit.md` has no
/// over-length row: a 3270 field never holds more than its `PIC X(8)`, so the
/// COBOL has no text for the case, and the response names only the fields.
const OVER_LENGTH_MESSAGE: &str = "";

/// The text shown in place of a credential by the `Debug` implementations.
const REDACTED: &str = "***";

/// The SPA route of the admin menu `COADM01C`.
const ADMIN_HOME_ROUTE: &str = "/admin";

/// The SPA route of the main menu `COMEN01C`.
const USER_HOME_ROUTE: &str = "/";

/// The body of `POST /auth/login`: the `USERIDI` and `PASSWDI` fields of the
/// sign-on map, as typed.
///
/// Each field is limited to the 8 characters of its `PIC X(8)` map field
/// [app/cpy-bms/COSGN00.CPY:L72, L78]. An absent field is the empty field the
/// COBOL receives as spaces, which the sign-on rule rejects with its audited
/// message. `Debug` never shows the password.
#[derive(Clone, Default, Deserialize, ToSchema, Validate)]
pub struct LoginRequest {
    /// `USERIDI`: the user ID as typed, case unchanged. Sign-on upper-cases
    /// it before the user is read.
    #[serde(default)]
    #[schema(max_length = 8)]
    #[validate(length(max = 8))]
    pub user_id: Option<String>,
    /// `PASSWDI`: the password as typed, case unchanged. Sign-on upper-cases
    /// it before it is verified.
    #[serde(default)]
    #[schema(max_length = 8)]
    #[validate(length(max = 8))]
    pub password: Option<String>,
}

impl fmt::Debug for LoginRequest {
    /// The user ID, and `***` in place of a password that is present.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginRequest")
            .field("user_id", &self.user_id)
            .field("password", &self.password.as_ref().map(|_| REDACTED))
            .finish()
    }
}

impl LoginRequest {
    /// COSGN00C PROCESS-ENTER-KEY — the map-field lengths ahead of
    /// V-COSGN00C-001 and V-COSGN00C-002.
    ///
    /// The one edit the request applies itself: `user_id` and `password` hold
    /// at most the 8 characters of `USERIDI` and `PASSWDI`
    /// [app/cpy-bms/COSGN00.CPY:L72, L78]. Blank fields pass here and reach
    /// the sign-on rule, which answers them with V-COSGN00C-001 and
    /// V-COSGN00C-002.
    ///
    /// # Errors
    ///
    /// A 400 [`ApiError`] of kind `Invalid` with an empty `message` (the audit
    /// records no over-length text for `COSGN00C`), listing `USERID` and then
    /// `PASSWD` for each field that is too long.
    pub fn check(&self) -> Result<(), ApiError> {
        self.validate()
            .map_err(|errors| reject_invalid(&errors, LoginField::from_json, OVER_LENGTH_MESSAGE))
    }

    /// COSGN00C PROCESS-ENTER-KEY — V-COSGN00C-001, V-COSGN00C-002: the map
    /// fields as `EXEC CICS RECEIVE MAP('COSGN0A')` delivers them
    /// [app/cbl/COSGN00C.cbl:L110-115].
    ///
    /// Each value passes as typed, without trimming or a case change; an
    /// absent field becomes the empty text, which the rule reads as
    /// `SPACES`.
    #[must_use]
    pub fn into_input(self) -> SignonInput {
        SignonInput {
            user_id: self.user_id.unwrap_or_default(),
            password: self.password.unwrap_or_default(),
        }
    }
}

/// The body of a successful `POST /auth/login`: the bearer token and the
/// `COMMAREA` fields `COSGN00C` sets before its `XCTL`
/// [app/cbl/COSGN00C.cbl:L224-240].
///
/// `Debug` never shows the token.
#[derive(Clone, Serialize, ToSchema)]
pub struct LoginResponse {
    /// The HS256 bearer token the authenticated endpoints require.
    pub token: String,
    /// The instant the token expires, RFC 3339.
    pub expires_at: DateTime<Utc>,
    /// `CDEMO-USER-ID`: the signed-on user ID, upper-cased as sign-on reads
    /// it, without trailing spaces.
    pub user_id: String,
    /// `CDEMO-USER-TYPE` [app/cpy/COCOM01Y.cpy:L26-28]: `A` for an
    /// administrator, `U` for a user. For navigation only: the server decides
    /// access from the token's claims, never from this value.
    pub usr_type: String,
    /// The SPA route the user type selects: `/admin` for the admin menu
    /// `COADM01C`, `/` for the main menu `COMEN01C`.
    #[schema(example = "/admin")]
    pub landing: String,
}

impl fmt::Debug for LoginResponse {
    /// Every field, with `***` in place of the token.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LoginResponse")
            .field("token", &REDACTED)
            .field("expires_at", &self.expires_at)
            .field("user_id", &self.user_id)
            .field("usr_type", &self.usr_type)
            .field("landing", &self.landing)
            .finish()
    }
}

impl LoginResponse {
    /// COSGN00C READ-USER-SEC-FILE — V-COSGN00C-005 passed: the response to a
    /// sign-on whose password matched, carrying `token` and its expiry.
    ///
    /// The landing is the `XCTL` target of `IF CDEMO-USRTYP-ADMIN`
    /// [app/cbl/COSGN00C.cbl:L230-240].
    #[must_use]
    pub fn new(token: String, expires_at: DateTime<Utc>, outcome: &SignonOutcome) -> Self {
        Self {
            token,
            expires_at,
            user_id: id_string(&outcome.user_id),
            usr_type: text(&outcome.usr_type),
            landing: landing_route(outcome.landing).to_owned(),
        }
    }
}

/// The SPA route of a [`Landing`].
const fn landing_route(landing: Landing) -> &'static str {
    match landing {
        Landing::AdminHome => ADMIN_HOME_ROUTE,
        Landing::UserHome => USER_HOME_ROUTE,
    }
}

/// A response that carries only a message, such as the logout text
/// `CCDA-MSG-THANK-YOU` [app/cpy/CSMSG01Y.cpy:L18-19].
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct MessageResponse {
    /// The message text, byte-identical to the COBOL literal, trailing spaces
    /// included.
    pub message: String,
}

impl MessageResponse {
    /// A response carrying `message` unchanged: never trimmed or padded.
    #[must_use]
    pub fn new(message: &str) -> Self {
        Self {
            message: message.to_owned(),
        }
    }
}

/// The BMS fields of the `COSGN00` map that a sign-on error names: the
/// `fields[].field` values of `POST /auth/login` errors, one per distinct
/// `BMS field` of the `COSGN00C` rows of `docs/parity/validation-audit.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum LoginField {
    /// `USERID` [app/cpy-bms/COSGN00.CPY:L67-72]: V-COSGN00C-001,
    /// V-COSGN00C-003, V-COSGN00C-004.
    #[serde(rename = "USERID")]
    UserId,
    /// `PASSWD` [app/cpy-bms/COSGN00.CPY:L73-78]: V-COSGN00C-002,
    /// V-COSGN00C-005.
    #[serde(rename = "PASSWD")]
    Passwd,
}

impl LoginField {
    /// The BMS field name, as the error body carries it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserId => "USERID",
            Self::Passwd => "PASSWD",
        }
    }

    /// The BMS field of a [`LoginRequest`] field, by its JSON name.
    fn from_json(name: &str) -> Option<Self> {
        match name {
            "user_id" => Some(Self::UserId),
            "password" => Some(Self::Passwd),
            _ => None,
        }
    }
}

impl BmsField for LoginField {
    fn as_str(self) -> &'static str {
        Self::as_str(self)
    }

    /// The lowest audit `Order` of the field's rows: `USERID` is tested first
    /// (V-COSGN00C-001), `PASSWD` second (V-COSGN00C-002).
    fn rank(self) -> u16 {
        match self {
            Self::UserId => 1,
            Self::Passwd => 2,
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use carddemo_domain::error::ErrorKind;
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{Alnum, UserId};
    use carddemo_domain::rules::signon::{Landing, SignonOutcome};
    use chrono::{DateTime, Utc};
    use serde_json::{Value, json};
    use utoipa::PartialSchema;

    use super::{LoginField, LoginRequest, LoginResponse, MessageResponse};
    use crate::dto::BmsField;

    const PASSWORD: &str = "NOTREAL1";
    const TOKEN: &str = "fake-bearer-token-for-tests";

    fn request(user_id: Option<&str>, password: Option<&str>) -> LoginRequest {
        LoginRequest {
            user_id: user_id.map(str::to_owned),
            password: password.map(str::to_owned),
        }
    }

    fn expiry() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).expect("a valid instant")
    }

    fn outcome(user_id: &str, usr_type: &str, landing: Landing) -> SignonOutcome {
        SignonOutcome {
            user_id: UserId::move_from(user_id),
            usr_type: Alnum::move_from(usr_type),
            landing,
        }
    }

    fn json_of(value: &impl serde::Serialize) -> Value {
        serde_json::to_value(value).expect("serializes")
    }

    #[test]
    fn check_rejects_fields_over_their_map_length_in_audit_order() {
        let nine = "ABCDEFGHI";
        // (user_id, password, expected BMS fields)
        let cases: [(Option<&str>, Option<&str>, &[&str]); 3] = [
            (Some(nine), Some("PASSWORD"), &["USERID"]),
            (Some("ADMIN001"), Some(nine), &["PASSWD"]),
            (Some(nine), Some(nine), &["USERID", "PASSWD"]),
        ];
        for (user_id, password, fields) in cases {
            let error = request(user_id, password)
                .check()
                .expect_err("a field is longer than 8");
            assert_eq!(error.status(), StatusCode::BAD_REQUEST);
            let body = error.body();
            assert_eq!(body.code, ErrorKind::Invalid.code());
            assert_eq!(body.message, "");
            let names: Vec<&str> = body
                .fields
                .iter()
                .map(|entry| entry.field.as_str())
                .collect();
            assert_eq!(names, fields, "{user_id:?} {password:?}");
            assert!(body.fields.iter().all(|entry| entry.message.is_empty()));
        }
    }

    #[test]
    fn check_accepts_fields_within_their_map_length() {
        let cases = [
            request(Some("ADMIN001"), Some("PASSWORD")),
            request(Some("user0001"), Some("pass")),
            request(Some(""), Some("")),
            request(Some("        "), None),
            request(None, None),
            // Eight characters of two bytes each: the limit counts characters.
            request(Some("éééééééé"), Some("PASSWORD")),
        ];
        for case in cases {
            assert!(case.check().is_ok(), "{case:?}");
        }
    }

    #[test]
    fn deserializes_absent_fields_as_none() {
        let empty: LoginRequest = serde_json::from_value(json!({})).expect("deserializes");
        assert_eq!(empty.user_id, None);
        assert_eq!(empty.password, None);

        let full: LoginRequest =
            serde_json::from_value(json!({"user_id": "admin001", "password": "pass1234"}))
                .expect("deserializes");
        assert_eq!(full.user_id.as_deref(), Some("admin001"));
        assert_eq!(full.password.as_deref(), Some("pass1234"));
    }

    #[test]
    fn into_input_keeps_the_values_as_typed() {
        let input = request(Some("user0001"), Some(" Pass1 ")).into_input();
        assert_eq!(input.user_id, "user0001");
        assert_eq!(input.password, " Pass1 ");

        let absent = request(None, None).into_input();
        assert_eq!(absent.user_id, "");
        assert_eq!(absent.password, "");
    }

    #[test]
    fn login_response_lands_by_user_type() {
        // (user ID stored, user type, landing, expected route)
        let cases = [
            ("ADMIN001", "A", Landing::AdminHome, "/admin"),
            ("USER1", "U", Landing::UserHome, "/"),
        ];
        for (user_id, usr_type, landing, route) in cases {
            let response = LoginResponse::new(
                TOKEN.to_owned(),
                expiry(),
                &outcome(user_id, usr_type, landing),
            );
            assert_eq!(
                json_of(&response),
                json!({
                    "token": TOKEN,
                    "expires_at": "2023-11-14T22:13:20Z",
                    "user_id": user_id,
                    "usr_type": usr_type,
                    "landing": route,
                })
            );
        }
    }

    #[test]
    fn message_response_keeps_the_literal_bytes() {
        // The constant is the 49-byte `VALUE` literal, unpadded to its `X(50)`
        // field; the response neither trims nor pads it.
        let response = MessageResponse::new(messages::CCDA_MSG_THANK_YOU);
        let wire = serde_json::to_string(&response).expect("serializes");
        let back: MessageResponse = serde_json::from_str(&wire).expect("deserializes");
        assert_eq!(back.message, messages::CCDA_MSG_THANK_YOU);
        assert_eq!(back.message.len(), messages::CCDA_MSG_THANK_YOU.len());
        assert!(back.message.ends_with("...      "));
    }

    #[test]
    fn debug_shows_no_credential() {
        let login = format!("{:?}", request(Some("ADMIN001"), Some(PASSWORD)));
        assert!(!login.contains(PASSWORD), "{login}");
        assert!(
            login.contains("ADMIN001") && login.contains("***"),
            "{login}"
        );
        assert!(format!("{:?}", request(None, None)).contains("password: None"));

        let response = LoginResponse::new(
            TOKEN.to_owned(),
            expiry(),
            &outcome("ADMIN001", "A", Landing::AdminHome),
        );
        let shown = format!("{response:?}");
        assert!(!shown.contains(TOKEN), "{shown}");
        assert!(shown.contains("***") && shown.contains("/admin"), "{shown}");
    }

    #[test]
    fn login_field_names_and_ranks_follow_the_audit() {
        // (variant, BMS field, rank)
        let cases = [
            (LoginField::UserId, "USERID", 1),
            (LoginField::Passwd, "PASSWD", 2),
        ];
        for (field, name, rank) in cases {
            assert_eq!(field.as_str(), name);
            assert_eq!(BmsField::as_str(field), name);
            assert_eq!(field.rank(), rank);
            assert_eq!(json_of(&field), json!(name));
            let back: LoginField = serde_json::from_value(json!(name)).expect("deserializes");
            assert_eq!(back, field);
        }
        assert_eq!(LoginField::from_json("user_id"), Some(LoginField::UserId));
        assert_eq!(LoginField::from_json("password"), Some(LoginField::Passwd));
        assert_eq!(LoginField::from_json("usr_type"), None);
    }

    #[test]
    fn schemas_carry_the_map_lengths_and_field_names() {
        let request = json_of(&LoginRequest::schema());
        for property in ["user_id", "password"] {
            assert_eq!(
                request["properties"][property]["maxLength"],
                json!(8),
                "{request}"
            );
        }
        assert!(request.get("required").is_none(), "{request}");

        let fields = json_of(&LoginField::schema());
        assert_eq!(fields["enum"], json!(["USERID", "PASSWD"]), "{fields}");

        let response = json_of(&LoginResponse::schema());
        assert_eq!(
            response["properties"]["expires_at"]["format"],
            json!("date-time")
        );
        assert_eq!(
            response["required"],
            json!(["token", "expires_at", "user_id", "usr_type", "landing"])
        );
    }
}
