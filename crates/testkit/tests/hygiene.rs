// Static hygiene and layering gates of the CardDemo repository.
//
// Every test reads repository text with std only and fails on each violation it finds,
// listed one per line as `path:line: rule: excerpt`. No service is needed, so the suite
// runs on a machine without Docker. The gates:
//
// - Rust sources under crates/: the keyword that opts out of memory safety, lint
//   suppression attributes, the unchecked sqlx query functions, the raw SQL entry point of
//   sqlx outside the testkit library, SQL statements built with the format macro, and the
//   legacy `transactions` table named outside its two owning modules.
// - Financial and reward crates: no floating-point type, except the printpdf page geometry
//   of crates/batch/src/pdf.rs, whose input is formatted text lines only.
// - Manifests: lints come only from the workspace table at deny or forbid, and the crate
//   graph keeps its forbidden edges absent.
// - migrations/*.sql: no floating-point column type.
// - web/: no lint or type-check suppression, no float parsing, and network calls only
//   through the single typed API client.
//
// This file is scanned by itself. Only its string literals are blanked before the scan, so
// every pattern it searches for is written inside a string literal and comments describe
// patterns in words.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use carddemo_testkit::fixtures::repo_root;

// Repository-relative path of this file, the one file whose string literals are blanked.
const THIS_FILE: &str = "crates/testkit/tests/hygiene.rs";

// The nine workspace crates, by directory under crates/.
const CRATES: [&str; 9] = [
    "domain",
    "codec",
    "persistence",
    "runtime",
    "api",
    "batch",
    "messaging",
    "audit",
    "testkit",
];

// Build output and installed dependencies, skipped at any depth.
const SKIP_DIRS: [&str; 6] = [
    "target",
    "node_modules",
    "dist",
    ".git",
    "test-results",
    "playwright-report",
];

// Longest excerpt a violation line carries, in characters.
const EXCERPT_CHARS: usize = 120;

// The one module allowed a floating-point type: printpdf takes page geometry as f32.
const PDF_MODULE: &str = "crates/batch/src/pdf.rs";

// The only web module allowed to call the network.
const API_CLIENT: &str = "web/src/api/client.ts";

// The modules whose query macros may name the legacy table; every other legacy read goes
// through the transaction_master view.
const TRANSACTIONS_OWNERS: [&str; 2] = [
    "crates/persistence/src/repos/transactions.rs",
    "crates/persistence/src/rewards.rs",
];

// ---------------------------------------------------------------------------------------
// Paths, walking and reporting
// ---------------------------------------------------------------------------------------

fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(repo_root)
}

// Repository-relative form of `path`, with `/` separators.
fn rel(path: &Path) -> String {
    let relative = path.strip_prefix(root()).unwrap_or(path);
    relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| exts.contains(&ext))
}

// Every file below `root` that `keep` accepts, sorted. Directory symlinks are not followed,
// and build or dependency output directories are skipped at any depth.
fn walk(root: &Path, keep: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let entries = fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!("cannot read an entry of {}: {error}", dir.display())
            });
            let path = entry.path();
            let file_type = entry
                .file_type()
                .unwrap_or_else(|error| panic!("cannot stat {}: {error}", path.display()));
            if file_type.is_dir() {
                let name = entry.file_name();
                if !SKIP_DIRS.iter().any(|skip| name == *skip) {
                    pending.push(path);
                }
            } else if (file_type.is_file() || path.is_file()) && keep(&path) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

// The directory `rel` under the repository root, which must exist.
fn require_dir(rel: &str) -> PathBuf {
    let dir = root().join(rel);
    assert!(
        dir.is_dir(),
        "hygiene input directory {rel} is missing under {}",
        root().display()
    );
    dir
}

// The file `rel` under the repository root, which must exist.
fn require_file(rel: &str) -> PathBuf {
    let file = root().join(rel);
    assert!(
        file.is_file(),
        "hygiene input file {rel} is missing under {}",
        root().display()
    );
    file
}

fn read_bytes(path: &Path) -> Vec<u8> {
    fs::read(path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
}

fn read_text(path: &Path) -> String {
    String::from_utf8_lossy(&read_bytes(path)).into_owned()
}

// 1-based line number of byte `offset`.
fn line_of(bytes: &[u8], offset: usize) -> usize {
    bytes[..offset.min(bytes.len())]
        .split(|&byte| byte == b'\n')
        .count()
}

// The trimmed text of 1-based line `line`, cut to EXCERPT_CHARS characters.
fn line_text(text: &[u8], line: usize) -> String {
    let raw = text
        .split(|&byte| byte == b'\n')
        .nth(line.saturating_sub(1))
        .unwrap_or_default();
    String::from_utf8_lossy(raw)
        .trim()
        .chars()
        .take(EXCERPT_CHARS)
        .collect()
}

fn report_line(violations: &mut Vec<String>, path: &str, src: &[u8], line: usize, rule: &str) {
    let excerpt = line_text(src, line);
    violations.push(format!("{path}:{line}: {rule}: {excerpt}"));
}

fn report(violations: &mut Vec<String>, path: &str, src: &[u8], offset: usize, rule: &str) {
    report_line(violations, path, src, line_of(src, offset), rule);
}

// Reports (offset, rule) hits of one file in source order.
fn report_in_order(
    violations: &mut Vec<String>,
    path: &str,
    src: &[u8],
    mut hits: Vec<(usize, String)>,
) {
    hits.sort();
    for (at, rule) in hits {
        report(violations, path, src, at, &rule);
    }
}

fn finish(violations: &[String]) {
    assert!(
        violations.is_empty(),
        "{} violation(s):\n{}",
        violations.len(),
        violations.join("\n")
    );
}

// ---------------------------------------------------------------------------------------
// Token helpers
// ---------------------------------------------------------------------------------------

fn is_ident(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn is_word_byte(byte: u8, extra_ident: &[u8]) -> bool {
    is_ident(byte) || extra_ident.contains(&byte)
}

fn occurrences(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return Vec::new();
    }
    hay.windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(at, _)| at)
        .collect()
}

// Occurrences of `tok` whose preceding byte is not an identifier byte.
fn find_left_token(hay: &[u8], tok: &[u8], extra_ident: &[u8]) -> Vec<usize> {
    occurrences(hay, tok)
        .into_iter()
        .filter(|&at| at == 0 || !is_word_byte(hay[at - 1], extra_ident))
        .collect()
}

// Occurrences of `tok` as a whole identifier: neither neighbour is an identifier byte.
fn find_token(hay: &[u8], tok: &[u8], extra_ident: &[u8]) -> Vec<usize> {
    find_left_token(hay, tok, extra_ident)
        .into_iter()
        .filter(|&at| {
            hay.get(at + tok.len())
                .is_none_or(|&byte| !is_word_byte(byte, extra_ident))
        })
        .collect()
}

fn skip_ws(text: &[u8], mut at: usize) -> usize {
    while text.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

// The identifier that starts at `at`, empty when none does.
fn word_at(text: &[u8], at: usize) -> &[u8] {
    let rest = text.get(at..).unwrap_or_default();
    let len = rest.iter().take_while(|&&byte| is_ident(byte)).count();
    &rest[..len]
}

// Calls of `name`: a left identifier boundary, then optional whitespace and an opening
// parenthesis.
fn find_call(hay: &[u8], name: &[u8], extra_ident: &[u8]) -> Vec<usize> {
    find_left_token(hay, name, extra_ident)
        .into_iter()
        .filter(|&at| hay.get(skip_ws(hay, at + name.len())) == Some(&b'('))
        .collect()
}

// ---------------------------------------------------------------------------------------
// Rust lexer
// ---------------------------------------------------------------------------------------

// What a span of Rust source is. Char and byte literals count as Str.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Code,
    Comment,
    Str,
}

// A lexical span. For Str, content_start..content_end are the bytes between the quotes;
// for the other kinds they equal start..end.
#[derive(Clone, Copy, Debug)]
struct Span {
    kind: Kind,
    start: usize,
    end: usize,
    content_start: usize,
    content_end: usize,
}

impl Span {
    fn plain(kind: Kind, start: usize, end: usize) -> Self {
        Self {
            kind,
            start,
            end,
            content_start: start,
            content_end: end,
        }
    }

    fn literal(start: usize, end: usize, content_start: usize, content_end: usize) -> Self {
        Self {
            kind: Kind::Str,
            start,
            end,
            content_start,
            content_end,
        }
    }
}

// Splits Rust source into consecutive Code, Comment and Str spans covering every byte.
fn lex_rust(src: &[u8]) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut code_start = 0;
    let mut at = 0;
    while at < src.len() {
        if let Some(span) = literal_at(src, at) {
            if span.start > code_start {
                spans.push(Span::plain(Kind::Code, code_start, span.start));
            }
            at = span.end;
            code_start = span.end;
            spans.push(span);
        } else {
            at += 1;
        }
    }
    if code_start < src.len() {
        spans.push(Span::plain(Kind::Code, code_start, src.len()));
    }
    spans
}

