//! CDRA, the port of `COACCT01`: every request taken from the configured
//! `CARDDEMO_QUEUE_REQUEST_ACCT` queue is answered on `CARD.DEMO.REPLY.ACCT` with the account its
//! key names or with an `INVALID REQUEST PARAMETERS` text, and a failed account read is reported
//! on `CARD.DEMO.ERROR` instead (AAP 0.3.4.2; `modern/audit/messaging/COACCT01.md`).
//!
//! This module orchestrates and owns no byte layout, no SQL and no broker rule:
//!
//! | Step | COBOL | Owner | Rules |
//! |---|---|---|---|
//! | take the next request with `basic_get` | `3000-GET-REQUEST` | `Channels::next` | COACCT01-001 |
//! | decode `WS-FUNC` and `WS-KEY` from bytes 0-14 | `REQUEST-MSG-COPY` | `domain::messaging::coacct01::decode_request` | COACCT01-002, COACCT01-003 |
//! | branch on `WS-FUNC = 'INQA' AND WS-KEY > ZEROES` | `4000-PROCESS-REQUEST-REPLY` | `domain::messaging::coacct01::decide` | COACCT01-004, COACCT01-005, COACCT01-022 |
//! | read the account by its key | `EXEC CICS READ DATASET('ACCTDAT ')` | `persistence::AccountRepository::find_by_id` | COACCT01-004 |
//! | build the reply or the error body from the read | `EVALUATE WS-RESP-CD` | `domain::messaging::coacct01::respond` | COACCT01-006 to COACCT01-021, COACCT01-028, COACCT01-029 |
//! | publish the reply to the fixed reply queue with the request's IDs, then acknowledge | `4100-PUT-REPLY` | `super::reply_fixed` with `Flow::Cdra` | COACCT01-023 to COACCT01-026 |
//! | publish the account-read error body to `CARD.DEMO.ERROR`, then acknowledge | `9000-ERROR`, `8000-TERMINATION` | `super::route_error` with `Flow::Cdra` | COACCT01-028, COACCT01-032 |
//! | route `MQPUT ERR` to `CARD.DEMO.ERROR` when the reply is not confirmed, and discard the request when that is not confirmed either | `4100-PUT-REPLY` `WHEN OTHER`, `9000-ERROR` `WHEN OTHER` | `super::reply_fixed`, `super::route_error`, `crate::error_body` | COACCT01-033, COACCT01-034 |
//!
//! # The request
//!
//! Only `decode_request` reads the body, and only its bytes 0-14; this module never indexes,
//! slices or logs it (Total Parsers Over Untrusted Input). The layout is `REQUEST-MSG-COPY`, not
//! the README's message layouts: code is authoritative (E02). A body shorter than 15 bytes or
//! with a key byte that is not an ASCII digit has no single COBOL result, so it is answered with
//! the masked bad-parameter reply of `undecoded_reply`, interim replacement handling and never a
//! parity result (COACCT01-003). The request's `reply_to` is ignored: every reply goes to
//! `CARD.DEMO.REPLY.ACCT` (COACCT01-023).
//!
//! # Queue names
//!
//! The request queue comes only from `runtime::config::Config::queue_request_acct`, because the
//! COBOL takes it from the trigger message and holds no literal for it. The read-failure error
//! body names it in `MQ-APPL-QUEUE-NAME PIC X(48)` (COACCT01-029), so `Worker::open` refuses a
//! configured name longer than 48 bytes or holding a byte outside printable ASCII, and the body
//! always carries the whole name (X-COACCT01-8). The reply and error queues are the COBOL
//! literals `CARD.DEMO.REPLY.ACCT` and `CARD.DEMO.ERROR`, owned by
//! `domain::messaging::coacct01`; the README's queue names are not used (E01).
//!
//! # The account read
//!
//! The `RESP` of the CICS `READ` becomes the outcome of `find_by_id`: a row is `NORMAL`, no row
//! is `NOTFND`, and any store error is `WHEN OTHER` (COACCT01-004, COACCT01-021, COACCT01-028).
//! The read-failure body, with its return message `ERROR WHILE READING ACCTF` cut to
//! `PIC X(25)` (E18), is built whole by the domain and published as it is, never through
//! `crate::error_body::render`. Its condition and reason codes are HALTED(H4): CICS defines
//! them and `PostgreSQL` has no counterpart, so they keep the initialised zeros (COACCT01-030,
//! COACCT01-031). A failure is traced with the value-free `PersistenceError` text only.
//!
//! # One request, one task
//!
//! Each request is processed as the first message of a CICS task, and `Worker` keeps no state
//! from one request to the next: it holds its channels, its queue name and its store, never a
//! request, a body, a key or a record (COACCT01-027; E14, E15). The `CICS RETRIEVE`, `MQOPEN`
//! and `MQCLOSE` paths and their artefacts have no counterpart here (E17).
//!
//! # Redaction
//!
//! No CDRA reply or error body holds a card number, an SSN or a password: the account record
//! has no such field, the not-found and bad-parameter replies mask the key, and a body that
//! does not decode echoes nothing (COACCT01-035). Spans and events name the queue, the IDs and
//! the outcome, never a body byte, a key, a balance or an `AccountRecord`.

