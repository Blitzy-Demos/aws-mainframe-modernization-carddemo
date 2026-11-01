//! `CBTRN02C` `REJECT-RECORD`: the 430-byte `DALYREJS` reject record
//! [app/cbl/CBTRN02C.cbl:L176-182], written as `FD-REJS-RECORD`
//! [app/cbl/CBTRN02C.cbl:L81-84] to the generation `DALYREJS(+1)` at
//! `LRECL=430` [app/jcl/POSTTRAN.jcl:L34-38].
//!
//! `REJECT-RECORD` is no copybook: it is working storage of `CBTRN02C`, the
//! 350-byte `REJECT-TRAN-DATA` followed by the 80-byte `VALIDATION-TRAILER`
//! [app/cbl/CBTRN02C.cbl:L177-178]. `2500-WRITE-REJECT-REC` fills the trailer
//! from `WS-VALIDATION-TRAILER` [app/cbl/CBTRN02C.cbl:L180-182], so its 80
//! bytes are the 4-digit reason and the 76-byte description (P-CBTRN02C-012)
//! [app/cbl/CBTRN02C.cbl:L446-451]. The layout therefore lists the two
//! `WS-VALIDATION-TRAILER` fields at their trailer offsets, and has no
//! `FILLER`.
//!
//! | Field | PIC | Offset | Length | Audit row | Rust |
//! |---|---|---|---|---|---|
//! | `REJECT-TRAN-DATA` | `X(350)` | 0 | 350 | P-CBTRN02C-001 | `Alnum<350>` |
//! | `WS-VALIDATION-FAIL-REASON` | `9(04)` | 350 | 4 | P-CBTRN02C-002 | `NumericCode<4>` |
//! | `WS-VALIDATION-FAIL-REASON-DESC` | `X(76)` | 354 | 76 | P-CBTRN02C-003 | `Alnum<76>` |
//!
//! P-CBTRN02C-002 labels the reason type `RejectReason`; the domain spells it
//! [`NumericCode<4>`](NumericCode), the type of every unsigned 4-digit code.
//!
//! # The rejected record, byte for byte
//!
//! `MOVE DALYTRAN-RECORD TO REJECT-TRAN-DATA` [app/cbl/CBTRN02C.cbl:L447] is
//! an alphanumeric group move: it copies the 350 bytes of the `DALYTRAN` input
//! record as read, its `FILLER` and the stored sign of its amount included,
//! without interpreting a field. [`RejectRecord::new`] reproduces it by
//! re-encoding the decoded [`DailyTranRecord`], which gives back its stored
//! bytes, so the reject file holds each rejected `DALYTRAN` input record byte
//! for byte. The reason and the description are the ones `1500-VALIDATE-TRAN`
//! set (P-CBTRN02C-008 to P-CBTRN02C-011); their texts belong to the domain
//! messages, not to this codec.

use carddemo_domain::numeric::{Alnum, NumericCode, NumericError};

use super::daily_tran::{DailyTranLayout, DailyTranRecord};
use crate::Layout;
use crate::charset::Charset;
use crate::error::{CodecError, CodecErrorKind};
use crate::field::{self, FieldKind, FieldSpec};

/// `CBTRN02C` `REJECT-TRAN-DATA PIC X(350)`, the `DALYTRAN` record as read —
/// P-CBTRN02C-001 [app/cbl/CBTRN02C.cbl:L177].
pub const REJECT_TRAN_DATA: FieldSpec =
    FieldSpec::new("REJECT-TRAN-DATA", 0, 350, FieldKind::Alnum);

/// `CBTRN02C` `WS-VALIDATION-FAIL-REASON PIC 9(04)`, the first 4 bytes of
/// `VALIDATION-TRAILER` — P-CBTRN02C-002 [app/cbl/CBTRN02C.cbl:L181].
pub const WS_VALIDATION_FAIL_REASON: FieldSpec =
    FieldSpec::new("WS-VALIDATION-FAIL-REASON", 350, 4, FieldKind::Unsigned);

/// `CBTRN02C` `WS-VALIDATION-FAIL-REASON-DESC PIC X(76)`, the last 76 bytes of
/// `VALIDATION-TRAILER` — P-CBTRN02C-003 [app/cbl/CBTRN02C.cbl:L182].
pub const WS_VALIDATION_FAIL_REASON_DESC: FieldSpec =
    FieldSpec::new("WS-VALIDATION-FAIL-REASON-DESC", 354, 76, FieldKind::Alnum);

/// The `CBTRN02C` `REJECT-RECORD` layout, 430 bytes
/// [app/cbl/CBTRN02C.cbl:L176-182].
pub struct RejectRecordLayout;

