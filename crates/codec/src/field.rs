//! Field specifications and the display-format field codecs of the copybook
//! layouts.
//!
//! Each layout lists its fields as [`FieldSpec`]s in copybook order, FILLER
//! included, and reads and writes them with the functions here:
//!
//! - `X(n)` alphanumeric fields and FILLER copy their bytes verbatim
//!   ([`alnum`], [`put_alnum`]). FILLER is kept as raw bytes, because the
//!   `DISCGRP` and `TCATBAL` fillers hold `0` characters, not spaces.
//! - `9(n)` unsigned display numerics hold the digits `0`–`9` only
//!   ([`unsigned`], [`put_unsigned`]).
//! - `S9(n)V9(s)` signed zoned numerics carry their sign as a trailing
//!   overpunch on the last digit ([`signed`], [`put_signed`]): `{ABCDEFGHI` for
//!   +0 to +9 and `}JKLMNOPQR` for −0 to −9. Every signed field of the fixture
//!   corpus uses overpunch (`acctdata`, `tcatbal` and `discgrp` always end in
//!   `{`; `DALYTRAN-AMT` mixes both tables), so the writer always emits
//!   overpunch, and a plain digit in the sign position is a decode error
//!   ([`CodecErrorKind::PlainDigitInSigned`]) and an escalation, never a repair.
//!
//! Packed (`COMP-3`) and binary (`COMP`) usages appear in none of the in-scope
//! record layouts, so no codec exists for them; a layout that needs one is an
//! escalation.
//!
//! # Internal bytes
//!
//! Every codec here works on internal bytes (ASCII extended to ISO-8859-1). An
//! EBCDIC record is translated as a whole by `Charset` before decoding and
//! after encoding. CP037 places `{` and `A`–`I` at `0xC0`–`0xC9` and `}` and
//! `J`–`R` at `0xD0`–`0xD9`, so the EBCDIC sign rule (zone nibble `C` positive,
//! `D` negative, on the last byte) holds by construction, and an EBCDIC last
//! byte `0xF0`–`0xF9` arrives as a plain digit and is rejected.
//!
//! # Negative zero
//!
//! A value that cannot round-trip through its typed form keeps its stored
//! bytes in [`PreservedFields`]. The only such value is a negative-zero
//! overpunch (every digit `0`, sign `}`), because the domain money and rate
//! types have no negative zero. No fixture record holds one, so on the corpus
//! the list is always empty; a non-empty list on fixture data is an escalation
//! for `docs/parity/escalations.md`, never a repair.

use carddemo_domain::numeric::Alnum;

use crate::error::{CodecError, CodecErrorKind};

/// Overpunch glyphs of the positive digits: entry `d` is the last byte of a
/// signed field whose last digit is `d` and whose sign is `+`.
pub const POSITIVE_OVERPUNCH: &[u8; 10] = b"{ABCDEFGHI";

/// Overpunch glyphs of the negative digits: entry `d` is the last byte of a
/// signed field whose last digit is `d` and whose sign is `-`.
pub const NEGATIVE_OVERPUNCH: &[u8; 10] = b"}JKLMNOPQR";

/// The display digits, entry `d` being the byte of digit `d`.
const DIGITS: &[u8; 10] = b"0123456789";

/// The widest numeric field the readers accept. Every value of up to 18 digits
/// is below `10^18`, so it fits an `i64` with its sign.
const MAX_DIGITS: usize = 18;

/// The PIC class of a copybook field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    /// `PIC X(n)`: alphanumeric, `n` bytes copied verbatim.
    Alnum,
    /// `PIC 9(n)`: unsigned display numeric, `n` digits `0`–`9`.
    Unsigned,
    /// `PIC S9(n)V9(scale)`: signed display numeric with a trailing overpunch
    /// sign on its last digit. The decoded value is in units of the last
    /// digit; the layout applies `scale` through the domain constructors.
    Signed {
        /// The number of digits after the implied decimal point (`V99` is 2).
        scale: u8,
    },
    /// `FILLER`: unnamed bytes, kept raw so that the record re-encodes to its
    /// stored bytes.
    Filler,
}

/// One copybook field: its name, position and PIC class within the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    /// The copybook field name, such as `ACCT-CURR-BAL` or `FILLER`.
    pub name: &'static str,
    /// The byte offset of the field from the start of the record.
    pub offset: usize,
    /// The field length in bytes.
    pub len: usize,
    /// The PIC class of the field.
    pub kind: FieldKind,
}

