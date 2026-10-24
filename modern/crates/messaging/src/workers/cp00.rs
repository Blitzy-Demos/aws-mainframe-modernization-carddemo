//! CP00, the port of `COPAUA0C`: every request taken from the configured
//! `CARDDEMO_QUEUE_REQUEST_PAUTH` queue is decided, answered on the request's `reply_to` and then
//! recorded (AAP 0.3.4.2; `modern/audit/messaging/COPAUA0C.md`).
//!
//! The worker owns the broker I/O, the authorization transaction and the order of the steps, and
//! nothing else. `domain::messaging::copaua0c` parses every body, makes every decision and builds
//! every reply, summary, detail and log record; `persistence` owns every SQL statement, reached
//! through the [`AuthStore`](crate::workers::cp00::AuthStore) and
//! [`AuthTx`](crate::workers::cp00::AuthTx) seam; `crate::broker` and `super::Channels` own the
//! AMQP rules.
//!
//! | Step | COBOL | Owner | Rules |
//! |---|---|---|---|
//! | take the next request with `basic_get` | `3100-READ-REQUEST-MQ` | `Channels::next` | COPAUA0C-001 |
//! | parse the body | `2100-EXTRACT-REQUEST-MSG` | `copaua0c::parse_request` | COPAUA0C-002 to COPAUA0C-017 |
//! | read the cross-reference | `5100-READ-XREF-RECORD` | [`AuthStore::find_xref`](crate::workers::cp00::AuthStore::find_xref) | COPAUA0C-020, COPAUA0C-021, COPAUA0C-114, COPAUA0C-122 |
//! | lock or create the summary, read account and customer | `5500`, `5200`, `5300` | [`AuthTx`](crate::workers::cp00::AuthTx) | COPAUA0C-022 to COPAUA0C-027, COPAUA0C-115, COPAUA0C-116, COPAUA0C-123 to COPAUA0C-126 |
//! | decide, with the i10 guard | `6000-MAKE-DECISION` | `copaua0c::decide` | COPAUA0C-040 to COPAUA0C-050 |
//! | publish the reply, confirm, acknowledge | `7100-SEND-RESPONSE` | `Channels::publish`, `Channels::ack` | COPAUA0C-060 to COPAUA0C-067, COPAUA0C-081, COPAUA0C-082, COPAUA0C-117, COPAUA0C-118 |
//! | update the summary, insert the detail, commit | `8400-UPDATE-SUMMARY`, `8500-INSERT-AUTH` | [`AuthTx`](crate::workers::cp00::AuthTx) | COPAUA0C-090 to COPAUA0C-103, COPAUA0C-119 to COPAUA0C-121 |
//! | write the `CSSL` row | `9500-LOG-ERROR` | `copaua0c::error_log`, [`AuthStore::log`](crate::workers::cp00::AuthStore::log) | COPAUA0C-110 to COPAUA0C-126 |
//!
//! # Reply, then write (E29)
//!
//! The COBOL puts the reply outside its unit of work before `8000-WRITE-AUTH-TO-DB`, and the
//! worker keeps that order and its exposure (COPAUA0C-080, COPAUA0C-081): for a card found in
//! the cross-reference it opens one `PostgreSQL` transaction, locks the account's summary row,
//! reads, decides, publishes, waits for the confirmation and acknowledges the request; only then
//! does it update the summary, insert the detail inside a savepoint and commit. A field error
//! and a card with no cross-reference row are answered with no lock and no authorization write
//! (COPAUA0C-082, COPAUA0C-103).
//!
//! # One lock serialises the decisions (i10)
//!
//! Every decision for an account waits on that account's summary row lock, taken by
//! `lock_or_create` inside the transaction and held until it commits or rolls back
//! (COPAUA0C-025, COPAUA0C-026). It is the only serialisation: no state is shared between
//! requests in the process, so two `carddemo-mq` processes and redelivered requests decide
//! against committed state. The worker never compares amounts; the guard is the domain's
//! (COPAUA0C-046 to COPAUA0C-049, E30).
//!
//! # Outcomes
//!
//! | Case | Reply | Log row | Settlement |
//! |---|---|---|---|
//! | card number unparseable | none; one alert event | none | dead-lettered (COPAUA0C-006) |
//! | field error | `05`/`9000` | none | acknowledged (COPAUA0C-012) |
//! | card not in the cross-reference | interim `05`/`3100` | `A001` | acknowledged (COPAUA0C-021, COPAUA0C-040) |
//! | card found | the decision | `A002`, `A003` when not found | acknowledged, then written |
//! | reply not confirmed; no `reply_to`, or one naming the Direct Reply-to pseudo-queue, neither published | none | `M004` | dead-lettered (COPAUA0C-117, `X-COPAUA0C-12`) |
//! | acknowledgement fails | confirmed | none | requeued by the broker, nothing written (COPAUA0C-118) |
//! | summary update fails | confirmed | `I003` | acknowledged, nothing written (COPAUA0C-119) |
//! | detail insert fails | confirmed | `I004` | acknowledged, summary committed alone (COPAUA0C-120) |
//! | commit refused by the server | confirmed | none; an `ERROR` event | acknowledged, rolled back, nothing written (COPAUA0C-121) |
//! | connection lost during the commit | confirmed | none; an `ERROR` event | acknowledged, outcome unknown: the summary update and any detail insert may have committed; not reconciled (COPAUA0C-121) |
//! | a store read fails | interim `05`/`9000` | `C001`, `C002`, `C003`, `I001` or `I002`, unless the database is unavailable | acknowledged, nothing written (COPAUA0C-122 to COPAUA0C-126, `X-COPAUA0C-1`) |
//!
//! A failed store read ends the COBOL task before its reply, with the request already removed by
//! the get outside syncpoint, so the COBOL sends none. i11 gives every removed request exactly
//! one reply and names `M004` as the only parsed request without one, so the worker writes the
//! audited row, rolls back and answers with `copaua0c::store_failure_reply`, a `05`/`9000`
//! decline confirmed and then acknowledged, with no authorization write. That is interim
//! handling, not a parity result: the second outcome of the open escalation candidate
//! `X-COPAUA0C-1` of the audit, pending the reviewer's and the user's decision.
//!
//! A read that failed with
//! [`PersistenceError::Unavailable`](persistence::PersistenceError::Unavailable) tells the
//! worker that the database cannot be reached, and every further `cssl_error_log` write of that
//! request would wait out another pool acquire before failing in turn. So no row is attempted
//! for the rest of the request, neither the read's row nor an `M004` row should the decline go
//! unconfirmed: one `ERROR` event stands for each row not written. The next request starts with
//! no such knowledge.
//!
//! # Redaction
//!
//! The reply's card field and every card-bearing log key come redacted from the domain (E32).
//! No event, span or error here holds a body byte, a card number or a record: events name the
//! outcome, a log location, a [`PersistenceError`](persistence::PersistenceError) (SQLSTATE and
//! constraint only) or a broker condition. The alert carries the body length and the request's
//! `message_id` and `correlation_id` as `runtime::telemetry::ScreenedText` shows them under the
//! worker's `runtime::telemetry::IdKey`, made from its `CARDDEMO_AMQP_URL`: the base64url
//! HMAC-SHA256 that the request's span records too, as the `Redaction` section of
//! `crate::workers` explains. The requester chooses both and either can be a card number, an SSN
//! or a password, so the alert never shows their text, nor the URL. The reply's `message_id`,
//! which `crate::ids` generates and which holds no requester text, is recorded verbatim on the
//! request's span as `reply_message_id` before the reply is published, so the settlement event,
//! an `M004` warning and the exported span all tie the request to its reply.
//!
//! The module's links are written as full paths, because the `pub mod cp00;` declaration carries
//! an outer doc comment and rustdoc resolves this merged documentation from the parent module.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

