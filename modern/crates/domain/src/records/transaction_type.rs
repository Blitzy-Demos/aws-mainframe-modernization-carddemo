//! `CVTRA03Y` `TRAN-TYPE-RECORD`, 60 bytes: one transaction type of the VSAM data version
//! (`app/cpy/CVTRA03Y.cpy:4-7`; `RECORDSIZE(60 60)` and `KEYS(2 0)` at
//! `app/jcl/TRANTYPE.jcl:40-41`).
//!
//! The fixture is `app/data/ASCII/trantype.txt`: 7 lines of 60 bytes, keys `01` to `07`, lines
//! 1 to 6 ending CRLF and line 7 ending LF (FIXFRM-009, E06). The terminators are line framing,
//! carried by `copybook::framing` around this record and never part of it (PIC-014).

use copybook::{Error, FixedRecord, PicX, RecordReader, RecordWriter};

/// `TRAN-TYPE-RECORD` (`CVTRA03Y.cpy:4-7`): a transaction type code and its description.
///
/// This is the VSAM data version of the transaction types: the `trantype` load replaces table
/// `transaction_types` with it, and only `tranrept` reads it, for the type descriptions
/// CBTRN03C prints (`1500-B-LOOKUP-TRANTYPE`). The Db2 data version of the same codes is
/// [`Db2TransactionType`](super::Db2TransactionType), which COTRTLIC, COTRTUPC and COBTUPDT
/// maintain. The two hold different texts from the start (`Purchase` here against the Db2
/// seed's `PURCHASE`), and the source synchronises them only through TRANEXTR, which is out of
/// scope. So no conversion between the two types is provided: no component copies between the
/// versions, and a maintenance change never reaches a report lookup (E23).
///
/// Every field is `PIC X(n)` and keeps its bytes verbatim, padding included (PIC-001); a byte
/// outside printable ASCII is a typed error naming the field (PIC-002).
///
/// [`FixedRecord::TRAILING_FILLER`] keeps its default of 0: every `trantype.txt` body is the
/// full 60 bytes (FIXFRM-009), and this record's `FILLER` holds `0` characters rather than
/// spaces (E05). A body that lacks it is therefore an anomaly, reported by `copybook::framing`
/// as a typed error (R7), never padded with spaces that no fixture record holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionTypeRecord {
    /// `TRAN-TYPE PIC X(02)`, offset 0, 2 bytes: the type code, the KSDS key `KEYS(2 0)` and
    /// the primary key of `transaction_types` (CVTRA03Y-001).
    pub tran_type: PicX<2>,
    /// `TRAN-TYPE-DESC PIC X(50)`, offset 2, 50 bytes: the description, space padding kept,
    /// stored in `transaction_types.tran_type_desc` (CVTRA03Y-002).
    pub desc: PicX<50>,
    /// `FILLER PIC X(08)`, offset 52, 8 bytes: kept byte for byte, never blanked; every fixture
    /// record holds `00000000` here (CVTRA03Y-003, PIC-012, E05).
    pub filler: PicX<8>,
}

