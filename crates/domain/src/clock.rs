//! Processing clock and the COBOL renderings of clock-dependent fields.
//!
//! Every time-dependent output of the port reads its instant from a [`Clock`]:
//! processing timestamps, the CDRD date and time, the CP00 detail keys and
//! error-log stamps, reward accrual and soft-delete instants. Adapters build an
//! [`AnyClock`] from the `CARDDEMO_CLOCK_OVERRIDE` setting with
//! [`AnyClock::from_override`]; this module reads no environment itself.
//!
//! # Instants
//!
//! A processing instant, and every instant a reward rule, soft delete or
//! accrual stores, is a whole microsecond of UTC time: no part finer than a
//! microsecond and no leap second. That is what a `PostgreSQL` `TIMESTAMPTZ`
//! holds and the resolution the database reads its own clock at, so an
//! instant written and read back equals the one a rule decided with. The
//! policy is enforced where instants enter, never by narrowing one later:
//!
//! - [`SystemClock`] reads the host clock at whole microseconds;
//! - [`parse_instant`] refuses RFC 3339 text finer than a microsecond or on a
//!   leap second; `CARDDEMO_CLOCK_OVERRIDE` and the instants of the reward-rule
//!   requests are read through it, and [`INSTANT_PATTERN`] publishes what it
//!   accepts;
//! - [`is_whole_microsecond`] lets a writer refuse an instant it did not
//!   parse, such as one of a [`FixedClock`] built directly.
//!
//! The formatters stand in for `FUNCTION CURRENT-DATE`, `EXEC CICS FORMATTIME`
//! and `ACCEPT … FROM DATE/DAY`. They render the instant in UTC, because the
//! replacement has no CICS or Language Environment local-time zone and parity
//! compares these fields by format (section F of
//! `docs/parity/parity-oracle-audit.md`). Each text formatter returns exactly
//! the width of its COBOL receiver:
//!
//! - a year is written as its low-order four (or two) digits, as a `MOVE` into
//!   `PIC 9(4)` (or `PIC 9(2)`) truncates, so an instant beyond year 9999 never
//!   widens a field;
//! - a leap second, which `chrono` holds as second 59 with a sub-second part of
//!   one second or more, renders as the last sub-second of second 59, because
//!   no COBOL time field can hold second 60.

use chrono::{DateTime, Datelike, SubsecRound, Timelike, Utc};

/// The fraction digits of a whole microsecond. RFC 3339 text may write more,
/// but only as zeros.
const MICROSECOND_DIGITS: u16 = 6;

/// Nanoseconds per microsecond.
const NANOS_PER_MICROSECOND: u32 = 1_000;

/// Nanoseconds per second. `chrono` holds a leap second as second 59 with a
/// sub-second part of this many nanoseconds or more.
const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// The ECMA-262 pattern of the text [`parse_instant`] accepts, as the
/// `OpenAPI` document publishes it for the reward-rule instants.
///
/// An RFC 3339 date-time: `YYYY-MM-DD`, then `T`, `t` or a space, then
/// `hh:mm:ss` with seconds `00` to `59`; an optional fraction of one or more
/// digits, every digit after the 6th `0`; and `Z`, `z` or an offset `+hh:mm`,
/// `-hh:mm` or `U+2212` `hh:mm` up to 23:59. It is exactly the shape `chrono`'s
/// RFC 3339 parser reads, less the leap second and the digits finer than a
/// microsecond that [`parse_instant`] refuses. Whether the day exists in its
/// month is left to the `date-time` format.
pub const INSTANT_PATTERN: &str = r"^[0-9]{4}-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])[Tt ]([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](\.[0-9]{1,6}0*)?([Zz]|[+\-\u2212]([01][0-9]|2[0-3]):[0-5][0-9])$";

/// Whether `t` is a whole microsecond of UTC time: no part finer than a
/// microsecond, and not a leap second.
///
/// These are the instants a `TIMESTAMPTZ` column stores exactly; the sqlx
/// encoder would silently drop the finer part of any other.
#[must_use]
pub fn is_whole_microsecond(t: DateTime<Utc>) -> bool {
    let nanos = t.nanosecond();
    nanos < NANOS_PER_SECOND && nanos % NANOS_PER_MICROSECOND == 0
}

