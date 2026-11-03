//! Cancellation safety of the store's statement savepoints against a migrated
//! `PostgreSQL` 16.15 (`carddemo_persistence::tx`, "Abandoned calls";
//! AAP 0.6.4).
//!
//! `carddemo-messaging` drops a port call's future when the call outlives its
//! deadline and goes on using the same unit of work: CP00 writes the
//! pending-authorization summary, then the detail, then commits, and a summary
//! written before a failed detail must persist (P-COPAUA0C-035). Each case
//! here runs one unit of work through a TCP proxy that can hold the server's
//! responses, and drops a store call at a chosen point:
//!
//! 1. while the `RELEASE SAVEPOINT` of the summary's savepoint has run on the
//!    server but its response is held;
//! 2. while the `SAVEPOINT` of the call has run but its response is held;
//! 3. while its own statement waits for a row lock another session holds;
//! 4. after its statement failed on the server, while the `ROLLBACK TO
//!    SAVEPOINT` has run but its response is held.
//!
//! In each case the summary written before the dropped call commits, the
//! dropped call's statement does not, and a later store call in the same unit
//! of work succeeds and commits with it, inside the one transaction until the
//! `COMMIT`. A fifth case fails a statement that runs outside any savepoint,
//! which aborts the transaction: later store calls are refused and the commit
//! fails rather than report success for the server's `ROLLBACK`.
//!
//! The test starts its own container through `carddemo_testkit::TestStack`,
//! so it needs a running Docker daemon (AAP 0.10.4). Committed state is read
//! through the stack's text-typed SQL helpers on connections that bypass the
//! proxy, so the test adds no compile-time query. It ends by closing the
//! proxied pool and tearing the proxy down, which frees the proxy's port and
//! ends its threads. A second test, which needs no Docker, checks that
//! teardown against a loopback listener standing in for the server.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io::{ErrorKind, Read, Write};
use std::mem;
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::pin::pin;
use std::str::FromStr;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Waker};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use carddemo_domain::entities::{
    FraudFlag, MatchStatus, NewPendingAuthDetail, PA_ACCOUNT_STATUS_OCCURS, PendingAuthSummary,
};
use carddemo_domain::numeric::{
    AccountId, Alnum, AuthCount, CardNumber, CustomerId, Money, NumericCode,
};
use carddemo_domain::ports::{AuthorizationStore, PortError};
use carddemo_persistence::{Db, PersistenceError};
use carddemo_testkit::{TestStack, block_on};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions, PgSslMode};

/// How long a case waits for the server to reach the point it drops a call at.
const PATIENCE: Duration = Duration::from_secs(30);

/// How long one wait for a held response lasts before the call is polled
/// again.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// How long a proxy's teardown waits for its threads to finish once every
/// socket they read or write is shut down.
const TEARDOWN_PATIENCE: Duration = Duration::from_secs(10);

/// The account of each case, one summary each.
const RELEASE_CASE: u64 = 101;
const SAVEPOINT_CASE: u64 = 102;
const BLOCKED_CASE: u64 = 103;
const ROLLBACK_CASE: u64 = 104;
const UNGUARDED_CASE: u64 = 105;

/// The statement whose response the proxy starts to hold, with every response
/// after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hold {
    Savepoint,
    Release,
    RollbackTo,
}

impl Hold {
    /// Whether `sql`, the text of an executed statement, is this one.
    fn matches(self, sql: &str) -> bool {
        let sql = sql.trim_start().to_ascii_uppercase();
        match self {
            Self::Savepoint => sql.starts_with("SAVEPOINT"),
            Self::Release => sql.starts_with("RELEASE"),
            Self::RollbackTo => sql.starts_with("ROLLBACK TO"),
        }
    }
}

/// What the proxy holds, and the requests whose responses it waits for.
#[derive(Default)]
struct Gate {
    /// The statement whose response is held next; cleared once one matches.
    armed: Option<Hold>,
    /// One entry per request the server has not answered with its
    /// `ReadyForQuery` yet, in order: whether its responses are held.
    requests: VecDeque<bool>,
    /// Whether responses are held, from the first response of the matched
    /// request on.
    holding: bool,
    /// The held responses.
    held: Vec<u8>,
    /// The `ReadyForQuery` messages among the held responses: one means the
    /// server has run and answered the matched statement.
    held_ready: usize,
    /// The client side of the proxied connection, for the release.
    client: Option<TcpStream>,
}

/// The state both directions of the proxied connection share.
#[derive(Default)]
struct Shared {
    gate: Mutex<Gate>,
    held: Condvar,
}

