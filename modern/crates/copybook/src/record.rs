//! The record contract and the field cursors that implement it (codec A, AAP 0.3.3.5, 0.5.4).
//!
//! [`FixedRecord`] is the contract every copybook record struct meets: a fixed width, a total
//! decoder and an encoder that reproduces the decoded bytes. The implementations live in
//! `domain::records`, one per copybook layout, never in this crate.
//!
//! [`RecordReader`] and [`RecordWriter`] keep those implementations short and free of offset
//! arithmetic. Each reads or writes one field at a time, left to right in copybook order, through
//! the picture codecs of [`pic`](crate::pic) and [`zoned`]. The reader owns every
//! offset and bounds check, so a record decoder needs no indexing or slicing, and it wraps each
//! primitive [`ErrorKind`] into an [`Error`] naming the copybook field, its byte offset in the
//! record and its width. A struct literal evaluates its fields in the order written, so listing
//! them in copybook order reads them in copybook order.
//!
//! Dates and timestamps are read only as [`PicDate`] and [`PicTimestamp`] (Typed Dates and
//! Timestamps). Neither cursor puts a byte of a field into an error or a `Debug` rendering: a
//! record can hold a card number, SSN, CVV or password (PIC-024, Cardholder Data Redaction).
//!
//! ```
//! use copybook::pic::{PicU, PicX};
//! use copybook::record::{FixedRecord, RecordReader, RecordWriter};
//!
//! /// `TAG-CODE PIC X(04)` and `TAG-SEQ PIC 9(03)`.
//! struct Tag {
//!     code: PicX<4>,
//!     seq: PicU<3>,
//! }
//!
//! impl FixedRecord for Tag {
//!     const WIDTH: usize = 7;
//!
//!     fn decode(bytes: &[u8]) -> Result<Self, copybook::Error> {
//!         let mut r = RecordReader::new(bytes, Self::WIDTH)?;
//!         let record = Self {
//!             code: r.pic_x("TAG-CODE")?,
//!             seq: r.pic_u("TAG-SEQ")?,
//!         };
//!         r.finish()?;
//!         Ok(record)
//!     }
//!
//!     fn encode(&self) -> Vec<u8> {
//!         let mut w = RecordWriter::with_capacity(Self::WIDTH);
//!         w.pic_x(&self.code).pic_u(self.seq);
//!         w.finish()
//!     }
//! }
//!
//! let tag = Tag::decode(b"AB  042")?;
//! assert_eq!(tag.code.to_text(), "AB");
//! assert_eq!(tag.seq.get(), 42);
//! assert_eq!(tag.encode(), b"AB  042");
//! # Ok::<(), copybook::Error>(())
//! ```

use std::fmt;

use rust_decimal::Decimal;

use crate::error::{Error, ErrorKind};
use crate::pic::{Count, PicDate, PicS, PicTimestamp, PicU, PicX, digit_count};
use crate::zoned::{self, binary, packed};

/// Field name of a whole-record failure: a slice of the wrong length or bytes left over.
const RECORD: &str = "record";

/// A copybook record layout with a lossless fixed-width byte codec (codec A).
///
/// Every implementation meets this contract, which `domain/tests/codec_a_roundtrip.rs` and the
/// `total_parsers.rs` suites check for each layout:
///
/// - [`decode`](Self::decode) is total: for any byte slice of any length it returns a record or
///   a typed [`Error`] and never panics (Total Parsers Over Untrusted Input). A slice whose length
///   is not [`WIDTH`](Self::WIDTH) is [`ErrorKind::WidthMismatch`] for field `"record"`, the check
///   [`RecordReader::new`] performs.
/// - [`encode`](Self::encode) returns exactly `WIDTH` bytes.
/// - `encode(&decode(bytes)?) == bytes` for every slice `decode` accepts: padding, `FILLER`
///   contents (PIC-012), leading zeros (PIC-003) and sign bytes (PIC-006) come back unchanged,
///   and an input that could not is a typed error, never a normalised value (R7).
///
/// Implementations live in `domain::records`, never in this crate, which supplies the field
/// codecs they compose.
pub trait FixedRecord: Sized {
    /// Record length in bytes as the copybook declares it, the file's `LRECL` or the copybook's
    /// `RECLN` comment: 50 for `CVACT03Y` `CARD-XREF-RECORD`, 80 for `CSUSR01Y` `SEC-USER-DATA`.
    const WIDTH: usize;

