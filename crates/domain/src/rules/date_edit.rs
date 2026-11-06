//! `CSUTLDPY`: the `CCYYMMDD` date edits of `COACTUPC` (CAUP).
//!
//! `COACTUPC` copies the procedure copybook `CSUTLDPY` [app/cbl/COACTUPC.cbl:L4232]
//! and its working storage `CSUTLDWY` [app/cbl/COACTUPC.cbl:L166], and performs
//! `EDIT-DATE-CCYYMMDD THRU EDIT-DATE-CCYYMMDD-EXIT` for the open, expiry,
//! reissue and birth dates, then `EDIT-DATE-OF-BIRTH` for a birth date whose
//! flags came back valid [app/cbl/COACTUPC.cbl:L1478-1543] (P-CSUTLDPY-001).
//! [`edit_date_ccyymmdd`] and [`edit_date_of_birth`] are those two performs.
//!
//! # Control flow
//!
//! The `THRU` range runs the paragraphs in their physical order:
//! `EDIT-DATE-CCYYMMDD` sets the three flags not OK, then `EDIT-YEAR-CCYY`,
//! `EDIT-MONTH`, `EDIT-DAY`, `EDIT-DAY-MONTH-YEAR` and `EDIT-DATE-LE`. A
//! `GO TO <paragraph>-EXIT` leaves only its own paragraph, so every part is
//! edited whatever the earlier parts held; only `EDIT-DAY-MONTH-YEAR` can leave
//! the range early, and it does so whenever a flag is not valid, so
//! `CSUTLDTC` sees only dates that passed every edit. Each failing step writes
//! its message only `IF WS-RETURN-MSG-OFF` ([`ReturnMsg`]), so the first
//! failure of the request stands, with the BMS field the validation audit
//! names for it.
//!
//! # Input classes the COBOL leaves undefined
//!
//! - A month holding a non-digit: `IF WS-VALID-MONTH` tests the numeric
//!   redefine of the raw two bytes before `TEST-NUMVAL` runs
//!   [app/cpy/CSUTLDPY.cpy:L111-124], and invalid zoned data gives
//!   unpredictable results (E-47, at `halt`). The edit answers
//!   [`DateValidationError::Unproven`] naming E-47.
//! - A day that `NUMVAL` converts from a fraction of exactly one half, such
//!   as `.5`: the floating-point result stored in the unsigned `9(2)` is the
//!   midpoint E-40 holds at `halt`. The edit answers `Unproven` naming E-40.
//!   Every other day `TEST-NUMVAL` accepts is pinned: the sign is dropped by
//!   the unsigned receiver and a fraction goes to the nearest integer, so
//!   `-5`, `5 ` and `5.` are day 05 and `.7` is day 01 (V-COACTUPC-019).
//! - Every call `EDIT-DATE-LE` makes: E-26 holds it at `halt`, so
//!   `date_validation::validate` answers `Unproven` naming E-26 and the edit
//!   passes that on.
//!
//! `EDIT-DAY-MONTH-YEAR` also runs after a blank or rejected part, and then
//! tests the 88-levels of a month or day still holding spaces, low-values or
//! the rejected text. Those tests are read as false, the reading under which
//! the reviewed rows hold: a blank day leaves the cursor on the day
//! (V-COACTUPC-018) instead of flagging the month too. The leap-year division
//! is made only for a year of four digits; any other year has already failed
//! with its own message and flag, which come first whatever the division of
//! its bytes would give.
//!
//! # Field values
//!
//! [`DateParts`] holds the parts as `1100-RECEIVE-MAP` moved them. Its rule
//! that a part holding `*` or `SPACES` becomes `LOW-VALUES`
//! [app/cbl/COACTUPC.cbl:L1142-1275] (P-COACTUPC-019) belongs to the caller;
//! the edits treat `SPACES` and `LOW-VALUES` alike.

use chrono::NaiveDate;

use super::ReturnMsg;
use super::date_validation::{
    CsutldtcResult, DatePicture, DateValidationError, LS_DATE_LEN, validate,
};
use crate::messages::{
    CANNOT_BE_IN_THE_FUTURE, CANNOT_HAVE_30_DAYS_IN_THIS_MONTH, CANNOT_HAVE_31_DAYS_IN_THIS_MONTH,
    CENTURY_NOT_VALID, DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31, DAY_MUST_BE_SUPPLIED,
    MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12, MONTH_MUST_BE_SUPPLIED, MUST_BE_4_DIGIT_NUMBER,
    NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS, YEAR_MUST_BE_SUPPLIED, date_edit_message,
    date_validation_error_message,
};
use crate::numeric::Alnum;

/// The width of `WS-EDIT-DATE-CCYY X(4)` [app/cpy/CSUTLDWY.cpy:L5-15].
const CCYY_LEN: usize = 4;

/// The width of `WS-EDIT-DATE-MM X(2)` [app/cpy/CSUTLDWY.cpy:L16].
const MM_LEN: usize = 2;

/// The width of `WS-EDIT-DATE-DD X(2)` [app/cpy/CSUTLDWY.cpy:L25].
const DD_LEN: usize = 2;

/// The cursor field of an `EDIT-DATE-LE` failure: its exit sets the date flags
/// valid again, so the cursor `EVALUATE` of `COACTUPC` falls to `WHEN OTHER`
/// [app/cbl/COACTUPC.cbl:L3162-3163] (V-COACTUPC-023, E-45).
const FIELD_ACCTSID: &str = "ACCTSID";

/// The escalation that holds a month holding a non-digit at `halt`.
const E_NON_DIGIT_MONTH: &str = "E-47";

/// The escalation that holds a `NUMVAL` result of exactly one half at `halt`.
const E_NUMVAL_MIDPOINT: &str = "E-40";

/// The bytes 9 and 10 of the `CSUTLDTC` date argument.
///
/// `EDIT-DATE-LE` passes the 8-byte `WS-EDIT-DATE-CCYYMMDD` to the 10-byte
/// `LS-DATE` [app/cbl/CSUTLDTC.cbl:L84], so `CEEDAYS` also reads the first two
/// bytes of the next item, `WS-EDIT-DATE-BINARY PIC S9(9) BINARY`
/// [app/cpy/CSUTLDWY.cpy:L35-37]. That item has no `VALUE` clause and no
/// `INITIALIZE` reaches it, and its only assignment, in `EDIT-DATE-OF-BIRTH`,
/// runs after the last `EDIT-DATE-LE` of a `COACTUPC` task, so every call
/// passes storage the program never defines. E-26 holds every such call at
/// `halt` (P-CSUTLDPY-014) and `date_validation::validate` answers it with
/// `Unproven` whatever the ten bytes are; these low-values only give the
/// argument a defined value and decide no outcome.
const EDIT_DATE_BINARY_HIGH_BYTES: [u8; 2] = [0x00, 0x00];

