//! `Alnum<N>`: a COBOL `PIC X(N)` alphanumeric item.
//!
//! Section A of `docs/parity/parity-oracle-audit.md` maps every copybook
//! alphanumeric field that is not a key or code to `Alnum<N>`, for example
//! `ACCT-OPEN-DATE X(10)` (P-CVACT01Y-006) [app/cpy/CVACT01Y.cpy:L10],
//! `ACCT-EXPIRAION-DATE X(10)` (P-CVACT01Y-007), `ACCT-GROUP-ID X(10)`
//! (P-CVACT01Y-012) [app/cpy/CVACT01Y.cpy:L16], `TRAN-DESC X(100)`
//! (P-CVTRA05Y-005) [app/cpy/CVTRA05Y.cpy:L9], `TRAN-ORIG-TS X(26)`
//! (P-CVTRA05Y-012) [app/cpy/CVTRA05Y.cpy:L16], `DALYTRAN-ORIG-TS X(26)`
//! (P-CVTRA06Y-012), `CUST-FIRST-NAME X(25)` (P-CVCUS01Y-002)
//! [app/cpy/CVCUS01Y.cpy:L6], `SEC-USR-FNAME X(20)` (P-CSUSR01Y-002) and the
//! reject record's `REJECT-TRAN-DATA X(350)` (P-CBTRN02C-001). Its SQL column
//! is `VARCHAR(N)`, right-trimmed: persistence binds [`Alnum::trimmed`] and
//! rebuilds the item with [`Alnum::from_trimmed`].
//!
//! The type holds the stored bytes exactly, trailing spaces and low-values
//! included, so that a decoded fixture field re-encodes to its original bytes.
//! Dates and timestamps stay text, as in the COBOL, so their comparisons are
//! the COBOL alphanumeric comparisons: `CBTRN02C` `1500-B-LOOKUP-ACCT` tests
//! `ACCT-EXPIRAION-DATE >= DALYTRAN-ORIG-TS (1:10)` byte by byte
//! (P-CBTRN02C-011) [app/cbl/CBTRN02C.cbl:L414-420], which is
//! [`Alnum::cobol_cmp`] of the expiry date against [`Alnum::ref_mod`]`(1, 10)`
//! of the timestamp.
//!
//! The type offers the alphanumeric `MOVE`, reference modification, the
//! `SPACES`/`LOW-VALUES` and `IS NUMERIC` tests and the comparison. It has no
//! arithmetic and no case conversion: sign-on upper-casing belongs to
//! `auth::password`.

use core::cmp::Ordering;
use core::iter::repeat;
use std::borrow::Cow;
use std::fmt;

use super::NumericError;

/// The COBOL figurative constant `SPACE`, the padding of every alphanumeric
/// `MOVE`.
const SPACE: u8 = b' ';

/// The COBOL figurative constant `LOW-VALUE`.
const LOW_VALUE: u8 = 0x00;

/// A COBOL `PIC X(N)` item: exactly `N` bytes, held as stored.
///
/// # Ordering
///
/// `Ord` and `PartialOrd` compare the stored bytes in byte order. Two items of
/// the same `X(N)` width therefore compare as the COBOL compares them, under
/// ASCII collation. The parity corpus is ASCII (escalation E-05: ASCII seeds
/// and drives parity), so ASCII collation is the one modelled. EBCDIC
/// collation, where letters sort before digits, differs and is not modelled:
/// a browse whose audited order depends on it is an escalation for the owner
/// of that rule or repository, not something this type can settle.
///
/// For operands of different widths, or a literal, use [`Alnum::cobol_cmp`],
/// which pads the shorter operand with spaces as the COBOL does.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Alnum<const N: usize>([u8; N]);

impl<const N: usize> Alnum<N> {
    /// The field length in bytes: the `N` of `X(N)`.
    pub const LEN: usize = N;

    /// An item holding `N` spaces: `MOVE SPACES`, and `INITIALIZE` of an
    /// alphanumeric item.
    #[must_use]
    pub const fn spaces() -> Self {
        Self([SPACE; N])
    }

