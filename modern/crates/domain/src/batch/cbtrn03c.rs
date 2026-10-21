#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! CBTRN03C, the transaction detail report of batch job TRANREPT, with the selection and
//! ordering of its SORT step (`CBTRN03C.cbl`, `TRANREPT.jcl`, `TRANREPT.prc`; AAP 0.3.4.3).
//!
//! This module ports the pure rows of `modern/audit/batch/TRANREPT.md`: the date options and the
//! DATEPARM record (CBTRN03C-001, CBTRN03C-002), the SORT `INCLUDE` and `SORT FIELDS`
//! (CBTRN03C-004, CBTRN03C-005), one pass of the read loop with its window test, card-block
//! change and lookups (CBTRN03C-011 to CBTRN03C-018), the header groups and page breaks
//! (CBTRN03C-019 to CBTRN03C-021), the detail and total records with their edited amounts
//! (CBTRN03C-022 to CBTRN03C-028), the end-of-file sequence (CBTRN03C-029 to CBTRN03C-031) and the
//! 133-byte record (CBTRN03C-032), plus the job-log texts of the program (CBTRN03C-006 to
//! CBTRN03C-008, CBTRN03C-010, CBTRN03C-012, CBTRN03C-014, CBTRN03C-016, CBTRN03C-018,
//! CBTRN03C-033 to CBTRN03C-036).
//!
//! # Flow
//!
//! `batch::jobs::tranrept` parses `--start-date` and `--end-date` into a [`DateParm`], selects
//! the `transactions` rows [`selects`] accepts in [`sort_key`] order, and reads `card_xref` and
//! the VSAM data version `transaction_types` and `transaction_categories`, never schema `db2`
//! (E23, CBTRN03C-038). It builds a [`Tranrept`] from the reference rows, passes each selected
//! row to [`Tranrept::process`], writing the records each pass gives, which it takes with
//! [`Tranrept::take_records`], shows the two [`Tranrept::end_of_file_display`] lines, and writes
//! the records [`Tranrept::finish`] returns, each followed by LF. This module does no I/O and
//! holds no path.
//!
//! # Fidelity
//!
//! Audited defects are reproduced (Behaviour constraint): the page-break test for an exact
//! multiple of 20 that a total pair can step over (X-TRANREPT-8), the per-card "account" total,
//! the last amount counted twice at end of file and the missing account total of the last card
//! (X-TRANREPT-7), and the window test whose `NEXT SENTENCE` leaves the loop (X-TRANREPT-9).
//! Every total is a `Money<9>` stored by `store::<9>()` (CBTRN03C-028); no floating point.
//!
//! # Halts and replacement choices
//!
//! - `HALTED(X-TRANREPT-H1)`: rows sharing a card number have no order the job fixes;
//!   [`SortKey`] orders them by `TRAN-ID`, the proposed resolution, which is no parity result.
//! - `HALTED(X-TRANREPT-H2)`: an empty selection runs the end-of-file path over a `TRAN-RECORD`
//!   no read has set; [`Tranrept::finish`] returns no records, which is no parity result.
//! - `HALTED(X-TRANREPT-H4)`: a card number that is not 16 digits ends the run with
//!   [`ReportError::InvalidCardNumber`], the proposed replacement behaviour.
//!
//! # Cardholder data
//!
//! No report record holds a card number (CBTRN03C-023). Card numbers stay [`CardNumber`] values,
//! keys of a private map, and every text this module builds shows only the redacted form
//! (CBTRN03C-012, CBTRN03C-014, CBTRN03C-039; Cardholder Data Redaction).
//!
//! The `TRAN-RECORD` job-log line of [`record_display`] also shows `TRAN-SOURCE`, `TRAN-DESC`,
//! `TRAN-MERCHANT-NAME`, `TRAN-MERCHANT-CITY`, `TRAN-MERCHANT-ZIP` and `FILLER` as `*` in every
//! position, whatever they hold: they are unvalidated text that can carry a card number, an SSN
//! or a password, and Cardholder Data Redaction prevails over byte parity for those 240 bytes
//! only. The line keeps its 350 bytes, and the record, its encoding and the report keep every
//! byte (CBTRN03C-012, CBTRN03C-039; X-TRANREPT-3).

use std::collections::BTreeMap;
use std::fmt;
use std::iter;

use chrono::NaiveDate;
use copybook::edited::EditedPicture;
use copybook::{FixedRecord, PicDate, PicTimestamp, PicU, PicX, RecordWriter};

use crate::money::Money;
use crate::records::{
    CardXrefRecord, TransactionCategoryRecord, TransactionRecord, TransactionTypeRecord,
};
use crate::sensitive::CardNumber;

/// Length of every report record: `FD-REPTFILE-REC PIC X(133)`, `LRECL=133,RECFM=FB`
/// (CBTRN03C-032).
pub const REPORT_RECORD_LEN: usize = 133;

/// Length of the DATEPARM record: `FD-DATEPARM-REC PIC X(80)` (CBTRN03C-001).
pub const DATEPARM_RECORD_LEN: usize = 80;

/// One report record, padded with spaces to [`REPORT_RECORD_LEN`] bytes.
pub type ReportRecord = [u8; REPORT_RECORD_LEN];

/// The first job-log line of every run (CBTRN03C-006).
pub const START_LINE: &str = "START OF EXECUTION OF PROGRAM CBTRN03C";

/// The last job-log line of a normal end (CBTRN03C-034).
pub const END_LINE: &str = "END OF EXECUTION OF PROGRAM CBTRN03C";

/// The job-log line of `9999-ABEND-PROGRAM`, the last line of a failed run (CBTRN03C-035).
pub const ABEND_LINE: &str = "ABENDING PROGRAM";

/// The open of the transaction selection failed (CBTRN03C-007).
pub const OPEN_TRANFILE_ERROR: &str = "ERROR OPENING TRANFILE";

/// The report generation file could not be created (CBTRN03C-007).
pub const OPEN_REPTFILE_ERROR: &str = "ERROR OPENING REPTFILE";

/// The read of `card_xref` failed before the first record (CBTRN03C-007).
pub const OPEN_CARDXREF_ERROR: &str = "ERROR OPENING CROSS REF FILE";

/// The read of `transaction_types` failed before the first record (CBTRN03C-007).
pub const OPEN_TRANTYPE_ERROR: &str = "ERROR OPENING TRANSACTION TYPE FILE";

/// The read of `transaction_categories` failed before the first record (CBTRN03C-007).
pub const OPEN_TRANCATG_ERROR: &str = "ERROR OPENING TRANSACTION CATG FILE";

/// Fetching the next selected row failed (CBTRN03C-010).
pub const READ_TRANFILE_ERROR: &str = "ERROR READING TRANSACTION FILE";

/// Writing a report record failed (CBTRN03C-033).
pub const WRITE_REPTFILE_ERROR: &str = "ERROR WRITING REPTFILE";

/// Closing the report generation file failed, the only close that can fail (CBTRN03C-034).
pub const CLOSE_REPTFILE_ERROR: &str = "ERROR CLOSING REPORT FILE";

/// The label of `9910-DISPLAY-IO-STATUS`, which every error path above shows after its line;
/// the status digits that follow have no counterpart (X-TRANREPT-H3; CBTRN03C-036).
pub const FILE_STATUS_LABEL: &str = "FILE STATUS IS: NNNN";

/// The status line of a lookup's `INVALID KEY`, from `MOVE 23 TO IO-STATUS` (CBTRN03C-036).
const NOT_FOUND_STATUS_LINE: &str = "FILE STATUS IS: NNNN0023";

/// Bytes of a `YYYY-MM-DD` date text, `PIC X(10)` (PIC-015).
const DATE_TEXT_LEN: usize = 10;

/// `WS-PAGE-SIZE PIC 9(03) COMP-3 VALUE 20` (CBTRN03C-021).
const PAGE_SIZE: u64 = 20;

/// `10^9`: `WS-LINE-COUNTER PIC 9(09) COMP-3` keeps the low nine digits of every `ADD`.
const LINE_COUNTER_MODULUS: u64 = 1_000_000_000;

/// Offset of `TRAN-CARD-NUM` in the 350-byte `TRAN-RECORD` (CVTRA05Y-011).
const CARD_NUM_OFFSET: usize = 262;

/// The byte the `TRAN-RECORD` job-log line shows in each position of `TRAN-SOURCE`, `TRAN-DESC`,
/// `TRAN-MERCHANT-NAME`, `TRAN-MERCHANT-CITY`, `TRAN-MERCHANT-ZIP` and `FILLER`, whatever they
/// hold (Cardholder Data Redaction; CBTRN03C-012, CBTRN03C-039, X-TRANREPT-3).
const TEXT_MASK: u8 = b'*';

/// Integer digits of `TRAN-AMT` and `WS-PAGE-TOTAL PIC S9(09)V99`.
const AMOUNT_INT_DIGITS: u32 = 9;

/// Scale of `TRAN-AMT` and `WS-PAGE-TOTAL PIC S9(09)V99`.
const AMOUNT_SCALE: u32 = 2;

/// Bytes of a signed DISPLAY `PIC S9(09)V99` item: eleven digits, the sign overpunched.
const AMOUNT_DISPLAY_LEN: usize = 11;

/// `ALL '-'`, the fill of `TRANSACTION-HEADER-2`.
const DASH: u8 = b'-';

