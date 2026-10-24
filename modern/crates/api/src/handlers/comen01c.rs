//! COMEN01C, the main menu: the handler of endpoint 2, `GET /api/v1/menus/main` (transaction
//! `CM00`, map `COMEN1A`; AAP 0.3.4.1).
//!
//! The response is the fixed option table of `COMEN02Y`, 11 options in table order with their
//! admin-only and installed flags and unavailable texts, plus the program's rejection texts and
//! the user type of the verified token (COMEN01C-004, COMEN01C-011, COMEN01C-020). The screen
//! applies ENTER's option edit and its range, admin-only and not-installed checks from that
//! content, and performs the transfers and PF3 itself, so this is the only request COMEN01C
//! makes.
//!
//! The handler extracts the [`SignedOnUser`], calls `services::comen01c::menu` and maps the menu
//! into [`MenuResponse`] (Thin Request Handlers). The service reads no state and cannot fail, so
//! the handler takes no application state and returns no error of its own; a request without a
//! valid token is refused with 401 by the extractor before it runs (COMEN01C-002). The menu is a
//! single resource, not a collection, so it takes no page size (Bounded Pagination).

use axum::Json;

use crate::auth::extract::SignedOnUser;
use crate::dto::comen01c::MenuResponse;
use crate::error::ScreenError;
use crate::services;

/// The `utoipa::Path` of [`main_menu`], from its `#[utoipa::path]`, for `crate::app::router`.
pub(crate) struct MainMenuOperation;

/// Main menu (CM00 / COMEN01C).
///
/// Returns the 11 options of `COMEN02Y` in table order with their admin-only and installed flags,
/// the user type of the bearer token and the program's rejection texts. The ENTER option checks,
/// the transfers and PF3 run in the screen from this response, with no further request.
///
/// # Errors
///
/// 401 with a `ScreenError` when the request carries no valid bearer token.
#[utoipa::path(
    impl_for = MainMenuOperation,
    get,
    path = "/api/v1/menus/main",
    operation_id = "comen01c_main_menu",
    tag = "COMEN01C",
    responses(
        (
            status = 200,
            description = "The main menu: the option table with its flags and the rejection texts",
            body = MenuResponse
        ),
        (status = 401, description = "No valid bearer token", body = ScreenError),
    ),
    security(("bearer_auth" = [])),
)]
// An `async fn` over a synchronous service: axum serves it as a future, which owns the extractor.
pub(crate) async fn main_menu(user: SignedOnUser) -> Json<MenuResponse> {
    Json(MenuResponse::from(services::comen01c::menu(&user)))
}
