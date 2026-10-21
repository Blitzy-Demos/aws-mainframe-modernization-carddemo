//! `TRAN-CAT-BAL-RECORD` of copybook `CVTRA01Y`, the 50-byte transaction category balance record
//! (`CVTRA01Y.cpy:4-10`; CVTRA01Y-001 to CVTRA01Y-005).
//!
//! The fixture `tcatbal.txt` holds 50 of these records, one per line, each with the key
//! (account n, `01`, `0001`), a zero balance and 22 `0` characters of `FILLER` (FIXFRM-007,
//! FIXFRM-012, E05). The CRLF and LF terminators of those lines (E06) are line framing, which
//! `copybook::framing` records and re-emits; this module sees only the 50-byte body.

use copybook::{Error, FixedRecord, PicU, PicX, RecordReader, RecordWriter};

use super::{put_zoned_money, zoned_money};
use crate::money::Money;

// `TRANCAT-ACCT-ID` 11, `TRANCAT-TYPE-CD` 2, `TRANCAT-CD` 4, `TRAN-CAT-BAL` 11 and `FILLER` 22
// bytes (CVTRA01Y-001 to CVTRA01Y-005) fill the record exactly: `RECLN = 50` (`CVTRA01Y.cpy:2`),
// the `RECORDSIZE(50 50)` of the cluster (`TCATBALF.jcl:41`).
const _: () = assert!(11 + 2 + 4 + 11 + 22 == TranCatBalanceRecord::WIDTH);

/// One balance per account, transaction type and transaction category: `TRAN-CAT-BAL-RECORD`
/// of `CVTRA01Y`, read and rewritten by CBTRN02C and CBACT04C.
///
/// The group `TRAN-CAT-KEY` is the first three fields, the leading 17 bytes: the KSDS key
/// `KEYS(17 0)` (`TCATBALF.jcl:40`) and the primary key (`acct_id`, `tran_type_cd`,
/// `tran_cat_cd`) of table `tran_cat_balances`. It has no field of its own: its bytes are those
/// of [`acct_id`](Self::acct_id), [`type_cd`](Self::type_cd) and [`cd`](Self::cd), in that order.
///
/// Every field is kept so that [`FixedRecord::encode`] returns the bytes
/// [`FixedRecord::decode`] read: the key digits with their leading zeros, the balance with its
/// overpunch sign byte, and the `FILLER` whatever it holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranCatBalanceRecord {
    /// `TRANCAT-ACCT-ID PIC 9(11)`, offset 0, 11 bytes: the account, first part of
    /// `TRAN-CAT-KEY`; a 64-bit value, as every identifier of 10 or more digits is (PIC-023;
    /// CVTRA01Y-001).
    pub acct_id: PicU<11>,
    /// `TRANCAT-TYPE-CD PIC X(02)`, offset 11, 2 bytes: the transaction type, second part of
    /// `TRAN-CAT-KEY`, kept verbatim (CVTRA01Y-002).
    pub type_cd: PicX<2>,
    /// `TRANCAT-CD PIC 9(04)`, offset 13, 4 bytes: the transaction category, third part of
    /// `TRAN-CAT-KEY` (CVTRA01Y-003).
    pub cd: PicU<4>,
    /// `TRAN-CAT-BAL PIC S9(09)V99`, offset 17, 11 bytes: the balance, signed DISPLAY with a
    /// trailing overpunch sign (PIC-005, PIC-006); every result stored into it passes the
    /// truncating `store` of its picture (CVTRA01Y-004).
    pub bal: Money<9>,
    /// `FILLER PIC X(22)`, offset 28, 22 bytes, kept byte for byte, the `0` characters of every
    /// `tcatbal.txt` record included: never blanked (PIC-012, E05; CVTRA01Y-005).
    pub filler: PicX<22>,
}

impl FixedRecord for TranCatBalanceRecord {
    const WIDTH: usize = 50;

