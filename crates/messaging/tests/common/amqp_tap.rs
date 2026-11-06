//! A pass-through AMQP 0-9-1 proxy between the consumers and the broker that
//! records, in one ordered [`Journal`], how the broker delivered each request,
//! how the consumers settled it and how the broker confirmed each publish.
//!
//! `carddemo_messaging::start` opens one broker connection with one
//! confirm-mode channel per flow. [`Tap::start`] listens on a loopback port and
//! [`Tap::url`] is the broker URL with that port, which the harness gives the
//! consumers, so every byte between them and the broker passes through the
//! tap: forwarded unchanged and in order, one whole frame at a time. The test's
//! own connection goes to the broker directly.
//!
//! On the way the tap reads each frame [AMQP 0-9-1 §4.2.3] and records:
//!
//! - from the broker: `basic.deliver` with the `message_id` and
//!   `correlation_id` of the content header that follows it; `basic.return`,
//!   attributed to the oldest unconfirmed publish of its channel with its
//!   routing key, since `RabbitMQ` sends the return before the confirm of the
//!   same message; and each publisher confirm, `basic.ack` or `basic.nack`,
//!   at the moment it is forwarded;
//! - from the consumers: `channel.open`, `confirm.select`, each
//!   `basic.publish` with its content header's IDs, numbered from 1 per
//!   channel after `confirm.select` as the broker numbers its confirms, and
//!   each settlement, `basic.ack`, `basic.reject` or `basic.nack`, with the
//!   length the harness's telemetry capture had when the frame arrived.
//!
//! A request's [`Handling`] is therefore what the broker saw, not what the
//! consumer reports about itself: which frame settled the delivery, in which
//! order with the confirms of its publishes, and which log lines had been
//! written before that frame left the consumer. A frame the tap cannot read
//! breaks the journal: the bytes still pass, unparsed, and every reading of
//! the journal fails with the reason.
//!
//! A [`ConfirmHold`] stops the broker-to-consumers direction at the next
//! publisher confirm and keeps it, recorded as held, until the hold is
//! released or dropped, or for at most `HOLD_LIMIT`, so a test can watch a
//! request whose reply is routed but not yet confirmed.
//!
//! The tap runs on standard-library threads and blocking sockets, outside the
//! tokio runtime. Every socket read has a short timeout after which the thread
//! checks whether the tap is stopping, the accept waits at most
//! `ACCEPT_TIMEOUT`, and [`Tap::shutdown`] joins the threads within a bound,
//! so a tap whose consumers never connect, or whose test panics, ends instead
//! of hanging.

use std::collections::HashMap;
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::{POLL_INTERVAL, pause};

/// The protocol header a client sends before its first frame: `AMQP`, a zero
/// and version 0-9-1.
const PROTOCOL_HEADER: &[u8; 8] = b"AMQP\x00\x00\x09\x01";

/// Frame type, channel and payload size: the bytes before a frame's payload.
const FRAME_HEADER_LEN: usize = 7;

/// The octet that ends every frame.
const FRAME_END: u8 = 0xCE;

/// The largest payload the tap accepts. The broker's `frame_max` is 128 KiB;
/// a larger size means the stream is not where the tap thinks it is.
const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// Frame types.
const METHOD_FRAME: u8 = 1;
const HEADER_FRAME: u8 = 2;
const BODY_FRAME: u8 = 3;
const HEARTBEAT_FRAME: u8 = 8;

/// The classes and methods the tap reads, as `(class, method)`.
const CHANNEL_OPEN: (u16, u16) = (20, 10);
const BASIC_PUBLISH: (u16, u16) = (60, 40);
const BASIC_RETURN: (u16, u16) = (60, 50);
const BASIC_DELIVER: (u16, u16) = (60, 60);
const BASIC_ACK: (u16, u16) = (60, 80);
const BASIC_REJECT: (u16, u16) = (60, 90);
const BASIC_NACK: (u16, u16) = (60, 120);
const CONFIRM_SELECT: (u16, u16) = (85, 10);

/// The `basic` class, whose content headers carry the message properties.
const BASIC_CLASS: u16 = 60;

/// The property flags of a `basic` content header, highest bit first, up to
/// `message-id`, the last property the tap reads [AMQP 0-9-1 §1.8.2].
const CONTENT_TYPE: u16 = 1 << 15;
const CONTENT_ENCODING: u16 = 1 << 14;
const HEADERS: u16 = 1 << 13;
const DELIVERY_MODE: u16 = 1 << 12;
const PRIORITY: u16 = 1 << 11;
const CORRELATION_ID: u16 = 1 << 10;
const REPLY_TO: u16 = 1 << 9;
const EXPIRATION: u16 = 1 << 8;
const MESSAGE_ID: u16 = 1 << 7;

/// The bit of a property-flags word that announces another word.
const MORE_FLAGS: u16 = 1;

/// Bytes read from a socket at once.
const CHUNK: usize = 64 * 1024;

/// How long a socket read or a held confirm waits before the thread checks
/// whether the tap is stopping.
const STOP_POLL: Duration = Duration::from_millis(100);

/// How long a write to a peer may block before the tap gives the connection
/// up.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the tap waits for the consumers to connect.
const ACCEPT_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the tap waits for its connection to the broker.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The longest a [`ConfirmHold`] keeps a confirm, whatever the test does.
const HOLD_LIMIT: Duration = Duration::from_secs(60);

/// How long [`Tap::shutdown`] lets a connected tap end on its own, from the
/// consumers' closed connection, before it stops the threads.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// How long [`Tap::shutdown`] waits for stopped threads to end.
const STOP_BOUND: Duration = Duration::from_secs(10);

