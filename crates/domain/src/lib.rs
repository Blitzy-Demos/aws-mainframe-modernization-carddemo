//! `carddemo-domain`: the pure business core of the `CardDemo` port.
//!
//! The crate holds every business decision of the replacement:
//!
//! - [`numeric`]: the copybook newtypes (money, rates, points, identifiers and
//!   fixed-width text) and the arithmetic the COBOL performs on them;
//! - [`entities`]: one typed record per record copybook, plus the reward rows;
//! - [`rules`]: one pure module per carried COBOL program;
//! - [`messages`]: the message catalog, every audited COBOL text held once and
//!   byte-identical, plus the texts the request writes;
//! - [`format`](mod@format): the edited pictures and fixed-width output lines;
//! - [`clock`]: the processing clock every time-dependent output reads;
//! - [`error`]: [`DomainError`], the business error every rule returns;
//! - [`auth`]: the password policy of the user security file;
//! - [`rewards`]: [`RewardEngine`], the single reward accrual path, and the
//!   admin rule validation;
//! - [`ports`]: the repository traits the persistence adapters implement.
//!
//! # Layering
//!
//! Ports and adapters. The crate performs no I/O, starts no async runtime and
//! serializes nothing. Its manifest names no database, runtime, HTTP, AMQP,
//! serialization or PDF crate and no other `carddemo-*` crate, and
//! `tests/manifest_boundary.rs` fails if one appears. The adapters sit around
//! it:
//!
//! - `carddemo-persistence` implements [`ports`] and [`rewards::RewardStore`]
//!   over a database transaction;
//! - `carddemo-api` (REST), `carddemo-batch` (CLI subcommands) and
//!   `carddemo-messaging` (queue consumers) open that transaction, call the
//!   rules with the port implementations bound to it, and commit.
//!
//! A rule takes its ports as generic parameters, never as trait objects: the
//! port methods return `impl Future + Send`, so the traits are not
//! object-safe. A rule commits nothing and never reads the system clock; time
//! comes from an injected [`Clock`], or from a value its caller derived from
//! one.
//!
//! # Ported code and its audits
//!
//! Every ported function names its COBOL program, the paragraph it ports and
//! the audit row that gates it, for example
//! `/// CBTRN02C 1500-B-LOOKUP-ACCT — P-CBTRN02C-004`, so a reviewer can trace
//! code to audit to COBOL. A function that returns a validation message adds
//! that message's `V-<PROGRAM>-<nnn>` row.
//!
//! Two audits gate the ported code, and only rows that passed their review are
//! implemented:
//!
//! - `docs/parity/parity-oracle-audit.md` (rows `P-<KEY>-<nnn>`): record
//!   layouts, per-program rules, arithmetic, fixture expectations, anomalies
//!   and time-dependent fields;
//! - `docs/parity/validation-audit.md` (rows `V-<PROGRAM>-<nnn>`): each
//!   validation's trigger, message text, status, BMS field and order.
//!
//! Defect-like COBOL behaviour is reproduced, not repaired. A case that
//! `docs/parity/escalations.md` holds at status `halt` gets no code that
//! guesses its result.
//!
//! # Money
//!
//! Money, rates and points are decimal newtypes ([`Money`], [`MoneyExpr`],
//! [`InterestRate`], [`RewardRate`], [`RewardPoints`]). No financial or reward
//! value is ever a binary floating-point number, and no newtype converts from
//! one. No money operation rounds: no carried program uses `ROUNDED`, so every
//! store truncates as its COBOL receiving field does. The only rounding in the
//! crate is the ceiling the reward rule prescribes for whole points.

pub mod auth;
pub mod clock;
pub mod entities;
pub mod error;
pub mod format;
pub mod messages;
pub mod numeric;
pub mod ports;
pub mod rewards;
pub mod rules;

pub use clock::Clock;
pub use error::{DomainError, ErrorKind};
pub use numeric::{InterestRate, Money, MoneyExpr, RewardPoints, RewardRate};
pub use rewards::RewardEngine;
