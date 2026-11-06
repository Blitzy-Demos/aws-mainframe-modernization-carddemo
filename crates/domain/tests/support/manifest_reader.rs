// Std-only reader of the TOML that Cargo manifests and Cargo configuration files are
// written in, and of the dependency declarations a manifest makes.
//
// The domain boundary test (crates/domain/tests/manifest_boundary.rs) and the repository
// hygiene gates (crates/testkit/tests/hygiene.rs) both include this file as a module by
// path. Sharing source rather than a crate keeps the domain free of any test-support
// dependency, and a TOML parser crate would be one more dependency of the crates those
// tests guard.
//
// The reader accepts the TOML 1.0 forms manifests use and rejects every other line, so a
// form it does not understand fails the gate instead of passing unread. Both including
// test crates compile it with dead-code warnings denied, so every item here is used by
// each of them.

use std::fmt;

// A line the reader rejects, 1-based, with the reason.
#[derive(Debug)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

// A TOML value. Strings are decoded; booleans, numbers and dates keep their written text.
#[derive(Debug, PartialEq, Eq)]
pub enum Value {
    // A table header, an array-of-tables header or an inline table, possibly empty.
    Table,
    Str(String),
    Array(Vec<Value>),
    Other(String),
}

impl fmt::Display for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Table => formatter.write_str("a table"),
            Self::Str(text) => write!(formatter, "{text:?}"),
            Self::Other(text) => formatter.write_str(text),
            Self::Array(items) => {
                formatter.write_str("[")?;
                for (at, item) in items.iter().enumerate() {
                    if at > 0 {
                        formatter.write_str(", ")?;
                    }
                    write!(formatter, "{item}")?;
                }
                formatter.write_str("]")
            }
        }
    }
}

// One table or key-value pair. `path` is the full key path: the current table followed by
// the dotted key, with every quoted segment decoded.
#[derive(Debug)]
pub struct Entry {
    pub line: usize,
    pub path: Vec<String>,
    pub value: Value,
}

// Every table and key-value pair of `text`, in document order. A key-value pair whose value
// is an inline table yields an entry for the table, empty ones included, then one entry per
// child. Inline tables inside arrays are read and kept as `Value::Table`.
pub fn entries(text: &str) -> Result<Vec<Entry>, ParseError> {
    let mut reader = Reader {
        text,
        at: 0,
        line: 1,
    };
    if text.starts_with('\u{feff}') {
        reader.at = '\u{feff}'.len_utf8();
    }
    let mut table: Vec<String> = Vec::new();
    let mut out = Vec::new();
    loop {
        reader.skip_spaces();
        reader.skip_comment();
        if reader.newline() {
            continue;
        }
        match reader.peek() {
            None => return Ok(out),
            Some(b'[') => table = reader.header(&mut out)?,
            Some(_) => reader.pair(&table, &mut out)?,
        }
        reader.end_of_line()?;
    }
}

// Characters a TOML string may not hold unescaped: the controls other than tab and
// newline. A newline is checked separately, by the string forms that forbid it.
fn is_control(ch: char) -> bool {
    matches!(ch, '\u{0}'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}')
}

// Whether `text` is a boolean, number or date-time as written. A date and a time may be
// separated by one space.
fn is_scalar(text: &str) -> bool {
    let word = |part: &str| {
        !part.is_empty()
            && part.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.' | b'_' | b':')
            })
    };
    match text.split_once(' ') {
        Some((date, time)) => {
            date.len() == 10
                && date
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'-')
                && word(time)
        }
        None => word(text),
    }
}

// Cursor over TOML text. `at` is a byte offset that only ever rests on a character
// boundary when text is sliced, and `line` is the 1-based line it is on.
struct Reader<'a> {
    text: &'a str,
    at: usize,
    line: usize,
}

