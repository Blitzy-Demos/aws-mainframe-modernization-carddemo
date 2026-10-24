//! CORPT00C, transaction-report submission: the rules of endpoint 14,
//! `POST /api/v1/reports/transactions` (transaction `CR00`, map `CORPT0A`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/CORPT00C.md`: the report
//! selection (CORPT00C-007), the Monthly and Yearly windows (CORPT00C-008 to CORPT00C-010), the
//! custom date edits (CORPT00C-011 to CORPT00C-028), the missing selection (CORPT00C-029), the
//! confirmation (CORPT00C-030 to CORPT00C-033), the job deck (CORPT00C-034, CORPT00C-035) and
//! the messages of a failed write and of a submission (CORPT00C-037, CORPT00C-038). The `client`
//! rows, F3 and every other key among them (CORPT00C-004, CORPT00C-005), send no request and
//! belong to the screen.
//!
//! # Flow
//!
//! `api::services::corpt00c` calls [`edit`], then [`confirm`]. It writes the [`job_deck`] of a
//! confirmed request to one generation file `TRNRPT00.G<nnnn>` of `CARDDEMO_INTRDR_DIR`, the
//! local stand-in for TDQ `JOBS` (AAP 0.3.4.4, CORPT00C-036), and answers a failed write with
//! [`tdq_write_failure`] and a written deck with [`submitted_message`]. This module holds no
//! path, directory or file name and does no I/O: the deck leaves it as bytes, and the endpoint
//! reads no table and runs no report (CORPT00C-039).
//!
//! Every send of the COBOL ends its task (`GO TO RETURN-TO-CICS`), so the answer to a request is
//! the first screen the program sends: the edits stop at the first that fails, in the program's
//! order, and a failed request yields no deck (CORPT00C-006). A failure is a [`ReportFailure`]:
//! the literal moved to `WS-MESSAGE`, the request field the cursor goes to and the status class.
//!
//! # Input and typed dates
//!
//! The ten inputs enter as the raw text of their map fields ([`ReportInput`]), the raw-edit-input
//! exception of AAP 0.3.4.1, so that the program's edit messages stay reproducible. Each is read
//! as a `MOVE` into its `PIC X(n)` field: its first `n` characters, padded with spaces. A field is
//! blank when it is all spaces or all low-values (`\0`). A date leaves this module only as a
//! typed [`NaiveDate`] (Typed Dates and Timestamps); the deck holds it as its 10-character
//! `YYYY-MM-DD` text.
//!
//! # Halts
//!
//! - `HALTED(H5)`: which custom dates CSUTLDTC accepts is the verdict of `CEEDAYS`, outside the
//!   repository (CORPT00C-027). [`edit`] passes the stand-in [`validate`], which is no parity
//!   result; [`edit_with`] takes the verdict as a parameter, so that a test substitutes it and
//!   proves CORPT00C-025, CORPT00C-026 and CORPT00C-028 for every verdict of a class.
//! - Halt candidate X-CORPT00C-H1: `NUMVAL-C` of a date part that is not digits has no single
//!   COBOL result. The request DTO admits digits only (X-CORPT00C-5); here such a part fails its
//!   part edit with that part's `Not a valid` message, the replacement's choice and no parity
//!   result.
//! - Halt candidate X-CORPT00C-H3: for a clock outside January 1601 to November 9999 the COBOL
//!   Monthly end date has no single result; `chrono` gives the last day of the month there, which
//!   is no parity result either.
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a value
//! or a [`ReportFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input). Digits are read with `char::to_digit` and checked arithmetic.

use core::fmt;
use std::iter;

use chrono::{Datelike, NaiveDate};
use copybook::PicX;

use super::csutldtc::{DateMask, LeResult, caller_accepts, validate};
use crate::clock::Clock;

/// Length of one deck record: `JOB-LINES PIC X(80)`, and `RECORDSIZE(80)` of TDQ `JOBS`
/// (CORPT00C-034, CORPT00C-036).
pub const DECK_RECORD_LEN: usize = 80;

/// Records in the deck: the 17 entries of `JOB-DATA-1`, `/*EOF` the last (CORPT00C-035).
pub const DECK_RECORD_COUNT: usize = 17;

/// Width of `MONTHLYI`, `YEARLYI` and `CUSTOMI PIC X(1)` (CORPT00C-007).
const SELECTOR_WIDTH: usize = 1;

/// Width of `CONFIRMI PIC X(1)` (CORPT00C-030).
const CONFIRM_WIDTH: usize = 1;

/// Width of the month and day parts, `PIC X(2)`, and of `WS-NUM-99 PIC 99` (CORPT00C-017).
const TWO_DIGIT_PART: usize = 2;

/// Width of the year parts, `PIC X(4)`, and of `WS-NUM-9999 PIC 9999` (CORPT00C-017).
const FOUR_DIGIT_PART: usize = 4;

/// The highest month the part edit passes: `> '12'` rejects (CORPT00C-018, CORPT00C-021).
const MAX_MONTH: u32 = 12;

/// The highest day the part edit passes: `> '31'` rejects (CORPT00C-019, CORPT00C-022).
const MAX_DAY: u32 = 31;

/// The day every month has, from which the last day of the current month is searched upward.
const SHORTEST_MONTH: u32 = 28;

/// The year modulus that keeps the four digits of a `YYYY` date part.
const FOUR_DIGIT_YEARS: i32 = 10_000;

/// No selector marked (CORPT00C-029).
const SELECT_REPORT_TYPE: &str = "Select a report type to print report...";

/// Head and tail of the confirmation prompt around the report name (CORPT00C-030).
const CONFIRM_PROMPT_HEAD: &str = "Please confirm to print the ";
const CONFIRM_PROMPT_TAIL: &str = " report...";

/// Tail of the message for a confirmation value the program does not know (CORPT00C-033).
const CONFIRM_INVALID_TAIL: &str = "\" is not a valid value to confirm...";

/// A deck record could not be written (CORPT00C-037).
const WRITE_FAILURE: &str = "Unable to Write TDQ (JOBS)...";

/// Tail of the submission message after the report name (CORPT00C-038).
const SUBMITTED_TAIL: &str = " report submitted for printing ...";

/// The custom start date was rejected (CORPT00C-025).
const START_DATE_INVALID: &str = "Start Date - Not a valid date...";

/// The custom end date was rejected (CORPT00C-026).
const END_DATE_INVALID: &str = "End Date - Not a valid date...";

/// The report a request asks for, as `PROCESS-ENTER-KEY` names it in `WS-REPORT-NAME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportKind {
    /// The current month (CORPT00C-008, CORPT00C-009).
    Monthly,
    /// The current year (CORPT00C-010).
    Yearly,
    /// The date range of the six date parts (CORPT00C-011 to CORPT00C-028).
    Custom,
}

impl ReportKind {
    /// The value moved to `WS-REPORT-NAME PIC X(10)`, without its padding: `Monthly`, `Yearly`
    /// or `Custom` (CORPT00C-008, CORPT00C-010, CORPT00C-028). None holds a space, so
    /// `DELIMITED BY SPACE` keeps each whole in the prompt and the submission message.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Monthly => "Monthly",
            Self::Yearly => "Yearly",
            Self::Custom => "Custom",
        }
    }
}

/// The request field a failure names: where the COBOL puts the cursor (`MOVE -1 TO <field>L`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReportField {
    /// `MONTHLY`: no selector (CORPT00C-029) and a failed write (CORPT00C-037).
    Monthly,
    /// `SDTMM`: its own edits, and a rejected start date (CORPT00C-025).
    StartMonth,
    /// `SDTDD`.
    StartDay,
    /// `SDTYYYY`.
    StartYear,
    /// `EDTMM`: its own edits, and a rejected end date (CORPT00C-026).
    EndMonth,
    /// `EDTDD`.
    EndDay,
    /// `EDTYYYY`.
    EndYear,
    /// `CONFIRM`: the prompt and an unknown value (CORPT00C-030, CORPT00C-033).
    Confirm,
}

