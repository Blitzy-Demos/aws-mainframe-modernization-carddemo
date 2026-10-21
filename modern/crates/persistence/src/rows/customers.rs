//! Codec B for table `customers` (migration `0001_reference_and_master.sql`): the 500-byte
//! `CUSTOMER-RECORD` of `CVCUS01Y` (`CVCUS01Y.cpy:4-23`; CVCUS01Y-001 to CVCUS01Y-019), which
//! the KSDS `KEYS(9 0)` of `CUSTFILE.jcl:50` held.
//!
//! The 19 copybook fields map to the 19 columns one for one, in copybook order:
//!
//! - every `PIC X(n)` field, `FILLER PIC X(168)` included, to `CHAR(n)`, byte for byte, padding
//!   kept (PIC-001, PIC-002, PIC-012);
//! - `CUST-ID`, `CUST-SSN` and `CUST-FICO-CREDIT-SCORE`, the three `PIC 9(n)` fields, to `BIGINT`
//!   with the range check of their picture, so the key is 64-bit end to end (PIC-003, PIC-023,
//!   Checked Integer Narrowing);
//! - `CUST-DOB-YYYY-MM-DD PIC X(10)` to `DATE`, `NULL` for ten spaces, never text (PIC-015,
//!   Typed Dates and Timestamps).
//!
//! # SSN
//!
//! `CUST-SSN` is sensitive (CVCUS01Y-013, PIC-024, E32). Storage is not egress, so `cust_ssn`
//! holds the full nine digits. They leave the record only through `Ssn::expose` and return
//! through `Ssn::new`, the constructor codec A's decoder uses, so a record read back equals the
//! record written. Because a row holds the raw SSN, [`CustomerRow`] derives no `Debug`, `Clone`
//! or `Serialize`, and every conversion error names its column and class, never the value
//! (Cardholder Data Redaction).

use chrono::NaiveDate;
use domain::records::CustomerRecord;
use domain::sensitive::Ssn;

use super::{
    column_to_date, date_to_column, i64_to_pic_u, pic_u_to_i64, pic_x_to_string, string_to_pic_x,
};
use crate::error::PersistenceError;

/// One row of `customers`: its 19 columns in table order, one per `CVCUS01Y` field
/// (CVCUS01Y-001 to CVCUS01Y-019).
///
/// Field names are the column names, as `sqlx::query_as!` and the binds of `repos::customers`
/// require. Each `String` holds exactly the picture's n ASCII bytes and each `i64` a value in
/// `0 ..= 10^n − 1`, the range the column's check constraint allows.
pub struct CustomerRow {
    /// `CUST-ID PIC 9(09)` (CVCUS01Y-001): the primary key, KSDS `KEYS(9 0)`.
    pub(crate) cust_id: i64,
    /// `CUST-FIRST-NAME PIC X(25)` (CVCUS01Y-002).
    pub(crate) cust_first_name: String,
    /// `CUST-MIDDLE-NAME PIC X(25)` (CVCUS01Y-003).
    pub(crate) cust_middle_name: String,
    /// `CUST-LAST-NAME PIC X(25)` (CVCUS01Y-004).
    pub(crate) cust_last_name: String,
    /// `CUST-ADDR-LINE-1 PIC X(50)` (CVCUS01Y-005).
    pub(crate) cust_addr_line_1: String,
    /// `CUST-ADDR-LINE-2 PIC X(50)` (CVCUS01Y-006).
    pub(crate) cust_addr_line_2: String,
    /// `CUST-ADDR-LINE-3 PIC X(50)` (CVCUS01Y-007).
    pub(crate) cust_addr_line_3: String,
    /// `CUST-ADDR-STATE-CD PIC X(02)` (CVCUS01Y-008).
    pub(crate) cust_addr_state_cd: String,
    /// `CUST-ADDR-COUNTRY-CD PIC X(03)` (CVCUS01Y-009).
    pub(crate) cust_addr_country_cd: String,
    /// `CUST-ADDR-ZIP PIC X(10)` (CVCUS01Y-010).
    pub(crate) cust_addr_zip: String,
    /// `CUST-PHONE-NUM-1 PIC X(15)` (CVCUS01Y-011).
    pub(crate) cust_phone_num_1: String,
    /// `CUST-PHONE-NUM-2 PIC X(15)` (CVCUS01Y-012).
    pub(crate) cust_phone_num_2: String,
    /// `CUST-SSN PIC 9(09)` (CVCUS01Y-013): the full SSN, bound and read only by
    /// `repos::customers`, never formatted (PIC-024, E32).
    pub(crate) cust_ssn: i64,
    /// `CUST-GOVT-ISSUED-ID PIC X(20)` (CVCUS01Y-014).
    pub(crate) cust_govt_issued_id: String,
    /// `CUST-DOB-YYYY-MM-DD PIC X(10)` (CVCUS01Y-015): `None`, stored `NULL`, for ten spaces
    /// (PIC-015).
    pub(crate) cust_dob_yyyy_mm_dd: Option<NaiveDate>,
    /// `CUST-EFT-ACCOUNT-ID PIC X(10)` (CVCUS01Y-016).
    pub(crate) cust_eft_account_id: String,
    /// `CUST-PRI-CARD-HOLDER-IND PIC X(01)` (CVCUS01Y-017).
    pub(crate) cust_pri_card_holder_ind: String,
    /// `CUST-FICO-CREDIT-SCORE PIC 9(03)` (CVCUS01Y-018).
    pub(crate) cust_fico_credit_score: i64,
    /// `FILLER PIC X(168)` (CVCUS01Y-019): kept byte for byte whatever it holds (PIC-012).
    pub(crate) filler: String,
}

