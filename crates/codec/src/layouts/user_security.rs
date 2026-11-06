//! `CSUSR01Y` `SEC-USER-DATA`: the 80-byte `USRSEC` user security record
//! [app/cpy/CSUSR01Y.cpy:L17-23].
//!
//! `DUSRSECJ` builds `USRSEC` from its ten instream `SYSUT1` rows
//! [app/jcl/DUSRSECJ.jcl:L35-44] with `IEBGENER` at `LRECL=80`
//! [app/jcl/DUSRSECJ.jcl:L48]; the EBCDIC counterpart is
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.USRSEC.PS`, the same ten rows as 80-byte
//! CP037 records. `batch seed` decodes each row with [`UserSecurityLayout`]
//! and hashes `SEC-USR-PWD` through the domain password policy.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `SEC-USR-ID` | `X(08)` | 0 | 8 | P-CSUSR01Y-001 | `UserId` |
//! | `SEC-USR-FNAME` | `X(20)` | 8 | 20 | P-CSUSR01Y-002 | `Alnum<20>` |
//! | `SEC-USR-LNAME` | `X(20)` | 28 | 20 | P-CSUSR01Y-003 | `Alnum<20>` |
//! | `SEC-USR-PWD` | `X(08)` | 48 | 8 | P-CSUSR01Y-004 | `Alnum<8>` |
//! | `SEC-USR-TYPE` | `X(01)` | 56 | 1 | P-CSUSR01Y-005 | `Alnum<1>` |
//! | `SEC-USR-FILLER` | `X(23)` | 57 | 23 | P-CSUSR01Y-006 | `Alnum<23>`, raw |
//!
//! # The password field
//!
//! [`UserSecurityRecord`] is this codec's own record, not the domain entity
//! `entities::UserSecurity`: the entity holds the argon2id hash that replaces
//! `SEC-USR-PWD` (P-CSUSR01Y-004, E-20), while the seed needs the 8 stored
//! bytes that hash is computed from. Every field, the password included, is
//! held exactly as stored: no case change, no trimming, because E-20 hashes
//! exactly those 8 space-padded bytes, case unchanged.
//!
//! The record serves the seed only and lives only between decoding a row and
//! hashing its password. Its `Debug` leaves the password out, so the plaintext
//! never reaches a log line or a tracing span.

use core::fmt;

use carddemo_domain::numeric::{Alnum, UserId};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec};

/// `CSUSR01Y` `SEC-USR-ID PIC X(08)`, the `USRSEC` key — P-CSUSR01Y-001
/// [app/cpy/CSUSR01Y.cpy:L18].
pub const SEC_USR_ID: FieldSpec = FieldSpec::new("SEC-USR-ID", 0, 8, FieldKind::Alnum);

/// `CSUSR01Y` `SEC-USR-FNAME PIC X(20)` — P-CSUSR01Y-002
/// [app/cpy/CSUSR01Y.cpy:L19].
pub const SEC_USR_FNAME: FieldSpec = FieldSpec::new("SEC-USR-FNAME", 8, 20, FieldKind::Alnum);

/// `CSUSR01Y` `SEC-USR-LNAME PIC X(20)` — P-CSUSR01Y-003
/// [app/cpy/CSUSR01Y.cpy:L20].
pub const SEC_USR_LNAME: FieldSpec = FieldSpec::new("SEC-USR-LNAME", 28, 20, FieldKind::Alnum);

/// `CSUSR01Y` `SEC-USR-PWD PIC X(08)` — P-CSUSR01Y-004
/// [app/cpy/CSUSR01Y.cpy:L21].
pub const SEC_USR_PWD: FieldSpec = FieldSpec::new("SEC-USR-PWD", 48, 8, FieldKind::Alnum);

/// `CSUSR01Y` `SEC-USR-TYPE PIC X(01)`: `A` administrator, `U` user —
/// P-CSUSR01Y-005 [app/cpy/CSUSR01Y.cpy:L22].
pub const SEC_USR_TYPE: FieldSpec = FieldSpec::new("SEC-USR-TYPE", 56, 1, FieldKind::Alnum);

/// `CSUSR01Y` `SEC-USR-FILLER PIC X(23)`, kept raw — P-CSUSR01Y-006
/// [app/cpy/CSUSR01Y.cpy:L23].
pub const SEC_USR_FILLER: FieldSpec = FieldSpec::new("SEC-USR-FILLER", 57, 23, FieldKind::Filler);

/// The `CSUSR01Y` `SEC-USER-DATA` layout, 80 bytes
/// [app/cpy/CSUSR01Y.cpy:L17-23].
pub struct UserSecurityLayout;

/// One decoded `USRSEC` record, every field as stored.
///
/// `Debug` omits [`UserSecurityRecord::sec_usr_pwd`].
#[derive(Clone, PartialEq, Eq)]
pub struct UserSecurityRecord {
    /// `SEC-USR-ID PIC X(08)` — P-CSUSR01Y-001 [app/cpy/CSUSR01Y.cpy:L18].
    pub sec_usr_id: UserId,
    /// `SEC-USR-FNAME PIC X(20)` — P-CSUSR01Y-002 [app/cpy/CSUSR01Y.cpy:L19].
    pub sec_usr_fname: Alnum<20>,
    /// `SEC-USR-LNAME PIC X(20)` — P-CSUSR01Y-003 [app/cpy/CSUSR01Y.cpy:L20].
    pub sec_usr_lname: Alnum<20>,
    /// `SEC-USR-PWD PIC X(08)`, the plaintext password as stored —
    /// P-CSUSR01Y-004 [app/cpy/CSUSR01Y.cpy:L21].
    ///
    /// The audit row types the persisted field as the argon2id hash; that hash
    /// is the domain entity's, computed by the seed from these 8 bytes, so the
    /// codec keeps the bytes themselves.
    pub sec_usr_pwd: Alnum<8>,
    /// `SEC-USR-TYPE PIC X(01)` — P-CSUSR01Y-005 [app/cpy/CSUSR01Y.cpy:L22].
    pub sec_usr_type: Alnum<1>,
    /// `SEC-USR-FILLER PIC X(23)`, its stored bytes — P-CSUSR01Y-006
    /// [app/cpy/CSUSR01Y.cpy:L23].
    pub filler: Alnum<23>,
}

/// Every field but the password, which stays out of every diagnostic.
impl fmt::Debug for UserSecurityRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("UserSecurityRecord")
            .field("sec_usr_id", &self.sec_usr_id)
            .field("sec_usr_fname", &self.sec_usr_fname)
            .field("sec_usr_lname", &self.sec_usr_lname)
            .field("sec_usr_type", &self.sec_usr_type)
            .field("filler", &self.filler)
            .finish_non_exhaustive()
    }
}

impl Layout for UserSecurityLayout {
    const LEN: usize = 80;
    const NAME: &'static str = "CSUSR01Y";
    const FIELDS: &'static [FieldSpec] = &[
        SEC_USR_ID,
        SEC_USR_FNAME,
        SEC_USR_LNAME,
        SEC_USR_PWD,
        SEC_USR_TYPE,
        SEC_USR_FILLER,
    ];
    type Record = UserSecurityRecord;

    /// Reads `SEC-USR-ID` through `SEC-USR-FILLER` (P-CSUSR01Y-001 to
    /// P-CSUSR01Y-006) [app/cpy/CSUSR01Y.cpy:L18-23], each as its stored
    /// bytes.
    ///
    /// # Errors
    ///
    /// [`CodecErrorKind::Length`] unless `bytes` is exactly 80 bytes long.
    fn decode(bytes: &[u8], charset: Charset) -> Result<Self::Record, CodecError> {
        if bytes.len() != Self::LEN {
            return Err(CodecError::new(
                Self::NAME,
                CodecErrorKind::Length {
                    expected: Self::LEN,
                    actual: bytes.len(),
                },
            ));
        }
        let rec = charset.to_internal(bytes);
        Ok(UserSecurityRecord {
            sec_usr_id: UserId::from_alnum(field::alnum(Self::NAME, &rec, &SEC_USR_ID)?),
            sec_usr_fname: field::alnum(Self::NAME, &rec, &SEC_USR_FNAME)?,
            sec_usr_lname: field::alnum(Self::NAME, &rec, &SEC_USR_LNAME)?,
            sec_usr_pwd: field::alnum(Self::NAME, &rec, &SEC_USR_PWD)?,
            sec_usr_type: field::alnum(Self::NAME, &rec, &SEC_USR_TYPE)?,
            filler: field::alnum(Self::NAME, &rec, &SEC_USR_FILLER)?,
        })
    }

    /// Writes `SEC-USR-ID` through `SEC-USR-FILLER` (P-CSUSR01Y-001 to
    /// P-CSUSR01Y-006) [app/cpy/CSUSR01Y.cpy:L18-23] from the bytes the
    /// record holds, then translates the 80 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let mut out = vec![b' '; Self::LEN];
        field::put_alnum(&mut out, &SEC_USR_ID, record.sec_usr_id.as_bytes());
        field::put_alnum(&mut out, &SEC_USR_FNAME, record.sec_usr_fname.as_bytes());
        field::put_alnum(&mut out, &SEC_USR_LNAME, record.sec_usr_lname.as_bytes());
        field::put_alnum(&mut out, &SEC_USR_PWD, record.sec_usr_pwd.as_bytes());
        field::put_alnum(&mut out, &SEC_USR_TYPE, record.sec_usr_type.as_bytes());
        field::put_alnum(&mut out, &SEC_USR_FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::numeric::{Alnum, UserId};

    use super::{UserSecurityLayout, UserSecurityRecord};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, field};

    /// The `DUSRSECJ` job, whose `SYSUT1` rows are the `USRSEC` records.
    const DUSRSECJ: &str = include_str!("../../../../app/jcl/DUSRSECJ.jcl");

    /// The EBCDIC `USRSEC` file: the same ten rows as 80-byte CP037 records.
    const USRSEC_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.USRSEC.PS");

    /// The physical lines of the ten `SYSUT1` rows [app/jcl/DUSRSECJ.jcl:L35-44].
    const SYSUT1_LINES: core::ops::RangeInclusive<usize> = 35..=44;

    /// The `SYSUT1` row on physical line `line` of `DUSRSECJ`, padded with
    /// spaces to the 80-byte card image `IEBGENER` copies.
    fn sysut1_row(line: usize) -> Vec<u8> {
        let text = DUSRSECJ
            .lines()
            .nth(line - 1)
            .unwrap_or_else(|| panic!("DUSRSECJ has no line {line}"));
        let mut row = text.as_bytes().to_vec();
        row.resize(UserSecurityLayout::LEN, b' ');
        row
    }

    #[test]
    fn fields_tile_the_80_byte_record() {
        assert_eq!(
            (UserSecurityLayout::NAME, UserSecurityLayout::LEN),
            ("CSUSR01Y", 80)
        );
        assert!(field::fields_are_contiguous(UserSecurityLayout::FIELDS, 80));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_80_bytes() {
        for len in [0, 79, 81] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    UserSecurityLayout::decode(&vec![b' '; len], charset),
                    Err(CodecError::new(
                        "CSUSR01Y",
                        CodecErrorKind::Length {
                            expected: 80,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn admin001_row_decodes_to_its_fields_and_re_encodes_to_its_bytes() -> Result<(), CodecError> {
        let row = sysut1_row(35);
        assert_eq!(
            row.get(..57),
            Some(&b"ADMIN001MARGARET            GOLD                PASSWORDA"[..])
        );
        let record = UserSecurityLayout::decode(&row, Charset::Ascii)?;
        assert_eq!(record.sec_usr_id, UserId::move_from("ADMIN001"));
        assert_eq!(record.sec_usr_fname, Alnum::move_from("MARGARET"));
        assert_eq!(record.sec_usr_lname, Alnum::move_from("GOLD"));
        assert_eq!(record.sec_usr_pwd.as_bytes(), b"PASSWORD");
        assert_eq!(record.sec_usr_type.as_bytes(), b"A");
        assert_eq!(record.filler, Alnum::spaces());
        assert_eq!(UserSecurityLayout::encode(&record, Charset::Ascii), row);
        Ok(())
    }

    #[test]
    fn every_sysut1_row_is_its_ebcdic_usrsec_record() -> Result<(), CodecError> {
        let records: Vec<&[u8]> = USRSEC_EBCDIC.chunks(UserSecurityLayout::LEN).collect();
        assert_eq!(records.len(), SYSUT1_LINES.count());
        for (line, stored) in SYSUT1_LINES.zip(records) {
            let record = UserSecurityLayout::decode(&sysut1_row(line), Charset::Ascii)?;
            assert_eq!(
                UserSecurityLayout::encode(&record, Charset::Ebcdic),
                stored,
                "DUSRSECJ line {line}"
            );
            assert_eq!(
                UserSecurityLayout::decode(stored, Charset::Ebcdic)?,
                record,
                "DUSRSECJ line {line}"
            );
        }
        Ok(())
    }

    #[test]
    fn stored_bytes_round_trip_and_debug_hides_the_password() -> Result<(), CodecError> {
        let record = UserSecurityRecord {
            sec_usr_id: UserId::move_from("user0042"),
            sec_usr_fname: Alnum::move_from("Ada"),
            sec_usr_lname: Alnum::move_from("Lovelace"),
            sec_usr_pwd: Alnum::move_from("SECRET01"),
            sec_usr_type: Alnum::move_from("U"),
            filler: Alnum::from_array(*b"0123456789ABCDEFGHIJKLM"),
        };
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = UserSecurityLayout::encode(&record, charset);
            assert_eq!(bytes.len(), 80, "{charset:?}");
            assert_eq!(
                UserSecurityLayout::decode(&bytes, charset)?,
                record,
                "{charset:?}"
            );
        }
        let ascii = UserSecurityLayout::encode(&record, Charset::Ascii);
        assert_eq!(ascii.get(48..56), Some(&b"SECRET01"[..]));
        let debug = format!("{record:?}");
        assert!(!debug.contains("SECRET01"), "{debug}");
        assert!(!debug.contains("sec_usr_pwd"), "{debug}");
        assert!(
            debug.contains("user0042") && debug.ends_with(".. }"),
            "{debug}"
        );
        Ok(())
    }
}
