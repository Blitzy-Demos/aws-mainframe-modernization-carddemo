//! Cardholder data as types that cannot leak: card numbers, SSNs, CVVs and passwords
//! (Cardholder Data Redaction, AAP 0.3.4.8).
//!
//! Every protected copybook field (class row `PIC-024` of `modern/audit/copybooks.md`) is held in
//! one of these wrappers around its picture type, never as a bare [`PicX`] or [`PicU`], whose
//! `Debug` shows the value. `Debug` and `Display` of every wrapper print only the redacted form,
//! so interpolating one into a log line, a tracing field, an error or a message cannot disclose
//! it, and `Serialize`, where it exists, emits that same redacted form:
//!
//! | Type | Picture | Fields (copybook rows) | `Debug`, `Display`, `Serialize` |
//! |---|---|---|---|
//! | [`CardNumber`] | `PIC X(16)` | `CARD-NUM` (CVACT02Y-001), `XREF-CARD-NUM` (CVACT03Y-001), `TRAN-CARD-NUM` (CVTRA05Y-011), `DALYTRAN-CARD-NUM` (CVTRA06Y-011), `PA-CARD-NUM` (CIPAUDTY-005), `PA-RQ-CARD-NUM` (CCPAURQY-003), `PA-RL-CARD-NUM` (CCPAURLY-001) | `************5740`: twelve `*` and the last four characters |
//! | [`Ssn`] | `PIC 9(09)` | `CUST-SSN` (CVCUS01Y-013) | `***-**-1234`: the last four digits of the zero-padded value |
//! | [`Cvv`] | `PIC 9(03)` | `CARD-CVV-CD` (CVACT02Y-003) | `***`; no `Serialize` |
//! | [`Password`] | `PIC X(08)` | `SEC-USR-PWD` (CSUSR01Y-004) | `********`; no `Serialize` |
//!
//! The COBOL shows card numbers and SSNs in full on screens, in the CP00 reply and in the `CSSL`
//! event key; showing only the redacted forms is the departure escalation `E32` records. The
//! redacted card number keeps the field's 16-character width, so the CP00 reply field and the
//! `ERR-EVENT-KEY PIC X(20)` of the error log (CCPAUERY-011) keep their layouts.
//!
//! # Raw access
//!
//! `expose()` on each wrapper is the only path to the raw value. Its callers are the codecs
//! (codec A record encoders, codec B row binds), the CP00 cross-reference lookup key and
//! `crate::password`. Never format, log or serialise what it returns. The database keeps the
//! full value, except that a password is stored only as its argon2id hash (`E25`).
//!
//! [`DisclosedCardNumber`] is the single full-value serialiser in the workspace. Only
//! [`CardNumber::disclose`] builds one, for the card-detail response of endpoint 8.
//!
//! # API schemas
//!
//! This crate has no `utoipa` dependency. The `api` DTOs describe these fields as `String`
//! schemas whose examples are redacted values such as `************5740` and `***-**-1234`.
//!
//! # Parsing
//!
//! No type here can be deserialised. Requests carry raw text, which the online rules turn
//! into values with [`CardNumber::parse`], [`Ssn::parse`] and [`Password::from_input`] after
//! their own edits, so that the COBOL edit messages survive (AAP 0.3.4.1). Each parser is total:
//! for any text it returns a value or a [`SensitiveError`] and never panics, and no error carries
//! any part of the text it rejected (Total Parsers Over Untrusted Input).

use core::fmt;

use copybook::pic::{PicU, PicX};
use serde::{Serialize, Serializer};

/// Width of a `PIC X(16)` card-number field.
const CARD_WIDTH: usize = 16;

/// Number of trailing characters a redacted card number or SSN keeps.
const LAST_DIGITS: usize = 4;

/// Digits of `CUST-SSN PIC 9(09)`.
const SSN_DIGITS: u32 = 9;

/// Digits of `CARD-CVV-CD PIC 9(03)`.
const CVV_DIGITS: u32 = 3;

/// Width of `SEC-USR-PWD PIC X(08)`.
const PASSWORD_WIDTH: usize = 8;

/// The byte that replaces each hidden position of a redacted card number.
const MASK: u8 = b'*';

/// The hidden area and group numbers of a redacted SSN, which its last four digits follow.
const SSN_MASK: &str = "***-**-";