use copybook::{PicS, PicU};
use domain::messaging::copaua0c::{
    self, CardNumberError, CustomerLookup, Decision, FieldError, FoundContext, LogSite, Lookup,
    ParseError, SqlState, SummaryError, SummaryState,
};
use domain::records::{
    AccountRecord, AuthReply, AuthRequestRecord, CardXrefRecord, CustomerRecord, ErrorLogRecord,
    PendingAuthDetailRecord, PendingAuthSummaryRecord,
};
use domain::{CardNumber, Clock};
use lapin::Connection;
use persistence::{
    AccountRepository, CardXrefRepository, CsslErrorLogRepository, CustomerRepository,
    PendingAuthDetailRepository, PendingAuthSummaryRepository, PersistenceError, PgStore, PgTx,
};
use runtime::config::Config;
use runtime::telemetry::{self, IdKey};
use tracing::{Level, Metadata, field};

use super::{Channels, Processed, Step, StepError};
use crate::broker::{self, Acknowledger, AmqpError, Consumer, PublishFailure, QueueCheck, Request};
use crate::{EventSite, emit, event_metadata, event_metadata_with_target, ids};

/// The environment variable of the request queue, which the start-up refusal and a failed
/// passive declare of the request queue name in place of the configured value.
pub(super) const REQUEST_QUEUE_VARIABLE: &str = "CARDDEMO_QUEUE_REQUEST_PAUTH";

/// The store the CP00 worker reads and writes through: the cross-reference read, the
/// authorization transaction and the error log.
///
/// Production uses [`PgStore`]. Tests wrap it to inject a failure, for example a transaction
/// whose summary write fails (`I003`, COPAUA0C-119), and delegate every other method. The trait
/// is no repository of one entity: it is the worker's whole view of the database.
pub trait AuthStore: Send + Sync {
    /// The authorization transaction [`AuthStore::begin`] opens.
    type Tx: AuthTx;

