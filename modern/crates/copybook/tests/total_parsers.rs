//! Total Parsers Over Untrusted Input, the `copybook` half (AAP 0.3.3.5): every public decoder
//! of bytes or characters from outside the process returns a value or a typed error for any
//! input of any length, and never panics. `domain/tests/total_parsers.rs` covers the record and
//! message decoders built on these.
//!
//! The decoders fed are `PicX::decode` and `PicX::from_text`, `PicX::moved_from`,
//! `PicU::decode`, `PicDate::decode`, `PicTimestamp::decode`, `TsForm::from_code`,
//! `zoned::{decode_signed, decode_pic_s}`, `zoned::packed::{byte_width, decode, decode_pic_s}`,
//! `zoned::binary::decode_count`, `lines`, `Framed::decode`, `framing::decode_all` and every
//! `RecordReader` method. `edited` only formats values and parses nothing.
//!
//! Inputs are generated, never read from a file: byte vectors of length 0 to 1,024, at least
//! 1,024 cases per property, plus a deterministic sweep of every length with boundary fill bytes
//! and a few structured vectors. A panic anywhere fails the test, and the test profile keeps
//! overflow checks on, so unchecked arithmetic in a decoder fails it too. Assertion messages
//! name the decoder and the input length only, never input bytes (Cardholder Data Redaction).

use std::iter;

use copybook::framing::{decode_all, encode_all};
use copybook::zoned::{self, binary, packed};
use copybook::{
    Error, FixedRecord, Framed, Line, PicDate, PicTimestamp, PicU, PicX, RecordReader,
    RecordWriter, Terminator, TsForm, lines,
};
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::strategy::Union;

/// Longest generated input, in bytes.
const MAX_LEN: usize = 1024;

/// Bytes [`read_sequence`] takes raw before its first picture field.
const LEAD: usize = 3;

/// `(int_digits, scale)` of the signed pictures the copybooks declare: `S9(10)V99`, `S9(09)V99`,
/// `S9(04)V99`, `S9(11)`, `S9(05)` and `S9(09)`, in DISPLAY (`CVACT01Y`, `CVTRA06Y`) and `COMP-3`
/// (`CIPAUSMY`, `CIPAUDTY`).
const AUDITED_PICTURES: [(u32, u32); 6] = [(10, 2), (9, 2), (4, 2), (11, 0), (5, 0), (9, 0)];

/// Fill bytes of the sweep: blank, the lowest and highest digit, the negative and positive zero
/// overpunch, and the two extreme bytes, which no picture accepts.
const FILLS: [u8; 7] = [b' ', b'0', b'9', b'}', b'{', 0xFF, 0x00];

/// Bytes that carry meaning in some picture or in line framing: digits, blank, date and time
/// separators, every overpunch sign, CR and LF.
const CODEC_TEXT: &[u8] = b"0123456789 -:.{}ABCDEFGHIJKLMNOPQR\r\n";

/// The low nibbles that are a `COMP-3` sign.
const PACKED_SIGNS: &[u8] = &[0x0C, 0x0D];

/// The well-formed fields [`read_sequence`] reads after its lead and before its date: `PIC X(16)`,
/// `PIC 9(09)`, `PIC S9(05)` DISPLAY (−121), `PIC S9(09) COMP-3` (12345), `PIC S9(10)V99`
/// DISPLAY (1234.50), `PIC S9(10)V99 COMP-3` (−123.45) and a `PIC S9(4) COMP` count of 7.
const FIELDS: [&[u8]; 7] = [
    b"SIXTEEN BYTES OK",
    b"000000050",
    b"0012J",
    &[0x00, 0x00, 0x12, 0x34, 0x5C],
    b"00000012345{",
    &[0x00, 0x00, 0x00, 0x00, 0x12, 0x34, 0x5D],
    &[0x00, 0x07],
];

/// A valid and a blank `PIC X(10)` date.
const DATES: [&[u8]; 2] = [b"2022-07-18", b"          "];

/// A `PIC X(26)` timestamp in each layout, `I`, `D` and `S`, and a blank one.
const TIMESTAMPS: [&[u8]; 4] = [
    b"2022-07-18 10:11:12.123456",
    b"2022-07-18-10.11.12.123456",
    b"2022-07-18                ",
    b"                          ",
];

/// The `CVACT03Y` `CARD-XREF-RECORD` layout (`app/cpy/CVACT03Y.cpy:5-8`): 50 bytes ending in a
/// 14-byte `FILLER`, so a 36-byte line body decodes against it (E04).
struct XrefShaped {
    card: PicX<16>,
    customer: PicU<9>,
    account: PicU<11>,
    filler: PicX<14>,
}