// The comment or literal that starts at `at`, if one does.
fn literal_at(src: &[u8], at: usize) -> Option<Span> {
    match src[at] {
        b'/' => comment_at(src, at),
        b'"' => Some(quoted_at(src, at, at)),
        b'\'' => char_at(src, at, at),
        b'b' | b'c' | b'r' if at == 0 || !is_ident(src[at - 1]) => prefixed_at(src, at),
        _ => None,
    }
}

fn comment_at(src: &[u8], at: usize) -> Option<Span> {
    match src.get(at + 1) {
        Some(b'/') => {
            let len = src[at..].iter().take_while(|&&byte| byte != b'\n').count();
            Some(Span::plain(Kind::Comment, at, at + len))
        }
        Some(b'*') => {
            let mut depth = 1_usize;
            let mut cursor = at + 2;
            while cursor < src.len() && depth > 0 {
                if src[cursor..].starts_with(b"/*") {
                    depth += 1;
                    cursor += 2;
                } else if src[cursor..].starts_with(b"*/") {
                    depth -= 1;
                    cursor += 2;
                } else {
                    cursor += 1;
                }
            }
            Some(Span::plain(Kind::Comment, at, cursor.min(src.len())))
        }
        _ => None,
    }
}

// A string whose opening quote sits at `quote`; `start` includes any prefix letter.
fn quoted_at(src: &[u8], start: usize, quote: usize) -> Span {
    let mut cursor = quote + 1;
    while cursor < src.len() {
        match src[cursor] {
            b'\\' => cursor += 2,
            b'"' => return Span::literal(start, cursor + 1, quote + 1, cursor),
            _ => cursor += 1,
        }
    }
    Span::literal(start, src.len(), (quote + 1).min(src.len()), src.len())
}

// A char or byte literal whose opening quote sits at `quote`, or None for a lifetime or a
// label, which stays code.
fn char_at(src: &[u8], start: usize, quote: usize) -> Option<Span> {
    let first = quote + 1;
    match *src.get(first)? {
        b'\\' => {
            let mut cursor = first + 2;
            while cursor < src.len() && !matches!(src[cursor], b'\'' | b'\n') {
                cursor += 1;
            }
            let end = if src.get(cursor) == Some(&b'\'') {
                cursor + 1
            } else {
                cursor.min(src.len())
            };
            Some(Span::literal(start, end, first, cursor.min(src.len())))
        }
        lead => {
            let close = first + utf8_width(lead);
            (src.get(close) == Some(&b'\'')).then(|| Span::literal(start, close + 1, first, close))
        }
    }
}

fn utf8_width(lead: u8) -> usize {
    match lead {
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        _ => 1,
    }
}

// A literal with a `b`, `c`, `r`, `br` or `cr` prefix at `at`, or None when the letter
// starts an identifier, including a raw identifier.
fn prefixed_at(src: &[u8], at: usize) -> Option<Span> {
    match (src[at], src.get(at + 1).copied()) {
        (b'b' | b'c', Some(b'"')) => Some(quoted_at(src, at, at + 1)),
        (b'b', Some(b'\'')) => char_at(src, at, at + 1),
        (b'b' | b'c', Some(b'r')) => raw_at(src, at, at + 2),
        (b'r', _) => raw_at(src, at, at + 1),
        _ => None,
    }
}

// A raw string whose hash run starts at `hashes_at`.
fn raw_at(src: &[u8], start: usize, hashes_at: usize) -> Option<Span> {
    let hashes = src[hashes_at..]
        .iter()
        .take_while(|&&byte| byte == b'#')
        .count();
    let quote = hashes_at + hashes;
    if src.get(quote) != Some(&b'"') {
        return None;
    }
    let content_start = quote + 1;
    let mut cursor = content_start;
    while cursor < src.len() {
        let closes = src[cursor] == b'"'
            && src
                .get(cursor + 1..cursor + 1 + hashes)
                .is_some_and(|run| run.iter().all(|&byte| byte == b'#'));
        if closes {
            return Some(Span::literal(
                start,
                cursor + 1 + hashes,
                content_start,
                cursor,
            ));
        }
        cursor += 1;
    }
    Some(Span::literal(start, src.len(), content_start, src.len()))
}

// Copy of `src` in which every byte of a span whose kind is not in `keep` is a space, except
// newlines. Offsets and line numbers are preserved, and the result stays valid UTF-8.
fn mask(src: &[u8], spans: &[Span], keep: &[Kind]) -> Vec<u8> {
    let mut out = src.to_vec();
    for span in spans.iter().filter(|span| !keep.contains(&span.kind)) {
        for byte in &mut out[span.start..span.end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    out
}

// One Rust source file and its scan texts: `full` is the whole file (this file's string
// literals blanked), `code` keeps only code spans.
struct RustFile {
    rel: String,
    src: Vec<u8>,
    spans: Vec<Span>,
    full: Vec<u8>,
    code: Vec<u8>,
}

impl RustFile {
    fn new(rel: String, src: Vec<u8>) -> Self {
        let spans = lex_rust(&src);
        let full = if rel == THIS_FILE {
            mask(&src, &spans, &[Kind::Code, Kind::Comment])
        } else {
            src.clone()
        };
        let code = mask(&src, &spans, &[Kind::Code]);
        Self {
            rel,
            src,
            spans,
            full,
            code,
        }
    }

    // Str spans that start inside the parentheses opened at `open` and closed at `close`.
    fn strs_between(&self, open: usize, close: usize) -> impl Iterator<Item = &Span> {
        self.spans
            .iter()
            .filter(move |span| span.kind == Kind::Str && span.start > open && span.start < close)
    }

    fn content(&self, span: &Span) -> &[u8] {
        &self.src[span.content_start..span.content_end]
    }
}

// Every .rs file under crates/, read once per test process. Each of the nine crates must
// contribute at least one file.
fn rust_files() -> &'static [RustFile] {
    static FILES: OnceLock<Vec<RustFile>> = OnceLock::new();
    FILES.get_or_init(|| {
        let crates = require_dir("crates");
        for name in CRATES {
            let dir = require_dir(&format!("crates/{name}"));
            assert!(
                !walk(&dir, |path| has_ext(path, &["rs"])).is_empty(),
                "crates/{name} holds no .rs file, so the Rust scans would pass vacuously"
            );
        }
        walk(&crates, |path| has_ext(path, &["rs"]))
            .into_iter()
            .map(|path| RustFile::new(rel(&path), read_bytes(&path)))
            .collect()
    })
}

// Index of the parenthesis that closes the one at `open`, counting only parentheses of
// `code` (where comments and literals are blanked), or the end of the text.
fn close_paren(code: &[u8], open: usize) -> usize {
    let mut depth = 0_usize;
    for (at, &byte) in code.iter().enumerate().skip(open) {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return at;
                }
            }
            _ => {}
        }
    }
    code.len()
}

// For a macro name at `at` with length `len` in `code`, the index of the opening parenthesis
// of its invocation: optional whitespace, the bang, optional whitespace, then the parenthesis.
fn macro_open(code: &[u8], at: usize, len: usize) -> Option<usize> {
    let bang = skip_ws(code, at + len);
    if code.get(bang) != Some(&b'!') {
        return None;
    }
    let open = skip_ws(code, bang + 1);
    (code.get(open) == Some(&b'(')).then_some(open)
}

// ---------------------------------------------------------------------------------------
// Rust matchers
// ---------------------------------------------------------------------------------------

// Offsets of lint suppression attributes: a hash, an optional bang, a bracket and then the
// allow or expect lint attribute, or a cfg_attr whose bracket names either of them.
// Whitespace is tolerated between every part.
fn suppression_hits(text: &[u8]) -> Vec<usize> {
    let mut hits = Vec::new();
    for at in occurrences(text, b"#") {
        let mut cursor = skip_ws(text, at + 1);
        if text.get(cursor) == Some(&b'!') {
            cursor = skip_ws(text, cursor + 1);
        }
        if text.get(cursor) != Some(&b'[') {
            continue;
        }
        let bracket = cursor;
        let name_at = skip_ws(text, bracket + 1);
        let name = word_at(text, name_at);
        let after_name = skip_ws(text, name_at + name.len());
        if text.get(after_name) != Some(&b'(') {
            continue;
        }
        let hit = match name {
            b"allow" | b"expect" => true,
            b"cfg_attr" => {
                let inner = &text[bracket..close_bracket(text, bracket)];
                names_call(inner, b"allow") || names_call(inner, b"expect")
            }
            _ => false,
        };
        if hit {
            hits.push(at);
        }
    }
    hits
}

