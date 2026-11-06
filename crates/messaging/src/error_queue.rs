//! The `MQ-ERR-DISPLAY` record that CDRD and CDRA write to `CARD.DEMO.ERROR`.
//!
//! `CODATE01` and `COACCT01` declare the same 113-byte record
//! [app/app-vsam-mq/cbl/CODATE01.cbl:L58-67]
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L58-67]. `INITIALIZE MQ-ERR-DISPLAY`
//! leaves its text fields spaces and its codes zeros
//! [app/app-vsam-mq/cbl/CODATE01.cbl:L134]
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L185], a failing path moves its values in,
//! and `9000-ERROR` puts the record to the error queue. The domain rules
//! (`mq_date_reply`, `mq_account_reply`) decide the values as an
//! [`ErrorRecordSpec`]; this module lays out the bytes the flows publish and
//! the text the consumer logs when that publish fails too.
//!
//! | Field | PIC | Bytes | Audit rows |
//! |---|---|---|---|
//! | `MQ-ERROR-PARA` | `X(25)` | 0-24 ([`ERROR_PARA`]) | P-CODATE01-001, P-COACCT01-001 |
//! | `FILLER` | `X(02)` | 25-26 | P-CODATE01-002, P-COACCT01-002 |
//! | `MQ-APPL-RETURN-MESSAGE` | `X(25)` | 27-51 ([`RETURN_MESSAGE`]) | P-CODATE01-003, P-COACCT01-003 |
//! | `FILLER` | `X(02)` | 52-53 | P-CODATE01-004, P-COACCT01-004 |
//! | `MQ-APPL-CONDITION-CODE` | `9(02)` | 54-55 ([`CONDITION_CODE`]) | P-CODATE01-005, P-COACCT01-005 |
//! | `FILLER` | `X(02)` | 56-57 | P-CODATE01-006, P-COACCT01-006 |
//! | `MQ-APPL-REASON-CODE` | `9(05)` | 58-62 ([`REASON_CODE`]) | P-CODATE01-007, P-COACCT01-007 |
//! | `FILLER` | `X(02)` | 63-64 | P-CODATE01-008, P-COACCT01-008 |
//! | `MQ-APPL-QUEUE-NAME` | `X(48)` | 65-112 ([`QUEUE_NAME`]) | P-CODATE01-009, P-COACCT01-009 |
//!
//! Byte positions are 0-based. Every `FILLER` is `VALUE SPACES`, which the
//! `INITIALIZE` does not touch.

use std::ops::Range;

use carddemo_domain::format;
use carddemo_domain::messages;
use carddemo_domain::rules::mq_date_reply::ErrorRecordSpec;

/// CODATE01 2100-OPEN-ERROR-QUEUE / COACCT01 2100-OPEN-ERROR-QUEUE —
/// P-CODATE01-011, P-COACCT01-010.
///
/// The error queue: `MOVE 'CARD.DEMO.ERROR' TO ERROR-QUEUE-NAME`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L243]
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L294]. Both programs name it as a literal,
/// so it is a constant here and not a setting; `infra/rabbitmq/definitions.json`
/// declares it as a durable queue.
pub const ERROR_QUEUE: &str = "CARD.DEMO.ERROR";

/// The length of `MQ-ERR-DISPLAY`, the bytes `DISPLAY MQ-ERR-DISPLAY` prints —
/// P-CODATE01-001 to P-CODATE01-009, P-COACCT01-001 to P-COACCT01-009.
pub const ERR_DISPLAY_LEN: usize = 113;

/// CODATE01 9000-ERROR / COACCT01 9000-ERROR — P-CODATE01-016, P-COACCT01-018.
///
/// The length of the published error message: `MOVE MQ-ERR-DISPLAY TO
/// ERROR-MESSAGE`, an `X(1000)` item, then `MOVE 1000 TO MQ-BUFFER-LENGTH`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L108] [app/app-vsam-mq/cbl/CODATE01.cbl:L409-411]
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L108] [app/app-vsam-mq/cbl/COACCT01.cbl:L505-507].
pub const ERROR_BUFFER_LEN: usize = 1000;

/// `MQ-ERROR-PARA PIC X(25)`, bytes 0-24 — P-CODATE01-001, P-COACCT01-001.
pub const ERROR_PARA: Range<usize> = 0..25;

