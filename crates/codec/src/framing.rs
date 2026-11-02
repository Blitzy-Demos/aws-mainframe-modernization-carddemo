//! Record framing of whole files: ASCII line framing and EBCDIC fixed-length
//! blocking.
//!
//! # ASCII
//!
//! An ASCII file is a sequence of lines. [`split_ascii`] splits it on `LF`, and
//! each record keeps its [`AsciiFrame`]: the length of its stored body and the
//! [`Terminator`] that followed it (`LF`, `CR LF`, or none for a final line
//! without a line end). [`decode_ascii_file`] pads each body with spaces to the
//! layout length before decoding; [`encode_ascii_file`] writes back exactly the
//! stored length and terminator, so a decoded file re-encodes byte for byte.
//!
//! The fixture corpus needs this (escalation E-04): `trancatg.txt` ends every
//! record with `CR LF`, `tcatbal.txt` and `trantype.txt` end all records but
//! the last with `CR LF`, and `cardxref.txt` stores each 50-byte record as its
//! first 36 bytes, the absent FILLER decoding as 14 spaces. Frames are
//! re-applied as stored and never normalised: no `CR LF` becomes `LF` and no
//! short record is padded on output.
//!
//! Record files written by batch use [`encode_ascii_lf`]: every record at full
//! width, each followed by `LF`. [`decode_ascii_lf`] reads them back frame by
//! frame, taking each record's [`Layout::LEN`] bytes whole and only then
//! requiring the `LF` after them. Alphanumeric fields copy bytes, so a record
//! may itself hold `LF` or `CR`; [`split_ascii`] would cut it at that `LF`, or
//! take a final `CR` for a `CR LF` line end, so it reads the fixture corpus
//! only.
//!
//! # EBCDIC
//!
//! An EBCDIC file (`app/data/EBCDIC/AWS.M2.CARDDEMO.*.PS`) is a sequence of
//! fixed-length blocks of the layout length with no terminators, for example
//! `CARDXREF` 2500 bytes = 50 records of 50 bytes. [`decode_ebcdic_file`]
//! rejects a file whose length is not a whole multiple of the record length.

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};

/// The line end that followed a stored ASCII record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Terminator {
    /// `LF` (`0x0A`).
    Lf,
    /// `CR LF` (`0x0D 0x0A`).
    CrLf,
    /// No line end: the final record of a file whose last byte is not `LF`.
    None,
}

impl Terminator {
    /// The bytes of this line end: `b"\n"`, `b"\r\n"` or `b""`.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Self::Lf => b"\n",
            Self::CrLf => b"\r\n",
            Self::None => b"",
        }
    }
}

/// How one ASCII record was stored: its body length and its line end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AsciiFrame {
    /// The length of the stored body, without its terminator. It may be below
    /// the layout length, as in `cardxref.txt` (36 of 50 bytes).
    pub stored_len: usize,
    /// The line end that followed the body.
    pub terminator: Terminator,
}

impl AsciiFrame {
    /// The frame of a record stored as `len` bytes followed by `LF`.
    #[must_use]
    pub const fn lf(len: usize) -> Self {
        Self {
            stored_len: len,
            terminator: Terminator::Lf,
        }
    }
}

/// A decoded ASCII record together with the frame it was stored in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Framed<R> {
    /// The stored length and line end, re-applied on encoding.
    pub frame: AsciiFrame,
    /// The decoded record.
    pub record: R,
}

/// Splits an ASCII file into its records, each with its frame and stored body.
///
/// - Records end at `LF`. A `CR` immediately before that `LF` makes the
///   terminator [`Terminator::CrLf`] and is not part of the body; any other
///   `CR` is record data.
/// - A non-empty final segment without `LF` is a record with
///   [`Terminator::None`]; a trailing `CR` there is record data.
/// - Nothing after the last `LF` means no further record, so empty input has
///   no records, while an empty line between two `LF`s is a record whose
///   `stored_len` is 0.
#[must_use]
pub fn split_ascii(bytes: &[u8]) -> Vec<(AsciiFrame, &[u8])> {
    bytes
        .split_inclusive(|&b| b == b'\n')
        .map(|segment| {
            let (body, terminator) = match segment.strip_suffix(b"\n") {
                Some(line) => match line.strip_suffix(b"\r") {
                    Some(body) => (body, Terminator::CrLf),
                    None => (line, Terminator::Lf),
                },
                None => (segment, Terminator::None),
            };
            let frame = AsciiFrame {
                stored_len: body.len(),
                terminator,
            };
            (frame, body)
        })
        .collect()
}

