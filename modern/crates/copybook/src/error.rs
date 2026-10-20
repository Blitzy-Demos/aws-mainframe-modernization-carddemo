//! Typed codec errors.
//!
//! Every decoder in this crate reports a failure as one context struct, [`Error`], over one
//! class enum, [`ErrorKind`], so that field name, byte offset and width are reported the same
//! way whichever picture failed. Primitive decoders (`pic`, `zoned`) return a bare
//! [`ErrorKind`]; `record::RecordReader` and `framing` wrap it with the field it belongs to.
//!
//! Each anomaly class the copybook audit names (`modern/audit/copybooks.md`) has its own variant,
//! and decoders return it instead of defaulting or normalising the input (R7, Total Parsers Over
//! Untrusted Input).
//!
//! Neither type holds a byte, character or substring of the input, and neither prints one: a
//! failing field can be a card number, SSN, CVV or password, and errors reach logs, API error
//! bodies and test output (Cardholder Data Redaction). They carry field name, offset, width,
//! class and, for the length classes, the two lengths only. Keep it that way: no variant may
//! gain a text, byte-vector or character payload.

use std::fmt;

/// The class of a codec failure, independent of the field it occurred in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The input slice is not as long as the field or record it must fill: any fixed-width
    /// picture (`PIC X(n)`, `PIC 9(n)`, `PIC S9(n)V9(m)`, `COMP-3`, `COMP`, the `X(10)` dates and
    /// `X(26)` timestamps) or a whole record. A line body longer than its record is this class
    /// too (PIC-013).
    WidthMismatch {
        /// Width the picture or record declares, in bytes.
        expected: usize,
        /// Length of the input that was offered, in bytes.
        actual: usize,
    },
    /// A byte that must be an ASCII digit is not, spaces included: an unsigned `PIC 9(n)` field
    /// (PIC-004) or a leading digit of a signed DISPLAY `PIC S9(n)V9(m)` field (PIC-009). Digit
    /// positions of dates and timestamps report [`ErrorKind::InvalidDate`] and
    /// [`ErrorKind::InvalidTimestamp`] instead.
    NonDigit,
    /// The last byte of a signed DISPLAY `PIC S9(n)V9(m)` field is neither a digit nor one of the
    /// trailing overpunch characters `{`, `A`–`I`, `}`, `J`–`R` (PIC-009).
    InvalidOverpunch,
    /// A plain ASCII digit sits in the overpunch sign position of a signed DISPLAY
    /// `PIC S9(n)V9(m)` field. The corpus holds none, and it is never read as a positive value
    /// (PIC-007).
    PlainDigitSign,
    /// An all-zero magnitude carries a negative sign: overpunch `}` in a signed DISPLAY field
    /// (PIC-008) or sign nibble `D` in a `COMP-3` field (PIC-010). A decimal zero has no sign, so
    /// it could not re-encode the same byte.
    NegativeZero,
    /// A digit nibble of a packed-decimal `COMP-3` field is above 9 (PIC-010).
    InvalidPackedDigit,
    /// The sign nibble of a packed-decimal `COMP-3` field is neither `C` nor `D`; `F`, the
    /// unsigned sign, is rejected as well (PIC-010).
    InvalidPackedSign,
    /// The leading pad nibble of a `COMP-3` field with an even digit count is not 0 (PIC-010).
    NonZeroPadNibble,
    /// A value lies outside its picture's range: `PIC 9(n)` (`PicU`), `PIC S9(n)` (`PicS`),
    /// `PIC S9(4) COMP` (`Count`), a year outside `0..=9999`, or a time finer than a microsecond.
    /// Also checked arithmetic overflow and an unsupported width parameter (PIC-023).
    OutOfRange,
    /// A framed line body is shorter than its record and the missing tail is longer than the
    /// record's trailing `FILLER`, so the body ends inside a field that carries data (PIC-013,
    /// E04).
    ShortBodyNotFiller {
        /// Record width the copybook declares, in bytes.
        expected: usize,
        /// Length of the line body, in bytes.
        actual: usize,
    },
    /// A non-blank `PIC X(10)` date that is not `YYYY-MM-DD`, is partly blank, or names no
    /// calendar date, such as month 13 or 30 February (PIC-016).
    InvalidDate,
    /// A non-blank `PIC X(26)` timestamp whose text matches none of the layouts `I`, `D` and `S`
    /// (PIC-021, E33).
    UnknownTimestampLayout,
    /// A `PIC X(26)` timestamp in a known layout whose digits, separators or values are invalid
    /// (PIC-021), or a value its layout cannot carry, such as a form-`S` value with a time other
    /// than midnight (PIC-020).
    InvalidTimestamp,
    /// A timestamp layout code, as stored in a `<col>_form CHAR(1)` column, other than `I`, `D`
    /// and `S` (E33).
    UnknownFormCode,
    /// Input remains after the last field of a record.
    TrailingBytes,
    /// A byte outside printable ASCII (0x20–0x7E) in a `PIC X(n)` field: a control byte, a CR
    /// inside a line body, or a byte above 0x7E. `CHAR(n)` counts characters, not bytes, so such a
    /// byte could not round-trip (PIC-002).
    NonPrintable,
    /// Encoding a short-bodied line would drop record bytes that are not spaces: the bytes
    /// beyond the line's `body_len` must be the blank trailing `FILLER` its decode supplied
    /// (PIC-013).
    NonBlankTail,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WidthMismatch { expected, actual } => {
                write!(
                    f,
                    "width mismatch: expected {expected} bytes, found {actual}"
                )
            }
            Self::ShortBodyNotFiller { expected, actual } => write!(
                f,
                "short line body: expected {expected} bytes, found {actual}, \
                 and the missing tail is not trailing filler"
            ),
            Self::NonDigit => f.write_str("non-digit byte in a numeric field"),
            Self::InvalidOverpunch => f.write_str("invalid overpunch sign byte"),
            Self::PlainDigitSign => f.write_str("plain digit in the overpunch sign position"),
            Self::NegativeZero => f.write_str("negative zero"),
            Self::InvalidPackedDigit => f.write_str("invalid packed-decimal digit nibble"),
            Self::InvalidPackedSign => f.write_str("invalid packed-decimal sign nibble"),
            Self::NonZeroPadNibble => f.write_str("non-zero packed-decimal pad nibble"),
            Self::OutOfRange => f.write_str("value out of range for the picture"),
            Self::InvalidDate => f.write_str("invalid date text"),
            Self::UnknownTimestampLayout => f.write_str("unknown timestamp layout"),
            Self::InvalidTimestamp => f.write_str("invalid timestamp text"),
            Self::UnknownFormCode => f.write_str("unknown timestamp layout code"),
            Self::TrailingBytes => f.write_str("trailing bytes after the last field"),
            Self::NonPrintable => f.write_str("non-printable byte in an alphanumeric field"),
            Self::NonBlankTail => f.write_str("record bytes beyond the line body are not spaces"),
        }
    }
}

