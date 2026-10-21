//! Codec B for table `transaction_categories` (migration 0001): the VSAM data version of the
//! transaction categories, one `CVTRA04Y` `TRAN-CAT-RECORD` per row (`CVTRA04Y.cpy:4-9`;
//! CVTRA04Y-001 to CVTRA04Y-004).
//!
//! | Column | Type | Field | PIC | Conversion |
//! |---|---|---|---|---|
//! | `tran_type_cd` | `CHAR(2)` | `TRAN-TYPE-CD` | `X(02)` | every byte kept (CVTRA04Y-001, PIC-001) |
//! | `tran_cat_cd` | `BIGINT CHECK (tran_cat_cd BETWEEN 0 AND 9999)` | `TRAN-CAT-CD` | `9(04)` | checked, both ways (CVTRA04Y-002, PIC-003, PIC-023) |
//! | `tran_cat_type_desc` | `CHAR(50)` | `TRAN-CAT-TYPE-DESC` | `X(50)` | every byte kept, padding included (CVTRA04Y-003, PIC-001) |
//! | `filler` | `CHAR(4)` | `FILLER` | `X(04)` | every byte kept, never blanked (CVTRA04Y-004, PIC-012, E05) |
//!
//! The primary key (`tran_type_cd`, `tran_cat_cd`) is `TRAN-CAT-KEY`, the KSDS `KEYS(6 0)` that
//! IDCAMS REPRO loads from `TRANCATG.PS` (`TRANCATG.jcl:40,54-61`). No foreign key joins it to
//! `transaction_types`, as VSAM declares none. The CRLF that ends every `trancatg.txt` line (E06)
//! belongs to the line's [`copybook::LineFraming`] and is never stored (PIC-014).
//!
//! Only this VSAM version lives here (E23). The Db2 version, `db2.transaction_type_category`,
//! has its own row type in `db2_transaction_type_categories` over
//! [`Db2TransactionCategory`](domain::records::Db2TransactionCategory), and nothing converts
//! between the two: no component copies between the versions, so a Db2 maintenance change never
//! reaches the `tranrept` lookups that read this table.

use domain::records::TransactionCategoryRecord;

use super::{i64_to_pic_u, pic_u_to_i64, pic_x_to_string, string_to_pic_x};
use crate::error::PersistenceError;

/// Column of `TRAN-TYPE-CD` (CVTRA04Y-001).
const TRAN_TYPE_CD: &str = "tran_type_cd";
/// Column of `TRAN-CAT-CD` (CVTRA04Y-002).
const TRAN_CAT_CD: &str = "tran_cat_cd";
/// Column of `TRAN-CAT-TYPE-DESC` (CVTRA04Y-003).
const TRAN_CAT_TYPE_DESC: &str = "tran_cat_type_desc";
/// Column of the record's `FILLER` (CVTRA04Y-004).
const FILLER: &str = "filler";

/// One row of `transaction_categories`, the stored form of a [`TransactionCategoryRecord`].
///
/// Each field is the column of the same name and type in migration 0001, so the repository's
/// query macros build and bind it by column name. `Debug` is left underived, as for every row
/// struct (`rows` module documentation).
#[derive(Clone, PartialEq, Eq)]
pub struct TransactionCategoryRow {
    /// `tran_type_cd CHAR(2)`: the two bytes of `TRAN-TYPE-CD PIC X(02)`, the first part of the
    /// primary key (CVTRA04Y-001).
    pub(crate) tran_type_cd: String,
    /// `tran_cat_cd BIGINT`: the value of `TRAN-CAT-CD PIC 9(04)`, the second part of the
    /// primary key. Its check constraint holds it to `0..=9999`, the four digits of the picture
    /// (CVTRA04Y-002, PIC-003).
    pub(crate) tran_cat_cd: i64,
    /// `tran_cat_type_desc CHAR(50)`: the 50 bytes of `TRAN-CAT-TYPE-DESC PIC X(50)`, its space
    /// padding included, never trimmed (CVTRA04Y-003, PIC-001).
    pub(crate) tran_cat_type_desc: String,
    /// `filler CHAR(4)`: the four bytes of the record's `FILLER PIC X(04)` as read, the `0000`
    /// that every fixture record carries included (CVTRA04Y-004, PIC-012, E05).
    pub(crate) filler: String,
}

