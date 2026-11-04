//! CP00: the card authorization decision of `COPAUA0C`.
//!
//! IBM MQ starts `COPAUA0C` by trigger on its request queue. For each request
//! it parses the comma-separated `CCPAURQY` text, reads the card's
//! cross-reference, account, customer and pending-authorization summary,
//! decides, puts the `CCPAURLY` reply to the request's reply-to queue and only
//! then records the authorization in the IMS pending-authorization database
//! (P-COPAUA0C-005). Here [`Cp00Flow`] is served by the long-running consumer
//! of [`crate::consumer::run`] on the request queue the settings name
//! (`CARDDEMO_QUEUE_PAUTH_REQUEST`). Every result comes from
//! `carddemo_domain::rules::auth_decision`; this file sequences its steps:
//!
//! 1. [`Flow::handle`]: the conversion of the request text and the 500-byte
//!    buffer check (`3100-READ-REQUEST-MQ`), the parse, the check that the
//!    stored receivers are storable and the amount
//!    (`2100-EXTRACT-REQUEST-MSG`), the reads in a read-only unit of work
//!    (`5100` to `5500`), the decision and the reply (`6000`, `7100`);
//! 2. the consumer publishes the reply, waits for its confirm and acknowledges
//!    the request;
//! 3. [`Flow::after_ack`]: the summary and detail writes (`8000`), in a unit of
//!    work of their own, so the reply and the data stay apart (AAP 0.3.3).
//!
//! Both units of work read the summary under its row lock
//! (`AuthorizationStore::read_summary_for_update`), as `COPAUA0C`'s `GU` runs
//! under the update intent of `PSBPAUTB` (`PROCOPT=AP`) and IMS program
//! isolation orders it with the `CBPAUP0C` purge BMP. A running
//! `batch cbpaup0j` sweep that holds the summary is waited for, and the write
//! unit then writes on the summary the purge left, so the purge never deletes
//! an authorization it did not read.
//!
//! Every `9500-LOG-ERROR` record is logged as one JSON line with the
//! `CCPAUERY` fields, a card number in its event key masked to the last four
//! characters, and the request's `message_id` and `correlation_id`;
//! `carddemo_runtime::telemetry` formats it. CP00 has no
//! error queue: a request without a confirmed reply is logged and rejected
//! without requeue, which consumes it as the COBOL's no-syncpoint get does
//! (P-COPAUA0C-002).
//!
//! # `RabbitMQ` direct reply-to
//!
//! CP00 answers no requester on `RabbitMQ` direct reply-to, a `reply_to` of
//! `amq.rabbitmq.reply-to`, which the broker rewrites to
//! `amq.rabbitmq.reply-to.<token>`. The broker delivers a mandatory publish to
//! that pseudo-queue yet returns it as unroutable, so the reply would reach the
//! requester while counting as not delivered, and the request would be
//! rejected without its writes: an approval with no credit hold. Such a request
//! is treated as one that names no reply queue, before anything is published:
//! a warning names the pseudo-queue, the `M004` record is logged and the
//! request is rejected without requeue, neither answered nor written
//! (P-COPAUA0C-012). A requester names a reply queue of its own instead, for
//! example a server-named `amq.gen-*` queue.
//!
//! # Not reproduced
//!
//! The per-task `MQOPEN` and `MQCLOSE` of the request queue (`M001`, `M005`),
//! the get loop with its `M003` read failure and the PSB scheduling (`I001`)
//! have no counterpart in the long-running consumer (E-22; P-COPAUA0C-001,
//! P-COPAUA0C-003, P-COPAUA0C-033, P-COPAUA0C-036, P-COPAUA0C-037). The one
//! `M003` the replacement keeps is the get of a request longer than the
//! 500-byte buffer.
//!
//! # Halt cases
//!
//! An input whose COBOL result the audit cannot pin reaches this flow as an
//! `auth_decision::Unproven` naming its `halt` escalation. Request text the
//! get's conversion cannot pin (E-80), a receiver the detail stores holding
//! X'00', which `PostgreSQL` text cannot hold (E-81), and an unproven request
//! amount are rejected before any read or reply. A card the cross-reference
//! lacks gets no reply either, because its decision on the first message of
//! a task tests a flag no statement set (E-41); a write plan of that kind
//! writes nothing. Each is logged with the escalation ID and its request is
//! rejected without requeue (AAP 0.9.6).

use std::time::Duration;

use carddemo_domain::clock::{self, AnyClock, Clock};
use carddemo_domain::rules::auth_decision::{
    self, AuthContext, AuthDecision, AuthRequest, AuthWritePlan, AuthorizationStore, ErrorLevel,
    ErrorLogEntry, Money, PortError, SummaryWrite, Unproven, WriteStep,
};
use carddemo_persistence::tx::Db;

use crate::connection::BROKER_TIMEOUT;
use crate::consumer::{self, Disposition, FailureAction, Flow, PublishFailure, Reply, Request};
use crate::db_deadline::{BoundedStore, DB_DEADLINE, bounded};
use crate::ids;

/// COPAUA0C 3100-READ-REQUEST-MQ — P-COPAUA0C-002: the paragraph a request
/// whose text the `MQGMO-CONVERT` get cannot pin (E-80) is rejected at,
/// recorded as the message span's `location`.
const READ_PARAGRAPH: &str = "3100-READ-REQUEST-MQ";

/// COPAUA0C 2100-EXTRACT-REQUEST-MSG — P-COPAUA0C-004: the paragraph a
/// request with an unproven amount, or with a receiver holding X'00' that
/// `PostgreSQL` text cannot store (E-81), is rejected at, recorded as the
/// message span's `location`.
const EXTRACT_PARAGRAPH: &str = "2100-EXTRACT-REQUEST-MSG";

/// COPAUA0C 6000-MAKE-DECISION — P-COPAUA0C-028: the paragraph a request
/// whose decision the audit cannot pin (E-41) is rejected at, recorded as the
/// message span's `location`.
const DECISION_PARAGRAPH: &str = "6000-MAKE-DECISION";

/// COPAUA0C 8000-WRITE-AUTH-TO-DB — P-COPAUA0C-035: the paragraph a write
/// plan the audit cannot pin is skipped at, recorded in its log event and as
/// the message span's `location`.
const WRITE_PARAGRAPH: &str = "8000-WRITE-AUTH-TO-DB";

/// COPAUA0C 7100-SEND-RESPONSE — P-COPAUA0C-012: the `RabbitMQ` direct
/// reply-to pseudo-queue, and the prefix of the
/// `amq.rabbitmq.reply-to.<token>` name the broker rewrites a request's
/// `reply_to` to. `RabbitMQ` reserves every queue name that starts with
/// `amq.`, so no declared queue starts with it.
const DIRECT_REPLY_TO: &str = "amq.rabbitmq.reply-to";

/// The database calls [`Cp00Flow::read`] makes, each under its own database
/// deadline: the `BEGIN`, the four reads of `auth_decision::gather`
/// (cross-reference, account, customer, the summary under its lock) and the
/// `ROLLBACK`.
const READ_CALLS: u32 = 6;

/// The database calls [`Cp00Flow::write`] makes, each under its own database
/// deadline: the `BEGIN`, the summary read under its lock, the summary write,
/// the detail insert and the `COMMIT`.
const WRITE_CALLS: u32 = 5;

/// The longest CP00's post-acknowledgement unit of work runs with every call
/// within its deadline: `WRITE_CALLS` database deadlines. What an abandoned
/// call leaves on the connection is settled inside the next call's deadline,
/// the `COMMIT`'s included, so the unit spends no time outside its calls.
pub(crate) const POST_ACK_BUDGET: Duration = DB_DEADLINE.saturating_mul(WRITE_CALLS);

/// The longest one CP00 delivery runs with every bounded step within its
/// limit: the read unit of work (`READ_CALLS` database deadlines), the reply's
/// publish and confirm ([`BROKER_TIMEOUT`]) and [`POST_ACK_BUDGET`]. CP00
/// publishes no error record. The acknowledgement between the reply and the
/// writes has no limit (`crate::consumer`).
pub(crate) const DELIVERY_BUDGET: Duration = DB_DEADLINE
    .saturating_mul(READ_CALLS)
    .saturating_add(BROKER_TIMEOUT)
    .saturating_add(POST_ACK_BUDGET);

/// The CP00 flow: the `COPAUA0C` rules adapted to [`Flow`].
#[derive(Debug, Clone)]
pub struct Cp00Flow {
    /// The pool the reads and the pending-authorization writes run on.
    db: Db,
    /// The `EXEC CICS ASKTIME` instant source of the detail key and the
    /// error-log stamps: the host clock, or the `CARDDEMO_CLOCK_OVERRIDE` pin.
    clock: AnyClock,
}

impl Cp00Flow {
    /// A flow that reads and writes through `db` and stamps with `clock`.
    #[must_use]
    pub const fn new(db: Db, clock: AnyClock) -> Self {
        Self { db, clock }
    }
}

/// COPAUA0C 5000-PROCESS-AUTH — P-COPAUA0C-005, P-COPAUA0C-035: what
/// `8000-WRITE-AUTH-TO-DB` needs once the reply is confirmed and the request
/// acknowledged.
///
/// The write plan itself is built in [`Flow::after_ack`], because the
/// `ASKTIME` that keys the detail runs in `8500-INSERT-AUTH`, after
/// `7100-SEND-RESPONSE` (P-COPAUA0C-030).
#[derive(Debug)]
pub struct PendingWrite {
    /// The parsed request.
    request: AuthRequest,
    /// `WS-TRANSACTION-AMT`, the request amount after `FUNCTION NUMVAL`.
    amount: Money<10>,
    /// The `6000-MAKE-DECISION` result the reply carried.
    decision: AuthDecision,
    /// What the reads found.
    context: AuthContext,
}

impl Flow for Cp00Flow {
    /// `WS-CICS-TRANID PIC X(04) VALUE 'CP00'`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L34].
    const NAME: &'static str = auth_decision::ERR_APPLICATION;

    type AfterAck = PendingWrite;

