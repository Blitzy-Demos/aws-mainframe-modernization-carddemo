//! CBPAUP0C, the expired pending-authorization purge that job CBPAUP0J runs: the pure rules of
//! `carddemo-batch cbpaup0j` (AAP 0.3.4.3).
//!
//! This module ports the rows of `modern/audit/batch/CBPAUP0J.md`: the SYSIN parse and its
//! defaults (CBPAUP0C-002, CBPAUP0C-004 to CBPAUP0C-007), today's `YYDDD` (CBPAUP0C-001), the
//! expiry test (CBPAUP0C-011, CBPAUP0C-012), the subtractions from the in-memory summary
//! (CBPAUP0C-013, CBPAUP0C-014; ARITH-046 to ARITH-048), the detail and summary delete decisions
//! (CBPAUP0C-015, CBPAUP0C-016), the checkpoint tests (CBPAUP0C-018 to CBPAUP0C-020) and every
//! SYSOUT line, in program order (CBPAUP0C-003, CBPAUP0C-008, CBPAUP0C-020, CBPAUP0C-021,
//! CBPAUP0C-023 to CBPAUP0C-028), with the return codes 0 and 16 (CBPAUP0C-022, CBPAUP0C-028).
//!
//! # Flow
//!
//! `batch::jobs::cbpaup0j` reads, deletes and commits; this module decides, counts and keeps the
//! SYSOUT lines. The job drives it in the program's order:
//!
//! 1. [`PurgeParams::parse`] the SYSIN record, then [`Purge::new`] with the injected clock, which
//!    is read once (CBPAUP0C-001).
//! 2. For each summary in ascending `acct_id` (CBPAUP0C-009), [`Purge::begin_summary`]. For each
//!    of its details in ascending (`auth_date_9c`, `auth_time_9c`) order (CBPAUP0C-010),
//!    [`SummaryPass::on_detail`]; the job deletes the detail when it answers
//!    [`DetailAction::Delete`] (CBPAUP0C-015).
//! 3. [`SummaryPass::end_summary`]. The job deletes the summary when it answers
//!    [`SummaryDisposition::Delete`]; `ON DELETE CASCADE` then removes every detail still under
//!    it, as IMS deletes child segments with their parent (CBPAUP0C-016, CIPAUDTY-029). When it
//!    answers [`CheckpointAction::Take`], the job commits and calls [`Purge::checkpoint_taken`]
//!    (CBPAUP0C-018, CBPAUP0C-020).
//! 4. At the end of the summaries the job commits, the final checkpoint (CBPAUP0C-019), then
//!    calls [`Purge::finish`], which returns code 0.
//! 5. When a read, a delete or a commit fails, the job calls [`Purge::abend`] instead, which
//!    returns code 16 (CBPAUP0C-023 to CBPAUP0C-028).
//!
//! A delete is counted when it is decided. A delete that then fails ends the run through
//! [`Purge::abend`], whose lines show no deleted count, so a count is never shown wrong.
//!
//! # No write-back
//!
//! CBPAUP0C issues no `REPL`: its subtractions change only the copy of the summary in working
//! storage, the [`WorkingSummary`] of a [`SummaryPass`], and only its approved count is used, by
//! the delete test. No action this module returns updates a summary, so every surviving summary
//! keeps its stored counts and amounts, `credit_balance` and `approved_auth_amt` included
//! (CBPAUP0C-017, ARITH-049; E30).
//!
//! # Text comparisons
//!
//! `P-CHKP-FREQ` and `P-CHKP-DIS-FREQ` are `PIC X(05)` and are compared with the `PIC 9(8)`
//! counters as text: the counter's eight digits against the parameter padded with spaces to
//! eight characters, in the EBCDIC collating sequence (CBPAUP0C-018, CBPAUP0C-020). The default
//! card's `00001` therefore means every 1000 summaries, not every summary. Both comparisons go
//! through one helper, `compare_as_text`.
//!
//! # Halts
//!
//! - `HALTED(H4)`: the two `DIBSTAT` characters of the failure lines are IMS status codes, which
//!   `PostgreSQL` has none of (CBPAUP0C-029). The replacement shows `--` there; it is no parity
//!   result, and the labels, counts and keys around it are.
//! - Halt candidate X-CBPAUP0J-H1, with X-ARITH-6: a `YYDDD` difference beyond ±9999 has no
//!   single `PIC S9(4) COMP` value (CBPAUP0C-012), and neither has a count taken below −9999
//!   (ARITH-046). [`SummaryPass::on_detail`] answers both with a [`PurgeHalt`] and changes
//!   nothing, so the job reports the case blocked.
//! - Halt candidate X-CBPAUP0J-H4: `WS-CURR-APP-ID` shown before any summary has been read is
//!   uninitialised storage (CBPAUP0C-020, CBPAUP0C-027). The replacement shows eleven zeros
//!   there, which is no parity result.
//!
//! # Displayed values
//!
//! Counts show eight digits and account keys eleven, with leading zeros and no sign character,
//! as `DISPLAY` shows the non-negative values this program displays (X-CBPAUP0J-6). A negative
//! `PA-ACCT-ID`, which CP00 never writes, ends in the low-order sign overpunch IBM documents for
//! negative values, while `WS-CURR-APP-ID PIC 9(11)` keeps only its magnitude. The
//! `PIC S9(8) COMP` counts are held in 64 bits; above 99 999 999 their COBOL value depends on
//! the `TRUNC` option, as X-ARITH-6 states for halfword counts, and the low-order eight digits
//! shown are no parity result.
//!
//! # Rules
//!
//! No line shows a card number: the program displays parameters, dates, counts and `PA-ACCT-ID`
//! only. The parameters are unvalidated SYSIN text, and a password is any 1 to 8 printable
//! characters, so any received bytes can be or hold a protected value. `PARM RECEIVED` and
//! `Debug` therefore show the received bytes only when they are one of two public values, the
//! default card's 17 bytes, published as the job's in-stream card, or 17 spaces, which no card
//! number, SSN or accepted password is. Any other value shows 17 `*`, and `Debug` then masks
//! every value derived from it, while the run still uses the bytes as received (Cardholder Data
//! Redaction; CBPAUP0C-003, X-CBPAUP0J-7). The parse is total and indexes nothing (Total Parsers
//! Over Untrusted Input). Amounts move only through `crate::money` and `store::<9>()`, and today
//! comes only from the injected [`Clock`]. Narrowing is checked or follows a COBOL store rule,
//! each such place commented (Checked Integer Narrowing). No I/O.

use core::cmp::Ordering;
use core::fmt;

use copybook::{Count, PicS};

use crate::clock::{Clock, yyddd};
use crate::money::Money;
use crate::records::{PendingAuthDetailRecord, PendingAuthSummaryRecord};

/// The in-stream SYSIN card of `CBPAUP0J.jcl:37`, the default of `--sysin` (CBPAUP0C-002).
pub const DEFAULT_SYSIN: &str = "00,00001,00001,Y";

/// Bytes of a SYSIN card: a `DD *` record is 80 bytes, and a shorter value is padded with spaces
/// to that length (CBPAUP0C-002).
pub const SYSIN_RECORD_LEN: usize = 80;

/// Bytes of `PRM-INFO`, the first 17 of the card, which `ACCEPT PRM-INFO FROM SYSIN` keeps
/// (`CBPAUP0C.cbl:98-108,189`; CBPAUP0C-002).
pub const PRM_INFO_LEN: usize = 17;

/// The return code of a normal end: `GOBACK` with `RETURN-CODE` never set (CBPAUP0C-022).
pub const NORMAL_RETURN_CODE: u8 = 0;

/// `MOVE 16 TO RETURN-CODE` in `9999-ABEND` (`CBPAUP0C.cbl:377-383`; CBPAUP0C-028).
pub const ABEND_RETURN_CODE: u8 = 16;

/// Width of `P-CHKP-FREQ` and `P-CHKP-DIS-FREQ PIC X(05)` (CBPAUP0C-005, CBPAUP0C-006).
const FREQ_LEN: usize = 5;

/// `MOVE 5 TO WS-EXPIRY-DAYS` when `P-EXPIRY-DAYS` is not `NUMERIC` (CBPAUP0C-004).
const DEFAULT_EXPIRY_DAYS: u8 = 5;

/// `MOVE 5 TO P-CHKP-FREQ`: the integer 5 moved into an alphanumeric field, left-justified and
/// padded with spaces (CBPAUP0C-005).
const DEFAULT_CHKP_FREQ: [u8; FREQ_LEN] = *b"5    ";

/// `MOVE 10 TO P-CHKP-DIS-FREQ`, left-justified and padded with spaces (CBPAUP0C-006).
const DEFAULT_CHKP_DIS_FREQ: [u8; FREQ_LEN] = *b"10   ";

/// The figurative constant `SPACES` against a five-byte field.
const SPACES: [u8; FREQ_LEN] = [b' '; FREQ_LEN];

/// The numeric literal `0` against an alphanumeric field: the one-character text `0`, padded
/// with spaces (CBPAUP0C-005).
const ZERO_TEXT: [u8; FREQ_LEN] = *b"0    ";

/// The figurative constant `LOW-VALUES` against a five-byte field. [`PurgeParams::parse`]
/// refuses control bytes, so no parameter equals it; the term keeps the COBOL condition whole
/// (CBPAUP0C-005, CBPAUP0C-006).
const LOW_VALUES: [u8; FREQ_LEN] = [0; FREQ_LEN];

/// The `P-DEBUG-FLAG` value of `88 DEBUG-ON` (CBPAUP0C-007).
const DEBUG_ON_FLAG: u8 = b'Y';

/// `PRM-INFO` of the default card, [`DEFAULT_SYSIN`] padded with spaces to 17 bytes:
/// `00,00001,00001,Y ` (CBPAUP0C-002). It is the published in-stream card of `CBPAUP0J.jcl:37`,
/// so showing it discloses nothing (CBPAUP0C-003, X-CBPAUP0J-7).
const DEFAULT_PRM_INFO: [u8; PRM_INFO_LEN] = prm_info_of(DEFAULT_SYSIN.as_bytes());

/// `PRM-INFO` of an empty or blank value: 17 spaces (CBPAUP0C-002). No card number or SSN is
/// blank, and COUSR01C and sign-on refuse a blank password, so showing it discloses nothing
/// (CBPAUP0C-003, X-CBPAUP0J-7).
const BLANK_PRM_INFO: [u8; PRM_INFO_LEN] = [b' '; PRM_INFO_LEN];

/// What `PARM RECEIVED` and `Debug` show in place of a `PRM-INFO` that is neither
/// [`DEFAULT_PRM_INFO`] nor [`BLANK_PRM_INFO`]: 17 `*`, whatever the received bytes hold, so the
/// line keeps its 41 characters and shows none of them (Cardholder Data Redaction; CBPAUP0C-003,
/// X-CBPAUP0J-7).
const MASKED_PRM_INFO: [u8; PRM_INFO_LEN] = [b'*'; PRM_INFO_LEN];

/// What `Debug` shows in place of `WS-EXPIRY-DAYS` of a masked `PRM-INFO`: 2 `*`, the width of
/// `P-EXPIRY-DAYS PIC 9(02)`, since the days are the value's first two bytes whenever they are
/// digits (CBPAUP0C-003, CBPAUP0C-004, X-CBPAUP0J-7).
const MASKED_EXPIRY_DAYS: [u8; 2] = [b'*'; 2];

