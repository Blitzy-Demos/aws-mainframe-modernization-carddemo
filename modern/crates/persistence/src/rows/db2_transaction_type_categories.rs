//! Codec B for `db2.transaction_type_category`, the Db2 data version of the transaction
//! categories.
//!
//! Each row holds one [`Db2TransactionCategory`], the DCLGEN group
//! `DCLTRANSACTION-TYPE-CATEGORY` of `DCLTRCAT.dcl`. Migration `0006_db2_transaction_types.sql`
//! creates the table from `TRNTYCAT.ddl:1-7` and seeds it with the 18 rows of
//! `DB2LTCAT.ctl:15-37`.
//!
//! | Column | Db2 column and host variable | Record accessor | Conversion |
//! |---|---|---|---|
//! | `trc_type_code CHAR(2)` | `TRC_TYPE_CODE CHAR(2) NOT NULL`, `DCL-TRC-TYPE-CODE PIC X(2)` | [`Db2TransactionCategory::type_code`] | `pic_x_to_string`, `string_to_pic_x` |
//! | `trc_type_category CHAR(4)` | `TRC_TYPE_CATEGORY CHAR(4) NOT NULL`, `DCL-TRC-TYPE-CATEGORY PIC X(4)` | [`Db2TransactionCategory::type_category`] | `pic_x_to_string`, `string_to_pic_x` |
//! | `trc_cat_data VARCHAR(50)` | `TRC_CAT_DATA VARCHAR(50) NOT NULL`, `DCL-TRC-CAT-DATA-LEN` and `-TEXT PIC X(50)` | [`Db2TransactionCategory::cat_data`] | `string_to_varchar`, `varchar_to_string` |
//!
//! The two keys keep every byte, padding included, as `CHAR(n)` returns it. The text is a
//! `VARCHAR`, so its length is part of the value: it is stored and read back verbatim, never
//! trimmed, padded or upper-cased.
//!
//! # Where the table is reached
//!
//! No statement of COTRTUPC, COTRTLIC or COBTUPDT names this table. It is reached only through
//! its foreign key `trc_type_code … REFERENCES db2.transaction_type (tr_type) ON DELETE
//! RESTRICT` (`TRNTYCAT.ddl:6-7`), which refuses to delete a type that still has categories, as
//! each of the seven seeded types does (COTRTUPC-002, COTRTUPC-033, COTRTLIC-050, COBTUPDT-017,
//! COBTUPDT-026). Reading these rows back is how a caller observes that a refused delete left
//! the categories unchanged.
//!
//! # Two data versions (E23)
//!
//! This row converts only to and from the Db2 version. It has no conversion to or from
//! [`TransactionCategoryRecord`](domain::records::TransactionCategoryRecord) or the
//! `transaction_categories` row, the VSAM version that `tranrept` reads: on the mainframe only
//! the daily TRANEXTR copied one version into the other, and TRANEXTR is out of scope, so a
//! maintenance change here never reaches the report lookups.

use domain::records::Db2TransactionCategory;

use super::{
    copybook_kind, pic_x_to_string, string_to_pic_x, string_to_varchar, varchar_to_string,
};
use crate::error::PersistenceError;

/// Column of `DCL-TRC-TYPE-CODE PIC X(2)`, the foreign key to `db2.transaction_type`.
const TRC_TYPE_CODE: &str = "trc_type_code";

/// Column of `DCL-TRC-TYPE-CATEGORY PIC X(4)`.
const TRC_TYPE_CATEGORY: &str = "trc_type_category";

/// Column of `DCL-TRC-CAT-DATA`, the category description.
const TRC_CAT_DATA: &str = "trc_cat_data";

/// Most bytes `trc_cat_data VARCHAR(50)` holds (`TRNTYCAT.ddl:4`).
const TRC_CAT_DATA_MAX_LEN: usize = 50;

/// One row of `db2.transaction_type_category`, its fields named and typed as the columns.
///
/// Primary key `x_tran_type_catg` is (`trc_type_code`, `trc_type_category`), the unique index of
/// `XTRNTYCAT.ddl`.
pub struct Db2TransactionTypeCategoryRow {
    /// `trc_type_code CHAR(2)`: exactly two ASCII bytes.
    pub(crate) trc_type_code: String,
    /// `trc_type_category CHAR(4)`: exactly four ASCII bytes, alphanumeric as the DDL declares
    /// it, unlike the VSAM `TRAN-CAT-CD PIC 9(04)`.
    pub(crate) trc_type_category: String,
    /// `trc_cat_data VARCHAR(50)`: at most 50 ASCII bytes, unpadded.
    pub(crate) trc_cat_data: String,
}