impl Shared {
    fn gate(&self) -> MutexGuard<'_, Gate> {
        self.gate.lock().expect("the proxy state is not poisoned")
    }

    /// A new client connection: nothing is owed on it yet.
    fn connected(&self, client: &TcpStream) {
        let mut gate = self.gate();
        gate.requests.clear();
        gate.client = Some(
            client
                .try_clone()
                .expect("a second handle on the client socket"),
        );
    }

    /// Records one request the client sent, before its bytes reach the
    /// server: `executed` is the SQL it executes, `None` for a request that
    /// only prepares a statement or synchronizes.
    fn requested(&self, executed: Option<&str>) {
        let mut gate = self.gate();
        let hold = match (gate.armed, executed) {
            (Some(hold), Some(sql)) if hold.matches(sql) => {
                gate.armed = None;
                true
            }
            _ => false,
        };
        gate.requests.push_back(hold);
    }

    /// Passes one server `message` to `client`, or holds it.
    fn responded(&self, message: &[u8], client: &mut TcpStream) -> std::io::Result<()> {
        let mut gate = self.gate();
        if !gate.holding && gate.requests.front() == Some(&true) {
            gate.holding = true;
        }
        let ready = message.first() == Some(&b'Z');
        if ready {
            gate.requests.pop_front();
        }
        if gate.holding {
            gate.held.extend_from_slice(message);
            if ready {
                gate.held_ready += 1;
                self.held.notify_all();
            }
            Ok(())
        } else {
            client.write_all(message)
        }
    }
}

/// A TCP proxy for one `PostgreSQL` connection at a time that can hold the
/// server's responses from a chosen statement on, which keeps a call waiting
/// after the server has run its request, as a late response does.
///
/// It reads the frontend protocol (the untyped startup message, then typed
/// messages) to learn the SQL of each request: a simple `Query`, or the
/// prepared statement a `Bind` names followed by an `Execute`, up to the
/// `Sync`. Every request gets exactly one `ReadyForQuery`, so the responses
/// are matched to the requests in order. TLS is off on the proxied pool, so
/// the stream is plain.
///
/// The proxy owns its threads and sockets, and [`Proxy::shut_down`] stops
/// them; dropping a proxy that has not been shut down runs it too. The
/// teardown marks the proxy as stopping, then connects to the proxy's own
/// port to wake the accept loop, which closes that connection without
/// dialing the server and returns, closing the listening socket. It shuts
/// down both sockets of every proxied connection and the client handle the
/// [`Gate`] keeps, which ends each forwarding thread's blocked read or write,
/// and then waits at most [`TEARDOWN_PATIENCE`] for every thread to finish.
/// A connection accepted while the teardown runs is closed, never proxied.
struct Proxy {
    port: u16,
    shared: Arc<Shared>,
    /// The proxied connections, for the teardown.
    connections: Arc<Mutex<Connections>>,
    /// The accept loop, until the teardown takes it.
    accept: Option<JoinHandle<()>>,
    /// What the teardown found, once it has run.
    outcome: Option<Teardown>,
}

/// The forwarding threads and sockets of a proxy, which its teardown stops.
#[derive(Default)]
struct Connections {
    /// Set by the teardown: the accept loop proxies no connection after it.
    stopping: bool,
    /// A handle on the client socket and on the server socket of every
    /// proxied connection.
    sockets: Vec<TcpStream>,
    /// The two forwarding threads of every proxied connection.
    workers: Vec<JoinHandle<()>>,
}

/// Locks `connections`. A thread that panicked while holding the lock left
/// every recorded handle in place, so the poison is ignored, and the teardown,
/// which also runs while a failed test unwinds, never panics on it.
fn lock(connections: &Mutex<Connections>) -> MutexGuard<'_, Connections> {
    connections.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What a proxy's teardown found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Teardown {
    /// Whether the accept loop, which owns the listening socket, finished, so
    /// the socket is closed and its port free.
    listener_closed: bool,
    /// How many forwarding threads still ran at the deadline; each is left
    /// detached.
    workers_running: usize,
    /// How many of the finished threads, the accept loop included, ended in a
    /// panic.
    panicked: usize,
}

impl Teardown {
    /// Every thread finished without a panic, the accept loop with its
    /// listening socket among them.
    const CLEAN: Self = Self {
        listener_closed: true,
        workers_running: 0,
        panicked: 0,
    };
}