use std::fmt;

use copybook::PicX;
use domain::messaging::coacct01::{self, AccountLookup, CdraAction, CdraResponse};
use lapin::Connection;
use persistence::{AccountRepository, PgStore};
use runtime::config::Config;
use tracing::Instrument;

use super::{Channels, Processed, Step, StepError};
use crate::broker::{Acknowledger, AmqpError, Consumer, QueueCheck};
use crate::error_body::Flow;

/// The environment variable of the request queue, which the start-up refusal (X-COACCT01-8) and
/// a failed passive declare of the request queue name in place of the configured value.
const REQUEST_QUEUE_VARIABLE: &str = "CARDDEMO_QUEUE_REQUEST_ACCT";

/// Bytes of `MQ-APPL-QUEUE-NAME PIC X(48)`, which receives `INPUT-QUEUE-NAME` when the account
/// read fails (`COACCT01.cbl:441`; COACCT01-029). `Worker::open` refuses a request queue name
/// longer than this, or holding a byte outside printable ASCII (`' '` to `'~'`), which the
/// alphanumeric `MOVE` of the error body would cut or carry as no MQ queue name can
/// (X-COACCT01-8). The limit is below the 255-byte AMQP short string, so the same check keeps
/// the name within what the broker takes.
const QUEUE_NAME_LEN: usize = PicX::<48>::WIDTH;

/// The CDRA worker: one consumer and one publisher channel, the configured request queue and
/// the account store (COACCT01-004, COACCT01-027).
///
/// `A` settles requests; production uses the [`Consumer`] channel itself, and tests substitute a
/// failing or recording [`Acknowledger`] through [`Worker::with_acknowledger`].
pub struct Worker<A: Acknowledger = Consumer> {
    channels: Channels<A>,
    request_queue: String,
    store: PgStore,
}

impl Worker<Consumer> {
    /// Opens the worker on `connection`: checks that `config.queue_request_acct` fits
    /// `MQ-APPL-QUEUE-NAME PIC X(48)` whole, then opens the consumer channel, checks the request
    /// queue, `CARD.DEMO.REPLY.ACCT` and `CARD.DEMO.ERROR` with a passive declare, and opens the
    /// publisher channel in confirm mode. `store` serves every account read (COACCT01-004).
    ///
    /// # Errors
    ///
    /// [`AmqpError::QueueName`] naming `CARDDEMO_QUEUE_REQUEST_ACCT` when the configured request
    /// queue is longer than 48 bytes or holds a byte outside printable ASCII, before any channel
    /// is opened (X-COACCT01-8); the [`AmqpError`] of a channel that could not be opened or put
    /// in confirm mode; or [`AmqpError::Declare`] naming the first of the three queues that does
    /// not exist.
    pub async fn open(
        connection: &Connection,
        config: &Config,
        store: PgStore,
    ) -> Result<Self, AmqpError> {
        let request_queue = config.queue_request_acct.as_str();
        super::check_request_queue(REQUEST_QUEUE_VARIABLE, request_queue, QUEUE_NAME_LEN)?;
        let channels = Channels::open(
            connection,
            &[
                QueueCheck::configured(REQUEST_QUEUE_VARIABLE, request_queue),
                QueueCheck::literal(coacct01::REPLY_QUEUE),
                QueueCheck::literal(coacct01::ERROR_QUEUE),
            ],
        )
        .await?;
        Ok(Self {
            channels,
            request_queue: request_queue.to_owned(),
            store,
        })
    }
}

impl<A: Acknowledger> Worker<A> {
    /// This worker with `acknowledger` settling every request in place of the consumer channel,
    /// the seam of the failure-injection tests. The channels, queue and store are kept.
    #[must_use]
    pub fn with_acknowledger<B: Acknowledger>(self, acknowledger: B) -> Worker<B> {
        Worker {
            channels: self.channels.with_acknowledger(acknowledger),
            request_queue: self.request_queue,
            store: self.store,
        }
    }
}