/// How a consumer settled a delivery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Settle {
    /// `basic.ack`.
    Ack {
        /// The `multiple` bit.
        multiple: bool,
    },
    /// `basic.reject`.
    Reject {
        /// The `requeue` bit.
        requeue: bool,
    },
    /// `basic.nack`.
    Nack {
        /// The `multiple` bit.
        multiple: bool,
        /// The `requeue` bit.
        requeue: bool,
    },
}

impl Settle {
    /// Whether the frame settles the delivery tag `tag` when it names
    /// `settled`: the same tag, or with `multiple` every tag up to it, or all
    /// of them for tag 0.
    fn covers(self, settled: u64, tag: u64) -> bool {
        let multiple = matches!(
            self,
            Self::Ack { multiple: true } | Self::Nack { multiple: true, .. }
        );
        settled == tag || (multiple && (settled == 0 || settled >= tag))
    }
}

/// How the broker answered a publish, as lapin's `Confirmation` reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A `basic.ack` and no `basic.return`: `Confirmation::Ack(None)`.
    Confirmed,
    /// A `basic.return`, then the `basic.ack`: `Confirmation::Ack(Some(_))`.
    Returned,
    /// A `basic.nack`: `Confirmation::Nack(_)`.
    Nacked,
    /// No confirm forwarded yet, or a publish outside confirm mode.
    Unconfirmed,
}

/// One recorded frame, or what the tap did with one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The consumers opened `channel`, which restarts its delivery and
    /// publish numbering.
    ChannelOpen {
        /// The channel.
        channel: u16,
    },
    /// The consumers put `channel` in confirm mode.
    ConfirmSelect {
        /// The channel.
        channel: u16,
    },
    /// The broker delivered a message to a consumer.
    Deliver {
        /// The channel.
        channel: u16,
        /// The consumer tag, `carddemo-<flow>`.
        consumer_tag: String,
        /// The delivery tag the settlement names.
        delivery_tag: u64,
        /// The `redelivered` bit.
        redelivered: bool,
        /// The exchange the message was published to.
        exchange: String,
        /// The routing key it was published with.
        routing_key: String,
        /// The `message_id` of its content header.
        message_id: Option<String>,
        /// The `correlation_id` of its content header.
        correlation_id: Option<String>,
    },
    /// A consumer published a message.
    Publish {
        /// The channel.
        channel: u16,
        /// The number the broker's confirm names, when the channel is in
        /// confirm mode.
        seq: Option<u64>,
        /// The exchange.
        exchange: String,
        /// The routing key, the target queue on the default exchange.
        routing_key: String,
        /// The `mandatory` bit.
        mandatory: bool,
        /// The `message_id` of its content header.
        message_id: Option<String>,
        /// The `correlation_id` of its content header.
        correlation_id: Option<String>,
    },
    /// The broker returned a mandatory publish as unroutable.
    Return {
        /// The channel.
        channel: u16,
        /// The reply code, 312 `NO_ROUTE` for a missing queue.
        reply_code: u16,
        /// The reply text.
        reply_text: String,
        /// The routing key of the returned message.
        routing_key: String,
        /// The journal index of the publish it returns.
        publish: Option<usize>,
    },
    /// The tap stopped at this confirm for a [`ConfirmHold`].
    ConfirmHeld {
        /// The channel.
        channel: u16,
        /// The publish number it confirms.
        tag: u64,
        /// `basic.ack` rather than `basic.nack`.
        ack: bool,
    },
    /// A held confirm reached `HOLD_LIMIT` before its hold was released.
    HoldExpired {
        /// The channel.
        channel: u16,
        /// The publish number it confirms.
        tag: u64,
    },
    /// The tap forwarded a publisher confirm to the consumers.
    Confirm {
        /// The channel.
        channel: u16,
        /// The publish number it confirms.
        tag: u64,
        /// The `multiple` bit.
        multiple: bool,
        /// `basic.ack` rather than `basic.nack`.
        ack: bool,
    },
    /// A consumer settled a delivery.
    Settlement {
        /// The channel.
        channel: u16,
        /// The delivery tag it names.
        delivery_tag: u64,
        /// The frame.
        settle: Settle,
        /// The byte length of the harness's telemetry capture when the frame
        /// reached the tap: every log line written before the frame left the
        /// consumer lies within it.
        captured: usize,
    },
}

impl Event {
    /// The channel the event concerns.
    fn channel(&self) -> u16 {
        match self {
            Self::ChannelOpen { channel }
            | Self::ConfirmSelect { channel }
            | Self::Deliver { channel, .. }
            | Self::Publish { channel, .. }
            | Self::Return { channel, .. }
            | Self::ConfirmHeld { channel, .. }
            | Self::HoldExpired { channel, .. }
            | Self::Confirm { channel, .. }
            | Self::Settlement { channel, .. } => *channel,
        }
    }
}

/// A consumer's settlement of a request, as the tap saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// Its journal index.
    pub index: usize,
    /// The frame.
    pub settle: Settle,
    /// The capture length when the frame reached the tap.
    pub captured: usize,
}

/// A publish of a consumer and the broker's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Published {
    /// Its journal index.
    pub index: usize,
    /// The number the broker's confirm names.
    pub seq: Option<u64>,
    /// The exchange.
    pub exchange: String,
    /// The routing key.
    pub routing_key: String,
    /// The `mandatory` bit.
    pub mandatory: bool,
    /// The broker's answer so far.
    pub outcome: Outcome,
    /// The journal index of the forwarded confirm, once there is one.
    pub confirm: Option<usize>,
}

