//! The domain services of the 27 REST operations: the one home of request logic outside
//! `domain` (Thin Request Handlers, AAP 0.3.4.1, 0.5.4).
//!
//! # Layering contract
//!
//! - **Handlers** (`api::handlers::<program>`) extract the validated DTO and the `SignedOnUser`
//!   or `AdminUser`, call one function of the program's module here, and map what it returns
//!   through the `From` conversions of `api::dto` and `api::error`. A handler does no arithmetic
//!   on a domain value, holds no `match` on a domain outcome and names neither `persistence` nor
//!   `sqlx`.
//! - **Services** (`services::<program>`) load through the repository traits of `persistence`,
//!   called fully qualified (`AccountRepository::lock_by_id(&mut tx, id)`), reading through the
//!   state's `PgStore` and writing inside a `PgTx`. They apply the rules of
//!   `domain::online::<program>`, persist what those rules decide, and return the domain's
//!   outcome or `ServiceError<F>`, where `F` is the program's domain failure type.
//! - A service never names `sqlx`, holds SQL text, calls `runtime::migrate` (Single Schema
//!   Executor) or reaches `RabbitMQ`, which no online transaction uses (AAP 0.5.3). It narrows
//!   an integer only through a checked `try_from`, never an `as` cast (Checked Integer
//!   Narrowing).
//! - `corpt00c` is the only service whose code outside its tests touches the filesystem: it
//!   writes endpoint 14's job deck under `CARDDEMO_INTRDR_DIR` through `std::fs` and
//!   `runtime::gdg::next_generation` (AAP 0.3.4.4, 0.5.3).
//!
//! Every module and function here is `pub(crate)`, because only `api::handlers` calls them, and
//! nothing is re-exported, so each function has exactly one path,
//! `crate::services::<program>::<action>`.
//!
//! # Endpoints
//!
//! One module per online program: 21 modules for the 22 programs behind the endpoints of AAP
//! 0.3.4.1.
//!
//! | Endpoints | Transaction / program | Functions |
//! |---|---|---|
//! | 1 | CC00 / COSGN00C | `cosgn00c::signon` |
//! | 2 | CM00 / COMEN01C | `comen01c::menu` |
//! | 3 `[A]` | CA00 / COADM01C | `coadm01c::menu` |
//! | 4 | CAVW / COACTVWC | `coactvwc::view` |
//! | 5, 6 | CAUP / COACTUPC | `coactupc::fetch`, `coactupc::save` |
//! | 7 | CCLI / COCRDLIC | `cocrdlic::list` |
//! | 8 | CCDL / COCRDSLC | `cocrdslc::view` |
//! | 9, 10 | CCUP / COCRDUPC | `cocrdupc::fetch`, `cocrdupc::save` |
//! | 11 | CT00 / COTRN00C | `cotrn00c::list` |
//! | 12 | CT01 / COTRN01C | `cotrn01c::view` |
//! | 13 | CT02 / COTRN02C | `cotrn02c::add` |
//! | 14 | CR00 / CORPT00C | `corpt00c::submit` |
//! | 15 | CB00 / COBIL00C | `cobil00c::pay` |
//! | 16 `[A]` | CU00 / COUSR00C | `cousr00c::list` |
//! | 17 `[A]` | CU01 / COUSR01C | `cousr01c::add` |
//! | 18, 19 `[A]` | CU02 / COUSR02C, CU03 / COUSR03C | `cousr02c::fetch`, `cousr02c::update` |
//! | 20 `[A]` | CU03 / COUSR03C | `cousr03c::delete` |
//! | 21 | CPVS / COPAUS0C | `copaus0c::summary` |
//! | 22, 23 | CPVD / COPAUS1C, COPAUS2C | `copaus1c::detail`, `copaus1c::toggle_fraud` |
//! | 24, 25 `[A]` | CTLI / COTRTLIC | `cotrtlic::list`, `cotrtlic::list_actions` |
//! | 26, 27 `[A]` | CTTU / COTRTUPC | `cotrtupc::fetch`, `cotrtupc::act` |
//!
//! - COPAUS2C has no module: COPAUS1C `LINK`s it, so its rules run inside
//!   `copaus1c::toggle_fraud`, in COPAUS1C's unit of work.
//! - Endpoint 18 is the user read COUSR02C and COUSR03C share, because their read paragraphs
//!   carry identical messages; it lives in `cousr02c`, and `cousr03c` holds only the delete.
//! - CP00, CDRD and CDRA have no REST endpoint: they are the `RabbitMQ` workers of `messaging`.
//!
//! # Shared conventions
//!
//! - **Identity by reference.** A service takes the `SignedOnUser` or `AdminUser` its handler
//!   extracted from the verified JWT, never an identity from a body, a query string or a header
//!   (AAP 0.5.1). An `AdminUser` argument is the admin gate, already passed. Endpoint 8's
//!   disclosure of a full card number depends on `user.user_type` alone (AAP 0.3.4.8).
//!   `cosgn00c::signon` takes no user, because endpoint 1 issues the token.
//! - **Path parameters as `&str`.** The request layer judges a path segment first. A segment
//!   that is not UTF-8 once percent-decoded is refused with 400 everywhere. The validated path
//!   DTOs also enforce the transport and map bounds, answering a segment outside them with 400
//!   `crate::dto::FIELD_LENGTH_MESSAGE` naming its field: `crate::dto::ValidPath<UserIdPath>`
//!   (endpoints 18 to 20) the map `LENGTH` and printable `PIC X(8)` text of `USRIDIN`, and
//!   `crate::dto::cotrtupc::TypeCodePath` (endpoints 26 and 27) the two characters of
//!   `TRTYPCDI`. Every other segment goes through `crate::dto::RawPath`, which refuses only the
//!   undecodable one, with `crate::dto::INVALID_PATH`. The text accepted then reaches the service
//!   as `&str`, unparsed and untrimmed, and a total edit answers a malformed key, a blank one
//!   included, with the program's own message before any read: the program's domain edit, or,
//!   for the card handle of endpoint 10, a parse whose failure is COCRDUPC's lock failure.
//! - **Bounded reads.** The page size is the one `crate::dto::resolve_page_size` gives: the
//!   validated `page_size` of the request (1 to 100), or the program's screen size when it is
//!   absent. Every browse binds a repository row limit derived from it, the page plus the
//!   look-ahead or skip rows the program's browse reads, converted with a checked
//!   `i64::try_from` (Bounded Pagination).
//! - **Time from the state.** Every date or timestamp a rule needs comes from `state.clock`
//!   (`FixedClock` in tests), never from the system clock directly.
//! - **Store errors.** A failed statement that the program answers with an audited text of its
//!   own is logged by the service and mapped into the domain's lookup or result value, so it
//!   arrives as `ServiceError::Rule`. `ServiceError::Store` is only for a store error with no
//!   audited text, such as a failed `begin`, `commit`, `rollback` or savepoint. The event that
//!   records the cause of a failure answered with a 5xx, the program's store-class text or
//!   `UNABLE_TO_PROCESS` for `ServiceError::Store` and `ServiceError::Internal`, is an `ERROR`,
//!   so `RUST_LOG=error` shows the cause of every 500 that a failed store call or task produced.
//!   A `WARN` is only for a failure the program's audited flow carries on past, so that the
//!   request can still succeed: COBIL00C's cross-reference read (COBIL00C-022). An outcome the
//!   audit gives the request itself logs nothing, whatever its status: a taken user ID (400), or
//!   COUSR03C's `Unable to Update User...` (500) for an ID with no row (X-COUSR03C-H1).
//! - **Savepoints.** A failed statement aborts a `PostgreSQL` transaction, so a statement after
//!   whose failure the COBOL unit of work goes on runs inside its own savepoint: COACTUPC's
//!   customer rewrite (COACTUPC-107), COBIL00C's payment statements (E11) and COPAUS2C's fraud
//!   insert, which falls back to an update on the duplicate key, `SQLCODE -803`.
//! - **Redaction.** No service logs or traces a card number, SSN, CVV, password or record: the
//!   event of a failed store statement carries only the `PersistenceError`, which names an
//!   SQLSTATE and a constraint or column at most (Cardholder Data Redaction, AAP 0.3.4.8).

