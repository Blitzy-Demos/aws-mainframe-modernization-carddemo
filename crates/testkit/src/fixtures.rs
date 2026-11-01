//! Absolute paths to the read-only COBOL fixture corpus and the parity documents,
//! plus a unique scratch-directory helper for test output.
//!
//! Every path derives from `CARGO_MANIFEST_DIR`, the absolute `crates/testkit`
//! directory at compile time, so a test resolves the same files whatever its
//! working directory. Consumer suites take their repository paths from here
//! rather than computing their own.
//!
//! The corpus under `app/` is the parity oracle: these helpers only build paths
//! to it and never write there. Test output goes to [`temp_dir`], under the
//! system temporary directory, never under the repository's `var/`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-process sequence that keeps [`temp_dir`] names distinct across parallel
/// test threads, even within one clock tick.
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Absolute path of the repository root, two levels above `crates/testkit`.
///
/// # Panics
///
/// Panics if the crate is not located at `<root>/crates/testkit`.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("carddemo-testkit must be located at <repository root>/crates/testkit")
}

/// Absolute path of `app/data/ASCII`, the seed and parity input.
///
/// It holds 9 files, 626 records in total: `acctdata.txt`, `carddata.txt`,
/// `cardxref.txt`, `custdata.txt`, `dailytran.txt`, `discgrp.txt`,
/// `tcatbal.txt`, `trancatg.txt` and `trantype.txt`. Read only.
///
/// # Panics
///
/// Panics under the same condition as [`repo_root`].
#[must_use]
pub fn ascii_dir() -> PathBuf {
    repo_root().join("app/data/ASCII")
}

/// Absolute path of `app/data/EBCDIC`, the EBCDIC half of the codec round trip.
///
/// It holds 13 `AWS.M2.CARDDEMO.*` files: 12 ending in `.PS`, plus
/// `AWS.M2.CARDDEMO.DALYTRAN.PS.INIT`. Read only.
///
/// # Panics
///
/// Panics under the same condition as [`repo_root`].
#[must_use]
pub fn ebcdic_dir() -> PathBuf {
    repo_root().join("app/data/EBCDIC")
}

/// Absolute path of `app/jcl/DUSRSECJ.jcl`, whose `SYSUT1` in-stream data holds
/// the 10 user rows the seed loads. Read only.
///
/// # Panics
///
/// Panics under the same condition as [`repo_root`].
#[must_use]
pub fn dusrsecj_jcl() -> PathBuf {
    repo_root().join("app/jcl/DUSRSECJ.jcl")
}

/// Absolute path of the document `name` under `docs/parity`, for example
/// `parity_doc("parity-oracle-audit.md")` or `parity_doc("validation-audit.md")`.
///
/// # Panics
///
/// Panics under the same condition as [`repo_root`].
#[must_use]
pub fn parity_doc(name: &str) -> PathBuf {
    repo_root().join("docs/parity").join(name)
}

/// Creates and returns a fresh directory `<prefix>-<pid>-<nanos>-<counter>` under
/// [`std::env::temp_dir`].
///
/// Batch tests use it as the output directory they hand to the `batch` binary as
/// `CARDDEMO_BATCH_DIR` or `CARDDEMO_STATEMENT_DIR`. The process ID, the
/// wall-clock nanoseconds and a per-process counter keep names distinct across
/// test processes and parallel test threads. The directory is not removed
/// afterwards; deleting it is the caller's choice.
///
/// # Panics
///
/// Panics, naming the path and the error, if the directory cannot be created.
#[must_use]
pub fn temp_dir(prefix: &str) -> PathBuf {
    let pid = process::id();
    // A clock set before the Unix epoch yields 0; the counter still separates names.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}-{counter}"));
    fs::create_dir_all(&dir)
        .unwrap_or_else(|error| panic!("cannot create test directory {}: {error}", dir.display()));
    dir
}
