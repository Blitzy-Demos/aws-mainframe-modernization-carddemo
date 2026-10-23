//! `CVTRA04Y` `TRAN-CAT-RECORD`, 60 bytes (`RECLN = 60`): one transaction category of the VSAM
//! data version (`app/cpy/CVTRA04Y.cpy:4-9`; CVTRA04Y-001 to CVTRA04Y-004).
//!
//! | Field | COBOL name | PIC | Offset | Width |
//! |---|---|---|---|---|
//! | [`type_cd`](TransactionCategoryRecord::type_cd) | `TRAN-TYPE-CD` | `X(02)` | 0 | 2 |
//! | [`cat_cd`](TransactionCategoryRecord::cat_cd) | `TRAN-CAT-CD` | `9(04)` | 2 | 4 |
//! | [`type_desc`](TransactionCategoryRecord::type_desc) | `TRAN-CAT-TYPE-DESC` | `X(50)` | 6 | 50 |
//! | [`filler`](TransactionCategoryRecord::filler) | `FILLER` | `X(04)` | 56 | 4 |
//!
//! The fixture `app/data/ASCII/trancatg.txt` holds 18 of these records, each a full 60-byte body
//! ended by CRLF (FIXFRM-008, E06). `copybook::framing` carries the terminator, so this codec
//! sees bodies only, and a CR inside a body is a typed error (PIC-002).

use copybook::{Error, FixedRecord, PicU, PicX, RecordReader, RecordWriter};

/// `TRAN-CAT-RECORD` (`CVTRA04Y.cpy:4`): a transaction category of the VSAM data version, the
/// KSDS that IDCAMS REPRO loads from `TRANCATG.PS` with `KEYS(6 0)` and `RECORDSIZE(60 60)`
/// (`TRANCATG.jcl:40-41,54-61`). Its key `TRAN-CAT-KEY` is [`type_cd`](Self::type_cd) followed
/// by [`cat_cd`](Self::cat_cd), the first six bytes.
///
/// The source holds transaction categories twice (E23). This is the VSAM version: the
/// `trancatg` load writes it to `transaction_categories`, and only `tranrept` reads it, for the
/// CBTRN03C report lookups. The Db2 version is
/// [`Db2TransactionCategory`](super::Db2TransactionCategory), seeded by migration 0006 and never
/// written by COTRTLIC, COTRTUPC or COBTUPDT: they maintain only its parent `TRANSACTION_TYPE`,
/// whose delete its rows restrict. No conversion between the two is provided, because no
/// component copies between the versions: TRANEXTR, the only source step that does, is out of
/// scope, so a Db2 maintenance change never alters a report lookup.
///
/// Every field is kept byte for byte: the description keeps its padding (PIC-001) and the
/// `FILLER` keeps whatever it holds, the four `0` characters of every fixture record included
/// (PIC-012, E05).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionCategoryRecord {
    /// `TRAN-TYPE-CD PIC X(02)`, offset 0: the transaction type code, the first part of
    /// `TRAN-CAT-KEY` and of the `transaction_categories` primary key (CVTRA04Y-001).
    pub type_cd: PicX<2>,
    /// `TRAN-CAT-CD PIC 9(04)`, offset 2: the category code, the second part of `TRAN-CAT-KEY`.
    /// Four ASCII digits, leading zeros restored on encode (CVTRA04Y-002, PIC-003); any other
    /// byte, a space included, is a typed error, never zero (PIC-004).
    pub cat_cd: PicU<4>,
    /// `TRAN-CAT-TYPE-DESC PIC X(50)`, offset 6: the category description, space-padded
    /// (CVTRA04Y-003).
    pub type_desc: PicX<50>,
    /// `FILLER PIC X(04)`, offset 56: kept verbatim, never blanked (CVTRA04Y-004, PIC-012, E05).
    pub filler: PicX<4>,
}

impl FixedRecord for TransactionCategoryRecord {
    /// `RECLN = 60` (`CVTRA04Y.cpy:2`) = `RECORDSIZE(60 60)` (`TRANCATG.jcl:41`).
    const WIDTH: usize = 60;

