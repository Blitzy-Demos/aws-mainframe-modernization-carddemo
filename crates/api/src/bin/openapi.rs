//! `openapi`: writes the `OpenAPI` document of the REST API to a file, the
//! input of the SPA's type generator.
//!
//! The document describes as REST resources the online transactions the
//! `CARDDEMO` CSD group defined [app/csd/CARDDEMO.CSD:L306-487]. It is
//! [`carddemo_api::spec_json`], written unchanged: no reformatting, and the one
//! trailing newline it ends with. Building it needs no database, no broker and
//! no settings, so this binary reads no environment variable and installs no
//! telemetry.
//!
//! # Usage
//!
//! ```text
//! cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json
//! pnpm --dir web gen:api
//! ```
//!
//! The first command writes the committed `web/openapi/openapi.json`, creating
//! missing parent directories; the second turns it into
//! `web/src/api/schema.d.ts`. `crates/api/tests/openapi_spec.rs` rebuilds the
//! document in memory and fails on any byte difference from the committed
//! file, so both commands run after every handler or DTO change.
//!
//! The only accepted arguments are `--out` followed by one non-empty path, and
//! `--help` or `-h` alone, which prints the `HELP` text.
//!
//! # Output and exit status
//!
//! Standard output carries only the help text; writing the document prints
//! nothing there. Failures are one line on standard error.
//!
//! | Status | Meaning |
//! |---|---|
//! | 0 | The document was written, or the help text was printed |
//! | 1 | A parent directory could not be created, the file could not be written, or the help text could not be printed |
//! | 2 | Usage error: any arguments other than `--out <path>` or a lone `--help` or `-h` |

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// The standard-error line of a usage error.
const USAGE: &str = "usage: openapi --out <path>";

/// The exit status of a usage error, distinct from an I/O failure's 1.
const USAGE_STATUS: u8 = 2;

/// The standard-output text of `--help` and `-h`.
const HELP: &str = "\
openapi: writes the OpenAPI 3.1 document of the CardDemo REST API to a file.

The document is carddemo_api::spec_json(), written unchanged and ending
with one newline. It is the input of the SPA's type generator, run as
pnpm --dir web gen:api.

usage: openapi --out <path>
       openapi --help

Options:
  --out <path>  The file to write. Missing parent directories are created;
                an existing file is replaced.
  -h, --help    Print this help.

No environment variable is read, and no database or broker is needed.
Writing the document prints nothing on standard output; a failure is one
line on standard error.

Exit status:
  0  The document was written, or this help was printed.
  1  A parent directory could not be created, the file could not be written,
     or this help could not be printed.
  2  Usage error: any other arguments.

Example, from the repository root, after a handler or DTO change:
  cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json
  pnpm --dir web gen:api
";

/// What a well-formed command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Request {
    /// `--out <path>`: write the document to the path.
    Write(PathBuf),
    /// `--help` or `-h`: print [`HELP`] to standard output.
    Help,
}

