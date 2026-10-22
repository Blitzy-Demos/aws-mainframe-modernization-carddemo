#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! Copybook record layouts as typed structs, one per layout, with their codec A byte form
//! (AAP 0.3.3.5, outcome o4).
//!
//! Each child module ports one layout of `modern/audit/copybooks.md`: a struct whose fields are
//! the layout's, in declaration order, and, for every layout with a byte form, an implementation
//! of [`FixedRecord`](copybook::FixedRecord) built from [`RecordReader`] and [`RecordWriter`]
//! calls in that same order.
//!
//! | Module | Type | Layout | Bytes |
//! |---|---|---|---|
//! | [`account`] | [`AccountRecord`] | `CVACT01Y` `ACCOUNT-RECORD` | 300 |
//! | [`card`] | [`CardRecord`] | `CVACT02Y` `CARD-RECORD` | 150 |
//! | [`card_xref`] | [`CardXrefRecord`] | `CVACT03Y` `CARD-XREF-RECORD` | 50 |
//! | [`customer`] | [`CustomerRecord`] | `CVCUS01Y` `CUSTOMER-RECORD` | 500 |
//! | [`transaction`] | [`TransactionRecord`] | `CVTRA05Y` `TRAN-RECORD` | 350 |
//! | [`daily_transaction`] | [`DailyTransactionRecord`] | `CVTRA06Y` `DALYTRAN-RECORD` | 350 |
//! | [`reject`] | [`RejectRecord`] | `CBTRN02C` `REJECT-RECORD` (`CBTRN02C.cbl:176-182`) | 430 |
//! | [`tran_cat_balance`] | [`TranCatBalanceRecord`] | `CVTRA01Y` `TRAN-CAT-BAL-RECORD` | 50 |
//! | [`disclosure_group`] | [`DisclosureGroupRecord`] | `CVTRA02Y` `DIS-GROUP-RECORD` | 50 |
//! | [`transaction_type`] | [`TransactionTypeRecord`] | `CVTRA03Y` `TRAN-TYPE-RECORD` | 60 |
//! | [`transaction_category`] | [`TransactionCategoryRecord`] | `CVTRA04Y` `TRAN-CAT-RECORD` | 60 |
//! | [`db2_transaction_type`] | [`Db2TransactionType`] | DCLGEN `DCLTRTYP`, the Db2 data version (E23) | row only |
//! | [`db2_transaction_category`] | [`Db2TransactionCategory`] | DCLGEN `DCLTRCAT`, the Db2 data version (E23) | row only |
//! | [`user_security`] | [`UserSecurityRecord`]; [`UserEntry`], [`UserListEntry`] | `CSUSR01Y` `SEC-USER-DATA`; its `users` projections, `password_hash` in place of `SEC-USR-PWD` (E25) | 80; row only |
//! | [`pending_auth_summary`] | [`PendingAuthSummaryRecord`] | `CIPAUSMY`, IMS segment `PAUTSUM0` | 100 |
//! | [`pending_auth_detail`] | [`PendingAuthDetailRecord`] | `CIPAUDTY`, IMS segment `PAUTDTL1` | 200 |
//! | [`auth_request`] | [`AuthRequestRecord`] | `CCPAURQY`, the CP00 request storage | 153 |
//! | [`auth_reply`] | [`AuthReply`] | `CCPAURLY`, the CP00 reply storage | 57 |
//! | [`error_log`] | [`ErrorLogRecord`], [`ErrorLevel`], [`ErrorSubsystem`] | `CCPAUERY` `ERROR-LOG-RECORD` | 122 |
//!
//! The two Db2 row types follow the DCLGEN host variables and have no byte form or fixture
//! framing. [`UserEntry`] and [`UserListEntry`] are the stored projections of `SEC-USER-DATA`
//! that codec B reads and writes, and have no byte form either. The CP00 wire bodies are
//! comma-delimited and are parsed and built by `crate::messaging`; the `auth_request` and
//! `auth_reply` byte forms are the copybook storage layouts, not the wire text.
//!
//! # Byte identity
//!
//! For every slice a `decode` accepts, `encode` returns the same bytes. `PIC X(n)` bytes are kept
//! verbatim, padding included (PIC-001), and every `FILLER` keeps its bytes, whatever they hold
//! (PIC-012, E05). Leading zeros, overpunch signs and packed sign nibbles come back from the
//! value (PIC-003, PIC-006, PIC-010). A record whose trailing `FILLER` a fixture line may lack
//! declares it as [`FixedRecord::TRAILING_FILLER`](copybook::FixedRecord::TRAILING_FILLER), and
//! `copybook::framing` supplies and re-trims that tail with the line's terminator (PIC-013,
//! PIC-014, E04).
//!
//! # Typed errors and totality
//!
//! Input that could not round-trip is a [`copybook::Error`] naming the field, its offset and its
//! width, never a default or a normalised value (R7): a byte outside printable ASCII in a
//! `PIC X(n)` field (PIC-002), a non-digit in a `PIC 9(n)` field (PIC-004), a plain digit in a
//! sign position, a negative zero or a bad overpunch (PIC-007 to PIC-009), a bad packed nibble
//! (PIC-010), a date or timestamp in no known layout (PIC-016, PIC-021). Every `decode` is total:
//! for any slice of any length it returns a record or such an error and never panics (Total
//! Parsers Over Untrusted Input). The attribute on this file's first line denies `unwrap`,
//! `expect` and indexing in this module, in every child module and in their unit tests.
//!
//! # Field types
//!
//! - `PIC X(10)` dates are [`PicDate`](copybook::PicDate) and `PIC X(26)` timestamps
//!   [`PicTimestamp`](copybook::PicTimestamp), never text (PIC-015, PIC-017 to PIC-021, Typed
//!   Dates and Timestamps).
//! - Signed amounts are [`Money`] and `DIS-INT-RATE` is [`Rate`], at scale 2 (PIC-005, PIC-010);
//!   integral fields are 64-bit `PicU` and `PicS` (PIC-023, Checked Integer Narrowing).
//! - Card numbers, SSNs, CVVs and passwords are the `crate::sensitive` wrappers, whose `Debug`
//!   redacts them, so a derived `Debug` on a record cannot disclose one (PIC-024, Cardholder Data
//!   Redaction).
//!
//! # Shared field helpers
//!
//! The amount pictures need a conversion between the decimal the `copybook` cursors read and the
//! `Money` or `Rate` type that carries the picture's width. The helpers below make it once for
//! every child: `super::zoned_money`, `super::packed_money`, `super::zoned_rate`,
//! `super::edited_amount`, their `put_` mirrors, and `super::varchar` for the Db2 `VARCHAR`
//! host variables. They are private to this module, which is the only place a
//! [`MoneyError`] becomes a codec error. Each takes the reader's offset before it reads, so its
//! errors name the field's own offset and width.

