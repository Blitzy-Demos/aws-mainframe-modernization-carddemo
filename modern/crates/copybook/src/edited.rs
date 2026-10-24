//! Numeric-edited output pictures: a COBOL `MOVE` of a numeric value to a numeric-edited
//! receiver, for every edited picture the in-scope sources declare (AAP 0.5.4).
//!
//! Money and amounts leave the services as strings laid out by the COBOL output picture, so no
//! consumer formats a number itself (AAP 0.3.4.1). [`EditedPicture::format`] reproduces the IBM
//! Enterprise COBOL editing rules for the symbols these pictures use:
//!
//! - `9` prints its digit; `Z` prints its digit or, while leading-zero suppression is in effect,
//!   a space; suppression ends at the first nonzero digit, the first `9` or the decimal point.
//! - `,` prints a comma, or a space while suppression is in effect; `.` always prints.
//! - When every digit position is `Z` and the value is zero, the whole field is spaces, sign and
//!   point included; with a nonzero value, `Z` positions after the point print their digits.
//! - A fixed `+` prints `+` or `-`; a fixed leading or trailing `-` prints a space or `-`.
//! - In a floating `-` string the leftmost symbol is the sign position and the others are digit
//!   positions suppressed like `Z`; the sign, `-` or a space, lands immediately left of the
//!   first printed digit or point, and every position left of it is a space.
//!
//! The value is stored by the COBOL `MOVE` rule, which no source in the repository modifies with
//! `ROUNDED` or `ON SIZE ERROR` (AAP 0.3.3.2): the fraction is truncated toward zero at the
//! picture's scale and integer digits beyond its integer positions are dropped (R7: never
//! rounded). A negative value none of whose kept digits is nonzero edits as zero without a minus
//! sign, as `truncated_digits` stores it; the COBOL result of that case is not fixed by any
//! repository source and is not claimed as parity (`X-ARITH-7` in `modern/audit/arithmetic.md`).
//!
//! Formatting only: de-editing input (`FUNCTION NUMVAL`) belongs to `domain`, so this module
//! parses nothing and adds no untrusted-input surface. `PIC-022` and `CCPAURQY-009` record that
//! the `+9(10).99` fields are only rendered here: the request token never passes through that
//! picture. Values are amounts, counts and SQL codes; no sensitive field is ever edited
//! (Cardholder Data Redaction).

use rust_decimal::Decimal;

use crate::zoned::{Digits, truncated_digits};

