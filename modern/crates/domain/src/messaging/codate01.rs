//! CDRD, the system-date request that CODATE01 serves over MQ, and the `MQ-ERR-DISPLAY` error
//! message that CODATE01 and COACCT01 declare identically (`CODATE01.cbl:58-67`,
//! `COACCT01.cbl:58-67`; AAP 0.3.4.2).
//!
//! This module formats bytes and decides nothing else. `messaging::workers::cdrd` consumes the
//! request, publishes the body built here, waits for the confirmation and acknowledges. The
//! routing, property and acknowledgement rows of `modern/audit/messaging/CODATE01.md` are the
//! worker's (CODATE01-002, CODATE01-010 to CODATE01-012, CODATE01-018); this module supplies only
//! the bytes those rows publish.
//!
//! | Item | COBOL | Rules |
//! |---|---|---|
//! | [`reply_body`] | `4000-PROCESS-REQUEST-REPLY`, `4100-PUT-REPLY` | CODATE01-001, CODATE01-003 to CODATE01-009, CODATE01-013 |
//! | [`MqErrDisplay`], [`ReturnMessage`] | `MQ-ERR-DISPLAY`, `9000-ERROR` | CODATE01-014 to CODATE01-017, CODATE01-019 |
//! | [`space_padded`] | `MOVE SPACES` and `STRING`, or a group `MOVE`, into a `PIC X(1000)` area | CODATE01-007, CODATE01-015 |
//!
//! The error message's condition and reason codes are halt H4 (CODATE01-016, CODATE01-017): no
//! digit they hold is a parity result. The paths the audit does not carry over (the `CICS
//! RETRIEVE` failure, `MQOPEN`, `MQGET` and `MQCLOSE` errors, the self-move of `RESP2` and the
//! recursive close error, E17) have no code here. Every body is built afresh from its own clock
//! reading, so nothing carries from one request to the next (CODATE01-013, as E14).

use copybook::{PicU, PicX};

use crate::clock::{Clock, hh_mm_ss, mm_dd_yyyy_dash};

/// The reply queue, the literal of `MOVE 'CARD.DEMO.REPLY.DATE' TO REPLY-QUEUE-NAME`
/// (`CODATE01.cbl:147`; CODATE01-002). The worker publishes every reply to the default exchange
/// with this routing key and never to the request's `reply_to`; the README's queue names are not
/// used (E01).
pub const REPLY_QUEUE: &str = "CARD.DEMO.REPLY.DATE";

/// The error queue, the literal of `MOVE 'CARD.DEMO.ERROR' TO ERROR-QUEUE-NAME`
/// (`CODATE01.cbl:243`; CODATE01-014, CODATE01-018). The worker publishes
/// [`MqErrDisplay::error_body`] here when a reply cannot be published.
pub const ERROR_QUEUE: &str = "CARD.DEMO.ERROR";

/// Bytes in every body CDRD publishes: `MQ-BUFFER PIC X(1000)`, and `MOVE 1000 TO
/// MQ-BUFFER-LENGTH` before both the reply put and the error put (`CODATE01.cbl:50,372,411`;
/// CODATE01-007, CODATE01-015).
pub const BUFFER_LEN: usize = 1_000;

/// Bytes of reply text ahead of the padding: `SYSTEM DATE : `, the 10-byte date,
/// `SYSTEM TIME : ` and the 8-byte time, 14 + 10 + 14 + 8 (CODATE01-003 to CODATE01-007).
pub const REPLY_TEXT_LEN: usize = 46;

/// Bytes in the `MQ-ERR-DISPLAY` group: 25 + 2 + 25 + 2 + 2 + 2 + 5 + 2 + 48
/// (`CODATE01.cbl:58-67`; CODATE01-015).
pub const MQ_ERR_DISPLAY_LEN: usize = 113;

/// The first `STRING` operand of the reply (`CODATE01.cbl:355`; CODATE01-003).
const SYSTEM_DATE_LABEL: &[u8; 14] = b"SYSTEM DATE : ";

/// The third `STRING` operand of the reply (`CODATE01.cbl:356`; CODATE01-005).
const SYSTEM_TIME_LABEL: &[u8; 14] = b"SYSTEM TIME : ";

/// `WS-MMDDYYYY PIC X(10)`, the `FORMATTIME MMDDYYYY DATESEP('-')` receiver
/// (`CODATE01.cbl:37,349-350`).
type ReplyDate = PicX<10>;

