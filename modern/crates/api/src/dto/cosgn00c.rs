//! COSGN00C, sign-on: the contract of endpoint 1, `POST /api/v1/signon` (transaction `CC00`, map
//! `COSGN0A`; AAP 0.3.4.1, 0.3.4.6). ENTER is the only key it serves; F3 sends no request.
//!
//! [`SignonRequest`] carries the two map inputs exactly as typed, `USERIDI` and `PASSWDI`, each
//! `LENGTH=8` (COSGN00-027, COSGN00-031). Missing members default to empty text, so a blank or
//! omitted field reaches the program's own blank-field edits (COSGN00C-007, COSGN00C-008) rather
//! than a deserialisation error. Nothing here trims, pads or upper-cases: `rules::edit` reads
//! each value as its 8-byte field and upper-cases it (COSGN00C-003, COSGN00C-009, E26).
//!
//! [`SignonResponse`] is the COMMAREA identity the program sets before its `XCTL`, with the
//! token that now carries it (COSGN00C-014 to COSGN00C-016). It is built by one `From`
//! conversion from the token the service issues and the domain's [`rules::SignedOn`], so the
//! handler only extracts, delegates and maps (Thin Request Handlers).
//!
//! # Redaction
//!
//! The password is input only. It is never serialised, the schema marks it `writeOnly` with the
//! example `********`, and the hand-written `Debug` prints that mask in its place. The response
//! `Debug` leaves the bearer token out (Cardholder Data Redaction, AAP 0.3.4.8).

use core::fmt;

use domain::online::cosgn00c as rules;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// What `Debug` and the schema example show for a password: one `*` per map position.
const PASSWORD_MASK: &str = "********";

/// The sign-on request: the user ID and password typed on the sign-on screen.
///
/// Both members are optional on the wire and default to empty text. Each admits at most the 8
/// characters of its map field (COSGN00-027, COSGN00-031); a longer value is rejected with a 400
/// naming the field. The program's edits then reject a value whose 8 characters do not fit 8
/// bytes of printable ASCII, and answer a blank field with its own message.
// The schema name `SignonRequest` is the frontend's `components["schemas"]["SignonRequest"]`
// (Generated API Types Only). `Debug` is written by hand so that the password never prints.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
pub struct SignonRequest {
    /// `USERIDI PIC X(8)`: the user ID as typed, at most 8 characters. Upper-cased by the
    /// program before the read, so `user0001` signs on as `USER0001` (COSGN00C-009).
    #[serde(default)]
    #[schema(example = "ADMIN001")]
    #[validate(length(max = 8))]
    pub user_id: String,
    /// `PASSWDI PIC X(8)`, a dark field: the password as typed, at most 8 characters. Accepted,
    /// never returned.
    // `skip_serializing_if` with an always-true predicate rather than `skip_serializing`: utoipa
    // drops a `skip_serializing` member from the schema altogether, which would leave the
    // generated `SignonRequest` without a password. The predicate keeps the field documented as
    // `writeOnly` while no serialisation ever emits it.
    #[serde(default, skip_serializing_if = "never_serialized")]
    #[schema(write_only, example = "********")]
    #[validate(length(max = 8))]
    pub password: String,
}

/// The serialisation predicate of [`SignonRequest::password`]: always skip it.
const fn never_serialized(_password: &str) -> bool {
    true
}

/// The user ID and the mask; the password itself is never shown.
impl fmt::Debug for SignonRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            user_id,
            password: _,
        } = self;
        f.debug_struct("SignonRequest")
            .field("user_id", user_id)
            .field("password", &PASSWORD_MASK)
            .finish()
    }
}

/// A successful sign-on: the session token, the COMMAREA identity it carries and the screen
/// the program transfers to (COSGN00C-014 to COSGN00C-016).
// The schema name `SignonResponse` is fixed (Generated API Types Only). `Debug` is written by
// hand so that the bearer token never reaches a log.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SignonResponse {
    /// The HS256 bearer token for every other endpoint, with claims `sub` (the user ID),
    /// `usr_type`, `iat` and `exp`.
    #[schema(example = "header.payload.signature")]
    pub token: String,
    /// `CDEMO-USER-ID PIC X(08)`: the upper-cased user ID the user was read by, without its
    /// trailing spaces (COSGN00C-014, COCOM01Y-005).
    #[schema(example = "ADMIN001")]
    pub user_id: String,
    /// `CDEMO-USER-TYPE PIC X(01)`: the stored user type, `A` for an administrator and `U` for
    /// a user, as its single character unchanged (COSGN00C-014, COCOM01Y-006).
    #[schema(example = "A")]
    pub user_type: String,
    /// The route of the screen the program transfers to: `/admin` (COADM01C) for type `A`,
    /// `/menu` (COMEN01C) for every other type (COSGN00C-015, COSGN00C-016).
    #[schema(example = "/admin")]
    pub next_route: String,
}

