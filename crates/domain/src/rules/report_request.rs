//! `CORPT00C` (transaction CR00, map `CORPT00`): the transaction report
//! request, `GET /reports/transactions`.
//!
//! `PROCESS-ENTER-KEY` [app/cbl/CORPT00C.cbl:L208-456] chooses the report and
//! its date range, and `SUBMIT-JOB-TO-INTRDR` [app/cbl/CORPT00C.cbl:L462-510]
//! asks for confirmation and writes the `TRANREPT` job to the TDQ `JOBS`, with
//! the two dates substituted into its `SYMNAMES` and `DATEPARM` lines; JES then
//! runs `CBTRN03C` over them. The replacement runs the report synchronously:
//! [`request`] performs the edits and returns the [`ReportRange`], and the
//! handler feeds that range to
//! [`ReportRun`](super::transaction_report::ReportRun), the `CBTRN03C` rules
//! `batch tranrept` shares (P-CORPT00C-012, E-34). Nothing is queued, so the
//! TDQ write failure `Unable to Write TDQ (JOBS)...` (P-CORPT00C-013) has no
//! counterpart and is not reproduced.
//!
//! # The first rejection answers
//!
//! `SEND-TRNRPT-SCREEN` ends the task with `GO TO RETURN-TO-CICS`
//! [app/cbl/CORPT00C.cbl:L556-591] (P-CORPT00C-002), so the first edit that
//! fails is the answer, in the COBOL order: the report type; for a custom
//! report the six empty checks in screen order, the six value edits in the
//! same order, then the start date and the end date through `CSUTLDTC`; last
//! the confirmation. The start date is never compared with the end date
//! (P-CORPT00C-008). Every rejection sets `WS-ERR-FLG`, so each one is a
//! [`DomainError`]; the confirmation `N`, which clears the screen and sets no
//! message, is one with an empty message (V-CORPT00C-017, E-49).
//!
//! # Input classes the audit holds at `halt`
//!
//! - A date part `FUNCTION NUMVAL-C` does not read as an argument (E-28), and
//!   a part longer than its map field, which no 3270 field can hold: the rule
//!   applies the E-28 decision that V-CORPT00C-008 to V-CORPT00C-013 record,
//!   rejecting the part with the value message of its own field when its edit
//!   is reached.
//! - A part with a fraction of exactly one half, which the unsigned receiver
//!   may keep rounded either way (E-40): the rule follows both values and
//!   answers [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected)
//!   only where the outcome depends on which one the receiver keeps.
//! - A date whose `CEEDAYS` feedback no audit row pins (E-31):
//!   [`date_validation`](super::date_validation) reports it unproven, and the
//!   rule answers it as unexpected.
//!
//! Time comes from the caller: the handler passes the processing date of its
//! [`Clock`](crate::clock::Clock) for `FUNCTION CURRENT-DATE`
//! [app/cbl/CORPT00C.cbl:L215] [app/cbl/CORPT00C.cbl:L241].

use std::iter;

use chrono::{Datelike, NaiveDate};

use super::Confirm;
use super::date_validation::{DatePicture, DateValidationError, validate};
use crate::error::DomainError;
use crate::messages::{
    END_DATE_DAY_CAN_NOT_BE_EMPTY, END_DATE_MONTH_CAN_NOT_BE_EMPTY, END_DATE_NOT_A_VALID_DATE,
    END_DATE_NOT_A_VALID_DAY, END_DATE_NOT_A_VALID_MONTH, END_DATE_NOT_A_VALID_YEAR,
    END_DATE_YEAR_CAN_NOT_BE_EMPTY, REPORT_NAME_CUSTOM, REPORT_NAME_MONTHLY, REPORT_NAME_YEARLY,
    SELECT_A_REPORT_TYPE_TO_PRINT_REPORT, START_DATE_DAY_CAN_NOT_BE_EMPTY,
    START_DATE_MONTH_CAN_NOT_BE_EMPTY, START_DATE_NOT_A_VALID_DATE, START_DATE_NOT_A_VALID_DAY,
    START_DATE_NOT_A_VALID_MONTH, START_DATE_NOT_A_VALID_YEAR, START_DATE_YEAR_CAN_NOT_BE_EMPTY,
    UNEXPECTED_ERROR, confirm_print_report_message, invalid_confirm_value_message,
    report_submitted_message,
};
use crate::numeric::Alnum;

/// The BMS field `MONTHLY` (`MONTHLYI X(1)` [app/cpy-bms/CORPT00.CPY:L60]):
/// the cursor field of the missing report type and of the screen the
/// confirmation `N` clears (`MOVE -1 TO MONTHLYL`, V-CORPT00C-001,
/// V-CORPT00C-017).
pub const FIELD_MONTHLY: &str = "MONTHLY";

/// The BMS field `SDTMM` (`SDTMMI X(2)` [app/cpy-bms/CORPT00.CPY:L78]): the
/// start month, and the cursor field of the start date check
/// (V-CORPT00C-002, V-CORPT00C-008, V-CORPT00C-014).
pub const FIELD_SDTMM: &str = "SDTMM";

/// The BMS field `SDTDD` (`SDTDDI X(2)` [app/cpy-bms/CORPT00.CPY:L84]): the
/// start day (V-CORPT00C-003, V-CORPT00C-009).
pub const FIELD_SDTDD: &str = "SDTDD";

/// The BMS field `SDTYYYY` (`SDTYYYYI X(4)` [app/cpy-bms/CORPT00.CPY:L90]):
/// the start year (V-CORPT00C-004, V-CORPT00C-010).
pub const FIELD_SDTYYYY: &str = "SDTYYYY";

/// The BMS field `EDTMM` (`EDTMMI X(2)` [app/cpy-bms/CORPT00.CPY:L96]): the end
/// month, and the cursor field of the end date check (V-CORPT00C-005,
/// V-CORPT00C-011, V-CORPT00C-015).
pub const FIELD_EDTMM: &str = "EDTMM";

/// The BMS field `EDTDD` (`EDTDDI X(2)` [app/cpy-bms/CORPT00.CPY:L102]): the
/// end day (V-CORPT00C-006, V-CORPT00C-012).
pub const FIELD_EDTDD: &str = "EDTDD";

/// The BMS field `EDTYYYY` (`EDTYYYYI X(4)` [app/cpy-bms/CORPT00.CPY:L108]):
/// the end year (V-CORPT00C-007, V-CORPT00C-013).
pub const FIELD_EDTYYYY: &str = "EDTYYYY";

/// The BMS field `CONFIRM` (`CONFIRMI X(1)` [app/cpy-bms/CORPT00.CPY:L114]):
/// the cursor field of the confirmation prompt and of an invalid confirmation
/// (`MOVE -1 TO CONFIRML`, V-CORPT00C-016, V-CORPT00C-018).
pub const FIELD_CONFIRM: &str = "CONFIRM";

/// The text of the confirmation `N`, which sets no message (V-CORPT00C-017,
/// E-49); its status alone tells the outcome.
const NO_MESSAGE: &str = "";

/// `WS-DATE-FORMAT PIC X(10) VALUE 'YYYY-MM-DD'` [app/cbl/CORPT00C.cbl:L72],
/// the picture both `CSUTLDTC` calls pass (P-CORPT00C-008).
const DATE_PICTURE: DatePicture = DatePicture::YyyyMmDdDashed;

/// The default currency sign `NUMVAL-C` reads: `CORPT00C` declares no
/// `CURRENCY SIGN` clause [app/cbl/CORPT00C.cbl:L27-28].
const CURRENCY_SIGN: u8 = b'$';

