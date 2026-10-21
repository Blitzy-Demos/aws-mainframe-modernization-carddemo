//! `CCPAUERY` `ERROR-LOG-RECORD`: the 122-byte record `COPAUA0C` writes to the CICS transient
//! data queue `CSSL` in `9500-LOG-ERROR` (`COPAUA0C.cbl:983-1011`; CCPAUERY-001 to
//! CCPAUERY-011, COPAUA0C-110).
//!
//! The replacement keeps each record as one row of `cssl_error_log` (migration 0005), its eleven
//! fields as `CHAR(n)` columns, written only by the CP00 worker in a transaction of its own (AAP
//! 0.5.2). [`ErrorLogRecord`] is that record as typed fields, with its codec A byte form through
//! [`FixedRecord`]:
//!
//! | Field | COBOL name | PIC | Offset | Width | Rule |
//! |---|---|---|---|---|---|
//! | `date` | `ERR-DATE` | `X(06)` | 0 | 6 | CCPAUERY-001 |
//! | `time` | `ERR-TIME` | `X(06)` | 6 | 6 | CCPAUERY-002 |
//! | `application` | `ERR-APPLICATION` | `X(08)` | 12 | 8 | CCPAUERY-003 |
//! | `program` | `ERR-PROGRAM` | `X(08)` | 20 | 8 | CCPAUERY-004 |
//! | `location` | `ERR-LOCATION` | `X(04)` | 28 | 4 | CCPAUERY-005 |
//! | `level` | `ERR-LEVEL` | `X(01)` | 32 | 1 | CCPAUERY-006 |
//! | `subsystem` | `ERR-SUBSYSTEM` | `X(01)` | 33 | 1 | CCPAUERY-007 |
//! | `code_1` | `ERR-CODE-1` | `X(09)` | 34 | 9 | CCPAUERY-008 |
//! | `code_2` | `ERR-CODE-2` | `X(09)` | 43 | 9 | CCPAUERY-009 |
//! | `message` | `ERR-MESSAGE` | `X(50)` | 52 | 50 | CCPAUERY-010 |
//! | `event_key` | `ERR-EVENT-KEY` | `X(20)` | 102 | 20 | CCPAUERY-011 |
//!
//! # Every field is text
//!
//! Each field is a `PicX`, its bytes kept verbatim, padding included (PIC-001):
//!
//! - `ERR-DATE` and `ERR-TIME` hold the `FORMATTIME` texts `YYMMDD` and `HHMMSS`. Typed Dates and
//!   Timestamps covers the `PIC X(10)` dates and `PIC X(26)` timestamps only, so these stay text.
//! - `ERR-LEVEL` and `ERR-SUBSYSTEM` stay one-byte text rather than enums, so a byte that no
//!   88-level names still decodes and re-encodes unchanged: `ERROR-LOG-RECORD` has no `VALUE`
//!   clause, and COBOL restricts neither field to its conditions. [`ErrorLogRecord::level`] and
//!   [`ErrorLogRecord::subsystem`] name the condition that holds, if any.
//! - `ERR-CODE-1` and `ERR-CODE-2` carry CICS, MQ and IMS status codes, whose values are halt H4
//!   (COPAUA0C-112); this module fixes only their type.
//!
//! # Event key and redaction
//!
//! `ERR-EVENT-KEY` receives a card number (`COPAUA0C.cbl:428,499,511,638,777,845,930`), an
//! account ID or a customer ID (`COPAUA0C.cbl:546,559,594,607`; COPAUA0C-115, COPAUA0C-116).
//! The COBOL moves the card number in full. Under Cardholder Data Redaction, which names log
//! records and reaches `cssl_error_log` (AAP 0.3.4.8), a card number reaches the key only through
//! [`card_event_key`], which stores its redacted form, so the table never holds a full number
//! (E32; COPAUA0C-113, COPAUA0C-114, COPAUA0C-120). No function here takes raw card bytes.
//! Account and customer IDs are not protected values, and the worker moves them as written.
//!
//! # Totality
//!
//! [`ErrorLogRecord::decode`] is total: any slice of any length gives a record or a typed
//! [`Error`] and never panics (Total Parsers Over Untrusted Input). A slice that is not 122
//! bytes long is a width error for field `"record"`, and a byte outside printable ASCII is an
//! error naming its field, offset and width, never a normalised value (PIC-002, R7). Errors
//! carry no field bytes, so a decode failure cannot disclose an event key (PIC-024).

