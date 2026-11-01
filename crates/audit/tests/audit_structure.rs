//! Pre-code structure check of the derive-lane parity audits (AAP 0.9.1).
//!
//! Every section of `docs/parity/parity-oracle-audit.md`, `docs/parity/validation-audit.md`
//! and `docs/parity/function-map.md`, and the escalation log `docs/parity/escalations.md`,
//! passes this check before its COBOL-informed review; the code a section gates is written
//! only after both. The check proves structure: the sections and columns each audit
//! declares, well-formed and unique row IDs, `path:line` citations that land on lines of
//! repository files, rows for every carried program and BMS map, escalation references that
//! resolve to the log, and well-formed review logs. It never requires a verdict to be
//! `pass`: passing review is recorded in `docs/parity/signoff.md`, which this check does not
//! read.
//!
//! Run it alone with `cargo test -p carddemo-audit --test audit_structure`. It uses `std` and
//! [`carddemo_audit`] only, so it builds before any other workspace crate exists.
//!
//! Each `#[test]` loads the committed documents and reports every violation its
//! `*_violations` function collects, one `<file>:<line>: <problem>` per line, so a single run
//! lists every defect.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use carddemo_audit::{
    Citation, Document, ESCALATION_COLUMNS, ESCALATION_STATUSES, ESCALATIONS_PATH,
    FUNCTION_MAP_COLUMNS, FUNCTION_MAP_PATH, IdKind, PARITY_AUDIT_PATH, PARITY_COMMON_COLUMNS,
    PARITY_LAYOUT_COLUMNS, PARITY_RULE_COLUMNS, PARITY_SECTIONS, REVIEW_LOG_COLUMNS, Repo, Row,
    Table, VALIDATION_AUDIT_PATH, VALIDATION_COLUMNS, citations, classify_id, code_span, id_tokens,
    ids_with_prefix, is_no_message, no_message_escalation,
};

/// The carried programs, copybook procedures and JCL-level jobs the parity audit covers
/// (AAP 0.9.1), each named in the `Program` or `Paragraph` cell of some data row.
const PARITY_PROGRAMS: [&str; 39] = [
    "COSGN00C", "COMEN01C", "COADM01C", "COACTVWC", "COACTUPC", "CSUTLDPY", "CSLKPCDY", "COCRDLIC",
    "COCRDSLC", "COCRDUPC", "COTRN00C", "COTRN01C", "COTRN02C", "CORPT00C", "COBIL00C", "COPAUS0C",
    "COPAUS1C", "COPAUS2C", "COUSR00C", "COUSR01C", "COUSR02C", "COUSR03C", "COTRTUPC", "COTRTLIC",
    "CSUTLDTC", "CBTRN02C", "CBACT04C", "CBSTM03A", "CBSTM03B", "CBTRN03C", "CBPAUP0C", "COBTUPDT",
    "TRANBKP", "COMBTRAN", "CREASTMT", "TRANREPT", "CODATE01", "COACCT01", "COPAUA0C",
];

/// The online programs with an input edit that the validation audit covers (AAP 0.9.1),
/// each named in the `Program` cell of some data row.
const VALIDATION_PROGRAMS: [&str; 19] = [
    "COACTUPC", "COCRDUPC", "COSGN00C", "COBIL00C", "COTRN02C", "COUSR00C", "COUSR01C", "COUSR02C",
    "COUSR03C", "COTRTUPC", "COACTVWC", "COCRDLIC", "COCRDSLC", "COTRN00C", "COTRN01C", "CORPT00C",
    "COPAUS0C", "COPAUS1C", "COTRTLIC",
];

/// The 21 BMS maps the function map covers: 17 in `app/bms`, `COPAU00` and `COPAU01` in
/// `app/app-authorization-ims-db2-mq/bms`, `COTRTLI` and `COTRTUP` in
/// `app/app-transaction-type-db2/bms`.
const BMS_MAPS: [&str; 21] = [
    "COACTUP", "COACTVW", "COADM01", "COBIL00", "COCRDLI", "COCRDSL", "COCRDUP", "COMEN01",
    "CORPT00", "COSGN00", "COTRN00", "COTRN01", "COTRN02", "COUSR00", "COUSR01", "COUSR02",
    "COUSR03", "COPAU00", "COPAU01", "COTRTLI", "COTRTUP",
];

