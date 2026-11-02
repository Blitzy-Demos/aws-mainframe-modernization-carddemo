//! CDRA: the account inquiry of `COACCT01`.
//!
//! IBM MQ starts `COACCT01` by trigger on its request queue. It reads each
//! request's function and account key, reads the account file `ACCTDAT` for an
//! `INQA` inquiry, and puts the reply to the fixed queue `CARD.DEMO.REPLY.ACCT`
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L198]. Here [`CdraFlow`] is served by the
//! long-running consumer of [`crate::consumer::run`] on the request queue the
//! settings name (`CARDDEMO_QUEUE_ACCT_REQUEST`), and it decides only what to
//! send:
//!
//! - every result comes from `carddemo_domain::rules::mq_account_reply`: the
//!   account response, the not-found and invalid-request texts, or the error
//!   record of a failed account read (P-COACCT01-011 to P-COACCT01-016);
//! - the account is read through `carddemo-persistence`, in a unit of work
//!   that only reads `accounts` and is rolled back once the read is done;
//! - a reply goes to [`REPLY_QUEUE`] with the request's `message_id` and
//!   `correlation_id`; the request's `reply_to` is ignored, as the COBOL never
//!   reads its saved `REPLYTOQ` (P-COACCT01-010, P-COACCT01-017);
//! - a failed account read gets no reply: its `MQ-ERR-DISPLAY` record goes to
//!   `CARD.DEMO.ERROR` and the request is rejected without requeue, the outcome
//!   the COBOL commits through `8000-TERMINATION` (P-COACCT01-015,
//!   P-COACCT01-018, P-COACCT01-032);
//! - a reply the broker does not confirm turns into the `MQPUT ERR` record,
//!   published the same way (P-COACCT01-017).
//!
//! Publishing, confirming, acknowledging and rejecting all belong to the
//! consumer. The `MQOPEN` and `MQCLOSE` failure paths of `COACCT01` have no
//! counterpart, because the consumer opens and closes no queue per request:
//! they are not reproduced (E-22; P-COACCT01-019, P-COACCT01-029,
//! P-COACCT01-030, P-COACCT01-031). No identity is checked: the request carries
//! no credential, and its descriptor fields serve routing and correlation only.

use std::future::{self, Future};

use carddemo_domain::entities::Account;
use carddemo_domain::numeric::AccountId;
use carddemo_domain::ports::{AccountReader, PortError, PortResult};
use carddemo_domain::rules::mq_account_reply::{self, ReplyOutcome};
use carddemo_persistence::{Db, UnitOfWork};

use crate::consumer::{Disposition, FailureAction, Flow, PublishFailure, Reply, Request};
use crate::error_queue;

/// COACCT01 1000-CONTROL / 2400-OPEN-OUTPUT-QUEUE — P-COACCT01-010,
/// P-COACCT01-031.
///
/// The reply queue: `MOVE 'CARD.DEMO.REPLY.ACCT' TO REPLY-QUEUE-NAME`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L198]. The COBOL names it as a literal,
/// so it is a constant here and not a setting. `infra/rabbitmq/definitions.json`
/// declares it as a durable queue.
pub const REPLY_QUEUE: &str = "CARD.DEMO.REPLY.ACCT";

/// The CDRA flow: the `COACCT01` rules adapted to [`Flow`].
#[derive(Debug, Clone)]
pub struct CdraFlow {
    /// The pool each request's read-only unit of work is begun on.
    db: Db,
    /// `INPUT-QUEUE-NAME`, the queue the request came from, which `COACCT01`
    /// takes from `MQTM-QNAME` [app/app-vsam-mq/cbl/COACCT01.cbl:L197]. The
    /// read-error record names it [app/app-vsam-mq/cbl/COACCT01.cbl:L441].
    input_queue: String,
}

impl CdraFlow {
    /// A flow that reads accounts through `db` and serves `input_queue`, the
    /// request queue the settings name (`CARDDEMO_QUEUE_ACCT_REQUEST`, default
    /// `CARD.DEMO.REQUEST.ACCT`).
    #[must_use]
    pub fn new(db: Db, input_queue: String) -> Self {
        Self { db, input_queue }
    }
}

impl Flow for CdraFlow {
    const NAME: &'static str = "CDRA";

    type AfterAck = ();

