//! Property tests of the `carddemo-codec` field codecs and record layouts
//! (AAP 0.5.3.2, "Field codecs" and "Tests"; goal o4).
//!
//! The fixture round trips prove o4 on the 626 stored records. This file
//! proves the codecs behind them on every value their PIC clauses allow:
//!
//! - **Field-level identity.** For `X(n)` and `FILLER`, `9(n)` and
//!   `S9(n)V99`, writing a decoded field reproduces its bytes, and decoding a
//!   written value returns it. Every byte string round-trips through both
//!   [`Charset`]s, and the CP037 tables are inverse permutations that match,
//!   entry for entry in both directions, an independent CP037 mapping written
//!   out in this file.
//! - **Record-level identity.** For all 12 layouts, any record built from
//!   PIC-valid fields satisfies `encode(decode(b, cs), cs) == b` in ASCII and
//!   in EBCDIC, and both charsets decode it to the same record.
//! - **Negative zero.** A `}` overpunch over all-zero digits decodes as
//!   `is_negative_zero()`, is kept as raw bytes in [`PreservedFields`], once
//!   per such field of a layout, and re-encodes to its stored bytes.
//! - **Malformed numerics are rejected.** A non-digit in a `9(n)` field or a
//!   signed body gives `NotNumeric`, a plain digit in the sign position gives
//!   `PlainDigitInSigned` (in EBCDIC, a last byte `0xF0`–`0xF9`), and any other
//!   sign byte gives `InvalidSign`, each located at its field.
//!
//! Every input is synthetic: no fixture file, container or service is used.
//! The expected digit, overpunch and CP037 bytes are written out here rather
//! than taken from the codec's own tables, so a wrong table cannot vouch for
//! itself.

use std::fmt::Debug;

use carddemo_codec::charset::{EBCDIC_TO_LATIN1, LATIN1_TO_EBCDIC};
use carddemo_codec::field::{self, fields_are_contiguous};
use carddemo_codec::{
    AccountLayout, AccountRecord, CardLayout, CardXrefLayout, Charset, CodecError, CodecErrorKind,
    CustomerLayout, DailyTranLayout, DailyTranRecord, DisclosureGroupLayout, DisclosureGroupRecord,
    FieldKind, FieldSpec, Layout, PreservedFields, RejectRecordLayout, Signed,
    TranCatBalanceLayout, TranCatBalanceRecord, TranCategoryLayout, TranTypeLayout,
    TransactionLayout, TransactionRecord, UserSecurityLayout,
};
use carddemo_domain::numeric::Alnum;
use proptest::prelude::*;
use proptest::sample::Index;

/// The layout label the field readers put in their errors.
const LAYOUT: &str = "PROPTEST";

/// The name of the ad-hoc field every field-level property reads and writes.
const FIELD: &str = "PROP-FIELD";

/// The widest numeric field the codecs accept.
const MAX_WIDTH: usize = 18;

/// The PIC class of the money fields: `S9(n)V99`.
const AMOUNT: FieldKind = FieldKind::Signed { scale: 2 };

/// The positive overpunch glyphs, entry `d` standing for a last digit `d`.
const PLUS: &[u8; 10] = b"{ABCDEFGHI";

/// The negative overpunch glyphs, entry `d` standing for a last digit `d`.
const MINUS: &[u8; 10] = b"}JKLMNOPQR";

/// All 20 overpunch glyphs: +0 to +9, then -0 to -9.
const OVERPUNCH: &[u8; 20] = b"{ABCDEFGHI}JKLMNOPQR";

/// Every signed field of the 12 layouts, as (layout name, field, offset,
/// length), in layout and copybook order: the `S9(10)V99` account money
/// fields (`CVACT01Y`), the `S9(09)V99` amounts and balance (`CVTRA06Y`,
/// `CVTRA05Y`, `CVTRA01Y`) and the `S9(04)V99` interest rate (`CVTRA02Y`).
const SIGNED_FIELDS: [(&str, &str, usize, usize); 9] = [
    ("CVACT01Y", "ACCT-CURR-BAL", 12, 12),
    ("CVACT01Y", "ACCT-CREDIT-LIMIT", 24, 12),
    ("CVACT01Y", "ACCT-CASH-CREDIT-LIMIT", 36, 12),
    ("CVACT01Y", "ACCT-CURR-CYC-CREDIT", 78, 12),
    ("CVACT01Y", "ACCT-CURR-CYC-DEBIT", 90, 12),
    ("CVTRA06Y", "DALYTRAN-AMT", 132, 11),
    ("CVTRA05Y", "TRAN-AMT", 132, 11),
    ("CVTRA01Y", "TRAN-CAT-BAL", 17, 11),
    ("CVTRA02Y", "DIS-INT-RATE", 16, 6),
];

