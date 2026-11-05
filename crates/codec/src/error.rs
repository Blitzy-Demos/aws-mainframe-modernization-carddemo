//! The single error type of the record codecs.
//!
//! Every fallible decode, encode and framing function in this crate returns
//! [`CodecError`]: the failure itself ([`CodecErrorKind`]) plus where it was
//! found, from the layout down to the byte offset, so a report such as
//! `CVACT01Y record 49 field ACCT-ADDR-ZIP at offset 102: …` points straight
//! at the fixture bytes. Bytes are shown in hex because the offending byte is
//! often a control character or an EBCDIC-only value. The encoders that only
//! lay out bytes cannot fail: [`Layout::encode`](crate::Layout::encode),
//! [`encode_ascii_lf`](crate::framing::encode_ascii_lf) and
//! [`encode_ebcdic_file`](crate::framing::encode_ebcdic_file) return their
//! bytes directly.

use std::fmt;

/// What went wrong while decoding or encoding a record or a file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecErrorKind {
    /// A record or field buffer does not have the length its layout requires,
    /// for example a buffer handed to `Layout::decode` that is not exactly
    /// `LEN` bytes.
    #[error("length {actual}, expected {expected}")]
    Length {
        /// The length the layout or field specification requires.
        expected: usize,
        /// The length actually supplied.
        actual: usize,
    },
    /// A byte other than `0`–`9` in an unsigned `9(n)` field, or in any byte
    /// of a signed field before its final sign position.
    #[error("non-digit byte 0x{byte:02X} in a numeric field")]
    NotNumeric {
        /// The offending byte, in the internal (ASCII) character set.
        byte: u8,
    },
    /// The final byte of a signed `S9(n)V99` field is a plain digit instead of
    /// a trailing overpunch glyph. Every signed fixture field uses overpunch,
    /// so this is an escalation case and the value is never repaired.
    #[error("plain digit byte 0x{byte:02X} in the sign position of a signed field")]
    PlainDigitInSigned {
        /// The offending byte, in the internal (ASCII) character set.
        byte: u8,
    },
    /// The final byte of a signed field is neither an overpunch glyph nor a
    /// digit.
    #[error("invalid sign byte 0x{byte:02X} in a signed field")]
    InvalidSign {
        /// The offending byte, in the internal (ASCII) character set.
        byte: u8,
    },
    /// A domain constructor rejected the decoded value; carries the text of
    /// the domain `NumericError`.
    #[error("value rejected: {0}")]
    Value(String),
    /// An ASCII line body is longer than the copybook record length.
    #[error("stored length {stored_len} exceeds the record length")]
    RecordTooLong {
        /// The length of the stored line body, without its terminator.
        stored_len: usize,
    },
    /// Re-applying a short ASCII frame (a stored length below `LEN`, as in
    /// `cardxref.txt`) would drop bytes that are not spaces.
    #[error("re-applying stored length {stored_len} would drop non-space bytes")]
    TruncationLoss {
        /// The stored length of the frame being re-applied.
        stored_len: usize,
    },
    /// A record file of full-width records, each followed by `LF`, ends in a
    /// partial frame: fewer bytes remain than the record length plus its `LF`.
    #[error("final frame of {frame_len} bytes is shorter than the record length plus LF")]
    PartialFrame {
        /// The number of bytes left for the final record and its `LF`.
        frame_len: usize,
    },
    /// The byte after a full-width record of a record file is not the `LF`
    /// that must separate it from the next record.
    #[error("byte 0x{byte:02X} instead of LF after a full-width record")]
    Separator {
        /// The byte found where the `LF` belongs.
        byte: u8,
    },
    /// An EBCDIC file length is not a whole multiple of the layout `LEN`.
    #[error("file length {file_len} is not a multiple of the record length")]
    Blocking {
        /// The length of the whole file in bytes.
        file_len: usize,
    },
}

/// A codec failure and its location: layout, record, field and offset.
///
/// The `Display` text already contains the [`CodecErrorKind`] text, so the
/// kind is deliberately not also exposed through `Error::source`: a reporter
/// that walks the source chain would otherwise print it twice.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{layout}{location}: {kind}", location = Location::new(*.record, *.field, *.offset))]
pub struct CodecError {
    /// The layout's `Layout::NAME` (the copybook name, such as `CVACT01Y`), or
    /// a caller-chosen label for an ad-hoc layout such as the `mnttrdb2` input
    /// reader.
    pub layout: &'static str,
    /// The 1-based record number in file order, when the error arose inside a
    /// file.
    pub record: Option<usize>,
    /// The copybook field name, when the error arose inside a field.
    pub field: Option<&'static str>,
    /// The byte offset of that field within the record.
    pub offset: Option<usize>,
    /// What went wrong.
    pub kind: CodecErrorKind,
}

impl CodecError {
    /// An error in `layout` with no record, field or offset yet.
    #[must_use]
    pub const fn new(layout: &'static str, kind: CodecErrorKind) -> Self {
        Self {
            layout,
            record: None,
            field: None,
            offset: None,
            kind,
        }
    }

    /// Locates the error in the field `name` starting at byte `offset`.
    #[must_use]
    pub fn at_field(mut self, name: &'static str, offset: usize) -> Self {
        self.field = Some(name);
        self.offset = Some(offset);
        self
    }

    /// Locates the error in record `n`, counted from 1 in file order.
    #[must_use]
    pub fn at_record(mut self, n: usize) -> Self {
        self.record = Some(n);
        self
    }
}

/// The location part of the `CodecError` text: each part present is written
/// with a leading space, and an absent part is left out.
struct Location {
    record: Option<usize>,
    field: Option<&'static str>,
    offset: Option<usize>,
}

