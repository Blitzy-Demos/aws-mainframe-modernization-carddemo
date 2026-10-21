//! Codec B for `cssl_error_log` (migration 0005): the 122-byte `CCPAUERY` `ERROR-LOG-RECORD`
//! that `COPAUA0C` writes to the CICS transient data queue `CSSL` in `9500-LOG-ERROR`
//! (`COPAUA0C.cbl:983-1011`, `CCPAUERY.cpy:19-40`; COPAUA0C-110).
//!
//! The eleven fields are eleven `CHAR(n)` columns in copybook order:
//!
//! | Record field | COBOL name | PIC | Column | Rule |
//! |---|---|---|---|---|
//! | `date` | `ERR-DATE` | `X(06)` | `err_date CHAR(6)` | CCPAUERY-001 |
//! | `time` | `ERR-TIME` | `X(06)` | `err_time CHAR(6)` | CCPAUERY-002 |
//! | `application` | `ERR-APPLICATION` | `X(08)` | `err_application CHAR(8)` | CCPAUERY-003 |
//! | `program` | `ERR-PROGRAM` | `X(08)` | `err_program CHAR(8)` | CCPAUERY-004 |
//! | `location` | `ERR-LOCATION` | `X(04)` | `err_location CHAR(4)` | CCPAUERY-005 |
//! | `level` | `ERR-LEVEL` | `X(01)` | `err_level CHAR(1)` | CCPAUERY-006 |
//! | `subsystem` | `ERR-SUBSYSTEM` | `X(01)` | `err_subsystem CHAR(1)` | CCPAUERY-007 |
//! | `code_1` | `ERR-CODE-1` | `X(09)` | `err_code_1 CHAR(9)` | CCPAUERY-008 |
//! | `code_2` | `ERR-CODE-2` | `X(09)` | `err_code_2 CHAR(9)` | CCPAUERY-009 |
//! | `message` | `ERR-MESSAGE` | `X(50)` | `err_message CHAR(50)` | CCPAUERY-010 |
//! | `event_key` | `ERR-EVENT-KEY` | `X(20)` | `err_event_key CHAR(20)` | CCPAUERY-011 |
//!
//! Each value travels as the text of its `PIC X(n)` bytes, padding included, through the
//! `rows` column conversions, never trimmed or re-padded (PIC-001, PIC-002). A TDQ record has no
//! key, so the table adds the surrogate `id BIGSERIAL`, a 64-bit key read as `i64` (Checked
//! Integer Narrowing). The database assigns it: [`CsslErrorLogBinds`] has no `id`, and codec B
//! never compares it (CCPAUERY-012).
//!
//! # Event key and redaction
//!
//! The COBOL moves full card numbers into `ERR-EVENT-KEY`
//! (`COPAUA0C.cbl:428,499,511,638,777,845,930`). Under Cardholder Data Redaction the key reaches
//! this module already in the 16-character redacted form, twelve `*` and the last four digits
//! padded with spaces to 20, which `domain::records::error_log::card_event_key` builds (E32;
//! PIC-024). The key is stored and read back exactly as given: nothing here derives, widens or
//! unmasks it. Neither struct derives `Debug`, `Clone` or `Serialize`, so no row value reaches a
//! log line or a payload by accident.
//!
//! # Status codes
//!
//! `ERR-CODE-1` and `ERR-CODE-2` carry CICS, MQ and IMS codes, such as the DL/I status of the
//! `I003` and `I004` rows (`COPAUA0C.cbl:843,928`). Their values are `HALTED(H4)` content that
//! the CP00 worker produces (COPAUA0C-112); this module fixes only their type and keeps whatever
//! bytes it receives.

use domain::records::error_log::ErrorLogRecord;

use super::{pic_x_to_string, string_to_pic_x};
use crate::error::PersistenceError;

