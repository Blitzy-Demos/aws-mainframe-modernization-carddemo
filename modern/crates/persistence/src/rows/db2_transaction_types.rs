//! Codec B for `db2.transaction_type`, the Db2 data version of the transaction types (migration
//! `0006_db2_transaction_types.sql`, over `TRNTYPE.ddl:1-4` and the DCLGEN layout
//! `DCLTRANSACTION-TYPE` of `DCLTRTYP.dcl:35-46`).
//!
//! | Column | Host variable | Row field | Record field | Conversion |
//! |---|---|---|---|---|
//! | `tr_type CHAR(2)`, primary key `xtran_type` | `DCL-TR-TYPE PIC X(2)` | `String` of exactly 2 bytes | `PicX<2>` | `pic_x_to_string`, `string_to_pic_x` (PIC-001, PIC-002) |
//! | `tr_description VARCHAR(50)` | `DCL-TR-DESCRIPTION-LEN PIC S9(4) COMP` and `DCL-TR-DESCRIPTION-TEXT PIC X(50)` | `String` of at most 50 bytes | `String` of at most 50 bytes | `string_to_varchar`, `varchar_to_string` |
//!
//! The row is the shape both COTRTLIC cursors select,
//! `queries/db2_transaction_types_page_forward.sql` (COTRTLIC-004) and
//! `queries/db2_transaction_types_page_backward.sql` (COTRTLIC-011), and the shape of COTRTUPC's
//! key read (`COTRTUPC.cbl:1475-1482`, COTRTUPC-009), so its fields are named as the selected
//! columns and the repository's query macros fill them by name.
//!
//! # Stored bytes
//!
//! A `VARCHAR` host variable pairs a text with its length, so the length, the leading and
//! trailing spaces and the case are all part of the stored value. Both directions convert the
//! description verbatim, and a row read back yields the record that was written. Trimming and
//! padding belong to the domain port of the program that writes the row, never to this module:
//!
//! - COTRTUPC stores the `FUNCTION TRIM` of the screen description, padded with spaces to 50
//!   bytes, with length 50, for its update and its insert alike
//!   (`COTRTUPC.cbl:1538-1547,1596-1602`; COTRTUPC-023, COTRTUPC-026).
//! - COTRTLIC's F10 update stores the trimmed description padded to 50 bytes (COTRTLIC-044).
//! - COBTUPDT stores all 50 input bytes, trailing spaces kept (COBTUPDT-010, COBTUPDT-012).
//! - A row seeded by migration 0006 keeps its own length, such as `PURCHASE` at 8 bytes.
//!
//! # Data version (E23)
//!
//! This table is not the VSAM data version in `transaction_types` that `tranrept` reads. No
//! conversion exists between [`Db2TransactionTypeRow`] and the VSAM `TransactionTypeRow` or
//! `TransactionTypeRecord`, so a CTTU, CTLI or MNTTRDB2 change never reaches the report lookups,
//! as on the mainframe before TRANEXTR runs (COTRTUPC-002, COBTUPDT-026).

use domain::records::Db2TransactionType;

use super::{
    copybook_kind, pic_x_to_string, string_to_pic_x, string_to_varchar, varchar_to_string,
};
use crate::error::PersistenceError;

/// Column of `DCL-TR-TYPE`, the primary key.
const TR_TYPE: &str = "tr_type";

/// Column of `DCL-TR-DESCRIPTION`.
const TR_DESCRIPTION: &str = "tr_description";

/// Maximum byte length of `TR_DESCRIPTION VARCHAR(50)` (`TRNTYPE.ddl:3`).
const DESCRIPTION_MAX: usize = 50;

