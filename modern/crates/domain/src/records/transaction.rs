//! `CVTRA05Y` `TRAN-RECORD`, the 350-byte transaction record (`CVTRA05Y.cpy:4-18`;
//! CVTRA05Y-001 to CVTRA05Y-014).
//!
//! It is the row shape of table `transactions` (migration 0002), the record of the SYSTRAN file
//! `intcalc` writes, and, byte for byte, the layout of `CVTRA06Y` `DALYTRAN-RECORD`
//! (`CVTRA06Y.cpy:4-18`; CVTRA06Y-001 to CVTRA06Y-014). The two copybooks differ only in their
//! field names, so they share one field routine: `decode_tran_layout` and `encode_tran_layout`
//! read and write the fourteen fields in copybook order, and a `TranFieldNames` table supplies
//! the names a decode error reports. [`TransactionRecord`]'s [`FixedRecord`] implementation
//! passes the `TRAN-` names; `daily_transaction` passes its `DALYTRAN-` names.
//!
//! # Timestamps
//!
//! `TRAN-ORIG-TS` and `TRAN-PROC-TS` are [`PicTimestamp`] values, never text (Typed Dates and
//! Timestamps). Twenty-six spaces are `None` (PIC-017). Every other value carries the layout code
//! of the text it was read from, and encoding rewrites that text from the value and the code, so
//! the round trip is lossless whichever writer produced the field (E33):
//!
//! | Writer | `TRAN-ORIG-TS` | `TRAN-PROC-TS` |
//! |---|---|---|
//! | CBTRN02C (`CBTRN02C.cbl:436-438`) | `I`, copied from `DALYTRAN-ORIG-TS` (PIC-018) | `D`, `DB2-FORMAT-TS` (PIC-019) |
//! | CBACT04C (`CBACT04C.cbl:497-498`) | `D`, `DB2-FORMAT-TS` | `D`, `DB2-FORMAT-TS` |
//! | COTRN02C (`COTRN02C.cbl:464-465`) | `S`, the screen date (PIC-020) | `S`, the screen date |
//! | COBIL00C (`COBIL00C.cbl:231-232`) | `I`, `WS-TIMESTAMP` | `I`, `WS-TIMESTAMP` |
//!
//! Text in no known layout, a partly blank value or an impossible date or time is a typed error
//! naming the field, never a normalised value (PIC-021, R7).
//!
//! # Card number
//!
//! `TRAN-CARD-NUM` is a [`CardNumber`], so the derived `Debug` of a record shows only the
//! redacted form (PIC-024, E32, Cardholder Data Redaction). Encoding is the one place this module
//! reads the raw bytes, through [`CardNumber::expose`].

use copybook::{Error, FixedRecord, PicTimestamp, PicU, PicX, RecordReader, RecordWriter};

use super::{put_zoned_money, zoned_money};
use crate::money::Money;
use crate::sensitive::CardNumber;

/// One record of `CVTRA05Y` `TRAN-RECORD`, its fields in copybook order (CVTRA05Y-001 to
/// CVTRA05Y-014).
///
/// The record holds no business rule: `TRAN-AMT` is a [`Money<9>`](Money) on which this module
/// performs no arithmetic, and every field keeps the bytes codec A decoded it from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransactionRecord {
    /// `TRAN-ID PIC X(16)`, offset 0: the record key, KSDS `KEYS(16 0)` and the primary key of
    /// `transactions` (CVTRA05Y-001).
    pub id: PicX<16>,
    /// `TRAN-TYPE-CD PIC X(02)`, offset 16 (CVTRA05Y-002).
    pub type_cd: PicX<2>,
    /// `TRAN-CAT-CD PIC 9(04)`, offset 18 (CVTRA05Y-003).
    pub cat_cd: PicU<4>,
    /// `TRAN-SOURCE PIC X(10)`, offset 22 (CVTRA05Y-004).
    pub source: PicX<10>,
    /// `TRAN-DESC PIC X(100)`, offset 32 (CVTRA05Y-005).
    pub desc: PicX<100>,
    /// `TRAN-AMT PIC S9(09)V99`, offset 132: eleven bytes of signed DISPLAY with a trailing
    /// overpunch sign (CVTRA05Y-006, PIC-005, PIC-006).
    pub amt: Money<9>,
    /// `TRAN-MERCHANT-ID PIC 9(09)`, offset 143 (CVTRA05Y-007).
    pub merchant_id: PicU<9>,
    /// `TRAN-MERCHANT-NAME PIC X(50)`, offset 152 (CVTRA05Y-008).
    pub merchant_name: PicX<50>,
    /// `TRAN-MERCHANT-CITY PIC X(50)`, offset 202 (CVTRA05Y-009).
    pub merchant_city: PicX<50>,
    /// `TRAN-MERCHANT-ZIP PIC X(10)`, offset 252 (CVTRA05Y-010).
    pub merchant_zip: PicX<10>,
    /// `TRAN-CARD-NUM PIC X(16)`, offset 262: sensitive, its `Debug` twelve `*` and the last four
    /// characters (CVTRA05Y-011, PIC-024).
    pub card_num: CardNumber,
    /// `TRAN-ORIG-TS PIC X(26)`, offset 278: blank, or layout `I`, `D` or `S` (CVTRA05Y-012,
    /// PIC-017 to PIC-021).
    pub orig_ts: PicTimestamp,
    /// `TRAN-PROC-TS PIC X(26)`, offset 304: blank, or layout `I`, `D` or `S`; the key of the VSAM
    /// alternate index `KEYS(26 304)` (`TRANIDX.jcl:25-27`), now the index on
    /// `transactions.proc_ts` (CVTRA05Y-013, PIC-017 to PIC-021).
    pub proc_ts: PicTimestamp,
    /// `FILLER PIC X(20)`, offset 330, kept byte for byte (CVTRA05Y-014, PIC-012).
    pub filler: PicX<20>,
}