/// `FILLER PIC X(02) VALUE SPACES` after `MQ-ERROR-PARA` — P-CODATE01-002,
/// P-COACCT01-002.
const FILLER_AFTER_PARA: Range<usize> = 25..27;

/// `MQ-APPL-RETURN-MESSAGE PIC X(25)`, bytes 27-51 — P-CODATE01-003,
/// P-COACCT01-003.
pub const RETURN_MESSAGE: Range<usize> = 27..52;

/// `FILLER PIC X(02) VALUE SPACES` after the return message — P-CODATE01-004,
/// P-COACCT01-004.
const FILLER_AFTER_MESSAGE: Range<usize> = 52..54;

/// `MQ-APPL-CONDITION-CODE PIC 9(02)`, bytes 54-55 — P-CODATE01-005,
/// P-COACCT01-005.
pub const CONDITION_CODE: Range<usize> = 54..56;

/// `FILLER PIC X(02) VALUE SPACES` after the condition code — P-CODATE01-006,
/// P-COACCT01-006.
const FILLER_AFTER_CONDITION: Range<usize> = 56..58;

/// `MQ-APPL-REASON-CODE PIC 9(05)`, bytes 58-62 — P-CODATE01-007,
/// P-COACCT01-007.
pub const REASON_CODE: Range<usize> = 58..63;

/// `FILLER PIC X(02) VALUE SPACES` after the reason code — P-CODATE01-008,
/// P-COACCT01-008.
const FILLER_AFTER_REASON: Range<usize> = 63..65;

/// `MQ-APPL-QUEUE-NAME PIC X(48)`, bytes 65-112 — P-CODATE01-009,
/// P-COACCT01-009.
pub const QUEUE_NAME: Range<usize> = 65..113;

/// The four `FILLER PIC X(02) VALUE SPACES` items — P-CODATE01-002,
/// P-CODATE01-004, P-CODATE01-006, P-CODATE01-008, P-COACCT01-002,
/// P-COACCT01-004, P-COACCT01-006, P-COACCT01-008.
const FILLERS: [Range<usize>; 4] = [
    FILLER_AFTER_PARA,
    FILLER_AFTER_MESSAGE,
    FILLER_AFTER_CONDITION,
    FILLER_AFTER_REASON,
];

// The nine fields tile the record in PIC order: each starts where the previous
// one ends, so their widths sum to the 113 bytes of MQ-ERR-DISPLAY, which fit
// the 1000-byte buffer.
const _: () = {
    let layout = [
        ERROR_PARA,
        FILLER_AFTER_PARA,
        RETURN_MESSAGE,
        FILLER_AFTER_MESSAGE,
        CONDITION_CODE,
        FILLER_AFTER_CONDITION,
        REASON_CODE,
        FILLER_AFTER_REASON,
        QUEUE_NAME,
    ];
    let mut offset = 0;
    let mut index = 0;
    while index < layout.len() {
        assert!(layout[index].start == offset);
        assert!(layout[index].end > layout[index].start);
        offset = layout[index].end;
        index += 1;
    }
    assert!(offset == ERR_DISPLAY_LEN);
    assert!(ERR_DISPLAY_LEN <= ERROR_BUFFER_LEN);
};

/// CODATE01 4100-PUT-REPLY / COACCT01 4100-PUT-REPLY `WHEN OTHER` —
/// P-CODATE01-015, P-CODATE01-019, P-COACCT01-017.
///
/// `MQ-APPL-CONDITION-CODE` of the `MQPUT ERR` record when a publish is
/// returned, nacked, not confirmed or lost with its channel. The COBOL moves
/// the failed `MQPUT`'s condition code [app/app-vsam-mq/cbl/CODATE01.cbl:L397]
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L493]; AMQP produces none, so the record
/// carries 02, the code escalation E-39 proposes for a failed publish (open:
/// the user confirms it).
pub const MQPUT_FAILED_CONDITION_CODE: u32 = 2;

