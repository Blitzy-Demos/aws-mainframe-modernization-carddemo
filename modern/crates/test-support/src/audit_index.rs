//! The audit-table index and the scanners behind binding a8 (AAP 0.8.2, 0.9.1).
//!
//! [`AuditIndex::load`] reads every rule table under `modern/audit/`, [`scan_test_tags`] finds
//! the `// audit:` tags in the tests, [`scan_source_citations`] finds the rule IDs cited in
//! `crates/*/src/**`, [`rule_modules`] lists the domain rule modules that must cite one, and
//! [`required_tags`] states which tags each rule needs. `tests/audit_coverage.rs` joins them and
//! decides; this module only parses and states the layer mapping.
//!
//! # Conventions
//!
//! `modern/audit/README.md` fixes them:
//!
//! - Rule files are every `.md` file under `modern/audit/` except the root `README.md`,
//!   `ESCALATIONS.md` and `HALTS.md`. Each holds exactly one rule table: a header row whose first
//!   cell is `Rule ID`, the separator row, then every following line that starts with `|`. Every
//!   row has as many cells as the header, and `\|` is a literal pipe, as in GitHub tables.
//!   Everything outside the rule table, fenced code blocks included, is prose the index ignores.
//! - A rule ID is an upper-case member name of letters and digits, a hyphen and three digits
//!   (`<MEMBER>-nnn`), unique across all rule files.
//! - The `Layer` column (`server`, `client`, `both`) exists in `online/*.md` and `screens.md`
//!   and nowhere else.
//! - A row whose expected value a halt blocks says `HALTED(Hn)` in its `Expected result` cell.
//!   The halt is a row of `HALTS.md`, whose `Status` decides [`AuditIndex::is_blocked`]. A
//!   blocked rule still requires its layer's tags: the coverage test reports it `BLOCKED`, never
//!   passed (AAP 0.8.5), and fails it when a required tag group is missing.
//! - A test proves a rule with the comment `// audit: <MEMBER>-nnn`, one rule ID per comment.
//!   Only the files [`scan_test_tags`] lists carry tags; such a comment in a unit test inside
//!   `crates/*/src/` is a citation, not a tag.
//! - In `crates/*/src/**` every word of the rule-ID form is a citation, whatever its member name.
//!
//! # Where the README overrides the plan's defaults
//!
//! - Files without a `Layer` column require one tag in any Rust test under `crates/*/tests/`, not
//!   in a per-file subset of crates, so [`required_tags`] lists every workspace crate for them.
//! - A tag comment carries exactly one rule ID; several IDs in one comment are an error.
//! - [`scan_source_citations`] reports every word of the rule-ID form, so it takes no index and
//!   [`AuditIndex`] exposes no set of member names to filter by.
//!
//! # Totality
//!
//! The audit files and the scanned sources are text this crate does not control, so every parser
//! here returns an [`AuditError`] or `None` and never panics: no indexing, slicing, `unwrap` or
//! `expect` (the crate root denies them).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The audit files that hold no rule table, directly under `modern/audit/`.
const NON_RULE_FILES: [&str; 3] = ["README.md", "ESCALATIONS.md", "HALTS.md"];
/// The halt report, directly under `modern/audit/`.
const HALTS_FILE: &str = "HALTS.md";
/// The first header cell of a rule table and of the halt table.
const RULE_TABLE_FIRST_CELL: &str = "Rule ID";
const HALT_TABLE_FIRST_CELL: &str = "ID";
const LAYER_COLUMN: &str = "Layer";
const EXPECTED_RESULT_COLUMN: &str = "Expected result";
const STATUS_COLUMN: &str = "Status";
const BLOCKED_STATUS: &str = "BLOCKED";
const HALT_MARKER: &str = "HALTED(";
const TAG_MARKER: &str = "audit:";

/// The eight workspace members; a Rust test under any of their `tests/` directories meets the
/// requirement of a rule file without a `Layer` column.
const WORKSPACE_CRATES: [&str; 8] = [
    "api",
    "batch",
    "copybook",
    "domain",
    "messaging",
    "persistence",
    "runtime",
    "test-support",
];
/// The domain directories whose modules implement audited rules (AAP 0.1.2).
const RULE_MODULE_DIRS: [&str; 3] = ["online", "batch", "messaging"];
/// Build, dependency and snapshot output, never scanned.
const SKIPPED_DIRS: [&str; 4] = ["target", "node_modules", "dist", "snapshots"];

