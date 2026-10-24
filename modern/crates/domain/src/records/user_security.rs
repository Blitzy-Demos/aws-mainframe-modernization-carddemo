//! `CSUSR01Y` `SEC-USER-DATA`: the 80-byte record of the `USRSEC` user-security file
//! (`app/cpy/CSUSR01Y.cpy:17-23`; copybook rows CSUSR01Y-001 to CSUSR01Y-006).
//!
//! | Field | COBOL name | Picture | Offset | Width | Rust type | Row |
//! |---|---|---|---|---|---|---|
//! | `id` | `SEC-USR-ID` | `PIC X(08)` | 0 | 8 | `PicX<8>` | CSUSR01Y-001 |
//! | `fname` | `SEC-USR-FNAME` | `PIC X(20)` | 8 | 20 | `PicX<20>` | CSUSR01Y-002 |
//! | `lname` | `SEC-USR-LNAME` | `PIC X(20)` | 28 | 20 | `PicX<20>` | CSUSR01Y-003 |
//! | `pwd` | `SEC-USR-PWD` | `PIC X(08)` | 48 | 8 | [`Password`] | CSUSR01Y-004 |
//! | `usr_type` | `SEC-USR-TYPE` | `PIC X(01)` | 56 | 1 | `PicX<1>` | CSUSR01Y-005 |
//! | `filler` | `SEC-USR-FILLER` | `PIC X(23)` | 57 | 23 | `PicX<23>` | CSUSR01Y-006 |
//!
//! The only records of this layout in the repository are the ten in-stream card images of
//! `app/jcl/DUSRSECJ.jcl:35-44` (FIXUSR-001 to FIXUSR-010). Each is stored as a 57-byte line that
//! ends where `SEC-USR-FILLER` begins. `dusrsecj` pads every line with spaces to the 80-byte
//! `LRECL=80` card image IEBGENER reads (LOADS-012) and decodes that, so codec A round-trips the
//! full 80 bytes (LOADS-034, i8). This module does no I/O: the deck is read by the batch job and
//! the tests.
//!
//! # The stored projections
//!
//! [`UserEntry`] and [`UserListEntry`] are the values codec B reads and writes for table
//! `users`: the user as the store holds it, and the four fields a COUSR00C page selects. Neither
//! has a byte form, because the store holds the argon2id PHC string in place of `SEC-USR-PWD`
//! (E25), so neither implements [`FixedRecord`].

use core::fmt;

use copybook::{Error, FixedRecord, PicX, RecordReader, RecordWriter};

use crate::sensitive::Password;

/// The `SEC-USR-TYPE` value of an administrator, the 88-level `CDEMO-USRTYP-ADMIN VALUE 'A'` of
/// the COMMAREA field sign-on copies it into (COCOM01Y-006, COSGN00C-015).
const ADMIN_TYPE: &[u8; 1] = b"A";

