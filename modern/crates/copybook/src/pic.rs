//! COBOL picture value types: `PIC X(n)`, unsigned `PIC 9(n)`, signed integral `PIC S9(n)`,
//! `PIC S9(4) COMP` counts, and the `PIC X(10)` dates and `PIC X(26)` timestamps (AAP 0.3.3.1).
//!
//! Width, sign and range live in each type, so a value can never be wider than its field or of
//! another class. Money and rates are `domain::money`; card numbers, SSNs, CVVs and passwords are
//! wrapped by `domain::sensitive`.
//!
//! Every decoder here is total and byte-exact: for any input of any length it returns a value or
//! a typed [`ErrorKind`] and never panics (Total Parsers Over Untrusted Input), and it succeeds
//! only when encoding the value yields the input bytes again (codec A). An anomaly is an error,
//! never a default or a normalised value (R7): spaces in a `PIC 9(n)` field, a byte outside
//! printable ASCII in a `PIC X(n)` field, a partly blank date, a timestamp in no known layout.
//!
//! Dates and timestamps decode to `chrono` values. Their text layouts, `YYYY-MM-DD` and the
//! timestamp layouts `I`, `D` and `S` (PIC-018 to PIC-020, E33), exist only inside these
//! codecs, the byte level that Typed Dates and Timestamps exempts. Outside them a date is a
//! [`PicDate`] and a timestamp a [`PicTimestamp`], whose [`TsForm`] records the layout the COBOL
//! wrote so that encoding reproduces the bytes.
//!
//! Integral values are 64-bit for every width up to 18 digits (PIC-023, Checked Integer
//! Narrowing). The digit helpers are crate-internal and shared with `zoned`, `edited` and
//! `record`.

use std::fmt;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};

use crate::error::ErrorKind;

const SPACE: u8 = b' ';

/// Width of a `PIC X(10)` date field.
const DATE_WIDTH: usize = 10;

/// Width of a `PIC X(26)` timestamp field.
const TIMESTAMP_WIDTH: usize = 26;

/// Highest year a four-digit year field can carry.
const MAX_YEAR: i32 = 9999;

/// Nanoseconds per microsecond: the timestamp text carries six fraction digits.
const NANOS_PER_MICRO: u32 = 1_000;

/// Nanoseconds per second; `chrono` represents a leap second with a sub-second value at or
/// above it.
const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// Bytes of a `PIC X(n)` field must be printable ASCII, `0x20`–`0x7E` (PIC-001, PIC-002).
fn is_printable(byte: u8) -> bool {
    matches!(byte, b' '..=b'~')
}

fn is_blank(bytes: &[u8]) -> bool {
    bytes.iter().all(|&byte| byte == SPACE)
}

/// Number of bytes a field of `n` decimal digits occupies in DISPLAY form.
pub(crate) fn digit_count(n: u32) -> usize {
    // TRUNCATION: lossless on every supported target, where `usize` is 32 bits or wider; the
    // fallback is unreachable there and only keeps this conversion free of a panic path.
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// Value of a run of ASCII digits, most significant first; an empty run is 0.
///
/// # Errors
///
/// [`ErrorKind::NonDigit`] for any byte outside `0`–`9`, spaces and signs included (PIC-004);
/// [`ErrorKind::OutOfRange`] when the value does not fit `u128`.
pub(crate) fn parse_digits(bytes: &[u8]) -> Result<u128, ErrorKind> {
    bytes.iter().try_fold(0_u128, |value, &byte| {
        let digit = digit_value(byte).ok_or(ErrorKind::NonDigit)?;
        value
            .checked_mul(10)
            .and_then(|shifted| shifted.checked_add(digit))
            .ok_or(ErrorKind::OutOfRange)
    })
}

fn digit_value(byte: u8) -> Option<u128> {
    if byte.is_ascii_digit() {
        byte.checked_sub(b'0').map(u128::from)
    } else {
        None
    }
}

/// Appends exactly `count` ASCII digits to `out`: the low-order `count` digits of `magnitude`,
/// zero-padded on the left.
pub(crate) fn push_digits(out: &mut Vec<u8>, magnitude: u128, count: u32) {
    // TRUNCATION: digits of `magnitude` above the low-order `count` are dropped, the COBOL MOVE
    // rule for a receiver with fewer integer positions; callers that must not truncate check the
    // value's range first.
    let start = out.len();
    let mut rest = magnitude;
    for _ in 0..count {
        out.push(ascii_digit(rest % 10));
        rest /= 10;
    }
    if let Some(digits) = out.get_mut(start..) {
        digits.reverse();
    }
}

/// The ASCII digit for a value below 10.
fn ascii_digit(value: u128) -> u8 {
    // Every caller passes a remainder modulo 10, so the conversion succeeds and the sum stays in
    // `0`–`9`; the fallback only keeps this function free of a panic path.
    u8::try_from(value)
        .ok()
        .and_then(|digit| b'0'.checked_add(digit))
        .unwrap_or(b'0')
}

/// COBOL `PIC X(n)`: exactly `N` bytes, padding kept and nothing trimmed (PIC-001).
///
/// [`PicX::decode`] accepts printable ASCII only (`0x20`–`0x7E`): a control byte, a CR inside a
/// line body or a byte above `0x7E` could not round-trip through a `CHAR(n)` column, which counts
/// characters rather than bytes, so it is [`ErrorKind::NonPrintable`] (PIC-002). Every other byte
/// is kept verbatim, `FILLER` contents included (PIC-012, E05). Values compare bytewise, which is
/// the ASCII collating sequence.
///
/// `Debug` shows the text, so a protected field (card number, SSN, CVV, password) is never held
/// as a bare `PicX` outside the `domain::sensitive` wrappers, whose `Debug` redacts it (PIC-024,
/// Cardholder Data Redaction).
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PicX<const N: usize>([u8; N]);

impl<const N: usize> PicX<N> {
    /// Field width in bytes.
    pub const WIDTH: usize = N;

    /// Decodes exactly `N` bytes, kept verbatim.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] when `bytes` is not `N` bytes long;
    /// [`ErrorKind::NonPrintable`] for a byte outside `0x20`–`0x7E` (PIC-002).
    pub fn decode(bytes: &[u8]) -> Result<Self, ErrorKind> {
        let field = <[u8; N]>::try_from(bytes).map_err(|_| ErrorKind::WidthMismatch {
            expected: N,
            actual: bytes.len(),
        })?;
        if field.iter().all(|&byte| is_printable(byte)) {
            Ok(Self(field))
        } else {
            Err(ErrorKind::NonPrintable)
        }
    }

    /// The value holding exactly these bytes.
    #[must_use]
    pub const fn from_array(bytes: [u8; N]) -> Self {
        Self(bytes)
    }

    /// `N` spaces, the value COBOL `SPACES` and `INITIALIZE` give an alphanumeric field.
    #[must_use]
    pub const fn spaces() -> Self {
        Self([SPACE; N])
    }

    /// The field's bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; N] {
        &self.0
    }

    /// Appends the field's `N` bytes, unchanged.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0);
    }

    /// `text` left-justified and space-padded to `N` bytes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] when `text` is longer than `N` bytes;
    /// [`ErrorKind::NonPrintable`] for a byte outside `0x20`–`0x7E`, so every non-ASCII
    /// character (PIC-002).
    pub fn from_text(text: &str) -> Result<Self, ErrorKind> {
        let bytes = text.as_bytes();
        if bytes.len() > N {
            return Err(ErrorKind::WidthMismatch {
                expected: N,
                actual: bytes.len(),
            });
        }
        if bytes.iter().all(|&byte| is_printable(byte)) {
            Ok(Self::moved_from(bytes))
        } else {
            Err(ErrorKind::NonPrintable)
        }
    }

    /// COBOL alphanumeric `MOVE` of `src` into this field: left-justified, space-padded on the
    /// right, and truncated on the right when `src` is longer than `N`.
    #[must_use]
    pub fn moved_from(src: &[u8]) -> Self {
        let mut field = [SPACE; N];
        // TRUNCATION: source bytes beyond `N` are dropped, the COBOL MOVE rule for a shorter
        // alphanumeric receiver.
        for (target, &byte) in field.iter_mut().zip(src) {
            *target = byte;
        }
        Self(field)
    }

    /// The bytes without trailing spaces; other whitespace is kept.
    #[must_use]
    pub fn trim_end(&self) -> &[u8] {
        let mut rest: &[u8] = &self.0;
        while let [head @ .., SPACE] = rest {
            rest = head;
        }
        rest
    }

    /// The text without trailing spaces.
    #[must_use]
    pub fn to_text(&self) -> String {
        String::from_utf8_lossy(self.trim_end()).into_owned()
    }

    /// Whether every byte is a space.
    #[must_use]
    pub fn is_spaces(&self) -> bool {
        is_blank(&self.0)
    }
}

