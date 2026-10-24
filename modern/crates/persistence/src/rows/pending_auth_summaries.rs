//! Codec B for `pending_auth_summaries`: the `CIPAUSMY` summary, IMS root segment `PAUTSUM0` of
//! `DBPAUTP0`, as one row per account (migration 0004; `CIPAUSMY.cpy:19-31`,
//! `DBPAUTP0.dbd:28-30`; CIPAUSMY-001 to CIPAUSMY-013).
//!
//! The root key `ACCNTID`, `PA-ACCT-ID PIC S9(11) COMP-3`, is the primary key `acct_id`. It is
//! signed, so its `BIGINT` range check is symmetric, and it is 64-bit like the `PIC 9(11)` key of
//! `accounts`, so the two join on one column type and no value narrows on the way (CIPAUSMY-001,
//! PIC-010, PIC-023; Checked Integer Narrowing). The packed `COMP-3` and binary `COMP` byte forms
//! belong to codec A: a row holds the field values, never their bytes.
//!
//! `PA-ACCOUNT-STATUS PIC X(02) OCCURS 5 TIMES` becomes the columns `account_status_1` to
//! `account_status_5`, occurrence k being array element k − 1. The array is destructured and
//! assembled whole, never indexed, so no conversion can panic (CIPAUSMY-004; Total Parsers Over
//! Untrusted Input).
//!
//! `approved_auth_amt` (`PA-APPROVED-AUTH-AMT`) is the authoritative pending sum of the
//! credit-limit invariant i10: CP00's guard reads it from this row while the deciding transaction
//! holds the row lock that `PendingAuthSummaryRepository::lock_or_create` takes, and never sums
//! detail rows for it (AAP 0.3.4.2; CIPAUSMY-011). This module only converts the value. The
//! insert and the lock are statements of `repos`; this file holds no SQL (R4).

use domain::records::PendingAuthSummaryRecord;
use rust_decimal::Decimal;

use super::{
    count_to_i16, decimal_to_money, i16_to_count, i64_to_pic_s, i64_to_pic_u, money_to_decimal,
    pic_s_to_i64, pic_u_to_i64, pic_x_to_string, string_to_pic_x,
};
use crate::error::PersistenceError;

/// One row of `pending_auth_summaries`, its columns in table order, each the codec B form of one
/// `CIPAUSMY` field. `Debug` is left underived, as for every row struct.
#[derive(Clone, PartialEq, Eq)]
pub struct PendingAuthSummaryRow {
    /// `acct_id BIGINT`, the primary key: `PA-ACCT-ID PIC S9(11) COMP-3` (CIPAUSMY-001, PIC-010).
    pub(crate) acct_id: i64,
    /// `cust_id BIGINT`: `PA-CUST-ID PIC 9(09)` (CIPAUSMY-002, PIC-003).
    pub(crate) cust_id: i64,
    /// `auth_status CHAR(1)`: `PA-AUTH-STATUS PIC X(01)` (CIPAUSMY-003, PIC-001).
    pub(crate) auth_status: String,
    /// `account_status_1 CHAR(2)`: `PA-ACCOUNT-STATUS(1)` (CIPAUSMY-004).
    pub(crate) account_status_1: String,
    /// `account_status_2 CHAR(2)`: `PA-ACCOUNT-STATUS(2)` (CIPAUSMY-004).
    pub(crate) account_status_2: String,
    /// `account_status_3 CHAR(2)`: `PA-ACCOUNT-STATUS(3)` (CIPAUSMY-004).
    pub(crate) account_status_3: String,
    /// `account_status_4 CHAR(2)`: `PA-ACCOUNT-STATUS(4)` (CIPAUSMY-004).
    pub(crate) account_status_4: String,
    /// `account_status_5 CHAR(2)`: `PA-ACCOUNT-STATUS(5)` (CIPAUSMY-004).
    pub(crate) account_status_5: String,
    /// `credit_limit NUMERIC(11,2)`: `PA-CREDIT-LIMIT PIC S9(09)V99 COMP-3` (CIPAUSMY-005).
    pub(crate) credit_limit: Decimal,
    /// `cash_limit NUMERIC(11,2)`: `PA-CASH-LIMIT PIC S9(09)V99 COMP-3` (CIPAUSMY-006).
    pub(crate) cash_limit: Decimal,
    /// `credit_balance NUMERIC(11,2)`: `PA-CREDIT-BALANCE PIC S9(09)V99 COMP-3` (CIPAUSMY-007).
    pub(crate) credit_balance: Decimal,
    /// `cash_balance NUMERIC(11,2)`: `PA-CASH-BALANCE PIC S9(09)V99 COMP-3` (CIPAUSMY-008).
    pub(crate) cash_balance: Decimal,
    /// `approved_auth_cnt SMALLINT`: `PA-APPROVED-AUTH-CNT PIC S9(04) COMP` (CIPAUSMY-009,
    /// PIC-011).
    pub(crate) approved_auth_cnt: i16,
    /// `declined_auth_cnt SMALLINT`: `PA-DECLINED-AUTH-CNT PIC S9(04) COMP` (CIPAUSMY-010,
    /// PIC-011).
    pub(crate) declined_auth_cnt: i16,
    /// `approved_auth_amt NUMERIC(11,2)`: `PA-APPROVED-AUTH-AMT PIC S9(09)V99 COMP-3`, the
    /// authoritative pending sum of i10 (CIPAUSMY-011).
    pub(crate) approved_auth_amt: Decimal,
    /// `declined_auth_amt NUMERIC(11,2)`: `PA-DECLINED-AUTH-AMT PIC S9(09)V99 COMP-3`
    /// (CIPAUSMY-012).
    pub(crate) declined_auth_amt: Decimal,
    /// `filler CHAR(34)`: `FILLER PIC X(34)`, byte for byte (CIPAUSMY-013, PIC-012).
    pub(crate) filler: String,
}

