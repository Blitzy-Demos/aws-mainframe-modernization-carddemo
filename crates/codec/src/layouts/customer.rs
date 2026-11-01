//! `CVCUS01Y` `CUSTOMER-RECORD`: the 500-byte record of the VSAM `CUSTDAT`
//! customer master [app/cpy/CVCUS01Y.cpy:L4-23], RECLN 500
//! [app/cpy/CVCUS01Y.cpy:L2].
//!
//! `CUSTFILE` loads `CUSTDAT` by `REPRO` from `AWS.M2.CARDDEMO.CUSTDATA.PS`
//! [app/jcl/CUSTFILE.jcl:L66-71]. Its ASCII fixture `app/data/ASCII/custdata.txt`
//! holds 50 records of 500 bytes, each ending LF; the EBCDIC counterpart
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.CUSTDATA.PS` holds the same 50 records as
//! fixed 500-byte CP037 blocks. `batch seed` decodes each record with
//! [`CustomerLayout`] into the `customers` table (P-CVCUS01Y-020).
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `CUST-ID` | `9(09)` | 0 | 9 | P-CVCUS01Y-001 | `CustomerId` |
//! | `CUST-FIRST-NAME` | `X(25)` | 9 | 25 | P-CVCUS01Y-002 | `Alnum<25>` |
//! | `CUST-MIDDLE-NAME` | `X(25)` | 34 | 25 | P-CVCUS01Y-003 | `Alnum<25>` |
//! | `CUST-LAST-NAME` | `X(25)` | 59 | 25 | P-CVCUS01Y-004 | `Alnum<25>` |
//! | `CUST-ADDR-LINE-1` | `X(50)` | 84 | 50 | P-CVCUS01Y-005 | `Alnum<50>` |
//! | `CUST-ADDR-LINE-2` | `X(50)` | 134 | 50 | P-CVCUS01Y-006 | `Alnum<50>` |
//! | `CUST-ADDR-LINE-3` | `X(50)` | 184 | 50 | P-CVCUS01Y-007 | `Alnum<50>` |
//! | `CUST-ADDR-STATE-CD` | `X(02)` | 234 | 2 | P-CVCUS01Y-008 | `Alnum<2>` |
//! | `CUST-ADDR-COUNTRY-CD` | `X(03)` | 236 | 3 | P-CVCUS01Y-009 | `Alnum<3>` |
//! | `CUST-ADDR-ZIP` | `X(10)` | 239 | 10 | P-CVCUS01Y-010 | `Alnum<10>` |
//! | `CUST-PHONE-NUM-1` | `X(15)` | 249 | 15 | P-CVCUS01Y-011 | `Alnum<15>` |
//! | `CUST-PHONE-NUM-2` | `X(15)` | 264 | 15 | P-CVCUS01Y-012 | `Alnum<15>` |
//! | `CUST-SSN` | `9(09)` | 279 | 9 | P-CVCUS01Y-013 | `CustSsn` |
//! | `CUST-GOVT-ISSUED-ID` | `X(20)` | 288 | 20 | P-CVCUS01Y-014 | `Alnum<20>` |
//! | `CUST-DOB-YYYY-MM-DD` | `X(10)` | 308 | 10 | P-CVCUS01Y-015 | `Alnum<10>` |
//! | `CUST-EFT-ACCOUNT-ID` | `X(10)` | 318 | 10 | P-CVCUS01Y-016 | `Alnum<10>` |
//! | `CUST-PRI-CARD-HOLDER-IND` | `X(01)` | 328 | 1 | P-CVCUS01Y-017 | `Alnum<1>` |
//! | `CUST-FICO-CREDIT-SCORE` | `9(03)` | 329 | 3 | P-CVCUS01Y-018 | `FicoScore` |
//! | `FILLER` | `X(168)` | 332 | 168 | P-CVCUS01Y-019 | `Alnum<168>`, raw |
//!
//! # Text fields that hold digits
//!
//! `CUST-GOVT-ISSUED-ID` and `CUST-EFT-ACCOUNT-ID` hold only digits in every
//! fixture record, but their PIC is `X`: they stay [`Alnum`], are never parsed
//! as numbers, and any byte they hold re-encodes as stored ("PIC clauses are
//! authoritative"). `CUST-DOB-YYYY-MM-DD` likewise stays text.
//!
//! # The social security number
//!
//! P-CVCUS01Y-013 names the Rust type of `CUST-SSN` `Ssn`; the domain declares
//! that `9(09)` class as [`CustSsn`], with the row's width and SQL check. Its
//! `Debug` is redacted, so the derived `Debug` of [`CustomerRecord`] never shows
//! the number.
//!
//! The record has no signed field, so no value needs `PreservedFields`: every
//! decoded record re-encodes to its stored bytes from its typed fields alone.

use carddemo_domain::entities::Customer;
use carddemo_domain::numeric::{Alnum, CustSsn, CustomerId, FicoScore, NumericError};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec};

/// `CVCUS01Y` `CUST-ID PIC 9(09)`, the `CUSTDAT` key — P-CVCUS01Y-001
/// [app/cpy/CVCUS01Y.cpy:L5].
pub const CUST_ID: FieldSpec = FieldSpec::new("CUST-ID", 0, 9, FieldKind::Unsigned);

/// `CVCUS01Y` `CUST-FIRST-NAME PIC X(25)` — P-CVCUS01Y-002
/// [app/cpy/CVCUS01Y.cpy:L6].
pub const CUST_FIRST_NAME: FieldSpec = FieldSpec::new("CUST-FIRST-NAME", 9, 25, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-MIDDLE-NAME PIC X(25)` — P-CVCUS01Y-003
/// [app/cpy/CVCUS01Y.cpy:L7].
pub const CUST_MIDDLE_NAME: FieldSpec =
    FieldSpec::new("CUST-MIDDLE-NAME", 34, 25, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-LAST-NAME PIC X(25)` — P-CVCUS01Y-004
/// [app/cpy/CVCUS01Y.cpy:L8].
pub const CUST_LAST_NAME: FieldSpec = FieldSpec::new("CUST-LAST-NAME", 59, 25, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-ADDR-LINE-1 PIC X(50)` — P-CVCUS01Y-005
/// [app/cpy/CVCUS01Y.cpy:L9].
pub const CUST_ADDR_LINE_1: FieldSpec =
    FieldSpec::new("CUST-ADDR-LINE-1", 84, 50, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-ADDR-LINE-2 PIC X(50)` — P-CVCUS01Y-006
/// [app/cpy/CVCUS01Y.cpy:L10].
pub const CUST_ADDR_LINE_2: FieldSpec =
    FieldSpec::new("CUST-ADDR-LINE-2", 134, 50, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-ADDR-LINE-3 PIC X(50)` — P-CVCUS01Y-007
/// [app/cpy/CVCUS01Y.cpy:L11].
pub const CUST_ADDR_LINE_3: FieldSpec =
    FieldSpec::new("CUST-ADDR-LINE-3", 184, 50, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-ADDR-STATE-CD PIC X(02)` — P-CVCUS01Y-008
/// [app/cpy/CVCUS01Y.cpy:L12].
pub const CUST_ADDR_STATE_CD: FieldSpec =
    FieldSpec::new("CUST-ADDR-STATE-CD", 234, 2, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-ADDR-COUNTRY-CD PIC X(03)` — P-CVCUS01Y-009
/// [app/cpy/CVCUS01Y.cpy:L13].
pub const CUST_ADDR_COUNTRY_CD: FieldSpec =
    FieldSpec::new("CUST-ADDR-COUNTRY-CD", 236, 3, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-ADDR-ZIP PIC X(10)` — P-CVCUS01Y-010
/// [app/cpy/CVCUS01Y.cpy:L14].
pub const CUST_ADDR_ZIP: FieldSpec = FieldSpec::new("CUST-ADDR-ZIP", 239, 10, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-PHONE-NUM-1 PIC X(15)` — P-CVCUS01Y-011
/// [app/cpy/CVCUS01Y.cpy:L15].
pub const CUST_PHONE_NUM_1: FieldSpec =
    FieldSpec::new("CUST-PHONE-NUM-1", 249, 15, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-PHONE-NUM-2 PIC X(15)` — P-CVCUS01Y-012
/// [app/cpy/CVCUS01Y.cpy:L16].
pub const CUST_PHONE_NUM_2: FieldSpec =
    FieldSpec::new("CUST-PHONE-NUM-2", 264, 15, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-SSN PIC 9(09)` — P-CVCUS01Y-013
/// [app/cpy/CVCUS01Y.cpy:L17].
pub const CUST_SSN: FieldSpec = FieldSpec::new("CUST-SSN", 279, 9, FieldKind::Unsigned);

/// `CVCUS01Y` `CUST-GOVT-ISSUED-ID PIC X(20)`, text although the fixture holds
/// digits — P-CVCUS01Y-014 [app/cpy/CVCUS01Y.cpy:L18].
pub const CUST_GOVT_ISSUED_ID: FieldSpec =
    FieldSpec::new("CUST-GOVT-ISSUED-ID", 288, 20, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-DOB-YYYY-MM-DD PIC X(10)`, the date of birth as text —
/// P-CVCUS01Y-015 [app/cpy/CVCUS01Y.cpy:L19].
pub const CUST_DOB_YYYY_MM_DD: FieldSpec =
    FieldSpec::new("CUST-DOB-YYYY-MM-DD", 308, 10, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-EFT-ACCOUNT-ID PIC X(10)`, text although the fixture holds
/// digits — P-CVCUS01Y-016 [app/cpy/CVCUS01Y.cpy:L20].
pub const CUST_EFT_ACCOUNT_ID: FieldSpec =
    FieldSpec::new("CUST-EFT-ACCOUNT-ID", 318, 10, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-PRI-CARD-HOLDER-IND PIC X(01)` — P-CVCUS01Y-017
/// [app/cpy/CVCUS01Y.cpy:L21].
pub const CUST_PRI_CARD_HOLDER_IND: FieldSpec =
    FieldSpec::new("CUST-PRI-CARD-HOLDER-IND", 328, 1, FieldKind::Alnum);

/// `CVCUS01Y` `CUST-FICO-CREDIT-SCORE PIC 9(03)` — P-CVCUS01Y-018
/// [app/cpy/CVCUS01Y.cpy:L22].
pub const CUST_FICO_CREDIT_SCORE: FieldSpec =
    FieldSpec::new("CUST-FICO-CREDIT-SCORE", 329, 3, FieldKind::Unsigned);

/// `CVCUS01Y` `FILLER PIC X(168)`, kept raw — P-CVCUS01Y-019
/// [app/cpy/CVCUS01Y.cpy:L23].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 332, 168, FieldKind::Filler);

/// The `CVCUS01Y` `CUSTOMER-RECORD` layout, 500 bytes
/// [app/cpy/CVCUS01Y.cpy:L4-23].
pub struct CustomerLayout;

/// One decoded `CUSTDAT` record: the typed customer and the stored bytes of its
/// trailing FILLER.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerRecord {
    /// `CUST-ID` through `CUST-FICO-CREDIT-SCORE` (P-CVCUS01Y-001 to
    /// P-CVCUS01Y-018) [app/cpy/CVCUS01Y.cpy:L5-22], each text field as stored,
    /// trailing spaces included.
    pub customer: Customer,
    /// `FILLER PIC X(168)`, its stored bytes — P-CVCUS01Y-019
    /// [app/cpy/CVCUS01Y.cpy:L23]. The `customers` table does not store it.
    pub filler: Alnum<168>,
}

impl Layout for CustomerLayout {
    const LEN: usize = 500;
    const NAME: &'static str = "CVCUS01Y";
    const FIELDS: &'static [FieldSpec] = &[
        CUST_ID,
        CUST_FIRST_NAME,
        CUST_MIDDLE_NAME,
        CUST_LAST_NAME,
        CUST_ADDR_LINE_1,
        CUST_ADDR_LINE_2,
        CUST_ADDR_LINE_3,
        CUST_ADDR_STATE_CD,
        CUST_ADDR_COUNTRY_CD,
        CUST_ADDR_ZIP,
        CUST_PHONE_NUM_1,
        CUST_PHONE_NUM_2,
        CUST_SSN,
        CUST_GOVT_ISSUED_ID,
        CUST_DOB_YYYY_MM_DD,
        CUST_EFT_ACCOUNT_ID,
        CUST_PRI_CARD_HOLDER_IND,
        CUST_FICO_CREDIT_SCORE,
        FILLER,
    ];
    type Record = CustomerRecord;

    /// Reads `CUST-ID` through `FILLER` (P-CVCUS01Y-001 to P-CVCUS01Y-019)
    /// [app/cpy/CVCUS01Y.cpy:L5-23] in copybook order: the three `9(n)` fields
    /// as digits, every `X(n)` field and FILLER as its stored bytes.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 500 bytes long.
    /// - [`CodecErrorKind::NotNumeric`] at `CUST-ID`, `CUST-SSN` or
    ///   `CUST-FICO-CREDIT-SCORE` when that field holds a byte other than
    ///   `0`–`9`.
    /// - [`CodecErrorKind::Value`] at the field whose domain constructor
    ///   rejects the decoded value.
    fn decode(bytes: &[u8], charset: Charset) -> Result<Self::Record, CodecError> {
        if bytes.len() != Self::LEN {
            return Err(CodecError::new(
                Self::NAME,
                CodecErrorKind::Length {
                    expected: Self::LEN,
                    actual: bytes.len(),
                },
            ));
        }
        let rec = charset.to_internal(bytes);
        Ok(CustomerRecord {
            customer: Customer {
                cust_id: digits(&rec, &CUST_ID, CustomerId::new)?,
                cust_first_name: text(&rec, &CUST_FIRST_NAME)?,
                cust_middle_name: text(&rec, &CUST_MIDDLE_NAME)?,
                cust_last_name: text(&rec, &CUST_LAST_NAME)?,
                cust_addr_line_1: text(&rec, &CUST_ADDR_LINE_1)?,
                cust_addr_line_2: text(&rec, &CUST_ADDR_LINE_2)?,
                cust_addr_line_3: text(&rec, &CUST_ADDR_LINE_3)?,
                cust_addr_state_cd: text(&rec, &CUST_ADDR_STATE_CD)?,
                cust_addr_country_cd: text(&rec, &CUST_ADDR_COUNTRY_CD)?,
                cust_addr_zip: text(&rec, &CUST_ADDR_ZIP)?,
                cust_phone_num_1: text(&rec, &CUST_PHONE_NUM_1)?,
                cust_phone_num_2: text(&rec, &CUST_PHONE_NUM_2)?,
                cust_ssn: digits(&rec, &CUST_SSN, CustSsn::new)?,
                cust_govt_issued_id: text(&rec, &CUST_GOVT_ISSUED_ID)?,
                cust_dob_yyyy_mm_dd: text(&rec, &CUST_DOB_YYYY_MM_DD)?,
                cust_eft_account_id: text(&rec, &CUST_EFT_ACCOUNT_ID)?,
                cust_pri_card_holder_ind: text(&rec, &CUST_PRI_CARD_HOLDER_IND)?,
                cust_fico_credit_score: digits(&rec, &CUST_FICO_CREDIT_SCORE, FicoScore::new)?,
            },
            filler: text(&rec, &FILLER)?,
        })
    }

    /// Writes `CUST-ID` through `FILLER` (P-CVCUS01Y-001 to P-CVCUS01Y-019)
    /// [app/cpy/CVCUS01Y.cpy:L5-23]: the `9(n)` fields zero-padded to their
    /// width, every `X(n)` field and FILLER from the bytes the record holds,
    /// then translates the 500 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let customer = &record.customer;
        let mut out = vec![b' '; Self::LEN];
        let numbers: [(&FieldSpec, u64); 3] = [
            (&CUST_ID, u64::from(customer.cust_id.get())),
            (&CUST_SSN, u64::from(customer.cust_ssn.get())),
            (
                &CUST_FICO_CREDIT_SCORE,
                u64::from(customer.cust_fico_credit_score.get()),
            ),
        ];
        for (spec, value) in numbers {
            field::put_unsigned(&mut out, spec, value);
        }
        let texts: [(&FieldSpec, &[u8]); 16] = [
            (&CUST_FIRST_NAME, customer.cust_first_name.as_bytes()),
            (&CUST_MIDDLE_NAME, customer.cust_middle_name.as_bytes()),
            (&CUST_LAST_NAME, customer.cust_last_name.as_bytes()),
            (&CUST_ADDR_LINE_1, customer.cust_addr_line_1.as_bytes()),
            (&CUST_ADDR_LINE_2, customer.cust_addr_line_2.as_bytes()),
            (&CUST_ADDR_LINE_3, customer.cust_addr_line_3.as_bytes()),
            (&CUST_ADDR_STATE_CD, customer.cust_addr_state_cd.as_bytes()),
            (
                &CUST_ADDR_COUNTRY_CD,
                customer.cust_addr_country_cd.as_bytes(),
            ),
            (&CUST_ADDR_ZIP, customer.cust_addr_zip.as_bytes()),
            (&CUST_PHONE_NUM_1, customer.cust_phone_num_1.as_bytes()),
            (&CUST_PHONE_NUM_2, customer.cust_phone_num_2.as_bytes()),
            (
                &CUST_GOVT_ISSUED_ID,
                customer.cust_govt_issued_id.as_bytes(),
            ),
            (
                &CUST_DOB_YYYY_MM_DD,
                customer.cust_dob_yyyy_mm_dd.as_bytes(),
            ),
            (
                &CUST_EFT_ACCOUNT_ID,
                customer.cust_eft_account_id.as_bytes(),
            ),
            (
                &CUST_PRI_CARD_HOLDER_IND,
                customer.cust_pri_card_holder_ind.as_bytes(),
            ),
            (&FILLER, record.filler.as_bytes()),
        ];
        for (spec, bytes) in texts {
            field::put_alnum(&mut out, spec, bytes);
        }
        charset.from_internal(&out)
    }
}

/// Reads the `X(N)` field `spec` of the internal record `rec` as its stored
/// bytes.
fn text<const N: usize>(rec: &[u8], spec: &FieldSpec) -> Result<Alnum<N>, CodecError> {
    field::alnum(CustomerLayout::NAME, rec, spec)
}

/// Reads the `9(n)` field `spec` of the internal record `rec` and builds its
/// domain value with `make`, which takes the field's unsigned integer type.
///
/// A value that does not fit that integer, or that `make` rejects, is a
/// [`CodecErrorKind::Value`] carrying the domain `NumericError` text, located at
/// the field.
fn digits<I, T>(
    rec: &[u8],
    spec: &FieldSpec,
    make: fn(I) -> Result<T, NumericError>,
) -> Result<T, CodecError>
where
    I: TryFrom<u64>,
{
    let value = field::unsigned(CustomerLayout::NAME, rec, spec)?;
    I::try_from(value)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(make)
        .map_err(|error| {
            CodecError::new(
                CustomerLayout::NAME,
                CodecErrorKind::Value(error.to_string()),
            )
            .at_field(spec.name, spec.offset)
        })
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::Customer;
    use carddemo_domain::numeric::{Alnum, CustSsn, CustomerId, FicoScore};

    use super::{CUST_FICO_CREDIT_SCORE, CUST_ID, CUST_SSN, CustomerLayout, CustomerRecord};
    use crate::{Charset, CodecError, CodecErrorKind, FieldSpec, Layout, field};

    /// The ASCII `CUSTDAT` fixture: 50 records of 500 bytes, each ending LF.
    const CUSTDATA_ASCII: &str = include_str!("../../../../app/data/ASCII/custdata.txt");

    /// The EBCDIC `CUSTDAT` fixture: the same 50 records as 500-byte CP037
    /// blocks.
    const CUSTDATA_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.CUSTDATA.PS");

    /// Record 1 of `custdata.txt` [app/data/ASCII/custdata.txt:L1]
    /// (P-CVCUS01Y-021), written field by field, each value padded to its
    /// width, so that the expected bytes never come from the layout's offsets.
    const RECORD_1: &str = concat!(
        "000000001",                                          // CUST-ID 9(09)
        "Immanuel                 ",                          // CUST-FIRST-NAME X(25)
        "Madeline                 ",                          // CUST-MIDDLE-NAME X(25)
        "Kessler                  ",                          // CUST-LAST-NAME X(25)
        "618 Deshaun Route                                 ", // CUST-ADDR-LINE-1 X(50)
        "Apt. 802                                          ", // CUST-ADDR-LINE-2 X(50)
        "Altenwerthshire                                   ", // CUST-ADDR-LINE-3 X(50)
        "NC",                                                 // CUST-ADDR-STATE-CD X(02)
        "USA",                                                // CUST-ADDR-COUNTRY-CD X(03)
        "12546     ",                                         // CUST-ADDR-ZIP X(10)
        "(908)119-8310  ",                                    // CUST-PHONE-NUM-1 X(15)
        "(373)693-8684  ",                                    // CUST-PHONE-NUM-2 X(15)
        "020973888",                                          // CUST-SSN 9(09)
        "00000000000049368437",                               // CUST-GOVT-ISSUED-ID X(20)
        "1961-06-08",                                         // CUST-DOB-YYYY-MM-DD X(10)
        "0053581756",                                         // CUST-EFT-ACCOUNT-ID X(10)
        "Y",                                                  // CUST-PRI-CARD-HOLDER-IND X(01)
        "274",                                                // CUST-FICO-CREDIT-SCORE 9(03)
        // FILLER X(168): four runs of 42 spaces.
        "                                          ",
        "                                          ",
        "                                          ",
        "                                          ",
    );

    /// A record no fixture holds: the widest identifiers, Latin-1 and
    /// low-value bytes in text fields, a non-digit in each `X` field that the
    /// fixture fills with digits, and a filler that is not spaces.
    fn hand_built() -> CustomerRecord {
        CustomerRecord {
            customer: Customer {
                cust_id: CustomerId::new(999_999_999).expect("9 digits fit 9(09)"),
                cust_first_name: Alnum::move_from_bytes(b"Ren\xe9e"),
                cust_middle_name: Alnum::spaces(),
                cust_last_name: Alnum::move_from_bytes(b"Nu\xf1ez \x00\x00"),
                cust_addr_line_1: Alnum::move_from("1 Main Street"),
                cust_addr_line_2: Alnum::move_from_bytes(&[b'#'; 50]),
                cust_addr_line_3: Alnum::move_from("Springfield"),
                cust_addr_state_cd: Alnum::move_from("IL"),
                cust_addr_country_cd: Alnum::move_from("USA"),
                cust_addr_zip: Alnum::move_from("62701-0001"),
                cust_phone_num_1: Alnum::move_from("(217)555-0100"),
                cust_phone_num_2: Alnum::spaces(),
                cust_ssn: CustSsn::new(1).expect("1 fits 9(09)"),
                cust_govt_issued_id: Alnum::move_from("A1B2-C3"),
                cust_dob_yyyy_mm_dd: Alnum::move_from("2000-02-29"),
                cust_eft_account_id: Alnum::move_from("EFT 42"),
                cust_pri_card_holder_ind: Alnum::move_from("N"),
                cust_fico_credit_score: FicoScore::new(999).expect("3 digits fit 9(03)"),
            },
            filler: Alnum::from_array([b'0'; 168]),
        }
    }

    #[test]
    fn fields_tile_the_500_byte_record() {
        assert_eq!(
            (CustomerLayout::NAME, CustomerLayout::LEN),
            ("CVCUS01Y", 500)
        );
        assert_eq!(CustomerLayout::FIELDS.len(), 19);
        assert!(field::fields_are_contiguous(CustomerLayout::FIELDS, 500));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_500_bytes() {
        for len in [0, 499, 501] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    CustomerLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CVCUS01Y",
                        CodecErrorKind::Length {
                            expected: 500,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn first_custdata_record_decodes_to_its_fields_and_re_encodes_to_its_bytes()
    -> Result<(), CodecError> {
        assert_eq!(RECORD_1.len(), 500);
        assert_eq!(CUSTDATA_ASCII.lines().next(), Some(RECORD_1));

        let record = CustomerLayout::decode(RECORD_1.as_bytes(), Charset::Ascii)?;
        let customer = &record.customer;
        assert_eq!(customer.cust_id.get(), 1);
        assert_eq!(customer.cust_first_name.trimmed(), "Immanuel");
        assert_eq!(customer.cust_middle_name.trimmed(), "Madeline");
        assert_eq!(customer.cust_last_name.trimmed(), "Kessler");
        assert_eq!(customer.cust_addr_line_1.trimmed(), "618 Deshaun Route");
        assert_eq!(customer.cust_addr_line_2.trimmed(), "Apt. 802");
        assert_eq!(customer.cust_addr_line_3.trimmed(), "Altenwerthshire");
        assert_eq!(customer.cust_addr_state_cd.trimmed(), "NC");
        assert_eq!(customer.cust_addr_country_cd.trimmed(), "USA");
        assert_eq!(customer.cust_addr_zip.trimmed(), "12546");
        assert_eq!(customer.cust_phone_num_1.trimmed(), "(908)119-8310");
        assert_eq!(customer.cust_phone_num_2.trimmed(), "(373)693-8684");
        assert_eq!(customer.cust_ssn.get(), 20_973_888);
        assert_eq!(
            customer.cust_govt_issued_id.trimmed(),
            "00000000000049368437"
        );
        assert_eq!(customer.cust_dob_yyyy_mm_dd.trimmed(), "1961-06-08");
        assert_eq!(customer.cust_eft_account_id.trimmed(), "0053581756");
        assert_eq!(customer.cust_pri_card_holder_ind.trimmed(), "Y");
        assert_eq!(customer.cust_fico_credit_score.get(), 274);
        assert_eq!(record.filler, Alnum::spaces());
        assert_eq!(
            CustomerLayout::encode(&record, Charset::Ascii),
            RECORD_1.as_bytes()
        );

        // Its EBCDIC counterpart, the first 500-byte block, is the same record.
        let stored = CUSTDATA_EBCDIC
            .get(..500)
            .expect("the file holds 50 blocks");
        assert_eq!(CustomerLayout::decode(stored, Charset::Ebcdic)?, record);
        assert_eq!(CustomerLayout::encode(&record, Charset::Ebcdic), stored);
        Ok(())
    }

    #[test]
    fn a_hand_built_record_round_trips_in_both_charsets() -> Result<(), CodecError> {
        let record = hand_built();
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = CustomerLayout::encode(&record, charset);
            assert_eq!(bytes.len(), 500, "{charset:?}");
            assert_eq!(
                CustomerLayout::decode(&bytes, charset)?,
                record,
                "{charset:?}"
            );
        }
        let ascii = CustomerLayout::encode(&record, Charset::Ascii);
        assert_eq!(ascii.get(..9), Some(&b"999999999"[..]));
        assert_eq!(ascii.get(279..288), Some(&b"000000001"[..]));
        assert_eq!(ascii.get(329..332), Some(&b"999"[..]));
        assert_eq!(ascii.get(332..), Some(&[b'0'; 168][..]));
        Ok(())
    }

    #[test]
    fn a_non_digit_in_a_numeric_field_is_reported_at_that_field() {
        let fields: [FieldSpec; 3] = [CUST_ID, CUST_SSN, CUST_FICO_CREDIT_SCORE];
        for spec in fields {
            let mut internal = RECORD_1.as_bytes().to_vec();
            internal[spec.end() - 1] = b' ';
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    CustomerLayout::decode(&charset.from_internal(&internal), charset),
                    Err(
                        CodecError::new("CVCUS01Y", CodecErrorKind::NotNumeric { byte: b' ' })
                            .at_field(spec.name, spec.offset)
                    ),
                    "{} in {charset:?}",
                    spec.name
                );
            }
        }
    }
}
