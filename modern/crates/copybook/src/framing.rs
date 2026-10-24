//! Lossless line framing of fixture files (codec A, AAP 0.3.3.5).
//!
//! The ASCII fixtures are not uniform, so codec A carries each line's framing instead of
//! normalising it (R7):
//!
//! - `tcatbal.txt` ends lines 1–49 with CRLF and line 50 with LF, `trancatg.txt` ends all 18
//!   lines with CRLF, and `trantype.txt` ends lines 1–6 with CRLF and line 7 with LF; `acctdata`,
//!   `carddata`, `cardxref`, `custdata`, `dailytran` and `discgrp` use LF only (E06,
//!   FIXFRM-013). Every file ends with a terminator, so no fixture line has
//!   [`Terminator::None`] (PIC-014, FIXFRM-010).
//! - Every `cardxref.txt` body is 36 bytes against the 50-byte `CVACT03Y` record; the missing 14
//!   bytes are its trailing `FILLER PIC X(14)` (CVACT03Y-004, E04, FIXFRM-011). Every other
//!   fixture body has its record's width: `acctdata` 300, `carddata` 150, `custdata` 500,
//!   `dailytran` 350, `discgrp` 50, `tcatbal` 50, `trancatg` 60 and `trantype` 60 bytes.
//!
//! [`lines`] splits an input into [`Line`]s without losing or adding a byte, [`Framed::decode`]
//! turns a line into its typed record plus its [`LineFraming`], and [`Framed::encode`]
//! reproduces the line exactly, short body and terminator included (PIC-013, PIC-014).
//!
//! [`LineFraming`] describes the file line, never the record: it is never stored, and no table
//! has a column for it. Codec B converts only [`Framed::record`]; every round trip that ends in
//! bytes carries the [`Framed`] envelope unchanged from codec A's decode to codec A's encode.
//!
//! Every function here works on the byte slice it is given and does no I/O (Local-Only Runtime).
//! [`lines`], [`Framed::decode`] and [`decode_all`] take bytes from outside the process and are
//! total: for any input of any length they return a value or a typed [`Error`] and never panic
//! (Total Parsers Over Untrusted Input). An error carries field name, offset, width and lengths,
//! never a byte of the line, which can hold a card number or SSN (Cardholder Data Redaction).
//!
//! ```
//! use copybook::framing::{Framed, Terminator, decode_all, encode_all};
//! use copybook::pic::PicX;
//! use copybook::record::{FixedRecord, RecordReader, RecordWriter};
//!
//! /// `TAG-CODE PIC X(04)` followed by `FILLER PIC X(02)`.
//! #[derive(Debug, PartialEq)]
//! struct Tag {
//!     code: PicX<4>,
//!     filler: PicX<2>,
//! }
//!
//! impl FixedRecord for Tag {
//!     const WIDTH: usize = 6;
//!     const TRAILING_FILLER: usize = 2;
//!
//!     fn decode(bytes: &[u8]) -> Result<Self, copybook::Error> {
//!         let mut r = RecordReader::new(bytes, Self::WIDTH)?;
//!         let record = Self { code: r.pic_x("TAG-CODE")?, filler: r.pic_x("FILLER")? };
//!         r.finish()?;
//!         Ok(record)
//!     }
//!
//!     fn encode(&self) -> Vec<u8> {
//!         let mut w = RecordWriter::with_capacity(Self::WIDTH);
//!         w.pic_x(&self.code).pic_x(&self.filler);
//!         w.finish()
//!     }
//! }
//!
//! // A full CRLF line, then a body that stops where the trailing FILLER begins.
//! let input = b"AB  00\r\nCD  \n";
//! let records: Vec<Framed<Tag>> = decode_all(input)?;
//! assert_eq!(records.len(), 2);
//! assert!(records.iter().any(|framed| framed.framing.terminator == Terminator::CrLf));
//! assert_eq!(records.last().map(|framed| framed.framing.body_len), Some(4));
//! assert_eq!(encode_all(&records), input);
//! # Ok::<(), copybook::Error>(())
//! ```

use std::fmt;
use std::iter::FusedIterator;
use std::mem;

use crate::error::{Error, ErrorKind};
use crate::record::FixedRecord;

/// Field name of a whole-record framing failure, as `record` names its own.
const RECORD: &str = "record";

/// The byte a short body's missing trailing `FILLER` decodes as.
const SPACE: u8 = b' ';

/// The byte that ends a line.
const LF: u8 = b'\n';

