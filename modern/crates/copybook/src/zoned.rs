//! Signed numeric representations: DISPLAY with a trailing overpunch sign, packed decimal
//! (`COMP-3`, [`packed`]) and halfword binary (`COMP`, [`binary`]) (AAP 0.3.3.5, codec A).
//!
//! These are representation codecs only. Values are `rust_decimal::Decimal` at the picture's
//! scale, or [`PicS`] and [`Count`](crate::pic::Count) for integral fields; the money and rate
//! algebra over them is `domain::money`.
//!
//! Fields served:
//! - signed DISPLAY `PIC S9(n)V9(m)`: `ACCT-CURR-BAL`, `ACCT-CREDIT-LIMIT`,
//!   `ACCT-CASH-CREDIT-LIMIT`, `ACCT-CURR-CYC-CREDIT`, `ACCT-CURR-CYC-DEBIT` `PIC S9(10)V99`
//!   (`CVACT01Y.cpy:7-9,13-14`), `TRAN-CAT-BAL PIC S9(09)V99` (`CVTRA01Y.cpy:9`),
//!   `DIS-INT-RATE PIC S9(04)V99` (`CVTRA02Y.cpy:9`), `TRAN-AMT` and `DALYTRAN-AMT`
//!   `PIC S9(09)V99` (`CVTRA05Y.cpy:10`, `CVTRA06Y.cpy:10`), and the `PIC S9(10)V99` fields of
//!   the CDRA reply `WS-ACCT-RESPONSE` (`COACCT01.cbl:130-169`);
//! - `COMP-3`: `PA-ACCT-ID PIC S9(11)`, the `PIC S9(09)V99` limits, balances and amounts of
//!   `CIPAUSMY.cpy:19-30`, the inverted keys `PA-AUTH-DATE-9C PIC S9(05)` and
//!   `PA-AUTH-TIME-9C PIC S9(09)`, and `PA-TRANSACTION-AMT`, `PA-APPROVED-AMT`
//!   `PIC S9(10)V99` (`CIPAUDTY.cpy:20-21,34-35`);
//! - `COMP`: `PA-APPROVED-AUTH-CNT`, `PA-DECLINED-AUTH-CNT` `PIC S9(4) COMP`
//!   (`CIPAUSMY.cpy:27-28`).
//!
//! Every decoder is total: for any input of any length and any width parameters it returns a
//! value or a typed [`ErrorKind`] and never panics (Total Parsers Over Untrusted Input). It
//! succeeds only when the matching encoder reproduces the input bytes, so a plain digit in the
//! sign position, a negative zero, or an unexpected sign, digit or pad nibble is an error, never
//! a normalised value (R7; PIC-005 to PIC-010). Errors carry no input bytes (Cardholder Data
//! Redaction).
//!
//! Encoders are infallible and apply the COBOL store rule, which no source in the repository
//! modifies with `ROUNDED` or `ON SIZE ERROR` (AAP 0.3.3.2): the fraction is truncated toward
//! zero at the picture's scale and integer digits beyond its width are dropped.

use rust_decimal::Decimal;

use crate::error::ErrorKind;
use crate::pic::{PicS, digit_count, parse_digits};

/// Most decimal digits a field may declare: a `Decimal` holds every value of up to 28 digits at
/// any scale up to 28 (its 96-bit mantissa exceeds 10^28).
const MAX_DIGITS: u32 = 28;

/// Overpunch sign bytes of a zero or positive last digit, indexed by digit: `{` is +0, `A`–`I`
/// are +1 to +9 (PIC-005).
const POSITIVE_OVERPUNCH: [u8; 10] = *b"{ABCDEFGHI";

/// Overpunch sign bytes of a negative last digit, indexed by digit: `}` is −0, `J`–`R` are −1 to
/// −9 (PIC-005).
const NEGATIVE_OVERPUNCH: [u8; 10] = *b"}JKLMNOPQR";

/// A value laid out for a receiving field: its sign and one digit value (0–9) per digit
/// position, most significant first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Digits {
    /// Whether the stored value is negative; never set when every digit is 0.
    pub negative: bool,
    /// Digit values 0–9, most significant first, exactly `int_digits + scale` of them.
    pub digits: Vec<u8>,
}