impl FieldSpec {
    /// A field `name` of `len` bytes starting at byte `offset`.
    #[must_use]
    pub const fn new(name: &'static str, offset: usize, len: usize, kind: FieldKind) -> Self {
        Self {
            name,
            offset,
            len,
            kind,
        }
    }

    /// The offset one past the field's last byte: `offset + len`.
    ///
    /// The sum saturates at `usize::MAX`, so a malformed specification yields
    /// an out-of-range window rather than an arithmetic overflow.
    #[must_use]
    pub const fn end(&self) -> usize {
        self.offset.saturating_add(self.len)
    }
}

/// Whether `fields` tile a record of `len` bytes exactly: the first field
/// starts at 0, each next field starts where the previous one ends, and the
/// last ends at `len`. An empty list tiles only a record of length 0.
///
/// Every layout's `FIELDS` satisfies this for its `LEN`, so no record byte is
/// left unread and no byte is read twice.
#[must_use]
pub const fn fields_are_contiguous(fields: &[FieldSpec], len: usize) -> bool {
    let mut next = 0;
    let mut rest = fields;
    while let [field, tail @ ..] = rest {
        if field.offset != next {
            return false;
        }
        next = field.end();
        rest = tail;
    }
    next == len
}

/// The bytes of `spec` within the record `rec`.
///
/// # Errors
///
/// [`CodecErrorKind::Length`] when `rec` is shorter than `spec.end()` bytes.
pub fn field_bytes<'a>(
    layout: &'static str,
    rec: &'a [u8],
    spec: &FieldSpec,
) -> Result<&'a [u8], CodecError> {
    rec.get(spec.offset..spec.end()).ok_or_else(|| {
        field_error(
            layout,
            spec,
            CodecErrorKind::Length {
                expected: spec.end(),
                actual: rec.len(),
            },
        )
    })
}

/// Reads the `X(N)` field `spec` of `rec`, its bytes copied verbatim: no
/// trimming and no normalisation, so trailing spaces and low-values are kept.
/// FILLER is read the same way.
///
/// # Errors
///
/// - [`CodecErrorKind::Length`] when `spec.len` is not `N`, or when `rec` is
///   too short for the field.
/// - [`CodecErrorKind::Value`] carrying the domain `NumericError` text, should
///   `Alnum::from_bytes` reject the bytes.
pub fn alnum<const N: usize>(
    layout: &'static str,
    rec: &[u8],
    spec: &FieldSpec,
) -> Result<Alnum<N>, CodecError> {
    if spec.len != N {
        return Err(field_error(
            layout,
            spec,
            CodecErrorKind::Length {
                expected: N,
                actual: spec.len,
            },
        ));
    }
    let bytes = field_bytes(layout, rec, spec)?;
    Alnum::from_bytes(bytes)
        .map_err(|e| field_error(layout, spec, CodecErrorKind::Value(e.to_string())))
}

/// Reads the unsigned `9(n)` field `spec` of `rec`.
///
/// # Errors
///
/// - [`CodecErrorKind::Length`] when `rec` is too short for the field, or the
///   field is empty.
/// - [`CodecErrorKind::Value`] when the field is wider than 18 digits.
/// - [`CodecErrorKind::NotNumeric`] naming the first byte that is not `0`–`9`.
pub fn unsigned(layout: &'static str, rec: &[u8], spec: &FieldSpec) -> Result<u64, CodecError> {
    let bytes = numeric_bytes(layout, rec, spec)?;
    parse_digits(layout, spec, bytes)
}

/// A decoded signed zoned value: its magnitude and the sign its overpunch
/// carried.
///
/// The magnitude is in units of the field's last digit (cents for `V99`); the
/// layout applies the scale through the domain constructors, such as
/// `Money::from_cents` and `InterestRate::from_hundredths`. The sign is kept
/// apart from the magnitude so that a negative zero stays visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signed {
    /// The digits of the field as a whole number.
    pub magnitude: u64,
    /// Whether the overpunch carried the negative sign.
    pub negative: bool,
}

impl Signed {
    /// The signed value in units of the last digit. A negative zero yields 0.
    ///
    /// A magnitude above `i64::MAX` saturates to `i64::MAX` (or its negation);
    /// decoded values have at most 18 digits and never reach it.
    #[must_use]
    pub fn value(self) -> i64 {
        let magnitude = i64::try_from(self.magnitude).unwrap_or(i64::MAX);
        if self.negative { -magnitude } else { magnitude }
    }