impl Db2TransactionTypeCategoryRow {
    /// The row that stores `record`: both keys byte for byte, the text verbatim.
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or non-ASCII
    /// byte in any field, which no `PostgreSQL` text column stores byte for byte. The record
    /// itself already limits the text to 50 bytes.
    pub fn from_record(record: &Db2TransactionCategory) -> Result<Self, PersistenceError> {
        Ok(Self {
            trc_type_code: pic_x_to_string(TRC_TYPE_CODE, record.type_code())?,
            trc_type_category: pic_x_to_string(TRC_TYPE_CATEGORY, record.type_category())?,
            trc_cat_data: string_to_varchar(TRC_CAT_DATA, record.cat_data(), TRC_CAT_DATA_MAX_LEN)?,
        })
    }

    /// The record this row stores, equal to the one [`Self::from_record`] wrote.
    ///
    /// # Errors
    ///
    /// For the first column, in column order, whose value does not fit:
    ///
    /// - [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or
    ///   non-ASCII character;
    /// - [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) for a key that is
    ///   not exactly 2 or 4 bytes long, or a text longer than 50 bytes;
    /// - [`DecodeKind::Copybook`](crate::error::DecodeKind::Copybook) when
    ///   [`Db2TransactionCategory::new`] rejects the text.
    pub fn into_record(self) -> Result<Db2TransactionCategory, PersistenceError> {
        let type_code = string_to_pic_x::<2>(TRC_TYPE_CODE, &self.trc_type_code)?;
        let type_category = string_to_pic_x::<4>(TRC_TYPE_CATEGORY, &self.trc_type_category)?;
        let cat_data = varchar_to_string(TRC_CAT_DATA, &self.trc_cat_data, TRC_CAT_DATA_MAX_LEN)?;
        // The constructor's one rejection is a text over 50 bytes, so its error belongs to
        // `trc_cat_data`.
        Db2TransactionCategory::new(type_code, type_category, cat_data)
            .map_err(|error| PersistenceError::decode(TRC_CAT_DATA, copybook_kind(&error)))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::PicX;
    use domain::records::Db2TransactionCategory;

    use super::{Db2TransactionTypeCategoryRow, TRC_CAT_DATA, TRC_TYPE_CATEGORY, TRC_TYPE_CODE};
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The 18 seed rows of `DB2LTCAT.ctl:20-37`, as migration 0006 inserts them: type code,
    /// category, text.
    const SEED: [(&str, &str, &str); 18] = [
        ("01", "0001", "REGULAR SALES DRAFT"),
        ("01", "0002", "REGULAR CASH ADVANCE"),
        ("01", "0003", "CONVENIENCE CHECK DEBIT"),
        ("01", "0004", "ATM CASH ADVANCE"),
        ("01", "0005", "INTEREST AMOUNT"),
        ("02", "0001", "CASH PAYMENT"),
        ("02", "0002", "ELECTRONIC PAYMENT"),
        ("02", "0003", "CHECK PAYMENT"),
        ("03", "0001", "CREDIT TO ACCOUNT"),
        ("03", "0002", "CREDIT TO PURCHASE BALANCE"),
        ("03", "0003", "CREDIT TO CASH BALANCE"),
        ("04", "0001", "ZERO DOLLAR AUTHORIZATION"),
        ("04", "0002", "ONLINE PURCHASE AUTHORIZATION"),
        ("04", "0003", "TRAVEL BOOKING AUTHORIZATION"),
        ("05", "0001", "REFUND CREDIT"),
        ("06", "0001", "FRAUD REVERSAL"),
        ("06", "0002", "NON FRAUD REVERSAL"),
        ("07", "0001", "SALES DRAFT CREDIT ADJUSTMENT"),
    ];

    /// The record holding these values; each key fills its field exactly.
    fn record(
        code: &str,
        category: &str,
        text: &str,
    ) -> Result<Db2TransactionCategory, Box<dyn StdError>> {
        Ok(Db2TransactionCategory::new(
            PicX::<2>::from_text(code)?,
            PicX::<4>::from_text(category)?,
            text.to_owned(),
        )?)
    }

    /// The row as a query would return these column values.
    fn row(code: &str, category: &str, text: &str) -> Db2TransactionTypeCategoryRow {
        Db2TransactionTypeCategoryRow {
            trc_type_code: code.to_owned(),
            trc_type_category: category.to_owned(),
            trc_cat_data: text.to_owned(),
        }
    }

    /// The error of a conversion that `column`'s value fails with `kind`.
    const fn rejected(column: &'static str, kind: DecodeKind) -> PersistenceError {
        PersistenceError::decode(column, kind)
    }

    #[test]
    fn every_seed_row_round_trips_unchanged() -> TestResult {
        for (code, category, text) in SEED {
            let written = record(code, category, text)?;
            let stored = Db2TransactionTypeCategoryRow::from_record(&written)?;
            assert_eq!(stored.trc_type_code, code);
            assert_eq!(stored.trc_type_category, category);
            // VARCHAR: the text keeps its own length, with no padding to 50.
            assert_eq!(stored.trc_cat_data, text);
            assert_eq!(stored.into_record()?, written);
        }
        Ok(())
    }

    #[test]
    fn keys_keep_their_padding_and_text_is_kept_verbatim_up_to_fifty_bytes() -> TestResult {
        let longest = "X".repeat(50);
        for (code, category, text) in [
            ("06", "0002", ""),
            ("6 ", "2   ", "  Non fraud reversal  "),
            ("  ", "    ", "REVERAL "),
            ("07", "0001", longest.as_str()),
        ] {
            let written = record(code, category, text)?;
            let stored = Db2TransactionTypeCategoryRow::from_record(&written)?;
            assert_eq!(stored.trc_type_code, code);
            assert_eq!(stored.trc_type_category, category);
            assert_eq!(stored.trc_cat_data, text);
            assert_eq!(stored.into_record()?, written);
        }
        Ok(())
    }

    #[test]
    fn stored_text_longer_than_fifty_bytes_is_rejected() {
        let long = "X".repeat(51);
        assert_eq!(
            row("01", "0001", &long).into_record().err(),
            Some(rejected(
                TRC_CAT_DATA,
                DecodeKind::WidthMismatch {
                    expected: 50,
                    actual: 51
                }
            ))
        );
    }

    #[test]
    fn stored_keys_must_fill_their_char_columns_exactly() {
        for (stored, column, expected, actual) in [
            (row("01", "000", "CASH PAYMENT"), TRC_TYPE_CATEGORY, 4, 3),
            (row("01", "00001", "CASH PAYMENT"), TRC_TYPE_CATEGORY, 4, 5),
            (row("1", "0001", "CASH PAYMENT"), TRC_TYPE_CODE, 2, 1),
            (row("", "", ""), TRC_TYPE_CODE, 2, 0),
        ] {
            assert_eq!(
                stored.into_record().err(),
                Some(rejected(
                    column,
                    DecodeKind::WidthMismatch { expected, actual }
                ))
            );
        }
    }

    #[test]
    fn nul_and_non_ascii_bytes_are_rejected_on_read() {
        for (stored, column) in [
            (row("0\0", "0001", "CASH PAYMENT"), TRC_TYPE_CODE),
            (row("01", "00\0 ", "CASH PAYMENT"), TRC_TYPE_CATEGORY),
            (row("01", "0001", "CASH\0PAYMENT"), TRC_CAT_DATA),
            (row("01", "0001", "CAF\u{c9}"), TRC_CAT_DATA),
        ] {
            assert_eq!(
                stored.into_record().err(),
                Some(rejected(column, DecodeKind::InvalidByte))
            );
        }
    }

    /// The record admits bytes its columns cannot store byte for byte; writing it fails instead
    /// of replacing them.
    #[test]
    fn nul_and_non_ascii_bytes_are_rejected_on_write() -> TestResult {
        let code = PicX::<2>::from_array(*b"01");
        let category = PicX::<4>::from_array(*b"0001");
        for (written, column) in [
            (
                Db2TransactionCategory::new(
                    PicX::from_array(*b"0\0"),
                    category.clone(),
                    "CASH PAYMENT".to_owned(),
                )?,
                TRC_TYPE_CODE,
            ),
            (
                Db2TransactionCategory::new(
                    code.clone(),
                    PicX::from_array([b'0', 0, b'0', b'1']),
                    "CASH PAYMENT".to_owned(),
                )?,
                TRC_TYPE_CATEGORY,
            ),
            (
                Db2TransactionCategory::new(
                    code.clone(),
                    category.clone(),
                    "CASH\0PAYMENT".to_owned(),
                )?,
                TRC_CAT_DATA,
            ),
            (
                // 25 two-byte characters: 50 bytes, within the record's limit.
                Db2TransactionCategory::new(code, category, "\u{c9}".repeat(25))?,
                TRC_CAT_DATA,
            ),
        ] {
            assert_eq!(
                Db2TransactionTypeCategoryRow::from_record(&written).err(),
                Some(rejected(column, DecodeKind::InvalidByte))
            );
        }
        Ok(())
    }
}
