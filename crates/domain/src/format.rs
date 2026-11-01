//! Edited pictures and the fixed-width output lines of the carried batch and MQ
//! programs.
//!
//! The ported rules decide what a line holds; this module owns its bytes: field
//! widths, `VALUE` literals and numeric editing. Every builder takes primitives
//! or numeric newtypes and none imports a rule, so the rules, the batch jobs and
//! the MQ flows share one layout. The outputs are compared byte for byte with
//! sections B to F of `docs/parity/parity-oracle-audit.md`, whose row IDs the
//! item docs cite.
//!
//! | Output | Record width | Builders | Audit rows |
//! |---|---|---|---|
//! | CBTRN03C `TRANREPT` report, CVTRA07Y layouts | 133 | `report_*` | B: P-CBTRN03C-006 to P-CBTRN03C-009; C: P-CBTRN03C-011, P-CBTRN03C-012, P-CBTRN03C-014; D.6 lines |
//! | CBSTM03A `STMTFILE` statement | 80 | `st_line*` | B: P-CBSTM03A-006 to P-CBSTM03A-008; C: P-CBSTM03A-013; D.5 `STMTFILE` lines |
//! | CBSTM03A `HTMLFILE` statement | 100 | [`HtmlFixedLine`], `html_*` | B: P-CBSTM03A-009, P-CBSTM03A-010; D.5 `HTMLFILE` lines |
//! | COPAUA0C CP00 reply | 64 | [`cp00_reply`] | B: P-COPAUA0C-011; C: P-COPAUA0C-020; D.10 replies |
//! | COACCT01 CDRA reply | 1000 | [`acct_response`], [`pad_reply_1000`] | B: P-COACCT01-013; D.9 replies |
//! | CODATE01 CDRD reply | 1000 | [`date_reply`], [`pad_reply_1000`] | B: P-CODATE01-013; F: P-CODATE01-020, P-CODATE01-021 |
//!
//! # Numeric editing
//!
//! [`EditedPicture`] reproduces the IBM Enterprise COBOL `MOVE` of a numeric
//! value into a numeric-edited item, for the symbols the carried programs use:
//! `9`, `Z` (also written `z`), the insertion comma, the actual decimal point, a
//! fixed leading or trailing `+` or `-`, a leading floating `+` or `-` string and
//! the repetition `n(k)`.
//!
//! - Fraction digits beyond the picture are truncated, never rounded: no program
//!   or copybook in the repository uses `ROUNDED` (P-CBACT04C-011). Integer
//!   digits beyond the picture are dropped from the left, as a `MOVE` without
//!   `ON SIZE ERROR` drops them.
//! - `Z` shows a leading zero as a space, and a comma as a space while no digit
//!   has been shown. Suppression ends at the first non-zero digit or at the
//!   decimal point, which is always shown.
//! - A picture with no `9` position shows a zero value as spaces only, sign and
//!   decimal point included (`+ZZZ,ZZZ,ZZZ.ZZ` of 0.00, P-CBTRN03C-012).
//! - A fixed `+` shows `+` or `-`; a fixed `-` shows `-` or a space. A floating
//!   string shows its sign immediately left of the first digit shown.
//! - A value whose kept digits are all zero shows the sign of zero, which is
//!   positive, as every store of the numeric algebra does (P-CBACT04C-018,
//!   escalation E-42).
//!
//! # Character set
//!
//! Every literal here is ASCII and so are the parity inputs, so a builder's
//! result is exactly its record width in bytes. Alphanumeric moves cut on the
//! right at the receiver width; [`move_alnum`] states what happens to non-ASCII
//! text.

use std::fmt;
use std::iter::repeat_n;

use rust_decimal::Decimal;

use crate::numeric::{Money, MoneyExpr};

/// The most positions an edited picture may have after repetitions are
/// expanded. The widest carried picture, `-ZZZ,ZZZ,ZZZ.ZZ`, has 15.
const MAX_POSITIONS: usize = 32;

/// The most digit positions a picture may have: the 28 significant digits a
/// `Decimal` holds, which also keeps every power of ten below `u128::MAX`.
const MAX_DIGITS: u32 = 28;

/// The byte width of a CBTRN03C `TRANREPT` record, `FD-REPTFILE-REC PIC X(133)`
/// [app/cbl/CBTRN03C.cbl:L85] (P-CBTRN03C-001, P-CBTRN03C-015).
pub const REPORT_RECORD_LEN: usize = 133;

/// The byte width of a CBSTM03A `STMTFILE` record, `FD-STMTFILE-REC PIC X(80)`
/// [app/cbl/CBSTM03A.CBL:L44-45] (P-CBSTM03A-001, P-CBSTM03A-015).
pub const STMT_RECORD_LEN: usize = 80;

/// The byte width of a CBSTM03A `HTMLFILE` record, `FD-HTMLFILE-REC PIC X(100)`
/// [app/cbl/CBSTM03A.CBL:L46-47] (P-CBSTM03A-001, P-CBSTM03A-015).
pub const HTML_RECORD_LEN: usize = 100;

/// The byte length of every CDRA and CDRD reply: `REPLY-MESSAGE PIC X(1000)` put
/// with `MQ-BUFFER-LENGTH` 1000 (P-CODATE01-014, P-COACCT01-017).
pub const REPLY_BUFFER_LEN: usize = 1000;

/// The bytes the COPAUA0C reply `STRING` fills: six fields, each followed by a
/// comma (P-COPAUA0C-011).
pub const CP00_REPLY_STRING_LEN: usize = 63;

/// The byte length of the COPAUA0C reply: the `STRING` pointer starts at 1 and
/// ends one past the 63 filled bytes, and `MQPUT1` sends that many bytes
/// (P-COPAUA0C-011, P-COPAUA0C-021).
pub const CP00_REPLY_LEN: usize = 64;

/// A sign symbol of an edited picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignSymbol {
    /// `+`: shows `+` for a value that is not negative, `-` otherwise.
    Plus,
    /// `-`: shows a space for a value that is not negative, `-` otherwise.
    Minus,
}

impl SignSymbol {
    /// The character a sign position shows.
    const fn shown(self, negative: bool) -> char {
        match (self, negative) {
            (_, true) => '-',
            (Self::Plus, false) => '+',
            (Self::Minus, false) => ' ',
        }
    }
}

/// One character position of an expanded picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Position {
    /// `9`: always shows its digit.
    Digit,
    /// `Z` or `z`: shows its digit, or a space while suppression is in force.
    Suppress,
    /// `,`: shows a comma once a digit has been shown, else a space.
    Comma,
    /// `.`: the actual decimal point, always shown.
    Point,
    /// A fixed sign, first or last position.
    FixedSign(SignSymbol),
    /// The leftmost symbol of a floating sign string: a sign position only.
    FloatSign,
    /// A later symbol of a floating sign string: a digit position that shows a
    /// space, or the sign, while no digit has been shown.
    FloatDigit,
}

/// Why a picture string is not an edited picture this module supports.
///
/// Positions are 0-based byte offsets into the picture string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PictureError {
    /// The picture string is empty.
    #[error("the picture string is empty")]
    Empty,
    /// A symbol other than `9`, `Z`, `z`, `,`, `.`, `+` and `-`.
    #[error("unsupported picture symbol at position {position}")]
    UnsupportedSymbol {
        /// Offset of the symbol.
        position: usize,
    },
    /// A repetition `(k)` that is unterminated, empty, zero, not decimal, or
    /// that follows the decimal point or nothing.
    #[error("invalid repetition at position {position}")]
    InvalidRepetition {
        /// Offset of the opening parenthesis.
        position: usize,
    },
    /// More than 32 positions after repetitions are expanded.
    #[error("the picture has more than {MAX_POSITIONS} positions")]
    TooLong,
    /// More than one decimal point.
    #[error("the picture has more than one decimal point")]
    MultiplePoints,
    /// Signs that mix `+` and `-`, or that are neither one leading or trailing
    /// symbol nor one leading floating string.
    #[error("the picture's sign symbols are mixed or misplaced")]
    InvalidSign,
    /// `Z` together with a floating sign string.
    #[error("zero suppression cannot be combined with a floating sign")]
    SuppressionWithFloat,
    /// `Z` right of a `9` in the integer part, or `Z` in the fraction while some
    /// other digit position is not `Z`.
    #[error("a zero-suppression position follows a digit position")]
    SuppressionAfterDigit,
    /// No digit position at all.
    #[error("the picture has no digit position")]
    NoDigits,
    /// More than 28 digit positions.
    #[error("the picture has more than {MAX_DIGITS} digit positions")]
    TooManyDigits,
}

/// A COBOL numeric-edited `PICTURE`, parsed once and applied by [`edit`].
///
/// Build a picture known at compile time with [`EditedPicture::new`], which
/// rejects an invalid string during constant evaluation, and any other with
/// [`EditedPicture::parse`]. The supported symbols and the editing rules are in
/// the module documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditedPicture {
    /// The picture string as written.
    text: &'static str,
    /// The expanded positions; only the first `width` are used.
    positions: [Position; MAX_POSITIONS],
    /// The number of character positions: the byte width of the edited item.
    width: usize,
    /// The number of digit positions, for the digit arithmetic.
    digit_count: u32,
    /// The same number, as the width of the kept digit text.
    digit_width: usize,
    /// The number of digit positions right of the decimal point.
    fraction_digits: u32,
    /// Whether some position is a `9`; without one a zero value shows spaces.
    has_nine: bool,
    /// The symbol of the floating sign string, when the picture has one.
    float_sign: Option<SignSymbol>,
}

/// The picture string expanded to one normalised symbol byte per position.
struct Expanded {
    symbols: [u8; MAX_POSITIONS],
    width: usize,
}

/// The value of an ASCII decimal digit.
const fn digit_value(byte: u8) -> Option<usize> {
    match byte {
        b'0' => Some(0),
        b'1' => Some(1),
        b'2' => Some(2),
        b'3' => Some(3),
        b'4' => Some(4),
        b'5' => Some(5),
        b'6' => Some(6),
        b'7' => Some(7),
        b'8' => Some(8),
        b'9' => Some(9),
        _ => None,
    }
}

/// Whether a normalised symbol byte is a sign.
const fn is_sign(symbol: u8) -> bool {
    symbol == b'+' || symbol == b'-'
}

/// The repetition count `(k)` whose `(` is at `open`, and the offset after its
/// `)`.
const fn repetition(bytes: &[u8], open: usize) -> Result<(usize, usize), PictureError> {
    let mut index = open + 1;
    let mut count = 0;
    while index < bytes.len() {
        if bytes[index] == b')' {
            if count == 0 {
                return Err(PictureError::InvalidRepetition { position: open });
            }
            return Ok((count, index + 1));
        }
        let Some(value) = digit_value(bytes[index]) else {
            return Err(PictureError::InvalidRepetition { position: open });
        };
        count = count * 10 + value;
        // Checked after every digit, so `count * 10` above never overflows.
        if count > MAX_POSITIONS {
            return Err(PictureError::TooLong);
        }
        index += 1;
    }
    Err(PictureError::InvalidRepetition { position: open })
}

/// The picture string with every repetition expanded and `z` read as `Z`.
const fn expand(text: &str) -> Result<Expanded, PictureError> {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return Err(PictureError::Empty);
    }
    let mut symbols = [0; MAX_POSITIONS];
    let mut width = 0;
    let mut index = 0;
    while index < bytes.len() {
        let symbol = match bytes[index] {
            b'Z' | b'z' => b'Z',
            byte @ (b'9' | b',' | b'.' | b'+' | b'-') => byte,
            b'(' => return Err(PictureError::InvalidRepetition { position: index }),
            _ => return Err(PictureError::UnsupportedSymbol { position: index }),
        };
        let mut count = 1;
        let mut next = index + 1;
        if next < bytes.len() && bytes[next] == b'(' {
            if symbol == b'.' {
                return Err(PictureError::InvalidRepetition { position: next });
            }
            match repetition(bytes, next) {
                Ok((repeat, after)) => {
                    count = repeat;
                    next = after;
                }
                Err(error) => return Err(error),
            }
        }
        while count > 0 {
            if width == MAX_POSITIONS {
                return Err(PictureError::TooLong);
            }
            symbols[width] = symbol;
            width += 1;
            count -= 1;
        }
        index = next;
    }
    Ok(Expanded { symbols, width })
}

/// Where the sign of a picture stands.
#[derive(Clone, Copy)]
enum SignShape {
    /// No sign symbol.
    Unsigned,
    /// One sign symbol, first or last.
    Fixed(SignSymbol),
    /// A leading run of at least two sign symbols.
    Floating(SignSymbol),
}