    // `TRAILING_FILLER` stays 0: every `trancatg.txt` body is the full 60 bytes, and the
    // trailing `FILLER` carries `0000` on every record (E05), so a body without it is a typed
    // error (R7) rather than a record whose `FILLER` is supplied as spaces (PIC-013).

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            type_cd: r.pic_x("TRAN-TYPE-CD")?,
            cat_cd: r.pic_u("TRAN-CAT-CD")?,
            type_desc: r.pic_x("TRAN-CAT-TYPE-DESC")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.type_cd)
            .pic_u(self.cat_cd)
            .pic_x(&self.type_desc)
            .pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::framing::{decode_all, encode_all};
    use copybook::{
        Error, ErrorKind, FixedRecord, Framed, Line, LineFraming, PicU, PicX, Terminator,
    };

    use super::TransactionCategoryRecord;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The body of `trancatg.txt` line 1: type `01`, category `0001`, `Regular Sales Draft`
    /// padded to 50 bytes, `FILLER` `0000` (FIXFRM-008).
    const LINE_1: &[u8; 60] = b"010001Regular Sales Draft                               0000";

    /// The body of `trancatg.txt` line 9: type `03`, category `0001`, `Credit to Account`.
    const LINE_9: &[u8; 60] = b"030001Credit to Account                                 0000";

    /// The body of `trancatg.txt` line 5: type `01`, category `0005`, `Interest Amount`.
    const LINE_5: &[u8; 60] = b"010005Interest Amount                                   0000";

    /// The 60 bytes of a record: type code, the four bytes of `TRAN-CAT-CD`, the 50-byte
    /// description of line 1 and a four-byte `FILLER`.
    fn record_bytes(cat_cd: [u8; 4], filler: [u8; 4]) -> Result<Vec<u8>, Box<dyn StdError>> {
        let desc = PicX::<50>::from_text("Regular Sales Draft")?;
        Ok([b"01".as_slice(), &cat_cd, desc.as_bytes(), &filler].concat())
    }

    // audit: CVTRA04Y-001
    // audit: CVTRA04Y-002
    // audit: CVTRA04Y-003
    // audit: CVTRA04Y-004
    #[test]
    fn fixture_bodies_decode_to_their_fields_and_re_encode_unchanged() -> TestResult {
        let cases = [
            (LINE_1, b"01", 1, "Regular Sales Draft"),
            (LINE_5, b"01", 5, "Interest Amount"),
            (LINE_9, b"03", 1, "Credit to Account"),
        ];
        for (line, type_cd, cat_cd, desc) in cases {
            let record = TransactionCategoryRecord::decode(line)?;
            assert_eq!(record.type_cd, PicX::from_array(*type_cd));
            assert_eq!(record.cat_cd.get(), cat_cd);
            assert_eq!(record.type_desc, PicX::<50>::from_text(desc)?);
            assert_eq!(record.filler, PicX::from_array(*b"0000"));
            assert!(!record.filler.is_spaces());
            let encoded = record.encode();
            assert_eq!(encoded.len(), TransactionCategoryRecord::WIDTH);
            assert_eq!(encoded, line);
        }
        Ok(())
    }

    // audit: CVTRA04Y-002
    #[test]
    fn a_constructed_record_encodes_leading_zeros_and_filler_verbatim() -> TestResult {
        let record = TransactionCategoryRecord {
            type_cd: PicX::from_array(*b"07"),
            cat_cd: PicU::new(42)?,
            type_desc: PicX::from_text("Sales draft credit adjustment")?,
            filler: PicX::from_array(*b"0 9X"),
        };
        let encoded = record.encode();
        let desc = PicX::<50>::from_text("Sales draft credit adjustment")?;
        let expected = [b"070042".as_slice(), desc.as_bytes(), b"0 9X"].concat();
        assert_eq!(encoded, expected);
        assert_eq!(TransactionCategoryRecord::decode(&encoded)?, record);
        Ok(())
    }

    #[test]
    fn a_non_digit_category_code_is_a_typed_error_at_its_field() -> TestResult {
        for cat_cd in [b"00A1", b"   1", b"+001", b"00\x001"] {
            let bytes = record_bytes(*cat_cd, *b"0000")?;
            let error = TransactionCategoryRecord::decode(&bytes).err();
            assert_eq!(
                error,
                Some(Error::new("TRAN-CAT-CD", 2, 4, ErrorKind::NonDigit)),
                "{cat_cd:?}",
            );
        }
        Ok(())
    }

    #[test]
    fn a_non_printable_byte_is_a_typed_error_at_its_field() -> TestResult {
        let bytes = record_bytes(*b"0001", *b"000\r")?;
        let error = TransactionCategoryRecord::decode(&bytes).err();
        assert!(
            matches!(
                error,
                Some(Error {
                    field: "FILLER",
                    offset: 56,
                    width: 4,
                    kind: ErrorKind::NonPrintable,
                })
            ),
            "{error:?}",
        );
        Ok(())
    }

    #[test]
    fn a_slice_of_any_other_length_is_a_record_width_error() -> TestResult {
        let short = LINE_1.get(..59).ok_or("line 1 is 60 bytes")?;
        let unframed = [LINE_1.as_slice(), b"\r\n"].concat();
        for bytes in [b"".as_slice(), short, &unframed] {
            assert_eq!(
                TransactionCategoryRecord::decode(bytes).err(),
                Some(Error::new(
                    "record",
                    0,
                    60,
                    ErrorKind::WidthMismatch {
                        expected: 60,
                        actual: bytes.len(),
                    },
                )),
            );
        }
        Ok(())
    }

    // audit: CVTRA04Y-004
    #[test]
    fn crlf_lines_round_trip_through_framing_and_a_body_without_filler_is_refused() -> TestResult {
        let input = [LINE_1.as_slice(), b"\r\n", LINE_9, b"\r\n"].concat();
        let framed = decode_all::<TransactionCategoryRecord>(&input)?;
        let framings: Vec<LineFraming> = framed.iter().map(|f| f.framing).collect();
        let crlf_60 = LineFraming {
            terminator: Terminator::CrLf,
            body_len: 60,
        };
        assert_eq!(framings, [crlf_60, crlf_60]);
        assert_eq!(encode_all(&framed), input);

        let body = LINE_1.get(..56).ok_or("line 1 is 60 bytes")?;
        assert!(matches!(
            Framed::<TransactionCategoryRecord>::decode(Line::new(body, Terminator::CrLf)),
            Err(Error {
                kind: ErrorKind::ShortBodyNotFiller {
                    expected: 60,
                    actual: 56,
                },
                ..
            })
        ));
        Ok(())
    }
}
