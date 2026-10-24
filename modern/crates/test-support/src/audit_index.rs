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
//!   The halt is a row of `HALTS.md`, whose `Status` is `BLOCKED`, `RESOLVED` or `DESCOPED`
//!   (backticks ignored, case-insensitive); any other status, an empty one included, is an
//!   error. Only `BLOCKED` makes [`AuditIndex::is_blocked`] true. A blocked rule still requires
//!   its layer's tags: the coverage test reports it `BLOCKED`, never passed (AAP 0.8.5), and
//!   fails it when a required tag group is missing.
//! - A test proves a rule with the comment `// audit: <MEMBER>-nnn`, one rule ID per comment.
//!   Only the files [`scan_test_tags`] lists carry tags; such a comment in a unit test inside
//!   `crates/*/src/` is a citation, not a tag. A tag is a real line comment: the same text in a
//!   string, raw string, template literal, regular expression or block comment is inert, and so
//!   is the text of a JSX element or a JSX attribute string in a `.tsx` file.
//! - In `crates/*/src/**` every word of the rule-ID form is a citation, whatever its member name
//!   and wherever it stands: code, strings and comments alike. Only a word inside a Rust doc
//!   comment (`///` but not `////`, `//!`, `/** */` but not `/***` or `/**/`, and `/*! */`)
//!   counts as documentation, which every rule module needs.
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
//! `expect` (the crate root denies them). The lexers behind the two scanners are total too: an
//! unterminated string, literal, comment or JSX element runs to the end of the text, or of its
//! line where the language ends it there, and the scan goes on.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The audit files that hold no rule table, directly under `modern/audit/`.
const NON_RULE_FILES: [&str; 3] = ["README.md", "ESCALATIONS.md", "HALTS.md"];
/// The halt report, directly under `modern/audit/`.
const HALTS_FILE: &str = "HALTS.md";
/// The first header cell of a rule table.
const RULE_TABLE_FIRST_CELL: &str = "Rule ID";
/// The first header cell of the halt table of `HALTS.md`.
const HALT_TABLE_FIRST_CELL: &str = "ID";
const LAYER_COLUMN: &str = "Layer";
const EXPECTED_RESULT_COLUMN: &str = "Expected result";
const STATUS_COLUMN: &str = "Status";
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

/// Every word of the rule-ID form in `text`, with its byte offset, bounded on both sides by a
/// non-word character or the text's start or end. A line break is a non-word character, so the
/// words of a whole file are exactly those of its lines.
fn find_ids(text: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut previous: Option<char> = None;
    text.char_indices().filter_map(move |(start, c)| {
        let at_boundary = previous.is_none_or(|p| !is_word_char(p));
        previous = Some(c);
        if !at_boundary || !c.is_ascii_uppercase() {
            return None;
        }
        let rest = text.get(start..)?;
        Some((start, rest.get(..rule_id_len(rest)?)?))
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
    /// Halt ID → its `Status` in `HALTS.md`.
    halts: BTreeMap<String, HaltStatus>,
}

impl AuditIndex {
    /// Reads `HALTS.md` and the rule table of every rule file under `audit_dir`, in path order.
    ///
    /// # Errors
    ///
    /// Fails, naming the file and line, when `audit_dir` or a file in it cannot be read or is not
    /// UTF-8; when `HALTS.md` has no halt table, a halt ID that is not `H` and digits, a duplicate
    /// halt, no `Status` column, or a status other than `BLOCKED`, `RESOLVED` or `DESCOPED`, an
    /// empty one included; when a rule file has no rule table or a second one, a table
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

    /// Whether `rule` is covered by a halt whose `HALTS.md` status is `BLOCKED`: such a rule gets
    /// no parity assertion and is reported blocked, never passed (AAP 0.8.5). A `RESOLVED` or
    /// `DESCOPED` halt blocks nothing, and [`AuditIndex::load`] rejects every other status.
    #[must_use]
    pub fn is_blocked(&self, rule: &AuditRule) -> bool {
        rule.halt
            .as_deref()
            .and_then(|halt| self.halts.get(halt))
            .is_some_and(|&status| status == HaltStatus::Blocked)
    }
}

/// The `Status` of a `HALTS.md` row (AAP 0.8.5): `BLOCKED` until a source-backed resolution
/// makes it `RESOLVED` or an explicit user scope change makes it `DESCOPED`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HaltStatus {
    /// The initial status: the bindings the halt lists cannot pass.
    Blocked,
    /// A source-backed resolution the reviewer confirmed; the case is ordinary audit rows.
    Resolved,
    /// The user removed or redefined the case.
    Descoped,
}

impl HaltStatus {
    /// The status a `Status` cell names, with its backticks removed and its surrounding
    /// whitespace trimmed, compared case-insensitively; `None` for any other text, an empty cell
    /// included.
    fn from_cell(cell: &str) -> Option<Self> {
        let value = cell.replace('`', "");
        let value = value.trim();
        [Self::Blocked, Self::Resolved, Self::Descoped]
            .into_iter()
            .find(|status| value.eq_ignore_ascii_case(status.name()))
    }

    fn name(self) -> &'static str {
        match self {
            Self::Blocked => "BLOCKED",
            Self::Resolved => "RESOLVED",
            Self::Descoped => "DESCOPED",
        }
    }
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
fn read_halts(audit_dir: &Path) -> Result<BTreeMap<String, HaltStatus>, AuditError> {
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
            let cell = row.cells.get(status_column).map_or("", String::as_str);
            // A status outside the closed set fails rather than unblocking the halt's rows.
            let status = HaltStatus::from_cell(cell).ok_or_else(|| {
                let found = if cell.is_empty() {
                    "an empty status".to_owned()
                } else {
                    format!("the status `{cell}`")
                };
                AuditError::new(
                    &path,
                    row.line,
                    format!(
                        "halt {id} has {found}; a halt's status is `BLOCKED`, `RESOLVED` or `DESCOPED`"
                    ),
                )
            })?;
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
    halts: &BTreeMap<String, HaltStatus>,
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
    halts: &BTreeMap<String, HaltStatus>,
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
        .find(|halt| halts.get(**halt) == Some(&HaltStatus::Blocked))
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
/// A tag is a line comment whose text after its opening slashes and any whitespace starts with
/// `audit:`, as the lexer of the file's language finds it: the Rust lexer for `.rs` files, which
/// knows strings, raw, byte and C strings, character literals told apart from lifetimes, and
/// nested block comments; the TypeScript lexer for `.ts` files, which knows strings, template
/// literals with their `${}` substitutions, block comments, and regular expressions, told apart
/// from division by the token before them; and the same lexer with JSX for `.tsx` files, which
/// also knows JSX elements and fragments, whose text and attribute strings hold no comments while
/// their `{}` expression containers hold code. Tag-shaped text inside any of those is inert, a
/// block comment never carries a tag, and a line comment that mentions `// audit:` only after
/// other text, such as a quoted example, is no tag.
///
/// # Errors
///
/// Fails, naming the file and line, when `modern_dir/crates` or a scanned directory or file
/// cannot be read, a file is not UTF-8, or a tag is malformed: the text after its `audit:` must
/// be exactly one rule ID.
pub fn scan_test_tags(modern_dir: &Path) -> Result<Vec<TagSite>, AuditError> {
    let mut sites = Vec::new();
    for (file, kind) in test_files(modern_dir)? {
        let path = modern_dir.join(&file);
        let text = read_text(&path)?;
        for (line, id) in file_tags(&path, Language::of(&file), &text)? {
            sites.push(TagSite {
                id: id.to_owned(),
                path: file.clone(),
                line,
                kind: kind.clone(),
            });
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

/// The tags of the test file at `path`, whose text is `text`, read with the lexer of `language`:
/// each tag's 1-based line and rule ID, in order.
fn file_tags<'t>(
    path: &Path,
    language: Language,
    text: &'t str,
) -> Result<Vec<(usize, &'t str)>, AuditError> {
    let starts = line_starts(text);
    let mut tags = Vec::new();
    for comment in language.comments(text) {
        if comment.kind != CommentKind::Line {
            continue;
        }
        let line = line_of(&starts, comment.start);
        let body = text.get(comment.start..comment.end).unwrap_or_default();
        if let Some(id) = comment_tag(path, line, body)? {
            tags.push((line, id));
        }
    }
    Ok(tags)
}

/// The rule ID the line comment `comment`, its opening `//` included, tags: `None` unless its
/// text after the opening slashes and any whitespace starts with `audit:`.
fn comment_tag<'t>(
    path: &Path,
    line: usize,
    comment: &'t str,
) -> Result<Option<&'t str>, AuditError> {
    let Some(tagged) = comment
        .trim_start_matches('/')
        .trim_start()
        .strip_prefix(TAG_MARKER)
    else {
        return Ok(None);
    };
    let tagged = tagged.trim();
    // The audit README allows one rule ID per tag comment, so a list is malformed too.
    if is_rule_id(tagged) {
        Ok(Some(tagged))
    } else {
        Err(AuditError::new(
            path,
            line,
            format!(
                "malformed audit tag `{tagged}`: a tag comment names exactly one rule ID, \
                 `// audit: <MEMBER>-nnn`"
            ),
        ))
    }
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
    /// Whether the word lies inside a Rust doc comment, as `rustc` reads one: a `///` line
    /// comment not opened by `////`, a `//!` line comment, a `/** */` block comment not opened by
    /// `/***` or `/**/`, or a `/*! */` block comment, every line of a multi-line block included.
    /// A word in an ordinary comment, a string or code is a citation too, but not documentation.
    pub in_doc_comment: bool,
}

/// Every word of the rule-ID form in `crates/*/src/**/*.rs` under `modern_dir`, resolved or
/// not, skipping `target` directories. Sorted by path, then line.
///
/// The audit README reads every such word in crate source as a citation, so nothing is filtered
/// by member name: a word that is not meant as a rule ID is written without its hyphen. The Rust
/// lexer decides [`Citation::in_doc_comment`] from the whole file, so a doc-looking line inside a
/// string or an ordinary block comment is no documentation, and a block doc comment is.
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
            citations.extend(file_citations(&file, &text));
        }
    }
    citations.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(citations)
}