/// Pads a stored record body with spaces to `len` bytes, the record length of
/// the layout `layout`.
///
/// # Errors
///
/// [`CodecErrorKind::RecordTooLong`] carrying `body.len()` when the body is
/// longer than `len`.
pub fn pad(layout: &'static str, body: &[u8], len: usize) -> Result<Vec<u8>, CodecError> {
    if body.len() > len {
        return Err(CodecError::new(
            layout,
            CodecErrorKind::RecordTooLong {
                stored_len: body.len(),
            },
        ));
    }
    let mut padded = Vec::with_capacity(len);
    padded.extend_from_slice(body);
    padded.resize(len, b' ');
    Ok(padded)
}

/// Decodes every record of the ASCII file `bytes` with the layout `L`, keeping
/// each record's frame.
///
/// # Errors
///
/// The first failing record's error, located at its 1-based record number:
/// [`CodecErrorKind::RecordTooLong`] when its body exceeds [`Layout::LEN`], or
/// any error of [`Layout::decode`].
pub fn decode_ascii_file<L: Layout>(bytes: &[u8]) -> Result<Vec<Framed<L::Record>>, CodecError> {
    split_ascii(bytes)
        .into_iter()
        .zip(1..)
        .map(|((frame, body), n)| {
            let padded = pad(L::NAME, body, L::LEN).map_err(|e| e.at_record(n))?;
            let record = L::decode(&padded, Charset::Ascii).map_err(|e| e.at_record(n))?;
            Ok(Framed { frame, record })
        })
        .collect()
}

/// Encodes framed records as an ASCII file: for each record, the first
/// `stored_len` bytes of its encoding, then its terminator.
///
/// # Errors
///
/// The first failing record's error, located at its 1-based record number:
///
/// - [`CodecErrorKind::RecordTooLong`] when `stored_len` exceeds
///   [`Layout::LEN`].
/// - [`CodecErrorKind::TruncationLoss`] when a byte the frame leaves out is
///   not a space: the record was changed beyond its stored length, so
///   re-applying the frame would lose data.
/// - [`CodecErrorKind::Length`] when [`Layout::encode`] breaks its contract and
///   returns fewer than `stored_len` bytes.
pub fn encode_ascii_file<L: Layout>(records: &[Framed<L::Record>]) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::with_capacity(records.len().saturating_mul(L::LEN.saturating_add(2)));
    for (framed, n) in records.iter().zip(1..) {
        let encoded = L::encode(&framed.record, Charset::Ascii);
        let stored =
            stored_part::<L>(&encoded, framed.frame.stored_len).map_err(|e| e.at_record(n))?;
        out.extend_from_slice(stored);
        out.extend_from_slice(framed.frame.terminator.as_bytes());
    }
    Ok(out)
}

/// The first `stored_len` bytes of the encoded record `encoded`, provided the
/// bytes after them are all spaces, the padding [`decode_ascii_file`] added.
fn stored_part<L: Layout>(encoded: &[u8], stored_len: usize) -> Result<&[u8], CodecError> {
    if stored_len > L::LEN {
        return Err(CodecError::new(
            L::NAME,
            CodecErrorKind::RecordTooLong { stored_len },
        ));
    }
    let Some((stored, dropped)) = encoded.split_at_checked(stored_len) else {
        return Err(CodecError::new(
            L::NAME,
            CodecErrorKind::Length {
                expected: L::LEN,
                actual: encoded.len(),
            },
        ));
    };
    if dropped.iter().any(|&b| b != b' ') {
        return Err(CodecError::new(
            L::NAME,
            CodecErrorKind::TruncationLoss { stored_len },
        ));
    }
    Ok(stored)
}

/// Encodes records as an ASCII file of full-width records, each followed by
/// `LF`: the format of every record file batch writes, which
/// [`decode_ascii_lf`] reads back. Each record is written as [`Layout::encode`]
/// returns it, so an `LF` or `CR` inside an alphanumeric field is record data,
/// not a line end.
#[must_use]
pub fn encode_ascii_lf<L: Layout>(records: &[L::Record]) -> Vec<u8> {
    let terminator = Terminator::Lf.as_bytes();
    let mut out = Vec::with_capacity(
        records
            .len()
            .saturating_mul(L::LEN.saturating_add(terminator.len())),
    );
    for record in records {
        out.extend_from_slice(&L::encode(record, Charset::Ascii));
        out.extend_from_slice(terminator);
    }
    out
}