/// One user of the `USRSEC` file, `SEC-USER-DATA` (`CSUSR01Y`, 80 bytes, `RECORDSIZE(80,80)`).
///
/// Fields are the copybook's, in its order, and every byte is kept: `PIC X(n)` padding
/// (PIC-001) and the `SEC-USR-FILLER` bytes, whatever they hold (PIC-012).
///
/// `SEC-USR-PWD` is a [`Password`], so the derived `Debug` prints `********` in its place and the
/// record has no `Serialize` or `Display` (PIC-024, Cardholder Data Redaction). Codec A reaches
/// the 8 raw bytes only through [`Password::expose`] in [`FixedRecord::encode`]. Codec B never
/// stores them: `persistence` writes only the argon2id PHC string `domain::password::hash`
/// computes over those bytes into `users.password_hash`, so that field alone is not
/// byte-identical through the database (E25, LOADS-033).
///
/// `SEC-USR-TYPE` is the admin gate: `A` is an administrator, whose sign-on transfers to the
/// admin menu and whose token passes `AdminUser`; every other value, `U` included, is a regular
/// user (CSUSR01Y-005, COSGN00C-015, COSGN00C-016). [`UserSecurityRecord::is_admin`] states that
/// test once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserSecurityRecord {
    /// `SEC-USR-ID PIC X(08)`, offset 0 (CSUSR01Y-001): the user ID, primary key
    /// `users.usr_id` = KSDS `KEYS(8,0)`.
    pub id: PicX<8>,
    /// `SEC-USR-FNAME PIC X(20)`, offset 8 (CSUSR01Y-002): first name, padding kept.
    pub fname: PicX<20>,
    /// `SEC-USR-LNAME PIC X(20)`, offset 28 (CSUSR01Y-003): last name, padding kept.
    pub lname: PicX<20>,
    /// `SEC-USR-PWD PIC X(08)`, offset 48 (CSUSR01Y-004): the password exactly as stored, case
    /// and padding included (E26). No column holds it; the store keeps only its hash (E25).
    pub pwd: Password,
    /// `SEC-USR-TYPE PIC X(01)`, offset 56 (CSUSR01Y-005): `A` administrator, `U` user.
    pub usr_type: PicX<1>,
    /// `SEC-USR-FILLER PIC X(23)`, offset 57 (CSUSR01Y-006): a named `FILLER` no program
    /// references, kept byte for byte (PIC-012); 23 spaces in every padded in-stream record.
    pub filler: PicX<23>,
}

impl UserSecurityRecord {
    /// Whether `SEC-USR-TYPE` is `A`, the condition `CDEMO-USRTYP-ADMIN` on which sign-on
    /// transfers to the admin menu (COSGN00C-015). A space, `U` or any other byte is a regular
    /// user (COSGN00C-016).
    #[must_use]
    pub fn is_admin(&self) -> bool {
        self.usr_type.as_bytes() == ADMIN_TYPE
    }
}

impl FixedRecord for UserSecurityRecord {
    /// `RECORDSIZE(80,80)` of the `USRSEC` cluster and `LRECL=80` of its sequential copy
    /// (`DUSRSECJ.jcl:48,66`; CSUSR01Y-006).
    const WIDTH: usize = 80;

    /// `SEC-USR-FILLER PIC X(23)` ends the record (CSUSR01Y-006), so a line body of 57 to 80
    /// bytes frames as this record with its missing tail decoded as spaces, and one of 56 bytes
    /// or fewer, which would end inside `SEC-USR-TYPE`, is a typed error (PIC-013). A 57-byte
    /// in-stream line framed this way decodes to the same record as its 80-byte padded card
    /// image (LOADS-012). `dusrsecj` still pads before decoding, so the value states the layout
    /// and changes nothing the load stores; it is not 0 even though no fixture file needs it.
    const TRAILING_FILLER: usize = 23;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            id: r.pic_x("SEC-USR-ID")?,
            fname: r.pic_x("SEC-USR-FNAME")?,
            lname: r.pic_x("SEC-USR-LNAME")?,
            pwd: Password::new(r.pic_x("SEC-USR-PWD")?),
            usr_type: r.pic_x("SEC-USR-TYPE")?,
            filler: r.pic_x("SEC-USR-FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.id)
            .pic_x(&self.fname)
            .pic_x(&self.lname)
            .bytes(self.pwd.expose())
            .pic_x(&self.usr_type)
            .pic_x(&self.filler);
        w.finish()
    }
}

