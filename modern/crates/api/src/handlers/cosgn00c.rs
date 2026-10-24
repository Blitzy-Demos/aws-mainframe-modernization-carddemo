//! COSGN00C, sign-on: the handler of endpoint 1, `POST /api/v1/signon` (transaction `CC00`, map
//! `COSGN0A`; AAP 0.3.4.1, 0.3.4.6).
//!
//! ENTER is the only key the endpoint serves (COSGN00C-003). F3, which replaces the screen with
//! the session-end text, and every other key, which the program answers with its invalid-key
//! message, send no request: they are `client` rows of `modern/audit/online/COSGN00C.md`
//! (COSGN00C-004, COSGN00C-005).
//!
//! The program keeps the signed-on identity in the COMMAREA it passes on its `XCTL`. Here the
//! bearer token the service issues carries it instead, as the claims `sub` (the upper-cased user
//! ID) and `usr_type` (the stored user type), and every other endpoint trusts only that verified
//! token (COSGN00C-014, AAP 0.5.1). The `XCTL` target becomes `next_route`: `/admin` (COADM01C)
//! for user type `A`, `/menu` (COMEN01C) for every other type (COSGN00C-015, COSGN00C-016).
//!
//! This is the one operation that takes no token, so it declares no `security` requirement and
//! extracts no identity: the user ID and password of the body are input to the program's edits,
//! never an identity the request asserts.
//!
//! The handler extracts, delegates and maps (Thin Request Handlers). The edits, the read, the
//! password check and the token live in `services::cosgn00c::signon` and the domain rules it
//! applies; each failure's message, field and status come from the `From` conversions of
//! `crate::error`. The request holds a password, so the handler never logs or formats it
//! (Cardholder Data Redaction, AAP 0.3.4.8).

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::dto::ValidJson;
use crate::dto::cosgn00c::{SignonRequest, SignonResponse};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`signon`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct SignonOperation;

