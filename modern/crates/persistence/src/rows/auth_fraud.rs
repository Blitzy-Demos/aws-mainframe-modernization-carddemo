//! Codec B for `auth_fraud`, the Db2 table `CARDDEMO.AUTHFRDS` that COPAUS2C writes when an
//! authorization is marked or unmarked as fraud (`AUTHFRDS.ddl:1-28`, migration 0004).
//!
//! [`AuthFraudBinds`] holds the values the insert binds, every column but `fraud_rpt_date`;
//! [`AuthFraudRow`] holds a stored row, every column. Both convert to and from
//! `domain::online::copaus2c::AuthFraudRow`, named `FraudRecord` here to keep it apart from the
//! row type of this module.
//!
//! # `FRAUD_RPT_DATE`
//!
//! COPAUS2C sets `FRAUD_RPT_DATE = CURRENT DATE` in its insert and in its `-803` update, never a
//! value it moves (`COPAUS2C.cbl:194,225`; COPAUS2C-007, COPAUS2C-008), so the domain record
//! carries no report date. The repository binds the injected clock's date beside the binds in
//! both statements, the stand-in for Db2's `CURRENT DATE`, and [`AuthFraudRow::into_record`]
//! returns the stored date beside the record. That pair is the one read shape.
//!
//! # Columns
//!
//! Every other column holds one field of the record, moved as COPAUS2C moves it (COPAUS2C-006):
//!
//! - `CHAR(n)`: the n bytes of a `PicX<n>`, padding included (PIC-001, PIC-002).
//!   `card_expiry_date` is `X(4)` text, and `processing_code` the six digits of
//!   `PA-PROCESSING-CODE`, both text and neither a typed value.
//! - `auth_ts TIMESTAMP(6)`: the typed `NaiveDateTime` the domain rebuilds from the detail's
//!   date and inverted time (COPAUS2C-004). The source column is a Db2 `TIMESTAMP`, not `X(26)`
//!   text, so no layout code goes with it (Typed Dates and Timestamps).
//! - `transaction_amt`, `approved_amt NUMERIC(12,2)`: the `Money<10>` amounts at scale 2.
//! - `pos_entry_mode SMALLINT`: the `PicU<2>` value 0-99, converted with checks both ways.
//! - `merchant_name VARCHAR(22)`: all 22 bytes of `PA-MERCHANT-NAME`, trailing spaces included,
//!   because COPAUS2C moves `LENGTH OF PA-MERCHANT-NAME`, always 22, into `MERCHANT-NAME-LEN`.
//! - `acct_id`, `cust_id BIGINT`: the `PicU<11>` and `PicU<9>` identifiers, 64-bit end to end
//!   (PIC-003, PIC-023, Checked Integer Narrowing).
//!
//! # Nullability
//!
//! Every column after the key is nullable in the DDL, but COPAUS2C passes no null indicator, so
//! every insert stores a value in every column (COPAUS2C-007): the binds hold each one in `Some`.
//! A stored `NULL` has no domain value and is never replaced by a default (R7): a `CHAR(n)` or
//! `VARCHAR(n)` column reads as [`DecodeKind::WidthMismatch`] with `actual: 0`, a numeric column
//! as [`DecodeKind::OutOfRange`].
//!
//! # Cardholder data
//!
//! `card_num` is the full card number, which the database alone keeps (COPAUS2C-013, PIC-024).
//! It leaves the record only through [`CardNumber::expose`] and re-enters through
//! [`CardNumber::new`]. Neither struct derives `Debug`, `Clone` or `Serialize`, and every error
//! names the column and the class of failure, never the value (Cardholder Data Redaction, E32).

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use copybook::{PicU, PicX};
use domain::online::copaus2c::AuthFraudRow as FraudRecord;
use domain::sensitive::CardNumber;
use rust_decimal::Decimal;

use super::{
    decimal_to_money, i64_to_pic_u, money_to_decimal, pic_u_to_i64, pic_x_to_string,
    string_to_pic_x, string_to_varchar, varchar_to_string,
};
use crate::error::{DecodeKind, PersistenceError};

/// `MERCHANT_NAME VARCHAR(22)`: the longest value the column holds (`AUTHFRDS.ddl:18`).
const MERCHANT_NAME_MAX: usize = 22;