/// A numeric-edited picture an in-scope program, copybook or map declares. Lowercase `z` in a
/// source picture behaves exactly as `Z`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EditedPicture {
    /// `PIC -zzzzzzzzz9.99`, 14 characters: `WS-APPROVED-AMT-DIS` (`COPAUA0C.cbl:66`), the
    /// amount the `CP00` reply STRING emits instead of `CCPAURLY`'s declared `+9(10).99`
    /// (E03; `ARITH-039`, `PIC-022`).
    MinusZ10Dot99,
    /// `PIC +9(10).99` ≡ `+9999999999.99`, 14 characters: `PA-RL-APPROVED-AMT`
    /// (`CCPAURLY.cpy:24`), `PA-RQ-TRANSACTION-AMT` (`CCPAURQY.cpy:27`) (`PIC-022`), and
    /// `WS-CURR-BAL` (`COBIL00C.cbl:56`, `ARITH-019`).
    Plus9x10Dot99,
    /// `PIC -zzzzzzz9.99`, 12 characters: `WS-AUTH-AMT`, `WS-DISPLAY-AMT12`
    /// (`COPAUS0C.cbl:55-56`) and `WS-AUTH-AMT` (`COPAUS1C.cbl:52`).
    MinusZ8Dot99,
    /// `PIC -zzzz9.99`, 9 characters: `WS-DISPLAY-AMT9` (`COPAUS0C.cbl:57`).
    MinusZ5Dot99,
    /// `PIC +99999999.99`, 12 characters: `WS-TRAN-AMT` (`COBIL00C.cbl:55`,
    /// `CORPT00C.cbl:77`, `COTRN00C.cbl:56`, `COTRN01C.cbl:49`, `COTRN02C.cbl:53`) and
    /// `WS-TRAN-AMT-E` (`COTRN02C.cbl:59`).
    Plus9x8Dot99,
    /// `PIC +ZZZ,ZZZ,ZZZ.99`, 15 characters: `WS-EDIT-CURRENCY-9-2-F` (`COACTUPC.cbl:371`,
    /// `COTRTUPC.cbl:118`) and the `PICOUT` of the five account-view amounts
    /// (`COACTVW.bms:120,141,162,174,195`; `COACTVW.CPY:302,314,326,332,344`; `COACTVW-022`,
    /// `COACTVW-028`, `COACTVW-034`, `COACTVW-037`, `COACTVW-043`).
    PlusZCommas9Dot99,
    /// `PIC Z(9).99-` ≡ `ZZZZZZZZZ.99-`, 13 characters: `ST-TRANAMT` and `ST-TOTAL-TRAMT`
    /// (`CBSTM03A.CBL:137,142`; `FIXSTMT-902`).
    Z9Dot99Minus,
    /// `PIC 9(9).99-` ≡ `999999999.99-`, 13 characters: `ST-CURR-BAL` (`CBSTM03A.CBL:113`;
    /// `FIXSTMT-902`).
    Nines9Dot99Minus,
    /// `PIC -ZZZ,ZZZ,ZZZ.ZZ`, 15 characters: `TRAN-REPORT-AMT` (`CVTRA07Y.cpy:30`;
    /// `FIXRPT-001`).
    MinusZCommas9DotZZ,
    /// `PIC +ZZZ,ZZZ,ZZZ.ZZ`, 15 characters: `REPT-PAGE-TOTAL`, `REPT-ACCOUNT-TOTAL` and
    /// `REPT-GRAND-TOTAL` (`CVTRA07Y.cpy:54,60,66`; `FIXRPT-001`).
    PlusZCommas9DotZZ,
    /// `PIC ----9`, 5 characters: `WS-VAR-SQLCODE` (`COBTUPDT.cbl:65`) and `WS-DISP-SQLCODE`
    /// (`COTRTUPC.cbl:68`, `CSDB2RWY.cpy:22`).
    FloatMinus5,
    /// `PIC +9(06)` ≡ `+999999`, 7 characters: `WS-SQLCODE` (`COPAUS2C.cbl:55`).
    Plus9x6,
    /// `PIC +9(09)` ≡ `+999999999`, 10 characters: `WS-SQLSTATE` (`COPAUS2C.cbl:56`).
    Plus9x9,
}

/// One character position of a numeric-edited picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Symbol {
    /// A leading `+`: `+` for a zero or positive value, `-` for a negative one.
    FixedPlus,
    /// A single leading `-`: a space for a zero or positive value, `-` for a negative one.
    FixedMinus,
    /// A trailing `-`, edited as [`Symbol::FixedMinus`].
    TrailingMinus,
    /// One `-` of a floating insertion string: the first is the sign position, the rest are
    /// digit positions.
    FloatMinus,
    /// A digit position with leading-zero suppression to spaces.
    Z,
    /// A digit position that always prints its digit.
    Nine,
    /// A simple-insertion comma.
    Comma,
    /// The actual decimal point.
    Point,
}

use Symbol::{Comma, FixedMinus, FixedPlus, FloatMinus, Nine, Point, TrailingMinus, Z};

/// `-zzzzzzzzz9.99`.
const MINUS_Z10_DOT_99: &[Symbol] = &[
    FixedMinus, Z, Z, Z, Z, Z, Z, Z, Z, Z, Nine, Point, Nine, Nine,
];

/// `+9(10).99`.
const PLUS_9X10_DOT_99: &[Symbol] = &[
    FixedPlus, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Point, Nine, Nine,
];

/// `-zzzzzzz9.99`.
const MINUS_Z8_DOT_99: &[Symbol] = &[FixedMinus, Z, Z, Z, Z, Z, Z, Z, Nine, Point, Nine, Nine];

/// `-zzzz9.99`.
const MINUS_Z5_DOT_99: &[Symbol] = &[FixedMinus, Z, Z, Z, Z, Nine, Point, Nine, Nine];

/// `+99999999.99`.
const PLUS_9X8_DOT_99: &[Symbol] = &[
    FixedPlus, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Point, Nine, Nine,
];

/// `+ZZZ,ZZZ,ZZZ.99`.
const PLUS_Z_COMMAS_9_DOT_99: &[Symbol] = &[
    FixedPlus, Z, Z, Z, Comma, Z, Z, Z, Comma, Z, Z, Z, Point, Nine, Nine,
];

