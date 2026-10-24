//! Codec B for table `transaction_types` (migration 0001): one row per `CVTRA03Y`
//! `TRAN-TYPE-RECORD`, the VSAM data version of the transaction types (CVTRA03Y-001 to
//! CVTRA03Y-003; the `trantype` load, FIXLOAD-008).
//!
//! Every field of the 60-byte record is `PIC X(n)` and every column `CHAR(n)`, so the row holds
//! the record as three texts of 2, 50 and 8 ASCII bytes, each kept byte for byte (PIC-001,
//! PIC-012):
//!
//! | Field | Column | What is kept |
//! |---|---|---|
//! | `TRAN-TYPE PIC X(02)`, offset 0 | `tran_type CHAR(2)`, the primary key, as the KSDS `KEYS(2 0)` | the type code (CVTRA03Y-001) |
//! | `TRAN-TYPE-DESC PIC X(50)`, offset 2 | `tran_type_desc CHAR(50)` | the description with its trailing spaces, in the fixture's case: `Purchase`, never the Db2 seed's `PURCHASE` (CVTRA03Y-002, E23) |
//! | `FILLER PIC X(08)`, offset 52 | `filler CHAR(8)` | the `FILLER` bytes, which hold `00000000` rather than spaces in every fixture record (CVTRA03Y-003, E05) |
//!
//! The terminators of `trantype.txt`, CRLF on lines 1 to 6 and LF on line 7, are line framing:
//! codec A carries them around the record and no column stores them (PIC-014, E06).
//!
//! # Data version
//!
//! This table is the VSAM data version, loaded from `trantype.txt` and read only by `tranrept`
//! for the descriptions CBTRN03C prints. The Db2 data version is `db2.transaction_type`
//! (migration 0006), whose record is [`Db2TransactionType`](domain::records::Db2TransactionType)
//! and whose row is mapped in `db2_transaction_types`. The source synchronises the two only
//! through TRANEXTR, which is out of scope, so no conversion between this row or its record and
//! the Db2 types exists here, and none may be added: a maintenance change through COTRTUPC or
//! COBTUPDT must never reach a report lookup (E23).

use domain::records::TransactionTypeRecord;

use super::{pic_x_to_string, string_to_pic_x};
use crate::error::PersistenceError;

/// Column `tran_type CHAR(2)`, the primary key (CVTRA03Y-001).
const TRAN_TYPE: &str = "tran_type";
/// Column `tran_type_desc CHAR(50)` (CVTRA03Y-002).
const TRAN_TYPE_DESC: &str = "tran_type_desc";
/// Column `filler CHAR(8)` (CVTRA03Y-003).
const FILLER: &str = "filler";

/// One row of `transaction_types`: a [`TransactionTypeRecord`] as its three `CHAR(n)` columns.
///
/// Each field is named as its column and holds exactly the bytes of its `PIC X(n)` field,
/// padding and `FILLER` contents included (PIC-001, PIC-012, E05). `Debug` is left underived,
/// as on every row struct of codec B.
#[derive(Clone, PartialEq, Eq)]
pub struct TransactionTypeRow {
    /// `tran_type CHAR(2)`: `TRAN-TYPE PIC X(02)`, the primary key (CVTRA03Y-001).
    pub(crate) tran_type: String,
    /// `tran_type_desc CHAR(50)`: `TRAN-TYPE-DESC PIC X(50)`, trailing spaces kept and the
    /// fixture's mixed case unchanged (CVTRA03Y-002, E23).
    pub(crate) tran_type_desc: String,
    /// `filler CHAR(8)`: `FILLER PIC X(08)`, byte for byte, never blanked (CVTRA03Y-003, PIC-012,
    /// E05).
    pub(crate) filler: String,
}