/// One `DALYREJS` record: the rejected input record and the reason it was
/// rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectRecord {
    /// `REJECT-TRAN-DATA PIC X(350)`, the stored bytes of the rejected
    /// `DALYTRAN` record — P-CBTRN02C-001 [app/cbl/CBTRN02C.cbl:L177].
    pub reject_tran_data: Alnum<350>,
    /// `WS-VALIDATION-FAIL-REASON PIC 9(04)`: `0100`, `0101`, `0102` or `0103`
    /// on a written reject — P-CBTRN02C-002 [app/cbl/CBTRN02C.cbl:L181].
    pub validation_fail_reason: NumericCode<4>,
    /// `WS-VALIDATION-FAIL-REASON-DESC PIC X(76)` — P-CBTRN02C-003
    /// [app/cbl/CBTRN02C.cbl:L182].
    pub validation_fail_reason_desc: Alnum<76>,
}

impl RejectRecord {
    /// `CBTRN02C` `2500-WRITE-REJECT-REC` — P-CBTRN02C-012
    /// [app/cbl/CBTRN02C.cbl:L446-451]: the reject record of `daily` with the
    /// reason and description `1500-VALIDATE-TRAN` set.
    ///
    /// `MOVE DALYTRAN-RECORD TO REJECT-TRAN-DATA` [app/cbl/CBTRN02C.cbl:L447]
    /// copies the input record's bytes, which [`DailyTranLayout::encode`]
    /// reproduces in the internal character set; `MOVE WS-VALIDATION-TRAILER
    /// TO VALIDATION-TRAILER` [app/cbl/CBTRN02C.cbl:L448] carries `reason` and
    /// `desc`.
    #[must_use]
    pub fn new(daily: &DailyTranRecord, reason: NumericCode<4>, desc: Alnum<76>) -> Self {
        Self {
            reject_tran_data: Alnum::move_from_bytes(&DailyTranLayout::encode(
                daily,
                Charset::Ascii,
            )),
            validation_fail_reason: reason,
            validation_fail_reason_desc: desc,
        }
    }
}

impl Layout for RejectRecordLayout {
    const LEN: usize = 430;
    const NAME: &'static str = "CBTRN02C-REJECT-RECORD";
    const FIELDS: &'static [FieldSpec] = &[
        REJECT_TRAN_DATA,
        WS_VALIDATION_FAIL_REASON,
        WS_VALIDATION_FAIL_REASON_DESC,
    ];
    type Record = RejectRecord;

    /// Reads `REJECT-TRAN-DATA`, `WS-VALIDATION-FAIL-REASON` and
    /// `WS-VALIDATION-FAIL-REASON-DESC` (P-CBTRN02C-001 to P-CBTRN02C-003)
    /// [app/cbl/CBTRN02C.cbl:L177-182]: the input record and the description
    /// as their stored bytes, the reason as unsigned display digits.
    ///
    /// # Errors
    ///
    /// - [`CodecErrorKind::Length`] unless `bytes` is exactly 430 bytes long.
    /// - [`CodecErrorKind::NotNumeric`] at `WS-VALIDATION-FAIL-REASON` for a
    ///   byte other than `0`–`9`.
    /// - [`CodecErrorKind::Value`] at `WS-VALIDATION-FAIL-REASON` should
    ///   [`NumericCode<4>`](NumericCode) reject the decoded value.
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
        let reject_tran_data = field::alnum(Self::NAME, &rec, &REJECT_TRAN_DATA)?;
        let reason = field::unsigned(Self::NAME, &rec, &WS_VALIDATION_FAIL_REASON)?;
        let validation_fail_reason = i64::try_from(reason)
            .map_err(|_| NumericError::OutOfRange)
            .and_then(NumericCode::<4>::try_from)
            .map_err(|e| value_error(&WS_VALIDATION_FAIL_REASON, &e))?;
        Ok(RejectRecord {
            reject_tran_data,
            validation_fail_reason,
            validation_fail_reason_desc: field::alnum(
                Self::NAME,
                &rec,
                &WS_VALIDATION_FAIL_REASON_DESC,
            )?,
        })
    }

    /// Writes `REJECT-TRAN-DATA`, `WS-VALIDATION-FAIL-REASON` and
    /// `WS-VALIDATION-FAIL-REASON-DESC` (P-CBTRN02C-001 to P-CBTRN02C-003)
    /// [app/cbl/CBTRN02C.cbl:L177-182]: the input record and the description
    /// from the bytes the record holds, the reason as 4 zero-padded digits,
    /// then translates the 430 bytes to `charset`.
    fn encode(record: &Self::Record, charset: Charset) -> Vec<u8> {
        let mut out = vec![b' '; Self::LEN];
        field::put_alnum(
            &mut out,
            &REJECT_TRAN_DATA,
            record.reject_tran_data.as_bytes(),
        );
        field::put_unsigned(
            &mut out,
            &WS_VALIDATION_FAIL_REASON,
            u64::from(record.validation_fail_reason.get()),
        );
        field::put_alnum(
            &mut out,
            &WS_VALIDATION_FAIL_REASON_DESC,
            record.validation_fail_reason_desc.as_bytes(),
        );
        charset.from_internal(&out)
    }
}