impl<const N: usize> Default for PicX<N> {
    fn default() -> Self {
        Self::spaces()
    }
}

impl<const N: usize> fmt::Debug for PicX<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PicX(\"{}\")", self.0.escape_ascii())
    }
}

/// COBOL `PIC 9(n)`, unsigned DISPLAY: a value in `0..=10^N − 1`, held as `u64` for every
/// width from 1 to 18, so every identifier of 10 or more digits is 64-bit (PIC-003, PIC-023,
/// Checked Integer Narrowing).
///
/// A width outside 1–18 fails to compile wherever a constructor is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PicU<const N: u32>(u64);

impl<const N: u32> PicU<N> {
    /// The largest value: `N` nines.
    pub const MAX: u64 = {
        assert!(N != 0 && N < 19, "PicU width must be 1..=18");
        10_u64.pow(N) - 1
    };

    /// Zero, which encodes as `N` zeros.
    pub const ZERO: Self = Self(0);

    /// The value `value`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] above [`Self::MAX`].
    pub const fn new(value: u64) -> Result<Self, ErrorKind> {
        if value > Self::MAX {
            Err(ErrorKind::OutOfRange)
        } else {
            Ok(Self(value))
        }
    }

    /// The value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Decodes exactly `N` ASCII digits.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] when `bytes` is not `N` bytes long; [`ErrorKind::NonDigit`]
    /// for any other byte, spaces and signs included: a blank field is an error, never zero
    /// (PIC-004, R7).
    pub fn decode(bytes: &[u8]) -> Result<Self, ErrorKind> {
        let width = digit_count(N);
        if bytes.len() != width {
            return Err(ErrorKind::WidthMismatch {
                expected: width,
                actual: bytes.len(),
            });
        }
        let value = u64::try_from(parse_digits(bytes)?).map_err(|_| ErrorKind::OutOfRange)?;
        Self::new(value)
    }

    /// Appends `N` digits, leading zeros restored (PIC-003).
    pub fn encode_into(self, out: &mut Vec<u8>) {
        push_digits(out, u128::from(self.0), N);
    }

    /// The next value: highest-plus-one key allocation.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] at [`Self::MAX`]; the value never wraps.
    pub fn succ(self) -> Result<Self, ErrorKind> {
        self.0
            .checked_add(1)
            .ok_or(ErrorKind::OutOfRange)
            .and_then(Self::new)
    }
}

impl<const N: u32> TryFrom<u64> for PicU<N> {
    type Error = ErrorKind;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl<const N: u32> TryFrom<i64> for PicU<N> {
    type Error = ErrorKind;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u64::try_from(value)
            .map_err(|_| ErrorKind::OutOfRange)
            .and_then(Self::new)
    }
}

impl<const N: u32> From<PicU<N>> for u64 {
    fn from(value: PicU<N>) -> Self {
        value.0
    }
}

/// COBOL signed integral `PIC S9(n)`: a value in `−(10^N − 1)..=10^N − 1`, held as `i64` for
/// every width from 1 to 18 (PIC-023). The type carries no representation; the DISPLAY and
/// `COMP-3` codecs are in `zoned`.
///
/// It holds `PA-ACCT-ID PIC S9(11) COMP-3`, which joins the `PIC 9(11)` account key through
/// `From<PicU<N>>` and `TryFrom<PicS<N>> for PicU<N>`, and the inverted authorization keys
/// `PA-AUTH-DATE-9C PIC S9(05)` and `PA-AUTH-TIME-9C PIC S9(09)` through
/// [`PicS::complement_from`] (`CIPAUSMY.cpy:19`, `CIPAUDTY.cpy:20-21`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PicS<const N: u32>(i64);

impl<const N: u32> PicS<N> {
    /// The largest magnitude: `N` nines.
    pub const MAX: i64 = {
        assert!(N != 0 && N < 19, "PicS width must be 1..=18");
        10_i64.pow(N) - 1
    };