impl FixedRecord for XrefShaped {
    const WIDTH: usize = 50;
    const TRAILING_FILLER: usize = 14;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            card: reader.pic_x("XREF-CARD-NUM")?,
            customer: reader.pic_u("XREF-CUST-ID")?,
            account: reader.pic_u("XREF-ACCT-ID")?,
            filler: reader.pic_x("FILLER")?,
        };
        reader.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut writer = RecordWriter::with_capacity(Self::WIDTH);
        writer
            .pic_x(&self.card)
            .pic_u(self.customer)
            .pic_u(self.account)
            .pic_x(&self.filler);
        writer.finish()
    }
}

/// The shared property configuration: 1,024 cases, the rule's minimum, and no failure
/// persistence, so a failing run writes no `proptest-regressions/` file.
fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 1024,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// A byte biased toward what the decoders inspect: any byte, a byte of [`CODEC_TEXT`], a
/// `COMP-3` digit pair, or a `COMP-3` digit and sign. Pure random bytes almost never get past a
/// field's first digit check; these reach the digit, sign, nibble and date branches behind it.
fn codec_byte() -> impl Strategy<Value = u8> {
    Union::new([
        any::<u8>().boxed(),
        prop::sample::select(CODEC_TEXT).boxed(),
        (0_u8..=9, 0_u8..=9)
            .prop_map(|(high, low)| (high << 4) | low)
            .boxed(),
        (0_u8..=9, prop::sample::select(PACKED_SIGNS))
            .prop_map(|(digit, sign)| (digit << 4) | sign)
            .boxed(),
    ])
}

/// `(int_digits, scale)` of a picture the codecs support: 1 to 18 digits, at most 4 of them
/// after the implied point (AAP 0.3.3.4).
fn supported_picture() -> impl Strategy<Value = (u32, u32)> {
    (1_u32..=18)
        .prop_flat_map(|digits| (0..=digits.min(4)).prop_map(move |scale| (digits - scale, scale)))
}

/// Byte width of a DISPLAY field of `digits` digits; a width no slice can have when it does not
/// fit, so no window of it exists.
fn display_width(digits: u32) -> usize {
    usize::try_from(digits).unwrap_or(usize::MAX)
}

/// `bytes`, then its first and its last `width` bytes when it is that long: the whole input
/// meets the width check, the windows send its content past it.
fn windows(bytes: &[u8], width: usize) -> impl Iterator<Item = &[u8]> {
    let head = bytes.get(..width);
    let tail = bytes
        .len()
        .checked_sub(width)
        .and_then(|start| bytes.get(start..));
    iter::once(bytes).chain(head).chain(tail)
}

/// Every public decoder of external input, over `bytes` and its windows.
fn feed_all(bytes: &[u8]) {
    feed_pic_x::<1>(bytes);
    feed_pic_x::<3>(bytes);
    feed_pic_x::<9>(bytes);
    feed_pic_x::<11>(bytes);
    feed_pic_x::<16>(bytes);
    feed_pic_x::<18>(bytes);
    feed_pic_u::<1>(bytes);
    feed_pic_u::<3>(bytes);
    feed_pic_u::<9>(bytes);
    feed_pic_u::<11>(bytes);
    feed_pic_u::<16>(bytes);
    feed_pic_u::<18>(bytes);
    feed_pic_s::<5>(bytes);
    feed_pic_s::<9>(bytes);
    feed_pic_s::<11>(bytes);
    for (int_digits, scale) in AUDITED_PICTURES {
        feed_picture(bytes, int_digits, scale);
    }
    feed_dates(bytes);
    feed_counts(bytes);
    feed_framing(bytes);
    read_sequence(bytes, LEAD);
}

/// `PicX::<N>::decode`, `PicX::<N>::from_text` on the same bytes as text, and
/// `PicX::<N>::moved_from`.
fn feed_pic_x<const N: usize>(bytes: &[u8]) {
    for field in windows(bytes, N) {
        let _ = PicX::<N>::decode(field);
        let _ = PicX::<N>::from_text(&String::from_utf8_lossy(field));
        let _ = PicX::<N>::moved_from(field);
    }
}

/// `PicU::<N>::decode`.
fn feed_pic_u<const N: u32>(bytes: &[u8]) {
    for field in windows(bytes, display_width(N)) {
        let _ = PicU::<N>::decode(field);
    }
}

/// `PIC S9(N)` in DISPLAY through `zoned::decode_pic_s` and in `COMP-3` through
/// `packed::decode_pic_s`.
fn feed_pic_s<const N: u32>(bytes: &[u8]) {
    for field in windows(bytes, display_width(N)) {
        let _ = zoned::decode_pic_s::<N>(field);
    }
    let packed_width = packed::byte_width(N).unwrap_or(usize::MAX);
    for field in windows(bytes, packed_width) {
        let _ = packed::decode_pic_s::<N>(field);
    }
}

