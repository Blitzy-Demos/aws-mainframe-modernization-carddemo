//! `CVTRA06Y` `DALYTRAN-RECORD`, the 350-byte daily transaction record POSTTRAN reads
//! (`CVTRA06Y.cpy:4-18`; CVTRA06Y-001 to CVTRA06Y-014).
//!
//! The layout is `CVTRA05Y` `TRAN-RECORD` byte for byte under other field names: each of the
//! fourteen fields has the offset, width and PIC of its `TRAN-` namesake (CVTRA06Y-001 to
//! CVTRA06Y-014 against CVTRA05Y-001 to CVTRA05Y-014). [`DailyTransactionRecord`] therefore has
//! the fields of [`TransactionRecord`], and its codec is the shared routine of
//! [`transaction`](super::transaction), given the `DALYTRAN-` names so that a decode error names
//! the daily field rather than its `TRAN-` twin.
//!
//! The input is `app/data/ASCII/dailytran.txt`: 300 lines, each a full 350-byte body ended by LF
//! (FIXFRM-005). Every `DALYTRAN-ORIG-TS` is layout `I` and every `DALYTRAN-PROC-TS` is 26
//! spaces, so the record holds a typed timestamp and a blank one, never text (FIXFRM-017,
//! PIC-017, PIC-018, E33, Typed Dates and Timestamps). The record has no table of its own:
//! `posttran` reads it from the file, and codec B round-trips it through the `transactions` row
//! shape by the lossless `From` conversions below (d9).
//!
//! # Layout identity is not posting
//!
//! The `From` conversions copy all fourteen fields unchanged, because the two layouts are one.
//! CBTRN02C's `2000-POST-TRANSACTION` moves only twelve of them into `TRAN-RECORD`
//! (CBTRN02C-012): `TRAN-PROC-TS` takes `DB2-FORMAT-TS` instead of `DALYTRAN-PROC-TS`
//! (CBTRN02C-013), and no statement moves the `FILLER` (CBTRN02C-014). Building a posted
//! transaction is that program's rule in `domain::batch::cbtrn02c`, never these conversions.
//!
//! # Card number
//!
//! `DALYTRAN-CARD-NUM` is a [`CardNumber`], so the derived `Debug` of a record shows only the
//! redacted form (CVTRA06Y-011, PIC-024, Cardholder Data Redaction). The raw bytes are read only
//! by the shared encoder, through [`CardNumber::expose`].

use copybook::{Error, FixedRecord, PicTimestamp, PicU, PicX};

use super::transaction::{
    TranFieldNames, TransactionRecord, decode_tran_layout, encode_tran_layout,
};
use crate::money::Money;
use crate::sensitive::CardNumber;