    // The record ends with `FILLER PIC X(22)` (CVTRA01Y-005), so a line body missing at most
    // those 22 bytes decodes with spaces there and re-emits at its own length (PIC-013). No
    // `tcatbal.txt` body is short (FIXFRM-007); a body ending inside the balance is an error.
    const TRAILING_FILLER: usize = 22;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            acct_id: r.pic_u("TRANCAT-ACCT-ID")?,
            type_cd: r.pic_x("TRANCAT-TYPE-CD")?,
            cd: r.pic_u("TRANCAT-CD")?,
            bal: zoned_money::<9>(&mut r, "TRAN-CAT-BAL")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_u(self.acct_id).pic_x(&self.type_cd).pic_u(self.cd);
        put_zoned_money(&mut w, &self.bal).pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, Framed, Line, Terminator};

    use super::TranCatBalanceRecord;
    use crate::money::{Decimal, Money};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Line 1 of `tcatbal.txt` without its CRLF, field by field: account 1, type `01`, category
    /// `0001`, balance 0.00 and 22 `0` characters of `FILLER` (FIXFRM-007, E05).
    const LINE_1: [&[u8]; 5] = [
        b"00000000001",
        b"01",
        b"0001",
        b"0000000000{",
        b"0000000000000000000000",
    ];

    /// `(field, offset, width)` of each field as `copybooks.md` lists them (CVTRA01Y-001 to
    /// CVTRA01Y-005).
    const LAYOUT: [(&str, usize, usize); 5] = [
        ("TRANCAT-ACCT-ID", 0, 11),
        ("TRANCAT-TYPE-CD", 11, 2),
        ("TRANCAT-CD", 13, 4),
        ("TRAN-CAT-BAL", 17, 11),
        ("FILLER", 28, 22),
    ];

    /// The bytes of [`LINE_1`] with field `index` replaced by `image`.
    fn line_1_with(index: usize, image: &[u8]) -> Vec<u8> {
        LINE_1
            .iter()
            .enumerate()
            .flat_map(|(i, &field)| if i == index { image } else { field })
            .copied()
            .collect()
    }

    // audit: CVTRA01Y-001
    // audit: CVTRA01Y-002
    // audit: CVTRA01Y-003
    // audit: CVTRA01Y-005
    #[test]
    fn audited_fields_are_contiguous_and_fill_the_record() {
        let mut offset = 0;
        for (_, at, width) in LAYOUT {
            assert_eq!(at, offset);
            offset += width;
        }
        assert_eq!(offset, TranCatBalanceRecord::WIDTH);
        assert_eq!(TranCatBalanceRecord::TRAILING_FILLER, 22);
    }

    // audit: FIXFRM-007
    // audit: FIXFRM-012
    // audit: CVTRA01Y-001
    // audit: CVTRA01Y-002
    // audit: CVTRA01Y-003
    // audit: CVTRA01Y-004
    // audit: CVTRA01Y-005
    // audit: PIC-012
    #[test]
    fn line_1_round_trips_with_zero_balance_and_zero_filler() -> TestResult {
        let bytes = LINE_1.concat();
        let record = TranCatBalanceRecord::decode(&bytes)?;
        assert_eq!(record.acct_id.get(), 1);
        assert_eq!(record.type_cd.as_bytes(), b"01");
        assert_eq!(record.cd.get(), 1);
        assert!(record.bal.is_zero());
        assert_eq!(record.bal, Money::<9>::ZERO);
        assert_eq!(record.filler.as_bytes(), &[b'0'; 22]);
        let encoded = record.encode();
        assert_eq!(encoded.len(), TranCatBalanceRecord::WIDTH);
        assert_eq!(encoded, bytes);
        Ok(())
    }

    // audit: CVTRA01Y-004
    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn signed_balances_round_trip_their_overpunch_byte() -> TestResult {
        let cases: [(&[u8; 11], i64); 7] = [
            (b"0000001234M", -12_344),
            (b"0000000010}", -100),
            (b"0000000000J", -1),
            (b"9999999999R", -99_999_999_999),
            (b"0000000000A", 1),
            (b"0000019400{", 194_000),
            (b"9999999999I", 99_999_999_999),
        ];
        for (image, cents) in cases {
            let bytes = line_1_with(3, image);
            let record = TranCatBalanceRecord::decode(&bytes)?;
            assert_eq!(record.bal, Money::<9>::new(Decimal::new(cents, 2))?);
            assert_eq!(record.bal.is_negative(), cents < 0);
            assert_eq!(record.encode(), bytes);
        }
        Ok(())
    }

    #[test]
    fn a_slice_of_any_other_length_is_a_record_width_error() {
        for len in [0, 17, 28, 49, 51, 100] {
            let bytes = vec![b'0'; len];
            assert_eq!(
                TranCatBalanceRecord::decode(&bytes).err(),
                Some(Error::new(
                    "record",
                    0,
                    50,
                    ErrorKind::WidthMismatch {
                        expected: 50,
                        actual: len,
                    },
                )),
            );
        }
    }

    // audit: PIC-002
    // audit: PIC-004
    // audit: PIC-007
    // audit: PIC-008
    // audit: PIC-009
    #[test]
    fn field_errors_name_the_field_at_its_audited_offset_and_width() -> TestResult {
        let cases: [(usize, &[u8], ErrorKind); 8] = [
            (0, b"0000000000 ", ErrorKind::NonDigit),
            (1, b"0\x01", ErrorKind::NonPrintable),
            (2, b"00x1", ErrorKind::NonDigit),
            (3, b"00000000000", ErrorKind::PlainDigitSign),
            (3, b"0000000000}", ErrorKind::NegativeZero),
            (3, b"0000000000?", ErrorKind::InvalidOverpunch),
            (3, b"000000000 {", ErrorKind::NonDigit),
            (4, b"000000000000000000000\x00", ErrorKind::NonPrintable),
        ];
        for (index, image, kind) in cases {
            let &(field, offset, width) = LAYOUT.get(index).ok_or("no field at this index")?;
            assert_eq!(
                TranCatBalanceRecord::decode(&line_1_with(index, image)).err(),
                Some(Error::new(field, offset, width, kind)),
            );
        }
        Ok(())
    }

    // audit: PIC-013
    #[test]
    fn a_body_short_only_of_filler_decodes_with_spaces_and_re_emits_short() -> TestResult {
        let full = LINE_1.concat();
        let key_and_balance = full.get(..28).ok_or("line 1 is shorter than 28 bytes")?;
        let framed: Framed<TranCatBalanceRecord> =
            Framed::decode(Line::new(key_and_balance, Terminator::CrLf))?;
        assert_eq!(framed.record.filler.as_bytes(), &[b' '; 22]);
        assert_eq!(framed.framing.body_len, 28);
        assert_eq!(framed.encode(), [key_and_balance, b"\r\n"].concat());

        let into_balance = full.get(..27).ok_or("line 1 is shorter than 27 bytes")?;
        assert_eq!(
            Framed::<TranCatBalanceRecord>::decode(Line::new(into_balance, Terminator::CrLf)).err(),
            Some(Error::new(
                "record",
                0,
                50,
                ErrorKind::ShortBodyNotFiller {
                    expected: 50,
                    actual: 27,
                },
            )),
        );
        Ok(())
    }
}