/// A malformed audit file, tag or directory, located by file and 1-based line.
///
/// Line 0 means the file or directory as a whole, for example one that cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditError {
    path: PathBuf,
    line: usize,
    message: String,
}

impl AuditError {
    fn new(path: &Path, line: usize, message: String) -> Self {
        Self {
            path: path.to_path_buf(),
            line,
            message,
        }
    }
}

impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.path.display(), self.line, self.message)
    }
}

impl std::error::Error for AuditError {}

/// Letters, digits and `_`: the characters that join a rule-ID-shaped run to its neighbours, as
/// `\b` treats them in the scans of `scripts/check.sh`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte length of the rule ID that starts `text`, when one does and it ends at a word
/// boundary: an upper-case ASCII letter, more upper-case letters or digits, `-`, three digits.
fn rule_id_len(text: &str) -> Option<usize> {
    if !text.bytes().next().is_some_and(|b| b.is_ascii_uppercase()) {
        return None;
    }
    let member = text
        .bytes()
        .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        .count();
    let number_and_rest = text.get(member..)?.strip_prefix('-')?;
    if !number_and_rest
        .get(..3)?
        .bytes()
        .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    if number_and_rest
        .get(3..)?
        .chars()
        .next()
        .is_some_and(is_word_char)
    {
        return None;
    }
    // The member name, the hyphen and the three digits.
    member.checked_add(4)
}

/// Whether `text` is exactly one rule ID.
fn is_rule_id(text: &str) -> bool {
    rule_id_len(text) == Some(text.len())
}

/// A table cell's text without its surrounding whitespace and one pair of surrounding backticks.
fn unquote(cell: &str) -> &str {
    let trimmed = cell.trim();
    trimmed
        .strip_prefix('`')
        .and_then(|inner| inner.strip_suffix('`'))
        .unwrap_or(trimmed)
}

/// The rule ID a whole table cell holds, if it holds exactly one.
fn parse_rule_id(cell: &str) -> Option<&str> {
    let id = unquote(cell);
    is_rule_id(id).then_some(id)
}

/// Every word of the rule-ID form in `line`, bounded on both sides by a non-word character or
/// the line's start or end.
fn find_ids(line: &str) -> impl Iterator<Item = &str> {
    let mut previous: Option<char> = None;
    line.char_indices().filter_map(move |(start, c)| {
        let at_boundary = previous.is_none_or(|p| !is_word_char(p));
        previous = Some(c);
        if !at_boundary || !c.is_ascii_uppercase() {
            return None;
        }
        let rest = line.get(start..)?;
        rest.get(..rule_id_len(rest)?)
    })
}

/// Whether `text` is a halt ID: `H` followed by one or more digits.
fn is_halt_id(text: &str) -> bool {
    text.strip_prefix('H')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The halt IDs of every `HALTED(Hn)` marker in `text`, in order.
fn halt_markers(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices(HALT_MARKER).filter_map(|(at, _)| {
        let rest = text.get(at.checked_add(HALT_MARKER.len())?..)?;
        let digits = rest.strip_prefix('H')?;
        let count = digits.bytes().take_while(u8::is_ascii_digit).count();
        if count == 0 || !digits.get(count..)?.starts_with(')') {
            return None;
        }
        // `H` and its digits.
        rest.get(..count.checked_add(1)?)
    })
}

/// One row of a markdown table, with its 1-based line.
struct Row {
    line: usize,
    cells: Vec<String>,
}

/// A markdown table: its header row's line and cells, and its rows.
struct Table {
    line: usize,
    header: Vec<String>,
    rows: Vec<Row>,
}

impl Table {
    /// The index of the header cell named `name`, compared case-insensitively.
    fn column(&self, name: &str) -> Option<usize> {
        self.header
            .iter()
            .position(|cell| cell.eq_ignore_ascii_case(name))
    }
}

/// The lines of `text`, each with its 1-based number.
fn numbered_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index.saturating_add(1), line))
}