/// The citations in `text`, the source of the Rust file `file` (relative to `modern/`), in
/// order: every word of the rule-ID form, flagged when it lies inside a doc comment.
fn file_citations(file: &Path, text: &str) -> Vec<Citation> {
    let starts = line_starts(text);
    let docs: Vec<Comment> = rust_comments(text)
        .into_iter()
        .filter(|comment| comment.doc)
        .collect();
    find_ids(text)
        .map(|(offset, id)| Citation {
            id: id.to_owned(),
            path: file.to_path_buf(),
            line: line_of(&starts, offset),
            in_doc_comment: covers(&docs, offset),
        })
        .collect()
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

/// The byte offset at which each line of `text` starts; the first is 0.
fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at.saturating_add(1)))
        .collect()
}

/// The 1-based line of the byte at `offset`, given the [`line_starts`] of its text.
fn line_of(starts: &[usize], offset: usize) -> usize {
    // The first line starts at 0, so at least one start precedes any offset.
    starts.partition_point(|&start| start <= offset)
}

/// Whether the byte at `offset` lies inside one of `comments`, which are in order and disjoint.
fn covers(comments: &[Comment], offset: usize) -> bool {
    let after = comments.partition_point(|comment| comment.end <= offset);
    comments
        .get(after)
        .is_some_and(|comment| comment.start <= offset)
}

/// The lexical grammar a scanned file is read with, chosen by its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    /// `.rs` files, read with the lexical grammar of the Rust Reference.
    Rust,
    /// `.ts` files, read with the ECMAScript grammar TypeScript extends, in which a `<` is always
    /// an operator, as in type assertions and type arguments.
    TypeScript,
    /// `.tsx` files: the TypeScript grammar with JSX, in which a `<` where an expression starts
    /// opens a JSX element unless it opens a generic arrow function's type parameters.
    Tsx,
}

impl Language {
    /// The language of the file at `path`: Rust for `.rs`, TypeScript with JSX for `.tsx`, and
    /// TypeScript for any other, which among the files [`scan_test_tags`] reads means `.ts`.
    fn of(path: &Path) -> Self {
        if has_extension(path, "rs") {
            Self::Rust
        } else if has_extension(path, "tsx") {
            Self::Tsx
        } else {
            Self::TypeScript
        }
    }

    /// Every comment of `text`, in order.
    fn comments(self, text: &str) -> Vec<Comment> {
        match self {
            Self::Rust => rust_comments(text),
            Self::TypeScript => typescript_comments(text, false),
            Self::Tsx => typescript_comments(text, true),
        }
    }
}

/// Whether a comment runs to the end of its line or to a closing delimiter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommentKind {
    /// `//` to the end of the line.
    Line,
    /// `/*` to `*/`, which nest in Rust.
    Block,
}

/// A comment a lexer found outside every string, character, template and regular-expression
/// literal of a source text, and outside the text and attribute strings of every JSX element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Comment {
    /// The byte offset of the opening `//` or `/*`.
    start: usize,
    /// The byte offset just past the comment: its line's `\n`, which it excludes, the end of the
    /// closing `*/`, or the end of the text for a comment left open.
    end: usize,
    /// A line or a block comment.
    kind: CommentKind,
    /// Whether Rust reads it as documentation: `///` not followed by a further `/`, `//!`, `/**`
    /// not followed by `*` or `/`, or `/*!`. Always false in TypeScript.
    doc: bool,
}

/// A byte position in a source text. Every delimiter the lexers recognise is an ASCII byte, and
/// no ASCII byte occurs inside a multi-byte UTF-8 sequence, so stepping through the bytes never
/// mistakes part of a character for a delimiter.
struct Cursor<'t> {
    bytes: &'t [u8],
    pos: usize,
}

impl<'t> Cursor<'t> {
    fn new(text: &'t str) -> Self {
        Self {
            bytes: text.as_bytes(),
            pos: 0,
        }
    }

    /// The byte `ahead` bytes past the position, if the text is that long.
    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.pos.checked_add(ahead)?).copied()
    }

    /// The byte at the position.
    fn peek(&self) -> Option<u8> {
        self.peek_at(0)
    }

    /// The byte just before the position.
    fn previous(&self) -> Option<u8> {
        self.bytes.get(self.pos.checked_sub(1)?).copied()
    }

    /// Whether the text at the position starts with `prefix`.
    fn at(&self, prefix: &[u8]) -> bool {
        self.bytes
            .get(self.pos..)
            .is_some_and(|rest| rest.starts_with(prefix))
    }

    /// Moves `count` bytes on, stopping at the end of the text.
    fn advance(&mut self, count: usize) {
        self.pos = self.pos.saturating_add(count).min(self.bytes.len());
    }

    /// Moves past every byte `keep` accepts.
    fn skip_while(&mut self, keep: fn(u8) -> bool) {
        while self.peek().is_some_and(keep) {
            self.advance(1);
        }
    }

    /// Moves to the next `\n`, or to the end of the text, without passing it.
    fn skip_line(&mut self) {
        self.skip_while(|byte| byte != b'\n');
    }

    /// The bytes from `start` to the position.
    fn since(&self, start: usize) -> &'t [u8] {
        self.bytes.get(start..self.pos).unwrap_or_default()
    }
}

/// Every comment of the Rust source `text`, in order, read with the lexical grammar of the Rust
/// Reference: line and nested block comments, doc comments among them; `"` strings, which span
/// lines and take `\` escapes; byte and C strings (`b"`, `c"`); raw strings (`r"`, `r#"`, `br#"`,
/// `cr#"`), which end at a `"` followed by as many `#`s and take no escapes; and character and
/// byte literals told apart from lifetimes and labels as `rustc` tells them.
fn rust_comments(text: &str) -> Vec<Comment> {
    let mut cursor = Cursor::new(text);
    let mut comments = Vec::new();
    while let Some(byte) = cursor.peek() {
        match byte {
            b'/' if cursor.peek_at(1) == Some(b'/') => {
                comments.push(rust_line_comment(&mut cursor));
            }
            b'/' if cursor.peek_at(1) == Some(b'*') => {
                comments.push(rust_block_comment(&mut cursor));
            }
            b'"' => {
                cursor.advance(1);
                skip_rust_string(&mut cursor);
            }
            b'\'' => {
                cursor.advance(1);
                skip_rust_quote(&mut cursor);
            }
            _ if is_rust_ident_start(byte) => skip_rust_word(&mut cursor),
            _ => cursor.advance(1),
        }
    }
    comments
}

/// The Rust line comment at the cursor, which it moves to the end of the line.
fn rust_line_comment(cursor: &mut Cursor<'_>) -> Comment {
    let start = cursor.pos;
    let doc = match cursor.peek_at(2) {
        Some(b'!') => true,
        Some(b'/') => cursor.peek_at(3) != Some(b'/'),
        _ => false,
    };
    cursor.skip_line();
    Comment {
        start,
        end: cursor.pos,
        kind: CommentKind::Line,
        doc,
    }
}

/// The Rust block comment at the cursor, nested comments included, which it moves past.
fn rust_block_comment(cursor: &mut Cursor<'_>) -> Comment {
    let start = cursor.pos;
    let doc = match cursor.peek_at(2) {
        Some(b'!') => true,
        Some(b'*') => !matches!(cursor.peek_at(3), Some(b'*' | b'/')),
        _ => false,
    };
    cursor.advance(2);
    let mut depth: usize = 1;
    while cursor.peek().is_some() {
        if cursor.at(b"/*") {
            cursor.advance(2);
            depth = depth.saturating_add(1);
        } else if cursor.at(b"*/") {
            cursor.advance(2);
            depth = depth.saturating_sub(1);
            if depth == 0 {
                break;
            }
        } else {
            cursor.advance(1);
        }
    }
    Comment {
        start,
        end: cursor.pos,
        kind: CommentKind::Block,
        doc,
    }
}

/// Moves past the rest of a Rust `"` string whose opening quote is behind the cursor.
fn skip_rust_string(cursor: &mut Cursor<'_>) {
    while let Some(byte) = cursor.peek() {
        cursor.advance(1);
        match byte {
            b'\\' => cursor.advance(1),
            b'"' => return,
            _ => {}
        }
    }
}

/// Moves past what follows a `'` behind the cursor, as `rustc` reads it: an identifier or digit
/// not directly followed by a second `'` begins a lifetime or label (`'a`, `'static`, `'outer:`),
/// or the character literal `'a'` when a `'` ends it; anything else is a character literal.
fn skip_rust_quote(cursor: &mut Cursor<'_>) {
    let can_be_lifetime = cursor.peek_at(1) != Some(b'\'')
        && cursor
            .peek()
            .is_some_and(|byte| is_rust_ident_start(byte) || byte.is_ascii_digit());
    if can_be_lifetime {
        cursor.skip_while(is_rust_ident_continue);
        if cursor.peek() == Some(b'\'') {
            cursor.advance(1);
        }
    } else {
        skip_rust_char(cursor);
    }
}