    /// COPAUA0C 2000-MAIN-PROCESS / 5000-PROCESS-AUTH — P-COPAUA0C-002 to
    /// P-COPAUA0C-012, P-COPAUA0C-032.
    ///
    /// Extracts the request, reads what the decision needs and answers with
    /// the `CCPAURLY` reply to the request's `reply_to`. A request is rejected
    /// without a reply when it is longer than the 500-byte buffer (`M003`),
    /// when a read fails critically (`C001` to `C003`, `I002`) or when it
    /// names no reply queue (`M004`): each of those ends the COBOL task before
    /// or at `7100-SEND-RESPONSE`, after its no-syncpoint get has consumed the
    /// request. A request on `RabbitMQ` direct reply-to, which CP00 does not
    /// answer, takes the same `M004` path after a warning that names it. A
    /// request whose text the get's conversion cannot pin (E-80),
    /// whose stored receivers hold X'00' (E-81) or whose amount is unproven is
    /// rejected before any read, and a card the cross-reference lacks after
    /// its `A001` warning, because its decision is unproven (E-41): no guessed
    /// reply is sent.
    async fn handle(&self, req: &Request) -> Disposition<PendingWrite> {
        let (request, amount) = match extract(req, &self.clock) {
            Ok(extracted) => extracted,
            Err(location) => return Disposition::Reject { location },
        };
        let context = match self.read(req, &request).await {
            Ok(context) => context,
            Err(location) => return Disposition::Reject { location },
        };
        match respond(req, request, amount, context) {
            Ok((reply, after_ack)) => Disposition::Reply { reply, after_ack },
            Err(NoReply::Unproven(unproven)) => {
                log_unproven(req, unproven, DECISION_PARAGRAPH);
                Disposition::Reject {
                    location: DECISION_PARAGRAPH,
                }
            }
            Err(NoReply::Logged(entry)) => {
                self.log_entry(req, &entry);
                Disposition::Reject {
                    location: entry.location,
                }
            }
            Err(NoReply::DirectReplyTo(entry)) => {
                log_direct_reply_to(req);
                self.log_entry(req, &entry);
                Disposition::Reject {
                    location: entry.location,
                }
            }
        }
    }

    /// COPAUA0C 7100-SEND-RESPONSE `M004` — P-COPAUA0C-012
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L767-778].
    ///
    /// Logs the critical `"FAILED TO PUT ON REPLY MQ"` record with the codes
    /// `failure` maps to. M004 ends the COBOL task before `8000`, and the
    /// consumer runs [`Flow::after_ack`] only after a confirmed reply, so no
    /// pending authorization is written; the consumer then rejects the request
    /// without requeue.
    fn on_reply_failure(&self, req: &Request, failure: &PublishFailure) -> FailureAction {
        let entry = reply_failure_entry_for(failure);
        self.log_entry(req, &entry);
        FailureAction::Logged {
            location: entry.location,
        }
    }

    /// COPAUA0C 8000-WRITE-AUTH-TO-DB / 8400-UPDATE-SUMMARY / 8500-INSERT-AUTH —
    /// P-COPAUA0C-013, P-COPAUA0C-014, P-COPAUA0C-019, P-COPAUA0C-030,
    /// P-COPAUA0C-035.
    ///
    /// Plans the writes at the clock's instant and runs them. A card the
    /// cross-reference lacks plans none
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L463-465], and
    /// [`Flow::handle`] already rejects such a request before any reply; a
    /// plan the audit cannot pin (a declined found card, E-41) is logged with
    /// its escalation and writes nothing. The request is already acknowledged,
    /// so nothing here reaches the broker.
    ///
    /// The plan fixes the detail, whose key the clock's instant gives, and the
    /// decision the reply carried. Its summary write comes from the read that
    /// decided the request, so [`Cp00Flow::write`] derives it again on the
    /// summary its unit of work locks first, which a purge may have changed or
    /// deleted since.
    ///
    /// Each failure's location is recorded in the message span as soon as it
    /// is logged ([`consumer::record_location`]): the `I003` or `I004` of a
    /// failed read or write, or `8000-WRITE-AUTH-TO-DB` for a skipped plan. A
    /// plan written, or none planned, leaves the span's `none`.
    async fn after_ack(&self, req: &Request, work: PendingWrite) {
        let PendingWrite {
            request,
            amount,
            decision,
            context,
        } = work;
        let planned =
            auth_decision::write_plan(&request, amount, &context, &decision, self.clock.now());
        match planned {
            Ok(Some(plan)) => self.write(req, &plan, &context, &decision).await,
            Ok(None) => {}
            Err(unproven) => {
                log_unproven(req, unproven, WRITE_PARAGRAPH);
                consumer::record_location(WRITE_PARAGRAPH);
            }
        }
    }
}

impl Cp00Flow {
    /// COPAUA0C 5100-READ-XREF-RECORD to 5500-READ-AUTH-SUMMRY — P-COPAUA0C-005
    /// to P-COPAUA0C-008, P-COPAUA0C-034
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L472-641].
    ///
    /// Runs the reads in a unit of work that is rolled back afterwards, since
    /// it changes nothing, and logs their records in the order raised. The
    /// summary read, the last and the only one that locks, waits for a purge
    /// sweep that holds the summary and holds it until the rollback, so the
    /// decision is made on what the purge left. The
    /// warnings `A001` to `A003` go on to the decline. A critical record ends
    /// the COBOL task through `9990-END-ROUTINE` before any reply, so its
    /// location is returned and the request is rejected. A unit of work that
    /// cannot be opened stands in for the failed first read: `C001`. Every
    /// event logged here names `req`, the request, by its IDs.
    ///
    /// The `BEGIN`, each read and the `ROLLBACK` run under the limits of
    /// `crate::db_deadline`. A read the server cancels at the statement
    /// timeout, or that outlives the database deadline, fails like any other
    /// read failure, so it becomes the `C001`, `C002`, `C003` or
    /// `I002` record of that read; the database cause of every failed read is
    /// logged beside the record, which keeps only the legacy codes. An
    /// abandoned read is rolled back to its savepoint by the next read or the
    /// `ROLLBACK`, within that call's deadline, so the reads after it still
    /// run in the same unit of work. `READ_CALLS` counts these calls for
    /// [`DELIVERY_BUDGET`], from which the shutdown drain is derived, so a call
    /// added here is counted there.
    async fn read(
        &self,
        req: &Request,
        request: &AuthRequest,
    ) -> Result<AuthContext, &'static str> {
        let mut uow = match bounded("begin", self.db.begin()).await {
            Ok(uow) => uow,
            Err(error) => {
                tracing::warn!(
                    flow = Self::NAME,
                    message_id = req.message_id.as_deref().unwrap_or(""),
                    correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                    error = %error,
                    "read unit of work not opened"
                );
                let entry = auth_decision::xref_read_failure_entry(&request.pa_rq_card_num, &error);
                self.log_entry(req, &entry);
                return Err(entry.location);
            }
        };
        let context =
            auth_decision::gather(&mut BoundedStore::new(Self::NAME, uow.store()), request).await;
        if let Err(error) = bounded("rollback", uow.rollback()).await {
            tracing::warn!(
                flow = Self::NAME,
                message_id = req.message_id.as_deref().unwrap_or(""),
                correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                error = %error,
                "read unit of work not rolled back"
            );
        }
        for entry in &context.log {
            self.log_entry(req, entry);
        }
        match context.critical_entry() {
            Some(entry) => Err(entry.location),
            None => Ok(context),
        }
    }

    /// COPAUA0C 8000-WRITE-AUTH-TO-DB — P-COPAUA0C-008, P-COPAUA0C-013,
    /// P-COPAUA0C-035: runs `plan` in a unit of work of its own and commits
    /// it.
    ///
    /// The first call after the `BEGIN` reads the plan's summary under its
    /// row lock ([`write_segments`]), so a running purge sweep that holds it
    /// is waited for, and the summary write is derived again on what that
    /// read finds, from `context` and `decision`
    /// (`auth_decision::locked_summary_write`); the detail is the plan's. The
    /// purge then either swept the summary before this unit read it, or
    /// reaches it only after this unit commits and sees the count and the
    /// detail written here (P-CBPAUP0C-004, P-CBPAUP0C-006). A locked read
    /// that fails is `I003` and nothing is written; a summary under the lock
    /// whose update the audit cannot pin (E-78) is logged with its escalation
    /// at `8000-WRITE-AUTH-TO-DB` for `req` and nothing is written.
    ///
    /// The commit stands for the `EXEC CICS RETURN` (after `I003` or `I004`) or
    /// the loop `SYNCPOINT` (after both writes) that commits the COBOL task, so
    /// a summary written before a failed detail insert persists, as the
    /// audit's "persists on later failure" column records. Each read and write
    /// runs under its own savepoint in the store. A unit of work that cannot be
    /// opened or committed loses the summary write, so it is logged as `I003`.
    /// Every event logged here names `req`, the acknowledged request, by its
    /// IDs.
    ///
    /// The `BEGIN`, the locked read, each write and the `COMMIT` run under the
    /// limits of `crate::db_deadline`, so a stalled database, or a purge that
    /// holds the summary too long, cannot hold the consumer after the
    /// acknowledgement. A read or write the server cancels at the statement
    /// timeout, or that outlives the database deadline, fails like any other
    /// failed one (`I003` or `I004`). A call abandoned at its deadline,
    /// wherever it waits, is rolled back to its own savepoint when the
    /// `COMMIT` call first settles the connection, within the commit's
    /// deadline, and the same unit of work then commits the summary written
    /// before a failed detail. A `BEGIN` or `COMMIT` that fails or outlives
    /// the deadline is `I003`. The database cause of each failure is logged
    /// beside the record, and the acknowledged request is never replayed.
    /// `WRITE_CALLS` counts these calls for [`POST_ACK_BUDGET`], which the
    /// shutdown drain waits for, so a call added here is counted there.
    ///
    /// Each record's location is recorded in the message span as soon as it
    /// is logged ([`consumer::record_location`]). A failed commit's `I003`
    /// replaces an earlier `I004`, because the summary that record left in
    /// place is lost with the commit.
    async fn write(
        &self,
        req: &Request,
        plan: &AuthWritePlan,
        context: &AuthContext,
        decision: &AuthDecision,
    ) {
        let mut uow = match bounded("begin", self.db.begin()).await {
            Ok(uow) => uow,
            Err(error) => {
                tracing::warn!(
                    flow = Self::NAME,
                    message_id = req.message_id.as_deref().unwrap_or(""),
                    correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                    error = %error,
                    "write unit of work not opened"
                );
                let entry = auth_decision::write_failure_entry(WriteStep::Summary, &error, plan);
                self.log_entry(req, &entry);
                consumer::record_location(entry.location);
                return;
            }
        };
        let mut store = BoundedStore::new(Self::NAME, uow.store());
        match write_segments(&mut store, plan, context, decision).await {
            Ok(()) => {}
            Err(WriteFailure::Logged(entry)) => {
                self.log_entry(req, &entry);
                consumer::record_location(entry.location);
            }
            Err(WriteFailure::Unproven(unproven)) => {
                log_unproven(req, unproven, WRITE_PARAGRAPH);
                consumer::record_location(WRITE_PARAGRAPH);
            }
        }
        if let Err(error) = bounded("commit", uow.commit()).await {
            tracing::warn!(
                flow = Self::NAME,
                message_id = req.message_id.as_deref().unwrap_or(""),
                correlation_id = req.correlation_id.as_deref().unwrap_or(""),
                error = %error,
                "write unit of work not committed"
            );
            let entry = auth_decision::write_failure_entry(WriteStep::Summary, &error, plan);
            self.log_entry(req, &entry);
            consumer::record_location(entry.location);
        }
    }

    /// COPAUA0C 9500-LOG-ERROR — P-COPAUA0C-015, P-COPAUA0C-031: logs `entry`
    /// of the request `req` stamped with the flow's clock.
    fn log_entry(&self, req: &Request, entry: &ErrorLogEntry) {
        log_record(req, entry, &self.clock);
    }
}