/// The trimmed cells of a table row that starts with `|`: an unescaped `|` ends a cell, `\|` is
/// a literal pipe, and the delimiters before the first cell and after the last are dropped.
fn split_cells(row: &str) -> Vec<String> {
    let Some(body) = row.trim().strip_prefix('|') else {
        return Vec::new();
    };
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut closed = false;
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        closed = c == '|';
        match c {
            '|' => {
                cells.push(cell.trim().to_owned());
                cell.clear();
            }
            '\\' => match chars.next() {
                Some('|') => cell.push('|'),
                Some(escaped) => {
                    cell.push('\\');
                    cell.push(escaped);
                }
                None => cell.push('\\'),
            },
            _ => cell.push(c),
        }
    }
    // A row may omit its closing pipe.
    if !closed {
        cells.push(cell.trim().to_owned());
    }
    cells
}

/// Whether `cells` form a separator row such as `|---|:---:|`.
fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells
            .iter()
            .all(|cell| cell.contains('-') && cell.chars().all(|c| matches!(c, '-' | ':')))
}

/// Whether a line opens or closes a fenced code block, returning its fence.
fn fence_of(trimmed: &str) -> Option<&'static str> {
    ["```", "~~~"]
        .into_iter()
        .find(|fence| trimmed.starts_with(fence))
}

/// Every table in `text` whose first header cell is `first_cell`, outside fenced code blocks.
fn read_tables(path: &Path, text: &str, first_cell: &str) -> Result<Vec<Table>, AuditError> {
    let mut tables = Vec::new();
    let mut lines = numbered_lines(text).peekable();
    let mut open_fence: Option<&str> = None;
    while let Some((line, content)) = lines.next() {
        let trimmed = content.trim();
        if let Some(fence) = open_fence {
            if trimmed.starts_with(fence) {
                open_fence = None;
            }
            continue;
        }
        if let Some(fence) = fence_of(trimmed) {
            open_fence = Some(fence);
            continue;
        }
        if !trimmed.starts_with('|') {
            continue;
        }
        let header = split_cells(trimmed);
        if !header
            .first()
            .is_some_and(|cell| cell.eq_ignore_ascii_case(first_cell))
        {
            continue;
        }
        if !lines
            .next()
            .is_some_and(|(_, separator)| is_separator(&split_cells(separator)))
        {
            return Err(AuditError::new(
                path,
                line,
                format!("the `{first_cell}` table header is not followed by a separator row"),
            ));
        }
        let mut rows = Vec::new();
        while let Some((row_line, row)) = lines.next_if(|(_, next)| next.trim().starts_with('|')) {
            let cells = split_cells(row);
            if cells.len() != header.len() {
                return Err(AuditError::new(
                    path,
                    row_line,
                    format!(
                        "the row has {} cells but its header has {}; a literal pipe is written `\\|`",
                        cells.len(),
                        header.len()
                    ),
                ));
            }
            rows.push(Row {
                line: row_line,
                cells,
            });
        }
        tables.push(Table { line, header, rows });
    }
    Ok(tables)
}

/// Reads `path` as UTF-8 text.
fn read_text(path: &Path) -> Result<String, AuditError> {
    let bytes = fs::read(path)
        .map_err(|err| AuditError::new(path, 0, format!("cannot read the file: {err}")))?;
    String::from_utf8(bytes).map_err(|err| {
        // The valid prefix splits into one piece per line up to the line of the invalid byte.
        let valid = err.utf8_error().valid_up_to();
        let line = err
            .as_bytes()
            .get(..valid)
            .map_or(0, |prefix| prefix.split(|&b| b == b'\n').count());
        AuditError::new(path, line, "the file is not valid UTF-8".to_owned())
    })
}

/// A directory entry, with symbolic links neither a file nor a directory.
struct DirEntry {
    name: String,
    is_dir: bool,
    is_file: bool,
}

/// The entries of `dir`, sorted by name.
fn entries(dir: &Path) -> Result<Vec<DirEntry>, AuditError> {
    let read_error =
        |err: io::Error| AuditError::new(dir, 0, format!("cannot read the directory: {err}"));
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).map_err(read_error)? {
        let entry = entry.map_err(read_error)?;
        let kind = entry.file_type().map_err(read_error)?;
        let name = entry.file_name().into_string().map_err(|name| {
            AuditError::new(
                dir,
                0,
                format!(
                    "the entry {} has a name that is not UTF-8",
                    name.to_string_lossy()
                ),
            )
        })?;
        found.push(DirEntry {
            name,
            is_dir: kind.is_dir(),
            is_file: kind.is_file(),
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// Appends every file under `base/dir` to `out` as a path relative to `base`, in sorted order,
/// skipping the directories of [`SKIPPED_DIRS`].
fn walk(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), AuditError> {
    for entry in entries(&base.join(dir))? {
        let path = dir.join(&entry.name);
        if entry.is_dir {
            if !SKIPPED_DIRS.contains(&entry.name.as_str()) {
                walk(base, &path, out)?;
            }
        } else if entry.is_file {
            out.push(path);
        }
    }
    Ok(())
}

/// Whether `path` has the extension `extension`, compared exactly.
fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|ext| ext == extension)
}

/// Whether the file name of `path` ends with `suffix`.
fn name_ends_with(path: &Path, suffix: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(suffix))
}