    /// Whether this is a negative zero: every digit `0` with the sign `-`.
    #[must_use]
    pub const fn is_negative_zero(self) -> bool {
        self.negative && self.magnitude == 0
    }
}

/// Reads the signed `S9(n)V9(s)` field `spec` of `rec`, whose sign is the
/// trailing overpunch of its last byte.
///
/// # Errors
///
/// - [`CodecErrorKind::Length`] when `rec` is too short for the field, or the
///   field is empty.
/// - [`CodecErrorKind::Value`] when the field is wider than 18 digits.
/// - [`CodecErrorKind::NotNumeric`] naming the first byte before the last that
///   is not `0`–`9`.
/// - [`CodecErrorKind::PlainDigitInSigned`] when the last byte is a plain
///   digit instead of an overpunch glyph.
/// - [`CodecErrorKind::InvalidSign`] when the last byte is neither.
pub fn signed(layout: &'static str, rec: &[u8], spec: &FieldSpec) -> Result<Signed, CodecError> {
    signed_with_bytes(layout, rec, spec).map(|(value, _)| value)
}

/// Reads the signed field `spec` of `rec` as [`signed`] does, and records its
/// stored bytes in `preserved` when the value is a negative zero, so that
/// [`put_signed_preserving`] can write them back. Layouts with signed fields
/// read them through this function.
///
/// # Errors
///
/// As [`signed`]; nothing is recorded when an error is returned.
pub fn signed_preserving(
    layout: &'static str,
    rec: &[u8],
    spec: &FieldSpec,
    preserved: &mut PreservedFields,
) -> Result<Signed, CodecError> {
    let (value, bytes) = signed_with_bytes(layout, rec, spec)?;
    if value.is_negative_zero() {
        preserved.push(RawField {
            field: spec.name,
            offset: spec.offset,
            bytes: bytes.to_vec(),
        });
    }
    Ok(value)
}

/// The decoded signed value of `spec` and the stored bytes it came from.
fn signed_with_bytes<'a>(
    layout: &'static str,
    rec: &'a [u8],
    spec: &FieldSpec,
) -> Result<(Signed, &'a [u8]), CodecError> {
    let bytes = numeric_bytes(layout, rec, spec)?;
    let Some((&last, body)) = bytes.split_last() else {
        return Err(empty_field(layout, spec));
    };
    let leading = parse_digits(layout, spec, body)?;
    let (digit, negative) = match overpunch_digit(last) {
        Some(found) => found,
        None if last.is_ascii_digit() => {
            return Err(field_error(
                layout,
                spec,
                CodecErrorKind::PlainDigitInSigned { byte: last },
            ));
        }
        None => {
            return Err(field_error(
                layout,
                spec,
                CodecErrorKind::InvalidSign { byte: last },
            ));
        }
    };
    let magnitude = push_digit(leading, digit).ok_or_else(|| too_wide(layout, spec))?;
    Ok((
        Signed {
            magnitude,
            negative,
        },
        bytes,
    ))
}

/// The bytes of a numeric field, checked to be 1 to 18 digits wide.
fn numeric_bytes<'a>(
    layout: &'static str,
    rec: &'a [u8],
    spec: &FieldSpec,
) -> Result<&'a [u8], CodecError> {
    let bytes = field_bytes(layout, rec, spec)?;
    if bytes.is_empty() {
        return Err(empty_field(layout, spec));
    }
    if bytes.len() > MAX_DIGITS {
        return Err(too_wide(layout, spec));
    }
    Ok(bytes)
}

/// The whole number spelled by `digits`, every byte of which must be `0`–`9`.
fn parse_digits(layout: &'static str, spec: &FieldSpec, digits: &[u8]) -> Result<u64, CodecError> {
    digits.iter().try_fold(0, |acc, &byte| {
        let digit = digit_value(byte)
            .ok_or_else(|| field_error(layout, spec, CodecErrorKind::NotNumeric { byte }))?;
        push_digit(acc, digit).ok_or_else(|| too_wide(layout, spec))
    })
}

/// The value of the digit byte `byte`, or `None` when it is not `0`–`9`.
fn digit_value(byte: u8) -> Option<u8> {
    position_in(DIGITS, byte)
}