/// Nanoseconds in one microsecond, the finest unit of `TIMESTAMP(6)`.
const NANOS_PER_MICRO: u32 = 1_000;

/// Nanoseconds in one second; `chrono` marks a leap second with a count at or above it.
const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// The values COPAUS2C's insert binds for one fraud row: every column of `auth_fraud` but
/// `fraud_rpt_date`, which the repository binds beside them (COPAUS2C-007).
///
/// Built only by [`AuthFraudBinds::from_record`]. Each nullable column holds `Some`, because
/// COPAUS2C writes a value into every column. `card_num` holds the full card number, so the
/// struct derives no `Debug`, `Clone` or `Serialize` (COPAUS2C-013).
pub struct AuthFraudBinds {
    /// `CARD_NUM CHAR(16)`, the first key part: the full card number (COPAUS2C-013).
    pub(crate) card_num: String,
    /// `AUTH_TS TIMESTAMP(6)`, the second key part (COPAUS2C-004).
    pub(crate) auth_ts: NaiveDateTime,
    pub(crate) auth_type: Option<String>,
    /// `CARD_EXPIRY_DATE CHAR(4)`: `X(4)` text, not a typed date.
    pub(crate) card_expiry_date: Option<String>,
    pub(crate) message_type: Option<String>,
    pub(crate) message_source: Option<String>,
    pub(crate) auth_id_code: Option<String>,
    pub(crate) auth_resp_code: Option<String>,
    pub(crate) auth_resp_reason: Option<String>,
    /// `PROCESSING_CODE CHAR(6)`: the six digits of `PA-PROCESSING-CODE`.
    pub(crate) processing_code: Option<String>,
    /// `TRANSACTION_AMT NUMERIC(12,2)`.
    pub(crate) transaction_amt: Option<Decimal>,
    /// `APPROVED_AMT NUMERIC(12,2)`.
    pub(crate) approved_amt: Option<Decimal>,
    pub(crate) merchant_catagory_code: Option<String>,
    pub(crate) acqr_country_code: Option<String>,
    /// `POS_ENTRY_MODE SMALLINT`: the value 0-99.
    pub(crate) pos_entry_mode: Option<i16>,
    pub(crate) merchant_id: Option<String>,
    /// `MERCHANT_NAME VARCHAR(22)`: all 22 bytes, trailing spaces included.
    pub(crate) merchant_name: Option<String>,
    pub(crate) merchant_city: Option<String>,
    pub(crate) merchant_state: Option<String>,
    pub(crate) merchant_zip: Option<String>,
    pub(crate) transaction_id: Option<String>,
    pub(crate) match_status: Option<String>,
    /// `AUTH_FRAUD CHAR(1)`: the action's flag, `F` or `R` (COPAUS2C-006, COPAUS2C-008).
    pub(crate) auth_fraud: Option<String>,
    /// `ACCT_ID BIGINT`, from `WS-ACCT-ID PIC 9(11)`.
    pub(crate) acct_id: Option<i64>,
    /// `CUST_ID BIGINT`, from `WS-CUST-ID PIC 9(9)`.
    pub(crate) cust_id: Option<i64>,
}

