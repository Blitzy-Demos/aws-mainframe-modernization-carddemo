//! The rules of the in-scope batch programs, one module per program (AAP 0.3.4.3).
//!
//! Each module turns a job's parameters and input records into typed effects: the rows the job
//! inserts, updates or deletes, its reject records, its output lines, its SYSOUT lines and its
//! return code. Reading and writing files, generation naming (`runtime::gdg`), repository calls
//! and database transactions belong to the `batch` crate's `batch::jobs::*` modules, which drive
//! these rules in the order the COBOL program runs them.
//!
//! | Subcommand | COBOL program | Module | Audit in `modern/audit/batch/` |
//! |---|---|---|---|
//! | `posttran` | `CBTRN02C` | [`cbtrn02c`] | `POSTTRAN.md` |
//! | `intcalc` | `CBACT04C` | [`cbact04c`] | `INTCALC.md` |
//! | `creastmt` | `CBSTM03A` with its subroutine `CBSTM03B` | [`cbstm03a`] | `CREASTMT.md` |
//! | `tranrept` | `CBTRN03C` | [`cbtrn03c`] | `TRANREPT.md` |
//! | `cbpaup0j` | `CBPAUP0C` | [`cbpaup0c`] | `CBPAUP0J.md` |
//! | `mnttrdb2` | `COBTUPDT` | [`cobtupdt`] | `MNTTRDB2.md` |
//! | `acctfile`, `cardfile`, `custfile`, `xreffile`, `discgrp`, `tcatbalf`, `trancatg`, `trantype`, `dusrsecj` | `IDCAMS` `REPRO`, after `IEBGENER` for `dusrsecj` | [`loads`] | `LOADS.md` |
//!
//! The expected results of these rules over the fixture corpus are in `modern/audit/fixtures.md`.
//!
//! # Contract
//!
//! Every child module honours the same rules:
//!
//! - No I/O and no `sqlx` (R3): no file, database, broker, network or environment access.
//! - Time comes only from an injected [`Clock`](crate::clock::Clock), passed as `&dyn Clock`; no
//!   module reads the system time itself.
//! - Money moves only through [`crate::money`]: every result reaches its receiving picture
//!   through [`MoneyExpr::store`](crate::money::MoneyExpr::store), `store::<K>()`, which truncates
//!   as the COBOL does without `ROUNDED`. No floating point.
//! - Dates and timestamps are typed (`PicDate`, `PicTimestamp`, `chrono`), never strings.
//! - Every parser of external bytes is total: any input yields a value or a typed error, never a
//!   panic.
//! - Audited COBOL defects are reproduced, not corrected (Behaviour constraint), among them E07,
//!   E08, E09, E10, E20 and E23 of `modern/audit/ESCALATIONS.md`.
//! - A path the audit cannot interpret to one result is marked `HALTED(..)` with its halt ID from
//!   `modern/audit/HALTS.md`, and nothing presents it as a parity result.

// No outer `///` docs on these declarations: rustdoc would then resolve the intra-doc links of
// each child's own `//!` docs from this module's scope, and `cargo doc` would fail on every one.
// The table above and each child's first doc line name its program.
pub mod cbact04c;
pub mod cbpaup0c;
pub mod cbstm03a;
pub mod cbtrn02c;
pub mod cbtrn03c;
pub mod cobtupdt;
pub mod loads;