/// The error for a decoded value of the field `spec` that its domain type
/// rejects, carrying the domain error text.
fn value_error(spec: &FieldSpec, err: &NumericError) -> CodecError {
    CodecError::new(
        RejectRecordLayout::NAME,
        CodecErrorKind::Value(err.to_string()),
    )
    .at_field(spec.name, spec.offset)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use carddemo_domain::numeric::{Alnum, NumericCode};

    use super::{RejectRecord, RejectRecordLayout};
    use crate::layouts::daily_tran::{DailyTranLayout, DailyTranRecord};
    use crate::{Charset, CodecError, CodecErrorKind, Layout, field};

    /// The ASCII `DALYTRAN` fixture: 300 records of 350 bytes, each ending LF.
    const DAILYTRAN_ASCII: &str = include_str!("../../../../app/data/ASCII/dailytran.txt");

    const CHARSETS: [Charset; 2] = [Charset::Ascii, Charset::Ebcdic];

    /// The sample description, the `1500-A-LOOKUP-XREF` text of reason 100
    /// (P-CBTRN02C-008).
    const DESC: &str = "INVALID CARD NUMBER FOUND";

    /// Record 2 of `dailytran.txt`, decoded.
    fn daily_record_2() -> Result<DailyTranRecord, Box<dyn Error>> {
        let line = DAILYTRAN_ASCII
            .lines()
            .nth(1)
            .ok_or("dailytran.txt has no record 2")?;
        Ok(DailyTranLayout::decode(line.as_bytes(), Charset::Ascii)?)
    }

    /// The reject of record 2 for reason 100, as `1500-A-LOOKUP-XREF` sets it.
    fn reject_of_record_2() -> Result<(DailyTranRecord, RejectRecord), Box<dyn Error>> {
        let daily = daily_record_2()?;
        let reject = RejectRecord::new(
            &daily,
            NumericCode::<4>::try_from(100)?,
            Alnum::move_from(DESC),
        );
        Ok((daily, reject))
    }

    #[test]
    fn fields_tile_the_430_byte_record() {
        assert_eq!(
            (RejectRecordLayout::NAME, RejectRecordLayout::LEN),
            ("CBTRN02C-REJECT-RECORD", 430)
        );
        assert_eq!(RejectRecordLayout::FIELDS.len(), 3);
        assert!(field::fields_are_contiguous(
            RejectRecordLayout::FIELDS,
            430
        ));
    }

    #[test]
    fn decode_rejects_a_record_that_is_not_430_bytes() {
        for len in [429, 431] {
            for charset in CHARSETS {
                assert_eq!(
                    RejectRecordLayout::decode(&vec![b'0'; len], charset),
                    Err(CodecError::new(
                        "CBTRN02C-REJECT-RECORD",
                        CodecErrorKind::Length {
                            expected: 430,
                            actual: len,
                        },
                    )),
                    "{len} bytes in {charset:?}"
                );
            }
        }
    }

    /// P-CBTRN02C-012: the 350 input bytes, then the reason as 4 digits, then
    /// the description padded with spaces to 76 bytes.
    #[test]
    fn a_reject_holds_the_input_record_then_its_reason_and_description()
    -> Result<(), Box<dyn Error>> {
        let (daily, reject) = reject_of_record_2()?;
        let bytes = RejectRecordLayout::encode(&reject, Charset::Ascii);
        assert_eq!(bytes.len(), 430);
        assert_eq!(
            bytes.get(..350),
            Some(&DailyTranLayout::encode(&daily, Charset::Ascii)[..])
        );
        assert_eq!(
            bytes.get(..350),
            DAILYTRAN_ASCII.lines().nth(1).map(str::as_bytes)
        );
        assert_eq!(bytes.get(350..354), Some(&b"0100"[..]));
        let mut desc = DESC.as_bytes().to_vec();
        desc.resize(76, b' ');
        assert_eq!(bytes.get(354..430), Some(&desc[..]));
        Ok(())
    }

    #[test]
    fn a_reject_round_trips_in_both_charsets() -> Result<(), Box<dyn Error>> {
        let (_, reject) = reject_of_record_2()?;
        for charset in CHARSETS {
            let bytes = RejectRecordLayout::encode(&reject, charset);
            assert_eq!(bytes.len(), 430, "{charset:?}");
            assert_eq!(
                RejectRecordLayout::decode(&bytes, charset)?,
                reject,
                "{charset:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_non_digit_reason_is_reported_at_its_field() -> Result<(), Box<dyn Error>> {
        let (_, reject) = reject_of_record_2()?;
        let mut bytes = RejectRecordLayout::encode(&reject, Charset::Ascii);
        bytes[352] = b' ';
        assert_eq!(
            RejectRecordLayout::decode(&bytes, Charset::Ascii),
            Err(CodecError::new(
                "CBTRN02C-REJECT-RECORD",
                CodecErrorKind::NotNumeric { byte: b' ' }
            )
            .at_field("WS-VALIDATION-FAIL-REASON", 350))
        );
        Ok(())
    }
}
