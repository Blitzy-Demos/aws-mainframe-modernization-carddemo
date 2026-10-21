//! Codec A primitives: a round trip for every picture kind and a typed error for every anomaly
//! class (bindings i8 and R7, AAP 0.9.1; rows `PIC-001` to `PIC-024` of
//! `modern/audit/copybooks.md` and the `FIXFRM` rows of `modern/audit/fixtures.md`).
//!
//! Every property checks both directions: a generated value encodes to bytes that decode back to
//! it, and bytes a decoder accepts re-encode unchanged. The fixture tests run every line of the
//! nine files of `app/data/ASCII/`, read in place, through `framing::lines` and through the field
//! codecs at the copybook offsets. The error tests assert the exact `ErrorKind` of each anomaly,
//! which is never normalised, and that no rendering of an error repeats its input.
//!
//! Fixture lines hold card numbers and SSNs, so no assertion here prints fixture bytes: a failure
//! names the file, the line and the field only (Cardholder Data Redaction).

use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use copybook::edited::EditedPicture;
use copybook::framing::{Lines, decode_all, encode_all};
use copybook::zoned::{self, binary, packed};
use copybook::{
    CobolTimestamp, Count, Error, ErrorKind, FixedRecord, Framed, Line, LineFraming, PicDate, PicS,
    PicTimestamp, PicU, PicX, RecordReader, RecordWriter, Terminator, TsForm, lines,
};
use proptest::prelude::*;
use rust_decimal::Decimal;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// The configuration of every property. Failure persistence is off: a `proptest-regressions/`
/// file would be a path outside the plan.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// The bytes of fixture `name` in `app/data/ASCII/`, read in place as raw bytes, so every CR
/// survives.
fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../app/data/ASCII")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|err| panic!("{name}: {err}"))
}

/// Calls `check` with each line number, counted from 1, and line of fixture `name`.
fn for_each_line(name: &str, mut check: impl FnMut(usize, &Line<'_>)) {
    let bytes = fixture(name);
    for (index, line) in lines(&bytes).enumerate() {
        check(index + 1, &line);
    }
}

/// The bytes `write` appends to an empty buffer.
fn encoded(write: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::new();
    write(&mut out);
    out
}

/// Bytes a field of `n` digits occupies in DISPLAY form.
fn width(n: u32) -> usize {
    usize::try_from(n).expect("a digit count fits usize")
}

/// The result of a test that uses `?` on codec errors.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A primitive failure as a proptest failure.
fn fail(kind: ErrorKind) -> TestCaseError {
    TestCaseError::fail(kind.to_string())
}

/// `len` bytes of `body` from `offset`; `at` names the file, line and field for a failure.
fn field<'a>(body: &'a [u8], offset: usize, len: usize, at: &str) -> &'a [u8] {
    body.get(offset..offset + len)
        .unwrap_or_else(|| panic!("{at}: line body too short"))
}

/// Asserts that neither rendering of `err` contains `input` (Cardholder Data Redaction).
fn assert_no_echo(err: &(impl fmt::Display + fmt::Debug), input: &[u8]) {
    let text = String::from_utf8_lossy(input);
    for rendered in [err.to_string(), format!("{err:?}")] {
        assert!(
            !rendered.contains(text.as_ref()),
            "an error rendering repeats its {}-byte input",
            input.len()
        );
    }
}

/// Asserts that `result` failed with exactly `expected`, and that the error does not repeat
/// `input`.
fn assert_kind<T>(result: Result<T, ErrorKind>, expected: ErrorKind, input: &[u8]) {
    let Some(kind) = result.err() else {
        panic!(
            "a {}-byte input decoded; expected {expected:?}",
            input.len()
        );
    };
    assert_eq!(kind, expected, "{}-byte input", input.len());
    assert_no_echo(&kind, input);
}

/// Asserts that `result` failed with an [`Error`] of exactly `kind` for `field_name` at
/// `offset`, and that the error does not repeat `input`.
fn assert_error<T>(
    result: Result<T, Error>,
    (field_name, offset, kind): (&str, usize, ErrorKind),
    input: &[u8],
) {
    let Some(err) = result.err() else {
        panic!("a {}-byte input decoded; expected {kind:?}", input.len());
    };
    assert_eq!(
        (err.field, err.offset, err.kind),
        (field_name, offset, kind),
        "{err}"
    );
    assert_no_echo(&err, input);
}

/// The trailing overpunch bytes: `{` and `A`–`I` positive, `}` and `J`–`R` negative (PIC-005).
const OVERPUNCH: &[u8; 20] = b"{ABCDEFGHI}JKLMNOPQR";

/// Printable ASCII, the bytes `PicX` accepts (PIC-001).
fn printable(len: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(0x20_u8..=0x7E, len)
}

/// A byte `PicX` rejects: a control byte, DEL or a byte above `0x7F` (PIC-002).
fn non_printable() -> impl Strategy<Value = u8> {
    prop_oneof![0_u8..0x20, 0x7F_u8..=0xFF]
}

/// Bytes of a signed DISPLAY field `len` bytes wide: ASCII digits, then an overpunch byte.
fn zoned_bytes(len: usize) -> impl Strategy<Value = Vec<u8>> {
    (
        prop::collection::vec(b'0'..=b'9', len - 1),
        prop::sample::select(OVERPUNCH.to_vec()),
    )
        .prop_map(|(mut digits, sign)| {
            digits.push(sign);
            digits
        })
}

/// Whether `bytes`, a signed DISPLAY field, is a negative zero: all digits `0`, sign `}`.
fn is_display_negative_zero(bytes: &[u8]) -> bool {
    matches!(bytes.split_last(), Some((b'}', digits)) if digits.iter().all(|&byte| byte == b'0'))
}

/// A mantissa whose magnitude has at most `digits` digits.
fn mantissa(digits: u32) -> impl Strategy<Value = i64> {
    let max = 10_i64.pow(digits) - 1;
    -max..=max
}

/// The bytes of a `COMP-3` field of `digits` digits from digit nibbles and a sign nibble, with
/// the zero pad nibble an even digit count needs (PIC-010).
fn packed_from_nibbles(digit_nibbles: &[u8], sign: u8) -> Vec<u8> {
    let pad = (digit_nibbles.len() % 2 == 0).then_some(0_u8);
    let nibbles: Vec<u8> = pad
        .into_iter()
        .chain(digit_nibbles.iter().copied())
        .chain(std::iter::once(sign))
        .collect();
    nibbles
        .chunks(2)
        .map(|pair| match pair {
            [high, low] => (high << 4) | low,
            _ => panic!("the pad nibble makes the nibble count even"),
        })
        .collect()
}

/// Bytes of a `COMP-3` field of `digits` digits: digit nibbles 0–9, sign `C` or `D`.
fn packed_bytes(digits: u32) -> impl Strategy<Value = Vec<u8>> {
    (
        prop::collection::vec(0_u8..=9, width(digits)),
        prop::sample::select(vec![0x0C_u8, 0x0D]),
    )
        .prop_map(|(nibbles, sign)| packed_from_nibbles(&nibbles, sign))
}

/// A calendar date with a four-digit year, `0000-01-01` to `9999-12-31`.
fn any_date() -> impl Strategy<Value = NaiveDate> {
    (0_i32..=9999, 1_u32..=366).prop_filter_map("day 366 of a common year", |(year, ordinal)| {
        NaiveDate::from_yo_opt(year, ordinal)
    })
}

/// A time of day with microsecond precision and no leap second.
fn any_time() -> impl Strategy<Value = NaiveTime> {
    (0_u32..24, 0_u32..60, 0_u32..60, 0_u32..1_000_000).prop_map(|(hour, minute, second, micro)| {
        NaiveTime::from_hms_micro_opt(hour, minute, second, micro).expect("a valid time of day")
    })
}

/// `YYYY-MM-DD` with zero-padded parts, written independently of the codec.
fn date_text(date: NaiveDate) -> String {
    format!("{:04}-{:02}-{:02}", date.year(), date.month(), date.day())
}

/// The 26-byte text of `value` in layout `form`, written independently of the codec
/// (PIC-018 to PIC-020).
fn timestamp_text(value: NaiveDateTime, form: TsForm) -> String {
    let date = date_text(value.date());
    let micro = value.nanosecond() / 1_000;
    let (hour, minute, second) = (value.hour(), value.minute(), value.second());
    match form {
        TsForm::Iso => format!("{date} {hour:02}:{minute:02}:{second:02}.{micro:06}"),
        TsForm::Db2 => format!("{date}-{hour:02}.{minute:02}.{second:02}.{micro:06}"),
        TsForm::ScreenDate => format!("{date}{:16}", ""),
    }
}

// ---------------------------------------------------------------------------------------------
// Round trips, one family per picture kind (i8)
// ---------------------------------------------------------------------------------------------

/// `PicX<N>` over `bytes`, `N` printable bytes: both directions keep every byte (PIC-001).
fn pic_x_round_trip<const N: usize>(bytes: &[u8]) -> Result<(), TestCaseError> {
    let array = <[u8; N]>::try_from(bytes).map_err(|_| TestCaseError::fail("strategy width"))?;
    let value = PicX::from_array(array);
    let out = encoded(|out| value.encode_into(out));
    prop_assert_eq!(out.as_slice(), bytes);
    prop_assert_eq!(PicX::<N>::decode(&out), Ok(value));
    let decoded = PicX::<N>::decode(bytes).map_err(fail)?;
    prop_assert_eq!(decoded.as_bytes().as_slice(), bytes);
    prop_assert_eq!(encoded(|out| decoded.encode_into(out)), bytes);
    Ok(())
}

/// `PicU<N>` over `value`: exactly `N` ASCII digits, leading zeros restored, and back (PIC-003).
fn pic_u_round_trip<const N: u32>(value: u64) -> Result<(), TestCaseError> {
    let key = PicU::<N>::new(value).map_err(fail)?;
    let out = encoded(|out| key.encode_into(out));
    prop_assert_eq!(
        &out,
        &format!("{value:0digits$}", digits = width(N)).into_bytes()
    );
    prop_assert_eq!(PicU::<N>::decode(&out), Ok(key));
    Ok(())
}

/// `PicU<N>` over `bytes`, `N` ASCII digits: the decoded key re-encodes the same digits.
fn pic_u_bytes_round_trip<const N: u32>(bytes: &[u8]) -> Result<(), TestCaseError> {
    let key = PicU::<N>::decode(bytes).map_err(fail)?;
    prop_assert_eq!(encoded(|out| key.encode_into(out)), bytes);
    Ok(())
}

/// Signed integral DISPLAY `PIC S9(N)` over `value`, negatives included (PIC-005, PIC-006).
fn pic_s_display_round_trip<const N: u32>(value: i64) -> Result<(), TestCaseError> {
    let signed = PicS::<N>::new(value).map_err(fail)?;
    let out = encoded(|out| zoned::encode_pic_s(signed, out));
    prop_assert_eq!(out.len(), width(N));
    prop_assert_eq!(zoned::decode_pic_s::<N>(&out), Ok(signed));
    Ok(())
}

/// Signed integral DISPLAY `PIC S9(N)` over `bytes`: accepted bytes re-encode unchanged, and the
/// only rejection of well-formed bytes is a negative zero (PIC-008).
fn pic_s_display_bytes_round_trip<const N: u32>(bytes: &[u8]) -> Result<(), TestCaseError> {
    match zoned::decode_pic_s::<N>(bytes) {
        Ok(signed) => {
            prop_assert_eq!(encoded(|out| zoned::encode_pic_s(signed, out)), bytes);
        }
        Err(kind) => {
            prop_assert_eq!(kind, ErrorKind::NegativeZero);
            prop_assert!(is_display_negative_zero(bytes));
        }
    }
    Ok(())
}

/// Signed DISPLAY `PIC S9(int_digits)V9(scale)` over `value` (PIC-005, PIC-006).
fn zoned_round_trip(value: Decimal, int_digits: u32, scale: u32) -> Result<(), TestCaseError> {
    let out = encoded(|out| zoned::encode_signed(value, int_digits, scale, out));
    prop_assert_eq!(out.len(), width(int_digits + scale));
    prop_assert!(out.last().is_some_and(|sign| OVERPUNCH.contains(sign)));
    prop_assert!(out.iter().rev().skip(1).all(u8::is_ascii_digit));
    let decoded = zoned::decode_signed(&out, int_digits, scale).map_err(fail)?;
    prop_assert_eq!(decoded, value);
    prop_assert_eq!(decoded.scale(), scale);
    Ok(())
}

