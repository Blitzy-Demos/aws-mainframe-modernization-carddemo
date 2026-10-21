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
//! A rule whose `HALTED(Hn)` halt is still `BLOCKED` in `HALTS.md` must carry its layer's tags
//! like every other row (`modern/audit/README.md`, "Layers and required tags"; AAP 0.8.2,
//! 0.9.1). It is printed as `BLOCKED` and never counted as satisfied (AAP 0.8.5), and a missing
//! tag group fails it like any other row, its violation naming the halt. Checks 2 to 4 apply to
//! it unchanged.
//!
//! A halt's `Status` is `BLOCKED`, `RESOLVED` or `DESCOPED`. The synthetic tests prove that any
//! other status, an empty one included, fails the index load at the halt's row instead of
//! unblocking its rules, and that a `RESOLVED` or `DESCOPED` halt blocks nothing, so its fully
//! tagged rules count as satisfied while a `BLOCKED` halt's never do.
//!
//! The test only reads files: no container, database or async runtime. Its first passing run is
//! checkpoint C7, once every layer's tagged tests exist; before then it fails by design, and it
//! is not part of the C2 gate (AAP 0.6.3).
//!
//! This file sits under `crates/*/tests/`, so the tag scanner reads it too: it writes no tag
//! marker and no word of the rule-ID form, only placeholders such as `<PROGRAM>-nnn`, and its
//! synthetic test builds the rule IDs it needs at run time.

use std::collections::BTreeMap;
use std::fs;
use std::io;
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
    /// One line per rule that lacks a required tag group, blocked rules included.
    violations: Vec<String>,
    /// Rules that are not blocked and whose every tag group is met.
    satisfied: usize,
    /// Rules whose halt is `BLOCKED`, tagged or not; never counted as satisfied.
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

/// Check 1: every rule carries the tags its layer requires, whether or not a halt blocks it.
///
/// A rule whose halt is `BLOCKED` is printed as `BLOCKED` and counted as blocked, never as
/// satisfied. When it also lacks a required tag group it fails like any other rule, and its
/// violation names the halt.
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
        let blocked_by = index
            .is_blocked(rule)
            .then(|| rule.halt.as_deref().unwrap_or_default());
        if let Some(halt) = blocked_by {
            println!("BLOCKED {id} ({halt}) {}:{line}", file.display());
            outcome.blocked += 1;
        }
        let present = tags.get(id.as_str()).into_iter().flatten().copied();
        let requirement = required_tags(rule);
        let unmet = requirement.unmet_groups(present);
        if unmet.is_empty() {
            if blocked_by.is_none() {
                outcome.satisfied += 1;
            }
            continue;
        }
        let groups = unmet
            .iter()
            .map(|group| describe_group(group))
            .collect::<Vec<_>>()
            .join("; ");
        let layer = match blocked_by {
            Some(halt) => format!("{}, BLOCKED by {halt}", layer_label(rule)),
            None => layer_label(rule),
        };
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

/// The member name of the synthetic rules. Their IDs are built at run time, so this file holds
/// no word of the rule-ID form for the scanners to read.
const SYNTHETIC_MEMBER: &str = "SYNTH";
/// The synthetic halt that the synthetic `HALTS.md` lists as `BLOCKED`.
const BLOCKED_HALT: &str = "H1";
/// The synthetic halt that the synthetic `HALTS.md` lists as `RESOLVED`, which blocks nothing.
const RESOLVED_HALT: &str = "H2";
/// The rule-table header of AAP 0.8.2, without the `Layer` column.
const RULE_HEADER: &str = "| Rule ID | Program | Paragraph | Copybook field or PIC \
                           | Input condition | Expected result | Source lines |";
/// The separator row under [`RULE_HEADER`].
const RULE_SEPARATOR: &str = "|---|---|---|---|---|---|---|";

/// A row of the synthetic rule files. Its rule ID is [`synthetic_id`] of its 1-based position in
/// [`SYNTHETIC_ROWS`].
struct SyntheticRow {
    /// The directory of its rule file under the audit directory: rule files under `online` have a
    /// `Layer` column, those under `batch` none.
    dir: &'static str,
    /// The `Layer` cell, `None` in a file without that column.
    layer: Option<&'static str>,
    /// The halt its `Expected result` cell names as `HALTED(Hn)`.
    halt: &'static str,
}

// The 1-based positions of the rows in `SYNTHETIC_ROWS`.
const BLOCKED_SERVER: usize = 1;
const BLOCKED_CLIENT: usize = 2;
const BLOCKED_BOTH: usize = 3;
const RESOLVED_SERVER: usize = 4;
const BLOCKED_UNLAYERED: usize = 5;

