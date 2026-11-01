//! Markdown audit-table parser and ID-coverage helpers for the parity audits under
//! `docs/parity/`.
//!
//! The derive-lane audits (`parity-oracle-audit.md`, `validation-audit.md`,
//! `function-map.md` and `escalations.md`) are GitHub pipe tables under `##` and `###`
//! headings. This crate is their one parser: [`Document::parse`] reads a file into
//! [`Table`]s that keep their heading context and 1-based source lines; the cell helpers
//! [`citations`], [`code_span`], [`key_values`] and [`no_message_escalation`] read the cell
//! conventions the audits define; [`ids_with_prefix`], [`scan_cited_ids`] and [`coverage`]
//! prove that every audit row ID is cited by a test and that no test cites an ID the audit
//! does not define. [`Repo`] resolves the repository paths that citations name, ignoring
//! ASCII case where the stored case differs from the cited one (E-03).
//!
//! The crate uses `std` only and declares no dependency of any kind, because the pre-code
//! structure check (`cargo test -p carddemo-audit --test audit_structure`) must build this
//! package alone, before any other workspace crate exists.
//!
//! Consumers:
//!
//! - `crates/audit/tests/audit_structure.rs`, the structure check of every audit section;
//! - `crates/api/tests/validation_audit_coverage.rs` and the API error tests, over the
//!   V-IDs;
//! - `crates/batch/tests/parity_audit_coverage.rs` and the batch and messaging tests that
//!   read fixture expectations, over the P-IDs.
//!
//! The unit tests and docs of this file hold sample IDs, so `crates/audit` must never be
//! passed to [`scan_cited_ids`]: its samples would count as citations.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs, io,
    path::{Path, PathBuf},
};

/// Repository-relative path of the parity oracle audit (`P-` rows, sections A to F).
pub const PARITY_AUDIT_PATH: &str = "docs/parity/parity-oracle-audit.md";

/// Repository-relative path of the input-validation audit (`V-` rows).
pub const VALIDATION_AUDIT_PATH: &str = "docs/parity/validation-audit.md";

/// Repository-relative path of the BMS function map (`F-` rows).
pub const FUNCTION_MAP_PATH: &str = "docs/parity/function-map.md";

/// Repository-relative path of the escalation log (`E-` rows).
pub const ESCALATIONS_PATH: &str = "docs/parity/escalations.md";

/// Columns every parity audit data table starts with, in order.
pub const PARITY_COMMON_COLUMNS: [&str; 6] = [
    "ID",
    "Program",
    "Paragraph",
    "Copybook field or PIC clause",
    "Input condition",
    "Expected result",
];

/// Columns a section A (record layouts) table appends after [`PARITY_COMMON_COLUMNS`].
pub const PARITY_LAYOUT_COLUMNS: [&str; 3] = ["Offset", "Length", "Usage"];

/// Column a section B (per-program rules) table appends after [`PARITY_COMMON_COLUMNS`].
pub const PARITY_RULE_COLUMNS: [&str; 1] = ["Persists on later failure"];

/// Columns of every validation audit data table, in order.
pub const VALIDATION_COLUMNS: [&str; 8] = [
    "ID",
    "Program",
    "Validation paragraph",
    "Trigger condition",
    "Message text",
    "Status",
    "BMS field",
    "Order",
];

/// Columns of every function map data table, in order. `web/e2e/navigation.spec.ts`
/// mirrors this header in TypeScript, so the two change together.
pub const FUNCTION_MAP_COLUMNS: [&str; 6] = [
    "ID",
    "BMS map",
    "Key or field",
    "COBOL action",
    "UI control",
    "Endpoint",
];

/// Columns of the escalation log and of every `## Escalations raised` table, in order.
pub const ESCALATION_COLUMNS: [&str; 7] = [
    "ID",
    "Lane change",
    "Trigger",
    "Evidence",
    "Decision",
    "Deciding request clause",
    "Status",
];

/// Columns of every `### Review log — <section>` table, in order.
pub const REVIEW_LOG_COLUMNS: [&str; 4] = ["Row ID", "Verdict", "Note", "Reviewer"];

/// The six `##` section headings of the parity audit, in document order.
pub const PARITY_SECTIONS: [&str; 6] = [
    "A. Record layouts",
    "B. Per-program rules",
    "C. Arithmetic",
    "D. Fixture expectations",
    "E. Anomalies",
    "F. Time-dependent fields",
];

/// The `##` heading under which each audit hands the escalations it raises to the log at
/// [`ESCALATIONS_PATH`].
pub const ESCALATIONS_RAISED: &str = "Escalations raised";

/// The words an escalation `Status` cell may start with, optionally followed by `: <note>`.
pub const ESCALATION_STATUSES: [&str; 3] = ["resolved", "open", "halt"];

/// A parsed audit document: every pipe table it holds, in document order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    tables: Vec<Table>,
}

impl Document {
    /// Parses `text` into its tables.
    ///
    /// Parsing is infallible and best effort: a table whose rows hold a cell count different
    /// from its header, or whose header is unusual, is kept exactly as written, so the
    /// structure check can report it. The rules:
    ///
    /// - Lines are those of [`str::lines`], numbered from 1, so CRLF text parses as LF text.
    /// - A line opening with 3 or more backticks or tildes starts a fenced block. It runs to
    ///   a line holding only a run of the same character at least as long, or to the end of
    ///   the text, and nothing inside it is read.
    /// - `## <text>` sets the section and clears the subsection and the keys; `### <text>`
    ///   sets the subsection and clears the keys; other heading levels change nothing.
    /// - A line starting with `Keys:` sets the keys until the next `##` or `###` heading.
    /// - A line starting with `|` directly followed by a delimiter row such as `|---|:--:|`
    ///   starts a table. The table takes the current section, subsection and keys, and its
    ///   rows continue while lines start with `|`.
    /// - Cells split at each `|` outside code spans, and `\|` stands for a literal `|`,
    ///   inside code spans too. Cells keep their backticks and are trimmed of spaces and
    ///   tabs.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let lines: Vec<&str> = text.lines().collect();
        let mut state = ParseState::default();
        let mut index = 0;
        while index < lines.len() {
            index += state.step(&lines, index);
        }
        Self {
            tables: state.finish(),
        }
    }

    /// Reads the UTF-8 file at `path` and parses it with [`Document::parse`].
    ///
    /// # Errors
    ///
    /// The read error, with its message prefixed by the path, so a failure names the file.
    pub fn load(path: &Path) -> io::Result<Self> {
        let text = fs::read_to_string(path).map_err(|error| with_path(path, &error))?;
        Ok(Self::parse(&text))
    }

    /// Every table of the document, in document order.
    #[must_use]
    pub fn tables(&self) -> &[Table] {
        &self.tables
    }

    /// Every table whose `##` section starts with `section_prefix`, for example `"D."`.
    ///
    /// Data tables and review logs are yielded alike; callers keep the data tables with
    /// [`Table::is_data_table`].
    pub fn section_tables(&self, section_prefix: &str) -> impl Iterator<Item = &Table> {
        self.tables.iter().filter(move |table| {
            table
                .section
                .as_deref()
                .is_some_and(|section| section.starts_with(section_prefix))
        })
    }

    /// The first data table row whose `ID` cell equals `id`, trimmed, with its table.
    /// Review logs, whose first column is `Row ID`, are never searched.
    #[must_use]
    pub fn find_row(&self, id: &str) -> Option<(&Table, &Row)> {
        let id = id.trim();
        self.tables
            .iter()
            .filter(|table| table.is_data_table())
            .find_map(|table| {
                table
                    .rows
                    .iter()
                    .find(|row| table.cell(row, "ID") == Some(id))
                    .map(|row| (table, row))
            })
    }
}

/// One pipe table and the heading context it appears in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    /// The nearest preceding `##` heading.
    pub section: Option<String>,
    /// The nearest `###` heading after that section heading.
    pub subsection: Option<String>,
    /// The raw text after the `Keys:` line in force for this table.
    pub keys: Option<String>,
    /// The 1-based line of the header row.
    pub line: usize,
    /// The header cells, trimmed.
    pub header: Vec<String>,
    /// The body rows, without the delimiter row.
    pub rows: Vec<Row>,
}

impl Table {
    /// The position of the header cell equal to `name`, trimmed.
    #[must_use]
    pub fn column(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        self.header.iter().position(|cell| cell == name)
    }

