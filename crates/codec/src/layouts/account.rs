//! `CVACT01Y` `ACCOUNT-RECORD`: the 300-byte `ACCTDAT` account record
//! [app/cpy/CVACT01Y.cpy:L4-17], `RECLN 300` [app/cpy/CVACT01Y.cpy:L2].
//!
//! `ACCTFILE` loads `ACCTDAT` from `app/data/ASCII/acctdata.txt` (50 records of
//! 300 bytes, each ending LF); the EBCDIC counterpart
//! `app/data/EBCDIC/AWS.M2.CARDDEMO.ACCTDATA.PS` holds the same 50 records as
//! 300-byte CP037 blocks. `batch seed` decodes each record with
//! [`AccountLayout`] into the domain [`Account`].
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `ACCT-ID` | `9(11)` | 0 | 11 | P-CVACT01Y-001 | `AccountId` |
//! | `ACCT-ACTIVE-STATUS` | `X(01)` | 11 | 1 | P-CVACT01Y-002 | `Alnum<1>` |
//! | `ACCT-CURR-BAL` | `S9(10)V99` | 12 | 12 | P-CVACT01Y-003 | `Money<10>` |
//! | `ACCT-CREDIT-LIMIT` | `S9(10)V99` | 24 | 12 | P-CVACT01Y-004 | `Money<10>` |
//! | `ACCT-CASH-CREDIT-LIMIT` | `S9(10)V99` | 36 | 12 | P-CVACT01Y-005 | `Money<10>` |
//! | `ACCT-OPEN-DATE` | `X(10)` | 48 | 10 | P-CVACT01Y-006 | `Alnum<10>` |
//! | `ACCT-EXPIRAION-DATE` | `X(10)` | 58 | 10 | P-CVACT01Y-007 | `Alnum<10>` |
//! | `ACCT-REISSUE-DATE` | `X(10)` | 68 | 10 | P-CVACT01Y-008 | `Alnum<10>` |
//! | `ACCT-CURR-CYC-CREDIT` | `S9(10)V99` | 78 | 12 | P-CVACT01Y-009 | `Money<10>` |
//! | `ACCT-CURR-CYC-DEBIT` | `S9(10)V99` | 90 | 12 | P-CVACT01Y-010 | `Money<10>` |
//! | `ACCT-ADDR-ZIP` | `X(10)` | 102 | 10 | P-CVACT01Y-011 | `Alnum<10>` |
//! | `ACCT-GROUP-ID` | `X(10)` | 112 | 10 | P-CVACT01Y-012 | `AccountGroupId` |
//! | `FILLER` | `X(178)` | 122 | 178 | P-CVACT01Y-013 | `Alnum<178>`, raw |
//!
//! The signed fields are `DISPLAY` zoned decimals whose sign is a trailing
//! overpunch; every one of them in the 50 fixture records carries a positive
//! glyph, and none is a negative zero, so [`AccountRecord::preserved`] is empty
//! for the whole corpus. `FILLER` is kept as its stored bytes (spaces in every
//! fixture record), so each record re-encodes byte for byte.
//!
//! # Fixture anomalies
//!
//! Both are read as stored, by PIC offset, and never repaired ("PIC clauses
//! are authoritative"):
//!
//! - **E-06** (P-CVACT01Y-019): `ACCT-GROUP-ID` (offset 112) is blank in all 50
//!   records, while `A000000000` sits in `ACCT-ADDR-ZIP` (offset 102). The
//!   blank group ID decodes and re-encodes as ten spaces, and the ZIP keeps
//!   `A000000000`; `CBACT04C` therefore reads the `DEFAULT` rates.
//! - **E-05** (P-CVACT01Y-018): record 49 holds `ACCT-ADDR-ZIP` `A000000000` in
//!   ASCII and `ZEROAPR` (space-padded to 10) in EBCDIC. Each decodes to its
//!   own bytes; ASCII seeds the database and drives parity.

use carddemo_domain::entities::Account;
use carddemo_domain::numeric::{AccountGroupId, AccountId, Alnum, Money};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec, PreservedFields};

/// The PIC class of the `S9(10)V99` money fields: two digits after the implied
/// decimal point.
const MONEY: FieldKind = FieldKind::Signed { scale: 2 };

/// `CVACT01Y` `ACCT-ID PIC 9(11)`, the `ACCTDAT` key — P-CVACT01Y-001
/// [app/cpy/CVACT01Y.cpy:L5].
pub const ACCT_ID: FieldSpec = FieldSpec::new("ACCT-ID", 0, 11, FieldKind::Unsigned);