/// The status class of a [`ReportFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An edit of the request failed: 400.
    Input,
    /// The deck could not be written: 500 (CORPT00C-037).
    Write,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400 for [`FailureClass::Input`], 500 for
    /// [`FailureClass::Write`].
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::Write => 500,
        }
    }
}

/// A screen CORPT00C sends instead of submitting: the message, the field in error and the
/// status class.
///
/// The message is the literal moved to `WS-MESSAGE PIC X(80)` without its trailing spaces, as
/// the response carries it; every one fits the 78-byte message row (CORPT00C-006). It holds no
/// request value but the one confirmation character of CORPT00C-033.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportFailure {
    message: String,
    field: ReportField,
    class: FailureClass,
}

impl ReportFailure {
    /// A failed edit of the request: status class [`FailureClass::Input`].
    fn input(message: impl Into<String>, field: ReportField) -> Self {
        Self {
            message: message.into(),
            field,
            class: FailureClass::Input,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The field the cursor goes to.
    #[must_use]
    pub const fn field(&self) -> ReportField {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for ReportFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ReportFailure {}

/// The ten inputs of `CORPT0A`, each the raw text of its map field (`ReportRequest` of
/// endpoint 14; CORPT00C-042). An absent field is the empty text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReportInput<'a> {
    /// `MONTHLY`, `PIC X(1)`.
    pub monthly: &'a str,
    /// `YEARLY`, `PIC X(1)`.
    pub yearly: &'a str,
    /// `CUSTOM`, `PIC X(1)`.
    pub custom: &'a str,
    /// `SDTMM`, `PIC X(2)`.
    pub start_month: &'a str,
    /// `SDTDD`, `PIC X(2)`.
    pub start_day: &'a str,
    /// `SDTYYYY`, `PIC X(4)`.
    pub start_year: &'a str,
    /// `EDTMM`, `PIC X(2)`.
    pub end_month: &'a str,
    /// `EDTDD`, `PIC X(2)`.
    pub end_day: &'a str,
    /// `EDTYYYY`, `PIC X(4)`.
    pub end_year: &'a str,
    /// `CONFIRM`, `PIC X(1)`, read by [`confirm`].
    pub confirm: &'a str,
}

/// A report whose dates the edits have set: what `SUBMIT-JOB-TO-INTRDR` receives.
///
/// Only [`edit`] and [`edit_with`] build one, so its dates are always those the program would
/// put in the deck (CORPT00C-008 to CORPT00C-010, CORPT00C-028).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ReportRequest {
    kind: ReportKind,
    start: NaiveDate,
    end: NaiveDate,
}

impl ReportRequest {
    /// The report, which names the prompt and the submission message.
    #[must_use]
    pub const fn kind(&self) -> ReportKind {
        self.kind
    }

    /// The start date, `PARM-START-DATE-1` and `PARM-START-DATE-2` of the deck.
    #[must_use]
    pub const fn start(&self) -> NaiveDate {
        self.start
    }

    /// The end date, `PARM-END-DATE-1` and `PARM-END-DATE-2` of the deck. No edit compares it
    /// with the start date, so it may precede it (CORPT00C-024).
    #[must_use]
    pub const fn end(&self) -> NaiveDate {
        self.end
    }
}

/// What a request's confirmation decides once its dates are set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConfirmOutcome {
    /// `Y` or `y`: write the [`job_deck`] of the request (CORPT00C-031).
    Submit(ReportRequest),
    /// `N` or `n`: 200 with an empty message and no deck; the screen empties every input
    /// (CORPT00C-032).
    Cleared,
}

/// The dates of a report taken from the clock: Monthly or Yearly (CORPT00C-008 to
/// CORPT00C-010), as `(start, end)`; `None` for [`ReportKind::Custom`], whose dates come from
/// the request.
///
/// `FUNCTION CURRENT-DATE` is the clock's local date. Monthly runs from the first to the last day
/// of the current month: the COBOL takes the day before the first of the next month, January of
/// the next year after December, which is the same date. Yearly runs from 1 January to
/// 31 December of the current year.
#[must_use]
pub fn date_window(kind: ReportKind, clock: &dyn Clock) -> Option<(NaiveDate, NaiveDate)> {
    match kind {
        ReportKind::Monthly => Some(monthly_window(clock.now().date())),
        ReportKind::Yearly => Some(yearly_window(clock.now().date())),
        ReportKind::Custom => None,
    }
}

/// The current month of `today`, first and last day (CORPT00C-008, CORPT00C-009).
fn monthly_window(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    // Day 1 and day 28 exist in every month, so neither fallback is ever taken; each keeps the
    // function total without a panic path.
    let first = today.with_day(1).unwrap_or(today);
    let last = (SHORTEST_MONTH..=MAX_DAY)
        .rev()
        .find_map(|day| today.with_day(day))
        .unwrap_or(today);
    (first, last)
}

/// The current year of `today`, 1 January and 31 December (CORPT00C-010).
fn yearly_window(today: NaiveDate) -> (NaiveDate, NaiveDate) {
    // Every year a date exists in has its first and last day, so neither fallback is ever taken.
    let first = today.with_ordinal(1).unwrap_or(today);
    let last = NaiveDate::from_ymd_opt(today.year(), MAX_MONTH, MAX_DAY).unwrap_or(today);
    (first, last)
}

/// `PROCESS-ENTER-KEY` up to `SUBMIT-JOB-TO-INTRDR`: the report the request selects and its
/// dates (CORPT00C-007 to CORPT00C-029), with CSUTLDTC's stand-in verdict on custom dates.
///
/// `HALTED(H5)`: [`validate`] decides which custom dates pass, and no test may present that as
/// the COBOL's (CORPT00C-027). Everything else is [`edit_with`]'s.
///
/// # Errors
///
/// The first edit that fails, as [`edit_with`] lists them.
pub fn edit(input: &ReportInput<'_>, clock: &dyn Clock) -> Result<ReportRequest, ReportFailure> {
    edit_with(input, clock, validate)
}

/// [`edit`] with the date-validity verdict supplied by `verdict`, which receives the 10-character
/// date and the picture `YYYY-MM-DD` and returns CSUTLDTC's result (CSUTLDTC-030, CSUTLDTC-031).
///
/// The first selector that is not blank, in the order `monthly`, `yearly`, `custom`, picks the
/// report; any character selects (CORPT00C-007). Monthly and Yearly take their dates from
/// `clock` ([`date_window`]), and the date parts are not read. Custom runs the part edits in
/// order: the six emptiness tests (CORPT00C-011 to CORPT00C-016), `NUMVAL-C` of every part
/// (CORPT00C-017), the six range tests (CORPT00C-018 to CORPT00C-023), then the start date and,
/// only when it is accepted, the end date (CORPT00C-024 to CORPT00C-026). A date is accepted
/// when [`caller_accepts`] its result (CSUTLDTC-031); the dates are not compared with each other.
///
/// # Errors
///
/// Every failure has status class [`FailureClass::Input`]:
///
/// - no selector marked: `Select a report type to print report...` on `monthly` (CORPT00C-029);
/// - a blank part: `Start Date - Month can NOT be empty...` and its five siblings, on that part
///   (CORPT00C-011 to CORPT00C-016);
/// - a month above 12 or a day above 31: `Start Date - Not a valid Month...`,
///   `Start Date - Not a valid Day...`, `End Date - Not a valid Month...` or
///   `End Date - Not a valid Day...`, on that part (CORPT00C-018, CORPT00C-019, CORPT00C-021,
///   CORPT00C-022). A part that is not digits fails the same way, and a year part that is not
///   digits with `Start Date - Not a valid Year...` or `End Date - Not a valid Year...`, which
///   is no parity result (halt candidate X-CORPT00C-H1);
/// - a rejected date: `Start Date - Not a valid date...` on [`ReportField::StartMonth`] or
///   `End Date - Not a valid date...` on [`ReportField::EndMonth`] (CORPT00C-025,
///   CORPT00C-026).
pub fn edit_with(
    input: &ReportInput<'_>,
    clock: &dyn Clock,
    verdict: impl Fn(&str, DateMask) -> LeResult,
) -> Result<ReportRequest, ReportFailure> {
    match selected(input) {
        Some(ReportKind::Monthly) => Ok(window_request(
            ReportKind::Monthly,
            monthly_window(clock.now().date()),
        )),
        Some(ReportKind::Yearly) => Ok(window_request(
            ReportKind::Yearly,
            yearly_window(clock.now().date()),
        )),
        Some(ReportKind::Custom) => edit_custom(input, &verdict),
        None => Err(ReportFailure::input(
            SELECT_REPORT_TYPE,
            ReportField::Monthly,
        )),
    }
}

/// The request of a clock-dated report.
const fn window_request(kind: ReportKind, (start, end): (NaiveDate, NaiveDate)) -> ReportRequest {
    ReportRequest { kind, start, end }
}

/// The `EVALUATE TRUE` over the three selectors: the first not blank, or `None` (CORPT00C-007).
fn selected(input: &ReportInput<'_>) -> Option<ReportKind> {
    [
        (input.monthly, ReportKind::Monthly),
        (input.yearly, ReportKind::Yearly),
        (input.custom, ReportKind::Custom),
    ]
    .into_iter()
    .find_map(|(text, kind)| (!is_blank(&fit(text, SELECTOR_WIDTH))).then_some(kind))
}

/// One date part of a custom request: its map field, width and messages.
#[derive(Clone, Copy)]
struct PartSpec {
    /// The field the part's failures name.
    field: ReportField,
    /// `PIC X(2)` or `PIC X(4)`, the width of the unsigned item `NUMVAL-C` stores into too.
    width: usize,
    /// The emptiness message (CORPT00C-011 to CORPT00C-016).
    empty: &'static str,
    /// The part-edit message (CORPT00C-018 to CORPT00C-023).
    invalid: &'static str,
    /// The highest value the range test passes; a year has none, as its only test is
    /// `IS NOT NUMERIC` (CORPT00C-020, CORPT00C-023).
    max: Option<u32>,
}

/// `SDTMM` (CORPT00C-011, CORPT00C-018).
const START_MONTH: PartSpec = PartSpec {
    field: ReportField::StartMonth,
    width: TWO_DIGIT_PART,
    empty: "Start Date - Month can NOT be empty...",
    invalid: "Start Date - Not a valid Month...",
    max: Some(MAX_MONTH),
};

/// `SDTDD` (CORPT00C-012, CORPT00C-019).
const START_DAY: PartSpec = PartSpec {
    field: ReportField::StartDay,
    width: TWO_DIGIT_PART,
    empty: "Start Date - Day can NOT be empty...",
    invalid: "Start Date - Not a valid Day...",
    max: Some(MAX_DAY),
};

/// `SDTYYYY` (CORPT00C-013, CORPT00C-020).
const START_YEAR: PartSpec = PartSpec {
    field: ReportField::StartYear,
    width: FOUR_DIGIT_PART,
    empty: "Start Date - Year can NOT be empty...",
    invalid: "Start Date - Not a valid Year...",
    max: None,
};

/// `EDTMM` (CORPT00C-014, CORPT00C-021).
const END_MONTH: PartSpec = PartSpec {
    field: ReportField::EndMonth,
    width: TWO_DIGIT_PART,
    empty: "End Date - Month can NOT be empty...",
    invalid: "End Date - Not a valid Month...",
    max: Some(MAX_MONTH),
};

/// `EDTDD` (CORPT00C-015, CORPT00C-022).
const END_DAY: PartSpec = PartSpec {
    field: ReportField::EndDay,
    width: TWO_DIGIT_PART,
    empty: "End Date - Day can NOT be empty...",
    invalid: "End Date - Not a valid Day...",
    max: Some(MAX_DAY),
};

/// `EDTYYYY` (CORPT00C-016, CORPT00C-023).
const END_YEAR: PartSpec = PartSpec {
    field: ReportField::EndYear,
    width: FOUR_DIGIT_PART,
    empty: "End Date - Year can NOT be empty...",
    invalid: "End Date - Not a valid Year...",
    max: None,
};

/// A date part as its map field holds it.
struct DatePart {
    spec: PartSpec,
    field: String,
}

impl DatePart {
    /// `text` moved into the part's map field.
    fn new(spec: PartSpec, text: &str) -> Self {
        Self {
            spec,
            field: fit(text, spec.width),
        }
    }