    /// The cell of `row` under the header `name`, or `None` when the table has no such
    /// column or the row is too short to reach it.
    #[must_use]
    pub fn cell<'a>(&'a self, row: &'a Row, name: &str) -> Option<&'a str> {
        let index = self.column(name)?;
        row.cells.get(index).map(String::as_str)
    }

    /// Whether the first header cell is `ID`: an audit data table, an escalation log or an
    /// `## Escalations raised` table.
    #[must_use]
    pub fn is_data_table(&self) -> bool {
        self.header.first().is_some_and(|cell| cell == "ID")
    }

    /// Whether the first header cell is `Row ID`: a review log.
    #[must_use]
    pub fn is_review_log(&self) -> bool {
        self.header.first().is_some_and(|cell| cell == "Row ID")
    }

    /// Whether the table sits under the `## Escalations raised` heading.
    #[must_use]
    pub fn is_escalations_raised(&self) -> bool {
        self.section.as_deref() == Some(ESCALATIONS_RAISED)
    }

    /// The key names of the `Keys:` line: its text split at `,` and `;`, each part trimmed
    /// of whitespace and backticks, empty parts dropped. Empty when no `Keys:` line applies.
    #[must_use]
    pub fn key_names(&self) -> Vec<String> {
        let Some(keys) = self.keys.as_deref() else {
            return Vec::new();
        };
        keys.split([',', ';'])
            .map(|part| part.trim_matches(|ch: char| ch.is_whitespace() || ch == '`'))
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

/// One body row of a [`Table`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row {
    /// The cells, trimmed of spaces and tabs, with their backticks kept.
    pub cells: Vec<String>,
    /// The 1-based source line.
    pub line: usize,
}

/// Line-by-line state of [`Document::parse`].
#[derive(Debug, Default)]
struct ParseState {
    section: Option<String>,
    subsection: Option<String>,
    keys: Option<String>,
    /// The character and run length that opened the current fenced block.
    fence: Option<(char, usize)>,
    open: Option<Table>,
    tables: Vec<Table>,
}

impl ParseState {
    /// Processes `lines[index]` and returns how many lines it consumed: 2 when that line and
    /// the delimiter row after it start a table, else 1.
    fn step(&mut self, lines: &[&str], index: usize) -> usize {
        let line = lines[index];
        if let Some(fence) = self.fence {
            if closes_fence(line, fence) {
                self.fence = None;
            }
            return 1;
        }
        if let Some(table) = self.open.as_mut() {
            if line.trim_start().starts_with('|') {
                table.rows.push(Row {
                    cells: split_cells(line),
                    line: index + 1,
                });
                return 1;
            }
            self.close_table();
        }
        if let Some(fence) = opens_fence(line) {
            self.fence = Some(fence);
        } else if let Some((level, text)) = parse_heading(line) {
            self.enter_heading(level, text);
        } else if let Some(keys) = line.trim().strip_prefix("Keys:") {
            self.keys = Some(keys.trim().to_owned());
        } else if line.trim_start().starts_with('|')
            && lines.get(index + 1).copied().is_some_and(is_delimiter_row)
        {
            self.open = Some(Table {
                section: self.section.clone(),
                subsection: self.subsection.clone(),
                keys: self.keys.clone(),
                line: index + 1,
                header: split_cells(line),
                rows: Vec::new(),
            });
            return 2;
        }
        1
    }

    fn enter_heading(&mut self, level: usize, text: &str) {
        match level {
            2 => {
                self.section = Some(text.to_owned());
                self.subsection = None;
                self.keys = None;
            }
            3 => {
                self.subsection = Some(text.to_owned());
                self.keys = None;
            }
            _ => {}
        }
    }

    fn close_table(&mut self) {
        if let Some(table) = self.open.take() {
            self.tables.push(table);
        }
    }

    fn finish(mut self) -> Vec<Table> {
        self.close_table();
        self.tables
    }
}

/// The fence character and run length when `line` opens a fenced block.
fn opens_fence(line: &str) -> Option<(char, usize)> {
    let fence = line.trim_start().chars().next()?;
    if fence != '`' && fence != '~' {
        return None;
    }
    let run = line
        .trim_start()
        .chars()
        .take_while(|&ch| ch == fence)
        .count();
    (run >= 3).then_some((fence, run))
}

/// Whether `line` closes the fenced block opened by `fence`: nothing but a run of the same
/// character, at least as long as the opening run.
fn closes_fence(line: &str, fence: (char, usize)) -> bool {
    let (ch, run) = fence;
    let text = line.trim();
    !text.is_empty() && text.chars().all(|other| other == ch) && text.chars().count() >= run
}

/// The level and text of an ATX heading: a run of `#` followed by a space, a tab or the end
/// of the line. The text is trimmed and loses its closing `#` run.
fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let text = line.trim_start();
    let level = text.chars().take_while(|&ch| ch == '#').count();
    if level == 0 {
        return None;
    }
    // `#` is one byte, so `level` is also the byte offset of the rest.
    let rest = &text[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim()))
}

/// Whether `line` is a table delimiter row: it starts with `|` and every cell is a run of
/// `-` with an optional `:` at either end.
fn is_delimiter_row(line: &str) -> bool {
    if !line.trim_start().starts_with('|') {
        return false;
    }
    let cells = split_cells(line);
    !cells.is_empty() && cells.iter().map(String::as_str).all(is_delimiter_cell)
}

fn is_delimiter_cell(cell: &str) -> bool {
    let cell = cell.trim();
    let cell = cell.strip_prefix(':').unwrap_or(cell);
    let cell = cell.strip_suffix(':').unwrap_or(cell);
    !cell.is_empty() && cell.chars().all(|ch| ch == '-')
}

/// Splits a table line into trimmed cells.
///
/// One leading `|` is dropped, and so is the empty cell after a trailing `|`. An unescaped
/// `|` outside every code span ends a cell; `\|` emits a literal `|` everywhere, inside code
/// spans too, as GitHub tables do. Every other character, backslashes and backticks
/// included, is kept.
fn split_cells(line: &str) -> Vec<String> {
    let line = line.trim_matches([' ', '\t']);
    let body = line.strip_prefix('|').unwrap_or(line);
    let chars: Vec<char> = body.chars().collect();
    let spans = code_spans(&chars);
    let mut cells = Vec::new();
    let mut current = String::new();
    let mut ended_on_split = false;
    let mut at = 0;
    while at < chars.len() {
        ended_on_split = false;
        if chars[at] == '\\' && chars.get(at + 1) == Some(&'|') {
            current.push('|');
            at += 2;
            continue;
        }
        if chars[at] == '|' && !inside_span(&spans, at) {
            cells.push(std::mem::take(&mut current));
            ended_on_split = true;
        } else {
            current.push(chars[at]);
        }
        at += 1;
    }
    if !ended_on_split {
        cells.push(current);
    }
    cells
        .iter()
        .map(|cell| cell.trim_matches([' ', '\t']).to_owned())
        .collect()
}

/// One code span over a char sequence: `start..end` covers the delimiters too, and
/// `content..closer` the characters between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodeSpanRange {
    start: usize,
    content: usize,
    closer: usize,
    end: usize,
}

impl CodeSpanRange {
    /// The length of the backtick run that delimits the span.
    const fn ticks(self) -> usize {
        self.content - self.start
    }
}

/// The code spans of `chars`, in order.
///
/// A backtick run of length k opens a span closed by the next run of exactly k backticks;
/// runs of other lengths before it are content. A run with no closer is literal text, and
/// scanning resumes after it, so a later run may still open a span.
fn code_spans(chars: &[char]) -> Vec<CodeSpanRange> {
    let mut spans = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if chars[at] != '`' {
            at += 1;
            continue;
        }
        let run = backtick_run(chars, at);
        if let Some(closer) = find_closer(chars, at + run, run) {
            spans.push(CodeSpanRange {
                start: at,
                content: at + run,
                closer,
                end: closer + run,
            });
            at = closer + run;
        } else {
            at += run;
        }
    }
    spans
}

/// The length of the backtick run starting at `from`.
fn backtick_run(chars: &[char], from: usize) -> usize {
    chars[from..].iter().take_while(|&&ch| ch == '`').count()
}

/// The start of the first maximal run of exactly `run` backticks at or after `from`.
fn find_closer(chars: &[char], from: usize, run: usize) -> Option<usize> {
    let mut at = from;
    while at < chars.len() {
        if chars[at] == '`' {
            let len = backtick_run(chars, at);
            if len == run {
                return Some(at);
            }
            at += len;
        } else {
            at += 1;
        }
    }
    None
}

