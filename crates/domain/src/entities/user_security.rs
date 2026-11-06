//! `UserSecurity`: the `USRSEC` user record, `CSUSR01Y` `SEC-USER-DATA`
//! [app/cpy/CSUSR01Y.cpy:L17-23].
//!
//! The COBOL record is 80 bytes, keyed by `SEC-USR-ID` (`KEYS(8,0)`
//! [app/jcl/DUSRSECJ.jcl:L65]). Its fields map to the `users` table of
//! `migrations/0002_master_data.sql` as section A of
//! `docs/parity/parity-oracle-audit.md` records them:
//!
//! | Field | PIC | Offset | Audit row | Rust | Column |
//! |---|---|---|---|---|---|
//! | `SEC-USR-ID` | `X(08)` | 0 | P-CSUSR01Y-001 | [`UserId`] | `user_id` |
//! | `SEC-USR-FNAME` | `X(20)` | 8 | P-CSUSR01Y-002 | [`Alnum<20>`](Alnum) | `sec_usr_fname` |
//! | `SEC-USR-LNAME` | `X(20)` | 28 | P-CSUSR01Y-003 | [`Alnum<20>`](Alnum) | `sec_usr_lname` |
//! | `SEC-USR-PWD` | `X(08)` | 48 | P-CSUSR01Y-004 | `String` (argon2id PHC string) | `password_hash` |
//! | `SEC-USR-TYPE` | `X(01)` | 56 | P-CSUSR01Y-005 | [`Alnum<1>`](Alnum) | `sec_usr_type` |
//! | `SEC-USR-FILLER` | `X(23)` | 57 | P-CSUSR01Y-006 | not modelled | not stored |
//!
//! The one change from the copybook is the password. The COBOL stores
//! `SEC-USR-PWD` in plaintext and compares it in plaintext
//! (`IF SEC-USR-PWD = WS-USER-PWD` [app/cbl/COSGN00C.cbl:L223]); this record
//! holds an argon2id hash in its place, a mandated deviation (E-20). Hashing
//! and verification belong to `crate::auth::password`, never to this entity.
//!
//! [`UserProfile`] is the same record without `SEC-USR-PWD`: what a read
//! returns where no credential is compared, so a hash is read only by the
//! sign-on and by the `COUSR02C` password compare, the two reads that verify
//! one.

use core::fmt;

use crate::numeric::{Alnum, UserId};

/// The hasher that produces [`UserSecurity::password_hash`] and the error it
/// reports, re-exported from [`crate::auth`] for the persistence seed, which
/// hashes the fixture passwords it loads. Hashing stays in `crate::auth`.
pub use crate::auth::{PasswordError, hash_stored_password};

/// The `SEC-USR-TYPE` value of an administrator: the 88-level
/// `CDEMO-USRTYP-ADMIN VALUE 'A'` [app/cpy/COCOM01Y.cpy:L27].
const ADMIN_USER_TYPE: &[u8; 1] = b"A";

/// A `USRSEC` user record, `CSUSR01Y` `SEC-USER-DATA`
/// [app/cpy/CSUSR01Y.cpy:L17-23].
///
/// `Debug` omits [`UserSecurity::password_hash`], so the hash never reaches a
/// log line or a tracing span.
#[derive(Clone, PartialEq, Eq)]
pub struct UserSecurity {
    /// `SEC-USR-ID PIC X(08)`, the record key — P-CSUSR01Y-001
    /// [app/cpy/CSUSR01Y.cpy:L18]. SQL `VARCHAR(8)` column `user_id`.
    ///
    /// The ID is held as stored: the admin programs keep the case it was
    /// entered in, and `COSGN00C` upper-cases only the ID typed at sign-on
    /// (E-20).
    pub sec_usr_id: UserId,
    /// `SEC-USR-FNAME PIC X(20)`, the first name — P-CSUSR01Y-002
    /// [app/cpy/CSUSR01Y.cpy:L19]. SQL `VARCHAR(20)` column `sec_usr_fname`,
    /// right-trimmed.
    pub sec_usr_fname: Alnum<20>,
    /// `SEC-USR-LNAME PIC X(20)`, the last name — P-CSUSR01Y-003
    /// [app/cpy/CSUSR01Y.cpy:L20]. SQL `VARCHAR(20)` column `sec_usr_lname`,
    /// right-trimmed.
    pub sec_usr_lname: Alnum<20>,
    /// The argon2id PHC string that replaces `SEC-USR-PWD PIC X(08)` —
    /// P-CSUSR01Y-004 [app/cpy/CSUSR01Y.cpy:L21]. SQL `TEXT` column
    /// `password_hash`.
    ///
    /// The hash covers the exact 8 bytes `COUSR01C` and `COUSR02C` store:
    /// the password as entered, case unchanged, space-padded and cut to
    /// `X(08)` (E-20). `crate::auth::password::hash_stored_password` produces
    /// it; `verify_signon` (sign-on, upper-cased input) and
    /// `verify_as_entered` (the `COUSR02C` "password changed?" test, E-21)
    /// check input against it. This entity never holds a plaintext password.
    ///
    /// The audit row names the type `PasswordHash`. No such newtype exists:
    /// the field is the `String` those functions return and read, so a stored
    /// value passes between them and the `password_hash` column unchanged.
    pub password_hash: String,
    /// `SEC-USR-TYPE PIC X(01)`, the user type — P-CSUSR01Y-005
    /// [app/cpy/CSUSR01Y.cpy:L22]. SQL `VARCHAR(1)` column `sec_usr_type`,
    /// right-trimmed. `A` is an administrator and `U` a regular user
    /// [app/cpy/COCOM01Y.cpy:L27-28]; see [`UserSecurity::is_admin`].
    pub sec_usr_type: Alnum<1>,
}

