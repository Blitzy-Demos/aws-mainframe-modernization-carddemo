//! `CVACT03Y` `CARD-XREF-RECORD`: the 50-byte `CCXREF` card cross-reference
//! record [app/cpy/CVACT03Y.cpy:L4-8], `RECLN 50` [app/cpy/CVACT03Y.cpy:L2].
//!
//! The ASCII fixture is `app/data/ASCII/cardxref.txt` (50 records); its EBCDIC
//! counterpart is `app/data/EBCDIC/AWS.M2.CARDDEMO.CARDXREF.PS`, the same 50
//! records as 50-byte CP037 blocks. `batch seed` decodes each ASCII record with
//! [`CardXrefLayout`] and loads its [`CardXref`] into `card_xref`
//! (P-CVACT03Y-005).
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `XREF-CARD-NUM` | `X(16)` | 0 | 16 | P-CVACT03Y-001 | `CardNumber` |
//! | `XREF-CUST-ID` | `9(09)` | 16 | 9 | P-CVACT03Y-002 | `CustomerId` |
//! | `XREF-ACCT-ID` | `9(11)` | 25 | 11 | P-CVACT03Y-003 | `AccountId` |
//! | `FILLER` | `X(14)` | 36 | 14 | P-CVACT03Y-004 | `Alnum<14>`, raw |
//!
//! # Short ASCII records (E-04)
//!
//! Every `cardxref.txt` line is stored as the first 36 of the record's 50
//! bytes, then `LF` (P-CVACT03Y-007). [`crate::framing`] pads each stored body
//! with spaces to 50 bytes before decoding and writes back only the stored 36
//! bytes on encoding, refusing with `TruncationLoss` should any dropped byte not
//! be a space. The ASCII `FILLER` therefore decodes as 14 spaces, and this
//! layout only ever sees full 50-byte records: a short record handed to
//! [`CardXrefLayout::decode`] is a length error, never padded here. The EBCDIC
//! file holds full 50-byte records whose `FILLER` is `0x40`, the CP037 space,
//! so both files decode to the same records.

use carddemo_domain::entities::CardXref;
use carddemo_domain::numeric::{AccountId, Alnum, CardNumber, CustomerId, NumericError};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec};

/// `CVACT03Y` `XREF-CARD-NUM PIC X(16)`, the `CCXREF` key — P-CVACT03Y-001
/// [app/cpy/CVACT03Y.cpy:L5].
pub const XREF_CARD_NUM: FieldSpec = FieldSpec::new("XREF-CARD-NUM", 0, 16, FieldKind::Alnum);

/// `CVACT03Y` `XREF-CUST-ID PIC 9(09)` — P-CVACT03Y-002
/// [app/cpy/CVACT03Y.cpy:L6].
pub const XREF_CUST_ID: FieldSpec = FieldSpec::new("XREF-CUST-ID", 16, 9, FieldKind::Unsigned);

/// `CVACT03Y` `XREF-ACCT-ID PIC 9(11)`, the key of the alternate index
/// `CXACAIX` — P-CVACT03Y-003 [app/cpy/CVACT03Y.cpy:L7].
pub const XREF_ACCT_ID: FieldSpec = FieldSpec::new("XREF-ACCT-ID", 25, 11, FieldKind::Unsigned);

/// `CVACT03Y` `FILLER PIC X(14)`, kept raw — P-CVACT03Y-004
/// [app/cpy/CVACT03Y.cpy:L8].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 36, 14, FieldKind::Filler);

/// The `CVACT03Y` `CARD-XREF-RECORD` layout, 50 bytes
/// [app/cpy/CVACT03Y.cpy:L4-8].
pub struct CardXrefLayout;

/// One decoded `CCXREF` record: the typed cross-reference and the stored
/// `FILLER` bytes.
///
/// The record has no signed field, so no value needs preserving beyond its
/// typed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardXrefRecord {
    /// `XREF-CARD-NUM`, `XREF-CUST-ID` and `XREF-ACCT-ID` — P-CVACT03Y-001 to
    /// P-CVACT03Y-003 [app/cpy/CVACT03Y.cpy:L5-7].
    pub xref: CardXref,
    /// `FILLER PIC X(14)`, its stored bytes — P-CVACT03Y-004
    /// [app/cpy/CVACT03Y.cpy:L8]. Spaces in both fixture files.
    pub filler: Alnum<14>,
}