/// The copybook names of the fourteen fields of a 350-byte transaction layout, in copybook
/// order: the `field` each decode error of [`decode_tran_layout`] reports.
///
/// `CVTRA05Y` and `CVTRA06Y` differ only in these names, `TRAN-` against `DALYTRAN-`, so each
/// layout passes its own table to the shared routine (CVTRA06Y-001 to CVTRA06Y-014).
pub(super) struct TranFieldNames {
    /// `TRAN-ID`, offset 0.
    pub(super) id: &'static str,
    /// `TRAN-TYPE-CD`, offset 16.
    pub(super) type_cd: &'static str,
    /// `TRAN-CAT-CD`, offset 18.
    pub(super) cat_cd: &'static str,
    /// `TRAN-SOURCE`, offset 22.
    pub(super) source: &'static str,
    /// `TRAN-DESC`, offset 32.
    pub(super) desc: &'static str,
    /// `TRAN-AMT`, offset 132.
    pub(super) amt: &'static str,
    /// `TRAN-MERCHANT-ID`, offset 143.
    pub(super) merchant_id: &'static str,
    /// `TRAN-MERCHANT-NAME`, offset 152.
    pub(super) merchant_name: &'static str,
    /// `TRAN-MERCHANT-CITY`, offset 202.
    pub(super) merchant_city: &'static str,
    /// `TRAN-MERCHANT-ZIP`, offset 252.
    pub(super) merchant_zip: &'static str,
    /// `TRAN-CARD-NUM`, offset 262.
    pub(super) card_num: &'static str,
    /// `TRAN-ORIG-TS`, offset 278.
    pub(super) orig_ts: &'static str,
    /// `TRAN-PROC-TS`, offset 304.
    pub(super) proc_ts: &'static str,
    /// The unnamed `FILLER`, offset 330.
    pub(super) filler: &'static str,
}

/// The field names of `CVTRA05Y` `TRAN-RECORD` (`CVTRA05Y.cpy:5-18`).
pub(super) const TRAN_NAMES: TranFieldNames = TranFieldNames {
    id: "TRAN-ID",
    type_cd: "TRAN-TYPE-CD",
    cat_cd: "TRAN-CAT-CD",
    source: "TRAN-SOURCE",
    desc: "TRAN-DESC",
    amt: "TRAN-AMT",
    merchant_id: "TRAN-MERCHANT-ID",
    merchant_name: "TRAN-MERCHANT-NAME",
    merchant_city: "TRAN-MERCHANT-CITY",
    merchant_zip: "TRAN-MERCHANT-ZIP",
    card_num: "TRAN-CARD-NUM",
    orig_ts: "TRAN-ORIG-TS",
    proc_ts: "TRAN-PROC-TS",
    filler: "FILLER",
};

