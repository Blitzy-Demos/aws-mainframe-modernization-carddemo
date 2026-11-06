//! Password policy for the `USRSEC` user file.
//!
//! `COSGN00C` compares the stored and entered passwords in plaintext
//! (`IF SEC-USR-PWD = WS-USER-PWD` [app/cbl/COSGN00C.cbl:L223]). Here the
//! stored value is an argon2id hash, a mandated deviation (E-20), computed over
//! exactly the `PIC X(08)` bytes the COBOL stores and compares.
//!
//! Storage and sign-on normalise differently, as in the COBOL:
//!
//! - Storage: `COUSR01C` `PROCESS-ENTER-KEY` [app/cbl/COUSR01C.cbl:L154-158],
//!   `COUSR02C` `UPDATE-USER-INFO` [app/cbl/COUSR02C.cbl:L227-229] and the
//!   `DUSRSECJ` seed move the password into `SEC-USR-PWD PIC X(08)` as entered:
//!   case unchanged, space-padded, right-truncated. [`hash_stored_password`]
//!   hashes that value.
//! - Sign-on: `COSGN00C` `PROCESS-ENTER-KEY` [app/cbl/COSGN00C.cbl:L132-136]
//!   upper-cases the user ID and the password (`FUNCTION UPPER-CASE`) and pads
//!   both to 8. [`normalize_signon_user_id`] gives the `USRSEC` key and
//!   [`verify_signon`] checks the password.
//!
//! A password or user ID stored in lower or mixed case therefore never signs
//! on, as in the COBOL.
//!
//! `COUSR02C`'s "password changed?" test is [`verify_as_entered`], which pads
//! without a case change. The COBOL reloads the current password into the edit
//! field [app/cbl/COUSR02C.cbl:L166-170]; a hash cannot be shown, so the
//! current password is never displayed (E-21).

pub mod password;

pub use password::{
    PasswordError, SEC_USR_ID_LEN, SEC_USR_PWD_LEN, hash_stored_password, normalize_signon_user_id,
    verify_as_entered, verify_signon,
};