    /// The part's range test after `NUMVAL-C` (CORPT00C-017 to CORPT00C-023): the part's value,
    /// whose zero-padded digits all later edits and the deck use.
    fn edit(&self) -> Result<u32, ReportFailure> {
        match (numval_c(&self.field), self.spec.max) {
            (Some(value), Some(max)) if value <= max => Ok(value),
            (Some(value), None) => Ok(value),
            _ => Err(ReportFailure::input(self.spec.invalid, self.spec.field)),
        }
    }
}

/// The custom path of `PROCESS-ENTER-KEY` (CORPT00C-011 to CORPT00C-028).
fn edit_custom(
    input: &ReportInput<'_>,
    verdict: &impl Fn(&str, DateMask) -> LeResult,
) -> Result<ReportRequest, ReportFailure> {
    let parts = [
        DatePart::new(START_MONTH, input.start_month),
        DatePart::new(START_DAY, input.start_day),
        DatePart::new(START_YEAR, input.start_year),
        DatePart::new(END_MONTH, input.end_month),
        DatePart::new(END_DAY, input.end_day),
        DatePart::new(END_YEAR, input.end_year),
    ];
    if let Some(blank) = parts.iter().find(|part| is_blank(&part.field)) {
        return Err(ReportFailure::input(blank.spec.empty, blank.spec.field));
    }
    let [
        start_month,
        start_day,
        start_year,
        end_month,
        end_day,
        end_year,
    ] = parts;
    let start_month = start_month.edit()?;
    let start_day = start_day.edit()?;
    let start_year = start_year.edit()?;
    let end_month = end_month.edit()?;
    let end_day = end_day.edit()?;
    let end_year = end_year.edit()?;
    let start = accepted_date(
        &date_text(start_year, start_month, start_day),
        START_DATE_INVALID,
        ReportField::StartMonth,
        verdict,
    )?;
    let end = accepted_date(
        &date_text(end_year, end_month, end_day),
        END_DATE_INVALID,
        ReportField::EndMonth,
        verdict,
    )?;
    Ok(ReportRequest {
        kind: ReportKind::Custom,
        start,
        end,
    })
}

/// CSUTLDTC's check of one custom date (CORPT00C-024 to CORPT00C-026): its typed value, or the
/// date's message on `field`.
fn accepted_date(
    text: &str,
    message: &'static str,
    field: ReportField,
    verdict: &impl Fn(&str, DateMask) -> LeResult,
) -> Result<NaiveDate, ReportFailure> {
    // HALTED(H5): which dates CEEDAYS accepts lives outside the repository; the verdict decides
    // and is no parity result (CORPT00C-027).
    let result = verdict(text, DateMask::YyyyMmDdDash);
    if !caller_accepts(&result) {
        return Err(ReportFailure::input(message, field));
    }
    // Only a substituted message-2513 verdict accepts text that spells no calendar date. The COBOL
    // would submit that text, but a typed date cannot hold it (Typed Dates and Timestamps), so it
    // is refused with the date's own message: the replacement's choice for halt candidate
    // X-CSUTLDTC-H2, no parity result.
    result
        .date()
        .ok_or_else(|| ReportFailure::input(message, field))
}

/// `WS-START-DATE` or `WS-END-DATE` built from the normalised parts: four year digits, `-`, two
/// month digits, `-`, two day digits (CORPT00C-024). A two-character part is at most 99 and a
/// four-character one at most 9999, so each fills its width exactly.
fn date_text(year: u32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

/// `FUNCTION NUMVAL-C` of a date part stored into its unsigned `PIC 99` or `PIC 9999` item
/// (CORPT00C-017): digits with spaces before or after them give their value, which never exceeds
/// the item, because the part and the item have the same number of digits.
///
/// `None` for any other text: a sign, a currency sign, a comma, a decimal point, an inner space or
/// a letter. The request DTO admits none of them (X-CORPT00C-5), and what the COBOL makes of a
/// text outside `NUMVAL-C`'s format is halt candidate X-CORPT00C-H1.
fn numval_c(field: &str) -> Option<u32> {
    let digits = field.trim_matches(' ');
    if digits.is_empty() {
        return None;
    }
    digits.chars().try_fold(0_u32, |value, c| {
        let digit = c.to_digit(10)?;
        value.checked_mul(10)?.checked_add(digit)
    })
}

/// `SUBMIT-JOB-TO-INTRDR` before its write loop: what the confirmation decides for `request`
/// (CORPT00C-030 to CORPT00C-033). `raw_confirm` is the raw text of `CONFIRM`.
///
/// The confirmation is read only after every date edit has passed, so a request with a blank
/// confirmation first gets any date message that applies (CORPT00C-030).
///
/// # Errors
///
/// Status class [`FailureClass::Input`], on `confirm`:
///
/// - a blank confirmation: `Please confirm to print the `, the report name and ` report...`,
///   such as `Please confirm to print the Monthly report...` (CORPT00C-030);
/// - any other character than `Y`, `y`, `N` and `n`: `"`, that character and
///   `" is not a valid value to confirm...` (CORPT00C-033).
pub fn confirm(request: ReportRequest, raw_confirm: &str) -> Result<ConfirmOutcome, ReportFailure> {
    let field = fit(raw_confirm, CONFIRM_WIDTH);
    if is_blank(&field) {
        let name = request.kind.name();
        return Err(ReportFailure::input(
            format!("{CONFIRM_PROMPT_HEAD}{name}{CONFIRM_PROMPT_TAIL}"),
            ReportField::Confirm,
        ));
    }
    match field.as_str() {
        "Y" | "y" => Ok(ConfirmOutcome::Submit(request)),
        "N" | "n" => Ok(ConfirmOutcome::Cleared),
        _ => {
            let value = delimited_by_space(&field);
            Err(ReportFailure::input(
                format!("\"{value}{CONFIRM_INVALID_TAIL}"),
                ReportField::Confirm,
            ))
        }
    }
}

/// One entry of `JOB-DATA-1` (CORPT00C-034): a fixed text, or a record holding the dates.
#[derive(Clone, Copy)]
enum JobLine {
    /// A `FILLER PIC X(80)` whose `VALUE` is the text.
    Text(&'static str),
    /// `FILLER-1`: `PARM-START-DATE,C'` (`X(18)`), `PARM-START-DATE-1 X(10)`, `'` (`X(52)`).
    StartParm,
    /// `FILLER-2`: `PARM-END-DATE,C'` (`X(16)`), `PARM-END-DATE-1 X(10)`, `'` (`X(54)`).
    EndParm,
    /// `FILLER-3`: `PARM-START-DATE-2 X(10)`, a space, `PARM-END-DATE-2 X(10)`, `X(59)` spaces.
    DateParm,
}

/// `JOB-DATA-1` in order (`CORPT00C.cbl:81-125`).
const JOB_DATA: [JobLine; DECK_RECORD_COUNT] = [
    JobLine::Text("//TRNRPT00 JOB 'TRAN REPORT',CLASS=A,MSGCLASS=0,"),
    JobLine::Text("// NOTIFY=&SYSUID"),
    JobLine::Text("//*"),
    JobLine::Text("//JOBLIB JCLLIB ORDER=('AWS.M2.CARDDEMO.PROC')"),
    JobLine::Text("//*"),
    JobLine::Text("//STEP10 EXEC PROC=TRANREPT"),
    JobLine::Text("//*"),
    JobLine::Text("//STEP05R.SYMNAMES DD *"),
    JobLine::Text("TRAN-CARD-NUM,263,16,ZD"),
    JobLine::Text("TRAN-PROC-DT,305,10,CH"),
    JobLine::StartParm,
    JobLine::EndParm,
    JobLine::Text("/*"),
    JobLine::Text("//STEP10R.DATEPARM DD *"),
    JobLine::DateParm,
    JobLine::Text("/*"),
    JobLine::Text("/*EOF"),
];

impl JobLine {
    /// The entry's 80 bytes, with the 10-character dates `start` and `end` in their places.
    fn record(self, start: &str, end: &str) -> PicX<DECK_RECORD_LEN> {
        let text = match self {
            Self::Text(text) => text.to_owned(),
            Self::StartParm => format!("PARM-START-DATE,C'{start}'"),
            Self::EndParm => format!("PARM-END-DATE,C'{end}'"),
            Self::DateParm => format!("{start} {end}"),
        };
        PicX::moved_from(text.as_bytes())
    }
}

/// The records `SUBMIT-JOB-TO-INTRDR` writes for `request`, in order: the 17 entries of
/// `JOB-DATA-1`, each its text padded with spaces to 80 bytes, the start and end dates in their
/// places, `/*EOF` the last (CORPT00C-034, CORPT00C-035).
///
/// The loop writes each entry and stops after the one equal to `/*EOF`; no entry before it is
/// blank, so it writes exactly these records and reads nothing beyond them (X-CORPT00C-3).
/// Records 8 to 13 override the `SYMNAMES` of the `SORT` step `STEP05R` of procedure `TRANREPT`,
/// and records 14 to 16 supply the `DATEPARM` record of its `CBTRN03C` step `STEP10R`. Each date
/// is its `YYYY-MM-DD` text. No user ID, report content or card number enters the deck.
#[must_use]
pub fn job_deck(request: &ReportRequest) -> Vec<PicX<DECK_RECORD_LEN>> {
    let start = deck_date(request.start);
    let end = deck_date(request.end);
    JOB_DATA
        .iter()
        .map(|line| line.record(&start, &end))
        .collect()
}

/// `date` as the 10 characters `YYYY-MM-DD` a `PIC X(10)` date field of the deck holds.
fn deck_date(date: NaiveDate) -> String {
    // TRUNCATION: a deck date has four year digits. Edited dates and the Monthly and Yearly dates
    // of a clock inside the COBOL's range already fit them; a clock beyond year 9999 (halt
    // candidate X-CORPT00C-H3) keeps the low-order digits, as a `PIC 9(04)` year receiver does,
    // so the date always fills exactly its 10 characters.
    let year = date.year().rem_euclid(FOUR_DIGIT_YEARS);
    format!("{year:04}-{:02}-{:02}", date.month(), date.day())
}

/// The message of a written deck: the report name up to its first space and
/// ` report submitted for printing ...`, such as `Monthly report submitted for printing ...`
/// (CORPT00C-038). `INITIALIZE-ALL-FIELDS` empties `WS-MESSAGE` first, so nothing precedes it.
#[must_use]
pub fn submitted_message(kind: ReportKind) -> String {
    let name = delimited_by_space(kind.name());
    format!("{name}{SUBMITTED_TAIL}")
}

/// The failure of a deck write: `Unable to Write TDQ (JOBS)...` on `monthly`, status class
/// [`FailureClass::Write`] (CORPT00C-037).
///
/// `api::services::corpt00c` returns it when the generation file cannot be created or a record
/// cannot be written. The task ends at once, so no submission message follows.
#[must_use]
pub fn tdq_write_failure() -> ReportFailure {
    ReportFailure {
        message: WRITE_FAILURE.to_owned(),
        field: ReportField::Monthly,
        class: FailureClass::Write,
    }
}

/// `text` moved into a `PIC X(width)` field: its first `width` characters, padded with spaces.
fn fit(text: &str, width: usize) -> String {
    text.chars().chain(iter::repeat(' ')).take(width).collect()
}

/// `= SPACES OR LOW-VALUES`: every character a space, or every one `\0`.
fn is_blank(field: &str) -> bool {
    field.chars().all(|c| c == ' ') || field.chars().all(|c| c == '\0')
}

/// A `STRING` source `DELIMITED BY SPACE`: the text before its first space.
fn delimited_by_space(text: &str) -> &str {
    text.split(' ').next().unwrap_or(text)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use chrono::NaiveDate;
    use copybook::PicX;

    use super::super::csutldtc::{DateMask, LeFeedback, LeResult};
    use super::ReportField::{Confirm, EndDay, EndMonth, EndYear, StartDay, StartMonth, StartYear};
    use super::{
        ConfirmOutcome, DECK_RECORD_COUNT, DECK_RECORD_LEN, FailureClass, ReportFailure,
        ReportField, ReportInput, ReportKind, ReportRequest, confirm, date_window, edit, edit_with,
        job_deck, numval_c, submitted_message, tdq_write_failure,
    };
    use crate::clock::FixedClock;

    type TestResult = Result<(), String>;

    /// A failure as the response shows it: message, field and HTTP status.
    type Shown = (String, ReportField, u16);

    // The audited literals, written out apart from the module's own constants.
    const SELECT: &str = "Select a report type to print report...";
    const SM_EMPTY: &str = "Start Date - Month can NOT be empty...";
    const SD_EMPTY: &str = "Start Date - Day can NOT be empty...";
    const SY_EMPTY: &str = "Start Date - Year can NOT be empty...";
    const EM_EMPTY: &str = "End Date - Month can NOT be empty...";
    const ED_EMPTY: &str = "End Date - Day can NOT be empty...";
    const EY_EMPTY: &str = "End Date - Year can NOT be empty...";
    const SM_INVALID: &str = "Start Date - Not a valid Month...";
    const SD_INVALID: &str = "Start Date - Not a valid Day...";
    const SY_INVALID: &str = "Start Date - Not a valid Year...";
    const EM_INVALID: &str = "End Date - Not a valid Month...";
    const ED_INVALID: &str = "End Date - Not a valid Day...";
    const EY_INVALID: &str = "End Date - Not a valid Year...";
    const START_INVALID: &str = "Start Date - Not a valid date...";
    const END_INVALID: &str = "End Date - Not a valid date...";

    /// Six valid custom parts: 2022-07-01 to 2022-07-31.
    const JULY: [&str; 6] = ["07", "01", "2022", "07", "31", "2022"];

    fn day(year: i32, month: u32, day: u32) -> Result<NaiveDate, String> {
        NaiveDate::from_ymd_opt(year, month, day).ok_or_else(|| format!("{year}-{month}-{day}"))
    }

    /// A clock stopped at 13:45:07 on the given day.
    fn clock_on((year, month, date): (i32, u32, u32)) -> Result<FixedClock, String> {
        let at = day(year, month, date)?
            .and_hms_opt(13, 45, 7)
            .ok_or("no such time")?;
        Ok(FixedClock::new(at))
    }

    /// The `FixedClock` instant the audit rows name: 2022-07-18.
    fn reference() -> Result<FixedClock, String> {
        clock_on((2022, 7, 18))
    }

    /// A verdict that accepts every date, substituted for the H5 call.
    fn accept_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::Valid)
    }

    /// A verdict that rejects every date with a non-2513 message, substituted for the H5 call.
    fn reject_all(text: &str, mask: DateMask) -> LeResult {
        LeResult::new(text, mask, LeFeedback::BadDateValue)
    }

    /// A custom request with these six parts, in the order start month, day, year, end month,
    /// day, year.
    fn custom(parts: [&str; 6]) -> ReportInput<'_> {
        let [
            start_month,
            start_day,
            start_year,
            end_month,
            end_day,
            end_year,
        ] = parts;
        ReportInput {
            custom: "X",
            start_month,
            start_day,
            start_year,
            end_month,
            end_day,
            end_year,
            ..ReportInput::default()
        }
    }