impl Proxy {
    /// Listens on a loopback port and proxies every connection to `upstream`.
    fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
        let port = listener.local_addr().expect("the proxy address").port();
        let shared = Arc::new(Shared::default());
        let connections = Arc::new(Mutex::new(Connections::default()));
        let accepted = Arc::clone(&shared);
        let recorded = Arc::clone(&connections);
        let accept = thread::spawn(move || {
            accept_connections(&listener, upstream, &accepted, &recorded);
        });
        Self {
            port,
            shared,
            connections,
            accept: Some(accept),
            outcome: None,
        }
    }

    /// Stops the proxy as its type's documentation describes, and reports
    /// what stopping it found. A second call only reports the first call's
    /// outcome.
    #[must_use = "the outcome says whether every thread finished"]
    fn shut_down(&mut self) -> Teardown {
        if let Some(outcome) = self.outcome {
            return outcome;
        }
        let (sockets, workers) = {
            let mut connections = lock(&self.connections);
            connections.stopping = true;
            (
                mem::take(&mut connections.sockets),
                mem::take(&mut connections.workers),
            )
        };
        // Kept open until the wait ends, so the accept loop accepts it; it is
        // refused when the loop has returned already.
        let wake = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        for socket in &sockets {
            let _ = socket.shutdown(Shutdown::Both);
        }
        let client = self
            .shared
            .gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .client
            .take();
        if let Some(client) = &client {
            let _ = client.shutdown(Shutdown::Both);
        }
        let accept = self.accept.take();
        let deadline = Instant::now() + TEARDOWN_PATIENCE;
        while !accept.iter().chain(&workers).all(JoinHandle::is_finished)
            && Instant::now() < deadline
        {
            thread::sleep(POLL_INTERVAL);
        }
        drop(wake);
        let (mut finished, running): (Vec<_>, Vec<_>) =
            workers.into_iter().partition(JoinHandle::is_finished);
        let listener_closed = match accept {
            Some(accept) if accept.is_finished() => {
                finished.push(accept);
                true
            }
            Some(_) => false,
            None => true,
        };
        let outcome = Teardown {
            listener_closed,
            workers_running: running.len(),
            panicked: finished
                .into_iter()
                .map(JoinHandle::join)
                .filter(Result::is_err)
                .count(),
        };
        self.outcome = Some(outcome);
        outcome
    }

    /// Holds the responses from the next executed `hold` statement on.
    fn arm(&self, hold: Hold) {
        self.shared.gate().armed = Some(hold);
    }

    /// Whether the server has answered the held statement, waiting at most
    /// `wait` for it.
    fn holds_answer(&self, wait: Duration) -> bool {
        let gate = self.shared.gate();
        let (gate, _) = self
            .shared
            .held
            .wait_timeout_while(gate, wait, |gate| gate.held_ready == 0)
            .expect("the proxy state is not poisoned");
        gate.held_ready > 0
    }

    /// Passes every held response to the client and stops holding.
    fn release(&self) {
        let mut gate = self.shared.gate();
        gate.armed = None;
        gate.holding = false;
        gate.held_ready = 0;
        let held = mem::take(&mut gate.held);
        if let Some(client) = gate.client.as_mut() {
            client
                .write_all(&held)
                .expect("the held responses reach the client");
        }
    }
}

impl Drop for Proxy {
    /// Runs the teardown unless [`Proxy::shut_down`] already has, and fails
    /// the test when it is not clean. While a failed test unwinds it only
    /// reports the outcome, because a panic then would abort the process.
    fn drop(&mut self) {
        if self.outcome.is_some() {
            return;
        }
        let outcome = self.shut_down();
        if thread::panicking() {
            if outcome != Teardown::CLEAN {
                eprintln!("the dropped proxy's teardown is not clean: {outcome:?}");
            }
        } else {
            assert_eq!(
                outcome,
                Teardown::CLEAN,
                "the dropped proxy's teardown is clean"
            );
        }
    }
}

/// The accept loop: proxies every connection to `upstream` until the
/// teardown marks `connections` as stopping, or until an accept or the
/// connection to the server fails. The loop's thread owns `listener`, which
/// closes when the loop returns.
fn accept_connections(
    listener: &TcpListener,
    upstream: SocketAddr,
    shared: &Arc<Shared>,
    connections: &Mutex<Connections>,
) {
    for client in listener.incoming() {
        let Ok(client) = client else { return };
        // The connection that wakes the loop for the teardown is never proxied.
        if lock(connections).stopping {
            return;
        }
        let Ok(server) = TcpStream::connect(upstream) else {
            return;
        };
        let mut connections = lock(connections);
        // A teardown that began during the connection to the server has shut
        // down every socket recorded before it: these two are closed instead.
        if connections.stopping {
            return;
        }
        shared.connected(&client);
        proxy_connection(client, server, shared, &mut connections);
    }
}