/// The first and last years of the `FUNCTION INTEGER-OF-DATE` and
/// `DATE-OF-INTEGER` domain, 1601-01-01 to 9999-12-31, which the monthly end
/// date passes through [app/cbl/CORPT00C.cbl:L229-230].
const INTEGER_DATE_YEARS: (i32, i32) = (1601, 9999);

/// `WS-START-DATE-YYYY PIC X(04)` filled from `WS-CURDATE-YEAR PIC 9(04)`
/// [app/cbl/CORPT00C.cbl:L61] [app/cpy/CSDAT01Y.cpy:L20]: the year keeps its
/// four low-order digits.
const YEAR_MODULUS: i32 = 10_000;

/// The report type chosen on the map: the `MONTHLY`, `YEARLY` or `CUSTOM`
/// selector.
///
/// The `EVALUATE TRUE` tests the three selectors in that order
/// [app/cbl/CORPT00C.cbl:L212-256] (P-CORPT00C-003), so a screen with more
/// than one entered runs the first; the API's single `report_type` names one
/// (E-48).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportType {
    /// `MONTHLYI` entered: the current calendar month (P-CORPT00C-003).
    Monthly,
    /// `YEARLYI` entered: the current calendar year (P-CORPT00C-004).
    Yearly,
    /// `CUSTOMI` entered: the dates of the six date parts (P-CORPT00C-005 to
    /// P-CORPT00C-008).
    Custom,
}

impl ReportType {
    /// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-010, P-CORPT00C-014,
    /// V-CORPT00C-016
    ///
    /// `WS-REPORT-NAME`: `Monthly`, `Yearly` or `Custom`
    /// [app/cbl/CORPT00C.cbl:L214] [app/cbl/CORPT00C.cbl:L240]
    /// [app/cbl/CORPT00C.cbl:L433].
    const fn report_name(self) -> &'static str {
        match self {
            Self::Monthly => REPORT_NAME_MONTHLY,
            Self::Yearly => REPORT_NAME_YEARLY,
            Self::Custom => REPORT_NAME_CUSTOM,
        }
    }
}

/// The six date parts of a custom report as entered: `SDTMMI X(2)`,
/// `SDTDDI X(2)`, `SDTYYYYI X(4)`, `EDTMMI X(2)`, `EDTDDI X(2)` and
/// `EDTYYYYI X(4)` [app/cpy-bms/CORPT00.CPY:L78-108].
///
/// The API splits `start_date` and `end_date` at their hyphens into these
/// parts, an empty part standing for a blank field (E-48). Monthly and yearly
/// reports read none of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CustomDates {
    /// `SDTMM`: the start month.
    pub start_month: String,
    /// `SDTDD`: the start day.
    pub start_day: String,
    /// `SDTYYYY`: the start year.
    pub start_year: String,
    /// `EDTMM`: the end month.
    pub end_month: String,
    /// `EDTDD`: the end day.
    pub end_day: String,
    /// `EDTYYYY`: the end year.
    pub end_year: String,
}

/// The `CORPT0A` screen input of one ENTER.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReportRequestInput {
    /// The selected report; `None` when no selector was entered
    /// (V-CORPT00C-001).
    pub report_type: Option<ReportType>,
    /// The date parts, read only for [`ReportType::Custom`].
    pub custom: CustomDates,
    /// `CONFIRM`: `Y` runs the report, blank asks for confirmation, `N`
    /// clears the screen, any other value is rejected. The API's confirmation
    /// dialog stands for `Y` (E-48).
    pub confirm: Confirm,
}

/// The report a confirmed request runs: its name and the dates the `TRANREPT`
/// job would carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReportRange {
    /// `WS-REPORT-NAME`: `Monthly`, `Yearly` or `Custom`.
    pub report_name: &'static str,
    /// `WS-START-DATE`, `YYYY-MM-DD`: `PARM-START-DATE-1` and `-2` of the job's
    /// `SYMNAMES` and `DATEPARM` lines [app/cbl/CORPT00C.cbl:L103-121]
    /// (P-CORPT00C-017).
    pub start: Alnum<10>,
    /// `WS-END-DATE`, `YYYY-MM-DD`: `PARM-END-DATE-1` and `-2`.
    pub end: Alnum<10>,
}

/// The outcome of a request every edit accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReportRequestOutcome {
    /// Confirmation `Y`: the report runs over `range` (P-CORPT00C-012), and
    /// the screen shows `message`, `<name> report submitted for printing ...`
    /// (P-CORPT00C-014), over a form `INITIALIZE-ALL-FIELDS` has emptied
    /// (P-CORPT00C-021).
    Submitted {
        /// The report and its dates.
        range: ReportRange,
        /// `WS-MESSAGE` after the job was written.
        message: String,
    },
}

/// The dates the edits leave for the report.
enum Dates {
    /// The dates the job would carry.
    Pinned(ReportRange),
    /// Dates every edit accepted while a part's value is unpinned (E-40): the
    /// confirmation still answers as the COBOL does whatever the value, and a
    /// confirmed run answers this error.
    Unpinned(DomainError),
}

/// CORPT00C PROCESS-ENTER-KEY, SUBMIT-JOB-TO-INTRDR — P-CORPT00C-002 to
/// P-CORPT00C-012, P-CORPT00C-014 to P-CORPT00C-017, P-CORPT00C-021;
/// V-CORPT00C-001 to V-CORPT00C-018.
///
/// The edits of one ENTER on `CORPT0A`, in the COBOL order, the first failure
/// answering ([app/cbl/CORPT00C.cbl:L208-510], P-CORPT00C-002):
///
/// 1. No report type selected is rejected (V-CORPT00C-001).
/// 2. Monthly runs from the first to the last day of the month of `today`, and
///    yearly from 1 January to 31 December of its year; neither has an edit.
///    A custom report edits its six date parts and checks both dates through
///    `CSUTLDTC`.
/// 3. The confirmation: `Y` returns [`ReportRequestOutcome::Submitted`];
///    blank, `N` and any other value are rejected.
///
/// `today` stands for `FUNCTION CURRENT-DATE`; the handler takes it from its
/// [`Clock`](crate::clock::Clock).
///
/// # Errors
///
/// The [`DomainError`] of the first failing edit:
///
/// - [`ErrorKind::Empty`](crate::error::ErrorKind::Empty):
///   `Select a report type to print report...` on [`FIELD_MONTHLY`]
///   (V-CORPT00C-001); the first blank custom part's
///   `… can NOT be empty...` text on its field (V-CORPT00C-002 to
///   V-CORPT00C-007).
/// - [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid): the first
///   custom part failing its value edit, `… Not a valid Month...`, `Day` or
///   `Year` on its field (V-CORPT00C-008 to V-CORPT00C-013); a date
///   `CSUTLDTC` rejects, `Start Date - Not a valid date...` on
///   [`FIELD_SDTMM`] or `End Date - Not a valid date...` on [`FIELD_EDTMM`]
///   (V-CORPT00C-014, V-CORPT00C-015); a blank confirmation,
///   `Please confirm to print the <name> report...` on [`FIELD_CONFIRM`]
///   (V-CORPT00C-016); `N`, no message, on [`FIELD_MONTHLY`]
///   (V-CORPT00C-017); any other value, `"<value>" is not a valid value to
///   confirm...` on [`FIELD_CONFIRM`] (V-CORPT00C-018).
/// - [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected),
///   `Unexpected error`, where no reviewed row pins the COBOL result: a
///   monthly report whose end date leaves the `INTEGER-OF-DATE` domain, on
///   [`FIELD_MONTHLY`]; a custom date `CSUTLDTC` has no pinned feedback for
///   (E-31), on the field of its check; an outcome that depends on how a
///   midpoint fraction is stored (E-40), on the field whose edit or check
///   depends on it.
pub fn request(
    input: &ReportRequestInput,
    today: NaiveDate,
) -> Result<ReportRequestOutcome, DomainError> {
    let Some(report_type) = input.report_type else {
        return Err(
            DomainError::empty(SELECT_A_REPORT_TYPE_TO_PRINT_REPORT).with_field(FIELD_MONTHLY)
        );
    };
    let dates = match report_type {
        ReportType::Monthly => Dates::Pinned(monthly_range(today)?),
        ReportType::Yearly => Dates::Pinned(yearly_range(today)),
        ReportType::Custom => custom_dates(&input.custom)?,
    };
    let report_name = report_type.report_name();
    confirm(report_name, &input.confirm)?;
    match dates {
        Dates::Pinned(range) => Ok(ReportRequestOutcome::Submitted {
            range,
            message: report_submitted_message(report_name),
        }),
        Dates::Unpinned(error) => Err(error),
    }
}

