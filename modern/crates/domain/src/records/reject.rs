//! `REJECT-RECORD` of CBTRN02C, the 430-byte record POSTTRAN writes to `DALYREJS(+1)` for every
//! daily transaction it rejects (`CBTRN02C.cbl:81-84,176-182,446-451`; `POSTTRAN.jcl:34-38`;
//! CBTRN02C-027).
//!
//! | Bytes | Field | COBOL | PIC |
//! |---|---|---|---|
//! | 1–350 | [`tran_data`](RejectRecord::tran_data) | `REJECT-TRAN-DATA`, from `DALYTRAN-RECORD` | `X(350)` |
//! | 351–354 | [`reason`](RejectRecord::reason) | `WS-VALIDATION-FAIL-REASON` | `9(04)` |
//! | 355–430 | [`description`](RejectRecord::description) | `WS-VALIDATION-FAIL-REASON-DESC` | `X(76)` |
//!
//! `2500-WRITE-REJECT-REC` moves the daily record into `REJECT-TRAN-DATA` and the 80-byte
//! `WS-VALIDATION-TRAILER` group into `VALIDATION-TRAILER`, so the trailer's two elementary
//! fields are this record's last two. The reason is one of `0100` to `0103`, four digits with
//! leading zeros (PIC-003); `0109` never reaches a reject record (CBTRN02C-023). The description
//! is the reason's literal, left-justified and space-padded to 76 bytes. The job writes each
//! record followed by LF in the generation file (CBTRN02C-028); that framing is the job's, not
//! this record's.
//!
//! The record is not a copybook layout, so `copybooks.md` has no row for it: its fields follow
//! the class rows of that file, `PIC X(n)` bytes kept verbatim (PIC-001, PIC-002) and `PIC 9(n)`
//! digits checked and re-padded (PIC-003, PIC-004).
//!
//! # Cardholder Data Redaction
//!
//! `REJECT-TRAN-DATA` holds the daily record's bytes as read, `DALYTRAN-CARD-NUM` among them
//! (PIC-024). The reject file is a local file, not egress, and keeps the full number as the COBOL
//! writes it (CBTRN02C-027), so the field stays a plain `PicX<350>`. No job-log event, span or
//! error may carry a record's bytes (CBTRN02C-037), so [`RejectRecord`] has a hand-written
//! `Debug` that never shows [`tran_data`](RejectRecord::tran_data), and no `Display` or
//! `Serialize`.

use core::fmt;

use copybook::{Error, FixedRecord, PicU, PicX, RecordReader, RecordWriter};

/// One rejected daily transaction with its validation trailer (`CBTRN02C.cbl:176-182`;
/// CBTRN02C-027).
///
/// `Debug` shows the reason and the description only, never the input bytes, which carry a full
/// card number (CBTRN02C-037, Cardholder Data Redaction).
#[derive(Clone, PartialEq, Eq)]
pub struct RejectRecord {
    /// `REJECT-TRAN-DATA PIC X(350)`: the 350 bytes of the rejected `DALYTRAN-RECORD`, verbatim
    /// (CBTRN02C-027, PIC-001). Holds the full `DALYTRAN-CARD-NUM` (PIC-024).
    pub tran_data: PicX<350>,
    /// `WS-VALIDATION-FAIL-REASON PIC 9(04)`, the first field of `VALIDATION-TRAILER`: `0100`,
    /// `0101`, `0102` or `0103` (CBTRN02C-027, PIC-003).
    pub reason: PicU<4>,
    /// `WS-VALIDATION-FAIL-REASON-DESC PIC X(76)`, the second field of `VALIDATION-TRAILER`: the
    /// reason's literal, space-padded (CBTRN02C-027, PIC-001).
    pub description: PicX<76>,
}

impl RejectRecord {
    /// The record `2500-WRITE-REJECT-REC` builds from the daily record's bytes and the current
    /// `WS-VALIDATION-TRAILER` (`CBTRN02C.cbl:446-448`; CBTRN02C-027).
    ///
    /// The alphanumeric fields are taken by reference and copied, as the two `MOVE`s copy them,
    /// so the caller keeps its own values.
    #[must_use]
    pub fn new(tran_data: &PicX<350>, reason: PicU<4>, description: &PicX<76>) -> Self {
        Self {
            tran_data: tran_data.clone(),
            reason,
            description: description.clone(),
        }
    }
}

impl FixedRecord for RejectRecord {
    /// `LRECL=430` of the `DALYREJS(+1)` DD, `FD-REJECT-RECORD PIC X(350)` plus
    /// `FD-VALIDATION-TRAILER PIC X(80)` (`POSTTRAN.jcl:36`; `CBTRN02C.cbl:81-84`).
    const WIDTH: usize = 430;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            tran_data: r.pic_x("REJECT-TRAN-DATA")?,
            reason: r.pic_u("WS-VALIDATION-FAIL-REASON")?,
            description: r.pic_x("WS-VALIDATION-FAIL-REASON-DESC")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.tran_data)
            .pic_u(self.reason)
            .pic_x(&self.description);
        w.finish()
    }
}

