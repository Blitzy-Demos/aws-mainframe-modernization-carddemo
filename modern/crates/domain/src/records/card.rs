//! The card master record, `CVACT02Y` `CARD-RECORD`, 150 bytes (`app/cpy/CVACT02Y.cpy:4-11`),
//! rows CVACT02Y-001 to CVACT02Y-007 of `modern/audit/copybooks.md`.
//!
//! One record per card of the `CARDDATA` KSDS, keyed by `CARD-NUM` (`KEYS(16 0)`) with the
//! non-unique alternate index on `CARD-ACCT-ID` (`KEYS(11 16)`, `app/jcl/CARDFILE.jcl:54,83-86`).
//! The fixture is `app/data/ASCII/carddata.txt`, 50 full-length LF lines (FIXFRM-002), and
//! codec B stores the record in table `cards`, whose `card_ref` handle is not a copybook field
//! and has no place here (CVACT02Y-008).
//!
//! `CARD-NUM` and `CARD-CVV-CD` are protected (PIC-024): they are held as [`CardNumber`] and
//! [`Cvv`], so the derived `Debug` shows only `************` and the last four digits, and
//! `***`. The record has no `Display` or `Serialize`; [`FixedRecord::encode`] is the one place
//! that reads the raw values, through `expose()` (Cardholder Data Redaction, E32).

use copybook::{Error, FixedRecord, PicDate, PicU, PicX, RecordReader, RecordWriter};

use crate::sensitive::{CardNumber, Cvv};

/// One card, `CVACT02Y` `CARD-RECORD`: its fields in copybook order, each typed to its picture.
///
/// `Debug` is derived and redacts the card number and the CVV through their wrappers
/// (PIC-024). Every `PIC X(n)` field keeps its bytes, padding and `FILLER` included (PIC-001,
/// PIC-012), and the expiry date is typed, never text (Typed Dates and Timestamps).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardRecord {
    /// `CARD-NUM PIC X(16)`, offset 0, the record key (CVACT02Y-001); redacted in `Debug`.
    pub num: CardNumber,
    /// `CARD-ACCT-ID PIC 9(11)`, offset 16, the owning account and alternate-index key
    /// (CVACT02Y-002, PIC-003).
    pub acct_id: PicU<11>,
    /// `CARD-CVV-CD PIC 9(03)`, offset 27 (CVACT02Y-003); `***` in `Debug`.
    pub cvv_cd: Cvv,
    /// `CARD-EMBOSSED-NAME PIC X(50)`, offset 30 (CVACT02Y-004).
    pub embossed_name: PicX<50>,
    /// `CARD-EXPIRAION-DATE PIC X(10)`, offset 80: `None` for ten spaces, otherwise a
    /// `YYYY-MM-DD` calendar date (CVACT02Y-005, PIC-015, PIC-016). The COBOL spelling is kept,
    /// as in column `cards.card_expiraion_date`.
    pub expiraion_date: PicDate,
    /// `CARD-ACTIVE-STATUS PIC X(01)`, offset 90 (CVACT02Y-006).
    pub active_status: PicX<1>,
    /// `FILLER PIC X(59)`, offset 91, kept byte for byte (CVACT02Y-007, PIC-012).
    pub filler: PicX<59>,
}

// `TRAILING_FILLER` keeps its default of 0 although the record ends in `FILLER`: every
// `carddata.txt` body is 150 bytes (FIXFRM-002), and a short body outside `cardxref.txt` is a
// typed error, never padded (FIXFRM-011, R7).
impl FixedRecord for CardRecord {
    /// `RECLN 150` (`CVACT02Y.cpy:2`), the cluster's `RECORDSIZE(150 150)` (`CARDFILE.jcl:55`);
    /// the unit tests prove the seven fields fill it end to end.
    const WIDTH: usize = 150;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            num: CardNumber::new(r.pic_x("CARD-NUM")?),
            acct_id: r.pic_u("CARD-ACCT-ID")?,
            cvv_cd: Cvv::new(r.pic_u("CARD-CVV-CD")?),
            embossed_name: r.pic_x("CARD-EMBOSSED-NAME")?,
            expiraion_date: r.date("CARD-EXPIRAION-DATE")?,
            active_status: r.pic_x("CARD-ACTIVE-STATUS")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(self.num.expose())
            .pic_u(self.acct_id)
            .pic_u(*self.cvv_cd.expose())
            .pic_x(&self.embossed_name)
            .date(self.expiraion_date)
            .pic_x(&self.active_status)
            .pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{
        Error, ErrorKind, FixedRecord, Framed, LineFraming, PicDate, PicU, PicX, Terminator, lines,
    };

