//! COCRDSLC, the card detail: the handler of endpoint 8, `GET /api/v1/cards/view` (transaction
//! `CCDL`, map `CCRDSLA`, route `/cards/view`; AAP 0.3.4.1).
//!
//! # The single card-detail endpoint
//!
//! This is the one operation of the API that may return all 16 digits of a card number
//! (Cardholder Data Redaction, AAP 0.3.4.8). The handler takes no part in that choice:
//! `services::cocrdslc` decides it from the verified token's user type alone, through the
//! domain's disclosure rule (COCRDSLC-022, COCRDSLC-023), and `dto::cocrdslc` serialises the
//! variant the domain chose. No request field can ask for disclosure, and the rule's owner clause
//! never grants it, because `CardDemo` links no user to a customer, account or card (AAP 0.5.1).
//!
//! # Thin Request Handlers
//!
//! [`view`] extracts the state, the [`SignedOnUser`] and the validated [`CardLookupQuery`],
//! calls `services::cocrdslc::view` once, and maps the result through the `From` conversions of
//! `dto::cocrdslc` and `error`. The identity is extracted before the query, so a request without
//! a valid token is a 401 whatever its query holds. The query names the account and exactly one
//! of the typed `card_num` and the `card_ref` handle of a COCRDLI list row; that exclusivity is
//! COCRDSLC's own edit, run by the service (COCRDSLC-006, COCRDSLC-007), not a check here.
//!
//! Nothing here logs or traces: the query may hold a typed card number. F3, the screen's only
//! other key, is client-side and sends no request (COCRDSLC-004).

use axum::Json;
use axum::extract::State;

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::ValidQuery;
use crate::dto::cocrdslc::{CardLookupQuery, CardViewResponse};
use crate::error::ScreenError;
use crate::services;

/// View a card (CCDL / COCRDSLC).
///
/// ENTER on the card detail screen: the card is looked up by the account and either the typed
/// `card_num` or the `card_ref` handle of a card list row, never both. The card number is
/// returned in full only to an administrator token; any other token receives twelve asterisks
/// followed by the last four digits. The response carries no card verification code.
///
/// # Errors
///
/// A `ScreenError` with the status of the step that failed: 400 when the query does not
/// deserialise, exceeds a screen field length or fails COCRDSLC's account and card edits; 401
/// without a valid bearer token; 404 when no card has the number or handle; 500 when the card
/// read fails.
#[utoipa::path(
    get,
    path = "/api/v1/cards/view",
    operation_id = "cocrdslc_view",
    tag = "COCRDSLC",
    params(CardLookupQuery),
    responses(
        (
            status = 200,
            description = "The card; its number in full for an administrator token, otherwise redacted to its last four digits",
            body = CardViewResponse
        ),
        (
            status = 400,
            description = "The query failed request validation or a COCRDSLC account or card edit",
            body = ScreenError
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
        (
            status = 404,
            description = "No card has the number or handle",
            body = ScreenError
        ),
        (status = 500, description = "The card read failed", body = ScreenError),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn view(
    State(state): State<AppState>,
    user: SignedOnUser,
    ValidQuery(query): ValidQuery<CardLookupQuery>,
) -> Result<Json<CardViewResponse>, ScreenError> {
    services::cocrdslc::view(&state, &user, query)
        .await
        .map(CardViewResponse::from)
        .map(Json)
        .map_err(ScreenError::from)
}