    /// The `card_xref` row of `card`, or `None` when the card has none, the read of
    /// `5100-READ-XREF-RECORD` by all 16 bytes of the card number (COPAUA0C-020, COPAUA0C-021).
    /// It takes no lock, so it runs outside the authorization transaction.
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed read, CP00's `C001` case (COPAUA0C-122).
    fn find_xref(
        &self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send;

    /// Opens the authorization transaction of one request whose card has a `card_xref` row.
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed `BEGIN`, CP00's `I001` case, the nearest
    /// counterpart of a failed PSB schedule (COPAUA0C-126).
    fn begin(&self) -> impl Future<Output = Result<Self::Tx, PersistenceError>> + Send;

    /// Appends `record` to `cssl_error_log` in a transaction of its own, committed at once,
    /// never in the authorization transaction, so the row stands whether or not the
    /// authorization commits (COPAUA0C-110; AAP 0.5.2). Returns the row's `id`.
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed insert; the COBOL writes `CSSL` with `NOHANDLE`,
    /// and the worker only traces the failure.
    fn log(
        &self,
        record: &ErrorLogRecord,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send;
}

/// One open authorization transaction: the summary lock, the account and customer reads and the
/// writes after the acknowledgement.
///
/// Production uses [`PgTx`], opened by [`PgStore`]'s [`AuthStore::begin`]. A test double wraps it
/// and delegates every method it does not fail. Dropping the transaction without
/// [`AuthTx::commit`] rolls it back, which releases the summary lock with nothing written.
pub trait AuthTx: Send {
    /// Locks the summary of `template.acct_id`, creating it from `template` when the account has
    /// none, and returns the locked row with `true` when this call created it: the i10 mechanism,
    /// `INSERT … ON CONFLICT (acct_id) DO NOTHING` then `SELECT … FOR UPDATE` (COPAUA0C-025,
    /// COPAUA0C-026). The lock holds until the transaction ends.
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed insert or locking read, CP00's `I002` case
    /// (COPAUA0C-125).
    fn lock_or_create_summary(
        &mut self,
        template: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(PendingAuthSummaryRecord, bool), PersistenceError>> + Send;

    /// The account `id`, or `None` when it has no row (COPAUA0C-022).
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed read, CP00's `C002` case (COPAUA0C-123).
    fn find_account(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send;

    /// The customer `id`, or `None` when it has no row (COPAUA0C-023).
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed read, CP00's `C003` case (COPAUA0C-124).
    fn find_customer(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send;

    /// Rewrites the locked summary of `record.acct_id` with `record`, returning whether exactly
    /// one row changed (COPAUA0C-097).
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed update, CP00's `I003` case (COPAUA0C-119).
    fn update_summary(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Inserts `record` under the summary of `acct_id` inside a savepoint, released on success
    /// and rolled back on failure, so the transaction stays usable and its summary update can
    /// still commit (COPAUA0C-098 to COPAUA0C-102, COPAUA0C-120).
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed insert, such as
    /// [`PersistenceError::Duplicate`] for two authorizations in one millisecond, CP00's `I004`
    /// case (COPAUA0C-099, COPAUA0C-120).
    fn insert_detail(
        &mut self,
        acct_id: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Commits the transaction, which releases the summary lock.
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed commit (COPAUA0C-121). A commit the server refuses
    /// is rolled back, so nothing of the transaction is written. On
    /// [`PersistenceError::Unavailable`], a connection lost during the commit, whether the
    /// transaction committed is unknown: `PostgreSQL` can commit before its answer is lost.
    fn commit(self) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Rolls the transaction back, which releases the summary lock and removes a summary this
    /// transaction created.
    ///
    /// # Errors
    ///
    /// The [`PersistenceError`] of the failed rollback; `PostgreSQL` discards the transaction
    /// with its connection then.
    fn rollback(self) -> impl Future<Output = Result<(), PersistenceError>> + Send;
}

impl AuthStore for PgStore {
    type Tx = PgTx<'static>;

    /// `CardXrefRepository::find_by_id` on a pooled connection.
    fn find_xref(
        &self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send {
        let mut store = self.clone();
        async move { CardXrefRepository::find_by_id(&mut store, card).await }
    }

    /// [`PgStore::begin`].
    fn begin(&self) -> impl Future<Output = Result<PgTx<'static>, PersistenceError>> + Send {
        PgStore::begin(self)
    }

    /// `CsslErrorLogRepository::insert` on a pooled connection, which commits the row at once.
    fn log(
        &self,
        record: &ErrorLogRecord,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send {
        let mut store = self.clone();
        async move { CsslErrorLogRepository::insert(&mut store, record).await }
    }
}

impl AuthTx for PgTx<'static> {
    /// `PendingAuthSummaryRepository::lock_or_create` inside this transaction.
    fn lock_or_create_summary(
        &mut self,
        template: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(PendingAuthSummaryRecord, bool), PersistenceError>> + Send
    {
        PendingAuthSummaryRepository::lock_or_create(self, template)
    }

    /// `AccountRepository::find_by_id` inside this transaction.
    fn find_account(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
        AccountRepository::find_by_id(self, id)
    }

    /// `CustomerRepository::find_by_id` inside this transaction.
    fn find_customer(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send {
        CustomerRepository::find_by_id(self, id)
    }

    /// `PendingAuthSummaryRepository::update` inside this transaction.
    fn update_summary(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        PendingAuthSummaryRepository::update(self, record)
    }

    /// [`PgTx::savepoint`], then `PendingAuthDetailRepository::insert` on it: the savepoint is
    /// released on success and rolled back on failure before the insert's error is returned.
    async fn insert_detail(
        &mut self,
        acct_id: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> Result<(), PersistenceError> {
        let mut savepoint = self.savepoint().await?;
        let inserted = PendingAuthDetailRepository::insert(&mut savepoint, acct_id, record).await;
        match inserted {
            Ok(()) => PgTx::commit(savepoint).await,
            Err(error) => {
                if let Err(rollback) = PgTx::rollback(savepoint).await {
                    detail_savepoint_rollback_failed(&rollback);
                }
                Err(error)
            }
        }
    }

    /// [`PgTx::commit`].
    fn commit(self) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        PgTx::commit(self)
    }

    /// [`PgTx::rollback`].
    fn rollback(self) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        PgTx::rollback(self)
    }
}

/// The CP00 worker: one consumer and one publisher channel, the configured request queue, the
/// store, the injected clock (COPAUA0C-130) and the key under which its span and alert record the
/// request IDs.
///
/// `S` is the store: production uses [`PgStore`], and tests substitute a failing one through
/// [`Worker::with_store`]. `A` settles requests: production uses the [`Consumer`] channel itself,
/// and tests substitute a failing [`Acknowledger`] through [`Worker::with_acknowledger`]. The
/// worker keeps nothing from one request to the next, neither a request, a body, an ID nor a
/// decision flag (E14, E15).
pub struct Worker<S: AuthStore = PgStore, A: Acknowledger = Consumer> {
    channels: Channels<A>,
    request_queue: String,
    store: S,
    clock: Arc<dyn Clock>,
    /// The key of `config.amqp_url` under which the span and the alert record each request's IDs
    /// as their keyed digests, as the `Redaction` section of `crate::workers` explains; never
    /// shown.
    id_key: IdKey,
}

impl Worker<PgStore, Consumer> {
    /// Opens the worker's channels on `connection`: checks that `config.queue_request_pauth` is
    /// a queue name AMQP can carry, then opens the consumer channel, checks the request queue and
    /// `CARD.DEMO.DEADLETTER.PAUTH` with a passive declare, and opens the publisher channel in
    /// confirm mode. Reply queues are the requests' `reply_to` and are not declared. `store`
    /// serves every read, write and log row; `clock` supplies the detail keys, the log dates and
    /// times and the reply message IDs; `config.amqp_url`, the `CARDDEMO_AMQP_URL`
    /// `carddemo-mq` connects with, keys the IDs each request span and alert records.
    ///
    /// # Errors
    ///
    /// [`AmqpError::QueueName`] naming `CARDDEMO_QUEUE_REQUEST_PAUTH` when the configured request
    /// queue is longer than the 255-byte AMQP short string or holds a byte outside printable
    /// ASCII, before any channel is opened; the [`AmqpError`] of a channel that could not be
    /// opened or put in confirm mode; or [`AmqpError::Declare`] naming the first of the two
    /// queues that does not exist.
    pub async fn open(
        connection: &Connection,
        config: &Config,
        store: PgStore,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, AmqpError> {
        super::check_request_queue(
            REQUEST_QUEUE_VARIABLE,
            &config.queue_request_pauth,
            super::AMQP_QUEUE_NAME_LEN,
        )?;
        let channels = Channels::open(
            connection,
            &[
                QueueCheck::configured(REQUEST_QUEUE_VARIABLE, &config.queue_request_pauth),
                QueueCheck::literal(broker::DEAD_LETTER_QUEUE),
            ],
        )
        .await?;
        Ok(Self {
            channels,
            request_queue: config.queue_request_pauth.clone(),
            store,
            clock,
            id_key: IdKey::new(&config.amqp_url),
        })
    }
}

impl<S: AuthStore, A: Acknowledger> Worker<S, A> {
    /// This worker with `acknowledger` settling every request in place of the consumer channel,
    /// the seam of the failure-injection tests. Channels, queue, store, clock and ID key are kept.
    #[must_use]
    pub fn with_acknowledger<B: Acknowledger>(self, acknowledger: B) -> Worker<S, B> {
        Worker {
            channels: self.channels.with_acknowledger(acknowledger),
            request_queue: self.request_queue,
            store: self.store,
            clock: self.clock,
            id_key: self.id_key,
        }
    }

    /// This worker with `store` in place of its store, the seam of the failure-injection tests.
    /// Channels, queue, acknowledger, clock and ID key are kept.
    #[must_use]
    pub fn with_store<T: AuthStore>(self, store: T) -> Worker<T, A> {
        Worker {
            channels: self.channels,
            request_queue: self.request_queue,
            store,
            clock: self.clock,
            id_key: self.id_key,
        }
    }
}

impl<S: AuthStore, A: Acknowledger> fmt::Debug for Worker<S, A> {
    /// The request queue only: channels, store and clock have nothing a reader of a log needs,
    /// and the ID key is a secret.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Worker")
            .field("request_queue", &self.request_queue)
            .finish_non_exhaustive()
    }
}

impl<S: AuthStore, A: Acknowledger> Step for Worker<S, A> {
    fn queue(&self) -> &str {
        &self.request_queue
    }

    /// Takes the next request and processes it inside its `mq.message` span: parse, read,
    /// decide, reply, acknowledge, then write, as the module documentation states
    /// (COPAUA0C-001, COPAUA0C-080, COPAUA0C-081). `super::in_message_span` opens the span and
    /// records a settled request there with one `INFO` event of its outcome, its redelivery
    /// flag and its body length, never a body byte or an ID; a requeued request gets none.
    async fn process_next(&mut self, connection: &Connection) -> Result<Processed, StepError> {
        let Self {
            channels,
            request_queue,
            store,
            clock,
            id_key,
        } = self;
        let Some(request) = channels.next(request_queue).await? else {
            return Ok(Processed::Idle);
        };
        let id_key: &IdKey = id_key;
        let mut message = Message {
            channels,
            connection,
            store,
            clock: clock.as_ref(),
            id_key,
            request: &request,
            store_unavailable: false,
        };
        Ok(super::in_message_span(request_queue, &request, id_key, message.process()).await)
    }

    async fn reopen(&mut self, connection: &Connection) -> Result<(), AmqpError> {
        self.channels.reopen(connection).await
    }
}

/// One request being processed, with everything its steps reach.
struct Message<'a, S: AuthStore, A: Acknowledger> {
    channels: &'a mut Channels<A>,
    connection: &'a Connection,
    store: &'a S,
    clock: &'a dyn Clock,
    /// The worker's ID key, under which the alert shows the request IDs.
    id_key: &'a IdKey,
    request: &'a Request,
    /// Whether a store read of this request failed with [`PersistenceError::Unavailable`], so
    /// that [`Message::log`] attempts no further `cssl_error_log` write for it. It is knowledge
    /// about this request only: each request starts with `false`, and nothing of it survives to
    /// the next request.
    store_unavailable: bool,
}

/// Why a reply left the request unacknowledged.
enum Unanswered {
    /// The reply was not published, or not confirmed: the `M004` path (COPAUA0C-117).
    NotConfirmed(Undelivered),
    /// The reply was confirmed and the acknowledgement failed: the delivery tag is dead
    /// (COPAUA0C-118).
    AckFailed(AmqpError),
}

/// Why a reply was not confirmed: the `M004` cases of COPAUA0C-117. Only [`Undelivered::Failed`]
/// follows a publication; the other two are refused before anything is published, so they leave
/// the publisher channel as it was.
enum Undelivered {
    /// The request carries no `reply_to`, so there is no queue to publish to (`X-COPAUA0C-7`).
    NoReplyTo,
    /// The request's `reply_to` names `RabbitMQ`'s Direct Reply-to pseudo-queue
    /// ([`broker::is_direct_reply_to`]), whose mandatory publication the broker would deliver
    /// and also return, so it cannot be confirmed without the requester holding the reply
    /// (COPAUA0C-117, `X-COPAUA0C-12`).
    DirectReplyTo,
    /// The publication failed, was negatively acknowledged or came back unroutable.
    Failed(PublishFailure),
}

impl fmt::Display for Undelivered {
    /// The case only, never the `reply_to` text: the requester chooses that value, and any
    /// diagnostic shows it only screened.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoReplyTo => f.write_str("the request names no reply_to queue"),
            Self::DirectReplyTo => f.write_str(
                "the request's reply_to names the Direct Reply-to pseudo-queue, whose mandatory \
                 publication cannot be confirmed without delivering it; it is refused before \
                 publication",
            ),
            Self::Failed(failure) => failure.fmt(f),
        }
    }
}

/// The queue a reply to `request` is published to: its `reply_to`, unless that is absent
/// ([`Undelivered::NoReplyTo`], `X-COPAUA0C-7`) or names the Direct Reply-to pseudo-queue
/// ([`Undelivered::DirectReplyTo`], COPAUA0C-117, `X-COPAUA0C-12`). Any other value is a
/// destination, the empty string included, which is published and comes back unroutable
/// (`X-COPAUA0C-7`).
///
/// # Errors
///
/// The [`Undelivered`] case of a `reply_to` that is refused before any publication.
fn reply_queue(request: &Request) -> Result<&str, Undelivered> {
    match request.reply_to.as_deref() {
        None => Err(Undelivered::NoReplyTo),
        Some(queue) if broker::is_direct_reply_to(queue) => Err(Undelivered::DirectReplyTo),
        Some(queue) => Ok(queue),
    }
}

/// What the reads of a found card returned, inside the transaction that holds the summary lock.
struct Found<T> {
    tx: T,
    summary: PendingAuthSummaryRecord,
    created: bool,
    account: Option<AccountRecord>,
    customer: CustomerLookup,
}

impl<S: AuthStore, A: Acknowledger> Message<'_, S, A> {
    /// Parses the whole body, which only `copaua0c::parse_request` reads, and takes the path its
    /// outcome selects (COPAUA0C-005, COPAUA0C-013, COPAUA0C-017).
    async fn process(&mut self) -> Processed {
        match copaua0c::parse_request(&self.request.body) {
            Err(ParseError::CardNumber(error)) => self.unparseable(&error).await,
            Err(ParseError::Field(error)) => self.field_error(&error).await,
            Ok(request) => self.authorize(&request).await,
        }
    }

    /// A card number that cannot be read: the `unparseable_alert` event, then the rejection that
    /// dead-letters the request. No reply, lock, write or log row (COPAUA0C-006; i11, E31).
    async fn unparseable(&self, error: &CardNumberError) -> Processed {
        unparseable_alert(self.request, self.id_key, error);
        self.dead_letter().await
    }

    /// A parseable card with a malformed field: the `05`/`9000` decline, confirmed and then
    /// acknowledged, with no lock and no write (COPAUA0C-007 to COPAUA0C-012, COPAUA0C-082).
    async fn field_error(&mut self, error: &FieldError) -> Processed {
        malformed_field(error);
        let reply = copaua0c::field_error_reply(error);
        match self.answer(&reply).await {
            Ok(()) => Processed::Replied,
            Err(why) => self.unanswered(why, &error.card, None).await,
        }
    }

    /// A parsed request: the cross-reference read, then the found or not-found path
    /// (COPAUA0C-020, COPAUA0C-021). A failed cross-reference read (`C001`) or `BEGIN` (`I001`)
    /// takes [`Message::read_failed`], the interim `05`/`9000` decline (COPAUA0C-122,
    /// COPAUA0C-126).
    ///
    /// The cross-reference is read on the pool, before the transaction opens, rather than inside
    /// it as COPAUA0C-020 and COPAUA0C-081 word it: the read takes no lock, so its result is the
    /// one a read inside the transaction would give, and a card with no row then opens no
    /// transaction at all. The transaction, and the summary lock that serialises the account's
    /// decisions, begins only for a found card, so a failed `BEGIN` is logged as `I001` after
    /// the cross-reference read, where the COBOL schedules its PSB first (COPAUA0C-126).
    async fn authorize(&mut self, request: &AuthRequestRecord) -> Processed {
        let card = &request.card_num;
        let read = self.store.find_xref(card).await;
        let xref = match read {
            Ok(Some(xref)) => xref,
            Ok(None) => return self.card_not_found(request).await,
            Err(error) => {
                let site = LogSite::C001 {
                    card,
                    sqlstate: sqlstate(&error),
                };
                return self.read_failed(request, site, &error, None).await;
            }
        };
        let opened = self.store.begin().await;
        match opened {
            Ok(tx) => self.authorize_found(request, &xref, tx).await,
            Err(error) => self.read_failed(request, LogSite::I001, &error, None).await,
        }
    }

    /// A card with no `card_xref` row: the `A001` row, then the decision, confirmed and
    /// acknowledged, with no lock and no authorization write (COPAUA0C-021, COPAUA0C-082,
    /// COPAUA0C-114). The decision is the domain's interim `05`/`3100`, not a parity result
    /// (COPAUA0C-040).
    async fn card_not_found(&mut self, request: &AuthRequestRecord) -> Processed {
        let card = &request.card_num;
        self.log(LogSite::A001 { card }).await;
        let decision = copaua0c::decide(request, &Lookup::CardNotInXref);
        match self.answer(&decision.reply).await {
            Ok(()) => Processed::Replied,
            Err(why) => self.unanswered(why, card, None).await,
        }
    }

    /// A card found in the cross-reference, inside `tx`: the locked reads, the decision, the
    /// reply and its acknowledgement, then the writes (COPAUA0C-041 to COPAUA0C-048,
    /// COPAUA0C-081). The decision is the domain's, the i10 guard included; nothing here
    /// compares an amount.
    async fn authorize_found(
        &mut self,
        request: &AuthRequestRecord,
        xref: &CardXrefRecord,
        tx: S::Tx,
    ) -> Processed {
        let Found {
            tx,
            summary,
            created,
            account,
            customer,
        } = match self.read_found(request, xref, tx).await {
            Ok(found) => found,
            Err(processed) => return processed,
        };
        let summary = if created {
            SummaryState::Created(&summary)
        } else {
            SummaryState::Existing(&summary)
        };
        let context = FoundContext {
            xref,
            account: account.as_ref(),
            customer,
            summary,
        };
        let decision = copaua0c::decide(request, &Lookup::CardFound(context));
        cp00_decision(&decision);
        match self.answer(&decision.reply).await {
            Ok(()) => self.write_after_ack(tx, request, &context, &decision).await,
            Err(why) => self.unanswered(why, &request.card_num, Some(tx)).await,
        }
    }

    /// The reads of a found card inside `tx`, in the replacement's order: the summary lock,
    /// then the account and the customer, each not-found writing its warning row and letting
    /// processing go on (COPAUA0C-022, COPAUA0C-023, COPAUA0C-025, COPAUA0C-081, COPAUA0C-115,
    /// COPAUA0C-116).
    ///
    /// # Errors
    ///
    /// The [`Processed`] outcome of a failed read, `I002`, `C002` or `C003`, which ends the
    /// request before its decision with the interim `05`/`9000` decline and no authorization
    /// write ([`Message::read_failed`]; COPAUA0C-123 to COPAUA0C-125).
    async fn read_found(
        &mut self,
        request: &AuthRequestRecord,
        xref: &CardXrefRecord,
        mut tx: S::Tx,
    ) -> Result<Found<S::Tx>, Processed> {
        let card = &request.card_num;
        let locked = tx
            .lock_or_create_summary(&copaua0c::summary_template(xref))
            .await;
        let (summary, created) = match locked {
            Ok(locked) => locked,
            Err(error) => {
                let site = LogSite::I002 {
                    card,
                    sqlstate: sqlstate(&error),
                };
                return Err(self.read_failed(request, site, &error, Some(tx)).await);
            }
        };
        let read = tx.find_account(xref.acct_id).await;
        let account = match read {
            Ok(account) => account,
            Err(error) => {
                let site = LogSite::C002 {
                    acct_id: xref.acct_id,
                    sqlstate: sqlstate(&error),
                };
                return Err(self.read_failed(request, site, &error, Some(tx)).await);
            }
        };
        if account.is_none() {
            self.log(LogSite::A002 {
                acct_id: xref.acct_id,
            })
            .await;
        }
        let read = tx.find_customer(xref.cust_id).await;
        let customer = match read {
            Ok(Some(_)) => CustomerLookup::Found,
            Ok(None) => {
                self.log(LogSite::A003 {
                    cust_id: xref.cust_id,
                })
                .await;
                CustomerLookup::NotFound
            }
            Err(error) => {
                let site = LogSite::C003 {
                    cust_id: xref.cust_id,
                    sqlstate: sqlstate(&error),
                };
                return Err(self.read_failed(request, site, &error, Some(tx)).await);
            }
        };
        Ok(Found {
            tx,
            summary,
            created,
            account,
            customer,
        })
    }

    /// The writes after the request's acknowledgement, in `tx`, which still holds the summary
    /// lock: the summary update, the detail insert inside a savepoint, then the commit
    /// (COPAUA0C-081, COPAUA0C-090 to COPAUA0C-103). Declines of a found card are written too.
    ///
    /// - A summary that cannot be built or updated: `I003`, the rollback, and neither table
    ///   changes (COPAUA0C-119).
    /// - A failed detail insert: `I004`, and the summary update still commits (COPAUA0C-099,
    ///   COPAUA0C-120).
    /// - A failed commit: an `ERROR` event only (COPAUA0C-121, E29). A commit the server refuses
    ///   is rolled back, and the confirmed reply stands unrecorded. After a connection lost
    ///   during the commit, the outcome is unknown, and the worker does not reconcile it.
    async fn write_after_ack(
        &self,
        mut tx: S::Tx,
        request: &AuthRequestRecord,
        context: &FoundContext<'_>,
        decision: &Decision,
    ) -> Processed {
        let card = &request.card_num;
        let row = match copaua0c::summary_update(
            context.summary,
            context.xref,
            context.account,
            decision,
        ) {
            Ok(row) => row,
            Err(error) => {
                // HALTED(X-ARITH-6), COPAUA0C-094 and COPAUA0C-095: a count at 9999 has no
                // expected next value, so no summary is written; the `I003` path is interim
                // handling, not a parity result.
                summary_update_unbuildable(error);
                let site = LogSite::I003 {
                    card,
                    sqlstate: None,
                };
                return self.summary_not_written(tx, site).await;
            }
        };
        let updated = tx.update_summary(&row).await;
        let failure = match updated {
            Ok(true) => None,
            Ok(false) => {
                summary_update_no_row();
                Some(LogSite::I003 {
                    card,
                    sqlstate: None,
                })
            }
            Err(error) => {
                summary_update_failed(&error);
                Some(LogSite::I003 {
                    card,
                    sqlstate: sqlstate(&error),
                })
            }
        };
        if let Some(site) = failure {
            return self.summary_not_written(tx, site).await;
        }
        let detail = copaua0c::detail_record(request, decision, self.clock);
        let inserted = tx
            .insert_detail(PicS::from(context.xref.acct_id), &detail)
            .await;
        if let Err(error) = inserted {
            detail_insert_failed(&error);
            let site = LogSite::I004 {
                card,
                sqlstate: sqlstate(&error),
            };
            self.log(site).await;
        }
        if let Err(error) = tx.commit().await {
            commit_failed(&error);
        }
        Processed::Replied
    }

    /// The `I003` path: the row, then the rollback, which leaves both tables as they were; the
    /// confirmed reply and the acknowledgement stand (COPAUA0C-119).
    async fn summary_not_written(&self, tx: S::Tx, site: LogSite<'_>) -> Processed {
        self.log(site).await;
        rollback(Some(tx)).await;
        Processed::Replied
    }

    /// Publishes `reply` to the request's `reply_to`, waits for its confirmation and then
    /// acknowledges the request (COPAUA0C-066, COPAUA0C-067, COPAUA0C-081, COPAUA0C-082).
    ///
    /// The body is the 63 written bytes of `copaua0c::reply_body`, its card field redacted; the
    /// COBOL's 64-byte length is HALTED(H6) (COPAUA0C-060 to COPAUA0C-065). The properties are
    /// `broker::cp00_properties` with a new `message_id` from the injected clock: the request's
    /// `correlation_id`, persistent, `expiration` `"5000"` (E16). That `message_id` is recorded
    /// verbatim as `reply_message_id` on the current span, the request's `mq.message` span,
    /// before the publication, so every later line of the request names its reply. The
    /// destination is [`reply_queue`]'s: an absent `reply_to` is not published; an empty one is,
    /// and comes back unroutable (`X-COPAUA0C-7`); one naming the Direct Reply-to pseudo-queue is
    /// not published, because `RabbitMQ` 3.13 would deliver it and also return it, and a
    /// delivered decision must never take the `M004` path (COPAUA0C-117, `X-COPAUA0C-12`).
    ///
    /// # Errors
    ///
    /// [`Unanswered::NotConfirmed`] when no confirmation came, or when the destination was
    /// refused before any publication, before any acknowledgement; [`Unanswered::AckFailed`]
    /// when the confirmed reply's acknowledgement failed.
    async fn answer(&self, reply: &AuthReply) -> Result<(), Unanswered> {
        let reply_to = reply_queue(self.request).map_err(Unanswered::NotConfirmed)?;
        let body = copaua0c::reply_body(reply);
        let message_id = ids::next_message_id(self.clock);
        telemetry::record_reply_message_id(&tracing::Span::current(), &message_id);
        let properties = broker::cp00_properties(self.request, message_id);
        self.channels
            .publish(reply_to, &body, properties)
            .await
            .map_err(|failure| Unanswered::NotConfirmed(Undelivered::Failed(failure)))?;
        self.channels
            .ack(self.request)
            .await
            .map_err(Unanswered::AckFailed)
    }

    /// Settles a request whose reply was not acknowledged, rolling back `tx`, if any, which
    /// releases the summary lock with nothing written.
    ///
    /// - Not confirmed: the `M004` row, the rollback, the rejection that dead-letters the
    ///   request, then a new publisher channel when a publication was attempted
    ///   (COPAUA0C-117). The row is not attempted when a store read of this request found the
    ///   database unavailable ([`Message::log`]).
    /// - Acknowledgement failed: no row, as the reply was confirmed; the rollback, then the
    ///   consumer channel is closed so that the broker requeues the request, and its
    ///   redelivery is processed as a new request (COPAUA0C-118).
    async fn unanswered(
        &mut self,
        why: Unanswered,
        card: &CardNumber,
        tx: Option<S::Tx>,
    ) -> Processed {
        match why {
            Unanswered::AckFailed(error) => {
                ack_failed(&error);
                rollback(tx).await;
                self.channels.abandon().await
            }
            Unanswered::NotConfirmed(undelivered) => {
                reply_unconfirmed(&undelivered);
                self.log(LogSite::M004 { card }).await;
                rollback(tx).await;
                let processed = self.dead_letter().await;
                if processed == Processed::DeadLettered
                    && matches!(undelivered, Undelivered::Failed(_))
                {
                    if let Err(error) = self.channels.reopen_publisher(self.connection).await {
                        publisher_reopen_failed(&error);
                    }
                }
                processed
            }
        }
    }

    /// A store read of `request` that failed with `error`: the audited critical row of `site`,
    /// the rollback of `tx`, if any, which releases the summary lock with nothing written, then
    /// `copaua0c::store_failure_reply`, the `05`/`9000` decline, confirmed and then acknowledged
    /// (COPAUA0C-122 to COPAUA0C-126).
    ///
    /// The COBOL task ends at the critical row before its reply. The decline is interim
    /// handling, not a parity result: the outcome of the open escalation candidate
    /// `X-COPAUA0C-1` that keeps i11's one reply per removed request, pending the reviewer's and
    /// the user's decision. A [`PersistenceError::Unavailable`] error marks the store down for
    /// the rest of this request, so neither the row of `site` nor an `M004` row is attempted
    /// ([`Message::log`]). A decline that is not confirmed takes the `M004` path and is
    /// dead-lettered; one whose acknowledgement fails is requeued ([`Message::unanswered`]).
    async fn read_failed(
        &mut self,
        request: &AuthRequestRecord,
        site: LogSite<'_>,
        error: &PersistenceError,
        tx: Option<S::Tx>,
    ) -> Processed {
        store_read_failed(site, error);
        if matches!(error, PersistenceError::Unavailable) {
            self.store_unavailable = true;
        }
        self.log(site).await;
        rollback(tx).await;
        // Boxed: the decline and its `M004` path, awaited this deep inside a found card's reads,
        // would grow every CP00 request's future past the size `clippy::large_futures` allows;
        // only a failed read pays for the allocation.
        Box::pin(async {
            match self.answer(&copaua0c::store_failure_reply(request)).await {
                Ok(()) => Processed::Replied,
                Err(why) => self.unanswered(why, &request.card_num, None).await,
            }
        })
        .await
    }

    /// Rejects the request with `requeue: false`, which moves it to
    /// `CARD.DEMO.DEADLETTER.PAUTH` through the request queue's dead-letter arguments
    /// (COPAUA0C-006, COPAUA0C-117). When the rejection fails, the consumer channel is closed and
    /// the broker requeues the request.
    async fn dead_letter(&self) -> Processed {
        match self.channels.reject_without_requeue(self.request).await {
            Ok(()) => Processed::DeadLettered,
            Err(error) => {
                reject_failed(&error);
                self.channels.abandon().await
            }
        }
    }

    /// Writes the `cssl_error_log` row of `site` through [`AuthStore::log`], in its own
    /// transaction, and emits its event: `ERROR` for a critical site, `WARN` for a warning
    /// (COPAUA0C-110). The record is the domain's, its event key redacted (COPAUA0C-113,
    /// E32); neither event carries the key. A failed write is traced and processing goes on.
    ///
    /// After a store read of this request found the database unavailable
    /// ([`Message::read_failed`]), the write is not attempted, as it could only wait out another
    /// pool acquire and fail: one more `ERROR` event, with the location and the message only,
    /// stands for the row not written.
    async fn log(&self, site: LogSite<'_>) {
        let location = location(site);
        cssl_error_log(site, &location);
        if self.store_unavailable {
            cssl_row_not_attempted(&location, site.message());
            return;
        }
        let record = copaua0c::error_log(site, self.clock);
        if let Err(error) = self.store.log(&record).await {
            cssl_row_write_failed(&location, &error);
        }
    }
}

/// The alert of a request whose card number cannot be read (COPAUA0C-006; i11, E31): one
/// `ERROR` event with target `runtime::telemetry::ALERT_TARGET`, `alert` set to
/// `CP00_CARD_UNPARSEABLE`, the request's `message_id` and `correlation_id` as
/// `Request::screened_message_id` and `Request::screened_correlation_id` give them under
/// `id_key`, the worker's key: the base64url HMAC-SHA256 the request's span records too, so the
/// alert is matched to the span's other lines. Then the body's length and the cause. It never
/// carries a body byte or the text of either ID.
fn unparseable_alert(request: &Request, id_key: &IdKey, error: &CardNumberError) {
    static SITE: EventSite<6> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata_with_target(
        "cp00_card_unparseable",
        telemetry::ALERT_TARGET,
        module_path!(),
        Level::ERROR,
        &[
            "message",
            "alert",
            "message_id",
            "correlation_id",
            "body_len",
            "cause",
        ],
        &SITE,
    );
    // An `Option` records nothing when it is `None`, which leaves the field out of the event.
    let message_id = request
        .screened_message_id(Some(id_key))
        .map(field::display);
    let correlation_id = request
        .screened_correlation_id(Some(id_key))
        .map(field::display);
    emit(
        &SITE,
        [
            Some(
                &"the CP00 request has no readable card number; it is dead-lettered without a \
                  reply",
            ),
            Some(&copaua0c::ALERT_CARD_UNPARSEABLE),
            Some(&message_id),
            Some(&correlation_id),
            Some(&error.body_len),
            Some(&field::debug(&error.cause)),
        ],
    );
}

/// Rolls `tx` back, when there is one; a failed rollback is traced, and `PostgreSQL` discards
/// the transaction with its connection.
async fn rollback<T: AuthTx>(tx: Option<T>) {
    let Some(tx) = tx else {
        return;
    };
    if let Err(error) = tx.rollback().await {
        rollback_failed(&error);
    }
}

/// `ERR-LOCATION` of `site` as text, for events.
fn location(site: LogSite<'_>) -> Cow<'static, str> {
    String::from_utf8_lossy(site.location())
}

/// The SQLSTATE of `error` where it has one, the interim `ERR-CODE-1` of a failed statement,
/// HALTED(H4) (COPAUA0C-112).
fn sqlstate(error: &PersistenceError) -> Option<SqlState> {
    error.sqlstate().and_then(SqlState::parse)
}

/// `WARN` when rolling back to the detail savepoint failed with `error`.
fn detail_savepoint_rollback_failed(error: &PersistenceError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "detail_savepoint_rollback_failed",
        module_path!(),
        Level::WARN,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"rolling back to the detail savepoint failed"),
            Some(&field::display(error)),
        ],
    );
}

/// `WARN` when a request whose card number parses has a malformed field, with the field's
/// `fault`, before its decline.
fn malformed_field(error: &FieldError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "malformed_field",
        module_path!(),
        Level::WARN,
        &["message", "fault"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the CP00 request has a malformed field; it is declined with no write"),
            Some(&field::display(&error.fault)),
        ],
    );
}

/// `DEBUG` with the `decision`'s status, reason and guard outcome, before its reply.
fn cp00_decision(decision: &Decision) {
    static SITE: EventSite<4> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "cp00_decision",
        module_path!(),
        Level::DEBUG,
        &["message", "status", "reason", "guard"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"CP00 decision"),
            Some(&field::debug(&decision.status)),
            Some(&field::debug(&decision.reason)),
            Some(&field::debug(&decision.guard)),
        ],
    );
}