/// Starts the two forwarding threads of one proxied connection, and records
/// them and a handle on each of its sockets in `connections`.
fn proxy_connection(
    client: TcpStream,
    server: TcpStream,
    shared: &Arc<Shared>,
    connections: &mut Connections,
) {
    let client_reader = client
        .try_clone()
        .expect("a second handle on the client socket");
    let server_writer = server
        .try_clone()
        .expect("a second handle on the server socket");
    connections.sockets.push(
        client
            .try_clone()
            .expect("a third handle on the client socket"),
    );
    connections.sockets.push(
        server
            .try_clone()
            .expect("a third handle on the server socket"),
    );
    let requests = Arc::clone(shared);
    connections.workers.push(thread::spawn(move || {
        forward_requests(client_reader, server_writer, &requests);
    }));
    let responses = Arc::clone(shared);
    connections.workers.push(thread::spawn(move || {
        forward_responses(server, client, &responses);
    }));
}

/// Client to server: every byte is forwarded, after the requests it completes
/// are recorded.
fn forward_requests(mut client: TcpStream, mut server: TcpStream, shared: &Shared) {
    let mut requests = Requests::default();
    let mut buffer = [0_u8; 8192];
    while let Ok(read) = client.read(&mut buffer) {
        if read == 0 {
            break;
        }
        requests.read(&buffer[..read], shared);
        if server.write_all(&buffer[..read]).is_err() {
            break;
        }
    }
    let _ = server.shutdown(Shutdown::Write);
}

/// Server to client: every complete message is passed on or held.
fn forward_responses(mut server: TcpStream, mut client: TcpStream, shared: &Shared) {
    let mut pending = Vec::new();
    let mut buffer = [0_u8; 8192];
    'read: while let Ok(read) = server.read(&mut buffer) {
        if read == 0 {
            break;
        }
        pending.extend_from_slice(&buffer[..read]);
        for message in take_messages(&mut pending) {
            if shared.responded(&message, &mut client).is_err() {
                break 'read;
            }
        }
    }
    let _ = client.shutdown(Shutdown::Both);
}

/// The frontend messages read so far on one connection.
#[derive(Default)]
struct Requests {
    /// Bytes of a message not yet complete.
    pending: Vec<u8>,
    /// Whether the untyped startup message has been read.
    started: bool,
    /// The SQL of each statement a `Parse` prepared, by name.
    statements: HashMap<String, String>,
    /// The SQL of the statement the current request binds.
    bound: Option<String>,
    /// Whether the current request executes its portal.
    executes: bool,
}

impl Requests {
    fn read(&mut self, bytes: &[u8], shared: &Shared) {
        self.pending.extend_from_slice(bytes);
        if !self.started {
            let Some(length) = length_at(&self.pending, 0) else {
                return;
            };
            if self.pending.len() < length {
                return;
            }
            self.pending.drain(..length);
            self.started = true;
        }
        for message in take_messages(&mut self.pending) {
            let body = message.get(5..).unwrap_or_default();
            self.message(message[0], body, shared);
        }
    }

    fn message(&mut self, kind: u8, body: &[u8], shared: &Shared) {
        match kind {
            b'P' => {
                let (name, rest) = c_string(body);
                let (sql, _) = c_string(rest);
                self.statements.insert(name, sql);
            }
            b'B' => {
                let (_portal, rest) = c_string(body);
                let (name, _) = c_string(rest);
                self.bound = self.statements.get(&name).cloned();
            }
            b'E' => self.executes = true,
            b'S' => {
                let executed = self.bound.take().filter(|_| self.executes);
                self.executes = false;
                shared.requested(executed.as_deref());
            }
            b'Q' => {
                let (sql, _) = c_string(body);
                shared.requested(Some(&sql));
            }
            _ => {}
        }
    }
}

/// The big-endian `Int32` at `at` in `bytes`, when the bytes reach that far.
fn length_at(bytes: &[u8], at: usize) -> Option<usize> {
    let field: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
    usize::try_from(u32::from_be_bytes(field)).ok()
}

/// Removes every complete typed message from the front of `pending`: a type
/// byte, then an `Int32` length that counts itself and the body.
fn take_messages(pending: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut messages = Vec::new();
    while let Some(length) = length_at(pending, 1) {
        let total = length + 1;
        if pending.len() < total {
            break;
        }
        messages.push(pending.drain(..total).collect());
    }
    messages
}

