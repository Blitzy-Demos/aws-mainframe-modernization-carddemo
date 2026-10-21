//! Codec B for table `transactions` (migration 0002): `CVTRA05Y` `TRAN-RECORD`, the 350-byte
//! transaction record (`CVTRA05Y.cpy:4-18`; CVTRA05Y-001 to CVTRA05Y-014).
//!
//! [`TransactionRow`] holds the sixteen columns of the table, in migration order. Twelve
//! copybook fields take one column each; `TRAN-ORIG-TS` and `TRAN-PROC-TS` take two, a
//! `TIMESTAMP(6)` value and its `<col>_form CHAR(1)` layout code. The two page queries,
//! `queries/transactions_page_forward.sql` and `queries/transactions_page_backward.sql`, select
//! exactly these columns into it (Bounded Pagination).
//!
//! `CVTRA06Y` `DALYTRAN-RECORD` is the same layout under other field names (CVTRA06Y-001 to
//! CVTRA06Y-014), so the 300 `dailytran.txt` records round-trip through this row by the lossless
//! `From` conversions of `domain::records::daily_transaction`, and no daily row type exists (d9).
//!
//! # Timestamps
//!
//! A timestamp is stored typed, never as text (Typed Dates and Timestamps). The layout code
//! beside it records which text the COBOL wrote, so codec A re-encodes the original 26 bytes
//! (E33):
//!
//! | Writer | `TRAN-ORIG-TS` | `TRAN-PROC-TS` |
//! |---|---|---|
//! | `dailytran.txt`, copied by CBTRN02C (`CBTRN02C.cbl:436`) | `I` (PIC-018) | blank (PIC-017) |
//! | CBTRN02C `DB2-FORMAT-TS` (`CBTRN02C.cbl:437-438`) | — | `D` (PIC-019) |
//! | CBACT04C `DB2-FORMAT-TS` (`CBACT04C.cbl:497-498`) | `D` | `D` |
//! | COTRN02C screen dates (`COTRN02C.cbl:464-465`) | `S`, stored at midnight (PIC-020) | `S` |
//! | COBIL00C `WS-TIMESTAMP` (`COBIL00C.cbl:231-232`) | `I` | `I` |
//!
//! Reading a row back rejects an unknown code, a value without its code, a code without its
//! value, and a value its layout cannot reproduce. Such a row is an error, never re-encoded in
//! another layout (PIC-021, R7).
//!
//! # Card number
//!
//! `TRAN-CARD-NUM` reaches its column only through [`CardNumber::expose`], and reading it back
//! wraps the stored bytes in a [`CardNumber`] again (CVTRA05Y-011, PIC-024). The row leaves
//! `Debug` underived and every error names a column and a class, never a value, so neither can
//! print the full number (Cardholder Data Redaction, E32).

use chrono::NaiveDateTime;
use domain::records::TransactionRecord;
use domain::sensitive::CardNumber;
use rust_decimal::Decimal;

use super::{
    columns_to_timestamp, decimal_to_money, i64_to_pic_u, money_to_decimal, pic_u_to_i64,
    pic_x_to_string, string_to_pic_x, timestamp_to_columns,
};
use crate::error::PersistenceError;

