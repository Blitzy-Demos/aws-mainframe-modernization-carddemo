//! The Db2 data version of a transaction type: one row of `CARDDEMO.TRANSACTION_TYPE`, held by
//! the replacement as `db2.transaction_type` (migration `0006_db2_transaction_types.sql`).
//!
//! The layout is the DCLGEN declaration `DCLTRANSACTION-TYPE` of `DCLTRTYP.dcl:35-46`, over the
//! table of `TRNTYPE.ddl:1-4`:
//!
//! | Host variable | PIC | Column | Rust |
//! |---|---|---|---|
//! | `DCL-TR-TYPE` | `X(2)` | `TR_TYPE CHAR(2) NOT NULL`, the primary key | [`PicX<2>`] |
//! | `DCL-TR-DESCRIPTION-LEN` | `S9(4) COMP` | the length of `TR_DESCRIPTION VARCHAR(50) NOT NULL` | the text's byte length |
//! | `DCL-TR-DESCRIPTION-TEXT` | `X(50)` | the text of `TR_DESCRIPTION` | [`String`] of at most 50 bytes |
//!
//! A `VARCHAR` host variable pairs a text with its length, so the row type holds the text alone
//! and its length is the text's length; [`Db2TransactionType::new`] is the only constructor and
//! enforces the 50-byte maximum. The type has no byte form and no fixture framing: CREADB21's
//! seed (`DB2LTTYP.ctl:15-23`, `'01','PURCHASE'` to `'07','ADJUSTMENT'`) is applied by migration
//! 0006, and the row mapping is `persistence::rows`.
//!
//! The Db2 version is read and written only by the ports of COTRTLIC, COTRTUPC and COBTUPDT
//! (COTRTUPC-002, COBTUPDT-026). It is a separate data version from
//! [`TransactionTypeRecord`](super::TransactionTypeRecord), the VSAM `CVTRA03Y` record that
//! `tranrept` reads, and no conversion exists between the two, so a maintenance change never
//! reaches the report lookups (E23).

use copybook::{Error, PicX};

/// Copybook name of the description text, the field a too-long description is reported under.
const DESCRIPTION_FIELD: &str = "DCL-TR-DESCRIPTION-TEXT";

/// Maximum byte length of `TR_DESCRIPTION VARCHAR(50)` and `DCL-TR-DESCRIPTION-TEXT PIC X(50)`.
const DESCRIPTION_MAX: usize = 50;

/// One row of the Db2 transaction-type table, `DCLTRANSACTION-TYPE` (`DCLTRTYP.dcl:35-46`).
///
/// The description is kept exactly as stored or as the caller supplies it: its leading and
/// trailing spaces, its case and its length are part of the value. A seeded row holds its own
/// length (`PURCHASE`, 8 bytes), while a row written through CTTU, CTLI or MNTTRDB2 holds 50
/// bytes, its text padded with spaces (COTRTUPC-023, COTRTLIC-044, COBTUPDT-010,
/// COBTUPDT-012). Trimming, upper-casing and padding belong to the program that writes the row,
/// never to this type. The fields are private so that no value can exceed the `VARCHAR(50)`
/// maximum.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Db2TransactionType {
    tr_type: PicX<2>,
    tr_description: String,
}

impl Db2TransactionType {
    /// The row with code `tr_type` (`DCL-TR-TYPE`) and description `tr_description`
    /// (`DCL-TR-DESCRIPTION-TEXT`, whose byte length is `DCL-TR-DESCRIPTION-LEN`), the
    /// description stored unchanged: never trimmed, upper-cased or padded.
    ///
    /// This is the shape of every host-variable pair the three programs bind: the fetch of
    /// COTRTUPC-009 and COTRTLIC-004, COTRTUPC's insert and update with the `FUNCTION TRIM` value
    /// of `COTRTUPC.cbl:1538-1542` (COTRTUPC-023, COTRTUPC-026), COTRTLIC's update
    /// (COTRTLIC-044) and COBTUPDT's insert and update of the full 50-byte input field
    /// (COBTUPDT-010, COBTUPDT-012).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`](copybook::ErrorKind::OutOfRange) for field
    /// `DCL-TR-DESCRIPTION-TEXT`, at offset 0 and width 50, when `tr_description` is longer than
    /// 50 bytes. The error carries no part of the text. Every other input is accepted, so the
    /// constructor never panics (Total Parsers Over Untrusted Input).
    pub fn new(tr_type: PicX<2>, tr_description: String) -> Result<Self, Error> {
        let tr_description = super::varchar::<DESCRIPTION_MAX>(DESCRIPTION_FIELD, tr_description)?;
        Ok(Self {
            tr_type,
            tr_description,
        })
    }

