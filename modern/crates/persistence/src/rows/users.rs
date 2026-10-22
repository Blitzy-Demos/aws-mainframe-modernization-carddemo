//! Codec B for `users` (migration 0003): `CSUSR01Y` `SEC-USER-DATA` with `password_hash` in
//! place of `SEC-USR-PWD` (`app/cpy/CSUSR01Y.cpy:17-23`; CSUSR01Y-001 to CSUSR01Y-006).
//!
//! | Column | Type | COBOL field | Typed value | Row |
//! |---|---|---|---|---|
//! | `usr_id` | `CHAR(8)` | `SEC-USR-ID PIC X(08)` | `PicX<8>` | CSUSR01Y-001 |
//! | `usr_fname` | `CHAR(20)` | `SEC-USR-FNAME PIC X(20)` | `PicX<20>` | CSUSR01Y-002 |
//! | `usr_lname` | `CHAR(20)` | `SEC-USR-LNAME PIC X(20)` | `PicX<20>` | CSUSR01Y-003 |
//! | `password_hash` | `TEXT` | in place of `SEC-USR-PWD PIC X(08)` | argon2id PHC `String` | CSUSR01Y-004 |
//! | `usr_type` | `CHAR(1)` | `SEC-USR-TYPE PIC X(01)` | `PicX<1>` | CSUSR01Y-005 |
//! | `filler` | `CHAR(23)` | `SEC-USR-FILLER PIC X(23)` | `PicX<23>` | CSUSR01Y-006 |
//!
//! Every `CHAR(n)` column keeps its padding, so the ID, names, type and `FILLER` read back byte
//! for byte (PIC-001, PIC-012). `SEC-USR-PWD` has no column: the store holds only the argon2id
//! PHC string `domain::password::hash` computes over the 8 password bytes, so codec B is not
//! byte-identical for that field, while codec A still round-trips each `USRSEC` record (E25,
//! LOADS-033, LOADS-034). No conversion here reads a password: the load path takes the hash of a
//! [`HashedUser`] and never its `record.pwd`, and the add and update paths arrive with a hash.
//!
//! Two row types carry the table:
//!
//! - [`UserRow`]: the six columns, built from a [`HashedUser`] (`dusrsecj`) or a [`UserEntry`]
//!   (user add and update), and read back by the single-user query as a [`UserEntry`].
//! - [`UserListRow`]: the four columns the COUSR00C page queries select,
//!   `queries/users_page_forward.sql` and `queries/users_page_backward.sql`, and nothing more, so
//!   a page never reads a hash (COUSR00C-028, Bounded Pagination). It reads back as a
//!   [`UserListEntry`].
//!
//! Their typed values, [`UserEntry`] and [`UserListEntry`], are the domain's stored projections
//! of `SEC-USER-DATA` (`domain::records::user_security`); the domain converts them into what the
//! COSGN00C, COUSR02C and COUSR00C rules take.
//!
//! No type here derives `Serialize`, and no `Debug` prints a hash: the row types leave `Debug`
//! underived, and the domain's [`UserEntry`] prints `<redacted>` in its place (PIC-024,
//! Cardholder Data Redaction). Errors name the column and the class, never the value.

use copybook::PicX;
use domain::batch::loads::HashedUser;
use domain::records::{UserEntry, UserListEntry};

use super::{pic_x_to_string, string_to_pic_x};
use crate::error::{DecodeKind, PersistenceError};

/// Column of `SEC-USR-ID` (CSUSR01Y-001).
const USR_ID: &str = "usr_id";
/// Column of `SEC-USR-FNAME` (CSUSR01Y-002).
const USR_FNAME: &str = "usr_fname";
/// Column of `SEC-USR-LNAME` (CSUSR01Y-003).
const USR_LNAME: &str = "usr_lname";
/// Column holding the argon2id hash in place of `SEC-USR-PWD` (CSUSR01Y-004, E25).
const PASSWORD_HASH: &str = "password_hash";
/// Column of `SEC-USR-TYPE` (CSUSR01Y-005).
const USR_TYPE: &str = "usr_type";
/// Column of `SEC-USR-FILLER` (CSUSR01Y-006).
const FILLER: &str = "filler";