use copybook::{Error, FixedRecord, PicX, RecordReader, RecordWriter};

use crate::sensitive::CardNumber;

/// The 88-level conditions of `ERR-LEVEL PIC X(01)` (`CCPAUERY.cpy:25-29`; CCPAUERY-006).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorLevel {
    /// `88 ERR-LOG VALUE 'L'`.
    Log,
    /// `88 ERR-INFO VALUE 'I'`.
    Info,
    /// `88 ERR-WARNING VALUE 'W'`: the not-found rows `A001` to `A003`, after which the request
    /// is still decided and answered (COPAUA0C-114).
    Warning,
    /// `88 ERR-CRITICAL VALUE 'C'`: the failure rows such as `M004`, `I003` and `I004`, after
    /// which `9500-LOG-ERROR` ends the COBOL task through `9990-END-ROUTINE` (COPAUA0C-110,
    /// COPAUA0C-117, COPAUA0C-120).
    Critical,
}

impl ErrorLevel {
    /// The condition's `VALUE`, the byte `SET <condition> TO TRUE` moves into `ERR-LEVEL`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Log => b'L',
            Self::Info => b'I',
            Self::Warning => b'W',
            Self::Critical => b'C',
        }
    }

    /// `ERR-LEVEL` as `SET <condition> TO TRUE` leaves it.
    #[must_use]
    pub const fn pic(self) -> PicX<1> {
        PicX::from_array([self.code()])
    }

    /// The condition `field` satisfies, or `None` for a byte no 88-level names. COBOL compares
    /// the byte exactly, so a lower-case `c` is not `ERR-CRITICAL`.
    #[must_use]
    pub const fn from_pic(field: &PicX<1>) -> Option<Self> {
        let [byte] = *field.as_bytes();
        match byte {
            b'L' => Some(Self::Log),
            b'I' => Some(Self::Info),
            b'W' => Some(Self::Warning),
            b'C' => Some(Self::Critical),
            _ => None,
        }
    }
}

/// The 88-level conditions of `ERR-SUBSYSTEM PIC X(01)` (`CCPAUERY.cpy:30-36`; CCPAUERY-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorSubsystem {
    /// `88 ERR-APP VALUE 'A'`: an application condition, such as a card missing from the
    /// cross-reference (COPAUA0C-114).
    App,
    /// `88 ERR-CICS VALUE 'C'`: a failed CICS command, such as a file read.
    Cics,
    /// `88 ERR-IMS VALUE 'I'`: a failed DL/I call, such as the summary update of `I003` or the
    /// detail insert of `I004` (COPAUA0C-119, COPAUA0C-120).
    Ims,
    /// `88 ERR-DB2 VALUE 'D'`.
    Db2,
    /// `88 ERR-MQ VALUE 'M'`: a failed MQ call, such as the reply put of `M004` (COPAUA0C-117).
    Mq,
    /// `88 ERR-FILE VALUE 'F'`.
    File,
}

impl ErrorSubsystem {
    /// The condition's `VALUE`, the byte `SET <condition> TO TRUE` moves into `ERR-SUBSYSTEM`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::App => b'A',
            Self::Cics => b'C',
            Self::Ims => b'I',
            Self::Db2 => b'D',
            Self::Mq => b'M',
            Self::File => b'F',
        }
    }

    /// `ERR-SUBSYSTEM` as `SET <condition> TO TRUE` leaves it.
    #[must_use]
    pub const fn pic(self) -> PicX<1> {
        PicX::from_array([self.code()])
    }

    /// The condition `field` satisfies, or `None` for a byte no 88-level names. COBOL compares
    /// the byte exactly, so a lower-case `m` is not `ERR-MQ`.
    #[must_use]
    pub const fn from_pic(field: &PicX<1>) -> Option<Self> {
        let [byte] = *field.as_bytes();
        match byte {
            b'A' => Some(Self::App),
            b'C' => Some(Self::Cics),
            b'I' => Some(Self::Ims),
            b'D' => Some(Self::Db2),
            b'M' => Some(Self::Mq),
            b'F' => Some(Self::File),
            _ => None,
        }
    }
}