/// One record of `CVTRA06Y` `DALYTRAN-RECORD`, its fields in copybook order (CVTRA06Y-001 to
/// CVTRA06Y-014).
///
/// The record holds no business rule: `DALYTRAN-AMT` is a [`Money<9>`](Money) on which this
/// module performs no arithmetic, and every field keeps the bytes codec A decoded it from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DailyTransactionRecord {
    /// `DALYTRAN-ID PIC X(16)`, offset 0: the transaction ID, moved unchanged to `TRAN-ID`
    /// when the record is posted (CVTRA06Y-001).
    pub id: PicX<16>,
    /// `DALYTRAN-TYPE-CD PIC X(02)`, offset 16 (CVTRA06Y-002).
    pub type_cd: PicX<2>,
    /// `DALYTRAN-CAT-CD PIC 9(04)`, offset 18 (CVTRA06Y-003).
    pub cat_cd: PicU<4>,
    /// `DALYTRAN-SOURCE PIC X(10)`, offset 22 (CVTRA06Y-004).
    pub source: PicX<10>,
    /// `DALYTRAN-DESC PIC X(100)`, offset 32 (CVTRA06Y-005).
    pub desc: PicX<100>,
    /// `DALYTRAN-AMT PIC S9(09)V99`, offset 132: eleven bytes of signed DISPLAY with a trailing
    /// overpunch sign (CVTRA06Y-006, PIC-005, PIC-006).
    pub amt: Money<9>,
    /// `DALYTRAN-MERCHANT-ID PIC 9(09)`, offset 143 (CVTRA06Y-007).
    pub merchant_id: PicU<9>,
    /// `DALYTRAN-MERCHANT-NAME PIC X(50)`, offset 152 (CVTRA06Y-008).
    pub merchant_name: PicX<50>,
    /// `DALYTRAN-MERCHANT-CITY PIC X(50)`, offset 202 (CVTRA06Y-009).
    pub merchant_city: PicX<50>,
    /// `DALYTRAN-MERCHANT-ZIP PIC X(10)`, offset 252 (CVTRA06Y-010).
    pub merchant_zip: PicX<10>,
    /// `DALYTRAN-CARD-NUM PIC X(16)`, offset 262: sensitive, its `Debug` twelve `*` and the last
    /// four characters; the key CBTRN02C looks up in the cross-reference (CVTRA06Y-011,
    /// PIC-024).
    pub card_num: CardNumber,
    /// `DALYTRAN-ORIG-TS PIC X(26)`, offset 278: layout `I` on every fixture line (CVTRA06Y-012,
    /// PIC-018); blank, `I`, `D` or `S` decode as for `TRAN-ORIG-TS` (PIC-017 to PIC-021).
    pub orig_ts: PicTimestamp,
    /// `DALYTRAN-PROC-TS PIC X(26)`, offset 304: 26 spaces on every fixture line, decoded as a
    /// blank timestamp (CVTRA06Y-013, PIC-017); any other text decodes as for `TRAN-PROC-TS`
    /// (PIC-018 to PIC-021).
    pub proc_ts: PicTimestamp,
    /// `FILLER PIC X(20)`, offset 330, kept byte for byte; all spaces in the fixture
    /// (CVTRA06Y-014, PIC-012).
    pub filler: PicX<20>,
}

/// The field names of `CVTRA06Y` `DALYTRAN-RECORD` (`CVTRA06Y.cpy:5-18`), so each decode error
/// of the shared routine names the daily field (CVTRA06Y-001 to CVTRA06Y-014).
const DALYTRAN_NAMES: TranFieldNames = TranFieldNames {
    id: "DALYTRAN-ID",
    type_cd: "DALYTRAN-TYPE-CD",
    cat_cd: "DALYTRAN-CAT-CD",
    source: "DALYTRAN-SOURCE",
    desc: "DALYTRAN-DESC",
    amt: "DALYTRAN-AMT",
    merchant_id: "DALYTRAN-MERCHANT-ID",
    merchant_name: "DALYTRAN-MERCHANT-NAME",
    merchant_city: "DALYTRAN-MERCHANT-CITY",
    merchant_zip: "DALYTRAN-MERCHANT-ZIP",
    card_num: "DALYTRAN-CARD-NUM",
    orig_ts: "DALYTRAN-ORIG-TS",
    proc_ts: "DALYTRAN-PROC-TS",
    filler: "FILLER",
};

// `TRAILING_FILLER` keeps its default of 0 although the layout ends in `FILLER PIC X(20)`: every
// `dailytran.txt` body is the full 350 bytes (FIXFRM-005), and a short body outside
// `cardxref.txt` is a typed error, never a tail supplied as spaces (FIXFRM-011, R7).
impl FixedRecord for DailyTransactionRecord {
    /// `RECLN = 350` (`CVTRA06Y.cpy:2`), the length of every `dailytran.txt` body (FIXFRM-005).
    /// It is the shared layout's width by construction, so the shared routine's width check is
    /// this record's.
    const WIDTH: usize = TransactionRecord::WIDTH;

    /// Decodes the fourteen fields through the shared routine under the `DALYTRAN-` names.
    ///
    /// # Errors
    ///
    /// Those of the shared `decode_tran_layout`: a record width error for field `"record"` when
    /// `bytes` is not 350 bytes long, otherwise the first field that fails, named by its
    /// `DALYTRAN-` name (or `FILLER`) with its offset and width (PIC-002, PIC-004, PIC-007 to
    /// PIC-009, PIC-021).
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        decode_tran_layout(bytes, &DALYTRAN_NAMES).map(Self::from)
    }

    /// Encodes the 350 bytes through the shared routine, so the bytes are those a
    /// [`TransactionRecord`] with the same fields encodes to.
    fn encode(&self) -> Vec<u8> {
        encode_tran_layout(&TransactionRecord::from(self.clone()))
    }
}