/// The sign shape of an expanded picture.
const fn sign_shape(expanded: &Expanded) -> Result<SignShape, PictureError> {
    let symbols = &expanded.symbols;
    let width = expanded.width;
    let mut signs = 0;
    let mut sign = 0;
    let mut index = 0;
    while index < width {
        if is_sign(symbols[index]) {
            if signs > 0 && symbols[index] != sign {
                return Err(PictureError::InvalidSign);
            }
            sign = symbols[index];
            signs += 1;
        }
        index += 1;
    }
    let mut leading = 0;
    while leading < width && is_sign(symbols[leading]) {
        leading += 1;
    }
    let symbol = if sign == b'+' {
        SignSymbol::Plus
    } else {
        SignSymbol::Minus
    };
    if signs == 0 {
        Ok(SignShape::Unsigned)
    } else if leading == signs && signs == 1 {
        Ok(SignShape::Fixed(symbol))
    } else if leading == signs {
        Ok(SignShape::Floating(symbol))
    } else if leading == 0 && signs == 1 && is_sign(symbols[width - 1]) {
        Ok(SignShape::Fixed(symbol))
    } else {
        Err(PictureError::InvalidSign)
    }
}

impl EditedPicture {
    /// The picture `text`, checked during constant evaluation.
    ///
    /// # Panics
    ///
    /// When `text` is not a supported edited picture; [`EditedPicture::parse`]
    /// gives the reason. In a `const` item the panic is a compile error, so the
    /// named pictures of this module can never be invalid at run time.
    #[must_use]
    pub const fn new(text: &'static str) -> Self {
        match Self::parse(text) {
            Ok(picture) => picture,
            Err(_) => panic!("not a supported edited picture"),
        }
    }

    /// Parses the edited picture `text`.
    ///
    /// # Errors
    ///
    /// A [`PictureError`] naming the first rule `text` breaks: an unsupported
    /// symbol, a malformed repetition, more than 32 positions or 28 digit
    /// positions, no digit position, a second decimal point, a misplaced or
    /// mixed sign, or zero suppression after a digit or beside a floating sign.
    pub const fn parse(text: &'static str) -> Result<Self, PictureError> {
        let expanded = match expand(text) {
            Ok(expanded) => expanded,
            Err(error) => return Err(error),
        };
        let shape = match sign_shape(&expanded) {
            Ok(shape) => shape,
            Err(error) => return Err(error),
        };
        let mut picture = Self {
            text,
            positions: [Position::Digit; MAX_POSITIONS],
            width: expanded.width,
            digit_count: 0,
            digit_width: 0,
            fraction_digits: 0,
            has_nine: false,
            float_sign: None,
        };
        if let SignShape::Floating(symbol) = shape {
            picture.float_sign = Some(symbol);
        }
        let mut seen_point = false;
        let mut z_in_fraction = false;
        let mut every_digit_is_z = true;
        let mut index = 0;
        while index < expanded.width {
            let position = match expanded.symbols[index] {
                b'9' => {
                    picture.has_nine = true;
                    Position::Digit
                }
                b'Z' => {
                    if matches!(shape, SignShape::Floating(_)) {
                        return Err(PictureError::SuppressionWithFloat);
                    }
                    if picture.has_nine && !seen_point {
                        return Err(PictureError::SuppressionAfterDigit);
                    }
                    if seen_point {
                        z_in_fraction = true;
                    }
                    Position::Suppress
                }
                b',' => Position::Comma,
                b'.' => {
                    if seen_point {
                        return Err(PictureError::MultiplePoints);
                    }
                    seen_point = true;
                    Position::Point
                }
                _ => match shape {
                    SignShape::Floating(_) if index == 0 => Position::FloatSign,
                    SignShape::Floating(_) => Position::FloatDigit,
                    SignShape::Fixed(symbol) => Position::FixedSign(symbol),
                    // `expand` yields only the symbols matched above and signs,
                    // and a picture holding a sign never has this shape.
                    SignShape::Unsigned => return Err(PictureError::InvalidSign),
                },
            };
            if matches!(
                position,
                Position::Digit | Position::Suppress | Position::FloatDigit
            ) {
                picture.digit_count += 1;
                picture.digit_width += 1;
                if seen_point {
                    picture.fraction_digits += 1;
                }
                if !matches!(position, Position::Suppress) {
                    every_digit_is_z = false;
                }
            }
            picture.positions[index] = position;
            index += 1;
        }
        if z_in_fraction && !every_digit_is_z {
            return Err(PictureError::SuppressionAfterDigit);
        }
        if picture.digit_count == 0 {
            return Err(PictureError::NoDigits);
        }
        if picture.digit_count > MAX_DIGITS {
            return Err(PictureError::TooManyDigits);
        }
        Ok(picture)
    }

    /// The picture string as written, for example `-ZZZ,ZZZ,ZZZ.ZZ`.
    #[must_use]
    pub const fn text(&self) -> &'static str {
        self.text
    }

    /// The byte width of the edited item: its number of character positions.
    #[must_use]
    pub const fn width(&self) -> usize {
        self.width
    }

    /// The digits the picture keeps of `value`, one per digit position: excess
    /// fraction digits truncated, excess integer digits dropped from the left.
    fn kept_digits(&self, value: Decimal) -> String {
        let magnitude = value.mantissa().unsigned_abs();
        let (units, shift) = match value.scale().checked_sub(self.fraction_digits) {
            Some(excess) => (magnitude / pow10(excess), 0),
            None => (magnitude, self.fraction_digits - value.scale()),
        };
        // `shift <= fraction_digits <= digit_count <= 28`, so both powers and
        // the kept value stay below `10^28`.
        let kept = units % pow10(self.digit_count - shift) * pow10(shift);
        format!("{kept:0width$}", width = self.digit_width)
    }

    /// The edited item for `digits` (from [`EditedPicture::kept_digits`]).
    fn render(&self, digits: &str, negative: bool) -> String {
        let mut digits = digits.chars();
        let mut shown = Vec::with_capacity(self.width);
        let mut significant = false;
        let mut first_significant = None;
        for (index, position) in self.positions.iter().take(self.width).enumerate() {
            let character = match *position {
                Position::Digit => {
                    significant = true;
                    // `digits` holds one digit per digit position, so it never
                    // runs out here.
                    digits.next().unwrap_or('0')
                }
                Position::Suppress | Position::FloatDigit => {
                    let digit = digits.next().unwrap_or('0');
                    if significant || digit != '0' {
                        significant = true;
                        digit
                    } else {
                        ' '
                    }
                }
                Position::Comma => {
                    if significant {
                        ','
                    } else {
                        ' '
                    }
                }
                Position::Point => {
                    significant = true;
                    '.'
                }
                Position::FixedSign(symbol) => symbol.shown(negative),
                Position::FloatSign => ' ',
            };
            if significant && first_significant.is_none() {
                first_significant = Some(index);
            }
            shown.push(character);
        }
        // A floating sign stands immediately left of the first digit (or
        // decimal point) shown. Position 0 of a floating picture is its sign
        // position and never significant, so that slot always exists.
        if let Some((slot, symbol)) = first_significant
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| shown.get_mut(index))
            .zip(self.float_sign)
        {
            *slot = symbol.shown(negative);
        }
        shown.into_iter().collect()
    }
}

impl fmt::Display for EditedPicture {
    /// The picture string as written.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.text)
    }
}

/// `10^exponent`. Callers keep `exponent` at most 28, far below the 38 digits
/// of `u128`.
const fn pow10(exponent: u32) -> u128 {
    10_u128.pow(exponent)
}

/// The COBOL `MOVE` of `value` into the numeric-edited item `pic`.
///
/// The result is exactly [`EditedPicture::width`] bytes; the editing rules are
/// in the module documentation. For example `-ZZZ,ZZZ,ZZZ.ZZ` of -1234.5 is
/// `-      1,234.50` and `----9` of -532 is ` -532`.
#[must_use]
pub fn edit(pic: &EditedPicture, value: Decimal) -> String {
    let digits = pic.kept_digits(value);
    let zero = digits.bytes().all(|digit| digit == b'0');
    if zero && !pic.has_nine {
        return " ".repeat(pic.width);
    }
    pic.render(&digits, value.is_sign_negative() && !zero)
}

/// [`edit`] of a stored money field.
#[must_use]
pub fn edit_money<const D: u8>(pic: &EditedPicture, value: Money<D>) -> String {
    edit(pic, value.to_decimal())
}

/// [`edit`] of an exact money result, before any store.
#[must_use]
pub fn edit_expr(pic: &EditedPicture, value: &MoneyExpr) -> String {
    edit(pic, value.to_decimal())
}

// Named pictures. Each stands on a reviewed audit row that records its output.
// COACTUPC and COTRTUPC `WS-EDIT-CURRENCY-9-2-F PIC +ZZZ,ZZZ,ZZZ.99` and COPAUS0C
// `WS-DISPLAY-AMT9 PIC -zzzz9.99` have no such row, so they have no constant
// here; a rule that gains one parses its picture with `EditedPicture::parse`.

/// CVTRA07Y `TRAN-REPORT-AMT PIC -ZZZ,ZZZ,ZZZ.ZZ` [app/cpy/CVTRA07Y.cpy:L30], the
/// detail amount of the transaction report: 183.88 is `         183.88` and
/// -47.88 is `-         47.88` — P-CBTRN03C-007, P-CBTRN03C-014.
pub const PIC_REPORT_AMT: EditedPicture = EditedPicture::new("-ZZZ,ZZZ,ZZZ.ZZ");

/// CVTRA07Y `REPT-PAGE-TOTAL`, `REPT-ACCOUNT-TOTAL` and `REPT-GRAND-TOTAL PIC
/// +ZZZ,ZZZ,ZZZ.ZZ` [app/cpy/CVTRA07Y.cpy:L54] [app/cpy/CVTRA07Y.cpy:L60]
/// [app/cpy/CVTRA07Y.cpy:L66]: 1725.65 is `+      1,725.65` and 0.00 is 15
/// spaces — P-CBTRN03C-011, P-CBTRN03C-012.
pub const PIC_REPORT_TOTAL: EditedPicture = EditedPicture::new("+ZZZ,ZZZ,ZZZ.ZZ");

/// COPAUA0C `WS-APPROVED-AMT-DIS PIC -zzzzzzzzz9.99`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L66], the approved amount
/// of the CP00 reply: 100.29 is `        100.29` and 0 is `          0.00` —
/// P-COPAUA0C-011, P-COPAUA0C-020.
pub const PIC_CP00_APPROVED_AMT: EditedPicture = EditedPicture::new("-zzzzzzzzz9.99");

/// CBSTM03A `ST-CURR-BAL PIC 9(9).99-` [app/cbl/CBSTM03A.CBL:L113], the current
/// balance of a statement: 1945.87 is `000001945.87 ` — P-CBSTM03A-006,
/// P-CBSTM03A-100.
pub const PIC_STMT_CURR_BAL: EditedPicture = EditedPicture::new("9(9).99-");

/// CBSTM03A `ST-TRANAMT` and `ST-TOTAL-TRAMT PIC Z(9).99-`
/// [app/cbl/CBSTM03A.CBL:L137] [app/cbl/CBSTM03A.CBL:L142]: 183.88 is
/// `      183.88 `, -47.88 is `       47.88-` and 0 is `         .00 ` —
/// P-CBSTM03A-007, P-CBSTM03A-008, P-CBSTM03A-013.
pub const PIC_STMT_TRAN_AMT: EditedPicture = EditedPicture::new("Z(9).99-");

/// COTRTUPC `WS-DISP-SQLCODE PIC ----9`
/// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L68] and COBTUPDT
/// `WS-VAR-SQLCODE PIC ----9` [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L65]
/// — P-COTRTUPC-005, P-COBTUPDT-007. [`edit_sqlcode`] applies it.
pub const PIC_SQLCODE: EditedPicture = EditedPicture::new("----9");

/// `WS-TRAN-AMT PIC +99999999.99` of the transaction screens
/// [app/cbl/COTRN00C.cbl:L56] [app/cbl/COTRN01C.cbl:L49]
/// [app/cbl/COTRN02C.cbl:L53] [app/cbl/COTRN02C.cbl:L59] [app/cbl/CORPT00C.cbl:L77]
/// [app/cbl/COBIL00C.cbl:L55]: -5.00 is `-00000005.00` — P-COTRN00C-008,
/// P-COTRN01C-005.
pub const PIC_TRAN_AMT_DISPLAY: EditedPicture = EditedPicture::new("+99999999.99");

/// COBIL00C `WS-CURR-BAL PIC +9999999999.99` [app/cbl/COBIL00C.cbl:L56], the
/// balance the bill payment screen shows: 1940.00 is `+0000001940.00` and -5.00
/// is `-0000000005.00` — P-COBIL00C-023.
pub const PIC_BILL_CURR_BAL: EditedPicture = EditedPicture::new("+9999999999.99");

/// COPAUS0C and COPAUS1C `WS-AUTH-AMT PIC -zzzzzzz9.99`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L55]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUS1C.cbl:L52], the approved amount
/// of a listed authorization — P-COPAUS0C-013.
pub const PIC_AUTH_AMT: EditedPicture = EditedPicture::new("-zzzzzzz9.99");

