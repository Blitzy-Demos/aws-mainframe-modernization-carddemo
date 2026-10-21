//! `CVTRA02Y` `DIS-GROUP-RECORD`, the disclosure group record: 50 bytes (`CVTRA02Y.cpy:4-10`,
//! rows CVTRA02Y-001 to CVTRA02Y-005 of `modern/audit/copybooks.md`).
//!
//! The VSAM KSDS `DISCGRP` holds one record per (group, type, category) key, `KEYS(16 0)` and
//! `RECORDSIZE(50 50)` (`DISCGRP.jcl:40-41`). Its fixture is `app/data/ASCII/discgrp.txt`: 51
//! LF-terminated lines of 50 bytes each (FIXFRM-006), loaded into table `disclosure_groups` by
//! the `discgrp` subcommand (FIXLOAD-005). CBACT04C reads it for the interest rate of each
//! category balance (CBACT04C-010, FIXINT-902).

use copybook::{Error, FixedRecord, PicU, PicX, RecordReader, RecordWriter};

use super::{put_zoned_rate, zoned_rate};
use crate::money::Rate;

/// Copybook name of `DIS-ACCT-GROUP-ID PIC X(10)` (CVTRA02Y-001), for field errors.
const ACCT_GROUP_ID: &str = "DIS-ACCT-GROUP-ID";

/// Copybook name of `DIS-TRAN-TYPE-CD PIC X(02)` (CVTRA02Y-002), for field errors.
const TRAN_TYPE_CD: &str = "DIS-TRAN-TYPE-CD";

/// Copybook name of `DIS-TRAN-CAT-CD PIC 9(04)` (CVTRA02Y-003), for field errors.
const TRAN_CAT_CD: &str = "DIS-TRAN-CAT-CD";

/// Copybook name of `DIS-INT-RATE PIC S9(04)V99` (CVTRA02Y-004), for field errors.
const INT_RATE: &str = "DIS-INT-RATE";

/// Copybook name of the record's trailing `FILLER PIC X(28)` (CVTRA02Y-005), for field errors.
const FILLER: &str = "FILLER";

// The five fields fill the record exactly, 10 + 2 + 4 + 6 + 28 = 50 = `RECORDSIZE(50 50)`: the
// three `PIC X` widths, the 4 digits of `DIS-TRAN-CAT-CD PIC 9(04)` and the 6 of `DIS-INT-RATE
// PIC S9(04)V99`, whose sign is overpunched on its last digit and takes no byte (PIC-005). The
// widths are literals because the numeric cursors expose none; a unit test below reads each
// field's offset and width back from the decoder's errors.
const _: () = assert!(
    PicX::<10>::WIDTH + PicX::<2>::WIDTH + 4 + 6 + PicX::<28>::WIDTH
        == DisclosureGroupRecord::WIDTH
);

