//! Codec B: the lossless conversion between a typed `domain::records` value, codec A's output,
//! and a row of the `PostgreSQL` schema that `migrations/*.sql` defines (AAP 0.3.3.5, outcome
//! o4, binding d9).
//!
//! Each child module maps one table, and is named after it (`db2_*` for schema `db2`): a row
//! struct whose fields are the table's columns, a `from_record` that builds it from a record and
//! an `into_record` that turns it back into one. Reading a written row back yields a record equal
//! to the one written, so codec A re-encodes it to the original fixture bytes. Row fields are
//! `pub(crate)`: only `repos` builds and binds rows, and this module holds no SQL text (R4).
//!
//! # Framing contract
//!
//! [`copybook::LineFraming`] describes a fixture line, not a record. Its terminator and body
//! length are never stored, and no table has a column for them (PIC-014). Codec B converts only
//! the `record` of a [`copybook::Framed`] envelope; every round trip that ends in bytes carries
//! the envelope unchanged from codec A's decode to codec A's encode. A 36-byte `cardxref.txt`
//! line therefore stores its absent `FILLER` as 14 spaces in `filler CHAR(14)`, and codec A
//! re-emits the line at 36 bytes from the envelope (PIC-013, E04).
//!
//! # Column types (AAP 0.3.3.4)
//!
//! The functions below are the only column conversions; every child converts through them.
//!
//! | Picture | Column | Row field | Conversion |
//! |---|---|---|---|
//! | `X(n)` and `FILLER X(n)` | `CHAR(n)`, which keeps and returns the padding | `String` of n ASCII bytes | `pic_x_to_string`, `string_to_pic_x` (PIC-001, PIC-002, PIC-012, E05) |
//! | `9(n)` | `BIGINT` with a `0 .. 10^n − 1` range check | `i64` | `pic_u_to_i64`, `i64_to_pic_u` (PIC-003, PIC-023) |
//! | `S9(n)`, DISPLAY or `COMP-3` | `BIGINT` with a symmetric range check | `i64` | `pic_s_to_i64`, `i64_to_pic_s` (PIC-010, PIC-023) |
//! | `S9(n)V99` money, DISPLAY or `COMP-3` | `NUMERIC(n+2,2)` | `Decimal` | `money_to_decimal`, `decimal_to_money` (PIC-005, PIC-010) |
//! | `S9(04)V99` rate | `NUMERIC(6,2)` | `Decimal` | `rate_to_decimal`, `decimal_to_rate` (PIC-005) |
//! | `S9(4) COMP` | `SMALLINT` | `i16` | `count_to_i16`, `i16_to_count` (PIC-011) |
//! | `X(10)` date | `DATE`, `NULL` for 10 spaces | `Option<NaiveDate>` | `date_to_column`, `column_to_date` (PIC-015, PIC-016) |
//! | `X(26)` timestamp | `TIMESTAMP(6)` plus `<col>_form CHAR(1)`, both `NULL` for 26 spaces | `Option<NaiveDateTime>`, `Option<String>` | `timestamp_to_columns`, `columns_to_timestamp` (PIC-017 to PIC-021, E33) |
//! | Db2 `VARCHAR(n)` | `VARCHAR(n)`, stored verbatim | `String` of at most n ASCII bytes | `string_to_varchar`, `varchar_to_string` |
//!
//! A date or timestamp travels as a `chrono` value; the layout code beside a timestamp is the
//! only text, and it is not a date (Typed Dates and Timestamps, E33). Protected fields reach these
//! functions only through the `expose()` of their `domain::sensitive` wrapper (PIC-024), and a
//! password never does: `users.password_hash` holds its argon2id hash in place of `SEC-USR-PWD`
//! (E25).
//!
//! # Errors
//!
//! Every fallible conversion names its column and returns a [`PersistenceError::Decode`] whose
//! [`DecodeKind`] gives the class, never the value, which can be a card number or SSN (Cardholder
//! Data Redaction). A value that does not fit its picture is an error, never truncated, padded,
//! trimmed or re-encoded in another layout (R7, Total Parsers Over Untrusted Input). Every row
//! struct leaves `Debug` underived for the same reason.