/// `value` as a receiving field of `int_digits` integer and `scale` fraction positions holds it
/// after a COBOL `MOVE` or arithmetic store without `ROUNDED`.
///
/// The digits are generated from the right, so the mantissa is divided, never multiplied, and no
/// width can overflow. A negative value whose kept digits are all 0 is stored as positive zero:
/// no digit survives to carry the sign, the COBOL result is not fixed by any repository source
/// (`X-ARITH-7` in `modern/audit/arithmetic.md`), and codec A rejects a negative zero (PIC-008,
/// PIC-010).
#[must_use]
pub(crate) fn truncated_digits(value: Decimal, int_digits: u32, scale: u32) -> Digits {
    let positions = int_digits.saturating_add(scale);
    let value_scale = value.scale();
    let mut magnitude = value.mantissa().unsigned_abs();
    let mut zero_fill = 0_u32;
    if scale >= value_scale {
        zero_fill = scale - value_scale;
    } else {
        // TRUNCATION: fraction digits beyond the receiver's scale are dropped toward zero, the
        // COBOL MOVE and store rule without ROUNDED. A `Decimal` scale is at most 28, and 10^28
        // fits `u128`, so the divisor always exists; no divisor would leave every digit 0.
        magnitude = 10_u128
            .checked_pow(value_scale - scale)
            .map_or(0, |divisor| magnitude / divisor);
    }
    let mut digits = Vec::with_capacity(digit_count(positions));
    for position in 0..positions {
        if position < zero_fill {
            digits.push(0);
        } else {
            digits.push(low_digit(magnitude));
            magnitude /= 10;
        }
    }
    // TRUNCATION: whatever `magnitude` still holds lies above the receiver's integer positions
    // and is dropped, the COBOL MOVE rule for a receiver with fewer integer digits.
    digits.reverse();
    let negative = value.is_sign_negative() && digits.iter().any(|&digit| digit != 0);
    Digits { negative, digits }
}

/// The lowest decimal digit of `value`.
fn low_digit(value: u128) -> u8 {
    // TRUNCATION: a remainder modulo 10 is at most 9, which `u8` holds exactly; the fallback is
    // unreachable and only keeps this free of a panic path.
    u8::try_from(value % 10).unwrap_or(0)
}

/// Total digit count `int_digits + scale` of a supported picture.
fn total_digits(int_digits: u32, scale: u32) -> Result<u32, ErrorKind> {
    int_digits
        .checked_add(scale)
        .filter(|total| (1..=MAX_DIGITS).contains(total))
        .ok_or(ErrorKind::OutOfRange)
}

/// The value `±magnitude × 10^−scale`, rejecting a negative zero (PIC-008, PIC-010).
fn signed_decimal(magnitude: u128, negative: bool, scale: u32) -> Result<Decimal, ErrorKind> {
    if negative && magnitude == 0 {
        return Err(ErrorKind::NegativeZero);
    }
    // TRUNCATION: every caller builds `magnitude` from at most `MAX_DIGITS` (28) digits, which
    // `total_digits` and `packed::byte_width` enforce, so it is below 10^28 and `i128` holds it
    // exactly; the error arm is unreachable.
    let unsigned = i128::try_from(magnitude).map_err(|_| ErrorKind::OutOfRange)?;
    let mantissa = if negative {
        unsigned.checked_neg().ok_or(ErrorKind::OutOfRange)?
    } else {
        unsigned
    };
    // TRUNCATION: |mantissa| is below 10^28, which the 96-bit `Decimal` mantissa holds, and every
    // caller's scale is at most its digit count, so at most 28; the error arm is unreachable.
    Decimal::try_from_i128_with_scale(mantissa, scale).map_err(|_| ErrorKind::OutOfRange)
}

/// A scale-0 value as a `PIC S9(N)` field.
fn pic_s_from_decimal<const N: u32>(value: Decimal) -> Result<PicS<N>, ErrorKind> {
    // TRUNCATION: both callers decode at scale 0, so the mantissa is the integer of at most N
    // digits, and N ≤ 18 for every `PicS<N>` that compiles: |mantissa| ≤ 10^18 − 1, below
    // `i64::MAX`, so the error arm is unreachable, and `PicS::new` re-checks the range.
    i64::try_from(value.mantissa())
        .map_err(|_| ErrorKind::OutOfRange)
        .and_then(PicS::new)
}

/// Digit value of `byte` in an overpunch table.
fn table_digit(table: &[u8; 10], byte: u8) -> Option<u128> {
    table
        .iter()
        .zip(0_u128..)
        .find_map(|(&candidate, digit)| (candidate == byte).then_some(digit))
}

/// Last digit and sign carried by an overpunch sign byte (PIC-005, PIC-007, PIC-009).
fn overpunch_digit(byte: u8) -> Result<(u128, bool), ErrorKind> {
    if let Some(digit) = table_digit(&POSITIVE_OVERPUNCH, byte) {
        Ok((digit, false))
    } else if let Some(digit) = table_digit(&NEGATIVE_OVERPUNCH, byte) {
        Ok((digit, true))
    } else if byte.is_ascii_digit() {
        Err(ErrorKind::PlainDigitSign)
    } else {
        Err(ErrorKind::InvalidOverpunch)
    }
}

/// Overpunch sign byte for a last digit and sign (PIC-006).
fn overpunch_byte(digit: u8, negative: bool) -> u8 {
    let table = if negative {
        &NEGATIVE_OVERPUNCH
    } else {
        &POSITIVE_OVERPUNCH
    };
    // `truncated_digits` yields digit values 0–9 only, so the lookup always succeeds; the
    // fallback only keeps this free of a panic path.
    table.get(usize::from(digit)).copied().unwrap_or(b'{')
}