/// A blocked `server`, `client` and `both` row, a `server` row whose halt is resolved, and a
/// blocked row of a file without a `Layer` column.
const SYNTHETIC_ROWS: [SyntheticRow; 5] = [
    SyntheticRow {
        dir: "online",
        layer: Some("server"),
        halt: BLOCKED_HALT,
    },
    SyntheticRow {
        dir: "online",
        layer: Some("client"),
        halt: BLOCKED_HALT,
    },
    SyntheticRow {
        dir: "online",
        layer: Some("both"),
        halt: BLOCKED_HALT,
    },
    SyntheticRow {
        dir: "online",
        layer: Some("server"),
        halt: RESOLVED_HALT,
    },
    SyntheticRow {
        dir: "batch",
        layer: None,
        halt: BLOCKED_HALT,
    },
];

// The tag groups of a `both` rule's requirement: the server half first, then the client half.
const SERVER_HALF: usize = 0;
const CLIENT_HALF: usize = 1;
/// The single tag group of a `server`, `client` or unlayered rule's requirement.
const ONLY_GROUP: usize = 0;

/// The rule ID of the synthetic row at 1-based position `row`.
fn synthetic_id(row: usize) -> String {
    format!("{SYNTHETIC_MEMBER}-{row:03}")
}

/// The synthetic halt report, in the shape of `modern/audit/HALTS.md`.
fn synthetic_halts() -> String {
    format!(
        "# Halt report\n\n\
         | ID | Path | Why no single result | Bindings blocked | Status | Resolution |\n\
         |---|---|---|---|---|---|\n\
         | {BLOCKED_HALT} | synthetic | synthetic | synthetic | BLOCKED | None |\n\
         | {RESOLVED_HALT} | synthetic | synthetic | synthetic | RESOLVED | synthetic |\n"
    )
}

/// The rule file under `dir`: its rule table holds every row of [`SYNTHETIC_ROWS`] in `dir`,
/// with a `Layer` column under `online`.
fn synthetic_rule_file(dir: &str) -> String {
    let (layer_header, layer_separator) = if dir == "online" {
        (" Layer |", "---|")
    } else {
        ("", "")
    };
    let mut text = format!("{RULE_HEADER}{layer_header}\n{RULE_SEPARATOR}{layer_separator}\n");
    for (row, spec) in (1..).zip(&SYNTHETIC_ROWS) {
        if spec.dir != dir {
            continue;
        }
        let id = synthetic_id(row);
        let halt = spec.halt;
        let layer = spec
            .layer
            .map_or_else(String::new, |layer| format!(" {layer} |"));
        text.push_str(&format!(
            "| {id} | {SYNTHETIC_MEMBER} | synthetic | synthetic | synthetic | HALTED({halt}) \
             | synthetic |{layer}\n"
        ));
    }
    text
}

/// A synthetic audit tree in a directory of the temporary directory unique to this process and
/// test, removed when the value drops, a panicking test included.
struct SyntheticTree {
    root: PathBuf,
}

impl SyntheticTree {
    /// Writes `HALTS.md` and the `online` and `batch` rule files of [`SYNTHETIC_ROWS`].
    fn create(test: &str) -> Self {
        let root = std::env::temp_dir().join(format!("carddemo-a8-{}-{test}", std::process::id()));
        // A tree an earlier process with the same ID left behind is replaced, never read.
        if let Err(err) = fs::remove_dir_all(&root) {
            assert_eq!(
                err.kind(),
                io::ErrorKind::NotFound,
                "cannot clear {}: {err}",
                root.display()
            );
        }
        // The guard exists before the first write, so a failed write still removes the tree.
        let tree = Self { root };
        tree.write("HALTS.md", &synthetic_halts());
        for dir in ["online", "batch"] {
            tree.write(
                &format!("{dir}/{SYNTHETIC_MEMBER}.md"),
                &synthetic_rule_file(dir),
            );
        }
        tree
    }

    /// Writes `text` to `file`, relative to the tree's root, creating its directory.
    fn write(&self, file: &str, text: &str) {
        let path = self.root.join(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .unwrap_or_else(|err| panic!("cannot create {}: {err}", parent.display()));
        }
        fs::write(&path, text)
            .unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
    }
}

