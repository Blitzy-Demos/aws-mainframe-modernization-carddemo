//! MNTTRDB2, the batch maintenance of the Db2 transaction types, as the pure rules of its program
//! COBTUPDT (`app/app-transaction-type-db2/cbl/COBTUPDT.cbl`; AAP 0.2.3, 0.3.4.3). The audit is
//! `modern/audit/batch/MNTTRDB2.md`.
//!
//! This module decodes the `--input` records and decides, record by record, the SQL action and the
//! job-log lines COBTUPDT displays. It does no I/O. `batch::jobs::mnttrdb2` reads the file, calls
//! [`decode_input`], [`Maintenance::open_lines`] and [`Maintenance::plan`], executes each
//! [`MaintenanceAction`] against `db2.transaction_type` through its repository, reports the
//! statement's result to [`Maintenance::complete`] as a [`SqlOutcome`], commits once after the last
//! record (COBTUPDT-024) and exits with [`Maintenance::return_code`] (COBTUPDT-023).
//!
//! | Item | COBOL | Rules |
//! |---|---|---|
//! | [`MaintenanceRecord`], [`parse_input`], [`decode_input`] | `WS-INPUT-REC`, `1002-READ-RECORDS` | COBTUPDT-002, COBTUPDT-003 |
//! | [`Maintenance::open_lines`] | `0001-OPEN-FILES` | COBTUPDT-004 |
//! | [`Maintenance::plan`] | `1002-READ-RECORDS`, `1003-TREAT-RECORD` | COBTUPDT-006 to COBTUPDT-009 |
//! | [`Maintenance::complete`] | `10031-INSERT-DB`, `10032-UPDATE-DB`, `10033-DELETE-DB` | COBTUPDT-010 to COBTUPDT-020 |
//! | the message overlay, [`Maintenance::return_code`] | `WS-RETURN-MSG`, `9999-ABEND`, `RETURN-CODE` | COBTUPDT-021 to COBTUPDT-023 |
//!
//! Behaviour the module reproduces rather than corrects (Behaviour constraint):
//!
//! - No field is validated: any two printable bytes are the type code, and the description is all
//!   50 bytes of `INPUT-REC-DESC`, trailing spaces kept, never trimmed as COTRTUPC trims its own
//!   (COBTUPDT-002, COBTUPDT-010, COBTUPDT-012; X-MNTTRDB2-3, X-MNTTRDB2-6).
//! - `WS-RETURN-MSG` is never cleared: each `STRING` overwrites only its own prefix, so a short
//!   message is displayed with the tail of a longer earlier one (COBTUPDT-021; X-MNTTRDB2-8).
//! - `9999-ABEND` never abends: it displays the message, sets return code 4 and the run goes on
//!   with the next record (COBTUPDT-022; X-MNTTRDB2-7).
//!
//! Behaviour the module changes on purpose (refinement override):
//!
//! - The `PROCESSING` line shows `INPUT-REC-DESC` as 50 `*`, whatever it holds, for every record,
//!   a comment line and an invalid one included. The description is unvalidated free text from
//!   the input file, so it can carry a card number, an SSN or a password, and Cardholder Data
//!   Redaction prevails over byte parity for those 50 bytes only: the literal, the action byte,
//!   the two code bytes and the line's 66 characters are the COBOL's, and the insert and the
//!   update still bind all 50 bytes (COBTUPDT-006, COBTUPDT-010, COBTUPDT-012; X-MNTTRDB2-10).
//!   `Debug` of [`MaintenanceRecord`] and [`MaintenanceAction`] masks the description the same
//!   way.
//!
//! Data version: every action names `db2.transaction_type`, the Db2 data version that migration
//! 0006 seeds. Nothing here produces or reads a VSAM
//! [`TransactionTypeRecord`](crate::records::TransactionTypeRecord), so a maintenance change never
//! reaches the `tranrept` lookups (E23; COBTUPDT-026, COBTUPDT-027).
//!
//! Halt: the five characters of the edited `SQLCODE` (`WS-VAR-SQLCODE PIC ----9`) that end the
//! error text are `HALTED(H3)`: candidate X-HALTS-2 of `modern/audit/HALTS.md`, proposed to join
//! H3, which `MNTTRDB2.md` cites for them (COBTUPDT-020). The value is Db2's and `PostgreSQL`
//! reports an SQLSTATE instead, so the module writes a blank stand-in there, which is no parity
//! result. Every other byte of that text is audited and the return code is unaffected.

use std::fmt;

use copybook::edited::EditedPicture;
use copybook::{Error, FixedRecord, PicX, RecordReader, lines};

use crate::records::Db2TransactionType;

/// Bytes in one input record: the FD `TR-RECORD RECORDING MODE F` record `WS-INPUT-VARS` and the
/// `WS-INPUT-REC` group it is read `INTO`, `PIC X(1)` + `PIC X(2)` + `PIC X(50)`
/// (`COBTUPDT.cbl:39-46,71-77`; COBTUPDT-002).
pub const INPUT_RECORD_LEN: usize = 53;

/// `INPUT-REC-TYPE PIC X(1)`, the action byte at offset 0 (column 1).
const TYPE_FIELD: &str = "INPUT-REC-TYPE";

/// `INPUT-REC-NUMBER PIC X(2)`, the type code at offset 1 (columns 2-3), bound as
/// `:INPUT-REC-NUMBER`.
const NUMBER_FIELD: &str = "INPUT-REC-NUMBER";

/// `INPUT-REC-DESC PIC X(50)`, the description at offset 3 (columns 4-53), bound as
/// `:INPUT-REC-DESC`.
const DESC_FIELD: &str = "INPUT-REC-DESC";

/// Width of `INPUT-REC-DESC PIC X(50)`.
const DESC_LEN: usize = 50;

/// Width of `WS-RETURN-MSG PIC X(80)` (`COBTUPDT.cbl:61-62`).
const RETURN_MSG_LEN: usize = 80;

/// The byte `VALUE SPACES` fills `WS-RETURN-MSG` with.
const SPACE: u8 = b' ';

/// `MOVE 4 TO RETURN-CODE` in `9999-ABEND` (`COBTUPDT.cbl:232`; COBTUPDT-022).
const RETURN_CODE_FAILED: u8 = 4;

/// `DISPLAY 'OPEN FILE OK'` for file status `00` (`COBTUPDT.cbl:84-85`; COBTUPDT-004).
const OPEN_FILE_OK: &str = "OPEN FILE OK";

/// `DISPLAY 'OPEN FILE NOT OK'` for any other status (`COBTUPDT.cbl:86-87`; COBTUPDT-004).
const OPEN_FILE_NOT_OK: &str = "OPEN FILE NOT OK";

/// The literal `DISPLAY 'PROCESSING   ' WS-INPUT-REC` shows ahead of the record, `PROCESSING` and
/// three spaces (`COBTUPDT.cbl:105`; COBTUPDT-006).
const PROCESSING: &str = "PROCESSING   ";

/// The byte the `PROCESSING` line and `Debug` show in each position of `INPUT-REC-DESC`, whatever
/// the description holds (Cardholder Data Redaction; COBTUPDT-006, X-MNTTRDB2-10).
const DESC_MASK: u8 = b'*';