/// The identity and the route; the token is left out.
impl fmt::Debug for SignonResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignonResponse")
            .field("user_id", &self.user_id)
            .field("user_type", &self.user_type)
            .field("next_route", &self.next_route)
            .finish_non_exhaustive()
    }
}

/// The response from the token `api::auth::jwt::JwtKeys::issue` returned and the
/// [`rules::SignedOn`] it was issued for: a field-for-field mapping.
impl From<(String, rules::SignedOn)> for SignonResponse {
    fn from((token, signed_on): (String, rules::SignedOn)) -> Self {
        Self {
            token,
            user_id: super::text(&signed_on.user_id),
            // The 1-byte type keeps a trailing space, unlike `super::text`: the response carries
            // the stored value unchanged (COSGN00C-016), byte for byte the token's `usr_type`.
            user_type: String::from_utf8_lossy(signed_on.user_type.code().as_bytes()).into_owned(),
            next_route: signed_on.next_route.path().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::body::Body;
    use axum::extract::{FromRequest, Request};
    use axum::http::StatusCode;
    use axum::http::header::CONTENT_TYPE;
    use domain::copybook::PicX;
    use serde_json::{Value, json};
    use utoipa::{PartialSchema, ToSchema};

    use super::{PASSWORD_MASK, SignonRequest, SignonResponse, rules};
    use crate::dto::{FIELD_LENGTH_MESSAGE, ValidJson};
    use crate::error::ScreenError;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A password the serialised and `Debug` forms must never show.
    const TYPED_PASSWORD: &str = "TYPEDPW1";

    /// `body` posted to a `ValidJson<SignonRequest>` extractor.
    async fn extract(body: &str) -> Result<Result<SignonRequest, ScreenError>, axum::http::Error> {
        let request = Request::builder()
            .method("POST")
            .uri("/api/v1/signon")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))?;
        Ok(ValidJson::<SignonRequest>::from_request(request, &())
            .await
            .map(|ValidJson(value)| value))
    }

    /// The JSON form of a component schema.
    fn schema_json<T: PartialSchema>() -> Result<Value, serde_json::Error> {
        serde_json::to_value(T::schema())
    }

    fn signed_on(user_id: [u8; 8], user_type: rules::UserType) -> rules::SignedOn {
        rules::SignedOn {
            next_route: rules::NextRoute::for_user_type(&user_type),
            user_id: PicX::from_array(user_id),
            user_type,
        }
    }