/// Parses `text` as an RFC 3339 instant that is a whole microsecond of UTC
/// time, and converts it to UTC.
///
/// The text is read by `chrono`'s RFC 3339 parser, whose shape
/// [`INSTANT_PATTERN`] states, and then held to the microsecond. `chrono`
/// would keep 9 fraction digits and skip any beyond them unread, so the digits
/// are checked in the text itself: every one after the 6th must be `0`.
/// `"…00.000001000Z"` and `"…00.000000000Z"` are therefore accepted, and
/// `"…00.000000900Z"` and `"…00.0000000001Z"` refused. Second `60`, which
/// `chrono` accepts as a leap second, is refused too.
///
/// # Errors
///
/// [`InstantError::NotRfc3339`] when `chrono` does not parse the text,
/// [`InstantError::FinerThanMicrosecond`] when a fraction digit after the 6th
/// is not `0`, and [`InstantError::LeapSecond`] for second `60`, checked in
/// that order.
pub fn parse_instant(text: &str) -> Result<DateTime<Utc>, InstantError> {
    let parsed = DateTime::parse_from_rfc3339(text).map_err(InstantError::NotRfc3339)?;
    // Parsed text starts with the 19 ASCII bytes `YYYY-MM-DDThh:mm:ss`; a
    // fraction is the `.` at byte 19 and the run of digits after it.
    let fraction = text
        .as_bytes()
        .get(19..)
        .and_then(|rest| rest.strip_prefix(b"."))
        .unwrap_or_default();
    if fraction
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .skip(usize::from(MICROSECOND_DIGITS))
        .any(|&digit| digit != b'0')
    {
        return Err(InstantError::FinerThanMicrosecond);
    }
    if parsed.nanosecond() >= NANOS_PER_SECOND {
        return Err(InstantError::LeapSecond);
    }
    Ok(parsed.with_timezone(&Utc))
}

/// Why text is not an instant [`parse_instant`] accepts.
///
/// Each text is a reason written to follow the name of the field or setting
/// that held the value, and quotes nothing of the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InstantError {
    /// Not an RFC 3339 date-time; `chrono`'s reason follows.
    #[error("value is not an RFC 3339 date-time: {0}")]
    NotRfc3339(chrono::ParseError),
    /// A fraction digit after the 6th is not `0`: the instant has a part finer
    /// than a microsecond, which a timestamp cannot hold.
    #[error("value is finer than a microsecond: a fraction digit after the 6th is not 0")]
    FinerThanMicrosecond,
    /// Second `60`, a leap second, which a timestamp cannot hold.
    #[error("value is a leap second (second 60), which a timestamp cannot hold")]
    LeapSecond,
}

/// Source of the processing instant.
///
/// The trait is object-safe, so adapters may hold an `Arc<dyn Clock>`, and its
/// `Send + Sync` bound lets a clock cross the threads of a multi-threaded
/// runtime. Rules take `&impl Clock`; the blanket implementation for `&C`
/// lets a borrowed clock be passed on unchanged.
pub trait Clock: Send + Sync {
    /// The current processing instant.
    fn now(&self) -> DateTime<Utc>;
}

impl<C: Clock + ?Sized> Clock for &C {
    fn now(&self) -> DateTime<Utc> {
        (**self).now()
    }
}

/// The host clock, read on every call: `EXEC CICS ASKTIME` and
/// `FUNCTION CURRENT-DATE` in the COBOL.
///
/// The reading is truncated to the whole microsecond, the resolution
/// `PostgreSQL` reads its own clock at and a `TIMESTAMPTZ` keeps, so every
/// instant it gives is [`is_whole_microsecond`]. No COBOL field is finer than
/// the hundredths of `DB2-FORMAT-TS` and the milliseconds of `FORMATTIME`,
/// which the truncation leaves unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        // `Utc::now` never reads a leap second, so whole microseconds remain.
        Utc::now().trunc_subsecs(MICROSECOND_DIGITS)
    }
}

/// A clock pinned to one instant, used when `CARDDEMO_CLOCK_OVERRIDE` is set
/// so that time-dependent output is reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedClock(pub DateTime<Utc>);

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// The clock an adapter runs on: the host clock, or the pinned override.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnyClock {
    /// No override: the host clock.
    System(SystemClock),
    /// `CARDDEMO_CLOCK_OVERRIDE` is set: that instant.
    Fixed(FixedClock),
}

