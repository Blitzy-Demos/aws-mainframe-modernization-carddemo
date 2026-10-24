//! The injected clock and the COBOL renderings of its readings (AAP 0.5.6, "Time").
//!
//! Every date and time a rule writes comes from a [`Clock`]: [`SystemClock`] in the binaries and
//! [`FixedClock`] in tests, which compare time-dependent fields by format. Rules take the clock
//! as `&dyn Clock`, and services hold it as `Arc<dyn Clock>`; nothing else in `domain` reads
//! time.
//!
//! # Local time
//!
//! CICS `ASKTIME` and `FORMATTIME`, `FUNCTION CURRENT-DATE` and `ACCEPT … FROM DATE` and
//! `FROM DAY` all give the region's local date and time, so a reading is a naive local
//! [`NaiveDateTime`] and is never converted to UTC.
//!
//! # Precision
//!
//! A [`SystemClock`] reading carries whole milliseconds, the finest unit an audited writer uses
//! (`FORMATTIME MILLISECONDS`, COPAUA0C-098). Each rendering then keeps what its COBOL keeps:
//! hundredths for the Db2 timestamp, whole seconds everywhere else, milliseconds only in the
//! inverted authorization key. A [`FixedClock`] returns its instant as given, and no rendering
//! keeps more than milliseconds of it.
//!
//! # Renderings
//!
//! | Function | Result | COBOL field | Rules |
//! |---|---|---|---|
//! | [`db2_timestamp`] | layout `D`, `YYYY-MM-DD-HH.MM.SS.hh0000` | `DB2-FORMAT-TS` | CBACT04C-022, CBTRN02C-013, PIC-019 |
//! | [`iso_timestamp_zero_micros`] | layout `I`, `YYYY-MM-DD HH:MM:SS.000000` | `WS-TIMESTAMP` (`CSDAT01Y`) | COBIL00C-027, PIC-018 |
//! | [`header_date`], [`header_time`] | `MM/DD/YY`, `HH:MM:SS` | `WS-CURDATE-MM-DD-YY`, `WS-CURTIME-HH-MM-SS` | COSGN00C-012, COBIL00C-036 |
//! | [`mm_dd_yyyy_dash`], [`hh_mm_ss`] | `MM-DD-YYYY`, `HH:MM:SS` | `WS-MMDDYYYY`, `WS-TIME` | CODATE01-004, CODATE01-006, CODATE01-008 |
//! | [`yyyy_mm_dd_dash`] | `YYYY-MM-DD` | `WS-CUR-DATE-X10` | COBIL00C-027 |
//! | [`mm_dd_yy_slash`] | `MM/DD/YY` | `WS-CUR-DATE`, then `PA-FRAUD-RPT-DATE` | COPAUS2C-001 |
//! | [`yymmdd`], [`hhmmss`] | six digits each | `ERR-DATE`, `ERR-TIME`; `ACCEPT … FROM DATE` | COPAUA0C-110, CBPAUP0C-001 |
//! | [`yyddd`] | `YY·1000 + day of year` | `WS-YYDDD`, `CURRENT-YYDDD` | COPAUA0C-098, CBPAUP0C-001 |
//! | [`hhmmssmmm`] | `HHMMSS·1000 + milliseconds` | `WS-TIME-WITH-MS` | COPAUA0C-098 |
//! | [`inverted_auth_keys`] | `PA-AUTH-DATE-9C`, `PA-AUTH-TIME-9C` | `PAUT9CTS` key | COPAUA0C-098 |
//!
//! A timestamp a record stores leaves this module as a typed [`CobolTimestamp`] with its layout
//! code (Typed Dates and Timestamps, E33), and codec A writes its text. The text renderings exist
//! only for fields that are text in the COBOL: screen headers, the CDRD reply, the `CSSL` log
//! date and time, `PA-FRAUD-RPT-DATE` and the values CBPAUP0C accepts.
//!
//! # Totality
//!
//! Every function here returns a value for every [`NaiveDateTime`] and never panics, and each
//! text has the fixed width of its COBOL field. A reading inside a leap second, which `chrono`
//! gives as second 59 with a fraction of one second or more, is read as the last instant of
//! second 59: no COBOL clock shows a second 60. A year outside 0–9999, which neither
//! [`SystemClock`] nor a COBOL clock produces, keeps the low-order four digits of its magnitude,
//! or two in the two-digit forms, as an unsigned `PIC 9(04)` or `PIC 9(02)` receiver keeps the
//! low-order digits of the absolute value a `MOVE` gives it: year −1 reads as `0001` and `01`, and
//! year 12 024 as `2024` and `24`.