/// The NUL-terminated string at the start of `bytes`, and the bytes after it.
fn c_string(bytes: &[u8]) -> (String, &[u8]) {
    let end = bytes
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]).into_owned();
    (text, bytes.get(end + 1..).unwrap_or_default())
}

/// Polls `call` until the proxy holds the server's answer to its `hold`
/// statement, then drops it, as a deadline drops a call whose response is
/// late, and lets the held responses through.
fn abandon_at<F: Future>(proxy: &Proxy, hold: Hold, call: F) {
    proxy.arm(hold);
    {
        let mut call = pin!(call);
        let mut context = Context::from_waker(Waker::noop());
        let deadline = Instant::now() + PATIENCE;
        while !proxy.holds_answer(POLL_INTERVAL) {
            assert!(
                call.as_mut().poll(&mut context).is_pending(),
                "the call finished although the answer to its {hold:?} is held"
            );
            assert!(
                Instant::now() < deadline,
                "the call never had its {hold:?} answered"
            );
        }
    }
    proxy.release();
}

/// Polls `call` until the server shows a statement waiting for a lock, then
/// drops it.
async fn abandon_when_blocked<F: Future>(stack: &TestStack, call: F) {
    let mut call = pin!(call);
    let deadline = Instant::now() + PATIENCE;
    loop {
        let mut context = Context::from_waker(Waker::noop());
        assert!(
            call.as_mut().poll(&mut context).is_pending(),
            "the write finished although its row is locked"
        );
        let waiting = text(
            stack,
            "SELECT count(*) FROM pg_stat_activity WHERE wait_event_type = 'Lock'",
        )
        .await;
        if waiting.as_deref() != Some("0") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the write never waited for the lock"
        );
    }
}

/// The first column of the first row `sql` reads, as text.
async fn text(stack: &TestStack, sql: &str) -> Option<String> {
    stack
        .fetch_text_rows(sql)
        .await
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next())
        .flatten()
}

/// The second column of the row of `account` among the rows `sql` reads,
/// whose first column is the account number.
async fn of_account(stack: &TestStack, sql: &str, account_id: u64) -> Option<String> {
    let key = account_id.to_string();
    stack
        .fetch_text_rows(sql)
        .await
        .into_iter()
        .find(|row| row.first().and_then(Option::as_deref) == Some(key.as_str()))
        .and_then(|row| row.into_iter().nth(1))
        .flatten()
}

/// The committed `PA-APPROVED-AUTH-CNT` of `account`'s summary, the mark each
/// summary write leaves.
async fn committed_count(stack: &TestStack, account_id: u64) -> Option<String> {
    of_account(
        stack,
        "SELECT pa_acct_id, pa_approved_auth_cnt FROM pending_auth_summaries",
        account_id,
    )
    .await
}

/// How many details of `account` are committed.
async fn committed_details(stack: &TestStack, account_id: u64) -> String {
    of_account(
        stack,
        "SELECT pa_acct_id, count(*) FROM pending_auth_details GROUP BY pa_acct_id",
        account_id,
    )
    .await
    .unwrap_or_else(|| "0".to_owned())
}

fn account(value: u64) -> AccountId {
    AccountId::new(value).expect("a 9(11) account number")
}

fn cents<const D: u8>(value: i64) -> Money<D> {
    Money::from_cents(value).expect("an amount within the field")
}

/// A summary of `account` whose approved count is `approved`.
fn summary(account_id: u64, approved: i16) -> PendingAuthSummary {
    PendingAuthSummary {
        pa_acct_id: account(account_id),
        pa_cust_id: CustomerId::new(1).expect("a customer number"),
        pa_auth_status: Alnum::move_from("A"),
        pa_account_status: [Alnum::spaces(); PA_ACCOUNT_STATUS_OCCURS],
        pa_credit_limit: cents(2_020_000),
        pa_cash_limit: cents(1_020_000),
        pa_credit_balance: cents(10_000),
        pa_cash_balance: Money::ZERO,
        pa_approved_auth_cnt: AuthCount::new(approved).expect("a count of at most four digits"),
        pa_declined_auth_cnt: AuthCount::new(0).expect("a count of at most four digits"),
        pa_approved_auth_amt: cents(10_000),
        pa_declined_auth_amt: Money::ZERO,
    }
}