/// Length of a redacted SSN: [`SSN_MASK`] and four digits.
const SSN_REDACTED_LEN: usize = 11;

/// What `Debug` and `Display` print for any CVV.
const CVV_REDACTED: &str = "***";

/// What `Debug` and `Display` print for any password.
const PASSWORD_REDACTED: &str = "********";

/// Why external text is not a protected value.
///
/// The message states the rule only. It never repeats the text, its length or any character of
/// it, because the rejected text may itself be a card number, an SSN or a password.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SensitiveError {
    /// A card number is not exactly 16 ASCII digits: the CP00 request rule (CCPAURQY-003,
    /// escalation `E31`), and every online edit that requires a 16-digit number.
    NotSixteenDigits,
    /// An SSN is not exactly 9 ASCII digits (CVCUS01Y-013).
    NotNineDigits,
    /// The text is longer than the field it fills.
    TooLong {
        /// The field width in bytes.
        max: usize,
    },
}

impl fmt::Display for SensitiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSixteenDigits => f.write_str("card number must be 16 ASCII digits"),
            Self::NotNineDigits => f.write_str("SSN must be 9 ASCII digits"),
            Self::TooLong { max } => write!(f, "value must be at most {max} bytes"),
        }
    }
}

impl std::error::Error for SensitiveError {}

/// The last four bytes of a card-number field, read by array destructuring, which cannot fail.
fn last_four(bytes: &[u8; CARD_WIDTH]) -> [u8; LAST_DIGITS] {
    let [.., thirteenth, fourteenth, fifteenth, sixteenth] = *bytes;
    [thirteenth, fourteenth, fifteenth, sixteenth]
}

/// Twelve [`MASK`] bytes followed by the last four bytes of `bytes` (PIC-024).
fn redact_card(bytes: &[u8; CARD_WIDTH]) -> [u8; CARD_WIDTH] {
    let [thirteenth, fourteenth, fifteenth, sixteenth] = last_four(bytes);
    [
        MASK, MASK, MASK, MASK, MASK, MASK, MASK, MASK, MASK, MASK, MASK, MASK, thirteenth,
        fourteenth, fifteenth, sixteenth,
    ]
}

/// `bytes` as text, one character per byte. `char::from` is total over every byte, so a stored
/// field that is not ASCII still renders without loss or failure.
fn text_of(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

/// The ASCII digit of the units position of `value`.
fn units_digit(value: u64) -> u8 {
    // TRUNCATION: `value % 10` is below 10, so the conversion into `u8` is exact; the fallback is
    // unreachable and only keeps this function free of a panic path.
    let digit = u8::try_from(value % 10).unwrap_or(0);
    b'0' + digit
}

/// A card number, `PIC X(16)` (CVACT02Y-001, CVACT03Y-001, CVTRA05Y-011, CVTRA06Y-011,
/// CIPAUDTY-005, CCPAURQY-003, CCPAURLY-001).
///
/// `Debug`, `Display` and `Serialize` emit only the redacted form, twelve `*` followed by the
/// field's last four characters, 16 characters in all (PIC-024, `E32`). Values compare and hash
/// bytewise, the ASCII collating sequence, so statements and reports order by card number and
/// batch jobs key maps by it without exposing the value. Card list rows and page keys carry the
/// `cards.card_ref` handle (CVACT02Y-008) rather than this value.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CardNumber(PicX<CARD_WIDTH>);

impl CardNumber {
    /// The card number held in a stored or decoded field, its 16 bytes kept verbatim.
    #[must_use]
    pub const fn new(raw: PicX<CARD_WIDTH>) -> Self {
        Self(raw)
    }

    /// The card number `text` names, which must be exactly 16 ASCII digits, `0` to `9`, with no
    /// sign, space or other character. This is the CP00 request rule (CCPAURQY-003, `E31`) and
    /// the rule every online edit applies once it requires a 16-digit number.
    ///
    /// # Errors
    ///
    /// [`SensitiveError::NotSixteenDigits`] for any other text, empty text included.
    pub fn parse(text: &str) -> Result<Self, SensitiveError> {
        let bytes = <[u8; CARD_WIDTH]>::try_from(text.as_bytes())
            .map_err(|_| SensitiveError::NotSixteenDigits)?;
        if bytes.iter().all(u8::is_ascii_digit) {
            Ok(Self(PicX::from_array(bytes)))
        } else {
            Err(SensitiveError::NotSixteenDigits)
        }
    }

    /// The raw field, for codecs, SQL binds and the CP00 cross-reference lookup key only.
    ///
    /// Never log, format or serialise the result: it is the full card number.
    #[must_use]
    pub const fn expose(&self) -> &PicX<CARD_WIDTH> {
        &self.0
    }

    /// The field's last four bytes, which the redacted form shows.
    #[must_use]
    pub fn last4(&self) -> [u8; LAST_DIGITS] {
        last_four(self.0.as_bytes())
    }

    /// The redacted form as the 16 bytes a fixed-width writer emits: the CP00 reply field
    /// (CCPAURLY-001) and the `CSSL` event key (CCPAUERY-011).
    #[must_use]
    pub fn redacted_bytes(&self) -> [u8; CARD_WIDTH] {
        redact_card(self.0.as_bytes())
    }

    /// The redacted form as text, such as `************5740`.
    #[must_use]
    pub fn redacted(&self) -> String {
        text_of(&self.redacted_bytes())
    }

    /// The full card number in its one serialisable form, for the card-detail response.
    ///
    /// Called only by `api::services::cocrdslc`, and only when the verified token's `usr_type`
    /// is `A`; every other caller and every other user type gets the redacted form.
    #[must_use]
    pub fn disclose(&self) -> DisclosedCardNumber {
        DisclosedCardNumber(self.0.clone())
    }
}

impl TryFrom<&str> for CardNumber {
    type Error = SensitiveError;

    /// [`CardNumber::parse`].
    fn try_from(text: &str) -> Result<Self, Self::Error> {
        Self::parse(text)
    }
}

impl fmt::Debug for CardNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

/// The redacted form, honouring width and alignment flags.
impl fmt::Display for CardNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.redacted())
    }
}