/// `INPUT-REC-DESC` as the job log and `Debug` show it: one [`DESC_MASK`] per position.
const MASKED_DESC: [u8; DESC_LEN] = [DESC_MASK; DESC_LEN];

/// Characters in a `PROCESSING` line: the literal and the 53 positions of `WS-INPUT-REC`, the
/// width of the COBOL line, which the masked description keeps (COBTUPDT-006).
const PROCESSING_LINE_LEN: usize = 66;

/// The action lines of `1003-TREAT-RECORD` (`COBTUPDT.cbl:112,115,118,121`; COBTUPDT-007,
/// COBTUPDT-008).
const ADDING_RECORD: &str = "ADDING RECORD";
const UPDATING_RECORD: &str = "UPDATING RECORD";
const DELETING_RECORD: &str = "DELETING RECORD";
const IGNORING_COMMENTED_LINE: &str = "IGNORING COMMENTED LINE";

/// The success lines of SQLCODE 0 (`COBTUPDT.cbl:153,179,209`; COBTUPDT-010, COBTUPDT-012,
/// COBTUPDT-015).
const RECORD_INSERTED: &str = "RECORD INSERTED SUCCESSFULLY";
const RECORD_UPDATED: &str = "RECORD UPDATED SUCCESSFULLY";
const RECORD_DELETED: &str = "RECORD DELETED SUCCESSFULLY";

/// The `WHEN OTHER` message `STRING`ed into `WS-RETURN-MSG` (`COBTUPDT.cbl:123-127`; COBTUPDT-009).
const TYPE_NOT_VALID: &[u8] = b"ERROR: TYPE NOT VALID";

/// The SQLCODE `+100` message of the update and the delete (`COBTUPDT.cbl:181,211`; COBTUPDT-013,
/// COBTUPDT-016).
const NO_RECORDS_FOUND: &[u8] = b"No records found.";

/// The first two operands of the negative-SQLCODE `STRING`, the same in all three statements
/// (`COBTUPDT.cbl:156-157,187-188,218-219`; COBTUPDT-019).
const ERROR_ACCESSING: &[u8] = b"Error accessing:";
const TABLE_SQLCODE: &[u8] = b" TRANSACTION_TYPE table. SQLCODE:";

/// `WS-VAR-SQLCODE PIC ----9`, the third operand of that `STRING` (`COBTUPDT.cbl:65`).
const SQLCODE_PICTURE: EditedPicture = EditedPicture::FloatMinus5;

/// The stand-in for the edited `SQLCODE`: one space per position of `PIC ----9`.
///
/// `HALTED(H3)` (X-HALTS-2; COBTUPDT-020): the digits are Db2's `SQLCODE`, which `PostgreSQL` does
/// not report, and `MNTTRDB2.md` fixes no replacement value for them. Blank positions show that no
/// code is known without inventing a Db2 code; they are not a parity result.
const HALTED_SQLCODE: [u8; SQLCODE_PICTURE.width()] = [SPACE; SQLCODE_PICTURE.width()];

const _: () = assert!(
    1 + 2 + DESC_LEN == INPUT_RECORD_LEN,
    "WS-INPUT-REC is PIC X(1), PIC X(2) and PIC X(50)"
);
const _: () = assert!(PROCESSING.len() == 13, "the PROCESSING literal is 13 bytes");
const _: () = assert!(
    PROCESSING.len() + 1 + 2 + MASKED_DESC.len() == PROCESSING_LINE_LEN
        && PROCESSING.len() + INPUT_RECORD_LEN == PROCESSING_LINE_LEN,
    "the masked PROCESSING line keeps the 66 characters of the COBOL line"
);
const _: () = assert!(
    TYPE_NOT_VALID.len() == 21 && NO_RECORDS_FOUND.len() == 17,
    "the two fixed messages are 21 and 17 bytes"
);
const _: () = assert!(
    ERROR_ACCESSING.len() + TABLE_SQLCODE.len() + HALTED_SQLCODE.len() == 54
        && 54 <= RETURN_MSG_LEN,
    "the SQL error text is 54 bytes and fits WS-RETURN-MSG"
);

/// Why a line of the `--input` file is not a COBTUPDT record (COBTUPDT-003, COBTUPDT-004).
///
/// It names the 1-based line, when known, and the failed check, never a byte of the line: the
/// codec error carries the field name, offset and width and, for a body of the wrong length, the
/// two lengths only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InputError {
    /// The 1-based line of the `--input` file, set by [`decode_input`]; `None` for a body checked
    /// alone by [`parse_input`].
    pub line: Option<usize>,
    /// The failed check: [`ErrorKind::WidthMismatch`](copybook::ErrorKind::WidthMismatch) for field
    /// `"record"` when the body is not 53 bytes, or
    /// [`ErrorKind::NonPrintable`](copybook::ErrorKind::NonPrintable) for the field holding a byte
    /// outside printable ASCII: a control byte, a CR inside the body or a byte above `0x7E`
    /// (PIC-002).
    pub cause: Error,
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "MNTTRDB2 input line {line}: {}", self.cause),
            None => write!(f, "MNTTRDB2 input record: {}", self.cause),
        }
    }
}

impl std::error::Error for InputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

/// One input record, the 53-byte group `WS-INPUT-REC` every rule after the read uses
/// (`COBTUPDT.cbl:71-77,101`; COBTUPDT-002).
///
/// The bytes are kept exactly as read, every one printable ASCII (PIC-001, PIC-002). The record
/// also holds the host-variable pair `:INPUT-REC-NUMBER, :INPUT-REC-DESC` that the insert and the
/// update bind, built once when the record is decoded, so that [`Maintenance::plan`] cannot fail.
///
/// `INPUT-REC-DESC` is unvalidated free text, a comment's included, so it can hold a card number,
/// an SSN or a password. `Debug` therefore shows what the `PROCESSING` line shows: the action byte
/// and the two code bytes as read, then 50 `*` in place of the description (Cardholder Data
/// Redaction; COBTUPDT-006, X-MNTTRDB2-10). [`raw`](Self::raw), [`description`](Self::description)
/// and [`encode`](FixedRecord::encode) return the description's bytes for the codec and the SQL
/// binds, never for a log line.
#[derive(Clone, PartialEq, Eq)]
pub struct MaintenanceRecord {
    raw: [u8; INPUT_RECORD_LEN],
    host: Db2TransactionType,
}

impl MaintenanceRecord {
    /// The action byte, `INPUT-REC-TYPE PIC X(1)` (column 1), which `1003-TREAT-RECORD` evaluates
    /// (COBTUPDT-007).
    #[must_use]
    pub const fn kind_byte(&self) -> u8 {
        let [kind, ..] = self.raw;
        kind
    }

    /// The type code, `INPUT-REC-NUMBER PIC X(2)` (columns 2-3), exactly as read: any two printable
    /// bytes, never edited as numeric (COBTUPDT-002; X-MNTTRDB2-6).
    #[must_use]
    pub fn code(&self) -> PicX<2> {
        let [_, first, second, ..] = self.raw;
        PicX::from_array([first, second])
    }

