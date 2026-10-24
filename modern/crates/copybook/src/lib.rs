#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! COBOL `PIC` clauses as Rust value types, and the lossless conversion between fixture bytes
//! and typed records that the replacement calls codec A (AAP 0.3.3.5, outcome o4).
//!
//! # Scope
//!
//! This crate knows pictures and byte layouts and nothing else. It holds no business rule: the
//! money and rate algebra over the decimals it decodes is `domain::money`, the wrappers that
//! redact card numbers, SSNs, CVVs and passwords are `domain::sensitive`, and the record structs,
//! one per copybook, are `domain::records`, each implementing [`FixedRecord`]. It has no serde
//! support and does no I/O, environment or network access: every function works on the byte
//! slice or value it is given (Local-Only Runtime).
//!
//! # Byte identity
//!
//! A decode succeeds only when encoding its result yields the input bytes again. Padding,
//! leading zeros, overpunch signs, `FILLER` bytes, date and timestamp layouts, short line bodies
//! and line terminators all survive the round trip (PIC-001, PIC-012, PIC-013, PIC-014). Input
//! that could not round-trip is a typed [`Error`] or [`ErrorKind`], never a default or a
//! normalised value (R7): a plain digit in a sign position, a negative zero, a short body ending
//! inside a data field, or a date or timestamp in no known layout (PIC-007, PIC-008, PIC-016,
//! PIC-021).
//!
//! # Totality
//!
//! Every public decoder returns a value or a typed error for any input of any length and never
//! panics (Total Parsers Over Untrusted Input). The attribute on this file's first line denies
//! `unwrap`, `expect`, indexing and slicing in every module of the crate, unit tests included;
//! a crate inheriting the workspace lints cannot add lint entries of its own, so the crate root
//! is where they are tightened. Errors carry the field name, offset and width, never the bytes,
//! which can hold a card number or SSN (PIC-024, Cardholder Data Redaction).
//!
//! # Modules
//!
//! - [`error`]: [`Error`], a failure in field context, over its class [`ErrorKind`].
//! - [`pic`]: `PIC X(n)`, `PIC 9(n)`, `PIC S9(n)`, `PIC S9(4) COMP` counts, `PIC X(10)` dates
//!   and `PIC X(26)` timestamps as value types; integral values are 64-bit (PIC-023).
//! - [`zoned`]: signed DISPLAY such as `PIC S9(10)V99` with a trailing overpunch, packed
//!   `COMP-3` and binary `COMP`.
//! - [`edited`]: numeric-edited output pictures such as `PIC -zzzzzzzzz9.99`; formatting only.
//! - [`record`]: the [`FixedRecord`] contract and its field cursors [`RecordReader`] and
//!   [`RecordWriter`].
//! - [`framing`]: [`lines`] splits an input into [`Line`]s, and [`Framed`] pairs each record
//!   with its [`LineFraming`]: [`Terminator`] and body length.

pub mod edited;
pub mod error;
pub mod framing;
pub mod pic;
pub mod record;
pub mod zoned;

pub use error::{Error, ErrorKind};
pub use framing::{Framed, Line, LineFraming, Terminator, lines};
pub use pic::{CobolTimestamp, Count, PicDate, PicS, PicTimestamp, PicU, PicX, TsForm};
pub use record::{FixedRecord, RecordReader, RecordWriter};

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use crate::{
        CobolTimestamp, Count, Error, ErrorKind, FixedRecord, Framed, Line, LineFraming, PicDate,
        PicS, PicTimestamp, PicU, PicX, RecordReader, RecordWriter, Terminator, TsForm, lines,
    };

    /// `PAIR-CODE PIC X(02)`, the smallest record that names the record re-exports.
    #[derive(Debug, PartialEq)]
    struct Pair(PicX<2>);

    impl FixedRecord for Pair {
        const WIDTH: usize = 2;

        fn decode(bytes: &[u8]) -> Result<Self, Error> {
            let mut r = RecordReader::new(bytes, Self::WIDTH)?;
            let code = r.pic_x("PAIR-CODE")?;
            r.finish()?;
            Ok(Self(code))
        }

        fn encode(&self) -> Vec<u8> {
            let mut w = RecordWriter::with_capacity(Self::WIDTH);
            w.pic_x(&self.0);
            w.finish()
        }
    }

    // Each value is built through the crate root and bound to a type named through its module,
    // and `Pair` implements the root trait while `Framed` requires the module's, so this compiles
    // only while every re-export names its module's item.
    #[test]
    fn root_reexports_name_the_module_items() -> Result<(), Box<dyn StdError>> {
        let kind: crate::error::ErrorKind = ErrorKind::WidthMismatch {
            expected: 2,
            actual: 3,
        };
        let rejected: Result<Pair, crate::error::Error> = Pair::decode(b"ABC");
        assert_eq!(rejected, Err(Error::new("record", 0, 2, kind)));

        let text: crate::pic::PicX<2> = PicX::<2>::spaces();
        assert_eq!(text.as_bytes(), b"  ");
        let key: crate::pic::PicU<3> = PicU::<3>::ZERO;
        assert_eq!(key.get(), 0);
        let signed: crate::pic::PicS<5> = PicS::<5>::default();
        assert_eq!(signed.get(), 0);
        let count: crate::pic::Count = Count::ZERO;
        assert_eq!(count.get(), 0);
        let date: crate::pic::PicDate = PicDate::BLANK;
        assert_eq!(date.value(), None);
        let stamp: crate::pic::PicTimestamp = PicTimestamp::decode(b"2022-06-10 19:27:53.000000")?;
        let value: Option<crate::pic::CobolTimestamp> = stamp.value();
        let form: Option<crate::pic::TsForm> = value.as_ref().map(CobolTimestamp::form);
        assert_eq!(form, Some(TsForm::Iso));

        let mut split: crate::framing::Lines<'_> = lines(b"AB\r\n");
        let line: Option<crate::framing::Line<'_>> = split.next();
        assert_eq!(line, Some(Line::new(b"AB", Terminator::CrLf)));
        let framed: crate::framing::Framed<Pair> =
            Framed::decode(Line::new(b"AB", Terminator::Lf))?;
        let framing: crate::framing::LineFraming = framed.framing;
        assert_eq!(
            framing,
            LineFraming {
                terminator: crate::framing::Terminator::Lf,
                body_len: 2,
            }
        );
        assert_eq!(framed.encode(), b"AB\n");
        Ok(())
    }
}