impl<'a> Reader<'a> {
    fn rest(&self) -> &'a str {
        &self.text[self.at..]
    }

    fn peek(&self) -> Option<u8> {
        self.text.as_bytes().get(self.at).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.text.as_bytes().get(self.at + ahead).copied()
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, ParseError> {
        Err(ParseError {
            line: self.line,
            message: message.into(),
        })
    }

    fn expect(&mut self, byte: u8, what: &str) -> Result<(), ParseError> {
        if self.peek() == Some(byte) {
            self.at += 1;
            Ok(())
        } else {
            self.error(format!("expected {what}"))
        }
    }

    fn skip_spaces(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.at += 1;
        }
    }

    // Consumes the newline, LF or CRLF, that starts here, if one does.
    fn newline(&mut self) -> bool {
        let len = match (self.peek(), self.peek_at(1)) {
            (Some(b'\n'), _) => 1,
            (Some(b'\r'), Some(b'\n')) => 2,
            _ => return false,
        };
        self.at += len;
        self.line += 1;
        true
    }

    // Skips the comment that starts here, if one does, up to its newline.
    fn skip_comment(&mut self) {
        if self.peek() != Some(b'#') {
            return;
        }
        let len = self.rest().find(['\n', '\r']).unwrap_or(self.rest().len());
        self.at += len;
    }

    // Whitespace, newlines and comments, as TOML allows between array elements.
    fn skip_trivia(&mut self) {
        loop {
            self.skip_spaces();
            self.skip_comment();
            if !self.newline() {
                return;
            }
        }
    }

    // After a statement: optional whitespace and comment, then a newline or the end.
    fn end_of_line(&mut self) -> Result<(), ParseError> {
        self.skip_spaces();
        self.skip_comment();
        if self.peek().is_none() || self.newline() {
            Ok(())
        } else {
            self.error("unexpected text after the statement")
        }
    }

    // A table or array-of-tables header, recorded in `out`; returns its key path.
    fn header(&mut self, out: &mut Vec<Entry>) -> Result<Vec<String>, ParseError> {
        let line = self.line;
        self.at += 1;
        let array = self.peek() == Some(b'[');
        if array {
            self.at += 1;
        }
        self.skip_spaces();
        let path = self.key()?;
        self.expect(b']', "`]` closing the table header")?;
        if array {
            self.expect(b']', "`]]` closing the array-of-tables header")?;
        }
        out.push(Entry {
            line,
            path: path.clone(),
            value: Value::Table,
        });
        Ok(path)
    }

    // A dotted key, with spaces or tabs allowed around each dot and after the key.
    fn key(&mut self) -> Result<Vec<String>, ParseError> {
        let mut segments = vec![self.key_segment()?];
        loop {
            self.skip_spaces();
            if self.peek() != Some(b'.') {
                return Ok(segments);
            }
            self.at += 1;
            self.skip_spaces();
            segments.push(self.key_segment()?);
        }
    }

    // One key segment: bare, basic-quoted with escapes decoded, or literal-quoted.
    fn key_segment(&mut self) -> Result<String, ParseError> {
        match self.peek() {
            Some(b'"') => self.basic_string(),
            Some(b'\'') => self.literal_string(),
            _ => {
                let len = self
                    .rest()
                    .bytes()
                    .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                    .count();
                if len == 0 {
                    return self.error("expected a key");
                }
                let segment = self.rest()[..len].to_owned();
                self.at += len;
                Ok(segment)
            }
        }
    }

    // `key = value` below `prefix`, recorded in `out`.
    fn pair(&mut self, prefix: &[String], out: &mut Vec<Entry>) -> Result<(), ParseError> {
        let line = self.line;
        let mut path = prefix.to_vec();
        path.extend(self.key()?);
        self.expect(b'=', "`=` after the key")?;
        self.skip_spaces();
        if self.peek() == Some(b'{') {
            return self.inline_table(&path, line, out);
        }
        let value = self.value()?;
        out.push(Entry { line, path, value });
        Ok(())
    }

    // An inline table at `path`: an entry for the table itself, then one per child.
    fn inline_table(
        &mut self,
        path: &[String],
        line: usize,
        out: &mut Vec<Entry>,
    ) -> Result<(), ParseError> {
        out.push(Entry {
            line,
            path: path.to_vec(),
            value: Value::Table,
        });
        self.at += 1;
        self.skip_spaces();
        if self.peek() == Some(b'}') {
            self.at += 1;
            return Ok(());
        }
        loop {
            self.pair(path, out)?;
            self.skip_spaces();
            match self.peek() {
                Some(b',') => {
                    self.at += 1;
                    self.skip_spaces();
                }
                Some(b'}') => {
                    self.at += 1;
                    return Ok(());
                }
                _ => return self.error("expected `,` or `}` in an inline table"),
            }
        }
    }

    // A value in array or key-value position. An inline table is read and kept as
    // `Value::Table` without its children.
    fn value(&mut self) -> Result<Value, ParseError> {
        match self.peek() {
            Some(b'"' | b'\'') => Ok(Value::Str(self.string()?)),
            Some(b'[') => self.array(),
            Some(b'{') => {
                let line = self.line;
                self.inline_table(&[], line, &mut Vec::new())?;
                Ok(Value::Table)
            }
            _ => self.scalar(),
        }
    }

    fn array(&mut self) -> Result<Value, ParseError> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.skip_trivia();
            if self.peek() == Some(b']') {
                self.at += 1;
                return Ok(Value::Array(items));
            }
            items.push(self.value()?);
            self.skip_trivia();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    return Ok(Value::Array(items));
                }
                _ => return self.error("expected `,` or `]` in an array"),
            }
        }
    }

    // A boolean, number or date-time: the text up to the next delimiter, trimmed.
    fn scalar(&mut self) -> Result<Value, ParseError> {
        let len = self
            .rest()
            .find([',', ']', '}', '#', '\n', '\r'])
            .unwrap_or(self.rest().len());
        let text = self.rest()[..len].trim_matches([' ', '\t']);
        if !is_scalar(text) {
            return self.error("expected a value");
        }
        let value = Value::Other(text.to_owned());
        self.at += len;
        Ok(value)
    }

    // Any of the four string forms, decoded.
    fn string(&mut self) -> Result<String, ParseError> {
        let rest = self.rest();
        if rest.starts_with("\"\"\"") {
            self.multiline_basic()
        } else if rest.starts_with("'''") {
            self.multiline_literal()
        } else if rest.starts_with('"') {
            self.basic_string()
        } else {
            self.literal_string()
        }
    }

    // The next character of a string body, or an error at the end of the text.
    fn next_char(&self) -> Result<char, ParseError> {
        match self.rest().chars().next() {
            Some(ch) => Ok(ch),
            None => self.error("unterminated string"),
        }
    }

    fn basic_string(&mut self) -> Result<String, ParseError> {
        self.at += 1;
        let mut text = String::new();
        loop {
            let ch = self.next_char()?;
            self.at += ch.len_utf8();
            match ch {
                '"' => return Ok(text),
                '\\' => text.push(self.escape()?),
                '\n' | '\r' => return self.error("newline inside a single-line string"),
                _ if is_control(ch) => return self.error("control character inside a string"),
                _ => text.push(ch),
            }
        }
    }

    fn literal_string(&mut self) -> Result<String, ParseError> {
        self.at += 1;
        let mut text = String::new();
        loop {
            let ch = self.next_char()?;
            self.at += ch.len_utf8();
            match ch {
                '\'' => return Ok(text),
                '\n' | '\r' => return self.error("newline inside a single-line string"),
                _ if is_control(ch) => return self.error("control character inside a string"),
                _ => text.push(ch),
            }
        }
    }

    // A run of `quote` inside a multi-line string with that delimiter: one or two are
    // content, three to five close the string after up to two content quotes. Returns
    // whether the string is closed.
    fn quote_run(&mut self, quote: char, text: &mut String) -> Result<bool, ParseError> {
        let run = self.rest().chars().take_while(|&ch| ch == quote).count();
        if run > 5 {
            return self.error("too many quotes closing a multi-line string");
        }
        let content = if run >= 3 { run - 3 } else { run };
        text.extend(std::iter::repeat_n(quote, content));
        self.at += run;
        Ok(run >= 3)
    }

    // A newline inside a multi-line string, kept as LF; false when none starts here.
    fn content_newline(&mut self, text: &mut String) -> bool {
        let consumed = self.newline();
        if consumed {
            text.push('\n');
        }
        consumed
    }

    fn multiline_basic(&mut self) -> Result<String, ParseError> {
        self.at += 3;
        self.newline();
        let mut text = String::new();
        loop {
            if self.content_newline(&mut text) {
                continue;
            }
            let ch = self.next_char()?;
            if ch == '"' {
                if self.quote_run('"', &mut text)? {
                    return Ok(text);
                }
                continue;
            }
            self.at += ch.len_utf8();
            if ch == '\\' {
                if !self.line_ending_backslash() {
                    text.push(self.escape()?);
                }
            } else if is_control(ch) {
                return self.error("control character inside a string");
            } else {
                text.push(ch);
            }
        }
    }

    fn multiline_literal(&mut self) -> Result<String, ParseError> {
        self.at += 3;
        self.newline();
        let mut text = String::new();
        loop {
            if self.content_newline(&mut text) {
                continue;
            }
            let ch = self.next_char()?;
            if ch == '\'' {
                if self.quote_run('\'', &mut text)? {
                    return Ok(text);
                }
                continue;
            }
            self.at += ch.len_utf8();
            if is_control(ch) {
                return self.error("control character inside a string");
            }
            text.push(ch);
        }
    }

    // After a backslash in a multi-line basic string: when only whitespace follows it on
    // its line, skips that whitespace and every newline and whitespace after it, and
    // returns true. Otherwise consumes nothing and returns false.
    fn line_ending_backslash(&mut self) -> bool {
        let start = self.at;
        self.skip_spaces();
        if !self.newline() {
            self.at = start;
            return false;
        }
        loop {
            self.skip_spaces();
            if !self.newline() {
                return true;
            }
        }
    }

    // The character an escape stands for; the backslash is already consumed.
    fn escape(&mut self) -> Result<char, ParseError> {
        let code = self.next_char()?;
        self.at += code.len_utf8();
        match code {
            'b' => Ok('\u{8}'),
            't' => Ok('\t'),
            'n' => Ok('\n'),
            'f' => Ok('\u{c}'),
            'r' => Ok('\r'),
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            'u' => self.unicode(4),
            'U' => self.unicode(8),
            _ => self.error(format!("invalid escape sequence `\\{code}`")),
        }
    }

    fn unicode(&mut self, digits: usize) -> Result<char, ParseError> {
        let Some(hex) = self
            .rest()
            .get(..digits)
            .filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
        else {
            return self.error(format!(
                "a unicode escape needs {digits} hexadecimal digits"
            ));
        };
        let scalar = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
        self.at += digits;
        match scalar {
            Some(ch) => Ok(ch),
            None => self.error(format!(
                "unicode escape {hex} is not a Unicode scalar value"
            )),
        }
    }
}

