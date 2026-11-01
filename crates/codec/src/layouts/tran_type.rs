//! `CVTRA03Y` `TRAN-TYPE-RECORD`: the 60-byte transaction type record of the
//! VSAM processing copy `TRANTYPE` (`RECLN = 60`) [app/cpy/CVTRA03Y.cpy:L1-7].
//!
//! `TRANTYPE` is the KSDS `KEYS(2 0)`, `RECORDSIZE(60 60)`
//! [app/jcl/TRANTYPE.jcl:L40-41] that only `CBTRN03C` reads
//! [app/cbl/CBTRN03C.cbl:L39-43], for CR00 and TRANREPT. `batch seed` decodes
//! `app/data/ASCII/trantype.txt` with [`TranTypeLayout`] into the table
//! `trantype_file` (P-CVTRA03Y-004).
//!
//! It is not the Db2 master `CARDDEMO.TRANSACTION_TYPE` (the domain
//! `TranTypeMaster`, table `transaction_types`), which this codec never reads
//! or writes. The two hold different text until TRANEXTR copies the masters
//! over the processing copies: the fixture stores `Purchase` where the Db2
//! load stores `PURCHASE` (P-CVTRA03Y-005). Descriptions therefore keep their
//! stored bytes and case; nothing here converts between the two versions.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `TRAN-TYPE` | `X(02)` | 0 | 2 | P-CVTRA03Y-001 | `TranTypeCode` |
//! | `TRAN-TYPE-DESC` | `X(50)` | 2 | 50 | P-CVTRA03Y-002 | `Alnum<50>` |
//! | `FILLER` | `X(08)` | 52 | 8 | P-CVTRA03Y-003 | `Alnum<8>`, raw |
//!
//! # Fixture bytes
//!
//! - The `FILLER` of every fixture record holds `00000000`, not spaces
//!   (P-CVTRA03Y-003), so it is kept as its stored bytes and written back as
//!   them.
//! - Records 1–6 of `trantype.txt` end CR LF and record 7 ends LF
//!   (P-CVTRA03Y-006, E-04). The line end is not part of the record:
//!   [`crate::framing`] keeps each record's terminator, and this layout sees
//!   the 60-byte body only.
//! - The EBCDIC counterpart `AWS.M2.CARDDEMO.TRANTYPE.PS` holds the same seven
//!   records as 60-byte CP037 blocks.

use carddemo_domain::entities::TranTypeRecord;
use carddemo_domain::numeric::{Alnum, TranTypeCode};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec};

/// `CVTRA03Y` `TRAN-TYPE PIC X(02)`, the `TRANTYPE` key — P-CVTRA03Y-001
/// [app/cpy/CVTRA03Y.cpy:L5].
pub const TRAN_TYPE: FieldSpec = FieldSpec::new("TRAN-TYPE", 0, 2, FieldKind::Alnum);

/// `CVTRA03Y` `TRAN-TYPE-DESC PIC X(50)` — P-CVTRA03Y-002
/// [app/cpy/CVTRA03Y.cpy:L6].
pub const TRAN_TYPE_DESC: FieldSpec = FieldSpec::new("TRAN-TYPE-DESC", 2, 50, FieldKind::Alnum);

/// `CVTRA03Y` `FILLER PIC X(08)`, kept raw — P-CVTRA03Y-003
/// [app/cpy/CVTRA03Y.cpy:L7].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 52, 8, FieldKind::Filler);

/// The `CVTRA03Y` `TRAN-TYPE-RECORD` layout, 60 bytes
/// [app/cpy/CVTRA03Y.cpy:L2-7].
pub struct TranTypeLayout;

/// One decoded `TRANTYPE` record: the domain processing-copy record and the
/// `FILLER` bytes the domain record does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeFileRecord {
    /// `TRAN-TYPE` and `TRAN-TYPE-DESC`, as stored — P-CVTRA03Y-001 and
    /// P-CVTRA03Y-002 [app/cpy/CVTRA03Y.cpy:L5-6].
    pub tran_type: TranTypeRecord,
    /// `FILLER PIC X(08)`, its stored bytes — P-CVTRA03Y-003
    /// [app/cpy/CVTRA03Y.cpy:L7].
    pub filler: Alnum<8>,
}

impl Layout for TranTypeLayout {
    const LEN: usize = 60;
    const NAME: &'static str = "CVTRA03Y";
    const FIELDS: &'static [FieldSpec] = &[TRAN_TYPE, TRAN_TYPE_DESC, FILLER];
    type Record = TranTypeFileRecord;

