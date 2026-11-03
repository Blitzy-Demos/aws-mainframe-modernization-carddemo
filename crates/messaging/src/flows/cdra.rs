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
//! - a request longer than the 1000-byte get buffer gets no reply: its get
//!   fails, and the `INP MQGET ERR:` record goes to `CARD.DEMO.ERROR` before
//!   the request is rejected without requeue (P-COACCT01-011);
//! - an `INQA` request whose key holds a byte other than a digit gets neither
//!   a reply nor a record: `WS-KEY > ZEROES` has no pinned result for it
//!   (E-28, halt), so it is logged with the escalation and rejected without
//!   requeue;
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
use std::time::Duration;

use carddemo_domain::entities::Account;
use carddemo_domain::numeric::AccountId;
use carddemo_domain::ports::{AccountReader, PortError, PortResult};
use carddemo_domain::rules::mq_account_reply::{self, AccountRequest, ReplyOutcome};
use carddemo_persistence::{Db, UnitOfWork};

use crate::connection::BROKER_TIMEOUT;
use crate::consumer::{Disposition, FailureAction, Flow, PublishFailure, Reply, Request};
use crate::db_deadline::{DB_DEADLINE, bounded};
use crate::error_queue;

/// COACCT01 1000-CONTROL / 2400-OPEN-OUTPUT-QUEUE — P-COACCT01-010,
/// P-COACCT01-031.
///
/// The reply queue: `MOVE 'CARD.DEMO.REPLY.ACCT' TO REPLY-QUEUE-NAME`
/// [app/app-vsam-mq/cbl/COACCT01.cbl:L198]. The COBOL names it as a literal,
/// so it is a constant here and not a setting. `infra/rabbitmq/definitions.json`
/// declares it as a durable queue.
pub const REPLY_QUEUE: &str = "CARD.DEMO.REPLY.ACCT";

/// COACCT01 4000-PROCESS-REQUEST-REPLY — P-COACCT01-011: the paragraph a
/// request whose key test the audit cannot pin (E-28) is rejected at,
/// recorded as the message span's `location`.
const PROCESS_PARAGRAPH: &str = "4000-PROCESS-REQUEST-REPLY";

/// The database calls one CDRA request makes through [`AccountFile`], each
/// under its own database deadline: the `BEGIN`, the account read and the
/// `ROLLBACK`.
const DB_CALLS: u32 = 3;

/// The publishes the consumer makes for one CDRA request, each held to
/// [`BROKER_TIMEOUT`]: the reply and, when it is not confirmed, the error
/// record.
const PUBLISHES: u32 = 2;

/// The longest one CDRA delivery runs with every bounded step within its
/// limit: `DB_CALLS` database deadlines and `PUBLISHES` broker timeouts. The
/// settlement after them has no limit (`crate::consumer`).
pub(crate) const DELIVERY_BUDGET: Duration = DB_DEADLINE
    .saturating_mul(DB_CALLS)
    .saturating_add(BROKER_TIMEOUT.saturating_mul(PUBLISHES));

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
    /// A request longer than the 1000-byte `MQ-BUFFER` fails its get
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L342-385]: it gets no reply but the
    /// `INP MQGET ERR:` record for the input queue, published to
    /// `CARD.DEMO.ERROR` before the request is rejected without requeue. IBM
    /// MQ would leave such a message on its queue; the consumer has no
    /// settlement that keeps a request without a reply, and requeues none.
    ///
    /// Otherwise copies the function and key out of the request
    /// (P-COACCT01-011). An `INQA` request whose key holds a byte other than a
    /// digit is held at `halt` (E-28): `WS-KEY > ZEROES` has no pinned result
    /// for it, so it is logged with the escalation and rejected without a
    /// reply or a record. Every other request goes to
    /// [`mq_account_reply::process`], which reads the account through an
    /// `AccountFile` and decides the outcome
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L390-460]; the file is closed and
    /// the outcome adapted to a [`Disposition`]. Only an `INQA` inquiry with a
    /// key above zero reads the file (P-COACCT01-012); every other request is
    /// answered without touching the database (P-COACCT01-016).
    async fn handle(&self, req: &Request) -> Disposition<()> {
        let request = match receive(req, &self.input_queue) {
            Ok(request) => request,
            Err(disposition) => return disposition,
        };
        let mut file = AccountFile::new(&self.db, req.message_id.as_deref().unwrap_or(""));
        let outcome = mq_account_reply::process(&mut file, &request, &self.input_queue).await;
        file.close().await;
        map_outcome(outcome, req)
    }

    /// COACCT01 4100-PUT-REPLY `WHEN OTHER` `'MQPUT ERR'` — P-COACCT01-017,
    /// P-COACCT01-018.
    ///
    /// The `MQ-ERR-DISPLAY` record of a reply put that failed
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L492-498], as `reply_failure_record`
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
/// (P-COACCT01-015). The record keeps only the legacy codes, so each such
/// failure is logged here at `error`, with the flow, the request's
/// `message_id`, the call, the account key and the database cause, as CP00
/// logs its `C001` record; the event also marks the message span as failed.
///
/// The `BEGIN`, the read and the `ROLLBACK` each run under the limits of
/// `crate::db_deadline`. A read the server cancels at the statement timeout,
/// or a `BEGIN` or read that outlives the database deadline, fails the read
/// the same way, so a stalled database ends in the error record rather than
/// stopping the consumer; a `ROLLBACK` that outlives the deadline is logged
/// and the unit of work is dropped, which sqlx rolls back on the connection.
/// `DB_CALLS` counts these three calls for [`DELIVERY_BUDGET`], from which the
/// shutdown drain is derived, so a call added here is counted there.
struct AccountFile<'d> {
    /// The pool the unit of work is begun on.
    db: &'d Db,
    /// The request's `message_id`, empty when it has none, for the failure
    /// events.
    message_id: &'d str,
    /// The unit of work, once the first read has begun it.
    uow: Option<UnitOfWork>,
}