/// Sign on (CC00 / COSGN00C)
///
/// ENTER on the sign-on screen: validates the user ID and the password, upper-cases both as the
/// program does, reads the user by the upper-cased ID, verifies the password against the stored
/// hash and returns a bearer token for every other operation, with the user ID, the user type
/// and the route of the menu the program transfers to. F3 and every other key are handled by the
/// screen and send no request.
///
/// # Errors
///
/// - 400: the body is not a sign-on request, a field holds more than its 8 screen positions or a
///   character no screen field carries, or the user ID or the password is blank.
/// - 401: no user has the user ID, or the password does not match the stored one.
/// - 500: the user could not be read or verified, or the token could not be issued.
/// - 503, with `Retry-After` in seconds: the server's password verification was saturated, so
///   the credentials were not checked; the same request may be sent again after that delay.
#[utoipa::path(
    impl_for = SignonOperation,
    post,
    path = "/api/v1/signon",
    operation_id = "cosgn00c_signon",
    tag = "COSGN00C",
    request_body = SignonRequest,
    responses(
        (
            status = 200,
            description = "Signed on: the bearer token, the user ID and type, and the next route",
            body = SignonResponse
        ),
        (
            status = 400,
            description = "A malformed body, an over-long or unprintable field, or a blank field",
            body = ScreenError
        ),
        (
            status = 401,
            description = "No user with this user ID, or a password that does not match",
            body = ScreenError
        ),
        (
            status = 500,
            description = "The user could not be read or verified, or no token could be issued",
            body = ScreenError
        ),
        (
            status = 503,
            description = "Password verification is saturated; the credentials were not checked",
            body = ScreenError,
            headers(
                (
                    "Retry-After" = u64,
                    description = "Seconds to wait before sending the request again"
                )
            )
        ),
    ),
)]
pub(crate) async fn signon(
    State(state): State<AppState>,
    ValidJson(request): ValidJson<SignonRequest>,
) -> Result<Json<SignonResponse>, ScreenError> {
    services::cosgn00c::signon(&state, request)
        .await
        .map(SignonResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}

// These tests pin the emitted operation only. Building an `AppState` needs a store, and the
// handlers folder never names the storage crate (Thin Request Handlers scan), so the endpoint's
// behaviour over a database is proven by the integration suites in `crates/api/tests`.
#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use serde_json::{Map, Value, json};
    use utoipa_axum::router::OpenApiRouter;

    use crate::app::{AppState, operation};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The operation's path item, under its JSON-pointer form.
    const PATH_ITEM: &str = "/paths/~1api~1v1~1signon";

    /// The document `crate::app::operation` builds from this handler alone, as
    /// `api::app::router()` registers it.
    fn document() -> Result<Value, serde_json::Error> {
        let (_, doc) = OpenApiRouter::<AppState>::new()
            .routes(operation(super::SignonOperation, super::signon))
            .split_for_parts();
        serde_json::to_value(doc)
    }

    /// The object at `pointer` in `doc`.
    fn object<'d>(doc: &'d Value, pointer: &str) -> Result<&'d Map<String, Value>, String> {
        doc.pointer(pointer)
            .and_then(Value::as_object)
            .ok_or_else(|| format!("no object at {pointer}"))
    }

    #[test]
    fn the_operation_is_the_one_unauthenticated_post() -> TestResult {
        let doc = document()?;
        let paths: Vec<&String> = object(&doc, "/paths")?.keys().collect();
        assert_eq!(paths, ["/api/v1/signon"]);
        let methods: Vec<&String> = object(&doc, PATH_ITEM)?.keys().collect();
        assert_eq!(methods, ["post"]);

        let operation = object(&doc, &format!("{PATH_ITEM}/post"))?;
        assert_eq!(
            operation.get("operationId"),
            Some(&json!("cosgn00c_signon"))
        );
        assert_eq!(operation.get("tags"), Some(&json!(["COSGN00C"])));
        assert_eq!(
            operation.get("summary"),
            Some(&json!("Sign on (CC00 / COSGN00C)"))
        );
        // Sign-on issues the token, so it is the only operation without a requirement.
        assert_eq!(operation.get("security"), None);
        assert_eq!(operation.get("parameters"), None);
        Ok(())
    }

    #[test]
    fn the_body_and_responses_are_the_named_schemas_of_the_contract() -> TestResult {
        let doc = document()?;
        let operation = format!("{PATH_ITEM}/post");
        assert_eq!(
            doc.pointer(&format!(
                "{operation}/requestBody/content/application~1json/schema/$ref"
            )),
            Some(&json!("#/components/schemas/SignonRequest"))
        );

        let responses = object(&doc, &format!("{operation}/responses"))?;
        let statuses: Vec<&String> = responses.keys().collect();
        assert_eq!(statuses, ["200", "400", "401", "500", "503"]);
        for (status, schema) in [
            ("200", "SignonResponse"),
            ("400", "ScreenError"),
            ("401", "ScreenError"),
            ("500", "ScreenError"),
            ("503", "ScreenError"),
        ] {
            assert_eq!(
                doc.pointer(&format!(
                    "{operation}/responses/{status}/content/application~1json/schema/$ref"
                )),
                Some(&json!(format!("#/components/schemas/{schema}"))),
                "{status}"
            );
        }
        // The saturation answer names one header: the delay before the request is sent again.
        let headers: Vec<&String> = object(&doc, &format!("{operation}/responses/503/headers"))?
            .keys()
            .collect();
        assert_eq!(headers, ["Retry-After"]);
        assert_eq!(
            doc.pointer(&format!(
                "{operation}/responses/503/headers/Retry-After/schema/type"
            )),
            Some(&json!("integer"))
        );

        let schemas = object(&doc, "/components/schemas")?;
        for name in ["SignonRequest", "SignonResponse"] {
            assert!(schemas.contains_key(name), "{name} is not registered");
        }
        assert_eq!(
            doc.pointer("/components/schemas/SignonRequest/properties/password/writeOnly"),
            Some(&json!(true))
        );
        Ok(())
    }
}