/// Column `tran_id`, `TRAN-ID PIC X(16)` (CVTRA05Y-001).
const TRAN_ID: &str = "tran_id";
/// Column `tran_type_cd`, `TRAN-TYPE-CD PIC X(02)` (CVTRA05Y-002).
const TRAN_TYPE_CD: &str = "tran_type_cd";
/// Column `tran_cat_cd`, `TRAN-CAT-CD PIC 9(04)` (CVTRA05Y-003).
const TRAN_CAT_CD: &str = "tran_cat_cd";
/// Column `tran_source`, `TRAN-SOURCE PIC X(10)` (CVTRA05Y-004).
const TRAN_SOURCE: &str = "tran_source";
/// Column `tran_desc`, `TRAN-DESC PIC X(100)` (CVTRA05Y-005).
const TRAN_DESC: &str = "tran_desc";
/// Column `tran_amt`, `TRAN-AMT PIC S9(09)V99` (CVTRA05Y-006).
const TRAN_AMT: &str = "tran_amt";
/// Column `tran_merchant_id`, `TRAN-MERCHANT-ID PIC 9(09)` (CVTRA05Y-007).
const TRAN_MERCHANT_ID: &str = "tran_merchant_id";
/// Column `tran_merchant_name`, `TRAN-MERCHANT-NAME PIC X(50)` (CVTRA05Y-008).
const TRAN_MERCHANT_NAME: &str = "tran_merchant_name";
/// Column `tran_merchant_city`, `TRAN-MERCHANT-CITY PIC X(50)` (CVTRA05Y-009).
const TRAN_MERCHANT_CITY: &str = "tran_merchant_city";
/// Column `tran_merchant_zip`, `TRAN-MERCHANT-ZIP PIC X(10)` (CVTRA05Y-010).
const TRAN_MERCHANT_ZIP: &str = "tran_merchant_zip";
/// Column `card_num`, `TRAN-CARD-NUM PIC X(16)` (CVTRA05Y-011).
const CARD_NUM: &str = "card_num";
/// Column `orig_ts` with `orig_ts_form`, `TRAN-ORIG-TS PIC X(26)` (CVTRA05Y-012).
const ORIG_TS: &str = "orig_ts";
/// Column `proc_ts` with `proc_ts_form`, `TRAN-PROC-TS PIC X(26)` (CVTRA05Y-013).
const PROC_TS: &str = "proc_ts";
/// Column `filler`, `FILLER PIC X(20)` (CVTRA05Y-014).
const FILLER: &str = "filler";

/// One row of `transactions`: the columns of migration 0002, in its order, typed as `sqlx`
/// decodes them (CVTRA05Y-001 to CVTRA05Y-014).
///
/// `CHAR(n)` columns are `String` with every padding byte kept, `BIGINT` columns `i64`,
/// `NUMERIC(11,2)` a `Decimal` at scale 2, and each timestamp an `Option<NaiveDateTime>` beside
/// its `Option<String>` layout code, both `None` for a blank field (PIC-001, PIC-003, PIC-005,
/// PIC-017). Fields are crate-visible so that only `repos` binds and reads them.
pub struct TransactionRow {
    /// `TRAN-ID`, the primary key, `CHAR(16)` (CVTRA05Y-001).
    pub(crate) tran_id: String,
    /// `TRAN-TYPE-CD`, `CHAR(2)` (CVTRA05Y-002).
    pub(crate) tran_type_cd: String,
    /// `TRAN-CAT-CD`, `BIGINT` in `0..=9999` (CVTRA05Y-003).
    pub(crate) tran_cat_cd: i64,
    /// `TRAN-SOURCE`, `CHAR(10)` (CVTRA05Y-004).
    pub(crate) tran_source: String,
    /// `TRAN-DESC`, `CHAR(100)`, its trailing spaces kept (CVTRA05Y-005).
    pub(crate) tran_desc: String,
    /// `TRAN-AMT`, `NUMERIC(11,2)` (CVTRA05Y-006).
    pub(crate) tran_amt: Decimal,
    /// `TRAN-MERCHANT-ID`, `BIGINT` in `0..=999999999` (CVTRA05Y-007).
    pub(crate) tran_merchant_id: i64,
    /// `TRAN-MERCHANT-NAME`, `CHAR(50)` (CVTRA05Y-008).
    pub(crate) tran_merchant_name: String,
    /// `TRAN-MERCHANT-CITY`, `CHAR(50)` (CVTRA05Y-009).
    pub(crate) tran_merchant_city: String,
    /// `TRAN-MERCHANT-ZIP`, `CHAR(10)` (CVTRA05Y-010).
    pub(crate) tran_merchant_zip: String,
    /// `TRAN-CARD-NUM`, `CHAR(16)`: the full card number, bound and read only by `repos`
    /// (CVTRA05Y-011, PIC-024).
    pub(crate) card_num: String,
    /// `TRAN-ORIG-TS` value, `TIMESTAMP(6)`, `NULL` for 26 spaces (CVTRA05Y-012).
    pub(crate) orig_ts: Option<NaiveDateTime>,
    /// `TRAN-ORIG-TS` layout code `I`, `D` or `S`, `CHAR(1)`, `NULL` with the value
    /// (CVTRA05Y-012, PIC-018 to PIC-020).
    pub(crate) orig_ts_form: Option<String>,
    /// `TRAN-PROC-TS` value, `TIMESTAMP(6)`, `NULL` for 26 spaces; the indexed processing time
    /// that replaces the alternate index `KEYS(26 304)` (CVTRA05Y-013).
    pub(crate) proc_ts: Option<NaiveDateTime>,
    /// `TRAN-PROC-TS` layout code `I`, `D` or `S`, `CHAR(1)`, `NULL` with the value
    /// (CVTRA05Y-013, PIC-018 to PIC-020).
    pub(crate) proc_ts_form: Option<String>,
    /// `FILLER`, `CHAR(20)`, kept byte for byte (CVTRA05Y-014, PIC-012).
    pub(crate) filler: String,
}

