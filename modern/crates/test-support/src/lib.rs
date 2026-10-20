//! Dev-only test harness for the `CardDemo` workspace: the integration tests of `persistence`,
//! `api`, `messaging` and `batch` get their containers, fixtures, audit index and trace capture
//! here. The crate is listed only under their `[dev-dependencies]`, never as a normal dependency.
//!
//! # Public-API boundary
//!
//! `persistence` dev-depends on this crate while this crate depends on `persistence`, so Cargo
//! compiles `persistence` twice and the types of the two copies differ. Every public signature
//! here therefore uses only standard-library types, `sqlx` types such as `PgPool`, `lapin` types
//! and this crate's own structs built from them, never a `persistence`, `domain` or `copybook`
//! type.
//!
//! # Local-only runtime
//!
//! The stores run only as testcontainers on the local Docker engine and are removed when their
//! guard drops. Their credentials are the local values `carddemo`/`carddemo`.
//!
//! # Modules
//!
//! - [`audit_index`]: the audit-table index and the scanners for test tags and rule citations.
//! - [`fixtures`]: fixture paths read in place, fixture loading and redaction needles.
//! - [`postgres`]: a `postgres:16` container migrated through the `runtime` migration entrypoint.
//! - [`rabbitmq`]: a `rabbitmq:3.13-management` container with the broker definitions mounted.
//! - [`trace_capture`]: an in-memory capture of the JSON tracing output.

// Crate-specific lints can live only here: a member inheriting `[lints] workspace = true` cannot
// add entries of its own. `deny` tightens the workspace set for the total parsing in
// `audit_index` and `fixtures`; it suppresses nothing.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

pub mod audit_index;
pub mod fixtures;
pub mod postgres;
pub mod rabbitmq;
pub mod trace_capture;

use std::fmt;
use std::path::{Path, PathBuf};

/// The `modern/` directory, for example `/src/carddemo/modern/crates/test-support/../..` for a
/// checkout at `/src/carddemo`.
///
/// `CARGO_MANIFEST_DIR` expands when this crate compiles, so the result is the same `modern/`
/// directory whichever crate's test calls it. The path is not canonicalised; callers that need an
/// absolute path, such as Docker bind mounts, canonicalise it themselves.
#[must_use]
pub fn modern_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The repository root, one level above [`modern_dir`]. The fixtures live beneath it, at
/// `app/data/ASCII` and `app/jcl`.
#[must_use]
pub fn repo_root() -> PathBuf {
    modern_dir().join("..")
}

/// A failure of the harness itself: a container that does not start, a store that never becomes
/// ready, or a fixture that cannot be read or loaded.
///
/// The message is the `Display` text of the library error behind it (sqlx, lapin, testcontainers,
/// `PersistenceError`, the domain `LoadError` and `PasswordError`, or `std::io::Error`), none of
/// which carries record bytes. No caller formats a card number, SSN, CVV or password into one, so
/// a failing test may print it (Cardholder Data Redaction).
#[derive(Debug)]
pub struct HarnessError {
    context: &'static str,
    message: String,
}

impl HarnessError {
    /// Wraps `err` under `context`, the harness step that failed, such as `"start postgres"`.
    pub(crate) fn new(context: &'static str, err: &dyn fmt::Display) -> Self {
        Self {
            context,
            message: err.to_string(),
        }
    }
}

impl fmt::Display for HarnessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.context, self.message)
    }
}

impl std::error::Error for HarnessError {}

#[cfg(test)]
mod tests {
    use super::{HarnessError, modern_dir, repo_root};

    #[test]
    fn paths_resolve_inside_the_checkout() {
        let modern = modern_dir();
        assert!(modern.join("Cargo.toml").is_file());
        assert!(modern.join("crates/test-support/Cargo.toml").is_file());
        let root = repo_root();
        assert!(root.join("app/data/ASCII/acctdata.txt").is_file());
        assert!(root.join("app/jcl/DUSRSECJ.jcl").is_file());
    }

    #[test]
    fn display_is_context_then_message() {
        let err = HarnessError::new("start postgres", &"connection refused");
        assert_eq!(err.to_string(), "start postgres: connection refused");
    }
}