/// The column names the checks read, taken from the library's header constants.
const ID: &str = PARITY_COMMON_COLUMNS[0];
const PROGRAM: &str = PARITY_COMMON_COLUMNS[1];
const PARAGRAPH: &str = PARITY_COMMON_COLUMNS[2];
const VALIDATION_PROGRAM: &str = VALIDATION_COLUMNS[1];
const MESSAGE_TEXT: &str = VALIDATION_COLUMNS[4];
const VALIDATION_STATUS: &str = VALIDATION_COLUMNS[5];
const BMS_MAP: &str = FUNCTION_MAP_COLUMNS[1];
const ESCALATION_STATUS: &str = ESCALATION_COLUMNS[6];
const ROW_ID: &str = REVIEW_LOG_COLUMNS[0];
const VERDICT: &str = REVIEW_LOG_COLUMNS[1];
const REVIEWER: &str = REVIEW_LOG_COLUMNS[3];

/// The verdicts a review log row may record; `fail` is well formed too.
const VERDICTS: [&str; 2] = ["pass", "fail"];

/// The line a violation of a whole file points at: the file's first line.
const FILE_LINE: usize = 1;

/// One of the three audits, with the path its violations name and the ID kind its data rows
/// define.
#[derive(Clone, Copy)]
struct Audit<'a> {
    path: &'static str,
    doc: &'a Document,
    kind: IdKind,
}

/// The three audits in a fixed order: parity, validation, function map.
fn audits<'a>(
    parity: &'a Document,
    validation: &'a Document,
    function_map: &'a Document,
) -> [Audit<'a>; 3] {
    [
        Audit {
            path: PARITY_AUDIT_PATH,
            doc: parity,
            kind: IdKind::Parity,
        },
        Audit {
            path: VALIDATION_AUDIT_PATH,
            doc: validation,
            kind: IdKind::Validation,
        },
        Audit {
            path: FUNCTION_MAP_PATH,
            doc: function_map,
            kind: IdKind::Function,
        },
    ]
}

/// The repository this crate sits in (`crates/audit`, two levels below the root).
fn repo() -> Repo {
    Repo::from_manifest_dir(env!("CARGO_MANIFEST_DIR"))
}

/// The committed audit at the repository-relative path `rel`.
fn load(repo: &Repo, rel: &str) -> Document {
    let Ok(doc) = repo.load(rel) else {
        panic!("{rel} is missing or unreadable: write the audit before the gated code (AAP 0.9.1)");
    };
    doc
}

/// Fails the calling test when `violations` is not empty, listing every one.
fn report(label: &str, violations: &[String]) {
    assert!(
        violations.is_empty(),
        "{label}: {} violation(s)\n{}",
        violations.len(),
        violations.join("\n")
    );
}

/// One violation, formatted `<file>:<line>: <problem>`.
fn violation(path: &str, line: usize, problem: &str) -> String {
    format!("{path}:{line}: {problem}")
}

/// An audit data table: first column `ID`, outside `## Escalations raised`.
fn is_data(table: &Table) -> bool {
    table.is_data_table() && !table.is_escalations_raised()
}

/// A handoff table: first column `ID`, under `## Escalations raised`.
fn is_handoff(table: &Table) -> bool {
    table.is_data_table() && table.is_escalations_raised()
}

/// Every row of every data table of `doc`, with its table.
fn data_rows(doc: &Document) -> impl Iterator<Item = (&Table, &Row)> {
    doc.tables()
        .iter()
        .filter(|table| is_data(table))
        .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
}

/// The trimmed cell of `row` under `name`; empty when the column or the cell is absent.
fn cell<'a>(table: &'a Table, row: &'a Row, name: &str) -> &'a str {
    table.cell(row, name).unwrap_or_default().trim()
}

/// The `##` section of `table`, empty before the first `##` heading.
fn section_of(table: &Table) -> &str {
    table.section.as_deref().unwrap_or_default()
}

/// The ASCII-upper-cased runs of ASCII letters and digits in `cell`, so `COACTUPC`,
/// `coactupc` and `COACTUPC/CSUTLDPY` all hold the token `COACTUPC`.
fn tokens(cell: &str) -> BTreeSet<String> {
    cell.split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_uppercase)
        .collect()
}