// Whether `inner` calls `name` as a whole word, so `allowed(` is not a call of `allow`.
fn names_call(inner: &[u8], name: &[u8]) -> bool {
    find_token(inner, name, b"")
        .into_iter()
        .any(|at| inner.get(skip_ws(inner, at + name.len())) == Some(&b'('))
}

// Index just past the bracket that closes the one at `open`, or the end of the text.
fn close_bracket(text: &[u8], open: usize) -> usize {
    let mut depth = 0_usize;
    for (at, &byte) in text.iter().enumerate().skip(open) {
        match byte {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
    }
    text.len()
}

// The unchecked sqlx query functions; their macro forms check SQL at compile time.
const UNCHECKED_SQLX: [&[u8]; 6] = [
    b"query",
    b"query_as",
    b"query_scalar",
    b"query_with",
    b"query_as_with",
    b"query_scalar_with",
];

// Offsets of calls of an unchecked sqlx query function through the sqlx path, including
// the turbofish form. The macro forms, with a bang before the parenthesis, are not hits.
fn unchecked_sqlx_hits(text: &[u8]) -> Vec<usize> {
    let mut hits = Vec::new();
    for at in find_token(text, b"sqlx", b"") {
        let colons = skip_ws(text, at + 4);
        if !text[colons..].starts_with(b"::") {
            continue;
        }
        let name_at = skip_ws(text, colons + 2);
        let name = word_at(text, name_at);
        if !UNCHECKED_SQLX.contains(&name) {
            continue;
        }
        let next = skip_ws(text, name_at + name.len());
        let turbofish =
            text[next..].starts_with(b"::") && text.get(skip_ws(text, next + 2)) == Some(&b'<');
        if text.get(next) == Some(&b'(') || turbofish {
            hits.push(at);
        }
    }
    hits
}

// Offsets of f32 and f64 in `code`: the byte before is not a letter, so literal suffixes
// such as `1.0_f64` and `1.0f64` and conversions such as `to_f64` are hits, and the byte
// after is not an identifier byte.
fn float_hits(code: &[u8]) -> Vec<usize> {
    let mut hits: Vec<usize> = [b"f32", b"f64"]
        .iter()
        .flat_map(|tok| occurrences(code, tok.as_slice()))
        .filter(|&at| {
            (at == 0 || !code[at - 1].is_ascii_alphabetic())
                && code.get(at + 3).is_none_or(|&byte| !is_ident(byte))
        })
        .collect();
    hits.sort_unstable();
    hits
}

// Lower-cased whole-word tokens of `text`, each punctuation byte a token of its own.
fn sql_tokens(text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut tokens = Vec::new();
    let mut word = String::new();
    for ch in lower.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            word.push(ch);
            continue;
        }
        if !word.is_empty() {
            tokens.push(std::mem::take(&mut word));
        }
        if !ch.is_whitespace() {
            tokens.push(ch.to_string());
        }
    }
    if !word.is_empty() {
        tokens.push(word);
    }
    tokens
}

fn is_sql_word(token: &str) -> bool {
    token
        .bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
}

// Index past one SQL identifier at `at`: a word or a double-quoted name, optionally
// schema-qualified with dots.
fn sql_ident_end(tokens: &[String], at: usize) -> Option<usize> {
    let first = tokens.get(at)?;
    let mut next = if first == "\"" {
        let close = tokens[at + 1..].iter().position(|token| token == "\"")?;
        if close == 0 {
            return None;
        }
        at + 1 + close + 1
    } else if is_sql_word(first) {
        at + 1
    } else {
        return None;
    };
    while tokens.get(next).is_some_and(|token| token == ".") {
        match sql_ident_end(tokens, next + 1) {
            Some(end) => next = end,
            None => break,
        }
    }
    Some(next)
}

const DDL_VERBS: [&str; 3] = ["create", "alter", "drop"];
const DDL_MODIFIERS: [&str; 9] = [
    "or",
    "replace",
    "unique",
    "temp",
    "temporary",
    "materialized",
    "if",
    "not",
    "exists",
];
const DDL_OBJECTS: [&str; 5] = ["table", "index", "view", "trigger", "function"];

// The SQL statement shape `text` has, if any.
fn sql_shape(text: &str) -> Option<&'static str> {
    let tokens = sql_tokens(text);
    let token = |at: usize| tokens.get(at).map_or("", String::as_str);
    for (at, word) in tokens.iter().enumerate() {
        let shape = match word.as_str() {
            "select" => tokens[at + 1..]
                .iter()
                .any(|later| later == "from")
                .then_some("SELECT ... FROM"),
            "insert" => (token(at + 1) == "into").then_some("INSERT INTO"),
            "update" => sql_ident_end(&tokens, at + 1)
                .is_some_and(|end| token(end) == "set")
                .then_some("UPDATE ... SET"),
            "delete" => (token(at + 1) == "from").then_some("DELETE FROM"),
            "truncate" => {
                let target = if token(at + 1) == "table" {
                    at + 2
                } else {
                    at + 1
                };
                sql_ident_end(&tokens, target).map(|_| "TRUNCATE")
            }
            verb if DDL_VERBS.contains(&verb) => (1..=4)
                .map(|ahead| token(at + ahead))
                .take_while(|next| !next.is_empty())
                .position(|next| DDL_OBJECTS.contains(&next))
                .filter(|&skipped| {
                    (1..=skipped).all(|ahead| DDL_MODIFIERS.contains(&token(at + ahead)))
                })
                .map(|_| "DDL"),
            _ => None,
        };
        if shape.is_some() {
            return shape;
        }
    }
    None
}

// The literal of every format macro call whose first string literal has a SQL statement
// shape, with that shape.
fn format_sql_hits(file: &RustFile) -> Vec<(usize, &'static str)> {
    let mut hits = Vec::new();
    for at in find_token(&file.code, b"format", b"") {
        let Some(open) = macro_open(&file.code, at, b"format".len()) else {
            continue;
        };
        let close = close_paren(&file.code, open);
        let Some(first) = file.strs_between(open, close).next() else {
            continue;
        };
        if let Some(shape) = sql_shape(&String::from_utf8_lossy(file.content(first))) {
            hits.push((first.start, shape));
        }
    }
    hits
}

// The sqlx query macros, checked and unchecked, whose SQL may name a table.
const QUERY_MACROS: [&[u8]; 6] = [
    b"query",
    b"query_as",
    b"query_scalar",
    b"query_unchecked",
    b"query_as_unchecked",
    b"query_scalar_unchecked",
];

// The parentheses (open, close) of every query macro invocation in `file`.
fn query_macro_calls(file: &RustFile) -> Vec<(usize, usize)> {
    let mut calls: Vec<(usize, usize)> = QUERY_MACROS
        .iter()
        .flat_map(|name| {
            find_token(&file.code, name, b"")
                .into_iter()
                .filter_map(|at| macro_open(&file.code, at, name.len()))
        })
        .map(|open| (open, close_paren(&file.code, open)))
        .collect();
    calls.sort_unstable();
    calls
}

// Offsets of the whole word `transactions`, in any case, inside a string literal of a query
// macro invocation; one per literal.
fn transactions_hits(file: &RustFile) -> Vec<usize> {
    let mut hits = Vec::new();
    for (open, close) in query_macro_calls(file) {
        for span in file.strs_between(open, close) {
            let lower = file.content(span).to_ascii_lowercase();
            if let Some(&at) = find_token(&lower, b"transactions", b"").first() {
                hits.push(span.content_start + at);
            }
        }
    }
    hits
}

// Whether `rel` lies under `crates/<crate>/src/` for one of `crates`, or any crate when
// `crates` is empty.
fn in_crate_src(rel: &str, crates: &[&str]) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    matches!(parts.as_slice(), ["crates", name, "src", _, ..] if crates.is_empty() || crates.contains(name))
}

// Reports every hit `hits` finds in the Rust files `scope` accepts.
fn scan_rust(
    scope: impl Fn(&RustFile) -> bool,
    hits: impl Fn(&RustFile) -> Vec<usize>,
    rule: &str,
) -> Vec<String> {
    let mut violations = Vec::new();
    for file in rust_files().iter().filter(|file| scope(file)) {
        for at in hits(file) {
            report(&mut violations, &file.rel, &file.src, at, rule);
        }
    }
    violations
}

