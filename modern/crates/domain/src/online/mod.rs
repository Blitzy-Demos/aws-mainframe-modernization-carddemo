#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
//! The rules of the CICS online programs behind the 27 REST endpoints (AAP 0.3.4.1), one module
//! per program.
//!
//! | Module | Program | Transaction | Endpoints | Audit in `modern/audit/online/` |
//! |---|---|---|---|---|
//! | [`cosgn00c`] | `COSGN00C` | CC00 | 1 | `COSGN00C.md` |
//! | [`comen01c`] | `COMEN01C` | CM00 | 2 | `COMEN01C.md` |
//! | [`coadm01c`] | `COADM01C` | CA00 | 3 | `COADM01C.md` |
//! | [`coactvwc`] | `COACTVWC` | CAVW | 4 | `COACTVWC.md` |
//! | [`coactupc`] | `COACTUPC` | CAUP | 5–6 | `COACTUPC.md` |
//! | [`cocrdlic`] | `COCRDLIC` | CCLI | 7 | `COCRDLIC.md` |
//! | [`cocrdslc`] | `COCRDSLC` | CCDL | 8 | `COCRDSLC.md` |
//! | [`cocrdupc`] | `COCRDUPC` | CCUP | 9–10 | `COCRDUPC.md` |
//! | [`cotrn00c`] | `COTRN00C` | CT00 | 11 | `COTRN00C.md` |
//! | [`cotrn01c`] | `COTRN01C` | CT01 | 12 | `COTRN01C.md` |
//! | [`cotrn02c`] | `COTRN02C` | CT02 | 13 | `COTRN02C.md` |
//! | [`corpt00c`] | `CORPT00C` | CR00 | 14 | `CORPT00C.md` |
//! | [`cobil00c`] | `COBIL00C` | CB00 | 15 | `COBIL00C.md` |
//! | [`cousr00c`] | `COUSR00C` | CU00 | 16 | `COUSR00C.md` |
//! | [`cousr01c`] | `COUSR01C` | CU01 | 17 | `COUSR01C.md` |
//! | [`cousr02c`] | `COUSR02C` | CU02 | 18 (the user read shared with `COUSR03C`) and 19 | `COUSR02C.md` |
//! | [`cousr03c`] | `COUSR03C` | CU03 | 18 (through [`cousr02c`]) and 20 | `COUSR03C.md` |
//! | [`copaus0c`] | `COPAUS0C` | CPVS | 21 | `COPAUS0C.md` |
//! | [`copaus1c`] | `COPAUS1C` | CPVD | 22–23 | `COPAUS1C.md` |
//! | [`copaus2c`] | `COPAUS2C`, linked from `COPAUS1C` | none | 23 (the store half) | `COPAUS2C.md` |
//! | [`cotrtlic`] | `COTRTLIC` | CTLI | 24–25 | `COTRTLIC.md` |
//! | [`cotrtupc`] | `COTRTUPC` | CTTU | 26–27 | `COTRTUPC.md` |
//! | [`csutldtc`] | `CSUTLDTC` with `CSUTLDPY`, `CSUTLDWY` and `CSLKPCDY` | none | used by [`coactupc`], [`cotrn02c`] and [`corpt00c`] | `CSUTLDTC.md`; `COACTUPC.md` for the edits only `COACTUPC` performs |
//!
//! Endpoints 3, 16 to 20 and 24 to 27 sit behind the admin gate, which is the verified token's
//! `api::auth::extract::AdminUser` and not a rule of these modules.
//!
//! # Layering
//!
//! - Every module is pure: functions over typed records ([`crate::records`]), over the raw screen
//!   text where a COBOL edit owns the messages for that input, over the verified
//!   [`UserType`](cosgn00c::UserType) where a program tests the user type, and over an injected
//!   [`Clock`](crate::clock::Clock) where a rule needs the date or the time. They return typed
//!   outcomes carrying the program's message literals.
//! - No module performs I/O or depends on `sqlx` (R3): no file, database, broker, network,
//!   environment or system-clock access.
//! - `api::services::<program>` loads through the repository traits, applies these rules and
//!   persists what they decide. `api::handlers::<program>` only extracts the request, calls its
//!   service and maps the outcome (Thin Request Handlers).
//!
//! # Audit contract
//!
//! - Each module ports the `server` rows of its audit file and the server half of its `both`
//!   rows, and cites their rule IDs in its doc comments (a8). The `client` rows send no request
//!   and belong to the screens of `modern/frontend/src/screens/`.
//! - A path the audit cannot interpret to one result is marked `HALTED(Hn)` with its ID from
//!   `modern/audit/HALTS.md`, or with the candidate name its audit file proposes, and nothing
//!   presents it as a parity result (AAP 0.8.5).
//! - The modules of the collection endpoints, [`cocrdlic`], [`cotrn00c`], [`cousr00c`],
//!   [`copaus0c`] and [`cotrtlic`], expose `DEFAULT_PAGE_SIZE`, the program's screen size, and
//!   take the page size as a parameter, which `api` bounds to 1–100 (Bounded Pagination).
//! - The attribute on this file's first line denies `unwrap`, `expect` and indexing in this
//!   module and in every child, unit tests included: screen text is untrusted input, and every
//!   edit over it answers any input with a value or a typed error (Total Parsers Over Untrusted
//!   Input).

// No outer `///` docs on these declarations: rustdoc would then resolve the intra-doc links of
// each child's own `//!` docs from this module's scope, and `cargo doc` would fail on every one.
// The table above and each child's first doc line name its program.
pub mod coactupc;
pub mod coactvwc;
pub mod coadm01c;
pub mod cobil00c;
pub mod cocrdlic;
pub mod cocrdslc;
pub mod cocrdupc;
pub mod comen01c;
pub mod copaus0c;
pub mod copaus1c;
pub mod copaus2c;
pub mod corpt00c;
pub mod cosgn00c;
pub mod cotrn00c;
pub mod cotrn01c;
pub mod cotrn02c;
pub mod cotrtlic;
pub mod cotrtupc;
pub mod cousr00c;
pub mod cousr01c;
pub mod cousr02c;
pub mod cousr03c;
pub mod csutldtc;
