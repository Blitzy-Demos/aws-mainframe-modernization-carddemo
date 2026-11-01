//! `CVTRA02Y` `DIS-GROUP-RECORD`: the 50-byte `DISCGRP` disclosure group
//! record, the interest rate of one account group, transaction type and
//! category [app/cpy/CVTRA02Y.cpy:L2-10].
//!
//! The `DISCGRP` job loads `app/data/ASCII/discgrp.txt` (51 records, each
//! ending LF); the EBCDIC counterpart `app/data/EBCDIC/AWS.M2.CARDDEMO.DISCGRP.PS`
//! holds the same 51 records as fixed 50-byte CP037 blocks. `batch seed`
//! decodes each record with [`DisclosureGroupLayout`] into the domain
//! [`DisclosureGroup`] that `CBACT04C` `1200-GET-INTEREST-RATE` reads.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `DIS-ACCT-GROUP-ID` | `X(10)` | 0 | 10 | P-CVTRA02Y-001 | `AccountGroupId` |
//! | `DIS-TRAN-TYPE-CD` | `X(02)` | 10 | 2 | P-CVTRA02Y-002 | `TranTypeCode` |
//! | `DIS-TRAN-CAT-CD` | `9(04)` | 12 | 4 | P-CVTRA02Y-003 | `TranCategoryCode` |
//! | `DIS-INT-RATE` | `S9(04)V99` | 16 | 6 | P-CVTRA02Y-004 | `InterestRate`, in hundredths |
//! | `FILLER` | `X(28)` | 22 | 28 | P-CVTRA02Y-005 | `Alnum<28>`, raw |
//!
//! The three key fields form the domain `DisclosureGroupKey` (`05
//! DIS-GROUP-KEY` [app/cpy/CVTRA02Y.cpy:L5]). P-CVTRA02Y-001 labels the group
//! ID `Alnum<10>`; the domain key types it `AccountGroupId`, which holds
//! exactly those 10 bytes. Group IDs such as `A000000000` and `DEFAULT   ` are
//! plain text and are kept as stored, trailing spaces included.
//!
//! # FILLER
//!
//! Every fixture record holds 28 `0` characters in FILLER, not spaces
//! [app/data/ASCII/discgrp.txt:L1], so [`DisclosureGroupRecord::filler`] keeps
//! the stored bytes and the encoder writes them back.
//!
//! # Record 34 (E-05)
//!
//! Record 34 (`DEFAULT   `, `07`, `0001`) holds `DIS-INT-RATE` `00000{` (0.00)
//! in ASCII [app/data/ASCII/discgrp.txt:L34] and `00150{` in EBCDIC. Each
//! corpus is decoded as its own bytes, never reconciled. The `S9(04)V99`
//! clause puts the implied point before the last two of the six digits, so
//! the EBCDIC value is 15.00 (1 500 hundredths), the same rate as the `00150{`
//! of record 1. Where P-CVTRA02Y-007 or E-05 give that EBCDIC rate as 1.50,
//! the bytes do not spell it; the decoder follows the PIC clause ("PIC clauses
//! are authoritative").

use carddemo_domain::entities::{DisclosureGroup, DisclosureGroupKey};
use carddemo_domain::numeric::{
    AccountGroupId, Alnum, InterestRate, NumericError, TranCategoryCode, TranTypeCode,
};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec, PreservedFields};

/// `CVTRA02Y` `DIS-ACCT-GROUP-ID PIC X(10)`, the account group of the key —
/// P-CVTRA02Y-001 [app/cpy/CVTRA02Y.cpy:L6].
pub const DIS_ACCT_GROUP_ID: FieldSpec =
    FieldSpec::new("DIS-ACCT-GROUP-ID", 0, 10, FieldKind::Alnum);

/// `CVTRA02Y` `DIS-TRAN-TYPE-CD PIC X(02)` — P-CVTRA02Y-002
/// [app/cpy/CVTRA02Y.cpy:L7].
pub const DIS_TRAN_TYPE_CD: FieldSpec = FieldSpec::new("DIS-TRAN-TYPE-CD", 10, 2, FieldKind::Alnum);

/// `CVTRA02Y` `DIS-TRAN-CAT-CD PIC 9(04)` — P-CVTRA02Y-003
/// [app/cpy/CVTRA02Y.cpy:L8].
pub const DIS_TRAN_CAT_CD: FieldSpec =
    FieldSpec::new("DIS-TRAN-CAT-CD", 12, 4, FieldKind::Unsigned);