/// `PIC S9(int_digits)V9(scale)` in DISPLAY through `zoned::decode_signed` and in `COMP-3`
/// through `packed::decode`, for any parameters: a digit count that overflows, is zero or is
/// above the codecs' limit is an error like any malformed byte.
fn feed_picture(bytes: &[u8], int_digits: u32, scale: u32) {
    let digits = int_digits.checked_add(scale);
    for field in windows(bytes, digits.map_or(usize::MAX, display_width)) {
        let _ = zoned::decode_signed(field, int_digits, scale);
    }
    let packed_digits = digits.unwrap_or(u32::MAX);
    let packed_width = packed::byte_width(packed_digits).unwrap_or(usize::MAX);
    for field in windows(bytes, packed_width) {
        let _ = packed::decode(field, packed_digits, scale);
    }
    // Taking `int_digits` as the whole digit count lets `scale` exceed it.
    let _ = packed::decode(bytes, int_digits, scale);
}

/// `PicDate::decode`, `PicTimestamp::decode`, and `TsForm::from_code` on every byte.
fn feed_dates(bytes: &[u8]) {
    for field in windows(bytes, PicDate::WIDTH) {
        let _ = PicDate::decode(field);
    }
    for field in windows(bytes, PicTimestamp::WIDTH) {
        let _ = PicTimestamp::decode(field);
    }
    for &byte in bytes {
        let _ = TsForm::from_code(char::from(byte));
    }
}

/// `binary::decode_count` on the whole input and on each 2-byte chunk, the last one short when
/// the length is odd.
fn feed_counts(bytes: &[u8]) {
    let _ = binary::decode_count(bytes);
    for field in bytes.chunks(binary::WIDTH) {
        let _ = binary::decode_count(field);
    }
}

/// `lines` to its end, `Framed::decode` on every line it yields and on the raw input as a line
/// body, and `decode_all`.
fn feed_framing(bytes: &[u8]) {
    let len = bytes.len();
    let mut split = lines(bytes);
    let mut rejoined = Vec::with_capacity(len);
    for line in split.by_ref() {
        let terminator = line.framing.terminator.as_bytes();
        rejoined.extend_from_slice(line.body);
        rejoined.extend_from_slice(terminator);
        assert!(
            line.framing.body_len == line.body.len(),
            "lines: body length not recorded in a {len}-byte input"
        );
        if let Ok(framed) = Framed::<XrefShaped>::decode(line) {
            let original = [line.body, terminator].concat();
            // audit: PIC-013
            assert!(
                framed.encode() == original,
                "Framed::encode: an accepted line did not re-encode in a {len}-byte input"
            );
            assert!(
                framed.try_encode().is_ok_and(|encoded| encoded == original),
                "Framed::try_encode: an accepted line was refused in a {len}-byte input"
            );
        }
    }
    assert!(
        split.next().is_none(),
        "lines: yielded a line after its end"
    );
    // audit: PIC-014
    assert!(
        rejoined == bytes,
        "lines: bodies and terminators do not rebuild a {len}-byte input"
    );
    for terminator in [Terminator::Lf, Terminator::CrLf, Terminator::None] {
        for body in windows(bytes, 36).chain(windows(bytes, XrefShaped::WIDTH)) {
            let _ = Framed::<XrefShaped>::decode(Line::new(body, terminator));
        }
    }
    if let Ok(records) = decode_all::<XrefShaped>(bytes) {
        assert!(
            encode_all(&records) == bytes,
            "decode_all: accepted records did not re-encode a {len}-byte input"
        );
    }
}

/// Reads `bytes` as one record with every `RecordReader` method in a fixed order, starting with
/// `take(lead)`, and returns whether `finish` accepted it.
///
/// A failed read leaves the reader where it was, so every later method meets the same
/// exhausted or misaligned rest; two calls with pictures the codecs reject fail before reading.
/// Readers of a wrong width are refused at construction.
fn read_sequence(bytes: &[u8], lead: usize) -> bool {
    let len = bytes.len();
    let _ = RecordReader::new(bytes, XrefShaped::WIDTH);
    let _ = RecordReader::new(bytes, len.saturating_add(1));
    let Ok(mut reader) = RecordReader::new(bytes, len) else {
        panic!("RecordReader::new: refused a {len}-byte record of its own length");
    };
    let _ = reader.take("LEAD", lead);
    let _ = reader.pic_x::<16>("PIC-X-16");
    let _ = reader.pic_u::<9>("PIC-9-09");
    let _ = reader.pic_s_zoned::<5>("PIC-S9-05");
    let _ = reader.pic_s_packed::<9>("PIC-S9-09-COMP-3");
    let _ = reader.zoned("PIC-S9-10-V99", 10, 2);
    let _ = reader.packed("PIC-S9-10-V99-COMP-3", 12, 2);
    let _ = reader.count("PIC-S9-4-COMP");
    let _ = reader.date("DATE");
    let _ = reader.timestamp("TIMESTAMP");
    let _ = reader.zoned("NO-WIDTH", u32::MAX, 1);
    let _ = reader.packed("UNSUPPORTED", 0, 0);
    assert!(
        reader.offset() <= len,
        "RecordReader: offset passed the end of a {len}-byte record"
    );
    reader.finish().is_ok()
}