    /// The description, `INPUT-REC-DESC PIC X(50)` (columns 4-53), all 50 bytes with their
    /// trailing spaces, as the insert and the update bind it (COBTUPDT-010, COBTUPDT-012). It may
    /// hold a protected value, so it is for those binds only, never for a log line (COBTUPDT-006).
    #[must_use]
    pub fn description(&self) -> PicX<DESC_LEN> {
        let [_, _, _, description @ ..] = self.raw;
        PicX::from_array(description)
    }

    /// The 53 record bytes as read, the line body without its terminator (COBTUPDT-002), for the
    /// codec and its checks. They include the description, so they never form a log line: the
    /// job log shows the masked form of [`Maintenance::plan`] (COBTUPDT-006; X-MNTTRDB2-10).
    #[must_use]
    pub const fn raw(&self) -> &[u8; INPUT_RECORD_LEN] {
        &self.raw
    }

    /// The 53 positions of `WS-INPUT-REC` as the job log and `Debug` show them: the action byte
    /// and the two code bytes as read, then [`MASKED_DESC`] (COBTUPDT-006; X-MNTTRDB2-10).
    fn masked(&self) -> [u8; INPUT_RECORD_LEN] {
        group(
            &PicX::from_array([self.kind_byte()]),
            &self.code(),
            &PicX::from_array(MASKED_DESC),
        )
    }
}

impl fmt::Debug for MaintenanceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MaintenanceRecord(\"{}\")", self.masked().escape_ascii())
    }
}

impl FixedRecord for MaintenanceRecord {
    const WIDTH: usize = INPUT_RECORD_LEN;

    /// Decodes the three fields of `WS-INPUT-REC` from exactly 53 bytes (codec A; COBTUPDT-002).
    ///
    /// # Errors
    ///
    /// [`ErrorKind::WidthMismatch`](copybook::ErrorKind::WidthMismatch) for field `"record"` when
    /// `bytes` is not 53 bytes long; [`ErrorKind::NonPrintable`](copybook::ErrorKind::NonPrintable)
    /// for `INPUT-REC-TYPE`, `INPUT-REC-NUMBER` or `INPUT-REC-DESC` when that field holds a byte
    /// outside `0x20`–`0x7E` (PIC-002). Every input is accepted or rejected without a panic (Total
    /// Parsers Over Untrusted Input).
    fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = RecordReader::new(bytes, Self::WIDTH)?;
        let kind: PicX<1> = reader.pic_x(TYPE_FIELD)?;
        let code: PicX<2> = reader.pic_x(NUMBER_FIELD)?;
        let description: PicX<DESC_LEN> = reader.pic_x(DESC_FIELD)?;
        reader.finish()?;
        // `:INPUT-REC-DESC` binds all 50 bytes, so the stored description is 50 bytes long
        // (X-MNTTRDB2-3); 50 bytes are within `VARCHAR(50)`, so `new` accepts it.
        let host = Db2TransactionType::new(code.clone(), text(description.as_bytes()))?;
        Ok(Self {
            raw: group(&kind, &code, &description),
            host,
        })
    }

    fn encode(&self) -> Vec<u8> {
        self.raw.to_vec()
    }
}

/// The 53 bytes of `WS-INPUT-REC` from its three fields, in declaration order.
fn group(kind: &PicX<1>, code: &PicX<2>, description: &PicX<DESC_LEN>) -> [u8; INPUT_RECORD_LEN] {
    let [kind_byte] = *kind.as_bytes();
    let [code_first, code_second] = *code.as_bytes();
    let mut raw = [SPACE; INPUT_RECORD_LEN];
    let [kind_slot, first_slot, second_slot, description_slot @ ..] = &mut raw;
    *kind_slot = kind_byte;
    *first_slot = code_first;
    *second_slot = code_second;
    *description_slot = *description.as_bytes();
    raw
}

/// Text of bytes that are printable ASCII: the record bytes after decoding, or the module's own
/// literals and spaces. Each byte becomes the character of the same code, so the text has exactly
/// one character per byte.
fn text(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

/// Decodes one line body, without its terminator, into a record (COBTUPDT-002, COBTUPDT-003).
///
/// The body must be exactly 53 printable ASCII bytes; nothing is padded, trimmed or cut (R7).
///
/// # Errors
///
/// An [`InputError`] with `line` `None` whose `cause` is the codec error of
/// [`MaintenanceRecord`]'s `decode`: a body that is not 53 bytes long, or a field holding a byte
/// outside `0x20`–`0x7E`. Total for any input (Total Parsers Over Untrusted Input).
pub fn parse_input(body: &[u8]) -> Result<MaintenanceRecord, InputError> {
    MaintenanceRecord::decode(body).map_err(|cause| InputError { line: None, cause })
}

/// Decodes a whole `--input` file into its records, in file order (COBTUPDT-003).
///
/// `copybook::framing` splits the file at each LF (PIC-014): a CR immediately before the LF
/// belongs to the terminator, a last line without LF has none, and an empty file holds no record
/// (COBTUPDT-025). Each body is decoded by [`parse_input`].
///
/// # Errors
///
/// The [`InputError`] of the first line that does not decode, with `line` its 1-based number.
/// The job then processes no record at all (COBTUPDT-004). Total for any input.
pub fn decode_input(bytes: &[u8]) -> Result<Vec<MaintenanceRecord>, InputError> {
    lines(bytes)
        .enumerate()
        .map(|(index, line)| {
            parse_input(line.body).map_err(|err| InputError {
                line: Some(index.saturating_add(1)),
                ..err
            })
        })
        .collect()
}

/// What `1003-TREAT-RECORD` does with one record (COBTUPDT-007 to COBTUPDT-009).
///
/// The job executes the three statements against `db2.transaction_type` only (COBTUPDT-026).
///
/// The row of an insert or an update holds all 50 description bytes for the binds. `Debug` shows
/// the variant and the code, and a row's description as 50 `*`, never its bytes (Cardholder Data
/// Redaction; COBTUPDT-006, X-MNTTRDB2-10).
#[derive(Clone, PartialEq, Eq)]
pub enum MaintenanceAction {
    /// Action `A`: `INSERT INTO … (TR_TYPE, TR_DESCRIPTION) VALUES (:INPUT-REC-NUMBER,
    /// :INPUT-REC-DESC)`, the row holding the code and all 50 description bytes (COBTUPDT-010,
    /// COBTUPDT-011).
    Insert(Db2TransactionType),
    /// Action `U`: `UPDATE … SET TR_DESCRIPTION = :INPUT-REC-DESC WHERE TR_TYPE =
    /// :INPUT-REC-NUMBER`, with the same row; it never inserts (COBTUPDT-012 to COBTUPDT-014).
    Update(Db2TransactionType),
    /// Action `D`: `DELETE … WHERE TR_TYPE = :INPUT-REC-NUMBER`; the description plays no part
    /// (COBTUPDT-015 to COBTUPDT-017).
    Delete(PicX<2>),
    /// Action `*`: a comment line, no SQL (COBTUPDT-008).
    Ignore,
    /// Any other action byte, lower case included: no SQL; [`Maintenance::plan`] has already
    /// displayed the message and set return code 4 (COBTUPDT-009).
    InvalidType,
}

impl fmt::Debug for MaintenanceAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Insert(row) => masked_row(f, "Insert", row),
            Self::Update(row) => masked_row(f, "Update", row),
            Self::Delete(code) => f.debug_tuple("Delete").field(code).finish(),
            Self::Ignore => f.write_str("Ignore"),
            Self::InvalidType => f.write_str("InvalidType"),
        }
    }
}