/// An approved detail of `account`, keyed by `time_9c` under one date.
fn detail(account_id: u64, time_9c: i32) -> NewPendingAuthDetail {
    NewPendingAuthDetail {
        pa_acct_id: account(account_id),
        pa_auth_date_9c: 99_999 - 22_182,
        pa_auth_time_9c: time_9c,
        pa_auth_orig_date: Alnum::move_from("220701"),
        pa_auth_orig_time: Alnum::move_from("120000"),
        pa_card_num: CardNumber::move_from("4111000000000001"),
        pa_auth_type: Alnum::move_from("SALE"),
        pa_card_expiry_date: Alnum::move_from("0525"),
        pa_message_type: Alnum::move_from("0100"),
        pa_message_source: Alnum::move_from("POS"),
        pa_auth_id_code: Alnum::move_from("120000"),
        pa_auth_resp_code: Alnum::move_from("00"),
        pa_auth_resp_reason: Alnum::move_from("0000"),
        pa_processing_code: NumericCode::new(0).expect("a six-digit code"),
        pa_transaction_amt: cents(4_000),
        pa_approved_amt: cents(3_000),
        pa_merchant_catagory_code: Alnum::move_from("5411"),
        pa_acqr_country_code: Alnum::move_from("840"),
        pa_pos_entry_mode: NumericCode::new(1).expect("a two-digit code"),
        pa_merchant_id: Alnum::move_from("800000000"),
        pa_merchant_name: Alnum::move_from("Abshire-Lowe"),
        pa_merchant_city: Alnum::move_from("Enoshaven"),
        pa_merchant_state: Alnum::move_from("NC"),
        pa_merchant_zip: Alnum::move_from("72112"),
        pa_transaction_id: Alnum::move_from("000000000000001"),
        pa_match_status: MatchStatus::Pending,
        pa_auth_fraud: FraudFlag::Blank,
        pa_fraud_rpt_date: Alnum::spaces(),
    }
}

/// Commits a summary of `account` with an approved count of 1, outside the
/// proxy.
async fn committed_summary(direct: &Db, account_id: u64) {
    let mut uow = direct.begin().await.expect("a unit of work");
    uow.store()
        .insert_summary(summary(account_id, 1))
        .await
        .expect("the summary is inserted");
    uow.commit().await.expect("the summary commits");
}

/// The `postgres://` address of the stack's database.
fn upstream(stack: &TestStack) -> SocketAddr {
    let options = PgConnectOptions::from_str(stack.database_url()).expect("the stack's URL");
    (options.get_host(), options.get_port())
        .to_socket_addrs()
        .expect("the stack's address resolves")
        .next()
        .expect("the stack has an address")
}

/// A pool of one connection to the stack's database through `proxy`.
async fn proxied_pool(stack: &TestStack, proxy: &Proxy) -> PgPool {
    let options = PgConnectOptions::from_str(stack.database_url())
        .expect("the stack's URL")
        .host("127.0.0.1")
        .port(proxy.port)
        .ssl_mode(PgSslMode::Disable);
    PgPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("the proxied pool connects")
}

/// Whether nothing listens on loopback `port` any more: a new listener binds
/// it, or, while another socket holds the port, a connection to it is
/// refused.
fn released(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
        || TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .is_err_and(|error| error.kind() == ErrorKind::ConnectionRefused)
}

/// The detail call is dropped while the `RELEASE SAVEPOINT` of the summary's
/// savepoint has run but is unanswered: the summary still commits with the
/// detail a later call inserts.
async fn an_unanswered_release_keeps_the_summary(stack: &TestStack, db: &Db, proxy: &Proxy) {
    committed_summary(&Db::from_pool(stack.pool().clone()), RELEASE_CASE).await;
    let mut uow = db.begin().await.expect("a unit of work");
    let mut store = uow.store();
    store
        .read_summary(account(RELEASE_CASE))
        .await
        .expect("the summary is read");
    store
        .update_summary(summary(RELEASE_CASE, 2))
        .await
        .expect("the summary is replaced");
    abandon_at(
        proxy,
        Hold::Release,
        store.insert_detail(detail(RELEASE_CASE, 1)),
    );
    store
        .insert_detail(detail(RELEASE_CASE, 1))
        .await
        .expect("a later store call succeeds");
    uow.commit().await.expect("the unit of work commits");
    assert_eq!(
        committed_count(stack, RELEASE_CASE).await.as_deref(),
        Some("2")
    );
    assert_eq!(committed_details(stack, RELEASE_CASE).await, "1");
}