/// IBM code page 037 to ISO-8859-1, written out independently of the codec's
/// tables: entry `e` is the Latin-1 byte for EBCDIC byte `e`, and row `n`
/// holds EBCDIC bytes `0xn0`–`0xnF`. The authority is CP037 as the Unicode
/// Consortium publishes it in `VENDORS/MICSFT/EBCDIC/CP037.TXT`, the mapping
/// Python's `cp037` codec is generated from. Every code point it maps to is
/// below U+0100, so each is its own Latin-1 byte.
const CP037_TO_LATIN1: [u8; 256] = [
    0x00, 0x01, 0x02, 0x03, 0x9C, 0x09, 0x86, 0x7F, 0x97, 0x8D, 0x8E, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
    0x10, 0x11, 0x12, 0x13, 0x9D, 0x85, 0x08, 0x87, 0x18, 0x19, 0x92, 0x8F, 0x1C, 0x1D, 0x1E, 0x1F,
    0x80, 0x81, 0x82, 0x83, 0x84, 0x0A, 0x17, 0x1B, 0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x05, 0x06, 0x07,
    0x90, 0x91, 0x16, 0x93, 0x94, 0x95, 0x96, 0x04, 0x98, 0x99, 0x9A, 0x9B, 0x14, 0x15, 0x9E, 0x1A,
    0x20, 0xA0, 0xE2, 0xE4, 0xE0, 0xE1, 0xE3, 0xE5, 0xE7, 0xF1, 0xA2, 0x2E, 0x3C, 0x28, 0x2B, 0x7C,
    0x26, 0xE9, 0xEA, 0xEB, 0xE8, 0xED, 0xEE, 0xEF, 0xEC, 0xDF, 0x21, 0x24, 0x2A, 0x29, 0x3B, 0xAC,
    0x2D, 0x2F, 0xC2, 0xC4, 0xC0, 0xC1, 0xC3, 0xC5, 0xC7, 0xD1, 0xA6, 0x2C, 0x25, 0x5F, 0x3E, 0x3F,
    0xF8, 0xC9, 0xCA, 0xCB, 0xC8, 0xCD, 0xCE, 0xCF, 0xCC, 0x60, 0x3A, 0x23, 0x40, 0x27, 0x3D, 0x22,
    0xD8, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0xAB, 0xBB, 0xF0, 0xFD, 0xFE, 0xB1,
    0xB0, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0xAA, 0xBA, 0xE6, 0xB8, 0xC6, 0xA4,
    0xB5, 0x7E, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0xA1, 0xBF, 0xD0, 0xDD, 0xDE, 0xAE,
    0x5E, 0xA3, 0xA5, 0xB7, 0xA9, 0xA7, 0xB6, 0xBC, 0xBD, 0xBE, 0x5B, 0x5D, 0xAF, 0xA8, 0xB4, 0xD7,
    0x7B, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0xAD, 0xF4, 0xF6, 0xF2, 0xF3, 0xF5,
    0x7D, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0xB9, 0xFB, 0xFC, 0xF9, 0xFA, 0xFF,
    0x5C, 0xF7, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0xB2, 0xD4, 0xD6, 0xD2, 0xD3, 0xD5,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0xB3, 0xDB, 0xDC, 0xD9, 0xDA, 0x9F,
];

/// The proptest configuration: the default case count, and no failure
/// persistence, so a failing run writes no `proptest-regressions/` directory
/// into the checkout. The shrunk failing case is still printed.
fn config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// A numeric field width: 1 to 18 digits.
fn width() -> impl Strategy<Value = usize> {
    1..=MAX_WIDTH
}

/// `w` digit bytes `0`–`9`.
fn digits(w: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(b'0'..=b'9', w)
}

/// One of the 20 overpunch glyphs.
fn overpunch() -> impl Strategy<Value = u8> {
    prop::sample::select(&OVERPUNCH[..])
}

/// `n` arbitrary bytes.
fn any_bytes(n: usize) -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), n)
}

/// A PIC-valid signed field of `w` bytes: `w - 1` digits and an overpunch
/// glyph. One case in four has an all-zero body with a `{` or `}` sign, so
/// that negative zero, which random digits almost never spell, is exercised
/// at every width.
fn signed_field(w: usize) -> impl Strategy<Value = Vec<u8>> {
    let body = w.saturating_sub(1);
    prop_oneof![
        3 => (digits(body), overpunch()),
        1 => (Just(vec![b'0'; body]), prop::sample::select(&b"{}"[..])),
    ]
    .prop_map(|(mut bytes, sign)| {
        bytes.push(sign);
        bytes
    })
}

/// A field width and a value of at most that many digits.
fn unsigned_case() -> impl Strategy<Value = (usize, u64)> {
    width().prop_flat_map(|len| (Just(len), 0..limit(len)))
}

