//! The Db2 data version of a transaction category: one row of
//! `CARDDEMO.TRANSACTION_TYPE_CATEGORY` as its DCLGEN declaration `DCLTRCAT` holds it
//! (`DCLTRCAT.dcl`, `TRNTYCAT.ddl:1-7`), stored in the replacement as a row of
//! `db2.transaction_type_category` (migration `0006_db2_transaction_types.sql`).
//!
//! | DCLGEN host variable | Column (`TRNTYCAT.ddl:2-4`) | Accessor |
//! |---|---|---|
//! | `DCL-TRC-TYPE-CODE PIC X(2)` | `TRC_TYPE_CODE CHAR(2) NOT NULL` | [`Db2TransactionCategory::type_code`] |
//! | `DCL-TRC-TYPE-CATEGORY PIC X(4)` | `TRC_TYPE_CATEGORY CHAR(4) NOT NULL` | [`Db2TransactionCategory::type_category`] |
//! | `DCL-TRC-CAT-DATA`: `-LEN PIC S9(4) COMP`, `-TEXT PIC X(50)` | `TRC_CAT_DATA VARCHAR(50) NOT NULL` | [`Db2TransactionCategory::cat_data`] |
//!
//! # Keys and the delete restriction
//!
//! The primary key is (`TRC_TYPE_CODE`, `TRC_TYPE_CATEGORY`) (`TRNTYCAT.ddl:5`). The foreign key
//! `TRC_TYPE_CODE` references `TRANSACTION_TYPE (TR_TYPE)` with `ON DELETE RESTRICT`
//! (`TRNTYCAT.ddl:6-7`). No statement of COTRTUPC, COTRTLIC or COBTUPDT names this table: it is
//! reached only through that restriction, which makes deleting a type that still has categories
//! fail, as it does for each of the seven seeded types (COTRTUPC-002, COTRTUPC-033,
//! COTRTLIC-050, COBTUPDT-017, COBTUPDT-026). Migration 0006 seeds the 18 rows of
//! `DB2LTCAT.ctl:15-37` once per database, as CREADB21 does.
//!
//! # Values
//!
//! - `TRC_TYPE_CATEGORY` is `CHAR(4)`, alphanumeric, unlike the VSAM `TRAN-CAT-CD PIC 9(04)` of
//!   `CVTRA04Y`, so it is a `PicX<4>` and the seed's `0001` is four characters, not a number.
//! - `TRC_CAT_DATA` is a `VARCHAR`: its length half, `DCL-TRC-CAT-DATA-LEN`, is part of the value,
//!   so the text is held exactly as given, neither trimmed nor padded, and [`str::len`] stands in
//!   for that length. The constructor admits at most 50 bytes, the `PIC X(50)` text half.
//!
//! # Two data versions (E23)
//!
//! This is a row type only: it has no [`FixedRecord`](copybook::FixedRecord) byte form and no
//! fixture framing, because no fixture holds the Db2 version, and no conversion to or from
//! [`TransactionCategoryRecord`](super::TransactionCategoryRecord), the VSAM version that
//! `tranrept` reads. No component copies between the two versions: on the mainframe only the
//! daily TRANEXTR did, and it is out of scope.

use copybook::{Error, PicX};

/// Field name of the `VARCHAR(50)` text half in a length error.
const CAT_DATA_TEXT: &str = "DCL-TRC-CAT-DATA-TEXT";

/// Most bytes `TRC_CAT_DATA VARCHAR(50)` holds (`TRNTYCAT.ddl:4`, `DCL-TRC-CAT-DATA-TEXT PIC
/// X(50)`).
const CAT_DATA_MAX_LEN: usize = 50;

/// One row of the Db2 table `CARDDEMO.TRANSACTION_TYPE_CATEGORY`, laid out as the DCLGEN group
/// `DCLTRANSACTION-TYPE-CATEGORY` (`DCLTRCAT.dcl`).
///
/// The fields are private so that a row whose text exceeds `VARCHAR(50)` cannot be built; every
/// value comes through [`Db2TransactionCategory::new`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Db2TransactionCategory {
    /// `TRC_TYPE_CODE CHAR(2)`, host variable `DCL-TRC-TYPE-CODE PIC X(2)`: the transaction type
    /// the category belongs to, and the foreign key to `TRANSACTION_TYPE`.
    type_code: PicX<2>,
    /// `TRC_TYPE_CATEGORY CHAR(4)`, host variable `DCL-TRC-TYPE-CATEGORY PIC X(4)`.
    type_category: PicX<4>,
    /// `TRC_CAT_DATA VARCHAR(50)`, host variables `DCL-TRC-CAT-DATA-LEN` and
    /// `DCL-TRC-CAT-DATA-TEXT`: at most 50 bytes, unpadded.
    cat_data: String,
}