/// Signed DISPLAY over `bytes`: accepted bytes re-encode unchanged; a negative zero is refused.
fn zoned_bytes_round_trip(bytes: &[u8], int_digits: u32, scale: u32) -> Result<(), TestCaseError> {
    match zoned::decode_signed(bytes, int_digits, scale) {
        Ok(value) => {
            prop_assert_eq!(value.scale(), scale);
            let out = encoded(|out| zoned::encode_signed(value, int_digits, scale, out));
            prop_assert_eq!(out.as_slice(), bytes);
        }
        Err(kind) => {
            prop_assert_eq!(kind, ErrorKind::NegativeZero);
            prop_assert!(is_display_negative_zero(bytes));
        }
    }
    Ok(())
}

/// `COMP-3` of `digits` digits, `scale` after the point, over `value` (PIC-010).
fn packed_round_trip(value: Decimal, digits: u32, scale: u32) -> Result<(), TestCaseError> {
    let out = encoded(|out| packed::encode(value, digits, scale, out));
    prop_assert_eq!(Ok(out.len()), packed::byte_width(digits));
    let decoded = packed::decode(&out, digits, scale).map_err(fail)?;
    prop_assert_eq!(decoded, value);
    prop_assert_eq!(decoded.scale(), scale);
    Ok(())
}

/// `COMP-3` over `bytes`: accepted bytes re-encode unchanged; the only rejection of well-formed
/// nibbles is a negative zero (PIC-010).
fn packed_bytes_round_trip(bytes: &[u8], digits: u32, scale: u32) -> Result<(), TestCaseError> {
    match packed::decode(bytes, digits, scale) {
        Ok(value) => {
            let out = encoded(|out| packed::encode(value, digits, scale, out));
            prop_assert_eq!(out.as_slice(), bytes);
        }
        Err(kind) => {
            prop_assert_eq!(kind, ErrorKind::NegativeZero);
            let (last, leading) = bytes.split_last().ok_or_else(|| fail(kind))?;
            prop_assert_eq!(last & 0x0F, 0x0D);
            prop_assert!(last >> 4 == 0 && leading.iter().all(|&byte| byte == 0));
        }
    }
    Ok(())
}

/// Signed integral `COMP-3` `PIC S9(N)` over `value`, through the `PicS` codecs (PIC-010).
fn packed_pic_s_round_trip<const N: u32>(value: i64) -> Result<(), TestCaseError> {
    let signed = PicS::<N>::new(value).map_err(fail)?;
    let out = encoded(|out| packed::encode_pic_s(signed, out));
    prop_assert_eq!(Ok(out.len()), packed::byte_width(N));
    prop_assert_eq!(packed::decode_pic_s::<N>(&out), Ok(signed));
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    // audit: PIC-001
    #[test]
    fn pic_x_keeps_printable_bytes(
        one in printable(1),
        ten in printable(10),
        sixteen in printable(16),
        twenty_six in printable(26),
        fifty in printable(50),
    ) {
        pic_x_round_trip::<1>(&one)?;
        pic_x_round_trip::<10>(&ten)?;
        pic_x_round_trip::<16>(&sixteen)?;
        pic_x_round_trip::<26>(&twenty_six)?;
        pic_x_round_trip::<50>(&fifty)?;
    }

    // audit: PIC-002
    #[test]
    fn pic_x_rejects_any_non_printable_byte(
        mut bytes in printable(26),
        position in 0_usize..26,
        byte in non_printable(),
    ) {
        bytes[position] = byte;
        prop_assert_eq!(PicX::<26>::decode(&bytes), Err(ErrorKind::NonPrintable));
    }

    // audit: PIC-003
    #[test]
    fn pic_u_values_restore_leading_zeros(
        three in 0..10_u64.pow(3),
        four in 0..10_u64.pow(4),
        nine in 0..10_u64.pow(9),
        eleven in 0..10_u64.pow(11),
        sixteen in 0..10_u64.pow(16),
        eighteen in 0..10_u64.pow(18),
    ) {
        pic_u_round_trip::<3>(three)?;
        pic_u_round_trip::<4>(four)?;
        pic_u_round_trip::<9>(nine)?;
        pic_u_round_trip::<11>(eleven)?;
        pic_u_round_trip::<16>(sixteen)?;
        pic_u_round_trip::<18>(eighteen)?;
    }

    // audit: PIC-003
    #[test]
    fn pic_u_digits_re_encode_unchanged(
        three in prop::collection::vec(b'0'..=b'9', 3),
        eleven in prop::collection::vec(b'0'..=b'9', 11),
        eighteen in prop::collection::vec(b'0'..=b'9', 18),
    ) {
        pic_u_bytes_round_trip::<3>(&three)?;
        pic_u_bytes_round_trip::<11>(&eleven)?;
        pic_u_bytes_round_trip::<18>(&eighteen)?;
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn pic_s_display_round_trips(
        five in mantissa(5),
        nine in mantissa(9),
        eleven in mantissa(11),
        five_bytes in zoned_bytes(5),
        eleven_bytes in zoned_bytes(11),
    ) {
        pic_s_display_round_trip::<5>(five)?;
        pic_s_display_round_trip::<9>(nine)?;
        pic_s_display_round_trip::<11>(eleven)?;
        pic_s_display_bytes_round_trip::<5>(&five_bytes)?;
        pic_s_display_bytes_round_trip::<11>(&eleven_bytes)?;
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn zoned_decimals_round_trip(
        balance in mantissa(12),
        amount in mantissa(11),
        rate in mantissa(6),
        balance_bytes in zoned_bytes(12),
        amount_bytes in zoned_bytes(11),
        rate_bytes in zoned_bytes(6),
    ) {
        // ACCT-CURR-BAL S9(10)V99, TRAN-CAT-BAL S9(09)V99, DIS-INT-RATE S9(04)V99.
        zoned_round_trip(Decimal::new(balance, 2), 10, 2)?;
        zoned_round_trip(Decimal::new(amount, 2), 9, 2)?;
        zoned_round_trip(Decimal::new(rate, 2), 4, 2)?;
        zoned_bytes_round_trip(&balance_bytes, 10, 2)?;
        zoned_bytes_round_trip(&amount_bytes, 9, 2)?;
        zoned_bytes_round_trip(&rate_bytes, 4, 2)?;
    }

    // audit: PIC-010
    #[test]
    fn packed_values_round_trip(
        acct_id in mantissa(11),
        limit in mantissa(11),
        amount in mantissa(12),
        date_key in mantissa(5),
        time_key in mantissa(9),
    ) {
        // PA-ACCT-ID S9(11), PA-CREDIT-LIMIT S9(09)V99, PA-TRANSACTION-AMT S9(10)V99,
        // PA-AUTH-DATE-9C S9(05) and PA-AUTH-TIME-9C S9(09), all COMP-3.
        packed_round_trip(Decimal::new(acct_id, 0), 11, 0)?;
        packed_round_trip(Decimal::new(limit, 2), 11, 2)?;
        packed_round_trip(Decimal::new(amount, 2), 12, 2)?;
        packed_round_trip(Decimal::new(date_key, 0), 5, 0)?;
        packed_round_trip(Decimal::new(time_key, 0), 9, 0)?;
        packed_pic_s_round_trip::<11>(acct_id)?;
        packed_pic_s_round_trip::<5>(date_key)?;
        packed_pic_s_round_trip::<9>(time_key)?;
    }

    // audit: PIC-010
    #[test]
    fn packed_bytes_re_encode_unchanged(
        acct_id in packed_bytes(11),
        amount in packed_bytes(12),
        date_key in packed_bytes(5),
        time_key in packed_bytes(9),
    ) {
        packed_bytes_round_trip(&acct_id, 11, 0)?;
        packed_bytes_round_trip(&acct_id, 11, 2)?;
        packed_bytes_round_trip(&amount, 12, 2)?;
        packed_bytes_round_trip(&date_key, 5, 0)?;
        packed_bytes_round_trip(&time_key, 9, 0)?;
    }

    // audit: PIC-011
    #[test]
    fn counts_round_trip_every_halfword(value in any::<i16>(), bytes in any::<[u8; 2]>()) {
        let count = Count::new(value);
        let out = encoded(|out| binary::encode_count(count, out));
        prop_assert_eq!(out.as_slice(), &value.to_be_bytes());
        prop_assert_eq!(binary::decode_count(&out), Ok(count));
        let decoded = binary::decode_count(&bytes).map_err(fail)?;
        prop_assert_eq!(i16::from(decoded), i16::from_be_bytes(bytes));
        prop_assert_eq!(encoded(|out| binary::encode_count(decoded, out)), bytes.to_vec());
    }
}

/// A timestamp value its layout can carry: any microsecond time for `I` and `D`, midnight for
/// `S`, which writes the date only (PIC-020).
fn timestamp_in(date: NaiveDate, time: NaiveTime, form: TsForm) -> NaiveDateTime {
    match form {
        TsForm::Iso | TsForm::Db2 => date.and_time(time),
        TsForm::ScreenDate => date.and_time(NaiveTime::MIN),
    }
}

/// Every timestamp layout.
const FORMS: [TsForm; 3] = [TsForm::Iso, TsForm::Db2, TsForm::ScreenDate];

proptest! {
    #![proptest_config(config())]

    // audit: PIC-015
    #[test]
    fn dates_round_trip(date in any_date()) {
        let text = date_text(date);
        let value = PicDate::new(Some(date)).map_err(fail)?;
        prop_assert_eq!(value.value(), Some(date));
        let out = encoded(|out| value.encode_into(out));
        prop_assert_eq!(out.as_slice(), text.as_bytes());
        let decoded = PicDate::decode(text.as_bytes()).map_err(fail)?;
        prop_assert_eq!(decoded, value);
        prop_assert_eq!(encoded(|out| decoded.encode_into(out)), out);
    }

    // audit: PIC-018
    // audit: PIC-019
    // audit: PIC-020
    #[test]
    fn timestamps_round_trip_in_every_layout(date in any_date(), time in any_time()) {
        for form in FORMS {
            let value = timestamp_in(date, time, form);
            let text = timestamp_text(value, form);
            prop_assert_eq!(text.len(), PicTimestamp::WIDTH);
            let stamp = PicTimestamp::new(Some(CobolTimestamp::new(value, form).map_err(fail)?));
            let out = encoded(|out| stamp.encode_into(out));
            prop_assert_eq!(out.as_slice(), text.as_bytes());
            let decoded = PicTimestamp::decode(text.as_bytes()).map_err(fail)?;
            prop_assert_eq!(decoded, stamp);
            let typed = decoded.value().ok_or_else(|| TestCaseError::fail("decoded blank"))?;
            prop_assert_eq!((typed.value(), typed.form()), (value, form));
            prop_assert_eq!(encoded(|out| decoded.encode_into(out)), out);
        }
    }

    #[test]
    fn inverted_keys_round_trip(yyddd in 0_u64..=99_999, hhmmssmmm in 0_u64..=999_999_999) {
        // PA-AUTH-DATE-9C = 99999 - YYDDD, PA-AUTH-TIME-9C = 999999999 - HHMMSSmmm
        // (COPAUA0C.cbl:874-875).
        let date_key = PicS::<5>::complement_from(yyddd).map_err(fail)?;
        prop_assert_eq!(u64::try_from(date_key.get()).ok(), Some(99_999 - yyddd));
        prop_assert_eq!(date_key.uncomplement(), Ok(yyddd));
        let time_key = PicS::<9>::complement_from(hhmmssmmm).map_err(fail)?;
        prop_assert_eq!(u64::try_from(time_key.get()).ok(), Some(999_999_999 - hhmmssmmm));
        prop_assert_eq!(time_key.uncomplement(), Ok(hhmmssmmm));
    }

    // audit: PIC-023
    #[test]
    fn account_keys_convert_between_signed_and_unsigned(acct_id in 0..10_u64.pow(11)) {
        // PA-ACCT-ID PIC S9(11) COMP-3 joins ACCT-ID PIC 9(11).
        let unsigned = PicU::<11>::new(acct_id).map_err(fail)?;
        let signed = PicS::<11>::from(unsigned);
        prop_assert_eq!(u64::try_from(signed.get()).ok(), Some(acct_id));
        prop_assert_eq!(PicU::<11>::try_from(signed), Ok(unsigned));
        prop_assert_eq!(u64::from(unsigned), acct_id);
    }
}

// audit: PIC-017
#[test]
fn blank_dates_and_timestamps_are_none() {
    let ten = [b' '; 10];
    assert_eq!(PicDate::decode(&ten), Ok(PicDate::BLANK));
    assert_eq!(PicDate::BLANK.value(), None);
    assert_eq!(PicDate::new(None), Ok(PicDate::BLANK));
    assert_eq!(encoded(|out| PicDate::BLANK.encode_into(out)), ten);
    assert_eq!(PicDate::default(), PicDate::BLANK);

    let twenty_six = [b' '; 26];
    assert_eq!(PicTimestamp::decode(&twenty_six), Ok(PicTimestamp::BLANK));
    assert_eq!(PicTimestamp::BLANK.value(), None);
    assert_eq!(PicTimestamp::new(None), PicTimestamp::BLANK);
    assert_eq!(
        encoded(|out| PicTimestamp::BLANK.encode_into(out)),
        twenty_six
    );
    assert_eq!(PicTimestamp::default(), PicTimestamp::BLANK);
    assert_eq!((PicDate::WIDTH, PicTimestamp::WIDTH), (10, 26));
}

#[test]
fn form_codes_round_trip() {
    for (form, code) in FORMS.into_iter().zip(['I', 'D', 'S']) {
        assert_eq!(form.code(), code);
        assert_eq!(TsForm::from_code(code), Ok(form));
    }
    for code in ['X', 'i', 'd', 's', ' ', '0'] {
        assert_eq!(TsForm::from_code(code), Err(ErrorKind::UnknownFormCode));
    }
}

// ---------------------------------------------------------------------------------------------
// A record of every field kind, through every reader and writer method
// ---------------------------------------------------------------------------------------------

/// A test-local record with one field of each kind the cursors read, 78 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AllKinds {
    /// Raw bytes, `take`n unchanged.
    raw: Vec<u8>,
    /// `PIC X(04)`.
    text: PicX<4>,
    /// `PIC 9(05)`.
    key: PicU<5>,
    /// `PIC S9(05)` DISPLAY.
    signed: PicS<5>,
    /// `PIC S9(09) COMP-3`.
    packed_key: PicS<9>,
    /// `PIC S9(09)V99` DISPLAY.
    amount: Decimal,
    /// `PIC S9(10)V99 COMP-3`.
    packed_amount: Decimal,
    /// `PIC S9(4) COMP`.
    count: Count,
    /// `PIC X(10)` date.
    date: PicDate,
    /// `PIC X(26)` timestamp.
    stamp: PicTimestamp,
}

/// Byte width of each `AllKinds` field, in field order.
const ALL_KINDS_WIDTHS: [usize; 10] = [3, 4, 5, 5, 5, 11, 7, 2, 10, 26];

impl FixedRecord for AllKinds {
    const WIDTH: usize = 78;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            raw: r.take("ALL-RAW", 3)?.to_vec(),
            text: r.pic_x("ALL-TEXT")?,
            key: r.pic_u("ALL-KEY")?,
            signed: r.pic_s_zoned("ALL-SIGNED")?,
            packed_key: r.pic_s_packed("ALL-PACKED-KEY")?,
            amount: r.zoned("ALL-AMOUNT", 9, 2)?,
            packed_amount: r.packed("ALL-PACKED-AMOUNT", 12, 2)?,
            count: r.count("ALL-COUNT")?,
            date: r.date("ALL-DATE")?,
            stamp: r.timestamp("ALL-STAMP")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.bytes(&self.raw)
            .pic_x(&self.text)
            .pic_u(self.key)
            .pic_s_zoned(self.signed)
            .pic_s_packed(self.packed_key)
            .zoned(self.amount, 9, 2)
            .packed(self.packed_amount, 12, 2)
            .count(self.count)
            .date(self.date)
            .timestamp(&self.stamp);
        w.finish()
    }
}