/// One violation per column of `required` that the header of `table` lacks.
fn missing_columns(path: &str, table: &Table, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter(|name| table.column(name).is_none())
        .map(|name| {
            violation(
                path,
                table.line,
                &format!("table lacks the column `{name}`"),
            )
        })
        .collect()
}

/// One violation per row of a data, handoff or review-log table of `doc` whose cell count
/// differs from its header's.
fn cell_count_violations(path: &str, doc: &Document) -> Vec<String> {
    let mut violations = Vec::new();
    for table in doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table() || table.is_review_log())
    {
        for row in &table.rows {
            if row.cells.len() != table.header.len() {
                let problem = format!(
                    "row has {} cells, but the header at line {} has {}",
                    row.cells.len(),
                    table.line,
                    table.header.len()
                );
                violations.push(violation(path, row.line, &problem));
            }
        }
    }
    violations
}

/// Violations of the tables of one scope (a parity section or a whole file) that hold no
/// data row or no review log, reported at `line`.
fn rows_and_review_violations(
    path: &str,
    line: usize,
    scope: &str,
    tables: &[&Table],
) -> Vec<String> {
    let mut violations = Vec::new();
    let rows: usize = tables
        .iter()
        .filter(|table| is_data(table))
        .map(|table| table.rows.len())
        .sum();
    if rows == 0 {
        violations.push(violation(path, line, &format!("{scope} has no data row")));
    }
    if !tables.iter().any(|table| table.is_review_log()) {
        violations.push(violation(path, line, &format!("{scope} has no review log")));
    }
    violations
}

/// The problem with a data row ID that should be of `kind`, if any.
fn id_problem(id: &str, kind: IdKind) -> Option<String> {
    if id.is_empty() {
        return Some("row has an empty `ID` cell".to_owned());
    }
    (classify_id(id) != Some(kind)).then(|| {
        let prefix = kind.prefix();
        format!("`{id}` is not a well-formed {prefix}- ID")
    })
}

/// Records the first line of `id` in `first`, or returns the violation of a repeat.
fn duplicate_violation(
    path: &str,
    first: &mut BTreeMap<String, usize>,
    id: &str,
    line: usize,
) -> Option<String> {
    match first.entry(id.to_owned()) {
        Entry::Occupied(entry) => {
            let defined = *entry.get();
            Some(violation(
                path,
                line,
                &format!("`{id}` repeats the ID first defined at line {defined}"),
            ))
        }
        Entry::Vacant(entry) => {
            entry.insert(line);
            None
        }
    }
}

/// Test 1: the six sections, their columns, rows and review logs, and the cell counts of
/// the parity audit.
fn parity_audit_sections_and_columns_violations(parity: &Document) -> Vec<String> {
    let path = PARITY_AUDIT_PATH;
    let mut violations = Vec::new();
    for table in parity.tables().iter().filter(|table| is_data(table)) {
        violations.extend(parity_table_violations(table));
    }
    for section in PARITY_SECTIONS {
        let tables: Vec<&Table> = parity
            .tables()
            .iter()
            .filter(|table| table.section.as_deref() == Some(section))
            .collect();
        let Some(first) = tables.first() else {
            let problem = format!("section `## {section}` is missing: no table sits under it");
            violations.push(violation(path, FILE_LINE, &problem));
            continue;
        };
        let scope = format!("section `## {section}`");
        violations.extend(rows_and_review_violations(
            path, first.line, &scope, &tables,
        ));
    }
    violations.extend(cell_count_violations(path, parity));
    violations
}

/// The placement and column violations of one parity data table.
fn parity_table_violations(table: &Table) -> Vec<String> {
    let path = PARITY_AUDIT_PATH;
    let mut violations = Vec::new();
    let mut required: Vec<&str> = PARITY_COMMON_COLUMNS.to_vec();
    match table.section.as_deref() {
        Some(section) if section == PARITY_SECTIONS[0] => required.extend(PARITY_LAYOUT_COLUMNS),
        Some(section) if section == PARITY_SECTIONS[1] => required.extend(PARITY_RULE_COLUMNS),
        Some(section) if PARITY_SECTIONS.contains(&section) => {}
        Some(section) => {
            let problem = format!("data table under `## {section}` is outside sections A to F");
            violations.push(violation(path, table.line, &problem));
        }
        None => {
            let problem = "data table precedes the first `##` heading";
            violations.push(violation(path, table.line, problem));
        }
    }
    violations.extend(missing_columns(path, table, &required));
    violations
}

