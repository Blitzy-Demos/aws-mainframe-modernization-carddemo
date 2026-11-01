//! `CVTRA04Y` `TRAN-CAT-RECORD`: the 60-byte transaction category record of
//! the VSAM processing copy `TRANCATG` [app/cpy/CVTRA04Y.cpy:L2-9].
//!
//! `TRANCATG` is loaded from `app/data/ASCII/trancatg.txt`; its EBCDIC
//! counterpart is `app/data/EBCDIC/AWS.M2.CARDDEMO.TRANCATG.PS`, the same 18
//! records as 60-byte CP037 blocks. Only `CBTRN03C` reads it (the CR00 report
//! and `TRANREPT`), and it becomes the table `trancatg_file`. It is not the Db2
//! master `CARDDEMO.TRANSACTION_TYPE_CATEGORY` (table `transaction_categories`,
//! domain `TranCategoryMaster`), which `DB2LTCAT` loads with upper-case
//! descriptions and which agrees with this copy only after `TRANEXTR`
//! (`batch seed --only tranextr`). This layout therefore decodes into the
//! processing-copy entity `TranCategoryRecord` and never into the master.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `TRAN-TYPE-CD` | `X(02)` | 0 | 2 | P-CVTRA04Y-001 | `TranTypeCode` |
//! | `TRAN-CAT-CD` | `9(04)` | 2 | 4 | P-CVTRA04Y-002 | `TranCategoryCode` |
//! | `TRAN-CAT-TYPE-DESC` | `X(50)` | 6 | 50 | P-CVTRA04Y-003 | `Alnum<50>` |
//! | `FILLER` | `X(04)` | 56 | 4 | P-CVTRA04Y-004 | `Alnum<4>`, raw |
//!
//! The group `TRAN-CAT-KEY` [app/cpy/CVTRA04Y.cpy:L5], the 6-byte KSDS key, is
//! `TRAN-TYPE-CD` followed by `TRAN-CAT-CD` and has no bytes of its own, so it
//! is no [`FieldSpec`].
//!
//! # FILLER and framing
//!
//! The FILLER of every fixture record holds the characters `0000`, not spaces.
//! The domain record does not model it, so [`TranCategoryFileRecord::filler`]
//! keeps its stored bytes and the record re-encodes byte for byte. Every
//! `trancatg.txt` record ends CR LF (E-04); terminators are the concern of
//! [`crate::framing`], and this layout sees only the 60-byte body.

use carddemo_domain::entities::TranCategoryRecord;
use carddemo_domain::numeric::{Alnum, NumericError, TranCategoryCode, TranTypeCode};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec};

/// `CVTRA04Y` `TRAN-TYPE-CD PIC X(02)`, the first part of `TRAN-CAT-KEY` —
/// P-CVTRA04Y-001 [app/cpy/CVTRA04Y.cpy:L6].
pub const TRAN_TYPE_CD: FieldSpec = FieldSpec::new("TRAN-TYPE-CD", 0, 2, FieldKind::Alnum);

/// `CVTRA04Y` `TRAN-CAT-CD PIC 9(04)`, the second part of `TRAN-CAT-KEY` —
/// P-CVTRA04Y-002 [app/cpy/CVTRA04Y.cpy:L7].
pub const TRAN_CAT_CD: FieldSpec = FieldSpec::new("TRAN-CAT-CD", 2, 4, FieldKind::Unsigned);

/// `CVTRA04Y` `TRAN-CAT-TYPE-DESC PIC X(50)` — P-CVTRA04Y-003
/// [app/cpy/CVTRA04Y.cpy:L8].
pub const TRAN_CAT_TYPE_DESC: FieldSpec =
    FieldSpec::new("TRAN-CAT-TYPE-DESC", 6, 50, FieldKind::Alnum);

/// `CVTRA04Y` `FILLER PIC X(04)`, kept raw — P-CVTRA04Y-004
/// [app/cpy/CVTRA04Y.cpy:L9].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 56, 4, FieldKind::Filler);

/// The `CVTRA04Y` `TRAN-CAT-RECORD` layout, 60 bytes
/// [app/cpy/CVTRA04Y.cpy:L2-9].
pub struct TranCategoryLayout;

