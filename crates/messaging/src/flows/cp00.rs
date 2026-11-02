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
//! 1. [`Flow::handle`]: the 500-byte buffer check, the parse and the amount
//!    (`2100-EXTRACT-REQUEST-MSG`), the reads in a read-only unit of work
//!    (`5100` to `5500`), the decision and the reply (`6000`, `7100`);
//! 2. the consumer publishes the reply, waits for its confirm and acknowledges
//!    the request;
//! 3. [`Flow::after_ack`]: the summary and detail writes (`8000`), in a unit of
//!    work of their own, so the reply and the data stay apart (AAP 0.3.3).
//!
//! Every `9500-LOG-ERROR` record is logged as one JSON line with the
//! `CCPAUERY` fields; `carddemo_runtime::telemetry` formats it. CP00 has no
//! error queue: a request without a confirmed reply is logged and rejected
//! without requeue, which consumes it as the COBOL's no-syncpoint get does
//! (P-COPAUA0C-002).
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
//! `auth_decision::Unproven` naming its `halt` escalation. A request amount
//! of that kind gets no reply; a write plan of that kind writes nothing. Both
//! are logged with the escalation ID (AAP 0.9.6).

use carddemo_domain::clock::{self, AnyClock, Clock};
use carddemo_domain::numeric::Money;
use carddemo_domain::ports::{AuthorizationStore, PortError};
use carddemo_domain::rules::auth_decision::{
    self, AuthContext, AuthDecision, AuthRequest, AuthWritePlan, ErrorLevel, ErrorLogEntry,
    SummaryWrite, Unproven, WriteStep,
};
use carddemo_persistence::Db;

use crate::consumer::{Disposition, FailureAction, Flow, PublishFailure, Reply, Request};
use crate::ids;

/// COPAUA0C 2100-EXTRACT-REQUEST-MSG — P-COPAUA0C-004: the paragraph a
/// request with an unproven amount is rejected at, recorded as the message
/// span's `location`.
const EXTRACT_PARAGRAPH: &str = "2100-EXTRACT-REQUEST-MSG";

/// COPAUA0C 8000-WRITE-AUTH-TO-DB — P-COPAUA0C-035: the paragraph a write
/// plan the audit cannot pin is skipped at, recorded in its log event.
const WRITE_PARAGRAPH: &str = "8000-WRITE-AUTH-TO-DB";

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
    /// when its amount is unproven, when a read fails critically (`C001` to
    /// `C003`, `I002`) or when it names no reply queue (`M004`): each of those
    /// ends the COBOL task before or at `7100-SEND-RESPONSE`, after its
    /// no-syncpoint get has consumed the request.
    async fn handle(&self, req: &Request) -> Disposition<PendingWrite> {
        let (request, amount) = match self.extract(req) {
            Ok(extracted) => extracted,
            Err(location) => return Disposition::Reject { location },
        };
        let context = match self.read(&request).await {
            Ok(context) => context,
            Err(location) => return Disposition::Reject { location },
        };
        match respond(req, request, amount, context) {
            Ok((reply, after_ack)) => Disposition::Reply { reply, after_ack },
            Err(entry) => {
                self.log_entry(&entry);
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
    fn on_reply_failure(&self, _req: &Request, failure: &PublishFailure) -> FailureAction {
        let entry = reply_failure_entry_for(failure);
        self.log_entry(&entry);
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
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L463-465]; a plan the
    /// audit cannot pin (a declined found card, E-41) is logged with its
    /// escalation and writes nothing. The request is already acknowledged, so
    /// nothing here reaches the broker.
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
            Ok(Some(plan)) => self.write(&plan).await,
            Ok(None) => {}
            Err(unproven) => log_unproven(req, unproven, WRITE_PARAGRAPH),
        }
    }
}