/// `Z(9).99-`.
const Z9_DOT_99_MINUS: &[Symbol] = &[Z, Z, Z, Z, Z, Z, Z, Z, Z, Point, Nine, Nine, TrailingMinus];

/// `9(9).99-`.
const NINES9_DOT_99_MINUS: &[Symbol] = &[
    Nine,
    Nine,
    Nine,
    Nine,
    Nine,
    Nine,
    Nine,
    Nine,
    Nine,
    Point,
    Nine,
    Nine,
    TrailingMinus,
];

/// `-ZZZ,ZZZ,ZZZ.ZZ`.
const MINUS_Z_COMMAS_9_DOT_ZZ: &[Symbol] = &[
    FixedMinus, Z, Z, Z, Comma, Z, Z, Z, Comma, Z, Z, Z, Point, Z, Z,
];

/// `+ZZZ,ZZZ,ZZZ.ZZ`.
const PLUS_Z_COMMAS_9_DOT_ZZ: &[Symbol] = &[
    FixedPlus, Z, Z, Z, Comma, Z, Z, Z, Comma, Z, Z, Z, Point, Z, Z,
];

/// `----9`.
const FLOAT_MINUS_5: &[Symbol] = &[FloatMinus, FloatMinus, FloatMinus, FloatMinus, Nine];

/// `+9(06)`.
const PLUS_9X6: &[Symbol] = &[FixedPlus, Nine, Nine, Nine, Nine, Nine, Nine];

/// `+9(09)`.
const PLUS_9X9: &[Symbol] = &[
    FixedPlus, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Nine, Nine,
];

impl EditedPicture {
    /// The picture's character positions, left to right.
    const fn symbols(self) -> &'static [Symbol] {
        match self {
            Self::MinusZ10Dot99 => MINUS_Z10_DOT_99,
            Self::Plus9x10Dot99 => PLUS_9X10_DOT_99,
            Self::MinusZ8Dot99 => MINUS_Z8_DOT_99,
            Self::MinusZ5Dot99 => MINUS_Z5_DOT_99,
            Self::Plus9x8Dot99 => PLUS_9X8_DOT_99,
            Self::PlusZCommas9Dot99 => PLUS_Z_COMMAS_9_DOT_99,
            Self::Z9Dot99Minus => Z9_DOT_99_MINUS,
            Self::Nines9Dot99Minus => NINES9_DOT_99_MINUS,
            Self::MinusZCommas9DotZZ => MINUS_Z_COMMAS_9_DOT_ZZ,
            Self::PlusZCommas9DotZZ => PLUS_Z_COMMAS_9_DOT_ZZ,
            Self::FloatMinus5 => FLOAT_MINUS_5,
            Self::Plus9x6 => PLUS_9X6,
            Self::Plus9x9 => PLUS_9X9,
        }
    }

    /// Characters in the edited field, the length of its expanded picture string: 14 for
    /// `-zzzzzzzzz9.99`, 5 for `----9`.
    #[must_use]
    pub const fn width(self) -> usize {
        self.symbols().len()
    }

    /// Digit positions after the decimal point: 2 for every amount picture, 0 for `----9`,
    /// `+9(06)` and `+9(09)`.
    #[must_use]
    pub fn scale(self) -> u32 {
        self.digit_positions().1
    }

    /// Digit positions before the decimal point. In a floating `-` string of n symbols the first
    /// is the sign position and the other n − 1 are digit positions, so `----9` has 4.
    fn int_positions(self) -> u32 {
        self.digit_positions().0
    }

    /// Digit positions before and after the decimal point.
    fn digit_positions(self) -> (u32, u32) {
        let mut before = 0_u32;
        let mut after = 0_u32;
        let mut past_point = false;
        let mut sign_position_seen = false;
        for &symbol in self.symbols() {
            let is_digit = match symbol {
                Point => {
                    past_point = true;
                    false
                }
                FloatMinus => {
                    let is_digit = sign_position_seen;
                    sign_position_seen = true;
                    is_digit
                }
                Z | Nine => true,
                FixedPlus | FixedMinus | TrailingMinus | Comma => false,
            };
            match (is_digit, past_point) {
                (true, false) => before = before.saturating_add(1),
                (true, true) => after = after.saturating_add(1),
                (false, _) => {}
            }
        }
        (before, after)
    }

    /// `value` as this picture edits it after a COBOL `MOVE`: exactly [`width`](Self::width)
    /// ASCII characters, for every `Decimal`.
    ///
    /// The value is first stored at the picture's integer positions and scale, fraction
    /// truncated toward zero and high-order digits dropped, then laid out by the editing rules
    /// of the module documentation: `-12.34` is `-0000000012.34` in `+9(10).99`, `1234.56` is
    /// `+      1,234.56` in `+ZZZ,ZZZ,ZZZ.99`, zero is all spaces in `-ZZZ,ZZZ,ZZZ.ZZ`, and
    /// `-100` is ` -100` in `----9`.
    #[must_use]
    pub fn format(self, value: Decimal) -> String {
        let symbols = self.symbols();
        // TRUNCATION: the COBOL MOVE to a numeric-edited receiver drops the fraction digits
        // beyond its scale toward zero and the integer digits beyond its integer positions, and
        // never rounds; `truncated_digits` keeps exactly the digits the picture can show.
        let Digits { negative, digits } =
            truncated_digits(value, self.int_positions(), self.scale());
        let mut out = String::with_capacity(symbols.len());
        // Every digit position is `Z` exactly when the picture has no `9`; a zero value then
        // leaves the whole field, sign and point included, as spaces.
        let is_zero = digits.iter().all(|&digit| digit == 0);
        if is_zero && !symbols.contains(&Nine) {
            for _ in symbols {
                out.push(' ');
            }
            return out;
        }
        let mut editor = Editor {
            negative,
            digits: digits.into_iter(),
            out,
            suppressing: true,
            float_sign: FloatSign::NotSeen,
        };
        for &symbol in symbols {
            editor.edit(symbol);
        }
        editor.out
    }
}

