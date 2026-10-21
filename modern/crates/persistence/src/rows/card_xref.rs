//! Codec B for table `card_xref`: the `CVACT03Y` `CARD-XREF-RECORD` as a row of migration 0001
//! (`app/cpy/CVACT03Y.cpy:4-8`, CVACT03Y-001 to CVACT03Y-004).
//!
//! | Field | Picture | Column | Row field |
//! |---|---|---|---|
//! | `XREF-CARD-NUM` | `X(16)`, the KSDS key `KEYS(16 0)` (`XREFFILE.jcl:43`) | `card_num CHAR(16)`, the primary key | `String` of 16 ASCII bytes (CVACT03Y-001, PIC-001) |
//! | `XREF-CUST-ID` | `9(09)` | `cust_id BIGINT`, checked `0 .. 999999999` | `i64` (CVACT03Y-002, PIC-003) |
//! | `XREF-ACCT-ID` | `9(11)`, the AIX key `KEYS(11,25)` (`XREFFILE.jcl:72-74`) | `acct_id BIGINT`, checked `0 .. 99999999999`, indexed by `card_xref_acct_id_idx` | `i64` (CVACT03Y-003, PIC-003, PIC-023) |
//! | `FILLER` | `X(14)` | `filler CHAR(14)` | `String` of 14 ASCII bytes (CVACT03Y-004, PIC-012) |
//!
//! # Short fixture bodies
//!
//! Every `cardxref.txt` line is a 36-byte body that ends where `FILLER` begins (E04, FIXFRM-003).
//! Codec A's `Framed::decode` supplies the absent 14 bytes as spaces and records `body_len` 36
//! in the line's [`copybook::LineFraming`] (FIXFRM-011, PIC-013), so this row stores 14 spaces in
//! `filler` and nothing about the length. The framing describes the line, not the record: no
//! column holds it, and the d9 round trip passes it alongside the row unchanged, re-encoding the
//! record read back inside the envelope codec A decoded, which yields the original 36 bytes and
//! terminator (PIC-014).
//!
//! # Cardholder data
//!
//! `card_num` holds the full card number. [`CardXrefRow::from_record`] reads it only through
//! [`CardNumber::expose`], and [`CardXrefRow::into_record`] wraps it straight back into codec A's
//! [`CardNumber::new`], so it leaves this module only as a bind value. The row derives no
//! `Debug`, `Clone` or `Serialize`, and every error names the column and the failure class, never
//! the value (PIC-024, FIXFRM-021, Cardholder Data Redaction).

use domain::records::card_xref::CardXrefRecord;
use domain::sensitive::CardNumber;

use super::{i64_to_pic_u, pic_u_to_i64, pic_x_to_string, string_to_pic_x};
use crate::error::PersistenceError;

/// Column of `XREF-CARD-NUM`.
const CARD_NUM: &str = "card_num";
/// Column of `XREF-CUST-ID`.
const CUST_ID: &str = "cust_id";
/// Column of `XREF-ACCT-ID`.
const ACCT_ID: &str = "acct_id";
/// Column of the record's `FILLER`.
const FILLER: &str = "filler";

/// One `card_xref` row, its fields the table's columns in migration order.
///
/// Fields are crate-only: `repos` builds rows for its binds and reads them from its queries, and
/// every other caller converts through [`CardXrefRow::from_record`] and
/// [`CardXrefRow::into_record`].
pub struct CardXrefRow {
    /// `card_num CHAR(16)`: the 16 bytes of `XREF-CARD-NUM`, kept verbatim (CVACT03Y-001).
    pub(crate) card_num: String,
    /// `cust_id BIGINT`: `XREF-CUST-ID PIC 9(09)` (CVACT03Y-002).
    pub(crate) cust_id: i64,
    /// `acct_id BIGINT`: `XREF-ACCT-ID PIC 9(11)`, 64-bit as every identifier of 10 or more
    /// digits is (CVACT03Y-003, PIC-023, Checked Integer Narrowing).
    pub(crate) acct_id: i64,
    /// `filler CHAR(14)`: the record's `FILLER` byte for byte, 14 spaces on every fixture line
    /// (CVACT03Y-004, PIC-012, PIC-013).
    pub(crate) filler: String,
}

