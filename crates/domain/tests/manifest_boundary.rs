//! R3 dependency boundary of `carddemo-domain`.
//!
//! The domain crate holds the business rules and the reward engine as pure code, so its
//! manifest names no runtime, database, HTTP, AMQP, serialization or PDF crate and no other
//! `carddemo-*` crate, in any dependency table, dev-dependencies included. The manifest is
//! read with `std` only: a TOML parser would be one more dependency of the crate this test
//! guards.

const MANIFEST: &str = include_str!("../Cargo.toml");

/// Crates the domain may not name, in canonical form (lower case, `-` folded to `_`).
const FORBIDDEN_CRATES: [&str; 7] = [
    "sqlx",
    "tokio",
    "axum",
    "lapin",
    "serde",
    "serde_json",
    "printpdf",
];

/// Prefix of every workspace crate, in canonical form, so it covers `carddemo-` and
/// `carddemo_` alike.
const FORBIDDEN_PREFIX: &str = "carddemo_";

/// Table names whose keys are dependencies. Cargo still reads the underscore spellings.
const DEPENDENCY_TABLES: [&str; 5] = [
    "dependencies",
    "dev-dependencies",
    "build-dependencies",
    "dev_dependencies",
    "build_dependencies",
];

/// A crate name a dependency table declares, either as a dependency key or as the
/// `package` a key renames, with the header of the table that declares it.
#[derive(Debug)]
struct Declared {
    name: String,
    table: String,
}

/// The table the statements after a header belong to.
enum Section {
    /// A dependency table such as `[dependencies]` or `[target.'cfg(unix)'.dependencies]`.
    Table { label: String },
    /// A dependency sub-table such as `[dependencies.foo]`, whose body may rename the package.
    SubTable { label: String, name: String },
    /// Any other table: its keys never name a dependency.
    Other,
}

/// Lexical state of [`Splitter`].
#[derive(Clone, Copy, Default)]
enum Lex {
    #[default]
    Code,
    Comment,
    /// Inside a string delimited by `quote`, triple-quoted when `multi`.
    Str {
        quote: char,
        multi: bool,
    },
}

/// Splits a manifest into logical statements, one per table header or `key = value` pair,
/// with comments removed and values that span lines joined onto one line.
#[derive(Default)]
struct Splitter {
    statements: Vec<String>,
    current: String,
    lex: Lex,
    /// Open brackets and braces outside strings; a newline inside them continues the value.
    depth: usize,
}

impl Splitter {
    fn split(manifest: &str) -> Vec<String> {
        let mut splitter = Self::default();
        let mut rest = manifest;
        while let Some(c) = rest.chars().next() {
            let consumed = splitter.feed(rest, c);
            rest = &rest[consumed..];
        }
        splitter.flush();
        splitter.statements
    }

    /// Consumes the token that starts `rest` with the character `c`, returning its length
    /// in bytes.
    fn feed(&mut self, rest: &str, c: char) -> usize {
        match self.lex {
            Lex::Comment => {
                if c == '\n' {
                    self.lex = Lex::Code;
                    self.end_line();
                }
                c.len_utf8()
            }
            Lex::Code => self.code(rest, c),
            Lex::Str { quote, multi } => self.string(rest, c, quote, multi),
        }
    }

    fn code(&mut self, rest: &str, c: char) -> usize {
        match c {
            '#' => {
                self.lex = Lex::Comment;
                1
            }
            '\n' => {
                self.end_line();
                1
            }
            '"' | '\'' => {
                let multi = quote_run(rest, c) >= 3;
                self.lex = Lex::Str { quote: c, multi };
                self.keep(rest, if multi { 3 } else { 1 })
            }
            '[' | '{' => {
                self.depth += 1;
                self.keep(rest, 1)
            }
            ']' | '}' => {
                self.depth = self.depth.saturating_sub(1);
                self.keep(rest, 1)
            }
            _ => self.keep(rest, c.len_utf8()),
        }
    }

    fn string(&mut self, rest: &str, c: char, quote: char, multi: bool) -> usize {
        if c == '\\' && quote == '"' {
            // The escaped character stays inside the string, so `\"` cannot close it.
            let escaped = rest[1..].chars().next().map_or(0, char::len_utf8);
            return self.keep(rest, 1 + escaped);
        }
        if c == quote {
            let run = quote_run(rest, quote);
            if !multi || run >= 3 {
                self.lex = Lex::Code;
                // A triple-quoted string may end with up to two content quotes before its
                // closing delimiter.
                return self.keep(rest, if multi { run.min(5) } else { 1 });
            }
        }
        self.keep(rest, c.len_utf8())
    }

