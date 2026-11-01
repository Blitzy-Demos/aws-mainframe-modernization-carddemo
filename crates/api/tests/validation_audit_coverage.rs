//! Coverage gate of the input-validation audit (AAP 0.9.1, 0.10.1 "Errors").
//!
//! Every data row of `docs/parity/validation-audit.md` defines a `V-<PROGRAM>-<nnn>` ID, and the
//! error test that sends the row's failing input cites that ID as a string literal. This test
//! fails when an audit ID is cited by no test source under `crates/api/tests`, `common/mod.rs`
//! included, or when a test source there cites an ID the audit does not define. Rows whose
//! message is `no-message (E-nn)` are IDs like any other and need a citation too.
//!
//! It needs no container, database or async runtime: only `std`, [`carddemo_audit`] and the
//! repository root from [`carddemo_testkit::fixtures`]. Run it alone with
//! `cargo test -p carddemo-api --test validation_audit_coverage`.
//!
//! This file is left out of the scan and holds no complete ID, not even in comments, so it can
//! neither prove nor hide a citation; `this_file_cites_no_validation_id` keeps it that way.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use carddemo_audit::{
    Coverage, IdKind, Repo, SourceLocation, VALIDATION_AUDIT_PATH, coverage, ids_with_prefix,
    scan_cited_ids,
};
use carddemo_testkit::fixtures::repo_root;

/// The directory, relative to the repository root, whose test sources cite the V-IDs.
const API_TESTS: &str = "crates/api/tests";

/// This file, relative to the repository root. Its lines never count as citations, because a
/// gate satisfied by its own text would prove nothing about the error tests.
const THIS_FILE: &str = "crates/api/tests/validation_audit_coverage.rs";

/// The last line of every coverage failure.
const REMEDY: &str =
    "cite the V-ID as a string literal in the test that proves it, or correct the ID";

/// The V-IDs the validation audit defines, review-log rows excluded.
///
/// Panics when the audit cannot be loaded or defines no V-ID, so a missing or misparsed audit
/// cannot pass vacuously.
fn defined_ids(repo: &Repo) -> BTreeSet<String> {
    let doc = repo
        .validation_audit()
        .unwrap_or_else(|error| panic!("the validation audit cannot be loaded: {error}"));
    let defined = ids_with_prefix(&doc, IdKind::Validation);
    assert!(
        !defined.is_empty(),
        "{VALIDATION_AUDIT_PATH} defines no V-ID under {}: an audit without data rows cannot \
         prove coverage",
        repo.root.display()
    );
    defined
}

/// Every V-ID cited by the `.rs` files under [`API_TESTS`], at any depth, with each line citing
/// it. Lines of [`THIS_FILE`] are dropped, and an ID cited only there is not cited at all.
///
/// Panics when the directory or one of its files cannot be read.
fn cited_ids(root: &Path) -> BTreeMap<String, Vec<SourceLocation>> {
    let tests = root.join(API_TESTS);
    let this_file = root.join(THIS_FILE);
    // The library's own walk, not a second one here, so every coverage gate reads test sources
    // by one rule set: recursive, `.rs` only, symbolic links not followed.
    scan_cited_ids(&[tests.as_path()], IdKind::Validation)
        .unwrap_or_else(|error| panic!("{API_TESTS} cannot be scanned: {error}"))
        .into_iter()
        .filter_map(|(id, locations)| {
            let elsewhere: Vec<SourceLocation> = locations
                .into_iter()
                .filter(|location| location.path != this_file)
                .collect();
            (!elsewhere.is_empty()).then_some((id, elsewhere))
        })
        .collect()
}

/// The `<PROGRAM>` segment of a `V-<PROGRAM>-<nnn>` ID: the text between the prefix and the row
/// number.
fn program(id: &str) -> &str {
    id.split_once('-')
        .and_then(|(_, rest)| rest.rsplit_once('-'))
        .map_or(id, |(name, _)| name)
}

/// `location` as `<path>:<line>`, the path relative to `root`.
fn located(root: &Path, location: &SourceLocation) -> String {
    let path = location.path.strip_prefix(root).unwrap_or(&location.path);
    format!("{}:{}", path.display(), location.line)
}

/// The failure message of an incomplete coverage `gaps`: the uncited IDs grouped by program, the
/// unknown IDs with every line citing them, then [`REMEDY`].
fn report(gaps: &Coverage, root: &Path) -> String {
    let mut by_program: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for id in &gaps.uncited {
        by_program.entry(program(id)).or_default().push(id);
    }
    let mut lines = vec![
        format!("{VALIDATION_AUDIT_PATH}: V-ID coverage by {API_TESTS} is incomplete"),
        format!(
            "uncited V-IDs ({}) in {} program(s):",
            gaps.uncited.len(),
            by_program.len()
        ),
    ];
    for (name, ids) in &by_program {
        lines.push(format!("  {name} ({}): {}", ids.len(), ids.join(", ")));
    }
    lines.push(format!(
        "unknown V-IDs ({}), not defined by {VALIDATION_AUDIT_PATH}:",
        gaps.unknown.len()
    ));
    for (id, locations) in &gaps.unknown {
        let at: Vec<String> = locations
            .iter()
            .map(|location| located(root, location))
            .collect();
        lines.push(format!("  {id} at {}", at.join(", ")));
    }
    lines.push(REMEDY.to_owned());
    lines.join("\n")
}

/// Every V-ID of the validation audit is cited by an API test, and every V-ID an API test cites
/// is defined by the audit.
#[test]
fn every_validation_id_is_cited_and_every_cited_id_exists() {
    let root = repo_root();
    let defined = defined_ids(&Repo::new(&root));
    let cited = cited_ids(&root);
    let gaps = coverage(&defined, &cited);
    assert!(gaps.is_complete(), "{}", report(&gaps, &root));
}

/// This file holds no complete V-ID, so leaving it out of the scan hides no citation; the scan
/// also fails if [`THIS_FILE`] no longer names this file.
#[test]
fn this_file_cites_no_validation_id() {
    let this_file = repo_root().join(THIS_FILE);
    let cited = scan_cited_ids(&[this_file.as_path()], IdKind::Validation)
        .unwrap_or_else(|error| panic!("{THIS_FILE} cannot be scanned: {error}"));
    assert!(
        cited.is_empty(),
        "{THIS_FILE} must hold no complete V-ID, found {:?}; cite it in the test that proves it",
        cited.keys().collect::<Vec<_>>()
    );
}