impl<'d> AccountFile<'d> {
    /// The file of the request `message_id`, not yet opened.
    fn new(db: &'d Db, message_id: &'d str) -> Self {
        Self {
            db,
            message_id,
            uow: None,
        }
    }

    /// The unit of work every read runs in, begun by the first call. `id` is
    /// the account the calling read is for, named by [`log_begin_failure`]
    /// when the `BEGIN` fails.
    ///
    /// # Errors
    ///
    /// The [`PortError`] mapping of the `Db::begin` failure, in practice
    /// [`PortError::Unavailable`]: no connection, a failed `BEGIN`, or a
    /// `BEGIN` past the database deadline.
    async fn unit_of_work(&mut self, id: AccountId) -> PortResult<&mut UnitOfWork> {
        let uow = match self.uow.take() {
            Some(uow) => uow,
            None => bounded("begin", self.db.begin())
                .await
                .inspect_err(|err| log_begin_failure(self.message_id, id, err))?,
        };
        Ok(self.uow.insert(uow))
    }

    /// Ends the unit of work, if one was begun. It only read, so it is rolled
    /// back; a rollback that fails or outlives the database deadline is logged
    /// and changes no outcome, because nothing was written.
    async fn close(self) {
        if let Some(uow) = self.uow
            && let Err(err) = bounded("rollback", uow.rollback()).await
        {
            tracing::warn!(error = %err, "read-only account unit of work did not roll back");
        }
    }
}

