//! `CCPAURQY`, the storage layout of a CP00 authorization request: 153 bytes, 18 fields
//! (CCPAURQY-001 to CCPAURQY-018).
//!
//! `COPAUA0C` reads the request into `PENDING-AUTH-REQUEST`, which copies this layout, by
//! `UNSTRING … DELIMITED BY ','` and computes the amount with `FUNCTION NUMVAL`
//! (`COPAUA0C.cbl:354-379`; COPAUA0C-002 to COPAUA0C-017). That comma-delimited wire body is
//! parsed by `domain::messaging::copaua0c::parse_request`, which builds an [`AuthRequestRecord`]
//! and owns every wire rule: the 16-digit card-number check, the field errors and their `05`/`9000`
//! decline, and the dead-lettering of an unparseable card number (E31). Nothing here reads the
//! wire text. The byte form below is the copybook storage, with `PA-RQ-TRANSACTION-AMT` in its
//! numeric-edited picture `+9(10).99` (PIC-022), so the request has the fixed-width codec every
//! layout of `modern/audit/copybooks.md` has. The layout has no table and no fixture.
//!
//! Its decoder is total: for any slice of any length it returns a record or a typed
//! [`copybook::Error`] naming the field, its offset and its width, never a panic and never a byte
//! of the input (Total Parsers Over Untrusted Input). `PA-RQ-CARD-NUM` is a [`CardNumber`], so the
//! derived `Debug` shows only its redacted form (PIC-024, Cardholder Data Redaction).

use copybook::{Error, FixedRecord, PicU, PicX, RecordReader, RecordWriter};

use super::{edited_amount, put_edited_amount};
use crate::money::Money;
use crate::sensitive::CardNumber;