/// CCPAURQY `PA-RQ-TRANSACTION-AMT` and CCPAURLY `PA-RL-APPROVED-AMT PIC
/// +9(10).99` [app/app-authorization-ims-db2-mq/cpy/CCPAURQY.cpy:L27]
/// [app/app-authorization-ims-db2-mq/cpy/CCPAURLY.cpy:L24], the request amount
/// receiver of COPAUA0C — P-CCPAURQY-009, P-CCPAURLY-006, P-COPAUA0C-016.
pub const PIC_CCPAU_AMT: EditedPicture = EditedPicture::new("+9(10).99");

/// COTRTUPC and COBTUPDT `MOVE SQLCODE TO WS-DISP-SQLCODE`: the `----9` text of
/// a Db2 SQLCODE, for example ` -532` for -532 and `  100` for 100 —
/// P-COTRTUPC-005, P-COBTUPDT-007.
#[must_use]
pub fn edit_sqlcode(code: i32) -> String {
    edit(&PIC_SQLCODE, Decimal::from(code))
}

// Alphanumeric moves and zoned text.

/// The trailing overpunch of a positive or zero zoned field: last digit 0 to 9
/// becomes `{` and `A` to `I` (P-COACCT01-013).
const POSITIVE_OVERPUNCH: &[u8; 10] = b"{ABCDEFGHI";

/// The trailing overpunch of a negative zoned field: last digit 0 to 9 becomes
/// `}` and `J` to `R` (P-COACCT01-013).
const NEGATIVE_OVERPUNCH: &[u8; 10] = b"}JKLMNOPQR";

/// The COBOL alphanumeric `MOVE` of `text` into an `X(width)` receiver:
/// left-justified, padded with spaces, cut on the right.
///
/// The cut is in bytes, as in the COBOL, and the result is always `width` bytes.
/// Every parity input is ASCII, where bytes and characters agree. For other text
/// the cut moves left to the nearest character boundary and the freed bytes are
/// spaces, so the result stays valid UTF-8.
#[must_use]
pub fn move_alnum(text: &str, width: usize) -> String {
    let mut end = text.len().min(width);
    // Offset 0 is always a boundary, so this stops.
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut moved = String::with_capacity(width);
    moved.push_str(text.get(..end).unwrap_or_default());
    moved.extend(repeat_n(' ', width - end));
    moved
}

/// The `DISPLAY` digits of an unsigned `9(digits)` field holding `value`: the
/// low-order `digits` digits, zero-padded, as a numeric `MOVE` stores them.
///
/// The same text is what an alphanumeric receiver gets from that field, as in
/// CBSTM03A `MOVE ACCT-ID TO ST-ACCT-ID` (P-CBSTM03A-099). `digits` 0 gives an
/// empty text.
#[must_use]
pub fn unsigned_display(value: u64, digits: usize) -> String {
    if digits == 0 {
        return String::new();
    }
    let kept = u32::try_from(digits)
        .ok()
        .and_then(|exponent| 10_u64.checked_pow(exponent))
        .map_or(value, |limit| value % limit);
    format!("{kept:0digits$}")
}

/// A `STRING` source `DELIMITED BY delimiter`: `source` up to, not including,
/// the first occurrence of `delimiter`, or all of `source` when `delimiter` does
/// not occur or is empty.
///
/// CBSTM03A builds its name, address and HTML cells this way
/// (P-CBSTM03A-006, P-CBSTM03A-010, P-CBSTM03A-091).
#[must_use]
pub fn delimited_by<'a>(source: &'a str, delimiter: &str) -> &'a str {
    if delimiter.is_empty() {
        return source;
    }
    source
        .split_once(delimiter)
        .map_or(source, |(head, _)| head)
}

/// The `DISPLAY` bytes of a signed `S9(D)V99` field moved unedited into a group,
/// as COACCT01 moves `ACCT-CURR-BAL` into `WS-ACCT-CURR-BAL PIC S9(10)V99`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L130-169]: `D + 2` digits with the sign as
/// a trailing overpunch, `{ABCDEFGHI` for +0 to +9 and `}JKLMNOPQR` for -0 to -9,
/// the table the codec layouts also use (section A). 194.00 as `S9(10)V99` is
/// `00000001940{` — P-COACCT01-013, P-COACCT01-020.
///
/// A zero is positive, so it carries `{` (escalation E-42).
#[must_use]
pub fn unedited_zoned<const D: u8>(value: Money<D>) -> String {
    let width = usize::from(D) + 2;
    let mut bytes = format!("{:0width$}", value.cents().unsigned_abs()).into_bytes();
    let table = if value.is_negative() {
        NEGATIVE_OVERPUNCH
    } else {
        POSITIVE_OVERPUNCH
    };
    let punched = bytes
        .last()
        .and_then(|&last| digit_value(last))
        .and_then(|digit| table.get(digit))
        .copied();
    if let (Some(last), Some(punched)) = (bytes.last_mut(), punched) {
        *last = punched;
    }
    bytes.into_iter().map(char::from).collect()
}

/// The elementary items `(content, width)` of a group, each moved into its
/// `X(width)` and concatenated in order.
fn group(items: &[(&str, usize)]) -> String {
    items
        .iter()
        .map(|&(text, width)| move_alnum(text, width))
        .collect()
}

/// What a `Debug` rendering shows in place of the money amounts and the
/// transaction source, so diagnostics never carry an account's financial data
/// or where its transactions came from.
const REDACTED: &str = "<redacted>";

// CVTRA07Y report lines: CBTRN03C, written by `batch tranrept` and returned by
// the CR00 report endpoint. Every group is moved into the 133-byte record, so
// each line is padded with spaces to `REPORT_RECORD_LEN`.

/// One detail line of the transaction report, the fields CBTRN03C
/// `1120-WRITE-DETAIL` moves into `TRANSACTION-DETAIL-REPORT`
/// [app/cbl/CBTRN03C.cbl:L361-374] — P-CBTRN03C-007.
///
/// Its `Debug` output withholds the amount, which is financial data, and the
/// source of the transaction, while [`report_detail_line`] still writes both
/// into the line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReportDetail<'a> {
    /// `TRAN-ID`, into `TRAN-REPORT-TRANS-ID X(16)`.
    pub tran_id: &'a str,
    /// `XREF-ACCT-ID 9(11)`, into `TRAN-REPORT-ACCOUNT-ID X(11)`.
    pub account_id: u64,
    /// `TRAN-TYPE-CD`, into `TRAN-REPORT-TYPE-CD X(02)`.
    pub type_cd: &'a str,
    /// `TRAN-TYPE-DESC` of the processing copy, into `TRAN-REPORT-TYPE-DESC X(15)`.
    pub type_desc: &'a str,
    /// `TRAN-CAT-CD 9(04)`, into `TRAN-REPORT-CAT-CD 9(04)`.
    pub cat_cd: u16,
    /// `TRAN-CAT-TYPE-DESC` of the processing copy, into
    /// `TRAN-REPORT-CAT-DESC X(29)`.
    pub cat_desc: &'a str,
    /// `TRAN-SOURCE`, into `TRAN-REPORT-SOURCE X(10)`.
    pub source: &'a str,
    /// `TRAN-AMT S9(09)V99`, edited into `TRAN-REPORT-AMT -ZZZ,ZZZ,ZZZ.ZZ`.
    pub amount: Money<9>,
}

/// Every field in declaration order, the source and the amount shown as
/// `<redacted>`. The exhaustive pattern makes a new field a compile error until
/// it is listed.
impl fmt::Debug for ReportDetail<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            tran_id,
            account_id,
            type_cd,
            type_desc,
            cat_cd,
            cat_desc,
            source: _,
            amount: _,
        } = self;
        f.debug_struct("ReportDetail")
            .field("tran_id", tran_id)
            .field("account_id", account_id)
            .field("type_cd", type_cd)
            .field("type_desc", type_desc)
            .field("cat_cd", cat_cd)
            .field("cat_desc", cat_desc)
            .field("source", &format_args!("{REDACTED}"))
            .field("amount", &format_args!("{REDACTED}"))
            .finish()
    }
}

/// CVTRA07Y `REPORT-NAME-HEADER` [app/cpy/CVTRA07Y.cpy:L4-13] with the DATEPARM
/// dates in `REPT-START-DATE` and `REPT-END-DATE`: `DALYREPT` X(38),
/// `Daily Transaction Report` X(41), `Date Range: ` X(12), start X(10), ` to `,
/// end X(10) — P-CBTRN03C-006, P-CBTRN03C-016.
#[must_use]
pub fn report_name_header(start_date: &str, end_date: &str) -> String {
    let header = group(&[
        ("DALYREPT", 38),
        ("Daily Transaction Report", 41),
        ("Date Range: ", 12),
        (start_date, 10),
        (" to ", 4),
        (end_date, 10),
    ]);
    move_alnum(&header, REPORT_RECORD_LEN)
}

/// CBTRN03C `WS-BLANK-LINE PIC X(133) VALUE SPACES` [app/cbl/CBTRN03C.cbl:L133],
/// the second line of every header block — P-CBTRN03C-017.
#[must_use]
pub fn report_blank_line() -> String {
    " ".repeat(REPORT_RECORD_LEN)
}

/// CVTRA07Y `TRANSACTION-HEADER-1` [app/cpy/CVTRA07Y.cpy:L33-46]: the column
/// titles — P-CBTRN03C-018.
#[must_use]
pub fn report_header_1() -> String {
    let header = group(&[
        ("Transaction ID", 17),
        ("Account ID", 12),
        ("Transaction Type", 19),
        ("Tran Category", 35),
        ("Tran Source", 14),
        ("", 1),
        ("        Amount", 16),
    ]);
    move_alnum(&header, REPORT_RECORD_LEN)
}

/// CVTRA07Y `TRANSACTION-HEADER-2 PIC X(133) VALUE ALL '-'`
/// [app/cpy/CVTRA07Y.cpy:L48] — P-CBTRN03C-019.
#[must_use]
pub fn report_header_2() -> String {
    "-".repeat(REPORT_RECORD_LEN)
}

/// CVTRA07Y `TRANSACTION-DETAIL-REPORT` [app/cpy/CVTRA07Y.cpy:L15-31]: TRAN-ID
/// X(16), space, account X(11), space, type X(02), `-`, type description X(15),
/// space, category 9(04), `-`, category description X(29), space, source X(10),
/// 4 spaces, the amount edited `-ZZZ,ZZZ,ZZZ.ZZ` and 2 spaces — P-CBTRN03C-007,
/// P-CBTRN03C-014, P-CBTRN03C-020.
#[must_use]
pub fn report_detail_line(detail: &ReportDetail<'_>) -> String {
    let account = unsigned_display(detail.account_id, 11);
    let category = unsigned_display(u64::from(detail.cat_cd), 4);
    let amount = edit_money(&PIC_REPORT_AMT, detail.amount);
    let line = group(&[
        (detail.tran_id, 16),
        ("", 1),
        (&account, 11),
        ("", 1),
        (detail.type_cd, 2),
        ("-", 1),
        (detail.type_desc, 15),
        ("", 1),
        (&category, 4),
        ("-", 1),
        (detail.cat_desc, 29),
        ("", 1),
        (detail.source, 10),
        ("", 4),
        (&amount, 15),
        ("", 2),
    ]);
    move_alnum(&line, REPORT_RECORD_LEN)
}

/// A totals group: the label in `X(label_width)`, `dots` periods (`VALUE ALL
/// '.'`) and the total edited `+ZZZ,ZZZ,ZZZ.ZZ`.
fn report_total(label: &str, label_width: usize, dots: usize, total: Money<9>) -> String {
    let dots = ".".repeat(dots);
    let total = edit_money(&PIC_REPORT_TOTAL, total);
    let line = group(&[(label, label_width), (&dots, dots.len()), (&total, 15)]);
    move_alnum(&line, REPORT_RECORD_LEN)
}

/// CVTRA07Y `REPORT-PAGE-TOTALS` [app/cpy/CVTRA07Y.cpy:L50-54]: `Page Total`
/// X(11), 86 periods and `WS-PAGE-TOTAL` edited `+ZZZ,ZZZ,ZZZ.ZZ` —
/// P-CBTRN03C-008, P-CBTRN03C-011, P-CBTRN03C-036.
#[must_use]
pub fn report_page_total(total: Money<9>) -> String {
    report_total("Page Total", 11, 86, total)
}

/// CVTRA07Y `REPORT-ACCOUNT-TOTALS` [app/cpy/CVTRA07Y.cpy:L56-60]: `Account
/// Total` X(13), 84 periods and `WS-ACCOUNT-TOTAL` edited `+ZZZ,ZZZ,ZZZ.ZZ` —
/// P-CBTRN03C-008, P-CBTRN03C-012, P-CBTRN03C-026.
#[must_use]
pub fn report_account_total(total: Money<9>) -> String {
    report_total("Account Total", 13, 84, total)
}