// The statement-PDF contract: the module takes its input as text lines and names no money
// or points type, which is what admits its f32 page geometry.
fn pdf_contract(violations: &mut Vec<String>) {
    require_file(PDF_MODULE);
    let pdf = rust_files()
        .iter()
        .find(|file| file.rel == PDF_MODULE)
        .unwrap_or_else(|| panic!("{PDF_MODULE} was not among the scanned Rust files"));
    let takes_lines = find_token(&pdf.code, b"fn", b"").into_iter().any(|at| {
        let Some(open) = pdf.code[at..].iter().position(|&byte| byte == b'(') else {
            return false;
        };
        let open = at + open;
        let params: Vec<u8> = pdf.code[open..close_paren(&pdf.code, open)]
            .iter()
            .copied()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect();
        !occurrences(&params, b"&[String]").is_empty()
    });
    if !takes_lines {
        report_line(
            violations,
            PDF_MODULE,
            &pdf.src,
            1,
            "pdf module has no fn taking &[String] text lines",
        );
    }
    for tok in [
        b"Money".as_slice(),
        b"MoneyExpr",
        b"RewardPoints",
        b"Decimal",
    ] {
        for at in find_token(&pdf.code, tok, b"") {
            report(
                violations,
                PDF_MODULE,
                &pdf.src,
                at,
                "pdf module names a money or points type",
            );
        }
    }
}

// ---------------------------------------------------------------------------------------
// Rust source checks
// ---------------------------------------------------------------------------------------

#[test]
fn no_unsafe_token() {
    let violations = scan_rust(
        |_| true,
        |file| find_token(&file.full, b"unsafe", b""),
        "unsafe token",
    );
    finish(&violations);
}

#[test]
fn no_rust_lint_suppressions() {
    let violations = scan_rust(
        |_| true,
        |file| suppression_hits(&file.full),
        "lint suppression attribute",
    );
    finish(&violations);
}

#[test]
fn only_checked_sqlx_macros() {
    let violations = scan_rust(
        |_| true,
        |file| unchecked_sqlx_hits(&file.full),
        "unchecked sqlx query function (use query! or query_as!)",
    );
    finish(&violations);
}

#[test]
fn raw_sql_only_in_testkit_src() {
    let violations = scan_rust(
        |file| !file.rel.starts_with("crates/testkit/src/"),
        |file| find_token(&file.full, b"raw_sql", b""),
        "raw_sql outside crates/testkit/src",
    );
    finish(&violations);
}

#[test]
fn no_sql_built_with_format() {
    let mut violations = Vec::new();
    for file in rust_files() {
        for (at, shape) in format_sql_hits(file) {
            let rule = format!("SQL built with format! ({shape})");
            report(&mut violations, &file.rel, &file.src, at, &rule);
        }
    }
    finish(&violations);
}

#[test]
fn transactions_table_only_in_owning_modules() {
    let persistence_macros: usize = rust_files()
        .iter()
        .filter(|file| in_crate_src(&file.rel, &["persistence"]))
        .map(|file| query_macro_calls(file).len())
        .sum();
    assert!(
        persistence_macros > 0,
        "no query macro found under crates/persistence/src, so this check would pass vacuously"
    );
    let violations = scan_rust(
        |file| in_crate_src(&file.rel, &[]) && !TRANSACTIONS_OWNERS.contains(&file.rel.as_str()),
        transactions_hits,
        "query names the transactions table (read through transaction_master)",
    );
    finish(&violations);
}

#[test]
fn no_float_in_financial_crates() {
    const FINANCIAL: [&str; 6] = [
        "domain",
        "codec",
        "persistence",
        "api",
        "batch",
        "messaging",
    ];
    let mut violations = scan_rust(
        |file| in_crate_src(&file.rel, &FINANCIAL) && file.rel != PDF_MODULE,
        |file| float_hits(&file.code),
        "floating-point type in financial code",
    );
    pdf_contract(&mut violations);
    finish(&violations);
}

// ---------------------------------------------------------------------------------------
// Minimal TOML reader
// ---------------------------------------------------------------------------------------

// A logical TOML statement: a table header, or a key-value pair whose path is the current
// table followed by the dotted key.
#[derive(Debug)]
enum TomlItem {
    Header(Vec<String>),
    KeyValue(Vec<String>, String),
}

#[derive(Debug)]
struct TomlLine {
    line: usize,
    item: TomlItem,
}

// Splits TOML text into logical statements with the line each starts on. Comments are
// removed, and values whose brackets or braces stay open continue onto later lines.
fn toml_statements(text: &str) -> Vec<(usize, String)> {
    let bytes = text.as_bytes();
    let mut statements = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut start_line = 1;
    let mut line = 1;
    let mut depth = 0_usize;
    let mut quote: Option<(u8, bool)> = None;
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        let mut take = 1;
        if let Some((open, multi)) = quote {
            if byte == b'\\' && open == b'"' {
                take = 2;
            } else if byte == open {
                let run = bytes[at..].iter().take_while(|&&next| next == open).count();
                if !multi {
                    quote = None;
                } else if run >= 3 {
                    take = run.min(5);
                    quote = None;
                }
            }
        } else {
            match byte {
                b'#' => {
                    at += bytes[at..]
                        .iter()
                        .take_while(|&&next| next != b'\n')
                        .count();
                    continue;
                }
                b'\n' if depth == 0 => {
                    push_statement(&mut statements, start_line, &current);
                    current.clear();
                    line += 1;
                    at += 1;
                    continue;
                }
                b'"' | b'\'' => {
                    let run = bytes[at..].iter().take_while(|&&next| next == byte).count();
                    let multi = run >= 3;
                    take = if multi { 3 } else { 1 };
                    quote = Some((byte, multi));
                }
                b'[' | b'{' => depth += 1,
                b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let end = (at + take).min(bytes.len());
        if current.iter().all(u8::is_ascii_whitespace) && !byte.is_ascii_whitespace() {
            start_line = line;
        }
        current.extend_from_slice(&bytes[at..end]);
        // Newlines inside the consumed bytes: those of a multi-line string or escape.
        line += bytes[at..end].split(|&next| next == b'\n').count() - 1;
        at = end;
    }
    push_statement(&mut statements, start_line, &current);
    statements
}

fn push_statement(statements: &mut Vec<(usize, String)>, line: usize, text: &[u8]) {
    let statement = String::from_utf8_lossy(text).trim().to_owned();
    if !statement.is_empty() {
        statements.push((line, statement));
    }
}

// Byte offset of the first `target` outside quotes and nested brackets or braces.
fn find_top_level(text: &str, target: u8) -> Option<usize> {
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut depth = 0_usize;
    for (at, byte) in text.bytes().enumerate() {
        if let Some(open) = quote {
            if byte == open && !escaped {
                quote = None;
            }
            escaped = open == b'"' && byte == b'\\' && !escaped;
            continue;
        }
        match byte {
            b'"' | b'\'' => quote = Some(byte),
            b'[' | b'{' => depth += 1,
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ if byte == target && depth == 0 => return Some(at),
            _ => {}
        }
    }
    None
}

fn split_top_level(text: &str, separator: u8) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut rest = text;
    while let Some(at) = find_top_level(rest, separator) {
        parts.push(&rest[..at]);
        rest = &rest[at + 1..];
    }
    parts.push(rest);
    parts
}

// Removes surrounding whitespace and one pair of matching quotes.
fn unquote(text: &str) -> &str {
    let text = text.trim();
    ['"', '\'']
        .into_iter()
        .find_map(|quote| text.strip_prefix(quote)?.strip_suffix(quote))
        .unwrap_or(text)
}

// Unquoted segments of a dotted key; dots inside quoted segments do not split.
fn key_segments(key: &str) -> Vec<String> {
    split_top_level(key, b'.')
        .into_iter()
        .map(|segment| unquote(segment).to_owned())
        .collect()
}

// Every header and key-value statement of `text`, with full key paths.
fn toml_lines(text: &str) -> Vec<TomlLine> {
    let mut table: Vec<String> = Vec::new();
    let mut lines = Vec::new();
    for (line, statement) in toml_statements(text) {
        if let Some(inner) = statement.strip_prefix('[') {
            let inner = inner.strip_suffix(']').unwrap_or(inner);
            let inner = inner
                .strip_prefix('[')
                .and_then(|nested| nested.strip_suffix(']'))
                .unwrap_or(inner);
            table = key_segments(inner);
            lines.push(TomlLine {
                line,
                item: TomlItem::Header(table.clone()),
            });
        } else if let Some(at) = find_top_level(&statement, b'=') {
            let mut path = table.clone();
            path.extend(key_segments(&statement[..at]));
            let value = statement[at + 1..].trim().to_owned();
            lines.push(TomlLine {
                line,
                item: TomlItem::KeyValue(path, value),
            });
        }
    }
    lines
}

// Entries of an inline table `{ a = "x", b = 1 }`, split shallowly; None for other values.
fn inline_table(value: &str) -> Option<Vec<(Vec<String>, String)>> {
    let body = value.trim().strip_prefix('{')?.strip_suffix('}')?;
    Some(
        split_top_level(body, b',')
            .into_iter()
            .filter_map(|entry| {
                let at = find_top_level(entry, b'=')?;
                Some((
                    key_segments(&entry[..at]),
                    entry[at + 1..].trim().to_owned(),
                ))
            })
            .collect(),
    )
}