/// A field width, a magnitude of at most that many digits, and a sign.
fn signed_case() -> impl Strategy<Value = (usize, u64, bool)> {
    width().prop_flat_map(|len| (Just(len), 0..limit(len), any::<bool>()))
}

/// Either record character set.
fn charset() -> impl Strategy<Value = Charset> {
    prop_oneof![Just(Charset::Ascii), Just(Charset::Ebcdic)]
}

/// `10^w`, the exclusive bound of a `w`-digit value.
fn limit(w: usize) -> u64 {
    u32::try_from(w)
        .ok()
        .and_then(|exponent| 10_u64.checked_pow(exponent))
        .expect("10^w fits a u64 for every width up to 18")
}

/// The ad-hoc field of `len` bytes at offset 0.
const fn prop_spec(len: usize, kind: FieldKind) -> FieldSpec {
    FieldSpec::new(FIELD, 0, len, kind)
}

/// Whether `bytes` spell a negative zero: every digit `0`, sign `}`.
fn is_negative_zero_bytes(bytes: &[u8]) -> bool {
    bytes
        .split_last()
        .is_some_and(|(&last, body)| last == b'}' && body.iter().all(|&byte| byte == b'0'))
}

/// The error of a read that must fail, or a test failure naming `what`.
fn expect_err<T>(result: Result<T, CodecError>, what: &str) -> Result<CodecError, TestCaseError> {
    result
        .err()
        .ok_or_else(|| TestCaseError::fail(format!("{what} was accepted")))
}

/// `X(N)` and `FILLER`: the bytes are read verbatim, by `field_bytes` and by
/// `alnum`, and `put_alnum` writes them back unchanged.
fn check_alnum<const N: usize>(input: &[u8]) -> Result<(), TestCaseError> {
    for kind in [FieldKind::Alnum, FieldKind::Filler] {
        let spec = prop_spec(N, kind);
        prop_assert_eq!(field::field_bytes(LAYOUT, input, &spec)?, input);
        let value: Alnum<N> = field::alnum(LAYOUT, input, &spec)?;
        prop_assert_eq!(value.as_bytes().as_slice(), input);
        let mut out = vec![b'#'; N];
        field::put_alnum(&mut out, &spec, value.as_bytes());
        prop_assert!(out == input, "{kind:?} wrote {out:?}");
    }
    Ok(())
}

/// `9(w)`: `value` is written as `w` zero-padded digits and read back.
fn check_unsigned_value(w: usize, value: u64) -> Result<(), TestCaseError> {
    let spec = prop_spec(w, FieldKind::Unsigned);
    let mut out = vec![b' '; w];
    field::put_unsigned(&mut out, &spec, value);
    prop_assert_eq!(out.as_slice(), format!("{value:0w$}").into_bytes());
    prop_assert_eq!(field::unsigned(LAYOUT, &out, &spec)?, value);
    Ok(())
}

/// `9(n)`: a digit string, leading zeros included, re-encodes to its bytes.
fn check_unsigned_bytes(stored: &[u8]) -> Result<(), TestCaseError> {
    let spec = prop_spec(stored.len(), FieldKind::Unsigned);
    let value = field::unsigned(LAYOUT, stored, &spec)?;
    let mut out = vec![b' '; stored.len()];
    field::put_unsigned(&mut out, &spec, value);
    prop_assert_eq!(out.as_slice(), stored);
    Ok(())
}

/// `S9(w)V99`: the signed value `±magnitude` is written as `w - 1` zero-padded
/// digits and a last-digit overpunch, never a plain digit, and read back. Zero
/// is written with the `+` glyph, since an `i64` has no negative zero.
fn check_signed_value(w: usize, magnitude: u64, negative: bool) -> Result<(), TestCaseError> {
    let spec = prop_spec(w, AMOUNT);
    let units = i64::try_from(magnitude)?;
    let value = if negative { -units } else { units };
    let mut out = vec![b' '; w];
    field::put_signed(&mut out, &spec, value);

    let text = format!("{magnitude:0w$}");
    let (Some((&sign, body)), Some((&last_digit, leading))) =
        (out.split_last(), text.as_bytes().split_last())
    else {
        return Err(TestCaseError::fail("a signed field has at least one byte"));
    };
    prop_assert_eq!(body, leading);
    let glyphs = if value < 0 { MINUS } else { PLUS };
    let expected = glyphs.get(usize::from(last_digit - b'0')).copied();
    prop_assert_eq!(Some(sign), expected);
    prop_assert!(OVERPUNCH.contains(&sign) && !sign.is_ascii_digit());

    let decoded = field::signed(LAYOUT, &out, &spec)?;
    prop_assert_eq!(
        decoded,
        Signed {
            magnitude,
            negative: value < 0,
        }
    );
    prop_assert_eq!(decoded.value()?, value);
    Ok(())
}