/// Decodes a signed DISPLAY `PIC S9(int_digits)V9(scale)` field with a trailing overpunch sign:
/// `int_digits + scale − 1` ASCII digits, then one byte carrying the last digit and the sign
/// (PIC-005). The result has scale exactly `scale`, so `0000000000{` at `PIC S9(09)V99` is
/// `0.00`.
///
/// # Errors
///
/// - [`ErrorKind::OutOfRange`] when `int_digits + scale` is 0 or above 28 (PIC-023);
/// - [`ErrorKind::WidthMismatch`] when `bytes` is not `int_digits + scale` bytes long;
/// - [`ErrorKind::NonDigit`] for a non-digit among the leading bytes (PIC-009);
/// - [`ErrorKind::PlainDigitSign`] for a plain digit in the sign position (PIC-007);
/// - [`ErrorKind::InvalidOverpunch`] for any other sign byte outside `{`, `}`, `A`–`R`
///   (PIC-009);
/// - [`ErrorKind::NegativeZero`] for an all-zero magnitude signed `}` (PIC-008).
pub fn decode_signed(bytes: &[u8], int_digits: u32, scale: u32) -> Result<Decimal, ErrorKind> {
    let width = digit_count(total_digits(int_digits, scale)?);
    let mismatch = ErrorKind::WidthMismatch {
        expected: width,
        actual: bytes.len(),
    };
    if bytes.len() != width {
        return Err(mismatch);
    }
    let (&sign_byte, leading) = bytes.split_last().ok_or(mismatch)?;
    let high = parse_digits(leading)?;
    let (last, negative) = overpunch_digit(sign_byte)?;
    let magnitude = high
        .checked_mul(10)
        .and_then(|shifted| shifted.checked_add(last))
        .ok_or(ErrorKind::OutOfRange)?;
    signed_decimal(magnitude, negative, scale)
}

/// Appends `value` as a signed DISPLAY `PIC S9(int_digits)V9(scale)` field: `int_digits + scale`
/// bytes, the last one the overpunch of the last digit, `{`/`A`–`I` for zero and positive values
/// and `}`/`J`–`R` for negative ones (PIC-006). Every field [`decode_signed`] accepts re-encodes
/// to the same bytes.
pub fn encode_signed(value: Decimal, int_digits: u32, scale: u32, out: &mut Vec<u8>) {
    // TRUNCATION: a value wider or finer than the picture is stored by the COBOL MOVE rule,
    // fraction truncated toward zero and high-order digits dropped (`truncated_digits`).
    let Digits { negative, digits } = truncated_digits(value, int_digits, scale);
    if let Some((&last, leading)) = digits.split_last() {
        out.extend(leading.iter().map(|&digit| b'0' + digit));
        out.push(overpunch_byte(last, negative));
    }
}

/// Decodes a signed integral DISPLAY `PIC S9(N)` field: [`decode_signed`] at scale 0.
///
/// # Errors
///
/// Those of [`decode_signed`] for `N` integer digits and scale 0.
pub fn decode_pic_s<const N: u32>(bytes: &[u8]) -> Result<PicS<N>, ErrorKind> {
    decode_signed(bytes, N, 0).and_then(pic_s_from_decimal)
}

/// Appends `value` as a signed integral DISPLAY `PIC S9(N)` field, `N` bytes.
pub fn encode_pic_s<const N: u32>(value: PicS<N>, out: &mut Vec<u8>) {
    encode_signed(Decimal::from(value.get()), N, 0, out);
}

/// Packed decimal, `USAGE COMP-3`: two digit nibbles per byte, most significant first, and the
/// sign in the low nibble of the last byte; a field with an even digit count starts with one pad
/// nibble (PIC-010).
pub mod packed {
    use rust_decimal::Decimal;

    use super::{Digits, MAX_DIGITS, pic_s_from_decimal, signed_decimal, truncated_digits};
    use crate::error::ErrorKind;
    use crate::pic::PicS;

    /// Sign nibble of a zero or positive value.
    const POSITIVE_SIGN: u8 = 0x0C;

    /// Sign nibble of a negative value.
    const NEGATIVE_SIGN: u8 = 0x0D;

    /// Bytes a `COMP-3` field of `digits` digits occupies: `digits / 2 + 1`, so `PIC S9(11)` and
    /// `PIC S9(09)V99` take 6, `PIC S9(10)V99` 7, `PIC S9(05)` 3 and `PIC S9(09)` 5
    /// (`CIPAUSMY.cpy:19-30`, `CIPAUDTY.cpy:20-21,34-35`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] when `digits` is 0 or above 28 (PIC-023).
    pub fn byte_width(digits: u32) -> Result<usize, ErrorKind> {
        if (1..=MAX_DIGITS).contains(&digits) {
            // TRUNCATION: `digits` is at most 28 here, so the width is at most 15, which `usize`
            // holds on every target; the error arm is unreachable.
            usize::try_from(digits / 2 + 1).map_err(|_| ErrorKind::OutOfRange)
        } else {
            Err(ErrorKind::OutOfRange)
        }
    }