pub mod accounts;
pub mod auth_fraud;
pub mod card_xref;
pub mod cards;
pub mod cssl_error_log;
pub mod customers;
pub mod db2_transaction_type_categories;
pub mod db2_transaction_types;
pub mod disclosure_groups;
pub mod pending_auth_details;
pub mod pending_auth_summaries;
pub mod tran_cat_balances;
pub mod transaction_categories;
pub mod transaction_types;
pub mod transactions;
pub mod users;

pub use accounts::AccountRow;
pub use auth_fraud::{AuthFraudBinds, AuthFraudRow};
pub use card_xref::CardXrefRow;
pub use cards::{CardBinds, CardRow};
pub use cssl_error_log::{CsslErrorLogBinds, CsslErrorLogRow};
pub use customers::CustomerRow;
pub use db2_transaction_type_categories::Db2TransactionTypeCategoryRow;
pub use db2_transaction_types::Db2TransactionTypeRow;
pub use disclosure_groups::DisclosureGroupRow;
pub use pending_auth_details::PendingAuthDetailRow;
pub use pending_auth_summaries::PendingAuthSummaryRow;
pub use tran_cat_balances::TranCatBalanceRow;
pub use transaction_categories::TransactionCategoryRow;
pub use transaction_types::TransactionTypeRow;
pub use transactions::TransactionRow;
pub use users::{UserEntry, UserListEntry, UserListRow, UserRow};

use chrono::{NaiveDate, NaiveDateTime};
use copybook::{CobolTimestamp, Count, PicDate, PicS, PicTimestamp, PicU, PicX, TsForm};
use domain::money::{Money, Rate};
use rust_decimal::Decimal;

use crate::error::{DecodeKind, PersistenceError};

/// The `PIC X(n)` field's bytes as the text of its `CHAR(n)` column, every byte kept, padding
/// and `FILLER` contents included (PIC-001, PIC-012, E05).
///
/// # Errors
///
/// [`DecodeKind::InvalidByte`] for a NUL or non-ASCII byte: no `PostgreSQL` text value holds NUL,
/// and `CHAR(n)` counts characters, so neither could be stored byte for byte. Every fixture byte
/// is printable ASCII, so such a byte is an anomaly to escalate, never one to replace (PIC-002,
/// R7).
pub(crate) fn pic_x_to_string<const N: usize>(
    column: &'static str,
    value: &PicX<N>,
) -> Result<String, PersistenceError> {
    stored_text(column, value.as_bytes()).map(str::to_owned)
}

/// The `PIC X(n)` field a `CHAR(n)` column holds, byte for byte (PIC-001, PIC-012).
///
/// `CHAR(n)` returns its padding, so a value written through [`pic_x_to_string`] reads back as
/// exactly N bytes. The bytes are kept as stored, without codec A's printable-ASCII restriction,
/// so that every value the write path accepts reads back equal.
///
/// # Errors
///
/// [`DecodeKind::InvalidByte`] for a NUL or non-ASCII character (PIC-002);
/// [`DecodeKind::WidthMismatch`] when the text is not exactly N bytes long.
pub(crate) fn string_to_pic_x<const N: usize>(
    column: &'static str,
    text: &str,
) -> Result<PicX<N>, PersistenceError> {
    let bytes = stored_text(column, text.as_bytes())?.as_bytes();
    <[u8; N]>::try_from(bytes)
        .map(PicX::from_array)
        .map_err(|_| {
            PersistenceError::decode(
                column,
                DecodeKind::WidthMismatch {
                    expected: N,
                    actual: bytes.len(),
                },
            )
        })
}