/// The digit and sign an overpunch glyph stands for, or `None` when `byte` is
/// not one of the 20 glyphs.
fn overpunch_digit(byte: u8) -> Option<(u8, bool)> {
    position_in(POSITIVE_OVERPUNCH, byte)
        .map(|digit| (digit, false))
        .or_else(|| position_in(NEGATIVE_OVERPUNCH, byte).map(|digit| (digit, true)))
}

/// The index of `byte` in the ten-entry `table`, as a digit value.
fn position_in(table: &[u8; 10], byte: u8) -> Option<u8> {
    table
        .iter()
        .zip(0u8..)
        .find_map(|(&entry, digit)| (entry == byte).then_some(digit))
}

/// `acc` with `digit` appended as its new lowest-order digit, or `None` on
/// overflow.
fn push_digit(acc: u64, digit: u8) -> Option<u64> {
    acc.checked_mul(10)?.checked_add(u64::from(digit))
}

/// The error `kind`, located at the field `spec` of `layout`.
fn field_error(layout: &'static str, spec: &FieldSpec, kind: CodecErrorKind) -> CodecError {
    CodecError::new(layout, kind).at_field(spec.name, spec.offset)
}

/// The error for a numeric field of no bytes: a digit is required.
fn empty_field(layout: &'static str, spec: &FieldSpec) -> CodecError {
    field_error(
        layout,
        spec,
        CodecErrorKind::Length {
            expected: 1,
            actual: 0,
        },
    )
}

/// The error for a numeric field wider than [`MAX_DIGITS`].
fn too_wide(layout: &'static str, spec: &FieldSpec) -> CodecError {
    field_error(
        layout,
        spec,
        CodecErrorKind::Value(format!(
            "field of {} digits is wider than {MAX_DIGITS} digits",
            spec.len
        )),
    )
}

/// Writes `bytes` into the field `spec` of the record buffer `out` as a COBOL
/// alphanumeric `MOVE`: left-justified, padded with spaces, cut on the right.
///
/// `out` is the whole internal record buffer; layouts always pass one of their
/// `LEN` bytes. Should the field lie outside `out`, nothing is written.
pub fn put_alnum(out: &mut [u8], spec: &FieldSpec, bytes: &[u8]) {
    let Some(window) = window_mut(out, spec) else {
        return;
    };
    window.fill(b' ');
    for (dst, &src) in window.iter_mut().zip(bytes) {
        *dst = src;
    }
}

/// Writes `value` into the unsigned `9(n)` field `spec` of `out`: its
/// low-order `n` digits, zero-padded on the left. High-order digits that do not
/// fit are dropped, as a COBOL `MOVE` to a narrower `9(n)` drops them.
///
/// Should the field lie outside `out`, nothing is written.
pub fn put_unsigned(out: &mut [u8], spec: &FieldSpec, value: u64) {
    if let Some(window) = window_mut(out, spec) {
        write_digits(window, value);
    }
}

/// Writes `value` into the signed `S9(n)V9(s)` field `spec` of `out`, in units
/// of the field's last digit: the low-order `n + s` digits of its magnitude,
/// zero-padded, the last digit replaced by its overpunch glyph from
/// [`POSITIVE_OVERPUNCH`] (`value >= 0`) or [`NEGATIVE_OVERPUNCH`]
/// (`value < 0`). Zero is written `0…0{`.
///
/// High-order digits that do not fit are dropped and the sign is kept, as in
/// a COBOL `MOVE` to a narrower signed field. Should the field lie outside
/// `out`, nothing is written.
pub fn put_signed(out: &mut [u8], spec: &FieldSpec, value: i64) {
    let Some(window) = window_mut(out, spec) else {
        return;
    };
    write_digits(window, value.unsigned_abs());
    let glyphs = if value < 0 {
        NEGATIVE_OVERPUNCH
    } else {
        POSITIVE_OVERPUNCH
    };
    if let Some(last) = window.last_mut() {
        *last = overpunch_glyph(glyphs, *last);
    }
}

/// Writes `value` as [`put_signed`] does, then restores the stored bytes that
/// `preserved` holds for the field while `value` is still zero, so that a
/// decoded negative zero re-encodes to its original bytes.
pub fn put_signed_preserving(
    out: &mut [u8],
    spec: &FieldSpec,
    value: i64,
    preserved: &PreservedFields,
) {
    put_signed(out, spec, value);
    preserved.restore(out, spec, value == 0);
}

/// The window of `spec` within `out`, or `None` when it lies outside.
fn window_mut<'a>(out: &'a mut [u8], spec: &FieldSpec) -> Option<&'a mut [u8]> {
    out.get_mut(spec.offset..spec.end())
}