    /// The value `value`.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] when the magnitude exceeds [`Self::MAX`].
    pub fn new(value: i64) -> Result<Self, ErrorKind> {
        if (-Self::MAX..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ErrorKind::OutOfRange)
        }
    }

    /// The value.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// `MAX − value`, the inverted key COPAUA0C stores: `99999 − YYDDD` for
    /// `PA-AUTH-DATE-9C` (N = 5) and `999999999 − HHMMSSmmm` for `PA-AUTH-TIME-9C` (N = 9)
    /// (`COPAUA0C.cbl:874-875`).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] when `value` exceeds [`Self::MAX`].
    pub fn complement_from(value: u64) -> Result<Self, ErrorKind> {
        let inverted = Self::MAX
            .unsigned_abs()
            .checked_sub(value)
            .ok_or(ErrorKind::OutOfRange)?;
        i64::try_from(inverted)
            .map_err(|_| ErrorKind::OutOfRange)
            .and_then(Self::new)
    }

    /// The value an inverted key was built from: `MAX − self`, the inverse of
    /// [`PicS::complement_from`].
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] for a negative value, which no inverted key holds.
    pub fn uncomplement(self) -> Result<u64, ErrorKind> {
        let value = u64::try_from(self.0).map_err(|_| ErrorKind::OutOfRange)?;
        Self::MAX
            .unsigned_abs()
            .checked_sub(value)
            .ok_or(ErrorKind::OutOfRange)
    }
}

impl<const N: u32> From<PicU<N>> for PicS<N> {
    fn from(value: PicU<N>) -> Self {
        // TRUNCATION: a `PicU<N>` value is at most 10^N − 1 ≤ 10^18 − 1, below `i64::MAX`, so the
        // conversion is exact for every width that compiles; the fallback is unreachable.
        Self(i64::try_from(value.get()).unwrap_or(Self::MAX))
    }
}

impl<const N: u32> TryFrom<PicS<N>> for PicU<N> {
    type Error = ErrorKind;

    fn try_from(value: PicS<N>) -> Result<Self, Self::Error> {
        u64::try_from(value.get())
            .map_err(|_| ErrorKind::OutOfRange)
            .and_then(Self::new)
    }
}

/// COBOL `PIC S9(4) COMP`: a two-byte binary count (`PA-APPROVED-AUTH-CNT`,
/// `PA-DECLINED-AUTH-CNT`, `CIPAUSMY.cpy:27-28`).
///
/// Every halfword value is a valid field: the codec decodes and re-encodes all 65,536 of them
/// unchanged, and the `SMALLINT` column holds the same range (PIC-011). The picture's four
/// digits bound arithmetic instead: [`Count::add_one`] and [`Count::sub_one`] produce a value
/// only within `−9999..=9999`, because beyond it the COBOL result depends on the `TRUNC`
/// compiler option and has no single expected value (ARITH-042, ARITH-046).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Count(i16);

impl Count {
    /// Zero, the value `INITIALIZE … REPLACING NUMERIC DATA BY ZERO` gives a count.
    pub const ZERO: Self = Self(0);

    /// The largest magnitude `ADD 1` and `SUBTRACT 1` produce: four nines.
    pub const MAX: i16 = 9999;

    /// The count `value`. Total: every halfword is a valid `PIC S9(4) COMP` field (PIC-011).
    #[must_use]
    pub const fn new(value: i16) -> Self {
        Self(value)
    }

    /// The value.
    #[must_use]
    pub const fn get(self) -> i16 {
        self.0
    }

    /// `ADD 1 TO` the count (ARITH-042).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] when the result exceeds [`Count::MAX`]; the count never wraps
    /// or truncates.
    pub fn add_one(self) -> Result<Self, ErrorKind> {
        self.step(1)
    }

    /// `SUBTRACT 1 FROM` the count (ARITH-046).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] when the result is below `−Count::MAX`; the count never wraps
    /// or truncates.
    pub fn sub_one(self) -> Result<Self, ErrorKind> {
        self.step(-1)
    }

    fn step(self, delta: i16) -> Result<Self, ErrorKind> {
        self.0
            .checked_add(delta)
            .filter(|value| (-Self::MAX..=Self::MAX).contains(value))
            .map(Self)
            .ok_or(ErrorKind::OutOfRange)
    }
}

impl From<i16> for Count {
    fn from(value: i16) -> Self {
        Self(value)
    }
}

impl From<Count> for i16 {
    fn from(value: Count) -> Self {
        value.0
    }
}

/// A `PIC X(10)` date: `ACCT-OPEN-DATE`, `ACCT-EXPIRAION-DATE`, `ACCT-REISSUE-DATE`
/// (`CVACT01Y.cpy:10-12`), `CARD-EXPIRAION-DATE` (`CVACT02Y.cpy:9`) and `CUST-DOB-YYYY-MM-DD`
/// (`CVCUS01Y.cpy:19`) (Typed Dates and Timestamps).
///
/// Ten spaces are `None`; any other value is a valid calendar date in `YYYY-MM-DD` with
/// zero-padded parts (PIC-015). Every other text is [`ErrorKind::InvalidDate`] (PIC-016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PicDate(Option<NaiveDate>);

impl PicDate {
    /// Field width in bytes.
    pub const WIDTH: usize = DATE_WIDTH;

    /// Ten spaces.
    pub const BLANK: Self = Self(None);

    /// The date `date`, or a blank field for `None`; codec B builds values from `DATE` columns
    /// through this.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] for a year outside `0..=9999`, which four digits cannot carry.
    pub fn new(date: Option<NaiveDate>) -> Result<Self, ErrorKind> {
        match date {
            Some(day) if !year_fits(day) => Err(ErrorKind::OutOfRange),
            _ => Ok(Self(date)),
        }
    }

    /// The date, `None` for a blank field.
    #[must_use]
    pub const fn value(self) -> Option<NaiveDate> {
        self.0
    }

    /// Decodes ten bytes: ten spaces, or `YYYY-MM-DD` naming a valid calendar date.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] when `bytes` is not 10 bytes long; [`ErrorKind::InvalidDate`]
    /// for a partly blank value, another layout or separator, an unpadded part, or an impossible
    /// date such as month 13 or 30 February (PIC-016).
    pub fn decode(bytes: &[u8]) -> Result<Self, ErrorKind> {
        let field = <[u8; DATE_WIDTH]>::try_from(bytes).map_err(|_| ErrorKind::WidthMismatch {
            expected: DATE_WIDTH,
            actual: bytes.len(),
        })?;
        if is_blank(&field) {
            return Ok(Self::BLANK);
        }
        parse_date_text(field)
            .map(|date| Self(Some(date)))
            .ok_or(ErrorKind::InvalidDate)
    }

