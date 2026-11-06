//! `CVTRA05Y` `TRAN-RECORD`: the 350-byte transaction master record
//! [app/cpy/CVTRA05Y.cpy:L2-18], the record of the VSAM KSDS `TRANSACT`, keyed
//! on `TRAN-ID` `KEYS(16 0)` [app/jcl/TRANFILE.jcl:L53].
//!
//! Batch writes its `TRANSACT` generation files with this layout, each record
//! at full width followed by `LF` (`framing::encode_ascii_lf`):
//! `TRANSACT.BKUP(+1)` (`tranbkp`, `tranrept`), `TRANSACT.COMBINED(+1)`
//! (`combtran`), `TRANSACT.DALY(+1)` (`tranrept`) and the interest
//! transactions of `SYSTRAN(+1)` (`intcalc`). `combtran` reads
//! `TRANSACT.BKUP(0)`, `SYSTRAN(0)` and `TRANSACT.COMBINED(+1)`, and
//! `tranrept` its `TRANSACT.BKUP(+1)` and `TRANSACT.DALY(+1)`, back through it
//! with `framing::decode_ascii_lf`. The live master starts empty (E-07), so no
//! fixture file of this layout exists. The fields have the widths and offsets
//! of `CVTRA06Y` `DALYTRAN-RECORD`, from which `CBTRN02C` moves them one by
//! one, so a daily transaction record also decodes here.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `TRAN-ID` | `X(16)` | 0 | 16 | P-CVTRA05Y-001 | `TranId` |
//! | `TRAN-TYPE-CD` | `X(02)` | 16 | 2 | P-CVTRA05Y-002 | `TranTypeCode` |
//! | `TRAN-CAT-CD` | `9(04)` | 18 | 4 | P-CVTRA05Y-003 | `TranCategoryCode` |
//! | `TRAN-SOURCE` | `X(10)` | 22 | 10 | P-CVTRA05Y-004 | `Alnum<10>` |
//! | `TRAN-DESC` | `X(100)` | 32 | 100 | P-CVTRA05Y-005 | `Alnum<100>` |
//! | `TRAN-AMT` | `S9(09)V99` | 132 | 11 | P-CVTRA05Y-006 | `Money<9>` |
//! | `TRAN-MERCHANT-ID` | `9(09)` | 143 | 9 | P-CVTRA05Y-007 | `MerchantId` |
//! | `TRAN-MERCHANT-NAME` | `X(50)` | 152 | 50 | P-CVTRA05Y-008 | `Alnum<50>` |
//! | `TRAN-MERCHANT-CITY` | `X(50)` | 202 | 50 | P-CVTRA05Y-009 | `Alnum<50>` |
//! | `TRAN-MERCHANT-ZIP` | `X(10)` | 252 | 10 | P-CVTRA05Y-010 | `Alnum<10>` |
//! | `TRAN-CARD-NUM` | `X(16)` | 262 | 16 | P-CVTRA05Y-011 | `CardNumber` |
//! | `TRAN-ORIG-TS` | `X(26)` | 278 | 26 | P-CVTRA05Y-012 | `Alnum<26>` |
//! | `TRAN-PROC-TS` | `X(26)` | 304 | 26 | P-CVTRA05Y-013 | `Alnum<26>` |
//! | `FILLER` | `X(20)` | 330 | 20 | P-CVTRA05Y-014 | `Alnum<20>`, raw |
//!
//! `TRAN-PROC-TS` is the key of the `TRANIDX` alternate index,
//! `KEYS(26 304)` [app/jcl/TRANFILE.jcl:L84]; in the replacement that index is
//! the `tran_proc_ts` column index, and the codec only carries the 26 bytes.
//!
//! # The record content and its FILLER
//!
//! [`TransactionRecord`] holds the domain `NewTransaction`, the record content
//! without the `transactions` row identity (E-09), plus the 20 FILLER bytes
//! and the raw bytes of a negative-zero `TRAN-AMT`, so that any decoded record
//! re-encodes to its stored bytes. POSTTRAN and INTCALC never set the FILLER,
//! whose COBOL content is therefore unpredictable; those bytes are excluded
//! from parity comparison and the replacement writes spaces in them (E-25,
//! P-CVTRA05Y-018), which is what [`TransactionRecord::new`] builds.