use chrono::{Datelike, Local, NaiveDateTime, Timelike};
use copybook::{CobolTimestamp, PicS, TsForm};

/// Nanoseconds in a millisecond.
const NANOS_PER_MILLI: u32 = 1_000_000;

/// Nanoseconds in a hundredth of a second, the `COB-MIL` unit of `FUNCTION CURRENT-DATE`.
const NANOS_PER_HUNDREDTH: u32 = 10_000_000;

/// The last nanosecond of a second; a leap-second fraction is read as this.
const LAST_NANO_OF_SECOND: u32 = 999_999_999;

/// The modulus that keeps the low-order four digits of a year.
const FOUR_DIGIT_YEARS: u32 = 10_000;

/// The modulus that keeps the low-order two digits of a year.
const TWO_DIGIT_YEARS: u32 = 100;

/// The source of the current local date and time.
///
/// Object-safe, so a service holds `Arc<dyn Clock>` and a rule takes `&dyn Clock`. A rule reads
/// it once per value it writes, as the COBOL performs its `ASKTIME` or `CURRENT-DATE` once per
/// record, reply or log row (CBACT04C-022, CBTRN02C-013, CODATE01-008, COPAUA0C-098,
/// COPAUA0C-110).
pub trait Clock: Send + Sync {
    /// The current local date and time.
    #[must_use]
    fn now(&self) -> NaiveDateTime;
}

/// The operating-system clock: local time, cut to whole milliseconds.
///
/// This is the one place in `domain` that reads a real clock. It stands in for CICS `ASKTIME`
/// and `FORMATTIME` and for `FUNCTION CURRENT-DATE`, which give local time, and for
/// `FORMATTIME MILLISECONDS`, whose unit is the millisecond (COPAUA0C-098).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> NaiveDateTime {
        let now = Local::now().naive_local();
        let nanos = now.nanosecond();
        // `with_nanosecond` refuses only a value of two seconds or more, which a value cut down
        // from a reading's own fraction never is; the fallback keeps the reading uncut.
        now.with_nanosecond(nanos - nanos % NANOS_PER_MILLI)
            .unwrap_or(now)
    }
}

/// A clock stopped at one instant, for tests: every reading returns it unchanged, so two
/// requests decided under one instant get the same inverted authorization key (COPAUA0C-098).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedClock(NaiveDateTime);

impl FixedClock {
    /// A clock that always reads `at`.
    #[must_use]
    pub const fn new(at: NaiveDateTime) -> Self {
        Self(at)
    }
}

impl Clock for FixedClock {
    fn now(&self) -> NaiveDateTime {
        self.0
    }
}

/// The nanoseconds of `at` within its second, a leap-second fraction read as the last one of
/// second 59.
fn nanos_in_second(at: NaiveDateTime) -> u32 {
    at.nanosecond().min(LAST_NANO_OF_SECOND)
}

/// The milliseconds of `at` within its second, 0–999.
fn millis(at: NaiveDateTime) -> u32 {
    nanos_in_second(at) / NANOS_PER_MILLI
}

/// The hundredths of `at` within its second, 0–99.
fn hundredths(at: NaiveDateTime) -> u32 {
    nanos_in_second(at) / NANOS_PER_HUNDREDTH
}

