//! COCRDUPC, the card update (transaction `CCUP`, map `CCRDUPA`): the handlers of endpoint 9,
//! `GET /api/v1/cards/edit` (ENTER fetch), and endpoint 10, `PUT /api/v1/cards/{card_ref}`
//! (`aid` ENTER validates, `aid` PF5 saves).
//!
//! Each handler extracts the verified [`SignedOnUser`] and the validated DTO, calls one function
//! of `crate::services::cocrdupc` and maps what it returns through the `From` conversions of
//! `crate::dto::cocrdupc` and [`ScreenError`] (Thin Request Handlers). The identity is extracted
//! before the query or the body, so a request without a valid token is a 401 whatever its input.
//!
//! - The attention key, the echoed state and the change check belong to the service and the
//!   domain: PF5 outside state `N` is processed as ENTER (COCRDUPC-007), and a save compares the
//!   card read for update with the echoed snapshot (COCRDUPC-037, COCRDUPC-044).
//! - F3 and F12 are the screen's own and send no request (COCRDUPC-004, COCRDUPC-006).
//! - The path segment of endpoint 10 is the card's `card_ref` handle as raw text, never parsed
//!   here: the service resolves it, and a handle that names no card is the program's lock
//!   failure (COCRDUPC-036, COCRDUPC-043).
//!
//! # Cardholder data
//!
//! A response carries the card number only redacted, twelve `*` and its last four digits, beside
//! its `card_ref` handle, and never a CVV (COCRDUPC-042, `E32`). The update rewrites the name,
//! the status and the expiry date only: the stored card number and CVV come from the card read
//! for update, never from the request (COCRDUPC-038). Neither handler logs or traces anything,
//! because the query of endpoint 9 may hold a typed card number.

use axum::Json;
use axum::extract::{Path, State};

use crate::app::AppState;
use crate::auth::extract::SignedOnUser;
use crate::dto::cocrdslc::CardLookupQuery;
use crate::dto::cocrdupc::{CardUpdateRequest, CardUpdateResult, CardUpdateView};
use crate::dto::{ValidJson, ValidQuery};
use crate::error::ScreenError;
use crate::services;

/// Fetch a card for update (CCUP / COCRDUPC).
///
/// ENTER with the account and card search keys, or with the `card_ref` handle of a card list row
/// in place of the card number: the card as the update screen shows it, its number redacted
/// beside its `card_ref` handle, and the snapshot that every request of the save operation
/// echoes. The account shown is the one requested.
///
/// # Errors
///
/// A `ScreenError` carrying the program's message: 400 when a search key fails its edit or the
/// query does not fit its screen fields, 401 without a valid bearer token, 404 when no card has
/// the number or handle, and 500 when the card read fails.
#[utoipa::path(
    get,
    path = "/api/v1/cards/edit",
    operation_id = "cocrdupc_fetch",
    tag = "COCRDUPC",
    params(CardLookupQuery),
    responses(
        (
            status = 200,
            description = "The card with its number redacted, and the snapshot to echo",
            body = CardUpdateView
        ),
        (
            status = 400,
            description = "A search key failed its edit, or the query does not fit its fields",
            body = ScreenError
        ),
        (
            status = 401,
            description = "The request carries no valid bearer token",
            body = ScreenError
        ),
        (
            status = 404,
            description = "No card has the number or handle searched for",
            body = ScreenError
        ),
        (
            status = 500,
            description = "The card read failed",
            body = ScreenError
        )
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn fetch(
    State(state): State<AppState>,
    user: SignedOnUser,
    ValidQuery(query): ValidQuery<CardLookupQuery>,
) -> Result<Json<CardUpdateView>, ScreenError> {
    services::cocrdupc::fetch(&state, &user, query)
        .await
        .map(CardUpdateView::from)
        .map(Json)
        .map_err(ScreenError::from)
}

/// Validate or save a card update (CCUP / COCRDUPC).
///
/// `aid` ENTER edits the name, the status and the expiry month and year against the echoed
/// snapshot and, when they pass, asks for confirmation. `aid` PF5 in state
/// `changes_ok_not_confirmed` runs the change check and the edits again and rewrites the card
/// the `card_ref` path segment names; PF5 in any other state is processed as ENTER. After a
/// save, a lock failure or a rewrite failure, the next request restarts the search. The stored
/// card number and CVV are never taken from the request.
///
/// # Errors
///
/// A `ScreenError` carrying the program's message: 400 when nothing changed, a field fails its
/// edit or the body does not fit its screen fields; 401 without a valid bearer token; 409 when
/// the card changed after it was fetched; 500 when the card cannot be locked for update, a
/// `card_ref` that names no card included, or when its rewrite fails.
#[utoipa::path(
    put,
    path = "/api/v1/cards/{card_ref}",
    operation_id = "cocrdupc_save",
    tag = "COCRDUPC",
    params(
        ("card_ref" = String, Path, description = "Card handle from the fetch or list response")
    ),
    request_body = CardUpdateRequest,
    // No 404: a save reads the card only for update, and a `card_ref` that names no card is
    // the program's lock failure, a 500, so this operation has no not-found answer to declare.
    responses(
        (
            status = 200,
            description = "The changes validated, the card saved, or the search restarted",
            body = CardUpdateResult
        ),
        (
            status = 400,
            description = "Nothing changed, a field failed its edit, or the body does not fit",
            body = ScreenError
        ),
        (
            status = 401,
            description = "The request carries no valid bearer token",
            body = ScreenError
        ),
        (
            status = 409,
            description = "The card changed after it was fetched",
            body = ScreenError
        ),
        (
            status = 500,
            description = "The card could not be locked for update, or its rewrite failed",
            body = ScreenError
        )
    ),
    security(("bearer_auth" = []))
)]
pub(crate) async fn save(
    State(state): State<AppState>,
    user: SignedOnUser,
    Path(card_ref): Path<String>,
    ValidJson(request): ValidJson<CardUpdateRequest>,
) -> Result<Json<CardUpdateResult>, ScreenError> {
    services::cocrdupc::save(&state, &user, &card_ref, request)
        .await
        .map(CardUpdateResult::from)
        .map(Json)
        .map_err(ScreenError::from)
}