/// CODATE01 4100-PUT-REPLY / COACCT01 4100-PUT-REPLY `WHEN OTHER` —
/// P-CODATE01-015, P-CODATE01-019, P-COACCT01-017.
///
/// `MQ-APPL-REASON-CODE` of the `MQPUT ERR` record when a mandatory publish is
/// returned unroutable: 00000, as escalation E-39 proposes for a returned
/// publish (open).
pub const REASON_RETURNED: u32 = 0;

/// CODATE01 4100-PUT-REPLY / COACCT01 4100-PUT-REPLY `WHEN OTHER` —
/// P-CODATE01-015, P-CODATE01-019, P-COACCT01-017.
///
/// `MQ-APPL-REASON-CODE` of the `MQPUT ERR` record when the broker nacks a
/// publish: 00000, as escalation E-39 proposes for a nacked publish (open).
pub const REASON_NACKED: u32 = 0;

/// CODATE01 4100-PUT-REPLY / COACCT01 4100-PUT-REPLY `WHEN OTHER` —
/// P-CODATE01-015, P-CODATE01-019, P-COACCT01-017.
///
/// `MQ-APPL-REASON-CODE` of the `MQPUT ERR` record when a publish gets no
/// confirmation: 00000, as escalation E-39 proposes for an unconfirmed publish
/// (open).
pub const REASON_NOT_CONFIRMED: u32 = 0;

/// CODATE01 4100-PUT-REPLY / COACCT01 4100-PUT-REPLY `WHEN OTHER` —
/// P-CODATE01-015, P-CODATE01-019, P-COACCT01-017.
///
/// `MQ-APPL-REASON-CODE` of the `MQPUT ERR` record when the channel fails
/// during the publish or while awaiting its confirmation. Such a publish is
/// never confirmed, so it takes the 00000 escalation E-39 proposes for an
/// unconfirmed publish (open); E-39 names no separate channel-failure code.
pub const REASON_CHANNEL_ERROR: u32 = 0;

/// CODATE01 9000-ERROR / COACCT01 9000-ERROR — P-CODATE01-016, P-COACCT01-018,
/// with the fields of P-CODATE01-001 to P-CODATE01-009 and P-COACCT01-001 to
/// P-COACCT01-009.
///
/// The message body `9000-ERROR` puts to [`ERROR_QUEUE`]
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L405-441]
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L501-537]: the 113-byte `MQ-ERR-DISPLAY`
/// moved into `ERROR-MESSAGE X(1000)`, so the result is exactly
/// [`ERROR_BUFFER_LEN`] bytes, the record followed by 887 spaces.
///
/// Each text field is the alphanumeric `MOVE` of the spec's value, cut or
/// space-padded to its `X(n)`; an empty `error_para` leaves the spaces of the
/// `INITIALIZE`. Each code is the numeric `MOVE` into its unsigned `9(n)`: the
/// low-order digits, zero-filled, so reason 2085 is `02085` and 123456 is
/// `23456`. The four `FILLER`s are spaces.
#[must_use]
pub fn record_bytes(spec: &ErrorRecordSpec) -> Vec<u8> {
    let mut message = vec![b' '; ERROR_BUFFER_LEN];
    for filler in FILLERS {
        put_text(&mut message, filler, "");
    }
    put_text(&mut message, ERROR_PARA, spec.error_para);
    put_text(&mut message, RETURN_MESSAGE, spec.return_message);
    put_code(&mut message, CONDITION_CODE, spec.condition_code);
    put_code(&mut message, REASON_CODE, spec.reason_code);
    put_text(&mut message, QUEUE_NAME, &spec.queue_name);
    message
}

/// CODATE01 9000-ERROR / COACCT01 9000-ERROR `DISPLAY MQ-ERR-DISPLAY` —
/// P-CODATE01-016, P-COACCT01-018.
///
/// The text the COBOL `DISPLAY` prints for `record`: its first
/// [`ERR_DISPLAY_LEN`] bytes, the `MQ-ERR-DISPLAY` group, without the spaces of
/// the 1000-byte buffer. A shorter `record` is shown space-padded to 113 bytes,
/// the width of the group. Bytes that are not UTF-8 are shown as U+FFFD; the
/// records of [`record_bytes`] are always UTF-8.
#[must_use]
pub fn display_text(record: &[u8]) -> String {
    String::from_utf8_lossy(&display_group(record)).into_owned()
}