/// How a file line ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Terminator {
    /// A single LF.
    Lf,
    /// A CR immediately followed by LF.
    CrLf,
    /// No terminator: the last line of an input that does not end with LF. No fixture line has
    /// it (PIC-014).
    None,
}

impl Terminator {
    /// The terminator's bytes: `\n`, `\r\n`, or none.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8] {
        match self {
            Terminator::Lf => b"\n",
            Terminator::CrLf => b"\r\n",
            Terminator::None => b"",
        }
    }
}

/// The framing of one file line: how it ends and how long its body is.
///
/// It describes the line, never the record: no column stores it, and codec B passes it through
/// unchanged (PIC-014).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineFraming {
    /// How the line ends.
    pub terminator: Terminator,
    /// Length of the line body in bytes, without the terminator. It is below the record's
    /// [`FixedRecord::WIDTH`] only for a body whose missing tail is the record's trailing
    /// `FILLER`, such as the 36-byte `cardxref.txt` bodies (PIC-013, E04).
    pub body_len: usize,
}

/// A decoded record together with the framing of the line it came from (codec A).
///
/// `Debug` shows `R`'s own `Debug`. For a record holding a card number, SSN, CVV or password
/// that is the redacted form of the `domain::sensitive` wrappers, never the raw value (PIC-024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Framed<R> {
    /// The typed record; the only part codec B stores.
    pub record: R,
    /// The framing of the line the record was decoded from, re-emitted by [`Framed::encode`].
    pub framing: LineFraming,
}

/// One line of an input as [`lines`] yields it: its body bytes and its framing.
///
/// The derived `Debug` prints the body bytes, which can hold card numbers and SSNs. It is for
/// tests only: services never log a `Line`, they log decoded record types, whose sensitive
/// fields redact (Cardholder Data Redaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line<'a> {
    /// The line without its terminator.
    pub body: &'a [u8],
    /// The line's terminator, with `body_len` equal to `body.len()`.
    pub framing: LineFraming,
}

impl<'a> Line<'a> {
    /// The line `body` ended by `terminator`, with `framing.body_len` set to `body.len()`.
    #[must_use]
    pub const fn new(body: &'a [u8], terminator: Terminator) -> Self {
        Self {
            body,
            framing: LineFraming {
                terminator,
                body_len: body.len(),
            },
        }
    }
}

/// Splits `input` into lines without losing or adding a byte (PIC-014).
///
/// A line ends at each LF. A CR immediately before that LF belongs to the terminator
/// ([`Terminator::CrLf`]); a CR anywhere else stays in the body, where the `PIC X(n)` decoders
/// reject it as non-printable. A last segment without LF is a line with [`Terminator::None`].
/// Empty input yields no line, and a final LF yields no empty line after it, while `b"\n"` alone
/// is one line with an empty body. Each line's `framing.body_len` equals `body.len()`.
///
/// Concatenating every line's `body` followed by its `framing.terminator.as_bytes()` reproduces
/// `input` byte for byte. The split is total for any input (Total Parsers Over Untrusted Input).
///
/// ```
/// use copybook::framing::{Terminator, lines};
///
/// let ends: Vec<Terminator> = lines(b"01\r\n02\n03").map(|line| line.framing.terminator).collect();
/// assert_eq!(ends, [Terminator::CrLf, Terminator::Lf, Terminator::None]);
/// ```
#[must_use]
pub fn lines(input: &[u8]) -> Lines<'_> {
    Lines { rest: input }
}

/// Iterator over the lines of a byte slice; see [`lines`].
///
/// `Debug` shows the number of bytes not yet split, never the bytes.
#[derive(Clone)]
pub struct Lines<'a> {
    /// The input not yet yielded, always a suffix of the input.
    rest: &'a [u8],
}

impl<'a> Iterator for Lines<'a> {
    type Item = Line<'a>;

    fn next(&mut self) -> Option<Line<'a>> {
        if self.rest.is_empty() {
            return None;
        }
        let line = match self.rest.iter().position(|&byte| byte == LF) {
            Some(lf) => {
                // `lf` is a position inside `rest`, so the split always succeeds.
                let (segment, from_lf) = self.rest.split_at_checked(lf)?;
                self.rest = from_lf.get(1..).unwrap_or_default();
                match segment.strip_suffix(b"\r") {
                    Some(body) => Line::new(body, Terminator::CrLf),
                    None => Line::new(segment, Terminator::Lf),
                }
            }
            None => Line::new(mem::take(&mut self.rest), Terminator::None),
        };
        Some(line)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // Every line consumes at least one byte, a body byte or its LF.
        (usize::from(!self.rest.is_empty()), Some(self.rest.len()))
    }
}