/// One stored `auth_fraud` row: every column of migration 0004, its nullable ones `Option`, as
/// the repository reads it.
///
/// [`AuthFraudRow::into_record`] turns it into the domain record and the stored report date.
/// `card_num` holds the full card number, so the struct derives no `Debug`, `Clone` or
/// `Serialize` (COPAUS2C-013).
pub struct AuthFraudRow {
    /// `CARD_NUM CHAR(16)`, the first key part: the full card number (COPAUS2C-013).
    pub(crate) card_num: String,
    /// `AUTH_TS TIMESTAMP(6)`, the second key part (COPAUS2C-004).
    pub(crate) auth_ts: NaiveDateTime,
    pub(crate) auth_type: Option<String>,
    /// `CARD_EXPIRY_DATE CHAR(4)`: `X(4)` text, not a typed date.
    pub(crate) card_expiry_date: Option<String>,
    pub(crate) message_type: Option<String>,
    pub(crate) message_source: Option<String>,
    pub(crate) auth_id_code: Option<String>,
    pub(crate) auth_resp_code: Option<String>,
    pub(crate) auth_resp_reason: Option<String>,
    /// `PROCESSING_CODE CHAR(6)`: the six digits of `PA-PROCESSING-CODE`.
    pub(crate) processing_code: Option<String>,
    /// `TRANSACTION_AMT NUMERIC(12,2)`.
    pub(crate) transaction_amt: Option<Decimal>,
    /// `APPROVED_AMT NUMERIC(12,2)`.
    pub(crate) approved_amt: Option<Decimal>,
    pub(crate) merchant_catagory_code: Option<String>,
    pub(crate) acqr_country_code: Option<String>,
    /// `POS_ENTRY_MODE SMALLINT`.
    pub(crate) pos_entry_mode: Option<i16>,
    pub(crate) merchant_id: Option<String>,
    /// `MERCHANT_NAME VARCHAR(22)`, exactly as stored.
    pub(crate) merchant_name: Option<String>,
    pub(crate) merchant_city: Option<String>,
    pub(crate) merchant_state: Option<String>,
    pub(crate) merchant_zip: Option<String>,
    pub(crate) transaction_id: Option<String>,
    pub(crate) match_status: Option<String>,
    /// `AUTH_FRAUD CHAR(1)`: `F` or `R` (COPAUS2C-006, COPAUS2C-008).
    pub(crate) auth_fraud: Option<String>,
    /// `FRAUD_RPT_DATE DATE`: the date of the last insert or update (COPAUS2C-007,
    /// COPAUS2C-008).
    pub(crate) fraud_rpt_date: Option<NaiveDate>,
    /// `ACCT_ID BIGINT`.
    pub(crate) acct_id: Option<i64>,
    /// `CUST_ID BIGINT`.
    pub(crate) cust_id: Option<i64>,
}

impl AuthFraudBinds {
    /// The binds of COPAUS2C's insert for `record` (`COPAUS2C.cbl:109-197`; COPAUS2C-006,
    /// COPAUS2C-007): the card number through [`CardNumber::expose`], `auth_ts` as the typed
    /// value, every `CHAR(n)` field verbatim, the amounts at scale 2, `pos_entry_mode` narrowed
    /// with a check, all 22 bytes of `merchant_name`, and the two identifiers as `BIGINT`.
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the column:
    ///
    /// - [`DecodeKind::InvalidByte`] for a NUL or non-ASCII byte in a text field, which no
    ///   `PostgreSQL` text column stores byte for byte (PIC-002);
    /// - [`DecodeKind::OutOfRange`] for an `auth_ts` finer than a microsecond or inside a leap
    ///   second, which `TIMESTAMP(6)` would store as another instant.
    pub fn from_record(record: &FraudRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            card_num: pic_x_to_string("card_num", record.card_num.expose())?,
            auth_ts: storable_timestamp(record.auth_ts)?,
            auth_type: char_bind("auth_type", &record.auth_type)?,
            card_expiry_date: char_bind("card_expiry_date", &record.card_expiry_date)?,
            message_type: char_bind("message_type", &record.message_type)?,
            message_source: char_bind("message_source", &record.message_source)?,
            auth_id_code: char_bind("auth_id_code", &record.auth_id_code)?,
            auth_resp_code: char_bind("auth_resp_code", &record.auth_resp_code)?,
            auth_resp_reason: char_bind("auth_resp_reason", &record.auth_resp_reason)?,
            processing_code: char_bind("processing_code", &record.processing_code)?,
            transaction_amt: Some(money_to_decimal(record.transaction_amt)),
            approved_amt: Some(money_to_decimal(record.approved_amt)),
            merchant_catagory_code: char_bind(
                "merchant_catagory_code",
                &record.merchant_catagory_code,
            )?,
            acqr_country_code: char_bind("acqr_country_code", &record.acqr_country_code)?,
            pos_entry_mode: Some(pos_entry_mode_bind(record.pos_entry_mode)?),
            merchant_id: char_bind("merchant_id", &record.merchant_id)?,
            merchant_name: Some(merchant_name_bind(&record.merchant_name)?),
            merchant_city: char_bind("merchant_city", &record.merchant_city)?,
            merchant_state: char_bind("merchant_state", &record.merchant_state)?,
            merchant_zip: char_bind("merchant_zip", &record.merchant_zip)?,
            transaction_id: char_bind("transaction_id", &record.transaction_id)?,
            match_status: char_bind("match_status", &record.match_status)?,
            auth_fraud: char_bind("auth_fraud", &record.auth_fraud)?,
            acct_id: Some(pic_u_to_i64("acct_id", record.acct_id)?),
            cust_id: Some(pic_u_to_i64("cust_id", record.cust_id)?),
        })
    }
}