/// `CVACT01Y` `ACCT-ACTIVE-STATUS PIC X(01)` — P-CVACT01Y-002
/// [app/cpy/CVACT01Y.cpy:L6].
pub const ACCT_ACTIVE_STATUS: FieldSpec =
    FieldSpec::new("ACCT-ACTIVE-STATUS", 11, 1, FieldKind::Alnum);

/// `CVACT01Y` `ACCT-CURR-BAL PIC S9(10)V99` — P-CVACT01Y-003
/// [app/cpy/CVACT01Y.cpy:L7].
pub const ACCT_CURR_BAL: FieldSpec = FieldSpec::new("ACCT-CURR-BAL", 12, 12, MONEY);

/// `CVACT01Y` `ACCT-CREDIT-LIMIT PIC S9(10)V99` — P-CVACT01Y-004
/// [app/cpy/CVACT01Y.cpy:L8].
pub const ACCT_CREDIT_LIMIT: FieldSpec = FieldSpec::new("ACCT-CREDIT-LIMIT", 24, 12, MONEY);

/// `CVACT01Y` `ACCT-CASH-CREDIT-LIMIT PIC S9(10)V99` — P-CVACT01Y-005
/// [app/cpy/CVACT01Y.cpy:L9].
pub const ACCT_CASH_CREDIT_LIMIT: FieldSpec =
    FieldSpec::new("ACCT-CASH-CREDIT-LIMIT", 36, 12, MONEY);

/// `CVACT01Y` `ACCT-OPEN-DATE PIC X(10)` — P-CVACT01Y-006
/// [app/cpy/CVACT01Y.cpy:L10].
pub const ACCT_OPEN_DATE: FieldSpec = FieldSpec::new("ACCT-OPEN-DATE", 48, 10, FieldKind::Alnum);

/// `CVACT01Y` `ACCT-EXPIRAION-DATE PIC X(10)`, the copybook's spelling kept —
/// P-CVACT01Y-007 [app/cpy/CVACT01Y.cpy:L11].
pub const ACCT_EXPIRAION_DATE: FieldSpec =
    FieldSpec::new("ACCT-EXPIRAION-DATE", 58, 10, FieldKind::Alnum);

/// `CVACT01Y` `ACCT-REISSUE-DATE PIC X(10)` — P-CVACT01Y-008
/// [app/cpy/CVACT01Y.cpy:L12].
pub const ACCT_REISSUE_DATE: FieldSpec =
    FieldSpec::new("ACCT-REISSUE-DATE", 68, 10, FieldKind::Alnum);

/// `CVACT01Y` `ACCT-CURR-CYC-CREDIT PIC S9(10)V99` — P-CVACT01Y-009
/// [app/cpy/CVACT01Y.cpy:L13].
pub const ACCT_CURR_CYC_CREDIT: FieldSpec = FieldSpec::new("ACCT-CURR-CYC-CREDIT", 78, 12, MONEY);

/// `CVACT01Y` `ACCT-CURR-CYC-DEBIT PIC S9(10)V99` — P-CVACT01Y-010
/// [app/cpy/CVACT01Y.cpy:L14].
pub const ACCT_CURR_CYC_DEBIT: FieldSpec = FieldSpec::new("ACCT-CURR-CYC-DEBIT", 90, 12, MONEY);

/// `CVACT01Y` `ACCT-ADDR-ZIP PIC X(10)`, which holds `A000000000` in every
/// fixture record (E-06) — P-CVACT01Y-011 [app/cpy/CVACT01Y.cpy:L15].
pub const ACCT_ADDR_ZIP: FieldSpec = FieldSpec::new("ACCT-ADDR-ZIP", 102, 10, FieldKind::Alnum);

/// `CVACT01Y` `ACCT-GROUP-ID PIC X(10)`, blank in every fixture record (E-06)
/// — P-CVACT01Y-012 [app/cpy/CVACT01Y.cpy:L16].
pub const ACCT_GROUP_ID: FieldSpec = FieldSpec::new("ACCT-GROUP-ID", 112, 10, FieldKind::Alnum);

/// `CVACT01Y` `FILLER PIC X(178)`, kept raw — P-CVACT01Y-013
/// [app/cpy/CVACT01Y.cpy:L17].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 122, 178, FieldKind::Filler);