/// Test 2: the columns, rows, review logs and cell counts of the validation audit and the
/// function map, and the `Status` and `Message text` cells of every validation row.
fn validation_audit_and_function_map_columns_violations(
    validation: &Document,
    function_map: &Document,
) -> Vec<String> {
    let files: [(&str, &Document, &[&str]); 2] = [
        (VALIDATION_AUDIT_PATH, validation, &VALIDATION_COLUMNS),
        (FUNCTION_MAP_PATH, function_map, &FUNCTION_MAP_COLUMNS),
    ];
    let mut violations = Vec::new();
    for (path, doc, columns) in files {
        for table in doc.tables().iter().filter(|table| is_data(table)) {
            violations.extend(missing_columns(path, table, columns));
        }
        let tables: Vec<&Table> = doc.tables().iter().collect();
        violations.extend(rows_and_review_violations(
            path, FILE_LINE, "the file", &tables,
        ));
        violations.extend(cell_count_violations(path, doc));
    }
    for (table, row) in data_rows(validation) {
        violations.extend(validation_row_violations(table, row));
    }
    violations
}

/// The `Status` and `Message text` violations of one validation data row. A column the
/// table lacks is reported by [`missing_columns`] and not again per row.
fn validation_row_violations(table: &Table, row: &Row) -> Vec<String> {
    let path = VALIDATION_AUDIT_PATH;
    let id = cell(table, row, ID);
    let mut violations = Vec::new();
    if table.column(VALIDATION_STATUS).is_some() && cell(table, row, VALIDATION_STATUS).is_empty() {
        let problem = format!("row {id} has an empty `{VALIDATION_STATUS}` cell");
        violations.push(violation(path, row.line, &problem));
    }
    if table.column(MESSAGE_TEXT).is_some() {
        let message = cell(table, row, MESSAGE_TEXT);
        if code_span(message).is_none() && no_message_escalation(message).is_none() {
            let problem = if is_no_message(message) {
                "no-message without an E-ID"
            } else {
                "message text is neither a code span nor no-message (E-nn)"
            };
            violations.push(violation(path, row.line, &format!("row {id}: {problem}")));
        }
    }
    violations
}

/// Test 3: the escalation log's columns, IDs, `Status` cells and cell counts.
///
/// The log is exempt from the citation check of test 5: its AAP-seeded evidence cites short
/// paths such as `[CBACT04C.cbl:L188-222]` that name no repository-relative file.
fn escalation_log_structure_violations(escalations: &Document) -> Vec<String> {
    let path = ESCALATIONS_PATH;
    let mut violations = Vec::new();
    let tables: Vec<&Table> = escalations
        .tables()
        .iter()
        .filter(|table| table.is_data_table())
        .collect();
    if tables.is_empty() {
        let problem = format!("the log holds no table whose first column is `{ID}`");
        violations.push(violation(path, FILE_LINE, &problem));
    }
    let mut first = BTreeMap::new();
    for table in tables {
        violations.extend(missing_columns(path, table, &ESCALATION_COLUMNS));
        for row in &table.rows {
            let id = cell(table, row, ID);
            if let Some(problem) = id_problem(id, IdKind::Escalation) {
                violations.push(violation(path, row.line, &problem));
            } else {
                violations.extend(duplicate_violation(path, &mut first, id, row.line));
            }
            let status = table
                .column(ESCALATION_STATUS)
                .map(|_| cell(table, row, ESCALATION_STATUS));
            if let Some(problem) = status.and_then(status_problem) {
                violations.push(violation(path, row.line, &format!("row {id}: {problem}")));
            }
        }
    }
    violations.extend(cell_count_violations(path, escalations));
    violations
}

