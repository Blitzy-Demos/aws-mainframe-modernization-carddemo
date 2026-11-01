//! Record codecs for the `CardDemo` copybook layouts, in ASCII and in EBCDIC
//! (IBM code page 037).
//!
//! Goal o4: each of the 626 records of the ASCII fixture corpus
//! (`app/data/ASCII/*.txt`) and each of the 626 records of its EBCDIC
//! counterpart (`app/data/EBCDIC/AWS.M2.CARDDEMO.*.PS`) decodes into its typed
//! record and re-encodes to its stored bytes, record by record and file by
//! file.
//!
//! # Codec model
//!
//! - [`charset`]: a stored record is translated as a whole into internal bytes
//!   (ASCII extended to ISO-8859-1) before decoding, and back after encoding.
//!   ASCII is the identity; EBCDIC is a bijective CP037 table.
//! - [`field`]: the display-format field codecs, which see internal bytes
//!   only: `X(n)` copied verbatim, `9(n)` as digits, and `S9(n)V9(s)` with its
//!   sign as a trailing overpunch on the last digit.
//! - [`framing`]: ASCII files are split into lines, each record keeping its
//!   stored length and terminator; EBCDIC files are fixed-length blocks of the
//!   record length, with no terminators.
//! - [`layouts`]: one [`Layout`] per copybook record, listing every field as a
//!   [`FieldSpec`] in copybook order. FILLER is kept as raw bytes, because the
//!   `DISCGRP` and `TCATBAL` fillers hold `0` characters, not spaces.
//!
//! No in-scope layout has a packed (`COMP-3`) or binary (`COMP`) field, so the
//! crate has no codec for either; a layout that needs one is an escalation for
//! `docs/parity/escalations.md`.
//!
//! # Fixture anomalies
//!
//! Anomalous fixtures are read as their bytes say and re-encoded as stored,
//! never repaired ("PIC clauses are authoritative"). Each is logged in
//! `docs/parity/escalations.md`.
//!
//! - **E-04, ASCII framing.** `trancatg.txt` holds 18 records of 60 bytes, each
//!   ending CR LF (1116 bytes). `tcatbal.txt` holds records 1–49 of 50 bytes
//!   ending CR LF and record 50 ending LF (2599 bytes). `trantype.txt` holds
//!   records 1–6 of 60 bytes ending CR LF and record 7 ending LF (433 bytes).
//!   `cardxref.txt` stores each of its 50 records as the first 36 of its 50
//!   bytes, then LF (1850 bytes); the absent FILLER decodes as 14 spaces, the
//!   content of the `0x40` (EBCDIC space) filler in the EBCDIC file. Every
//!   other file holds full-width records ending LF. The codec keeps each
//!   record's stored length and terminator, so every file re-encodes byte for
//!   byte.
//! - **E-05, ASCII and EBCDIC content.** Account record 49 holds
//!   `ACCT-ADDR-ZIP` `A000000000` in ASCII and `ZEROAPR` in EBCDIC.
//!   Disclosure-group record 34 (`DEFAULT`, `07`, `0001`) holds `DIS-INT-RATE`
//!   `00000{` (0.00) in ASCII and `00150{` (15.00) in EBCDIC. The `S9(04)V99`
//!   clause puts the implied point before the last two of the six digits
//!   `001500`, so the value is 15.00, as record 1's identical `00150{` is, and
//!   not the 1.50 a point one digit too far left would give. ASCII seeds the
//!   database and drives parity; EBCDIC serves only the o4 round trip.
//! - **E-06, blank group IDs.** Every `ACCT-GROUP-ID` (offset 112) is blank,
//!   while `A000000000` sits in `ACCT-ADDR-ZIP` (offset 102). Both are read at
//!   their PIC offsets, as stored.

pub mod charset;
pub mod error;
pub mod field;
pub mod framing;
pub mod layouts;

