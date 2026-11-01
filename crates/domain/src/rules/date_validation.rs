//! `CSUTLDTC`: the date check through LE `CEEDAYS` that CT02, CR00 and the
//! CAUP date edits share.
//!
//! `CSUTLDTC` [app/cbl/CSUTLDTC.cbl:L20-157] passes a 10-byte date and a
//! 10-byte picture to `CEEDAYS`, names the 12-byte feedback token in a 15-byte
//! text and returns the 80-byte `WS-MESSAGE` as `LS-RESULT`, with
//! `RETURN-CODE` set to the severity. Its callers read only bytes 1-4 (the
//! severity) and 16-19 (the message number) of that result:
//!
//! - `COTRN02C` [app/cbl/COTRN02C.cbl:L389-427] and `CORPT00C`
//!   [app/cbl/CORPT00C.cbl:L381-426] accept severity `0000` or message number
//!   `2513`: [`CsutldtcResult::accepts_with_2513`].
//! - `CSUTLDPY` `EDIT-DATE-LE` [app/cpy/CSUTLDPY.cpy:L284-321] accepts severity
//!   0 only: [`CsutldtcResult::is_valid`].
//!
//! # Feedback per input class
//!
//! `CEEDAYS` is IBM code, absent from the repository. The feedback each class
//! of input gets is the one the reviewed rows of
//! `docs/parity/parity-oracle-audit.md` pin from the IBM z/OS Language
//! Environment documentation. Where they pin none, `docs/parity/escalations.md`
//! holds the class at `halt` and [`validate`] returns
//! [`DateValidationError::Unproven`] naming that escalation, never a guessed
//! severity or message number:
//!
//! | Picture | Date | Result | Rows |
//! |---|---|---|---|
//! | `YYYY-MM-DD` | a non-digit under `Y`, `M` or `D`, or a byte other than `-` under `-` | 2520 `Nonnumeric data` | P-CSUTLDTC-006 |
//! | `YYYY-MM-DD` | digits, year `0000` | unproven, E-31 | P-CSUTLDTC-009 |
//! | `YYYY-MM-DD` | digits outside 1582-10-15 to 9999-12-31, a calendar date | 2513 `Unsupp. Range` | P-CSUTLDTC-004 |
//! | `YYYY-MM-DD` | digits outside that range, not a calendar date | unproven, E-31 | P-CSUTLDTC-009 |
//! | `YYYY-MM-DD` | digits inside the range, month `00` or above `12` | 2517 `Invalid month` | P-CSUTLDTC-005 |
//! | `YYYY-MM-DD` | digits inside the range, day `00` or beyond the month | 2508 `Datevalue error` | P-CSUTLDTC-003 |
//! | `YYYY-MM-DD` | digits inside the range, a calendar date | 0000 `Date is valid` | P-CSUTLDTC-002 |
//! | `YYYYMMDD` and two bytes | any | unproven, E-26 | P-CSUTLDPY-012 |
//!
//! A calendar date is a valid date of the proleptic Gregorian calendar, the
//! calendar of the Lilian day count: `1500-02-29`, a Julian leap day, is not
//! one, so it falls in the unproven class rather than in 2513. The tokens of
//! P-CSUTLDTC-007 (2507, 2509, 2518, 2521 and any other) are unreachable with a
//! fixed picture holding `Y`, `M` and `D`; [`CeedaysFeedback`] still names them
//! with the texts the `EVALUATE` moves.
//!
//! # The `CSUTLDPY` call
//!
//! `EDIT-DATE-LE` passes the 8-byte `WS-EDIT-DATE-CCYYMMDD` and the 8-byte
//! `WS-DATE-FORMAT` to the 10-byte `LS-DATE` and `LS-DATE-FORMAT`, so
//! `CEEDAYS` reads two bytes of the adjacent storage after each
//! [app/cpy/CSUTLDWY.cpy:L35-37] [app/cpy/CSUTLDWY.cpy:L58-63]: the first two
//! bytes of `WS-EDIT-DATE-BINARY` after the date, which `rules::date_edit`
//! determines, and two spaces after the picture, which
//! [`DatePicture::YyyyMmDd`] holds. E-26 holds every such call at `halt`, so
//! [`validate`] answers it with [`DateValidationError::Unproven`] whatever the
//! ten date bytes are.

use std::str;

use chrono::NaiveDate;

use crate::format::{move_alnum, unsigned_display};
use crate::messages::{
    BAD_PIC_STRING, DATE_IS_INVALID, DATE_IS_VALID, DATEVALUE_ERROR, INSUFFICIENT, INVALID_ERA,
    INVALID_MONTH, MASK_USED, MESG_CODE, NONNUMERIC_DATA, TST_DATE, UNSUPP_RANGE, YEAR_IN_ERA_IS_0,
};