/// `WS-EDIT-DATE-CCYYMMDD` [app/cpy/CSUTLDWY.cpy:L4-34]: the year, month and
/// day of one date, each the bytes of its `PIC X` part.
///
/// The `COACTUP` map enters the parts separately (`OPNYEAR`, `OPNMON`,
/// `OPNDAY` and so on) and `1100-RECEIVE-MAP` moves each into its part of the
/// `ACUP-NEW-*` date [app/cbl/COACTUPC.cbl:L772-789].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateParts {
    /// `WS-EDIT-DATE-CCYY X(4)`.
    pub ccyy: [u8; CCYY_LEN],
    /// `WS-EDIT-DATE-MM X(2)`.
    pub mm: [u8; MM_LEN],
    /// `WS-EDIT-DATE-DD X(2)`.
    pub dd: [u8; DD_LEN],
}

impl DateParts {
    /// The parts as entered, each moved by the COBOL alphanumeric `MOVE`
    /// into its `PIC X` part: left-justified, padded with spaces, cut on the
    /// right.
    #[must_use]
    pub fn from_entered(year: &str, month: &str, day: &str) -> Self {
        Self {
            ccyy: *Alnum::<CCYY_LEN>::move_from(year).as_bytes(),
            mm: *Alnum::<MM_LEN>::move_from(month).as_bytes(),
            dd: *Alnum::<DD_LEN>::move_from(day).as_bytes(),
        }
    }
}

/// The state of one date part: an 88-level of `WS-EDIT-YEAR-FLG`,
/// `WS-EDIT-MONTH` or `WS-EDIT-DAY` [app/cpy/CSUTLDWY.cpy:L43-57].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagState {
    /// `LOW-VALUES`: `FLG-…-ISVALID`.
    Valid,
    /// `'0'`: `FLG-…-NOT-OK`.
    NotOk,
    /// `'B'`: `FLG-…-BLANK`.
    Blank,
}

/// `WS-EDIT-DATE-FLGS` [app/cpy/CSUTLDWY.cpy:L43-57], which `COACTUPC` moves
/// to the flags of the date it edited (`WS-EDIT-OPEN-DATE-FLGS` and so on)
/// to place the cursor and colour the fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateEditFlags {
    /// `WS-EDIT-YEAR-FLG`.
    pub year: FlagState,
    /// `WS-EDIT-MONTH`.
    pub month: FlagState,
    /// `WS-EDIT-DAY`.
    pub day: FlagState,
}

impl DateEditFlags {
    /// `WS-EDIT-DATE-IS-VALID`: all three flags `LOW-VALUES`.
    pub const VALID: Self = Self {
        year: FlagState::Valid,
        month: FlagState::Valid,
        day: FlagState::Valid,
    };

    /// `WS-EDIT-DATE-IS-INVALID`: all three flags `'0'`.
    pub const INVALID: Self = Self {
        year: FlagState::NotOk,
        month: FlagState::NotOk,
        day: FlagState::NotOk,
    };

    /// The 88-level `WS-EDIT-DATE-IS-VALID`, which `COACTUPC` tests as
    /// `WS-EDIT-DT-OF-BIRTH-ISVALID` before the birth-date check
    /// [app/cbl/COACTUPC.cbl:L1539].
    #[must_use]
    pub const fn is_valid(self) -> bool {
        matches!(
            self,
            Self {
                year: FlagState::Valid,
                month: FlagState::Valid,
                day: FlagState::Valid,
            }
        )
    }
}

/// The BMS fields of one date's parts: the field each failure's cursor is
/// placed on, as the validation audit's "BMS field" column records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DateBmsFields {
    /// The year field, for example `OPNYEAR`.
    pub year: &'static str,
    /// The month field, for example `OPNMON`.
    pub month: &'static str,
    /// The day field, for example `OPNDAY`.
    pub day: &'static str,
}

impl DateBmsFields {
    /// Open Date: `OPNYEAR`, `OPNMON`, `OPNDAY` [app/cpy-bms/COACTUP.CPY:L72-84]
    /// (V-COACTUPC-013 to V-COACTUPC-022).
    pub const OPEN_DATE: Self = Self {
        year: "OPNYEAR",
        month: "OPNMON",
        day: "OPNDAY",
    };

    /// Expiry Date: `EXPYEAR`, `EXPMON`, `EXPDAY`
    /// [app/cpy-bms/COACTUP.CPY:L96-108] (V-COACTUPC-026 to V-COACTUPC-035).
    pub const EXPIRY_DATE: Self = Self {
        year: "EXPYEAR",
        month: "EXPMON",
        day: "EXPDAY",
    };

    /// Reissue Date: `RISYEAR`, `RISMON`, `RISDAY`
    /// [app/cpy-bms/COACTUP.CPY:L120-132] (V-COACTUPC-039 to V-COACTUPC-048).
    pub const REISSUE_DATE: Self = Self {
        year: "RISYEAR",
        month: "RISMON",
        day: "RISDAY",
    };

    /// Date of Birth: `DOBYEAR`, `DOBMON`, `DOBDAY`
    /// [app/cpy-bms/COACTUP.CPY:L186-198] (V-COACTUPC-066 to V-COACTUPC-077).
    pub const DATE_OF_BIRTH: Self = Self {
        year: "DOBYEAR",
        month: "DOBMON",
        day: "DOBDAY",
    };
}