/// `WS-TIME PIC X(8)`, the `FORMATTIME TIME TIMESEP` receiver (`CODATE01.cbl:38,351-352`).
type ReplyTime = PicX<8>;

/// Each `FILLER PIC X(02) VALUE SPACES` of `MQ-ERR-DISPLAY`.
const FILLER: [u8; 2] = [b' '; 2];

const _: () = assert!(
    SYSTEM_DATE_LABEL.len() + ReplyDate::WIDTH + SYSTEM_TIME_LABEL.len() + ReplyTime::WIDTH
        == REPLY_TEXT_LEN,
    "the CDRD reply text is 46 bytes"
);

// The two code fields encode as many digits as their largest value has nines, 2 and 5.
const _: () = assert!(
    PicU::<2>::MAX == 99 && PicU::<5>::MAX == 99_999,
    "the code fields are PIC 9(02) and PIC 9(05)"
);

const _: () = assert!(
    PicX::<25>::WIDTH
        + FILLER.len()
        + PicX::<25>::WIDTH
        + FILLER.len()
        + 2
        + FILLER.len()
        + 5
        + FILLER.len()
        + PicX::<48>::WIDTH
        == MQ_ERR_DISPLAY_LEN,
    "the MQ-ERR-DISPLAY fields sum to 113 bytes"
);

const _: () = assert!(
    REPLY_TEXT_LEN <= BUFFER_LEN && MQ_ERR_DISPLAY_LEN <= BUFFER_LEN,
    "the reply text and the error group fit the 1,000-byte buffer"
);

/// `content` left-justified in a 1,000-byte message area, the rest spaces, cut at 1,000 bytes.
///
/// This is `MOVE SPACES TO REPLY-MESSAGE` followed by a `STRING … DELIMITED BY SIZE INTO
/// REPLY-MESSAGE`, which writes only its operands, and equally a group `MOVE` into the area, such
/// as `MOVE MQ-ERR-DISPLAY TO ERROR-MESSAGE`, which pads with spaces (`CODATE01.cbl:340,355-360,
/// 409`; CODATE01-007, CODATE01-015). The CDRA bodies of `coacct01` are padded through it too.
#[must_use]
pub fn space_padded(content: &[u8]) -> [u8; BUFFER_LEN] {
    // TRUNCATION: content beyond the 1,000-byte area is dropped, as an alphanumeric MOVE or a
    // STRING into a full receiver drops it; the area is never wider than `MQ-BUFFER`.
    *PicX::<BUFFER_LEN>::moved_from(content).as_bytes()
}

/// The CDRD reply body that `4000-PROCESS-REQUEST-REPLY` builds and `4100-PUT-REPLY` puts
/// (`CODATE01.cbl:339-375`):
///
/// | Offset | Length | Bytes | Rule |
/// |---|---|---|---|
/// | 0 | 14 | `SYSTEM DATE : ` | CODATE01-003 |
/// | 14 | 10 | the date as `MM-DD-YYYY` | CODATE01-004 |
/// | 24 | 14 | `SYSTEM TIME : ` | CODATE01-005 |
/// | 38 | 8 | the time as `HH:MM:SS`, whole seconds | CODATE01-006 |
/// | 46 | 954 | spaces | CODATE01-007 |
///
/// The clock is read exactly once and both the date and the time are rendered from that reading,
/// as one `EXEC CICS ASKTIME` feeds one `FORMATTIME` (CODATE01-008). Each call builds a new body,
/// so no value carries from one reply to the next (CODATE01-013).
///
/// The function takes no request: the program copies the request into `REQUEST-MSG-COPY` and
/// never reads it again, so the reply's bytes do not depend on the request (CODATE01-001), and
/// the body holds no card number, SSN, CVV or password (CODATE01-009).
///
/// The worker owns everything around the body: routing to [`REPLY_QUEUE`] (CODATE01-002), the
/// request's `message_id` and `correlation_id` copied unchanged (CODATE01-010), `delivery_mode`
/// 2 with no `expiration` (CODATE01-011), and the acknowledgement only after a confirmed publish
/// (CODATE01-012).
#[must_use]
pub fn reply_body(clock: &dyn Clock) -> [u8; BUFFER_LEN] {
    let at = clock.now();
    let date = ReplyDate::moved_from(mm_dd_yyyy_dash(at).as_bytes());
    let time = ReplyTime::moved_from(hh_mm_ss(at).as_bytes());
    let mut text = Vec::with_capacity(REPLY_TEXT_LEN);
    text.extend_from_slice(SYSTEM_DATE_LABEL);
    date.encode_into(&mut text);
    text.extend_from_slice(SYSTEM_TIME_LABEL);
    time.encode_into(&mut text);
    space_padded(&text)
}