/// Decodes a record file of full-width records, each followed by `LF`, with
/// the layout `L`: the inverse of [`encode_ascii_lf`].
///
/// The file is a sequence of frames, each exactly [`Layout::LEN`] record bytes
/// and then one `LF`. The record bytes are taken whole and decoded as they
/// are, `LF` and `CR` included: nothing is padded and no `CR LF` line end is
/// recognised. Empty input has no records.
///
/// # Errors
///
/// The first failing record's error, located at its 1-based record number:
///
/// - [`CodecErrorKind::PartialFrame`] carrying the number of bytes left when
///   fewer than `LEN + 1` remain for the final record and its `LF`.
/// - [`CodecErrorKind::Separator`] carrying the byte after the record bytes
///   when it is not `LF`.
/// - Any error of [`Layout::decode`].
pub fn decode_ascii_lf<L: Layout>(bytes: &[u8]) -> Result<Vec<L::Record>, CodecError> {
    let mut records = Vec::with_capacity(bytes.len() / L::LEN.saturating_add(1));
    let mut rest = bytes;
    for n in 1.. {
        if rest.is_empty() {
            break;
        }
        let error = |kind| CodecError::new(L::NAME, kind).at_record(n);
        let partial = || {
            error(CodecErrorKind::PartialFrame {
                frame_len: rest.len(),
            })
        };
        let (body, after) = rest.split_at_checked(L::LEN).ok_or_else(partial)?;
        let (&separator, next) = after.split_first().ok_or_else(partial)?;
        if separator != b'\n' {
            return Err(error(CodecErrorKind::Separator { byte: separator }));
        }
        records.push(L::decode(body, Charset::Ascii).map_err(|e| e.at_record(n))?);
        rest = next;
    }
    Ok(records)
}

/// Decodes every fixed-length block of the EBCDIC file `bytes` with the layout
/// `L`.
///
/// # Errors
///
/// - [`CodecErrorKind::Blocking`] carrying the file length when it is not a
///   whole multiple of [`Layout::LEN`], or when `LEN` is 0.
/// - The first failing record's [`Layout::decode`] error, located at its
///   1-based record number.
pub fn decode_ebcdic_file<L: Layout>(bytes: &[u8]) -> Result<Vec<L::Record>, CodecError> {
    if L::LEN == 0 || !bytes.len().is_multiple_of(L::LEN) {
        return Err(CodecError::new(
            L::NAME,
            CodecErrorKind::Blocking {
                file_len: bytes.len(),
            },
        ));
    }
    bytes
        .chunks_exact(L::LEN)
        .zip(1..)
        .map(|(block, n)| L::decode(block, Charset::Ebcdic).map_err(|e| e.at_record(n)))
        .collect()
}

/// Encodes records as an EBCDIC file: the concatenation of their encodings,
/// with no terminators.
#[must_use]
pub fn encode_ebcdic_file<L: Layout>(records: &[L::Record]) -> Vec<u8> {
    let mut out = Vec::with_capacity(records.len().saturating_mul(L::LEN));
    for record in records {
        out.extend_from_slice(&L::encode(record, Charset::Ebcdic));
    }
    out
}

#[cfg(test)]
mod tests {
    use carddemo_domain::numeric::Alnum;

    use super::{
        AsciiFrame, Framed, Terminator, decode_ascii_file, decode_ascii_lf, decode_ebcdic_file,
        encode_ascii_file, encode_ascii_lf, encode_ebcdic_file, pad, split_ascii,
    };
    use crate::Layout;
    use crate::charset::Charset;
    use crate::error::{CodecError, CodecErrorKind};
    use crate::field::{self, FieldKind, FieldSpec};

    const LEFT: FieldSpec = FieldSpec::new("LEFT", 0, 2, FieldKind::Alnum);
    const RIGHT: FieldSpec = FieldSpec::new("RIGHT", 2, 2, FieldKind::Alnum);
    const COUNT: FieldSpec = FieldSpec::new("COUNT", 0, 2, FieldKind::Unsigned);