// In the order `cargo fmt` keeps module declarations, which the formatting gate checks; the
// table above lists the layouts in their copybook order.
pub mod account;
pub mod auth_reply;
pub mod auth_request;
pub mod card;
pub mod card_xref;
pub mod customer;
pub mod daily_transaction;
pub mod db2_transaction_category;
pub mod db2_transaction_type;
pub mod disclosure_group;
pub mod error_log;
pub mod pending_auth_detail;
pub mod pending_auth_summary;
pub mod reject;
pub mod tran_cat_balance;
pub mod transaction;
pub mod transaction_category;
pub mod transaction_type;
pub mod user_security;

pub use account::AccountRecord;
pub use auth_reply::AuthReply;
pub use auth_request::AuthRequestRecord;
pub use card::CardRecord;
pub use card_xref::CardXrefRecord;
pub use customer::CustomerRecord;
pub use daily_transaction::DailyTransactionRecord;
pub use db2_transaction_category::Db2TransactionCategory;
pub use db2_transaction_type::Db2TransactionType;
pub use disclosure_group::DisclosureGroupRecord;
pub use error_log::{ErrorLevel, ErrorLogRecord, ErrorSubsystem};
pub use pending_auth_detail::PendingAuthDetailRecord;
pub use pending_auth_summary::PendingAuthSummaryRecord;
pub use reject::RejectRecord;
pub use tran_cat_balance::TranCatBalanceRecord;
pub use transaction::TransactionRecord;
pub use transaction_category::TransactionCategoryRecord;
pub use transaction_type::TransactionTypeRecord;
pub use user_security::{UserEntry, UserListEntry, UserSecurityRecord};