/// Whether the char at `at` lies inside one of `spans`, delimiters included.
fn inside_span(spans: &[CodeSpanRange], at: usize) -> bool {
    spans.iter().any(|span| span.start <= at && at < span.end)
}

/// The ranges of `chars` outside every code span, in order; empty ranges included.
fn gaps(chars: &[char]) -> Vec<(usize, usize)> {
    let mut gaps = Vec::new();
    let mut from = 0;
    for span in code_spans(chars) {
        gaps.push((from, span.start));
        from = span.end;
    }
    gaps.push((from, chars.len()));
    gaps
}

/// A `path:line` citation: physical lines `start` to `end` of the file at `path`, both
/// 1-based and inclusive. A single-line citation has `start == end`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Citation {
    /// The path as written, which [`Repo::resolve`] turns into a file.
    pub path: String,
    /// The first cited line.
    pub start: usize,
    /// The last cited line.
    pub end: usize,
}

/// The line citations of a cell, in order.
///
/// A citation is a bracket `[<path>:<item>, <item>, …]` outside every code span, where the
/// path holds no whitespace, `[`, `]` or `:` and each item is `L<n>` or `L<n>-<m>`. Each
/// item becomes its own [`Citation`] with the bracket's path. A bracket with any other item
/// cites nothing, nor does a bracket without a `:`:
///
/// ```text
/// [app/cbl/CBACT04C.cbl:L167]              -> (167, 167)
/// [app/cbl/COPAUA0C.cbl:L46, L730-731]     -> (46, 46), (730, 731)
/// [app/jcl/DUSRSECJ.jcl:STEP01]            -> nothing
/// [app/cbl/CBACT04C.cbl:L167, 1300-PARA]   -> nothing
/// [E-09]                                   -> nothing
/// [see [app/x.cbl:L1]]                     -> (1, 1), from the inner bracket
/// ```
///
/// Ranges are returned as written, reversed ranges and line 0 included, so the structure
/// check can report them; so are bare file names, which the caller resolves.
#[must_use]
pub fn citations(cell: &str) -> Vec<Citation> {
    let chars: Vec<char> = cell.chars().collect();
    let mut found = Vec::new();
    for (from, to) in gaps(&chars) {
        bracket_citations(&chars[from..to], &mut found);
    }
    found
}

/// Appends the citations of every bracket in `gap`, a stretch of text outside code spans.
fn bracket_citations(gap: &[char], found: &mut Vec<Citation>) {
    let mut at = 0;
    while at < gap.len() {
        if gap[at] != '[' {
            at += 1;
            continue;
        }
        let Some(offset) = gap[at + 1..].iter().position(|&ch| ch == ']') else {
            return;
        };
        let close = at + 1 + offset;
        let inner: String = gap[at + 1..close].iter().collect();
        if let Some(cited) = parse_citation(&inner) {
            found.extend(cited);
            at = close + 1;
        } else {
            at += 1;
        }
    }
}

/// The citations of one bracket's content, or `None` when it is not a line citation.
fn parse_citation(inner: &str) -> Option<Vec<Citation>> {
    let (path, items) = inner.split_once(':')?;
    if path.is_empty()
        || path
            .chars()
            .any(|ch| ch.is_whitespace() || ch == '[' || ch == ']')
    {
        return None;
    }
    items
        .split(',')
        .map(|item| {
            let (start, end) = parse_line_item(item.trim_matches([' ', '\t']))?;
            Some(Citation {
                path: path.to_owned(),
                start,
                end,
            })
        })
        .collect()
}

/// `L<n>` as `(n, n)` or `L<n>-<m>` as `(n, m)`.
fn parse_line_item(item: &str) -> Option<(usize, usize)> {
    let range = item.strip_prefix('L')?;
    if let Some((start, end)) = range.split_once('-') {
        Some((parse_digits(start)?, parse_digits(end)?))
    } else {
        let line = parse_digits(range)?;
        Some((line, line))
    }
}

/// One or more ASCII digits that fit `usize`.
fn parse_digits(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// The exact text of a code span, with the width the audit pads it to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeSpan {
    /// The characters between the delimiters (see [`code_span`]).
    pub text: String,
    /// The `width=<n>` given after the span, if any.
    pub width: Option<usize>,
}

/// The first code span of a cell, or `None` when the cell holds none.
///
/// This is the contract audit authors and `crates/domain/src/messages.rs` rely on for exact
/// texts:
///
/// - `text` is exactly the characters between the delimiters. A span delimited by a single
///   backtick is never stripped, so significant leading and trailing spaces survive.
/// - When the delimiter has 2 or more backticks and the content begins and ends with a
///   space and is not all spaces, one space is removed from each side, as in `CommonMark`.
///   Authors use this form for a text that holds a backtick.
/// - `width` is the first `width=<n>` (one or more ASCII digits) anywhere after the closing
///   delimiter, whatever wraps it, provided the character before `width` is not an ASCII
///   letter, digit or `_`. It is `None` when there is none or its digits overflow `usize`.
///
/// ```text
/// `"Payment ok.  "` width=80   -> text "Payment ok.  " (quotes included), width 80
/// `` `x` ``                    -> text `x`
/// ```
#[must_use]
pub fn code_span(cell: &str) -> Option<CodeSpan> {
    let chars: Vec<char> = cell.chars().collect();
    let span = code_spans(&chars).into_iter().next()?;
    let content = &chars[span.content..span.closer];
    let padded = content.first() == Some(&' ')
        && content.last() == Some(&' ')
        && content.iter().any(|&ch| ch != ' ');
    let text = if span.ticks() >= 2 && padded {
        &content[1..content.len() - 1]
    } else {
        content
    };
    Some(CodeSpan {
        text: text.iter().collect(),
        width: width_after(&chars[span.end..]),
    })
}

/// The value of the first `width=<digits>` in `rest` not preceded by a word character.
fn width_after(rest: &[char]) -> Option<usize> {
    const NEEDLE: [char; 6] = ['w', 'i', 'd', 't', 'h', '='];
    let mut at = 0;
    while at + NEEDLE.len() <= rest.len() {
        let bounded = at == 0 || !is_word_char(rest[at - 1]);
        if bounded && rest[at..at + NEEDLE.len()] == NEEDLE {
            let digits: String = rest[at + NEEDLE.len()..]
                .iter()
                .take_while(|ch| ch.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return digits.parse().ok();
            }
        }
        at += 1;
    }
    None
}

fn is_word_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// One `key=value` pair of a structured result cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    /// The text before the first `=`, trimmed.
    pub key: String,
    /// The text after it, trimmed, with its backticks kept for [`code_span`].
    pub value: String,
}

/// The `key=value` pairs of a structured result cell (`key=value; key=value`), in order,
/// duplicates kept.
///
/// The cell splits at each `;` outside code spans, and each part at its first `=` outside
/// code spans; a part without such an `=` is skipped.
///
/// ```text
/// route=/cards; control=button "Next"   -> route=/cards, control=button "Next"
/// message=`a; b`; status=409            -> message=`a; b`, status=409
/// ```
#[must_use]
pub fn key_values(cell: &str) -> Vec<KeyValue> {
    let chars: Vec<char> = cell.chars().collect();
    let spans = code_spans(&chars);
    let mut pairs = Vec::new();
    let mut part_start = 0;
    for (at, &ch) in chars.iter().enumerate() {
        if ch == ';' && !inside_span(&spans, at) {
            pairs.extend(key_value(&chars, &spans, part_start, at));
            part_start = at + 1;
        }
    }
    pairs.extend(key_value(&chars, &spans, part_start, chars.len()));
    pairs
}

/// The pair held by `chars[from..to]`, split at its first `=` outside code spans.
fn key_value(chars: &[char], spans: &[CodeSpanRange], from: usize, to: usize) -> Option<KeyValue> {
    let equals = (from..to).find(|&at| chars[at] == '=' && !inside_span(spans, at))?;
    let key: String = chars[from..equals].iter().collect();
    let value: String = chars[equals + 1..to].iter().collect();
    Some(KeyValue {
        key: key.trim().to_owned(),
        value: value.trim().to_owned(),
    })
}

/// Whether a message cell records a path that sets no message (`no-message (E-nn)`).
#[must_use]
pub fn is_no_message(cell: &str) -> bool {
    cell.trim().starts_with("no-message")
}