/// What `Debug` shows in place of a frequency of a masked `PRM-INFO`, since the frequency is five
/// of the bytes the mask hides unless the default replaced them: 5 `*` (CBPAUP0C-003,
/// X-CBPAUP0J-7).
const MASKED_FREQ: [u8; FREQ_LEN] = [b'*'; FREQ_LEN];

/// Spaces that pad a five-character parameter to the eight digits of a `PIC 9(8)` counter in a
/// text comparison (CBPAUP0C-018).
const COUNTER_PAD: [u8; 3] = [b' '; 3];

/// `10^8`: a `PIC 9(8)` counter keeps the low-order eight digits of what it receives.
const PIC_9_8_MODULUS: u32 = 100_000_000;

/// `10^8` for the eight digits a `PIC S9(8) COMP` count shows.
const COUNT_DISPLAY_MODULUS: u64 = 100_000_000;

/// `10^5`: `WS-AUTH-DATE PIC 9(05)` keeps the low-order five digits of what it receives.
const PIC_9_5_MODULUS: i64 = 100_000;

/// The minuend of `COMPUTE WS-AUTH-DATE = 99999 - PA-AUTH-DATE-9C` (CBPAUP0C-011).
const AUTH_DATE_NINES: i64 = 99_999;

/// The largest magnitude `WS-DAY-DIFF PIC S9(4) COMP` holds with a single value (CBPAUP0C-012).
const DAY_DIFF_MAX: i16 = 9_999;

/// What the failure lines show where the COBOL shows `DIBSTAT`: `HALTED(H4)`, no parity result
/// (CBPAUP0C-029).
const UNDEFINED_DIBSTAT: &str = "--";

/// What a line shows for an account key before any summary has been read: halt candidate
/// X-CBPAUP0J-H4, no parity result (CBPAUP0C-020, CBPAUP0C-027).
const UNSET_APP_ID: &str = "00000000000";

/// The ASCII form of a negative zoned low-order digit 0 to 9, the EBCDIC `D` zone: `}` and `J` to
/// `R` (X-CBPAUP0J-6).
const NEGATIVE_OVERPUNCH: &str = "}JKLMNOPQR";

/// `DISPLAY ' '`.
const BLANK_LINE: &str = " ";
/// An asterisk, 37 hyphens and an asterisk (CBPAUP0C-003, CBPAUP0C-021).
const DASHES: &str = "*-------------------------------------*";
/// CBPAUP0C-003.
const STARTING: &str = "STARTING PROGRAM CBPAUP0C::";
/// CBPAUP0C-003.
const PARM_RECEIVED: &str = "CBPAUP0C PARM RECEIVED :";
/// CBPAUP0C-003: `TODAYS DATE`, 12 spaces and a colon.
const TODAYS_DATE: &str = "TODAYS DATE            :";
/// CBPAUP0C-008.
const DEBUG_SMRY_READ: &str = "DEBUG: AUTH SMRY READ : ";
/// CBPAUP0C-008.
const DEBUG_DTL_READ: &str = "DEBUG: AUTH DTL READ : ";
/// CBPAUP0C-008.
const DEBUG_DTL_DLET: &str = "DEBUG: AUTH DTL DLET : ";
/// CBPAUP0C-008.
const DEBUG_SMRY_DLET: &str = "DEBUG: AUTH SMRY DLET : ";
/// CBPAUP0C-020.
const CHKP_SUCCESS: &str = "CHKP SUCCESS: AUTH COUNT - ";
/// CBPAUP0C-020, CBPAUP0C-027.
const APP_ID_SEPARATOR: &str = ", APP ID - ";
/// CBPAUP0C-027.
const CHKP_FAILED: &str = "CHKP FAILED: DIBSTAT - ";
/// CBPAUP0C-027.
const REC_COUNT_SEPARATOR: &str = ", REC COUNT - ";
/// CBPAUP0C-021.
const TOTAL_SUMMARY_READ: &str = "# TOTAL SUMMARY READ  :";
/// CBPAUP0C-021.
const SUMMARY_REC_DELETED: &str = "# SUMMARY REC DELETED :";
/// CBPAUP0C-021.
const TOTAL_DETAILS_READ: &str = "# TOTAL DETAILS READ  :";
/// CBPAUP0C-021.
const DETAILS_REC_DELETED: &str = "# DETAILS REC DELETED :";
/// CBPAUP0C-023.
const SUMMARY_READ_FAILED: &str = "AUTH SUMMARY READ FAILED  :";
/// CBPAUP0C-023.
const SUMMARY_READ_BEFORE_ABEND: &str = "SUMMARY READ BEFORE ABEND :";
/// CBPAUP0C-024.
const DETAIL_READ_FAILED: &str = "AUTH DETAIL READ FAILED  :";
/// CBPAUP0C-024.
const SUMMARY_AUTH_APP_ID: &str = "SUMMARY AUTH APP ID      :";
/// CBPAUP0C-024.
const DETAIL_READ_BEFORE_ABEND: &str = "DETAIL READ BEFORE ABEND :";
/// CBPAUP0C-025.
const DETAIL_DELETE_FAILED: &str = "AUTH DETAIL DELETE FAILED :";
/// CBPAUP0C-025: `AUTH APP ID`, 15 spaces and a colon.
const DETAIL_DELETE_APP_ID: &str = "AUTH APP ID               :";
/// CBPAUP0C-026.
const SUMMARY_DELETE_FAILED: &str = "AUTH SUMMARY DELETE FAILED :";
/// CBPAUP0C-026: `AUTH APP ID`, 16 spaces and a colon.
const SUMMARY_DELETE_APP_ID: &str = "AUTH APP ID                :";
/// CBPAUP0C-028.
const ABENDING: &str = "CBPAUP0C ABENDING ...";

/// Why a SYSIN value is not a card `ACCEPT` could have read (CBPAUP0C-002). The job prints no
/// line, touches no table and exits non-zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysinError {
    /// The value is longer than the 80-byte card.
    TooLong {
        /// The value's length in bytes.
        len: usize,
    },
    /// The byte at 0-based `offset` is not printable ASCII (0x20 to 0x7E): a line terminator,
    /// another control byte or part of a non-ASCII character.
    NotPrintable {
        /// The byte's 0-based offset in the value.
        offset: usize,
    },
}

impl fmt::Display for SysinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { len } => {
                write!(
                    f,
                    "SYSIN record is {len} bytes, longer than the {SYSIN_RECORD_LEN}-byte card"
                )
            }
            Self::NotPrintable { offset } => {
                write!(f, "SYSIN byte {offset} is not printable ASCII")
            }
        }
    }
}

impl std::error::Error for SysinError {}

/// `P-DEBUG-FLAG` after its default: `88 DEBUG-ON` (`Y`) or `88 DEBUG-OFF` (`N`, which every
/// other byte becomes) (CBPAUP0C-007).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugFlag {
    /// `Y`: the `DEBUG:` lines of CBPAUP0C-008 are shown.
    On,
    /// Anything else: they are not.
    Off,
}

/// `PRM-INFO` as received, and the run parameters CBPAUP0C derives from it in
/// `1000-INITIALIZE` (`CBPAUP0C.cbl:98-108,189-209`; CBPAUP0C-002 to CBPAUP0C-007).
///
/// Built only by [`PurgeParams::parse`], so every value holds the defaults the program applies.
///
/// The received bytes are unvalidated SYSIN text, so they can be or hold a card number, an SSN
/// or a password, which is any 1 to 8 printable characters, digits and punctuation included. The
/// run uses them as received, while `PARM RECEIVED` and `Debug` show them only when they are one
/// of the two public values, the default card's 17 bytes or 17 spaces, and otherwise 17 `*`;
/// `Debug` then masks every value derived from them, the debug flag aside (Cardholder Data
/// Redaction; CBPAUP0C-003, X-CBPAUP0J-7).
#[derive(Clone, PartialEq, Eq)]
pub struct PurgeParams {
    /// The 17 bytes of `PRM-INFO` before any default. `PARM RECEIVED` shows them only when they
    /// are the default card's or 17 spaces (CBPAUP0C-003, X-CBPAUP0J-7).
    raw: [u8; PRM_INFO_LEN],
    /// `WS-EXPIRY-DAYS`: 0 to 99, or 5.
    expiry_days: u8,
    /// `P-CHKP-FREQ` after its default.
    chkp_freq: [u8; FREQ_LEN],
    /// `P-CHKP-DIS-FREQ` after its default.
    chkp_dis_freq: [u8; FREQ_LEN],
    /// `P-DEBUG-FLAG` after its default.
    debug: DebugFlag,
}

impl PurgeParams {
    /// Reads one SYSIN record as `ACCEPT PRM-INFO FROM SYSIN` does, then applies the defaults of
    /// `1000-INITIALIZE` (CBPAUP0C-002 to CBPAUP0C-007).
    ///
    /// The value is padded with spaces to the 80-byte card and its first 17 bytes are
    /// `PRM-INFO`: bytes 1-2 `P-EXPIRY-DAYS PIC 9(02)`, 4-8 `P-CHKP-FREQ PIC X(05)`, 10-14
    /// `P-CHKP-DIS-FREQ PIC X(05)`, 16 `P-DEBUG-FLAG PIC X(01)`; bytes 3, 9, 15 and 17 are
    /// `FILLER` and never tested. An empty value gives 17 spaces, so every default. Total for
    /// every byte sequence (Total Parsers Over Untrusted Input).
    ///
    /// # Errors
    ///
    /// [`SysinError::TooLong`] for a value longer than 80 bytes, and
    /// [`SysinError::NotPrintable`] for one holding a byte outside 0x20 to 0x7E (CBPAUP0C-002).
    pub fn parse(sysin: &[u8]) -> Result<Self, SysinError> {
        if sysin.len() > SYSIN_RECORD_LEN {
            return Err(SysinError::TooLong { len: sysin.len() });
        }
        if let Some(offset) = sysin.iter().position(|&byte| !is_printable(byte)) {
            return Err(SysinError::NotPrintable { offset });
        }
        let mut raw = [b' '; PRM_INFO_LEN];
        for (slot, &byte) in raw.iter_mut().zip(sysin) {
            *slot = byte;
        }
        // P-EXPIRY-DAYS, FILLER; P-CHKP-FREQ, FILLER; P-CHKP-DIS-FREQ, FILLER, P-DEBUG-FLAG,
        // FILLER.
        let [e1, e2, _, rest @ ..] = raw;
        let [c1, c2, c3, c4, c5, _, rest @ ..] = rest;
        let [d1, d2, d3, d4, d5, _, flag, _] = rest;
        Ok(Self {
            raw,
            expiry_days: expiry_days([e1, e2]),
            chkp_freq: frequency_or_default([c1, c2, c3, c4, c5], DEFAULT_CHKP_FREQ),
            chkp_dis_freq: frequency_or_default([d1, d2, d3, d4, d5], DEFAULT_CHKP_DIS_FREQ),
            debug: if flag == DEBUG_ON_FLAG {
                DebugFlag::On
            } else {
                DebugFlag::Off
            },
        })
    }

    /// The 17 bytes of `PRM-INFO` as received, before any default (CBPAUP0C-002), for the run's
    /// logic and its checks. They can hold any printable text, a password of digits and
    /// punctuation included, so they never form a log line: `PARM RECEIVED` shows the form of
    /// CBPAUP0C-003, which is the received bytes only for the default card and 17 spaces
    /// (X-CBPAUP0J-7).
    #[must_use]
    pub const fn raw(&self) -> &[u8; PRM_INFO_LEN] {
        &self.raw
    }

