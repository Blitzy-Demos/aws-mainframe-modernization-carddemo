//! Coverage gate of the parity oracle audit (AAP 0.9.1, "After implementation: coverage").
//!
//! Every data row of `docs/parity/parity-oracle-audit.md`, sections A to F, defines a
//! `P-<KEY>-<nnn>` ID, and the test that proves the row cites that ID literally. This test
//! fails when an audit ID is cited by no test source, or when a test source cites an ID the
//! audit does not define. Layout rows (section A) count like every other row, so the codec and
//! persistence suites cite theirs too.
//!
//! The test sources are the `.rs` files, at any depth (`common/`, `compile_fail/`, `support/`
//! and every other sub-folder), under the `tests/` directory of each workspace crate in
//! `crates/`, except `crates/audit`. The parser's own tests hold made-up and sample IDs to
//! exercise the parser, so their lines prove nothing about the parity tests and would report
//! IDs the audit never defined. Source files outside `tests/` are not read: ported functions
//! name their audit row in their doc comments, and those names are not tests.
//!
//! It needs no container, database or async runtime: only `std`, [`carddemo_audit`] and the
//! repository root from [`carddemo_testkit::fixtures`]. Run it alone with
//! `cargo test -p carddemo-batch --test parity_audit_coverage`.
//!
//! This file is left out of the scan and holds no complete ID, not even in comments, so it can
//! neither prove nor hide a citation; `this_file_cites_no_parity_id` keeps it that way.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use carddemo_audit::{
    Coverage, IdKind, PARITY_AUDIT_PATH, SourceLocation, coverage, ids_with_prefix, scan_cited_ids,
};
use carddemo_testkit::fixtures;

/// The directory, relative to the repository root, that holds one folder per workspace crate.
const CRATES: &str = "crates";

/// The folder of each crate whose `.rs` files are test sources.
const TESTS: &str = "tests";

/// The crate folder left out of the scan: `crates/audit`, whose tests hold sample IDs, as the
/// `carddemo-audit` contract for [`scan_cited_ids`] requires.
const AUDIT_CRATE: &str = "audit";

/// This file, relative to the repository root. Its lines never count as citations, because a
/// gate satisfied by its own text would prove nothing about the parity tests.
const THIS_FILE: &str = "crates/batch/tests/parity_audit_coverage.rs";

/// The prefix every parity audit ID starts with, followed by its key and row number.
const PREFIX: &str = "P-";

/// The last line of every coverage failure.
const REMEDY: &str =
    "cite the P-ID as a string literal in the test that proves it, or correct the ID";

/// The P-IDs the parity audit defines, review-log rows excluded.
///
/// Panics when the audit cannot be loaded or defines no P-ID, so a missing or misparsed audit
/// cannot pass vacuously.
fn defined_ids(root: &Path) -> BTreeSet<String> {
    let defined = ids_with_prefix(&common::parity(), IdKind::Parity);
    assert!(
        !defined.is_empty(),
        "{PARITY_AUDIT_PATH} defines no P-ID under {}: an audit without data rows cannot \
         prove coverage",
        root.display()
    );
    defined
}

/// The `tests/` directory of every crate folder under [`CRATES`] that has one, in name order,
/// [`AUDIT_CRATE`] excluded.
///
/// Panics when [`CRATES`] cannot be listed.
fn test_dirs(root: &Path) -> Vec<PathBuf> {
    let crates = root.join(CRATES);
    let mut entries = fs::read_dir(&crates)
        .and_then(Iterator::collect::<io::Result<Vec<fs::DirEntry>>>)
        .unwrap_or_else(|error| panic!("{} cannot be listed: {error}", crates.display()));
    entries.sort_by_key(fs::DirEntry::file_name);
    entries
        .into_iter()
        .filter(|entry| entry.file_name() != AUDIT_CRATE)
        .map(|entry| entry.path().join(TESTS))
        .filter(|tests| tests.is_dir())
        .collect()
}