fn main() -> ExitCode {
    let result = match parse_args(env::args_os().skip(1)) {
        Some(Request::Write(path)) => write_document(&path, &carddemo_api::spec_json()),
        Some(Request::Help) => write_help(&mut io::stdout().lock()),
        None => {
            eprintln!("{USAGE}");
            return ExitCode::from(USAGE_STATUS);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// The request `args`, the command line without the program name, makes:
/// [`Request::Help`] when it is exactly `--help` or `-h`, [`Request::Write`]
/// when it is exactly `--out` followed by one path, and `None` for anything
/// else, including the `--out=<path>` form and a help flag beside any other
/// argument.
///
/// An empty path is rejected here as a missing value, so it is reported as a
/// usage error rather than as a failure to write a file named "". The value
/// after `--out` is always a path, so `--out --help` writes a file named
/// `--help`.
fn parse_args(mut args: impl Iterator<Item = OsString>) -> Option<Request> {
    match (args.next(), args.next(), args.next()) {
        (Some(flag), None, None) if flag == "--help" || flag == "-h" => Some(Request::Help),
        (Some(flag), Some(path), None) if flag == "--out" && !path.is_empty() => {
            Some(Request::Write(PathBuf::from(path)))
        }
        _ => None,
    }
}

/// Writes [`HELP`] to `out` and flushes it.
///
/// # Errors
///
/// The standard-error line naming the help text, with the I/O error, for
/// example when standard output is a full device or a closed pipe.
fn write_help(out: &mut impl Write) -> Result<(), String> {
    out.write_all(HELP.as_bytes())
        .and_then(|()| out.flush())
        .map_err(|err| format!("openapi: cannot write the help text: {err}"))
}

/// Writes `document` to `path`, first creating its missing parent directories.
///
/// A path with no directory part, such as `openapi.json`, has the empty path as
/// its parent; that names the current directory, which exists, so nothing is
/// created for it.
///
/// # Errors
///
/// The standard-error line naming the directory that could not be created or
/// the file that could not be written, with the I/O error.
fn write_document(path: &Path, document: &str) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .map_err(|err| format!("openapi: cannot create {}: {err}", parent.display()))?;
    }
    fs::write(path, document)
        .map_err(|err| format!("openapi: cannot write {}: {err}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::process;

    use super::{HELP, Request, USAGE, parse_args, write_document, write_help};

    fn parse(args: &[&str]) -> Option<Request> {
        parse_args(args.iter().map(OsString::from))
    }

    /// A writer that refuses every byte, as a full device does.
    struct Full;

    impl Write for Full {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::StorageFull))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// A directory of its own under the system temporary directory, removed
    /// when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("openapi-bin-{}-{name}", process::id()));
            if dir.exists() {
                fs::remove_dir_all(&dir).expect("a stale scratch directory is removable");
            }
            fs::create_dir_all(&dir).expect("the scratch directory is creatable");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            // A directory that cannot be removed fails the test with its path and
            // the I/O error, the only report libtest shows for a passing test.
            // While the test is already unwinding, a second panic would abort the
            // process and hide the first failure, so the error goes to standard
            // error instead.
            if let Err(err) = fs::remove_dir_all(&self.0) {
                let message = format!(
                    "the scratch directory {} cannot be removed: {err}",
                    self.0.display()
                );
                if std::thread::panicking() {
                    eprintln!("{message}");
                } else {
                    panic!("{message}");
                }
            }
        }
    }

    #[test]
    fn out_followed_by_one_path_is_accepted() {
        for (args, expected) in [
            (
                ["--out", "web/openapi/openapi.json"],
                "web/openapi/openapi.json",
            ),
            (["--out", "openapi.json"], "openapi.json"),
            (["--out", "/abs/dir/openapi.json"], "/abs/dir/openapi.json"),
            (["--out", "--help"], "--help"),
            (["--out", "-h"], "-h"),
        ] {
            assert_eq!(
                parse(&args),
                Some(Request::Write(PathBuf::from(expected))),
                "{args:?}"
            );
        }
    }

    #[test]
    fn a_help_flag_alone_asks_for_the_help_text() {
        for flag in ["--help", "-h"] {
            assert_eq!(parse(&[flag]), Some(Request::Help), "{flag}");
        }
    }

    #[test]
    fn any_other_command_line_is_a_usage_error() {
        let cases: [&[&str]; 21] = [
            &[],
            &["--out"],
            &["--out", ""],
            &["--out", "a", "b"],
            &["--out", "a", "--out", "b"],
            &["--out=web/openapi/openapi.json"],
            &["--output", "openapi.json"],
            &["openapi.json"],
            &["openapi.json", "--out"],
            &["-o", "openapi.json"],
            &["--version"],
            &["-V"],
            &["help"],
            &["-help"],
            &["--HELP"],
            &["--help=yes"],
            &["--help", "-h"],
            &["--help", "openapi.json"],
            &["-h", "--out", "openapi.json"],
            &["--out", "openapi.json", "--help"],
            &["--out", "openapi.json", "-h"],
        ];
        for args in cases {
            assert_eq!(parse(args), None, "{args:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_path_is_accepted() {
        use std::os::unix::ffi::OsStringExt;

        let path = OsString::from_vec(b"spec-\xff.json".to_vec());
        let args = [OsString::from("--out"), path.clone()];
        assert_eq!(
            parse_args(args.into_iter()),
            Some(Request::Write(PathBuf::from(path)))
        );
    }

    #[test]
    fn the_help_text_names_the_usage_the_help_flag_the_statuses_and_the_example() {
        for needle in [
            "cargo run -p carddemo-api --bin openapi -- --out web/openapi/openapi.json\n",
            "pnpm --dir web gen:api\n",
            USAGE,
            "-h, --help",
            "openapi --help",
            "\n  0  ",
            "\n  1  ",
            "\n  2  ",
        ] {
            assert!(HELP.contains(needle), "{needle:?} is missing from:\n{HELP}");
        }
        assert!(
            HELP.ends_with('\n') && !HELP.ends_with("\n\n"),
            "the help text ends with one newline"
        );
        for line in HELP.lines() {
            assert!(line.len() <= 80, "{line:?} is wider than 80 columns");
        }
    }

    #[test]
    fn the_help_text_is_written_whole_or_its_failure_is_one_line() {
        let mut out = Vec::new();
        write_help(&mut out).expect("the help text is written to memory");
        assert_eq!(out, HELP.as_bytes());

        let message = write_help(&mut Full).expect_err("a full device refuses the help text");
        assert_eq!(
            message,
            format!(
                "openapi: cannot write the help text: {}",
                io::Error::from(io::ErrorKind::StorageFull)
            )
        );
    }

    #[test]
    fn missing_parent_directories_are_created_and_the_bytes_are_unchanged() {
        let scratch = Scratch::new("nested");
        let path = scratch.path().join("nested/dir/openapi.json");
        let document = "{\n  \"openapi\": \"3.1.0\"\n}\n";

        write_document(&path, document).expect("the document is written");

        assert_eq!(
            fs::read(&path).expect("the file is readable"),
            document.as_bytes()
        );
    }

    #[test]
    fn an_existing_file_is_replaced() {
        let scratch = Scratch::new("replace");
        let path = scratch.path().join("openapi.json");
        fs::write(&path, "an older and longer document\n").expect("the old file is written");

        write_document(&path, "{}\n").expect("the document is written");

        assert_eq!(fs::read(&path).expect("the file is readable"), b"{}\n");
    }

    #[test]
    fn a_parent_that_is_a_regular_file_fails_with_the_directory_named() {
        let scratch = Scratch::new("blocked");
        let blocker = scratch.path().join("regular-file");
        fs::write(&blocker, "").expect("the blocking file is written");
        let parent = blocker.join("dir");

        let message = write_document(&parent.join("openapi.json"), "{}\n")
            .expect_err("a directory cannot be created under a regular file");

        let prefix = format!("openapi: cannot create {}: ", parent.display());
        assert!(message.starts_with(&prefix), "{message}");
    }

    #[test]
    fn a_path_that_is_a_directory_fails_with_the_file_named() {
        let scratch = Scratch::new("directory");
        let path = scratch.path().join("openapi.json");
        fs::create_dir(&path).expect("the directory is created");

        let message =
            write_document(&path, "{}\n").expect_err("a directory cannot be written as a file");

        let prefix = format!("openapi: cannot write {}: ", path.display());
        assert!(message.starts_with(&prefix), "{message}");
    }
}