/// The problem with an escalation `Status` cell, if any: its first word, up to whitespace,
/// `:` or `;`, must be one of [`ESCALATION_STATUSES`], and a note after it must open with
/// `:` or `;` (`open: user confirms`, `resolved; known difference in signoff.md`).
fn status_problem(status: &str) -> Option<String> {
    let end = status
        .find(|ch: char| ch.is_whitespace() || ch == ':' || ch == ';')
        .unwrap_or(status.len());
    let (word, rest) = status.split_at(end);
    if !ESCALATION_STATUSES.contains(&word) {
        let allowed = ESCALATION_STATUSES.join(", ");
        return Some(format!(
            "status `{status}` does not start with one of {allowed}"
        ));
    }
    let rest = rest.trim_start();
    (!rest.is_empty() && !rest.starts_with([':', ';']))
        .then(|| format!("status `{status}`: a note after `{word}` must follow `:` or `;`"))
}

/// Test 4: every data row ID of each audit is well formed for that audit and unique within
/// it. Handoff and review rows define no audit ID and are not checked here.
fn ids_are_well_formed_and_unique_violations(
    parity: &Document,
    validation: &Document,
    function_map: &Document,
) -> Vec<String> {
    let mut violations = Vec::new();
    for audit in audits(parity, validation, function_map) {
        let mut first = BTreeMap::new();
        for (table, row) in data_rows(audit.doc) {
            let id = cell(table, row, ID);
            if let Some(problem) = id_problem(id, audit.kind) {
                violations.push(violation(audit.path, row.line, &problem));
            } else {
                violations.extend(duplicate_violation(audit.path, &mut first, id, row.line));
            }
        }
    }
    violations
}

/// Test 5: every data row of the three audits cites at least one `path:line`, and every
/// citation names a repository file and lines inside it.
///
/// [`Repo::line_count`] reads bytes, because the EBCDIC fixtures are not UTF-8, and counts
/// one line per LF plus an unterminated last line, so CRLF files and files without a final
/// LF count their physical lines. Counts are cached per cited path; `None` stands for a path
/// that resolves to no readable file. `docs/parity/escalations.md` is exempt (see test 3).
fn citations_resolve_to_repository_lines_violations(
    parity: &Document,
    validation: &Document,
    function_map: &Document,
    repo: &Repo,
) -> Vec<String> {
    let mut counts = BTreeMap::new();
    let mut violations = Vec::new();
    for audit in audits(parity, validation, function_map) {
        for (table, row) in data_rows(audit.doc) {
            violations.extend(row_citation_violations(
                audit.path,
                table,
                row,
                repo,
                &mut counts,
            ));
        }
    }
    violations
}

/// The citation violations of one data row.
fn row_citation_violations(
    path: &str,
    table: &Table,
    row: &Row,
    repo: &Repo,
    counts: &mut BTreeMap<String, Option<usize>>,
) -> Vec<String> {
    let id = cell(table, row, ID);
    let cited: Vec<Citation> = row.cells.iter().flat_map(|text| citations(text)).collect();
    if cited.is_empty() {
        let problem = format!("row {id} cites no path:line");
        return vec![violation(path, row.line, &problem)];
    }
    let mut violations = Vec::new();
    for Citation {
        path: file,
        start,
        end,
    } in &cited
    {
        let count = *counts
            .entry(file.clone())
            .or_insert_with(|| repo.line_count(file).ok().flatten());
        let problem = match count {
            None => format!("row {id} cites `{file}`, which is no readable repository file"),
            Some(lines) if !((1..=*end).contains(start) && *end <= lines) => format!(
                "row {id} cites {file}:L{start}-{end}, outside 1..={lines}, the file's line count"
            ),
            Some(_) => continue,
        };
        violations.push(violation(path, row.line, &problem));
    }
    violations
}

/// Test 6: every required program, job and BMS map has a data row in its audit.
fn required_programs_have_rows_violations(
    parity: &Document,
    validation: &Document,
    function_map: &Document,
) -> Vec<String> {
    let checks: [(&str, &Document, &[&str], &[&str]); 3] = [
        (
            PARITY_AUDIT_PATH,
            parity,
            &PARITY_PROGRAMS,
            &[PROGRAM, PARAGRAPH],
        ),
        (
            VALIDATION_AUDIT_PATH,
            validation,
            &VALIDATION_PROGRAMS,
            &[VALIDATION_PROGRAM],
        ),
        (FUNCTION_MAP_PATH, function_map, &BMS_MAPS, &[BMS_MAP]),
    ];
    let mut violations = Vec::new();
    for (path, doc, names, columns) in checks {
        let mut found = BTreeSet::new();
        for (table, row) in data_rows(doc) {
            for column in columns {
                found.extend(tokens(cell(table, row, column)));
            }
        }
        let cells = columns.join("` or `");
        for name in names {
            if !found.contains(&name.to_ascii_uppercase()) {
                let problem = format!("no data row names {name} in its `{cells}` cell");
                violations.push(violation(path, FILE_LINE, &problem));
            }
        }
    }
    violations
}

