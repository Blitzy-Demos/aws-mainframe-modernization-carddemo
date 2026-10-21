//! The `MQ-ERR-DISPLAY` group the CDRD and CDRA workers fill when a broker put fails, and the
//! `CARD.DEMO.ERROR` body made from it (AAP 0.3.4.2).
//!
//! `CODATE01` and `COACCT01` fill `MQ-ERR-DISPLAY` with `MQPUT ERR` at two put sites, which
//! this module maps to the domain call that renders them. It owns no layout: the group, its
//! padding to 1,000 bytes and its literals are `domain::messaging::codate01`'s, which
//! `COACCT01` shares (`COACCT01.cbl:58-67`).
//!
//! | Site | COBOL | Queue name in the group | What the worker does with it | Rules |
//! |---|---|---|---|---|
//! | [`Site::ReplyPut`] | `4100-PUT-REPLY` `WHEN OTHER`, then `PERFORM 9000-ERROR` (`CODATE01.cbl:396-409`, `COACCT01.cbl:492-505`) | the flow's reply queue | publishes [`render`]'s body to `CARD.DEMO.ERROR`, waits for the confirmation, then acknowledges | CODATE01-014, CODATE01-015, CODATE01-018, COACCT01-029, COACCT01-032, COACCT01-033 |
//! | [`Site::ErrorPut`] | `9000-ERROR` `WHEN OTHER`, then `DISPLAY MQ-ERR-DISPLAY` (`CODATE01.cbl:433-438`, `COACCT01.cbl:529-534`) | `CARD.DEMO.ERROR` | publishes nothing: the group's return message and queue name go into the `ERROR` tracing event that stands in for the `DISPLAY` | CODATE01-019, COACCT01-034 |
//!
//! Not here:
//!
//! - The CDRA account-read error (`ERROR WHILE READING ACCTF`, COACCT01-028, E18) is a ready
//!   body that `domain::messaging::coacct01::respond` builds and `workers::cdra` publishes.
//! - The `INP MQGET ERR:` get failure has no site: neither audit carries it over. A failed
//!   `basic_get` means the consumer channel is lost, and the worker reconnects and lets the
//!   broker redeliver the unacknowledged request instead of publishing an error body.
//! - The `MQOPEN`, `MQCLOSE` and `CICS RETRIEVE` error paths and their artefacts, the self-move
//!   of `RESP2`, the open flag set on the wrong queue and the recursive close error (E17), have
//!   no AMQP counterpart and no rule row.
//!
//! HALTED(H4): the COBOL moves the MQ completion and reason codes of the failed `MQPUT` into
//! `MQ-APPL-CONDITION-CODE` and `MQ-APPL-REASON-CODE` (`CODATE01.cbl:397-398,434-435`). IBM MQ
//! defines them and AMQP has no counterpart (CODATE01-016, CODATE01-017, COACCT01-030,
//! COACCT01-031). This module invents no code: the fields keep the `INITIALIZE` zeros that
//! `MqErrDisplay::for_failure` leaves, and their digits are never a parity result.
//!
//! Every group is built afresh from its flow and site, never from the request: no request byte
//! reaches an error body, so it holds only literals, queue names, code digits and spaces
//! (CODATE01-009, COACCT01-035; Cardholder Data Redaction).

use domain::messaging::coacct01;
use domain::messaging::codate01::{self, BUFFER_LEN, MqErrDisplay, ReturnMessage};

/// The worker whose put failed. Its program fixes the reply queue the error group names; the two
/// programs declare the same group and the same `MQPUT ERR` literal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Flow {
    /// CDRD, the port of `CODATE01`.
    Cdrd,
    /// CDRA, the port of `COACCT01`.
    Cdra,
}