/// Shows `reason` and `description` and omits `tran_data`, whose bytes include the full
/// `DALYTRAN-CARD-NUM` (CBTRN02C-037, PIC-024, Cardholder Data Redaction).
impl fmt::Debug for RejectRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RejectRecord")
            .field("reason", &self.reason)
            .field("description", &self.description)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, PicU, PicX};

    use super::RejectRecord;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A card number of no fixture record, so the test holds no fixture card number.
    const CARD: &[u8; 16] = b"4000002000003000";

    /// Offset of `DALYTRAN-CARD-NUM` in `DALYTRAN-RECORD` (CVTRA06Y-011).
    const CARD_OFFSET: usize = 262;

    /// The `0103` description, `CBTRN02C.cbl:418-419` (CBTRN02C-008).
    const AFTER_EXPIRATION: &str = "TRANSACTION RECEIVED AFTER ACCT EXPIRATION";

    /// A 350-byte daily record image: a transaction ID, spaces, and [`CARD`] at its offset.
    fn daily_image() -> Result<PicX<350>, ErrorKind> {
        let mut image = b"0000000000000001".to_vec();
        image.resize(CARD_OFFSET, b' ');
        image.extend_from_slice(CARD);
        image.resize(350, b' ');
        PicX::<350>::decode(&image)
    }

    /// The record for `reason` and `description` over [`daily_image`].
    fn reject(reason: u64, description: &str) -> Result<RejectRecord, Box<dyn StdError>> {
        Ok(RejectRecord::new(
            &daily_image()?,
            PicU::<4>::new(reason)?,
            &PicX::<76>::from_text(description)?,
        ))
    }

    /// Whether `haystack` contains `needle` as a contiguous run.
    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack
            .windows(needle.len())
            .any(|window| window == needle)
    }

    // audit: CBTRN02C-027
    // audit: CBTRN02C-008
    #[test]
    fn encodes_input_bytes_then_reason_then_padded_description() -> TestResult {
        let image = daily_image()?;
        let record = reject(103, AFTER_EXPIRATION)?;
        let bytes = record.encode();
        assert_eq!(bytes.len(), RejectRecord::WIDTH);

        let (input, trailer) = bytes.split_at_checked(350).ok_or("short record")?;
        assert_eq!(input, image.as_bytes());
        let (reason, description) = trailer.split_at_checked(4).ok_or("short trailer")?;
        assert_eq!(reason, b"0103");
        let mut expected = AFTER_EXPIRATION.as_bytes().to_vec();
        expected.resize(76, b' ');
        assert_eq!(description, expected.as_slice());
        Ok(())
    }

    // audit: CBTRN02C-027
    // audit: CBTRN02C-007
    #[test]
    fn reason_keeps_its_leading_zeros() -> TestResult {
        let bytes = reject(102, "OVERLIMIT TRANSACTION")?.encode();
        assert_eq!(bytes.get(350..354), Some(b"0102".as_slice()));
        // The description starts at 354; its 21-byte literal ends at 375.
        let padding = bytes.get(375..).ok_or("short record")?;
        assert_eq!(padding.len(), 55);
        assert!(padding.iter().all(|&byte| byte == b' '));
        Ok(())
    }

    // audit: CBTRN02C-027
    #[test]
    fn decode_inverts_encode() -> TestResult {
        let record = reject(103, AFTER_EXPIRATION)?;
        let bytes = record.encode();
        let decoded = RejectRecord::decode(&bytes)?;
        assert_eq!(decoded, record);
        assert_eq!(decoded.encode(), bytes);
        assert_eq!(decoded.reason.get(), 103);
        assert_eq!(decoded.description.to_text(), AFTER_EXPIRATION);
        Ok(())
    }

    // audit: CBTRN02C-037
    #[test]
    fn debug_never_shows_the_input_bytes() -> TestResult {
        let record = reject(103, AFTER_EXPIRATION)?;
        assert!(contains(record.tran_data.as_bytes(), CARD));
        let shown = format!("{record:?}");
        assert!(!contains(shown.as_bytes(), CARD));
        assert!(!shown.contains("tran_data"));
        assert!(shown.starts_with("RejectRecord {"));
        assert!(shown.contains("PicU(103)"));
        assert!(shown.contains(AFTER_EXPIRATION));
        assert!(shown.ends_with(".. }"));
        Ok(())
    }

    #[test]
    fn any_other_length_is_a_record_width_error() -> TestResult {
        let bytes = reject(100, "INVALID CARD NUMBER FOUND")?.encode();
        let short = bytes.get(..429).ok_or("short record")?;
        let mut long = bytes.clone();
        long.push(b' ');
        for (slice, actual) in [(short, 429), (long.as_slice(), 431), (b"".as_slice(), 0)] {
            assert!(matches!(
                RejectRecord::decode(slice),
                Err(Error {
                    field: "record",
                    offset: 0,
                    width: 430,
                    kind: ErrorKind::WidthMismatch { expected: 430, actual: a },
                }) if a == actual
            ));
        }
        Ok(())
    }

    // audit: PIC-002
    // audit: PIC-004
    #[test]
    fn field_errors_name_the_field_offset_and_width() -> TestResult {
        let bytes = reject(101, "ACCOUNT RECORD NOT FOUND")?.encode();

        let mut control_byte = bytes.clone();
        let first = control_byte.first_mut().ok_or("empty record")?;
        *first = b'\t';
        assert_eq!(
            RejectRecord::decode(&control_byte).err(),
            Some(Error::new(
                "REJECT-TRAN-DATA",
                0,
                350,
                ErrorKind::NonPrintable
            )),
        );

        let mut blank_reason = bytes;
        for byte in blank_reason.iter_mut().skip(350).take(4) {
            *byte = b' ';
        }
        assert_eq!(
            RejectRecord::decode(&blank_reason).err(),
            Some(Error::new(
                "WS-VALIDATION-FAIL-REASON",
                350,
                4,
                ErrorKind::NonDigit
            )),
        );
        Ok(())
    }
}