// The dependency table a declaration sits in: `dependencies` is Normal, the dev and build
// tables in either spelling are Dev and Build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
}

// The full identity of a dependency table: its kind and, for `target.<platform>.<kind>`,
// the decoded platform.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub kind: DepKind,
    pub target: Option<String>,
}

// A dependency: the key it is declared under and the package that key resolves to.
#[derive(Debug)]
pub struct Dependency {
    pub key: String,
    pub package: String,
}

// A dependency table, or a dependency in one, at the line it is first declared. A
// declaration without a dependency is the table itself, empty or not.
#[derive(Debug)]
pub struct Declaration {
    pub line: usize,
    pub table: Table,
    pub dependency: Option<Dependency>,
}

// What the statements of one declaration say before its package is resolved.
struct Draft {
    line: usize,
    table: Table,
    key: Option<String>,
    package: Option<String>,
    inherits: bool,
}

fn dep_kind(name: &str) -> Option<DepKind> {
    match name {
        "dependencies" => Some(DepKind::Normal),
        "dev-dependencies" | "dev_dependencies" => Some(DepKind::Dev),
        "build-dependencies" | "build_dependencies" => Some(DepKind::Build),
        _ => None,
    }
}

// The dependency table a full key path addresses, and the path below that table:
// `target`, `cfg(unix)`, `dev-dependencies`, `foo`, `workspace` gives the Dev table of
// `cfg(unix)` and `foo`, `workspace`.
fn dependency_scope(path: &[String]) -> Option<(Table, &[String])> {
    match path {
        [first, platform, kind, rest @ ..] if first == "target" => dep_kind(kind).map(|kind| {
            let table = Table {
                kind,
                target: Some(platform.clone()),
            };
            (table, rest)
        }),
        [kind, rest @ ..] => dep_kind(kind).map(|kind| (Table { kind, target: None }, rest)),
        [] => None,
    }
}