    #[tokio::test]
    async fn missing_and_empty_members_reach_the_program_as_empty_text() -> TestResult {
        for body in [
            "{}",
            r#"{"user_id": ""}"#,
            r#"{"password": ""}"#,
            r#"{"user_id": "", "password": ""}"#,
        ] {
            let request = extract(body)
                .await?
                .map_err(|error| error.message().to_owned())?;
            assert_eq!(request.user_id, "", "{body}");
            assert_eq!(request.password, "", "{body}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn values_are_kept_exactly_as_typed() -> TestResult {
        let request = extract(r#"{"user_id": " user01 ", "password": "pa ss wd"}"#).await?;
        let request = request.map_err(|error| error.message().to_owned())?;
        assert_eq!(request.user_id, " user01 ");
        assert_eq!(request.password, "pa ss wd");
        Ok(())
    }

    #[tokio::test]
    async fn each_member_admits_at_most_its_eight_map_positions() -> TestResult {
        for (body, field) in [
            (
                json!({"user_id": "ABCDEFGHI", "password": "ABCDEFGH"}),
                "user_id",
            ),
            (
                json!({"user_id": "ADMIN001", "password": "ABCDEFGHI"}),
                "password",
            ),
        ] {
            let Err(error) = extract(&body.to_string()).await? else {
                return Err(format!("{field}: nine characters were accepted").into());
            };
            assert_eq!(
                (error.status(), error.message(), error.field()),
                (StatusCode::BAD_REQUEST, FIELD_LENGTH_MESSAGE, Some(field))
            );
        }
        // Eight characters fit, whatever their byte length; the program's edit judges the bytes.
        let eight = extract(r#"{"user_id": "ÄBCDEFGH", "password": "12345678"}"#).await?;
        assert!(eight.is_ok());
        Ok(())
    }

    #[tokio::test]
    async fn the_password_is_never_serialised_or_shown_by_debug() -> TestResult {
        let body = json!({"user_id": "ADMIN001", "password": TYPED_PASSWORD}).to_string();
        let request = extract(&body)
            .await?
            .map_err(|error| error.message().to_owned())?;
        assert_eq!(request.password, TYPED_PASSWORD);

        let serialised = serde_json::to_value(&request)?;
        assert_eq!(serialised, json!({"user_id": "ADMIN001"}));

        let debug = format!("{request:?}");
        assert_eq!(
            debug,
            format!(r#"SignonRequest {{ user_id: "ADMIN001", password: "{PASSWORD_MASK}" }}"#)
        );
        assert!(!debug.contains(TYPED_PASSWORD));
        Ok(())
    }

    #[test]
    fn the_response_maps_the_signed_on_identity_field_for_field() {
        let cases = [
            (
                *b"ADMIN001",
                rules::UserType::ADMIN,
                "ADMIN001",
                "A",
                "/admin",
            ),
            (
                *b"USER0001",
                rules::UserType::USER,
                "USER0001",
                "U",
                "/menu",
            ),
            (*b"USER01  ", rules::UserType::USER, "USER01", "U", "/menu"),
            (
                *b"ODDTYPE1",
                rules::UserType::from_code(PicX::from_array(*b" ")),
                "ODDTYPE1",
                " ",
                "/menu",
            ),
        ];
        for (user_id, user_type, id, code, route) in cases {
            let response = SignonResponse::from((
                "header.payload.sig".to_owned(),
                signed_on(user_id, user_type),
            ));
            assert_eq!(
                response,
                SignonResponse {
                    token: "header.payload.sig".to_owned(),
                    user_id: id.to_owned(),
                    user_type: code.to_owned(),
                    next_route: route.to_owned(),
                }
            );
        }
    }

    #[test]
    fn the_response_debug_leaves_the_token_out() -> TestResult {
        let response = SignonResponse::from((
            "eyTOKENTEXT".to_owned(),
            signed_on(*b"ADMIN001", rules::UserType::ADMIN),
        ));
        let debug = format!("{response:?}");
        assert_eq!(
            debug,
            r#"SignonResponse { user_id: "ADMIN001", user_type: "A", next_route: "/admin", .. }"#
        );
        assert!(!debug.contains("eyTOKENTEXT"));
        assert_eq!(
            serde_json::to_value(&response)?,
            json!({
                "token": "eyTOKENTEXT",
                "user_id": "ADMIN001",
                "user_type": "A",
                "next_route": "/admin"
            })
        );
        Ok(())
    }

    #[test]
    fn the_request_schema_documents_a_write_only_masked_password() -> TestResult {
        assert_eq!(<SignonRequest as ToSchema>::name(), "SignonRequest");
        let schema = schema_json::<SignonRequest>()?;
        let password = &schema["properties"]["password"];
        assert_eq!(password["type"], "string");
        assert_eq!(password["writeOnly"], true);
        assert_eq!(password["example"], PASSWORD_MASK);
        assert_eq!(schema["properties"]["user_id"]["example"], "ADMIN001");
        // Both members default to empty text, so neither is required on the wire.
        assert!(schema.get("required").is_none());
        Ok(())
    }

    #[test]
    fn the_response_schema_requires_every_member() -> TestResult {
        assert_eq!(<SignonResponse as ToSchema>::name(), "SignonResponse");
        let schema = schema_json::<SignonResponse>()?;
        assert_eq!(
            schema["required"],
            json!(["token", "user_id", "user_type", "next_route"])
        );
        assert!(
            schema["properties"]
                .as_object()
                .is_some_and(|properties| properties.values().all(|p| p["writeOnly"].is_null()))
        );
        Ok(())
    }
}