/// `Debug` of the host row of an insert or an update: its code, and [`MASKED_DESC`] in place of
/// its description (COBTUPDT-006; X-MNTTRDB2-10).
fn masked_row(f: &mut fmt::Formatter<'_>, variant: &str, row: &Db2TransactionType) -> fmt::Result {
    f.debug_struct(variant)
        .field("tr_type", row.tr_type())
        .field(
            "tr_description",
            &format_args!("\"{}\"", MASKED_DESC.escape_ascii()),
        )
        .finish()
}

/// How the statement of an [`Insert`](MaintenanceAction::Insert),
/// [`Update`](MaintenanceAction::Update) or [`Delete`](MaintenanceAction::Delete) ended, standing
/// for the Db2 `SQLCODE` that COBTUPDT tests, as `MNTTRDB2.md` maps them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SqlOutcome {
    /// The statement completed and, for an update or delete, changed a row: SQLCODE 0.
    Ok,
    /// An update or delete changed no row: SQLCODE `+100` (COBTUPDT-013, COBTUPDT-016).
    NotFound,
    /// The statement failed: a negative SQLCODE, such as `unique_violation` on the insert
    /// (COBTUPDT-011), `foreign_key_violation` on the delete of a type with categories
    /// (COBTUPDT-017), a deadlock or lock timeout, or any other error (COBTUPDT-014, COBTUPDT-019).
    Failed,
}

/// The file status `OPEN INPUT TR-RECORD` ends with, which `0001-OPEN-FILES` tests
/// (COBTUPDT-004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpenStatus {
    /// Status `00`; in the replacement, the `--input` file was read in full and every line
    /// decoded.
    Ok,
    /// Any other status; in the replacement, the file could not be read or a line did not decode,
    /// and the job processes no record.
    NotOk,
}

/// The run state COBTUPDT carries from record to record: `WS-RETURN-MSG PIC X(80)` and
/// `RETURN-CODE` (COBTUPDT-021, COBTUPDT-023).
///
/// One value serves one run: create it with [`new`](Self::new), call [`plan`](Self::plan) for each
/// record in file order, then [`complete`](Self::complete) with the outcome of the statement the
/// plan named, and read [`return_code`](Self::return_code) after the last record. Every method
/// returns its job-log lines in program order, one `DISPLAY` per line, trailing spaces included.
#[derive(Clone, PartialEq, Eq)]
pub struct Maintenance {
    /// `WS-RETURN-MSG`: 80 spaces at the start of the run, never cleared (COBTUPDT-021).
    return_msg: [u8; RETURN_MSG_LEN],
    /// `RETURN-CODE`: 0 until `9999-ABEND` first sets it to 4, never set back (COBTUPDT-023).
    return_code: u8,
}

impl Maintenance {
    /// The state at the start of a run: `WS-RETURN-MSG` holds 80 spaces (`VALUE SPACES`) and the
    /// return code is 0 (COBTUPDT-021, COBTUPDT-023).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            return_msg: [SPACE; RETURN_MSG_LEN],
            return_code: 0,
        }
    }

    /// The first job-log line, `0001-OPEN-FILES`'s: `OPEN FILE OK` for [`OpenStatus::Ok`],
    /// `OPEN FILE NOT OK` otherwise (COBTUPDT-004).
    #[must_use]
    pub fn open_lines(status: OpenStatus) -> Vec<String> {
        let line = match status {
            OpenStatus::Ok => OPEN_FILE_OK,
            OpenStatus::NotOk => OPEN_FILE_NOT_OK,
        };
        vec![line.to_owned()]
    }

    /// The lines `1002-READ-RECORDS` and `1003-TREAT-RECORD` display for `record`, and the
    /// statement to execute next (COBTUPDT-006 to COBTUPDT-009).
    ///
    /// The first line is `PROCESSING   `, the action byte, the two code bytes as read and 50 `*`
    /// in place of the description, 66 characters, the width of the COBOL line, for every record:
    /// the description never reaches the job log (Cardholder Data Redaction; COBTUPDT-006,
    /// X-MNTTRDB2-10). The action byte is compared exactly: `A`, `U` and `D` add `ADDING RECORD`,
    /// `UPDATING RECORD` or `DELETING RECORD` and name their statement; `*` adds
    /// `IGNORING COMMENTED LINE` (COBTUPDT-007, COBTUPDT-008). Any other byte writes
    /// `ERROR: TYPE NOT VALID` over the start of `WS-RETURN-MSG`, adds the 80 bytes of that field
    /// as a line and sets the return code to 4, with no action line (COBTUPDT-009, COBTUPDT-021,
    /// COBTUPDT-022).
    #[must_use]
    pub fn plan(&mut self, record: &MaintenanceRecord) -> (Vec<String>, MaintenanceAction) {
        let mut processing = String::with_capacity(PROCESSING_LINE_LEN);
        processing.push_str(PROCESSING);
        processing.push_str(&text(&record.masked()));
        let mut lines = vec![processing];
        let action = match record.kind_byte() {
            b'A' => {
                lines.push(ADDING_RECORD.to_owned());
                MaintenanceAction::Insert(record.host.clone())
            }
            b'U' => {
                lines.push(UPDATING_RECORD.to_owned());
                MaintenanceAction::Update(record.host.clone())
            }
            b'D' => {
                lines.push(DELETING_RECORD.to_owned());
                MaintenanceAction::Delete(record.code())
            }
            b'*' => {
                lines.push(IGNORING_COMMENTED_LINE.to_owned());
                MaintenanceAction::Ignore
            }
            _ => {
                lines.push(self.abend(&[TYPE_NOT_VALID]));
                MaintenanceAction::InvalidType
            }
        };
        (lines, action)
    }

    /// The lines the statement's `EVALUATE` displays for `outcome` (COBTUPDT-010 to COBTUPDT-022).
    ///
    /// - [`SqlOutcome::Ok`]: `RECORD INSERTED SUCCESSFULLY`, `RECORD UPDATED SUCCESSFULLY` or
    ///   `RECORD DELETED SUCCESSFULLY`.
    /// - [`SqlOutcome::NotFound`] for an update or delete: `No records found.` written over the
    ///   start of `WS-RETURN-MSG`, then its 80 bytes as one line, and return code 4.
    /// - [`SqlOutcome::Failed`]: `Error accessing: TRANSACTION_TYPE table. SQLCODE:` and the five
    ///   `SQLCODE` positions, 54 bytes written over the start of `WS-RETURN-MSG`, then its 80 bytes
    ///   as one line, and return code 4. The text names `TRANSACTION_TYPE` whichever statement
    ///   failed (COBTUPDT-019); its last five characters are `HALTED(H3)` (COBTUPDT-020).
    ///
    /// An insert has no `WHEN` for a positive SQLCODE, so [`SqlOutcome::NotFound`] for an
    /// [`Insert`](MaintenanceAction::Insert), which `PostgreSQL` never yields, displays nothing and
    /// leaves the state unchanged (COBTUPDT-018). [`Ignore`](MaintenanceAction::Ignore) and
    /// [`InvalidType`](MaintenanceAction::InvalidType) ran no statement: they return no line and
    /// change nothing, so calling this method for them is optional.
    pub fn complete(&mut self, action: &MaintenanceAction, outcome: SqlOutcome) -> Vec<String> {
        match (action, outcome) {
            (MaintenanceAction::Insert(_), SqlOutcome::Ok) => vec![RECORD_INSERTED.to_owned()],
            (MaintenanceAction::Update(_), SqlOutcome::Ok) => vec![RECORD_UPDATED.to_owned()],
            (MaintenanceAction::Delete(_), SqlOutcome::Ok) => vec![RECORD_DELETED.to_owned()],
            (MaintenanceAction::Update(_) | MaintenanceAction::Delete(_), SqlOutcome::NotFound) => {
                vec![self.abend(&[NO_RECORDS_FOUND])]
            }
            (
                MaintenanceAction::Insert(_)
                | MaintenanceAction::Update(_)
                | MaintenanceAction::Delete(_),
                SqlOutcome::Failed,
            ) => {
                // HALTED(H3): the last operand stands for the edited Db2 SQLCODE (COBTUPDT-020).
                vec![self.abend(&[ERROR_ACCESSING, TABLE_SQLCODE, &HALTED_SQLCODE])]
            }
            (MaintenanceAction::Insert(_), SqlOutcome::NotFound)
            | (MaintenanceAction::Ignore | MaintenanceAction::InvalidType, _) => Vec::new(),
        }
    }

    /// `RETURN-CODE` so far: 4 once any record reached `9999-ABEND`, 0 otherwise, an empty run
    /// included (COBTUPDT-023). The job exits with it after its commit.
    #[must_use]
    pub const fn return_code(&self) -> u8 {
        self.return_code
    }

    /// `STRING operands DELIMITED BY SIZE INTO WS-RETURN-MSG`, then `PERFORM 9999-ABEND`:
    /// `DISPLAY WS-RETURN-MSG` and `MOVE 4 TO RETURN-CODE` (COBTUPDT-021, COBTUPDT-022). Returns
    /// the displayed line, all 80 bytes.
    fn abend(&mut self, operands: &[&[u8]]) -> String {
        self.overlay(operands);
        self.return_code = RETURN_CODE_FAILED;
        text(&self.return_msg)
    }

    /// COBOL `STRING … DELIMITED BY SIZE INTO WS-RETURN-MSG` with no `POINTER`: the operands'
    /// bytes, in order, from position 1, stopping at position 80; every later position keeps what
    /// it held (COBTUPDT-021).
    fn overlay(&mut self, operands: &[&[u8]]) {
        let source = operands.iter().flat_map(|operand| operand.iter());
        for (target, &byte) in self.return_msg.iter_mut().zip(source) {
            *target = byte;
        }
    }
}