/// The `MQ-APPL-RETURN-MESSAGE` literals of the CODATE01 paths the replacement carries over.
///
/// Only a failed reply put reaches the error queue (CODATE01-014). The `CICS RETRIEVE`,
/// `MQOPEN`, `MQGET` and `MQCLOSE` paths and their texts have no AMQP counterpart and no rule
/// row in the audit, so they have no variant. When the error put fails too, the worker logs the
/// same text in a tracing event in place of the COBOL `DISPLAY` (CODATE01-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReturnMessage {
    /// `MOVE 'MQPUT ERR' TO MQ-APPL-RETURN-MESSAGE` after a failed `MQPUT`
    /// (`CODATE01.cbl:400,437`; CODATE01-014, CODATE01-019).
    MqPutErr,
}

impl ReturnMessage {
    /// The literal exactly as the program moves it, before the `PIC X(25)` padding
    /// (CODATE01-015).
    #[must_use]
    pub const fn literal(self) -> &'static str {
        match self {
            Self::MqPutErr => "MQPUT ERR",
        }
    }
}

/// `MQ-ERR-DISPLAY`, the error message group of CODATE01, declared identically in COACCT01
/// (`CODATE01.cbl:58-67`, `COACCT01.cbl:58-67`; CODATE01-015):
///
/// | Offset | Length | Field |
/// |---|---|---|
/// | 0 | 25 | `MQ-ERROR-PARA PIC X(25)` |
/// | 25 | 2 | `FILLER PIC X(02) VALUE SPACES` |
/// | 27 | 25 | `MQ-APPL-RETURN-MESSAGE PIC X(25)` |
/// | 52 | 2 | `FILLER PIC X(02) VALUE SPACES` |
/// | 54 | 2 | `MQ-APPL-CONDITION-CODE PIC 9(02)` |
/// | 56 | 2 | `FILLER PIC X(02) VALUE SPACES` |
/// | 58 | 5 | `MQ-APPL-REASON-CODE PIC 9(05)` |
/// | 63 | 2 | `FILLER PIC X(02) VALUE SPACES` |
/// | 65 | 48 | `MQ-APPL-QUEUE-NAME PIC X(48)` |
///
/// The four `FILLER`s keep their `VALUE SPACES`, which `INITIALIZE` leaves unchanged, so they are
/// constant spaces in [`MqErrDisplay::encode`] rather than fields. Start from
/// [`MqErrDisplay::initialized`] and set fields with COBOL `MOVE` semantics
/// ([`PicX::moved_from`]); [`MqErrDisplay::for_failure`] does that for this program's literals.
/// The code fields are halt H4 (CODATE01-016, CODATE01-017): their digits are never a parity
/// result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqErrDisplay {
    /// `MQ-ERROR-PARA`. Spaces from `INITIALIZE`: its only writer is the `CICS RETRIEVE`
    /// failure (`CODATE01.cbl:149`), which a long-running AMQP consumer does not have
    /// (CODATE01-015).
    pub error_para: PicX<25>,
    /// `MQ-APPL-RETURN-MESSAGE`, a literal moved in by the failing paragraph (CODATE01-015).
    pub return_message: PicX<25>,
    // HALTED(H4): the MQ completion code, or the CICS `RESP` in COACCT01, has no AMQP or
    // `PostgreSQL` counterpart.
    /// `MQ-APPL-CONDITION-CODE`, zeros from `INITIALIZE` (CODATE01-016).
    pub condition_code: PicU<2>,
    // HALTED(H4): the MQ reason code, or the CICS `RESP2` in COACCT01, has no AMQP or
    // `PostgreSQL` counterpart.
    /// `MQ-APPL-REASON-CODE`, zeros from `INITIALIZE` (CODATE01-017).
    pub reason_code: PicU<5>,
    /// `MQ-APPL-QUEUE-NAME`, the queue whose operation failed (CODATE01-015).
    pub queue_name: PicX<48>,
}