/// The daily record as the `transactions` row shape's record: every field moved to its `TRAN-`
/// namesake unchanged, `DALYTRAN-PROC-TS` and `FILLER` included, because the layouts are
/// identical (CVTRA06Y-001 to CVTRA06Y-014 against CVTRA05Y-001 to CVTRA05Y-014). This is codec
/// B's path for `dailytran.txt` (d9), not CBTRN02C's posting moves (CBTRN02C-012 to
/// CBTRN02C-014).
impl From<DailyTransactionRecord> for TransactionRecord {
    fn from(daily: DailyTransactionRecord) -> Self {
        let DailyTransactionRecord {
            id,
            type_cd,
            cat_cd,
            source,
            desc,
            amt,
            merchant_id,
            merchant_name,
            merchant_city,
            merchant_zip,
            card_num,
            orig_ts,
            proc_ts,
            filler,
        } = daily;
        Self {
            id,
            type_cd,
            cat_cd,
            source,
            desc,
            amt,
            merchant_id,
            merchant_name,
            merchant_city,
            merchant_zip,
            card_num,
            orig_ts,
            proc_ts,
            filler,
        }
    }
}

/// The inverse of `From<DailyTransactionRecord> for TransactionRecord`: every field moved to its
/// `DALYTRAN-` namesake unchanged (CVTRA06Y-001 to CVTRA06Y-014), so a daily record read back
/// from the `transactions` row shape equals the record written (d9).
impl From<TransactionRecord> for DailyTransactionRecord {
    fn from(record: TransactionRecord) -> Self {
        let TransactionRecord {
            id,
            type_cd,
            cat_cd,
            source,
            desc,
            amt,
            merchant_id,
            merchant_name,
            merchant_city,
            merchant_zip,
            card_num,
            orig_ts,
            proc_ts,
            filler,
        } = record;
        Self {
            id,
            type_cd,
            cat_cd,
            source,
            desc,
            amt,
            merchant_id,
            merchant_name,
            merchant_city,
            merchant_zip,
            card_num,
            orig_ts,
            proc_ts,
            filler,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::framing::{Framed, Line, Terminator};
    use copybook::{Error, ErrorKind, FixedRecord, PicTimestamp, TsForm};

    use super::DailyTransactionRecord;
    use crate::money::{Decimal, Money};
    use crate::records::TransactionRecord;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// `DALYTRAN-CARD-NUM` of [`line_one`].
    const CARD: &str = "4859452612877065";

    /// Offset of `DALYTRAN-AMT` (CVTRA06Y-006).
    const AMT_AT: usize = 132;
    /// Offset of `DALYTRAN-ORIG-TS` (CVTRA06Y-012).
    const ORIG_TS_AT: usize = 278;
    /// Offset of `DALYTRAN-PROC-TS` (CVTRA06Y-013).
    const PROC_TS_AT: usize = 304;
    /// Offset of `FILLER` (CVTRA06Y-014).
    const FILLER_AT: usize = 330;

    /// `DALYTRAN-ORIG-TS` of every `dailytran.txt` line, layout `I` (FIXFRM-017, PIC-018).
    const ORIG_TS: &[u8; 26] = b"2022-06-10 19:27:53.000000";

    /// `text` followed by spaces to `N` bytes.
    fn padded<const N: usize>(text: &[u8]) -> Vec<u8> {
        let mut field = text.to_vec();
        field.resize(N, b' ');
        field
    }

    /// The 350-byte body of `dailytran.txt` line 1, field by field in copybook order.
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
            ORIG_TS,
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

    /// The instant `date` at `time`, with no fraction.
    fn at(date: (i32, u32, u32), time: (u32, u32, u32)) -> Result<NaiveDateTime, &'static str> {
        let (year, month, day) = date;
        let (hour, minute, second) = time;
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(hour, minute, second))
            .ok_or("no such instant")
    }

    /// The layout code and value of a timestamp field, `None` when it is blank.
    fn form_and_value(field: PicTimestamp) -> Option<(TsForm, NaiveDateTime)> {
        field.value().map(|stamp| (stamp.form(), stamp.value()))
    }