/// The detail call is dropped while its `SAVEPOINT` has run but is
/// unanswered: the later writes stay inside the one transaction, and its
/// `COMMIT` commits the summary with them.
async fn an_unanswered_savepoint_keeps_the_transaction(stack: &TestStack, db: &Db, proxy: &Proxy) {
    committed_summary(&Db::from_pool(stack.pool().clone()), SAVEPOINT_CASE).await;
    let mut uow = db.begin().await.expect("a unit of work");
    let mut store = uow.store();
    store
        .update_summary(summary(SAVEPOINT_CASE, 2))
        .await
        .expect("the summary is replaced");
    abandon_at(
        proxy,
        Hold::Savepoint,
        store.insert_detail(detail(SAVEPOINT_CASE, 1)),
    );
    store
        .insert_detail(detail(SAVEPOINT_CASE, 1))
        .await
        .expect("a later store call succeeds");
    assert_eq!(
        (
            committed_count(stack, SAVEPOINT_CASE).await.as_deref(),
            committed_details(stack, SAVEPOINT_CASE).await,
        ),
        (Some("1"), "0".to_owned()),
        "a write became visible before the unit of work committed"
    );
    uow.commit().await.expect("the unit of work commits");
    assert_eq!(
        committed_count(stack, SAVEPOINT_CASE).await.as_deref(),
        Some("2")
    );
    assert_eq!(committed_details(stack, SAVEPOINT_CASE).await, "1");
}

/// The second summary write is dropped while it waits for a row lock another
/// session holds: once the lock is gone it is undone, and the detail written
/// before it and the one written after it commit.
async fn a_blocked_write_is_undone_alone(stack: &TestStack, db: &Db) {
    let direct = Db::from_pool(stack.pool().clone());
    committed_summary(&direct, BLOCKED_CASE).await;
    let mut holder = direct
        .begin()
        .await
        .expect("the lock holder's unit of work");
    holder
        .store()
        .update_summary(summary(BLOCKED_CASE, 9))
        .await
        .expect("the lock holder locks the summary");
    let mut uow = db.begin().await.expect("a unit of work");
    let mut store = uow.store();
    store
        .insert_detail(detail(BLOCKED_CASE, 1))
        .await
        .expect("the first detail is inserted");
    abandon_when_blocked(stack, store.update_summary(summary(BLOCKED_CASE, 2))).await;
    holder.rollback().await.expect("the lock holder rolls back");
    store
        .insert_detail(detail(BLOCKED_CASE, 2))
        .await
        .expect("a later store call succeeds");
    uow.commit().await.expect("the unit of work commits");
    assert_eq!(
        committed_count(stack, BLOCKED_CASE).await.as_deref(),
        Some("1"),
        "the dropped summary write was committed"
    );
    assert_eq!(committed_details(stack, BLOCKED_CASE).await, "2");
}

/// A summary insert fails on the server and is dropped while its `ROLLBACK
/// TO SAVEPOINT` has run but is unanswered: the transaction stays usable, and
/// the summary replaced before it commits with the later detail.
async fn an_unanswered_rollback_keeps_the_transaction(stack: &TestStack, db: &Db, proxy: &Proxy) {
    committed_summary(&Db::from_pool(stack.pool().clone()), ROLLBACK_CASE).await;
    let mut uow = db.begin().await.expect("a unit of work");
    let mut store = uow.store();
    store
        .update_summary(summary(ROLLBACK_CASE, 2))
        .await
        .expect("the summary is replaced");
    abandon_at(
        proxy,
        Hold::RollbackTo,
        store.insert_summary(summary(ROLLBACK_CASE, 3)),
    );
    store
        .insert_detail(detail(ROLLBACK_CASE, 1))
        .await
        .expect("a later store call succeeds");
    uow.commit().await.expect("the unit of work commits");
    assert_eq!(
        committed_count(stack, ROLLBACK_CASE).await.as_deref(),
        Some("2")
    );
    assert_eq!(committed_details(stack, ROLLBACK_CASE).await, "1");
}