/// A `users` row: every column of migration 0003, in copybook order (CSUSR01Y-001 to
/// CSUSR01Y-006).
///
/// Only `repos` binds and reads it. `Debug`, `Clone` and `Serialize` are left underived, so no
/// formatting of a row can show its `password_hash`.
pub struct UserRow {
    /// `usr_id CHAR(8)`, the primary key `users_pkey` (CSUSR01Y-001).
    pub(crate) usr_id: String,
    /// `usr_fname CHAR(20)` (CSUSR01Y-002).
    pub(crate) usr_fname: String,
    /// `usr_lname CHAR(20)` (CSUSR01Y-003).
    pub(crate) usr_lname: String,
    /// `password_hash TEXT`: the argon2id PHC string standing for `SEC-USR-PWD` (CSUSR01Y-004,
    /// E25).
    pub(crate) password_hash: String,
    /// `usr_type CHAR(1)` (CSUSR01Y-005).
    pub(crate) usr_type: String,
    /// `filler CHAR(23)`, kept byte for byte (CSUSR01Y-006, PIC-012).
    pub(crate) filler: String,
}

impl UserRow {
    /// The row `dusrsecj` stores for one in-stream record: its ID, names, type and `FILLER`
    /// verbatim, and the PHC string [`domain::batch::loads::hash_user`] computed (LOADS-033,
    /// E25). The record's plaintext `pwd` is never read.
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`] on the first column, in copybook order, whose value holds a
    /// NUL or non-ASCII byte, which no `CHAR(n)` or `TEXT` column stores byte for byte (PIC-002).
    pub fn from_hashed(user: &HashedUser) -> Result<Self, PersistenceError> {
        let record = &user.record;
        Self::from_fields(
            &record.id,
            &record.fname,
            &record.lname,
            &user.password_hash,
            &record.usr_type,
            &record.filler,
        )
    }

    /// The row that stores `entry`, for the user add (COUSR01C) and update (COUSR02C) writes.
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`] on the first column, in copybook order, whose value holds a
    /// NUL or non-ASCII byte (PIC-002).
    pub fn from_entry(entry: &UserEntry) -> Result<Self, PersistenceError> {
        Self::from_fields(
            &entry.id,
            &entry.fname,
            &entry.lname,
            &entry.password_hash,
            &entry.usr_type,
            &entry.filler,
        )
    }

    /// The typed values the row holds, each `CHAR(n)` column as its `PIC X(n)` field and the
    /// hash verbatim (PIC-001, PIC-012). A row written through [`UserRow::from_entry`] reads
    /// back as an entry equal to the one written.
    ///
    /// # Errors
    ///
    /// On the first failing column, in copybook order: [`DecodeKind::InvalidByte`] for a NUL or
    /// non-ASCII character (PIC-002); [`DecodeKind::WidthMismatch`] for a `CHAR(n)` value that
    /// is not exactly n bytes long.
    pub fn into_entry(self) -> Result<UserEntry, PersistenceError> {
        let id = string_to_pic_x(USR_ID, &self.usr_id)?;
        let fname = string_to_pic_x(USR_FNAME, &self.usr_fname)?;
        let lname = string_to_pic_x(USR_LNAME, &self.usr_lname)?;
        check_hash(&self.password_hash)?;
        let usr_type = string_to_pic_x(USR_TYPE, &self.usr_type)?;
        let filler = string_to_pic_x(FILLER, &self.filler)?;
        Ok(UserEntry {
            id,
            fname,
            lname,
            password_hash: self.password_hash,
            usr_type,
            filler,
        })
    }

    /// The row of these field values. Field initialisers run in the order written, so the
    /// columns are checked in copybook order.
    fn from_fields(
        id: &PicX<8>,
        fname: &PicX<20>,
        lname: &PicX<20>,
        password_hash: &str,
        usr_type: &PicX<1>,
        filler: &PicX<23>,
    ) -> Result<Self, PersistenceError> {
        Ok(Self {
            usr_id: pic_x_to_string(USR_ID, id)?,
            usr_fname: pic_x_to_string(USR_FNAME, fname)?,
            usr_lname: pic_x_to_string(USR_LNAME, lname)?,
            password_hash: check_hash(password_hash)?.to_owned(),
            usr_type: pic_x_to_string(USR_TYPE, usr_type)?,
            filler: pic_x_to_string(FILLER, filler)?,
        })
    }
}

