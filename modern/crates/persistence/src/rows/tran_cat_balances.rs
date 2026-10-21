//! Codec B for table `tran_cat_balances`: `TRAN-CAT-BAL-RECORD` of copybook `CVTRA01Y`, the
//! 50-byte transaction category balance record (`CVTRA01Y.cpy:4-10`; CVTRA01Y-001 to
//! CVTRA01Y-005).
//!
//! The row holds the record's five fields as the five columns of migration 0001, in copybook
//! order. The three parts of `TRAN-CAT-KEY` form the primary key (`acct_id`, `tran_type_cd`,
//! `tran_cat_cd`), as they form the KSDS key `KEYS(17 0)` (`TCATBALF.jcl:40`). Every
//! `tcatbal.txt` record carries 22 `0` characters of `FILLER`, which `filler CHAR(22)` keeps
//! byte for byte (PIC-012, E05). The CRLF that ends 49 of its 50 lines is line framing, which
//! codec A records and re-emits and no column stores (PIC-014, E06).

use domain::records::TranCatBalanceRecord;
use rust_decimal::Decimal;

use super::{
    decimal_to_money, i64_to_pic_u, money_to_decimal, pic_u_to_i64, pic_x_to_string,
    string_to_pic_x,
};
use crate::error::PersistenceError;

/// Column of `TRANCAT-ACCT-ID` (CVTRA01Y-001).
const ACCT_ID: &str = "acct_id";
/// Column of `TRANCAT-TYPE-CD` (CVTRA01Y-002).
const TRAN_TYPE_CD: &str = "tran_type_cd";
/// Column of `TRANCAT-CD` (CVTRA01Y-003).
const TRAN_CAT_CD: &str = "tran_cat_cd";
/// Column of `TRAN-CAT-BAL` (CVTRA01Y-004).
const TRAN_CAT_BAL: &str = "tran_cat_bal";
/// Column of the record's `FILLER` (CVTRA01Y-005).
const FILLER: &str = "filler";

/// One row of `tran_cat_balances`: a [`TranCatBalanceRecord`] as its columns hold it, read and
/// rewritten by CBTRN02C's posting and CBACT04C's interest run.
///
/// `Debug` stays underived, as for every row type (`rows` module documentation).
pub struct TranCatBalanceRow {
    /// `acct_id BIGINT CHECK (acct_id BETWEEN 0 AND 99999999999)`: `TRANCAT-ACCT-ID PIC 9(11)`,
    /// first part of the key, 64-bit end to end (PIC-003, PIC-023; CVTRA01Y-001).
    pub(crate) acct_id: i64,
    /// `tran_type_cd CHAR(2)`: `TRANCAT-TYPE-CD PIC X(02)`, second part of the key, its two
    /// bytes verbatim (PIC-001; CVTRA01Y-002).
    pub(crate) tran_type_cd: String,
    /// `tran_cat_cd BIGINT CHECK (tran_cat_cd BETWEEN 0 AND 9999)`: `TRANCAT-CD PIC 9(04)`,
    /// third part of the key (PIC-003; CVTRA01Y-003).
    pub(crate) tran_cat_cd: i64,
    /// `tran_cat_bal NUMERIC(11,2)`: `TRAN-CAT-BAL PIC S9(09)V99`, sign and scale kept, which
    /// is exactly the picture's range (PIC-005; CVTRA01Y-004).
    pub(crate) tran_cat_bal: Decimal,
    /// `filler CHAR(22)`: the record's `FILLER PIC X(22)`, whatever it holds, the `0`
    /// characters of every `tcatbal.txt` record included (PIC-012, E05; CVTRA01Y-005).
    pub(crate) filler: String,
}