impl FixedRecord for TransactionTypeRecord {
    /// `RECLN = 60` (`CVTRA03Y.cpy:2`), the `RECORDSIZE(60 60)` of the KSDS (CVTRA03Y-003).
    const WIDTH: usize = 60;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        // Struct-literal fields are evaluated in the order written, which is copybook order.
        let record = Self {
            tran_type: r.pic_x("TRAN-TYPE")?,
            desc: r.pic_x("TRAN-TYPE-DESC")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.tran_type)
            .pic_x(&self.desc)
            .pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, PicX};

    use super::TransactionTypeRecord;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Body of `trantype.txt` line 1, without its CRLF: `01`, `Purchase` space-padded to 50
    /// bytes, FILLER `00000000` (FIXFRM-009).
    const LINE_1: &[u8; 60] = b"01Purchase                                          00000000";

    /// Body of `trantype.txt` line 6, without its CRLF: the fixture's `Reversal`, where the Db2
    /// seed has `REVERAL` (E23).
    const LINE_6: &[u8; 60] = b"06Reversal                                          00000000";

    /// Body of `trantype.txt` line 7, the only line ending LF (E06).
    const LINE_7: &[u8; 60] = b"07Adjustment                                        00000000";

    /// The fixture `FILLER` of every record (E05).
    const FIXTURE_FILLER: &[u8; 8] = b"00000000";

    /// `LINE_1` with the byte at `offset` replaced by `byte`.
    fn line_1_with(offset: usize, byte: u8) -> Result<Vec<u8>, Box<dyn StdError>> {
        let mut body = LINE_1.to_vec();
        *body.get_mut(offset).ok_or("offset outside the record")? = byte;
        Ok(body)
    }

    // audit: CVTRA03Y-001
    // audit: CVTRA03Y-002
    // audit: CVTRA03Y-003
    // audit: FIXFRM-009
    #[test]
    fn fixture_bodies_decode_field_by_field_and_re_encode_verbatim() -> TestResult {
        for (body, code, description) in [
            (LINE_1, b"01", "Purchase"),
            (LINE_6, b"06", "Reversal"),
            (LINE_7, b"07", "Adjustment"),
        ] {
            let record = TransactionTypeRecord::decode(body)?;
            let expected = TransactionTypeRecord {
                tran_type: PicX::from_array(*code),
                desc: PicX::from_text(description)?,
                filler: PicX::from_array(*FIXTURE_FILLER),
            };
            assert_eq!(record, expected);
            // The description keeps its 50 bytes, trailing spaces included, at offset 2.
            assert_eq!(Some(record.desc.as_bytes().as_slice()), body.get(2..52));
            assert_eq!(record.desc.to_text(), description);
            assert_eq!(record.encode(), body);
            assert_eq!(expected.encode(), body);
        }
        Ok(())
    }

    // audit: PIC-012
    #[test]
    fn filler_bytes_survive_whatever_they_hold() -> TestResult {
        let record = TransactionTypeRecord::decode(LINE_1)?;
        assert_eq!(record.filler.as_bytes(), FIXTURE_FILLER);
        let changed = TransactionTypeRecord {
            filler: PicX::from_array(*b"0 1~0 1~"),
            ..record
        };
        let bytes = changed.encode();
        assert_eq!(bytes.get(52..), Some(b"0 1~0 1~".as_slice()));
        assert_eq!(TransactionTypeRecord::decode(&bytes)?, changed);
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn malformed_slices_are_typed_errors_naming_the_field() -> TestResult {
        let mismatch = |actual| ErrorKind::WidthMismatch {
            expected: 60,
            actual,
        };
        let cases = [
            (Vec::new(), Error::new("record", 0, 60, mismatch(0))),
            (
                LINE_1.get(..59).ok_or("short")?.to_vec(),
                Error::new("record", 0, 60, mismatch(59)),
            ),
            (
                LINE_1.get(..52).ok_or("short")?.to_vec(),
                Error::new("record", 0, 60, mismatch(52)),
            ),
            (
                [LINE_1.as_slice(), b"\r".as_slice()].concat(),
                Error::new("record", 0, 60, mismatch(61)),
            ),
            (
                line_1_with(1, 0x00)?,
                Error::new("TRAN-TYPE", 0, 2, ErrorKind::NonPrintable),
            ),
            (
                line_1_with(10, b'\r')?,
                Error::new("TRAN-TYPE-DESC", 2, 50, ErrorKind::NonPrintable),
            ),
            (
                line_1_with(59, 0x7F)?,
                Error::new("FILLER", 52, 8, ErrorKind::NonPrintable),
            ),
        ];
        for (bytes, error) in cases {
            assert_eq!(TransactionTypeRecord::decode(&bytes), Err(error));
        }
        Ok(())
    }

    #[test]
    fn encode_writes_exactly_the_record_width() {
        let blank = TransactionTypeRecord {
            tran_type: PicX::spaces(),
            desc: PicX::spaces(),
            filler: PicX::spaces(),
        };
        assert_eq!(blank.encode(), vec![b' '; TransactionTypeRecord::WIDTH]);
    }
}