/// The escalation ID of a `no-message (E-nn)` cell.
///
/// The cell, trimmed, is `no-message`, optional whitespace, then the parenthesised ID, with
/// nothing after the `)`. `no-message(E-05)` and `no-message ( E-05 )` both give `E-05`;
/// `no-message`, `no-message (E-nn)`, `no-message (E-5)` and `no-message (E-05) x` give
/// `None`.
#[must_use]
pub fn no_message_escalation(cell: &str) -> Option<String> {
    let rest = cell.trim().strip_prefix("no-message")?;
    let inner = rest
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?
        .trim();
    (classify_id(inner) == Some(IdKind::Escalation)).then(|| inner.to_owned())
}

/// The four kinds of audit row ID.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IdKind {
    /// `P-<KEY>-<nnn>`, a row of [`PARITY_AUDIT_PATH`].
    Parity,
    /// `V-<PROGRAM>-<nnn>`, a row of [`VALIDATION_AUDIT_PATH`].
    Validation,
    /// `F-<MAP>-<nnn>`, a row of [`FUNCTION_MAP_PATH`].
    Function,
    /// `E-<nn>`, a row of [`ESCALATIONS_PATH`].
    Escalation,
}

impl IdKind {
    /// The letter the IDs of this kind start with.
    #[must_use]
    pub const fn prefix(self) -> char {
        match self {
            Self::Parity => 'P',
            Self::Validation => 'V',
            Self::Function => 'F',
            Self::Escalation => 'E',
        }
    }
}

/// The kind of `token` when it is exactly a well-formed ID, untrimmed.
///
/// - Escalation: `E-` followed by 2 or more ASCII digits (`E-05`, `E-123`).
/// - Parity, validation and function map: the prefix letter, then one or more `-<SEG>`
///   segments of `[A-Z0-9]+`, then `-` and 3 or more ASCII digits (`P-CBTRN02C-004`,
///   `V-COACTUPC-012`, `F-COSGN00-001`).
///
/// Lower case, `_`, empty segments, `P-001` (no segment) and `P-CBTRN02C-04` (two digits)
/// are not IDs.
#[must_use]
pub fn classify_id(token: &str) -> Option<IdKind> {
    if let Some(digits) = token.strip_prefix("E-") {
        let numeric = digits.len() >= 2 && digits.bytes().all(|byte| byte.is_ascii_digit());
        return numeric.then_some(IdKind::Escalation);
    }
    let kind = match token.as_bytes().first()? {
        b'P' => IdKind::Parity,
        b'V' => IdKind::Validation,
        b'F' => IdKind::Function,
        _ => return None,
    };
    // The prefix letter is one byte, so slicing after it stays on a char boundary.
    let body = token[1..].strip_prefix('-')?;
    let (segments, number) = body.rsplit_once('-')?;
    let numbered = number.len() >= 3 && number.bytes().all(|byte| byte.is_ascii_digit());
    let segmented = segments.split('-').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    });
    (numbered && segmented).then_some(kind)
}

/// Every ID written in `text`, in order, with its kind.
///
/// A candidate starts at an ASCII letter or digit whose predecessor is absent or is not an
/// ASCII letter, digit, `-` or `_`. It runs over the longest stretch of ASCII letters,
/// digits and `-`, loses one trailing `-`, and counts when [`classify_id`] accepts it.
/// `CVE-2026-25727`, `RUSTSEC-2026-0187`, `E-nn`, `xP-ABC-001` and `P-ABC-001abc` hold no
/// ID; `E-01…E-22` holds two; `CBTRN02C 1500-B-LOOKUP-ACCT — P-CBTRN02C-004` holds one.
#[must_use]
pub fn id_tokens(text: &str) -> Vec<(IdKind, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let starts =
            chars[at].is_ascii_alphanumeric() && (at == 0 || !is_id_neighbour(chars[at - 1]));
        if !starts {
            at += 1;
            continue;
        }
        let run = chars[at..]
            .iter()
            .take_while(|ch| ch.is_ascii_alphanumeric() || **ch == '-')
            .count();
        let mut token: String = chars[at..at + run].iter().collect();
        if token.ends_with('-') {
            token.pop();
        }
        if let Some(kind) = classify_id(&token) {
            found.push((kind, token));
        }
        at += run;
    }
    found
}

fn is_id_neighbour(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'
}

/// The IDs of `kind` that `doc` defines: the trimmed first cell of every row of every data
/// table, kept when it classifies as `kind`. Review logs (first column `Row ID`) define
/// nothing; the E-IDs of `## Escalations raised` tables are kept only for
/// [`IdKind::Escalation`].
#[must_use]
pub fn ids_with_prefix(doc: &Document, kind: IdKind) -> BTreeSet<String> {
    doc.tables
        .iter()
        .filter(|table| table.is_data_table())
        .flat_map(|table| &table.rows)
        .filter_map(|row| {
            let id = row.cells.first()?.trim();
            (classify_id(id) == Some(kind)).then(|| id.to_owned())
        })
        .collect()
}

/// A line of a source file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceLocation {
    /// The file, as reached from the scanned entry.
    pub path: PathBuf,
    /// The 1-based line.
    pub line: usize,
}

impl fmt::Display for SourceLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.path.display(), self.line)
    }
}

/// Every ID of `kind` cited in the Rust sources under `dirs`, with each line citing it.
///
/// Each entry is a directory, scanned recursively, or a file, read whatever its extension.
/// Directory entries are visited in name order and symbolic links are not followed;
/// subdirectories named `target` or starting with `.` are skipped, and only files with
/// extension `rs` are read. Files are decoded lossily, so a stray non-UTF-8 byte does not
/// hide the IDs around it. An ID is recorded once per line that holds it, through
/// [`id_tokens`].
///
/// IDs count only when written literally: a name such as `fn p_cbtrn02c_004` cites nothing.
/// `crates/audit` must not be scanned, because its own tests and docs hold sample IDs.
///
/// # Errors
///
/// A missing entry or an unreadable directory or file, with the path in the message.
pub fn scan_cited_ids(
    dirs: &[&Path],
    kind: IdKind,
) -> io::Result<BTreeMap<String, Vec<SourceLocation>>> {
    let mut files = Vec::new();
    for &entry in dirs {
        let metadata = fs::metadata(entry).map_err(|error| with_path(entry, &error))?;
        if metadata.is_dir() {
            collect_rust_files(entry, &mut files)?;
        } else {
            // The `.rs` filter applies to directory walks only; a file named here was chosen.
            files.push(entry.to_path_buf());
        }
    }
    let mut cited: BTreeMap<String, Vec<SourceLocation>> = BTreeMap::new();
    for file in files {
        let bytes = fs::read(&file).map_err(|error| with_path(&file, &error))?;
        for (index, line) in String::from_utf8_lossy(&bytes).lines().enumerate() {
            let mut on_line = BTreeSet::new();
            for (found, id) in id_tokens(line) {
                if found == kind && on_line.insert(id.clone()) {
                    cited.entry(id).or_default().push(SourceLocation {
                        path: file.clone(),
                        line: index + 1,
                    });
                }
            }
        }
    }
    Ok(cited)
}

/// Appends the `.rs` files under `dir` to `files`, in name order, depth first.
fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut entries = fs::read_dir(dir)
        .and_then(Iterator::collect::<io::Result<Vec<fs::DirEntry>>>)
        .map_err(|error| with_path(dir, &error))?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| with_path(&path, &error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_dir() {
            if name != "target" && !name.starts_with('.') {
                collect_rust_files(&path, files)?;
            }
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// The error `error` with its message prefixed by `path`.
fn with_path(path: &Path, error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

/// The difference between the IDs an audit defines and the IDs the tests cite.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Defined IDs that no scanned line cites.
    pub uncited: BTreeSet<String>,
    /// Cited IDs the audit does not define, with every line citing them.
    pub unknown: BTreeMap<String, Vec<SourceLocation>>,
}

/// Compares the `defined` IDs of an audit with the `cited` IDs of [`scan_cited_ids`].
#[must_use]
pub fn coverage(
    defined: &BTreeSet<String>,
    cited: &BTreeMap<String, Vec<SourceLocation>>,
) -> Coverage {
    Coverage {
        uncited: defined
            .iter()
            .filter(|id| !cited.contains_key(*id))
            .cloned()
            .collect(),
        unknown: cited
            .iter()
            .filter(|(id, _)| !defined.contains(*id))
            .map(|(id, locations)| (id.clone(), locations.clone()))
            .collect(),
    }
}

impl Coverage {
    /// Whether every defined ID is cited and every cited ID is defined.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.uncited.is_empty() && self.unknown.is_empty()
    }

    /// Fails the calling test unless the coverage is complete.
    ///
    /// # Panics
    ///
    /// When [`Coverage::is_complete`] is false, with a message naming `label`, every
    /// uncited ID and every unknown ID with its locations.
    pub fn assert_complete(&self, label: &str) {
        assert!(
            self.is_complete(),
            "{label}: audit coverage incomplete\n{self}"
        );
    }
}

impl fmt::Display for Coverage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "uncited IDs ({}):", self.uncited.len())?;
        for id in &self.uncited {
            writeln!(f, "  {id}")?;
        }
        writeln!(f, "unknown IDs ({}):", self.unknown.len())?;
        for (id, locations) in &self.unknown {
            let at = locations
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(f, "  {id} at {at}")?;
        }
        Ok(())
    }
}