/// `CVTRA02Y` `DIS-INT-RATE PIC S9(04)V99`, the annual interest rate with a
/// trailing overpunch sign — P-CVTRA02Y-004 [app/cpy/CVTRA02Y.cpy:L9].
pub const DIS_INT_RATE: FieldSpec =
    FieldSpec::new("DIS-INT-RATE", 16, 6, FieldKind::Signed { scale: 2 });

/// `CVTRA02Y` `FILLER PIC X(28)`, kept raw — P-CVTRA02Y-005
/// [app/cpy/CVTRA02Y.cpy:L10].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 22, 28, FieldKind::Filler);

/// The `CVTRA02Y` `DIS-GROUP-RECORD` layout, 50 bytes
/// [app/cpy/CVTRA02Y.cpy:L2-10].
pub struct DisclosureGroupLayout;

/// One decoded `DISCGRP` record: the domain disclosure group plus the bytes
/// the domain type does not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureGroupRecord {
    /// `DIS-GROUP-KEY` and `DIS-INT-RATE` — P-CVTRA02Y-001 to P-CVTRA02Y-004
    /// [app/cpy/CVTRA02Y.cpy:L5-9].
    pub group: DisclosureGroup,
    /// `FILLER PIC X(28)`, its stored bytes — P-CVTRA02Y-005
    /// [app/cpy/CVTRA02Y.cpy:L10].
    pub filler: Alnum<28>,
    /// The stored bytes of a negative-zero `DIS-INT-RATE` (`00000}`), which
    /// [`InterestRate`] holds as plain zero; empty for every fixture record.
    pub preserved: PreservedFields,
}

impl Layout for DisclosureGroupLayout {
    const LEN: usize = 50;
    const NAME: &'static str = "CVTRA02Y";
    const FIELDS: &'static [FieldSpec] = &[
        DIS_ACCT_GROUP_ID,
        DIS_TRAN_TYPE_CD,
        DIS_TRAN_CAT_CD,
        DIS_INT_RATE,
        FILLER,
    ];
    type Record = DisclosureGroupRecord;

    /// Reads `DIS-ACCT-GROUP-ID` through `FILLER` (P-CVTRA02Y-001 to
    /// P-CVTRA02Y-005) [app/cpy/CVTRA02Y.cpy:L6-10]: the text fields as
    /// stored, the category as its four digits, the rate as hundredths.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 50 bytes long.
    /// - The field errors of [`field::unsigned`] and [`field::signed`] at
    ///   `DIS-TRAN-CAT-CD` and `DIS-INT-RATE`: a non-digit byte, a plain digit
    ///   or an invalid byte in the rate's sign position.
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
        let mut preserved = PreservedFields::new();
        let key = DisclosureGroupKey {
            dis_acct_group_id: AccountGroupId::from_alnum(field::alnum(
                Self::NAME,
                &rec,
                &DIS_ACCT_GROUP_ID,
            )?),
            dis_tran_type_cd: TranTypeCode::from_alnum(field::alnum(
                Self::NAME,
                &rec,
                &DIS_TRAN_TYPE_CD,
            )?),
            dis_tran_cat_cd: read_category(&rec)?,
        };
        let dis_int_rate = read_rate(&rec, &mut preserved)?;
        Ok(DisclosureGroupRecord {
            group: DisclosureGroup { key, dis_int_rate },
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
            preserved,
        })
    }

    /// Writes `DIS-ACCT-GROUP-ID` through `FILLER` (P-CVTRA02Y-001 to
    /// P-CVTRA02Y-005) [app/cpy/CVTRA02Y.cpy:L6-10]: the category zero-padded
    /// to four digits, the rate in hundredths with its overpunch sign (a
    /// preserved negative zero restored while the rate is still zero), FILLER
    /// from the stored bytes; then translates the 50 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let key = &record.group.key;
        let mut out = vec![b' '; Self::LEN];
        field::put_alnum(
            &mut out,
            &DIS_ACCT_GROUP_ID,
            key.dis_acct_group_id.as_bytes(),
        );
        field::put_alnum(&mut out, &DIS_TRAN_TYPE_CD, key.dis_tran_type_cd.as_bytes());
        field::put_unsigned(
            &mut out,
            &DIS_TRAN_CAT_CD,
            u64::from(key.dis_tran_cat_cd.get()),
        );
        field::put_signed_preserving(
            &mut out,
            &DIS_INT_RATE,
            i64::from(record.group.dis_int_rate.hundredths()),
            &record.preserved,
        );
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// `DIS-TRAN-CAT-CD PIC 9(04)` of the internal record `rec` — P-CVTRA02Y-003
/// [app/cpy/CVTRA02Y.cpy:L8].
///
/// Four digits always fit [`TranCategoryCode`]; a rejection is still reported
/// as [`CodecErrorKind::Value`] at the field rather than assumed away.
fn read_category(rec: &[u8]) -> Result<TranCategoryCode, CodecError> {
    let digits = field::unsigned(DisclosureGroupLayout::NAME, rec, &DIS_TRAN_CAT_CD)?;
    u16::try_from(digits)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(TranCategoryCode::new)
        .map_err(|error| rejected(&DIS_TRAN_CAT_CD, error))
}

