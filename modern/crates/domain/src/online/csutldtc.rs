//! CSUTLDTC date validity, and the field edits that only COACTUPC performs (AAP 0.6.1).
//!
//! Two rule sets live here because both come from the date copybooks `CSUTLDWY` and `CSUTLDPY`
//! and the lookup copybook `CSLKPCDY`:
//!
//! - **Date validity**, the `CEEDAYS` wrapper `CSUTLDTC` (CSUTLDTC-001 to CSUTLDTC-018).
//!   [`validate`] returns an [`LeResult`], the model of the 80-byte `LS-RESULT`. COTRN02C and
//!   CORPT00C read it with [`caller_accepts`] (CSUTLDTC-028 to CSUTLDTC-031); COACTUPC reads it
//!   through `EDIT-DATE-LE`, which is [`edit_date_le`] (CSUTLDTC-019 to CSUTLDTC-026).
//! - **COACTUPC's edits.** The `CSUTLDPY` date-part edits [`edit_date_ccyymmdd`] and
//!   [`edit_date_of_birth`] (COACTUPC-117 to COACTUPC-131), and from COACTUPC itself
//!   [`edit_num_reqd`] (COACTUPC-053), [`edit_us_phone`] (COACTUPC-055 to COACTUPC-059),
//!   [`edit_us_ssn`] (COACTUPC-060 to COACTUPC-063), [`edit_us_state`] (COACTUPC-065) and
//!   [`edit_us_state_zip`] (COACTUPC-067), with the three `CSLKPCDY` tables they test. Only
//!   `domain::online::coactupc` calls them.
//!
//! # Halt H5
//!
//! Which date text `CEEDAYS` accepts, and which feedback token it returns, Language Environment
//! defines outside the repository (CSUTLDTC-006). [`validate`] stands in for it with a calendar
//! check so that the flows around it can run. That verdict is `HALTED(H5)`: it is no parity
//! result, and every test over it is reported `BLOCKED`. What the COBOL fixes once a token is
//! known stays exact and testable with a substituted verdict ([`LeResult::new`],
//! [`edit_date_ccyymmdd_with`]): the 88-level decoding and result texts, the result layout,
//! the callers' acceptance test and `EDIT-DATE-LE`'s message.
//!
//! # Edit semantics
//!
//! - A field enters as the raw request text of its map field (the raw-edit-input exception of
//!   AAP 0.3.4.1) and is read as a COBOL `MOVE` into its `PIC X(n)` receiver: the first `n`
//!   characters, padded with spaces.
//! - A field is blank when it is all spaces or all low-values (`\0`). `1100-RECEIVE-MAP` has
//!   already turned a blank field, or `*` followed by spaces, into low-values (COACTUPC-007);
//!   that `*` rule belongs to the caller.
//! - A failure is an [`EditFailure`]: `TRIM(WS-EDIT-VARIABLE-NAME)` followed by the audited
//!   literal, and the [`Part`] the response names. COACTUPC sets a message only while
//!   `WS-RETURN-MSG` is blank, so within an edit the first failing check is returned, and the
//!   caller keeps the first failure of its edit sequence (COACTUPC-013).
//! - No message holds a field value. SSN digits leave only as the [`PicU`] that [`edit_us_ssn`]
//!   returns, which the caller wraps in `crate::sensitive::Ssn` (Cardholder Data Redaction).
//! - A date leaves only as a typed [`NaiveDate`] (Typed Dates and Timestamps).
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a
//! value or an [`EditFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input). Digits are read with `char::to_digit` and checked arithmetic, never with
//! an integer cast (Checked Integer Narrowing).

use core::fmt;
use std::iter;

use chrono::NaiveDate;
use copybook::PicU;

use crate::clock::Clock;

/// Width of `LS-DATE PIC X(10)`: CSUTLDTC reads exactly ten characters (CSUTLDTC-001).
const LS_DATE_WIDTH: usize = 10;

/// Width of `LS-DATE-FORMAT PIC X(10)` (CSUTLDTC-001, CSUTLDTC-020).
const LS_DATE_FORMAT_WIDTH: usize = 10;

/// Length of `LS-RESULT PIC X(80)` (CSUTLDTC-003).
pub const LS_RESULT_LEN: usize = 80;

/// Bytes of the tested date that reach result positions 48-55 (CSUTLDTC-007).
const TESTED_DATE_BYTES: usize = 8;

/// `Vstring-length` of the date string, the halfword `X'000A'` that the group move of line 122
/// puts in result positions 46-47 (CSUTLDTC-007).
const VSTRING_LENGTH_HALFWORD: [u8; 2] = [0x00, 0x0A];

/// Width of `WS-EDIT-VARIABLE-NAME PIC X(25)`: a longer name loses its tail (COACTUPC-022).
const NAME_WIDTH: usize = 25;

/// Width of `WS-RETURN-MSG PIC X(75)`: `STRING` stops at its end (CSUTLDTC-024).
const MESSAGE_WIDTH: usize = 75;

/// Width of `WS-EDIT-ALPHANUM-ONLY PIC X(256)`, which `1245-EDIT-NUM-REQD` reference-modifies.
const ALPHANUM_WIDTH: usize = 256;

/// The months `88 WS-31-DAY-MONTH` names (`CSUTLDWY.cpy:21-23`).
const THIRTY_ONE_DAY_MONTHS: [u32; 7] = [1, 3, 5, 7, 8, 10, 12];

/// The picture string a caller passes to CSUTLDTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DateMask {
    /// `YYYY-MM-DD`, the `WS-DATE-FORMAT` of COTRN02C and CORPT00C (CSUTLDTC-028,
    /// CSUTLDTC-030).
    YyyyMmDdDash,
    /// `YYYYMMDD`, which `EDIT-DATE-LE` moves into its 8-byte `WS-DATE-FORMAT` (CSUTLDTC-019).
    YyyyMmDd,
}

impl DateMask {
    /// The picture string's significant characters.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::YyyyMmDdDash => "YYYY-MM-DD",
            Self::YyyyMmDd => "YYYYMMDD",
        }
    }

    /// The positions of the picture, in order.
    const fn slots(self) -> &'static [Slot] {
        match self {
            Self::YyyyMmDdDash => &[
                Slot::Year,
                Slot::Year,
                Slot::Year,
                Slot::Year,
                Slot::Dash,
                Slot::Month,
                Slot::Month,
                Slot::Dash,
                Slot::Day,
                Slot::Day,
            ],
            Self::YyyyMmDd => &[
                Slot::Year,
                Slot::Year,
                Slot::Year,
                Slot::Year,
                Slot::Month,
                Slot::Month,
                Slot::Day,
                Slot::Day,
            ],
        }
    }
}

/// One position of a picture string: a digit of a date part, or the `-` separator.
#[derive(Clone, Copy)]
enum Slot {
    Year,
    Month,
    Day,
    Dash,
}

/// The `CEEDAYS` feedback token, decoded as the `EVALUATE TRUE` over the nine 88-levels of
/// `FEEDBACK-TOKEN-VALUE` decodes it (CSUTLDTC-009 to CSUTLDTC-018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeFeedback {
    /// The all-zero token, `88 FC-INVALID-DATE`: the date is valid despite the 88-level's name
    /// (CSUTLDTC-009).
    Valid,
    /// `FC-INSUFFICIENT-DATA`, severity 3, message 2507 (`X'09CB'`) (CSUTLDTC-010).
    InsufficientData,
    /// `FC-BAD-DATE-VALUE`, severity 3, message 2508 (CSUTLDTC-011).
    BadDateValue,
    /// `FC-INVALID-ERA`, severity 3, message 2509 (CSUTLDTC-012).
    InvalidEra,
    /// `FC-UNSUPP-RANGE`, severity 3, message 2513 (`X'09D1'`), the message number COTRN02C and
    /// CORPT00C accept (CSUTLDTC-013).
    UnsupportedRange,
    /// `FC-INVALID-MONTH`, severity 3, message 2517 (CSUTLDTC-014).
    InvalidMonth,
    /// `FC-BAD-PIC-STRING`, severity 3, message 2518 (CSUTLDTC-015).
    BadPicString,
    /// `FC-NON-NUMERIC-DATA`, severity 3, message 2520 (CSUTLDTC-016).
    NonNumericData,
    /// `FC-YEAR-IN-ERA-ZERO`, severity 3, message 2521 (CSUTLDTC-017).
    YearInEraZero,
    /// Any other token, `WHEN OTHER` (CSUTLDTC-018), with the signed halfwords of its condition
    /// ID: `SEVERITY` (token bytes 1-2) and `MSG-NO` (bytes 3-4).
    Other {
        /// `SEVERITY PIC S9(4) BINARY`.
        severity: i16,
        /// `MSG-NO PIC S9(4) BINARY`.
        msg_no: i16,
    },
}

