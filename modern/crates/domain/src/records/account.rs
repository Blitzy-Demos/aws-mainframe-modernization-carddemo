//! `CVACT01Y` `ACCOUNT-RECORD`, the 300-byte account master record (`app/cpy/CVACT01Y.cpy:4-17`;
//! CVACT01Y-001 to CVACT01Y-013).
//!
//! The record is the account KSDS, keyed on `ACCT-ID` (`KEYS(11 0)`, `RECORDSIZE(300 300)`,
//! `app/jcl/ACCTFILE.jcl:40-41`). The `acctfile` load reads it from `app/data/ASCII/acctdata.txt`,
//! whose 50 lines all carry the full 300-byte body, so the record declares no trailing `FILLER`
//! for short bodies, and table `accounts` (migration 0001) stores it column by column.
//!
//! In every fixture record `ACCT-ADDR-ZIP` holds the disclosure group code and `ACCT-GROUP-ID` is
//! blank (E07). Both are kept byte for byte, as the COBOL reads them; the interest lookup that
//! consequently takes the `DEFAULT` group belongs to `crate::batch`, not to this layout.

use copybook::{Error, FixedRecord, PicDate, PicU, PicX, RecordReader, RecordWriter};

use super::{put_zoned_money, zoned_money};
use crate::money::Money;

/// `ACCOUNT-RECORD` of `app/cpy/CVACT01Y.cpy`: one field per copybook item, in copybook order
/// (CVACT01Y-001 to CVACT01Y-013).
///
/// Amounts are `PIC S9(10)V99` DISPLAY with a trailing overpunch sign, held as [`Money`] with ten
/// integer digits (PIC-005, PIC-006). The three `PIC X(10)` dates are [`PicDate`], `None` for ten
/// spaces (PIC-015, Typed Dates and Timestamps). Every `PIC X(n)` field keeps its bytes verbatim,
/// `FILLER` included (PIC-001, PIC-012). No field is cardholder data, so the derived `Debug`
/// discloses nothing protected (PIC-024).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountRecord {
    /// `ACCT-ID PIC 9(11)`, offset 0, 11 bytes: the account number and KSDS key, 64-bit
    /// (CVACT01Y-001, PIC-003, PIC-023).
    pub id: PicU<11>,
    /// `ACCT-ACTIVE-STATUS PIC X(01)`, offset 11, 1 byte (CVACT01Y-002).
    pub active_status: PicX<1>,
    /// `ACCT-CURR-BAL PIC S9(10)V99`, offset 12, 12 bytes (CVACT01Y-003).
    pub curr_bal: Money<10>,
    /// `ACCT-CREDIT-LIMIT PIC S9(10)V99`, offset 24, 12 bytes (CVACT01Y-004).
    pub credit_limit: Money<10>,
    /// `ACCT-CASH-CREDIT-LIMIT PIC S9(10)V99`, offset 36, 12 bytes (CVACT01Y-005).
    pub cash_credit_limit: Money<10>,
    /// `ACCT-OPEN-DATE PIC X(10)`, offset 48, 10 bytes (CVACT01Y-006).
    pub open_date: PicDate,
    /// `ACCT-EXPIRAION-DATE PIC X(10)`, offset 58, 10 bytes (CVACT01Y-007). The copybook's
    /// spelling is kept, as in column `accounts.acct_expiraion_date`.
    pub expiraion_date: PicDate,
    /// `ACCT-REISSUE-DATE PIC X(10)`, offset 68, 10 bytes (CVACT01Y-008).
    pub reissue_date: PicDate,
    /// `ACCT-CURR-CYC-CREDIT PIC S9(10)V99`, offset 78, 12 bytes (CVACT01Y-009).
    pub curr_cyc_credit: Money<10>,
    /// `ACCT-CURR-CYC-DEBIT PIC S9(10)V99`, offset 90, 12 bytes (CVACT01Y-010).
    pub curr_cyc_debit: Money<10>,
    /// `ACCT-ADDR-ZIP PIC X(10)`, offset 102, 10 bytes (CVACT01Y-011). Every fixture record holds
    /// the disclosure group code here, such as `A000000000`, kept verbatim (E07).
    pub addr_zip: PicX<10>,
    /// `ACCT-GROUP-ID PIC X(10)`, offset 112, 10 bytes (CVACT01Y-012); blank in every fixture
    /// record (E07).
    pub group_id: PicX<10>,
    /// `FILLER PIC X(178)`, offset 122, 178 bytes, whatever they hold (CVACT01Y-013, PIC-012).
    pub filler: PicX<178>,
}

// The field widths, in copybook order, fill the record exactly: `RECLN 300` in the copybook and
// `RECORDSIZE(300 300)` in `ACCTFILE.jcl:41` (CVACT01Y-001 to CVACT01Y-013).
const _: () = assert!(
    11 + 1 + 12 + 12 + 12 + 10 + 10 + 10 + 12 + 12 + 10 + 10 + 178
        == <AccountRecord as FixedRecord>::WIDTH
);