impl AuthFraudRow {
    /// The domain record a stored row holds, with the row's `fraud_rpt_date` beside it, `None`
    /// for a `NULL` date (COPAUS2C-007, COPAUS2C-008). Every column but the report date must hold
    /// a value, as every row COPAUS2C writes does.
    ///
    /// # Errors
    ///
    /// A [`PersistenceError::Decode`] naming the column:
    ///
    /// - [`DecodeKind::WidthMismatch`] for a `CHAR(n)` text that is not n bytes, a
    ///   `merchant_name` that is not 22, or a `NULL` in any text column (`actual: 0`);
    /// - [`DecodeKind::OutOfRange`] for an amount, `pos_entry_mode` or identifier its picture
    ///   cannot hold, a negative one included, or a `NULL` in any numeric column;
    /// - [`DecodeKind::InvalidByte`] for a NUL or non-ASCII character.
    pub fn into_record(self) -> Result<(FraudRecord, Option<NaiveDate>), PersistenceError> {
        let record = FraudRecord {
            card_num: CardNumber::new(string_to_pic_x("card_num", &self.card_num)?),
            auth_ts: self.auth_ts,
            auth_type: required_char("auth_type", self.auth_type.as_deref())?,
            card_expiry_date: required_char("card_expiry_date", self.card_expiry_date.as_deref())?,
            message_type: required_char("message_type", self.message_type.as_deref())?,
            message_source: required_char("message_source", self.message_source.as_deref())?,
            auth_id_code: required_char("auth_id_code", self.auth_id_code.as_deref())?,
            auth_resp_code: required_char("auth_resp_code", self.auth_resp_code.as_deref())?,
            auth_resp_reason: required_char("auth_resp_reason", self.auth_resp_reason.as_deref())?,
            processing_code: required_char("processing_code", self.processing_code.as_deref())?,
            transaction_amt: decimal_to_money(
                "transaction_amt",
                required_number("transaction_amt", self.transaction_amt)?,
            )?,
            approved_amt: decimal_to_money(
                "approved_amt",
                required_number("approved_amt", self.approved_amt)?,
            )?,
            merchant_catagory_code: required_char(
                "merchant_catagory_code",
                self.merchant_catagory_code.as_deref(),
            )?,
            acqr_country_code: required_char(
                "acqr_country_code",
                self.acqr_country_code.as_deref(),
            )?,
            pos_entry_mode: i64_to_pic_u(
                "pos_entry_mode",
                i64::from(required_number("pos_entry_mode", self.pos_entry_mode)?),
            )?,
            merchant_id: required_char("merchant_id", self.merchant_id.as_deref())?,
            merchant_name: merchant_name_field(self.merchant_name.as_deref())?,
            merchant_city: required_char("merchant_city", self.merchant_city.as_deref())?,
            merchant_state: required_char("merchant_state", self.merchant_state.as_deref())?,
            merchant_zip: required_char("merchant_zip", self.merchant_zip.as_deref())?,
            transaction_id: required_char("transaction_id", self.transaction_id.as_deref())?,
            match_status: required_char("match_status", self.match_status.as_deref())?,
            auth_fraud: required_char("auth_fraud", self.auth_fraud.as_deref())?,
            acct_id: i64_to_pic_u("acct_id", required_number("acct_id", self.acct_id)?)?,
            cust_id: i64_to_pic_u("cust_id", required_number("cust_id", self.cust_id)?)?,
        };
        Ok((record, self.fraud_rpt_date))
    }
}

/// The `Some` bind of a nullable `CHAR(n)` column: the field's n bytes verbatim (PIC-001).
fn char_bind<const N: usize>(
    column: &'static str,
    value: &PicX<N>,
) -> Result<Option<String>, PersistenceError> {
    pic_x_to_string(column, value).map(Some)
}

