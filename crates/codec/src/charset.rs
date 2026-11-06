//! Record character sets: ASCII, as stored in `app/data/ASCII`, and EBCDIC as IBM
//! code page 037 (CP037), as stored in `app/data/EBCDIC`.
//!
//! The field codecs work on internal bytes only (ASCII extended to ISO-8859-1).
//! An EBCDIC record is translated as a whole into internal bytes, decoded by the
//! same field codecs, and translated back after encoding. ASCII is the identity.
//!
//! CP037 maps the 256 EBCDIC byte values one-to-one onto the 256 ISO-8859-1 byte
//! values, so translation is lossless in both directions and every record
//! re-encodes to its original bytes. [`LATIN1_TO_EBCDIC`] is derived from
//! [`EBCDIC_TO_LATIN1`] at compile time, and the build fails if that table is not
//! a permutation of `0..=255`.
//!
//! CP037 places the overpunch glyphs `{` and `A`–`I` (+0 to +9) at `0xC0`–`0xC9`,
//! `}` and `J`–`R` (−0 to −9) at `0xD0`–`0xD9`, and the digits at `0xF0`–`0xF9`.
//! The EBCDIC sign of a signed zoned field (zone nibble `C` positive, `D`
//! negative, on the last byte) therefore follows from the ASCII overpunch rule
//! with no EBCDIC-specific sign handling.
//!
//! Through these tables the 626 records of the nine EBCDIC counterpart files decode
//! to their space-padded ASCII records, except the two whose content differs
//! (escalation E-05 in `docs/parity/escalations.md`): account record 49 at offset
//! 102 (`ZEROAPR` against `A000000000`) and disclosure-group record 34 in
//! `DIS-INT-RATE` at offset 16 (`00150{` against `00000{`). The card
//! cross-reference filler is `0x40`, the EBCDIC space.

/// Record character set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Charset {
    /// ASCII text, as in the `app/data/ASCII` corpus. Stored bytes are internal
    /// bytes, so translation is the identity.
    Ascii,
    /// IBM code page 037, as in the `app/data/EBCDIC` corpus.
    Ebcdic,
}

impl Charset {
    /// Translates stored record bytes in this character set into internal
    /// (ISO-8859-1) bytes, one byte for one byte.
    #[must_use]
    pub fn to_internal(self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Self::Ascii => bytes.to_vec(),
            Self::Ebcdic => translate(bytes, &EBCDIC_TO_LATIN1),
        }
    }

    /// Translates internal (ISO-8859-1) bytes into stored record bytes in this
    /// character set, one byte for one byte. Inverse of [`Charset::to_internal`].
    #[must_use]
    pub fn from_internal(self, bytes: &[u8]) -> Vec<u8> {
        match self {
            Self::Ascii => bytes.to_vec(),
            Self::Ebcdic => translate(bytes, &LATIN1_TO_EBCDIC),
        }
    }
}

fn translate(bytes: &[u8], table: &[u8; 256]) -> Vec<u8> {
    bytes.iter().map(|&b| table[usize::from(b)]).collect()
}

/// CP037 to ISO-8859-1: entry `e` is the internal byte for EBCDIC byte `e`.
/// Row `n` holds EBCDIC bytes `0xn0`–`0xnF`.
pub const EBCDIC_TO_LATIN1: [u8; 256] = [
    0x00, 0x01, 0x02, 0x03, 0x9C, 0x09, 0x86, 0x7F, 0x97, 0x8D, 0x8E, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F,
    0x10, 0x11, 0x12, 0x13, 0x9D, 0x85, 0x08, 0x87, 0x18, 0x19, 0x92, 0x8F, 0x1C, 0x1D, 0x1E, 0x1F,
    0x80, 0x81, 0x82, 0x83, 0x84, 0x0A, 0x17, 0x1B, 0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x05, 0x06, 0x07,
    0x90, 0x91, 0x16, 0x93, 0x94, 0x95, 0x96, 0x04, 0x98, 0x99, 0x9A, 0x9B, 0x14, 0x15, 0x9E, 0x1A,
    0x20, 0xA0, 0xE2, 0xE4, 0xE0, 0xE1, 0xE3, 0xE5, 0xE7, 0xF1, 0xA2, 0x2E, 0x3C, 0x28, 0x2B, 0x7C,
    0x26, 0xE9, 0xEA, 0xEB, 0xE8, 0xED, 0xEE, 0xEF, 0xEC, 0xDF, 0x21, 0x24, 0x2A, 0x29, 0x3B, 0xAC,
    0x2D, 0x2F, 0xC2, 0xC4, 0xC0, 0xC1, 0xC3, 0xC5, 0xC7, 0xD1, 0xA6, 0x2C, 0x25, 0x5F, 0x3E, 0x3F,
    0xF8, 0xC9, 0xCA, 0xCB, 0xC8, 0xCD, 0xCE, 0xCF, 0xCC, 0x60, 0x3A, 0x23, 0x40, 0x27, 0x3D, 0x22,
    0xD8, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0xAB, 0xBB, 0xF0, 0xFD, 0xFE, 0xB1,
    0xB0, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E, 0x6F, 0x70, 0x71, 0x72, 0xAA, 0xBA, 0xE6, 0xB8, 0xC6, 0xA4,
    0xB5, 0x7E, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0xA1, 0xBF, 0xD0, 0xDD, 0xDE, 0xAE,
    0x5E, 0xA3, 0xA5, 0xB7, 0xA9, 0xA7, 0xB6, 0xBC, 0xBD, 0xBE, 0x5B, 0x5D, 0xAF, 0xA8, 0xB4, 0xD7,
    0x7B, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0xAD, 0xF4, 0xF6, 0xF2, 0xF3, 0xF5,
    0x7D, 0x4A, 0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x50, 0x51, 0x52, 0xB9, 0xFB, 0xFC, 0xF9, 0xFA, 0xFF,
    0x5C, 0xF7, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5A, 0xB2, 0xD4, 0xD6, 0xD2, 0xD3, 0xD5,
    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0xB3, 0xDB, 0xDC, 0xD9, 0xDA, 0x9F,
];