impl Drop for SyntheticTree {
    fn drop(&mut self) {
        if let Err(err) = fs::remove_dir_all(&self.root) {
            eprintln!(
                "cannot remove the synthetic audit tree {}: {err}",
                self.root.display()
            );
        }
    }
}

/// A case of [`blocked_rules_need_their_layer_tags`]: the tags present and the outcome of
/// check 1 over the synthetic tree.
struct TagCase {
    name: &'static str,
    /// The tags present: the 1-based row of [`SYNTHETIC_ROWS`] each names, and its kind.
    tags: Vec<(usize, TagKind)>,
    /// The rows expected to fail, each with the positions of its unmet groups in the requirement
    /// [`required_tags`] states for it.
    failing: Vec<(usize, Vec<usize>)>,
    satisfied: usize,
    blocked: usize,
}

/// The cases of [`blocked_rules_need_their_layer_tags`].
fn tag_cases() -> Vec<TagCase> {
    let batch_test = || TagKind::RustTest("batch".to_owned());
    vec![
        TagCase {
            name: "no tags: every blocked row fails, and so does the resolved row",
            tags: Vec::new(),
            failing: vec![
                (BLOCKED_SERVER, vec![ONLY_GROUP]),
                (BLOCKED_CLIENT, vec![ONLY_GROUP]),
                (BLOCKED_BOTH, vec![SERVER_HALF, CLIENT_HALF]),
                (RESOLVED_SERVER, vec![ONLY_GROUP]),
                (BLOCKED_UNLAYERED, vec![ONLY_GROUP]),
            ],
            satisfied: 0,
            blocked: 4,
        },
        TagCase {
            name: "every row tagged: the blocked rows stay BLOCKED, only the resolved row passes",
            tags: vec![
                (BLOCKED_SERVER, TagKind::ApiEndpoints),
                (BLOCKED_CLIENT, TagKind::Nav),
                (BLOCKED_BOTH, TagKind::ApiEndpoints),
                (BLOCKED_BOTH, TagKind::Vitest),
                (RESOLVED_SERVER, TagKind::ApiEndpoints),
                (BLOCKED_UNLAYERED, batch_test()),
            ],
            failing: Vec::new(),
            satisfied: 1,
            blocked: 4,
        },
        TagCase {
            name: "the blocked both row carries only its server half: its client half fails",
            tags: vec![
                (BLOCKED_SERVER, TagKind::ApiEndpoints),
                (BLOCKED_CLIENT, TagKind::Vitest),
                (BLOCKED_BOTH, TagKind::ApiEndpoints),
                (RESOLVED_SERVER, TagKind::ApiEndpoints),
                (BLOCKED_UNLAYERED, batch_test()),
            ],
            failing: vec![(BLOCKED_BOTH, vec![CLIENT_HALF])],
            satisfied: 1,
            blocked: 4,
        },
        TagCase {
            name: "tags of the wrong kind meet no group of a blocked row",
            tags: vec![
                (BLOCKED_SERVER, TagKind::Vitest),
                (BLOCKED_CLIENT, TagKind::ApiEndpoints),
                (BLOCKED_BOTH, TagKind::Nav),
                (RESOLVED_SERVER, TagKind::ApiEndpoints),
                (BLOCKED_UNLAYERED, TagKind::Nav),
            ],
            failing: vec![
                (BLOCKED_SERVER, vec![ONLY_GROUP]),
                (BLOCKED_CLIENT, vec![ONLY_GROUP]),
                (BLOCKED_BOTH, vec![SERVER_HALF]),
                (BLOCKED_UNLAYERED, vec![ONLY_GROUP]),
            ],
            satisfied: 1,
            blocked: 4,
        },
    ]
}

/// The violation check 1 reports for the synthetic row `row` when the groups at `unmet` of its
/// requirement are unmet. The line comes from the index; the file, layer and halt from
/// [`SYNTHETIC_ROWS`].
fn expected_violation(index: &AuditIndex, row: usize, unmet: &[usize]) -> String {
    let id = synthetic_id(row);
    let rule = index
        .get(&id)
        .unwrap_or_else(|| panic!("the synthetic index has no rule {id}"));
    let spec = row
        .checked_sub(1)
        .and_then(|at| SYNTHETIC_ROWS.get(at))
        .unwrap_or_else(|| panic!("no synthetic row {row}"));
    let requirement = required_tags(rule);
    let groups = unmet
        .iter()
        .map(|&at| {
            let group = requirement
                .groups
                .get(at)
                .unwrap_or_else(|| panic!("rule {id} has no tag group {at}"));
            describe_group(group)
        })
        .collect::<Vec<_>>()
        .join("; ");
    let file = format!("audit/{}/{SYNTHETIC_MEMBER}.md", spec.dir);
    let line = rule.line;
    let layer = spec.layer.unwrap_or("by file");
    let blocked = if spec.halt == BLOCKED_HALT {
        format!(", BLOCKED by {BLOCKED_HALT}")
    } else {
        String::new()
    };
    format!("{file}:{line}: rule {id} ({layer}{blocked}) lacks required tags: {groups}")
}

