//! Binding a8, "no code without its audit" (AAP 0.8.2, 0.9.1, 0.9.2).
//!
//! The test joins what `test_support::audit_index` parses and scans, and decides four checks:
//!
//! 1. Every rule of every rule file under `modern/audit/` carries the test tags its layer
//!    requires, as [`required_tags`] states them from `modern/audit/README.md`.
//! 2. Every test tag names a rule of the index.
//! 3. Every rule-ID-shaped word cited in `crates/*/src/**` names a rule of the index.
//! 4. Every rule module, `crates/domain/src/{online,batch,messaging}/*.rs` other than `mod.rs`,
//!    cites at least one rule ID in a doc comment (AAP 0.1.2).
//!
//! A rule whose `HALTED(Hn)` halt is still `BLOCKED` in `HALTS.md` is exempt from check 1 only:
//! it is printed as `BLOCKED` and never counted as satisfied, so it keeps g8 open through the
//! halt report rather than failing or passing here (AAP 0.8.5, 0.9.5). Checks 2 to 4 apply to it
//! unchanged.
//!
//! The test only reads files: no container, database or async runtime. Its first passing run is
//! checkpoint C7, once every layer's tagged tests exist; before then it fails by design, and it
//! is not part of the C2 gate (AAP 0.6.3).
//!
//! This file sits under `crates/*/tests/`, so the tag scanner reads it too: it writes no tag
//! marker and no word of the rule-ID form, only placeholders such as `<PROGRAM>-nnn`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use test_support::audit_index::{
    AuditIndex, AuditRule, Citation, Layer, TagKind, TagSite, audit_dir, required_tags,
    rule_modules, scan_source_citations, scan_test_tags,
};
use test_support::modern_dir;

/// The tag kinds present for each rule ID, as the tag scan found them.
type TagsByRule<'a> = BTreeMap<&'a str, Vec<&'a TagKind>>;

/// The outcome of check 1.
struct RequiredTags {
    /// One line per rule that lacks a required tag group.
    violations: Vec<String>,
    /// Rules that are not blocked and whose every tag group is met.
    satisfied: usize,
    /// Rules exempted from the check because their halt is `BLOCKED`.
    blocked: usize,
}

/// How many rules each layer holds; `by_file` counts the rules of files without a `Layer`
/// column.
#[derive(Default)]
struct LayerCounts {
    server: usize,
    client: usize,
    both: usize,
    by_file: usize,
}

/// `path` relative to `modern`. The scanners already return paths under `modern/`, so a path
/// that does not start with it is returned unchanged.
fn relative<'p>(path: &'p Path, modern: &Path) -> &'p Path {
    path.strip_prefix(modern).unwrap_or(path)
}

/// A rule file as a path relative to `modern/`; the index stores it relative to `modern/audit/`.
fn rule_file(rule: &AuditRule) -> PathBuf {
    Path::new("audit").join(&rule.file)
}

/// The layer a violation names: the rule's `Layer` cell, or `by file` for a file without one.
fn layer_label(rule: &AuditRule) -> String {
    rule.layer
        .map_or_else(|| "by file".to_owned(), |layer| layer.to_string())
}

/// The tag kinds present for each rule ID.
fn tags_by_rule(tags: &[TagSite]) -> TagsByRule<'_> {
    let mut by_rule = TagsByRule::new();
    for site in tags {
        by_rule
            .entry(site.id.as_str())
            .or_default()
            .push(&site.kind);
    }
    by_rule
}

/// Guards against a scanner that found nothing, which would let the checks pass vacuously.
fn sanity_guards(index: &AuditIndex, tags: &[TagSite], modules: &[PathBuf]) -> Vec<String> {
    let mut violations = Vec::new();
    if index.rules().is_empty() {
        violations.push(
            "index: no rule table under audit/ holds a rule; the index read nothing".to_owned(),
        );
    }
    if tags.is_empty() {
        violations.push(
            "tags: the scan found no test tags under crates/*/tests, frontend/src or frontend/e2e"
                .to_owned(),
        );
    }
    if modules.is_empty() {
        violations.push(
            "modules: no rule module under crates/domain/src/{online,batch,messaging}; check 4 \
             would pass vacuously"
                .to_owned(),
        );
    }
    violations
}