impl UserSecurity {
    /// Whether the user is an administrator: `SEC-USR-TYPE` is exactly `A`.
    ///
    /// `COSGN00C` `READ-USER-SEC-FILE` moves `SEC-USR-TYPE` to
    /// `CDEMO-USER-TYPE` and transfers to the admin menu `COADM01C` when the
    /// 88-level `CDEMO-USRTYP-ADMIN VALUE 'A'` holds, and to the main menu
    /// `COMEN01C` otherwise — P-COSGN00C-009 [app/cbl/COSGN00C.cbl:L227-240]
    /// [app/cpy/COCOM01Y.cpy:L27]. Every other byte, `U`, a space and a
    /// lower-case `a` included, is therefore not an administrator.
    ///
    /// This drives the `usr_type` claim of the sign-on JWT and, through it,
    /// the 403 gate on every `/admin/**` endpoint.
    #[must_use]
    pub fn is_admin(&self) -> bool {
        is_admin_type(self.sec_usr_type)
    }
}

/// The 88-level `CDEMO-USRTYP-ADMIN VALUE 'A'` [app/cpy/COCOM01Y.cpy:L27] on
/// a `SEC-USR-TYPE`, the one test [`UserSecurity::is_admin`] and
/// [`UserProfile::is_admin`] share.
fn is_admin_type(user_type: Alnum<1>) -> bool {
    user_type.as_bytes() == ADMIN_USER_TYPE
}

/// A `USRSEC` user record without its password: `CSUSR01Y` `SEC-USER-DATA`
/// [app/cpy/CSUSR01Y.cpy:L17-23] less `SEC-USR-PWD` (P-CSUSR01Y-004).
///
/// The record a read returns where no credential is compared: the `COUSR00C`
/// user list, the `COUSR02C` and `COUSR03C` fetch, the `COUSR03C` delete and
/// the bearer token's subject check. Only the `COSGN00C` sign-on and the
/// `COUSR02C` password compare read a [`UserSecurity`], which carries the
/// hash. This record holds no secret, so its `Debug` shows every field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserProfile {
    /// `SEC-USR-ID PIC X(08)`, the record key — P-CSUSR01Y-001
    /// [app/cpy/CSUSR01Y.cpy:L18]. SQL `VARCHAR(8)` column `user_id`, held as
    /// stored, case kept (E-20).
    pub sec_usr_id: UserId,
    /// `SEC-USR-FNAME PIC X(20)`, the first name — P-CSUSR01Y-002
    /// [app/cpy/CSUSR01Y.cpy:L19]. SQL `VARCHAR(20)` column `sec_usr_fname`,
    /// right-trimmed.
    pub sec_usr_fname: Alnum<20>,
    /// `SEC-USR-LNAME PIC X(20)`, the last name — P-CSUSR01Y-003
    /// [app/cpy/CSUSR01Y.cpy:L20]. SQL `VARCHAR(20)` column `sec_usr_lname`,
    /// right-trimmed.
    pub sec_usr_lname: Alnum<20>,
    /// `SEC-USR-TYPE PIC X(01)`, the user type — P-CSUSR01Y-005
    /// [app/cpy/CSUSR01Y.cpy:L22]. SQL `VARCHAR(1)` column `sec_usr_type`,
    /// right-trimmed; see [`UserProfile::is_admin`].
    pub sec_usr_type: Alnum<1>,
}