impl Default for Maintenance {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Maintenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Maintenance")
            .field(
                "return_msg",
                &format_args!("\"{}\"", self.return_msg.escape_ascii()),
            )
            .field("return_code", &self.return_code)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{Error, ErrorKind, FixedRecord, PicX};

    use super::{
        INPUT_RECORD_LEN, InputError, Maintenance, MaintenanceAction, MaintenanceRecord,
        OpenStatus, SqlOutcome, decode_input, parse_input,
    };
    use crate::records::Db2TransactionType;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A position an expected line leaves unasserted: the five `SQLCODE` characters, halted.
    const HALTED: u8 = b'?';

    /// The error text of COBTUPDT-019 with its five halted `SQLCODE` positions (COBTUPDT-020).
    const SQL_ERROR: &str = "Error accessing: TRANSACTION_TYPE table. SQLCODE:?????";

    /// A record as `MNTTRDB2.md` writes it: action byte, code, description padded to 50 bytes.
    fn body(kind: char, code: &str, description: &str) -> Vec<u8> {
        format!("{kind}{code}{description:<50}").into_bytes()
    }

    /// `text` padded with spaces to the 80 bytes of `WS-RETURN-MSG`.
    fn msg(text: &str) -> String {
        format!("{text:<80}")
    }

    /// The 50 `*` that `MNTTRDB2.md` writes in place of the description (COBTUPDT-006).
    const MASK: &str = "**************************************************";

    /// The `PROCESSING` line of a record body as `MNTTRDB2.md` writes it: the literal, the action
    /// byte and the two code bytes, then the 50 `*` of [`MASK`] (COBTUPDT-006).
    fn processing(body: &[u8]) -> String {
        let head = String::from_utf8_lossy(body.get(..3).unwrap_or_default());
        format!("PROCESSING   {head}{MASK}")
    }

    /// The Db2 row an `A` or `U` record binds: the code and all 50 description bytes.
    fn host_row(code: &str, description: &str) -> Result<Db2TransactionType, Box<dyn StdError>> {
        Ok(Db2TransactionType::new(
            PicX::<2>::from_text(code)?,
            format!("{description:<50}"),
        )?)
    }

    /// Compares every line, byte by byte, except the positions `expected` marks [`HALTED`].
    fn assert_lines(actual: &[String], expected: &[String], context: &str) {
        assert_eq!(actual.len(), expected.len(), "{context}: line count");
        for (line, (got, want)) in actual.iter().zip(expected).enumerate() {
            assert_eq!(got.len(), want.len(), "{context}: length of line {line}");
            for (position, (a, e)) in got.bytes().zip(want.bytes()).enumerate() {
                if e != HALTED {
                    assert_eq!(a, e, "{context}: line {line}, position {}", position + 1);
                }
            }
        }
    }

    /// The lines and action of one record, completed with `outcome` when its statement ran.
    fn treat(
        run: &mut Maintenance,
        record: &[u8],
        outcome: Option<SqlOutcome>,
    ) -> Result<(Vec<String>, MaintenanceAction), Box<dyn StdError>> {
        let (mut lines, action) = run.plan(&parse_input(record)?);
        if let Some(outcome) = outcome {
            lines.extend(run.complete(&action, outcome));
        }
        Ok((lines, action))
    }

    /// One derived record of `MNTTRDB2.md`: its rule, its bytes, the outcome a freshly migrated
    /// database gives its statement, and the lines after its `PROCESSING` line.
    struct Step {
        rule: &'static str,
        body: Vec<u8>,
        outcome: Option<SqlOutcome>,
        after: Vec<String>,
    }

    fn step(
        rule: &'static str,
        body: Vec<u8>,
        outcome: Option<SqlOutcome>,
        after: &[&str],
    ) -> Step {
        let after = after.iter().map(|line| (*line).to_owned()).collect();
        Step {
            rule,
            body,
            outcome,
            after,
        }
    }