/// ISO-8859-1 to CP037: entry `l` is the EBCDIC byte for internal byte `l`.
pub const LATIN1_TO_EBCDIC: [u8; 256] = invert(&EBCDIC_TO_LATIN1);

/// Inverts a byte permutation. Constant evaluation fails unless every value
/// `0..=255` occurs in `table` exactly once.
///
/// Each loop keeps its position both as an index and as a byte, because a cast
/// would trip the pedantic cast lints and `From` is not callable in a `const fn`.
const fn invert(table: &[u8; 256]) -> [u8; 256] {
    let mut out = [0u8; 256];
    let mut j = 0usize;
    let mut jb = 0u8;
    while j < 256 {
        let mut hits = 0u32;
        let mut i = 0usize;
        let mut ib = 0u8;
        while i < 256 {
            if table[i] == jb {
                out[j] = ib;
                hits += 1;
            }
            i += 1;
            ib = ib.wrapping_add(1);
        }
        assert!(
            hits == 1,
            "translation table is not a permutation of 0..=255"
        );
        j += 1;
        jb = jb.wrapping_add(1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_bytes() -> Vec<u8> {
        (0..=255u8).collect()
    }

    #[test]
    fn tables_are_mutually_inverse_permutations() {
        for b in 0..=255u8 {
            let i = usize::from(b);
            assert_eq!(
                LATIN1_TO_EBCDIC[usize::from(EBCDIC_TO_LATIN1[i])],
                b,
                "EBCDIC 0x{b:02X}"
            );
            assert_eq!(
                EBCDIC_TO_LATIN1[usize::from(LATIN1_TO_EBCDIC[i])],
                b,
                "Latin-1 0x{b:02X}"
            );
        }
        for table in [EBCDIC_TO_LATIN1, LATIN1_TO_EBCDIC] {
            let mut sorted = table.to_vec();
            sorted.sort_unstable();
            assert_eq!(sorted, all_bytes());
        }
    }

    #[test]
    fn record_glyphs_map_to_their_cp037_bytes() {
        // (EBCDIC byte, internal byte): space, overpunch signs, punctuation, a
        // lower-case letter, and LF, whose CP037 position is 0x25.
        let mut pairs: Vec<(u8, u8)> = vec![
            (0x40, b' '),
            (0xC0, b'{'),
            (0xD0, b'}'),
            (0x4B, b'.'),
            (0x60, b'-'),
            (0x81, b'a'),
            (0x25, 0x0A),
        ];
        pairs.extend((0xF0..=0xF9).zip(b'0'..=b'9'));
        pairs.extend((0xC1..=0xC9).zip(b'A'..=b'I'));
        pairs.extend((0xD1..=0xD9).zip(b'J'..=b'R'));
        for (ebcdic, latin1) in pairs {
            assert_eq!(
                EBCDIC_TO_LATIN1[usize::from(ebcdic)],
                latin1,
                "EBCDIC 0x{ebcdic:02X}"
            );
            assert_eq!(
                LATIN1_TO_EBCDIC[usize::from(latin1)],
                ebcdic,
                "Latin-1 0x{latin1:02X}"
            );
        }
    }

    #[test]
    fn ascii_is_identity() {
        let bytes = all_bytes();
        assert_eq!(Charset::Ascii.to_internal(&bytes), bytes);
        assert_eq!(Charset::Ascii.from_internal(&bytes), bytes);
    }

    #[test]
    fn ebcdic_translates_through_the_tables() {
        let bytes = all_bytes();
        assert_eq!(Charset::Ebcdic.to_internal(&bytes), EBCDIC_TO_LATIN1);
        assert_eq!(Charset::Ebcdic.from_internal(&bytes), LATIN1_TO_EBCDIC);
    }

    #[test]
    fn every_charset_round_trips_every_byte() {
        let bytes = all_bytes();
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            assert_eq!(
                charset.from_internal(&charset.to_internal(&bytes)),
                bytes,
                "{charset:?} decode then encode"
            );
            assert_eq!(
                charset.to_internal(&charset.from_internal(&bytes)),
                bytes,
                "{charset:?} encode then decode"
            );
            assert!(charset.to_internal(&[]).is_empty());
        }
    }
}