/// Progress of the floating `-` sign through a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FloatSign {
    /// No floating string has started.
    NotSeen,
    /// The sign position has been written as a space and the sign awaits the first printed
    /// digit or point.
    Pending,
    /// The sign has been placed.
    Placed,
}

/// The state of one left-to-right edit of a value into a picture.
struct Editor {
    /// Whether the stored value is negative.
    negative: bool,
    /// The stored digit values, most significant first, one per digit position.
    digits: std::vec::IntoIter<u8>,
    /// The edited characters so far.
    out: String,
    /// Whether leading-zero suppression is still in effect.
    suppressing: bool,
    /// Where the floating sign stands.
    float_sign: FloatSign,
}

impl Editor {
    /// Writes the characters of one picture position.
    fn edit(&mut self, symbol: Symbol) {
        match symbol {
            FixedPlus => {
                let sign = if self.negative { '-' } else { '+' };
                self.out.push(sign);
            }
            FixedMinus | TrailingMinus => {
                let sign = self.minus_or_space();
                self.out.push(sign);
            }
            FloatMinus if self.float_sign == FloatSign::NotSeen => {
                self.float_sign = FloatSign::Pending;
                self.out.push(' ');
            }
            FloatMinus | Z => self.digit(true),
            Nine => self.digit(false),
            Comma => {
                let comma = if self.suppressing { ' ' } else { ',' };
                self.out.push(comma);
            }
            Point => {
                self.end_suppression();
                self.out.push('.');
            }
        }
    }

    /// Writes the next stored digit, or a space for a leading zero in a suppressible position.
    fn digit(&mut self, suppressible: bool) {
        // The picture has exactly one stored digit per digit position, so a digit is always
        // left; the fallback only keeps this free of a panic path.
        let digit = self.digits.next().unwrap_or(0);
        if suppressible && self.suppressing && digit == 0 {
            self.out.push(' ');
        } else {
            self.end_suppression();
            self.out
                .push(char::from_digit(u32::from(digit), 10).unwrap_or('0'));
        }
    }

    /// Ends leading-zero suppression at the first printed digit or point. A pending floating
    /// sign then replaces the space just written, which is the position immediately left of
    /// that digit or point: the sign position itself or the last suppressed floating position.
    fn end_suppression(&mut self) {
        if !self.suppressing {
            return;
        }
        self.suppressing = false;
        if self.float_sign == FloatSign::Pending {
            self.float_sign = FloatSign::Placed;
            let sign = self.minus_or_space();
            self.out.pop();
            self.out.push(sign);
        }
    }