/// The repository checkout that audit citations and document paths are relative to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    /// The repository root.
    pub root: PathBuf,
}

impl Repo {
    /// The repository rooted at `root`.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The repository holding the crate whose manifest directory is `dir`, which callers
    /// pass as `env!("CARGO_MANIFEST_DIR")`: every workspace crate sits at `crates/<name>`,
    /// so the root is that directory's grandparent.
    #[must_use]
    pub fn from_manifest_dir(dir: &str) -> Self {
        let manifest = Path::new(dir);
        let root = manifest
            .ancestors()
            .nth(2)
            .map_or_else(|| manifest.join("../.."), Path::to_path_buf);
        Self { root }
    }

    /// The file or directory a repository-relative `rel` path names, spelled as stored.
    ///
    /// Each `/`-separated segment matches its stored name exactly or, failing that, the one
    /// entry of its directory equal to it ignoring ASCII case (E-03), so
    /// `app/cbl/CBSTM03A.cbl` resolves to the stored `app/cbl/CBSTM03A.CBL`.
    ///
    /// `None` when `rel` is empty, absolute, holds a `\` or an empty, `.` or `..` segment (a
    /// trailing `/` included), or when a segment matches no entry, several entries, or an
    /// unreadable directory.
    #[must_use]
    pub fn resolve(&self, rel: &str) -> Option<PathBuf> {
        if rel.is_empty() || rel.starts_with('/') || rel.contains('\\') {
            return None;
        }
        if Path::new(rel).is_absolute()
            || rel
                .split('/')
                .any(|segment| segment.is_empty() || segment == "." || segment == "..")
        {
            return None;
        }
        let mut path = self.root.clone();
        for segment in rel.split('/') {
            path = resolve_segment(&path, segment)?;
        }
        Some(path)
    }

    /// The physical line count of the file at `rel` (see [`count_lines`]), or `None` when
    /// [`Repo::resolve`] finds no such path. The file is read as bytes, because the EBCDIC
    /// fixtures are not UTF-8.
    ///
    /// # Errors
    ///
    /// The read error of a resolved path that cannot be read, a directory included.
    pub fn line_count(&self, rel: &str) -> io::Result<Option<usize>> {
        let Some(path) = self.resolve(rel) else {
            return Ok(None);
        };
        let bytes = fs::read(&path).map_err(|error| with_path(&path, &error))?;
        Ok(Some(count_lines(&bytes)))
    }

    /// Loads the audit document at the repository-relative path `rel`.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::NotFound`] when `rel` does not resolve, else the error of
    /// [`Document::load`].
    pub fn load(&self, rel: &str) -> io::Result<Document> {
        let path = self.resolve(rel).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("{rel}: not found under {}", self.root.display()),
            )
        })?;
        Document::load(&path)
    }

    /// Loads [`PARITY_AUDIT_PATH`].
    ///
    /// # Errors
    ///
    /// As [`Repo::load`].
    pub fn parity_audit(&self) -> io::Result<Document> {
        self.load(PARITY_AUDIT_PATH)
    }

    /// Loads [`VALIDATION_AUDIT_PATH`].
    ///
    /// # Errors
    ///
    /// As [`Repo::load`].
    pub fn validation_audit(&self) -> io::Result<Document> {
        self.load(VALIDATION_AUDIT_PATH)
    }

    /// Loads [`FUNCTION_MAP_PATH`].
    ///
    /// # Errors
    ///
    /// As [`Repo::load`].
    pub fn function_map(&self) -> io::Result<Document> {
        self.load(FUNCTION_MAP_PATH)
    }

    /// Loads [`ESCALATIONS_PATH`].
    ///
    /// # Errors
    ///
    /// As [`Repo::load`].
    pub fn escalations(&self) -> io::Result<Document> {
        self.load(ESCALATIONS_PATH)
    }
}

/// The entry of `dir` named `segment`: the exact name when it exists, else the one entry
/// equal to it ignoring ASCII case.
fn resolve_segment(dir: &Path, segment: &str) -> Option<PathBuf> {
    let exact = dir.join(segment);
    if exact.exists() {
        return Some(exact);
    }
    let entries = fs::read_dir(dir)
        .and_then(Iterator::collect::<io::Result<Vec<fs::DirEntry>>>)
        .ok()?;
    let mut matches = entries.into_iter().filter(|entry| {
        entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(segment))
    });
    let found = matches.next()?;
    matches.next().is_none().then(|| found.path())
}