/// `ALL '.'`, the fill between a total's label and its amount.
const DOT: u8 = b'.';

/// `SPACES`, the padding of every shorter record.
const SPACE: u8 = b' ';

// The two defaults are evaluated at compile time, where the `None` arm would be a build error,
// so neither leaves a panic path in the program.

/// The default start date, `PARM-START-DATE` of `TRANREPT.jcl:43` (CBTRN03C-001).
const DEFAULT_START: NaiveDate = match NaiveDate::from_ymd_opt(2022, 1, 1) {
    Some(date) => date,
    None => panic!("2022-01-01 is a calendar date"),
};

/// The default end date, `PARM-END-DATE` of `TRANREPT.jcl:44` (CBTRN03C-001).
const DEFAULT_END: NaiveDate = match NaiveDate::from_ymd_opt(2022, 7, 6) {
    Some(date) => date,
    None => panic!("2022-07-06 is a calendar date"),
};

/// The text `bytes` hold, one character per byte.
fn text_of(bytes: &[u8]) -> String {
    bytes.iter().copied().map(char::from).collect()
}

/// Which date option names no valid date (CBTRN03C-002).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DateParmError {
    /// The start date: not 10 characters `YYYY-MM-DD` naming a calendar date of years 0 to 9999.
    Start,
    /// The end date, by the same rule.
    End,
}

impl fmt::Display for DateParmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Start => "the start date is not a valid YYYY-MM-DD calendar date",
            Self::End => "the end date is not a valid YYYY-MM-DD calendar date",
        })
    }
}

impl std::error::Error for DateParmError {}

/// The reporting window: `WS-START-DATE` and `WS-END-DATE` of `WS-DATEPARM-RECORD`
/// (`CBTRN03C.cbl:122-125`), which also stand for the SORT step's `PARM-START-DATE` and
/// `PARM-END-DATE`, so the program's window and the SORT's are one (CBTRN03C-001, X-TRANREPT-1).
///
/// The dates are typed (Typed Dates and Timestamps); their `YYYY-MM-DD` text exists only for the
/// DATEPARM record, the report header and the text comparisons of [`selects`]. A start date after
/// the end date is valid and selects nothing (CBTRN03C-002, CBTRN03C-037).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DateParm {
    start: NaiveDate,
    end: NaiveDate,
    start_text: [u8; DATE_TEXT_LEN],
    end_text: [u8; DATE_TEXT_LEN],
}

impl DateParm {
    /// The window of `tranrept` without options: `2022-01-01` to `2022-07-06`
    /// (`TRANREPT.jcl:43-44`; CBTRN03C-001).
    pub const DEFAULT: Self = Self {
        start: DEFAULT_START,
        end: DEFAULT_END,
        start_text: *b"2022-01-01",
        end_text: *b"2022-07-06",
    };

    /// The window from `start` to `end`, both inclusive.
    ///
    /// # Errors
    ///
    /// [`DateParmError::Start`] or [`DateParmError::End`] for a date whose year lies outside
    /// 0 to 9999, which the 10-character text cannot carry (PIC-015).
    pub fn new(start: NaiveDate, end: NaiveDate) -> Result<Self, DateParmError> {
        Ok(Self {
            start,
            end,
            start_text: date_text(start).ok_or(DateParmError::Start)?,
            end_text: date_text(end).ok_or(DateParmError::End)?,
        })
    }

    /// The window the option texts name: each exactly 10 characters `YYYY-MM-DD` naming a valid
    /// calendar date (PIC-015; CBTRN03C-001). The parse is total over any text.
    ///
    /// # Errors
    ///
    /// [`DateParmError::Start`] or [`DateParmError::End`] for text in another layout
    /// (`2022-1-1`, `01/01/2022`), of another length, blank or partly blank, or naming an
    /// impossible date (`2022-02-30`), the start date checked first (PIC-016; CBTRN03C-002).
    pub fn parse(start: &str, end: &str) -> Result<Self, DateParmError> {
        let start = parse_date(start).ok_or(DateParmError::Start)?;
        let end = parse_date(end).ok_or(DateParmError::End)?;
        Self::new(start, end)
    }

    /// The first date reported.
    #[must_use]
    pub const fn start(&self) -> NaiveDate {
        self.start
    }

    /// The last date reported.
    #[must_use]
    pub const fn end(&self) -> NaiveDate {
        self.end
    }

    /// The DATEPARM record in the CR00 deck's shape: the start date text, one space, the end
    /// date text and 59 spaces, 80 bytes. `READ … INTO` moves its first 21 bytes into
    /// `WS-DATEPARM-RECORD` (CBTRN03C-001).
    #[must_use]
    pub fn record(&self) -> [u8; DATEPARM_RECORD_LEN] {
        let mut record = [SPACE; DATEPARM_RECORD_LEN];
        let text = self
            .start_text
            .iter()
            .chain(iter::once(&SPACE))
            .chain(self.end_text.iter());
        for (target, &byte) in record.iter_mut().zip(text) {
            *target = byte;
        }
        record
    }

    /// The job-log line of the DATEPARM read: `Reporting from ` + `WS-START-DATE` + ` to ` +
    /// `WS-END-DATE` (CBTRN03C-008).
    #[must_use]
    pub fn reporting_line(&self) -> String {
        let start = text_of(&self.start_text);
        let end = text_of(&self.end_text);
        format!("Reporting from {start} to {end}")
    }

    /// Whether a `YYYY-MM-DD` processing-date text lies in the window, by the `CH` comparisons
    /// `GE` start `AND` `LE` end (CBTRN03C-004, CBTRN03C-011).
    fn contains(&self, date_text: &[u8]) -> bool {
        date_text >= self.start_text.as_slice() && date_text <= self.end_text.as_slice()
    }
}

/// [`DateParm::DEFAULT`].
impl Default for DateParm {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The `YYYY-MM-DD` text of `date`, or `None` for a year outside 0 to 9999 (PIC-015).
fn date_text(date: NaiveDate) -> Option<[u8; DATE_TEXT_LEN]> {
    let field = PicDate::new(Some(date)).ok()?;
    let mut text = Vec::with_capacity(DATE_TEXT_LEN);
    field.encode_into(&mut text);
    <[u8; DATE_TEXT_LEN]>::try_from(text.as_slice()).ok()
}

/// The calendar date option `text` names, or `None`: blank text names no date (PIC-016).
fn parse_date(text: &str) -> Option<NaiveDate> {
    PicDate::decode(text.as_bytes()).ok()?.value()
}

/// Whether the SORT step's `INCLUDE` selects `t`: `TRAN-PROC-DT`, the first 10 bytes of
/// `TRAN-PROC-TS` at offset 304, compared as `CH` text `GE` the start date `AND` `LE` the end
/// date, both inclusive (`TRANREPT.prc:40-46`; CBTRN03C-004). The program's window test applies
/// the same comparison to every record it reads (CBTRN03C-011).
///
/// The text is the field's codec A encoding, whose first 10 bytes are the `YYYY-MM-DD` date in
/// every layout (PIC-018 to PIC-020), so the job's `proc_ts::date` range selects the same rows. A
/// blank timestamp is 26 spaces, which compare below every digit, so it is never selected
/// (PIC-017).
#[must_use]
pub fn selects(t: &TransactionRecord, parm: &DateParm) -> bool {
    let mut text = Vec::with_capacity(PicTimestamp::WIDTH);
    t.proc_ts.encode_into(&mut text);
    text.get(..DATE_TEXT_LEN)
        .is_some_and(|date| parm.contains(date))
}

/// The order in which the report reads the selected rows (CBTRN03C-005).
///
/// `SORT FIELDS=(TRAN-CARD-NUM,A)` compares the 16 card digits as `ZD`, numeric order, which for
/// equal-length digit strings is byte order; the card number compares here bytewise
/// ([`CardNumber`]'s `Ord`), the job's `card_num COLLATE "C"`. `HALTED(X-TRANREPT-H1)`: the
/// sort has no `EQUALS`, so rows sharing a card number have no order the job fixes; `TRAN-ID`
/// ascending, the input order the KSDS unload supplies, breaks the tie as the proposed
/// resolution, not as a parity result. `Debug` shows the card number redacted.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SortKey {
    card_num: CardNumber,
    tran_id: PicX<16>,
}

/// The [`SortKey`] of `t`: its card number, then its transaction ID (CBTRN03C-005).
#[must_use]
pub fn sort_key(t: &TransactionRecord) -> SortKey {
    SortKey {
        card_num: t.card_num.clone(),
        tran_id: t.id.clone(),
    }
}