/// `ERROR` when the summary update cannot be built, with the domain's `error`.
fn summary_update_unbuildable(error: SummaryError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "summary_update_unbuildable",
        module_path!(),
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the summary update cannot be built"),
            Some(&field::display(error)),
        ],
    );
}

/// `ERROR` when the summary update changed no row.
fn summary_update_no_row() {
    static SITE: EventSite<1> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "summary_update_no_row",
        module_path!(),
        Level::ERROR,
        &["message"],
        &SITE,
    );
    emit(&SITE, [Some(&"the summary update changed no row")]);
}

/// `ERROR` when the summary update failed with the store `error`.
fn summary_update_failed(error: &PersistenceError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "summary_update_failed",
        module_path!(),
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the summary update failed"),
            Some(&field::display(error)),
        ],
    );
}

/// `ERROR` when the detail insert failed with the store `error`, so the summary update commits
/// alone.
fn detail_insert_failed(error: &PersistenceError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "detail_insert_failed",
        module_path!(),
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the detail insert failed; the summary update commits alone"),
            Some(&field::display(error)),
        ],
    );
}

/// `ERROR` when the authorization commit failed with the store `error`. Its message tells the
/// two cases apart: [`PersistenceError::Unavailable`], a connection lost during the commit,
/// leaves the outcome unknown and unreconciled; any other error is a commit the server refused
/// and rolled back, so the confirmed reply stands unrecorded.
fn commit_failed(error: &PersistenceError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "commit_failed",
        module_path!(),
        Level::ERROR,
        &["message", "error"],
        &SITE,
    );
    let message = if matches!(error, PersistenceError::Unavailable) {
        "the connection was lost during the authorization commit; whether it committed is \
         unknown, and it is not reconciled"
    } else {
        "the server refused the authorization commit and rolled it back; the confirmed reply \
         stands unrecorded"
    };
    emit(&SITE, [Some(&message), Some(&field::display(error))]);
}