use copybook::edited::EditedPicture;
use copybook::{Error, ErrorKind, PicU, RecordReader, RecordWriter};

use crate::money::{Decimal, Money, MoneyError, Rate};

/// Scale of every amount and rate picture the records hold (`V99`).
const SCALE: u32 = 2;

/// The numeric-edited picture of `PA-RQ-TRANSACTION-AMT` and `PA-RL-APPROVED-AMT`
/// (`CCPAURQY.cpy:27`, `CCPAURLY.cpy:24`; PIC-022).
const EDITED_AMOUNT: EditedPicture = EditedPicture::Plus9x10Dot99;

/// Integer digits of `PIC +9(10).99`.
const EDITED_INT_DIGITS: usize = 10;

/// Cents per unit: the two digits after the point of `PIC +9(10).99`.
const CENTS_PER_UNIT: u64 = 100;

/// The codec error for an amount or rate that its `Money` or `Rate` type rejects: both
/// [`MoneyError`] classes are a value outside the picture, [`ErrorKind::OutOfRange`], reported
/// for `field` at `offset`, `width` bytes wide. Every helper below maps a `MoneyError` through
/// this function and nothing else does.
fn money_error(
    field: &'static str,
    offset: usize,
    width: usize,
) -> impl FnOnce(MoneyError) -> Error {
    move |_: MoneyError| Error::new(field, offset, width, ErrorKind::OutOfRange)
}

/// Bytes the reader consumed since `start`, the width of the field just read.
fn consumed(r: &RecordReader<'_>, start: usize) -> usize {
    r.offset().saturating_sub(start)
}

/// Decodes a signed DISPLAY `PIC S9(N)V99` amount with a trailing overpunch sign, `N + 2` bytes,
/// such as `ACCT-CURR-BAL PIC S9(10)V99` or `TRAN-CAT-BAL PIC S9(09)V99` (PIC-005, PIC-007 to
/// PIC-009).
///
/// # Errors
///
/// Those of [`RecordReader::zoned`] for `N` integer digits at scale 2, which name this field's
/// offset and width; [`ErrorKind::OutOfRange`] if `Money<N>` rejects the decoded value.
fn zoned_money<const N: u32>(
    r: &mut RecordReader<'_>,
    field: &'static str,
) -> Result<Money<N>, Error> {
    let start = r.offset();
    let value = r.zoned(field, N, SCALE)?;
    Money::new(value).map_err(money_error(field, start, consumed(r, start)))
}

/// Appends `value` as a signed DISPLAY `PIC S9(N)V99` field, `N + 2` bytes with a trailing
/// overpunch sign (PIC-006): the mirror of [`zoned_money`].
fn put_zoned_money<'w, const N: u32>(
    w: &'w mut RecordWriter,
    value: &Money<N>,
) -> &'w mut RecordWriter {
    w.zoned(value.as_decimal(), N, SCALE)
}

/// Digits of a `PIC S9(int_digits)V99` field, or [`ErrorKind::OutOfRange`] with width 0 for
/// `field` at `offset` when the count overflows: a picture with no width.
fn amount_digits(field: &'static str, offset: usize, int_digits: u32) -> Result<u32, Error> {
    int_digits
        .checked_add(SCALE)
        .ok_or(Error::new(field, offset, 0, ErrorKind::OutOfRange))
}

/// Decodes a `PIC S9(N)V99 COMP-3` amount, `N + 2` packed digits, such as `PA-CREDIT-LIMIT
/// PIC S9(09)V99 COMP-3` or `PA-TRANSACTION-AMT PIC S9(10)V99 COMP-3` (PIC-010).
///
/// # Errors
///
/// [`ErrorKind::OutOfRange`] with width 0 when `N + 2` overflows; those of
/// [`RecordReader::packed`] for `N + 2` digits at scale 2, which name this field's offset and
/// width; [`ErrorKind::OutOfRange`] if `Money<N>` rejects the decoded value.
fn packed_money<const N: u32>(
    r: &mut RecordReader<'_>,
    field: &'static str,
) -> Result<Money<N>, Error> {
    let start = r.offset();
    let digits = amount_digits(field, start, N)?;
    let value = r.packed(field, digits, SCALE)?;
    Money::new(value).map_err(money_error(field, start, consumed(r, start)))
}