/// CORPT00C SUBMIT-JOB-TO-INTRDR — P-CORPT00C-010 to P-CORPT00C-012,
/// P-CORPT00C-021; V-CORPT00C-016 to V-CORPT00C-018.
///
/// The confirmation test before the job is written
/// [app/cbl/CORPT00C.cbl:L464-494]: blank sets the prompt naming the report,
/// `N` or `n` runs `INITIALIZE-ALL-FIELDS` and sets the error flag with no
/// message, `Y` or `y` goes on, and any other value is quoted back, cut at its
/// first space.
fn confirm(report_name: &str, confirm: &Confirm) -> Result<(), DomainError> {
    match confirm {
        Confirm::Yes => Ok(()),
        Confirm::Blank => Err(
            DomainError::invalid(confirm_print_report_message(report_name))
                .with_field(FIELD_CONFIRM),
        ),
        Confirm::No => Err(DomainError::invalid(NO_MESSAGE).with_field(FIELD_MONTHLY)),
        Confirm::Other(value) => {
            Err(DomainError::invalid(invalid_confirm_value_message(value))
                .with_field(FIELD_CONFIRM))
        }
    }
}

/// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-003, P-CORPT00C-017
///
/// The monthly range [app/cbl/CORPT00C.cbl:L213-238]: the start date is the
/// current year and month with day `01`; for the end date the day is set to 1,
/// the month advanced (December rolling into January of the next year), and
/// `DATE-OF-INTEGER(INTEGER-OF-DATE(...) - 1)` gives the day before, the last
/// day of the current month.
///
/// # Errors
///
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) on
/// [`FIELD_MONTHLY`] when the first of the next month or the day before it
/// lies outside 1601-01-01 to 9999-12-31, where the two functions have no
/// defined result: a processing date in December 9999 (whose year `ADD 1`
/// takes beyond the `9(04)` field) or before 1601-01-01.
fn monthly_range(today: NaiveDate) -> Result<ReportRange, DomainError> {
    let month = today.month();
    let (next_year, next_month) = if month == 12 {
        (today.year() + 1, 1)
    } else {
        (today.year(), month + 1)
    };
    // No reviewed row pins INTEGER-OF-DATE or DATE-OF-INTEGER outside their
    // domain, and FUNCTION CURRENT-DATE never leaves it; a clock pinned there
    // gets the unexpected-error answer rather than a guessed date.
    let end = NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .filter(|first| in_integer_date_domain(*first))
        .and_then(|first| first.pred_opt())
        .filter(|end| in_integer_date_domain(*end));
    let Some(end) = end else {
        return Err(unexpected(FIELD_MONTHLY));
    };
    Ok(ReportRange {
        report_name: REPORT_NAME_MONTHLY,
        start: date_text(current_year(today), month, 1),
        end: date_text(current_year(end), end.month(), end.day()),
    })
}

/// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-004, P-CORPT00C-017
///
/// The yearly range [app/cbl/CORPT00C.cbl:L239-255]: the current year with
/// month and day `01` to `12` and `31`.
fn yearly_range(today: NaiveDate) -> ReportRange {
    let year = current_year(today);
    ReportRange {
        report_name: REPORT_NAME_YEARLY,
        start: date_text(year, 1, 1),
        end: date_text(year, 12, 31),
    }
}

/// Whether `date` lies in the `INTEGER-OF-DATE` and `DATE-OF-INTEGER` domain.
fn in_integer_date_domain(date: NaiveDate) -> bool {
    let (first, last) = INTEGER_DATE_YEARS;
    (first..=last).contains(&date.year())
}

/// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-003, P-CORPT00C-004, P-CORPT00C-017
///
/// `MOVE WS-CURDATE-YEAR TO WS-START-DATE-YYYY`: the year of `date` as the
/// four low-order digits of the `9(04)` field.
fn current_year(date: NaiveDate) -> u32 {
    // rem_euclid leaves 0 to 9999, which always converts; the fallback only
    // keeps the conversion free of a panic.
    u32::try_from(date.year().rem_euclid(YEAR_MODULUS)).unwrap_or_default()
}

/// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-003, P-CORPT00C-004,
/// P-CORPT00C-008, P-CORPT00C-017
///
/// The `X(10)` date of `WS-START-DATE` or `WS-END-DATE`
/// [app/cbl/CORPT00C.cbl:L60-71]: four year digits, `-`, two month digits,
/// `-`, two day digits, each part keeping the low-order digits of its field.
fn date_text(year: u32, month: u32, day: u32) -> Alnum<10> {
    Alnum::move_from(&format!(
        "{:04}-{:02}-{:02}",
        year % 10_000,
        month % 100,
        day % 100
    ))
}

/// The unexpected-error answer for an outcome no reviewed row pins, on
/// `field`.
fn unexpected(field: &'static str) -> DomainError {
    DomainError::unexpected(UNEXPECTED_ERROR).with_field(field)
}

/// The value edit of one custom date part, applied to the digits its
/// `NUMVAL-C` conversion moved back into the map field
/// [app/cbl/CORPT00C.cbl:L329-379] (P-CORPT00C-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ValueEdit {
    /// A month or day: `IS NOT NUMERIC OR > limit`, the alphanumeric
    /// comparison of the two digits with the literal, so `00` passes.
    AtMost(&'static [u8; 2]),
    /// A year: `IS NOT NUMERIC`. The four digits moved back always pass it, so
    /// only a part the conversion does not read fails.
    Numeric,
}

impl ValueEdit {
    /// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-007;
    /// V-CORPT00C-008 to V-CORPT00C-013.
    ///
    /// Whether the edit rejects the field holding the digits of `stored`.
    fn rejects(self, stored: u32) -> bool {
        match self {
            Self::AtMost(limit) => {
                let field = Alnum::<2>::move_from(&format!("{:02}", stored % 100));
                !field.is_numeric() || field.cobol_cmp(limit).is_gt()
            }
            Self::Numeric => {
                !Alnum::<4>::move_from(&format!("{:04}", stored % 10_000)).is_numeric()
            }
        }
    }
}

/// The edits of one custom date part: its field and its two messages.
struct PartRule {
    /// The BMS field of both messages.
    field: &'static str,
    /// The empty-check message [app/cbl/CORPT00C.cbl:L258-303].
    empty: &'static str,
    /// The value-edit message [app/cbl/CORPT00C.cbl:L329-379].
    invalid: &'static str,
    /// The value edit.
    edit: ValueEdit,
}

/// The month limit `'12'` [app/cbl/CORPT00C.cbl:L330]
/// [app/cbl/CORPT00C.cbl:L356].
const MONTH_LIMIT: &[u8; 2] = b"12";

