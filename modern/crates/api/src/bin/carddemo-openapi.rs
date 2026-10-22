//! `carddemo-openapi`: prints the `OpenAPI` document of the 27 `CardDemo` REST operations,
//! [`api::app::openapi`], as pretty-printed JSON followed by one newline on standard output.
//!
//! `scripts/regen-api.sh` writes that output to `modern/openapi/openapi.json`, generates the
//! frontend's `schema.d.ts` from it, and fails when either committed file differs, so standard
//! output carries the document and nothing else, and one build always prints the same bytes
//! (Generated API Types Only). `GET /api-docs/openapi.json`, which the running API adds after the
//! split, is not one of the document's operations.
//!
//! The binary reads no environment variable, opens no database or broker connection and applies
//! no migration, so it runs offline and under an empty environment. It installs no tracing
//! subscriber: the JSON subscriber of `runtime` writes to standard output and would corrupt the
//! document.
//!
//! The exit status is non-zero when the document cannot be serialised or standard output cannot
//! take it, for example a closed pipe; a fixed one-line message on standard error then says
//! which.

use std::io::{self, Write as _};
use std::process::ExitCode;

fn main() -> ExitCode {
    let document = api::app::openapi();
    let Ok(json) = document.to_pretty_json() else {
        let _ = writeln!(
            io::stderr(),
            "carddemo-openapi: failed to serialise the OpenAPI document"
        );
        return ExitCode::FAILURE;
    };
    // Written and flushed through `Write` rather than `println!`, which panics when standard
    // output is closed.
    let mut out = io::stdout().lock();
    if writeln!(out, "{json}").and_then(|()| out.flush()).is_err() {
        let _ = writeln!(
            io::stderr(),
            "carddemo-openapi: failed to write the OpenAPI document to standard output"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