/// `WARN` when the acknowledgement of a confirmed reply failed with `error`, so nothing is
/// written and the request is requeued.
fn ack_failed(error: &AmqpError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "ack_failed",
        module_path!(),
        Level::WARN,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the acknowledgement failed; nothing is written and the request is requeued"),
            Some(&field::display(error)),
        ],
    );
}

/// `WARN` when the reply was not confirmed, for the `undelivered` reason, so the request is
/// dead-lettered.
fn reply_unconfirmed(undelivered: &Undelivered) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "reply_unconfirmed",
        module_path!(),
        Level::WARN,
        &["message", "reason"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the reply was not confirmed; the request is dead-lettered"),
            Some(&field::display(undelivered)),
        ],
    );
}

/// `WARN` when reopening the publisher channel after an unconfirmed reply failed with `error`.
fn publisher_reopen_failed(error: &AmqpError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "publisher_reopen_failed",
        module_path!(),
        Level::WARN,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"reopening the publisher channel failed; the next get reports it"),
            Some(&field::display(error)),
        ],
    );
}

/// `ERROR` when a store read failed at the log location of `site` with the store `error`,
/// before the `05`/`9000` decline.
fn store_read_failed(site: LogSite<'_>, error: &PersistenceError) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "store_read_failed",
        module_path!(),
        Level::ERROR,
        &["message", "location", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(
                &"a CP00 store read failed; the request is declined 05/9000 with no authorization \
                  write",
            ),
            Some(&field::display(&location(site))),
            Some(&field::display(error)),
        ],
    );
}