/// What the journal holds for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Handling {
    /// The journal index of its `basic.deliver`.
    pub deliver: usize,
    /// The channel it was delivered on.
    pub channel: u16,
    /// The consumer tag it was delivered to.
    pub consumer_tag: String,
    /// The `redelivered` bit of its delivery.
    pub redelivered: bool,
    /// Every frame that settled it, in order.
    pub settlements: Vec<Settlement>,
    /// The publishes on its channel after its delivery and before its first
    /// settlement: with a prefetch of 1, the publishes it caused.
    pub publishes: Vec<Published>,
}

/// Every event the tap recorded, in the order it recorded them; an event's
/// index is its sequence number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Journal {
    /// The events.
    events: Vec<Event>,
    /// Why the tap stopped reading frames, when it did.
    broken: Option<String>,
}

impl Journal {
    /// The recorded events, in order.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// The journal indexes of the deliveries of the request `message_id`.
    ///
    /// # Errors
    ///
    /// Returns why the journal is broken, when it is.
    pub fn deliveries(&self, message_id: &str) -> Result<Vec<usize>, String> {
        self.readable()?;
        Ok(self
            .events
            .iter()
            .enumerate()
            .filter(|(_, event)| {
                matches!(event, Event::Deliver { message_id: Some(id), .. } if id == message_id)
            })
            .map(|(index, _)| index)
            .collect())
    }

    /// The handling of the request `message_id`, or `None` while it has not
    /// been delivered.
    ///
    /// # Errors
    ///
    /// Returns why the journal is broken, when it is, and that the request was
    /// delivered more than once.
    pub fn handling(&self, message_id: &str) -> Result<Option<Handling>, String> {
        let deliveries = self.deliveries(message_id)?;
        let deliver = match deliveries.as_slice() {
            [] => return Ok(None),
            [deliver] => *deliver,
            _ => {
                return Err(format!(
                    "{message_id} was delivered {} times, at {deliveries:?}",
                    deliveries.len()
                ));
            }
        };
        let Event::Deliver {
            channel,
            consumer_tag,
            delivery_tag,
            redelivered,
            ..
        } = &self.events[deliver]
        else {
            return Err(format!("journal event #{deliver} is not a basic.deliver"));
        };
        let span = self.channel_span(*channel, deliver);
        let settlements: Vec<Settlement> = span
            .clone()
            .filter_map(|index| match self.events[index] {
                Event::Settlement {
                    channel: of,
                    delivery_tag: settled,
                    settle,
                    captured,
                } if of == *channel && settle.covers(settled, *delivery_tag) => Some(Settlement {
                    index,
                    settle,
                    captured,
                }),
                _ => None,
            })
            .collect();
        let end = settlements.first().map_or(span.end, |first| first.index);
        let publishes = (deliver + 1..end)
            .filter(|&index| self.events[index].channel() == *channel)
            .filter_map(|index| self.published(index))
            .collect();
        Ok(Some(Handling {
            deliver,
            channel: *channel,
            consumer_tag: consumer_tag.clone(),
            redelivered: *redelivered,
            settlements,
            publishes,
        }))
    }

    /// Fails with the reason when the journal is broken.
    fn readable(&self) -> Result<(), String> {
        self.broken.as_ref().map_or(Ok(()), |reason| {
            Err(format!("the AMQP tap stopped reading frames: {reason}"))
        })
    }

    /// The indexes after `from` up to the next `channel.open` of `channel`,
    /// which restarts its numbering.
    fn channel_span(&self, channel: u16, from: usize) -> std::ops::Range<usize> {
        let end = (from + 1..self.events.len())
            .find(|&index| self.events[index] == Event::ChannelOpen { channel })
            .unwrap_or(self.events.len());
        from + 1..end
    }

    /// The publish at `index` with the broker's answer, or `None` when the
    /// event there is no publish.
    fn published(&self, index: usize) -> Option<Published> {
        let Event::Publish {
            channel,
            seq,
            exchange,
            routing_key,
            mandatory,
            ..
        } = &self.events[index]
        else {
            return None;
        };
        let confirm = seq.and_then(|seq| self.confirm_of(*channel, seq, index));
        let outcome = match confirm.map(|(_, ack)| ack) {
            None => Outcome::Unconfirmed,
            Some(false) => Outcome::Nacked,
            Some(true) if self.is_returned(index) => Outcome::Returned,
            Some(true) => Outcome::Confirmed,
        };
        Some(Published {
            index,
            seq: *seq,
            exchange: exchange.clone(),
            routing_key: routing_key.clone(),
            mandatory: *mandatory,
            outcome,
            confirm: confirm.map(|(at, _)| at),
        })
    }

    /// The first forwarded confirm of the publish `seq` on `channel`, after
    /// its journal index `from`, with whether it is an ack.
    fn confirm_of(&self, channel: u16, seq: u64, from: usize) -> Option<(usize, bool)> {
        self.channel_span(channel, from)
            .find_map(|index| match self.events[index] {
                Event::Confirm {
                    channel: of,
                    tag,
                    multiple,
                    ack,
                } if of == channel && (tag == seq || (multiple && tag >= seq)) => {
                    Some((index, ack))
                }
                _ => None,
            })
    }

    /// The oldest publish on `channel` with `routing_key` that has neither a
    /// confirm nor a return yet: the message a `basic.return` gives back.
    fn returned_publish(&self, channel: u16, routing_key: &str) -> Option<usize> {
        (0..self.events.len()).find(|&index| {
            let Event::Publish {
                channel: of,
                seq,
                routing_key: key,
                ..
            } = &self.events[index]
            else {
                return false;
            };
            let confirmed = seq.is_some_and(|seq| self.confirm_of(channel, seq, index).is_some());
            *of == channel && key == routing_key && !confirmed && !self.is_returned(index)
        })
    }