    // audit: CVTRA06Y-001
    // audit: CVTRA06Y-003
    // audit: CVTRA06Y-006
    // audit: CVTRA06Y-007
    // audit: CVTRA06Y-011
    // audit: CVTRA06Y-012
    // audit: CVTRA06Y-013
    // audit: CVTRA06Y-014
    // audit: FIXFRM-017
    #[test]
    fn line_one_decodes_to_its_fields_and_re_encodes_byte_for_byte() -> TestResult {
        let bytes = line_one();
        assert_eq!(DailyTransactionRecord::WIDTH, 350);
        let record = DailyTransactionRecord::decode(&bytes)?;
        assert_eq!(record.id.as_bytes(), b"0000000000683580");
        assert_eq!(record.type_cd.as_bytes(), b"01");
        assert_eq!(record.cat_cd.get(), 1);
        assert_eq!(record.source.as_bytes(), b"POS TERM  ");
        assert_eq!(record.desc.to_text(), "Purchase at Abshire-Lowe");
        // `0000005047G`: ten digits, then `G`, a positive last digit 7 (PIC-005).
        assert_eq!(record.amt, Money::<9>::new(Decimal::new(50_477, 2))?);
        assert_eq!(record.merchant_id.get(), 800_000_000);
        assert_eq!(record.merchant_name.to_text(), "Abshire-Lowe");
        assert_eq!(record.merchant_city.to_text(), "North Enoshaven");
        assert_eq!(record.merchant_zip.as_bytes(), b"72112     ");
        assert_eq!(record.card_num.expose().as_bytes(), CARD.as_bytes());
        assert_eq!(
            form_and_value(record.orig_ts),
            Some((TsForm::Iso, at((2022, 6, 10), (19, 27, 53))?)),
        );
        assert_eq!(record.proc_ts, PicTimestamp::BLANK);
        assert_eq!(record.proc_ts.value(), None);
        assert!(record.filler.is_spaces());
        let encoded = record.encode();
        assert_eq!(encoded.len(), DailyTransactionRecord::WIDTH);
        assert_eq!(encoded, bytes);
        Ok(())
    }

    // audit: FIXFRM-005
    // audit: FIXFRM-011
    #[test]
    fn line_one_keeps_its_lf_framing_and_a_short_body_is_a_typed_error() -> TestResult {
        let body = line_one();
        let framed = Framed::<DailyTransactionRecord>::decode(Line::new(&body, Terminator::Lf))?;
        assert_eq!(framed.framing.terminator, Terminator::Lf);
        assert_eq!(framed.framing.body_len, 350);
        let mut line = body.clone();
        line.push(b'\n');
        assert_eq!(framed.encode(), line);

        // A body that stops inside the trailing `FILLER` is not supplied with spaces.
        let short = body.get(..349).ok_or("short body")?;
        assert_eq!(
            Framed::<DailyTransactionRecord>::decode(Line::new(short, Terminator::Lf)),
            Err(Error::new(
                "record",
                0,
                350,
                ErrorKind::ShortBodyNotFiller {
                    expected: 350,
                    actual: 349,
                },
            )),
        );
        Ok(())
    }