/// The low-order digits of the year of `at` that `modulus` keeps, taken from the year's
/// magnitude, so a negative year keeps the digits of its absolute value.
fn year_digits(at: NaiveDateTime, modulus: u32) -> u32 {
    // TRUNCATION: a COBOL year field of four or two digits is unsigned and keeps the low-order
    // digits of the year it receives; `unsigned_abs` drops the sign as the `MOVE` does, and the
    // remainder drops the high-order digits.
    at.year().unsigned_abs() % modulus
}

/// `at` as the timestamp a COBOL writer stores in layout `form`, its year cut to four digits and
/// its fraction of a second replaced by `nanos`.
fn cobol_timestamp(at: NaiveDateTime, nanos: u32, form: TsForm) -> CobolTimestamp {
    // `CobolTimestamp::new` refuses a year outside 0–9999, a fraction finer than a microsecond, a
    // leap second and a layout-`S` time other than midnight. Callers pass a fraction of whole
    // hundredths or zero, and the year is cut to four digits. Whether a year is a leap year
    // depends only on whether 4, 100 and 400 divide it, which neither dropping the sign nor
    // removing a multiple of 10 000 changes, so `with_year` keeps every date: 29 February of year
    // −4 becomes 29 February of 0004. The fallback is therefore never taken; it rebuilds from
    // `NaiveDateTime::default()`, midnight of 1 January 1970, which passes every check in every
    // layout, so it ends after one call.
    // TRUNCATION: the four-digit year is at most 9 999, far inside `i32`, so the checked
    // conversion is exact; its error arm only joins the fallback.
    i32::try_from(year_digits(at, FOUR_DIGIT_YEARS))
        .ok()
        .and_then(|year| at.with_year(year))
        .and_then(|value| value.with_nanosecond(nanos))
        .and_then(|value| CobolTimestamp::new(value, form).ok())
        .unwrap_or_else(|| cobol_timestamp(NaiveDateTime::default(), 0, form))
}

/// `DB2-FORMAT-TS`, the timestamp `Z-GET-DB2-FORMAT-TIMESTAMP` builds from
/// `FUNCTION CURRENT-DATE`: layout `D`, with the fraction cut to the hundredths of `COB-MIL`
/// followed by `DB2-REST` `0000`, so codec A writes `YYYY-MM-DD-HH.MM.SS.hh0000`
/// (`CBACT04C.cbl:613-626`, `CBTRN02C.cbl:692-705`; CBACT04C-022, CBTRN02C-013, PIC-019).
#[must_use]
pub fn db2_timestamp(at: NaiveDateTime) -> CobolTimestamp {
    cobol_timestamp(at, hundredths(at) * NANOS_PER_HUNDREDTH, TsForm::Db2)
}

/// `WS-TIMESTAMP` of `CSDAT01Y` as COBIL00C's `GET-CURRENT-TIMESTAMP` fills it: `FORMATTIME`'s
/// date and whole-second time over the `VALUE`d separators, which `INITIALIZE` keeps, and six
/// zero fraction digits. Layout `I`, so codec A writes `YYYY-MM-DD HH:MM:SS.000000`
/// (`COBIL00C.cbl:249-266`; COBIL00C-027, PIC-018).
#[must_use]
pub fn iso_timestamp_zero_micros(at: NaiveDateTime) -> CobolTimestamp {
    cobol_timestamp(at, 0, TsForm::Iso)
}

/// `MM/DD/YY` with `/` between the parts.
fn slash_date(at: NaiveDateTime) -> String {
    format!(
        "{:02}/{:02}/{:02}",
        at.month(),
        at.day(),
        year_digits(at, TWO_DIGIT_YEARS)
    )
}

/// `HH:MM:SS` on the 24-hour clock, the fraction of a second dropped.
fn colon_time(at: NaiveDateTime) -> String {
    format!("{:02}:{:02}:{:02}", at.hour(), at.minute(), at.second())
}