    /// Reads `TRAN-TYPE`, `TRAN-TYPE-DESC` and `FILLER` (P-CVTRA03Y-001 to
    /// P-CVTRA03Y-003) [app/cpy/CVTRA03Y.cpy:L5-7], each as its stored bytes.
    ///
    /// No domain constructor can reject a decoded value: a type code is any two
    /// stored bytes, as `TranTypeCode::from_alnum` takes them, and the
    /// description and filler are plain `X(n)` items.
    ///
    /// # Errors
    ///
    /// [`CodecErrorKind::Length`] unless `bytes` is exactly 60 bytes long.
    fn decode(bytes: &[u8], charset: Charset) -> Result<Self::Record, CodecError> {
        if bytes.len() != Self::LEN {
            return Err(CodecError::new(
                Self::NAME,
                CodecErrorKind::Length {
                    expected: Self::LEN,
                    actual: bytes.len(),
                },
            ));
        }
        let rec = charset.to_internal(bytes);
        Ok(TranTypeFileRecord {
            tran_type: TranTypeRecord {
                tran_type: TranTypeCode::from_alnum(field::alnum(Self::NAME, &rec, &TRAN_TYPE)?),
                tran_type_desc: field::alnum(Self::NAME, &rec, &TRAN_TYPE_DESC)?,
            },
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
        })
    }

    /// Writes `TRAN-TYPE`, `TRAN-TYPE-DESC` and `FILLER` (P-CVTRA03Y-001 to
    /// P-CVTRA03Y-003) [app/cpy/CVTRA03Y.cpy:L5-7] from the bytes the record
    /// holds, then translates the 60 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let mut out = vec![b' '; Self::LEN];
        field::put_alnum(&mut out, &TRAN_TYPE, record.tran_type.tran_type.as_bytes());
        field::put_alnum(
            &mut out,
            &TRAN_TYPE_DESC,
            record.tran_type.tran_type_desc.as_bytes(),
        );
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::TranTypeRecord;
    use carddemo_domain::numeric::{Alnum, TranTypeCode};

    use super::{FILLER, TRAN_TYPE, TRAN_TYPE_DESC, TranTypeFileRecord, TranTypeLayout};
    use crate::{Charset, CodecError, CodecErrorKind, FieldKind, Layout, field};

    /// The ASCII `TRANTYPE` fixture: seven records, 1–6 ending CR LF and 7
    /// ending LF.
    const TRANTYPE_ASCII: &[u8] = include_bytes!("../../../../app/data/ASCII/trantype.txt");

    /// The EBCDIC `TRANTYPE` file: the same seven records as 60-byte CP037
    /// blocks.
    const TRANTYPE_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.TRANTYPE.PS");

    /// The code and description of each fixture record, in file order
    /// [app/data/ASCII/trantype.txt:L1-7].
    const FIXTURE: [(&str, &str); 7] = [
        ("01", "Purchase"),
        ("02", "Payment"),
        ("03", "Credit"),
        ("04", "Authorization"),
        ("05", "Refund"),
        ("06", "Reversal"),
        ("07", "Adjustment"),
    ];

    /// The stored lines of the ASCII fixture, each split into its body and its
    /// line end (`CR LF`, `LF`, or nothing at the end of the file).
    fn ascii_lines() -> Vec<(&'static [u8], &'static [u8])> {
        TRANTYPE_ASCII
            .split_inclusive(|&b| b == b'\n')
            .map(|line| {
                let end = [&b"\r\n"[..], b"\n"]
                    .into_iter()
                    .find(|end| line.ends_with(end))
                    .unwrap_or_default();
                line.split_at(line.len() - end.len())
            })
            .collect()
    }