    /// Appends `YYYY-MM-DD` with zero-padded parts, or ten spaces for a blank field.
    pub fn encode_into(self, out: &mut Vec<u8>) {
        match self.0 {
            Some(date) => push_date(out, date),
            None => out.extend_from_slice(&[SPACE; DATE_WIDTH]),
        }
    }
}

fn year_fits(date: NaiveDate) -> bool {
    (0..=MAX_YEAR).contains(&date.year())
}

/// The value of a fixed run of ASCII digits.
fn digits_u32<const K: usize>(digits: [u8; K]) -> Option<u32> {
    u32::try_from(parse_digits(&digits).ok()?).ok()
}

/// `YYYY-MM-DD` by fixed position; `chrono`'s text parser would accept unpadded parts, which
/// could not re-encode to the same bytes.
fn parse_date_text(text: [u8; DATE_WIDTH]) -> Option<NaiveDate> {
    let [y1, y2, y3, y4, b'-', m1, m2, b'-', d1, d2] = text else {
        return None;
    };
    let year = i32::try_from(digits_u32([y1, y2, y3, y4])?).ok()?;
    NaiveDate::from_ymd_opt(year, digits_u32([m1, m2])?, digits_u32([d1, d2])?)
}

fn push_date(out: &mut Vec<u8>, date: NaiveDate) {
    // Every constructor and decoder admits only years 0..=9999, so the conversion succeeds; the
    // fallback only keeps this function free of a panic path.
    let year = u128::try_from(date.year()).unwrap_or_default();
    push_digits(out, year, 4);
    out.push(b'-');
    push_digits(out, u128::from(date.month()), 2);
    out.push(b'-');
    push_digits(out, u128::from(date.day()), 2);
}

/// The text layout a COBOL writer used for a `PIC X(26)` timestamp, stored as the
/// `<col>_form CHAR(1)` code beside the typed value (E33).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TsForm {
    /// Code `I`, `YYYY-MM-DD HH:MM:SS.ffffff`: every `DALYTRAN-ORIG-TS` of `dailytran.txt`, which
    /// CBTRN02C copies to `TRAN-ORIG-TS`, and COBIL00C's `WS-TIMESTAMP` (`CSDAT01Y.cpy:42-55`,
    /// `COBIL00C.cbl:231-232`) (PIC-018).
    Iso,
    /// Code `D`, the Db2 form `YYYY-MM-DD-HH.MM.SS.ffffff`: `DB2-FORMAT-TS`
    /// (`CBTRN02C.cbl:159-171,436-438`, `CBACT04C.cbl:497`) (PIC-019).
    Db2,
    /// Code `S`, a `YYYY-MM-DD` screen date followed by 16 spaces, which COTRN02C moves into
    /// the field (`COTRN02C.cbl:464-465`); the value is that date at midnight (PIC-020).
    ScreenDate,
}

impl TsForm {
    /// The layout code of the `<col>_form` column.
    #[must_use]
    pub const fn code(self) -> char {
        match self {
            Self::Iso => 'I',
            Self::Db2 => 'D',
            Self::ScreenDate => 'S',
        }
    }

    /// The layout a `<col>_form` code names.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::UnknownFormCode`] for any character other than `I`, `D` and `S`.
    pub const fn from_code(code: char) -> Result<Self, ErrorKind> {
        match code {
            'I' => Ok(Self::Iso),
            'D' => Ok(Self::Db2),
            'S' => Ok(Self::ScreenDate),
            _ => Err(ErrorKind::UnknownFormCode),
        }
    }

    /// The separator between date and time and the one between time parts, or `None` for the
    /// date-only layout.
    const fn clock_separators(self) -> Option<(u8, u8)> {
        match self {
            Self::Iso => Some((b' ', b':')),
            Self::Db2 => Some((b'-', b'.')),
            Self::ScreenDate => None,
        }
    }
}

/// A non-blank `PIC X(26)` timestamp: its value and the layout it was written in.
///
/// The value has microsecond precision, the six fraction digits of the text and of the
/// `TIMESTAMP(6)` column, and a year of four digits; a form-`S` value is a date at midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CobolTimestamp {
    value: NaiveDateTime,
    form: TsForm,
}

impl CobolTimestamp {
    /// The timestamp `value` written in layout `form`; codec B builds values from the
    /// `TIMESTAMP(6)` column and its `_form` code through this.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] for a year outside `0..=9999`, a time finer than a microsecond,
    /// or a leap second; [`ErrorKind::InvalidTimestamp`] for a [`TsForm::ScreenDate`] value
    /// whose time is not midnight, which that layout cannot carry (PIC-020).
    pub fn new(value: NaiveDateTime, form: TsForm) -> Result<Self, ErrorKind> {
        let nanos = value.nanosecond();
        if !year_fits(value.date()) || nanos % NANOS_PER_MICRO != 0 || nanos >= NANOS_PER_SECOND {
            return Err(ErrorKind::OutOfRange);
        }
        if form == TsForm::ScreenDate && value.time() != NaiveTime::MIN {
            return Err(ErrorKind::InvalidTimestamp);
        }
        Ok(Self { value, form })
    }

    /// The timestamp.
    #[must_use]
    pub fn value(&self) -> NaiveDateTime {
        self.value
    }

    /// The layout the COBOL wrote it in.
    #[must_use]
    pub fn form(&self) -> TsForm {
        self.form
    }

    /// Appends the 26 bytes of the value in its layout.
    fn encode_into(&self, out: &mut Vec<u8>) {
        push_date(out, self.value.date());
        match self.form.clock_separators() {
            Some((date_time, time)) => push_clock(out, self.value.time(), date_time, time),
            None => out.extend_from_slice(&[SPACE; TIMESTAMP_WIDTH - DATE_WIDTH]),
        }
    }
}

/// Appends `<date_time>HH<time>MM<time>SS.ffffff`, the 16 bytes after the date.
fn push_clock(out: &mut Vec<u8>, clock: NaiveTime, date_time: u8, time: u8) {
    out.push(date_time);
    push_digits(out, u128::from(clock.hour()), 2);
    out.push(time);
    push_digits(out, u128::from(clock.minute()), 2);
    out.push(time);
    push_digits(out, u128::from(clock.second()), 2);
    out.push(b'.');
    push_digits(out, u128::from(clock.nanosecond() / NANOS_PER_MICRO), 6);
}