/// Any `AllKinds` value.
fn all_kinds() -> impl Strategy<Value = AllKinds> {
    let pictures = (
        prop::collection::vec(any::<u8>(), 3),
        printable(4),
        0..10_u64.pow(5),
        mantissa(5),
        mantissa(9),
    );
    let values = (
        mantissa(11),
        mantissa(12),
        any::<i16>(),
        prop::option::of(any_date()),
        (any_date(), any_time(), prop::sample::select(FORMS.to_vec())),
        any::<bool>(),
    );
    (pictures, values).prop_map(|((raw, text, key, signed, packed_key), values)| {
        let (amount, packed_amount, count, date, (day, time, form), blank) = values;
        let stamp = (!blank).then(|| {
            CobolTimestamp::new(timestamp_in(day, time, form), form).expect("a layout value")
        });
        AllKinds {
            raw,
            text: PicX::decode(&text).expect("printable text"),
            key: PicU::new(key).expect("five digits"),
            signed: PicS::new(signed).expect("five digits"),
            packed_key: PicS::new(packed_key).expect("nine digits"),
            amount: Decimal::new(amount, 2),
            packed_amount: Decimal::new(packed_amount, 2),
            count: Count::new(count),
            date: PicDate::new(date).expect("a four-digit year"),
            stamp: PicTimestamp::new(stamp),
        }
    })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn every_reader_and_writer_method_round_trips(record in all_kinds()) {
        let bytes = record.encode();
        prop_assert_eq!(bytes.len(), AllKinds::WIDTH);
        prop_assert_eq!(AllKinds::decode(&bytes), Ok(record.clone()));
        let framed = Framed {
            record,
            framing: LineFraming { terminator: Terminator::CrLf, body_len: AllKinds::WIDTH },
        };
        let line = framed.encode();
        prop_assert_eq!(line.get(..AllKinds::WIDTH), Some(bytes.as_slice()));
        prop_assert_eq!(line.get(AllKinds::WIDTH..), Some(b"\r\n".as_slice()));
        prop_assert_eq!(framed.try_encode(), Ok(line));
    }
}

#[test]
fn reader_offset_advances_by_each_field_width() -> TestResult {
    let record = AllKinds {
        raw: vec![0x00, 0xFF, b'\r'],
        text: PicX::from_array(*b"AB C"),
        key: PicU::new(42)?,
        signed: PicS::new(-305)?,
        packed_key: PicS::new(999_999_999)?,
        amount: Decimal::new(-91_900, 2),
        packed_amount: Decimal::new(123_456_789_012, 2),
        count: Count::new(-1),
        date: PicDate::decode(b"2022-07-18")?,
        stamp: PicTimestamp::decode(b"2022-07-18-00.00.00.120000")?,
    };
    let bytes = record.encode();
    let mut r = RecordReader::new(&bytes, AllKinds::WIDTH)?;
    let mut offsets = vec![r.offset()];
    assert_eq!(r.take("ALL-RAW", 3)?, record.raw.as_slice());
    offsets.push(r.offset());
    assert_eq!(r.pic_x::<4>("ALL-TEXT")?, record.text);
    offsets.push(r.offset());
    assert_eq!(r.pic_u::<5>("ALL-KEY")?, record.key);
    offsets.push(r.offset());
    assert_eq!(r.pic_s_zoned::<5>("ALL-SIGNED")?, record.signed);
    offsets.push(r.offset());
    assert_eq!(r.pic_s_packed::<9>("ALL-PACKED-KEY")?, record.packed_key);
    offsets.push(r.offset());
    assert_eq!(r.zoned("ALL-AMOUNT", 9, 2)?, record.amount);
    offsets.push(r.offset());
    assert_eq!(r.packed("ALL-PACKED-AMOUNT", 12, 2)?, record.packed_amount);
    offsets.push(r.offset());
    assert_eq!(r.count("ALL-COUNT")?, record.count);
    offsets.push(r.offset());
    assert_eq!(r.date("ALL-DATE")?, record.date);
    offsets.push(r.offset());
    assert_eq!(r.timestamp("ALL-STAMP")?, record.stamp);
    offsets.push(r.offset());
    r.finish()?;
    let steps: Vec<usize> = offsets.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert_eq!(steps, ALL_KINDS_WIDTHS);
    assert_eq!(offsets.last(), Some(&AllKinds::WIDTH));
    Ok(())
}

#[test]
fn writer_tracks_its_length() {
    let mut w = RecordWriter::with_capacity(AllKinds::WIDTH);
    assert!(w.is_empty());
    assert_eq!(w.len(), 0);
    w.bytes(b"ABC").count(Count::ZERO);
    assert!(!w.is_empty());
    assert_eq!(w.len(), 5);
    assert_eq!(w.finish(), b"ABC\x00\x00");
}

// ---------------------------------------------------------------------------------------------
// Fixture framing (E04, E06)
// ---------------------------------------------------------------------------------------------

/// The nine fixture files: name, line count, body length, and how many leading lines end CRLF
/// (every later line ends LF).
const FIXTURES: [(&str, usize, usize, usize); 9] = [
    ("acctdata.txt", 50, 300, 0),
    ("carddata.txt", 50, 150, 0),
    ("cardxref.txt", 50, 36, 0),
    ("custdata.txt", 50, 500, 0),
    ("dailytran.txt", 300, 350, 0),
    ("discgrp.txt", 51, 50, 0),
    ("tcatbal.txt", 50, 50, 49),
    ("trancatg.txt", 18, 60, 18),
    ("trantype.txt", 7, 60, 6),
];

// audit: PIC-014
// audit: FIXFRM-010
// audit: FIXFRM-013
// audit: FIXFRM-018
#[test]
fn fixture_lines_reassemble_byte_for_byte() {
    let mut total = 0;
    for (name, line_count, body_len, crlf_lines) in FIXTURES {
        let bytes = fixture(name);
        let mut reassembled = Vec::with_capacity(bytes.len());
        let mut count = 0;
        for (index, line) in lines(&bytes).enumerate() {
            let number = index + 1;
            let expected = if number <= crlf_lines {
                Terminator::CrLf
            } else {
                Terminator::Lf
            };
            assert!(
                line.framing.terminator == expected,
                "{name}:{number}: terminator"
            );
            assert!(
                line.body.len() == body_len && line.framing.body_len == body_len,
                "{name}:{number}: body length"
            );
            assert!(
                line.body.iter().all(|byte| (0x20..=0x7E).contains(byte)),
                "{name}:{number}: a body byte outside printable ASCII"
            );
            reassembled.extend_from_slice(line.body);
            reassembled.extend_from_slice(line.framing.terminator.as_bytes());
            count += 1;
        }
        assert!(count == line_count, "{name}: {count} lines");
        assert!(reassembled == bytes, "{name}: reassembled lines differ");
        total += count;
    }
    assert_eq!(total, 626);
}

/// `(body, terminator)` of every line of `input`.
fn split(input: &[u8]) -> Vec<(Vec<u8>, Terminator)> {
    lines(input)
        .map(|line| {
            assert_eq!(line.framing.body_len, line.body.len());
            (line.body.to_vec(), line.framing.terminator)
        })
        .collect()
}

// audit: PIC-014
#[test]
fn framing_edge_cases_split_losslessly() {
    assert!(split(b"").is_empty());
    assert_eq!(split(b"abc"), [(b"abc".to_vec(), Terminator::None)]);
    assert_eq!(
        split(b"a\rb\n"),
        [(b"a\rb".to_vec(), Terminator::Lf)],
        "a lone CR stays in the body"
    );
    assert_eq!(split(b"\r\n"), [(Vec::new(), Terminator::CrLf)]);
    assert_eq!(
        split(b"\n\n"),
        [(Vec::new(), Terminator::Lf), (Vec::new(), Terminator::Lf)]
    );
    assert_eq!(split(b"\r"), [(b"\r".to_vec(), Terminator::None)]);
    assert_eq!(
        split(b"01\r\n02\n03"),
        [
            (b"01".to_vec(), Terminator::CrLf),
            (b"02".to_vec(), Terminator::Lf),
            (b"03".to_vec(), Terminator::None),
        ]
    );
    for input in [
        b"".as_slice(),
        b"abc",
        b"a\rb\n",
        b"\r\n",
        b"\n\n",
        b"x\r\r\ny",
    ] {
        let reassembled: Vec<u8> = lines(input)
            .flat_map(|line| [line.body, line.framing.terminator.as_bytes()].concat())
            .collect();
        assert_eq!(reassembled, input);
    }
    let mut finished = lines(b"z");
    assert!(finished.next().is_some());
    assert!(finished.next().is_none());
    assert!(finished.next().is_none(), "the split is fused");
    assert_eq!(lines(b"").size_hint(), (0, Some(0)));
    assert_eq!(lines(b"ab\ncd").size_hint(), (1, Some(5)));
}