/// A Db2 `VARCHAR(max)` value bound as it stands, length included: never trimmed, padded or
/// upper-cased, as the stored length is part of the value (`DCLTRTYP.dcl`, `TRNTYPE.ddl:3`).
///
/// # Errors
///
/// [`DecodeKind::InvalidByte`] for a NUL or non-ASCII character; [`DecodeKind::WidthMismatch`]
/// when the text is longer than `max` bytes.
pub(crate) fn string_to_varchar(
    column: &'static str,
    text: &str,
    max: usize,
) -> Result<String, PersistenceError> {
    bounded_text(column, text, max)
}

/// The text of a Db2 `VARCHAR(max)` column, exactly as stored, its length included.
///
/// # Errors
///
/// [`DecodeKind::InvalidByte`] for a NUL or non-ASCII character; [`DecodeKind::WidthMismatch`]
/// when the text is longer than `max` bytes.
pub(crate) fn varchar_to_string(
    column: &'static str,
    text: &str,
    max: usize,
) -> Result<String, PersistenceError> {
    bounded_text(column, text, max)
}

/// The `BIGINT` value of a `PIC 9(n)` field (PIC-003, PIC-023).
///
/// # Errors
///
/// [`DecodeKind::OutOfRange`] when the value exceeds `i64::MAX`, which no width up to 18 digits
/// reaches.
pub(crate) fn pic_u_to_i64<const N: u32>(
    column: &'static str,
    value: PicU<N>,
) -> Result<i64, PersistenceError> {
    // TRUNCATION: none survives. The conversion is checked: a `PicU<N>` value is at most
    // 10^18 − 1, below `i64::MAX`, so it is exact, and a larger one would be an error.
    i64::try_from(value.get()).map_err(|_| PersistenceError::decode(column, DecodeKind::OutOfRange))
}

/// The `PIC 9(n)` field a `BIGINT` column holds (PIC-003, PIC-023).
///
/// # Errors
///
/// [`DecodeKind::OutOfRange`] for a negative value or one above `10^N − 1`.
pub(crate) fn i64_to_pic_u<const N: u32>(
    column: &'static str,
    value: i64,
) -> Result<PicU<N>, PersistenceError> {
    // TRUNCATION: none survives. The conversion is checked: a value from 0 to 10^N − 1 converts
    // exactly, and a negative one or one above 10^N − 1 is `OutOfRange`, never wrapped or
    // truncated.
    PicU::try_from(value).map_err(|_| PersistenceError::decode(column, DecodeKind::OutOfRange))
}

/// The `BIGINT` value of a signed integral `PIC S9(n)` field, DISPLAY or `COMP-3`, such as the
/// authorization account key `PA-ACCT-ID PIC S9(11) COMP-3` (PIC-010, PIC-023).
#[must_use]
pub(crate) fn pic_s_to_i64<const N: u32>(value: PicS<N>) -> i64 {
    value.get()
}

/// The `PIC S9(n)` field a `BIGINT` column holds (PIC-010, PIC-023).
///
/// # Errors
///
/// [`DecodeKind::OutOfRange`] when the magnitude exceeds `10^N − 1`.
pub(crate) fn i64_to_pic_s<const N: u32>(
    column: &'static str,
    value: i64,
) -> Result<PicS<N>, PersistenceError> {
    PicS::new(value).map_err(|_| PersistenceError::decode(column, DecodeKind::OutOfRange))
}

/// The `NUMERIC(n+2,2)` value of an `S9(n)V99` amount, at scale 2 (PIC-005, PIC-010).
#[must_use]
pub(crate) fn money_to_decimal<const N: u32>(value: Money<N>) -> Decimal {
    value.as_decimal()
}