    /// An item holding exactly these bytes.
    #[must_use]
    pub const fn from_array(bytes: [u8; N]) -> Self {
        Self(bytes)
    }

    /// An item holding exactly these bytes, as a record codec decodes a field.
    ///
    /// # Errors
    ///
    /// [`NumericError::WrongLength`] when `bytes` is not exactly `N` bytes
    /// long. Nothing is padded or cut: a slice of another length is a layout
    /// error, not a `MOVE`.
    // No `#[must_use]` here, unlike the other constructors: `Result` already
    // carries it, and clippy's `double_must_use` rejects the pair.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, NumericError> {
        <[u8; N]>::try_from(bytes)
            .map(Self)
            .map_err(|_| NumericError::WrongLength {
                expected: N,
                actual: bytes.len(),
            })
    }

    /// The item rebuilt from the right-trimmed text of its `VARCHAR(N)`
    /// column: the path persistence reads a stored item back by.
    ///
    /// Each character becomes the byte with the same code point (ISO 8859-1,
    /// Latin-1), the reverse of the mapping [`Alnum::as_text`] applies, so `é`
    /// becomes byte `0xE9` again. The bytes fill the item left-justified and
    /// the rest of it is spaces, which restores the trailing spaces
    /// [`Alnum::trimmed`] removed. The width counts characters, not UTF-8
    /// bytes: `ééé` fills an `X(3)` item. This is the exact inverse of
    /// [`Alnum::trimmed`]: `Alnum::from_trimmed(&item.trimmed())` is
    /// `Ok(item)` for every item.
    ///
    /// # Errors
    ///
    /// [`NumericError::TooLong`] when `text` has more than `N` characters,
    /// checked first, and [`NumericError::NotLatin1`] when a character lies
    /// above `U+00FF`. Nothing is cut or replaced: either would change the
    /// stored bytes.
    // No `#[must_use]`, for the reason given at `from_bytes`.
    pub fn from_trimmed(text: &str) -> Result<Self, NumericError> {
        let actual = text.chars().count();
        if actual > N {
            return Err(NumericError::TooLong { max: N, actual });
        }
        let mut bytes = [SPACE; N];
        for (dst, character) in bytes.iter_mut().zip(text.chars()) {
            *dst = u8::try_from(character).map_err(|_| NumericError::NotLatin1)?;
        }
        Ok(Self(bytes))
    }

    /// The COBOL alphanumeric `MOVE` of `src` into an `X(N)` receiver.
    ///
    /// The first `min(src.len(), N)` bytes are copied left-justified, the rest
    /// of the receiver is filled with spaces, and bytes beyond `N` are cut on
    /// the right.
    #[must_use]
    pub fn move_from_bytes(src: &[u8]) -> Self {
        let mut bytes = [SPACE; N];
        for (dst, &byte) in bytes.iter_mut().zip(src) {
            *dst = byte;
        }
        Self(bytes)
    }

    /// The COBOL alphanumeric `MOVE` of the UTF-8 bytes of `src`.
    ///
    /// The semantics are the byte semantics of [`Alnum::move_from_bytes`], as
    /// in the COBOL: a multi-byte UTF-8 character that straddles byte `N` is
    /// cut, and a non-ASCII character is stored as its UTF-8 bytes, which
    /// [`Alnum::as_text`] then reads one byte per character. Text and bytes
    /// agree only for ASCII text.
    ///
    /// Request text is held to ASCII only where its API request DTO checks
    /// it, which a DTO does for a member whose bytes its rule moves unedited
    /// or quotes back in its answer. Other members arrive as entered, for the
    /// audited edit to read: `tran_type_update::map_input` moves the
    /// `COTRTUPC` type code and description as given, so a non-ASCII
    /// character there is stored as its UTF-8 bytes.
    ///
    /// Text read back from an SQL column is rebuilt with
    /// [`Alnum::from_trimmed`], with one exception: the `COTRTUPC` no-change
    /// test, `tran_type_update::unchanged`, moves the stored description with
    /// this method, so that it and the entered description, which
    /// `map_input` also moves, compare in the same byte form.
    #[must_use]
    pub fn move_from(src: &str) -> Self {
        Self::move_from_bytes(src.as_bytes())
    }

    /// The stored bytes, all `N` of them.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; N] {
        &self.0
    }

    /// The stored bytes as text, trailing spaces included.
    ///
    /// When every byte is ASCII the text borrows the bytes. Otherwise each byte
    /// becomes the character with the same code point (ISO 8859-1, Latin-1):
    /// byte `0xE9` reads as `é`. That mapping is lossless and deterministic,
    /// one character per byte, and never fails, whatever the bytes are.
    #[must_use]
    pub fn as_text(&self) -> Cow<'_, str> {
        text_of(&self.0)
    }

    /// The stored bytes without their trailing spaces.
    ///
    /// Only spaces are removed. Trailing low-values and any other byte are
    /// kept, so a field the COBOL leaves as low-values is not mistaken for an
    /// empty one.
    #[must_use]
    pub fn trimmed_bytes(&self) -> &[u8] {
        let mut bytes: &[u8] = &self.0;
        while let [rest @ .., SPACE] = bytes {
            bytes = rest;
        }
        bytes
    }

    /// The text of [`Alnum::trimmed_bytes`], under the same ASCII and Latin-1
    /// rule as [`Alnum::as_text`].
    ///
    /// This is the right-trimmed `VARCHAR(N)` value persistence binds; reading
    /// it back, persistence rebuilds the same item with
    /// [`Alnum::from_trimmed`].
    #[must_use]
    pub fn trimmed(&self) -> Cow<'_, str> {
        text_of(self.trimmed_bytes())
    }

    /// The COBOL condition `= SPACES OR = LOW-VALUES`: every byte is a space,
    /// or every byte is `0x00`.
    ///
    /// A mix of spaces and low-values satisfies neither figurative constant,
    /// so it is not blank.
    #[must_use]
    pub fn is_blank(&self) -> bool {
        self.0.iter().all(|&byte| byte == SPACE) || self.0.iter().all(|&byte| byte == LOW_VALUE)
    }

    /// The COBOL class test `IS NUMERIC` on an alphanumeric item: every byte
    /// is a digit `0` to `9`.
    ///
    /// Spaces, signs and decimal points fail the test, as in the COBOL.
    #[must_use]
    pub fn is_numeric(&self) -> bool {
        self.0.iter().all(u8::is_ascii_digit)
    }

    /// The first `min(len, N)` stored bytes.
    #[must_use]
    pub fn prefix(&self, len: usize) -> &[u8] {
        let (head, _) = self.0.split_at(len.min(N));
        head
    }

    /// The COBOL reference modification `(start:len)`: `len` bytes from the
    /// 1-based position `start`.
    ///
    /// `None` when `start` is 0, `len` is 0, or the range runs past byte `N`.
    /// The COBOL result is undefined there, so a caller that meets `None`
    /// escalates rather than guessing a value.
    #[must_use]
    pub fn ref_mod(&self, start: usize, len: usize) -> Option<&[u8]> {
        if len == 0 {
            return None;
        }
        let offset = start.checked_sub(1)?;
        let end = offset.checked_add(len)?;
        self.0.get(offset..end)
    }

    /// The COBOL alphanumeric comparison of this item with `other`.
    ///
    /// The shorter operand is treated as padded on the right with spaces to
    /// the length of the longer, and the two are then compared byte by byte.
    /// This is the comparison behind `IF field = 'Y'` against a literal and
    /// behind comparisons of fields of different widths, such as the expiry
    /// test of `CBTRN02C` (P-CBTRN02C-011). Byte order is ASCII collation; see
    /// the type's ordering notes for EBCDIC.
    #[must_use]
    pub fn cobol_cmp(&self, other: &[u8]) -> Ordering {
        let width = N.max(other.len());
        space_padded(&self.0, width).cmp(space_padded(other, width))
    }
}