#[test]
fn terminators_have_their_bytes() {
    assert_eq!(Terminator::Lf.as_bytes(), b"\n");
    assert_eq!(Terminator::CrLf.as_bytes(), b"\r\n");
    assert_eq!(Terminator::None.as_bytes(), b"");
    let line = Line::new(b"AB", Terminator::CrLf);
    assert_eq!(
        line.framing,
        LineFraming {
            terminator: Terminator::CrLf,
            body_len: 2
        }
    );
}

// ---------------------------------------------------------------------------------------------
// Fixture fields at their copybook offsets
// ---------------------------------------------------------------------------------------------

/// Decodes the signed DISPLAY field at `offset` of `picture` `(int_digits, scale)`, checks that
/// it re-encodes to the same bytes, and returns it.
fn zoned_field(body: &[u8], offset: usize, picture: (u32, u32), at: &str) -> Decimal {
    let (int_digits, scale) = picture;
    let bytes = field(body, offset, width(int_digits + scale), at);
    let value = zoned::decode_signed(bytes, int_digits, scale)
        .unwrap_or_else(|kind| panic!("{at}: {kind}"));
    assert!(value.scale() == scale, "{at}: scale");
    let out = encoded(|out| zoned::encode_signed(value, int_digits, scale, out));
    assert!(out == bytes, "{at}: re-encoded bytes differ");
    value
}

/// Decodes the `PIC 9(N)` field at `offset`, checks its round trip and returns it.
fn pic_u_field<const N: u32>(body: &[u8], offset: usize, at: &str) -> PicU<N> {
    let bytes = field(body, offset, width(N), at);
    let value = PicU::<N>::decode(bytes).unwrap_or_else(|kind| panic!("{at}: {kind}"));
    assert!(
        encoded(|out| value.encode_into(out)) == bytes,
        "{at}: re-encoded bytes differ"
    );
    value
}

/// Decodes the `PIC X(N)` field at `offset`, checks its round trip and returns it.
fn pic_x_field<const N: usize>(body: &[u8], offset: usize, at: &str) -> PicX<N> {
    let bytes = field(body, offset, N, at);
    let value = PicX::<N>::decode(bytes).unwrap_or_else(|kind| panic!("{at}: {kind}"));
    assert!(
        encoded(|out| value.encode_into(out)) == bytes,
        "{at}: re-encoded bytes differ"
    );
    value
}

/// Decodes the `PIC X(10)` date at `offset`, checks its round trip and returns the date, which
/// every fixture date has (FIXFRM-016).
fn date_field(body: &[u8], offset: usize, at: &str) -> NaiveDate {
    let bytes = field(body, offset, PicDate::WIDTH, at);
    let value = PicDate::decode(bytes).unwrap_or_else(|kind| panic!("{at}: {kind}"));
    assert!(
        encoded(|out| value.encode_into(out)) == bytes,
        "{at}: re-encoded bytes differ"
    );
    value.value().unwrap_or_else(|| panic!("{at}: blank date"))
}

/// Decodes the `PIC X(26)` timestamp at `offset`, checks its round trip and returns it.
fn timestamp_field(body: &[u8], offset: usize, at: &str) -> PicTimestamp {
    let bytes = field(body, offset, PicTimestamp::WIDTH, at);
    let value = PicTimestamp::decode(bytes).unwrap_or_else(|kind| panic!("{at}: {kind}"));
    assert!(
        encoded(|out| value.encode_into(out)) == bytes,
        "{at}: re-encoded bytes differ"
    );
    value
}

/// Asserts that the `PIC X(N)` `FILLER` at `offset` is kept byte for byte and holds only `fill`
/// (PIC-012, E05).
fn filler_field<const N: usize>(body: &[u8], offset: usize, fill: u8, at: &str) {
    let filler = pic_x_field::<N>(body, offset, at);
    assert!(
        filler.as_bytes().iter().all(|&byte| byte == fill),
        "{at}: FILLER content"
    );
}

/// The five `PIC S9(10)V99` fields of `CVACT01Y`: name and offset (CVACT01Y-003 to -005, -009,
/// -010).
const ACCOUNT_AMOUNTS: [(&str, usize); 5] = [
    ("ACCT-CURR-BAL", 12),
    ("ACCT-CREDIT-LIMIT", 24),
    ("ACCT-CASH-CREDIT-LIMIT", 36),
    ("ACCT-CURR-CYC-CREDIT", 78),
    ("ACCT-CURR-CYC-DEBIT", 90),
];

/// The three `PIC X(10)` dates of `CVACT01Y`: name and offset (CVACT01Y-006 to -008).
const ACCOUNT_DATES: [(&str, usize); 3] = [
    ("ACCT-OPEN-DATE", 48),
    ("ACCT-EXPIRAION-DATE", 58),
    ("ACCT-REISSUE-DATE", 68),
];

// audit: PIC-005
// audit: PIC-006
// audit: PIC-012
// audit: PIC-015
// audit: FIXFRM-001
// audit: FIXFRM-014
// audit: FIXFRM-015
// audit: FIXFRM-016
#[test]
fn acctdata_fields_round_trip() {
    let name = "acctdata.txt";
    let mut earliest_expiry: Option<NaiveDate> = None;
    for_each_line(name, |number, line| {
        let body = line.body;
        let id = pic_u_field::<11>(body, 0, &format!("{name}:{number} ACCT-ID"));
        assert!(
            usize::try_from(id.get()).ok() == Some(number),
            "{name}:{number}: line n holds account n"
        );
        for (field_name, offset) in ACCOUNT_AMOUNTS {
            let at = format!("{name}:{number} {field_name}");
            zoned_field(body, offset, (10, 2), &at);
            assert!(body.get(offset + 11) == Some(&b'{'), "{at}: sign byte");
        }
        for (field_name, offset) in ACCOUNT_DATES {
            let date = date_field(body, offset, &format!("{name}:{number} {field_name}"));
            if field_name == "ACCT-EXPIRAION-DATE" {
                earliest_expiry = Some(earliest_expiry.map_or(date, |seen| seen.min(date)));
            }
        }
        let zip = pic_x_field::<10>(body, 102, &format!("{name}:{number} ACCT-ADDR-ZIP"));
        assert!(
            zip.as_bytes() == b"A000000000",
            "{name}:{number}: ACCT-ADDR-ZIP"
        );
        let group = pic_x_field::<10>(body, 112, &format!("{name}:{number} ACCT-GROUP-ID"));
        assert!(group.is_spaces(), "{name}:{number}: ACCT-GROUP-ID");
        filler_field::<178>(body, 122, b' ', &format!("{name}:{number} FILLER"));
    });
    assert_eq!(earliest_expiry, NaiveDate::from_ymd_opt(2023, 1, 6));
}

// audit: PIC-003
// audit: PIC-012
// audit: PIC-015
// audit: FIXFRM-002
// audit: FIXFRM-016
// audit: FIXFRM-019
#[test]
fn carddata_fields_round_trip() {
    let name = "carddata.txt";
    let mut accounts = BTreeSet::new();
    for_each_line(name, |number, line| {
        let body = line.body;
        let at = |field_name: &str| format!("{name}:{number} {field_name}");
        pic_x_field::<16>(body, 0, &at("CARD-NUM"));
        accounts.insert(pic_u_field::<11>(body, 16, &at("CARD-ACCT-ID")).get());
        pic_u_field::<3>(body, 27, &at("CARD-CVV-CD"));
        pic_x_field::<50>(body, 30, &at("CARD-EMBOSSED-NAME"));
        date_field(body, 80, &at("CARD-EXPIRAION-DATE"));
        let status = pic_x_field::<1>(body, 90, &at("CARD-ACTIVE-STATUS"));
        assert!(
            status.as_bytes() == b"Y",
            "{name}:{number}: CARD-ACTIVE-STATUS"
        );
        filler_field::<59>(body, 91, b' ', &at("FILLER"));
    });
    assert_eq!(accounts.len(), 50);
}

// audit: PIC-003
// audit: PIC-012
// audit: PIC-015
// audit: FIXFRM-004
// audit: FIXFRM-016
// audit: FIXFRM-019
#[test]
fn custdata_fields_round_trip() {
    let name = "custdata.txt";
    for_each_line(name, |number, line| {
        let body = line.body;
        let at = |field_name: &str| format!("{name}:{number} {field_name}");
        let id = pic_u_field::<9>(body, 0, &at("CUST-ID"));
        assert!(
            usize::try_from(id.get()).ok() == Some(number),
            "{name}:{number}: line n holds customer n"
        );
        pic_u_field::<9>(body, 279, &at("CUST-SSN"));
        date_field(body, 308, &at("CUST-DOB-YYYY-MM-DD"));
        pic_u_field::<3>(body, 329, &at("CUST-FICO-CREDIT-SCORE"));
        filler_field::<168>(body, 332, b' ', &at("FILLER"));
    });
}

/// The instant of every `DALYTRAN-ORIG-TS` in `dailytran.txt` (FIXFRM-017).
fn dailytran_orig_ts() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2022, 6, 10)
        .and_then(|day| day.and_hms_opt(19, 27, 53))
        .expect("a valid instant")
}

// audit: PIC-005
// audit: PIC-006
// audit: PIC-017
// audit: PIC-018
// audit: FIXFRM-005
// audit: FIXFRM-015
// audit: FIXFRM-017
// audit: FIXFRM-019
#[test]
fn dailytran_fields_round_trip() {
    let name = "dailytran.txt";
    let mut sign_bytes = [0_usize; 20];
    let (mut purchases, mut credits, mut negatives) = (0, 0, 0);
    let mut cards = BTreeSet::new();
    for_each_line(name, |number, line| {
        let body = line.body;
        let at = |field_name: &str| format!("{name}:{number} {field_name}");
        let kind = (
            pic_x_field::<2>(body, 16, &at("DALYTRAN-TYPE-CD")),
            pic_u_field::<4>(body, 18, &at("DALYTRAN-CAT-CD")).get(),
            pic_x_field::<10>(body, 22, &at("DALYTRAN-SOURCE")),
        );
        let is_credit = match kind {
            (code, 1, source) if code.as_bytes() == b"01" && source.as_bytes() == b"POS TERM  " => {
                false
            }
            (code, 1, source) if code.as_bytes() == b"03" && source.as_bytes() == b"OPERATOR  " => {
                true
            }
            _ => panic!("{name}:{number}: type, category or source"),
        };
        let amount = zoned_field(body, 132, (9, 2), &at("DALYTRAN-AMT"));
        assert!(
            amount.is_sign_negative() == is_credit,
            "{name}:{number}: DALYTRAN-AMT is negative exactly on type 03"
        );
        let sign = body.get(142).copied().unwrap_or_default();
        let slot = OVERPUNCH.iter().position(|&byte| byte == sign);
        sign_bytes[slot.unwrap_or_else(|| panic!("{name}:{number}: sign byte"))] += 1;
        pic_u_field::<9>(body, 143, &at("DALYTRAN-MERCHANT-ID"));
        cards.insert(pic_x_field::<16>(body, 262, &at("DALYTRAN-CARD-NUM")));
        let orig = timestamp_field(body, 278, &at("DALYTRAN-ORIG-TS")).value();
        assert!(
            orig.is_some_and(|ts| ts.form() == TsForm::Iso && ts.value() == dailytran_orig_ts()),
            "{name}:{number}: DALYTRAN-ORIG-TS"
        );
        let proc = timestamp_field(body, 304, &at("DALYTRAN-PROC-TS"));
        assert!(
            proc == PicTimestamp::BLANK,
            "{name}:{number}: DALYTRAN-PROC-TS"
        );
        filler_field::<20>(body, 330, b' ', &at("FILLER"));
        purchases += usize::from(!is_credit);
        credits += usize::from(is_credit);
        negatives += usize::from(amount.is_sign_negative());
    });
    assert_eq!((purchases, credits, negatives), (250, 50, 50));
    let (positive, negative) = sign_bytes.split_at(10);
    assert!(
        sign_bytes.iter().all(|&seen| seen > 0),
        "every overpunch byte occurs"
    );
    assert_eq!(
        (positive[0], positive[1..].iter().sum::<usize>()),
        (25, 225)
    );
    assert_eq!((negative[0], negative[1..].iter().sum::<usize>()), (6, 44));
    let xref_cards: BTreeSet<PicX<16>> = {
        let mut set = BTreeSet::new();
        for_each_line("cardxref.txt", |number, line| {
            set.insert(pic_x_field::<16>(
                line.body,
                0,
                &format!("cardxref.txt:{number}"),
            ));
        });
        set
    };
    assert!(
        cards.len() == 50,
        "{name}: distinct DALYTRAN-CARD-NUM values"
    );
    assert!(
        cards == xref_cards,
        "{name}: every card is on one cardxref.txt line"
    );
}