/// Decodes the fourteen fields of a 350-byte transaction layout from `bytes`, left to right in
/// copybook order, each failure reported under its name in `names` (CVTRA05Y-001 to
/// CVTRA05Y-014). The struct literal lists the fields in that order, so it reads them in that
/// order, and the reader owns every offset and bounds check: the decode is total for any slice
/// (Total Parsers Over Untrusted Input).
///
/// # Errors
///
/// [`ErrorKind::WidthMismatch`](copybook::ErrorKind::WidthMismatch) for field `"record"` when
/// `bytes` is not 350 bytes long. Otherwise the first field that fails, at its offset and width:
/// [`NonPrintable`](copybook::ErrorKind::NonPrintable) in a `PIC X(n)` field, the card number and
/// the `FILLER` included (PIC-002); [`NonDigit`](copybook::ErrorKind::NonDigit) in `TRAN-CAT-CD`
/// or `TRAN-MERCHANT-ID` (PIC-004); the signed DISPLAY classes of `TRAN-AMT`, a plain-digit sign
/// among them (PIC-007 to PIC-009); and
/// [`UnknownTimestampLayout`](copybook::ErrorKind::UnknownTimestampLayout) or
/// [`InvalidTimestamp`](copybook::ErrorKind::InvalidTimestamp) in either timestamp (PIC-021).
pub(super) fn decode_tran_layout(
    bytes: &[u8],
    names: &TranFieldNames,
) -> Result<TransactionRecord, Error> {
    let mut r = RecordReader::new(bytes, TransactionRecord::WIDTH)?;
    let record = TransactionRecord {
        id: r.pic_x(names.id)?,
        type_cd: r.pic_x(names.type_cd)?,
        cat_cd: r.pic_u(names.cat_cd)?,
        source: r.pic_x(names.source)?,
        desc: r.pic_x(names.desc)?,
        amt: zoned_money(&mut r, names.amt)?,
        merchant_id: r.pic_u(names.merchant_id)?,
        merchant_name: r.pic_x(names.merchant_name)?,
        merchant_city: r.pic_x(names.merchant_city)?,
        merchant_zip: r.pic_x(names.merchant_zip)?,
        card_num: CardNumber::new(r.pic_x(names.card_num)?),
        orig_ts: r.timestamp(names.orig_ts)?,
        proc_ts: r.timestamp(names.proc_ts)?,
        filler: r.pic_x(names.filler)?,
    };
    r.finish()?;
    Ok(record)
}

/// Encodes `record` as the 350 bytes of a transaction layout, the mirror of
/// [`decode_tran_layout`]: `PIC X(n)` bytes and the `FILLER` unchanged, leading zeros restored,
/// the overpunch sign rebuilt from the amount (PIC-006), and each timestamp rewritten from its
/// value and layout code (PIC-017 to PIC-020). The card number is read through
/// [`CardNumber::expose`], its one raw use in this module.
#[must_use]
pub(super) fn encode_tran_layout(record: &TransactionRecord) -> Vec<u8> {
    let mut w = RecordWriter::with_capacity(TransactionRecord::WIDTH);
    w.pic_x(&record.id)
        .pic_x(&record.type_cd)
        .pic_u(record.cat_cd)
        .pic_x(&record.source)
        .pic_x(&record.desc);
    put_zoned_money(&mut w, &record.amt)
        .pic_u(record.merchant_id)
        .pic_x(&record.merchant_name)
        .pic_x(&record.merchant_city)
        .pic_x(&record.merchant_zip)
        .pic_x(record.card_num.expose())
        .timestamp(&record.orig_ts)
        .timestamp(&record.proc_ts)
        .pic_x(&record.filler);
    w.finish()
}

