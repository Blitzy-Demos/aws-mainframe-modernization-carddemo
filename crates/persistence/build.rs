//! Build script for `carddemo-persistence`.
//!
//! `sqlx::migrate!("../../migrations")` embeds the workspace `migrations/` directory into
//! the crate at compile time. The macro reads each file through `include_str!`, so Cargo
//! sees edits to existing migrations, but not a migration added to or removed from the
//! directory: the build would keep a stale migrator. Declaring the directory here makes
//! Cargo rebuild the crate whenever its contents change. The path is relative to this
//! package root, where Cargo runs build scripts.

fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
