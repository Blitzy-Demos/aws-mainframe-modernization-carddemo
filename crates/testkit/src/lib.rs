//! Shared test stack for the `CardDemo` workspace: dev-only support that the
//! integration suites of `carddemo-persistence`, `carddemo-api`,
//! `carddemo-batch` and `carddemo-messaging` use instead of writing their own.
//! The crate appears only under the `[dev-dependencies]` of those four crates
//! and never in a `[dependencies]` table.
//!
//! It provides:
//!
//! - [`containers`]: a [`TestStack`] holding a freshly migrated `PostgreSQL`
//!   16.15 database and, on request, a `RabbitMQ` 3.13.7 broker loaded with the
//!   repository's `infra/rabbitmq` definitions. Both run on the image tags
//!   `docker-compose.yml` uses. The stack hands out a `PgPool`, the AMQP URL, a
//!   `lapin` connection, the fixture seed, and text-typed SQL helpers that read
//!   every column in its `PostgreSQL` text form.
//! - [`fixtures`]: absolute paths to the read-only COBOL fixture corpus and the
//!   parity documents, plus unique scratch directories for test output.
//! - [`block_on`]: the single process-wide async runtime that drives the stack
//!   from plain `#[test]` functions.
//!
//! The crate is also the home of the repository's static gates:
//! `tests/hygiene.rs` (source, manifest, SQL and web hygiene) and
//! `tests/compose_tags.rs` (the container tags and app-profile settings
//! against `docker-compose.yml`).
//!
//! Public signatures name only `sqlx`, `lapin` and standard-library types. The
//! `carddemo-persistence` suites reach this crate through a dev-dependency
//! cycle (persistence, testkit, persistence), so no persistence type may cross
//! this API: those suites would otherwise meet two distinct copies of it.

use std::future::Future;
use std::sync::OnceLock;

use tokio::runtime::{Builder, Runtime};

pub mod containers;
pub mod fixtures;

pub use containers::TestStack;

/// The one runtime of the test process. Connection pools, AMQP connections and
/// containers keep their I/O bound to the runtime that created them, so every
/// [`block_on`] call uses this instance. A [`TestStack`] dropped outside any
/// runtime enters it to remove its containers; one dropped inside a runtime
/// keeps that runtime for them. Either way, the stack's network is removed on a
/// thread of its own with its own current-thread runtime.
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Returns the process-wide multi-thread runtime, building it on first use.
///
/// [`TestStack`]'s `Drop` enters it when a stack is dropped outside any
/// runtime, because removing a container needs a current runtime handle.
///
/// # Panics
///
/// Panics if the runtime cannot be built, for example when the operating system
/// refuses to start its worker threads or its I/O driver.
pub(crate) fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap_or_else(|error| {
                panic!("carddemo-testkit cannot build its tokio runtime: {error}")
            })
    })
}

/// Runs `future` to completion on the shared test runtime and returns its output.
///
/// Every call uses the same runtime, built once per test process. A `PgPool`,
/// connection or container created in one call therefore stays usable in later
/// calls, which a runtime per call would break. Typical use in a plain
/// `#[test]`: `let stack = carddemo_testkit::block_on(TestStack::postgres());`,
/// followed by further `block_on` calls that use `stack.pool()`.
///
/// Parallel test threads may call this at the same time. Each caller drives its
/// own future on its own thread, while tasks it spawns run on the runtime's
/// shared worker threads.
///
/// The signature names only standard-library types, so crates without a
/// `tokio` dependency drive their async code through it.
///
/// # Panics
///
/// Panics if the shared runtime cannot be built on first use, if `future`
/// panics, or if called from inside an asynchronous execution context: within
/// another `block_on`, inside an `async` block being polled, or in a
/// `#[tokio::test]`.
pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}
