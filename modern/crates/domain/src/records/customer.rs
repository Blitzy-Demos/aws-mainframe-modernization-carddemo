//! `CVCUS01Y` `CUSTOMER-RECORD`, the 500-byte customer master record (`CVCUS01Y.cpy:4-23`;
//! CVCUS01Y-001 to CVCUS01Y-019).
//!
//! `custfile` loads the 50 records of `custdata.txt` through this layout into table `customers`,
//! keyed by `CUST-ID` as the KSDS `KEYS(9 0)` of `CUSTFILE.jcl:50` is, and the account, card and
//! authorization programs read it back from there (FIXFRM-004, FIXLOAD-004).
//!
//! Two fields are typed beyond their picture:
//!
//! - `CUST-SSN PIC 9(09)` is an [`Ssn`], so the derived `Debug` of a record prints only
//!   `***-**-` and the last four digits, and the record has no `Display` or `Serialize` through
//!   which the full value could leave (PIC-024, E32, Cardholder Data Redaction). The encoder reads
//!   the raw digits through [`Ssn::expose`] and nothing else does.
//! - `CUST-DOB-YYYY-MM-DD PIC X(10)` is a [`PicDate`], never text (PIC-015, PIC-016, Typed Dates
//!   and Timestamps).

use copybook::{Error, FixedRecord, PicDate, PicU, PicX, RecordReader, RecordWriter};

use crate::sensitive::Ssn;

/// Width of the trailing `FILLER PIC X(168)` (CVCUS01Y-019).
const FILLER_WIDTH: usize = 168;

/// Byte widths of the 19 fields in copybook order, `CUST-ID` (CVCUS01Y-001) to `FILLER`
/// (CVCUS01Y-019), as `CVCUS01Y.cpy:5-23` declares them.
const FIELD_WIDTHS: [usize; 19] = [
    9,              // CUST-ID
    25,             // CUST-FIRST-NAME
    25,             // CUST-MIDDLE-NAME
    25,             // CUST-LAST-NAME
    50,             // CUST-ADDR-LINE-1
    50,             // CUST-ADDR-LINE-2
    50,             // CUST-ADDR-LINE-3
    2,              // CUST-ADDR-STATE-CD
    3,              // CUST-ADDR-COUNTRY-CD
    10,             // CUST-ADDR-ZIP
    15,             // CUST-PHONE-NUM-1
    15,             // CUST-PHONE-NUM-2
    9,              // CUST-SSN
    20,             // CUST-GOVT-ISSUED-ID
    PicDate::WIDTH, // CUST-DOB-YYYY-MM-DD
    10,             // CUST-EFT-ACCOUNT-ID
    1,              // CUST-PRI-CARD-HOLDER-IND
    3,              // CUST-FICO-CREDIT-SCORE
    FILLER_WIDTH,   // FILLER
];

/// Sum of `widths`, by slice pattern so that the constant evaluation needs no indexing.
const fn total(widths: &[usize]) -> usize {
    match widths {
        [] => 0,
        [first, rest @ ..] => *first + total(rest),
    }
}