    #[test]
    fn fields_tile_the_60_byte_record_at_the_audited_offsets() {
        assert_eq!(
            (TranTypeLayout::NAME, TranTypeLayout::LEN),
            ("CVTRA03Y", 60)
        );
        assert!(field::fields_are_contiguous(TranTypeLayout::FIELDS, 60));
        assert_eq!(TranTypeLayout::FIELDS, [TRAN_TYPE, TRAN_TYPE_DESC, FILLER]);
        let specs: Vec<_> = TranTypeLayout::FIELDS
            .iter()
            .map(|spec| (spec.name, spec.offset, spec.len, spec.kind))
            .collect();
        assert_eq!(
            specs,
            [
                ("TRAN-TYPE", 0, 2, FieldKind::Alnum),
                ("TRAN-TYPE-DESC", 2, 50, FieldKind::Alnum),
                ("FILLER", 52, 8, FieldKind::Filler),
            ]
        );
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_60_bytes() {
        for len in [0, 59, 61] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    TranTypeLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CVTRA03Y",
                        CodecErrorKind::Length {
                            expected: 60,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn fixture_record_1_decodes_to_its_fields_and_re_encodes_to_its_body() -> Result<(), CodecError>
    {
        let lines = ascii_lines();
        let (body, end) = lines[0];
        assert_eq!(end, b"\r\n");
        let expected = [
            &b"01"[..],
            format!("{:<50}", "Purchase").as_bytes(),
            b"00000000",
        ]
        .concat();
        assert_eq!(body, expected.as_slice());

        let record = TranTypeLayout::decode(body, Charset::Ascii)?;
        assert_eq!(record.tran_type.tran_type, TranTypeCode::move_from("01"));
        assert_eq!(record.tran_type.tran_type_desc.trimmed(), "Purchase");
        assert_eq!(
            record.tran_type.tran_type_desc,
            Alnum::move_from("Purchase"),
            "the stored case is kept, not the Db2 master's PURCHASE"
        );
        assert_eq!(record.filler.as_bytes(), b"00000000");
        assert_eq!(TranTypeLayout::encode(&record, Charset::Ascii), body);
        Ok(())
    }

    #[test]
    fn every_fixture_record_round_trips_and_is_its_ebcdic_block() -> Result<(), CodecError> {
        let lines = ascii_lines();
        let blocks: Vec<&[u8]> = TRANTYPE_EBCDIC.chunks(TranTypeLayout::LEN).collect();
        assert_eq!((lines.len(), blocks.len()), (FIXTURE.len(), FIXTURE.len()));
        let records = lines.into_iter().zip(blocks).zip(FIXTURE);
        for (n, (((body, end), block), (code, desc))) in (1..).zip(records) {
            let terminator: &[u8] = if n < 7 { b"\r\n" } else { b"\n" };
            assert_eq!(end, terminator, "record {n}");

            let record = TranTypeLayout::decode(body, Charset::Ascii)?;
            assert_eq!(record.tran_type.tran_type.trimmed(), code, "record {n}");
            assert_eq!(
                record.tran_type.tran_type_desc.trimmed(),
                desc,
                "record {n}"
            );
            assert_eq!(record.filler.as_bytes(), b"00000000", "record {n}");
            assert_eq!(
                TranTypeLayout::encode(&record, Charset::Ascii),
                body,
                "record {n}"
            );

            assert_eq!(
                TranTypeLayout::decode(block, Charset::Ebcdic)?,
                record,
                "record {n}"
            );
            assert_eq!(
                TranTypeLayout::encode(&record, Charset::Ebcdic),
                block,
                "record {n}"
            );
        }
        Ok(())
    }

    #[test]
    fn hand_built_records_round_trip_in_both_charsets() -> Result<(), CodecError> {
        let filler = [0x00, 0xFF, b'0', b' ', b'9', b'z', 0x0D, 0x0A];
        let records = [
            TranTypeFileRecord {
                tran_type: TranTypeRecord {
                    tran_type: TranTypeCode::move_from("A"),
                    tran_type_desc: Alnum::move_from_bytes(b"Caf\xE9 reversal, Mixed CASE"),
                },
                filler: Alnum::from_array(filler),
            },
            TranTypeFileRecord {
                tran_type: TranTypeRecord {
                    tran_type: TranTypeCode::move_from("99"),
                    tran_type_desc: Alnum::from_array([b'X'; 50]),
                },
                filler: Alnum::spaces(),
            },
        ];
        for record in &records {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                let bytes = TranTypeLayout::encode(record, charset);
                assert_eq!(bytes.len(), 60, "{charset:?}");
                assert_eq!(
                    TranTypeLayout::decode(&bytes, charset)?,
                    *record,
                    "{charset:?}"
                );
            }
        }
        let ascii = TranTypeLayout::encode(&records[0], Charset::Ascii);
        assert_eq!(ascii.get(..2), Some(&b"A "[..]));
        assert_eq!(ascii.get(2..27), Some(&b"Caf\xE9 reversal, Mixed CASE"[..]));
        assert_eq!(ascii.get(27..52), Some(&[b' '; 25][..]));
        assert_eq!(ascii.get(52..), Some(&filler[..]));
        Ok(())
    }
}