/// The byte length of `LS-DATE PIC X(10)` and of `LS-DATE-FORMAT PIC X(10)`
/// [app/cbl/CSUTLDTC.cbl:L84-85] (P-CSUTLDTC-001).
pub const LS_DATE_LEN: usize = 10;

/// The byte length of `WS-MESSAGE`, moved to `LS-RESULT PIC X(80)`
/// [app/cbl/CSUTLDTC.cbl:L42-57] [app/cbl/CSUTLDTC.cbl:L86] (P-CSUTLDTC-008).
pub const RESULT_LEN: usize = 80;

/// `MOVE LENGTH OF LS-DATE TO VSTRING-LENGTH OF WS-DATE-TO-TEST`: the
/// `PIC S9(4) BINARY` prefix of the date vstring [app/cbl/CSUTLDTC.cbl:L25-31]
/// [app/cbl/CSUTLDTC.cbl:L105-106], equal to [`LS_DATE_LEN`].
const VSTRING_LENGTH: u16 = 10;

/// The digits of `WS-SEVERITY X(04)` and of `WS-MSG-NO X(04)`, each filled by a
/// `MOVE` to its `PIC 9(4)` redefinition [app/cbl/CSUTLDTC.cbl:L43-47].
const CODE_DIGITS: usize = 4;

/// The `SEVERITY` half-word of every non-zero 88-level feedback token: each
/// `VALUE` opens with X'0003' [app/cbl/CSUTLDTC.cbl:L63-70].
const SEVERITY_ERROR: u16 = 3;

/// `WS-DATE-FORMAT PIC X(10) VALUE 'YYYY-MM-DD'` of `COTRN02C`
/// [app/cbl/COTRN02C.cbl:L60] and of `CORPT00C` [app/cbl/CORPT00C.cbl:L72]
/// (P-COTRN02C-015, P-CORPT00C-008).
const PICTURE_DASHED: &[u8; LS_DATE_LEN] = b"YYYY-MM-DD";

/// `CSUTLDWY` `WS-DATE-FORMAT PIC X(08) VALUE 'YYYYMMDD'` read through the
/// 10-byte linkage: its last two bytes are the first two of
/// `WS-DATE-VALIDATION-RESULT`, its `WS-SEVERITY X(04)`, which `INITIALIZE`
/// has just set to spaces [app/cpy/CSUTLDWY.cpy:L58-63]
/// [app/cpy/CSUTLDPY.cpy:L290-296] (P-CSUTLDPY-012).
const PICTURE_CSUTLDPY: &[u8; LS_DATE_LEN] = b"YYYYMMDD  ";

/// The escalation that holds every `CSUTLDPY` call at `halt`: 8-byte arguments
/// read through the 10-byte linkage (P-CSUTLDPY-012).
const E_CSUTLDPY_LINKAGE: &str = "E-26";

/// The escalation that holds at `halt` the dates with year `0000` and the dates
/// both outside the supported range and not calendar dates (P-CSUTLDTC-009).
const E_CEEDAYS_RANGE: &str = "E-31";

/// The first date `CEEDAYS` supports, the first day of the Lilian count
/// (P-CSUTLDTC-002).
const FIRST_SUPPORTED: DigitDate = DigitDate {
    year: 1582,
    month: 10,
    day: 15,
};

/// The last date `CEEDAYS` supports (P-CSUTLDTC-002).
const LAST_SUPPORTED: DigitDate = DigitDate {
    year: 9999,
    month: 12,
    day: 31,
};

/// The picture a caller passes as `LS-DATE-FORMAT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatePicture {
    /// `YYYY-MM-DD`: the `WS-DATE-FORMAT` of `COTRN02C` and `CORPT00C`.
    YyyyMmDdDashed,
    /// `YYYYMMDD` and two spaces: the 8-byte picture of `CSUTLDPY`
    /// `EDIT-DATE-LE` as the 10-byte linkage reads it (E-26).
    YyyyMmDd,
}

impl DatePicture {
    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-001
    ///
    /// The ten bytes `CSUTLDTC` reads as `LS-DATE-FORMAT`, passes to
    /// `CEEDAYS` and shows as `WS-DATE-FMT`.
    #[must_use]
    pub const fn as_bytes(self) -> &'static [u8; LS_DATE_LEN] {
        match self {
            Self::YyyyMmDdDashed => PICTURE_DASHED,
            Self::YyyyMmDd => PICTURE_CSUTLDPY,
        }
    }
}