/// CVTRA07Y `REPORT-GRAND-TOTALS` [app/cpy/CVTRA07Y.cpy:L62-66]: `Grand Total`
/// X(11), 86 periods and `WS-GRAND-TOTAL` edited `+ZZZ,ZZZ,ZZZ.ZZ` —
/// P-CBTRN03C-009, P-CBTRN03C-012, P-CBTRN03C-466.
#[must_use]
pub fn report_grand_total(total: Money<9>) -> String {
    report_total("Grand Total", 11, 86, total)
}

// CBSTM03A STATEMENT-LINES [app/cbl/CBSTM03A.CBL:L85-146], the 80-byte
// STMTFILE records of `batch creastmt`. `INITIALIZE STATEMENT-LINES` and the
// MOVEs of 5000-CREATE-STATEMENT and 6000-WRITE-TRANS set every named field
// before its line is written, so each builder takes all of its line's fields.

/// 80 hyphens: `ST-LINE5`, `ST-LINE10` and `ST-LINE12`, `VALUE ALL '-'`.
fn stmt_rule() -> String {
    "-".repeat(STMT_RECORD_LEN)
}

/// `ST-LINE0` [app/cbl/CBSTM03A.CBL:L86-89]: 31 asterisks, `START OF
/// STATEMENT`, 31 asterisks — P-CBSTM03A-016.
#[must_use]
pub fn st_line0() -> String {
    [
        "*".repeat(31),
        "START OF STATEMENT".to_owned(),
        "*".repeat(31),
    ]
    .concat()
}

/// `ST-LINE1` [app/cbl/CBSTM03A.CBL:L90-92]: `ST-NAME` X(75), the `STRING` of
/// the customer names, and 5 spaces — P-CBSTM03A-006, P-CBSTM03A-095.
#[must_use]
pub fn st_line1(name: &str) -> String {
    group(&[(name, 75), ("", 5)])
}

/// `ST-LINE2` [app/cbl/CBSTM03A.CBL:L93-95]: `ST-ADD1` X(50) and 30 spaces —
/// P-CBSTM03A-006, P-CBSTM03A-096.
#[must_use]
pub fn st_line2(addr1: &str) -> String {
    group(&[(addr1, 50), ("", 30)])
}

/// `ST-LINE3` [app/cbl/CBSTM03A.CBL:L96-98]: `ST-ADD2` X(50) and 30 spaces —
/// P-CBSTM03A-006, P-CBSTM03A-097.
#[must_use]
pub fn st_line3(addr2: &str) -> String {
    group(&[(addr2, 50), ("", 30)])
}

/// `ST-LINE4` [app/cbl/CBSTM03A.CBL:L99-100]: `ST-ADD3` X(80), the `STRING` of
/// address line 3, state, country and ZIP — P-CBSTM03A-006, P-CBSTM03A-098.
#[must_use]
pub fn st_line4(addr3: &str) -> String {
    move_alnum(addr3, STMT_RECORD_LEN)
}

/// `ST-LINE5` [app/cbl/CBSTM03A.CBL:L101-102]: 80 hyphens — P-CBSTM03A-017,
/// P-CBSTM03A-019.
#[must_use]
pub fn st_line5() -> String {
    stmt_rule()
}

/// `ST-LINE6` [app/cbl/CBSTM03A.CBL:L103-106]: 33 spaces, `Basic Details` X(14)
/// and 33 spaces — P-CBSTM03A-018.
#[must_use]
pub fn st_line6() -> String {
    group(&[("", 33), ("Basic Details", 14), ("", 33)])
}

/// `ST-LINE7` [app/cbl/CBSTM03A.CBL:L107-110]: `Account ID         :` X(20), the
/// 11 digits of `ACCT-ID 9(11)` left-justified in `ST-ACCT-ID X(20)`, and 40
/// spaces — P-CBSTM03A-006, P-CBSTM03A-099.
#[must_use]
pub fn st_line7(account_id: u64) -> String {
    group(&[
        ("Account ID         :", 20),
        (&unsigned_display(account_id, 11), 20),
        ("", 40),
    ])
}

/// `ST-LINE8` [app/cbl/CBSTM03A.CBL:L111-115]: `Current Balance    :` X(20),
/// `ACCT-CURR-BAL` edited `9(9).99-` and 47 spaces — P-CBSTM03A-006,
/// P-CBSTM03A-100.
#[must_use]
pub fn st_line8(curr_bal: Money<10>) -> String {
    group(&[
        ("Current Balance    :", 20),
        (&edit_money(&PIC_STMT_CURR_BAL, curr_bal), 13),
        ("", 7),
        ("", 40),
    ])
}

/// `ST-LINE9` [app/cbl/CBSTM03A.CBL:L116-119]: `FICO Score         :` X(20),
/// the 3 digits of `CUST-FICO-CREDIT-SCORE 9(03)` left-justified in
/// `ST-FICO-SCORE X(20)`, and 40 spaces — P-CBSTM03A-006, P-CBSTM03A-101.
#[must_use]
pub fn st_line9(fico_score: u16) -> String {
    group(&[
        ("FICO Score         :", 20),
        (&unsigned_display(u64::from(fico_score), 3), 20),
        ("", 40),
    ])
}

/// `ST-LINE10` [app/cbl/CBSTM03A.CBL:L120-121]: 80 hyphens — P-CBSTM03A-020.
#[must_use]
pub fn st_line10() -> String {
    stmt_rule()
}

/// `ST-LINE11` [app/cbl/CBSTM03A.CBL:L122-125]: 30 spaces, `TRANSACTION SUMMARY `
/// X(20) and 30 spaces — P-CBSTM03A-021.
#[must_use]
pub fn st_line11() -> String {
    group(&[("", 30), ("TRANSACTION SUMMARY ", 20), ("", 30)])
}

/// `ST-LINE12` [app/cbl/CBSTM03A.CBL:L126-127]: 80 hyphens — P-CBSTM03A-022,
/// P-CBSTM03A-024, P-CBSTM03A-025.
#[must_use]
pub fn st_line12() -> String {
    stmt_rule()
}

/// `ST-LINE13` [app/cbl/CBSTM03A.CBL:L128-131]: `Tran ID         ` X(16),
/// `Tran Details    ` X(51) and `  Tran Amount` X(13) — P-CBSTM03A-023.
#[must_use]
pub fn st_line13() -> String {
    group(&[
        ("Tran ID         ", 16),
        ("Tran Details    ", 51),
        ("  Tran Amount", 13),
    ])
}

/// `ST-LINE14` [app/cbl/CBSTM03A.CBL:L132-137]: `TRNX-ID` in `ST-TRANID` X(16),
/// a space, `TRNX-DESC` cut to `ST-TRANDT` X(49), `$` and `TRNX-AMT` edited
/// `Z(9).99-` — P-CBSTM03A-007, P-CBSTM03A-102.
#[must_use]
pub fn st_line14(tran_id: &str, tran_details: &str, amount: Money<9>) -> String {
    group(&[
        (tran_id, 16),
        (" ", 1),
        (tran_details, 49),
        ("$", 1),
        (&edit_money(&PIC_STMT_TRAN_AMT, amount), 13),
    ])
}

/// `ST-LINE14A` [app/cbl/CBSTM03A.CBL:L138-142]: `Total EXP:` X(10), 56 spaces,
/// `$` and the card total `WS-TRN-AMT S9(9)V99` edited `Z(9).99-` —
/// P-CBSTM03A-008, P-CBSTM03A-109.
#[must_use]
pub fn st_line14a(total: Money<9>) -> String {
    group(&[
        ("Total EXP:", 10),
        ("", 56),
        ("$", 1),
        (&edit_money(&PIC_STMT_TRAN_AMT, total), 13),
    ])
}

/// `ST-LINE15` [app/cbl/CBSTM03A.CBL:L143-146]: 32 asterisks, `END OF
/// STATEMENT`, 32 asterisks — P-CBSTM03A-026.
#[must_use]
pub fn st_line15() -> String {
    [
        "*".repeat(32),
        "END OF STATEMENT".to_owned(),
        "*".repeat(32),
    ]
    .concat()
}

// CBSTM03A HTML-LINES [app/cbl/CBSTM03A.CBL:L148-223], the 100-byte HTMLFILE
// records of `batch creastmt`.

/// A fixed HTML line: an 88-level value of `HTML-FIXED-LN PIC X(100)`
/// [app/cbl/CBSTM03A.CBL:L149-211], written after `SET … TO TRUE`.
///
/// Each value is the `VALUE` literal; a literal continued on the next source
/// line runs to column 72, so no space is inserted at the join (P-CBSTM03A-009).
/// `HTML-LTDS` (`<td>`) is declared but never set by CBSTM03A, so it has no
/// variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HtmlFixedLine {
    /// `HTML-L01` [app/cbl/CBSTM03A.CBL:L150] — P-CBSTM03A-027.
    L01,
    /// `HTML-L02` [app/cbl/CBSTM03A.CBL:L151] — P-CBSTM03A-028.
    L02,
    /// `HTML-L03` [app/cbl/CBSTM03A.CBL:L152] — P-CBSTM03A-029.
    L03,
    /// `HTML-L04` [app/cbl/CBSTM03A.CBL:L153] — P-CBSTM03A-030.
    L04,
    /// `HTML-L05` [app/cbl/CBSTM03A.CBL:L154] — P-CBSTM03A-031.
    L05,
    /// `HTML-L06` [app/cbl/CBSTM03A.CBL:L155] — P-CBSTM03A-032.
    L06,
    /// `HTML-L07` [app/cbl/CBSTM03A.CBL:L156] — P-CBSTM03A-033.
    L07,
    /// `HTML-L08` [app/cbl/CBSTM03A.CBL:L157-158] — P-CBSTM03A-034.
    L08,
    /// `HTML-LTRS` [app/cbl/CBSTM03A.CBL:L159] — P-CBSTM03A-035.
    Ltrs,
    /// `HTML-LTRE` [app/cbl/CBSTM03A.CBL:L160] — P-CBSTM03A-038.
    Ltre,
    /// `HTML-LTDE` [app/cbl/CBSTM03A.CBL:L162] — P-CBSTM03A-037.
    Ltde,
    /// `HTML-L10` [app/cbl/CBSTM03A.CBL:L163-164] — P-CBSTM03A-036.
    L10,
    /// `HTML-L15` [app/cbl/CBSTM03A.CBL:L165-166] — P-CBSTM03A-040.
    L15,
    /// `HTML-L16` [app/cbl/CBSTM03A.CBL:L167-168] — P-CBSTM03A-041.
    L16,
    /// `HTML-L17` [app/cbl/CBSTM03A.CBL:L169-170] — P-CBSTM03A-042.
    L17,
    /// `HTML-L18` [app/cbl/CBSTM03A.CBL:L171-172] — P-CBSTM03A-043.
    L18,
    /// `HTML-L22-35` [app/cbl/CBSTM03A.CBL:L173-175] — P-CBSTM03A-047.
    L22And35,
    /// `HTML-L30-42` [app/cbl/CBSTM03A.CBL:L176-178] — P-CBSTM03A-051.
    L30And42,
    /// `HTML-L31` [app/cbl/CBSTM03A.CBL:L179-180] — P-CBSTM03A-052.
    L31,
    /// `HTML-L43` [app/cbl/CBSTM03A.CBL:L181-182] — P-CBSTM03A-061.
    L43,
    /// `HTML-L47` [app/cbl/CBSTM03A.CBL:L183-185] — P-CBSTM03A-065.
    L47,
    /// `HTML-L48` [app/cbl/CBSTM03A.CBL:L186-187] — P-CBSTM03A-066.
    L48,
    /// `HTML-L50` [app/cbl/CBSTM03A.CBL:L188-190] — P-CBSTM03A-068.
    L50,
    /// `HTML-L51` [app/cbl/CBSTM03A.CBL:L191-192] — P-CBSTM03A-069.
    L51,
    /// `HTML-L53` [app/cbl/CBSTM03A.CBL:L193-195] — P-CBSTM03A-071.
    L53,
    /// `HTML-L54` [app/cbl/CBSTM03A.CBL:L196-197] — P-CBSTM03A-072.
    L54,
    /// `HTML-L58` [app/cbl/CBSTM03A.CBL:L198-200] — P-CBSTM03A-076.
    L58,
    /// `HTML-L61` [app/cbl/CBSTM03A.CBL:L201-203] — P-CBSTM03A-078.
    L61,
    /// `HTML-L64` [app/cbl/CBSTM03A.CBL:L204-206] — P-CBSTM03A-080.
    L64,
    /// `HTML-L75` [app/cbl/CBSTM03A.CBL:L207-208] — P-CBSTM03A-085.
    L75,
    /// `HTML-L78` [app/cbl/CBSTM03A.CBL:L209] — P-CBSTM03A-088.
    L78,
    /// `HTML-L79` [app/cbl/CBSTM03A.CBL:L210] — P-CBSTM03A-089.
    L79,
    /// `HTML-L80` [app/cbl/CBSTM03A.CBL:L211] — P-CBSTM03A-090.
    L80,
}