/// The job-log line `DISPLAY TRAN-RECORD` shows for each record read (CBTRN03C-012): the 350
/// bytes of the record's codec A encoding with its free-text fields masked and its card number
/// in the redacted form, so the line keeps its width and holds no card number, SSN or password
/// (CBTRN03C-039, X-TRANREPT-3, Cardholder Data Redaction).
///
/// `TRAN-SOURCE`, `TRAN-DESC`, `TRAN-MERCHANT-NAME`, `TRAN-MERCHANT-CITY`, `TRAN-MERCHANT-ZIP`
/// and `FILLER` are unvalidated text, so any of them can carry a protected value: each shows as
/// `*` in every position, whatever it holds. `TRAN-CARD-NUM` shows as twelve `*` and its last
/// four digits. `TRAN-ID`, the type and category codes, `TRAN-AMT`, `TRAN-MERCHANT-ID` and the
/// two timestamps show as encoded. Only the line is masked: `t`, its encoding and the report
/// records built from it keep every byte.
///
/// The COBOL shows it after the window test and before the lookups, so the job shows it for a
/// row [`selects`] accepts, before passing that row to [`Tranrept::process`]; a row outside the
/// window leaves the loop first and shows nothing (CBTRN03C-011).
#[must_use]
pub fn record_display(t: &TransactionRecord) -> Vec<u8> {
    let shown = TransactionRecord {
        source: PicX::from_array([TEXT_MASK; 10]),
        desc: PicX::from_array([TEXT_MASK; 100]),
        merchant_name: PicX::from_array([TEXT_MASK; 50]),
        merchant_city: PicX::from_array([TEXT_MASK; 50]),
        merchant_zip: PicX::from_array([TEXT_MASK; 10]),
        filler: PicX::from_array([TEXT_MASK; 20]),
        ..t.clone()
    };
    let mut line = shown.encode();
    let redacted = t.card_num.redacted_bytes();
    for (target, byte) in line.iter_mut().skip(CARD_NUM_OFFSET).zip(redacted) {
        *target = byte;
    }
    line
}

/// What the read loop does after one record (CBTRN03C-011).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flow {
    /// The record was reported; pass the next one.
    Continue,
    /// The record failed the window test and `NEXT SENTENCE` left the loop: read no further
    /// record. The report keeps only what was written before, with no total.
    Stop,
}

/// A failure that ends the run through `9999-ABEND-PROGRAM` (CBTRN03C-035). None carries a card
/// number: the cross-reference failure carries only its redacted form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ReportError {
    /// The card number is not in `card_xref`: `INVALID KEY` of `1500-A-LOOKUP-XREF`
    /// (CBTRN03C-014).
    XrefNotFound {
        /// The card number's redacted form, twelve `*` and its last four digits (PIC-024).
        redacted_card: [u8; 16],
    },
    /// The type code is not in `transaction_types`: `INVALID KEY` of `1500-B-LOOKUP-TRANTYPE`
    /// (CBTRN03C-016).
    TypeNotFound {
        /// `TRAN-TYPE-CD`.
        type_cd: [u8; 2],
    },
    /// The (type, category) pair is not in `transaction_categories`: `INVALID KEY` of
    /// `1500-C-LOOKUP-TRANCATG` (CBTRN03C-018).
    CategoryNotFound {
        /// `TRAN-TYPE-CD`.
        type_cd: [u8; 2],
        /// `TRAN-CAT-CD`.
        cat_cd: PicU<4>,
    },
    /// `HALTED(X-TRANREPT-H4)`: `TRAN-CARD-NUM` is not 16 digits, so neither its sort order nor,
    /// for a blank first card, its account ID has a single COBOL result. The proposed replacement
    /// ends the run through the abend path, with no line of its own before `ABENDING PROGRAM`.
    InvalidCardNumber,
}

impl ReportError {
    /// The job-log lines the COBOL shows before `ABENDING PROGRAM`: the `INVALID` line and
    /// `FILE STATUS IS: NNNN0023` for a lookup's `INVALID KEY` (CBTRN03C-014, CBTRN03C-016,
    /// CBTRN03C-018, CBTRN03C-036), `None` for [`ReportError::InvalidCardNumber`].
    #[must_use]
    pub fn job_log_lines(&self) -> Option<[String; 2]> {
        match self {
            Self::InvalidCardNumber => None,
            Self::XrefNotFound { .. }
            | Self::TypeNotFound { .. }
            | Self::CategoryNotFound { .. } => {
                Some([self.to_string(), NOT_FOUND_STATUS_LINE.to_owned()])
            }
        }
    }
}

/// The `INVALID` job-log line of a lookup failure, the card number redacted
/// (`INVALID CARD NUMBER : ************5740`, `INVALID TRANSACTION TYPE : 09`,
/// `INVALID TRAN CATG KEY : 010009`), or a description of the invalid card number.
impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::XrefNotFound { redacted_card } => {
                write!(f, "INVALID CARD NUMBER : {}", text_of(redacted_card))
            }
            Self::TypeNotFound { type_cd } => {
                write!(f, "INVALID TRANSACTION TYPE : {}", text_of(type_cd))
            }
            Self::CategoryNotFound { type_cd, cat_cd } => {
                let mut key = type_cd.to_vec();
                cat_cd.encode_into(&mut key);
                write!(f, "INVALID TRAN CATG KEY : {}", text_of(&key))
            }
            Self::InvalidCardNumber => f.write_str("TRAN-CARD-NUM is not 16 digits"),
        }
    }
}

impl std::error::Error for ReportError {}

/// Where the run stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunState {
    /// Records are being read.
    Running,
    /// A record failed the window test and the loop was left (CBTRN03C-011).
    Stopped,
    /// A lookup failed and the program abended (CBTRN03C-035).
    Abended(ReportError),
}

/// One run of CBTRN03C over the selected rows: its working storage, `WS-REPORT-VARS`
/// (`CBTRN03C.cbl:127-137`), the three lookup files and the report records written so far.
///
/// The lookups hold the rows the job passes to [`Tranrept::new`]: `card_xref` by card number,
/// and the VSAM data version of the types and categories by their keys (E23). No conversion from
/// the Db2 data version exists, so a maintenance change never reaches a report description
/// (CBTRN03C-038).
#[derive(Debug)]
pub struct Tranrept {
    /// `WS-DATEPARM-RECORD`, moved into the name header on the first detail (CBTRN03C-019).
    parm: DateParm,
    /// `card_xref`: `XREF-CARD-NUM` → `XREF-ACCT-ID` (CBTRN03C-013).
    xrefs: BTreeMap<CardNumber, PicU<11>>,
    /// `transaction_types`: `TRAN-TYPE` → `TRAN-TYPE-DESC` (CBTRN03C-015).
    types: BTreeMap<PicX<2>, PicX<50>>,
    /// `transaction_categories`: `TRAN-CAT-KEY` → `TRAN-CAT-TYPE-DESC` (CBTRN03C-017).
    categories: BTreeMap<(PicX<2>, PicU<4>), PicX<50>>,
    state: RunState,
    /// `WS-FIRST-TIME PIC X VALUE 'Y'`: no detail written yet.
    first_time: bool,
    /// `WS-CURR-CARD-NUM` with the `XREF-ACCT-ID` its lookup moved; `None` while it holds its
    /// initial spaces, which no 16-digit card number equals.
    block: Option<(CardNumber, PicU<11>)>,
    /// `WS-LINE-COUNTER PIC 9(09) COMP-3 VALUE 0`.
    line_counter: u64,
    /// `WS-PAGE-TOTAL PIC S9(09)V99 VALUE 0`.
    page_total: Money<9>,
    /// `WS-ACCOUNT-TOTAL PIC S9(09)V99 VALUE 0`.
    account_total: Money<9>,
    /// `WS-GRAND-TOTAL PIC S9(09)V99 VALUE 0`.
    grand_total: Money<9>,
    /// `TRAN-AMT` of the last record read, which `TRAN-RECORD` still holds at end of file
    /// (CBTRN03C-010, CBTRN03C-030); `None` before the first read.
    last_amt: Option<Money<9>>,
    /// The report records written, in order.
    records: Vec<ReportRecord>,
}

impl Tranrept {
    /// A run over the window `parm` with the given lookup rows. A key that appears twice keeps
    /// its last row; the tables' primary keys rule that out.
    #[must_use]
    pub fn new(
        parm: DateParm,
        xrefs: impl IntoIterator<Item = CardXrefRecord>,
        types: impl IntoIterator<Item = TransactionTypeRecord>,
        categories: impl IntoIterator<Item = TransactionCategoryRecord>,
    ) -> Self {
        Self {
            parm,
            xrefs: xrefs
                .into_iter()
                .map(|xref| (xref.card_num, xref.acct_id))
                .collect(),
            types: types
                .into_iter()
                .map(|tran_type| (tran_type.tran_type, tran_type.desc))
                .collect(),
            categories: categories
                .into_iter()
                .map(|category| ((category.type_cd, category.cat_cd), category.type_desc))
                .collect(),
            state: RunState::Running,
            first_time: true,
            block: None,
            line_counter: 0,
            page_total: Money::ZERO,
            account_total: Money::ZERO,
            grand_total: Money::ZERO,
            last_amt: None,
            records: Vec::new(),
        }
    }