/// `PENDING-AUTH-REQUEST`, the 18 fields of `CCPAURQY` in copybook order (`CCPAURQY.cpy:19-36`).
///
/// This is the copybook layout, not the wire message. The CP00 request body is 18 comma-delimited
/// tokens, which `domain::messaging::copaua0c::parse_request` moves into these fields under the
/// `UNSTRING` and `NUMVAL` rules of `modern/audit/messaging/COPAUA0C.md` (COPAUA0C-002 to
/// COPAUA0C-017). Offsets in the field documentation are storage offsets, not wire positions.
///
/// Alphanumeric fields keep their bytes verbatim, padding included (PIC-001); the two `PIC 9(n)`
/// fields restore their leading zeros on encode (PIC-003). `auth_date`, `auth_time` and
/// `card_expiry_date` stay text: Typed Dates and Timestamps covers only the `X(10)` dates and the
/// `X(26)` timestamps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthRequestRecord {
    /// `PA-RQ-AUTH-DATE PIC X(06)`, offset 0, 6 bytes; token 1, carried into `PA-AUTH-ORIG-DATE`
    /// (CCPAURQY-001).
    pub auth_date: PicX<6>,
    /// `PA-RQ-AUTH-TIME PIC X(06)`, offset 6, 6 bytes; token 2, carried into
    /// `PA-AUTH-ORIG-TIME` and `PA-RL-AUTH-ID-CODE` (CCPAURQY-002).
    pub auth_time: PicX<6>,
    /// `PA-RQ-CARD-NUM PIC X(16)`, offset 12, 16 bytes; token 3, the cross-reference lookup key,
    /// carried into `PA-RL-CARD-NUM` and `PA-CARD-NUM`. Sensitive: `Debug` shows twelve `*` and
    /// the last four characters (CCPAURQY-003, PIC-024).
    pub card_num: CardNumber,
    /// `PA-RQ-AUTH-TYPE PIC X(04)`, offset 28, 4 bytes; token 4, carried into `PA-AUTH-TYPE`
    /// (CCPAURQY-004).
    pub auth_type: PicX<4>,
    /// `PA-RQ-CARD-EXPIRY-DATE PIC X(04)`, offset 32, 4 bytes; token 5, carried into
    /// `PA-CARD-EXPIRY-DATE`, kept as received (CCPAURQY-005).
    pub card_expiry_date: PicX<4>,
    /// `PA-RQ-MESSAGE-TYPE PIC X(06)`, offset 36, 6 bytes; token 6, carried into
    /// `PA-MESSAGE-TYPE` (CCPAURQY-006).
    pub message_type: PicX<6>,
    /// `PA-RQ-MESSAGE-SOURCE PIC X(06)`, offset 42, 6 bytes; token 7, carried into
    /// `PA-MESSAGE-SOURCE` (CCPAURQY-007).
    pub message_source: PicX<6>,
    /// `PA-RQ-PROCESSING-CODE PIC 9(06)`, offset 48, 6 bytes; token 8, carried into
    /// `PA-PROCESSING-CODE` (CCPAURQY-008, COPAUA0C-004).
    pub processing_code: PicU<6>,
    /// `PA-RQ-TRANSACTION-AMT PIC +9(10).99`, offset 54, 14 bytes: sign `+` or `-`, ten digits,
    /// `.`, two digits. Token 9 reaches it through `WS-TRANSACTION-AMT-AN` and `FUNCTION NUMVAL`,
    /// never through this picture; carried into `WS-TRANSACTION-AMT` and `PA-TRANSACTION-AMT`
    /// (CCPAURQY-009, PIC-022, COPAUA0C-014).
    pub transaction_amt: Money<10>,
    /// `PA-RQ-MERCHANT-CATAGORY-CODE PIC X(04)`, offset 68, 4 bytes, the COBOL spelling kept;
    /// token 10, carried into `PA-MERCHANT-CATAGORY-CODE` (CCPAURQY-010).
    pub merchant_catagory_code: PicX<4>,
    /// `PA-RQ-ACQR-COUNTRY-CODE PIC X(03)`, offset 72, 3 bytes; token 11, carried into
    /// `PA-ACQR-COUNTRY-CODE` (CCPAURQY-011).
    pub acqr_country_code: PicX<3>,
    /// `PA-RQ-POS-ENTRY-MODE PIC 9(02)`, offset 75, 2 bytes; token 12, carried into
    /// `PA-POS-ENTRY-MODE` (CCPAURQY-012, COPAUA0C-004).
    pub pos_entry_mode: PicU<2>,
    /// `PA-RQ-MERCHANT-ID PIC X(15)`, offset 77, 15 bytes; token 13, carried into
    /// `PA-MERCHANT-ID` (CCPAURQY-013).
    pub merchant_id: PicX<15>,
    /// `PA-RQ-MERCHANT-NAME PIC X(22)`, offset 92, 22 bytes; token 14, carried into
    /// `PA-MERCHANT-NAME` (CCPAURQY-014).
    pub merchant_name: PicX<22>,
    /// `PA-RQ-MERCHANT-CITY PIC X(13)`, offset 114, 13 bytes; token 15, carried into
    /// `PA-MERCHANT-CITY` (CCPAURQY-015).
    pub merchant_city: PicX<13>,
    /// `PA-RQ-MERCHANT-STATE PIC X(02)`, offset 127, 2 bytes; token 16, carried into
    /// `PA-MERCHANT-STATE` (CCPAURQY-016).
    pub merchant_state: PicX<2>,
    /// `PA-RQ-MERCHANT-ZIP PIC X(09)`, offset 129, 9 bytes; token 17, carried into
    /// `PA-MERCHANT-ZIP` (CCPAURQY-017).
    pub merchant_zip: PicX<9>,
    /// `PA-RQ-TRANSACTION-ID PIC X(15)`, offset 138, 15 bytes; token 18, carried into
    /// `PA-RL-TRANSACTION-ID` and `PA-TRANSACTION-ID` (CCPAURQY-018).
    pub transaction_id: PicX<15>,
}

impl FixedRecord for AuthRequestRecord {
    /// 153 bytes, the sum of the 18 field widths (CCPAURQY-001 to CCPAURQY-018); the `const`
    /// check after this implementation adds them up.
    const WIDTH: usize = 153;