    /// COBTUPDT-101 to COBTUPDT-110, in file order.
    fn derived_steps() -> Vec<Step> {
        let not_found = msg("No records found.");
        let sql_error = msg(SQL_ERROR);
        vec![
            step(
                "COBTUPDT-101",
                body('*', "  ", "AUDIT TEST INPUT FOR MNTTRDB2"),
                None,
                &["IGNORING COMMENTED LINE"],
            ),
            step(
                "COBTUPDT-102",
                body('A', "08", "AUDIT TEST TYPE EIGHT"),
                Some(SqlOutcome::Ok),
                &["ADDING RECORD", "RECORD INSERTED SUCCESSFULLY"],
            ),
            step(
                "COBTUPDT-103",
                body('U', "01", "AUDIT TEST PURCHASE RENAMED"),
                Some(SqlOutcome::Ok),
                &["UPDATING RECORD", "RECORD UPDATED SUCCESSFULLY"],
            ),
            step(
                "COBTUPDT-104",
                body('U', "99", "AUDIT TEST ABSENT TYPE"),
                Some(SqlOutcome::NotFound),
                &["UPDATING RECORD", &not_found],
            ),
            step(
                "COBTUPDT-105",
                body('D', "08", ""),
                Some(SqlOutcome::Ok),
                &["DELETING RECORD", "RECORD DELETED SUCCESSFULLY"],
            ),
            step(
                "COBTUPDT-106",
                body('D', "99", ""),
                Some(SqlOutcome::NotFound),
                &["DELETING RECORD", &not_found],
            ),
            step(
                "COBTUPDT-107",
                body('X', "01", "AUDIT TEST NOT APPLIED"),
                None,
                &[&msg("ERROR: TYPE NOT VALID")],
            ),
            step(
                "COBTUPDT-108",
                body('U', "98", "AUDIT TEST ABSENT TYPE TWO"),
                Some(SqlOutcome::NotFound),
                &["UPDATING RECORD", &msg("No records found.ALID")],
            ),
            step(
                "COBTUPDT-109",
                body('A', "01", "AUDIT TEST DUPLICATE KEY"),
                Some(SqlOutcome::Failed),
                &["ADDING RECORD", &sql_error],
            ),
            step(
                "COBTUPDT-110",
                body('D', "02", ""),
                Some(SqlOutcome::Failed),
                &["DELETING RECORD", &sql_error],
            ),
        ]
    }

    /// The `--input` file holding `steps`, each record followed by LF.
    fn input_file(steps: &[Step]) -> Vec<u8> {
        steps
            .iter()
            .flat_map(|step| step.body.iter().copied().chain([b'\n']))
            .collect()
    }

    // audit: COBTUPDT-111
    #[test]
    fn the_derived_input_yields_the_audited_job_log_and_return_code() -> TestResult {
        let steps = derived_steps();
        let records = decode_input(&input_file(&steps))?;
        assert_eq!(records.len(), 10);
        let mut run = Maintenance::new();
        let mut log = Maintenance::open_lines(OpenStatus::Ok);
        for (step, record) in steps.iter().zip(&records) {
            assert_eq!(
                record.raw().as_slice(),
                step.body.as_slice(),
                "{}",
                step.rule
            );
            let (mut lines, action) = run.plan(record);
            if let Some(outcome) = step.outcome {
                lines.extend(run.complete(&action, outcome));
            }
            let mut expected = vec![processing(&step.body)];
            expected.extend(step.after.iter().cloned());
            assert_lines(&lines, &expected, step.rule);
            log.extend(lines);
        }
        assert_eq!(log.len(), 29);
        assert_eq!(log.first().map(String::as_str), Some("OPEN FILE OK"));
        assert_eq!(run.return_code(), 4);
        Ok(())
    }

    // audit: COBTUPDT-102
    // audit: COBTUPDT-103
    // audit: COBTUPDT-105
    // audit: COBTUPDT-101
    // audit: COBTUPDT-107
    #[test]
    fn the_derived_records_plan_their_audited_statements() -> TestResult {
        let mut run = Maintenance::new();
        let mut plan = |record: &[u8]| -> Result<MaintenanceAction, Box<dyn StdError>> {
            Ok(run.plan(&parse_input(record)?).1)
        };
        let insert = plan(&body('A', "08", "AUDIT TEST TYPE EIGHT"))?;
        assert_eq!(
            insert,
            MaintenanceAction::Insert(host_row("08", "AUDIT TEST TYPE EIGHT")?)
        );
        let update = plan(&body('U', "01", "AUDIT TEST PURCHASE RENAMED"))?;
        let MaintenanceAction::Update(row) = &update else {
            return Err("COBTUPDT-103 plans an update".into());
        };
        // COBTUPDT-012: all 50 bytes are bound, so the stored length is 50 (X-MNTTRDB2-3).
        assert_eq!(row.tr_description().len(), 50);
        assert!(
            row.tr_description()
                .starts_with("AUDIT TEST PURCHASE RENAMED ")
        );
        assert_eq!(row.tr_type(), &PicX::<2>::from_text("01")?);
        assert_eq!(
            plan(&body('D', "08", ""))?,
            MaintenanceAction::Delete(PicX::<2>::from_text("08")?)
        );
        assert_eq!(
            plan(&body('*', "  ", "AUDIT TEST INPUT FOR MNTTRDB2"))?,
            MaintenanceAction::Ignore
        );
        assert_eq!(
            plan(&body('X', "01", "AUDIT TEST NOT APPLIED"))?,
            MaintenanceAction::InvalidType
        );
        Ok(())
    }

    // audit: COBTUPDT-112
    // audit: COBTUPDT-023
    #[test]
    fn a_run_without_failures_ends_with_return_code_zero() -> TestResult {
        let steps: Vec<Step> = derived_steps()
            .into_iter()
            .filter(|step| {
                [
                    "COBTUPDT-101",
                    "COBTUPDT-102",
                    "COBTUPDT-103",
                    "COBTUPDT-105",
                ]
                .contains(&step.rule)
            })
            .collect();
        let records = decode_input(&input_file(&steps))?;
        let mut run = Maintenance::new();
        let mut log = Maintenance::open_lines(OpenStatus::Ok);
        for (step, record) in steps.iter().zip(&records) {
            let (lines, action) = run.plan(record);
            log.extend(lines);
            if let Some(outcome) = step.outcome {
                log.extend(run.complete(&action, outcome));
            }
        }
        assert_eq!(log.len(), 12);
        assert!(
            log.iter().all(|line| line.len() != 80),
            "WS-RETURN-MSG is never displayed"
        );
        assert_eq!(run.return_code(), 0);
        Ok(())
    }

    // audit: COBTUPDT-113
    // audit: COBTUPDT-025
    #[test]
    fn an_empty_input_holds_no_record_and_keeps_return_code_zero() -> TestResult {
        assert!(decode_input(b"")?.is_empty());
        let run = Maintenance::default();
        assert_eq!(run, Maintenance::new());
        assert_eq!(run.return_code(), 0);
        assert_eq!(Maintenance::open_lines(OpenStatus::Ok), ["OPEN FILE OK"]);
        Ok(())
    }

    // audit: COBTUPDT-004
    #[test]
    fn the_open_line_names_the_open_status() {
        assert_eq!(Maintenance::open_lines(OpenStatus::Ok), ["OPEN FILE OK"]);
        assert_eq!(
            Maintenance::open_lines(OpenStatus::NotOk),
            ["OPEN FILE NOT OK"]
        );
    }