/// Fills `window` with the low-order digits of `value`, zero-padded on the
/// left.
fn write_digits(window: &mut [u8], value: u64) {
    let mut rest = value;
    for dst in window.iter_mut().rev() {
        *dst = u8::try_from(rest % 10).map_or(b'0', |digit| b'0' + digit);
        rest /= 10;
    }
}

/// The glyph of `glyphs` for the digit byte `digit`. `digit` always comes from
/// [`write_digits`]; any other byte is returned unchanged.
fn overpunch_glyph(glyphs: &[u8; 10], digit: u8) -> u8 {
    digit_value(digit)
        .and_then(|value| glyphs.get(usize::from(value)))
        .copied()
        .unwrap_or(digit)
}

/// The stored bytes of one field whose value cannot round-trip through its
/// typed form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawField {
    /// The copybook field name.
    pub field: &'static str,
    /// The byte offset of the field within the record.
    pub offset: usize,
    /// The stored bytes, in the internal (ISO-8859-1) character set.
    pub bytes: Vec<u8>,
}

/// The raw fields of one decoded record that its typed fields cannot
/// reproduce: today only negative-zero overpunch values.
///
/// A record decoded from the fixture corpus always has an empty list. A
/// non-empty one on fixture data is an escalation for
/// `docs/parity/escalations.md`, never a repair: the bytes are written back as
/// stored for as long as the typed value is unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreservedFields(Vec<RawField>);

impl PreservedFields {
    /// An empty list.
    #[must_use]
    pub const fn new() -> Self {
        Self(Vec::new())
    }

    /// Records the stored bytes of one field.
    pub fn push(&mut self, raw: RawField) {
        self.0.push(raw);
    }

    /// Whether no field is recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The number of recorded fields.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The recorded fields, in the order they were decoded.
    pub fn iter(&self) -> std::slice::Iter<'_, RawField> {
        self.0.iter()
    }

    /// The field recorded at byte `offset`, if any.
    #[must_use]
    pub fn get(&self, offset: usize) -> Option<&RawField> {
        self.0.iter().find(|raw| raw.offset == offset)
    }

    /// Writes the bytes recorded for `spec.offset` back into the window of
    /// `spec` within `out`, but only while `current_is_zero`: the typed value
    /// still equals what the stored bytes decode to. An edited value is never
    /// overwritten. Nothing is written when no bytes are recorded for the
    /// field, or when their length is not the field's.
    pub fn restore(&self, out: &mut [u8], spec: &FieldSpec, current_is_zero: bool) {
        if !current_is_zero {
            return;
        }
        let (Some(raw), Some(window)) = (self.get(spec.offset), window_mut(out, spec)) else {
            return;
        };
        if raw.bytes.len() == window.len() {
            for (dst, &src) in window.iter_mut().zip(&raw.bytes) {
                *dst = src;
            }
        }
    }
}

impl<'a> IntoIterator for &'a PreservedFields {
    type Item = &'a RawField;
    type IntoIter = std::slice::Iter<'a, RawField>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYOUT: &str = "TESTREC";

    fn spec(name: &'static str, offset: usize, len: usize, kind: FieldKind) -> FieldSpec {
        FieldSpec::new(name, offset, len, kind)
    }

    fn signed_spec(len: usize) -> FieldSpec {
        spec("TEST-AMT", 0, len, FieldKind::Signed { scale: 2 })
    }

    fn encode_signed(len: usize, value: i64) -> Vec<u8> {
        let mut out = vec![b' '; len];
        put_signed(&mut out, &signed_spec(len), value);
        out
    }

    #[test]
    fn spec_end_is_offset_plus_len_and_saturates() {
        assert_eq!(spec("A", 12, 12, FieldKind::Unsigned).end(), 24);
        assert_eq!(spec("A", usize::MAX, 2, FieldKind::Alnum).end(), usize::MAX);
    }

    #[test]
    fn fields_are_contiguous_accepts_only_an_exact_tiling() {
        let a = spec("A", 0, 2, FieldKind::Alnum);
        let b = spec("B", 2, 3, FieldKind::Unsigned);
        let c = spec("C", 5, 5, FieldKind::Filler);
        let cases: [(&[FieldSpec], usize, bool); 8] = [
            (&[a, b, c], 10, true),
            (&[], 0, true),
            (&[], 1, false),
            (&[a, spec("B", 3, 3, FieldKind::Unsigned)], 6, false),
            (&[a, spec("B", 1, 3, FieldKind::Unsigned)], 4, false),
            (&[a, b, c], 11, false),
            (&[a, b, c], 9, false),
            (&[spec("A", 1, 2, FieldKind::Alnum)], 3, false),
        ];
        for (fields, len, expected) in cases {
            assert_eq!(
                fields_are_contiguous(fields, len),
                expected,
                "{fields:?} {len}"
            );
        }
    }

