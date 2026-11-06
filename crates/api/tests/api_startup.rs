//! The start-up failure report of the `api` binary for a rejected
//! `CARDDEMO_CORS_ORIGIN`.
//!
//! Each case runs the real binary (`CARGO_BIN_EXE_api`) as a child process
//! with an emptied environment plus `RUST_LOG` and the origin the case sets,
//! and needs no Docker: the `state` stage rejects the origin before the first
//! database connection attempt. Every origin carries a synthetic credential,
//! in its user information, query, fragment or path, assembled at run time.
//!
//! The binary must exit with status 1 and write exactly one line on standard
//! error, `api: state: invalid CARDDEMO_CORS_ORIGIN: <reason>`. Standard
//! output must stay JSON-only, with the `api failed` ERROR event of stage
//! `state` among its lines carrying the same error text. Neither stream may
//! hold the origin or any part of its credential.

use std::process::Command;

use carddemo_runtime::config::keys;
use serde_json::Value as Json;

/// The user name of the origins that carry user information.
const USER: &str = "carddemo";

/// A stand-in for a password written into the origin's user information.
const PASSWORD: &str = "example-password-7f3a";

/// A stand-in for a token written into the origin's query.
const QUERY_SECRET: &str = "example-query-secret-7f3a";

/// A stand-in for a token written into the origin's fragment.
const FRAGMENT_SECRET: &str = "example-fragment-secret-7f3a";

/// A stand-in for a token written into the origin's path.
const PATH_SECRET: &str = "example-path-secret-7f3a";

/// Every stand-in credential, none of which either stream may hold.
const SECRETS: [&str; 4] = [PASSWORD, QUERY_SECRET, FRAGMENT_SECRET, PATH_SECRET];

/// The reason the `api` gives for an origin with user information.
const USER_INFO: &str = "an origin carries no user information";

/// The reason the `api` gives for an origin with a path, query or fragment.
const PATH: &str = "an origin has no path, query or fragment, not even a trailing `/`";

#[test]
fn a_rejected_origin_is_reported_by_setting_and_reason_only() {
    let cases = [
        (
            format!("http://{USER}:{PASSWORD}@localhost:5173"),
            USER_INFO,
        ),
        (format!("http://{PASSWORD}@localhost:5173"), USER_INFO),
        (format!("http://localhost:5173?token={QUERY_SECRET}"), PATH),
        (
            format!("http://localhost:5173#access_token={FRAGMENT_SECRET}"),
            PATH,
        ),
        (format!("http://localhost:5173/{PATH_SECRET}"), PATH),
    ];
    for (origin, reason) in &cases {
        check(origin, reason);
    }
}

/// Runs `api` with `origin` as `CARDDEMO_CORS_ORIGIN` and checks its exit
/// status, its one standard error line, its JSON standard output with the
/// failure's ERROR event, and that neither stream holds `origin` or a
/// stand-in credential.
fn check(origin: &str, reason: &str) {
    let program = env!("CARGO_BIN_EXE_api");
    let mut command = Command::new(program);
    command
        .env_clear()
        .env(keys::RUST_LOG, "info")
        .env(keys::CORS_ORIGIN, origin);
    // A coverage run (cargo llvm-cov) names each process's profile file in
    // LLVM_PROFILE_FILE; cleared, the child would write default_*.profraw into
    // the crate directory. Only that variable is passed on, and only when set.
    if let Some(profile_file) = std::env::var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile_file);
    }
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let context = format!("{origin:?}\n--- stderr ---\n{stderr}\n--- stdout ---\n{stdout}");

    assert_eq!(output.status.code(), Some(1), "{context}");

    let error = format!("invalid CARDDEMO_CORS_ORIGIN: {reason}");
    assert_eq!(stderr, format!("api: state: {error}\n"), "{context}");

    let events: Vec<Json> = stdout
        .lines()
        .map(|line| {
            serde_json::from_str(line).unwrap_or_else(|parse| {
                panic!("stdout line {line:?} is not JSON: {parse}\n{context}")
            })
        })
        .collect();
    let failed: Vec<&Json> = events
        .iter()
        .filter(|event| event["fields"]["message"] == "api failed")
        .collect();
    let [event] = failed.as_slice() else {
        panic!("expected one `api failed` event: {context}");
    };
    assert_eq!(event["level"], "ERROR", "{context}");
    assert_eq!(event["fields"]["stage"], "state", "{context}");
    assert_eq!(event["fields"]["error"], error.as_str(), "{context}");

    for stream in [&stdout, &stderr] {
        assert!(!stream.contains(origin), "{context}");
        for secret in SECRETS {
            assert!(!stream.contains(secret), "{secret:?}: {context}");
        }
    }
}