/// One row of a COUSR00C page: exactly the `usr_id, usr_fname, usr_lname, usr_type` select list
/// of `queries/users_page_forward.sql` and `queries/users_page_backward.sql`, which name no
/// credential column (COUSR00C-028, Bounded Pagination).
///
/// Only `repos` reads it; `Debug`, `Clone` and `Serialize` are left underived, as on every row.
pub struct UserListRow {
    /// `usr_id CHAR(8)` (CSUSR01Y-001).
    pub(crate) usr_id: String,
    /// `usr_fname CHAR(20)` (CSUSR01Y-002).
    pub(crate) usr_fname: String,
    /// `usr_lname CHAR(20)` (CSUSR01Y-003).
    pub(crate) usr_lname: String,
    /// `usr_type CHAR(1)` (CSUSR01Y-005).
    pub(crate) usr_type: String,
}

impl UserListRow {
    /// The typed values of the four columns (PIC-001).
    ///
    /// # Errors
    ///
    /// On the first failing column, in select-list order: [`DecodeKind::InvalidByte`] for a NUL
    /// or non-ASCII character (PIC-002); [`DecodeKind::WidthMismatch`] for a value that is not
    /// exactly its `CHAR(n)` width.
    pub fn into_entry(self) -> Result<UserListEntry, PersistenceError> {
        Ok(UserListEntry {
            id: string_to_pic_x(USR_ID, &self.usr_id)?,
            fname: string_to_pic_x(USR_FNAME, &self.usr_fname)?,
            lname: string_to_pic_x(USR_LNAME, &self.usr_lname)?,
            usr_type: string_to_pic_x(USR_TYPE, &self.usr_type)?,
        })
    }
}