    /// One pass of the read loop (`CBTRN03C.cbl:170-196`) for the next selected row `t`, in
    /// [`sort_key`] order.
    ///
    /// A record outside the window leaves the loop (CBTRN03C-011): [`Flow::Stop`], and every
    /// later call returns it too without reading its record. Otherwise the card-block change
    /// writes the previous card's account total, unless no detail has been written yet, and looks
    /// up the new card (CBTRN03C-013); the type and category are looked up (CBTRN03C-015,
    /// CBTRN03C-017); and `1100-WRITE-TRANSACTION-REPORT` writes the first header group, a page
    /// break when the line counter is a multiple of 20, then the detail (CBTRN03C-019 to
    /// CBTRN03C-024).
    ///
    /// # Errors
    ///
    /// The abend of the run (CBTRN03C-035): [`ReportError::XrefNotFound`],
    /// [`ReportError::TypeNotFound`] or [`ReportError::CategoryNotFound`] for a lookup's
    /// `INVALID KEY`, and [`ReportError::InvalidCardNumber`] for a card number that is not 16
    /// digits. After an error the run is over: every later call and [`Tranrept::finish`] return
    /// the same error.
    pub fn process(&mut self, t: &TransactionRecord) -> Result<Flow, ReportError> {
        match self.state {
            RunState::Abended(error) => return Err(error),
            RunState::Stopped => return Ok(Flow::Stop),
            RunState::Running => {}
        }
        if !selects(t, &self.parm) {
            self.state = RunState::Stopped;
            return Ok(Flow::Stop);
        }
        self.last_amt = Some(t.amt);
        match self.report(t) {
            Ok(()) => Ok(Flow::Continue),
            Err(error) => {
                self.state = RunState::Abended(error);
                Err(error)
            }
        }
    }

    /// The two job-log lines of the end-of-file branch (CBTRN03C-029): `TRAN-AMT ` + the last
    /// record's amount, then `WS-PAGE-TOTAL` + the page total before the repeated amount is
    /// added, each as its 11 digits with the sign overpunched on the last (PIC-005,
    /// X-TRANREPT-5). `None` when the branch is not taken: no record was reported, the loop was
    /// left, or the run abended.
    #[must_use]
    pub fn end_of_file_display(&self) -> Option<[String; 2]> {
        match (self.state, self.last_amt) {
            (RunState::Running, Some(amt)) => Some([
                format!("TRAN-AMT {}", amount_display(amt)),
                format!("WS-PAGE-TOTAL{}", amount_display(self.page_total)),
            ]),
            _ => None,
        }
    }

    /// The report records written since the last call, or since the run began, in order
    /// (CBTRN03C-032), so that the job writes them as the run goes rather than holding the
    /// whole report. Taking them changes nothing else: the records [`Tranrept::finish`] returns
    /// are then only those not yet taken, and every taken chunk followed by them is the report
    /// a run that never takes any returns from `finish`.
    #[must_use]
    pub fn take_records(&mut self) -> Vec<ReportRecord> {
        std::mem::take(&mut self.records)
    }

    /// Ends the run and returns every report record not yet taken by
    /// [`Tranrept::take_records`], in order (CBTRN03C-032).
    ///
    /// After the last record the end-of-file branch adds the last amount to the page and account
    /// totals a second time, because `TRAN-RECORD` still holds it, then writes the page total,
    /// its dashes and the grand total; the doubled account total of the last card is discarded
    /// unwritten (CBTRN03C-030, CBTRN03C-031, X-TRANREPT-7). A run left by the window test ends
    /// with the records written before it and no total (CBTRN03C-011).
    ///
    /// `HALTED(X-TRANREPT-H2)`: with no record read, the COBOL runs its window test over a
    /// `TRAN-RECORD` no read has set, which has no single result (CBTRN03C-037). The replacement
    /// returns no record, the result spaces or low-values would give, which is no parity result.
    ///
    /// # Errors
    ///
    /// The [`ReportError`] that abended the run, if [`Tranrept::process`] returned one.
    pub fn finish(mut self) -> Result<Vec<ReportRecord>, ReportError> {
        match self.state {
            RunState::Abended(error) => return Err(error),
            RunState::Stopped => return Ok(self.records),
            RunState::Running => {}
        }
        if let Some(amt) = self.last_amt {
            self.add_amount(amt);
            self.write_page_totals();
            self.write_grand_totals();
        }
        Ok(self.records)
    }

    /// The lookups and the report paragraph of one record inside the window.
    fn report(&mut self, t: &TransactionRecord) -> Result<(), ReportError> {
        if !is_sixteen_digits(&t.card_num) {
            return Err(ReportError::InvalidCardNumber);
        }
        let acct_id = self.block_account(&t.card_num)?;
        let type_desc = self
            .types
            .get(&t.type_cd)
            .cloned()
            .ok_or(ReportError::TypeNotFound {
                type_cd: *t.type_cd.as_bytes(),
            })?;
        let cat_desc = self
            .categories
            .get(&(t.type_cd.clone(), t.cat_cd))
            .cloned()
            .ok_or(ReportError::CategoryNotFound {
                type_cd: *t.type_cd.as_bytes(),
                cat_cd: t.cat_cd,
            })?;
        self.write_transaction_report(t, acct_id, &type_desc, &cat_desc);
        Ok(())
    }

    /// The account ID of `card`'s block (`CBTRN03C.cbl:181-188`; CBTRN03C-013). A card that
    /// differs from `WS-CURR-CARD-NUM` starts a block: the previous card's account total is
    /// written when a detail has been, then the card becomes current and is looked up.
    fn block_account(&mut self, card: &CardNumber) -> Result<PicU<11>, ReportError> {
        if let Some((current, acct_id)) = &self.block {
            if current == card {
                return Ok(*acct_id);
            }
        }
        if !self.first_time {
            self.write_account_totals();
        }
        let acct_id = self
            .xrefs
            .get(card)
            .copied()
            .ok_or(ReportError::XrefNotFound {
                redacted_card: card.redacted_bytes(),
            })?;
        self.block = Some((card.clone(), acct_id));
        Ok(acct_id)
    }

    /// `1100-WRITE-TRANSACTION-REPORT` (`CBTRN03C.cbl:274-290`; CBTRN03C-019, CBTRN03C-021,
    /// CBTRN03C-022).
    fn write_transaction_report(
        &mut self,
        t: &TransactionRecord,
        acct_id: PicU<11>,
        type_desc: &PicX<50>,
        cat_desc: &PicX<50>,
    ) {
        if self.first_time {
            self.first_time = false;
            self.write_headers();
        }
        if self.line_counter % PAGE_SIZE == 0 {
            self.write_page_totals();
            self.write_headers();
        }
        self.add_amount(t.amt);
        self.write_counted(detail(t, acct_id, type_desc, cat_desc));
    }

    /// `ADD TRAN-AMT TO WS-PAGE-TOTAL WS-ACCOUNT-TOTAL`, each sum stored by the store rule
    /// (CBTRN03C-022, CBTRN03C-028).
    fn add_amount(&mut self, amt: Money<9>) {
        self.page_total = (self.page_total + amt).store::<9>();
        self.account_total = (self.account_total + amt).store::<9>();
    }

    /// `1110-WRITE-PAGE-TOTALS` (`CBTRN03C.cbl:293-304`; CBTRN03C-025).
    fn write_page_totals(&mut self) {
        self.write(page_totals(self.page_total));
        self.grand_total = (self.grand_total + self.page_total).store::<9>();
        self.page_total = Money::ZERO;
        self.count_line();
        self.write_counted(header_2());
    }

    /// `1120-WRITE-ACCOUNT-TOTALS` (`CBTRN03C.cbl:306-316`; CBTRN03C-027).
    fn write_account_totals(&mut self) {
        self.write(account_totals(self.account_total));
        self.account_total = Money::ZERO;
        self.count_line();
        self.write_counted(header_2());
    }

    /// `1110-WRITE-GRAND-TOTALS`, the one record that adds nothing to the line counter
    /// (`CBTRN03C.cbl:318-322`; CBTRN03C-031).
    fn write_grand_totals(&mut self) {
        self.write(grand_totals(self.grand_total));
    }

    /// `1120-WRITE-HEADERS`, four records each counted (`CBTRN03C.cbl:324-341`; CBTRN03C-020).
    fn write_headers(&mut self) {
        self.write_counted(name_header(&self.parm));
        self.write_counted(blank_line());
        self.write_counted(header_1());
        self.write_counted(header_2());
    }

    /// `1111-WRITE-REPORT-REC` of `record`.
    fn write(&mut self, record: ReportRecord) {
        self.records.push(record);
    }

    /// `1111-WRITE-REPORT-REC` of `record`, then `ADD 1 TO WS-LINE-COUNTER`.
    fn write_counted(&mut self, record: ReportRecord) {
        self.write(record);
        self.count_line();
    }

    /// `ADD 1 TO WS-LINE-COUNTER`, with no `ON SIZE ERROR`: the sum keeps its low nine digits,
    /// so the counter after 999 999 999 is 0 (the store rule of `modern/audit/arithmetic.md`).
    fn count_line(&mut self) {
        // TRUNCATION: `ADD 1` into `WS-LINE-COUNTER PIC 9(09) COMP-3` has no `ON SIZE ERROR`, so
        // the COBOL store keeps the low nine digits and 999 999 999 + 1 stores 0
        // (`CBTRN03C.cbl:129,299,302,311,314,327-339,373`; CBTRN03C-021). The counter is always
        // below 10^9, far below `u64::MAX`, so `saturating_add` never saturates and the modulo
        // is exactly that store.
        self.line_counter = self.line_counter.saturating_add(1) % LINE_COUNTER_MODULUS;
    }
}

/// Whether every byte of `card` is an ASCII digit, the card numbers every in-scope writer stores
/// (X-TRANREPT-H4). The raw bytes are only tested here, never copied.
fn is_sixteen_digits(card: &CardNumber) -> bool {
    card.expose().as_bytes().iter().all(u8::is_ascii_digit)
}