    use super::CardRecord;
    use crate::sensitive::{CardNumber, Cvv};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The fields of `CARD-RECORD` as `modern/audit/copybooks.md` gives them (CVACT02Y-001 to
    /// CVACT02Y-007): copybook name, offset, width, and the error class of a `0x01` byte at the
    /// field's first position.
    const LAYOUT: [(&str, usize, usize, ErrorKind); 7] = [
        ("CARD-NUM", 0, 16, ErrorKind::NonPrintable),
        ("CARD-ACCT-ID", 16, 11, ErrorKind::NonDigit),
        ("CARD-CVV-CD", 27, 3, ErrorKind::NonDigit),
        ("CARD-EMBOSSED-NAME", 30, 50, ErrorKind::NonPrintable),
        ("CARD-EXPIRAION-DATE", 80, 10, ErrorKind::InvalidDate),
        ("CARD-ACTIVE-STATUS", 90, 1, ErrorKind::NonPrintable),
        ("FILLER", 91, 59, ErrorKind::NonPrintable),
    ];

    /// Line 1 of `carddata.txt` without its LF (`carddata.txt:1`): card number, account
    /// `00000000050`, CVV, embossed name, expiry `2023-03-09`, status `Y` and 59 spaces of
    /// `FILLER`. Each continuation line starts a field.
    const LINE_1: &[u8] = b"0500024453765740\
        00000000050\
        747\
        Aniya Von                                         \
        2023-03-09\
        Y                                                           ";

    /// `CARD-NUM` of [`LINE_1`].
    const LINE_1_CARD: &str = "0500024453765740";

    /// `CARD-CVV-CD` of [`LINE_1`].
    const LINE_1_CVV: &str = "747";

    /// The record [`LINE_1`] holds, built from typed values rather than decoded.
    fn line_1_record() -> Result<CardRecord, Box<dyn StdError>> {
        Ok(CardRecord {
            num: CardNumber::parse(LINE_1_CARD)?,
            acct_id: PicU::new(50)?,
            cvv_cd: Cvv::new(PicU::new(747)?),
            embossed_name: PicX::from_text("Aniya Von")?,
            expiraion_date: PicDate::new(Some(
                NaiveDate::from_ymd_opt(2023, 3, 9).ok_or("2023-03-09 is a calendar date")?,
            ))?,
            active_status: PicX::from_text("Y")?,
            filler: PicX::spaces(),
        })
    }

    // audit: CVACT02Y-001
    // audit: CVACT02Y-002
    // audit: CVACT02Y-003
    // audit: CVACT02Y-004
    // audit: CVACT02Y-005
    // audit: CVACT02Y-006
    // audit: CVACT02Y-007
    // audit: FIXFRM-002
    #[test]
    fn carddata_line_1_decodes_to_its_fields_and_re_encodes_unchanged() -> TestResult {
        assert_eq!(LINE_1.len(), CardRecord::WIDTH);
        let record = CardRecord::decode(LINE_1)?;
        assert_eq!(record, line_1_record()?);
        assert!(record.filler.is_spaces());
        // Compared by equality, so a failure never prints the card number or CVV.
        assert!(
            record.encode() == LINE_1,
            "line 1 must re-encode to its own bytes"
        );
        Ok(())
    }

    // audit: CVACT02Y-001
    // audit: CVACT02Y-003
    // audit: PIC-024
    // audit: FIXFRM-021
    #[test]
    fn debug_shows_only_the_redacted_card_number_and_cvv() -> TestResult {
        let text = format!("{:?}", CardRecord::decode(LINE_1)?);
        assert!(
            !text.contains(LINE_1_CARD),
            "Debug must not show the card number"
        );
        assert!(!text.contains(LINE_1_CVV), "Debug must not show the CVV");
        assert!(text.contains("num: ************5740"));
        assert!(text.contains("cvv_cd: ***"));
        Ok(())
    }

    // audit: PIC-013
    // audit: PIC-014
    // audit: FIXFRM-002
    #[test]
    fn framed_line_keeps_its_lf_and_a_short_body_is_a_typed_error() -> TestResult {
        let mut line = LINE_1.to_vec();
        line.push(b'\n');
        let mut split = lines(&line);
        let framed = Framed::<CardRecord>::decode(split.next().ok_or("one line")?)?;
        assert!(split.next().is_none());
        assert_eq!(
            framed.framing,
            LineFraming {
                terminator: Terminator::Lf,
                body_len: CardRecord::WIDTH,
            }
        );
        assert!(
            framed.encode() == line,
            "the framed line must re-encode with its LF"
        );

        // A body that stops where `FILLER` begins is not padded: only `cardxref.txt` is short.
        let short = LINE_1.get(..91).ok_or("91 bytes")?;
        let mut split = lines(short);
        let rejected = Framed::<CardRecord>::decode(split.next().ok_or("one line")?);
        assert_eq!(
            rejected.err(),
            Some(Error::new(
                "record",
                0,
                CardRecord::WIDTH,
                ErrorKind::ShortBodyNotFiller {
                    expected: CardRecord::WIDTH,
                    actual: 91,
                },
            ))
        );
        Ok(())
    }

    #[test]
    fn any_other_length_is_a_record_width_error() {
        for len in [0, 1, 91, 149, 151, 300] {
            let bytes: Vec<u8> = LINE_1.iter().copied().cycle().take(len).collect();
            assert_eq!(
                CardRecord::decode(&bytes).err(),
                Some(Error::new(
                    "record",
                    0,
                    CardRecord::WIDTH,
                    ErrorKind::WidthMismatch {
                        expected: CardRecord::WIDTH,
                        actual: len,
                    },
                )),
                "length {len}",
            );
        }
    }

    // audit: CVACT02Y-007
    #[test]
    fn the_seven_fields_fill_the_150_byte_record_end_to_end() {
        let end = LAYOUT.iter().fold(0, |start, &(name, offset, width, _)| {
            assert_eq!(
                offset, start,
                "{name} must start where the previous field ends"
            );
            start + width
        });
        assert_eq!(end, CardRecord::WIDTH);
    }

    // audit: PIC-002
    // audit: PIC-004
    // audit: PIC-016
    #[test]
    fn a_bad_first_byte_names_each_field_its_offset_and_width() -> TestResult {
        for (name, offset, width, kind) in LAYOUT {
            let mut bytes = LINE_1.to_vec();
            *bytes.get_mut(offset).ok_or("offset inside the record")? = 0x01;
            assert_eq!(
                CardRecord::decode(&bytes).err(),
                Some(Error::new(name, offset, width, kind)),
            );
        }
        Ok(())
    }
}