/// The `S9(n)V99` amount a `NUMERIC(n+2,2)` column holds, exactly: nothing is rounded or
/// truncated (PIC-005, PIC-010).
///
/// # Errors
///
/// [`DecodeKind::OutOfRange`] for a magnitude of `10^N` or more, or a non-zero digit beyond the
/// second decimal place.
pub(crate) fn decimal_to_money<const N: u32>(
    column: &'static str,
    value: Decimal,
) -> Result<Money<N>, PersistenceError> {
    Money::new(value).map_err(|_| PersistenceError::decode(column, DecodeKind::OutOfRange))
}

/// The `NUMERIC(6,2)` value of the `DIS-INT-RATE PIC S9(04)V99` rate, at scale 2 (PIC-005).
#[must_use]
pub(crate) fn rate_to_decimal<const N: u32>(value: Rate<N>) -> Decimal {
    value.as_decimal()
}

/// The rate a `NUMERIC(6,2)` column holds, exactly: nothing is rounded or truncated (PIC-005).
///
/// # Errors
///
/// [`DecodeKind::OutOfRange`] for a magnitude of `10^N` or more, or a non-zero digit beyond the
/// second decimal place.
pub(crate) fn decimal_to_rate<const N: u32>(
    column: &'static str,
    value: Decimal,
) -> Result<Rate<N>, PersistenceError> {
    Rate::new(value).map_err(|_| PersistenceError::decode(column, DecodeKind::OutOfRange))
}

/// The `SMALLINT` value of a `PIC S9(4) COMP` count (PIC-011).
#[must_use]
pub(crate) fn count_to_i16(value: Count) -> i16 {
    value.get()
}

/// The `PIC S9(4) COMP` count a `SMALLINT` column holds. Infallible: every halfword is a valid
/// two-byte binary field, and `SMALLINT` holds the same range (PIC-011).
#[must_use]
pub(crate) fn i16_to_count(value: i16) -> Count {
    Count::new(value)
}

/// The `DATE` value of a `PIC X(10)` date: `None`, stored `NULL`, for 10 spaces (PIC-015).
#[must_use]
pub(crate) fn date_to_column(value: PicDate) -> Option<NaiveDate> {
    value.value()
}

/// The `PIC X(10)` date a `DATE` column holds; `NULL` is 10 spaces (PIC-015, PIC-016).
///
/// # Errors
///
/// [`DecodeKind::OutOfRange`] for a year outside `0..=9999`, which the four-digit `YYYY-MM-DD`
/// text of codec A cannot carry.
pub(crate) fn column_to_date(
    column: &'static str,
    value: Option<NaiveDate>,
) -> Result<PicDate, PersistenceError> {
    PicDate::new(value).map_err(|_| PersistenceError::decode(column, DecodeKind::OutOfRange))
}

/// The `TIMESTAMP(6)` value of a `PIC X(26)` timestamp and the layout code of its
/// `<col>_form CHAR(1)` column: `I`, `D` or `S` (PIC-018 to PIC-020), or both `None`, stored
/// `NULL`, for 26 spaces (PIC-017, E33).
#[must_use]
pub(crate) fn timestamp_to_columns(
    value: &PicTimestamp,
) -> (Option<NaiveDateTime>, Option<String>) {
    value
        .value()
        .map(|stamp| (stamp.value(), stamp.form().code().to_string()))
        .unzip()
}

/// The `PIC X(26)` timestamp a `TIMESTAMP(6)` column and its `<col>_form` code hold; both
/// `NULL` is 26 spaces (PIC-017 to PIC-021, E33).
///
/// # Errors
///
/// - [`DecodeKind::UnknownForm`] for a code that is not exactly one of `I`, `D` and `S`.
/// - [`DecodeKind::FormMismatch`] for a value without a code, a code without a value, or a value
///   its layout cannot reproduce: a time other than midnight in layout `S`, a time finer than a
///   microsecond, a leap second, or a year outside `0..=9999`. Such a value is an error, never
///   written in another layout (Typed Dates and Timestamps).
pub(crate) fn columns_to_timestamp(
    column: &'static str,
    value: Option<NaiveDateTime>,
    form: Option<String>,
) -> Result<PicTimestamp, PersistenceError> {
    let form = form.map(|code| layout(column, &code)).transpose()?;
    match (value, form) {
        (None, None) => Ok(PicTimestamp::BLANK),
        (Some(value), Some(form)) => CobolTimestamp::new(value, form)
            .map(|stamp| PicTimestamp::new(Some(stamp)))
            .map_err(|_| PersistenceError::decode(column, DecodeKind::FormMismatch)),
        (Some(_), None) | (None, Some(_)) => {
            Err(PersistenceError::decode(column, DecodeKind::FormMismatch))
        }
    }
}

