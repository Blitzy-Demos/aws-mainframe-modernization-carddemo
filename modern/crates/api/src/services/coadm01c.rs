//! COADM01C, the admin menu service: the content of endpoint 3, `GET /api/v1/menus/admin`
//! (transaction `CA00`, map `COADM1A`; AAP 0.3.4.1).
//!
//! The menu is the fixed table of `COADM02Y`, six options (`COADM02Y.cpy:22`), with the
//! program's invalid-option and not-installed texts. [`menu`] returns it as the domain builds it
//! in `domain::online::coadm01c::admin_menu`, which ports the `server` rows of
//! `modern/audit/online/COADM01C.md`:
//!
//! - COADM01C-005: exactly six options in table order, the same for every admin and request.
//! - COADM01C-006 to COADM01C-011: one option each, number, name and program from `COADM02Y`.
//! - COADM01C-012: every option admin-only; the menu states no user type and no admin-only text.
//! - COADM01C-013: every option installed.
//! - COADM01C-014: the invalid-option and not-installed texts as the program moves them.
//!
//! COADM01C-001, the gate, is not decided here: `crate::auth::extract::AdminUser` refuses a
//! request without a valid token with 401 and a token whose `usr_type` is not `A` with 403
//! before this service runs (AAP 0.3.4.6). The user type comes only from the verified token, so
//! nothing in a request can open the gate (AAP 0.5.1). COADM01C itself tests no user type, so
//! the admin's identity selects nothing: the parameter only makes the gate part of the
//! signature, and every admin gets the same menu.
//!
//! The service reads and writes no store and needs no clock, so it is a plain `fn` with no
//! application state. The menu is a single resource, not a collection, so it takes no page
//! size (Bounded Pagination). The handler maps the result through the main menu's
//! `MenuResponse`, which serves both menus (Thin Request Handlers). The `client` rows
//! (COADM01C-002 to COADM01C-004, COADM01C-015 to COADM01C-022) send no request and belong to
//! the `COADM01` screen.

use domain::online::coadm01c;
use domain::online::comen01c::Menu;

use crate::auth::extract::AdminUser;

/// The admin menu for an administrator the `AdminUser` gate has admitted: the six `COADM02Y`
/// options in table order with the program's two option texts (COADM01C-001, COADM01C-005 to
/// COADM01C-014).
#[must_use]
pub(crate) fn menu(_admin: &AdminUser) -> Menu {
    coadm01c::admin_menu()
}

#[cfg(test)]
mod tests {
    use domain::copybook::PicX;
    use domain::online::coadm01c::{ADMIN_MENU_OPTIONS, INVALID_OPTION, admin_menu};
    use domain::online::comen01c::MenuEntry;
    use domain::online::cosgn00c::UserType;

    use super::menu;
    use crate::auth::extract::{AdminUser, SignedOnUser};

    /// An administrator as the gate admits one: a verified user ID and user type `A`.
    fn admin(user_id: &str) -> AdminUser {
        AdminUser(SignedOnUser {
            user_id: PicX::from_text(user_id)
                .expect("a user ID of at most 8 printable ASCII bytes"),
            user_type: UserType::ADMIN,
        })
    }

    // audit: COADM01C-001
    // audit: COADM01C-005
    // audit: COADM01C-012
    // audit: COADM01C-014
    #[test]
    fn every_admin_gets_the_six_option_admin_menu() {
        for user_id in ["ADMIN001", "A", "ADMIN 2"] {
            let menu = menu(&admin(user_id));
            assert_eq!(menu, admin_menu());
            let options: Vec<_> = menu.entries().iter().map(MenuEntry::option).collect();
            assert_eq!(options, ADMIN_MENU_OPTIONS.iter().collect::<Vec<_>>());
            assert_eq!(menu.user_type(), None);
            assert_eq!(menu.admin_only(), None);
            assert_eq!(menu.invalid_option(), INVALID_OPTION);
        }
    }
}