    #[test]
    fn overpunch_glyphs_encode_and_decode_every_digit() {
        let one = signed_spec(1);
        for ((&plus, &minus), digit) in POSITIVE_OVERPUNCH
            .iter()
            .zip(NEGATIVE_OVERPUNCH)
            .zip(0_i64..)
        {
            assert_eq!(encode_signed(1, digit), [plus]);
            let positive = signed(LAYOUT, &[plus], &one).unwrap();
            assert_eq!(positive.value(), digit);
            assert!(!positive.negative);

            let negative = signed(LAYOUT, &[minus], &one).unwrap();
            assert!(negative.negative);
            assert_eq!(negative.value(), -digit);
            if digit == 0 {
                // An `i64` has no negative zero: -0 is written with the `+` glyph.
                assert_eq!(encode_signed(1, -digit), [plus]);
                assert!(negative.is_negative_zero());
            } else {
                assert_eq!(encode_signed(1, -digit), [minus]);
                assert!(!negative.is_negative_zero());
            }
        }
    }

    #[test]
    fn fixture_style_values_decode_and_round_trip() {
        let cases: [(&[u8], u64, bool, i64); 6] = [
            // acctdata record 1 `ACCT-CURR-BAL`: 194.00.
            (b"00000001940{", 19_400, false, 19_400),
            // dailytran record 1 `DALYTRAN-AMT`: 504.77.
            (b"0000005047G", 50_477, false, 50_477),
            // dailytran record 2 `DALYTRAN-AMT`: -919.00.
            (b"0000009190}", 91_900, true, -91_900),
            (b"0000000005N", 55, true, -55),
            // discgrp record 34 `DIS-INT-RATE`: 0.00.
            (b"00000{", 0, false, 0),
            (
                b"99999999999999999I",
                999_999_999_999_999_999,
                false,
                999_999_999_999_999_999,
            ),
        ];
        for (bytes, magnitude, negative, value) in cases {
            let field = signed_spec(bytes.len());
            let decoded = signed(LAYOUT, bytes, &field).unwrap();
            assert_eq!(
                decoded,
                Signed {
                    magnitude,
                    negative
                },
                "{bytes:?}"
            );
            assert_eq!(decoded.value(), value);
            assert!(!decoded.is_negative_zero());
            assert_eq!(encode_signed(bytes.len(), decoded.value()), bytes);
        }
    }

    #[test]
    fn unsigned_reads_digits_and_writes_them_back() {
        let cases: [(&[u8], u64); 4] = [
            (b"00000000001", 1),
            (b"0001", 1),
            (b"9", 9),
            (b"999999999999999999", 999_999_999_999_999_999),
        ];
        for (bytes, value) in cases {
            let field = spec("TEST-ID", 0, bytes.len(), FieldKind::Unsigned);
            assert_eq!(unsigned(LAYOUT, bytes, &field).unwrap(), value);
            let mut out = vec![b' '; bytes.len()];
            put_unsigned(&mut out, &field, value);
            assert_eq!(out, bytes);
        }
    }

    #[test]
    fn decode_errors_name_kind_layout_field_and_offset() {
        use CodecErrorKind::{InvalidSign, NotNumeric, PlainDigitInSigned};

        let amt = spec("TEST-AMT", 2, 4, FieldKind::Signed { scale: 2 });
        let id = spec("TEST-ID", 2, 3, FieldKind::Unsigned);
        let cases: [(&[u8], &FieldSpec, bool, CodecErrorKind); 6] = [
            (b"XX0015", &amt, true, PlainDigitInSigned { byte: b'5' }),
            (b"XX001 ", &amt, true, InvalidSign { byte: b' ' }),
            (b"XX0-1{", &amt, true, NotNumeric { byte: b'-' }),
            (b"XX 01{", &amt, true, NotNumeric { byte: b' ' }),
            (b"XX12A", &id, false, NotNumeric { byte: b'A' }),
            (b"XX1 3", &id, false, NotNumeric { byte: b' ' }),
        ];
        for (rec, field, is_signed, kind) in cases {
            let err = if is_signed {
                signed(LAYOUT, rec, field).unwrap_err()
            } else {
                unsigned(LAYOUT, rec, field).unwrap_err()
            };
            let expected = CodecError::new(LAYOUT, kind).at_field(field.name, 2);
            assert_eq!(err, expected, "{rec:?}");
        }
    }