impl Cp00Flow {
    /// COPAUA0C 3100-READ-REQUEST-MQ / 2100-EXTRACT-REQUEST-MSG — P-COPAUA0C-002,
    /// P-COPAUA0C-004, P-COPAUA0C-016, P-COPAUA0C-025
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L351-380].
    ///
    /// A body longer than `W01-GET-BUFFER PIC X(500)` fails the COBOL get with
    /// the critical `M003` record and no reply. Otherwise the request is
    /// parsed and its amount converted. Returns the location the request is
    /// rejected at when it gets no reply.
    fn extract(&self, req: &Request) -> Result<(AuthRequest, Money<10>), &'static str> {
        if let Some(entry) = auth_decision::oversize_request_entry(&req.body) {
            self.log_entry(&entry);
            return Err(entry.location);
        }
        let request = auth_decision::parse_request(&req.body);
        match auth_decision::parse_amount(&request) {
            Ok(amount) => Ok((request, amount)),
            Err(unproven) => {
                log_unproven(req, unproven, EXTRACT_PARAGRAPH);
                Err(EXTRACT_PARAGRAPH)
            }
        }
    }

    /// COPAUA0C 5100-READ-XREF-RECORD to 5500-READ-AUTH-SUMMRY — P-COPAUA0C-005
    /// to P-COPAUA0C-008, P-COPAUA0C-034
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L472-641].
    ///
    /// Runs the reads in a unit of work that is rolled back afterwards, since
    /// it changes nothing, and logs their records in the order raised. The
    /// warnings `A001` to `A003` go on to the decline. A critical record ends
    /// the COBOL task through `9990-END-ROUTINE` before any reply, so its
    /// location is returned and the request is rejected. A unit of work that
    /// cannot be opened stands in for the failed first read: `C001`.
    async fn read(&self, request: &AuthRequest) -> Result<AuthContext, &'static str> {
        let mut uow = match self.db.begin().await {
            Ok(uow) => uow,
            Err(error) => {
                tracing::warn!(flow = Self::NAME, error = %error, "read unit of work not opened");
                let entry = auth_decision::xref_read_failure_entry(
                    &request.pa_rq_card_num,
                    &PortError::from(error),
                );
                self.log_entry(&entry);
                return Err(entry.location);
            }
        };
        let context = auth_decision::gather(&mut uow.store(), request).await;
        if let Err(error) = uow.rollback().await {
            tracing::warn!(flow = Self::NAME, error = %error, "read unit of work not rolled back");
        }
        for entry in &context.log {
            self.log_entry(entry);
        }
        match context.critical_entry() {
            Some(entry) => Err(entry.location),
            None => Ok(context),
        }
    }

    /// COPAUA0C 8000-WRITE-AUTH-TO-DB — P-COPAUA0C-035: runs `plan` in a unit
    /// of work of its own and commits it.
    ///
    /// The commit stands for the `EXEC CICS RETURN` (after `I003` or `I004`) or
    /// the loop `SYNCPOINT` (after both writes) that commits the COBOL task, so
    /// a summary written before a failed detail insert persists, as the
    /// audit's "persists on later failure" column records. Each write runs
    /// under its own savepoint in the store. A unit of work that cannot be
    /// opened or committed loses the summary write, so it is logged as `I003`.
    async fn write(&self, plan: &AuthWritePlan) {
        let mut uow = match self.db.begin().await {
            Ok(uow) => uow,
            Err(error) => {
                self.log_entry(&auth_decision::write_failure_entry(
                    WriteStep::Summary,
                    &PortError::from(error),
                    plan,
                ));
                return;
            }
        };
        if let Some(entry) = write_segments(&mut uow.store(), plan).await {
            self.log_entry(&entry);
        }
        if let Err(error) = uow.commit().await {
            self.log_entry(&auth_decision::write_failure_entry(
                WriteStep::Summary,
                &PortError::from(error),
                plan,
            ));
        }
    }

    /// COPAUA0C 9500-LOG-ERROR — P-COPAUA0C-015, P-COPAUA0C-031: logs `entry`
    /// stamped with the flow's clock.
    fn log_entry(&self, entry: &ErrorLogEntry) {
        log_record(entry, &self.clock);
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
/// A card the cross-reference lacks is answered too, with the `3100` decline
/// the domain gives it (the E-41 proposal that section D row P-COPAUA0C-026
/// records); only its write is skipped.
///
/// # Errors
///
/// The `M004` record when the request names no reply queue: the `MQPUT1` to
/// a blank `MQOD-OBJECTNAME` fails, and its AMQP counterpart is a publish the
/// broker returns as unroutable, [`PublishFailure::Returned`].
fn respond(
    req: &Request,
    request: AuthRequest,
    amount: Money<10>,
    context: AuthContext,
) -> Result<(Reply, PendingWrite), ErrorLogEntry> {
    let decision = auth_decision::decide(amount, &context);
    let Some(queue) = reply_queue(req) else {
        return Err(reply_failure_entry_for(&PublishFailure::Returned));
    };
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

/// COPAUA0C 3100-READ-REQUEST-MQ / 7100-SEND-RESPONSE — P-COPAUA0C-002,
/// P-COPAUA0C-012: the reply queue, `MQMD-REPLYTOQ` saved as `WS-REPLY-QNAME`
/// and moved to `MQOD-OBJECTNAME`
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L413]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L742].
///
/// MQ object names are blank-padded `X(48)`, so trailing spaces are padding
/// and are removed; an AMQP queue name matches only exactly. `None` when
/// `reply_to` is absent, empty or only spaces.
fn reply_queue(req: &Request) -> Option<&str> {
    req.reply_to
        .as_deref()
        .map(|name| name.trim_end_matches(' '))
        .filter(|name| !name.is_empty())
}