/// A `PIC X(26)` timestamp: `TRAN-ORIG-TS`, `TRAN-PROC-TS` (`CVTRA05Y.cpy:16-17`) and their
/// `DALYTRAN-` twins (`CVTRA06Y.cpy:16-17`) (Typed Dates and Timestamps, E33).
///
/// Twenty-six spaces are `None` (PIC-017; every `DALYTRAN-PROC-TS` of the fixture). A non-blank
/// value is in one of the layouts of [`TsForm`] (PIC-018 to PIC-020) and re-encodes to the same
/// bytes; every other text is an error (PIC-021).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PicTimestamp(Option<CobolTimestamp>);

impl PicTimestamp {
    /// Field width in bytes.
    pub const WIDTH: usize = TIMESTAMP_WIDTH;

    /// Twenty-six spaces.
    pub const BLANK: Self = Self(None);

    /// The timestamp `value`, or a blank field for `None`.
    #[must_use]
    pub const fn new(value: Option<CobolTimestamp>) -> Self {
        Self(value)
    }

    /// The timestamp, `None` for a blank field.
    #[must_use]
    pub fn value(&self) -> Option<CobolTimestamp> {
        self.0
    }

    /// Decodes 26 bytes. The layout is told from the bytes after the date: all spaces is `S`;
    /// otherwise a space at offset 10 is `I` and a `-` there is `D`. Every digit and separator
    /// of that layout is then checked.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] when `bytes` is not 26 bytes long;
    /// [`ErrorKind::UnknownTimestampLayout`] when offset 10 matches no layout;
    /// [`ErrorKind::InvalidTimestamp`] for a known layout with a wrong separator, a non-digit or
    /// blank part, or an impossible date or time such as hour 24 or second 60 (PIC-021, R7).
    pub fn decode(bytes: &[u8]) -> Result<Self, ErrorKind> {
        let field =
            <[u8; TIMESTAMP_WIDTH]>::try_from(bytes).map_err(|_| ErrorKind::WidthMismatch {
                expected: TIMESTAMP_WIDTH,
                actual: bytes.len(),
            })?;
        if is_blank(&field) {
            return Ok(Self::BLANK);
        }
        let form = detect_form(&field)?;
        let value = match form.clock_separators() {
            Some((date_time, time)) => parse_clock_text(field, date_time, time),
            None => parse_screen_date(&field),
        }
        .ok_or(ErrorKind::InvalidTimestamp)?;
        Ok(Self(Some(CobolTimestamp { value, form })))
    }

    /// Appends the value's 26 bytes in its layout, or 26 spaces for a blank field.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self.0 {
            Some(timestamp) => timestamp.encode_into(out),
            None => out.extend_from_slice(&[SPACE; TIMESTAMP_WIDTH]),
        }
    }
}

/// The layout of a non-blank timestamp, told from the bytes after its date.
fn detect_form(field: &[u8; TIMESTAMP_WIDTH]) -> Result<TsForm, ErrorKind> {
    let (_, after_date) = field
        .split_first_chunk::<DATE_WIDTH>()
        .ok_or(ErrorKind::UnknownTimestampLayout)?;
    if is_blank(after_date) {
        return Ok(TsForm::ScreenDate);
    }
    match after_date.first() {
        Some(&b' ') => Ok(TsForm::Iso),
        Some(&b'-') => Ok(TsForm::Db2),
        _ => Err(ErrorKind::UnknownTimestampLayout),
    }
}

/// `YYYY-MM-DD`, then `<date_time>HH<time>MM<time>SS.ffffff`, by fixed position.
fn parse_clock_text(
    field: [u8; TIMESTAMP_WIDTH],
    date_time: u8,
    time: u8,
) -> Option<NaiveDateTime> {
    let [
        date @ ..,
        sep_date,
        hh1,
        hh2,
        sep_hour,
        mm1,
        mm2,
        sep_minute,
        ss1,
        ss2,
        b'.',
        f1,
        f2,
        f3,
        f4,
        f5,
        f6,
    ] = field
    else {
        return None;
    };
    if sep_date != date_time || sep_hour != time || sep_minute != time {
        return None;
    }
    let clock = NaiveTime::from_hms_micro_opt(
        digits_u32([hh1, hh2])?,
        digits_u32([mm1, mm2])?,
        digits_u32([ss1, ss2])?,
        digits_u32([f1, f2, f3, f4, f5, f6])?,
    )?;
    parse_date_text(date).map(|day| day.and_time(clock))
}

/// The `YYYY-MM-DD` date of a form-`S` field, whose 16 trailing spaces [`detect_form`] has
/// checked, as midnight of that date.
fn parse_screen_date(field: &[u8; TIMESTAMP_WIDTH]) -> Option<NaiveDateTime> {
    let (date, _) = field.split_first_chunk::<DATE_WIDTH>()?;
    parse_date_text(*date).map(|day| day.and_time(NaiveTime::MIN))
}

#[cfg(test)]
mod tests {
    use super::{
        CobolTimestamp, Count, ErrorKind, NaiveDate, NaiveDateTime, NaiveTime, PicDate, PicS,
        PicTimestamp, PicU, PicX, TsForm, digit_count, parse_digits, push_digits,
    };

    fn date(year: i32, month: u32, day: u32) -> Result<NaiveDate, ErrorKind> {
        NaiveDate::from_ymd_opt(year, month, day).ok_or(ErrorKind::InvalidDate)
    }

    fn date_time(
        day: NaiveDate,
        hms: (u32, u32, u32),
        micro: u32,
    ) -> Result<NaiveDateTime, ErrorKind> {
        let (hour, minute, second) = hms;
        NaiveTime::from_hms_micro_opt(hour, minute, second, micro)
            .map(|clock| day.and_time(clock))
            .ok_or(ErrorKind::InvalidTimestamp)
    }

    fn digits(magnitude: u128, count: u32) -> Vec<u8> {
        let mut out = b"<".to_vec();
        push_digits(&mut out, magnitude, count);
        out
    }

    fn encoded_date(value: PicDate) -> Vec<u8> {
        let mut out = Vec::new();
        value.encode_into(&mut out);
        out
    }

    fn encoded_timestamp(value: &PicTimestamp) -> Vec<u8> {
        let mut out = Vec::new();
        value.encode_into(&mut out);
        out
    }

    /// Decodes `text` and checks that it re-encodes to the same bytes.
    fn timestamp_round_trip(text: &[u8]) -> Result<Option<CobolTimestamp>, ErrorKind> {
        let value = PicTimestamp::decode(text)?;
        assert_eq!(encoded_timestamp(&value), text);
        Ok(value.value())
    }

