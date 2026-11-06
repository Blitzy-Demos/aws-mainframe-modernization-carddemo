//! The two versions of the transaction category reference data.
//!
//! The COBOL keeps the categories twice. The VSAM processing copy `TRANCATG`
//! holds `CVTRA04Y` `TRAN-CAT-RECORD`s loaded from `trancatg.txt`, is read
//! only by `CBTRN03C` (the CR00 report and `TRANREPT`) and is the table
//! `trancatg_file`. The Db2 master `CARDDEMO.TRANSACTION_TYPE_CATEGORY` is
//! loaded by `DB2LTCAT` with upper-case descriptions (`REGULAR SALES DRAFT`
//! [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:L20], where the fixture
//! holds `Regular Sales Draft` [app/data/ASCII/trancatg.txt:L1]) and is the
//! table `transaction_categories`. The two agree only after `TRANEXTR`
//! (`batch seed --only tranextr`) copies the master over the processing copy.
//! [`TranCategoryRecord`] and [`TranCategoryMaster`] keep them apart: neither
//! converts into the other.
//!
//! Every field has the type its reviewed section A row of
//! `docs/parity/parity-oracle-audit.md` gives it, and names that row.

use crate::numeric::{Alnum, NumericError, TranCategoryCode, TranTypeCode};

/// A category of the VSAM processing copy `TRANCATG`: `CVTRA04Y`
/// `TRAN-CAT-RECORD` [app/cpy/CVTRA04Y.cpy:L4-9], 60 bytes, table
/// `trancatg_file`.
///
/// The 6-byte KSDS key, the group `TRAN-CAT-KEY` [app/cpy/CVTRA04Y.cpy:L5], is
/// flattened into [`tran_type_cd`](Self::tran_type_cd) and
/// [`tran_cat_cd`](Self::tran_cat_cd), the two primary-key columns. The
/// trailing `FILLER PIC X(04)` (P-CVTRA04Y-004) [app/cpy/CVTRA04Y.cpy:L9] is
/// not modelled: it is not stored, and the codec layout keeps its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranCategoryRecord {
    /// `TRAN-TYPE-CD PIC X(02)` — P-CVTRA04Y-001 [app/cpy/CVTRA04Y.cpy:L6].
    /// Column `tran_type_cd VARCHAR(2)`, right-trimmed.
    pub tran_type_cd: TranTypeCode,
    /// `TRAN-CAT-CD PIC 9(04)` — P-CVTRA04Y-002 [app/cpy/CVTRA04Y.cpy:L7].
    /// Column `tran_cat_cd INTEGER`, 0 to 9999.
    pub tran_cat_cd: TranCategoryCode,
    /// `TRAN-CAT-TYPE-DESC PIC X(50)` — P-CVTRA04Y-003
    /// [app/cpy/CVTRA04Y.cpy:L8]. Column `tran_cat_type_desc VARCHAR(50)`,
    /// right-trimmed.
    pub tran_cat_type_desc: Alnum<50>,
}

/// The width of `TRC_CAT_DATA VARCHAR(50)` (P-TRNTYCAT-003)
/// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L4]: the most characters
/// [`TranCategoryMaster::trc_cat_data`] holds, one stored byte each. It is also
/// the `X(50)` of the DCLGEN host variable `DCL-TRC-CAT-DATA-TEXT`
/// [app/app-transaction-type-db2/dcl/DCLTRCAT.dcl:L50-51].
pub const TRC_CAT_DATA_MAX_LEN: usize = 50;

/// A category of the Db2 master `CARDDEMO.TRANSACTION_TYPE_CATEGORY`
/// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L1-7], table
/// `transaction_categories`.
///
/// The primary key is (`TRC_TYPE_CODE`, `TRC_TYPE_CATEGORY`)
/// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L5]. `TRC_TYPE_CODE`
/// references `TRANSACTION_TYPE (TR_TYPE)` `ON DELETE RESTRICT`
/// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L6-7], so a type that still
/// has categories cannot be deleted. The DCLGEN host variables are
/// `DCL-TRC-TYPE-CODE PIC X(2)`, `DCL-TRC-TYPE-CATEGORY PIC X(4)` and the
/// `VARCHAR` group `DCL-TRC-CAT-DATA`, a `DCL-TRC-CAT-DATA-LEN PIC S9(4) COMP`
/// length and a `DCL-TRC-CAT-DATA-TEXT PIC X(50)` text
/// [app/app-transaction-type-db2/dcl/DCLTRCAT.dcl:L39-51].
///
/// The fields are public so that adapters and tests can build a master as a
/// struct literal; [`TranCategoryMaster::try_new`] is the checked path from
/// description text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranCategoryMaster {
    /// `TRC_TYPE_CODE CHAR(2) NOT NULL` — P-TRNTYCAT-001
    /// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L2]. Column
    /// `trc_type_code VARCHAR(2)`, right-trimmed.
    pub trc_type_code: TranTypeCode,
    /// `TRC_TYPE_CATEGORY CHAR(4) NOT NULL` — P-TRNTYCAT-002
    /// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L3].
    ///
    /// Every loaded value is four digits, `0001` to `0005`
    /// [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:L20-37], and the audit row
    /// types the column as the `9(04)` category code. The column
    /// `trc_type_category VARCHAR(4)` keeps the Db2 text: persistence binds the
    /// code's zero-padded `Display` form and reads the column back through its
    /// `FromStr`.
    pub trc_type_category: TranCategoryCode,
    /// `TRC_CAT_DATA VARCHAR(50) NOT NULL` — P-TRNTYCAT-003
    /// [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L4].
    ///
    /// An `X(50)` item, as the reviewed audit row types it, rather than a string
    /// holding the Db2 `VARCHAR` value exactly: the column
    /// `trc_cat_data VARCHAR(50)` holds the right-trimmed text, which its
    /// `CHECK` enforces, so persistence binds [`Alnum::trimmed`] and reads the
    /// column back with [`Alnum::from_trimmed`]. Trailing spaces of a value are
    /// therefore not kept apart from the padding, and descriptions compare as
    /// the COBOL compares `X(50)` items.
    pub trc_cat_data: Alnum<TRC_CAT_DATA_MAX_LEN>,
}

