//! Transaction types, in the two versions the COBOL keeps.
//!
//! - [`TranTypeRecord`] is the VSAM processing copy `TRANTYPE`: the `CVTRA03Y`
//!   `TRAN-TYPE-RECORD` [app/cpy/CVTRA03Y.cpy:L4-7], one row of the table
//!   `trantype_file`. Only `CBTRN03C` reads it (CR00 and TRANREPT).
//! - [`TranTypeMaster`] is a row of the Db2 table `CARDDEMO.TRANSACTION_TYPE`
//!   [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L1-4], one row of the table
//!   `transaction_types`. `COTRTUPC` (CTTU), `COTRTLIC` (CTLI) and `COBTUPDT`
//!   (MNTTRDB2) are the programs that change it.
//!
//! The two start different: the Db2 load stores `PURCHASE`
//! [app/app-transaction-type-db2/ctl/DB2LTTYP.ctl:L17] where the fixture holds
//! `Purchase` [app/data/ASCII/trantype.txt:L1]. They agree only after TRANEXTR
//! copies the masters over the processing copies (`batch seed --only
//! tranextr`), so an edit of a master reaches the reports only then. The two
//! types therefore stay separate, with no conversion between them.
//!
//! `TranTypeRecord` takes the copybook field names and `TranTypeMaster` the Db2
//! column names. Field types and widths are those of the reviewed section A rows
//! of `docs/parity/parity-oracle-audit.md` cited at each field.

use crate::numeric::{Alnum, NumericError, TranTypeCode};

/// A VSAM `TRANTYPE` record, `CVTRA03Y` `TRAN-TYPE-RECORD` (RECLN 60)
/// [app/cpy/CVTRA03Y.cpy:L1-7]: one row of `trantype_file`.
///
/// The trailing `FILLER X(08)` (P-CVTRA03Y-003) [app/cpy/CVTRA03Y.cpy:L7] is
/// not stored and has no field here; the record codec keeps its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeRecord {
    /// `TRAN-TYPE PIC X(02)` (P-CVTRA03Y-001) [app/cpy/CVTRA03Y.cpy:L5], the
    /// KSDS key `KEYS(2 0)` [app/jcl/TRANTYPE.jcl:L40]. Column `tran_type`.
    pub tran_type: TranTypeCode,
    /// `TRAN-TYPE-DESC PIC X(50)` (P-CVTRA03Y-002) [app/cpy/CVTRA03Y.cpy:L6],
    /// trailing spaces included. Column `tran_type_desc`, right-trimmed.
    pub tran_type_desc: Alnum<50>,
}

/// The most characters a `TR_DESCRIPTION` holds: the 50 bytes of
/// `VARCHAR(50)` (P-TRNTYPE-002)
/// [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L3] and of its host variable
/// text `DCL-TR-DESCRIPTION-TEXT PIC X(50)`
/// [app/app-transaction-type-db2/dcl/DCLTRTYP.dcl:L45-46], one ISO 8859-1 byte
/// per character.
pub const TR_DESCRIPTION_MAX_LEN: usize = 50;

/// A row of the Db2 master `CARDDEMO.TRANSACTION_TYPE`
/// [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L1-4]: one row of
/// `transaction_types`.
///
/// The fields are public so that tests and adapters can write a master as a
/// struct literal. [`TranTypeMaster::try_new`] is the checked path, and
/// persistence and the rules build masters through it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeMaster {
    /// `TR_TYPE CHAR(2) NOT NULL` (P-TRNTYPE-001)
    /// [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L2], the primary key
    /// [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L4] with the unique index
    /// `XTRAN_TYPE` [app/app-transaction-type-db2/ddl/XTRNTYPE.ddl:L1-3]; host
    /// variable `DCL-TR-TYPE PIC X(2)`
    /// [app/app-transaction-type-db2/dcl/DCLTRTYP.dcl:L38]. Column `tr_type`.
    pub tr_type: TranTypeCode,
    /// `TR_DESCRIPTION VARCHAR(50) NOT NULL` (P-TRNTYPE-002)
    /// [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L3], read and written
    /// through the host variable `DCL-TR-DESCRIPTION`, a VARCHAR of
    /// `DCL-TR-DESCRIPTION-LEN PIC S9(4) COMP` and `DCL-TR-DESCRIPTION-TEXT PIC
    /// X(50)` [app/app-transaction-type-db2/dcl/DCLTRTYP.dcl:L40-46]. Column
    /// `tr_description`.
    ///
    /// The text is held as given, with no padding and no trimming: the Db2
    /// value carries its own length, so the loaded `PURCHASE` stays 8
    /// characters and a description `COTRTUPC` writes blank-padded to 50
    /// (P-COTRTUPC-007) stays 50. Which form is stored is the rules' decision.
    /// Section A labels the column's type `Alnum<50>`; its width and
    /// nullability are the ones kept here, and the field is text of its own
    /// length because a fixed `X(50)` item would pad that length away.
    ///
    /// The `transaction_types` column admits only the right-trimmed form (its
    /// CHECK rejects trailing spaces), so a master read back carries none.
    pub tr_description: String,
}