impl Flow {
    /// The program's reply queue, the literal moved into `REPLY-QUEUE-NAME`: for CDRD
    /// `CARD.DEMO.REPLY.DATE` (`CODATE01.cbl:147`; CODATE01-002) and for CDRA
    /// `CARD.DEMO.REPLY.ACCT` (`COACCT01.cbl:198`; COACCT01-023).
    #[must_use]
    pub const fn reply_queue(self) -> &'static str {
        match self {
            Self::Cdrd => codate01::REPLY_QUEUE,
            Self::Cdra => coacct01::REPLY_QUEUE,
        }
    }

    /// The program's error queue, `CARD.DEMO.ERROR`, the literal moved into `ERROR-QUEUE-NAME`
    /// (`CODATE01.cbl:243`, `COACCT01.cbl:294`; CODATE01-018, COACCT01-032).
    #[must_use]
    pub const fn error_queue(self) -> &'static str {
        match self {
            Self::Cdrd => codate01::ERROR_QUEUE,
            Self::Cdra => coacct01::ERROR_QUEUE,
        }
    }
}

/// The put whose failure fills `MQ-ERR-DISPLAY` with `MQPUT ERR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Site {
    /// The reply's publication fails while the consumer channel is open: a negative
    /// confirmation, a returned message or a publisher-channel error. `4100-PUT-REPLY` moves
    /// `REPLY-QUEUE-NAME` into `MQ-APPL-QUEUE-NAME` and performs `9000-ERROR`, which puts the
    /// group on `CARD.DEMO.ERROR` (CODATE01-014, COACCT01-033).
    ReplyPut,
    /// The error message's own publication fails too. `9000-ERROR` moves `ERROR-QUEUE-NAME`
    /// into `MQ-APPL-QUEUE-NAME` and `DISPLAY`s the group; nothing is published
    /// (CODATE01-019, COACCT01-034).
    ErrorPut,
}

impl Site {
    /// The queue whose put failed, which the site moves into `MQ-APPL-QUEUE-NAME`: the flow's
    /// reply queue for [`Site::ReplyPut`], its error queue for [`Site::ErrorPut`].
    #[must_use]
    pub const fn queue(self, flow: Flow) -> &'static str {
        match self {
            Self::ReplyPut => flow.reply_queue(),
            Self::ErrorPut => flow.error_queue(),
        }
    }
}

/// `MQ-ERR-DISPLAY` as `flow`'s program fills it at `site`: `MQ-APPL-RETURN-MESSAGE` `MQPUT
/// ERR`, `MQ-APPL-QUEUE-NAME` the site's queue, `MQ-ERROR-PARA` spaces and the code fields
/// zeros (CODATE01-015, COACCT01-029; the codes HALTED(H4)).
///
/// For [`Site::ErrorPut`] the worker's tracing event carries this group's return message and
/// queue name (CODATE01-019, COACCT01-034).
#[must_use]
pub fn mq_err_display(flow: Flow, site: Site) -> MqErrDisplay {
    MqErrDisplay::for_failure(ReturnMessage::MqPutErr, site.queue(flow))
}

/// The 1,000-byte body published to `CARD.DEMO.ERROR` after `flow`'s reply put fails: the
/// [`Site::ReplyPut`] group moved into `ERROR-MESSAGE` and padded with 887 spaces
/// (`CODATE01.cbl:409-411`, `COACCT01.cbl:505-507`; CODATE01-015, COACCT01-029). The worker
/// publishes it with the request's `message_id` and `correlation_id` (CODATE01-018,
/// COACCT01-032).
#[must_use]
pub fn render(flow: Flow) -> [u8; BUFFER_LEN] {
    mq_err_display(flow, Site::ReplyPut).error_body()
}

#[cfg(test)]
mod tests {
    use copybook::PicU;
    use domain::messaging::codate01::{
        BUFFER_LEN, MQ_ERR_DISPLAY_LEN, MqErrDisplay, ReturnMessage,
    };

    use super::{Flow, Site, mq_err_display, render};

    type TestResult = Result<(), &'static str>;

    const FLOWS: [Flow; 2] = [Flow::Cdrd, Flow::Cdra];

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
    fn padded(text: &str, width: usize) -> Vec<u8> {
        let mut field = text.as_bytes().to_vec();
        field.resize(width, b' ');
        field
    }