    /// The message line of an invalid action byte.
    fn invalid(run: &mut Maintenance) -> Result<String, Box<dyn StdError>> {
        let (lines, _) = treat(run, &body('X', "01", ""), None)?;
        lines
            .last()
            .cloned()
            .ok_or_else(|| "no message line".into())
    }

    /// The message line of an update that finds no row.
    fn not_found(run: &mut Maintenance) -> Result<String, Box<dyn StdError>> {
        let (lines, _) = treat(run, &body('U', "99", ""), Some(SqlOutcome::NotFound))?;
        lines
            .last()
            .cloned()
            .ok_or_else(|| "no message line".into())
    }

    /// The message line of an insert that fails.
    fn failed(run: &mut Maintenance) -> Result<String, Box<dyn StdError>> {
        let (lines, _) = treat(run, &body('A', "01", ""), Some(SqlOutcome::Failed))?;
        lines
            .last()
            .cloned()
            .ok_or_else(|| "no message line".into())
    }

    // audit: COBTUPDT-021
    // audit: COBTUPDT-022
    #[test]
    fn each_message_overwrites_only_its_own_prefix() -> TestResult {
        let mut run = Maintenance::new();
        assert_eq!(not_found(&mut run)?, msg("No records found."));
        assert_eq!(invalid(&mut run)?, msg("ERROR: TYPE NOT VALID"));
        assert_eq!(not_found(&mut run)?, msg("No records found.ALID"));
        let error = failed(&mut run)?;
        assert_lines(&[error], &[msg(SQL_ERROR)], "error text");
        let residue = [
            not_found(&mut run)?,
            invalid(&mut run)?,
            not_found(&mut run)?,
        ];
        let expected = [
            msg("No records found.TRANSACTION_TYPE table. SQLCODE:?????"),
            msg("ERROR: TYPE NOT VALIDSACTION_TYPE table. SQLCODE:?????"),
            msg("No records found.ALIDSACTION_TYPE table. SQLCODE:?????"),
        ];
        assert_lines(&residue, &expected, "residue after the error text");
        assert_eq!(run.return_code(), 4);
        Ok(())
    }

    // audit: COBTUPDT-019
    #[test]
    fn every_failed_statement_shows_the_transaction_type_error_text() -> TestResult {
        for record in [
            body('A', "01", ""),
            body('U', "01", ""),
            body('D', "02", ""),
        ] {
            let mut run = Maintenance::new();
            let (lines, _) = treat(&mut run, &record, Some(SqlOutcome::Failed))?;
            let message = lines.last().cloned().unwrap_or_default();
            assert_lines(&[message], &[msg(SQL_ERROR)], "failed statement");
            assert_eq!(run.return_code(), 4);
        }
        Ok(())
    }

    // Blocked by H3 (candidate X-HALTS-2): this pins the replacement's own blank stand-in for the
    // edited SQLCODE, which is no COBOL parity result.
    #[test]
    fn the_halted_sqlcode_positions_hold_the_blank_stand_in() -> TestResult {
        let line = failed(&mut Maintenance::new())?;
        assert_eq!(line.get(49..54), Some("     "));
        Ok(())
    }

    // audit: COBTUPDT-007
    // audit: COBTUPDT-009
    #[test]
    fn only_the_four_exact_action_bytes_are_actions() -> TestResult {
        for kind in ['a', 'u', 'd', ' ', '0', '8', 'X', '#', '~'] {
            let record = body(kind, "01", "AUDIT TEST NOT APPLIED");
            let mut run = Maintenance::new();
            let (lines, action) = treat(&mut run, &record, None)?;
            assert_eq!(
                action,
                MaintenanceAction::InvalidType,
                "action byte {kind:?}"
            );
            let expected = [processing(&record), msg("ERROR: TYPE NOT VALID")];
            assert_eq!(lines, expected, "action byte {kind:?}");
            assert_eq!(run.return_code(), 4, "action byte {kind:?}");
            assert!(run.complete(&action, SqlOutcome::Failed).is_empty());
        }
        let action_lines = [
            ('A', "ADDING RECORD"),
            ('U', "UPDATING RECORD"),
            ('D', "DELETING RECORD"),
            ('*', "IGNORING COMMENTED LINE"),
        ];
        for (kind, line) in action_lines {
            let record = body(kind, "01", "");
            let mut run = Maintenance::new();
            let (lines, _) = treat(&mut run, &record, None)?;
            assert_eq!(
                lines,
                [processing(&record), line.to_owned()],
                "action byte {kind:?}"
            );
            assert_eq!(run.return_code(), 0, "action byte {kind:?}");
        }
        Ok(())
    }

    // audit: COBTUPDT-008
    #[test]
    fn a_comment_line_changes_neither_the_message_nor_the_return_code() -> TestResult {
        let mut run = Maintenance::new();
        let (lines, action) = treat(&mut run, &body('*', "ZZ", "ANY TEXT AT ALL"), None)?;
        assert_eq!(action, MaintenanceAction::Ignore);
        assert_eq!(lines.len(), 2);
        assert!(run.complete(&action, SqlOutcome::Ok).is_empty());
        assert!(run.complete(&action, SqlOutcome::Failed).is_empty());
        assert_eq!(run.return_code(), 0);
        assert_eq!(not_found(&mut run)?, msg("No records found."));
        Ok(())
    }

    // audit: COBTUPDT-018
    #[test]
    fn an_insert_has_no_branch_for_a_positive_sqlcode() -> TestResult {
        let mut run = Maintenance::new();
        let (lines, _) = treat(&mut run, &body('A', "08", "X"), Some(SqlOutcome::NotFound))?;
        assert_eq!(lines.len(), 2, "PROCESSING and ADDING RECORD only");
        assert_eq!(run.return_code(), 0);
        assert_eq!(not_found(&mut run)?, msg("No records found."));
        Ok(())
    }

    // audit: COBTUPDT-010
    // audit: COBTUPDT-012
    // audit: COBTUPDT-015
    #[test]
    fn statements_bind_the_record_fields_unvalidated_and_untrimmed() -> TestResult {
        let mut run = Maintenance::new();
        // X-MNTTRDB2-6: a code that is not numeric reaches the SQL as given.
        let (_, insert) = run.plan(&parse_input(&body('A', "A ", "  LEADING"))?);
        assert_eq!(
            insert,
            MaintenanceAction::Insert(host_row("A ", "  LEADING")?)
        );
        let (_, update) = run.plan(&parse_input(&body('U', " 8", ""))?);
        assert_eq!(update, MaintenanceAction::Update(host_row(" 8", "")?));
        let (_, delete) = run.plan(&parse_input(&body('D', "07", "IGNORED TEXT"))?);
        assert_eq!(
            delete,
            MaintenanceAction::Delete(PicX::<2>::from_text("07")?)
        );
        let outcomes = [
            (insert, "RECORD INSERTED SUCCESSFULLY"),
            (update, "RECORD UPDATED SUCCESSFULLY"),
            (delete, "RECORD DELETED SUCCESSFULLY"),
        ];
        for (action, line) in outcomes {
            assert_eq!(run.complete(&action, SqlOutcome::Ok), [line]);
        }
        assert_eq!(run.return_code(), 0);
        Ok(())
    }