/// The day limit `'31'` [app/cbl/CORPT00C.cbl:L339]
/// [app/cbl/CORPT00C.cbl:L365].
const DAY_LIMIT: &[u8; 2] = b"31";

/// `SDTMM` — V-CORPT00C-002, V-CORPT00C-008.
const START_MONTH: PartRule = PartRule {
    field: FIELD_SDTMM,
    empty: START_DATE_MONTH_CAN_NOT_BE_EMPTY,
    invalid: START_DATE_NOT_A_VALID_MONTH,
    edit: ValueEdit::AtMost(MONTH_LIMIT),
};

/// `SDTDD` — V-CORPT00C-003, V-CORPT00C-009.
const START_DAY: PartRule = PartRule {
    field: FIELD_SDTDD,
    empty: START_DATE_DAY_CAN_NOT_BE_EMPTY,
    invalid: START_DATE_NOT_A_VALID_DAY,
    edit: ValueEdit::AtMost(DAY_LIMIT),
};

/// `SDTYYYY` — V-CORPT00C-004, V-CORPT00C-010.
const START_YEAR: PartRule = PartRule {
    field: FIELD_SDTYYYY,
    empty: START_DATE_YEAR_CAN_NOT_BE_EMPTY,
    invalid: START_DATE_NOT_A_VALID_YEAR,
    edit: ValueEdit::Numeric,
};

/// `EDTMM` — V-CORPT00C-005, V-CORPT00C-011.
const END_MONTH: PartRule = PartRule {
    field: FIELD_EDTMM,
    empty: END_DATE_MONTH_CAN_NOT_BE_EMPTY,
    invalid: END_DATE_NOT_A_VALID_MONTH,
    edit: ValueEdit::AtMost(MONTH_LIMIT),
};

/// `EDTDD` — V-CORPT00C-006, V-CORPT00C-012.
const END_DAY: PartRule = PartRule {
    field: FIELD_EDTDD,
    empty: END_DATE_DAY_CAN_NOT_BE_EMPTY,
    invalid: END_DATE_NOT_A_VALID_DAY,
    edit: ValueEdit::AtMost(DAY_LIMIT),
};

/// `EDTYYYY` — V-CORPT00C-007, V-CORPT00C-013.
const END_YEAR: PartRule = PartRule {
    field: FIELD_EDTYYYY,
    empty: END_DATE_YEAR_CAN_NOT_BE_EMPTY,
    invalid: END_DATE_NOT_A_VALID_YEAR,
    edit: ValueEdit::Numeric,
};

/// The value an unsigned receiver keeps from `FUNCTION NUMVAL-C` of a part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stored {
    /// The rounded value, reduced to the receiver's digits.
    value: u32,
    /// For a fraction of exactly one half, the value rounded the other way:
    /// E-40 does not pin which of the two the receiver keeps.
    alternative: Option<u32>,
}

impl Stored {
    /// Every value the receiver may hold: one, or two for a midpoint.
    fn candidates(self) -> impl Iterator<Item = u32> {
        iter::once(self.value).chain(self.alternative)
    }
}

/// One custom date part as `RECEIVE MAP` and the conversion leave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Entry {
    /// `= SPACES OR LOW-VALUES` on the map field.
    blank: bool,
    /// The converted value; `None` for text `NUMVAL-C` does not read as an
    /// argument the audit pins, and for text longer than the map field.
    stored: Option<Stored>,
}

/// An argument `FUNCTION NUMVAL-C` reads, split at its decimal point.
struct NumvalC<'a> {
    /// The digits before the decimal point.
    integer: &'a [u8],
    /// The digits after it.
    fraction: &'a [u8],
}

impl<'a> NumvalC<'a> {
    /// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-006, P-CORPT00C-015,
    /// P-CORPT00C-016
    ///
    /// `text` read as a `NUMVAL-C` argument, `None` when it is not one. The two
    /// IBM formats are read, with `sp` one or more spaces, `cs` the currency
    /// sign `$` and `d` one or more digits:
    /// `[sp][+|-][sp][cs][sp]{d[.[d]] | .d}[sp]` and
    /// `[sp][cs][sp]{d[.[d]] | .d}[sp][+|-|CR|DB][sp]`.
    ///
    /// Commas and a lower-case `cr` or `db`, whose treatment the audit does not
    /// pin, are read as text that is not an argument; with the malformed text
    /// of E-28 they take the E-28 decision. `LOW-VALUES` never are an argument.
    fn parse(text: &'a [u8]) -> Option<Self> {
        let rest = skip_spaces(text);
        let (signed, rest) = match rest {
            [b'+' | b'-', tail @ ..] => (true, skip_spaces(tail)),
            _ => (false, rest),
        };
        let rest = match rest {
            [CURRENCY_SIGN, tail @ ..] => skip_spaces(tail),
            _ => rest,
        };
        let (integer, rest) = split_digits(rest);
        let (fraction, rest) = match rest {
            [b'.', tail @ ..] => split_digits(tail),
            _ => rest.split_at(0),
        };
        if integer.is_empty() && fraction.is_empty() {
            return None;
        }
        let rest = skip_spaces(rest);
        let rest = match rest {
            [b'+' | b'-', tail @ ..] if !signed => skip_spaces(tail),
            [b'C', b'R', tail @ ..] | [b'D', b'B', tail @ ..] if !signed => skip_spaces(tail),
            _ => rest,
        };
        rest.is_empty().then_some(Self { integer, fraction })
    }

    /// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-015
    ///
    /// The value `COMPUTE WS-NUM-99` or `WS-NUM-9999` stores, for a receiver
    /// of `modulus` values: `NUMVAL-C` is evaluated in floating point, so a
    /// fraction rounds to the nearest integer (E-40), a sign is dropped by the
    /// unsigned receiver, and the receiver keeps the low-order digits. A
    /// fraction of exactly one half keeps both roundings as candidates.
    fn store(&self, modulus: u32) -> Stored {
        let value = self
            .integer
            .iter()
            .fold(0, |value, &digit| value * 10 + u32::from(digit - b'0'));
        // No fraction, or one below one half, keeps the integer.
        let (value, midpoint) = match self.fraction {
            [b'5', rest @ ..] if rest.iter().all(|&digit| digit == b'0') => (value, true),
            [first, ..] if *first >= b'5' => (value + 1, false),
            _ => (value, false),
        };
        Stored {
            value: value % modulus,
            alternative: midpoint.then(|| (value + 1) % modulus),
        }
    }
}

/// `text` without its leading spaces.
fn skip_spaces(text: &[u8]) -> &[u8] {
    let start = text
        .iter()
        .position(|&byte| byte != b' ')
        .unwrap_or(text.len());
    text.split_at(start).1
}

/// The leading ASCII digits of `text`, and the rest.
fn split_digits(text: &[u8]) -> (&[u8], &[u8]) {
    let end = text
        .iter()
        .position(|byte| !byte.is_ascii_digit())
        .unwrap_or(text.len());
    text.split_at(end)
}

/// `10^digits`: the number of values of an unsigned `9(digits)` receiver.
const fn receiver_modulus(digits: usize) -> u32 {
    let mut modulus = 1;
    let mut left = digits;
    while left > 0 {
        modulus *= 10;
        left -= 1;
    }
    modulus
}

