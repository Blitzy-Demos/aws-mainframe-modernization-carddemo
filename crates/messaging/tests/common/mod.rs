//! Shared harness of the `carddemo-messaging` integration tests. The targets
//! `cdrd_date.rs`, `cdra_account.rs`, `cp00_authorization.rs` and
//! `ack_after_confirm.rs` each include it with `mod common;`.
//!
//! [`Harness`] starts a seeded `PostgreSQL` and a `RabbitMQ` broker loaded with
//! `infra/rabbitmq/definitions.json`, runs the CDRD, CDRA and CP00 consumers on
//! them through [`carddemo_messaging::start`], and gives the test a
//! confirm-mode client channel of its own. Every log event and message span the
//! consumers emit is captured as the JSON lines the `mq` binary writes to
//! standard output, in a buffer owned by the harness, so tests running side by
//! side in one process never see each other's lines.
//!
//! The MQ message descriptor maps onto AMQP properties as the consumers read
//! them: `MSGID` is `message_id`, `CORRELID` is `correlation_id` and `REPLYTOQ`
//! is `reply_to` [app/app-vsam-mq/cbl/CODATE01.cbl:L313-315]
//! [app/app-vsam-mq/cbl/COACCT01.cbl:L364-366]
//! [app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl:L411-413].
//!
//! [`audit_row`] and [`audit_texts`] read `docs/parity/parity-oracle-audit.md`,
//! where every expected value of these tests comes from.
//!
//! The polling helpers sleep on a blocking thread, because the crate enables no
//! tokio timer.

use std::io;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use carddemo_messaging::Running;
use carddemo_persistence::Db;
use carddemo_runtime::config::{Settings, keys};
use carddemo_runtime::telemetry::{TelemetryGuard, subscriber_with_writer};
use carddemo_testkit::{TestStack, fixtures};
use lapin::message::Delivery;
use lapin::options::{
    BasicAckOptions, BasicGetOptions, BasicPublishOptions, ConfirmSelectOptions,
    QueueDeclareOptions,
};
use lapin::protocol::constants::REPLY_SUCCESS;
use lapin::publisher_confirm::Confirmation;
use lapin::types::FieldTable;
use lapin::{BasicProperties, Channel, Connection, Queue};
use tracing::instrument::WithSubscriber;

/// Interval between two attempts of every polling helper.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long [`Harness::start`] waits for each request queue to report its
/// consumer.
const READY_TIMEOUT: Duration = Duration::from_secs(30);

/// Service name of the captured span lines.
const SERVICE_NAME: &str = "mq-test";

/// Filter directives of the captured telemetry: the `mq` binary's default.
const LOG_DIRECTIVES: &str = "info";

/// File name of the parity oracle audit under `docs/parity`.
const PARITY_AUDIT: &str = "parity-oracle-audit.md";

/// Header of the audit column whose code spans are a row's exact texts.
const EXPECTED_RESULT: &str = "Expected result";

/// Text of the parity oracle audit, read once per test process.
static AUDIT_TEXT: OnceLock<String> = OnceLock::new();

/// One writer onto a harness's capture buffer. The telemetry makes one per
/// line and writes each line in a single call, so lines never interleave.
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Sleeps for `period` on a blocking thread, leaving the runtime's workers to
/// the consumers.
async fn pause(period: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(period))
        .await
        .expect("sleep task");
}

/// A running consumer stack and a client of its broker, owned by one test.
///
/// Build it with [`Harness::start`] and end it with [`Harness::shutdown`],
/// which stops the consumers before the containers go.
pub struct Harness {
    /// The containers: a seeded `PostgreSQL` database and the broker.
    pub stack: TestStack,
    /// The database handle the consumers were started with.
    pub db: Db,
    /// The settings the consumers were started with: the stack's URLs, the
    /// default request queue names and the clock override.
    pub settings: Settings,
    /// The test's own channel on the broker, in publisher-confirm mode.
    pub channel: Channel,
    /// The connection of `channel` and of the helpers' short-lived channels.
    connection: Connection,
    /// The started consumers.
    running: Running,
    /// Every JSON line the consumers' telemetry wrote, in write order.
    buffer: Arc<Mutex<Vec<u8>>>,
    /// Owns the tracer provider of the capture.
    guard: TelemetryGuard,
}

