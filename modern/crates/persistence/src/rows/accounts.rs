//! Codec B for table `accounts`: the `CVACT01Y` `ACCOUNT-RECORD` (`app/cpy/CVACT01Y.cpy:4-17`)
//! as one row of migration `0001_reference_and_master.sql` (CVACT01Y-001 to CVACT01Y-013).
//!
//! Every copybook field is one column, in copybook order, and the account KSDS key `ACCT-ID`
//! (`KEYS(11 0)`, `app/jcl/ACCTFILE.jcl:40`) is the primary key `acct_id`. The five
//! `PIC S9(10)V99` amounts are `NUMERIC(12,2)` at scale 2 (PIC-005), the three `PIC X(10)` dates
//! are `DATE`, `NULL` for ten spaces (PIC-015, Typed Dates and Timestamps), and the `PIC X(n)`
//! fields are `CHAR(n)` holding their bytes verbatim (PIC-001, PIC-012).
//!
//! Every fixture account holds its disclosure group code in `ACCT-ADDR-ZIP` and leaves
//! `ACCT-GROUP-ID` blank (E07). Both columns store exactly the bytes codec A decoded, so the
//! `DEFAULT` interest lookup that follows from them stays the batch rule's concern, not this one.

use chrono::NaiveDate;
use domain::records::account::AccountRecord;
use rust_decimal::Decimal;

use super::{
    column_to_date, date_to_column, decimal_to_money, i64_to_pic_u, money_to_decimal, pic_u_to_i64,
    pic_x_to_string, string_to_pic_x,
};
use crate::error::PersistenceError;

/// One row of `accounts`: a field per column, with the column's name, in the migration's order.
///
/// Only the nullable `DATE` columns are `Option`. No account field is cardholder data, but, like
/// every codec B row, the row derives neither `Debug` nor `Serialize`, and it leaves `Clone`
/// underived too, so that no row value reaches a log or a response by accident.
pub struct AccountRow {
    /// `ACCT-ID PIC 9(11)`: `BIGINT` range-checked to `0..=99999999999`, 64-bit end to end
    /// (CVACT01Y-001, PIC-003, PIC-023).
    pub(crate) acct_id: i64,
    /// `ACCT-ACTIVE-STATUS PIC X(01)`: `CHAR(1)` (CVACT01Y-002).
    pub(crate) acct_active_status: String,
    /// `ACCT-CURR-BAL PIC S9(10)V99`: `NUMERIC(12,2)` (CVACT01Y-003).
    pub(crate) acct_curr_bal: Decimal,
    /// `ACCT-CREDIT-LIMIT PIC S9(10)V99`: `NUMERIC(12,2)` (CVACT01Y-004).
    pub(crate) acct_credit_limit: Decimal,
    /// `ACCT-CASH-CREDIT-LIMIT PIC S9(10)V99`: `NUMERIC(12,2)` (CVACT01Y-005).
    pub(crate) acct_cash_credit_limit: Decimal,
    /// `ACCT-OPEN-DATE PIC X(10)`: `DATE`, `NULL` for ten spaces (CVACT01Y-006).
    pub(crate) acct_open_date: Option<NaiveDate>,
    /// `ACCT-EXPIRAION-DATE PIC X(10)`: `DATE`, `NULL` for ten spaces; the copybook's spelling
    /// is kept (CVACT01Y-007).
    pub(crate) acct_expiraion_date: Option<NaiveDate>,
    /// `ACCT-REISSUE-DATE PIC X(10)`: `DATE`, `NULL` for ten spaces (CVACT01Y-008).
    pub(crate) acct_reissue_date: Option<NaiveDate>,
    /// `ACCT-CURR-CYC-CREDIT PIC S9(10)V99`: `NUMERIC(12,2)` (CVACT01Y-009).
    pub(crate) acct_curr_cyc_credit: Decimal,
    /// `ACCT-CURR-CYC-DEBIT PIC S9(10)V99`: `NUMERIC(12,2)` (CVACT01Y-010).
    pub(crate) acct_curr_cyc_debit: Decimal,
    /// `ACCT-ADDR-ZIP PIC X(10)`: `CHAR(10)`, the disclosure group code in every fixture record
    /// (CVACT01Y-011, E07).
    pub(crate) acct_addr_zip: String,
    /// `ACCT-GROUP-ID PIC X(10)`: `CHAR(10)`, blank in every fixture record (CVACT01Y-012, E07).
    pub(crate) acct_group_id: String,
    /// `FILLER PIC X(178)`: `CHAR(178)`, whatever it holds (CVACT01Y-013, PIC-012).
    pub(crate) filler: String,
}