    #[test]
    fn numeric_width_is_limited_to_eighteen_digits_and_one_byte_minimum() {
        let wide = [b'0'; 19];
        let unsigned_err = unsigned(LAYOUT, &wide, &spec("W", 0, 19, FieldKind::Unsigned));
        let signed_err = signed(LAYOUT, &wide, &signed_spec(19));
        for err in [unsigned_err.unwrap_err(), signed_err.unwrap_err()] {
            assert_eq!(
                err.kind,
                CodecErrorKind::Value("field of 19 digits is wider than 18 digits".to_owned())
            );
        }

        let empty_kind = CodecErrorKind::Length {
            expected: 1,
            actual: 0,
        };
        let empty_unsigned = unsigned(LAYOUT, b"", &spec("E", 0, 0, FieldKind::Unsigned));
        assert_eq!(empty_unsigned.unwrap_err().kind, empty_kind);
        assert_eq!(
            signed(LAYOUT, b"", &signed_spec(0)).unwrap_err().kind,
            empty_kind
        );
    }

    #[test]
    fn readers_reject_a_record_too_short_without_panicking() {
        let field = spec("TEST-FIELD", 3, 4, FieldKind::Alnum);
        let expected = CodecError::new(
            LAYOUT,
            CodecErrorKind::Length {
                expected: 7,
                actual: 5,
            },
        )
        .at_field("TEST-FIELD", 3);
        let rec = b"12345";
        assert_eq!(field_bytes(LAYOUT, rec, &field).unwrap_err(), expected);
        assert_eq!(alnum::<4>(LAYOUT, rec, &field).unwrap_err(), expected);
        assert_eq!(unsigned(LAYOUT, rec, &field).unwrap_err(), expected);
        assert_eq!(signed(LAYOUT, rec, &field).unwrap_err(), expected);
        let huge = spec("TEST-FIELD", usize::MAX, 4, FieldKind::Alnum);
        assert!(field_bytes(LAYOUT, rec, &huge).is_err());
    }

    #[test]
    fn alnum_copies_bytes_verbatim_and_checks_the_width() {
        let rec = b"ID\x00AB  \xE9Z";
        let field = spec("TEST-NAME", 2, 7, FieldKind::Alnum);
        let value = alnum::<7>(LAYOUT, rec, &field).unwrap();
        assert_eq!(value.as_bytes(), b"\x00AB  \xE9Z");
        assert_eq!(field_bytes(LAYOUT, rec, &field).unwrap(), b"\x00AB  \xE9Z");

        let mut out = vec![b'#'; rec.len()];
        put_alnum(&mut out, &field, value.as_bytes());
        assert_eq!(out, b"##\x00AB  \xE9Z");

        let mismatch = alnum::<6>(LAYOUT, rec, &field).unwrap_err();
        let expected_kind = CodecErrorKind::Length {
            expected: 6,
            actual: 7,
        };
        assert_eq!(
            mismatch,
            CodecError::new(LAYOUT, expected_kind).at_field("TEST-NAME", 2)
        );
    }

    #[test]
    fn put_alnum_pads_with_spaces_and_cuts_on_the_right() {
        let field = spec("TEST-NAME", 1, 4, FieldKind::Alnum);
        let cases: [(&[u8], &[u8]); 4] = [
            (b"AB", b"#AB  #"),
            (b"ABCDEF", b"#ABCD#"),
            (b"WXYZ", b"#WXYZ#"),
            (b"", b"#    #"),
        ];
        for (src, expected) in cases {
            let mut out = *b"######";
            put_alnum(&mut out, &field, src);
            assert_eq!(out, expected, "{src:?}");
        }
    }