/// `S9(n)V99`: digits and an overpunch re-encode to their bytes through the
/// preserving path, and a negative zero is recognised exactly when the sign is
/// `}` over all-zero digits, is recorded once as raw bytes, and is restored.
fn check_signed_bytes(stored: &[u8]) -> Result<(), TestCaseError> {
    let spec = prop_spec(stored.len(), AMOUNT);
    let mut preserved = PreservedFields::new();
    let decoded = field::signed_preserving(LAYOUT, stored, &spec, &mut preserved)?;
    prop_assert_eq!(decoded, field::signed(LAYOUT, stored, &spec)?);

    let negative_zero = is_negative_zero_bytes(stored);
    prop_assert_eq!(decoded.is_negative_zero(), negative_zero);
    prop_assert_eq!(preserved.len(), usize::from(negative_zero));
    if negative_zero {
        let raw = preserved.get(0);
        prop_assert_eq!(raw.map(|raw| raw.bytes.as_slice()), Some(stored));
        prop_assert_eq!(raw.map(|raw| raw.field), Some(FIELD));
        // Without the preserved bytes the writer can only emit +0, so the
        // stored `}` survives through the preserving path alone.
        let mut bare = vec![b' '; stored.len()];
        field::put_signed(&mut bare, &spec, 0);
        prop_assert_eq!(bare.last(), Some(&b'{'));
    }

    let mut out = vec![b' '; stored.len()];
    field::put_signed_preserving(&mut out, &spec, decoded.value()?, &preserved);
    prop_assert_eq!(out.as_slice(), stored);
    Ok(())
}

/// ISO-8859-1 to IBM code page 037, the inverse of [`CP037_TO_LATIN1`] and
/// built from it alone: entry `l` is the EBCDIC byte for Latin-1 byte `l`.
/// Panics unless the mapping is a permutation of `0..=255`, since only a
/// permutation has an inverse.
fn latin1_to_cp037() -> [u8; 256] {
    let mut inverse = [0_u8; 256];
    let mut seen = [false; 256];
    for (ebcdic, latin1) in (0..=255_u8).zip(CP037_TO_LATIN1) {
        let slot = usize::from(latin1);
        assert!(
            !seen[slot],
            "the CP037 mapping sends two EBCDIC bytes to Latin-1 0x{latin1:02X}"
        );
        seen[slot] = true;
        inverse[slot] = ebcdic;
    }
    inverse
}

/// Both charsets translate one byte for one byte and losslessly in both
/// directions; ASCII is the identity and EBCDIC is CP037, checked in each
/// direction against the independent mapping rather than the codec's tables.
fn check_charset(bytes: &[u8]) -> Result<(), TestCaseError> {
    for cs in [Charset::Ascii, Charset::Ebcdic] {
        let internal = cs.to_internal(bytes);
        let external = cs.from_internal(bytes);
        prop_assert!(internal.len() == bytes.len() && external.len() == bytes.len());
        prop_assert!(
            cs.from_internal(&internal) == bytes,
            "{cs:?} decode then encode"
        );
        prop_assert!(
            cs.to_internal(&external) == bytes,
            "{cs:?} encode then decode"
        );
    }
    prop_assert_eq!(Charset::Ascii.to_internal(bytes), bytes);
    prop_assert_eq!(Charset::Ascii.from_internal(bytes), bytes);
    let decoded: Vec<u8> = bytes
        .iter()
        .map(|&byte| CP037_TO_LATIN1[usize::from(byte)])
        .collect();
    prop_assert_eq!(Charset::Ebcdic.to_internal(bytes), decoded);
    let inverse = latin1_to_cp037();
    let encoded: Vec<u8> = bytes
        .iter()
        .map(|&byte| inverse[usize::from(byte)])
        .collect();
    prop_assert_eq!(Charset::Ebcdic.from_internal(bytes), encoded);
    Ok(())
}

/// The record strategy of layout `L`.
fn layout_strategy<L: Layout>() -> impl Strategy<Value = Vec<u8>> {
    record_strategy(L::FIELDS, L::LEN)
}

/// Internal bytes of one PIC-valid record: its fields' strategies concatenated
/// in `FIELDS` order. The fields must tile the record exactly, so the
/// generated bytes cover all `len` bytes and nothing more.
fn record_strategy(fields: &'static [FieldSpec], len: usize) -> impl Strategy<Value = Vec<u8>> {
    assert!(
        fields_are_contiguous(fields, len),
        "the fields do not tile a {len}-byte record"
    );
    fields
        .iter()
        .map(field_strategy)
        .collect::<Vec<_>>()
        .prop_map(|parts| parts.concat())
}