// audit: PIC-005
// audit: PIC-012
// audit: FIXFRM-006
// audit: FIXFRM-012
// audit: FIXFRM-015
// audit: FIXFRM-019
#[test]
fn discgrp_fields_round_trip() {
    let name = "discgrp.txt";
    let mut keys_by_group: Vec<Vec<(PicX<2>, u64)>> = vec![Vec::new(); 3];
    let mut rates = Vec::new();
    for_each_line(name, |number, line| {
        let body = line.body;
        let at = |field_name: &str| format!("{name}:{number} {field_name}");
        let group = pic_x_field::<10>(body, 0, &at("DIS-ACCT-GROUP-ID"));
        let expected: &[u8; 10] = match number {
            1..=17 => b"A000000000",
            18..=34 => b"DEFAULT   ",
            _ => b"ZEROAPR   ",
        };
        assert!(
            group.as_bytes() == expected,
            "{name}:{number}: DIS-ACCT-GROUP-ID"
        );
        let key = (
            pic_x_field::<2>(body, 10, &at("DIS-TRAN-TYPE-CD")),
            pic_u_field::<4>(body, 12, &at("DIS-TRAN-CAT-CD")).get(),
        );
        keys_by_group[(number - 1) / 17].push(key);
        let rate = zoned_field(body, 16, (4, 2), &at("DIS-INT-RATE"));
        assert!(body.get(21) == Some(&b'{'), "{name}:{number}: sign byte");
        if number > 34 {
            assert!(rate.is_zero(), "{name}:{number}: ZEROAPR rate");
        }
        rates.push((number, rate));
        filler_field::<28>(body, 22, b'0', &at("FILLER"));
    });
    assert!(keys_by_group.iter().all(|keys| keys.len() == 17));
    assert!(keys_by_group.windows(2).all(|pair| pair[0] == pair[1]));
    assert!(
        rates.contains(&(18, Decimal::new(1_500, 2))),
        "DEFAULT 01/0001"
    );
    assert!(rates.contains(&(25, Decimal::ZERO)), "DEFAULT 03/0001");
    let mut distinct: Vec<Decimal> = rates.iter().map(|&(_, rate)| rate).collect();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct,
        [
            Decimal::ZERO,
            Decimal::new(1_500, 2),
            Decimal::new(2_500, 2)
        ]
    );
}

// audit: PIC-005
// audit: PIC-012
// audit: PIC-014
// audit: FIXFRM-007
// audit: FIXFRM-012
// audit: FIXFRM-015
// audit: FIXFRM-019
#[test]
fn tcatbal_fields_round_trip() {
    let name = "tcatbal.txt";
    for_each_line(name, |number, line| {
        let body = line.body;
        let at = |field_name: &str| format!("{name}:{number} {field_name}");
        let account = pic_u_field::<11>(body, 0, &at("TRANCAT-ACCT-ID"));
        let type_cd = pic_x_field::<2>(body, 11, &at("TRANCAT-TYPE-CD"));
        let cat_cd = pic_u_field::<4>(body, 13, &at("TRANCAT-CD"));
        assert!(
            usize::try_from(account.get()).ok() == Some(number)
                && type_cd.as_bytes() == b"01"
                && cat_cd.get() == 1,
            "{name}:{number}: line n holds the key (account n, 01, 0001)"
        );
        let balance = zoned_field(body, 17, (9, 2), &at("TRAN-CAT-BAL"));
        assert!(
            balance.is_zero() && balance.scale() == 2 && body.get(27) == Some(&b'{'),
            "{name}:{number}: TRAN-CAT-BAL is 0.00"
        );
        filler_field::<22>(body, 28, b'0', &at("FILLER"));
        let expected = if number < 50 {
            Terminator::CrLf
        } else {
            Terminator::Lf
        };
        assert!(
            line.framing.terminator == expected,
            "{name}:{number}: terminator"
        );
    });
}

// audit: PIC-012
// audit: FIXFRM-008
// audit: FIXFRM-009
// audit: FIXFRM-012
#[test]
fn transaction_type_and_category_fields_round_trip() {
    let mut categories = Vec::new();
    for_each_line("trancatg.txt", |number, line| {
        let at = |field_name: &str| format!("trancatg.txt:{number} {field_name}");
        let key = (
            pic_x_field::<2>(line.body, 0, &at("TRAN-TYPE-CD")),
            pic_u_field::<4>(line.body, 2, &at("TRAN-CAT-CD")),
        );
        let description = pic_x_field::<50>(line.body, 6, &at("TRAN-CAT-TYPE-DESC"));
        filler_field::<4>(line.body, 56, b'0', &at("FILLER"));
        categories.push((key, description.to_text()));
    });
    assert_eq!(categories.len(), 18);
    let first = &categories[0];
    assert_eq!(
        (first.0.0.as_bytes(), first.0.1.get(), first.1.as_str()),
        (b"01", 1, "Regular Sales Draft")
    );
    let ninth = &categories[8];
    assert_eq!(
        (ninth.0.0.as_bytes(), ninth.0.1.get(), ninth.1.as_str()),
        (b"03", 1, "Credit to Account")
    );

    let mut types = Vec::new();
    for_each_line("trantype.txt", |number, line| {
        let at = |field_name: &str| format!("trantype.txt:{number} {field_name}");
        let code = pic_x_field::<2>(line.body, 0, &at("TRAN-TYPE"));
        assert!(
            code.to_text() == format!("{number:02}"),
            "trantype.txt:{number}: key"
        );
        let description = pic_x_field::<50>(line.body, 2, &at("TRAN-TYPE-DESC"));
        filler_field::<8>(line.body, 52, b'0', &at("FILLER"));
        types.push(description.to_text());
    });
    assert_eq!(types.len(), 7);
    assert_eq!(
        (types[0].as_str(), types[2].as_str()),
        ("Purchase", "Credit")
    );
}

/// Each fixture with the offset and length of its record key (FIXFRM-020).
const KEYS: [(&str, usize, usize); 9] = [
    ("acctdata.txt", 0, 11),
    ("carddata.txt", 0, 16),
    ("cardxref.txt", 0, 16),
    ("custdata.txt", 0, 9),
    ("discgrp.txt", 0, 16),
    ("tcatbal.txt", 0, 17),
    ("trancatg.txt", 0, 6),
    ("trantype.txt", 0, 2),
    ("dailytran.txt", 0, 16),
];

// audit: FIXFRM-020
#[test]
fn fixture_keys_are_unique_and_ascending() {
    for (name, offset, len) in KEYS {
        let mut previous: Option<Vec<u8>> = None;
        for_each_line(name, |number, line| {
            let key = field(line.body, offset, len, &format!("{name}:{number} key")).to_vec();
            assert!(
                previous.as_ref().is_none_or(|seen| *seen < key),
                "{name}:{number}: key not above the previous line's"
            );
            previous = Some(key);
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Short bodies: the `CVACT03Y` shape (E04)
// ---------------------------------------------------------------------------------------------

/// A test-local record of the `CVACT03Y` `CARD-XREF-RECORD` layout (`CVACT03Y.cpy:5-8`), 50
/// bytes ending in a 14-byte `FILLER`.
#[derive(Clone, PartialEq, Eq)]
struct XrefShaped {
    card_num: PicX<16>,
    cust_id: PicU<9>,
    acct_id: PicU<11>,
    filler: PicX<14>,
}

/// Names the type only: `XREF-CARD-NUM` is a card number (Cardholder Data Redaction).
impl fmt::Debug for XrefShaped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("XrefShaped").finish_non_exhaustive()
    }
}

impl FixedRecord for XrefShaped {
    const WIDTH: usize = 50;
    const TRAILING_FILLER: usize = 14;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            card_num: r.pic_x("XREF-CARD-NUM")?,
            cust_id: r.pic_u("XREF-CUST-ID")?,
            acct_id: r.pic_u("XREF-ACCT-ID")?,
            filler: r.pic_x("FILLER")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.card_num)
            .pic_u(self.cust_id)
            .pic_u(self.acct_id)
            .pic_x(&self.filler);
        w.finish()
    }
}

/// A 36-byte body of the `cardxref.txt` shape with an invented card number.
const XREF_BODY: &[u8; 36] = b"987654321098765400000004200000000042";

// audit: PIC-013
// audit: PIC-024
// audit: FIXFRM-003
// audit: FIXFRM-011
// audit: FIXFRM-019
#[test]
fn cardxref_short_bodies_round_trip() {
    let name = "cardxref.txt";
    let (mut customers, mut accounts) = (BTreeSet::new(), BTreeSet::new());
    for_each_line(name, |number, line| {
        let framed = Framed::<XrefShaped>::decode(*line)
            .unwrap_or_else(|err| panic!("{name}:{number}: {err}"));
        assert!(
            framed.framing
                == LineFraming {
                    terminator: Terminator::Lf,
                    body_len: 36
                },
            "{name}:{number}: framing"
        );
        assert!(
            framed.record.filler == PicX::spaces(),
            "{name}:{number}: FILLER"
        );
        let mut line_bytes = line.body.to_vec();
        line_bytes.push(b'\n');
        assert!(framed.encode() == line_bytes, "{name}:{number}: encode");
        assert!(
            framed.try_encode().ok() == Some(line_bytes),
            "{name}:{number}: try_encode"
        );
        let shown = format!("{framed:?}");
        let card = String::from_utf8_lossy(framed.record.card_num.as_bytes()).into_owned();
        assert!(
            !shown.contains(&card),
            "{name}:{number}: Debug shows XREF-CARD-NUM"
        );
        let (cust_id, acct_id) = (framed.record.cust_id.get(), framed.record.acct_id.get());
        assert!(
            cust_id == acct_id,
            "{name}:{number}: customer equals account"
        );
        customers.insert(cust_id);
        accounts.insert(acct_id);
    });
    assert_eq!((customers.len(), accounts.len()), (50, 50));

    let bytes = fixture(name);
    let records: Vec<Framed<XrefShaped>> =
        decode_all(&bytes).unwrap_or_else(|err| panic!("{name}: {err}"));
    assert_eq!(records.len(), 50);
    assert!(encode_all(&records) == bytes, "{name}: encode_all");
}

// audit: PIC-013
#[test]
fn short_body_limits() -> TestResult {
    let line = Line::new(XREF_BODY, Terminator::CrLf);
    let framed = Framed::<XrefShaped>::decode(line)?;
    assert_eq!(framed.framing.body_len, 36);
    assert_eq!(framed.encode(), [XREF_BODY.as_slice(), b"\r\n"].concat());

    let short = XREF_BODY.get(..35).unwrap_or_default();
    let not_filler = ErrorKind::ShortBodyNotFiller {
        expected: 50,
        actual: 35,
    };
    let result = Framed::<XrefShaped>::decode(Line::new(short, Terminator::Lf));
    assert_error(result, ("record", 0, not_filler), short);

    let full = [XREF_BODY.as_slice(), b"00000000000000"].concat();
    let framed = Framed::<XrefShaped>::decode(Line::new(&full, Terminator::None))?;
    assert_eq!(framed.framing.body_len, 50);
    assert_eq!(framed.record.filler.as_bytes(), b"00000000000000");
    assert_eq!(framed.encode(), full);

    let mut bad_field = full.clone();
    bad_field[16] = b'X';
    let result = Framed::<XrefShaped>::decode(Line::new(&bad_field, Terminator::Lf));
    assert_error(
        result,
        ("XREF-CUST-ID", 16, ErrorKind::NonDigit),
        &bad_field,
    );

    let long = [full.as_slice(), b"0"].concat();
    let too_long = ErrorKind::WidthMismatch {
        expected: 50,
        actual: 51,
    };
    let result = Framed::<XrefShaped>::decode(Line::new(&long, Terminator::Lf));
    assert_error(result, ("record", 0, too_long), &long);
    Ok(())
}

