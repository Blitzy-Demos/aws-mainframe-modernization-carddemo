//! CDRD: the system date and time inquiry of `CODATE01`.
//!
//! IBM MQ starts `CODATE01` by trigger on its request queue. It answers each
//! request with the CICS date and time and puts the reply to the fixed queue
//! `CARD.DEMO.REPLY.DATE` [app/app-vsam-mq/cbl/CODATE01.cbl:L147]. Here
//! [`CdrdFlow`] is served by the long-running consumer of
//! [`crate::consumer::run`] on the request queue the settings name
//! (`CARDDEMO_QUEUE_DATE_REQUEST`), and it decides only what to send:
//!
//! - every byte of the reply comes from
//!   `carddemo_domain::rules::mq_date_reply::reply_body`, and the content of
//!   the request body is never read (P-CODATE01-012, P-CODATE01-013);
//! - a request longer than the 1000-byte get buffer gets no reply: its get
//!   fails, and the `INP MQGET ERR:` record of `3000-GET-REQUEST` goes to
//!   `CARD.DEMO.ERROR` before the request is rejected without requeue
//!   (P-CODATE01-012);
//! - the reply goes to [`REPLY_QUEUE`] with the request's `message_id` and
//!   `correlation_id`; the request's `reply_to` is ignored, as the COBOL never
//!   reads its saved `REPLYTOQ` (P-CODATE01-014);
//! - a reply the broker does not confirm turns into the `MQPUT ERR` record of
//!   `MQ-ERR-DISPLAY`, which the consumer publishes to `CARD.DEMO.ERROR` before
//!   it rejects the request without requeue (P-CODATE01-015, P-CODATE01-024).
//!
//! Publishing, confirming, acknowledging and rejecting all belong to the
//! consumer. The `MQOPEN` and `MQCLOSE` failure paths of `CODATE01` have no
//! counterpart, because the consumer opens and closes no queue per request:
//! they are not reproduced (E-22; P-CODATE01-017, P-CODATE01-018,
//! P-CODATE01-022, P-CODATE01-023).

use std::future::{self, Future};

use carddemo_domain::clock::{AnyClock, Clock};
use carddemo_domain::rules::mq_date_reply;

use crate::consumer::{Disposition, FailureAction, Flow, PublishFailure, Reply, Request};
use crate::error_queue;

/// CODATE01 1000-CONTROL / 2400-OPEN-OUTPUT-QUEUE — P-CODATE01-010,
/// P-CODATE01-014, P-CODATE01-023.
///
/// The reply queue: `MOVE 'CARD.DEMO.REPLY.DATE' TO REPLY-QUEUE-NAME`
/// [app/app-vsam-mq/cbl/CODATE01.cbl:L147]. The COBOL names it as a literal,
/// so it is a constant here and not a setting. `infra/rabbitmq/definitions.json`
/// declares it as a durable queue.
pub const REPLY_QUEUE: &str = "CARD.DEMO.REPLY.DATE";

/// The CDRD flow: the `CODATE01` rules adapted to [`Flow`].
#[derive(Debug, Clone)]
pub struct CdrdFlow {
    /// The `EXEC CICS ASKTIME` instant source: the host clock, or the
    /// `CARDDEMO_CLOCK_OVERRIDE` pin.
    clock: AnyClock,
    /// `INPUT-QUEUE-NAME`, the queue the request came from, which `CODATE01`
    /// takes from `MQTM-QNAME` [app/app-vsam-mq/cbl/CODATE01.cbl:L146]. The
    /// record of a failed get names it [app/app-vsam-mq/cbl/CODATE01.cbl:L332].
    input_queue: String,
}

impl CdrdFlow {
    /// A flow that dates its replies with `clock` and serves `input_queue`,
    /// the request queue the settings name (`CARDDEMO_QUEUE_DATE_REQUEST`,
    /// default `CARD.DEMO.REQUEST.DATE`).
    #[must_use]
    pub const fn new(clock: AnyClock, input_queue: String) -> Self {
        Self { clock, input_queue }
    }
}

impl Flow for CdrdFlow {
    const NAME: &'static str = "CDRD";

    type AfterAck = ();