impl LeFeedback {
    /// Decodes an 8-byte feedback token. Each of the nine 88-levels compares the whole token, so
    /// a token that differs from its `VALUE` in any byte, the case and facility bytes included,
    /// is [`LeFeedback::Other`] (CSUTLDTC-018).
    #[must_use]
    pub const fn from_token(token: [u8; 8]) -> Self {
        match token {
            [0, 0, 0, 0, 0, 0, 0, 0] => Self::Valid,
            [0x00, 0x03, 0x09, 0xCB, 0x59, 0xC3, 0xC5, 0xC5] => Self::InsufficientData,
            [0x00, 0x03, 0x09, 0xCC, 0x59, 0xC3, 0xC5, 0xC5] => Self::BadDateValue,
            [0x00, 0x03, 0x09, 0xCD, 0x59, 0xC3, 0xC5, 0xC5] => Self::InvalidEra,
            [0x00, 0x03, 0x09, 0xD1, 0x59, 0xC3, 0xC5, 0xC5] => Self::UnsupportedRange,
            [0x00, 0x03, 0x09, 0xD5, 0x59, 0xC3, 0xC5, 0xC5] => Self::InvalidMonth,
            [0x00, 0x03, 0x09, 0xD6, 0x59, 0xC3, 0xC5, 0xC5] => Self::BadPicString,
            [0x00, 0x03, 0x09, 0xD8, 0x59, 0xC3, 0xC5, 0xC5] => Self::NonNumericData,
            [0x00, 0x03, 0x09, 0xD9, 0x59, 0xC3, 0xC5, 0xC5] => Self::YearInEraZero,
            [s0, s1, m0, m1, ..] => Self::Other {
                severity: i16::from_be_bytes([s0, s1]),
                msg_no: i16::from_be_bytes([m0, m1]),
            },
        }
    }

    /// The severity as result positions 1-4 hold it: the `MOVE` into the unsigned
    /// `WS-SEVERITY-N PIC 9(4)` drops the sign and keeps the low-order four digits
    /// (CSUTLDTC-008).
    #[must_use]
    pub const fn severity(self) -> u16 {
        match self {
            Self::Valid => 0,
            Self::InsufficientData
            | Self::BadDateValue
            | Self::InvalidEra
            | Self::UnsupportedRange
            | Self::InvalidMonth
            | Self::BadPicString
            | Self::NonNumericData
            | Self::YearInEraZero => 3,
            Self::Other { severity, .. } => four_digits(severity),
        }
    }

    /// The message number as result positions 16-19 hold it, decoded as [`Self::severity`]
    /// decodes the severity (CSUTLDTC-008).
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
            Self::Other { msg_no, .. } => four_digits(msg_no),
        }
    }

    /// `WS-RESULT PIC X(15)`, result positions 21-35: the literal its `EVALUATE` branch moves,
    /// padded with spaces to 15 characters (CSUTLDTC-009 to CSUTLDTC-018). No caller reads it.
    #[must_use]
    pub const fn result_text(self) -> &'static str {
        match self {
            Self::Valid => "Date is valid  ",
            Self::InsufficientData => "Insufficient   ",
            Self::BadDateValue => "Datevalue error",
            Self::InvalidEra => "Invalid Era    ",
            Self::UnsupportedRange => "Unsupp. Range  ",
            Self::InvalidMonth => "Invalid month  ",
            Self::BadPicString => "Bad Pic String ",
            Self::NonNumericData => "Nonnumeric data",
            Self::YearInEraZero => "YearInEra is 0 ",
            Self::Other { .. } => "Date is invalid",
        }
    }
}

/// The low-order four digits of a halfword's magnitude, as a `MOVE` into `PIC 9(4)` keeps them.
const fn four_digits(halfword: i16) -> u16 {
    halfword.unsigned_abs() % 10_000
}

/// The `LS-RESULT` CSUTLDTC returns, as its consumers read it (CSUTLDTC-003).
///
/// Consumers read only the severity code (positions 1-4) and the message number (positions
/// 16-19); [`LeResult::to_record`] gives all 80 bytes. [`LeResult::date`] is the typed date the
/// tested text spells, which a consumer takes once it has accepted the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeResult {
    feedback: LeFeedback,
    mask: DateMask,
    severity_code: String,
    msg_no: String,
    tested: [u8; TESTED_DATE_BYTES],
    date: Option<NaiveDate>,
}

impl LeResult {
    /// The result CSUTLDTC returns for `text` under `mask` when `CEEDAYS` answers `feedback`.
    ///
    /// This is how a test substitutes the date-validity verdict, the one value halt H5 leaves
    /// open, to prove the rows that hold for every verdict of a class (CSUTLDTC-022 to
    /// CSUTLDTC-024, CSUTLDTC-029, CSUTLDTC-031). `text` is read as the 10 characters of
    /// `LS-DATE` (CSUTLDTC-005), and [`LeResult::date`] is the calendar date they spell under
    /// `mask`, whatever `feedback` says.
    #[must_use]
    pub fn new(text: &str, mask: DateMask, feedback: LeFeedback) -> Self {
        let field = fit(text, LS_DATE_WIDTH);
        let date = read_calendar(&field, mask).ok();
        Self::build(&field, mask, feedback, date)
    }

    /// Assembles the result from the 10-character date field CSUTLDTC received.
    fn build(field: &str, mask: DateMask, feedback: LeFeedback, date: Option<NaiveDate>) -> Self {
        let mut tested = [b' '; TESTED_DATE_BYTES];
        for (slot, byte) in tested.iter_mut().zip(field.bytes()) {
            *slot = byte;
        }
        Self {
            feedback,
            mask,
            severity_code: format!("{:04}", feedback.severity()),
            msg_no: format!("{:04}", feedback.msg_no()),
            tested,
            date,
        }
    }

    /// The decoded feedback token.
    #[must_use]
    pub const fn feedback(&self) -> LeFeedback {
        self.feedback
    }

    /// The picture string the caller passed.
    #[must_use]
    pub const fn mask(&self) -> DateMask {
        self.mask
    }

    /// Positions 1-4, `WS-SEVERITY`: four digits, `0000` for a valid date (CSUTLDTC-008).
    #[must_use]
    pub fn severity_code(&self) -> &str {
        &self.severity_code
    }

    /// Positions 16-19, `WS-MSG-NO`: four digits, such as `2513` (CSUTLDTC-008).
    #[must_use]
    pub fn msg_no(&self) -> &str {
        &self.msg_no
    }

    /// Positions 21-35, `WS-RESULT` (CSUTLDTC-009 to CSUTLDTC-018).
    #[must_use]
    pub const fn result_text(&self) -> &'static str {
        self.feedback.result_text()
    }

    /// The calendar date the tested text spells under the picture, or `None` when it spells
    /// none. This typed value, never the Lilian day number `CEEDAYS` returns (CSUTLDTC-006), is
    /// what a consumer keeps for a date it accepts.
    #[must_use]
    pub const fn date(&self) -> Option<NaiveDate> {
        self.date
    }

    /// All 80 bytes of `LS-RESULT` in the layout of CSUTLDTC-003: severity, `Mesg Code: `,
    /// message number, result text, `TstDate: `, the date field, `Mask used:`, the picture.
    ///
    /// Positions 46-55 hold what the group move of line 122 leaves there (X-CSUTLDTC-2): the
    /// length halfword `X'000A'` and the first 8 bytes of the 10-character date field, on the
    /// assumption that `CEEDAYS` leaves its input string as given, which is halt H5's
    /// (CSUTLDTC-007). Positions 67-76 hold the 10 characters of `LS-DATE-FORMAT`, so `YYYYMMDD`
    /// arrives followed by two spaces (CSUTLDTC-020). No consumer reads past position 19.
    #[must_use]
    pub fn to_record(&self) -> [u8; LS_RESULT_LEN] {
        let mask_field = fit(self.mask.text(), LS_DATE_FORMAT_WIDTH);
        let bytes = self
            .severity_code
            .bytes()
            .chain(*b"Mesg Code: ")
            .chain(self.msg_no.bytes())
            .chain(*b" ")
            .chain(self.result_text().bytes())
            .chain(*b" TstDate: ")
            .chain(VSTRING_LENGTH_HALFWORD)
            .chain(self.tested)
            .chain(*b" Mask used:")
            .chain(mask_field.bytes())
            .chain(*b"    ");
        let mut record = [b' '; LS_RESULT_LEN];
        for (slot, byte) in record.iter_mut().zip(bytes) {
            *slot = byte;
        }
        record
    }
}

/// The 15 October 1582 that [`validate`]'s stand-in verdict takes as the first supported date.
const LILIAN_DAY_ONE: Option<NaiveDate> = NaiveDate::from_ymd_opt(1582, 10, 15);

/// CSUTLDTC (`A000-MAIN`): the verdict on `text` under `mask`, as the 80-byte result models it
/// (CSUTLDTC-001 to CSUTLDTC-018).
///
/// `text` is read as the 10 characters CSUTLDTC takes from `LS-DATE` (CSUTLDTC-005); a caller
/// that passes a shorter field, such as COACTUPC's eight date characters, passes only those
/// characters, which the replacement judges alone (X-CSUTLDTC-1).
///
/// `HALTED(H5)`: the verdict itself belongs to `CEEDAYS`, outside the repository (CSUTLDTC-006).
/// This function stands in for it with a calendar check, in order, of the text without its
/// trailing spaces: fewer characters than the picture is message 2507, more is 2508; a non-digit
/// where the picture has a digit is 2520 and anything but `-` where it has one is 2508; a month
/// outside 1-12 is 2517 and a day the month does not have is 2508; a calendar date before
/// 15 October 1582 is 2513; anything else is valid. None of this is a parity result: every test
/// that asserts it is reported `BLOCKED`, and no consumer may present it as the COBOL's.
#[must_use]
pub fn validate(text: &str, mask: DateMask) -> LeResult {
    let field = fit(text, LS_DATE_WIDTH);
    // HALTED(H5): CEEDAYS validity rule lives outside the repository; this calendar check stands
    // in for it and is no parity result.
    let (feedback, date) = match read_calendar(&field, mask) {
        Ok(date) if LILIAN_DAY_ONE.is_some_and(|first| date < first) => {
            (LeFeedback::UnsupportedRange, Some(date))
        }
        Ok(date) => (LeFeedback::Valid, Some(date)),
        Err(feedback) => (feedback, None),
    };
    LeResult::build(&field, mask, feedback, date)
}