/// `ERROR-LOG-RECORD` (`CCPAUERY.cpy:19-40`), one `CSSL` record and one `cssl_error_log` row
/// (COPAUA0C-110). The fields are in copybook order.
///
/// `9500-LOG-ERROR` sets the date, time, application and program; the paragraph that performs it
/// has set the location, level, subsystem, codes, message and event key (`COPAUA0C.cbl:983-1011`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorLogRecord {
    /// `ERR-DATE PIC X(06)`, offset 0: the `FORMATTIME` `YYMMDD` date of the write
    /// (CCPAUERY-001).
    pub date: PicX<6>,
    /// `ERR-TIME PIC X(06)`, offset 6: the `FORMATTIME` 24-hour `HHMMSS` time of the write
    /// (CCPAUERY-002).
    pub time: PicX<6>,
    /// `ERR-APPLICATION PIC X(08)`, offset 12: the CICS transaction ID, `CP00` then four spaces
    /// (CCPAUERY-003).
    pub application: PicX<8>,
    /// `ERR-PROGRAM PIC X(08)`, offset 20: the program, `COPAUA0C` (CCPAUERY-004).
    pub program: PicX<8>,
    /// `ERR-LOCATION PIC X(04)`, offset 28: the location code, such as `M004`, `I003`, `I004` or
    /// `A001` (CCPAUERY-005).
    pub location: PicX<4>,
    /// `ERR-LEVEL PIC X(01)`, offset 32, whatever byte it holds; [`ErrorLogRecord::level`] names
    /// its 88-level condition (CCPAUERY-006).
    pub level: PicX<1>,
    /// `ERR-SUBSYSTEM PIC X(01)`, offset 33, whatever byte it holds;
    /// [`ErrorLogRecord::subsystem`] names its 88-level condition (CCPAUERY-007).
    pub subsystem: PicX<1>,
    /// `ERR-CODE-1 PIC X(09)`, offset 34; its CICS, MQ and IMS values are halt H4
    /// (CCPAUERY-008, COPAUA0C-112).
    pub code_1: PicX<9>,
    /// `ERR-CODE-2 PIC X(09)`, offset 43; halt H4 likewise (CCPAUERY-009, COPAUA0C-112).
    pub code_2: PicX<9>,
    /// `ERR-MESSAGE PIC X(50)`, offset 52, such as `FAILED TO PUT ON REPLY MQ` padded with spaces
    /// (CCPAUERY-010).
    pub message: PicX<50>,
    /// `ERR-EVENT-KEY PIC X(20)`, offset 102: a card number only as [`card_event_key`] builds
    /// it, or an account or customer ID as written (CCPAUERY-011, E32).
    pub event_key: PicX<20>,
}

impl ErrorLogRecord {
    /// The 88-level condition `ERR-LEVEL` satisfies, or `None` for a byte none names.
    #[must_use]
    pub const fn level(&self) -> Option<ErrorLevel> {
        ErrorLevel::from_pic(&self.level)
    }

    /// The 88-level condition `ERR-SUBSYSTEM` satisfies, or `None` for a byte none names.
    #[must_use]
    pub const fn subsystem(&self) -> Option<ErrorSubsystem> {
        ErrorSubsystem::from_pic(&self.subsystem)
    }
}

/// `ERR-EVENT-KEY` for a card number: its redacted form, twelve `*` and the last four digits,
/// then four spaces, so `0500024453765740` gives `************5740    `.
///
/// This is the only path from a card number to the event key, so neither a `CSSL` record nor a
/// `cssl_error_log` row holds a full number (Cardholder Data Redaction, E32). The COBOL moves the
/// whole `X(16)` number into the `X(20)` key; the redacted form keeps that width, and the same
/// alphanumeric `MOVE` pads it (CCPAUERY-011; COPAUA0C-113, COPAUA0C-114, COPAUA0C-120).
#[must_use]
pub fn card_event_key(card: &CardNumber) -> PicX<20> {
    PicX::moved_from(&card.redacted_bytes())
}

