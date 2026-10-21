//! COMEN01C, the main menu: the service of endpoint 2, `GET /api/v1/menus/main` (transaction
//! `CM00`, map `COMEN1A`; AAP 0.3.4.1).
//!
//! The menu is the fixed option table of `COMEN02Y` (`CARDDEMO-MAIN-MENU-OPTIONS`,
//! `app/cpy/COMEN02Y.cpy:21-90`), compiled into `domain::online::comen01c`: 11 options in table
//! order, each with its number, display text, program, admin-only and installed flags and the
//! text it shows when it cannot be taken, plus the program's two rejection texts (COMEN01C-011,
//! COMEN01C-013, COMEN01C-015, COMEN01C-017, COMEN01C-020, COMEN01C-021). The screen applies
//! ENTER's option edit and its range, admin-only and not-installed checks with that content and
//! sends no further request, so this service is the whole of the server's part of COMEN01C.
//!
//! # Identity
//!
//! The user type the menu states, the one the screen's admin-only check compares, is the
//! `CDEMO-USER-TYPE` sign-on set in the COMMAREA (COCOM01Y-006): here the verified token's
//! `usr_type` claim as [`SignedOnUser`] carries it, never a request field (AAP 0.5.1;
//! COMEN01C-004). COMEN01C tests no user type on entry, so every signed-on user is served and
//! endpoint 2 has no admin gate. A request without a valid token is refused with 401 by the
//! extractor before this service runs (COMEN01C-002).
//!
//! # Shape
//!
//! Nothing is read, written or timed: the table comes from the copybook and the header's date and
//! time are the screen's (COMEN01C-019), so the service takes no application state and cannot
//! fail. The menu is a single resource, not a collection, so it takes no page size (Bounded
//! Pagination). The handler maps the returned [`Menu`] to `MenuResponse` and decides nothing
//! (Thin Request Handlers).

use domain::online::comen01c::{self, Menu};

use crate::auth::extract::SignedOnUser;

/// Endpoint 2's content for `user`: the main menu of `COMEN02Y` with its flags and texts, stating
/// the user type of the verified token, which is the menu's only part that differs between users
/// (COMEN01C-004, COMEN01C-011, COMEN01C-020).
#[must_use]
pub(crate) fn menu(user: &SignedOnUser) -> Menu {
    comen01c::main_menu(&user.user_type)
}

#[cfg(test)]
mod tests {
    use domain::copybook::PicX;
    use domain::online::comen01c::{ADMIN_ONLY, INVALID_OPTION, OPTION_COUNT, main_menu};
    use domain::online::cosgn00c::UserType;

    use super::menu;
    use crate::auth::extract::SignedOnUser;

    /// A signed-on user as the extractor rebuilds one from verified claims.
    fn signed_on(user_id: [u8; 8], user_type: UserType) -> SignedOnUser {
        SignedOnUser {
            user_id: PicX::from_array(user_id),
            user_type,
        }
    }

    // audit: COMEN01C-004
    // audit: COMEN01C-011
    // audit: COMEN01C-013
    // audit: COMEN01C-020
    #[test]
    fn every_user_type_gets_the_same_table_and_its_own_type() {
        let reference = menu(&signed_on(*b"USER0001", UserType::USER));
        for user in [
            signed_on(*b"USER0001", UserType::USER),
            signed_on(*b"ADMIN001", UserType::ADMIN),
            signed_on(*b"OTHER001", UserType::from_code(PicX::from_array([b' ']))),
        ] {
            let served = menu(&user);
            assert_eq!(served, main_menu(&user.user_type));
            assert_eq!(served.user_type(), Some(&user.user_type));
            assert_eq!(served.entries(), reference.entries());
            assert_eq!(served.entries().len(), usize::from(OPTION_COUNT));
            assert!(served.entries().iter().all(|entry| {
                let option = entry.option();
                !option.access().is_admin_only() && option.installation().is_installed()
            }));
            assert_eq!(served.invalid_option(), INVALID_OPTION);
            assert_eq!(served.admin_only(), Some(ADMIN_ONLY));
        }
    }
}