pub use charset::Charset;
pub use error::{CodecError, CodecErrorKind};
pub use field::{FieldKind, FieldSpec, PreservedFields, RawField, Signed};
pub use framing::{
    AsciiFrame, Framed, Terminator, decode_ascii_file, decode_ebcdic_file, encode_ascii_file,
    encode_ascii_lf, encode_ebcdic_file, pad, split_ascii,
};
pub use layouts::{
    AccountLayout, AccountRecord, CardLayout, CardRecord, CardXrefLayout, CardXrefRecord,
    CustomerLayout, CustomerRecord, DailyTranLayout, DailyTranRecord, DisclosureGroupLayout,
    DisclosureGroupRecord, RejectRecord, RejectRecordLayout, TranCatBalanceLayout,
    TranCatBalanceRecord, TranCategoryFileRecord, TranCategoryLayout, TranTypeFileRecord,
    TranTypeLayout, TransactionLayout, TransactionRecord, UserSecurityLayout, UserSecurityRecord,
};

/// A copybook record layout: its length, its fields and its typed record, with
/// the decoder and encoder between stored bytes and that record.
///
/// Every layout keeps one contract:
///
/// - [`Layout::decode`] takes exactly [`Layout::LEN`] stored bytes, translates
///   them with [`Charset::to_internal`], then reads each field of
///   [`Layout::FIELDS`] with the [`field`] readers.
/// - [`Layout::encode`] builds a `LEN`-byte internal buffer of spaces, writes
///   every field (FILLER from the raw bytes the record holds), restores the
///   [`PreservedFields`] the decoder recorded for values its typed fields
///   cannot reproduce, then translates the buffer with
///   [`Charset::from_internal`].
///
/// Hence `encode(&decode(b, cs)?, cs) == b` for every fixture record `b` in its
/// character set `cs`: goal o4 at record level, which [`framing`] extends to
/// whole files.
///
/// Layouts are unit types and the trait has no `self` methods, so generic code
/// such as [`decode_ascii_file`] names the layout as a type parameter.
pub trait Layout {
    /// The record length in bytes: the copybook `RECLN`.
    const LEN: usize;

    /// The copybook name, for example `"CVACT01Y"`. It labels every
    /// [`CodecError`] the layout reports.
    const NAME: &'static str;

    /// Every field in copybook order, FILLER included: contiguous from offset 0
    /// and summing to [`Layout::LEN`], as [`field::fields_are_contiguous`]
    /// checks.
    const FIELDS: &'static [FieldSpec];

    /// The typed record.
    type Record;

    /// Decodes one stored record in `charset`.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes.len() == LEN`.
    /// - The field errors of the [`field`] readers, and
    ///   [`CodecErrorKind::Value`] when a domain constructor rejects a decoded
    ///   value, each located at its field.
    fn decode(bytes: &[u8], charset: Charset) -> Result<Self::Record, CodecError>;

    /// Encodes `record` as stored bytes in `charset`: always exactly `LEN`
    /// bytes.
    #[must_use]
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8>;
}

#[cfg(test)]
mod tests {
    use super::{
        AccountLayout, CardLayout, CardXrefLayout, CustomerLayout, DailyTranLayout,
        DisclosureGroupLayout, Layout, RejectRecordLayout, TranCatBalanceLayout,
        TranCategoryLayout, TranTypeLayout, TransactionLayout, UserSecurityLayout, field,
    };

    /// Asserts that `L` is the layout of `name`, `len` bytes long, and that its
    /// fields tile exactly those bytes.
    fn check<L: Layout>(name: &str, len: usize) {
        assert_eq!((L::NAME, L::LEN), (name, len), "layout {name}");
        assert!(
            field::fields_are_contiguous(L::FIELDS, L::LEN),
            "{} fields do not tile its {} bytes",
            L::NAME,
            L::LEN
        );
    }

    #[test]
    fn every_layout_has_its_copybook_name_and_length_and_tiles_its_fields() {
        check::<AccountLayout>("CVACT01Y", 300);
        check::<CardLayout>("CVACT02Y", 150);
        check::<CardXrefLayout>("CVACT03Y", 50);
        check::<CustomerLayout>("CVCUS01Y", 500);
        check::<DailyTranLayout>("CVTRA06Y", 350);
        check::<TransactionLayout>("CVTRA05Y", 350);
        check::<TranCatBalanceLayout>("CVTRA01Y", 50);
        check::<DisclosureGroupLayout>("CVTRA02Y", 50);
        check::<TranTypeLayout>("CVTRA03Y", 60);
        check::<TranCategoryLayout>("CVTRA04Y", 60);
        check::<UserSecurityLayout>("CSUSR01Y", 80);
        check::<RejectRecordLayout>("CBTRN02C-REJECT-RECORD", 430);
    }
}
