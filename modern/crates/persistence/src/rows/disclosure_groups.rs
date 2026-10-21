//! Codec B for table `disclosure_groups` (migration 0001): the `CVTRA02Y` `DIS-GROUP-RECORD`
//! layout (`CVTRA02Y.cpy:4-10`), rows CVTRA02Y-001 to CVTRA02Y-005 of `modern/audit/copybooks.md`.
//!
//! The table replaces the KSDS `DISCGRP`, keyed `KEYS(16 0)` on `DIS-GROUP-KEY` and sized
//! `RECORDSIZE(50 50)` (`DISCGRP.jcl:40-41`), so its primary key is (`group_id`, `tran_type_cd`,
//! `tran_cat_cd`). The `discgrp` load writes the 51 `discgrp.txt` records through this module,
//! and CBACT04C's rate lookup reads them back through it.
//!
//! | Column | Type | Field | Picture | Conversion |
//! |---|---|---|---|---|
//! | `group_id` | `CHAR(10)` | `DIS-ACCT-GROUP-ID` | `X(10)` | every byte kept (CVTRA02Y-001, PIC-001) |
//! | `tran_type_cd` | `CHAR(2)` | `DIS-TRAN-TYPE-CD` | `X(02)` | every byte kept (CVTRA02Y-002, PIC-001) |
//! | `tran_cat_cd` | `BIGINT`, `0 .. 9999` | `DIS-TRAN-CAT-CD` | `9(04)` | checked, 64-bit (CVTRA02Y-003, PIC-003, PIC-023) |
//! | `dis_int_rate` | `NUMERIC(6,2)` | `DIS-INT-RATE` | `S9(04)V99` | exact rate at scale 2 (CVTRA02Y-004, PIC-005) |
//! | `filler` | `CHAR(28)` | `FILLER` | `X(28)` | every byte kept (CVTRA02Y-005, PIC-012, E05) |
//!
//! # Fixture shape
//!
//! Every `discgrp.txt` record holds 28 `0` characters in its `FILLER`; they are stored and read
//! back as they stand, never blanked (E05). Every `acctdata.txt` account leaves `ACCT-GROUP-ID`
//! blank (E07), so CBACT04C reads the `DEFAULT` records; `DEFAULT` is stored with its three
//! padding spaces, exactly as the record holds it, and the fallback to that group belongs to the
//! interest rules, never to this conversion.
//!
//! # The rate
//!
//! `DIS-INT-RATE` travels as a [`Rate<4>`](domain::money::Rate) in the record and as a
//! `Decimal` in the column, converted only by `rate_to_decimal` and `decimal_to_rate`: the
//! value is exact, never rounded, and keeps its sign, so codec A re-encodes the overpunched sign
//! byte it was decoded from (PIC-005, PIC-006). A zero rate stays zero, so CBACT04C's
//! `IF DIS-INT-RATE NOT = 0` (`CBACT04C.cbl:214`) skips the same balances after the round trip.

use domain::records::DisclosureGroupRecord;
use rust_decimal::Decimal;

use super::{
    decimal_to_rate, i64_to_pic_u, pic_u_to_i64, pic_x_to_string, rate_to_decimal, string_to_pic_x,
};
use crate::error::PersistenceError;

/// Column of `DIS-ACCT-GROUP-ID PIC X(10)` (CVTRA02Y-001).
const GROUP_ID: &str = "group_id";

/// Column of `DIS-TRAN-TYPE-CD PIC X(02)` (CVTRA02Y-002).
const TRAN_TYPE_CD: &str = "tran_type_cd";

/// Column of `DIS-TRAN-CAT-CD PIC 9(04)` (CVTRA02Y-003).
const TRAN_CAT_CD: &str = "tran_cat_cd";

/// Column of `DIS-INT-RATE PIC S9(04)V99` (CVTRA02Y-004).
const DIS_INT_RATE: &str = "dis_int_rate";

/// Column of the record's trailing `FILLER PIC X(28)` (CVTRA02Y-005).
const FILLER: &str = "filler";

/// One `disclosure_groups` row: the columns of migration 0001, in copybook field order.
///
/// Field names are the column names, so the repository's `query_as!` statements fill the struct
/// directly. `Debug` is left underived, as for every row struct in `rows`.
#[derive(Clone, PartialEq, Eq)]
pub struct DisclosureGroupRow {
    /// `group_id CHAR(10)`: `DIS-ACCT-GROUP-ID`, 10 ASCII bytes, padding included (CVTRA02Y-001).
    pub(crate) group_id: String,
    /// `tran_type_cd CHAR(2)`: `DIS-TRAN-TYPE-CD`, 2 ASCII bytes (CVTRA02Y-002).
    pub(crate) tran_type_cd: String,
    /// `tran_cat_cd BIGINT`: `DIS-TRAN-CAT-CD`, between 0 and 9999 (CVTRA02Y-003).
    pub(crate) tran_cat_cd: i64,
    /// `dis_int_rate NUMERIC(6,2)`: `DIS-INT-RATE`, the signed annual rate in percent at scale 2
    /// (CVTRA02Y-004).
    pub(crate) dis_int_rate: Decimal,
    /// `filler CHAR(28)`: the record's `FILLER`, 28 ASCII bytes kept as they stand (CVTRA02Y-005,
    /// E05).
    pub(crate) filler: String,
}