// audit: PIC-013
#[test]
fn try_encode_refuses_bytes_a_short_body_would_drop() -> TestResult {
    let full = [XREF_BODY.as_slice(), b"00000000000000"].concat();
    let record = XrefShaped::decode(&full)?;
    let framing = |body_len| LineFraming {
        terminator: Terminator::Lf,
        body_len,
    };
    let dropping = Framed {
        record: record.clone(),
        framing: framing(36),
    };
    assert_error(
        dropping.try_encode(),
        ("record", 36, ErrorKind::NonBlankTail),
        XREF_BODY,
    );
    let too_short = Framed {
        record: record.clone(),
        framing: framing(35),
    };
    let not_filler = ErrorKind::ShortBodyNotFiller {
        expected: 50,
        actual: 35,
    };
    assert_error(too_short.try_encode(), ("record", 0, not_filler), XREF_BODY);
    let too_long = Framed {
        record: record.clone(),
        framing: framing(51),
    };
    let mismatch = ErrorKind::WidthMismatch {
        expected: 50,
        actual: 51,
    };
    assert_error(too_long.try_encode(), ("record", 0, mismatch), XREF_BODY);
    assert_eq!(
        too_long.encode(),
        [full.as_slice(), b"\n"].concat(),
        "encode cuts nothing"
    );
    let whole = Framed {
        record,
        framing: framing(50),
    };
    assert_eq!(whole.try_encode()?, [full.as_slice(), b"\n"].concat());
    Ok(())
}

#[test]
fn decode_all_reports_the_absolute_offset() {
    let good = [XREF_BODY.as_slice(), b"\n"].concat();
    let mut bad_field = *XREF_BODY;
    bad_field[16] = b'X';
    let input = [good.as_slice(), &bad_field, b"\n"].concat();
    let line_start = good.len();
    assert_error(
        decode_all::<XrefShaped>(&input),
        ("XREF-CUST-ID", line_start + 16, ErrorKind::NonDigit),
        &bad_field,
    );
    let short_line = XREF_BODY.get(..20).unwrap_or_default();
    let input = [good.as_slice(), short_line].concat();
    let not_filler = ErrorKind::ShortBodyNotFiller {
        expected: 50,
        actual: 20,
    };
    assert_error(
        decode_all::<XrefShaped>(&input),
        ("record", line_start, not_filler),
        short_line,
    );
    assert_eq!(decode_all::<XrefShaped>(b""), Ok(Vec::new()));
}

// ---------------------------------------------------------------------------------------------
// Typed errors (R7): never a default or a normalised value
// ---------------------------------------------------------------------------------------------

// audit: PIC-007
// audit: PIC-008
// audit: PIC-009
#[test]
fn display_sign_anomalies_are_typed_errors() {
    let cases: [(&[u8], u32, ErrorKind); 9] = [
        (b"123456789015", 10, ErrorKind::PlainDigitSign),
        (b"00000042000", 9, ErrorKind::PlainDigitSign),
        (b"0000000000}", 9, ErrorKind::NegativeZero),
        (b"00000000000}", 10, ErrorKind::NegativeZero),
        (b"0000004200#", 9, ErrorKind::InvalidOverpunch),
        (b"0000004200\xFF", 9, ErrorKind::InvalidOverpunch),
        (b"0000004200 ", 9, ErrorKind::InvalidOverpunch),
        (b"00000 4200{", 9, ErrorKind::NonDigit),
        (b"-000004200{", 9, ErrorKind::NonDigit),
    ];
    for (bytes, int_digits, kind) in cases {
        assert_kind(zoned::decode_signed(bytes, int_digits, 2), kind, bytes);
    }
    assert_kind(
        zoned::decode_pic_s::<6>(b"00000}"),
        ErrorKind::NegativeZero,
        b"00000}",
    );
    assert_kind(
        zoned::decode_pic_s::<6>(b"123456"),
        ErrorKind::PlainDigitSign,
        b"123456",
    );
}

// audit: PIC-004
#[test]
fn unsigned_digits_never_default() {
    assert_kind(
        PicU::<9>::decode(b"0000X0042"),
        ErrorKind::NonDigit,
        b"0000X0042",
    );
    assert_kind(
        PicU::<9>::decode(b"+00000042"),
        ErrorKind::NonDigit,
        b"+00000042",
    );
    let blank = [b' '; 11];
    let result = PicU::<11>::decode(&blank);
    assert!(result != Ok(PicU::ZERO), "spaces are never zero");
    assert_kind(result, ErrorKind::NonDigit, &blank);
    assert_eq!(PicU::<11>::new(10_u64.pow(11)), Err(ErrorKind::OutOfRange));
    assert_eq!(PicU::<11>::try_from(-1_i64), Err(ErrorKind::OutOfRange));
    assert_eq!(PicU::<11>::try_from(42_i64).map(PicU::get), Ok(42));
    assert_eq!(PicU::<3>::try_from(1_000_u64), Err(ErrorKind::OutOfRange));
    assert_eq!(PicU::<3>::default(), PicU::ZERO);
}

#[test]
fn key_successors_never_wrap() {
    assert_eq!(
        PicU::<3>::new(998).and_then(PicU::succ).map(PicU::get),
        Ok(999)
    );
    assert_eq!(
        PicU::<3>::new(999).and_then(PicU::succ),
        Err(ErrorKind::OutOfRange)
    );
    let highest = PicU::<11>::new(99_999_999_999);
    assert_eq!(highest.and_then(PicU::succ), Err(ErrorKind::OutOfRange));
    assert_eq!(PicU::<11>::MAX, 99_999_999_999);
    // audit: PIC-003
    assert_eq!(
        encoded(|out| PicU::<11>::new(1).expect("in range").encode_into(out)),
        b"00000000001"
    );
}

// audit: PIC-010
#[test]
fn packed_anomalies_are_typed_errors() {
    // PIC S9(05) COMP-3, 3 bytes: 00042 positive is 00 04 2C.
    for sign in [0x0A_u8, 0x0B, 0x0E, 0x0F, 0x00, 0x09] {
        let bytes = [0x00, 0x04, 0x20 | sign];
        assert_kind(
            packed::decode(&bytes, 5, 0),
            ErrorKind::InvalidPackedSign,
            &bytes,
        );
    }
    let bad_digit = [0x00, 0x4A, 0x2C];
    assert_kind(
        packed::decode(&bad_digit, 5, 0),
        ErrorKind::InvalidPackedDigit,
        &bad_digit,
    );
    // PIC S9(10)V99 COMP-3, 7 bytes with a leading pad nibble.
    let bad_pad = [0x10, 0x00, 0x00, 0x00, 0x04, 0x20, 0x0C];
    assert_kind(
        packed::decode(&bad_pad, 12, 2),
        ErrorKind::NonZeroPadNibble,
        &bad_pad,
    );
    let negative_zero = [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0D];
    assert_kind(
        packed::decode(&negative_zero, 12, 2),
        ErrorKind::NegativeZero,
        &negative_zero,
    );
    assert_kind(
        packed::decode_pic_s::<5>(&[0x00, 0x00, 0x0D]),
        ErrorKind::NegativeZero,
        &[0x00, 0x00, 0x0D],
    );
    assert_eq!(
        packed::decode(&[0x00, 0x04, 0x2C], 5, 0),
        Ok(Decimal::new(42, 0))
    );
    assert_eq!(
        packed::decode(&[0x00, 0x04, 0x2D], 5, 2),
        Ok(Decimal::new(-42, 2))
    );
}

// audit: PIC-010
// audit: PIC-023
#[test]
fn unsupported_pictures_are_out_of_range() {
    let widths = [(11, 6), (11, 6), (12, 7), (5, 3), (9, 5), (1, 1), (28, 15)];
    for (digits, bytes) in widths {
        assert_eq!(packed::byte_width(digits), Ok(bytes), "{digits} digits");
    }
    for digits in [0, 29, u32::MAX] {
        assert_eq!(packed::byte_width(digits), Err(ErrorKind::OutOfRange));
        assert_eq!(
            packed::decode(&[0x00, 0x0C], digits, 0),
            Err(ErrorKind::OutOfRange)
        );
    }
    assert_eq!(
        packed::decode(&[0x00, 0x0C], 2, 3),
        Err(ErrorKind::OutOfRange)
    );
    for (int_digits, scale) in [(0, 0), (27, 2), (u32::MAX, 1)] {
        let result = zoned::decode_signed(b"00000{", int_digits, scale);
        assert_eq!(result, Err(ErrorKind::OutOfRange));
    }
    let mut widest = vec![b'9'; 27];
    widest.push(b'R');
    let value = zoned::decode_signed(&widest, 28, 0).expect("28 digits fit");
    assert_eq!(
        encoded(|out| zoned::encode_signed(value, 28, 0, out)),
        widest
    );
    let nines = [b'9'; 18];
    let key = PicU::<18>::decode(&nines).expect("18 digits fit");
    let widest_key: u64 = key.get();
    assert_eq!(widest_key, 999_999_999_999_999_999);
    let mut signed_nines = nines.to_vec();
    signed_nines[17] = b'R';
    let signed = zoned::decode_pic_s::<18>(&signed_nines).expect("18 digits fit");
    let widest_signed: i64 = signed.get();
    assert_eq!(widest_signed, -999_999_999_999_999_999);
    assert_eq!(PicS::<18>::MAX, 999_999_999_999_999_999);
}

#[test]
fn count_arithmetic_stays_within_four_digits() {
    let top = Count::new(Count::MAX);
    assert_eq!(Count::new(9998).add_one(), Ok(top));
    assert_eq!(top.add_one(), Err(ErrorKind::OutOfRange));
    assert_eq!(Count::new(i16::MAX).add_one(), Err(ErrorKind::OutOfRange));
    assert_eq!(Count::new(-9999).sub_one(), Err(ErrorKind::OutOfRange));
    assert_eq!(Count::new(i16::MIN).sub_one(), Err(ErrorKind::OutOfRange));
    assert_eq!(Count::ZERO.sub_one().map(i16::from), Ok(-1));
    assert_eq!(Count::from(30_000).get(), 30_000);
    assert_eq!(Count::default(), Count::ZERO);
    // A halfword beyond four digits is still a valid field (PIC-011).
    assert_eq!(binary::decode_count(&[0x75, 0x30]), Ok(Count::new(30_000)));
}

// audit: PIC-016
#[test]
fn dates_in_no_known_layout_are_typed_errors() {
    let cases: [&[u8; 10]; 12] = [
        b"2O22-01-01",
        b"2022-0A-01",
        b"2022-01-0B",
        b"2022/01/01",
        b"2022-02-30",
        b"2022-1-01 ",
        b"22-01-01  ",
        b"2022-01-  ",
        b"     01-01",
        b"2022-13-01",
        b"2022-00-10",
        b"20220101  ",
    ];
    for text in cases {
        assert_kind(PicDate::decode(text), ErrorKind::InvalidDate, text);
    }
    for year in [-1, 10_000] {
        let date = NaiveDate::from_ymd_opt(year, 1, 1).expect("chrono holds the year");
        assert_eq!(PicDate::new(Some(date)), Err(ErrorKind::OutOfRange));
    }
    let leap = NaiveDate::from_ymd_opt(2024, 2, 29);
    assert_eq!(PicDate::decode(b"2024-02-29").map(PicDate::value), Ok(leap));
}