    /// Whether a `basic.return` gave back the publish at `index`.
    fn is_returned(&self, index: usize) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, Event::Return { publish: Some(of), .. } if *of == index))
    }
}

impl fmt::Display for Journal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "AMQP tap journal ({} events):", self.events.len())?;
        for (index, event) in self.events.iter().enumerate() {
            writeln!(f, "  #{index} {event:?}")?;
        }
        if let Some(reason) = &self.broken {
            writeln!(f, "  broken: {reason}")?;
        }
        Ok(())
    }
}

/// The tap's state behind one lock: the journal and the confirm gate.
#[derive(Default)]
struct State {
    /// What the tap recorded.
    journal: Journal,
    /// The number of the [`ConfirmHold`] that waits for the next confirm, or
    /// holds it.
    hold: Option<u64>,
    /// The number of holds armed so far.
    holds: u64,
}

/// What the tap's threads and its owner share.
struct Shared {
    /// The journal and the gate.
    state: Mutex<State>,
    /// Signalled when a hold is released and when the tap stops.
    released: Condvar,
    /// Set when the tap stops: every thread ends at its next check.
    stopping: AtomicBool,
    /// Set once the consumers are connected through to the broker.
    connected: AtomicBool,
    /// The harness's telemetry capture, whose length each settlement records.
    capture: Arc<Mutex<Vec<u8>>>,
}

impl Shared {
    /// The state, whatever thread panicked while holding it.
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The capture's current length.
    fn captured(&self) -> usize {
        self.capture
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    /// Whether the tap is stopping.
    fn stopped(&self) -> bool {
        self.stopping.load(Ordering::SeqCst)
    }

    /// Stops every thread and releases a held confirm.
    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        // Taken so the store cannot fall between a held confirm's check and
        // its wait.
        drop(self.lock());
        self.released.notify_all();
    }

    /// Breaks the journal with `reason`, keeping the first reason given.
    fn fail(&self, reason: String) {
        self.lock().journal.broken.get_or_insert(reason);
    }

    /// Records what a frame showed. A publisher confirm is returned instead,
    /// for [`Shared::forward_confirm`] to record as it is forwarded.
    fn record(&self, seen: Seen) -> Option<Confirm> {
        match seen {
            Seen::Nothing => None,
            Seen::Event(event) => {
                self.lock().journal.events.push(event);
                None
            }
            Seen::Return {
                channel,
                reply_code,
                reply_text,
                routing_key,
            } => {
                let mut state = self.lock();
                let publish = state.journal.returned_publish(channel, &routing_key);
                state.journal.events.push(Event::Return {
                    channel,
                    reply_code,
                    reply_text,
                    routing_key,
                    publish,
                });
                None
            }
            Seen::Confirm(confirm) => Some(confirm),
        }
    }

    /// Records `confirm` as forwarded, first holding it while a
    /// [`ConfirmHold`] is armed: recorded as held, kept until the hold is
    /// released, the tap stops or `HOLD_LIMIT` passes, which is recorded too.
    fn forward_confirm(&self, confirm: Confirm) {
        let Confirm {
            channel,
            tag,
            multiple,
            ack,
        } = confirm;
        let mut state = self.lock();
        if let Some(hold) = state.hold {
            state
                .journal
                .events
                .push(Event::ConfirmHeld { channel, tag, ack });
            let limit = Instant::now() + HOLD_LIMIT;
            while state.hold == Some(hold) && !self.stopped() {
                let left = limit.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    state
                        .journal
                        .events
                        .push(Event::HoldExpired { channel, tag });
                    break;
                }
                state = self
                    .released
                    .wait_timeout(state, left.min(STOP_POLL))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            // One hold, one confirm: the next one passes, whatever became of
            // this hold.
            if state.hold == Some(hold) {
                state.hold = None;
            }
        }
        state.journal.events.push(Event::Confirm {
            channel,
            tag,
            multiple,
            ack,
        });
    }
}

/// A publisher confirm on its way to the consumers.
#[derive(Clone, Copy, Debug)]
struct Confirm {
    /// The channel.
    channel: u16,
    /// The publish number it names.
    tag: u64,
    /// The `multiple` bit.
    multiple: bool,
    /// `basic.ack` rather than `basic.nack`.
    ack: bool,
}

/// What one frame showed the tap.
enum Seen {
    /// Nothing the journal keeps.
    Nothing,
    /// An event to record now.
    Event(Event),
    /// A `basic.return`, attributed when it is recorded.
    Return {
        /// The channel.
        channel: u16,
        /// The reply code.
        reply_code: u16,
        /// The reply text.
        reply_text: String,
        /// The routing key.
        routing_key: String,
    },
    /// A publisher confirm, recorded as it is forwarded.
    Confirm(Confirm),
}

/// Which end sent the bytes a [`Reader`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sender {
    /// The consumers' connection.
    Consumers,
    /// The broker.
    Broker,
}

/// A method frame whose content header has not arrived yet.
enum Content {
    /// A `basic.deliver`, recorded with the header's IDs.
    Deliver {
        /// The consumer tag.
        consumer_tag: String,
        /// The delivery tag.
        delivery_tag: u64,
        /// The `redelivered` bit.
        redelivered: bool,
        /// The exchange.
        exchange: String,
        /// The routing key.
        routing_key: String,
    },
    /// A `basic.publish`, recorded with the header's IDs.
    Publish {
        /// The confirm number.
        seq: Option<u64>,
        /// The exchange.
        exchange: String,
        /// The routing key.
        routing_key: String,
        /// The `mandatory` bit.
        mandatory: bool,
    },
}