/// The screen header date, `WS-CURDATE-MM-DD-YY` of `CSDAT01Y`: `MM/DD/YY`, the year's last two
/// digits (`COSGN00C.cbl:190`; COSGN00C-012, COBIL00C-036).
#[must_use]
pub fn header_date(at: NaiveDateTime) -> String {
    slash_date(at)
}

/// The screen header time, `WS-CURTIME-HH-MM-SS` of `CSDAT01Y`: `HH:MM:SS`
/// (`COSGN00C.cbl:196`; COSGN00C-012, COBIL00C-036).
#[must_use]
pub fn header_time(at: NaiveDateTime) -> String {
    colon_time(at)
}

/// The CDRD reply date, `FORMATTIME MMDDYYYY DATESEP('-')` into `WS-MMDDYYYY PIC X(10)`:
/// `MM-DD-YYYY` (`CODATE01.cbl:347-350,355`; CODATE01-004).
#[must_use]
pub fn mm_dd_yyyy_dash(at: NaiveDateTime) -> String {
    format!(
        "{:02}-{:02}-{:04}",
        at.month(),
        at.day(),
        year_digits(at, FOUR_DIGIT_YEARS)
    )
}

/// The CDRD reply time, `FORMATTIME TIME TIMESEP` into `WS-TIME PIC X(8)`: `HH:MM:SS`, the
/// `TIMESEP` default separator `:`, whole seconds (`CODATE01.cbl:347-353,356`; CODATE01-006).
/// The date and time of one reply come from one reading (CODATE01-008).
#[must_use]
pub fn hh_mm_ss(at: NaiveDateTime) -> String {
    colon_time(at)
}

/// `FORMATTIME YYYYMMDD DATESEP('-')` into `WS-CUR-DATE-X10`: `YYYY-MM-DD`
/// (`COBIL00C.cbl:255-261`; COBIL00C-027).
#[must_use]
pub fn yyyy_mm_dd_dash(at: NaiveDateTime) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        year_digits(at, FOUR_DIGIT_YEARS),
        at.month(),
        at.day()
    )
}

/// `FORMATTIME MMDDYY DATESEP` with no separator value, so `/`, into `WS-CUR-DATE PIC X(08)`,
/// which COPAUS2C moves to `PA-FRAUD-RPT-DATE`: `MM/DD/YY` (`COPAUS2C.cbl:91-101`;
/// COPAUS2C-001).
#[must_use]
pub fn mm_dd_yy_slash(at: NaiveDateTime) -> String {
    slash_date(at)
}

/// `FORMATTIME YYMMDD` into `ERR-DATE X(06)` (`COPAUA0C.cbl:986-998`; COPAUA0C-110), and the
/// `YYMMDD` of CBPAUP0C's `ACCEPT CURRENT-DATE FROM DATE` (`CBPAUP0C.cbl:186`; CBPAUP0C-001):
/// six digits, no separators.
#[must_use]
pub fn yymmdd(at: NaiveDateTime) -> String {
    format!(
        "{:02}{:02}{:02}",
        year_digits(at, TWO_DIGIT_YEARS),
        at.month(),
        at.day()
    )
}

/// `FORMATTIME TIME` with no `TIMESEP` into `ERR-TIME X(06)`: `HHMMSS`, six digits
/// (`COPAUA0C.cbl:986-999`; COPAUA0C-110).
#[must_use]
pub fn hhmmss(at: NaiveDateTime) -> String {
    format!("{:02}{:02}{:02}", at.hour(), at.minute(), at.second())
}

/// `YYDDD`: the year's last two digits times 1000 plus the day of the year, 1–366, so at most
/// 99 366. COPAUA0C keeps the first five characters of `FORMATTIME YYDDD` in `WS-YYDDD PIC 9(05)`
/// (`COPAUA0C.cbl:863,868`), and CBPAUP0C accepts `CURRENT-YYDDD PIC 9(05)` `FROM DAY`
/// (`CBPAUP0C.cbl:187`) (COPAUA0C-098, CBPAUP0C-001).
#[must_use]
pub fn yyddd(at: NaiveDateTime) -> u32 {
    year_digits(at, TWO_DIGIT_YEARS) * 1_000 + at.ordinal()
}