/// Every P-ID cited by the test sources of [`test_dirs`], with each line citing it. Lines of
/// [`THIS_FILE`] are dropped, and an ID cited only there is not cited at all.
///
/// Panics when a directory or file cannot be read, or when no test source cites any P-ID, so a
/// scan that reads nothing fails as such rather than as a list of every audit ID.
fn cited_ids(root: &Path) -> BTreeMap<String, Vec<SourceLocation>> {
    let dirs = test_dirs(root);
    let entries: Vec<&Path> = dirs.iter().map(PathBuf::as_path).collect();
    let this_file = root.join(THIS_FILE);
    // The library's own walk, not a second one here, so every coverage gate reads test sources
    // by one rule set: recursive, `.rs` only (so `snapshots/` and `fixtures/` contribute
    // nothing), `target` and hidden folders skipped, symbolic links not followed.
    let cited: BTreeMap<String, Vec<SourceLocation>> = scan_cited_ids(&entries, IdKind::Parity)
        .unwrap_or_else(|error| {
            panic!("the test sources under {CRATES} cannot be scanned: {error}")
        })
        .into_iter()
        .filter_map(|(id, locations)| {
            let elsewhere: Vec<SourceLocation> = locations
                .into_iter()
                .filter(|location| location.path != this_file)
                .collect();
            (!elsewhere.is_empty()).then_some((id, elsewhere))
        })
        .collect();
    assert!(
        !cited.is_empty(),
        "no test source under {} cites a P-ID: the scanned directories were {:?}",
        root.join(CRATES).display(),
        dirs.iter()
            .map(|dir| relative(root, dir))
            .collect::<Vec<_>>()
    );
    cited
}

/// The `<KEY>` segment of a `P-<KEY>-<nnn>` ID: the text between the prefix and the row
/// number.
fn key(id: &str) -> &str {
    id.strip_prefix(PREFIX)
        .and_then(|rest| rest.rsplit_once('-'))
        .map_or(id, |(name, _)| name)
}

/// `path` relative to `root`, or as it stands when it lies outside `root`.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The failure message of an incomplete coverage `gaps`: the totals, the uncited IDs grouped by
/// key, the unknown IDs with every line citing them, then [`REMEDY`].
fn report(gaps: &Coverage, defined: usize, cited: usize, root: &Path) -> String {
    let mut by_key: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in &gaps.uncited {
        by_key.entry(key(id)).or_default().push(id);
    }
    let mut lines = vec![
        format!(
            "{PARITY_AUDIT_PATH}: P-ID coverage by the test sources of {CRATES}/*/{TESTS} is incomplete"
        ),
        format!("defined P-IDs: {defined}; cited P-IDs: {cited}"),
        format!(
            "uncited P-IDs ({}) in {} key(s):",
            gaps.uncited.len(),
            by_key.len()
        ),
    ];
    for (name, ids) in &by_key {
        lines.push(format!("  {name} ({}): {}", ids.len(), ids.join(", ")));
    }
    lines.push(format!(
        "unknown P-IDs ({}), not defined by {PARITY_AUDIT_PATH}:",
        gaps.unknown.len()
    ));
    for (id, locations) in &gaps.unknown {
        let at: Vec<String> = locations
            .iter()
            .map(|location| format!("{}:{}", relative(root, &location.path), location.line))
            .collect();
        lines.push(format!("  {id} at {}", at.join(", ")));
    }
    lines.push(REMEDY.to_owned());
    lines.join("\n")
}

/// Every P-ID of the parity audit is cited by a test, and every P-ID a test cites is defined
/// by the audit.
#[test]
fn every_parity_id_is_cited_and_every_cited_id_exists() {
    let root = fixtures::repo_root();
    let defined = defined_ids(&root);
    let cited = cited_ids(&root);
    let gaps = coverage(&defined, &cited);
    assert!(
        gaps.is_complete(),
        "{}",
        report(&gaps, defined.len(), cited.len(), &root)
    );
}

/// [`THIS_FILE`] names this file, and this file holds no complete P-ID, so leaving it out of
/// the scan hides no citation.
#[test]
fn this_file_cites_no_parity_id() {
    assert!(
        Path::new(file!()).ends_with(THIS_FILE),
        "THIS_FILE is {THIS_FILE}, but this file is {}",
        file!()
    );
    let this_file = fixtures::repo_root().join(THIS_FILE);
    let cited = scan_cited_ids(&[this_file.as_path()], IdKind::Parity)
        .unwrap_or_else(|error| panic!("{THIS_FILE} cannot be scanned: {error}"));
    assert!(
        cited.is_empty(),
        "{THIS_FILE} must hold no complete P-ID, found {:?}; cite it in the test that proves it",
        cited.keys().collect::<Vec<_>>()
    );
}