use carddemo_domain::entities::NewTransaction;
use carddemo_domain::numeric::{
    Alnum, CardNumber, MerchantId, Money, NumericError, TranCategoryCode, TranId, TranTypeCode,
};

use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec, PreservedFields};

/// `CVTRA05Y` `TRAN-ID PIC X(16)`, the `TRANSACT` key — P-CVTRA05Y-001
/// [app/cpy/CVTRA05Y.cpy:L5].
pub const TRAN_ID: FieldSpec = FieldSpec::new("TRAN-ID", 0, 16, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-TYPE-CD PIC X(02)` — P-CVTRA05Y-002
/// [app/cpy/CVTRA05Y.cpy:L6].
pub const TRAN_TYPE_CD: FieldSpec = FieldSpec::new("TRAN-TYPE-CD", 16, 2, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-CAT-CD PIC 9(04)` — P-CVTRA05Y-003
/// [app/cpy/CVTRA05Y.cpy:L7].
pub const TRAN_CAT_CD: FieldSpec = FieldSpec::new("TRAN-CAT-CD", 18, 4, FieldKind::Unsigned);

/// `CVTRA05Y` `TRAN-SOURCE PIC X(10)` — P-CVTRA05Y-004
/// [app/cpy/CVTRA05Y.cpy:L8].
pub const TRAN_SOURCE: FieldSpec = FieldSpec::new("TRAN-SOURCE", 22, 10, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-DESC PIC X(100)` — P-CVTRA05Y-005
/// [app/cpy/CVTRA05Y.cpy:L9].
pub const TRAN_DESC: FieldSpec = FieldSpec::new("TRAN-DESC", 32, 100, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-AMT PIC S9(09)V99`, sign as a trailing overpunch —
/// P-CVTRA05Y-006 [app/cpy/CVTRA05Y.cpy:L10].
pub const TRAN_AMT: FieldSpec = FieldSpec::new("TRAN-AMT", 132, 11, FieldKind::Signed { scale: 2 });

/// `CVTRA05Y` `TRAN-MERCHANT-ID PIC 9(09)` — P-CVTRA05Y-007
/// [app/cpy/CVTRA05Y.cpy:L11].
pub const TRAN_MERCHANT_ID: FieldSpec =
    FieldSpec::new("TRAN-MERCHANT-ID", 143, 9, FieldKind::Unsigned);

/// `CVTRA05Y` `TRAN-MERCHANT-NAME PIC X(50)` — P-CVTRA05Y-008
/// [app/cpy/CVTRA05Y.cpy:L12].
pub const TRAN_MERCHANT_NAME: FieldSpec =
    FieldSpec::new("TRAN-MERCHANT-NAME", 152, 50, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-MERCHANT-CITY PIC X(50)` — P-CVTRA05Y-009
/// [app/cpy/CVTRA05Y.cpy:L13].
pub const TRAN_MERCHANT_CITY: FieldSpec =
    FieldSpec::new("TRAN-MERCHANT-CITY", 202, 50, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-MERCHANT-ZIP PIC X(10)` — P-CVTRA05Y-010
/// [app/cpy/CVTRA05Y.cpy:L14].
pub const TRAN_MERCHANT_ZIP: FieldSpec =
    FieldSpec::new("TRAN-MERCHANT-ZIP", 252, 10, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-CARD-NUM PIC X(16)` — P-CVTRA05Y-011
/// [app/cpy/CVTRA05Y.cpy:L15].
pub const TRAN_CARD_NUM: FieldSpec = FieldSpec::new("TRAN-CARD-NUM", 262, 16, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-ORIG-TS PIC X(26)` — P-CVTRA05Y-012
/// [app/cpy/CVTRA05Y.cpy:L16].
pub const TRAN_ORIG_TS: FieldSpec = FieldSpec::new("TRAN-ORIG-TS", 278, 26, FieldKind::Alnum);

/// `CVTRA05Y` `TRAN-PROC-TS PIC X(26)`, the `TRANIDX` alternate key
/// `KEYS(26 304)` [app/jcl/TRANFILE.jcl:L84] — P-CVTRA05Y-013
/// [app/cpy/CVTRA05Y.cpy:L17].
pub const TRAN_PROC_TS: FieldSpec = FieldSpec::new("TRAN-PROC-TS", 304, 26, FieldKind::Alnum);

/// `CVTRA05Y` `FILLER PIC X(20)`, kept raw — P-CVTRA05Y-014
/// [app/cpy/CVTRA05Y.cpy:L18].
pub const FILLER: FieldSpec = FieldSpec::new("FILLER", 330, 20, FieldKind::Filler);

/// The `CVTRA05Y` `TRAN-RECORD` layout, 350 bytes
/// [app/cpy/CVTRA05Y.cpy:L2-18].
pub struct TransactionLayout;

/// One decoded `TRANSACT` record: its content, its FILLER bytes and the raw
/// bytes its typed fields cannot reproduce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionRecord {
    /// `TRAN-ID` through `TRAN-PROC-TS` — P-CVTRA05Y-001 to P-CVTRA05Y-013
    /// [app/cpy/CVTRA05Y.cpy:L5-17].
    pub transaction: NewTransaction,
    /// `FILLER PIC X(20)`, its stored bytes — P-CVTRA05Y-014
    /// [app/cpy/CVTRA05Y.cpy:L18].
    pub filler: Alnum<20>,
    /// The stored bytes of a negative-zero `TRAN-AMT` (P-CVTRA05Y-006),
    /// written back while the amount stays zero. A non-empty list on fixture
    /// data is an escalation, never a repair.
    pub preserved: PreservedFields,
}

impl TransactionRecord {
    /// The record batch writes for `transaction`: FILLER spaces, the content
    /// the replacement gives the bytes the COBOL never sets (E-25,
    /// P-CVTRA05Y-018), and nothing preserved.
    #[must_use]
    pub const fn new(transaction: NewTransaction) -> Self {
        Self {
            transaction,
            filler: Alnum::spaces(),
            preserved: PreservedFields::new(),
        }
    }
}

impl Layout for TransactionLayout {
    const LEN: usize = 350;
    const NAME: &'static str = "CVTRA05Y";
    const FIELDS: &'static [FieldSpec] = &[
        TRAN_ID,
        TRAN_TYPE_CD,
        TRAN_CAT_CD,
        TRAN_SOURCE,
        TRAN_DESC,
        TRAN_AMT,
        TRAN_MERCHANT_ID,
        TRAN_MERCHANT_NAME,
        TRAN_MERCHANT_CITY,
        TRAN_MERCHANT_ZIP,
        TRAN_CARD_NUM,
        TRAN_ORIG_TS,
        TRAN_PROC_TS,
        FILLER,
    ];
    type Record = TransactionRecord;

    /// Reads `TRAN-ID` through `FILLER` (P-CVTRA05Y-001 to P-CVTRA05Y-014)
    /// [app/cpy/CVTRA05Y.cpy:L5-18]: the `X(n)` fields as their stored bytes,
    /// the `9(n)` fields as digits and `TRAN-AMT` as an overpunch-signed cent
    /// count.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 350 bytes long.
    /// - The errors of the `field` readers for `TRAN-CAT-CD`, `TRAN-AMT` and
    ///   `TRAN-MERCHANT-ID`, located at their fields.
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
        let transaction = read_transaction(&rec, &mut preserved)?;
        Ok(TransactionRecord {
            transaction,
            filler: field::alnum(Self::NAME, &rec, &FILLER)?,
            preserved,
        })
    }

    /// Writes `TRAN-ID` through `FILLER` (P-CVTRA05Y-001 to P-CVTRA05Y-014)
    /// [app/cpy/CVTRA05Y.cpy:L5-18] into a 350-byte buffer of spaces, restores
    /// a preserved negative-zero `TRAN-AMT`, then translates the buffer to
    /// `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let mut out = vec![b' '; Self::LEN];
        write_transaction(&mut out, &record.transaction, &record.preserved);
        field::put_alnum(&mut out, &FILLER, record.filler.as_bytes());
        charset.from_internal(&out)
    }
}