/// The class of a `copybook` or `domain` constructor's rejection, such as a description over
/// the 50 bytes of `Db2TransactionType::new`, for the column whose value it rejected. The
/// [`copybook::Error`] holds field name, offset and width, never the value.
#[must_use]
pub(crate) fn copybook_kind(error: &copybook::Error) -> DecodeKind {
    DecodeKind::Copybook(error.kind)
}

/// `bytes` as text that a `PostgreSQL` text column stores and returns byte for byte: ASCII
/// without NUL (PIC-002).
fn stored_text<'a>(column: &'static str, bytes: &'a [u8]) -> Result<&'a str, PersistenceError> {
    match std::str::from_utf8(bytes) {
        Ok(text) if bytes.iter().all(|&byte| byte != 0 && byte.is_ascii()) => Ok(text),
        _ => Err(PersistenceError::decode(column, DecodeKind::InvalidByte)),
    }
}

/// `text` unchanged, when it is storable text of at most `max` bytes.
fn bounded_text(column: &'static str, text: &str, max: usize) -> Result<String, PersistenceError> {
    let checked = stored_text(column, text.as_bytes())?;
    if checked.len() > max {
        return Err(PersistenceError::decode(
            column,
            DecodeKind::WidthMismatch {
                expected: max,
                actual: checked.len(),
            },
        ));
    }
    Ok(checked.to_owned())
}