/// CODATE01 9000-ERROR / COACCT01 9000-ERROR `WHEN OTHER` — P-CODATE01-016,
/// P-COACCT01-018.
///
/// The text the COBOL shows when the error record itself cannot be put
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L433-439]
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L529-535]: the `MQ-ERR-DISPLAY` of
/// `record` with the failed put's `condition_code` and `reason_code`,
/// `ERROR-QUEUE-NAME` ([`ERROR_QUEUE`]) as the queue and `MQPUT ERR` as the
/// return message, then `DISPLAY MQ-ERR-DISPLAY`. `MQ-ERROR-PARA` keeps the
/// value `record` holds. The consumer logs this text when the publish of
/// `record` to [`ERROR_QUEUE`] is not confirmed.
///
/// `record` is read as [`display_text`] reads it: its first 113 bytes, a
/// shorter one space-padded.
#[must_use]
pub fn error_put_failure_display(record: &[u8], condition_code: u32, reason_code: u32) -> String {
    let mut group = display_group(record);
    put_code(&mut group, CONDITION_CODE, condition_code);
    put_code(&mut group, REASON_CODE, reason_code);
    put_text(&mut group, QUEUE_NAME, ERROR_QUEUE);
    put_text(&mut group, RETURN_MESSAGE, messages::MQPUT_ERR);
    display_text(&group)
}

/// The `MQ-ERR-DISPLAY` group of `record`: its first [`ERR_DISPLAY_LEN`]
/// bytes, space-padded when `record` is shorter.
fn display_group(record: &[u8]) -> [u8; ERR_DISPLAY_LEN] {
    let mut group = [b' '; ERR_DISPLAY_LEN];
    for (slot, byte) in group.iter_mut().zip(record) {
        *slot = *byte;
    }
    group
}

/// The alphanumeric `MOVE` of `text` into the `X(n)` field at `field`.
fn put_text(record: &mut [u8], field: Range<usize>, text: &str) {
    let moved = format::move_alnum(text, field.len());
    put(record, field, moved.as_bytes());
}

/// The numeric `MOVE` of `code` into the unsigned `9(n)` field at `field`: the
/// low-order `n` digits of its value, zero-filled.
fn put_code(record: &mut [u8], field: Range<usize>, code: u32) {
    let digits = format::unsigned_display(u64::from(code), field.len());
    put(record, field, digits.as_bytes());
}