    fn keep(&mut self, rest: &str, len: usize) -> usize {
        self.current.push_str(&rest[..len]);
        len
    }

    fn end_line(&mut self) {
        if self.depth == 0 {
            self.flush();
        } else {
            self.current.push(' ');
        }
    }

    fn flush(&mut self) {
        let statement = self.current.trim();
        if !statement.is_empty() {
            self.statements.push(statement.to_owned());
        }
        self.current.clear();
    }
}

/// Number of consecutive `quote` characters that start `text`.
fn quote_run(text: &str, quote: char) -> usize {
    text.chars().take_while(|&c| c == quote).count()
}

/// Byte offset of the first `target` outside strings and outside nested brackets or braces.
fn find_top_level(text: &str, target: char) -> Option<usize> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut depth = 0_usize;
    text.char_indices().find_map(|(at, c)| {
        if let Some(open) = quote {
            if c == open && !escaped {
                quote = None;
            }
            escaped = open == '"' && c == '\\' && !escaped;
            return None;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '[' | '{' => depth += 1,
            ']' | '}' => depth = depth.saturating_sub(1),
            _ if c == target && depth == 0 => return Some(at),
            _ => {}
        }
        None
    })
}

/// Splits `text` at every top-level `separator`.
fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = find_top_level(rest, separator) {
        parts.push(&rest[..at]);
        rest = &rest[at + separator.len_utf8()..];
    }
    parts.push(rest);
    parts
}

/// Removes surrounding whitespace and one pair of matching quotes.
fn unquote(text: &str) -> &str {
    let text = text.trim();
    ['"', '\'']
        .into_iter()
        .find_map(|quote| text.strip_prefix(quote)?.strip_suffix(quote))
        .unwrap_or(text)
}

/// Segments of a dotted key, unquoted: `target.'cfg(unix)'.dependencies` gives `target`,
/// `cfg(unix)` and `dependencies`. Dots inside quoted segments do not split.
fn key_segments(key: &str) -> Vec<&str> {
    split_top_level(key, '.').into_iter().map(unquote).collect()
}

/// The key and value of a `key = value` statement.
fn split_key_value(statement: &str) -> Option<(&str, &str)> {
    let at = find_top_level(statement, '=')?;
    Some((statement[..at].trim(), statement[at + 1..].trim()))
}

/// The dotted key inside a `[table]` or `[[array-of-tables]]` header, or `None` when the
/// statement is not a header.
fn table_header(statement: &str) -> Option<&str> {
    let inner = statement.strip_prefix('[')?.strip_suffix(']')?;
    let inner = inner
        .strip_prefix('[')
        .and_then(|nested| nested.strip_suffix(']'))
        .unwrap_or(inner);
    Some(inner.trim())
}

fn section_of(header: &str) -> Section {
    let segments = key_segments(header);
    // `[target.<triple or cfg>.<kind>]` scopes a dependency table to a platform.
    let local = match segments.as_slice() {
        ["target", _, scoped @ ..] => scoped,
        all => all,
    };
    let label = format!("[{header}]");
    match local {
        [kind] if DEPENDENCY_TABLES.contains(kind) => Section::Table { label },
        [kind, name] if DEPENDENCY_TABLES.contains(kind) => Section::SubTable {
            label,
            name: (*name).to_owned(),
        },
        _ => Section::Other,
    }
}

/// The `package` an inline dependency table renames, as in `foo = { package = "serde" }`.
fn inline_package(value: &str) -> Option<&str> {
    let body = value.trim().strip_prefix('{')?.strip_suffix('}')?;
    split_top_level(body, ',').into_iter().find_map(|entry| {
        let (key, value) = split_key_value(entry)?;
        (key_segments(key) == ["package"]).then(|| unquote(value))
    })
}

fn push(declared: &mut Vec<Declared>, name: &str, table: &str) {
    declared.push(Declared {
        name: name.to_owned(),
        table: table.to_owned(),
    });
}