impl MqErrDisplay {
    /// `INITIALIZE MQ-ERR-DISPLAY` (`CODATE01.cbl:134`): the alphanumeric fields spaces and the
    /// numeric fields zeros (CODATE01-015).
    #[must_use]
    pub const fn initialized() -> Self {
        Self {
            error_para: PicX::spaces(),
            return_message: PicX::spaces(),
            condition_code: PicU::ZERO,
            reason_code: PicU::ZERO,
            queue_name: PicX::spaces(),
        }
    }

    /// The error message of a failed put: [`MqErrDisplay::initialized`], then `MOVE` of the
    /// literal to `MQ-APPL-RETURN-MESSAGE` and of the queue name to `MQ-APPL-QUEUE-NAME`
    /// (`CODATE01.cbl:397-400,433-437`). Each is an alphanumeric `MOVE`, padded with spaces and
    /// cut on the right, so a configured queue name longer than 48 bytes keeps its first 48.
    ///
    /// A failed reply put gives `for_failure(ReturnMessage::MqPutErr, REPLY_QUEUE)`, which the
    /// worker publishes to [`ERROR_QUEUE`] (CODATE01-014, CODATE01-015). When that publication
    /// fails too, `for_failure(ReturnMessage::MqPutErr, ERROR_QUEUE)` holds the return message and
    /// queue name of the worker's tracing event, which stands in for the COBOL `DISPLAY`
    /// (CODATE01-019). `error_para` stays spaces and the code fields stay zeros (CODATE01-016,
    /// CODATE01-017).
    #[must_use]
    pub fn for_failure(message: ReturnMessage, queue_name: &str) -> Self {
        Self {
            return_message: PicX::moved_from(message.literal().as_bytes()),
            queue_name: PicX::moved_from(queue_name.as_bytes()),
            ..Self::initialized()
        }
    }

    /// This message with `condition_code` and `reason_code` in the code fields, for a value an
    /// audit row designates; each field's width is in its type, so no value is cut.
    ///
    /// HALTED(H4): the COBOL moves the MQ completion and reason codes here (the CICS `RESP` and
    /// `RESP2` in COACCT01). IBM MQ and CICS define them, and AMQP and `PostgreSQL` have no
    /// counterpart, so these digits are never a parity result (CODATE01-016, CODATE01-017).
    // HALTED(H4): the code fields carry no parity value; the initialised zeros are the default.
    #[must_use]
    pub const fn with_codes(self, condition_code: PicU<2>, reason_code: PicU<5>) -> Self {
        Self {
            condition_code,
            reason_code,
            ..self
        }
    }

    /// The 113 bytes of the group in declaration order, `FILLER`s as spaces and the codes as
    /// zero-padded digits (`CODATE01.cbl:58-67`; CODATE01-015).
    #[must_use]
    pub fn encode(&self) -> [u8; MQ_ERR_DISPLAY_LEN] {
        let mut group = Vec::with_capacity(MQ_ERR_DISPLAY_LEN);
        self.error_para.encode_into(&mut group);
        group.extend_from_slice(&FILLER);
        self.return_message.encode_into(&mut group);
        group.extend_from_slice(&FILLER);
        self.condition_code.encode_into(&mut group);
        group.extend_from_slice(&FILLER);
        self.reason_code.encode_into(&mut group);
        group.extend_from_slice(&FILLER);
        self.queue_name.encode_into(&mut group);
        // The field widths sum to the group's length (checked at compile time above), so this
        // MOVE copies every byte and neither pads nor cuts.
        *PicX::<MQ_ERR_DISPLAY_LEN>::moved_from(&group).as_bytes()
    }

    /// The 1,000-byte error body: `MOVE MQ-ERR-DISPLAY TO ERROR-MESSAGE`, which pads the group
    /// with 887 spaces, and the put of all 1,000 bytes (`CODATE01.cbl:409-411`; CODATE01-015).
    /// The worker publishes it to [`ERROR_QUEUE`] with the request's IDs (CODATE01-018).
    #[must_use]
    pub fn error_body(&self) -> [u8; BUFFER_LEN] {
        space_padded(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{PicU, PicX};

    use super::{
        BUFFER_LEN, ERROR_QUEUE, MQ_ERR_DISPLAY_LEN, MqErrDisplay, REPLY_QUEUE, REPLY_TEXT_LEN,
        ReturnMessage, reply_body, space_padded,
    };
    use crate::clock::{Clock, FixedClock, hh_mm_ss, mm_dd_yyyy_dash};

    type TestResult = Result<(), &'static str>;

    /// The instant `date` at `time`, given as hour, minute and second.
    fn instant(
        date: (i32, u32, u32),
        time: (u32, u32, u32),
    ) -> Result<NaiveDateTime, &'static str> {
        let (year, month, day) = date;
        let (hour, minute, second) = time;
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|day| day.and_hms_opt(hour, minute, second))
            .ok_or("no such instant")
    }