// Expands inline tables into one (path, value) pair per leaf value.
fn flatten(path: &[String], value: &str, out: &mut Vec<(Vec<String>, String)>) {
    if let Some(entries) = inline_table(value) {
        for (key, inner) in entries {
            let mut nested = path.to_vec();
            nested.extend(key);
            flatten(&nested, &inner, out);
        }
    } else {
        out.push((path.to_vec(), value.trim().to_owned()));
    }
}

// Every leaf (line, path, value) of `text`, inline tables expanded.
fn toml_leaves(text: &str) -> Vec<(usize, Vec<String>, String)> {
    let mut leaves = Vec::new();
    for entry in toml_lines(text) {
        if let TomlItem::KeyValue(path, value) = &entry.item {
            let mut flat = Vec::new();
            flatten(path, value, &mut flat);
            leaves.extend(
                flat.into_iter()
                    .map(|(path, value)| (entry.line, path, value)),
            );
        }
    }
    leaves
}

fn has_prefix(path: &[String], prefix: &[&str]) -> bool {
    path.len() >= prefix.len()
        && path
            .iter()
            .zip(prefix)
            .all(|(segment, want)| segment == want)
}

// ---------------------------------------------------------------------------------------
// Manifest checks
// ---------------------------------------------------------------------------------------

// The manifest of crates/<name>, which must exist, with its repository-relative path.
fn crate_manifest(name: &str) -> (String, String) {
    let rel = format!("crates/{name}/Cargo.toml");
    let text = read_text(&require_file(&rel));
    (rel, text)
}

// A member manifest takes its lints only from the workspace: `[lints]` holding exactly
// `workspace = true`, or the equivalent top-level dotted or inline form.
fn member_lints(violations: &mut Vec<String>, rel: &str, text: &str) {
    let src = text.as_bytes();
    let mut inherits = false;
    for entry in toml_lines(text) {
        match &entry.item {
            TomlItem::Header(path) if has_prefix(path, &["lints"]) && path.len() > 1 => {
                report_line(violations, rel, src, entry.line, "crate-level lint table");
            }
            TomlItem::KeyValue(path, value) if has_prefix(path, &["lints"]) => {
                let mut flat = Vec::new();
                flatten(path, value, &mut flat);
                for (leaf, leaf_value) in flat {
                    if leaf == ["lints", "workspace"] && leaf_value == "true" {
                        inherits = true;
                    } else {
                        report_line(
                            violations,
                            rel,
                            src,
                            entry.line,
                            "lints entry other than workspace = true",
                        );
                    }
                }
            }
            _ => {}
        }
    }
    if !inherits {
        report_line(violations, rel, src, 1, "missing [lints] workspace = true");
    }
}

// Every `[workspace.lints]` entry of the root manifest is at deny or forbid, and the
// required gates are present.
fn workspace_lints(violations: &mut Vec<String>, rel: &str, text: &str) {
    let src = text.as_bytes();
    let mut levels: Vec<(String, String, String)> = Vec::new();
    for (line, path, value) in toml_leaves(text) {
        if !has_prefix(&path, &["workspace", "lints"]) {
            continue;
        }
        let (tool, lint, attribute) = match &path[2..] {
            [tool, lint] => (tool, lint, "level"),
            [tool, lint, attribute] => (tool, lint, attribute.as_str()),
            _ => {
                report_line(
                    violations,
                    rel,
                    src,
                    line,
                    "unrecognised workspace lint entry",
                );
                continue;
            }
        };
        match attribute {
            "level" => {}
            "priority" => continue,
            _ => {
                report_line(
                    violations,
                    rel,
                    src,
                    line,
                    "unrecognised workspace lint entry",
                );
                continue;
            }
        }
        let level = unquote(&value).to_owned();
        if level != "deny" && level != "forbid" {
            report_line(
                violations,
                rel,
                src,
                line,
                "workspace lint level is neither deny nor forbid",
            );
        }
        levels.push((tool.clone(), lint.clone(), level));
    }
    let required: [(&str, &str, &[&str]); 3] = [
        ("rust", "unsafe_code", &["forbid"]),
        ("clippy", "all", &["deny", "forbid"]),
        ("clippy", "pedantic", &["deny", "forbid"]),
    ];
    for (tool, lint, accepted) in required {
        let present = levels.iter().any(|(have_tool, have_lint, level)| {
            have_tool == tool && have_lint == lint && accepted.contains(&level.as_str())
        });
        if !present {
            let rule = format!(
                "required workspace lint {tool}.{lint} at {} is missing",
                accepted.join(" or ")
            );
            report_line(violations, rel, src, 1, &rule);
        }
    }
}

// A Cargo configuration file, when present, carries no rustflags that allow, warn or cap
// lints.
fn cargo_config_caps(violations: &mut Vec<String>) {
    for rel in [".cargo/config.toml", ".cargo/config"] {
        let path = root().join(rel);
        if !path.is_file() {
            continue;
        }
        let text = read_text(&path);
        for (line, key, value) in toml_leaves(&text) {
            let is_flags = key
                .iter()
                .any(|segment| segment.to_ascii_lowercase().contains("rustflags"));
            let caps = value
                .split(|ch: char| ch.is_whitespace() || matches!(ch, '[' | ']' | '"' | '\'' | ','))
                .any(|flag| {
                    ["--cap-lints", "-A", "-W", "--allow", "--warn"]
                        .iter()
                        .any(|cap| flag.starts_with(cap))
                });
            if is_flags && caps {
                report_line(
                    violations,
                    rel,
                    text.as_bytes(),
                    line,
                    "rustflags relax or cap lints",
                );
            }
        }
    }
}

#[test]
fn lint_configuration_is_workspace_only() {
    let mut violations = Vec::new();
    for name in CRATES {
        let (rel, text) = crate_manifest(name);
        member_lints(&mut violations, &rel, &text);
    }
    let root_manifest = read_text(&require_file("Cargo.toml"));
    workspace_lints(&mut violations, "Cargo.toml", &root_manifest);
    cargo_config_caps(&mut violations);
    finish(&violations);
}

// The dependency table a declaration sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DepKind {
    Normal,
    Dev,
    Build,
}

fn dep_kind(table: &str) -> Option<DepKind> {
    match table {
        "dependencies" => Some(DepKind::Normal),
        "dev-dependencies" | "dev_dependencies" => Some(DepKind::Dev),
        "build-dependencies" | "build_dependencies" => Some(DepKind::Build),
        _ => None,
    }
}

// The dependency table a key path addresses, whether it is target-scoped, and the path
// below the table: `[target.'cfg(unix)'.dev-dependencies]` plus `foo.workspace` gives
// (Dev, true, [foo, workspace]).
fn dep_scope(path: &[String]) -> Option<(DepKind, bool, &[String])> {
    match path {
        [target, _, table, rest @ ..] if target == "target" => {
            dep_kind(table).map(|kind| (kind, true, rest))
        }
        [table, rest @ ..] => dep_kind(table).map(|kind| (kind, false, rest)),
        [] => None,
    }
}

// One declared dependency of a manifest, with the package it resolves to.
#[derive(Debug)]
struct Dep {
    key: String,
    kind: DepKind,
    target: bool,
    line: usize,
    package: Option<String>,
    workspace: bool,
}

// The dependencies and dependency-table header lines a manifest declares. Keys of any other
// table, such as `name` under `[package]`, are never dependencies.
fn manifest_deps(text: &str) -> (Vec<Dep>, Vec<usize>) {
    let mut deps: Vec<Dep> = Vec::new();
    let mut headers = Vec::new();
    for entry in toml_lines(text) {
        let mut leaves = Vec::new();
        match &entry.item {
            TomlItem::Header(path) => match dep_scope(path) {
                Some((_, _, [])) => headers.push(entry.line),
                Some((kind, target, [name, ..])) => {
                    headers.push(entry.line);
                    dep_entry(&mut deps, name, kind, target, entry.line);
                }
                None => {}
            },
            TomlItem::KeyValue(path, value) => flatten(path, value, &mut leaves),
        }
        for (path, value) in leaves {
            let Some((kind, target, [name, attribute @ ..])) = dep_scope(&path) else {
                continue;
            };
            let dep = dep_entry(&mut deps, name, kind, target, entry.line);
            match attribute {
                [field] if field == "package" => dep.package = Some(unquote(&value).to_owned()),
                [field] if field == "workspace" => dep.workspace = value == "true",
                _ => {}
            }
        }
    }
    (deps, headers)
}