impl HtmlFixedLine {
    /// The `VALUE` literal, without the padding of the record.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::L01 => "<!DOCTYPE html>",
            Self::L02 => "<html lang=\"en\">",
            Self::L03 => "<head>",
            Self::L04 => "<meta charset=\"utf-8\">",
            Self::L05 => "<title>HTML Table Layout</title>",
            Self::L06 => "</head>",
            Self::L07 => "<body style=\"margin:0px;\">",
            Self::L08 => {
                "<table  align=\"center\" frame=\"box\" style=\"width:70%; font:12px Segoe UI,sans-serif;\">"
            }
            Self::Ltrs => "<tr>",
            Self::Ltre => "</tr>",
            Self::Ltde => "</td>",
            Self::L10 => "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#1d1d96b3;\">",
            Self::L15 => "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#FFAF33;\">",
            Self::L16 => "<p style=\"font-size:16px\">Bank of XYZ</p>",
            Self::L17 => "<p>410 Terry Ave N</p>",
            Self::L18 => "<p>Seattle WA 99999</p>",
            Self::L22And35 => {
                "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#f2f2f2;\">"
            }
            Self::L30And42 => {
                "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#33FFD1; text-align:center;\">"
            }
            Self::L31 => "<p style=\"font-size:16px\">Basic Details</p>",
            Self::L43 => "<p style=\"font-size:16px\">Transaction Summary</p>",
            Self::L47 => {
                "<td style=\"width:25%; padding:0px 5px; background-color:#33FF5E; text-align:left;\">"
            }
            Self::L48 => "<p style=\"font-size:16px\">Tran ID</p>",
            Self::L50 => {
                "<td style=\"width:55%; padding:0px 5px; background-color:#33FF5E; text-align:left;\">"
            }
            Self::L51 => "<p style=\"font-size:16px\">Tran Details</p>",
            Self::L53 => {
                "<td style=\"width:20%; padding:0px 5px; background-color:#33FF5E; text-align:right;\">"
            }
            Self::L54 => "<p style=\"font-size:16px\">Amount</p>",
            Self::L58 => {
                "<td style=\"width:25%; padding:0px 5px; background-color:#f2f2f2; text-align:left;\">"
            }
            Self::L61 => {
                "<td style=\"width:55%; padding:0px 5px; background-color:#f2f2f2; text-align:left;\">"
            }
            Self::L64 => {
                "<td style=\"width:20%; padding:0px 5px; background-color:#f2f2f2; text-align:right;\">"
            }
            Self::L75 => "<h3>End of Statement</h3>",
            Self::L78 => "</table>",
            Self::L79 => "</body>",
            Self::L80 => "</html>",
        }
    }

    /// The 100-byte `HTMLFILE` record: the literal padded with spaces.
    #[must_use]
    pub fn record(self) -> String {
        move_alnum(self.text(), HTML_RECORD_LEN)
    }
}

/// The opening tag the name and the fixed headings share,
/// `'<p style="font-size:16px">'` [app/cbl/CBSTM03A.CBL:L562].
const HTML_P_16PX: &str = "<p style=\"font-size:16px\">";

/// A `STRING … INTO` an `HTMLFILE` record or line that `MOVE SPACES` cleared:
/// `parts` concatenated and padded to 100 bytes.
fn html_record(parts: &[&str]) -> String {
    move_alnum(&parts.concat(), HTML_RECORD_LEN)
}

/// A paragraph built as CBSTM03A builds its name and address lines: the field
/// `DELIMITED BY '  '` (two spaces), then `'  '` and `'</p>'`
/// (P-CBSTM03A-010).
fn html_spaced_paragraph(open: &str, field: &str) -> String {
    html_record(&[open, delimited_by(field, "  "), "  ", "</p>"])
}

/// A cell built as CBSTM03A builds its basic details and transaction cells: the
/// label, the field `DELIMITED BY '*'` and `'</p>'` (P-CBSTM03A-010,
/// P-CBSTM03A-091).
fn html_star_paragraph(label: &str, field: &str) -> String {
    html_record(&[label, delimited_by(field, "*"), "</p>"])
}

/// `HTML-L11` [app/cbl/CBSTM03A.CBL:L212-216] after `MOVE ACCT-ID TO L11-ACCT`:
/// `<h3>Statement for Account Number: ` X(34), the 11 account digits in X(20)
/// and `</h3>` X(05) — P-CBSTM03A-110.
#[must_use]
pub fn html_l11(account_id: u64) -> String {
    let line = group(&[
        ("<h3>Statement for Account Number: ", 34),
        (&unsigned_display(account_id, 11), 20),
        ("</h3>", 5),
    ]);
    move_alnum(&line, HTML_RECORD_LEN)
}

/// The name line of 5200-WRITE-HTML-NMADBS [app/cbl/CBSTM03A.CBL:L560-568]:
/// `ST-NAME` moved into `L23-NAME X(50)`, then `<p style="font-size:16px">`, the
/// name up to its first two spaces, two spaces and `</p>` — P-CBSTM03A-010,
/// P-CBSTM03A-111.
#[must_use]
pub fn html_l23(name: &str) -> String {
    html_spaced_paragraph(HTML_P_16PX, &move_alnum(name, 50))
}

/// The `HTML-ADDR-LN` of `ST-ADD1 X(50)` [app/cbl/CBSTM03A.CBL:L569-576] —
/// P-CBSTM03A-010, P-CBSTM03A-112.
#[must_use]
pub fn html_add1_ln(addr1: &str) -> String {
    html_spaced_paragraph("<p>", &move_alnum(addr1, 50))
}

/// The `HTML-ADDR-LN` of `ST-ADD2 X(50)` [app/cbl/CBSTM03A.CBL:L577-584] —
/// P-CBSTM03A-010, P-CBSTM03A-113.
#[must_use]
pub fn html_add2_ln(addr2: &str) -> String {
    html_spaced_paragraph("<p>", &move_alnum(addr2, 50))
}

/// The `HTML-ADDR-LN` of `ST-ADD3 X(80)` [app/cbl/CBSTM03A.CBL:L585-592] —
/// P-CBSTM03A-010, P-CBSTM03A-114.
#[must_use]
pub fn html_add3_ln(addr3: &str) -> String {
    html_spaced_paragraph("<p>", &move_alnum(addr3, 80))
}

/// The `HTML-BSIC-LN` of `ST-ACCT-ID X(20)` [app/cbl/CBSTM03A.CBL:L613-619]:
/// `<p>Account ID         : `, the field and `</p>` — P-CBSTM03A-010,
/// P-CBSTM03A-115.
#[must_use]
pub fn html_acct_ln(account_id: u64) -> String {
    let field = move_alnum(&unsigned_display(account_id, 11), 20);
    html_star_paragraph("<p>Account ID         : ", &field)
}

/// The `HTML-BSIC-LN` of `ST-CURR-BAL PIC 9(9).99-`
/// [app/cbl/CBSTM03A.CBL:L620-626] — P-CBSTM03A-010, P-CBSTM03A-116.
#[must_use]
pub fn html_bal_ln(curr_bal: Money<10>) -> String {
    let field = edit_money(&PIC_STMT_CURR_BAL, curr_bal);
    html_star_paragraph("<p>Current Balance    : ", &field)
}

/// The `HTML-BSIC-LN` of `ST-FICO-SCORE X(20)` [app/cbl/CBSTM03A.CBL:L627-633]
/// — P-CBSTM03A-010, P-CBSTM03A-117.
#[must_use]
pub fn html_fico_ln(fico_score: u16) -> String {
    let field = move_alnum(&unsigned_display(u64::from(fico_score), 3), 20);
    html_star_paragraph("<p>FICO Score         : ", &field)
}

/// The `HTML-TRAN-LN` of `ST-TRANID X(16)` [app/cbl/CBSTM03A.CBL:L686-692] —
/// P-CBSTM03A-091.
#[must_use]
pub fn html_tranid_ln(tran_id: &str) -> String {
    html_star_paragraph("<p>", &move_alnum(tran_id, 16))
}

/// The `HTML-TRAN-LN` of `ST-TRANDT X(49)` [app/cbl/CBSTM03A.CBL:L698-704] —
/// P-CBSTM03A-092.
#[must_use]
pub fn html_trandt_ln(tran_details: &str) -> String {
    html_star_paragraph("<p>", &move_alnum(tran_details, 49))
}

/// The `HTML-TRAN-LN` of `ST-TRANAMT PIC Z(9).99-` [app/cbl/CBSTM03A.CBL:L710-716]
/// — P-CBSTM03A-093.
#[must_use]
pub fn html_tranamt_ln(amount: Money<9>) -> String {
    html_star_paragraph("<p>", &edit_money(&PIC_STMT_TRAN_AMT, amount))
}

// MQ reply bodies. The error-queue record MQ-ERR-DISPLAY is built by the
// messaging crate's error queue, not here.

/// The COPAUA0C CP00 reply [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L720-731]:
/// `STRING PA-RL-CARD-NUM ',' PA-RL-TRANSACTION-ID ',' PA-RL-AUTH-ID-CODE ','
/// PA-RL-AUTH-RESP-CODE ',' PA-RL-AUTH-RESP-REASON ',' WS-APPROVED-AMT-DIS ','
/// DELIMITED BY SIZE`, the fields at their CCPAURLY widths X(16), X(15), X(06),
/// X(02), X(04) and the amount edited `-zzzzzzzzz9.99` — P-COPAUA0C-011,
/// P-COPAUA0C-021.
///
/// The result is the whole [`CP00_REPLY_LEN`]-byte message: the
/// [`CP00_REPLY_STRING_LEN`] bytes the `STRING` fills and byte 64, which the
/// `STRING WITH POINTER` length includes but nothing sets. One request per task
/// makes this the first-message reply (E-13, P-COPAUA0C-027).
#[must_use]
pub fn cp00_reply(
    card_num: &str,
    transaction_id: &str,
    auth_id_code: &str,
    resp_code: &str,
    resp_reason: &str,
    approved_amt: Money<10>,
) -> String {
    let amount = edit_money(&PIC_CP00_APPROVED_AMT, approved_amt);
    let mut reply = group(&[
        (card_num, 16),
        (",", 1),
        (transaction_id, 15),
        (",", 1),
        (auth_id_code, 6),
        (",", 1),
        (resp_code, 2),
        (",", 1),
        (resp_reason, 4),
        (",", 1),
        (&amount, 14),
        (",", 1),
    ]);
    // Byte 64 comes from W02-PUT-BUFFER, which has no VALUE, so the COBOL byte is
    // unpredictable (E-25). It is written as a space, the fill E-25 and
    // P-CVTRA05Y-018 record for never-set bytes; parity comparisons exclude it.
    reply.push(' ');
    reply
}

/// The account fields COACCT01 moves into `WS-ACCT-RESPONSE`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L406-427] after a successful read —
/// P-COACCT01-013.
///
/// Its `Debug` output withholds the balance, the two limits and the cycle
/// totals, which are financial data, while [`acct_response`] still writes them
/// into the reply.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct AccountResponse<'a> {
    /// `ACCT-ID 9(11)`, into `WS-ACCT-ID 9(11)`.
    pub acct_id: u64,
    /// `ACCT-ACTIVE-STATUS`, into `WS-ACCT-ACTIVE-STATUS X(01)`.
    pub active_status: &'a str,
    /// `ACCT-CURR-BAL`, into `WS-ACCT-CURR-BAL S9(10)V99`.
    pub curr_bal: Money<10>,
    /// `ACCT-CREDIT-LIMIT`, into `WS-ACCT-CREDIT-LIMIT S9(10)V99`.
    pub credit_limit: Money<10>,
    /// `ACCT-CASH-CREDIT-LIMIT`, into `WS-ACCT-CASH-CREDIT-LIMIT S9(10)V99`.
    pub cash_credit_limit: Money<10>,
    /// `ACCT-OPEN-DATE`, into `WS-ACCT-OPEN-DATE X(10)`.
    pub open_date: &'a str,
    /// `ACCT-EXPIRAION-DATE`, into `WS-ACCT-EXPIRAION-DATE X(10)`.
    pub expiration_date: &'a str,
    /// `ACCT-REISSUE-DATE`, into `WS-ACCT-REISSUE-DATE X(10)`.
    pub reissue_date: &'a str,
    /// `ACCT-CURR-CYC-CREDIT`, into `WS-ACCT-CURR-CYC-CREDIT S9(10)V99`.
    pub curr_cyc_credit: Money<10>,
    /// `ACCT-CURR-CYC-DEBIT`, into `WS-ACCT-CURR-CYC-DEBIT S9(10)V99`.
    pub curr_cyc_debit: Money<10>,
    /// `ACCT-GROUP-ID`, into `WS-ACCT-GROUP-ID X(10)`.
    pub group_id: &'a str,
}