    /// The 17 bytes `PARM RECEIVED` and `Debug` show: the received bytes when they are
    /// [`DEFAULT_PRM_INFO`] or [`BLANK_PRM_INFO`], otherwise [`MASKED_PRM_INFO`]. The decision
    /// is taken over the whole value, so no fragment of any other value is shown (CBPAUP0C-003;
    /// X-CBPAUP0J-7). Bytes 18 to 80 of the card are never part of `PRM-INFO`, so the default
    /// card followed by more text is still shown as the default card.
    fn shown(&self) -> [u8; PRM_INFO_LEN] {
        if is_public(&self.raw) {
            self.raw
        } else {
            MASKED_PRM_INFO
        }
    }

    /// `WS-EXPIRY-DAYS`: the two digits of `P-EXPIRY-DAYS` when both are digits, otherwise 5
    /// (CBPAUP0C-004).
    #[must_use]
    pub const fn expiry_days(&self) -> u8 {
        self.expiry_days
    }

    /// `P-CHKP-FREQ` after its default: kept as received unless it is spaces or `0    `, which
    /// become `5    ` (CBPAUP0C-005).
    #[must_use]
    pub const fn chkp_freq(&self) -> [u8; FREQ_LEN] {
        self.chkp_freq
    }

    /// `P-CHKP-DIS-FREQ` after its default: kept as received unless it is spaces or `0    `,
    /// which become `10   ` (CBPAUP0C-006).
    #[must_use]
    pub const fn chkp_dis_freq(&self) -> [u8; FREQ_LEN] {
        self.chkp_dis_freq
    }

    /// `P-DEBUG-FLAG` after its default (CBPAUP0C-007).
    #[must_use]
    pub const fn debug(&self) -> DebugFlag {
        self.debug
    }
}

impl fmt::Debug for PurgeParams {
    /// For the default card and 17 spaces, the received bytes and the derived parameters. For
    /// any other value, every value derived from the received bytes is masked: the received
    /// bytes as 17 `*`, the expiry days as 2 `*` and each frequency as 5 `*`, so the output is
    /// the same whatever the bytes except for the debug flag (CBPAUP0C-003; X-CBPAUP0J-7).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = f.debug_struct("PurgeParams");
        if is_public(&self.raw) {
            out.field("received", &format_args!("\"{}\"", self.raw.escape_ascii()))
                .field("expiry_days", &self.expiry_days)
                .field(
                    "chkp_freq",
                    &format_args!("\"{}\"", self.chkp_freq.escape_ascii()),
                )
                .field(
                    "chkp_dis_freq",
                    &format_args!("\"{}\"", self.chkp_dis_freq.escape_ascii()),
                );
        } else {
            out.field(
                "received",
                &format_args!("\"{}\"", MASKED_PRM_INFO.escape_ascii()),
            )
            .field(
                "expiry_days",
                &format_args!("\"{}\"", MASKED_EXPIRY_DAYS.escape_ascii()),
            )
            .field(
                "chkp_freq",
                &format_args!("\"{}\"", MASKED_FREQ.escape_ascii()),
            )
            .field(
                "chkp_dis_freq",
                &format_args!("\"{}\"", MASKED_FREQ.escape_ascii()),
            );
        }
        out.field("debug", &self.debug).finish()
    }
}

/// Whether `PARM RECEIVED` and `Debug` may show a received `PRM-INFO` as received: only when it
/// is [`DEFAULT_PRM_INFO`], the published default card, or [`BLANK_PRM_INFO`], which no card
/// number, SSN or accepted password is. Any other value, whatever its bytes, can be or hold a
/// protected value, since a password is any 1 to 8 printable characters (Cardholder Data
/// Redaction; CBPAUP0C-003, X-CBPAUP0J-7).
fn is_public(prm_info: &[u8; PRM_INFO_LEN]) -> bool {
    *prm_info == DEFAULT_PRM_INFO || *prm_info == BLANK_PRM_INFO
}

/// The first 17 bytes of `text` padded with spaces to 17, as [`PurgeParams::parse`] keeps them:
/// [`DEFAULT_PRM_INFO`] at compile time (CBPAUP0C-002).
const fn prm_info_of(text: &[u8]) -> [u8; PRM_INFO_LEN] {
    let mut prm_info = BLANK_PRM_INFO;
    let mut source = text;
    let mut target: &mut [u8] = &mut prm_info;
    while let ([byte, source_rest @ ..], [slot, target_rest @ ..]) = (source, target) {
        *slot = *byte;
        source = source_rest;
        target = target_rest;
    }
    prm_info
}

/// Whether `byte` is printable ASCII, the bytes a SYSIN card holds (CBPAUP0C-002).
fn is_printable(byte: u8) -> bool {
    (b' '..=b'~').contains(&byte)
}

/// `IF P-EXPIRY-DAYS IS NUMERIC MOVE P-EXPIRY-DAYS TO WS-EXPIRY-DAYS ELSE MOVE 5`: the class
/// test of an unsigned `PIC 9(02)` holds when both bytes are digits (CBPAUP0C-004).
fn expiry_days(field: [u8; 2]) -> u8 {
    match field {
        [tens @ b'0'..=b'9', units @ b'0'..=b'9'] => (tens - b'0') * 10 + (units - b'0'),
        _ => DEFAULT_EXPIRY_DAYS,
    }
}

/// `IF field = SPACES OR 0 OR LOW-VALUES MOVE default TO field` (CBPAUP0C-005, CBPAUP0C-006).
fn frequency_or_default(field: [u8; FREQ_LEN], default: [u8; FREQ_LEN]) -> [u8; FREQ_LEN] {
    if field == SPACES || field == ZERO_TEXT || field == LOW_VALUES {
        default
    } else {
        field
    }
}

/// What the job does with a detail just read (CBPAUP0C-012, CBPAUP0C-015).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailAction {
    /// The detail qualifies: delete its row by its key (`acct_id`, `auth_date_9c`,
    /// `auth_time_9c`) in the open checkpoint interval's transaction.
    Delete,
    /// The detail does not qualify and stays unchanged.
    Keep,
}

/// What the job does with a summary after its last detail (CBPAUP0C-016, CBPAUP0C-017).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryDisposition {
    /// Delete the summary row; `ON DELETE CASCADE` removes every detail still under it, expired
    /// or not, as IMS deletes child segments with their parent (CIPAUDTY-029). Those details
    /// were counted as read and are not counted as deleted.
    Delete,
    /// Keep the summary row exactly as stored: no column is updated (E30).
    Keep,
}

/// Whether a checkpoint falls due after a summary (CBPAUP0C-018).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointAction {
    /// Commit the open transaction, start a new one, then call [`Purge::checkpoint_taken`].
    Take,
    /// Keep the transaction open.
    NotDue,
}

/// The decisions taken after a summary's last detail, in the order the job carries them out:
/// the summary delete, then the checkpoint (`CBPAUP0C.cbl:156-164`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryAction {
    /// The delete test of CBPAUP0C-016.
    pub delete_summary: SummaryDisposition,
    /// The checkpoint test of CBPAUP0C-018.
    pub checkpoint: CheckpointAction,
}

/// The copy of `PENDING-AUTH-SUMMARY` in working storage, which the expiry test of each
/// qualifying detail adjusts and which is never written back (CBPAUP0C-013, CBPAUP0C-014,
/// CBPAUP0C-017).
///
/// Only `approved_auth_cnt` is ever used, by the delete test (CBPAUP0C-016); the other fields
/// follow the COBOL statements and are discarded with the pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkingSummary {
    /// `PA-ACCT-ID PIC S9(11) COMP-3` (CIPAUSMY-001).
    pub acct_id: PicS<11>,
    /// `PA-APPROVED-AUTH-CNT PIC S9(04) COMP` (CIPAUSMY-009).
    pub approved_auth_cnt: Count,
    /// `PA-DECLINED-AUTH-CNT PIC S9(04) COMP` (CIPAUSMY-010).
    pub declined_auth_cnt: Count,
    /// `PA-APPROVED-AUTH-AMT PIC S9(09)V99 COMP-3` (CIPAUSMY-011).
    pub approved_auth_amt: Money<9>,
    /// `PA-DECLINED-AUTH-AMT PIC S9(09)V99 COMP-3` (CIPAUSMY-012).
    pub declined_auth_amt: Money<9>,
}

impl WorkingSummary {
    /// The working copy `GN … INTO (PENDING-AUTH-SUMMARY)` gives (CBPAUP0C-009).
    fn of(summary: &PendingAuthSummaryRecord) -> Self {
        Self {
            acct_id: summary.acct_id,
            approved_auth_cnt: summary.approved_auth_cnt,
            declined_auth_cnt: summary.declined_auth_cnt,
            approved_auth_amt: summary.approved_auth_amt,
            declined_auth_amt: summary.declined_auth_amt,
        }
    }

    /// The working copy after a qualifying detail: response `00` takes 1 from the approved count
    /// and the detail's approved amount from the approved amount, any other response 1 from the
    /// declined count and the transaction amount from the declined amount, each amount stored at
    /// `S9(09)V99` (CBPAUP0C-013, CBPAUP0C-014; ARITH-046 to ARITH-048).
    fn after_expired(self, detail: &PendingAuthDetailRecord) -> Result<Self, PurgeHalt> {
        if detail.is_approved() {
            Ok(Self {
                approved_auth_cnt: count_less_one(self.approved_auth_cnt)?,
                approved_auth_amt: (self.approved_auth_amt - detail.approved_amt).store::<9>(),
                ..self
            })
        } else {
            Ok(Self {
                declined_auth_cnt: count_less_one(self.declined_auth_cnt)?,
                declined_auth_amt: (self.declined_auth_amt - detail.transaction_amt).store::<9>(),
                ..self
            })
        }
    }
}

/// A case the purge cannot decide, because its COBOL value depends on the `TRUNC` compiler
/// option. [`SummaryPass::on_detail`] returns it and changes nothing; the job reports the case
/// blocked and claims no parity for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeHalt {
    /// `WS-DAY-DIFF`: today's `YYDDD` less the authorization's lies beyond ±9999, outside the
    /// four digits of `PIC S9(4) COMP` (CBPAUP0C-012; halt candidate X-CBPAUP0J-H1, with
    /// X-ARITH-6).
    DayDifference,
    /// `SUBTRACT 1` from a count of −9999 (ARITH-046; X-ARITH-6).
    AuthorizationCount,
}

impl fmt::Display for PurgeHalt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DayDifference => "expiry day difference beyond the four digits of WS-DAY-DIFF",
            Self::AuthorizationCount => "authorization count below the four digits of its field",
        })
    }
}

impl std::error::Error for PurgeHalt {}

