//! `CCPAURLY`, the CP00 reply storage `PENDING-AUTH-RESPONSE`: six fields, 57 bytes
//! (`COPAUA0C.cbl:181-182`; CCPAURLY-001 to CCPAURLY-006).
//!
//! [`AuthReply`] is the typed outcome of one CP00 authorization decision, the fields
//! `6000-MAKE-DECISION` moves into `PA-RL-*` (`COPAUA0C.cbl:660-718`). Its byte form is the
//! copybook's storage layout, which no queue carries: the reply body the requester receives is
//! the comma-delimited text `crate::messaging::copaua0c` builds from these fields (COPAUA0C-060
//! to COPAUA0C-065).

use copybook::{Error, FixedRecord, PicX, RecordReader, RecordWriter};

use super::{EDITED_AMOUNT, edited_amount, put_edited_amount};
use crate::money::Money;
use crate::sensitive::CardNumber;

/// The CP00 reply, `CCPAURLY`, in copybook order.
///
/// The field order is the order of the reply body, field order from `CCPAURLY` and bytes from the
/// emitted `STRING` (`COPAUA0C.cbl:722-731`; E03). `crate::messaging::copaua0c` produces those
/// bytes, not this struct: it writes the card field in its 16-character redacted form
/// (COPAUA0C-061, E32), the amount through `WS-APPROVED-AMT-DIS PIC -zzzzzzzzz9.99` rather than
/// `PIC +9(10).99` (COPAUA0C-064, PIC-022), a comma after every field, and the 63 written bytes
/// only, because the body length the COBOL sends is `HALTED(H6)` (COPAUA0C-065). This struct
/// holds the decision those bytes report and makes no decision itself.
///
/// `card_num` is a [`CardNumber`], so the derived `Debug` shows the redacted form only (PIC-024,
/// Cardholder Data Redaction). The struct has no `Display` and no `Serialize`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthReply {
    /// `PA-RL-CARD-NUM PIC X(16)`, offset 0, 16 bytes: the request's card number, moved from
    /// `PA-RQ-CARD-NUM` (`COPAUA0C.cbl:660`; CCPAURLY-001).
    pub card_num: CardNumber,
    /// `PA-RL-TRANSACTION-ID PIC X(15)`, offset 16, 15 bytes: moved from `PA-RQ-TRANSACTION-ID`,
    /// the request's field 18 (`COPAUA0C.cbl:661`; CCPAURLY-002).
    pub transaction_id: PicX<15>,
    /// `PA-RL-AUTH-ID-CODE PIC X(06)`, offset 31, 6 bytes: moved from `PA-RQ-AUTH-TIME`, the
    /// request's field 2 (`COPAUA0C.cbl:662`; CCPAURLY-003).
    pub auth_id_code: PicX<6>,
    /// `PA-RL-AUTH-RESP-CODE PIC X(02)`, offset 37, 2 bytes: [`AuthReply::APPROVED`] or
    /// [`AuthReply::DECLINED`] (`COPAUA0C.cbl:688,693`; CCPAURLY-004, COPAUA0C-044).
    pub auth_resp_code: PicX<2>,
    /// `PA-RL-AUTH-RESP-REASON PIC X(04)`, offset 39, 4 bytes: `0000` for an approval, otherwise
    /// the decline's reason code (`COPAUA0C.cbl:698-717`; CCPAURLY-005, COPAUA0C-045).
    pub auth_resp_reason: PicX<4>,
    /// `PA-RL-APPROVED-AMT PIC +9(10).99`, offset 43, 14 bytes: the request amount on an
    /// approval and 0 on a decline (`COPAUA0C.cbl:689,694`; CCPAURLY-006, COPAUA0C-044).
    pub approved_amt: Money<10>,
}

impl AuthReply {
    /// `PA-RL-AUTH-RESP-CODE` of an approval, `MOVE '00'` (`COPAUA0C.cbl:693`; CCPAURLY-004,
    /// COPAUA0C-044).
    pub const APPROVED: &[u8; 2] = b"00";

