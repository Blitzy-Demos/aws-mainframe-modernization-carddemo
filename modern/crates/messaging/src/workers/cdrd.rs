//! CDRD, the port of `CODATE01`: every request taken from the configured
//! `CARDDEMO_QUEUE_REQUEST_DATE` queue is answered with the system date and time on
//! `CARD.DEMO.REPLY.DATE` (AAP 0.3.4.2; `modern/audit/messaging/CODATE01.md`).
//!
//! This module orchestrates and owns no byte layout and no broker rule:
//!
//! | Step | COBOL | Owner | Rules |
//! |---|---|---|---|
//! | take the next request with `basic_get` | `3000-GET-REQUEST` | `Channels::next` | CODATE01-001 |
//! | build the 1,000-byte reply from one clock reading | `4000-PROCESS-REQUEST-REPLY` | `domain::messaging::codate01::reply_body` | CODATE01-003 to CODATE01-009, CODATE01-013 |
//! | publish to the fixed reply queue with the request's IDs, then acknowledge | `4100-PUT-REPLY` | `super::reply_fixed` with [`Flow::Cdrd`](crate::error_body::Flow::Cdrd) | CODATE01-002, CODATE01-010 to CODATE01-012 |
//! | route `MQ-ERR-DISPLAY` to `CARD.DEMO.ERROR` when the reply is not confirmed | `9000-ERROR` | `super::reply_fixed`, `crate::error_body` | CODATE01-014, CODATE01-015, CODATE01-018, CODATE01-019 |
//!
//! # The request is never read
//!
//! `CODATE01` copies the request into `REQUEST-MSG-COPY` and never reads it again, so the reply
//! does not depend on it (CODATE01-001). The worker does not decode, inspect or log the body: the
//! only request data it touches are the `message_id` and `correlation_id` the reply copies
//! (CODATE01-010) and the span records, and the delivery tag that settles it. No byte of the body
//! reaches a reply, an error message, an event or a span (CODATE01-009; Cardholder Data
//! Redaction). The request's `reply_to` is ignored: the reply always goes to
//! `domain::messaging::codate01::REPLY_QUEUE` (CODATE01-002).
//!
//! # Queue names
//!
//! The request queue comes only from `runtime::config::Config::queue_request_date`, because the
//! COBOL takes it from the trigger message and holds no literal for it. The reply and error
//! queues are the COBOL literals `CARD.DEMO.REPLY.DATE` and `CARD.DEMO.ERROR`, owned by
//! `domain::messaging::codate01`; the README's queue names are not used (E01).
//!
//! # One request, one task
//!
//! Each request is processed as the first message of a CICS task, and
//! [`Worker`](crate::workers::cdrd::Worker) keeps no state from one request to the next: it holds
//! its channels, its queue name and its clock, never a request, a body or an ID (CODATE01-013;
//! E14, E15). The date and time come only from the injected [`Clock`](domain::Clock), read once
//! per reply, so the date and the time describe one instant (CODATE01-008).
//!
//! The module's links are written as full paths because the `pub mod cdrd;` declaration carries
//! an outer doc comment, and rustdoc resolves this merged documentation from the parent module.
//!
//! # Error bodies
//!
//! The `MQ-ERR-DISPLAY` body is `crate::error_body::render`'s, published by `super::reply_fixed`.
//! Its MQ condition and reason codes are HALTED(H4): this worker never fabricates them, and the
//! `MQOPEN`, `MQCLOSE` and `CICS RETRIEVE` artefacts of the COBOL error paths have no counterpart
//! here (E17).

use std::fmt;
use std::sync::Arc;

use domain::Clock;
use domain::messaging::codate01;
use lapin::Connection;
use runtime::config::Config;
use tracing::Instrument;

use super::{Channels, Processed, Step, StepError};
use crate::broker::{Acknowledger, AmqpError, Consumer};
use crate::error_body::Flow;

/// The CDRD worker: one consumer and one publisher channel, the configured request queue and the
/// injected clock (CODATE01-008, CODATE01-013).
///
/// `A` settles requests; production uses the [`Consumer`] channel itself, and tests substitute a
/// failing or recording [`Acknowledger`] through [`Worker::with_acknowledger`].
pub struct Worker<A: Acknowledger = Consumer> {
    channels: Channels<A>,
    request_queue: String,
    clock: Arc<dyn Clock>,
}

impl Worker<Consumer> {
    /// Opens the worker's channels on `connection`: the consumer channel, a passive declare of
    /// the request queue `config.queue_request_date`, `CARD.DEMO.REPLY.DATE` and
    /// `CARD.DEMO.ERROR`, then the publisher channel in confirm mode. `clock` supplies the date
    /// and time of every reply (CODATE01-008).
    ///
    /// # Errors
    ///
    /// The [`AmqpError`] of a channel that could not be opened or put in confirm mode, or
    /// [`AmqpError::Declare`] naming the first of the three queues that does not exist.
    pub async fn open(
        connection: &Connection,
        config: &Config,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, AmqpError> {
        let channels = Channels::open(
            connection,
            &[
                config.queue_request_date.as_str(),
                codate01::REPLY_QUEUE,
                codate01::ERROR_QUEUE,
            ],
        )
        .await?;
        Ok(Self {
            channels,
            request_queue: config.queue_request_date.clone(),
            clock,
        })
    }
}

impl<A: Acknowledger> Worker<A> {
    /// This worker with `acknowledger` settling every request in place of the consumer channel,
    /// the seam of the failure-injection tests. The channels, queue and clock are kept.
    #[must_use]
    pub fn with_acknowledger<B: Acknowledger>(self, acknowledger: B) -> Worker<B> {
        Worker {
            channels: self.channels.with_acknowledger(acknowledger),
            request_queue: self.request_queue,
            clock: self.clock,
        }
    }
}

impl<A: Acknowledger> fmt::Debug for Worker<A> {
    /// The request queue only: channels and clock have nothing a reader of a log needs.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker")
            .field("request_queue", &self.request_queue)
            .finish_non_exhaustive()
    }
}

impl<A: Acknowledger> Step for Worker<A> {
    fn queue(&self) -> &str {
        &self.request_queue
    }

    /// Takes the next request and answers it inside its `mq.message` span: the reply body from
    /// one clock reading, published to `CARD.DEMO.REPLY.DATE` with the request's IDs and then
    /// acknowledged, or the `MQPUT ERR` body routed to `CARD.DEMO.ERROR` when the reply is not
    /// confirmed (CODATE01-001, CODATE01-002, CODATE01-010 to CODATE01-012, CODATE01-014,
    /// CODATE01-018, CODATE01-019). The body is never read.
    async fn process_next(&mut self, connection: &Connection) -> Result<Processed, StepError> {
        let Self {
            channels,
            request_queue,
            clock,
        } = self;
        let Some(request) = channels.next(request_queue).await? else {
            return Ok(Processed::Idle);
        };
        let span = super::message_span(request_queue, &request);
        let processed = async {
            let body = codate01::reply_body(clock.as_ref());
            super::reply_fixed(channels, connection, &request, Flow::Cdrd, &body).await
        }
        .instrument(span)
        .await;
        Ok(processed)
    }

    async fn reopen(&mut self, connection: &Connection) -> Result<(), AmqpError> {
        self.channels.reopen(connection).await
    }
}