#[test]
fn blocked_rules_need_their_layer_tags() {
    let tree = SyntheticTree::create("blocked_rules_need_their_layer_tags");
    let index = AuditIndex::load(&tree.root)
        .unwrap_or_else(|err| panic!("cannot load the synthetic audit index: {err}"));
    assert_eq!(
        index.rules().len(),
        SYNTHETIC_ROWS.len(),
        "the synthetic index holds every synthetic row"
    );
    for case in tag_cases() {
        let name = case.name;
        let sites: Vec<TagSite> = case
            .tags
            .into_iter()
            .map(|(row, kind)| TagSite {
                id: synthetic_id(row),
                path: PathBuf::from("synthetic"),
                line: row,
                kind,
            })
            .collect();
        let outcome = check_required_tags(&index, &tags_by_rule(&sites));
        let mut violations = outcome.violations;
        violations.sort();
        for (row, _) in &case.failing {
            let mention = format!("rule {} (", synthetic_id(*row));
            let named = violations
                .iter()
                .filter(|violation| violation.contains(&mention))
                .count();
            assert_eq!(named, 1, "{name}: one violation names `{mention}`");
        }
        let mut expected: Vec<String> = case
            .failing
            .iter()
            .map(|(row, unmet)| expected_violation(&index, *row, unmet))
            .collect();
        expected.sort();
        assert_eq!(violations, expected, "{name}: violations");
        assert_eq!(outcome.blocked, case.blocked, "{name}: blocked rules");
        assert_eq!(outcome.satisfied, case.satisfied, "{name}: satisfied rules");
    }
}

/// A halt report in the shape of [`synthetic_halts`] whose rows are `rows`, each a halt ID and
/// the text of its `Status` cell.
fn halt_report(rows: &[(&str, &str)]) -> String {
    let mut text = "# Halt report\n\n\
                    | ID | Path | Why no single result | Bindings blocked | Status | Resolution |\n\
                    |---|---|---|---|---|---|\n"
        .to_owned();
    for (halt, status) in rows {
        text.push_str(&format!(
            "| {halt} | synthetic | synthetic | synthetic | {status} | synthetic |\n"
        ));
    }
    text
}

/// The 1-based line of the row of halt `halt` in the halt report `text`.
fn halt_line(text: &str, halt: &str) -> usize {
    let prefix = format!("| {halt} |");
    (1..)
        .zip(text.lines())
        .find_map(|(line, content)| content.starts_with(&prefix).then_some(line))
        .unwrap_or_else(|| panic!("the halt report has no row for {halt}"))
}

/// One tag of every kind each synthetic row's layer requires, so check 1 finds every group met.
fn every_row_tagged() -> Vec<TagSite> {
    let mut sites = Vec::new();
    for (row, spec) in (1..).zip(&SYNTHETIC_ROWS) {
        let kinds = match spec.layer {
            Some("server") => vec![TagKind::ApiEndpoints],
            Some("client") => vec![TagKind::Nav],
            Some("both") => vec![TagKind::ApiEndpoints, TagKind::Vitest],
            Some(other) => panic!("synthetic row {row} has the unknown layer `{other}`"),
            None => vec![TagKind::RustTest("batch".to_owned())],
        };
        sites.extend(kinds.into_iter().map(|kind| TagSite {
            id: synthetic_id(row),
            path: PathBuf::from("synthetic"),
            line: row,
            kind,
        }));
    }
    sites
}

/// A `Status` cell outside the closed set, and the synthetic halt whose row carries it; the other
/// halt keeps its valid status.
struct InvalidStatus {
    name: &'static str,
    halt: &'static str,
    status: &'static str,
}