    /// Every byte of a reply-put error body naming `queue`, except the code fields at offsets
    /// 54-55 and 58-62, which are HALTED(H4) and asserted nowhere.
    fn assert_reply_put_body(body: &[u8], queue: &str) -> TestResult {
        assert_eq!(body.len(), BUFFER_LEN);
        assert!(
            all_spaces(segment(body, 0, 27)?),
            "MQ-ERROR-PARA and its FILLER"
        );
        assert_eq!(segment(body, 27, 25)?, padded("MQPUT ERR", 25));
        for (start, len) in [(52, 2), (56, 2), (63, 2)] {
            assert!(all_spaces(segment(body, start, len)?), "FILLER at {start}");
        }
        assert_eq!(segment(body, 65, 48)?, padded(queue, 48));
        let tail = body.get(MQ_ERR_DISPLAY_LEN..).ok_or("no padding")?;
        assert_eq!(tail.len(), 887);
        assert!(all_spaces(tail));
        Ok(())
    }

    // audit: CODATE01-014
    // audit: CODATE01-015
    #[test]
    fn cdrd_reply_put_body_names_the_date_reply_queue() -> TestResult {
        let body = render(Flow::Cdrd);
        let expected =
            MqErrDisplay::for_failure(ReturnMessage::MqPutErr, "CARD.DEMO.REPLY.DATE").error_body();
        assert_eq!(body, expected);
        assert_reply_put_body(&body, "CARD.DEMO.REPLY.DATE")
    }

    // audit: COACCT01-029
    // audit: COACCT01-033
    #[test]
    fn cdra_reply_put_body_names_the_account_reply_queue() -> TestResult {
        let body = render(Flow::Cdra);
        let expected =
            MqErrDisplay::for_failure(ReturnMessage::MqPutErr, "CARD.DEMO.REPLY.ACCT").error_body();
        assert_eq!(body, expected);
        assert_reply_put_body(&body, "CARD.DEMO.REPLY.ACCT")
    }

    // audit: CODATE01-002
    // audit: CODATE01-018
    // audit: COACCT01-023
    // audit: COACCT01-032
    #[test]
    fn flows_name_the_cobol_queue_literals() {
        assert_eq!(Flow::Cdrd.reply_queue(), "CARD.DEMO.REPLY.DATE");
        assert_eq!(Flow::Cdra.reply_queue(), "CARD.DEMO.REPLY.ACCT");
        for flow in FLOWS {
            assert_eq!(flow.error_queue(), "CARD.DEMO.ERROR");
            assert_eq!(Site::ReplyPut.queue(flow), flow.reply_queue());
            assert_eq!(Site::ErrorPut.queue(flow), "CARD.DEMO.ERROR");
        }
    }

    // audit: CODATE01-019
    // audit: COACCT01-034
    #[test]
    fn error_put_group_names_the_error_queue() {
        for flow in FLOWS {
            let group = mq_err_display(flow, Site::ErrorPut);
            assert!(group.error_para.is_spaces());
            assert_eq!(group.return_message.to_text(), "MQPUT ERR");
            assert_eq!(group.queue_name.to_text(), "CARD.DEMO.ERROR");
            assert_eq!(
                group,
                MqErrDisplay::for_failure(ReturnMessage::MqPutErr, "CARD.DEMO.ERROR")
            );
        }
    }

    // HALTED(H4): checks only that this module sets no code, so the fields keep the
    // `INITIALIZE` zeros; no parity value is asserted and the test carries no audit tag.
    #[test]
    fn code_fields_keep_the_initialized_zeros() {
        for flow in FLOWS {
            for site in [Site::ReplyPut, Site::ErrorPut] {
                let group = mq_err_display(flow, site);
                assert_eq!(group.condition_code, PicU::ZERO);
                assert_eq!(group.reason_code, PicU::ZERO);
            }
        }
    }
}