/// The bind values of one `cssl_error_log` insert: every column except the database-assigned
/// `id`, each the verbatim text of its `PIC X(n)` field, in copybook order (CCPAUERY-001 to
/// CCPAUERY-011).
///
/// `repos::cssl_error_log` binds every field. `Debug` is not derived (Cardholder Data
/// Redaction).
pub struct CsslErrorLogBinds {
    /// `err_date CHAR(6)`, from `ERR-DATE PIC X(06)`, the `FORMATTIME` `YYMMDD` text
    /// (CCPAUERY-001).
    pub(crate) err_date: String,
    /// `err_time CHAR(6)`, from `ERR-TIME PIC X(06)`, the `FORMATTIME` `HHMMSS` text
    /// (CCPAUERY-002).
    pub(crate) err_time: String,
    /// `err_application CHAR(8)`, from `ERR-APPLICATION PIC X(08)`: `CP00` and four spaces
    /// (CCPAUERY-003).
    pub(crate) err_application: String,
    /// `err_program CHAR(8)`, from `ERR-PROGRAM PIC X(08)`: `COPAUA0C` (CCPAUERY-004).
    pub(crate) err_program: String,
    /// `err_location CHAR(4)`, from `ERR-LOCATION PIC X(04)`, such as `M004` (CCPAUERY-005).
    pub(crate) err_location: String,
    /// `err_level CHAR(1)`, from `ERR-LEVEL PIC X(01)` (CCPAUERY-006).
    pub(crate) err_level: String,
    /// `err_subsystem CHAR(1)`, from `ERR-SUBSYSTEM PIC X(01)` (CCPAUERY-007).
    pub(crate) err_subsystem: String,
    /// `err_code_1 CHAR(9)`, from `ERR-CODE-1 PIC X(09)`: `HALTED(H4)` content (CCPAUERY-008).
    pub(crate) err_code_1: String,
    /// `err_code_2 CHAR(9)`, from `ERR-CODE-2 PIC X(09)`: `HALTED(H4)` content (CCPAUERY-009).
    pub(crate) err_code_2: String,
    /// `err_message CHAR(50)`, from `ERR-MESSAGE PIC X(50)`, such as
    /// `FAILED TO PUT ON REPLY MQ` and 25 spaces (CCPAUERY-010).
    pub(crate) err_message: String,
    /// `err_event_key CHAR(20)`, from `ERR-EVENT-KEY PIC X(20)`: a card number only in its
    /// redacted form, or an account or customer ID as written (CCPAUERY-011, E32).
    pub(crate) err_event_key: String,
}

/// One `cssl_error_log` row as a read of every column returns it: the surrogate `id` and the
/// eleven `CHAR(n)` texts, padding included (CCPAUERY-001 to CCPAUERY-012).
///
/// Field names are the column names, so `repos::cssl_error_log` reads rows with `query_as!`.
/// `Debug` is not derived (Cardholder Data Redaction).
pub struct CsslErrorLogRow {
    /// `id BIGSERIAL`, the surrogate key a TDQ record lacks (CCPAUERY-012).
    pub(crate) id: i64,
    /// `err_date CHAR(6)` (CCPAUERY-001).
    pub(crate) err_date: String,
    /// `err_time CHAR(6)` (CCPAUERY-002).
    pub(crate) err_time: String,
    /// `err_application CHAR(8)` (CCPAUERY-003).
    pub(crate) err_application: String,
    /// `err_program CHAR(8)` (CCPAUERY-004).
    pub(crate) err_program: String,
    /// `err_location CHAR(4)` (CCPAUERY-005).
    pub(crate) err_location: String,
    /// `err_level CHAR(1)` (CCPAUERY-006).
    pub(crate) err_level: String,
    /// `err_subsystem CHAR(1)` (CCPAUERY-007).
    pub(crate) err_subsystem: String,
    /// `err_code_1 CHAR(9)` (CCPAUERY-008).
    pub(crate) err_code_1: String,
    /// `err_code_2 CHAR(9)` (CCPAUERY-009).
    pub(crate) err_code_2: String,
    /// `err_message CHAR(50)` (CCPAUERY-010).
    pub(crate) err_message: String,
    /// `err_event_key CHAR(20)` (CCPAUERY-011, E32).
    pub(crate) err_event_key: String,
}

impl CsslErrorLogBinds {
    /// The bind values for `record`, every field's bytes verbatim. The event key is stored as
    /// given: the writer has already redacted any card number in it (E32).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] with [`crate::error::DecodeKind::InvalidByte`], naming the
    /// column, for a NUL or non-ASCII byte, which no `CHAR(n)` column stores byte for byte
    /// (PIC-002).
    pub fn from_record(record: &ErrorLogRecord) -> Result<Self, PersistenceError> {
        Ok(Self {
            err_date: pic_x_to_string("err_date", &record.date)?,
            err_time: pic_x_to_string("err_time", &record.time)?,
            err_application: pic_x_to_string("err_application", &record.application)?,
            err_program: pic_x_to_string("err_program", &record.program)?,
            err_location: pic_x_to_string("err_location", &record.location)?,
            err_level: pic_x_to_string("err_level", &record.level)?,
            err_subsystem: pic_x_to_string("err_subsystem", &record.subsystem)?,
            err_code_1: pic_x_to_string("err_code_1", &record.code_1)?,
            err_code_2: pic_x_to_string("err_code_2", &record.code_2)?,
            err_message: pic_x_to_string("err_message", &record.message)?,
            err_event_key: pic_x_to_string("err_event_key", &record.event_key)?,
        })
    }
}