    /// Decodes the 18 fields in copybook order.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`](copybook::ErrorKind::WidthMismatch) for field `"record"`
    /// when `bytes` is not 153 bytes long;
    /// [`ErrorKind::NonPrintable`](copybook::ErrorKind::NonPrintable) for a byte outside
    /// `0x20`–`0x7E` in a `PIC X(n)` field (PIC-002);
    /// [`ErrorKind::NonDigit`](copybook::ErrorKind::NonDigit) for a non-digit in a `PIC 9(n)`
    /// field (PIC-004) or a bad sign, digit or point byte in `PA-RQ-TRANSACTION-AMT`;
    /// [`ErrorKind::NegativeZero`](copybook::ErrorKind::NegativeZero) for an amount of
    /// `-0000000000.00`, which no [`Money`] value re-encodes and which is never normalised to
    /// zero (R7, PIC-022). Each names the field, its offset and its width.
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            auth_date: r.pic_x("PA-RQ-AUTH-DATE")?,
            auth_time: r.pic_x("PA-RQ-AUTH-TIME")?,
            card_num: CardNumber::new(r.pic_x("PA-RQ-CARD-NUM")?),
            auth_type: r.pic_x("PA-RQ-AUTH-TYPE")?,
            card_expiry_date: r.pic_x("PA-RQ-CARD-EXPIRY-DATE")?,
            message_type: r.pic_x("PA-RQ-MESSAGE-TYPE")?,
            message_source: r.pic_x("PA-RQ-MESSAGE-SOURCE")?,
            processing_code: r.pic_u("PA-RQ-PROCESSING-CODE")?,
            transaction_amt: edited_amount(&mut r, "PA-RQ-TRANSACTION-AMT")?,
            merchant_catagory_code: r.pic_x("PA-RQ-MERCHANT-CATAGORY-CODE")?,
            acqr_country_code: r.pic_x("PA-RQ-ACQR-COUNTRY-CODE")?,
            pos_entry_mode: r.pic_u("PA-RQ-POS-ENTRY-MODE")?,
            merchant_id: r.pic_x("PA-RQ-MERCHANT-ID")?,
            merchant_name: r.pic_x("PA-RQ-MERCHANT-NAME")?,
            merchant_city: r.pic_x("PA-RQ-MERCHANT-CITY")?,
            merchant_state: r.pic_x("PA-RQ-MERCHANT-STATE")?,
            merchant_zip: r.pic_x("PA-RQ-MERCHANT-ZIP")?,
            transaction_id: r.pic_x("PA-RQ-TRANSACTION-ID")?,
        };
        r.finish()?;
        Ok(record)
    }

    /// Encodes exactly 153 bytes, the card number through [`CardNumber::expose`], the codec's
    /// raw path, and the amount in `PIC +9(10).99` (PIC-022).
    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.auth_date)
            .pic_x(&self.auth_time)
            .pic_x(self.card_num.expose())
            .pic_x(&self.auth_type)
            .pic_x(&self.card_expiry_date)
            .pic_x(&self.message_type)
            .pic_x(&self.message_source)
            .pic_u(self.processing_code);
        put_edited_amount(&mut w, &self.transaction_amt)
            .pic_x(&self.merchant_catagory_code)
            .pic_x(&self.acqr_country_code)
            .pic_u(self.pos_entry_mode)
            .pic_x(&self.merchant_id)
            .pic_x(&self.merchant_name)
            .pic_x(&self.merchant_city)
            .pic_x(&self.merchant_state)
            .pic_x(&self.merchant_zip)
            .pic_x(&self.transaction_id);
        w.finish()
    }
}

