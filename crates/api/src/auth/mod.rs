//! Authentication of API requests: the bearer token that stands for the
//! signed-on user, and the extractors that read it back.
//!
//! COSGN00C hands the signed-on user to the next program in the COMMAREA
//! fields `CDEMO-USER-ID PIC X(08)` and `CDEMO-USER-TYPE PIC X(01)`, whose 88
//! levels are `CDEMO-USRTYP-ADMIN` (`'A'`) and `CDEMO-USRTYP-USER` (`'U'`)
//! [app/cpy/COCOM01Y.cpy:L25-28]. Here `POST /auth/login` issues an HS256
//! bearer token that carries the same two values as its `sub` and `usr_type`
//! claims ([`jwt`]), and every later request presents it in its
//! `Authorization` header ([`extract`]).
//!
//! The actor of every operation is always the verified token's `sub` and
//! `usr_type`. No request body field, no header other than `Authorization`
//! and no query parameter ever names the acting user or a role.
//!
//! User routes take [`AuthUser`]. Every `/admin/**` route, the reward-rule
//! routes included, takes [`AdminUser`]. A request without a valid token
//! (missing, malformed, badly signed, expired, or with a `usr_type` other than
//! `A` or `U`) is 401, as is a valid token whose `sub` names no stored user,
//! and a valid token whose `usr_type` is not `A` is 403 on `/admin/**`.

pub mod extract;
pub mod jwt;

pub use extract::{AdminUser, AuthUser};
pub use jwt::{Claims, IssuedToken, JwtError, JwtKeys, UserType};