    fn shown(failure: &ReportFailure) -> Shown {
        (
            failure.message().to_owned(),
            failure.field(),
            failure.class().http_status(),
        )
    }

    fn failed(result: &Result<ReportRequest, ReportFailure>) -> Option<Shown> {
        result.as_ref().err().map(shown)
    }

    fn input_failure(message: &str, field: ReportField) -> Shown {
        (message.to_owned(), field, 400)
    }

    fn dates(request: &ReportRequest) -> (ReportKind, NaiveDate, NaiveDate) {
        (request.kind(), request.start(), request.end())
    }

    fn submitted(input: &ReportInput<'_>, clock: &FixedClock) -> Result<ReportRequest, String> {
        edit_with(input, clock, accept_all).map_err(|failure| failure.to_string())
    }

    #[test]
    fn report_kinds_carry_their_cobol_names() {
        assert_eq!(ReportKind::Monthly.name(), "Monthly");
        assert_eq!(ReportKind::Yearly.name(), "Yearly");
        assert_eq!(ReportKind::Custom.name(), "Custom");
    }

    // audit: CORPT00C-008, CORPT00C-009
    #[test]
    fn monthly_runs_from_the_first_to_the_last_day_of_the_current_month() -> TestResult {
        let cases = [
            ((2022, 7, 18), (2022, 7, 1), (2022, 7, 31)),
            ((2022, 12, 5), (2022, 12, 1), (2022, 12, 31)),
            ((2024, 2, 10), (2024, 2, 1), (2024, 2, 29)),
            ((2023, 2, 10), (2023, 2, 1), (2023, 2, 28)),
            ((2022, 4, 30), (2022, 4, 1), (2022, 4, 30)),
            ((2022, 1, 1), (2022, 1, 1), (2022, 1, 31)),
        ];
        for (today, (fy, fm, fd), (ly, lm, ld)) in cases {
            let clock = clock_on(today)?;
            let (first, last) = (day(fy, fm, fd)?, day(ly, lm, ld)?);
            assert_eq!(
                date_window(ReportKind::Monthly, &clock),
                Some((first, last))
            );
            let input = ReportInput {
                monthly: "X",
                ..ReportInput::default()
            };
            let request = edit(&input, &clock).map_err(|failure| failure.to_string())?;
            assert_eq!(
                dates(&request),
                (ReportKind::Monthly, first, last),
                "{today:?}"
            );
        }
        Ok(())
    }