/// Appends `value` as a `PIC S9(N)V99 COMP-3` field, pad nibble 0 for an even digit count and
/// sign nibble `C` or `D` (PIC-010): the mirror of [`packed_money`]. Every `Money<N>` compiles
/// only for `N` up to 18, so `N + 2` never saturates.
fn put_packed_money<'w, const N: u32>(
    w: &'w mut RecordWriter,
    value: &Money<N>,
) -> &'w mut RecordWriter {
    w.packed(value.as_decimal(), N.saturating_add(SCALE), SCALE)
}

/// Decodes a signed DISPLAY `PIC S9(N)V99` rate with a trailing overpunch sign, `N + 2` bytes:
/// `DIS-INT-RATE PIC S9(04)V99` (`CVTRA02Y.cpy:9`; PIC-005, PIC-007 to PIC-009).
///
/// # Errors
///
/// Those of [`RecordReader::zoned`] for `N` integer digits at scale 2, which name this field's
/// offset and width; [`ErrorKind::OutOfRange`] if `Rate<N>` rejects the decoded value.
fn zoned_rate<const N: u32>(
    r: &mut RecordReader<'_>,
    field: &'static str,
) -> Result<Rate<N>, Error> {
    let start = r.offset();
    let value = r.zoned(field, N, SCALE)?;
    Rate::new(value).map_err(money_error(field, start, consumed(r, start)))
}

/// Appends `value` as a signed DISPLAY `PIC S9(N)V99` rate field (PIC-006): the mirror of
/// [`zoned_rate`].
fn put_zoned_rate<'w, const N: u32>(
    w: &'w mut RecordWriter,
    value: &Rate<N>,
) -> &'w mut RecordWriter {
    w.zoned(value.as_decimal(), N, SCALE)
}

/// The value of a `PIC +9(10).99` image: sign `+` or `-`, ten digits, `.`, two digits.
///
/// The `copybook` editor renders this picture and parses nothing (PIC-022), so the record
/// storage images of `CCPAURQY` and `CCPAURLY` are de-edited here. The CP00 request token never
/// passes through this picture: `crate::messaging` parses the wire amount (CCPAURQY-009, E31).
///
/// # Errors
///
/// [`ErrorKind::WidthMismatch`] when `bytes` is not 14 bytes long; [`ErrorKind::NonDigit`] for a
/// sign byte other than `+` and `-`, a non-digit among the twelve digit positions, or a byte
/// other than `.` at the point; [`ErrorKind::NegativeZero`] for `-` before an all-zero value,
/// which [`EditedPicture::format`] could not reproduce (R7: never normalised to zero).
fn de_edit_amount(bytes: &[u8]) -> Result<Decimal, ErrorKind> {
    let mismatch = ErrorKind::WidthMismatch {
        expected: EDITED_AMOUNT.width(),
        actual: bytes.len(),
    };
    if bytes.len() != EDITED_AMOUNT.width() {
        return Err(mismatch);
    }
    let (&sign, unsigned) = bytes.split_first().ok_or(mismatch)?;
    let negative = match sign {
        b'+' => false,
        b'-' => true,
        _ => return Err(ErrorKind::NonDigit),
    };
    let (int_text, fraction) = unsigned
        .split_at_checked(EDITED_INT_DIGITS)
        .ok_or(mismatch)?;
    let (&point, cents_text) = fraction.split_first().ok_or(mismatch)?;
    let units = PicU::<10>::decode(int_text)?.get();
    if point != b'.' {
        return Err(ErrorKind::NonDigit);
    }
    let cents = PicU::<2>::decode(cents_text)?.get();
    let magnitude = units
        .checked_mul(CENTS_PER_UNIT)
        .and_then(|scaled| scaled.checked_add(cents))
        .ok_or(ErrorKind::OutOfRange)?;
    if negative && magnitude == 0 {
        return Err(ErrorKind::NegativeZero);
    }
    // TRUNCATION: ten integer and two fraction digits are below 10^12, far inside `i64`, so the
    // checked conversion always succeeds; its error arm only keeps this free of a panic path.
    let unsigned_mantissa = i64::try_from(magnitude).map_err(|_| ErrorKind::OutOfRange)?;
    let mantissa = if negative {
        unsigned_mantissa
            .checked_neg()
            .ok_or(ErrorKind::OutOfRange)?
    } else {
        unsigned_mantissa
    };
    Decimal::try_new(mantissa, SCALE).map_err(|_| ErrorKind::OutOfRange)
}