impl TransactionRow {
    /// The row that stores `record`: each `PIC X(n)` field verbatim, its padding included, the
    /// two `9(n)` fields as `BIGINT`, the amount at scale 2, the card number through
    /// [`CardNumber::expose`], and each timestamp as its value and layout code (CVTRA05Y-001 to
    /// CVTRA05Y-014, PIC-017 to PIC-020, E33).
    ///
    /// # Errors
    ///
    /// [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for the first
    /// `PIC X(n)` field, card number and `FILLER` included, that holds a NUL or non-ASCII byte,
    /// which no `CHAR(n)` column stores byte for byte (PIC-002). The error names the column and
    /// never the value.
    pub fn from_record(record: &TransactionRecord) -> Result<Self, PersistenceError> {
        let (orig_ts, orig_ts_form) = timestamp_to_columns(&record.orig_ts);
        let (proc_ts, proc_ts_form) = timestamp_to_columns(&record.proc_ts);
        Ok(Self {
            tran_id: pic_x_to_string(TRAN_ID, &record.id)?,
            tran_type_cd: pic_x_to_string(TRAN_TYPE_CD, &record.type_cd)?,
            tran_cat_cd: pic_u_to_i64(TRAN_CAT_CD, record.cat_cd)?,
            tran_source: pic_x_to_string(TRAN_SOURCE, &record.source)?,
            tran_desc: pic_x_to_string(TRAN_DESC, &record.desc)?,
            tran_amt: money_to_decimal(record.amt),
            tran_merchant_id: pic_u_to_i64(TRAN_MERCHANT_ID, record.merchant_id)?,
            tran_merchant_name: pic_x_to_string(TRAN_MERCHANT_NAME, &record.merchant_name)?,
            tran_merchant_city: pic_x_to_string(TRAN_MERCHANT_CITY, &record.merchant_city)?,
            tran_merchant_zip: pic_x_to_string(TRAN_MERCHANT_ZIP, &record.merchant_zip)?,
            card_num: pic_x_to_string(CARD_NUM, record.card_num.expose())?,
            orig_ts,
            orig_ts_form,
            proc_ts,
            proc_ts_form,
            filler: pic_x_to_string(FILLER, &record.filler)?,
        })
    }