/// `WARN` when rejecting the request failed with `error`, so the broker requeues it.
fn reject_failed(error: &AmqpError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "reject_failed",
        module_path!(),
        Level::WARN,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"rejecting the request failed; the broker requeues it"),
            Some(&field::display(error)),
        ],
    );
}

/// The `CSSL error log` event of the row of `site`, with its `location` and its message:
/// `ERROR` for a critical site, `WARN` for a warning (COPAUA0C-110). It never carries the row's
/// event key.
fn cssl_error_log(site: LogSite<'_>, location: &str) {
    // An event's level is fixed at its callsite, so each level has its own callsite.
    if site.ends_task() {
        static SITE: EventSite<3> = EventSite::new(&METADATA);
        static METADATA: Metadata<'static> = event_metadata(
            "cssl_error_log_critical",
            module_path!(),
            Level::ERROR,
            &["message", "location", "err_message"],
            &SITE,
        );
        emit(
            &SITE,
            [
                Some(&"CSSL error log"),
                Some(&field::display(location)),
                Some(&site.message()),
            ],
        );
    } else {
        static SITE: EventSite<3> = EventSite::new(&METADATA);
        static METADATA: Metadata<'static> = event_metadata(
            "cssl_error_log_warning",
            module_path!(),
            Level::WARN,
            &["message", "location", "err_message"],
            &SITE,
        );
        emit(
            &SITE,
            [
                Some(&"CSSL error log"),
                Some(&field::display(location)),
                Some(&site.message()),
            ],
        );
    }
}