impl CsslErrorLogRow {
    /// The row's `id` and the record its columns hold, each field exactly the stored bytes.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`], naming the column, with
    /// [`crate::error::DecodeKind::InvalidByte`] for a NUL or non-ASCII character (PIC-002) or
    /// [`crate::error::DecodeKind::WidthMismatch`] for text that is not exactly its picture's
    /// width, such as a 49-byte `err_message` (PIC-001).
    pub fn into_record(self) -> Result<(i64, ErrorLogRecord), PersistenceError> {
        let Self {
            id,
            err_date,
            err_time,
            err_application,
            err_program,
            err_location,
            err_level,
            err_subsystem,
            err_code_1,
            err_code_2,
            err_message,
            err_event_key,
        } = self;
        let record = ErrorLogRecord {
            date: string_to_pic_x("err_date", &err_date)?,
            time: string_to_pic_x("err_time", &err_time)?,
            application: string_to_pic_x("err_application", &err_application)?,
            program: string_to_pic_x("err_program", &err_program)?,
            location: string_to_pic_x("err_location", &err_location)?,
            level: string_to_pic_x("err_level", &err_level)?,
            subsystem: string_to_pic_x("err_subsystem", &err_subsystem)?,
            code_1: string_to_pic_x("err_code_1", &err_code_1)?,
            code_2: string_to_pic_x("err_code_2", &err_code_2)?,
            message: string_to_pic_x("err_message", &err_message)?,
            event_key: string_to_pic_x("err_event_key", &err_event_key)?,
        };
        Ok((id, record))
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{FixedRecord, PicX};
    use domain::records::error_log::{ErrorLevel, ErrorLogRecord, ErrorSubsystem, card_event_key};
    use domain::sensitive::CardNumber;

    use super::{CsslErrorLogBinds, CsslErrorLogRow};
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The first cross-reference record's card number (`cardxref.txt:1`).
    const CARD: &str = "0500024453765740";

    /// `ERR-EVENT-KEY` for [`CARD`]: its redacted form, then four spaces.
    const CARD_KEY: &str = "************5740    ";

    /// An account-ID event key, which the worker moves as written (`COPAUA0C.cbl:546,559`).
    const ACCOUNT_KEY: &str = "00000000050         ";

    /// A database-assigned `id`.
    const ID: i64 = 41;

    /// The column names in copybook order, the order of [`texts`].
    const COLUMNS: [&str; 11] = [
        "err_date",
        "err_time",
        "err_application",
        "err_program",
        "err_location",
        "err_level",
        "err_subsystem",
        "err_code_1",
        "err_code_2",
        "err_message",
        "err_event_key",
    ];

    /// The `M004` row of a failed reply put (`COPAUA0C.cbl:767-779`): critical, MQ,
    /// `FAILED TO PUT ON REPLY MQ`, codes blank, and the request's card as its redacted key.
    fn m004_record() -> Result<ErrorLogRecord, Box<dyn StdError>> {
        Ok(ErrorLogRecord {
            date: PicX::from_array(*b"220718"),
            time: PicX::from_array(*b"193005"),
            application: PicX::moved_from(b"CP00"),
            program: PicX::from_array(*b"COPAUA0C"),
            location: PicX::from_array(*b"M004"),
            level: ErrorLevel::Critical.pic(),
            subsystem: ErrorSubsystem::Mq.pic(),
            code_1: PicX::spaces(),
            code_2: PicX::spaces(),
            message: PicX::moved_from(b"FAILED TO PUT ON REPLY MQ"),
            event_key: card_event_key(&CardNumber::parse(CARD)?),
        })
    }

    /// The row a read returns for `binds` stored under `id`.
    fn row(id: i64, binds: CsslErrorLogBinds) -> CsslErrorLogRow {
        let CsslErrorLogBinds {
            err_date,
            err_time,
            err_application,
            err_program,
            err_location,
            err_level,
            err_subsystem,
            err_code_1,
            err_code_2,
            err_message,
            err_event_key,
        } = binds;
        CsslErrorLogRow {
            id,
            err_date,
            err_time,
            err_application,
            err_program,
            err_location,
            err_level,
            err_subsystem,
            err_code_1,
            err_code_2,
            err_message,
            err_event_key,
        }
    }

    /// The bind texts in column order.
    fn bound(binds: &CsslErrorLogBinds) -> [&str; 11] {
        [
            &binds.err_date,
            &binds.err_time,
            &binds.err_application,
            &binds.err_program,
            &binds.err_location,
            &binds.err_level,
            &binds.err_subsystem,
            &binds.err_code_1,
            &binds.err_code_2,
            &binds.err_message,
            &binds.err_event_key,
        ]
    }

    /// The row's texts in column order, for editing one column.
    fn texts(row: &mut CsslErrorLogRow) -> [&mut String; 11] {
        [
            &mut row.err_date,
            &mut row.err_time,
            &mut row.err_application,
            &mut row.err_program,
            &mut row.err_location,
            &mut row.err_level,
            &mut row.err_subsystem,
            &mut row.err_code_1,
            &mut row.err_code_2,
            &mut row.err_message,
            &mut row.err_event_key,
        ]
    }

    /// `field` with its first byte replaced by `byte`.
    fn with_first<const N: usize>(field: &PicX<N>, byte: u8) -> PicX<N> {
        let mut bytes = *field.as_bytes();
        if let Some(first) = bytes.first_mut() {
            *first = byte;
        }
        PicX::from_array(bytes)
    }

    /// `base` with the first byte of column `position` (copybook order) replaced by `byte`.
    fn record_with_first(base: &ErrorLogRecord, position: usize, byte: u8) -> ErrorLogRecord {
        let mut record = base.clone();
        match position {
            0 => record.date = with_first(&base.date, byte),
            1 => record.time = with_first(&base.time, byte),
            2 => record.application = with_first(&base.application, byte),
            3 => record.program = with_first(&base.program, byte),
            4 => record.location = with_first(&base.location, byte),
            5 => record.level = with_first(&base.level, byte),
            6 => record.subsystem = with_first(&base.subsystem, byte),
            7 => record.code_1 = with_first(&base.code_1, byte),
            8 => record.code_2 = with_first(&base.code_2, byte),
            9 => record.message = with_first(&base.message, byte),
            _ => record.event_key = with_first(&base.event_key, byte),
        }
        record
    }

    // audit: CCPAUERY-001
    // audit: CCPAUERY-003
    // audit: CCPAUERY-005
    // audit: CCPAUERY-010
    // audit: CCPAUERY-011
    // audit: CCPAUERY-012
    // audit: COPAUA0C-117
    // audit: PIC-001
    // audit: PIC-024
    #[test]
    fn m004_row_round_trips_with_its_event_key_as_given() -> TestResult {
        let card_record = m004_record()?;
        let account_record = ErrorLogRecord {
            event_key: PicX::moved_from(b"00000000050"),
            ..card_record.clone()
        };
        for (record, key) in [(card_record, CARD_KEY), (account_record, ACCOUNT_KEY)] {
            let binds = CsslErrorLogBinds::from_record(&record)?;
            let values = bound(&binds);
            let message = format!("{:<50}", "FAILED TO PUT ON REPLY MQ");
            let expected: [&str; 11] = [
                "220718",
                "193005",
                "CP00    ",
                "COPAUA0C",
                "M004",
                "C",
                "M",
                "         ",
                "         ",
                message.as_str(),
                key,
            ];
            assert_eq!(values, expected);
            // The columns in copybook order are the 122 record bytes, nothing trimmed or added.
            let stored = values.concat();
            assert_eq!(stored.as_bytes(), record.encode().as_slice());
            assert!(!stored.contains(CARD), "{stored}");

            let (id, back) = row(ID, binds).into_record()?;
            assert_eq!(id, ID);
            assert_eq!(back, record);
        }
        Ok(())
    }

    // audit: CCPAUERY-010
    // audit: PIC-001
    #[test]
    fn a_column_of_the_wrong_width_is_an_error() -> TestResult {
        // A trimmed message, one byte short of the picture, and one byte over.
        for actual in [25, 49, 51] {
            let mut stored = row(ID, CsslErrorLogBinds::from_record(&m004_record()?)?);
            stored.err_message = format!("{:<actual$}", "FAILED TO PUT ON REPLY MQ");
            assert_eq!(
                stored.into_record(),
                Err(PersistenceError::decode(
                    "err_message",
                    DecodeKind::WidthMismatch {
                        expected: 50,
                        actual
                    }
                ))
            );
        }
        Ok(())
    }

    // audit: CCPAUERY-008
    // audit: PIC-002
    #[test]
    fn a_nul_or_non_ascii_byte_is_an_error_naming_its_column_both_ways() -> TestResult {
        let base = m004_record()?;
        // A NUL, and the first byte of the UTF-8 form of `é`.
        for (byte, character) in [(0x00, '\0'), (0xC3, '\u{e9}')] {
            for (position, column) in COLUMNS.into_iter().enumerate() {
                let invalid = Err(PersistenceError::decode(column, DecodeKind::InvalidByte));

                let record = record_with_first(&base, position, byte);
                assert_eq!(CsslErrorLogBinds::from_record(&record).map(drop), invalid);

                let mut stored = row(ID, CsslErrorLogBinds::from_record(&base)?);
                for (index, text) in texts(&mut stored).into_iter().enumerate() {
                    if index == position {
                        text.insert(0, character);
                        text.pop();
                    }
                }
                assert_eq!(stored.into_record().map(drop), invalid);
            }
        }
        Ok(())
    }
}
