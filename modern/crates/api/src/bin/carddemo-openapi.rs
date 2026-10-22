//! `carddemo-openapi`: prints the `OpenAPI` document of the 27 `CardDemo` REST operations,
//! [`api::app::openapi`], as pretty-printed JSON followed by one newline on standard output.
//!
//! `scripts/regen-api.sh` writes that output to `modern/openapi/openapi.json`, generates the
//! frontend's `schema.d.ts` from it, and fails when either committed file differs, so standard
//! output carries the document and nothing else, and one build always prints the same bytes
//! (Generated API Types Only). `GET /api-docs/openapi.json`, which the running API adds after the
//! split, is not one of the document's operations.
//!
//! Usage: `carddemo-openapi [-h | --help]`. With no argument it prints the document. A single
//! `-h` or `--help` prints the usage on standard output instead. Any other command line, an
//! unknown argument or more than one argument, is refused: a fixed one-line message that never
//! repeats the input, then the usage, goes to standard error.
//!
//! The binary reads only its command-line arguments: no environment variable, no database or
//! broker connection and no migration, so it runs offline and under `env -i`. It installs no
//! tracing subscriber: the JSON subscriber of `runtime` writes to standard output and would
//! corrupt the document.
//!
//! Exit status: 0 once the document or the usage is written; 1 when the document cannot be
//! serialised or standard output cannot take the document or the usage, for example a closed
//! pipe, with a fixed one-line message on standard error saying which; 2 for a refused command
//! line.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Write as _};
use std::process::ExitCode;

/// The usage: on standard output for `-h` or `--help`, on standard error after a refusal.
const USAGE: &str = "\
Usage: carddemo-openapi [-h | --help]

Prints the OpenAPI document of the 27 CardDemo REST operations, built from the API handlers, as
pretty-printed JSON followed by one newline on standard output. scripts/regen-api.sh writes it
to modern/openapi/openapi.json. It reads no environment variable and opens no database or
broker connection.

Exit status:
  0  the document was written, or with -h or --help this usage
  1  the document could not be serialised, or standard output could not take the document or
     the usage (for example a closed pipe)
  2  usage error: an unexpected argument
";

fn main() -> ExitCode {
    // `args_os` rather than `args`, which panics on an argument that is not valid Unicode.
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => print_document(),
        [flag] if flag == "-h" || flag == "--help" => {
            write_stdout(format_args!("{USAGE}"), "the usage")
        }
        _ => {
            let mut err = io::stderr().lock();
            let _ = writeln!(
                err,
                "carddemo-openapi: no arguments are taken other than a single -h or --help"
            )
            .and_then(|()| err.write_all(USAGE.as_bytes()));
            ExitCode::from(2)
        }
    }
}

/// Prints the document as pretty-printed JSON followed by one newline.
fn print_document() -> ExitCode {
    let document = api::app::openapi();
    let Ok(json) = document.to_pretty_json() else {
        let _ = writeln!(
            io::stderr(),
            "carddemo-openapi: failed to serialise the OpenAPI document"
        );
        return ExitCode::FAILURE;
    };
    write_stdout(format_args!("{json}\n"), "the OpenAPI document")
}

/// Writes `text` to standard output and flushes it. On failure a fixed message naming `what`
/// goes to standard error and the exit status is 1.
fn write_stdout(text: fmt::Arguments<'_>, what: &str) -> ExitCode {
    // Written and flushed through `Write` rather than `println!`, which panics when standard
    // output is closed.
    let mut out = io::stdout().lock();
    if out.write_fmt(text).and_then(|()| out.flush()).is_err() {
        let _ = writeln!(
            io::stderr(),
            "carddemo-openapi: failed to write {what} to standard output"
        );
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