// audit: PIC-021
#[test]
fn timestamps_in_no_known_layout_are_typed_errors() {
    let unknown: [&[u8; 26]; 3] = [
        b"2022-06-10T19:27:53.000000",
        b"2022-06-10.19.27.53.000000",
        b"2022-06-10/19:27:53.000000",
    ];
    for text in unknown {
        assert_kind(
            PicTimestamp::decode(text),
            ErrorKind::UnknownTimestampLayout,
            text,
        );
    }
    let invalid: [&[u8; 26]; 13] = [
        b"2022-06-10 25:61:00.000000",
        b"2022-06-10 1X:27:53.000000",
        b"2022-06-10 19:2X:53.000000",
        b"2022-06-10 19:27:5X.000000",
        b"2022-06-10 19:27:53.00000X",
        b"2O22-06-10 19:27:53.000000",
        b"2022-06-10       x        ",
        b"2022-06-10 23:59:60.000000",
        b"2022-06-10 19:27:53       ",
        b"2022-06-10 19.27.53.000000",
        b"2022-06-10-19:27:53.000000",
        b"2022-02-30-19.27.53.000000",
        b"2022-02-30                ",
    ];
    for text in invalid {
        assert_kind(
            PicTimestamp::decode(text),
            ErrorKind::InvalidTimestamp,
            text,
        );
    }
}

// audit: PIC-020
#[test]
fn timestamp_values_their_layout_cannot_carry_are_refused() {
    let day = NaiveDate::from_ymd_opt(2022, 7, 18).expect("a valid date");
    let noon = day.and_hms_opt(12, 0, 0).expect("a valid time");
    assert_eq!(
        CobolTimestamp::new(noon, TsForm::ScreenDate),
        Err(ErrorKind::InvalidTimestamp)
    );
    let nanos = day.and_hms_nano_opt(12, 0, 0, 1).expect("a valid time");
    assert_eq!(
        CobolTimestamp::new(nanos, TsForm::Iso),
        Err(ErrorKind::OutOfRange)
    );
    let leap = day
        .and_hms_micro_opt(23, 59, 59, 1_500_000)
        .expect("chrono's leap second");
    assert_eq!(
        CobolTimestamp::new(leap, TsForm::Db2),
        Err(ErrorKind::OutOfRange)
    );
    let far = NaiveDate::from_ymd_opt(10_000, 1, 1).map(|date| date.and_time(NaiveTime::MIN));
    let far = far.expect("chrono holds the year");
    assert_eq!(
        CobolTimestamp::new(far, TsForm::Iso),
        Err(ErrorKind::OutOfRange)
    );
    let midnight = day.and_time(NaiveTime::MIN);
    let screen = CobolTimestamp::new(midnight, TsForm::ScreenDate).expect("midnight");
    assert_eq!(
        encoded(|out| PicTimestamp::new(Some(screen)).encode_into(out)),
        b"2022-07-18                "
    );
}

/// `len` bytes of `~` and `#`, which no error text holds, so even a one-byte input of a
/// width-mismatch case cannot match the lengths an error prints.
fn marker_bytes(len: usize) -> Vec<u8> {
    b"~#".iter().copied().cycle().take(len).collect()
}

/// Asserts that every input one byte shorter or longer than `expected` fails `decode` with
/// `WidthMismatch` naming both lengths.
fn assert_width_checked<T>(expected: usize, decode: impl Fn(&[u8]) -> Result<T, ErrorKind>) {
    for actual in [expected - 1, expected + 1] {
        let input = marker_bytes(actual);
        assert_kind(
            decode(&input),
            ErrorKind::WidthMismatch { expected, actual },
            &input,
        );
    }
}

// audit: PIC-001
// audit: PIC-003
// audit: PIC-015
// audit: PIC-021
#[test]
fn every_decoder_checks_its_width() {
    assert_width_checked(16, PicX::<16>::decode);
    assert_width_checked(11, PicU::<11>::decode);
    assert_width_checked(9, zoned::decode_pic_s::<9>);
    assert_width_checked(5, packed::decode_pic_s::<9>);
    assert_width_checked(12, |bytes| zoned::decode_signed(bytes, 10, 2));
    assert_width_checked(7, |bytes| packed::decode(bytes, 12, 2));
    assert_width_checked(2, binary::decode_count);
    assert_width_checked(10, PicDate::decode);
    assert_width_checked(26, PicTimestamp::decode);
    assert_kind(
        PicX::<8>::from_text("TOO-LONG-TEXT"),
        ErrorKind::WidthMismatch {
            expected: 8,
            actual: 13,
        },
        b"TOO-LONG-TEXT",
    );
}

#[test]
fn reader_failures_name_the_field_and_leave_the_reader_unchanged() -> TestResult {
    let bytes = *b"ABCD0042X";
    assert_error(
        RecordReader::new(&bytes, 10),
        (
            "record",
            0,
            ErrorKind::WidthMismatch {
                expected: 10,
                actual: 9,
            },
        ),
        &bytes,
    );
    let mut r = RecordReader::new(&bytes, bytes.len())?;
    assert_eq!(r.pic_x::<4>("TAG-CODE")?.as_bytes(), b"ABCD");
    assert_error(
        r.pic_u::<5>("TAG-SEQ"),
        ("TAG-SEQ", 4, ErrorKind::NonDigit),
        &bytes,
    );
    assert_eq!(r.offset(), 4, "a failed read consumes nothing");
    let short = ErrorKind::WidthMismatch {
        expected: 6,
        actual: 5,
    };
    assert_error(r.take("TAG-REST", 6), ("TAG-REST", 4, short), &bytes);
    assert_error(
        r.zoned("TAG-AMOUNT", u32::MAX, 1),
        ("TAG-AMOUNT", 4, ErrorKind::OutOfRange),
        &bytes,
    );
    assert_error(
        r.packed("TAG-PACKED", 0, 0),
        ("TAG-PACKED", 4, ErrorKind::OutOfRange),
        &bytes,
    );
    assert_eq!(r.take("TAG-SEQ", 4)?, b"0042");
    let debug = format!("{r:?}");
    assert!(
        debug.contains("offset: 8") && !debug.contains("0042"),
        "{debug}"
    );
    assert_error(r.finish(), ("record", 8, ErrorKind::TrailingBytes), &bytes);
    Ok(())
}

/// The error a decoder returns for an input of `kind`'s class, or `kind` itself when none is
/// returned. The exhaustive `match` stops compiling when a variant is added without a case.
fn reached_by_a_decoder(kind: ErrorKind) -> Result<ErrorKind, ErrorKind> {
    let reached = match kind {
        ErrorKind::WidthMismatch { .. } => PicX::<6>::decode(b"WIDTH").err(),
        ErrorKind::NonDigit => PicU::<6>::decode(b"12X456").err(),
        ErrorKind::InvalidOverpunch => zoned::decode_signed(b"00042#", 4, 2).err(),
        ErrorKind::PlainDigitSign => zoned::decode_signed(b"000420", 4, 2).err(),
        ErrorKind::NegativeZero => zoned::decode_signed(b"00000}", 4, 2).err(),
        ErrorKind::InvalidPackedDigit => packed::decode(&[0xAB, 0x0C], 3, 0).err(),
        ErrorKind::InvalidPackedSign => packed::decode(&[0x12, 0x3F], 3, 0).err(),
        ErrorKind::NonZeroPadNibble => packed::decode(&[0x12, 0x3C], 2, 0).err(),
        ErrorKind::OutOfRange => PicU::<3>::new(1_000).err(),
        ErrorKind::ShortBodyNotFiller { .. } => {
            Framed::<XrefShaped>::decode(Line::new(b"98765432109876", Terminator::Lf))
                .err()
                .map(|err| err.kind)
        }
        ErrorKind::InvalidDate => PicDate::decode(b"2022-02-30").err(),
        ErrorKind::UnknownTimestampLayout => {
            PicTimestamp::decode(b"2022-06-10T19:27:53.000000").err()
        }
        ErrorKind::InvalidTimestamp => PicTimestamp::decode(b"2022-06-10 25:61:00.000000").err(),
        ErrorKind::UnknownFormCode => TsForm::from_code('X').err(),
        ErrorKind::TrailingBytes => RecordReader::new(b"ABCDEF", 6)
            .ok()
            .and_then(|r| r.finish().err())
            .map(|err| err.kind),
        ErrorKind::NonPrintable => PicX::<6>::decode(b"AB\tCDE").err(),
        ErrorKind::NonBlankTail => XrefShaped::decode(&[b'0'; 50]).ok().and_then(|record| {
            let framing = LineFraming {
                terminator: Terminator::Lf,
                body_len: 36,
            };
            Framed { record, framing }
                .try_encode()
                .err()
                .map(|err| err.kind)
        }),
    };
    reached.ok_or(kind)
}

// audit: PIC-024
#[test]
fn every_error_class_renders_without_its_input() {
    let all = [
        ErrorKind::WidthMismatch {
            expected: 6,
            actual: 5,
        },
        ErrorKind::NonDigit,
        ErrorKind::InvalidOverpunch,
        ErrorKind::PlainDigitSign,
        ErrorKind::NegativeZero,
        ErrorKind::InvalidPackedDigit,
        ErrorKind::InvalidPackedSign,
        ErrorKind::NonZeroPadNibble,
        ErrorKind::OutOfRange,
        ErrorKind::ShortBodyNotFiller {
            expected: 50,
            actual: 14,
        },
        ErrorKind::InvalidDate,
        ErrorKind::UnknownTimestampLayout,
        ErrorKind::InvalidTimestamp,
        ErrorKind::UnknownFormCode,
        ErrorKind::TrailingBytes,
        ErrorKind::NonPrintable,
        ErrorKind::NonBlankTail,
    ];
    let mut phrases = BTreeSet::new();
    for kind in all {
        assert_eq!(
            reached_by_a_decoder(kind),
            Ok(kind),
            "{kind:?} is reachable"
        );
        let text = kind.to_string();
        assert!(!text.is_empty());
        phrases.insert(text);
        let error = Error::new("XREF-CARD-NUM", 0, 16, kind);
        let rendered = error.to_string();
        assert!(
            rendered.starts_with("XREF-CARD-NUM (offset 0, width 16): "),
            "{rendered}"
        );
        assert_no_echo(&error, b"9876543210987654");
        let boxed: Box<dyn std::error::Error> = Box::new(error);
        assert!(boxed.source().is_none());
    }
    assert_eq!(phrases.len(), all.len(), "every class has its own phrase");
}