impl Location {
    const fn new(
        record: Option<usize>,
        field: Option<&'static str>,
        offset: Option<usize>,
    ) -> Self {
        Self {
            record,
            field,
            offset,
        }
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(record) = self.record {
            write!(f, " record {record}")?;
        }
        if let Some(field) = self.field {
            write!(f, " field {field}")?;
        }
        if let Some(offset) = self.offset {
            write!(f, " at offset {offset}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{CodecError, CodecErrorKind};
    use std::error::Error;

    fn plain_digit() -> CodecErrorKind {
        CodecErrorKind::PlainDigitInSigned { byte: b'5' }
    }

    #[test]
    fn new_sets_no_location() {
        let err = CodecError::new("CVTRA06Y", plain_digit());
        assert_eq!(err.layout, "CVTRA06Y");
        assert_eq!(err.record, None);
        assert_eq!(err.field, None);
        assert_eq!(err.offset, None);
        assert_eq!(err.kind, plain_digit());
    }

    #[test]
    fn builders_set_only_their_own_fields() {
        let field_only = CodecError::new("CVTRA06Y", plain_digit()).at_field("DALYTRAN-AMT", 132);
        assert_eq!(field_only.field, Some("DALYTRAN-AMT"));
        assert_eq!(field_only.offset, Some(132));
        assert_eq!(field_only.record, None);

        let record_only = CodecError::new("CVTRA06Y", plain_digit()).at_record(3);
        assert_eq!(record_only.record, Some(3));
        assert_eq!(record_only.field, None);
        assert_eq!(record_only.offset, None);

        let both = field_only.at_record(3);
        assert_eq!(
            both,
            CodecError {
                layout: "CVTRA06Y",
                record: Some(3),
                field: Some("DALYTRAN-AMT"),
                offset: Some(132),
                kind: plain_digit(),
            }
        );
    }

    #[test]
    fn display_names_every_present_part_and_omits_absent_ones() {
        let base = CodecError::new("CVTRA06Y", plain_digit());
        let kind_text = "plain digit byte 0x35 in the sign position of a signed field";
        let cases = [
            (
                base.clone().at_field("DALYTRAN-AMT", 132).at_record(3),
                format!("CVTRA06Y record 3 field DALYTRAN-AMT at offset 132: {kind_text}"),
            ),
            (
                base.clone().at_field("DALYTRAN-AMT", 132),
                format!("CVTRA06Y field DALYTRAN-AMT at offset 132: {kind_text}"),
            ),
            (
                base.clone().at_record(3),
                format!("CVTRA06Y record 3: {kind_text}"),
            ),
            (base.clone(), format!("CVTRA06Y: {kind_text}")),
            (
                CodecError {
                    offset: Some(132),
                    ..base.clone()
                },
                format!("CVTRA06Y at offset 132: {kind_text}"),
            ),
            (
                CodecError {
                    field: Some("DALYTRAN-AMT"),
                    ..base
                },
                format!("CVTRA06Y field DALYTRAN-AMT: {kind_text}"),
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn kind_display_texts() {
        let cases = [
            (
                CodecErrorKind::Length {
                    expected: 300,
                    actual: 299,
                },
                "length 299, expected 300",
            ),
            (
                CodecErrorKind::NotNumeric { byte: b'A' },
                "non-digit byte 0x41 in a numeric field",
            ),
            (
                CodecErrorKind::PlainDigitInSigned { byte: b'0' },
                "plain digit byte 0x30 in the sign position of a signed field",
            ),
            (
                CodecErrorKind::InvalidSign { byte: b'\n' },
                "invalid sign byte 0x0A in a signed field",
            ),
            (
                CodecErrorKind::Value("account id out of range".to_owned()),
                "value rejected: account id out of range",
            ),
            (
                CodecErrorKind::RecordTooLong { stored_len: 51 },
                "stored length 51 exceeds the record length",
            ),
            (
                CodecErrorKind::TruncationLoss { stored_len: 36 },
                "re-applying stored length 36 would drop non-space bytes",
            ),
            (
                CodecErrorKind::PartialFrame { frame_len: 350 },
                "final frame of 350 bytes is shorter than the record length plus LF",
            ),
            (
                CodecErrorKind::Separator { byte: b'\r' },
                "byte 0x0D instead of LF after a full-width record",
            ),
            (
                CodecErrorKind::Blocking { file_len: 15_001 },
                "file length 15001 is not a multiple of the record length",
            ),
        ];
        for (kind, expected) in cases {
            assert_eq!(kind.to_string(), expected);
        }
    }

    #[test]
    fn clone_and_equality_compare_every_part() {
        let err = CodecError::new("CVACT01Y", CodecErrorKind::Value("bad".to_owned()))
            .at_field("ACCT-ADDR-ZIP", 102)
            .at_record(49);
        let copy = err.clone();
        assert_eq!(copy, err);
        assert_ne!(copy.clone().at_record(50), err);
        assert_ne!(copy.clone().at_field("ACCT-ADDR-ZIP", 103), err);
        assert_ne!(
            CodecError {
                layout: "CVACT02Y",
                ..copy.clone()
            },
            err
        );
        assert_ne!(
            CodecError {
                kind: CodecErrorKind::Value("worse".to_owned()),
                ..copy
            },
            err
        );
    }

    #[test]
    fn kind_is_not_repeated_as_source() {
        let err = CodecError::new("CVACT03Y", CodecErrorKind::Blocking { file_len: 2_501 });
        assert!(err.source().is_none());
    }
}