// The declaration of `name` in the given table, created on first sight.
fn dep_entry<'a>(
    deps: &'a mut Vec<Dep>,
    name: &str,
    kind: DepKind,
    target: bool,
    line: usize,
) -> &'a mut Dep {
    let at = deps
        .iter()
        .position(|dep| dep.key == name && dep.kind == kind && dep.target == target)
        .unwrap_or_else(|| {
            deps.push(Dep {
                key: name.to_owned(),
                kind,
                target,
                line,
                package: None,
                workspace: false,
            });
            deps.len() - 1
        });
    &mut deps[at]
}

// Lower case with `_` folded to `-`, so a respelled crate name compares equal.
fn canonical(name: &str) -> String {
    name.to_ascii_lowercase().replace('_', "-")
}

// The package renames of the root `[workspace.dependencies]`, keyed by canonical name.
fn workspace_renames(text: &str) -> Vec<(String, String)> {
    toml_leaves(text)
        .into_iter()
        .filter_map(|(_, path, value)| match path.as_slice() {
            [workspace, dependencies, name, field]
                if workspace == "workspace"
                    && dependencies == "dependencies"
                    && field == "package" =>
            {
                Some((canonical(name), canonical(unquote(&value))))
            }
            _ => None,
        })
        .collect()
}

// The canonical package a declaration resolves to: an explicit `package` wins, then the
// rename of the workspace entry it inherits, then its own key.
fn effective_package(dep: &Dep, renames: &[(String, String)]) -> String {
    if let Some(package) = &dep.package {
        return canonical(package);
    }
    let key = canonical(&dep.key);
    renames
        .iter()
        .find(|(name, _)| dep.workspace && *name == key)
        .map_or(key, |(_, package)| package.clone())
}

// Crates that may name carddemo-testkit, and then only as a dev-dependency.
const TESTKIT_USERS: [&str; 4] = ["persistence", "api", "batch", "messaging"];

// The crate graph rules for the crate in directory `name`; the reason for each forbidden
// dependency, or None when the package may be named.
fn edge_rule(name: &str, package: &str, kind: DepKind) -> Option<&'static str> {
    const ADAPTERS: [&str; 3] = ["carddemo-api", "carddemo-batch", "carddemo-messaging"];
    const DOMAIN_FORBIDDEN: [&str; 7] = [
        "sqlx",
        "tokio",
        "axum",
        "lapin",
        "serde",
        "serde-json",
        "printpdf",
    ];
    if package == "carddemo-testkit" {
        return if kind != DepKind::Dev {
            Some("carddemo-testkit outside dev-dependencies")
        } else if !TESTKIT_USERS.contains(&name) {
            Some("carddemo-testkit in a crate outside persistence, api, batch and messaging")
        } else {
            None
        };
    }
    let own = format!("carddemo-{name}");
    match name {
        "audit" => Some("carddemo-audit declares a dependency"),
        "domain" if package.starts_with("carddemo-") || DOMAIN_FORBIDDEN.contains(&package) => {
            Some("forbidden edge from carddemo-domain")
        }
        "codec" if matches!(package, "sqlx" | "tokio") => {
            Some("forbidden edge from carddemo-codec")
        }
        "persistence" if matches!(package, "axum" | "lapin") || ADAPTERS.contains(&package) => {
            Some("forbidden edge from carddemo-persistence")
        }
        "api" | "batch" | "messaging" if ADAPTERS.contains(&package) && package != own => {
            Some("edge between adapter crates")
        }
        _ => None,
    }
}

// Asserts that the manifest parser found `key` in a table of `kind` of crate `name`, so the
// edge rules cannot pass on a manifest the parser misread.
fn require_dep(all: &[(&str, Vec<Dep>)], name: &str, key: &str, kind: DepKind) {
    let found = all.iter().any(|(crate_name, deps)| {
        *crate_name == name && deps.iter().any(|dep| dep.key == key && dep.kind == kind)
    });
    assert!(
        found,
        "the manifest parser did not find `{key}` ({kind:?}) in crates/{name}/Cargo.toml"
    );
}

#[test]
fn forbidden_crate_edges() {
    let renames = workspace_renames(&read_text(&require_file("Cargo.toml")));
    let mut violations = Vec::new();
    let mut all = Vec::new();
    for name in CRATES {
        let (rel, text) = crate_manifest(name);
        let (deps, headers) = manifest_deps(&text);
        if name == "audit" {
            for line in headers {
                report_line(
                    &mut violations,
                    &rel,
                    text.as_bytes(),
                    line,
                    "carddemo-audit declares a dependency table",
                );
            }
        }
        for dep in &deps {
            let package = effective_package(dep, &renames);
            if let Some(rule) = edge_rule(name, &package, dep.kind) {
                let rule = format!("{rule} ({package})");
                report_line(&mut violations, &rel, text.as_bytes(), dep.line, &rule);
            }
        }
        all.push((name, deps));
    }
    require_dep(&all, "persistence", "sqlx", DepKind::Normal);
    require_dep(&all, "codec", "thiserror", DepKind::Normal);
    require_dep(&all, "codec", "carddemo-domain", DepKind::Normal);
    require_dep(&all, "domain", "rust_decimal", DepKind::Normal);
    require_dep(&all, "persistence", "carddemo-testkit", DepKind::Dev);
    finish(&violations);
}

// ---------------------------------------------------------------------------------------
// SQL and web checks
// ---------------------------------------------------------------------------------------