/// A user as the store holds it: the `SEC-USER-DATA` fields other than `SEC-USR-PWD`, and the
/// argon2id PHC string that stands for it (CSUSR01Y-001 to CSUSR01Y-006, E25).
///
/// It is the stored projection codec B reads and writes for a `users` row. It has no byte form:
/// the store holds the argon2id PHC string in place of `SEC-USR-PWD`.
///
/// `Debug` prints `<redacted>` in place of the hash; no `Display` or `Serialize` exists
/// (PIC-024, Cardholder Data Redaction).
#[derive(Clone, PartialEq, Eq)]
pub struct UserEntry {
    /// `SEC-USR-ID PIC X(08)`, the key (CSUSR01Y-001).
    pub id: PicX<8>,
    /// `SEC-USR-FNAME PIC X(20)`, padding kept (CSUSR01Y-002).
    pub fname: PicX<20>,
    /// `SEC-USR-LNAME PIC X(20)`, padding kept (CSUSR01Y-003).
    pub lname: PicX<20>,
    /// The argon2id PHC string over the 8 `SEC-USR-PWD` bytes, beginning `$argon2id$`
    /// (CSUSR01Y-004, LOADS-033, E25). Sign-on and the update's change check verify a password
    /// against it with `crate::password::verify` (E26, E27).
    pub password_hash: String,
    /// `SEC-USR-TYPE PIC X(01)`: `A` administrator, `U` user (CSUSR01Y-005).
    pub usr_type: PicX<1>,
    /// `SEC-USR-FILLER PIC X(23)`, kept byte for byte; a rewrite stores it unchanged
    /// (CSUSR01Y-006, PIC-012).
    pub filler: PicX<23>,
}

impl fmt::Debug for UserEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UserEntry")
            .field("id", &self.id)
            .field("fname", &self.fname)
            .field("lname", &self.lname)
            .field("password_hash", &format_args!("<redacted>"))
            .field("usr_type", &self.usr_type)
            .field("filler", &self.filler)
            .finish()
    }
}