/// One disclosure group record, `CVTRA02Y` `DIS-GROUP-RECORD`: the interest rate that applies to
/// one transaction type and category within one account group.
///
/// | Field | COBOL name | PIC | Offset | Bytes | Rule |
/// |---|---|---|---|---|---|
/// | [`acct_group_id`](Self::acct_group_id) | `DIS-ACCT-GROUP-ID` | `X(10)` | 0 | 10 | CVTRA02Y-001 |
/// | [`tran_type_cd`](Self::tran_type_cd) | `DIS-TRAN-TYPE-CD` | `X(02)` | 10 | 2 | CVTRA02Y-002 |
/// | [`tran_cat_cd`](Self::tran_cat_cd) | `DIS-TRAN-CAT-CD` | `9(04)` | 12 | 4 | CVTRA02Y-003 |
/// | [`int_rate`](Self::int_rate) | `DIS-INT-RATE` | `S9(04)V99` | 16 | 6 | CVTRA02Y-004 |
/// | [`filler`](Self::filler) | `FILLER` | `X(28)` | 22 | 28 | CVTRA02Y-005 |
///
/// The first three fields form `DIS-GROUP-KEY`, the KSDS key `KEYS(16 0)` and the primary key
/// (`group_id`, `tran_type_cd`, `tran_cat_cd`) of `disclosure_groups`.
///
/// # The `DEFAULT` group
///
/// CBACT04C looks a rate up under the account's `ACCT-GROUP-ID` and, when no record has that
/// key, again under group `DEFAULT` (the seven letters and three spaces) with the same type and
/// category (CBACT04C-010). Every `acctdata.txt` account leaves `ACCT-GROUP-ID` blank and carries
/// its group code in `ACCT-ADDR-ZIP` instead (E07, FIXFRM-014), so every fixture lookup reads a
/// `DEFAULT` record: lines 18-34 of `discgrp.txt` (FIXINT-902). The `A000000000` and `ZEROAPR`
/// records load and round-trip but are never read by the interest job. The record keeps the group
/// bytes as stored; the fallback belongs to the interest rules, not to this layout.
///
/// # Byte identity
///
/// [`encode`](FixedRecord::encode) reproduces every slice [`decode`](FixedRecord::decode)
/// accepts: the group and type bytes verbatim, padding included (PIC-001), the category with its
/// leading zeros (PIC-003), the rate's overpunched sign byte (PIC-006) and the `FILLER` bytes,
/// which hold 28 `0` characters on every fixture line and are kept, never blanked (PIC-012,
/// E05). The rate is a [`Rate<4>`], not money: it enters an amount only through
/// [`monthly_interest`](crate::money::monthly_interest) (R18).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisclosureGroupRecord {
    /// `DIS-ACCT-GROUP-ID PIC X(10)`, offset 0: the account group, such as `A000000000`,
    /// `DEFAULT` or `ZEROAPR` space-padded to 10 bytes (CVTRA02Y-001).
    pub acct_group_id: PicX<10>,
    /// `DIS-TRAN-TYPE-CD PIC X(02)`, offset 10: the transaction type code (CVTRA02Y-002).
    pub tran_type_cd: PicX<2>,
    /// `DIS-TRAN-CAT-CD PIC 9(04)`, offset 12: the transaction category code (CVTRA02Y-003).
    pub tran_cat_cd: PicU<4>,
    /// `DIS-INT-RATE PIC S9(04)V99`, offset 16: the annual interest rate in percent, signed
    /// DISPLAY with a trailing overpunch (CVTRA02Y-004, PIC-005). A zero rate means CBACT04C
    /// computes no interest for the balance (`CBACT04C.cbl:214`, ARITH-015).
    pub int_rate: Rate<4>,
    /// `FILLER PIC X(28)`, offset 22: kept byte for byte (CVTRA02Y-005, PIC-012, E05).
    pub filler: PicX<28>,
}

impl FixedRecord for DisclosureGroupRecord {
    const WIDTH: usize = 50;