    /// COACCT01 3000-GET-REQUEST / 4000-PROCESS-REQUEST-REPLY — P-COACCT01-011
    /// to P-COACCT01-016.
    ///
    /// Copies the function and key out of the request (P-COACCT01-011), lets
    /// [`mq_account_reply::process`] read the account through an
    /// [`AccountFile`] and decide the outcome
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L390-460], closes the file, and
    /// adapts the outcome to a [`Disposition`]. Only an `INQA` inquiry with a
    /// key above zero reads the file (P-COACCT01-012); every other request is
    /// answered without touching the database (P-COACCT01-016).
    async fn handle(&self, req: &Request) -> Disposition<()> {
        let request = mq_account_reply::parse_request(&req.body);
        let mut file = AccountFile::new(&self.db);
        let outcome = mq_account_reply::process(&mut file, &request, &self.input_queue).await;
        file.close().await;
        map_outcome(outcome, req)
    }

    /// COACCT01 4100-PUT-REPLY `WHEN OTHER` `'MQPUT ERR'` — P-COACCT01-017,
    /// P-COACCT01-018.
    ///
    /// The `MQ-ERR-DISPLAY` record of a reply put that failed
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L492-498], as [`reply_failure_record`]
    /// lays it out. The consumer publishes it to `CARD.DEMO.ERROR` and then
    /// rejects the request without requeue.
    fn on_reply_failure(&self, _req: &Request, failure: &PublishFailure) -> FailureAction {
        FailureAction::ErrorRecord(reply_failure_record(failure))
    }

    /// COACCT01 4000-MAIN-PROCESS — P-COACCT01-032.
    ///
    /// Nothing follows a confirmed reply: the `SYNCPOINT` that commits the get
    /// and the put is the consumer's acknowledgement, and `COACCT01` writes
    /// nothing.
    fn after_ack<'a>(
        &'a self,
        _req: &'a Request,
        _work: Self::AfterAck,
    ) -> impl Future<Output = ()> + Send + 'a {
        future::ready(())
    }
}

/// `ACCTDAT` as one CDRA request reaches it: a read-only unit of work begun on
/// the first read and rolled back by [`AccountFile::close`].
///
/// Beginning on demand keeps the database out of every request that reads no
/// account. `COACCT01` reads `ACCTDAT` only for an `INQA` inquiry with a key
/// above zero [app/app-vsam-mq/cbl/COACCT01.cbl:L393-404] (P-COACCT01-012),
/// so its invalid-request reply (P-COACCT01-016) is answered even when the
/// database cannot be reached.
///
/// The read is the program's only file access, so a unit of work that cannot
/// be begun fails the read like any other non-`NOTFND` failure: the domain
/// rule turns it into the `ERROR WHILE READING ACCTFILE` record and no reply
/// (P-COACCT01-015).
struct AccountFile<'d> {
    /// The pool the unit of work is begun on.
    db: &'d Db,
    /// The unit of work, once the first read has begun it.
    uow: Option<UnitOfWork>,
}

impl<'d> AccountFile<'d> {
    /// The file of one request, not yet opened.
    fn new(db: &'d Db) -> Self {
        Self { db, uow: None }
    }

    /// The unit of work every read runs in, begun by the first call.
    ///
    /// # Errors
    ///
    /// The [`PortError`] mapping of the `Db::begin` failure, in practice
    /// [`PortError::Unavailable`]: no connection, or a failed `BEGIN`.
    async fn unit_of_work(&mut self) -> PortResult<&mut UnitOfWork> {
        let uow = match self.uow.take() {
            Some(uow) => uow,
            None => self.db.begin().await.map_err(|err| {
                tracing::warn!(error = %err, "account unit of work could not begin");
                PortError::from(err)
            })?,
        };
        Ok(self.uow.insert(uow))
    }

    /// Ends the unit of work, if one was begun. It only read, so it is rolled
    /// back; a rollback that fails is logged and changes no outcome, because
    /// nothing was written.
    async fn close(self) {
        if let Some(uow) = self.uow
            && let Err(err) = uow.rollback().await
        {
            tracing::warn!(error = %err, "read-only account unit of work did not roll back");
        }
    }
}

/// Logs a read failure other than `NOTFND`. The domain rule keeps only its
/// outcome, the error record, so the adapter detail is written here.
fn log_read_failure<T>(result: PortResult<T>) -> PortResult<T> {
    if let Err(err) = &result
        && !matches!(err, PortError::NotFound)
    {
        tracing::warn!(error = %err, "account read failed other than NOTFND");
    }
    result
}