/// A user as a COUSR00C page lists it: the four fields `POPULATE-USER-DATA` moves to the map, as
/// stored, padding kept (COUSR00C-028). It has no hash field.
///
/// It is the stored projection codec B reads from the page queries, and has no byte form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserListEntry {
    /// `SEC-USR-ID PIC X(08)` (CSUSR01Y-001).
    pub id: PicX<8>,
    /// `SEC-USR-FNAME PIC X(20)` (CSUSR01Y-002).
    pub fname: PicX<20>,
    /// `SEC-USR-LNAME PIC X(20)` (CSUSR01Y-003).
    pub lname: PicX<20>,
    /// `SEC-USR-TYPE PIC X(01)` (CSUSR01Y-005).
    pub usr_type: PicX<1>,
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, Framed, Line, LineFraming, PicX, Terminator};

    use super::{UserEntry, UserSecurityRecord};
    use crate::sensitive::Password;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// `DUSRSECJ.jcl:35`, the first in-stream record: 57 bytes, 8 + 20 + 20 + 8 + 1, ending where
    /// `SEC-USR-FILLER` begins. Its password is the demo value `README.md:203-204` publishes.
    const ADMIN001: &[u8; 57] = b"ADMIN001MARGARET            GOLD                PASSWORDA";

    /// The 8 bytes of `SEC-USR-PWD` in [`ADMIN001`], at offset 48.
    const ADMIN001_PWD: &[u8; 8] = b"PASSWORD";

    /// A fixed PHC-shaped string: no test here calls argon2, and nothing checked depends on the
    /// hash being one `crate::password::hash` produced.
    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$ZmFrZWhhc2hmb3J0ZXN0c29ubHk";

    /// `line` padded with spaces to the 80-byte card image `dusrsecj` decodes (LOADS-012).
    fn card_image(line: &[u8]) -> Vec<u8> {
        let mut image = line.to_vec();
        image.resize(UserSecurityRecord::WIDTH, b' ');
        image
    }

    /// `bytes` with the byte at `offset` replaced by `byte`.
    fn with_byte(bytes: &[u8], offset: usize, byte: u8) -> Vec<u8> {
        bytes
            .iter()
            .enumerate()
            .map(|(at, &old)| if at == offset { byte } else { old })
            .collect()
    }

    // audit: CSUSR01Y-001
    // audit: CSUSR01Y-002
    // audit: CSUSR01Y-003
    // audit: CSUSR01Y-004
    // audit: CSUSR01Y-005
    // audit: CSUSR01Y-006
    // audit: FIXUSR-001
    // audit: PIC-013
    #[test]
    fn admin001_line_frames_at_57_bytes_and_re_encodes_identically() -> TestResult {
        let framed = Framed::<UserSecurityRecord>::decode(Line::new(ADMIN001, Terminator::Lf))?;
        assert_eq!(
            framed.framing,
            LineFraming {
                terminator: Terminator::Lf,
                body_len: 57,
            }
        );

        let record = &framed.record;
        assert_eq!(record.id.as_bytes(), b"ADMIN001");
        assert_eq!(record.fname.as_bytes(), b"MARGARET            ");
        assert_eq!(record.lname.as_bytes(), b"GOLD                ");
        assert_eq!(record.pwd.expose(), ADMIN001_PWD);
        assert_eq!(record.usr_type.as_bytes(), b"A");
        assert!(record.filler.is_spaces());

        let mut line = ADMIN001.to_vec();
        line.push(b'\n');
        assert_eq!(framed.encode(), line);
        assert_eq!(framed.try_encode()?, line);
        Ok(())
    }

    // audit: FIXUSR-001
    // audit: LOADS-012
    // audit: LOADS-034
    #[test]
    fn padded_card_image_decodes_to_the_framed_record_and_re_encodes_80_bytes() -> TestResult {
        let image = card_image(ADMIN001);
        let record = UserSecurityRecord::decode(&image)?;
        let framed = Framed::<UserSecurityRecord>::decode(Line::new(ADMIN001, Terminator::Lf))?;
        assert_eq!(record, framed.record);

        let bytes = record.encode();
        assert_eq!(bytes.len(), UserSecurityRecord::WIDTH);
        assert_eq!(bytes, image);
        Ok(())
    }

    // audit: PIC-012
    #[test]
    fn filler_bytes_are_kept_verbatim() -> TestResult {
        let mut image = ADMIN001.to_vec();
        image.extend_from_slice(b"00000000000000000000000");
        let record = UserSecurityRecord::decode(&image)?;
        assert_eq!(record.filler.as_bytes(), b"00000000000000000000000");
        assert_eq!(record.encode(), image);

        // A 57-byte framing would drop that FILLER, so the checked encoder refuses it.
        let framed = Framed {
            record,
            framing: LineFraming {
                terminator: Terminator::Lf,
                body_len: 57,
            },
        };
        assert_eq!(
            framed.try_encode(),
            Err(Error::new("record", 57, 80, ErrorKind::NonBlankTail))
        );
        Ok(())
    }

    // audit: PIC-024
    #[test]
    fn debug_never_shows_the_password() -> TestResult {
        let framed = Framed::<UserSecurityRecord>::decode(Line::new(ADMIN001, Terminator::Lf))?;
        for text in [format!("{:?}", framed.record), format!("{framed:?}")] {
            assert!(!text.contains("PASSWORD"), "{text}");
            assert!(text.contains("********"), "{text}");
            assert!(text.contains("ADMIN001"), "{text}");
        }
        Ok(())
    }

    // audit: PIC-024
    #[test]
    fn entry_debug_never_shows_the_hash() -> TestResult {
        let entry = UserEntry {
            id: PicX::from_text("ADMIN001")?,
            fname: PicX::from_text("MARGARET")?,
            lname: PicX::from_text("GOLD")?,
            password_hash: HASH.to_owned(),
            usr_type: PicX::from_text("A")?,
            filler: PicX::spaces(),
        };
        let text = format!("{entry:?}");
        assert!(!text.contains(HASH), "{text}");
        assert!(!text.contains("argon2id"), "{text}");
        assert!(text.contains("<redacted>"), "{text}");
        for shown in ["ADMIN001", "MARGARET", "GOLD", "usr_type: PicX(\"A\")"] {
            assert!(text.contains(shown), "{text}");
        }
        Ok(())
    }

    // audit: PIC-013
    #[test]
    fn body_ending_inside_sec_usr_type_is_short_body_not_filler() -> TestResult {
        let body = ADMIN001.get(..56).ok_or("ADMIN001 holds 57 bytes")?;
        assert_eq!(
            Framed::<UserSecurityRecord>::decode(Line::new(body, Terminator::Lf)),
            Err(Error::new(
                "record",
                0,
                80,
                ErrorKind::ShortBodyNotFiller {
                    expected: 80,
                    actual: 56,
                },
            ))
        );
        Ok(())
    }

    #[test]
    fn any_length_but_80_is_a_record_width_mismatch() {
        for len in [0, 1, 56, 57, 79, 81, 1024] {
            let bytes = vec![b' '; len];
            assert_eq!(
                UserSecurityRecord::decode(&bytes),
                Err(Error::new(
                    "record",
                    0,
                    80,
                    ErrorKind::WidthMismatch {
                        expected: 80,
                        actual: len,
                    },
                )),
                "{len} bytes"
            );
        }
        let long = vec![b' '; 81];
        assert_eq!(
            Framed::<UserSecurityRecord>::decode(Line::new(&long, Terminator::Lf)),
            Err(Error::new(
                "record",
                0,
                80,
                ErrorKind::WidthMismatch {
                    expected: 80,
                    actual: 81,
                },
            ))
        );
    }

    // audit: PIC-002
    #[test]
    fn a_non_printable_byte_names_its_field_offset_and_width() {
        let image = card_image(ADMIN001);
        // (offset of the bad byte, field it falls in, field offset, field width)
        let cases = [
            (0, "SEC-USR-ID", 0, 8),
            (27, "SEC-USR-FNAME", 8, 20),
            (28, "SEC-USR-LNAME", 28, 20),
            (55, "SEC-USR-PWD", 48, 8),
            (56, "SEC-USR-TYPE", 56, 1),
            (79, "SEC-USR-FILLER", 57, 23),
        ];
        for (offset, field, field_offset, width) in cases {
            let expected = Error::new(field, field_offset, width, ErrorKind::NonPrintable);
            for byte in [b'\t', b'\r', 0x00, 0x7F, 0xC1] {
                assert_eq!(
                    UserSecurityRecord::decode(&with_byte(&image, offset, byte)),
                    Err(expected),
                    "byte {byte:#04x} at offset {offset}"
                );
            }
        }
    }

    // audit: COSGN00C-015
    // audit: COSGN00C-016
    #[test]
    fn only_user_type_a_is_an_administrator() -> TestResult {
        let admin = UserSecurityRecord::decode(&card_image(ADMIN001))?;
        for (usr_type, admin_expected) in
            [(b'A', true), (b'U', false), (b' ', false), (b'a', false)]
        {
            let record = UserSecurityRecord {
                usr_type: PicX::from_array([usr_type]),
                ..admin.clone()
            };
            assert_eq!(
                record.is_admin(),
                admin_expected,
                "type {}",
                char::from(usr_type)
            );
        }
        Ok(())
    }

    #[test]
    fn records_built_from_values_encode_their_fields_in_copybook_order() -> TestResult {
        let record = UserSecurityRecord {
            id: PicX::from_text("USER0001")?,
            fname: PicX::from_text("LAWRENCE")?,
            lname: PicX::from_text("THOMAS")?,
            pwd: Password::from_input("pwd1")?,
            usr_type: PicX::from_text("U")?,
            filler: PicX::spaces(),
        };
        let mut expected = b"USER0001LAWRENCE            THOMAS              pwd1    U".to_vec();
        expected.resize(80, b' ');
        assert_eq!(record.encode(), expected);
        assert_eq!(UserSecurityRecord::decode(&expected)?, record);
        assert!(!record.is_admin());
        Ok(())
    }
}