/// Moves past the rest of a Rust character or byte literal whose opening `'` is behind the
/// cursor. As in `rustc`, `'''` is one literal, `\` escapes the next byte, and a literal left
/// open ends before a `/` or a line break that no `'` follows, so it cannot swallow a comment.
fn skip_rust_char(cursor: &mut Cursor<'_>) {
    if cursor.peek_at(1) == Some(b'\'') && cursor.peek() != Some(b'\\') {
        cursor.advance(2);
        return;
    }
    while let Some(byte) = cursor.peek() {
        match byte {
            b'\'' => {
                cursor.advance(1);
                return;
            }
            b'/' => return,
            b'\n' if cursor.peek_at(1) != Some(b'\'') => return,
            b'\\' => cursor.advance(2),
            _ => cursor.advance(1),
        }
    }
}

/// Moves past the Rust identifier or keyword at the cursor and, when it is the prefix of a byte,
/// C or raw string or of a byte literal, past that literal too.
fn skip_rust_word(cursor: &mut Cursor<'_>) {
    let start = cursor.pos;
    cursor.skip_while(is_rust_ident_continue);
    match (cursor.since(start), cursor.peek()) {
        (b"r" | b"br" | b"cr", Some(b'"' | b'#')) => skip_rust_raw_string(cursor),
        (b"b" | b"c", Some(b'"')) => {
            cursor.advance(1);
            skip_rust_string(cursor);
        }
        (b"b", Some(b'\'')) => {
            cursor.advance(1);
            skip_rust_char(cursor);
        }
        _ => {}
    }
}

/// Moves past a Rust raw string whose prefix is behind the cursor: its `#`s, a `"`, then
/// everything up to a `"` followed by as many `#`s. When no `"` follows the `#`s, as in the raw
/// identifier `r#type`, the cursor stays put and the identifier lexes as code.
fn skip_rust_raw_string(cursor: &mut Cursor<'_>) {
    let mut hashes: usize = 0;
    while cursor.peek_at(hashes) == Some(b'#') {
        hashes = hashes.saturating_add(1);
    }
    if cursor.peek_at(hashes) != Some(b'"') {
        return;
    }
    cursor.advance(hashes.saturating_add(1));
    while let Some(byte) = cursor.peek() {
        cursor.advance(1);
        if byte == b'"' && (0..hashes).all(|at| cursor.peek_at(at) == Some(b'#')) {
            cursor.advance(hashes);
            return;
        }
    }
}

/// Whether `byte` can start a Rust identifier. Every byte of a non-ASCII character counts, so a
/// Unicode identifier stays one word.
fn is_rust_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || !byte.is_ascii()
}

/// Whether `byte` can continue a Rust identifier.
fn is_rust_ident_continue(byte: u8) -> bool {
    is_rust_ident_start(byte) || byte.is_ascii_digit()
}

/// The keywords after which a `/` opens a regular expression rather than dividing.
const REGEX_KEYWORDS: [&str; 14] = [
    "return",
    "typeof",
    "instanceof",
    "in",
    "of",
    "new",
    "delete",
    "void",
    "throw",
    "case",
    "do",
    "else",
    "yield",
    "await",
];

/// The keywords whose parenthesised head is followed by a statement, so a `/` right after the
/// `)` that closes the head opens a regular expression.
const CONDITION_KEYWORDS: [&str; 4] = ["if", "while", "for", "with"];

/// The punctuators after which a `/` opens a regular expression, other than those the lexer
/// reads by rules of their own: braces, parentheses, `.`, `!`, `<`, `++` and `--`.
const REGEX_PUNCTUATORS: &[u8] = b",=:[&|?;+-*%>~^";

/// What the last token before the cursor was, which decides whether a TypeScript `/` opens a
/// regular expression or divides, and in `.tsx` whether a `<` may open a JSX element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Previous {
    /// An expression starts: the start of the text, an operator or punctuator of
    /// [`REGEX_PUNCTUATORS`], `(`, `{`, a `${` that opens a substitution, a `<`, a spread `...`,
    /// a prefix `++` or `--`, a `!` that negates or begins `!=`, the `)` that closes the head of
    /// a [`CONDITION_KEYWORDS`] keyword, or a keyword of [`REGEX_KEYWORDS`]. A `/` opens a
    /// regular expression, except right after a `<`, as in the closing tag `</` of JSX-like text
    /// in a `.ts` file, and in `.tsx` a `<` may open a JSX element.
    Operator,
    /// `}`: a `/` opens a regular expression, except in a `/>`, as in the end of a JSX-like
    /// element after an attribute expression in a `.ts` file.
    CloseBrace,
    /// The `.` of a property access: the identifier after it is a property name, never a keyword,
    /// and a `/` divides.
    Dot,
    /// A keyword of [`CONDITION_KEYWORDS`], or the `await` of `for await`: the `(` after it opens
    /// a head, and a `/` divides.
    Condition,
    /// An operand ends: an identifier, a private name, a number, a string, template or
    /// regular-expression literal, a JSX element or fragment, a `)` other than a condition's,
    /// `]`, a postfix `++` or `--`, a non-null `!`, or another punctuator. A `/` divides.
    Operand,
}

/// The state of one level of TypeScript code: the top level of the text, a `${}` substitution
/// of a template literal or a JSX `{}` expression container.
#[derive(Debug, Default)]
struct Code {
    /// The `{` opened in this code and not yet closed; a `}` while none is open ends the level.
    braces: usize,
    /// One entry per `(` opened in this code and not yet closed, innermost last: whether it
    /// opened the head of a [`CONDITION_KEYWORDS`] keyword. A level that ends discards its
    /// entries, and a `)` that finds none ends an operand, so unbalanced text stays harmless.
    parens: Vec<bool>,
}

/// A lexical context the TypeScript lexer has entered inside the top-level code.
#[derive(Debug)]
enum Context {
    /// The code of a `${}` substitution; its template literal resumes after its `}`.
    Substitution(Code),
    /// The code of a JSX `{}` expression container; its tag or children resume after its `}`.
    JsxExpression(Code),
    /// A JSX opening tag past its name: attributes, up to the `>` or `/>` that ends it.
    JsxTag,
    /// The children of a JSX element or fragment, up to its closing tag.
    JsxChildren,
}

/// The state of the TypeScript lexer.
struct TypeScriptLexer<'t> {
    cursor: Cursor<'t>,
    comments: Vec<Comment>,
    /// Whether a `<` where an expression starts may open a JSX element, as in `.tsx` files.
    jsx: bool,
    /// The top-level code of the text.
    top: Code,
    /// The contexts entered inside the top-level code, innermost last.
    contexts: Vec<Context>,
    previous: Previous,
}

/// Every comment of the TypeScript source `text`, in order, reading JSX when `jsx` holds, as for
/// `.tsx` files. Comments lie outside `'` and `"` strings, template literals and their nested
/// `${}` substitutions (which hold code, so strings, templates and comments), and
/// regular-expression literals. A `/` opens a regular expression where an expression starts, as
/// the token before it decides: a postfix `++` or `--`, a non-null `!` and a property name after
/// `.` end an operand, while the `)` that closes an `if`, `while`, `for` or `with` head is
/// followed by a statement. With `jsx`, a `<` where an expression starts opens a JSX element or
/// fragment, unless TypeScript reads it as the type parameters of a generic arrow function: the
/// text of its children and its attribute strings are inert, its `{}` expression containers hold
/// code, and its tags may hold comments between attributes. Without `jsx`, a `<` is always an
/// operator, as in type assertions and type arguments. A string ends at an unescaped line break
/// and a regular expression at any line break, so a stray quote or slash, such as an apostrophe
/// in JSX-like text of a `.ts` file, hides at most the rest of its line; a template literal, a
/// block comment, a JSX element or a JSX attribute string left open runs to the end of the text.
fn typescript_comments(text: &str, jsx: bool) -> Vec<Comment> {
    let mut lexer = TypeScriptLexer {
        cursor: Cursor::new(text),
        comments: Vec::new(),
        jsx,
        top: Code::default(),
        contexts: Vec::new(),
        previous: Previous::Operator,
    };
    while let Some(byte) = lexer.cursor.peek() {
        match lexer.contexts.last() {
            Some(Context::JsxTag) => lexer.jsx_tag(byte),
            Some(Context::JsxChildren) => lexer.jsx_children(byte),
            Some(Context::Substitution(_) | Context::JsxExpression(_)) | None => lexer.step(byte),
        }
    }
    lexer.comments
}

