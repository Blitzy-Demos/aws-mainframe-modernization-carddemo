//! Codec B for table `pending_auth_details` (migration 0004): the pending-authorization detail
//! `CIPAUDTY`, IMS child segment `PAUTDTL1` (`CIPAUDTY.cpy:19-54`, `DBPAUTP0.dbd:36-37`;
//! CIPAUDTY-001 to CIPAUDTY-029).
//!
//! - **Key.** The primary key is the parent segment's `ACCNTID`, column `acct_id`, followed by
//!   the unique sequence field `PAUT9CTS`, columns `auth_date_9c` and `auth_time_9c`. The
//!   account lies outside the 200-byte segment (CIPAUDTY-029), so it travels beside the record:
//!   [`PendingAuthDetailRow::from_record`] takes it and [`PendingAuthDetailRow::into_record`]
//!   returns it. The two key parts are stored as the record holds them, already inverted
//!   (`99999 − YYDDD`, `999999999 − HHMMSSmmm`; CIPAUDTY-001, CIPAUDTY-002, COPAUA0C-098), so
//!   ascending key order is the IMS twin order, newest first. All three are `BIGINT` with
//!   symmetric range checks (PIC-010, PIC-023).
//! - **Card number.** `PA-CARD-NUM` is stored in full in `card_num CHAR(16)`, reached only
//!   through [`CardNumber::expose`] and read back through [`CardNumber::new`], codec A's
//!   constructor, which keeps the 16 bytes as stored (CIPAUDTY-005, PIC-024). The row therefore
//!   leaves `Debug`, `Clone` and `Serialize` underived, and no error names a value: only egress
//!   is redacted, and a row is never egress (Cardholder Data Redaction, E32).
//! - **Text.** Every `PIC X(n)` field, `FILLER` included, is `CHAR(n)` text kept byte for byte
//!   (PIC-001, PIC-012). `PA-AUTH-ORIG-DATE`, `PA-AUTH-ORIG-TIME`, `PA-CARD-EXPIRY-DATE` and
//!   `PA-FRAUD-RPT-DATE` look like dates but are `X(06)`, `X(04)` and `X(08)`, outside Typed
//!   Dates and Timestamps, so they stay text as received (CIPAUDTY-003, CIPAUDTY-004,
//!   CIPAUDTY-007, CIPAUDTY-027).
//! - **Numbers.** `PA-PROCESSING-CODE PIC 9(06)` and `PA-POS-ENTRY-MODE PIC 9(02)` are `BIGINT`
//!   with `0 .. 10^n − 1` checks (CIPAUDTY-013, CIPAUDTY-018, PIC-003); the two `S9(10)V99
//!   COMP-3` amounts are `NUMERIC(12,2)` at scale 2 (CIPAUDTY-014, CIPAUDTY-015).
//!
//! The row's fields are the column list of `queries/pending_auth_details_page_forward.sql`,
//! which the COPAUS0C browse maps into this row by name (Bounded Pagination), and of every other
//! statement `repos` issues on the table.

use copybook::PicS;
use domain::records::PendingAuthDetailRecord;
use domain::sensitive::CardNumber;
use rust_decimal::Decimal;

use super::{
    decimal_to_money, i64_to_pic_s, i64_to_pic_u, money_to_decimal, pic_s_to_i64, pic_u_to_i64,
    pic_x_to_string, string_to_pic_x,
};
use crate::error::PersistenceError;