    /// CODATE01 3000-GET-REQUEST / 4000-PROCESS-REQUEST-REPLY / 4100-PUT-REPLY —
    /// P-CODATE01-012, P-CODATE01-013, P-CODATE01-014, P-CODATE01-020,
    /// P-CODATE01-021.
    ///
    /// A request longer than the 1000-byte `MQ-BUFFER` fails its get
    /// [app/app-vsam-mq/cbl/CODATE01.cbl:L291-335]: it gets no reply but the
    /// `INP MQGET ERR:` record for the input queue, which the consumer
    /// publishes to `CARD.DEMO.ERROR` before it rejects the request without
    /// requeue. IBM MQ would leave such a message on its queue; the consumer
    /// has no settlement that keeps a request without a reply, and requeues
    /// none.
    ///
    /// Every other request is answered with the reply
    /// `4000-PROCESS-REQUEST-REPLY` builds at the clock's instant
    /// [app/app-vsam-mq/cbl/CODATE01.cbl:L339-364]: `SYSTEM DATE : ` with
    /// `mm-dd-yyyy`, then `SYSTEM TIME : ` with `hh:mm:ss`, space-filled to the
    /// 1000 bytes `4100-PUT-REPLY` puts
    /// [app/app-vsam-mq/cbl/CODATE01.cbl:L371-372]. The domain rule pads the
    /// body; it is sent unchanged. Only the length of `req.body` is checked,
    /// because the paragraph reads none of the request.
    ///
    /// The reply goes to [`REPLY_QUEUE`] and echoes the request's
    /// `message_id` (`MQMD-MSGID`) and `correlation_id` (`MQMD-CORRELID`)
    /// [app/app-vsam-mq/cbl/CODATE01.cbl:L373-374]. The request's `reply_to`
    /// is ignored. Nothing is awaited, so the future is ready at once.
    fn handle<'a>(
        &'a self,
        req: &'a Request,
    ) -> impl Future<Output = Disposition<Self::AfterAck>> + Send + 'a {
        let disposition = match mq_date_reply::oversize_request_record(&req.body, &self.input_queue)
        {
            Some(spec) => Disposition::ErrorRecord {
                record: error_queue::record_bytes(&spec),
            },
            None => Disposition::Reply {
                reply: Reply {
                    queue: REPLY_QUEUE.to_owned(),
                    body: mq_date_reply::reply_body(self.clock.now()).into_bytes(),
                    message_id: req.message_id.clone(),
                    correlation_id: req.correlation_id.clone(),
                },
                after_ack: (),
            },
        };
        future::ready(disposition)
    }

    /// CODATE01 4100-PUT-REPLY `WHEN OTHER` `'MQPUT ERR'` — P-CODATE01-015,
    /// P-CODATE01-016, P-CODATE01-019.
    ///
    /// The `MQ-ERR-DISPLAY` record of a reply put that failed
    /// [app/app-vsam-mq/cbl/CODATE01.cbl:L396-402]: return message
    /// `MQPUT ERR`, the condition and reason codes `failure` maps to (E-39) and
    /// [`REPLY_QUEUE`] as the queue name, laid out in the 1000-byte message
    /// `9000-ERROR` puts. The consumer publishes it to `CARD.DEMO.ERROR` and
    /// then rejects the request without requeue.
    fn on_reply_failure(&self, _req: &Request, failure: &PublishFailure) -> FailureAction {
        let (condition_code, reason_code) = failure.mq_codes();
        FailureAction::ErrorRecord(error_queue::record_bytes(
            &mq_date_reply::put_reply_failure(condition_code, reason_code, REPLY_QUEUE),
        ))
    }

    /// CODATE01 4000-MAIN-PROCESS — P-CODATE01-024.
    ///
    /// Nothing follows a confirmed reply: the `SYNCPOINT` that commits the get
    /// and the put is the consumer's acknowledgement, and `CODATE01` writes
    /// nothing else.
    fn after_ack<'a>(
        &'a self,
        _req: &'a Request,
        _work: Self::AfterAck,
    ) -> impl Future<Output = ()> + Send + 'a {
        future::ready(())
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use carddemo_domain::clock::{AnyClock, Clock};
    use carddemo_domain::messages;
    use carddemo_domain::rules::mq_date_reply;

    use super::{CdrdFlow, REPLY_QUEUE};
    use crate::consumer::{Disposition, FailureAction, Flow, PublishFailure, Reply, Request};
    use crate::error_queue;

    /// The section F pin used throughout the parity tests.
    const PINNED: &str = "2022-07-01T12:00:00Z";

    /// `INPUT-QUEUE-NAME`, the request queue's local default.
    const INPUT_QUEUE: &str = "CARD.DEMO.REQUEST.DATE";

    fn pinned_clock() -> AnyClock {
        AnyClock::from_override(Some(PINNED)).expect("the pin is RFC 3339")
    }

    /// Built from the public fields rather than `Request::from_parts`, whose
    /// broker-properties argument would tie this flow's tests to the AMQP
    /// client types the flow itself never names.
    fn request(body: &[u8]) -> Request {
        Request {
            body: body.to_vec(),
            message_id: Some("cdrd-msg-0001".to_owned()),
            correlation_id: Some("cdrd-corr-0001".to_owned()),
            reply_to: Some("CARD.DEMO.SOMEWHERE.ELSE".to_owned()),
        }
    }

    /// Polls `future` once with a no-op waker. The CDRD futures await nothing,
    /// so a pending result is a failure.
    fn ready_now<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        match pin!(future).poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("the CDRD future must complete on its first poll"),
        }
    }

    fn reply_of(flow: &CdrdFlow, req: &Request) -> Reply {
        let Disposition::Reply {
            reply,
            after_ack: (),
        } = ready_now(flow.handle(req))
        else {
            panic!("CDRD answers every request with a reply");
        };
        reply
    }

    /// `true` when `text` has a digit wherever `pattern` has `d` and the same
    /// byte everywhere else.
    fn matches_shape(text: &[u8], pattern: &[u8]) -> bool {
        text.len() == pattern.len()
            && text.iter().zip(pattern).all(|(byte, shape)| {
                if *shape == b'd' {
                    byte.is_ascii_digit()
                } else {
                    byte == shape
                }
            })
    }

    /// P-CODATE01-013, P-CODATE01-020, P-CODATE01-021: labels at 0..14 and
    /// 24..38, the date at 1-based bytes 15-24, the time at 39-46, then spaces
    /// to 1000 bytes. Returns the date and time texts.
    fn assert_reply_format(body: &[u8]) -> (String, String) {
        assert_eq!(body.len(), 1000);
        assert_eq!(&body[..14], b"SYSTEM DATE : ");
        assert!(matches_shape(&body[14..24], b"dd-dd-dddd"), "{body:?}");
        assert_eq!(&body[24..38], b"SYSTEM TIME : ");
        assert!(matches_shape(&body[38..46], b"dd:dd:dd"), "{body:?}");
        assert!(body[46..].iter().all(|byte| *byte == b' '));
        (
            String::from_utf8(body[14..24].to_vec()).expect("ASCII date"),
            String::from_utf8(body[38..46].to_vec()).expect("ASCII time"),
        )
    }

    #[test]
    fn names_the_cobol_transaction_and_reply_queue() {
        // P-CODATE01-010, P-CODATE01-023: the literal moved to REPLY-QUEUE-NAME.
        assert_eq!(CdrdFlow::NAME, "CDRD");
        assert_eq!(REPLY_QUEUE, "CARD.DEMO.REPLY.DATE");
    }

    #[test]
    fn replies_to_the_fixed_queue_with_the_request_ids_and_the_domain_body() {
        // P-CODATE01-013, P-CODATE01-014: the domain body unchanged, the IDs
        // echoed, the request's reply_to ignored.
        let clock = pinned_clock();
        let flow = CdrdFlow::new(clock, INPUT_QUEUE.to_owned());
        let req = request(b"DATE");
        let reply = reply_of(&flow, &req);
        assert_eq!(reply.queue, REPLY_QUEUE);
        assert_ne!(Some(reply.queue.as_str()), req.reply_to.as_deref());
        assert_eq!(reply.message_id, req.message_id);
        assert_eq!(reply.correlation_id, req.correlation_id);
        assert_eq!(reply.body.len(), mq_date_reply::REPLY_BUFFER_LEN);
        assert_eq!(
            reply.body,
            mq_date_reply::reply_body(clock.now()).into_bytes()
        );
        let (date, time) = assert_reply_format(&reply.body);
        assert_eq!(date, "07-01-2022");
        assert_eq!(time, "12:00:00");
    }

    #[test]
    fn the_request_body_is_never_read() {
        // P-CODATE01-012: the body is ignored, so any body gets the same reply.
        let flow = CdrdFlow::new(pinned_clock(), INPUT_QUEUE.to_owned());
        let expected = reply_of(&flow, &request(b"DATE"));
        for body in [&b""[..], b"INQA00000000001", &[0xFF; 1000][..]] {
            assert_eq!(reply_of(&flow, &request(body)), expected, "{body:?}");
        }
    }

    #[test]
    fn absent_ids_stay_absent_in_the_reply() {
        // P-CODATE01-014: the descriptor IDs are echoed as received.
        let flow = CdrdFlow::new(pinned_clock(), INPUT_QUEUE.to_owned());
        let bare = Request {
            body: Vec::new(),
            message_id: None,
            correlation_id: None,
            reply_to: None,
        };
        let reply = reply_of(&flow, &bare);
        assert_eq!(reply.message_id, None);
        assert_eq!(reply.correlation_id, None);
        assert_eq!(reply.queue, REPLY_QUEUE);
    }

    #[test]
    fn the_system_clock_reply_has_the_audited_format() {
        // P-CODATE01-020, P-CODATE01-021: time fields compared by format.
        let flow = CdrdFlow::new(
            AnyClock::from_override(None).expect("no override"),
            INPUT_QUEUE.to_owned(),
        );
        assert_reply_format(&reply_of(&flow, &request(b"DATE")).body);
    }

    #[test]
    fn an_unconfirmed_reply_becomes_the_mqput_err_record() {
        // P-CODATE01-015, P-CODATE01-016, P-CODATE01-019: MQPUT ERR, the codes
        // of the publish failure (E-39) and REPLY-QUEUE-NAME, in 1000 bytes.
        let flow = CdrdFlow::new(pinned_clock(), INPUT_QUEUE.to_owned());
        let req = request(b"DATE");
        for failure in [
            PublishFailure::Returned,
            PublishFailure::Nacked,
            PublishFailure::NotRequested,
        ] {
            let (condition_code, reason_code) = failure.mq_codes();
            let FailureAction::ErrorRecord(record) = flow.on_reply_failure(&req, &failure) else {
                panic!("CDRD publishes an error record for {failure:?}");
            };
            assert_eq!(record.len(), 1000, "{failure:?}");
            assert_eq!(
                record,
                error_queue::record_bytes(&mq_date_reply::put_reply_failure(
                    condition_code,
                    reason_code,
                    REPLY_QUEUE
                )),
                "{failure:?}"
            );
            assert_eq!(
                &record[error_queue::RETURN_MESSAGE],
                format!("{:<25}", messages::MQPUT_ERR).as_bytes()
            );
            assert_eq!(
                &record[error_queue::QUEUE_NAME],
                format!("{:<48}", "CARD.DEMO.REPLY.DATE").as_bytes()
            );
            assert_eq!(
                &record[error_queue::CONDITION_CODE],
                format!("{condition_code:02}").as_bytes()
            );
            assert_eq!(
                &record[error_queue::REASON_CODE],
                format!("{reason_code:05}").as_bytes()
            );
            assert!(record[error_queue::ERROR_PARA].iter().all(|b| *b == b' '));
            assert!(record[113..].iter().all(|byte| *byte == b' '));
            assert!(error_queue::display_text(&record).contains(messages::MQPUT_ERR));
        }
    }

    #[test]
    fn a_request_longer_than_the_get_buffer_gets_the_inp_mqget_err_record() {
        // P-CODATE01-012: MQ-BUFFER-LENGTH 1000 without ACCEPT-TRUNCATED-MSG,
        // so the get fails (MQCC_WARNING 1, MQRC_TRUNCATED_MSG_FAILED 2080) and
        // 'INP MQGET ERR:' names INPUT-QUEUE-NAME; no reply is put.
        let flow = CdrdFlow::new(pinned_clock(), INPUT_QUEUE.to_owned());
        let Disposition::ErrorRecord { record } = ready_now(flow.handle(&request(&[b'D'; 1001])))
        else {
            panic!("a request longer than 1000 bytes gets no reply");
        };
        assert_eq!(record.len(), error_queue::ERROR_BUFFER_LEN);
        assert_eq!(
            record,
            error_queue::record_bytes(&mq_date_reply::get_request_failure(1, 2080, INPUT_QUEUE))
        );
        assert_eq!(
            &record[error_queue::RETURN_MESSAGE],
            format!("{:<25}", messages::INP_MQGET_ERR).as_bytes()
        );
        assert_eq!(&record[error_queue::CONDITION_CODE], b"01");
        assert_eq!(&record[error_queue::REASON_CODE], b"02080");
        assert_eq!(
            &record[error_queue::QUEUE_NAME],
            format!("{INPUT_QUEUE:<48}").as_bytes()
        );
        assert!(record[error_queue::ERROR_PARA].iter().all(|b| *b == b' '));
        assert!(record[113..].iter().all(|byte| *byte == b' '));
        assert_eq!(reply_of(&flow, &request(&[b'D'; 1000])).queue, REPLY_QUEUE);
    }

    #[test]
    fn after_ack_completes_at_once() {
        // P-CODATE01-024: no work follows a confirmed reply.
        let flow = CdrdFlow::new(pinned_clock(), INPUT_QUEUE.to_owned());
        let req = request(b"DATE");
        ready_now(flow.after_ack(&req, ()));
    }
}