impl TypeScriptLexer<'_> {
    /// The code the cursor is in: the innermost substitution or JSX expression container, or
    /// else the top level.
    fn code(&mut self) -> &mut Code {
        match self.contexts.last_mut() {
            Some(Context::Substitution(code) | Context::JsxExpression(code)) => code,
            Some(Context::JsxTag | Context::JsxChildren) | None => &mut self.top,
        }
    }

    /// Moves past the code token that starts with `byte`, the byte at the cursor.
    fn step(&mut self, byte: u8) {
        match byte {
            b'/' => self.slash(),
            b'\'' | b'"' => {
                self.cursor.advance(1);
                skip_typescript_string(&mut self.cursor, byte);
                self.previous = Previous::Operand;
            }
            b'`' => {
                self.cursor.advance(1);
                self.template();
            }
            b'{' => {
                let code = self.code();
                code.braces = code.braces.saturating_add(1);
                self.cursor.advance(1);
                self.previous = Previous::Operator;
            }
            b'}' => self.close_brace(),
            b'(' => self.open_paren(),
            b')' => self.close_paren(),
            b'<' => self.less_than(),
            b'.' => self.dot(),
            b'+' | b'-' if self.cursor.peek_at(1) == Some(byte) => self.update(),
            b'!' => self.bang(),
            b'#' => self.private_name(),
            _ if is_typescript_ident_start(byte) => self.word(),
            _ if byte.is_ascii_digit() => {
                self.cursor.skip_while(is_typescript_number_byte);
                self.previous = Previous::Operand;
            }
            _ if byte.is_ascii_whitespace() => self.cursor.advance(1),
            _ => {
                self.cursor.advance(1);
                self.previous = if REGEX_PUNCTUATORS.contains(&byte) {
                    Previous::Operator
                } else {
                    Previous::Operand
                };
            }
        }
    }

    /// Moves past the identifier or keyword at the cursor. After a `.` it is a property name,
    /// which ends an operand whatever it spells. Otherwise a keyword of [`CONDITION_KEYWORDS`],
    /// or the `await` of `for await`, is followed by a head, a keyword of [`REGEX_KEYWORDS`]
    /// starts an expression, and any other word ends an operand.
    fn word(&mut self) {
        let start = self.cursor.pos;
        self.cursor.skip_while(is_typescript_ident_continue);
        let word = self.cursor.since(start);
        let spells = |names: &[&str]| names.iter().any(|name| name.as_bytes() == word);
        self.previous = match self.previous {
            Previous::Dot => Previous::Operand,
            Previous::Condition if word == b"await" => Previous::Condition,
            _ if spells(&CONDITION_KEYWORDS) => Previous::Condition,
            _ if spells(&REGEX_KEYWORDS) => Previous::Operator,
            _ => Previous::Operand,
        };
    }

    /// Moves past the `#` at the cursor and the identifier after it: a private name such as
    /// `#in`, which ends an operand whatever it spells. A `#` without one ends an operand too.
    fn private_name(&mut self) {
        self.cursor.advance(1);
        self.cursor.skip_while(is_typescript_ident_continue);
        self.previous = Previous::Operand;
    }

    /// Moves past the `(` at the cursor, recording whether it opens the head of a
    /// [`CONDITION_KEYWORDS`] keyword.
    fn open_paren(&mut self) {
        let head = self.previous == Previous::Condition;
        self.code().parens.push(head);
        self.cursor.advance(1);
        self.previous = Previous::Operator;
    }

    /// Moves past the `)` at the cursor. When it closes the head of a [`CONDITION_KEYWORDS`]
    /// keyword a statement follows, so an expression starts; any other `)`, one that closes no
    /// `(` of its code included, ends an operand.
    fn close_paren(&mut self) {
        let head = self.code().parens.pop() == Some(true);
        self.cursor.advance(1);
        self.previous = if head {
            Previous::Operator
        } else {
            Previous::Operand
        };
    }

    /// Moves past the `.` at the cursor: a spread or rest `...`, after which an expression
    /// starts, or the `.` of a property access, after which a property name follows.
    fn dot(&mut self) {
        if self.cursor.at(b"...") {
            self.cursor.advance(3);
            self.previous = Previous::Operator;
        } else {
            self.cursor.advance(1);
            self.previous = Previous::Dot;
        }
    }

    /// Moves past the `++` or `--` at the cursor: postfix when it ends an operand on the
    /// operand's line, which it leaves an operand, and otherwise prefix, so an expression starts.
    fn update(&mut self) {
        let postfix = self.ends_operand();
        self.cursor.advance(2);
        if !postfix {
            self.previous = Previous::Operator;
        }
    }

    /// Moves past the `!` at the cursor: TypeScript's non-null assertion when it ends an operand
    /// on the operand's line and no `=` follows (`total! / 2` divides), which leaves the operand,
    /// and otherwise a negation or the start of `!=` or `!==`, so an expression starts.
    fn bang(&mut self) {
        let assertion = self.ends_operand() && self.cursor.peek_at(1) != Some(b'=');
        self.cursor.advance(1);
        if !assertion {
            self.previous = Previous::Operator;
        }
    }

    /// Whether a `++`, `--` or `!` at the cursor applies to the operand before it: the token
    /// before is an operand and no line break separates them, since a line break there makes the
    /// `++`, `--` or `!` start the next statement, as automatic semicolon insertion reads it.
    fn ends_operand(&self) -> bool {
        self.previous == Previous::Operand && !line_break_before(&self.cursor)
    }

    /// Moves past the `<` at the cursor. In `.tsx`, where an expression starts, it opens a JSX
    /// element or fragment unless it opens the type parameters of a generic arrow function.
    /// Otherwise it is an operator, or with a second `<` the shift `<<`, which opens no element.
    /// The lexer does not tell type positions from expressions, so a generic function type
    /// written where an expression could start (`type F = <T>(x: T) => T`) opens an element too.
    fn less_than(&mut self) {
        let element = self.jsx
            && self.previous == Previous::Operator
            && opens_jsx_element(&self.cursor)
            && !opens_type_parameters(&self.cursor);
        if element {
            self.open_element();
        } else {
            let width = if self.cursor.at(b"<<") { 2 } else { 1 };
            self.cursor.advance(width);
            self.previous = Previous::Operator;
        }
    }

    /// Moves past the comment, regular expression or division that the `/` at the cursor opens.
    fn slash(&mut self) {
        if self.comment() {
            return;
        }
        let regex = match self.previous {
            Previous::Operator => self.cursor.previous() != Some(b'<'),
            Previous::CloseBrace => self.cursor.peek_at(1) != Some(b'>'),
            Previous::Dot | Previous::Condition | Previous::Operand => false,
        };
        self.cursor.advance(1);
        if regex {
            skip_typescript_regex(&mut self.cursor);
            self.previous = Previous::Operand;
        } else {
            self.previous = Previous::Operator;
        }
    }

    /// Records and moves past the line or block comment that the `/` at the cursor opens, if it
    /// opens one; whether it did.
    fn comment(&mut self) -> bool {
        let start = self.cursor.pos;
        let kind = match self.cursor.peek_at(1) {
            Some(b'/') => {
                self.cursor.skip_line();
                CommentKind::Line
            }
            Some(b'*') => {
                self.cursor.advance(2);
                while self.cursor.peek().is_some() && !self.cursor.at(b"*/") {
                    self.cursor.advance(1);
                }
                self.cursor.advance(2);
                CommentKind::Block
            }
            _ => return false,
        };
        self.comments.push(Comment {
            start,
            end: self.cursor.pos,
            kind,
            doc: false,
        });
        true
    }

    /// Moves past the `}` at the cursor. It closes a `{` of its code while one is open;
    /// otherwise it ends its substitution, whose template literal resumes, or its JSX expression
    /// container, whose tag or children resume. A `}` that ends nothing is a punctuator.
    fn close_brace(&mut self) {
        self.cursor.advance(1);
        let code = self.code();
        if code.braces > 0 {
            code.braces = code.braces.saturating_sub(1);
            self.previous = Previous::CloseBrace;
            return;
        }
        match self.contexts.last() {
            Some(Context::Substitution(_)) => {
                self.contexts.pop();
                self.template();
            }
            Some(Context::JsxExpression(_)) => {
                self.contexts.pop();
            }
            Some(Context::JsxTag | Context::JsxChildren) | None => {
                self.previous = Previous::CloseBrace;
            }
        }
    }

    /// Moves through the text of a template literal, from the cursor up to and past its closing
    /// backtick, or past a `${`, whose substitution it enters.
    fn template(&mut self) {
        while let Some(byte) = self.cursor.peek() {
            match byte {
                b'\\' => self.cursor.advance(2),
                b'`' => {
                    self.cursor.advance(1);
                    self.previous = Previous::Operand;
                    return;
                }
                b'$' if self.cursor.peek_at(1) == Some(b'{') => {
                    self.cursor.advance(2);
                    self.contexts.push(Context::Substitution(Code::default()));
                    self.previous = Previous::Operator;
                    return;
                }
                _ => self.cursor.advance(1),
            }
        }
    }

    /// Enters the JSX element or fragment whose `<` is at the cursor: a fragment's children, or
    /// the opening tag of an element past its name and any type arguments (`<Table<Row>`).
    fn open_element(&mut self) {
        self.cursor.advance(1);
        self.cursor.skip_while(is_typescript_space);
        if self.cursor.peek() == Some(b'>') {
            self.cursor.advance(1);
            self.contexts.push(Context::JsxChildren);
            return;
        }
        self.cursor.skip_while(is_jsx_name_byte);
        self.cursor.skip_while(is_typescript_space);
        if self.cursor.peek() == Some(b'<') {
            skip_type_arguments(&mut self.cursor);
        }
        self.contexts.push(Context::JsxTag);
    }

    /// Moves past the part of a JSX opening tag that starts with `byte`, the byte at the cursor:
    /// a line or block comment, which TypeScript reads between attributes; the `/>` that ends a
    /// self-closing element, or the `>` after which its children follow; an attribute string,
    /// which takes no escapes and may span lines; a `{}` expression container, a spread
    /// `{...props}` included; an element given as an attribute value; or a byte of an attribute
    /// name or `=`, which is inert.
    fn jsx_tag(&mut self, byte: u8) {
        match byte {
            b'/' if self.cursor.peek_at(1) == Some(b'>') => {
                self.cursor.advance(2);
                self.contexts.pop();
                self.element_closed();
            }
            b'/' => {
                if !self.comment() {
                    self.cursor.advance(1);
                }
            }
            b'>' => {
                self.cursor.advance(1);
                self.contexts.pop();
                self.contexts.push(Context::JsxChildren);
            }
            b'\'' | b'"' => {
                self.cursor.advance(1);
                skip_jsx_string(&mut self.cursor, byte);
            }
            b'{' => self.open_expression(),
            b'<' if opens_jsx_element(&self.cursor) => self.open_element(),
            _ => self.cursor.advance(1),
        }
    }

    /// Moves past the part of JSX children that starts with `byte`, the byte at the cursor: a
    /// closing tag `</name>` or `</>`, which ends the innermost element or fragment; a nested
    /// element or fragment; a `{}` expression container; or text up to the next `<` or `{`,
    /// which is inert, comment openers, quotes, backticks and slashes included.
    fn jsx_children(&mut self, byte: u8) {
        match byte {
            b'<' if opens_closing_tag(&self.cursor) => self.close_element(),
            b'<' if opens_jsx_element(&self.cursor) => self.open_element(),
            b'{' => self.open_expression(),
            _ => {
                self.cursor.advance(1);
                self.cursor.skip_while(|next| !matches!(next, b'<' | b'{'));
            }
        }
    }

    /// Enters the JSX `{}` expression container whose `{` is at the cursor: code, in which an
    /// expression starts.
    fn open_expression(&mut self) {
        self.cursor.advance(1);
        self.contexts.push(Context::JsxExpression(Code::default()));
        self.previous = Previous::Operator;
    }

    /// Moves past the closing tag at the cursor, `</`, a name and `>`, and ends the innermost
    /// element or fragment. A closing tag left open ends where its name does.
    fn close_element(&mut self) {
        self.cursor.advance(1);
        self.cursor.skip_while(is_typescript_space);
        self.cursor.advance(1);
        self.cursor
            .skip_while(|byte| is_jsx_name_byte(byte) || is_typescript_space(byte));
        if self.cursor.peek() == Some(b'>') {
            self.cursor.advance(1);
        }
        self.contexts.pop();
        self.element_closed();
    }

    /// Follows a JSX element or fragment whose context has just ended. Nested in the children or
    /// the tag of another element, it leaves that element's context to resume; otherwise its code
    /// resumes after it, and the element ends an operand.
    fn element_closed(&mut self) {
        if !matches!(
            self.contexts.last(),
            Some(Context::JsxTag | Context::JsxChildren)
        ) {
            self.previous = Previous::Operand;
        }
    }
}