/// The DL/I command that failed, which decides the failure lines [`Purge::abend`] shows
/// (CBPAUP0C-023 to CBPAUP0C-027).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurgeFailure {
    /// Reading the next summary failed (`GN`, CBPAUP0C-023).
    SummaryRead,
    /// Reading the current summary's next detail failed (`GNP`, CBPAUP0C-024).
    DetailRead,
    /// Deleting a detail [`SummaryPass::on_detail`] chose failed (`DLET`, CBPAUP0C-025).
    DetailDelete,
    /// Deleting a summary [`SummaryPass::end_summary`] chose failed (`DLET`, CBPAUP0C-026).
    SummaryDelete,
    /// The commit of a checkpoint [`SummaryPass::end_summary`] asked for failed (`CHKP`,
    /// CBPAUP0C-027).
    Checkpoint,
    /// The commit at the end of the summaries failed: the final `CHKP`, which follows the read
    /// that found no further summary (CBPAUP0C-019, CBPAUP0C-027).
    FinalCheckpoint,
}

/// One run of CBPAUP0C: its parameters, today's `YYDDD`, its counters and the SYSOUT lines shown
/// so far (`CBPAUP0C.cbl:41-77`).
#[derive(Debug, Clone)]
pub struct Purge {
    /// The parameters after their defaults.
    params: PurgeParams,
    /// `CURRENT-YYDDD PIC 9(05)`, read once (CBPAUP0C-001).
    today: u32,
    /// Every line shown so far, in program order.
    sysout: Vec<String>,
    /// `WS-NO-SUMRY-READ PIC S9(8) COMP`.
    summaries_read: u64,
    /// `WS-NO-SUMRY-DELETED PIC S9(8) COMP`.
    summaries_deleted: u64,
    /// `WS-NO-DTL-READ PIC S9(8) COMP`.
    details_read: u64,
    /// `WS-NO-DTL-DELETED PIC S9(8) COMP`.
    details_deleted: u64,
    /// Checkpoints taken; no COBOL field, as `WS-NO-CHKP` is reset by the display test.
    checkpoints: u64,
    /// `WS-AUTH-SMRY-PROC-CNT PIC 9(8)`: summaries since the last checkpoint.
    proc_cnt: u32,
    /// `WS-NO-CHKP PIC 9(8)`: checkpoints since the last `CHKP SUCCESS` line.
    chkp_cnt: u32,
    /// `PA-ACCT-ID` of the last summary read, which `WS-CURR-APP-ID` copies; `None` before the
    /// first.
    current: Option<PicS<11>>,
}

impl Purge {
    /// Starts a run: reads today's date once from `clock` as `YYDDD` (`ACCEPT CURRENT-YYDDD FROM
    /// DAY`, CBPAUP0C-001) and shows the five opening lines (CBPAUP0C-003). `PARM RECEIVED`
    /// shows `PRM-INFO` as received only when it is the default card's `00,00001,00001,Y ` or an
    /// empty value's 17 spaces, the two values that can never be a protected value, and
    /// otherwise 17 `*`, so the line is 41 characters either way and never shows a card number,
    /// an SSN or a password, which may be any 1 to 8 printable characters, that SYSIN carried
    /// (Cardholder Data Redaction; X-CBPAUP0J-7). The run still uses the parameters as received.
    #[must_use]
    pub fn new(params: PurgeParams, clock: &dyn Clock) -> Self {
        let today = yyddd(clock.now());
        let received: String = params.shown().into_iter().map(char::from).collect();
        let sysout = vec![
            STARTING.to_owned(),
            DASHES.to_owned(),
            format!("{PARM_RECEIVED}{received}"),
            format!("{TODAYS_DATE}{today:05}"),
            BLANK_LINE.to_owned(),
        ];
        Self {
            params,
            today,
            sysout,
            summaries_read: 0,
            summaries_deleted: 0,
            details_read: 0,
            details_deleted: 0,
            checkpoints: 0,
            proc_cnt: 0,
            chkp_cnt: 0,
            current: None,
        }
    }

    /// `CURRENT-YYDDD`: today as the year's last two digits times 1000 plus the day of the year
    /// (CBPAUP0C-001).
    #[must_use]
    pub const fn today(&self) -> u32 {
        self.today
    }

    /// The parameters after their defaults.
    #[must_use]
    pub const fn params(&self) -> &PurgeParams {
        &self.params
    }

    /// The lines shown so far, in program order.
    #[must_use]
    pub fn sysout(&self) -> &[String] {
        &self.sysout
    }

    /// Takes in the next summary, read in ascending `acct_id` (`GN`, status spaces): the debug
    /// line with the count before this read, then `WS-NO-SUMRY-READ` + 1,
    /// `WS-AUTH-SMRY-PROC-CNT` + 1 and `WS-CURR-APP-ID` = `PA-ACCT-ID` (CBPAUP0C-008,
    /// CBPAUP0C-009). The returned pass holds the working copy of the summary.
    pub fn begin_summary(&mut self, summary: &PendingAuthSummaryRecord) -> SummaryPass<'_> {
        self.debug_line(DEBUG_SMRY_READ, &display_count(self.summaries_read));
        self.summaries_read = self.summaries_read.saturating_add(1);
        self.proc_cnt = add_one_pic_9_8(self.proc_cnt);
        self.current = Some(summary.acct_id);
        SummaryPass {
            purge: self,
            working: WorkingSummary::of(summary),
        }
    }

    /// The checkpoint the job has just committed (`CHKP`, status spaces): `WS-NO-CHKP` + 1, then
    /// `IF WS-NO-CHKP >= P-CHKP-DIS-FREQ`, compared as text, resets it and shows the
    /// `CHKP SUCCESS` line with the summaries read and `WS-CURR-APP-ID` (CBPAUP0C-020).
    pub fn checkpoint_taken(&mut self) {
        self.checkpoints = self.checkpoints.saturating_add(1);
        self.chkp_cnt = add_one_pic_9_8(self.chkp_cnt);
        if compare_as_text(self.chkp_cnt, self.params.chkp_dis_freq) != Ordering::Less {
            self.chkp_cnt = 0;
            let line = format!(
                "{CHKP_SUCCESS}{}{APP_ID_SEPARATOR}{}",
                display_count(self.summaries_read),
                self.curr_app_id()
            );
            self.sysout.push(line);
        }
    }

    /// Ends the run normally, once the job has committed the open transaction: the debug line of
    /// the read that found no further summary, the final checkpoint (CBPAUP0C-019,
    /// CBPAUP0C-020) and the eight end lines with the four totals (CBPAUP0C-021); return code 0
    /// (CBPAUP0C-022).
    #[must_use]
    pub fn finish(mut self) -> PurgeReport {
        self.debug_line(DEBUG_SMRY_READ, &display_count(self.summaries_read));
        self.checkpoint_taken();
        let totals = [
            BLANK_LINE.to_owned(),
            DASHES.to_owned(),
            format!("{TOTAL_SUMMARY_READ}{}", display_count(self.summaries_read)),
            format!(
                "{SUMMARY_REC_DELETED}{}",
                display_count(self.summaries_deleted)
            ),
            format!("{TOTAL_DETAILS_READ}{}", display_count(self.details_read)),
            format!(
                "{DETAILS_REC_DELETED}{}",
                display_count(self.details_deleted)
            ),
            DASHES.to_owned(),
            BLANK_LINE.to_owned(),
        ];
        self.sysout.extend(totals);
        PurgeReport {
            sysout: self.sysout,
            summaries_read: self.summaries_read,
            summaries_deleted: self.summaries_deleted,
            details_read: self.details_read,
            details_deleted: self.details_deleted,
            checkpoints: self.checkpoints,
        }
    }

    /// Ends the run at a failed read, delete or commit: the lines `failure` names, then
    /// `CBPAUP0C ABENDING ...`; return code 16 (CBPAUP0C-023 to CBPAUP0C-028). No further read,
    /// delete or checkpoint follows, and no end lines are shown. Every delete of the open
    /// interval that succeeded stands, so the job commits them unless the failure was the commit
    /// itself (CBPAUP0C-028).
    #[must_use]
    pub fn abend(mut self, failure: PurgeFailure) -> AbendReport {
        let read = display_count(self.summaries_read);
        let pa_acct_id = self
            .current
            .map_or_else(|| UNSET_APP_ID.to_owned(), display_pa_acct_id);
        let lines = match failure {
            PurgeFailure::SummaryRead => {
                self.debug_line(DEBUG_SMRY_READ, &read);
                vec![
                    format!("{SUMMARY_READ_FAILED}{UNDEFINED_DIBSTAT}"),
                    format!("{SUMMARY_READ_BEFORE_ABEND}{read}"),
                ]
            }
            PurgeFailure::DetailRead => {
                let details = display_count(self.details_read);
                self.debug_line(DEBUG_DTL_READ, &details);
                vec![
                    format!("{DETAIL_READ_FAILED}{UNDEFINED_DIBSTAT}"),
                    format!("{SUMMARY_AUTH_APP_ID}{pa_acct_id}"),
                    format!("{DETAIL_READ_BEFORE_ABEND}{details}"),
                ]
            }
            PurgeFailure::DetailDelete => vec![
                format!("{DETAIL_DELETE_FAILED}{UNDEFINED_DIBSTAT}"),
                format!("{DETAIL_DELETE_APP_ID}{pa_acct_id}"),
            ],
            PurgeFailure::SummaryDelete => vec![
                format!("{SUMMARY_DELETE_FAILED}{UNDEFINED_DIBSTAT}"),
                format!("{SUMMARY_DELETE_APP_ID}{pa_acct_id}"),
            ],
            PurgeFailure::Checkpoint => vec![self.chkp_failed_line()],
            PurgeFailure::FinalCheckpoint => {
                self.debug_line(DEBUG_SMRY_READ, &read);
                vec![self.chkp_failed_line()]
            }
        };
        self.sysout.extend(lines);
        self.sysout.push(ABENDING.to_owned());
        AbendReport {
            sysout: self.sysout,
        }
    }

    /// Shows `label` + `value` when `DEBUG-ON` (CBPAUP0C-008).
    fn debug_line(&mut self, label: &str, value: &str) {
        if self.params.debug == DebugFlag::On {
            self.sysout.push(format!("{label}{value}"));
        }
    }

    /// `WS-CURR-APP-ID PIC 9(11)` as shown: the `PA-ACCT-ID` it copies, unsigned.
    fn curr_app_id(&self) -> String {
        self.current
            .map_or_else(|| UNSET_APP_ID.to_owned(), display_curr_app_id)
    }

    /// The `CHKP FAILED` line (CBPAUP0C-027).
    fn chkp_failed_line(&self) -> String {
        format!(
            "{CHKP_FAILED}{UNDEFINED_DIBSTAT}{REC_COUNT_SEPARATOR}{}{APP_ID_SEPARATOR}{}",
            display_count(self.summaries_read),
            self.curr_app_id()
        )
    }
}

/// The processing of one summary, from its read to its delete and checkpoint tests: holds the
/// [`WorkingSummary`] its details adjust (`CBPAUP0C.cbl:144-164`).
#[derive(Debug)]
#[must_use = "a summary's delete and checkpoint tests run only in `end_summary`"]
pub struct SummaryPass<'a> {
    /// The run the summary belongs to.
    purge: &'a mut Purge,
    /// The working copy of the summary.
    working: WorkingSummary,
}

