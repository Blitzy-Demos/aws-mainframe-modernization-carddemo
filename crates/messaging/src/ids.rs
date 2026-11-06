//! Message identifiers for CP00 replies.
//!
//! `COPAUA0C` puts its authorization reply with
//! `MOVE MQMI-NONE TO MQMD-MSGID OF MQM-MD-REPLY`
//! [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L746], so IBM MQ assigns
//! the reply a new 24-byte `MsgId`. An AMQP broker assigns no `message_id`, so
//! the CP00 flow takes one from [`new_message_id`]: 24 bytes written as 48
//! lowercase hexadecimal characters. The reply's `correlation_id` still echoes the
//! request's; only the message ID is new.
//!
//! CDRD and CDRA do not use this module. `CODATE01` and `COACCT01` copy the
//! request `MsgId` into the reply, and their flows echo the request `message_id`.
//!
//! The ID is unique, not secret, and nothing parses it. Its layout:
//!
//! | Characters | Content |
//! |---|---|
//! | 1–16 | Wall-clock nanoseconds since the Unix epoch (`u64`, wrapping) |
//! | 17–24 | Process ID (`u32`) |
//! | 25–32 | Low 32 bits of a per-process call counter |
//! | 33–48 | 64 bits from a randomly keyed hash of the three fields above |
//!
//! The counter keeps IDs distinct within one process even when the clock
//! repeats a reading or steps back, the process ID keeps concurrent processes
//! on one host apart, and the keyed hash separates hosts.

use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Calls made to [`new_message_id`] by this process.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// `COPAUA0C 7100-SEND-RESPONSE`: the `MQMD-MSGID` that `MQMI-NONE` asks IBM MQ
/// to generate for the reply [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L746].
///
/// Returns 48 lowercase hexadecimal characters (24 bytes, the width of an MQ
/// `MsgId`), distinct on every call. Safe to call from any thread.
#[must_use]
pub fn new_message_id() -> String {
    // A clock set before 1970 reads as zero; the counter still keeps IDs distinct.
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let nanos = elapsed
        .as_secs()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(elapsed.subsec_nanos()));
    let pid = std::process::id();
    // Uniqueness needs only distinct values, not ordering with other memory.
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let noise = RandomState::new().hash_one((nanos, pid, count));
    let low = count & 0xFFFF_FFFF;
    format!("{nanos:016x}{pid:08x}{low:08x}{noise:016x}")
}

#[cfg(test)]
mod tests {
    use super::new_message_id;
    use std::collections::HashSet;
    use std::thread;

    #[test]
    fn is_48_lowercase_hex_characters() {
        let id = new_message_id();
        assert_eq!(id.len(), 48, "{id}");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{id}"
        );
    }

    #[test]
    fn consecutive_ids_are_distinct() {
        let ids: HashSet<String> = (0..10_000).map(|_| new_message_id()).collect();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn ids_from_concurrent_threads_are_distinct() {
        let handles: Vec<_> = (0..4)
            .map(|_| thread::spawn(|| (0..1_000).map(|_| new_message_id()).collect::<Vec<_>>()))
            .collect();
        let mut ids = HashSet::new();
        for handle in handles {
            ids.extend(handle.join().expect("ID generator thread panicked"));
        }
        assert_eq!(ids.len(), 4_000);
    }
}