    /// Short-body allowance: the number of trailing bytes, read as spaces, that a line body may
    /// omit. The default, 0, accepts no short body, and an implementation may keep it even when
    /// its layout ends in `FILLER`, so that every short body is a typed error (R7). A non-zero
    /// value is at most the width of the layout's trailing `FILLER`, so no omitted byte belongs
    /// to a field that carries data.
    ///
    /// `framing::Framed::decode` accepts a line body shorter than [`WIDTH`](Self::WIDTH) by at
    /// most this many bytes: it supplies the missing tail as spaces and re-emits the body at its
    /// own length (PIC-013). The value never exceeds `WIDTH`. For example, two layouts that allow
    /// their whole trailing `FILLER` to be omitted:
    ///
    /// - `CVACT03Y` ends with `FILLER PIC X(14)` (CVACT03Y-004), so the 36-byte lines of
    ///   `cardxref.txt` decode against the 50-byte record (E04). They are the only short bodies in
    ///   the fixture corpus.
    /// - `CSUSR01Y` ends with `SEC-USR-FILLER PIC X(23)` (CSUSR01Y-006), the 23 bytes the 57-byte
    ///   in-stream lines of `app/jcl/DUSRSECJ.jcl:35-44` lack against the 80-byte record.
    ///   `dusrsecj` pads those lines to the 80-byte card image IEBGENER reads before decoding
    ///   them, so they never take the short-body path.
    const TRAILING_FILLER: usize = 0;

    /// Decodes one record from exactly [`WIDTH`](Self::WIDTH) bytes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] for field `"record"` when `bytes` is not `WIDTH` bytes long;
    /// otherwise the first field that fails to decode, as an [`Error`] naming that field, its
    /// offset and its width; [`ErrorKind::TrailingBytes`] when the fields read do not fill the
    /// record.
    fn decode(bytes: &[u8]) -> Result<Self, Error>;

    /// Encodes the record as exactly [`WIDTH`](Self::WIDTH) bytes.
    #[must_use]
    fn encode(&self) -> Vec<u8>;
}

/// A cursor over the bytes of one record that decodes its fields left to right.
///
/// Each method consumes one field's bytes and returns its value. A failure is an [`Error`] that
/// carries the copybook field name the caller passed (`"ACCT-CURR-BAL"`), the field's byte offset
/// within the record and its width, and never the bytes themselves. Fields are cut with
/// `split_at_checked` and offsets advanced with checked arithmetic, so every method is total for
/// any input (Total Parsers Over Untrusted Input), and a failed method leaves the reader where it
/// was.
///
/// `Debug` shows the offset and the record width only, never the bytes (PIC-024).
pub struct RecordReader<'a> {
    /// Bytes not yet consumed.
    rest: &'a [u8],
    /// Byte offset of `rest` within the record.
    offset: usize,
    /// Record width the reader was created for.
    width: usize,
}

impl<'a> RecordReader<'a> {
    /// A reader over `bytes`, which must be exactly `width` bytes long, the record's
    /// [`FixedRecord::WIDTH`]. Every implementation passes its width here, so the length check
    /// of the [`FixedRecord`] contract lives in one place.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] for field `"record"` at offset 0 when `bytes` is not `width`
    /// bytes long.
    pub fn new(bytes: &'a [u8], width: usize) -> Result<Self, Error> {
        if bytes.len() == width {
            Ok(Self {
                rest: bytes,
                offset: 0,
                width,
            })
        } else {
            Err(Error::new(
                RECORD,
                0,
                width,
                ErrorKind::WidthMismatch {
                    expected: width,
                    actual: bytes.len(),
                },
            ))
        }
    }

    /// Byte offset of the next field within the record.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Consumes the next `len` bytes unchanged: a field kept as raw bytes, or a group the caller
    /// decodes itself.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`] when fewer than `len` bytes remain, with `expected` the
    /// requested `len` and `actual` the bytes left; [`ErrorKind::OutOfRange`] when the next
    /// offset would overflow `usize`. Either way the reader is unchanged.
    pub fn take(&mut self, field: &'static str, len: usize) -> Result<&'a [u8], Error> {
        self.read(field, len, Ok)
    }

    /// Decodes a `PIC X(N)` field, its `N` bytes kept verbatim (PIC-001, PIC-012).
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take) for `N` bytes, then those of [`PicX::decode`] with this
    /// field's name, offset and width.
    pub fn pic_x<const N: usize>(&mut self, field: &'static str) -> Result<PicX<N>, Error> {
        self.read(field, N, PicX::<N>::decode)
    }