impl AnyClock {
    /// Builds the clock from the value of `CARDDEMO_CLOCK_OVERRIDE`.
    ///
    /// An absent value (`None`) selects the [`SystemClock`]. A supplied value
    /// is trimmed of surrounding whitespace and must then be an RFC 3339
    /// instant of at most microsecond precision and no leap second, read by
    /// [`parse_instant`], which is converted to UTC and selects a
    /// [`FixedClock`]. A supplied value that is empty or only whitespace is an
    /// error, never the system clock, so a mistyped pin cannot fall back to
    /// wall time.
    ///
    /// Treating an empty variable as unset is the settings layer's job
    /// (`Settings` in `crates/runtime`), not this parser's.
    ///
    /// # Errors
    ///
    /// - [`ClockError::InvalidOverride`] when the supplied value, trimmed, is
    ///   not an RFC 3339 instant, including when it is empty or only
    ///   whitespace.
    /// - [`ClockError::InexactOverride`] when it is one, but finer than a
    ///   microsecond or a leap second, so the instants the clock stamps could
    ///   not be stored as pinned.
    pub fn from_override(value: Option<&str>) -> Result<Self, ClockError> {
        let Some(text) = value.map(str::trim) else {
            return Ok(Self::System(SystemClock));
        };
        match parse_instant(text) {
            Ok(instant) => Ok(Self::Fixed(FixedClock(instant))),
            Err(InstantError::NotRfc3339(_)) => Err(ClockError::InvalidOverride(text.to_owned())),
            Err(reason) => Err(ClockError::InexactOverride {
                value: text.to_owned(),
                reason,
            }),
        }
    }
}

impl Clock for AnyClock {
    fn now(&self) -> DateTime<Utc> {
        match self {
            Self::System(clock) => clock.now(),
            Self::Fixed(clock) => clock.now(),
        }
    }
}

/// Failure to build a clock from configuration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClockError {
    /// The override, as trimmed, is not an RFC 3339 instant.
    #[error("CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: {0}")]
    InvalidOverride(String),
    /// The override, as trimmed, is an RFC 3339 instant but not a whole
    /// microsecond of UTC time: finer than a microsecond, or a leap second.
    #[error(
        "CARDDEMO_CLOCK_OVERRIDE is not an instant a timestamp holds exactly: {value}: {reason}"
    )]
    InexactOverride {
        /// The override, as trimmed.
        value: String,
        /// Why the instant cannot be held exactly.
        reason: InstantError,
    },
}

/// The year as the four digits a `PIC 9(4)` receiver keeps.
fn year4(t: DateTime<Utc>) -> i32 {
    t.year().rem_euclid(10_000)
}

/// The year as the two digits a `PIC 9(2)` receiver keeps.
fn year2(t: DateTime<Utc>) -> i32 {
    t.year().rem_euclid(100)
}

/// Nanoseconds within the second, with a leap second held at second 59.
fn subsec_nanos(t: DateTime<Utc>) -> u32 {
    t.nanosecond().min(999_999_999)
}

/// CBTRN02C `Z-GET-DB2-FORMAT-TIMESTAMP` — P-CBTRN02C-220; CBACT04C
/// `Z-GET-DB2-FORMAT-TIMESTAMP` — P-CBACT04C-176.
///
/// `DB2-FORMAT-TS X(26)` built from `FUNCTION CURRENT-DATE`
/// [app/cbl/CBTRN02C.cbl:L692-705] [app/cbl/CBACT04C.cbl:L613-627]:
/// `YYYY-MM-DD-HH.MM.SS.hh0000`, where `hh` is `COB-MIL`, the hundredths of a
/// second (truncated), followed by the literal `0000`. POSTTRAN stamps
/// `TRAN-PROC-TS` with it; INTCALC stamps both `TRAN-ORIG-TS` and
/// `TRAN-PROC-TS` of its interest records.
#[must_use]
pub fn db2_timestamp(t: DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02}-{:02}.{:02}.{:02}.{:02}0000",
        year4(t),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        subsec_nanos(t) / 10_000_000,
    )
}