// In endpoint order. Each declaration stands alone because rustfmt sorts a run of adjacent `mod`
// items, which would undo the order; the trailing comment names the module's endpoints.
pub(crate) mod cosgn00c; // endpoint 1

pub(crate) mod comen01c; // endpoint 2

pub(crate) mod coadm01c; // endpoint 3

pub(crate) mod coactvwc; // endpoint 4

pub(crate) mod coactupc; // endpoints 5 and 6

pub(crate) mod cocrdlic; // endpoint 7

pub(crate) mod cocrdslc; // endpoint 8

pub(crate) mod cocrdupc; // endpoints 9 and 10

pub(crate) mod cotrn00c; // endpoint 11

pub(crate) mod cotrn01c; // endpoint 12

pub(crate) mod cotrn02c; // endpoint 13

pub(crate) mod corpt00c; // endpoint 14

pub(crate) mod cobil00c; // endpoint 15

pub(crate) mod cousr00c; // endpoint 16

pub(crate) mod cousr01c; // endpoint 17

pub(crate) mod cousr02c; // endpoints 18 and 19

pub(crate) mod cousr03c; // endpoint 20

pub(crate) mod copaus0c; // endpoint 21

pub(crate) mod copaus1c; // endpoints 22 and 23

pub(crate) mod cotrtlic; // endpoints 24 and 25

pub(crate) mod cotrtupc; // endpoints 26 and 27