impl FixedRecord for ErrorLogRecord {
    /// `LENGTH OF ERROR-LOG-RECORD`, the length the `CSSL` write passes
    /// (`COPAUA0C.cbl:1001-1006`).
    const WIDTH: usize = 122;

    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = RecordReader::new(bytes, Self::WIDTH)?;
        let record = Self {
            date: r.pic_x("ERR-DATE")?,
            time: r.pic_x("ERR-TIME")?,
            application: r.pic_x("ERR-APPLICATION")?,
            program: r.pic_x("ERR-PROGRAM")?,
            location: r.pic_x("ERR-LOCATION")?,
            level: r.pic_x("ERR-LEVEL")?,
            subsystem: r.pic_x("ERR-SUBSYSTEM")?,
            code_1: r.pic_x("ERR-CODE-1")?,
            code_2: r.pic_x("ERR-CODE-2")?,
            message: r.pic_x("ERR-MESSAGE")?,
            event_key: r.pic_x("ERR-EVENT-KEY")?,
        };
        r.finish()?;
        Ok(record)
    }

    fn encode(&self) -> Vec<u8> {
        let mut w = RecordWriter::with_capacity(Self::WIDTH);
        w.pic_x(&self.date)
            .pic_x(&self.time)
            .pic_x(&self.application)
            .pic_x(&self.program)
            .pic_x(&self.location)
            .pic_x(&self.level)
            .pic_x(&self.subsystem)
            .pic_x(&self.code_1)
            .pic_x(&self.code_2)
            .pic_x(&self.message)
            .pic_x(&self.event_key);
        w.finish()
    }
}