/// COACCT01 3000-GET-REQUEST — P-COACCT01-010, P-COACCT01-011.
///
/// The function and key of `req`, or the disposition that ends it before the
/// account file is touched: the `INP MQGET ERR:` record for `input_queue` of
/// a body longer than the 1000-byte get buffer, or the rejection of an `INQA`
/// key the audit cannot pin (E-28), logged here.
fn receive(req: &Request, input_queue: &str) -> Result<AccountRequest, Disposition<()>> {
    if let Some(spec) = mq_account_reply::oversize_request_record(&req.body, input_queue) {
        return Err(Disposition::ErrorRecord {
            record: error_queue::record_bytes(&spec),
        });
    }
    let request = mq_account_reply::parse_request(&req.body);
    if let Some(escalation) = mq_account_reply::unproven_escalation(&request) {
        log_unproven(req, escalation);
        return Err(Disposition::Reject {
            location: PROCESS_PARAGRAPH,
        });
    }
    Ok(request)
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY — P-COACCT01-011: logs a request whose
/// outcome the audit cannot pin, with the escalation that holds it at `halt`
/// and the paragraph whose output is skipped for it (AAP 0.9.6).
fn log_unproven(req: &Request, escalation: &'static str) {
    tracing::error!(
        flow = CdraFlow::NAME,
        message_id = req.message_id.as_deref().unwrap_or(""),
        escalation,
        paragraph = PROCESS_PARAGRAPH,
        "COACCT01 result unproven: the paragraph's output is skipped"
    );
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY — P-COACCT01-015: logs at `error` the
/// `BEGIN` that failed before account `id` of request `message_id` was read.
/// The read fails with it, so the request ends in the error record and no
/// reply; the domain rule keeps only that outcome, so the cause is written
/// here.
fn log_begin_failure(message_id: &str, id: AccountId, err: &PortError) {
    tracing::error!(
        flow = CdraFlow::NAME,
        message_id,
        call = "begin",
        account_id = %id,
        error = %err,
        "account unit of work could not begin"
    );
}

/// COACCT01 4000-PROCESS-REQUEST-REPLY — P-COACCT01-015: logs at `error` a
/// failure other than `NOTFND` of the read `call` of account `id` for request
/// `message_id`, and returns `result`. The domain rule keeps only its
/// outcome, the error record, so the adapter detail is written here; `NOTFND`
/// is the ordinary outcome the COBOL branches on and is not logged.
fn log_read_failure<T>(
    message_id: &str,
    call: &'static str,
    id: AccountId,
    result: PortResult<T>,
) -> PortResult<T> {
    if let Err(err) = &result
        && !matches!(err, PortError::NotFound)
    {
        tracing::error!(
            flow = CdraFlow::NAME,
            message_id,
            call,
            account_id = %id,
            error = %err,
            "account read failed other than NOTFND"
        );
    }
    result
}

impl AccountReader for AccountFile<'_> {
    /// COACCT01 4000-PROCESS-REQUEST-REPLY `EXEC CICS READ DATASET(ACCTDAT)`
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L396-404] — P-COACCT01-012.
    async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
        let call = "read_account";
        let uow = self.unit_of_work(id).await?;
        let read = bounded(call, uow.store().read_account(id)).await;
        log_read_failure(self.message_id, call, id, read)
    }

    /// Never called by `COACCT01`, which reads without `UPDATE`; it reads the
    /// same file in the same unit of work so the port stays complete.
    async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
        let call = "read_account_for_update";
        let uow = self.unit_of_work(id).await?;
        let read = bounded(call, uow.store().read_account_for_update(id)).await;
        log_read_failure(self.message_id, call, id, read)
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
    use std::io;
    use std::sync::{Arc, Mutex};

    use carddemo_domain::format::{self, REPLY_BUFFER_LEN};
    use carddemo_domain::messages;
    use carddemo_domain::numeric::AccountId;
    use carddemo_domain::ports::PortError;
    use carddemo_domain::rules::mq_account_reply::{self, ReplyOutcome};

    use super::{
        CdraFlow, PROCESS_PARAGRAPH, REPLY_QUEUE, log_begin_failure, log_read_failure, map_outcome,
        receive, reply_failure_record,
    };
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

    /// A shared in-memory sink standing in for standard output.
    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Capture {
        fn text(&self) -> String {
            let bytes = self.0.lock().expect("capture buffer").clone();
            String::from_utf8(bytes).expect("JSON lines are UTF-8")
        }
    }

    impl io::Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("capture buffer poisoned"))?
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
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
            format::move_alnum(messages::ERROR_WHILE_READING_ACCTFILE, 25).as_bytes()
        );
        assert_eq!(
            &record[error_queue::RETURN_MESSAGE],
            &messages::ERROR_WHILE_READING_ACCTFILE.as_bytes()[..25]
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
    fn a_failed_begin_or_read_is_one_error_event_that_fails_the_message_span() {
        // P-COACCT01-015: a BEGIN or an account read that fails other than
        // NOTFND ends the request in the error record without a reply. Its
        // cause is one ERROR event with the request's context, which marks
        // the message span as failed, as CP00's C001 record does; NOTFND is
        // an ordinary outcome and logs nothing.
        let id = AccountId::new(2).expect("an account number");
        let timed_out = PortError::Unavailable(
            "pool timed out while waiting for an open connection".to_owned(),
        );
        let begin = "account unit of work could not begin";
        let read = "account read failed other than NOTFND";
        let cases = [
            ("begin", timed_out.clone(), Some(begin)),
            ("read_account", timed_out, Some(read)),
            ("read_account_for_update", PortError::Contention, Some(read)),
            ("read_account", PortError::NotFound, None),
        ];
        for (call, error, message) in cases {
            let capture = Capture::default();
            let sink = capture.clone();
            let (subscriber, guard) =
                carddemo_runtime::telemetry::subscriber_with_writer("mq", "info", move || {
                    sink.clone()
                })
                .expect("valid directives");
            tracing::subscriber::with_default(subscriber, || {
                let span = tracing::info_span!("message", flow = CdraFlow::NAME);
                let _entered = span.enter();
                if call == "begin" {
                    log_begin_failure("cdra-msg-0001", id, &error);
                } else {
                    let failed = Err::<(), _>(error.clone());
                    let result = log_read_failure("cdra-msg-0001", call, id, failed);
                    assert_eq!(result, Err(error.clone()), "{call}");
                }
            });
            guard.shutdown().expect("the tracer provider shuts down");
            let text = capture.text();
            let (spans, events): (Vec<&str>, Vec<&str>) = text
                .lines()
                .partition(|line| line.contains("\"type\":\"span\""));
            assert_eq!(spans.len(), 1, "{text}");
            let Some(message) = message else {
                assert_eq!(events, Vec::<&str>::new(), "{call}");
                assert!(
                    spans[0].contains("\"status\":{\"code\":\"unset\"}"),
                    "{text}"
                );
                continue;
            };
            assert_eq!(events.len(), 1, "{text}");
            assert!(events[0].contains("\"level\":\"ERROR\""), "{text}");
            let fields = events[0]
                .split_once("\"fields\":{")
                .and_then(|(_, rest)| rest.split_once('}'))
                .map_or("", |(fields, _)| fields);
            for expected in [
                format!("\"message\":\"{message}\""),
                "\"flow\":\"CDRA\"".to_owned(),
                "\"message_id\":\"cdra-msg-0001\"".to_owned(),
                format!("\"call\":\"{call}\""),
                "\"account_id\":\"00000000002\"".to_owned(),
                format!("\"error\":\"{error}\""),
            ] {
                assert!(fields.contains(&expected), "{expected} missing from {text}");
            }
            assert!(
                spans[0].contains("\"status\":{\"code\":\"error\""),
                "{text}"
            );
        }
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
            assert_eq!(
                &record[error_queue::RETURN_MESSAGE],
                field(messages::MQPUT_ERR, 25)
            );
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
            assert!(error_queue::display_text(&record).contains(messages::MQPUT_ERR));
        }
    }

    #[test]
    fn a_request_longer_than_the_get_buffer_gets_the_inp_mqget_err_record() {
        // P-COACCT01-010 (as CODATE01), P-COACCT01-011: MQ-BUFFER-LENGTH 1000
        // without ACCEPT-TRUNCATED-MSG; the failed get (MQCC_WARNING 1,
        // MQRC_TRUNCATED_MSG_FAILED 2080) puts 'INP MQGET ERR:' naming
        // INPUT-QUEUE-NAME, and no account is read.
        let body = [b"INQA00000000001".as_slice(), &[b' '; 986]].concat();
        let Err(disposition) = receive(&request(&body), INPUT_QUEUE) else {
            panic!("a request longer than 1000 bytes is not processed");
        };
        let record = record_of(disposition);
        assert_eq!(record.len(), error_queue::ERROR_BUFFER_LEN);
        assert_eq!(
            record,
            error_queue::record_bytes(
                &mq_account_reply::oversize_request_record(&body, INPUT_QUEUE)
                    .expect("an oversize request")
            )
        );
        assert_eq!(
            &record[error_queue::RETURN_MESSAGE],
            field(messages::INP_MQGET_ERR, 25)
        );
        assert_eq!(&record[error_queue::CONDITION_CODE], b"01");
        assert_eq!(&record[error_queue::REASON_CODE], b"02080");
        assert_eq!(&record[error_queue::QUEUE_NAME], field(INPUT_QUEUE, 48));
        assert!(record[error_queue::ERROR_PARA].iter().all(|b| *b == b' '));
        assert!(
            record[error_queue::ERR_DISPLAY_LEN..]
                .iter()
                .all(|b| *b == b' ')
        );
        let fits = receive(&request(&body[..1000]), INPUT_QUEUE);
        assert!(matches!(fits, Ok(parsed) if parsed == mq_account_reply::parse_request(&body)));
    }

    #[test]
    fn an_inqa_key_with_a_non_digit_is_rejected_without_a_reply() {
        // P-COACCT01-011, E-28 (halt): no reply and no error record is guessed
        // for WS-KEY > ZEROES on a key that is not numeric.
        for body in [&b"INQA0000000000A"[..], b"INQA123", b"INQA 0000000001"] {
            let outcome = receive(&request(body), INPUT_QUEUE);
            assert!(
                matches!(outcome, Err(Disposition::Reject { location }) if location == PROCESS_PARAGRAPH),
                "{body:?}"
            );
        }
    }

    #[test]
    fn pinned_requests_go_on_to_the_account_rule() {
        // P-COACCT01-012 (INQA, key above zero), P-COACCT01-016 and
        // P-COACCT01-027 (another function), P-COACCT01-028 (key zero),
        // P-COACCT01-026 (NOTFND key).
        for body in [
            &b"INQA00000000001"[..],
            b"INQA00000000000",
            b"INQA99999999999",
            b"INQX00000000001",
            b"INQXABCDEFGHIJK",
        ] {
            let outcome = receive(&request(body), INPUT_QUEUE);
            assert!(
                matches!(outcome, Ok(parsed) if parsed == mq_account_reply::parse_request(body)),
                "{body:?}"
            );
        }
    }
}