impl Layout for CardXrefLayout {
    const LEN: usize = 50;
    const NAME: &'static str = "CVACT03Y";
    const FIELDS: &'static [FieldSpec] = &[XREF_CARD_NUM, XREF_CUST_ID, XREF_ACCT_ID, FILLER];
    type Record = CardXrefRecord;

    /// Reads `XREF-CARD-NUM` through `FILLER` (P-CVACT03Y-001 to
    /// P-CVACT03Y-004) [app/cpy/CVACT03Y.cpy:L5-8]: the card number and the
    /// filler as their stored bytes, the customer and account numbers as
    /// unsigned display digits.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 50 bytes long,
    ///   which includes a 36-byte `cardxref.txt` line not yet padded by
    ///   [`crate::framing`].
    /// - [`CodecErrorKind::NotNumeric`] at `XREF-CUST-ID` or `XREF-ACCT-ID`
    ///   for a byte other than `0`–`9`.
    /// - [`CodecErrorKind::Value`] at the field whose domain type rejects the
    ///   decoded value.
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
        let xref_card_num = CardNumber::from_alnum(field::alnum(Self::NAME, &rec, &XREF_CARD_NUM)?);
        let xref_cust_id = u32::try_from(field::unsigned(Self::NAME, &rec, &XREF_CUST_ID)?)
            .map_err(|_| NumericError::OutOfRange)
            .and_then(CustomerId::new)
            .map_err(|e| rejected(&XREF_CUST_ID, &e))?;
        let xref_acct_id = AccountId::new(field::unsigned(Self::NAME, &rec, &XREF_ACCT_ID)?)
            .map_err(|e| rejected(&XREF_ACCT_ID, &e))?;
        Ok(CardXrefRecord {
            xref: CardXref {
                xref_card_num,
                xref_cust_id,
                xref_acct_id,
            },
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
        })
    }

    /// Writes `XREF-CARD-NUM` through `FILLER` (P-CVACT03Y-001 to
    /// P-CVACT03Y-004) [app/cpy/CVACT03Y.cpy:L5-8]: the card number and the
    /// filler from the bytes the record holds, the customer and account
    /// numbers as zero-padded digits, then translates the 50 bytes to
    /// `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let mut out = vec![b' '; Self::LEN];
        field::put_alnum(
            &mut out,
            &XREF_CARD_NUM,
            record.xref.xref_card_num.as_bytes(),
        );
        field::put_unsigned(
            &mut out,
            &XREF_CUST_ID,
            u64::from(record.xref.xref_cust_id.get()),
        );
        field::put_unsigned(&mut out, &XREF_ACCT_ID, record.xref.xref_acct_id.get());
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// The error for a decoded value of the field `spec` that its domain type
/// rejects, carrying the domain error text.
fn rejected(spec: &FieldSpec, err: &NumericError) -> CodecError {
    CodecError::new(CardXrefLayout::NAME, CodecErrorKind::Value(err.to_string()))
        .at_field(spec.name, spec.offset)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use carddemo_domain::entities::CardXref;
    use carddemo_domain::numeric::{AccountId, Alnum, CardNumber, CustomerId};

    use super::{CardXrefLayout, CardXrefRecord};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, field};

    /// The ASCII fixture: 50 records, each stored as 36 bytes then `LF`.
    const CARDXREF_ASCII: &[u8] = include_bytes!("../../../../app/data/ASCII/cardxref.txt");

    /// The EBCDIC fixture: the same 50 records as 50-byte CP037 blocks.
    const CARDXREF_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.CARDXREF.PS");

    /// The stored body of `cardxref.txt` line 1, without its `LF`.
    fn first_ascii_line() -> &'static [u8] {
        CARDXREF_ASCII
            .split(|&b| b == b'\n')
            .next()
            .unwrap_or_default()
    }

    #[test]
    fn fields_tile_the_50_byte_record() {
        assert_eq!(
            (CardXrefLayout::NAME, CardXrefLayout::LEN),
            ("CVACT03Y", 50)
        );
        assert!(field::fields_are_contiguous(CardXrefLayout::FIELDS, 50));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_50_bytes() {
        let short = [b'0'; 49];
        let long = [b'0'; 51];
        // A stored cardxref.txt line reaches the layout only once framing has
        // padded it, so the bare 36 bytes are a length error too.
        let unpadded = first_ascii_line();
        assert_eq!(unpadded.len(), 36);
        for bytes in [&short[..], &long[..], unpadded] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    CardXrefLayout::decode(bytes, charset),
                    Err(CodecError::new(
                        "CVACT03Y",
                        CodecErrorKind::Length {
                            expected: 50,
                            actual: bytes.len(),
                        },
                    )),
                    "{} bytes in {charset:?}",
                    bytes.len()
                );
            }
        }
    }

    /// P-CVACT03Y-006: `cardxref.txt` line 1, padded as framing pads it, and
    /// the first EBCDIC record are the same cross-reference, each re-encoding
    /// to its own bytes.
    #[test]
    fn first_fixture_record_decodes_in_both_charsets() -> Result<(), Box<dyn Error>> {
        let line = first_ascii_line();
        assert_eq!(line, b"050002445376574000000005000000000050");
        let mut padded = line.to_vec();
        padded.resize(50, b' ');

        let record = CardXrefLayout::decode(&padded, Charset::Ascii)?;
        assert_eq!(
            record.xref.xref_card_num,
            CardNumber::from_trimmed("0500024453765740")?
        );
        assert_eq!(record.xref.xref_cust_id, CustomerId::new(50)?);
        assert_eq!(record.xref.xref_acct_id, AccountId::new(50)?);
        assert_eq!(record.filler, Alnum::spaces());
        assert_eq!(CardXrefLayout::encode(&record, Charset::Ascii), padded);

        let stored = CARDXREF_EBCDIC.get(..50).unwrap_or_default();
        assert_eq!(stored.get(36..), Some(&[0x40; 14][..]));
        assert_eq!(CardXrefLayout::decode(stored, Charset::Ebcdic)?, record);
        assert_eq!(CardXrefLayout::encode(&record, Charset::Ebcdic), stored);
        Ok(())
    }

    #[test]
    fn a_non_digit_is_reported_at_its_field() {
        let mut bytes = concat!("0500024453765740", "000000050", "000000000A0")
            .as_bytes()
            .to_vec();
        bytes.resize(50, b' ');
        assert_eq!(
            CardXrefLayout::decode(&bytes, Charset::Ascii),
            Err(
                CodecError::new("CVACT03Y", CodecErrorKind::NotNumeric { byte: b'A' })
                    .at_field("XREF-ACCT-ID", 25)
            )
        );
        bytes[20] = b' ';
        assert_eq!(
            CardXrefLayout::decode(&bytes, Charset::Ascii),
            Err(
                CodecError::new("CVACT03Y", CodecErrorKind::NotNumeric { byte: b' ' })
                    .at_field("XREF-CUST-ID", 16)
            )
        );
    }

    #[test]
    fn a_hand_built_record_round_trips_in_both_charsets() -> Result<(), Box<dyn Error>> {
        let record = CardXrefRecord {
            xref: CardXref {
                xref_card_num: CardNumber::move_from("4111-1111 xY"),
                xref_cust_id: CustomerId::new(999_999_999)?,
                xref_acct_id: AccountId::new(7)?,
            },
            filler: Alnum::from_array(*b"0123456789abcd"),
        };
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = CardXrefLayout::encode(&record, charset);
            assert_eq!(bytes.len(), 50, "{charset:?}");
            assert_eq!(
                CardXrefLayout::decode(&bytes, charset)?,
                record,
                "{charset:?}"
            );
        }
        assert_eq!(
            CardXrefLayout::encode(&record, Charset::Ascii),
            concat!(
                "4111-1111 xY    ",
                "999999999",
                "00000000007",
                "0123456789abcd"
            )
            .as_bytes()
        );
        Ok(())
    }
}