/// COPAUA0C 3100-READ-REQUEST-MQ / 2100-EXTRACT-REQUEST-MSG — P-COPAUA0C-002,
/// P-COPAUA0C-004, P-COPAUA0C-016, P-COPAUA0C-025
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L386-432]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L351-380].
///
/// In the order the COBOL meets them:
///
/// 1. the body is converted as the `MQGMO-CONVERT` get converts it
///    ([`auth_decision::convert_request`]); text that is not UTF-8 or holds a
///    character outside ISO 8859-1 is unproven (E-80) and rejected at
///    `3100-READ-REQUEST-MQ`;
/// 2. converted text longer than `W01-GET-BUFFER PIC X(500)` fails the get
///    with the critical `M003` record;
/// 3. the converted text is parsed; a receiver the detail stores as text
///    holding X'00', which `PostgreSQL` text cannot hold
///    ([`auth_decision::storable`]), is unproven (E-81) and rejected at
///    `2100-EXTRACT-REQUEST-MSG`;
/// 4. the amount is converted; an unproven amount is rejected there too.
///
/// Each record is logged stamped with `clock`. Returns the location the
/// request is rejected at when it gets no reply; such a request is neither
/// read nor written.
fn extract(req: &Request, clock: &impl Clock) -> Result<(AuthRequest, Money<10>), &'static str> {
    let data = match auth_decision::convert_request(&req.body) {
        Ok(data) => data,
        Err(unproven) => {
            log_unproven(req, unproven, READ_PARAGRAPH);
            return Err(READ_PARAGRAPH);
        }
    };
    if let Some(entry) = auth_decision::oversize_request_entry(&data) {
        log_record(req, &entry, clock);
        return Err(entry.location);
    }
    let request = auth_decision::parse_request(&data);
    if let Err(unproven) = auth_decision::storable(&request) {
        log_unproven(req, unproven, EXTRACT_PARAGRAPH);
        return Err(EXTRACT_PARAGRAPH);
    }
    match auth_decision::parse_amount(&request) {
        Ok(amount) => Ok((request, amount)),
        Err(unproven) => {
            log_unproven(req, unproven, EXTRACT_PARAGRAPH);
            Err(EXTRACT_PARAGRAPH)
        }
    }
}

/// COPAUA0C 6000-MAKE-DECISION / 7100-SEND-RESPONSE — P-COPAUA0C-009 to
/// P-COPAUA0C-012, P-COPAUA0C-026 to P-COPAUA0C-029
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L657-779].
///
/// Decides the request from what the reads found and builds its reply: the
/// domain's 64-byte first-message `CCPAURLY` text (E-13), addressed to the
/// request's `reply_to`, with the request's `correlation_id` (`MQMD-CORRELID`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L745]) and a new
/// `message_id` (`MQMI-NONE` [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L746]).
///
/// A card the cross-reference lacks is not answered: its decision tests a
/// flag no statement set (E-41, halt, P-COPAUA0C-028), so the `3100` decline
/// the domain gives it, the proposal section D row P-COPAUA0C-026 records, is
/// not sent. [`auth_decision::decidable`] decides this before the decision
/// and before the reply queue is looked at, since `6000-MAKE-DECISION` runs
/// before `7100-SEND-RESPONSE`.
///
/// # Errors
///
/// - [`NoReply::Unproven`] (`E-41`) when the cross-reference lacks the card.
/// - [`NoReply::Logged`] with the `M004` record when the request names no
///   reply queue: the `MQPUT1` to a blank `MQOD-OBJECTNAME` fails, and its
///   AMQP counterpart is a publish the broker returns as unroutable,
///   [`PublishFailure::Returned`].
/// - [`NoReply::DirectReplyTo`] with the same `M004` record when the request
///   names `RabbitMQ` direct reply-to ([`NoQueue::DirectReplyTo`]): the broker
///   would deliver the reply and still return it, so it is not published.
fn respond(
    req: &Request,
    request: AuthRequest,
    amount: Money<10>,
    context: AuthContext,
) -> Result<(Reply, PendingWrite), NoReply> {
    auth_decision::decidable(&context).map_err(NoReply::Unproven)?;
    let decision = auth_decision::decide(amount, &context);
    let queue = reply_queue(req).map_err(|no_queue| {
        let entry = reply_failure_entry_for(&PublishFailure::Returned);
        match no_queue {
            NoQueue::Blank => NoReply::Logged(entry),
            NoQueue::DirectReplyTo => NoReply::DirectReplyTo(entry),
        }
    })?;
    let reply = Reply {
        queue: queue.to_owned(),
        body: auth_decision::reply_body(&request, &decision).into_bytes(),
        message_id: Some(ids::new_message_id()),
        correlation_id: req.correlation_id.clone(),
    };
    let work = PendingWrite {
        request,
        amount,
        decision,
        context,
    };
    Ok((reply, work))
}

/// COPAUA0C 6000-MAKE-DECISION / 7100-SEND-RESPONSE — P-COPAUA0C-012,
/// P-COPAUA0C-028: why [`respond`] gives a request no reply.
#[derive(Debug, PartialEq, Eq)]
enum NoReply {
    /// The decision is one the audit cannot pin: logged with its escalation
    /// at `6000-MAKE-DECISION`, with no record of its own.
    Unproven(Unproven),
    /// The `CCPAUERY` record to log, `M004`, of a request that names no reply
    /// queue.
    Logged(ErrorLogEntry),
    /// The `M004` record of a request on `RabbitMQ` direct reply-to, which CP00
    /// does not answer: logged after the warning of [`log_direct_reply_to`].
    DirectReplyTo(ErrorLogEntry),
}

/// COPAUA0C 7100-SEND-RESPONSE — P-COPAUA0C-012: why a request's `reply_to`
/// gives [`respond`] no queue to answer on. Either way the reply's `MQPUT1`
/// fails, `M004`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoQueue {
    /// `reply_to` is absent, empty or only spaces: a blank `MQOD-OBJECTNAME`.
    Blank,
    /// `reply_to` names `RabbitMQ` direct reply-to ([`DIRECT_REPLY_TO`]). The
    /// broker delivers a mandatory publish to it yet returns it as
    /// unroutable, so no reply to it can be confirmed.
    DirectReplyTo,
}

/// COPAUA0C 3100-READ-REQUEST-MQ / 7100-SEND-RESPONSE — P-COPAUA0C-002,
/// P-COPAUA0C-012: the reply queue, `MQMD-REPLYTOQ` saved as `WS-REPLY-QNAME`
/// and moved to `MQOD-OBJECTNAME`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L413]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L742].
///
/// MQ object names are blank-padded `X(48)`, so trailing spaces are padding
/// and are removed; an AMQP queue name matches only exactly.
///
/// # Errors
///
/// - [`NoQueue::Blank`] when `reply_to` is absent, empty or only spaces.
/// - [`NoQueue::DirectReplyTo`] when, without its padding, it starts with
///   [`DIRECT_REPLY_TO`], compared case-sensitively as the broker compares
///   queue names. A server-named `amq.gen-*` queue is a reply queue.
fn reply_queue(req: &Request) -> Result<&str, NoQueue> {
    let name = req
        .reply_to
        .as_deref()
        .map_or("", |name| name.trim_end_matches(' '));
    if name.is_empty() {
        Err(NoQueue::Blank)
    } else if name.starts_with(DIRECT_REPLY_TO) {
        Err(NoQueue::DirectReplyTo)
    } else {
        Ok(name)
    }
}

/// COPAUA0C 7100-SEND-RESPONSE — P-COPAUA0C-012: the `M004` record of a reply
/// that `failure` kept from being delivered, with the completion and reason
/// codes the failure maps to (E-39).
fn reply_failure_entry_for(failure: &PublishFailure) -> ErrorLogEntry {
    let (condition_code, reason_code) = failure.mq_codes();
    auth_decision::reply_failure_entry(i64::from(condition_code), i64::from(reason_code))
}

/// COPAUA0C 8000-WRITE-AUTH-TO-DB — P-COPAUA0C-035: why [`write_segments`]
/// left a write undone.
#[derive(Debug, PartialEq, Eq)]
enum WriteFailure {
    /// The `9500-LOG-ERROR` record of the failed read or write: `I003` for the
    /// locked summary read or the summary write, `I004` for the detail insert.
    Logged(ErrorLogEntry),
    /// The summary under the lock takes an update the audit cannot pin
    /// (E-78): logged with its escalation at `8000-WRITE-AUTH-TO-DB`, with no
    /// record of its own, and nothing is written.
    Unproven(Unproven),
}