impl AccountReader for AccountFile<'_> {
    /// COACCT01 4000-PROCESS-REQUEST-REPLY `EXEC CICS READ DATASET(ACCTDAT)`
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L396-404] — P-COACCT01-012.
    async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
        let uow = self.unit_of_work().await?;
        log_read_failure(uow.store().read_account(id).await)
    }

    /// Never called by `COACCT01`, which reads without `UPDATE`; it reads the
    /// same file in the same unit of work so the port stays complete.
    async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
        let uow = self.unit_of_work().await?;
        log_read_failure(uow.store().read_account_for_update(id).await)
    }
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY / 4100-PUT-REPLY / 9000-ERROR —
/// P-COACCT01-013 to P-COACCT01-018.
///
/// The disposition of `outcome` for `req`:
///
/// - [`ReplyOutcome::Reply`]: the reply text goes to [`REPLY_QUEUE`] exactly as
///   the domain padded it to the 1000 bytes of `MQ-BUFFER-LENGTH`
///   [app/app-vsam-mq/cbl/COACCT01.cbl:L467-468], with the request's
///   `message_id` and `correlation_id` echoed as `SAVE-MSGID` and
///   `SAVE-CORELID` [app/app-vsam-mq/cbl/COACCT01.cbl:L469-470]. It is the
///   account response (P-COACCT01-013), the `NOTFND` text (P-COACCT01-014) or
///   the invalid-request text (P-COACCT01-016).
/// - [`ReplyOutcome::ErrorRecord`]: no reply exists. The record's 1000 bytes go
///   to `CARD.DEMO.ERROR` and the request is rejected without requeue, as
///   `9000-ERROR` puts it and `8000-TERMINATION` returns normally
///   [app/app-vsam-mq/cbl/COACCT01.cbl:L437-445]
///   [app/app-vsam-mq/cbl/COACCT01.cbl:L549] (P-COACCT01-015, P-COACCT01-018).
fn map_outcome(outcome: ReplyOutcome, req: &Request) -> Disposition<()> {
    match outcome {
        ReplyOutcome::Reply(text) => Disposition::Reply {
            reply: Reply {
                queue: REPLY_QUEUE.to_owned(),
                body: text.into_bytes(),
                message_id: req.message_id.clone(),
                correlation_id: req.correlation_id.clone(),
            },
            after_ack: (),
        },
        ReplyOutcome::ErrorRecord(spec) => Disposition::ErrorRecord {
            record: error_queue::record_bytes(&spec),
        },
    }
}

/// COACCT01 4100-PUT-REPLY `WHEN OTHER` `'MQPUT ERR'` — P-COACCT01-017,
/// P-COACCT01-018.
///
/// The 1000-byte error message for a reply `failure`: return message
/// `MQPUT ERR`, the condition and reason codes `failure` maps to (E-39) and
/// [`REPLY_QUEUE`] as `REPLY-QUEUE-NAME`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L493-496], laid out by
/// [`error_queue::record_bytes`].
fn reply_failure_record(failure: &PublishFailure) -> Vec<u8> {
    let (condition_code, reason_code) = failure.mq_codes();
    error_queue::record_bytes(&mq_account_reply::put_reply_failure(
        condition_code,
        reason_code,
        REPLY_QUEUE,
    ))
}

#[cfg(test)]
mod tests {
    use carddemo_domain::format::{self, REPLY_BUFFER_LEN};
    use carddemo_domain::messages;
    use carddemo_domain::rules::mq_account_reply::{self, ReplyOutcome};

    use super::{CdraFlow, REPLY_QUEUE, map_outcome, reply_failure_record};
    use crate::consumer::{Disposition, Flow, PublishFailure, Reply, Request};
    use crate::error_queue;

    /// `INPUT-QUEUE-NAME`, the request queue's local default.
    const INPUT_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";

    /// The broker definitions that declare the reply queue.
    const DEFINITIONS: &str = include_str!("../../../../infra/rabbitmq/definitions.json");

    /// Built from the public fields rather than `Request::from_parts`, whose
    /// broker-properties argument would tie this flow's tests to the AMQP
    /// client types the flow itself never names.
    fn request(body: &[u8]) -> Request {
        Request {
            body: body.to_vec(),
            message_id: Some("cdra-msg-0001".to_owned()),
            correlation_id: Some("cdra-corr-0001".to_owned()),
            reply_to: Some("CARD.DEMO.SOMEWHERE.ELSE".to_owned()),
        }
    }

    fn reply_of(disposition: Disposition<()>) -> Reply {
        let Disposition::Reply {
            reply,
            after_ack: (),
        } = disposition
        else {
            panic!("a reply outcome is answered with a reply");
        };
        reply
    }

    fn record_of(disposition: Disposition<()>) -> Vec<u8> {
        let Disposition::ErrorRecord { record } = disposition else {
            panic!("an error-record outcome gets no reply");
        };
        record
    }

    /// `text` moved into an `X(n)` field `width` bytes wide.
    fn field(text: &str, width: usize) -> Vec<u8> {
        format!("{text:<width$}").into_bytes()
    }

    #[test]
    fn names_the_cobol_transaction_and_the_declared_reply_queue() {
        // P-COACCT01-010, P-COACCT01-031: the literal moved to REPLY-QUEUE-NAME,
        // a durable queue of the broker definitions.
        assert_eq!(CdraFlow::NAME, "CDRA");
        assert_eq!(REPLY_QUEUE, "CARD.DEMO.REPLY.ACCT");
        let declaration = DEFINITIONS
            .split(&format!("\"name\": \"{REPLY_QUEUE}\""))
            .nth(1)
            .and_then(|rest| rest.split('}').next())
            .expect("definitions.json declares the reply queue");
        assert!(declaration.contains("\"durable\": true"), "{declaration}");
    }