impl Db2TransactionCategory {
    /// The row with key (`type_code`, `type_category`) and description `cat_data`, the text
    /// stored exactly as given: no trimming, case folding or padding.
    ///
    /// # Errors
    ///
    /// [`copybook::ErrorKind::OutOfRange`] for field `DCL-TRC-CAT-DATA-TEXT` at offset 0 and
    /// width 50 when `cat_data` is longer than 50 bytes, which `TRC_CAT_DATA VARCHAR(50)` cannot
    /// hold. The error carries no part of the text.
    pub fn new(
        type_code: PicX<2>,
        type_category: PicX<4>,
        cat_data: String,
    ) -> Result<Self, Error> {
        let cat_data = super::varchar::<CAT_DATA_MAX_LEN>(CAT_DATA_TEXT, cat_data)?;
        Ok(Self {
            type_code,
            type_category,
            cat_data,
        })
    }

    /// `TRC_TYPE_CODE`, the two bytes of `DCL-TRC-TYPE-CODE PIC X(2)`.
    #[must_use]
    pub fn type_code(&self) -> &PicX<2> {
        &self.type_code
    }

    /// `TRC_TYPE_CATEGORY`, the four bytes of `DCL-TRC-TYPE-CATEGORY PIC X(4)`.
    #[must_use]
    pub fn type_category(&self) -> &PicX<4> {
        &self.type_category
    }

    /// `TRC_CAT_DATA`, the description as stored: `DCL-TRC-CAT-DATA-TEXT` cut to the length in
    /// `DCL-TRC-CAT-DATA-LEN`.
    #[must_use]
    pub fn cat_data(&self) -> &str {
        &self.cat_data
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, PicX};

    use super::{CAT_DATA_MAX_LEN, CAT_DATA_TEXT, Db2TransactionCategory};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The 18 seed rows of `DB2LTCAT.ctl:20-37`, in their order: type code, category, text.
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

    /// The row built from text values; every key the tests use fills its field exactly, so the
    /// alphanumeric `MOVE` neither pads nor truncates it.
    fn row(code: &str, category: &str, text: String) -> Result<Db2TransactionCategory, Error> {
        Db2TransactionCategory::new(
            PicX::<2>::moved_from(code.as_bytes()),
            PicX::<4>::moved_from(category.as_bytes()),
            text,
        )
    }

    #[test]
    fn accepts_every_seed_row_and_returns_its_values_unchanged() -> TestResult {
        for (code, category, text) in SEED {
            let category_row = row(code, category, text.to_owned())?;
            assert_eq!(category_row.type_code().as_bytes(), code.as_bytes());
            assert_eq!(category_row.type_category().as_bytes(), category.as_bytes());
            // VARCHAR: the stored text keeps its own length, with no padding to 50.
            assert_eq!(category_row.cat_data(), text);
        }
        Ok(())
    }

    #[test]
    fn keeps_text_exactly_as_given_up_to_fifty_bytes() -> TestResult {
        for text in [
            String::new(),
            " Regular sales draft  ".to_owned(),
            "X".repeat(CAT_DATA_MAX_LEN),
        ] {
            assert_eq!(row("01", "0001", text.clone())?.cat_data(), text);
        }
        Ok(())
    }

    #[test]
    fn rejects_a_description_longer_than_fifty_bytes() {
        for text in ["X".repeat(CAT_DATA_MAX_LEN + 1), "\u{e9}".repeat(26)] {
            let rejected = row("01", "0001", text);
            assert!(matches!(
                rejected,
                Err(Error {
                    kind: ErrorKind::OutOfRange,
                    ..
                })
            ));
            assert_eq!(
                rejected,
                Err(Error::new(
                    CAT_DATA_TEXT,
                    0,
                    CAT_DATA_MAX_LEN,
                    ErrorKind::OutOfRange
                )),
            );
        }
    }

    #[test]
    fn rows_differing_in_any_field_are_unequal() -> TestResult {
        let first = row("01", "0001", "REGULAR SALES DRAFT".to_owned())?;
        assert_eq!(first.clone(), first);
        for other in [
            row("02", "0001", "REGULAR SALES DRAFT".to_owned())?,
            row("01", "0002", "REGULAR SALES DRAFT".to_owned())?,
            row("01", "0001", "REGULAR SALES DRAFT ".to_owned())?,
        ] {
            assert_ne!(other, first);
        }
        Ok(())
    }
}