/// One customer, `CUSTOMER-RECORD` of `CVCUS01Y`: 19 fields, 500 bytes (CVCUS01Y-001 to
/// CVCUS01Y-019).
///
/// Every `PIC X(n)` field keeps its bytes verbatim, trailing spaces included (PIC-001), and
/// `FILLER` keeps whatever it holds (PIC-012). `Debug` redacts the SSN through [`Ssn`]; every
/// other field shows its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomerRecord {
    /// `CUST-ID PIC 9(09)`, offset 0 (CVCUS01Y-001): the customer key, KSDS `KEYS(9 0)`.
    pub id: PicU<9>,
    /// `CUST-FIRST-NAME PIC X(25)`, offset 9 (CVCUS01Y-002).
    pub first_name: PicX<25>,
    /// `CUST-MIDDLE-NAME PIC X(25)`, offset 34 (CVCUS01Y-003).
    pub middle_name: PicX<25>,
    /// `CUST-LAST-NAME PIC X(25)`, offset 59 (CVCUS01Y-004).
    pub last_name: PicX<25>,
    /// `CUST-ADDR-LINE-1 PIC X(50)`, offset 84 (CVCUS01Y-005).
    pub addr_line_1: PicX<50>,
    /// `CUST-ADDR-LINE-2 PIC X(50)`, offset 134 (CVCUS01Y-006).
    pub addr_line_2: PicX<50>,
    /// `CUST-ADDR-LINE-3 PIC X(50)`, offset 184 (CVCUS01Y-007).
    pub addr_line_3: PicX<50>,
    /// `CUST-ADDR-STATE-CD PIC X(02)`, offset 234 (CVCUS01Y-008).
    pub addr_state_cd: PicX<2>,
    /// `CUST-ADDR-COUNTRY-CD PIC X(03)`, offset 236 (CVCUS01Y-009).
    pub addr_country_cd: PicX<3>,
    /// `CUST-ADDR-ZIP PIC X(10)`, offset 239 (CVCUS01Y-010).
    pub addr_zip: PicX<10>,
    /// `CUST-PHONE-NUM-1 PIC X(15)`, offset 249 (CVCUS01Y-011).
    pub phone_num_1: PicX<15>,
    /// `CUST-PHONE-NUM-2 PIC X(15)`, offset 264 (CVCUS01Y-012).
    pub phone_num_2: PicX<15>,
    /// `CUST-SSN PIC 9(09)`, offset 279 (CVCUS01Y-013): sensitive, redacted to `***-**-` and the
    /// last four digits wherever it is formatted (PIC-024).
    pub ssn: Ssn,
    /// `CUST-GOVT-ISSUED-ID PIC X(20)`, offset 288 (CVCUS01Y-014).
    pub govt_issued_id: PicX<20>,
    /// `CUST-DOB-YYYY-MM-DD PIC X(10)`, offset 308 (CVCUS01Y-015): `None` for ten spaces
    /// (PIC-015).
    pub dob_yyyy_mm_dd: PicDate,
    /// `CUST-EFT-ACCOUNT-ID PIC X(10)`, offset 318 (CVCUS01Y-016).
    pub eft_account_id: PicX<10>,
    /// `CUST-PRI-CARD-HOLDER-IND PIC X(01)`, offset 328 (CVCUS01Y-017).
    pub pri_card_holder_ind: PicX<1>,
    /// `CUST-FICO-CREDIT-SCORE PIC 9(03)`, offset 329 (CVCUS01Y-018).
    pub fico_credit_score: PicU<3>,
    /// `FILLER PIC X(168)`, offset 332 (CVCUS01Y-019): all spaces in the fixture, kept byte for
    /// byte whatever it holds (PIC-012).
    pub filler: PicX<FILLER_WIDTH>,
}

impl FixedRecord for CustomerRecord {
    /// `RECORDSIZE(500 500)` (`CUSTFILE.jcl:51`). The fields must fill it exactly, so a mistyped
    /// width in `FIELD_WIDTHS` fails the build rather than the first decode.
    const WIDTH: usize = {
        let width = 500;
        assert!(
            total(&FIELD_WIDTHS) == width,
            "the CVCUS01Y field widths must add up to 500 bytes"
        );
        width
    };

    /// The record ends with `FILLER PIC X(168)`, so a line body may lack up to those 168 bytes
    /// (PIC-013). Every `custdata.txt` body is the full 500 bytes (FIXFRM-004).
    const TRAILING_FILLER: usize = FILLER_WIDTH;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            id: r.pic_u("CUST-ID")?,
            first_name: r.pic_x("CUST-FIRST-NAME")?,
            middle_name: r.pic_x("CUST-MIDDLE-NAME")?,
            last_name: r.pic_x("CUST-LAST-NAME")?,
            addr_line_1: r.pic_x("CUST-ADDR-LINE-1")?,
            addr_line_2: r.pic_x("CUST-ADDR-LINE-2")?,
            addr_line_3: r.pic_x("CUST-ADDR-LINE-3")?,
            addr_state_cd: r.pic_x("CUST-ADDR-STATE-CD")?,
            addr_country_cd: r.pic_x("CUST-ADDR-COUNTRY-CD")?,
            addr_zip: r.pic_x("CUST-ADDR-ZIP")?,
            phone_num_1: r.pic_x("CUST-PHONE-NUM-1")?,
            phone_num_2: r.pic_x("CUST-PHONE-NUM-2")?,
            ssn: Ssn::new(r.pic_u("CUST-SSN")?),
            govt_issued_id: r.pic_x("CUST-GOVT-ISSUED-ID")?,
            dob_yyyy_mm_dd: r.date("CUST-DOB-YYYY-MM-DD")?,
            eft_account_id: r.pic_x("CUST-EFT-ACCOUNT-ID")?,
            pri_card_holder_ind: r.pic_x("CUST-PRI-CARD-HOLDER-IND")?,
            fico_credit_score: r.pic_u("CUST-FICO-CREDIT-SCORE")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_u(self.id)
            .pic_x(&self.first_name)
            .pic_x(&self.middle_name)
            .pic_x(&self.last_name)
            .pic_x(&self.addr_line_1)
            .pic_x(&self.addr_line_2)
            .pic_x(&self.addr_line_3)
            .pic_x(&self.addr_state_cd)
            .pic_x(&self.addr_country_cd)
            .pic_x(&self.addr_zip)
            .pic_x(&self.phone_num_1)
            .pic_x(&self.phone_num_2)
            .pic_u(*self.ssn.expose())
            .pic_x(&self.govt_issued_id)
            .date(self.dob_yyyy_mm_dd)
            .pic_x(&self.eft_account_id)
            .pic_x(&self.pri_card_holder_ind)
            .pic_u(self.fico_credit_score)
            .pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{Error, ErrorKind, FixedRecord, Framed, Line, PicU, Terminator};

