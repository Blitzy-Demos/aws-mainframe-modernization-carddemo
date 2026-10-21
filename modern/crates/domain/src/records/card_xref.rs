//! `CVACT03Y` `CARD-XREF-RECORD`, the card cross-reference: 50 bytes linking a card number to
//! its customer and account (`app/cpy/CVACT03Y.cpy:4-8`, CVACT03Y-001 to CVACT03Y-004).
//!
//! The record is that of the KSDS `CARDXREF`, keyed by `XREF-CARD-NUM` (`KEYS(16 0)`,
//! `RECORDSIZE(50 50)`, `app/jcl/XREFFILE.jcl:43-44`) with a non-unique alternate index on
//! `XREF-ACCT-ID` (`KEYS(11,25)`, `XREFFILE.jcl:72-74`). It becomes table `card_xref`, with the
//! alternate index replaced by an index on `acct_id`.
//!
//! # Short fixture bodies
//!
//! Every line of `app/data/ASCII/cardxref.txt` is a 36-byte body ended by LF: card, customer and
//! account, with the trailing `FILLER PIC X(14)` absent (E04, FIXFRM-003). The record declares
//! that `FILLER` as [`FixedRecord::TRAILING_FILLER`], so `copybook::Framed::decode` supplies the
//! 14 bytes as spaces before calling [`CardXrefRecord::decode`] and keeps `body_len` 36, and
//! `Framed::encode` cuts the 50 encoded bytes back to those 36 (PIC-013, FIXFRM-011). This
//! module therefore always sees the whole 50-byte record: a direct `decode` of a shorter slice is
//! a width error, and a body that ends inside a data field is rejected by the framing (R7).
//!
//! # Cardholder data
//!
//! `XREF-CARD-NUM` is held as [`CardNumber`], so the derived `Debug` of a record shows only the
//! redacted `************` + last four form. [`CardXrefRecord::encode`] is the one place here
//! that reads the raw bytes, through [`CardNumber::expose`] (PIC-024, Cardholder Data
//! Redaction).

use copybook::{Error, FixedRecord, PicU, PicX, RecordReader, RecordWriter};

use crate::sensitive::CardNumber;

/// One `CVACT03Y` `CARD-XREF-RECORD`, its fields in copybook order.
///
/// Every field keeps the bytes it was decoded from: the card number and `FILLER` verbatim, the
/// two identifiers through their digit count (PIC-001, PIC-003, PIC-012).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardXrefRecord {
    /// `XREF-CARD-NUM PIC X(16)`, offset 0, the record key (CVACT03Y-001). Its 16 bytes are kept
    /// verbatim and checked as printable ASCII only, never as digits (PIC-001, PIC-002,
    /// FIXFRM-019); `Debug` shows the redacted form (PIC-024).
    pub card_num: CardNumber,
    /// `XREF-CUST-ID PIC 9(09)`, offset 16 (CVACT03Y-002); a non-digit byte, a space included,
    /// is a typed error (PIC-004).
    pub cust_id: PicU<9>,
    /// `XREF-ACCT-ID PIC 9(11)`, offset 25, the alternate-index key (CVACT03Y-003); 64-bit, as
    /// every identifier of 10 or more digits is (PIC-023, Checked Integer Narrowing).
    pub acct_id: PicU<11>,
    /// `FILLER PIC X(14)`, offset 36 (CVACT03Y-004): kept byte for byte, whatever it holds
    /// (PIC-012). Absent from every fixture line, so it decodes there as 14 spaces (PIC-013).
    pub filler: PicX<14>,
}

impl FixedRecord for CardXrefRecord {
    /// The copybook's `RECLN 50` and the cluster's `RECORDSIZE(50 50)` (`XREFFILE.jcl:44`).
    const WIDTH: usize = 50;

    /// `FILLER PIC X(14)` at offset 36, the record's last field (CVACT03Y-004).
    ///
    /// The 50 lines of `cardxref.txt` end where this `FILLER` begins, 36 bytes into the record
    /// (E04, FIXFRM-003, FIXFRM-011). Declaring it lets the framing accept those bodies, supply
    /// the missing tail as spaces and re-emit each line at its own 36 bytes, and nothing shorter:
    /// a body that ends inside `XREF-ACCT-ID` or an earlier field is a typed error (PIC-013, R7).
    const TRAILING_FILLER: usize = 14;

