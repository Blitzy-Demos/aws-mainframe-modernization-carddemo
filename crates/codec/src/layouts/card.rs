//! `CVACT02Y` `CARD-RECORD`: the 150-byte `CARDDAT` card record
//! [app/cpy/CVACT02Y.cpy:L2-11].
//!
//! `CARDFILE` defines the `CARDDAT` cluster with `KEYS(16 0)` and
//! `RECORDSIZE(150 150)` and loads it by `REPRO` from
//! `AWS.M2.CARDDEMO.CARDDATA.PS` [app/jcl/CARDFILE.jcl:L54-55]
//! [app/jcl/CARDFILE.jcl:L70-75]. That dataset is
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.CARDDATA.PS`, 50 records as 150-byte CP037
//! blocks; its ASCII counterpart `app/data/ASCII/carddata.txt` holds the same
//! 50 records of 150 bytes, each ending LF. `batch seed` decodes each ASCII
//! record with [`CardLayout`] into the `cards` table.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `CARD-NUM` | `X(16)` | 0 | 16 | P-CVACT02Y-001 | `CardNumber` |
//! | `CARD-ACCT-ID` | `9(11)` | 16 | 11 | P-CVACT02Y-002 | `AccountId` |
//! | `CARD-CVV-CD` | `9(03)` | 27 | 3 | P-CVACT02Y-003 | `Cvv` |
//! | `CARD-EMBOSSED-NAME` | `X(50)` | 30 | 50 | P-CVACT02Y-004 | `Alnum<50>` |
//! | `CARD-EXPIRAION-DATE` | `X(10)` | 80 | 10 | P-CVACT02Y-005 | `Alnum<10>` |
//! | `CARD-ACTIVE-STATUS` | `X(01)` | 90 | 1 | P-CVACT02Y-006 | `Alnum<1>` |
//! | `FILLER` | `X(59)` | 91 | 59 | P-CVACT02Y-007 | `Alnum<59>`, raw |
//!
//! The record has no signed field, so every value round-trips through its
//! typed form and the layout preserves no stored bytes beside the `FILLER`.
//! The domain entity [`Card`] does not model the `FILLER` (the `cards` table
//! does not store it), so [`CardRecord`] carries its stored bytes next to the
//! entity, and the record re-encodes to the bytes it was decoded from.

use carddemo_domain::entities::Card;
use carddemo_domain::numeric::{Alnum, CardNumber, NumericError};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec};

/// `CVACT02Y` `CARD-NUM PIC X(16)`, the `CARDDAT` key — P-CVACT02Y-001
/// [app/cpy/CVACT02Y.cpy:L5].
pub const CARD_NUM: FieldSpec = FieldSpec::new("CARD-NUM", 0, 16, FieldKind::Alnum);

/// `CVACT02Y` `CARD-ACCT-ID PIC 9(11)`, the `CARDAIX` alternate key —
/// P-CVACT02Y-002 [app/cpy/CVACT02Y.cpy:L6].
pub const CARD_ACCT_ID: FieldSpec = FieldSpec::new("CARD-ACCT-ID", 16, 11, FieldKind::Unsigned);

/// `CVACT02Y` `CARD-CVV-CD PIC 9(03)` — P-CVACT02Y-003
/// [app/cpy/CVACT02Y.cpy:L7].
pub const CARD_CVV_CD: FieldSpec = FieldSpec::new("CARD-CVV-CD", 27, 3, FieldKind::Unsigned);

/// `CVACT02Y` `CARD-EMBOSSED-NAME PIC X(50)` — P-CVACT02Y-004
/// [app/cpy/CVACT02Y.cpy:L8].
pub const CARD_EMBOSSED_NAME: FieldSpec =
    FieldSpec::new("CARD-EMBOSSED-NAME", 30, 50, FieldKind::Alnum);

/// `CVACT02Y` `CARD-EXPIRAION-DATE PIC X(10)`, the copybook spelling kept —
/// P-CVACT02Y-005 [app/cpy/CVACT02Y.cpy:L9].
pub const CARD_EXPIRAION_DATE: FieldSpec =
    FieldSpec::new("CARD-EXPIRAION-DATE", 80, 10, FieldKind::Alnum);

/// `CVACT02Y` `CARD-ACTIVE-STATUS PIC X(01)` — P-CVACT02Y-006
/// [app/cpy/CVACT02Y.cpy:L10].
pub const CARD_ACTIVE_STATUS: FieldSpec =
    FieldSpec::new("CARD-ACTIVE-STATUS", 90, 1, FieldKind::Alnum);

/// `CVACT02Y` `FILLER PIC X(59)`, kept raw — P-CVACT02Y-007
/// [app/cpy/CVACT02Y.cpy:L11].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 91, 59, FieldKind::Filler);

/// The `CVACT02Y` `CARD-RECORD` layout, 150 bytes
/// [app/cpy/CVACT02Y.cpy:L2-11].
pub struct CardLayout;

/// One decoded `CARDDAT` record: the card and the stored bytes of its
/// `FILLER`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRecord {
    /// `CARD-NUM` through `CARD-ACTIVE-STATUS` — P-CVACT02Y-001 to
    /// P-CVACT02Y-006 [app/cpy/CVACT02Y.cpy:L5-10].
    pub card: Card,
    /// `FILLER PIC X(59)`, its stored bytes — P-CVACT02Y-007
    /// [app/cpy/CVACT02Y.cpy:L11].
    pub filler: Alnum<59>,
}

impl Layout for CardLayout {
    const LEN: usize = 150;
    const NAME: &'static str = "CVACT02Y";
    const FIELDS: &'static [FieldSpec] = &[
        CARD_NUM,
        CARD_ACCT_ID,
        CARD_CVV_CD,
        CARD_EMBOSSED_NAME,
        CARD_EXPIRAION_DATE,
        CARD_ACTIVE_STATUS,
        FILLER,
    ];
    type Record = CardRecord;

    /// Reads `CARD-NUM` through `FILLER` (P-CVACT02Y-001 to P-CVACT02Y-007)
    /// [app/cpy/CVACT02Y.cpy:L5-11]: the `X(n)` fields and the `FILLER` as
    /// their stored bytes, `CARD-ACCT-ID` and `CARD-CVV-CD` as unsigned digits.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 150 bytes long.
    /// - [`CodecErrorKind::NotNumeric`] at `CARD-ACCT-ID` or `CARD-CVV-CD` when
    ///   a byte of the field is not `0`–`9`.
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
        let card = Card {
            card_num: CardNumber::from_alnum(field::alnum(Self::NAME, &rec, &CARD_NUM)?),
            card_acct_id: digit_id(&rec, &CARD_ACCT_ID)?,
            card_cvv_cd: digit_id(&rec, &CARD_CVV_CD)?,
            card_embossed_name: field::alnum(Self::NAME, &rec, &CARD_EMBOSSED_NAME)?,
            card_expiraion_date: field::alnum(Self::NAME, &rec, &CARD_EXPIRAION_DATE)?,
            card_active_status: field::alnum(Self::NAME, &rec, &CARD_ACTIVE_STATUS)?,
        };
        Ok(CardRecord {
            card,
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
        })
    }

    /// Writes `CARD-NUM` through `FILLER` (P-CVACT02Y-001 to P-CVACT02Y-007)
    /// [app/cpy/CVACT02Y.cpy:L5-11]: the `X(n)` fields and the `FILLER` from
    /// the bytes the record holds, `CARD-ACCT-ID` and `CARD-CVV-CD` as their
    /// zero-padded digits; then translates the 150 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let card = &record.card;
        let mut out = vec![b' '; Self::LEN];
        field::put_alnum(&mut out, &CARD_NUM, card.card_num.as_bytes());
        field::put_unsigned(&mut out, &CARD_ACCT_ID, card.card_acct_id.get());
        field::put_unsigned(&mut out, &CARD_CVV_CD, u64::from(card.card_cvv_cd.get()));
        field::put_alnum(
            &mut out,
            &CARD_EMBOSSED_NAME,
            card.card_embossed_name.as_bytes(),
        );
        field::put_alnum(
            &mut out,
            &CARD_EXPIRAION_DATE,
            card.card_expiraion_date.as_bytes(),
        );
        field::put_alnum(
            &mut out,
            &CARD_ACTIVE_STATUS,
            card.card_active_status.as_bytes(),
        );
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// Reads the unsigned `9(n)` field `spec` of `rec` into the identifier type
/// `T` through its `TryFrom<i64>`, the conversion its SQL column binds by.
///
/// `CARD-ACCT-ID` (P-CVACT02Y-002) [app/cpy/CVACT02Y.cpy:L6] and
/// `CARD-CVV-CD` (P-CVACT02Y-003) [app/cpy/CVACT02Y.cpy:L7] hold at most as
/// many digits as their types, so a field of digits always converts; a
/// rejected value is still reported, as [`CodecErrorKind::Value`] at the field.
fn digit_id<T>(rec: &[u8], spec: &FieldSpec) -> Result<T, CodecError>
where
    T: TryFrom<i64, Error = NumericError>,
{
    let digits = field::unsigned(CardLayout::NAME, rec, spec)?;
    i64::try_from(digits)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(T::try_from)
        .map_err(|e| {
            CodecError::new(CardLayout::NAME, CodecErrorKind::Value(e.to_string()))
                .at_field(spec.name, spec.offset)
        })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use carddemo_domain::entities::Card;
    use carddemo_domain::numeric::{AccountId, Alnum, CardNumber, Cvv, NumericError};

    use super::{CARD_ACCT_ID, CARD_CVV_CD, CardLayout, CardRecord};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, field};

    /// The ASCII `CARDDAT` fixture: 50 records of 150 bytes, each ending LF.
    const CARDDATA_ASCII: &str = include_str!("../../../../app/data/ASCII/carddata.txt");

    /// The EBCDIC `CARDDAT` fixture: the same 50 records as 150-byte CP037
    /// blocks.
    const CARDDATA_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.CARDDATA.PS");

    /// Record 1 of `carddata.txt` (P-CVACT02Y-009) [app/data/ASCII/carddata.txt:L1],
    /// built field by field: `CARD-NUM`, `CARD-ACCT-ID`, `CARD-CVV-CD` and
    /// `CARD-EMBOSSED-NAME` padded to offset 80, then `CARD-EXPIRAION-DATE`,
    /// `CARD-ACTIVE-STATUS` and the 59 spaces of `FILLER`.
    fn fixture_line_1() -> Vec<u8> {
        let mut line = concat!("0500024453765740", "00000000050", "747", "Aniya Von")
            .as_bytes()
            .to_vec();
        line.resize(80, b' ');
        line.extend_from_slice(b"2023-03-09Y");
        line.resize(CardLayout::LEN, b' ');
        line
    }

    /// A card no fixture holds: a one-digit verification code, an inactive
    /// status and a `FILLER` of `0` characters.
    fn hand_built() -> Result<CardRecord, NumericError> {
        Ok(CardRecord {
            card: Card {
                card_num: CardNumber::from_trimmed("4111111111111111")?,
                card_acct_id: AccountId::new(1)?,
                card_cvv_cd: Cvv::new(7)?,
                card_embossed_name: Alnum::move_from("Grace Hopper"),
                card_expiraion_date: Alnum::move_from("2030-12-31"),
                card_active_status: Alnum::move_from("N"),
            },
            filler: Alnum::from_array([b'0'; 59]),
        })
    }

    #[test]
    fn fields_tile_the_150_byte_record() {
        assert_eq!((CardLayout::NAME, CardLayout::LEN), ("CVACT02Y", 150));
        assert!(field::fields_are_contiguous(CardLayout::FIELDS, 150));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_150_bytes() {
        for len in [149, 151] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    CardLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CVACT02Y",
                        CodecErrorKind::Length {
                            expected: 150,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn fixture_record_1_decodes_to_its_fields_and_re_encodes() -> Result<(), CodecError> {
        let line = fixture_line_1();
        assert_eq!(
            CARDDATA_ASCII.lines().next().map(str::as_bytes),
            Some(&line[..])
        );
        let record = CardLayout::decode(&line, Charset::Ascii)?;
        let card = &record.card;
        assert_eq!(card.card_num.as_bytes(), b"0500024453765740");
        assert_eq!(card.card_acct_id.get(), 50);
        assert_eq!(card.card_cvv_cd.get(), 747);
        assert_eq!(card.card_embossed_name.trimmed(), "Aniya Von");
        assert_eq!(card.card_expiraion_date.as_bytes(), b"2023-03-09");
        assert_eq!(card.card_active_status.as_bytes(), b"Y");
        assert_eq!(record.filler, Alnum::spaces());
        assert_eq!(CardLayout::encode(&record, Charset::Ascii), line);
        Ok(())
    }

    #[test]
    fn a_hand_built_record_zero_pads_its_digits_and_round_trips() -> Result<(), Box<dyn Error>> {
        let record = hand_built()?;
        let ascii = CardLayout::encode(&record, Charset::Ascii);
        assert_eq!(ascii.get(16..27), Some(&b"00000000001"[..]));
        assert_eq!(ascii.get(27..30), Some(&b"007"[..]));
        assert_eq!(ascii.get(91..), Some(&[b'0'; 59][..]));
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = CardLayout::encode(&record, charset);
            assert_eq!(bytes.len(), 150, "{charset:?}");
            assert_eq!(CardLayout::decode(&bytes, charset)?, record, "{charset:?}");
        }
        Ok(())
    }

    #[test]
    fn a_non_digit_in_a_numeric_field_is_reported_at_that_field() {
        for (spec, at) in [(CARD_ACCT_ID, 20), (CARD_CVV_CD, 28)] {
            let mut line = fixture_line_1();
            if let Some(byte) = line.get_mut(at) {
                *byte = b'X';
            }
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                let stored = charset.from_internal(&line);
                assert_eq!(
                    CardLayout::decode(&stored, charset),
                    Err(
                        CodecError::new("CVACT02Y", CodecErrorKind::NotNumeric { byte: b'X' })
                            .at_field(spec.name, spec.offset)
                    ),
                    "{} in {charset:?}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn every_fixture_card_is_its_ebcdic_record_and_round_trips() -> Result<(), CodecError> {
        let ebcdic: Vec<&[u8]> = CARDDATA_EBCDIC.chunks(CardLayout::LEN).collect();
        let ascii: Vec<&[u8]> = CARDDATA_ASCII.lines().map(str::as_bytes).collect();
        assert_eq!((ascii.len(), ebcdic.len()), (50, 50));
        for (number, (line, block)) in (1..).zip(ascii.into_iter().zip(ebcdic)) {
            let record = CardLayout::decode(line, Charset::Ascii)?;
            assert_eq!(
                CardLayout::encode(&record, Charset::Ascii),
                line,
                "record {number}"
            );
            assert_eq!(
                CardLayout::decode(block, Charset::Ebcdic)?,
                record,
                "record {number}"
            );
            assert_eq!(
                CardLayout::encode(&record, Charset::Ebcdic),
                block,
                "record {number}"
            );
        }
        Ok(())
    }
}