    use super::{CustomerRecord, FILLER_WIDTH};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Line 1 of `custdata.txt` up to its `FILLER`, one literal per field: customer 1, SSN
    /// `020973888`, date of birth `1961-06-08`.
    const LINE_ONE_DATA: &[u8] = concat!(
        "000000001",
        "Immanuel                 ",
        "Madeline                 ",
        "Kessler                  ",
        "618 Deshaun Route                                 ",
        "Apt. 802                                          ",
        "Altenwerthshire                                   ",
        "NC",
        "USA",
        "12546     ",
        "(908)119-8310  ",
        "(373)693-8684  ",
        "020973888",
        "00000000000049368437",
        "1961-06-08",
        "0053581756",
        "Y",
        "274",
    )
    .as_bytes();

    /// The full 500-byte body of `custdata.txt` line 1: [`LINE_ONE_DATA`] and 168 spaces of
    /// `FILLER`.
    fn line_one() -> Vec<u8> {
        let mut line = LINE_ONE_DATA.to_vec();
        line.extend_from_slice(&[b' '; FILLER_WIDTH]);
        line
    }

    /// Line 1 with `bytes` written over it from `offset`.
    fn with_field(offset: usize, bytes: &[u8]) -> Result<Vec<u8>, Box<dyn StdError>> {
        let mut line = line_one();
        line.get_mut(offset..offset + bytes.len())
            .ok_or("replacement outside the record")?
            .copy_from_slice(bytes);
        Ok(line)
    }

    // audit: CVCUS01Y-001
    // audit: CVCUS01Y-013
    // audit: CVCUS01Y-015
    // audit: CVCUS01Y-018
    // audit: CVCUS01Y-019
    #[test]
    fn decodes_custdata_line_one_and_re_encodes_its_bytes() -> TestResult {
        let line = line_one();
        let record = CustomerRecord::decode(&line)?;
        assert_eq!(record.id.get(), 1);
        assert_eq!(record.first_name.to_text(), "Immanuel");
        assert_eq!(record.middle_name.to_text(), "Madeline");
        assert_eq!(record.last_name.to_text(), "Kessler");
        assert_eq!(record.addr_line_1.to_text(), "618 Deshaun Route");
        assert_eq!(record.addr_line_3.to_text(), "Altenwerthshire");
        assert_eq!(record.addr_state_cd.as_bytes(), b"NC");
        assert_eq!(record.addr_country_cd.as_bytes(), b"USA");
        assert_eq!(record.addr_zip.as_bytes(), b"12546     ");
        assert_eq!(record.phone_num_2.as_bytes(), b"(373)693-8684  ");
        assert_eq!(*record.ssn.expose(), PicU::<9>::new(20_973_888)?);
        assert_eq!(record.govt_issued_id.as_bytes(), b"00000000000049368437");
        assert_eq!(
            record.dob_yyyy_mm_dd.value(),
            NaiveDate::from_ymd_opt(1961, 6, 8)
        );
        assert_eq!(record.eft_account_id.as_bytes(), b"0053581756");
        assert_eq!(record.pri_card_holder_ind.as_bytes(), b"Y");
        assert_eq!(record.fico_credit_score.get(), 274);
        assert!(record.filler.is_spaces());
        let encoded = record.encode();
        assert_eq!(encoded.len(), CustomerRecord::WIDTH);
        assert_eq!(encoded, line);
        Ok(())
    }