/// The layout a `<col>_form CHAR(1)` code names.
fn layout(column: &'static str, code: &str) -> Result<TsForm, PersistenceError> {
    let mut chars = code.chars();
    match (chars.next(), chars.next()) {
        (Some(only), None) => TsForm::from_code(only)
            .map_err(|_| PersistenceError::decode(column, DecodeKind::UnknownForm)),
        _ => Err(PersistenceError::decode(column, DecodeKind::UnknownForm)),
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use copybook::{
        CobolTimestamp, Count, ErrorKind, PicDate, PicS, PicTimestamp, PicU, PicX, TsForm,
    };
    use domain::money::{Money, Rate};
    use rust_decimal::Decimal;

    use super::{
        column_to_date, columns_to_timestamp, copybook_kind, count_to_i16, date_to_column,
        decimal_to_money, decimal_to_rate, i16_to_count, i64_to_pic_s, i64_to_pic_u,
        money_to_decimal, pic_s_to_i64, pic_u_to_i64, pic_x_to_string, rate_to_decimal,
        string_to_pic_x, string_to_varchar, timestamp_to_columns, varchar_to_string,
    };
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    const COLUMN: &str = "test_col";

    /// Not a fixture card number: errors must not repeat whatever value they rejected.
    const CARD_DIGITS: &str = "9876543210987654";

    fn decode_error(kind: DecodeKind) -> PersistenceError {
        PersistenceError::decode(COLUMN, kind)
    }

    fn day(year: i32, month: u32, date: u32) -> Result<NaiveDate, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(year, month, date).ok_or_else(|| "invalid test date".into())
    }

    fn at(
        date: NaiveDate,
        (hour, minute, second): (u32, u32, u32),
        micro: u32,
    ) -> Result<NaiveDateTime, Box<dyn StdError>> {
        NaiveTime::from_hms_micro_opt(hour, minute, second, micro)
            .map(|time| date.and_time(time))
            .ok_or_else(|| "invalid test time".into())
    }

    // audit: PIC-001
    // audit: PIC-012
    #[test]
    fn pic_x_keeps_every_byte_both_ways() -> TestResult {
        for bytes in [*b"AB  ", *b"    ", *b"0X1Z", *b" ~a{"] {
            let field = PicX::<4>::from_array(bytes);
            let text = pic_x_to_string(COLUMN, &field)?;
            assert_eq!(text.as_bytes(), &bytes);
            assert_eq!(string_to_pic_x::<4>(COLUMN, &text)?, field);
        }
        Ok(())
    }

    // audit: PIC-002
    #[test]
    fn nul_and_non_ascii_bytes_are_invalid_both_ways() {
        let invalid = Err(decode_error(DecodeKind::InvalidByte));
        for bytes in [[b'A', 0, b'B'], [b'A', 0xC3, 0xA9], [0x80, b' ', b' ']] {
            let field = PicX::<3>::from_array(bytes);
            assert_eq!(pic_x_to_string(COLUMN, &field).map(drop), invalid);
        }
        for text in ["A\0B", "A\u{e9}", "\u{20ac}"] {
            assert_eq!(string_to_pic_x::<3>(COLUMN, text).map(drop), invalid);
            assert_eq!(string_to_varchar(COLUMN, text, 50).map(drop), invalid);
            assert_eq!(varchar_to_string(COLUMN, text, 50).map(drop), invalid);
        }
    }

    #[test]
    fn char_text_must_be_exactly_the_picture_width() {
        for (text, actual) in [("ABC", 3), ("ABCDE", 5), ("", 0)] {
            assert_eq!(
                string_to_pic_x::<4>(COLUMN, text),
                Err(decode_error(DecodeKind::WidthMismatch {
                    expected: 4,
                    actual
                }))
            );
        }
    }

    // audit: PIC-003
    // audit: PIC-023
    #[test]
    fn unsigned_keys_round_trip_and_reject_out_of_range() -> TestResult {
        for value in [0, 1, 99_999_999_999] {
            let key = PicU::<11>::new(value)?;
            let column = pic_u_to_i64(COLUMN, key)?;
            assert_eq!(i64_to_pic_u::<11>(COLUMN, column)?, key);
        }
        let largest = PicU::<18>::new(PicU::<18>::MAX)?;
        assert_eq!(
            i64_to_pic_u::<18>(COLUMN, pic_u_to_i64(COLUMN, largest)?)?,
            largest
        );
        for value in [-1, i64::MIN, 100_000_000_000, i64::MAX] {
            assert_eq!(
                i64_to_pic_u::<11>(COLUMN, value),
                Err(decode_error(DecodeKind::OutOfRange))
            );
        }
        Ok(())
    }

    // audit: PIC-010
    #[test]
    fn signed_keys_round_trip_and_reject_out_of_range() -> TestResult {
        for value in [-99_999_999_999, -1, 0, 99_999_999_999] {
            let key = PicS::<11>::new(value)?;
            assert_eq!(i64_to_pic_s::<11>(COLUMN, pic_s_to_i64(key))?, key);
        }
        for value in [100_000_000_000, -100_000_000_000, i64::MIN, i64::MAX] {
            assert_eq!(
                i64_to_pic_s::<11>(COLUMN, value),
                Err(decode_error(DecodeKind::OutOfRange))
            );
        }
        Ok(())
    }

    // audit: PIC-005
    #[test]
    fn money_round_trips_exactly_and_rejects_what_its_picture_cannot_hold() -> TestResult {
        for text in ["0.00", "-123.45", "9999999999.99", "-9999999999.99", "0.01"] {
            let amount = Money::<10>::new(text.parse::<Decimal>()?)?;
            let column = money_to_decimal(amount);
            assert_eq!(column.scale(), 2);
            assert_eq!(decimal_to_money::<10>(COLUMN, column)?, amount);
        }
        let out_of_range = Err(decode_error(DecodeKind::OutOfRange));
        for text in ["10000000000.00", "-10000000000.00", "1.234"] {
            let column = text.parse::<Decimal>()?;
            assert_eq!(
                decimal_to_money::<10>(COLUMN, column).map(drop),
                out_of_range
            );
        }
        // Ten integer digits fit `S9(10)V99` and not `S9(09)V99`.
        let ten_digits = "1000000000.00".parse::<Decimal>()?;
        assert_eq!(decimal_to_money::<10>(COLUMN, ten_digits).map(drop), Ok(()));
        assert_eq!(
            decimal_to_money::<9>(COLUMN, ten_digits).map(drop),
            out_of_range
        );
        Ok(())
    }

    #[test]
    fn rates_round_trip_exactly_and_reject_out_of_range() -> TestResult {
        for text in ["15.00", "0.00", "-25.50", "9999.99"] {
            let rate = Rate::<4>::new(text.parse::<Decimal>()?)?;
            let back = decimal_to_rate::<4>(COLUMN, rate_to_decimal(rate))?;
            assert_eq!(back, rate);
            assert_eq!(back.is_zero(), rate.is_zero());
        }
        for text in ["10000.00", "-10000.00", "1.005"] {
            assert_eq!(
                decimal_to_rate::<4>(COLUMN, text.parse::<Decimal>()?),
                Err(decode_error(DecodeKind::OutOfRange))
            );
        }
        Ok(())
    }

    // audit: PIC-011
    #[test]
    fn every_smallint_is_a_count() {
        for value in [i16::MIN, -9_999, -1, 0, 1, 9_999, i16::MAX] {
            assert_eq!(count_to_i16(i16_to_count(value)), value);
            assert_eq!(i16_to_count(value), Count::new(value));
        }
    }

    // audit: PIC-015
    // audit: PIC-016
    #[test]
    fn dates_round_trip_and_blank_is_null() -> TestResult {
        assert_eq!(date_to_column(PicDate::BLANK), None);
        assert_eq!(column_to_date(COLUMN, None)?, PicDate::BLANK);
        for date in [day(2014, 11, 20)?, day(0, 1, 1)?, day(9999, 12, 31)?] {
            let field = PicDate::new(Some(date))?;
            assert_eq!(date_to_column(field), Some(date));
            assert_eq!(column_to_date(COLUMN, Some(date))?, field);
        }
        for date in [day(10_000, 1, 1)?, day(-1, 12, 31)?] {
            assert_eq!(
                column_to_date(COLUMN, Some(date)),
                Err(decode_error(DecodeKind::OutOfRange))
            );
        }
        Ok(())
    }

    // audit: PIC-017
    // audit: PIC-018
    // audit: PIC-019
    // audit: PIC-020
    #[test]
    fn timestamps_round_trip_in_every_layout() -> TestResult {
        assert_eq!(timestamp_to_columns(&PicTimestamp::BLANK), (None, None));
        assert_eq!(
            columns_to_timestamp(COLUMN, None, None)?,
            PicTimestamp::BLANK
        );
        let cases: [(&[u8; 26], &str); 4] = [
            (b"2022-06-10 19:27:53.000000", "I"),
            (b"2022-07-18-23.59.59.120000", "D"),
            (b"0001-01-01-00.00.00.123456", "D"),
            (b"2022-07-18                ", "S"),
        ];
        for (text, code) in cases {
            let field = PicTimestamp::decode(text)?;
            let (value, form) = timestamp_to_columns(&field);
            assert_eq!(form.as_deref(), Some(code));
            let back = columns_to_timestamp(COLUMN, value, form)?;
            assert_eq!(back, field);
            let mut bytes = Vec::new();
            back.encode_into(&mut bytes);
            assert_eq!(bytes.as_slice(), text.as_slice());
        }
        Ok(())
    }

    // audit: PIC-021
    #[test]
    fn unknown_layout_codes_are_rejected() -> TestResult {
        let value = Some(at(day(2022, 6, 10)?, (19, 27, 53), 0)?);
        for code in ["X", "", "ID", " ", "i", "\0"] {
            for stamp in [value, None] {
                assert_eq!(
                    columns_to_timestamp(COLUMN, stamp, Some(code.to_owned())),
                    Err(decode_error(DecodeKind::UnknownForm))
                );
            }
        }
        Ok(())
    }

    #[test]
    fn value_and_code_must_agree() -> TestResult {
        let date = day(2022, 7, 18)?;
        let noon = at(date, (12, 0, 0), 0)?;
        let too_fine = date
            .and_hms_nano_opt(19, 27, 53, 1)
            .ok_or("invalid test time")?;
        let leap = date
            .and_hms_micro_opt(23, 59, 59, 1_500_000)
            .ok_or("invalid test time")?;
        let far = at(day(10_000, 1, 1)?, (0, 0, 0), 0)?;
        let cases = [
            (Some(noon), None),
            (None, Some("I")),
            (None, Some("S")),
            (Some(noon), Some("S")),
            (Some(too_fine), Some("I")),
            (Some(leap), Some("D")),
            (Some(far), Some("I")),
        ];
        for (stamp, code) in cases {
            assert_eq!(
                columns_to_timestamp(COLUMN, stamp, code.map(str::to_owned)),
                Err(decode_error(DecodeKind::FormMismatch))
            );
        }
        let midnight = CobolTimestamp::new(at(date, (0, 0, 0), 0)?, TsForm::ScreenDate)?;
        assert_eq!(
            columns_to_timestamp(COLUMN, Some(midnight.value()), Some("S".to_owned()))?,
            PicTimestamp::new(Some(midnight))
        );
        Ok(())
    }

    #[test]
    fn varchar_text_is_kept_verbatim_up_to_its_maximum() -> TestResult {
        let padded = format!("{:<50}", "PURCHASE");
        for text in [
            "PURCHASE",
            "REVERAL",
            "  leading and trailing  ",
            "",
            padded.as_str(),
        ] {
            assert_eq!(string_to_varchar(COLUMN, text, 50)?, text);
            assert_eq!(varchar_to_string(COLUMN, text, 50)?, text);
        }
        let long = "X".repeat(51);
        let too_long = Err(decode_error(DecodeKind::WidthMismatch {
            expected: 50,
            actual: 51,
        }));
        assert_eq!(string_to_varchar(COLUMN, &long, 50), too_long);
        assert_eq!(varchar_to_string(COLUMN, &long, 50), too_long);
        Ok(())
    }

    #[test]
    fn copybook_rejections_keep_their_class() {
        for kind in [ErrorKind::OutOfRange, ErrorKind::NonPrintable] {
            let error = copybook::Error::new("TR-DESCRIPTION-TEXT", 0, 50, kind);
            assert_eq!(copybook_kind(&error), DecodeKind::Copybook(kind));
        }
    }

    /// Cardholder Data Redaction: an error names its column and class, never the value.
    // audit: PIC-024
    #[test]
    fn errors_never_repeat_the_value() {
        let outcomes = [
            string_to_pic_x::<16>(COLUMN, &format!("{CARD_DIGITS}\0")).map(drop),
            string_to_pic_x::<17>(COLUMN, CARD_DIGITS).map(drop),
            string_to_varchar(COLUMN, CARD_DIGITS, 15).map(drop),
            i64_to_pic_u::<9>(COLUMN, 9_876_543_210).map(drop),
        ];
        let errors: Vec<PersistenceError> = outcomes.into_iter().filter_map(Result::err).collect();
        assert_eq!(errors.len(), 4);
        for error in errors {
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(text.contains(COLUMN), "{text}");
                assert!(!text.contains("9876"), "{text}");
            }
        }
    }
}