/// The `CEEDAYS` feedback token: one variant per 88-level condition of
/// `FEEDBACK-TOKEN-VALUE` [app/cbl/CSUTLDTC.cbl:L60-70], plus the
/// `WHEN OTHER` of the `EVALUATE` [app/cbl/CSUTLDTC.cbl:L147-148].
///
/// Each non-zero 88-level value holds severity 3 and its message number in
/// `CASE-1-CONDITION-ID`, X'0003' then X'09CB' for 2507 and so on, so the
/// condition names pin both numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CeedaysFeedback {
    /// `FC-INVALID-DATE`, the all-zero token (`CEE000`): despite its name, the
    /// date is valid — P-CSUTLDTC-002.
    Valid,
    /// `FC-INSUFFICIENT-DATA`, `CEE2EB`, message 2507 — P-CSUTLDTC-003.
    InsufficientData,
    /// `FC-BAD-DATE-VALUE`, `CEE2EC`, message 2508 — P-CSUTLDTC-003.
    BadDateValue,
    /// `FC-INVALID-ERA`, `CEE2ED`, message 2509 — P-CSUTLDTC-007.
    InvalidEra,
    /// `FC-UNSUPP-RANGE`, `CEE2EH`, message 2513 — P-CSUTLDTC-004.
    UnsupportedRange,
    /// `FC-INVALID-MONTH`, `CEE2EL`, message 2517 — P-CSUTLDTC-005.
    InvalidMonth,
    /// `FC-BAD-PIC-STRING`, `CEE2EM`, message 2518 — P-CSUTLDTC-007.
    BadPicString,
    /// `FC-NON-NUMERIC-DATA`, `CEE2EO`, message 2520 — P-CSUTLDTC-006.
    NonNumericData,
    /// `FC-YEAR-IN-ERA-ZERO`, `CEE2EP`, message 2521 — P-CSUTLDTC-007.
    YearInEraZero,
    /// Any other token, with its `SEVERITY` and `MSG-NO` half-words —
    /// P-CSUTLDTC-007.
    Other {
        /// `SEVERITY OF FEEDBACK-CODE`.
        severity: u16,
        /// `MSG-NO OF FEEDBACK-CODE`.
        msg_no: u16,
    },
}

impl CeedaysFeedback {
    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-002 to P-CSUTLDTC-007
    ///
    /// The text the `EVALUATE` moves to `WS-RESULT PIC X(15)`
    /// [app/cbl/CSUTLDTC.cbl:L128-149], as declared; the move pads it to 15
    /// bytes.
    #[must_use]
    pub const fn result_text(self) -> &'static str {
        match self {
            Self::Valid => DATE_IS_VALID,
            Self::InsufficientData => INSUFFICIENT,
            Self::BadDateValue => DATEVALUE_ERROR,
            Self::InvalidEra => INVALID_ERA,
            Self::UnsupportedRange => UNSUPP_RANGE,
            Self::InvalidMonth => INVALID_MONTH,
            Self::BadPicString => BAD_PIC_STRING,
            Self::NonNumericData => NONNUMERIC_DATA,
            Self::YearInEraZero => YEAR_IN_ERA_IS_0,
            Self::Other { .. } => DATE_IS_INVALID,
        }
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-002 to P-CSUTLDTC-007
    ///
    /// `SEVERITY OF FEEDBACK-CODE`, which also becomes `RETURN-CODE`
    /// [app/cbl/CSUTLDTC.cbl:L98] [app/cbl/CSUTLDTC.cbl:L123].
    #[must_use]
    pub const fn severity(self) -> u16 {
        match self {
            Self::Valid => 0,
            Self::Other { severity, .. } => severity,
            Self::InsufficientData
            | Self::BadDateValue
            | Self::InvalidEra
            | Self::UnsupportedRange
            | Self::InvalidMonth
            | Self::BadPicString
            | Self::NonNumericData
            | Self::YearInEraZero => SEVERITY_ERROR,
        }
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-002 to P-CSUTLDTC-007
    ///
    /// `MSG-NO OF FEEDBACK-CODE` [app/cbl/CSUTLDTC.cbl:L124]: the low half-word
    /// of the token's condition ID.
    #[must_use]
    pub const fn msg_no(self) -> u16 {
        match self {
            Self::Valid => 0,
            Self::InsufficientData => 2507,
            Self::BadDateValue => 2508,
            Self::InvalidEra => 2509,
            Self::UnsupportedRange => 2513,
            Self::InvalidMonth => 2517,
            Self::BadPicString => 2518,
            Self::NonNumericData => 2520,
            Self::YearInEraZero => 2521,
            Self::Other { msg_no, .. } => msg_no,
        }
    }
}

/// What `CSUTLDTC` returns for one call: the feedback, and the date and
/// picture it was given, from which [`CsutldtcResult::result_line`] rebuilds
/// the 80-byte `LS-RESULT`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CsutldtcResult {
    feedback: CeedaysFeedback,
    date: [u8; LS_DATE_LEN],
    picture: [u8; LS_DATE_LEN],
}