// Whether `name` has the shape of a crate name. Any other key or package means a form the
// dependency checks cannot resolve, so it is rejected rather than passed unchecked.
fn is_crate_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

// Folds one attribute statement of a dependency into its draft.
fn apply(draft: &mut Draft, attribute: &[String], entry: &Entry) -> Result<(), ParseError> {
    let key = draft.key.clone().unwrap_or_default();
    let fail = |message: String| {
        Err(ParseError {
            line: entry.line,
            message,
        })
    };
    let [field] = attribute else {
        return Ok(());
    };
    match (field.as_str(), &entry.value) {
        ("package", Value::Str(package)) if draft.package.is_none() => {
            draft.package = Some(package.clone());
        }
        ("package", Value::Str(_)) => return fail(format!("`{key}` names its package twice")),
        ("package", value) => {
            return fail(format!(
                "the package of `{key}` must be a string, not {value}"
            ));
        }
        ("workspace", Value::Other(flag)) if flag == "true" => draft.inherits = true,
        ("workspace", value) => {
            return fail(format!(
                "`{key}` sets workspace to {value}; only true inherits"
            ));
        }
        _ => {}
    }
    Ok(())
}

// The package `workspace.dependencies.<key>` names in the workspace manifest: its
// `package` when it renames one, else the key itself. Cargo matches the key exactly.
fn workspace_package(workspace: &[Entry], key: &str, line: usize) -> Result<String, ParseError> {
    let mut declared = false;
    let mut package = None;
    for entry in workspace {
        let [first, second, name, rest @ ..] = entry.path.as_slice() else {
            continue;
        };
        if first != "workspace" || second != "dependencies" || name != key {
            continue;
        }
        declared = true;
        if rest.len() == 1 && rest[0] == "package" {
            match &entry.value {
                Value::Str(renamed) => package = Some(renamed.clone()),
                other => {
                    return Err(ParseError {
                        line,
                        message: format!(
                            "the workspace names the package of `{key}` as {other}, not a string"
                        ),
                    });
                }
            }
        }
    }
    if !declared {
        return Err(ParseError {
            line,
            message: format!(
                "`{key}` inherits from the workspace, whose dependencies do not declare it"
            ),
        });
    }
    Ok(package.unwrap_or_else(|| key.to_owned()))
}