    /// `-` for a negative value, a space otherwise.
    fn minus_or_space(&self) -> char {
        if self.negative { '-' } else { ' ' }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rust_decimal::Decimal;

    use super::EditedPicture::{
        self, FloatMinus5, MinusZ5Dot99, MinusZ8Dot99, MinusZ10Dot99, MinusZCommas9DotZZ,
        Nines9Dot99Minus, Plus9x6, Plus9x8Dot99, Plus9x9, Plus9x10Dot99, PlusZCommas9Dot99,
        PlusZCommas9DotZZ, Z9Dot99Minus,
    };
    use super::Symbol;

    type TestResult = Result<(), Box<dyn Error>>;

    /// Every picture with its PIC string as the source writes it, repetition factors expanded,
    /// and its scale.
    const PICTURES: [(EditedPicture, &str, u32); 13] = [
        (MinusZ10Dot99, "-zzzzzzzzz9.99", 2),
        (Plus9x10Dot99, "+9999999999.99", 2),
        (MinusZ8Dot99, "-zzzzzzz9.99", 2),
        (MinusZ5Dot99, "-zzzz9.99", 2),
        (Plus9x8Dot99, "+99999999.99", 2),
        (PlusZCommas9Dot99, "+ZZZ,ZZZ,ZZZ.99", 2),
        (Z9Dot99Minus, "ZZZZZZZZZ.99-", 2),
        (Nines9Dot99Minus, "999999999.99-", 2),
        (MinusZCommas9DotZZ, "-ZZZ,ZZZ,ZZZ.ZZ", 2),
        (PlusZCommas9DotZZ, "+ZZZ,ZZZ,ZZZ.ZZ", 2),
        (FloatMinus5, "----9", 0),
        (Plus9x6, "+999999", 0),
        (Plus9x9, "+999999999", 0),
    ];

    /// `(picture, value, edited field)`. Per picture: zero, small positive and negative values,
    /// the largest representable value, a value too wide for the picture (high-order digits
    /// dropped), a sub-cent or fractional value (truncated, never rounded), and the audited
    /// examples named in the comments.
    const CASES: &[(EditedPicture, &str, &str)] = &[
        // audit: ARITH-039
        (MinusZ10Dot99, "100.00", "        100.00"),
        (MinusZ10Dot99, "0.00", "          0.00"),
        (MinusZ10Dot99, "-5.25", "-         5.25"),
        (MinusZ10Dot99, "1234567890.12", " 1234567890.12"),
        (MinusZ10Dot99, "0", "          0.00"),
        (MinusZ10Dot99, "-5.5", "-         5.50"),
        (MinusZ10Dot99, "9999999999.99", " 9999999999.99"),
        (MinusZ10Dot99, "12345678901.23", " 2345678901.23"),
        (MinusZ10Dot99, "0.999", "          0.99"),
        (MinusZ10Dot99, "-10000000000.00", "          0.00"),
        // audit: ARITH-019
        (Plus9x10Dot99, "843.00", "+0000000843.00"),
        (Plus9x10Dot99, "0.00", "+0000000000.00"),
        (Plus9x10Dot99, "-25.00", "-0000000025.00"),
        (Plus9x10Dot99, "1234567890.12", "+1234567890.12"),
        (Plus9x10Dot99, "-12.34", "-0000000012.34"),
        (Plus9x10Dot99, "9999999999.99", "+9999999999.99"),
        (Plus9x10Dot99, "-12345678901.23", "-2345678901.23"),
        (Plus9x10Dot99, "0.999", "+0000000000.99"),
        (MinusZ8Dot99, "0", "        0.00"),
        (MinusZ8Dot99, "12.34", "       12.34"),
        (MinusZ8Dot99, "-12.34", "-      12.34"),
        (MinusZ8Dot99, "99999999.99", " 99999999.99"),
        (MinusZ8Dot99, "123456789.01", " 23456789.01"),
        (MinusZ8Dot99, "0.999", "        0.99"),
        (MinusZ5Dot99, "0", "     0.00"),
        (MinusZ5Dot99, "7.05", "     7.05"),
        (MinusZ5Dot99, "-7.05", "-    7.05"),
        (MinusZ5Dot99, "99999.99", " 99999.99"),
        (MinusZ5Dot99, "123456.78", " 23456.78"),
        (MinusZ5Dot99, "-0.999", "-    0.99"),
        (Plus9x8Dot99, "0", "+00000000.00"),
        (Plus9x8Dot99, "504.77", "+00000504.77"),
        (Plus9x8Dot99, "-919.00", "-00000919.00"),
        (Plus9x8Dot99, "99999999.99", "+99999999.99"),
        (Plus9x8Dot99, "123456789.01", "+23456789.01"),
        (Plus9x8Dot99, "0.999", "+00000000.99"),
        (PlusZCommas9Dot99, "0", "+           .00"),
        (PlusZCommas9Dot99, "1234.56", "+      1,234.56"),
        (PlusZCommas9Dot99, "-1234.56", "-      1,234.56"),
        (PlusZCommas9Dot99, "1000.00", "+      1,000.00"),
        (PlusZCommas9Dot99, "0.05", "+           .05"),
        (PlusZCommas9Dot99, "999999999.99", "+999,999,999.99"),
        (PlusZCommas9Dot99, "1234567890.12", "+234,567,890.12"),
        (PlusZCommas9Dot99, "0.999", "+           .99"),
        // audit: FIXSTMT-902
        (Z9Dot99Minus, "183.88", "      183.88 "),
        (Z9Dot99Minus, "-47.88", "       47.88-"),
        (Z9Dot99Minus, "0.99", "         .99 "),
        (Z9Dot99Minus, "1453.87", "     1453.87 "),
        (Z9Dot99Minus, "-132.88", "      132.88-"),
        (Z9Dot99Minus, "0", "         .00 "),
        (Z9Dot99Minus, "0.50", "         .50 "),
        (Z9Dot99Minus, "-1.00", "        1.00-"),
        (Z9Dot99Minus, "999999999.99", "999999999.99 "),
        (Z9Dot99Minus, "-1234567890.12", "234567890.12-"),
        (Z9Dot99Minus, "0.999", "         .99 "),
        // audit: FIXSTMT-902
        (Nines9Dot99Minus, "1945.87", "000001945.87 "),
        (Nines9Dot99Minus, "-125.88", "000000125.88-"),
        (Nines9Dot99Minus, "0", "000000000.00 "),
        (Nines9Dot99Minus, "999999999.99", "999999999.99 "),
        (Nines9Dot99Minus, "1234567890.12", "234567890.12 "),
        (Nines9Dot99Minus, "-0.999", "000000000.99-"),
        // audit: FIXRPT-001
        (MinusZCommas9DotZZ, "183.88", "         183.88"),
        (MinusZCommas9DotZZ, "-47.88", "-         47.88"),
        (MinusZCommas9DotZZ, "0.99", "            .99"),
        (MinusZCommas9DotZZ, "0", "               "),
        (MinusZCommas9DotZZ, "-0.004", "               "),
        (MinusZCommas9DotZZ, "1.00", "           1.00"),
        (MinusZCommas9DotZZ, "-1234567.89", "-  1,234,567.89"),
        (MinusZCommas9DotZZ, "999999999.99", " 999,999,999.99"),
        (MinusZCommas9DotZZ, "-1234567890.12", "-234,567,890.12"),
        (MinusZCommas9DotZZ, "0.999", "            .99"),
        // audit: FIXRPT-001
        (PlusZCommas9DotZZ, "1453.87", "+      1,453.87"),
        (PlusZCommas9DotZZ, "11275.67", "+     11,275.67"),
        (PlusZCommas9DotZZ, "-132.88", "-        132.88"),
        (PlusZCommas9DotZZ, "0", "               "),
        (PlusZCommas9DotZZ, "999999999.99", "+999,999,999.99"),
        (PlusZCommas9DotZZ, "1234567890.12", "+234,567,890.12"),
        (PlusZCommas9DotZZ, "1000000000.00", "               "),
        (PlusZCommas9DotZZ, "0.999", "+           .99"),
        (FloatMinus5, "-100", " -100"),
        (FloatMinus5, "0", "    0"),
        (FloatMinus5, "-1234", "-1234"),
        (FloatMinus5, "12345", " 2345"),
        (FloatMinus5, "-5", "   -5"),
        (FloatMinus5, "100", "  100"),
        (FloatMinus5, "-803", " -803"),
        (FloatMinus5, "9999", " 9999"),
        (FloatMinus5, "-9999", "-9999"),
        (FloatMinus5, "-12345", "-2345"),
        (FloatMinus5, "-0.999", "    0"),
        (FloatMinus5, "-99.9", "  -99"),
        (Plus9x6, "-100", "-000100"),
        (Plus9x6, "0", "+000000"),
        (Plus9x6, "100", "+000100"),
        (Plus9x6, "999999", "+999999"),
        (Plus9x6, "-1234567", "-234567"),
        (Plus9x6, "0.999", "+000000"),
        (Plus9x6, "-0.5", "+000000"),
        (Plus9x9, "0", "+000000000"),
        (Plus9x9, "12345", "+000012345"),
        (Plus9x9, "-803", "-000000803"),
        (Plus9x9, "999999999", "+999999999"),
        (Plus9x9, "1234567890", "+234567890"),
        (Plus9x9, "-0.999", "+000000000"),
    ];

    /// The symbols of an expanded picture string under the IBM classification: a leading `+`
    /// or single leading `-` is a fixed sign, a final `-` a trailing sign, and a run of two or
    /// more `-` a floating string; `None` for any other character.
    fn symbols_of(text: &str) -> Option<Vec<Symbol>> {
        let chars: Vec<char> = text.chars().collect();
        let last = chars.len().saturating_sub(1);
        let is_minus = |position: Option<usize>| {
            position.and_then(|position| chars.get(position)) == Some(&'-')
        };
        chars
            .iter()
            .enumerate()
            .map(|(position, &character)| match character {
                '+' => Some(Symbol::FixedPlus),
                '-' if position == last && position > 0 => Some(Symbol::TrailingMinus),
                '-' if is_minus(position.checked_add(1)) || is_minus(position.checked_sub(1)) => {
                    Some(Symbol::FloatMinus)
                }
                '-' => Some(Symbol::FixedMinus),
                'Z' | 'z' => Some(Symbol::Z),
                '9' => Some(Symbol::Nine),
                ',' => Some(Symbol::Comma),
                '.' => Some(Symbol::Point),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn symbol_tables_match_the_pic_strings() {
        for (picture, text, scale) in PICTURES {
            assert_eq!(
                symbols_of(text).as_deref(),
                Some(picture.symbols()),
                "{picture:?}"
            );
            assert_eq!(picture.width(), text.len(), "{picture:?}");
            assert_eq!(picture.scale(), scale, "{picture:?}");
        }
    }

    #[test]
    fn integer_positions_count_floating_digits_but_not_the_sign_position() {
        let expected = [10, 10, 8, 5, 8, 9, 9, 9, 9, 9, 4, 6, 9];
        for ((picture, _, _), int_positions) in PICTURES.into_iter().zip(expected) {
            assert_eq!(picture.int_positions(), int_positions, "{picture:?}");
        }
    }

    // audit: ARITH-019
    // audit: ARITH-039
    // audit: FIXSTMT-902
    // audit: FIXRPT-001
    #[test]
    fn formats_audited_and_rule_examples() -> TestResult {
        for &(picture, value, expected) in CASES {
            let edited = picture.format(Decimal::from_str_exact(value)?);
            assert_eq!(edited, expected, "{picture:?} {value}");
        }
        Ok(())
    }

    #[test]
    fn every_picture_has_cases() {
        for (picture, _, _) in PICTURES {
            let count = CASES.iter().filter(|case| case.0 == picture).count();
            assert!(count >= 6, "{picture:?} has {count} cases");
        }
    }

    #[test]
    fn any_value_edits_to_exactly_the_picture_width_in_ascii() -> TestResult {
        let values = [
            Decimal::MAX,
            Decimal::MIN,
            Decimal::ZERO,
            Decimal::ONE,
            Decimal::NEGATIVE_ONE,
            Decimal::new(1, 28),
            Decimal::new(-1, 28),
            Decimal::new(-5, 1),
            Decimal::from_str_exact("-0.00")?,
            Decimal::from_str_exact("79228162514264337593543950.335")?,
        ];
        for (picture, _, _) in PICTURES {
            for value in values {
                let edited = picture.format(value);
                assert_eq!(edited.len(), picture.width(), "{picture:?} {value}");
                assert!(edited.is_ascii(), "{picture:?} {value}");
            }
        }
        Ok(())
    }
}
