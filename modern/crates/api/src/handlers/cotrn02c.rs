//! Endpoint 13, `POST /api/v1/transactions`: the transaction add of transaction `CT02`, program
//! COTRN02C, map `COTRN2A` (AAP 0.3.4.1).
//!
//! The handler extracts the verified identity and the validated [`TranAddRequest`], calls
//! `services::cotrn02c::add` and maps what it returns through [`TranAddResult::from`] and
//! [`ScreenError::from`] (Thin Request Handlers). The attention key, the confirmation, the edits,
//! the reads and the write are the service's and the domain's; nothing is inspected here.
//!
//! - The original and processing dates arrive as the raw text of their map fields, because
//!   COTRN02C's own date edits word the messages for them (the raw-edit-input exception of
//!   Typed Dates and Timestamps); they reach the service unchanged.
//! - A typed card number is request input and is neither logged nor echoed. The card the
//!   response carries is the domain's `CardNumber`, which serialises redacted (Cardholder Data
//!   Redaction, `E32`).
//! - `GET` on the same path is `handlers::cotrn00c::list`; `api::app::router()` registers both
//!   on the same path item.

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidJson;
use crate::dto::cotrn02c::{TranAddRequest, TranAddResult};
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`add`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct AddOperation;

/// Add a transaction (CT02 / COTRN02C)
///
/// `ENTER` runs COTRN02C's key edits, cross-reference read and data edits on the request's
/// fields: a blank, `N` or `n` confirmation answers with the confirmation prompt and the edited
/// fields, and `Y` or `y` adds the transaction and answers with its new ID. `PF5` first copies
/// the data fields of the transaction with the highest ID, then runs `ENTER` on them. `F4`
/// (clear) and `PF3` (back) are the screen's own and send no request. The card number in the
/// response is redacted to its last four digits.
///
/// # Errors
///
/// A [`ScreenError`] with the program's message and field:
///
/// - 400: a COTRN02C edit failed, the confirmation is not `Y`, `N` or blank, the key is neither
///   `ENTER` nor `PF5`, or the body is unreadable or exceeds a map field length.
/// - 401: the request carries no valid bearer token; nothing is read.
/// - 404: the account or card has no cross-reference, or `PF5` finds no transaction to copy.
/// - 500: a read or the write failed, or the new transaction ID already exists.
// No 409: COTRN02C answers a duplicate transaction ID with a 500 of its own (`Tran ID already
// exist...`, COTRN02C-050), and its failure classes give 400, 404 and 500 only. No 403: the
// endpoint serves every signed-on user type (COTRN02C-003).
#[utoipa::path(
    impl_for = AddOperation,
    post,
    path = "/api/v1/transactions",
    operation_id = "cotrn02c_add",
    tag = "COTRN02C",
    request_body = TranAddRequest,
    responses(
        (
            status = 200,
            description = "The confirmation prompt with the edited fields, or the added \
                           transaction's ID; the card number is redacted",
            body = TranAddResult
        ),
        (
            status = 400,
            description = "A COTRN02C edit failed, the key is neither ENTER nor PF5, or the body \
                           is unreadable or exceeds a map field length",
            body = ScreenError
        ),
        (
            status = 401,
            description = "No valid bearer token",
            body = ScreenError
        ),
        (
            status = 404,
            description = "The account or card has no cross-reference, or PF5 finds no \
                           transaction to copy",
            body = ScreenError
        ),
        (
            status = 500,
            description = "A read or the write failed, or the new transaction ID already exists",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn add(
    State(state): State<AppState>,
    user: SignedOnUser,
    ValidJson(request): ValidJson<TranAddRequest>,
) -> Result<Json<TranAddResult>, ScreenError> {
    services::cotrn02c::add(&state, &user, request)
        .await
        .map(TranAddResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;

    use serde_json::{Value, json};
    use utoipa_axum::router::OpenApiRouter;

    use crate::app::{AppState, operation};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The `$ref` of the component schema `name`.
    fn schema_ref(name: &str) -> Value {
        json!({ "$ref": format!("#/components/schemas/{name}") })
    }

    /// The operation registers as `api::app::router()` registers it and declares exactly what
    /// endpoint 13 can answer: 200, the 400 of the edits and the extractor, the 401 of the token
    /// check, the 404 of the not-found reads and the 500 of the store (COTRN02C-007,
    /// COTRN02C-013, COTRN02C-018, COTRN02C-050, COTRN02C-053, COTRN02C-061), never 403, 409 or
    /// 422.
    #[test]
    fn declares_the_endpoint_13_operation() -> TestResult {
        let (_, doc) = OpenApiRouter::<AppState>::new()
            .routes(operation(super::AddOperation, super::add))
            .split_for_parts();
        let doc = serde_json::to_value(doc)?;

        let path = doc
            .pointer("/paths/~1api~1v1~1transactions")
            .and_then(Value::as_object)
            .ok_or("no /api/v1/transactions path item")?;
        assert_eq!(
            path.keys().map(String::as_str).collect::<Vec<_>>(),
            ["post"]
        );
        let operation = path.get("post").ok_or("no post operation")?;

        assert_eq!(operation["operationId"], "cotrn02c_add");
        assert_eq!(operation["tags"], json!(["COTRN02C"]));
        assert_eq!(operation["summary"], "Add a transaction (CT02 / COTRN02C)");
        assert_eq!(operation["security"], json!([{ "bearer_auth": [] }]));
        assert_eq!(
            operation["requestBody"]["content"]["application/json"]["schema"],
            schema_ref("TranAddRequest")
        );
        assert_eq!(operation["requestBody"]["required"], true);

        let responses = operation["responses"]
            .as_object()
            .ok_or("no responses object")?;
        assert_eq!(
            responses
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["200", "400", "401", "404", "500"])
        );
        for (status, response) in responses {
            let body = if status == "200" {
                "TranAddResult"
            } else {
                "ScreenError"
            };
            assert_eq!(
                response["content"]["application/json"]["schema"],
                schema_ref(body),
                "status {status}"
            );
        }

        let schemas = doc
            .pointer("/components/schemas")
            .and_then(Value::as_object)
            .ok_or("no component schemas")?;
        for name in [
            "TranAddRequest",
            "TranAddFields",
            "TranAddResult",
            "ScreenError",
        ] {
            assert!(schemas.contains_key(name), "schema {name} not registered");
        }
        Ok(())
    }
}