    /// Decodes an unsigned DISPLAY `PIC 9(N)` field of `N` digits (PIC-003, PIC-004).
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take) for `N` bytes, then those of [`PicU::decode`] with this
    /// field's name, offset and width.
    pub fn pic_u<const N: u32>(&mut self, field: &'static str) -> Result<PicU<N>, Error> {
        self.read(field, digit_count(N), PicU::<N>::decode)
    }

    /// Decodes a signed integral DISPLAY `PIC S9(N)` field with a trailing overpunch sign,
    /// `N` bytes (PIC-005 to PIC-009).
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take) for `N` bytes, then those of [`zoned::decode_pic_s`] with
    /// this field's name, offset and width.
    pub fn pic_s_zoned<const N: u32>(&mut self, field: &'static str) -> Result<PicS<N>, Error> {
        self.read(field, digit_count(N), zoned::decode_pic_s::<N>)
    }

    /// Decodes a signed integral `PIC S9(N) COMP-3` field, [`packed::byte_width`]`(N)` bytes
    /// (PIC-010).
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take) for that width, then those of [`packed::decode_pic_s`] with
    /// this field's name, offset and width.
    pub fn pic_s_packed<const N: u32>(&mut self, field: &'static str) -> Result<PicS<N>, Error> {
        let width = self.picture_width(field, packed::byte_width(N))?;
        self.read(field, width, packed::decode_pic_s::<N>)
    }

    /// Decodes a signed DISPLAY `PIC S9(int_digits)V9(scale)` field with a trailing overpunch
    /// sign, `int_digits + scale` bytes, to a `Decimal` at scale `scale` (PIC-005 to PIC-009).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] with width 0 when `int_digits + scale` overflows `u32`, a
    /// picture with no width; then those of [`take`](Self::take) for `int_digits + scale` bytes;
    /// then those of [`zoned::decode_signed`] with this field's name, offset and width, which
    /// include an unsupported digit count (PIC-023).
    pub fn zoned(
        &mut self,
        field: &'static str,
        int_digits: u32,
        scale: u32,
    ) -> Result<Decimal, Error> {
        let digits = int_digits
            .checked_add(scale)
            .map(digit_count)
            .ok_or(ErrorKind::OutOfRange);
        let width = self.picture_width(field, digits)?;
        self.read(field, width, |bytes| {
            zoned::decode_signed(bytes, int_digits, scale)
        })
    }

    /// Decodes a `COMP-3` field of `digits` digits, `scale` of them after the implied point,
    /// [`packed::byte_width`]`(digits)` bytes, to a `Decimal` at scale `scale` (PIC-010).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] with width 0 when [`packed::byte_width`] rejects `digits`
    /// (PIC-023); then those of [`take`](Self::take) for that width; then those of
    /// [`packed::decode`] with this field's name, offset and width.
    pub fn packed(
        &mut self,
        field: &'static str,
        digits: u32,
        scale: u32,
    ) -> Result<Decimal, Error> {
        let width = self.picture_width(field, packed::byte_width(digits))?;
        self.read(field, width, |bytes| packed::decode(bytes, digits, scale))
    }

    /// Decodes a `PIC S9(4) COMP` count, 2 bytes big-endian (PIC-011).
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take) for 2 bytes; every halfword is a valid count.
    pub fn count(&mut self, field: &'static str) -> Result<Count, Error> {
        self.read(field, binary::WIDTH, binary::decode_count)
    }

    /// Decodes a `PIC X(10)` date: ten spaces or `YYYY-MM-DD` (PIC-015, PIC-016).
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take) for 10 bytes, then those of [`PicDate::decode`] with this
    /// field's name, offset and width.
    pub fn date(&mut self, field: &'static str) -> Result<PicDate, Error> {
        self.read(field, PicDate::WIDTH, PicDate::decode)
    }

    /// Decodes a `PIC X(26)` timestamp: 26 spaces or a value in layout `I`, `D` or `S`
    /// (PIC-017 to PIC-021).
    ///
    /// # Errors
    ///
    /// Those of [`take`](Self::take) for 26 bytes, then those of [`PicTimestamp::decode`] with
    /// this field's name, offset and width.
    pub fn timestamp(&mut self, field: &'static str) -> Result<PicTimestamp, Error> {
        self.read(field, PicTimestamp::WIDTH, PicTimestamp::decode)
    }

    /// Ends the record.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::TrailingBytes`] for field `"record"` at the offset reached, with the record
    /// width, when bytes remain: the fields read do not add up to the record.
    pub fn finish(self) -> Result<(), Error> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(Error::new(
                RECORD,
                self.offset,
                self.width,
                ErrorKind::TrailingBytes,
            ))
        }
    }

    /// Decodes the next `width` bytes and consumes them only when `decode` succeeds, wrapping a
    /// failure with the field's name, offset and width.
    fn read<T>(
        &mut self,
        field: &'static str,
        width: usize,
        decode: impl FnOnce(&'a [u8]) -> Result<T, ErrorKind>,
    ) -> Result<T, Error> {
        let in_context = |kind| Error::new(field, self.offset, width, kind);
        let (bytes, tail) = self
            .rest
            .split_at_checked(width)
            .ok_or(ErrorKind::WidthMismatch {
                expected: width,
                actual: self.rest.len(),
            })
            .map_err(in_context)?;
        let next = self
            .offset
            .checked_add(width)
            .ok_or(ErrorKind::OutOfRange)
            .map_err(in_context)?;
        let value = decode(bytes).map_err(in_context)?;
        self.rest = tail;
        self.offset = next;
        Ok(value)
    }

    /// The byte width a picture declares, or its failure at the current offset with width 0,
    /// because a picture the codecs reject has no width to report.
    fn picture_width(
        &self,
        field: &'static str,
        width: Result<usize, ErrorKind>,
    ) -> Result<usize, Error> {
        width.map_err(|kind| Error::new(field, self.offset, 0, kind))
    }
}