    // audit: CORPT00C-010
    #[test]
    fn yearly_runs_from_the_first_to_the_last_day_of_the_current_year() -> TestResult {
        for today in [(2022, 7, 18), (2024, 2, 29), (2023, 12, 31), (2023, 1, 1)] {
            let clock = clock_on(today)?;
            let (first, last) = (day(today.0, 1, 1)?, day(today.0, 12, 31)?);
            assert_eq!(date_window(ReportKind::Yearly, &clock), Some((first, last)));
            let input = ReportInput {
                yearly: "X",
                ..ReportInput::default()
            };
            let request = edit(&input, &clock).map_err(|failure| failure.to_string())?;
            assert_eq!(
                dates(&request),
                (ReportKind::Yearly, first, last),
                "{today:?}"
            );
        }
        assert_eq!(date_window(ReportKind::Custom, &reference()?), None);
        Ok(())
    }

    // audit: CORPT00C-007
    #[test]
    fn the_first_selector_not_blank_picks_the_report() -> TestResult {
        let clock = reference()?;
        // (monthly, yearly, custom, report); the custom parts are blank throughout, so only a
        // custom report taken would read them.
        let cases = [
            ("N", "", "", ReportKind::Monthly),
            ("X", "X", "X", ReportKind::Monthly),
            ("", "Y", "X", ReportKind::Yearly),
            ("\0", "?", "", ReportKind::Yearly),
            ("Monthly", "", "", ReportKind::Monthly),
        ];
        for (monthly, yearly, custom, kind) in cases {
            let input = ReportInput {
                monthly,
                yearly,
                custom,
                ..ReportInput::default()
            };
            let request = edit(&input, &clock).map_err(|failure| failure.to_string())?;
            assert_eq!(request.kind(), kind, "{input:?}");
        }
        // A selector holding a space first is blank as its one-character field holds it.
        let only_custom = ReportInput {
            monthly: " X",
            ..custom(JULY)
        };
        assert_eq!(submitted(&only_custom, &clock)?.kind(), ReportKind::Custom);
        Ok(())
    }