    /// Decodes a `COMP-3` field of `digits` digits, `scale` of them after the implied point.
    /// The result has scale exactly `scale`.
    ///
    /// # Errors
    ///
    /// - [`ErrorKind::OutOfRange`] when `digits` is 0 or above 28, or `scale` exceeds `digits`;
    /// - [`ErrorKind::WidthMismatch`] when `bytes` is not [`byte_width`] bytes long;
    /// - [`ErrorKind::NonZeroPadNibble`] when the leading pad nibble of an even digit count is
    ///   not 0;
    /// - [`ErrorKind::InvalidPackedDigit`] for a digit nibble above 9;
    /// - [`ErrorKind::InvalidPackedSign`] for a sign nibble other than `C` and `D`, the unsigned
    ///   `F` included;
    /// - [`ErrorKind::NegativeZero`] for an all-zero magnitude signed `D` (all PIC-010).
    pub fn decode(bytes: &[u8], digits: u32, scale: u32) -> Result<Decimal, ErrorKind> {
        let width = byte_width(digits)?;
        if scale > digits {
            return Err(ErrorKind::OutOfRange);
        }
        let mismatch = ErrorKind::WidthMismatch {
            expected: width,
            actual: bytes.len(),
        };
        if bytes.len() != width {
            return Err(mismatch);
        }
        let (&last, leading) = bytes.split_last().ok_or(mismatch)?;
        let mut nibbles = leading
            .iter()
            .flat_map(|&byte| [byte >> 4, byte & 0x0F])
            .chain(std::iter::once(last >> 4));
        if digits % 2 == 0 && nibbles.next() != Some(0) {
            return Err(ErrorKind::NonZeroPadNibble);
        }
        let magnitude = nibbles.try_fold(0_u128, |value, nibble| {
            if nibble > 9 {
                return Err(ErrorKind::InvalidPackedDigit);
            }
            value
                .checked_mul(10)
                .and_then(|shifted| shifted.checked_add(u128::from(nibble)))
                .ok_or(ErrorKind::OutOfRange)
        })?;
        let negative = match last & 0x0F {
            POSITIVE_SIGN => false,
            NEGATIVE_SIGN => true,
            _ => return Err(ErrorKind::InvalidPackedSign),
        };
        signed_decimal(magnitude, negative, scale)
    }

    /// Appends `value` as a `COMP-3` field of `digits` digits, `scale` after the implied point:
    /// pad nibble 0 for an even digit count, sign nibble `C` for zero and positive values and
    /// `D` for negative ones (PIC-010). Every field [`decode`] accepts re-encodes to the same
    /// bytes.
    pub fn encode(value: Decimal, digits: u32, scale: u32, out: &mut Vec<u8>) {
        // TRUNCATION: a value wider or finer than the picture is stored by the COBOL MOVE rule,
        // fraction truncated toward zero and high-order digits dropped (`truncated_digits`).
        let Digits {
            negative,
            digits: values,
        } = truncated_digits(value, digits.saturating_sub(scale), scale);
        let pad = (digits % 2 == 0).then_some(0_u8);
        let sign = if negative {
            NEGATIVE_SIGN
        } else {
            POSITIVE_SIGN
        };
        let mut nibbles = pad.into_iter().chain(values).chain(std::iter::once(sign));
        while let Some(high) = nibbles.next() {
            // The pad nibble makes the nibble count even, so a low nibble always follows; the
            // fallback only keeps this free of a panic path.
            let low = nibbles.next().unwrap_or(0);
            out.push((high << 4) | low);
        }
    }

    /// Decodes a signed integral `COMP-3` `PIC S9(N)` field: [`decode`] at scale 0.
    ///
    /// # Errors
    ///
    /// Those of [`decode`] for `N` digits and scale 0.
    pub fn decode_pic_s<const N: u32>(bytes: &[u8]) -> Result<PicS<N>, ErrorKind> {
        decode(bytes, N, 0).and_then(pic_s_from_decimal)
    }

    /// Appends `value` as a signed integral `COMP-3` `PIC S9(N)` field, [`byte_width`]`(N)`
    /// bytes.
    pub fn encode_pic_s<const N: u32>(value: PicS<N>, out: &mut Vec<u8>) {
        encode(Decimal::from(value.get()), N, 0, out);
    }
}

/// Halfword binary, `USAGE COMP` `PIC S9(4)`: two bytes, big-endian two's complement (PIC-011).
pub mod binary {
    use crate::error::ErrorKind;
    use crate::pic::Count;

    /// Bytes of a `PIC S9(4) COMP` field.
    pub const WIDTH: usize = 2;