// `TRAILING_FILLER` keeps its default of 0 although the layout ends in `FILLER PIC X(20)`: no
// fixture line holds this record (PIC-013 lists no `CVTRA05Y` input), so a short body is an
// anomaly raised as a typed error, never a tail supplied as spaces (R7).
impl FixedRecord for TransactionRecord {
    /// `RECLN = 350` (`CVTRA05Y.cpy:2`), the KSDS `RECORDSIZE(350 350)` (CVTRA05Y-014).
    const WIDTH: usize = 350;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        decode_tran_layout(bytes, &TRAN_NAMES)
    }

    fn encode(&self) -> Vec<u8> {
        encode_tran_layout(self)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{Error, ErrorKind, FixedRecord, PicTimestamp, TsForm};

    use super::{
        TRAN_NAMES, TranFieldNames, TransactionRecord, decode_tran_layout, encode_tran_layout,
    };
    use crate::money::{Decimal, Money};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// `TRAN-CARD-NUM` of [`line_one`].
    const CARD: &str = "4859452612877065";

    /// Offset of `TRAN-AMT` (CVTRA05Y-006).
    const AMT_AT: usize = 132;
    /// Offset of `TRAN-ORIG-TS` (CVTRA05Y-012).
    const ORIG_TS_AT: usize = 278;
    /// Offset of `TRAN-PROC-TS` (CVTRA05Y-013).
    const PROC_TS_AT: usize = 304;

    /// `DALYTRAN-ORIG-TS` of `dailytran.txt` line 1, layout `I` (PIC-018).
    const ISO_TS: &[u8; 26] = b"2022-06-10 19:27:53.000000";
    /// A `DB2-FORMAT-TS` value, layout `D`: two hundredths digits, then `0000` (PIC-019).
    const DB2_TS: &[u8; 26] = b"2022-07-18-10.15.42.370000";
    /// A COTRN02C screen date followed by 16 spaces, layout `S` (PIC-020).
    const SCREEN_TS: &[u8; 26] = b"2022-07-18                ";

    /// `text` followed by spaces to `N` bytes.
    fn padded<const N: usize>(text: &[u8]) -> Vec<u8> {
        let mut field = text.to_vec();
        field.resize(N, b' ');
        field
    }

    /// The 350 bytes of `dailytran.txt` line 1, the `CVTRA06Y` twin of this layout: layout `I`
    /// in `TRAN-ORIG-TS`, a blank `TRAN-PROC-TS` and a blank `FILLER`.
    fn line_one() -> Vec<u8> {
        let fields: [&[u8]; 14] = [
            b"0000000000683580",
            b"01",
            b"0001",
            &padded::<10>(b"POS TERM"),
            &padded::<100>(b"Purchase at Abshire-Lowe"),
            b"0000005047G",
            b"800000000",
            &padded::<50>(b"Abshire-Lowe"),
            &padded::<50>(b"North Enoshaven"),
            &padded::<10>(b"72112"),
            CARD.as_bytes(),
            ISO_TS,
            &[b' '; 26],
            &[b' '; 20],
        ];
        fields.concat()
    }

    /// `bytes` with `field` written over its bytes from `offset`.
    fn replaced(bytes: &[u8], offset: usize, field: &[u8]) -> Result<Vec<u8>, &'static str> {
        let end = offset
            .checked_add(field.len())
            .ok_or("field end overflows")?;
        let mut out = bytes.to_vec();
        out.get_mut(offset..end)
            .ok_or("field beyond the record")?
            .copy_from_slice(field);
        Ok(out)
    }

    /// [`line_one`] with `orig_ts` and `proc_ts` in its two timestamp fields.
    fn with_timestamps(orig_ts: &[u8; 26], proc_ts: &[u8; 26]) -> Result<Vec<u8>, &'static str> {
        let with_origin = replaced(&line_one(), ORIG_TS_AT, orig_ts)?;
        replaced(&with_origin, PROC_TS_AT, proc_ts)
    }

    /// The instant `date` at `time` and `micros` microseconds.
    fn at(
        date: (i32, u32, u32),
        time: (u32, u32, u32),
        micros: u32,
    ) -> Result<NaiveDateTime, &'static str> {
        let (year, month, day) = date;
        let (hour, minute, second) = time;
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_micro_opt(hour, minute, second, micros))
            .ok_or("no such instant")
    }

    /// The layout code and value of a timestamp field, `None` when it is blank.
    fn form_and_value(field: PicTimestamp) -> Option<(TsForm, NaiveDateTime)> {
        field.value().map(|stamp| (stamp.form(), stamp.value()))
    }

    // audit: CVTRA05Y-001
    // audit: CVTRA05Y-003
    // audit: CVTRA05Y-006
    // audit: CVTRA05Y-007
    // audit: CVTRA05Y-011
    // audit: CVTRA05Y-012
    // audit: CVTRA05Y-013
    // audit: CVTRA05Y-014
    // audit: PIC-017
    // audit: PIC-018
    #[test]
    fn layout_i_origin_and_blank_processing_timestamp_round_trip() -> TestResult {
        let bytes = line_one();
        let record = TransactionRecord::decode(&bytes)?;
        assert_eq!(record.id.as_bytes(), b"0000000000683580");
        assert_eq!(record.type_cd.as_bytes(), b"01");
        assert_eq!(record.cat_cd.get(), 1);
        assert_eq!(record.source.as_bytes(), b"POS TERM  ");
        assert_eq!(record.desc.to_text(), "Purchase at Abshire-Lowe");
        assert_eq!(record.amt, Money::<9>::new(Decimal::new(50_477, 2))?);
        assert_eq!(record.merchant_id.get(), 800_000_000);
        assert_eq!(record.merchant_name.to_text(), "Abshire-Lowe");
        assert_eq!(record.merchant_city.to_text(), "North Enoshaven");
        assert_eq!(record.merchant_zip.as_bytes(), b"72112     ");
        assert_eq!(record.card_num.expose().as_bytes(), CARD.as_bytes());
        assert_eq!(
            form_and_value(record.orig_ts),
            Some((TsForm::Iso, at((2022, 6, 10), (19, 27, 53), 0)?)),
        );
        assert_eq!(record.proc_ts, PicTimestamp::BLANK);
        assert_eq!(record.proc_ts.value(), None);
        assert!(record.filler.is_spaces());
        let encoded = record.encode();
        assert_eq!(encoded.len(), TransactionRecord::WIDTH);
        assert_eq!(encoded, bytes);
        Ok(())
    }

    // audit: PIC-018
    // audit: PIC-019
    // audit: PIC-020
    #[test]
    fn every_writer_layout_pair_round_trips_with_its_layout_codes() -> TestResult {
        let iso = (TsForm::Iso, at((2022, 6, 10), (19, 27, 53), 0)?);
        let db2 = (TsForm::Db2, at((2022, 7, 18), (10, 15, 42), 370_000)?);
        let screen = (TsForm::ScreenDate, at((2022, 7, 18), (0, 0, 0), 0)?);
        let cases = [
            // CBTRN02C: the daily record's origin, then `DB2-FORMAT-TS` (`CBTRN02C.cbl:436-438`).
            (ISO_TS, DB2_TS, iso, db2),
            // CBACT04C: `DB2-FORMAT-TS` in both (`CBACT04C.cbl:497-498`).
            (DB2_TS, DB2_TS, db2, db2),
            // COTRN02C: the two screen dates (`COTRN02C.cbl:464-465`).
            (SCREEN_TS, SCREEN_TS, screen, screen),
            // COBIL00C: `WS-TIMESTAMP` in both (`COBIL00C.cbl:231-232`).
            (ISO_TS, ISO_TS, iso, iso),
        ];
        for (orig_text, proc_text, orig, proc) in cases {
            let bytes = with_timestamps(orig_text, proc_text)?;
            let record = TransactionRecord::decode(&bytes)?;
            assert_eq!(form_and_value(record.orig_ts), Some(orig));
            assert_eq!(form_and_value(record.proc_ts), Some(proc));
            assert_eq!(record.encode(), bytes);
        }
        Ok(())
    }

    // audit: PIC-021
    #[test]
    fn timestamp_text_in_no_known_layout_is_a_typed_error_at_its_field() -> TestResult {
        let unknown = ErrorKind::UnknownTimestampLayout;
        let invalid = ErrorKind::InvalidTimestamp;
        let orig = ("TRAN-ORIG-TS", ORIG_TS_AT);
        let proc = ("TRAN-PROC-TS", PROC_TS_AT);
        let cases: [((&str, usize), &[u8; 26], ErrorKind); 5] = [
            // A `T` between date and time is no layout of PIC-018 to PIC-020.
            (orig, b"2022-06-10T19:27:53.000000", unknown),
            // Layout `I` naming hour 25.
            (proc, b"2022-06-10 25:27:53.000000", invalid),
            // The Db2 date-time separator with the `I` time separators.
            (orig, b"2022-06-10-19:27:53.000000", invalid),
            // A partly blank value: the clock ends after the minutes.
            (proc, b"2022-06-10 19:27          ", invalid),
            // A screen date naming 30 February.
            (orig, b"2022-02-30                ", invalid),
        ];
        for ((field, offset), text, kind) in cases {
            let bytes = replaced(&line_one(), offset, text)?;
            let error = TransactionRecord::decode(&bytes).err();
            assert_eq!(error, Some(Error::new(field, offset, 26, kind)));
            assert!(matches!(
                error,
                Some(Error {
                    kind: ErrorKind::UnknownTimestampLayout | ErrorKind::InvalidTimestamp,
                    ..
                })
            ));
        }
        Ok(())
    }

    // audit: PIC-007
    // audit: PIC-008
    #[test]
    fn amount_sign_anomalies_are_typed_errors_at_tran_amt() -> TestResult {
        let cases: [(&[u8; 11], ErrorKind); 2] = [
            (b"00000050477", ErrorKind::PlainDigitSign),
            (b"0000000000}", ErrorKind::NegativeZero),
        ];
        for (amount, kind) in cases {
            let bytes = replaced(&line_one(), AMT_AT, amount)?;
            assert_eq!(
                TransactionRecord::decode(&bytes),
                Err(Error::new("TRAN-AMT", AMT_AT, 11, kind)),
            );
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-004
    // audit: PIC-009
    #[test]
    fn a_control_byte_at_each_field_start_names_its_field_offset_and_width() -> TestResult {
        let alphanumeric = ErrorKind::NonPrintable;
        let numeric = ErrorKind::NonDigit;
        let timestamp = ErrorKind::InvalidTimestamp;
        let fields: [(&str, usize, usize, ErrorKind); 14] = [
            ("TRAN-ID", 0, 16, alphanumeric),
            ("TRAN-TYPE-CD", 16, 2, alphanumeric),
            ("TRAN-CAT-CD", 18, 4, numeric),
            ("TRAN-SOURCE", 22, 10, alphanumeric),
            ("TRAN-DESC", 32, 100, alphanumeric),
            ("TRAN-AMT", AMT_AT, 11, numeric),
            ("TRAN-MERCHANT-ID", 143, 9, numeric),
            ("TRAN-MERCHANT-NAME", 152, 50, alphanumeric),
            ("TRAN-MERCHANT-CITY", 202, 50, alphanumeric),
            ("TRAN-MERCHANT-ZIP", 252, 10, alphanumeric),
            ("TRAN-CARD-NUM", 262, 16, alphanumeric),
            ("TRAN-ORIG-TS", ORIG_TS_AT, 26, timestamp),
            ("TRAN-PROC-TS", PROC_TS_AT, 26, timestamp),
            ("FILLER", 330, 20, alphanumeric),
        ];
        let valid = line_one();
        for (field, offset, width, kind) in fields {
            let bytes = replaced(&valid, offset, &[0x01])?;
            assert_eq!(
                TransactionRecord::decode(&bytes),
                Err(Error::new(field, offset, width, kind)),
            );
        }
        Ok(())
    }

    // audit: CVTRA06Y-012
    #[test]
    fn the_shared_routine_reports_the_names_of_the_table_it_is_given() -> TestResult {
        const DALYTRAN_STAMPS: TranFieldNames = TranFieldNames {
            orig_ts: "DALYTRAN-ORIG-TS",
            proc_ts: "DALYTRAN-PROC-TS",
            ..TRAN_NAMES
        };
        let valid = line_one();
        let record = decode_tran_layout(&valid, &DALYTRAN_STAMPS)?;
        assert_eq!(record, decode_tran_layout(&valid, &TRAN_NAMES)?);
        assert_eq!(encode_tran_layout(&record), valid);

        let bytes = replaced(&valid, ORIG_TS_AT, b"2022-06-10T19:27:53.000000")?;
        assert_eq!(
            decode_tran_layout(&bytes, &DALYTRAN_STAMPS),
            Err(Error::new(
                "DALYTRAN-ORIG-TS",
                ORIG_TS_AT,
                26,
                ErrorKind::UnknownTimestampLayout,
            )),
        );
        Ok(())
    }

    #[test]
    fn every_other_length_is_a_record_width_error_never_a_panic() -> TestResult {
        let input = line_one().repeat(3);
        for len in 0..=input.len() {
            let slice = input.get(..len).ok_or("prefix beyond the input")?;
            let decoded = TransactionRecord::decode(slice);
            if len == TransactionRecord::WIDTH {
                assert_eq!(decoded?.encode(), line_one());
            } else {
                assert_eq!(
                    decoded,
                    Err(Error::new(
                        "record",
                        0,
                        TransactionRecord::WIDTH,
                        ErrorKind::WidthMismatch {
                            expected: TransactionRecord::WIDTH,
                            actual: len,
                        },
                    )),
                );
            }
        }
        Ok(())
    }

    // audit: PIC-024
    #[test]
    fn debug_shows_only_the_redacted_card_number() -> TestResult {
        let record = TransactionRecord::decode(&line_one())?;
        let debug = format!("{record:?}");
        assert!(!debug.contains(CARD));
        assert!(debug.contains("************7065"));
        Ok(())
    }
}