/// The next piece of a byte stream to forward.
enum Next {
    /// More bytes are needed.
    Incomplete,
    /// The client's protocol header.
    Header(Vec<u8>),
    /// One whole frame.
    Frame(Vec<u8>),
    /// Bytes forwarded unread, after the stream broke.
    Raw(Vec<u8>),
    /// Bytes that broke the stream, with the reason.
    Broken(Vec<u8>, String),
}

/// One direction's frame reader: it cuts the stream into frames and keeps
/// what it needs between them.
struct Reader {
    /// Who sends the stream.
    sender: Sender,
    /// Bytes read and not yet forwarded.
    pending: Vec<u8>,
    /// The protocol header is still due (the consumers' direction only).
    header_due: bool,
    /// A frame could not be read: every later byte is forwarded unread.
    broken: bool,
    /// Per channel, the method waiting for its content header.
    content: HashMap<u16, Content>,
    /// Per channel in confirm mode, the number of the last publish.
    confirm_seq: HashMap<u16, u64>,
}

impl Reader {
    /// A reader of the stream `sender` writes.
    fn new(sender: Sender) -> Self {
        Self {
            sender,
            pending: Vec::new(),
            header_due: sender == Sender::Consumers,
            broken: false,
            content: HashMap::new(),
            confirm_seq: HashMap::new(),
        }
    }

    /// The next piece of the stream read so far.
    fn next_piece(&mut self) -> Next {
        if self.broken {
            return if self.pending.is_empty() {
                Next::Incomplete
            } else {
                Next::Raw(std::mem::take(&mut self.pending))
            };
        }
        if self.header_due {
            if self.pending.len() < PROTOCOL_HEADER.len() {
                return Next::Incomplete;
            }
            self.header_due = false;
            let header: Vec<u8> = self.pending.drain(..PROTOCOL_HEADER.len()).collect();
            if header != PROTOCOL_HEADER {
                return self.break_with(header, "the client sent no AMQP 0-9-1 protocol header");
            }
            return Next::Header(header);
        }
        if self.pending.len() < FRAME_HEADER_LEN {
            return Next::Incomplete;
        }
        let size = u32::from_be_bytes([
            self.pending[3],
            self.pending[4],
            self.pending[5],
            self.pending[6],
        ]);
        let Some(size) = usize::try_from(size)
            .ok()
            .filter(|&size| size <= MAX_PAYLOAD)
        else {
            let bytes = std::mem::take(&mut self.pending);
            return self.break_with(bytes, &format!("a frame payload of {size} bytes"));
        };
        let total = FRAME_HEADER_LEN + size + 1;
        if self.pending.len() < total {
            return Next::Incomplete;
        }
        let frame: Vec<u8> = self.pending.drain(..total).collect();
        if frame[total - 1] != FRAME_END {
            return self.break_with(frame, "a frame without its frame-end octet");
        }
        Next::Frame(frame)
    }

    /// Marks the stream broken by `bytes` for `reason`.
    fn break_with(&mut self, bytes: Vec<u8>, reason: &str) -> Next {
        self.broken = true;
        Next::Broken(bytes, format!("{:?} stream: {reason}", self.sender))
    }

    /// What the whole frame `frame` shows, given what came before it on its
    /// channel. `captured` is the capture length when its bytes arrived.
    fn observe(&mut self, frame: &[u8], captured: usize) -> Result<Seen, String> {
        let channel = u16::from_be_bytes([frame[1], frame[2]]);
        let payload = &frame[FRAME_HEADER_LEN..frame.len() - 1];
        match frame[0] {
            METHOD_FRAME => self.method(channel, payload, captured),
            HEADER_FRAME => self.content_header(channel, payload),
            BODY_FRAME | HEARTBEAT_FRAME => Ok(Seen::Nothing),
            other => Err(format!(
                "{:?} stream: frame type {other} on channel {channel}",
                self.sender
            )),
        }
    }