/// The number of physical lines in `bytes`: one per LF, plus one for a final line without
/// a trailing LF. CR bytes are content, so CRLF text counts as LF text.
///
/// ```text
/// ""              -> 0
/// "a\n"           -> 1
/// "a\r\nb\r\n"    -> 2
/// "a\nb"          -> 2
/// ```
#[must_use]
pub fn count_lines(bytes: &[u8]) -> usize {
    let newlines: usize = bytes.iter().map(|&byte| usize::from(byte == b'\n')).sum();
    newlines + usize::from(bytes.last().is_some_and(|&last| last != b'\n'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A fresh directory under the system temp dir, removed when dropped.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_nanos());
            let name = format!("carddemo-audit-{label}-{}-{nanos}", std::process::id());
            let path = std::env::temp_dir().join(name);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn write(&self, rel: &str, contents: &[u8]) -> PathBuf {
            let path = self.path.join(rel);
            let parent = path.parent().expect("temp file parent");
            fs::create_dir_all(parent).expect("create temp parent");
            fs::write(&path, contents).expect("write temp file");
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Cleanup is best effort: a leftover temp dir must not mask the test result.
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn cite(path: &str, start: usize, end: usize) -> Citation {
        Citation {
            path: path.to_owned(),
            start,
            end,
        }
    }

    fn pair(key: &str, value: &str) -> KeyValue {
        KeyValue {
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    fn location(path: &str, line: usize) -> SourceLocation {
        SourceLocation {
            path: PathBuf::from(path),
            line,
        }
    }

    fn ids(text: &str) -> Vec<String> {
        id_tokens(text).into_iter().map(|(_, id)| id).collect()
    }

    #[test]
    fn split_cells_handles_escapes_and_code_spans() {
        assert_eq!(split_cells(r"| a \| b | `x\|y` |"), ["a | b", "`x|y`"]);
        assert_eq!(split_cells("| `a | b` | c |"), ["`a | b`", "c"]);
        assert_eq!(split_cells("| `a | b |"), ["`a", "b"]);
        assert_eq!(split_cells("| ``a ` | b`` | c |"), ["``a ` | b``", "c"]);
        assert_eq!(split_cells(r"| a\b | \`x | y` |"), [r"a\b", r"\`x | y`"]);
        assert_eq!(split_cells("\t| a |\t"), ["a"]);
    }

    #[test]
    fn split_cells_takes_optional_outer_pipes() {
        let expected = ["a", "b"];
        assert_eq!(split_cells("| a | b |"), expected);
        assert_eq!(split_cells("| a | b"), expected);
        assert_eq!(split_cells("a | b |"), expected);
        assert_eq!(split_cells("a | b"), expected);
        assert_eq!(split_cells("| a | |"), ["a", ""]);
        assert_eq!(split_cells("|"), [""]);
    }

    #[test]
    fn delimiter_rows() {
        for row in ["|:---|:--:|---:|", "| --- | - |", "  |---|", "|-"] {
            assert!(is_delimiter_row(row), "{row}");
        }
        for row in [
            "|:|",
            "| a | --- |",
            "|::|",
            "---|---",
            "|",
            "| -:- |",
            "|--|  |",
        ] {
            assert!(!is_delimiter_row(row), "{row}");
        }
    }

    #[test]
    fn parse_tracks_sections_subsections_and_keys() {
        let text = "# CardDemo parity oracle audit\n\
            ## D. Fixture expectations\n\
            Intro text.\n\
            ### D.1 POSTTRAN reject set\n\
            Keys: record, `tran_id`; reason\n\
            | ID | Program |\n\
            |---|---|\n\
            | P-ZZTEST-001 | CBTRN02C |\n\
            # Stray title\n\
            #### Detail\n\
            ##not-a-heading\n\
            | ID | Program |\n\
            |---|---|\n\
            | P-ZZTEST-002 | CBTRN02C |\n\
            ### Review log — D. Fixture expectations\n\
            | Row ID | Verdict | Note | Reviewer |\n\
            |---|---|---|---|\n\
            | P-ZZTEST-001 | pass | ok | reviewer |\n";
        let doc = Document::parse(text);
        let tables = doc.tables();
        assert_eq!(tables.len(), 3);
        for table in &tables[..2] {
            assert_eq!(table.section.as_deref(), Some("D. Fixture expectations"));
            assert_eq!(table.subsection.as_deref(), Some("D.1 POSTTRAN reject set"));
            assert_eq!(table.keys.as_deref(), Some("record, `tran_id`; reason"));
            assert_eq!(table.key_names(), ["record", "tran_id", "reason"]);
            assert!(table.is_data_table() && !table.is_review_log());
        }
        let review = &tables[2];
        assert_eq!(review.section.as_deref(), Some("D. Fixture expectations"));
        assert_eq!(
            review.subsection.as_deref(),
            Some("Review log — D. Fixture expectations")
        );
        assert_eq!(review.keys, None);
        assert!(review.key_names().is_empty());
        assert!(review.is_review_log() && !review.is_data_table());
        assert_eq!(review.header, REVIEW_LOG_COLUMNS);
    }

    #[test]
    fn parse_heading_forms() {
        assert_eq!(
            parse_heading("## A. Record layouts"),
            Some((2, "A. Record layouts"))
        );
        assert_eq!(parse_heading("  ###\tB.1 x ##  "), Some((3, "B.1 x")));
        assert_eq!(parse_heading("##"), Some((2, "")));
        assert_eq!(parse_heading("##x"), None);
        assert_eq!(parse_heading("text"), None);
        let doc = Document::parse("## First\n## Second ##\n| ID |\n|---|\n");
        assert_eq!(doc.tables()[0].section.as_deref(), Some("Second"));
    }

    #[test]
    fn parse_skips_fenced_blocks() {
        let text = "## A. Record layouts\n\
            ```text\n\
            ## Hidden\n\
            | ID | X |\n\
            |---|---|\n\
            | P-ZZTEST-009 | x |\n\
            ```\n\
            ~~~~\n\
            | ID | X |\n\
            |---|---|\n\
            ~~~\n\
            ~~~~ x\n\
            still fenced\n\
            ~~~~~\n\
            | ID | X |\n\
            |---|---|\n\
            | P-ZZTEST-001 | y |\n";
        let doc = Document::parse(text);
        assert_eq!(doc.tables().len(), 1);
        let table = &doc.tables()[0];
        assert_eq!(table.section.as_deref(), Some("A. Record layouts"));
        assert_eq!(table.rows[0].cells, ["P-ZZTEST-001", "y"]);
        assert_eq!(table.line, 15);

        let unclosed = Document::parse("## A\n````\n| ID |\n|---|\n| P-ZZTEST-002 |\n```\n");
        assert!(unclosed.tables().is_empty());
    }

    #[test]
    fn parse_numbers_lines_and_reads_crlf_as_lf() {
        let lf = "## A. Record layouts\n\n| ID | Program |\n|---|---|\n\
            | P-ZZTEST-001 | X |\n| P-ZZTEST-002 | Y |\n";
        let doc = Document::parse(lf);
        let table = &doc.tables()[0];
        assert_eq!(table.line, 3);
        let lines: Vec<usize> = table.rows.iter().map(|row| row.line).collect();
        assert_eq!(lines, [5, 6]);
        assert_eq!(Document::parse(&lf.replace('\n', "\r\n")), doc);
    }

    #[test]
    fn parse_table_boundaries() {
        let text = "| not | a table |\n\
            just text\n\
            | ID | A |\n\
            |---|---|\n\
            | P-ZZTEST-001 | a |\n\
            \n\
            | P-ZZTEST-002 | b |\n\
            | ID | B |\n\
            |---|---|\n\
            | P-ZZTEST-003 | c | extra |\n\
            Keys: k\n\
            | ID | C |\n\
            |---|---|";
        let doc = Document::parse(text);
        let tables = doc.tables();
        assert_eq!(tables.len(), 3);
        assert_eq!(tables[0].header, ["ID", "A"]);
        assert_eq!(tables[0].rows.len(), 1);
        assert_eq!(tables[1].header, ["ID", "B"]);
        assert_eq!(tables[1].line, 8);
        assert_eq!(tables[1].rows[0].cells, ["P-ZZTEST-003", "c", "extra"]);
        assert_eq!(tables[1].keys, None);
        assert_eq!(tables[2].keys.as_deref(), Some("k"));
        assert!(tables[2].rows.is_empty());
        assert!(tables.iter().all(|table| table.section.is_none()));
    }

    #[test]
    fn find_row_cell_and_section_filters() {
        let text = "## D. Fixture expectations\n\
            ### Review log — D\n\
            | Row ID | Verdict | Note | Reviewer |\n\
            |---|---|---|---|\n\
            | P-ZZTEST-003 | pass | ok | r |\n\
            ### D.1 Sample\n\
            | ID | Program | Expected result |\n\
            |---|---|---|\n\
            | P-ZZTEST-001 | CBTRN02C | reason=0102 |\n\
            ## E. Anomalies\n\
            | ID | Program | Expected result |\n\
            |---|---|---|\n\
            | P-ZZTEST-002 | CBACT04C |\n\
            ## Escalations raised\n\
            | ID | Lane change | Trigger | Evidence | Decision | Deciding request clause | Status |\n\
            |---|---|---|---|---|---|---|\n\
            | E-90 | #1 → #2 | t | e | d | c | open: user confirms |\n";
        let doc = Document::parse(text);
        let (table, row) = doc.find_row("  P-ZZTEST-002 ").expect("row");
        assert_eq!(table.section.as_deref(), Some("E. Anomalies"));
        assert_eq!(table.cell(row, "Program"), Some("CBACT04C"));
        assert_eq!(table.cell(row, "Expected result"), None);
        assert_eq!(table.cell(row, "Missing"), None);
        assert_eq!(table.column(" Program "), Some(1));
        assert_eq!(table.column("Expected result"), Some(2));
        assert_eq!(doc.find_row("P-ZZTEST-003"), None);
        assert_eq!(doc.find_row("P-ZZTEST-404"), None);
        assert_eq!(doc.section_tables("D.").count(), 2);
        assert_eq!(
            doc.section_tables("D.")
                .filter(|table| table.is_data_table())
                .count(),
            1
        );
        let raised: Vec<&Table> = doc
            .tables()
            .iter()
            .filter(|table| table.is_escalations_raised())
            .collect();
        assert_eq!(raised.len(), 1);
        assert_eq!(raised[0].header, ESCALATION_COLUMNS);
        assert!(raised[0].is_data_table());
    }

    #[test]
    fn parity_section_headings_parse_to_the_constants() {
        let mut text = String::new();
        for section in PARITY_SECTIONS {
            text.push_str("## ");
            text.push_str(section);
            text.push_str("\n\n| ID | Program |\n|---|---|\n| P-ZZTEST-001 | X |\n\n");
        }
        let doc = Document::parse(&text);
        let sections: Vec<&str> = doc
            .tables()
            .iter()
            .filter_map(|table| table.section.as_deref())
            .collect();
        assert_eq!(sections, PARITY_SECTIONS);
    }

    #[test]
    fn code_span_text_and_width() {
        let span = code_span(r#"`"abc"` (width=80)"#).expect("span");
        assert_eq!(span.text, r#""abc""#);
        assert_eq!(span.width, Some(80));
        let width = |cell: &str| code_span(cell).map(|span| span.width);
        assert_eq!(width("`x` width=100"), Some(Some(100)));
        assert_eq!(width("`x`width=7"), Some(Some(7)));
        assert_eq!(width("`x` subwidth=5"), Some(None));
        assert_eq!(width("`x` width_x=5 width=6"), Some(Some(6)));
        assert_eq!(width("`x` width=99999999999999999999999"), Some(None));
        assert_eq!(width("`x` width="), Some(None));
        assert_eq!(width("width=5 `x`"), Some(None));
        assert_eq!(width("no span width=5"), None);
    }

    #[test]
    fn code_span_stripping() {
        let text = |cell: &str| code_span(cell).map(|span| span.text);
        assert_eq!(text("`Payment ok.  `").as_deref(), Some("Payment ok.  "));
        assert_eq!(text("` a `").as_deref(), Some(" a "));
        assert_eq!(text("`` `x` ``").as_deref(), Some("`x`"));
        assert_eq!(text("```  a  ```").as_deref(), Some(" a "));
        assert_eq!(text("``   ``").as_deref(), Some("   "));
        assert_eq!(text("`` a``").as_deref(), Some(" a"));
        assert_eq!(text("`a` `b`").as_deref(), Some("a"));
        assert_eq!(text("`unclosed ``x``").as_deref(), Some("x"));
        assert_eq!(text("no span here"), None);
        assert_eq!(text("`unclosed"), None);
    }

    #[test]
    fn citations_single_range_and_list() {
        assert_eq!(
            citations("PARA [app/cbl/CBACT04C.cbl:L167] text"),
            [cite("app/cbl/CBACT04C.cbl", 167, 167)]
        );
        assert_eq!(
            citations("[app/cpy/CSUTLDPY.cpy:L284-323]"),
            [cite("app/cpy/CSUTLDPY.cpy", 284, 323)]
        );
        let path = "app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl";
        assert_eq!(
            citations(&format!("[{path}:L46, L730-731]")),
            [cite(path, 46, 46), cite(path, 730, 731)]
        );
        assert_eq!(
            citations("[a.cbl:L1] and [b.cpy:L2-3] [COPAUA0C.cbl:L46]"),
            [
                cite("a.cbl", 1, 1),
                cite("b.cpy", 2, 3),
                cite("COPAUA0C.cbl", 46, 46)
            ]
        );
        assert_eq!(
            citations("[a.cbl:L9-3] [b.cbl:L0]"),
            [cite("a.cbl", 9, 3), cite("b.cbl", 0, 0)]
        );
        assert_eq!(citations("[see [app/x.cbl:L1]]"), [cite("app/x.cbl", 1, 1)]);
        assert_eq!(
            citations("`[app/x.cbl:L1]` [app/y.cbl:L2]"),
            [cite("app/y.cbl", 2, 2)]
        );
    }

    #[test]
    fn citations_ignore_other_brackets() {
        for cell in [
            "[app/jcl/DUSRSECJ.jcl:STEP01]",
            "[E-09]",
            "[app/cbl/CBACT04C.cbl:L167, 1300-COMPUTE-INTEREST]",
            "[app/data/ASCII/dailytran.txt:type columns 17-22]",
            "[app/x.cbl:L1,]",
            "[app/x.cbl:]",
            "[app/x.cbl:L]",
            "[app/x.cbl:L1-]",
            "[app/x.cbl:L-1]",
            "[app/x.cbl:L1-2-3]",
            "[app/x.cbl:L99999999999999999999999]",
            "[app/cbl]",
            "[:L1]",
            "[a b:L1]",
            "[app/x.cbl:`L1`]",
            "[app/x.cbl:L1",
            "`[app/x.cbl:L1]`",
        ] {
            assert!(citations(cell).is_empty(), "{cell}");
        }
    }

    #[test]
    fn key_values_split_outside_code_spans() {
        assert_eq!(
            key_values(r#"route=/cards; control=button "Next""#),
            [pair("route", "/cards"), pair("control", r#"button "Next""#)]
        );
        assert_eq!(
            key_values("message=`a; b`; status=409"),
            [pair("message", "`a; b`"), pair("status", "409")]
        );
        assert_eq!(
            key_values("`k=v`=x; plain; k=1; k=2; expr=a=b;"),
            [
                pair("`k=v`", "x"),
                pair("k", "1"),
                pair("k", "2"),
                pair("expr", "a=b"),
            ]
        );
        assert!(key_values("").is_empty());
        assert!(key_values("no pairs; `a=b`").is_empty());
    }

    #[test]
    fn classify_id_accepts_only_well_formed_ids() {
        for (token, kind) in [
            ("P-CBTRN02C-004", IdKind::Parity),
            ("V-COACTUPC-012", IdKind::Validation),
            ("F-COSGN00-001", IdKind::Function),
            ("E-05", IdKind::Escalation),
            ("E-123", IdKind::Escalation),
            ("P-A-B9-1234", IdKind::Parity),
        ] {
            assert_eq!(classify_id(token), Some(kind), "{token}");
        }
        for token in [
            "CVE-2026-25727",
            "E-nn",
            "E-5",
            "E-",
            "E-05a",
            "p-x-001",
            "P-x-001",
            "P-CBTRN02C-04",
            "P-001",
            "P--X-001",
            "P-X--001",
            "P_X-001",
            "P-X_Y-001",
            "P-X-001a",
            " P-X-001",
            "P-X-001 ",
            "X-ABC-001",
            "",
        ] {
            assert_eq!(classify_id(token), None, "{token}");
        }
        assert_eq!(IdKind::Parity.prefix(), 'P');
        assert_eq!(IdKind::Validation.prefix(), 'V');
        assert_eq!(IdKind::Function.prefix(), 'F');
        assert_eq!(IdKind::Escalation.prefix(), 'E');
    }

    #[test]
    fn id_tokens_respect_boundaries() {
        assert!(
            ids("CVE-2026-25727 RUSTSEC-2026-0187 E-nn P-<KEY>-<nnn> xP-ABC-001 P-ABC-001abc")
                .is_empty()
        );
        assert!(ids("a_P-ABC-001 a-P-ABC-001 _E-05").is_empty());
        assert_eq!(ids("E-01…E-22"), ["E-01", "E-22"]);
        assert_eq!(
            id_tokens("/// CBTRN02C 1500-B-LOOKUP-ACCT — P-CBTRN02C-004"),
            [(IdKind::Parity, "P-CBTRN02C-004".to_owned())]
        );
        assert_eq!(ids("P-ABC-001-"), ["P-ABC-001"]);
        assert_eq!(
            id_tokens("(V-COACTUPC-012, F-COSGN00-001; no-message (E-24))"),
            [
                (IdKind::Validation, "V-COACTUPC-012".to_owned()),
                (IdKind::Function, "F-COSGN00-001".to_owned()),
                (IdKind::Escalation, "E-24".to_owned()),
            ]
        );
    }

    #[test]
    fn no_message_cells() {
        for cell in [
            "no-message(E-05)",
            "no-message ( E-05 )",
            "  no-message (E-05)  ",
        ] {
            assert_eq!(
                no_message_escalation(cell).as_deref(),
                Some("E-05"),
                "{cell}"
            );
        }
        for cell in [
            "no-message",
            "no-message (E-nn)",
            "no-message (E-5)",
            "no-message (E-05) x",
            "no-message (E-05",
            "no-message E-05",
            "message (E-05)",
            "no-message (P-ZZTEST-001)",
        ] {
            assert_eq!(no_message_escalation(cell), None, "{cell}");
        }
        assert!(is_no_message(" no-message (E-24)"));
        assert!(is_no_message("no-message"));
        assert!(!is_no_message(r#"`"Account ID NOT found..."`"#));
    }

    #[test]
    fn ids_with_prefix_reads_data_tables_by_kind() {
        let text = "## A. Record layouts\n\
            | ID | Program |\n\
            |---|---|\n\
            | P-ZZTEST-001 | X |\n\
            |  P-ZZTEST-002  | X |\n\
            | P-ZZTEST-1 | X |\n\
            | | X |\n\
            ### Review log — A\n\
            | Row ID | Verdict | Note | Reviewer |\n\
            |---|---|---|---|\n\
            | P-ZZTEST-003 | pass | ok | r |\n\
            ## Escalations raised\n\
            | ID | Lane change | Trigger | Evidence | Decision | Deciding request clause | Status |\n\
            |---|---|---|---|---|---|---|\n\
            | E-90 | #1 → #2 | t | e | d | c | resolved |\n\
            | E-91 | #1 → #2 | t | e | d | c | halt |\n";
        let doc = Document::parse(text);
        let expected_parity: BTreeSet<String> =
            BTreeSet::from(["P-ZZTEST-001", "P-ZZTEST-002"].map(String::from));
        let expected_escalations: BTreeSet<String> =
            BTreeSet::from(["E-90", "E-91"].map(String::from));
        assert_eq!(ids_with_prefix(&doc, IdKind::Parity), expected_parity);
        assert_eq!(
            ids_with_prefix(&doc, IdKind::Escalation),
            expected_escalations
        );
        assert!(ids_with_prefix(&doc, IdKind::Validation).is_empty());
    }

    #[test]
    fn count_lines_counts_lf_and_a_final_unterminated_line() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"\n"), 1);
        assert_eq!(count_lines(b"a\r\nb\r\n"), 2);
        assert_eq!(count_lines(b"a\nb"), 2);
        assert_eq!(count_lines(b"a\r"), 1);
        assert_eq!(count_lines(&[0xC1; 15000]), 1);
    }

    #[test]
    fn coverage_reports_uncited_and_unknown_ids() {
        let defined = BTreeSet::from(["P-ZZTEST-001", "P-ZZTEST-002"].map(String::from));
        let cited = BTreeMap::from([
            (
                "P-ZZTEST-001".to_owned(),
                vec![location("crates/x/tests/a.rs", 3)],
            ),
            (
                "P-ZZTEST-404".to_owned(),
                vec![
                    location("crates/x/tests/a.rs", 7),
                    location("crates/x/tests/b.rs", 9),
                ],
            ),
        ]);
        let result = coverage(&defined, &cited);
        assert_eq!(result.uncited, BTreeSet::from(["P-ZZTEST-002".to_owned()]));
        assert_eq!(
            result
                .unknown
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["P-ZZTEST-404"]
        );
        assert_eq!(result.unknown["P-ZZTEST-404"], cited["P-ZZTEST-404"]);
        assert!(!result.is_complete());
        let report = result.to_string();
        assert!(
            report.contains("uncited IDs (1):\n  P-ZZTEST-002\n"),
            "{report}"
        );
        assert!(report.contains("unknown IDs (1):\n"), "{report}");
        assert!(
            report.contains("P-ZZTEST-404 at crates/x/tests/a.rs:7, crates/x/tests/b.rs:9"),
            "{report}"
        );
        assert_eq!(location("a.rs", 4).to_string(), "a.rs:4");

        let complete = coverage(
            &defined,
            &BTreeMap::from([
                ("P-ZZTEST-001".to_owned(), vec![location("a.rs", 1)]),
                ("P-ZZTEST-002".to_owned(), vec![location("a.rs", 2)]),
            ]),
        );
        assert!(complete.is_complete());
        complete.assert_complete("parity audit");
        Coverage::default().assert_complete("empty audit");
    }

    #[test]
    #[should_panic(expected = "audit coverage incomplete")]
    fn assert_complete_panics_when_incomplete() {
        let defined = BTreeSet::from(["P-ZZTEST-001".to_owned()]);
        coverage(&defined, &BTreeMap::new()).assert_complete("parity audit");
    }

    #[test]
    fn repo_resolves_paths_ignoring_ascii_case() {
        let temp = TempDir::new("resolve");
        let program = temp.write("app/cbl/CBSTM03A.CBL", b"line 1\nline 2\n");
        let copybook = temp.write("app/cpy/COSTM01.CPY", b"a\r\nb\r\nc");
        let repo = Repo::new(temp.path.clone());
        assert_eq!(repo.resolve("app/cbl/CBSTM03A.cbl"), Some(program.clone()));
        assert_eq!(repo.resolve("APP/CBL/cbstm03a.cbl"), Some(program.clone()));
        assert_eq!(repo.resolve("app/cbl/CBSTM03A.CBL"), Some(program));
        assert_eq!(repo.resolve("app/cpy/costm01.cpy"), Some(copybook));
        assert_eq!(repo.resolve("app"), Some(temp.path.join("app")));
        for rel in [
            "",
            "..",
            "app/../app/cbl/CBSTM03A.CBL",
            "/abs",
            "a//b",
            r"a\b",
            "./a",
            "app/cbl/",
            "app/cbl/MISSING.cbl",
            "missing/CBSTM03A.CBL",
        ] {
            assert_eq!(repo.resolve(rel), None, "{rel}");
        }

        temp.write("x/Foo.txt", b"1");
        temp.write("x/FOO.TXT", b"2");
        let listed = fs::read_dir(temp.path.join("x")).expect("list x").count();
        if listed == 2 {
            assert_eq!(repo.resolve("x/foo.txt"), None);
        }
        assert_eq!(repo.resolve("x/Foo.txt"), Some(temp.path.join("x/Foo.txt")));
    }

    #[test]
    fn repo_line_counts_and_loads() {
        let temp = TempDir::new("load");
        temp.write("app/cpy/COSTM01.CPY", b"a\r\nb\r\nc");
        temp.write("app/data/EBCDIC/ACCT.PS", &[0xC1, 0xFF, 0x40, 0x0D]);
        temp.write(
            PARITY_AUDIT_PATH,
            b"## A. Record layouts\n| ID |\n|---|\n| P-ZZTEST-001 |\n",
        );
        let repo = Repo::new(temp.path.clone());
        assert_eq!(
            repo.line_count("app/cpy/costm01.cpy").expect("count"),
            Some(3)
        );
        assert_eq!(
            repo.line_count("app/data/ebcdic/acct.ps").expect("count"),
            Some(1)
        );
        assert_eq!(repo.line_count("app/missing.txt").expect("count"), None);
        assert!(repo.line_count("app/cpy").is_err());

        let parity = repo.parity_audit().expect("parity audit");
        assert_eq!(parity.tables().len(), 1);
        assert_eq!(repo.load(PARITY_AUDIT_PATH).expect("load"), parity);
        for missing in [
            repo.validation_audit(),
            repo.function_map(),
            repo.escalations(),
        ] {
            let error = missing.expect_err("absent document");
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
            assert!(error.to_string().contains(": not found under "), "{error}");
        }
        let error = Document::load(&temp.path.join("absent.md")).expect_err("absent file");
        assert!(error.to_string().contains("absent.md: "), "{error}");
    }

    #[test]
    fn repo_from_manifest_dir_takes_the_grandparent() {
        assert_eq!(
            Repo::from_manifest_dir("/work/repo/crates/audit").root,
            PathBuf::from("/work/repo")
        );
        assert_eq!(
            Repo::from_manifest_dir("audit").root,
            Path::new("audit").join("../..")
        );
        let repo = Repo::from_manifest_dir(env!("CARGO_MANIFEST_DIR"));
        assert!(repo.root.join("Cargo.toml").is_file());
        assert!(repo.root.join("crates/audit/Cargo.toml").is_file());
    }

    #[test]
    fn scan_cited_ids_walks_rust_sources_only() {
        let temp = TempDir::new("scan");
        let first = temp.write(
            "src/a.rs",
            b"// header\n// P-ZZTEST-001 and P-ZZTEST-001 again\nfn f() {}\n\
              /// P-ZZTEST-001, E-90 and V-ZZTEST-001\n",
        );
        let lossy = temp.write("src/lossy.rs", b"// \xFF P-ZZTEST-003\n");
        let nested = temp.write("src/nested/b.rs", b"\n// P-ZZTEST-002\n");
        temp.write("target/b.rs", b"// P-ZZTEST-777\n");
        temp.write("src/target/b.rs", b"// P-ZZTEST-776\n");
        temp.write(".hidden/c.rs", b"// P-ZZTEST-778\n");
        temp.write("d.txt", b"P-ZZTEST-779\n");
        temp.write("src/e.rsx", b"P-ZZTEST-780\n");

        let cited = scan_cited_ids(&[temp.path.as_path()], IdKind::Parity).expect("scan");
        let expected = BTreeMap::from([
            (
                "P-ZZTEST-001".to_owned(),
                vec![
                    SourceLocation {
                        path: first.clone(),
                        line: 2,
                    },
                    SourceLocation {
                        path: first.clone(),
                        line: 4,
                    },
                ],
            ),
            (
                "P-ZZTEST-002".to_owned(),
                vec![SourceLocation {
                    path: nested,
                    line: 2,
                }],
            ),
            (
                "P-ZZTEST-003".to_owned(),
                vec![SourceLocation {
                    path: lossy,
                    line: 1,
                }],
            ),
        ]);
        assert_eq!(cited, expected);

        let escalations = scan_cited_ids(&[first.as_path()], IdKind::Escalation).expect("scan");
        assert_eq!(
            escalations,
            BTreeMap::from([(
                "E-90".to_owned(),
                vec![SourceLocation {
                    path: first.clone(),
                    line: 4
                }],
            )])
        );
        let validation = scan_cited_ids(&[first.as_path()], IdKind::Validation).expect("scan");
        assert_eq!(validation.keys().collect::<Vec<_>>(), ["V-ZZTEST-001"]);

        let error = scan_cited_ids(&[temp.path.join("absent").as_path()], IdKind::Parity)
            .expect_err("missing entry");
        assert!(error.to_string().contains("absent: "), "{error}");
    }
}