/// The calendar date `field` spells under `mask`, or the stand-in feedback that rejects it
/// (`HALTED(H5)`, see [`validate`]).
fn read_calendar(field: &str, mask: DateMask) -> Result<NaiveDate, LeFeedback> {
    let significant = field.trim_end_matches(' ');
    let slots = mask.slots();
    match significant.chars().count().cmp(&slots.len()) {
        core::cmp::Ordering::Less => return Err(LeFeedback::InsufficientData),
        core::cmp::Ordering::Greater => return Err(LeFeedback::BadDateValue),
        core::cmp::Ordering::Equal => {}
    }
    let (mut year, mut month, mut day) = (0_u32, 0_u32, 0_u32);
    for (c, slot) in significant.chars().zip(slots) {
        let part = match slot {
            Slot::Dash if c == '-' => continue,
            Slot::Dash => return Err(LeFeedback::BadDateValue),
            Slot::Year => &mut year,
            Slot::Month => &mut month,
            Slot::Day => &mut day,
        };
        let digit = c.to_digit(10).ok_or(LeFeedback::NonNumericData)?;
        *part = part
            .checked_mul(10)
            .and_then(|tens| tens.checked_add(digit))
            .ok_or(LeFeedback::BadDateValue)?;
    }
    if !(1..=12).contains(&month) {
        return Err(LeFeedback::InvalidMonth);
    }
    i32::try_from(year)
        .ok()
        .and_then(|year| NaiveDate::from_ymd_opt(year, month, day))
        .ok_or(LeFeedback::BadDateValue)
}

/// The acceptance test COTRN02C and CORPT00C apply to each date's result: severity `0000`
/// accepts, and so does message number `2513` despite its non-zero severity (CSUTLDTC-029,
/// CSUTLDTC-031; COTRN02C-037, CORPT00C-025, CORPT00C-026).
///
/// The test itself is the COBOL's and exact; which dates reach which branch is the verdict's,
/// `HALTED(H5)` (CSUTLDTC-013, X-CSUTLDTC-H2, CORPT00C-027). A result [`validate`] accepts always
/// has a [`LeResult::date`]; one accepted through a substituted 2513 may not, and its caller
/// cannot store it typed (COTRN02C-046).
#[must_use]
pub fn caller_accepts(result: &LeResult) -> bool {
    result.severity_code() == "0000" || result.msg_no() == "2513"
}

/// `EDIT-DATE-LE`, after its call to CSUTLDTC: the date passes when the severity is zero
/// (CSUTLDTC-022) and fails otherwise (CSUTLDTC-023), with the message of CSUTLDTC-024 on the
/// year part, the first of the three parts lines 302-304 flag.
///
/// The message's fixed text holds for every non-zero severity; the severity and message-number
/// characters embedded in it are the verdict's, `HALTED(H5)` (CSUTLDTC-025, COACTUPC-129).
///
/// # Errors
///
/// A non-zero severity: `TRIM(name)`, ` validation error Sev code: `, the four severity
/// characters, ` Message code: ` and the four message-number characters.
pub fn edit_date_le(name: &str, result: &LeResult) -> Result<(), EditFailure> {
    if result.feedback().severity() == 0 {
        return Ok(());
    }
    let literal = format!(
        " validation error Sev code: {} Message code: {}",
        result.severity_code(),
        result.msg_no()
    );
    Err(EditFailure::compose(name, &literal, Part::Year))
}

/// The sub-field an [`EditFailure`] names, so the caller can report the map field in error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Part {
    /// The edited field as a whole: [`edit_num_reqd`] and [`edit_us_state`].
    Field,
    /// The year part of a date: `OPNYEAR`, `EXPYEAR`, `RISYEAR` or `DOBYEAR`.
    Year,
    /// The month part of a date.
    Month,
    /// The day part of a date.
    Day,
    /// The area code of a phone number: `ACSPH1A` or `ACSPH2A`.
    AreaCode,
    /// The prefix of a phone number: `ACSPH1B` or `ACSPH2B`.
    Prefix,
    /// The line number of a phone number: `ACSPH1C` or `ACSPH2C`.
    LineNumber,
    /// The first three SSN characters, `ACTSSN1`.
    SsnFirst3,
    /// The fourth and fifth SSN characters, `ACTSSN2`.
    SsnMiddle2,
    /// The last four SSN characters, `ACTSSN3`.
    SsnLast4,
    /// The state field of the state and ZIP cross edit, `ACSSTTE`. The COBOL flags the ZIP too;
    /// the response names the state only (COACTUPC-067, X-COACTUPC-11).
    State,
}

/// A failed edit: the message COACTUPC sets and the part in error.
///
/// The message names the field and never holds its value, so `Debug` and `Display` disclose
/// nothing a request carried (Cardholder Data Redaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditFailure {
    message: String,
    part: Part,
}