/// The `CVACT01Y` `ACCOUNT-RECORD` layout, 300 bytes
/// [app/cpy/CVACT01Y.cpy:L4-17].
pub struct AccountLayout;

/// One decoded `ACCTDAT` record: the domain account plus the bytes it does not
/// model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRecord {
    /// `ACCT-ID` through `ACCT-GROUP-ID` (P-CVACT01Y-001 to P-CVACT01Y-012)
    /// [app/cpy/CVACT01Y.cpy:L5-16].
    pub account: Account,
    /// `FILLER PIC X(178)`, its stored bytes — P-CVACT01Y-013
    /// [app/cpy/CVACT01Y.cpy:L17].
    pub filler: Alnum<178>,
    /// The stored bytes of any negative-zero money field, which
    /// [`Money`] cannot represent. Empty for every fixture record; a non-empty
    /// list on fixture data is an escalation, never a repair.
    pub preserved: PreservedFields,
}

impl Layout for AccountLayout {
    const LEN: usize = 300;
    const NAME: &'static str = "CVACT01Y";
    const FIELDS: &'static [FieldSpec] = &[
        ACCT_ID,
        ACCT_ACTIVE_STATUS,
        ACCT_CURR_BAL,
        ACCT_CREDIT_LIMIT,
        ACCT_CASH_CREDIT_LIMIT,
        ACCT_OPEN_DATE,
        ACCT_EXPIRAION_DATE,
        ACCT_REISSUE_DATE,
        ACCT_CURR_CYC_CREDIT,
        ACCT_CURR_CYC_DEBIT,
        ACCT_ADDR_ZIP,
        ACCT_GROUP_ID,
        FILLER,
    ];
    type Record = AccountRecord;

    /// Reads `ACCT-ID` through `FILLER` (P-CVACT01Y-001 to P-CVACT01Y-013)
    /// [app/cpy/CVACT01Y.cpy:L5-17] in copybook order: the `X(n)` fields and
    /// `FILLER` as their stored bytes, `ACCT-ID` as 11 digits and the five
    /// money fields as overpunch-signed cent counts.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 300 bytes long.
    /// - The field errors of [`field::unsigned`] and
    ///   [`field::signed_preserving`] (a non-digit, a plain digit or an invalid
    ///   byte in the sign position), located at the field.
    /// - [`CodecErrorKind::Value`] should a domain constructor reject a value.
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
        let mut preserved = PreservedFields::new();
        // Struct fields are evaluated in source order, which is the copybook
        // order, so the first faulty field is the one reported.
        let account = Account {
            acct_id: read_account_id(&rec)?,
            acct_active_status: field::alnum(Self::NAME, &rec, &ACCT_ACTIVE_STATUS)?,
            acct_curr_bal: read_money(&rec, &ACCT_CURR_BAL, &mut preserved)?,
            acct_credit_limit: read_money(&rec, &ACCT_CREDIT_LIMIT, &mut preserved)?,
            acct_cash_credit_limit: read_money(&rec, &ACCT_CASH_CREDIT_LIMIT, &mut preserved)?,
            acct_open_date: field::alnum(Self::NAME, &rec, &ACCT_OPEN_DATE)?,
            acct_expiraion_date: field::alnum(Self::NAME, &rec, &ACCT_EXPIRAION_DATE)?,
            acct_reissue_date: field::alnum(Self::NAME, &rec, &ACCT_REISSUE_DATE)?,
            acct_curr_cyc_credit: read_money(&rec, &ACCT_CURR_CYC_CREDIT, &mut preserved)?,
            acct_curr_cyc_debit: read_money(&rec, &ACCT_CURR_CYC_DEBIT, &mut preserved)?,
            acct_addr_zip: field::alnum(Self::NAME, &rec, &ACCT_ADDR_ZIP)?,
            acct_group_id: AccountGroupId::from_alnum(field::alnum(
                Self::NAME,
                &rec,
                &ACCT_GROUP_ID,
            )?),
        };
        Ok(AccountRecord {
            account,
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
            preserved,
        })
    }

    /// Writes `ACCT-ID` through `FILLER` (P-CVACT01Y-001 to P-CVACT01Y-013)
    /// [app/cpy/CVACT01Y.cpy:L5-17]: `ACCT-ID` zero-padded to 11 digits, the
    /// money fields as 12 digits with a trailing overpunch sign (restoring any
    /// preserved negative zero), the `X(n)` fields and `FILLER` from the bytes
    /// the record holds; then translates the 300 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let account = &record.account;
        let preserved = &record.preserved;
        let mut out = vec![b' '; Self::LEN];
        field::put_unsigned(&mut out, &ACCT_ID, account.acct_id.get());
        field::put_alnum(
            &mut out,
            &ACCT_ACTIVE_STATUS,
            account.acct_active_status.as_bytes(),
        );
        put_money(&mut out, &ACCT_CURR_BAL, account.acct_curr_bal, preserved);
        put_money(
            &mut out,
            &ACCT_CREDIT_LIMIT,
            account.acct_credit_limit,
            preserved,
        );
        put_money(
            &mut out,
            &ACCT_CASH_CREDIT_LIMIT,
            account.acct_cash_credit_limit,
            preserved,
        );
        field::put_alnum(&mut out, &ACCT_OPEN_DATE, account.acct_open_date.as_bytes());
        field::put_alnum(
            &mut out,
            &ACCT_EXPIRAION_DATE,
            account.acct_expiraion_date.as_bytes(),
        );
        field::put_alnum(
            &mut out,
            &ACCT_REISSUE_DATE,
            account.acct_reissue_date.as_bytes(),
        );
        put_money(
            &mut out,
            &ACCT_CURR_CYC_CREDIT,
            account.acct_curr_cyc_credit,
            preserved,
        );
        put_money(
            &mut out,
            &ACCT_CURR_CYC_DEBIT,
            account.acct_curr_cyc_debit,
            preserved,
        );
        field::put_alnum(&mut out, &ACCT_ADDR_ZIP, account.acct_addr_zip.as_bytes());
        field::put_alnum(&mut out, &ACCT_GROUP_ID, account.acct_group_id.as_bytes());
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// Reads `ACCT-ID PIC 9(11)` — P-CVACT01Y-001 [app/cpy/CVACT01Y.cpy:L5].
fn read_account_id(rec: &[u8]) -> Result<AccountId, CodecError> {
    let value = field::unsigned(AccountLayout::NAME, rec, &ACCT_ID)?;
    AccountId::new(value).map_err(|error| value_error(&ACCT_ID, error.to_string()))
}