    /// Decodes the four fields of exactly [`WIDTH`](Self::WIDTH) bytes, left to right.
    ///
    /// A slice of any other length, the 36-byte fixture body included, is
    /// `ErrorKind::WidthMismatch` for field `"record"`: completing a short body is the framing's
    /// job. A field that fails is reported with its name, offset and width, never its bytes.
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            card_num: CardNumber::new(r.pic_x("XREF-CARD-NUM")?),
            cust_id: r.pic_u("XREF-CUST-ID")?,
            acct_id: r.pic_u("XREF-ACCT-ID")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    /// Encodes all 50 bytes, `FILLER` included; `Framed::encode` cuts a short fixture line back
    /// to its `body_len`.
    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(self.card_num.expose())
            .pic_u(self.cust_id)
            .pic_u(self.acct_id)
            .pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{
        Error, ErrorKind, FixedRecord, Framed, Line, LineFraming, PicU, PicX, Terminator, lines,
    };

    use super::CardXrefRecord;
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Line 1 of `cardxref.txt` with its LF: card, customer `000000050` and account
    /// `00000000050` in a 36-byte body, the trailing `FILLER` absent (FIXFRM-011).
    const LINE: &[u8] = b"050002445376574000000005000000000050\n";

    /// `XREF-CARD-NUM` of [`LINE`].
    const CARD: [u8; 16] = *b"0500024453765740";

    /// Fourteen spaces, the `FILLER` the framing supplies for a fixture line.
    const BLANK_FILLER: [u8; 14] = *b"              ";

