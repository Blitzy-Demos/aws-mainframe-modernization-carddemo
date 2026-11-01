//! Processing clock and the COBOL renderings of clock-dependent fields.
//!
//! Every time-dependent output of the port reads its instant from a [`Clock`]:
//! processing timestamps, the CDRD date and time, the CP00 detail keys and
//! error-log stamps, reward accrual and soft-delete instants. Adapters build an
//! [`AnyClock`] from the `CARDDEMO_CLOCK_OVERRIDE` setting with
//! [`AnyClock::from_override`]; this module reads no environment itself.
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

use chrono::{DateTime, Datelike, Timelike, Utc};

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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
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
    /// An absent value, or one that is empty after trimming, selects the
    /// [`SystemClock`]. Any other value is parsed as an RFC 3339 instant,
    /// converted to UTC, and selects a [`FixedClock`].
    ///
    /// # Errors
    ///
    /// [`ClockError::InvalidOverride`] when the trimmed value is not an
    /// RFC 3339 instant.
    pub fn from_override(value: Option<&str>) -> Result<Self, ClockError> {
        match value.map(str::trim) {
            None | Some("") => Ok(Self::System(SystemClock)),
            Some(text) => DateTime::parse_from_rfc3339(text)
                .map(|instant| Self::Fixed(FixedClock(instant.with_timezone(&Utc))))
                .map_err(|_| ClockError::InvalidOverride(text.to_owned())),
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
        AnyClock, Clock, ClockError, FixedClock, SystemClock, auth_key_complement, cics_timestamp,
        db2_timestamp, hhmmss, hhmmss_colon, hhmmss_compact, milliseconds, mmddyyyy_dashed, yyddd,
        yymmdd,
    };
    use chrono::{DateTime, NaiveDate, Utc};

    /// The instant the TRANREPT parity run pins the processing clock to.
    const PINNED: &str = "2022-07-01T12:00:00Z";

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("test instant is RFC 3339")
            .with_timezone(&Utc)
    }

    #[test]
    fn from_override_without_a_value_selects_the_system_clock() {
        for value in [None, Some(""), Some("   "), Some("\t\n")] {
            assert_eq!(
                AnyClock::from_override(value),
                Ok(AnyClock::System(SystemClock)),
                "{value:?}"
            );
        }
    }

    #[test]
    fn from_override_with_an_instant_selects_a_fixed_clock_in_utc() {
        for value in [
            PINNED,
            " 2022-07-01T12:00:00Z\n",
            "2022-07-01T14:00:00+02:00",
            "2022-07-01T07:00:00-05:00",
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
        ] {
            let error = AnyClock::from_override(Some(value)).expect_err("invalid override");
            assert_eq!(error, ClockError::InvalidOverride(kept.to_owned()));
            assert_eq!(
                error.to_string(),
                format!("CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: {kept}")
            );
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
    fn the_system_clock_reads_the_host_clock() {
        let before = Utc::now();
        let read = AnyClock::System(SystemClock).now();
        let after = Utc::now();
        assert!(before <= read && read <= after);
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