impl TranTypeMaster {
    /// A master with this type code and description, checked to fit
    /// `TR_DESCRIPTION`.
    ///
    /// The description is kept exactly as given. Its length counts characters,
    /// each the one ISO 8859-1 byte Db2 and `DCL-TR-DESCRIPTION-TEXT` store it
    /// as, the way [`Alnum::from_trimmed`] measures every stored text: 50 `é`
    /// fit, although they are 100 UTF-8 bytes. The empty description is
    /// accepted, because the DDL allows an empty VARCHAR; the COBOL empty-field
    /// edits belong to the rules.
    ///
    /// # Errors
    ///
    /// [`NumericError::TooLong`] when the description has more than
    /// [`TR_DESCRIPTION_MAX_LEN`] characters, checked first, and
    /// [`NumericError::NotLatin1`] when a character lies above `U+00FF`, which
    /// no stored byte holds. Nothing is cut or replaced.
    // No `#[must_use]`: `Result` already carries it, and clippy's
    // `double_must_use` rejects the pair.
    pub fn try_new(tr_type: TranTypeCode, tr_description: String) -> Result<Self, NumericError> {
        // The stored-form check of every X(n) text, not `str::len`: a character
        // above U+007F is one stored byte but two UTF-8 bytes, so a UTF-8 count
        // would reject descriptions the column holds. The item it builds is
        // discarded; the description is kept unpadded.
        Alnum::<TR_DESCRIPTION_MAX_LEN>::from_trimmed(&tr_description)?;
        Ok(Self {
            tr_type,
            tr_description,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{TR_DESCRIPTION_MAX_LEN, TranTypeMaster, TranTypeRecord};
    use crate::numeric::{Alnum, NumericError, TranTypeCode};

    #[test]
    fn try_new_keeps_every_description_that_fits_exactly_as_given() {
        let tr_type = TranTypeCode::move_from("01");
        let cases = [
            String::new(),
            "PURCHASE".to_owned(),
            "A".repeat(TR_DESCRIPTION_MAX_LEN),
            "é".repeat(TR_DESCRIPTION_MAX_LEN),
            format!("{:<50}", "REVERSAL"),
            "  ADJUSTMENT".to_owned(),
        ];
        for tr_description in cases {
            assert_eq!(
                TranTypeMaster::try_new(tr_type, tr_description.clone()),
                Ok(TranTypeMaster {
                    tr_type,
                    tr_description: tr_description.clone(),
                }),
                "{tr_description:?}"
            );
        }
    }

    #[test]
    fn try_new_rejects_descriptions_the_column_cannot_hold() {
        let too_long = NumericError::TooLong {
            max: TR_DESCRIPTION_MAX_LEN,
            actual: 51,
        };
        let cases = [
            ("A".repeat(51), too_long),
            ("é".repeat(51), too_long),
            (format!("{:<51}", "PURCHASE"), too_long),
            ("€".to_owned(), NumericError::NotLatin1),
            (format!("{}€", "A".repeat(49)), NumericError::NotLatin1),
            (format!("{}€", "A".repeat(50)), too_long),
        ];
        for (tr_description, expected) in cases {
            assert_eq!(
                TranTypeMaster::try_new(TranTypeCode::move_from("01"), tr_description.clone()),
                Err(expected),
                "{tr_description:?}"
            );
        }
    }

    #[test]
    fn processing_copy_records_compare_and_clone_by_value() {
        // trantype.txt record 1: type 01, "Purchase" padded to 50.
        let record = TranTypeRecord {
            tran_type: TranTypeCode::move_from("01"),
            tran_type_desc: Alnum::move_from("Purchase"),
        };
        let copy = record.clone();
        assert_eq!(copy, record);
        assert_eq!(copy.tran_type_desc.trimmed(), "Purchase");
        assert_eq!(copy.tran_type_desc.as_bytes().len(), 50);

        let differences = [
            TranTypeRecord {
                tran_type: TranTypeCode::move_from("02"),
                ..record.clone()
            },
            TranTypeRecord {
                tran_type_desc: Alnum::move_from("PURCHASE"),
                ..record.clone()
            },
        ];
        for other in differences {
            assert_ne!(other, record, "{other:?}");
        }
    }
}