/// The bytes one field's PIC allows: digits for `9(n)`, digits and an
/// overpunch for `S9(n)V9(s)`, and any byte at all for `X(n)` and `FILLER`.
fn field_strategy(spec: &FieldSpec) -> BoxedStrategy<Vec<u8>> {
    match spec.kind {
        FieldKind::Unsigned => digits(spec.len).boxed(),
        FieldKind::Signed { .. } => signed_field(spec.len).boxed(),
        FieldKind::Alnum | FieldKind::Filler => any_bytes(spec.len).boxed(),
    }
}

/// Record-level identity of layout `L` on the internal record `internal`: the
/// ASCII record and its EBCDIC translation each re-encode to their bytes, and
/// both decode to the same typed record.
fn check_layout<L: Layout>(internal: &[u8]) -> Result<(), TestCaseError>
where
    L::Record: PartialEq + Debug,
{
    prop_assert_eq!(internal.len(), L::LEN);
    let from_ascii = L::decode(internal, Charset::Ascii)?;
    prop_assert_eq!(L::encode(&from_ascii, Charset::Ascii), internal);

    let stored = Charset::Ebcdic.from_internal(internal);
    let from_ebcdic = L::decode(&stored, Charset::Ebcdic)?;
    prop_assert!(L::encode(&from_ebcdic, Charset::Ebcdic) == stored);
    prop_assert_eq!(from_ebcdic, from_ascii);
    Ok(())
}

/// The record types that keep negative-zero signed fields as raw bytes.
trait Preserving {
    /// The raw fields the decoder kept.
    fn preserved(&self) -> &PreservedFields;
}

impl Preserving for AccountRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for DailyTranRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for TransactionRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for TranCatBalanceRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

impl Preserving for DisclosureGroupRecord {
    fn preserved(&self) -> &PreservedFields {
        &self.preserved
    }
}