    /// The `len` bytes of `bytes` from `start`.
    fn segment(bytes: &[u8], start: usize, len: usize) -> Result<&[u8], &'static str> {
        start
            .checked_add(len)
            .and_then(|end| bytes.get(start..end))
            .ok_or("segment out of range")
    }

    fn all_spaces(bytes: &[u8]) -> bool {
        bytes.iter().all(|&byte| byte == b' ')
    }

    /// `text` followed by spaces to `width` bytes.
    fn padded(text: &[u8], width: usize) -> Vec<u8> {
        let mut field = text.to_vec();
        field.resize(width, b' ');
        field
    }

    /// Whether `bytes` has the layout `pattern`, where `9` stands for any ASCII digit and every
    /// other byte for itself.
    fn has_format(bytes: &[u8], pattern: &[u8]) -> bool {
        bytes.len() == pattern.len()
            && bytes.iter().zip(pattern).all(|(&byte, &shape)| {
                if shape == b'9' {
                    byte.is_ascii_digit()
                } else {
                    byte == shape
                }
            })
    }

    /// A clock that returns `first` on its first reading and `later` on every other, counting
    /// its readings.
    struct TwoInstantClock {
        first: NaiveDateTime,
        later: NaiveDateTime,
        reads: AtomicUsize,
    }

    impl Clock for TwoInstantClock {
        fn now(&self) -> NaiveDateTime {
            if self.reads.fetch_add(1, Ordering::SeqCst) == 0 {
                self.first
            } else {
                self.later
            }
        }
    }

    // audit: CODATE01-002
    // audit: CODATE01-007
    // audit: CODATE01-018
    #[test]
    fn queues_and_lengths_are_the_cobol_values() {
        assert_eq!(REPLY_QUEUE, "CARD.DEMO.REPLY.DATE");
        assert_eq!(ERROR_QUEUE, "CARD.DEMO.ERROR");
        assert_eq!(BUFFER_LEN, 1_000);
        assert_eq!(REPLY_TEXT_LEN, 46);
        assert_eq!(MQ_ERR_DISPLAY_LEN, 113);
    }

    // audit: CODATE01-003
    // audit: CODATE01-004
    // audit: CODATE01-005
    // audit: CODATE01-006
    // audit: CODATE01-007
    #[test]
    fn reply_body_is_the_date_and_time_text_then_spaces() -> TestResult {
        let body = reply_body(&FixedClock::new(instant((2022, 7, 18), (13, 45, 7))?));
        assert_eq!(body.len(), BUFFER_LEN);
        assert_eq!(segment(&body, 0, 14)?, b"SYSTEM DATE : ".as_slice());
        assert!(has_format(segment(&body, 14, 10)?, b"99-99-9999"));
        assert_eq!(segment(&body, 24, 14)?, b"SYSTEM TIME : ".as_slice());
        assert!(has_format(segment(&body, 38, 8)?, b"99:99:99"));
        assert!(all_spaces(segment(&body, REPLY_TEXT_LEN, 954)?));
        Ok(())
    }

    // audit: CODATE01-008
    #[test]
    fn reply_body_renders_date_and_time_from_one_reading() -> TestResult {
        let first = instant((1999, 12, 31), (23, 59, 59))?;
        let clock = TwoInstantClock {
            first,
            later: instant((2000, 1, 1), (0, 0, 0))?,
            reads: AtomicUsize::new(0),
        };
        let body = reply_body(&clock);
        assert_eq!(clock.reads.load(Ordering::SeqCst), 1);
        assert_eq!(segment(&body, 14, 10)?, mm_dd_yyyy_dash(first).as_bytes());
        assert_eq!(segment(&body, 38, 8)?, hh_mm_ss(first).as_bytes());
        // A second reading would have crossed midnight, so a body built from two readings could
        // pair the old date with the new time.
        assert_eq!(clock.now(), clock.later);
        Ok(())
    }

    // audit: CODATE01-013
    #[test]
    fn each_reply_depends_only_on_its_own_reading() -> TestResult {
        let morning = FixedClock::new(instant((2022, 7, 18), (8, 0, 1))?);
        let evening = FixedClock::new(instant((2023, 1, 2), (21, 30, 0))?);
        let first = reply_body(&morning);
        let other = reply_body(&evening);
        assert_ne!(first, other);
        assert_eq!(reply_body(&morning), first);
        Ok(())
    }

    // audit: CODATE01-007
    #[test]
    fn space_padded_fills_and_cuts_at_the_buffer() -> TestResult {
        assert!(all_spaces(&space_padded(b"")));
        let short = space_padded(b"AB");
        assert_eq!(segment(&short, 0, 2)?, b"AB".as_slice());
        assert!(all_spaces(segment(&short, 2, 998)?));
        let long = space_padded(&[b'A'; 1_200]);
        assert!(long.iter().all(|&byte| byte == b'A'));
        Ok(())
    }

    // audit: CODATE01-014
    #[test]
    fn mqput_err_is_the_only_carried_return_message() {
        assert_eq!(ReturnMessage::MqPutErr.literal(), "MQPUT ERR");
        let message = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, REPLY_QUEUE);
        assert_eq!(message.return_message.to_text(), "MQPUT ERR");
        assert_eq!(message.queue_name.to_text(), REPLY_QUEUE);
    }

    // audit: CODATE01-015
    #[test]
    fn error_body_lays_out_mq_err_display() -> TestResult {
        let message = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, REPLY_QUEUE);
        let group = message.encode();
        assert_eq!(group.len(), MQ_ERR_DISPLAY_LEN);
        assert!(all_spaces(segment(&group, 0, 25)?));
        assert!(all_spaces(segment(&group, 25, 2)?));
        assert_eq!(segment(&group, 27, 25)?, padded(b"MQPUT ERR", 25));
        assert!(all_spaces(segment(&group, 52, 2)?));
        assert!(all_spaces(segment(&group, 56, 2)?));
        assert!(all_spaces(segment(&group, 63, 2)?));
        assert_eq!(segment(&group, 65, 48)?, padded(REPLY_QUEUE.as_bytes(), 48));
        let body = message.error_body();
        assert_eq!(body.len(), BUFFER_LEN);
        assert_eq!(segment(&body, 0, MQ_ERR_DISPLAY_LEN)?, group.as_slice());
        assert!(all_spaces(segment(&body, MQ_ERR_DISPLAY_LEN, 887)?));
        Ok(())
    }

    // audit: CODATE01-015
    #[test]
    fn queue_name_is_cut_at_48_bytes() -> TestResult {
        let long_name = "Q".repeat(60);
        let group = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, &long_name).encode();
        assert_eq!(segment(&group, 65, 48)?, [b'Q'; 48].as_slice());
        assert_eq!(group.len(), MQ_ERR_DISPLAY_LEN);
        Ok(())
    }

    // `INITIALIZE` gives the code fields zeros; their published digits are HALTED(H4), so this
    // checks the default only, never a parity value, and carries no audit tag.
    #[test]
    fn initialized_is_spaces_and_zeros() {
        let initial = MqErrDisplay::initialized();
        assert!(initial.error_para.is_spaces());
        assert!(initial.return_message.is_spaces());
        assert!(initial.queue_name.is_spaces());
        assert_eq!(initial.condition_code, PicU::ZERO);
        assert_eq!(initial.reason_code, PicU::ZERO);
        let failure = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, ERROR_QUEUE);
        assert_eq!(failure.error_para, PicX::spaces());
        assert_eq!(failure.condition_code, PicU::ZERO);
        assert_eq!(failure.reason_code, PicU::ZERO);
    }

    // HALTED(H4): runs the code setter for coverage; the digits are checked by format only.
    #[test]
    fn with_codes_changes_only_the_code_fields() -> TestResult {
        let base = MqErrDisplay::for_failure(ReturnMessage::MqPutErr, REPLY_QUEUE);
        let condition = PicU::<2>::new(2).map_err(|_| "condition code")?;
        let reason = PicU::<5>::new(2_080).map_err(|_| "reason code")?;
        let coded = base.clone().with_codes(condition, reason);
        assert_eq!(coded.condition_code, condition);
        assert_eq!(coded.reason_code, reason);
        let (before, after) = (base.encode(), coded.encode());
        for (start, len) in [(0, 54), (56, 2), (63, 50)] {
            assert_eq!(segment(&before, start, len)?, segment(&after, start, len)?);
        }
        assert!(has_format(segment(&after, 54, 2)?, b"99"));
        assert!(has_format(segment(&after, 58, 5)?, b"99999"));
        Ok(())
    }
}