impl EditFailure {
    /// The failure whose message `STRING FUNCTION TRIM(WS-EDIT-VARIABLE-NAME) literal DELIMITED
    /// BY SIZE INTO WS-RETURN-MSG` composes after `MOVE name TO WS-EDIT-VARIABLE-NAME`.
    ///
    /// The name keeps its first 25 characters (COACTUPC-022) and loses its leading and trailing
    /// spaces; the message stops at 75 characters, and the trailing spaces of `WS-RETURN-MSG`
    /// are not part of it (COACTUPC-130). An empty name gives the literal alone. COACTUPC's own
    /// edits build their failures with this too.
    #[must_use]
    pub fn compose(name: &str, literal: &str, part: Part) -> Self {
        let name: String = name.chars().take(NAME_WIDTH).collect();
        let message: String = name
            .trim_matches(' ')
            .chars()
            .chain(literal.chars())
            .take(MESSAGE_WIDTH)
            .collect();
        Self {
            message: message.trim_end_matches(' ').to_owned(),
            part,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The part in error.
    #[must_use]
    pub const fn part(&self) -> Part {
        self.part
    }
}

impl fmt::Display for EditFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EditFailure {}

/// `text` moved into a `PIC X(width)` receiver: its first `width` characters, padded with spaces.
fn fit(text: &str, width: usize) -> String {
    text.chars().chain(iter::repeat(' ')).take(width).collect()
}

/// `EQUAL SPACES OR EQUAL LOW-VALUES`: every character a space, or every one `\0`.
fn is_blank(field: &str) -> bool {
    field.chars().all(|c| c == ' ') || field.chars().all(|c| c == '\0')
}

/// The value of a field of ASCII digits, or `None` when any character is not one.
fn digits_value(field: &str) -> Option<u32> {
    field.chars().try_fold(0_u32, |value, c| {
        let digit = c.to_digit(10)?;
        value.checked_mul(10)?.checked_add(digit)
    })
}

/// The three literals of a required-digits edit.
struct DigitLiterals {
    /// The field is blank.
    supplied: &'static str,
    /// A character is not a digit.
    not_digits: &'static str,
    /// Every digit is zero.
    zero: &'static str,
}

/// `1245-EDIT-NUM-REQD` (COACTUPC-053).
const NUM_REQD: DigitLiterals = DigitLiterals {
    supplied: " must be supplied.",
    not_digits: " must be all numeric.",
    zero: " must not be zero.",
};

/// `EDIT-AREA-CODE` before its table test (COACTUPC-056).
const AREA_CODE: DigitLiterals = DigitLiterals {
    supplied: ": Area code must be supplied.",
    not_digits: ": Area code must be A 3 digit number.",
    zero: ": Area code cannot be zero",
};

/// `EDIT-US-PHONE-PREFIX` (COACTUPC-058).
const PREFIX: DigitLiterals = DigitLiterals {
    supplied: ": Prefix code must be supplied.",
    not_digits: ": Prefix code must be A 3 digit number.",
    zero: ": Prefix code cannot be zero",
};

/// `EDIT-US-PHONE-LINENUM` (COACTUPC-059).
const LINE_NUMBER: DigitLiterals = DigitLiterals {
    supplied: ": Line number code must be supplied.",
    not_digits: ": Line number code must be A 4 digit number.",
    zero: ": Line number code cannot be zero",
};

/// The required-digits checks in their order: blank, then any non-digit, then all zeros. A
/// numeric field's `NUMVAL` is zero exactly when every digit is.
fn edit_digits(
    name: &str,
    field: &str,
    literals: &DigitLiterals,
    part: Part,
) -> Result<(), EditFailure> {
    if is_blank(field) {
        return Err(EditFailure::compose(name, literals.supplied, part));
    }
    if !field.chars().all(|c| c.is_ascii_digit()) {
        return Err(EditFailure::compose(name, literals.not_digits, part));
    }
    if field.chars().all(|c| c == '0') {
        return Err(EditFailure::compose(name, literals.zero, part));
    }
    Ok(())
}

/// `1245-EDIT-NUM-REQD` (COACTUPC-053): the first `len` characters of `value` must be supplied,
/// all digits and not all zeros. COACTUPC runs it for the FICO score (3), the ZIP (5), the EFT
/// account ID (10) and, through [`edit_us_ssn`], the SSN parts (COACTUPC-026, COACTUPC-032,
/// COACTUPC-037, COACTUPC-060).
///
/// `value` is moved into the 256-character `WS-EDIT-ALPHANUM-ONLY` and its first `len`
/// characters are edited, so a `len` above 256 edits 256.
///
/// # Errors
///
/// `TRIM(name)` followed by ` must be supplied.`, ` must be all numeric.` or ` must not be
/// zero.`, on [`Part::Field`].
pub fn edit_num_reqd(name: &str, value: &str, len: usize) -> Result<(), EditFailure> {
    let field = fit(value, len.min(ALPHANUM_WIDTH));
    edit_digits(name, &field, &NUM_REQD, Part::Field)
}

/// `EDIT-YEAR-CCYY`, blank year (COACTUPC-118).
const YEAR_SUPPLIED: &str = " : Year must be supplied.";
/// `EDIT-YEAR-CCYY`, a year that is not four digits (COACTUPC-119).
const YEAR_NOT_DIGITS: &str = " must be 4 digit number.";
/// `EDIT-YEAR-CCYY`, a century other than 19 and 20 (COACTUPC-120).
const CENTURY_NOT_VALID: &str = " : Century is not valid.";
/// `EDIT-MONTH`, blank month (COACTUPC-121).
const MONTH_SUPPLIED: &str = " : Month must be supplied.";
/// `EDIT-MONTH`, a month outside 1-12 (COACTUPC-122, COACTUPC-123).
const MONTH_NOT_VALID: &str = ": Month must be a number between 1 and 12.";
/// `EDIT-DAY`, blank day (COACTUPC-124).
const DAY_SUPPLIED: &str = " : Day must be supplied.";
/// `EDIT-DAY`, a day `TEST-NUMVAL` rejects or outside 1-31 (COACTUPC-125).
const DAY_NOT_VALID: &str = ":day must be a number between 1 and 31.";
/// `EDIT-DAY-MONTH-YEAR`, day 31 in a shorter month (COACTUPC-126).
const NO_31_DAYS: &str = ":Cannot have 31 days in this month.";
/// `EDIT-DAY-MONTH-YEAR`, 30 February (COACTUPC-127).
const NO_30_DAYS: &str = ":Cannot have 30 days in this month.";
/// `EDIT-DAY-MONTH-YEAR`, 29 February outside a leap year (COACTUPC-128).
const NOT_LEAP_YEAR: &str = ":Not a leap year.Cannot have 29 days in this month.";
/// `EDIT-DATE-OF-BIRTH` (COACTUPC-130); its trailing space is padding of `WS-RETURN-MSG`.
const IN_THE_FUTURE: &str = ":cannot be in the future ";

/// `EDIT-DATE-CCYYMMDD` through its exit (COACTUPC-117 to COACTUPC-131), with the verdict of
/// [`validate`] for its `EDIT-DATE-LE` step (CSUTLDTC-027).
///
/// # Errors
///
/// As [`edit_date_ccyymmdd_with`].
pub fn edit_date_ccyymmdd(
    name: &str,
    year: &str,
    month: &str,
    day: &str,
) -> Result<NaiveDate, EditFailure> {
    edit_date_ccyymmdd_with(name, year, month, day, validate)
}

/// `EDIT-DATE-CCYYMMDD` through its exit, with `verdict` standing in for the CSUTLDTC call of
/// `EDIT-DATE-LE` (COACTUPC-117 to COACTUPC-131).
///
/// `year`, `month` and `day` are the raw text of the three map fields, moved into the 4-, 2-
/// and 2-character parts of `WS-EDIT-DATE-CCYYMMDD`. The year edit (COACTUPC-118 to
/// COACTUPC-120), the month edit (COACTUPC-121 to COACTUPC-123) and the day edit
/// (COACTUPC-124, COACTUPC-125) run in that order and the first failure is returned. When all
/// three pass, the day-month-year checks follow (COACTUPC-126 to COACTUPC-128), and then
/// `EDIT-DATE-LE`, whose verdict `verdict` gives for the eight characters under `YYYYMMDD`
/// (CSUTLDTC-019 to CSUTLDTC-025, COACTUPC-129); [`edit_date_ccyymmdd`] passes [`validate`],
/// whose verdict is `HALTED(H5)`. The accepted date is the typed date of the edited year, month
/// and day values (COACTUPC-131).
///
/// After a failed part the COBOL still runs the day-month-year checks, which then either only
/// flag parts after the message is set or read text no edit accepted (halt candidate
/// X-COACTUPC-H3); the replacement skips them, which changes no message.
///
/// # Errors
///
/// The first failing check's [`EditFailure`]: on [`Part::Year`] for the year checks, the leap
/// year check and the LE rejection; on [`Part::Month`] for the month checks and the 31- and
/// 30-day checks; on [`Part::Day`] for the day checks.
pub fn edit_date_ccyymmdd_with(
    name: &str,
    year: &str,
    month: &str,
    day: &str,
    verdict: impl FnOnce(&str, DateMask) -> LeResult,
) -> Result<NaiveDate, EditFailure> {
    let (year, month, day) = (fit(year, 4), fit(month, 2), fit(day, 2));
    let ccyy = edit_year(name, &year)?;
    let mm = edit_month(name, &month)?;
    let dd = edit_day(name, &day)?;
    edit_day_month_year(name, ccyy, mm, dd)?;
    edit_date_le(
        name,
        &verdict(&format!("{year}{month}{day}"), DateMask::YyyyMmDd),
    )?;
    // Unreachable fallback: the checks above leave a year of 1900-2099, a month of 1-12 and a
    // day the month has, which `from_ymd_opt` always accepts.
    i32::try_from(ccyy)
        .ok()
        .and_then(|year| NaiveDate::from_ymd_opt(year, mm, dd))
        .ok_or_else(|| EditFailure::compose(name, DAY_NOT_VALID, Part::Day))
}

/// `EDIT-YEAR-CCYY`: supplied, four digits, century 19 or 20 (COACTUPC-118 to COACTUPC-120).
fn edit_year(name: &str, year: &str) -> Result<u32, EditFailure> {
    if is_blank(year) {
        return Err(EditFailure::compose(name, YEAR_SUPPLIED, Part::Year));
    }
    let Some(ccyy) = digits_value(year) else {
        return Err(EditFailure::compose(name, YEAR_NOT_DIGITS, Part::Year));
    };
    if (1900..=2099).contains(&ccyy) {
        Ok(ccyy)
    } else {
        Err(EditFailure::compose(name, CENTURY_NOT_VALID, Part::Year))
    }
}

/// `EDIT-MONTH`: supplied, then two digits `01` to `12` (COACTUPC-121 to COACTUPC-123).
fn edit_month(name: &str, month: &str) -> Result<u32, EditFailure> {
    if is_blank(month) {
        return Err(EditFailure::compose(name, MONTH_SUPPLIED, Part::Month));
    }
    // Halt candidate X-COACTUPC-H3: the COBOL tests the range on the numeric redefinition before
    // any digit check, so a month with a non-digit has no single COBOL result. Rejecting it with
    // the range message is the replacement's choice, not a parity result (COACTUPC-122).
    match digits_value(month) {
        Some(mm @ 1..=12) => Ok(mm),
        _ => Err(EditFailure::compose(name, MONTH_NOT_VALID, Part::Month)),
    }
}

/// `EDIT-DAY`: supplied, accepted by `TEST-NUMVAL`, and 1 to 31 once its `NUMVAL` is stored
/// into the unsigned `PIC 9(2)` (COACTUPC-124, COACTUPC-125).
fn edit_day(name: &str, day: &str) -> Result<u32, EditFailure> {
    if is_blank(day) {
        return Err(EditFailure::compose(name, DAY_SUPPLIED, Part::Day));
    }
    match numval_two_chars(day) {
        Some(dd @ 1..=31) => Ok(dd),
        _ => Err(EditFailure::compose(name, DAY_NOT_VALID, Part::Day)),
    }
}

/// `NUMVAL` of a two-character field stored into an unsigned `PIC 9(2)`, or `None` when
/// `TEST-NUMVAL` rejects the text.
///
/// Two characters hold a `NUMVAL` string only as two digits, a digit with a space, a sign or a
/// decimal point on one side, or a point before a digit; `CR` and `DB` need a third character.
/// The store drops the sign and the fraction, so `-1`, ` 1`, `1 ` and `1.` give 1 and `.5`
/// gives 0 (COACTUPC-125).
fn numval_two_chars(field: &str) -> Option<u32> {
    let mut chars = field.chars();
    let (first, second) = (chars.next()?, chars.next()?);
    if chars.next().is_some() {
        return None;
    }
    match (first, second, first.to_digit(10), second.to_digit(10)) {
        (_, _, Some(tens), Some(units)) => Some(tens * 10 + units),
        (' ' | '+' | '-', _, None, Some(units)) | (_, ' ' | '+' | '-' | '.', Some(units), None) => {
            Some(units)
        }
        ('.', _, None, Some(_)) => Some(0),
        _ => None,
    }
}

/// `EDIT-DAY-MONTH-YEAR` over values whose part edits passed (COACTUPC-126 to COACTUPC-128).
/// The leap test divides the year by 400 when its last two digits are `00` and by 4 otherwise,
/// which for 1900-2099 is the Gregorian rule.
fn edit_day_month_year(name: &str, ccyy: u32, mm: u32, dd: u32) -> Result<(), EditFailure> {
    if dd == 31 && !THIRTY_ONE_DAY_MONTHS.contains(&mm) {
        return Err(EditFailure::compose(name, NO_31_DAYS, Part::Month));
    }
    if mm == 2 && dd == 30 {
        return Err(EditFailure::compose(name, NO_30_DAYS, Part::Month));
    }
    let divisor = if ccyy % 100 == 0 { 400 } else { 4 };
    if mm == 2 && dd == 29 && ccyy % divisor != 0 {
        return Err(EditFailure::compose(name, NOT_LEAP_YEAR, Part::Year));
    }
    Ok(())
}

/// `EDIT-DATE-OF-BIRTH` (COACTUPC-130): the date of birth must lie before today, the injected
/// clock's date; a date of birth equal to today counts as future.
///
/// COACTUPC runs it only after [`edit_date_ccyymmdd`] accepted the date. After an LE rejection
/// the COBOL runs it too, but it cannot replace the LE message (COACTUPC-025), so a caller
/// holding that failure has nothing more to run.
///
/// # Errors
///
/// `TRIM(name)` followed by `:cannot be in the future`, on [`Part::Year`] (`DOBYEAR`).
pub fn edit_date_of_birth(
    name: &str,
    dob: NaiveDate,
    clock: &dyn Clock,
) -> Result<(), EditFailure> {
    if clock.now().date() > dob {
        Ok(())
    } else {
        Err(EditFailure::compose(name, IN_THE_FUTURE, Part::Year))
    }
}

/// `EDIT-AREA-CODE`, an area code outside `VALID-GENERAL-PURP-CODE` (COACTUPC-057).
const NOT_GENERAL_PURPOSE: &str = ": Not valid North America general purpose area code";

/// `1260-EDIT-US-PHONE-NUM` (COACTUPC-055 to COACTUPC-059): an optional phone number of area
/// code, prefix and line number, the raw text of its three map fields.
///
/// Three blank parts are a valid, absent number. The COBOL's test at line 2238 checks the area
/// code for spaces where the line number is meant, but every blank part holds low-values after
/// the receive (COACTUPC-007), so the test is that all three parts are blank. Otherwise the area
/// code, prefix and line number edits run in turn, and the first failure is returned.
///
/// # Errors
///
/// `TRIM(name)` followed by the literal of the first failing check: on [`Part::AreaCode`],
/// `: Area code must be supplied.`, `: Area code must be A 3 digit number.`, `: Area code cannot
/// be zero` or `: Not valid North America general purpose area code`; on [`Part::Prefix`] and
/// [`Part::LineNumber`], the `Prefix code` and `Line number code` texts of the same shape with
/// 3 and 4 digits and no table test.
pub fn edit_us_phone(name: &str, area: &str, prefix: &str, line: &str) -> Result<(), EditFailure> {
    let (area, prefix, line) = (fit(area, 3), fit(prefix, 3), fit(line, 4));
    if is_blank(&area) && is_blank(&prefix) && is_blank(&line) {
        return Ok(());
    }
    edit_area_code(name, &area)?;
    edit_digits(name, &prefix, &PREFIX, Part::Prefix)?;
    edit_digits(name, &line, &LINE_NUMBER, Part::LineNumber)
}

/// `EDIT-AREA-CODE`: three digits, not `000`, and one of the general-purpose codes
/// (COACTUPC-056, COACTUPC-057).
fn edit_area_code(name: &str, area: &str) -> Result<(), EditFailure> {
    edit_digits(name, area, &AREA_CODE, Part::AreaCode)?;
    if GENERAL_PURPOSE_AREA_CODES.binary_search(&area).is_ok() {
        Ok(())
    } else {
        Err(EditFailure::compose(
            name,
            NOT_GENERAL_PURPOSE,
            Part::AreaCode,
        ))
    }
}

/// The name `1265-EDIT-US-SSN` moves in for part 1 (COACTUPC-060).
const SSN_FIRST_NAME: &str = "SSN: First 3 chars";
/// The name for part 2 (COACTUPC-062).
const SSN_MIDDLE_NAME: &str = "SSN 4th & 5th chars";
/// The name for part 3 (COACTUPC-063).
const SSN_LAST_NAME: &str = "SSN Last 4 chars";
/// `88 INVALID-SSN-PART1` failed (COACTUPC-061).
const SSN_FIRST_FORBIDDEN: &str = ": should not be 000, 666, or between 900 and 999";

/// `1265-EDIT-US-SSN` (COACTUPC-024, COACTUPC-060 to COACTUPC-063): the three SSN parts, the raw
/// text of `ACTSSN1`, `ACTSSN2` and `ACTSSN3`, as the nine-digit `CUST-SSN PIC 9(09)`.
///
/// Part 1 is edited by `1245-EDIT-NUM-REQD`; only when it passes do the forbidden-value test and
/// the part 2 and part 3 edits run, because the `IF` of line 2448 encloses them up to the
/// paragraph's period. The first failure is returned. The value is cardholder data: it leaves
/// only as the returned [`PicU`], whose `Debug` shows it, so the caller wraps it in
/// `crate::sensitive::Ssn` at once and never formats the `Result`. No failure holds a digit.
///
/// # Errors
///
/// On [`Part::SsnFirst3`], `SSN: First 3 chars` followed by ` must be supplied.`, ` must be all
/// numeric.`, ` must not be zero.` or `: should not be 000, 666, or between 900 and 999` (for
/// `666` and `900` to `999`); on [`Part::SsnMiddle2`] and [`Part::SsnLast4`], `SSN 4th & 5th
/// chars` and `SSN Last 4 chars` followed by the first three of those texts.
pub fn edit_us_ssn(part1: &str, part2: &str, part3: &str) -> Result<PicU<9>, EditFailure> {
    let first = fit(part1, 3);
    edit_digits(SSN_FIRST_NAME, &first, &NUM_REQD, Part::SsnFirst3)?;
    // `88 INVALID-SSN-PART1` holds 0, 666 and 900 through 999; zero failed the edit above.
    if first == "666" || first.starts_with('9') {
        return Err(EditFailure::compose(
            SSN_FIRST_NAME,
            SSN_FIRST_FORBIDDEN,
            Part::SsnFirst3,
        ));
    }
    let middle = fit(part2, 2);
    edit_digits(SSN_MIDDLE_NAME, &middle, &NUM_REQD, Part::SsnMiddle2)?;
    let last = fit(part3, 4);
    edit_digits(SSN_LAST_NAME, &last, &NUM_REQD, Part::SsnLast4)?;
    // Unreachable fallback: the three parts hold 3, 2 and 4 ASCII digits, nine in all, which
    // `decode` always accepts.
    PicU::<9>::decode(format!("{first}{middle}{last}").as_bytes())
        .map_err(|_| EditFailure::compose(SSN_FIRST_NAME, NUM_REQD.not_digits, Part::SsnFirst3))
}

/// `1270-EDIT-US-STATE-CD`, a code outside `VALID-US-STATE-CODE` (COACTUPC-065).
const NOT_VALID_STATE: &str = ": is not a valid state code";

/// `1270-EDIT-US-STATE-CD` (COACTUPC-031, COACTUPC-065): the two-character state code must be
/// one of the 56 codes of `VALID-US-STATE-CODE`, exactly, so a lower-case code fails. COACTUPC
/// runs it only after the code passed its letters edit.
///
/// # Errors
///
/// `TRIM(name)` followed by `: is not a valid state code`, on [`Part::Field`].
pub fn edit_us_state(name: &str, code: &str) -> Result<(), EditFailure> {
    let code = fit(code, 2);
    if US_STATE_CODES.binary_search(&code.as_str()).is_ok() {
        Ok(())
    } else {
        Err(EditFailure::compose(name, NOT_VALID_STATE, Part::Field))
    }
}

/// `1280-EDIT-US-STATE-ZIP-CD`, a state and ZIP prefix outside the combinations (COACTUPC-067).
const INVALID_ZIP_FOR_STATE: &str = "Invalid zip code for state";

/// `1280-EDIT-US-STATE-ZIP-CD` (COACTUPC-067): the state code followed by the first two ZIP
/// characters must be one of the 240 values of `VALID-US-STATE-ZIP-CD2-COMBO`, exactly.
/// COACTUPC runs it last, after the state and ZIP edits passed.
///
/// # Errors
///
/// `Invalid zip code for state`, with no field name, on [`Part::State`].
pub fn edit_us_state_zip(state: &str, zip: &str) -> Result<(), EditFailure> {
    let key = fit(state, 2) + &fit(zip, 2);
    if US_STATE_ZIP2_COMBOS.binary_search(&key.as_str()).is_ok() {
        Ok(())
    } else {
        // The STRING of line 2551 holds the literal alone, so the name is empty.
        Err(EditFailure::compose("", INVALID_ZIP_FOR_STATE, Part::State))
    }
}

/// `88 VALID-GENERAL-PURP-CODE` (`CSLKPCDY.cpy:521-930`): the 410 area codes the phone edit
/// accepts (COACTUPC-057), sorted for `binary_search`. The copybook's other two area-code lists,
/// `VALID-PHONE-AREA-CODE` and `VALID-EASY-RECOG-AREA-CODE`, are tested by no edit.
pub const GENERAL_PURPOSE_AREA_CODES: &[&str; 410] = &[
    "201", "202", "203", "204", "205", "206", "207", "208", "209", "210", "212", "213", "214",
    "215", "216", "217", "218", "219", "220", "223", "224", "225", "226", "228", "229", "231",
    "234", "236", "239", "240", "242", "246", "248", "249", "250", "251", "252", "253", "254",
    "256", "260", "262", "264", "267", "268", "269", "270", "272", "276", "279", "281", "284",
    "289", "301", "302", "303", "304", "305", "306", "307", "308", "309", "310", "312", "313",
    "314", "315", "316", "317", "318", "319", "320", "321", "323", "325", "326", "330", "331",
    "332", "334", "336", "337", "339", "340", "341", "343", "345", "346", "347", "351", "352",
    "360", "361", "364", "365", "367", "368", "380", "385", "386", "401", "402", "403", "404",
    "405", "406", "407", "408", "409", "410", "412", "413", "414", "415", "416", "417", "418",
    "419", "423", "424", "425", "430", "431", "432", "434", "435", "437", "438", "440", "441",
    "442", "443", "445", "447", "448", "450", "458", "463", "464", "469", "470", "473", "474",
    "475", "478", "479", "480", "484", "501", "502", "503", "504", "505", "506", "507", "508",
    "509", "510", "512", "513", "514", "515", "516", "517", "518", "519", "520", "530", "531",
    "534", "539", "540", "541", "548", "551", "559", "561", "562", "563", "564", "567", "570",
    "571", "572", "573", "574", "575", "579", "580", "581", "582", "585", "586", "587", "601",
    "602", "603", "604", "605", "606", "607", "608", "609", "610", "612", "613", "614", "615",
    "616", "617", "618", "619", "620", "623", "626", "628", "629", "630", "631", "636", "639",
    "640", "641", "646", "647", "649", "650", "651", "656", "657", "658", "659", "660", "661",
    "662", "664", "667", "669", "670", "671", "672", "678", "680", "681", "682", "683", "684",
    "689", "701", "702", "703", "704", "705", "706", "707", "708", "709", "712", "713", "714",
    "715", "716", "717", "718", "719", "720", "721", "724", "725", "726", "727", "731", "732",
    "734", "737", "740", "742", "743", "747", "753", "754", "757", "758", "760", "762", "763",
    "765", "767", "769", "770", "771", "772", "773", "774", "775", "778", "779", "780", "781",
    "782", "784", "785", "786", "787", "801", "802", "803", "804", "805", "806", "807", "808",
    "809", "810", "812", "813", "814", "815", "816", "817", "818", "819", "820", "825", "826",
    "828", "829", "830", "831", "832", "838", "839", "840", "843", "845", "847", "848", "849",
    "850", "854", "856", "857", "858", "859", "860", "862", "863", "864", "865", "867", "868",
    "869", "870", "872", "873", "876", "878", "901", "902", "903", "904", "905", "906", "907",
    "908", "909", "910", "912", "913", "914", "915", "916", "917", "918", "919", "920", "925",
    "928", "929", "930", "931", "934", "936", "937", "938", "939", "940", "941", "943", "945",
    "947", "948", "949", "951", "952", "954", "956", "959", "970", "971", "972", "973", "978",
    "979", "980", "983", "984", "985", "986", "989",
];

/// `88 VALID-US-STATE-CODE` (`CSLKPCDY.cpy:1012-1069`): the 50 states, `DC`, `AS`, `GU`, `MP`, `PR`
/// and `VI` (COACTUPC-065), sorted for `binary_search`.
pub const US_STATE_CODES: &[&str; 56] = &[
    "AK", "AL", "AR", "AS", "AZ", "CA", "CO", "CT", "DC", "DE", "FL", "GA", "GU", "HI", "IA", "ID",
    "IL", "IN", "KS", "KY", "LA", "MA", "MD", "ME", "MI", "MN", "MO", "MP", "MS", "MT", "NC", "ND",
    "NE", "NH", "NJ", "NM", "NV", "NY", "OH", "OK", "OR", "PA", "PR", "RI", "SC", "SD", "TN", "TX",
    "UT", "VA", "VI", "VT", "WA", "WI", "WV", "WY",
];

/// `88 VALID-US-STATE-ZIP-CD2-COMBO` (`CSLKPCDY.cpy:1071-1314`): a state code followed by the
/// first two ZIP digits, 240 values (COACTUPC-067), sorted for `binary_search`.
pub const US_STATE_ZIP2_COMBOS: &[&str; 240] = &[
    "AA34", "AE90", "AE91", "AE92", "AE93", "AE94", "AE95", "AE96", "AE97", "AE98", "AK99", "AL35",
    "AL36", "AP96", "AR71", "AR72", "AS96", "AZ85", "AZ86", "CA90", "CA91", "CA92", "CA93", "CA94",
    "CA95", "CA96", "CO80", "CO81", "CT60", "CT61", "CT62", "CT63", "CT64", "CT65", "CT66", "CT67",
    "CT68", "CT69", "DC20", "DC56", "DC88", "DE19", "FL32", "FL33", "FL34", "FM96", "GA30", "GA31",
    "GA39", "GU96", "HI96", "IA50", "IA51", "IA52", "ID83", "IL60", "IL61", "IL62", "IN46", "IN47",
    "KS66", "KS67", "KY40", "KY41", "KY42", "LA70", "LA71", "MA10", "MA11", "MA12", "MA13", "MA14",
    "MA15", "MA16", "MA17", "MA18", "MA19", "MA20", "MA21", "MA22", "MA23", "MA24", "MA25", "MA26",
    "MA27", "MA55", "MD20", "MD21", "ME39", "ME40", "ME41", "ME42", "ME43", "ME44", "ME45", "ME46",
    "ME47", "ME48", "ME49", "MH96", "MI48", "MI49", "MN55", "MN56", "MO63", "MO64", "MO65", "MO72",
    "MP96", "MS38", "MS39", "MT59", "NC27", "NC28", "ND58", "NE68", "NE69", "NH30", "NH31", "NH32",
    "NH33", "NH34", "NH35", "NH36", "NH37", "NH38", "NJ70", "NJ71", "NJ72", "NJ73", "NJ74", "NJ75",
    "NJ76", "NJ77", "NJ78", "NJ79", "NJ80", "NJ81", "NJ82", "NJ83", "NJ84", "NJ85", "NJ86", "NJ87",
    "NJ88", "NJ89", "NM87", "NM88", "NV88", "NV89", "NY10", "NY11", "NY12", "NY13", "NY14", "NY50",
    "NY54", "NY63", "OH43", "OH44", "OH45", "OK73", "OK74", "OR97", "PA15", "PA16", "PA17", "PA18",
    "PA19", "PR60", "PR61", "PR62", "PR63", "PR64", "PR65", "PR66", "PR67", "PR68", "PR69", "PR70",
    "PR71", "PR72", "PR73", "PR74", "PR75", "PR76", "PR77", "PR78", "PR79", "PR90", "PR91", "PR92",
    "PR93", "PR94", "PR95", "PR96", "PR97", "PR98", "PW96", "RI28", "RI29", "SC29", "SD57", "TN37",
    "TN38", "TX73", "TX75", "TX76", "TX77", "TX78", "TX79", "TX88", "UT84", "VA20", "VA22", "VA23",
    "VA24", "VI80", "VI82", "VI83", "VI84", "VI85", "VT50", "VT51", "VT52", "VT53", "VT54", "VT56",
    "VT57", "VT58", "VT59", "WA98", "WA99", "WI53", "WI54", "WV24", "WV25", "WV26", "WY82", "WY83",
];

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use copybook::PicU;

    use super::Part::{
        AreaCode, Day, Field, LineNumber, Month, Prefix, SsnFirst3, SsnLast4, SsnMiddle2, State,
        Year,
    };
    use super::{
        DateMask, EditFailure, GENERAL_PURPOSE_AREA_CODES, LS_RESULT_LEN, LeFeedback, LeResult,
        Part, US_STATE_CODES, US_STATE_ZIP2_COMBOS, caller_accepts, digits_value,
        edit_date_ccyymmdd, edit_date_ccyymmdd_with, edit_date_le, edit_date_of_birth,
        edit_num_reqd, edit_us_phone, edit_us_ssn, edit_us_state, edit_us_state_zip,
        numval_two_chars, validate,
    };
    use crate::clock::FixedClock;

    // The audited literals, written out apart from the module's own constants.
    const SUPPLIED: &str = " must be supplied.";
    const NUMERIC: &str = " must be all numeric.";
    const ZERO: &str = " must not be zero.";
    const MONTH_RANGE: &str = ": Month must be a number between 1 and 12.";
    const DAY_RANGE: &str = ":day must be a number between 1 and 31.";
    const NO_31: &str = ":Cannot have 31 days in this month.";
    const LEAP: &str = ":Not a leap year.Cannot have 29 days in this month.";
    const FORBIDDEN: &str = ": should not be 000, 666, or between 900 and 999";

    /// Outcome of an edit: passed, or its failure's message and part.
    type Outcome = Result<(), (String, Part)>;

    fn opened<T>(result: Result<T, EditFailure>) -> Outcome {
        result
            .map(|_| ())
            .map_err(|f| (f.message().to_owned(), f.part()))
    }

    /// Passes for `None`; fails with `name` followed by the literal, on the part, otherwise.
    fn expected(name: &str, failure: Option<(&str, Part)>) -> Outcome {
        failure.map_or(Ok(()), |(literal, part)| {
            Err((format!("{name}{literal}"), part))
        })
    }

    fn date(year: i32, month: u32, day: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(year, month, day)
    }

    /// A verdict that accepts every date, substituted for the H5 call.
    fn accept_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::Valid)
    }

    #[test]
    fn masks_spell_their_picture_strings() {
        assert_eq!(DateMask::YyyyMmDdDash.text(), "YYYY-MM-DD");
        assert_eq!(DateMask::YyyyMmDd.text(), "YYYYMMDD");
    }

    #[test]
    fn tokens_decode_as_the_nine_88_levels_and_when_other() {
        // CSUTLDTC-008 to CSUTLDTC-018.
        let cee = |msg: u8| [0x00, 0x03, 0x09, msg, 0x59, 0xC3, 0xC5, 0xC5];
        let other = |severity, msg_no| LeFeedback::Other { severity, msg_no };
        let cases = [
            ([0; 8], LeFeedback::Valid, 0, 0, "Date is valid  "),
            (
                cee(0xCB),
                LeFeedback::InsufficientData,
                3,
                2507,
                "Insufficient   ",
            ),
            (
                cee(0xCC),
                LeFeedback::BadDateValue,
                3,
                2508,
                "Datevalue error",
            ),
            (
                cee(0xCD),
                LeFeedback::InvalidEra,
                3,
                2509,
                "Invalid Era    ",
            ),
            (
                cee(0xD1),
                LeFeedback::UnsupportedRange,
                3,
                2513,
                "Unsupp. Range  ",
            ),
            (
                cee(0xD5),
                LeFeedback::InvalidMonth,
                3,
                2517,
                "Invalid month  ",
            ),
            (
                cee(0xD6),
                LeFeedback::BadPicString,
                3,
                2518,
                "Bad Pic String ",
            ),
            (
                cee(0xD8),
                LeFeedback::NonNumericData,
                3,
                2520,
                "Nonnumeric data",
            ),
            (
                cee(0xD9),
                LeFeedback::YearInEraZero,
                3,
                2521,
                "YearInEra is 0 ",
            ),
            (
                [0, 3, 0x09, 0xD1, 0, 0, 0, 0],
                other(3, 2513),
                3,
                2513,
                "Date is invalid",
            ),
            (
                [0xFF, 0xFD, 0x7F, 0xFF, 0, 0, 0, 1],
                other(-3, 32767),
                3,
                2767,
                "Date is invalid",
            ),
        ];
        for (token, feedback, severity, msg_no, text) in cases {
            assert_eq!(LeFeedback::from_token(token), feedback);
            assert_eq!(feedback.severity(), severity);
            assert_eq!(feedback.msg_no(), msg_no);
            assert_eq!(feedback.result_text(), text);
            assert_eq!(text.len(), 15);
        }
    }

    #[test]
    fn result_record_has_the_80_byte_layout() {
        // CSUTLDTC-003, CSUTLDTC-007, CSUTLDTC-020.
        let dashed = LeResult::new("2020-01-31", DateMask::YyyyMmDdDash, LeFeedback::Valid);
        let mut expected = b"0000Mesg Code: 0000 Date is valid   TstDate: \x00\x0A".to_vec();
        expected.extend_from_slice(b"2020-01- Mask used:YYYY-MM-DD    ");
        assert_eq!(dashed.to_record().to_vec(), expected);

        let plain = LeResult::new("2020013", DateMask::YyyyMmDd, LeFeedback::InsufficientData);
        let mut expected = b"0003Mesg Code: 2507 Insufficient    TstDate: \x00\x0A".to_vec();
        expected.extend_from_slice(b"2020013  Mask used:YYYYMMDD      ");
        assert_eq!(plain.to_record().to_vec(), expected);
        assert_eq!(plain.to_record().len(), LS_RESULT_LEN);
        assert_eq!(plain.result_text(), "Insufficient   ");
        assert_eq!(plain.mask(), DateMask::YyyyMmDd);
        assert_eq!(plain.date(), None);
    }

    #[test]
    fn stand_in_verdicts_blocked_h5() {
        // BLOCKED (H5): these pin the stand-in's own rule; none is a COBOL parity result.
        let (dash, plain) = (DateMask::YyyyMmDdDash, DateMask::YyyyMmDd);
        let cases = [
            ("2020-01-31", dash, LeFeedback::Valid, date(2020, 1, 31)),
            ("20200131", plain, LeFeedback::Valid, date(2020, 1, 31)),
            (
                "2020-01-31-extra",
                dash,
                LeFeedback::Valid,
                date(2020, 1, 31),
            ),
            ("", dash, LeFeedback::InsufficientData, None),
            ("2020-1-31", dash, LeFeedback::InsufficientData, None),
            ("2020013100", plain, LeFeedback::BadDateValue, None),
            ("2020/01/31", dash, LeFeedback::BadDateValue, None),
            ("2020-0A-31", dash, LeFeedback::NonNumericData, None),
            ("２０２０-01-31", dash, LeFeedback::NonNumericData, None),
            ("2020-13-01", dash, LeFeedback::InvalidMonth, None),
            ("2021-02-29", dash, LeFeedback::BadDateValue, None),
            (
                "1582-10-14",
                dash,
                LeFeedback::UnsupportedRange,
                date(1582, 10, 14),
            ),
            ("1582-10-15", dash, LeFeedback::Valid, date(1582, 10, 15)),
        ];
        for (text, mask, feedback, typed) in cases {
            let result = validate(text, mask);
            assert_eq!(result.feedback(), feedback, "{text}");
            assert_eq!(result.date(), typed, "{text}");
            assert_eq!(caller_accepts(&result), typed.is_some(), "{text}");
        }
    }

    #[test]
    fn callers_accept_severity_zero_or_message_2513() {
        // CSUTLDTC-029, CSUTLDTC-031, read over substituted verdicts.
        let other = |severity, msg_no| LeFeedback::Other { severity, msg_no };
        let cases = [
            (LeFeedback::Valid, true),
            (LeFeedback::UnsupportedRange, true),
            (LeFeedback::BadDateValue, false),
            (LeFeedback::NonNumericData, false),
            (other(3, 2513), true),
            (other(0, 7), true),
            (other(4, 2512), false),
        ];
        for (feedback, accepted) in cases {
            let result = LeResult::new("2020-02-30", DateMask::YyyyMmDdDash, feedback);
            assert_eq!(caller_accepts(&result), accepted, "{feedback:?}");
            assert_eq!(result.date(), None);
        }
    }

    #[test]
    fn edit_date_le_composes_its_message_for_any_nonzero_severity() {
        // CSUTLDTC-022 to CSUTLDTC-024; the embedded codes are those of the substituted verdict.
        let valid = LeResult::new("20200131", DateMask::YyyyMmDd, LeFeedback::Valid);
        assert_eq!(opened(edit_date_le("Open Date", &valid)), Ok(()));
        let rejected = LeResult::new("20200131", DateMask::YyyyMmDd, LeFeedback::BadDateValue);
        let message = " validation error Sev code: 0003 Message code: 2508";
        assert_eq!(
            opened(edit_date_le("Date of Birth", &rejected)),
            expected("Date of Birth", Some((message, Year)))
        );
    }

    #[test]
    fn compose_trims_and_cuts_as_the_cobol_moves() {
        let cut = EditFailure::compose("Current Cycle Credit Limit", " is not valid", Field);
        assert_eq!(cut.message(), "Current Cycle Credit Limi is not valid");
        assert_eq!(cut.to_string(), cut.message());
        let long = EditFailure::compose(&"N".repeat(30), LEAP, Year);
        assert_eq!(long.message().chars().count(), 75);
        let future = EditFailure::compose("  Date of Birth", ":cannot be in the future ", Year);
        assert_eq!(future.message(), "Date of Birth:cannot be in the future");
        let bare = EditFailure::compose("", "Invalid zip code for state", State);
        assert_eq!(bare.message(), "Invalid zip code for state");
        assert!(std::error::Error::source(&bare).is_none());
    }

    #[test]
    fn num_reqd_checks_supplied_numeric_then_zero() {
        // COACTUPC-053.
        let cases = [
            ("", 3, Some(SUPPLIED)),
            ("\0\0\0", 3, Some(SUPPLIED)),
            ("123", 0, Some(SUPPLIED)),
            ("\0 \0", 3, Some(NUMERIC)),
            ("12a", 3, Some(NUMERIC)),
            ("12", 3, Some(NUMERIC)),
            ("١٢٣", 3, Some(NUMERIC)),
            ("000", 3, Some(ZERO)),
            ("123", 3, None),
            ("1234", 3, None),
            ("12345     ", 5, None),
        ];
        for (value, len, literal) in cases {
            let outcome = opened(edit_num_reqd("FICO Score", value, len));
            assert_eq!(
                outcome,
                expected("FICO Score", literal.map(|l| (l, Field))),
                "{value:?}"
            );
        }
        let wide = "1".repeat(300);
        assert_eq!(opened(edit_num_reqd("EFT Account Id", &wide, 300)), Ok(()));
    }

    #[test]
    fn date_part_edits_return_the_first_failure() {
        // COACTUPC-117 to COACTUPC-128.
        let cases = [
            ("", "01", "01", (" : Year must be supplied.", Year)),
            ("", "13", "", (" : Year must be supplied.", Year)),
            ("20a0", "01", "01", (" must be 4 digit number.", Year)),
            ("1899", "01", "01", (" : Century is not valid.", Year)),
            ("2100", "01", "01", (" : Century is not valid.", Year)),
            ("2020", "\0\0", "01", (" : Month must be supplied.", Month)),
            ("2020", "13", "01", (MONTH_RANGE, Month)),
            ("2020", "00", "01", (MONTH_RANGE, Month)),
            ("2020", " 3", "01", (MONTH_RANGE, Month)),
            ("2020", "01", "", (" : Day must be supplied.", Day)),
            ("2020", "01", "32", (DAY_RANGE, Day)),
            ("2020", "01", "0A", (DAY_RANGE, Day)),
            ("2020", "01", ".5", (DAY_RANGE, Day)),
            ("2020", "04", "31", (NO_31, Month)),
            ("2020", "02", "31", (NO_31, Month)),
            (
                "2020",
                "02",
                "30",
                (":Cannot have 30 days in this month.", Month),
            ),
            ("1900", "02", "29", (LEAP, Year)),
            ("2021", "02", "29", (LEAP, Year)),
        ];
        for (y, m, d, failure) in cases {
            let outcome = opened(edit_date_ccyymmdd_with("Open Date", y, m, d, accept_all));
            assert_eq!(
                outcome,
                expected("Open Date", Some(failure)),
                "{y:?} {m:?} {d:?}"
            );
        }
    }

    #[test]
    fn accepted_dates_are_the_edited_values() {
        // COACTUPC-125, COACTUPC-131, CSUTLDTC-022, with a substituted accepting verdict.
        let cases = [
            ("2000", "02", "29", date(2000, 2, 29)),
            ("2024", "02", "29", date(2024, 2, 29)),
            ("2020", "12", "31", date(2020, 12, 31)),
            ("2020", "01", "01", date(2020, 1, 1)),
        ];
        for (y, m, d, typed) in cases {
            let accepted = edit_date_ccyymmdd_with("Open Date", y, m, d, accept_all);
            assert_eq!(accepted.ok(), typed, "{y:?} {m:?} {d:?}");
        }
        for day in ["1.", "-1", " 1", "1 ", "+1", "1+", "1-"] {
            let accepted = edit_date_ccyymmdd_with("Open Date", "2020", "01", day, accept_all);
            assert_eq!(accepted.ok(), date(2020, 1, 1), "{day:?}");
        }
    }

    #[test]
    fn le_step_receives_the_eight_characters_blocked_h5() {
        // CSUTLDTC-021, CSUTLDTC-027; BLOCKED (H5) where the stand-in verdict decides.
        let mut seen = None;
        let accepted = edit_date_ccyymmdd_with("Expiry Date", "2020", "01", "1.", |text, mask| {
            seen = Some((text.to_owned(), mask));
            LeResult::new(text, mask, LeFeedback::Valid)
        });
        assert_eq!(accepted.ok(), date(2020, 1, 1));
        assert_eq!(seen, Some(("2020011.".to_owned(), DateMask::YyyyMmDd)));
        let message = " validation error Sev code: 0003 Message code: 2520";
        assert_eq!(
            opened(edit_date_ccyymmdd("Expiry Date", "2020", "01", "1.")),
            expected("Expiry Date", Some((message, Year)))
        );
        let valid = edit_date_ccyymmdd("Expiry Date", "2020", "01", "31");
        assert_eq!(valid.ok(), date(2020, 1, 31));
    }

    #[test]
    fn numval_reads_only_two_character_forms() {
        let cases = [
            ("07", Some(7)),
            ("99", Some(99)),
            (".5", Some(0)),
            ("+.", None),
            (". ", None),
            ("  ", None),
            ("1", None),
            ("123", None),
        ];
        for (field, value) in cases {
            assert_eq!(numval_two_chars(field), value, "{field:?}");
        }
        assert_eq!(digits_value("99999999999"), None);
        assert_eq!(digits_value("0042"), Some(42));
    }

    #[test]
    fn date_of_birth_must_precede_today() {
        // COACTUPC-130.
        let noon = date(2024, 5, 10).and_then(|today| today.and_hms_opt(12, 0, 0));
        let clock = FixedClock::new(noon.unwrap_or_default());
        let future = expected("Date of Birth", Some((":cannot be in the future", Year)));
        for (day, outcome) in [(9, Ok(())), (10, future.clone()), (11, future)] {
            let dob = date(2024, 5, day).unwrap_or_default();
            let result = edit_date_of_birth("Date of Birth", dob, &clock);
            assert_eq!(opened(result), outcome, "{day}");
        }
    }

    #[test]
    fn phone_edits_area_prefix_then_line() {
        // COACTUPC-055 to COACTUPC-059.
        let cases = [
            ("", "", "", None),
            ("\0\0\0", "   ", "", None),
            ("201", "555", "1234", None),
            (
                "",
                "",
                "1234",
                Some((": Area code must be supplied.", AreaCode)),
            ),
            (
                "2a1",
                "555",
                "1234",
                Some((": Area code must be A 3 digit number.", AreaCode)),
            ),
            (
                "000",
                "555",
                "1234",
                Some((": Area code cannot be zero", AreaCode)),
            ),
            (
                "200",
                "555",
                "1234",
                Some((
                    ": Not valid North America general purpose area code",
                    AreaCode,
                )),
            ),
            (
                "201",
                "",
                "1234",
                Some((": Prefix code must be supplied.", Prefix)),
            ),
            (
                "201",
                "5a5",
                "1234",
                Some((": Prefix code must be A 3 digit number.", Prefix)),
            ),
            (
                "201",
                "000",
                "1234",
                Some((": Prefix code cannot be zero", Prefix)),
            ),
            (
                "201",
                "555",
                "",
                Some((": Line number code must be supplied.", LineNumber)),
            ),
            (
                "201",
                "555",
                "12a4",
                Some((": Line number code must be A 4 digit number.", LineNumber)),
            ),
            (
                "201",
                "555",
                "0000",
                Some((": Line number code cannot be zero", LineNumber)),
            ),
        ];
        for (area, prefix, line, failure) in cases {
            let outcome = opened(edit_us_phone("Phone Number 1", area, prefix, line));
            assert_eq!(
                outcome,
                expected("Phone Number 1", failure),
                "{area:?} {prefix:?} {line:?}"
            );
        }
    }

    #[test]
    fn ssn_edits_part_one_before_parts_two_and_three() {
        // COACTUPC-024, COACTUPC-060 to COACTUPC-063.
        let (first, middle, last) = (
            "SSN: First 3 chars",
            "SSN 4th & 5th chars",
            "SSN Last 4 chars",
        );
        let cases = [
            ("", "", "", first, SUPPLIED, SsnFirst3),
            ("12a", "45", "6789", first, NUMERIC, SsnFirst3),
            ("000", "", "", first, ZERO, SsnFirst3),
            ("666", "", "", first, FORBIDDEN, SsnFirst3),
            ("900", "45", "6789", first, FORBIDDEN, SsnFirst3),
            ("999", "45", "6789", first, FORBIDDEN, SsnFirst3),
            ("123", "", "6789", middle, SUPPLIED, SsnMiddle2),
            ("123", "4x", "6789", middle, NUMERIC, SsnMiddle2),
            ("123", "00", "6789", middle, ZERO, SsnMiddle2),
            ("123", "45", "", last, SUPPLIED, SsnLast4),
            ("123", "45", "67x9", last, NUMERIC, SsnLast4),
            ("123", "45", "0000", last, ZERO, SsnLast4),
        ];
        for (part1, part2, part3, name, literal, part) in cases {
            let result = edit_us_ssn(part1, part2, part3);
            if let Err(failure) = &result {
                assert!(!format!("{failure:?}").contains("6789"));
            }
            assert_eq!(opened(result), expected(name, Some((literal, part))));
        }
        assert_eq!(
            edit_us_ssn("123", "45", "6789").map(PicU::get),
            Ok(123_456_789)
        );
        assert_eq!(
            edit_us_ssn("665", "01", "0001").map(PicU::get),
            Ok(665_010_001)
        );
    }

    #[test]
    fn state_and_state_zip_use_the_lookup_tables() {
        // COACTUPC-065, COACTUPC-067.
        let not_state = expected("State", Some((": is not a valid state code", Field)));
        for (code, valid) in [
            ("NY", true),
            ("NYC", true),
            ("ny", false),
            ("ZZ", false),
            ("", false),
        ] {
            let outcome = if valid { Ok(()) } else { not_state.clone() };
            assert_eq!(opened(edit_us_state("State", code)), outcome, "{code:?}");
        }
        let bad_zip = expected("", Some(("Invalid zip code for state", State)));
        for (state, zip, valid) in [
            ("NY", "10001", true),
            ("CA", "90210", true),
            ("NY", "90210", false),
            ("", "", false),
        ] {
            let outcome = if valid { Ok(()) } else { bad_zip.clone() };
            assert_eq!(
                opened(edit_us_state_zip(state, zip)),
                outcome,
                "{state:?} {zip:?}"
            );
        }
    }

    #[test]
    fn lookup_tables_are_sorted_and_shaped_as_the_copybook() {
        // COACTUPC-057 (410 codes), COACTUPC-065 (56), COACTUPC-067 (240): counts are the types.
        let sorted = |table: &[&str]| table.windows(2).all(|pair| matches!(pair, [a, b] if a < b));
        assert!(sorted(GENERAL_PURPOSE_AREA_CODES));
        assert!(sorted(US_STATE_CODES));
        assert!(sorted(US_STATE_ZIP2_COMBOS));
        let digits = |code: &&str| code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit());
        assert!(GENERAL_PURPOSE_AREA_CODES.iter().all(digits));
        let upper = |code: &&str| code.len() == 2 && code.bytes().all(|b| b.is_ascii_uppercase());
        assert!(US_STATE_CODES.iter().all(upper));
        assert!(US_STATE_ZIP2_COMBOS.iter().all(|combo| {
            combo.len() == 4
                && combo.bytes().take(2).all(|b| b.is_ascii_uppercase())
                && combo.bytes().skip(2).all(|b| b.is_ascii_digit())
        }));
    }
}