/// CORPT00C RECEIVE-TRNRPT-SCREEN, PROCESS-ENTER-KEY — P-CORPT00C-005,
/// P-CORPT00C-006, P-CORPT00C-015, P-CORPT00C-016, P-CORPT00C-019
///
/// One date part received into its `X(N)` map field, then converted by
/// `COMPUTE WS-NUM-… = FUNCTION NUMVAL-C(…)` and moved back
/// [app/cbl/CORPT00C.cbl:L305-327]. Each map field is as wide as its receiver,
/// `X(2)` into `WS-NUM-99 PIC 99` and `X(4)` into `WS-NUM-9999 PIC 9999`
/// [app/cbl/CORPT00C.cbl:L74-75], so `N` sizes both.
///
/// Text longer than the field has no 3270 counterpart. It is rejected rather
/// than cut, so no entered character is dropped unseen: it is not blank, and
/// its value edit fails as a malformed part's does.
fn receive<const N: usize>(text: &str) -> Entry {
    if text.len() > N {
        return Entry {
            blank: false,
            stored: None,
        };
    }
    let field = Alnum::<N>::move_from(text);
    Entry {
        blank: field.is_blank(),
        stored: NumvalC::parse(field.as_bytes()).map(|number| number.store(receiver_modulus(N))),
    }
}

/// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-007, P-CORPT00C-016;
/// V-CORPT00C-008 to V-CORPT00C-013.
///
/// The value edit of one part [app/cbl/CORPT00C.cbl:L329-379], returning the
/// value it passed with.
///
/// A part `NUMVAL-C` does not read, or one longer than its field, has a value
/// the COBOL leaves unpredictable (E-28, at `halt`); it gets the outcome the
/// E-28 decision states and V-CORPT00C-008 to V-CORPT00C-013 record: the
/// part's own value message. A midpoint whose two candidate values the edit
/// treats differently is unexpected (E-40).
fn edit_part(rule: &PartRule, stored: Option<Stored>) -> Result<Stored, DomainError> {
    let invalid = || DomainError::invalid(rule.invalid).with_field(rule.field);
    let Some(stored) = stored else {
        return Err(invalid());
    };
    let mut verdicts = stored.candidates().map(|value| rule.edit.rejects(value));
    let rejected = verdicts.next().unwrap_or_default();
    if verdicts.any(|other| other != rejected) {
        return Err(unexpected(rule.field));
    }
    if rejected { Err(invalid()) } else { Ok(stored) }
}

/// What `CSUTLDTC` answers for one candidate date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// Severity `0000` or message number `2513`.
    Accepted,
    /// Any other pinned feedback.
    Rejected,
    /// Feedback no reviewed row pins (E-31).
    Unproven,
}

/// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-008; V-CORPT00C-014,
/// V-CORPT00C-015.
///
/// `CALL 'CSUTLDTC'` with the date built from the parts and the picture
/// `YYYY-MM-DD` [app/cbl/CORPT00C.cbl:L381-426]: the date passes when the
/// severity is `0000` or the message number `2513`, so a calendar date before
/// 1582-10-15 passes too (P-CSUTLDTC-004).
///
/// Every candidate date the parts allow is checked. The date is
/// `Some` when it passed and is pinned, `None` when it passed for each of
/// several candidates (E-40).
///
/// # Errors
///
/// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid), `message` on
/// `field`, when `CSUTLDTC` rejects every candidate;
/// [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) on `field`
/// when a candidate's feedback is unproven (E-31) or the candidates are
/// answered differently (E-40).
fn check_date(
    [year, month, day]: [Stored; 3],
    message: &'static str,
    field: &'static str,
) -> Result<Option<Alnum<10>>, DomainError> {
    let checked: Vec<(Alnum<10>, Verdict)> = year
        .candidates()
        .flat_map(|year| {
            month
                .candidates()
                .flat_map(move |month| day.candidates().map(move |day| date_text(year, month, day)))
        })
        .map(|date| {
            let verdict = match validate(date.as_bytes(), DATE_PICTURE) {
                Ok(result) if result.accepts_with_2513() => Verdict::Accepted,
                Ok(_) => Verdict::Rejected,
                Err(DateValidationError::Unproven { .. }) => Verdict::Unproven,
            };
            (date, verdict)
        })
        .collect();
    if checked
        .iter()
        .all(|&(_, verdict)| verdict == Verdict::Rejected)
    {
        return Err(DomainError::invalid(message).with_field(field));
    }
    if checked
        .iter()
        .any(|&(_, verdict)| verdict != Verdict::Accepted)
    {
        return Err(unexpected(field));
    }
    Ok(match checked.as_slice() {
        [(date, _)] => Some(*date),
        _ => None,
    })
}