impl fmt::Debug for RecordReader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordReader")
            .field("offset", &self.offset)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

/// A builder for the bytes of one record that encodes its fields left to right.
///
/// Every method appends one field and returns the writer, so a record encoder is one chain in
/// copybook order, ended by [`finish`](Self::finish). Encoding is infallible: values are already
/// typed to their pictures, and the `Decimal` encoders store a wider or finer value by the COBOL
/// `MOVE` rule, as [`zoned::encode_signed`] and [`packed::encode`] state. For every picture the
/// [`RecordReader`] accepts, the matching method writes exactly the bytes that reader method
/// consumes, so a record decoded field by field re-encodes to its input.
///
/// `Debug` shows the number of bytes written only, never the bytes (PIC-024).
pub struct RecordWriter {
    out: Vec<u8>,
}

impl RecordWriter {
    /// An empty writer with room for `width` bytes, the record's [`FixedRecord::WIDTH`].
    #[must_use]
    pub fn with_capacity(width: usize) -> Self {
        Self {
            out: Vec::with_capacity(width),
        }
    }

    /// Appends a `PIC X(N)` field's `N` bytes, unchanged (PIC-001, PIC-012).
    pub fn pic_x<const N: usize>(&mut self, value: &PicX<N>) -> &mut Self {
        value.encode_into(&mut self.out);
        self
    }

    /// Appends a `PIC 9(N)` field as `N` digits, leading zeros restored (PIC-003).
    pub fn pic_u<const N: u32>(&mut self, value: PicU<N>) -> &mut Self {
        value.encode_into(&mut self.out);
        self
    }

    /// Appends a `PIC S9(N)` DISPLAY field, `N` bytes with a trailing overpunch sign (PIC-006).
    pub fn pic_s_zoned<const N: u32>(&mut self, value: PicS<N>) -> &mut Self {
        zoned::encode_pic_s(value, &mut self.out);
        self
    }

    /// Appends a `PIC S9(N) COMP-3` field, [`packed::byte_width`]`(N)` bytes (PIC-010).
    pub fn pic_s_packed<const N: u32>(&mut self, value: PicS<N>) -> &mut Self {
        packed::encode_pic_s(value, &mut self.out);
        self
    }

    /// Appends a `PIC S9(int_digits)V9(scale)` DISPLAY field, `int_digits + scale` bytes with a
    /// trailing overpunch sign (PIC-006).
    pub fn zoned(&mut self, value: Decimal, int_digits: u32, scale: u32) -> &mut Self {
        zoned::encode_signed(value, int_digits, scale, &mut self.out);
        self
    }

    /// Appends a `COMP-3` field of `digits` digits, `scale` after the implied point,
    /// [`packed::byte_width`]`(digits)` bytes (PIC-010).
    pub fn packed(&mut self, value: Decimal, digits: u32, scale: u32) -> &mut Self {
        packed::encode(value, digits, scale, &mut self.out);
        self
    }

