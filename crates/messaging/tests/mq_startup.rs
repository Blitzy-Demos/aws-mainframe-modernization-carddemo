//! The start-up failure reports of the `mq` binary.
//!
//! Each case runs the real binary (`CARGO_BIN_EXE_mq`) as a child process with
//! an emptied environment plus the variables the case sets, and needs no
//! Docker: every case fails before the broker is reached. The database URL of
//! every case points at a loopback port nothing listens on and carries
//! [`SECRET`] as its password, so a case that reached the database would be
//! refused, and none may write the password.
//!
//! The binary must exit with status 1, write exactly one line on standard
//! error, `mq: <stage>: <error>`, keep standard output JSON-only with the
//! failure's own JSON event among its lines, and do it at once.

use std::net::{Ipv4Addr, TcpListener};
use std::process::Command;
use std::time::{Duration, Instant};

use carddemo_runtime::config::keys;

/// A stand-in for a credential written into the database URL.
const SECRET: &str = "not-a-real-mq-secret";

/// How long one failing start may take: well under the 30-second acquire
/// timeout a refused database used to wait out.
const PROMPT: Duration = Duration::from_secs(10);

/// What the one standard error line of a case must read after its stage
/// prefix, `mq: <stage>: `.
enum Cause {
    /// Exactly this text.
    Exact(&'static str),
    /// Text that holds this fragment.
    Holding(&'static str),
}

/// One failing start of `mq`.
struct Case {
    /// The stage the failure is reported at.
    stage: &'static str,
    /// The variable the case sets, with its value, beside the database URL.
    variable: Option<(&'static str, &'static str)>,
    /// What the standard error line reads after its stage prefix.
    cause: Cause,
    /// The message of the JSON event that reports the failure on standard
    /// output.
    event: &'static str,
}

/// A loopback port nothing listens on: bound to take a free port, then
/// released, so a connection to it is refused.
fn refused_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap_or_else(|error| panic!("cannot bind a loopback port: {error}"));
    listener
        .local_addr()
        .unwrap_or_else(|error| panic!("cannot read the bound loopback port: {error}"))
        .port()
}

#[test]
fn each_start_up_failure_is_one_stderr_line_beside_json_stdout() {
    let cases = [
        Case {
            stage: "configuration",
            variable: Some((keys::HTTP_ADDR, "notanaddr")),
            cause: Cause::Exact(r#"invalid value for CARDDEMO_HTTP_ADDR: "notanaddr""#),
            event: "mq start-up failed",
        },
        Case {
            stage: "telemetry",
            variable: Some((keys::RUST_LOG, "job=loud")),
            cause: Cause::Holding("invalid log filter: "),
            event: "mq start-up failed",
        },
        Case {
            stage: "clock",
            variable: Some((keys::CLOCK_OVERRIDE, "not-a-time")),
            cause: Cause::Exact(
                "invalid clock override: CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: \
                 not-a-time",
            ),
            event: "mq stopped",
        },
        Case {
            stage: "clock",
            variable: Some((keys::CLOCK_OVERRIDE, "not\na-time")),
            cause: Cause::Exact(
                "invalid clock override: CARDDEMO_CLOCK_OVERRIDE is not an RFC 3339 instant: \
                 not a-time",
            ),
            event: "mq stopped",
        },
        Case {
            stage: "database",
            variable: None,
            cause: Cause::Holding("Connection refused"),
            event: "mq stopped",
        },
    ];
    for case in &cases {
        check(case);
    }
}

/// Runs `mq` as `case` sets it up and checks its exit status, its one
/// standard error line, its JSON standard output and that neither stream
/// holds the database URL or its password.
fn check(case: &Case) {
    let program = env!("CARGO_BIN_EXE_mq");
    let database_url = format!(
        "postgres://carddemo:{SECRET}@127.0.0.1:{}/carddemo",
        refused_port()
    );
    let mut command = Command::new(program);
    command.env_clear().env(keys::DATABASE_URL, &database_url);
    // A coverage run (cargo llvm-cov) names each process's profile file in
    // LLVM_PROFILE_FILE; cleared, the child would write default_*.profraw into
    // the crate directory. Only that variable is passed on, and only when set.
    if let Some(profile_file) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile_file);
    }
    if let Some((key, value)) = case.variable {
        command.env(key, value);
    }
    let started = Instant::now();
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let context = format!(
        "{} {:?}\n--- stderr ---\n{stderr}\n--- stdout ---\n{stdout}",
        case.stage, case.variable
    );

    assert_eq!(output.status.code(), Some(1), "{context}");
    assert!(elapsed < PROMPT, "took {elapsed:?}: {context}");

    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 1, "{context}");
    assert!(stderr.ends_with('\n'), "{context}");
    let prefix = format!("mq: {}: ", case.stage);
    let Some(text) = lines[0].strip_prefix(&prefix) else {
        panic!("the stderr line does not start with {prefix:?}: {context}");
    };
    match case.cause {
        Cause::Exact(expected) => assert_eq!(text, expected, "{context}"),
        Cause::Holding(fragment) => assert!(text.contains(fragment), "{context}"),
    }

    for line in stdout.lines() {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "stdout line is not a JSON object: {line:?}\n{context}"
        );
    }
    let stage_field = format!(r#""stage":"{}""#, case.stage);
    assert!(
        stdout
            .lines()
            .any(|line| line.contains(case.event) && line.contains(&stage_field)),
        "no {:?} event with {stage_field}: {context}",
        case.event
    );

    for stream in [&stdout, &stderr] {
        assert!(!stream.contains(SECRET), "{context}");
        assert!(!stream.contains(&database_url), "{context}");
    }
}