/// One row of `db2.transaction_type`, as the two COTRTLIC page queries and COTRTUPC's key read
/// select it: `tr_type`, then `tr_description`, both `NOT NULL`.
///
/// The fields are crate-only, so only `repos` builds and binds rows. Like every row struct, it
/// leaves `Debug` underived.
pub struct Db2TransactionTypeRow {
    /// `tr_type CHAR(2)`: the code's two bytes, padding included.
    pub(crate) tr_type: String,
    /// `tr_description VARCHAR(50)`: the description as stored, its length included.
    pub(crate) tr_description: String,
}

impl Db2TransactionTypeRow {
    /// The row that stores `record`: the code's two bytes and the description verbatim, neither
    /// trimmed nor padded (COTRTUPC-023, COTRTLIC-044, COBTUPDT-010, COBTUPDT-012).
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for column `tr_type`
    /// or `tr_description` when the code or the description holds a NUL or non-ASCII byte, which
    /// a `PostgreSQL` text column cannot store byte for byte (PIC-002). The code is checked
    /// first. [`Db2TransactionType::new`] already bounds the description to 50 bytes, so a
    /// constructed record never fails the width check.
    pub fn from_record(record: &Db2TransactionType) -> Result<Self, PersistenceError> {
        Ok(Self {
            tr_type: pic_x_to_string(TR_TYPE, record.tr_type())?,
            tr_description: string_to_varchar(
                TR_DESCRIPTION,
                record.tr_description(),
                DESCRIPTION_MAX,
            )?,
        })
    }