/// The `modern/audit/` directory.
#[must_use]
pub fn audit_dir() -> PathBuf {
    crate::modern_dir().join("audit")
}

/// Where a rule's outcome is observable, from the `Layer` column of `online/*.md` and
/// `screens.md` (AAP 0.8.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// Observable through an endpoint: status, message, field, response values or store effects.
    Server,
    /// Behaviour that sends no request: navigation, key bindings, what a screen renders.
    Client,
    /// A check the endpoint performs that the screen's zod schema repeats before sending.
    Both,
}

impl Layer {
    fn from_cell(cell: &str) -> Option<Self> {
        let value = unquote(cell);
        [Self::Server, Self::Client, Self::Both]
            .into_iter()
            .find(|layer| value.eq_ignore_ascii_case(layer.name()))
    }

    fn name(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::Client => "client",
            Self::Both => "both",
        }
    }
}

impl fmt::Display for Layer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One row of a rule table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRule {
    /// The rule ID, `<MEMBER>-nnn`.
    pub id: String,
    /// The rule file, relative to `modern/audit/`, such as `online/<PROGRAM>.md`.
    pub file: PathBuf,
    /// The row's 1-based line in [`AuditRule::file`].
    pub line: usize,
    /// The `Layer` cell; `None` in a file without that column.
    pub layer: Option<Layer>,
    /// The halt of a `HALTED(Hn)` marker in the `Expected result` cell, such as `H2`. When the
    /// cell names several halts, the first that `HALTS.md` still lists as `BLOCKED`, else the
    /// first named.
    pub halt: Option<String>,
}

/// Every rule of every rule file under `modern/audit/`, and the status of every halt.
#[derive(Debug, Clone)]
pub struct AuditIndex {
    rules: Vec<AuditRule>,
    by_id: BTreeMap<String, usize>,
    /// Halt ID → `Status` cell, trimmed and without backticks.
    halts: BTreeMap<String, String>,
}

impl AuditIndex {
    /// Reads `HALTS.md` and the rule table of every rule file under `audit_dir`, in path order.
    ///
    /// # Errors
    ///
    /// Fails, naming the file and line, when `audit_dir` or a file in it cannot be read or is not
    /// UTF-8; when `HALTS.md` has no halt table, a halt ID that is not `H` and digits, a duplicate
    /// halt or no `Status` column; when a rule file has no rule table or a second one, a table
    /// without its separator row or `Expected result` column, or a row whose cell count differs
    /// from its header's; when a `Layer` column is missing from `online/*.md` or `screens.md`,
    /// present elsewhere, or holds a value other than `server`, `client` or `both`; when a row's
    /// first cell is not a rule ID or repeats an ID defined earlier; or when a `HALTED(Hn)` marker
    /// names a halt `HALTS.md` does not list.
    pub fn load(audit_dir: &Path) -> Result<Self, AuditError> {
        let files = rule_files(audit_dir)?;
        let halts = read_halts(audit_dir)?;
        let mut rules: Vec<AuditRule> = Vec::new();
        let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
        for file in &files {
            for rule in read_rule_file(audit_dir, file, &halts)? {
                if let Some(first) = by_id.get(&rule.id).and_then(|&at| rules.get(at)) {
                    return Err(AuditError::new(
                        &audit_dir.join(&rule.file),
                        rule.line,
                        format!(
                            "duplicate rule ID {}, first defined at {}:{}",
                            rule.id,
                            first.file.display(),
                            first.line
                        ),
                    ));
                }
                by_id.insert(rule.id.clone(), rules.len());
                rules.push(rule);
            }
        }
        Ok(Self {
            rules,
            by_id,
            halts,
        })
    }