// The conversions are public while the fields stay crate-only: the row is re-exported from
// `rows`, and a crate-only conversion that no repository calls yet is dead code, which
// `warnings = "deny"` turns into a build error that no lint attribute may silence (R15).
impl TransactionCategoryRow {
    /// The row that stores `record`, every byte of its three `PIC X` fields kept and its category
    /// code held in the 64-bit `BIGINT` column (CVTRA04Y-001 to CVTRA04Y-004, PIC-023).
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte), naming the column,
    /// for a NUL or non-ASCII byte in `TRAN-TYPE-CD`, `TRAN-CAT-TYPE-DESC` or `FILLER`, which no
    /// `CHAR(n)` column stores byte for byte (PIC-002). A record codec A decoded never holds one;
    /// the error stands for a record built from bytes that codec A never checked.
    /// [`DecodeKind::OutOfRange`](crate::error::DecodeKind::OutOfRange) for a category code
    /// beyond `i64::MAX`, which no `PicU<4>` value reaches; the conversion is checked rather than
    /// assumed (Checked Integer Narrowing).
    pub fn from_record(record: &TransactionCategoryRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            tran_type_cd: pic_x_to_string(TRAN_TYPE_CD, &record.type_cd)?,
            tran_cat_cd: pic_u_to_i64(TRAN_CAT_CD, record.cat_cd)?,
            tran_cat_type_desc: pic_x_to_string(TRAN_CAT_TYPE_DESC, &record.type_desc)?,
            filler: pic_x_to_string(FILLER, &record.filler)?,
        })
    }

    /// The record this row stores, equal to the one [`Self::from_record`] was given, so codec A
    /// re-encodes it to the original 60 bytes (binding d9).
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the first column, in field order, whose value does
    /// not fit its picture. Nothing is truncated, padded or trimmed to make it fit (R7):
    ///
    /// - [`DecodeKind::OutOfRange`](crate::error::DecodeKind::OutOfRange) for a `tran_cat_cd`
    ///   that is negative or above 9999, which `PIC 9(04)` cannot hold (CVTRA04Y-002).
    /// - [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) for a
    ///   `tran_type_cd`, `tran_cat_type_desc` or `filler` text that is not exactly 2, 50 or 4
    ///   bytes long.
    /// - [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or
    ///   non-ASCII character in any of those three columns (PIC-002).
    pub fn into_record(self) -> Result<TransactionCategoryRecord, PersistenceError> {
        Ok(TransactionCategoryRecord {
            type_cd: string_to_pic_x(TRAN_TYPE_CD, &self.tran_type_cd)?,
            cat_cd: i64_to_pic_u::<4>(TRAN_CAT_CD, self.tran_cat_cd)?,
            type_desc: string_to_pic_x(TRAN_CAT_TYPE_DESC, &self.tran_cat_type_desc)?,
            filler: string_to_pic_x(FILLER, &self.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{FixedRecord, PicU, PicX};
    use domain::records::TransactionCategoryRecord;

    use super::{FILLER, TRAN_CAT_CD, TRAN_CAT_TYPE_DESC, TRAN_TYPE_CD, TransactionCategoryRow};
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    // Two fixture bodies are quoted rather than the file read: this crate reaches fixture paths
    // only through `test_support`, and the whole-file round trip is `codec_b_roundtrip` (d9).

    /// The body of `trancatg.txt` line 1: type `01`, category `0001`, `Regular Sales Draft`
    /// padded to 50 bytes, `FILLER` `0000` (FIXFRM-008).
    const LINE_1: &[u8; 60] = b"010001Regular Sales Draft                               0000";

    /// The body of `trancatg.txt` line 18, the last: type `07`, category `0001`.
    const LINE_18: &[u8; 60] = b"070001Sales draft credit adjustment                     0000";

    /// The row that stores the record codec A decodes from `body`.
    fn row_of(body: &[u8]) -> Result<TransactionCategoryRow, Box<dyn StdError>> {
        let record = TransactionCategoryRecord::decode(body)?;
        Ok(TransactionCategoryRow::from_record(&record)?)
    }

    // audit: CVTRA04Y-001
    // audit: CVTRA04Y-002
    // audit: CVTRA04Y-003
    // audit: CVTRA04Y-004
    // audit: FIXFRM-008
    #[test]
    fn fixture_records_are_stored_byte_for_byte_and_read_back_unchanged() -> TestResult {
        let cases = [
            (LINE_1, "01", 1, "Regular Sales Draft"),
            (LINE_18, "07", 1, "Sales draft credit adjustment"),
        ];
        for (body, type_cd, cat_cd, desc) in cases {
            let record = TransactionCategoryRecord::decode(body)?;
            let row = TransactionCategoryRow::from_record(&record)?;
            assert_eq!(row.tran_type_cd, type_cd);
            assert_eq!(row.tran_cat_cd, cat_cd);
            assert_eq!(row.tran_cat_type_desc, format!("{desc:<50}"));
            assert_eq!(row.filler, "0000");
            let back = row.into_record()?;
            assert_eq!(back, record);
            assert_eq!(back.encode(), body);
        }
        Ok(())
    }

    // audit: PIC-001
    // audit: PIC-003
    // audit: PIC-012
    #[test]
    fn every_byte_and_both_category_bounds_survive_both_directions() -> TestResult {
        for (cat_cd, filler) in [(0, *b"0 9X"), (9_999, *b"~{} "), (42, *b"    ")] {
            let record = TransactionCategoryRecord {
                type_cd: PicX::from_array(*b" Z"),
                cat_cd: PicU::new(cat_cd)?,
                type_desc: PicX::from_text("  leading, inner  and trailing spaces kept;")?,
                filler: PicX::from_array(filler),
            };
            let row = TransactionCategoryRow::from_record(&record)?;
            assert_eq!(row.tran_type_cd.as_bytes(), record.type_cd.as_bytes());
            // TRUNCATION: none survives. The conversion is checked: `tran_cat_cd` holds a
            // `PIC 9(04)` value, 0 to 9999, which `u64` holds exactly, and `?` refuses a
            // negative value instead of wrapping.
            assert_eq!(u64::try_from(row.tran_cat_cd)?, cat_cd);
            assert_eq!(
                row.tran_cat_type_desc.as_bytes(),
                record.type_desc.as_bytes()
            );
            assert_eq!(row.filler.as_bytes(), &filler);
            let stored = row.clone();
            let back = row.into_record()?;
            assert_eq!(back, record);
            assert_eq!(back.encode().len(), TransactionCategoryRecord::WIDTH);
            assert!(TransactionCategoryRow::from_record(&back)? == stored);
        }
        Ok(())
    }

    // audit: CVTRA04Y-002
    // audit: PIC-023
    #[test]
    fn a_category_code_outside_four_digits_is_out_of_range() -> TestResult {
        for tran_cat_cd in [10_000, -1, i64::MIN, i64::MAX] {
            let row = TransactionCategoryRow {
                tran_cat_cd,
                ..row_of(LINE_1)?
            };
            assert_eq!(
                row.into_record(),
                Err(PersistenceError::decode(
                    TRAN_CAT_CD,
                    DecodeKind::OutOfRange
                ))
            );
        }
        Ok(())
    }

    #[test]
    fn text_of_any_other_width_is_a_width_mismatch_at_its_column() -> TestResult {
        let base = row_of(LINE_1)?;
        let cases = [
            (
                TransactionCategoryRow {
                    tran_cat_type_desc: "D".repeat(51),
                    ..base.clone()
                },
                TRAN_CAT_TYPE_DESC,
                50,
                51,
            ),
            (
                TransactionCategoryRow {
                    tran_cat_type_desc: "Regular Sales Draft".to_owned(),
                    ..base.clone()
                },
                TRAN_CAT_TYPE_DESC,
                50,
                19,
            ),
            (
                TransactionCategoryRow {
                    tran_type_cd: "1".to_owned(),
                    ..base.clone()
                },
                TRAN_TYPE_CD,
                2,
                1,
            ),
            (
                TransactionCategoryRow {
                    filler: "00000".to_owned(),
                    ..base
                },
                FILLER,
                4,
                5,
            ),
        ];
        for (row, column, expected, actual) in cases {
            assert_eq!(
                row.into_record(),
                Err(PersistenceError::decode(
                    column,
                    DecodeKind::WidthMismatch { expected, actual }
                ))
            );
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn nul_and_non_ascii_bytes_are_invalid_both_ways() -> TestResult {
        let record = TransactionCategoryRecord::decode(LINE_1)?;
        let records = [
            (
                TransactionCategoryRecord {
                    type_cd: PicX::from_array([0, b'1']),
                    ..record.clone()
                },
                TRAN_TYPE_CD,
            ),
            (
                TransactionCategoryRecord {
                    filler: PicX::from_array([b'0', 0x80, b'0', b'0']),
                    ..record
                },
                FILLER,
            ),
        ];
        for (record, column) in records {
            assert_eq!(
                TransactionCategoryRow::from_record(&record).err(),
                Some(PersistenceError::decode(column, DecodeKind::InvalidByte))
            );
        }
        let base = row_of(LINE_1)?;
        let rows = [
            (
                TransactionCategoryRow {
                    tran_cat_type_desc: format!("{:<49}\u{e9}", "Regular Sales Draft"),
                    ..base.clone()
                },
                TRAN_CAT_TYPE_DESC,
            ),
            (
                TransactionCategoryRow {
                    filler: "0\u{0}00".to_owned(),
                    ..base
                },
                FILLER,
            ),
        ];
        for (row, column) in rows {
            assert_eq!(
                row.into_record(),
                Err(PersistenceError::decode(column, DecodeKind::InvalidByte))
            );
        }
        Ok(())
    }
}