/// CSUTLDPY EDIT-DATE-CCYYMMDD THRU EDIT-DATE-CCYYMMDD-EXIT — P-CSUTLDPY-001 to
/// P-CSUTLDPY-012, P-CSUTLDPY-014, V-COACTUPC-013 to V-COACTUPC-023,
/// V-COACTUPC-026 to V-COACTUPC-036, V-COACTUPC-039 to V-COACTUPC-049,
/// V-COACTUPC-066 to V-COACTUPC-076
///
/// Edits the date `parts` of the field labelled `field_name` (a
/// `crate::messages::FIELD_*` label, moved to `WS-EDIT-VARIABLE-NAME`) and
/// returns `WS-EDIT-DATE-FLGS` as the range leaves it. A failing step writes
/// its message into `msg` only when no earlier message is set, with the field
/// of `bms_fields` the audit names for it: the year field for the year edits
/// and the leap-year edit, the month field for the month edits and the 31-day
/// and 30-day edits (they flag the month before the day), the day field for
/// the day edits, and `ACCTSID` for a `CSUTLDTC` rejection.
///
/// # Errors
///
/// [`DateValidationError::Unproven`] for an input class the audit holds at
/// `halt`, naming its escalation:
///
/// - E-47 for a month that is neither blank nor two digits;
/// - E-40 for a day `NUMVAL` reads as exactly one half;
/// - E-26, from `date_validation::validate`, for every date that passes the
///   edits and so reaches `EDIT-DATE-LE`.
pub fn edit_date_ccyymmdd(
    field_name: &'static str,
    parts: &DateParts,
    msg: &mut ReturnMsg,
    bms_fields: DateBmsFields,
) -> Result<DateEditFlags, DateValidationError> {
    // EDIT-DATE-CCYYMMDD: SET WS-EDIT-DATE-IS-INVALID TO TRUE.
    let mut edit = DateEdit {
        field_name,
        fields: bms_fields,
        date: *parts,
        flags: DateEditFlags::INVALID,
        msg,
    };
    edit.edit_year();
    edit.edit_month()?;
    edit.edit_day()?;
    if edit.edit_day_month_year() == Flow::Continue {
        edit.edit_date_le()?;
    }
    Ok(edit.flags)
}

/// CSUTLDPY EDIT-DATE-OF-BIRTH — P-CSUTLDPY-013, V-COACTUPC-077
///
/// The reasonableness check of a birth date: `INTEGER-OF-DATE` of
/// `FUNCTION CURRENT-DATE`, here `today` from the processing clock, must be
/// greater than that of the birth date [app/cpy/CSUTLDPY.cpy:L341-372], so a
/// birth date equal to today is rejected. A rejection writes
/// `":cannot be in the future "` after the trimmed `field_name` into `msg`,
/// when no earlier message is set, on `bms_field` (`DOBYEAR`), and returns
/// the three flags not OK; a pass returns them valid, the state
/// `edit_date_ccyymmdd` left them in.
///
/// `COACTUPC` performs it only for `parts` that [`edit_date_ccyymmdd`] left
/// valid [app/cbl/COACTUPC.cbl:L1539-1543]; the day is read as that edit
/// converted it, so `5 ` is the fifth. Parts that do not form a calendar date
/// have no `INTEGER-OF-DATE`; `COACTUPC` never passes them, and they are
/// rejected rather than given a day number.
#[must_use]
pub fn edit_date_of_birth(
    field_name: &'static str,
    parts: &DateParts,
    today: NaiveDate,
    msg: &mut ReturnMsg,
    bms_field: &'static str,
) -> DateEditFlags {
    if birth_date(*parts).is_some_and(|birth| today > birth) {
        return DateEditFlags::VALID;
    }
    msg.set_if_off(
        date_edit_message(field_name, CANNOT_BE_IN_THE_FUTURE),
        bms_field,
    );
    DateEditFlags::INVALID
}

/// Where the `THRU` range goes after `EDIT-DAY-MONTH-YEAR`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    /// On to `EDIT-DATE-LE`: every flag is valid.
    Continue,
    /// `GO TO EDIT-DATE-CCYYMMDD-EXIT`.
    ExitRange,
}

/// The working storage of one `EDIT-DATE-CCYYMMDD` perform.
struct DateEdit<'m> {
    /// `WS-EDIT-VARIABLE-NAME`, as its `FIELD_*` label.
    field_name: &'static str,
    /// The BMS fields of the date's parts.
    fields: DateBmsFields,
    /// `WS-EDIT-DATE-CCYYMMDD`, which `EDIT-DAY` rewrites with the day
    /// `NUMVAL` converts.
    date: DateParts,
    /// `WS-EDIT-DATE-FLGS`.
    flags: DateEditFlags,
    /// `WS-RETURN-MSG`.
    msg: &'m mut ReturnMsg,
}