/// COPAUA0C 7100-SEND-RESPONSE — P-COPAUA0C-012: the `M004` record of a reply
/// that `failure` kept from being delivered, with the completion and reason
/// codes the failure maps to (E-39).
fn reply_failure_entry_for(failure: &PublishFailure) -> ErrorLogEntry {
    let (condition_code, reason_code) = failure.mq_codes();
    auth_decision::reply_failure_entry(i64::from(condition_code), i64::from(reason_code))
}

/// COPAUA0C 8400-UPDATE-SUMMARY / 8500-INSERT-AUTH — P-COPAUA0C-013,
/// P-COPAUA0C-014, P-COPAUA0C-035
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L824-847]
/// [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L913-932].
///
/// Writes the summary (`REPL` when `5500` found one, `ISRT` otherwise), then
/// the detail under it. A failed summary write is `I003`, which ends the
/// COBOL task before the detail insert, so the detail is not attempted; a
/// failed detail insert is `I004`. Returns the record of the failed write,
/// `None` when both were written.
async fn write_segments<S: AuthorizationStore>(
    store: &mut S,
    plan: &AuthWritePlan,
) -> Option<ErrorLogEntry> {
    let summary = match &plan.summary {
        SummaryWrite::Insert(summary) => store.insert_summary(summary.clone()).await,
        SummaryWrite::Replace(summary) => store.update_summary(summary.clone()).await,
    };
    if let Err(error) = summary {
        return Some(auth_decision::write_failure_entry(
            WriteStep::Summary,
            &error,
            plan,
        ));
    }
    store
        .insert_detail(plan.detail.clone())
        .await
        .err()
        .map(|error| auth_decision::write_failure_entry(WriteStep::Detail, &error, plan))
}