/// The redacted form as a string.
impl Serialize for CardNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.redacted())
    }
}

/// A card number cleared for disclosure in full.
///
/// Its `Serialize` emits all 16 characters as a string: the only full-value serialiser in the
/// workspace, used only by the card-detail response of endpoint 8 for an admin token (`E32`).
/// `Debug` and `Display` still emit the redacted form, so a disclosed number that reaches a log
/// line or an error stays hidden there. Only [`CardNumber::disclose`] constructs one.
#[derive(Clone, PartialEq, Eq)]
pub struct DisclosedCardNumber(PicX<CARD_WIDTH>);

impl fmt::Debug for DisclosedCardNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&text_of(&redact_card(self.0.as_bytes())))
    }
}

/// The redacted form, honouring width and alignment flags.
impl fmt::Display for DisclosedCardNumber {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&text_of(&redact_card(self.0.as_bytes())))
    }
}

/// All 16 characters of the field, padding included, as a string.
impl Serialize for DisclosedCardNumber {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&text_of(self.0.as_bytes()))
    }
}

/// A social security number, `CUST-SSN PIC 9(09)` (CVCUS01Y-013).
///
/// `Debug`, `Display` and `Serialize` emit only the redacted form, `***-**-` followed by the
/// last four digits of the zero-padded 9-digit value, 11 characters in all (PIC-024, `E32`).
/// COACTUPC's changed-by-another-user check compares SSNs through [`Ssn::last4`], because the
/// snapshot a client echoes holds only the redacted form (`E32`).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Ssn(PicU<SSN_DIGITS>);

impl Ssn {
    /// The SSN held in a stored or decoded field.
    #[must_use]
    pub const fn new(raw: PicU<SSN_DIGITS>) -> Self {
        Self(raw)
    }

    /// The SSN `text` names, which must be exactly 9 ASCII digits, leading zeros included, with
    /// no dash, sign, space or other character.
    ///
    /// # Errors
    ///
    /// [`SensitiveError::NotNineDigits`] for any other text, empty text included.
    pub fn parse(text: &str) -> Result<Self, SensitiveError> {
        PicU::decode(text.as_bytes())
            .map(Self)
            .map_err(|_| SensitiveError::NotNineDigits)
    }