    /// [`LINE`] without its LF.
    fn body() -> Result<&'static [u8], Box<dyn StdError>> {
        Ok(LINE.strip_suffix(b"\n").ok_or("line 1 ends with LF")?)
    }

    /// The 50 record bytes of [`LINE`] with `filler` in place of the absent `FILLER`.
    fn full_record(filler: &[u8; 14]) -> Result<Vec<u8>, Box<dyn StdError>> {
        Ok([body()?, filler.as_slice()].concat())
    }

    /// The record [`LINE`] decodes to.
    fn line_one() -> Result<CardXrefRecord, Box<dyn StdError>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(CARD)),
            cust_id: PicU::new(50)?,
            acct_id: PicU::new(50)?,
            filler: PicX::from_array(BLANK_FILLER),
        })
    }

    // audit: FIXFRM-003
    // audit: FIXFRM-011
    // audit: PIC-013
    // audit: CVACT03Y-004
    #[test]
    fn short_fixture_line_round_trips_through_framing() -> TestResult {
        let line = lines(LINE).next().ok_or("LINE holds one line")?;
        let framed = Framed::<CardXrefRecord>::decode(line)?;
        assert_eq!(framed.record, line_one()?);
        assert_eq!(
            framed.framing,
            LineFraming {
                terminator: Terminator::Lf,
                body_len: 36,
            }
        );
        // Byte comparisons use `assert!`, whose failure message never prints the card number.
        assert!(
            framed.encode() == LINE,
            "encode re-emits the 37 bytes of the line"
        );
        assert!(
            framed.try_encode()? == LINE,
            "try_encode accepts the blank FILLER beyond body_len"
        );
        Ok(())
    }

    // audit: CVACT03Y-001
    // audit: CVACT03Y-002
    // audit: CVACT03Y-003
    // audit: CVACT03Y-004
    // audit: PIC-012
    #[test]
    fn full_record_round_trips_with_its_filler_bytes_kept() -> TestResult {
        for filler in [BLANK_FILLER, *b"ABCDEFGHIJKLMN"] {
            let bytes = full_record(&filler)?;
            let record = CardXrefRecord::decode(&bytes)?;
            assert_eq!(
                record,
                CardXrefRecord {
                    filler: PicX::from_array(filler),
                    ..line_one()?
                }
            );
            assert!(
                record.encode() == bytes,
                "encode returns the decoded 50 bytes"
            );
        }
        Ok(())
    }

    // audit: CVACT03Y-001
    // audit: CVACT03Y-002
    // audit: CVACT03Y-003
    // audit: PIC-003
    // audit: PIC-023
    #[test]
    fn fields_encode_at_their_offsets_with_leading_zeros_and_eleven_digit_accounts() -> TestResult {
        let record = CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(*b"0000111122223333")),
            cust_id: PicU::new(7_654_321)?,
            acct_id: PicU::new(99_999_999_999)?,
            filler: PicX::from_array(*b"ABCDEFGHIJKLMN"),
        };
        let bytes: &[u8] = b"000011112222333300765432199999999999ABCDEFGHIJKLMN";
        assert_eq!(bytes.len(), CardXrefRecord::WIDTH);
        assert!(
            record.encode() == bytes,
            "fields at offsets 0, 16, 25 and 36"
        );
        assert_eq!(CardXrefRecord::decode(bytes)?, record);
        Ok(())
    }

    // audit: PIC-024
    #[test]
    fn debug_shows_only_the_redacted_card_number() -> TestResult {
        let rendered = format!("{:?}", line_one()?);
        assert!(
            !rendered.contains(std::str::from_utf8(&CARD)?),
            "Debug of a record must not disclose XREF-CARD-NUM"
        );
        assert!(rendered.contains("************5740"));
        Ok(())
    }

    // audit: PIC-013
    #[test]
    fn framing_rejects_a_body_ending_inside_a_data_field_or_past_the_record() -> TestResult {
        let short = body()?.get(..35).ok_or("LINE's body has 35 bytes")?;
        assert_eq!(
            Framed::<CardXrefRecord>::decode(Line::new(short, Terminator::Lf)).err(),
            Some(Error::new(
                "record",
                0,
                50,
                ErrorKind::ShortBodyNotFiller {
                    expected: 50,
                    actual: 35,
                },
            )),
        );
        let long = [full_record(&BLANK_FILLER)?.as_slice(), b" "].concat();
        assert_eq!(
            Framed::<CardXrefRecord>::decode(Line::new(&long, Terminator::Lf)).err(),
            Some(Error::new(
                "record",
                0,
                50,
                ErrorKind::WidthMismatch {
                    expected: 50,
                    actual: 51,
                },
            )),
        );
        Ok(())
    }

    #[test]
    fn direct_decode_requires_exactly_fifty_bytes() -> TestResult {
        for len in [0, 35, 36, 49, 51] {
            let bytes = full_record(&BLANK_FILLER)?
                .into_iter()
                .cycle()
                .take(len)
                .collect::<Vec<u8>>();
            assert_eq!(
                CardXrefRecord::decode(&bytes).err(),
                Some(Error::new(
                    "record",
                    0,
                    50,
                    ErrorKind::WidthMismatch {
                        expected: 50,
                        actual: len,
                    },
                )),
            );
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-004
    #[test]
    fn a_failing_field_is_reported_by_name_offset_and_width() {
        let card = CARD.as_slice();
        let cust: &[u8] = b"000000050";
        let acct: &[u8] = b"00000000050";
        let cases: [([&[u8]; 3], Error); 4] = [
            (
                [b"050002445376574\t", cust, acct],
                Error::new("XREF-CARD-NUM", 0, 16, ErrorKind::NonPrintable),
            ),
            (
                [card, b"00000005 ", acct],
                Error::new("XREF-CUST-ID", 16, 9, ErrorKind::NonDigit),
            ),
            (
                [card, b"-00000050", acct],
                Error::new("XREF-CUST-ID", 16, 9, ErrorKind::NonDigit),
            ),
            (
                [card, cust, b"0000000005A"],
                Error::new("XREF-ACCT-ID", 25, 11, ErrorKind::NonDigit),
            ),
        ];
        for ([card_num, cust_id, acct_id], error) in cases {
            let bytes = [card_num, cust_id, acct_id, BLANK_FILLER.as_slice()].concat();
            assert_eq!(CardXrefRecord::decode(&bytes).err(), Some(error));
        }
    }
}