/// Test 7: every E-ID written in a data, handoff or review-log table of the four files is
/// defined by the escalation log, and every `## Escalations raised` handoff table of the
/// three audits carries the log's columns and rows the log consolidates.
fn escalation_references_resolve_violations(
    parity: &Document,
    validation: &Document,
    function_map: &Document,
    escalations: &Document,
) -> Vec<String> {
    let defined = ids_with_prefix(escalations, IdKind::Escalation);
    let files = [
        (PARITY_AUDIT_PATH, parity),
        (VALIDATION_AUDIT_PATH, validation),
        (FUNCTION_MAP_PATH, function_map),
        (ESCALATIONS_PATH, escalations),
    ];
    let mut violations = Vec::new();
    for (path, doc) in files {
        violations.extend(reference_violations(path, doc, &defined));
    }
    for audit in audits(parity, validation, function_map) {
        for table in audit.doc.tables().iter().filter(|table| is_handoff(table)) {
            violations.extend(handoff_violations(audit.path, table, &defined));
        }
    }
    violations
}

/// One violation per distinct undefined E-ID of each row of a data, handoff or review-log
/// table of `doc`.
fn reference_violations(path: &str, doc: &Document, defined: &BTreeSet<String>) -> Vec<String> {
    let mut violations = Vec::new();
    for table in doc
        .tables()
        .iter()
        .filter(|table| table.is_data_table() || table.is_review_log())
    {
        for row in &table.rows {
            let unknown: BTreeSet<String> = row
                .cells
                .iter()
                .flat_map(|text| id_tokens(text))
                .filter(|(kind, id)| *kind == IdKind::Escalation && !defined.contains(id))
                .map(|(_, id)| id)
                .collect();
            for id in unknown {
                let problem = format!("{id} is not defined in {ESCALATIONS_PATH}");
                violations.push(violation(path, row.line, &problem));
            }
        }
    }
    violations
}

/// The column and ID violations of one `## Escalations raised` handoff table.
fn handoff_violations(path: &str, table: &Table, defined: &BTreeSet<String>) -> Vec<String> {
    let mut violations = missing_columns(path, table, &ESCALATION_COLUMNS);
    for row in &table.rows {
        let id = cell(table, row, ID);
        let problem = match id_problem(id, IdKind::Escalation) {
            Some(problem) => problem,
            None if !defined.contains(id) => {
                format!("handoff row {id} is not consolidated in {ESCALATIONS_PATH}")
            }
            None => continue,
        };
        violations.push(violation(path, row.line, &problem));
    }
    violations
}

/// Test 8: every review-log row names a data row of its own audit (of its own `##` section
/// in the parity audit), records `pass` or `fail`, and names its reviewer. Verdicts are not
/// required to be `pass`.
fn review_logs_are_well_formed_violations(
    parity: &Document,
    validation: &Document,
    function_map: &Document,
) -> Vec<String> {
    let mut violations = Vec::new();
    for audit in audits(parity, validation, function_map) {
        let mut by_section: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (table, row) in data_rows(audit.doc) {
            let id = cell(table, row, ID);
            if !id.is_empty() {
                by_section.entry(section_of(table)).or_default().insert(id);
            }
        }
        let every: BTreeSet<&str> = by_section.values().flatten().copied().collect();
        for table in audit
            .doc
            .tables()
            .iter()
            .filter(|table| table.is_review_log())
        {
            let (scope, label) = if audit.kind == IdKind::Parity {
                let section = section_of(table);
                let scope = by_section.get(section).cloned().unwrap_or_default();
                (scope, format!("section `## {section}`"))
            } else {
                (every.clone(), "this file".to_owned())
            };
            violations.extend(review_log_violations(audit.path, table, &scope, &label));
        }
    }
    violations
}