impl DateEdit<'_> {
    /// `IF WS-RETURN-MSG-OFF STRING FUNCTION TRIM(WS-EDIT-VARIABLE-NAME)
    /// <suffix> … INTO WS-RETURN-MSG`, with the cursor field of the step.
    fn reject(&mut self, suffix: &str, field: &'static str) {
        self.msg
            .set_if_off(date_edit_message(self.field_name, suffix), field);
    }

    /// CSUTLDPY EDIT-YEAR-CCYY — P-CSUTLDPY-002 to P-CSUTLDPY-004,
    /// V-COACTUPC-013 to V-COACTUPC-015, V-COACTUPC-026 to V-COACTUPC-028,
    /// V-COACTUPC-039 to V-COACTUPC-041, V-COACTUPC-066 to V-COACTUPC-068
    ///
    /// Blank (`SPACES` or `LOW-VALUES`), not four digits (`IS NOT NUMERIC`),
    /// or a century other than 19 or 20 (`THIS-CENTURY`, `LAST-CENTURY`)
    /// [app/cpy/CSUTLDPY.cpy:L25-90].
    fn edit_year(&mut self) {
        let year_field = self.fields.year;
        self.flags.year = FlagState::NotOk;
        let year = Alnum::<CCYY_LEN>::from_array(self.date.ccyy);
        if year.is_blank() {
            self.flags.year = FlagState::Blank;
            self.reject(YEAR_MUST_BE_SUPPLIED, year_field);
            return;
        }
        if !year.is_numeric() {
            self.reject(MUST_BE_4_DIGIT_NUMBER, year_field);
            return;
        }
        let [c1, c2, _, _] = self.date.ccyy;
        if !matches!(two_digits([c1, c2]), Some(19 | 20)) {
            self.reject(CENTURY_NOT_VALID, year_field);
            return;
        }
        self.flags.year = FlagState::Valid;
    }

    /// CSUTLDPY EDIT-MONTH — P-CSUTLDPY-005, P-CSUTLDPY-006, V-COACTUPC-016,
    /// V-COACTUPC-017, V-COACTUPC-029, V-COACTUPC-030, V-COACTUPC-042,
    /// V-COACTUPC-043, V-COACTUPC-069, V-COACTUPC-070
    ///
    /// Blank, or not `01` to `12` (`WS-VALID-MONTH`)
    /// [app/cpy/CSUTLDPY.cpy:L91-147]. The `TEST-NUMVAL` test that follows
    /// accepts every two digits and its `NUMVAL` gives them back unchanged, so
    /// a month that passed the range test passes it too.
    ///
    /// # Errors
    ///
    /// [`DateValidationError::Unproven`] naming E-47 for a month that is
    /// neither blank nor two digits: the range test reads it as zoned data.
    fn edit_month(&mut self) -> Result<(), DateValidationError> {
        let month_field = self.fields.month;
        self.flags.month = FlagState::NotOk;
        if Alnum::<MM_LEN>::from_array(self.date.mm).is_blank() {
            self.flags.month = FlagState::Blank;
            self.reject(MONTH_MUST_BE_SUPPLIED, month_field);
            return Ok(());
        }
        let month = two_digits(self.date.mm).ok_or(DateValidationError::Unproven {
            escalation_id: E_NON_DIGIT_MONTH,
        })?;
        if !(1..=12).contains(&month) {
            self.reject(MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12, month_field);
            return Ok(());
        }
        self.flags.month = FlagState::Valid;
        Ok(())
    }

    /// CSUTLDPY EDIT-DAY — P-CSUTLDPY-007, P-CSUTLDPY-008, V-COACTUPC-018,
    /// V-COACTUPC-019, V-COACTUPC-031, V-COACTUPC-032, V-COACTUPC-044,
    /// V-COACTUPC-045, V-COACTUPC-071, V-COACTUPC-072
    ///
    /// Blank, not a `NUMVAL` argument (`TEST-NUMVAL`), or a value outside 1
    /// to 31 (`WS-VALID-DAY`) [app/cpy/CSUTLDPY.cpy:L150-207]. A day that
    /// converts is stored back as two digits by `COMPUTE WS-EDIT-DATE-DD-N =
    /// FUNCTION NUMVAL(WS-EDIT-DATE-DD)`, and that value is what the later
    /// steps and `CSUTLDTC` see.
    ///
    /// # Errors
    ///
    /// [`DateValidationError::Unproven`] naming E-40 for a day `NUMVAL` reads
    /// as exactly one half.
    fn edit_day(&mut self) -> Result<(), DateValidationError> {
        let day_field = self.fields.day;
        self.flags.day = FlagState::Valid;
        if Alnum::<DD_LEN>::from_array(self.date.dd).is_blank() {
            self.flags.day = FlagState::Blank;
            self.reject(DAY_MUST_BE_SUPPLIED, day_field);
            return Ok(());
        }
        let Some(day) = numval_unsigned_2(self.date.dd)? else {
            self.flags.day = FlagState::NotOk;
            self.reject(DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31, day_field);
            return Ok(());
        };
        self.date.dd = two_digit_bytes(day);
        if !(1..=31).contains(&day) {
            self.flags.day = FlagState::NotOk;
            self.reject(DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31, day_field);
        }
        Ok(())
    }

    /// CSUTLDPY EDIT-DAY-MONTH-YEAR — P-CSUTLDPY-009 to P-CSUTLDPY-011,
    /// V-COACTUPC-020 to V-COACTUPC-022, V-COACTUPC-033 to V-COACTUPC-035,
    /// V-COACTUPC-046 to V-COACTUPC-048, V-COACTUPC-073 to V-COACTUPC-075
    ///
    /// Day 31 outside the months 1, 3, 5, 7, 8, 10 and 12, 30 February, and 29
    /// February when the year leaves a remainder divided by 4, or by 400 when
    /// it ends in `00` [app/cpy/CSUTLDPY.cpy:L209-282]. Each failure ends the
    /// range; so does any flag left not valid by the earlier steps.
    ///
    /// A month or day that does not hold two digits matches none of the
    /// 88-levels tested here, and a year that does not hold four digits is not
    /// divided (see the module notes).
    fn edit_day_month_year(&mut self) -> Flow {
        let month = two_digits(self.date.mm);
        let day = two_digits(self.date.dd);
        let thirty_one_day_month = matches!(month, Some(1 | 3 | 5 | 7 | 8 | 10 | 12));
        if !thirty_one_day_month && day == Some(31) {
            self.flags.day = FlagState::NotOk;
            self.flags.month = FlagState::NotOk;
            self.reject(CANNOT_HAVE_31_DAYS_IN_THIS_MONTH, self.fields.month);
            return Flow::ExitRange;
        }
        let february = month == Some(2);
        if february && day == Some(30) {
            self.flags.day = FlagState::NotOk;
            self.flags.month = FlagState::NotOk;
            self.reject(CANNOT_HAVE_30_DAYS_IN_THIS_MONTH, self.fields.month);
            return Flow::ExitRange;
        }
        if february && day == Some(29) && four_digits(self.date.ccyy).is_some_and(not_a_leap_year) {
            self.flags = DateEditFlags::INVALID;
            self.reject(NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS, self.fields.year);
            return Flow::ExitRange;
        }
        if self.flags.is_valid() {
            Flow::Continue
        } else {
            Flow::ExitRange
        }
    }

    /// CSUTLDPY EDIT-DATE-LE — P-CSUTLDPY-012, P-CSUTLDPY-014,
    /// V-COACTUPC-023, V-COACTUPC-036, V-COACTUPC-049, V-COACTUPC-076
    ///
    /// `CALL 'CSUTLDTC' USING WS-EDIT-DATE-CCYYMMDD, WS-DATE-FORMAT,
    /// WS-DATE-VALIDATION-RESULT` with the picture `YYYYMMDD`
    /// [app/cpy/CSUTLDPY.cpy:L284-297], then the severity test.
    ///
    /// # Errors
    ///
    /// [`DateValidationError::Unproven`] from `date_validation::validate`:
    /// E-26 holds every call with this picture at `halt`.
    fn edit_date_le(&mut self) -> Result<(), DateValidationError> {
        let result = validate(&self.csutldtc_date(), DatePicture::YyyyMmDd)?;
        self.check_severity(&result);
        Ok(())
    }

    /// CSUTLDPY EDIT-DATE-LE, EDIT-DATE-LE-EXIT — P-CSUTLDPY-012,
    /// V-COACTUPC-023, V-COACTUPC-036, V-COACTUPC-049, V-COACTUPC-076
    ///
    /// `IF WS-SEVERITY-N = 0` [app/cpy/CSUTLDPY.cpy:L298-328]: a non-zero
    /// severity writes `" validation error Sev code: "`, the four severity
    /// bytes, `" Message code: "` and the four message-number bytes after the
    /// trimmed label, on `ACCTSID`. That branch also sets the three flags not
    /// OK, but `GO TO EDIT-DATE-LE-EXIT` lands on `SET WS-EDIT-DATE-IS-VALID`,
    /// which follows the exit's `EXIT` in the same paragraph, so the range
    /// always leaves this step with the flags valid (E-45).
    fn check_severity(&mut self, result: &CsutldtcResult) {
        if !result.is_valid() {
            self.msg.set_if_off(
                date_validation_error_message(
                    self.field_name,
                    &result.severity_code(),
                    &result.msg_num(),
                ),
                FIELD_ACCTSID,
            );
        }
        self.flags = DateEditFlags::VALID;
    }

    /// The ten bytes `CSUTLDTC` reads as `LS-DATE`: `WS-EDIT-DATE-CCYYMMDD`
    /// and the two bytes after it ([`EDIT_DATE_BINARY_HIGH_BYTES`]).
    fn csutldtc_date(&self) -> [u8; LS_DATE_LEN] {
        let [y1, y2, y3, y4] = self.date.ccyy;
        let [m1, m2] = self.date.mm;
        let [d1, d2] = self.date.dd;
        let [b1, b2] = EDIT_DATE_BINARY_HIGH_BYTES;
        [y1, y2, y3, y4, m1, m2, d1, d2, b1, b2]
    }
}