impl FusedIterator for Lines<'_> {}

impl fmt::Debug for Lines<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lines")
            .field("remaining", &self.rest.len())
            .finish_non_exhaustive()
    }
}

/// Checks a line body length against `R`'s record and returns how many trailing bytes are
/// missing: 0 for a full body, at most `R::TRAILING_FILLER` for an accepted short one.
///
/// [`Framed::decode`] and [`Framed::try_encode`] share it, so `try_encode` accepts exactly the
/// lengths `decode` produces.
fn missing_tail<R: FixedRecord>(body_len: usize) -> Result<usize, Error> {
    let kind = match R::WIDTH.checked_sub(body_len) {
        Some(missing) if missing <= R::TRAILING_FILLER => return Ok(missing),
        Some(_) => ErrorKind::ShortBodyNotFiller {
            expected: R::WIDTH,
            actual: body_len,
        },
        None => ErrorKind::WidthMismatch {
            expected: R::WIDTH,
            actual: body_len,
        },
    };
    Err(Error::new(RECORD, 0, R::WIDTH, kind))
}

impl<R: FixedRecord> Framed<R> {
    /// Decodes one line into its record and framing (PIC-013, PIC-014).
    ///
    /// - A body of exactly [`FixedRecord::WIDTH`] bytes is decoded as it stands.
    /// - A shorter body is accepted only when every missing byte lies in the record's trailing
    ///   `FILLER`, that is when `WIDTH - body_len <= TRAILING_FILLER`: the missing tail is
    ///   supplied as spaces before decoding, and `body_len` keeps the line's own length, so
    ///   [`encode`](Self::encode) re-emits the line as it was. Line 1 of `cardxref.txt`, a
    ///   36-byte body holding card, customer `000000050` and account `00000000050`, decodes with
    ///   14 spaces of `FILLER` and `body_len` 36 (E04, FIXFRM-011).
    ///
    /// The framing stored is the line's terminator with `body_len` taken from `line.body.len()`.
    ///
    /// # Errors
    ///
    /// - [`ErrorKind::ShortBodyNotFiller`] for field `"record"` at offset 0 when the body is
    ///   shorter than the record by more than its trailing `FILLER`, so it ends inside a field
    ///   that carries data (R7).
    /// - [`ErrorKind::WidthMismatch`] for field `"record"` at offset 0 when the body is longer
    ///   than the record.
    /// - Otherwise the record's own [`FixedRecord::decode`] error, unchanged.
    pub fn decode(line: Line<'_>) -> Result<Self, Error> {
        let body = line.body;
        let record = if missing_tail::<R>(body.len())? == 0 {
            R::decode(body)?
        } else {
            let mut padded = Vec::with_capacity(R::WIDTH);
            padded.extend_from_slice(body);
            padded.resize(R::WIDTH, SPACE);
            R::decode(&padded)?
        };
        Ok(Self {
            record,
            framing: LineFraming {
                terminator: line.framing.terminator,
                body_len: body.len(),
            },
        })
    }

    /// Encodes the line: the record's bytes cut to `framing.body_len`, then the terminator.
    ///
    /// A short body comes back short. The record bytes beyond `body_len` are dropped, so a
    /// `Framed` built by hand with `body_len` below `WIDTH` must hold spaces there, as every
    /// `Framed` from [`decode`](Self::decode) does; [`try_encode`](Self::try_encode) checks it.
    /// A `body_len` above `WIDTH` cuts nothing.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        self.frame(self.record.encode())
    }

    /// Encodes the line as [`encode`](Self::encode) does, after checking that the bytes it
    /// returns decode back to this envelope (PIC-013).
    ///
    /// # Errors
    ///
    /// - [`ErrorKind::ShortBodyNotFiller`] or [`ErrorKind::WidthMismatch`], as
    ///   [`decode`](Self::decode) reports them, when `framing.body_len` is a length `decode`
    ///   would reject.
    /// - [`ErrorKind::NonBlankTail`] for field `"record"` at offset `body_len` when a record
    ///   byte beyond `body_len` is not a space: encoding would drop data, not blank `FILLER`.
    pub fn try_encode(&self) -> Result<Vec<u8>, Error> {
        let body_len = self.framing.body_len;
        missing_tail::<R>(body_len)?;
        let bytes = self.record.encode();
        if bytes.iter().skip(body_len).all(|&byte| byte == SPACE) {
            Ok(self.frame(bytes))
        } else {
            Err(Error::new(
                RECORD,
                body_len,
                R::WIDTH,
                ErrorKind::NonBlankTail,
            ))
        }
    }

    /// Cuts encoded record bytes to the body length and appends the terminator.
    fn frame(&self, mut bytes: Vec<u8>) -> Vec<u8> {
        bytes.truncate(self.framing.body_len);
        bytes.extend_from_slice(self.framing.terminator.as_bytes());
        bytes
    }
}