/// Every field in declaration order, the five money fields shown as
/// `<redacted>`. The exhaustive pattern makes a new field a compile error until
/// it is listed.
impl fmt::Debug for AccountResponse<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            acct_id,
            active_status,
            curr_bal: _,
            credit_limit: _,
            cash_credit_limit: _,
            open_date,
            expiration_date,
            reissue_date,
            curr_cyc_credit: _,
            curr_cyc_debit: _,
            group_id,
        } = self;
        f.debug_struct("AccountResponse")
            .field("acct_id", acct_id)
            .field("active_status", active_status)
            .field("curr_bal", &format_args!("{REDACTED}"))
            .field("credit_limit", &format_args!("{REDACTED}"))
            .field("cash_credit_limit", &format_args!("{REDACTED}"))
            .field("open_date", open_date)
            .field("expiration_date", expiration_date)
            .field("reissue_date", reissue_date)
            .field("curr_cyc_credit", &format_args!("{REDACTED}"))
            .field("curr_cyc_debit", &format_args!("{REDACTED}"))
            .field("group_id", group_id)
            .finish()
    }
}

/// COACCT01 `WS-ACCT-RESPONSE` [app/app-vsam-mq/cbl/COACCT01.cbl:L130-169], the
/// 252 bytes of the CDRA reply for a found account: each `VALUE` label followed
/// by its field, the money fields unedited zoned ([`unedited_zoned`]) —
/// P-COACCT01-013, P-COACCT01-020. [`pad_reply_1000`] makes it the reply.
#[must_use]
pub fn acct_response(account: &AccountResponse<'_>) -> String {
    let acct_id = unsigned_display(account.acct_id, 11);
    let curr_bal = unedited_zoned(account.curr_bal);
    let credit_limit = unedited_zoned(account.credit_limit);
    let cash_credit_limit = unedited_zoned(account.cash_credit_limit);
    let curr_cyc_credit = unedited_zoned(account.curr_cyc_credit);
    let curr_cyc_debit = unedited_zoned(account.curr_cyc_debit);
    group(&[
        ("ACCOUNT ID : ", 13),
        (&acct_id, 11),
        ("ACCOUNT STATUS : ", 17),
        (account.active_status, 1),
        ("BALANCE : ", 10),
        (&curr_bal, 12),
        ("CREDIT LIMIT : ", 15),
        (&credit_limit, 12),
        ("CASH LIMIT : ", 13),
        (&cash_credit_limit, 12),
        ("OPEN DATE : ", 12),
        (account.open_date, 10),
        ("EXPR DATE : ", 12),
        (account.expiration_date, 10),
        ("REIS DATE : ", 12),
        (account.reissue_date, 10),
        ("CREDIT BAL : ", 13),
        (&curr_cyc_credit, 12),
        ("DEBIT BAL : ", 12),
        (&curr_cyc_debit, 12),
        ("GROUP ID : ", 11),
        (account.group_id, 10),
    ])
}

/// The CODATE01 label before the date, `'SYSTEM DATE : '`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L355] — P-CODATE01-013.
pub const SYSTEM_DATE_LABEL: &str = "SYSTEM DATE : ";

/// The CODATE01 label before the time, `'SYSTEM TIME : '`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L356] — P-CODATE01-013.
pub const SYSTEM_TIME_LABEL: &str = "SYSTEM TIME : ";

/// The CODATE01 CDRD reply text [app/app-vsam-mq/cbl/CODATE01.cbl:L355-360]:
/// `STRING 'SYSTEM DATE : ' WS-MMDDYYYY 'SYSTEM TIME : ' WS-TIME DELIMITED BY
/// SIZE`, with `WS-MMDDYYYY X(10)` (`mm-dd-yyyy`) and `WS-TIME X(8)`
/// (`hh:mm:ss`) as `crate::clock` formats them: 46 bytes with no separator
/// between the date and the second label — P-CODATE01-013, P-CODATE01-020,
/// P-CODATE01-021. [`pad_reply_1000`] makes it the reply.
#[must_use]
pub fn date_reply(mmddyyyy: &str, hhmmss: &str) -> String {
    group(&[
        (SYSTEM_DATE_LABEL, 14),
        (mmddyyyy, 10),
        (SYSTEM_TIME_LABEL, 14),
        (hhmmss, 8),
    ])
}

