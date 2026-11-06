//! R3 dependency boundary of `carddemo-domain`.
//!
//! The domain crate holds the business rules and the reward engine as pure code, so its
//! manifest names no runtime, database, HTTP, AMQP, serialization or PDF crate and no other
//! `carddemo-*` crate, in any dependency table, dev-dependencies and platform tables
//! included. Both the key of every dependency and the package it resolves to are checked.
//! A dependency inherited with `workspace = true` resolves through the root manifest's
//! `[workspace.dependencies]`, so a package renamed there is checked under its real name.
//!
//! Both manifests are read with `std` only, through the TOML reader in
//! `support/manifest_reader.rs`, which `crates/testkit/tests/hygiene.rs` includes as source
//! too: a TOML parser, or a shared test-support crate, would be one more dependency of the
//! crate this test guards.

#[path = "support/manifest_reader.rs"]
mod manifest_reader;

use manifest_reader::{DepKind, Table, declarations};

const MANIFEST: &str = include_str!("../Cargo.toml");

/// The root workspace manifest, through whose `[workspace.dependencies]` inherited
/// dependencies resolve.
const WORKSPACE_MANIFEST: &str = include_str!("../../../Cargo.toml");

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

/// A crate name a dependency table declares, either as a dependency key or as the package
/// that key resolves to, with the header of the table that declares it and its line.
#[derive(Debug)]
struct Declared {
    name: String,
    table: String,
    line: usize,
}

/// The header a dependency table is written under, such as `[dev-dependencies]` or
/// `[target.'cfg(unix)'.dependencies]`.
fn label(table: &Table) -> String {
    let kind = match table.kind {
        DepKind::Normal => "dependencies",
        DepKind::Dev => "dev-dependencies",
        DepKind::Build => "build-dependencies",
    };
    match &table.target {
        Some(platform) => format!("[target.'{platform}'.{kind}]"),
        None => format!("[{kind}]"),
    }
}

/// Every crate name the manifest's dependency tables declare: each dependency key and,
/// where it differs, the package the key resolves to. Keys of other tables, such as `name`
/// under `[package]`, are never reported. A manifest the reader cannot resolve is an error,
/// so the boundary check never passes a form it did not read.
fn declared_crates(manifest: &str, workspace_manifest: &str) -> Result<Vec<Declared>, String> {
    let declarations = declarations(manifest, workspace_manifest)
        .map_err(|error| format!("line {}: {}", error.line, error.message))?;
    let mut declared = Vec::new();
    for declaration in declarations {
        let Some(dependency) = declaration.dependency else {
            continue;
        };
        let table = label(&declaration.table);
        let mut names = vec![dependency.key];
        if dependency.package != names[0] {
            names.push(dependency.package);
        }
        declared.extend(names.into_iter().map(|name| Declared {
            name,
            table: table.clone(),
            line: declaration.line,
        }));
    }
    Ok(declared)
}

/// Whether the R3 boundary forbids `name`. Names are compared case-insensitively with `-`
/// and `_` folded, so a respelled name cannot slip through.
fn is_forbidden(name: &str) -> bool {
    let canonical = name.to_ascii_lowercase().replace('-', "_");
    canonical.starts_with(FORBIDDEN_PREFIX) || FORBIDDEN_CRATES.contains(&canonical.as_str())
}

/// The R3 violations of a domain manifest, one line per forbidden crate name. Panics when
/// the manifests cannot be read, or when the reader misses an entry the domain manifest is
/// known to declare, so a misread manifest cannot pass vacuously.
fn boundary_violations(manifest: &str, workspace_manifest: &str) -> Vec<String> {
    let declared = declared_crates(manifest, workspace_manifest)
        .unwrap_or_else(|error| panic!("the domain manifest cannot be checked: {error}"));
    for (name, table) in [
        ("rust_decimal", "[dependencies]"),
        ("chrono", "[dependencies]"),
        ("trybuild", "[dev-dependencies]"),
    ] {
        assert!(
            declared
                .iter()
                .any(|entry| entry.name == name && entry.table == table),
            "the manifest reader did not find `{name}` in {table}; it found {declared:?}"
        );
    }
    declared
        .iter()
        .filter(|entry| is_forbidden(&entry.name))
        .map(|entry| {
            format!(
                "line {}: carddemo-domain must not depend on `{}` (found in {})",
                entry.line, entry.name, entry.table
            )
        })
        .collect()
}