/// Reads `TRAN-ID` through `TRAN-PROC-TS` (P-CVTRA05Y-001 to P-CVTRA05Y-013)
/// [app/cpy/CVTRA05Y.cpy:L5-17] from the internal bytes `rec`, recording a
/// negative-zero `TRAN-AMT` in `preserved`.
fn read_transaction(
    rec: &[u8],
    preserved: &mut PreservedFields,
) -> Result<NewTransaction, CodecError> {
    let name = TransactionLayout::NAME;
    Ok(NewTransaction {
        tran_id: TranId::from_alnum(field::alnum(name, rec, &TRAN_ID)?),
        tran_type_cd: TranTypeCode::from_alnum(field::alnum(name, rec, &TRAN_TYPE_CD)?),
        tran_cat_cd: read_digits::<TranCategoryCode>(rec, &TRAN_CAT_CD)?,
        tran_source: field::alnum(name, rec, &TRAN_SOURCE)?,
        tran_desc: field::alnum(name, rec, &TRAN_DESC)?,
        tran_amt: read_amount(rec, preserved)?,
        tran_merchant_id: read_digits::<MerchantId>(rec, &TRAN_MERCHANT_ID)?,
        tran_merchant_name: field::alnum(name, rec, &TRAN_MERCHANT_NAME)?,
        tran_merchant_city: field::alnum(name, rec, &TRAN_MERCHANT_CITY)?,
        tran_merchant_zip: field::alnum(name, rec, &TRAN_MERCHANT_ZIP)?,
        tran_card_num: CardNumber::from_alnum(field::alnum(name, rec, &TRAN_CARD_NUM)?),
        tran_orig_ts: field::alnum(name, rec, &TRAN_ORIG_TS)?,
        tran_proc_ts: field::alnum(name, rec, &TRAN_PROC_TS)?,
    })
}

