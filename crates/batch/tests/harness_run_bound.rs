//! The bound on the `batch` children of the shared harness: a child that never exits is
//! killed and reaped once the bound of its harness passes, and its run panics with a message
//! that names the arguments, the bound, where the bound was set and the reaped status,
//! instead of waiting forever.
//!
//! `batch posttran` opens its `--input` file before it reads or writes anything else. The
//! input here is a FIFO that no process opens for writing, so that open never returns and the
//! child runs until the harness kills it.

pub mod common;

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::process::Command;
use std::time::{Duration, Instant};

use common::Harness;

/// The bound this test sets in place of [`common::RUN_TIMEOUT`].
const BOUND: Duration = Duration::from_secs(3);

/// How much later than [`BOUND`] the run may end, for the kill, the reap and the collection of
/// the closed pipes on a loaded host. It is far below [`common::RUN_TIMEOUT`], so a run stopped
/// at the default bound instead of the override fails the test.
const SLACK: Duration = Duration::from_secs(15);

#[test]
fn a_child_that_never_exits_is_killed_and_reaped_at_the_bound() {
    let h = Harness::new().with_run_timeout(BOUND);
    let fifo = h.batch_dir().join("dailytran.fifo");
    let made = Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap_or_else(|error| panic!("cannot run mkfifo: {error}"));
    assert!(made.success(), "mkfifo {} failed: {made}", fifo.display());
    let input = fifo
        .to_str()
        .unwrap_or_else(|| panic!("{} is not UTF-8", fifo.display()));
    let args = ["posttran", "--input", input];

    let started = Instant::now();
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| h.run(&args, None)));
    let elapsed = started.elapsed();

    let payload = match outcome {
        Ok(output) => panic!(
            "batch {args:?} exited with {} instead of blocking on the FIFO\n--- stderr ---\n{}\n\
             --- stdout ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout),
        ),
        Err(payload) => payload,
    };
    let message = panic_text(payload.as_ref());
    assert!(
        elapsed >= BOUND,
        "the run ended {elapsed:?} after its start, before its bound of {BOUND:?}: {message}"
    );
    assert!(
        elapsed <= BOUND + SLACK,
        "the run ended {elapsed:?} after its start, more than {SLACK:?} past its bound of \
         {BOUND:?}: {message}"
    );
    for expected in [
        format!("batch {args:?} (pid "),
        format!("did not exit within its bound of {BOUND:?} (set by Harness::with_run_timeout)"),
        "kill sent SIGKILL, reap returned signal: 9 (SIGKILL)".to_owned(),
        "--- stderr ---".to_owned(),
        "--- stdout ---".to_owned(),
    ] {
        assert!(
            message.contains(&expected),
            "the panic message lacks {expected:?}:\n{message}"
        );
    }
}

/// The text of a panic payload: the formatted message of `panic!`, or its literal.
fn panic_text(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("<a panic payload that is not text>")
}