    /// Appends a `PIC S9(4) COMP` count, 2 bytes big-endian (PIC-011).
    pub fn count(&mut self, value: Count) -> &mut Self {
        binary::encode_count(value, &mut self.out);
        self
    }

    /// Appends a `PIC X(10)` date: `YYYY-MM-DD`, or ten spaces for a blank date (PIC-015).
    pub fn date(&mut self, value: PicDate) -> &mut Self {
        value.encode_into(&mut self.out);
        self
    }

    /// Appends a `PIC X(26)` timestamp in the layout it was decoded from, or 26 spaces for a
    /// blank one (PIC-017 to PIC-020).
    pub fn timestamp(&mut self, value: &PicTimestamp) -> &mut Self {
        value.encode_into(&mut self.out);
        self
    }

    /// Appends raw bytes unchanged, the mirror of [`RecordReader::take`].
    pub fn bytes(&mut self, bytes: &[u8]) -> &mut Self {
        self.out.extend_from_slice(bytes);
        self
    }

    /// Number of bytes written so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.out.len()
    }

    /// Whether nothing has been written yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.out.is_empty()
    }

    /// The record's bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.out
    }
}

impl fmt::Debug for RecordWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RecordWriter")
            .field("len", &self.out.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use rust_decimal::Decimal;

    use super::{FixedRecord, RecordReader, RecordWriter};
    use crate::error::{Error, ErrorKind};
    use crate::pic::{CobolTimestamp, Count, PicDate, PicS, PicTimestamp, PicU, PicX, TsForm};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Line 1 of `cardxref.txt` without its LF: 36 bytes against the 50-byte `CVACT03Y` record,
    /// the 14 missing bytes being its trailing `FILLER` (E04).
    const XREF_LINE: &[u8] = b"050002445376574000000005000000000050";

    /// `XREF-CARD-NUM` of [`XREF_LINE`].
    const XREF_CARD: &[u8; 16] = b"0500024453765740";

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

    /// One field of every kind the cursors serve, 66 bytes, without trailing `FILLER`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    struct EveryKind {
        code: PicX<3>,
        seq: PicU<4>,
        zoned_int: PicS<3>,
        packed_int: PicS<5>,
        zoned_amount: Decimal,
        packed_amount: Decimal,
        count: Count,
        date: PicDate,
        timestamp: PicTimestamp,
        raw: Vec<u8>,
    }

    impl FixedRecord for EveryKind {
        const WIDTH: usize = 66;

        fn decode(bytes: &[u8]) -> Result<Self, Error> {
            let mut r = RecordReader::new(bytes, Self::WIDTH)?;
            let record = Self {
                code: r.pic_x("CODE")?,
                seq: r.pic_u("SEQ")?,
                zoned_int: r.pic_s_zoned("ZONED-INT")?,
                packed_int: r.pic_s_packed("PACKED-INT")?,
                zoned_amount: r.zoned("ZONED-AMT", 5, 2)?,
                packed_amount: r.packed("PACKED-AMT", 11, 2)?,
                count: r.count("COUNT")?,
                date: r.date("DATE")?,
                timestamp: r.timestamp("TS")?,
                raw: r.take("RAW", 2)?.to_vec(),
            };
            r.finish()?;
            Ok(record)
        }

        fn encode(&self) -> Vec<u8> {
            let mut w = RecordWriter::with_capacity(Self::WIDTH);
            w.pic_x(&self.code)
                .pic_u(self.seq)
                .pic_s_zoned(self.zoned_int)
                .pic_s_packed(self.packed_int)
                .zoned(self.zoned_amount, 5, 2)
                .packed(self.packed_amount, 11, 2)
                .count(self.count)
                .date(self.date)
                .timestamp(&self.timestamp)
                .bytes(&self.raw);
            w.finish()
        }
    }

    /// The fields of [`EveryKind`] in order, each as `(width, bytes)`.
    const EVERY_KIND_FIELDS: [(usize, &[u8]); 10] = [
        (3, b"AB "),
        (4, b"0042"),
        // −121: `J` is the overpunch of a negative last digit 1.
        (3, b"12J"),
        // −12345, sign nibble `D`.
        (3, &[0x12, 0x34, 0x5D]),
        // +123.45 at `S9(05)V99`: `E` is the overpunch of a positive last digit 5.
        (7, b"001234E"),
        // +123.45 at `S9(09)V99 COMP-3`: 11 digits and the sign nibble `C`.
        (6, &[0x00, 0x00, 0x00, 0x12, 0x34, 0x5C]),
        // −2 as a big-endian halfword.
        (2, &[0xFF, 0xFE]),
        (10, b"2022-07-18"),
        (26, b"2022-06-10 19:27:53.123456"),
        // Raw bytes pass through whatever they hold.
        (2, &[0x00, 0xFF]),
    ];

    fn every_kind_bytes() -> Vec<u8> {
        EVERY_KIND_FIELDS
            .iter()
            .flat_map(|&(_, bytes)| bytes.iter().copied())
            .collect()
    }

    fn every_kind() -> Result<EveryKind, Box<dyn StdError>> {
        let date = NaiveDate::from_ymd_opt(2022, 7, 18).ok_or("date")?;
        let stamp_date = NaiveDate::from_ymd_opt(2022, 6, 10).ok_or("timestamp date")?;
        let clock = NaiveTime::from_hms_micro_opt(19, 27, 53, 123_456).ok_or("clock")?;
        let stamp = CobolTimestamp::new(NaiveDateTime::new(stamp_date, clock), TsForm::Iso)?;
        Ok(EveryKind {
            code: PicX::from_array(*b"AB "),
            seq: PicU::new(42)?,
            zoned_int: PicS::new(-121)?,
            packed_int: PicS::new(-12_345)?,
            zoned_amount: Decimal::new(12_345, 2),
            packed_amount: Decimal::new(12_345, 2),
            count: Count::new(-2),
            date: PicDate::new(Some(date))?,
            timestamp: PicTimestamp::new(Some(stamp)),
            raw: vec![0x00, 0xFF],
        })
    }

    /// Reads one field from `bytes` behind a one-byte prefix, so that a field error reports
    /// offset 1.
    fn after_prefix<T>(
        bytes: &[u8],
        read: impl FnOnce(&mut RecordReader<'_>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let record = [b"#".as_slice(), bytes].concat();
        let mut r = RecordReader::new(&record, record.len())?;
        r.take("PREFIX", 1)?;
        read(&mut r)
    }

    #[test]
    fn cardxref_record_round_trips_with_its_blank_trailing_filler() -> TestResult {
        assert_eq!(XREF_LINE.len() + Xref::TRAILING_FILLER, Xref::WIDTH);
        let bytes = [XREF_LINE, &[b' '; 14]].concat();
        let record = Xref::decode(&bytes)?;
        assert_eq!(record.card_num, PicX::from_array(*XREF_CARD));
        assert_eq!(record.cust_id.get(), 50);
        assert_eq!(record.acct_id.get(), 50);
        assert!(record.filler.is_spaces());
        assert_eq!(record.encode(), bytes);
        Ok(())
    }

    #[test]
    fn trailing_filler_defaults_to_zero() {
        assert_eq!(EveryKind::TRAILING_FILLER, 0);
    }

    #[test]
    fn field_error_names_field_offset_and_width_but_no_bytes() -> TestResult {
        let bytes = [
            XREF_CARD.as_slice(),
            b"0000 0050",
            b"00000000050",
            &[b' '; 14],
        ]
        .concat();
        let error = Xref::decode(&bytes)
            .err()
            .ok_or("a blank digit was accepted")?;
        assert_eq!(
            error,
            Error::new("XREF-CUST-ID", 16, 9, ErrorKind::NonDigit)
        );
        let rendered = format!("{error:?} {error}");
        assert!(!rendered.contains("0500024453765740"), "{rendered}");
        assert!(!rendered.contains("0000 0050"), "{rendered}");
        Ok(())
    }

    #[test]
    fn new_rejects_a_slice_of_any_other_length() {
        for len in [0, 36, 49, 51] {
            assert_eq!(
                RecordReader::new(&vec![b' '; len], 50).err(),
                Some(Error::new(
                    "record",
                    0,
                    50,
                    ErrorKind::WidthMismatch {
                        expected: 50,
                        actual: len,
                    },
                )),
            );
        }
        assert_eq!(
            Xref::decode(XREF_LINE).err(),
            Some(Error::new(
                "record",
                0,
                50,
                ErrorKind::WidthMismatch {
                    expected: 50,
                    actual: 36,
                },
            )),
        );
    }

    #[test]
    fn take_past_the_end_fails_and_leaves_the_reader_unchanged() -> TestResult {
        let mut r = RecordReader::new(b"abcde", 5)?;
        assert_eq!(r.take("HEAD", 0), Ok(b"".as_slice()));
        assert_eq!(r.take("HEAD", 2), Ok(b"ab".as_slice()));
        assert_eq!(r.offset(), 2);
        for (len, remaining) in [(4, 3), (usize::MAX, 3)] {
            assert_eq!(
                r.take("TAIL", len),
                Err(Error::new(
                    "TAIL",
                    2,
                    len,
                    ErrorKind::WidthMismatch {
                        expected: len,
                        actual: remaining,
                    },
                )),
            );
            assert_eq!(r.offset(), 2);
        }
        assert_eq!(r.take("TAIL", 3), Ok(b"cde".as_slice()));
        assert_eq!(r.offset(), 5);
        assert_eq!(
            r.take("AFTER", 1),
            Err(Error::new(
                "AFTER",
                5,
                1,
                ErrorKind::WidthMismatch {
                    expected: 1,
                    actual: 0,
                },
            )),
        );
        r.finish()?;
        Ok(())
    }

    #[test]
    fn a_field_that_fails_to_decode_is_not_consumed() -> TestResult {
        let mut r = RecordReader::new(b"#12 4", 5)?;
        r.take("PREFIX", 1)?;
        assert_eq!(
            r.pic_u::<4>("DIGITS"),
            Err(Error::new("DIGITS", 1, 4, ErrorKind::NonDigit))
        );
        assert_eq!(r.offset(), 1);
        assert_eq!(r.pic_x::<4>("TEXT")?, PicX::from_array(*b"12 4"));
        assert_eq!(r.offset(), 5);
        r.finish()?;
        Ok(())
    }

    #[test]
    fn finish_reports_trailing_bytes_at_the_offset_reached() -> TestResult {
        let mut r = RecordReader::new(b"abcd", 4)?;
        r.take("HEAD", 1)?;
        assert_eq!(
            r.finish(),
            Err(Error::new("record", 1, 4, ErrorKind::TrailingBytes))
        );
        Ok(())
    }

    #[test]
    fn every_field_kind_round_trips() -> TestResult {
        let bytes = every_kind_bytes();
        assert_eq!(bytes.len(), EveryKind::WIDTH);
        let record = EveryKind::decode(&bytes)?;
        assert_eq!(record, every_kind()?);
        assert_eq!(record.zoned_amount.scale(), 2);
        assert_eq!(record.packed_amount.scale(), 2);
        assert_eq!(record.encode(), bytes);
        assert_eq!(every_kind()?.encode(), bytes);
        Ok(())
    }

    #[test]
    fn reader_offsets_and_writer_lengths_follow_the_field_widths() -> TestResult {
        let bytes = every_kind_bytes();
        let mut r = RecordReader::new(&bytes, EveryKind::WIDTH)?;
        let mut w = RecordWriter::with_capacity(EveryKind::WIDTH);
        assert!(w.is_empty());
        let mut ends = Vec::new();
        w.pic_x(&r.pic_x::<3>("CODE")?);
        ends.push((r.offset(), w.len()));
        w.pic_u(r.pic_u::<4>("SEQ")?);
        ends.push((r.offset(), w.len()));
        w.pic_s_zoned(r.pic_s_zoned::<3>("ZONED-INT")?);
        ends.push((r.offset(), w.len()));
        w.pic_s_packed(r.pic_s_packed::<5>("PACKED-INT")?);
        ends.push((r.offset(), w.len()));
        w.zoned(r.zoned("ZONED-AMT", 5, 2)?, 5, 2);
        ends.push((r.offset(), w.len()));
        w.packed(r.packed("PACKED-AMT", 11, 2)?, 11, 2);
        ends.push((r.offset(), w.len()));
        w.count(r.count("COUNT")?);
        ends.push((r.offset(), w.len()));
        w.date(r.date("DATE")?);
        ends.push((r.offset(), w.len()));
        w.timestamp(&r.timestamp("TS")?);
        ends.push((r.offset(), w.len()));
        w.bytes(r.take("RAW", 2)?);
        ends.push((r.offset(), w.len()));
        r.finish()?;

        let expected: Vec<(usize, usize)> = EVERY_KIND_FIELDS
            .iter()
            .scan(0, |end, &(width, field)| {
                assert_eq!(field.len(), width);
                *end += width;
                Some((*end, *end))
            })
            .collect();
        assert_eq!(ends, expected);
        assert_eq!(w.len(), EveryKind::WIDTH);
        assert_eq!(w.finish(), bytes);
        Ok(())
    }

    #[test]
    fn blank_dates_and_timestamps_decode_and_encode_as_spaces() -> TestResult {
        let date = after_prefix(&[b' '; 10], |r| r.date("DATE"))?;
        let timestamp = after_prefix(&[b' '; 26], |r| r.timestamp("TS"))?;
        assert_eq!(date, PicDate::BLANK);
        assert_eq!(timestamp, PicTimestamp::BLANK);
        let mut w = RecordWriter::with_capacity(36);
        w.date(date).timestamp(&timestamp);
        assert_eq!(w.finish(), [b' '; 36]);
        Ok(())
    }

    /// Each typed reader wraps its codec's failure with the field name, the field's offset (1,
    /// after the prefix) and its width; a picture the codecs reject reports width 0.
    #[test]
    fn typed_readers_wrap_codec_errors_in_field_context() {
        let field = |width, kind| Some(Error::new("F", 1, width, kind));
        let cases = [
            (
                after_prefix(b"A\x01", |r| r.pic_x::<2>("F")).err(),
                field(2, ErrorKind::NonPrintable),
            ),
            (
                after_prefix(b"1 3", |r| r.pic_u::<3>("F")).err(),
                field(3, ErrorKind::NonDigit),
            ),
            (
                after_prefix(b"123", |r| r.pic_s_zoned::<3>("F")).err(),
                field(3, ErrorKind::PlainDigitSign),
            ),
            (
                after_prefix(b"00}", |r| r.pic_s_zoned::<3>("F")).err(),
                field(3, ErrorKind::NegativeZero),
            ),
            (
                after_prefix(&[0x12, 0x34, 0x5F], |r| r.pic_s_packed::<5>("F")).err(),
                field(3, ErrorKind::InvalidPackedSign),
            ),
            (
                after_prefix(b"001234?", |r| r.zoned("F", 5, 2)).err(),
                field(7, ErrorKind::InvalidOverpunch),
            ),
            (
                after_prefix(b"", |r| r.zoned("F", 0, 0)).err(),
                field(0, ErrorKind::OutOfRange),
            ),
            (
                after_prefix(b"", |r| r.zoned("F", u32::MAX, 1)).err(),
                field(0, ErrorKind::OutOfRange),
            ),
            (
                after_prefix(&[0x10, 0, 0, 0, 0, 0x0C], |r| r.packed("F", 10, 2)).err(),
                field(6, ErrorKind::NonZeroPadNibble),
            ),
            (
                after_prefix(&[0x00, 0x00, 0x0C], |r| r.packed("F", 5, 6)).err(),
                field(3, ErrorKind::OutOfRange),
            ),
            (
                after_prefix(b"", |r| r.packed("F", 0, 0)).err(),
                field(0, ErrorKind::OutOfRange),
            ),
            (
                after_prefix(&[0x01], |r| r.count("F")).err(),
                field(
                    2,
                    ErrorKind::WidthMismatch {
                        expected: 2,
                        actual: 1,
                    },
                ),
            ),
            (
                after_prefix(b"2022-13-01", |r| r.date("F")).err(),
                field(10, ErrorKind::InvalidDate),
            ),
            (
                after_prefix(b"2022-06-10X19:27:53.000000", |r| r.timestamp("F")).err(),
                field(26, ErrorKind::UnknownTimestampLayout),
            ),
        ];
        for (index, (actual, expected)) in cases.into_iter().enumerate() {
            assert_eq!(actual, expected, "case {index}");
        }
    }

    #[test]
    fn debug_shows_positions_and_lengths_never_bytes() -> TestResult {
        let bytes = [XREF_LINE, &[b' '; 14]].concat();
        let mut r = RecordReader::new(&bytes, Xref::WIDTH)?;
        let card = r.pic_x::<16>("XREF-CARD-NUM")?;
        let reader = format!("{r:?}");
        assert_eq!(reader, "RecordReader { offset: 16, width: 50, .. }");

        let mut w = RecordWriter::with_capacity(Xref::WIDTH);
        w.pic_x(&card);
        let writer = format!("{w:?}");
        assert_eq!(writer, "RecordWriter { len: 16, .. }");
        Ok(())
    }
}