/// COBIL00C `GET-CURRENT-TIMESTAMP` — P-COBIL00C-026.
///
/// CSDAT01Y `WS-TIMESTAMP X(26)` [app/cbl/COBIL00C.cbl:L249-267]
/// [app/cpy/CSDAT01Y.cpy:L42-56]: `YYYY-MM-DD HH:MM:SS.000000`. The date
/// (`FORMATTIME YYYYMMDD DATESEP('-')`) fills positions 1-10 and the time
/// (`TIME TIMESEP(':')`, whole seconds) positions 12-19. `INITIALIZE` leaves
/// the FILLER items alone, so position 11 keeps its `VALUE ' '` and position 20
/// its `VALUE '.'`; `WS-TIMESTAMP-TM-MS6` is zeroed. CB00 stamps both
/// `TRAN-ORIG-TS` and `TRAN-PROC-TS` of the payment with it. CT02 reads no
/// clock (P-COTRN02C-030: its timestamps are the entered dates padded with
/// spaces), so this formatter serves CB00 alone.
#[must_use]
pub fn cics_timestamp(t: DateTime<Utc>) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.000000",
        year4(t),
        t.month(),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
    )
}

/// CODATE01 `4000-PROCESS-REQUEST-REPLY` — P-CODATE01-020.
///
/// `WS-MMDDYYYY X(10)` from `FORMATTIME MMDDYYYY DATESEP('-')`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L343-360]: `mm-dd-yyyy`, the date of the
/// CDRD reply.
#[must_use]
pub fn mmddyyyy_dashed(t: DateTime<Utc>) -> String {
    format!("{:02}-{:02}-{:04}", t.month(), t.day(), year4(t))
}

/// CODATE01 `4000-PROCESS-REQUEST-REPLY` — P-CODATE01-021.
///
/// `WS-TIME X(8)` from `FORMATTIME TIME TIMESEP`, whose default separator is
/// `:` [app/app-vsam-mq/cbl/CODATE01.cbl:L343-360]: `hh:mm:ss` in whole
/// seconds, the time of the CDRD reply.
#[must_use]
pub fn hhmmss_colon(t: DateTime<Utc>) -> String {
    format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
}

/// COPAUA0C `8500-INSERT-AUTH` — P-COPAUA0C-030; CBPAUP0C `1000-INITIALIZE` —
/// P-CBPAUP0C-012.
///
/// `FORMATTIME YYDDD` moved into `WS-YYDDD PIC 9(05)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L861-868], and
/// `ACCEPT CURRENT-YYDDD FROM DAY`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L186-187]: the two-digit
/// year times 1000 plus the day of the year (1 to 366).
#[must_use]
pub fn yyddd(t: DateTime<Utc>) -> u32 {
    year2(t).unsigned_abs() * 1_000 + t.ordinal()
}

/// COPAUA0C `8500-INSERT-AUTH` — P-COPAUA0C-030.
///
/// `FORMATTIME TIME` moved into `WS-CUR-TIME-N6 PIC 9(06)`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L861-869]: `HHMMSS` as
/// a number, in whole seconds. The six-character text the error log carries is
/// [`hhmmss_compact`].
#[must_use]
pub fn hhmmss(t: DateTime<Utc>) -> u32 {
    t.hour() * 10_000 + t.minute() * 100 + t.second()
}

/// COPAUA0C `8500-INSERT-AUTH` — P-COPAUA0C-030.
///
/// `FORMATTIME MILLISECONDS` into `WS-CUR-TIME-MS`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L861-865]: the
/// milliseconds within the second (0 to 999, truncated).
#[must_use]
pub fn milliseconds(t: DateTime<Utc>) -> u32 {
    subsec_nanos(t) / 1_000_000
}

/// COPAUA0C `8500-INSERT-AUTH` — P-COPAUA0C-030.
///
/// The complemented key of a pending-authorization detail segment
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L857-875], returned as
/// `(PA-AUTH-DATE-9C, PA-AUTH-TIME-9C)`:
///
/// - `PA-AUTH-DATE-9C = 99999 - YYDDD`;
/// - `PA-AUTH-TIME-9C = 999999999 - (HHMMSS * 1000 + milliseconds)`.
///
/// The `(date, time)` key shrinks as time advances, so IMS key order lists the
/// newest detail first.
#[must_use]
pub fn auth_key_complement(t: DateTime<Utc>) -> (i32, i64) {
    // YYDDD is at most 99_366, so the difference lies in 633..=99_998 and
    // `cast_signed` keeps its value.
    let date = (99_999 - yyddd(t)).cast_signed();
    let time_with_ms = i64::from(hhmmss(t)) * 1_000 + i64::from(milliseconds(t));
    (date, 999_999_999 - time_with_ms)
}