    #[test]
    fn replies_go_to_the_fixed_queue_with_the_domain_text_and_the_request_ids() {
        // P-COACCT01-013, P-COACCT01-014, P-COACCT01-016, P-COACCT01-017: the
        // domain's 1000-byte text unchanged, MSGID and CORRELID echoed, the
        // request's reply_to ignored.
        let texts = [
            format::pad_reply_1000(&messages::cdra_account_not_found_reply("99999999999")),
            format::pad_reply_1000(&messages::cdra_invalid_request_reply("00000000001", "INQX")),
            format::pad_reply_1000(
                "ACCOUNT ID : 00000000001ACCOUNT STATUS : YBALANCE : 00000001940{",
            ),
        ];
        let req = request(b"INQA99999999999");
        for text in texts {
            assert_eq!(text.len(), REPLY_BUFFER_LEN);
            let reply = reply_of(map_outcome(ReplyOutcome::Reply(text.clone()), &req));
            assert_eq!(reply.queue, REPLY_QUEUE);
            assert_ne!(Some(reply.queue.as_str()), req.reply_to.as_deref());
            assert_eq!(reply.message_id, req.message_id);
            assert_eq!(reply.correlation_id, req.correlation_id);
            assert_eq!(reply.body, text.into_bytes());
        }
    }

    #[test]
    fn absent_ids_stay_absent_in_the_reply() {
        // P-COACCT01-017: the descriptor IDs are echoed as received.
        let bare = Request {
            body: Vec::new(),
            message_id: None,
            correlation_id: None,
            reply_to: None,
        };
        let text = format::pad_reply_1000(&messages::cdra_invalid_request_reply("", ""));
        let reply = reply_of(map_outcome(ReplyOutcome::Reply(text), &bare));
        assert_eq!(reply.message_id, None);
        assert_eq!(reply.correlation_id, None);
        assert_eq!(reply.queue, REPLY_QUEUE);
    }

    #[test]
    fn a_failed_account_read_becomes_its_error_record_and_no_reply() {
        // P-COACCT01-015, P-COACCT01-018: 'ERROR WHILE READING ACCTFILE' cut to
        // X(25), INPUT-QUEUE-NAME, codes 00 and 00000 (E-39), in 1000 bytes.
        let spec = mq_account_reply::read_error_record(INPUT_QUEUE);
        let record = record_of(map_outcome(
            ReplyOutcome::ErrorRecord(spec.clone()),
            &request(b"INQA00000000001"),
        ));
        assert_eq!(record.len(), error_queue::ERROR_BUFFER_LEN);
        assert_eq!(record, error_queue::record_bytes(&spec));
        assert_eq!(
            &record[error_queue::RETURN_MESSAGE],
            b"ERROR WHILE READING ACCTF"
        );
        assert_eq!(&record[error_queue::QUEUE_NAME], field(INPUT_QUEUE, 48));
        assert_eq!(&record[error_queue::CONDITION_CODE], b"00");
        assert_eq!(&record[error_queue::REASON_CODE], b"00000");
        assert!(record[error_queue::ERROR_PARA].iter().all(|b| *b == b' '));
        assert!(
            record[error_queue::ERR_DISPLAY_LEN..]
                .iter()
                .all(|b| *b == b' ')
        );
    }

    #[test]
    fn an_unconfirmed_reply_becomes_the_mqput_err_record() {
        // P-COACCT01-017, P-COACCT01-018: MQPUT ERR, the codes of the publish
        // failure (E-39) and REPLY-QUEUE-NAME, in 1000 bytes.
        for failure in [
            PublishFailure::Returned,
            PublishFailure::Nacked,
            PublishFailure::NotRequested,
        ] {
            let (condition_code, reason_code) = failure.mq_codes();
            let record = reply_failure_record(&failure);
            assert_eq!(record.len(), error_queue::ERROR_BUFFER_LEN, "{failure:?}");
            assert_eq!(
                record,
                error_queue::record_bytes(&mq_account_reply::put_reply_failure(
                    condition_code,
                    reason_code,
                    REPLY_QUEUE
                )),
                "{failure:?}"
            );
            assert_eq!(&record[error_queue::RETURN_MESSAGE], field("MQPUT ERR", 25));
            assert_eq!(
                &record[error_queue::QUEUE_NAME],
                field("CARD.DEMO.REPLY.ACCT", 48)
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
            assert!(
                record[error_queue::ERR_DISPLAY_LEN..]
                    .iter()
                    .all(|b| *b == b' ')
            );
            assert!(error_queue::display_text(&record).contains("MQPUT ERR"));
        }
    }
}
