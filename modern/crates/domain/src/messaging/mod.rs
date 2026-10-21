#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! The rules of the three MQ-triggered CICS programs that become `RabbitMQ` workers (AAP
//! 0.3.4.2), one module per program:
//!
//! | Module | Program | Transaction |
//! |---|---|---|
//! | [`codate01`] | `CODATE01`, with the `MQ-ERR-DISPLAY` layout `COACCT01` shares | CDRD |
//! | [`coacct01`] | `COACCT01` | CDRA |
//! | [`copaua0c`] | `COPAUA0C` | CP00 |
//!
//! These modules decode requests, decide and format reply, error and log bodies, and nothing
//! else. Consumption, the confirmed publish, acknowledgement and rejection, dead-lettering,
//! database transactions and the summary row lock taken by the worker all live in the
//! `messaging` crate's `broker` and `workers` modules.
//!
//! The modules depend only on `crate::{records, money, clock, sensitive}` and `copybook`: no
//! I/O and no `sqlx` (R3). The lint attribute above denies `unwrap`, `expect` and indexing in
//! this module and every child, unit tests included, so the CDRA and CP00 request decoders stay
//! total over untrusted bytes (Total Parsers Over Untrusted Input).

// Declared in dependency order; the blank lines keep rustfmt from sorting them.
pub mod codate01;

// Reuses `codate01`'s `MQ-ERR-DISPLAY` layout and buffer helpers.
pub mod coacct01;

pub mod copaua0c;