impl<A: Acknowledger> fmt::Debug for Worker<A> {
    /// The request queue only: channels and store have nothing a reader of a log needs.
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

    /// Takes the next request and answers it inside its `mq.message` span: a reply published
    /// to `CARD.DEMO.REPLY.ACCT` with the request's IDs and then acknowledged, or the
    /// account-read error body published to `CARD.DEMO.ERROR` and then acknowledged
    /// (COACCT01-001, COACCT01-023 to COACCT01-026, COACCT01-028, COACCT01-032). An unconfirmed
    /// reply routes `MQPUT ERR` to `CARD.DEMO.ERROR`, and an unconfirmed error message discards
    /// the request (COACCT01-033, COACCT01-034).
    async fn process_next(&mut self, connection: &Connection) -> Result<Processed, StepError> {
        let Self {
            channels,
            request_queue,
            store,
        } = self;
        let Some(request) = channels.next(request_queue).await? else {
            return Ok(Processed::Idle);
        };
        let span = super::message_span(request_queue, &request);
        let processed = async {
            match answer(store, request_queue, &request.body).await {
                CdraResponse::Reply(body) => {
                    super::reply_fixed(channels, connection, &request, Flow::Cdra, &body).await
                }
                CdraResponse::Error(body) => {
                    super::route_error(channels, connection, &request, Flow::Cdra, &body).await
                }
            }
        }
        .instrument(span)
        .await;
        Ok(processed)
    }

    async fn reopen(&mut self, connection: &Connection) -> Result<(), AmqpError> {
        self.channels.reopen(connection).await
    }
}