// audit: PIC-024
#[test]
fn cursors_and_splits_never_show_record_bytes() -> TestResult {
    let record = XrefShaped::decode(&[XREF_BODY.as_slice(), &[b' '; 14]].concat())?;
    let card = "9876543210987654";
    let reader_bytes = record.encode();
    let reader = RecordReader::new(&reader_bytes, XrefShaped::WIDTH)?;
    let mut writer = RecordWriter::with_capacity(XrefShaped::WIDTH);
    writer.pic_x(&record.card_num);
    let split: Lines<'_> = lines(XREF_BODY);
    for shown in [
        format!("{reader:?}"),
        format!("{writer:?}"),
        format!("{split:?}"),
        format!("{record:?}"),
    ] {
        assert!(!shown.contains(card) && !shown.contains("98765"), "{shown}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Numeric-edited pictures
// ---------------------------------------------------------------------------------------------

/// Every numeric-edited picture.
const PICTURES: [EditedPicture; 13] = [
    EditedPicture::MinusZ10Dot99,
    EditedPicture::Plus9x10Dot99,
    EditedPicture::MinusZ8Dot99,
    EditedPicture::MinusZ5Dot99,
    EditedPicture::Plus9x8Dot99,
    EditedPicture::PlusZCommas9Dot99,
    EditedPicture::Z9Dot99Minus,
    EditedPicture::Nines9Dot99Minus,
    EditedPicture::MinusZCommas9DotZZ,
    EditedPicture::PlusZCommas9DotZZ,
    EditedPicture::FloatMinus5,
    EditedPicture::Plus9x6,
    EditedPicture::Plus9x9,
];

/// `(value, edited field)` known answers of one picture.
type Answers = &'static [(&'static str, &'static str)];

/// `PIC -zzzzzzzzz9.99`, `WS-APPROVED-AMT-DIS` (`COPAUA0C.cbl:66`): the CP00 reply amount (E03).
const MINUS_Z10_DOT_99: Answers = &[
    ("100.00", "        100.00"),
    ("-5.00", "-         5.00"),
    ("0", "          0.00"),
    ("9999999999.99", " 9999999999.99"),
    ("12345678901.23", " 2345678901.23"),
    ("0.999", "          0.99"),
];

/// `PIC +9(10).99`, `PA-RL-APPROVED-AMT` (`CCPAURLY.cpy:24`).
const PLUS_9X10_DOT_99: Answers = &[
    ("100.00", "+0000000100.00"),
    ("-5.00", "-0000000005.00"),
    ("0", "+0000000000.00"),
    ("9999999999.99", "+9999999999.99"),
    ("-12345678901.23", "-2345678901.23"),
    ("0.999", "+0000000000.99"),
];

/// `PIC -zzzzzzz9.99`.
const MINUS_Z8_DOT_99: Answers = &[
    ("0", "        0.00"),
    ("12.34", "       12.34"),
    ("-12.34", "-      12.34"),
    ("99999999.99", " 99999999.99"),
    ("123456789.01", " 23456789.01"),
    ("-0.999", "-       0.99"),
];

/// `PIC -zzzz9.99`.
const MINUS_Z5_DOT_99: Answers = &[
    ("0", "     0.00"),
    ("7.05", "     7.05"),
    ("-7.05", "-    7.05"),
    ("99999.99", " 99999.99"),
    ("123456.78", " 23456.78"),
    ("0.999", "     0.99"),
];

/// `PIC +99999999.99`.
const PLUS_9X8_DOT_99: Answers = &[
    ("0", "+00000000.00"),
    ("504.77", "+00000504.77"),
    ("-919.00", "-00000919.00"),
    ("99999999.99", "+99999999.99"),
    ("123456789.01", "+23456789.01"),
    ("0.999", "+00000000.99"),
];

/// `PIC +ZZZ,ZZZ,ZZZ.99`: commas suppressed with the leading zeros.
const PLUS_Z_COMMAS_9_DOT_99: Answers = &[
    ("0", "+           .00"),
    ("1234.56", "+      1,234.56"),
    ("-1234.56", "-      1,234.56"),
    ("1000.00", "+      1,000.00"),
    ("999999999.99", "+999,999,999.99"),
    ("1234567890.12", "+234,567,890.12"),
    ("0.999", "+           .99"),
];

/// `PIC Z(9).99-`: a trailing sign, a space for zero and positive values.
const Z9_DOT_99_MINUS: Answers = &[
    ("0", "         .00 "),
    ("5.00", "        5.00 "),
    ("-5.00", "        5.00-"),
    ("999999999.99", "999999999.99 "),
    ("-1234567890.12", "234567890.12-"),
    ("0.999", "         .99 "),
];

/// `PIC 9(9).99-`.
const NINES_9_DOT_99_MINUS: Answers = &[
    ("0", "000000000.00 "),
    ("1945.87", "000001945.87 "),
    ("-125.88", "000000125.88-"),
    ("999999999.99", "999999999.99 "),
    ("1234567890.12", "234567890.12 "),
    ("-0.999", "000000000.99-"),
];

/// `PIC -ZZZ,ZZZ,ZZZ.ZZ`: every digit position is `Z`, so zero blanks the whole field.
const MINUS_Z_COMMAS_9_DOT_ZZ: Answers = &[
    ("0", "               "),
    ("183.88", "         183.88"),
    ("-47.88", "-         47.88"),
    ("-1234567.89", "-  1,234,567.89"),
    ("999999999.99", " 999,999,999.99"),
    ("-1234567890.12", "-234,567,890.12"),
    ("0.999", "            .99"),
    ("-0.004", "               "),
];

/// `PIC +ZZZ,ZZZ,ZZZ.ZZ`: zero, and a value whose kept digits are all zero, blank the field.
const PLUS_Z_COMMAS_9_DOT_ZZ: Answers = &[
    ("0", "               "),
    ("1453.87", "+      1,453.87"),
    ("-132.88", "-        132.88"),
    ("999999999.99", "+999,999,999.99"),
    ("1234567890.12", "+234,567,890.12"),
    ("1000000000.00", "               "),
    ("0.999", "+           .99"),
];

/// `PIC ----9`: the floating sign lands left of the first printed digit.
const FLOAT_MINUS_5: Answers = &[
    ("0", "    0"),
    ("100", "  100"),
    ("-100", " -100"),
    ("-5", "   -5"),
    ("9999", " 9999"),
    ("-9999", "-9999"),
    ("12345", " 2345"),
    ("-99.9", "  -99"),
];

/// `PIC +9(06)`.
const PLUS_9X6: Answers = &[
    ("0", "+000000"),
    ("100", "+000100"),
    ("-100", "-000100"),
    ("999999", "+999999"),
    ("-1234567", "-234567"),
    ("0.999", "+000000"),
];

/// `PIC +9(09)`.
const PLUS_9X9: Answers = &[
    ("0", "+000000000"),
    ("12345", "+000012345"),
    ("-803", "-000000803"),
    ("999999999", "+999999999"),
    ("1234567890", "+234567890"),
    ("-0.999", "+000000000"),
];

/// The width, scale and known answers of `picture` under the IBM editing rules: zero, a
/// positive and a negative value, the largest value, a value one integer digit too wide (its
/// high-order digit dropped) and a value finer than the scale (truncated, never rounded), plus
/// zero blanking and comma suppression where the picture has them. The exhaustive `match` stops
/// compiling when a picture is added without answers.
fn known_answers(picture: EditedPicture) -> (usize, u32, Answers) {
    match picture {
        EditedPicture::MinusZ10Dot99 => (14, 2, MINUS_Z10_DOT_99),
        EditedPicture::Plus9x10Dot99 => (14, 2, PLUS_9X10_DOT_99),
        EditedPicture::MinusZ8Dot99 => (12, 2, MINUS_Z8_DOT_99),
        EditedPicture::MinusZ5Dot99 => (9, 2, MINUS_Z5_DOT_99),
        EditedPicture::Plus9x8Dot99 => (12, 2, PLUS_9X8_DOT_99),
        EditedPicture::PlusZCommas9Dot99 => (15, 2, PLUS_Z_COMMAS_9_DOT_99),
        EditedPicture::Z9Dot99Minus => (13, 2, Z9_DOT_99_MINUS),
        EditedPicture::Nines9Dot99Minus => (13, 2, NINES_9_DOT_99_MINUS),
        EditedPicture::MinusZCommas9DotZZ => (15, 2, MINUS_Z_COMMAS_9_DOT_ZZ),
        EditedPicture::PlusZCommas9DotZZ => (15, 2, PLUS_Z_COMMAS_9_DOT_ZZ),
        EditedPicture::FloatMinus5 => (5, 0, FLOAT_MINUS_5),
        EditedPicture::Plus9x6 => (7, 0, PLUS_9X6),
        EditedPicture::Plus9x9 => (10, 0, PLUS_9X9),
    }
}

// audit: PIC-022
#[test]
fn edited_pictures_give_their_known_answers() -> TestResult {
    let mut seen: Vec<EditedPicture> = Vec::new();
    for picture in PICTURES {
        assert!(!seen.contains(&picture), "{picture:?} listed twice");
        seen.push(picture);
        let (width, scale, answers) = known_answers(picture);
        assert_eq!(
            (picture.width(), picture.scale()),
            (width, scale),
            "{picture:?}"
        );
        assert!(answers.len() >= 6, "{picture:?}");
        for &(value, expected) in answers {
            let edited = picture.format(Decimal::from_str_exact(value)?);
            assert_eq!(edited, expected, "{picture:?} {value}");
            assert_eq!(edited.len(), width, "{picture:?} {value}");
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    // audit: PIC-022
    #[test]
    fn edited_pictures_keep_their_width_for_any_value(
        mantissa in -(10_i128.pow(20) - 1)..10_i128.pow(20),
        scale in 0_u32..=6,
    ) {
        let value = Decimal::try_from_i128_with_scale(mantissa, scale)
            .map_err(|err| TestCaseError::fail(err.to_string()))?;
        for picture in PICTURES {
            let edited = picture.format(value);
            prop_assert_eq!(edited.len(), picture.width());
            prop_assert!(edited.is_ascii());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// COBOL MOVE truncation in the encoders, and the alphanumeric helpers
// ---------------------------------------------------------------------------------------------

#[test]
fn encoders_store_by_the_move_rule() -> TestResult {
    // High-order digits dropped, fraction truncated toward zero, sign kept (AAP 0.3.3.2).
    let wide = Decimal::from_str_exact("12345678901.239")?;
    assert_eq!(
        encoded(|out| zoned::encode_signed(wide, 9, 2, out)),
        b"3456789012C"
    );
    assert_eq!(
        encoded(|out| zoned::encode_signed(-wide, 9, 2, out)),
        b"3456789012L"
    );
    let whole = Decimal::new(5, 0);
    assert_eq!(
        encoded(|out| zoned::encode_signed(whole, 9, 2, out)),
        b"0000000050{"
    );
    let below_a_cent = Decimal::from_str_exact("-0.004")?;
    assert_eq!(
        encoded(|out| zoned::encode_signed(below_a_cent, 9, 2, out)),
        b"0000000000{",
        "no digit survives to carry the sign"
    );
    let packed_wide = Decimal::from_str_exact("1234567890123.456")?;
    assert_eq!(
        encoded(|out| packed::encode(packed_wide, 11, 2, out)),
        [0x56, 0x78, 0x90, 0x12, 0x34, 0x5C]
    );
    assert_eq!(
        encoded(|out| packed::encode(-packed_wide, 11, 2, out)),
        [0x56, 0x78, 0x90, 0x12, 0x34, 0x5D]
    );
    assert!(
        encoded(|out| zoned::encode_signed(wide, 0, 0, out)).is_empty(),
        "a picture of no positions writes no byte"
    );
    let signed = PicS::<5>::new(-305)?;
    assert_eq!(encoded(|out| zoned::encode_pic_s(signed, out)), b"0030N");
    assert_eq!(
        encoded(|out| packed::encode_pic_s(signed, out)),
        [0x00, 0x30, 0x5D]
    );
    Ok(())
}

#[test]
fn signed_keys_convert_and_invert_within_range() -> TestResult {
    let negative = PicS::<11>::new(-42)?;
    assert_eq!(PicU::<11>::try_from(negative), Err(ErrorKind::OutOfRange));
    assert_eq!(negative.uncomplement(), Err(ErrorKind::OutOfRange));
    assert_eq!(
        PicS::<5>::complement_from(100_000),
        Err(ErrorKind::OutOfRange)
    );
    assert_eq!(PicS::<5>::complement_from(0)?.get(), 99_999);
    assert_eq!(PicS::<9>::complement_from(193_045_123)?.get(), 806_954_876);
    assert_eq!(
        PicS::<11>::new(-100_000_000_000),
        Err(ErrorKind::OutOfRange)
    );
    assert_eq!(PicS::<11>::new(-99_999_999_999)?.get(), -99_999_999_999);
    assert_eq!(PicS::<5>::default().get(), 0);
    Ok(())
}

// audit: PIC-001
// audit: PIC-012
#[test]
fn alphanumeric_values_keep_their_padding() -> TestResult {
    let value = PicX::<8>::from_text("AB C")?;
    assert_eq!(value.as_bytes(), b"AB C    ");
    assert_eq!(value.trim_end(), b"AB C");
    assert_eq!(value.to_text(), "AB C");
    assert!(!value.is_spaces());
    assert_eq!(PicX::<8>::WIDTH, 8);
    assert_eq!(PicX::<4>::moved_from(b"ABCDEFG").as_bytes(), b"ABCD");
    assert_eq!(PicX::<4>::moved_from(b"AB").as_bytes(), b"AB  ");
    assert!(PicX::<4>::default().is_spaces());
    assert_eq!(PicX::<4>::default(), PicX::spaces());
    assert_kind(
        PicX::<8>::from_text("caf\u{e9}  "),
        ErrorKind::NonPrintable,
        b"caf",
    );
    let zeros = PicX::<6>::decode(b"000000")?;
    assert!(
        zeros < PicX::from_array(*b"000001"),
        "values order bytewise"
    );
    assert_eq!(format!("{zeros:?}"), "PicX(\"000000\")");
    Ok(())
}