    /// Every rule, in file then row order.
    #[must_use]
    pub fn rules(&self) -> &[AuditRule] {
        &self.rules
    }

    /// The rule with ID `id`.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&AuditRule> {
        self.by_id.get(id).and_then(|&at| self.rules.get(at))
    }

    /// Whether `rule` is covered by a halt whose `HALTS.md` status is `BLOCKED`
    /// (case-insensitive): such a rule gets no parity assertion and is reported blocked, never
    /// passed (AAP 0.8.5).
    #[must_use]
    pub fn is_blocked(&self, rule: &AuditRule) -> bool {
        rule.halt
            .as_deref()
            .and_then(|halt| self.halts.get(halt))
            .is_some_and(|status| is_blocked_status(status))
    }
}

fn is_blocked_status(status: &str) -> bool {
    status.eq_ignore_ascii_case(BLOCKED_STATUS)
}

/// The rule files under `audit_dir`, relative to it and sorted.
fn rule_files(audit_dir: &Path) -> Result<Vec<PathBuf>, AuditError> {
    let mut files = Vec::new();
    walk(audit_dir, Path::new(""), &mut files)?;
    files.retain(|file| {
        has_extension(file, "md") && !NON_RULE_FILES.iter().any(|name| file == Path::new(name))
    });
    files.sort();
    Ok(files)
}

/// The halt table of `HALTS.md`: halt ID → status.
fn read_halts(audit_dir: &Path) -> Result<BTreeMap<String, String>, AuditError> {
    let path = audit_dir.join(HALTS_FILE);
    let text = read_text(&path)?;
    let tables = read_tables(&path, &text, HALT_TABLE_FIRST_CELL)?;
    if tables.is_empty() {
        return Err(AuditError::new(
            &path,
            0,
            format!("no halt table: no header row starts with `| {HALT_TABLE_FIRST_CELL} |`"),
        ));
    }
    let mut halts = BTreeMap::new();
    for table in &tables {
        let status_column = table.column(STATUS_COLUMN).ok_or_else(|| {
            AuditError::new(
                &path,
                table.line,
                format!("the halt table has no `{STATUS_COLUMN}` column"),
            )
        })?;
        for row in &table.rows {
            let id = row.cells.first().map_or("", |cell| unquote(cell));
            if !is_halt_id(id) {
                return Err(AuditError::new(
                    &path,
                    row.line,
                    format!("`{id}` is not a halt ID: `H` followed by digits"),
                ));
            }
            let status = row
                .cells
                .get(status_column)
                .map_or_else(String::new, |cell| cell.replace('`', "").trim().to_owned());
            if halts.insert(id.to_owned(), status).is_some() {
                return Err(AuditError::new(
                    &path,
                    row.line,
                    format!("duplicate halt ID {id}"),
                ));
            }
        }
    }
    Ok(halts)
}

/// Whether the rule file `file`, relative to `modern/audit/`, has a `Layer` column.
fn is_layered(file: &Path) -> bool {
    file.starts_with("online") || file == Path::new("screens.md")
}

/// The rules of the rule file `file`, relative to `audit_dir`.
fn read_rule_file(
    audit_dir: &Path,
    file: &Path,
    halts: &BTreeMap<String, String>,
) -> Result<Vec<AuditRule>, AuditError> {
    let path = audit_dir.join(file);
    let text = read_text(&path)?;
    let mut tables = read_tables(&path, &text, RULE_TABLE_FIRST_CELL)?.into_iter();
    let Some(table) = tables.next() else {
        return Err(AuditError::new(
            &path,
            0,
            format!("no rule table: no header row starts with `| {RULE_TABLE_FIRST_CELL} |`"),
        ));
    };
    if let Some(second) = tables.next() {
        return Err(AuditError::new(
            &path,
            second.line,
            "a second rule table; a rule file holds exactly one, and candidates are bullets"
                .to_owned(),
        ));
    }
    let layer_column = table.column(LAYER_COLUMN);
    match (is_layered(file), layer_column) {
        (true, None) => {
            return Err(AuditError::new(
                &path,
                table.line,
                format!("the rule table has no `{LAYER_COLUMN}` column"),
            ));
        }
        (false, Some(_)) => {
            return Err(AuditError::new(
                &path,
                table.line,
                format!("a `{LAYER_COLUMN}` column belongs only in `online/*.md` and `screens.md`"),
            ));
        }
        _ => {}
    }
    let expected_column = table.column(EXPECTED_RESULT_COLUMN).ok_or_else(|| {
        AuditError::new(
            &path,
            table.line,
            format!("the rule table has no `{EXPECTED_RESULT_COLUMN}` column"),
        )
    })?;
    let columns = RuleColumns {
        layer: layer_column,
        expected: expected_column,
    };
    table
        .rows
        .iter()
        .map(|row| rule_from_row(&path, file, row, columns, halts))
        .collect()
}