    // audit: CVTRA06Y-013
    // audit: CVTRA06Y-014
    #[test]
    fn both_from_conversions_are_lossless() -> TestResult {
        // Line 1, and line 1 with a layout `D` processing time and non-space `FILLER`, so the
        // two fields CBTRN02C's posting moves do not copy are carried too.
        let with_proc_ts = replaced(&line_one(), PROC_TS_AT, b"2022-07-01-12.00.00.000000")?;
        let with_filler = replaced(&with_proc_ts, FILLER_AT, &[b'0'; 20])?;
        let line = line_one();
        for bytes in [&line, &with_filler] {
            let daily = DailyTransactionRecord::decode(bytes)?;
            let tran = TransactionRecord::from(daily.clone());
            assert_eq!(tran, TransactionRecord::decode(bytes)?);
            assert_eq!(&tran.encode(), bytes);
            assert_eq!(DailyTransactionRecord::from(tran), daily);
        }
        let posted_layout = DailyTransactionRecord::decode(&with_filler)?;
        assert_eq!(
            form_and_value(posted_layout.proc_ts),
            Some((TsForm::Db2, at((2022, 7, 1), (12, 0, 0))?)),
        );
        assert_eq!(posted_layout.filler.as_bytes(), &[b'0'; 20]);
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-004
    // audit: PIC-009
    #[test]
    fn a_control_byte_at_each_field_start_names_its_dalytran_field() -> TestResult {
        let alphanumeric = ErrorKind::NonPrintable;
        let numeric = ErrorKind::NonDigit;
        let timestamp = ErrorKind::InvalidTimestamp;
        let fields: [(&str, usize, usize, ErrorKind); 14] = [
            ("DALYTRAN-ID", 0, 16, alphanumeric),
            ("DALYTRAN-TYPE-CD", 16, 2, alphanumeric),
            ("DALYTRAN-CAT-CD", 18, 4, numeric),
            ("DALYTRAN-SOURCE", 22, 10, alphanumeric),
            ("DALYTRAN-DESC", 32, 100, alphanumeric),
            ("DALYTRAN-AMT", AMT_AT, 11, numeric),
            ("DALYTRAN-MERCHANT-ID", 143, 9, numeric),
            ("DALYTRAN-MERCHANT-NAME", 152, 50, alphanumeric),
            ("DALYTRAN-MERCHANT-CITY", 202, 50, alphanumeric),
            ("DALYTRAN-MERCHANT-ZIP", 252, 10, alphanumeric),
            ("DALYTRAN-CARD-NUM", 262, 16, alphanumeric),
            ("DALYTRAN-ORIG-TS", ORIG_TS_AT, 26, timestamp),
            ("DALYTRAN-PROC-TS", PROC_TS_AT, 26, timestamp),
            ("FILLER", FILLER_AT, 20, alphanumeric),
        ];
        let valid = line_one();
        for (field, offset, width, kind) in fields {
            let bytes = replaced(&valid, offset, &[0x01])?;
            assert_eq!(
                DailyTransactionRecord::decode(&bytes),
                Err(Error::new(field, offset, width, kind)),
            );
        }
        Ok(())
    }

    // audit: PIC-007
    // audit: PIC-008
    // audit: PIC-021
    #[test]
    fn amount_and_timestamp_anomalies_are_typed_errors_at_the_daily_field() -> TestResult {
        let cases: [(&str, usize, &[u8], usize, ErrorKind); 4] = [
            (
                "DALYTRAN-AMT",
                AMT_AT,
                b"00000050477",
                11,
                ErrorKind::PlainDigitSign,
            ),
            (
                "DALYTRAN-AMT",
                AMT_AT,
                b"0000000000}",
                11,
                ErrorKind::NegativeZero,
            ),
            (
                "DALYTRAN-ORIG-TS",
                ORIG_TS_AT,
                b"2022-06-10T19:27:53.000000",
                26,
                ErrorKind::UnknownTimestampLayout,
            ),
            (
                "DALYTRAN-PROC-TS",
                PROC_TS_AT,
                b"2022-06-10 19:27          ",
                26,
                ErrorKind::InvalidTimestamp,
            ),
        ];
        for (field, offset, text, width, kind) in cases {
            let bytes = replaced(&line_one(), offset, text)?;
            let error = DailyTransactionRecord::decode(&bytes).err();
            assert_eq!(error, Some(Error::new(field, offset, width, kind)));
            assert!(
                matches!(error, Some(Error { field: name, .. }) if name.starts_with("DALYTRAN-"))
            );
        }
        Ok(())
    }

    #[test]
    fn every_other_length_is_a_record_width_error_never_a_panic() -> TestResult {
        let input = line_one().repeat(2);
        for len in 0..=input.len() {
            let slice = input.get(..len).ok_or("prefix beyond the input")?;
            let decoded = DailyTransactionRecord::decode(slice);
            if len == DailyTransactionRecord::WIDTH {
                assert_eq!(decoded?.encode(), line_one());
            } else {
                assert_eq!(
                    decoded,
                    Err(Error::new(
                        "record",
                        0,
                        DailyTransactionRecord::WIDTH,
                        ErrorKind::WidthMismatch {
                            expected: DailyTransactionRecord::WIDTH,
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
        let record = DailyTransactionRecord::decode(&line_one())?;
        let debug = format!("{record:?}");
        assert!(!debug.contains(CARD));
        assert!(debug.contains("************7065"));
        assert!(debug.starts_with("DailyTransactionRecord {"));
        Ok(())
    }
}