/// A record [`read_sequence`] reads to its end: the lead, [`FIELDS`], `date` and `timestamp`.
fn well_formed(date: &[u8], timestamp: &[u8]) -> Vec<u8> {
    let mut record = b"abc".to_vec();
    for field in FIELDS {
        record.extend_from_slice(field);
    }
    record.extend_from_slice(date);
    record.extend_from_slice(timestamp);
    record
}

/// Line-framing shapes the fill sweep cannot produce.
fn edge_vectors() -> Vec<Vec<u8>> {
    let mut two_good_then_bad = vec![b'0'; 36];
    two_good_then_bad.push(b'\n');
    two_good_then_bad.extend_from_slice(&[b'0'; XrefShaped::WIDTH]);
    two_good_then_bad.extend_from_slice(b"\r\nX");
    vec![
        b"\r\n".repeat(MAX_LEN / 2),
        b"\n\r".repeat(MAX_LEN / 2),
        [&[b'0'; MAX_LEN - 1][..], b"\r"].concat(),
        [&[b'0'; 36][..], b"\r"].concat(),
        b"\r".to_vec(),
        b"\n".to_vec(),
        vec![b'\n'; MAX_LEN],
        (0x80..=0xFF).collect(),
        (0x00..=0xFF).collect(),
        two_good_then_bad,
    ]
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn arbitrary_bytes_reach_every_decoder(
        bytes in prop::collection::vec(any::<u8>(), 0..=MAX_LEN),
    ) {
        feed_all(&bytes);
    }

    #[test]
    fn codec_shaped_bytes_reach_every_decoder(
        bytes in prop::collection::vec(codec_byte(), 0..=MAX_LEN),
    ) {
        feed_all(&bytes);
    }

    #[test]
    fn short_inputs_meet_every_width_exactly(
        bytes in prop::collection::vec(codec_byte(), 0..=64),
    ) {
        feed_all(&bytes);
    }

    #[test]
    fn signed_pictures_with_any_parameters(
        bytes in prop::collection::vec(codec_byte(), 0..=MAX_LEN),
        (int_digits, scale) in supported_picture(),
        wild_int_digits in any::<u32>(),
        wild_scale in any::<u32>(),
    ) {
        feed_picture(&bytes, int_digits, scale);
        feed_picture(&bytes, wild_int_digits, wild_scale);
    }

    #[test]
    fn every_char_is_a_form_code_or_an_error(code in any::<char>()) {
        if let Ok(form) = TsForm::from_code(code) {
            prop_assert_eq!(form.code(), code);
        }
    }

    #[test]
    fn reader_after_any_lead(
        bytes in prop::collection::vec(codec_byte(), 0..=MAX_LEN),
        lead in 0_usize..=2 * MAX_LEN,
    ) {
        read_sequence(&bytes, lead);
    }

    #[test]
    fn near_valid_records_reach_every_decoder(
        date in prop::sample::select(&DATES[..]),
        timestamp in prop::sample::select(&TIMESTAMPS[..]),
        mutations in prop::collection::vec((any::<Index>(), codec_byte()), 1..=8),
    ) {
        let mut record = well_formed(date, timestamp);
        let len = record.len();
        for (position, byte) in mutations {
            if let Some(slot) = record.get_mut(position.index(len)) {
                *slot = byte;
            }
        }
        feed_all(&record);
    }
}

#[test]
fn uniform_fills_at_every_length() {
    for fill in FILLS {
        for len in 0..=MAX_LEN {
            feed_all(&vec![fill; len]);
        }
    }
}

#[test]
fn structured_edge_vectors() {
    for bytes in edge_vectors() {
        feed_all(&bytes);
    }
}

#[test]
fn well_formed_records_and_every_prefix() {
    for date in DATES {
        for timestamp in TIMESTAMPS {
            let record = well_formed(date, timestamp);
            assert!(
                read_sequence(&record, LEAD),
                "RecordReader: a well-formed {}-byte record did not read to its end",
                record.len()
            );
            for len in 0..=record.len() {
                if let Some(prefix) = record.get(..len) {
                    feed_all(prefix);
                }
            }
        }
    }
}