impl TranCatBalanceRow {
    /// The row that stores `record`: keys as `BIGINT`, the balance exactly at scale 2, type code
    /// and `FILLER` byte for byte (CVTRA01Y-001 to CVTRA01Y-005).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] with [`DecodeKind::InvalidByte`] when the type code or
    /// `FILLER` holds a NUL or non-ASCII byte, which no `CHAR(n)` column stores byte for byte
    /// (PIC-002). The key conversions cannot fail: a `9(11)` value is below `i64::MAX`
    /// (PIC-023).
    ///
    /// [`DecodeKind::InvalidByte`]: crate::error::DecodeKind::InvalidByte
    pub fn from_record(record: &TranCatBalanceRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            acct_id: pic_u_to_i64(ACCT_ID, record.acct_id)?,
            tran_type_cd: pic_x_to_string(TRAN_TYPE_CD, &record.type_cd)?,
            tran_cat_cd: pic_u_to_i64(TRAN_CAT_CD, record.cd)?,
            tran_cat_bal: money_to_decimal(record.bal),
            filler: pic_x_to_string(FILLER, &record.filler)?,
        })
    }

    /// The record this row stores, equal to the one [`from_record`](Self::from_record) was
    /// given, so codec A re-encodes it to the original bytes (d9). Nothing is rounded, trimmed
    /// or padded: a stored value its picture cannot hold is an error (R7).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the column, with:
    /// - [`DecodeKind::OutOfRange`] for a negative `acct_id` or one above 11 digits, a
    ///   `tran_cat_cd` outside `0..=9999`, or a `tran_cat_bal` of 10 or more integer digits or
    ///   with a non-zero third decimal (PIC-003, PIC-005);
    /// - [`DecodeKind::WidthMismatch`] when `tran_type_cd` is not 2 bytes or `filler` not 22;
    /// - [`DecodeKind::InvalidByte`] for a NUL or non-ASCII character in either (PIC-002).
    ///
    /// [`DecodeKind::OutOfRange`]: crate::error::DecodeKind::OutOfRange
    /// [`DecodeKind::WidthMismatch`]: crate::error::DecodeKind::WidthMismatch
    /// [`DecodeKind::InvalidByte`]: crate::error::DecodeKind::InvalidByte
    pub fn into_record(self) -> Result<TranCatBalanceRecord, PersistenceError> {
        Ok(TranCatBalanceRecord {
            acct_id: i64_to_pic_u::<11>(ACCT_ID, self.acct_id)?,
            type_cd: string_to_pic_x::<2>(TRAN_TYPE_CD, &self.tran_type_cd)?,
            cd: i64_to_pic_u::<4>(TRAN_CAT_CD, self.tran_cat_cd)?,
            bal: decimal_to_money::<9>(TRAN_CAT_BAL, self.tran_cat_bal)?,
            filler: string_to_pic_x::<22>(FILLER, &self.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::framing::{decode_all, encode_all};
    use copybook::{Framed, PicU, PicX, Terminator};
    use domain::money::Money;
    use domain::records::TranCatBalanceRecord;
    use rust_decimal::Decimal;

    use super::TranCatBalanceRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The fixture of the TCATBALF load, read in place (AAP 0.2.2).
    const TCATBAL: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../app/data/ASCII/tcatbal.txt"
    );

    /// A record with `tcatbal.txt`'s key shape and `FILLER`, and the balance `bal`.
    fn record(
        acct_id: u64,
        type_cd: [u8; 2],
        cd: u64,
        bal: &str,
    ) -> Result<TranCatBalanceRecord, Box<dyn StdError>> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(acct_id)?,
            type_cd: PicX::from_array(type_cd),
            cd: PicU::new(cd)?,
            bal: Money::new(bal.parse::<Decimal>()?)?,
            filler: PicX::from_array([b'0'; 22]),
        })
    }

    // audit: CVTRA01Y-001
    // audit: CVTRA01Y-002
    // audit: CVTRA01Y-003
    // audit: CVTRA01Y-004
    // audit: CVTRA01Y-005
    // audit: PIC-005
    // audit: PIC-012
    #[test]
    fn records_round_trip_through_their_columns_sign_and_scale_kept() -> TestResult {
        let cases: [(u64, [u8; 2], u64, &str); 5] = [
            (1, *b"01", 1, "0.00"),
            (50, *b"01", 1, "-123.44"),
            (99_999_999_999, *b"07", 9_999, "999999999.99"),
            (0, *b"  ", 0, "-999999999.99"),
            (12, *b"02", 5, "0.01"),
        ];
        for (acct_id, type_cd, cd, bal) in cases {
            let original = record(acct_id, type_cd, cd, bal)?;
            let row = TranCatBalanceRow::from_record(&original)?;
            assert_eq!(u64::try_from(row.acct_id)?, acct_id);
            assert_eq!(row.tran_type_cd.as_bytes(), &type_cd);
            assert_eq!(u64::try_from(row.tran_cat_cd)?, cd);
            assert_eq!(row.tran_cat_bal, bal.parse::<Decimal>()?);
            assert_eq!(row.tran_cat_bal.scale(), 2);
            assert_eq!(
                row.tran_cat_bal.is_sign_negative(),
                original.bal.is_negative()
            );
            assert_eq!(row.filler, "0".repeat(22));
            assert_eq!(row.into_record()?, original);
        }
        Ok(())
    }

    /// A row of `record(1, *b"01", 1, "0.00")` with one column replaced by `change`.
    fn row_with(
        change: impl FnOnce(&mut TranCatBalanceRow),
    ) -> Result<TranCatBalanceRow, Box<dyn StdError>> {
        let mut row = TranCatBalanceRow::from_record(&record(1, *b"01", 1, "0.00")?)?;
        change(&mut row);
        Ok(row)
    }

    // audit: PIC-003
    // audit: PIC-005
    // audit: CVTRA01Y-004
    #[test]
    fn stored_values_the_picture_cannot_hold_are_errors_naming_their_column() -> TestResult {
        let out_of_range = DecodeKind::OutOfRange;
        let cases: [(TranCatBalanceRow, &str, DecodeKind); 9] = [
            (
                row_with(|row| row.tran_cat_bal = Decimal::new(100_000_000_000, 2))?,
                "tran_cat_bal",
                out_of_range,
            ),
            (
                row_with(|row| row.tran_cat_bal = Decimal::new(-100_000_000_000, 2))?,
                "tran_cat_bal",
                out_of_range,
            ),
            (
                row_with(|row| row.tran_cat_bal = Decimal::new(1_234, 3))?,
                "tran_cat_bal",
                out_of_range,
            ),
            (row_with(|row| row.acct_id = -1)?, "acct_id", out_of_range),
            (
                row_with(|row| row.acct_id = 100_000_000_000)?,
                "acct_id",
                out_of_range,
            ),
            (
                row_with(|row| row.tran_cat_cd = 10_000)?,
                "tran_cat_cd",
                out_of_range,
            ),
            (
                row_with(|row| row.filler = "0".repeat(21))?,
                "filler",
                DecodeKind::WidthMismatch {
                    expected: 22,
                    actual: 21,
                },
            ),
            (
                row_with(|row| row.tran_type_cd = "011".to_owned())?,
                "tran_type_cd",
                DecodeKind::WidthMismatch {
                    expected: 2,
                    actual: 3,
                },
            ),
            (
                row_with(|row| row.tran_type_cd = "0\0".to_owned())?,
                "tran_type_cd",
                DecodeKind::InvalidByte,
            ),
        ];
        for (row, column, kind) in cases {
            assert_eq!(
                row.into_record(),
                Err(PersistenceError::decode(column, kind))
            );
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn a_byte_no_char_column_stores_is_an_error_naming_its_column() -> TestResult {
        let mut type_nul = record(1, *b"01", 1, "0.00")?;
        type_nul.type_cd = PicX::from_array([b'0', 0]);
        let mut filler_non_ascii = record(1, *b"01", 1, "0.00")?;
        filler_non_ascii.filler = PicX::from_array([0x80; 22]);
        for (original, column) in [(type_nul, "tran_type_cd"), (filler_non_ascii, "filler")] {
            assert_eq!(
                TranCatBalanceRow::from_record(&original).map(drop),
                Err(PersistenceError::decode(column, DecodeKind::InvalidByte))
            );
        }
        Ok(())
    }

    // audit: FIXFRM-007
    // audit: FIXFRM-012
    // audit: PIC-014
    #[test]
    fn every_tcatbal_line_survives_the_columns_byte_for_byte() -> TestResult {
        let input = std::fs::read(TCATBAL)?;
        let decoded = decode_all::<TranCatBalanceRecord>(&input)?;
        assert_eq!(decoded.len(), 50);
        let crlf = decoded
            .iter()
            .filter(|framed| framed.framing.terminator == Terminator::CrLf)
            .count();
        assert_eq!(crlf, 49);
        let mut stored = Vec::with_capacity(decoded.len());
        for framed in decoded {
            let row = TranCatBalanceRow::from_record(&framed.record)?;
            assert_eq!(row.filler, "0".repeat(22));
            stored.push(Framed {
                record: row.into_record()?,
                framing: framed.framing,
            });
        }
        assert_eq!(encode_all(&stored), input);
        Ok(())
    }
}
