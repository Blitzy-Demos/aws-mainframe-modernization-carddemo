//! Pre-code structure check of the derive-lane parity audits (AAP 0.9.1).
//!
//! Every section of `docs/parity/parity-oracle-audit.md`, `docs/parity/validation-audit.md`
//! and `docs/parity/function-map.md`, and the escalation log `docs/parity/escalations.md`,
//! passes this check before its COBOL-informed review; the code a section gates is written
//! only after both. The check proves structure: the sections and columns each audit
//! declares, well-formed and unique row IDs whose key equals the row's `Program` cell (the
//! `BMS map` cell in the function map), `path:line` citations that land on lines of
//! repository files, rows for every carried program and BMS map, escalation references that
//! resolve to the log, and well-formed review logs. It never requires a verdict to be
//! `pass`: passing review is recorded in `docs/parity/signoff.md`, which this check does not
//! read.
//!
//! The checks select their tables by first column: `ID` for data, handoff and log tables,
//! `Row ID` for review logs. So that a malformed table cannot drop out of that selection
//! unnoticed, tests 1 to 3 also report every table that its headings or its header's
//! columns mark as an audit table but whose first column differs, and every line starting
//! with `|` that the parser reads into no table, as when a delimiter row is lost.
//!
//! Run it alone with `cargo test -p carddemo-audit --test audit_structure`. It uses `std` and
//! [`carddemo_audit`] only, so it builds before any other workspace crate exists.
//!
//! Each `#[test]` over the committed documents reports every violation its `*_violations`
//! function collects, one `<file>:<line>: <problem>` per line, so a single run lists every
//! defect. The other tests run those functions over inline markdown holding known defects,
//! so a check that stops reporting one fails.

use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

use carddemo_audit::{
    Citation, Document, ESCALATION_COLUMNS, ESCALATION_STATUSES, ESCALATIONS_PATH,
    ESCALATIONS_RAISED, FUNCTION_MAP_COLUMNS, FUNCTION_MAP_PATH, IdKind, PARITY_AUDIT_PATH,
    PARITY_COMMON_COLUMNS, PARITY_LAYOUT_COLUMNS, PARITY_RULE_COLUMNS, PARITY_SECTIONS,
    REVIEW_LOG_COLUMNS, Repo, Row, Table, VALIDATION_AUDIT_PATH, VALIDATION_COLUMNS, citations,
    classify_id, code_span, id_tokens, ids_with_prefix, is_no_message, malformed_citations,
    no_message_escalation,
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

/// One of the three audits, with the path its violations name, the ID kind its data rows
/// define and the column that keys those IDs.
#[derive(Clone, Copy)]
struct Audit<'a> {
    path: &'static str,
    doc: &'a Document,
    kind: IdKind,
    /// The column whose trimmed cell each data row's ID key ([`id_key`]) equals: `Program`
    /// for `P-<PROGRAM>-<nnn>` and `V-<PROGRAM>-<nnn>`, `BMS map` for `F-<MAP>-<nnn>`.
    key_column: &'static str,
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
            key_column: PROGRAM,
        },
        Audit {
            path: VALIDATION_AUDIT_PATH,
            doc: validation,
            kind: IdKind::Validation,
            key_column: VALIDATION_PROGRAM,
        },
        Audit {
            path: FUNCTION_MAP_PATH,
            doc: function_map,
            kind: IdKind::Function,
            key_column: BMS_MAP,
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

/// An audit data table as the later checks select it: first column `ID`, outside
/// `## Escalations raised`. Tests 1 to 3 first report, through
/// [`recognition_file_violations`], every table that its headings or header mark as an
/// audit table but that fails this selection, and every pipe-table line read into no
/// table, so a malformed table cannot leave the checks unnoticed.
fn is_data(table: &Table) -> bool {
    table.is_data_table() && !table.is_escalations_raised()
}

/// A handoff table as the later checks select it: first column `ID`, under
/// `## Escalations raised`. A table there with another first column is reported by
/// [`recognition_violations`].
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

/// The key of a P-, V- or F- `id` that [`classify_id`] accepts: the text between its prefix
/// and its final `-<nnn>`, a multi-segment key with its segments joined by `-` as written
/// (`COACTUPC` of `V-COACTUPC-012`, `AB-CD` of `P-AB-CD-001`).
fn id_key(id: &str) -> Option<&str> {
    let (_, rest) = id.split_once('-')?;
    let (key, _) = rest.rsplit_once('-')?;
    Some(key)
}

/// The violation of a data row whose well-formed `id` has a key ([`id_key`]) other than the
/// row's trimmed `column` cell, compared exactly: the ID patterns `P-<PROGRAM>-<nnn>`,
/// `V-<PROGRAM>-<nnn>` and `F-<MAP>-<nnn>` key each ID by its `Program` or `BMS map` cell.
/// A table lacking `column` is reported by [`missing_columns`] and not again per row.
fn id_key_violation(
    path: &str,
    table: &Table,
    row: &Row,
    id: &str,
    column: &str,
) -> Option<String> {
    let key = id_key(id)?;
    let value = table.column(column).map(|_| cell(table, row, column))?;
    if key == value {
        return None;
    }
    let problem = if value.is_empty() {
        format!("`{id}` key `{key}` differs from its empty `{column}` cell")
    } else {
        format!("`{id}` key `{key}` differs from its `{column}` cell `{value}`")
    };
    Some(violation(path, row.line, &problem))
}

/// The `###` heading every review log sits under starts with this text:
/// `### Review log — <section>`.
const RECOGNITION_REVIEW_LOG_HEADING: &str = "Review log";

/// The role a table's headings give it in an audit file, and with it the first column the
/// checks select a table of that role by.
#[derive(Clone, Copy)]
enum RecognitionRole {
    /// A data table of the audit.
    Data,
    /// A table under `## Escalations raised`.
    Handoff,
    /// A table under a `### Review log` heading.
    ReviewLog,
    /// A table of the escalation log, which holds nothing else.
    Log,
}

impl RecognitionRole {
    /// The first column a table of this role needs: `Row ID` for a review log
    /// ([`Table::is_review_log`]), `ID` otherwise ([`Table::is_data_table`]).
    fn first_column(self) -> &'static str {
        match self {
            Self::ReviewLog => ROW_ID,
            Self::Data | Self::Handoff | Self::Log => ID,
        }
    }

    /// The role as a violation names it.
    fn noun(self) -> &'static str {
        match self {
            Self::Data => "a data table",
            Self::Handoff => "a handoff table",
            Self::ReviewLog => "a review log",
            Self::Log => "the escalation log",
        }
    }
}

/// Where one audit file holds audit tables, and which headers mark an audit table anywhere
/// else in it; read by [`recognition_violations`].
struct Recognition {
    /// The `##` sections whose tables, outside a `### Review log` heading, are data tables.
    data_sections: &'static [&'static str],
    /// The headers a table at no such position is measured against: a table holding at
    /// least half of the columns after a header's first is an attempt at that header.
    schemas: &'static [&'static [&'static str]],
    /// Whether every table of the file is the escalation log.
    log: bool,
}

/// The parity audit: data tables under sections A to F.
const RECOGNITION_PARITY: Recognition = Recognition {
    data_sections: &PARITY_SECTIONS,
    schemas: &[
        &PARITY_COMMON_COLUMNS,
        &ESCALATION_COLUMNS,
        &REVIEW_LOG_COLUMNS,
    ],
    log: false,
};