/// Reads the unsigned `9(n)` field `spec` of `rec` into its domain identifier:
/// `TRAN-CAT-CD` (P-CVTRA05Y-003) [app/cpy/CVTRA05Y.cpy:L7] or
/// `TRAN-MERCHANT-ID` (P-CVTRA05Y-007) [app/cpy/CVTRA05Y.cpy:L11].
///
/// # Errors
///
/// The errors of `field::unsigned`, and [`CodecErrorKind::Value`] when the
/// identifier rejects the value, each located at `spec`.
fn read_digits<T>(rec: &[u8], spec: &FieldSpec) -> Result<T, CodecError>
where
    T: TryFrom<i64, Error = NumericError>,
{
    let digits = field::unsigned(TransactionLayout::NAME, rec, spec)?;
    i64::try_from(digits)
        .map_err(|_| NumericError::OutOfRange)
        .and_then(T::try_from)
        .map_err(|error| value_error(spec, error))
}

/// Reads `TRAN-AMT PIC S9(09)V99` (P-CVTRA05Y-006) [app/cpy/CVTRA05Y.cpy:L10]
/// of `rec` as a cent count, recording a negative zero in `preserved`.
///
/// # Errors
///
/// The errors of `field::signed_preserving`, and [`CodecErrorKind::Value`]
/// when `Money::<9>::from_cents` rejects the value, each located at
/// `TRAN-AMT`.
fn read_amount(rec: &[u8], preserved: &mut PreservedFields) -> Result<Money<9>, CodecError> {
    field::signed_preserving(TransactionLayout::NAME, rec, &TRAN_AMT, preserved)?
        .value()
        .and_then(Money::from_cents)
        .map_err(|error| value_error(&TRAN_AMT, error))
}