/// `hash` unchanged, when it is text the `password_hash TEXT` column stores and returns byte for
/// byte: ASCII without NUL, which every PHC string is (PIC-002). `TEXT` declares no width, so
/// nothing else is checked, and the `CHAR(n)` and `VARCHAR(n)` conversions of `rows` do not
/// apply.
fn check_hash(hash: &str) -> Result<&str, PersistenceError> {
    if hash.bytes().all(|byte| byte != 0 && byte.is_ascii()) {
        Ok(hash)
    } else {
        Err(PersistenceError::decode(
            PASSWORD_HASH,
            DecodeKind::InvalidByte,
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{FixedRecord, PicX};
    use domain::batch::loads::HashedUser;
    use domain::records::{UserEntry, UserListEntry, UserSecurityRecord};

    use super::{UserListRow, UserRow};
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// `DUSRSECJ.jcl:35`, the first in-stream record, 57 bytes: its `SEC-USR-PWD` at offset 48
    /// is the demo password `README.md:203-204` publishes.
    const ADMIN001: &[u8; 57] = b"ADMIN001MARGARET            GOLD                PASSWORDA";

    /// The 8 `SEC-USR-PWD` bytes of [`ADMIN001`].
    const PLAINTEXT: &str = "PASSWORD";

    /// A fixed PHC-shaped string: no test calls argon2, and no column check depends on the
    /// hash being one `domain::password::hash` produced.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$ZmFrZWhhc2hmb3J0ZXN0c29ubHk";

    fn decode_error(column: &'static str, kind: DecodeKind) -> PersistenceError {
        PersistenceError::decode(column, kind)
    }

    /// [`ADMIN001`] padded to its 80-byte card image and decoded, as `dusrsecj` does
    /// (LOADS-012), with [`HASH`] as its hash.
    fn admin001() -> Result<HashedUser, Box<dyn StdError>> {
        let mut image = ADMIN001.to_vec();
        image.resize(UserSecurityRecord::WIDTH, b' ');
        Ok(HashedUser {
            record: UserSecurityRecord::decode(&image)?,
            password_hash: HASH.to_owned(),
        })
    }

    /// The entry [`admin001`] stores.
    fn admin001_entry() -> Result<UserEntry, Box<dyn StdError>> {
        Ok(UserEntry {
            id: PicX::from_text("ADMIN001")?,
            fname: PicX::from_text("MARGARET")?,
            lname: PicX::from_text("GOLD")?,
            password_hash: HASH.to_owned(),
            usr_type: PicX::from_text("A")?,
            filler: PicX::spaces(),
        })
    }

    /// A row read back from `users`, every column as given.
    fn row(usr_id: &str, usr_fname: &str, password_hash: &str) -> UserRow {
        UserRow {
            usr_id: usr_id.to_owned(),
            usr_fname: usr_fname.to_owned(),
            usr_lname: format!("{:<20}", "THOMAS"),
            password_hash: password_hash.to_owned(),
            usr_type: "U".to_owned(),
            filler: " ".repeat(23),
        }
    }

    /// A page row read back from `users`, every column as given.
    fn list_row(usr_id: &str, usr_lname: &str) -> UserListRow {
        UserListRow {
            usr_id: usr_id.to_owned(),
            usr_fname: format!("{:<20}", "LAWRENCE"),
            usr_lname: usr_lname.to_owned(),
            usr_type: "U".to_owned(),
        }
    }

    // audit: CSUSR01Y-001
    // audit: CSUSR01Y-002
    // audit: CSUSR01Y-003
    // audit: CSUSR01Y-004
    // audit: CSUSR01Y-005
    // audit: CSUSR01Y-006
    // audit: LOADS-033
    #[test]
    fn hashed_record_stores_its_hash_and_never_its_password() -> TestResult {
        let user = admin001()?;
        assert_eq!(user.record.pwd.expose(), PLAINTEXT.as_bytes());

        let row = UserRow::from_hashed(&user)?;
        assert_eq!(row.usr_id, "ADMIN001");
        assert_eq!(row.usr_fname, format!("{:<20}", "MARGARET"));
        assert_eq!(row.usr_lname, format!("{:<20}", "GOLD"));
        assert_eq!(row.password_hash, HASH);
        assert_eq!(row.usr_type, "A");
        assert_eq!(row.filler, " ".repeat(23));
        for column in [
            &row.usr_id,
            &row.usr_fname,
            &row.usr_lname,
            &row.password_hash,
            &row.usr_type,
            &row.filler,
        ] {
            assert!(!column.contains(PLAINTEXT), "{column}");
        }

        assert_eq!(row.into_entry()?, admin001_entry()?);
        Ok(())
    }

    // audit: PIC-001
    // audit: PIC-012
    #[test]
    fn entries_round_trip_through_their_row_byte_for_byte() -> TestResult {
        let padded = admin001_entry()?;
        let mut odd = padded.clone();
        odd.fname = PicX::from_array(*b" leading and trail ~");
        odd.usr_type = PicX::from_text("x")?;
        odd.filler = PicX::from_array(*b"0123456789ABCDEFGHIJKLM");
        for entry in [padded, odd] {
            let row = UserRow::from_entry(&entry)?;
            assert_eq!(row.usr_fname.as_bytes(), entry.fname.as_bytes());
            assert_eq!(row.filler.as_bytes(), entry.filler.as_bytes());
            assert_eq!(row.into_entry()?, entry);
        }
        Ok(())
    }

    #[test]
    fn row_values_that_do_not_fit_their_picture_are_typed_errors() {
        let user0001 = format!("{:<20}", "LAWRENCE");
        let short_id = row("USER001", &user0001, HASH);
        assert_eq!(
            short_id.into_entry().map(drop),
            Err(decode_error(
                "usr_id",
                DecodeKind::WidthMismatch {
                    expected: 8,
                    actual: 7,
                },
            ))
        );

        let accented = format!("{:<19}", "LAWRENC\u{e9}");
        assert_eq!(
            row("USER0001", &accented, HASH).into_entry().map(drop),
            Err(decode_error("usr_fname", DecodeKind::InvalidByte))
        );

        let nul_hash = format!("{HASH}\0");
        assert_eq!(
            row("USER0001", &user0001, &nul_hash).into_entry().map(drop),
            Err(decode_error("password_hash", DecodeKind::InvalidByte))
        );
        let non_ascii_hash = format!("{HASH}\u{e9}");
        assert_eq!(
            row("USER0001", &user0001, &non_ascii_hash)
                .into_entry()
                .map(drop),
            Err(decode_error("password_hash", DecodeKind::InvalidByte))
        );
    }

    #[test]
    fn entry_values_a_column_cannot_store_are_typed_errors() -> TestResult {
        let mut accented = admin001_entry()?;
        accented.lname = PicX::from_array(*b"GOLD\xC3\xA9              ");
        assert_eq!(
            UserRow::from_entry(&accented).map(drop),
            Err(decode_error("usr_lname", DecodeKind::InvalidByte))
        );

        let mut nul_hash = admin001_entry()?;
        nul_hash.password_hash = format!("{HASH}\0");
        assert_eq!(
            UserRow::from_entry(&nul_hash).map(drop),
            Err(decode_error("password_hash", DecodeKind::InvalidByte))
        );

        let mut user = admin001()?;
        user.password_hash = format!("\0{HASH}");
        assert_eq!(
            UserRow::from_hashed(&user).map(drop),
            Err(decode_error("password_hash", DecodeKind::InvalidByte))
        );
        Ok(())
    }

    /// Cardholder Data Redaction: an error names its column and class, never the hash.
    #[test]
    fn errors_never_repeat_the_hash() -> TestResult {
        let nul_hash = format!("{HASH}\0");
        let error = row("USER0001", &format!("{:<20}", "LAWRENCE"), &nul_hash)
            .into_entry()
            .err()
            .ok_or("a NUL in the hash must be rejected")?;
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(text.contains("password_hash"), "{text}");
            assert!(!text.contains("argon2id"), "{text}");
            assert!(!text.contains("19456"), "{text}");
        }
        Ok(())
    }

    // audit: COUSR00C-028
    #[test]
    fn page_rows_read_back_as_their_four_typed_fields() -> TestResult {
        let lname = format!("{:<20}", "THOMAS");
        assert_eq!(
            list_row("USER0001", &lname).into_entry()?,
            UserListEntry {
                id: PicX::from_text("USER0001")?,
                fname: PicX::from_text("LAWRENCE")?,
                lname: PicX::from_text("THOMAS")?,
                usr_type: PicX::from_text("U")?,
            }
        );

        assert_eq!(
            list_row("USER0001", "THOMAS").into_entry(),
            Err(decode_error(
                "usr_lname",
                DecodeKind::WidthMismatch {
                    expected: 20,
                    actual: 6,
                },
            ))
        );
        assert_eq!(
            list_row("USER\u{0}001", &lname).into_entry(),
            Err(decode_error("usr_id", DecodeKind::InvalidByte))
        );
        Ok(())
    }
}
