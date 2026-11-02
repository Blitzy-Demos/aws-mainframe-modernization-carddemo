//! `CODATE01` (CDRD): the system date and time reply, and the error record of
//! a reply that could not be put.
//!
//! `CODATE01` answers every request on its trigger queue with the CICS date
//! and time [app/app-vsam-mq/cbl/CODATE01.cbl:L339-364]; it reads none of the
//! request body. This module decides the bytes: the 1000-byte reply text, and
//! the data of the `MQ-ERR-DISPLAY` record written to `CARD.DEMO.ERROR` when
//! the reply put fails [app/app-vsam-mq/cbl/CODATE01.cbl:L392-403] or when a
//! request longer than the 1000-byte get buffer fails its get
//! [app/app-vsam-mq/cbl/CODATE01.cbl:L329-335]. It is pure:
//! the instant comes from the caller's clock, and the record's bytes are laid
//! out by the messaging crate's error-queue module.
//!
//! The MQ mechanics belong to the CDRD flow and the shared consumer loop of
//! the messaging crate: the fixed reply queue, the `message_id` and
//! `correlation_id` echo (P-CODATE01-014), acknowledgement only after the
//! reply's publisher confirm, and rejection without requeue after an error
//! record (P-CODATE01-024). When the error record cannot be put either,
//! `9000-ERROR` only `DISPLAY`s it (P-CODATE01-016); the flow logs that same
//! record, so this module exposes nothing for it. The `MQOPEN` and `MQCLOSE`
//! failure paths have no counterpart in a long-running consumer and are not
//! reproduced (E-22, P-CODATE01-017, P-CODATE01-018).

use chrono::{DateTime, Utc};

use crate::{clock, format, messages};

/// The byte length of the CDRD reply: `REPLY-MESSAGE PIC X(1000)` put with
/// `MQ-BUFFER-LENGTH` 1000 [app/app-vsam-mq/cbl/CODATE01.cbl:L371-372] —
/// P-CODATE01-013, P-CODATE01-014.
///
/// Re-exported from [`crate::format`], which pads the reply to it, so the rule
/// and the layout cannot disagree on the length.
pub use crate::format::REPLY_BUFFER_LEN;

/// CODATE01 4000-PROCESS-REQUEST-REPLY — P-CODATE01-013, P-CODATE01-020,
/// P-CODATE01-021.
///
/// The reply text for a request answered at `now`:
/// `MOVE SPACES TO REPLY-MESSAGE`, then `STRING 'SYSTEM DATE : ' WS-MMDDYYYY
/// 'SYSTEM TIME : ' WS-TIME DELIMITED BY SIZE`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L340-360]. The date is `mm-dd-yyyy`
/// (`FORMATTIME MMDDYYYY DATESEP('-')`) at bytes 15-24 and the time
/// `hh:mm:ss` (`FORMATTIME TIME TIMESEP`, default separator `:`) at bytes
/// 39-46, both rendered in UTC by [`crate::clock`]. No separator stands between
/// the date and the second label. The 46 text bytes are followed by 954 spaces,
/// so the result is exactly [`REPLY_BUFFER_LEN`] bytes, the length the reply
/// put sends (P-CODATE01-014). The two labels are layout literals owned by
/// [`crate::format`], not message-catalog texts, so they come in with the
/// layout.
#[must_use]
pub fn reply_body(now: DateTime<Utc>) -> String {
    let text = format::date_reply(&clock::mmddyyyy_dashed(now), &clock::hhmmss_colon(now));
    format::pad_reply_1000(&text)
}

/// The data of one `MQ-ERR-DISPLAY` record
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L58-67], 113 bytes in a 1000-byte buffer
/// on `CARD.DEMO.ERROR` — P-CODATE01-001 to P-CODATE01-009, P-CODATE01-016.
///
/// The fields hold the values the COBOL moves; the messaging crate's
/// error-queue module lays out the bytes: each text field moved into its
/// `PIC X(n)` (cut or space-padded), each code moved into its `PIC 9(n)`
/// (low-order digits, zero-filled), and the four two-byte `FILLER`s as spaces.
/// `COACCT01` declares the same record [app/app-vsam-mq/cbl/COACCT01.cbl:L58-67]
/// and its rule reuses this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRecordSpec {
    /// `MQ-ERROR-PARA PIC X(25)` at offset 0 (P-CODATE01-001). Empty when the
    /// program never sets it, which leaves the spaces of
    /// `INITIALIZE MQ-ERR-DISPLAY`.
    pub error_para: &'static str,
    /// `MQ-APPL-RETURN-MESSAGE PIC X(25)` at offset 27 (P-CODATE01-003).
    pub return_message: &'static str,
    /// `MQ-APPL-CONDITION-CODE PIC 9(02)` at offset 54 (P-CODATE01-005).
    pub condition_code: u32,
    /// `MQ-APPL-REASON-CODE PIC 9(05)` at offset 58 (P-CODATE01-007).
    pub reason_code: u32,
    /// `MQ-APPL-QUEUE-NAME PIC X(48)` at offset 65 (P-CODATE01-009).
    pub queue_name: String,
}