// Copy of SQL text with `--` comments, nested block comments and single-quoted literals
// (with doubled-quote escapes) blanked to spaces, newlines kept.
fn strip_sql(src: &[u8]) -> Vec<u8> {
    let mut out = src.to_vec();
    let mut at = 0;
    while at < src.len() {
        let end = if src[at..].starts_with(b"--") {
            at + src[at..].iter().take_while(|&&byte| byte != b'\n').count()
        } else if src[at..].starts_with(b"/*") {
            sql_block_end(src, at)
        } else if src[at] == b'\'' {
            sql_literal_end(src, at)
        } else {
            at += 1;
            continue;
        };
        for byte in &mut out[at..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
        at = end;
    }
    out
}

fn sql_block_end(src: &[u8], open: usize) -> usize {
    let mut depth = 0_usize;
    let mut at = open;
    while at < src.len() {
        if src[at..].starts_with(b"/*") {
            depth += 1;
            at += 2;
        } else if src[at..].starts_with(b"*/") {
            depth -= 1;
            at += 2;
            if depth == 0 {
                return at;
            }
        } else {
            at += 1;
        }
    }
    src.len()
}

fn sql_literal_end(src: &[u8], open: usize) -> usize {
    let mut at = open + 1;
    while at < src.len() {
        if src[at] == b'\'' {
            if src.get(at + 1) == Some(&b'\'') {
                at += 2;
                continue;
            }
            return at + 1;
        }
        at += 1;
    }
    src.len()
}

// Offsets of floating-point column types in SQL text: the whole words real, float, float4
// and float8, and double followed by whitespace and precision, in any case.
fn sql_float_hits(src: &[u8]) -> Vec<usize> {
    let lower = strip_sql(src).to_ascii_lowercase();
    let mut hits: Vec<usize> = [b"real".as_slice(), b"float", b"float4", b"float8"]
        .iter()
        .flat_map(|word| find_token(&lower, word, b""))
        .collect();
    for at in find_token(&lower, b"double", b"") {
        let next = skip_ws(&lower, at + b"double".len());
        let spaced = next > at + b"double".len();
        let precision = lower[next..].starts_with(b"precision")
            && lower
                .get(next + b"precision".len())
                .is_none_or(|&byte| !is_ident(byte));
        if spaced && precision {
            hits.push(at);
        }
    }
    hits.sort_unstable();
    hits
}

#[test]
fn no_float_in_migrations() {
    let dir = require_dir("migrations");
    let files = walk(&dir, |path| has_ext(path, &["sql"]));
    assert!(
        !files.is_empty(),
        "migrations/ holds no .sql file, so this check would pass vacuously"
    );
    let mut violations = Vec::new();
    for path in files {
        let src = read_bytes(&path);
        for at in sql_float_hits(&src) {
            report(
                &mut violations,
                &rel(&path),
                &src,
                at,
                "floating-point SQL type",
            );
        }
    }
    finish(&violations);
}

// Every file under the web directory `rel_dir` with its relative path, generated files
// included. Bytes are scanned as they are; excerpts decode them lossily. The directory must
// hold at least one file.
fn web_files(rel_dir: &str) -> Vec<(String, Vec<u8>)> {
    let dir = require_dir(rel_dir);
    let files: Vec<(String, Vec<u8>)> = walk(&dir, |_| true)
        .into_iter()
        .map(|path| (rel(&path), read_bytes(&path)))
        .collect();
    assert!(
        !files.is_empty(),
        "{rel_dir} holds no file, so the web scans would pass vacuously"
    );
    files
}

// Lint and type-check suppression directives in web sources.
const WEB_SUPPRESSIONS: [&str; 4] = [
    "eslint-disable",
    "@ts-ignore",
    "@ts-expect-error",
    "@ts-nocheck",
];

#[test]
fn no_web_lint_suppressions() {
    let mut violations = Vec::new();
    let mut files = web_files("web/src");
    files.extend(web_files("web/e2e"));
    for (path, text) in &files {
        let hits = WEB_SUPPRESSIONS
            .iter()
            .flat_map(|directive| {
                occurrences(text, directive.as_bytes())
                    .into_iter()
                    .map(move |at| (at, format!("web suppression {directive}")))
            })
            .collect();
        report_in_order(&mut violations, path, text, hits);
    }
    let vite = "web/vite.config.ts";
    let config = read_bytes(&require_file(vite));
    for at in occurrences(&config, b"chunkSizeWarningLimit") {
        report(
            &mut violations,
            vite,
            &config,
            at,
            "raising the Vite chunk size warning limit is a suppression",
        );
    }
    finish(&violations);
}

// Calls of `name` in JavaScript or TypeScript text, where `$` is an identifier byte.
fn js_calls(text: &[u8], name: &[u8]) -> Vec<usize> {
    find_call(text, name, b"$")
}

#[test]
fn no_float_parsing_in_web() {
    let mut violations = Vec::new();
    for (path, text) in &web_files("web/src") {
        let hits = ["parseFloat", "Number"]
            .iter()
            .flat_map(|name| {
                js_calls(text, name.as_bytes()).into_iter().map(move |at| {
                    (
                        at,
                        format!("{name}() in web code (keep decimals as strings)"),
                    )
                })
            })
            .collect();
        report_in_order(&mut violations, path, text, hits);
    }
    finish(&violations);
}

#[test]
fn fetch_only_in_api_client() {
    require_file(API_CLIENT);
    let mut violations = Vec::new();
    for (path, text) in &web_files("web/src") {
        if path == API_CLIENT {
            continue;
        }
        for at in js_calls(text, b"fetch") {
            report(
                &mut violations,
                path,
                text,
                at,
                "fetch() outside web/src/api/client.ts",
            );
        }
    }
    finish(&violations);
}

// ---------------------------------------------------------------------------------------
// Self-checks of the matchers
// ---------------------------------------------------------------------------------------

fn lexed(src: &str) -> Vec<(Kind, &str)> {
    lex_rust(src.as_bytes())
        .iter()
        .map(|span| (span.kind, &src[span.start..span.end]))
        .collect()
}

fn str_contents(src: &str) -> Vec<&str> {
    lex_rust(src.as_bytes())
        .iter()
        .filter(|span| span.kind == Kind::Str)
        .map(|span| &src[span.content_start..span.content_end])
        .collect()
}

#[test]
fn rust_lexer_self_check() {
    use Kind::{Code, Comment, Str};

    let raw = r###"let s = r##"a "# b"##;"###;
    assert_eq!(
        lexed(raw),
        [
            (Code, "let s = "),
            (Str, r###"r##"a "# b"##"###),
            (Code, ";")
        ]
    );
    assert_eq!(str_contents(raw), [r##"a "# b"##]);

    assert_eq!(
        lexed("a /* b /* c */ d */ e"),
        [(Code, "a "), (Comment, "/* b /* c */ d */"), (Code, " e")]
    );
    assert_eq!(
        lexed("fn f<'a>(x: &'a str) -> char { 'a' }"),
        [
            (Code, "fn f<'a>(x: &'a str) -> char { "),
            (Str, "'a'"),
            (Code, " }")
        ]
    );
    assert_eq!(
        lexed(r#"let q = '\''; let d = b'"'; let s = "x";"#),
        [
            (Code, "let q = "),
            (Str, r"'\''"),
            (Code, "; let d = "),
            (Str, r#"b'"'"#),
            (Code, "; let s = "),
            (Str, r#""x""#),
            (Code, ";")
        ]
    );
    assert_eq!(
        lexed(r#""a \" b" // c"#),
        [(Str, r#""a \" b""#), (Code, " "), (Comment, "// c")]
    );
    assert_eq!(lexed(r#""\\" x"#), [(Str, r#""\\""#), (Code, " x")]);
    assert_eq!(lexed("let r#type = 1;"), [(Code, "let r#type = 1;")]);
    assert_eq!(lexed("let br = rb + cr;"), [(Code, "let br = rb + cr;")]);

    let prefixed = r###"br##"a"#b"## c"x" b"y""###;
    assert_eq!(
        lexed(prefixed),
        [
            (Str, r###"br##"a"#b"##"###),
            (Code, " "),
            (Str, r#"c"x""#),
            (Code, " "),
            (Str, r#"b"y""#)
        ]
    );
    assert_eq!(str_contents(prefixed), [r##"a"#b"##, "x", "y"]);

    assert_eq!(
        lexed("/// d\n//! e\nfn g(s: &'static str) { 'outer: loop { break 'outer; } }"),
        [
            (Comment, "/// d"),
            (Code, "\n"),
            (Comment, "//! e"),
            (
                Code,
                "\nfn g(s: &'static str) { 'outer: loop { break 'outer; } }"
            )
        ]
    );
    assert_eq!(
        lexed("let e = '\u{e9}';"),
        [(Code, "let e = "), (Str, "'\u{e9}'"), (Code, ";")]
    );

    let src = "x \"a\nb\" y // z";
    let spans = lex_rust(src.as_bytes());
    assert_eq!(mask(src.as_bytes(), &spans, &[Code]), b"x   \n   y     ");
    assert_eq!(
        mask(src.as_bytes(), &spans, &[Code, Comment]),
        b"x   \n   y // z"
    );
}

#[test]
fn suppression_matcher_self_check() {
    for positive in [
        "#[allow(dead_code)]",
        "# ! [ allow ( unused ) ]",
        "#![expect(clippy::pedantic)]",
        "#[ expect (x)]",
        "#\n[\nallow\n(x)]",
        "#[cfg_attr(test, allow(x))]",
        "#![cfg_attr(feature = \"f\", expect(y))]",
        "# [cfg_attr (all(), allow (z))]",
    ] {
        assert_eq!(
            suppression_hits(positive.as_bytes()),
            [0],
            "not detected: {positive}"
        );
    }
    for negative in [
        "#[should_panic(expected = \"boom\")]",
        "#[allowed(x)]",
        "#[derive(Debug, Clone)]",
        "#[cfg_attr(test, derive(Debug))]",
        "#[cfg_attr(test, allowed(x))]",
        "#[cfg_attr(test, doc = \"x\")] fn f() {} allow(y)",
        "#[doc = \"allow(x)\"]",
        "#[test]",
        "#[allow]",
        "allow(x) expect(y)",
    ] {
        assert!(
            suppression_hits(negative.as_bytes()).is_empty(),
            "false positive: {negative}"
        );
    }
}

#[test]
fn rust_matchers_self_check() {
    for positive in [
        "sqlx::query(\"x\")",
        "sqlx :: query_as ( \"x\" )",
        "sqlx::query_scalar_with(\"x\", args)",
        "sqlx::query_as::<_, Row>(\"x\")",
        "::sqlx::query_with(q, a)",
    ] {
        assert_eq!(
            unchecked_sqlx_hits(positive.as_bytes()).len(),
            1,
            "{positive}"
        );
    }
    for negative in [
        "sqlx::query!(\"x\")",
        "sqlx::query_as!(Row, \"x\")",
        "sqlx::query_file!(\"q.sql\")",
        "sqlx::QueryBuilder::new(\"x\")",
        "mysqlx::query(\"x\")",
        "sqlx::queryx(\"x\")",
    ] {
        assert!(
            unchecked_sqlx_hits(negative.as_bytes()).is_empty(),
            "{negative}"
        );
    }

    for positive in [
        "let x: f64 = 1.0;",
        "y as f32",
        "1.0_f64",
        "1.0f64",
        "d.to_f64()",
        "f32::from(r)",
    ] {
        assert_eq!(float_hits(positive.as_bytes()).len(), 1, "{positive}");
    }
    for negative in ["xf64", "f64x", "let f6 = 4;", "F64", "rf32"] {
        assert!(float_hits(negative.as_bytes()).is_empty(), "{negative}");
    }

    let formats = RustFile::new(
        "crates/x/src/a.rs".to_owned(),
        concat!(
            "let a = format!(\"SELECT a FROM t WHERE id = {id}\");\n",
            "let b = format!( /* ( */ \"{path}:{line}\" );\n",
            "let c = format ! (\"delete from {t}\");\n",
            "let d = format!(\"{}\", \"select x from y\");\n",
            "// format!(\"insert into t\")\n",
        )
        .as_bytes()
        .to_vec(),
    );
    let shapes: Vec<(usize, &str)> = format_sql_hits(&formats)
        .into_iter()
        .map(|(at, shape)| (line_of(&formats.src, at), shape))
        .collect();
    assert_eq!(shapes, [(1, "SELECT ... FROM"), (3, "DELETE FROM")]);

    let queries = RustFile::new(
        "crates/x/src/b.rs".to_owned(),
        concat!(
            "sqlx::query!(\"SELECT * FROM transactions\");\n",
            "sqlx::query_as!(Row, r#\"select x from public.transactions\"#);\n",
            "query_scalar!(\"SELECT 1 FROM transaction_master WHERE transaction_id = 1\");\n",
            "let s = \"transactions\";\n",
            "query!( \"x\" /* ) */ , \"TRANSACTIONS\");\n",
        )
        .as_bytes()
        .to_vec(),
    );
    assert_eq!(query_macro_calls(&queries).len(), 4);
    let lines: Vec<usize> = transactions_hits(&queries)
        .into_iter()
        .map(|at| line_of(&queries.src, at))
        .collect();
    assert_eq!(lines, [1, 2, 5]);
}

#[test]
fn sql_shape_self_check() {
    for (positive, shape) in [
        ("SELECT a FROM b", "SELECT ... FROM"),
        ("insert into t (x)", "INSERT INTO"),
        ("UPDATE accounts SET x = 1", "UPDATE ... SET"),
        ("update public.\"accounts\" set x = 1", "UPDATE ... SET"),
        ("delete from t", "DELETE FROM"),
        ("CREATE OR REPLACE FUNCTION f()", "DDL"),
        ("create unique index i on t", "DDL"),
        ("DROP TABLE IF EXISTS t", "DDL"),
        ("ALTER TABLE t ADD c int", "DDL"),
        ("TRUNCATE TABLE t", "TRUNCATE"),
    ] {
        assert_eq!(sql_shape(positive), Some(shape), "{positive}");
    }
    for negative in [
        "Unable to Update Account...",
        "Please delete associated child records first:",
        "Enter or update id of account to update",
        "PLEASE SELECT ONLY ONE RECORD TO VIEW OR UPDATE",
        "Delete failed with message:",
        "{path}:{line}: rule: excerpt",
        "create the account table",
        "drop a b c d table",
    ] {
        assert_eq!(sql_shape(negative), None, "{negative}");
    }
}

#[test]
fn text_matchers_self_check() {
    let text = b"fetch(a); window.fetch(b); globalThis.fetch (c); prefetch(d); refetch(e); \
                 fetchQuery(f); $fetch(g); _fetch(h)";
    assert_eq!(js_calls(text, b"fetch").len(), 3);

    let numbers = b"Number(a); Number (b); Number.parseFloat(c); parseFloat(d); \
                    Number.isInteger(e); toNumber(f); BigNumber(g); $Number(h)";
    assert_eq!(js_calls(numbers, b"Number").len(), 2);
    assert_eq!(js_calls(numbers, b"parseFloat").len(), 2);

    let sql = concat!(
        "amount REAL,\n",
        "rate Float8, ratio float4, f FLOAT,\n",
        "x double\n  precision,\n",
        "-- real float\n",
        "/* double precision /* nested real */ float */\n",
        "note text DEFAULT 'real ''float'' double precision',\n",
        "is_real boolean, doubled precision_x int, double precisionx int\n",
    );
    let lines: Vec<usize> = sql_float_hits(sql.as_bytes())
        .into_iter()
        .map(|at| line_of(sql.as_bytes(), at))
        .collect();
    assert_eq!(lines, [1, 2, 2, 2, 3]);
}

#[test]
fn toml_lint_rules_self_check() {
    let member = |text: &str| {
        let mut violations = Vec::new();
        member_lints(&mut violations, "m/Cargo.toml", text);
        violations.len()
    };
    assert_eq!(
        member("[package]\nname = \"x\"\n\n[lints]\nworkspace = true\n"),
        0
    );
    assert_eq!(
        member("lints.workspace = true\n[package]\nname = \"x\"\n"),
        0
    );
    assert_eq!(member("lints = { workspace = true }\n"), 0);
    assert_eq!(member("[lints.clippy]\npedantic = \"allow\"\n"), 3);
    assert_eq!(
        member("[lints]\nworkspace = true\nrust.unsafe_code = \"allow\"\n"),
        1
    );
    assert_eq!(member("[package]\nname = \"x\"\n"), 1);
    assert_eq!(member("[lints]\nworkspace = false\n"), 2);

    let workspace = |text: &str| {
        let mut violations = Vec::new();
        workspace_lints(&mut violations, "Cargo.toml", text);
        violations
    };
    let grouped = "[workspace.lints.rust]\nunsafe_code = \"forbid\"\n\n[workspace.lints.clippy]\n\
                   all = { level = \"deny\", priority = -1 }\npedantic = { level = \"deny\", priority = -1 }\n";
    assert!(workspace(grouped).is_empty());
    let dotted = "[workspace.lints]\nrust.unsafe_code = \"forbid\"\n\
                  clippy.all = { level = \"deny\", priority = -1 }\nclippy.pedantic = \"deny\"\n";
    assert!(workspace(dotted).is_empty());
    let relaxed = format!("{grouped}module_name_repetitions = \"allow\"\n");
    assert_eq!(
        workspace(&relaxed),
        [
            "Cargo.toml:7: workspace lint level is neither deny nor forbid: module_name_repetitions = \"allow\""
        ]
    );
    assert_eq!(
        workspace("[workspace.lints.clippy]\nall = \"warn\"\n").len(),
        4
    );
}

#[test]
fn toml_dependency_reader_self_check() {
    const SAMPLE: &str = r#"[package]
name = "carddemo-x"
description = """
[dependencies]
serde = "1"
"""

[dependencies]   # trailing comment
sqlx.workspace = true
chrono = { workspace = true, features = [
    "serde",  # a feature name, not a dependency
] }
renamed = { package = "lapin", version = "2" }
aliased.workspace = true

[dev-dependencies]
carddemo-testkit = { path = "../testkit" }

[target.'cfg(unix)'.build-dependencies]
tokio = "1"

[dependencies.foo]
package = "serde_json"

[features]
default = ["dep:serde"]

[[bench]]
name = "carddemo-bench"
"#;
    let renames = workspace_renames(
        "[workspace.dependencies]\naliased = { package = \"printpdf\", version = \"0.7\" }\n",
    );
    let (deps, headers) = manifest_deps(SAMPLE);
    let resolved: Vec<(&str, DepKind, bool, usize, String)> = deps
        .iter()
        .map(|dep| {
            let package = effective_package(dep, &renames);
            (dep.key.as_str(), dep.kind, dep.target, dep.line, package)
        })
        .collect();
    let expected = [
        ("sqlx", DepKind::Normal, false, 9, "sqlx"),
        ("chrono", DepKind::Normal, false, 10, "chrono"),
        ("renamed", DepKind::Normal, false, 13, "lapin"),
        ("aliased", DepKind::Normal, false, 14, "printpdf"),
        (
            "carddemo-testkit",
            DepKind::Dev,
            false,
            17,
            "carddemo-testkit",
        ),
        ("tokio", DepKind::Build, true, 20, "tokio"),
        ("foo", DepKind::Normal, false, 22, "serde-json"),
    ];
    assert_eq!(resolved.len(), expected.len(), "{resolved:?}");
    for (got, want) in resolved.iter().zip(expected) {
        assert_eq!((got.0, got.1, got.2, got.3, got.4.as_str()), want);
    }
    assert_eq!(headers, [8, 16, 19, 22]);

    assert!(edge_rule("domain", "serde-json", DepKind::Normal).is_some());
    assert!(edge_rule("domain", "carddemo-codec", DepKind::Dev).is_some());
    assert!(edge_rule("domain", "rust-decimal", DepKind::Normal).is_none());
    assert!(edge_rule("codec", "tokio", DepKind::Dev).is_some());
    assert!(edge_rule("codec", "carddemo-domain", DepKind::Normal).is_none());
    assert!(edge_rule("persistence", "carddemo-api", DepKind::Normal).is_some());
    assert!(edge_rule("persistence", "lapin", DepKind::Dev).is_some());
    assert!(edge_rule("api", "carddemo-batch", DepKind::Dev).is_some());
    assert!(edge_rule("messaging", "carddemo-api", DepKind::Build).is_some());
    assert!(edge_rule("api", "carddemo-persistence", DepKind::Normal).is_none());
    assert!(edge_rule("api", "carddemo-testkit", DepKind::Normal).is_some());
    assert!(edge_rule("runtime", "carddemo-testkit", DepKind::Dev).is_some());
    assert!(edge_rule("batch", "carddemo-testkit", DepKind::Dev).is_none());
    assert!(edge_rule("audit", "thiserror", DepKind::Normal).is_some());
}