/// The rule-table columns a rule is read from.
#[derive(Clone, Copy)]
struct RuleColumns {
    layer: Option<usize>,
    expected: usize,
}

/// The rule one row of a rule table defines.
fn rule_from_row(
    path: &Path,
    file: &Path,
    row: &Row,
    columns: RuleColumns,
    halts: &BTreeMap<String, String>,
) -> Result<AuditRule, AuditError> {
    let cell = |column: usize| row.cells.get(column).map_or("", String::as_str);
    let first = cell(0);
    let id = parse_rule_id(first).ok_or_else(|| {
        AuditError::new(
            path,
            row.line,
            format!(
                "`{first}` is not a rule ID: an upper-case member name, a hyphen and three digits"
            ),
        )
    })?;
    let layer = columns
        .layer
        .map(|column| {
            let value = cell(column);
            Layer::from_cell(value).ok_or_else(|| {
                AuditError::new(
                    path,
                    row.line,
                    format!("layer `{value}` of {id} is not `server`, `client` or `both`"),
                )
            })
        })
        .transpose()?;
    let markers: Vec<&str> = halt_markers(cell(columns.expected)).collect();
    if let Some(unknown) = markers.iter().find(|halt| !halts.contains_key(**halt)) {
        return Err(AuditError::new(
            path,
            row.line,
            format!("`HALTED({unknown})` in {id} names no halt of `{HALTS_FILE}`"),
        ));
    }
    let halt = markers
        .iter()
        .find(|halt| {
            halts
                .get(**halt)
                .is_some_and(|status| is_blocked_status(status))
        })
        .or_else(|| markers.first())
        .map(|halt| (*halt).to_owned());
    Ok(AuditRule {
        id: id.to_owned(),
        file: file.to_path_buf(),
        line: row.line,
        layer,
        halt,
    })
}

/// The kind of test a `// audit:` tag sits in, from the file's path under `modern/`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TagKind {
    /// `crates/api/tests/endpoints_*.rs`: the server half of an online rule.
    ApiEndpoints,
    /// Any other Rust test under `crates/<dir>/tests/`, named by its crate directory.
    RustTest(String),
    /// `frontend/e2e/nav.spec.ts`: the Playwright NAV suite.
    Nav,
    /// A Vitest file, `frontend/src/**/*.test.ts` or `*.test.tsx`.
    Vitest,
}

impl fmt::Display for TagKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiEndpoints => f.write_str("crates/api/tests/endpoints_*.rs"),
            Self::RustTest(dir) => write!(f, "crates/{dir}/tests"),
            Self::Nav => f.write_str("frontend/e2e/nav.spec.ts"),
            Self::Vitest => f.write_str("frontend/src/**/*.test.ts(x)"),
        }
    }
}

/// The tags a rule needs: every group must be met by at least one tag of a kind it lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The groups, each a set of alternative tag kinds; never empty, and no group is empty.
    pub groups: Vec<Vec<TagKind>>,
}

impl Requirement {
    /// The groups that no kind in `kinds` meets, in order.
    #[must_use]
    pub fn unmet_groups<'a>(
        &self,
        kinds: impl IntoIterator<Item = &'a TagKind>,
    ) -> Vec<&[TagKind]> {
        let present: BTreeSet<&TagKind> = kinds.into_iter().collect();
        self.groups
            .iter()
            .filter(|group| !group.iter().any(|kind| present.contains(kind)))
            .map(Vec::as_slice)
            .collect()
    }

    /// Whether the tag kinds present for a rule meet every group.
    #[must_use]
    pub fn satisfied_by<'a>(&self, kinds: impl IntoIterator<Item = &'a TagKind>) -> bool {
        self.unmet_groups(kinds).is_empty()
    }
}