    // audit: CVCUS01Y-015
    // audit: CVCUS01Y-019
    #[test]
    fn blank_date_and_non_space_filler_round_trip_unchanged() -> TestResult {
        let blank_dob = with_field(308, b"          ")?;
        let record = CustomerRecord::decode(&blank_dob)?;
        assert_eq!(record.dob_yyyy_mm_dd.value(), None);
        assert_eq!(record.encode(), blank_dob);

        let marked_filler = with_field(332, b"0000")?;
        let record = CustomerRecord::decode(&marked_filler)?;
        assert!(!record.filler.is_spaces());
        assert_eq!(record.encode(), marked_filler);
        Ok(())
    }

    // audit: CVCUS01Y-013
    #[test]
    fn debug_shows_only_the_last_four_ssn_digits() -> TestResult {
        let record = CustomerRecord::decode(&line_one())?;
        let text = format!("{record:?}");
        assert!(!text.contains("020973888"));
        assert!(!text.contains("020-97-3888"));
        assert!(text.contains("***-**-3888"));

        let error = CustomerRecord::decode(&with_field(279, b"02097388 ")?)
            .err()
            .ok_or("a blank SSN digit decoded")?;
        assert!(!format!("{error} {error:?}").contains("0209738"));
        Ok(())
    }

    // audit: CVCUS01Y-001
    // audit: CVCUS01Y-002
    // audit: CVCUS01Y-013
    // audit: CVCUS01Y-015
    // audit: CVCUS01Y-018
    // audit: CVCUS01Y-019
    #[test]
    fn field_errors_name_the_field_offset_and_width() -> TestResult {
        let cases: [(&'static str, usize, usize, &[u8], ErrorKind); 6] = [
            ("CUST-ID", 0, 9, b"00000000A", ErrorKind::NonDigit),
            ("CUST-FIRST-NAME", 9, 25, b"\t", ErrorKind::NonPrintable),
            ("CUST-SSN", 279, 9, b"02097388 ", ErrorKind::NonDigit),
            (
                "CUST-DOB-YYYY-MM-DD",
                308,
                10,
                b"1961-02-30",
                ErrorKind::InvalidDate,
            ),
            (
                "CUST-FICO-CREDIT-SCORE",
                329,
                3,
                b"+74",
                ErrorKind::NonDigit,
            ),
            (
                "FILLER",
                332,
                FILLER_WIDTH,
                b"\x7f",
                ErrorKind::NonPrintable,
            ),
        ];
        for (field, offset, width, bytes, kind) in cases {
            assert_eq!(
                CustomerRecord::decode(&with_field(offset, bytes)?),
                Err(Error::new(field, offset, width, kind)),
            );
        }
        Ok(())
    }

    #[test]
    fn a_slice_of_any_other_length_is_a_record_width_error() -> TestResult {
        let line = line_one();
        let mut long = line.clone();
        long.push(b' ');
        let short = line
            .get(..CustomerRecord::WIDTH - 1)
            .ok_or("line one is shorter than the record")?;
        for bytes in [&[][..], short, long.as_slice()] {
            assert!(matches!(
                CustomerRecord::decode(bytes),
                Err(Error {
                    field: "record",
                    offset: 0,
                    width: 500,
                    kind: ErrorKind::WidthMismatch { expected: 500, actual },
                }) if actual == bytes.len()
            ));
        }
        Ok(())
    }

    // audit: CVCUS01Y-019
    #[test]
    fn a_body_may_lack_only_the_trailing_filler() -> TestResult {
        assert_eq!(CustomerRecord::TRAILING_FILLER, FILLER_WIDTH);
        let framed = Framed::<CustomerRecord>::decode(Line::new(LINE_ONE_DATA, Terminator::Lf))?;
        assert!(framed.record.filler.is_spaces());
        assert_eq!(framed.framing.body_len, LINE_ONE_DATA.len());
        let mut expected = LINE_ONE_DATA.to_vec();
        expected.push(b'\n');
        assert_eq!(framed.try_encode()?, expected);

        let into_fico = LINE_ONE_DATA
            .split_last()
            .map(|(_, rest)| rest)
            .ok_or("line one has no data")?;
        assert_eq!(
            Framed::<CustomerRecord>::decode(Line::new(into_fico, Terminator::Lf)).err(),
            Some(Error::new(
                "record",
                0,
                CustomerRecord::WIDTH,
                ErrorKind::ShortBodyNotFiller {
                    expected: CustomerRecord::WIDTH,
                    actual: into_fico.len(),
                },
            )),
        );
        Ok(())
    }
}