/// Decodes a numeric-edited `PIC +9(10).99` amount, 14 bytes: `PA-RQ-TRANSACTION-AMT` and
/// `PA-RL-APPROVED-AMT` (`CCPAURQY.cpy:27`, `CCPAURLY.cpy:24`; PIC-022).
///
/// # Errors
///
/// Those of [`RecordReader::take`] for 14 bytes; then those of the de-editing rule
/// (`de_edit_amount`) as an error for this field at its offset, 14 bytes wide:
/// [`ErrorKind::NonDigit`] for a bad sign, digit or point byte and [`ErrorKind::NegativeZero`]
/// for `-0000000000.00`; [`ErrorKind::OutOfRange`] if `Money<10>` rejects the value. On an error
/// the caller abandons the record, so the reader's position after it is not used.
fn edited_amount(r: &mut RecordReader<'_>, field: &'static str) -> Result<Money<10>, Error> {
    let start = r.offset();
    let width = EDITED_AMOUNT.width();
    let bytes = r.take(field, width)?;
    let value = de_edit_amount(bytes).map_err(|kind| Error::new(field, start, width, kind))?;
    Money::new(value).map_err(money_error(field, start, width))
}

/// Appends `value` as a `PIC +9(10).99` field, exactly 14 bytes, through
/// [`EditedPicture::format`]: the mirror of [`edited_amount`], so `123.45` is `+0000000123.45`
/// and `-12.34` is `-0000000012.34`.
fn put_edited_amount<'w>(w: &'w mut RecordWriter, value: &Money<10>) -> &'w mut RecordWriter {
    w.bytes(EDITED_AMOUNT.format(value.as_decimal()).as_bytes())
}