    /// The raw field, for codecs and SQL binds only.
    ///
    /// Never log, format or serialise the result: it is the full SSN.
    #[must_use]
    pub const fn expose(&self) -> &PicU<SSN_DIGITS> {
        &self.0
    }

    /// The last four digits of the zero-padded 9-digit value, as ASCII digits.
    #[must_use]
    pub fn last4(&self) -> [u8; LAST_DIGITS] {
        let low = self.0.get() % 10_000;
        [
            units_digit(low / 1_000),
            units_digit(low / 100),
            units_digit(low / 10),
            units_digit(low),
        ]
    }

    /// The redacted form, such as `***-**-1234`.
    #[must_use]
    pub fn redacted(&self) -> String {
        let mut text = String::with_capacity(SSN_REDACTED_LEN);
        text.push_str(SSN_MASK);
        text.push_str(&text_of(&self.last4()));
        text
    }
}

impl TryFrom<&str> for Ssn {
    type Error = SensitiveError;

    /// [`Ssn::parse`].
    fn try_from(text: &str) -> Result<Self, Self::Error> {
        Self::parse(text)
    }
}

impl fmt::Debug for Ssn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.redacted())
    }
}

/// The redacted form, honouring width and alignment flags.
impl fmt::Display for Ssn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.redacted())
    }
}

/// The redacted form as a string.
impl Serialize for Ssn {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.redacted())
    }
}

/// A card verification value, `CARD-CVV-CD PIC 9(03)` (CVACT02Y-003).
///
/// `Debug` and `Display` print `***` whatever the digits. It has no `Serialize`, so no response,
/// DTO, schema property or broker message can carry it (PIC-024).
#[derive(Clone, PartialEq, Eq)]
pub struct Cvv(PicU<CVV_DIGITS>);

impl Cvv {
    /// The CVV held in a stored or decoded field.
    #[must_use]
    pub const fn new(raw: PicU<CVV_DIGITS>) -> Self {
        Self(raw)
    }

    /// The raw field, for codecs and SQL binds only.
    ///
    /// Never log, format or serialise the result: it is the CVV itself.
    #[must_use]
    pub const fn expose(&self) -> &PicU<CVV_DIGITS> {
        &self.0
    }
}

impl fmt::Debug for Cvv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(CVV_REDACTED)
    }
}

/// `***`, honouring width and alignment flags.
impl fmt::Display for Cvv {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(CVV_REDACTED)
    }
}

/// A password, `SEC-USR-PWD PIC X(08)` (CSUSR01Y-004).
///
/// `Debug` and `Display` print `********` whatever the bytes. It has no `Serialize`, and the
/// store keeps only its argon2id hash over the 8 bytes [`Password::expose`] returns, never the
/// bytes themselves (`E25`).
#[derive(Clone, PartialEq, Eq)]
pub struct Password(PicX<PASSWORD_WIDTH>);

impl Password {
    /// The password held in a decoded field, such as the in-stream `USRSEC` records.
    #[must_use]
    pub const fn new(raw: PicX<PASSWORD_WIDTH>) -> Self {
        Self(raw)
    }

    /// The password exactly as typed, left-justified and padded with spaces to 8 bytes.
    ///
    /// No case folding happens here: COUSR01C stores the password as typed, and sign-on's
    /// upper-casing belongs to `online::cosgn00c`, so a lower-case password never verifies
    /// (`E26`). Blank input is the 8-space password; rejecting it is the online edits' rule.
    ///
    /// # Errors
    ///
    /// [`SensitiveError::TooLong`] with `max` 8 when `text` is longer than 8 bytes.
    pub fn from_input(text: &str) -> Result<Self, SensitiveError> {
        let bytes = text.as_bytes();
        if bytes.len() > PASSWORD_WIDTH {
            Err(SensitiveError::TooLong {
                max: PASSWORD_WIDTH,
            })
        } else {
            Ok(Self(PicX::moved_from(bytes)))
        }
    }

    /// The 8 stored bytes, the input to `crate::password` hashing and verification and to the
    /// `USRSEC` record codec only.
    ///
    /// Never log, format or serialise the result: it is the password itself.
    #[must_use]
    pub const fn expose(&self) -> &[u8; PASSWORD_WIDTH] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(PASSWORD_REDACTED)
    }
}

/// `********`, honouring width and alignment flags.
impl fmt::Display for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(PASSWORD_REDACTED)
    }
}