    /// The transaction-type code, `DCL-TR-TYPE PIC X(2)`, column `TR_TYPE CHAR(2)`, the table's
    /// primary key (`TRNTYPE.ddl:2,4`; COTRTUPC-009).
    #[must_use]
    pub fn tr_type(&self) -> &PicX<2> {
        &self.tr_type
    }

    /// The description, `DCL-TR-DESCRIPTION-TEXT`, column `TR_DESCRIPTION VARCHAR(50)`
    /// (`TRNTYPE.ddl:3`): the text up to its stored length, trailing spaces included, at most 50
    /// bytes (COTRTUPC-009, COTRTLIC-004). Its byte length is `DCL-TR-DESCRIPTION-LEN`.
    #[must_use]
    pub fn tr_description(&self) -> &str {
        &self.tr_description
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, PicX};

    use super::{DESCRIPTION_FIELD, DESCRIPTION_MAX, Db2TransactionType};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The row `code`, `description`, through [`Db2TransactionType::new`].
    fn row(code: &str, description: &str) -> Result<Db2TransactionType, Box<dyn StdError>> {
        Ok(Db2TransactionType::new(
            PicX::<2>::from_text(code)?,
            description.to_owned(),
        )?)
    }

    /// The description `new` stores for `code` and `description`.
    fn row_description(code: &str, description: &str) -> Result<String, Box<dyn StdError>> {
        Ok(row(code, description)?.tr_description().to_owned())
    }

    /// The error a description longer than `VARCHAR(50)` yields.
    const TOO_LONG: Error =
        Error::new(DESCRIPTION_FIELD, 0, DESCRIPTION_MAX, ErrorKind::OutOfRange);

    // audit: COTRTUPC-009
    // audit: COTRTLIC-004
    #[test]
    fn new_accepts_the_seed_rows_and_the_accessors_return_them() -> TestResult {
        // `DB2LTTYP.ctl:17,22,23`: the seed keeps its own lengths and its spelling (E23).
        for (code, description) in [("01", "PURCHASE"), ("06", "REVERAL"), ("07", "ADJUSTMENT")] {
            let row = row(code, description)?;
            assert_eq!(row.tr_type(), &PicX::<2>::from_text(code)?);
            assert_eq!(row.tr_type().as_bytes(), code.as_bytes());
            assert_eq!(row.tr_description(), description);
            assert_eq!(row.tr_description().len(), description.len());
        }
        Ok(())
    }

    // audit: COTRTUPC-023
    // audit: COBTUPDT-010
    // audit: COBTUPDT-012
    #[test]
    fn new_keeps_a_full_width_description_byte_for_byte() -> TestResult {
        // COTRTUPC-023 and COBTUPDT-012: a written description is 50 bytes, padded with spaces.
        let padded = format!("{:<50}", "AUDIT TEST PURCHASE RENAMED");
        assert_eq!(padded.len(), DESCRIPTION_MAX);
        let row = row("01", &padded)?;
        assert_eq!(row.tr_description(), padded);

        // Fifty non-space bytes are the most the column holds.
        let full = "X".repeat(DESCRIPTION_MAX);
        assert_eq!(row_description("02", &full)?, full);
        Ok(())
    }

    #[test]
    fn new_never_trims_cases_or_pads_the_text() -> TestResult {
        for description in ["", " ", "  leading", "trailing  ", "Mixed Case", "\tTAB"] {
            assert_eq!(row_description("99", description)?, description);
        }
        Ok(())
    }

    #[test]
    fn new_rejects_more_than_fifty_bytes() -> TestResult {
        let code = PicX::<2>::from_text("01")?;
        let long = Db2TransactionType::new(code.clone(), "X".repeat(DESCRIPTION_MAX + 1));
        assert!(matches!(
            long,
            Err(Error {
                field: DESCRIPTION_FIELD,
                offset: 0,
                width: DESCRIPTION_MAX,
                kind: ErrorKind::OutOfRange
            })
        ));
        assert_eq!(long.err(), Some(TOO_LONG));

        // The maximum counts bytes: 26 two-byte characters are 52 bytes.
        let wide = Db2TransactionType::new(code, "é".repeat(26));
        assert_eq!(wide.err(), Some(TOO_LONG));
        Ok(())
    }

    #[test]
    fn rows_compare_by_code_and_exact_description() -> TestResult {
        let seeded = row("01", "PURCHASE")?;
        assert_eq!(seeded.clone(), seeded);
        assert_ne!(seeded, row("01", "PURCHASE ")?);
        assert_ne!(seeded, row("01", "Purchase")?);
        assert_ne!(seeded, row("02", "PURCHASE")?);
        Ok(())
    }
}