/// `ERROR` standing for the `cssl_error_log` row of `location` and `err_message` that is not
/// written, because the database is unavailable.
fn cssl_row_not_attempted(location: &str, err_message: &str) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "cssl_row_not_attempted",
        module_path!(),
        Level::ERROR,
        &["message", "location", "err_message"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the cssl_error_log row is not written because the database is unavailable"),
            Some(&field::display(location)),
            Some(&err_message),
        ],
    );
}

/// `ERROR` when the `cssl_error_log` row of `location` could not be written, with the store
/// `error`.
fn cssl_row_write_failed(location: &str, error: &PersistenceError) {
    static SITE: EventSite<3> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "cssl_row_write_failed",
        module_path!(),
        Level::ERROR,
        &["message", "location", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"the cssl_error_log row could not be written"),
            Some(&field::display(location)),
            Some(&field::display(error)),
        ],
    );
}

/// `WARN` when rolling back the authorization transaction failed with the store `error`.
fn rollback_failed(error: &PersistenceError) {
    static SITE: EventSite<2> = EventSite::new(&METADATA);
    static METADATA: Metadata<'static> = event_metadata(
        "rollback_failed",
        module_path!(),
        Level::WARN,
        &["message", "error"],
        &SITE,
    );
    emit(
        &SITE,
        [
            Some(&"rolling back the authorization transaction failed"),
            Some(&field::display(error)),
        ],
    );
}