/// COPAUA0C 5500-READ-AUTH-SUMMRY / 8400-UPDATE-SUMMARY / 8500-INSERT-AUTH —
/// P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-014, P-COPAUA0C-035
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L616-640]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L801-847]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L913-932].
///
/// The store calls of the write unit of work for `plan`, between its `BEGIN`
/// and its `COMMIT`, in order:
///
/// 1. the plan's summary read under its row lock
///    ([`AuthorizationStore::read_summary_for_update`]), which waits for a
///    running purge sweep that holds it. A read that fails other than `GE` is
///    `I003` and nothing is written: the summary the unit would replace or
///    insert under is unknown;
/// 2. the summary write derived again on what the lock found, from `context`
///    and `decision` (`auth_decision::locked_summary_write`): `REPL` of the
///    summary found, `ISRT` of a new one on `GE`. An unpinned update (E-78)
///    writes nothing. A failed summary write is `I003`, which ends the COBOL
///    task before the detail insert, so the detail is not attempted;
/// 3. the plan's detail under it. A failed detail insert is `I004`.
///
/// `context` and `decision` are those `plan` was built from, so the summary
/// write exists whenever the plan does. Returns why a write was left undone,
/// `Ok` when both were written.
async fn write_segments<S: AuthorizationStore>(
    store: &mut S,
    plan: &AuthWritePlan,
    context: &AuthContext,
    decision: &AuthDecision,
) -> Result<(), WriteFailure> {
    let summary_failure = |error: &PortError| {
        WriteFailure::Logged(auth_decision::write_failure_entry(
            WriteStep::Summary,
            error,
            plan,
        ))
    };
    let locked = match store.read_summary_for_update(plan.detail.pa_acct_id).await {
        Ok(summary) => Some(summary),
        Err(PortError::NotFound) => None,
        Err(error) => return Err(summary_failure(&error)),
    };
    let summary = match auth_decision::locked_summary_write(context, decision, locked.as_ref()) {
        Ok(Some(summary)) => summary,
        // The context plans no write: nothing is written, as for no plan.
        Ok(None) => return Ok(()),
        Err(unproven) => return Err(WriteFailure::Unproven(unproven)),
    };
    let written = match summary {
        SummaryWrite::Insert(summary) => store.insert_summary(summary).await,
        SummaryWrite::Replace(summary) => store.update_summary(summary).await,
    };
    if let Err(error) = written {
        return Err(summary_failure(&error));
    }
    store
        .insert_detail(plan.detail.clone())
        .await
        .map(|_| ())
        .map_err(|error| {
            WriteFailure::Logged(auth_decision::write_failure_entry(
                WriteStep::Detail,
                &error,
                plan,
            ))
        })
}

/// Logs an input class whose COBOL result the audit cannot pin: the
/// escalation that holds it at `halt` and the paragraph whose output is
/// skipped for it, the reply at `3100`, `2100` or `6000` or the writes at
/// `8000` (AAP 0.9.6).
fn log_unproven(req: &Request, unproven: Unproven, paragraph: &'static str) {
    tracing::error!(
        flow = Cp00Flow::NAME,
        message_id = req.message_id.as_deref().unwrap_or(""),
        escalation = unproven.escalation_id,
        paragraph,
        "COPAUA0C result unproven: the paragraph's output is skipped"
    );
}

/// COPAUA0C 7100-SEND-RESPONSE — P-COPAUA0C-012: logs at `warn` that `req`
/// names `RabbitMQ` direct reply-to, with its `reply_to` as the request carries
/// it. The broker delivers a mandatory publish to that pseudo-queue yet
/// returns it as unroutable, so CP00 publishes no reply to it; the `M004`
/// record follows and the request is rejected without its writes.
fn log_direct_reply_to(req: &Request) {
    tracing::warn!(
        flow = Cp00Flow::NAME,
        message_id = req.message_id.as_deref().unwrap_or(""),
        correlation_id = req.correlation_id.as_deref().unwrap_or(""),
        reply_to = req.reply_to.as_deref().unwrap_or(""),
        "RabbitMQ direct reply-to is not supported: the broker returns a mandatory reply to it \
         as unroutable while still delivering it, so no reply is sent; name a reply queue"
    );
}

/// COPAUA0C 9500-LOG-ERROR — P-COPAUA0C-015, P-COPAUA0C-031
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L983-1011].
///
/// The `ERROR-LOG-RECORD` the COBOL writes to the TD queue `CSSL`, as one
/// event whose fields are the `CCPAUERY` fields
/// [app/app-authorization-ims-db2-mq/cpy/CCPAUERY.cpy:L19-40]: `ERR-DATE`
/// (`YYMMDD`) and `ERR-TIME` (`HHMMSS`) of the clock's instant, the
/// application `CP00`, the program `COPAUA0C`, then the entry's location,
/// level, subsystem, codes, message and event key. Each `X(n)` value is
/// logged without its right padding. A critical record is an `ERROR` event, a
/// warning a `WARN` event.
///
/// The fields keep the copybook names (`err_location`, `err_level`,
/// `err_message`, …) rather than bare `level` and `message`, which the JSON
/// formatter already uses for the event's own level and message.
///
/// The event key is logged through [`logged_event_key`]: a card number is
/// masked to its last four characters, so no log line carries a full card
/// number, while the account and customer keys are logged as they are. The
/// [`ErrorLogEntry`] itself keeps the key the COBOL moves.
///
/// After the `CCPAUERY` fields come the `message_id` and `correlation_id` of
/// `req`, the request the record was raised for, empty when it has none. The
/// event names its request itself, so it can be attributed when the
/// `info`-level message span is filtered out.
fn log_record(req: &Request, entry: &ErrorLogEntry, clock: &impl Clock) {
    let now = clock.now();
    let err_date = clock::yymmdd(now);
    let err_time = clock::hhmmss_compact(now);
    let err_level = char::from(entry.level.code());
    let err_subsystem = char::from(entry.source.code());
    let err_code_1 = entry.code_1.trimmed();
    let err_code_2 = entry.code_2.trimmed();
    let err_event_key = logged_event_key(entry);
    let message_id = req.message_id.as_deref().unwrap_or("");
    let correlation_id = req.correlation_id.as_deref().unwrap_or("");
    // One field list for both levels: a tracing callsite fixes its level at
    // compile time, so each level needs its own macro call.
    macro_rules! error_log_record {
        ($event:ident) => {
            tracing::$event!(
                err_date = %err_date,
                err_time = %err_time,
                err_application = auth_decision::ERR_APPLICATION,
                err_program = auth_decision::ERR_PROGRAM,
                err_location = entry.location,
                err_level = %err_level,
                err_subsystem = %err_subsystem,
                err_code_1 = %err_code_1,
                err_code_2 = %err_code_2,
                err_message = entry.message,
                err_event_key = %err_event_key,
                message_id,
                correlation_id,
                "ERROR-LOG-RECORD"
            )
        };
    }
    match entry.level {
        ErrorLevel::Critical => error_log_record!(error),
        ErrorLevel::Warning => error_log_record!(warn),
    }
}

/// COPAUA0C 5200-READ-ACCT-RECORD / 5300-READ-CUST-RECORD — P-COPAUA0C-007:
/// the `ERR-LOCATION`s whose `ERR-EVENT-KEY` is not a card number. `A002` and
/// `C002` carry the account ID `WS-CARD-RID-ACCT-ID-X`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L546]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L559]; `A003` and `C003`
/// the customer ID `WS-CARD-RID-CUST-ID`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L594]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L607]. Every other
/// record moves `XREF-CARD-NUM` or `PA-CARD-NUM` into the key
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L499]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L930].
const NON_CARD_KEY_LOCATIONS: [&str; 4] = ["A002", "C002", "A003", "C003"];

/// The trailing characters of a card number a log line shows, as the
/// `Debug` form of `CardNumber` shows them.
const CARD_KEY_SHOWN: usize = 4;