impl AccountRow {
    /// The row that stores `record`, one column per field, every value kept exactly: the key and
    /// amounts by value and scale, the dates as calendar dates or `NULL`, and the alphanumeric
    /// fields byte for byte, padding and `FILLER` included (CVACT01Y-001 to CVACT01Y-013).
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the column, with
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte), when
    /// `ACCT-ACTIVE-STATUS`, `ACCT-ADDR-ZIP`, `ACCT-GROUP-ID` or `FILLER` holds a NUL or non-ASCII
    /// byte, which a `CHAR(n)` column cannot store byte for byte (PIC-002). Codec A decodes only
    /// printable ASCII, so every record it produced converts.
    pub fn from_record(record: &AccountRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            acct_id: pic_u_to_i64("acct_id", record.id)?,
            acct_active_status: pic_x_to_string("acct_active_status", &record.active_status)?,
            acct_curr_bal: money_to_decimal(record.curr_bal),
            acct_credit_limit: money_to_decimal(record.credit_limit),
            acct_cash_credit_limit: money_to_decimal(record.cash_credit_limit),
            acct_open_date: date_to_column(record.open_date),
            acct_expiraion_date: date_to_column(record.expiraion_date),
            acct_reissue_date: date_to_column(record.reissue_date),
            acct_curr_cyc_credit: money_to_decimal(record.curr_cyc_credit),
            acct_curr_cyc_debit: money_to_decimal(record.curr_cyc_debit),
            acct_addr_zip: pic_x_to_string("acct_addr_zip", &record.addr_zip)?,
            acct_group_id: pic_x_to_string("acct_group_id", &record.group_id)?,
            filler: pic_x_to_string("filler", &record.filler)?,
        })
    }

    /// The record this row holds. For a row that [`AccountRow::from_record`] built, or that was
    /// read back after it was written, this is a record equal to the original, which codec A
    /// re-encodes to the original 300 bytes (binding d9).
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the first column, in copybook order, whose value
    /// its picture cannot hold; nothing is rounded, truncated, padded or trimmed to make it fit:
    ///
    /// - `acct_id`: [`DecodeKind::OutOfRange`](crate::error::DecodeKind::OutOfRange) for a
    ///   negative value or one above `99999999999` (PIC-003).
    /// - The five amounts: `OutOfRange` for a magnitude of `10^10` or more, or a non-zero digit
    ///   beyond the second decimal place (PIC-005).
    /// - The three dates: `OutOfRange` for a year outside `0..=9999`, which `YYYY-MM-DD` cannot
    ///   carry (PIC-015).
    /// - `acct_active_status`, `acct_addr_zip`, `acct_group_id` and `filler`:
    ///   [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or
    ///   non-ASCII character (PIC-002), and
    ///   [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) for text that is
    ///   not exactly 1, 10, 10 or 178 bytes long (PIC-001, PIC-012).
    pub fn into_record(self) -> Result<AccountRecord, PersistenceError> {
        Ok(AccountRecord {
            id: i64_to_pic_u("acct_id", self.acct_id)?,
            active_status: string_to_pic_x("acct_active_status", &self.acct_active_status)?,
            curr_bal: decimal_to_money("acct_curr_bal", self.acct_curr_bal)?,
            credit_limit: decimal_to_money("acct_credit_limit", self.acct_credit_limit)?,
            cash_credit_limit: decimal_to_money(
                "acct_cash_credit_limit",
                self.acct_cash_credit_limit,
            )?,
            open_date: column_to_date("acct_open_date", self.acct_open_date)?,
            expiraion_date: column_to_date("acct_expiraion_date", self.acct_expiraion_date)?,
            reissue_date: column_to_date("acct_reissue_date", self.acct_reissue_date)?,
            curr_cyc_credit: decimal_to_money("acct_curr_cyc_credit", self.acct_curr_cyc_credit)?,
            curr_cyc_debit: decimal_to_money("acct_curr_cyc_debit", self.acct_curr_cyc_debit)?,
            addr_zip: string_to_pic_x("acct_addr_zip", &self.acct_addr_zip)?,
            group_id: string_to_pic_x("acct_group_id", &self.acct_group_id)?,
            filler: string_to_pic_x("filler", &self.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{FixedRecord, PicX};
    use domain::records::account::AccountRecord;
    use rust_decimal::Decimal;

    use super::AccountRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// One change to a row read back from the database.
    type RowEdit = fn(&mut AccountRow);

    /// One change to a record before it is written.
    type RecordEdit = fn(&mut AccountRecord);

    /// `app/data/ASCII/acctdata.txt` line 1 up to `ACCT-ADDR-ZIP`; `ACCT-GROUP-ID` and `FILLER`
    /// are spaces.
    const FIXTURE_LINE_1: [&str; 11] = [
        "00000000001",  // ACCT-ID
        "Y",            // ACCT-ACTIVE-STATUS
        "00000001940{", // ACCT-CURR-BAL
        "00000020200{", // ACCT-CREDIT-LIMIT
        "00000010200{", // ACCT-CASH-CREDIT-LIMIT
        "2014-11-20",   // ACCT-OPEN-DATE
        "2025-05-20",   // ACCT-EXPIRAION-DATE
        "2025-05-20",   // ACCT-REISSUE-DATE
        "00000000000{", // ACCT-CURR-CYC-CREDIT
        "00000000000{", // ACCT-CURR-CYC-DEBIT
        "A000000000",   // ACCT-ADDR-ZIP
    ];

    /// A fixture-shaped line whose fields of one type all differ, so a swapped column shows:
    /// negative overpunch signs (`}` is −0, `J` −1), a blank date and non-blank tail fields.
    const DISTINCT_FIELDS: [&str; 13] = [
        "00000000042",             // ACCT-ID
        "Y",                       // ACCT-ACTIVE-STATUS
        "00000001940}",            // ACCT-CURR-BAL: -194.00
        "00000020200{",            // ACCT-CREDIT-LIMIT: 2020.00
        "00000010200{",            // ACCT-CASH-CREDIT-LIMIT: 1020.00
        "2014-11-20",              // ACCT-OPEN-DATE
        "          ",              // ACCT-EXPIRAION-DATE: blank
        "2025-05-20",              // ACCT-REISSUE-DATE
        "00000000123J",            // ACCT-CURR-CYC-CREDIT: -12.31
        "00000000456B",            // ACCT-CURR-CYC-DEBIT: 45.62
        "A000000000",              // ACCT-ADDR-ZIP
        "DEFAULT   ",              // ACCT-GROUP-ID
        "~filler bytes are kept~", // FILLER, space-padded to 178
    ];

    /// The widest values each picture holds, both signs, and every date blank.
    const EXTREMES: [&str; 12] = [
        "99999999999",  // ACCT-ID
        "N",            // ACCT-ACTIVE-STATUS
        "99999999999R", // ACCT-CURR-BAL: -9999999999.99
        "99999999999I", // ACCT-CREDIT-LIMIT: 9999999999.99
        "00000000000J", // ACCT-CASH-CREDIT-LIMIT: -0.01
        "          ",   // ACCT-OPEN-DATE
        "          ",   // ACCT-EXPIRAION-DATE
        "          ",   // ACCT-REISSUE-DATE
        "00000000000A", // ACCT-CURR-CYC-CREDIT: 0.01
        "99999999999R", // ACCT-CURR-CYC-DEBIT: -9999999999.99
        "          ",   // ACCT-ADDR-ZIP
        "          ",   // ACCT-GROUP-ID
    ];

    /// The smallest key, the first and last dates `YYYY-MM-DD` carries and a leap day, up to
    /// `ACCT-GROUP-ID`; `line` fills `FILLER` with `~`, so it holds no space at all.
    const BOUNDARIES: [&str; 12] = [
        "00000000000",  // ACCT-ID
        " ",            // ACCT-ACTIVE-STATUS
        "00000000000{", // ACCT-CURR-BAL
        "00000000000{", // ACCT-CREDIT-LIMIT
        "00000000000{", // ACCT-CASH-CREDIT-LIMIT
        "0000-01-01",   // ACCT-OPEN-DATE
        "9999-12-31",   // ACCT-EXPIRAION-DATE
        "2024-02-29",   // ACCT-REISSUE-DATE
        "00000000000{", // ACCT-CURR-CYC-CREDIT
        "00000000000{", // ACCT-CURR-CYC-DEBIT
        "12345-6789",   // ACCT-ADDR-ZIP
        "!\"#$%&'()*",  // ACCT-GROUP-ID
    ];

    /// The 300-byte body of a line: `fields` in copybook order, padded to the record width with
    /// `pad`.
    fn line(fields: &[&str], pad: u8) -> Vec<u8> {
        let mut body = fields.concat().into_bytes();
        body.resize(AccountRecord::WIDTH, pad);
        body
    }

    /// `10^10` at scale 2: eleven integer digits, one more than `S9(10)V99` holds.
    fn eleven_digits() -> Decimal {
        Decimal::new(1_000_000_000_000, 2)
    }

    fn day(year: i32, month: u32, date: u32) -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(year, month, date).ok_or_else(|| "invalid test date".into())
    }

    fn fixture_line_1_row() -> Result<AccountRow, Box<dyn StdError>> {
        let record = AccountRecord::decode(&line(&FIXTURE_LINE_1, b' '))?;
        Ok(AccountRow::from_record(&record)?)
    }

    // audit: CVACT01Y-001
    // audit: CVACT01Y-002
    // audit: CVACT01Y-003
    // audit: CVACT01Y-004
    // audit: CVACT01Y-005
    // audit: CVACT01Y-006
    // audit: CVACT01Y-007
    // audit: CVACT01Y-008
    // audit: CVACT01Y-009
    // audit: CVACT01Y-010
    // audit: CVACT01Y-011
    // audit: CVACT01Y-012
    // audit: CVACT01Y-013
    #[test]
    fn each_field_lands_in_its_own_column() -> TestResult {
        let row = AccountRow::from_record(&AccountRecord::decode(&line(&DISTINCT_FIELDS, b' '))?)?;
        assert_eq!(row.acct_id, 42);
        assert_eq!(row.acct_active_status, "Y");
        assert_eq!(row.acct_curr_bal, Decimal::new(-19_400, 2));
        assert_eq!(row.acct_credit_limit, Decimal::new(202_000, 2));
        assert_eq!(row.acct_cash_credit_limit, Decimal::new(102_000, 2));
        assert_eq!(row.acct_open_date, Some(day(2014, 11, 20)?));
        assert_eq!(row.acct_expiraion_date, None);
        assert_eq!(row.acct_reissue_date, Some(day(2025, 5, 20)?));
        assert_eq!(row.acct_curr_cyc_credit, Decimal::new(-1_231, 2));
        assert_eq!(row.acct_curr_cyc_debit, Decimal::new(4_562, 2));
        assert_eq!(row.acct_addr_zip, "A000000000");
        assert_eq!(row.acct_group_id, "DEFAULT   ");
        assert_eq!(row.filler, format!("{:<178}", "~filler bytes are kept~"));
        // `Decimal` equality ignores scale; the `NUMERIC(12,2)` binds must carry scale 2.
        let amounts = [
            row.acct_curr_bal,
            row.acct_credit_limit,
            row.acct_cash_credit_limit,
            row.acct_curr_cyc_credit,
            row.acct_curr_cyc_debit,
        ];
        assert!(amounts.iter().all(|amount| amount.scale() == 2));
        Ok(())
    }

    // audit: PIC-001
    // audit: PIC-003
    // audit: PIC-005
    // audit: PIC-012
    // audit: PIC-015
    // audit: PIC-023
    #[test]
    fn records_round_trip_through_the_row_to_their_original_bytes() -> TestResult {
        let bodies = [
            line(&FIXTURE_LINE_1, b' '),
            line(&DISTINCT_FIELDS, b' '),
            line(&EXTREMES, b' '),
            line(&BOUNDARIES, b'~'),
        ];
        for body in bodies {
            let record = AccountRecord::decode(&body)?;
            let back = AccountRow::from_record(&record)?.into_record()?;
            assert_eq!(back, record);
            assert_eq!(back.encode(), body);
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn from_record_rejects_bytes_a_char_column_cannot_store() -> TestResult {
        let cases: [(RecordEdit, &str); 4] = [
            (
                |record| record.active_status = PicX::from_array([0]),
                "acct_active_status",
            ),
            (
                |record| record.addr_zip = PicX::from_array(*b"A00000000\x80"),
                "acct_addr_zip",
            ),
            (
                |record| record.group_id = PicX::from_array(*b"\xC3\xA9        "),
                "acct_group_id",
            ),
            (
                |record| record.filler = PicX::from_array([0xFF; 178]),
                "filler",
            ),
        ];
        let base = AccountRecord::decode(&line(&FIXTURE_LINE_1, b' '))?;
        for (edit, column) in cases {
            let mut record = base.clone();
            edit(&mut record);
            assert_eq!(
                AccountRow::from_record(&record).map(drop),
                Err(PersistenceError::decode(column, DecodeKind::InvalidByte)),
                "{column}"
            );
        }
        Ok(())
    }

    /// Applies each edit to the row of fixture line 1 and expects `into_record` to reject it
    /// with the named column and class.
    fn assert_rejected(cases: &[(RowEdit, &'static str, DecodeKind)]) -> TestResult {
        for &(edit, column, kind) in cases {
            let mut row = fixture_line_1_row()?;
            edit(&mut row);
            assert_eq!(
                row.into_record(),
                Err(PersistenceError::decode(column, kind)),
                "{column}"
            );
        }
        Ok(())
    }

    // audit: PIC-003
    // audit: PIC-005
    // audit: PIC-015
    #[test]
    fn into_record_rejects_keys_amounts_and_dates_their_pictures_cannot_hold() -> TestResult {
        use DecodeKind::OutOfRange;

        assert_rejected(&[
            (|row| row.acct_id = -1, "acct_id", OutOfRange),
            (|row| row.acct_id = i64::MIN, "acct_id", OutOfRange),
            (|row| row.acct_id = 100_000_000_000, "acct_id", OutOfRange),
            (
                |row| row.acct_curr_bal = eleven_digits(),
                "acct_curr_bal",
                OutOfRange,
            ),
            (
                |row| row.acct_credit_limit = -eleven_digits(),
                "acct_credit_limit",
                OutOfRange,
            ),
            (
                |row| row.acct_cash_credit_limit = Decimal::new(1, 3),
                "acct_cash_credit_limit",
                OutOfRange,
            ),
            (
                |row| row.acct_open_date = Some(NaiveDate::MAX),
                "acct_open_date",
                OutOfRange,
            ),
            (
                |row| row.acct_expiraion_date = Some(NaiveDate::MIN),
                "acct_expiraion_date",
                OutOfRange,
            ),
            (
                |row| row.acct_reissue_date = Some(NaiveDate::MAX),
                "acct_reissue_date",
                OutOfRange,
            ),
            (
                |row| row.acct_curr_cyc_credit = eleven_digits(),
                "acct_curr_cyc_credit",
                OutOfRange,
            ),
            (
                |row| row.acct_curr_cyc_debit = -eleven_digits(),
                "acct_curr_cyc_debit",
                OutOfRange,
            ),
        ])
    }

    // audit: PIC-001
    // audit: PIC-002
    // audit: PIC-012
    #[test]
    fn into_record_rejects_text_that_is_not_ascii_of_its_char_width() -> TestResult {
        use DecodeKind::{InvalidByte, WidthMismatch};

        assert_rejected(&[
            (
                |row| row.acct_active_status = String::new(),
                "acct_active_status",
                WidthMismatch {
                    expected: 1,
                    actual: 0,
                },
            ),
            (
                |row| row.acct_addr_zip = "A00000000\u{e9}".to_owned(),
                "acct_addr_zip",
                InvalidByte,
            ),
            (
                |row| row.acct_addr_zip = "A000000000 ".to_owned(),
                "acct_addr_zip",
                WidthMismatch {
                    expected: 10,
                    actual: 11,
                },
            ),
            (
                |row| row.acct_group_id = "DEFAULT".to_owned(),
                "acct_group_id",
                WidthMismatch {
                    expected: 10,
                    actual: 7,
                },
            ),
            (
                |row| row.acct_group_id = "\0".repeat(10),
                "acct_group_id",
                InvalidByte,
            ),
            (
                |row| row.filler = " ".repeat(177),
                "filler",
                WidthMismatch {
                    expected: 178,
                    actual: 177,
                },
            ),
            (
                |row| row.filler = format!("\0{}", " ".repeat(177)),
                "filler",
                InvalidByte,
            ),
        ])
    }
}