impl DisclosureGroupRow {
    /// The row that stores `record`, every field converted exactly (CVTRA02Y-001 to
    /// CVTRA02Y-005).
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) when the group, the
    /// type code or the `FILLER` holds a NUL or non-ASCII byte, which no `CHAR` column stores byte
    /// for byte (PIC-002). Codec A admits only printable ASCII into these fields, so a decoded
    /// record always converts.
    pub fn from_record(record: &DisclosureGroupRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            group_id: pic_x_to_string(GROUP_ID, &record.acct_group_id)?,
            tran_type_cd: pic_x_to_string(TRAN_TYPE_CD, &record.tran_type_cd)?,
            tran_cat_cd: pic_u_to_i64(TRAN_CAT_CD, record.tran_cat_cd)?,
            dis_int_rate: rate_to_decimal(record.int_rate),
            filler: pic_x_to_string(FILLER, &record.filler)?,
        })
    }

    /// The record this row stores, equal to the one [`from_record`](Self::from_record) was given
    /// (CVTRA02Y-001 to CVTRA02Y-005).
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the column, never its value:
    ///
    /// - [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) when `group_id`,
    ///   `tran_type_cd` or `filler` is not exactly 10, 2 or 28 bytes long;
    /// - [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or
    ///   non-ASCII character in one of them;
    /// - [`DecodeKind::OutOfRange`](crate::error::DecodeKind::OutOfRange) for a category outside
    ///   `0 ..= 9999`, or a rate whose magnitude reaches 10000 or that has a non-zero digit beyond
    ///   the second decimal place. Such a value is refused, never rounded or truncated.
    pub fn into_record(self) -> Result<DisclosureGroupRecord, PersistenceError> {
        Ok(DisclosureGroupRecord {
            acct_group_id: string_to_pic_x::<10>(GROUP_ID, &self.group_id)?,
            tran_type_cd: string_to_pic_x::<2>(TRAN_TYPE_CD, &self.tran_type_cd)?,
            tran_cat_cd: i64_to_pic_u::<4>(TRAN_CAT_CD, self.tran_cat_cd)?,
            int_rate: decimal_to_rate::<4>(DIS_INT_RATE, self.dis_int_rate)?,
            filler: string_to_pic_x::<28>(FILLER, &self.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{FixedRecord, PicX};
    use domain::records::DisclosureGroupRecord;
    use rust_decimal::Decimal;

    use super::DisclosureGroupRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// An edit that leaves a stored value its picture cannot hold.
    type Mutation = fn(&mut DisclosureGroupRow);

    /// `discgrp.txt:1` without its LF: group `A000000000`, type `01`, category `0001`, rate
    /// `00150{`, which is 15.00.
    const LINE_1: &[u8] = b"A00000000001000100150{0000000000000000000000000000";

    /// `discgrp.txt:25`: the zero `DEFAULT` rate of type `03`, for which CBACT04C computes no
    /// interest.
    const LINE_25: &[u8] = b"DEFAULT   03000100000{0000000000000000000000000000";

    /// Not in the corpus: rate −25.01, whose last digit overpunches as `J`.
    const NEGATIVE: &[u8] = b"ZEROAPR   04000200250J0000000000000000000000000000";

    /// The `FILLER` of every `discgrp.txt` line (E05).
    const ZERO_FILLER: &str = "0000000000000000000000000000";

    /// Each line with the column values its row must hold: group, type, category, and the rate
    /// in hundredths.
    const LINES: [(&[u8], &str, &str, i64, i64); 3] = [
        (LINE_1, "A000000000", "01", 1, 1_500),
        (LINE_25, "DEFAULT   ", "03", 1, 0),
        (NEGATIVE, "ZEROAPR   ", "04", 2, -2_501),
    ];

    fn row_of(line: &[u8]) -> Result<DisclosureGroupRow, Box<dyn StdError>> {
        Ok(DisclosureGroupRow::from_record(
            &DisclosureGroupRecord::decode(line)?,
        )?)
    }

    // audit: CVTRA02Y-001
    // audit: CVTRA02Y-002
    // audit: CVTRA02Y-003
    // audit: CVTRA02Y-004
    // audit: CVTRA02Y-005
    // audit: PIC-005
    // audit: PIC-006
    // audit: PIC-012
    #[test]
    fn records_round_trip_through_their_columns_byte_for_byte() -> TestResult {
        for (line, group, type_cd, cat_cd, rate_hundredths) in LINES {
            let record = DisclosureGroupRecord::decode(line)?;
            let row = DisclosureGroupRow::from_record(&record)?;
            assert_eq!(row.group_id, group);
            assert_eq!(row.tran_type_cd, type_cd);
            assert_eq!(row.tran_cat_cd, cat_cd);
            assert_eq!(row.dis_int_rate, Decimal::new(rate_hundredths, 2));
            assert_eq!(row.dis_int_rate.scale(), 2);
            assert_eq!(row.filler, ZERO_FILLER);

            let back = row.into_record()?;
            assert_eq!(back.int_rate.is_zero(), rate_hundredths == 0);
            assert_eq!(back, record);
            assert_eq!(back.encode(), line);
        }
        Ok(())
    }

    /// `PostgreSQL` returns a `NUMERIC(6,2)` value at scale 2, but a value at any scale that the
    /// picture holds exactly reads back as the same rate.
    // audit: CVTRA02Y-004
    #[test]
    fn a_rate_at_another_scale_reads_back_exactly() -> TestResult {
        for (stored, expected) in [("15", 1_500), ("15.0000", 1_500), ("-0.00", 0)] {
            let mut row = row_of(LINE_1)?;
            row.dis_int_rate = stored.parse::<Decimal>()?;
            let rate = row.into_record()?.int_rate;
            assert_eq!(rate.as_decimal(), Decimal::new(expected, 2));
            assert_eq!(rate.as_decimal().scale(), 2);
            assert_eq!(rate.is_zero(), expected == 0);
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-023
    #[test]
    fn a_stored_value_the_picture_cannot_hold_is_refused() -> TestResult {
        let out_of_range = DecodeKind::OutOfRange;
        let cases: [(Mutation, &str, DecodeKind); 10] = [
            (
                |row| row.dis_int_rate = Decimal::new(1_000_000, 2),
                "dis_int_rate",
                out_of_range,
            ),
            (
                |row| row.dis_int_rate = Decimal::new(-1_000_000, 2),
                "dis_int_rate",
                out_of_range,
            ),
            (
                |row| row.dis_int_rate = Decimal::new(1_005, 3),
                "dis_int_rate",
                out_of_range,
            ),
            (|row| row.tran_cat_cd = -1, "tran_cat_cd", out_of_range),
            (|row| row.tran_cat_cd = 10_000, "tran_cat_cd", out_of_range),
            (
                |row| row.filler = "0".repeat(27),
                "filler",
                DecodeKind::WidthMismatch {
                    expected: 28,
                    actual: 27,
                },
            ),
            (
                |row| row.filler = "0".repeat(29),
                "filler",
                DecodeKind::WidthMismatch {
                    expected: 28,
                    actual: 29,
                },
            ),
            (
                |row| row.group_id = "DEFAULT".to_owned(),
                "group_id",
                DecodeKind::WidthMismatch {
                    expected: 10,
                    actual: 7,
                },
            ),
            (
                |row| row.tran_type_cd = "0\u{e9}".to_owned(),
                "tran_type_cd",
                DecodeKind::InvalidByte,
            ),
            (
                |row| row.group_id = "A00000000\0".to_owned(),
                "group_id",
                DecodeKind::InvalidByte,
            ),
        ];
        for (mutate, column, kind) in cases {
            let mut row = row_of(LINE_1)?;
            mutate(&mut row);
            assert_eq!(
                row.into_record(),
                Err(PersistenceError::decode(column, kind))
            );
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn a_record_byte_no_char_column_stores_is_refused() -> TestResult {
        let valid = DisclosureGroupRecord::decode(LINE_1)?;
        let cases = [
            (
                DisclosureGroupRecord {
                    acct_group_id: PicX::from_array(*b"A00000000\xC3"),
                    ..valid.clone()
                },
                "group_id",
            ),
            (
                DisclosureGroupRecord {
                    tran_type_cd: PicX::from_array([b'0', 0]),
                    ..valid.clone()
                },
                "tran_type_cd",
            ),
            (
                DisclosureGroupRecord {
                    filler: PicX::from_array([0x80; 28]),
                    ..valid
                },
                "filler",
            ),
        ];
        for (record, column) in cases {
            assert!(
                DisclosureGroupRow::from_record(&record)
                    == Err(PersistenceError::decode(column, DecodeKind::InvalidByte)),
                "{column}"
            );
        }
        Ok(())
    }
}