/// Writes `TRAN-ID` through `TRAN-PROC-TS` (P-CVTRA05Y-001 to P-CVTRA05Y-013)
/// [app/cpy/CVTRA05Y.cpy:L5-17] of `transaction` into the internal record
/// buffer `out`, restoring the `TRAN-AMT` bytes `preserved` holds while the
/// amount is zero.
fn write_transaction(out: &mut [u8], transaction: &NewTransaction, preserved: &PreservedFields) {
    field::put_alnum(out, &TRAN_ID, transaction.tran_id.as_bytes());
    field::put_alnum(out, &TRAN_TYPE_CD, transaction.tran_type_cd.as_bytes());
    field::put_unsigned(out, &TRAN_CAT_CD, u64::from(transaction.tran_cat_cd.get()));
    field::put_alnum(out, &TRAN_SOURCE, transaction.tran_source.as_bytes());
    field::put_alnum(out, &TRAN_DESC, transaction.tran_desc.as_bytes());
    field::put_signed_preserving(out, &TRAN_AMT, transaction.tran_amt.cents(), preserved);
    field::put_unsigned(
        out,
        &TRAN_MERCHANT_ID,
        u64::from(transaction.tran_merchant_id.get()),
    );
    field::put_alnum(
        out,
        &TRAN_MERCHANT_NAME,
        transaction.tran_merchant_name.as_bytes(),
    );
    field::put_alnum(
        out,
        &TRAN_MERCHANT_CITY,
        transaction.tran_merchant_city.as_bytes(),
    );
    field::put_alnum(
        out,
        &TRAN_MERCHANT_ZIP,
        transaction.tran_merchant_zip.as_bytes(),
    );
    field::put_alnum(out, &TRAN_CARD_NUM, transaction.tran_card_num.as_bytes());
    field::put_alnum(out, &TRAN_ORIG_TS, transaction.tran_orig_ts.as_bytes());
    field::put_alnum(out, &TRAN_PROC_TS, transaction.tran_proc_ts.as_bytes());
}