    // audit: COBTUPDT-002
    // audit: COBTUPDT-006
    #[test]
    fn a_record_keeps_its_53_bytes_and_their_three_fields() -> TestResult {
        assert_eq!(INPUT_RECORD_LEN, 53);
        assert_eq!(MaintenanceRecord::WIDTH, INPUT_RECORD_LEN);
        assert_eq!(MaintenanceRecord::TRAILING_FILLER, 0);
        let bytes = body('U', "01", "AUDIT TEST PURCHASE RENAMED");
        let record = parse_input(&bytes)?;
        assert_eq!(record.kind_byte(), b'U');
        assert_eq!(record.code(), PicX::<2>::from_text("01")?);
        assert_eq!(
            record.description(),
            PicX::<50>::from_text("AUDIT TEST PURCHASE RENAMED")?
        );
        assert_eq!(record.raw().as_slice(), bytes.as_slice());
        assert_eq!(record.encode(), bytes);
        assert_eq!(MaintenanceRecord::decode(&record.encode())?, record);
        let (lines, action) = Maintenance::new().plan(&record);
        assert_eq!(lines.first().map(String::len), Some(66));
        assert_eq!(
            lines.first().map(String::as_str),
            Some(format!("PROCESSING   U01{MASK}").as_str())
        );
        assert_eq!(
            format!("{record:?}"),
            format!("MaintenanceRecord(\"U01{MASK}\")")
        );
        assert_eq!(
            format!("{action:?}"),
            format!("Update {{ tr_type: PicX(\"01\"), tr_description: \"{MASK}\" }}")
        );
        Ok(())
    }

    /// `bytes` with `byte` at `offset`.
    fn with_byte(mut bytes: Vec<u8>, offset: usize, byte: u8) -> Vec<u8> {
        if let Some(slot) = bytes.get_mut(offset) {
            *slot = byte;
        }
        bytes
    }

    // audit: COBTUPDT-003
    #[test]
    fn a_body_of_another_length_or_with_a_non_printable_byte_is_a_typed_error() {
        let valid = body('A', "08", "AUDIT TEST TYPE EIGHT");
        for len in [0, 1, 52, 54, 106] {
            let mut bytes = valid.clone();
            bytes.resize(len, b' ');
            let kind = ErrorKind::WidthMismatch {
                expected: 53,
                actual: len,
            };
            let cause = Error::new("record", 0, 53, kind);
            assert_eq!(parse_input(&bytes), Err(InputError { line: None, cause }));
        }
        let fields = [
            (0, "INPUT-REC-TYPE", 0, 1),
            (2, "INPUT-REC-NUMBER", 1, 2),
            (3, "INPUT-REC-DESC", 3, 50),
            (52, "INPUT-REC-DESC", 3, 50),
        ];
        for (at, field, offset, width) in fields {
            for byte in [0x00, 0x09, b'\r', b'\n', 0x1F, 0x7F, 0x80, 0xFF] {
                let cause = Error::new(field, offset, width, ErrorKind::NonPrintable);
                assert_eq!(
                    parse_input(&with_byte(valid.clone(), at, byte)),
                    Err(InputError { line: None, cause }),
                    "byte {byte:#04x} at {at}"
                );
            }
        }
        for byte in 0x20..=0x7E {
            assert!(parse_input(&with_byte(valid.clone(), 30, byte)).is_ok());
        }
    }

    // audit: COBTUPDT-003
    // audit: COBTUPDT-004
    #[test]
    fn decode_input_splits_lines_and_names_the_first_failing_line() -> TestResult {
        let first = body('A', "08", "AUDIT TEST TYPE EIGHT");
        let second = body('U', "01", "AUDIT TEST PURCHASE RENAMED");
        let third = body('D', "08", "");
        // LF, CRLF, and a last line without a terminator (PIC-014).
        let mut file = [first.as_slice(), b"\n", &second, b"\r\n", &third].concat();
        let records = decode_input(&file)?;
        let raws: Vec<&[u8]> = records
            .iter()
            .map(|record| record.raw().as_slice())
            .collect();
        assert_eq!(raws, [first.as_slice(), &second, &third]);

        // Only the CR immediately before the LF is terminator; the line's other CR stays in the
        // body, which is then 54 bytes long.
        file.extend_from_slice(b"\r\r\n");
        let cause = Error::new(
            "record",
            0,
            53,
            ErrorKind::WidthMismatch {
                expected: 53,
                actual: 54,
            },
        );
        assert_eq!(
            decode_input(&file),
            Err(InputError {
                line: Some(3),
                cause
            })
        );
        let lone_lf = Error::new(
            "record",
            0,
            53,
            ErrorKind::WidthMismatch {
                expected: 53,
                actual: 0,
            },
        );
        let blank_line = [first.as_slice(), b"\n\n", &second].concat();
        assert_eq!(
            decode_input(&blank_line),
            Err(InputError {
                line: Some(2),
                cause: lone_lf
            })
        );
        let tab = with_byte(second.clone(), 10, b'\t');
        let error = decode_input(&[first.as_slice(), b"\n", &tab, b"\n"].concat());
        assert_eq!(
            error.map_err(|err| (err.line, err.cause.field)),
            Err((Some(2), "INPUT-REC-DESC"))
        );
        Ok(())
    }

    #[test]
    fn an_input_error_names_the_line_and_check_but_no_record_byte() -> TestResult {
        let valid = body('A', "08", "SECRET AUDIT TEXT");
        let Err(error) = decode_input(&[valid.as_slice(), b"\nA08SECRET\n"].concat()) else {
            return Err("a 9-byte body is rejected".into());
        };
        assert_eq!(
            error.to_string(),
            "MNTTRDB2 input line 2: record (offset 0, width 53): width mismatch: expected 53 \
             bytes, found 9"
        );
        assert!(!error.to_string().contains("SECRET"));
        let Err(alone) = parse_input(&with_byte(valid, 5, 0x01)) else {
            return Err("a control byte is rejected".into());
        };
        let text = alone.to_string();
        assert!(text.starts_with("MNTTRDB2 input record: INPUT-REC-DESC (offset 3, width 50)"));
        assert!(!text.contains("SECRET"));
        assert!(std::error::Error::source(&alone).is_some());
        Ok(())
    }

    #[test]
    fn decoding_never_panics_on_arbitrary_bytes() {
        let fills = [0x00, b' ', b'A', b'*', b'\r', b'\n', 0x7F, 0xFF];
        for len in 0..=1024 {
            for fill in fills {
                let bytes = vec![fill; len];
                let printable = (0x20..=0x7E).contains(&fill);
                assert_eq!(parse_input(&bytes).is_ok(), len == 53 && printable);
                let _ = decode_input(&bytes);
            }
        }
        let valid = body('D', "02", "");
        for at in 0..INPUT_RECORD_LEN {
            for byte in 0..=u8::MAX {
                let _ = parse_input(&with_byte(valid.clone(), at, byte));
            }
        }
    }

    #[test]
    fn debug_shows_the_message_and_return_code() -> TestResult {
        let mut run = Maintenance::new();
        invalid(&mut run)?;
        assert_eq!(
            format!("{run:?}"),
            format!(
                "Maintenance {{ return_msg: \"{}\", return_code: 4 }}",
                msg("ERROR: TYPE NOT VALID")
            )
        );
        Ok(())
    }
}