/// The account-reference lock, which runs outside any savepoint, fails on
/// the server and aborts the transaction: the next store call is refused
/// without reaching the server, and the commit fails with nothing committed.
async fn an_aborted_transaction_is_not_committed(stack: &TestStack, db: &Db) {
    committed_summary(&Db::from_pool(stack.pool().clone()), UNGUARDED_CASE).await;
    stack
        .exec_sql("ALTER TABLE accounts RENAME TO accounts_hidden")
        .await;
    let mut uow = db.begin().await.expect("a unit of work");
    uow.store()
        .update_summary(summary(UNGUARDED_CASE, 2))
        .await
        .expect("the summary is replaced");
    let locked = uow.lock_account_reference(account(UNGUARDED_CASE)).await;
    assert!(
        matches!(locked, Err(PersistenceError::Sqlx(_))),
        "{locked:?}"
    );
    let refused = uow.store().insert_detail(detail(UNGUARDED_CASE, 1)).await;
    assert!(
        matches!(&refused, Err(PortError::Unavailable(text)) if text.contains("unusable")),
        "{refused:?}"
    );
    let committed = uow.commit().await;
    assert!(
        matches!(
            committed,
            Err(PersistenceError::Port(PortError::Unavailable(_)))
        ),
        "{committed:?}"
    );
    stack
        .exec_sql("ALTER TABLE accounts_hidden RENAME TO accounts")
        .await;
    assert_eq!(
        committed_count(stack, UNGUARDED_CASE).await.as_deref(),
        Some("1"),
        "the summary written in the aborted transaction was committed"
    );
    let mut next = db
        .begin()
        .await
        .expect("the connection serves the next unit of work");
    next.store()
        .read_summary(account(UNGUARDED_CASE))
        .await
        .expect("the next unit of work reads");
    next.rollback()
        .await
        .expect("the next unit of work rolls back");
}

#[test]
fn an_abandoned_store_call_is_undone_alone_and_its_unit_of_work_still_commits() {
    block_on(async {
        let stack = TestStack::postgres().await;
        let mut proxy = Proxy::start(upstream(&stack));
        let db = Db::from_pool(proxied_pool(&stack, &proxy).await);
        an_unanswered_release_keeps_the_summary(&stack, &db, &proxy).await;
        an_unanswered_savepoint_keeps_the_transaction(&stack, &db, &proxy).await;
        a_blocked_write_is_undone_alone(&stack, &db).await;
        an_unanswered_rollback_keeps_the_transaction(&stack, &db, &proxy).await;
        an_aborted_transaction_is_not_committed(&stack, &db).await;
        db.close().await;
        assert_eq!(proxy.shut_down(), Teardown::CLEAN);
        assert!(released(proxy.port), "the proxy's port is still taken");
    });
}

/// How the teardown test ends a proxy.
#[derive(Debug, Clone, Copy)]
enum Ending {
    /// [`Proxy::shut_down`], twice: the second call reports the first one's
    /// outcome.
    ShutDown,
    /// Dropping the proxy, which fails the test unless the teardown is clean.
    Dropped,
}

/// Ending a proxy, by its teardown or by dropping it, frees its port, ends
/// its accept loop and the two forwarding threads of a live proxied
/// connection, and closes both ends of that connection. A loopback listener
/// stands in for the server, so the test needs no Docker.
#[test]
fn an_ended_proxy_frees_its_port_ends_its_threads_and_closes_its_connection() {
    for ending in [Ending::ShutDown, Ending::Dropped] {
        let server = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
        let mut proxy = Proxy::start(server.local_addr().expect("the server address"));
        let port = proxy.port;
        let mut client =
            TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("a connection to the proxy");
        let (mut backend, _) = server.accept().expect("the proxy connects to the server");
        for stream in [&client, &backend] {
            stream
                .set_read_timeout(Some(TEARDOWN_PATIENCE))
                .expect("a read timeout");
        }
        // A startup message for protocol 3.0 one way and a `ReadyForQuery`
        // the other prove that both forwarding threads run.
        let startup = [0_u8, 0, 0, 8, 0, 3, 0, 0];
        client.write_all(&startup).expect("the client writes");
        let mut forwarded = [0_u8; 8];
        backend
            .read_exact(&mut forwarded)
            .expect("the request reaches the server");
        assert_eq!(forwarded, startup, "{ending:?}");
        let ready = *b"Z\0\0\0\x05I";
        backend.write_all(&ready).expect("the server writes");
        let mut passed = [0_u8; 6];
        client
            .read_exact(&mut passed)
            .expect("the response reaches the client");
        assert_eq!(passed, ready, "{ending:?}");
        match ending {
            Ending::ShutDown => {
                let outcome = proxy.shut_down();
                assert_eq!(outcome, Teardown::CLEAN, "{ending:?}");
                assert_eq!(proxy.shut_down(), outcome, "{ending:?}: a second call");
                drop(proxy);
            }
            Ending::Dropped => drop(proxy),
        }
        assert!(
            released(port),
            "{ending:?}: the proxy's port is still taken"
        );
        let mut rest = [0_u8; 1];
        assert_eq!(
            client.read(&mut rest).map_err(|error| error.kind()),
            Ok(0),
            "{ending:?}: the client sees the end of the stream"
        );
        assert_eq!(
            backend.read(&mut rest).map_err(|error| error.kind()),
            Ok(0),
            "{ending:?}: the server sees the end of the stream"
        );
    }
}