/// Checks a Db2 `VARCHAR(MAX)` host variable such as `DCL-TR-DESCRIPTION-TEXT`
/// (`DCLTRTYP.dcl`, `TRNTYPE.ddl:3`): `value` is returned unchanged, untrimmed and unpadded,
/// when it holds at most `MAX` bytes.
///
/// # Errors
///
/// [`ErrorKind::OutOfRange`] for `field` at offset 0 and width `MAX` when `value` is longer than
/// `MAX` bytes. The host variable is checked on its own, outside any record, so its offset is 0;
/// the error carries no part of the text.
fn varchar<const MAX: usize>(field: &'static str, value: String) -> Result<String, Error> {
    if value.len() <= MAX {
        Ok(value)
    } else {
        Err(Error::new(field, 0, MAX, ErrorKind::OutOfRange))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, RecordReader, RecordWriter};

    use super::{
        de_edit_amount, edited_amount, money_error, packed_money, put_edited_amount,
        put_packed_money, put_zoned_money, put_zoned_rate, varchar, zoned_money, zoned_rate,
    };
    use crate::money::{Decimal, Money, MoneyError, Rate};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Field name every helper test reads under.
    const FIELD: &str = "TEST-AMT";

    /// A two-byte `PIC X(02)` field read before the amount, so errors show a non-zero offset.
    const PREFIX: &[u8] = b"AB";

    /// `PREFIX` followed by `field`, the bytes of a two-field record.
    fn prefixed(field: &[u8]) -> Vec<u8> {
        let mut record = PREFIX.to_vec();
        record.extend_from_slice(field);
        record
    }

    /// A reader over `record` positioned after its two-byte prefix.
    fn after_prefix(record: &[u8]) -> Result<RecordReader<'_>, Error> {
        let mut r = RecordReader::new(record, record.len())?;
        r.pic_x::<2>("TEST-CODE")?;
        Ok(r)
    }

    /// `bytes` decodes through `zoned_money::<N>` to `cents` hundredths and re-encodes to itself.
    fn zoned_money_round_trip<const N: u32>(bytes: &[u8], cents: i64) -> TestResult {
        let mut r = RecordReader::new(bytes, bytes.len())?;
        let value = zoned_money::<N>(&mut r, FIELD)?;
        r.finish()?;
        assert_eq!(value, Money::<N>::new(Decimal::new(cents, 2))?);
        let mut w = RecordWriter::with_capacity(bytes.len());
        put_zoned_money(&mut w, &value);
        assert_eq!(w.finish(), bytes);
        Ok(())
    }

    /// `bytes` decodes through `packed_money::<N>` to `cents` hundredths and re-encodes to itself.
    fn packed_money_round_trip<const N: u32>(bytes: &[u8], cents: i64) -> TestResult {
        let mut r = RecordReader::new(bytes, bytes.len())?;
        let value = packed_money::<N>(&mut r, FIELD)?;
        r.finish()?;
        assert_eq!(value, Money::<N>::new(Decimal::new(cents, 2))?);
        let mut w = RecordWriter::with_capacity(bytes.len());
        put_packed_money(&mut w, &value);
        assert_eq!(w.finish(), bytes);
        Ok(())
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn zoned_money_round_trips_positive_and_negative_overpunch() -> TestResult {
        // `ACCT-CURR-BAL PIC S9(10)V99` of `acctdata.txt` line 1: `{` is a positive last digit 0.
        zoned_money_round_trip::<10>(b"00000001940{", 19_400)?;
        // `TRAN-CAT-BAL PIC S9(09)V99`: `M` is a negative last digit 4.
        zoned_money_round_trip::<9>(b"0000001234M", -12_344)?;
        // A one-cent value keeps its leading zeros.
        zoned_money_round_trip::<9>(b"0000000000A", 1)
    }

    // audit: PIC-010
    #[test]
    fn packed_money_round_trips_both_signs_and_pad_nibble() -> TestResult {
        // `PA-CREDIT-LIMIT PIC S9(09)V99 COMP-3`: 11 digits, 6 bytes, sign nibble `D`.
        packed_money_round_trip::<9>(&[0x00, 0x00, 0x00, 0x12, 0x34, 0x5D], -12_345)?;
        // `PA-TRANSACTION-AMT PIC S9(10)V99 COMP-3`: 12 digits, a pad nibble, 7 bytes, sign `C`.
        packed_money_round_trip::<10>(&[0x00, 0x00, 0x00, 0x00, 0x12, 0x34, 0x5C], 12_345)?;
        packed_money_round_trip::<9>(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x0C], 0)
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn zoned_rate_round_trips_dis_int_rate() -> TestResult {
        // `DIS-INT-RATE PIC S9(04)V99`: 15.00 (`{` is +0) and -2.51 (`J` is -1).
        for (bytes, hundredths) in [(b"00150{", 1_500_i64), (b"00025J", -251)] {
            let mut r = RecordReader::new(bytes, bytes.len())?;
            let rate = zoned_rate::<4>(&mut r, FIELD)?;
            r.finish()?;
            assert_eq!(rate, Rate::<4>::new(Decimal::new(hundredths, 2))?);
            let mut w = RecordWriter::with_capacity(bytes.len());
            put_zoned_rate(&mut w, &rate);
            assert_eq!(w.finish(), bytes);
        }
        Ok(())
    }

    // audit: PIC-022
    #[test]
    fn edited_amount_round_trips_every_sign_and_extreme() -> TestResult {
        let cases: [(&[u8; 14], i64); 5] = [
            (b"+0000000123.45", 12_345),
            (b"-0000000012.34", -1_234),
            (b"+0000000000.00", 0),
            (b"+9999999999.99", 999_999_999_999),
            (b"-9999999999.99", -999_999_999_999),
        ];
        for (image, cents) in cases {
            let record = prefixed(image);
            let mut r = after_prefix(&record)?;
            let amount = edited_amount(&mut r, FIELD)?;
            r.finish()?;
            assert_eq!(amount, Money::<10>::new(Decimal::new(cents, 2))?);
            let mut w = RecordWriter::with_capacity(record.len());
            w.bytes(PREFIX);
            put_edited_amount(&mut w, &amount);
            assert_eq!(w.finish(), record);
        }
        Ok(())
    }

    // audit: PIC-022
    #[test]
    fn edited_amount_rejects_malformed_images_at_the_field() -> TestResult {
        let cases: [(&[u8; 14], ErrorKind); 7] = [
            (b"-0000000000.00", ErrorKind::NegativeZero),
            (b"*0000000123.45", ErrorKind::NonDigit),
            (b" 0000000123.45", ErrorKind::NonDigit),
            (b"+0000000123,45", ErrorKind::NonDigit),
            (b"+00000001 3.45", ErrorKind::NonDigit),
            (b"+0000000123.4x", ErrorKind::NonDigit),
            (b"+-000000123.45", ErrorKind::NonDigit),
        ];
        for (image, kind) in cases {
            let record = prefixed(image);
            let mut r = after_prefix(&record)?;
            assert_eq!(
                edited_amount(&mut r, FIELD).err(),
                Some(Error::new(FIELD, PREFIX.len(), 14, kind)),
            );
        }
        Ok(())
    }

    #[test]
    fn de_editing_rejects_any_other_length() {
        for bytes in [b"".as_slice(), b"+", b"+0000000123.4", b"+0000000123.456"] {
            assert_eq!(
                de_edit_amount(bytes),
                Err(ErrorKind::WidthMismatch {
                    expected: 14,
                    actual: bytes.len(),
                }),
            );
        }
    }

    // audit: PIC-007
    // audit: PIC-008
    // audit: PIC-010
    #[test]
    fn amount_decode_errors_name_the_field_offset_and_width() -> TestResult {
        let plain_digit_sign = prefixed(b"000000019400");
        let mut r = after_prefix(&plain_digit_sign)?;
        assert_eq!(
            zoned_money::<10>(&mut r, FIELD).err(),
            Some(Error::new(FIELD, 2, 12, ErrorKind::PlainDigitSign)),
        );

        let negative_zero = prefixed(b"00000}");
        let mut r = after_prefix(&negative_zero)?;
        assert_eq!(
            zoned_rate::<4>(&mut r, FIELD).err(),
            Some(Error::new(FIELD, 2, 6, ErrorKind::NegativeZero)),
        );

        let unsigned_packed = prefixed(&[0x00, 0x00, 0x00, 0x12, 0x34, 0x5F]);
        let mut r = after_prefix(&unsigned_packed)?;
        assert_eq!(
            packed_money::<9>(&mut r, FIELD).err(),
            Some(Error::new(FIELD, 2, 6, ErrorKind::InvalidPackedSign)),
        );
        Ok(())
    }

    #[test]
    fn short_slices_are_width_errors_never_panics() -> TestResult {
        let short = prefixed(b"0001");
        let mismatch = |expected| ErrorKind::WidthMismatch {
            expected,
            actual: 4,
        };

        let mut r = after_prefix(&short)?;
        assert_eq!(
            zoned_money::<10>(&mut r, FIELD).err(),
            Some(Error::new(FIELD, 2, 12, mismatch(12))),
        );
        let mut r = after_prefix(&short)?;
        assert_eq!(
            packed_money::<10>(&mut r, FIELD).err(),
            Some(Error::new(FIELD, 2, 7, mismatch(7))),
        );
        let mut r = after_prefix(&short)?;
        assert_eq!(
            zoned_rate::<4>(&mut r, FIELD).err(),
            Some(Error::new(FIELD, 2, 6, mismatch(6))),
        );
        let mut r = after_prefix(&short)?;
        assert_eq!(
            edited_amount(&mut r, FIELD).err(),
            Some(Error::new(FIELD, 2, 14, mismatch(14))),
        );
        Ok(())
    }

    #[test]
    fn every_money_error_is_out_of_range_for_the_field() {
        for error in [MoneyError::ScaleTooLarge, MoneyError::OutOfRange] {
            assert_eq!(
                money_error(FIELD, 7, 12)(error),
                Error::new(FIELD, 7, 12, ErrorKind::OutOfRange),
            );
        }
    }

    #[test]
    fn varchar_accepts_up_to_its_maximum_byte_length_unchanged() {
        const DESC: &str = "DCL-TR-DESCRIPTION-TEXT";
        for text in [String::new(), "PURCHASE".to_owned(), "X".repeat(50)] {
            assert_eq!(varchar::<50>(DESC, text.clone()), Ok(text));
        }
        assert_eq!(
            varchar::<50>(DESC, "X".repeat(51)),
            Err(Error::new(DESC, 0, 50, ErrorKind::OutOfRange)),
        );
        // The limit counts bytes: 26 two-byte characters are 52 bytes.
        assert_eq!(
            varchar::<50>(DESC, "é".repeat(26)),
            Err(Error::new(DESC, 0, 50, ErrorKind::OutOfRange)),
        );
    }
}