    /// What the method frame `payload` of `channel` shows.
    fn method(&mut self, channel: u16, payload: &[u8], captured: usize) -> Result<Seen, String> {
        let mut args = Args::new(payload);
        let method = (args.u16()?, args.u16()?);
        let seen = match (self.sender, method) {
            (Sender::Consumers, CHANNEL_OPEN) => {
                self.content.remove(&channel);
                self.confirm_seq.remove(&channel);
                Seen::Event(Event::ChannelOpen { channel })
            }
            (Sender::Consumers, CONFIRM_SELECT) => {
                self.confirm_seq.insert(channel, 0);
                Seen::Event(Event::ConfirmSelect { channel })
            }
            (Sender::Consumers, BASIC_PUBLISH) => {
                args.u16()?;
                let exchange = args.short_string()?;
                let routing_key = args.short_string()?;
                let bits = args.u8()?;
                let seq = self.confirm_seq.get_mut(&channel).map(|seq| {
                    *seq += 1;
                    *seq
                });
                self.content.insert(
                    channel,
                    Content::Publish {
                        seq,
                        exchange,
                        routing_key,
                        mandatory: bits & 1 != 0,
                    },
                );
                Seen::Nothing
            }
            (Sender::Consumers, BASIC_ACK | BASIC_REJECT | BASIC_NACK) => {
                let delivery_tag = args.u64()?;
                let bits = args.u8()?;
                let settle = match method {
                    BASIC_ACK => Settle::Ack {
                        multiple: bits & 1 != 0,
                    },
                    BASIC_REJECT => Settle::Reject {
                        requeue: bits & 1 != 0,
                    },
                    _ => Settle::Nack {
                        multiple: bits & 1 != 0,
                        requeue: bits & 2 != 0,
                    },
                };
                Seen::Event(Event::Settlement {
                    channel,
                    delivery_tag,
                    settle,
                    captured,
                })
            }
            (Sender::Broker, BASIC_DELIVER) => {
                let consumer_tag = args.short_string()?;
                let delivery_tag = args.u64()?;
                let redelivered = args.u8()? & 1 != 0;
                let exchange = args.short_string()?;
                let routing_key = args.short_string()?;
                self.content.insert(
                    channel,
                    Content::Deliver {
                        consumer_tag,
                        delivery_tag,
                        redelivered,
                        exchange,
                        routing_key,
                    },
                );
                Seen::Nothing
            }
            (Sender::Broker, BASIC_RETURN) => {
                let reply_code = args.u16()?;
                let reply_text = args.short_string()?;
                args.short_string()?;
                let routing_key = args.short_string()?;
                self.content.remove(&channel);
                Seen::Return {
                    channel,
                    reply_code,
                    reply_text,
                    routing_key,
                }
            }
            (Sender::Broker, BASIC_ACK | BASIC_NACK) => {
                let tag = args.u64()?;
                let bits = args.u8()?;
                Seen::Confirm(Confirm {
                    channel,
                    tag,
                    multiple: bits & 1 != 0,
                    ack: method == BASIC_ACK,
                })
            }
            _ => Seen::Nothing,
        };
        Ok(seen)
    }

    /// What the content header `payload` of `channel` shows: the deliver or
    /// publish it completes, with its `message_id` and `correlation_id`.
    fn content_header(&mut self, channel: u16, payload: &[u8]) -> Result<Seen, String> {
        let Some(content) = self.content.remove(&channel) else {
            return Ok(Seen::Nothing);
        };
        let mut args = Args::new(payload);
        let class = args.u16()?;
        if class != BASIC_CLASS {
            return Err(format!(
                "{:?} stream: a content header of class {class} on channel {channel}",
                self.sender
            ));
        }
        args.u16()?;
        args.u64()?;
        let flags = args.u16()?;
        let mut more = flags & MORE_FLAGS != 0;
        while more {
            more = args.u16()? & MORE_FLAGS != 0;
        }
        let (message_id, correlation_id) = ids(&mut args, flags)?;
        let event = match content {
            Content::Deliver {
                consumer_tag,
                delivery_tag,
                redelivered,
                exchange,
                routing_key,
            } => Event::Deliver {
                channel,
                consumer_tag,
                delivery_tag,
                redelivered,
                exchange,
                routing_key,
                message_id,
                correlation_id,
            },
            Content::Publish {
                seq,
                exchange,
                routing_key,
                mandatory,
            } => Event::Publish {
                channel,
                seq,
                exchange,
                routing_key,
                mandatory,
                message_id,
                correlation_id,
            },
        };
        Ok(Seen::Event(event))
    }
}

/// The `message_id` and `correlation_id` of a `basic` content header whose
/// property list `args` starts, as `flags` announces its properties.
fn ids(args: &mut Args<'_>, flags: u16) -> Result<(Option<String>, Option<String>), String> {
    let mut correlation_id = None;
    let mut message_id = None;
    for flag in [
        CONTENT_TYPE,
        CONTENT_ENCODING,
        HEADERS,
        DELIVERY_MODE,
        PRIORITY,
        CORRELATION_ID,
        REPLY_TO,
        EXPIRATION,
        MESSAGE_ID,
    ] {
        if flags & flag == 0 {
            continue;
        }
        match flag {
            HEADERS => {
                let length = args.u32()?;
                args.skip(usize::try_from(length).map_err(|error| error.to_string())?)?;
            }
            DELIVERY_MODE | PRIORITY => {
                args.u8()?;
            }
            CORRELATION_ID => correlation_id = Some(args.short_string()?),
            MESSAGE_ID => message_id = Some(args.short_string()?),
            _ => {
                args.short_string()?;
            }
        }
    }
    Ok((message_id, correlation_id))
}

/// A cursor over a frame payload's arguments, which fail on a short payload.
struct Args<'p> {
    /// The payload.
    bytes: &'p [u8],
    /// The next byte to read.
    at: usize,
}