/// Copies `bytes` into `record` at `field`. The callers pass exactly
/// `field.len()` bytes and ranges inside the record, so nothing is cut; the
/// copy never panics.
fn put(record: &mut [u8], field: Range<usize>, bytes: &[u8]) {
    if let Some(slots) = record.get_mut(field) {
        for (slot, byte) in slots.iter_mut().zip(bytes) {
            *slot = *byte;
        }
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::messages;
    use carddemo_domain::rules::mq_account_reply::read_error_record;
    use carddemo_domain::rules::mq_date_reply::put_reply_failure;

    use super::{
        CONDITION_CODE, ERR_DISPLAY_LEN, ERROR_BUFFER_LEN, ERROR_PARA, ERROR_QUEUE, FILLERS,
        MQPUT_FAILED_CONDITION_CODE, QUEUE_NAME, REASON_CHANNEL_ERROR, REASON_CODE, REASON_NACKED,
        REASON_NOT_CONFIRMED, REASON_RETURNED, RETURN_MESSAGE, display_text,
        error_put_failure_display, record_bytes,
    };

    const DATE_REPLY_QUEUE: &str = "CARD.DEMO.REPLY.DATE";
    const ACCT_REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";

    /// `text` left-justified and space-padded to `width` bytes.
    fn padded(text: &str, width: usize) -> Vec<u8> {
        let mut bytes = text.as_bytes().to_vec();
        bytes.resize(width, b' ');
        bytes
    }

    fn all_spaces(bytes: &[u8]) -> bool {
        bytes.iter().all(|byte| *byte == b' ')
    }

    #[test]
    fn record_is_the_113_byte_group_then_spaces_to_1000() {
        // P-CODATE01-016, P-COACCT01-018: MQ-ERR-DISPLAY moved into ERROR-MESSAGE
        // X(1000) and put with MQ-BUFFER-LENGTH 1000.
        for spec in [
            put_reply_failure(2, 2085, DATE_REPLY_QUEUE),
            read_error_record(ACCT_REQUEST_QUEUE),
        ] {
            let record = record_bytes(&spec);
            assert_eq!(record.len(), ERROR_BUFFER_LEN, "{spec:?}");
            assert!(all_spaces(&record[ERR_DISPLAY_LEN..]), "{spec:?}");
        }
    }

    #[test]
    fn cdrd_put_failure_record_matches_the_section_a_layout() {
        // P-CODATE01-001..009 (layout), P-CODATE01-015 (4100-PUT-REPLY MQPUT ERR):
        // MQ-ERROR-PARA keeps the INITIALIZE spaces, the codes are MOVEd into
        // 9(02) and 9(05), and the reply queue fills X(48).
        let record = record_bytes(&put_reply_failure(2, 2085, DATE_REPLY_QUEUE));
        assert!(all_spaces(&record[ERROR_PARA]));
        assert_eq!(
            &record[RETURN_MESSAGE],
            padded(messages::MQPUT_ERR, 25).as_slice()
        );
        assert_eq!(&record[CONDITION_CODE], b"02");
        assert_eq!(&record[REASON_CODE], b"02085");
        assert_eq!(&record[QUEUE_NAME], padded(DATE_REPLY_QUEUE, 48).as_slice());
        for filler in FILLERS {
            assert_eq!(&record[filler], b"  ");
        }
        let mut expected = padded("", 27);
        expected.extend(padded(messages::MQPUT_ERR, 27));
        expected.extend_from_slice(b"02  02085  ");
        expected.extend(padded(DATE_REPLY_QUEUE, 48));
        assert_eq!(&record[..ERR_DISPLAY_LEN], expected.as_slice());
    }

    #[test]
    fn cdra_read_error_record_cuts_the_message_and_names_the_input_queue() {
        // P-COACCT01-015: 'ERROR WHILE READING ACCTFILE' (28 bytes) cut to X(25),
        // queue = INPUT-QUEUE-NAME; codes 00 and 00000 as E-39 proposes for an
        // account read error. Layout P-COACCT01-001..009.
        let record = record_bytes(&read_error_record(ACCT_REQUEST_QUEUE));
        assert_eq!(messages::ERROR_WHILE_READING_ACCTFILE.len(), 28);
        assert_eq!(&record[RETURN_MESSAGE], b"ERROR WHILE READING ACCTF");
        assert_eq!(&record[CONDITION_CODE], b"00");
        assert_eq!(&record[REASON_CODE], b"00000");
        assert_eq!(
            &record[QUEUE_NAME],
            padded(ACCT_REQUEST_QUEUE, 48).as_slice()
        );
        assert!(all_spaces(&record[ERROR_PARA]));
    }

    #[test]
    fn codes_keep_their_low_order_digits_zero_filled() {
        // P-CODATE01-005, P-CODATE01-007, P-COACCT01-005, P-COACCT01-007: MOVE of
        // the binary codes into unsigned 9(02) and 9(05).
        let cases: [(u32, &[u8; 2], u32, &[u8; 5]); 5] = [
            (0, b"00", 0, b"00000"),
            (2, b"02", 2085, b"02085"),
            (2, b"02", 123_456, b"23456"),
            (123, b"23", 99_999, b"99999"),
            (u32::MAX, b"95", u32::MAX, b"67295"),
        ];
        for (condition, condition_text, reason, reason_text) in cases {
            let record = record_bytes(&put_reply_failure(condition, reason, DATE_REPLY_QUEUE));
            assert_eq!(&record[CONDITION_CODE], condition_text, "{condition}");
            assert_eq!(&record[REASON_CODE], reason_text, "{reason}");
        }
    }

    #[test]
    fn queue_names_are_cut_or_padded_to_x48() {
        // P-CODATE01-009, P-COACCT01-009: alphanumeric MOVE into X(48).
        let long = "Q".repeat(60);
        let record = record_bytes(&put_reply_failure(2, 0, &long));
        assert_eq!(&record[QUEUE_NAME], "Q".repeat(48).as_bytes());
        assert!(all_spaces(&record[ERR_DISPLAY_LEN..]));
        let empty = record_bytes(&put_reply_failure(2, 0, ""));
        assert!(all_spaces(&empty[QUEUE_NAME]));
    }

    #[test]
    fn broker_failure_codes_are_the_e39_proposal() {
        // P-CODATE01-019 (E-39, open): condition 02 and reason 00000 for a
        // returned, nacked or unconfirmed publish; P-CODATE01-015, P-COACCT01-017.
        for reason in [
            REASON_RETURNED,
            REASON_NACKED,
            REASON_NOT_CONFIRMED,
            REASON_CHANNEL_ERROR,
        ] {
            let record = record_bytes(&put_reply_failure(
                MQPUT_FAILED_CONDITION_CODE,
                reason,
                DATE_REPLY_QUEUE,
            ));
            assert_eq!(&record[CONDITION_CODE], b"02");
            assert_eq!(&record[REASON_CODE], b"00000");
        }
    }

    #[test]
    fn display_text_is_the_group_without_the_buffer_spaces() {
        // P-CODATE01-016, P-COACCT01-018: DISPLAY MQ-ERR-DISPLAY prints 113 bytes.
        let record = record_bytes(&put_reply_failure(2, 2085, DATE_REPLY_QUEUE));
        let text = display_text(&record);
        assert_eq!(text.len(), ERR_DISPLAY_LEN);
        assert_eq!(text.as_bytes(), &record[..ERR_DISPLAY_LEN]);
        assert_eq!(display_text(b"AB"), format!("AB{}", " ".repeat(111)));
        assert_eq!(display_text(&[]), " ".repeat(ERR_DISPLAY_LEN));
    }

    #[test]
    fn error_put_failure_rewrites_codes_queue_and_message_and_keeps_the_para() {
        // P-CODATE01-016, P-COACCT01-018: 9000-ERROR WHEN OTHER moves the failed
        // put's codes, ERROR-QUEUE-NAME and 'MQPUT ERR', then DISPLAYs the record;
        // MQ-ERROR-PARA is not moved. Starts from the P-COACCT01-015 record.
        let mut record = record_bytes(&read_error_record(ACCT_REQUEST_QUEUE));
        record[..6].copy_from_slice(b"PARA-X");
        let shown = error_put_failure_display(&record, 2, 123_456);
        let bytes = shown.as_bytes();
        assert_eq!(shown.len(), ERR_DISPLAY_LEN);
        assert_eq!(&bytes[ERROR_PARA], padded("PARA-X", 25).as_slice());
        assert_eq!(
            &bytes[RETURN_MESSAGE],
            padded(messages::MQPUT_ERR, 25).as_slice()
        );
        assert_eq!(&bytes[CONDITION_CODE], b"02");
        assert_eq!(&bytes[REASON_CODE], b"23456");
        assert_eq!(&bytes[QUEUE_NAME], padded(ERROR_QUEUE, 48).as_slice());
        for filler in FILLERS {
            assert_eq!(&bytes[filler], b"  ");
        }
    }

    #[test]
    fn error_put_failure_of_a_short_record_pads_the_group() {
        // P-CODATE01-016: the DISPLAYed group is always 113 bytes.
        let shown = error_put_failure_display(&[], 0, 0);
        let mut expected = padded("", 27);
        expected.extend(padded(messages::MQPUT_ERR, 27));
        expected.extend_from_slice(b"00  00000  ");
        expected.extend(padded(ERROR_QUEUE, 48));
        assert_eq!(shown.as_bytes(), expected.as_slice());
    }

    #[test]
    fn error_queue_is_a_durable_queue_of_the_broker_definitions() {
        // P-CODATE01-011, P-COACCT01-010: the literal error queue is declared by
        // infra/rabbitmq/definitions.json, durable.
        let definitions: String = include_str!("../../../infra/rabbitmq/definitions.json")
            .split_whitespace()
            .collect();
        let entry = format!("\"name\":\"{ERROR_QUEUE}\"");
        let start = definitions
            .find(&entry)
            .expect("CARD.DEMO.ERROR is declared");
        let queue = definitions[start..]
            .split('}')
            .next()
            .expect("the queue object ends");
        assert!(queue.contains("\"durable\":true"), "{queue}");
    }
}