impl<const N: usize> Default for Alnum<N> {
    /// `INITIALIZE` of an alphanumeric item: all spaces.
    fn default() -> Self {
        Self::spaces()
    }
}

impl<const N: usize> fmt::Debug for Alnum<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Alnum<{N}>({:?})", self.as_text())
    }
}

impl<const N: usize> fmt::Display for Alnum<N> {
    /// The full text of the item, trailing spaces included.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_text())
    }
}

/// `bytes` as text: borrowed when all of them are ASCII, otherwise one Latin-1
/// character per byte.
fn text_of(bytes: &[u8]) -> Cow<'_, str> {
    match bytes.is_ascii().then(|| core::str::from_utf8(bytes)) {
        Some(Ok(text)) => Cow::Borrowed(text),
        _ => Cow::Owned(bytes.iter().copied().map(char::from).collect()),
    }
}

/// `bytes` followed by spaces, `width` bytes in all when `bytes` is no longer.
fn space_padded(bytes: &[u8], width: usize) -> impl Iterator<Item = u8> {
    bytes.iter().copied().chain(repeat(SPACE)).take(width)
}

#[cfg(test)]
mod tests {
    use core::cmp::Ordering;
    use std::borrow::Cow;

    use super::{Alnum, NumericError};

    /// A `TRAN-ORIG-TS X(26)` value as the fixtures hold it.
    const TIMESTAMP: &[u8; 26] = b"2022-06-10 19:27:53.000000";