/// One decoded `TRANCATG` record: the processing-copy category and the stored
/// bytes of its FILLER.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranCategoryFileRecord {
    /// `TRAN-TYPE-CD`, `TRAN-CAT-CD` and `TRAN-CAT-TYPE-DESC` — P-CVTRA04Y-001
    /// to P-CVTRA04Y-003 [app/cpy/CVTRA04Y.cpy:L6-8].
    pub tran_category: TranCategoryRecord,
    /// `FILLER PIC X(04)`, its stored bytes (`0000` in every fixture record) —
    /// P-CVTRA04Y-004 [app/cpy/CVTRA04Y.cpy:L9].
    pub filler: Alnum<4>,
}

impl Layout for TranCategoryLayout {
    const LEN: usize = 60;
    const NAME: &'static str = "CVTRA04Y";
    const FIELDS: &'static [FieldSpec] = &[TRAN_TYPE_CD, TRAN_CAT_CD, TRAN_CAT_TYPE_DESC, FILLER];
    type Record = TranCategoryFileRecord;

    /// Reads `TRAN-TYPE-CD` through `FILLER` (P-CVTRA04Y-001 to
    /// P-CVTRA04Y-004) [app/cpy/CVTRA04Y.cpy:L6-9]: the code and the
    /// description as their stored bytes, `TRAN-CAT-CD` as four display
    /// digits.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 60 bytes long.
    /// - [`CodecErrorKind::NotNumeric`] at `TRAN-CAT-CD` (offset 2) when one of
    ///   its four bytes is not `0`–`9`.
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
        Ok(TranCategoryFileRecord {
            tran_category: TranCategoryRecord {
                tran_type_cd: TranTypeCode::from_alnum(field::alnum(
                    Self::NAME,
                    &rec,
                    &TRAN_TYPE_CD,
                )?),
                tran_cat_cd: tran_cat_cd(field::unsigned(Self::NAME, &rec, &TRAN_CAT_CD)?)?,
                tran_cat_type_desc: field::alnum(Self::NAME, &rec, &TRAN_CAT_TYPE_DESC)?,
            },
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
        })
    }

    /// Writes `TRAN-TYPE-CD` through `FILLER` (P-CVTRA04Y-001 to
    /// P-CVTRA04Y-004) [app/cpy/CVTRA04Y.cpy:L6-9]: `TRAN-CAT-CD` as its four
    /// zero-padded digits, every other field from the bytes the record holds;
    /// then translates the 60 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let category = &record.tran_category;
        let mut out = vec![b' '; Self::LEN];
        field::put_alnum(&mut out, &TRAN_TYPE_CD, category.tran_type_cd.as_bytes());
        field::put_unsigned(
            &mut out,
            &TRAN_CAT_CD,
            u64::from(category.tran_cat_cd.get()),
        );
        field::put_alnum(
            &mut out,
            &TRAN_CAT_TYPE_DESC,
            category.tran_cat_type_desc.as_bytes(),
        );
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// The decoded digits of `TRAN-CAT-CD` as the domain category code —
/// P-CVTRA04Y-002 [app/cpy/CVTRA04Y.cpy:L7].
///
/// Four display digits are always below `10^4`, so a value read by
/// [`field::unsigned`] is always accepted; the domain constructor still
/// decides, and its rejection is reported rather than assumed away.
///
/// # Errors
///
/// [`CodecErrorKind::Value`] carrying the domain `NumericError` text, located
/// at `TRAN-CAT-CD` (offset 2), when the value is `10^4` or more.
fn tran_cat_cd(digits: u64) -> Result<TranCategoryCode, CodecError> {
    i64::try_from(digits)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(TranCategoryCode::try_from)
        .map_err(|error| {
            CodecError::new(
                TranCategoryLayout::NAME,
                CodecErrorKind::Value(error.to_string()),
            )
            .at_field(TRAN_CAT_CD.name, TRAN_CAT_CD.offset)
        })
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::TranCategoryRecord;
    use carddemo_domain::numeric::{Alnum, NumericError, TranCategoryCode, TranTypeCode};

    use super::{TRAN_CAT_CD, TranCategoryFileRecord, TranCategoryLayout, tran_cat_cd};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, field};

    /// The ASCII `TRANCATG` fixture: 18 records of 60 bytes, each ending CR LF.
    const TRANCATG_ASCII: &[u8] = include_bytes!("../../../../app/data/ASCII/trancatg.txt");

    /// The EBCDIC `TRANCATG` fixture: the same 18 records as 60-byte CP037
    /// blocks.
    const TRANCATG_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.TRANCATG.PS");

    /// The 60-byte body of record `n` (counted from 1) of `trancatg.txt`,
    /// without its CR LF.
    fn ascii_body(n: usize) -> &'static [u8] {
        TRANCATG_ASCII
            .split(|&byte| byte == b'\n')
            .nth(n - 1)
            .and_then(|line| line.strip_suffix(b"\r"))
            .unwrap_or_else(|| panic!("trancatg.txt has no CR LF record {n}"))
    }

    /// Record `n` (counted from 1) of the EBCDIC `TRANCATG` file.
    fn ebcdic_record(n: usize) -> &'static [u8] {
        TRANCATG_EBCDIC
            .chunks(TranCategoryLayout::LEN)
            .nth(n - 1)
            .unwrap_or_else(|| panic!("the EBCDIC TRANCATG file has no record {n}"))
    }

    fn category(value: u16) -> TranCategoryCode {
        TranCategoryCode::new(value).unwrap_or_else(|error| panic!("{value}: {error}"))
    }

    #[test]
    fn fields_tile_the_60_byte_record() {
        assert_eq!(
            (TranCategoryLayout::NAME, TranCategoryLayout::LEN),
            ("CVTRA04Y", 60)
        );
        assert!(field::fields_are_contiguous(TranCategoryLayout::FIELDS, 60));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_60_bytes() {
        for len in [0, 59, 61] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    TranCategoryLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CVTRA04Y",
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
    fn first_fixture_record_decodes_to_its_fields_and_re_encodes_to_its_bytes()
    -> Result<(), CodecError> {
        let body = ascii_body(1);
        let mut expected = format!("{:<56}", "010001Regular Sales Draft").into_bytes();
        expected.extend_from_slice(b"0000");
        assert_eq!(body, expected.as_slice());

        let record = TranCategoryLayout::decode(body, Charset::Ascii)?;
        let category_record = &record.tran_category;
        assert_eq!(category_record.tran_type_cd, TranTypeCode::move_from("01"));
        assert_eq!(category_record.tran_cat_cd.get(), 1);
        assert_eq!(
            category_record.tran_cat_type_desc.trimmed(),
            "Regular Sales Draft"
        );
        assert_eq!(record.filler.as_bytes(), b"0000");
        assert_eq!(TranCategoryLayout::encode(&record, Charset::Ascii), body);
        Ok(())
    }

    #[test]
    fn travel_booking_record_round_trips_and_is_fixture_record_14() -> Result<(), CodecError> {
        let record = TranCategoryFileRecord {
            tran_category: TranCategoryRecord {
                tran_type_cd: TranTypeCode::move_from("04"),
                tran_cat_cd: category(3),
                tran_cat_type_desc: Alnum::move_from("Travel booking authorization"),
            },
            filler: Alnum::from_array(*b"0000"),
        };
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = TranCategoryLayout::encode(&record, charset);
            assert_eq!(bytes.len(), 60, "{charset:?}");
            assert_eq!(
                TranCategoryLayout::decode(&bytes, charset)?,
                record,
                "{charset:?}"
            );
        }
        assert_eq!(
            TranCategoryLayout::encode(&record, Charset::Ascii),
            ascii_body(14)
        );
        assert_eq!(
            TranCategoryLayout::encode(&record, Charset::Ebcdic),
            ebcdic_record(14)
        );
        Ok(())
    }

    #[test]
    fn a_non_digit_category_byte_is_rejected_at_tran_cat_cd() {
        let mut body = ascii_body(1).to_vec();
        if let Some(byte) = body.get_mut(3) {
            *byte = b' ';
        }
        assert_eq!(
            TranCategoryLayout::decode(&body, Charset::Ascii),
            Err(
                CodecError::new("CVTRA04Y", CodecErrorKind::NotNumeric { byte: b' ' })
                    .at_field("TRAN-CAT-CD", 2)
            )
        );
    }

    #[test]
    fn category_digits_beyond_four_are_a_value_error_at_tran_cat_cd() {
        assert_eq!(tran_cat_cd(9999), Ok(category(9999)));
        let out_of_range = CodecError::new(
            "CVTRA04Y",
            CodecErrorKind::Value(NumericError::OutOfRange.to_string()),
        )
        .at_field(TRAN_CAT_CD.name, TRAN_CAT_CD.offset);
        for digits in [10_000, u64::MAX] {
            assert_eq!(tran_cat_cd(digits), Err(out_of_range.clone()), "{digits}");
        }
    }
}