impl PendingAuthSummaryRow {
    /// The row that stores `record`. Every value is kept exactly: the key and customer as
    /// `BIGINT`, the amounts at scale 2, the counts as `SMALLINT` and every text byte, padding
    /// and `FILLER` contents included (CIPAUSMY-001 to CIPAUSMY-013).
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte), naming the column,
    /// for a NUL or non-ASCII byte in `PA-AUTH-STATUS`, a `PA-ACCOUNT-STATUS` occurrence or
    /// `FILLER`, which no `CHAR(n)` column stores byte for byte (PIC-002). The integral, money
    /// and count fields always fit their columns.
    pub fn from_record(record: &PendingAuthSummaryRecord) -> Result<Self, PersistenceError> {
        let [status_1, status_2, status_3, status_4, status_5] = &record.account_status;
        Ok(Self {
            acct_id: pic_s_to_i64(record.acct_id),
            cust_id: pic_u_to_i64("cust_id", record.cust_id)?,
            auth_status: pic_x_to_string("auth_status", &record.auth_status)?,
            account_status_1: pic_x_to_string("account_status_1", status_1)?,
            account_status_2: pic_x_to_string("account_status_2", status_2)?,
            account_status_3: pic_x_to_string("account_status_3", status_3)?,
            account_status_4: pic_x_to_string("account_status_4", status_4)?,
            account_status_5: pic_x_to_string("account_status_5", status_5)?,
            credit_limit: money_to_decimal(record.credit_limit),
            cash_limit: money_to_decimal(record.cash_limit),
            credit_balance: money_to_decimal(record.credit_balance),
            cash_balance: money_to_decimal(record.cash_balance),
            approved_auth_cnt: count_to_i16(record.approved_auth_cnt),
            declined_auth_cnt: count_to_i16(record.declined_auth_cnt),
            approved_auth_amt: money_to_decimal(record.approved_auth_amt),
            declined_auth_amt: money_to_decimal(record.declined_auth_amt),
            filler: pic_x_to_string("filler", &record.filler)?,
        })
    }

    /// The `CIPAUSMY` record this row stores, equal to the record [`Self::from_record`] was
    /// given. A value the picture cannot hold is an error, never truncated, padded or trimmed
    /// (R7).
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the column:
    ///
    /// - [`DecodeKind::OutOfRange`](crate::error::DecodeKind::OutOfRange) for an `acct_id`
    ///   whose magnitude exceeds 11 digits, a `cust_id` that is negative or exceeds 9 digits, or
    ///   an amount with more than 9 integer digits or a non-zero third decimal place.
    /// - [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) for
    ///   `auth_status`, an `account_status_k` or `filler` whose length is not its picture width
    ///   of 1, 2 or 34 bytes.
    /// - [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for a NUL or
    ///   non-ASCII character in one of those text columns.
    ///
    /// Every `SMALLINT` is a valid count, so the two counts never fail (PIC-011).
    pub fn into_record(self) -> Result<PendingAuthSummaryRecord, PersistenceError> {
        Ok(PendingAuthSummaryRecord {
            acct_id: i64_to_pic_s::<11>("acct_id", self.acct_id)?,
            cust_id: i64_to_pic_u::<9>("cust_id", self.cust_id)?,
            auth_status: string_to_pic_x::<1>("auth_status", &self.auth_status)?,
            account_status: [
                string_to_pic_x::<2>("account_status_1", &self.account_status_1)?,
                string_to_pic_x::<2>("account_status_2", &self.account_status_2)?,
                string_to_pic_x::<2>("account_status_3", &self.account_status_3)?,
                string_to_pic_x::<2>("account_status_4", &self.account_status_4)?,
                string_to_pic_x::<2>("account_status_5", &self.account_status_5)?,
            ],
            credit_limit: decimal_to_money::<9>("credit_limit", self.credit_limit)?,
            cash_limit: decimal_to_money::<9>("cash_limit", self.cash_limit)?,
            credit_balance: decimal_to_money::<9>("credit_balance", self.credit_balance)?,
            cash_balance: decimal_to_money::<9>("cash_balance", self.cash_balance)?,
            approved_auth_cnt: i16_to_count(self.approved_auth_cnt),
            declined_auth_cnt: i16_to_count(self.declined_auth_cnt),
            approved_auth_amt: decimal_to_money::<9>("approved_auth_amt", self.approved_auth_amt)?,
            declined_auth_amt: decimal_to_money::<9>("declined_auth_amt", self.declined_auth_amt)?,
            filler: string_to_pic_x::<34>("filler", &self.filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Count, PicS, PicU, PicX};
    use domain::money::Money;
    use domain::records::PendingAuthSummaryRecord;
    use rust_decimal::Decimal;

    use super::PendingAuthSummaryRow;
    use crate::error::{DecodeKind, PersistenceError};
    use crate::rows::pic_u_to_i64;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A change that leaves a stored row holding a value its picture cannot hold.
    type CorruptRow = fn(&mut PendingAuthSummaryRow);

    /// A change that leaves a record holding a byte no `CHAR(n)` column stores.
    type TaintRecord = fn(&mut PendingAuthSummaryRecord);

    /// Two printable bytes at the top of the ASCII range, for every occurrence of the extremes.
    const TILDES: PicX<2> = PicX::from_array(*b"~~");

    /// The amount of `cents` hundredths.
    fn money(cents: i64) -> Result<Money<9>, Box<dyn StdError>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    fn width(expected: usize, actual: usize) -> DecodeKind {
        DecodeKind::WidthMismatch { expected, actual }
    }

    /// A summary whose neighbouring fields all differ: distinct status occurrences, a negative
    /// credit balance, the counts 0 and 9999, and the largest and a negative amount.
    fn sample_record() -> Result<PendingAuthSummaryRecord, Box<dyn StdError>> {
        Ok(PendingAuthSummaryRecord {
            acct_id: PicS::new(50)?,
            cust_id: PicU::new(50)?,
            auth_status: PicX::from_array(*b"A"),
            account_status: [
                PicX::from_array(*b"A1"),
                PicX::from_array(*b"B2"),
                PicX::from_array(*b"C3"),
                PicX::from_array(*b"D4"),
                PicX::from_array(*b"E5"),
            ],
            credit_limit: money(500_000)?,
            cash_limit: money(150_000)?,
            credit_balance: money(-123_456)?,
            cash_balance: Money::ZERO,
            approved_auth_cnt: Count::new(0),
            declined_auth_cnt: Count::new(Count::MAX),
            approved_auth_amt: money(99_999_999_999)?,
            declined_auth_amt: money(-1)?,
            filler: PicX::from_array(*b"FILLER KEPT AS STORED, NOT BLANKED"),
        })
    }

    // audit: CIPAUSMY-001
    // audit: CIPAUSMY-002
    // audit: CIPAUSMY-003
    // audit: CIPAUSMY-004
    // audit: CIPAUSMY-005
    // audit: CIPAUSMY-006
    // audit: CIPAUSMY-007
    // audit: CIPAUSMY-008
    // audit: CIPAUSMY-009
    // audit: CIPAUSMY-010
    // audit: CIPAUSMY-011
    // audit: CIPAUSMY-012
    // audit: CIPAUSMY-013
    #[test]
    fn every_field_maps_to_its_column_and_back() -> TestResult {
        let record = sample_record()?;
        let row = PendingAuthSummaryRow::from_record(&record)?;
        assert_eq!((row.acct_id, row.cust_id), (50, 50));
        assert_eq!(row.auth_status, "A");
        assert_eq!(
            [
                row.account_status_1.as_str(),
                row.account_status_2.as_str(),
                row.account_status_3.as_str(),
                row.account_status_4.as_str(),
                row.account_status_5.as_str(),
            ],
            ["A1", "B2", "C3", "D4", "E5"]
        );
        let amounts = [
            (row.credit_limit, 500_000),
            (row.cash_limit, 150_000),
            (row.credit_balance, -123_456),
            (row.cash_balance, 0),
            (row.approved_auth_amt, 99_999_999_999),
            (row.declined_auth_amt, -1),
        ];
        for (column, cents) in amounts {
            assert_eq!(column, Decimal::new(cents, 2));
            assert_eq!(column.scale(), 2);
        }
        assert_eq!((row.approved_auth_cnt, row.declined_auth_cnt), (0, 9_999));
        assert_eq!(row.filler.as_bytes(), record.filler.as_bytes());

        let back = row.clone().into_record()?;
        assert_eq!(back, record);
        assert!(PendingAuthSummaryRow::from_record(&back)? == row);
        Ok(())
    }

    // audit: PIC-010
    // audit: PIC-011
    // audit: PIC-012
    #[test]
    fn extreme_values_round_trip() -> TestResult {
        let largest = money(99_999_999_999)?;
        let smallest = money(-99_999_999_999)?;
        let cases = [
            (-99_999_999_999, (i16::MIN, i16::MAX), smallest),
            (99_999_999_999, (i16::MAX, i16::MIN), largest),
            (0, (-Count::MAX, 0), Money::ZERO),
        ];
        for (acct_id, (approved, declined), amount) in cases {
            let record = PendingAuthSummaryRecord {
                acct_id: PicS::new(acct_id)?,
                cust_id: PicU::new(PicU::<9>::MAX)?,
                auth_status: PicX::from_array(*b"~"),
                account_status: [TILDES; 5],
                credit_limit: amount,
                cash_limit: amount,
                credit_balance: amount,
                cash_balance: amount,
                approved_auth_cnt: Count::new(approved),
                declined_auth_cnt: Count::new(declined),
                approved_auth_amt: amount,
                declined_auth_amt: amount,
                filler: PicX::from_array([b'~'; 34]),
            };
            let row = PendingAuthSummaryRow::from_record(&record)?;
            assert_eq!(row.acct_id, acct_id);
            assert_eq!(row.cust_id, 999_999_999);
            assert_eq!(
                (row.approved_auth_cnt, row.declined_auth_cnt),
                (approved, declined)
            );
            assert_eq!(row.into_record()?, record);
        }
        Ok(())
    }

    /// The row CP00's first authorization for an account creates holds zero amounts and counts,
    /// and its signed key is the account's `BIGINT` key value (CIPAUSMY-001, CIPAUSMY-011).
    // audit: CIPAUSMY-001
    // audit: CIPAUSMY-011
    // audit: PIC-023
    #[test]
    fn initialized_summary_is_zero_and_keyed_by_the_account() -> TestResult {
        let account = PicU::<11>::new(50)?;
        let record =
            PendingAuthSummaryRecord::initialized(PicS::<11>::from(account), PicU::new(50)?);
        let row = PendingAuthSummaryRow::from_record(&record)?;
        assert_eq!(row.acct_id, pic_u_to_i64("acct_id", account)?);
        for amount in [
            row.credit_limit,
            row.cash_limit,
            row.credit_balance,
            row.cash_balance,
            row.approved_auth_amt,
            row.declined_auth_amt,
        ] {
            assert!(amount.is_zero());
            assert_eq!(amount.scale(), 2);
        }
        assert_eq!((row.approved_auth_cnt, row.declined_auth_cnt), (0, 0));
        assert_eq!(row.into_record()?, record);
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-003
    // audit: PIC-005
    // audit: PIC-010
    #[test]
    fn stored_values_their_picture_cannot_hold_fail_naming_the_column() -> TestResult {
        let cases: [(CorruptRow, &'static str, DecodeKind); 19] = [
            (
                |row| row.acct_id = 100_000_000_000,
                "acct_id",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.acct_id = -100_000_000_000,
                "acct_id",
                DecodeKind::OutOfRange,
            ),
            (|row| row.cust_id = -1, "cust_id", DecodeKind::OutOfRange),
            (
                |row| row.cust_id = 1_000_000_000,
                "cust_id",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.auth_status = String::new(),
                "auth_status",
                width(1, 0),
            ),
            (
                |row| row.account_status_1 = "ABC".to_owned(),
                "account_status_1",
                width(2, 3),
            ),
            (
                |row| row.account_status_2 = "A\0".to_owned(),
                "account_status_2",
                DecodeKind::InvalidByte,
            ),
            (
                |row| row.account_status_3 = "X".to_owned(),
                "account_status_3",
                width(2, 1),
            ),
            (
                |row| row.account_status_4 = String::new(),
                "account_status_4",
                width(2, 0),
            ),
            (
                |row| row.account_status_5 = "\u{e9}".to_owned(),
                "account_status_5",
                DecodeKind::InvalidByte,
            ),
            // Ten integer digits: 1000000000.00 fits `S9(10)V99`, not `S9(09)V99`.
            (
                |row| row.credit_limit = Decimal::new(100_000_000_000, 2),
                "credit_limit",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.cash_limit = Decimal::new(-100_000_000_000, 2),
                "cash_limit",
                DecodeKind::OutOfRange,
            ),
            // A non-zero third decimal place.
            (
                |row| row.credit_balance = Decimal::new(-1_005, 3),
                "credit_balance",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.cash_balance = Decimal::new(100_000_000_000, 2),
                "cash_balance",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.approved_auth_amt = Decimal::new(100_000_000_000, 2),
                "approved_auth_amt",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.declined_auth_amt = Decimal::new(1, 3),
                "declined_auth_amt",
                DecodeKind::OutOfRange,
            ),
            (|row| row.filler = " ".repeat(33), "filler", width(34, 33)),
            (|row| row.filler = " ".repeat(35), "filler", width(34, 35)),
            (
                |row| row.filler = format!("{:<34}", "\0"),
                "filler",
                DecodeKind::InvalidByte,
            ),
        ];
        let valid = PendingAuthSummaryRow::from_record(&sample_record()?)?;
        for (corrupt, column, kind) in cases {
            let mut row = valid.clone();
            corrupt(&mut row);
            assert_eq!(
                row.into_record().map(drop),
                Err(PersistenceError::decode(column, kind)),
                "{column}"
            );
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn record_bytes_no_column_can_store_fail_naming_the_column() -> TestResult {
        let cases: [(TaintRecord, &'static str); 7] = [
            (|r| r.auth_status = PicX::from_array([0]), "auth_status"),
            (
                |r| {
                    let [status, ..] = &mut r.account_status;
                    *status = PicX::from_array([b'A', 0]);
                },
                "account_status_1",
            ),
            (
                |r| {
                    let [_, status, ..] = &mut r.account_status;
                    *status = PicX::from_array([0x80, b' ']);
                },
                "account_status_2",
            ),
            (
                |r| {
                    let [_, _, status, ..] = &mut r.account_status;
                    *status = PicX::from_array([0, 0]);
                },
                "account_status_3",
            ),
            (
                |r| {
                    let [.., status, _] = &mut r.account_status;
                    *status = PicX::from_array([b' ', 0xFF]);
                },
                "account_status_4",
            ),
            (
                |r| {
                    let [.., status] = &mut r.account_status;
                    *status = PicX::from_array([0xC3, 0xA9]);
                },
                "account_status_5",
            ),
            (|r| r.filler = PicX::from_array([0x80; 34]), "filler"),
        ];
        for (taint, column) in cases {
            let mut record = sample_record()?;
            taint(&mut record);
            assert_eq!(
                PendingAuthSummaryRow::from_record(&record).map(drop),
                Err(PersistenceError::decode(column, DecodeKind::InvalidByte)),
                "{column}"
            );
        }
        Ok(())
    }
}