/// Reads one `S9(10)V99` money field `spec` — P-CVACT01Y-003, -004, -005, -009
/// and -010 [app/cpy/CVACT01Y.cpy:L7-9, L13-14] — recording its stored bytes
/// in `preserved` when it is a negative zero.
fn read_money(
    rec: &[u8],
    spec: &FieldSpec,
    preserved: &mut PreservedFields,
) -> Result<Money<10>, CodecError> {
    field::signed_preserving(AccountLayout::NAME, rec, spec, preserved)?
        .value()
        .and_then(Money::<10>::from_cents)
        .map_err(|error| value_error(spec, error.to_string()))
}

/// Writes one `S9(10)V99` money field `spec` — P-CVACT01Y-003, -004, -005,
/// -009 and -010 [app/cpy/CVACT01Y.cpy:L7-9, L13-14] — as its signed cent
/// count, restoring the bytes `preserved` holds for a negative zero.
fn put_money(out: &mut [u8], spec: &FieldSpec, value: Money<10>, preserved: &PreservedFields) {
    field::put_signed_preserving(out, spec, value.cents(), preserved);
}

/// The [`CodecErrorKind::Value`] error `message` located at the field `spec`.
fn value_error(spec: &FieldSpec, message: String) -> CodecError {
    CodecError::new(AccountLayout::NAME, CodecErrorKind::Value(message))
        .at_field(spec.name, spec.offset)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use carddemo_domain::entities::Account;
    use carddemo_domain::numeric::{AccountGroupId, AccountId, Alnum, Money};

    use super::{ACCT_CURR_BAL, ACCT_CURR_CYC_DEBIT, ACCT_ID, AccountLayout, AccountRecord};
    use crate::field::{self, FieldSpec, PreservedFields};
    use crate::{Charset, CodecError, CodecErrorKind, Layout};

    /// The ASCII `ACCTDAT` file: 50 records of 300 bytes, each ending LF.
    const ACCTDATA_ASCII: &[u8] = include_bytes!("../../../../app/data/ASCII/acctdata.txt");

    /// The EBCDIC `ACCTDAT` file: the same 50 records as 300-byte CP037 blocks.
    const ACCTDATA_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.ACCTDATA.PS");

    /// The record bodies of `acctdata.txt`, without their LF.
    fn ascii_records() -> Vec<&'static [u8]> {
        ACCTDATA_ASCII
            .chunks(AccountLayout::LEN + 1)
            .map(|line| {
                line.strip_suffix(b"\n")
                    .unwrap_or_else(|| panic!("an acctdata.txt record does not end in LF"))
            })
            .collect()
    }

    /// `acctdata.txt` L1 built from its field values, padded with the 188
    /// spaces of the blank `ACCT-GROUP-ID` and `FILLER`.
    fn first_fixture_line() -> Vec<u8> {
        let mut line = concat!(
            "00000000001",
            "Y",
            "00000001940{",
            "00000020200{",
            "00000010200{",
            "2014-11-20",
            "2025-05-20",
            "2025-05-20",
            "00000000000{",
            "00000000000{",
            "A000000000",
        )
        .as_bytes()
        .to_vec();
        assert_eq!(line.len(), 112);
        line.resize(AccountLayout::LEN, b' ');
        line
    }

    /// A record with a distinct value in every field, negative money included.
    fn hand_built_record() -> Result<AccountRecord, Box<dyn Error>> {
        Ok(AccountRecord {
            account: Account {
                acct_id: AccountId::new(12_345_678_901)?,
                acct_active_status: Alnum::move_from("N"),
                acct_curr_bal: Money::<10>::from_cents(-123_456)?,
                acct_credit_limit: Money::<10>::from_cents(999_999_999_999)?,
                acct_cash_credit_limit: Money::<10>::from_cents(-1)?,
                acct_open_date: Alnum::move_from("2020-01-31"),
                acct_expiraion_date: Alnum::move_from("2030-12-31"),
                acct_reissue_date: Alnum::move_from("2025-06-30"),
                acct_curr_cyc_credit: Money::<10>::ZERO,
                acct_curr_cyc_debit: Money::<10>::from_cents(-50)?,
                acct_addr_zip: Alnum::move_from("12345"),
                acct_group_id: AccountGroupId::move_from("GROUP01"),
            },
            filler: Alnum::move_from("FILLER-BYTES"),
            preserved: PreservedFields::new(),
        })
    }

    #[test]
    fn fields_tile_the_300_byte_record() {
        assert_eq!((AccountLayout::NAME, AccountLayout::LEN), ("CVACT01Y", 300));
        assert_eq!(AccountLayout::FIELDS.len(), 13);
        assert!(field::fields_are_contiguous(
            AccountLayout::FIELDS,
            AccountLayout::LEN
        ));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_300_bytes() {
        for len in [299, 301] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    AccountLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CVACT01Y",
                        CodecErrorKind::Length {
                            expected: 300,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn first_fixture_record_decodes_to_its_fields_and_re_encodes_to_its_bytes()
    -> Result<(), CodecError> {
        let line = first_fixture_line();
        assert_eq!(ascii_records().first().copied(), Some(line.as_slice()));
        let record = AccountLayout::decode(&line, Charset::Ascii)?;
        let account = &record.account;
        assert_eq!(account.acct_id.get(), 1);
        assert_eq!(account.acct_active_status.as_bytes(), b"Y");
        assert_eq!(account.acct_curr_bal.cents(), 19_400);
        assert_eq!(account.acct_credit_limit.cents(), 202_000);
        assert_eq!(account.acct_cash_credit_limit.cents(), 102_000);
        assert_eq!(account.acct_open_date.as_bytes(), b"2014-11-20");
        assert_eq!(account.acct_expiraion_date.as_bytes(), b"2025-05-20");
        assert_eq!(account.acct_reissue_date.as_bytes(), b"2025-05-20");
        assert!(account.acct_curr_cyc_credit.is_zero());
        assert!(account.acct_curr_cyc_debit.is_zero());
        assert_eq!(account.acct_addr_zip.as_bytes(), b"A000000000");
        assert_eq!(account.acct_group_id.as_bytes(), &[b' '; 10], "E-06");
        assert_eq!(record.filler, Alnum::spaces());
        assert!(record.preserved.is_empty());
        assert_eq!(AccountLayout::encode(&record, Charset::Ascii), line);
        Ok(())
    }

    #[test]
    fn a_hand_built_record_encodes_every_field_at_its_offset() -> Result<(), Box<dyn Error>> {
        let record = hand_built_record()?;
        let mut expected = concat!(
            "12345678901",
            "N",
            "00000012345O",
            "99999999999I",
            "00000000000J",
            "2020-01-31",
            "2030-12-31",
            "2025-06-30",
            "00000000000{",
            "00000000005}",
            "12345     ",
            "GROUP01   ",
            "FILLER-BYTES",
        )
        .as_bytes()
        .to_vec();
        expected.resize(AccountLayout::LEN, b' ');
        let encoded = AccountLayout::encode(&record, Charset::Ascii);
        assert_eq!(encoded.get(12..24), Some(&b"00000012345O"[..]));
        assert_eq!(encoded, expected);
        Ok(())
    }

    #[test]
    fn a_hand_built_record_round_trips_in_both_charsets() -> Result<(), Box<dyn Error>> {
        let record = hand_built_record()?;
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let encoded = AccountLayout::encode(&record, charset);
            assert_eq!(encoded.len(), AccountLayout::LEN, "{charset:?}");
            assert_eq!(
                AccountLayout::decode(&encoded, charset)?,
                record,
                "{charset:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_negative_zero_balance_keeps_its_bytes_until_the_balance_changes()
    -> Result<(), Box<dyn Error>> {
        let mut line = first_fixture_line();
        line.splice(12..24, b"00000000000}".iter().copied());
        let mut record = AccountLayout::decode(&line, Charset::Ascii)?;
        assert!(record.account.acct_curr_bal.is_zero());
        assert_eq!(record.preserved.len(), 1);
        assert_eq!(
            record.preserved.get(12).map(|raw| raw.bytes.as_slice()),
            Some(&b"00000000000}"[..])
        );
        assert_eq!(AccountLayout::encode(&record, Charset::Ascii), line);

        record.account.acct_curr_bal = Money::<10>::from_cents(5)?;
        let encoded = AccountLayout::encode(&record, Charset::Ascii);
        assert_eq!(encoded.get(12..24), Some(&b"00000000000E"[..]));
        Ok(())
    }

    #[test]
    fn a_faulty_field_is_reported_at_its_copybook_position() {
        let cases: [(usize, u8, CodecErrorKind, FieldSpec); 3] = [
            (5, b'X', CodecErrorKind::NotNumeric { byte: b'X' }, ACCT_ID),
            (
                23,
                b'0',
                CodecErrorKind::PlainDigitInSigned { byte: b'0' },
                ACCT_CURR_BAL,
            ),
            (
                101,
                b' ',
                CodecErrorKind::InvalidSign { byte: b' ' },
                ACCT_CURR_CYC_DEBIT,
            ),
        ];
        for (offset, byte, kind, spec) in cases {
            let mut line = first_fixture_line();
            line[offset] = byte;
            assert_eq!(
                AccountLayout::decode(&line, Charset::Ascii),
                Err(CodecError::new("CVACT01Y", kind).at_field(spec.name, spec.offset)),
                "byte 0x{byte:02X} at offset {offset}"
            );
        }
    }

    #[test]
    fn every_fixture_record_round_trips_and_its_ebcdic_twin_differs_only_at_e05()
    -> Result<(), CodecError> {
        let ascii = ascii_records();
        let ebcdic: Vec<&[u8]> = ACCTDATA_EBCDIC.chunks(AccountLayout::LEN).collect();
        assert_eq!((ascii.len(), ebcdic.len()), (50, 50));
        for (number, (stored_ascii, stored_ebcdic)) in (1..).zip(ascii.into_iter().zip(ebcdic)) {
            let from_ascii = AccountLayout::decode(stored_ascii, Charset::Ascii)?;
            let from_ebcdic = AccountLayout::decode(stored_ebcdic, Charset::Ebcdic)?;
            assert_eq!(
                AccountLayout::encode(&from_ascii, Charset::Ascii),
                stored_ascii,
                "ASCII record {number}"
            );
            assert_eq!(
                AccountLayout::encode(&from_ebcdic, Charset::Ebcdic),
                stored_ebcdic,
                "EBCDIC record {number}"
            );
            assert!(from_ascii.preserved.is_empty() && from_ebcdic.preserved.is_empty());
            assert!(
                from_ascii.account.acct_group_id.as_alnum().is_blank(),
                "E-06, record {number}"
            );
            assert_eq!(from_ascii.account.acct_addr_zip.as_bytes(), b"A000000000");
            let mut twin = from_ebcdic;
            if number == 49 {
                assert_eq!(twin.account.acct_addr_zip.as_bytes(), b"ZEROAPR   ", "E-05");
                twin.account.acct_addr_zip = from_ascii.account.acct_addr_zip;
            }
            assert_eq!(twin, from_ascii, "record {number}");
        }
        Ok(())
    }
}
