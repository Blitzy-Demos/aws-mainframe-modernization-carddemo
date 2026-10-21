//! The `message_id` of a CP00 reply (COPAUA0C-067).
//!
//! COPAUA0C moves `MQMI-NONE` to `MQMD-MSGID` of the reply (`COPAUA0C.cbl:745-746`), so the
//! queue manager gives every reply a new message ID and none is the request's. `RabbitMQ`
//! assigns no message ID, so the CP00 worker sets the one [`next_message_id`] generates. CDRD and
//! CDRA do not use this module: their replies copy the request's `message_id` and
//! `correlation_id`, as CODATE01 and COACCT01 do.
//!
//! # Format
//!
//! `<timestamp>-<process>-<sequence>`, ASCII only:
//!
//! - `<timestamp>`: the injected [`Clock`]'s reading as `YYYYMMDDHHMMSSmmm`, 17 digits, cut to
//!   the millisecond;
//! - `<process>`: the operating-system process ID in decimal, at most 10 digits;
//! - `<sequence>`: the next value of a process-wide 64-bit counter in decimal, at most 20 digits.
//!   It starts at 0 and wraps back to 0 after its largest value.
//!
//! An ID is therefore at most 49 bytes. A reading in a year outside 0–9999, which
//! [`SystemClock`](domain::SystemClock) never gives, adds a sign and at most two year digits, so
//! no ID exceeds 52 bytes. Both bounds sit far inside the 255 bytes of an AMQP short string,
//! the type of the `message_id` property, so the ID is never checked or cut at run time.
//!
//! Nothing in the ID comes from the request, so it carries no card number and no byte of the
//! body (Cardholder Data Redaction).
//!
//! # Uniqueness
//!
//! - Within one process, every call draws a different counter value, because the counter is a
//!   single atomic whose read-modify-write never hands one value out twice, whichever worker
//!   thread calls. Two calls under one clock instant, as under a
//!   [`FixedClock`](domain::FixedClock), therefore still differ. Only after 2^64 IDs does the
//!   counter wrap and a value recur, under a clock reading that has long since moved on.
//! - Two processes running at once, such as the two `carddemo-mq` processes of the i10 test,
//!   have different process IDs.
//! - A process ID the operating system reuses belongs to a process started after the earlier one
//!   ended, so its IDs repeat the earlier ones only if its clock reads one of the earlier
//!   process's milliseconds again.

use std::sync::atomic::{AtomicU64, Ordering};

use domain::Clock;

/// The sequence part of the next ID: the number of IDs this process has generated, modulo 2^64.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A new `message_id` for one CP00 reply (COPAUA0C-067): `clock`'s reading to the millisecond,
/// this process's ID and the next counter value, joined by `-`, in the format the module
/// documentation states.
///
/// Every call in one process returns a different ID, also under a clock that does not move.
#[must_use]
pub fn next_message_id(clock: &dyn Clock) -> String {
    let timestamp = clock.now().format("%Y%m%d%H%M%S%3f");
    let process = std::process::id();
    // Relaxed suffices: each `fetch_add` returns a value no other call receives, whatever the
    // ordering, and no other memory is published through the counter. It wraps at its largest
    // value instead of panicking.
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{timestamp}-{process}-{sequence}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;

    use chrono::{NaiveDate, NaiveDateTime};
    use domain::FixedClock;

    use super::next_message_id;

    /// The longest ID for a reading in a year 0–9999.
    const MAX_LEN_FOUR_DIGIT_YEAR: usize = 49;
    /// The longest ID for any reading.
    const MAX_LEN_ANY_YEAR: usize = 52;
    /// The longest value of an AMQP short string, the type of the `message_id` property.
    const AMQP_SHORT_STRING_MAX: usize = 255;

    type TestResult = Result<(), Box<dyn Error>>;

    /// The timestamp, process and sequence parts of `id`, split from the right, so that the sign
    /// of a year outside 0–9999 stays in the timestamp.
    fn parts(id: &str) -> Result<(&str, &str, &str), String> {
        let mut parts = id.rsplitn(3, '-');
        match (parts.next(), parts.next(), parts.next()) {
            (Some(sequence), Some(process), Some(timestamp)) => Ok((timestamp, process, sequence)),
            _ => Err(format!("{id:?} has fewer than three parts")),
        }
    }

    /// 2022-07-18 at `10:20:30` plus `micros` microseconds.
    fn instant(micros: u32) -> Result<NaiveDateTime, String> {
        NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_micro_opt(10, 20, 30, micros))
            .ok_or_else(|| format!("no instant with {micros} microseconds"))
    }

    // audit: COPAUA0C-067
    #[test]
    fn id_is_the_reading_to_the_millisecond_then_the_process_then_the_sequence() -> TestResult {
        let process = std::process::id().to_string();
        for micros in [456_000, 456_999] {
            let id = next_message_id(&FixedClock::new(instant(micros)?));
            let (timestamp, id_process, sequence) = parts(&id)?;
            assert!(id.starts_with("20220718102030456-"), "{id}");
            assert_eq!(timestamp, "20220718102030456");
            assert_eq!(id_process, process);
            sequence.parse::<u64>()?;
            assert!(id.is_ascii(), "{id}");
            assert!(id.len() <= MAX_LEN_FOUR_DIGIT_YEAR, "{id}");
        }
        Ok(())
    }

    // audit: COPAUA0C-067
    #[test]
    fn ids_under_one_instant_differ_and_their_sequence_rises() -> TestResult {
        let clock = FixedClock::new(instant(456_000)?);
        let first = next_message_id(&clock);
        let second = next_message_id(&clock);
        assert_ne!(first, second);
        let (first_timestamp, first_process, first_sequence) = parts(&first)?;
        let (second_timestamp, second_process, second_sequence) = parts(&second)?;
        assert_eq!(first_timestamp, second_timestamp);
        assert_eq!(first_process, second_process);
        assert!(second_sequence.parse::<u64>()? > first_sequence.parse::<u64>()?);
        Ok(())
    }

    // audit: COPAUA0C-067
    #[test]
    fn ids_drawn_on_several_threads_at_once_are_distinct() -> TestResult {
        const THREADS: usize = 4;
        const PER_THREAD: usize = 1_000;
        let clock = FixedClock::new(instant(456_000)?);
        let batches = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    scope.spawn(|| {
                        (0..PER_THREAD)
                            .map(|_| next_message_id(&clock))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().map_err(|_| "an ID thread panicked"))
                .collect::<Result<Vec<_>, _>>()
        })?;
        let distinct: BTreeSet<String> = batches.into_iter().flatten().collect();
        assert_eq!(distinct.len(), THREADS * PER_THREAD);
        Ok(())
    }

    // audit: COPAUA0C-067
    #[test]
    fn ids_stay_far_inside_an_amqp_short_string_for_any_reading() -> TestResult {
        for at in [NaiveDateTime::MIN, NaiveDateTime::MAX] {
            let id = next_message_id(&FixedClock::new(at));
            let (_, process, sequence) = parts(&id)?;
            assert_eq!(process, std::process::id().to_string());
            sequence.parse::<u64>()?;
            assert!(id.is_ascii(), "{id}");
            assert!(id.len() <= MAX_LEN_ANY_YEAR, "{id}");
            assert!(id.len() <= AMQP_SHORT_STRING_MAX, "{id}");
        }
        Ok(())
    }
}