    // audit: CORPT00C-029
    #[test]
    fn no_selector_asks_for_a_report_type() -> TestResult {
        let clock = reference()?;
        for blank in ["", " ", "\0"] {
            let input = ReportInput {
                monthly: blank,
                yearly: blank,
                custom: blank,
                start_month: "13",
                confirm: "Y",
                ..ReportInput::default()
            };
            let result = edit(&input, &clock);
            assert_eq!(
                failed(&result),
                Some(input_failure(SELECT, ReportField::Monthly))
            );
        }
        Ok(())
    }

    // audit: CORPT00C-006, CORPT00C-011, CORPT00C-012, CORPT00C-013, CORPT00C-014
    // audit: CORPT00C-015, CORPT00C-016
    #[test]
    fn the_first_blank_part_decides_the_message() -> TestResult {
        let clock = reference()?;
        let cases = [
            (["", "", "", "", "", ""], SM_EMPTY, StartMonth),
            (["13", "  ", "", "", "", ""], SD_EMPTY, StartDay),
            (["07", "99", "\0\0\0\0", "", "", ""], SY_EMPTY, StartYear),
            (["07", "01", "2022", " ", "", "2022"], EM_EMPTY, EndMonth),
            (["07", "01", "2022", "07", "\0\0", ""], ED_EMPTY, EndDay),
            (["07", "01", "2022", "07", "31", "    "], EY_EMPTY, EndYear),
        ];
        for (parts, message, field) in cases {
            let result = edit_with(&custom(parts), &clock, accept_all);
            assert_eq!(
                failed(&result),
                Some(input_failure(message, field)),
                "{parts:?}"
            );
        }
        Ok(())
    }

    // audit: CORPT00C-017, CORPT00C-024
    #[test]
    fn parts_are_normalised_and_dates_built_start_first() -> TestResult {
        let clock = reference()?;
        let seen = RefCell::new(Vec::new());
        let recording = |text: &str, mask: DateMask| {
            seen.borrow_mut().push((text.to_owned(), mask));
            accept_all(text, mask)
        };
        let input = custom(["1", "5", "2022", " 7", "4 ", "22"]);
        let request = edit_with(&input, &clock, recording).map_err(|f| f.to_string())?;
        let expected = vec![
            ("2022-01-05".to_owned(), DateMask::YyyyMmDdDash),
            ("0022-07-04".to_owned(), DateMask::YyyyMmDdDash),
        ];
        assert_eq!(seen.into_inner(), expected);
        assert_eq!(
            dates(&request),
            (ReportKind::Custom, day(2022, 1, 5)?, day(22, 7, 4)?)
        );
        Ok(())
    }

    // audit: CORPT00C-017
    #[test]
    fn numval_c_reads_digits_with_surrounding_spaces_only() {
        let cases = [
            ("07", Some(7)),
            (" 7", Some(7)),
            ("7 ", Some(7)),
            ("2022", Some(2022)),
            ("  22", Some(22)),
            ("0000", Some(0)),
            ("  ", None),
            ("", None),
            ("1 2", None),
            ("-3", None),
            ("1.", None),
            ("\0\0", None),
        ];
        for (field, value) in cases {
            assert_eq!(numval_c(field), value, "{field:?}");
        }
    }

    // audit: CORPT00C-018, CORPT00C-019, CORPT00C-020, CORPT00C-021, CORPT00C-022, CORPT00C-023
    #[test]
    fn months_above_12_and_days_above_31_fail_in_part_order() -> TestResult {
        let clock = reference()?;
        let cases = [
            (
                ["13", "32", "2022", "13", "32", "2022"],
                SM_INVALID,
                StartMonth,
            ),
            (
                ["99", "01", "2022", "01", "01", "2022"],
                SM_INVALID,
                StartMonth,
            ),
            (
                ["12", "32", "2022", "13", "01", "2022"],
                SD_INVALID,
                StartDay,
            ),
            (
                ["12", "31", "2022", "13", "32", "2022"],
                EM_INVALID,
                EndMonth,
            ),
            (["12", "31", "2022", "12", "99", "2022"], ED_INVALID, EndDay),
        ];
        for (parts, message, field) in cases {
            let result = edit_with(&custom(parts), &clock, accept_all);
            assert_eq!(
                failed(&result),
                Some(input_failure(message, field)),
                "{parts:?}"
            );
        }
        // No year is rejected by its part edit, and every year reaches the date edit.
        let request = submitted(&custom(["01", "01", "0000", "12", "31", "9999"]), &clock)?;
        assert_eq!(
            dates(&request),
            (ReportKind::Custom, day(0, 1, 1)?, day(9999, 12, 31)?)
        );
        Ok(())
    }

    // audit: CORPT00C-018, CORPT00C-019
    #[test]
    fn month_and_day_00_pass_their_part_edits() -> TestResult {
        let clock = reference()?;
        let seen = RefCell::new(Vec::new());
        let recording = |text: &str, mask: DateMask| {
            seen.borrow_mut().push(text.to_owned());
            reject_all(text, mask)
        };
        let result = edit_with(
            &custom(["00", "00", "2022", "1", "1", "2022"]),
            &clock,
            recording,
        );
        assert_eq!(
            failed(&result),
            Some(input_failure(START_INVALID, StartMonth))
        );
        assert_eq!(seen.into_inner(), vec!["2022-00-00".to_owned()]);
        Ok(())
    }

    // Halt candidate X-CORPT00C-H1 and escalation candidate X-CORPT00C-5: BLOCKED. A part that
    // is not digits has no single COBOL result and the request DTO never passes one; these pin
    // the replacement's choice only, the part's own message, and assert no parity.
    #[test]
    fn a_part_that_is_not_digits_fails_its_own_part_edit() -> TestResult {
        let clock = reference()?;
        let cases = [
            (
                ["1a", "01", "2022", "01", "01", "2022"],
                SM_INVALID,
                StartMonth,
            ),
            (
                ["01", "+1", "2022", "01", "01", "2022"],
                SD_INVALID,
                StartDay,
            ),
            (
                ["01", "01", "20x2", "01", "01", "2022"],
                SY_INVALID,
                StartYear,
            ),
            (
                ["01", "01", "1 22", "01", "01", "2022"],
                SY_INVALID,
                StartYear,
            ),
            (
                ["01", "01", "2022", "1.", "01", "2022"],
                EM_INVALID,
                EndMonth,
            ),
            (
                ["01", "01", "2022", "01", "\u{e9}", "2022"],
                ED_INVALID,
                EndDay,
            ),
            (
                ["01", "01", "2022", "01", "01", "2O22"],
                EY_INVALID,
                EndYear,
            ),
            (
                ["01", "01", "2022", "01", "01", "\0 22"],
                EY_INVALID,
                EndYear,
            ),
        ];
        for (parts, message, field) in cases {
            let result = edit_with(&custom(parts), &clock, accept_all);
            assert_eq!(
                failed(&result),
                Some(input_failure(message, field)),
                "{parts:?}"
            );
        }
        Ok(())
    }