    #[test]
    fn digit_helpers() {
        assert_eq!(digit_count(11), 11);
        assert_eq!(parse_digits(b""), Ok(0));
        assert_eq!(parse_digits(b"0012345"), Ok(12_345));
        for bad in [b"12a4".as_slice(), b" 1", b"+1", b"1-"] {
            assert_eq!(parse_digits(bad), Err(ErrorKind::NonDigit));
        }
        assert_eq!(parse_digits(&[b'9'; 40]), Err(ErrorKind::OutOfRange));
        assert_eq!(parse_digits(&[b'9'; 38]), Ok(10_u128.pow(38) - 1));
        assert_eq!(digits(7, 3), b"<007");
        assert_eq!(digits(12_345, 3), b"<345");
        assert_eq!(digits(5, 0), b"<");
        assert_eq!(
            digits(u128::MAX, 39),
            b"<340282366920938463463374607431768211455"
        );
    }

    #[test]
    fn pic_x_keeps_printable_bytes_verbatim() -> Result<(), ErrorKind> {
        let printable: Vec<u8> = (b' '..=b'~').collect();
        let field = PicX::<95>::decode(&printable)?;
        assert_eq!(field.as_bytes().as_slice(), printable.as_slice());
        let mut out = b"#".to_vec();
        field.encode_into(&mut out);
        assert_eq!(out.get(1..), Some(printable.as_slice()));
        assert_eq!(PicX::<95>::WIDTH, 95);
        Ok(())
    }

    #[test]
    fn pic_x_rejects_every_non_printable_byte() {
        let rejected = (0..=u8::MAX).filter(|byte| !(b' '..=b'~').contains(byte));
        let mut count = 0;
        for byte in rejected {
            assert_eq!(
                PicX::<3>::decode(&[b'A', byte, b' ']),
                Err(ErrorKind::NonPrintable)
            );
            count += 1;
        }
        assert_eq!(count, 161);
    }

    #[test]
    fn pic_x_width_mismatch() {
        let cases: [(&[u8], usize); 3] = [(b"", 0), (b"AB", 2), (b"ABCD", 4)];
        for (bytes, actual) in cases {
            assert_eq!(
                PicX::<3>::decode(bytes),
                Err(ErrorKind::WidthMismatch {
                    expected: 3,
                    actual
                })
            );
        }
    }

    #[test]
    fn pic_x_text_and_move() -> Result<(), ErrorKind> {
        assert_eq!(PicX::<5>::from_text("AB")?.as_bytes(), b"AB   ");
        assert_eq!(PicX::<5>::from_text("")?, PicX::<5>::spaces());
        assert_eq!(PicX::<5>::from_text("ABCDE")?.as_bytes(), b"ABCDE");
        assert_eq!(
            PicX::<5>::from_text("ABCDEF"),
            Err(ErrorKind::WidthMismatch {
                expected: 5,
                actual: 6
            })
        );
        assert_eq!(
            PicX::<5>::from_text("caf\u{e9}"),
            Err(ErrorKind::NonPrintable)
        );
        assert_eq!(PicX::<5>::from_text("A\tB"), Err(ErrorKind::NonPrintable));
        assert_eq!(PicX::<4>::moved_from(b"AB").as_bytes(), b"AB  ");
        assert_eq!(PicX::<4>::moved_from(b"ABCDEFG").as_bytes(), b"ABCD");
        assert_eq!(PicX::<4>::moved_from(b"").as_bytes(), b"    ");
        Ok(())
    }

    #[test]
    fn pic_x_views_order_and_debug() {
        let value = PicX::from_array(*b"AB C  ");
        assert_eq!(value.trim_end(), b"AB C");
        assert_eq!(value.to_text(), "AB C");
        assert!(!value.is_spaces());
        assert_eq!(PicX::<3>::spaces().trim_end(), b"");
        assert!(PicX::<3>::spaces().is_spaces());
        assert_eq!(PicX::<3>::default(), PicX::<3>::spaces());
        assert_eq!(PicX::from_array(*b"A\t ").trim_end(), b"A\t");
        assert!(PicX::from_array(*b"A ") < PicX::from_array(*b"AB"));
        assert!(PicX::from_array(*b"AB") < PicX::from_array(*b"B "));
        assert_eq!(
            format!("{:?}", PicX::from_array(*b"A\"\x01")),
            "PicX(\"A\\\"\\x01\")"
        );
    }

    #[test]
    fn pic_u_round_trip_restores_leading_zeros() -> Result<(), ErrorKind> {
        let key = PicU::<11>::decode(b"00000000001")?;
        assert_eq!(key.get(), 1);
        let mut out = Vec::new();
        key.encode_into(&mut out);
        assert_eq!(out, b"00000000001");

        let mut short = Vec::new();
        PicU::<5>::new(42)?.encode_into(&mut short);
        assert_eq!(short, b"00042");
        assert_eq!(PicU::<5>::ZERO, PicU::<5>::default());
        assert_eq!(u64::from(PicU::<5>::new(42)?), 42);
        Ok(())
    }

    #[test]
    fn pic_u_rejects_non_digits_and_wrong_widths() {
        for bad in [
            b"0000000000 ".as_slice(),
            b"           ",
            b"+0000000001",
            b"0000000000A",
            b"-0000000001",
        ] {
            assert_eq!(PicU::<11>::decode(bad), Err(ErrorKind::NonDigit));
        }
        assert_eq!(
            PicU::<4>::decode(b"123"),
            Err(ErrorKind::WidthMismatch {
                expected: 4,
                actual: 3
            })
        );
        assert_eq!(
            PicU::<4>::decode(b"12345"),
            Err(ErrorKind::WidthMismatch {
                expected: 4,
                actual: 5
            })
        );
    }

    #[test]
    fn pic_u_range_and_successor() -> Result<(), ErrorKind> {
        assert_eq!(PicU::<11>::MAX, 99_999_999_999);
        assert_eq!(PicU::<11>::new(99_999_999_999)?.get(), 99_999_999_999);
        assert_eq!(PicU::<11>::new(100_000_000_000), Err(ErrorKind::OutOfRange));
        assert_eq!(PicU::<3>::new(5)?.succ()?.get(), 6);
        assert_eq!(PicU::<3>::new(999)?.succ(), Err(ErrorKind::OutOfRange));
        assert_eq!(PicU::<3>::try_from(999_u64)?.get(), 999);
        assert_eq!(PicU::<3>::try_from(1_000_u64), Err(ErrorKind::OutOfRange));
        assert_eq!(PicU::<3>::try_from(7_i64)?.get(), 7);
        assert_eq!(PicU::<3>::try_from(-1_i64), Err(ErrorKind::OutOfRange));
        assert_eq!(PicU::<3>::try_from(1_000_i64), Err(ErrorKind::OutOfRange));
        Ok(())
    }