/// The [`CodecErrorKind::Value`] error carrying the domain `error` text,
/// located at the field `spec` of `CVTRA05Y`.
fn value_error(spec: &FieldSpec, error: NumericError) -> CodecError {
    CodecError::new(
        TransactionLayout::NAME,
        CodecErrorKind::Value(error.to_string()),
    )
    .at_field(spec.name, spec.offset)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use carddemo_domain::entities::NewTransaction;
    use carddemo_domain::numeric::{
        Alnum, CardNumber, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode,
    };

    use super::{TRAN_AMT, TRAN_CAT_CD, TransactionLayout, TransactionRecord};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, field};

    type TestResult = Result<(), Box<dyn Error>>;

    /// The ASCII daily transaction file. `CVTRA06Y` has the widths and offsets
    /// of `CVTRA05Y`, and `CBTRN02C` moves each daily field to its `TRAN-`
    /// field, so its records are `TRAN-RECORD` bytes.
    const DAILYTRAN_ASCII: &str = include_str!("../../../../app/data/ASCII/dailytran.txt");

    /// The EBCDIC counterpart: the same 300 records as 350-byte CP037 blocks.
    const DAILYTRAN_EBCDIC: &[u8] =
        include_bytes!("../../../../app/data/EBCDIC/AWS.M2.CARDDEMO.DALYTRAN.PS");

    /// The fields of `dailytran.txt` line 2, each with its width, written out
    /// independently of the layout constants.
    const LINE_2_FIELDS: [(&str, usize); 14] = [
        ("0000000001774260", 16),
        ("03", 2),
        ("0001", 4),
        ("OPERATOR", 10),
        ("Return item at Nitzsche, Nicolas and Lowe", 100),
        ("0000009190}", 11),
        ("800000000", 9),
        ("Nitzsche, Nicolas and Lowe", 50),
        ("Fidelshire", 50),
        ("53378", 10),
        ("0927987108636232", 16),
        ("2022-06-10 19:27:53.000000", 26),
        ("", 26),
        ("", 20),
    ];

    /// The record built from `fields`, each value right-padded with spaces to
    /// its width.
    fn padded(fields: &[(&str, usize)]) -> Vec<u8> {
        fields
            .iter()
            .flat_map(|&(text, width)| format!("{text:<width$}").into_bytes())
            .collect()
    }

    /// A transaction as POSTTRAN posts it: positive amount, category 5 and
    /// merchant 0 (zero-padded digits), stamped with a processing timestamp.
    fn posted_transaction() -> Result<NewTransaction, Box<dyn Error>> {
        Ok(NewTransaction {
            tran_id: TranId::move_from("0000000000683580"),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: TranCategoryCode::new(5)?,
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: Money::from_cents(50_477)?,
            tran_merchant_id: MerchantId::new(0)?,
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from("4859452612877065"),
            tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            tran_proc_ts: Alnum::move_from("2022-07-01 12:00:00.000000"),
        })
    }

    #[test]
    fn fields_tile_the_350_byte_record() {
        assert_eq!(
            (TransactionLayout::NAME, TransactionLayout::LEN),
            ("CVTRA05Y", 350)
        );
        assert_eq!(TransactionLayout::FIELDS.len(), 14);
        assert!(field::fields_are_contiguous(TransactionLayout::FIELDS, 350));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_350_bytes() {
        for len in [0, 349, 351] {
            for charset in [Charset::Ascii, Charset::Ebcdic] {
                assert_eq!(
                    TransactionLayout::decode(&vec![b' '; len], charset),
                    Err(CodecError::new(
                        "CVTRA05Y",
                        CodecErrorKind::Length {
                            expected: 350,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    #[test]
    fn dailytran_line_2_decodes_to_its_fields_and_re_encodes_to_its_bytes() -> TestResult {
        let bytes = padded(&LINE_2_FIELDS);
        assert_eq!(
            DAILYTRAN_ASCII.lines().nth(1).map(str::as_bytes),
            Some(bytes.as_slice())
        );
        let record = TransactionLayout::decode(&bytes, Charset::Ascii)?;
        let transaction = &record.transaction;
        assert_eq!(transaction.tran_id, TranId::move_from("0000000001774260"));
        assert_eq!(transaction.tran_type_cd, TranTypeCode::move_from("03"));
        assert_eq!(transaction.tran_cat_cd, TranCategoryCode::new(1)?);
        assert_eq!(transaction.tran_source, Alnum::move_from("OPERATOR"));
        assert_eq!(
            transaction.tran_desc,
            Alnum::move_from("Return item at Nitzsche, Nicolas and Lowe")
        );
        assert_eq!(transaction.tran_amt.cents(), -91_900);
        assert_eq!(transaction.tran_merchant_id, MerchantId::new(800_000_000)?);
        assert_eq!(
            transaction.tran_merchant_name,
            Alnum::move_from("Nitzsche, Nicolas and Lowe")
        );
        assert_eq!(
            transaction.tran_merchant_city,
            Alnum::move_from("Fidelshire")
        );
        assert_eq!(transaction.tran_merchant_zip, Alnum::move_from("53378"));
        assert_eq!(
            transaction.tran_card_num,
            CardNumber::move_from("0927987108636232")
        );
        assert_eq!(
            transaction.tran_orig_ts,
            Alnum::move_from("2022-06-10 19:27:53.000000")
        );
        assert_eq!(transaction.tran_proc_ts, Alnum::spaces());
        assert_eq!(record.filler, Alnum::spaces());
        assert!(record.preserved.is_empty());
        assert_eq!(TransactionLayout::encode(&record, Charset::Ascii), bytes);
        Ok(())
    }

    #[test]
    fn dailytran_line_2_is_its_ebcdic_record() -> TestResult {
        let stored = DAILYTRAN_EBCDIC
            .chunks(TransactionLayout::LEN)
            .nth(1)
            .ok_or("the EBCDIC file has no second record")?;
        let record = TransactionLayout::decode(&padded(&LINE_2_FIELDS), Charset::Ascii)?;
        assert_eq!(TransactionLayout::encode(&record, Charset::Ebcdic), stored);
        assert_eq!(TransactionLayout::decode(stored, Charset::Ebcdic)?, record);
        Ok(())
    }

    #[test]
    fn posted_record_round_trips_in_both_charsets() -> TestResult {
        let record = TransactionRecord::new(posted_transaction()?);
        assert_eq!(record.filler, Alnum::spaces());
        assert!(record.preserved.is_empty());
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let bytes = TransactionLayout::encode(&record, charset);
            assert_eq!(bytes.len(), 350, "{charset:?}");
            assert_eq!(
                TransactionLayout::decode(&bytes, charset)?,
                record,
                "{charset:?}"
            );
        }
        let ascii = TransactionLayout::encode(&record, Charset::Ascii);
        let windows: [(usize, usize, &[u8]); 5] = [
            (18, 22, b"0005"),
            (132, 143, b"0000005047G"),
            (143, 152, b"000000000"),
            (304, 330, b"2022-07-01 12:00:00.000000"),
            (330, 350, b"                    "),
        ];
        for (start, end, expected) in windows {
            assert_eq!(
                ascii.get(start..end),
                Some(expected),
                "bytes {start}..{end}"
            );
        }
        Ok(())
    }

    #[test]
    fn negative_zero_amount_is_preserved_and_re_encoded_as_stored() -> TestResult {
        let mut fields = LINE_2_FIELDS;
        fields[5] = ("0000000000}", 11);
        let bytes = padded(&fields);
        for charset in [Charset::Ascii, Charset::Ebcdic] {
            let stored = charset.from_internal(&bytes);
            let record = TransactionLayout::decode(&stored, charset)?;
            assert!(record.transaction.tran_amt.is_zero(), "{charset:?}");
            assert_eq!(record.preserved.len(), 1, "{charset:?}");
            assert_eq!(
                record.preserved.get(132).map(|raw| raw.field),
                Some("TRAN-AMT")
            );
            assert_eq!(TransactionLayout::encode(&record, charset), stored);
        }
        Ok(())
    }

    #[test]
    fn invalid_numeric_bytes_are_located_at_their_field() {
        let cases: [(usize, &str, CodecErrorKind, &str); 3] = [
            (
                2,
                "00A1",
                CodecErrorKind::NotNumeric { byte: b'A' },
                TRAN_CAT_CD.name,
            ),
            (
                5,
                "00000091900",
                CodecErrorKind::PlainDigitInSigned { byte: b'0' },
                TRAN_AMT.name,
            ),
            (
                6,
                "80000000 ",
                CodecErrorKind::NotNumeric { byte: b' ' },
                "TRAN-MERCHANT-ID",
            ),
        ];
        for (index, value, kind, name) in cases {
            let mut fields = LINE_2_FIELDS;
            fields[index].0 = value;
            let error = TransactionLayout::decode(&padded(&fields), Charset::Ascii);
            assert_eq!(
                error.map_err(|e| (e.kind, e.field)),
                Err((kind, Some(name))),
                "{value:?}"
            );
        }
    }
}