/// `DIS-INT-RATE PIC S9(04)V99` of the internal record `rec`, as hundredths —
/// P-CVTRA02Y-004 [app/cpy/CVTRA02Y.cpy:L9]. A negative zero is recorded in
/// `preserved` so that the encoder writes its stored bytes back.
///
/// Six digits always fit [`InterestRate`]; a rejection is still reported as
/// [`CodecErrorKind::Value`] at the field rather than assumed away.
fn read_rate(rec: &[u8], preserved: &mut PreservedFields) -> Result<InterestRate, CodecError> {
    let stored =
        field::signed_preserving(DisclosureGroupLayout::NAME, rec, &DIS_INT_RATE, preserved)?;
    stored
        .value()
        .and_then(|hundredths| i32::try_from(hundredths).map_err(|_| NumericError::OutOfRange))
        .and_then(InterestRate::from_hundredths)
        .map_err(|error| rejected(&DIS_INT_RATE, error))
}

/// The error for the field `spec` whose decoded value the domain constructor
/// rejected with `error`.
fn rejected(spec: &FieldSpec, error: NumericError) -> CodecError {
    CodecError::new(
        DisclosureGroupLayout::NAME,
        CodecErrorKind::Value(error.to_string()),
    )
    .at_field(spec.name, spec.offset)
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::{DisclosureGroup, DisclosureGroupKey};
    use carddemo_domain::numeric::{
        AccountGroupId, Alnum, InterestRate, TranCategoryCode, TranTypeCode,
    };

    use super::{
        DIS_INT_RATE, DIS_TRAN_CAT_CD, DisclosureGroupLayout, DisclosureGroupRecord, FILLER,
    };
    use crate::{Charset, CodecError, CodecErrorKind, Layout, PreservedFields, field};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The ASCII `DISCGRP` fixture: 51 records of 50 bytes, each ending LF.
    const DISCGRP_ASCII: &str = include_str!("../../../../app/data/ASCII/discgrp.txt");

    /// The EBCDIC `DISCGRP` fixture: the same 51 records as 50-byte CP037
    /// blocks with no terminators.
    const DISCGRP_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.DISCGRP.PS");

    /// The number of records in each `DISCGRP` fixture file.
    const RECORDS: usize = 51;

    /// The 28 `0` characters of every fixture FILLER.
    const ZERO_FILLER: &str = "0000000000000000000000000000";

    /// Record `n` (from 1) of the ASCII fixture, without its LF.
    fn ascii_record(n: usize) -> &'static [u8] {
        DISCGRP_ASCII
            .lines()
            .nth(n - 1)
            .unwrap_or_else(|| panic!("discgrp.txt has no record {n}"))
            .as_bytes()
    }

    /// Record `n` (from 1) of the EBCDIC fixture.
    fn ebcdic_record(n: usize) -> &'static [u8] {
        DISCGRP_EBCDIC
            .chunks(DisclosureGroupLayout::LEN)
            .nth(n - 1)
            .unwrap_or_else(|| panic!("AWS.M2.CARDDEMO.DISCGRP.PS has no record {n}"))
    }

    /// `rec` with `bytes` written over it from byte `offset`.
    fn patched(rec: &[u8], offset: usize, bytes: &[u8]) -> Vec<u8> {
        let mut out = rec.to_vec();
        out.splice(offset..offset + bytes.len(), bytes.iter().copied());
        out
    }

    /// The key (`group`, `tran_type`, `category`).
    fn key(
        group: &str,
        tran_type: &str,
        category: u16,
    ) -> Result<DisclosureGroupKey, Box<dyn std::error::Error>> {
        Ok(DisclosureGroupKey {
            dis_acct_group_id: AccountGroupId::move_from(group),
            dis_tran_type_cd: TranTypeCode::move_from(tran_type),
            dis_tran_cat_cd: TranCategoryCode::new(category)?,
        })
    }

    #[test]
    fn fields_tile_the_50_byte_record() {
        assert_eq!(
            (DisclosureGroupLayout::NAME, DisclosureGroupLayout::LEN),
            ("CVTRA02Y", 50)
        );
        assert!(field::fields_are_contiguous(
            DisclosureGroupLayout::FIELDS,
            50
        ));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_50_bytes() {
        for len in [0, 49, 51] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    DisclosureGroupLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CVTRA02Y",
                        CodecErrorKind::Length {
                            expected: 50,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn record_1_decodes_to_its_fields_and_re_encodes_in_both_charsets() -> TestResult {
        let line = concat!(
            "A000000000",
            "01",
            "0001",
            "00150{",
            "0000000000000000000000000000"
        );
        assert_eq!(ascii_record(1), line.as_bytes());

        let record = DisclosureGroupLayout::decode(line.as_bytes(), Charset::Ascii)?;
        assert_eq!(record.group.key, key("A000000000", "01", 1)?);
        assert_eq!(record.group.dis_int_rate.hundredths(), 1_500);
        assert_eq!(record.group.dis_int_rate.to_string(), "15.00");
        assert_eq!(record.filler.as_bytes(), ZERO_FILLER.as_bytes());
        assert!(record.preserved.is_empty());

        assert_eq!(
            DisclosureGroupLayout::encode(&record, Charset::Ascii),
            line.as_bytes()
        );
        assert_eq!(
            DisclosureGroupLayout::encode(&record, Charset::Ebcdic),
            ebcdic_record(1)
        );
        assert_eq!(
            DisclosureGroupLayout::decode(ebcdic_record(1), Charset::Ebcdic)?,
            record
        );
        Ok(())
    }

    #[test]
    fn record_34_holds_its_own_rate_in_each_corpus() -> TestResult {
        let line = concat!(
            "DEFAULT   ",
            "07",
            "0001",
            "00000{",
            "0000000000000000000000000000"
        );
        assert_eq!(ascii_record(34), line.as_bytes());

        let ascii = DisclosureGroupLayout::decode(line.as_bytes(), Charset::Ascii)?;
        assert_eq!(ascii.group.key, key("DEFAULT", "07", 1)?);
        assert!(ascii.group.key.is_default_group());
        assert_eq!(ascii.group.dis_int_rate, InterestRate::ZERO);
        assert_eq!(
            DisclosureGroupLayout::encode(&ascii, Charset::Ascii),
            line.as_bytes()
        );

        // E-05: the EBCDIC record spells `00150{`, which S9(04)V99 reads as
        // 15.00; everything else equals the ASCII record.
        let ebcdic = DisclosureGroupLayout::decode(ebcdic_record(34), Charset::Ebcdic)?;
        assert_eq!(ebcdic.group.dis_int_rate.hundredths(), 1_500);
        assert_eq!(ebcdic.group.dis_int_rate.to_string(), "15.00");
        assert_eq!(
            DisclosureGroupRecord {
                group: DisclosureGroup {
                    dis_int_rate: InterestRate::ZERO,
                    ..ebcdic.group.clone()
                },
                ..ebcdic.clone()
            },
            ascii
        );
        assert_eq!(
            DisclosureGroupLayout::encode(&ebcdic, Charset::Ebcdic),
            ebcdic_record(34)
        );
        Ok(())
    }

    #[test]
    fn every_fixture_record_round_trips_with_nothing_preserved() -> TestResult {
        assert_eq!(DISCGRP_ASCII.lines().count(), RECORDS);
        assert_eq!(DISCGRP_EBCDIC.len(), RECORDS * DisclosureGroupLayout::LEN);
        for n in 1..=RECORDS {
            for (stored, charset) in [
                (ascii_record(n), Charset::Ascii),
                (ebcdic_record(n), Charset::Ebcdic),
            ] {
                let record = DisclosureGroupLayout::decode(stored, charset)?;
                assert!(record.preserved.is_empty(), "record {n} in {charset:?}");
                assert_eq!(record.filler.as_bytes(), ZERO_FILLER.as_bytes());
                assert_eq!(
                    DisclosureGroupLayout::encode(&record, charset),
                    stored,
                    "record {n} in {charset:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn a_negative_rate_round_trips_in_both_charsets() -> TestResult {
        let record = DisclosureGroupRecord {
            group: DisclosureGroup {
                key: key("ZEROAPR", "02", 3)?,
                dis_int_rate: InterestRate::from_hundredths(-150)?,
            },
            filler: Alnum::move_from(ZERO_FILLER),
            preserved: PreservedFields::new(),
        };
        let ascii = DisclosureGroupLayout::encode(&record, Charset::Ascii);
        assert_eq!(
            ascii,
            concat!(
                "ZEROAPR   ",
                "02",
                "0003",
                "00015}",
                "0000000000000000000000000000"
            )
            .as_bytes()
        );
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let stored = DisclosureGroupLayout::encode(&record, charset);
            assert_eq!(stored.len(), 50);
            assert_eq!(
                DisclosureGroupLayout::decode(&stored, charset)?,
                record,
                "{charset:?}"
            );
        }
        // CP037 `}` is 0xD0: zone nibble D, the negative sign.
        let ebcdic = DisclosureGroupLayout::encode(&record, Charset::Ebcdic);
        assert_eq!(
            ebcdic.get(16..22),
            Some(&[0xF0, 0xF0, 0xF0, 0xF1, 0xF5, 0xD0][..])
        );
        Ok(())
    }

    #[test]
    fn a_negative_zero_rate_keeps_its_stored_bytes_until_the_rate_changes() -> TestResult {
        let stored = patched(ascii_record(1), DIS_INT_RATE.offset, b"00000}");
        let mut record = DisclosureGroupLayout::decode(&stored, Charset::Ascii)?;
        assert_eq!(record.group.dis_int_rate, InterestRate::ZERO);
        assert_eq!(record.preserved.len(), 1);
        assert_eq!(
            DisclosureGroupLayout::encode(&record, Charset::Ascii),
            stored
        );

        record.group.dis_int_rate = InterestRate::from_hundredths(1)?;
        assert_eq!(
            DisclosureGroupLayout::encode(&record, Charset::Ascii),
            patched(&stored, DIS_INT_RATE.offset, b"00000A")
        );
        Ok(())
    }

    #[test]
    fn field_errors_name_their_field_and_offset() {
        let cases = [
            (
                DIS_TRAN_CAT_CD.offset,
                &b"00A1"[..],
                "DIS-TRAN-CAT-CD",
                CodecErrorKind::NotNumeric { byte: b'A' },
            ),
            (
                DIS_INT_RATE.offset,
                &b"0X150{"[..],
                "DIS-INT-RATE",
                CodecErrorKind::NotNumeric { byte: b'X' },
            ),
            (
                DIS_INT_RATE.offset,
                &b"001500"[..],
                "DIS-INT-RATE",
                CodecErrorKind::PlainDigitInSigned { byte: b'0' },
            ),
            (
                DIS_INT_RATE.offset,
                &b"00150 "[..],
                "DIS-INT-RATE",
                CodecErrorKind::InvalidSign { byte: b' ' },
            ),
        ];
        for (offset, bytes, name, kind) in cases {
            let stored = patched(ascii_record(1), offset, bytes);
            assert_eq!(
                DisclosureGroupLayout::decode(&stored, Charset::Ascii),
                Err(CodecError::new("CVTRA02Y", kind).at_field(name, offset)),
                "{name} {bytes:?}"
            );
        }
    }

    #[test]
    fn filler_is_written_from_its_stored_bytes() -> TestResult {
        let mut record = DisclosureGroupLayout::decode(ascii_record(18), Charset::Ascii)?;
        record.filler = Alnum::spaces();
        let encoded = DisclosureGroupLayout::encode(&record, Charset::Ascii);
        assert_eq!(encoded.get(FILLER.offset..), Some(&[b' '; 28][..]));
        assert_eq!(encoded.get(..FILLER.offset), ascii_record(18).get(..22));
        Ok(())
    }
}