    /// `PA-RL-AUTH-RESP-CODE` of a decline, `MOVE '05'` (`COPAUA0C.cbl:688`; CCPAURLY-004,
    /// COPAUA0C-044, COPAUA0C-012).
    pub const DECLINED: &[u8; 2] = b"05";
}

// The six field widths of CCPAURLY-001 to CCPAURLY-006 add up to the record width.
const _: () = assert!(
    PicX::<16>::WIDTH
        + PicX::<15>::WIDTH
        + PicX::<6>::WIDTH
        + PicX::<2>::WIDTH
        + PicX::<4>::WIDTH
        + EDITED_AMOUNT.width()
        == AuthReply::WIDTH
);

impl FixedRecord for AuthReply {
    const WIDTH: usize = 57;

    /// Decodes the six fields in copybook order (CCPAURLY-001 to CCPAURLY-006).
    ///
    /// # Errors
    ///
    /// [`copybook::ErrorKind::WidthMismatch`] for field `"record"` unless `bytes` is 57 bytes
    /// long; [`copybook::ErrorKind::NonPrintable`] for a byte outside printable ASCII in one of the
    /// five `PIC X(n)` fields (PIC-002); for `PA-RL-APPROVED-AMT`, the errors of the
    /// `PIC +9(10).99` image (PIC-022): [`copybook::ErrorKind::NonDigit`] for a bad sign, digit or
    /// point byte and [`copybook::ErrorKind::NegativeZero`] for `-0000000000.00`. Each error names
    /// the field, its offset and its width, never its bytes.
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            card_num: CardNumber::new(r.pic_x("PA-RL-CARD-NUM")?),
            transaction_id: r.pic_x("PA-RL-TRANSACTION-ID")?,
            auth_id_code: r.pic_x("PA-RL-AUTH-ID-CODE")?,
            auth_resp_code: r.pic_x("PA-RL-AUTH-RESP-CODE")?,
            auth_resp_reason: r.pic_x("PA-RL-AUTH-RESP-REASON")?,
            approved_amt: edited_amount(&mut r, "PA-RL-APPROVED-AMT")?,
        };
        r.finish()?;
        Ok(record)
    }

    /// Encodes the six fields in copybook order, exactly 57 bytes: the card number's stored
    /// bytes, the four `PIC X(n)` fields unchanged and the amount as `PIC +9(10).99`.
    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(self.card_num.expose())
            .pic_x(&self.transaction_id)
            .pic_x(&self.auth_id_code)
            .pic_x(&self.auth_resp_code)
            .pic_x(&self.auth_resp_reason);
        put_edited_amount(&mut w, &self.approved_amt);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, PicX};

    use super::AuthReply;
    use crate::money::{Decimal, Money};
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// One reply: response code, reason, amount image and the amount in hundredths.
    type ReplyCase = ([u8; 2], [u8; 4], &'static [u8; 14], i64);

    /// A synthetic card number, no fixture's.
    const CARD: &str = "4000123412345740";

    /// The redacted form of [`CARD`].
    const CARD_REDACTED: &str = "************5740";

    /// `CARD`, then the transaction ID, auth-ID code, response code, reason and amount images,
    /// the 57 bytes of one reply.
    fn reply_bytes(code: [u8; 2], reason: [u8; 4], amount: &[u8; 14]) -> Vec<u8> {
        let mut bytes = CARD.as_bytes().to_vec();
        bytes.extend_from_slice(b"TXN-0000000042 ");
        bytes.extend_from_slice(b"192233");
        bytes.extend_from_slice(&code);
        bytes.extend_from_slice(&reason);
        bytes.extend_from_slice(amount);
        bytes
    }

    /// The reply [`reply_bytes`] lays out, built from typed values.
    fn reply(code: [u8; 2], reason: [u8; 4], cents: i64) -> Result<AuthReply, Box<dyn StdError>> {
        Ok(AuthReply {
            card_num: CardNumber::parse(CARD)?,
            transaction_id: PicX::from_array(*b"TXN-0000000042 "),
            auth_id_code: PicX::from_array(*b"192233"),
            auth_resp_code: PicX::from_array(code),
            auth_resp_reason: PicX::from_array(reason),
            approved_amt: Money::new(Decimal::new(cents, 2))?,
        })
    }

    // audit: CCPAURLY-001
    // audit: CCPAURLY-002
    // audit: CCPAURLY-003
    // audit: CCPAURLY-004
    // audit: CCPAURLY-005
    // audit: CCPAURLY-006
    #[test]
    fn approved_and_declined_replies_round_trip() -> TestResult {
        let cases: [ReplyCase; 2] = [
            (*AuthReply::APPROVED, *b"0000", b"+0000000100.00", 10_000),
            (*AuthReply::DECLINED, *b"4100", b"+0000000000.00", 0),
        ];
        for (code, reason, amount, cents) in cases {
            let bytes = reply_bytes(code, reason, amount);
            assert_eq!(bytes.len(), AuthReply::WIDTH);
            let expected = reply(code, reason, cents)?;
            let decoded = AuthReply::decode(&bytes)?;
            assert_eq!(decoded, expected);
            assert_eq!(decoded.card_num.expose().as_bytes(), CARD.as_bytes());
            assert_eq!(decoded.auth_resp_code.as_bytes(), &code);
            assert_eq!(decoded.encode(), bytes);
            assert_eq!(expected.encode(), bytes);
        }
        Ok(())
    }

    #[test]
    fn response_codes_are_the_moved_literals() {
        assert_eq!(AuthReply::APPROVED, b"00");
        assert_eq!(AuthReply::DECLINED, b"05");
    }

    // audit: PIC-024
    #[test]
    fn debug_shows_only_the_redacted_card_number() -> TestResult {
        let rendered = format!("{:?}", reply(*AuthReply::APPROVED, *b"0000", 10_000)?);
        assert!(!rendered.contains(CARD));
        assert!(rendered.contains(CARD_REDACTED));
        Ok(())
    }

    #[test]
    fn slices_of_any_other_length_are_width_errors() -> TestResult {
        let bytes = reply_bytes(*AuthReply::APPROVED, *b"0000", b"+0000000100.00");
        let (_, short) = bytes.split_last().ok_or("empty reply")?;
        let mut long = bytes.clone();
        long.push(b' ');
        for slice in [b"".as_slice(), short, long.as_slice()] {
            assert!(matches!(
                AuthReply::decode(slice),
                Err(error) if error == Error::new(
                    "record",
                    0,
                    AuthReply::WIDTH,
                    ErrorKind::WidthMismatch {
                        expected: AuthReply::WIDTH,
                        actual: slice.len(),
                    },
                )
            ));
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-022
    #[test]
    fn field_errors_name_the_field_offset_and_width() {
        let cases: [(Vec<u8>, Error); 3] = [
            (
                reply_bytes(*b"0\t", *b"0000", b"+0000000100.00"),
                Error::new("PA-RL-AUTH-RESP-CODE", 37, 2, ErrorKind::NonPrintable),
            ),
            (
                reply_bytes(*AuthReply::APPROVED, *b"0000", b"*0000000100.00"),
                Error::new("PA-RL-APPROVED-AMT", 43, 14, ErrorKind::NonDigit),
            ),
            (
                reply_bytes(*AuthReply::DECLINED, *b"4100", b"-0000000000.00"),
                Error::new("PA-RL-APPROVED-AMT", 43, 14, ErrorKind::NegativeZero),
            ),
        ];
        for (bytes, expected) in cases {
            assert_eq!(AuthReply::decode(&bytes), Err(expected));
        }
    }
}