/// The validation audit: data tables under the `##` section of each program.
const RECOGNITION_VALIDATION: Recognition = Recognition {
    data_sections: &VALIDATION_PROGRAMS,
    schemas: &[
        &VALIDATION_COLUMNS,
        &ESCALATION_COLUMNS,
        &REVIEW_LOG_COLUMNS,
    ],
    log: false,
};

/// The function map: data tables under the `##` section of each BMS map.
const RECOGNITION_FUNCTION_MAP: Recognition = Recognition {
    data_sections: &BMS_MAPS,
    schemas: &[
        &FUNCTION_MAP_COLUMNS,
        &ESCALATION_COLUMNS,
        &REVIEW_LOG_COLUMNS,
    ],
    log: false,
};

/// The escalation log: its one table, and any other, is the log.
const RECOGNITION_ESCALATIONS: Recognition = Recognition {
    data_sections: &[],
    schemas: &[],
    log: true,
};

/// The role the headings of `table` give it under `recognition`, if any. A `### Review log`
/// heading decides before an `## Escalations raised` section, which decides before a data
/// section.
fn recognition_role(table: &Table, recognition: &Recognition) -> Option<RecognitionRole> {
    if recognition.log {
        return Some(RecognitionRole::Log);
    }
    let review = table
        .subsection
        .as_deref()
        .is_some_and(|subsection| subsection.starts_with(RECOGNITION_REVIEW_LOG_HEADING));
    if review {
        return Some(RecognitionRole::ReviewLog);
    }
    if table.is_escalations_raised() {
        return Some(RecognitionRole::Handoff);
    }
    let data = table
        .section
        .as_deref()
        .is_some_and(|section| recognition.data_sections.contains(&section));
    data.then_some(RecognitionRole::Data)
}

/// The first column of the first header in `schemas` of which `table` holds at least half
/// of the columns after the first, if any.
fn recognition_shape(table: &Table, schemas: &[&'static [&'static str]]) -> Option<&'static str> {
    schemas.iter().find_map(|schema| {
        let (first, rest) = schema.split_first()?;
        let held = rest
            .iter()
            .filter(|name| table.column(name).is_some())
            .count();
        (held > 0 && 2 * held >= rest.len()).then_some(*first)
    })
}

/// Where `table` sits, as a violation names it.
fn recognition_place(table: &Table) -> String {
    let section = table.section.as_deref().map(|text| format!("`## {text}`"));
    let subsection = table
        .subsection
        .as_deref()
        .map(|text| format!("`### {text}`"));
    match (section, subsection) {
        (Some(section), Some(subsection)) => format!("table under {section} › {subsection}"),
        (Some(heading), None) | (None, Some(heading)) => format!("table under {heading}"),
        (None, None) => "table before the first `##` heading".to_owned(),
    }
}

/// One violation per table of `doc` that its headings ([`recognition_role`]) or, at no
/// such position, its header ([`recognition_shape`]) mark as an audit table, but whose
/// first column is not the one the checks select that table by. Every other check reads
/// only the tables [`Table::is_data_table`] or [`Table::is_review_log`] accepts, so such a
/// table, a renamed first header for example, would drop out of them unnoticed. A legend
/// such as the parity audit's `## Gates` or the validation audit's `## Status mapping`
/// table has neither the position nor the shape and is not reported.
fn recognition_violations(path: &str, doc: &Document, recognition: &Recognition) -> Vec<String> {
    let mut violations = Vec::new();
    for table in doc.tables() {
        let first = table.header.first().map_or("", String::as_str);
        let problem = if let Some(role) = recognition_role(table, recognition) {
            let expected = role.first_column();
            let noun = role.noun();
            (first != expected).then(|| {
                let place = recognition_place(table);
                format!(
                    "{place} has first column `{first}`, not `{expected}`: it is not read as {noun}"
                )
            })
        } else {
            recognition_shape(table, recognition.schemas)
                .filter(|expected| first != *expected)
                .map(|expected| {
                    let place = recognition_place(table);
                    format!(
                        "{place} has the columns of an audit table but first column `{first}`, \
                         not `{expected}`: it is not read as one"
                    )
                })
        };
        if let Some(problem) = problem {
            violations.push(violation(path, table.line, &problem));
        }
    }
    violations
}

/// One violation per run of consecutive [`Document::stray_table_lines`] of `doc`, at the
/// run's first line: lines that start with `|` but that the parser reads into no table,
/// because a header or its delimiter row is missing, so their table would drop out of every
/// check unnoticed.
fn recognition_stray_violations(path: &str, doc: &Document) -> Vec<String> {
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for &line in doc.stray_table_lines() {
        match runs.last_mut() {
            Some((_, end)) if *end + 1 == line => *end = line,
            _ => runs.push((line, line)),
        }
    }
    runs.into_iter()
        .map(|(start, end)| {
            let lines = if start == end {
                "pipe-table line".to_owned()
            } else {
                format!("pipe-table lines {start}-{end}")
            };
            let problem =
                format!("{lines} outside any table: a header or delimiter row is missing");
            violation(path, start, &problem)
        })
        .collect()
}

/// The recognition violations of tests 1 to 3: every table of `doc` that should be read as
/// an audit table but is not ([`recognition_violations`]), and every line that should belong
/// to a table but belongs to none ([`recognition_stray_violations`]).
fn recognition_file_violations(
    path: &str,
    doc: &Document,
    recognition: &Recognition,
) -> Vec<String> {
    let mut violations = recognition_violations(path, doc, recognition);
    violations.extend(recognition_stray_violations(path, doc));
    violations
}

/// Test 1: the six sections, their columns, rows and review logs, the cell counts and the
/// table recognition ([`recognition_file_violations`]) of the parity audit.
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
    violations.extend(recognition_file_violations(
        path,
        parity,
        &RECOGNITION_PARITY,
    ));
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

/// Test 2: the columns, rows, review logs, cell counts and table recognition
/// ([`recognition_file_violations`]) of the validation audit and the function map, and the
/// `Status` and `Message text` cells of every validation row.
fn validation_audit_and_function_map_columns_violations(
    validation: &Document,
    function_map: &Document,
) -> Vec<String> {
    let files: [(&str, &Document, &[&str], &Recognition); 2] = [
        (
            VALIDATION_AUDIT_PATH,
            validation,
            &VALIDATION_COLUMNS,
            &RECOGNITION_VALIDATION,
        ),
        (
            FUNCTION_MAP_PATH,
            function_map,
            &FUNCTION_MAP_COLUMNS,
            &RECOGNITION_FUNCTION_MAP,
        ),
    ];
    let mut violations = Vec::new();
    for (path, doc, columns, recognition) in files {
        for table in doc.tables().iter().filter(|table| is_data(table)) {
            violations.extend(missing_columns(path, table, columns));
        }
        let tables: Vec<&Table> = doc.tables().iter().collect();
        violations.extend(rows_and_review_violations(
            path, FILE_LINE, "the file", &tables,
        ));
        violations.extend(cell_count_violations(path, doc));
        violations.extend(recognition_file_violations(path, doc, recognition));
    }
    for (table, row) in data_rows(validation) {
        violations.extend(validation_row_violations(table, row));
    }
    violations
}