impl UserProfile {
    /// Whether the user is an administrator: `SEC-USR-TYPE` is exactly `A`,
    /// the test [`UserSecurity::is_admin`] applies (P-COSGN00C-009).
    #[must_use]
    pub fn is_admin(&self) -> bool {
        is_admin_type(self.sec_usr_type)
    }
}

impl From<&UserSecurity> for UserProfile {
    /// Every field of `user` but its password hash.
    fn from(user: &UserSecurity) -> Self {
        Self {
            sec_usr_id: user.sec_usr_id,
            sec_usr_fname: user.sec_usr_fname,
            sec_usr_lname: user.sec_usr_lname,
            sec_usr_type: user.sec_usr_type,
        }
    }
}

impl fmt::Debug for UserSecurity {
    /// Every field but the password hash, ended with `..` to show that one is
    /// left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserSecurity")
            .field("sec_usr_id", &self.sec_usr_id)
            .field("sec_usr_fname", &self.sec_usr_fname)
            .field("sec_usr_lname", &self.sec_usr_lname)
            .field("sec_usr_type", &self.sec_usr_type)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::{UserProfile, UserSecurity};
    use crate::numeric::{Alnum, UserId};

    /// A well-formed argon2id PHC string with a made-up salt and output.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHRzYWx0$aGFzaGhhc2hoYXNoaGFzaGhhc2hoYXNoaGFzaGhhc2g";

    /// The seeded `ADMIN001` row, P-CSUSR01Y-008 [app/jcl/DUSRSECJ.jcl:L35],
    /// with the given user type.
    fn admin001(user_type: [u8; 1]) -> UserSecurity {
        UserSecurity {
            sec_usr_id: UserId::move_from("ADMIN001"),
            sec_usr_fname: Alnum::move_from("MARGARET"),
            sec_usr_lname: Alnum::move_from("GOLD"),
            password_hash: HASH.to_owned(),
            sec_usr_type: Alnum::from_array(user_type),
        }
    }

    #[test]
    fn only_user_type_a_is_admin() {
        // `A` and `U` are the two types the seed rows carry (P-CSUSR01Y-008,
        // P-CSUSR01Y-009); a space and a lower-case `a` satisfy neither 88-level.
        let cases = [
            (*b"A", true),
            (*b"U", false),
            (*b" ", false),
            (*b"a", false),
        ];
        for (user_type, expected) in cases {
            let user = admin001(user_type);
            assert_eq!(user.is_admin(), expected, "{user_type:?}");
            assert_eq!(
                UserProfile::from(&user).is_admin(),
                expected,
                "profile {user_type:?}"
            );
        }
    }

    #[test]
    fn a_profile_keeps_every_field_but_the_hash() {
        let profile = UserProfile::from(&admin001(*b"U"));
        assert_eq!(
            profile,
            UserProfile {
                sec_usr_id: UserId::move_from("ADMIN001"),
                sec_usr_fname: Alnum::move_from("MARGARET"),
                sec_usr_lname: Alnum::move_from("GOLD"),
                sec_usr_type: Alnum::from_array(*b"U"),
            }
        );
        let rendered = format!("{profile:?}");
        assert!(!rendered.contains(HASH), "{rendered}");
        assert!(!rendered.contains("password_hash"), "{rendered}");
        assert!(rendered.contains("\"MARGARET            \""), "{rendered}");
    }

    #[test]
    fn debug_shows_the_copybook_fields_and_omits_the_hash() {
        let rendered = format!("{:?}", admin001(*b"A"));

        assert!(!rendered.contains(HASH), "{rendered}");
        assert!(!rendered.contains("argon2id"), "{rendered}");
        assert!(!rendered.contains("password_hash"), "{rendered}");
        for shown in [
            "UserSecurity { ",
            "sec_usr_id: ",
            "\"ADMIN001\"",
            "sec_usr_fname: ",
            "\"MARGARET            \"",
            "sec_usr_lname: ",
            "\"GOLD                \"",
            "sec_usr_type: ",
            "\"A\"",
        ] {
            assert!(
                rendered.contains(shown),
                "{shown:?} missing from {rendered}"
            );
        }
        assert!(rendered.ends_with(", .. }"), "{rendered}");
    }
}