/// CODATE01 3000-GET-REQUEST — P-CODATE01-012.
///
/// The byte length of the request buffer: the `MQGET` reads into `MQ-BUFFER
/// PIC X(1000)` [app/app-vsam-mq/cbl/CODATE01.cbl:L50] with `MQ-BUFFER-LENGTH`
/// 1000 [app/app-vsam-mq/cbl/CODATE01.cbl:L291], the buffer the reply is put
/// from, so it is [`REPLY_BUFFER_LEN`]. `COACCT01` gets its requests the same
/// way [app/app-vsam-mq/cbl/COACCT01.cbl:L342].
pub const GET_BUFFER_LEN: usize = REPLY_BUFFER_LEN;

/// The IBM MQ completion code `MQCC_WARNING`, which `MQGET` returns with
/// [`MQRC_TRUNCATED_MSG_FAILED`] — P-CODATE01-012.
pub const MQCC_WARNING: u32 = 1;

/// The IBM MQ reason code `MQRC_TRUNCATED_MSG_FAILED`: the message is longer
/// than the buffer and the get options lack `MQGMO-ACCEPT-TRUNCATED-MSG`, as
/// those of `3000-GET-REQUEST` do
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L296-299] — P-CODATE01-012.
pub const MQRC_TRUNCATED_MSG_FAILED: u32 = 2080;

/// CODATE01 3000-GET-REQUEST — P-CODATE01-012.
///
/// The error record of a get that failed other than with no message
/// available [app/app-vsam-mq/cbl/CODATE01.cbl:L329-335]: the get's condition
/// and reason codes, `INPUT-QUEUE-NAME` as the queue and `'INP MQGET ERR:'` as
/// the return message. `9000-ERROR` then puts it to `CARD.DEMO.ERROR` and
/// `8000-TERMINATION` ends the task; no reply is put. `MQ-ERROR-PARA` keeps
/// the spaces of `INITIALIZE MQ-ERR-DISPLAY`, as in [`put_reply_failure`].
#[must_use]
pub fn get_request_failure(
    condition_code: u32,
    reason_code: u32,
    input_queue: &str,
) -> ErrorRecordSpec {
    ErrorRecordSpec {
        error_para: "",
        return_message: messages::INP_MQGET_ERR,
        condition_code,
        reason_code,
        queue_name: input_queue.to_owned(),
    }
}

/// CODATE01 3000-GET-REQUEST — P-CODATE01-012: the outcome of a request
/// `body` longer than [`GET_BUFFER_LEN`], or `None` for one that fits.
///
/// The `MQGET` options are `MQGMO-SYNCPOINT`, `MQGMO-FAIL-IF-QUIESCING`,
/// `MQGMO-CONVERT` and `MQGMO-WAIT`, without `MQGMO-ACCEPT-TRUNCATED-MSG`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L296-299], so IBM MQ fails the get of
/// such a message with [`MQCC_WARNING`] and [`MQRC_TRUNCATED_MSG_FAILED`]. That
/// is not `MQRC-NO-MSG-AVAILABLE`, so the request gets no reply but the
/// [`get_request_failure`] record for `input_queue`.
///
/// IBM MQ leaves a message whose get fails this way on its queue. The
/// consumer cannot: it publishes the record and rejects the request without
/// requeue, the only settlement it has for a request without a reply, so the
/// request is consumed once, as single delivery assumes.
#[must_use]
pub fn oversize_request_record(body: &[u8], input_queue: &str) -> Option<ErrorRecordSpec> {
    (body.len() > GET_BUFFER_LEN)
        .then(|| get_request_failure(MQCC_WARNING, MQRC_TRUNCATED_MSG_FAILED, input_queue))
}