impl Harness {
    /// Starts a seeded stack and the CDRD, CDRA and CP00 consumers on it, with
    /// their telemetry captured for [`Harness::logs`], and returns once every
    /// request queue reports a consumer.
    ///
    /// `clock_override` is the `CARDDEMO_CLOCK_OVERRIDE` value, an RFC 3339
    /// instant that pins every time-dependent output, or `None` for the system
    /// clock. Every other setting keeps its default, and the process
    /// environment is never read or changed, because parallel tests share it.
    ///
    /// # Panics
    ///
    /// Panics if the containers do not start, the seed fails, the settings or
    /// the telemetry cannot be built, the consumers do not start, the client
    /// connection or channel cannot be opened, or a request queue reports no
    /// consumer within 30 seconds.
    pub async fn start(clock_override: Option<&str>) -> Self {
        let stack = TestStack::with_rabbitmq().await;
        stack.seed().await.expect("seed the test database");
        let settings = Settings::from_lookup(|key| match key {
            keys::AMQP_URL => stack.amqp_url().map(str::to_owned),
            keys::DATABASE_URL => Some(stack.database_url().to_owned()),
            keys::CLOCK_OVERRIDE => clock_override.map(str::to_owned),
            _ => None,
        })
        .expect("resolve the test settings");
        let db = Db::from_pool(stack.pool().clone());

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer = {
            let buffer = Arc::clone(&buffer);
            move || Capture(Arc::clone(&buffer))
        };
        let (subscriber, guard) = subscriber_with_writer(SERVICE_NAME, LOG_DIRECTIVES, writer)
            .expect("build the capture telemetry");
        // The subscriber is current only while the consumers start: `start`
        // spawns every consumer task with the dispatcher current at the spawn,
        // so the tasks keep writing here, and no process-wide subscriber mixes
        // the lines of tests that run in parallel.
        let running = carddemo_messaging::start(&settings, db.clone())
            .with_subscriber(subscriber)
            .await
            .expect("start the consumers");

        let connection = stack.amqp_connection().await;
        let channel = connection
            .create_channel()
            .await
            .expect("open the test channel");
        channel
            .confirm_select(ConfirmSelectOptions::default())
            .await
            .expect("put the test channel in confirm mode");

        let harness = Self {
            stack,
            db,
            settings,
            channel,
            connection,
            running,
            buffer,
            guard,
        };
        harness.wait_for_consumers().await;
        harness
    }

    /// Waits until each request queue reports a consumer. The consumers
    /// subscribe inside their spawned tasks, after `start` returns, so a
    /// request or an injected failure could otherwise precede them.
    async fn wait_for_consumers(&self) {
        let queues = [
            &self.settings.queue_date_request,
            &self.settings.queue_acct_request,
            &self.settings.queue_pauth_request,
        ];
        for queue in queues {
            let until = Instant::now() + READY_TIMEOUT;
            while self.passive_declare(queue).await.consumer_count() == 0 {
                assert!(
                    Instant::now() < until,
                    "no consumer on {queue} within {READY_TIMEOUT:?}"
                );
                pause(POLL_INTERVAL).await;
            }
        }
    }

