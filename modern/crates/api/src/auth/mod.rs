//! The API's identity: the `CardDemo` COMMAREA identity of a signed-on user, carried as a signed
//! token (AAP 0.3.4.6, 0.5.1).
//!
//! It replaces `CDEMO-USER-ID PIC X(08)` and `CDEMO-USER-TYPE PIC X(01)`, with the 88-levels
//! `CDEMO-USRTYP-ADMIN VALUE 'A'` and `CDEMO-USRTYP-USER VALUE 'U'` (`COCOM01Y.cpy:25-28`;
//! COCOM01Y-005, COCOM01Y-006), which COSGN00C sets after a successful password check and passes
//! on by `XCTL` to COADM01C or COMEN01C (`COSGN00C.cbl:224-240`; COSGN00C-014).
//!
//! - [`jwt`] issues and verifies the HS256 tokens of [`JwtKeys`](jwt::JwtKeys), whose
//!   [`Claims`](jwt::Claims) are `sub`, `usr_type`, `iat` and `exp`.
//! - [`extract`] provides [`SignedOnUser`](extract::SignedOnUser), which guards every endpoint
//!   except 1 (`POST /api/v1/signon`), and [`AdminUser`](extract::AdminUser), the gate of the
//!   `[A]` endpoints 3, 16 to 20 and 24 to 27.
//!
//! The acting identity comes only from verified claims, never from a body, a query string, a
//! cookie or any other header. The frontend's route guard only hides the admin screens; these
//! extractors are the control.

pub mod extract;
pub mod jwt;