/// The tags `rule` needs (AAP 0.8.2, 0.9.1; `modern/audit/README.md`, "Layers and required
/// tags"): `server` a tag in `crates/api/tests/endpoints_*.rs`; `client` one in
/// `frontend/e2e/nav.spec.ts` or a Vitest file; `both` one of each; a rule from a file without a
/// `Layer` column one in any Rust test under `crates/*/tests/`.
///
/// A rule for which [`AuditIndex::is_blocked`] holds requires the same tags (the README's
/// `HALTED(Hn)` rows still carry theirs): the coverage test reports it `BLOCKED`, never passed
/// (AAP 0.8.5), and fails it when a required tag group is missing.
#[must_use]
pub fn required_tags(rule: &AuditRule) -> Requirement {
    let frontend = || vec![TagKind::Nav, TagKind::Vitest];
    let groups = match rule.layer {
        Some(Layer::Server) => vec![vec![TagKind::ApiEndpoints]],
        Some(Layer::Client) => vec![frontend()],
        Some(Layer::Both) => vec![vec![TagKind::ApiEndpoints], frontend()],
        // The audit README accepts a tag in any crate's Rust tests for these files, so the group
        // lists every workspace crate rather than the crates that consume the file.
        None => vec![
            std::iter::once(TagKind::ApiEndpoints)
                .chain(
                    WORKSPACE_CRATES
                        .iter()
                        .map(|&dir| TagKind::RustTest(dir.to_owned())),
                )
                .collect(),
        ],
    };
    Requirement { groups }
}

/// One `// audit:` tag in a test file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSite {
    /// The rule ID the tag names; [`AuditIndex::get`] decides whether a rule has it.
    pub id: String,
    /// The test file, relative to `modern/`.
    pub path: PathBuf,
    /// The tag's 1-based line.
    pub line: usize,
    /// The kind of test the file is.
    pub kind: TagKind,
}

/// Every `// audit:` tag in the tests under `modern_dir`: `crates/*/tests/**/*.rs`,
/// `frontend/src/**/*.test.ts`, `frontend/src/**/*.test.tsx` and `frontend/e2e/nav.spec.ts`,
/// skipping `target`, `node_modules`, `dist` and `snapshots` directories. A missing `frontend/`
/// or `tests/` directory has no tags. Sorted by path, then line.
///
/// # Errors
///
/// Fails, naming the file and line, when `modern_dir/crates` or a scanned directory or file
/// cannot be read, a file is not UTF-8, or a tag is malformed: the text after `audit:` must be
/// exactly one rule ID.
pub fn scan_test_tags(modern_dir: &Path) -> Result<Vec<TagSite>, AuditError> {
    let mut sites = Vec::new();
    for (file, kind) in test_files(modern_dir)? {
        let path = modern_dir.join(&file);
        let text = read_text(&path)?;
        for (line, content) in numbered_lines(&text) {
            for id in line_tags(&path, line, content)? {
                sites.push(TagSite {
                    id: id.to_owned(),
                    path: file.clone(),
                    line,
                    kind: kind.clone(),
                });
            }
        }
    }
    sites.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(sites)
}

/// The crate directories under `modern_dir/crates`, sorted.
fn crate_dirs(modern_dir: &Path) -> Result<Vec<String>, AuditError> {
    Ok(entries(&modern_dir.join("crates"))?
        .into_iter()
        .filter(|entry| entry.is_dir)
        .map(|entry| entry.name)
        .collect())
}

/// The test files that can carry tags, relative to `modern_dir`, with their kinds.
fn test_files(modern_dir: &Path) -> Result<Vec<(PathBuf, TagKind)>, AuditError> {
    let mut files = Vec::new();
    for dir in crate_dirs(modern_dir)? {
        let tests = Path::new("crates").join(&dir).join("tests");
        if !modern_dir.join(&tests).is_dir() {
            continue;
        }
        let mut found = Vec::new();
        walk(modern_dir, &tests, &mut found)?;
        for file in found.into_iter().filter(|file| has_extension(file, "rs")) {
            let endpoint_test = dir == "api"
                && file.parent() == Some(tests.as_path())
                && name_starts_with(&file, "endpoints_");
            let kind = if endpoint_test {
                TagKind::ApiEndpoints
            } else {
                TagKind::RustTest(dir.clone())
            };
            files.push((file, kind));
        }
    }
    let vitest_root = Path::new("frontend").join("src");
    if modern_dir.join(&vitest_root).is_dir() {
        let mut found = Vec::new();
        walk(modern_dir, &vitest_root, &mut found)?;
        files.extend(
            found
                .into_iter()
                .filter(|file| {
                    name_ends_with(file, ".test.ts") || name_ends_with(file, ".test.tsx")
                })
                .map(|file| (file, TagKind::Vitest)),
        );
    }
    let nav = Path::new("frontend").join("e2e").join("nav.spec.ts");
    if modern_dir.join(&nav).is_file() {
        files.push((nav, TagKind::Nav));
    }
    Ok(files)
}

