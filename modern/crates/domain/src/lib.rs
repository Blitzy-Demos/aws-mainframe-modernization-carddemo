//! The business rules of `CardDemo`: every audited COBOL rule the replacement depends on, as a
//! pure function over typed copybook records that returns a typed outcome and the exact COBOL
//! message literals (AAP 0.1.2).
//!
//! The `api` services, the `messaging` workers and the `carddemo-batch` jobs load records through
//! the `persistence` repositories, hand them to these rules and persist what the rules decide.
//! Each endpoint, worker and job takes its rules and its messages from the module of the COBOL
//! program that owns it.
//!
//! # Layering
//!
//! - **Pure.** Library code performs no database, broker, network, filesystem or environment
//!   access and keeps no global state. Only unit tests read files, in place and read-only: the
//!   fixtures of `app/data/ASCII` and the copybooks of `app/cpy`.
//! - **No `sqlx`** (R3). Neither `sqlx` nor `persistence` appears in any section of this crate's
//!   manifest, so a query written here does not compile; `scripts/check.sh` step 8 also scans
//!   the dependency tree. The dependency runs the other way: `persistence` maps these records to
//!   rows (codec B).
//! - **Time through [`Clock`] only.** Rules take `&dyn Clock` and services hold
//!   `Arc<dyn Clock>`. [`SystemClock`], injected by the binaries, is the one reader of the
//!   operating-system clock; tests inject [`FixedClock`] and compare time fields by format.
//! - **One other operating-system facility:** the random source that salts [`password::hash`].
//! - **Session state as parameters.** What a COBOL program receives in the `COCOM01Y`
//!   COMMAREA arrives as arguments: the user ID and the user type
//!   ([`UserType`](online::cosgn00c::UserType)) from the verified token, the account, card and
//!   page keys from the request. No rule keeps them between calls.
//! - **Money through [`money`] only.** No floating point; every result reaches its receiving
//!   picture through [`MoneyExpr::store`], which truncates as the COBOL does without `ROUNDED`.
//! - **Protected fields as [`sensitive`] types.** Their `Debug`, `Display` and, where it exists,
//!   `Serialize` show only the redacted form, so formatting or serialising an outcome, error or
//!   message cannot disclose a card number, SSN, CVV or password. Only `expose()`, whose callers
//!   [`sensitive`] lists, reaches the raw value.
//!
//! # Modules
//!
//! - [`sensitive`]: [`CardNumber`], [`Ssn`], [`Cvv`] and [`Password`], the redacting wrappers
//!   of the protected fields, and [`DisclosedCardNumber`], the one full-number serialiser.
//! - [`money`]: [`Money`], [`Rate`] and [`MoneyExpr`], the copybook amount algebra, with the
//!   COBOL store rule and [`monthly_interest`](money::monthly_interest), the only path from a
//!   rate to money (R18).
//! - [`clock`]: the injected [`Clock`] and the COBOL renderings of its readings.
//! - [`messages`]: the texts shared through `CSMSG01Y` and `CSMSG02Y`; a literal only one
//!   program moves is a constant of that program's module.
//! - [`password`]: argon2id [`hash`](password::hash) and [`verify`](password::verify) over the
//!   eight bytes of `SEC-USR-PWD`, stored as a hash instead of plaintext (E25).
//! - [`records`]: one struct per copybook layout, with its codec A byte form through
//!   [`FixedRecord`](copybook::FixedRecord).
//! - [`online`]: the rules of the 22 online programs and `CSUTLDTC` behind the 27 REST
//!   endpoints, one module per program.
//! - [`batch`]: the rules of the in-scope batch programs and the fixture loads behind the 15
//!   `carddemo-batch` subcommands.
//! - [`messaging`]: the rules of `CODATE01`, `COACCT01` and `COPAUA0C` behind the CDRD, CDRA and
//!   CP00 `RabbitMQ` workers.
//!
//! # Re-exports
//!
//! - [`copybook`], the whole crate. A consumer that does not depend on `copybook` itself names
//!   the PIC types through it: `domain::copybook::PicX`, `PicU`, `PicDate` and the rest.
//! - The shared components of AAP 0.5.4 at the crate root, each also reachable through its
//!   module: [`Money`], [`MoneyExpr`] and [`Rate`]; [`Clock`], [`FixedClock`] and
//!   [`SystemClock`]; [`CardNumber`], [`Cvv`], [`DisclosedCardNumber`], [`Password`] and [`Ssn`].
//!
//! # Audit contract
//!
//! - No rule without its audit (a8). Every rule module under [`online`], [`batch`] and
//!   [`messaging`] ports rows of its `modern/audit/**` file and cites their rule IDs in its doc
//!   comments; each test that proves a row carries that row's `// audit:` tag. The
//!   `audit_coverage` test of `test-support` fails on a rule module without a citation and on a
//!   cited ID that names no row.
//! - Audited COBOL defects are reproduced, not corrected (Behaviour constraint). Every departure
//!   from the COBOL is an escalation of `modern/audit/ESCALATIONS.md` that states its reason,
//!   such as the i10 credit-limit guard (E30) and cardholder-data redaction (E32).
//! - A path the audit cannot interpret to one result is marked `HALTED(Hn)` with its halt ID
//!   from `modern/audit/HALTS.md`, and nothing presents it as a parity result (AAP 0.8.5).

// The shared components come first, then the rule layers that build on them; the blank lines
// keep rustfmt from sorting the declarations. They carry no outer `///` docs: rustdoc would then
// resolve the intra-doc links of each module's own `//!` docs from this scope, and fail.
pub mod sensitive;

pub mod money;

pub mod clock;

pub mod messages;

pub mod password;

pub mod records;

pub mod online;

pub mod batch;

pub mod messaging;

pub use copybook;

pub use clock::{Clock, FixedClock, SystemClock};
pub use money::{Money, MoneyExpr, Rate};
pub use sensitive::{CardNumber, Cvv, DisclosedCardNumber, Password, Ssn};