/// The `Status` and `Message text` violations of one validation data row. A column the
/// table lacks is reported by [`missing_columns`] and not again per row.
///
/// A message cell starting with `no-message` is valid only in the exact `no-message (E-nn)`
/// form, so a code span after the marker never stands in for the E-ID; only other text may
/// be a code span instead. Test 7 checks that the E-ID is defined by the escalation log:
/// its [`reference_violations`] scans every cell of every data table.
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
        let problem = if is_no_message(message) {
            no_message_escalation(message)
                .is_none()
                .then_some("no-message without an E-ID")
        } else {
            code_span(message)
                .is_none()
                .then_some("message text is neither a code span nor no-message (E-nn)")
        };
        if let Some(problem) = problem {
            violations.push(violation(path, row.line, &format!("row {id}: {problem}")));
        }
    }
    violations
}

/// Test 3: the escalation log's columns, IDs, `Status` cells, cell counts and table
/// recognition ([`recognition_file_violations`]).
///
/// The citations of the log are checked by test 5, with those of the three audits.
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
    violations.extend(recognition_file_violations(
        path,
        escalations,
        &RECOGNITION_ESCALATIONS,
    ));
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

/// Test 4: every data row ID of each audit is well formed for that audit, unique within it,
/// and keyed by its row: the key of a P- or V- ID equals the row's `Program` cell, that of
/// an F- ID the row's `BMS map` cell ([`id_key_violation`]). Handoff and review rows define
/// no audit ID and are not checked here.
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
                violations.extend(id_key_violation(
                    audit.path,
                    table,
                    row,
                    id,
                    audit.key_column,
                ));
            }
        }
    }
    violations
}

/// Test 5: every data row of the three audits cites at least one `path:line`, and every
/// citation in every table of the four documents is well formed and names a repository
/// file and lines inside it. The four documents are the three audits, whose data, handoff,
/// review-log and other tables are all read, and the escalation log. Rows other than audit
/// data rows need not cite, but each citation they hold is checked.
///
/// A cited path is repository-relative and resolves through [`Repo::resolve`], ignoring
/// ASCII case (E-03); a bare file name such as `[CBACT04C.cbl:L188]` names no repository
/// file and is reported. [`Repo::line_count`] reads bytes, because the EBCDIC fixtures are
/// not UTF-8, and counts one line per LF plus an unterminated last line, so CRLF files and
/// files without a final LF count their physical lines. Counts are cached per cited path;
/// `None` stands for a path that resolves to no readable file.
fn citations_resolve_to_repository_lines_violations(
    parity: &Document,
    validation: &Document,
    function_map: &Document,
    escalations: &Document,
    repo: &Repo,
) -> Vec<String> {
    let mut files: Vec<(&str, &Document, BTreeSet<usize>)> =
        audits(parity, validation, function_map)
            .into_iter()
            .map(|audit| {
                let required = data_rows(audit.doc).map(|(_, row)| row.line).collect();
                (audit.path, audit.doc, required)
            })
            .collect();
    files.push((ESCALATIONS_PATH, escalations, BTreeSet::new()));
    let mut counts = BTreeMap::new();
    let mut violations = Vec::new();
    for (path, doc, required) in files {
        for table in doc.tables() {
            for row in &table.rows {
                violations.extend(row_citation_violations(
                    path,
                    table,
                    row,
                    required.contains(&row.line),
                    repo,
                    &mut counts,
                ));
            }
        }
    }
    violations
}

/// How a citation violation names `row`: by its `ID` cell, else by the `Row ID` cell of a
/// review log, else by the line the violation points at alone.
fn citation_row_label(table: &Table, row: &Row) -> String {
    let id = cell(table, row, ID);
    if !id.is_empty() {
        return format!("row {id}");
    }
    let reviewed = cell(table, row, ROW_ID);
    if reviewed.is_empty() {
        "row".to_owned()
    } else {
        format!("review row `{reviewed}`")
    }
}

/// The problem with citing lines `start` to `end` of a file of `lines` lines, if any: every
/// defect the range has, in this order, joined with `; `. Lines count from 1, so a bound of
/// 0 names no line; a start after the end is a reversed range; a bound after the last line
/// lies outside the file. [`citations`] returns ranges as written, so each defect gets its
/// own reason and none hides another. The last line is inside the file: `L20` of a 20-line
/// file has no problem.
fn citation_range_problem(start: usize, end: usize, lines: usize) -> Option<String> {
    let mut defects = Vec::new();
    if start == 0 || end == 0 {
        defects.push("line 0, but lines count from 1".to_owned());
    }
    if start > end {
        defects.push(format!("a reversed range: start {start} > end {end}"));
    }
    if start.max(end) > lines {
        defects.push(format!("outside 1..={lines}, the file's line count"));
    }
    (!defects.is_empty()).then(|| defects.join("; "))
}