/// A CDRA or CDRD reply body in `REPLY-MESSAGE PIC X(1000)`: `body` padded with
/// spaces (or cut) to the 1000 bytes the reply put sends — P-CODATE01-013,
/// P-CODATE01-014, P-COACCT01-013, P-COACCT01-017.
#[must_use]
pub fn pad_reply_1000(body: &str) -> String {
    move_alnum(body, REPLY_BUFFER_LEN)
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;

    use super::{
        AccountResponse, CP00_REPLY_LEN, CP00_REPLY_STRING_LEN, EditedPicture, HTML_RECORD_LEN,
        HtmlFixedLine, PIC_AUTH_AMT, PIC_BILL_CURR_BAL, PIC_CCPAU_AMT, PIC_CP00_APPROVED_AMT,
        PIC_REPORT_AMT, PIC_REPORT_TOTAL, PIC_SQLCODE, PIC_STMT_CURR_BAL, PIC_STMT_TRAN_AMT,
        PIC_TRAN_AMT_DISPLAY, PictureError, REPLY_BUFFER_LEN, REPORT_RECORD_LEN, ReportDetail,
        STMT_RECORD_LEN, acct_response, cp00_reply, date_reply, delimited_by, edit, edit_expr,
        edit_money, edit_sqlcode, html_acct_ln, html_add1_ln, html_add2_ln, html_add3_ln,
        html_bal_ln, html_fico_ln, html_l11, html_l23, html_tranamt_ln, html_trandt_ln,
        html_tranid_ln, move_alnum, pad_reply_1000, report_account_total, report_blank_line,
        report_detail_line, report_grand_total, report_header_1, report_header_2,
        report_name_header, report_page_total, st_line0, st_line1, st_line2, st_line3, st_line4,
        st_line5, st_line6, st_line7, st_line8, st_line9, st_line10, st_line11, st_line12,
        st_line13, st_line14, st_line14a, st_line15, unedited_zoned, unsigned_display,
    };
    use crate::numeric::{Money, MoneyExpr};

    fn dec(text: &str) -> Decimal {
        text.parse().expect("test decimal literal")
    }

    fn money<const D: u8>(text: &str) -> Money<D> {
        Money::from_decimal(dec(text)).expect("test amount fits the field")
    }

    /// `text` right-padded with spaces to `width`, as the audit's `width=<n>`.
    fn padded(text: &str, width: usize) -> String {
        format!("{text:<width$}")
    }

    #[test]
    fn named_pictures_keep_their_pic_clause_and_width() {
        let cases = [
            (PIC_REPORT_AMT, "-ZZZ,ZZZ,ZZZ.ZZ", 15),
            (PIC_REPORT_TOTAL, "+ZZZ,ZZZ,ZZZ.ZZ", 15),
            (PIC_CP00_APPROVED_AMT, "-zzzzzzzzz9.99", 14),
            (PIC_STMT_CURR_BAL, "9(9).99-", 13),
            (PIC_STMT_TRAN_AMT, "Z(9).99-", 13),
            (PIC_SQLCODE, "----9", 5),
            (PIC_TRAN_AMT_DISPLAY, "+99999999.99", 12),
            (PIC_BILL_CURR_BAL, "+9999999999.99", 14),
            (PIC_AUTH_AMT, "-zzzzzzz9.99", 12),
            (PIC_CCPAU_AMT, "+9(10).99", 14),
        ];
        for (picture, text, width) in cases {
            assert_eq!(picture.text(), text);
            assert_eq!(picture.to_string(), text);
            assert_eq!(picture.width(), width, "{text}");
        }
    }

    /// Audited examples (rows cited per picture) plus positive, negative, zero,
    /// high-order truncation and fraction truncation cases.
    #[test]
    fn named_pictures_edit_values_as_the_cobol_move() {
        let cases: [(EditedPicture, &str, &str); 45] = [
            // P-CBTRN03C-014
            (PIC_REPORT_AMT, "183.88", "         183.88"),
            (PIC_REPORT_AMT, "-47.88", "-         47.88"),
            (PIC_REPORT_AMT, "-1234.5", "-      1,234.50"),
            (PIC_REPORT_AMT, "0", "               "),
            (PIC_REPORT_AMT, "0.05", "            .05"),
            (PIC_REPORT_AMT, "10.006", "          10.00"),
            (PIC_REPORT_AMT, "1234567890.12", " 234,567,890.12"),
            // P-CBTRN03C-011, P-CBTRN03C-012
            (PIC_REPORT_TOTAL, "1725.65", "+      1,725.65"),
            (PIC_REPORT_TOTAL, "78720.36", "+     78,720.36"),
            (PIC_REPORT_TOTAL, "-47.88", "-         47.88"),
            (PIC_REPORT_TOTAL, "0.00", "               "),
            (PIC_REPORT_TOTAL, "999999999.99", "+999,999,999.99"),
            (PIC_REPORT_TOTAL, "1000000000.00", "               "),
            // P-COPAUA0C-020, P-COPAUA0C-021 to P-COPAUA0C-025
            (PIC_CP00_APPROVED_AMT, "100.29", "        100.29"),
            (PIC_CP00_APPROVED_AMT, "0", "          0.00"),
            (PIC_CP00_APPROVED_AMT, "-12.50", "-        12.50"),
            (PIC_CP00_APPROVED_AMT, "5677.00", "       5677.00"),
            (PIC_CP00_APPROVED_AMT, "-50.00", "-        50.00"),
            (PIC_CP00_APPROVED_AMT, "9999999999.99", " 9999999999.99"),
            // P-CBSTM03A-100
            (PIC_STMT_CURR_BAL, "1945.87", "000001945.87 "),
            (PIC_STMT_CURR_BAL, "-5.5", "000000005.50-"),
            (PIC_STMT_CURR_BAL, "0", "000000000.00 "),
            (PIC_STMT_CURR_BAL, "1234567890.12", "234567890.12 "),
            // P-CBSTM03A-013
            (PIC_STMT_TRAN_AMT, "183.88", "      183.88 "),
            (PIC_STMT_TRAN_AMT, "-47.88", "       47.88-"),
            (PIC_STMT_TRAN_AMT, "0", "         .00 "),
            (PIC_STMT_TRAN_AMT, "1472.64", "     1472.64 "),
            (PIC_STMT_TRAN_AMT, "0.07", "         .07 "),
            // P-COTRN00C-008, P-COTRN01C-005
            (PIC_TRAN_AMT_DISPLAY, "-5.00", "-00000005.00"),
            (PIC_TRAN_AMT_DISPLAY, "183.88", "+00000183.88"),
            (PIC_TRAN_AMT_DISPLAY, "0", "+00000000.00"),
            (PIC_TRAN_AMT_DISPLAY, "123456789.01", "+23456789.01"),
            // A negative value that keeps only zeros, and a negative zero, show
            // the sign of zero (E-42).
            (PIC_TRAN_AMT_DISPLAY, "-0.009", "+00000000.00"),
            (PIC_TRAN_AMT_DISPLAY, "-0.00", "+00000000.00"),
            // P-COBIL00C-023
            (PIC_BILL_CURR_BAL, "1940.00", "+0000001940.00"),
            (PIC_BILL_CURR_BAL, "-5.00", "-0000000005.00"),
            (PIC_BILL_CURR_BAL, "0", "+0000000000.00"),
            // P-COPAUS0C-013
            (PIC_AUTH_AMT, "100.29", "      100.29"),
            (PIC_AUTH_AMT, "-0.5", "-       0.50"),
            (PIC_AUTH_AMT, "0", "        0.00"),
            (PIC_AUTH_AMT, "123456789.00", " 23456789.00"),
            // P-CCPAURLY-006
            (PIC_CCPAU_AMT, "100.29", "+0000000100.29"),
            (PIC_CCPAU_AMT, "-50", "-0000000050.00"),
            (PIC_CCPAU_AMT, "0", "+0000000000.00"),
            (PIC_CCPAU_AMT, "12345678901.23", "+2345678901.23"),
        ];
        for (picture, value, expected) in cases {
            assert_eq!(edit(&picture, dec(value)), expected, "{picture} of {value}");
            assert_eq!(expected.len(), picture.width());
        }
    }

    #[test]
    fn sqlcode_edits_through_the_floating_minus_string() {
        let cases = [
            (-532, " -532"),
            (-803, " -803"),
            (-911, " -911"),
            (100, "  100"),
            (0, "    0"),
            (-5, "   -5"),
            (12_345, " 2345"),
            (-12_345, "-2345"),
            (-10_000, "    0"),
        ];
        for (code, expected) in cases {
            assert_eq!(edit_sqlcode(code), expected, "{code}");
        }
    }

    #[test]
    fn parsed_pictures_follow_the_editing_rules() {
        let cases = [
            // Every digit position Z: a zero value is all spaces.
            ("ZZZ.ZZ", "0", "      "),
            ("ZZZ.ZZ", "0.05", "   .05"),
            ("ZZZ.ZZ", "12.3", " 12.30"),
            // A floating string with no 9 blanks a zero value too.
            ("----", "0", "    "),
            ("----", "-12", " -12"),
            ("+++9", "0", "  +0"),
            ("+++9", "-7", "  -7"),
            ("+++9", "123", "+123"),
            ("+++9", "-1234", "-234"),
            // Lower-case z, repetition and fixed signs.
            ("zz9", "7", "  7"),
            ("Z(3)", "5", "  5"),
            ("-9", "3", " 3"),
            ("-9", "-3", "-3"),
            ("+9", "3", "+3"),
            ("9-", "-3", "3-"),
            ("9+", "3", "3+"),
            ("9(3)", "12", "012"),
            ("99,999", "1234", "01,234"),
            ("ZZ,ZZ9", "999", "   999"),
            ("ZZ,ZZ9", "1000", " 1,000"),
            (".99", "1.25", ".25"),
        ];
        for (text, value, expected) in cases {
            let picture = EditedPicture::parse(text).expect("supported picture");
            assert_eq!(edit(&picture, dec(value)), expected, "{text} of {value}");
            assert_eq!(picture.width(), expected.len(), "{text}");
        }
        assert_eq!(
            EditedPicture::parse("zz9"),
            EditedPicture::parse("ZZ9").map(|picture| EditedPicture {
                text: "zz9",
                ..picture
            })
        );
    }

    #[test]
    fn unsupported_pictures_are_rejected_with_their_reason() {
        let cases = [
            ("", PictureError::Empty),
            ("X(5)", PictureError::UnsupportedSymbol { position: 0 }),
            ("9V99", PictureError::UnsupportedSymbol { position: 1 }),
            ("9(", PictureError::InvalidRepetition { position: 1 }),
            ("9()", PictureError::InvalidRepetition { position: 1 }),
            ("9(0)", PictureError::InvalidRepetition { position: 1 }),
            ("9(a)", PictureError::InvalidRepetition { position: 1 }),
            ("(3)9", PictureError::InvalidRepetition { position: 0 }),
            ("9.(2)", PictureError::InvalidRepetition { position: 2 }),
            ("9(33)", PictureError::TooLong),
            ("9(20)9(13)", PictureError::TooLong),
            ("9.9.9", PictureError::MultiplePoints),
            ("+-9", PictureError::InvalidSign),
            ("9-9", PictureError::InvalidSign),
            ("-9-", PictureError::InvalidSign),
            ("--Z9", PictureError::SuppressionWithFloat),
            ("9Z", PictureError::SuppressionAfterDigit),
            ("Z.Z9", PictureError::SuppressionAfterDigit),
            ("-", PictureError::NoDigits),
            (".,", PictureError::NoDigits),
            ("9(29)", PictureError::TooManyDigits),
        ];
        for (text, error) in cases {
            assert_eq!(EditedPicture::parse(text), Err(error), "{text:?}");
        }
    }

    #[test]
    fn picture_errors_report_their_texts() {
        let cases = [
            (PictureError::Empty, "the picture string is empty"),
            (
                PictureError::UnsupportedSymbol { position: 3 },
                "unsupported picture symbol at position 3",
            ),
            (
                PictureError::InvalidRepetition { position: 1 },
                "invalid repetition at position 1",
            ),
            (
                PictureError::TooLong,
                "the picture has more than 32 positions",
            ),
            (
                PictureError::MultiplePoints,
                "the picture has more than one decimal point",
            ),
            (
                PictureError::InvalidSign,
                "the picture's sign symbols are mixed or misplaced",
            ),
            (
                PictureError::SuppressionWithFloat,
                "zero suppression cannot be combined with a floating sign",
            ),
            (
                PictureError::SuppressionAfterDigit,
                "a zero-suppression position follows a digit position",
            ),
            (PictureError::NoDigits, "the picture has no digit position"),
            (
                PictureError::TooManyDigits,
                "the picture has more than 28 digit positions",
            ),
        ];
        for (error, text) in cases {
            assert_eq!(error.to_string(), text);
        }
    }

    #[test]
    fn money_and_expression_conveniences_edit_their_decimal() {
        let balance = money::<10>("1945.87");
        let amount = money::<9>("-47.88");
        assert_eq!(
            edit_money(&PIC_STMT_CURR_BAL, balance),
            edit(&PIC_STMT_CURR_BAL, balance.to_decimal())
        );
        let total: MoneyExpr = balance + amount;
        assert_eq!(edit_expr(&PIC_REPORT_TOTAL, &total), "+      1,897.99");
        assert_eq!(edit_money(&PIC_REPORT_AMT, amount), "-         47.88");
    }

    #[test]
    fn alphanumeric_move_pads_and_cuts_in_bytes() {
        let cases = [
            ("POS TERM", 10, "POS TERM  "),
            ("Regular Sales Draft", 15, "Regular Sales D"),
            ("ABCDE", 5, "ABCDE"),
            ("", 3, "   "),
            ("X", 0, ""),
            // A character straddling the cut is dropped and its byte is a space.
            ("a\u{e9}", 2, "a "),
        ];
        for (text, width, expected) in cases {
            let moved = move_alnum(text, width);
            assert_eq!(moved, expected, "{text:?} into X({width})");
            assert_eq!(moved.len(), width);
        }
    }

    #[test]
    fn unsigned_display_keeps_the_low_order_digits() {
        let cases = [
            (50, 11, "00000000050"),
            (623, 3, "623"),
            (78, 3, "078"),
            (12_345, 4, "2345"),
            (1, 4, "0001"),
            (u64::MAX, 20, "18446744073709551615"),
            (7, 0, ""),
        ];
        for (value, digits, expected) in cases {
            assert_eq!(
                unsigned_display(value, digits),
                expected,
                "{value} as 9({digits})"
            );
        }
    }

    #[test]
    fn delimited_by_stops_before_the_first_delimiter() {
        let cases = [
            ("Aniya Alba Von  ", "  ", "Aniya Alba Von"),
            ("Aniya Alba Von", "  ", "Aniya Alba Von"),
            ("  leading", "  ", ""),
            ("Aniya Alba", " ", "Aniya"),
            ("12*34", "*", "12"),
            ("no star", "*", "no star"),
            ("whole", "", "whole"),
        ];
        for (source, delimiter, expected) in cases {
            assert_eq!(delimited_by(source, delimiter), expected, "{source:?}");
        }
    }

    #[test]
    fn unedited_zoned_overpunches_the_last_digit() {
        assert_eq!(unedited_zoned(money::<10>("194.00")), "00000001940{");
        assert_eq!(unedited_zoned(money::<10>("2020.00")), "00000020200{");
        assert_eq!(unedited_zoned(money::<10>("0")), "00000000000{");
        assert_eq!(unedited_zoned(money::<10>("12.34")), "00000000123D");
        assert_eq!(unedited_zoned(money::<10>("-1.23")), "00000000012L");
        assert_eq!(unedited_zoned(money::<10>("-0.10")), "00000000001}");
        assert_eq!(unedited_zoned(money::<9>("9.99")), "0000000099I");
        assert_eq!(unedited_zoned(money::<9>("-9.99")), "0000000099R");
    }

    #[test]
    fn report_lines_equal_the_audited_tranrept_records() {
        let purchase = ReportDetail {
            tran_id: "0000000058866561",
            account_id: 50,
            type_cd: "01",
            type_desc: "Purchase",
            cat_cd: 1,
            cat_desc: "Regular Sales Draft",
            source: "POS TERM",
            amount: money("183.88"),
        };
        let credit = ReportDetail {
            tran_id: "0000000577826814",
            type_cd: "03",
            type_desc: "Credit",
            cat_desc: "Credit to Account",
            source: "OPERATOR",
            amount: money("-47.88"),
            ..purchase
        };
        let cases = [
            // P-CBTRN03C-016
            (
                report_name_header("2022-01-01", "2022-07-06"),
                "DALYREPT                              Daily Transaction Report                 Date Range: 2022-01-01 to 2022-07-06",
            ),
            // P-CBTRN03C-017
            (report_blank_line(), ""),
            // P-CBTRN03C-018
            (
                report_header_1(),
                "Transaction ID   Account ID  Transaction Type   Tran Category                      Tran Source            Amount",
            ),
            // P-CBTRN03C-019
            (
                report_header_2(),
                "-------------------------------------------------------------------------------------------------------------------------------------",
            ),
            // P-CBTRN03C-020
            (
                report_detail_line(&purchase),
                "0000000058866561 00000000050 01-Purchase        0001-Regular Sales Draft           POS TERM               183.88",
            ),
            // P-CBTRN03C-023
            (
                report_detail_line(&credit),
                "0000000577826814 00000000050 03-Credit          0001-Credit to Account             OPERATOR      -         47.88",
            ),
            // P-CBTRN03C-026
            (
                report_account_total(money("1453.87")),
                "Account Total....................................................................................+      1,453.87",
            ),
            // P-CBTRN03C-036
            (
                report_page_total(money("4665.52")),
                "Page Total ......................................................................................+      4,665.52",
            ),
            // P-CBTRN03C-466
            (
                report_grand_total(money("78720.36")),
                "Grand Total......................................................................................+     78,720.36",
            ),
            // A zero total is all spaces in the amount (P-CBTRN03C-012).
            (
                report_grand_total(Money::<9>::ZERO),
                "Grand Total......................................................................................",
            ),
        ];
        for (line, audited) in cases {
            assert_eq!(line, padded(audited, REPORT_RECORD_LEN));
            assert_eq!(line.len(), REPORT_RECORD_LEN);
        }
    }

    #[test]
    fn statement_lines_equal_the_audited_stmtfile_records() {
        let cases = [
            // P-CBSTM03A-016
            (
                st_line0(),
                "*******************************START OF STATEMENT*******************************",
            ),
            // P-CBSTM03A-095 to P-CBSTM03A-098, P-CBSTM03A-122
            (st_line1("Aniya Alba Von"), "Aniya Alba Von"),
            (st_line2("1588 Nienow Cape"), "1588 Nienow Cape"),
            (st_line3("Suite 187"), "Suite 187"),
            (st_line4("New OR USA 04257"), "New OR USA 04257"),
            (st_line4("Port GU USA 07923-8822"), "Port GU USA 07923-8822"),
            // P-CBSTM03A-017, P-CBSTM03A-019, P-CBSTM03A-020, P-CBSTM03A-022
            (
                st_line5(),
                "--------------------------------------------------------------------------------",
            ),
            (
                st_line10(),
                "--------------------------------------------------------------------------------",
            ),
            (
                st_line12(),
                "--------------------------------------------------------------------------------",
            ),
            // P-CBSTM03A-018
            (st_line6(), "                                 Basic Details"),
            // P-CBSTM03A-099 to P-CBSTM03A-101, P-CBSTM03A-125
            (st_line7(50), "Account ID         :00000000050"),
            (
                st_line8(money("1945.87")),
                "Current Balance    :000001945.87",
            ),
            (st_line9(623), "FICO Score         :623"),
            (st_line9(78), "FICO Score         :078"),
            // P-CBSTM03A-021
            (
                st_line11(),
                "                              TRANSACTION SUMMARY",
            ),
            // P-CBSTM03A-023
            (
                st_line13(),
                "Tran ID         Tran Details                                         Tran Amount",
            ),
            // P-CBSTM03A-102, P-CBSTM03A-105
            (
                st_line14(
                    "0000000058866561",
                    "Purchase at Blick-Rippin",
                    money("183.88"),
                ),
                "0000000058866561 Purchase at Blick-Rippin                         $      183.88",
            ),
            (
                st_line14(
                    "0000000577826814",
                    "Return item at DuBuque, Wuckert and Mraz",
                    money("-47.88"),
                ),
                "0000000577826814 Return item at DuBuque, Wuckert and Mraz         $       47.88-",
            ),
            // P-CBSTM03A-109
            (
                st_line14a(money("1472.64")),
                "Total EXP:                                                        $     1472.64",
            ),
            // P-CBSTM03A-026
            (
                st_line15(),
                "********************************END OF STATEMENT********************************",
            ),
        ];
        for (line, audited) in cases {
            assert_eq!(line, padded(audited, STMT_RECORD_LEN));
            assert_eq!(line.len(), STMT_RECORD_LEN);
        }
        // ST-TRANDT holds the first 49 bytes of TRNX-DESC (P-CBSTM03A-007).
        let long = st_line14("1", &"D".repeat(100), Money::ZERO);
        assert_eq!(&long[17..66], "D".repeat(49));
        assert_eq!(&long[66..], "$         .00 ");
    }

    #[test]
    fn fixed_html_lines_equal_the_audited_htmlfile_records() {
        let cases = [
            (HtmlFixedLine::L01, "<!DOCTYPE html>"),
            (HtmlFixedLine::L02, "<html lang=\"en\">"),
            (HtmlFixedLine::L03, "<head>"),
            (HtmlFixedLine::L04, "<meta charset=\"utf-8\">"),
            (HtmlFixedLine::L05, "<title>HTML Table Layout</title>"),
            (HtmlFixedLine::L06, "</head>"),
            (HtmlFixedLine::L07, "<body style=\"margin:0px;\">"),
            (
                HtmlFixedLine::L08,
                "<table  align=\"center\" frame=\"box\" style=\"width:70%; font:12px Segoe UI,sans-serif;\">",
            ),
            (HtmlFixedLine::Ltrs, "<tr>"),
            (
                HtmlFixedLine::L10,
                "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#1d1d96b3;\">",
            ),
            (HtmlFixedLine::Ltde, "</td>"),
            (HtmlFixedLine::Ltre, "</tr>"),
            (
                HtmlFixedLine::L15,
                "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#FFAF33;\">",
            ),
            (
                HtmlFixedLine::L16,
                "<p style=\"font-size:16px\">Bank of XYZ</p>",
            ),
            (HtmlFixedLine::L17, "<p>410 Terry Ave N</p>"),
            (HtmlFixedLine::L18, "<p>Seattle WA 99999</p>"),
            (
                HtmlFixedLine::L22And35,
                "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#f2f2f2;\">",
            ),
            (
                HtmlFixedLine::L30And42,
                "<td colspan=\"3\" style=\"padding:0px 5px;background-color:#33FFD1; text-align:center;\">",
            ),
            (
                HtmlFixedLine::L31,
                "<p style=\"font-size:16px\">Basic Details</p>",
            ),
            (
                HtmlFixedLine::L43,
                "<p style=\"font-size:16px\">Transaction Summary</p>",
            ),
            (
                HtmlFixedLine::L47,
                "<td style=\"width:25%; padding:0px 5px; background-color:#33FF5E; text-align:left;\">",
            ),
            (
                HtmlFixedLine::L48,
                "<p style=\"font-size:16px\">Tran ID</p>",
            ),
            (
                HtmlFixedLine::L50,
                "<td style=\"width:55%; padding:0px 5px; background-color:#33FF5E; text-align:left;\">",
            ),
            (
                HtmlFixedLine::L51,
                "<p style=\"font-size:16px\">Tran Details</p>",
            ),
            (
                HtmlFixedLine::L53,
                "<td style=\"width:20%; padding:0px 5px; background-color:#33FF5E; text-align:right;\">",
            ),
            (HtmlFixedLine::L54, "<p style=\"font-size:16px\">Amount</p>"),
            (
                HtmlFixedLine::L58,
                "<td style=\"width:25%; padding:0px 5px; background-color:#f2f2f2; text-align:left;\">",
            ),
            (
                HtmlFixedLine::L61,
                "<td style=\"width:55%; padding:0px 5px; background-color:#f2f2f2; text-align:left;\">",
            ),
            (
                HtmlFixedLine::L64,
                "<td style=\"width:20%; padding:0px 5px; background-color:#f2f2f2; text-align:right;\">",
            ),
            (HtmlFixedLine::L75, "<h3>End of Statement</h3>"),
            (HtmlFixedLine::L78, "</table>"),
            (HtmlFixedLine::L79, "</body>"),
            (HtmlFixedLine::L80, "</html>"),
        ];
        for (line, audited) in cases {
            assert_eq!(line.text(), audited, "{line:?}");
            assert_eq!(line.record(), padded(audited, HTML_RECORD_LEN));
            assert_eq!(line.record().len(), HTML_RECORD_LEN);
        }
    }

    #[test]
    fn built_html_lines_equal_the_audited_htmlfile_records() {
        let cases = [
            // P-CBSTM03A-110
            (
                html_l11(50),
                "<h3>Statement for Account Number: 00000000050         </h3>",
            ),
            // P-CBSTM03A-111 to P-CBSTM03A-114, P-CBSTM03A-1252
            (
                html_l23("Aniya Alba Von"),
                "<p style=\"font-size:16px\">Aniya Alba Von  </p>",
            ),
            (
                html_add1_ln("1588 Nienow Cape"),
                "<p>1588 Nienow Cape  </p>",
            ),
            (html_add2_ln("Suite 187"), "<p>Suite 187  </p>"),
            (
                html_add3_ln("New OR USA 04257"),
                "<p>New OR USA 04257  </p>",
            ),
            (
                html_add3_ln("Erniechester TX USA 87156-8689"),
                "<p>Erniechester TX USA 87156-8689  </p>",
            ),
            // P-CBSTM03A-115 to P-CBSTM03A-117
            (
                html_acct_ln(50),
                "<p>Account ID         : 00000000050         </p>",
            ),
            (
                html_bal_ln(money("1945.87")),
                "<p>Current Balance    : 000001945.87 </p>",
            ),
            (
                html_fico_ln(623),
                "<p>FICO Score         : 623                 </p>",
            ),
            // P-CBSTM03A-091 to P-CBSTM03A-093: the STMTFILE fields of the line.
            (
                html_tranid_ln("0000000058866561"),
                "<p>0000000058866561</p>",
            ),
            (
                html_trandt_ln("Purchase at Blick-Rippin"),
                "<p>Purchase at Blick-Rippin                         </p>",
            ),
            (html_tranamt_ln(money("-47.88")), "<p>       47.88-</p>"),
            // DELIMITED BY '*' stops at an asterisk; '  ' at a double space of
            // the moved field, so a full field is copied whole.
            (html_trandt_ln("a*b"), "<p>a</p>"),
            (
                html_add1_ln(&"A".repeat(60)),
                &["<p>", &"A".repeat(50), "  </p>"].concat(),
            ),
            (
                html_l23("First  Last"),
                "<p style=\"font-size:16px\">First  </p>",
            ),
        ];
        for (line, audited) in cases {
            assert_eq!(line, padded(audited, HTML_RECORD_LEN));
            assert_eq!(line.len(), HTML_RECORD_LEN);
        }
    }

    #[test]
    fn cp00_replies_equal_the_audited_reply_bytes() {
        // P-COPAUA0C-021, P-COPAUA0C-023, P-COPAUA0C-024, P-COPAUA0C-026: the
        // 63 STRING bytes, then byte 64, never set by the COBOL ({UNDEF:1}),
        // written as a space (E-25).
        let cases = [
            (
                cp00_reply(
                    "0500024453765740",
                    "TXN000000000001",
                    "120000",
                    "00",
                    "0000",
                    money("100.00"),
                ),
                "0500024453765740,TXN000000000001,120000,00,0000,        100.00,",
            ),
            (
                cp00_reply(
                    "0500024453765740",
                    "TXN000000000001",
                    "120000",
                    "05",
                    "4100",
                    Money::ZERO,
                ),
                "0500024453765740,TXN000000000001,120000,05,4100,          0.00,",
            ),
            (
                cp00_reply(
                    "0500024453765740",
                    "TXN000000000001",
                    "120000",
                    "00",
                    "0000",
                    money("-50.00"),
                ),
                "0500024453765740,TXN000000000001,120000,00,0000,-        50.00,",
            ),
            (
                cp00_reply(
                    "9999999999999999",
                    "TXN000000000002",
                    "120000",
                    "05",
                    "3100",
                    Money::ZERO,
                ),
                "9999999999999999,TXN000000000002,120000,05,3100,          0.00,",
            ),
            // Short fields are space-padded, long ones cut, at CCPAURLY widths.
            (
                cp00_reply(
                    "4111",
                    "TXN-LONGER-THAN-15",
                    "1200",
                    "0",
                    "41",
                    money("1.00"),
                ),
                "4111            ,TXN-LONGER-THAN,1200  ,0 ,41  ,          1.00,",
            ),
        ];
        for (reply, string_bytes) in cases {
            assert_eq!(string_bytes.len(), CP00_REPLY_STRING_LEN);
            assert_eq!(reply, [string_bytes, " "].concat());
            assert_eq!(reply.len(), CP00_REPLY_LEN);
        }
    }

    #[test]
    fn cdra_response_equals_the_audited_reply_bytes() {
        // P-COACCT01-020: account 00000000001 of the seeded state.
        let account = AccountResponse {
            acct_id: 1,
            active_status: "Y",
            curr_bal: money("194.00"),
            credit_limit: money("2020.00"),
            cash_credit_limit: money("1020.00"),
            open_date: "2014-11-20",
            expiration_date: "2025-05-20",
            reissue_date: "2025-05-20",
            curr_cyc_credit: Money::ZERO,
            curr_cyc_debit: Money::ZERO,
            group_id: "",
        };
        let response = acct_response(&account);
        assert_eq!(response.len(), 252);
        let reply = pad_reply_1000(&response);
        assert_eq!(
            reply,
            padded(
                "ACCOUNT ID : 00000000001ACCOUNT STATUS : YBALANCE : 00000001940{CREDIT LIMIT : 00000020200{CASH LIMIT : 00000010200{OPEN DATE : 2014-11-20EXPR DATE : 2025-05-20REIS DATE : 2025-05-20CREDIT BAL : 00000000000{DEBIT BAL : 00000000000{GROUP ID :",
                REPLY_BUFFER_LEN,
            )
        );
        let negative = AccountResponse {
            curr_bal: money("-12.34"),
            group_id: "DEFAULT",
            ..account
        };
        let response = acct_response(&negative);
        assert_eq!(&response[52..64], "00000000123M");
        assert_eq!(&response[242..], "DEFAULT   ");
    }

    #[test]
    fn debug_withholds_amounts_and_transaction_source() {
        // The P-CBTRN03C-020 detail, its source replaced by a marker no other
        // field holds, and the P-COACCT01-020 account above.
        let detail = ReportDetail {
            tran_id: "0000000058866561",
            account_id: 50,
            type_cd: "01",
            type_desc: "Purchase",
            cat_cd: 1,
            cat_desc: "Regular Sales Draft",
            source: "SRC-MARKER",
            amount: money("183.88"),
        };
        let account = AccountResponse {
            acct_id: 1,
            active_status: "Y",
            curr_bal: money("194.00"),
            credit_limit: money("2020.00"),
            cash_credit_limit: money("1020.00"),
            open_date: "2014-11-20",
            expiration_date: "2025-05-20",
            reissue_date: "2025-05-20",
            curr_cyc_credit: Money::ZERO,
            curr_cyc_debit: Money::ZERO,
            group_id: "",
        };
        // (compact, pretty, expected compact, a kept field, every withheld
        // value: each amount as `Money` displays it or as its cent count, and
        // the detail's source as set here and as the audited record has it)
        let cases = [
            (
                format!("{detail:?}"),
                format!("{detail:#?}"),
                "ReportDetail { tran_id: \"0000000058866561\", account_id: 50, type_cd: \"01\", type_desc: \"Purchase\", cat_cd: 1, cat_desc: \"Regular Sales Draft\", source: <redacted>, amount: <redacted> }",
                "tran_id: \"0000000058866561\"",
                &["183.88", "18388", "SRC-MARKER", "POS TERM"][..],
            ),
            (
                format!("{account:?}"),
                format!("{account:#?}"),
                "AccountResponse { acct_id: 1, active_status: \"Y\", curr_bal: <redacted>, credit_limit: <redacted>, cash_credit_limit: <redacted>, open_date: \"2014-11-20\", expiration_date: \"2025-05-20\", reissue_date: \"2025-05-20\", curr_cyc_credit: <redacted>, curr_cyc_debit: <redacted>, group_id: \"\" }",
                "open_date: \"2014-11-20\"",
                &[
                    "194.00", "2020.00", "1020.00", "0.00", "19400", "202000", "102000",
                ][..],
            ),
        ];
        for (compact, pretty, expected, kept, withheld) in cases {
            assert_eq!(compact, expected);
            for debug in [&compact, &pretty] {
                assert!(debug.contains("<redacted>"), "{debug}");
                assert!(debug.contains(kept), "{debug}");
                assert!(!debug.contains("Money"), "{debug}");
                for value in withheld {
                    assert!(!debug.contains(value), "{debug} shows {value}");
                }
            }
        }
    }

    #[test]
    fn cdrd_reply_concatenates_labels_date_and_time() {
        // P-CODATE01-013: 46 STRING bytes, then spaces to 1000.
        let text = date_reply("07-18-2022", "13:45:09");
        assert_eq!(text, "SYSTEM DATE : 07-18-2022SYSTEM TIME : 13:45:09");
        let reply = pad_reply_1000(&text);
        assert_eq!(reply.len(), REPLY_BUFFER_LEN);
        assert!(reply[46..].bytes().all(|byte| byte == b' '));
        assert_eq!(pad_reply_1000(&"X".repeat(1001)), "X".repeat(1000));
    }
}