// The declaration a draft resolves to.
fn resolve(draft: Draft, workspace: &[Entry]) -> Result<Declaration, ParseError> {
    let Draft {
        line,
        table,
        key,
        package,
        inherits,
    } = draft;
    let fail = |message: String| Err(ParseError { line, message });
    let Some(key) = key else {
        return Ok(Declaration {
            line,
            table,
            dependency: None,
        });
    };
    let package = match (package, inherits) {
        // Cargo ignores `package` beside `workspace = true` and takes the workspace's.
        (Some(_), true) => {
            return fail(format!(
                "`{key}` names a package and inherits from the workspace, which Cargo ignores"
            ));
        }
        (Some(package), false) => package,
        (None, true) => workspace_package(workspace, &key, line)?,
        (None, false) => key.clone(),
    };
    for name in [&key, &package] {
        if !is_crate_name(name) {
            return fail(format!(
                "`{name}` is not a crate name the dependency checks can vouch for"
            ));
        }
    }
    Ok(Declaration {
        line,
        table,
        dependency: Some(Dependency { key, package }),
    })
}

// Every dependency table and dependency `manifest` declares, in document order, wherever
// its key path puts it: under a header, as a root-level dotted key or as an inline table.
// The statements of one dependency in one table merge into one declaration at the line of
// the first. A dependency inherited with `workspace = true` resolves its package through
// `workspace_manifest`.
pub fn declarations(
    manifest: &str,
    workspace_manifest: &str,
) -> Result<Vec<Declaration>, ParseError> {
    let mut drafts: Vec<Draft> = Vec::new();
    for entry in entries(manifest)? {
        let Some((table, below)) = dependency_scope(&entry.path) else {
            continue;
        };
        let Some((key, attribute)) = below.split_first() else {
            if entry.value != Value::Table {
                return Err(ParseError {
                    line: entry.line,
                    message: format!("a dependency table holds {}", entry.value),
                });
            }
            drafts.push(Draft {
                line: entry.line,
                table,
                key: None,
                package: None,
                inherits: false,
            });
            continue;
        };
        let known = drafts.iter().position(|draft| {
            draft.key.as_ref() == Some(key)
                && draft.table.kind == table.kind
                && draft.table.target == table.target
        });
        let at = known.unwrap_or_else(|| {
            drafts.push(Draft {
                line: entry.line,
                table,
                key: Some(key.clone()),
                package: None,
                inherits: false,
            });
            drafts.len() - 1
        });
        apply(&mut drafts[at], attribute, &entry)?;
    }
    let workspace = match drafts.iter().find(|draft| draft.inherits) {
        Some(draft) => entries(workspace_manifest).map_err(|error| ParseError {
            line: draft.line,
            message: format!(
                "the workspace manifest cannot be read (its line {}: {})",
                error.line, error.message
            ),
        })?,
        None => Vec::new(),
    };
    drafts
        .into_iter()
        .map(|draft| resolve(draft, &workspace))
        .collect()
}