/// The body `4000-PROCESS-REQUEST-REPLY` publishes for the request `body` taken from
/// `request_queue`, reading the account through `store` only when `decide` asks for it
/// (`COACCT01.cbl:390-460`):
///
/// - a body that does not decode: the masked reply of `undecoded_reply` (COACCT01-003);
/// - a function other than `INQA` or a zero key: the bad-parameter reply, with no read
///   (COACCT01-005, COACCT01-022);
/// - otherwise exactly one `find_by_id` by the key (COACCT01-004), whose outcome `respond` turns
///   into the found or not-found reply or the read-failure error body (COACCT01-006 to
///   COACCT01-021, COACCT01-028).
async fn answer<R: AccountRepository>(
    store: &mut R,
    request_queue: &str,
    body: &[u8],
) -> CdraResponse {
    let request = match coacct01::decode_request(body) {
        Ok(request) => request,
        // HALTED(X-COACCT01-H1), HALTED(X-COACCT01-H2): a short body or a non-digit key has no
        // single COBOL result; the masked reply is interim handling, never a parity result.
        Err(error) => {
            tracing::warn!(
                error = %error,
                "the request does not decode; the masked bad-parameter reply answers it"
            );
            return CdraResponse::Reply(coacct01::undecoded_reply());
        }
    };
    let id = match coacct01::decide(&request) {
        CdraAction::ReadAccount(id) => id,
        CdraAction::Reply(reply) => return CdraResponse::Reply(*reply),
    };
    let read = store.find_by_id(id).await;
    let lookup = match &read {
        Ok(Some(account)) => AccountLookup::Found(account),
        Ok(None) => AccountLookup::NotFound,
        Err(error) => {
            tracing::warn!(
                error = %error,
                "the account read failed; the error message goes to the error queue"
            );
            AccountLookup::ReadFailed
        }
    };
    coacct01::respond(lookup, request_queue)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{FixedRecord, PicU};
    use domain::messaging::coacct01::{self, AccountLookup, CdraAction, CdraResponse};
    use domain::records::AccountRecord;
    use persistence::{AccountCustomerChain, AccountRepository, ChainLink, PersistenceError};

    use super::{QUEUE_NAME_LEN, REQUEST_QUEUE_VARIABLE, answer};
    use crate::broker::AmqpError;
    use crate::workers::check_request_queue;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The configured request queue, the default of `CARDDEMO_QUEUE_REQUEST_ACCT`.
    const REQUEST_QUEUE: &str = "CARD.DEMO.REQUEST.ACCT";

    /// The data fields of a synthetic account 1, offsets 0 to 111; `ACCT-GROUP-ID` and `FILLER`
    /// after them are spaces.
    const ACCOUNT_1_DATA: &str = concat!(
        "00000000001",  // ACCT-ID
        "Y",            // ACCT-ACTIVE-STATUS
        "00000012345{", // ACCT-CURR-BAL
        "00000050000{", // ACCT-CREDIT-LIMIT
        "00000020000{", // ACCT-CASH-CREDIT-LIMIT
        "2020-01-31",   // ACCT-OPEN-DATE
        "2029-12-31",   // ACCT-EXPIRAION-DATE
        "2024-06-30",   // ACCT-REISSUE-DATE
        "00000000150A", // ACCT-CURR-CYC-CREDIT
        "00000000075}", // ACCT-CURR-CYC-DEBIT
        "A000000000",   // ACCT-ADDR-ZIP
    );

    /// An in-memory `accounts` table, or one whose every statement fails as an unreachable
    /// database does. It records the keys [`AccountRepository::find_by_id`] is asked for.
    struct Accounts {
        table: Result<Vec<AccountRecord>, PersistenceError>,
        reads: Vec<PicU<11>>,
    }

    impl Accounts {
        fn holding(rows: Vec<AccountRecord>) -> Self {
            Self {
                table: Ok(rows),
                reads: Vec::new(),
            }
        }

        fn unreachable() -> Self {
            Self {
                table: Err(PersistenceError::Unavailable),
                reads: Vec::new(),
            }
        }

        fn rows(&mut self) -> Result<&mut Vec<AccountRecord>, PersistenceError> {
            self.table.as_mut().map_err(|error| error.clone())
        }

        fn keyed(&mut self, id: PicU<11>) -> Result<Option<AccountRecord>, PersistenceError> {
            Ok(self.rows()?.iter().find(|row| row.id == id).cloned())
        }
    }

    impl AccountRepository for Accounts {
        async fn find_with_customer(&mut self, _acct_id: PicU<11>) -> AccountCustomerChain {
            // The double holds no cross-reference rows, so the chain ends at its first read.
            let xref = match self.rows() {
                Ok(_) => ChainLink::NotFound,
                Err(error) => ChainLink::Failed(error),
            };
            AccountCustomerChain {
                xref,
                account: ChainLink::NotReached,
                customer: ChainLink::NotReached,
            }
        }

        async fn find_by_id(
            &mut self,
            id: PicU<11>,
        ) -> Result<Option<AccountRecord>, PersistenceError> {
            self.reads.push(id);
            self.keyed(id)
        }

        async fn lock_by_id(
            &mut self,
            id: PicU<11>,
        ) -> Result<Option<AccountRecord>, PersistenceError> {
            self.keyed(id)
        }

        async fn find_all(&mut self) -> Result<Vec<AccountRecord>, PersistenceError> {
            Ok(self.rows()?.clone())
        }

        async fn insert(&mut self, record: &AccountRecord) -> Result<(), PersistenceError> {
            let rows = self.rows()?;
            if rows.iter().any(|row| row.id == record.id) {
                return Err(PersistenceError::Duplicate {
                    constraint: Some("accounts_pkey".to_owned()),
                });
            }
            rows.push(record.clone());
            Ok(())
        }

        async fn update(&mut self, record: &AccountRecord) -> Result<bool, PersistenceError> {
            match self.rows()?.iter_mut().find(|row| row.id == record.id) {
                Some(row) => {
                    row.clone_from(record);
                    Ok(true)
                }
                None => Ok(false),
            }
        }

        async fn replace_all(
            &mut self,
            records: &[AccountRecord],
        ) -> Result<u64, PersistenceError> {
            records.clone_into(self.rows()?);
            u64::try_from(records.len()).map_err(|_| PersistenceError::Database {
                sqlstate: None,
                constraint: None,
            })
        }
    }

    /// One request body, the store it is answered from, and the body and reads expected.
    struct Case {
        name: &'static str,
        body: Vec<u8>,
        store: Accounts,
        expected: CdraResponse,
        reads: Vec<PicU<11>>,
    }

    /// The synthetic account 1 as codec A decodes its 300 bytes.
    fn account_1() -> Result<AccountRecord, Box<dyn StdError>> {
        let mut line = ACCOUNT_1_DATA.as_bytes().to_vec();
        line.resize(AccountRecord::WIDTH, b' ');
        Ok(AccountRecord::decode(&line)?)
    }

    fn key(id: u64) -> Result<PicU<11>, Box<dyn StdError>> {
        Ok(PicU::new(id)?)
    }

    /// The bad-parameter reply the domain decides for `body` without a read.
    fn bad_parameter(body: &[u8]) -> Result<CdraResponse, Box<dyn StdError>> {
        match coacct01::decide(&coacct01::decode_request(body)?) {
            CdraAction::Reply(reply) => Ok(CdraResponse::Reply(*reply)),
            CdraAction::ReadAccount(_) => Err("the domain reads an account for this body".into()),
        }
    }

    fn cases() -> Result<Vec<Case>, Box<dyn StdError>> {
        let account = account_1()?;
        let mut padded = b"INQA00000000001".to_vec();
        padded.resize(1_000, b' ');
        Ok(vec![
            Case {
                name: "a body shorter than WS-FUNC and WS-KEY",
                body: b"INQA0000000".to_vec(),
                store: Accounts::holding(vec![account.clone()]),
                expected: CdraResponse::Reply(coacct01::undecoded_reply()),
                reads: Vec::new(),
            },
            Case {
                name: "a key byte that is not a digit",
                body: b"INQA0000000000X".to_vec(),
                store: Accounts::holding(vec![account.clone()]),
                expected: CdraResponse::Reply(coacct01::undecoded_reply()),
                reads: Vec::new(),
            },
            Case {
                name: "a function other than INQA",
                body: b"inqa00000000001".to_vec(),
                store: Accounts::holding(vec![account.clone()]),
                expected: bad_parameter(b"inqa00000000001")?,
                reads: Vec::new(),
            },
            Case {
                name: "a zero key",
                body: b"INQA00000000000".to_vec(),
                store: Accounts::holding(vec![account.clone()]),
                expected: bad_parameter(b"INQA00000000000")?,
                reads: Vec::new(),
            },
            Case {
                name: "a stored account, the request padded to the 1,000-byte buffer",
                body: padded,
                store: Accounts::holding(vec![account.clone()]),
                expected: coacct01::respond(AccountLookup::Found(&account), REQUEST_QUEUE),
                reads: vec![key(1)?],
            },
            Case {
                name: "a key no account holds",
                body: b"INQA00000000002".to_vec(),
                store: Accounts::holding(vec![account.clone()]),
                expected: coacct01::respond(AccountLookup::NotFound, REQUEST_QUEUE),
                reads: vec![key(2)?],
            },
            Case {
                name: "a read that fails",
                body: b"INQA00000000001".to_vec(),
                store: Accounts::unreachable(),
                expected: coacct01::respond(AccountLookup::ReadFailed, REQUEST_QUEUE),
                reads: vec![key(1)?],
            },
        ])
    }

    // audit: COACCT01-004
    // audit: COACCT01-005
    // audit: COACCT01-021
    // audit: COACCT01-028
    #[tokio::test]
    async fn each_request_is_answered_from_at_most_one_read_by_its_key() -> TestResult {
        for mut case in cases()? {
            let response = answer(&mut case.store, REQUEST_QUEUE, &case.body).await;
            assert_eq!(response, case.expected, "{}", case.name);
            assert_eq!(case.store.reads, case.reads, "{}", case.name);
        }
        Ok(())
    }

    #[test]
    fn the_request_queue_must_fit_mq_appl_queue_name_whole() {
        let longest = "Q".repeat(QUEUE_NAME_LEN);
        for name in [REQUEST_QUEUE, "CARD DEMO REQUEST ACCT", longest.as_str()] {
            assert!(
                check_request_queue(REQUEST_QUEUE_VARIABLE, name, QUEUE_NAME_LEN).is_ok(),
                "{name}"
            );
        }
        let too_long = "Q".repeat(QUEUE_NAME_LEN + 1);
        for name in [
            too_long.as_str(),
            "CARD.DEMO\tREQUEST.ACCT",
            "CARD.DEMO.REQUÊTE.ACCT",
            "CARD.DEMO.REQUEST.\u{7f}",
        ] {
            let error = check_request_queue(REQUEST_QUEUE_VARIABLE, name, QUEUE_NAME_LEN).err();
            assert!(
                matches!(
                    error,
                    Some(AmqpError::QueueName {
                        variable: REQUEST_QUEUE_VARIABLE,
                        max_len: 48,
                    })
                ),
                "{name:?}: {error:?}"
            );
            let text = error.map(|error| error.to_string()).unwrap_or_default();
            assert!(text.contains(REQUEST_QUEUE_VARIABLE), "{text}");
            assert!(!text.contains(name), "{text}");
        }
    }

    #[test]
    fn a_refused_queue_name_is_a_configuration_error_no_reconnect_cures() {
        let error = AmqpError::QueueName {
            variable: REQUEST_QUEUE_VARIABLE,
            max_len: QUEUE_NAME_LEN,
        };
        assert!(!error.is_connection_lost());
        assert!(error.source().is_none());
    }
}