impl TranCategoryMaster {
    /// The master row holding the description `trc_cat_data`, as `DB2LTCAT`
    /// loads one [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:L20-37].
    ///
    /// Trailing spaces are dropped first, as a Db2 storage assignment drops
    /// the excess trailing blanks of a string longer than its `VARCHAR` column
    /// (IBM Db2 13 for z/OS SQL Reference, "String assignments"). The rest
    /// fills the `X(50)` item left-justified and space-padded, one ISO 8859-1
    /// byte per character, as [`Alnum::from_trimmed`] builds it. The empty
    /// string is accepted: the DDL allows an empty `VARCHAR`, and the COBOL
    /// empty-field edits belong to the rules.
    ///
    /// # Errors
    ///
    /// [`NumericError::TooLong`] when the description, without its trailing
    /// spaces, has more than [`TRC_CAT_DATA_MAX_LEN`] characters, and
    /// [`NumericError::NotLatin1`] when a character lies above `U+00FF`, which
    /// no single stored byte holds. Nothing is cut or replaced.
    // `&str` rather than an owned `String`: the text is only read, and clippy's
    // pedantic `needless_pass_by_value`, denied in this workspace, rejects an
    // owned parameter that is only borrowed.
    pub fn try_new(
        trc_type_code: TranTypeCode,
        trc_type_category: TranCategoryCode,
        trc_cat_data: &str,
    ) -> Result<Self, NumericError> {
        Ok(Self {
            trc_type_code,
            trc_type_category,
            trc_cat_data: Alnum::from_trimmed(trc_cat_data.trim_end_matches(' '))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{TRC_CAT_DATA_MAX_LEN, TranCategoryMaster, TranCategoryRecord};
    use crate::numeric::{Alnum, NumericError, TranCategoryCode, TranTypeCode};

    fn category(value: u16) -> TranCategoryCode {
        TranCategoryCode::new(value).expect("a 9(04) category code")
    }

    #[test]
    fn try_new_holds_the_first_db2ltcat_row_as_loaded() {
        let master = TranCategoryMaster::try_new(
            TranTypeCode::move_from("01"),
            category(1),
            "REGULAR SALES DRAFT",
        )
        .expect("DB2LTCAT.ctl L20 fits the master");
        assert_eq!(master.trc_type_code.trimmed(), "01");
        assert_eq!(master.trc_type_category.to_string(), "0001");
        assert_eq!(master.trc_cat_data.trimmed(), "REGULAR SALES DRAFT");
        assert_eq!(
            master.trc_cat_data,
            Alnum::from_trimmed("REGULAR SALES DRAFT").expect("fits X(50)")
        );
    }

    #[test]
    fn try_new_accepts_descriptions_the_column_holds() {
        let full = "D".repeat(TRC_CAT_DATA_MAX_LEN);
        let latin1 = "é".repeat(TRC_CAT_DATA_MAX_LEN);
        let cases = [
            (String::new(), String::new()),
            (full.clone(), full.clone()),
            (latin1.clone(), latin1),
            (format!("{full}   "), full),
            ("ABC  ".to_owned(), "ABC".to_owned()),
        ];
        for (text, stored) in cases {
            let master =
                TranCategoryMaster::try_new(TranTypeCode::move_from("04"), category(3), &text)
                    .unwrap_or_else(|error| panic!("{text:?} rejected: {error}"));
            assert_eq!(master.trc_cat_data.trimmed(), stored, "{text:?}");
        }
    }

    #[test]
    fn try_new_rejects_descriptions_the_column_cannot_hold() {
        let cases = [
            (
                "D".repeat(TRC_CAT_DATA_MAX_LEN + 1),
                NumericError::TooLong {
                    max: TRC_CAT_DATA_MAX_LEN,
                    actual: TRC_CAT_DATA_MAX_LEN + 1,
                },
            ),
            (
                format!("{} D", "D".repeat(TRC_CAT_DATA_MAX_LEN - 1)),
                NumericError::TooLong {
                    max: TRC_CAT_DATA_MAX_LEN,
                    actual: TRC_CAT_DATA_MAX_LEN + 1,
                },
            ),
            ("PAYMENT €".to_owned(), NumericError::NotLatin1),
        ];
        for (text, expected) in cases {
            assert_eq!(
                TranCategoryMaster::try_new(TranTypeCode::move_from("02"), category(1), &text),
                Err(expected),
                "{text:?}"
            );
        }
    }

    #[test]
    fn tran_category_record_compares_and_clones_by_value() {
        let record = TranCategoryRecord {
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: category(1),
            tran_cat_type_desc: Alnum::move_from("Regular Sales Draft"),
        };
        let copy = record.clone();
        assert_eq!(copy, record);

        let other_category = TranCategoryRecord {
            tran_cat_cd: category(2),
            ..record
        };
        let other_type = TranCategoryRecord {
            tran_type_cd: TranTypeCode::move_from("02"),
            ..record
        };
        let master_case = TranCategoryRecord {
            tran_cat_type_desc: Alnum::move_from("REGULAR SALES DRAFT"),
            ..record
        };
        for different in [other_category, other_type, master_case] {
            assert_ne!(different, record);
        }
    }
}