/// Whether the file name of `path` starts with `prefix`.
fn name_starts_with(path: &Path, prefix: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(prefix))
}

/// The rule IDs tagged on one line of a test file, one per `// audit:` comment.
fn line_tags<'t>(path: &Path, line: usize, text: &'t str) -> Result<Vec<&'t str>, AuditError> {
    let mut ids = Vec::new();
    for (at, _) in text.match_indices(TAG_MARKER) {
        let opens_comment = text
            .get(..at)
            .is_some_and(|before| before.trim_end_matches([' ', '\t']).ends_with("//"));
        if !opens_comment {
            continue;
        }
        let tagged = text
            .get(at.saturating_add(TAG_MARKER.len())..)
            .map_or("", str::trim);
        // The audit README allows one rule ID per tag comment, so a list is malformed too.
        if !is_rule_id(tagged) {
            return Err(AuditError::new(
                path,
                line,
                format!(
                    "malformed audit tag `{tagged}`: a tag comment names exactly one rule ID, \
                     `// audit: <MEMBER>-nnn`"
                ),
            ));
        }
        ids.push(tagged);
    }
    Ok(ids)
}

/// A rule-ID-shaped word in the source of a crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Citation {
    /// The word; [`AuditIndex::get`] decides whether a rule has it.
    pub id: String,
    /// The source file, relative to `modern/`.
    pub path: PathBuf,
    /// The word's 1-based line.
    pub line: usize,
    /// Whether the line, after its indentation, starts with `///` or `//!`.
    pub in_doc_comment: bool,
}

/// Every word of the rule-ID form in `crates/*/src/**/*.rs` under `modern_dir`, resolved or
/// not, skipping `target` directories. Sorted by path, then line.
///
/// The audit README reads every such word in crate source as a citation, so nothing is filtered
/// by member name: a word that is not meant as a rule ID is written without its hyphen.
///
/// # Errors
///
/// Fails, naming the file, when `modern_dir/crates` or a scanned directory or file cannot be
/// read, or a file is not UTF-8.
pub fn scan_source_citations(modern_dir: &Path) -> Result<Vec<Citation>, AuditError> {
    let mut citations = Vec::new();
    for dir in crate_dirs(modern_dir)? {
        let src = Path::new("crates").join(&dir).join("src");
        if !modern_dir.join(&src).is_dir() {
            continue;
        }
        let mut found = Vec::new();
        walk(modern_dir, &src, &mut found)?;
        for file in found.into_iter().filter(|file| has_extension(file, "rs")) {
            let text = read_text(&modern_dir.join(&file))?;
            for (line, content) in numbered_lines(&text) {
                let code = content.trim_start();
                let in_doc_comment = code.starts_with("///") || code.starts_with("//!");
                citations.extend(find_ids(content).map(|id| Citation {
                    id: id.to_owned(),
                    path: file.clone(),
                    line,
                    in_doc_comment,
                }));
            }
        }
    }
    citations.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(citations)
}

/// The rule modules, `crates/domain/src/{online,batch,messaging}/*.rs` without `mod.rs`,
/// relative to `modern_dir` and sorted. A missing directory has no modules.
///
/// # Errors
///
/// Fails, naming the directory, when one of the three directories exists but cannot be read.
pub fn rule_modules(modern_dir: &Path) -> Result<Vec<PathBuf>, AuditError> {
    let mut modules = Vec::new();
    for layer in RULE_MODULE_DIRS {
        let dir = Path::new("crates").join("domain").join("src").join(layer);
        let full = modern_dir.join(&dir);
        if !full.is_dir() {
            continue;
        }
        for entry in entries(&full)? {
            let module = dir.join(&entry.name);
            if entry.is_file && has_extension(&module, "rs") && entry.name != "mod.rs" {
                modules.push(module);
            }
        }
    }
    modules.sort();
    Ok(modules)
}
