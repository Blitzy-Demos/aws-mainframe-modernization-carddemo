//! Process plumbing shared by the binaries `api` (`crates/api`), `batch`
//! (`crates/batch`) and `mq` (`crates/messaging`).
//!
//! - [`config`]: typed settings. [`config::Settings::from_env`] reads every
//!   `CARDDEMO_*` variable and `RUST_LOG` over the local defaults that
//!   `.env.example` lists.
//! - [`telemetry`]: [`telemetry::init`] installs the process's JSON logs and
//!   span export as the global subscriber and returns the
//!   [`telemetry::TelemetryGuard`] that shuts them down.
//! - [`json_span_exporter`]: the `opentelemetry_sdk` span exporter that writes
//!   each finished span as one JSON line.
//!
//! # Standard output
//!
//! Standard output carries JSON lines only, one object per line, of two kinds:
//!
//! - log events, written by the `tracing-subscriber` JSON formatter;
//! - finished spans, written by [`json_span_exporter::JsonSpanExporter`] as
//!   `"type":"span"` lines holding the trace, span and parent identifiers, the
//!   name, the start and end in Unix nanoseconds, the attributes and the status.
//!
//! # Usage
//!
//! A binary resolves its settings, installs telemetry once, runs, and shuts
//! telemetry down before it exits:
//!
//! ```no_run
//! use carddemo_runtime::config::Settings;
//! use carddemo_runtime::telemetry;
//!
//! let settings = Settings::from_env()?;
//! let guard = telemetry::init("api")?;
//! tracing::info!(http_addr = %settings.http_addr, "api starting");
//! // Run the process here, then shut telemetry down before exiting.
//! guard.shutdown()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! # Dependencies
//!
//! The crate depends on no `carddemo-*` crate, so every adapter crate can use it
//! without a dependency cycle. For that reason
//! [`config::Settings::clock_override`] stays the raw
//! `CARDDEMO_CLOCK_OVERRIDE` text: consumers parse it with the domain clock.

pub mod config;
pub mod json_span_exporter;
pub mod telemetry;
