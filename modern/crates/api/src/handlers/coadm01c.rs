//! COADM01C, the admin menu: the handler of endpoint 3, `GET /api/v1/menus/admin` (transaction
//! `CA00`, map `COADM1A`; AAP 0.3.4.1, 0.3.4.6).
//!
//! The endpoint returns the six options of `COADM02Y` in table order and the program's two option
//! texts, as `services::coadm01c::menu` builds them, through the main menu's `MenuResponse`,
//! which serves both menus (COADM01C-005 to COADM01C-014).
//!
//! The admin gate is the `AdminUser` extractor: a request without a valid token is refused with
//! 401 and a token whose `usr_type` is not `A` with 403, before the service runs (COADM01C-001,
//! n5). The user type comes only from the verified token, never from the request (AAP 0.5.1);
//! the frontend's `/admin` route guard only hides the screen. ENTER's option edit, the transfers
//! to the six option screens and F3 run on the `COADM01` screen from the response it holds, with
//! no further request (COADM01C-002 to COADM01C-004, COADM01C-015 to COADM01C-022).
//!
//! The handler extracts the identity, calls one service function and maps its result (Thin
//! Request Handlers). The menu is a fixed table, a single resource rather than a collection, so
//! it takes no `page_size` (Bounded Pagination).

use axum::Json;

use crate::auth::extract::AdminUser;
use crate::dto::comen01c::MenuResponse;
use crate::error::ScreenError;
use crate::services;

/// Admin menu (CA00 / COADM01C)
///
/// Returns the administrator option table: the six `COADM02Y` options in table order, each with
/// its number, display text, program and admin-only and installed flags, and the invalid-option
/// and not-installed texts. ENTER's option checks, the transfers and PF3 run in the screen,
/// without a further request.
///
/// # Errors
///
/// 401 without a valid token and 403 for a token whose user type is not `A`, both answered by the
/// `AdminUser` extractor before the service runs.
// An `async fn`, so that the extracted identity moves into the future the service borrows it
// from; the same form as the main menu's handler.
#[utoipa::path(
    get,
    path = "/api/v1/menus/admin",
    operation_id = "coadm01c_admin_menu",
    tag = "COADM01C",
    responses(
        (
            status = 200,
            description = "The admin menu: the six `COADM02Y` options in table order and the \
                           program's invalid-option and not-installed texts",
            body = MenuResponse
        ),
        (
            status = 401,
            description = "No valid bearer token: the request is refused before the menu is built",
            body = ScreenError
        ),
        (
            status = 403,
            description = "A valid token whose user type is not `A`: the admin gate refuses it",
            body = ScreenError
        ),
    ),
    security(("bearer_auth" = [])),
)]
pub(crate) async fn admin_menu(admin: AdminUser) -> Json<MenuResponse> {
    Json(MenuResponse::from(services::coadm01c::menu(&admin)))
}