impl CustomerRow {
    /// The row that stores `record`, every field unchanged: `PIC X(n)` bytes verbatim, the three
    /// `PIC 9(n)` values as `BIGINT`, the date of birth as a `DATE` or `NULL` (CVCUS01Y-001 to
    /// CVCUS01Y-019).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the first failing column, with
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or non-ASCII
    /// byte in a `PIC X(n)` field, which its `CHAR(n)` column could not return byte for byte
    /// (PIC-002). The `PIC 9(n)` values always fit `BIGINT` (PIC-023).
    pub fn from_record(record: &CustomerRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            cust_id: pic_u_to_i64("cust_id", record.id)?,
            cust_first_name: pic_x_to_string("cust_first_name", &record.first_name)?,
            cust_middle_name: pic_x_to_string("cust_middle_name", &record.middle_name)?,
            cust_last_name: pic_x_to_string("cust_last_name", &record.last_name)?,
            cust_addr_line_1: pic_x_to_string("cust_addr_line_1", &record.addr_line_1)?,
            cust_addr_line_2: pic_x_to_string("cust_addr_line_2", &record.addr_line_2)?,
            cust_addr_line_3: pic_x_to_string("cust_addr_line_3", &record.addr_line_3)?,
            cust_addr_state_cd: pic_x_to_string("cust_addr_state_cd", &record.addr_state_cd)?,
            cust_addr_country_cd: pic_x_to_string("cust_addr_country_cd", &record.addr_country_cd)?,
            cust_addr_zip: pic_x_to_string("cust_addr_zip", &record.addr_zip)?,
            cust_phone_num_1: pic_x_to_string("cust_phone_num_1", &record.phone_num_1)?,
            cust_phone_num_2: pic_x_to_string("cust_phone_num_2", &record.phone_num_2)?,
            cust_ssn: pic_u_to_i64("cust_ssn", *record.ssn.expose())?,
            cust_govt_issued_id: pic_x_to_string("cust_govt_issued_id", &record.govt_issued_id)?,
            cust_dob_yyyy_mm_dd: date_to_column(record.dob_yyyy_mm_dd),
            cust_eft_account_id: pic_x_to_string("cust_eft_account_id", &record.eft_account_id)?,
            cust_pri_card_holder_ind: pic_x_to_string(
                "cust_pri_card_holder_ind",
                &record.pri_card_holder_ind,
            )?,
            cust_fico_credit_score: pic_u_to_i64(
                "cust_fico_credit_score",
                record.fico_credit_score,
            )?,
            filler: pic_x_to_string("filler", &record.filler)?,
        })
    }

    /// The record this row stores, through checked conversions only: nothing is trimmed,
    /// padded, truncated or re-encoded, so codec A re-encodes it to the bytes it was decoded
    /// from (CVCUS01Y-001 to CVCUS01Y-019). The SSN is rebuilt with `Ssn::new`, as codec A's
    /// decoder builds it (CVCUS01Y-013, PIC-024).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the first failing column, never its value:
    ///
    /// - [`DecodeKind::OutOfRange`](crate::error::DecodeKind::OutOfRange) for `cust_id` or
    ///   `cust_ssn` outside `0 ..= 999_999_999`, `cust_fico_credit_score` outside `0 ..= 999`
    ///   (PIC-003), or a `cust_dob_yyyy_mm_dd` year outside `0 ..= 9999`, which the
    ///   `YYYY-MM-DD` text cannot carry (PIC-015);
    /// - [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) for `CHAR` text
    ///   that is not exactly its picture's width;
    /// - [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or
    ///   non-ASCII character (PIC-002).
    pub fn into_record(self) -> Result<CustomerRecord, PersistenceError> {
        let Self {
            cust_id,
            cust_first_name,
            cust_middle_name,
            cust_last_name,
            cust_addr_line_1,
            cust_addr_line_2,
            cust_addr_line_3,
            cust_addr_state_cd,
            cust_addr_country_cd,
            cust_addr_zip,
            cust_phone_num_1,
            cust_phone_num_2,
            cust_ssn,
            cust_govt_issued_id,
            cust_dob_yyyy_mm_dd,
            cust_eft_account_id,
            cust_pri_card_holder_ind,
            cust_fico_credit_score,
            filler,
        } = self;
        Ok(CustomerRecord {
            id: i64_to_pic_u("cust_id", cust_id)?,
            first_name: string_to_pic_x("cust_first_name", &cust_first_name)?,
            middle_name: string_to_pic_x("cust_middle_name", &cust_middle_name)?,
            last_name: string_to_pic_x("cust_last_name", &cust_last_name)?,
            addr_line_1: string_to_pic_x("cust_addr_line_1", &cust_addr_line_1)?,
            addr_line_2: string_to_pic_x("cust_addr_line_2", &cust_addr_line_2)?,
            addr_line_3: string_to_pic_x("cust_addr_line_3", &cust_addr_line_3)?,
            addr_state_cd: string_to_pic_x("cust_addr_state_cd", &cust_addr_state_cd)?,
            addr_country_cd: string_to_pic_x("cust_addr_country_cd", &cust_addr_country_cd)?,
            addr_zip: string_to_pic_x("cust_addr_zip", &cust_addr_zip)?,
            phone_num_1: string_to_pic_x("cust_phone_num_1", &cust_phone_num_1)?,
            phone_num_2: string_to_pic_x("cust_phone_num_2", &cust_phone_num_2)?,
            ssn: Ssn::new(i64_to_pic_u("cust_ssn", cust_ssn)?),
            govt_issued_id: string_to_pic_x("cust_govt_issued_id", &cust_govt_issued_id)?,
            dob_yyyy_mm_dd: column_to_date("cust_dob_yyyy_mm_dd", cust_dob_yyyy_mm_dd)?,
            eft_account_id: string_to_pic_x("cust_eft_account_id", &cust_eft_account_id)?,
            pri_card_holder_ind: string_to_pic_x(
                "cust_pri_card_holder_ind",
                &cust_pri_card_holder_ind,
            )?,
            fico_credit_score: i64_to_pic_u("cust_fico_credit_score", cust_fico_credit_score)?,
            filler: string_to_pic_x("filler", &filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{FixedRecord, PicX};
    use domain::records::CustomerRecord;

    use super::CustomerRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A corruption of one record field and the column whose conversion must reject it.
    type RecordCase = (fn(&mut CustomerRecord), &'static str);

    /// A corruption of one row column, the column it touches and the class it must be
    /// rejected with.
    type RowCase = (fn(&mut CustomerRow), &'static str, DecodeKind);

    /// An invented SSN, none of the 50 fixture values. Errors and record formatting must never
    /// repeat it (Cardholder Data Redaction).
    const SSN_DIGITS: &str = "987123456";

    /// [`SSN_DIGITS`] as the `cust_ssn` value.
    const SSN: i64 = 987_123_456;

    /// The trailing bytes of the test record's `FILLER`, which must survive verbatim (PIC-012).
    const FILLER_MARK: &str = "~filler kept~";

    /// A 500-byte line in the shape of `custdata.txt` with invented values and `dob` as its
    /// `CUST-DOB-YYYY-MM-DD` text: every field left-justified and space-padded to its
    /// `CVCUS01Y` width, then a `FILLER` of 168 bytes ending in [`FILLER_MARK`].
    fn custdata_line(dob: &str) -> Vec<u8> {
        let fields = [
            ("000000042", 9),
            ("Testfirst", 25),
            ("Middle", 25),
            ("Lastname", 25),
            ("1 Example Street", 50),
            ("Suite 9", 50),
            ("Sampletown", 50),
            ("CA", 2),
            ("USA", 3),
            ("94000-1234", 10),
            ("(555)010-0001", 15),
            ("(555)010-0002", 15),
            (SSN_DIGITS, 9),
            ("00000000000000004242", 20),
            (dob, 10),
            ("0000004242", 10),
            ("N", 1),
            ("750", 3),
        ];
        let mut line = String::with_capacity(CustomerRecord::WIDTH);
        for (text, width) in fields {
            line.push_str(&format!("{text:<width$}"));
        }
        line.push_str(&format!("{FILLER_MARK:>168}"));
        line.into_bytes()
    }

    // audit: CVCUS01Y-001
    // audit: CVCUS01Y-002
    // audit: CVCUS01Y-010
    // audit: CVCUS01Y-013
    // audit: CVCUS01Y-015
    // audit: CVCUS01Y-018
    // audit: CVCUS01Y-019
    // audit: PIC-001
    // audit: PIC-003
    // audit: PIC-012
    // audit: PIC-015
    #[test]
    fn custdata_shaped_records_round_trip_byte_for_byte() -> TestResult {
        let birth_date = NaiveDate::from_ymd_opt(1980, 2, 29).ok_or("invalid test date")?;
        for (dob, stored) in [("1980-02-29", Some(birth_date)), ("", None)] {
            let line = custdata_line(dob);
            let record = CustomerRecord::decode(&line)?;
            let row = CustomerRow::from_record(&record)?;
            assert_eq!(row.cust_id, 42);
            assert_eq!(row.cust_first_name, format!("{:<25}", "Testfirst"));
            assert_eq!(row.cust_addr_zip, "94000-1234");
            assert_eq!(row.cust_phone_num_1, "(555)010-0001  ");
            assert!(
                row.cust_ssn == SSN,
                "cust_ssn differs from the record's SSN"
            );
            assert_eq!(row.cust_dob_yyyy_mm_dd, stored);
            assert_eq!(row.cust_pri_card_holder_ind, "N");
            assert_eq!(row.cust_fico_credit_score, 750);
            assert_eq!(row.filler, format!("{FILLER_MARK:>168}"));
            let back = row.into_record()?;
            assert!(back == record, "record differs after the round trip");
            assert!(
                back.encode() == line,
                "re-encoded bytes differ from the line"
            );
            assert!(!format!("{back:?}").contains(SSN_DIGITS));
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn record_bytes_no_column_can_hold_are_rejected_by_column() -> TestResult {
        let cases: [RecordCase; 3] = [
            (
                |record| record.middle_name = PicX::moved_from(b"Nul\0name"),
                "cust_middle_name",
            ),
            (
                |record| record.addr_line_2 = PicX::moved_from(&[b'A', 0x80]),
                "cust_addr_line_2",
            ),
            (
                |record| record.filler = PicX::moved_from("caf\u{e9}".as_bytes()),
                "filler",
            ),
        ];
        for (corrupt, column) in cases {
            let mut record = CustomerRecord::decode(&custdata_line(""))?;
            corrupt(&mut record);
            assert_eq!(
                CustomerRow::from_record(&record).map(drop),
                Err(PersistenceError::decode(column, DecodeKind::InvalidByte))
            );
        }
        Ok(())
    }

    // audit: CVCUS01Y-013
    // audit: CVCUS01Y-018
    // audit: CVCUS01Y-019
    // audit: PIC-002
    // audit: PIC-003
    // audit: PIC-015
    // audit: PIC-023
    #[test]
    fn stored_values_outside_their_picture_are_rejected_by_column_and_class() -> TestResult {
        let out_of_range = DecodeKind::OutOfRange;
        let cases: [RowCase; 13] = [
            (|row| row.cust_id = -1, "cust_id", out_of_range),
            (|row| row.cust_id = 1_000_000_000, "cust_id", out_of_range),
            (|row| row.cust_ssn = 1_000_000_000, "cust_ssn", out_of_range),
            (|row| row.cust_ssn = i64::MAX, "cust_ssn", out_of_range),
            (|row| row.cust_ssn = -1, "cust_ssn", out_of_range),
            (
                |row| row.cust_fico_credit_score = -1,
                "cust_fico_credit_score",
                out_of_range,
            ),
            (
                |row| row.cust_fico_credit_score = 1_000,
                "cust_fico_credit_score",
                out_of_range,
            ),
            (
                |row| row.cust_dob_yyyy_mm_dd = NaiveDate::from_ymd_opt(10_000, 1, 1),
                "cust_dob_yyyy_mm_dd",
                out_of_range,
            ),
            (
                |row| row.filler = " ".repeat(167),
                "filler",
                DecodeKind::WidthMismatch {
                    expected: 168,
                    actual: 167,
                },
            ),
            (
                |row| row.cust_addr_state_cd = "CAL".to_owned(),
                "cust_addr_state_cd",
                DecodeKind::WidthMismatch {
                    expected: 2,
                    actual: 3,
                },
            ),
            (
                |row| row.cust_last_name = format!("{:<25}", "Nul\0name"),
                "cust_last_name",
                DecodeKind::InvalidByte,
            ),
            (
                |row| row.cust_first_name = format!("{:<24}", "Caf\u{e9}"),
                "cust_first_name",
                DecodeKind::InvalidByte,
            ),
            (
                |row| row.cust_pri_card_holder_ind = "\0".to_owned(),
                "cust_pri_card_holder_ind",
                DecodeKind::InvalidByte,
            ),
        ];
        for (corrupt, column, kind) in cases {
            let record = CustomerRecord::decode(&custdata_line("1980-02-29"))?;
            let mut row = CustomerRow::from_record(&record)?;
            corrupt(&mut row);
            assert_eq!(
                row.into_record().map(drop),
                Err(PersistenceError::decode(column, kind))
            );
        }
        Ok(())
    }

    /// Cardholder Data Redaction: a rejected SSN is reported by its column and class alone.
    // audit: PIC-024
    #[test]
    fn errors_never_repeat_the_ssn() -> TestResult {
        let record = CustomerRecord::decode(&custdata_line(""))?;
        let mut row = CustomerRow::from_record(&record)?;
        // Ten digits, the last nine of them the invented SSN.
        row.cust_ssn = 1_987_123_456;
        let error = row
            .into_record()
            .err()
            .ok_or("a ten-digit SSN must be rejected")?;
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(text.contains("cust_ssn"), "{text}");
            assert!(!text.contains("987") && !text.contains("3456"), "{text}");
        }
        Ok(())
    }
}