impl<'p> Args<'p> {
    /// A cursor at the start of `bytes`.
    fn new(bytes: &'p [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    /// The next `length` bytes.
    fn take(&mut self, length: usize) -> Result<&'p [u8], String> {
        let end = self
            .at
            .checked_add(length)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| {
                format!(
                    "a payload of {} bytes ends before byte {} + {length}",
                    self.bytes.len(),
                    self.at
                )
            })?;
        let taken = &self.bytes[self.at..end];
        self.at = end;
        Ok(taken)
    }

    /// Skips `length` bytes.
    fn skip(&mut self, length: usize) -> Result<(), String> {
        self.take(length).map(drop)
    }

    /// An octet.
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    /// A big-endian short.
    fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// A big-endian long.
    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.take(4)?;
        Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A big-endian long long.
    fn u64(&mut self) -> Result<u64, String> {
        let bytes = self.take(8)?;
        let mut word = [0; 8];
        word.copy_from_slice(bytes);
        Ok(u64::from_be_bytes(word))
    }

    /// A short string: a length octet, then that many bytes of text.
    fn short_string(&mut self) -> Result<String, String> {
        let length = self.u8()?;
        let bytes = self.take(usize::from(length))?;
        Ok(String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Forwards what `source` sends to `sink` frame by frame, recording each
/// frame, until `source` ends, a socket fails or the tap stops; then ends the
/// write half of `sink`, so its peer sees the end too.
fn pump(shared: &Shared, sender: Sender, mut source: &TcpStream, mut sink: &TcpStream) {
    let mut reader = Reader::new(sender);
    let mut chunk = vec![0; CHUNK];
    loop {
        match source.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                // Sampled as soon as the bytes arrive, so a log line written
                // after the frame left the consumer is not counted.
                let captured = shared.captured();
                reader.pending.extend_from_slice(&chunk[..read]);
                if forward(shared, &mut reader, sink, captured).is_err() {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                if shared.stopped() {
                    break;
                }
            }
            Err(error) => {
                if !shared.stopped() {
                    shared.fail(format!("{sender:?} stream: read failed: {error}"));
                }
                break;
            }
        }
    }
    // Ignored: the peer may have closed already, and either way this
    // direction is over.
    let _ = sink.write_all(&reader.pending);
    let _ = sink.shutdown(Shutdown::Write);
}

/// Forwards every whole frame `reader` holds to `sink`, recording each one
/// before it goes; a publisher confirm is recorded, and held, as it goes.
fn forward(
    shared: &Shared,
    reader: &mut Reader,
    mut sink: &TcpStream,
    captured: usize,
) -> io::Result<()> {
    loop {
        match reader.next_piece() {
            Next::Incomplete => return Ok(()),
            Next::Header(bytes) | Next::Raw(bytes) => sink.write_all(&bytes)?,
            Next::Broken(bytes, reason) => {
                shared.fail(reason);
                sink.write_all(&bytes)?;
            }
            Next::Frame(frame) => {
                match reader.observe(&frame, captured) {
                    Ok(seen) => {
                        if let Some(confirm) = shared.record(seen) {
                            shared.forward_confirm(confirm);
                        }
                    }
                    Err(reason) => {
                        reader.broken = true;
                        shared.fail(reason);
                    }
                }
                sink.write_all(&frame)?;
            }
        }
    }
}

/// Accepts the consumers' connection, connects it to `broker_addr` and pumps
/// both directions until both end. Any failure breaks the journal.
fn serve(shared: &Arc<Shared>, listener: &TcpListener, broker_addr: SocketAddr) {
    let Some(consumers) = accept(shared, listener) else {
        return;
    };
    let broker = match TcpStream::connect_timeout(&broker_addr, CONNECT_TIMEOUT) {
        Ok(stream) => stream,
        Err(error) => {
            shared.fail(format!("cannot connect to the broker: {error}"));
            return;
        }
    };
    let (consumers_out, broker_out) = match writers(&consumers, &broker) {
        Ok(streams) => streams,
        Err(error) => {
            shared.fail(format!("cannot set the tap's sockets up: {error}"));
            return;
        }
    };
    shared.connected.store(true, Ordering::SeqCst);
    let upstream = {
        let shared = Arc::clone(shared);
        thread::spawn(move || pump(&shared, Sender::Consumers, &consumers, &broker_out))
    };
    pump(shared, Sender::Broker, &broker, &consumers_out);
    if upstream.join().is_err() {
        shared.fail("the consumers' direction panicked".to_owned());
    }
}

/// Sets both sockets up for the pumps, blocking with `STOP_POLL` reads and
/// `WRITE_TIMEOUT` writes and without Nagle's delay, and returns a second
/// handle on each: the one the other direction writes to.
fn writers(consumers: &TcpStream, broker: &TcpStream) -> io::Result<(TcpStream, TcpStream)> {
    for stream in [consumers, broker] {
        stream.set_nonblocking(false)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(STOP_POLL))?;
        stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    }
    Ok((consumers.try_clone()?, broker.try_clone()?))
}

/// The consumers' connection, polled for until `ACCEPT_TIMEOUT` or the tap
/// stops.
fn accept(shared: &Shared, listener: &TcpListener) -> Option<TcpStream> {
    let until = Instant::now() + ACCEPT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if shared.stopped() {
                    return None;
                }
                if Instant::now() >= until {
                    shared.fail(format!(
                        "the consumers did not connect within {ACCEPT_TIMEOUT:?}"
                    ));
                    return None;
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(error) => {
                shared.fail(format!("accept failed: {error}"));
                return None;
            }
        }
    }
}

/// A running tap between the consumers and the broker.
pub struct Tap {
    /// The broker URL with the tap's address, for the consumers.
    url: String,
    /// The state the threads share.
    shared: Arc<Shared>,
    /// The thread that accepts the connection and pumps it.
    worker: Option<JoinHandle<()>>,
}