/// The signed fields of `fields`, in copybook order.
fn signed_specs(fields: &'static [FieldSpec]) -> Vec<&'static FieldSpec> {
    fields
        .iter()
        .filter(|spec| matches!(spec.kind, FieldKind::Signed { .. }))
        .collect()
}

/// In both charsets, layout `L` keeps exactly one raw field per signed field
/// of `internal` that is a negative zero, holding that field's stored bytes.
fn check_preserved<L: Layout>(internal: &[u8]) -> Result<(), TestCaseError>
where
    L::Record: Preserving,
{
    let negative_zeros: Vec<&FieldSpec> = signed_specs(L::FIELDS)
        .into_iter()
        .filter(|spec| {
            internal
                .get(spec.offset..spec.end())
                .is_some_and(is_negative_zero_bytes)
        })
        .collect();
    for cs in [Charset::Ascii, Charset::Ebcdic] {
        let record = L::decode(&cs.from_internal(internal), cs)?;
        let preserved = record.preserved();
        prop_assert!(
            preserved.len() == negative_zeros.len(),
            "{cs:?}: {preserved:?} for {negative_zeros:?}"
        );
        for spec in &negative_zeros {
            let raw = preserved.get(spec.offset);
            prop_assert_eq!(raw.map(|raw| raw.field), Some(spec.name));
            prop_assert_eq!(
                raw.map(|raw| raw.bytes.as_slice()),
                internal.get(spec.offset..spec.end())
            );
        }
    }
    Ok(())
}

/// A signed field whose last byte is the plain digit ending `stored` is
/// rejected as `PlainDigitInSigned`, at the field, by both readers, and
/// nothing is preserved.
fn check_plain_digit_sign(stored: &[u8]) -> Result<(), TestCaseError> {
    let spec = prop_spec(stored.len(), AMOUNT);
    let plain = stored.last().copied();
    let mut preserved = PreservedFields::new();
    let results = [
        field::signed(LAYOUT, stored, &spec),
        field::signed_preserving(LAYOUT, stored, &spec, &mut preserved),
    ];
    for result in results {
        let err = expect_err(result, "a plain-digit sign")?;
        prop_assert!(
            matches!(err.kind, CodecErrorKind::PlainDigitInSigned { byte } if Some(byte) == plain),
            "{err}"
        );
        prop_assert_eq!((err.field, err.offset), (Some(FIELD), Some(0)));
    }
    prop_assert!(preserved.is_empty());
    Ok(())
}

/// The digit string `digits` with the byte `bad` at the position `at` picks:
/// `unsigned` rejects it as `NotNumeric` naming that byte, and so does
/// `signed` when it is the body of a field with a valid sign.
fn check_non_digit(digits: &[u8], at: Index, bad: u8) -> Result<(), TestCaseError> {
    let mut stored = digits.to_vec();
    let position = at.index(stored.len());
    if let Some(slot) = stored.get_mut(position) {
        *slot = bad;
    }
    let spec = prop_spec(stored.len(), FieldKind::Unsigned);
    let err = expect_err(field::unsigned(LAYOUT, &stored, &spec), "a non-digit")?;
    prop_assert!(
        matches!(err.kind, CodecErrorKind::NotNumeric { byte } if byte == bad),
        "{err}"
    );
    prop_assert_eq!((err.field, err.offset), (Some(FIELD), Some(0)));

    // The same digits with a sign appended are one byte wider, so the signed
    // case stops at a 17-digit body: 19 bytes would exceed the widest field.
    if stored.len() < MAX_WIDTH {
        stored.push(b'{');
        let spec = prop_spec(stored.len(), AMOUNT);
        let err = expect_err(field::signed(LAYOUT, &stored, &spec), "a non-digit body")?;
        prop_assert!(
            matches!(err.kind, CodecErrorKind::NotNumeric { byte } if byte == bad),
            "{err}"
        );
    }
    Ok(())
}

/// A signed field of the digits `body` ended by `sign`, which is neither an
/// overpunch glyph nor a digit, is rejected as `InvalidSign` naming `sign`.
fn check_invalid_sign(body: &[u8], sign: u8) -> Result<(), TestCaseError> {
    let mut stored = body.to_vec();
    stored.push(sign);
    let spec = prop_spec(stored.len(), AMOUNT);
    let err = expect_err(field::signed(LAYOUT, &stored, &spec), "an invalid sign")?;
    prop_assert!(
        matches!(err.kind, CodecErrorKind::InvalidSign { byte } if byte == sign),
        "{err}"
    );
    Ok(())
}

/// In the record `internal` of layout `L`, the signed field `pick` selects
/// gets the plain digit `digit` as its stored last byte in `cs`: `b'0' + digit`
/// in ASCII, `0xF0 + digit` in EBCDIC. `L::decode` rejects the record as
/// `PlainDigitInSigned` at that field, naming the internal digit byte.
fn check_layout_plain_digit_sign<L: Layout>(
    internal: &[u8],
    pick: Index,
    cs: Charset,
    digit: u8,
) -> Result<(), TestCaseError> {
    let signed = signed_specs(L::FIELDS);
    prop_assert!(!signed.is_empty(), "{} has no signed field", L::NAME);
    let spec = *pick.get(&signed);
    let plain = b'0' + digit;
    let stored_digit = match cs {
        Charset::Ascii => plain,
        Charset::Ebcdic => 0xF0 + digit,
    };
    let mut stored = cs.from_internal(internal);
    let Some(sign) = stored.get_mut(spec.end() - 1) else {
        return Err(TestCaseError::fail("the field lies outside the record"));
    };
    *sign = stored_digit;

    let err = expect_err(L::decode(&stored, cs), "a plain-digit sign")?;
    prop_assert!(
        matches!(err.kind, CodecErrorKind::PlainDigitInSigned { byte } if byte == plain),
        "{cs:?}: {err}"
    );
    prop_assert_eq!(
        (err.layout, err.field, err.offset),
        (L::NAME, Some(spec.name), Some(spec.offset))
    );
    Ok(())
}

/// The signed fields of layout `L` as `SIGNED_FIELDS` rows.
fn signed_rows<L: Layout>() -> Vec<(&'static str, &'static str, usize, usize)> {
    signed_specs(L::FIELDS)
        .into_iter()
        .map(|spec| (L::NAME, spec.name, spec.offset, spec.len))
        .collect()
}

#[test]
fn cp037_tables_match_the_independent_cp037_mapping_entry_for_entry() {
    let all: Vec<u8> = (0..=255_u8).collect();
    let inverse = latin1_to_cp037();
    for &byte in &all {
        let index = usize::from(byte);
        assert_eq!(
            EBCDIC_TO_LATIN1[index], CP037_TO_LATIN1[index],
            "EBCDIC_TO_LATIN1 at EBCDIC 0x{byte:02X}"
        );
        assert_eq!(
            LATIN1_TO_EBCDIC[index], inverse[index],
            "LATIN1_TO_EBCDIC at Latin-1 0x{byte:02X}"
        );
    }
    assert_eq!(
        Charset::Ebcdic.to_internal(&all),
        CP037_TO_LATIN1,
        "decoding every EBCDIC byte"
    );
    assert_eq!(
        Charset::Ebcdic.from_internal(&CP037_TO_LATIN1),
        all,
        "encoding the decoded EBCDIC bytes"
    );
    for table in [EBCDIC_TO_LATIN1, LATIN1_TO_EBCDIC] {
        let mut sorted = table.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, all, "a table is not a permutation of 0..=255");
    }
    for &byte in &all {
        let index = usize::from(byte);
        assert_eq!(
            LATIN1_TO_EBCDIC[usize::from(EBCDIC_TO_LATIN1[index])],
            byte,
            "EBCDIC 0x{byte:02X}"
        );
        assert_eq!(
            EBCDIC_TO_LATIN1[usize::from(LATIN1_TO_EBCDIC[index])],
            byte,
            "Latin-1 0x{byte:02X}"
        );
    }
    let glyphs = [(0x40, b' '), (0xF0, b'0'), (0xC0, b'{'), (0xD0, b'}')];
    for (ebcdic, internal) in glyphs {
        assert_eq!(EBCDIC_TO_LATIN1[ebcdic], internal, "EBCDIC 0x{ebcdic:02X}");
    }
}