/// Moves past the rest of a TypeScript string whose opening `quote` is behind the cursor: `\`
/// escapes the next byte, a line continuation included, and an unescaped line break ends the
/// string unclosed.
fn skip_typescript_string(cursor: &mut Cursor<'_>, quote: u8) {
    while let Some(byte) = cursor.peek() {
        if byte == b'\n' {
            return;
        }
        cursor.advance(1);
        if byte == quote {
            return;
        }
        if byte == b'\\' {
            cursor.advance(if cursor.at(b"\r\n") { 2 } else { 1 });
        }
    }
}

/// Moves past the rest of a regular expression whose opening `/` is behind the cursor, and its
/// flags: `\` escapes the next byte, a `[...]` class may hold `/`, and a line break ends it
/// unclosed.
fn skip_typescript_regex(cursor: &mut Cursor<'_>) {
    let mut in_class = false;
    while let Some(byte) = cursor.peek() {
        match byte {
            b'\n' => return,
            b'\\' => {
                // An escaped line break still ends the expression.
                let width = if cursor.at(b"\\\n") { 1 } else { 2 };
                cursor.advance(width);
            }
            b'/' if !in_class => {
                cursor.advance(1);
                cursor.skip_while(is_typescript_ident_continue);
                return;
            }
            b'[' => {
                in_class = true;
                cursor.advance(1);
            }
            b']' => {
                in_class = false;
                cursor.advance(1);
            }
            _ => cursor.advance(1),
        }
    }
}

/// Moves past the rest of a JSX attribute string whose opening `quote` is behind the cursor: it
/// takes no escapes, may span lines and ends at the next `quote`, or at the end of the text.
fn skip_jsx_string(cursor: &mut Cursor<'_>, quote: u8) {
    while let Some(byte) = cursor.peek() {
        cursor.advance(1);
        if byte == quote {
            return;
        }
    }
}