    // audit: CORPT00C-025, CORPT00C-026
    #[test]
    fn a_rejected_date_names_its_month_and_stops_the_edits() -> TestResult {
        let clock = reference()?;
        let seen = RefCell::new(Vec::new());
        let recording = |text: &str, mask: DateMask| {
            seen.borrow_mut().push(text.to_owned());
            reject_all(text, mask)
        };
        let result = edit_with(&custom(JULY), &clock, recording);
        assert_eq!(
            failed(&result),
            Some(input_failure(START_INVALID, StartMonth))
        );
        // The end date is never checked once the start date is rejected.
        assert_eq!(seen.into_inner(), vec!["2022-07-01".to_owned()]);

        let start_only = |text: &str, mask: DateMask| {
            if text == "2022-07-01" {
                accept_all(text, mask)
            } else {
                reject_all(text, mask)
            }
        };
        let result = edit_with(&custom(JULY), &clock, start_only);
        assert_eq!(failed(&result), Some(input_failure(END_INVALID, EndMonth)));
        Ok(())
    }

    // audit: CORPT00C-025, CORPT00C-026, CORPT00C-028
    #[test]
    fn message_2513_accepts_and_other_rejections_refuse() -> TestResult {
        let clock = reference()?;
        let verdicts = [
            (LeFeedback::UnsupportedRange, true),
            (LeFeedback::InvalidMonth, false),
            (LeFeedback::NonNumericData, false),
            (
                LeFeedback::Other {
                    severity: 0,
                    msg_no: 2513,
                },
                true,
            ),
            (
                LeFeedback::Other {
                    severity: 2,
                    msg_no: 1,
                },
                false,
            ),
        ];
        for (feedback, accepted) in verdicts {
            let verdict = |text: &str, mask: DateMask| LeResult::new(text, mask, feedback);
            let result = edit_with(&custom(JULY), &clock, verdict);
            if accepted {
                let request = result.map_err(|failure| failure.to_string())?;
                assert_eq!(
                    dates(&request),
                    (ReportKind::Custom, day(2022, 7, 1)?, day(2022, 7, 31)?)
                );
            } else {
                assert_eq!(
                    failed(&result),
                    Some(input_failure(START_INVALID, StartMonth)),
                    "{feedback:?}"
                );
            }
        }
        // Accepted through 2513, text that spells no calendar date cannot be typed: refused with
        // the date's message (X-CSUTLDTC-H2, the replacement's choice, BLOCKED).
        let range =
            |text: &str, mask: DateMask| LeResult::new(text, mask, LeFeedback::UnsupportedRange);
        let result = edit_with(
            &custom(["02", "30", "2022", "07", "31", "2022"]),
            &clock,
            range,
        );
        assert_eq!(
            failed(&result),
            Some(input_failure(START_INVALID, StartMonth))
        );
        let result = edit_with(
            &custom(["07", "01", "2022", "06", "31", "2022"]),
            &clock,
            range,
        );
        assert_eq!(failed(&result), Some(input_failure(END_INVALID, EndMonth)));
        Ok(())
    }

    // audit: CORPT00C-027
    #[test]
    fn the_stand_in_verdict_decides_custom_dates() -> TestResult {
        // BLOCKED (H5): these pin the stand-in verdict of `validate` only; no date is asserted
        // valid or invalid as a COBOL parity result.
        let clock = reference()?;
        let request = edit(&custom(["02", "29", "2024", "03", "01", "2024"]), &clock)
            .map_err(|failure| failure.to_string())?;
        assert_eq!(request.start(), day(2024, 2, 29)?);
        let result = edit(&custom(["02", "29", "2023", "03", "01", "2023"]), &clock);
        assert_eq!(
            failed(&result),
            Some(input_failure(START_INVALID, StartMonth))
        );
        let result = edit(&custom(["01", "01", "2023", "04", "31", "2023"]), &clock);
        assert_eq!(failed(&result), Some(input_failure(END_INVALID, EndMonth)));
        let early = edit(&custom(["01", "01", "1500", "01", "01", "1500"]), &clock)
            .map_err(|failure| failure.to_string())?;
        assert_eq!(early.start(), day(1500, 1, 1)?);
        Ok(())
    }

    // audit: CORPT00C-024, CORPT00C-028
    #[test]
    fn a_custom_window_is_kept_as_entered_even_reversed() -> TestResult {
        let clock = reference()?;
        let request = submitted(&custom(["12", "31", "2022", "01", "01", "2022"]), &clock)?;
        assert_eq!(
            dates(&request),
            (ReportKind::Custom, day(2022, 12, 31)?, day(2022, 1, 1)?)
        );
        Ok(())
    }

    /// One request of each report, under the reference clock.
    fn each_kind() -> Result<[ReportRequest; 3], String> {
        let clock = reference()?;
        let monthly = ReportInput {
            monthly: "X",
            ..ReportInput::default()
        };
        let yearly = ReportInput {
            yearly: "X",
            ..ReportInput::default()
        };
        Ok([
            submitted(&monthly, &clock)?,
            submitted(&yearly, &clock)?,
            submitted(&custom(JULY), &clock)?,
        ])
    }

    // audit: CORPT00C-030
    #[test]
    fn a_blank_confirmation_prompts_with_the_report_name() -> TestResult {
        let prompts = [
            "Please confirm to print the Monthly report...",
            "Please confirm to print the Yearly report...",
            "Please confirm to print the Custom report...",
        ];
        for (request, prompt) in each_kind()?.into_iter().zip(prompts) {
            for blank in ["", " ", "\0", " Y"] {
                let outcome = confirm(request, blank);
                let shown_failure = outcome.as_ref().err().map(shown);
                assert_eq!(shown_failure, Some(input_failure(prompt, Confirm)));
            }
        }
        Ok(())
    }

    // audit: CORPT00C-031, CORPT00C-032
    #[test]
    fn y_submits_and_n_clears() -> TestResult {
        for request in each_kind()? {
            for yes in ["Y", "y", "Yes"] {
                assert_eq!(confirm(request, yes), Ok(ConfirmOutcome::Submit(request)));
            }
            for no in ["N", "n", "no"] {
                assert_eq!(confirm(request, no), Ok(ConfirmOutcome::Cleared));
            }
        }
        Ok(())
    }

    // audit: CORPT00C-033
    #[test]
    fn any_other_confirmation_is_quoted_back() -> TestResult {
        let [monthly, ..] = each_kind()?;
        let cases = [
            ("X", "\"X\" is not a valid value to confirm..."),
            ("1", "\"1\" is not a valid value to confirm..."),
            ("Xy", "\"X\" is not a valid value to confirm..."),
            ("\u{e9}", "\"\u{e9}\" is not a valid value to confirm..."),
        ];
        for (raw, message) in cases {
            let outcome = confirm(monthly, raw);
            let shown_failure = outcome.as_ref().err().map(shown);
            assert_eq!(shown_failure, Some(input_failure(message, Confirm)));
        }
        assert_eq!(
            "\"X\" is not a valid value to confirm...".chars().count(),
            38
        );
        Ok(())
    }

    /// Each record of `deck` as text, without its trailing spaces.
    fn deck_texts(deck: &[PicX<DECK_RECORD_LEN>]) -> Vec<String> {
        deck.iter().map(PicX::to_text).collect()
    }

