//! Shared process plumbing of the `CardDemo` replacement binaries.
//!
//! Its consumers are the `carddemo-api`, `carddemo-mq`, `carddemo-batch` and `carddemo-migrate`
//! binaries and the `test-support` harness. Each item has exactly one path, under its module.
//!
//! # Modules
//!
//! - [`config`]: the only reader of the application environment variables that
//!   `modern/.env.example` lists: database and `RabbitMQ` URLs, request queue names, the API
//!   bind address, the JWT settings, the output and fixture directories and the span export
//!   switch.
//! - [`telemetry`]: JSON tracing to standard output, optional `OpenTelemetry` span export to
//!   standard output, the redaction-safe span constructors of the HTTP, broker and batch paths,
//!   and the `carddemo::alert` target of alert events.
//! - [`db`]: the `PostgreSQL` pool constructor, which takes every connection setting from
//!   `DATABASE_URL` or an explicit default and none from the libpq environment variables or the
//!   pgpass file, refuses a URL that names no host, user or password, a `PGOPTIONS` that is not
//!   blank and a query parameter that `sqlx` would log with its value, reports a refused
//!   connection, an unresolvable host or a timeout as such, returns a `PgPool` and never
//!   migrates, and the value-free summary under which a `sqlx` error is logged.
//! - [`gdg`]: generation file names, the local counterpart of the `(+1)` generations of the
//!   generation data groups whose bases `DEFGDGB.jcl` defines.
//! - [`migrate`]: the single schema executor. It embeds `crates/persistence/migrations`, and the
//!   `apply` function of the `migrate` module is the only path by which the schema reaches a
//!   database; migration `0006` performs the create-and-seed of `CREADB21.jcl` once per
//!   database. Only the `carddemo-migrate` binary and the `test-support` harness call it. It
//!   also holds the value-free summary under which a migration error is logged.
//!
//! # Constraints
//!
//! - No `sqlx` query macros, so the crate compiles against an empty database and a fresh
//!   checkout creates its schema before `persistence`, whose macros need that schema, is built.
//! - No dependency on any other workspace crate or on any HTTP crate: the api hands the span
//!   constructors plain strings.
//! - Output goes to standard output and local files only, with no collector, network exporter,
//!   cloud service or object storage (Local-Only Runtime).

pub mod config;
pub mod db;
pub mod gdg;
pub mod migrate;
pub mod telemetry;