/// One row of `pending_auth_details`, its fields the table's columns in migration order.
///
/// `Debug`, `Clone` and `Serialize` stay underived: `card_num` holds the full card number
/// (Cardholder Data Redaction, E32).
pub struct PendingAuthDetailRow {
    /// Parent key `ACCNTID`, `PA-ACCT-ID PIC S9(11) COMP-3` of the summary (CIPAUDTY-029).
    pub(crate) acct_id: i64,
    /// `PA-AUTH-DATE-9C PIC S9(05) COMP-3`, the inverted date (CIPAUDTY-001).
    pub(crate) auth_date_9c: i64,
    /// `PA-AUTH-TIME-9C PIC S9(09) COMP-3`, the inverted time (CIPAUDTY-002).
    pub(crate) auth_time_9c: i64,
    /// `PA-AUTH-ORIG-DATE PIC X(06)` (CIPAUDTY-003).
    pub(crate) auth_orig_date: String,
    /// `PA-AUTH-ORIG-TIME PIC X(06)` (CIPAUDTY-004).
    pub(crate) auth_orig_time: String,
    /// `PA-CARD-NUM PIC X(16)`, in full (CIPAUDTY-005, PIC-024).
    pub(crate) card_num: String,
    /// `PA-AUTH-TYPE PIC X(04)` (CIPAUDTY-006).
    pub(crate) auth_type: String,
    /// `PA-CARD-EXPIRY-DATE PIC X(04)` (CIPAUDTY-007).
    pub(crate) card_expiry_date: String,
    /// `PA-MESSAGE-TYPE PIC X(06)` (CIPAUDTY-008).
    pub(crate) message_type: String,
    /// `PA-MESSAGE-SOURCE PIC X(06)` (CIPAUDTY-009).
    pub(crate) message_source: String,
    /// `PA-AUTH-ID-CODE PIC X(06)` (CIPAUDTY-010).
    pub(crate) auth_id_code: String,
    /// `PA-AUTH-RESP-CODE PIC X(02)` (CIPAUDTY-011).
    pub(crate) auth_resp_code: String,
    /// `PA-AUTH-RESP-REASON PIC X(04)` (CIPAUDTY-012).
    pub(crate) auth_resp_reason: String,
    /// `PA-PROCESSING-CODE PIC 9(06)` (CIPAUDTY-013).
    pub(crate) processing_code: i64,
    /// `PA-TRANSACTION-AMT PIC S9(10)V99 COMP-3` (CIPAUDTY-014).
    pub(crate) transaction_amt: Decimal,
    /// `PA-APPROVED-AMT PIC S9(10)V99 COMP-3` (CIPAUDTY-015).
    pub(crate) approved_amt: Decimal,
    /// `PA-MERCHANT-CATAGORY-CODE PIC X(04)`, the COBOL spelling kept (CIPAUDTY-016).
    pub(crate) merchant_catagory_code: String,
    /// `PA-ACQR-COUNTRY-CODE PIC X(03)` (CIPAUDTY-017).
    pub(crate) acqr_country_code: String,
    /// `PA-POS-ENTRY-MODE PIC 9(02)` (CIPAUDTY-018).
    pub(crate) pos_entry_mode: i64,
    /// `PA-MERCHANT-ID PIC X(15)` (CIPAUDTY-019).
    pub(crate) merchant_id: String,
    /// `PA-MERCHANT-NAME PIC X(22)` (CIPAUDTY-020).
    pub(crate) merchant_name: String,
    /// `PA-MERCHANT-CITY PIC X(13)` (CIPAUDTY-021).
    pub(crate) merchant_city: String,
    /// `PA-MERCHANT-STATE PIC X(02)` (CIPAUDTY-022).
    pub(crate) merchant_state: String,
    /// `PA-MERCHANT-ZIP PIC X(09)` (CIPAUDTY-023).
    pub(crate) merchant_zip: String,
    /// `PA-TRANSACTION-ID PIC X(15)` (CIPAUDTY-024).
    pub(crate) transaction_id: String,
    /// `PA-MATCH-STATUS PIC X(01)` (CIPAUDTY-025).
    pub(crate) match_status: String,
    /// `PA-AUTH-FRAUD PIC X(01)` (CIPAUDTY-026).
    pub(crate) auth_fraud: String,
    /// `PA-FRAUD-RPT-DATE PIC X(08)`, text rather than a typed date (CIPAUDTY-027).
    pub(crate) fraud_rpt_date: String,
    /// `FILLER PIC X(17)`, whatever bytes it holds (CIPAUDTY-028, PIC-012).
    pub(crate) filler: String,
}