    /// The broker's current state of `queue`, from a passive declare on a
    /// channel of its own: the broker closes the channel of a passive declare
    /// of a missing queue (404), and that must never be the test's `channel`.
    async fn passive_declare(&self, queue: &str) -> Queue {
        let probe = self
            .connection
            .create_channel()
            .await
            .unwrap_or_else(|error| panic!("cannot open a channel to inspect {queue}: {error}"));
        let state = probe
            .queue_declare(
                queue,
                QueueDeclareOptions {
                    passive: true,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .unwrap_or_else(|error| panic!("cannot inspect queue {queue}: {error}"));
        probe
            .close(REPLY_SUCCESS, "inspected")
            .await
            .unwrap_or_else(|error| {
                panic!("cannot close the channel that inspected {queue}: {error}")
            });
        state
    }

    /// Publishes `body` to `queue` through the default exchange, as a client
    /// application puts a request: `message_id`, `correlation_id` and, when
    /// given, `reply_to` carry the MQ `MSGID`, `CORRELID` and `REPLYTOQ`.
    ///
    /// # Panics
    ///
    /// Panics if the publish fails or the broker does not confirm it with a
    /// plain `Ack`. The publish is mandatory, so one to a missing queue comes
    /// back returned and panics too.
    pub async fn publish(
        &self,
        queue: &str,
        body: &[u8],
        message_id: &str,
        correlation_id: &str,
        reply_to: Option<&str>,
    ) {
        let mut properties = BasicProperties::default()
            .with_message_id(message_id.into())
            .with_correlation_id(correlation_id.into());
        if let Some(reply_to) = reply_to {
            properties = properties.with_reply_to(reply_to.into());
        }
        let confirmation = self
            .channel
            .basic_publish(
                "",
                queue,
                BasicPublishOptions {
                    mandatory: true,
                    ..BasicPublishOptions::default()
                },
                body,
                properties,
            )
            .await
            .unwrap_or_else(|error| panic!("cannot publish to {queue}: {error}"))
            .await
            .unwrap_or_else(|error| panic!("no confirmation of the publish to {queue}: {error}"));
        assert!(
            matches!(confirmation, Confirmation::Ack(None)),
            "publish to {queue} not confirmed: {confirmation:?}"
        );
    }

    /// Declares a server-named, exclusive, auto-delete queue on the test's
    /// channel and returns its name.
    ///
    /// Any connection can publish to it through the default exchange, which is
    /// how CP00 replies to a request's `reply_to` arrive. CDRD and CDRA reply to
    /// fixed queues [app/app-vsam-mq/cbl/CODATE01.cbl:L147]
    /// [app/app-vsam-mq/cbl/COACCT01.cbl:L198], so for them it is a `reply_to`
    /// the flows must ignore.
    ///
    /// # Panics
    ///
    /// Panics if the broker refuses the declaration.
    pub async fn temp_reply_queue(&self) -> String {
        let queue = self
            .channel
            .queue_declare(
                "",
                QueueDeclareOptions {
                    exclusive: true,
                    auto_delete: true,
                    ..QueueDeclareOptions::default()
                },
                FieldTable::default(),
            )
            .await
            .expect("declare a temporary reply queue");
        queue.name().as_str().to_owned()
    }

    /// Takes the next message of `queue`, waiting up to `deadline`, then
    /// acknowledges and returns it.
    ///
    /// # Panics
    ///
    /// Panics if no message arrives within `deadline`, or if the get or the
    /// acknowledgement fails. A get from a missing queue closes the test's
    /// channel and fails.
    pub async fn next_message(&self, queue: &str, deadline: Duration) -> Delivery {
        let until = Instant::now() + deadline;
        loop {
            if let Some(delivery) = self.try_get(queue).await {
                delivery
                    .acker
                    .ack(BasicAckOptions::default())
                    .await
                    .unwrap_or_else(|error| {
                        panic!("cannot acknowledge a message of {queue}: {error}")
                    });
                return delivery;
            }
            assert!(
                Instant::now() < until,
                "no message on {queue} within {deadline:?}"
            );
            pause(POLL_INTERVAL).await;
        }
    }

    /// Checks `queue` for the whole of `wait` and returns when it stayed empty.
    ///
    /// # Panics
    ///
    /// Panics with the queue and the message body as soon as a message
    /// appears, or if a get fails.
    pub async fn assert_no_message(&self, queue: &str, wait: Duration) {
        let until = Instant::now() + wait;
        loop {
            if let Some(delivery) = self.try_get(queue).await {
                let body = String::from_utf8_lossy(&delivery.data);
                panic!("unexpected message on {queue}: {body:?}");
            }
            if Instant::now() >= until {
                return;
            }
            pause(POLL_INTERVAL).await;
        }
    }

    /// One unacknowledged `basic.get` from `queue` on the test's channel.
    async fn try_get(&self, queue: &str) -> Option<Delivery> {
        self.channel
            .basic_get(queue, BasicGetOptions { no_ack: false })
            .await
            .unwrap_or_else(|error| panic!("cannot get from {queue}: {error}"))
            .map(|message| message.delivery)
    }

    /// The number of messages ready on `queue`, read without consuming any.
    ///
    /// # Panics
    ///
    /// Panics if the queue does not exist or cannot be inspected.
    pub async fn queue_depth(&self, queue: &str) -> u32 {
        self.passive_declare(queue).await.message_count()
    }

    /// Every non-blank line the consumers' telemetry has written so far, in
    /// write order: JSON log events and `"type":"span"` lines.
    #[must_use]
    pub fn logs(&self) -> Vec<String> {
        let bytes = self
            .buffer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        bytes
            .split(|&byte| byte == b'\n')
            .map(|line| String::from_utf8_lossy(line).into_owned())
            .filter(|line| !line.trim().is_empty())
            .collect()
    }

    /// The first captured line holding every one of `needles`, waiting up to
    /// `deadline` for it to be written.
    ///
    /// # Panics
    ///
    /// Panics, listing the needles and the number of captured lines, if no
    /// such line is written within `deadline`.
    pub async fn wait_for_log(&self, needles: &[&str], deadline: Duration) -> String {
        let until = Instant::now() + deadline;
        loop {
            let lines = self.logs();
            let captured = lines.len();
            if let Some(line) = lines
                .into_iter()
                .find(|line| needles.iter().all(|needle| line.contains(needle)))
            {
                return line;
            }
            assert!(
                Instant::now() < until,
                "no captured line holds all of {needles:?} within {deadline:?}; \
                 {captured} line(s) captured"
            );
            pause(POLL_INTERVAL).await;
        }
    }

    /// Stops the consumers, closes the test's channel and connection, shuts
    /// the capture down and removes the containers, in that order.
    ///
    /// # Panics
    ///
    /// Panics if the consumers' connection or the test's connection cannot be
    /// closed, if the test's channel fails to close for a reason other than
    /// the broker having closed it already, or if the capture's tracer provider
    /// fails to shut down.
    pub async fn shutdown(self) {
        let Self {
            stack,
            db,
            settings,
            channel,
            connection,
            running,
            buffer,
            guard,
        } = self;
        running.shutdown().await.expect("stop the consumers");
        match channel.close(REPLY_SUCCESS, "done").await {
            // A test that provoked a channel error left the channel closed by
            // the broker, so there is nothing left to close.
            Ok(()) | Err(lapin::Error::InvalidChannelState(_)) => {}
            Err(error) => panic!("cannot close the test channel: {error}"),
        }
        connection
            .close(REPLY_SUCCESS, "done")
            .await
            .expect("close the test connection");
        guard.shutdown().expect("shut the capture telemetry down");
        drop(db);
        drop(settings);
        drop(buffer);
        // Last, once nothing uses the containers. `TestStack`'s drop removes
        // them through the current runtime, which it supports from inside
        // `block_on`, where every test calls this.
        drop(stack);
    }
}

/// The line of `docs/parity/parity-oracle-audit.md` that holds the row `id`,
/// for example `P-CODATE01-013`, as stored.
///
/// The row is the first table line whose first cell is `id`: a row's own table
/// precedes the review log that repeats its ID.
///
/// # Panics
///
/// Panics if the audit cannot be read or holds no row `id`.
#[must_use]
pub fn audit_row(id: &str) -> String {
    let lines: Vec<&str> = audit_text().lines().collect();
    lines[row_index(&lines, id)].to_owned()
}

/// The exact texts of the "Expected result" cell of the audit row `id`, in
/// cell order, as the audit's conventions define them.
///
/// An exact text is a code span holding a double-quoted string, with `\"` and
/// `\\` as its escapes, and `\|` standing for `|` as in every table cell. A
/// `width=<n>` after the span right-pads the value with spaces to n bytes.
/// `{UNDEF:n}` stands for n bytes the COBOL never sets: it is returned as
/// written and counts as n bytes towards the width, so a test replaces it with
/// the bytes the replacement writes there.
///
/// # Panics
///
/// Panics, naming the row, if the audit cannot be read, holds no row `id`, its
/// table has no "Expected result" column, the cell holds no code span, a code
/// span is not a well-formed double-quoted string, or a padded value would be
/// longer than its width or holds a `{FIELD}` placeholder of unknown length.
#[must_use]
pub fn audit_texts(id: &str) -> Vec<String> {
    let lines: Vec<&str> = audit_text().lines().collect();
    let index = row_index(&lines, id);
    let column = header_cells(&lines, index, id)
        .iter()
        .position(|cell| cell == EXPECTED_RESULT)
        .unwrap_or_else(|| panic!("the table of audit row {id} has no {EXPECTED_RESULT:?} column"));
    let cells = split_cells(lines[index]);
    let cell = cells
        .get(column)
        .unwrap_or_else(|| panic!("audit row {id} has no {EXPECTED_RESULT:?} cell"));
    let texts = exact_texts(cell, id);
    assert!(
        !texts.is_empty(),
        "the {EXPECTED_RESULT:?} cell of audit row {id} holds no code span"
    );
    texts
}

/// The audit's text, read on first use.
///
/// # Panics
///
/// Panics, naming the path, if the file cannot be read.
fn audit_text() -> &'static str {
    AUDIT_TEXT.get_or_init(|| {
        let path = fixtures::parity_doc(PARITY_AUDIT);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    })
}

/// The index in `lines` of the first table line whose first cell is `id`.
fn row_index(lines: &[&str], id: &str) -> usize {
    lines
        .iter()
        .position(|line| {
            // `contains` only skips lines that cannot hold the ID.
            is_table_line(line)
                && line.contains(id)
                && split_cells(line).first().is_some_and(|cell| cell == id)
        })
        .unwrap_or_else(|| panic!("audit row {id} not found in {PARITY_AUDIT}"))
}

/// The header cells of the table holding the row at `index`: the line directly
/// above the table's delimiter row.
fn header_cells(lines: &[&str], index: usize, id: &str) -> Vec<String> {
    let delimiter = (0..index)
        .rev()
        .take_while(|&at| is_table_line(lines[at]))
        .find(|&at| is_delimiter_row(lines[at]))
        .unwrap_or_else(|| panic!("audit row {id} is not below a table delimiter row"));
    let header = delimiter
        .checked_sub(1)
        .map(|at| lines[at])
        .filter(|line| is_table_line(line))
        .unwrap_or_else(|| panic!("the table of audit row {id} has no header line"));
    split_cells(header)
}

/// Whether `line` belongs to a pipe table.
fn is_table_line(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

/// Whether `line` is a table delimiter row: every cell is a run of `-` with an
/// optional `:` at either end.
fn is_delimiter_row(line: &str) -> bool {
    let cells = split_cells(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.strip_prefix(':').unwrap_or(cell);
            let cell = cell.strip_suffix(':').unwrap_or(cell);
            !cell.is_empty() && cell.chars().all(|ch| ch == '-')
        })
}

/// The trimmed cells of a table line, as GitHub reads them. One leading `|`
/// and the empty cell after a trailing `|` are dropped. A `|` ends a cell only
/// outside code spans, and `\|` stands for a literal `|` everywhere, code spans
/// included.
fn split_cells(line: &str) -> Vec<String> {
    let line = line.trim();
    let body = line.strip_prefix('|').unwrap_or(line);
    let chars: Vec<char> = body.chars().collect();
    let spans = code_spans(&chars);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut ended_on_split = false;
    let mut at = 0;
    while at < chars.len() {
        ended_on_split = false;
        if chars[at] == '\\' && chars.get(at + 1) == Some(&'|') {
            cell.push('|');
            at += 2;
            continue;
        }
        if chars[at] == '|' && !spans.iter().any(|span| span.start <= at && at < span.end) {
            cells.push(std::mem::take(&mut cell));
            ended_on_split = true;
        } else {
            cell.push(chars[at]);
        }
        at += 1;
    }
    if !ended_on_split {
        cells.push(cell);
    }
    cells.iter().map(|cell| cell.trim().to_owned()).collect()
}

/// One code span of a char sequence: `start..end` covers its delimiters,
/// `open..close` its content.
#[derive(Clone, Copy)]
struct CodeSpan {
    start: usize,
    open: usize,
    close: usize,
    end: usize,
}

/// The code spans of `chars` in order, as `CommonMark` delimits them: a run of n
/// backticks opens a span that the next run of exactly n backticks closes, and
/// a run without such a closer is literal text.
fn code_spans(chars: &[char]) -> Vec<CodeSpan> {
    let run_at = |from: usize| chars[from..].iter().take_while(|&&ch| ch == '`').count();
    let mut spans = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '`' {
            at += 1;
            continue;
        }
        let run = run_at(at);
        let mut probe = at + run;
        let mut closer = None;
        while probe < chars.len() {
            if chars[probe] == '`' {
                let length = run_at(probe);
                if length == run {
                    closer = Some(probe);
                    break;
                }
                probe += length;
            } else {
                probe += 1;
            }
        }
        match closer {
            Some(close) => {
                spans.push(CodeSpan {
                    start: at,
                    open: at + run,
                    close,
                    end: close + run,
                });
                at = close + run;
            }
            None => at += run,
        }
    }
    spans
}