/// Check 1: every rule that no `BLOCKED` halt covers carries the tags its layer requires.
///
/// A blocked rule is printed and skipped, never counted as satisfied.
fn check_required_tags(index: &AuditIndex, tags: &TagsByRule<'_>) -> RequiredTags {
    let mut outcome = RequiredTags {
        violations: Vec::new(),
        satisfied: 0,
        blocked: 0,
    };
    for rule in index.rules() {
        let id = &rule.id;
        let file = rule_file(rule);
        let line = rule.line;
        if index.is_blocked(rule) {
            let halt = rule.halt.as_deref().unwrap_or_default();
            println!("BLOCKED {id} ({halt}) {}:{line}", file.display());
            outcome.blocked += 1;
            continue;
        }
        let present = tags.get(id.as_str()).into_iter().flatten().copied();
        let requirement = required_tags(rule);
        let unmet = requirement.unmet_groups(present);
        if unmet.is_empty() {
            outcome.satisfied += 1;
            continue;
        }
        let groups = unmet
            .iter()
            .map(|group| describe_group(group))
            .collect::<Vec<_>>()
            .join("; ");
        let layer = layer_label(rule);
        outcome.violations.push(format!(
            "{}:{line}: rule {id} ({layer}) lacks required tags: {groups}",
            file.display()
        ));
    }
    outcome
}

/// One unmet tag group: its alternative kinds, any one of which would meet it.
fn describe_group(group: &[TagKind]) -> String {
    group
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" or ")
}

/// Check 2: every test tag names a rule, whether or not a halt blocks that rule.
fn check_tags_resolve(index: &AuditIndex, tags: &[TagSite], modern: &Path) -> Vec<String> {
    tags.iter()
        .filter(|site| index.get(&site.id).is_none())
        .map(|site| {
            let path = relative(&site.path, modern).display();
            let line = site.line;
            let id = &site.id;
            format!("{path}:{line}: test tag names no rule: {id}")
        })
        .collect()
}

/// Check 3: every rule-ID-shaped word in crate source names a rule.
fn check_citations_resolve(
    index: &AuditIndex,
    citations: &[Citation],
    modern: &Path,
) -> Vec<String> {
    citations
        .iter()
        .filter(|citation| index.get(&citation.id).is_none())
        .map(|citation| {
            let path = relative(&citation.path, modern).display();
            let line = citation.line;
            let id = &citation.id;
            format!("{path}:{line}: source cites unknown rule {id}")
        })
        .collect()
}

/// Check 4: every rule module cites at least one rule ID in a doc comment.
fn check_rule_modules_cite(
    modules: &[PathBuf],
    citations: &[Citation],
    modern: &Path,
) -> Vec<String> {
    modules
        .iter()
        .map(|module| relative(module, modern))
        .filter(|module| {
            !citations.iter().any(|citation| {
                citation.in_doc_comment && relative(&citation.path, modern) == *module
            })
        })
        .map(|module| {
            let module = module.display();
            format!("{module}: rule module cites no rule ID in a doc comment (AAP 0.1.2)")
        })
        .collect()
}

/// How many rules each layer holds, blocked rules included.
fn layer_counts(rules: &[AuditRule]) -> LayerCounts {
    let mut counts = LayerCounts::default();
    for rule in rules {
        let slot = match rule.layer {
            Some(Layer::Server) => &mut counts.server,
            Some(Layer::Client) => &mut counts.client,
            Some(Layer::Both) => &mut counts.both,
            None => &mut counts.by_file,
        };
        *slot += 1;
    }
    counts
}

#[test]
fn audit_rules_are_tagged_and_every_citation_resolves() {
    let modern = modern_dir();
    let index = AuditIndex::load(&audit_dir())
        .unwrap_or_else(|err| panic!("cannot load the audit index: {err}"));
    let tags =
        scan_test_tags(&modern).unwrap_or_else(|err| panic!("cannot scan the test tags: {err}"));
    let citations = scan_source_citations(&modern)
        .unwrap_or_else(|err| panic!("cannot scan the source citations: {err}"));
    let modules =
        rule_modules(&modern).unwrap_or_else(|err| panic!("cannot list the rule modules: {err}"));
    let by_rule = tags_by_rule(&tags);

    let mut violations = sanity_guards(&index, &tags, &modules);
    let required = check_required_tags(&index, &by_rule);
    violations.extend(required.violations);
    violations.extend(check_tags_resolve(&index, &tags, &modern));
    violations.extend(check_citations_resolve(&index, &citations, &modern));
    violations.extend(check_rule_modules_cite(&modules, &citations, &modern));
    violations.sort();
    for violation in &violations {
        println!("{violation}");
    }

    let LayerCounts {
        server,
        client,
        both,
        by_file,
    } = layer_counts(index.rules());
    let total = index.rules().len();
    let satisfied = required.satisfied;
    let blocked = required.blocked;
    let failures = violations.len();
    println!(
        "audit_coverage: {total} rules ({server} server, {client} client, {both} both, \
         {by_file} by file), {satisfied} satisfied, {blocked} BLOCKED, {failures} failures"
    );
    assert!(
        violations.is_empty(),
        "{n} audit coverage failure(s)",
        n = violations.len()
    );
}