impl PendingAuthDetailRow {
    /// The row that stores `record` under the summary whose root key is `acct`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the column, with [`crate::error::DecodeKind`]
    /// `InvalidByte` for a NUL or non-ASCII byte in a `PIC X(n)` field, the card number included
    /// (PIC-002). The error never holds the value (PIC-024).
    pub fn from_record(
        acct: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> Result<Self, PersistenceError> {
        Ok(Self {
            acct_id: pic_s_to_i64(acct),
            auth_date_9c: pic_s_to_i64(record.auth_date_9c),
            auth_time_9c: pic_s_to_i64(record.auth_time_9c),
            auth_orig_date: pic_x_to_string("auth_orig_date", &record.orig_date)?,
            auth_orig_time: pic_x_to_string("auth_orig_time", &record.orig_time)?,
            card_num: pic_x_to_string("card_num", record.card_num.expose())?,
            auth_type: pic_x_to_string("auth_type", &record.auth_type)?,
            card_expiry_date: pic_x_to_string("card_expiry_date", &record.card_expiry_date)?,
            message_type: pic_x_to_string("message_type", &record.message_type)?,
            message_source: pic_x_to_string("message_source", &record.message_source)?,
            auth_id_code: pic_x_to_string("auth_id_code", &record.auth_id_code)?,
            auth_resp_code: pic_x_to_string("auth_resp_code", &record.auth_resp_code)?,
            auth_resp_reason: pic_x_to_string("auth_resp_reason", &record.auth_resp_reason)?,
            processing_code: pic_u_to_i64("processing_code", record.processing_code)?,
            transaction_amt: money_to_decimal(record.transaction_amt),
            approved_amt: money_to_decimal(record.approved_amt),
            merchant_catagory_code: pic_x_to_string(
                "merchant_catagory_code",
                &record.merchant_catagory_code,
            )?,
            acqr_country_code: pic_x_to_string("acqr_country_code", &record.acqr_country_code)?,
            pos_entry_mode: pic_u_to_i64("pos_entry_mode", record.pos_entry_mode)?,
            merchant_id: pic_x_to_string("merchant_id", &record.merchant_id)?,
            merchant_name: pic_x_to_string("merchant_name", &record.merchant_name)?,
            merchant_city: pic_x_to_string("merchant_city", &record.merchant_city)?,
            merchant_state: pic_x_to_string("merchant_state", &record.merchant_state)?,
            merchant_zip: pic_x_to_string("merchant_zip", &record.merchant_zip)?,
            transaction_id: pic_x_to_string("transaction_id", &record.transaction_id)?,
            match_status: pic_x_to_string("match_status", &record.match_status)?,
            auth_fraud: pic_x_to_string("auth_fraud", &record.auth_fraud)?,
            fraud_rpt_date: pic_x_to_string("fraud_rpt_date", &record.fraud_rpt_date)?,
            filler: pic_x_to_string("filler", &record.filler)?,
        })
    }

    /// The summary root key and the detail record this row stores, equal to the pair
    /// [`Self::from_record`] wrote.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the column, with [`crate::error::DecodeKind`]:
    ///
    /// - `OutOfRange` for a key or `9(n)` value outside its picture, or an amount of `10^10` or
    ///   more or with a non-zero third decimal (PIC-010, PIC-023);
    /// - `WidthMismatch` for text that is not exactly its `CHAR(n)` width;
    /// - `InvalidByte` for a NUL or non-ASCII character (PIC-002).
    ///
    /// The error never holds the value, which can be the card number (PIC-024).
    pub fn into_record(self) -> Result<(PicS<11>, PendingAuthDetailRecord), PersistenceError> {
        let acct = i64_to_pic_s("acct_id", self.acct_id)?;
        let record = PendingAuthDetailRecord {
            auth_date_9c: i64_to_pic_s("auth_date_9c", self.auth_date_9c)?,
            auth_time_9c: i64_to_pic_s("auth_time_9c", self.auth_time_9c)?,
            orig_date: string_to_pic_x("auth_orig_date", &self.auth_orig_date)?,
            orig_time: string_to_pic_x("auth_orig_time", &self.auth_orig_time)?,
            card_num: CardNumber::new(string_to_pic_x("card_num", &self.card_num)?),
            auth_type: string_to_pic_x("auth_type", &self.auth_type)?,
            card_expiry_date: string_to_pic_x("card_expiry_date", &self.card_expiry_date)?,
            message_type: string_to_pic_x("message_type", &self.message_type)?,
            message_source: string_to_pic_x("message_source", &self.message_source)?,
            auth_id_code: string_to_pic_x("auth_id_code", &self.auth_id_code)?,
            auth_resp_code: string_to_pic_x("auth_resp_code", &self.auth_resp_code)?,
            auth_resp_reason: string_to_pic_x("auth_resp_reason", &self.auth_resp_reason)?,
            processing_code: i64_to_pic_u("processing_code", self.processing_code)?,
            transaction_amt: decimal_to_money("transaction_amt", self.transaction_amt)?,
            approved_amt: decimal_to_money("approved_amt", self.approved_amt)?,
            merchant_catagory_code: string_to_pic_x(
                "merchant_catagory_code",
                &self.merchant_catagory_code,
            )?,
            acqr_country_code: string_to_pic_x("acqr_country_code", &self.acqr_country_code)?,
            pos_entry_mode: i64_to_pic_u("pos_entry_mode", self.pos_entry_mode)?,
            merchant_id: string_to_pic_x("merchant_id", &self.merchant_id)?,
            merchant_name: string_to_pic_x("merchant_name", &self.merchant_name)?,
            merchant_city: string_to_pic_x("merchant_city", &self.merchant_city)?,
            merchant_state: string_to_pic_x("merchant_state", &self.merchant_state)?,
            merchant_zip: string_to_pic_x("merchant_zip", &self.merchant_zip)?,
            transaction_id: string_to_pic_x("transaction_id", &self.transaction_id)?,
            match_status: string_to_pic_x("match_status", &self.match_status)?,
            auth_fraud: string_to_pic_x("auth_fraud", &self.auth_fraud)?,
            fraud_rpt_date: string_to_pic_x("fraud_rpt_date", &self.fraud_rpt_date)?,
            filler: string_to_pic_x("filler", &self.filler)?,
        };
        Ok((acct, record))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{PicS, PicU, PicX};
    use domain::money::Money;
    use domain::records::PendingAuthDetailRecord;
    use domain::sensitive::CardNumber;
    use rust_decimal::Decimal;

    use super::PendingAuthDetailRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A stored-value case: the column it corrupts, the class it must fail with, the corruption.
    type StoredCase = (&'static str, DecodeKind, fn(&mut PendingAuthDetailRow));

    /// A write case: the column whose record field it corrupts, and the corruption.
    type WriteCase = (&'static str, fn(&mut PendingAuthDetailRecord));

    /// Not a fixture card number: no error may repeat it.
    const CARD: &str = "9876543210987654";

    /// Its last four digits, the part a redacted form shows and an error must not.
    const LAST_FOUR: &str = "7654";

    /// The parts of a detail the round-trip cases vary.
    struct Case {
        acct: i64,
        date_9c: i64,
        time_9c: i64,
        processing_code: u64,
        pos_entry_mode: u64,
        transaction_cents: i64,
        approved_cents: i64,
        resp_code: &'static str,
        match_status: &'static str,
        fraud: &'static str,
        fraud_rpt_date: &'static str,
    }

    const CASES: [Case; 3] = [
        // An approval on day 001 of 2025 at 12:34:56.789, keys inverted as CP00 builds them,
        // its fraud since confirmed, with negative amounts.
        Case {
            acct: 50,
            date_9c: 99_999 - 25_001,
            time_9c: 999_999_999 - 123_456_789,
            processing_code: 3_000,
            pos_entry_mode: 5,
            transaction_cents: -123_456,
            approved_cents: -123_456,
            resp_code: "00",
            match_status: "P",
            fraud: "F",
            fraud_rpt_date: "07/18/22",
        },
        // Every numeric column at the top of its picture, with the smallest signed keys.
        Case {
            acct: 99_999_999_999,
            date_9c: -99_999,
            time_9c: -999_999_999,
            processing_code: 999_999,
            pos_entry_mode: 99,
            transaction_cents: 999_999_999_999,
            approved_cents: 0,
            resp_code: "05",
            match_status: "D",
            fraud: "R",
            fraud_rpt_date: "07/19/22",
        },
        // The bottom of every picture, with blank fraud fields as CP00 writes them.
        Case {
            acct: -99_999_999_999,
            date_9c: 0,
            time_9c: 0,
            processing_code: 0,
            pos_entry_mode: 0,
            transaction_cents: -999_999_999_999,
            approved_cents: -1,
            resp_code: "00",
            match_status: "M",
            fraud: " ",
            fraud_rpt_date: "",
        },
    ];

    fn record(case: &Case) -> Result<PendingAuthDetailRecord, Box<dyn StdError>> {
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::new(case.date_9c)?,
            auth_time_9c: PicS::new(case.time_9c)?,
            orig_date: PicX::from_text("250101")?,
            orig_time: PicX::from_text("123456")?,
            card_num: CardNumber::parse(CARD)?,
            auth_type: PicX::from_text("AUTH")?,
            card_expiry_date: PicX::from_text("1227")?,
            message_type: PicX::from_text("0100")?,
            message_source: PicX::from_text("POS")?,
            auth_id_code: PicX::from_text("123456")?,
            auth_resp_code: PicX::from_text(case.resp_code)?,
            auth_resp_reason: PicX::from_text("0000")?,
            processing_code: PicU::new(case.processing_code)?,
            transaction_amt: Money::new(Decimal::new(case.transaction_cents, 2))?,
            approved_amt: Money::new(Decimal::new(case.approved_cents, 2))?,
            merchant_catagory_code: PicX::from_text("5411")?,
            acqr_country_code: PicX::from_text("840")?,
            pos_entry_mode: PicU::new(case.pos_entry_mode)?,
            merchant_id: PicX::from_text("MERCH0000000001")?,
            merchant_name: PicX::from_text("CORNER GROCERY")?,
            merchant_city: PicX::from_text("SEATTLE")?,
            merchant_state: PicX::from_text("WA")?,
            merchant_zip: PicX::from_text("98101")?,
            transaction_id: PicX::from_text("TXN000000000001")?,
            match_status: PicX::from_text(case.match_status)?,
            auth_fraud: PicX::from_text(case.fraud)?,
            fraud_rpt_date: PicX::from_text(case.fraud_rpt_date)?,
            // Non-space FILLER bytes are kept like any others (PIC-012).
            filler: PicX::from_array(*b"~FILLER{}  bytes!"),
        })
    }

    /// The row of the first case, for the cases that corrupt one column of it.
    fn stored_row() -> Result<PendingAuthDetailRow, Box<dyn StdError>> {
        let [first, ..] = &CASES;
        Ok(PendingAuthDetailRow::from_record(
            PicS::new(first.acct)?,
            &record(first)?,
        )?)
    }

    /// Cardholder Data Redaction: an error names its column and class, never the value.
    fn assert_redacted(error: &PersistenceError, column: &str) {
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(text.contains(column), "{text}");
            assert!(!text.contains(CARD) && !text.contains(LAST_FOUR), "{text}");
        }
    }

    // audit: CIPAUDTY-001
    // audit: CIPAUDTY-002
    // audit: CIPAUDTY-005
    // audit: CIPAUDTY-013
    // audit: CIPAUDTY-014
    // audit: CIPAUDTY-015
    // audit: CIPAUDTY-018
    // audit: CIPAUDTY-026
    // audit: CIPAUDTY-027
    // audit: CIPAUDTY-028
    // audit: CIPAUDTY-029
    #[test]
    fn every_column_round_trips_unchanged() -> TestResult {
        for case in &CASES {
            let acct = PicS::<11>::new(case.acct)?;
            let original = record(case)?;
            let row = PendingAuthDetailRow::from_record(acct, &original)?;

            assert_eq!(
                (row.acct_id, row.auth_date_9c, row.auth_time_9c),
                (case.acct, case.date_9c, case.time_9c)
            );
            // TRUNCATION: none survives. Both conversions are checked: a `PA-PROCESSING-CODE`
            // value (`PIC 9(06)`) is at most 999999 and a `PA-POS-ENTRY-MODE` one (`PIC 9(02)`)
            // at most 99, so `i64` holds both exactly, and `?` refuses any value outside `i64`
            // instead of wrapping.
            assert_eq!(
                (row.processing_code, row.pos_entry_mode),
                (
                    i64::try_from(case.processing_code)?,
                    i64::try_from(case.pos_entry_mode)?
                )
            );
            for (column, cents) in [
                (row.transaction_amt, case.transaction_cents),
                (row.approved_amt, case.approved_cents),
            ] {
                assert_eq!(column, Decimal::new(cents, 2));
                assert_eq!(column.scale(), 2);
            }
            assert_eq!(row.card_num, CARD);
            assert_eq!(row.merchant_name, "CORNER GROCERY        ");
            assert_eq!(row.auth_fraud, case.fraud);
            assert_eq!(row.fraud_rpt_date, format!("{:<8}", case.fraud_rpt_date));
            assert_eq!(row.filler, "~FILLER{}  bytes!");

            assert_eq!(row.into_record()?, (acct, original));
        }
        Ok(())
    }

    #[test]
    fn inverted_keys_are_stored_as_the_record_holds_them() -> TestResult {
        let [first, ..] = &CASES;
        let row = stored_row()?;
        assert_eq!(row.auth_date_9c, first.date_9c);
        let (_, back) = row.into_record()?;
        assert_eq!(back.auth_date_9c.uncomplement()?, 25_001);
        assert_eq!(back.auth_time_9c.uncomplement()?, 123_456_789);
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-023
    // audit: PIC-024
    #[test]
    fn stored_values_that_do_not_fit_their_picture_are_rejected() -> TestResult {
        let out_of_range = DecodeKind::OutOfRange;
        let cases: [StoredCase; 15] = [
            ("acct_id", out_of_range, |row| row.acct_id = 100_000_000_000),
            ("auth_date_9c", out_of_range, |row| {
                row.auth_date_9c = 100_000;
            }),
            ("auth_date_9c", out_of_range, |row| {
                row.auth_date_9c = -100_000;
            }),
            ("auth_time_9c", out_of_range, |row| {
                row.auth_time_9c = 1_000_000_000;
            }),
            ("processing_code", out_of_range, |row| {
                row.processing_code = -1;
            }),
            ("processing_code", out_of_range, |row| {
                row.processing_code = 1_000_000;
            }),
            ("pos_entry_mode", out_of_range, |row| {
                row.pos_entry_mode = 100;
            }),
            ("transaction_amt", out_of_range, |row| {
                row.transaction_amt = Decimal::new(1_000_000_000_000, 2);
            }),
            ("approved_amt", out_of_range, |row| {
                row.approved_amt = Decimal::new(1_234, 3);
            }),
            (
                "merchant_name",
                DecodeKind::WidthMismatch {
                    expected: 22,
                    actual: 21,
                },
                |row| row.merchant_name = "X".repeat(21),
            ),
            ("card_num", DecodeKind::InvalidByte, |row| {
                row.card_num = "9876\u{0}43210987654".to_owned();
            }),
            (
                "card_num",
                DecodeKind::WidthMismatch {
                    expected: 16,
                    actual: 17,
                },
                |row| row.card_num = format!("{CARD}0"),
            ),
            (
                "fraud_rpt_date",
                DecodeKind::WidthMismatch {
                    expected: 8,
                    actual: 7,
                },
                |row| row.fraud_rpt_date = "07/18/2".to_owned(),
            ),
            ("auth_fraud", DecodeKind::InvalidByte, |row| {
                row.auth_fraud = "\u{e9}".to_owned();
            }),
            ("filler", DecodeKind::InvalidByte, |row| {
                row.filler = format!("{:<16}\u{0}", "");
            }),
        ];
        for (column, kind, corrupt) in cases {
            let mut row = stored_row()?;
            corrupt(&mut row);
            let outcome = row.into_record().map(drop);
            assert_eq!(outcome, Err(PersistenceError::decode(column, kind)));
            if let Err(error) = outcome {
                assert_redacted(&error, column);
            }
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-024
    #[test]
    fn record_bytes_no_column_can_hold_are_rejected() -> TestResult {
        let cases: [WriteCase; 3] = [
            ("card_num", |record| {
                record.card_num = CardNumber::new(PicX::moved_from(b"9876\x0043210987654"));
            }),
            ("merchant_name", |record| {
                record.merchant_name = PicX::moved_from(b"CAF\xc3\xa9");
            }),
            ("filler", |record| record.filler = PicX::moved_from(b"\x00")),
        ];
        let [first, ..] = &CASES;
        for (column, corrupt) in cases {
            let mut detail = record(first)?;
            corrupt(&mut detail);
            let outcome = PendingAuthDetailRow::from_record(PicS::new(first.acct)?, &detail);
            let error = outcome.err().ok_or("a corrupt record was stored")?;
            assert_eq!(
                error,
                PersistenceError::decode(column, DecodeKind::InvalidByte)
            );
            assert_redacted(&error, column);
        }
        Ok(())
    }
}