/// The cases of [`halt_status_outside_the_closed_set_fails_at_its_row`]. Read as anything but
/// `BLOCKED`, each would unblock the rules of a halt that has no valid terminal status.
const INVALID_STATUSES: [InvalidStatus; 5] = [
    InvalidStatus {
        name: "a misspelt BLOCKED",
        halt: BLOCKED_HALT,
        status: "BLOCKD",
    },
    InvalidStatus {
        name: "an empty cell",
        halt: RESOLVED_HALT,
        status: "",
    },
    InvalidStatus {
        name: "a dash for no status",
        halt: BLOCKED_HALT,
        status: "—",
    },
    InvalidStatus {
        name: "a status outside the set",
        halt: RESOLVED_HALT,
        status: "PENDING",
    },
    InvalidStatus {
        name: "a valid status followed by more text",
        halt: BLOCKED_HALT,
        status: "BLOCKED, acknowledged",
    },
];

#[test]
fn halt_status_outside_the_closed_set_fails_at_its_row() {
    let tree = SyntheticTree::create("halt_status_outside_the_closed_set_fails_at_its_row");
    let halts_path = tree.root.join("HALTS.md");
    for case in &INVALID_STATUSES {
        let name = case.name;
        let rows = [(BLOCKED_HALT, "BLOCKED"), (RESOLVED_HALT, "RESOLVED")].map(|(halt, valid)| {
            let status = if halt == case.halt {
                case.status
            } else {
                valid
            };
            (halt, status)
        });
        let report = halt_report(&rows);
        tree.write("HALTS.md", &report);
        let Err(err) = AuditIndex::load(&tree.root) else {
            panic!(
                "{name}: the index loaded halt {} with status `{}`",
                case.halt, case.status
            );
        };
        let message = err.to_string();
        let location = format!(
            "{}:{}: ",
            halts_path.display(),
            halt_line(&report, case.halt)
        );
        assert!(
            message.starts_with(&location),
            "{name}: `{message}` starts with `{location}`"
        );
        let halt = format!("halt {} ", case.halt);
        assert!(
            message.contains(&halt),
            "{name}: `{message}` names `{halt}`"
        );
        let allowed = "`BLOCKED`, `RESOLVED` or `DESCOPED`";
        assert!(
            message.contains(allowed),
            "{name}: `{message}` names the allowed statuses"
        );
    }
}

/// A valid `Status` cell of the synthetic [`BLOCKED_HALT`] row, whether it blocks the rows that
/// name that halt, and the outcome of check 1 when every synthetic row carries its tags.
struct ValidStatus {
    status: &'static str,
    blocks: bool,
    satisfied: usize,
    blocked: usize,
}

/// The cases of [`only_a_blocked_halt_blocks_its_rules`]: the initial status, both terminal
/// statuses, and a terminal status written in backticks and lower case.
const VALID_STATUSES: [ValidStatus; 4] = [
    ValidStatus {
        status: "BLOCKED",
        blocks: true,
        satisfied: 1,
        blocked: 4,
    },
    ValidStatus {
        status: "RESOLVED",
        blocks: false,
        satisfied: 5,
        blocked: 0,
    },
    ValidStatus {
        status: "DESCOPED",
        blocks: false,
        satisfied: 5,
        blocked: 0,
    },
    ValidStatus {
        status: "`descoped`",
        blocks: false,
        satisfied: 5,
        blocked: 0,
    },
];

#[test]
fn only_a_blocked_halt_blocks_its_rules() {
    let tree = SyntheticTree::create("only_a_blocked_halt_blocks_its_rules");
    let sites = every_row_tagged();
    let tags = tags_by_rule(&sites);
    for case in &VALID_STATUSES {
        let status = case.status;
        let report = halt_report(&[(BLOCKED_HALT, status), (RESOLVED_HALT, "RESOLVED")]);
        tree.write("HALTS.md", &report);
        let index = AuditIndex::load(&tree.root).unwrap_or_else(|err| {
            panic!("status `{status}`: cannot load the synthetic audit index: {err}")
        });
        for (row, spec) in (1..).zip(&SYNTHETIC_ROWS) {
            let id = synthetic_id(row);
            let rule = index
                .get(&id)
                .unwrap_or_else(|| panic!("status `{status}`: the index has no rule {id}"));
            assert_eq!(
                index.is_blocked(rule),
                case.blocks && spec.halt == BLOCKED_HALT,
                "status `{status}`: whether rule {id} is blocked"
            );
        }
        let outcome = check_required_tags(&index, &tags);
        assert_eq!(
            outcome.violations,
            Vec::<String>::new(),
            "status `{status}`: violations"
        );
        assert_eq!(
            outcome.blocked, case.blocked,
            "status `{status}`: blocked rules"
        );
        assert_eq!(
            outcome.satisfied, case.satisfied,
            "status `{status}`: satisfied rules"
        );
    }
}