// Both conversions are public while the columns stay crate-only, so integration tests and
// callers outside `repos` can run codec B without reaching the column values.
impl CardXrefRow {
    /// The row that stores `record`: every byte of the card number and `FILLER`, and both
    /// identifiers at full width.
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for column `card_num`
    /// or `filler` when the field holds a NUL or non-ASCII byte, which no `CHAR(n)` column stores
    /// byte for byte (PIC-002). Codec A admits only printable ASCII, so a decoded record never
    /// fails here.
    pub fn from_record(record: &CardXrefRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            card_num: pic_x_to_string(CARD_NUM, record.card_num.expose())?,
            cust_id: pic_u_to_i64(CUST_ID, record.cust_id)?,
            acct_id: pic_u_to_i64(ACCT_ID, record.acct_id)?,
            filler: pic_x_to_string(FILLER, &record.filler)?,
        })
    }

    /// The record this row stores, equal to the one [`CardXrefRow::from_record`] was given.
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the first column, in migration order, whose value
    /// does not fit its picture; nothing is truncated, padded or defaulted (R7):
    /// - `card_num` or `filler`: `InvalidByte` for a NUL or non-ASCII character, `WidthMismatch`
    ///   unless the text is exactly 16 or 14 bytes long;
    /// - `cust_id` or `acct_id`: `OutOfRange` for a negative value or one above `999999999` or
    ///   `99999999999`.
    pub fn into_record(self) -> Result<CardXrefRecord, PersistenceError> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(string_to_pic_x::<16>(CARD_NUM, &self.card_num)?),
            cust_id: i64_to_pic_u::<9>(CUST_ID, self.cust_id)?,
            acct_id: i64_to_pic_u::<11>(ACCT_ID, self.acct_id)?,
            filler: string_to_pic_x::<14>(FILLER, &self.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use copybook::framing::{decode_all, encode_all};
    use copybook::{Framed, PicU, PicX, Terminator, lines};
    use domain::records::card_xref::CardXrefRecord;
    use domain::sensitive::CardNumber;

    use super::CardXrefRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The 36-byte body of `cardxref.txt` line 1: card, customer `000000050` and account
    /// `00000000050`, the trailing `FILLER` absent.
    const BODY: &[u8] = b"050002445376574000000005000000000050";

    /// `XREF-CARD-NUM` of [`BODY`].
    const CARD: &str = "0500024453765740";

    /// The 14 spaces codec A supplies for the absent `FILLER`.
    const BLANK_FILLER: &str = "              ";

    /// `app/data/ASCII/cardxref.txt`, read in place.
    ///
    /// Resolved from this crate's manifest directory rather than `test_support::fixtures`: a unit
    /// test inside the library needs no container harness, and the fixture path is the same.
    fn fixture() -> Result<Vec<u8>, Box<dyn StdError>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/cardxref.txt");
        Ok(std::fs::read(path)?)
    }

    /// The row of [`BODY`] as codec B writes it.
    fn valid_row() -> CardXrefRow {
        CardXrefRow {
            card_num: CARD.to_owned(),
            cust_id: 50,
            acct_id: 50,
            filler: BLANK_FILLER.to_owned(),
        }
    }

    /// Whether `text` holds four consecutive digits, the shortest run that could be part of a
    /// card number; no error text has one.
    fn has_digit_run(text: &str) -> bool {
        text.as_bytes()
            .windows(4)
            .any(|run| run.iter().all(u8::is_ascii_digit))
    }

    // audit: FIXFRM-011
    // audit: PIC-013
    // audit: PIC-014
    // audit: CVACT03Y-004
    #[test]
    fn short_line_round_trips_inside_its_unchanged_framing() -> TestResult {
        for terminator in [Terminator::Lf, Terminator::CrLf] {
            let bytes = [BODY, terminator.as_bytes()].concat();
            let line = lines(&bytes).next().ok_or("the input holds one line")?;
            let framed = Framed::<CardXrefRecord>::decode(line)?;
            assert_eq!(framed.framing.terminator, terminator);
            assert_eq!(framed.framing.body_len, 36);

            let row = CardXrefRow::from_record(&framed.record)?;
            // Byte comparisons use `assert!`, whose failure message never prints the card number.
            assert!(
                row.card_num == CARD,
                "card_num holds XREF-CARD-NUM verbatim"
            );
            assert_eq!((row.cust_id, row.acct_id), (50, 50));
            assert_eq!(row.filler, BLANK_FILLER);

            let back = Framed {
                record: row.into_record()?,
                framing: framed.framing,
            };
            assert_eq!(back, framed);
            assert!(
                back.encode() == bytes,
                "the 36 bytes and terminator come back"
            );
            assert!(
                back.try_encode()? == bytes,
                "the blank FILLER beyond body_len is accepted"
            );
        }
        Ok(())
    }

    // audit: FIXFRM-003
    #[test]
    fn every_fixture_line_round_trips_byte_for_byte() -> TestResult {
        let input = fixture()?;
        let decoded = decode_all::<CardXrefRecord>(&input)?;
        assert_eq!(decoded.len(), 50);
        let mut back = Vec::with_capacity(decoded.len());
        for framed in &decoded {
            assert_eq!(framed.framing.body_len, 36);
            let row = CardXrefRow::from_record(&framed.record)?;
            assert_eq!(row.filler, BLANK_FILLER);
            assert_eq!(row.cust_id, row.acct_id);
            back.push(Framed {
                record: row.into_record()?,
                framing: framed.framing,
            });
        }
        assert_eq!(back, decoded);
        assert!(
            encode_all(&back) == input,
            "cardxref.txt comes back byte for byte"
        );
        Ok(())
    }

    // audit: CVACT03Y-001
    // audit: CVACT03Y-002
    // audit: CVACT03Y-003
    // audit: PIC-001
    // audit: PIC-012
    // audit: PIC-023
    #[test]
    fn widest_values_and_every_filler_byte_survive() -> TestResult {
        let cases: [([u8; 16], u64, u64, [u8; 14]); 3] = [
            (*b"0000000000000000", 0, 0, *b"              "),
            (
                *b"9999999999999999",
                PicU::<9>::MAX,
                PicU::<11>::MAX,
                *b"ABCDEFGHIJKLMN",
            ),
            (*b" AB cd~{}|  12  ", 7_654_321, 1, *b" 0 0 0 0 0 0 0"),
        ];
        for (card, cust_id, acct_id, filler) in cases {
            let record = CardXrefRecord {
                card_num: CardNumber::new(PicX::from_array(card)),
                cust_id: PicU::new(cust_id)?,
                acct_id: PicU::new(acct_id)?,
                filler: PicX::from_array(filler),
            };
            let row = CardXrefRow::from_record(&record)?;
            assert!(row.card_num.as_bytes() == card, "card_num keeps every byte");
            assert_eq!(row.cust_id, i64::try_from(cust_id)?);
            assert_eq!(row.acct_id, i64::try_from(acct_id)?);
            assert_eq!(row.filler.as_bytes(), filler);
            assert_eq!(row.into_record()?, record);
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-003
    // audit: PIC-024
    #[test]
    fn stored_values_outside_their_picture_are_typed_errors() {
        let cases = [
            (
                CardXrefRow {
                    cust_id: 1_000_000_000,
                    ..valid_row()
                },
                "cust_id",
                DecodeKind::OutOfRange,
            ),
            (
                CardXrefRow {
                    cust_id: -1,
                    ..valid_row()
                },
                "cust_id",
                DecodeKind::OutOfRange,
            ),
            (
                CardXrefRow {
                    acct_id: 100_000_000_000,
                    ..valid_row()
                },
                "acct_id",
                DecodeKind::OutOfRange,
            ),
            (
                CardXrefRow {
                    acct_id: i64::MIN,
                    ..valid_row()
                },
                "acct_id",
                DecodeKind::OutOfRange,
            ),
            (
                CardXrefRow {
                    filler: " ".repeat(13),
                    ..valid_row()
                },
                "filler",
                DecodeKind::WidthMismatch {
                    expected: 14,
                    actual: 13,
                },
            ),
            (
                CardXrefRow {
                    filler: " ".repeat(15),
                    ..valid_row()
                },
                "filler",
                DecodeKind::WidthMismatch {
                    expected: 14,
                    actual: 15,
                },
            ),
            (
                CardXrefRow {
                    card_num: "050002445376574\0".to_owned(),
                    ..valid_row()
                },
                "card_num",
                DecodeKind::InvalidByte,
            ),
            (
                CardXrefRow {
                    card_num: "05000244537657\u{e9}".to_owned(),
                    ..valid_row()
                },
                "card_num",
                DecodeKind::InvalidByte,
            ),
            (
                CardXrefRow {
                    card_num: "050002445376574".to_owned(),
                    ..valid_row()
                },
                "card_num",
                DecodeKind::WidthMismatch {
                    expected: 16,
                    actual: 15,
                },
            ),
        ];
        for (row, column, kind) in cases {
            let error = row.into_record().err();
            assert_eq!(error, Some(PersistenceError::decode(column, kind)));
            for text in error.iter().flat_map(|e| [e.to_string(), format!("{e:?}")]) {
                assert!(!has_digit_run(&text), "{text}");
            }
        }
    }

    // audit: PIC-002
    // audit: PIC-024
    #[test]
    fn record_bytes_no_column_can_store_are_typed_errors() -> TestResult {
        let valid = valid_row().into_record()?;
        let cases = [
            (
                CardXrefRecord {
                    card_num: CardNumber::new(PicX::from_array(*b"050002445376574\0")),
                    ..valid.clone()
                },
                "card_num",
            ),
            (
                CardXrefRecord {
                    card_num: CardNumber::new(PicX::from_array(*b"0500024453765\xC3\xA90")),
                    ..valid.clone()
                },
                "card_num",
            ),
            (
                CardXrefRecord {
                    filler: PicX::from_array(*b"             \0"),
                    ..valid
                },
                "filler",
            ),
        ];
        for (record, column) in cases {
            let error = CardXrefRow::from_record(&record).err();
            assert_eq!(
                error,
                Some(PersistenceError::decode(column, DecodeKind::InvalidByte))
            );
            for text in error.iter().flat_map(|e| [e.to_string(), format!("{e:?}")]) {
                assert!(!has_digit_run(&text), "{text}");
            }
        }
        Ok(())
    }
}