const _: () = assert!(
    PicX::<6>::WIDTH
        + PicX::<6>::WIDTH
        + PicX::<8>::WIDTH
        + PicX::<8>::WIDTH
        + PicX::<4>::WIDTH
        + PicX::<1>::WIDTH
        + PicX::<1>::WIDTH
        + PicX::<9>::WIDTH
        + PicX::<9>::WIDTH
        + PicX::<50>::WIDTH
        + PicX::<20>::WIDTH
        == <ErrorLogRecord as FixedRecord>::WIDTH,
    "the eleven ERROR-LOG-RECORD fields, in copybook order, fill its 122 bytes"
);

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, PicX};

    use super::{ErrorLevel, ErrorLogRecord, ErrorSubsystem, card_event_key};
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The first cross-reference record's card number (`cardxref.txt:1`).
    const CARD: &str = "0500024453765740";

    /// `ERR-EVENT-KEY` for [`CARD`]: its redacted form, then four spaces.
    const CARD_KEY: &[u8; 20] = b"************5740    ";

    /// Each field's COBOL name, offset and width, in copybook order (CCPAUERY-001 to
    /// CCPAUERY-011).
    const FIELDS: [(&str, usize, usize); 11] = [
        ("ERR-DATE", 0, 6),
        ("ERR-TIME", 6, 6),
        ("ERR-APPLICATION", 12, 8),
        ("ERR-PROGRAM", 20, 8),
        ("ERR-LOCATION", 28, 4),
        ("ERR-LEVEL", 32, 1),
        ("ERR-SUBSYSTEM", 33, 1),
        ("ERR-CODE-1", 34, 9),
        ("ERR-CODE-2", 43, 9),
        ("ERR-MESSAGE", 52, 50),
        ("ERR-EVENT-KEY", 102, 20),
    ];

    /// Offsets of `ERR-LEVEL` and `ERR-SUBSYSTEM`.
    const CONDITION_OFFSETS: [usize; 2] = [32, 33];

    /// The `M004` row of a failed reply put, as the CP00 worker builds it: critical, MQ,
    /// `FAILED TO PUT ON REPLY MQ`, codes blank and the request's card as its event key.
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

    /// The 122 bytes of [`m004_record`], field by field.
    fn m004_bytes() -> Vec<u8> {
        [
            b"220718".as_slice(),
            b"193005",
            b"CP00    ",
            b"COPAUA0C",
            b"M004",
            b"C",
            b"M",
            &[b' '; 9],
            &[b' '; 9],
            b"FAILED TO PUT ON REPLY MQ",
            &[b' '; 25],
            CARD_KEY,
        ]
        .concat()
    }

    // audit: CCPAUERY-006
    // audit: CCPAUERY-007
    // audit: COPAUA0C-110
    // audit: COPAUA0C-117
    #[test]
    fn m004_critical_mq_row_round_trips() -> TestResult {
        let record = m004_record()?;
        let bytes = m004_bytes();
        assert_eq!(bytes.len(), ErrorLogRecord::WIDTH);
        assert_eq!(record.encode(), bytes);
        let decoded = ErrorLogRecord::decode(&bytes)?;
        assert_eq!(decoded, record);
        assert_eq!(decoded.encode(), bytes);
        assert_eq!(decoded.level(), Some(ErrorLevel::Critical));
        assert_eq!(decoded.subsystem(), Some(ErrorSubsystem::Mq));
        assert_eq!(decoded.location.to_text(), "M004");
        assert_eq!(decoded.message.to_text(), "FAILED TO PUT ON REPLY MQ");
        Ok(())
    }

    // audit: CCPAUERY-006
    // audit: CCPAUERY-007
    #[test]
    fn every_condition_value_round_trips_through_its_field() {
        let levels = [
            (ErrorLevel::Log, b'L'),
            (ErrorLevel::Info, b'I'),
            (ErrorLevel::Warning, b'W'),
            (ErrorLevel::Critical, b'C'),
        ];
        for (level, code) in levels {
            assert_eq!(level.code(), code);
            assert_eq!(level.pic(), PicX::from_array([code]));
            assert_eq!(ErrorLevel::from_pic(&level.pic()), Some(level));
        }
        let subsystems = [
            (ErrorSubsystem::App, b'A'),
            (ErrorSubsystem::Cics, b'C'),
            (ErrorSubsystem::Ims, b'I'),
            (ErrorSubsystem::Db2, b'D'),
            (ErrorSubsystem::Mq, b'M'),
            (ErrorSubsystem::File, b'F'),
        ];
        for (subsystem, code) in subsystems {
            assert_eq!(subsystem.code(), code);
            assert_eq!(subsystem.pic(), PicX::from_array([code]));
            assert_eq!(ErrorSubsystem::from_pic(&subsystem.pic()), Some(subsystem));
        }
    }

    // audit: CCPAUERY-006
    // audit: CCPAUERY-007
    #[test]
    fn a_byte_no_condition_names_decodes_unchanged_and_names_none() -> TestResult {
        for byte in [b' ', b'X', b'c', b'm', b'0'] {
            let mut bytes = m004_bytes();
            for offset in CONDITION_OFFSETS {
                *bytes.get_mut(offset).ok_or("offset outside the record")? = byte;
            }
            let record = ErrorLogRecord::decode(&bytes)?;
            assert_eq!(record.level, PicX::from_array([byte]));
            assert_eq!(record.subsystem, PicX::from_array([byte]));
            assert_eq!(record.level(), None);
            assert_eq!(record.subsystem(), None);
            assert_eq!(record.encode(), bytes);
        }
        Ok(())
    }

    // audit: CCPAUERY-011
    // audit: COPAUA0C-113
    // audit: PIC-024
    #[test]
    fn a_card_reaches_the_event_key_only_redacted() -> TestResult {
        let key = card_event_key(&CardNumber::parse(CARD)?);
        assert_eq!(key.as_bytes(), CARD_KEY);
        assert_eq!(key.to_text(), "************5740");

        let record = m004_record()?;
        let encoded = String::from_utf8(record.encode())?;
        let rendered = format!("{record:?}");
        for text in [encoded, rendered] {
            assert!(!text.contains(CARD), "{text}");
            assert!(text.contains("************5740"), "{text}");
        }
        Ok(())
    }

    // audit: CCPAUERY-001
    // audit: CCPAUERY-011
    // audit: PIC-002
    #[test]
    fn a_non_printable_byte_is_an_error_at_its_field() -> TestResult {
        let mut end = 0;
        for (field, offset, width) in FIELDS {
            assert_eq!(offset, end, "{field}");
            end += width;
            for position in [offset, offset + width - 1] {
                let mut bytes = m004_bytes();
                *bytes
                    .get_mut(position)
                    .ok_or("position outside the record")? = 0x00;
                assert_eq!(
                    ErrorLogRecord::decode(&bytes).err(),
                    Some(Error::new(field, offset, width, ErrorKind::NonPrintable)),
                );
            }
        }
        assert_eq!(end, ErrorLogRecord::WIDTH);
        Ok(())
    }

    #[test]
    fn a_slice_of_any_other_length_is_a_width_error() {
        let mut short = m004_bytes();
        short.pop();
        let mut long = m004_bytes();
        long.push(b' ');
        let cases = [Vec::new(), vec![b' '; 1], short, long, vec![b' '; 244]];
        for bytes in cases {
            assert_eq!(
                ErrorLogRecord::decode(&bytes).err(),
                Some(Error::new(
                    "record",
                    0,
                    ErrorLogRecord::WIDTH,
                    ErrorKind::WidthMismatch {
                        expected: ErrorLogRecord::WIDTH,
                        actual: bytes.len(),
                    },
                )),
            );
        }
    }
}