/// The exact texts of the code spans of `cell`, each padded to the width that
/// follows it.
fn exact_texts(cell: &str, id: &str) -> Vec<String> {
    let chars: Vec<char> = cell.chars().collect();
    code_spans(&chars)
        .into_iter()
        .map(|span| {
            let content = &chars[span.open..span.close];
            // CommonMark: content that begins and ends with a space, and is not
            // only spaces, loses one space at each end.
            let padded = content.first() == Some(&' ')
                && content.last() == Some(&' ')
                && content.iter().any(|&ch| ch != ' ');
            let content: String = if padded {
                content[1..content.len() - 1].iter().collect()
            } else {
                content.iter().collect()
            };
            let text = unquote(&content, id);
            match width_after(&chars[span.end..], id) {
                Some(width) => pad(text, width, id),
                None => text,
            }
        })
        .collect()
}

/// The value of the double-quoted string `content`, whose only escapes are
/// `\"` and `\\`.
fn unquote(content: &str, id: &str) -> String {
    let inner = content
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| {
            panic!("audit row {id}: the code span {content:?} is not a double-quoted exact text")
        });
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                _ => panic!(
                    "audit row {id}: {content:?} holds a backslash that escapes no quote or backslash"
                ),
            },
            '"' => panic!("audit row {id}: {content:?} holds an unescaped quote"),
            _ => text.push(ch),
        }
    }
    text
}

