//! `CVTRA01Y` `TRAN-CAT-BAL-RECORD`: the 50-byte `TCATBALF` transaction
//! category balance record [app/cpy/CVTRA01Y.cpy:L2-10].
//!
//! The record opens with the group `05 TRAN-CAT-KEY`
//! [app/cpy/CVTRA01Y.cpy:L5], the 17-byte key of the KSDS, made of the first
//! three elementary fields. A group holds no bytes of its own, so
//! [`TranCatBalanceLayout::FIELDS`] lists the elementary items only, and the
//! group becomes the domain `TranCatBalanceKey`.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `TRANCAT-ACCT-ID` | `9(11)` | 0 | 11 | P-CVTRA01Y-001 | `AccountId` |
//! | `TRANCAT-TYPE-CD` | `X(02)` | 11 | 2 | P-CVTRA01Y-002 | `TranTypeCode` |
//! | `TRANCAT-CD` | `9(04)` | 13 | 4 | P-CVTRA01Y-003 | `TranCategoryCode` |
//! | `TRAN-CAT-BAL` | `S9(09)V99` | 17 | 11 | P-CVTRA01Y-004 | `Money<9>` |
//! | `FILLER` | `X(22)` | 28 | 22 | P-CVTRA01Y-005 | `Alnum<22>`, raw |
//!
//! # Fixture bytes
//!
//! `app/data/ASCII/tcatbal.txt` and its EBCDIC counterpart
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.TCATBALF.PS` hold the same 50 records.
//! Every `TRAN-CAT-BAL` is +0.00, written with the positive overpunch
//! `0000000000{`. The `FILLER` of every record holds 22 `0` characters, not
//! spaces. The filler is not stored in the database and the domain entity does
//! not model it, so [`TranCatBalanceRecord`] keeps its bytes raw and the
//! encoder writes them back as they are, never normalised to spaces.
//!
//! The ASCII file ends records 1–49 with CR LF and record 50 with LF alone
//! (E-04). Terminators are [`crate::framing`]'s concern: this layout sees only
//! the 50-byte record body.

use carddemo_domain::entities::{TranCatBalance, TranCatBalanceKey};
use carddemo_domain::numeric::{
    AccountId, Alnum, Money, NumericError, TranCategoryCode, TranTypeCode,
};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec, PreservedFields};

/// `CVTRA01Y` `TRANCAT-ACCT-ID PIC 9(11)`, the first part of `TRAN-CAT-KEY` —
/// P-CVTRA01Y-001 [app/cpy/CVTRA01Y.cpy:L6].
pub const TRANCAT_ACCT_ID: FieldSpec =
    FieldSpec::new("TRANCAT-ACCT-ID", 0, 11, FieldKind::Unsigned);

/// `CVTRA01Y` `TRANCAT-TYPE-CD PIC X(02)`, the second part of `TRAN-CAT-KEY` —
/// P-CVTRA01Y-002 [app/cpy/CVTRA01Y.cpy:L7].
pub const TRANCAT_TYPE_CD: FieldSpec = FieldSpec::new("TRANCAT-TYPE-CD", 11, 2, FieldKind::Alnum);

/// `CVTRA01Y` `TRANCAT-CD PIC 9(04)`, the last part of `TRAN-CAT-KEY`, which
/// ends at byte 17 — P-CVTRA01Y-003 [app/cpy/CVTRA01Y.cpy:L8].
pub const TRANCAT_CD: FieldSpec = FieldSpec::new("TRANCAT-CD", 13, 4, FieldKind::Unsigned);

/// `CVTRA01Y` `TRAN-CAT-BAL PIC S9(09)V99`, signed by a trailing overpunch —
/// P-CVTRA01Y-004 [app/cpy/CVTRA01Y.cpy:L9].
pub const TRAN_CAT_BAL: FieldSpec =
    FieldSpec::new("TRAN-CAT-BAL", 17, 11, FieldKind::Signed { scale: 2 });

/// `CVTRA01Y` `FILLER PIC X(22)`, kept raw: the fixture bytes are `0`
/// characters — P-CVTRA01Y-005 [app/cpy/CVTRA01Y.cpy:L10].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 28, 22, FieldKind::Filler);

/// The `CVTRA01Y` `TRAN-CAT-BAL-RECORD` layout, 50 bytes
/// [app/cpy/CVTRA01Y.cpy:L2-10].
pub struct TranCatBalanceLayout;

/// One decoded `TCATBALF` record: the domain balance, the stored filler, and
/// the stored bytes of any value the typed fields cannot reproduce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranCatBalanceRecord {
    /// `TRAN-CAT-KEY` and `TRAN-CAT-BAL` — P-CVTRA01Y-001 to P-CVTRA01Y-004
    /// [app/cpy/CVTRA01Y.cpy:L5-9].
    pub balance: TranCatBalance,
    /// `FILLER PIC X(22)`, its stored bytes — P-CVTRA01Y-005
    /// [app/cpy/CVTRA01Y.cpy:L10].
    pub filler: Alnum<22>,
    /// The stored bytes of a negative-zero `TRAN-CAT-BAL`, which `Money<9>`
    /// cannot hold. Empty for every fixture record.
    pub preserved: PreservedFields,
}

impl Layout for TranCatBalanceLayout {
    const LEN: usize = 50;
    const NAME: &'static str = "CVTRA01Y";
    const FIELDS: &'static [FieldSpec] = &[
        TRANCAT_ACCT_ID,
        TRANCAT_TYPE_CD,
        TRANCAT_CD,
        TRAN_CAT_BAL,
        FILLER,
    ];
    type Record = TranCatBalanceRecord;

    /// Reads `TRANCAT-ACCT-ID` through `FILLER` (P-CVTRA01Y-001 to
    /// P-CVTRA01Y-005) [app/cpy/CVTRA01Y.cpy:L6-10] in copybook order, so the
    /// first faulty field is the one reported.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 50 bytes long.
    /// - The errors of the [`field`] readers: a non-digit in a `9(n)` field or
    ///   before the sign position of `TRAN-CAT-BAL`, or a sign byte that is not
    ///   an overpunch glyph.
    /// - [`CodecErrorKind::Value`] should a domain constructor reject a decoded
    ///   value. The field widths keep every value inside its type, so no
    ///   stored record reaches this.
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
        let trancat_acct_id = AccountId::new(field::unsigned(Self::NAME, &rec, &TRANCAT_ACCT_ID)?)
            .map_err(|e| value_error(&TRANCAT_ACCT_ID, e))?;
        let trancat_type_cd =
            TranTypeCode::from_alnum(field::alnum(Self::NAME, &rec, &TRANCAT_TYPE_CD)?);
        let trancat_cd = category_code(field::unsigned(Self::NAME, &rec, &TRANCAT_CD)?)
            .map_err(|e| value_error(&TRANCAT_CD, e))?;
        let tran_cat_bal =
            field::signed_preserving(Self::NAME, &rec, &TRAN_CAT_BAL, &mut preserved)?
                .value()
                .and_then(Money::<9>::from_cents)
                .map_err(|e| value_error(&TRAN_CAT_BAL, e))?;
        Ok(TranCatBalanceRecord {
            balance: TranCatBalance {
                key: TranCatBalanceKey {
                    trancat_acct_id,
                    trancat_type_cd,
                    trancat_cd,
                },
                tran_cat_bal,
            },
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
            preserved,
        })
    }

    /// Writes `TRANCAT-ACCT-ID` through `FILLER` (P-CVTRA01Y-001 to
    /// P-CVTRA01Y-005) [app/cpy/CVTRA01Y.cpy:L6-10]: the key digits
    /// zero-padded, the balance with its overpunch sign (a preserved negative
    /// zero written back as stored), and the filler bytes as the record holds
    /// them. The 50 bytes are then translated to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let key = &record.balance.key;
        let mut out = vec![b' '; Self::LEN];
        field::put_unsigned(&mut out, &TRANCAT_ACCT_ID, key.trancat_acct_id.get());
        field::put_alnum(&mut out, &TRANCAT_TYPE_CD, key.trancat_type_cd.as_bytes());
        field::put_unsigned(&mut out, &TRANCAT_CD, u64::from(key.trancat_cd.get()));
        field::put_signed_preserving(
            &mut out,
            &TRAN_CAT_BAL,
            record.balance.tran_cat_bal.cents(),
            &record.preserved,
        );
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// `TRANCAT-CD` (P-CVTRA01Y-003) [app/cpy/CVTRA01Y.cpy:L8] from the value its
/// four digits decode to.
///
/// # Errors
///
/// [`NumericError::OutOfRange`] for a value of `10^4` or more, which four
/// digits never produce.
fn category_code(value: u64) -> Result<TranCategoryCode, NumericError> {
    u16::try_from(value)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(TranCategoryCode::new)
}

/// The [`CodecErrorKind::Value`] error for a decoded value of `spec` that its
/// domain constructor rejects, located at the field.
fn value_error(spec: &FieldSpec, error: NumericError) -> CodecError {
    CodecError::new(
        TranCatBalanceLayout::NAME,
        CodecErrorKind::Value(error.to_string()),
    )
    .at_field(spec.name, spec.offset)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use carddemo_domain::entities::{TranCatBalance, TranCatBalanceKey};
    use carddemo_domain::numeric::{AccountId, Alnum, Money, TranCategoryCode, TranTypeCode};

    use super::{TRANCAT_CD, TranCatBalanceLayout, TranCatBalanceRecord};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, PreservedFields, field};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The ASCII `TCATBALF` fixture: 50 records, 1–49 ending CR LF and 50
    /// ending LF (E-04).
    const TCATBAL_ASCII: &[u8] = include_bytes!("../../../../app/data/ASCII/tcatbal.txt");

    /// The EBCDIC `TCATBALF` fixture: the same 50 records as 50-byte CP037
    /// blocks.
    const TCATBAL_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.TCATBALF.PS");

    /// The body of `tcatbal.txt` line 1, field by field: account 1, type `01`,
    /// category 1, balance +0.00 and 22 `0` characters of filler.
    const LINE_1: &str = concat!(
        "00000000001",
        "01",
        "0001",
        "0000000000{",
        "0000000000000000000000"
    );

    /// The 50 record bodies of `tcatbal.txt`, each without its CR LF or LF.
    fn ascii_records() -> Vec<&'static [u8]> {
        TCATBAL_ASCII
            .split_inclusive(|&byte| byte == b'\n')
            .map(|line| line.strip_suffix(b"\n").unwrap_or(line))
            .map(|line| line.strip_suffix(b"\r").unwrap_or(line))
            .collect()
    }

    #[test]
    fn fields_tile_the_50_byte_record_and_the_key_spans_17_bytes() {
        assert_eq!(
            (TranCatBalanceLayout::NAME, TranCatBalanceLayout::LEN),
            ("CVTRA01Y", 50)
        );
        assert!(field::fields_are_contiguous(
            TranCatBalanceLayout::FIELDS,
            50
        ));
        assert_eq!(TRANCAT_CD.end(), 17, "TRAN-CAT-KEY is KEYS(17 0)");
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_50_bytes() {
        for len in [0, 49, 51] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    TranCatBalanceLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CVTRA01Y",
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
    fn first_fixture_record_decodes_to_its_fields_and_re_encodes_to_its_bytes() -> TestResult {
        assert_eq!(
            TCATBAL_ASCII.get(..52),
            Some(format!("{LINE_1}\r\n").as_bytes())
        );
        let record = TranCatBalanceLayout::decode(LINE_1.as_bytes(), Charset::Ascii)?;
        let key = &record.balance.key;
        assert_eq!(key.trancat_acct_id, AccountId::new(1)?);
        assert_eq!(key.trancat_type_cd, TranTypeCode::move_from("01"));
        assert_eq!(key.trancat_cd, TranCategoryCode::new(1)?);
        assert_eq!(record.balance.tran_cat_bal.cents(), 0);
        assert_eq!(record.filler.as_bytes(), &[b'0'; 22]);
        assert!(record.preserved.is_empty());
        assert_eq!(
            TranCatBalanceLayout::encode(&record, Charset::Ascii),
            LINE_1.as_bytes()
        );
        Ok(())
    }

    #[test]
    fn every_ascii_record_re_encodes_and_is_its_ebcdic_counterpart() -> Result<(), CodecError> {
        let ascii = ascii_records();
        let ebcdic: Vec<&[u8]> = TCATBAL_EBCDIC.chunks(TranCatBalanceLayout::LEN).collect();
        assert_eq!((ascii.len(), ebcdic.len()), (50, 50));
        for (n, (stored, block)) in ascii.into_iter().zip(ebcdic).enumerate() {
            let record = TranCatBalanceLayout::decode(stored, Charset::Ascii)?;
            assert!(record.preserved.is_empty(), "record {}", n + 1);
            assert_eq!(
                TranCatBalanceLayout::encode(&record, Charset::Ascii),
                stored,
                "record {}",
                n + 1
            );
            assert_eq!(
                TranCatBalanceLayout::encode(&record, Charset::Ebcdic),
                block,
                "record {}",
                n + 1
            );
            assert_eq!(
                TranCatBalanceLayout::decode(block, Charset::Ebcdic)?,
                record,
                "record {}",
                n + 1
            );
        }
        Ok(())
    }

    #[test]
    fn negative_balance_encodes_its_overpunch_and_round_trips() -> TestResult {
        let record = TranCatBalanceRecord {
            balance: TranCatBalance {
                key: TranCatBalanceKey {
                    trancat_acct_id: AccountId::new(99_999_999_999)?,
                    trancat_type_cd: TranTypeCode::move_from("05"),
                    trancat_cd: TranCategoryCode::new(9_999)?,
                },
                tran_cat_bal: Money::from_cents(-1_234)?,
            },
            filler: Alnum::from_array(*b"0123456789ABCDEFGHIJKL"),
            preserved: PreservedFields::new(),
        };
        let ascii = TranCatBalanceLayout::encode(&record, Charset::Ascii);
        assert_eq!(ascii.get(17..28), Some(&b"0000000123M"[..]));
        assert_eq!(
            ascii,
            concat!(
                "99999999999",
                "05",
                "9999",
                "0000000123M",
                "0123456789ABCDEFGHIJKL"
            )
            .as_bytes()
        );
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = TranCatBalanceLayout::encode(&record, charset);
            assert_eq!(bytes.len(), 50, "{charset:?}");
            assert_eq!(
                TranCatBalanceLayout::decode(&bytes, charset)?,
                record,
                "{charset:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn negative_zero_balance_is_preserved_and_written_back_as_stored() -> Result<(), CodecError> {
        let stored = LINE_1.replace("0000000000{", "0000000000}");
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = charset.from_internal(stored.as_bytes());
            let record = TranCatBalanceLayout::decode(&bytes, charset)?;
            assert!(record.balance.tran_cat_bal.is_zero(), "{charset:?}");
            assert_eq!(
                record.preserved.get(17).map(|raw| raw.bytes.as_slice()),
                Some(&b"0000000000}"[..]),
                "{charset:?}"
            );
            assert_eq!(
                TranCatBalanceLayout::encode(&record, charset),
                bytes,
                "{charset:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn field_errors_name_the_layout_field_and_offset() {
        let cases = [
            (
                0,
                b'X',
                "TRANCAT-ACCT-ID",
                0,
                CodecErrorKind::NotNumeric { byte: b'X' },
            ),
            (
                16,
                b' ',
                "TRANCAT-CD",
                13,
                CodecErrorKind::NotNumeric { byte: b' ' },
            ),
            (
                27,
                b'0',
                "TRAN-CAT-BAL",
                17,
                CodecErrorKind::PlainDigitInSigned { byte: b'0' },
            ),
            (
                27,
                b'*',
                "TRAN-CAT-BAL",
                17,
                CodecErrorKind::InvalidSign { byte: b'*' },
            ),
        ];
        for (at, byte, name, offset, kind) in cases {
            let mut bytes = LINE_1.as_bytes().to_vec();
            if let Some(slot) = bytes.get_mut(at) {
                *slot = byte;
            }
            assert_eq!(
                TranCatBalanceLayout::decode(&bytes, Charset::Ascii),
                Err(CodecError::new("CVTRA01Y", kind).at_field(name, offset)),
                "byte 0x{byte:02X} at {at}"
            );
        }
    }
}
