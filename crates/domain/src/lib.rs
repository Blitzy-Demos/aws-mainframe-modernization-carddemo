//! `carddemo-domain`: the pure business core of the `CardDemo` port.
//!
//! The crate holds the copybook newtypes ([`numeric`]), the typed records
//! ([`entities`]), the ported COBOL rules, the message catalog ([`messages`]),
//! the edited-picture formatting ([`format`](mod@format)), the processing clock
//! ([`clock`]), the business error ([`error`]) and the password policy
//! ([`auth`]).
//!
//! It performs no I/O, starts no async runtime and serializes nothing: the
//! persistence, HTTP, AMQP and CLI adapters do that around it. Money, rates and
//! points are decimal newtypes, never binary floating point. No money operation
//! rounds: every store truncates as its COBOL receiving field does.

pub mod auth;
pub mod clock;
pub mod entities;
pub mod error;
pub mod format;
pub mod messages;
pub mod numeric;

pub use clock::Clock;
pub use error::{DomainError, ErrorKind};
pub use numeric::{InterestRate, Money, MoneyExpr, RewardPoints, RewardRate};