    /// The record this row holds, its description exactly as stored, trailing spaces and length
    /// included (COTRTUPC-009, COTRTLIC-004, COTRTLIC-011).
    ///
    /// # Errors
    ///
    /// Each error names its column and the class of the failure, never the value. The code is
    /// checked first.
    ///
    /// - `tr_type`: [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL
    ///   or non-ASCII character (PIC-002);
    ///   [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) when the text is
    ///   not exactly 2 bytes long.
    /// - `tr_description`: [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte)
    ///   for a NUL or non-ASCII character;
    ///   [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) when the text is
    ///   longer than 50 bytes; [`DecodeKind::Copybook`](crate::error::DecodeKind::Copybook) with
    ///   the class [`Db2TransactionType::new`] reports, should it reject the description.
    pub fn into_record(self) -> Result<Db2TransactionType, PersistenceError> {
        let tr_type = string_to_pic_x::<2>(TR_TYPE, &self.tr_type)?;
        let tr_description =
            varchar_to_string(TR_DESCRIPTION, &self.tr_description, DESCRIPTION_MAX)?;
        Db2TransactionType::new(tr_type, tr_description)
            .map_err(|error| PersistenceError::decode(TR_DESCRIPTION, copybook_kind(&error)))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::PicX;
    use domain::records::Db2TransactionType;

    use super::{DESCRIPTION_MAX, Db2TransactionTypeRow, TR_DESCRIPTION, TR_TYPE};
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The record `code`, `description`, through the domain constructor.
    fn record(code: &str, description: &str) -> Result<Db2TransactionType, Box<dyn StdError>> {
        Ok(Db2TransactionType::new(
            PicX::<2>::from_text(code)?,
            description.to_owned(),
        )?)
    }

    /// The row the database returns for `code`, `description`.
    fn stored(code: &str, description: &str) -> Db2TransactionTypeRow {
        Db2TransactionTypeRow {
            tr_type: code.to_owned(),
            tr_description: description.to_owned(),
        }
    }

    // audit: COTRTUPC-009
    // audit: COTRTLIC-004
    #[test]
    fn seeded_rows_read_back_as_stored() -> TestResult {
        // `DB2LTTYP.ctl:17,22,23`: unpadded, in the source's spelling (E23).
        for (code, description) in [("01", "PURCHASE"), ("06", "REVERAL"), ("07", "ADJUSTMENT")] {
            let read = stored(code, description).into_record()?;
            assert_eq!(read, record(code, description)?);
            assert_eq!(read.tr_type().as_bytes(), code.as_bytes());
            assert_eq!(read.tr_description().len(), description.len());

            let written = Db2TransactionTypeRow::from_record(&read)?;
            assert_eq!(written.tr_type, code);
            assert_eq!(written.tr_description, description);
        }
        Ok(())
    }

    // audit: COTRTUPC-023
    // audit: COTRTLIC-044
    // audit: COBTUPDT-010
    // audit: COBTUPDT-012
    #[test]
    fn written_descriptions_round_trip_verbatim() -> TestResult {
        let padded = format!("{:<50}", "AUDIT TEST PURCHASE RENAMED");
        let full = "X".repeat(DESCRIPTION_MAX);
        for description in [
            padded.as_str(),
            full.as_str(),
            "PURCHASE  ",
            "  REVERAL",
            "Mixed Case",
            " ",
            "",
        ] {
            let original = record("01", description)?;
            let written = Db2TransactionTypeRow::from_record(&original)?;
            assert_eq!(written.tr_type, "01");
            assert_eq!(written.tr_description, description);
            assert_eq!(written.tr_description.len(), description.len());
            assert_eq!(written.into_record()?, original);
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn stored_values_that_do_not_fit_are_rejected() {
        let long = "X".repeat(DESCRIPTION_MAX + 1);
        let cases = [
            (
                stored("01", &long),
                TR_DESCRIPTION,
                DecodeKind::WidthMismatch {
                    expected: 50,
                    actual: 51,
                },
            ),
            (
                stored("01", "PURCHAS\u{e9}"),
                TR_DESCRIPTION,
                DecodeKind::InvalidByte,
            ),
            (
                stored("01", "PUR\0CHASE"),
                TR_DESCRIPTION,
                DecodeKind::InvalidByte,
            ),
            (
                stored("1", "PURCHASE"),
                TR_TYPE,
                DecodeKind::WidthMismatch {
                    expected: 2,
                    actual: 1,
                },
            ),
            (
                stored("123", "PURCHASE"),
                TR_TYPE,
                DecodeKind::WidthMismatch {
                    expected: 2,
                    actual: 3,
                },
            ),
            (
                stored("", "PURCHASE"),
                TR_TYPE,
                DecodeKind::WidthMismatch {
                    expected: 2,
                    actual: 0,
                },
            ),
            (
                stored("\u{e9}", "PURCHASE"),
                TR_TYPE,
                DecodeKind::InvalidByte,
            ),
            (stored("0\0", "PURCHASE"), TR_TYPE, DecodeKind::InvalidByte),
            // Both columns fail: the code is reported.
            (
                stored("1", &long),
                TR_TYPE,
                DecodeKind::WidthMismatch {
                    expected: 2,
                    actual: 1,
                },
            ),
        ];
        for (row, column, kind) in cases {
            assert_eq!(
                row.into_record(),
                Err(PersistenceError::decode(column, kind))
            );
        }
    }

    // audit: PIC-002
    #[test]
    fn records_a_text_column_cannot_store_are_rejected() -> TestResult {
        // The domain constructor bounds only the byte length, so these records exist; 25 two-byte
        // characters are exactly 50 bytes.
        let code = PicX::<2>::from_text("01")?;
        for description in ["\u{e9}".repeat(25), "A\0B".to_owned()] {
            let record = Db2TransactionType::new(code.clone(), description)?;
            assert_eq!(
                Db2TransactionTypeRow::from_record(&record).map(drop),
                Err(PersistenceError::decode(
                    TR_DESCRIPTION,
                    DecodeKind::InvalidByte
                ))
            );
        }
        for bytes in [[0x80, b'1'], [b'0', 0]] {
            let record = Db2TransactionType::new(PicX::from_array(bytes), "PURCHASE".to_owned())?;
            assert_eq!(
                Db2TransactionTypeRow::from_record(&record).map(drop),
                Err(PersistenceError::decode(TR_TYPE, DecodeKind::InvalidByte))
            );
        }
        Ok(())
    }
}