/// COPAUA0C 9500-LOG-ERROR — P-COPAUA0C-015: `ERR-EVENT-KEY` of `entry` as its
/// log line carries it, without its right padding.
///
/// A card number, the key of every location but those in
/// [`NON_CARD_KEY_LOCATIONS`], has every character but its last four replaced
/// by `*`, and every character when it has four or fewer: the log keeps the
/// `CCPAUERY` field and enough of the card to tell records apart, never the
/// full number. The account and customer keys are logged as they are.
fn logged_event_key(entry: &ErrorLogEntry) -> String {
    let key = entry.event_key.trimmed();
    if NON_CARD_KEY_LOCATIONS.contains(&entry.location) {
        return key.into_owned();
    }
    let count = key.chars().count();
    let hidden = if count > CARD_KEY_SHOWN {
        count - CARD_KEY_SHOWN
    } else {
        count
    };
    std::iter::repeat_n('*', hidden)
        .chain(key.chars().skip(hidden))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::future::{self, Future};
    use std::io;
    use std::pin::pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};
    use std::time::{Duration, Instant};

    use carddemo_codec::{CustomerLayout, decode_ascii_file};
    use carddemo_domain::clock::{AnyClock, Clock};
    use carddemo_domain::entities::{
        Account, AuthFraud, CardXref, Customer, NewPendingAuthDetail, PendingAuthDetail,
        PendingAuthSummary,
    };
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, Alnum, AuthCount, AuthId, CardNumber, CustomerId, Money,
    };
    use carddemo_domain::ports::{AuthorizationStore, Browse, Direction, PortError, PortResult};
    use carddemo_domain::rules::auth_decision::{
        self, AuthContext, AuthWritePlan, ErrorLevel, ErrorLogEntry, ErrorSource, Lookup,
        SummaryWrite, Unproven, WriteStep,
    };
    use carddemo_persistence::connect_with_statement_timeout;
    use carddemo_testkit::{TestStack, fixtures};

    use super::{
        Cp00Flow, NoQueue, NoReply, POST_ACK_BUDGET, PendingWrite, WRITE_CALLS, WriteFailure,
        log_direct_reply_to, log_record, logged_event_key, reply_failure_entry_for, reply_queue,
        respond, write_segments,
    };
    use crate::consumer::{Disposition, Flow, PublishFailure, Reply, Request};
    use crate::db_deadline::{BoundedStore, DB_DEADLINE};

    /// The section F pin of P-COPAUA0C-030 and P-COPAUA0C-031.
    const PINNED: &str = "2022-07-01T12:00:00Z";

    /// The card of the first cross-reference fixture record, on customer and
    /// account 50 [app/data/ASCII/cardxref.txt:L1].
    const FIXTURE_CARD: &str = "0500024453765740";

    /// The card of P-COPAUA0C-026, which no cross-reference holds.
    const UNKNOWN_CARD: &str = "9999999999999999";

    fn pinned_clock() -> AnyClock {
        AnyClock::from_override(Some(PINNED)).expect("the pin is RFC 3339")
    }

    /// The section D request of P-COPAUA0C-021 to P-COPAUA0C-026 with its
    /// card, amount and transaction ID replaced.
    fn request_text(card: &str, amount: &str, transaction: &str) -> String {
        format!(
            "220701,120000,{card},0100,1225,0100,POS,000000,{amount},5411,840,01,\
             M00000000000001,FIXTURE MERCHANT,SEATTLE,WA,98101,{transaction}"
        )
    }

    /// Built from the public fields rather than `Request::from_parts`, whose
    /// broker-properties argument would tie these tests to the AMQP client
    /// types the flow never names.
    fn request(body: &str, reply_to: Option<&str>) -> Request {
        Request {
            body: body.as_bytes().to_vec(),
            message_id: Some("cp00-msg-0001".to_owned()),
            correlation_id: Some("cp00-corr-0001".to_owned()),
            reply_to: reply_to.map(str::to_owned),
        }
    }

    /// Polls `future` once with a no-op waker. The fake store answers at once,
    /// so a pending result is a failure.
    fn ready_now<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        match pin!(future).poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("the fake store answers on the first poll"),
        }
    }

    fn cents(value: i64) -> Money<10> {
        Money::from_cents(value).expect("an amount within S9(10)V99")
    }

    fn fixture_xref() -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(FIXTURE_CARD),
            xref_cust_id: CustomerId::new(50).expect("a customer number"),
            xref_acct_id: AccountId::new(50).expect("an account number"),
        }
    }

    /// Account 50 as `acctdata.txt` L50 decodes: credit limit 6169.00, balance
    /// 492.00, so 5677.00 is available (P-COPAUA0C-021).
    fn fixture_account() -> Account {
        Account {
            acct_id: AccountId::new(50).expect("an account number"),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: cents(49_200),
            acct_credit_limit: cents(616_900),
            acct_cash_credit_limit: cents(458_700),
            acct_open_date: Alnum::move_from("2011-04-22"),
            acct_expiraion_date: Alnum::move_from("2023-03-09"),
            acct_reissue_date: Alnum::move_from("2023-03-09"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    /// Customer 50 as `custdata.txt` L50 decodes through the shared
    /// [`CustomerLayout`] [app/data/ASCII/custdata.txt:L50].
    fn fixture_customer() -> Customer {
        let path = fixtures::ascii_dir().join("custdata.txt");
        let bytes =
            fs::read(&path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
        let id = CustomerId::new(50).expect("a customer number");
        decode_ascii_file::<CustomerLayout>(&bytes)
            .unwrap_or_else(|err| panic!("cannot decode {}: {err}", path.display()))
            .into_iter()
            .map(|framed| framed.record.customer)
            .find(|customer| customer.cust_id == id)
            .unwrap_or_else(|| panic!("{} holds no customer 50", path.display()))
    }

    /// What the reads find for the fixture card in the seeded state: its
    /// cross-reference, account and customer, and no pending authorization.
    fn fixture_context() -> AuthContext {
        AuthContext {
            xref: Lookup::Found(fixture_xref()),
            account: Lookup::Found(fixture_account()),
            customer: Lookup::Found(fixture_customer()),
            summary: Lookup::NotFound,
            log: Vec::new(),
        }
    }

    /// What the reads find for a card the cross-reference lacks: nothing past
    /// the `A001` warning (P-COPAUA0C-006).
    fn unknown_card_context() -> AuthContext {
        let card = CardNumber::move_from(UNKNOWN_CARD);
        AuthContext {
            xref: Lookup::NotFound,
            account: Lookup::NotFound,
            customer: Lookup::NotFound,
            summary: Lookup::NotFound,
            log: vec![ErrorLogEntry {
                location: "A001",
                level: ErrorLevel::Warning,
                source: ErrorSource::App,
                code_1: Alnum::spaces(),
                code_2: Alnum::spaces(),
                message: messages::CARD_NOT_FOUND_IN_XREF,
                event_key: Alnum::move_from_bytes(card.as_bytes()),
            }],
        }
    }

    /// Converts and parses `body` and answers it against `context`, as
    /// `handle` does after the reads.
    fn answer(req: &Request, context: AuthContext) -> Result<(Reply, PendingWrite), NoReply> {
        let data = auth_decision::convert_request(&req.body).expect("convertible request text");
        let request = auth_decision::parse_request(&data);
        let amount = auth_decision::parse_amount(&request).expect("a pinned amount");
        respond(req, request, amount, context)
    }

    /// The write plan `after_ack` builds for `work` at the pinned instant.
    fn plan_of(work: &PendingWrite) -> Option<AuthWritePlan> {
        auth_decision::write_plan(
            &work.request,
            work.amount,
            &work.context,
            &work.decision,
            pinned_clock().now(),
        )
        .expect("a pinned plan")
    }

    /// The `MQMD-MSGID` shape of P-COPAUA0C-029: 48 lower-case hex characters.
    fn is_message_id(id: &str) -> bool {
        id.len() == 48
            && id
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    }

    /// An `AuthorizationStore` that records the calls `write_segments` makes
    /// and fails the one named in `failing`.
    #[derive(Default)]
    struct FakeStore {
        calls: Vec<&'static str>,
        failing: Option<&'static str>,
        stalling: Option<&'static str>,
        /// The summary the locked read finds; `None` answers `GE`.
        locked: Option<PendingAuthSummary>,
        /// The summaries inserted or replaced, in call order.
        written: Vec<PendingAuthSummary>,
    }

    impl FakeStore {
        /// A store whose locked read finds `locked`, or `GE` for `None`.
        fn locking(locked: Option<PendingAuthSummary>) -> Self {
            Self {
                locked,
                ..Self::default()
            }
        }

        fn failing(call: &'static str) -> Self {
            Self {
                failing: Some(call),
                ..Self::default()
            }
        }

        /// A store whose `call` never answers, as a write blocked by a lock
        /// another session holds.
        fn stalling(call: &'static str) -> Self {
            Self {
                stalling: Some(call),
                ..Self::default()
            }
        }

        /// Records `call` and answers it: never when it is the stalling call,
        /// the injected failure when it is the failing call, `value` otherwise.
        async fn answer<T: Send>(&mut self, call: &'static str, value: T) -> PortResult<T> {
            self.calls.push(call);
            if self.stalling == Some(call) {
                future::pending::<()>().await;
            }
            if self.failing == Some(call) {
                Err(PortError::Unavailable("injected failure".to_owned()))
            } else {
                Ok(value)
            }
        }

        /// A call `8000-WRITE-AUTH-TO-DB` never makes: recorded, so a test
        /// sees it, and refused.
        fn outside_8000<T>(&mut self, call: &'static str) -> PortResult<T> {
            self.calls.push(call);
            Err(PortError::Unavailable(format!(
                "{call} is not an 8000 write"
            )))
        }
    }

    impl AuthorizationStore for FakeStore {
        async fn read_summary(&mut self, _id: AccountId) -> PortResult<PendingAuthSummary> {
            self.outside_8000("read_summary")
        }

        async fn read_summary_for_update(
            &mut self,
            _id: AccountId,
        ) -> PortResult<PendingAuthSummary> {
            let locked = self.locked.clone();
            self.answer("read_summary_for_update", ()).await?;
            locked.ok_or(PortError::NotFound)
        }

        async fn read_summary_of_detail(&mut self, _id: AuthId) -> PortResult<PendingAuthSummary> {
            self.outside_8000("read_summary_of_detail")
        }

        async fn insert_summary(&mut self, summary: PendingAuthSummary) -> PortResult<()> {
            self.answer("insert_summary", ()).await?;
            self.written.push(summary);
            Ok(())
        }

        async fn update_summary(&mut self, summary: PendingAuthSummary) -> PortResult<()> {
            self.answer("update_summary", ()).await?;
            self.written.push(summary);
            Ok(())
        }

        async fn delete_summary(&mut self, _id: AccountId) -> PortResult<()> {
            self.outside_8000("delete_summary")
        }

        async fn summaries_after(
            &mut self,
            _after: Option<AccountId>,
            _limit: u16,
        ) -> PortResult<Vec<PendingAuthSummary>> {
            self.outside_8000("summaries_after")
        }

        async fn insert_detail(&mut self, _detail: NewPendingAuthDetail) -> PortResult<AuthId> {
            self.answer("insert_detail", AuthId::from(1)).await
        }

        async fn read_detail(&mut self, _id: AuthId) -> PortResult<PendingAuthDetail> {
            self.outside_8000("read_detail")
        }

        async fn read_detail_for_update(&mut self, _id: AuthId) -> PortResult<PendingAuthDetail> {
            self.outside_8000("read_detail_for_update")
        }

        async fn details_for_account(
            &mut self,
            _id: AccountId,
            _browse: Browse<AuthId>,
        ) -> PortResult<Vec<PendingAuthDetail>> {
            self.outside_8000("details_for_account")
        }

        async fn adjacent_detail(
            &mut self,
            _account: AccountId,
            _auth_date_9c: i32,
            _auth_time_9c: i32,
            _direction: Direction,
        ) -> PortResult<Option<PendingAuthDetail>> {
            self.outside_8000("adjacent_detail")
        }

        async fn update_detail(&mut self, _detail: PendingAuthDetail) -> PortResult<()> {
            self.outside_8000("update_detail")
        }

        async fn delete_detail(&mut self, _id: AuthId) -> PortResult<()> {
            self.outside_8000("delete_detail")
        }

        async fn insert_fraud(&mut self, _fraud: AuthFraud) -> PortResult<()> {
            self.outside_8000("insert_fraud")
        }

        async fn update_fraud(&mut self, _fraud: AuthFraud) -> PortResult<()> {
            self.outside_8000("update_fraud")
        }
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

    /// The JSON line `log_record` writes for `entry` of the request `req`
    /// through the binaries' telemetry subscriber, at the pinned instant,
    /// outside any span.
    fn logged_line(req: &Request, entry: &ErrorLogEntry) -> String {
        one_line(|| log_record(req, entry, &pinned_clock()))
    }

    /// The one JSON line `log` writes through the binaries' telemetry
    /// subscriber.
    fn one_line(log: impl FnOnce()) -> String {
        let capture = Capture::default();
        let sink = capture.clone();
        let (subscriber, guard) =
            carddemo_runtime::telemetry::subscriber_with_writer("mq", "info", move || sink.clone())
                .expect("valid directives");
        tracing::subscriber::with_default(subscriber, log);
        guard.shutdown().expect("the tracer provider shuts down");
        let text = capture.text();
        let mut lines = text.lines();
        let line = lines.next().expect("one JSON line").to_owned();
        assert_eq!(lines.next(), None, "{text}");
        line
    }

    #[test]
    fn the_flow_is_named_for_the_cics_transaction() {
        // WS-CICS-TRANID VALUE 'CP00' [COPAUA0C.cbl:L34].
        assert_eq!(Cp00Flow::NAME, "CP00");
    }

    #[test]
    fn the_context_customer_is_the_fixture_record_of_customer_50() {
        // 000000050 Aniya Alba Von ... OR ... 623 [app/data/ASCII/custdata.txt:L50].
        let customer = fixture_customer();
        assert_eq!(customer.cust_first_name.trimmed(), "Aniya");
        assert_eq!(customer.cust_middle_name.trimmed(), "Alba");
        assert_eq!(customer.cust_last_name.trimmed(), "Von");
        assert_eq!(customer.cust_addr_state_cd.trimmed(), "OR");
        assert_eq!(customer.cust_fico_credit_score.to_string(), "623");
    }

    #[test]
    fn the_reply_queue_is_the_reply_to_without_its_blank_padding() {
        // P-COPAUA0C-002, P-COPAUA0C-012: WS-REPLY-QNAME X(48) to MQOD-OBJECTNAME.
        // RabbitMQ direct reply-to, as sent or as the broker rewrites it, is no
        // reply queue; server-named queues and names that only differ in case
        // from the pseudo-queue are.
        let cases = [
            (None, Err(NoQueue::Blank)),
            (Some(""), Err(NoQueue::Blank)),
            (Some("     "), Err(NoQueue::Blank)),
            (Some("REPLY.Q   "), Ok("REPLY.Q")),
            (Some("REPLY.Q"), Ok("REPLY.Q")),
            (Some("AMQ.GEN-x y"), Ok("AMQ.GEN-x y")),
            (Some("amq.gen-abc"), Ok("amq.gen-abc")),
            (Some("AMQ.RABBITMQ.REPLY-TO"), Ok("AMQ.RABBITMQ.REPLY-TO")),
            (Some("amq.rabbitmq.reply-to"), Err(NoQueue::DirectReplyTo)),
            (
                Some("amq.rabbitmq.reply-to   "),
                Err(NoQueue::DirectReplyTo),
            ),
            (
                Some("amq.rabbitmq.reply-to.g1h2AA==.abc"),
                Err(NoQueue::DirectReplyTo),
            ),
        ];
        for (reply_to, expected) in cases {
            let req = request("", reply_to);
            assert_eq!(reply_queue(&req), expected, "{reply_to:?}");
        }
    }

    #[test]
    fn a_found_card_gets_the_audited_reply_with_a_new_message_id() {
        // P-COPAUA0C-021 (section D), P-COPAUA0C-011, P-COPAUA0C-012,
        // P-COPAUA0C-029: byte 64 is never set and written as a space (E-25).
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let req = request(&body, Some("CP00.REPLY.QUEUE   "));
        let (reply, work) = answer(&req, fixture_context()).expect("a reply queue is named");
        assert_eq!(reply.queue, "CP00.REPLY.QUEUE");
        assert_eq!(
            reply.body,
            b"0500024453765740,TXN000000000001,120000,00,0000,        100.00, "
        );
        assert_eq!(
            reply.body,
            auth_decision::reply_body(&work.request, &work.decision).into_bytes()
        );
        assert_eq!(reply.correlation_id, req.correlation_id);
        let message_id = reply.message_id.as_deref().expect("a new message ID");
        assert!(is_message_id(message_id), "{message_id}");
        assert_ne!(reply.message_id, req.message_id);
        assert!(work.decision.approved);
        let plan = plan_of(&work).expect("a found card is written");
        assert!(matches!(plan.summary, SummaryWrite::Insert(_)), "{plan:?}");
        assert_eq!(plan.detail.pa_card_num, CardNumber::move_from(FIXTURE_CARD));
    }

    #[test]
    fn every_reply_takes_its_own_message_id() {
        // P-COPAUA0C-029: MQMI-NONE, a new MSGID per reply.
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let req = request(&body, Some("CP00.REPLY.QUEUE"));
        let first = answer(&req, fixture_context()).expect("a reply").0;
        let second = answer(&req, fixture_context()).expect("a reply").0;
        assert_ne!(first.message_id, second.message_id);
        assert_eq!(first.body, second.body);
    }

    #[test]
    fn a_non_ascii_request_is_answered_with_the_characters_it_sent() {
        // P-COPAUA0C-002, P-COPAUA0C-011, P-COPAUA0C-014 (E-80): the reply
        // body is the UTF-8 of the 64-character reply, the transaction ID
        // TXNÉ followed by the 11 spaces of its X(15), and the detail keeps
        // the merchant name sent.
        let body = request_text(FIXTURE_CARD, "100.00", "TXNÉ")
            .replace("FIXTURE MERCHANT", "CAFÉ MÜNCHEN");
        let req = request(&body, Some("CP00.REPLY.QUEUE"));
        let (reply, work) = answer(&req, fixture_context()).expect("a reply queue is named");
        assert_eq!(
            reply.body,
            "0500024453765740,TXNÉ           ,120000,00,0000,        100.00, ".as_bytes()
        );
        assert_eq!(reply.body.get(17..22), Some(&b"TXN\xC3\x89"[..]));
        let text = String::from_utf8(reply.body).expect("a UTF-8 reply");
        assert_eq!(text.chars().count(), 64);
        let plan = plan_of(&work).expect("a found card is written");
        assert_eq!(plan.detail.pa_merchant_name.trimmed(), "CAFÉ MÜNCHEN");
        assert_eq!(plan.detail.pa_transaction_id.trimmed(), "TXNÉ");
    }

    #[test]
    fn extract_converts_checks_the_buffer_then_parses_as_the_cobol_does() {
        // P-COPAUA0C-002, P-COPAUA0C-004 (E-80, E-81, E-28): text the
        // conversion cannot pin is rejected at 3100; converted text past 500
        // bytes is M003, whatever its UTF-8 length; a stored receiver holding
        // X'00' and an unproven amount are rejected at 2100. None of them is
        // read or answered.
        let ascii = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let name = "FIXTURE MERCHANT";
        let named = |merchant: &str| ascii.replace(name, merchant);
        // The name that makes the converted text exactly the 500-byte buffer.
        let filling = "É".repeat(auth_decision::REQUEST_BUFFER_LEN - ascii.len() + name.len());
        // The text as a Latin-1 sender encodes it: one byte per character.
        let latin1: Vec<u8> = named("CAFÉ MÜNCHEN")
            .chars()
            .map(|character| u8::try_from(character).expect("an ISO 8859-1 character"))
            .collect();
        let cases: [(Vec<u8>, Result<i64, &str>); 9] = [
            (ascii.clone().into_bytes(), Ok(10_000)),
            (named("CAFÉ MÜNCHEN").into_bytes(), Ok(10_000)),
            (named(&filling).into_bytes(), Ok(10_000)),
            (named(&format!("{filling}É")).into_bytes(), Err("M003")),
            (named("CAFÉ €").into_bytes(), Err(super::READ_PARAGRAPH)),
            (latin1, Err(super::READ_PARAGRAPH)),
            (
                named("NUL\0MERCHANT").into_bytes(),
                Err(super::EXTRACT_PARAGRAPH),
            ),
            (
                request_text(FIXTURE_CARD, "abc", "TXN000000000001").into_bytes(),
                Err(super::EXTRACT_PARAGRAPH),
            ),
            (
                request_text(FIXTURE_CARD, "5.00", "TXN000000000001\0").into_bytes(),
                Ok(500),
            ),
        ];
        for (body, expected) in cases {
            let req = Request {
                body,
                ..request("", Some("CP00.REPLY.QUEUE"))
            };
            let extracted = super::extract(&req, &pinned_clock());
            assert_eq!(
                extracted.map(|(_, amount)| amount.cents()),
                expected,
                "{:?}",
                req.body
            );
        }
    }

    #[test]
    fn an_unknown_card_is_halted_without_a_reply() {
        // P-COPAUA0C-028 (E-41 halt): the decision of a card the cross-reference
        // lacks is unproven, so the P-COPAUA0C-026 proposal (05/3100) is not
        // sent, whether or not a reply queue is named, and no write follows.
        // 6000-MAKE-DECISION runs before 7100-SEND-RESPONSE looks at the
        // reply queue, so direct reply-to does not turn it into M004.
        let body = request_text(UNKNOWN_CARD, "100.00", "TXN000000000002");
        for reply_to in [
            Some("CP00.REPLY.QUEUE"),
            None,
            Some("amq.rabbitmq.reply-to"),
        ] {
            let req = request(&body, reply_to);
            let outcome = answer(&req, unknown_card_context());
            assert_eq!(
                outcome.map(|(reply, _)| reply),
                Err(NoReply::Unproven(Unproven {
                    escalation_id: "E-41"
                })),
                "{reply_to:?}"
            );
        }
    }

    #[test]
    fn a_request_without_a_reply_queue_is_the_m004_record() {
        // P-COPAUA0C-012: MQPUT1 to a blank reply-to queue fails, M004, and no
        // reply exists; the codes are those of a returned publish (E-39).
        // RabbitMQ direct reply-to, as sent, as the broker rewrites it and
        // blank-padded, takes the same record before anything is published.
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let (condition_code, reason_code) = PublishFailure::Returned.mq_codes();
        let cases = [
            (None, false),
            (Some(""), false),
            (Some("   "), false),
            (Some("amq.rabbitmq.reply-to"), true),
            (Some("amq.rabbitmq.reply-to.g1h2AA==.abc"), true),
            (Some("amq.rabbitmq.reply-to.g1h2AA==.abc   "), true),
        ];
        for (reply_to, direct_reply_to) in cases {
            let req = request(&body, reply_to);
            let entry = match answer(&req, fixture_context()) {
                Err(NoReply::Logged(entry)) if !direct_reply_to => entry,
                Err(NoReply::DirectReplyTo(entry)) if direct_reply_to => entry,
                other => panic!("{reply_to:?} is not its M004 record: {other:?}"),
            };
            assert_eq!(entry.location, "M004", "{reply_to:?}");
            assert_eq!(entry.level, ErrorLevel::Critical);
            assert_eq!(entry.source, ErrorSource::Mq);
            assert_eq!(entry.message, messages::FAILED_TO_PUT_ON_REPLY_MQ);
            assert_eq!(entry.code_1.to_string(), format!("{condition_code:09}"));
            assert_eq!(entry.code_2.to_string(), format!("{reason_code:09}"));
        }
    }

    #[test]
    fn a_direct_reply_to_request_is_one_warning_naming_it() {
        // P-COPAUA0C-012: the warning before the M004 record names the request
        // and the pseudo-queue it gave as its reply-to, never its body.
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let reply_to = "amq.rabbitmq.reply-to.g1h2AA==.abc";
        let req = request(&body, Some(reply_to));
        let line = one_line(|| log_direct_reply_to(&req));
        for expected in [
            "\"level\":\"WARN\"".to_owned(),
            "\"flow\":\"CP00\"".to_owned(),
            "\"message_id\":\"cp00-msg-0001\"".to_owned(),
            "\"correlation_id\":\"cp00-corr-0001\"".to_owned(),
            format!("\"reply_to\":\"{reply_to}\""),
            "RabbitMQ direct reply-to is not supported".to_owned(),
        ] {
            assert!(line.contains(&expected), "{expected} missing from {line}");
        }
        assert!(!line.contains(FIXTURE_CARD), "{line}");
        assert!(!line.contains("ERROR-LOG-RECORD"), "{line}");
    }

    #[test]
    fn every_unconfirmed_reply_is_logged_as_m004_with_its_codes() {
        // P-COPAUA0C-012: ERR-CODE-1 and ERR-CODE-2 through WS-CODE-DISPLAY
        // 9(9); the event key PA-CARD-NUM is not set before 8500 (E-25).
        for failure in [
            PublishFailure::Returned,
            PublishFailure::Nacked,
            PublishFailure::NotRequested,
        ] {
            let (condition_code, reason_code) = failure.mq_codes();
            let entry = reply_failure_entry_for(&failure);
            assert_eq!(
                entry,
                auth_decision::reply_failure_entry(
                    i64::from(condition_code),
                    i64::from(reason_code)
                ),
                "{failure:?}"
            );
            assert_eq!(entry.location, "M004");
            assert_eq!(entry.level, ErrorLevel::Critical);
            assert_eq!(entry.source, ErrorSource::Mq);
            assert_eq!(entry.event_key, Alnum::spaces());
        }
    }

    /// The P-COPAUA0C-021 approval, decided with the summary `5500` found when
    /// `summary_found`, and the write plan `after_ack` builds for it.
    fn approved_work(summary_found: bool) -> (PendingWrite, AuthWritePlan) {
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let req = request(&body, Some("CP00.REPLY.QUEUE"));
        let mut context = fixture_context();
        if summary_found {
            context.summary = Lookup::Found(new_summary());
        }
        let (_, work) = answer(&req, context).expect("a reply");
        let plan = plan_of(&work).expect("a found card is written");
        (work, plan)
    }

    /// The plan of [`approved_work`].
    fn approved_plan(summary_found: bool) -> AuthWritePlan {
        approved_work(summary_found).1
    }

    /// The summary the P-COPAUA0C-021 approval inserts when `5500` finds
    /// none: one approval of 100.00.
    fn new_summary() -> PendingAuthSummary {
        match approved_plan(false).summary {
            SummaryWrite::Insert(summary) => summary,
            other @ SummaryWrite::Replace(_) => panic!("a new summary: {other:?}"),
        }
    }

    /// The outcome of the write unit's store calls for `work` and its `plan`
    /// against `store`, which answers at once.
    fn write_now(
        store: &mut FakeStore,
        work: &PendingWrite,
        plan: &AuthWritePlan,
    ) -> Result<(), WriteFailure> {
        ready_now(write_segments(store, plan, &work.context, &work.decision))
    }

    /// The record of a failed read or write, which `outcome` must be.
    fn logged(outcome: Result<(), WriteFailure>) -> ErrorLogEntry {
        match outcome {
            Err(WriteFailure::Logged(entry)) => entry,
            other => panic!("a logged failure: {other:?}"),
        }
    }

    #[test]
    fn both_segments_are_written_summary_first() {
        // P-COPAUA0C-035, P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-014: the
        // summary read under its lock, ISRT or REPL of the summary, then ISRT
        // of the detail; nothing is logged.
        for (locked, summary_call) in [
            (None, "insert_summary"),
            (Some(new_summary()), "update_summary"),
        ] {
            let (work, plan) = approved_work(locked.is_some());
            let mut store = FakeStore::locking(locked);
            assert_eq!(write_now(&mut store, &work, &plan), Ok(()));
            assert_eq!(
                store.calls,
                ["read_summary_for_update", summary_call, "insert_detail"]
            );
        }
    }

    #[test]
    fn the_summary_write_follows_the_summary_the_lock_finds() {
        // P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-019, P-CBPAUP0C-006: the
        // plan comes from the read that decided the request; the write unit
        // writes on the summary it locks. A summary written since is replaced
        // with its own count plus one; one a purge deleted is inserted anew.
        let found = new_summary();
        let cases = [
            (false, Some(found), "update_summary", 2, "200.00"),
            (true, None, "insert_summary", 1, "100.00"),
        ];
        for (summary_found, locked, summary_call, count, total) in cases {
            let (work, plan) = approved_work(summary_found);
            let mut store = FakeStore::locking(locked);
            assert_eq!(
                write_now(&mut store, &work, &plan),
                Ok(()),
                "{summary_call}"
            );
            assert_eq!(
                store.calls,
                ["read_summary_for_update", summary_call, "insert_detail"]
            );
            let [written] = store.written.as_slice() else {
                panic!("one summary write: {:?}", store.written);
            };
            assert_eq!(written.pa_approved_auth_cnt.get(), count, "{summary_call}");
            assert_eq!(
                written.pa_approved_auth_amt.to_string(),
                total,
                "{summary_call}"
            );
            assert_eq!(
                written.pa_credit_balance.to_string(),
                total,
                "{summary_call}"
            );
        }
    }

    #[test]
    fn the_post_ack_budget_counts_every_call_of_the_write_unit() {
        // POST_ACK_BUDGET is WRITE_CALLS database deadlines: the BEGIN and the
        // COMMIT of `write` and every store call `write_segments` makes.
        let (work, plan) = approved_work(true);
        let mut store = FakeStore::locking(Some(new_summary()));
        assert_eq!(write_now(&mut store, &work, &plan), Ok(()));
        let calls = u32::try_from(store.calls.len()).expect("a handful of calls");
        assert_eq!(calls + 2, WRITE_CALLS);
        assert_eq!(POST_ACK_BUDGET, DB_DEADLINE * WRITE_CALLS);
    }

    #[test]
    fn a_failed_locked_read_is_i003_and_writes_nothing() {
        // P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-035: without the summary
        // it holds, the write unit has nothing to replace or insert under; the
        // record is the summary write's I003, its event key not set (E-25).
        let (work, plan) = approved_work(true);
        let mut store = FakeStore {
            locked: Some(new_summary()),
            ..FakeStore::failing("read_summary_for_update")
        };
        let entry = logged(write_now(&mut store, &work, &plan));
        assert_eq!(store.calls, ["read_summary_for_update"]);
        assert_eq!(store.written, []);
        assert_eq!(entry.location, "I003");
        assert_eq!(entry.level, ErrorLevel::Critical);
        assert_eq!(entry.source, ErrorSource::Ims);
        assert_eq!(entry.message, messages::IMS_UPDATE_SUMRY_FAILED);
        assert_eq!(entry.code_1.trimmed(), "AO");
        assert_eq!(entry.event_key, Alnum::spaces());
    }

    #[test]
    fn an_unpinned_update_of_the_locked_summary_writes_nothing() {
        // P-COPAUA0C-013, E-78: the summary under the lock already holds 9999
        // approvals, so the ADD 1 is unpinned and nothing is written.
        let (work, plan) = approved_work(false);
        let at_9999 = PendingAuthSummary {
            pa_approved_auth_cnt: AuthCount::new(9_999).expect("a count"),
            ..new_summary()
        };
        let mut store = FakeStore::locking(Some(at_9999));
        assert_eq!(
            write_now(&mut store, &work, &plan),
            Err(WriteFailure::Unproven(Unproven {
                escalation_id: "E-78"
            }))
        );
        assert_eq!(store.calls, ["read_summary_for_update"]);
        assert_eq!(store.written, []);
    }

    #[test]
    fn a_failed_summary_write_is_i003_and_skips_the_detail() {
        // P-COPAUA0C-013, P-COPAUA0C-035: I003 ends the task before 8500; its
        // event key PA-CARD-NUM is not set yet (E-25).
        for (locked, summary_call) in [
            (None, "insert_summary"),
            (Some(new_summary()), "update_summary"),
        ] {
            let (work, plan) = approved_work(false);
            let mut store = FakeStore {
                locked,
                ..FakeStore::failing(summary_call)
            };
            let entry = logged(write_now(&mut store, &work, &plan));
            assert_eq!(store.calls, ["read_summary_for_update", summary_call]);
            assert_eq!(entry.location, "I003");
            assert_eq!(entry.level, ErrorLevel::Critical);
            assert_eq!(entry.source, ErrorSource::Ims);
            assert_eq!(entry.message, messages::IMS_UPDATE_SUMRY_FAILED);
            assert_eq!(entry.event_key, Alnum::spaces());
        }
    }

    #[test]
    fn a_failed_detail_insert_is_i004_after_the_summary_write() {
        // P-COPAUA0C-014, P-COPAUA0C-035: the summary write stands; I004 is
        // keyed by the card 8500 moved into the detail.
        let (work, plan) = approved_work(false);
        let mut store = FakeStore::failing("insert_detail");
        let entry = logged(write_now(&mut store, &work, &plan));
        assert_eq!(
            store.calls,
            ["read_summary_for_update", "insert_summary", "insert_detail"]
        );
        assert_eq!(entry.location, "I004");
        assert_eq!(entry.level, ErrorLevel::Critical);
        assert_eq!(entry.source, ErrorSource::Ims);
        assert_eq!(entry.message, messages::IMS_INSERT_DETL_FAILED);
        assert_eq!(entry.event_key.trimmed(), FIXTURE_CARD);
    }

    #[test]
    fn a_write_past_the_database_deadline_takes_its_failure_path() {
        // P-COPAUA0C-008, P-COPAUA0C-013, P-COPAUA0C-014, P-COPAUA0C-035: a
        // locked summary read that never completes, held by a lock another
        // session keeps, is I003 and nothing is written; so is a summary write
        // that never completes, and the detail is not attempted; a detail
        // insert that never completes is I004 after the summary write. Each
        // carries the IMS status of a store failure (E-44), and none is
        // retried.
        let (work, plan) = approved_work(false);
        let cases = [
            ("read_summary_for_update", "I003"),
            ("insert_summary", "I003"),
            ("insert_detail", "I004"),
        ];
        for (stalled, location) in cases {
            let mut store = BoundedStore::with_deadline(
                Cp00Flow::NAME,
                FakeStore::stalling(stalled),
                Duration::from_millis(20),
            );
            let outcome = carddemo_testkit::block_on(write_segments(
                &mut store,
                &plan,
                &work.context,
                &work.decision,
            ));
            let entry = logged(outcome);
            assert_eq!(entry.location, location, "{stalled}");
            assert_eq!(entry.level, ErrorLevel::Critical, "{stalled}");
            assert_eq!(entry.code_1.trimmed(), "AO", "{stalled}");
        }
    }

    #[test]
    fn an_error_log_record_is_one_json_line_of_ccpauery_fields() {
        // P-COPAUA0C-015, P-COPAUA0C-031: ERR-DATE YYMMDD and ERR-TIME HHMMSS of
        // the pinned instant, application CP00, program COPAUA0C; a critical
        // record is an ERROR event, a warning a WARN event. The A001 key, the
        // card number, is logged masked to its last four characters. Outside
        // any span, the record still names its request by its IDs.
        let critical = reply_failure_entry_for(&PublishFailure::Returned);
        let warning = unknown_card_context().log.remove(0);
        let req = request("", Some("CP00.REPLY.QUEUE"));
        let message = |text: &str| format!("\"err_message\":\"{text}\"");
        let cases = [
            (
                &critical,
                "\"level\":\"ERROR\"",
                [
                    "\"err_location\":\"M004\"".to_owned(),
                    "\"err_level\":\"C\"".to_owned(),
                    "\"err_subsystem\":\"M\"".to_owned(),
                    message(messages::FAILED_TO_PUT_ON_REPLY_MQ),
                    "\"err_event_key\":\"\"".to_owned(),
                ],
            ),
            (
                &warning,
                "\"level\":\"WARN\"",
                [
                    "\"err_location\":\"A001\"".to_owned(),
                    "\"err_level\":\"W\"".to_owned(),
                    "\"err_subsystem\":\"A\"".to_owned(),
                    message(messages::CARD_NOT_FOUND_IN_XREF),
                    "\"err_event_key\":\"************9999\"".to_owned(),
                ],
            ),
        ];
        for (entry, level, fields) in cases {
            let line = logged_line(&req, entry);
            let common = [
                level,
                "\"message\":\"ERROR-LOG-RECORD\"",
                "\"err_date\":\"220701\"",
                "\"err_time\":\"120000\"",
                "\"err_application\":\"CP00\"",
                "\"err_program\":\"COPAUA0C\"",
                "\"message_id\":\"cp00-msg-0001\"",
                "\"correlation_id\":\"cp00-corr-0001\"",
            ];
            for expected in common.into_iter().chain(fields.iter().map(String::as_str)) {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
            assert!(!line.contains(UNKNOWN_CARD), "{line}");
        }
        let codes = logged_line(&req, &critical);
        assert!(codes.contains(&format!("\"err_code_1\":\"{}\"", critical.code_1.trimmed())));
        assert!(codes.contains(&format!("\"err_code_2\":\"{}\"", critical.code_2.trimmed())));
        // A request without IDs is logged with empty ones.
        let bare = Request {
            message_id: None,
            correlation_id: None,
            ..req
        };
        let line = logged_line(&bare, &critical);
        for expected in ["\"message_id\":\"\"", "\"correlation_id\":\"\""] {
            assert!(line.contains(expected), "{expected} missing from {line}");
        }
    }

    /// A record at `location` keyed by `key`. Only the location and the key
    /// decide how the log shows the key.
    fn keyed_entry(location: &'static str, key: &str) -> ErrorLogEntry {
        ErrorLogEntry {
            location,
            level: ErrorLevel::Warning,
            source: ErrorSource::App,
            code_1: Alnum::spaces(),
            code_2: Alnum::spaces(),
            message: messages::ACCT_NOT_FOUND_IN_XREF,
            event_key: Alnum::move_from(key),
        }
    }

    #[test]
    fn a_card_number_event_key_is_logged_masked_and_other_keys_as_they_are() {
        // P-COPAUA0C-015: XREF-CARD-NUM (A001, C001) and PA-CARD-NUM (I004)
        // keep only their last four characters in the log; the account and
        // customer keys of A002/C002 and A003/C003 stay whole; a key never set
        // (M004, E-25) stays empty. The records themselves keep the full key.
        let failure = PortError::Unavailable("injected failure".to_owned());
        let card = CardNumber::move_from(FIXTURE_CARD);
        let cases = [
            (unknown_card_context().log.remove(0), "************9999"),
            (
                auth_decision::xref_read_failure_entry(&card, &failure),
                "************5740",
            ),
            (
                auth_decision::write_failure_entry(
                    WriteStep::Detail,
                    &failure,
                    &approved_plan(false),
                ),
                "************5740",
            ),
            (reply_failure_entry_for(&PublishFailure::Nacked), ""),
            (keyed_entry("A002", "00000000050"), "00000000050"),
            (keyed_entry("C002", "00000000050"), "00000000050"),
            (keyed_entry("A003", "000000050"), "000000050"),
            (keyed_entry("C003", "000000050"), "000000050"),
            (keyed_entry("A001", "1234"), "****"),
        ];
        // The request names the card too; its body is never logged.
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let req = request(&body, Some("CP00.REPLY.QUEUE"));
        for (entry, logged) in cases {
            assert_eq!(logged_event_key(&entry), logged, "{}", entry.location);
            let line = logged_line(&req, &entry);
            assert!(
                line.contains(&format!("\"err_event_key\":\"{logged}\"")),
                "{line}"
            );
            assert!(
                !line.contains(FIXTURE_CARD) && !line.contains(UNKNOWN_CARD),
                "{line}"
            );
        }
        assert_eq!(
            auth_decision::xref_read_failure_entry(&card, &failure)
                .event_key
                .trimmed(),
            FIXTURE_CARD
        );
    }

    #[test]
    fn a_detail_insert_the_server_cancels_keeps_the_summary_written_before_it() {
        // P-COPAUA0C-013, P-COPAUA0C-014, P-COPAUA0C-035: another session holds
        // the detail table until the test releases it. The server cancels the
        // detail insert (I004), the summary written before it is committed,
        // and the write ends while the lock is still held, so neither the
        // insert nor the COMMIT waits for the lock.
        //
        // The lock session is one DO statement: it locks the table, then polls
        // cp00_lock_release until the test inserts the release row, so it
        // holds the lock for as long as the test needs, however late the test
        // runs. The write starts only once pg_locks shows the lock granted.
        // Every wait is bounded, and a lock session that fails or ends before
        // its release fails the test with its cause, so nothing waits forever.
        //
        // The flow's pool applies the statement timeout to every statement of
        // the flow, its reads and the summary write included. The lock is held
        // until the release however long the insert waits, so the timeout is
        // set long enough for those statements to finish on a loaded host.
        const STATEMENT_TIMEOUT: Duration = Duration::from_secs(5);
        // The longest the lock may take to be granted, and the lock session
        // to end once released.
        const SESSION_WAIT: Duration = Duration::from_secs(30);
        // The longest the write may take: every call of it within its
        // database deadline, plus a margin for the test's own scheduling.
        const WRITE_LIMIT: Duration = POST_ACK_BUDGET.saturating_add(Duration::from_secs(5));
        let stack = Arc::new(carddemo_testkit::block_on(TestStack::postgres()));
        carddemo_testkit::block_on(async {
            stack.seed().await.expect("the fixture state loads");
            let db = connect_with_statement_timeout(stack.database_url(), STATEMENT_TIMEOUT)
                .await
                .expect("the pool opens");
            let flow = Cp00Flow::new(db, pinned_clock());
            let body = request_text(FIXTURE_CARD, "10.00", "TXN000000000001");
            let req = request(&body, Some("CP00.REPLY.QUEUE"));
            let Disposition::Reply { after_ack, .. } = flow.handle(&req).await else {
                panic!("a found card within its limit is answered");
            };

            stack
                .exec_sql("CREATE TABLE cp00_lock_release (released boolean NOT NULL)")
                .await;
            let locker = Arc::clone(&stack);
            let lock = tokio::spawn(async move {
                // Without a transaction block the statement runs in a
                // transaction of its own, which holds the lock until the
                // statement ends and rolls back on any error, releasing it.
                // In READ COMMITTED each poll reads a fresh snapshot, so it
                // sees the committed release row. The statement gives up after
                // 300 seconds, longer than SESSION_WAIT and WRITE_LIMIT
                // together, so it ends unreleased only after the test failed.
                locker
                    .exec_sql(
                        "DO $$ \
                         DECLARE give_up timestamptz := \
                           clock_timestamp() + interval '300 seconds'; \
                         BEGIN \
                           LOCK TABLE pending_auth_details IN ACCESS EXCLUSIVE MODE; \
                           WHILE NOT EXISTS (SELECT 1 FROM cp00_lock_release) LOOP \
                             IF clock_timestamp() > give_up THEN \
                               RAISE EXCEPTION 'no release row within 300 seconds'; \
                             END IF; \
                             PERFORM pg_sleep(0.02); \
                           END LOOP; \
                         END $$",
                    )
                    .await;
            });
            let granted = "SELECT count(*) FROM pg_locks JOIN pg_class ON pg_class.oid = \
                           pg_locks.relation WHERE pg_class.relname = 'pending_auth_details' \
                           AND pg_locks.mode = 'AccessExclusiveLock' AND pg_locks.granted";
            let waiting = Instant::now();
            while stack.fetch_text_rows(granted).await != [[Some("1".to_owned())]] {
                if lock.is_finished() {
                    await_lock_session(lock, SESSION_WAIT).await;
                    panic!("the lock session ended before its lock was seen granted");
                }
                assert!(
                    waiting.elapsed() < SESSION_WAIT,
                    "the lock was not granted within {SESSION_WAIT:?}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }

            let started = Instant::now();
            tokio::time::timeout(WRITE_LIMIT, flow.after_ack(&req, after_ack))
                .await
                .unwrap_or_else(|_| panic!("the write did not end within {WRITE_LIMIT:?}"));
            let took = started.elapsed();
            assert!(took >= STATEMENT_TIMEOUT, "the write took {took:?}");
            // Nothing has released the lock yet, so a lock session still
            // running proves the write ended while the lock was held.
            if lock.is_finished() {
                await_lock_session(lock, SESSION_WAIT).await;
                panic!("the lock session ended before its release");
            }
            assert_eq!(
                stack
                    .fetch_text_rows(
                        "SELECT pa_approved_auth_cnt, pa_approved_auth_amt \
                         FROM pending_auth_summaries WHERE pa_acct_id = 50"
                    )
                    .await,
                [[Some("1".to_owned()), Some("10.00".to_owned())]]
            );
            stack
                .exec_sql("INSERT INTO cp00_lock_release VALUES (true)")
                .await;
            await_lock_session(lock, SESSION_WAIT).await;
            assert_eq!(
                stack
                    .fetch_text_rows("SELECT count(*) FROM pending_auth_details")
                    .await,
                [[Some("0".to_owned())]]
            );
        });
    }

    /// Awaits the lock session `lock` for at most `limit`, and panics with the
    /// cause when it failed, its panic naming the database error, or when it
    /// is still running after `limit`.
    async fn await_lock_session(lock: tokio::task::JoinHandle<()>, limit: Duration) {
        let Ok(ended) = tokio::time::timeout(limit, lock).await else {
            panic!("the lock session did not end within {limit:?}");
        };
        if let Err(error) = ended {
            panic!("the lock session failed: {error}");
        }
    }
}