/// The column, row ID, verdict and reviewer violations of one review log, whose rows may
/// name only the IDs in `scope`, described by `label`. A column the log lacks is reported
/// once by [`missing_columns`], not again per row.
fn review_log_violations(
    path: &str,
    table: &Table,
    scope: &BTreeSet<&str>,
    label: &str,
) -> Vec<String> {
    let mut violations = missing_columns(path, table, &REVIEW_LOG_COLUMNS);
    let verdicts = VERDICTS.join("` or `");
    for row in &table.rows {
        let id = cell(table, row, ROW_ID);
        if !scope.contains(id) {
            let problem = format!("review row `{id}` names no data row of {label}");
            violations.push(violation(path, row.line, &problem));
        }
        let verdict = cell(table, row, VERDICT);
        if table.column(VERDICT).is_some() && !VERDICTS.contains(&verdict) {
            let problem = format!("review row `{id}` has verdict `{verdict}`, not `{verdicts}`");
            violations.push(violation(path, row.line, &problem));
        }
        if table.column(REVIEWER).is_some() && cell(table, row, REVIEWER).is_empty() {
            let problem = format!("review row `{id}` names no reviewer");
            violations.push(violation(path, row.line, &problem));
        }
    }
    violations
}

#[test]
fn parity_audit_sections_and_columns() {
    let parity = load(&repo(), PARITY_AUDIT_PATH);
    report(
        "parity_audit_sections_and_columns",
        &parity_audit_sections_and_columns_violations(&parity),
    );
}

#[test]
fn validation_audit_and_function_map_columns() {
    let repo = repo();
    let validation = load(&repo, VALIDATION_AUDIT_PATH);
    let function_map = load(&repo, FUNCTION_MAP_PATH);
    report(
        "validation_audit_and_function_map_columns",
        &validation_audit_and_function_map_columns_violations(&validation, &function_map),
    );
}

#[test]
fn escalation_log_structure() {
    let escalations = load(&repo(), ESCALATIONS_PATH);
    report(
        "escalation_log_structure",
        &escalation_log_structure_violations(&escalations),
    );
}

#[test]
fn ids_are_well_formed_and_unique() {
    let repo = repo();
    let parity = load(&repo, PARITY_AUDIT_PATH);
    let validation = load(&repo, VALIDATION_AUDIT_PATH);
    let function_map = load(&repo, FUNCTION_MAP_PATH);
    report(
        "ids_are_well_formed_and_unique",
        &ids_are_well_formed_and_unique_violations(&parity, &validation, &function_map),
    );
}

#[test]
fn citations_resolve_to_repository_lines() {
    let repo = repo();
    let parity = load(&repo, PARITY_AUDIT_PATH);
    let validation = load(&repo, VALIDATION_AUDIT_PATH);
    let function_map = load(&repo, FUNCTION_MAP_PATH);
    report(
        "citations_resolve_to_repository_lines",
        &citations_resolve_to_repository_lines_violations(
            &parity,
            &validation,
            &function_map,
            &repo,
        ),
    );
}

#[test]
fn required_programs_have_rows() {
    let repo = repo();
    let parity = load(&repo, PARITY_AUDIT_PATH);
    let validation = load(&repo, VALIDATION_AUDIT_PATH);
    let function_map = load(&repo, FUNCTION_MAP_PATH);
    report(
        "required_programs_have_rows",
        &required_programs_have_rows_violations(&parity, &validation, &function_map),
    );
}

#[test]
fn escalation_references_resolve() {
    let repo = repo();
    let parity = load(&repo, PARITY_AUDIT_PATH);
    let validation = load(&repo, VALIDATION_AUDIT_PATH);
    let function_map = load(&repo, FUNCTION_MAP_PATH);
    let escalations = load(&repo, ESCALATIONS_PATH);
    report(
        "escalation_references_resolve",
        &escalation_references_resolve_violations(
            &parity,
            &validation,
            &function_map,
            &escalations,
        ),
    );
}

#[test]
fn review_logs_are_well_formed() {
    let repo = repo();
    let parity = load(&repo, PARITY_AUDIT_PATH);
    let validation = load(&repo, VALIDATION_AUDIT_PATH);
    let function_map = load(&repo, FUNCTION_MAP_PATH);
    report(
        "review_logs_are_well_formed",
        &review_logs_are_well_formed_violations(&parity, &validation, &function_map),
    );
}