impl FixedRecord for AccountRecord {
    const WIDTH: usize = 300;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        // A struct literal evaluates its fields in the order written: copybook order.
        let record = Self {
            id: r.pic_u("ACCT-ID")?,
            active_status: r.pic_x("ACCT-ACTIVE-STATUS")?,
            curr_bal: zoned_money(&mut r, "ACCT-CURR-BAL")?,
            credit_limit: zoned_money(&mut r, "ACCT-CREDIT-LIMIT")?,
            cash_credit_limit: zoned_money(&mut r, "ACCT-CASH-CREDIT-LIMIT")?,
            open_date: r.date("ACCT-OPEN-DATE")?,
            expiraion_date: r.date("ACCT-EXPIRAION-DATE")?,
            reissue_date: r.date("ACCT-REISSUE-DATE")?,
            curr_cyc_credit: zoned_money(&mut r, "ACCT-CURR-CYC-CREDIT")?,
            curr_cyc_debit: zoned_money(&mut r, "ACCT-CURR-CYC-DEBIT")?,
            addr_zip: r.pic_x("ACCT-ADDR-ZIP")?,
            group_id: r.pic_x("ACCT-GROUP-ID")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_u(self.id).pic_x(&self.active_status);
        put_zoned_money(&mut w, &self.curr_bal);
        put_zoned_money(&mut w, &self.credit_limit);
        put_zoned_money(&mut w, &self.cash_credit_limit);
        w.date(self.open_date)
            .date(self.expiraion_date)
            .date(self.reissue_date);
        put_zoned_money(&mut w, &self.curr_cyc_credit);
        put_zoned_money(&mut w, &self.curr_cyc_debit);
        w.pic_x(&self.addr_zip)
            .pic_x(&self.group_id)
            .pic_x(&self.filler);
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{Error, ErrorKind, FixedRecord, PicDate};

    use super::AccountRecord;
    use crate::money::{Decimal, Money};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Reads one amount field of a decoded record.
    type AmountField = fn(&AccountRecord) -> Money<10>;

    /// The data fields of `acctdata.txt` line 1, offsets 0 to 111; the 188 bytes after them,
    /// `ACCT-GROUP-ID` and `FILLER`, are spaces.
    const LINE_1_DATA: &str = concat!(
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
    );

    /// Spaces on line 1 after its data fields: `ACCT-GROUP-ID` (10) and `FILLER` (178).
    const LINE_1_BLANK_TAIL: usize = 188;

    /// The 300-byte body of `acctdata.txt` line 1, without its LF terminator.
    fn line_1() -> Vec<u8> {
        let mut body = LINE_1_DATA.as_bytes().to_vec();
        body.extend_from_slice(&[b' '; LINE_1_BLANK_TAIL]);
        body
    }

    /// Line 1 with `patch` written over the bytes from offset `at`.
    fn patched(at: usize, patch: &[u8]) -> Result<Vec<u8>, Box<dyn StdError>> {
        let mut body = line_1();
        let end = at.checked_add(patch.len()).ok_or("patch end overflows")?;
        body.get_mut(at..end)
            .ok_or("patch outside the record")?
            .copy_from_slice(patch);
        Ok(body)
    }

    /// `cents` hundredths as a `PIC S9(10)V99` amount.
    fn money(cents: i64) -> Result<Money<10>, Box<dyn StdError>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    /// The calendar date `year`-`month`-`day` as a `PIC X(10)` date.
    fn date(year: i32, month: u32, day: u32) -> Result<PicDate, Box<dyn StdError>> {
        let value = NaiveDate::from_ymd_opt(year, month, day).ok_or("not a calendar date")?;
        Ok(PicDate::new(Some(value))?)
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
    fn fixture_line_1_decodes_field_by_field_and_re_encodes_unchanged() -> TestResult {
        let body = line_1();
        let account = AccountRecord::decode(&body)?;
        assert_eq!(account.id.get(), 1);
        assert_eq!(account.active_status.as_bytes(), b"Y");
        assert_eq!(account.curr_bal, money(19_400)?);
        assert_eq!(account.credit_limit, money(202_000)?);
        assert_eq!(account.cash_credit_limit, money(102_000)?);
        assert_eq!(account.open_date, date(2014, 11, 20)?);
        assert_eq!(account.expiraion_date, date(2025, 5, 20)?);
        assert_eq!(account.reissue_date, date(2025, 5, 20)?);
        assert_eq!(account.curr_cyc_credit, Money::<10>::ZERO);
        assert_eq!(account.curr_cyc_debit, Money::<10>::ZERO);
        assert_eq!(account.addr_zip.as_bytes(), b"A000000000");
        assert!(account.group_id.is_spaces());
        assert!(account.filler.is_spaces());

        let encoded = account.encode();
        assert_eq!(encoded.len(), AccountRecord::WIDTH);
        assert_eq!(encoded, body);
        Ok(())
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn every_amount_keeps_its_sign_byte_and_value() -> TestResult {
        let cases: [(usize, &[u8; 12], AmountField, i64); 5] = [
            // `}` is a negative last digit 0, `R` a negative 9, `J` a negative 1.
            (12, b"00000001940}", |a| a.curr_bal, -19_400),
            (24, b"99999999999I", |a| a.credit_limit, 999_999_999_999),
            (36, b"00000000000J", |a| a.cash_credit_limit, -1),
            (78, b"00000012345R", |a| a.curr_cyc_credit, -123_459),
            (90, b"00000000000A", |a| a.curr_cyc_debit, 1),
        ];
        for (at, image, field, cents) in cases {
            let body = patched(at, image)?;
            let account = AccountRecord::decode(&body)?;
            assert_eq!(field(&account), money(cents)?);
            assert_eq!(account.encode(), body);
        }
        Ok(())
    }

    // audit: PIC-001
    // audit: PIC-012
    // audit: PIC-015
    #[test]
    fn blank_dates_and_any_printable_text_round_trip_verbatim() -> TestResult {
        let blank = b"          ";
        let cases: [(usize, &[u8]); 8] = [
            (48, blank),
            (58, blank),
            (68, blank),
            (68, b"2024-02-29"),
            (11, b"N"),
            (102, b"12345-6789"),
            (112, b"DEFAULT   "),
            (122, b"~filler bytes are kept, whatever they hold~"),
        ];
        for (at, patch) in cases {
            let body = patched(at, patch)?;
            assert_eq!(AccountRecord::decode(&body)?.encode(), body);
        }

        let account = AccountRecord::decode(&patched(48, blank)?)?;
        assert_eq!(account.open_date, PicDate::BLANK);
        let account = AccountRecord::decode(&patched(68, b"2024-02-29")?)?;
        assert_eq!(account.reissue_date, date(2024, 2, 29)?);
        let account = AccountRecord::decode(&patched(112, b"DEFAULT   ")?)?;
        assert_eq!(account.group_id.as_bytes(), b"DEFAULT   ");
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-004
    // audit: PIC-007
    // audit: PIC-008
    // audit: PIC-009
    // audit: PIC-016
    #[test]
    fn malformed_fields_are_typed_errors_naming_the_field() -> TestResult {
        use copybook::ErrorKind::{
            InvalidDate, InvalidOverpunch, NegativeZero, NonDigit, NonPrintable, PlainDigitSign,
        };

        let cases: [(usize, &[u8], Error); 15] = [
            (0, b"0000000000X", Error::new("ACCT-ID", 0, 11, NonDigit)),
            (0, b" ", Error::new("ACCT-ID", 0, 11, NonDigit)),
            (
                11,
                b"\t",
                Error::new("ACCT-ACTIVE-STATUS", 11, 1, NonPrintable),
            ),
            // R7: a plain digit in the sign position is never read as a positive value.
            (
                23,
                b"0",
                Error::new("ACCT-CURR-BAL", 12, 12, PlainDigitSign),
            ),
            (
                24,
                b"00000000000}",
                Error::new("ACCT-CREDIT-LIMIT", 24, 12, NegativeZero),
            ),
            (
                47,
                b"S",
                Error::new("ACCT-CASH-CREDIT-LIMIT", 36, 12, InvalidOverpunch),
            ),
            (
                48,
                b"2014-13-20",
                Error::new("ACCT-OPEN-DATE", 48, 10, InvalidDate),
            ),
            (
                58,
                b"2025-02-30",
                Error::new("ACCT-EXPIRAION-DATE", 58, 10, InvalidDate),
            ),
            (
                68,
                b"2025-05-2 ",
                Error::new("ACCT-REISSUE-DATE", 68, 10, InvalidDate),
            ),
            (
                78,
                b" ",
                Error::new("ACCT-CURR-CYC-CREDIT", 78, 12, NonDigit),
            ),
            (
                101,
                b"5",
                Error::new("ACCT-CURR-CYC-DEBIT", 90, 12, PlainDigitSign),
            ),
            (
                102,
                b"\x7F",
                Error::new("ACCT-ADDR-ZIP", 102, 10, NonPrintable),
            ),
            (
                112,
                b"\r",
                Error::new("ACCT-GROUP-ID", 112, 10, NonPrintable),
            ),
            (299, b"\0", Error::new("FILLER", 122, 178, NonPrintable)),
            (299, b"\xC1", Error::new("FILLER", 122, 178, NonPrintable)),
        ];
        for (at, patch, error) in cases {
            assert_eq!(AccountRecord::decode(&patched(at, patch)?), Err(error));
        }
        Ok(())
    }

    #[test]
    fn only_a_300_byte_slice_decodes() -> TestResult {
        let mut two_lines = line_1();
        two_lines.extend_from_slice(&line_1());
        for len in 0..=two_lines.len() {
            let slice = two_lines.get(..len).ok_or("prefix outside the buffer")?;
            let decoded = AccountRecord::decode(slice);
            if len == AccountRecord::WIDTH {
                assert!(decoded.is_ok());
            } else {
                let mismatch = ErrorKind::WidthMismatch {
                    expected: AccountRecord::WIDTH,
                    actual: len,
                };
                assert_eq!(
                    decoded,
                    Err(Error::new("record", 0, AccountRecord::WIDTH, mismatch)),
                );
            }
        }
        Ok(())
    }
}