impl std::error::Error for ErrorKind {}

/// A codec failure in context: which field failed, where it sits and how wide it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Error {
    /// Copybook field name, such as `"ACCT-CURR-BAL"`, or `"record"` for a whole-record or
    /// framing failure.
    pub field: &'static str,
    /// Byte offset of the field within its record. `framing::decode_all` rebases it to the
    /// offset within the whole input.
    pub offset: usize,
    /// Field width in bytes; the record width for a whole-record failure.
    pub width: usize,
    /// The failure class.
    pub kind: ErrorKind,
}

impl Error {
    /// Builds an error for `field` at `offset`, `width` bytes wide.
    #[must_use]
    pub const fn new(field: &'static str, offset: usize, width: usize, kind: ErrorKind) -> Self {
        Self {
            field,
            offset,
            width,
            kind,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            field,
            offset,
            width,
            kind,
        } = self;
        write!(f, "{field} (offset {offset}, width {width}): {kind}")
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use super::{Error, ErrorKind};
    use std::collections::HashSet;

    const EXPECTED: usize = 50;
    const ACTUAL: usize = 36;

    /// Every variant once, the length classes with the `cardxref.txt` lengths (E04).
    const ALL: [ErrorKind; 17] = [
        ErrorKind::WidthMismatch {
            expected: EXPECTED,
            actual: ACTUAL,
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
            expected: EXPECTED,
            actual: ACTUAL,
        },
        ErrorKind::InvalidDate,
        ErrorKind::UnknownTimestampLayout,
        ErrorKind::InvalidTimestamp,
        ErrorKind::UnknownFormCode,
        ErrorKind::TrailingBytes,
        ErrorKind::NonPrintable,
        ErrorKind::NonBlankTail,
    ];

    /// The lengths a variant carries. The exhaustive `match` stops this module compiling when a
    /// variant is added without being classified here; add it to `ALL` at the same time.
    fn lengths(kind: ErrorKind) -> Option<(usize, usize)> {
        match kind {
            ErrorKind::WidthMismatch { expected, actual }
            | ErrorKind::ShortBodyNotFiller { expected, actual } => Some((expected, actual)),
            ErrorKind::NonDigit
            | ErrorKind::InvalidOverpunch
            | ErrorKind::PlainDigitSign
            | ErrorKind::NegativeZero
            | ErrorKind::InvalidPackedDigit
            | ErrorKind::InvalidPackedSign
            | ErrorKind::NonZeroPadNibble
            | ErrorKind::OutOfRange
            | ErrorKind::InvalidDate
            | ErrorKind::UnknownTimestampLayout
            | ErrorKind::InvalidTimestamp
            | ErrorKind::UnknownFormCode
            | ErrorKind::TrailingBytes
            | ErrorKind::NonPrintable
            | ErrorKind::NonBlankTail => None,
        }
    }

    /// Length of the longest run of ASCII digits in `text`.
    fn longest_digit_run(text: &str) -> usize {
        text.split(|c: char| !c.is_ascii_digit())
            .map(str::len)
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn kind_display_is_a_fixed_phrase_with_only_the_lengths() {
        for kind in ALL {
            let text = kind.to_string();
            assert!(!text.is_empty(), "{kind:?}");
            if let Some((expected, actual)) = lengths(kind) {
                assert!(
                    text.contains(&format!("expected {expected} bytes")),
                    "{text}"
                );
                assert!(text.contains(&format!("found {actual}")), "{text}");
            }
            let without_lengths = text
                .replace(&EXPECTED.to_string(), "")
                .replace(&ACTUAL.to_string(), "");
            assert!(
                !without_lengths.bytes().any(|b| b.is_ascii_digit()),
                "{kind:?} prints a digit that is not a length: {text}"
            );
        }
    }

    #[test]
    fn kind_display_texts() {
        let cases = [
            (
                ErrorKind::WidthMismatch {
                    expected: 50,
                    actual: 36,
                },
                "width mismatch: expected 50 bytes, found 36",
            ),
            (
                ErrorKind::ShortBodyNotFiller {
                    expected: 50,
                    actual: 35,
                },
                "short line body: expected 50 bytes, found 35, \
                 and the missing tail is not trailing filler",
            ),
            (ErrorKind::NonDigit, "non-digit byte in a numeric field"),
            (ErrorKind::InvalidOverpunch, "invalid overpunch sign byte"),
            (
                ErrorKind::PlainDigitSign,
                "plain digit in the overpunch sign position",
            ),
            (ErrorKind::NegativeZero, "negative zero"),
            (
                ErrorKind::InvalidPackedDigit,
                "invalid packed-decimal digit nibble",
            ),
            (
                ErrorKind::InvalidPackedSign,
                "invalid packed-decimal sign nibble",
            ),
            (
                ErrorKind::NonZeroPadNibble,
                "non-zero packed-decimal pad nibble",
            ),
            (ErrorKind::OutOfRange, "value out of range for the picture"),
            (ErrorKind::InvalidDate, "invalid date text"),
            (
                ErrorKind::UnknownTimestampLayout,
                "unknown timestamp layout",
            ),
            (ErrorKind::InvalidTimestamp, "invalid timestamp text"),
            (ErrorKind::UnknownFormCode, "unknown timestamp layout code"),
            (
                ErrorKind::TrailingBytes,
                "trailing bytes after the last field",
            ),
            (
                ErrorKind::NonPrintable,
                "non-printable byte in an alphanumeric field",
            ),
            (
                ErrorKind::NonBlankTail,
                "record bytes beyond the line body are not spaces",
            ),
        ];
        assert_eq!(cases.len(), ALL.len());
        for (kind, text) in cases {
            assert_eq!(kind.to_string(), text);
        }
    }

    #[test]
    fn every_class_has_its_own_phrase() {
        let phrases: HashSet<String> = ALL.iter().map(ToString::to_string).collect();
        assert_eq!(phrases.len(), ALL.len());
    }

    #[test]
    fn new_keeps_its_fields_and_display_names_them() {
        let error = Error::new("ACCT-CURR-BAL", 12, 12, ErrorKind::InvalidOverpunch);
        assert_eq!(error.field, "ACCT-CURR-BAL");
        assert_eq!(error.offset, 12);
        assert_eq!(error.width, 12);
        assert_eq!(error.kind, ErrorKind::InvalidOverpunch);
        assert_eq!(
            error,
            Error {
                field: "ACCT-CURR-BAL",
                offset: 12,
                width: 12,
                kind: ErrorKind::InvalidOverpunch,
            }
        );
        assert_eq!(
            error.to_string(),
            "ACCT-CURR-BAL (offset 12, width 12): invalid overpunch sign byte"
        );

        let record = Error::new(
            "record",
            0,
            50,
            ErrorKind::ShortBodyNotFiller {
                expected: 50,
                actual: 35,
            },
        );
        assert_eq!(
            record.to_string(),
            "record (offset 0, width 50): short line body: expected 50 bytes, found 35, \
             and the missing tail is not trailing filler"
        );
    }

    /// A field that failed to decode may hold a card number; no rendering of the error can
    /// reproduce it, because neither type has anywhere to keep it (Cardholder Data Redaction).
    #[test]
    fn no_rendering_reproduces_field_content() {
        const PAYLOAD: &str = "9876543210987654";
        for kind in ALL {
            let error = Error::new("XREF-CARD-NUM", 0, PAYLOAD.len(), kind);
            for text in [
                error.to_string(),
                format!("{error:?}"),
                kind.to_string(),
                format!("{kind:?}"),
            ] {
                assert!(!text.contains(PAYLOAD), "{text}");
                assert!(longest_digit_run(&text) <= 2, "{text}");
            }
        }
    }

    #[test]
    fn both_types_are_std_errors_without_a_source() {
        let kind: Box<dyn std::error::Error> = Box::new(ErrorKind::NonDigit);
        assert!(kind.source().is_none());
        assert_eq!(kind.to_string(), "non-digit byte in a numeric field");

        let error: Box<dyn std::error::Error> =
            Box::from(Error::new("CUST-ID", 0, 9, ErrorKind::NonDigit));
        assert!(error.source().is_none());
        assert_eq!(
            error.to_string(),
            "CUST-ID (offset 0, width 9): non-digit byte in a numeric field"
        );
    }

    #[test]
    fn question_mark_converts_a_kind_into_a_boxed_error() {
        fn fails() -> Result<(), Box<dyn std::error::Error>> {
            Err(ErrorKind::TrailingBytes)?;
            Ok(())
        }
        let outcome = fails();
        assert!(matches!(
            outcome.as_ref().map_err(ToString::to_string),
            Err(text) if text == "trailing bytes after the last field"
        ));
    }
}