    #[test]
    fn len_is_the_pic_width() {
        assert_eq!(Alnum::<10>::LEN, 10);
        assert_eq!(Alnum::<26>::LEN, 26);
        assert_eq!(Alnum::<350>::LEN, 350);
    }

    #[test]
    fn move_pads_with_spaces_and_truncates_on_the_right() {
        let cases: [(&str, &[u8; 4]); 5] = [
            ("ab", b"ab  "),
            ("abcd", b"abcd"),
            ("abcdef", b"abcd"),
            ("", b"    "),
            ("  x", b"  x "),
        ];
        for (src, expected) in cases {
            assert_eq!(Alnum::<4>::move_from(src).as_bytes(), expected, "{src:?}");
            assert_eq!(
                Alnum::<4>::move_from_bytes(src.as_bytes()).as_bytes(),
                expected,
                "{src:?}"
            );
        }
    }

    #[test]
    fn move_keeps_low_values_and_cuts_a_multi_byte_character() {
        assert_eq!(
            Alnum::<4>::move_from_bytes(&[0x00, b'a']).as_bytes(),
            &[0x00, b'a', b' ', b' ']
        );
        // "é" is 0xC3 0xA9 in UTF-8: the second byte falls past the receiver.
        assert_eq!(Alnum::<2>::move_from("aé").as_bytes(), &[b'a', 0xC3]);
    }

    #[test]
    fn spaces_and_default_are_all_spaces() {
        assert_eq!(Alnum::<5>::spaces().as_bytes(), b"     ");
        assert_eq!(Alnum::<5>::default(), Alnum::<5>::spaces());
        assert!(Alnum::<5>::default().is_blank());
    }