#[test]
fn domain_manifest_names_no_forbidden_crate() {
    let violations = boundary_violations(MANIFEST, WORKSPACE_MANIFEST);
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

#[test]
fn parser_reports_every_forbidden_form_and_nothing_else() {
    const WORKSPACE: &str = "[workspace.dependencies]\nsqlx = \"0.8\"\nchrono = \"0.4\"\n";
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
"t\u006fkio" = "1"
'axum' = "0.8"
pdf = { package = 'printpdf', version = "0.7" }

[dev-dependencies]
carddemo-testkit = { path = "../testkit" }

[target.'cfg(unix)'.dependencies]
tokio = "1"

[dependencies.foo]
package = "s\u0065rde"
version = "1"

[features]
default = ["dep:serde"]

[[bench]]
name = "carddemo-bench"
"#;

    let declared = declared_crates(SAMPLE, WORKSPACE).unwrap_or_else(|error| panic!("{error}"));
    let found: Vec<(&str, &str, usize)> = declared
        .iter()
        .filter(|entry| is_forbidden(&entry.name))
        .map(|entry| (entry.name.as_str(), entry.table.as_str(), entry.line))
        .collect();
    assert_eq!(
        found,
        [
            ("sqlx", "[dependencies]", 10),
            ("tokio", "[dependencies]", 14),
            ("axum", "[dependencies]", 15),
            ("printpdf", "[dependencies]", 16),
            ("carddemo-testkit", "[dev-dependencies]", 19),
            ("tokio", "[target.'cfg(unix)'.dependencies]", 22),
            ("serde", "[dependencies]", 24),
        ]
    );

    // Forms the reader cannot vouch for fail closed: a literal string keeps its backslash,
    // so it names no crate, and an inherited key the workspace does not declare resolves
    // to nothing.
    for unresolved in [
        "[dependencies]\n't\\u006fkio' = \"1\"\n",
        "[dependencies]\nwire = { package = 't\\u006fkio' }\n",
        "[dependencies]\nwire.workspace = true\n",
    ] {
        let error = declared_crates(unresolved, WORKSPACE).expect_err(unresolved);
        assert!(error.starts_with("line 2: "), "{error}");
    }
}

/// `text` with `insertion` placed right after the first `marker`. The marker must exist,
/// so a later manifest edit cannot turn a planted case into a vacuous one.
fn insert_after(text: &str, marker: &str, insertion: &str) -> String {
    assert!(
        text.contains(marker),
        "`{marker}` is missing, so the planted case would be vacuous"
    );
    text.replacen(marker, &format!("{marker}{insertion}"), 1)
}

/// The 1-based number of the line of `text` that reads `wanted`.
fn line_number(text: &str, wanted: &str) -> usize {
    text.lines()
        .position(|line| line == wanted)
        .map_or_else(|| panic!("no line reads `{wanted}`"), |at| at + 1)
}

#[test]
fn boundary_rejects_planted_dependencies() {
    let inherited = "wire.workspace = true";
    let inheriting = insert_after(MANIFEST, "[dev-dependencies]\n", &format!("{inherited}\n"));
    let renaming = insert_after(
        WORKSPACE_MANIFEST,
        "[workspace.dependencies]\n",
        "wire = { package = \"serde\", version = \"1\" }\n",
    );
    let dotted = format!("target.'cfg(unix)'.dependencies.sqlx = \"0.8\"\n{MANIFEST}");
    let cases = [
        // An allowed key inheriting a workspace dependency renamed to a forbidden package.
        (
            inheriting.as_str(),
            renaming.as_str(),
            format!(
                "line {}: carddemo-domain must not depend on `serde` (found in [dev-dependencies])",
                line_number(&inheriting, inherited)
            ),
        ),
        // A root-level dotted key declaring a platform dependency before any table header.
        (
            dotted.as_str(),
            WORKSPACE_MANIFEST,
            "line 1: carddemo-domain must not depend on `sqlx` (found in [target.'cfg(unix)'.dependencies])"
                .to_owned(),
        ),
    ];
    for (manifest, workspace_manifest, expected) in cases {
        assert_eq!(
            boundary_violations(manifest, workspace_manifest),
            [expected]
        );
    }
}