/// COPAUA0C `9500-LOG-ERROR` — P-COPAUA0C-031; CBPAUP0C `1000-INITIALIZE` —
/// P-CBPAUP0C-012.
///
/// `ERR-DATE X(6)` from `FORMATTIME YYMMDD`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L986-999], and
/// `ACCEPT CURRENT-DATE FROM DATE`
/// [app/app-authorization-ims-db2-mq/cbl/CBPAUP0C.cbl:L186-187]: `yymmdd`.
#[must_use]
pub fn yymmdd(t: DateTime<Utc>) -> String {
    format!("{:02}{:02}{:02}", year2(t), t.month(), t.day())
}

/// COPAUA0C `9500-LOG-ERROR` — P-COPAUA0C-031.
///
/// `ERR-TIME X(6)` from `FORMATTIME TIME` with no separator
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L986-999]: `hhmmss` in
/// whole seconds.
#[must_use]
pub fn hhmmss_compact(t: DateTime<Utc>) -> String {
    format!("{:02}{:02}{:02}", t.hour(), t.minute(), t.second())
}

#[cfg(test)]
mod tests {
    use super::{
        AnyClock, Clock, ClockError, FixedClock, InstantError, SystemClock, auth_key_complement,
        cics_timestamp, db2_timestamp, hhmmss, hhmmss_colon, hhmmss_compact, is_whole_microsecond,
        milliseconds, mmddyyyy_dashed, parse_instant, yyddd, yymmdd,
    };
    use chrono::{DateTime, NaiveDate, SubsecRound, TimeDelta, Utc};