    /// The 17 records of CORPT00C-034 for start date `start` and end date `end`, unpadded.
    fn expected_deck(start: &str, end: &str) -> Vec<String> {
        [
            "//TRNRPT00 JOB 'TRAN REPORT',CLASS=A,MSGCLASS=0,",
            "// NOTIFY=&SYSUID",
            "//*",
            "//JOBLIB JCLLIB ORDER=('AWS.M2.CARDDEMO.PROC')",
            "//*",
            "//STEP10 EXEC PROC=TRANREPT",
            "//*",
            "//STEP05R.SYMNAMES DD *",
            "TRAN-CARD-NUM,263,16,ZD",
            "TRAN-PROC-DT,305,10,CH",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([
            format!("PARM-START-DATE,C'{start}'"),
            format!("PARM-END-DATE,C'{end}'"),
            "/*".to_owned(),
            "//STEP10R.DATEPARM DD *".to_owned(),
            format!("{start} {end}"),
            "/*".to_owned(),
            "/*EOF".to_owned(),
        ])
        .collect()
    }

    // audit: CORPT00C-034, CORPT00C-035
    #[test]
    fn the_deck_is_the_17_records_of_job_data() -> TestResult {
        let [monthly, yearly, custom_july] = each_kind()?;
        let cases = [
            (monthly, "2022-07-01", "2022-07-31"),
            (yearly, "2022-01-01", "2022-12-31"),
            (custom_july, "2022-07-01", "2022-07-31"),
        ];
        for (request, start, end) in cases {
            let deck = job_deck(&request);
            assert_eq!(deck.len(), DECK_RECORD_COUNT);
            assert_eq!(deck_texts(&deck), expected_deck(start, end));
            // Each record is its text padded with spaces to exactly 80 bytes.
            for (record, text) in deck.iter().zip(expected_deck(start, end)) {
                assert_eq!(record.as_bytes().len(), DECK_RECORD_LEN);
                assert_eq!(
                    record.as_bytes().as_slice(),
                    format!("{text:<80}").as_bytes()
                );
            }
            assert_eq!(deck.last().map(PicX::to_text).as_deref(), Some("/*EOF"));
            assert!(deck.iter().all(|record| !record.is_spaces()));
        }
        Ok(())
    }

    // audit: CORPT00C-034
    #[test]
    fn deck_dates_sit_at_their_cobol_offsets() -> TestResult {
        let [monthly, ..] = each_kind()?;
        let deck = job_deck(&monthly);
        let bytes = |record: usize, from: usize, to: usize| {
            deck.get(record)
                .and_then(|r| r.as_bytes().get(from..to))
                .map(<[u8]>::to_vec)
        };
        // FILLER-1: X(18), PARM-START-DATE-1 X(10), X(52) starting with the quote.
        assert_eq!(bytes(10, 18, 28), Some(b"2022-07-01".to_vec()));
        assert_eq!(bytes(10, 28, 29), Some(b"'".to_vec()));
        // FILLER-2: X(16), PARM-END-DATE-1 X(10), X(54) starting with the quote.
        assert_eq!(bytes(11, 16, 26), Some(b"2022-07-31".to_vec()));
        assert_eq!(bytes(11, 26, 27), Some(b"'".to_vec()));
        // FILLER-3: PARM-START-DATE-2 X(10), a space, PARM-END-DATE-2 X(10), X(59).
        assert_eq!(bytes(14, 0, 21), Some(b"2022-07-01 2022-07-31".to_vec()));
        assert_eq!(bytes(14, 21, 80), Some(vec![b' '; 59]));
        Ok(())
    }

    // audit: CORPT00C-017, CORPT00C-034
    #[test]
    fn the_deck_holds_normalised_custom_dates() -> TestResult {
        let request = submitted(&custom(["1", "5", "22", "12", "9", "2022"]), &reference()?)?;
        assert_eq!(
            deck_texts(&job_deck(&request)),
            expected_deck("0022-01-05", "2022-12-09")
        );
        Ok(())
    }

    // Halt candidate X-CORPT00C-H3: BLOCKED. A clock in December 9999 or beyond year 9999 has no
    // single COBOL Monthly date; this pins only that the replacement stays total and keeps each
    // deck date at 10 characters.
    #[test]
    fn a_clock_beyond_the_cobol_range_keeps_ten_character_dates() -> TestResult {
        let monthly = ReportInput {
            monthly: "X",
            ..ReportInput::default()
        };
        let december = submitted(&monthly, &clock_on((9999, 12, 15))?)?;
        assert_eq!(december.end(), day(9999, 12, 31)?);
        let late = submitted(&monthly, &clock_on((12_024, 3, 15))?)?;
        let deck = job_deck(&late);
        assert_eq!(
            deck.get(14).map(PicX::to_text).as_deref(),
            Some("2024-03-01 2024-03-31")
        );
        Ok(())
    }

    // audit: CORPT00C-038
    #[test]
    fn a_written_deck_reports_its_report_name() {
        assert_eq!(
            submitted_message(ReportKind::Monthly),
            "Monthly report submitted for printing ..."
        );
        assert_eq!(
            submitted_message(ReportKind::Yearly),
            "Yearly report submitted for printing ..."
        );
        assert_eq!(
            submitted_message(ReportKind::Custom),
            "Custom report submitted for printing ..."
        );
    }

    // audit: CORPT00C-037
    #[test]
    fn a_failed_write_is_a_500_on_monthly() {
        let failure = tdq_write_failure();
        assert_eq!(
            shown(&failure),
            (
                "Unable to Write TDQ (JOBS)...".to_owned(),
                ReportField::Monthly,
                500
            )
        );
        assert_eq!(failure.class(), FailureClass::Write);
        assert_eq!(failure.to_string(), failure.message());
        assert_eq!(FailureClass::Input.http_status(), 400);
    }

    // audit: CORPT00C-006
    #[test]
    fn every_message_fits_the_78_byte_message_row() {
        let longest = [
            SELECT,
            SM_EMPTY,
            SD_EMPTY,
            SY_EMPTY,
            EM_EMPTY,
            ED_EMPTY,
            EY_EMPTY,
            SM_INVALID,
            SD_INVALID,
            SY_INVALID,
            EM_INVALID,
            ED_INVALID,
            EY_INVALID,
            START_INVALID,
            END_INVALID,
            "Please confirm to print the Monthly report...",
            "Unable to Write TDQ (JOBS)...",
            "Monthly report submitted for printing ...",
        ]
        .into_iter()
        .map(str::len)
        .max();
        assert_eq!(longest, Some(45));
        assert!(longest.is_some_and(|len| len <= 78));
    }

    #[test]
    fn every_input_gets_a_value_or_a_failure() -> TestResult {
        // Total Parsers Over Untrusted Input: odd text in every field ends in a value or a
        // failure, never a panic.
        let clock = reference()?;
        let odd = [
            "",
            "\0",
            "\u{e9}\u{e9}\u{e9}",
            "-",
            "99999999",
            " 1 ",
            "\u{1F600}",
            "1\0",
        ];
        for text in odd {
            for position in 0..6 {
                let mut parts = JULY;
                if let Some(part) = parts.get_mut(position) {
                    *part = text;
                }
                let input = custom(parts);
                if let Ok(request) = edit(&input, &clock) {
                    assert_eq!(job_deck(&request).len(), DECK_RECORD_COUNT);
                    let _ = confirm(request, text);
                }
            }
            let selectors = ReportInput {
                monthly: text,
                yearly: text,
                custom: text,
                confirm: text,
                ..ReportInput::default()
            };
            let _ = edit(&selectors, &clock).map(|request| confirm(request, text));
        }
        Ok(())
    }
}