/// The `n` of a `width=<n>` that directly follows a code span, spaces between
/// them allowed.
fn width_after(rest: &[char], id: &str) -> Option<usize> {
    let rest: String = rest
        .iter()
        .skip_while(|&&ch| ch == ' ' || ch == '\t')
        .collect();
    let digits: String = rest
        .strip_prefix("width=")?
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    if digits.is_empty() {
        return None;
    }
    let width = digits
        .parse()
        .unwrap_or_else(|error| panic!("audit row {id}: width={digits} is not a width: {error}"));
    Some(width)
}

/// `text` right-padded with spaces to `width` bytes, each `{UNDEF:n}` counting
/// as the n bytes it stands for.
fn pad(mut text: String, width: usize, id: &str) -> String {
    let length = value_length(&text, id);
    assert!(
        length <= width,
        "audit row {id}: {text:?} is {length} bytes, more than its width={width}"
    );
    text.push_str(&" ".repeat(width - length));
    text
}

/// The byte length of the value `text` stands for: `{UNDEF:n}` counts n bytes,
/// and any other `{` is a literal character, such as an overpunched `+0`.
///
/// # Panics
///
/// Panics on a `{FIELD}` placeholder, a COBOL data name in braces, because the
/// length of the field's content is not part of the text.
fn value_length(text: &str, id: &str) -> usize {
    let mut length = 0;
    let mut rest = text;
    while let Some(at) = rest.find('{') {
        length += at;
        let tail = &rest[at..];
        if let Some((bytes, written)) = undefined_bytes(tail) {
            length += bytes;
            rest = &tail[written..];
        } else if starts_with_field(tail) {
            panic!("audit row {id}: the width of {text:?} depends on a field's content");
        } else {
            length += 1;
            rest = &tail[1..];
        }
    }
    length + rest.len()
}

/// For a `tail` starting with `{UNDEF:n}`: n, and the placeholder's own length.
fn undefined_bytes(tail: &str) -> Option<(usize, usize)> {
    let rest = tail.strip_prefix("{UNDEF:")?;
    let (digits, _) = rest.split_once('}')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let bytes = digits.parse().ok()?;
    Some((bytes, "{UNDEF:}".len() + digits.len()))
}

/// Whether `tail` starts with a `{FIELD}` placeholder: an upper-case COBOL data
/// name, letters, digits and hyphens, in braces.
fn starts_with_field(tail: &str) -> bool {
    tail.strip_prefix('{')
        .and_then(|rest| rest.split_once('}'))
        .is_some_and(|(name, _)| {
            name.starts_with(|ch: char| ch.is_ascii_uppercase())
                && name
                    .chars()
                    .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-')
        })
}