/// The citation violations of one row: every bracket shaped like a line citation that
/// [`citations`] rejects (see [`malformed_citations`]), a `required` row citing no
/// `path:line`, every citation naming no readable repository file, and every citation whose
/// lines [`citation_range_problem`] rejects, with each of its reasons.
fn row_citation_violations(
    path: &str,
    table: &Table,
    row: &Row,
    required: bool,
    repo: &Repo,
    counts: &mut BTreeMap<String, Option<usize>>,
) -> Vec<String> {
    let label = citation_row_label(table, row);
    let mut violations: Vec<String> = row
        .cells
        .iter()
        .flat_map(|text| malformed_citations(text))
        .map(|bracket| {
            let problem = format!(
                "{label} has the malformed citation `{bracket}`: not `path:L<n>` or \
                 `path:L<n>-<m>` with line numbers that fit usize"
            );
            violation(path, row.line, &problem)
        })
        .collect();
    let cited: Vec<Citation> = row.cells.iter().flat_map(|text| citations(text)).collect();
    if cited.is_empty() {
        if required {
            let problem = format!("{label} cites no path:line");
            violations.push(violation(path, row.line, &problem));
        }
        return violations;
    }
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
            None => format!("{label} cites `{file}`, which is no readable repository file"),
            Some(lines) => match citation_range_problem(*start, *end, lines) {
                Some(range) => format!("{label} cites {file}:L{start}-{end}, {range}"),
                None => continue,
            },
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

/// The ID of the one data row of the inline audit [`no_message_validation`] builds.
const NO_MESSAGE_ROW_ID: &str = "V-COACTUPC-001";

/// A validation audit parsed from inline markdown: one `## COACTUPC` table with the
/// [`VALIDATION_COLUMNS`] header and one data row whose `Message text` cell is `message`.
fn no_message_validation(message: &str) -> Document {
    let header = VALIDATION_COLUMNS.join(" | ");
    let delimiter = ["---"; VALIDATION_COLUMNS.len()].join("|");
    let row = [
        NO_MESSAGE_ROW_ID,
        "COACTUPC",
        "9700-CHECK-CHANGE-IN-REC [app/cbl/COACTUPC.cbl:L4109-4193]",
        "PUT body without version token; kind=Invalid",
        message,
        "400",
        "—",
        "1",
    ]
    .join(" | ");
    Document::parse(&format!(
        "## COACTUPC\n\n| {header} |\n|{delimiter}|\n| {row} |\n"
    ))
}

/// The one table and data row of an inline audit [`no_message_validation`] builds.
fn no_message_row(validation: &Document) -> (&Table, &Row) {
    let [table] = validation.tables() else {
        panic!("the inline validation audit parses to exactly one table");
    };
    let [row] = table.rows.as_slice() else {
        panic!("the inline validation table parses to exactly one data row");
    };
    (table, row)
}

/// An escalation log parsed from inline markdown: one `## Log` table with the
/// [`ESCALATION_COLUMNS`] header that defines `E-24` and no other E-ID.
fn no_message_escalation_log() -> Document {
    let header = ESCALATION_COLUMNS.join(" | ");
    let delimiter = ["---"; ESCALATION_COLUMNS.len()].join("|");
    let row = [
        "E-24",
        "#1 → #2",
        "PUT without version token",
        "[app/cbl/COACTUPC.cbl:L4109-4193]",
        "400 with no message",
        "AAP 0.3.7",
        "resolved",
    ]
    .join(" | ");
    Document::parse(&format!(
        "## Log\n\n| {header} |\n|{delimiter}|\n| {row} |\n"
    ))
}

/// A `no-message` cell passes the row check only as `no-message (E-nn)`, whatever follows
/// the marker; any other message text must be a code span.
#[test]
fn no_message_cells_link_an_escalation_id() {
    let no_id = "no-message without an E-ID";
    let neither = "message text is neither a code span nor no-message (E-nn)";
    let cases: [(&str, Option<&str>); 6] = [
        ("no-message `none`", Some(no_id)),
        ("no-message", Some(no_id)),
        ("no-message (E-5)", Some(no_id)),
        ("Synthetic message text", Some(neither)),
        ("`\"Synthetic message text\"`", None),
        ("no-message (E-24)", None),
    ];
    for (message, problem) in cases {
        let validation = no_message_validation(message);
        let (table, row) = no_message_row(&validation);
        assert_eq!(
            cell(table, row, MESSAGE_TEXT),
            message,
            "parsed message cell"
        );
        let expected: Vec<String> = problem
            .map(|problem| {
                let problem = format!("row {NO_MESSAGE_ROW_ID}: {problem}");
                violation(VALIDATION_AUDIT_PATH, row.line, &problem)
            })
            .into_iter()
            .collect();
        assert_eq!(
            validation_row_violations(table, row),
            expected,
            "message cell {message:?}"
        );
    }
}

/// A well-formed `no-message (E-nn)` cell whose E-ID the escalation log does not define
/// passes the row check and is reported by the escalation reference check of test 7.
#[test]
fn no_message_escalation_ids_resolve_to_the_log() {
    let escalations = no_message_escalation_log();
    let empty = Document::default();
    for (message, unknown) in [
        ("no-message (E-24)", None),
        ("no-message (E-99)", Some("E-99")),
    ] {
        let validation = no_message_validation(message);
        let (table, row) = no_message_row(&validation);
        assert_eq!(
            validation_row_violations(table, row),
            Vec::<String>::new(),
            "row check of message cell {message:?}"
        );
        let expected: Vec<String> = unknown
            .map(|id| {
                let problem = format!("{id} is not defined in {ESCALATIONS_PATH}");
                violation(VALIDATION_AUDIT_PATH, row.line, &problem)
            })
            .into_iter()
            .collect();
        assert_eq!(
            escalation_references_resolve_violations(&empty, &validation, &empty, &escalations),
            expected,
            "reference check of message cell {message:?}"
        );
    }
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

/// An inline audit of test 4: under `## <section>`, one data table with `columns` as its
/// header and one row per `(id, key)` of `rows`, holding `id` in the first cell, `key` in
/// the second (the `Program` or `BMS map` column of the audits' headers) and `x` in every
/// other cell.
fn id_key_audit(section: &str, columns: &[&str], rows: &[(&str, &str)]) -> String {
    let mut lines = vec![
        format!("## {section}"),
        String::new(),
        format!("| {} |", columns.join(" | ")),
        format!("|{}", "---|".repeat(columns.len())),
    ];
    for (id, key) in rows {
        let mut cells = vec![*id, *key];
        cells.resize(columns.len(), "x");
        lines.push(format!("| {} |", cells.join(" | ")));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// One case of [`id_keys_equal_their_program_or_map_cells`]: the inline audit `text` read
/// as the audit at `path`, and the violations test 4 must report for it, in order, each as
/// the ID of the row it points at and its problem. A case expecting none is a positive
/// control.
struct IdKeyCase {
    label: &'static str,
    path: &'static str,
    text: String,
    expected: Vec<(&'static str, &'static str)>,
}

/// The cases of [`id_keys_equal_their_program_or_map_cells`]: rows of each audit keyed by
/// another program or map, an empty and a lower-case key cell, multi-segment keys, rows
/// keyed by their own cells, and a table lacking the key column.
fn id_key_cases() -> [IdKeyCase; 6] {
    let mut renamed = VALIDATION_COLUMNS;
    renamed[1] = "Programme";
    let case = |label, path, text, expected| IdKeyCase {
        label,
        path,
        text,
        expected,
    };
    [
        case(
            "validation row keyed by another program",
            VALIDATION_AUDIT_PATH,
            id_key_audit(
                "COACTUPC",
                &VALIDATION_COLUMNS,
                &[
                    ("V-COACTUPC-001", "COACTUPC"),
                    ("V-COSGN00C-901", "COACTUPC"),
                ],
            ),
            vec![(
                "V-COSGN00C-901",
                "`V-COSGN00C-901` key `COSGN00C` differs from its `Program` cell `COACTUPC`",
            )],
        ),
        case(
            "validation rows with an empty and a lower-case `Program` cell",
            VALIDATION_AUDIT_PATH,
            id_key_audit(
                "COACTUPC",
                &VALIDATION_COLUMNS,
                &[("V-COACTUPC-003", ""), ("V-COACTUPC-004", "coactupc")],
            ),
            vec![
                (
                    "V-COACTUPC-003",
                    "`V-COACTUPC-003` key `COACTUPC` differs from its empty `Program` cell",
                ),
                (
                    "V-COACTUPC-004",
                    "`V-COACTUPC-004` key `COACTUPC` differs from its `Program` cell `coactupc`",
                ),
            ],
        ),
        case(
            "parity rows keyed by another program, a multi-segment key included",
            PARITY_AUDIT_PATH,
            id_key_audit(
                PARITY_SECTIONS[1],
                &PARITY_COMMON_COLUMNS,
                &[("P-CBTRN02C-004", "CBACT04C"), ("P-AB-CD-002", "CD")],
            ),
            vec![
                (
                    "P-CBTRN02C-004",
                    "`P-CBTRN02C-004` key `CBTRN02C` differs from its `Program` cell `CBACT04C`",
                ),
                (
                    "P-AB-CD-002",
                    "`P-AB-CD-002` key `AB-CD` differs from its `Program` cell `CD`",
                ),
            ],
        ),
        case(
            "function map row keyed by another map",
            FUNCTION_MAP_PATH,
            id_key_audit(
                BMS_MAPS[9],
                &FUNCTION_MAP_COLUMNS,
                &[("F-COSGN00-001", "COSGN00"), ("F-COMEN01-001", "COSGN00")],
            ),
            vec![(
                "F-COMEN01-001",
                "`F-COMEN01-001` key `COMEN01` differs from its `BMS map` cell `COSGN00`",
            )],
        ),
        case(
            "parity rows keyed by their `Program` cells, a multi-segment key included",
            PARITY_AUDIT_PATH,
            id_key_audit(
                PARITY_SECTIONS[0],
                &PARITY_COMMON_COLUMNS,
                &[("P-CVACT01Y-001", "CVACT01Y"), ("P-AB-CD-001", "AB-CD")],
            ),
            Vec::new(),
        ),
        case(
            "validation table lacking the `Program` column",
            VALIDATION_AUDIT_PATH,
            id_key_audit("COACTUPC", &renamed, &[("V-COSGN00C-901", "COACTUPC")]),
            Vec::new(),
        ),
    ]
}

/// The key of a well-formed P-, V- or F- ID must equal its row's `Program` or `BMS map`
/// cell exactly; a table lacking that column is left to tests 1 and 2.
#[test]
fn id_keys_equal_their_program_or_map_cells() {
    let empty = Document::default();
    for case in id_key_cases() {
        let doc = Document::parse(&case.text);
        let [parity, validation, function_map] =
            [PARITY_AUDIT_PATH, VALIDATION_AUDIT_PATH, FUNCTION_MAP_PATH]
                .map(|audit| if audit == case.path { &doc } else { &empty });
        let expected: Vec<String> = case
            .expected
            .iter()
            .map(|(id, problem)| violation(case.path, recognition_line(&case.text, id), problem))
            .collect();
        assert_eq!(
            ids_are_well_formed_and_unique_violations(parity, validation, function_map),
            expected,
            "{}",
            case.label
        );
    }
}

#[test]
fn citations_resolve_to_repository_lines() {
    let repo = repo();
    let parity = load(&repo, PARITY_AUDIT_PATH);
    let validation = load(&repo, VALIDATION_AUDIT_PATH);
    let function_map = load(&repo, FUNCTION_MAP_PATH);
    let escalations = load(&repo, ESCALATIONS_PATH);
    report(
        "citations_resolve_to_repository_lines",
        &citations_resolve_to_repository_lines_violations(
            &parity,
            &validation,
            &function_map,
            &escalations,
            &repo,
        ),
    );
}

/// The header and delimiter rows of an escalation log or `## Escalations raised` table.
const CITATION_ESCALATION_HEADER: [&str; 2] = [
    "| ID | Lane change | Trigger | Evidence | Decision | Deciding request clause | Status |",
    "|---|---|---|---|---|---|---|",
];

/// One inline case of test 5: the markdown `lines` of a stand-in for the document at
/// `path`, and the violations they must yield, in order, each as the 1-based line it points
/// at and a fragment of its problem. A case expecting none is a positive control.
struct CitationCase {
    label: &'static str,
    path: &'static str,
    lines: &'static [&'static str],
    expected: &'static [(usize, &'static str)],
}

/// The inline cases of test 5. Each cites real repository files, so the line counts are
/// those of the protected oracle: `CBACT04C.cbl` 652, `CBTRN02C.cbl` 731, `COSGN00C.cbl` 260
/// and `COSGN00.bms` 210.
const CITATION_CASES: [CitationCase; 8] = [
    CitationCase {
        label: "an audit data row: an overflowed citation beside a valid one",
        path: FUNCTION_MAP_PATH,
        lines: &[
            "## COSGN00",
            "",
            "| ID | BMS map | Key or field | COBOL action | UI control | Endpoint |",
            "|---|---|---|---|---|---|",
            "| F-COSGN00-001 | COSGN00 | ENTER [app/bms/COSGN00.bms:L1] | MAIN-PARA \
             [app/cbl/CBACT04C.cbl:L99999999999999999999999] | button | POST /auth/login |",
        ],
        expected: &[(
            5,
            "row F-COSGN00-001 has the malformed citation \
             `[app/cbl/CBACT04C.cbl:L99999999999999999999999]`",
        )],
    },
    CitationCase {
        label: "an audit data row whose only citations are malformed",
        path: FUNCTION_MAP_PATH,
        lines: &[
            "## COSGN00",
            "",
            "| ID | BMS map | Key or field | COBOL action | UI control | Endpoint |",
            "|---|---|---|---|---|---|",
            "| F-COSGN00-002 | COSGN00 | PF3 | [app/cbl/COSGN00C.cbl:L88, RETURN-TO-PREV] \
             [app/cbl/COSGN00C.cbl:L1-99999999999999999999999] | link | POST /auth/logout |",
        ],
        expected: &[
            (
                5,
                "row F-COSGN00-002 has the malformed citation \
                 `[app/cbl/COSGN00C.cbl:L88, RETURN-TO-PREV]`",
            ),
            (
                5,
                "row F-COSGN00-002 has the malformed citation \
                 `[app/cbl/COSGN00C.cbl:L1-99999999999999999999999]`",
            ),
            (5, "row F-COSGN00-002 cites no path:line"),
        ],
    },
    CitationCase {
        label: "escalation log Evidence cells: beyond the file, a bare name, malformed",
        path: ESCALATIONS_PATH,
        lines: &[
            "## Log",
            "",
            CITATION_ESCALATION_HEADER[0],
            CITATION_ESCALATION_HEADER[1],
            "| E-15 | #1 → #2 | Rejects | [app/cbl/CBTRN02C.cbl:L393-422] \
             [app/cbl/CBTRN02C.cbl:L999999] | Reproduced | Functional parity | resolved |",
            "| E-16 | #1 → #2 | Purge | [CBPAUP0C.cbl:L156] | Reproduced | Functional parity | \
             resolved |",
            "| E-17 | #1 → #2 | Filter | [app/cbl/COTRN00C.cbl:L206-36O] | Optional filter | \
             Online transactions | resolved |",
        ],
        expected: &[
            (
                5,
                "row E-15 cites app/cbl/CBTRN02C.cbl:L999999-999999, outside 1..=731",
            ),
            (
                6,
                "row E-16 cites `CBPAUP0C.cbl`, which is no readable repository file",
            ),
            (
                7,
                "row E-17 has the malformed citation `[app/cbl/COTRN00C.cbl:L206-36O]`",
            ),
        ],
    },
    CitationCase {
        label: "an audit handoff row: beyond the file and malformed",
        path: PARITY_AUDIT_PATH,
        lines: &[
            "## Escalations raised",
            "",
            CITATION_ESCALATION_HEADER[0],
            CITATION_ESCALATION_HEADER[1],
            "| E-14 | #1 → #2 | Last account | [app/cbl/CBACT04C.cbl:L188-9999] | Reproduced | \
             Functional parity | resolved |",
            "| E-15 | #1 → #2 | Rejects | [app/cbl/CBTRN02C.cbl:L393-422, L5x] | Reproduced | \
             Functional parity | resolved |",
        ],
        expected: &[
            (
                5,
                "row E-14 cites app/cbl/CBACT04C.cbl:L188-9999, outside 1..=652",
            ),
            (
                6,
                "row E-15 has the malformed citation `[app/cbl/CBTRN02C.cbl:L393-422, L5x]`",
            ),
        ],
    },
    CitationCase {
        label: "a review log Note: malformed, reversed and unresolved citations",
        path: VALIDATION_AUDIT_PATH,
        lines: &[
            "## COSGN00C",
            "",
            "### Review log — validation-audit",
            "",
            "| Row ID | Verdict | Note | Reviewer |",
            "|---|---|---|---|",
            "| V-COSGN00C-001 | pass | re-read [app/cbl/COSGN00C.cbl:L12a] \
             [app/cbl/COSGN00C.cbl:L140-120] [app/cbl/NOSUCH.cbl:L1] | reviewer |",
        ],
        expected: &[
            (
                7,
                "review row `V-COSGN00C-001` has the malformed citation \
                 `[app/cbl/COSGN00C.cbl:L12a]`",
            ),
            (
                7,
                "review row `V-COSGN00C-001` cites app/cbl/COSGN00C.cbl:L140-120, a reversed \
                 range: start 140 > end 120",
            ),
            (
                7,
                "review row `V-COSGN00C-001` cites `app/cbl/NOSUCH.cbl`, which is no readable",
            ),
        ],
    },
    CitationCase {
        label: "audit data rows: line 0, a reversed range, past the last line, no file, none",
        path: PARITY_AUDIT_PATH,
        lines: &[
            "## B. Per-program rules",
            "",
            "| ID | Program | Paragraph | Copybook field or PIC clause | Input condition | \
             Expected result | Persists on later failure |",
            "|---|---|---|---|---|---|---|",
            "| P-CBACT04C-001 | CBACT04C | 1300-COMPUTE-INTEREST [app/cbl/CBACT04C.cbl:L0] | x | \
             y | z [app/cbl/CBACT04C.cbl:L470-460] [app/cbl/CBACT04C.cbl:L653] | no |",
            "| P-CBACT04C-002 | CBACT04C | 1300-COMPUTE-INTEREST | x | y | \
             z [app/cbl/NOSUCH.cbl:L1] | no |",
            "| P-CBACT04C-003 | CBACT04C | 1300-COMPUTE-INTEREST | x | y | z | no |",
        ],
        expected: &[
            (
                5,
                "row P-CBACT04C-001 cites app/cbl/CBACT04C.cbl:L0-0, line 0, but lines count \
                 from 1",
            ),
            (
                5,
                "row P-CBACT04C-001 cites app/cbl/CBACT04C.cbl:L470-460, a reversed range: \
                 start 470 > end 460",
            ),
            (
                5,
                "row P-CBACT04C-001 cites app/cbl/CBACT04C.cbl:L653-653, outside 1..=652",
            ),
            (
                6,
                "row P-CBACT04C-002 cites `app/cbl/NOSUCH.cbl`, which is no readable",
            ),
            (7, "row P-CBACT04C-003 cites no path:line"),
        ],
    },
    CitationCase {
        label: "positive control: valid and stored-case-insensitive citations, other brackets, \
                and review, handoff and other rows without a citation",
        path: PARITY_AUDIT_PATH,
        lines: &[
            "## Gates",
            "",
            "| Section | Content | Gates |",
            "|---|---|---|",
            "| B. Per-program rules | paragraph decisions [exhaustive] | rules |",
            "",
            "## B. Per-program rules",
            "",
            "| ID | Program | Paragraph | Copybook field or PIC clause | Input condition | \
             Expected result | Persists on later failure |",
            "|---|---|---|---|---|---|---|",
            "| P-CBACT04C-001 | CBACT04C | 1300-COMPUTE-INTEREST [app/cbl/CBACT04C.cbl:L1] | \
             [app/jcl/DUSRSECJ.jcl:STEP01] items=[] | \
             [app/data/ASCII/dailytran.txt:type columns 17-22] [E-09] | \
             [see [app/cbl/cbact04c.cbl:L652]] [app/cbl/CBSTM03A.cbl:L1-5] \
             `[app/cbl/X.cbl:L0]` | no |",
            "",
            "### Review log — B. Per-program rules",
            "",
            "| Row ID | Verdict | Note | Reviewer |",
            "|---|---|---|---|",
            "| P-CBACT04C-001 | pass | re-read 1300-COMPUTE-INTEREST | reviewer |",
            "",
            "## Escalations raised",
            "",
            CITATION_ESCALATION_HEADER[0],
            CITATION_ESCALATION_HEADER[1],
            "| E-14 | #1 → #2 | Last account | 50 accounts | Reproduced | Functional parity | \
             resolved |",
        ],
        expected: &[],
    },
    CitationCase {
        label: "positive control: escalation log rows with valid citations or none",
        path: ESCALATIONS_PATH,
        lines: &[
            "## Log",
            "",
            CITATION_ESCALATION_HEADER[0],
            CITATION_ESCALATION_HEADER[1],
            "| E-01 | #1 → #2 | Copybook count | 62 vs 30 [app/cpy] [README.md:L269-294] | \
             All read | Source inventory | resolved |",
            "| E-10 | #1 → #2 | Version floor | no 7.x release | 0.17.0 | Pinned stack | \
             open: user confirms |",
        ],
        expected: &[],
    },
];

/// The test 5 violations of `case`: its lines parsed as the document at its path, with
/// every other document of the four empty.
fn citation_case_violations(case: &CitationCase, repo: &Repo) -> Vec<String> {
    let text = case.lines.join("\n");
    let [parity, validation, function_map, escalations] = [
        PARITY_AUDIT_PATH,
        VALIDATION_AUDIT_PATH,
        FUNCTION_MAP_PATH,
        ESCALATIONS_PATH,
    ]
    .map(|rel| {
        if rel == case.path {
            Document::parse(&text)
        } else {
            Document::default()
        }
    });
    citations_resolve_to_repository_lines_violations(
        &parity,
        &validation,
        &function_map,
        &escalations,
        repo,
    )
}

/// The mismatch between the violations `found` for `case` and those it expects, if any.
fn citation_case_mismatch(case: &CitationCase, found: &[String]) -> Option<String> {
    let matches = found.len() == case.expected.len()
        && found
            .iter()
            .zip(case.expected)
            .all(|(text, (line, fragment))| {
                text.starts_with(&format!("{}:{line}: ", case.path)) && text.contains(fragment)
            });
    (!matches).then(|| {
        format!(
            "{}: expected {:?}, found:\n{}",
            case.label,
            case.expected,
            found.join("\n")
        )
    })
}

#[test]
fn citations_are_checked_in_every_table_of_the_four_documents() {
    let repo = repo();
    let mut mismatches = Vec::new();
    for case in &CITATION_CASES {
        let rows = Document::parse(&case.lines.join("\n"))
            .tables()
            .iter()
            .map(|table| table.rows.len())
            .sum::<usize>();
        assert!(rows > 0, "{}: the case parses to no table row", case.label);
        let found = citation_case_violations(case, &repo);
        mismatches.extend(citation_case_mismatch(case, &found));
    }
    report(
        "citations_are_checked_in_every_table_of_the_four_documents",
        &mismatches,
    );
}

/// The fragment matching of test 5's inline cases cannot show that a wrong reason is
/// absent, so each range is held here to the exact reasons it must get.
#[test]
fn citation_ranges_get_each_of_their_reasons_and_no_other() {
    let cases = [
        (20, 15, 20, Some("a reversed range: start 20 > end 15")),
        (0, 0, 652, Some("line 0, but lines count from 1")),
        (21, 21, 20, Some("outside 1..=20, the file's line count")),
        (1, 21, 20, Some("outside 1..=20, the file's line count")),
        (
            30,
            25,
            20,
            Some("a reversed range: start 30 > end 25; outside 1..=20, the file's line count"),
        ),
        (
            25,
            15,
            20,
            Some("a reversed range: start 25 > end 15; outside 1..=20, the file's line count"),
        ),
        (
            3,
            0,
            20,
            Some("line 0, but lines count from 1; a reversed range: start 3 > end 0"),
        ),
        (
            30,
            0,
            20,
            Some(
                "line 0, but lines count from 1; a reversed range: start 30 > end 0; \
                 outside 1..=20, the file's line count",
            ),
        ),
        (20, 20, 20, None),
        (1, 20, 20, None),
    ];
    for (start, end, lines, expected) in cases {
        assert_eq!(
            citation_range_problem(start, end, lines).as_deref(),
            expected,
            "L{start}-{end} of a {lines}-line file"
        );
    }
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

/// A pipe table: `columns` as its header, its delimiter row, and one row per ID of `ids`
/// whose other cells hold `x`.
fn recognition_table(columns: &[&str], ids: &[&str]) -> String {
    let mut lines = vec![
        format!("| {} |", columns.join(" | ")),
        format!("|{}", "---|".repeat(columns.len())),
    ];
    for id in ids {
        let mut cells = vec![*id];
        cells.resize(columns.len(), "x");
        lines.push(format!("| {} |", cells.join(" | ")));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// The delimiter row, with its LF, of a table of `columns` columns built by
/// [`recognition_table`].
fn recognition_delimiter(columns: usize) -> String {
    format!("|{}\n", "---|".repeat(columns))
}

/// `text` with the first `from` after the first `marker` replaced by `to`.
fn recognition_mutate(text: &str, marker: &str, from: &str, to: &str) -> String {
    let Some(at) = text.find(marker) else {
        panic!("`{marker}` is not in the sample");
    };
    let (before, after) = text.split_at(at);
    assert!(after.contains(from), "`{from}` does not follow `{marker}`");
    format!("{before}{}", after.replacen(from, to, 1))
}

/// The 1-based number of the first line of `text` that holds `needle`.
fn recognition_line(text: &str, needle: &str) -> usize {
    let Some(index) = text.lines().position(|line| line.contains(needle)) else {
        panic!("`{needle}` is not in the sample");
    };
    index + 1
}

/// The problem [`recognition_violations`] reports for a table at `place` that its headings
/// give `role` but whose first column is `first`.
fn recognition_unread(place: &str, first: &str, role: RecognitionRole) -> String {
    let expected = role.first_column();
    let noun = role.noun();
    format!(
        "table under {place} has first column `{first}`, not `{expected}`: it is not read as {noun}"
    )
}

/// The problem [`recognition_stray_violations`] reports for the stray run of the header at
/// `header` and the one row after it.
fn recognition_stray(header: usize) -> String {
    let row = header + 1;
    format!(
        "pipe-table lines {header}-{row} outside any table: a header or delimiter row is missing"
    )
}

/// A parity audit that test 1 accepts, in its state before review: a `## Gates` legend,
/// then the six sections, each with one data table and an empty review log (header and
/// delimiter row only), and a second data table under `### Second table` in section C.
fn recognition_parity_sample() -> String {
    let mut parts = vec![
        "# Sample parity audit\n\n## Gates\n\n".to_owned(),
        recognition_table(&["Section", "Content", "Gates"], &[PARITY_SECTIONS[2]]),
    ];
    for (index, section) in PARITY_SECTIONS.into_iter().enumerate() {
        let mut columns = PARITY_COMMON_COLUMNS.to_vec();
        match index {
            0 => columns.extend(PARITY_LAYOUT_COLUMNS),
            1 => columns.extend(PARITY_RULE_COLUMNS),
            _ => {}
        }
        parts.push(format!("\n## {section}\n\n### First table\n\n"));
        parts.push(recognition_table(
            &columns,
            &[&format!("P-ZZTEST-{index}01")],
        ));
        if section == PARITY_SECTIONS[2] {
            parts.push("\n### Second table\n\n".to_owned());
            parts.push(recognition_table(&columns, &["P-ZZTEST-202"]));
        }
        parts.push(format!("\n### Review log — {section}\n\n"));
        parts.push(recognition_table(&REVIEW_LOG_COLUMNS, &[]));
    }
    parts.concat()
}

/// The `### Review log — sample` with no row and the `## Escalations raised` table that
/// close the validation and function map samples.
fn recognition_review_and_handoff() -> String {
    [
        "\n### Review log — sample\n\n".to_owned(),
        recognition_table(&REVIEW_LOG_COLUMNS, &[]),
        format!("\n## {ESCALATIONS_RAISED}\n\n"),
        recognition_table(&ESCALATION_COLUMNS, &["E-90"]),
    ]
    .concat()
}

/// A validation audit sample: the `## Status mapping` legend, which shares only `Status`
/// with [`VALIDATION_COLUMNS`], a data table under `## COSGN00C`, then
/// [`recognition_review_and_handoff`].
fn recognition_validation_sample() -> String {
    [
        "# Sample validation audit\n\n## Status mapping\n\n".to_owned(),
        recognition_table(&["Kind", "Status", "Rows"], &["`Invalid`"]),
        format!("\n## {}\n\n", VALIDATION_PROGRAMS[2]),
        recognition_table(&VALIDATION_COLUMNS, &["V-ZZTEST-001"]),
        recognition_review_and_handoff(),
    ]
    .concat()
}

/// A function map sample: a data table under `## COSGN00`, then
/// [`recognition_review_and_handoff`].
fn recognition_function_map_sample() -> String {
    [
        format!("# Sample function map\n\n## {}\n\n", BMS_MAPS[9]),
        recognition_table(&FUNCTION_MAP_COLUMNS, &["F-ZZTEST-001"]),
        recognition_review_and_handoff(),
    ]
    .concat()
}

/// An escalation log sample: its one table under `## Log`.
fn recognition_escalations_sample() -> String {
    [
        "# Sample escalation log\n\n## Log\n\n".to_owned(),
        recognition_table(&ESCALATION_COLUMNS, &["E-01"]),
    ]
    .concat()
}

/// One case of [`recognition_reports_unread_tables_at_every_position`]: the sample `text`
/// of the file at `path`, the recognition tests 2 and 3 apply to that file, and the
/// violations expected, in order, each as a text only its line holds and its problem. A
/// case expecting none is a positive control.
struct RecognitionCase {
    label: &'static str,
    path: &'static str,
    recognition: &'static Recognition,
    text: String,
    expected: Vec<(&'static str, String)>,
}

/// The validation audit cases: every position, the shape of a table outside them, and the
/// `## Status mapping` legend, which stays unreported even with a renamed first header.
fn recognition_validation_cases() -> Vec<RecognitionCase> {
    let sample = recognition_validation_sample();
    let program = format!("## {}", VALIDATION_PROGRAMS[2]);
    let raised = format!("## {ESCALATIONS_RAISED}");
    let misplaced = recognition_mutate(&sample, &program, &program, "## COSGN0OC");
    let case = |label, text, expected| RecognitionCase {
        label,
        path: VALIDATION_AUDIT_PATH,
        recognition: &RECOGNITION_VALIDATION,
        text,
        expected,
    };
    vec![
        case("validation sample", sample.clone(), Vec::new()),
        case(
            "validation data table with a renamed first header",
            recognition_mutate(&sample, &program, "| ID |", "| Identifier |"),
            vec![(
                "| Identifier |",
                recognition_unread(&format!("`{program}`"), "Identifier", RecognitionRole::Data),
            )],
        ),
        case(
            "validation data table outside every position, by its columns",
            recognition_mutate(&misplaced, "## COSGN0OC", "| ID |", "| Identifier |"),
            vec![(
                "| Identifier |",
                "table under `## COSGN0OC` has the columns of an audit table but first column \
                 `Identifier`, not `ID`: it is not read as one"
                    .to_owned(),
            )],
        ),
        case(
            "validation review log with a renamed first header",
            recognition_mutate(&sample, "### Review log", "| Row ID |", "| Reviewed row |"),
            vec![(
                "| Reviewed row |",
                recognition_unread(
                    &format!("`{program}` › `### Review log — sample`"),
                    "Reviewed row",
                    RecognitionRole::ReviewLog,
                ),
            )],
        ),
        case(
            "validation handoff table with a renamed first header",
            recognition_mutate(&sample, &raised, "| ID |", "| E-ID |"),
            vec![(
                "| E-ID |",
                recognition_unread(&format!("`{raised}`"), "E-ID", RecognitionRole::Handoff),
            )],
        ),
        case(
            "status-mapping legend with a renamed first header",
            recognition_mutate(&sample, "## Status mapping", "| Kind |", "| Category |"),
            Vec::new(),
        ),
    ]
}

/// The function map and escalation log cases: their positions and a lost delimiter row in
/// each.
fn recognition_map_and_log_cases() -> Vec<RecognitionCase> {
    let map_sample = recognition_function_map_sample();
    let log_sample = recognition_escalations_sample();
    let map = format!("## {}", BMS_MAPS[9]);
    let map_delimiter = recognition_delimiter(FUNCTION_MAP_COLUMNS.len());
    let log_delimiter = recognition_delimiter(ESCALATION_COLUMNS.len());
    let lost_map = recognition_mutate(&map_sample, &map, &map_delimiter, "");
    let lost_log = recognition_mutate(&log_sample, "## Log", &log_delimiter, "");
    let lost_map_problem = recognition_stray(recognition_line(&lost_map, "| ID | BMS map |"));
    let lost_log_problem = recognition_stray(recognition_line(&lost_log, "| ID |"));
    let note = recognition_table(&["Note", "Text"], &["a"]);
    let noted_log = recognition_mutate(&log_sample, "## Log", "## Log", &format!("{note}\n## Log"));
    let map_case = |label, text, expected| RecognitionCase {
        label,
        path: FUNCTION_MAP_PATH,
        recognition: &RECOGNITION_FUNCTION_MAP,
        text,
        expected,
    };
    let log_case = |label, text, expected| RecognitionCase {
        label,
        path: ESCALATIONS_PATH,
        recognition: &RECOGNITION_ESCALATIONS,
        text,
        expected,
    };
    vec![
        map_case("function map sample", map_sample.clone(), Vec::new()),
        map_case(
            "function map data table with a renamed first header",
            recognition_mutate(&map_sample, &map, "| ID |", "| Key |"),
            vec![(
                "| Key |",
                recognition_unread(&format!("`{map}`"), "Key", RecognitionRole::Data),
            )],
        ),
        map_case(
            "function map data table without its delimiter row",
            lost_map,
            vec![("| ID | BMS map |", lost_map_problem)],
        ),
        log_case("escalation log sample", log_sample.clone(), Vec::new()),
        log_case(
            "escalation log with a renamed first header",
            recognition_mutate(&log_sample, "## Log", "| ID |", "| Escalation |"),
            vec![(
                "| Escalation |",
                recognition_unread("`## Log`", "Escalation", RecognitionRole::Log),
            )],
        ),
        log_case(
            "escalation log without its delimiter row",
            lost_log,
            vec![("| ID |", lost_log_problem)],
        ),
        log_case(
            "escalation log with a second table before its `## Log` heading",
            noted_log,
            vec![(
                "| Note |",
                "table before the first `##` heading has first column `Note`, not `ID`: it is not \
                 read as the escalation log"
                    .to_owned(),
            )],
        ),
    ]
}

#[test]
fn recognition_reports_parity_tables_the_checks_cannot_read() {
    let sample = recognition_parity_sample();
    let arithmetic = PARITY_SECTIONS[2];
    let delimiter = recognition_delimiter(PARITY_COMMON_COLUMNS.len());
    let renamed = recognition_mutate(&sample, "### Second table", "| ID |", "| Identifier |");
    let lost = recognition_mutate(&sample, "### Second table", &delimiter, "");
    let review = recognition_mutate(&sample, "### Review log — C", "| Row ID |", "| Row |");
    let review_delimiter = recognition_delimiter(REVIEW_LOG_COLUMNS.len());
    let lost_review = recognition_mutate(&sample, "### Review log — C", &review_delimiter, "");
    let renamed_line = recognition_line(&renamed, "| Identifier |");
    let lost_line = recognition_line(&lost, "P-ZZTEST-202") - 1;
    let first_arithmetic_line = recognition_line(&review, "P-ZZTEST-201") - 2;
    let review_line = recognition_line(&review, "| Row |");
    let lost_review_line = recognition_line(&lost_review, "### Review log — C") + 2;
    let place = format!("`## {arithmetic}`");
    let no_review_log = violation(
        PARITY_AUDIT_PATH,
        first_arithmetic_line,
        &format!("section {place} has no review log"),
    );
    let cases = [
        (
            "parity sample, its `## Gates` legend included",
            sample,
            Vec::new(),
        ),
        (
            "parity data table with a renamed first header",
            renamed,
            vec![violation(
                PARITY_AUDIT_PATH,
                renamed_line,
                &recognition_unread(
                    &format!("{place} › `### Second table`"),
                    "Identifier",
                    RecognitionRole::Data,
                ),
            )],
        ),
        (
            "parity data table without its delimiter row",
            lost,
            vec![violation(
                PARITY_AUDIT_PATH,
                lost_line,
                &recognition_stray(lost_line),
            )],
        ),
        (
            "parity empty review log without its delimiter row",
            lost_review,
            vec![
                no_review_log.clone(),
                violation(
                    PARITY_AUDIT_PATH,
                    lost_review_line,
                    "pipe-table line outside any table: a header or delimiter row is missing",
                ),
            ],
        ),
        (
            "parity review log with a renamed first header",
            review,
            vec![
                no_review_log,
                violation(
                    PARITY_AUDIT_PATH,
                    review_line,
                    &recognition_unread(
                        &format!("{place} › `### Review log — {arithmetic}`"),
                        "Row",
                        RecognitionRole::ReviewLog,
                    ),
                ),
            ],
        ),
    ];
    for (label, text, expected) in cases {
        let violations = parity_audit_sections_and_columns_violations(&Document::parse(&text));
        assert_eq!(violations, expected, "{label}");
    }
}

#[test]
fn recognition_reports_unread_tables_at_every_position() {
    let cases = recognition_validation_cases()
        .into_iter()
        .chain(recognition_map_and_log_cases());
    for case in cases {
        let expected: Vec<String> = case
            .expected
            .iter()
            .map(|(needle, problem)| {
                violation(case.path, recognition_line(&case.text, needle), problem)
            })
            .collect();
        let doc = Document::parse(&case.text);
        let violations = recognition_file_violations(case.path, &doc, case.recognition);
        assert_eq!(violations, expected, "{}", case.label);
    }
}