impl CsutldtcResult {
    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-002 to P-CSUTLDTC-006
    ///
    /// The `CEEDAYS` feedback of the call.
    #[must_use]
    pub const fn feedback(&self) -> CeedaysFeedback {
        self.feedback
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-008
    ///
    /// Bytes 1-4 of the result, `CSUTLDTC-RESULT-SEV-CD` to the callers: the
    /// severity moved to `WS-SEVERITY-N PIC 9(4)`, four digits.
    #[must_use]
    pub fn severity_code(&self) -> String {
        code_text(self.feedback.severity())
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-008
    ///
    /// Bytes 16-19 of the result, `CSUTLDTC-RESULT-MSG-NUM` to the callers: the
    /// message number moved to `WS-MSG-NO-N PIC 9(4)`, four digits.
    #[must_use]
    pub fn msg_num(&self) -> String {
        code_text(self.feedback.msg_no())
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-002, P-CSUTLDPY-012
    ///
    /// Whether the severity is 0: the test of `CSUTLDPY` `EDIT-DATE-LE`,
    /// `IF WS-SEVERITY-N = 0` [app/cpy/CSUTLDPY.cpy:L298].
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.severity_code() == code_text(0)
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-004, P-COTRN02C-015, P-CORPT00C-008
    ///
    /// Whether the severity is `0000` or the message number `2513`: the test
    /// of `COTRN02C` [app/cbl/COTRN02C.cbl:L397-400] and `CORPT00C`
    /// [app/cbl/CORPT00C.cbl:L396-399], which accept a calendar date outside
    /// the supported range.
    #[must_use]
    pub fn accepts_with_2513(&self) -> bool {
        self.is_valid() || self.msg_num() == code_text(CeedaysFeedback::UnsupportedRange.msg_no())
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-008, P-CSUTLDTC-010
    ///
    /// The 80-byte `WS-MESSAGE` moved to `LS-RESULT`
    /// [app/cbl/CSUTLDTC.cbl:L42-57] [app/cbl/CSUTLDTC.cbl:L97], each field
    /// moved to its `PIC X(n)`.
    ///
    /// The `TstDate:` field, bytes 46-55, holds X'000A' and then the first
    /// eight date bytes, not the date: after the call, `MOVE WS-DATE-TO-TEST
    /// TO WS-DATE` [app/cbl/CSUTLDTC.cbl:L122] sends the binary length and the
    /// ten date bytes, 12 bytes, into the 10-byte field (E-76). No caller tests
    /// or shows these bytes. The date bytes are moved as text: should the
    /// eighth byte end inside a multi-byte UTF-8 character, that character
    /// becomes a space, as [`move_alnum`] treats a cut character, so the line
    /// stays valid UTF-8 and exactly [`RESULT_LEN`] bytes.
    #[must_use]
    pub fn result_line(&self) -> String {
        let severity = self.severity_code();
        let msg_no = self.msg_num();
        let tst_date = tst_date(&self.date);
        // Each field and its PIC X width; an empty text is a FILLER VALUE
        // SPACE or SPACES.
        let fields: [(&str, usize); 13] = [
            (&severity, CODE_DIGITS),                  // WS-SEVERITY X(04)
            (MESG_CODE, 11),                           // FILLER X(11)
            (&msg_no, CODE_DIGITS),                    // WS-MSG-NO X(04)
            ("", 1),                                   // FILLER X(01)
            (self.feedback.result_text(), 15),         // WS-RESULT X(15)
            ("", 1),                                   // FILLER X(01)
            (TST_DATE, 9),                             // FILLER X(09)
            (utf8_prefix(&tst_date), LS_DATE_LEN),     // WS-DATE X(10)
            ("", 1),                                   // FILLER X(01)
            (MASK_USED, 10),                           // FILLER X(10)
            (utf8_prefix(&self.picture), LS_DATE_LEN), // WS-DATE-FMT X(10)
            ("", 1),                                   // FILLER X(01)
            ("", 3),                                   // FILLER X(03)
        ];
        fields
            .iter()
            .map(|&(text, width)| move_alnum(text, width))
            .collect()
    }
}

/// Why [`validate`] gives no feedback.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DateValidationError {
    /// No reviewed audit row pins the `CEEDAYS` feedback for the date's input
    /// class, and the named escalation of `docs/parity/escalations.md` holds
    /// it at `halt`. A caller answers with its unexpected-error outcome and
    /// never with a guessed accept or reject.
    #[error("the CEEDAYS feedback for this date is unproven (escalation {escalation_id})")]
    Unproven {
        /// The escalation ID, for example `E-31`.
        escalation_id: &'static str,
    },
}

/// CSUTLDTC A000-MAIN — P-CSUTLDTC-001 to P-CSUTLDTC-006, P-CSUTLDTC-009,
/// P-CSUTLDPY-012
///
/// The result `CSUTLDTC` returns for `date`, its `LS-DATE`, checked against
/// `picture`. Callers pass the ten bytes exactly as their COBOL passes them:
/// `COTRN02C` the `X(10)` screen field after its format edit, `CORPT00C` the
/// date it builds from numeric parts and `-` fillers, and `rules::date_edit`
/// the eight date digits followed by the two bytes the linkage overlays.
///
/// # Errors
///
/// [`DateValidationError::Unproven`] for an input class the audit holds at
/// `halt` (see the module table): every [`DatePicture::YyyyMmDd`] call
/// (E-26), and with [`DatePicture::YyyyMmDdDashed`] a date with year `0000`
/// or a date outside 1582-10-15 to 9999-12-31 that is not a calendar date
/// (E-31).
pub fn validate(
    date: &[u8; LS_DATE_LEN],
    picture: DatePicture,
) -> Result<CsutldtcResult, DateValidationError> {
    let feedback = match picture {
        DatePicture::YyyyMmDdDashed => dashed_feedback(date)?,
        DatePicture::YyyyMmDd => return Err(unproven(E_CSUTLDPY_LINKAGE)),
    };
    Ok(CsutldtcResult {
        feedback,
        date: *date,
        picture: *picture.as_bytes(),
    })
}

/// CSUTLDTC A000-MAIN — P-CSUTLDTC-002 to P-CSUTLDTC-006, P-CSUTLDTC-009
///
/// The `CEEDAYS` feedback for `date` under the picture `YYYY-MM-DD`, by the
/// classes of the module table, tested in its order.
fn dashed_feedback(date: &[u8; LS_DATE_LEN]) -> Result<CeedaysFeedback, DateValidationError> {
    // A field that is not digits has no value to range-check or to test as a
    // month or day, so every date not in digit form is the non-numeric class,
    // whatever digits its other fields hold. P-CSUTLDTC-006 also covers a
    // blank or a separator other than `-`, although IBM documents that
    // CEEDAYS skips leading blanks and treats picture punctuation as
    // placeholders; no carried caller passes such a date, because COTRN02C's
    // format edit rejects it first [app/cbl/COTRN02C.cbl:L353-381] and
    // CORPT00C builds its dates from numeric parts and `-` fillers
    // [app/cbl/CORPT00C.cbl:L60-71].
    let Some(parts) = DigitDate::from_dashed(date) else {
        return Ok(CeedaysFeedback::NonNumericData);
    };
    if parts.year == 0 {
        return Err(unproven(E_CEEDAYS_RANGE));
    }
    let calendar_date = parts.is_calendar_date();
    // A date that is not a calendar date is placed against the range field
    // by field, year then month then day, as its YYYY-MM-DD text compares:
    // 1582-10-00 lies before the range, 1582-13-01 inside it.
    if parts < FIRST_SUPPORTED || parts > LAST_SUPPORTED {
        return if calendar_date {
            Ok(CeedaysFeedback::UnsupportedRange)
        } else {
            Err(unproven(E_CEEDAYS_RANGE))
        };
    }
    // Month before day: a day is measured against its month, so a month 00 or
    // above 12 is the invalid-month class whatever the day. IBM documents no
    // order between the two, and both are rejections for every caller.
    if !(1..=12).contains(&parts.month) {
        return Ok(CeedaysFeedback::InvalidMonth);
    }
    Ok(if calendar_date {
        CeedaysFeedback::Valid
    } else {
        CeedaysFeedback::BadDateValue
    })
}

/// The year, month and day of a date whose `Y`, `M` and `D` positions all hold
/// digits. The derived order compares year, then month, then day.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct DigitDate {
    year: u32,
    month: u32,
    day: u32,
}

impl DigitDate {
    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-006
    ///
    /// The fields of `date` read under the picture `YYYY-MM-DD`; `None` when a
    /// `Y`, `M` or `D` position holds a byte other than an ASCII digit or a `-`
    /// position holds a byte other than `-`.
    fn from_dashed(date: &[u8; LS_DATE_LEN]) -> Option<Self> {
        let mut parts = Self {
            year: 0,
            month: 0,
            day: 0,
        };
        for (&term, &byte) in PICTURE_DASHED.iter().zip(date) {
            let field = match term {
                b'Y' => &mut parts.year,
                b'M' => &mut parts.month,
                b'D' => &mut parts.day,
                separator if byte == separator => continue,
                _ => return None,
            };
            *field = *field * 10 + char::from(byte).to_digit(10)?;
        }
        Some(parts)
    }

    /// CSUTLDTC A000-MAIN — P-CSUTLDTC-002 to P-CSUTLDTC-004, P-CSUTLDTC-009
    ///
    /// Whether the fields form a date of the proleptic Gregorian calendar.
    fn is_calendar_date(self) -> bool {
        i32::try_from(self.year)
            .ok()
            .and_then(|year| NaiveDate::from_ymd_opt(year, self.month, self.day))
            .is_some()
    }
}

/// CSUTLDTC A000-MAIN — P-CSUTLDTC-009, P-CSUTLDPY-012
///
/// The error for an input class `escalation_id` holds at `halt`.
const fn unproven(escalation_id: &'static str) -> DateValidationError {
    DateValidationError::Unproven { escalation_id }
}

/// CSUTLDTC A000-MAIN — P-CSUTLDTC-008
///
/// `value` moved to a `PIC 9(4)` field and shown: its four low-order digits.
fn code_text(value: u16) -> String {
    unsigned_display(u64::from(value), CODE_DIGITS)
}

/// CSUTLDTC A000-MAIN — P-CSUTLDTC-008, P-CSUTLDTC-010
///
/// `WS-DATE` after `MOVE WS-DATE-TO-TEST TO WS-DATE`: the big-endian binary
/// [`VSTRING_LENGTH`], then the date bytes, cut on the right at ten bytes.
fn tst_date(date: &[u8; LS_DATE_LEN]) -> [u8; LS_DATE_LEN] {
    let mut moved = [b' '; LS_DATE_LEN];
    let sent = VSTRING_LENGTH
        .to_be_bytes()
        .into_iter()
        .chain(date.iter().copied());
    for (slot, byte) in moved.iter_mut().zip(sent) {
        *slot = byte;
    }
    moved
}

/// CSUTLDTC A000-MAIN — P-CSUTLDTC-008
///
/// The longest prefix of `bytes` that is valid UTF-8.
fn utf8_prefix(bytes: &[u8]) -> &str {
    str::from_utf8(bytes).unwrap_or_else(|error| {
        bytes
            .get(..error.valid_up_to())
            .and_then(|valid| str::from_utf8(valid).ok())
            .unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        CeedaysFeedback, CsutldtcResult, DatePicture, DateValidationError, LS_DATE_LEN, RESULT_LEN,
        VSTRING_LENGTH, validate,
    };
    use crate::messages::{
        BAD_PIC_STRING, DATE_IS_INVALID, DATE_IS_VALID, DATEVALUE_ERROR, INSUFFICIENT, INVALID_ERA,
        INVALID_MONTH, MASK_USED, MESG_CODE, NONNUMERIC_DATA, TST_DATE, UNSUPP_RANGE,
        YEAR_IN_ERA_IS_0,
    };

    /// The ten bytes of an ASCII `text`, which must be exactly ten bytes long.
    fn date(text: &str) -> [u8; LS_DATE_LEN] {
        text.as_bytes().try_into().expect("a ten-byte date")
    }

    /// The result of `text` under `YYYY-MM-DD`, for a class the audit pins.
    fn checked(text: &str) -> CsutldtcResult {
        validate(&date(text), DatePicture::YyyyMmDdDashed).expect("a pinned input class")
    }

    #[test]
    fn pictures_are_the_ten_bytes_the_callers_pass() {
        // P-CSUTLDTC-001, P-COTRN02C-015, P-CORPT00C-008, P-CSUTLDPY-012
        assert_eq!(DatePicture::YyyyMmDdDashed.as_bytes(), b"YYYY-MM-DD");
        assert_eq!(DatePicture::YyyyMmDd.as_bytes(), b"YYYYMMDD  ");
        assert_eq!(usize::from(VSTRING_LENGTH), LS_DATE_LEN);
    }

    #[test]
    fn feedback_tokens_carry_their_severity_number_and_text() {
        // P-CSUTLDTC-002 to P-CSUTLDTC-007
        let cases = [
            (CeedaysFeedback::Valid, 0, 0, DATE_IS_VALID),
            (CeedaysFeedback::InsufficientData, 3, 2507, INSUFFICIENT),
            (CeedaysFeedback::BadDateValue, 3, 2508, DATEVALUE_ERROR),
            (CeedaysFeedback::InvalidEra, 3, 2509, INVALID_ERA),
            (CeedaysFeedback::UnsupportedRange, 3, 2513, UNSUPP_RANGE),
            (CeedaysFeedback::InvalidMonth, 3, 2517, INVALID_MONTH),
            (CeedaysFeedback::BadPicString, 3, 2518, BAD_PIC_STRING),
            (CeedaysFeedback::NonNumericData, 3, 2520, NONNUMERIC_DATA),
            (CeedaysFeedback::YearInEraZero, 3, 2521, YEAR_IN_ERA_IS_0),
            (
                CeedaysFeedback::Other {
                    severity: 2,
                    msg_no: 2530,
                },
                2,
                2530,
                DATE_IS_INVALID,
            ),
        ];
        for (feedback, severity, msg_no, text) in cases {
            assert_eq!(feedback.severity(), severity, "{feedback:?}");
            assert_eq!(feedback.msg_no(), msg_no, "{feedback:?}");
            assert_eq!(feedback.result_text(), text, "{feedback:?}");
            assert!(text.len() <= 15, "{feedback:?} fits WS-RESULT X(15)");
        }
    }

    #[test]
    fn dashed_dates_get_their_audited_feedback() {
        let cases = [
            // P-CSUTLDTC-002: calendar dates inside 1582-10-15..9999-12-31.
            ("2022-07-18", CeedaysFeedback::Valid),
            ("2024-02-29", CeedaysFeedback::Valid),
            ("2000-02-29", CeedaysFeedback::Valid),
            ("1582-10-15", CeedaysFeedback::Valid),
            ("9999-12-31", CeedaysFeedback::Valid),
            // P-CSUTLDTC-003: day 00 or beyond the month, inside the range.
            ("2023-02-29", CeedaysFeedback::BadDateValue),
            ("1900-02-29", CeedaysFeedback::BadDateValue),
            ("2022-02-30", CeedaysFeedback::BadDateValue),
            ("2022-04-31", CeedaysFeedback::BadDateValue),
            ("2022-07-00", CeedaysFeedback::BadDateValue),
            ("2022-07-32", CeedaysFeedback::BadDateValue),
            ("1582-11-31", CeedaysFeedback::BadDateValue),
            ("9999-11-31", CeedaysFeedback::BadDateValue),
            // P-CSUTLDTC-004: calendar dates outside the range, year 0001 on.
            ("1582-10-14", CeedaysFeedback::UnsupportedRange),
            ("1500-01-01", CeedaysFeedback::UnsupportedRange),
            ("0001-01-01", CeedaysFeedback::UnsupportedRange),
            ("0022-07-18", CeedaysFeedback::UnsupportedRange),
            // P-CSUTLDTC-005: month 00 or above 12, whatever the day.
            ("2022-13-01", CeedaysFeedback::InvalidMonth),
            ("2022-00-15", CeedaysFeedback::InvalidMonth),
            ("2022-13-32", CeedaysFeedback::InvalidMonth),
            ("2022-00-00", CeedaysFeedback::InvalidMonth),
            ("1582-13-01", CeedaysFeedback::InvalidMonth),
            // P-CSUTLDTC-006: a non-digit under Y, M or D, or a separator
            // other than '-'.
            ("2022-0A-18", CeedaysFeedback::NonNumericData),
            ("20A2-07-18", CeedaysFeedback::NonNumericData),
            ("2022-07-1 ", CeedaysFeedback::NonNumericData),
            ("2022/07/18", CeedaysFeedback::NonNumericData),
            ("20220718  ", CeedaysFeedback::NonNumericData),
            ("          ", CeedaysFeedback::NonNumericData),
            ("0000-AB-01", CeedaysFeedback::NonNumericData),
        ];
        for (text, feedback) in cases {
            assert_eq!(checked(text).feedback(), feedback, "{text}");
        }
        let low_values = [0_u8; LS_DATE_LEN];
        let result = validate(&low_values, DatePicture::YyyyMmDdDashed).expect("a pinned class");
        assert_eq!(result.feedback(), CeedaysFeedback::NonNumericData);
    }

    #[test]
    fn callers_read_the_severity_and_message_number_as_four_digits() {
        // P-CSUTLDTC-008; COTRN02C and CORPT00C accept 0000 or 2513
        // (P-COTRN02C-015, P-CORPT00C-008), CSUTLDPY severity 0 only
        // (P-CSUTLDPY-012).
        let cases = [
            ("2022-07-18", "0000", "0000", true, true),
            ("1500-01-01", "0003", "2513", false, true),
            ("2022-02-30", "0003", "2508", false, false),
            ("2022-13-01", "0003", "2517", false, false),
            ("2022-0A-18", "0003", "2520", false, false),
        ];
        for (text, severity, msg_no, valid, accepted) in cases {
            let result = checked(text);
            assert_eq!(result.severity_code(), severity, "{text}");
            assert_eq!(result.msg_num(), msg_no, "{text}");
            assert_eq!(result.is_valid(), valid, "{text}");
            assert_eq!(result.accepts_with_2513(), accepted, "{text}");
        }
    }

    #[test]
    fn other_tokens_show_the_low_order_digits_of_their_numbers() {
        // P-CSUTLDTC-007, P-CSUTLDTC-008: MOVE to PIC 9(4) keeps four digits.
        let result = CsutldtcResult {
            feedback: CeedaysFeedback::Other {
                severity: 10_003,
                msg_no: 12_513,
            },
            date: date("2022-07-18"),
            picture: *DatePicture::YyyyMmDdDashed.as_bytes(),
        };
        assert_eq!(result.severity_code(), "0003");
        assert_eq!(result.msg_num(), "2513");
        assert!(!result.is_valid());
        assert!(result.accepts_with_2513());
    }

    #[test]
    fn halt_classes_are_unproven_with_their_escalation() {
        // P-CSUTLDTC-009 (E-31): year 0000, and out of range and not a
        // calendar date, a Julian-only leap day included.
        let range_halts = [
            "0000-01-01",
            "0000-13-45",
            "1500-02-30",
            "1500-02-29",
            "1582-10-00",
            "1582-09-31",
            "9999-12-32",
            "9999-13-01",
        ];
        for text in range_halts {
            assert_eq!(
                validate(&date(text), DatePicture::YyyyMmDdDashed),
                Err(DateValidationError::Unproven {
                    escalation_id: "E-31"
                }),
                "{text}"
            );
        }
        // P-CSUTLDPY-012 (E-26): every call with the CSUTLDPY picture, here
        // with the two bytes of WS-EDIT-DATE-BINARY after the date.
        let overlaid = [b'2', b'0', b'2', b'2', b'0', b'7', b'1', b'8', 0x00, 0x00];
        for bytes in [overlaid, date("20220718  ")] {
            assert_eq!(
                validate(&bytes, DatePicture::YyyyMmDd),
                Err(DateValidationError::Unproven {
                    escalation_id: "E-26"
                })
            );
        }
        let error = DateValidationError::Unproven {
            escalation_id: "E-31",
        };
        assert!(error.to_string().contains("E-31"));
    }

    #[test]
    fn result_line_is_the_80_byte_ws_message() {
        // P-CSUTLDTC-008, P-CSUTLDTC-010 (E-76): bytes 46-55 hold X'000A'
        // and the first eight date bytes.
        let cases = [
            ("2022-07-18", "0000", "0000", DATE_IS_VALID),
            ("2022-13-01", "0003", "2517", INVALID_MONTH),
        ];
        for (text, severity, msg_no, result_text) in cases {
            let date_head = text.get(..8).expect("an ASCII date");
            let expected = format!(
                "{severity}{MESG_CODE:<11}{msg_no} {result_text:<15} {TST_DATE:<9}\0\n{date_head} {MASK_USED:<10}YYYY-MM-DD    "
            );
            let line = checked(text).result_line();
            assert_eq!(line.len(), RESULT_LEN, "{text}");
            assert_eq!(line, expected, "{text}");
            assert_eq!(line.get(..4), Some(severity), "bytes 1-4, {text}");
            assert_eq!(line.get(15..19), Some(msg_no), "bytes 16-19, {text}");
            assert_eq!(
                line.as_bytes().get(45..55),
                Some(&[&[0x00, 0x0A], &text.as_bytes()[..8]].concat()[..]),
                "bytes 46-55, {text}"
            );
        }
    }

    #[test]
    fn result_line_keeps_its_width_when_a_character_is_cut() {
        // P-CSUTLDTC-008: the eighth date byte opens a two-byte character,
        // which the ten-byte TstDate field cannot hold whole.
        let mut bytes = date("2022-07---");
        bytes[7] = 0xC3;
        bytes[8] = 0xA9;
        let result = validate(&bytes, DatePicture::YyyyMmDdDashed).expect("a pinned class");
        assert_eq!(result.feedback(), CeedaysFeedback::NonNumericData);
        let line = result.result_line();
        assert_eq!(line.len(), RESULT_LEN);
        assert_eq!(line.get(45..55), Some("\0\n2022-07 "));
    }
}