/// CORPT00C PROCESS-ENTER-KEY — P-CORPT00C-005 to P-CORPT00C-008,
/// P-CORPT00C-015 to P-CORPT00C-017; V-CORPT00C-002 to V-CORPT00C-015.
///
/// The custom range [app/cbl/CORPT00C.cbl:L256-436]: the empty checks of the
/// six parts in screen order, their `NUMVAL-C` conversion, their value edits
/// in the same order, then the start and the end date through `CSUTLDTC`.
fn custom_dates(dates: &CustomDates) -> Result<Dates, DomainError> {
    let parts = [
        (&START_MONTH, receive::<2>(&dates.start_month)),
        (&START_DAY, receive::<2>(&dates.start_day)),
        (&START_YEAR, receive::<4>(&dates.start_year)),
        (&END_MONTH, receive::<2>(&dates.end_month)),
        (&END_DAY, receive::<2>(&dates.end_day)),
        (&END_YEAR, receive::<4>(&dates.end_year)),
    ];
    if let Some((rule, _)) = parts.iter().find(|(_, entry)| entry.blank) {
        return Err(DomainError::empty(rule.empty).with_field(rule.field));
    }
    // The edits are pure, so editing every part and then taking the results in
    // screen order answers what the COBOL's first failing edit answers.
    let [
        start_month,
        start_day,
        start_year,
        end_month,
        end_day,
        end_year,
    ] = parts.map(|(rule, entry)| edit_part(rule, entry.stored));
    let start_month = start_month?;
    let start_day = start_day?;
    let start_year = start_year?;
    let end_month = end_month?;
    let end_day = end_day?;
    let end_year = end_year?;
    let start = check_date(
        [start_year, start_month, start_day],
        START_DATE_NOT_A_VALID_DATE,
        FIELD_SDTMM,
    )?;
    let end = check_date(
        [end_year, end_month, end_day],
        END_DATE_NOT_A_VALID_DATE,
        FIELD_EDTMM,
    )?;
    Ok(match (start, end) {
        (Some(start), Some(end)) => Dates::Pinned(ReportRange {
            report_name: REPORT_NAME_CUSTOM,
            start,
            end,
        }),
        (None, _) => Dates::Unpinned(unexpected(FIELD_SDTMM)),
        (Some(_), None) => Dates::Unpinned(unexpected(FIELD_EDTMM)),
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::{
        CustomDates, Entry, FIELD_CONFIRM, FIELD_EDTDD, FIELD_EDTMM, FIELD_EDTYYYY, FIELD_MONTHLY,
        FIELD_SDTDD, FIELD_SDTMM, FIELD_SDTYYYY, ReportRange, ReportRequestInput,
        ReportRequestOutcome, ReportType, Stored, receive, request,
    };
    use crate::error::{DomainError, ErrorKind};
    use crate::messages::{
        END_DATE_DAY_CAN_NOT_BE_EMPTY, END_DATE_MONTH_CAN_NOT_BE_EMPTY, END_DATE_NOT_A_VALID_DATE,
        END_DATE_NOT_A_VALID_DAY, END_DATE_NOT_A_VALID_MONTH, END_DATE_NOT_A_VALID_YEAR,
        END_DATE_YEAR_CAN_NOT_BE_EMPTY, REPORT_NAME_CUSTOM, REPORT_NAME_MONTHLY,
        REPORT_NAME_YEARLY, SELECT_A_REPORT_TYPE_TO_PRINT_REPORT, START_DATE_DAY_CAN_NOT_BE_EMPTY,
        START_DATE_MONTH_CAN_NOT_BE_EMPTY, START_DATE_NOT_A_VALID_DATE, START_DATE_NOT_A_VALID_DAY,
        START_DATE_NOT_A_VALID_MONTH, START_DATE_NOT_A_VALID_YEAR,
        START_DATE_YEAR_CAN_NOT_BE_EMPTY, UNEXPECTED_ERROR, confirm_print_report_message,
        invalid_confirm_value_message, report_submitted_message,
    };
    use crate::numeric::Alnum;
    use crate::rules::Confirm;

    /// The `TRANREPT` default range [app/proc/TRANREPT.prc:L41-42], the
    /// validation audit's custom example (P-CVTRA05Y-016): start year, month,
    /// day, then end year, month, day.
    const TRANREPT_RANGE: [&str; 6] = ["2022", "01", "01", "2022", "07", "06"];

    /// A processing date that is inside the TRANREPT range.
    fn today() -> NaiveDate {
        date(2022, 7, 1)
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("a calendar date")
    }

    /// The input of `report_type` with `confirm`, and the custom parts
    /// `[start year, start month, start day, end year, end month, end day]`.
    fn input(
        report_type: Option<ReportType>,
        parts: [&str; 6],
        confirm: Confirm,
    ) -> ReportRequestInput {
        let [
            start_year,
            start_month,
            start_day,
            end_year,
            end_month,
            end_day,
        ] = parts.map(str::to_owned);
        ReportRequestInput {
            report_type,
            custom: CustomDates {
                start_month,
                start_day,
                start_year,
                end_month,
                end_day,
                end_year,
            },
            confirm,
        }
    }

    /// A confirmed custom report over `parts`.
    fn custom(parts: [&str; 6]) -> Result<ReportRequestOutcome, DomainError> {
        request(
            &input(Some(ReportType::Custom), parts, Confirm::Yes),
            today(),
        )
    }

    /// The outcome a confirmed report of `name` over `start` to `end` gives.
    fn submitted(
        name: &'static str,
        start: &str,
        end: &str,
        message: &str,
    ) -> ReportRequestOutcome {
        ReportRequestOutcome::Submitted {
            range: ReportRange {
                report_name: name,
                start: Alnum::move_from(start),
                end: Alnum::move_from(end),
            },
            message: message.to_owned(),
        }
    }

    /// `parts` with the part at `index` replaced by `text`.
    fn with_part(parts: [&'static str; 6], index: usize, text: &'static str) -> [&'static str; 6] {
        let mut parts = parts;
        if let Some(slot) = parts.get_mut(index) {
            *slot = text;
        }
        parts
    }

    #[test]
    fn monthly_runs_from_the_first_to_the_last_day_of_the_current_month() {
        // P-CORPT00C-003, P-CORPT00C-014, P-CORPT00C-017.
        let cases = [
            (date(2022, 7, 1), "2022-07-01", "2022-07-31"),
            (date(2022, 7, 31), "2022-07-01", "2022-07-31"),
            (date(2022, 4, 15), "2022-04-01", "2022-04-30"),
            (date(2022, 11, 30), "2022-11-01", "2022-11-30"),
            (date(2022, 12, 10), "2022-12-01", "2022-12-31"),
            (date(2024, 2, 15), "2024-02-01", "2024-02-29"),
            (date(2023, 2, 28), "2023-02-01", "2023-02-28"),
            (date(2000, 2, 1), "2000-02-01", "2000-02-29"),
            (date(1900, 2, 1), "1900-02-01", "1900-02-28"),
            (date(1601, 1, 15), "1601-01-01", "1601-01-31"),
            (date(9999, 11, 5), "9999-11-01", "9999-11-30"),
        ];
        for (today, start, end) in cases {
            assert_eq!(
                request(
                    &input(Some(ReportType::Monthly), [""; 6], Confirm::Yes),
                    today
                ),
                Ok(submitted(
                    REPORT_NAME_MONTHLY,
                    start,
                    end,
                    &report_submitted_message(REPORT_NAME_MONTHLY)
                )),
                "{today}"
            );
        }
    }

    #[test]
    fn monthly_outside_the_integer_of_date_domain_is_unexpected() {
        // INTEGER-OF-DATE and DATE-OF-INTEGER have no result before 1601-01-01
        // or after 9999-12-31; no row pins the COBOL outcome there.
        for today in [date(9999, 12, 1), date(1600, 12, 31), date(1500, 6, 15)] {
            let error = request(
                &input(Some(ReportType::Monthly), [""; 6], Confirm::Yes),
                today,
            )
            .expect_err("no pinned range");
            assert_eq!(
                error,
                DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_MONTHLY),
                "{today}"
            );
        }
    }

    #[test]
    fn yearly_runs_over_the_current_calendar_year() {
        // P-CORPT00C-004, P-CORPT00C-014, P-CORPT00C-017.
        let cases = [
            (date(2022, 7, 6), "2022-01-01", "2022-12-31"),
            (date(2024, 12, 31), "2024-01-01", "2024-12-31"),
            (date(999, 3, 1), "0999-01-01", "0999-12-31"),
        ];
        for (today, start, end) in cases {
            assert_eq!(
                request(
                    &input(Some(ReportType::Yearly), [""; 6], Confirm::Yes),
                    today
                ),
                Ok(submitted(
                    REPORT_NAME_YEARLY,
                    start,
                    end,
                    &report_submitted_message(REPORT_NAME_YEARLY)
                )),
                "{today}"
            );
        }
    }

    #[test]
    fn a_missing_report_type_is_rejected_before_anything_else() {
        // V-CORPT00C-001, P-CORPT00C-009.
        for confirm in [
            Confirm::Yes,
            Confirm::Blank,
            Confirm::No,
            Confirm::Other("X".to_owned()),
        ] {
            assert_eq!(
                request(&input(None, ["", "13", "", "", "", ""], confirm), today()),
                Err(DomainError::empty(SELECT_A_REPORT_TYPE_TO_PRINT_REPORT)
                    .with_field(FIELD_MONTHLY))
            );
        }
    }

    #[test]
    fn custom_runs_over_the_entered_dates() {
        // P-CORPT00C-005 to P-CORPT00C-008, P-CORPT00C-014.
        let message = report_submitted_message(REPORT_NAME_CUSTOM);
        let cases = [
            (TRANREPT_RANGE, "2022-01-01", "2022-07-06"),
            // One-digit parts are padded in their fields and converted to two.
            (
                ["2022", "1", "1", "2022", "7", "6"],
                "2022-01-01",
                "2022-07-06",
            ),
            // The start date is never compared with the end date.
            (
                ["2022", "07", "06", "2022", "01", "01"],
                "2022-07-06",
                "2022-01-01",
            ),
            // The last day of each month length, and a leap day.
            (
                ["2022", "04", "30", "2024", "02", "29"],
                "2022-04-30",
                "2024-02-29",
            ),
            (
                ["2022", "12", "31", "2023", "02", "28"],
                "2022-12-31",
                "2023-02-28",
            ),
        ];
        for (parts, start, end) in cases {
            assert_eq!(
                custom(parts),
                Ok(submitted(REPORT_NAME_CUSTOM, start, end, &message)),
                "{parts:?}"
            );
        }
    }

    #[test]
    fn dates_csutldtc_reports_out_of_range_pass_with_2513() {
        // P-CORPT00C-008, P-CSUTLDTC-004: a calendar date before 1582-10-15
        // has message 2513 and is accepted; a short year is converted to four
        // digits and lands there.
        let cases = [
            (
                ["1500", "01", "01", "2022", "07", "06"],
                "1500-01-01",
                "2022-07-06",
            ),
            (
                ["22", "01", "01", "2022", "07", "06"],
                "0022-01-01",
                "2022-07-06",
            ),
            (
                ["2022", "01", "01", "1", "12", "31"],
                "2022-01-01",
                "0001-12-31",
            ),
        ];
        for (parts, start, end) in cases {
            assert_eq!(
                custom(parts),
                Ok(submitted(
                    REPORT_NAME_CUSTOM,
                    start,
                    end,
                    &report_submitted_message(REPORT_NAME_CUSTOM)
                )),
                "{parts:?}"
            );
        }
    }

    #[test]
    fn empty_parts_are_rejected_in_screen_order() {
        // V-CORPT00C-002 to V-CORPT00C-007, P-CORPT00C-005: indexes into
        // TRANREPT_RANGE, in the order the EVALUATE tests the fields.
        let order = [
            (1, START_DATE_MONTH_CAN_NOT_BE_EMPTY, FIELD_SDTMM),
            (2, START_DATE_DAY_CAN_NOT_BE_EMPTY, FIELD_SDTDD),
            (0, START_DATE_YEAR_CAN_NOT_BE_EMPTY, FIELD_SDTYYYY),
            (4, END_DATE_MONTH_CAN_NOT_BE_EMPTY, FIELD_EDTMM),
            (5, END_DATE_DAY_CAN_NOT_BE_EMPTY, FIELD_EDTDD),
            (3, END_DATE_YEAR_CAN_NOT_BE_EMPTY, FIELD_EDTYYYY),
        ];
        for (position, &(index, message, field)) in order.iter().enumerate() {
            let expected = Err(DomainError::empty(message).with_field(field));
            // SPACES or LOW-VALUES filling the field; the years are X(4).
            let blanks = if index % 3 == 0 {
                ["", " ", "    ", "\0\0\0\0"]
            } else {
                ["", " ", "  ", "\0\0"]
            };
            for blank in blanks {
                assert_eq!(
                    custom(with_part(TRANREPT_RANGE, index, blank)),
                    expected,
                    "{blank:?}"
                );
            }
            // Every later field blank too: this one is still named first.
            let mut parts = TRANREPT_RANGE;
            for &(later, _, _) in order.iter().skip(position) {
                parts = with_part(parts, later, "");
            }
            assert_eq!(custom(parts), expected, "{field} with later blanks");
        }
        // The empty checks run before any value edit: a later blank part
        // answers before an earlier invalid one.
        assert_eq!(
            custom(["2022", "13", "01", "2022", "07", ""]),
            Err(DomainError::empty(END_DATE_DAY_CAN_NOT_BE_EMPTY).with_field(FIELD_EDTDD))
        );
    }

    #[test]
    fn value_edits_reject_in_screen_order() {
        // V-CORPT00C-008 to V-CORPT00C-013, P-CORPT00C-007; malformed and
        // over-long parts take the E-28 decision (P-CORPT00C-016).
        let cases: [(usize, &str, &str, &str); 14] = [
            (1, "13", START_DATE_NOT_A_VALID_MONTH, FIELD_SDTMM),
            (1, "99", START_DATE_NOT_A_VALID_MONTH, FIELD_SDTMM),
            (1, "AB", START_DATE_NOT_A_VALID_MONTH, FIELD_SDTMM),
            (1, "123", START_DATE_NOT_A_VALID_MONTH, FIELD_SDTMM),
            (2, "32", START_DATE_NOT_A_VALID_DAY, FIELD_SDTDD),
            (2, "1A", START_DATE_NOT_A_VALID_DAY, FIELD_SDTDD),
            (0, "20A2", START_DATE_NOT_A_VALID_YEAR, FIELD_SDTYYYY),
            (0, "20221", START_DATE_NOT_A_VALID_YEAR, FIELD_SDTYYYY),
            (4, "13", END_DATE_NOT_A_VALID_MONTH, FIELD_EDTMM),
            (4, "1,", END_DATE_NOT_A_VALID_MONTH, FIELD_EDTMM),
            (5, "32", END_DATE_NOT_A_VALID_DAY, FIELD_EDTDD),
            (5, "\0 ", END_DATE_NOT_A_VALID_DAY, FIELD_EDTDD),
            (3, "20A2", END_DATE_NOT_A_VALID_YEAR, FIELD_EDTYYYY),
            (3, "2 22", END_DATE_NOT_A_VALID_YEAR, FIELD_EDTYYYY),
        ];
        for (index, text, message, field) in cases {
            assert_eq!(
                custom(with_part(TRANREPT_RANGE, index, text)),
                Err(DomainError::invalid(message).with_field(field)),
                "{text:?}"
            );
        }
        // The first failing edit answers: start day before end month.
        assert_eq!(
            custom(["2022", "01", "32", "2022", "13", "06"]),
            Err(DomainError::invalid(START_DATE_NOT_A_VALID_DAY).with_field(FIELD_SDTDD))
        );
        // A value edit answers before either date check.
        assert_eq!(
            custom(["2022", "02", "30", "20A2", "07", "06"]),
            Err(DomainError::invalid(END_DATE_NOT_A_VALID_YEAR).with_field(FIELD_EDTYYYY))
        );
    }

    #[test]
    fn dates_are_checked_through_csutldtc_start_first() {
        // V-CORPT00C-014, V-CORPT00C-015, P-CORPT00C-008: "12" and "31" pass
        // their value edits, and so do "00" months and days, which CSUTLDTC
        // then rejects.
        let start = Err(DomainError::invalid(START_DATE_NOT_A_VALID_DATE).with_field(FIELD_SDTMM));
        let end = Err(DomainError::invalid(END_DATE_NOT_A_VALID_DATE).with_field(FIELD_EDTMM));
        let cases = [
            (["2022", "02", "30", "2022", "07", "06"], &start),
            (["2023", "02", "29", "2022", "07", "06"], &start),
            (["2022", "00", "15", "2022", "07", "06"], &start),
            (["2022", "01", "00", "2022", "07", "06"], &start),
            (["2022", "02", "30", "2022", "04", "31"], &start),
            (["2022", "01", "01", "2022", "04", "31"], &end),
            (["2022", "01", "01", "2022", "00", "06"], &end),
            (["2022", "01", "01", "2022", "12", "00"], &end),
        ];
        for (parts, expected) in cases {
            assert_eq!(&custom(parts), expected, "{parts:?}");
        }
    }

    #[test]
    fn dates_with_unpinned_csutldtc_feedback_are_unexpected() {
        // E-31: year 0000, and out-of-range dates that are not calendar dates.
        let start = Err(DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_SDTMM));
        let end = Err(DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_EDTMM));
        let cases = [
            (["0", "01", "01", "2022", "07", "06"], &start),
            (["0000", "01", "01", "2022", "07", "06"], &start),
            (["1500", "02", "30", "2022", "07", "06"], &start),
            (["2022", "01", "01", "0", "07", "06"], &end),
        ];
        for (parts, expected) in cases {
            assert_eq!(&custom(parts), expected, "{parts:?}");
        }
        // A start date CSUTLDTC rejects answers before an unproven end date.
        assert_eq!(
            custom(["2022", "02", "30", "0", "07", "06"]),
            Err(DomainError::invalid(START_DATE_NOT_A_VALID_DATE).with_field(FIELD_SDTMM))
        );
        assert_eq!(
            start.map_err(|error| error.kind),
            Err(ErrorKind::Unexpected)
        );
    }

    #[test]
    fn midpoints_stop_only_the_outcomes_that_depend_on_them() {
        // E-40: a fraction of exactly one half is stored rounded either way.
        // ".5" as the start month is 00 (rejected by CSUTLDTC) or 01.
        assert_eq!(
            custom(["2022", ".5", "01", "2022", "07", "06"]),
            Err(DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_SDTMM))
        );
        // ".5" as the start day of February is 00 (rejected) or 01.
        assert_eq!(
            custom(["2022", "02", ".5", "2022", "07", "06"]),
            Err(DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_SDTMM))
        );
        // A later edit that fails whatever the midpoint keeps still answers.
        assert_eq!(
            custom(["2022", ".5", "01", "2022", "07", "32"]),
            Err(DomainError::invalid(END_DATE_NOT_A_VALID_DAY).with_field(FIELD_EDTDD))
        );
        // "99.5" is the year 0099 or 0100: CSUTLDTC accepts both (2513), so
        // the edits pass and only the dates stay unpinned.
        let unpinned = ["99.5", "01", "01", "2022", "07", "06"];
        assert_eq!(
            custom(unpinned),
            Err(DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_SDTMM))
        );
        assert_eq!(
            custom(["2022", "01", "01", "99.5", "07", "06"]),
            Err(DomainError::unexpected(UNEXPECTED_ERROR).with_field(FIELD_EDTMM))
        );
        assert_eq!(
            custom(["99.5", "01", "01", "2022", "04", "31"]),
            Err(DomainError::invalid(END_DATE_NOT_A_VALID_DATE).with_field(FIELD_EDTMM))
        );
        assert_eq!(
            request(
                &input(Some(ReportType::Custom), unpinned, Confirm::Blank),
                today()
            ),
            Err(
                DomainError::invalid(confirm_print_report_message(REPORT_NAME_CUSTOM))
                    .with_field(FIELD_CONFIRM)
            )
        );
        assert_eq!(
            request(
                &input(Some(ReportType::Custom), unpinned, Confirm::No),
                today()
            ),
            Err(DomainError::invalid("").with_field(FIELD_MONTHLY))
        );
    }

    #[test]
    fn the_confirmation_decides_after_every_edit() {
        // V-CORPT00C-016 to V-CORPT00C-018, P-CORPT00C-010, P-CORPT00C-011,
        // P-CORPT00C-014; exact texts, each report name DELIMITED BY SPACE.
        let reports = [
            (ReportType::Monthly, REPORT_NAME_MONTHLY),
            (ReportType::Yearly, REPORT_NAME_YEARLY),
            (ReportType::Custom, REPORT_NAME_CUSTOM),
        ];
        for (report_type, name) in reports {
            let ask = |confirm: Confirm| {
                request(&input(Some(report_type), TRANREPT_RANGE, confirm), today())
            };
            assert_eq!(
                ask(Confirm::Blank),
                Err(DomainError::invalid(confirm_print_report_message(name))
                    .with_field(FIELD_CONFIRM))
            );
            let cleared = ask(Confirm::No).expect_err("N clears the screen");
            assert_eq!(cleared, DomainError::invalid("").with_field(FIELD_MONTHLY));
            assert_eq!(cleared.kind, ErrorKind::Invalid);
            assert_eq!(
                ask(Confirm::Other("X".to_owned())),
                Err(DomainError::invalid(invalid_confirm_value_message("X"))
                    .with_field(FIELD_CONFIRM))
            );
            // CONFIRMI is X(1): the quoted value is its first character.
            assert_eq!(
                ask(Confirm::Other("XY".to_owned())),
                Err(DomainError::invalid(invalid_confirm_value_message("X"))
                    .with_field(FIELD_CONFIRM))
            );
            let Ok(ReportRequestOutcome::Submitted { range, message }) = ask(Confirm::Yes) else {
                panic!("{name}: Y runs the report");
            };
            assert_eq!(range.report_name, name);
            assert_eq!(message, report_submitted_message(name));
        }
        // Y and N are accepted in either case.
        assert_eq!(Confirm::parse(Some("y")), Confirm::Yes);
        assert_eq!(Confirm::parse(Some("n")), Confirm::No);
    }

    #[test]
    fn edits_answer_before_the_confirmation() {
        // P-CORPT00C-002: the job submission, and its confirmation test, is
        // reached only when every edit passed.
        for confirm in [Confirm::Blank, Confirm::No, Confirm::Other("X".to_owned())] {
            assert_eq!(
                request(
                    &input(
                        Some(ReportType::Custom),
                        with_part(TRANREPT_RANGE, 1, "13"),
                        confirm
                    ),
                    today()
                ),
                Err(DomainError::invalid(START_DATE_NOT_A_VALID_MONTH).with_field(FIELD_SDTMM))
            );
        }
    }

    #[test]
    fn parts_convert_as_numval_c_reads_them() {
        // P-CORPT00C-006, P-CORPT00C-015, P-CORPT00C-016: the value the
        // unsigned receiver keeps; `None` for text NUMVAL-C does not read.
        let exact = |value| {
            Some(Stored {
                value,
                alternative: None,
            })
        };
        let midpoint = |value, other| {
            Some(Stored {
                value,
                alternative: Some(other),
            })
        };
        let two: [(&str, Option<Stored>); 22] = [
            ("7", exact(7)),
            (" 7", exact(7)),
            ("07", exact(7)),
            ("12", exact(12)),
            ("99", exact(99)),
            ("+7", exact(7)),
            ("-7", exact(7)),
            ("7+", exact(7)),
            ("7-", exact(7)),
            ("$7", exact(7)),
            ("7.", exact(7)),
            (".4", exact(0)),
            (".6", exact(1)),
            (".5", midpoint(0, 1)),
            ("AB", None),
            ("1 ", exact(1)),
            ("+", None),
            (".", None),
            ("$", None),
            ("1$", None),
            ("1,", None),
            ("\0 ", None),
        ];
        for (text, expected) in two {
            assert_eq!(receive::<2>(text).stored, expected, "{text:?}");
        }
        let four: [(&str, Option<Stored>); 18] = [
            ("2022", exact(2022)),
            ("22", exact(22)),
            ("12.4", exact(12)),
            ("12.6", exact(13)),
            ("12.5", midpoint(12, 13)),
            ("2.50", midpoint(2, 3)),
            ("2.51", exact(3)),
            ("99.9", exact(100)),
            ("12CR", exact(12)),
            ("1 DB", exact(1)),
            ("+ 12", exact(12)),
            ("$ 12", exact(12)),
            ("-$12", exact(12)),
            ("12cr", None),
            ("+1-", None),
            ("--1", None),
            ("1.2.", None),
            ("1 2 ", None),
        ];
        for (text, expected) in four {
            assert_eq!(receive::<4>(text).stored, expected, "{text:?}");
        }
    }

    #[test]
    fn parts_are_received_into_their_map_fields() {
        // P-CORPT00C-005, P-CORPT00C-019: blank is SPACES or LOW-VALUES over
        // the whole field; text longer than the field is neither blank nor
        // read, so it is rejected rather than cut.
        let cases: [(&str, bool, bool); 7] = [
            ("", true, false),
            (" ", true, false),
            ("\0\0", true, false),
            ("\0", false, false),
            ("\0 ", false, false),
            ("123", false, false),
            ("   ", false, false),
        ];
        for (text, blank, read) in cases {
            let Entry { blank: got, stored } = receive::<2>(text);
            assert_eq!((got, stored.is_some()), (blank, read), "{text:?}");
        }
    }
}