    #[test]
    fn from_bytes_requires_the_exact_length() {
        let item = Alnum::<26>::from_bytes(TIMESTAMP).expect("26 bytes fit X(26)");
        assert_eq!(item.as_bytes(), TIMESTAMP);
        assert_eq!(item, Alnum::from_array(*TIMESTAMP));

        for bytes in [&b"2022-06-10"[..], &[], &[b' '; 27]] {
            assert_eq!(
                Alnum::<26>::from_bytes(bytes),
                Err(NumericError::WrongLength {
                    expected: 26,
                    actual: bytes.len(),
                }),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn trailing_spaces_are_kept_by_the_views_and_removed_by_trimming() {
        let item = Alnum::<8>::move_from("Pat");
        assert_eq!(item.as_bytes(), b"Pat     ");
        assert_eq!(item.as_text(), "Pat     ");
        assert_eq!(item.to_string(), "Pat     ");
        assert_eq!(item.trimmed_bytes(), b"Pat");
        assert_eq!(item.trimmed(), "Pat");
    }

    #[test]
    fn trimming_removes_only_trailing_spaces() {
        let cases: [(&[u8; 5], &[u8]); 5] = [
            (b"     ", b""),
            (b" a b ", b" a b"),
            (b"ab\0\0\0", b"ab\0\0\0"),
            (b"ab\0  ", b"ab\0"),
            (b"ab\t  ", b"ab\t"),
        ];
        for (stored, trimmed) in cases {
            let item = Alnum::from_array(*stored);
            assert_eq!(item.trimmed_bytes(), trimmed, "{stored:?}");
            assert_eq!(item.trimmed().as_bytes(), trimmed, "{stored:?}");
        }
    }

    #[test]
    fn blank_means_all_spaces_or_all_low_values() {
        let cases: [(&[u8; 2], bool); 6] = [
            (b"  ", true),
            (&[0x00, 0x00], true),
            (&[0x20, 0x00], false),
            (&[0x00, 0x20], false),
            (b"a ", false),
            (b" a", false),
        ];
        for (stored, blank) in cases {
            assert_eq!(Alnum::from_array(*stored).is_blank(), blank, "{stored:?}");
        }
    }

    #[test]
    fn numeric_class_test_accepts_digits_only() {
        let cases: [(&[u8; 4], bool); 6] = [
            (b"0123", true),
            (b"9999", true),
            (b"12 3", false),
            (b"    ", false),
            (b"-123", false),
            (b"1.23", false),
        ];
        for (stored, numeric) in cases {
            assert_eq!(
                Alnum::from_array(*stored).is_numeric(),
                numeric,
                "{stored:?}"
            );
        }
    }

    #[test]
    fn prefix_is_bounded_by_the_width() {
        let item = Alnum::<26>::from_array(*TIMESTAMP);
        assert_eq!(item.prefix(10), b"2022-06-10");
        assert_eq!(item.prefix(0), b"");
        assert_eq!(item.prefix(26), TIMESTAMP);
        assert_eq!(item.prefix(100), TIMESTAMP);
    }

    #[test]
    fn reference_modification_is_one_based_and_bounded() {
        let item = Alnum::<26>::from_array(*TIMESTAMP);
        assert_eq!(item.ref_mod(1, 10), Some(&b"2022-06-10"[..]));
        assert_eq!(item.ref_mod(12, 8), Some(&b"19:27:53"[..]));
        assert_eq!(item.ref_mod(26, 1), Some(&b"0"[..]));
        assert_eq!(item.ref_mod(1, 26), Some(&TIMESTAMP[..]));
        for (start, len) in [(0, 1), (1, 0), (27, 1), (20, 8), (1, 27), (usize::MAX, 2)] {
            assert_eq!(item.ref_mod(start, len), None, "({start}:{len})");
        }
    }

    #[test]
    fn cobol_comparison_pads_the_shorter_operand_with_spaces() {
        let cases: [(&[u8; 3], &[u8], Ordering); 7] = [
            (b"Y  ", b"Y", Ordering::Equal),
            (b"A  ", b"B", Ordering::Less),
            (b"B  ", b"A", Ordering::Greater),
            (b"Y  ", b"Y     ", Ordering::Equal),
            (b"Y  ", b"Y    X", Ordering::Less),
            (b"Y\0 ", b"Y", Ordering::Less),
            (b"   ", b"", Ordering::Equal),
        ];
        for (stored, other, expected) in cases {
            assert_eq!(
                Alnum::from_array(*stored).cobol_cmp(other),
                expected,
                "{stored:?} vs {other:?}"
            );
        }
    }

    #[test]
    fn expiry_test_compares_the_date_with_the_timestamp_date_part() {
        // CBTRN02C: IF ACCT-EXPIRAION-DATE >= DALYTRAN-ORIG-TS (1:10).
        let orig_ts = Alnum::<26>::from_array(*TIMESTAMP);
        let date_part = orig_ts.ref_mod(1, 10).expect("(1:10) lies inside X(26)");
        let cases = [
            ("2025-01-01", Ordering::Greater),
            ("2022-06-10", Ordering::Equal),
            ("2022-06-09", Ordering::Less),
        ];
        for (expiry, expected) in cases {
            assert_eq!(
                Alnum::<10>::move_from(expiry).cobol_cmp(date_part),
                expected,
                "{expiry}"
            );
        }
    }

    #[test]
    fn ordering_is_byte_order() {
        let earlier = Alnum::<10>::move_from("2022-01-01");
        let later = Alnum::<10>::move_from("2022-12-31");
        assert!(earlier < later);
        assert_eq!(earlier.cmp(&later), Ordering::Less);
        // ASCII collation: digits sort before letters.
        assert!(Alnum::<1>::move_from("9") < Alnum::<1>::move_from("A"));
    }

    #[test]
    fn ascii_text_is_borrowed() {
        let item = Alnum::<4>::move_from("ab");
        assert!(matches!(item.as_text(), Cow::Borrowed("ab  ")));
        assert!(matches!(item.trimmed(), Cow::Borrowed("ab")));
    }

    #[test]
    fn non_ascii_text_maps_each_byte_to_its_latin_1_character() {
        let item = Alnum::<3>::from_array([b'e', 0xE9, b' ']);
        let text = item.as_text();
        assert!(matches!(text, Cow::Owned(_)));
        assert_eq!(text, "eé ");
        assert_eq!(text.chars().count(), 3);
        assert_eq!(item.trimmed(), "eé");
        // Valid UTF-8 is still read one byte per character, never as UTF-8.
        let utf8 = Alnum::<2>::move_from("é");
        assert_eq!(utf8.as_text(), "\u{c3}\u{a9}");
    }

    #[test]
    fn from_trimmed_restores_every_byte_value() {
        for byte in u8::MIN..=u8::MAX {
            let items = [
                Alnum::<3>::from_array([byte, b'x', byte]),
                Alnum::<3>::from_array([b'a', byte, b' ']),
                Alnum::<3>::from_array([byte; 3]),
            ];
            for item in items {
                assert_eq!(Alnum::from_trimmed(&item.trimmed()), Ok(item), "{item:?}");
            }
        }
        // Byte 0xE9 reads as "é" and is rebuilt as 0xE9, not as its UTF-8 lead
        // byte 0xC3, which only the UTF-8 `MOVE` stores.
        let acute = Alnum::<1>::from_array([0xE9]);
        assert_eq!(acute.trimmed(), "é");
        assert_eq!(Alnum::<1>::from_trimmed(&acute.trimmed()), Ok(acute));
        assert_eq!(Alnum::<1>::move_from("é").as_bytes(), &[0xC3]);
    }

    #[test]
    fn from_trimmed_counts_characters_and_pads_with_spaces() {
        let cases: [(&str, [u8; 3]); 6] = [
            ("ééé", [0xE9; 3]),
            ("\u{ff}", [0xFF, b' ', b' ']),
            ("AB", *b"AB "),
            (" x", *b" x "),
            ("", *b"   "),
            ("a\0", *b"a\0 "),
        ];
        for (text, expected) in cases {
            assert_eq!(
                Alnum::<3>::from_trimmed(text),
                Ok(Alnum::from_array(expected)),
                "{text:?}"
            );
        }
    }

    #[test]
    fn from_trimmed_rejects_text_the_item_cannot_hold() {
        let cases: [(&str, NumericError); 5] = [
            ("\u{100}", NumericError::NotLatin1),
            ("€", NumericError::NotLatin1),
            ("a😀", NumericError::NotLatin1),
            ("ABCD", NumericError::TooLong { max: 3, actual: 4 }),
            ("éééé", NumericError::TooLong { max: 3, actual: 4 }),
        ];
        for (text, expected) in cases {
            assert_eq!(Alnum::<3>::from_trimmed(text), Err(expected), "{text:?}");
        }
    }

    #[test]
    fn debug_and_display_show_the_full_text() {
        let item = Alnum::<4>::move_from("Y");
        assert_eq!(format!("{item:?}"), r#"Alnum<4>("Y   ")"#);
        assert_eq!(format!("{item}"), "Y   ");
        assert_eq!(
            format!("{:?}", Alnum::<2>::from_array([0x00, 0xE9])),
            r#"Alnum<2>("\0é")"#
        );
    }
}