    /// Decodes a `PIC S9(4) COMP` count. Every halfword is a valid field: all 65,536 values
    /// decode and re-encode unchanged, as the `SMALLINT` column holds them (PIC-011). The
    /// picture's four digits bound only `Count`'s arithmetic.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] when `bytes` is not 2 bytes long.
    pub fn decode_count(bytes: &[u8]) -> Result<Count, ErrorKind> {
        let field = <[u8; WIDTH]>::try_from(bytes).map_err(|_| ErrorKind::WidthMismatch {
            expected: WIDTH,
            actual: bytes.len(),
        })?;
        Ok(Count::new(i16::from_be_bytes(field)))
    }

    /// Appends `value` as a `PIC S9(4) COMP` field, 2 bytes big-endian.
    pub fn encode_count(value: Count, out: &mut Vec<u8>) {
        out.extend_from_slice(&value.get().to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rust_decimal::Decimal;

    use super::{
        Digits, ErrorKind, PicS, binary, decode_pic_s, decode_signed, encode_pic_s, encode_signed,
        packed, truncated_digits,
    };
    use crate::pic::Count;

    type TestResult = Result<(), Box<dyn Error>>;

    /// The fixture whose amounts use every overpunch sign byte (PIC-005).
    const DAILYTRAN: &[u8] = include_bytes!("../../../../app/data/ASCII/dailytran.txt");

    fn dec(text: &str) -> Result<Decimal, rust_decimal::Error> {
        Decimal::from_str_exact(text)
    }

    fn signed(value: Decimal, int_digits: u32, scale: u32) -> Vec<u8> {
        let mut out = Vec::new();
        encode_signed(value, int_digits, scale, &mut out);
        out
    }

    fn packed_bytes(value: Decimal, digits: u32, scale: u32) -> Vec<u8> {
        let mut out = Vec::new();
        packed::encode(value, digits, scale, &mut out);
        out
    }

    /// Decodes `bytes`, checks the text and scale of the value and that it re-encodes to `bytes`.
    fn display_round_trip(bytes: &[u8], int_digits: u32, scale: u32, text: &str) -> TestResult {
        let value = decode_signed(bytes, int_digits, scale)?;
        assert_eq!(value.to_string(), text);
        assert_eq!(value.scale(), scale);
        assert_eq!(signed(value, int_digits, scale), bytes);
        Ok(())
    }

    fn packed_round_trip(bytes: &[u8], digits: u32, scale: u32, text: &str) -> TestResult {
        let value = packed::decode(bytes, digits, scale)?;
        assert_eq!(value.to_string(), text);
        assert_eq!(value.scale(), scale);
        assert_eq!(packed_bytes(value, digits, scale), bytes);
        Ok(())
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn every_overpunch_byte_decodes_and_re_encodes() -> TestResult {
        let positive = b"{ABCDEFGHI"
            .iter()
            .zip(0_u32..)
            .map(|(&byte, digit)| (byte, digit, ""));
        let negative = b"}JKLMNOPQR"
            .iter()
            .zip(0_u32..)
            .map(|(&byte, digit)| (byte, digit, "-"));
        let mut seen = 0;
        for (sign_byte, digit, minus) in positive.chain(negative) {
            let bytes = [b'0', b'0', b'1', sign_byte];
            display_round_trip(&bytes, 2, 2, &format!("{minus}0.1{digit}"))?;
            seen += 1;
        }
        assert_eq!(seen, 20);
        Ok(())
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn fixture_amounts() -> TestResult {
        // ACCT-CURR-BAL PIC S9(10)V99, acctdata.txt:1
        display_round_trip(b"00000001940{", 10, 2, "194.00")?;
        // DALYTRAN-AMT PIC S9(09)V99, dailytran.txt:1 and :2
        display_round_trip(b"0000005047G", 9, 2, "504.77")?;
        display_round_trip(b"0000009190}", 9, 2, "-919.00")?;
        // TRAN-CAT-BAL PIC S9(09)V99, tcatbal.txt:1: zero keeps its scale
        display_round_trip(b"0000000000{", 9, 2, "0.00")?;
        // DIS-INT-RATE PIC S9(04)V99, discgrp.txt:1
        display_round_trip(b"00150{", 4, 2, "15.00")?;
        Ok(())
    }

    // audit: PIC-005
    // audit: PIC-006
    #[test]
    fn every_dailytran_amount_round_trips() -> TestResult {
        let mut sign_bytes: Vec<u8> = Vec::new();
        let mut count = 0;
        for line in DAILYTRAN.split(|&byte| byte == b'\n') {
            let Some(amount) = line.get(132..143) else {
                assert!(line.is_empty(), "short line of {} bytes", line.len());
                continue;
            };
            let value = decode_signed(amount, 9, 2)?;
            assert_eq!(signed(value, 9, 2), amount);
            if let Some(last) = amount
                .last()
                .copied()
                .filter(|last| !sign_bytes.contains(last))
            {
                sign_bytes.push(last);
            }
            count += 1;
        }
        sign_bytes.sort_unstable();
        assert_eq!(count, 300);
        assert_eq!(sign_bytes, b"ABCDEFGHIJKLMNOPQR{}");
        Ok(())
    }

    #[test]
    fn widest_display_field() -> TestResult {
        let mut nines = vec![b'9'; 27];
        nines.push(b'I');
        display_round_trip(&nines, 26, 2, "99999999999999999999999999.99")?;
        nines.pop();
        nines.push(b'R');
        display_round_trip(&nines, 28, 0, "-9999999999999999999999999999")?;
        Ok(())
    }

    // audit: PIC-007
    // audit: PIC-008
    // audit: PIC-009
    #[test]
    fn display_decode_errors() {
        let cases: [(&[u8], ErrorKind); 9] = [
            (b"00000000001", ErrorKind::PlainDigitSign),
            (b"00000000009", ErrorKind::PlainDigitSign),
            (b"0000000000S", ErrorKind::InvalidOverpunch),
            (b"0000000000 ", ErrorKind::InvalidOverpunch),
            (b"0000000000@", ErrorKind::InvalidOverpunch),
            (b"00000 0000{", ErrorKind::NonDigit),
            (b"+000000000{", ErrorKind::NonDigit),
            (b"000000000A}", ErrorKind::NonDigit),
            (b"0000000000}", ErrorKind::NegativeZero),
        ];
        for (bytes, kind) in cases {
            assert_eq!(
                decode_signed(bytes, 9, 2),
                Err(kind),
                "{} bytes",
                bytes.len()
            );
        }
        let too_short = ErrorKind::WidthMismatch {
            expected: 11,
            actual: 10,
        };
        assert_eq!(decode_signed(b"000000000{", 9, 2), Err(too_short));
        let empty = ErrorKind::WidthMismatch {
            expected: 11,
            actual: 0,
        };
        assert_eq!(decode_signed(b"", 9, 2), Err(empty));
        let too_long = ErrorKind::WidthMismatch {
            expected: 11,
            actual: 12,
        };
        assert_eq!(decode_signed(b"00000000000{", 9, 2), Err(too_long));
    }

    // audit: PIC-023
    #[test]
    fn unsupported_display_widths() -> TestResult {
        for (int_digits, scale) in [
            (0, 0),
            (29, 0),
            (27, 2),
            (0, 29),
            (u32::MAX, 1),
            (1, u32::MAX),
        ] {
            assert_eq!(
                decode_signed(b"0{", int_digits, scale),
                Err(ErrorKind::OutOfRange),
                "{int_digits},{scale}"
            );
        }
        display_round_trip(b"5{", 0, 2, "0.50")
    }

    /// Each case is a COBOL MOVE into a receiver without ROUNDED: the fraction truncates toward
    /// zero and integer digits beyond the receiver are dropped (AAP 0.3.3.2).
    #[test]
    fn encode_truncates_as_cobol_move() -> TestResult {
        let cases = [
            ("123.456", 3, 2, b"1234E".as_slice()),
            ("0.999", 3, 2, b"0009I"),
            ("-0.999", 3, 2, b"0009R"),
            ("12345.67", 2, 2, b"456G"),
            ("-12345.67", 2, 2, b"456P"),
            ("5", 3, 2, b"0050{"),
            ("-12.34", 3, 2, b"0123M"),
            ("7", 1, 0, b"G"),
        ];
        for (text, int_digits, scale, bytes) in cases {
            assert_eq!(signed(dec(text)?, int_digits, scale), bytes, "{text}");
        }
        Ok(())
    }

    /// A negative value whose kept digits are all 0 is stored as positive zero (`X-ARITH-7`), so
    /// codec A never writes the negative zero it rejects (PIC-008).
    // audit: PIC-008
    #[test]
    fn negative_that_truncates_to_zero_encodes_positive() -> TestResult {
        assert_eq!(signed(dec("-0.001")?, 3, 2), b"0000{");
        assert_eq!(signed(dec("-1000.00")?, 2, 2), b"000{");
        assert_eq!(signed(-Decimal::ZERO, 3, 2), b"0000{");
        assert_eq!(packed_bytes(dec("-0.001")?, 5, 2), [0x00, 0x00, 0x0C]);
        Ok(())
    }

    #[test]
    fn zero_width_encodes_nothing() {
        assert_eq!(signed(Decimal::ONE, 0, 0), b"");
    }

    #[test]
    fn truncated_digits_cases() -> TestResult {
        let digits = |value, int_digits, scale| truncated_digits(value, int_digits, scale);
        // value scale below the target: zero-filled on the right
        assert_eq!(
            digits(dec("5")?, 3, 2),
            Digits {
                negative: false,
                digits: vec![0, 0, 5, 0, 0],
            }
        );
        // value scale above the target: truncated toward zero
        assert_eq!(
            digits(dec("-123.456")?, 3, 2),
            Digits {
                negative: true,
                digits: vec![1, 2, 3, 4, 5],
            }
        );
        // equal scales, high-order digits dropped
        assert_eq!(
            digits(dec("98765.43")?, 2, 2),
            Digits {
                negative: false,
                digits: vec![6, 5, 4, 3],
            }
        );
        // no positions at all
        assert_eq!(
            digits(dec("-7")?, 0, 0),
            Digits {
                negative: false,
                digits: vec![],
            }
        );
        Ok(())
    }

    /// Mantissas near the 96-bit `Decimal` limit, at scale 0 and at scale 28, are divided and
    /// never multiplied, so no width overflows.
    #[test]
    fn truncated_digits_near_the_mantissa_limit() -> TestResult {
        let max = truncated_digits(Decimal::MAX, 28, 0);
        assert!(!max.negative);
        let text: String = max
            .digits
            .iter()
            .map(|&digit| char::from(b'0' + digit))
            .collect();
        // Decimal::MAX is 79228162514264337593543950335; the 29th digit is dropped.
        assert_eq!(text, "9228162514264337593543950335");
        let min = truncated_digits(Decimal::MIN, 29, 4);
        assert!(min.negative);
        assert_eq!(min.digits.len(), 33);
        assert_eq!(min.digits.first(), Some(&7));
        assert_eq!(min.digits.iter().rev().take(4).max(), Some(&0));
        // TRUNCATION: the literal is 2^96 − 1, the largest 96-bit mantissa (that of
        // `Decimal::MAX`), so the `i128` fits the mantissa exactly, and 28 is the largest scale
        // `Decimal` supports, so the checked constructor succeeds and the `?` arm is unreachable.
        let fine = Decimal::try_from_i128_with_scale(79_228_162_514_264_337_593_543_950_335, 28)?;
        assert_eq!(signed(fine, 3, 2), b"0079B");
        assert_eq!(truncated_digits(fine, 0, 0).digits, Vec::<u8>::new());
        assert_eq!(signed(fine, 40, 30).len(), 70);
        Ok(())
    }

    #[test]
    fn display_pic_s_range_ends() -> TestResult {
        let cases = [
            (99_999, b"9999I".as_slice()),
            (-99_999, b"9999R"),
            (0, b"0000{"),
            (-10, b"0001}"),
        ];
        for (value, bytes) in cases {
            let decoded = decode_pic_s::<5>(bytes)?;
            assert_eq!(decoded.get(), value);
            let mut out = Vec::new();
            encode_pic_s(decoded, &mut out);
            assert_eq!(out, bytes);
        }
        for value in [PicS::<18>::MAX, -PicS::<18>::MAX, 1, -1] {
            let field = PicS::<18>::new(value)?;
            let mut out = Vec::new();
            encode_pic_s(field, &mut out);
            assert_eq!(out.len(), 18);
            assert_eq!(decode_pic_s::<18>(&out)?, field);
        }
        assert_eq!(decode_pic_s::<5>(b"0000}"), Err(ErrorKind::NegativeZero));
        assert_eq!(
            decode_pic_s::<11>(b"0000{"),
            Err(ErrorKind::WidthMismatch {
                expected: 11,
                actual: 5,
            })
        );
        Ok(())
    }

    #[test]
    fn packed_byte_widths() {
        for (digits, width) in [
            (11, 6),
            (9 + 2, 6),
            (10 + 2, 7),
            (5, 3),
            (9, 5),
            (1, 1),
            (28, 15),
        ] {
            assert_eq!(packed::byte_width(digits), Ok(width), "{digits}");
        }
        for digits in [0, 29, u32::MAX] {
            assert_eq!(packed::byte_width(digits), Err(ErrorKind::OutOfRange));
        }
    }

    // audit: PIC-010
    #[test]
    fn packed_layouts_round_trip() -> TestResult {
        // PA-ACCT-ID PIC S9(11) COMP-3: odd digit count, no pad nibble
        packed_round_trip(&[0x12, 0x34, 0x56, 0x78, 0x90, 0x1C], 11, 0, "12345678901")?;
        packed_round_trip(&[0x12, 0x34, 0x56, 0x78, 0x90, 0x1D], 11, 0, "-12345678901")?;
        // PA-TRANSACTION-AMT PIC S9(10)V99 COMP-3: even digit count, leading pad nibble 0
        packed_round_trip(
            &[0x01, 0x23, 0x45, 0x67, 0x89, 0x01, 0x2C],
            12,
            2,
            "1234567890.12",
        )?;
        // PA-CREDIT-BALANCE PIC S9(09)V99 COMP-3
        packed_round_trip(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x5D], 11, 2, "-0.05")?;
        packed_round_trip(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x0C], 11, 2, "0.00")?;
        // widest supported field: 28 digits, even, so 15 bytes
        let mut widest = vec![0x09];
        widest.extend([0x99; 13]);
        widest.push(0x9C);
        packed_round_trip(&widest, 28, 0, "9999999999999999999999999999")?;
        Ok(())
    }

    #[test]
    fn packed_encode_truncates_as_cobol_move() -> TestResult {
        assert_eq!(packed_bytes(dec("123.456")?, 5, 2), [0x12, 0x34, 0x5C]);
        assert_eq!(packed_bytes(dec("-12345.67")?, 4, 2), [0x04, 0x56, 0x7D]);
        // the high-order digit is dropped and the zero that remains is positive
        assert_eq!(
            packed_bytes(dec("1000000000.00")?, 11, 2),
            [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C]
        );
        Ok(())
    }

    // audit: PIC-010
    #[test]
    fn packed_decode_errors() {
        let cases: [(&[u8], ErrorKind); 9] = [
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x1F],
                ErrorKind::InvalidPackedSign,
            ),
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x1A],
                ErrorKind::InvalidPackedSign,
            ),
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x1B],
                ErrorKind::InvalidPackedSign,
            ),
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x1E],
                ErrorKind::InvalidPackedSign,
            ),
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x15],
                ErrorKind::InvalidPackedSign,
            ),
            (
                &[0x00, 0x00, 0xA0, 0x00, 0x00, 0x1C],
                ErrorKind::InvalidPackedDigit,
            ),
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0xFC],
                ErrorKind::InvalidPackedDigit,
            ),
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00, 0x0D],
                ErrorKind::NegativeZero,
            ),
            (
                &[0x00, 0x00, 0x00, 0x00, 0x00],
                ErrorKind::WidthMismatch {
                    expected: 6,
                    actual: 5,
                },
            ),
        ];
        for (bytes, kind) in cases {
            assert_eq!(
                packed::decode(bytes, 11, 0),
                Err(kind),
                "{} bytes",
                bytes.len()
            );
        }
        // PIC S9(10)V99: the pad nibble of an even digit count must be 0
        let pad = [0x10, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C];
        assert_eq!(
            packed::decode(&pad, 12, 2),
            Err(ErrorKind::NonZeroPadNibble)
        );
        assert_eq!(
            packed::decode(&[], 12, 2),
            Err(ErrorKind::WidthMismatch {
                expected: 7,
                actual: 0,
            })
        );
        for (digits, scale) in [(0, 0), (29, 0), (5, 6)] {
            assert_eq!(
                packed::decode(&[0x0C], digits, scale),
                Err(ErrorKind::OutOfRange)
            );
        }
    }

    // audit: PIC-010
    #[test]
    fn packed_pic_s_keys() -> TestResult {
        // PA-AUTH-DATE-9C PIC S9(05) COMP-3 and PA-AUTH-TIME-9C PIC S9(09) COMP-3
        let date = PicS::<5>::complement_from(22_199)?;
        let mut out = Vec::new();
        packed::encode_pic_s(date, &mut out);
        assert_eq!(out, [0x77, 0x80, 0x0C]);
        assert_eq!(packed::decode_pic_s::<5>(&out)?, date);
        let time = PicS::<9>::new(-PicS::<9>::MAX)?;
        out.clear();
        packed::encode_pic_s(time, &mut out);
        assert_eq!(out, [0x99, 0x99, 0x99, 0x99, 0x9D]);
        assert_eq!(packed::decode_pic_s::<9>(&out)?, time);
        let account = PicS::<11>::new(50)?;
        out.clear();
        packed::encode_pic_s(account, &mut out);
        assert_eq!(packed::decode_pic_s::<11>(&out)?, account);
        let short = out.get(..5).ok_or("account key shorter than 5 bytes")?;
        assert_eq!(
            packed::decode_pic_s::<11>(short),
            Err(ErrorKind::WidthMismatch {
                expected: 6,
                actual: 5,
            })
        );
        Ok(())
    }

    // audit: PIC-011
    #[test]
    fn binary_counts() -> TestResult {
        let cases: [([u8; 2], i16); 5] = [
            ([0x00, 0x05], 5),
            ([0xFF, 0xFB], -5),
            ([0x27, 0x10], 10_000),
            ([0x80, 0x00], i16::MIN),
            ([0x7F, 0xFF], i16::MAX),
        ];
        for (bytes, value) in cases {
            let count = binary::decode_count(&bytes)?;
            assert_eq!(count.get(), value);
            let mut out = Vec::new();
            binary::encode_count(count, &mut out);
            assert_eq!(out, bytes);
        }
        for value in i16::MIN..=i16::MAX {
            let mut out = Vec::new();
            binary::encode_count(Count::new(value), &mut out);
            assert_eq!(binary::decode_count(&out)?.get(), value);
        }
        for bytes in [b"".as_slice(), &[0x01], &[0x00, 0x01, 0x02]] {
            assert_eq!(
                binary::decode_count(bytes),
                Err(ErrorKind::WidthMismatch {
                    expected: binary::WIDTH,
                    actual: bytes.len(),
                })
            );
        }
        Ok(())
    }
}