/// Logs an input class whose COBOL result the audit cannot pin: the
/// escalation that holds it at `halt` and the paragraph whose output is
/// skipped for it, the reply at `2100` or the writes at `8000` (AAP 0.9.6).
fn log_unproven(req: &Request, unproven: Unproven, paragraph: &'static str) {
    tracing::error!(
        flow = Cp00Flow::NAME,
        message_id = req.message_id.as_deref().unwrap_or(""),
        escalation = unproven.escalation_id,
        paragraph,
        "COPAUA0C result unproven: the paragraph's output is skipped"
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
/// The event key is logged as the record carries it, the full card number of
/// `A001`, `C001` and `I004` included, because the replacement keeps the
/// `CSSL` record's content; the log stays on the local JSON stdout.
fn log_record(entry: &ErrorLogEntry, clock: &impl Clock) {
    let now = clock.now();
    let err_date = clock::yymmdd(now);
    let err_time = clock::hhmmss_compact(now);
    let err_level = char::from(entry.level.code());
    let err_subsystem = char::from(entry.source.code());
    let err_code_1 = entry.code_1.trimmed();
    let err_code_2 = entry.code_2.trimmed();
    let err_event_key = entry.event_key.trimmed();
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
                "ERROR-LOG-RECORD"
            )
        };
    }
    match entry.level {
        ErrorLevel::Critical => error_log_record!(error),
        ErrorLevel::Warning => error_log_record!(warn),
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::io;
    use std::pin::pin;
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Waker};

    use carddemo_domain::clock::{AnyClock, Clock};
    use carddemo_domain::entities::{
        Account, AuthFraud, CardXref, Customer, NewPendingAuthDetail, PendingAuthDetail,
        PendingAuthSummary,
    };
    use carddemo_domain::messages;
    use carddemo_domain::numeric::{
        AccountGroupId, AccountId, Alnum, AuthId, CardNumber, CustSsn, CustomerId, FicoScore, Money,
    };
    use carddemo_domain::ports::{AuthorizationStore, Browse, Direction, PortError, PortResult};
    use carddemo_domain::rules::auth_decision::{
        self, AuthContext, AuthWritePlan, ErrorLevel, ErrorLogEntry, ErrorSource, Lookup,
        SummaryWrite,
    };

    use super::{
        Cp00Flow, PendingWrite, log_record, reply_failure_entry_for, reply_queue, respond,
        write_segments,
    };
    use crate::consumer::{Flow, PublishFailure, Reply, Request};

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

    /// Customer 50 as `custdata.txt` L50 decodes.
    fn fixture_customer() -> Customer {
        Customer {
            cust_id: CustomerId::new(50).expect("a customer number"),
            cust_first_name: Alnum::move_from("Aniya"),
            cust_middle_name: Alnum::move_from("Von"),
            cust_last_name: Alnum::move_from("Kessler"),
            cust_addr_line_1: Alnum::move_from("618 Deshaun Route"),
            cust_addr_line_2: Alnum::move_from("Apt. 802"),
            cust_addr_line_3: Alnum::move_from("Altenwerthshire"),
            cust_addr_state_cd: Alnum::move_from("NC"),
            cust_addr_country_cd: Alnum::move_from("USA"),
            cust_addr_zip: Alnum::move_from("12546"),
            cust_phone_num_1: Alnum::move_from("(908)119-8310"),
            cust_phone_num_2: Alnum::move_from("(373)693-8684"),
            cust_ssn: CustSsn::new(20_973_888).expect("a nine-digit number"),
            cust_govt_issued_id: Alnum::move_from("00000000000049368437"),
            cust_dob_yyyy_mm_dd: Alnum::move_from("1961-06-08"),
            cust_eft_account_id: Alnum::move_from("0053581756"),
            cust_pri_card_holder_ind: Alnum::move_from("Y"),
            cust_fico_credit_score: FicoScore::new(274).expect("a three-digit score"),
        }
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

    /// Parses `body` and answers it against `context`, as `handle` does after
    /// the reads.
    fn answer(req: &Request, context: AuthContext) -> Result<(Reply, PendingWrite), ErrorLogEntry> {
        let request = auth_decision::parse_request(&req.body);
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
    }

    impl FakeStore {
        fn failing(call: &'static str) -> Self {
            Self {
                calls: Vec::new(),
                failing: Some(call),
            }
        }

        /// Records `call` and answers it: the injected failure when it is the
        /// failing call, `value` otherwise.
        fn answer<T>(&mut self, call: &'static str, value: T) -> PortResult<T> {
            self.calls.push(call);
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

        async fn insert_summary(&mut self, _summary: PendingAuthSummary) -> PortResult<()> {
            self.answer("insert_summary", ())
        }

        async fn update_summary(&mut self, _summary: PendingAuthSummary) -> PortResult<()> {
            self.answer("update_summary", ())
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
            self.answer("insert_detail", AuthId::from(1))
        }

        async fn read_detail(&mut self, _id: AuthId) -> PortResult<PendingAuthDetail> {
            self.outside_8000("read_detail")
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
            _id: AuthId,
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

    /// The JSON line `log_record` writes for `entry` through the binaries'
    /// telemetry subscriber, at the pinned instant.
    fn logged_line(entry: &ErrorLogEntry) -> String {
        let capture = Capture::default();
        let sink = capture.clone();
        let (subscriber, guard) =
            carddemo_runtime::telemetry::subscriber_with_writer("mq", "info", move || sink.clone())
                .expect("valid directives");
        tracing::subscriber::with_default(subscriber, || log_record(entry, &pinned_clock()));
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
    fn the_reply_queue_is_the_reply_to_without_its_blank_padding() {
        // P-COPAUA0C-002, P-COPAUA0C-012: WS-REPLY-QNAME X(48) to MQOD-OBJECTNAME.
        let cases = [
            (None, None),
            (Some(""), None),
            (Some("     "), None),
            (Some("REPLY.Q   "), Some("REPLY.Q")),
            (Some("REPLY.Q"), Some("REPLY.Q")),
            (Some("AMQ.GEN-x y"), Some("AMQ.GEN-x y")),
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
    fn an_unknown_card_is_answered_and_plans_no_write() {
        // P-COPAUA0C-026 (section D, the E-41 proposal), P-COPAUA0C-035: the
        // reply is sent and 8000 is not performed.
        let body = request_text(UNKNOWN_CARD, "100.00", "TXN000000000002");
        let req = request(&body, Some("CP00.REPLY.QUEUE"));
        let (reply, work) = answer(&req, unknown_card_context()).expect("a reply");
        assert_eq!(
            reply.body,
            b"9999999999999999,TXN000000000002,120000,05,3100,          0.00, "
        );
        assert_eq!(reply.correlation_id, req.correlation_id);
        assert!(!work.decision.approved);
        assert!(plan_of(&work).is_none());
    }

    #[test]
    fn a_request_without_a_reply_queue_is_the_m004_record() {
        // P-COPAUA0C-012: MQPUT1 to a blank reply-to queue fails, M004, and no
        // reply exists; the codes are those of a returned publish (E-39).
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let (condition_code, reason_code) = PublishFailure::Returned.mq_codes();
        for reply_to in [None, Some(""), Some("   ")] {
            let req = request(&body, reply_to);
            let entry = answer(&req, fixture_context()).expect_err("no reply queue");
            assert_eq!(entry.location, "M004", "{reply_to:?}");
            assert_eq!(entry.level, ErrorLevel::Critical);
            assert_eq!(entry.source, ErrorSource::Mq);
            assert_eq!(entry.message, messages::FAILED_TO_PUT_ON_REPLY_MQ);
            assert_eq!(entry.code_1.to_string(), format!("{condition_code:09}"));
            assert_eq!(entry.code_2.to_string(), format!("{reason_code:09}"));
        }
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

    /// The plan of the P-COPAUA0C-021 approval, with the summary `5500` found
    /// when `summary_found`.
    fn approved_plan(summary_found: bool) -> AuthWritePlan {
        let body = request_text(FIXTURE_CARD, "100.00", "TXN000000000001");
        let req = request(&body, Some("CP00.REPLY.QUEUE"));
        let mut context = fixture_context();
        if summary_found {
            let found = match plan_of(&answer(&req, fixture_context()).expect("a reply").1) {
                Some(AuthWritePlan {
                    summary: SummaryWrite::Insert(summary),
                    ..
                }) => summary,
                other => panic!("a new summary: {other:?}"),
            };
            context.summary = Lookup::Found(found);
        }
        let (_, work) = answer(&req, context).expect("a reply");
        plan_of(&work).expect("a found card is written")
    }

    #[test]
    fn both_segments_are_written_summary_first() {
        // P-COPAUA0C-035, P-COPAUA0C-013, P-COPAUA0C-014: ISRT or REPL of the
        // summary, then ISRT of the detail; nothing is logged.
        for (summary_found, summary_call) in [(false, "insert_summary"), (true, "update_summary")] {
            let plan = approved_plan(summary_found);
            let mut store = FakeStore::default();
            assert_eq!(ready_now(write_segments(&mut store, &plan)), None);
            assert_eq!(store.calls, [summary_call, "insert_detail"]);
        }
    }

    #[test]
    fn a_failed_summary_write_is_i003_and_skips_the_detail() {
        // P-COPAUA0C-013, P-COPAUA0C-035: I003 ends the task before 8500; its
        // event key PA-CARD-NUM is not set yet (E-25).
        for (summary_found, summary_call) in [(false, "insert_summary"), (true, "update_summary")] {
            let plan = approved_plan(summary_found);
            let mut store = FakeStore::failing(summary_call);
            let entry = ready_now(write_segments(&mut store, &plan)).expect("an I003 record");
            assert_eq!(store.calls, [summary_call]);
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
        let plan = approved_plan(false);
        let mut store = FakeStore::failing("insert_detail");
        let entry = ready_now(write_segments(&mut store, &plan)).expect("an I004 record");
        assert_eq!(store.calls, ["insert_summary", "insert_detail"]);
        assert_eq!(entry.location, "I004");
        assert_eq!(entry.level, ErrorLevel::Critical);
        assert_eq!(entry.source, ErrorSource::Ims);
        assert_eq!(entry.message, messages::IMS_INSERT_DETL_FAILED);
        assert_eq!(entry.event_key.trimmed(), FIXTURE_CARD);
    }

    #[test]
    fn an_error_log_record_is_one_json_line_of_ccpauery_fields() {
        // P-COPAUA0C-015, P-COPAUA0C-031: ERR-DATE YYMMDD and ERR-TIME HHMMSS of
        // the pinned instant, application CP00, program COPAUA0C; a critical
        // record is an ERROR event, a warning a WARN event.
        let critical = reply_failure_entry_for(&PublishFailure::Returned);
        let warning = unknown_card_context().log.remove(0);
        let cases = [
            (
                &critical,
                "\"level\":\"ERROR\"",
                [
                    "\"err_location\":\"M004\"",
                    "\"err_level\":\"C\"",
                    "\"err_subsystem\":\"M\"",
                    "\"err_message\":\"FAILED TO PUT ON REPLY MQ\"",
                    "\"err_event_key\":\"\"",
                ],
            ),
            (
                &warning,
                "\"level\":\"WARN\"",
                [
                    "\"err_location\":\"A001\"",
                    "\"err_level\":\"W\"",
                    "\"err_subsystem\":\"A\"",
                    "\"err_message\":\"CARD NOT FOUND IN XREF\"",
                    "\"err_event_key\":\"9999999999999999\"",
                ],
            ),
        ];
        for (entry, level, fields) in cases {
            let line = logged_line(entry);
            let common = [
                level,
                "\"message\":\"ERROR-LOG-RECORD\"",
                "\"err_date\":\"220701\"",
                "\"err_time\":\"120000\"",
                "\"err_application\":\"CP00\"",
                "\"err_program\":\"COPAUA0C\"",
            ];
            for expected in common.iter().chain(&fields) {
                assert!(line.contains(expected), "{expected} missing from {line}");
            }
        }
        let codes = logged_line(&critical);
        assert!(codes.contains(&format!("\"err_code_1\":\"{}\"", critical.code_1.trimmed())));
        assert!(codes.contains(&format!("\"err_code_2\":\"{}\"", critical.code_2.trimmed())));
    }
}