/// `amt` as a signed DISPLAY `PIC S9(09)V99` item shows: 11 digits, no point, the sign
/// overpunched on the last digit (PIC-005; CBTRN03C-029).
fn amount_display(amt: Money<9>) -> String {
    let mut item = RecordWriter::with_capacity(AMOUNT_DISPLAY_LEN);
    item.zoned(amt.as_decimal(), AMOUNT_INT_DIGITS, AMOUNT_SCALE);
    text_of(&item.finish())
}

/// A report record built left to right: each group item `MOVE`d to `FD-REPTFILE-REC` is padded
/// with spaces to 133 bytes (CBTRN03C-032).
struct RecordBuilder {
    bytes: ReportRecord,
    at: usize,
}

impl RecordBuilder {
    /// 133 spaces, nothing placed yet.
    const fn new() -> Self {
        Self {
            bytes: [SPACE; REPORT_RECORD_LEN],
            at: 0,
        }
    }

    /// The next `width` bytes hold `src` as an alphanumeric `MOVE` leaves it: left-justified,
    /// padded with spaces and cut on the right when `src` is longer than `width`.
    #[must_use]
    fn field(mut self, src: &[u8], width: usize) -> Self {
        for (target, &byte) in self.bytes.iter_mut().skip(self.at).take(width).zip(src) {
            *target = byte;
        }
        self.at = self.at.saturating_add(width);
        self
    }

    /// The next `width` bytes hold `byte`, a `FILLER` of `VALUE ALL` or `VALUE SPACES`.
    #[must_use]
    fn fill(mut self, byte: u8, width: usize) -> Self {
        for target in self.bytes.iter_mut().skip(self.at).take(width) {
            *target = byte;
        }
        self.at = self.at.saturating_add(width);
        self
    }

    /// The finished record.
    const fn finish(self) -> ReportRecord {
        self.bytes
    }
}

/// `REPORT-NAME-HEADER`, 115 bytes (`CVTRA07Y.cpy:4-13`; CBTRN03C-020): `REPT-SHORT-NAME X(38)`,
/// `REPT-LONG-NAME X(41)`, `REPT-DATE-HEADER X(12)`, the start date, ` to `, the end date.
fn name_header(parm: &DateParm) -> ReportRecord {
    RecordBuilder::new()
        .field(b"DALYREPT", 38)
        .field(b"Daily Transaction Report", 41)
        .field(b"Date Range: ", 12)
        .field(&parm.start_text, DATE_TEXT_LEN)
        .field(b" to ", 4)
        .field(&parm.end_text, DATE_TEXT_LEN)
        .finish()
}

/// `WS-BLANK-LINE PIC X(133) VALUE SPACES` (CBTRN03C-020).
const fn blank_line() -> ReportRecord {
    [SPACE; REPORT_RECORD_LEN]
}

/// `TRANSACTION-HEADER-1`, 114 bytes of column titles, `Amount` ending in column 112 above the
/// amount field (`CVTRA07Y.cpy:33-46`; CBTRN03C-020).
fn header_1() -> ReportRecord {
    RecordBuilder::new()
        .field(b"Transaction ID", 17)
        .field(b"Account ID", 12)
        .field(b"Transaction Type", 19)
        .field(b"Tran Category", 35)
        .field(b"Tran Source", 14)
        .fill(SPACE, 1)
        .field(b"        Amount", 16)
        .finish()
}

/// `TRANSACTION-HEADER-2 PIC X(133) VALUE ALL '-'`, the dash record after the header titles and
/// after every total but the grand total (CBTRN03C-020, CBTRN03C-025, CBTRN03C-027).
const fn header_2() -> ReportRecord {
    [DASH; REPORT_RECORD_LEN]
}

/// `TRANSACTION-DETAIL-REPORT`, 114 bytes (`CVTRA07Y.cpy:15-31`; CBTRN03C-023). `INITIALIZE`
/// resets only the named fields, which the moves then fill, so every `FILLER` keeps its `VALUE`.
/// The descriptions are cut to 15 and 29 bytes; the record holds no card number.
fn detail(
    t: &TransactionRecord,
    acct_id: PicU<11>,
    type_desc: &PicX<50>,
    cat_desc: &PicX<50>,
) -> ReportRecord {
    let mut account = Vec::with_capacity(11);
    acct_id.encode_into(&mut account);
    let mut category = Vec::with_capacity(4);
    t.cat_cd.encode_into(&mut category);
    // `TRAN-REPORT-AMT PIC -ZZZ,ZZZ,ZZZ.ZZ` (CBTRN03C-024).
    let amount = EditedPicture::MinusZCommas9DotZZ.format(t.amt.as_decimal());
    RecordBuilder::new()
        .field(t.id.as_bytes(), 16)
        .fill(SPACE, 1)
        .field(&account, 11)
        .fill(SPACE, 1)
        .field(t.type_cd.as_bytes(), 2)
        .fill(DASH, 1)
        .field(type_desc.as_bytes(), 15)
        .fill(SPACE, 1)
        .field(&category, 4)
        .fill(DASH, 1)
        .field(cat_desc.as_bytes(), 29)
        .fill(SPACE, 1)
        .field(t.source.as_bytes(), 10)
        .fill(SPACE, 4)
        .field(amount.as_bytes(), 15)
        .fill(SPACE, 2)
        .finish()
}

/// A 112-byte total record: `label` in `label_width` bytes, `dots` periods and the total edited
/// `+ZZZ,ZZZ,ZZZ.ZZ` (CBTRN03C-026).
fn total_record(label: &[u8], label_width: usize, dots: usize, total: Money<9>) -> ReportRecord {
    let amount = EditedPicture::PlusZCommas9DotZZ.format(total.as_decimal());
    RecordBuilder::new()
        .field(label, label_width)
        .fill(DOT, dots)
        .field(amount.as_bytes(), 15)
        .finish()
}

/// `REPORT-PAGE-TOTALS`: `Page Total` in `X(11)`, 86 `.`, `REPT-PAGE-TOTAL`
/// (`CVTRA07Y.cpy:50-54`; CBTRN03C-025).
fn page_totals(total: Money<9>) -> ReportRecord {
    total_record(b"Page Total", 11, 86, total)
}

/// `REPORT-ACCOUNT-TOTALS`: `Account Total` in `X(13)`, 84 `.`, `REPT-ACCOUNT-TOTAL`
/// (`CVTRA07Y.cpy:56-60`; CBTRN03C-027).
fn account_totals(total: Money<9>) -> ReportRecord {
    total_record(b"Account Total", 13, 84, total)
}