/// `WS-TIME-WITH-MS = (WS-CUR-TIME-N6 * 1000) + WS-CUR-TIME-MS`: the 24-hour `HHMMSS` time times
/// 1000 plus the milliseconds 0–999, so at most 235 959 999 (`COPAUA0C.cbl:864-872`;
/// COPAUA0C-098).
#[must_use]
pub fn hhmmssmmm(at: NaiveDateTime) -> u32 {
    let hhmmss = at.hour() * 10_000 + at.minute() * 100 + at.second();
    hhmmss * 1_000 + millis(at)
}

/// The inverted key of a pending-authorization detail, `PAUT9CTS`:
/// `PA-AUTH-DATE-9C PIC S9(05) = 99999 − YYDDD` and
/// `PA-AUTH-TIME-9C PIC S9(09) = 999999999 − HHMMSSmmm` (`COPAUA0C.cbl:874-875`,
/// `CIPAUDTY.cpy:20-21`; COPAUA0C-098). Newer authorizations of one century get lower keys.
#[must_use]
pub fn inverted_auth_keys(at: NaiveDateTime) -> (PicS<5>, PicS<9>) {
    // `complement_from` refuses only a value above the key's nines; `yyddd` is at most 99 366 and
    // `hhmmssmmm` at most 235 959 999, so both always succeed and the zero fallback is never
    // taken.
    let date = PicS::<5>::complement_from(u64::from(yyddd(at))).unwrap_or_default();
    let time = PicS::<9>::complement_from(u64::from(hhmmssmmm(at))).unwrap_or_default();
    (date, time)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{NaiveDate, NaiveDateTime, Timelike};
    use copybook::{CobolTimestamp, PicTimestamp, TsForm};

    use super::{
        Clock, FixedClock, SystemClock, db2_timestamp, header_date, header_time, hh_mm_ss, hhmmss,
        hhmmssmmm, inverted_auth_keys, iso_timestamp_zero_micros, mm_dd_yy_slash, mm_dd_yyyy_dash,
        yyddd, yymmdd, yyyy_mm_dd_dash,
    };

    type TestResult = Result<(), &'static str>;

    /// The instant `date` at `time`, given as hour, minute, second and millisecond; a
    /// millisecond of 1000 or more is a leap second.
    fn instant(
        date: (i32, u32, u32),
        time: (u32, u32, u32, u32),
    ) -> Result<NaiveDateTime, &'static str> {
        let (year, month, day) = date;
        let (hour, minute, second, milli) = time;
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|day| day.and_hms_milli_opt(hour, minute, second, milli))
            .ok_or("no such instant")
    }

    /// The reference instant of the brief: 2022-07-18 13:45:07.123.
    fn reference() -> Result<NaiveDateTime, &'static str> {
        instant((2022, 7, 18), (13, 45, 7, 123))
    }

    /// The 26 bytes codec A writes for `stamp`.
    fn encoded(stamp: CobolTimestamp) -> Vec<u8> {
        let mut out = Vec::new();
        PicTimestamp::new(Some(stamp)).encode_into(&mut out);
        out
    }

    #[test]
    fn fixed_clock_returns_its_instant_on_every_reading() -> TestResult {
        let at = reference()?;
        let clock = FixedClock::new(at);
        assert_eq!(clock.now(), at);
        assert_eq!(clock.now(), clock.now());
        let shared: Arc<dyn Clock> = Arc::new(clock);
        assert_eq!(shared.now(), at);
        Ok(())
    }

    #[test]
    fn system_clock_reads_whole_milliseconds() {
        let clock: &dyn Clock = &SystemClock;
        assert_eq!(clock.now().nanosecond() % 1_000_000, 0);
    }

    // audit: CBACT04C-022, CBTRN02C-013, PIC-019
    #[test]
    fn db2_timestamp_keeps_hundredths_in_layout_d() -> TestResult {
        let stamp = db2_timestamp(reference()?);
        assert_eq!(stamp.form(), TsForm::Db2);
        assert_eq!(stamp.value(), instant((2022, 7, 18), (13, 45, 7, 120))?);
        assert_eq!(encoded(stamp), b"2022-07-18-13.45.07.120000");
        // The fixture instants of POSTED and INTEREST-RAW.
        let posted = db2_timestamp(instant((2022, 7, 1), (12, 0, 0, 0))?);
        assert_eq!(encoded(posted), b"2022-07-01-12.00.00.000000");
        let interest = db2_timestamp(instant((2022, 7, 18), (0, 0, 0, 0))?);
        assert_eq!(encoded(interest), b"2022-07-18-00.00.00.000000");
        Ok(())
    }

    // audit: COBIL00C-027, PIC-018
    #[test]
    fn iso_timestamp_drops_the_fraction_in_layout_i() -> TestResult {
        let stamp = iso_timestamp_zero_micros(reference()?);
        assert_eq!(stamp.form(), TsForm::Iso);
        assert_eq!(stamp.value(), instant((2022, 7, 18), (13, 45, 7, 0))?);
        assert_eq!(stamp.value().nanosecond(), 0);
        assert_eq!(encoded(stamp), b"2022-07-18 13:45:07.000000");
        Ok(())
    }

    // audit: COSGN00C-012, COBIL00C-036, CODATE01-004, CODATE01-006, COBIL00C-027
    // audit: COPAUS2C-001, COPAUA0C-110, CBPAUP0C-001
    #[test]
    fn text_renderings_have_the_cobol_layouts() -> TestResult {
        // Each expected line holds, in order: header date and time, CDRD date and time,
        // COBIL00C date, COPAUS2C date, CSSL date and time.
        let cases = [
            (
                reference()?,
                "07/18/22 13:45:07 07-18-2022 13:45:07 2022-07-18 07/18/22 220718 134507",
            ),
            (
                instant((2000, 1, 2), (3, 4, 5, 6))?,
                "01/02/00 03:04:05 01-02-2000 03:04:05 2000-01-02 01/02/00 000102 030405",
            ),
            (
                instant((1999, 12, 31), (23, 59, 59, 999))?,
                "12/31/99 23:59:59 12-31-1999 23:59:59 1999-12-31 12/31/99 991231 235959",
            ),
        ];
        for (at, expected) in cases {
            let rendered = [
                header_date(at),
                header_time(at),
                mm_dd_yyyy_dash(at),
                hh_mm_ss(at),
                yyyy_mm_dd_dash(at),
                mm_dd_yy_slash(at),
                yymmdd(at),
                hhmmss(at),
            ];
            assert_eq!(rendered.join(" "), expected, "{at}");
        }
        Ok(())
    }

    // audit: COPAUA0C-098, CBPAUP0C-001
    #[test]
    fn authorization_numbers_and_inverted_keys() -> TestResult {
        // (instant, YYDDD, HHMMSSmmm, PA-AUTH-DATE-9C, PA-AUTH-TIME-9C)
        let cases = [
            (reference()?, 22_199, 134_507_123, 77_800, 865_492_876),
            // The worked row of the COPAUA0C audit: day 32 of 2025.
            (
                instant((2025, 2, 1), (13, 45, 7, 123))?,
                25_032,
                134_507_123,
                74_967,
                865_492_876,
            ),
            (
                instant((2024, 12, 31), (0, 0, 0, 0))?,
                24_366,
                0,
                75_633,
                999_999_999,
            ),
            (
                instant((2025, 1, 1), (23, 59, 59, 999))?,
                25_001,
                235_959_999,
                74_998,
                764_040_000,
            ),
            (
                instant((2000, 1, 2), (3, 4, 5, 6))?,
                2,
                30_405_006,
                99_997,
                969_594_993,
            ),
        ];
        for (at, date, time, date_key, time_key) in cases {
            assert_eq!(yyddd(at), date, "{at}");
            assert_eq!(hhmmssmmm(at), time, "{at}");
            let (date_9c, time_9c) = inverted_auth_keys(at);
            assert_eq!((date_9c.get(), time_9c.get()), (date_key, time_key), "{at}");
            assert_eq!(date_9c.uncomplement(), Ok(u64::from(date)));
            assert_eq!(time_9c.uncomplement(), Ok(u64::from(time)));
        }
        Ok(())
    }

    #[test]
    fn one_instant_gives_one_key() -> TestResult {
        let clock = FixedClock::new(reference()?);
        assert_eq!(
            inverted_auth_keys(clock.now()),
            inverted_auth_keys(clock.now())
        );
        Ok(())
    }

    #[test]
    fn a_leap_second_reads_as_the_end_of_second_59() -> TestResult {
        let leap = instant((2016, 12, 31), (23, 59, 59, 1_500))?;
        assert_eq!(hhmmssmmm(leap), 235_959_999);
        assert_eq!(hh_mm_ss(leap), "23:59:59");
        assert_eq!(inverted_auth_keys(leap).1.get(), 764_040_000);
        assert_eq!(encoded(db2_timestamp(leap)), b"2016-12-31-23.59.59.990000");
        assert_eq!(
            encoded(iso_timestamp_zero_micros(leap)),
            b"2016-12-31 23:59:59.000000"
        );
        Ok(())
    }

    #[test]
    fn a_year_beyond_four_digits_keeps_its_low_order_digits() -> TestResult {
        // 12024 keeps 2024. A negative year keeps the low-order digits of its magnitude, as an
        // unsigned receiver does, so -1 and -10001 keep 0001 and -4 keeps 0004. 29 February
        // survives every cut.
        let late = instant((12_024, 2, 29), (8, 9, 10, 0))?;
        assert_eq!(encoded(db2_timestamp(late)), b"2024-02-29-08.09.10.000000");
        assert_eq!(mm_dd_yyyy_dash(late), "02-29-2024");
        assert_eq!(header_date(late), "02/29/24");
        assert_eq!(yyddd(late), 24_060);
        // (date, Db2 timestamp, layout-I timestamp, then the header, CDRD, COBIL00C and CSSL
        // dates, YYDDD)
        let cases = [
            (
                (-1, 12, 31),
                "0001-12-31-00.00.00.000000",
                "0001-12-31 00:00:00.000000",
                "12/31/01 12-31-0001 0001-12-31 011231",
                1_365,
            ),
            (
                (-10_001, 12, 31),
                "0001-12-31-00.00.00.000000",
                "0001-12-31 00:00:00.000000",
                "12/31/01 12-31-0001 0001-12-31 011231",
                1_365,
            ),
            (
                (-4, 2, 29),
                "0004-02-29-00.00.00.000000",
                "0004-02-29 00:00:00.000000",
                "02/29/04 02-29-0004 0004-02-29 040229",
                4_060,
            ),
        ];
        for (date, db2, iso, dates, date_number) in cases {
            let early = instant(date, (0, 0, 0, 0))?;
            assert_eq!(encoded(db2_timestamp(early)), db2.as_bytes(), "{early}");
            assert_eq!(
                encoded(iso_timestamp_zero_micros(early)),
                iso.as_bytes(),
                "{early}"
            );
            let rendered = [
                header_date(early),
                mm_dd_yyyy_dash(early),
                yyyy_mm_dd_dash(early),
                yymmdd(early),
            ];
            assert_eq!(rendered.join(" "), dates, "{early}");
            assert_eq!(yyddd(early), date_number, "{early}");
        }
        Ok(())
    }
}