impl SummaryPass<'_> {
    /// The working copy of the summary, as the details processed so far left it.
    #[must_use]
    pub const fn working(&self) -> &WorkingSummary {
        &self.working
    }

    /// Takes in the summary's next detail (`GNP`, status spaces): the debug line with the count
    /// before this read and `WS-NO-DTL-READ` + 1 (CBPAUP0C-008, CBPAUP0C-010); then the expiry
    /// test (CBPAUP0C-011, CBPAUP0C-012). A qualifying detail adjusts the working copy
    /// (CBPAUP0C-013, CBPAUP0C-014), shows the `DLET` debug line, is counted as deleted and
    /// answers [`DetailAction::Delete`] (CBPAUP0C-015); any other answers
    /// [`DetailAction::Keep`] and changes nothing.
    ///
    /// # Errors
    ///
    /// [`PurgeHalt::DayDifference`] when the `YYDDD` difference lies beyond ±9999, and
    /// [`PurgeHalt::AuthorizationCount`] when a qualifying detail would take a count below
    /// −9999. The detail is then counted as read and nothing else changes.
    pub fn on_detail(
        &mut self,
        detail: &PendingAuthDetailRecord,
    ) -> Result<DetailAction, PurgeHalt> {
        let purge = &mut *self.purge;
        purge.debug_line(DEBUG_DTL_READ, &display_count(purge.details_read));
        purge.details_read = purge.details_read.saturating_add(1);
        let day_diff = day_difference(purge.today, detail.auth_date_9c)?;
        if day_diff < i16::from(purge.params.expiry_days) {
            return Ok(DetailAction::Keep);
        }
        self.working = self.working.after_expired(detail)?;
        purge.debug_line(DEBUG_DTL_DLET, &display_pa_acct_id(self.working.acct_id));
        purge.details_deleted = purge.details_deleted.saturating_add(1);
        Ok(DetailAction::Delete)
    }

    /// Ends the summary once its details are exhausted (`GNP` status `GE` or `GB`): the debug
    /// line of that last read (CBPAUP0C-008), then the delete test
    /// `IF PA-APPROVED-AUTH-CNT <= 0 AND PA-APPROVED-AUTH-CNT <= 0`, which names the approved
    /// count twice and the declined count never (CBPAUP0C-016), and the checkpoint test
    /// `IF WS-AUTH-SMRY-PROC-CNT > P-CHKP-FREQ`, compared as text, which resets the counter when
    /// it holds (CBPAUP0C-018). A summary to delete shows its `DLET` debug line and is counted
    /// as deleted.
    #[must_use]
    pub fn end_summary(self) -> SummaryAction {
        let Self { purge, working } = self;
        purge.debug_line(DEBUG_DTL_READ, &display_count(purge.details_read));
        let delete_summary = if working.approved_auth_cnt.get() <= 0 {
            purge.debug_line(DEBUG_SMRY_DLET, &display_pa_acct_id(working.acct_id));
            purge.summaries_deleted = purge.summaries_deleted.saturating_add(1);
            SummaryDisposition::Delete
        } else {
            SummaryDisposition::Keep
        };
        let checkpoint =
            if compare_as_text(purge.proc_cnt, purge.params.chkp_freq) == Ordering::Greater {
                purge.proc_cnt = 0;
                CheckpointAction::Take
            } else {
                CheckpointAction::NotDue
            };
        SummaryAction {
            delete_summary,
            checkpoint,
        }
    }
}

/// A run that ended normally (CBPAUP0C-021, CBPAUP0C-022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeReport {
    /// Every line shown, in program order.
    sysout: Vec<String>,
    /// `WS-NO-SUMRY-READ`.
    summaries_read: u64,
    /// `WS-NO-SUMRY-DELETED`.
    summaries_deleted: u64,
    /// `WS-NO-DTL-READ`.
    details_read: u64,
    /// `WS-NO-DTL-DELETED`.
    details_deleted: u64,
    /// Checkpoints taken, the final one included.
    checkpoints: u64,
}

impl PurgeReport {
    /// Every SYSOUT line, in program order.
    #[must_use]
    pub fn sysout(&self) -> &[String] {
        &self.sysout
    }

    /// The summaries read (`# TOTAL SUMMARY READ  :`).
    #[must_use]
    pub const fn summaries_read(&self) -> u64 {
        self.summaries_read
    }

    /// The summaries deleted (`# SUMMARY REC DELETED :`).
    #[must_use]
    pub const fn summaries_deleted(&self) -> u64 {
        self.summaries_deleted
    }

    /// The details read (`# TOTAL DETAILS READ  :`), those removed with their summary included.
    #[must_use]
    pub const fn details_read(&self) -> u64 {
        self.details_read
    }

    /// The details deleted one by one (`# DETAILS REC DELETED :`), not those removed with their
    /// summary.
    #[must_use]
    pub const fn details_deleted(&self) -> u64 {
        self.details_deleted
    }

    /// The checkpoints taken, the final one included: the commits of the run.
    #[must_use]
    pub const fn checkpoints(&self) -> u64 {
        self.checkpoints
    }

    /// The return code: 0, as `RETURN-CODE` is never set on a normal end (CBPAUP0C-022).
    #[must_use]
    pub const fn return_code(&self) -> u8 {
        NORMAL_RETURN_CODE
    }
}

/// A run that ended in `9999-ABEND` (CBPAUP0C-028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbendReport {
    /// Every line shown, in program order, `CBPAUP0C ABENDING ...` the last.
    sysout: Vec<String>,
}

impl AbendReport {
    /// Every SYSOUT line, in program order.
    #[must_use]
    pub fn sysout(&self) -> &[String] {
        &self.sysout
    }

    /// The return code: [`ABEND_RETURN_CODE`], 16 (CBPAUP0C-028).
    #[must_use]
    pub const fn return_code(&self) -> u8 {
        ABEND_RETURN_CODE
    }
}

/// `COMPUTE WS-AUTH-DATE = 99999 - PA-AUTH-DATE-9C`: the authorization's `YYDDD` again for the
/// keys CP00 writes; key 75634 gives 24365 (CBPAUP0C-011).
fn auth_date(key: PicS<5>) -> i64 {
    // TRUNCATION: the unsigned PIC 9(05) receiver keeps the low-order five digits of
    // 99999 - key, which lies between 0 and 199 998 for every PicS<5> key, so a negative key,
    // which no in-scope writer produces, keeps the low five digits of 100 000 or more
    // (CBPAUP0C-011); rem_euclid drops exactly those digits.
    (AUTH_DATE_NINES - key.get()).rem_euclid(PIC_9_5_MODULUS)
}

/// `COMPUTE WS-DAY-DIFF = CURRENT-YYDDD - WS-AUTH-DATE`, a subtraction of `YYDDD` numbers, not a
/// count of days (CBPAUP0C-012; X-ARITH-4).
fn day_difference(today: u32, key: PicS<5>) -> Result<i16, PurgeHalt> {
    let difference = i64::from(today) - auth_date(key);
    // TRUNCATION: the PIC S9(4) COMP receiver gets a difference between -99 999 and 99 366. The
    // checked conversion is exact from -9999 to 9999; beyond, the stored value depends on the
    // TRUNC compiler option and has no single result (CBPAUP0C-012; halt candidate
    // X-CBPAUP0J-H1, X-ARITH-6), so no value is produced.
    i16::try_from(difference)
        .ok()
        .filter(|diff| (-DAY_DIFF_MAX..=DAY_DIFF_MAX).contains(diff))
        .ok_or(PurgeHalt::DayDifference)
}

/// `SUBTRACT 1 FROM` a `PIC S9(04) COMP` count, which has no single result below −9999
/// (ARITH-046; X-ARITH-6).
fn count_less_one(count: Count) -> Result<Count, PurgeHalt> {
    count.sub_one().map_err(|_| PurgeHalt::AuthorizationCount)
}

/// `ADD 1 TO` a `PIC 9(8)` counter (`WS-AUTH-SMRY-PROC-CNT`, `WS-NO-CHKP`).
fn add_one_pic_9_8(value: u32) -> u32 {
    // TRUNCATION: an ADD with no ON SIZE ERROR keeps the low-order eight digits of an unsigned
    // DISPLAY receiver, the store rule of AAP 0.3.3.2, so 99 999 999 + 1 stores 0 (CBPAUP0C-009,
    // CBPAUP0C-020). The counter is below 10^8, so the addition never saturates.
    value.saturating_add(1) % PIC_9_8_MODULUS
}

/// A `PIC 9(8)` counter compared with a `PIC X(05)` parameter, as COBOL compares a numeric
/// integer with an alphanumeric item: the counter's eight digits against the parameter padded
/// with spaces to eight characters, character by character in the EBCDIC collating sequence
/// (CBPAUP0C-018, CBPAUP0C-020; X-CBPAUP0J-3). `00001000` > `00001   `, while `00000002` <
/// `00001   `. Equality never holds, as the padded parameter ends in spaces.
fn compare_as_text(counter: u32, parameter: [u8; FREQ_LEN]) -> Ordering {
    let digits = format!("{counter:08}");
    let padded = parameter.into_iter().chain(COUNTER_PAD);
    digits.bytes().map(ebcdic_rank).cmp(padded.map(ebcdic_rank))
}

/// The place of `byte` against the digits in the EBCDIC collating sequence. EBCDIC puts the ten
/// digits at 0xF0 to 0xF9, above the space, every letter and every other printable character, in
/// code pages 037 and 1047 alike. Ranking each digit by its value and every other byte below
/// them therefore gives the EBCDIC result of any comparison whose one operand is all digits, as
/// the counter's text always is. In ASCII order letters would sort above the digits instead.
fn ebcdic_rank(byte: u8) -> u8 {
    if byte.is_ascii_digit() {
        byte - b'0' + 1
    } else {
        0
    }
}

/// A `PIC S9(8) COMP` count as `DISPLAY` shows it: eight digits with leading zeros and no sign
/// (CBPAUP0C-008, CBPAUP0C-021; X-CBPAUP0J-6).
fn display_count(count: u64) -> String {
    // TRUNCATION: the count is shown as its low-order eight digits. Up to 99 999 999 that is the
    // whole value; beyond it, the COBOL value depends on the TRUNC option, as X-ARITH-6 states
    // for halfword counts, and the digits shown are no parity result.
    format!("{:08}", count % COUNT_DISPLAY_MODULUS)
}

/// `PA-ACCT-ID PIC S9(11) COMP-3` as `DISPLAY` shows it: eleven digits with leading zeros, a
/// non-negative key with no sign character, a negative one with the low-order sign overpunch
/// (CBPAUP0C-008, CBPAUP0C-024 to CBPAUP0C-026; X-CBPAUP0J-6).
fn display_pa_acct_id(acct_id: PicS<11>) -> String {
    let value = acct_id.get();
    if value >= 0 {
        return format!("{value:011}");
    }
    let magnitude = value.unsigned_abs();
    let units = magnitude % 10;
    let zone = NEGATIVE_OVERPUNCH
        .chars()
        .zip(0_u64..)
        .find_map(|(zone, digit)| (digit == units).then_some(zone))
        .unwrap_or('}');
    format!("{:010}{zone}", magnitude / 10)
}

/// `WS-CURR-APP-ID PIC 9(11)` after `MOVE PA-ACCT-ID TO WS-CURR-APP-ID`: an unsigned receiver
/// keeps the key's magnitude, shown as eleven digits (CBPAUP0C-009, CBPAUP0C-020).
fn display_curr_app_id(acct_id: PicS<11>) -> String {
    format!("{:011}", acct_id.get().unsigned_abs())
}

#[cfg(test)]
mod tests {
    use core::cmp::Ordering;
    use std::error::Error;

    use chrono::NaiveDate;
    use copybook::{Count, FixedRecord, PicS, PicU, PicX};
    use proptest::prelude::*;
    use proptest::strategy::Union;

