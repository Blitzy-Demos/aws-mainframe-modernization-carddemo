//! `CVTRA06Y` `DALYTRAN-RECORD`: the 350-byte daily transaction record
//! [app/cpy/CVTRA06Y.cpy:L2-18].
//!
//! `DALYTRAN` is the sequential input that `POSTTRAN` posts with `CBTRN02C`;
//! `batch posttran --input app/data/ASCII/dailytran.txt` decodes each of its
//! 300 records with [`DailyTranLayout`]. The EBCDIC counterpart is
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.DALYTRAN.PS`, the same 300 records as
//! 350-byte CP037 blocks. A rejected record is copied whole, as read, into
//! `REJECT-TRAN-DATA` of the `DALYREJS` reject record (P-CBTRN02C-001).
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `DALYTRAN-ID` | `X(16)` | 0 | 16 | P-CVTRA06Y-001 | `TranId` |
//! | `DALYTRAN-TYPE-CD` | `X(02)` | 16 | 2 | P-CVTRA06Y-002 | `TranTypeCode` |
//! | `DALYTRAN-CAT-CD` | `9(04)` | 18 | 4 | P-CVTRA06Y-003 | `TranCategoryCode` |
//! | `DALYTRAN-SOURCE` | `X(10)` | 22 | 10 | P-CVTRA06Y-004 | `Alnum<10>` |
//! | `DALYTRAN-DESC` | `X(100)` | 32 | 100 | P-CVTRA06Y-005 | `Alnum<100>` |
//! | `DALYTRAN-AMT` | `S9(09)V99` | 132 | 11 | P-CVTRA06Y-006 | `Money<9>` |
//! | `DALYTRAN-MERCHANT-ID` | `9(09)` | 143 | 9 | P-CVTRA06Y-007 | `MerchantId` |
//! | `DALYTRAN-MERCHANT-NAME` | `X(50)` | 152 | 50 | P-CVTRA06Y-008 | `Alnum<50>` |
//! | `DALYTRAN-MERCHANT-CITY` | `X(50)` | 202 | 50 | P-CVTRA06Y-009 | `Alnum<50>` |
//! | `DALYTRAN-MERCHANT-ZIP` | `X(10)` | 252 | 10 | P-CVTRA06Y-010 | `Alnum<10>` |
//! | `DALYTRAN-CARD-NUM` | `X(16)` | 262 | 16 | P-CVTRA06Y-011 | `CardNumber` |
//! | `DALYTRAN-ORIG-TS` | `X(26)` | 278 | 26 | P-CVTRA06Y-012 | `Alnum<26>` |
//! | `DALYTRAN-PROC-TS` | `X(26)` | 304 | 26 | P-CVTRA06Y-013 | `Alnum<26>` |
//! | `FILLER` | `X(20)` | 330 | 20 | P-CVTRA06Y-014 | `Alnum<20>`, raw |
//!
//! # Fixture facts
//!
//! Every record of `dailytran.txt` is 350 bytes followed by LF.
//! `DALYTRAN-AMT` carries a trailing overpunch in all 300: 250 positive, 50
//! negative, none a negative zero, so [`DailyTranRecord::preserved`] is empty
//! for the whole corpus. `DALYTRAN-PROC-TS` is 26 spaces and `FILLER` 20
//! spaces in every record; both are read as stored and written back as held.
//!
//! The `CVTRA05Y` transaction record has the same offsets under `TRAN-` names,
//! but it is its own copybook with its own audit rows, so its layout owns its
//! constants and shares nothing with this one but [`crate::field`].

use carddemo_domain::entities::DailyTran;
use carddemo_domain::numeric::{
    Alnum, CardNumber, MerchantId, Money, NumericError, TranCategoryCode, TranId, TranTypeCode,
};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec, PreservedFields};

/// `CVTRA06Y` `DALYTRAN-ID PIC X(16)` — P-CVTRA06Y-001
/// [app/cpy/CVTRA06Y.cpy:L5].
pub const DALYTRAN_ID: FieldSpec = FieldSpec::new("DALYTRAN-ID", 0, 16, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-TYPE-CD PIC X(02)` — P-CVTRA06Y-002
/// [app/cpy/CVTRA06Y.cpy:L6].
pub const DALYTRAN_TYPE_CD: FieldSpec = FieldSpec::new("DALYTRAN-TYPE-CD", 16, 2, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-CAT-CD PIC 9(04)` — P-CVTRA06Y-003
/// [app/cpy/CVTRA06Y.cpy:L7].
pub const DALYTRAN_CAT_CD: FieldSpec =
    FieldSpec::new("DALYTRAN-CAT-CD", 18, 4, FieldKind::Unsigned);

/// `CVTRA06Y` `DALYTRAN-SOURCE PIC X(10)` — P-CVTRA06Y-004
/// [app/cpy/CVTRA06Y.cpy:L8].
pub const DALYTRAN_SOURCE: FieldSpec = FieldSpec::new("DALYTRAN-SOURCE", 22, 10, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-DESC PIC X(100)` — P-CVTRA06Y-005
/// [app/cpy/CVTRA06Y.cpy:L9].
pub const DALYTRAN_DESC: FieldSpec = FieldSpec::new("DALYTRAN-DESC", 32, 100, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-AMT PIC S9(09)V99`, trailing overpunch — P-CVTRA06Y-006
/// [app/cpy/CVTRA06Y.cpy:L10].
pub const DALYTRAN_AMT: FieldSpec =
    FieldSpec::new("DALYTRAN-AMT", 132, 11, FieldKind::Signed { scale: 2 });

/// `CVTRA06Y` `DALYTRAN-MERCHANT-ID PIC 9(09)` — P-CVTRA06Y-007
/// [app/cpy/CVTRA06Y.cpy:L11].
pub const DALYTRAN_MERCHANT_ID: FieldSpec =
    FieldSpec::new("DALYTRAN-MERCHANT-ID", 143, 9, FieldKind::Unsigned);

/// `CVTRA06Y` `DALYTRAN-MERCHANT-NAME PIC X(50)` — P-CVTRA06Y-008
/// [app/cpy/CVTRA06Y.cpy:L12].
pub const DALYTRAN_MERCHANT_NAME: FieldSpec =
    FieldSpec::new("DALYTRAN-MERCHANT-NAME", 152, 50, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-MERCHANT-CITY PIC X(50)` — P-CVTRA06Y-009
/// [app/cpy/CVTRA06Y.cpy:L13].
pub const DALYTRAN_MERCHANT_CITY: FieldSpec =
    FieldSpec::new("DALYTRAN-MERCHANT-CITY", 202, 50, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-MERCHANT-ZIP PIC X(10)` — P-CVTRA06Y-010
/// [app/cpy/CVTRA06Y.cpy:L14].
pub const DALYTRAN_MERCHANT_ZIP: FieldSpec =
    FieldSpec::new("DALYTRAN-MERCHANT-ZIP", 252, 10, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-CARD-NUM PIC X(16)` — P-CVTRA06Y-011
/// [app/cpy/CVTRA06Y.cpy:L15].
pub const DALYTRAN_CARD_NUM: FieldSpec =
    FieldSpec::new("DALYTRAN-CARD-NUM", 262, 16, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-ORIG-TS PIC X(26)` — P-CVTRA06Y-012
/// [app/cpy/CVTRA06Y.cpy:L16].
pub const DALYTRAN_ORIG_TS: FieldSpec =
    FieldSpec::new("DALYTRAN-ORIG-TS", 278, 26, FieldKind::Alnum);

/// `CVTRA06Y` `DALYTRAN-PROC-TS PIC X(26)`, blank in every fixture record —
/// P-CVTRA06Y-013 [app/cpy/CVTRA06Y.cpy:L17].
pub const DALYTRAN_PROC_TS: FieldSpec =
    FieldSpec::new("DALYTRAN-PROC-TS", 304, 26, FieldKind::Alnum);

/// `CVTRA06Y` `FILLER PIC X(20)`, kept raw — P-CVTRA06Y-014
/// [app/cpy/CVTRA06Y.cpy:L18].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 330, 20, FieldKind::Filler);

/// The `CVTRA06Y` `DALYTRAN-RECORD` layout, 350 bytes
/// [app/cpy/CVTRA06Y.cpy:L2-18].
pub struct DailyTranLayout;

/// One decoded `DALYTRAN` record: the domain entity, plus the bytes the entity
/// does not model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyTranRecord {
    /// `DALYTRAN-ID` through `DALYTRAN-PROC-TS` (P-CVTRA06Y-001 to
    /// P-CVTRA06Y-013) [app/cpy/CVTRA06Y.cpy:L5-17].
    pub daily_tran: DailyTran,
    /// `FILLER PIC X(20)`, its stored bytes — P-CVTRA06Y-014
    /// [app/cpy/CVTRA06Y.cpy:L18].
    pub filler: Alnum<20>,
    /// The stored bytes of a negative-zero `DALYTRAN-AMT`, which
    /// [`Money`] cannot hold. Empty for every fixture record; a non-empty list
    /// on fixture data is an escalation, never a repair.
    pub preserved: PreservedFields,
}

impl Layout for DailyTranLayout {
    const LEN: usize = 350;
    const NAME: &'static str = "CVTRA06Y";
    const FIELDS: &'static [FieldSpec] = &[
        DALYTRAN_ID,
        DALYTRAN_TYPE_CD,
        DALYTRAN_CAT_CD,
        DALYTRAN_SOURCE,
        DALYTRAN_DESC,
        DALYTRAN_AMT,
        DALYTRAN_MERCHANT_ID,
        DALYTRAN_MERCHANT_NAME,
        DALYTRAN_MERCHANT_CITY,
        DALYTRAN_MERCHANT_ZIP,
        DALYTRAN_CARD_NUM,
        DALYTRAN_ORIG_TS,
        DALYTRAN_PROC_TS,
        FILLER,
    ];
    type Record = DailyTranRecord;

    /// Reads `DALYTRAN-ID` through `FILLER` (P-CVTRA06Y-001 to P-CVTRA06Y-014)
    /// [app/cpy/CVTRA06Y.cpy:L5-18] in copybook order.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 350 bytes long.
    /// - [`CodecErrorKind::NotNumeric`] for a non-digit in `DALYTRAN-CAT-CD`,
    ///   `DALYTRAN-MERCHANT-ID` or the leading digits of `DALYTRAN-AMT`.
    /// - [`CodecErrorKind::PlainDigitInSigned`] or
    ///   [`CodecErrorKind::InvalidSign`] for a last `DALYTRAN-AMT` byte that is
    ///   no overpunch glyph.
    ///
    /// Each field error is located at its field.
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
        let daily_tran = decode_daily_tran(&rec, &mut preserved)?;
        Ok(DailyTranRecord {
            daily_tran,
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
            preserved,
        })
    }

    /// Writes `DALYTRAN-ID` through `FILLER` (P-CVTRA06Y-001 to P-CVTRA06Y-014)
    /// [app/cpy/CVTRA06Y.cpy:L5-18], restores a preserved negative-zero
    /// `DALYTRAN-AMT`, then translates the 350 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let mut out = vec![b' '; Self::LEN];
        encode_daily_tran(&mut out, &record.daily_tran, &record.preserved);
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// Reads the 13 modelled fields `DALYTRAN-ID` through `DALYTRAN-PROC-TS`
/// (P-CVTRA06Y-001 to P-CVTRA06Y-013) [app/cpy/CVTRA06Y.cpy:L5-17] of the
/// internal record `rec`, recording a negative-zero amount in `preserved`.
fn decode_daily_tran(rec: &[u8], preserved: &mut PreservedFields) -> Result<DailyTran, CodecError> {
    let name = DailyTranLayout::NAME;
    Ok(DailyTran {
        dalytran_id: TranId::from_alnum(field::alnum(name, rec, &DALYTRAN_ID)?),
        dalytran_type_cd: TranTypeCode::from_alnum(field::alnum(name, rec, &DALYTRAN_TYPE_CD)?),
        dalytran_cat_cd: decode_cat_cd(rec)?,
        dalytran_source: field::alnum(name, rec, &DALYTRAN_SOURCE)?,
        dalytran_desc: field::alnum(name, rec, &DALYTRAN_DESC)?,
        dalytran_amt: decode_amt(rec, preserved)?,
        dalytran_merchant_id: decode_merchant_id(rec)?,
        dalytran_merchant_name: field::alnum(name, rec, &DALYTRAN_MERCHANT_NAME)?,
        dalytran_merchant_city: field::alnum(name, rec, &DALYTRAN_MERCHANT_CITY)?,
        dalytran_merchant_zip: field::alnum(name, rec, &DALYTRAN_MERCHANT_ZIP)?,
        dalytran_card_num: CardNumber::from_alnum(field::alnum(name, rec, &DALYTRAN_CARD_NUM)?),
        dalytran_orig_ts: field::alnum(name, rec, &DALYTRAN_ORIG_TS)?,
        dalytran_proc_ts: field::alnum(name, rec, &DALYTRAN_PROC_TS)?,
    })
}

/// `DALYTRAN-CAT-CD PIC 9(04)` — P-CVTRA06Y-003 [app/cpy/CVTRA06Y.cpy:L7].
fn decode_cat_cd(rec: &[u8]) -> Result<TranCategoryCode, CodecError> {
    let digits = field::unsigned(DailyTranLayout::NAME, rec, &DALYTRAN_CAT_CD)?;
    u16::try_from(digits)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(TranCategoryCode::new)
        .map_err(|e| value_error(&DALYTRAN_CAT_CD, e))
}

/// `DALYTRAN-AMT PIC S9(09)V99`: the overpunch value in cents — P-CVTRA06Y-006
/// [app/cpy/CVTRA06Y.cpy:L10].
fn decode_amt(rec: &[u8], preserved: &mut PreservedFields) -> Result<Money<9>, CodecError> {
    field::signed_preserving(DailyTranLayout::NAME, rec, &DALYTRAN_AMT, preserved)?
        .value()
        .and_then(Money::from_cents)
        .map_err(|e| value_error(&DALYTRAN_AMT, e))
}

/// `DALYTRAN-MERCHANT-ID PIC 9(09)` — P-CVTRA06Y-007
/// [app/cpy/CVTRA06Y.cpy:L11].
fn decode_merchant_id(rec: &[u8]) -> Result<MerchantId, CodecError> {
    let digits = field::unsigned(DailyTranLayout::NAME, rec, &DALYTRAN_MERCHANT_ID)?;
    u32::try_from(digits)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(MerchantId::new)
        .map_err(|e| value_error(&DALYTRAN_MERCHANT_ID, e))
}

/// The domain rejection `error` of the value read from `spec`, located at
/// that field.
fn value_error(spec: &FieldSpec, error: NumericError) -> CodecError {
    CodecError::new(
        DailyTranLayout::NAME,
        CodecErrorKind::Value(error.to_string()),
    )
    .at_field(spec.name, spec.offset)
}

/// Writes the 13 modelled fields of `tran` (P-CVTRA06Y-001 to P-CVTRA06Y-013)
/// [app/cpy/CVTRA06Y.cpy:L5-17] into the internal record buffer `out`, the
/// amount through `preserved`.
fn encode_daily_tran(out: &mut [u8], tran: &DailyTran, preserved: &PreservedFields) {
    field::put_alnum(out, &DALYTRAN_ID, tran.dalytran_id.as_bytes());
    field::put_alnum(out, &DALYTRAN_TYPE_CD, tran.dalytran_type_cd.as_bytes());
    field::put_unsigned(out, &DALYTRAN_CAT_CD, u64::from(tran.dalytran_cat_cd.get()));
    field::put_alnum(out, &DALYTRAN_SOURCE, tran.dalytran_source.as_bytes());
    field::put_alnum(out, &DALYTRAN_DESC, tran.dalytran_desc.as_bytes());
    field::put_signed_preserving(out, &DALYTRAN_AMT, tran.dalytran_amt.cents(), preserved);
    field::put_unsigned(
        out,
        &DALYTRAN_MERCHANT_ID,
        u64::from(tran.dalytran_merchant_id.get()),
    );
    field::put_alnum(
        out,
        &DALYTRAN_MERCHANT_NAME,
        tran.dalytran_merchant_name.as_bytes(),
    );
    field::put_alnum(
        out,
        &DALYTRAN_MERCHANT_CITY,
        tran.dalytran_merchant_city.as_bytes(),
    );
    field::put_alnum(
        out,
        &DALYTRAN_MERCHANT_ZIP,
        tran.dalytran_merchant_zip.as_bytes(),
    );
    field::put_alnum(out, &DALYTRAN_CARD_NUM, tran.dalytran_card_num.as_bytes());
    field::put_alnum(out, &DALYTRAN_ORIG_TS, tran.dalytran_orig_ts.as_bytes());
    field::put_alnum(out, &DALYTRAN_PROC_TS, tran.dalytran_proc_ts.as_bytes());
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::DailyTran;
    use carddemo_domain::numeric::{
        Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode,
    };

    use super::{DailyTranLayout, DailyTranRecord};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, PreservedFields, field};

    /// The ASCII `DALYTRAN` fixture: 300 records of 350 bytes, each ending LF.
    const DAILYTRAN_ASCII: &str = include_str!("../../../../app/data/ASCII/dailytran.txt");

    /// The EBCDIC `DALYTRAN` fixture: the same 300 records as 350-byte CP037
    /// blocks.
    const DAILYTRAN_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.DALYTRAN.PS");

    const CHARSETS: [Charset; 2] = [Charset::Ascii, Charset::Ebcdic];

    /// `parts` concatenated, each text right-padded with spaces to its width.
    /// The widths are written out from the copybook, not taken from the
    /// layout, so the layout's offsets are checked rather than assumed.
    fn padded(parts: &[(&str, usize)]) -> Vec<u8> {
        let mut out = Vec::new();
        for &(text, width) in parts {
            assert!(text.len() <= width, "{text:?} is wider than {width}");
            out.extend_from_slice(text.as_bytes());
            out.resize(out.len() + (width - text.len()), b' ');
        }
        out
    }

    /// Record 2 of `dailytran.txt`, a return with a negative amount, field by
    /// field at the `CVTRA06Y` widths [app/cpy/CVTRA06Y.cpy:L5-18].
    fn fixture_line_2() -> Vec<u8> {
        padded(&[
            ("0000000001774260", 16),
            ("03", 2),
            ("0001", 4),
            ("OPERATOR", 10),
            ("Return item at Nitzsche, Nicolas and Lowe", 100),
            ("0000009190}", 11),
            ("800000000", 9),
            ("Nitzsche, Nicolas and Lowe", 50),
            ("Fidelshire", 50),
            ("53378", 10),
            ("0927987108636232", 16),
            ("2022-06-10 19:27:53.000000", 26),
            ("", 26),
            ("", 20),
        ])
    }

    /// The stored EBCDIC record `n`, counted from 1.
    fn ebcdic_record(n: usize) -> &'static [u8] {
        DAILYTRAN_EBCDIC
            .chunks(DailyTranLayout::LEN)
            .nth(n - 1)
            .unwrap_or_else(|| panic!("the EBCDIC DALYTRAN file has no record {n}"))
    }

    /// A purchase built by hand from record 1's values (`0000005047G`, cents
    /// 50 477), with a processing timestamp and a non-blank filler so that
    /// every byte of the record carries content.
    fn purchase_record() -> DailyTranRecord {
        DailyTranRecord {
            daily_tran: DailyTran {
                dalytran_id: TranId::move_from("0000000000683580"),
                dalytran_type_cd: TranTypeCode::move_from("01"),
                dalytran_cat_cd: TranCategoryCode::new(1).expect("1 fits 9(04)"),
                dalytran_source: Alnum::move_from("POS TERM"),
                dalytran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
                dalytran_amt: Money::from_cents(50_477).expect("504.77 fits S9(09)V99"),
                dalytran_merchant_id: MerchantId::new(800_000_000).expect("fits 9(09)"),
                dalytran_merchant_name: Alnum::move_from("Abshire-Lowe"),
                dalytran_merchant_city: Alnum::move_from("North Enoshaven"),
                dalytran_merchant_zip: Alnum::move_from("72112"),
                dalytran_card_num: CardNumber::move_from("4859452612877065"),
                dalytran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
                dalytran_proc_ts: Alnum::move_from("2022-07-01-12.00.00.000000"),
            },
            filler: Alnum::from_array(*b"0123456789ABCDEFGHIJ"),
            preserved: PreservedFields::new(),
        }
    }

    #[test]
    fn fields_tile_the_350_byte_record() {
        assert_eq!(
            (DailyTranLayout::NAME, DailyTranLayout::LEN),
            ("CVTRA06Y", 350)
        );
        assert_eq!(DailyTranLayout::FIELDS.len(), 14);
        assert!(field::fields_are_contiguous(DailyTranLayout::FIELDS, 350));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_350_bytes() {
        for len in [0, 349, 351] {
            for charset in CHARSETS {
                assert_eq!(
                    DailyTranLayout::decode(&vec![b' '; len], charset),
                    Err(CodecError::new(
                        "CVTRA06Y",
                        CodecErrorKind::Length {
                            expected: 350,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn fixture_record_2_decodes_to_its_fields_and_its_bytes() -> Result<(), CodecError> {
        let line = fixture_line_2();
        assert_eq!(
            DAILYTRAN_ASCII.lines().nth(1).map(str::as_bytes),
            Some(&line[..])
        );
        let record = DailyTranLayout::decode(&line, Charset::Ascii)?;
        let tran = &record.daily_tran;
        assert_eq!(tran.dalytran_id, TranId::move_from("0000000001774260"));
        assert_eq!(tran.dalytran_type_cd, TranTypeCode::move_from("03"));
        assert_eq!(tran.dalytran_cat_cd.get(), 1);
        assert_eq!(tran.dalytran_source, Alnum::move_from("OPERATOR"));
        assert_eq!(tran.dalytran_amt.cents(), -91_900);
        assert_eq!(tran.dalytran_merchant_id.get(), 800_000_000);
        assert_eq!(
            tran.dalytran_card_num,
            CardNumber::move_from("0927987108636232")
        );
        assert_eq!(
            tran.dalytran_orig_ts,
            Alnum::move_from("2022-06-10 19:27:53.000000")
        );
        assert_eq!(tran.dalytran_proc_ts, Alnum::spaces());
        assert_eq!(record.filler, Alnum::spaces());
        assert!(record.preserved.is_empty());
        assert_eq!(DailyTranLayout::encode(&record, Charset::Ascii), line);
        assert_eq!(
            DailyTranLayout::encode(&record, Charset::Ebcdic),
            ebcdic_record(2)
        );
        assert_eq!(
            DailyTranLayout::decode(ebcdic_record(2), Charset::Ebcdic)?,
            record
        );
        Ok(())
    }

    #[test]
    fn hand_built_records_round_trip_in_both_charsets() -> Result<(), CodecError> {
        let purchase = purchase_record();
        let ascii = DailyTranLayout::encode(&purchase, Charset::Ascii);
        assert_eq!(ascii.get(132..143), Some(&b"0000005047G"[..]));
        assert_eq!(ascii.get(330..), Some(&b"0123456789ABCDEFGHIJ"[..]));
        let ret = DailyTranLayout::decode(&fixture_line_2(), Charset::Ascii)?;
        for record in [purchase, ret] {
            for charset in CHARSETS {
                let bytes = DailyTranLayout::encode(&record, charset);
                assert_eq!(bytes.len(), 350, "{charset:?}");
                assert_eq!(
                    DailyTranLayout::decode(&bytes, charset)?,
                    record,
                    "{charset:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn every_fixture_record_round_trips_with_nothing_preserved() -> Result<(), CodecError> {
        let lines: Vec<&str> = DAILYTRAN_ASCII.lines().collect();
        assert_eq!(lines.len(), 300);
        assert_eq!(DAILYTRAN_EBCDIC.len(), 300 * DailyTranLayout::LEN);
        let mut negative = 0;
        for (n, line) in (1..).zip(&lines) {
            let record = DailyTranLayout::decode(line.as_bytes(), Charset::Ascii)
                .map_err(|e| e.at_record(n))?;
            assert!(record.preserved.is_empty(), "record {n}");
            assert!(record.daily_tran.dalytran_proc_ts.is_blank(), "record {n}");
            assert_eq!(record.filler, Alnum::spaces(), "record {n}");
            assert_eq!(
                DailyTranLayout::encode(&record, Charset::Ascii),
                line.as_bytes(),
                "record {n}"
            );
            assert_eq!(
                DailyTranLayout::encode(&record, Charset::Ebcdic),
                ebcdic_record(n),
                "record {n}"
            );
            if record.daily_tran.dalytran_amt.is_negative() {
                negative += 1;
            }
        }
        assert_eq!((lines.len() - negative, negative), (250, 50));
        Ok(())
    }

    #[test]
    fn a_negative_zero_amount_is_preserved_until_the_amount_changes() -> Result<(), CodecError> {
        let mut line = fixture_line_2();
        line[132..143].copy_from_slice(b"0000000000}");
        for charset in CHARSETS {
            let stored = charset.from_internal(&line);
            let mut record = DailyTranLayout::decode(&stored, charset)?;
            assert!(record.daily_tran.dalytran_amt.is_zero(), "{charset:?}");
            assert_eq!(record.preserved.len(), 1, "{charset:?}");
            assert_eq!(
                record.preserved.get(132).map(|raw| raw.bytes.as_slice()),
                Some(&b"0000000000}"[..]),
                "{charset:?}"
            );
            assert_eq!(DailyTranLayout::encode(&record, charset), stored);

            record.daily_tran.dalytran_amt = Money::from_cents(-1).expect("-0.01 fits");
            let edited = charset.to_internal(&DailyTranLayout::encode(&record, charset));
            assert_eq!(
                edited.get(132..143),
                Some(&b"0000000000J"[..]),
                "{charset:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_malformed_numeric_field_is_reported_at_its_field() {
        use CodecErrorKind::{InvalidSign, NotNumeric, PlainDigitInSigned};
        let cases = [
            (18, b'X', "DALYTRAN-CAT-CD", 18, NotNumeric { byte: b'X' }),
            (132, b'-', "DALYTRAN-AMT", 132, NotNumeric { byte: b'-' }),
            (
                142,
                b'5',
                "DALYTRAN-AMT",
                132,
                PlainDigitInSigned { byte: b'5' },
            ),
            (142, b'*', "DALYTRAN-AMT", 132, InvalidSign { byte: b'*' }),
            (
                151,
                b' ',
                "DALYTRAN-MERCHANT-ID",
                143,
                NotNumeric { byte: b' ' },
            ),
        ];
        for (at, byte, name, offset, kind) in cases {
            let mut line = fixture_line_2();
            line[at] = byte;
            assert_eq!(
                DailyTranLayout::decode(&line, Charset::Ascii),
                Err(CodecError::new("CVTRA06Y", kind).at_field(name, offset)),
                "byte 0x{byte:02X} at offset {at}"
            );
        }
    }
}