impl TransactionTypeRow {
    /// The row that stores `record`, every byte verbatim: the `FILLER` bytes as they are (E05)
    /// and the description with its trailing spaces (E23).
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for the first field,
    /// in copybook order, that holds a NUL or non-ASCII byte, which no `CHAR(n)` column could
    /// store byte for byte (PIC-002). Codec A rejects such bytes on decode, so only a record
    /// built by hand can carry one.
    pub fn from_record(record: &TransactionTypeRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            tran_type: pic_x_to_string(TRAN_TYPE, &record.tran_type)?,
            tran_type_desc: pic_x_to_string(TRAN_TYPE_DESC, &record.desc)?,
            filler: pic_x_to_string(FILLER, &record.filler)?,
        })
    }

    /// The record this row stores, byte for byte: for a row built by
    /// [`from_record`](Self::from_record), a record equal to the one written, which codec A
    /// re-encodes to the original fixture bytes (binding d9).
    ///
    /// # Errors
    ///
    /// For the first column, in copybook order, whose text does not fit its picture:
    ///
    /// - [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or
    ///   non-ASCII character (PIC-002);
    /// - [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) when the text is
    ///   not exactly 2, 50 or 8 bytes long. A value is never padded or cut to fit (R7).
    pub fn into_record(self) -> Result<TransactionTypeRecord, PersistenceError> {
        Ok(TransactionTypeRecord {
            tran_type: string_to_pic_x(TRAN_TYPE, &self.tran_type)?,
            desc: string_to_pic_x(TRAN_TYPE_DESC, &self.tran_type_desc)?,
            filler: string_to_pic_x(FILLER, &self.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use copybook::framing::{decode_all, encode_all};
    use copybook::{FixedRecord, Framed, PicX, Terminator};
    use domain::records::TransactionTypeRecord;

    use super::TransactionTypeRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Body of `trantype.txt` line 1, without its CRLF: `01`, `Purchase` space-padded to 50
    /// bytes, `FILLER` `00000000` (FIXFRM-009).
    const LINE_1: &[u8; 60] = b"01Purchase                                          00000000";

    /// A record no fixture holds: a 50-byte description without padding and a `FILLER` of
    /// mixed printable bytes, spaces among them.
    const UNPADDED: &[u8; 60] = b"ZZ~Description of fifty bytes; none of them padding}0 1~0 1~";

    /// The fixture as a `transaction_types` load reads it, in place.
    fn trantype_fixture() -> Result<Vec<u8>, Box<dyn StdError>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/trantype.txt");
        Ok(std::fs::read(path)?)
    }

    /// A row as a `SELECT` of the three columns returns it.
    fn row(tran_type: &str, tran_type_desc: &str, filler: &str) -> TransactionTypeRow {
        TransactionTypeRow {
            tran_type: tran_type.to_owned(),
            tran_type_desc: tran_type_desc.to_owned(),
            filler: filler.to_owned(),
        }
    }

    fn decode_error(column: &'static str, kind: DecodeKind) -> PersistenceError {
        PersistenceError::decode(column, kind)
    }

    // audit: CVTRA03Y-001
    // audit: CVTRA03Y-002
    // audit: CVTRA03Y-003
    // audit: PIC-012
    #[test]
    fn records_with_non_space_filler_convert_byte_for_byte_both_ways() -> TestResult {
        for (body, code, filler) in [(LINE_1, "01", "00000000"), (UNPADDED, "ZZ", "0 1~0 1~")] {
            let record = TransactionTypeRecord::decode(body)?;
            let row = TransactionTypeRow::from_record(&record)?;
            assert_eq!(row.tran_type, code);
            assert_eq!(
                Some(row.tran_type_desc.as_bytes()),
                body.get(2..52),
                "the description keeps all 50 bytes"
            );
            assert_eq!(row.filler, filler);
            assert!(TransactionTypeRow::from_record(&record)? == row);
            let back = row.into_record()?;
            assert_eq!(back, record);
            assert_eq!(back.encode(), body);
        }
        Ok(())
    }

    /// E23: the VSAM data version keeps the fixture's `Purchase` and `Reversal`, padded to 50
    /// bytes, where the Db2 seed holds `PURCHASE` and `REVERAL`.
    // audit: FIXFRM-009
    // audit: PIC-014
    #[test]
    fn every_trantype_line_returns_to_its_own_bytes() -> TestResult {
        let input = trantype_fixture()?;
        let lines = decode_all::<TransactionTypeRecord>(&input)?;
        let terminators: Vec<Terminator> =
            lines.iter().map(|line| line.framing.terminator).collect();
        assert_eq!(
            terminators,
            [[Terminator::CrLf; 6].as_slice(), &[Terminator::Lf]].concat(),
            "E06"
        );
        let expected = [
            ("01", "Purchase"),
            ("02", "Payment"),
            ("03", "Credit"),
            ("04", "Authorization"),
            ("05", "Refund"),
            ("06", "Reversal"),
            ("07", "Adjustment"),
        ];
        assert_eq!(lines.len(), expected.len());
        let mut restored = Vec::with_capacity(lines.len());
        for (line, (code, description)) in lines.into_iter().zip(expected) {
            let row = TransactionTypeRow::from_record(&line.record)?;
            assert_eq!(row.tran_type, code);
            assert_eq!(row.tran_type_desc, format!("{description:<50}"));
            assert_eq!(row.filler, "00000000", "E05");
            restored.push(Framed {
                record: row.into_record()?,
                framing: line.framing,
            });
        }
        assert_eq!(encode_all(&restored), input);
        Ok(())
    }

    #[test]
    fn stored_text_of_another_width_is_rejected_naming_its_column() {
        let padded = format!("{:<50}", "Purchase");
        let short = format!("{:<49}", "Purchase");
        let long = format!("{:<51}", "Purchase");
        let width = |expected, actual| DecodeKind::WidthMismatch { expected, actual };
        let cases = [
            (
                row("01", &short, "00000000"),
                "tran_type_desc",
                width(50, 49),
            ),
            (
                row("01", &long, "00000000"),
                "tran_type_desc",
                width(50, 51),
            ),
            (row("1", &padded, "00000000"), "tran_type", width(2, 1)),
            (row("011", &padded, "00000000"), "tran_type", width(2, 3)),
            (row("01", &padded, "0000000"), "filler", width(8, 7)),
            (row("01", &padded, ""), "filler", width(8, 0)),
            (row("", "", ""), "tran_type", width(2, 0)),
        ];
        for (stored, column, kind) in cases {
            assert_eq!(stored.into_record(), Err(decode_error(column, kind)));
        }
    }

    // audit: PIC-002
    #[test]
    fn nul_and_non_ascii_bytes_are_rejected_both_ways_naming_their_column() -> TestResult {
        let record = TransactionTypeRecord::decode(LINE_1)?;
        let invalid = |column| Err(decode_error(column, DecodeKind::InvalidByte));
        let written = [
            (
                TransactionTypeRecord {
                    filler: PicX::from_array(*b"0000000\x80"),
                    ..record.clone()
                },
                "filler",
            ),
            (
                TransactionTypeRecord {
                    tran_type: PicX::from_array(*b"0\0"),
                    ..record.clone()
                },
                "tran_type",
            ),
            (
                TransactionTypeRecord {
                    desc: PicX::from_array([0xC3; 50]),
                    filler: PicX::from_array([0x80; 8]),
                    ..record
                },
                "tran_type_desc",
            ),
        ];
        for (value, column) in written {
            assert_eq!(
                TransactionTypeRow::from_record(&value).map(drop),
                invalid(column)
            );
        }
        let padded = format!("{:<50}", "Purchase");
        let accented = format!("{:<49}", "Purchas\u{e9}");
        let stored = [
            (row("01", &padded, "0000000\u{80}"), "filler"),
            (row("0\0", &padded, "00000000"), "tran_type"),
            (row("01", &accented, "00000000"), "tran_type_desc"),
        ];
        for (value, column) in stored {
            assert_eq!(value.into_record().map(drop), invalid(column));
        }
        Ok(())
    }
}