/// Moves past the type arguments `<…>` at the cursor that follow a JSX element name, as in
/// `<Table<Row> rows={rows} />`: nested `<` and `>` balance, the `>` of `=>` counts for neither,
/// and arguments left open run to the end of the text.
fn skip_type_arguments(cursor: &mut Cursor<'_>) {
    let mut depth: usize = 0;
    while let Some(byte) = cursor.peek() {
        let width = if cursor.at(b"=>") { 2 } else { 1 };
        cursor.advance(width);
        match byte {
            b'<' => depth = depth.saturating_add(1),
            b'>' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// Whether a line break lies between the cursor and the last byte before it that is not
/// whitespace.
fn line_break_before(cursor: &Cursor<'_>) -> bool {
    cursor
        .bytes
        .get(..cursor.pos)
        .unwrap_or_default()
        .iter()
        .rev()
        .take_while(|byte| byte.is_ascii_whitespace())
        .any(|&byte| byte == b'\n')
}

/// The offset from the cursor of the first byte at or past the offset `ahead` that `keep`
/// rejects, or of the end of the text.
fn skip_ahead(cursor: &Cursor<'_>, ahead: usize, keep: fn(u8) -> bool) -> usize {
    let mut at = ahead;
    while cursor.peek_at(at).is_some_and(keep) {
        at = at.saturating_add(1);
    }
    at
}

/// The identifier that starts at the offset `ahead` from the cursor, empty when none does, and
/// the offset just past it.
fn word_at<'t>(cursor: &Cursor<'t>, ahead: usize) -> (&'t [u8], usize) {
    let end = skip_ahead(cursor, ahead, is_typescript_ident_continue);
    let word = cursor
        .pos
        .checked_add(ahead)
        .zip(cursor.pos.checked_add(end))
        .and_then(|(from, to)| cursor.bytes.get(from..to))
        .unwrap_or_default();
    (word, end)
}

/// Whether the `<` at the cursor can open a JSX element or fragment: after any whitespace, an
/// element name or the `>` of `<>` follows.
fn opens_jsx_element(cursor: &Cursor<'_>) -> bool {
    cursor
        .peek_at(skip_ahead(cursor, 1, is_typescript_space))
        .is_some_and(|byte| byte == b'>' || is_typescript_ident_start(byte))
}

/// Whether the `<` at the cursor, among JSX children, opens a closing tag: after any whitespace,
/// a `/` follows.
fn opens_closing_tag(cursor: &Cursor<'_>) -> bool {
    cursor.peek_at(skip_ahead(cursor, 1, is_typescript_space)) == Some(b'/')
}

/// Whether the `<` at the cursor, where an expression starts in `.tsx`, opens the type
/// parameters of a generic arrow function rather than a JSX element, as TypeScript decides
/// there: a name, after an optional `const`, followed by `,` (`<T,>(x: T) => x`), by `=`
/// (`<T = unknown>`), or by `extends` and then anything but `=`, `>` or `/`
/// (`<T extends object>(x: T) => x`). Whitespace may surround each token; `<T extends>` is an
/// element with the attribute `extends`.
fn opens_type_parameters(cursor: &Cursor<'_>) -> bool {
    let (first, mut end) = word_at(cursor, skip_ahead(cursor, 1, is_typescript_space));
    if first == b"const" {
        let next = skip_ahead(cursor, end, is_typescript_space);
        if cursor.peek_at(next).is_some_and(is_typescript_ident_start) {
            end = word_at(cursor, next).1;
        }
    }
    let after = skip_ahead(cursor, end, is_typescript_space);
    if matches!(cursor.peek_at(after), Some(b',' | b'=')) {
        return true;
    }
    let (keyword, end) = word_at(cursor, after);
    let next = skip_ahead(cursor, end, is_typescript_space);
    keyword == b"extends" && !matches!(cursor.peek_at(next), Some(b'=' | b'>' | b'/'))
}

/// Whether `byte` is TypeScript whitespace or a line break, among ASCII bytes.
fn is_typescript_space(byte: u8) -> bool {
    byte.is_ascii_whitespace()
}

/// Whether `byte` can continue a JSX element name: an identifier byte, or the `.`, `:` or `-` of
/// a member, namespaced or dashed name.
fn is_jsx_name_byte(byte: u8) -> bool {
    is_typescript_ident_continue(byte) || matches!(byte, b'.' | b':' | b'-')
}

/// Whether `byte` can start a TypeScript identifier. Every byte of a non-ASCII character counts,
/// so a Unicode identifier stays one word.
fn is_typescript_ident_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$') || !byte.is_ascii()
}

/// Whether `byte` can continue a TypeScript identifier.
fn is_typescript_ident_continue(byte: u8) -> bool {
    is_typescript_ident_start(byte) || byte.is_ascii_digit()
}

/// Whether `byte` can continue a TypeScript number: digits, letters for prefixes, exponents and
/// suffixes, `_` separators and the decimal point.
fn is_typescript_number_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.')
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        AuditRule, Comment, Language, Layer, TagKind, TagSite, file_citations, file_tags,
        required_tags, scan_test_tags,
    };

    /// A rule ID assembled at run time: this file is itself scanned for citations, so it spells
    /// out none.
    fn rule_id(member: &str, number: u16) -> String {
        format!("{member}-{number:03}")
    }

    /// `template` with every `@ID` replaced by `id`.
    fn fill(template: &str, id: &str) -> String {
        template.replace("@ID", id)
    }

    /// A tag-scanner case: its name, the language it is lexed as, its source with `@ID` for a
    /// rule ID, and the 1-based lines of its real tags.
    type TagCase = (&'static str, Language, &'static str, &'static [usize]);

    const TAG_CASES: &[TagCase] = &[
        ("rust line comment", Language::Rust, "// audit: @ID\n", &[1]),
        (
            "rust tag after code",
            Language::Rust,
            "fn f() {\n    foo(); // audit: @ID\n}\n",
            &[2],
        ),
        (
            "rust outer doc comment",
            Language::Rust,
            "/// audit: @ID\nfn f() {}\n",
            &[1],
        ),
        (
            "rust tag without spaces after extra slashes",
            Language::Rust,
            "////audit:@ID\n",
            &[1],
        ),
        (
            "rust tags one per comment",
            Language::Rust,
            "// audit: @ID\n#[test]\nfn f() {} // audit: @ID\n",
            &[1, 3],
        ),
        (
            "rust inner doc comment",
            Language::Rust,
            "//! audit: @ID\n",
            &[],
        ),
        (
            "rust multi-line string",
            Language::Rust,
            "let s = \"\n// audit: @ID\n\";\n// audit: @ID\n",
            &[4],
        ),
        (
            "rust escaped quote in a string",
            Language::Rust,
            "let s = \"a \\\" b\n// audit: @ID\n\";\n",
            &[],
        ),
        (
            "rust string ending in an escaped backslash",
            Language::Rust,
            "let s = \"\\\\\"; // audit: @ID\n",
            &[1],
        ),
        (
            "rust tag text in a string",
            Language::Rust,
            "let s = \"// audit: @ID\";\n",
            &[],
        ),
        (
            "rust raw string",
            Language::Rust,
            "let s = r#\"a \"quoted\" line\n// audit: @ID\n\"#;\n// audit: @ID\n",
            &[4],
        ),
        (
            "rust byte raw string with two hashes",
            Language::Rust,
            "let s = br##\"x \"# still raw\n// audit: @ID\n\"##;\n",
            &[],
        ),
        (
            "rust raw string takes no escapes",
            Language::Rust,
            "let s = r\"\\\"; // audit: @ID\n",
            &[1],
        ),
        (
            "rust byte string",
            Language::Rust,
            "let s = b\"\n// audit: @ID\n\";\n",
            &[],
        ),
        (
            "rust C string",
            Language::Rust,
            "let s = c\"\n// audit: @ID\n\";\n",
            &[],
        ),
        (
            "rust nested block comment",
            Language::Rust,
            "/* outer /* inner */\n// audit: @ID\n*/\n// audit: @ID\n",
            &[4],
        ),
        (
            "rust block doc comment",
            Language::Rust,
            "/**\n// audit: @ID\n */\nfn f() {}\n",
            &[],
        ),
        (
            "rust double-quote character literal",
            Language::Rust,
            "let q = '\"';\n// audit: @ID\n",
            &[2],
        ),
        (
            "rust escaped quote character literal",
            Language::Rust,
            "let q = '\\''; // audit: @ID\n",
            &[1],
        ),
        (
            "rust byte literal of a double quote",
            Language::Rust,
            "let q = b'\"'; // audit: @ID\n",
            &[1],
        ),
        (
            "rust unicode escape character literal",
            Language::Rust,
            "let q = '\\u{2019}'; // audit: @ID\n",
            &[1],
        ),
        (
            "rust non-ASCII character literal",
            Language::Rust,
            "let q = '\u{e9}'; // audit: @ID\n",
            &[1],
        ),
        (
            "rust lifetimes and labels",
            Language::Rust,
            "fn f<'a>(x: &'a str) -> &'static str {\n    'outer: loop {\n        break 'outer;\n    }\n    x\n}\n// audit: @ID\n",
            &[7],
        ),
        (
            "rust raw identifier",
            Language::Rust,
            "let r#type = 1; // audit: @ID\n",
            &[1],
        ),
        (
            "rust quoted example in a comment",
            Language::Rust,
            "// see \"// audit: @ID\"\n",
            &[],
        ),
        (
            "rust tag mentioned after other text",
            Language::Rust,
            "// a tag reads // audit: @ID\n",
            &[],
        ),
        (
            "typescript line comment",
            Language::TypeScript,
            "// audit: @ID\n",
            &[1],
        ),
        (
            "typescript tag after code",
            Language::TypeScript,
            "expect(parse(\"x\")).toBe(true); // audit: @ID\n",
            &[1],
        ),
        (
            "typescript multi-line template literal",
            Language::TypeScript,
            "const t = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[4],
        ),
        (
            "typescript nested template substitutions",
            Language::TypeScript,
            "const t = `a ${ `b ${ { k: 1 }.k }\n// audit: @ID\n` } c`;\n// audit: @ID\n",
            &[4],
        ),
        (
            "typescript block comment inside a substitution",
            Language::TypeScript,
            "const t = `${ x /* } */ }\n// audit: @ID\n`;\n",
            &[],
        ),
        (
            "typescript string inside a substitution",
            Language::TypeScript,
            "const t = `${ \"}\" }\n// audit: @ID\n`;\n",
            &[],
        ),
        (
            "typescript escaped backtick in a template literal",
            Language::TypeScript,
            "const t = `\\`\n// audit: @ID\n`;\n",
            &[],
        ),
        (
            "typescript doc block comment",
            Language::TypeScript,
            "/**\n * // audit: @ID\n */\nit(\"x\", () => {});\n",
            &[],
        ),
        (
            "typescript block comment",
            Language::TypeScript,
            "/* // audit: @ID */\n",
            &[],
        ),
        (
            "typescript regex class holding a comment opener",
            Language::TypeScript,
            "const r = /[// audit: @ID]/;\n",
            &[],
        ),
        (
            "typescript tag after a regex with quotes and slashes",
            Language::TypeScript,
            "const r = /[\"'`]\\/[/]/g; // audit: @ID\n",
            &[1],
        ),
        (
            "typescript regex after a keyword",
            Language::TypeScript,
            "function f(s) {\n  return /'/.test(s); // audit: @ID\n}\n",
            &[2],
        ),
        (
            "typescript single-quoted string",
            Language::TypeScript,
            "const s = '// audit: @ID';\n",
            &[],
        ),
        (
            "typescript double-quoted string",
            Language::TypeScript,
            "const s = \"// audit: @ID\";\n",
            &[],
        ),
        (
            "typescript tag after a template literal closes",
            Language::TypeScript,
            "const t = `x ${y}`; // audit: @ID\n",
            &[1],
        ),
        (
            "typescript tag after a division",
            Language::TypeScript,
            "const half = total / 2; // audit: @ID\n",
            &[1],
        ),
        (
            "typescript tag after a JSX closing tag",
            Language::TypeScript,
            "render(<div>text</div>); // audit: @ID\n",
            &[1],
        ),
        (
            "typescript tag after a self-closing JSX element",
            Language::TypeScript,
            "render(<Screen onExit={() => {}} />); // audit: @ID\n",
            &[1],
        ),
        (
            "typescript stray apostrophe hides only its line",
            Language::TypeScript,
            "render(<p>Don't</p>);\n// audit: @ID\n",
            &[2],
        ),
        (
            "typescript quoted example in a comment",
            Language::TypeScript,
            "// see \"// audit: @ID\"\n",
            &[],
        ),
        (
            "typescript postfix increment before a division",
            Language::TypeScript,
            "let n = 2;\nconst half = n++ / 2; const text = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[5],
        ),
        (
            "typescript postfix decrement before a division",
            Language::TypeScript,
            "let n = 2;\nconst half = n-- / 2; const text = `\n// audit: @ID\n`;\n",
            &[],
        ),
        (
            "typescript plus after a postfix increment before a regex",
            Language::TypeScript,
            "const s = n+++/'/.source; // audit: @ID\n",
            &[1],
        ),
        (
            "typescript increment after a line break is prefix",
            Language::TypeScript,
            "let n = 2\n++\n/'/.lastIndex; // audit: @ID\n",
            &[3],
        ),
        (
            "typescript non-null assertion before a division",
            Language::TypeScript,
            "const half = total! / 2; const text = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[4],
        ),
        (
            "typescript inequality before a regex",
            Language::TypeScript,
            "const ne = x != /'/.source; // audit: @ID\n",
            &[1],
        ),
        (
            "typescript negation after a line break",
            Language::TypeScript,
            "const ok = x\n!/'/.test(s); // audit: @ID\n",
            &[2],
        ),
        (
            "typescript keyword property name before a division",
            Language::TypeScript,
            "const half = x.of / 2; const text = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[4],
        ),
        (
            "typescript keyword name after optional chaining before a division",
            Language::TypeScript,
            "const half = x?.return / 2; const text = `\n// audit: @ID\n`;\n",
            &[],
        ),
        (
            "typescript private name spelled as a keyword before a division",
            Language::TypeScript,
            "class C {\n  #in = 4;\n  half() {\n    return this.#in / 2 + `\n// audit: @ID\n`.length;\n  }\n}\n// audit: @ID\n",
            &[9],
        ),
        (
            "typescript spread before a regex",
            Language::TypeScript,
            "const chars = [.../'/.source]; // audit: @ID\n",
            &[1],
        ),
        (
            "typescript regex after an if head",
            Language::TypeScript,
            "if (ok) /'/.test(s); // audit: @ID\n",
            &[1],
        ),
        (
            "typescript regex after a for head with nested parentheses",
            Language::TypeScript,
            "for (let i = 0; (i) < n; i++) /'/.test(s); // audit: @ID\n",
            &[1],
        ),
        (
            "typescript regex after a for await head",
            Language::TypeScript,
            "for await (const x of xs) /'/.test(x); // audit: @ID\n",
            &[1],
        ),
        (
            "typescript division after a call inside an if head",
            Language::TypeScript,
            "if (f(x) / 2) { const text = `\n// audit: @ID\n`; }\n// audit: @ID\n",
            &[4],
        ),
        (
            "typescript unbalanced closing parentheses",
            Language::TypeScript,
            ")) / 2; const text = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[4],
        ),
        (
            "typescript parentheses of a substitution stay inside it",
            Language::TypeScript,
            "if (`${ ( }`) /'/.test(s); // audit: @ID\n",
            &[1],
        ),
        (
            "typescript regex after an if head inside a substitution",
            Language::TypeScript,
            "const t = `${ (() => { if (ok) /'/.test(s); return 1; })() }`; // audit: @ID\n",
            &[1],
        ),
        (
            "typescript comment after an object literal in a substitution",
            Language::TypeScript,
            "const t = `${ { a: 1 }\n// audit: @ID\n}`;\n",
            &[2],
        ),
        (
            "typescript type assertion before a division",
            Language::TypeScript,
            "const half = <number>total / 2; const text = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx JSX child text",
            Language::Tsx,
            "render(<pre>\n// audit: @ID\n</pre>);\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx apostrophe in JSX text is plain text",
            Language::Tsx,
            "render(<p>Don't</p>);\n// audit: @ID\n",
            &[2],
        ),
        (
            "tsx quotes, backticks and slashes in JSX text",
            Language::Tsx,
            "render(<p>\"a\" `b` /c/ // audit: @ID {'{'} d</p>); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx nested elements and fragments",
            Language::Tsx,
            "render(\n  <>\n    <div>\n      <span>\n// audit: @ID\n      </span>\n    </div>\n    <br />\n  </>,\n);\n// audit: @ID\n",
            &[11],
        ),
        (
            "tsx attribute string holding tag text",
            Language::Tsx,
            "render(<input placeholder=\"// audit: @ID\" />); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx multi-line attribute string",
            Language::Tsx,
            "render(<p title='a\n// audit: @ID\nb'>x</p>);\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx attribute string takes no escapes",
            Language::Tsx,
            "render(<p title=\"a\\\">x</p>);\n// audit: @ID\n",
            &[2],
        ),
        (
            "tsx template in an expression container",
            Language::Tsx,
            "render(<p>{`\n// audit: @ID\n`}</p>);\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx template in an attribute expression",
            Language::Tsx,
            "render(<p title={`\n// audit: @ID\n`} />);\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx braces inside an expression container",
            Language::Tsx,
            "render(<p>{ { a: 1 }.a }\n// audit: @ID\n</p>);\n",
            &[],
        ),
        (
            "tsx comment after an object literal in an expression container",
            Language::Tsx,
            "render(\n  <p>\n    {[{ a: 1 }].map((o) => o.a)\n    // audit: @ID\n    }\n  </p>,\n);\n",
            &[4],
        ),
        (
            "tsx division after an element",
            Language::Tsx,
            "const n = <a /> / 2; const text = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx regex in an expression container",
            Language::Tsx,
            "render(<p>{/'/.test(s) && 'x'}</p>); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx real line comment in an expression container",
            Language::Tsx,
            "render(\n  <p>\n    {\n      // audit: @ID\n    }\n  </p>,\n);\n",
            &[4],
        ),
        (
            "tsx real block comment in an expression container",
            Language::Tsx,
            "render(<p>{/* // audit: @ID */}</p>); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx real comment between attributes",
            Language::Tsx,
            "render(\n  <p\n    // audit: @ID\n    id=\"x\"\n  />,\n);\n",
            &[3],
        ),
        (
            "tsx tag after a closing tag",
            Language::Tsx,
            "render(<div>text</div>); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx tag after a self-closing element",
            Language::Tsx,
            "render(<Screen onExit={() => {}} />); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx tag after a fragment",
            Language::Tsx,
            "render(<>text</>); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx elements after an arrow, a ternary and a return",
            Language::Tsx,
            "const f = () => <p>a</p>;\nconst g = ok ? <p>\n// audit: @ID\n</p> : null;\nfunction h() {\n  return <p>\n// audit: @ID\n</p>;\n}\n// audit: @ID\n",
            &[10],
        ),
        (
            "tsx element type arguments",
            Language::Tsx,
            "render(<Table<Row> rows={rows} />); // audit: @ID\n",
            &[1],
        ),
        (
            "tsx element as an attribute value",
            Language::Tsx,
            "render(<Slot fallback=<p>x</p>>\n// audit: @ID\n</Slot>);\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx unterminated element",
            Language::Tsx,
            "render(<div>\n// audit: @ID\n",
            &[],
        ),
        (
            "tsx generic arrow with a trailing comma",
            Language::Tsx,
            "const id = <T,>(x: T) => x; // audit: @ID\n",
            &[1],
        ),
        (
            "tsx generic arrow with a constraint",
            Language::Tsx,
            "const id = <T extends object>(x: T) => x; // audit: @ID\n",
            &[1],
        ),
        (
            "tsx generic arrow with a const parameter and a default",
            Language::Tsx,
            "const id = <const T = unknown>(x: T) => x; // audit: @ID\n",
            &[1],
        ),
        (
            "tsx extends attribute stays an element",
            Language::Tsx,
            "render(<T extends>\n// audit: @ID\n</T>);\n// audit: @ID\n",
            &[4],
        ),
        (
            "tsx comparisons and shifts",
            Language::Tsx,
            "const lt = a < b && c<d;\nconst s = a <<b;\nconst t = `\n// audit: @ID\n`;\n// audit: @ID\n",
            &[6],
        ),
    ];

    /// A malformed tag in a real comment: its name, language, source with `@ID` for a rule ID,
    /// and the 1-based line the error names.
    type MalformedCase = (&'static str, Language, &'static str, usize);

    const MALFORMED_CASES: &[MalformedCase] = &[
        (
            "rust two IDs in one comment",
            Language::Rust,
            "fn f() {}\n\n// audit: @ID, @ID\n",
            3,
        ),
        ("rust empty tag", Language::Rust, "// audit:\n", 1),
        (
            "rust doc comment tag with trailing text",
            Language::Rust,
            "/// audit: @ID and more\n",
            1,
        ),
        (
            "typescript text after the ID",
            Language::TypeScript,
            "const a = `x`;\n// audit: @ID extra\n",
            2,
        ),
        (
            "typescript two tags in one comment",
            Language::TypeScript,
            "// audit: @ID // audit: @ID\n",
            1,
        ),
        (
            "typescript two IDs after an if-head regex",
            Language::TypeScript,
            "if (ok) /'/.test(s); // audit: @ID, @ID\n",
            1,
        ),
        (
            "tsx text after the ID in an expression container",
            Language::Tsx,
            "render(\n  <p>\n    {\n      // audit: @ID extra\n    }\n  </p>,\n);\n",
            4,
        ),
    ];

    /// A citation case: its name, a Rust source with `@ID` for a rule ID, and each citation's
    /// 1-based line and whether it lies in a doc comment.
    type DocCase = (&'static str, &'static str, &'static [(usize, bool)]);

    const DOC_CASES: &[DocCase] = &[
        (
            "four slashes open an ordinary comment",
            "//// @ID\nfn f() {}\n",
            &[(1, false)],
        ),
        (
            "doc-looking text in a string",
            "const S: &str = \"/// @ID\";\n",
            &[(1, false)],
        ),
        (
            "doc-looking line in a multi-line string",
            "const S: &str = \"\n/// @ID\n\";\n",
            &[(2, false)],
        ),
        (
            "doc-looking text in a raw string",
            "const S: &str = r#\"/// @ID\"#;\n",
            &[(1, false)],
        ),
        (
            "doc-looking line in an ordinary block comment",
            "/*\n/// @ID\n*/\nfn f() {}\n",
            &[(2, false)],
        ),
        (
            "doc block nested in an ordinary block comment",
            "/* /** @ID */ */\n",
            &[(1, false)],
        ),
        (
            "three stars open an ordinary block comment",
            "/*** @ID */\n",
            &[(1, false)],
        ),
        (
            "empty block comment before a line comment",
            "/**/ // @ID\n",
            &[(1, false)],
        ),
        ("ordinary line comment", "// @ID\n", &[(1, false)]),
        ("word in code", "let x = @ID;\n", &[(1, false)]),
        (
            "outer line doc comment",
            "/// @ID\nfn f() {}\n",
            &[(1, true)],
        ),
        ("inner line doc comment", "//! @ID\n", &[(1, true)]),
        (
            "multi-line outer block doc comment",
            "/**\n * Implements\n * @ID.\n */\nfn f() {}\n",
            &[(3, true)],
        ),
        ("inner block doc comment", "/*! @ID */\n", &[(1, true)]),
        (
            "multi-line inner block doc comment",
            "/*!\n@ID\n*/\n",
            &[(2, true)],
        ),
        (
            "block doc comment holding a nested comment",
            "/** a /* b */ @ID */\nfn f() {}\n",
            &[(1, true)],
        ),
        (
            "doc comment after a double-quote character literal",
            "const Q: char = '\"';\n/// @ID\nfn f() {}\n",
            &[(2, true)],
        ),
        (
            "doc comment and code citations",
            "/// @ID\nconst A: &str = \"@ID\";\n",
            &[(1, true), (2, false)],
        ),
    ];

    /// Openers left unterminated, for the totality test.
    const LEXER_FRAGMENTS: &[&str] = &[
        "'", "b'", "'\\", "'\u{e9}", "\u{e9}'", "r#", "r##\"", "br#\"x\"", "\"\\", "c\"", "r#type",
        "/", "/*", "/**", "/*!", "/* /*", "`", "`${", "`${`${", "${}", "}", "}}`", "/[", "/\\",
        "</", "/>", "x++ /", "x\n--/", "x! /", "!=/", "x.of /", "...", "#in /", "#", ")", "if ((",
        "<", "<<", "<>", "</>", "<T,>", "<a b=<c>",
    ];

    /// Longer TypeScript and JSX constructs left unterminated, for the totality test.
    const LEXER_CONSTRUCTS: &[&str] = &[
        "for await (x) /",
        "<const T = U>",
        "<T extends V>",
        "<T extends>",
        "<k<L<M>=>N>/>",
        "<a b='x' c=\"y\" d={`${e}`} {...f} // g\n>h{i}<j/><></></a>",
    ];

    /// A Rust endpoint test whose tag-shaped text is all inert.
    const INERT_RUST_TEST: &str = r##"//! Endpoint tests whose tag-shaped text is all inert.

/* A block comment:
// audit: @ID
*/

/// ```text
/// see "// audit: @ID"
/// ```
#[test]
fn quotes_tags() {
    let multi_line = "
// audit: @ID
";
    let raw = r#"
// audit: @ID
"#;
    let quote = '"';
    assert_ne!(multi_line, raw);
    assert_eq!(quote, '"');
}
"##;

    /// A Vitest file whose tag-shaped text is all inert.
    const INERT_TYPESCRIPT_TEST: &str = r#"/**
 * Vitest cases whose tag-shaped text is all inert:
 * // audit: @ID
 */
const template = `
// audit: @ID
`;
const nested = `${`
// audit: @ID
`}`;
const quoted = '// audit: @ID';
const pattern = /[// audit: @ID]/;

it("keeps every tag-shaped text inert", () => {
  expect(`${template}${nested}${quoted}`).not.toMatch(pattern);
});
"#;

    /// A Vitest render test whose tag-shaped text is all inert: JSX text, a JSX attribute string,
    /// and a template literal in an expression container.
    const INERT_TSX_TEST: &str = r#"it("renders the map", () => {
  render(
    <Screen title="// audit: @ID">
      <pre>
// audit: @ID
      </pre>
      <>Don't // audit: @ID</>
      {`
// audit: @ID
`}
    </Screen>,
  );
});
"#;

    /// A real tag appended to [`INERT_RUST_TEST`], on the fourth line it adds.
    const REAL_RUST_TAG: &str = "\n#[test]\nfn proves_the_rule() {\n    // audit: @ID\n}\n";

    /// A real tag appended to [`INERT_TYPESCRIPT_TEST`], on the third line it adds.
    const REAL_TYPESCRIPT_TAG: &str = "\nit(\"proves the rule\", () => {\n  // audit: @ID\n});\n";

    /// A real tag appended to [`INERT_TSX_TEST`], after a self-closing element on the first line
    /// it adds.
    const REAL_TSX_TAG: &str = "render(<Screen onExit={() => {}} />); // audit: @ID\n";

    /// A directory of its own under the system temporary directory, removed with its contents
    /// when dropped.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        /// An empty tree holding a `crates/` directory, named after `label` and this process.
        fn new(label: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("audit-index-{label}-{}", std::process::id()));
            if root.exists() {
                fs::remove_dir_all(&root)
                    .unwrap_or_else(|err| panic!("cannot clear {}: {err}", root.display()));
            }
            fs::create_dir_all(root.join("crates"))
                .unwrap_or_else(|err| panic!("cannot create {}: {err}", root.display()));
            Self { root }
        }

        /// Writes `text` to `relative` under the tree, creating its directories.
        fn write(&self, relative: &str, text: &str) {
            let path = self.root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)
                    .unwrap_or_else(|err| panic!("cannot create {}: {err}", parent.display()));
            }
            fs::write(&path, text)
                .unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            // A failed removal leaves only a stale directory, which the next run clears.
            if let Err(err) = fs::remove_dir_all(&self.root) {
                eprintln!(
                    "cannot remove the temporary tree {}: {err}",
                    self.root.display()
                );
            }
        }
    }

    /// An online rule with ID `id` on `layer`: `client` needs a frontend tag, `both` an endpoint
    /// tag and a frontend tag.
    fn layer_rule(id: &str, layer: Layer) -> AuditRule {
        AuditRule {
            id: id.to_owned(),
            file: PathBuf::from("online/TREE.md"),
            line: 1,
            layer: Some(layer),
            halt: None,
        }
    }

    /// Asserts that `comments` lie in order and apart on character boundaries of `text`, each
    /// opening with `//` or `/*`.
    fn assert_well_formed(text: &str, comments: &[Comment]) {
        let mut done = 0;
        for comment in comments {
            assert!(done <= comment.start, "{text:?}: {comment:?} overlaps");
            let body = text
                .get(comment.start..comment.end)
                .unwrap_or_else(|| panic!("{text:?}: {comment:?} is no span of the text"));
            assert!(
                body.starts_with("//") || body.starts_with("/*"),
                "{text:?}: {comment:?} opens no comment"
            );
            done = comment.end;
        }
    }

    #[test]
    fn tags_are_read_only_from_real_line_comments() {
        let id = rule_id("TAGRULE", 7);
        for &(name, language, source, tagged_lines) in TAG_CASES {
            let text = fill(source, &id);
            let tags = file_tags(Path::new(name), language, &text)
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            let expected: Vec<(usize, &str)> = tagged_lines
                .iter()
                .map(|&line| (line, id.as_str()))
                .collect();
            assert_eq!(tags, expected, "{name}");
        }
    }

    #[test]
    fn malformed_tags_in_real_comments_fail_at_their_line() {
        let id = rule_id("TAGRULE", 8);
        for &(name, language, source, line) in MALFORMED_CASES {
            let path = Path::new(name);
            let err = file_tags(path, language, &fill(source, &id))
                .err()
                .unwrap_or_else(|| panic!("{name}: the malformed tag was accepted"));
            assert_eq!((err.path.as_path(), err.line), (path, line), "{name}");
            assert!(
                err.message.starts_with("malformed audit tag"),
                "{name}: {err}"
            );
        }
    }

    #[test]
    fn only_rust_doc_comments_count_as_documentation() {
        let id = rule_id("DOCRULE", 9);
        let file = Path::new("crates/domain/src/online/case.rs");
        for &(name, source, expected) in DOC_CASES {
            let citations = file_citations(file, &fill(source, &id));
            assert!(
                citations
                    .iter()
                    .all(|citation| citation.id == id && citation.path == file),
                "{name}"
            );
            let found: Vec<(usize, bool)> = citations
                .iter()
                .map(|citation| (citation.line, citation.in_doc_comment))
                .collect();
            assert_eq!(found, expected, "{name}");
        }
    }

    #[test]
    fn lexers_are_total_on_every_prefix() {
        let id = rule_id("LEXRULE", 1);
        let mut texts: Vec<String> = TAG_CASES
            .iter()
            .map(|case| case.2)
            .chain(MALFORMED_CASES.iter().map(|case| case.2))
            .chain(DOC_CASES.iter().map(|case| case.1))
            .chain(LEXER_FRAGMENTS.iter().copied())
            .chain(LEXER_CONSTRUCTS.iter().copied())
            .chain([INERT_RUST_TEST, INERT_TYPESCRIPT_TEST, INERT_TSX_TEST])
            .map(|source| fill(source, &id))
            .collect();
        texts.push(LEXER_FRAGMENTS.concat());
        texts.push(LEXER_CONSTRUCTS.concat());
        for text in &texts {
            let ends = text.char_indices().map(|(at, _)| at).chain([text.len()]);
            for end in ends {
                let prefix = text.get(..end).unwrap_or_default();
                for language in [Language::Rust, Language::TypeScript, Language::Tsx] {
                    assert_well_formed(prefix, &language.comments(prefix));
                }
            }
        }
    }

    #[test]
    fn inert_tag_text_cannot_satisfy_a_missing_tag() {
        let id = rule_id("TREE", 1);
        let tree = TempTree::new("inert");
        tree.write(
            "crates/api/tests/endpoints_x.rs",
            &fill(INERT_RUST_TEST, &id),
        );
        tree.write("frontend/src/x.test.ts", &fill(INERT_TYPESCRIPT_TEST, &id));
        tree.write(
            "frontend/src/screens/Screen.test.tsx",
            &fill(INERT_TSX_TEST, &id),
        );
        let sites = scan_test_tags(&tree.root).unwrap_or_else(|err| panic!("{err}"));
        assert_eq!(sites, Vec::<TagSite>::new());
        for layer in [Layer::Client, Layer::Both] {
            let kinds = sites.iter().map(|site| &site.kind);
            assert!(
                !required_tags(&layer_rule(&id, layer)).satisfied_by(kinds),
                "{layer}"
            );
        }
    }

    #[test]
    fn real_tags_beside_inert_text_are_found_exactly() {
        let id = rule_id("TREE", 2);
        let tree = TempTree::new("real");
        let rust = fill(INERT_RUST_TEST, &id);
        let typescript = fill(INERT_TYPESCRIPT_TEST, &id);
        let tsx = fill(INERT_TSX_TEST, &id);
        tree.write(
            "crates/api/tests/endpoints_x.rs",
            &format!("{rust}{}", fill(REAL_RUST_TAG, &id)),
        );
        tree.write(
            "frontend/src/x.test.ts",
            &format!("{typescript}{}", fill(REAL_TYPESCRIPT_TAG, &id)),
        );
        let screen = "frontend/src/screens/Screen.test.tsx";
        tree.write(screen, &format!("{tsx}{}", fill(REAL_TSX_TAG, &id)));
        let sites = scan_test_tags(&tree.root).unwrap_or_else(|err| panic!("{err}"));
        let site = |path: &str, line: usize, kind: TagKind| TagSite {
            id: id.clone(),
            path: PathBuf::from(path),
            line,
            kind,
        };
        let expected = vec![
            site(
                "crates/api/tests/endpoints_x.rs",
                rust.lines().count() + 4,
                TagKind::ApiEndpoints,
            ),
            site(screen, tsx.lines().count() + 1, TagKind::Vitest),
            site(
                "frontend/src/x.test.ts",
                typescript.lines().count() + 3,
                TagKind::Vitest,
            ),
        ];
        assert_eq!(sites, expected);
        let kinds = sites.iter().map(|site| &site.kind);
        assert!(required_tags(&layer_rule(&id, Layer::Both)).satisfied_by(kinds));
        let screen_kinds = sites
            .iter()
            .filter(|found| found.path == Path::new(screen))
            .map(|found| &found.kind);
        assert!(required_tags(&layer_rule(&id, Layer::Client)).satisfied_by(screen_kinds));
    }
}