#[cfg(test)]
mod tests {
    use domain::messaging::copaua0c::{CardNumberCause, CardNumberError};
    use persistence::PersistenceError;
    use runtime::telemetry::{ALERT_TARGET, IdKey, ScreenedText};
    use test_support::trace_capture;

    use super::{Undelivered, commit_failed, reply_queue, unparseable_alert};
    use crate::broker::Request;

    // audit: COPAUA0C-006
    #[test]
    fn the_unparseable_card_alert_shows_the_ids_screened_and_no_body_byte() {
        let body = b"220718,102030,ABC".to_vec();
        let body_len = body.len();
        let request = Request {
            delivery_tag: 9,
            body,
            message_id: Some("PASS WOR".to_owned()),
            correlation_id: Some("SECRET".to_owned()),
            reply_to: Some("123-45-6789".to_owned()),
            redelivered: false,
        };
        let error = CardNumberError {
            body_len,
            cause: CardNumberCause::NotSixteenDigits,
        };
        // The worker's key, made from its `CARDDEMO_AMQP_URL` (here the local default), and how
        // the alert shows each ID: its base64url HMAC-SHA256 under it, as the request's span
        // shows them.
        let amqp_url = "amqp://carddemo:carddemo@localhost:5672/%2f";
        let id_key = IdKey::new(amqp_url);
        let shown_message_id = ScreenedText::keyed("PASS WOR", &id_key).to_string();
        let shown_correlation_id = ScreenedText::keyed("SECRET", &id_key).to_string();
        for digest in [&shown_message_id, &shown_correlation_id] {
            assert_eq!(digest.len(), 43, "{digest}");
        }

        let capture = trace_capture::capture();
        unparseable_alert(&request, &id_key, &error);

        let alerts = capture.lines_with_target(ALERT_TARGET);
        let [alert] = alerts.as_slice() else {
            panic!("expected exactly one alert line, got {alerts:?}");
        };
        let expected = [
            r#""level":"ERROR""#.to_owned(),
            r#""target":"carddemo::alert""#.to_owned(),
            r#""alert":"CP00_CARD_UNPARSEABLE""#.to_owned(),
            format!(r#""body_len":{body_len}"#),
            format!(r#""message_id":"{shown_message_id}""#),
            format!(r#""correlation_id":"{shown_correlation_id}""#),
            r#""cause":"NotSixteenDigits""#.to_owned(),
            concat!(
                r#""message":"the CP00 request has no readable card number; "#,
                r#"it is dead-lettered without a reply""#
            )
            .to_owned(),
        ];
        for member in &expected {
            assert!(
                alert.contains(member.as_str()),
                "{member} missing from {alert}"
            );
        }
        assert_eq!(capture.lines().len(), 1, "{}", capture.text());

        // The three properties, the key's URL, and the body's fields in the comma-delimited
        // context they have in the body, so that a timestamp's digits cannot match them.
        let text = capture.text();
        for value in [
            "PASS WOR",
            "SECRET",
            "123-45-6789",
            amqp_url,
            "220718,102030,ABC",
            "220718,",
            ",102030,",
            ",ABC",
        ] {
            assert!(!text.contains(value), "{value:?} in {text}");
        }
    }

    // audit: COPAUA0C-121
    #[test]
    fn the_commit_failed_event_tells_a_refused_commit_from_a_lost_connection() {
        let cases = [
            (
                PersistenceError::Database {
                    sqlstate: Some("40001".to_owned()),
                    constraint: None,
                },
                concat!(
                    r#""message":"the server refused the authorization commit and rolled it "#,
                    r#"back; the confirmed reply stands unrecorded""#
                ),
                r#""error":"database error (SQLSTATE 40001)""#,
            ),
            (
                PersistenceError::Unavailable,
                concat!(
                    r#""message":"the connection was lost during the authorization commit; "#,
                    r#"whether it committed is unknown, and it is not reconciled""#
                ),
                r#""error":"database unavailable""#,
            ),
        ];
        for (error, message, shown_error) in cases {
            let capture = trace_capture::capture();
            commit_failed(&error);

            let lines = capture.lines();
            let [line] = lines.as_slice() else {
                panic!("expected exactly one line for {error:?}, got {lines:?}");
            };
            for member in [r#""level":"ERROR""#, message, shown_error] {
                assert!(line.contains(member), "{member} missing from {line}");
            }
            assert_eq!(
                shown_error,
                format!(r#""error":"{error}""#),
                "{}",
                capture.text()
            );
        }
    }

    // audit: COPAUA0C-117
    #[test]
    fn the_reply_queue_refuses_an_absent_reply_to_and_the_direct_reply_to_pseudo_queue() {
        let cases: [(Option<&str>, Result<&str, &str>); 6] = [
            (None, Err("NoReplyTo")),
            (Some("amq.rabbitmq.reply-to"), Err("DirectReplyTo")),
            (
                Some("amq.rabbitmq.reply-to.g1h2AAZjM2FiZmI3"),
                Err("DirectReplyTo"),
            ),
            (Some(""), Ok("")),
            (Some("QA.REPLY.CTRL"), Ok("QA.REPLY.CTRL")),
            (Some("amq.rabbitmq.reply-tox"), Ok("amq.rabbitmq.reply-tox")),
        ];
        for (reply_to, expected) in cases {
            let request = Request {
                delivery_tag: 1,
                body: Vec::new(),
                message_id: None,
                correlation_id: None,
                reply_to: reply_to.map(str::to_owned),
                redelivered: false,
            };
            let outcome = reply_queue(&request).map_err(|undelivered| {
                // A refusal names its case and never the requester's value.
                let text = undelivered.to_string();
                if let Some(value) = reply_to.filter(|value| !value.is_empty()) {
                    assert!(!text.contains(value), "{value:?} in {text:?}");
                }
                match undelivered {
                    Undelivered::NoReplyTo => "NoReplyTo",
                    Undelivered::DirectReplyTo => "DirectReplyTo",
                    Undelivered::Failed(_) => "Failed",
                }
            });
            assert_eq!(outcome, expected, "reply_to {reply_to:?}");
        }
    }
}