#[test]
fn every_layout_tiles_its_record_and_keeps_its_signed_fields_at_their_pic_offsets() {
    let layouts: [(&[FieldSpec], usize); 12] = [
        (AccountLayout::FIELDS, AccountLayout::LEN),
        (CardLayout::FIELDS, CardLayout::LEN),
        (CardXrefLayout::FIELDS, CardXrefLayout::LEN),
        (CustomerLayout::FIELDS, CustomerLayout::LEN),
        (DailyTranLayout::FIELDS, DailyTranLayout::LEN),
        (TransactionLayout::FIELDS, TransactionLayout::LEN),
        (TranCatBalanceLayout::FIELDS, TranCatBalanceLayout::LEN),
        (DisclosureGroupLayout::FIELDS, DisclosureGroupLayout::LEN),
        (TranTypeLayout::FIELDS, TranTypeLayout::LEN),
        (TranCategoryLayout::FIELDS, TranCategoryLayout::LEN),
        (UserSecurityLayout::FIELDS, UserSecurityLayout::LEN),
        (RejectRecordLayout::FIELDS, RejectRecordLayout::LEN),
    ];
    for (fields, len) in layouts {
        assert!(fields_are_contiguous(fields, len), "{fields:?}");
    }
    // Only the five preserving layouts have signed fields, so the
    // negative-zero properties below cover every signed field there is.
    let found: Vec<_> = [
        signed_rows::<AccountLayout>(),
        signed_rows::<CardLayout>(),
        signed_rows::<CardXrefLayout>(),
        signed_rows::<CustomerLayout>(),
        signed_rows::<DailyTranLayout>(),
        signed_rows::<TransactionLayout>(),
        signed_rows::<TranCatBalanceLayout>(),
        signed_rows::<DisclosureGroupLayout>(),
        signed_rows::<TranTypeLayout>(),
        signed_rows::<TranCategoryLayout>(),
        signed_rows::<UserSecurityLayout>(),
        signed_rows::<RejectRecordLayout>(),
    ]
    .concat();
    assert_eq!(found, SIGNED_FIELDS);
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn alnum_fields_copy_every_byte_both_ways(
        one in any_bytes(1),
        two in any_bytes(2),
        ten in any_bytes(10),
        sixteen in any_bytes(16),
        twenty_six in any_bytes(26),
        fifty in any_bytes(50),
    ) {
        check_alnum::<1>(&one)?;
        check_alnum::<2>(&two)?;
        check_alnum::<10>(&ten)?;
        check_alnum::<16>(&sixteen)?;
        check_alnum::<26>(&twenty_six)?;
        check_alnum::<50>(&fifty)?;
    }

    #[test]
    fn unsigned_values_round_trip_zero_padded((len, value) in unsigned_case()) {
        check_unsigned_value(len, value)?;
    }

    #[test]
    fn unsigned_digit_strings_round_trip_byte_for_byte(
        stored in width().prop_flat_map(digits),
    ) {
        check_unsigned_bytes(&stored)?;
    }

    #[test]
    fn signed_values_round_trip_with_an_overpunch_sign(
        (len, magnitude, negative) in signed_case(),
    ) {
        check_signed_value(len, magnitude, negative)?;
    }

    #[test]
    fn signed_fields_round_trip_byte_for_byte_negative_zero_included(
        stored in width().prop_flat_map(signed_field),
    ) {
        check_signed_bytes(&stored)?;
    }

    #[test]
    fn charsets_round_trip_every_byte_string(
        bytes in prop::collection::vec(any::<u8>(), 0..=512),
    ) {
        check_charset(&bytes)?;
    }

    #[test]
    fn account_records_round_trip(internal in layout_strategy::<AccountLayout>()) {
        check_layout::<AccountLayout>(&internal)?;
        check_preserved::<AccountLayout>(&internal)?;
    }

    #[test]
    fn card_records_round_trip(internal in layout_strategy::<CardLayout>()) {
        check_layout::<CardLayout>(&internal)?;
    }

    #[test]
    fn card_xref_records_round_trip(internal in layout_strategy::<CardXrefLayout>()) {
        check_layout::<CardXrefLayout>(&internal)?;
    }

    #[test]
    fn customer_records_round_trip(internal in layout_strategy::<CustomerLayout>()) {
        check_layout::<CustomerLayout>(&internal)?;
    }

    #[test]
    fn daily_tran_records_round_trip(internal in layout_strategy::<DailyTranLayout>()) {
        check_layout::<DailyTranLayout>(&internal)?;
        check_preserved::<DailyTranLayout>(&internal)?;
    }

    #[test]
    fn transaction_records_round_trip(internal in layout_strategy::<TransactionLayout>()) {
        check_layout::<TransactionLayout>(&internal)?;
        check_preserved::<TransactionLayout>(&internal)?;
    }

    #[test]
    fn tran_cat_balance_records_round_trip(
        internal in layout_strategy::<TranCatBalanceLayout>(),
    ) {
        check_layout::<TranCatBalanceLayout>(&internal)?;
        check_preserved::<TranCatBalanceLayout>(&internal)?;
    }

    #[test]
    fn disclosure_group_records_round_trip(
        internal in layout_strategy::<DisclosureGroupLayout>(),
    ) {
        check_layout::<DisclosureGroupLayout>(&internal)?;
        check_preserved::<DisclosureGroupLayout>(&internal)?;
    }

    #[test]
    fn tran_type_records_round_trip(internal in layout_strategy::<TranTypeLayout>()) {
        check_layout::<TranTypeLayout>(&internal)?;
    }

    #[test]
    fn tran_category_records_round_trip(internal in layout_strategy::<TranCategoryLayout>()) {
        check_layout::<TranCategoryLayout>(&internal)?;
    }

    #[test]
    fn user_security_records_round_trip(internal in layout_strategy::<UserSecurityLayout>()) {
        check_layout::<UserSecurityLayout>(&internal)?;
    }

    #[test]
    fn reject_records_round_trip(internal in layout_strategy::<RejectRecordLayout>()) {
        check_layout::<RejectRecordLayout>(&internal)?;
    }

    #[test]
    fn signed_rejects_a_plain_digit_sign(stored in width().prop_flat_map(digits)) {
        check_plain_digit_sign(&stored)?;
    }

    #[test]
    fn numeric_fields_reject_a_non_digit(
        (stored, at, bad) in (
            width().prop_flat_map(digits),
            any::<Index>(),
            any::<u8>().prop_filter("a non-digit byte", |byte| !byte.is_ascii_digit()),
        ),
    ) {
        check_non_digit(&stored, at, bad)?;
    }

    #[test]
    fn signed_rejects_an_invalid_sign(
        (body, sign) in (
            (0..MAX_WIDTH).prop_flat_map(digits),
            any::<u8>().prop_filter("neither a digit nor an overpunch glyph", |byte| {
                !byte.is_ascii_digit() && !OVERPUNCH.contains(byte)
            }),
        ),
    ) {
        check_invalid_sign(&body, sign)?;
    }

    #[test]
    fn account_records_reject_a_plain_digit_sign(
        internal in layout_strategy::<AccountLayout>(),
        pick in any::<Index>(),
        cs in charset(),
        digit in 0_u8..=9,
    ) {
        check_layout_plain_digit_sign::<AccountLayout>(&internal, pick, cs, digit)?;
    }

    #[test]
    fn daily_tran_records_reject_a_plain_digit_sign(
        internal in layout_strategy::<DailyTranLayout>(),
        pick in any::<Index>(),
        cs in charset(),
        digit in 0_u8..=9,
    ) {
        check_layout_plain_digit_sign::<DailyTranLayout>(&internal, pick, cs, digit)?;
    }

    #[test]
    fn transaction_records_reject_a_plain_digit_sign(
        internal in layout_strategy::<TransactionLayout>(),
        pick in any::<Index>(),
        cs in charset(),
        digit in 0_u8..=9,
    ) {
        check_layout_plain_digit_sign::<TransactionLayout>(&internal, pick, cs, digit)?;
    }

    #[test]
    fn tran_cat_balance_records_reject_a_plain_digit_sign(
        internal in layout_strategy::<TranCatBalanceLayout>(),
        pick in any::<Index>(),
        cs in charset(),
        digit in 0_u8..=9,
    ) {
        check_layout_plain_digit_sign::<TranCatBalanceLayout>(&internal, pick, cs, digit)?;
    }

    #[test]
    fn disclosure_group_records_reject_a_plain_digit_sign(
        internal in layout_strategy::<DisclosureGroupLayout>(),
        pick in any::<Index>(),
        cs in charset(),
        digit in 0_u8..=9,
    ) {
        check_layout_plain_digit_sign::<DisclosureGroupLayout>(&internal, pick, cs, digit)?;
    }
}