    /// The record ends with `FILLER PIC X(28)` (CVTRA02Y-005), so a line body may stop anywhere
    /// from offset 22 on and still decode, its missing tail read as spaces and re-emitted at the
    /// body's own length (PIC-013). Every `discgrp.txt` body is the full 50 bytes (FIXFRM-006).
    const TRAILING_FILLER: usize = 28;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            acct_group_id: r.pic_x(ACCT_GROUP_ID)?,
            tran_type_cd: r.pic_x(TRAN_TYPE_CD)?,
            tran_cat_cd: r.pic_u(TRAN_CAT_CD)?,
            int_rate: zoned_rate::<4>(&mut r, INT_RATE)?,
            filler: r.pic_x(FILLER)?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.acct_group_id)
            .pic_x(&self.tran_type_cd)
            .pic_u(self.tran_cat_cd);
        put_zoned_rate(&mut w, &self.int_rate).pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::framing::{decode_all, encode_all};
    use copybook::{Error, ErrorKind, FixedRecord, PicU, PicX};

    use super::DisclosureGroupRecord;
    use crate::money::{Decimal, Rate};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The `FILLER` of every `discgrp.txt` line: 28 `0` characters (E05).
    const ZERO_FILLER: [u8; 28] = [b'0'; 28];

    /// Line 1 of `discgrp.txt` without its LF: group `A000000000`, type `01`, category `0001`,
    /// rate `00150{` = 15.00.
    const LINE_1: &[u8; 50] = b"A00000000001000100150{0000000000000000000000000000";

    /// A `discgrp.txt` line body and the field values it decodes to; its `FILLER` is
    /// [`ZERO_FILLER`].
    struct LineCase {
        line: &'static [u8],
        group: [u8; 10],
        type_cd: [u8; 2],
        cat_cd: u64,
        rate_hundredths: i64,
    }

    impl LineCase {
        /// The record the line's fields make.
        fn record(&self) -> Result<DisclosureGroupRecord, Box<dyn StdError>> {
            Ok(DisclosureGroupRecord {
                acct_group_id: PicX::from_array(self.group),
                tran_type_cd: PicX::from_array(self.type_cd),
                tran_cat_cd: PicU::new(self.cat_cd)?,
                int_rate: Rate::new(Decimal::new(self.rate_hundredths, 2))?,
                filler: PicX::from_array(ZERO_FILLER),
            })
        }
    }

    // audit: CVTRA02Y-001
    // audit: CVTRA02Y-002
    // audit: CVTRA02Y-003
    // audit: CVTRA02Y-004
    // audit: CVTRA02Y-005
    // audit: FIXFRM-006
    // audit: PIC-005
    // audit: PIC-006
    // audit: PIC-012
    #[test]
    fn fixture_lines_decode_to_their_fields_and_re_encode_byte_for_byte() -> TestResult {
        let cases = [
            LineCase {
                line: LINE_1,
                group: *b"A000000000",
                type_cd: *b"01",
                cat_cd: 1,
                rate_hundredths: 1_500,
            },
            // `discgrp.txt:18`: the `DEFAULT` rate every type `01` interest lookup reads (E07).
            LineCase {
                line: b"DEFAULT   01000100150{0000000000000000000000000000",
                group: *b"DEFAULT   ",
                type_cd: *b"01",
                cat_cd: 1,
                rate_hundredths: 1_500,
            },
            // `discgrp.txt:25`: the zero `DEFAULT` rate of type `03`, for which no interest runs.
            LineCase {
                line: b"DEFAULT   03000100000{0000000000000000000000000000",
                group: *b"DEFAULT   ",
                type_cd: *b"03",
                cat_cd: 1,
                rate_hundredths: 0,
            },
            // `discgrp.txt:35`: every `ZEROAPR` rate is zero.
            LineCase {
                line: b"ZEROAPR   01000100000{0000000000000000000000000000",
                group: *b"ZEROAPR   ",
                type_cd: *b"01",
                cat_cd: 1,
                rate_hundredths: 0,
            },
            // Not in the corpus: a negative rate, -25.01, whose last digit 1 overpunches as `J`.
            LineCase {
                line: b"ZEROAPR   04000200250J0000000000000000000000000000",
                group: *b"ZEROAPR   ",
                type_cd: *b"04",
                cat_cd: 2,
                rate_hundredths: -2_501,
            },
        ];
        for case in cases {
            let expected = case.record()?;
            let decoded = DisclosureGroupRecord::decode(case.line)?;
            assert_eq!(decoded, expected);
            assert_eq!(decoded.int_rate.is_zero(), case.rate_hundredths == 0);
            assert!(!decoded.filler.is_spaces(), "FILLER keeps its `0` bytes");
            assert_eq!(decoded.encode(), case.line);
            assert_eq!(expected.encode(), case.line);
        }
        Ok(())
    }

    // audit: CVTRA02Y-004
    #[test]
    fn line_one_rate_is_fifteen_percent_and_not_zero() -> TestResult {
        let rate = DisclosureGroupRecord::decode(LINE_1)?.int_rate;
        assert_eq!(rate, Rate::<4>::new(Decimal::new(1_500, 2))?);
        assert_eq!(rate.as_decimal().to_string(), "15.00");
        assert!(!rate.is_zero());
        Ok(())
    }

    #[test]
    fn a_slice_of_any_other_length_is_a_record_width_error() {
        for len in [0, 22, 49, 51, 1_024] {
            let bytes = vec![b'0'; len];
            assert!(matches!(
                DisclosureGroupRecord::decode(&bytes),
                Err(Error {
                    field: "record",
                    offset: 0,
                    width: 50,
                    kind: ErrorKind::WidthMismatch {
                        expected: 50,
                        actual,
                    },
                }) if actual == len
            ));
        }
    }

    // audit: PIC-002
    // audit: PIC-004
    // audit: PIC-007
    // audit: PIC-008
    // audit: PIC-009
    #[test]
    fn a_bad_field_is_an_error_naming_its_offset_and_width() {
        let cases: [([&[u8]; 5], Error); 7] = [
            (
                [b"A00000000\x01", b"01", b"0001", b"00150{", &ZERO_FILLER],
                Error::new("DIS-ACCT-GROUP-ID", 0, 10, ErrorKind::NonPrintable),
            ),
            (
                [b"A000000000", b"0\r", b"0001", b"00150{", &ZERO_FILLER],
                Error::new("DIS-TRAN-TYPE-CD", 10, 2, ErrorKind::NonPrintable),
            ),
            (
                [b"A000000000", b"01", b"0 01", b"00150{", &ZERO_FILLER],
                Error::new("DIS-TRAN-CAT-CD", 12, 4, ErrorKind::NonDigit),
            ),
            (
                [b"A000000000", b"01", b"0001", b"001500", &ZERO_FILLER],
                Error::new("DIS-INT-RATE", 16, 6, ErrorKind::PlainDigitSign),
            ),
            (
                [b"A000000000", b"01", b"0001", b"00000}", &ZERO_FILLER],
                Error::new("DIS-INT-RATE", 16, 6, ErrorKind::NegativeZero),
            ),
            (
                [b"A000000000", b"01", b"0001", b"00150?", &ZERO_FILLER],
                Error::new("DIS-INT-RATE", 16, 6, ErrorKind::InvalidOverpunch),
            ),
            (
                [
                    b"A000000000",
                    b"01",
                    b"0001",
                    b"00150{",
                    b"000000000000000000000000000\x7f",
                ],
                Error::new("FILLER", 22, 28, ErrorKind::NonPrintable),
            ),
        ];
        for (fields, expected) in cases {
            let bytes = fields.concat();
            assert_eq!(bytes.len(), DisclosureGroupRecord::WIDTH);
            assert_eq!(DisclosureGroupRecord::decode(&bytes).err(), Some(expected));
        }
    }

    // audit: PIC-013
    // audit: PIC-014
    #[test]
    fn a_body_cut_inside_the_trailing_filler_round_trips_at_its_own_length() -> TestResult {
        // Line 1 cut after `DIS-INT-RATE`, then `discgrp.txt:25` in full.
        let input = [
            b"A00000000001000100150{\n".as_slice(),
            b"DEFAULT   03000100000{0000000000000000000000000000\n",
        ]
        .concat();
        let framed = decode_all::<DisclosureGroupRecord>(&input)?;
        let short = framed.first().ok_or("the first line did not decode")?;
        assert_eq!(short.framing.body_len, 22);
        assert!(short.record.filler.is_spaces());
        assert!(!short.record.int_rate.is_zero());
        let full = framed.last().ok_or("the second line did not decode")?;
        assert_eq!(full.framing.body_len, DisclosureGroupRecord::WIDTH);
        assert_eq!(encode_all(&framed), input);

        assert_eq!(
            decode_all::<DisclosureGroupRecord>(b"A00000000001000100150\n").err(),
            Some(Error::new(
                "record",
                0,
                50,
                ErrorKind::ShortBodyNotFiller {
                    expected: 50,
                    actual: 21,
                },
            )),
        );
        Ok(())
    }
}