/// `auth_ts` unchanged when `TIMESTAMP(6)` stores it as the same instant.
///
/// The column keeps microseconds, and `sqlx` drops any finer part when it binds the value, so a
/// sub-microsecond part or a leap second would read back as another instant. Such a value is
/// refused, never moved (R7). Every `auth_ts` the domain builds is a whole number of
/// milliseconds (COPAUS2C-004), so none is refused in practice. The repository applies the same
/// check to the key of its update and its read, so no key is moved to another instant either.
pub(crate) fn storable_timestamp(value: NaiveDateTime) -> Result<NaiveDateTime, PersistenceError> {
    let nanos = value.nanosecond();
    if nanos < NANOS_PER_SECOND && nanos % NANOS_PER_MICRO == 0 {
        Ok(value)
    } else {
        Err(PersistenceError::decode("auth_ts", DecodeKind::OutOfRange))
    }
}

/// `MOVE PA-POS-ENTRY-MODE TO POS-ENTRY-MODE`: the `PIC 9(02)` value in the `SMALLINT` column.
fn pos_entry_mode_bind(value: PicU<2>) -> Result<i16, PersistenceError> {
    let wide = pic_u_to_i64("pos_entry_mode", value)?;
    // TRUNCATION: none survives. The conversion is checked: a `PicU<2>` value is at most 99,
    // which `SMALLINT` holds, so it is exact and the error arm is unreachable.
    i16::try_from(wide)
        .map_err(|_| PersistenceError::decode("pos_entry_mode", DecodeKind::OutOfRange))
}

/// `MOVE PA-MERCHANT-NAME TO MERCHANT-NAME-TEXT` with `MERCHANT-NAME-LEN` 22: all 22 bytes of
/// the field as the `VARCHAR(22)` value, trailing spaces included.
fn merchant_name_bind(value: &PicX<MERCHANT_NAME_MAX>) -> Result<String, PersistenceError> {
    let text = pic_x_to_string("merchant_name", value)?;
    string_to_varchar("merchant_name", &text, MERCHANT_NAME_MAX)
}

/// The `PicX<22>` a `merchant_name VARCHAR(22)` value holds: exactly 22 bytes, as every value
/// COPAUS2C writes is. A shorter value or `NULL` is a width mismatch, never padded.
fn merchant_name_field(value: Option<&str>) -> Result<PicX<MERCHANT_NAME_MAX>, PersistenceError> {
    let text = value.ok_or_else(|| missing_text::<MERCHANT_NAME_MAX>("merchant_name"))?;
    let stored = varchar_to_string("merchant_name", text, MERCHANT_NAME_MAX)?;
    string_to_pic_x("merchant_name", &stored)
}

/// The `PicX<N>` a nullable `CHAR(N)` column holds; `NULL` is a width mismatch of zero bytes.
fn required_char<const N: usize>(
    column: &'static str,
    value: Option<&str>,
) -> Result<PicX<N>, PersistenceError> {
    value.map_or_else(
        || Err(missing_text::<N>(column)),
        |text| string_to_pic_x(column, text),
    )
}

/// The value of a nullable numeric column; `NULL` is out of range for every numeric picture.
fn required_number<T>(column: &'static str, value: Option<T>) -> Result<T, PersistenceError> {
    value.ok_or_else(|| PersistenceError::decode(column, DecodeKind::OutOfRange))
}