/// CODATE01 4100-PUT-REPLY — P-CODATE01-015, P-CODATE01-019.
///
/// The error record of a reply put that failed, `WHEN OTHER` of the `MQPUT`
/// outcome [app/app-vsam-mq/cbl/CODATE01.cbl:L396-402]: the put's condition
/// and reason codes, `REPLY-QUEUE-NAME` as the queue and `'MQPUT ERR'` as the
/// return message. `9000-ERROR` then puts it to `CARD.DEMO.ERROR`; the request
/// is consumed and no reply exists.
///
/// `MQ-ERROR-PARA` stays as `INITIALIZE MQ-ERR-DISPLAY` left it
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L134]: spaces. Its only assignment,
/// `'CICS RETRIEVE'` [app/app-vsam-mq/cbl/CODATE01.cbl:L149], ends the task
/// before any request is read.
///
/// AMQP has no MQ codes; the caller passes the codes its publish failure maps
/// to (E-39).
#[must_use]
pub fn put_reply_failure(
    condition_code: u32,
    reason_code: u32,
    reply_queue: &str,
) -> ErrorRecordSpec {
    ErrorRecordSpec {
        error_para: "",
        return_message: messages::MQPUT_ERR,
        condition_code,
        reason_code,
        queue_name: reply_queue.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{
        ErrorRecordSpec, GET_BUFFER_LEN, REPLY_BUFFER_LEN, get_request_failure,
        oversize_request_record, put_reply_failure, reply_body,
    };
    use crate::messages;

    fn at(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("test instant is RFC 3339")
            .with_timezone(&Utc)
    }

    #[test]
    fn reply_is_labels_date_and_time_then_spaces_to_1000_bytes() {
        // P-CODATE01-013, P-CODATE01-014: 46 STRING bytes, then 954 spaces.
        let reply = reply_body(at("2022-07-18T13:45:09Z"));
        assert_eq!(REPLY_BUFFER_LEN, 1000);
        assert_eq!(reply.len(), REPLY_BUFFER_LEN);
        assert_eq!(
            &reply[..46],
            "SYSTEM DATE : 07-18-2022SYSTEM TIME : 13:45:09"
        );
        assert!(reply[46..].bytes().all(|byte| byte == b' '));
    }

    #[test]
    fn date_and_time_sit_at_their_audited_positions_in_utc() {
        // P-CODATE01-020: date at bytes 15-24; P-CODATE01-021: time at bytes
        // 39-46 (1-based), whole seconds, rendered in UTC.
        let cases = [
            ("2022-07-18T13:45:09Z", "07-18-2022", "13:45:09"),
            ("2022-01-05T00:00:00Z", "01-05-2022", "00:00:00"),
            ("2023-12-31T23:59:59.999Z", "12-31-2023", "23:59:59"),
            ("2024-02-29T09:08:07Z", "02-29-2024", "09:08:07"),
            ("2022-07-18T23:30:00-02:00", "07-19-2022", "01:30:00"),
        ];
        for (instant, date, time) in cases {
            let reply = reply_body(at(instant));
            assert_eq!(&reply[..14], "SYSTEM DATE : ", "{instant}");
            assert_eq!(&reply[14..24], date, "{instant}");
            assert_eq!(&reply[24..38], "SYSTEM TIME : ", "{instant}");
            assert_eq!(&reply[38..46], time, "{instant}");
            assert_eq!(reply.len(), REPLY_BUFFER_LEN, "{instant}");
        }
    }

    #[test]
    fn put_failure_record_carries_mqput_err_the_codes_and_the_reply_queue() {
        // P-CODATE01-015: 'MQPUT ERR', the put's codes and REPLY-QUEUE-NAME;
        // MQ-ERROR-PARA keeps the spaces of the INITIALIZE (P-CODATE01-001).
        // P-CODATE01-019 (E-39): the codes are the caller's, passed through
        // unchanged; their MOVE into 9(02) and 9(05) belongs to the layout.
        let cases = [
            (2, 2085, "CARD.DEMO.REPLY.DATE"),
            (0, 0, "CARD.DEMO.REPLY.DATE"),
            (2, 123_456, "ANOTHER.QUEUE"),
        ];
        for (condition_code, reason_code, queue) in cases {
            assert_eq!(
                put_reply_failure(condition_code, reason_code, queue),
                ErrorRecordSpec {
                    error_para: "",
                    return_message: messages::MQPUT_ERR,
                    condition_code,
                    reason_code,
                    queue_name: queue.to_owned(),
                },
                "{condition_code} {reason_code} {queue}"
            );
        }
    }

    #[test]
    fn a_request_longer_than_the_get_buffer_is_the_inp_mqget_err_record() {
        // P-CODATE01-012: MQ-BUFFER-LENGTH 1000 without ACCEPT-TRUNCATED-MSG;
        // the failed get's codes (MQCC_WARNING 1, MQRC_TRUNCATED_MSG_FAILED
        // 2080), INPUT-QUEUE-NAME and 'INP MQGET ERR:'.
        assert_eq!(GET_BUFFER_LEN, 1000);
        let queue = "CARD.DEMO.REQUEST.DATE";
        let expected = ErrorRecordSpec {
            error_para: "",
            return_message: messages::INP_MQGET_ERR,
            condition_code: 1,
            reason_code: 2080,
            queue_name: queue.to_owned(),
        };
        assert_eq!(get_request_failure(1, 2080, queue), expected);
        for len in [1001, 4096] {
            assert_eq!(
                oversize_request_record(&vec![b'X'; len], queue),
                Some(expected.clone()),
                "{len}"
            );
        }
        for len in [0, 4, 999, 1000] {
            assert_eq!(
                oversize_request_record(&vec![b' '; len], queue),
                None,
                "{len}"
            );
        }
    }

    #[test]
    fn put_failure_texts_fit_their_record_fields_whole() {
        // P-CODATE01-003, P-CODATE01-009: 'MQPUT ERR' fits X(25) and the reply
        // queue name fits X(48), so the record keeps both whole.
        let spec = put_reply_failure(2, 2085, "CARD.DEMO.REPLY.DATE");
        assert_eq!(spec.return_message, messages::MQPUT_ERR);
        assert!(spec.return_message.len() <= 25);
        assert!(spec.queue_name.len() <= 48);
    }
}