/// CSUTLDPY EDIT-DAY-MONTH-YEAR — P-CSUTLDPY-011
///
/// `DIVIDE WS-EDIT-DATE-CCYY-N BY WS-DIV-BY … REMAINDER WS-REMAINDER` with
/// `WS-DIV-BY` 400 when `WS-EDIT-DATE-YY-N = 0`, else 4
/// [app/cpy/CSUTLDPY.cpy:L243-256]: a non-zero remainder is not a leap year,
/// so 1900 and 2100 are rejected and 2000 accepted.
fn not_a_leap_year(year: u32) -> bool {
    let divisor = if year % 100 == 0 { 400 } else { 4 };
    year % divisor != 0
}

/// CSUTLDPY EDIT-DATE-OF-BIRTH — P-CSUTLDPY-013
///
/// `WS-EDIT-DATE-CCYYMMDD-N` as `INTEGER-OF-DATE` reads it: the year, the
/// month and the day as `EDIT-DAY` converts it; `None` when the parts do not
/// form a calendar date.
fn birth_date(parts: DateParts) -> Option<NaiveDate> {
    let year = i32::try_from(four_digits(parts.ccyy)?).ok()?;
    let month = two_digits(parts.mm)?;
    let day = numval_unsigned_2(parts.dd).ok().flatten()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

/// The value of a `9(2)` redefine whose two bytes are digits; `None` when
/// either byte is not a digit.
fn two_digits([tens, units]: [u8; 2]) -> Option<u32> {
    let digit = |byte: u8| char::from(byte).to_digit(10);
    Some(digit(tens)? * 10 + digit(units)?)
}

/// The value of the `9(4)` redefine `WS-EDIT-DATE-CCYY-N` when its four bytes
/// are digits.
fn four_digits([c1, c2, y1, y2]: [u8; CCYY_LEN]) -> Option<u32> {
    Some(two_digits([c1, c2])? * 100 + two_digits([y1, y2])?)
}

/// `value` stored in an unsigned `9(2)` and read back as its two bytes: the
/// two low-order digits.
fn two_digit_bytes(value: u32) -> [u8; DD_LEN] {
    // `place % 10` always fits a u8; the fallback only keeps the conversion
    // free of a panic.
    let digit = |place: u32| u8::try_from(place % 10).map_or(b'0', |units| b'0' + units);
    [digit(value / 10), digit(value)]
}

/// CSUTLDPY EDIT-DAY — P-CSUTLDPY-008, V-COACTUPC-019
///
/// `FUNCTION TEST-NUMVAL(WS-EDIT-DATE-DD) = 0`, then `COMPUTE
/// WS-EDIT-DATE-DD-N = FUNCTION NUMVAL(WS-EDIT-DATE-DD)`
/// [app/cpy/CSUTLDPY.cpy:L170-185]: `Ok(None)` when the field is not a
/// `NUMVAL` argument, else the value the unsigned `9(2)` receives. The
/// receiver drops the sign, and `NUMVAL`, a floating-point function, puts a
/// fraction at the nearest integer (E-40, pinned there).
///
/// # Errors
///
/// [`DateValidationError::Unproven`] naming E-40 for a fraction of exactly one
/// half, the midpoint E-40 holds at `halt`.
fn numval_unsigned_2(field: [u8; DD_LEN]) -> Result<Option<u32>, DateValidationError> {
    let Some((integer, fraction)) = numval_argument(&field) else {
        return Ok(None);
    };
    let whole = integer.iter().fold(0, |value, &digit| {
        value * 10 + char::from(digit).to_digit(10).unwrap_or(0)
    });
    let rounded = match fraction.split_first() {
        None => whole,
        Some((&first, rest)) => match first {
            b'0'..=b'4' => whole,
            b'5' if rest.iter().all(|&digit| digit == b'0') => {
                return Err(DateValidationError::Unproven {
                    escalation_id: E_NUMVAL_MIDPOINT,
                });
            }
            _ => whole + 1,
        },
    };
    Ok(Some(rounded % 100))
}

/// CSUTLDPY EDIT-DAY `FUNCTION TEST-NUMVAL` — P-CSUTLDPY-008
///
/// The digits before and after the decimal point of `text` when it has one of
/// the `NUMVAL` argument forms: optional spaces, an optional `+` or `-` with
/// optional spaces, at least one digit with an optional decimal point
/// (`5`, `5.`, `.5`), optional spaces, and, without a leading sign, an
/// optional trailing `+` or `-` with optional spaces. The trailing `CR` and
/// `DB` forms need two bytes beside a digit, more than a two-byte day holds.
/// `None` for any other text; low-values are not spaces.
fn numval_argument(text: &[u8]) -> Option<(&[u8], &[u8])> {
    let rest = skip_spaces(text);
    let (leading_sign, rest) = match rest.split_first() {
        Some((b'+' | b'-', tail)) => (true, skip_spaces(tail)),
        _ => (false, rest),
    };
    let (integer, rest) = split_digits(rest);
    let (fraction, rest) = match rest.split_first() {
        Some((b'.', tail)) => split_digits(tail),
        _ => rest.split_at(0),
    };
    if integer.is_empty() && fraction.is_empty() {
        return None;
    }
    let rest = skip_spaces(rest);
    let rest = match rest.split_first() {
        Some((b'+' | b'-', tail)) if !leading_sign => skip_spaces(tail),
        _ => rest,
    };
    rest.is_empty().then_some((integer, fraction))
}

/// `text` without its leading spaces.
fn skip_spaces(text: &[u8]) -> &[u8] {
    let start = text
        .iter()
        .position(|&byte| byte != b' ')
        .unwrap_or(text.len());
    text.split_at(start).1
}

/// The leading digits of `text`, and the rest.
fn split_digits(text: &[u8]) -> (&[u8], &[u8]) {
    let end = text
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(text.len());
    text.split_at(end)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::FlagState::{Blank, NotOk, Valid};
    use super::{
        DateBmsFields, DateEdit, DateEditFlags, DateParts, FlagState, edit_date_ccyymmdd,
        edit_date_of_birth,
    };
    use crate::messages::{
        CANNOT_BE_IN_THE_FUTURE, CANNOT_HAVE_30_DAYS_IN_THIS_MONTH as DAYS_30,
        CANNOT_HAVE_31_DAYS_IN_THIS_MONTH as DAYS_31, CENTURY_NOT_VALID as CENTURY,
        DAY_MUST_BE_A_NUMBER_BETWEEN_1_AND_31 as DAY_RANGE, DAY_MUST_BE_SUPPLIED as DAY_SUPPLIED,
        FIELD_ACCOUNT_STATUS, FIELD_DATE_OF_BIRTH, FIELD_EXPIRY_DATE, FIELD_OPEN_DATE,
        FIELD_REISSUE_DATE, FIELD_SSN_FIRST_3_CHARS,
        MONTH_MUST_BE_A_NUMBER_BETWEEN_1_AND_12 as MONTH_RANGE,
        MONTH_MUST_BE_SUPPLIED as MONTH_SUPPLIED, MUST_BE_4_DIGIT_NUMBER as FOUR_DIGITS,
        MUST_BE_SUPPLIED, MUST_BE_Y_OR_N, NOT_A_LEAP_YEAR_CANNOT_HAVE_29_DAYS as LEAP,
        YEAR_MUST_BE_SUPPLIED as YEAR_SUPPLIED, date_edit_message, date_validation_error_message,
        field_edit_message,
    };
    use crate::rules::ReturnMsg;
    use crate::rules::date_validation::{DatePicture, DateValidationError, validate};

    /// The four dates `COACTUPC` edits, with their labels and fields.
    const DATES: [(&str, DateBmsFields); 4] = [
        (FIELD_OPEN_DATE, DateBmsFields::OPEN_DATE),
        (FIELD_EXPIRY_DATE, DateBmsFields::EXPIRY_DATE),
        (FIELD_REISSUE_DATE, DateBmsFields::REISSUE_DATE),
        (FIELD_DATE_OF_BIRTH, DateBmsFields::DATE_OF_BIRTH),
    ];

    const fn flags(year: FlagState, month: FlagState, day: FlagState) -> DateEditFlags {
        DateEditFlags { year, month, day }
    }

    const fn unproven(escalation_id: &'static str) -> DateValidationError {
        DateValidationError::Unproven { escalation_id }
    }

    /// The Open Date edit of the entered parts, and the message it left.
    fn edit_open(
        year: &str,
        month: &str,
        day: &str,
    ) -> (Result<DateEditFlags, DateValidationError>, ReturnMsg) {
        let mut msg = ReturnMsg::default();
        let parts = DateParts::from_entered(year, month, day);
        let result =
            edit_date_ccyymmdd(FIELD_OPEN_DATE, &parts, &mut msg, DateBmsFields::OPEN_DATE);
        (result, msg)
    }

    /// The working storage of an Open Date perform over the entered parts.
    fn open_edit<'m>(msg: &'m mut ReturnMsg, year: &str, month: &str, day: &str) -> DateEdit<'m> {
        DateEdit {
            field_name: FIELD_OPEN_DATE,
            fields: DateBmsFields::OPEN_DATE,
            date: DateParts::from_entered(year, month, day),
            flags: DateEditFlags::INVALID,
            msg,
        }
    }

    #[test]
    fn from_entered_moves_each_part_into_its_pic_x() {
        let cases = [
            (("2024", "4", ""), (*b"2024", *b"4 ", *b"  ")),
            (("202412", "123", "1234"), (*b"2024", *b"12", *b"12")),
            (("", "", ""), (*b"    ", *b"  ", *b"  ")),
        ];
        for ((year, month, day), (ccyy, mm, dd)) in cases {
            assert_eq!(
                DateParts::from_entered(year, month, day),
                DateParts { ccyy, mm, dd },
                "{year:?} {month:?} {day:?}"
            );
        }
    }

    /// Asserts that each Open Date case is rejected with the text of its
    /// suffix on `field`, the first message of the perform, and leaves its
    /// three flags.
    fn assert_rejections(field: &'static str, cases: &[([&str; 3], &str, [FlagState; 3])]) {
        for &([year, month, day], suffix, [y, m, d]) in cases {
            let (result, msg) = edit_open(year, month, day);
            assert_eq!(result, Ok(flags(y, m, d)), "{year:?} {month:?} {day:?}");
            assert_eq!(
                msg.into_parts(),
                Some((date_edit_message(FIELD_OPEN_DATE, suffix), field)),
                "{year:?} {month:?} {day:?}"
            );
        }
    }

    // P-CSUTLDPY-001 to P-CSUTLDPY-004, P-CSUTLDPY-011; V-COACTUPC-013 to
    // V-COACTUPC-015, V-COACTUPC-022. Every step runs and the first message
    // stands: a later failure only sets its flags.
    #[test]
    fn year_failures_are_on_the_year_field() {
        assert_rejections(
            "OPNYEAR",
            &[
                (["", "", ""], YEAR_SUPPLIED, [Blank, Blank, Blank]),
                (["", "04", "15"], YEAR_SUPPLIED, [Blank, Valid, Valid]),
                (["20A2", "04", "15"], FOUR_DIGITS, [NotOk, Valid, Valid]),
                (["1899", "04", "15"], CENTURY, [NotOk, Valid, Valid]),
                (["2105", "04", "15"], CENTURY, [NotOk, Valid, Valid]),
                (["20A2", "13", "32"], FOUR_DIGITS, [NotOk, NotOk, NotOk]),
                (["1900", "02", "29"], LEAP, [NotOk, NotOk, NotOk]),
                (["2023", "02", "29"], LEAP, [NotOk, NotOk, NotOk]),
                // The century text stands; the leap test still flags all three.
                (["2100", "02", "29"], CENTURY, [NotOk, NotOk, NotOk]),
                // A year that is not four digits is not divided.
                (["", "02", "29"], YEAR_SUPPLIED, [Blank, Valid, Valid]),
            ],
        );
    }

    // P-CSUTLDPY-005, P-CSUTLDPY-006, P-CSUTLDPY-009, P-CSUTLDPY-010;
    // V-COACTUPC-016, V-COACTUPC-017, V-COACTUPC-020, V-COACTUPC-021. The 31-
    // and 30-day edits flag the month before the day.
    #[test]
    fn month_failures_are_on_the_month_field() {
        assert_rejections(
            "OPNMON",
            &[
                (["2024", "", ""], MONTH_SUPPLIED, [Valid, Blank, Blank]),
                (["2024", "13", "15"], MONTH_RANGE, [Valid, NotOk, Valid]),
                (["2024", "00", "15"], MONTH_RANGE, [Valid, NotOk, Valid]),
                (["2024", "04", "31"], DAYS_31, [Valid, NotOk, NotOk]),
                (["2024", "02", "31"], DAYS_31, [Valid, NotOk, NotOk]),
                (["2024", "02", "30"], DAYS_30, [Valid, NotOk, NotOk]),
                // A blank month is no 31-day month.
                (["2024", "", "31"], MONTH_SUPPLIED, [Valid, NotOk, NotOk]),
            ],
        );
    }

    // P-CSUTLDPY-007, P-CSUTLDPY-008; V-COACTUPC-018, V-COACTUPC-019.
    #[test]
    fn day_failures_are_on_the_day_field() {
        assert_rejections(
            "OPNDAY",
            &[
                (["2024", "04", ""], DAY_SUPPLIED, [Valid, Valid, Blank]),
                (["2024", "04", "00"], DAY_RANGE, [Valid, Valid, NotOk]),
                (["2024", "04", "32"], DAY_RANGE, [Valid, Valid, NotOk]),
                (["2024", "04", "AB"], DAY_RANGE, [Valid, Valid, NotOk]),
                (["2024", "04", ".3"], DAY_RANGE, [Valid, Valid, NotOk]),
            ],
        );
    }

    // P-CSUTLDPY-002 to P-CSUTLDPY-011 for each date: V-COACTUPC-013 to -022,
    // -026 to -035, -039 to -048, -066 to -075.
    #[test]
    fn every_date_names_its_own_label_and_fields() {
        for (label, fields) in DATES {
            let cases = [
                (("", "04", "15"), YEAR_SUPPLIED, fields.year),
                (("2024", "13", "15"), MONTH_RANGE, fields.month),
                (("2024", "04", "32"), DAY_RANGE, fields.day),
                (("2024", "06", "31"), DAYS_31, fields.month),
                (("2023", "02", "29"), LEAP, fields.year),
            ];
            for ((year, month, day), suffix, field) in cases {
                let mut msg = ReturnMsg::default();
                let parts = DateParts::from_entered(year, month, day);
                assert!(edit_date_ccyymmdd(label, &parts, &mut msg, fields).is_ok());
                assert_eq!(
                    msg.into_parts(),
                    Some((date_edit_message(label, suffix), field))
                );
            }
        }
        let (_, msg) = edit_open("", "04", "15");
        let year_supplied = date_edit_message(FIELD_OPEN_DATE, YEAR_SUPPLIED);
        assert_eq!(msg.message(), Some(year_supplied.as_str()));
    }

    #[test]
    fn low_values_parts_are_blank() {
        let mut msg = ReturnMsg::default();
        let parts = DateParts {
            ccyy: [0; 4],
            mm: [0; 2],
            dd: [0; 2],
        };
        let result =
            edit_date_ccyymmdd(FIELD_OPEN_DATE, &parts, &mut msg, DateBmsFields::OPEN_DATE);
        assert_eq!(result, Ok(flags(Blank, Blank, Blank)));
        assert_eq!(
            msg.into_parts(),
            Some((date_edit_message(FIELD_OPEN_DATE, YEAR_SUPPLIED), "OPNYEAR"))
        );
    }

    // WS-RETURN-MSG-OFF: a message an earlier field set is kept.
    #[test]
    fn an_earlier_message_stands() {
        let mut msg = ReturnMsg::default();
        let status = field_edit_message(FIELD_ACCOUNT_STATUS, MUST_BE_Y_OR_N);
        msg.set_if_off(status.as_str(), "ACSTTUS");
        let parts = DateParts::from_entered("2024", "13", "15");
        let result =
            edit_date_ccyymmdd(FIELD_OPEN_DATE, &parts, &mut msg, DateBmsFields::OPEN_DATE);
        assert_eq!(result, Ok(flags(Valid, NotOk, Valid)));
        assert_eq!(msg.into_parts(), Some((status, "ACSTTUS")));
    }

    // P-CSUTLDPY-012, P-CSUTLDPY-014, E-26: a date that passes every edit
    // reaches CSUTLDTC, whose outcome is held at halt.
    #[test]
    fn dates_that_pass_every_edit_reach_the_csutldtc_halt() {
        let dates = [
            ("2024", "02", "29"),
            ("2000", "02", "29"),
            ("2024", "04", "30"),
            ("2024", "01", "31"),
            ("1999", "12", "31"),
            ("2024", "04", "5 "),
            ("2024", "04", "-5"),
        ];
        for (year, month, day) in dates {
            let (result, msg) = edit_open(year, month, day);
            assert_eq!(result, Err(unproven("E-26")), "{year:?} {month:?} {day:?}");
            assert!(msg.is_off(), "{year:?} {month:?} {day:?}");
        }
    }

    // E-47: a month holding a non-digit is tested as zoned data.
    #[test]
    fn a_month_holding_a_non_digit_is_the_e47_halt() {
        for month in [" 5", "5 ", "1A", "*1", " \u{0}"] {
            assert_eq!(
                edit_open("2024", month, "15").0,
                Err(unproven("E-47")),
                "{month:?}"
            );
        }
        assert_eq!(edit_open("", "5 ", "15").0, Err(unproven("E-47")));
    }

    // P-CSUTLDPY-008, V-COACTUPC-019, E-40: NUMVAL stored in the unsigned 9(2).
    #[test]
    fn edit_day_stores_the_numval_value_as_two_digits() {
        let cases = [
            ("15", *b"15", Valid),
            ("5 ", *b"05", Valid),
            (" 5", *b"05", Valid),
            ("+5", *b"05", Valid),
            ("-5", *b"05", Valid),
            ("5-", *b"05", Valid),
            ("5+", *b"05", Valid),
            ("5.", *b"05", Valid),
            (".7", *b"01", Valid),
            (".3", *b"00", NotOk),
            ("-0", *b"00", NotOk),
            ("32", *b"32", NotOk),
        ];
        for (day, stored, state) in cases {
            let mut msg = ReturnMsg::default();
            let mut edit = open_edit(&mut msg, "2024", "04", day);
            assert_eq!(edit.edit_day(), Ok(()), "{day:?}");
            assert_eq!((edit.date.dd, edit.flags.day), (stored, state), "{day:?}");
        }
    }

    // P-CSUTLDPY-008, V-COACTUPC-019: texts TEST-NUMVAL rejects keep their
    // bytes and are not OK.
    #[test]
    fn days_that_are_not_numval_arguments_are_rejected() {
        for day in ["AB", "5A", "+-", "+.", ". ", "--", " \u{0}"] {
            let mut msg = ReturnMsg::default();
            let mut edit = open_edit(&mut msg, "2024", "04", day);
            assert_eq!(edit.edit_day(), Ok(()), "{day:?}");
            assert_eq!(edit.flags.day, NotOk, "{day:?}");
            assert_eq!(
                edit.date.dd,
                DateParts::from_entered("", "", day).dd,
                "{day:?}"
            );
            assert_eq!(
                msg.into_parts(),
                Some((date_edit_message(FIELD_OPEN_DATE, DAY_RANGE), "OPNDAY")),
                "{day:?}"
            );
        }
    }

    // E-40: the 9(2) store of exactly one half is not guessed.
    #[test]
    fn a_day_of_one_half_is_the_e40_halt() {
        assert_eq!(edit_open("2024", "04", ".5").0, Err(unproven("E-40")));
    }

    // P-CSUTLDPY-012, P-CSUTLDPY-014: the 8 date bytes, then the two bytes
    // of WS-EDIT-DATE-BINARY the 10-byte linkage overlays.
    #[test]
    fn csutldtc_reads_the_date_and_two_more_bytes() {
        let mut msg = ReturnMsg::default();
        let mut edit = open_edit(&mut msg, "2024", "04", "5 ");
        assert_eq!(edit.edit_day(), Ok(()));
        assert_eq!(edit.csutldtc_date(), *b"20240405\0\0");
    }

    // P-CSUTLDPY-012, V-COACTUPC-023, E-45: a non-zero severity writes its
    // message on ACCTSID and the exit leaves the flags valid.
    #[test]
    fn a_csutldtc_rejection_names_its_codes_and_leaves_the_flags_valid() {
        let Ok(rejected) = validate(b"2024-02-30", DatePicture::YyyyMmDdDashed) else {
            panic!("the dashed picture pins 2024-02-30");
        };
        let mut msg = ReturnMsg::default();
        let mut edit = open_edit(&mut msg, "2024", "02", "30");
        edit.check_severity(&rejected);
        assert_eq!(edit.flags, DateEditFlags::VALID);
        let expected = date_validation_error_message(FIELD_OPEN_DATE, "0003", "2508");
        assert_eq!(msg.into_parts(), Some((expected, "ACCTSID")));

        let Ok(accepted) = validate(b"2024-02-29", DatePicture::YyyyMmDdDashed) else {
            panic!("the dashed picture pins 2024-02-29");
        };
        let mut msg = ReturnMsg::default();
        let mut edit = open_edit(&mut msg, "2024", "02", "29");
        edit.check_severity(&accepted);
        assert_eq!(edit.flags, DateEditFlags::VALID);
        assert!(msg.is_off());
    }

    // P-CSUTLDPY-013, V-COACTUPC-077: the clock date must exceed the birth
    // date, so today and later are rejected on DOBYEAR.
    #[test]
    fn a_birth_date_must_precede_today() {
        let Some(today) = NaiveDate::from_ymd_opt(2024, 6, 15) else {
            panic!("2024-06-15 is a date");
        };
        let future = date_edit_message(FIELD_DATE_OF_BIRTH, CANNOT_BE_IN_THE_FUTURE);
        let cases = [
            (("2024", "06", "14"), DateEditFlags::VALID, None),
            (("1980", "06", "5 "), DateEditFlags::VALID, None),
            (
                ("2024", "06", "15"),
                DateEditFlags::INVALID,
                Some(future.clone()),
            ),
            (
                ("2024", "06", "16"),
                DateEditFlags::INVALID,
                Some(future.clone()),
            ),
            (
                ("2023", "02", "29"),
                DateEditFlags::INVALID,
                Some(future.clone()),
            ),
        ];
        for ((year, month, day), expected, message) in cases {
            let mut msg = ReturnMsg::default();
            let parts = DateParts::from_entered(year, month, day);
            let result =
                edit_date_of_birth(FIELD_DATE_OF_BIRTH, &parts, today, &mut msg, "DOBYEAR");
            assert_eq!(result, expected, "{year:?} {month:?} {day:?}");
            assert_eq!(
                msg.message(),
                message.as_deref(),
                "{year:?} {month:?} {day:?}"
            );
            assert_eq!(msg.field(), message.as_ref().map(|_| "DOBYEAR"));
        }

        let mut msg = ReturnMsg::default();
        msg.set_if_off(
            field_edit_message(FIELD_SSN_FIRST_3_CHARS, MUST_BE_SUPPLIED),
            "ACTSSN1",
        );
        let parts = DateParts::from_entered("2025", "01", "01");
        let result = edit_date_of_birth(FIELD_DATE_OF_BIRTH, &parts, today, &mut msg, "DOBYEAR");
        assert_eq!(result, DateEditFlags::INVALID);
        assert_eq!(msg.field(), Some("ACTSSN1"));
    }

    // The fields named here are the COACTUP map's: year X(4), month and day
    // X(2), as the parts are.
    #[test]
    fn bms_fields_are_the_coactup_map_fields() {
        let map = include_str!("../../../../app/cpy-bms/COACTUP.CPY");
        for (_, fields) in DATES {
            for (field, width) in [(fields.year, 4), (fields.month, 2), (fields.day, 2)] {
                let declaration = format!(" {field}I  PIC X({width}).");
                assert!(map.contains(&declaration), "{declaration}");
            }
        }
    }

    #[test]
    fn flag_constants_are_the_88_levels() {
        assert!(DateEditFlags::VALID.is_valid());
        assert!(!DateEditFlags::INVALID.is_valid());
        assert!(!flags(Valid, Valid, Blank).is_valid());
    }
}