/// `REPORT-GRAND-TOTALS`: `Grand Total` in `X(11)`, 86 `.`, `REPT-GRAND-TOTAL`
/// (`CVTRA07Y.cpy:62-66`; CBTRN03C-031).
fn grand_totals(total: Money<9>) -> ReportRecord {
    total_record(b"Grand Total", 11, 86, total)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{FixedRecord, PicTimestamp, PicU, PicX};

    use super::{
        DateParm, DateParmError, Flow, REPORT_RECORD_LEN, ReportError, ReportRecord, Tranrept,
        account_totals, detail, page_totals, record_display, selects, sort_key,
    };
    use crate::money::{Decimal, Money};
    use crate::records::{
        CardXrefRecord, TransactionCategoryRecord, TransactionRecord, TransactionTypeRecord,
    };
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Three synthetic card numbers in ascending order, for the card blocks of FIXRPT-001 to
    /// FIXRPT-003 (accounts 50, 27 and 2).
    const CARD_1: [u8; 16] = *b"4000000000000001";
    const CARD_2: [u8; 16] = *b"4000000000000002";
    const CARD_3: [u8; 16] = *b"4000000000000003";

    /// A card number `card_xref` does not hold.
    const UNKNOWN_CARD: [u8; 16] = *b"4000000000009999";

    /// The processing timestamp `posttran` stamps in the POSTED scenario, layout `D` (PIC-019).
    const POSTED_TS: &[u8; 26] = b"2022-07-01-10.15.42.370000";

    /// Line 1 of the default name header (FIXRPT-902).
    const NAME_HEADER: &str = "DALYREPT                              Daily Transaction Report                 Date Range: 2022-01-01 to 2022-07-06";

    /// `TRANSACTION-HEADER-1` (FIXRPT-902).
    const HEADER_1: &str = "Transaction ID   Account ID  Transaction Type   Tran Category                      Tran Source            Amount";

    /// The kind of a transaction: its type, category and source.
    #[derive(Clone, Copy)]
    enum Kind {
        /// Type `01`, category `0001`, source `POS TERM`.
        Purchase,
        /// Type `03`, category `0001`, source `OPERATOR`.
        Credit,
    }

    /// The 16-byte `TRAN-ID` standing for `{T:n}`.
    fn id(n: u64) -> String {
        format!("{n:016}")
    }

    /// The amount of `cents` hundredths.
    fn money(cents: i64) -> Result<Money<9>, Box<dyn StdError>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    /// A transaction row with processing timestamp `proc_ts`.
    fn tran_at(
        n: u64,
        card: [u8; 16],
        kind: Kind,
        cents: i64,
        proc_ts: &[u8; 26],
    ) -> Result<TransactionRecord, Box<dyn StdError>> {
        let (type_cd, source) = match kind {
            Kind::Purchase => (*b"01", "POS TERM"),
            Kind::Credit => (*b"03", "OPERATOR"),
        };
        Ok(TransactionRecord {
            id: PicX::from_text(&id(n))?,
            type_cd: PicX::from_array(type_cd),
            cat_cd: PicU::new(1)?,
            source: PicX::from_text(source)?,
            desc: PicX::from_text("Purchase at Abshire-Lowe")?,
            amt: money(cents)?,
            merchant_id: PicU::new(800_000_000)?,
            merchant_name: PicX::from_text("Abshire-Lowe")?,
            merchant_city: PicX::from_text("North Enoshaven")?,
            merchant_zip: PicX::from_text("72112")?,
            card_num: CardNumber::new(PicX::from_array(card)),
            orig_ts: PicTimestamp::decode(b"2022-06-10 19:27:53.000000")?,
            proc_ts: PicTimestamp::decode(proc_ts)?,
            filler: PicX::spaces(),
        })
    }

    /// A transaction row posted in the POSTED scenario.
    fn tran(
        n: u64,
        card: [u8; 16],
        kind: Kind,
        cents: i64,
    ) -> Result<TransactionRecord, Box<dyn StdError>> {
        tran_at(n, card, kind, cents, POSTED_TS)
    }

    /// The cross-reference rows of the three cards.
    fn xrefs() -> Result<Vec<CardXrefRecord>, Box<dyn StdError>> {
        [(CARD_1, 50), (CARD_2, 27), (CARD_3, 2)]
            .into_iter()
            .map(|(card, acct_id)| {
                Ok(CardXrefRecord {
                    card_num: CardNumber::new(PicX::from_array(card)),
                    cust_id: PicU::new(acct_id)?,
                    acct_id: PicU::new(acct_id)?,
                    filler: PicX::spaces(),
                })
            })
            .collect()
    }

    /// Types `01` and `03` of the VSAM data version (`trantype.txt:1,3`).
    fn vsam_types() -> Result<Vec<TransactionTypeRecord>, Box<dyn StdError>> {
        [(*b"01", "Purchase"), (*b"03", "Credit")]
            .into_iter()
            .map(|(code, desc)| {
                Ok(TransactionTypeRecord {
                    tran_type: PicX::from_array(code),
                    desc: PicX::from_text(desc)?,
                    filler: PicX::from_array(*b"00000000"),
                })
            })
            .collect()
    }

    /// Categories `01`/`0001` and `03`/`0001` of the VSAM data version (`trancatg.txt:1,9`).
    fn vsam_categories() -> Result<Vec<TransactionCategoryRecord>, Box<dyn StdError>> {
        [
            (*b"01", "Regular Sales Draft"),
            (*b"03", "Credit to Account"),
        ]
        .into_iter()
        .map(|(code, desc)| {
            Ok(TransactionCategoryRecord {
                type_cd: PicX::from_array(code),
                cat_cd: PicU::new(1)?,
                type_desc: PicX::from_text(desc)?,
                filler: PicX::from_array(*b"0000"),
            })
        })
        .collect()
    }

    /// A run over the default window with the lookup rows above.
    fn run() -> Result<Tranrept, Box<dyn StdError>> {
        Ok(Tranrept::new(
            DateParm::DEFAULT,
            xrefs()?,
            vsam_types()?,
            vsam_categories()?,
        ))
    }

    /// `text` padded with spaces to a report record.
    fn padded(text: &str) -> Result<ReportRecord, Box<dyn StdError>> {
        let mut record = [b' '; REPORT_RECORD_LEN];
        if text.len() > REPORT_RECORD_LEN {
            return Err(format!("{} bytes exceed the record", text.len()).into());
        }
        for (target, &byte) in record.iter_mut().zip(text.as_bytes()) {
            *target = byte;
        }
        Ok(record)
    }

    /// The expected detail of `{T:n}` in account `acct` with the edited amount `amount`.
    fn detail_text(n: u64, acct: &str, kind: Kind, amount: &str) -> String {
        let body = match kind {
            Kind::Purchase => "01-Purchase        0001-Regular Sales Draft           POS TERM  ",
            Kind::Credit => "03-Credit          0001-Credit to Account             OPERATOR  ",
        };
        format!("{} {acct} {body}    {amount}", id(n))
    }

    /// The four records of a header group of the default window (FIXRPT-902).
    fn header_group() -> Result<Vec<ReportRecord>, Box<dyn StdError>> {
        Ok(vec![
            padded(NAME_HEADER)?,
            [b' '; REPORT_RECORD_LEN],
            padded(HEADER_1)?,
            [b'-'; REPORT_RECORD_LEN],
        ])
    }

    /// A total record: `label`, dots to column 97, then the edited `amount`.
    fn total_text(label: &str, amount: &str) -> String {
        format!("{label:.<97}{amount}")
    }

    /// The `n`th record, 1-based.
    fn nth(records: &[ReportRecord], n: usize) -> Result<&ReportRecord, String> {
        n.checked_sub(1)
            .and_then(|at| records.get(at))
            .ok_or_else(|| format!("no record {n}"))
    }

    // audit: CBTRN03C-001
    // audit: CBTRN03C-008
    // audit: FIXRPT-901
    #[test]
    fn the_default_window_is_the_jcl_constants_in_the_cr00_record_shape() -> TestResult {
        let start = NaiveDate::from_ymd_opt(2022, 1, 1).ok_or("2022-01-01")?;
        let end = NaiveDate::from_ymd_opt(2022, 7, 6).ok_or("2022-07-06")?;
        let parm = DateParm::default();
        assert_eq!(parm, DateParm::DEFAULT);
        assert_eq!(parm, DateParm::new(start, end)?);
        assert_eq!((parm.start(), parm.end()), (start, end));
        let mut expected = b"2022-01-01 2022-07-06".to_vec();
        expected.resize(80, b' ');
        assert_eq!(parm.record().as_slice(), expected.as_slice());
        assert_eq!(
            parm.reporting_line(),
            "Reporting from 2022-01-01 to 2022-07-06"
        );
        Ok(())
    }

    // audit: CBTRN03C-002
    #[test]
    fn option_text_parses_only_valid_ten_character_dates() -> TestResult {
        assert_eq!(
            DateParm::parse("2022-01-01", "2022-07-06")?,
            DateParm::DEFAULT
        );
        let long = "2022-01-01".repeat(200);
        let invalid = [
            "2022-1-1",
            "01/01/2022",
            "",
            "          ",
            "2022-01-0 ",
            "2022-02-30",
            "2022-13-01",
            "2022-01-01 ",
            "\u{ff12}022-01-1",
            long.as_str(),
        ];
        for text in invalid {
            assert_eq!(
                DateParm::parse(text, "2022-07-06"),
                Err(DateParmError::Start)
            );
            assert_eq!(DateParm::parse("2022-01-01", text), Err(DateParmError::End));
        }
        assert_eq!(DateParm::parse("bad", "bad"), Err(DateParmError::Start));
        // A start after the end is a valid window that selects nothing.
        let reversed = DateParm::parse("2022-07-06", "2022-01-01")?;
        assert!(!selects(&tran(1, CARD_1, Kind::Purchase, 100)?, &reversed));
        let beyond = NaiveDate::from_ymd_opt(10_000, 1, 1).ok_or("year 10000")?;
        assert_eq!(
            DateParm::new(beyond, DateParm::DEFAULT.end()),
            Err(DateParmError::Start)
        );
        assert_eq!(
            DateParm::new(DateParm::DEFAULT.start(), beyond),
            Err(DateParmError::End)
        );
        Ok(())
    }

    // audit: CBTRN03C-004
    #[test]
    fn selection_is_the_inclusive_processing_date_window_and_never_a_blank_timestamp() -> TestResult
    {
        let parm = DateParm::DEFAULT;
        let cases: [(&[u8; 26], bool); 8] = [
            (b"2022-01-01 00:00:00.000000", true),
            (b"2022-07-06-23.59.59.990000", true),
            (b"2022-07-06                ", true),
            (POSTED_TS, true),
            (b"2021-12-31-23.59.59.990000", false),
            (b"2022-07-07 00:00:00.000000", false),
            (b"2022-07-07                ", false),
            (&[b' '; 26], false),
        ];
        for (proc_ts, selected) in cases {
            let row = tran_at(1, CARD_1, Kind::Purchase, 100, proc_ts)?;
            assert_eq!(selects(&row, &parm), selected, "{proc_ts:?}");
        }
        Ok(())
    }

    // audit: CBTRN03C-005
    #[test]
    fn rows_sort_by_card_number_then_transaction_id() -> TestResult {
        let mut rows = vec![
            tran(9, CARD_2, Kind::Purchase, 100)?,
            tran(3, CARD_1, Kind::Purchase, 100)?,
            tran(1, CARD_2, Kind::Purchase, 100)?,
            tran(7, CARD_1, Kind::Purchase, 100)?,
        ];
        rows.sort_by_key(sort_key);
        let order: Vec<String> = rows.iter().map(|row| row.id.to_text()).collect();
        assert_eq!(order, [id(3), id(7), id(1), id(9)]);
        let shown = format!("{:?}", sort_key(&tran(1, CARD_1, Kind::Purchase, 100)?));
        assert!(!shown.contains("4000000000000001"), "{shown}");
        assert!(shown.contains("************0001"), "{shown}");
        Ok(())
    }

    // audit: CBTRN03C-012
    #[test]
    fn the_record_display_masks_the_free_text_and_redacts_the_card_number() -> TestResult {
        let row = tran(21, CARD_1, Kind::Purchase, 18_388)?;
        let encoded = row.encode();
        let shown = record_display(&row);
        assert_eq!(shown.len(), TransactionRecord::WIDTH);
        // TRAN-SOURCE, TRAN-DESC, TRAN-MERCHANT-NAME, TRAN-MERCHANT-CITY, TRAN-MERCHANT-ZIP and
        // FILLER, every byte `*` whatever the row holds, its spaces included.
        let masked = [22..32, 32..132, 152..202, 202..252, 252..262, 330..350];
        for range in masked {
            let bytes = shown.get(range.clone()).ok_or("a masked field")?;
            assert!(bytes.iter().all(|&byte| byte == b'*'), "{range:?}");
        }
        // TRAN-ID, TRAN-TYPE-CD and TRAN-CAT-CD; TRAN-AMT and TRAN-MERCHANT-ID; the timestamps.
        for range in [0..22, 132..152, 278..330] {
            assert_eq!(shown.get(range.clone()), encoded.get(range));
        }
        assert_eq!(shown.get(262..278), Some(b"************0001".as_slice()));
        Ok(())
    }

    /// The rows of card blocks 1 and 2 of POSTED (FIXRPT-001, FIXRPT-002) and the first row of
    /// block 3 (FIXRPT-003), in `tran_id` order within each block.
    fn three_blocks() -> Result<Vec<TransactionRecord>, Box<dyn StdError>> {
        let rows = [
            (21, CARD_1, Kind::Purchase, 18_388),
            (102, CARD_1, Kind::Purchase, 1_400),
            (142, CARD_1, Kind::Purchase, 96_744),
            (184, CARD_1, Kind::Credit, -4_788),
            (214, CARD_1, Kind::Purchase, 9_477),
            (257, CARD_1, Kind::Purchase, 24_166),
            (37, CARD_2, Kind::Purchase, 77_733),
            (52, CARD_2, Kind::Purchase, 57_977),
            (145, CARD_2, Kind::Purchase, 31_988),
            (247, CARD_2, Kind::Purchase, 97_511),
            (277, CARD_2, Kind::Purchase, 93_255),
            (281, CARD_2, Kind::Credit, -37_299),
            (24, CARD_3, Kind::Purchase, 35_511),
        ];
        rows.into_iter()
            .map(|(n, card, kind, cents)| tran(n, card, kind, cents))
            .collect()
    }

    /// The 30 records the three blocks give: FIXRPT-001 and FIXRPT-002 as audited (records 1 to
    /// 26), the first detail of FIXRPT-003 (27), then the end of file over 355.11 (28 to 30).
    fn three_block_report() -> Result<Vec<ReportRecord>, Box<dyn StdError>> {
        let acct_50 = "00000000050";
        let acct_27 = "00000000027";
        let mut expected = header_group()?;
        for text in [
            detail_text(21, acct_50, Kind::Purchase, "         183.88  "),
            detail_text(102, acct_50, Kind::Purchase, "          14.00  "),
            detail_text(142, acct_50, Kind::Purchase, "         967.44  "),
            detail_text(184, acct_50, Kind::Credit, "-         47.88  "),
            detail_text(214, acct_50, Kind::Purchase, "          94.77  "),
            detail_text(257, acct_50, Kind::Purchase, "         241.66  "),
            total_text("Account Total", "+      1,453.87"),
        ] {
            expected.push(padded(&text)?);
        }
        expected.push([b'-'; REPORT_RECORD_LEN]);
        for text in [
            detail_text(37, acct_27, Kind::Purchase, "         777.33  "),
            detail_text(52, acct_27, Kind::Purchase, "         579.77  "),
            detail_text(145, acct_27, Kind::Purchase, "         319.88  "),
            detail_text(247, acct_27, Kind::Purchase, "         975.11  "),
            detail_text(277, acct_27, Kind::Purchase, "         932.55  "),
            detail_text(281, acct_27, Kind::Credit, "-        372.99  "),
            total_text("Account Total", "+      3,211.65"),
        ] {
            expected.push(padded(&text)?);
        }
        expected.push([b'-'; REPORT_RECORD_LEN]);
        expected.push(padded(&total_text("Page Total ", "+      4,665.52"))?);
        expected.push([b'-'; REPORT_RECORD_LEN]);
        expected.extend(header_group()?);
        let first_of_block_3 = detail_text(24, "00000000002", Kind::Purchase, "         355.11  ");
        expected.push(padded(&first_of_block_3)?);
        expected.push(padded(&total_text("Page Total ", "+        710.22"))?);
        expected.push([b'-'; REPORT_RECORD_LEN]);
        expected.push(padded(&total_text("Grand Total", "+      5,375.74"))?);
        Ok(expected)
    }

    // audit: CBTRN03C-013
    // audit: CBTRN03C-015
    // audit: CBTRN03C-017
    // audit: CBTRN03C-019
    // audit: CBTRN03C-020
    // audit: CBTRN03C-021
    // audit: CBTRN03C-022
    // audit: CBTRN03C-023
    // audit: CBTRN03C-025
    // audit: CBTRN03C-027
    // audit: CBTRN03C-029
    // audit: CBTRN03C-030
    // audit: CBTRN03C-031
    // audit: CBTRN03C-032
    // audit: FIXRPT-001
    // audit: FIXRPT-002
    // audit: FIXRPT-902
    #[test]
    fn card_blocks_totals_page_break_and_end_of_file_follow_the_audited_records() -> TestResult {
        let mut report = run()?;
        for row in three_blocks()? {
            assert_eq!(report.process(&row)?, Flow::Continue);
        }
        assert_eq!(
            report.end_of_file_display(),
            Some([
                "TRAN-AMT 0000003551A".to_owned(),
                "WS-PAGE-TOTAL0000003551A".to_owned(),
            ])
        );
        let records = report.finish()?;
        let expected = three_block_report()?;
        assert_eq!(records.len(), expected.len());
        for (n, (record, wanted)) in records.iter().zip(&expected).enumerate() {
            assert_eq!(
                String::from_utf8_lossy(record),
                String::from_utf8_lossy(wanted),
                "record {}",
                n.saturating_add(1)
            );
        }
        // No record holds a card number (CBTRN03C-039).
        for card in [CARD_1, CARD_2, CARD_3] {
            assert!(
                !records
                    .iter()
                    .any(|record| record.windows(16).any(|w| w == card))
            );
        }
        Ok(())
    }

    // audit: CBTRN03C-021
    #[test]
    fn page_breaks_test_the_line_counter_for_an_exact_multiple_of_twenty() -> TestResult {
        // One card of 17 rows: the counter reaches 20 before the 17th detail.
        let mut report = run()?;
        for n in 1..=17 {
            report.process(&tran(n, CARD_1, Kind::Purchase, 100)?)?;
        }
        let records = report.finish()?;
        assert_eq!(records.len(), 30);
        assert_eq!(
            nth(&records, 21)?,
            &padded(&total_text("Page Total ", "+         16.00"))?
        );
        assert_eq!(nth(&records, 23)?, &padded(NAME_HEADER)?);
        assert_eq!(
            nth(&records, 28)?,
            &padded(&total_text("Page Total ", "+          2.00"))?
        );
        assert_eq!(
            nth(&records, 30)?,
            &padded(&total_text("Grand Total", "+         18.00"))?
        );

        // 15 rows then 20: the account-total pair takes the counter from 19 to 21, stepping over
        // 20, so the page runs on to 40 (X-TRANREPT-8).
        let mut report = run()?;
        for n in 1..=15 {
            report.process(&tran(n, CARD_1, Kind::Purchase, 100)?)?;
        }
        for n in 16..=35 {
            report.process(&tran(n, CARD_2, Kind::Purchase, 100)?)?;
        }
        let records = report.finish()?;
        assert_eq!(records.len(), 50);
        let page_total_at: Vec<usize> = records
            .iter()
            .enumerate()
            .filter(|(_, record)| record.starts_with(b"Page Total"))
            .map(|(at, _)| at.saturating_add(1))
            .collect();
        assert_eq!(page_total_at, [41, 48]);
        assert_eq!(
            nth(&records, 20)?,
            &padded(&total_text("Account Total", "+         15.00"))?
        );
        assert_eq!(
            nth(&records, 41)?,
            &padded(&total_text("Page Total ", "+         34.00"))?
        );
        Ok(())
    }

    // audit: CBTRN03C-021
    #[test]
    fn the_line_counter_keeps_its_low_nine_digits() -> TestResult {
        let mut report = run()?;
        report.line_counter = 999_999_999;
        report.count_line();
        assert_eq!(report.line_counter, 0);
        report.count_line();
        assert_eq!(report.line_counter, 1);
        Ok(())
    }

    // audit: CBTRN03C-011
    #[test]
    fn a_record_outside_the_window_leaves_the_loop_with_no_total() -> TestResult {
        let late = b"2022-07-07-10.15.42.370000";
        let mut report = run()?;
        assert_eq!(
            report.process(&tran(1, CARD_1, Kind::Purchase, 100)?)?,
            Flow::Continue
        );
        assert_eq!(
            report.process(&tran_at(2, CARD_1, Kind::Purchase, 200, late)?)?,
            Flow::Stop
        );
        // The loop is over: a later row inside the window is not read.
        assert_eq!(
            report.process(&tran(3, CARD_2, Kind::Purchase, 300)?)?,
            Flow::Stop
        );
        assert_eq!(report.end_of_file_display(), None);
        let mut expected = header_group()?;
        expected.push(padded(&detail_text(
            1,
            "00000000050",
            Kind::Purchase,
            "           1.00  ",
        ))?);
        assert_eq!(report.finish()?, expected);

        let mut report = run()?;
        assert_eq!(
            report.process(&tran_at(1, CARD_1, Kind::Purchase, 100, late)?)?,
            Flow::Stop
        );
        assert_eq!(report.finish()?, Vec::<ReportRecord>::new());
        Ok(())
    }

    /// Records taken after each pass, then those `finish` returns, are the report of the same
    /// run finished at once, in the same order: over three card blocks with their totals and
    /// page break, and over a run the window test leaves.
    // audit: CBTRN03C-011
    // audit: CBTRN03C-032
    #[test]
    fn records_taken_as_the_run_goes_are_the_report_finished_at_once() -> TestResult {
        let late = b"2022-07-07-10.15.42.370000";
        let left = vec![
            tran(1, CARD_1, Kind::Purchase, 100)?,
            tran_at(2, CARD_1, Kind::Purchase, 200, late)?,
            tran(3, CARD_2, Kind::Purchase, 300)?,
        ];
        for rows in [three_blocks()?, left] {
            let mut whole = run()?;
            let mut streamed = run()?;
            let mut taken = Vec::new();
            for row in &rows {
                let flow = whole.process(row)?;
                assert_eq!(streamed.process(row)?, flow);
                taken.extend(streamed.take_records());
            }
            assert!(
                streamed.take_records().is_empty(),
                "a record was taken twice"
            );
            taken.extend(streamed.finish()?);
            let expected = whole.finish()?;
            assert!(!expected.is_empty());
            assert_eq!(taken, expected, "{} rows", rows.len());
        }
        Ok(())
    }

    // audit: CBTRN03C-037
    #[test]
    fn an_empty_selection_writes_no_record() -> TestResult {
        let report = run()?;
        assert_eq!(report.end_of_file_display(), None);
        assert_eq!(report.finish()?, Vec::<ReportRecord>::new());
        Ok(())
    }

    // audit: CBTRN03C-014
    // audit: CBTRN03C-016
    // audit: CBTRN03C-018
    // audit: CBTRN03C-035
    // audit: CBTRN03C-036
    // audit: CBTRN03C-039
    #[test]
    fn a_failed_lookup_abends_with_the_audited_lines_and_no_card_number() -> TestResult {
        let unknown_type = TransactionRecord {
            type_cd: PicX::from_array(*b"09"),
            ..tran(2, CARD_1, Kind::Purchase, 100)?
        };
        let unknown_category = TransactionRecord {
            cat_cd: PicU::new(9)?,
            ..tran(2, CARD_1, Kind::Purchase, 100)?
        };
        let cases = [
            (
                tran(2, UNKNOWN_CARD, Kind::Purchase, 100)?,
                ReportError::XrefNotFound {
                    redacted_card: *b"************9999",
                },
                "INVALID CARD NUMBER : ************9999",
            ),
            (
                unknown_type,
                ReportError::TypeNotFound { type_cd: *b"09" },
                "INVALID TRANSACTION TYPE : 09",
            ),
            (
                unknown_category,
                ReportError::CategoryNotFound {
                    type_cd: *b"01",
                    cat_cd: PicU::new(9)?,
                },
                "INVALID TRAN CATG KEY : 010009",
            ),
        ];
        for (row, error, line) in cases {
            let mut report = run()?;
            report.process(&tran(1, CARD_1, Kind::Purchase, 100)?)?;
            assert_eq!(report.process(&row), Err(error));
            assert_eq!(
                error.job_log_lines(),
                Some([line.to_owned(), "FILE STATUS IS: NNNN0023".to_owned()])
            );
            assert!(!format!("{error} {error:?}").contains("4000000000009999"));
            // The run is over: every later call and the end return the same error.
            assert_eq!(
                report.process(&tran(3, CARD_1, Kind::Purchase, 100)?),
                Err(error)
            );
            assert_eq!(report.end_of_file_display(), None);
            assert_eq!(report.finish(), Err(error));
        }
        Ok(())
    }

    #[test]
    fn a_card_number_that_is_not_sixteen_digits_ends_the_run() -> TestResult {
        for card in [*b"                ", *b"40000000000A0001"] {
            let mut report = run()?;
            let row = tran(1, card, Kind::Purchase, 100)?;
            assert_eq!(report.process(&row), Err(ReportError::InvalidCardNumber));
            assert_eq!(ReportError::InvalidCardNumber.job_log_lines(), None);
            assert_eq!(report.finish(), Err(ReportError::InvalidCardNumber));
        }
        Ok(())
    }

    // audit: CBTRN03C-015
    // audit: CBTRN03C-017
    // audit: CBTRN03C-023
    // audit: CBTRN03C-038
    #[test]
    fn descriptions_come_from_the_vsam_rows_cut_to_their_report_widths() -> TestResult {
        let row = tran(21, CARD_1, Kind::Purchase, 18_388)?;
        let vsam = detail(
            &row,
            PicU::new(50)?,
            &PicX::from_text("Purchase")?,
            &PicX::from_text("Regular Sales Draft")?,
        );
        assert_eq!(
            vsam,
            padded(&detail_text(
                21,
                "00000000050",
                Kind::Purchase,
                "         183.88  "
            ))?
        );
        let long = detail(
            &row,
            PicU::new(99_999_999_999)?,
            &PicX::from_text("Purchase by card holder present")?,
            &PicX::from_text("Regular sales draft for a card holder present")?,
        );
        let expected = format!(
            "{} 99999999999 01-Purchase by car 0001-Regular sales draft for a car POS TERM               183.88",
            id(21)
        );
        assert_eq!(long, padded(&expected)?);
        assert_eq!(long.get(112..), Some([b' '; 21].as_slice()));
        Ok(())
    }

    // audit: CBTRN03C-024
    // audit: CBTRN03C-026
    #[test]
    fn amounts_follow_the_report_pictures() -> TestResult {
        let details = [
            (18_388, "         183.88"),
            (-4_788, "-         47.88"),
            (99, "            .99"),
            (123_456_789, "   1,234,567.89"),
            (0, "               "),
            (-99_999_999_999, "-999,999,999.99"),
        ];
        for (cents, edited) in details {
            let row = tran(1, CARD_1, Kind::Purchase, cents)?;
            let record = detail(
                &row,
                PicU::new(50)?,
                &PicX::from_text("Purchase")?,
                &PicX::from_text("Regular Sales Draft")?,
            );
            assert_eq!(record.get(97..112), Some(edited.as_bytes()), "{cents}");
        }
        let totals = [
            (145_387, "+      1,453.87"),
            (-13_288, "-        132.88"),
            (7_872_036, "+     78,720.36"),
            (99_999_999_999, "+999,999,999.99"),
            (0, "               "),
        ];
        for (cents, edited) in totals {
            for record in [page_totals(money(cents)?), account_totals(money(cents)?)] {
                assert_eq!(record.get(97..112), Some(edited.as_bytes()), "{cents}");
                assert_eq!(record.get(112..), Some([b' '; 21].as_slice()));
            }
        }
        Ok(())
    }

    // audit: CBTRN03C-028
    // audit: ARITH-030
    #[test]
    fn totals_store_by_truncation_at_nine_integer_digits() -> TestResult {
        let mut report = run()?;
        report.process(&tran(1, CARD_1, Kind::Purchase, 99_999_999_999)?)?;
        report.process(&tran(2, CARD_1, Kind::Purchase, 1)?)?;
        assert_eq!(report.page_total, Money::<9>::ZERO);
        let records = report.finish()?;
        // The end of file adds 0.01 again: page total 0.01, grand total 0.00 + 0.01.
        assert_eq!(
            nth(&records, 7)?,
            &padded(&total_text("Page Total ", "+           .01"))?
        );
        assert_eq!(
            nth(&records, 9)?,
            &padded(&total_text("Grand Total", "+           .01"))?
        );

        let mut report = run()?;
        report.process(&tran(1, CARD_1, Kind::Credit, -99_999_999_999)?)?;
        report.process(&tran(2, CARD_1, Kind::Credit, -100)?)?;
        assert_eq!(report.page_total, money(-99)?);
        assert_eq!(report.account_total, money(-99)?);
        Ok(())
    }

    // audit: CBTRN03C-020
    #[test]
    fn every_header_group_carries_the_window_dates() -> TestResult {
        let parm = DateParm::parse("2022-06-01", "2022-07-01")?;
        let mut report = Tranrept::new(parm, xrefs()?, vsam_types()?, vsam_categories()?);
        report.process(&tran(1, CARD_1, Kind::Purchase, 100)?)?;
        let records = report.finish()?;
        let header = NAME_HEADER.replace("2022-01-01 to 2022-07-06", "2022-06-01 to 2022-07-01");
        assert_eq!(nth(&records, 1)?, &padded(&header)?);
        assert_eq!(nth(&records, 1)?.get(115..), Some([b' '; 18].as_slice()));
        Ok(())
    }
}