/// Records the dependency a `key = value` line of a dependency table declares, and the
/// package it renames, in the plain (`foo = "1"`), dotted (`foo.workspace = true`,
/// `foo.package = "x"`) and inline-table (`foo = { package = "x" }`) forms.
fn record_entry(declared: &mut Vec<Declared>, key: &str, value: &str, table: &str) {
    let segments = key_segments(key);
    let Some((&name, rest)) = segments.split_first() else {
        return;
    };
    push(declared, name, table);
    let package = match rest {
        ["package"] => Some(unquote(value)),
        _ => inline_package(value),
    };
    if let Some(package) = package {
        push(declared, package, table);
    }
}

/// Every crate name the manifest's dependency tables declare. Keys of other tables, such as
/// `name` under `[package]`, are never reported.
fn declared_crates(manifest: &str) -> Vec<Declared> {
    let mut declared = Vec::new();
    let mut section = Section::Other;
    for statement in Splitter::split(manifest) {
        if let Some(header) = table_header(&statement) {
            section = section_of(header);
            if let Section::SubTable { label, name } = &section {
                push(&mut declared, name, label);
            }
        } else if let Some((key, value)) = split_key_value(&statement) {
            match &section {
                Section::Table { label } => record_entry(&mut declared, key, value, label),
                Section::SubTable { label, .. } => {
                    if key_segments(key) == ["package"] {
                        push(&mut declared, unquote(value), label);
                    }
                }
                Section::Other => {}
            }
        }
    }
    declared
}

/// Whether the R3 boundary forbids `name`. Names are compared case-insensitively with `-`
/// and `_` folded, so a respelled name cannot slip through.
fn is_forbidden(name: &str) -> bool {
    let canonical = name.to_ascii_lowercase().replace('-', "_");
    canonical.starts_with(FORBIDDEN_PREFIX) || FORBIDDEN_CRATES.contains(&canonical.as_str())
}

/// Whether `name` has the shape of a crate name. A declared name of any other shape means
/// the parser met a form it does not resolve, so the boundary test fails rather than pass
/// it unchecked.
fn is_crate_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn forbidden(declared: &[Declared]) -> Vec<&Declared> {
    declared
        .iter()
        .filter(|entry| is_forbidden(&entry.name))
        .collect()
}

#[test]
fn domain_manifest_names_no_forbidden_crate() {
    let declared = declared_crates(MANIFEST);

    // A parser that reads nothing must not pass vacuously.
    for (name, table) in [
        ("rust_decimal", "[dependencies]"),
        ("chrono", "[dependencies]"),
        ("trybuild", "[dev-dependencies]"),
    ] {
        assert!(
            declared
                .iter()
                .any(|entry| entry.name == name && entry.table == table),
            "the manifest parser did not find `{name}` in {table}; it found {declared:?}"
        );
    }

    for entry in &declared {
        assert!(
            is_crate_name(&entry.name),
            "unresolved dependency key `{}` in {}: the boundary check cannot vouch for it",
            entry.name,
            entry.table
        );
    }

    let violations: Vec<String> = forbidden(&declared)
        .iter()
        .map(|entry| {
            format!(
                "carddemo-domain must not depend on `{}` (found in {})",
                entry.name, entry.table
            )
        })
        .collect();
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn parser_reports_every_forbidden_form_and_nothing_else() {
    const SAMPLE: &str = r#"
[package]
name = "carddemo-x"
description = """
[dependencies]
serde = "1"
"""

[dependencies]   # trailing comment
sqlx.workspace = true
chrono = { workspace = true, features = [
    "serde",  # a feature name, not a dependency
] }

[dev-dependencies]
carddemo-testkit = { path = "../testkit" }

[target.'cfg(unix)'.dependencies]
tokio = "1"

[dependencies.foo]
package = "serde"
version = "1"

[features]
default = ["dep:serde"]

[[bench]]
name = "carddemo-bench"
"#;

    let declared = declared_crates(SAMPLE);
    let found: Vec<(&str, &str)> = forbidden(&declared)
        .iter()
        .map(|entry| (entry.name.as_str(), entry.table.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            ("sqlx", "[dependencies]"),
            ("carddemo-testkit", "[dev-dependencies]"),
            ("tokio", "[target.'cfg(unix)'.dependencies]"),
            ("serde", "[dependencies.foo]"),
        ]
    );
}