// The widths of `CCPAURQY.cpy:19-36` in copybook order, `PIC +9(10).99` counting 14 bytes, add up
// to the record width the decoder checks.
const _: () = assert!(
    6 + 6 + 16 + 4 + 4 + 6 + 6 + 6 + 14 + 4 + 3 + 2 + 15 + 22 + 13 + 2 + 9 + 15
        == <AuthRequestRecord as FixedRecord>::WIDTH,
    "CCPAURQY field widths must add up to 153 bytes"
);

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, PicU, PicX};

    use super::AuthRequestRecord;
    use crate::money::{Decimal, Money};
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A synthetic `PA-RQ-CARD-NUM`, in no fixture.
    const CARD: &[u8; 16] = b"1234567890123456";

    /// Index of `PA-RQ-TRANSACTION-AMT` in [`FIELDS`].
    const AMOUNT: usize = 8;

    /// A hand-built request image, one entry per field in copybook order, widths as
    /// `CCPAURQY.cpy:19-36` declares them.
    const FIELDS: [&[u8]; 18] = [
        b"220618",
        b"143012",
        CARD,
        b"0100",
        b"0927",
        b"0200  ",
        b"POS   ",
        b"000000",
        b"+0000000100.00",
        b"5411",
        b"840",
        b"05",
        b"MERCHANT0000001",
        b"CORNER GROCERY        ",
        b"SEATTLE      ",
        b"WA",
        b"98101    ",
        b"TXN000000000001",
    ];

    /// The image of [`FIELDS`] with field `index` replaced by `bytes`.
    fn image_with(index: usize, bytes: &[u8]) -> Vec<u8> {
        FIELDS
            .iter()
            .enumerate()
            .flat_map(|(i, field)| if i == index { bytes } else { *field })
            .copied()
            .collect()
    }

    /// The record [`FIELDS`] holds, with `cents` hundredths in `PA-RQ-TRANSACTION-AMT`.
    fn expected(cents: i64) -> Result<AuthRequestRecord, Box<dyn StdError>> {
        Ok(AuthRequestRecord {
            auth_date: PicX::from_text("220618")?,
            auth_time: PicX::from_text("143012")?,
            card_num: CardNumber::new(PicX::from_array(*CARD)),
            auth_type: PicX::from_text("0100")?,
            card_expiry_date: PicX::from_text("0927")?,
            message_type: PicX::from_text("0200")?,
            message_source: PicX::from_text("POS")?,
            processing_code: PicU::new(0)?,
            transaction_amt: Money::new(Decimal::new(cents, 2))?,
            merchant_catagory_code: PicX::from_text("5411")?,
            acqr_country_code: PicX::from_text("840")?,
            pos_entry_mode: PicU::new(5)?,
            merchant_id: PicX::from_text("MERCHANT0000001")?,
            merchant_name: PicX::from_text("CORNER GROCERY")?,
            merchant_city: PicX::from_text("SEATTLE")?,
            merchant_state: PicX::from_text("WA")?,
            merchant_zip: PicX::from_text("98101")?,
            transaction_id: PicX::from_text("TXN000000000001")?,
        })
    }

    // audit: CCPAURQY-009
    // audit: PIC-022
    #[test]
    fn request_image_round_trips_every_field_and_amount_sign() -> TestResult {
        let cases: [(&[u8; 14], i64); 4] = [
            (b"+0000000100.00", 10_000),
            (b"-0000000012.34", -1_234),
            (b"+0000000000.00", 0),
            (b"-9999999999.99", -999_999_999_999),
        ];
        for (amount, cents) in cases {
            let image = image_with(AMOUNT, amount);
            assert_eq!(image.len(), AuthRequestRecord::WIDTH);
            let record = AuthRequestRecord::decode(&image)?;
            assert_eq!(record, expected(cents)?);
            assert_eq!(record.encode(), image);
        }
        Ok(())
    }

    #[test]
    fn malformed_fields_are_typed_errors_at_their_offsets() {
        let cases: [(usize, &[u8], Error); 9] = [
            (
                0,
                b"220\x0018",
                Error::new("PA-RQ-AUTH-DATE", 0, 6, ErrorKind::NonPrintable),
            ),
            (
                2,
                b"123456789012345\x80",
                Error::new("PA-RQ-CARD-NUM", 12, 16, ErrorKind::NonPrintable),
            ),
            (
                7,
                b"00A000",
                Error::new("PA-RQ-PROCESSING-CODE", 48, 6, ErrorKind::NonDigit),
            ),
            (
                AMOUNT,
                b"-0000000000.00",
                Error::new("PA-RQ-TRANSACTION-AMT", 54, 14, ErrorKind::NegativeZero),
            ),
            (
                AMOUNT,
                b"*0000000100.00",
                Error::new("PA-RQ-TRANSACTION-AMT", 54, 14, ErrorKind::NonDigit),
            ),
            (
                AMOUNT,
                b" 0000000100.00",
                Error::new("PA-RQ-TRANSACTION-AMT", 54, 14, ErrorKind::NonDigit),
            ),
            (
                AMOUNT,
                b"+0000000100,00",
                Error::new("PA-RQ-TRANSACTION-AMT", 54, 14, ErrorKind::NonDigit),
            ),
            (
                11,
                b" 5",
                Error::new("PA-RQ-POS-ENTRY-MODE", 75, 2, ErrorKind::NonDigit),
            ),
            (
                17,
                b"TXN00000000000\x7F",
                Error::new("PA-RQ-TRANSACTION-ID", 138, 15, ErrorKind::NonPrintable),
            ),
        ];
        for (index, bytes, error) in cases {
            assert_eq!(
                AuthRequestRecord::decode(&image_with(index, bytes)),
                Err(error)
            );
        }
    }

    #[test]
    fn any_other_length_is_a_record_width_error() {
        for len in [0, 1, 152, 154, 1_024] {
            let mut bytes = image_with(AMOUNT, b"+0000000100.00");
            bytes.resize(len, b' ');
            assert_eq!(
                AuthRequestRecord::decode(&bytes),
                Err(Error::new(
                    "record",
                    0,
                    AuthRequestRecord::WIDTH,
                    ErrorKind::WidthMismatch {
                        expected: AuthRequestRecord::WIDTH,
                        actual: len,
                    },
                )),
            );
        }
    }

    // audit: CCPAURQY-003
    // audit: PIC-024
    #[test]
    fn debug_and_errors_never_show_the_full_card_number() -> TestResult {
        let full = std::str::from_utf8(CARD)?;
        let record = AuthRequestRecord::decode(&image_with(AMOUNT, b"+0000000100.00"))?;
        let debug = format!("{record:?}");
        assert!(!debug.contains(full), "Debug shows the full card number");
        assert!(debug.contains("card_num: ************3456"));

        let Err(error) = AuthRequestRecord::decode(&image_with(2, b"123456789012345\x80")) else {
            return Err("a non-printable card-number byte must be rejected".into());
        };
        let shown = format!("{error} {error:?}");
        assert!(shown.contains("PA-RQ-CARD-NUM"));
        assert!(
            !shown.contains("123456789012345"),
            "an error shows card bytes"
        );
        Ok(())
    }
}