/// Decodes every line of `input` with [`Framed::decode`], in input order.
///
/// # Errors
///
/// The error of the first line that fails, with its `offset` rebased from the offset within the
/// record to the absolute byte offset in `input`: the line's start plus the field offset. A
/// whole-record failure (field `"record"`, offset 0) therefore reports where its line starts.
/// Were that sum to overflow `usize`, the field offset would be kept.
pub fn decode_all<R: FixedRecord>(input: &[u8]) -> Result<Vec<Framed<R>>, Error> {
    let mut remaining = lines(input);
    let mut records = Vec::new();
    loop {
        // `rest` is always a suffix of `input`, so this is exactly the next line's start.
        let line_start = input.len().saturating_sub(remaining.rest.len());
        let Some(line) = remaining.next() else {
            return Ok(records);
        };
        records.push(Framed::decode(line).map_err(|err| rebase(err, line_start))?);
    }
}

/// Moves `err`'s offset from within a line's record to within the whole input.
fn rebase(err: Error, line_start: usize) -> Error {
    Error {
        offset: line_start.checked_add(err.offset).unwrap_or(err.offset),
        ..err
    }
}

/// Concatenates the [`Framed::encode`] output of every record, in order. For the output of
/// [`decode_all`] it reproduces the decoded input byte for byte.
#[must_use]
pub fn encode_all<R: FixedRecord>(records: &[Framed<R>]) -> Vec<u8> {
    records.iter().flat_map(Framed::encode).collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::{Framed, Line, LineFraming, Terminator, decode_all, encode_all, lines, rebase};
    use crate::error::{Error, ErrorKind};
    use crate::pic::{PicU, PicX};
    use crate::record::{FixedRecord, RecordReader, RecordWriter};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A line as the split tests compare it: body and terminator.
    type Split<'a> = (&'a [u8], Terminator);

    /// An input and the lines it must split into.
    type SplitCase = (&'static [u8], &'static [Split<'static>]);

    /// Line 1 of `cardxref.txt` without its LF: card, customer `000000050` and account
    /// `00000000050`, 36 bytes against the 50-byte `CVACT03Y` record (E04).
    const XREF_LINE: &[u8] = b"050002445376574000000005000000000050";

    /// [`XREF_LINE`] with a full 14-byte `FILLER` of `0` characters, the non-space `FILLER`
    /// shape of E05.
    const XREF_FULL: &[u8] = b"05000244537657400000000500000000005000000000000000";

    /// [`XREF_LINE`] with a non-digit in the first byte of `XREF-CUST-ID` (offset 16).
    const XREF_BAD_CUST: &[u8] = b"0500024453765740X0000005000000000050";

    /// The `CVACT03Y` `CARD-XREF-RECORD` layout (CVACT03Y-001 to CVACT03Y-004).
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Xref {
        card_num: PicX<16>,
        cust_id: PicU<9>,
        acct_id: PicU<11>,
        filler: PicX<14>,
    }

    impl FixedRecord for Xref {
        const WIDTH: usize = 50;
        const TRAILING_FILLER: usize = 14;

        fn decode(bytes: &[u8]) -> Result<Self, Error> {
            let mut r = RecordReader::new(bytes, Self::WIDTH)?;
            let record = Self {
                card_num: r.pic_x("XREF-CARD-NUM")?,
                cust_id: r.pic_u("XREF-CUST-ID")?,
                acct_id: r.pic_u("XREF-ACCT-ID")?,
                filler: r.pic_x("FILLER")?,
            };
            r.finish()?;
            Ok(record)
        }

        fn encode(&self) -> Vec<u8> {
            let mut w = RecordWriter::with_capacity(Self::WIDTH);
            w.pic_x(&self.card_num)
                .pic_u(self.cust_id)
                .pic_u(self.acct_id)
                .pic_x(&self.filler);
            w.finish()
        }
    }

    /// The input rebuilt from its lines: every body followed by its terminator.
    fn rejoined(input: &[u8]) -> Vec<u8> {
        lines(input)
            .flat_map(|line| {
                line.body
                    .iter()
                    .chain(line.framing.terminator.as_bytes())
                    .copied()
            })
            .collect()
    }

    /// `body` followed by `terminator`.
    fn line_bytes(body: &[u8], terminator: Terminator) -> Vec<u8> {
        [body, terminator.as_bytes()].concat()
    }

    // audit: PIC-014
    #[test]
    fn terminator_bytes() {
        assert_eq!(Terminator::Lf.as_bytes(), b"\n");
        assert_eq!(Terminator::CrLf.as_bytes(), b"\r\n");
        assert_eq!(Terminator::None.as_bytes(), b"");
    }

    // audit: PIC-014
    #[test]
    fn lines_split_at_lf_and_keep_every_byte() {
        let cases: [SplitCase; 7] = [
            (b"", &[]),
            (b"a\nb\n", &[(b"a", Terminator::Lf), (b"b", Terminator::Lf)]),
            (
                b"a\r\nb\n",
                &[(b"a", Terminator::CrLf), (b"b", Terminator::Lf)],
            ),
            (b"a\rb\n", &[(b"a\rb", Terminator::Lf)]),
            (b"a", &[(b"a", Terminator::None)]),
            (b"\n", &[(b"", Terminator::Lf)]),
            (b"\r", &[(b"\r", Terminator::None)]),
        ];
        for (input, expected) in cases {
            let split: Vec<Split<'_>> = lines(input)
                .map(|line| (line.body, line.framing.terminator))
                .collect();
            assert_eq!(split, expected, "{input:?}");
            assert!(
                lines(input).all(|line| line.framing.body_len == line.body.len()),
                "{input:?}"
            );
            assert_eq!(rejoined(input), input, "{input:?}");
        }
    }

    #[test]
    fn lines_is_fused_and_bounds_its_length() {
        let mut split = lines(b"ab\r\ncd");
        assert_eq!(split.size_hint(), (1, Some(6)));
        assert_eq!(split.next(), Some(Line::new(b"ab", Terminator::CrLf)));
        assert_eq!(split.next(), Some(Line::new(b"cd", Terminator::None)));
        assert_eq!(split.size_hint(), (0, Some(0)));
        assert_eq!(split.next(), None);
        assert_eq!(split.next(), None);
    }

    #[test]
    fn lines_debug_shows_only_the_remaining_length() {
        let rendered = format!("{:?}", lines(XREF_LINE));
        assert_eq!(rendered, "Lines { remaining: 36, .. }");
    }

    // audit: PIC-013
    // audit: FIXFRM-011
    #[test]
    fn short_body_decodes_with_blank_filler_and_reencodes_short() -> TestResult {
        let input = line_bytes(XREF_LINE, Terminator::Lf);
        let line = lines(&input).next().ok_or("no line")?;
        let framed = Framed::<Xref>::decode(line)?;
        assert_eq!(framed.record.filler, PicX::spaces());
        assert_eq!(framed.record.cust_id.get(), 50);
        assert_eq!(framed.record.acct_id.get(), 50);
        assert_eq!(
            framed.framing,
            LineFraming {
                terminator: Terminator::Lf,
                body_len: 36,
            }
        );
        assert_eq!(framed.encode(), input);
        assert_eq!(framed.try_encode()?, input);
        Ok(())
    }

    // audit: PIC-014
    #[test]
    fn full_body_round_trips_with_its_terminator() -> TestResult {
        for terminator in [Terminator::Lf, Terminator::CrLf, Terminator::None] {
            let framed = Framed::<Xref>::decode(Line::new(XREF_FULL, terminator))?;
            assert_eq!(framed.framing.body_len, Xref::WIDTH);
            assert_eq!(framed.framing.terminator, terminator);
            assert!(!framed.record.filler.is_spaces());
            let expected = line_bytes(XREF_FULL, terminator);
            assert_eq!(framed.encode(), expected);
            assert_eq!(framed.try_encode()?, expected);
        }
        Ok(())
    }

    // audit: PIC-013
    #[test]
    fn body_lengths_outside_the_filler_rule_are_typed_errors() {
        let cases = [
            (
                35,
                ErrorKind::ShortBodyNotFiller {
                    expected: 50,
                    actual: 35,
                },
            ),
            (
                0,
                ErrorKind::ShortBodyNotFiller {
                    expected: 50,
                    actual: 0,
                },
            ),
            (
                51,
                ErrorKind::WidthMismatch {
                    expected: 50,
                    actual: 51,
                },
            ),
        ];
        for (len, kind) in cases {
            let body = vec![b'0'; len];
            assert_eq!(
                Framed::<Xref>::decode(Line::new(&body, Terminator::Lf)),
                Err(Error::new("record", 0, 50, kind)),
                "body of {len} bytes"
            );
        }
    }

    #[test]
    fn record_errors_propagate_unchanged() {
        let line = Line::new(XREF_BAD_CUST, Terminator::Lf);
        assert_eq!(
            Framed::<Xref>::decode(line),
            Err(Error::new("XREF-CUST-ID", 16, 9, ErrorKind::NonDigit))
        );
    }

    // audit: PIC-013
    #[test]
    fn try_encode_rejects_what_decode_would_not_accept() -> TestResult {
        let mut framed = Framed::<Xref>::decode(Line::new(XREF_FULL, Terminator::Lf))?;
        framed.framing.body_len = 36;
        // `encode` drops the 14 `0` bytes of FILLER beyond the body; `try_encode` refuses.
        assert_eq!(framed.encode(), line_bytes(XREF_LINE, Terminator::Lf));
        assert_eq!(
            framed.try_encode(),
            Err(Error::new("record", 36, 50, ErrorKind::NonBlankTail))
        );
        let cases = [
            (
                35,
                ErrorKind::ShortBodyNotFiller {
                    expected: 50,
                    actual: 35,
                },
            ),
            (
                51,
                ErrorKind::WidthMismatch {
                    expected: 50,
                    actual: 51,
                },
            ),
        ];
        for (body_len, kind) in cases {
            framed.framing.body_len = body_len;
            assert_eq!(
                framed.try_encode(),
                Err(Error::new("record", 0, 50, kind)),
                "body_len {body_len}"
            );
        }
        // A `body_len` above the width cuts nothing.
        assert_eq!(framed.encode(), line_bytes(XREF_FULL, Terminator::Lf));
        Ok(())
    }

    // audit: PIC-014
    // audit: FIXFRM-013
    #[test]
    fn decode_all_and_encode_all_reproduce_mixed_framing() -> TestResult {
        let input = [
            line_bytes(XREF_FULL, Terminator::CrLf),
            line_bytes(XREF_LINE, Terminator::Lf),
            line_bytes(XREF_FULL, Terminator::Lf),
            line_bytes(XREF_LINE, Terminator::None),
        ]
        .concat();
        let records = decode_all::<Xref>(&input)?;
        let framings: Vec<LineFraming> = records.iter().map(|framed| framed.framing).collect();
        let expected = [
            (Terminator::CrLf, 50),
            (Terminator::Lf, 36),
            (Terminator::Lf, 50),
            (Terminator::None, 36),
        ]
        .map(|(terminator, body_len)| LineFraming {
            terminator,
            body_len,
        });
        assert_eq!(framings, expected);
        assert_eq!(encode_all(&records), input);
        assert!(decode_all::<Xref>(b"")?.is_empty());
        Ok(())
    }

    #[test]
    fn decode_all_rebases_error_offsets_to_the_input() {
        let first = line_bytes(XREF_LINE, Terminator::Lf);
        let cases = [
            (
                line_bytes(XREF_BAD_CUST, Terminator::Lf),
                Error::new("XREF-CUST-ID", 37 + 16, 9, ErrorKind::NonDigit),
            ),
            (
                line_bytes(&[b'0'; 35], Terminator::CrLf),
                Error::new(
                    "record",
                    37,
                    50,
                    ErrorKind::ShortBodyNotFiller {
                        expected: 50,
                        actual: 35,
                    },
                ),
            ),
        ];
        for (second, expected) in cases {
            let input = [first.as_slice(), second.as_slice(), first.as_slice()].concat();
            assert_eq!(decode_all::<Xref>(&input), Err(expected));
        }
    }

    #[test]
    fn rebase_keeps_the_field_offset_on_overflow() {
        let err = Error::new("FILLER", 36, 14, ErrorKind::NonPrintable);
        assert_eq!(rebase(err, 100).offset, 136);
        assert_eq!(rebase(err, usize::MAX), err);
    }
}