    /// The instant the TRANREPT parity run pins the processing clock to.
    const PINNED: &str = "2022-07-01T12:00:00Z";

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("test instant is RFC 3339")
            .with_timezone(&Utc)
    }

    #[test]
    fn from_override_without_a_value_selects_the_system_clock() {
        assert_eq!(
            AnyClock::from_override(None),
            Ok(AnyClock::System(SystemClock))
        );
    }

    #[test]
    fn from_override_with_an_instant_selects_a_fixed_clock_in_utc() {
        for value in [
            PINNED,
            " 2022-07-01T12:00:00Z\n",
            "2022-07-01T14:00:00+02:00",
            "2022-07-01T07:00:00-05:00",
            "2022-07-01T12:00:00.000000000Z",
            "2022-07-01T12:00:00.0000000000000Z",
        ] {
            let clock = AnyClock::from_override(Some(value)).expect("valid override");
            assert_eq!(clock, AnyClock::Fixed(FixedClock(at(PINNED))), "{value:?}");
            assert_eq!(clock.now(), at(PINNED), "{value:?}");
        }
    }

    #[test]
    fn from_override_rejects_text_that_is_not_rfc_3339() {
        for (value, kept) in [
            ("not-a-date", "not-a-date"),
            (" 2022-07-01 ", "2022-07-01"),
            ("2022-07-01T12:00:00", "2022-07-01T12:00:00"),
            ("", ""),
            ("   ", ""),
            ("\t\n", ""),
        ] {
            let error = AnyClock::from_override(Some(value)).expect_err("invalid override");
            assert_eq!(error, ClockError::InvalidOverride(kept.to_owned()));
            assert_eq!(
                error.to_string(),
                format!("CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: {kept}")
            );
        }
    }

    #[test]
    fn from_override_rejects_an_instant_a_timestamp_cannot_hold() {
        for (value, kept, reason) in [
            (
                "2022-07-01T12:00:00.000000900Z",
                "2022-07-01T12:00:00.000000900Z",
                InstantError::FinerThanMicrosecond,
            ),
            (
                "2022-07-01T12:00:00.0000000001Z",
                "2022-07-01T12:00:00.0000000001Z",
                InstantError::FinerThanMicrosecond,
            ),
            (
                " 2016-12-31T23:59:60Z\n",
                "2016-12-31T23:59:60Z",
                InstantError::LeapSecond,
            ),
        ] {
            let error = AnyClock::from_override(Some(value)).expect_err("an inexact override");
            assert_eq!(
                error,
                ClockError::InexactOverride {
                    value: kept.to_owned(),
                    reason,
                },
                "{value:?}"
            );
            assert_eq!(
                error.to_string(),
                format!(
                    "CARDDEMO_CLOCK_OVERRIDE is not an instant a timestamp holds exactly: \
                     {kept}: {reason}"
                )
            );
        }
    }

    /// 2022-07-01T12:00:00Z plus `micros` microseconds.
    fn pinned_plus(micros: i64) -> DateTime<Utc> {
        at(PINNED) + TimeDelta::microseconds(micros)
    }

    #[test]
    fn parse_instant_accepts_whole_microseconds_in_every_rfc_3339_form() {
        for (text, expected) in [
            (PINNED, pinned_plus(0)),
            ("2022-07-01T12:00:00.000001Z", pinned_plus(1)),
            ("2022-07-01T12:00:00.000001000Z", pinned_plus(1)),
            ("2022-07-01T12:00:00.000000000Z", pinned_plus(0)),
            ("2022-07-01T12:00:00.123456000000000Z", pinned_plus(123_456)),
            ("2022-07-01T12:00:00.5Z", pinned_plus(500_000)),
            ("2022-07-01T14:00:00.000001+02:00", pinned_plus(1)),
            ("2022-07-01T12:00:00-00:00", pinned_plus(0)),
            ("2022-07-01T07:00:00\u{2212}05:00", pinned_plus(0)),
            ("2022-07-01t12:00:00z", pinned_plus(0)),
            ("2022-07-01 12:00:00Z", pinned_plus(0)),
            (
                "2016-12-31T23:59:59.999999Z",
                at("2016-12-31T23:59:59.999999Z"),
            ),
        ] {
            let parsed = parse_instant(text).unwrap_or_else(|error| panic!("{text:?}: {error}"));
            assert_eq!(parsed, expected, "{text:?}");
            assert!(is_whole_microsecond(parsed), "{text:?}");
        }
    }

    #[test]
    fn parse_instant_refuses_digits_finer_than_a_microsecond_and_leap_seconds() {
        for (text, expected) in [
            (
                "2022-07-01T12:00:00.000000900Z",
                InstantError::FinerThanMicrosecond,
            ),
            (
                "2022-07-01T12:00:00.0000001Z",
                InstantError::FinerThanMicrosecond,
            ),
            (
                "2022-07-01T12:00:00.0000000001Z",
                InstantError::FinerThanMicrosecond,
            ),
            (
                "2022-07-01T12:00:00.000001000000000000001+02:00",
                InstantError::FinerThanMicrosecond,
            ),
            (
                "2016-12-31T23:59:60.0000001Z",
                InstantError::FinerThanMicrosecond,
            ),
            ("2016-12-31T23:59:60Z", InstantError::LeapSecond),
            ("2016-12-31T23:59:60.5Z", InstantError::LeapSecond),
            ("2017-01-01T01:59:60+02:00", InstantError::LeapSecond),
        ] {
            assert_eq!(parse_instant(text), Err(expected), "{text:?}");
        }
        assert_eq!(
            InstantError::FinerThanMicrosecond.to_string(),
            "value is finer than a microsecond: a fraction digit after the 6th is not 0"
        );
        assert_eq!(
            InstantError::LeapSecond.to_string(),
            "value is a leap second (second 60), which a timestamp cannot hold"
        );
    }

    #[test]
    fn parse_instant_refuses_text_that_is_not_rfc_3339() {
        for text in [
            "",
            "not-a-date",
            "2022-07-01",
            "2022-07-01T12:00:00",
            " 2022-07-01T12:00:00Z",
            "2022-07-01T12:00:00Z ",
            "2022-07-01T12:00:00.Z",
            "2022-07-01T24:00:00Z",
            "2022-07-01T12:60:00Z",
            "2022-07-01T12:00:61Z",
            "2022-02-30T00:00:00Z",
            "2022-07-01T12:00:00+24:00",
            "2022-07-01T12:00:00+0200",
            "2022-07-01T12:00:00.0000001",
        ] {
            let error = parse_instant(text).expect_err("not RFC 3339");
            assert!(matches!(error, InstantError::NotRfc3339(_)), "{text:?}");
            assert!(
                error
                    .to_string()
                    .starts_with("value is not an RFC 3339 date-time: "),
                "{text:?}: {error}"
            );
        }
    }

    #[test]
    fn whole_microseconds_are_the_instants_a_timestamp_holds() {
        let leap = NaiveDate::from_ymd_opt(2016, 12, 31)
            .and_then(|date| date.and_hms_nano_opt(23, 59, 59, 1_000_000_000))
            .expect("a leap second")
            .and_utc();
        for (t, expected) in [
            (pinned_plus(0), true),
            (pinned_plus(1), true),
            (pinned_plus(999_999), true),
            (pinned_plus(0) + TimeDelta::nanoseconds(1), false),
            (pinned_plus(1) + TimeDelta::nanoseconds(999), false),
            (leap, false),
            (leap + TimeDelta::microseconds(1), false),
        ] {
            assert_eq!(is_whole_microsecond(t), expected, "{t:?}");
        }
    }

    fn read<C: Clock>(clock: C) -> DateTime<Utc> {
        clock.now()
    }

    #[test]
    fn clocks_report_their_instant_through_values_references_and_trait_objects() {
        let fixed = FixedClock(at(PINNED));
        let any = AnyClock::Fixed(fixed);
        let borrowed: &FixedClock = &fixed;
        let nested: &&AnyClock = &&any;
        let object: &dyn Clock = &any;
        assert_eq!(read(fixed), at(PINNED));
        assert_eq!(read(any), at(PINNED));
        assert_eq!(read(borrowed), at(PINNED));
        assert_eq!(read(nested), at(PINNED));
        assert_eq!(read(object), at(PINNED));
    }

    #[test]
    fn the_system_clock_reads_the_host_clock_at_whole_microseconds() {
        // The reading is truncated, so it may lie below a `before` taken in
        // the same microsecond; `before` is truncated likewise.
        let before = Utc::now().trunc_subsecs(6);
        let read = AnyClock::System(SystemClock).now();
        let after = Utc::now();
        assert!(before <= read && read <= after, "{before} {read} {after}");
        assert!(is_whole_microsecond(read), "{read:?}");
    }

    #[test]
    fn db2_timestamp_keeps_hundredths_then_four_zeros() {
        for (instant, expected) in [
            (PINNED, "2022-07-01-12.00.00.000000"),
            ("2022-07-01T12:00:00.123Z", "2022-07-01-12.00.00.120000"),
            (
                "2022-07-01T12:00:00.999999999Z",
                "2022-07-01-12.00.00.990000",
            ),
            ("2022-01-02T03:04:05.06Z", "2022-01-02-03.04.05.060000"),
            ("2022-07-01T23:59:59.009Z", "2022-07-01-23.59.59.000000"),
        ] {
            assert_eq!(db2_timestamp(at(instant)), expected, "{instant}");
        }
    }

    #[test]
    fn cics_timestamp_keeps_the_filler_separators_and_zero_microseconds() {
        for (instant, expected) in [
            (PINNED, "2022-07-01 12:00:00.000000"),
            ("2022-07-01T12:34:56.789Z", "2022-07-01 12:34:56.000000"),
            ("2009-01-05T08:07:06Z", "2009-01-05 08:07:06.000000"),
        ] {
            let text = cics_timestamp(at(instant));
            assert_eq!(text, expected, "{instant}");
            assert_eq!(&text[10..11], " ", "position 11");
            assert_eq!(&text[19..20], ".", "position 20");
        }
    }

    #[test]
    fn codate01_date_and_time_use_their_separators() {
        for (instant, date, time) in [
            (PINNED, "07-01-2022", "12:00:00"),
            ("2009-01-05T08:07:06.999Z", "01-05-2009", "08:07:06"),
            ("2022-12-31T23:59:59Z", "12-31-2022", "23:59:59"),
        ] {
            assert_eq!(mmddyyyy_dashed(at(instant)), date, "{instant}");
            assert_eq!(hhmmss_colon(at(instant)), time, "{instant}");
        }
    }

    #[test]
    fn copaua0c_numeric_fields_follow_formattime() {
        for (instant, day, time, millis) in [
            (PINNED, 22_182, 120_000, 0),
            ("2022-12-31T23:59:59.999Z", 22_365, 235_959, 999),
            ("2024-12-31T09:05:07.123456Z", 24_366, 90_507, 123),
            ("2000-01-01T00:00:00Z", 1, 0, 0),
            ("2009-01-05T00:00:01.001Z", 9_005, 1, 1),
        ] {
            let t = at(instant);
            assert_eq!(yyddd(t), day, "{instant}");
            assert_eq!(hhmmss(t), time, "{instant}");
            assert_eq!(milliseconds(t), millis, "{instant}");
        }
    }

    #[test]
    fn auth_key_complement_orders_newer_details_first() {
        for (instant, expected) in [
            (PINNED, (77_817, 879_999_999)),
            ("2022-12-31T23:59:59.999Z", (77_634, 764_040_000)),
            ("2000-01-01T00:00:00Z", (99_998, 999_999_999)),
            ("2024-12-31T00:00:00.001Z", (75_633, 999_999_998)),
        ] {
            assert_eq!(auth_key_complement(at(instant)), expected, "{instant}");
        }
        let older = auth_key_complement(at("2022-07-01T12:00:00.000Z"));
        let newer = auth_key_complement(at("2022-07-01T12:00:00.001Z"));
        assert!(newer.1 < older.1);
        let next_day = auth_key_complement(at("2022-07-02T00:00:00Z"));
        assert!(next_day.0 < older.0);
    }

    #[test]
    fn error_log_date_and_time_are_six_digits() {
        for (instant, date, time) in [
            (PINNED, "220701", "120000"),
            ("2009-01-05T08:07:06.5Z", "090105", "080706"),
            ("2000-10-31T00:00:00Z", "001031", "000000"),
        ] {
            assert_eq!(yymmdd(at(instant)), date, "{instant}");
            assert_eq!(hhmmss_compact(at(instant)), time, "{instant}");
        }
    }

    type Formatter = fn(DateTime<Utc>) -> String;

    fn year_12345() -> DateTime<Utc> {
        NaiveDate::from_ymd_opt(12_345, 1, 2)
            .and_then(|date| date.and_hms_opt(3, 4, 5))
            .expect("valid date")
            .and_utc()
    }

    #[test]
    fn every_field_keeps_its_cobol_width_at_the_edges() {
        let formatters: [(Formatter, usize); 6] = [
            (db2_timestamp, 26),
            (cics_timestamp, 26),
            (mmddyyyy_dashed, 10),
            (hhmmss_colon, 8),
            (yymmdd, 6),
            (hhmmss_compact, 6),
        ];
        for t in [
            at(PINNED),
            at("0000-01-01T00:00:00Z"),
            at("9999-12-31T23:59:59.999999999Z"),
            at("2016-12-31T23:59:60.5Z"),
            year_12345(),
        ] {
            for (format, width) in formatters {
                let text = format(t);
                assert_eq!(text.len(), width, "{t}: {text}");
            }
            assert!((1..=99_366).contains(&yyddd(t)), "{t}");
            assert!(hhmmss(t) <= 235_959, "{t}");
            assert!(milliseconds(t) <= 999, "{t}");
        }
    }

    #[test]
    fn a_leap_second_stays_on_second_59() {
        let t = at("2016-12-31T23:59:60.5Z");
        assert_eq!(db2_timestamp(t), "2016-12-31-23.59.59.990000");
        assert_eq!(cics_timestamp(t), "2016-12-31 23:59:59.000000");
        assert_eq!(hhmmss(t), 235_959);
        assert_eq!(milliseconds(t), 999);
    }

    #[test]
    fn a_year_beyond_four_digits_keeps_its_low_order_digits() {
        let t = year_12345();
        assert_eq!(db2_timestamp(t), "2345-01-02-03.04.05.000000");
        assert_eq!(mmddyyyy_dashed(t), "01-02-2345");
        assert_eq!(yymmdd(t), "450102");
        assert_eq!(yyddd(t), 45_002);
    }
}