/// The error for a `NULL` in a text column whose field is N bytes wide.
const fn missing_text<const N: usize>(column: &'static str) -> PersistenceError {
    PersistenceError::decode(
        column,
        DecodeKind::WidthMismatch {
            expected: N,
            actual: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{PicU, PicX};
    use domain::money::Money;
    use domain::online::copaus2c::AuthFraudRow as FraudRecord;
    use domain::sensitive::CardNumber;
    use rust_decimal::Decimal;

    use super::{AuthFraudBinds, AuthFraudRow};
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A change to a stored row, the column it breaks and the class of the resulting error.
    type Case = (fn(&mut AuthFraudRow), &'static str, DecodeKind);

    /// Not a fixture card number: no error may repeat any part of it.
    const CARD: &[u8; 16] = b"9876543210987654";

    fn auth_ts() -> Result<NaiveDateTime, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(2022, 1, 15)
            .and_then(|date| date.and_hms_micro_opt(10, 30, 45, 123_000))
            .ok_or_else(|| "invalid test timestamp".into())
    }

    fn report_date() -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(2022, 7, 18).ok_or_else(|| "invalid test date".into())
    }

    /// A fraud row as `copaus2c::fraud_row` builds one, every field distinct and the identifiers
    /// at their widest values.
    fn record() -> Result<FraudRecord, Box<dyn StdError>> {
        Ok(FraudRecord {
            card_num: CardNumber::new(PicX::from_array(*CARD)),
            auth_ts: auth_ts()?,
            auth_type: PicX::from_array(*b"0100"),
            card_expiry_date: PicX::from_array(*b"0925"),
            message_type: PicX::from_array(*b"0100  "),
            message_source: PicX::from_array(*b"POS   "),
            auth_id_code: PicX::from_array(*b"123456"),
            auth_resp_code: PicX::from_array(*b"00"),
            auth_resp_reason: PicX::from_array(*b"0000"),
            processing_code: PicX::from_array(*b"004000"),
            transaction_amt: Money::new("9999999999.99".parse::<Decimal>()?)?,
            approved_amt: Money::new("-0.01".parse::<Decimal>()?)?,
            merchant_catagory_code: PicX::from_array(*b"5411"),
            acqr_country_code: PicX::from_array(*b"840"),
            pos_entry_mode: PicU::new(99)?,
            merchant_id: PicX::from_array(*b"MERCHANT0000001"),
            merchant_name: PicX::from_array(*b"CORNER STORE          "),
            merchant_city: PicX::from_array(*b"SPRINGFIELD  "),
            merchant_state: PicX::from_array(*b"IL"),
            merchant_zip: PicX::from_array(*b"62701    "),
            transaction_id: PicX::from_array(*b"TXN000000000001"),
            match_status: PicX::from_array(*b"P"),
            auth_fraud: PicX::from_array(*b"F"),
            acct_id: PicU::new(99_999_999_999)?,
            cust_id: PicU::new(999_999_999)?,
        })
    }

    /// The row the database returns for `binds` and the report date the repository bound.
    fn stored(binds: AuthFraudBinds, fraud_rpt_date: Option<NaiveDate>) -> AuthFraudRow {
        AuthFraudRow {
            card_num: binds.card_num,
            auth_ts: binds.auth_ts,
            auth_type: binds.auth_type,
            card_expiry_date: binds.card_expiry_date,
            message_type: binds.message_type,
            message_source: binds.message_source,
            auth_id_code: binds.auth_id_code,
            auth_resp_code: binds.auth_resp_code,
            auth_resp_reason: binds.auth_resp_reason,
            processing_code: binds.processing_code,
            transaction_amt: binds.transaction_amt,
            approved_amt: binds.approved_amt,
            merchant_catagory_code: binds.merchant_catagory_code,
            acqr_country_code: binds.acqr_country_code,
            pos_entry_mode: binds.pos_entry_mode,
            merchant_id: binds.merchant_id,
            merchant_name: binds.merchant_name,
            merchant_city: binds.merchant_city,
            merchant_state: binds.merchant_state,
            merchant_zip: binds.merchant_zip,
            transaction_id: binds.transaction_id,
            match_status: binds.match_status,
            auth_fraud: binds.auth_fraud,
            fraud_rpt_date,
            acct_id: binds.acct_id,
            cust_id: binds.cust_id,
        }
    }

    fn stored_record() -> Result<AuthFraudRow, Box<dyn StdError>> {
        Ok(stored(
            AuthFraudBinds::from_record(&record()?)?,
            Some(report_date()?),
        ))
    }

    fn decode_error(column: &'static str, kind: DecodeKind) -> PersistenceError {
        PersistenceError::decode(column, kind)
    }

    // audit: COPAUS2C-007
    // audit: PIC-001
    // audit: PIC-003
    #[test]
    fn binds_store_every_column_and_read_back_equal() -> TestResult {
        let original = record()?;
        let binds = AuthFraudBinds::from_record(&original)?;
        assert_eq!(binds.card_num.as_bytes(), CARD);
        assert_eq!(binds.auth_ts, auth_ts()?);
        assert_eq!(binds.card_expiry_date.as_deref(), Some("0925"));
        assert_eq!(binds.processing_code.as_deref(), Some("004000"));
        assert_eq!(binds.transaction_amt.map(|amt| amt.scale()), Some(2));
        assert_eq!(binds.approved_amt, Some("-0.01".parse::<Decimal>()?));
        assert_eq!(binds.pos_entry_mode, Some(99));
        assert_eq!(
            binds.merchant_name.as_deref(),
            Some("CORNER STORE          ")
        );
        assert_eq!(binds.auth_fraud.as_deref(), Some("F"));
        assert_eq!(binds.acct_id, Some(99_999_999_999));
        assert_eq!(binds.cust_id, Some(999_999_999));
        for date in [Some(report_date()?), None] {
            let row = stored(AuthFraudBinds::from_record(&original)?, date);
            assert_eq!(row.into_record()?, (original.clone(), date));
        }
        Ok(())
    }

    /// COPAUS2C writes a value in every column, so a stored `NULL` has no domain value.
    #[test]
    fn a_null_where_a_value_is_required_is_a_typed_error() -> TestResult {
        let text = |expected| DecodeKind::WidthMismatch {
            expected,
            actual: 0,
        };
        let cases: [Case; 23] = [
            (|row| row.auth_type = None, "auth_type", text(4)),
            (
                |row| row.card_expiry_date = None,
                "card_expiry_date",
                text(4),
            ),
            (|row| row.message_type = None, "message_type", text(6)),
            (|row| row.message_source = None, "message_source", text(6)),
            (|row| row.auth_id_code = None, "auth_id_code", text(6)),
            (|row| row.auth_resp_code = None, "auth_resp_code", text(2)),
            (
                |row| row.auth_resp_reason = None,
                "auth_resp_reason",
                text(4),
            ),
            (|row| row.processing_code = None, "processing_code", text(6)),
            (
                |row| row.transaction_amt = None,
                "transaction_amt",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.approved_amt = None,
                "approved_amt",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.merchant_catagory_code = None,
                "merchant_catagory_code",
                text(4),
            ),
            (
                |row| row.acqr_country_code = None,
                "acqr_country_code",
                text(3),
            ),
            (
                |row| row.pos_entry_mode = None,
                "pos_entry_mode",
                DecodeKind::OutOfRange,
            ),
            (|row| row.merchant_id = None, "merchant_id", text(15)),
            (|row| row.merchant_name = None, "merchant_name", text(22)),
            (|row| row.merchant_city = None, "merchant_city", text(13)),
            (|row| row.merchant_state = None, "merchant_state", text(2)),
            (|row| row.merchant_zip = None, "merchant_zip", text(9)),
            (|row| row.transaction_id = None, "transaction_id", text(15)),
            (|row| row.match_status = None, "match_status", text(1)),
            (|row| row.auth_fraud = None, "auth_fraud", text(1)),
            (|row| row.acct_id = None, "acct_id", DecodeKind::OutOfRange),
            (|row| row.cust_id = None, "cust_id", DecodeKind::OutOfRange),
        ];
        for (clear, column, kind) in cases {
            let mut row = stored_record()?;
            clear(&mut row);
            assert_eq!(
                row.into_record().map(drop),
                Err(decode_error(column, kind)),
                "{column}"
            );
        }
        Ok(())
    }

    /// Stored values no field of the record holds: each is refused, never cut, padded or moved.
    #[test]
    fn stored_values_outside_their_picture_are_refused() -> TestResult {
        let cases: [Case; 9] = [
            (
                |row| row.merchant_name = Some("X".repeat(23)),
                "merchant_name",
                DecodeKind::WidthMismatch {
                    expected: 22,
                    actual: 23,
                },
            ),
            (
                |row| row.merchant_name = Some("CORNER STORE".to_owned()),
                "merchant_name",
                DecodeKind::WidthMismatch {
                    expected: 22,
                    actual: 12,
                },
            ),
            (
                |row| row.card_num = "987654321098765".to_owned(),
                "card_num",
                DecodeKind::WidthMismatch {
                    expected: 16,
                    actual: 15,
                },
            ),
            (
                |row| row.auth_resp_code = Some("0".to_owned()),
                "auth_resp_code",
                DecodeKind::WidthMismatch {
                    expected: 2,
                    actual: 1,
                },
            ),
            (
                |row| row.pos_entry_mode = Some(100),
                "pos_entry_mode",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.pos_entry_mode = Some(-1),
                "pos_entry_mode",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.acct_id = Some(-1),
                "acct_id",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.cust_id = Some(1_000_000_000),
                "cust_id",
                DecodeKind::OutOfRange,
            ),
            (
                |row| row.transaction_amt = Some(Decimal::new(10_000_000_000_000, 3)),
                "transaction_amt",
                DecodeKind::OutOfRange,
            ),
        ];
        for (corrupt, column, kind) in cases {
            let mut row = stored_record()?;
            corrupt(&mut row);
            assert_eq!(
                row.into_record().map(drop),
                Err(decode_error(column, kind)),
                "{column}"
            );
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn text_a_column_cannot_store_is_refused_both_ways() -> TestResult {
        let mut original = record()?;
        original.merchant_city = PicX::from_array(*b"SPRINGFI\xC3\xA9LD ");
        assert_eq!(
            AuthFraudBinds::from_record(&original).map(drop),
            Err(decode_error("merchant_city", DecodeKind::InvalidByte))
        );
        original = record()?;
        original.merchant_name = PicX::from_array(*b"CORNER\0STORE          ");
        assert_eq!(
            AuthFraudBinds::from_record(&original).map(drop),
            Err(decode_error("merchant_name", DecodeKind::InvalidByte))
        );
        let mut row = stored_record()?;
        row.merchant_zip = Some("62701 \u{e9} ".to_owned());
        assert_eq!(
            row.into_record().map(drop),
            Err(decode_error("merchant_zip", DecodeKind::InvalidByte))
        );
        Ok(())
    }

    /// `TIMESTAMP(6)` keeps microseconds: a finer value or a leap second would come back as
    /// another instant, so it is refused rather than moved.
    // audit: COPAUS2C-004
    #[test]
    fn an_auth_ts_the_column_cannot_hold_exactly_is_refused() -> TestResult {
        let date = NaiveDate::from_ymd_opt(2022, 1, 15).ok_or("invalid test date")?;
        let finer = date
            .and_hms_nano_opt(10, 30, 45, 123_000_001)
            .ok_or("invalid test time")?;
        let leap = date
            .and_hms_micro_opt(23, 59, 59, 1_500_000)
            .ok_or("invalid test time")?;
        for value in [finer, leap] {
            let mut original = record()?;
            original.auth_ts = value;
            assert_eq!(
                AuthFraudBinds::from_record(&original).map(drop),
                Err(decode_error("auth_ts", DecodeKind::OutOfRange))
            );
        }
        let whole = date
            .and_hms_micro_opt(23, 59, 59, 999_999)
            .ok_or("invalid test time")?;
        let mut original = record()?;
        original.auth_ts = whole;
        assert_eq!(AuthFraudBinds::from_record(&original)?.auth_ts, whole);
        Ok(())
    }

    /// Cardholder Data Redaction: a failing conversion names its column and class, never the
    /// card number or any part of it.
    // audit: COPAUS2C-013
    // audit: PIC-024
    #[test]
    fn errors_never_repeat_the_card_number() -> TestResult {
        let mut errors = Vec::new();
        for card in ["98765432109876543", "987654321098765\u{e9}", ""] {
            let mut row = stored_record()?;
            row.card_num = card.to_owned();
            errors.extend(row.into_record().err());
        }
        let mut original = record()?;
        original.card_num = CardNumber::new(PicX::from_array(*b"98765432\xFF0987654"));
        errors.extend(AuthFraudBinds::from_record(&original).err());
        assert_eq!(errors.len(), 4);
        for error in errors {
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(text.contains("card_num"), "{text}");
                for piece in ["9876", "5432", "3210", "7654"] {
                    assert!(!text.contains(piece), "{text}");
                }
            }
        }
        Ok(())
    }
}