    /// A 4-byte record of two `X(2)` fields.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct PairRecord {
        left: Alnum<2>,
        right: Alnum<2>,
    }

    /// The layout of [`PairRecord`], standing in for a copybook layout.
    struct Pair;

    impl Layout for Pair {
        const LEN: usize = 4;
        const NAME: &'static str = "TEST";
        const FIELDS: &'static [FieldSpec] = &[LEFT, RIGHT];
        type Record = PairRecord;

        fn decode(bytes: &[u8], charset: Charset) -> Result<PairRecord, CodecError> {
            check_len::<Self>(bytes)?;
            let rec = charset.to_internal(bytes);
            Ok(PairRecord {
                left: field::alnum(Self::NAME, &rec, &LEFT)?,
                right: field::alnum(Self::NAME, &rec, &RIGHT)?,
            })
        }

        fn encode(record: &PairRecord, charset: Charset) -> Vec<u8> {
            let mut rec = vec![b' '; Self::LEN];
            field::put_alnum(&mut rec, &LEFT, record.left.as_bytes());
            field::put_alnum(&mut rec, &RIGHT, record.right.as_bytes());
            charset.from_internal(&rec)
        }
    }

    /// A 2-byte record of one `9(2)` field, whose decoder can fail.
    struct Count;

    impl Layout for Count {
        const LEN: usize = 2;
        const NAME: &'static str = "COUNT";
        const FIELDS: &'static [FieldSpec] = &[COUNT];
        type Record = u64;

        fn decode(bytes: &[u8], charset: Charset) -> Result<u64, CodecError> {
            check_len::<Self>(bytes)?;
            field::unsigned(Self::NAME, &charset.to_internal(bytes), &COUNT)
        }

        fn encode(record: &u64, charset: Charset) -> Vec<u8> {
            let mut rec = vec![b'0'; Self::LEN];
            field::put_unsigned(&mut rec, &COUNT, *record);
            charset.from_internal(&rec)
        }
    }

    /// A degenerate layout of length 0.
    struct Zero;

    impl Layout for Zero {
        const LEN: usize = 0;
        const NAME: &'static str = "ZERO";
        const FIELDS: &'static [FieldSpec] = &[];
        type Record = ();

        fn decode(bytes: &[u8], _charset: Charset) -> Result<(), CodecError> {
            check_len::<Self>(bytes)
        }

        fn encode(_record: &(), _charset: Charset) -> Vec<u8> {
            Vec::new()
        }
    }

    fn check_len<L: Layout>(bytes: &[u8]) -> Result<(), CodecError> {
        if bytes.len() == L::LEN {
            Ok(())
        } else {
            Err(CodecError::new(
                L::NAME,
                CodecErrorKind::Length {
                    expected: L::LEN,
                    actual: bytes.len(),
                },
            ))
        }
    }

    fn pair(left: [u8; 2], right: [u8; 2]) -> PairRecord {
        PairRecord {
            left: Alnum::from_array(left),
            right: Alnum::from_array(right),
        }
    }

    fn frame(stored_len: usize, terminator: Terminator) -> AsciiFrame {
        AsciiFrame {
            stored_len,
            terminator,
        }
    }

    /// The records `split_ascii` should return, each as (stored length,
    /// terminator, body).
    type Expected = &'static [(usize, Terminator, &'static str)];

    fn at_record(layout: &'static str, kind: CodecErrorKind, n: usize) -> CodecError {
        CodecError::new(layout, kind).at_record(n)
    }

    #[test]
    fn terminators_and_lf_frames() {
        assert_eq!(Terminator::Lf.as_bytes(), b"\n");
        assert_eq!(Terminator::CrLf.as_bytes(), b"\r\n");
        assert_eq!(Terminator::None.as_bytes(), b"");
        assert_eq!(AsciiFrame::lf(36), frame(36, Terminator::Lf));
    }

    #[test]
    fn split_ascii_keeps_each_body_and_terminator() {
        use Terminator::{CrLf, Lf};
        let cases: [(&str, Expected); 7] = [
            (
                "ab\ncd\r\nef",
                &[(2, Lf, "ab"), (2, CrLf, "cd"), (2, Terminator::None, "ef")],
            ),
            ("a\rb\n", &[(3, Lf, "a\rb")]),
            ("", &[]),
            ("x\n\n", &[(1, Lf, "x"), (0, Lf, "")]),
            ("\r\n", &[(0, CrLf, "")]),
            ("ab\r", &[(3, Terminator::None, "ab\r")]),
            ("a\r\r\n", &[(2, CrLf, "a\r")]),
        ];
        for (input, expected) in cases {
            let expected: Vec<(AsciiFrame, &[u8])> = expected
                .iter()
                .map(|&(len, terminator, body)| (frame(len, terminator), body.as_bytes()))
                .collect();
            assert_eq!(split_ascii(input.as_bytes()), expected, "input {input:?}");
        }
    }

    #[test]
    fn pad_fills_with_spaces_up_to_the_record_length() {
        assert_eq!(pad("TEST", b"ab", 4), Ok(b"ab  ".to_vec()));
        assert_eq!(pad("TEST", b"abcd", 4), Ok(b"abcd".to_vec()));
        assert_eq!(pad("TEST", b"", 0), Ok(Vec::new()));
        assert_eq!(
            pad("TEST", b"abcde", 4),
            Err(CodecError::new(
                "TEST",
                CodecErrorKind::RecordTooLong { stored_len: 5 }
            ))
        );
    }

    #[test]
    fn ascii_files_round_trip_byte_for_byte_in_every_frame() {
        let file: &[u8] = b"abcd\r\nef\ngh  \r\n\nijkl";
        let decoded = decode_ascii_file::<Pair>(file).expect("decodes");
        let expected = vec![
            Framed {
                frame: frame(4, Terminator::CrLf),
                record: pair(*b"ab", *b"cd"),
            },
            Framed {
                frame: AsciiFrame::lf(2),
                record: pair(*b"ef", *b"  "),
            },
            Framed {
                frame: frame(4, Terminator::CrLf),
                record: pair(*b"gh", *b"  "),
            },
            Framed {
                frame: AsciiFrame::lf(0),
                record: pair(*b"  ", *b"  "),
            },
            Framed {
                frame: frame(4, Terminator::None),
                record: pair(*b"ij", *b"kl"),
            },
        ];
        assert_eq!(decoded, expected);
        assert_eq!(encode_ascii_file::<Pair>(&decoded), Ok(file.to_vec()));
    }

    #[test]
    fn empty_files_have_no_records() {
        assert_eq!(decode_ascii_file::<Pair>(b""), Ok(Vec::new()));
        assert_eq!(encode_ascii_file::<Pair>(&[]), Ok(Vec::new()));
        assert_eq!(decode_ebcdic_file::<Pair>(b""), Ok(Vec::new()));
        assert!(encode_ebcdic_file::<Pair>(&[]).is_empty());
        assert!(encode_ascii_lf::<Pair>(&[]).is_empty());
        assert_eq!(decode_ascii_lf::<Pair>(b""), Ok(Vec::new()));
    }

    #[test]
    fn short_frames_accept_edits_within_their_stored_length() {
        let mut decoded = decode_ascii_file::<Pair>(b"ab\n").expect("decodes");
        decoded.first_mut().expect("one record").record.left = Alnum::from_array(*b"zz");
        assert_eq!(encode_ascii_file::<Pair>(&decoded), Ok(b"zz\n".to_vec()));
    }

    #[test]
    fn decode_errors_carry_their_one_based_record_number() {
        let not_numeric = CodecError::new("COUNT", CodecErrorKind::NotNumeric { byte: b'x' })
            .at_field("COUNT", 0)
            .at_record(3);
        let cases = [
            (
                decode_ascii_file::<Pair>(b"abcd\nabcd\nabcde\nab\n").map(|_| ()),
                at_record("TEST", CodecErrorKind::RecordTooLong { stored_len: 5 }, 3),
            ),
            (
                decode_ascii_file::<Count>(b"12\r\n34\n1x\n").map(|_| ()),
                not_numeric.clone(),
            ),
            (
                decode_ebcdic_file::<Count>(&[0xF1, 0xF2, 0xF3, 0xF4, 0xF1, 0xA7]).map(|_| ()),
                not_numeric,
            ),
        ];
        for (result, expected) in cases {
            assert_eq!(result, Err(expected));
        }
    }

    #[test]
    fn encode_errors_carry_their_one_based_record_number() {
        let full = Framed {
            frame: AsciiFrame::lf(4),
            record: pair(*b"ab", *b"cd"),
        };
        let edited_short = Framed {
            frame: AsciiFrame::lf(2),
            record: pair(*b"ab", *b"zz"),
        };
        let too_long = Framed {
            frame: frame(5, Terminator::CrLf),
            record: pair(*b"ab", *b"cd"),
        };
        let cases = [
            (
                vec![full.clone(), edited_short],
                at_record("TEST", CodecErrorKind::TruncationLoss { stored_len: 2 }, 2),
            ),
            (
                vec![full.clone(), full, too_long],
                at_record("TEST", CodecErrorKind::RecordTooLong { stored_len: 5 }, 3),
            ),
        ];
        for (records, expected) in cases {
            assert_eq!(encode_ascii_file::<Pair>(&records), Err(expected));
        }
    }

    #[test]
    fn encode_ascii_lf_writes_full_width_records_each_followed_by_lf() {
        let records = [pair(*b"ab", *b"cd"), pair(*b"ef", *b"  ")];
        assert_eq!(encode_ascii_lf::<Pair>(&records), b"abcd\nef  \n");
    }

    #[test]
    fn lf_record_files_round_trip_records_holding_lf_and_cr() {
        // Alphanumeric fields copy bytes, so LF, CR, CR LF and a final CR are
        // record data. The last case is stored as `c\r` and then the LF, the
        // bytes of a CR LF line end.
        let cases = [
            vec![pair(*b"a\n", *b"bc")],
            vec![pair(*b"\r\n", *b"\n\n"), pair(*b"ab", *b"cd")],
            vec![pair(*b"ab", *b"cd"), pair(*b"\n\r", *b"\r\r")],
            vec![pair(*b"ab", *b"c\r")],
        ];
        for records in cases {
            let file = encode_ascii_lf::<Pair>(&records);
            assert_eq!(file.len(), records.len() * 5, "{records:?}");
            assert_eq!(decode_ascii_lf::<Pair>(&file), Ok(records));
        }
    }

    #[test]
    fn lf_record_files_of_a_zero_length_layout_are_one_lf_per_record() {
        assert_eq!(encode_ascii_lf::<Zero>(&[(), ()]), b"\n\n");
        assert_eq!(decode_ascii_lf::<Zero>(b"\n\n"), Ok(vec![(), ()]));
        assert_eq!(decode_ascii_lf::<Zero>(b""), Ok(Vec::new()));
    }

    #[test]
    fn lf_record_file_errors_carry_their_one_based_record_number() {
        let partial =
            |frame_len, n| at_record("TEST", CodecErrorKind::PartialFrame { frame_len }, n);
        let separator = |layout, byte, n| at_record(layout, CodecErrorKind::Separator { byte }, n);
        let cases = [
            (decode_ascii_lf::<Pair>(b"abcd").map(|_| ()), partial(4, 1)),
            (
                decode_ascii_lf::<Pair>(b"abcd\nab").map(|_| ()),
                partial(2, 2),
            ),
            (
                decode_ascii_lf::<Pair>(b"abcd\nabcd").map(|_| ()),
                partial(4, 2),
            ),
            (
                decode_ascii_lf::<Pair>(b"abcd\r\n").map(|_| ()),
                separator("TEST", b'\r', 1),
            ),
            (
                decode_ascii_lf::<Pair>(b"abcd\nabcde\n").map(|_| ()),
                separator("TEST", b'e', 2),
            ),
            (
                decode_ascii_lf::<Zero>(b"\n\nx").map(|_| ()),
                separator("ZERO", b'x', 3),
            ),
            (
                decode_ascii_lf::<Count>(b"12\n1x\n").map(|_| ()),
                CodecError::new("COUNT", CodecErrorKind::NotNumeric { byte: b'x' })
                    .at_field("COUNT", 0)
                    .at_record(2),
            ),
        ];
        for (result, expected) in cases {
            assert_eq!(result, Err(expected));
        }
    }

    #[test]
    fn ebcdic_files_are_fixed_length_blocks() {
        // "abcd", "ef  " and "{}09" in CP037.
        let file = [
            0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x40, 0x40, 0xC0, 0xD0, 0xF0, 0xF9,
        ];
        let records = vec![
            pair(*b"ab", *b"cd"),
            pair(*b"ef", *b"  "),
            pair(*b"{}", *b"09"),
        ];
        assert_eq!(decode_ebcdic_file::<Pair>(&file), Ok(records.clone()));
        assert_eq!(encode_ebcdic_file::<Pair>(&records), file);
    }

    #[test]
    fn ebcdic_files_must_be_whole_multiples_of_the_record_length() {
        let cases = [
            (
                decode_ebcdic_file::<Pair>(&[0x40; 7]).map(|_| ()),
                CodecError::new("TEST", CodecErrorKind::Blocking { file_len: 7 }),
            ),
            (
                decode_ebcdic_file::<Zero>(&[0x40; 3]).map(|_| ()),
                CodecError::new("ZERO", CodecErrorKind::Blocking { file_len: 3 }),
            ),
            (
                decode_ebcdic_file::<Zero>(&[]).map(|_| ()),
                CodecError::new("ZERO", CodecErrorKind::Blocking { file_len: 0 }),
            ),
        ];
        for (result, expected) in cases {
            assert_eq!(result, Err(expected));
        }
    }
}