    #[test]
    fn pic_u_eighteen_digits() -> Result<(), ErrorKind> {
        let nines = [b'9'; 18];
        let widest = PicU::<18>::decode(&nines)?;
        assert_eq!(widest.get(), 999_999_999_999_999_999);
        assert_eq!(widest.get(), PicU::<18>::MAX);
        let mut out = Vec::new();
        widest.encode_into(&mut out);
        assert_eq!(out, nines);
        assert_eq!(widest.succ(), Err(ErrorKind::OutOfRange));
        Ok(())
    }

    #[test]
    fn pic_s_range_and_account_key_join() -> Result<(), ErrorKind> {
        assert_eq!(PicS::<11>::MAX, 99_999_999_999);
        assert_eq!(PicS::<11>::new(99_999_999_999)?.get(), 99_999_999_999);
        assert_eq!(PicS::<11>::new(-99_999_999_999)?.get(), -99_999_999_999);
        assert_eq!(PicS::<11>::new(100_000_000_000), Err(ErrorKind::OutOfRange));
        assert_eq!(
            PicS::<11>::new(-100_000_000_000),
            Err(ErrorKind::OutOfRange)
        );
        assert_eq!(PicS::<11>::default().get(), 0);

        let account = PicU::<11>::new(50)?;
        let signed = PicS::<11>::from(account);
        assert_eq!(signed.get(), 50);
        assert_eq!(PicU::<11>::try_from(signed)?, account);
        assert_eq!(
            PicS::<18>::from(PicU::<18>::new(PicU::<18>::MAX)?).get(),
            999_999_999_999_999_999
        );
        assert_eq!(
            PicU::<11>::try_from(PicS::<11>::new(-1)?),
            Err(ErrorKind::OutOfRange)
        );
        Ok(())
    }

    #[test]
    fn pic_s_inverted_keys() -> Result<(), ErrorKind> {
        assert_eq!(PicS::<5>::complement_from(0)?.get(), 99_999);
        assert_eq!(PicS::<5>::complement_from(99_999)?.get(), 0);
        assert_eq!(
            PicS::<5>::complement_from(100_000),
            Err(ErrorKind::OutOfRange)
        );
        assert_eq!(PicS::<5>::complement_from(25_160)?.get(), 74_839);
        let time_key = PicS::<9>::complement_from(193_027_123)?;
        assert_eq!(time_key.get(), 806_972_876);
        assert_eq!(time_key.uncomplement(), Ok(193_027_123));
        for value in [0, 1, 54_321, 99_999] {
            assert_eq!(PicS::<5>::complement_from(value)?.uncomplement(), Ok(value));
        }
        assert_eq!(
            PicS::<5>::new(-1)?.uncomplement(),
            Err(ErrorKind::OutOfRange)
        );
        Ok(())
    }

    #[test]
    fn count_holds_every_halfword_and_steps_within_four_digits() -> Result<(), ErrorKind> {
        assert_eq!(Count::ZERO.add_one()?.get(), 1);
        assert_eq!(Count::new(23).add_one()?, Count::new(24));
        assert_eq!(Count::new(1).sub_one()?, Count::ZERO);
        assert_eq!(Count::ZERO.sub_one()?.get(), -1);
        assert_eq!(Count::new(9_998).add_one()?.get(), Count::MAX);
        assert_eq!(Count::new(9_999).add_one(), Err(ErrorKind::OutOfRange));
        assert_eq!(Count::new(-9_999).sub_one(), Err(ErrorKind::OutOfRange));
        assert_eq!(Count::new(i16::MAX).add_one(), Err(ErrorKind::OutOfRange));
        assert_eq!(Count::new(i16::MIN).sub_one(), Err(ErrorKind::OutOfRange));
        assert_eq!(Count::new(10_000).sub_one()?.get(), 9_999);
        assert_eq!(i16::from(Count::from(i16::MIN)), i16::MIN);
        assert_eq!(Count::default(), Count::ZERO);
        Ok(())
    }

    #[test]
    fn pic_date_round_trips() -> Result<(), ErrorKind> {
        for (text, expected) in [
            (b"2014-11-20", date(2014, 11, 20)?),
            (b"2025-05-20", date(2025, 5, 20)?),
            (b"0000-01-01", date(0, 1, 1)?),
            (b"9999-12-31", date(9999, 12, 31)?),
            (b"2024-02-29", date(2024, 2, 29)?),
        ] {
            let value = PicDate::decode(text)?;
            assert_eq!(value.value(), Some(expected));
            assert_eq!(encoded_date(value), text);
        }
        let blank = PicDate::decode(&[b' '; 10])?;
        assert_eq!(blank, PicDate::BLANK);
        assert_eq!(blank.value(), None);
        assert_eq!(encoded_date(blank), [b' '; 10]);
        assert_eq!(PicDate::default(), PicDate::BLANK);
        assert_eq!(PicDate::WIDTH, 10);
        Ok(())
    }

    #[test]
    fn pic_date_rejects_other_text() {
        for bad in [
            b"2022-02-30",
            b"2022-2-03 ",
            b"2022-02-  ",
            b"     02-03",
            b"2022/01/01",
            b"2022-13-01",
            b"2022-00-10",
            b"22-01-01  ",
            b"2022-01-1A",
            b"2022-1A-01",
            b"+022-01-01",
        ] {
            assert_eq!(PicDate::decode(bad), Err(ErrorKind::InvalidDate));
        }
        assert_eq!(
            PicDate::decode(b"2022-01-0"),
            Err(ErrorKind::WidthMismatch {
                expected: 10,
                actual: 9
            })
        );
        assert_eq!(
            PicDate::decode(b"2022-01-011"),
            Err(ErrorKind::WidthMismatch {
                expected: 10,
                actual: 11
            })
        );
    }

    #[test]
    fn pic_date_new_checks_the_year() -> Result<(), ErrorKind> {
        assert_eq!(PicDate::new(None)?, PicDate::BLANK);
        let first = PicDate::new(Some(date(0, 1, 1)?))?;
        assert_eq!(encoded_date(first), b"0000-01-01");
        assert_eq!(
            PicDate::new(Some(date(10_000, 1, 1)?)),
            Err(ErrorKind::OutOfRange)
        );
        assert_eq!(
            PicDate::new(Some(date(-1, 12, 31)?)),
            Err(ErrorKind::OutOfRange)
        );
        Ok(())
    }