#[cfg(test)]
mod tests {
    use core::fmt;
    use std::collections::HashSet;
    use std::error::Error;

    use copybook::pic::{PicU, PicX};
    use serde::Serialize;

    use super::{CardNumber, Cvv, DisclosedCardNumber, Password, SensitiveError, Ssn};

    /// The first cross-reference record's card number (`cardxref.txt:1`).
    const CARD: &str = "0500024453765740";

    /// The redacted form of [`CARD`].
    const CARD_REDACTED: &str = "************5740";

    /// What a value hands its serializer. serde implements `Serializer` for `fmt::Formatter`,
    /// writing the text of `serialize_str` and of the other scalar methods, so this captures the
    /// serialised value without a JSON dependency.
    struct Serialized<'v, T>(&'v T);

    impl<T: Serialize> fmt::Display for Serialized<'_, T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.serialize(f)
        }
    }

    fn serialized<T: Serialize>(value: &T) -> String {
        Serialized(value).to_string()
    }

    #[test]
    fn card_number_shows_only_its_last_four_digits() -> Result<(), SensitiveError> {
        let card = CardNumber::parse(CARD)?;
        assert_eq!(format!("{card:?}"), CARD_REDACTED);
        assert_eq!(format!("{card}"), CARD_REDACTED);
        assert_eq!(serialized(&card), CARD_REDACTED);
        assert_eq!(card.redacted(), CARD_REDACTED);
        assert_eq!(&card.redacted_bytes(), CARD_REDACTED.as_bytes());
        assert_eq!(card.last4(), *b"5740");
        assert_eq!(
            format!("{card:>18}|{card:<17}|"),
            format!("  {CARD_REDACTED}|{CARD_REDACTED} |")
        );
        assert_eq!(card.expose().as_bytes(), CARD.as_bytes());
        Ok(())
    }

    #[test]
    fn disclosure_serialises_in_full_and_still_formats_redacted() -> Result<(), SensitiveError> {
        let disclosed: DisclosedCardNumber = CardNumber::parse(CARD)?.disclose();
        assert_eq!(serialized(&disclosed), CARD);
        assert_eq!(format!("{disclosed:?}"), CARD_REDACTED);
        assert_eq!(format!("{disclosed}"), CARD_REDACTED);
        assert_eq!(format!("{disclosed:>17}"), format!(" {CARD_REDACTED}"));
        assert_eq!(disclosed.clone(), disclosed);
        Ok(())
    }

    #[test]
    fn stored_card_bytes_are_kept_verbatim() {
        let stored = CardNumber::new(PicX::moved_from(b"4000 12"));
        assert_eq!(stored.expose().as_bytes(), b"4000 12         ");
        assert_eq!(stored.last4(), *b"    ");
        assert_eq!(stored.redacted(), "************    ");
        let disclosed = stored.disclose();
        assert_eq!(serialized(&disclosed), "4000 12         ");
        assert_eq!(format!("{disclosed:?}"), "************    ");
    }

    #[test]
    fn card_parse_accepts_exactly_sixteen_ascii_digits() -> Result<(), SensitiveError> {
        assert_eq!(CardNumber::try_from(CARD)?, CardNumber::parse(CARD)?);
        let rejected = [
            "",
            "050002445376574",
            "05000244537657401",
            "050002445376574A",
            "0500 24453765740",
            " 500024453765740",
            "050002445376574 ",
            "+500024453765740",
            "05000244537657\u{0664}",
            "\u{ff10}500024453765740",
        ];
        for text in rejected {
            assert_eq!(
                CardNumber::parse(text),
                Err(SensitiveError::NotSixteenDigits)
            );
            assert_eq!(
                CardNumber::try_from(text),
                Err(SensitiveError::NotSixteenDigits)
            );
        }
        Ok(())
    }

    #[test]
    fn card_numbers_order_and_hash_by_their_bytes() -> Result<(), SensitiveError> {
        let low = CardNumber::parse("0000000000000001")?;
        let high = CardNumber::parse("0000000000000002")?;
        assert!(low < high);
        assert_eq!(low.cmp(&low.clone()), core::cmp::Ordering::Equal);
        let keys: HashSet<CardNumber> = [low.clone(), high, low].into_iter().collect();
        assert_eq!(keys.len(), 2);
        Ok(())
    }

    #[test]
    fn ssn_shows_only_its_last_four_digits() -> Result<(), Box<dyn Error>> {
        let ssn = Ssn::parse("123456789")?;
        assert_eq!(format!("{ssn:?}"), "***-**-6789");
        assert_eq!(format!("{ssn}"), "***-**-6789");
        assert_eq!(format!("{ssn:<12}|"), "***-**-6789 |");
        assert_eq!(serialized(&ssn), "***-**-6789");
        assert_eq!(ssn.last4(), *b"6789");
        assert_eq!(ssn.expose().get(), 123_456_789);
        assert_eq!(Ssn::try_from("123456789")?, ssn);
        let keys: HashSet<Ssn> = [ssn, ssn].into_iter().collect();
        assert_eq!(keys.len(), 1);
        Ok(())
    }

    #[test]
    fn ssn_last_four_digits_keep_their_leading_zeros() -> Result<(), Box<dyn Error>> {
        let cases = [
            ("000001234", 1_234, "***-**-1234"),
            ("000000056", 56, "***-**-0056"),
            ("000000000", 0, "***-**-0000"),
            ("999990001", 999_990_001, "***-**-0001"),
        ];
        for (text, value, redacted) in cases {
            let parsed = Ssn::parse(text)?;
            assert_eq!(parsed, Ssn::new(PicU::new(value)?));
            assert_eq!(parsed.redacted(), redacted);
            assert_eq!(serialized(&parsed), redacted);
        }
        Ok(())
    }

    #[test]
    fn ssn_parse_accepts_exactly_nine_ascii_digits() {
        let rejected = [
            "",
            "12345678",
            "1234567890",
            "123-45-6789",
            "12345678A",
            " 12345678",
            "-12345678",
            "12345678\u{0669}",
        ];
        for text in rejected {
            assert_eq!(Ssn::parse(text), Err(SensitiveError::NotNineDigits));
            assert_eq!(Ssn::try_from(text), Err(SensitiveError::NotNineDigits));
        }
    }

    #[test]
    fn cvv_never_formats_its_digits() -> Result<(), Box<dyn Error>> {
        let cvv = Cvv::new(PicU::new(987)?);
        assert_eq!(format!("{cvv:?}"), "***");
        assert_eq!(format!("{cvv}"), "***");
        assert_eq!(format!("{cvv:>4}"), " ***");
        assert_eq!(cvv.expose().get(), 987);
        assert_eq!(cvv.clone(), cvv);
        Ok(())
    }

    #[test]
    fn password_is_padded_as_typed_and_never_formatted() -> Result<(), SensitiveError> {
        let typed = Password::from_input("abc")?;
        assert_eq!(typed.expose(), b"abc     ");
        assert_eq!(typed, Password::new(PicX::moved_from(b"abc")));
        for shown in [format!("{typed:?}"), format!("{typed}")] {
            assert_eq!(shown, "********");
            assert!(!shown.contains(['a', 'b', 'c']));
        }
        assert_eq!(format!("{typed:>9}"), " ********");
        assert_eq!(Password::from_input("Passw0rd")?.expose(), b"Passw0rd");
        assert_eq!(Password::from_input("")?.expose(), b"        ");
        assert_ne!(
            Password::from_input("password")?,
            Password::from_input("PASSWORD")?
        );
        Ok(())
    }

    #[test]
    fn password_longer_than_eight_bytes_is_rejected() {
        // Nine bytes in seven characters: the width counts bytes, as the 8-byte field does.
        for text in ["123456789", "pa\u{df}w\u{f6}rt"] {
            assert_eq!(
                Password::from_input(text),
                Err(SensitiveError::TooLong { max: 8 })
            );
        }
    }

    #[test]
    fn errors_state_the_rule_and_never_the_input() {
        let cases = [
            (
                SensitiveError::NotSixteenDigits,
                "card number must be 16 ASCII digits",
            ),
            (SensitiveError::NotNineDigits, "SSN must be 9 ASCII digits"),
            (
                SensitiveError::TooLong { max: 8 },
                "value must be at most 8 bytes",
            ),
        ];
        for (error, message) in cases {
            assert_eq!(error.to_string(), message);
            let source: &dyn Error = &error;
            assert!(source.source().is_none());
        }
    }
}