impl Tap {
    /// Starts a tap in front of the broker of `broker_url`, an
    /// `amqp://user:password@host:port/vhost` URL, whose settlements record
    /// the length of `capture`.
    ///
    /// # Panics
    ///
    /// Panics if the URL has no `host:port` that resolves, or if the loopback
    /// listener cannot be opened. The URL itself is never shown, since it
    /// holds the password.
    #[must_use]
    pub fn start(broker_url: &str, capture: Arc<Mutex<Vec<u8>>>) -> Self {
        let (scheme, rest) = broker_url
            .split_once("://")
            .expect("the broker URL has a scheme");
        let (credentials, location) = rest
            .rsplit_once('@')
            .expect("the broker URL has credentials");
        let (authority, vhost) = location.split_once('/').unwrap_or((location, ""));
        let broker = authority
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next())
            .unwrap_or_else(|| panic!("the broker URL's host:port {authority} does not resolve"));
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .unwrap_or_else(|error| panic!("cannot open the AMQP tap's listener: {error}"));
        listener
            .set_nonblocking(true)
            .unwrap_or_else(|error| panic!("cannot poll the AMQP tap's listener: {error}"));
        let port = listener
            .local_addr()
            .unwrap_or_else(|error| panic!("the AMQP tap's listener has no address: {error}"))
            .port();
        let shared = Arc::new(Shared {
            state: Mutex::new(State::default()),
            released: Condvar::new(),
            stopping: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            capture,
        });
        let worker = {
            let shared = Arc::clone(&shared);
            thread::spawn(move || serve(&shared, &listener, broker))
        };
        Self {
            url: format!("{scheme}://{credentials}@127.0.0.1:{port}/{vhost}"),
            shared,
            worker: Some(worker),
        }
    }

    /// The broker URL with the tap's address: the consumers' `AMQP_URL`.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A copy of the journal so far.
    #[must_use]
    pub fn journal(&self) -> Journal {
        self.shared.lock().journal.clone()
    }

    /// Arms a hold of the next publisher confirm the broker sends the
    /// consumers. See [`ConfirmHold`].
    ///
    /// # Panics
    ///
    /// Panics if another hold is armed.
    #[must_use]
    pub fn hold_next_confirm(&self) -> ConfirmHold<'_> {
        let mut state = self.shared.lock();
        assert!(state.hold.is_none(), "a confirm hold is armed already");
        state.holds += 1;
        let number = state.holds;
        state.hold = Some(number);
        let armed_at = state.journal.events.len();
        drop(state);
        ConfirmHold {
            tap: self,
            number,
            armed_at,
        }
    }

    /// Ends the tap once the consumers have closed their connection: lets
    /// the connection end on its own for `SHUTDOWN_GRACE`, then stops the
    /// threads and joins them.
    ///
    /// # Panics
    ///
    /// Panics if the threads do not end within `STOP_BOUND` of being stopped,
    /// or if the tap's thread panicked.
    pub async fn shutdown(mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        if self.shared.connected.load(Ordering::SeqCst) {
            let grace = Instant::now() + SHUTDOWN_GRACE;
            while !worker.is_finished() && Instant::now() < grace {
                pause(POLL_INTERVAL).await;
            }
        }
        self.shared.stop();
        let bound = Instant::now() + STOP_BOUND;
        while !worker.is_finished() {
            assert!(
                Instant::now() < bound,
                "the AMQP tap's threads did not end within {STOP_BOUND:?} of being stopped"
            );
            pause(POLL_INTERVAL).await;
        }
        worker
            .join()
            .unwrap_or_else(|_| panic!("the AMQP tap's thread panicked"));
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // A tap dropped without `shutdown`, as when its test panics, stops at
        // once; its threads end at their next check.
        self.shared.stop();
    }
}

/// A publisher confirm the tap held, as the journal records it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeldConfirm {
    /// The journal index of the `ConfirmHeld` event.
    pub index: usize,
    /// The channel.
    pub channel: u16,
    /// The publish number it confirms.
    pub tag: u64,
    /// `basic.ack` rather than `basic.nack`.
    pub ack: bool,
}

/// An armed hold of the next publisher confirm: the tap records the confirm
/// as held and forwards nothing more to the consumers until the hold is
/// released or dropped, or `HOLD_LIMIT` passes. Dropping it releases it, so a
/// panicking test cannot leave the consumers waiting.
pub struct ConfirmHold<'t> {
    /// The tap that holds.
    tap: &'t Tap,
    /// The hold's number in the tap's gate.
    number: u64,
    /// The journal length when the hold was armed.
    armed_at: usize,
}

impl ConfirmHold<'_> {
    /// Waits up to `deadline` until the tap holds a confirm, and returns it.
    ///
    /// # Panics
    ///
    /// Panics, with the journal, if the journal is broken or no confirm is
    /// held within `deadline`.
    pub async fn wait_held(&self, deadline: Duration) -> HeldConfirm {
        let until = Instant::now() + deadline;
        loop {
            let journal = self.tap.journal();
            if let Err(reason) = journal.readable() {
                panic!("{reason}\n{journal}");
            }
            let held =
                journal.events[self.armed_at..]
                    .iter()
                    .enumerate()
                    .find_map(|(offset, event)| match *event {
                        Event::ConfirmHeld { channel, tag, ack } => Some(HeldConfirm {
                            index: self.armed_at + offset,
                            channel,
                            tag,
                            ack,
                        }),
                        _ => None,
                    });
            if let Some(held) = held {
                return held;
            }
            assert!(
                Instant::now() < until,
                "no publisher confirm held within {deadline:?}\n{journal}"
            );
            pause(POLL_INTERVAL).await;
        }
    }

    /// Releases the held confirm, or disarms the hold if none was held yet.
    ///
    /// # Panics
    ///
    /// Panics, with the journal, if a held confirm reached `HOLD_LIMIT` and
    /// went to the consumers before this release.
    pub fn release(self) {
        let journal = self.tap.journal();
        let expired = journal.events[self.armed_at..]
            .iter()
            .any(|event| matches!(event, Event::HoldExpired { .. }));
        assert!(
            !expired,
            "the held confirm went out after {HOLD_LIMIT:?}, before its release\n{journal}"
        );
    }
}

impl Drop for ConfirmHold<'_> {
    fn drop(&mut self) {
        let mut state = self.tap.shared.lock();
        if state.hold == Some(self.number) {
            state.hold = None;
        }
        drop(state);
        self.tap.shared.released.notify_all();
    }
}