    #[test]
    fn writers_keep_low_order_digits_and_leave_neighbours_alone() {
        let field = spec("TEST-ID", 1, 4, FieldKind::Unsigned);
        let unsigned_cases: [(u64, &[u8]); 3] =
            [(123_456, b"#3456#"), (7, b"#0007#"), (u64::MAX, b"#1615#")];
        for (value, expected) in unsigned_cases {
            let mut out = *b"######";
            put_unsigned(&mut out, &field, value);
            assert_eq!(out, expected, "{value}");
        }

        let amt = spec("TEST-AMT", 1, 3, FieldKind::Signed { scale: 2 });
        let signed_cases: [(i64, &[u8]); 5] = [
            (12_345, b"#34E#"),
            (-12_345, b"#34N#"),
            (-1_000, b"#00}#"),
            (i64::MIN, b"#80Q#"),
            (0, b"#00{#"),
        ];
        for (value, expected) in signed_cases {
            let mut out = *b"#####";
            put_signed(&mut out, &amt, value);
            assert_eq!(out, expected, "{value}");
        }
    }

    #[test]
    fn writers_ignore_a_field_outside_the_buffer() {
        let outside = spec("TEST-FIELD", 4, 4, FieldKind::Alnum);
        let mut out = *b"#####";
        put_alnum(&mut out, &outside, b"ABCD");
        put_unsigned(&mut out, &outside, 1234);
        put_signed(&mut out, &outside, -1234);
        let mut preserved = PreservedFields::new();
        preserved.push(RawField {
            field: "TEST-FIELD",
            offset: 4,
            bytes: b"000}".to_vec(),
        });
        put_signed_preserving(&mut out, &outside, 0, &preserved);
        assert_eq!(&out, b"#####");
    }

    #[test]
    fn negative_zero_is_preserved_and_restored_only_while_zero() {
        let field = spec("TEST-AMT", 0, 4, FieldKind::Signed { scale: 2 });
        let mut preserved = PreservedFields::new();
        let decoded = signed_preserving(LAYOUT, b"000}", &field, &mut preserved).unwrap();
        assert!(decoded.is_negative_zero());
        assert_eq!(decoded.value(), 0);
        let raw = RawField {
            field: "TEST-AMT",
            offset: 0,
            bytes: b"000}".to_vec(),
        };
        assert_eq!(preserved.len(), 1);
        assert_eq!(preserved.get(0), Some(&raw));
        assert_eq!(preserved.get(1), None);

        let mut out = vec![b' '; 4];
        put_signed_preserving(&mut out, &field, 0, &preserved);
        assert_eq!(out, b"000}");
        put_signed_preserving(&mut out, &field, 55, &preserved);
        assert_eq!(out, b"005E");
        put_signed_preserving(&mut out, &field, 5, &preserved);
        assert_eq!(out, b"000E");

        let mut plain = PreservedFields::new();
        let zero = signed_preserving(LAYOUT, b"000{", &field, &mut plain).unwrap();
        assert!(!zero.is_negative_zero());
        assert!(plain.is_empty());
        let rejected = signed_preserving(LAYOUT, b"0000", &field, &mut plain);
        assert!(rejected.is_err());
        assert!(plain.is_empty());
    }

    #[test]
    fn restore_writes_only_matching_raw_bytes_while_zero() {
        let field = spec("TEST-AMT", 2, 4, FieldKind::Signed { scale: 2 });
        let mut preserved = PreservedFields::new();
        preserved.push(RawField {
            field: "TEST-AMT",
            offset: 2,
            bytes: b"000}".to_vec(),
        });
        preserved.push(RawField {
            field: "TEST-SHORT",
            offset: 0,
            bytes: b"}".to_vec(),
        });

        let mut out = *b"##000{##";
        preserved.restore(&mut out, &field, false);
        assert_eq!(&out, b"##000{##");
        preserved.restore(&mut out, &field, true);
        assert_eq!(&out, b"##000}##");

        let wrong_width = spec("TEST-SHORT", 0, 2, FieldKind::Signed { scale: 0 });
        preserved.restore(&mut out, &wrong_width, true);
        assert_eq!(&out, b"##000}##");

        let mut offsets = Vec::new();
        for raw in &preserved {
            offsets.push(raw.offset);
        }
        assert_eq!(offsets, [2, 0]);
        assert_eq!(preserved.iter().count(), 2);
        assert_eq!(PreservedFields::default(), PreservedFields::new());
    }

    #[test]
    fn signed_value_saturates_beyond_the_i64_range() {
        let cases = [
            (u64::MAX, false, i64::MAX),
            (u64::MAX, true, -i64::MAX),
            (0, true, 0),
            (42, true, -42),
        ];
        for (magnitude, negative, value) in cases {
            assert_eq!(
                Signed {
                    magnitude,
                    negative
                }
                .value(),
                value
            );
        }
    }
}