    #[test]
    fn ts_form_codes() {
        for form in [TsForm::Iso, TsForm::Db2, TsForm::ScreenDate] {
            assert_eq!(TsForm::from_code(form.code()), Ok(form));
        }
        assert_eq!(TsForm::Iso.code(), 'I');
        assert_eq!(TsForm::Db2.code(), 'D');
        assert_eq!(TsForm::ScreenDate.code(), 'S');
        for bad in ['X', 'i', ' ', '\u{e9}'] {
            assert_eq!(TsForm::from_code(bad), Err(ErrorKind::UnknownFormCode));
        }
    }

    #[test]
    fn pic_timestamp_layouts_round_trip() -> Result<(), ErrorKind> {
        let iso = timestamp_round_trip(b"2022-06-10 19:27:53.000000")?;
        assert_eq!(iso.map(|ts| ts.form()), Some(TsForm::Iso));
        assert_eq!(
            iso.map(|ts| ts.value()),
            Some(date_time(date(2022, 6, 10)?, (19, 27, 53), 0)?)
        );

        let db2 = timestamp_round_trip(b"2022-07-18-10.15.30.120000")?;
        assert_eq!(db2.map(|ts| ts.form()), Some(TsForm::Db2));
        assert_eq!(
            db2.map(|ts| ts.value()),
            Some(date_time(date(2022, 7, 18)?, (10, 15, 30), 120_000)?)
        );

        let screen = timestamp_round_trip(b"2022-06-10                ")?;
        assert_eq!(screen.map(|ts| ts.form()), Some(TsForm::ScreenDate));
        assert_eq!(
            screen.map(|ts| ts.value()),
            Some(date_time(date(2022, 6, 10)?, (0, 0, 0), 0)?)
        );

        assert_eq!(timestamp_round_trip(&[b' '; 26])?, None);
        assert_eq!(PicTimestamp::default(), PicTimestamp::BLANK);
        assert_eq!(PicTimestamp::WIDTH, 26);
        let precise = timestamp_round_trip(b"0000-01-01 23:59:59.999999")?;
        assert_eq!(
            precise.map(|ts| ts.value()),
            Some(date_time(date(0, 1, 1)?, (23, 59, 59), 999_999)?)
        );
        Ok(())
    }

    #[test]
    fn pic_timestamp_rejects_unknown_layouts() {
        for bad in [
            b"2022-06-10T19:27:53.000000",
            b"2022-06-10.19.27.53.000000",
            b"2022-06-10:               ",
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        ] {
            assert_eq!(
                PicTimestamp::decode(bad),
                Err(ErrorKind::UnknownTimestampLayout)
            );
        }
    }

    #[test]
    fn pic_timestamp_rejects_invalid_text_in_a_known_layout() {
        for bad in [
            b"2022-06-10 19.27.53.000000",
            b"2022-06-10-19:27:53.000000",
            b"2022-06-10 19:27:53,000000",
            b"2022-06-10 24:00:00.000000",
            b"2022-06-10 23:60:00.000000",
            b"2022-06-10 23:59:60.000000",
            b"2022-06-10 19:27:53.0000  ",
            b"2022-06-10 19:27:5 .000000",
            b"2022-06-10 1A:27:53.000000",
            b"2022-06-10 19:2A:53.000000",
            b"2022-06-10 19:27:53.00000A",
            b"2022-02-30 19:27:53.000000",
            b"2022-02-30-19.27.53.000000",
            b"2022-02-30                ",
            b"2022-06-                  ",
            b"           19:27:53.000000",
            b"2022/06/10 19:27:53.000000",
        ] {
            assert_eq!(PicTimestamp::decode(bad), Err(ErrorKind::InvalidTimestamp));
        }
        assert_eq!(
            PicTimestamp::decode(b"2022-06-10 19:27:53.00000"),
            Err(ErrorKind::WidthMismatch {
                expected: 26,
                actual: 25
            })
        );
        assert_eq!(
            PicTimestamp::decode(b""),
            Err(ErrorKind::WidthMismatch {
                expected: 26,
                actual: 0
            })
        );
    }

    #[test]
    fn cobol_timestamp_new_checks_what_the_text_can_carry() -> Result<(), ErrorKind> {
        let day = date(2022, 6, 10)?;
        let noon = date_time(day, (12, 0, 0), 0)?;
        assert_eq!(
            CobolTimestamp::new(noon, TsForm::ScreenDate),
            Err(ErrorKind::InvalidTimestamp)
        );
        let midnight = date_time(day, (0, 0, 0), 0)?;
        let screen = CobolTimestamp::new(midnight, TsForm::ScreenDate)?;
        assert_eq!(
            encoded_timestamp(&PicTimestamp::new(Some(screen))),
            b"2022-06-10                "
        );

        let sub_micro = NaiveTime::from_hms_nano_opt(12, 0, 0, 1_500)
            .map(|clock| day.and_time(clock))
            .ok_or(ErrorKind::InvalidTimestamp)?;
        assert_eq!(
            CobolTimestamp::new(sub_micro, TsForm::Iso),
            Err(ErrorKind::OutOfRange)
        );
        let leap = NaiveTime::from_hms_nano_opt(23, 59, 59, 1_000_000_000)
            .map(|clock| day.and_time(clock))
            .ok_or(ErrorKind::InvalidTimestamp)?;
        assert_eq!(
            CobolTimestamp::new(leap, TsForm::Db2),
            Err(ErrorKind::OutOfRange)
        );
        for year in [-1, 10_000] {
            let far = date_time(date(year, 1, 1)?, (0, 0, 0), 0)?;
            assert_eq!(
                CobolTimestamp::new(far, TsForm::Iso),
                Err(ErrorKind::OutOfRange)
            );
        }
        Ok(())
    }

    #[test]
    fn cobol_timestamp_encodes_in_its_layout() -> Result<(), ErrorKind> {
        let value = date_time(date(2022, 7, 18)?, (9, 5, 7), 123_456)?;
        for (form, text) in [
            (TsForm::Iso, b"2022-07-18 09:05:07.123456"),
            (TsForm::Db2, b"2022-07-18-09.05.07.123456"),
        ] {
            let timestamp = CobolTimestamp::new(value, form)?;
            assert_eq!(timestamp.value(), value);
            assert_eq!(timestamp.form(), form);
            let field = PicTimestamp::new(Some(timestamp));
            assert_eq!(field.value(), Some(timestamp));
            assert_eq!(encoded_timestamp(&field), text);
            assert_eq!(PicTimestamp::decode(text)?, field);
        }
        assert_eq!(encoded_timestamp(&PicTimestamp::BLANK), [b' '; 26]);
        Ok(())
    }
}