    use super::{
        ABEND_RETURN_CODE, CheckpointAction, DEFAULT_SYSIN, DebugFlag, DetailAction,
        NORMAL_RETURN_CODE, PRM_INFO_LEN, Purge, PurgeFailure, PurgeHalt, PurgeParams,
        SYSIN_RECORD_LEN, SummaryDisposition, SysinError, add_one_pic_9_8, auth_date,
        compare_as_text, day_difference, display_count, display_curr_app_id, display_pa_acct_id,
    };
    use crate::clock::FixedClock;
    use crate::money::{Decimal, Money};
    use crate::records::{PendingAuthDetailRecord, PendingAuthSummaryRecord};

    type TestResult = Result<(), Box<dyn Error>>;

    /// A clock stopped at noon on the given day.
    fn clock_on(year: i32, month: u32, day: u32) -> Result<FixedClock, Box<dyn Error>> {
        let at = NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_opt(12, 0, 0))
            .ok_or("no such date")?;
        Ok(FixedClock::new(at))
    }

    /// 2025-02-09, `YYDDD` 25040.
    fn february_9() -> Result<FixedClock, Box<dyn Error>> {
        clock_on(2025, 2, 9)
    }

    fn money<const N: u32>(cents: i64) -> Result<Money<N>, Box<dyn Error>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    fn purge_with(sysin: &str) -> Result<Purge, Box<dyn Error>> {
        Ok(Purge::new(
            PurgeParams::parse(sysin.as_bytes())?,
            &february_9()?,
        ))
    }

    /// A stored summary with the given counts, approved amount 300.00 and declined amount 250.00.
    fn summary(
        acct_id: i64,
        approved: i16,
        declined: i16,
    ) -> Result<PendingAuthSummaryRecord, Box<dyn Error>> {
        let mut record = PendingAuthSummaryRecord::initialized(PicS::new(acct_id)?, PicU::new(9)?);
        record.approved_auth_cnt = Count::new(approved);
        record.declined_auth_cnt = Count::new(declined);
        record.approved_auth_amt = money(30_000)?;
        record.declined_auth_amt = money(25_000)?;
        Ok(record)
    }

    /// The 200-byte `PAUTDTL1` segment of a detail whose keys, counts and amounts are zero and
    /// whose text fields, the card number among them, are blank (CIPAUDTY-001 to CIPAUDTY-028).
    fn blank_detail_segment() -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PendingAuthDetailRecord::WIDTH);
        bytes.extend([0x00, 0x00, 0x0C]); // PA-AUTH-DATE-9C, packed
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x0C]); // PA-AUTH-TIME-9C, packed
        bytes.extend([b' '; 60]); // PA-AUTH-ORIG-DATE to PA-AUTH-RESP-REASON
        bytes.extend(*b"000000"); // PA-PROCESSING-CODE
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C]); // PA-TRANSACTION-AMT, packed
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C]); // PA-APPROVED-AMT, packed
        bytes.extend([b' '; 7]); // PA-MERCHANT-CATAGORY-CODE, PA-ACQR-COUNTRY-CODE
        bytes.extend(*b"00"); // PA-POS-ENTRY-MODE
        bytes.extend([b' '; 103]); // PA-MERCHANT-ID to FILLER
        bytes
    }

    /// A detail authorized on `yyddd`, with the key CP00 writes (`99999 − YYDDD`), response
    /// `resp` and the given amounts in cents. Its card number is blank: no case needs one.
    fn detail(
        yyddd: i64,
        resp: [u8; 2],
        transaction_cents: i64,
        approved_cents: i64,
    ) -> Result<PendingAuthDetailRecord, Box<dyn Error>> {
        let blank = PendingAuthDetailRecord::decode(&blank_detail_segment())?;
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::new(99_999 - yyddd)?,
            auth_time_9c: PicS::new(764_040_877)?,
            auth_resp_code: PicX::from_array(resp),
            transaction_amt: money(transaction_cents)?,
            approved_amt: money(approved_cents)?,
            ..blank
        })
    }

    // CBPAUP0C-002, CBPAUP0C-004 to CBPAUP0C-007: the default card of CBPAUP0J.jcl:37.
    #[test]
    fn default_card_gives_the_audited_parameters() -> TestResult {
        let params = PurgeParams::parse(DEFAULT_SYSIN.as_bytes())?;
        assert_eq!(params.raw(), b"00,00001,00001,Y ");
        assert_eq!(params.expiry_days(), 0);
        assert_eq!(params.chkp_freq(), *b"00001");
        assert_eq!(params.chkp_dis_freq(), *b"00001");
        assert_eq!(params.debug(), DebugFlag::On);
        Ok(())
    }

    /// A SYSIN value with its expiry, frequency, display frequency and debug flag.
    type DefaultsCase<'a> = (&'a [u8], u8, &'a [u8; 5], &'a [u8; 5], DebugFlag);

    // CBPAUP0C-004 to CBPAUP0C-007: each default replaces only the values the COBOL tests for.
    #[test]
    fn defaults_replace_only_the_tested_values() -> TestResult {
        let cases: [DefaultsCase<'_>; 8] = [
            (b"", 5, b"5    ", b"10   ", DebugFlag::Off),
            (b"                 ", 5, b"5    ", b"10   ", DebugFlag::Off),
            (b" 5,0    ,0    ,y", 5, b"5    ", b"10   ", DebugFlag::Off),
            (b"5 ,00000,0000 ,N", 5, b"00000", b"0000 ", DebugFlag::Off),
            (b"AB,12345,ABCDE,Y", 5, b"12345", b"ABCDE", DebugFlag::On),
            (b"07", 7, b"5    ", b"10   ", DebugFlag::Off),
            (b"99,00010,00002,Y", 99, b"00010", b"00002", DebugFlag::On),
            (b"12;     ;     ;Y", 12, b"5    ", b"10   ", DebugFlag::On),
        ];
        for (sysin, expiry, chkp, dis, debug) in cases {
            let params = PurgeParams::parse(sysin)?;
            let context = String::from_utf8_lossy(sysin);
            assert_eq!(params.expiry_days(), expiry, "expiry of {context:?}");
            assert_eq!(&params.chkp_freq(), chkp, "frequency of {context:?}");
            assert_eq!(
                &params.chkp_dis_freq(),
                dis,
                "display frequency of {context:?}"
            );
            assert_eq!(params.debug(), debug, "debug flag of {context:?}");
        }
        Ok(())
    }

    // CBPAUP0C-002: bytes 18 to 80 of the card never reach PRM-INFO; a short value is padded.
    #[test]
    fn prm_info_is_the_first_17_bytes_of_the_padded_card() -> TestResult {
        let card = format!(
            "{DEFAULT_SYSIN}{}",
            "X".repeat(SYSIN_RECORD_LEN - DEFAULT_SYSIN.len())
        );
        assert_eq!(
            PurgeParams::parse(card.as_bytes())?.raw(),
            b"00,00001,00001,YX"
        );
        assert_eq!(PurgeParams::parse(b"05")?.raw(), b"05               ");
        Ok(())
    }

    // CBPAUP0C-002: a value no 80-byte card can hold is a typed error.
    #[test]
    fn values_no_card_holds_are_typed_errors() {
        let long = vec![b'0'; SYSIN_RECORD_LEN + 1];
        let cases: [(&[u8], SysinError); 5] = [
            (&long, SysinError::TooLong { len: 81 }),
            (b"00,00001\n", SysinError::NotPrintable { offset: 8 }),
            (b"00\t", SysinError::NotPrintable { offset: 2 }),
            (b"\x7f", SysinError::NotPrintable { offset: 0 }),
            ("0é".as_bytes(), SysinError::NotPrintable { offset: 1 }),
        ];
        for (sysin, error) in cases {
            assert_eq!(PurgeParams::parse(sysin), Err(error));
        }
        assert_eq!(
            SysinError::TooLong { len: 81 }.to_string(),
            "SYSIN record is 81 bytes, longer than the 80-byte card"
        );
        assert_eq!(
            SysinError::NotPrintable { offset: 8 }.to_string(),
            "SYSIN byte 8 is not printable ASCII"
        );
    }

    proptest! {
        // CBPAUP0C-002, Total Parsers Over Untrusted Input: any byte sequence gives a value or a
        // typed error, and the error exactly when no card holds the bytes.
        #[test]
        fn parse_is_total(bytes in proptest::collection::vec(any::<u8>(), 0..=1024)) {
            let card = bytes.len() <= SYSIN_RECORD_LEN
                && bytes.iter().all(|byte| (0x20..=0x7e).contains(byte));
            prop_assert_eq!(PurgeParams::parse(&bytes).is_ok(), card);
        }
    }

    // CBPAUP0C-001, CBPAUP0C-003: the opening lines, PRM-INFO before any default.
    #[test]
    fn opening_lines_show_prm_info_as_received_and_today() -> TestResult {
        let purge = purge_with(" ")?;
        assert_eq!(purge.today(), 25_040);
        assert_eq!(purge.params().chkp_freq(), *b"5    ");
        assert_eq!(
            purge.sysout(),
            [
                "STARTING PROGRAM CBPAUP0C::",
                "*-------------------------------------*",
                "CBPAUP0C PARM RECEIVED :                 ",
                "TODAYS DATE            :25040",
                " ",
            ]
        );
        for (date, yyddd) in [((2024, 12, 31), 24_366), ((2025, 1, 1), 25_001)] {
            let (year, month, day) = date;
            let clock = clock_on(year, month, day)?;
            let params = PurgeParams::parse(DEFAULT_SYSIN.as_bytes())?;
            assert_eq!(Purge::new(params, &clock).today(), yyddd);
        }
        Ok(())
    }

    // CBPAUP0C-003, X-CBPAUP0J-7: PARM RECEIVED shows PRM-INFO as received only for the default
    // card and a blank value, decided over the whole value, and otherwise 17 `*`.
    #[test]
    fn parm_received_shows_only_the_default_card_or_a_blank_value() -> TestResult {
        let trailing = format!(
            "{DEFAULT_SYSIN} {}",
            "X".repeat(SYSIN_RECORD_LEN - DEFAULT_SYSIN.len() - 1)
        );
        let cases: [(&[u8], bool); 14] = [
            (DEFAULT_SYSIN.as_bytes(), true),
            (b"", true),
            (b"                 ", true),
            (trailing.as_bytes(), true),
            (b"12;     ;     ;Y", false),
            (b"99,00010,00002,Y", false),
            (b"12,34567", false),
            (b"12!34567", false),
            (b"00,00001,00001,N", false),
            (b"00,00001,00001,YX", false),
            (b"7", false),
            (b"01,00001,00001,Y", false),
            (b"00,00001,00001", false),
            (b"*****************", false),
        ];
        for (sysin, public) in cases {
            let params = PurgeParams::parse(sysin)?;
            let context = String::from_utf8_lossy(sysin);
            let shown = if public { *params.raw() } else { [b'*'; 17] };
            assert_eq!(params.shown(), shown, "shown form of {context:?}");
            let purge = Purge::new(params, &february_9()?);
            let line = format!(
                "CBPAUP0C PARM RECEIVED :{}",
                String::from_utf8_lossy(&shown)
            );
            assert_eq!(purge.sysout().get(2), Some(&line), "line of {context:?}");
            assert_eq!(line.len(), 41);
        }
        Ok(())
    }

    // CBPAUP0C-003, X-CBPAUP0J-7: Debug shows the parameters of the default card and a blank
    // value, and masks every value derived from any other, the debug flag aside.
    #[test]
    fn debug_shows_the_parameters_in_the_shown_form() -> TestResult {
        let cases: [(&[u8], &str); 4] = [
            (
                DEFAULT_SYSIN.as_bytes(),
                concat!(
                    r#"PurgeParams { received: "00,00001,00001,Y ", expiry_days: 0, "#,
                    r#"chkp_freq: "00001", chkp_dis_freq: "00001", debug: On }"#,
                ),
            ),
            (
                b"",
                concat!(
                    r#"PurgeParams { received: "                 ", expiry_days: 5, "#,
                    r#"chkp_freq: "5    ", chkp_dis_freq: "10   ", debug: Off }"#,
                ),
            ),
            (
                b"07\"00000\\0    ,N",
                concat!(
                    r#"PurgeParams { received: "*****************", expiry_days: "**", "#,
                    r#"chkp_freq: "*****", chkp_dis_freq: "*****", debug: Off }"#,
                ),
            ),
            (
                b"12,ABCDE,00002,Y",
                concat!(
                    r#"PurgeParams { received: "*****************", expiry_days: "**", "#,
                    r#"chkp_freq: "*****", chkp_dis_freq: "*****", debug: On }"#,
                ),
            ),
        ];
        for (sysin, debug) in cases {
            let params = PurgeParams::parse(sysin)?;
            assert_eq!(format!("{params:?}"), debug);
        }
        Ok(())
    }

    /// The bytes a password of digits and punctuation, or a near miss of the default card, is
    /// made of: digits, punctuation, a space, `*` and the default card's flag.
    const NEAR_DEFAULT_BYTES: &[u8] = b"0123456789 ,;!.-/\"\\#*Y";

    /// SYSIN values close to the public ones: the default card with up to three of its 17 bytes
    /// replaced and up to 63 more printable bytes after them, and values of 0 to 17 digits and
    /// punctuation alone, as a password or a well-formed custom card is.
    fn near_the_default_card() -> impl Strategy<Value = Vec<u8>> {
        let byte = || proptest::sample::select(NEAR_DEFAULT_BYTES.to_vec());
        Union::new([
            (
                proptest::collection::vec((0..PRM_INFO_LEN, byte()), 0..=3),
                proptest::collection::vec(0x20_u8..=0x7e, 0..=SYSIN_RECORD_LEN - PRM_INFO_LEN),
            )
                .prop_map(|(edits, tail)| {
                    let mut card = b"00,00001,00001,Y ".to_vec();
                    for (offset, value) in edits {
                        if let Some(slot) = card.get_mut(offset) {
                            *slot = value;
                        }
                    }
                    card.extend(tail);
                    card
                })
                .boxed(),
            proptest::collection::vec(byte(), 0..=PRM_INFO_LEN).boxed(),
        ])
    }

    proptest! {
        // CBPAUP0C-003, X-CBPAUP0J-7: whatever a card holds, the shown form is the received bytes
        // only when they are the default card's 17 bytes or 17 spaces, and otherwise 17 `*`; a
        // masked value's Debug is the same constant whatever the bytes except for the debug
        // flag, and its PARM RECEIVED line is the masked line.
        #[test]
        fn the_shown_form_is_a_public_value_or_the_mask(
            bytes in Union::new([
                proptest::collection::vec(0x20_u8..=0x7e, 0..=SYSIN_RECORD_LEN).boxed(),
                near_the_default_card().boxed(),
            ])
        ) {
            let Ok(params) = PurgeParams::parse(&bytes) else {
                return Err(TestCaseError::fail("a printable card was refused"));
            };
            let raw = *params.raw();
            let public = raw == *b"00,00001,00001,Y " || raw == [b' '; 17];
            let shown = params.shown();
            prop_assert_eq!(shown, if public { raw } else { [b'*'; 17] });
            if !public {
                let flag = match params.debug() {
                    DebugFlag::On => "On",
                    DebugFlag::Off => "Off",
                };
                let masked = format!(
                    "PurgeParams {{ received: \"*****************\", expiry_days: \"**\", \
                     chkp_freq: \"*****\", chkp_dis_freq: \"*****\", debug: {flag} }}"
                );
                prop_assert_eq!(format!("{params:?}"), masked);
                let Ok(clock) = february_9() else {
                    return Err(TestCaseError::fail("no clock"));
                };
                let purge = Purge::new(params, &clock);
                prop_assert_eq!(
                    purge.sysout().get(2).map(String::as_str),
                    Some("CBPAUP0C PARM RECEIVED :*****************")
                );
            }
        }
    }

    // CBPAUP0C-018, CBPAUP0C-020: a counter's eight digits against the space-padded parameter,
    // in EBCDIC order, where letters and spaces sort below digits.
    #[test]
    fn counters_compare_with_parameters_as_ebcdic_text() {
        let cases: [(u32, &[u8; 5], Ordering); 12] = [
            (2, b"00001", Ordering::Less),
            (999, b"00001", Ordering::Less),
            (1_000, b"00001", Ordering::Greater),
            (0, b"00000", Ordering::Greater),
            (1, b"00000", Ordering::Greater),
            (49_999_999, b"5    ", Ordering::Less),
            (50_000_000, b"5    ", Ordering::Greater),
            (9_999_999, b"10   ", Ordering::Less),
            (10_000_000, b"10   ", Ordering::Greater),
            (1, b"A    ", Ordering::Greater),
            (1, b" 1   ", Ordering::Greater),
            (99_999_999, b"99999", Ordering::Greater),
        ];
        for (counter, parameter, expected) in cases {
            assert_eq!(
                compare_as_text(counter, *parameter),
                expected,
                "{counter:08} against {:?}",
                String::from_utf8_lossy(parameter)
            );
        }
    }

    // CBPAUP0C-011: 99999 less the key, its low five digits.
    #[test]
    fn auth_date_keeps_five_digits() -> TestResult {
        for (key, expected) in [
            (75_634, 24_365),
            (0, 99_999),
            (99_999, 0),
            (-1, 0),
            (-99_999, 99_998),
        ] {
            assert_eq!(auth_date(PicS::new(key)?), expected, "key {key}");
        }
        Ok(())
    }

    // CBPAUP0C-012: a subtraction of YYDDD numbers, exact within four digits; beyond is halt
    // candidate X-CBPAUP0J-H1.
    #[test]
    fn day_difference_is_a_yyddd_subtraction() -> TestResult {
        let cases = [
            (25_040, 25_035, Ok(5)),
            (25_001, 24_365, Ok(636)),
            (25_040, 25_041, Ok(-1)),
            (1, 10_000, Ok(-9_999)),
            (10_000, 1, Ok(9_999)),
            (1, 10_001, Err(PurgeHalt::DayDifference)),
            (25_001, 15_000, Err(PurgeHalt::DayDifference)),
            (1, 99_365, Err(PurgeHalt::DayDifference)),
        ];
        for (today, authorized, expected) in cases {
            let key = PicS::new(99_999 - authorized)?;
            assert_eq!(
                day_difference(today, key),
                expected,
                "{today} less {authorized}"
            );
        }
        Ok(())
    }

    // CBPAUP0C-012 to CBPAUP0C-015, CBPAUP0C-017; ARITH-046 to ARITH-048: a qualifying detail is
    // deleted and adjusts the working copy only; a recent one is kept.
    #[test]
    fn qualifying_details_adjust_only_the_working_copy() -> TestResult {
        let mut purge = purge_with("05,00001,00001,N")?;
        let stored = summary(42, 2, 1)?;
        let mut pass = purge.begin_summary(&stored);
        assert_eq!(
            pass.on_detail(&detail(25_035, *b"00", 12_000, 10_000)?)?,
            DetailAction::Delete
        );
        assert_eq!(pass.working().approved_auth_cnt, Count::new(1));
        assert_eq!(pass.working().approved_auth_amt, money::<9>(20_000)?);
        assert_eq!(pass.working().declined_auth_cnt, Count::new(1));
        assert_eq!(pass.working().declined_auth_amt, money::<9>(25_000)?);
        let before = *pass.working();
        assert_eq!(
            pass.on_detail(&detail(25_036, *b"00", 500, 500)?)?,
            DetailAction::Keep
        );
        assert_eq!(*pass.working(), before);
        assert_eq!(
            pass.on_detail(&detail(24_365, *b"05", 7_500, 0)?)?,
            DetailAction::Delete
        );
        assert_eq!(pass.working().declined_auth_cnt, Count::new(0));
        assert_eq!(pass.working().declined_auth_amt, money::<9>(17_500)?);
        assert_eq!(pass.working().approved_auth_cnt, Count::new(1));
        assert_eq!(pass.working().acct_id, stored.acct_id);
        let action = pass.end_summary();
        assert_eq!(action.delete_summary, SummaryDisposition::Keep);
        assert_eq!(action.checkpoint, CheckpointAction::NotDue);
        assert_eq!(stored, summary(42, 2, 1)?);
        let report = purge.finish();
        assert_eq!(report.details_read(), 3);
        assert_eq!(report.details_deleted(), 2);
        Ok(())
    }

    // ARITH-047: the store at S9(09)V99 drops the high-order digit and keeps the sign.
    #[test]
    fn approved_amount_store_truncates_high_order_digits() -> TestResult {
        let mut purge = purge_with(DEFAULT_SYSIN)?;
        let mut stored = summary(42, 1, 0)?;
        stored.approved_auth_amt = money(5_000)?;
        let mut pass = purge.begin_summary(&stored);
        let action = pass.on_detail(&detail(25_040, *b"00", 0, 100_000_010_000)?)?;
        assert_eq!(action, DetailAction::Delete);
        assert_eq!(pass.working().approved_auth_amt, money::<9>(-5_000)?);
        Ok(())
    }

    // CBPAUP0C-016: the condition names the approved count twice and the declined count never.
    #[test]
    fn summary_delete_tests_only_the_approved_count() -> TestResult {
        let expired_approval = detail(25_040, *b"00", 100, 100)?;
        let expired_decline = detail(25_040, *b"51", 100, 0)?;
        let cases = [
            (
                (1, 0),
                vec![expired_approval.clone()],
                SummaryDisposition::Delete,
            ),
            ((0, 5), vec![], SummaryDisposition::Delete),
            ((-3, 0), vec![], SummaryDisposition::Delete),
            ((2, 0), vec![expired_approval], SummaryDisposition::Keep),
            ((1, 3), vec![expired_decline], SummaryDisposition::Keep),
        ];
        for ((approved, declined), details, expected) in cases {
            let mut purge = purge_with(DEFAULT_SYSIN)?;
            let stored = summary(42, approved, declined)?;
            let mut pass = purge.begin_summary(&stored);
            for detail in &details {
                assert_eq!(pass.on_detail(detail)?, DetailAction::Delete);
            }
            let action = pass.end_summary();
            assert_eq!(
                action.delete_summary, expected,
                "counts {approved}, {declined}"
            );
        }
        Ok(())
    }

    /// The 1-based numbers of the summaries after which a checkpoint falls due, over `count`
    /// summaries without details.
    fn checkpoint_positions(sysin: &str, count: usize) -> Result<Vec<usize>, Box<dyn Error>> {
        let mut purge = purge_with(sysin)?;
        let stored = summary(42, 1, 0)?;
        let mut positions = Vec::new();
        for position in 1..=count {
            let action = purge.begin_summary(&stored).end_summary();
            if action.checkpoint == CheckpointAction::Take {
                purge.checkpoint_taken();
                positions.push(position);
            }
        }
        Ok(positions)
    }

    // CBPAUP0C-018: `00001` means every 1000 summaries, `00000` every summary, `5    ` 50000000.
    #[test]
    fn checkpoints_fall_due_by_the_text_comparison() -> TestResult {
        assert_eq!(checkpoint_positions(DEFAULT_SYSIN, 2_500)?, [1_000, 2_000]);
        assert_eq!(checkpoint_positions("00,00000,00001,N", 3)?, [1, 2, 3]);
        assert_eq!(checkpoint_positions("", 1_000)?, Vec::<usize>::new());
        Ok(())
    }

    // CBPAUP0C-019, CBPAUP0C-020: the success line follows the display frequency, compared as
    // text, and shows the summaries read and the last account.
    #[test]
    fn checkpoint_success_line_follows_the_display_frequency() -> TestResult {
        let mut purge = purge_with("00,00000,00000,N")?;
        let stored = summary(42, 1, 0)?;
        let action = purge.begin_summary(&stored).end_summary();
        assert_eq!(action.checkpoint, CheckpointAction::Take);
        purge.checkpoint_taken();
        let report = purge.finish();
        let success: Vec<&String> = report
            .sysout()
            .iter()
            .filter(|line| line.starts_with("CHKP SUCCESS"))
            .collect();
        let line = "CHKP SUCCESS: AUTH COUNT - 00000001, APP ID - 00000000042";
        assert_eq!(success, [line, line]);
        assert_eq!(report.checkpoints(), 2);

        let mut purge = purge_with(DEFAULT_SYSIN)?;
        let action = purge.begin_summary(&stored).end_summary();
        assert_eq!(action.checkpoint, CheckpointAction::NotDue);
        let opened = purge.sysout().len();
        for _ in 1..1_000 {
            purge.checkpoint_taken();
        }
        assert_eq!(purge.sysout().len(), opened);
        purge.checkpoint_taken();
        assert_eq!(purge.sysout().get(opened..), Some(&[line.to_owned()][..]));
        Ok(())
    }

    // CBPAUP0C-003, CBPAUP0C-008 to CBPAUP0C-010, CBPAUP0C-015, CBPAUP0C-016, CBPAUP0C-019,
    // CBPAUP0C-021, CBPAUP0C-022: a default run with debug lines, in program order.
    #[test]
    fn default_run_shows_every_line_in_program_order() -> TestResult {
        let mut purge = purge_with(DEFAULT_SYSIN)?;
        let first = summary(42, 1, 0)?;
        let mut pass = purge.begin_summary(&first);
        assert_eq!(
            pass.on_detail(&detail(25_040, *b"00", 100, 100)?)?,
            DetailAction::Delete
        );
        assert_eq!(
            pass.on_detail(&detail(25_041, *b"00", 100, 100)?)?,
            DetailAction::Keep
        );
        assert_eq!(
            pass.end_summary().delete_summary,
            SummaryDisposition::Delete
        );
        let second = summary(77, 2, 0)?;
        let mut pass = purge.begin_summary(&second);
        assert_eq!(
            pass.on_detail(&detail(25_039, *b"00", 100, 100)?)?,
            DetailAction::Delete
        );
        assert_eq!(pass.end_summary().delete_summary, SummaryDisposition::Keep);
        let report = purge.finish();
        assert_eq!(
            report.sysout(),
            [
                "STARTING PROGRAM CBPAUP0C::",
                "*-------------------------------------*",
                "CBPAUP0C PARM RECEIVED :00,00001,00001,Y ",
                "TODAYS DATE            :25040",
                " ",
                "DEBUG: AUTH SMRY READ : 00000000",
                "DEBUG: AUTH DTL READ : 00000000",
                "DEBUG: AUTH DTL DLET : 00000000042",
                "DEBUG: AUTH DTL READ : 00000001",
                "DEBUG: AUTH DTL READ : 00000002",
                "DEBUG: AUTH SMRY DLET : 00000000042",
                "DEBUG: AUTH SMRY READ : 00000001",
                "DEBUG: AUTH DTL READ : 00000002",
                "DEBUG: AUTH DTL DLET : 00000000077",
                "DEBUG: AUTH DTL READ : 00000003",
                "DEBUG: AUTH SMRY READ : 00000002",
                " ",
                "*-------------------------------------*",
                "# TOTAL SUMMARY READ  :00000002",
                "# SUMMARY REC DELETED :00000001",
                "# TOTAL DETAILS READ  :00000003",
                "# DETAILS REC DELETED :00000002",
                "*-------------------------------------*",
                " ",
            ]
        );
        assert_eq!(report.summaries_read(), 2);
        assert_eq!(report.summaries_deleted(), 1);
        assert_eq!(report.checkpoints(), 1);
        assert_eq!(report.return_code(), NORMAL_RETURN_CODE);
        Ok(())
    }

    // CBPAUP0C-007, CBPAUP0C-008: debug off shows the opening and end lines only.
    #[test]
    fn debug_off_shows_no_debug_lines() -> TestResult {
        let mut purge = purge_with("00,00001,00001,N")?;
        let stored = summary(42, 0, 0)?;
        let mut pass = purge.begin_summary(&stored);
        assert_eq!(
            pass.on_detail(&detail(25_040, *b"00", 100, 100)?)?,
            DetailAction::Delete
        );
        assert_eq!(
            pass.end_summary().delete_summary,
            SummaryDisposition::Delete
        );
        let report = purge.finish();
        assert_eq!(report.sysout().len(), 13);
        assert!(
            report
                .sysout()
                .iter()
                .all(|line| !line.starts_with("DEBUG"))
        );
        assert_eq!(
            report.sysout().get(10),
            Some(&"# DETAILS REC DELETED :00000001".to_owned())
        );
        Ok(())
    }

    /// A default run that has read summary 42 and one recent detail of it.
    fn after_one_detail() -> Result<Purge, Box<dyn Error>> {
        let mut purge = purge_with(DEFAULT_SYSIN)?;
        let stored = summary(42, 1, 0)?;
        let mut pass = purge.begin_summary(&stored);
        assert_eq!(
            pass.on_detail(&detail(25_041, *b"00", 100, 100)?)?,
            DetailAction::Keep
        );
        Ok(purge)
    }

    // CBPAUP0C-023 to CBPAUP0C-029: the failure lines of each command, `--` for the DL/I status
    // (HALTED(H4)), then the abend line and return code 16.
    #[test]
    fn abend_lines_name_the_failed_command() -> TestResult {
        let chkp_failed = "CHKP FAILED: DIBSTAT - --, REC COUNT - 00000001, APP ID - 00000000042";
        let cases: [(PurgeFailure, &[&str]); 6] = [
            (
                PurgeFailure::SummaryRead,
                &[
                    "DEBUG: AUTH SMRY READ : 00000001",
                    "AUTH SUMMARY READ FAILED  :--",
                    "SUMMARY READ BEFORE ABEND :00000001",
                ],
            ),
            (
                PurgeFailure::DetailRead,
                &[
                    "DEBUG: AUTH DTL READ : 00000001",
                    "AUTH DETAIL READ FAILED  :--",
                    "SUMMARY AUTH APP ID      :00000000042",
                    "DETAIL READ BEFORE ABEND :00000001",
                ],
            ),
            (
                PurgeFailure::DetailDelete,
                &[
                    "AUTH DETAIL DELETE FAILED :--",
                    "AUTH APP ID               :00000000042",
                ],
            ),
            (
                PurgeFailure::SummaryDelete,
                &[
                    "AUTH SUMMARY DELETE FAILED :--",
                    "AUTH APP ID                :00000000042",
                ],
            ),
            (PurgeFailure::Checkpoint, &[chkp_failed]),
            (
                PurgeFailure::FinalCheckpoint,
                &["DEBUG: AUTH SMRY READ : 00000001", chkp_failed],
            ),
        ];
        for (failure, lines) in cases {
            let purge = after_one_detail()?;
            let before = purge.sysout().len();
            let report = purge.abend(failure);
            let mut expected: Vec<&str> = lines.to_vec();
            expected.push("CBPAUP0C ABENDING ...");
            assert_eq!(
                report.sysout().get(before..).unwrap_or_default(),
                &expected[..],
                "{failure:?}"
            );
            assert_eq!(report.return_code(), ABEND_RETURN_CODE);
        }
        Ok(())
    }

    // CBPAUP0C-027, CBPAUP0C-028: a failed final checkpoint over an empty table shows the
    // never-set WS-CURR-APP-ID as zeros (halt candidate X-CBPAUP0J-H4).
    #[test]
    fn abend_before_any_summary_shows_the_unset_account() -> TestResult {
        let purge = purge_with("00,00001,00001,N")?;
        let report = purge.abend(PurgeFailure::FinalCheckpoint);
        assert_eq!(
            report.sysout().get(5..).unwrap_or_default(),
            [
                "CHKP FAILED: DIBSTAT - --, REC COUNT - 00000000, APP ID - 00000000000",
                "CBPAUP0C ABENDING ...",
            ]
        );
        Ok(())
    }

    // CBPAUP0C-012 (halt candidate X-CBPAUP0J-H1) and ARITH-046 (X-ARITH-6): a case with no
    // single COBOL value is reported and changes nothing but the read count.
    #[test]
    fn halted_cases_change_nothing() -> TestResult {
        let mut purge = Purge::new(
            PurgeParams::parse(DEFAULT_SYSIN.as_bytes())?,
            &clock_on(2025, 1, 1)?,
        );
        let stored = summary(42, -9_999, 0)?;
        let mut pass = purge.begin_summary(&stored);
        let before = *pass.working();
        let ancient = detail(15_000, *b"00", 100, 100)?;
        assert_eq!(pass.on_detail(&ancient), Err(PurgeHalt::DayDifference));
        assert_eq!(
            pass.on_detail(&detail(25_001, *b"00", 100, 100)?),
            Err(PurgeHalt::AuthorizationCount)
        );
        assert_eq!(*pass.working(), before);
        assert_eq!(
            pass.end_summary().delete_summary,
            SummaryDisposition::Delete
        );
        let report = purge.finish();
        assert_eq!((report.details_read(), report.details_deleted()), (2, 0));
        assert!(!PurgeHalt::DayDifference.to_string().is_empty());
        assert!(!PurgeHalt::AuthorizationCount.to_string().is_empty());
        Ok(())
    }

    // X-CBPAUP0J-6, CBPAUP0C-009, CBPAUP0C-021: displayed keys and counts.
    #[test]
    fn values_display_as_cobol_shows_them() -> TestResult {
        let keys = [
            (42, "00000000042"),
            (99_999_999_999, "99999999999"),
            (-12_345, "0000001234N"),
            (-10, "0000000001}"),
        ];
        for (key, shown) in keys {
            assert_eq!(display_pa_acct_id(PicS::new(key)?), shown);
        }
        assert_eq!(display_curr_app_id(PicS::new(-12_345)?), "00000012345");
        assert_eq!(display_count(3), "00000003");
        assert_eq!(display_count(100_000_003), "00000003");
        assert_eq!(add_one_pic_9_8(0), 1);
        assert_eq!(add_one_pic_9_8(99_999_999), 0);
        Ok(())
    }
}