    /// The record this row stores, each column converted back by the checked inverse of
    /// [`TransactionRow::from_record`], so a row written from a record reads back equal to it
    /// and codec A re-encodes the original 350 bytes (CVTRA05Y-001 to CVTRA05Y-014, d9). The
    /// card number is wrapped in a [`CardNumber`] by its stored-field constructor, as codec A's
    /// decode wraps it.
    ///
    /// # Errors
    ///
    /// The first column, in table order, whose value does not fit its picture, as a
    /// [`PersistenceError::Decode`] naming that column, never the value:
    ///
    /// - [`DecodeKind::WidthMismatch`](crate::error::DecodeKind::WidthMismatch) for a `CHAR(n)`
    ///   text that is not exactly n bytes, and
    ///   [`DecodeKind::InvalidByte`](crate::error::DecodeKind::InvalidByte) for one holding a
    ///   NUL or non-ASCII character (PIC-001, PIC-002).
    /// - [`DecodeKind::OutOfRange`](crate::error::DecodeKind::OutOfRange) for `tran_cat_cd`
    ///   outside `0..=9999`, `tran_merchant_id` outside `0..=999999999` (PIC-003), or a
    ///   `tran_amt` of ten or more integer digits or with a third decimal place (PIC-005).
    /// - [`DecodeKind::UnknownForm`](crate::error::DecodeKind::UnknownForm) for a layout code
    ///   other than `I`, `D` and `S`, and
    ///   [`DecodeKind::FormMismatch`](crate::error::DecodeKind::FormMismatch) for a timestamp
    ///   without its code, a code without its timestamp, or a timestamp its layout cannot
    ///   reproduce (PIC-021, E33).
    pub fn into_record(self) -> Result<TransactionRecord, PersistenceError> {
        let Self {
            tran_id,
            tran_type_cd,
            tran_cat_cd,
            tran_source,
            tran_desc,
            tran_amt,
            tran_merchant_id,
            tran_merchant_name,
            tran_merchant_city,
            tran_merchant_zip,
            card_num,
            orig_ts,
            orig_ts_form,
            proc_ts,
            proc_ts_form,
            filler,
        } = self;
        Ok(TransactionRecord {
            id: string_to_pic_x(TRAN_ID, &tran_id)?,
            type_cd: string_to_pic_x(TRAN_TYPE_CD, &tran_type_cd)?,
            cat_cd: i64_to_pic_u(TRAN_CAT_CD, tran_cat_cd)?,
            source: string_to_pic_x(TRAN_SOURCE, &tran_source)?,
            desc: string_to_pic_x(TRAN_DESC, &tran_desc)?,
            amt: decimal_to_money(TRAN_AMT, tran_amt)?,
            merchant_id: i64_to_pic_u(TRAN_MERCHANT_ID, tran_merchant_id)?,
            merchant_name: string_to_pic_x(TRAN_MERCHANT_NAME, &tran_merchant_name)?,
            merchant_city: string_to_pic_x(TRAN_MERCHANT_CITY, &tran_merchant_city)?,
            merchant_zip: string_to_pic_x(TRAN_MERCHANT_ZIP, &tran_merchant_zip)?,
            card_num: CardNumber::new(string_to_pic_x(CARD_NUM, &card_num)?),
            orig_ts: columns_to_timestamp(ORIG_TS, orig_ts, orig_ts_form)?,
            proc_ts: columns_to_timestamp(PROC_TS, proc_ts, proc_ts_form)?,
            filler: string_to_pic_x(FILLER, &filler)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{FixedRecord, PicX};
    use domain::records::{DailyTransactionRecord, TransactionRecord};
    use domain::sensitive::CardNumber;
    use rust_decimal::Decimal;

    use super::TransactionRow;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The value and layout code a timestamp's two columns hold.
    type Columns = (Option<NaiveDateTime>, Option<&'static str>);

    /// A row edit that makes the row unreadable.
    type Edit = fn(&mut TransactionRow);

    /// A card number in no fixture file: no error may repeat it, nor any long part of it.
    const CARD: &str = "9876543210987654";

    /// `DALYTRAN-ORIG-TS` of `dailytran.txt` line 1, layout `I` (PIC-018).
    const ISO_TS: &[u8; 26] = b"2022-06-10 19:27:53.000000";
    /// A `DB2-FORMAT-TS` value, layout `D`: two hundredths digits, then `0000` (PIC-019).
    const DB2_TS: &[u8; 26] = b"2022-07-18-10.15.42.370000";
    /// A COTRN02C screen date followed by 16 spaces, layout `S` (PIC-020).
    const SCREEN_TS: &[u8; 26] = b"2022-07-18                ";
    /// A blank timestamp, every `DALYTRAN-PROC-TS` of the fixture (PIC-017).
    const BLANK_TS: &[u8; 26] = &[b' '; 26];

    /// `+504.77`, the overpunch `G` being a positive 7 (PIC-006).
    const PLUS_AMT: &[u8; 11] = b"0000005047G";
    /// `-123.41`, the overpunch `J` being a negative 1 (PIC-006).
    const MINUS_AMT: &[u8; 11] = b"0000001234J";

    /// `text` followed by spaces to `N` bytes.
    fn padded<const N: usize>(text: &[u8]) -> Vec<u8> {
        let mut field = text.to_vec();
        field.resize(N, b' ');
        field
    }

    /// A 350-byte transaction line shaped as `dailytran.txt` line 1, with [`CARD`] as its card
    /// number and `amt`, `orig_ts` and `proc_ts` in their fields.
    fn line(amt: &[u8; 11], orig_ts: &[u8; 26], proc_ts: &[u8; 26]) -> Vec<u8> {
        let fields: [&[u8]; 14] = [
            b"0000000000683580",
            b"01",
            b"0001",
            &padded::<10>(b"POS TERM"),
            &padded::<100>(b"Purchase at Abshire-Lowe"),
            amt,
            b"800000000",
            &padded::<50>(b"Abshire-Lowe"),
            &padded::<50>(b"North Enoshaven"),
            &padded::<10>(b"72112"),
            CARD.as_bytes(),
            orig_ts,
            proc_ts,
            &[b' '; 20],
        ];
        fields.concat()
    }

    /// The instant on `(year, month, day)` at `(hour, minute, second)` and `micros` microseconds.
    fn at(
        (year, month, day): (i32, u32, u32),
        (hour, minute, second): (u32, u32, u32),
        micros: u32,
    ) -> Result<NaiveDateTime, &'static str> {
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_micro_opt(hour, minute, second, micros))
            .ok_or("no such instant")
    }

    /// The row of a CBTRN02C-posted transaction: layout `I` origin, layout `D` processing time.
    fn posted_row() -> Result<TransactionRow, Box<dyn StdError>> {
        let record = TransactionRecord::decode(&line(PLUS_AMT, ISO_TS, DB2_TS))?;
        Ok(TransactionRow::from_record(&record)?)
    }

    // audit: CVTRA05Y-006
    // audit: CVTRA05Y-012
    // audit: CVTRA05Y-013
    // audit: PIC-017
    // audit: PIC-018
    // audit: PIC-019
    // audit: PIC-020
    #[test]
    fn every_writer_layout_pair_round_trips_through_the_row() -> TestResult {
        let blank: Columns = (None, None);
        let iso: Columns = (Some(at((2022, 6, 10), (19, 27, 53), 0)?), Some("I"));
        let db2: Columns = (Some(at((2022, 7, 18), (10, 15, 42), 370_000)?), Some("D"));
        let screen: Columns = (Some(at((2022, 7, 18), (0, 0, 0), 0)?), Some("S"));
        let cases = [
            // The fixture shape: `DALYTRAN-ORIG-TS` in layout `I`, `DALYTRAN-PROC-TS` blank.
            (ISO_TS, BLANK_TS, PLUS_AMT, iso, blank),
            // CBTRN02C: the daily origin, then `DB2-FORMAT-TS` (`CBTRN02C.cbl:436-438`).
            (ISO_TS, DB2_TS, MINUS_AMT, iso, db2),
            // CBACT04C: `DB2-FORMAT-TS` in both (`CBACT04C.cbl:497-498`).
            (DB2_TS, DB2_TS, PLUS_AMT, db2, db2),
            // COTRN02C: the two screen dates (`COTRN02C.cbl:464-465`).
            (SCREEN_TS, SCREEN_TS, MINUS_AMT, screen, screen),
            // COBIL00C: `WS-TIMESTAMP` in both (`COBIL00C.cbl:231-232`).
            (ISO_TS, ISO_TS, PLUS_AMT, iso, iso),
        ];
        for (orig_text, proc_text, amt_text, orig, proc) in cases {
            let bytes = line(amt_text, orig_text, proc_text);
            let record = TransactionRecord::decode(&bytes)?;
            let row = TransactionRow::from_record(&record)?;
            assert_eq!((row.orig_ts, row.orig_ts_form.as_deref()), orig);
            assert_eq!((row.proc_ts, row.proc_ts_form.as_deref()), proc);
            let back = row.into_record()?;
            assert_eq!(back, record);
            assert!(back.encode() == bytes, "re-encoded line differs");
        }
        Ok(())
    }

    // audit: CVTRA06Y-001
    // audit: CVTRA06Y-003
    // audit: CVTRA06Y-005
    // audit: CVTRA06Y-006
    // audit: CVTRA06Y-007
    // audit: CVTRA06Y-011
    // audit: CVTRA06Y-012
    // audit: CVTRA06Y-013
    // audit: CVTRA06Y-014
    // audit: PIC-001
    // audit: PIC-003
    // audit: PIC-005
    // audit: PIC-012
    #[test]
    fn a_dailytran_line_round_trips_through_the_transaction_row() -> TestResult {
        let bytes = line(PLUS_AMT, ISO_TS, BLANK_TS);
        let daily = DailyTransactionRecord::decode(&bytes)?;
        let row = TransactionRow::from_record(&TransactionRecord::from(daily.clone()))?;
        assert_eq!(row.tran_id, "0000000000683580");
        assert_eq!(row.tran_type_cd, "01");
        assert_eq!(row.tran_cat_cd, 1);
        assert_eq!(row.tran_source, "POS TERM  ");
        assert_eq!(row.tran_desc.len(), 100);
        assert_eq!(row.tran_desc.trim_end(), "Purchase at Abshire-Lowe");
        assert_eq!(row.tran_amt, Decimal::new(50_477, 2));
        assert_eq!(row.tran_amt.scale(), 2);
        assert_eq!(row.tran_merchant_id, 800_000_000);
        assert_eq!(row.tran_merchant_name.len(), 50);
        assert_eq!(row.tran_merchant_city.trim_end(), "North Enoshaven");
        assert_eq!(row.tran_merchant_zip, "72112     ");
        assert!(row.card_num == CARD, "card number column differs");
        assert_eq!(row.proc_ts, None);
        assert_eq!(row.proc_ts_form, None);
        assert_eq!(row.filler, " ".repeat(20));
        let back = DailyTransactionRecord::from(row.into_record()?);
        assert_eq!(back, daily);
        assert!(back.encode() == bytes, "re-encoded dailytran line differs");
        Ok(())
    }

    // audit: CVTRA05Y-001
    // audit: CVTRA05Y-003
    // audit: CVTRA05Y-005
    // audit: CVTRA05Y-007
    // audit: CVTRA05Y-011
    // audit: CVTRA05Y-014
    // audit: PIC-002
    // audit: PIC-021
    #[test]
    fn stored_values_that_do_not_fit_are_typed_errors_naming_their_column() -> TestResult {
        use DecodeKind::{FormMismatch, InvalidByte, OutOfRange, UnknownForm};
        let width = |expected, actual| DecodeKind::WidthMismatch { expected, actual };
        let cases: [(Edit, &str, DecodeKind); 15] = [
            (
                |row| row.orig_ts_form = Some("X".to_owned()),
                "orig_ts",
                UnknownForm,
            ),
            (
                |row| row.proc_ts_form = Some("DD".to_owned()),
                "proc_ts",
                UnknownForm,
            ),
            (|row| row.proc_ts_form = None, "proc_ts", FormMismatch),
            (|row| row.orig_ts = None, "orig_ts", FormMismatch),
            // `10:15:42.37` in the date-only layout `S`, which would drop the time.
            (
                |row| row.proc_ts_form = Some("S".to_owned()),
                "proc_ts",
                FormMismatch,
            ),
            (|row| row.tran_cat_cd = 10_000, "tran_cat_cd", OutOfRange),
            (|row| row.tran_cat_cd = -1, "tran_cat_cd", OutOfRange),
            (
                |row| row.tran_merchant_id = 1_000_000_000,
                "tran_merchant_id",
                OutOfRange,
            ),
            // Ten integer digits, one more than `S9(09)V99` holds.
            (
                |row| row.tran_amt = Decimal::new(100_000_000_000, 2),
                "tran_amt",
                OutOfRange,
            ),
            (
                |row| row.tran_amt = Decimal::new(12_345, 3),
                "tran_amt",
                OutOfRange,
            ),
            (|row| row.tran_id.truncate(15), "tran_id", width(16, 15)),
            (|row| row.tran_desc.push(' '), "tran_desc", width(100, 101)),
            (|row| row.filler.clear(), "filler", width(20, 0)),
            (
                |row| row.tran_type_cd = "0\u{e9}".to_owned(),
                "tran_type_cd",
                InvalidByte,
            ),
            (|row| row.card_num.push('\0'), "card_num", InvalidByte),
        ];
        for (edit, column, kind) in cases {
            let mut row = posted_row()?;
            edit(&mut row);
            let error = row.into_record().err();
            assert_eq!(error, Some(PersistenceError::decode(column, kind)));
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn record_bytes_no_char_column_can_store_are_typed_errors() -> TestResult {
        let record = TransactionRecord::decode(&line(PLUS_AMT, ISO_TS, DB2_TS))?;
        let mut card = record.clone();
        card.card_num = CardNumber::new(PicX::from_array(*b"987654321098765\0"));
        let mut desc = record.clone();
        desc.desc = PicX::from_array([0x80; 100]);
        let mut filler = record;
        filler.filler = PicX::from_array([0xFF; 20]);
        for (bad, column) in [(card, "card_num"), (desc, "tran_desc"), (filler, "filler")] {
            assert_eq!(
                TransactionRow::from_record(&bad).err(),
                Some(PersistenceError::decode(column, DecodeKind::InvalidByte))
            );
        }
        Ok(())
    }

    /// Cardholder Data Redaction: an error names its column and class, never the card number.
    // audit: PIC-024
    #[test]
    fn errors_never_repeat_the_card_number() -> TestResult {
        let edits: [Edit; 3] = [
            |row| row.card_num.truncate(15),
            |row| row.card_num.push('5'),
            |row| row.card_num.push('\0'),
        ];
        let mut errors = Vec::new();
        for edit in edits {
            let mut row = posted_row()?;
            edit(&mut row);
            errors.extend(row.into_record().err());
        }
        let mut record = TransactionRecord::decode(&line(PLUS_AMT, ISO_TS, BLANK_TS))?;
        record.card_num = CardNumber::new(PicX::from_array(*b"\x80876543210987654"));
        errors.extend(TransactionRow::from_record(&record).err());
        assert_eq!(errors.len(), 4);
        for error in errors {
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(text.contains("card_num"), "{text}");
                assert!(!text.contains("98765432"), "{text}");
                assert!(!text.contains("10987654"), "{text}");
            }
        }
        Ok(())
    }
}
