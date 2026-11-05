// Static hygiene and layering gates of the CardDemo repository.
//
// Every test reads repository text with std only and fails on each violation it finds,
// listed one per line as `path:line: rule: excerpt`. No service is needed, so the suite
// runs on a machine without Docker. The gates:
//
// - Rust sources under crates/: the keyword that opts out of memory safety, lint
//   suppression attributes, the unchecked sqlx query functions and macros with the imports
//   and crate renames that hide them, the raw SQL entry point of sqlx outside the testkit
//   library, SQL statements built with the format macro, and the legacy `transactions`
//   table named outside its two owning modules. Matchers skip comments between tokens.
// - Financial and reward crates: no floating-point type, except the printpdf page geometry
//   of crates/batch/src/pdf.rs, whose input is formatted text lines only.
// - Manifests and build flags: lints come only from the workspace table at deny or forbid;
//   no Cargo configuration file (in the repository root, an ancestor directory or the
//   Cargo home), no environment variable of the RUSTFLAGS or RUSTDOCFLAGS family and no
//   argument file either names relaxes or caps them; the crate graph keeps its forbidden
//   edges absent; and the sqlx package is never renamed. Manifests and configuration files
//   are read with the std-only TOML reader this file shares by source with
//   crates/domain/tests/manifest_boundary.rs.
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

// Index of the first byte at or after `at` that is not ASCII whitespace, for text whose
// comments are already blanked, such as stripped SQL.
fn skip_spaces(text: &[u8], mut at: usize) -> usize {
    while text.get(at).is_some_and(u8::is_ascii_whitespace) {
        at += 1;
    }
    at
}

// Index of the first byte at or after `at` that is not Rust trivia: the whitespace the Rust
// lexer accepts, line comments and nested block comments.
fn skip_ws(text: &[u8], at: usize) -> usize {
    skip_trivia(text, at, is_rust_space, rust_comment_end)
}

// Index of the first byte at or after `at` that is not JavaScript or TypeScript trivia:
// white space, line terminators, line comments and block comments, which do not nest.
fn skip_js_trivia(text: &[u8], at: usize) -> usize {
    skip_trivia(text, at, is_js_space, js_comment_end)
}

// Index of the first byte at or after `at` that starts neither a character `is_space`
// accepts nor a comment, whose end `comment_end` gives.
fn skip_trivia(
    text: &[u8],
    mut at: usize,
    is_space: fn(char) -> bool,
    comment_end: fn(&[u8], usize) -> Option<usize>,
) -> usize {
    loop {
        let space = space_len(text, at, is_space);
        if space > 0 {
            at += space;
        } else if let Some(end) = comment_end(text, at) {
            at = end;
        } else {
            return at;
        }
    }
}

// Byte length of the character at `at` when `is_space` accepts it, else 0. A byte sequence
// that is not UTF-8 is never a space.
fn space_len(text: &[u8], at: usize, is_space: fn(char) -> bool) -> usize {
    let Some(&lead) = text.get(at) else {
        return 0;
    };
    text.get(at..at + utf8_width(lead))
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|decoded| decoded.chars().next())
        .filter(|&ch| is_space(ch))
        .map_or(0, char::len_utf8)
}

// Rust whitespace: the Pattern_White_Space characters.
fn is_rust_space(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n'
            | '\u{B}'
            | '\u{C}'
            | '\r'
            | ' '
            | '\u{85}'
            | '\u{200E}'
            | '\u{200F}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

// JavaScript white space and line terminators.
fn is_js_space(ch: char) -> bool {
    ('\u{2000}'..='\u{200A}').contains(&ch)
        || matches!(
            ch,
            '\t' | '\n'
                | '\u{B}'
                | '\u{C}'
                | '\r'
                | ' '
                | '\u{A0}'
                | '\u{1680}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
        )
}

// End of the Rust comment that starts at `at`, if one does.
fn rust_comment_end(text: &[u8], at: usize) -> Option<usize> {
    if text.get(at) != Some(&b'/') {
        return None;
    }
    comment_at(text, at).map(|comment| comment.end)
}

// End of the JavaScript or TypeScript comment that starts at `at`, if one does: a line
// comment runs to the next line terminator and a block comment to its first close.
fn js_comment_end(text: &[u8], at: usize) -> Option<usize> {
    let rest = text.get(at..)?;
    if rest.starts_with(b"//") {
        let terminators = [
            b"\n".as_slice(),
            b"\r",
            "\u{2028}".as_bytes(),
            "\u{2029}".as_bytes(),
        ];
        let len = (2..rest.len())
            .find(|&from| terminators.iter().any(|end| rest[from..].starts_with(end)))
            .unwrap_or(rest.len());
        Some(at + len)
    } else if rest.starts_with(b"/*") {
        let close = rest[2..]
            .windows(2)
            .position(|pair| pair == b"*/")
            .map_or(rest.len(), |from| from + 4);
        Some(at + close)
    } else {
        None
    }
}

// The identifier that starts at `at`, empty when none does.
fn word_at(text: &[u8], at: usize) -> &[u8] {
    let rest = text.get(at..).unwrap_or_default();
    let len = rest.iter().take_while(|&&byte| is_ident(byte)).count();
    &rest[..len]
}

// Calls of `name` in JavaScript or TypeScript text: a left identifier boundary, then
// optional trivia and an opening parenthesis.
fn find_call(hay: &[u8], name: &[u8], extra_ident: &[u8]) -> Vec<usize> {
    find_left_token(hay, name, extra_ident)
        .into_iter()
        .filter(|&at| hay.get(skip_js_trivia(hay, at + name.len())) == Some(&b'('))
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

    // Str spans that start strictly between the delimiters or commas at `open` and `close`.
    fn strs_between(&self, open: usize, close: usize) -> impl Iterator<Item = &Span> {
        self.spans
            .iter()
            .filter(move |span| span.kind == Kind::Str && span.start > open && span.start < close)
    }

    // The values of the literals that start between `open` and `close`, decoded and joined in
    // source order, as the compiler sees them.
    fn value(&self, open: usize, close: usize) -> LitValue {
        let mut value = LitValue::default();
        for span in self.strs_between(open, close) {
            push_literal(&self.src, span, &mut value);
        }
        value
    }
}

// A string value assembled from literals: its bytes, and for each byte the source offset of
// the character or escape it was decoded from, so a hit inside the value keeps its line.
#[derive(Default)]
struct LitValue {
    bytes: Vec<u8>,
    offsets: Vec<usize>,
}

// Appends the value of the literal `span` of `src` to `value`. A raw literal, whose prefix
// holds an r, is taken verbatim; a cooked literal has its escapes decoded.
fn push_literal(src: &[u8], span: &Span, value: &mut LitValue) {
    let base = span.content_start;
    let content = &src[base..span.content_end];
    if src[span.start..base].contains(&b'r') {
        value.bytes.extend_from_slice(content);
        value.offsets.extend(base..span.content_end);
        return;
    }
    let mut at = 0;
    while at < content.len() {
        let (bytes, used) = if content[at] == b'\\' {
            escape_at(content, at)
        } else {
            (vec![content[at]], 1)
        };
        value.bytes.extend_from_slice(&bytes);
        value
            .offsets
            .extend(std::iter::repeat_n(base + at, bytes.len()));
        at += used;
    }
}

// The decoded bytes of the escape whose backslash is at `at` in cooked literal content, and
// the number of content bytes it spans. A backslash before a line break spans the break and
// the whitespace after it and decodes to nothing. An escape this reader does not know keeps
// its backslash as a plain byte.
fn escape_at(content: &[u8], at: usize) -> (Vec<u8>, usize) {
    let plain = || (vec![b'\\'], 1);
    let Some(&kind) = content.get(at + 1) else {
        return plain();
    };
    match kind {
        b'n' => (vec![b'\n'], 2),
        b'r' => (vec![b'\r'], 2),
        b't' => (vec![b'\t'], 2),
        b'0' => (vec![0], 2),
        b'\\' | b'\'' | b'"' => (vec![kind], 2),
        b'\n' | b'\r' => {
            let skipped = content[at + 1..]
                .iter()
                .take_while(|&&byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
                .count();
            (Vec::new(), 1 + skipped)
        }
        b'x' => hex_escape(content, at).unwrap_or_else(plain),
        b'u' => unicode_escape(content, at).unwrap_or_else(plain),
        _ => plain(),
    }
}

// The byte of the two-digit hexadecimal escape at `at`, and its length of four bytes.
fn hex_escape(content: &[u8], at: usize) -> Option<(Vec<u8>, usize)> {
    let digits = content
        .get(at + 2..at + 4)
        .filter(|digits| digits.iter().all(u8::is_ascii_hexdigit))?;
    let byte = u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
    Some((vec![byte], 4))
}

// The UTF-8 bytes of the braced Unicode escape at `at`, whose one to six hexadecimal digits
// may be separated by underscores, and its length up to the closing brace.
fn unicode_escape(content: &[u8], at: usize) -> Option<(Vec<u8>, usize)> {
    if content.get(at + 2) != Some(&b'{') {
        return None;
    }
    let body = at + 3;
    let close = body + content.get(body..)?.iter().position(|&byte| byte == b'}')?;
    let digits: Vec<u8> = content[body..close]
        .iter()
        .copied()
        .filter(|&byte| byte != b'_')
        .collect();
    if digits.is_empty() || digits.len() > 6 || !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let code = u32::from_str_radix(std::str::from_utf8(&digits).ok()?, 16).ok()?;
    let decoded = char::from_u32(code)?;
    Some((decoded.to_string().into_bytes(), close + 1 - at))
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

// Index of the delimiter that closes the parenthesis, bracket or brace at `open`, counting
// only that delimiter's own pair in `code` (where comments and literals are blanked), or the
// end of the text when it never closes or `open` holds no opening delimiter.
fn close_delim(code: &[u8], open: usize) -> usize {
    let Some(&opener) = code.get(open) else {
        return code.len();
    };
    let closer = match opener {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        _ => return code.len(),
    };
    let mut depth = 0_usize;
    for (at, &byte) in code.iter().enumerate().skip(open) {
        if byte == opener {
            depth += 1;
        } else if byte == closer {
            depth -= 1;
            if depth == 0 {
                return at;
            }
        }
    }
    code.len()
}

// Index of the parenthesis that closes the one at `open`, counting only parentheses of
// `code` (where comments and literals are blanked), or the end of the text.
fn close_paren(code: &[u8], open: usize) -> usize {
    close_delim(code, open)
}

// For a macro name at `at` with length `len` in `code`, the index of the opening delimiter
// of its invocation: optional whitespace, the bang, optional whitespace, then a parenthesis,
// a bracket or a brace, the three delimiters a macro invocation may use.
fn macro_open(code: &[u8], at: usize, len: usize) -> Option<usize> {
    let bang = skip_ws(code, at + len);
    if code.get(bang) != Some(&b'!') {
        return None;
    }
    let open = skip_ws(code, bang + 1);
    matches!(code.get(open), Some(b'(' | b'[' | b'{')).then_some(open)
}

// ---------------------------------------------------------------------------------------
// Rust matchers
// ---------------------------------------------------------------------------------------

// Offsets of lint suppression attributes: a hash, an optional bang, a bracket and then the
// allow or expect lint attribute, or a cfg_attr whose bracket names either of them. Each
// attribute name may be spelled as a raw identifier, and Rust trivia, whitespace and
// comments, is tolerated between every part.
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
        let (name, name_end) = ident_at(text, skip_ws(text, bracket + 1));
        let after_name = skip_ws(text, name_end);
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
// Brackets inside comments and literals do not count.
fn close_bracket(text: &[u8], open: usize) -> usize {
    let mut depth = 0_usize;
    let mut at = open;
    while at < text.len() {
        if let Some(span) = literal_at(text, at) {
            at = span.end;
            continue;
        }
        match text[at] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
        at += 1;
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

// The sqlx query macros that skip the compile-time check of their SQL and its types.
const UNCHECKED_MACROS: [&[u8]; 6] = [
    b"query_unchecked",
    b"query_as_unchecked",
    b"query_scalar_unchecked",
    b"query_file_unchecked",
    b"query_file_as_unchecked",
    b"query_file_scalar_unchecked",
];

// Rules of the compile-time SQL gate.
const SQLX_CALL: &str = "unchecked sqlx query function (use query! or query_as!)";
const SQLX_MACRO: &str = "unchecked sqlx query macro (use query! or query_as!)";
const SQLX_VALUE: &str = "unchecked sqlx query function used as a value";
const SQLX_IMPORT: &str = "unchecked sqlx query function imported by name";
const SQLX_GLOB: &str = "glob import of the sqlx crate root";
const SQLX_ALIAS: &str = "sqlx crate renamed, which hides its query functions from this check";
const SQLX_DEP_RENAME: &str =
    "sqlx package renamed in a manifest, which hides its query functions from this check";

// Offsets of calls of an unchecked sqlx query function through the sqlx path, including
// the turbofish form and a raw identifier, with Rust trivia between the tokens. The macro
// forms, with a bang before the parenthesis, are not hits.
fn unchecked_sqlx_hits(text: &[u8]) -> Vec<usize> {
    let mut hits = Vec::new();
    for at in find_token(text, b"sqlx", b"") {
        let Some(next) = after_sqlx_fn(text, at) else {
            continue;
        };
        let turbofish =
            text[next..].starts_with(b"::") && text.get(skip_ws(text, next + 2)) == Some(&b'<');
        if text.get(next) == Some(&b'(') || turbofish {
            hits.push(at);
        }
    }
    hits
}

// For the token `sqlx` at `at`, the index of the first byte after the trivia that follows
// an unchecked query function named through it, or None when no such function follows.
fn after_sqlx_fn(text: &[u8], at: usize) -> Option<usize> {
    let colons = skip_ws(text, at + b"sqlx".len());
    if !text[colons..].starts_with(b"::") {
        return None;
    }
    let (name, end) = ident_at(text, skip_ws(text, colons + 2));
    UNCHECKED_SQLX.contains(&name).then(|| skip_ws(text, end))
}

// The identifier at `at` with any raw-identifier prefix removed, and the index past it; the
// name is empty when no identifier starts there.
fn ident_at(text: &[u8], at: usize) -> (&[u8], usize) {
    let raw = text.get(at..).is_some_and(|rest| rest.starts_with(b"r#"));
    let start = if raw && !word_at(text, at + 2).is_empty() {
        at + 2
    } else {
        at
    };
    let name = word_at(text, start);
    (name, start + name.len())
}

// Offsets of the unchecked sqlx query macro names as whole identifiers, wherever they
// appear: invoked, imported or re-exported.
fn unchecked_macro_hits(text: &[u8]) -> Vec<usize> {
    let mut hits: Vec<usize> = UNCHECKED_MACROS
        .iter()
        .flat_map(|name| find_token(text, name, b""))
        .collect();
    hits.sort_unstable();
    hits
}

// Offsets in `code` of an unchecked sqlx query function named through the sqlx path as a
// value, such as a closure argument: followed by neither a call, a bang nor a path
// separator. Paths inside use declarations are left to the import check.
fn sqlx_fn_value_hits(code: &[u8]) -> Vec<usize> {
    let imports: Vec<(usize, usize)> = find_token(code, b"use", b"")
        .into_iter()
        .map(|at| (at, use_leaves(code, at).1))
        .collect();
    find_token(code, b"sqlx", b"")
        .into_iter()
        .filter(|&at| !imports.iter().any(|&(start, end)| start < at && at < end))
        .filter(|&at| {
            after_sqlx_fn(code, at).is_some_and(|next| {
                !matches!(code.get(next), Some(b'(' | b'!')) && !code[next..].starts_with(b"::")
            })
        })
        .collect()
}

// One import of a use declaration: its path without a leading `::` and with a trailing
// `self` folded into its parent, the name it binds when renamed, whether it is a glob, and
// the offset of its last segment or glob.
struct UseLeaf<'a> {
    path: Vec<&'a [u8]>,
    rename: Option<&'a [u8]>,
    glob: bool,
    at: usize,
}

// The imports of the use declaration whose keyword sits at `at` in `code`, where comments
// and literals are blanked, and the index past its tree. Reading stops at the first token a
// use tree cannot hold and keeps the imports read so far, so the keyword in another role,
// such as a precise-capturing bound, yields none.
fn use_leaves(code: &[u8], at: usize) -> (Vec<UseLeaf<'_>>, usize) {
    let mut leaves = Vec::new();
    let start = skip_ws(code, at + b"use".len());
    let end = use_tree(code, start, &[], &mut leaves).unwrap_or(start);
    (leaves, end)
}

// Reads the use tree at `at` below `prefix` into `leaves`; the index past it and its
// trailing trivia, or None at a token no use tree holds.
fn use_tree<'a>(
    code: &'a [u8],
    at: usize,
    prefix: &[&'a [u8]],
    leaves: &mut Vec<UseLeaf<'a>>,
) -> Option<usize> {
    let mut path = prefix.to_vec();
    let mut at = if code[at..].starts_with(b"::") {
        skip_ws(code, at + 2)
    } else {
        at
    };
    loop {
        match code.get(at) {
            Some(b'*') => {
                leaves.push(UseLeaf {
                    path,
                    rename: None,
                    glob: true,
                    at,
                });
                return Some(skip_ws(code, at + 1));
            }
            Some(b'{') => return use_group(code, at, &path, leaves),
            _ => {}
        }
        let (segment, end) = segment_at(code, at);
        if segment.is_empty() {
            return None;
        }
        let next = skip_ws(code, end);
        if code[next..].starts_with(b"::") {
            path.push(segment);
            at = skip_ws(code, next + 2);
            continue;
        }
        if segment != b"self" || path.is_empty() {
            path.push(segment);
        }
        let mut leaf = UseLeaf {
            path,
            rename: None,
            glob: false,
            at,
        };
        if word_at(code, next) != b"as" {
            leaves.push(leaf);
            return Some(next);
        }
        let (name, name_end) = ident_at(code, skip_ws(code, next + b"as".len()));
        if name.is_empty() {
            return None;
        }
        leaf.rename = Some(name);
        leaves.push(leaf);
        return Some(skip_ws(code, name_end));
    }
}

// Reads the brace group of use trees opened at `open` below `prefix`; the index past its
// close and the trivia after it, or None at a token no group holds.
fn use_group<'a>(
    code: &'a [u8],
    open: usize,
    prefix: &[&'a [u8]],
    leaves: &mut Vec<UseLeaf<'a>>,
) -> Option<usize> {
    let mut at = skip_ws(code, open + 1);
    loop {
        if code.get(at) == Some(&b'}') {
            return Some(skip_ws(code, at + 1));
        }
        at = use_tree(code, at, prefix, leaves)?;
        match code.get(at) {
            Some(b',') => at = skip_ws(code, at + 1),
            Some(b'}') => return Some(skip_ws(code, at + 1)),
            _ => return None,
        }
    }
}

// The path segment at `at`, an identifier without any raw prefix or a macro metavariable
// such as `$crate`, and the index past it; the segment is empty when none starts there.
fn segment_at(code: &[u8], at: usize) -> (&[u8], usize) {
    if code.get(at) != Some(&b'$') {
        return ident_at(code, at);
    }
    let end = at + 1 + word_at(code, at + 1).len();
    if end == at + 1 {
        (&[], at)
    } else {
        (&code[at..end], end)
    }
}

// Offsets and rules of the declarations in `code` that let an unchecked sqlx query function
// be called without the sqlx path: the function imported by name, a glob import of the sqlx
// root, or the crate renamed by a use or extern crate declaration.
fn sqlx_import_hits(code: &[u8]) -> Vec<(usize, &'static str)> {
    let mut hits = Vec::new();
    for at in find_token(code, b"use", b"") {
        for leaf in use_leaves(code, at).0 {
            let binds = leaf.rename.is_none_or(|name| name != b"_");
            let renamed = leaf
                .rename
                .is_some_and(|name| name != b"_" && name != b"sqlx");
            let rule = match leaf.path.as_slice() {
                [.., root, name]
                    if *root == b"sqlx" && UNCHECKED_SQLX.contains(name) && !leaf.glob && binds =>
                {
                    Some(SQLX_IMPORT)
                }
                [.., root] if *root == b"sqlx" && leaf.glob => Some(SQLX_GLOB),
                [.., root] if *root == b"sqlx" && renamed => Some(SQLX_ALIAS),
                _ => None,
            };
            if let Some(rule) = rule {
                hits.push((leaf.at, rule));
            }
        }
    }
    for at in find_token(code, b"extern", b"") {
        let crate_at = skip_ws(code, at + b"extern".len());
        if word_at(code, crate_at) != b"crate" {
            continue;
        }
        let (name, end) = ident_at(code, skip_ws(code, crate_at + b"crate".len()));
        let as_at = skip_ws(code, end);
        if name != b"sqlx" || word_at(code, as_at) != b"as" {
            continue;
        }
        let (alias, _) = ident_at(code, skip_ws(code, as_at + b"as".len()));
        if !alias.is_empty() && alias != b"_" && alias != b"sqlx" {
            hits.push((at, SQLX_ALIAS));
        }
    }
    hits
}

// Every way `file` reaches SQL that sqlx does not check at compile time, as (offset, rule)
// in source order: calls and macro names in the full text, imports and function values in
// the code alone.
fn sqlx_bypass_hits(file: &RustFile) -> Vec<(usize, &'static str)> {
    let mut hits: Vec<(usize, &'static str)> = unchecked_sqlx_hits(&file.full)
        .into_iter()
        .map(|at| (at, SQLX_CALL))
        .collect();
    hits.extend(
        unchecked_macro_hits(&file.full)
            .into_iter()
            .map(|at| (at, SQLX_MACRO)),
    );
    hits.extend(sqlx_import_hits(&file.code));
    hits.extend(
        sqlx_fn_value_hits(&file.code)
            .into_iter()
            .map(|at| (at, SQLX_VALUE)),
    );
    hits.sort_unstable();
    hits
}

// Lines of the dependencies of `manifest` that resolve to the sqlx package under another
// key, a crate rename that hides the sqlx path. Inherited dependencies resolve through
// `workspace_manifest`; a manifest the shared TOML reader cannot resolve is an error.
fn sqlx_renamed_deps(manifest: &str, workspace_manifest: &str) -> Result<Vec<usize>, ParseError> {
    Ok(declarations(manifest, workspace_manifest)?
        .iter()
        .filter_map(|declaration| {
            let dependency = declaration.dependency.as_ref()?;
            (dependency.key != "sqlx" && canonical(&dependency.package) == "sqlx")
                .then_some(declaration.line)
        })
        .collect())
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

// The bounds of each top-level argument of the macro invocation whose delimiters sit at
// `open` and `close` in `code`: the delimiter or comma before the argument and the comma or
// delimiter after it. Commas nested in parentheses, brackets or braces do not split.
fn macro_args(code: &[u8], open: usize, close: usize) -> Vec<(usize, usize)> {
    let mut bounds = Vec::new();
    let mut depth = 0_usize;
    let mut start = open;
    for (at, &byte) in code.iter().enumerate().take(close).skip(open + 1) {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b',' if depth == 0 => {
                bounds.push((start, at));
                start = at;
            }
            _ => {}
        }
    }
    bounds.push((start, close));
    bounds
}

// The format string of every format macro call, its first argument with the literals in it
// decoded and joined, that has a SQL statement shape: the start of its first literal, with
// that shape. Later arguments are values, not the format string.
fn format_sql_hits(file: &RustFile) -> Vec<(usize, &'static str)> {
    let mut hits = Vec::new();
    for at in find_token(&file.code, b"format", b"") {
        let Some(open) = macro_open(&file.code, at, b"format".len()) else {
            continue;
        };
        let close = close_delim(&file.code, open);
        let Some(&(left, right)) = macro_args(&file.code, open, close).first() else {
            continue;
        };
        let Some(first) = file.strs_between(left, right).next() else {
            continue;
        };
        let template = file.value(left, right);
        if let Some(shape) = sql_shape(&String::from_utf8_lossy(&template.bytes)) {
            hits.push((first.start, shape));
        }
    }
    hits
}

// The sqlx query macros, checked and unchecked, whose SQL may name a table, each with whether
// its grammar is `$out_struct:path, $query:expr`, a record path before the SQL source, rather
// than `$query:expr`, the SQL source first.
const QUERY_MACROS: [(&[u8], bool); 6] = [
    (b"query", false),
    (b"query_as", true),
    (b"query_scalar", false),
    (b"query_unchecked", false),
    (b"query_as_unchecked", true),
    (b"query_scalar_unchecked", false),
];

// Every query macro invocation in `file`, whether it uses parentheses, brackets or braces, as
// (open, close, record): its delimiters, and whether a record path precedes its SQL source.
fn query_macro_calls(file: &RustFile) -> Vec<(usize, usize, bool)> {
    let mut calls: Vec<(usize, usize, bool)> = QUERY_MACROS
        .iter()
        .flat_map(|&(name, record)| {
            find_token(&file.code, name, b"")
                .into_iter()
                .filter_map(move |at| macro_open(&file.code, at, name.len()))
                .map(move |open| (open, close_delim(&file.code, open), record))
        })
        .collect();
    calls.sort_unstable();
    calls
}

// Index of the comma that ends the record path of the record query macro invocation whose
// delimiters sit at `open` and `close` in `code` (where comments and literals are blanked):
// the first comma outside every parenthesis, bracket, brace and generic argument list. Angle
// brackets count only outside the other groups, so a comparison in a const-generic block
// neither opens nor closes a list, and the `>` of an arrow closes none. None when the path
// runs to `close`, leaving the invocation no SQL source.
fn record_path_end(code: &[u8], open: usize, close: usize) -> Option<usize> {
    let mut depth = 0_usize;
    let mut angles = 0_usize;
    for (at, &byte) in code.iter().enumerate().take(close).skip(open + 1) {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'<' if depth == 0 => angles += 1,
            b'>' if depth == 0 && code[at - 1] != b'-' => angles = angles.saturating_sub(1),
            b',' if depth == 0 && angles == 0 => return Some(at),
            _ => {}
        }
    }
    None
}

// The SQL of the query macro invocation whose delimiters sit at `open` and `close`, selected
// as the sqlx grammar selects it: the SQL source is the first argument, or for a `record`
// form the argument after the record path. Its literals are decoded and joined, as sqlx joins
// a `+` chain of literals; an SQL source without a literal has no SQL. Literals inside the
// record path and the bind arguments after the SQL source are never SQL.
fn query_sql(file: &RustFile, open: usize, close: usize, record: bool) -> Option<LitValue> {
    let start = if record {
        record_path_end(&file.code, open, close)?
    } else {
        open
    };
    let (left, right) = macro_args(&file.code, open, close)
        .into_iter()
        .find(|&(left, _)| left == start)?;
    file.strs_between(left, right)
        .next()
        .map(|_| file.value(left, right))
}

// Offsets of the whole word `transactions`, in any case, in the SQL of a query macro
// invocation, at the source character it was decoded from; one per invocation.
fn transactions_hits(file: &RustFile) -> Vec<usize> {
    query_macro_calls(file)
        .into_iter()
        .filter_map(|(open, close, record)| {
            let sql = query_sql(file, open, close, record)?;
            let lower = sql.bytes.to_ascii_lowercase();
            let at = *find_token(&lower, b"transactions", b"").first()?;
            Some(sql.offsets[at])
        })
        .collect()
}

// Whether `rel` lies under `crates/<crate>/src/` for one of `crates`, or any crate when
// `crates` is empty.
fn in_crate_src(rel: &str, crates: &[&str]) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    matches!(parts.as_slice(), ["crates", name, "src", _, ..] if crates.is_empty() || crates.contains(name))
}

// The financial and reward crates, whose src trees the floating-point scan reads.
const FINANCIAL: [&str; 6] = [
    "domain",
    "codec",
    "persistence",
    "api",
    "batch",
    "messaging",
];

// Whether the floating-point scan reads the repository-relative file `rel`: a .rs file
// under the src tree of a financial crate, other than the statement-PDF module.
fn float_scanned(rel: &str) -> bool {
    has_ext(Path::new(rel), &["rs"]) && in_crate_src(rel, &FINANCIAL) && rel != PDF_MODULE
}

// One line per financial crate the floating-point scan would cover vacuously, in FINANCIAL
// order: its src tree is missing, or holds none of the files the scan reads. `files` are
// repository-relative Rust files and `dir_exists` tells whether a repository-relative
// directory exists. Whole-crate presence is not enough, because a build script or an
// integration test lies outside src and is never scanned.
fn float_scan_gaps(files: &[&str], dir_exists: impl Fn(&str) -> bool) -> Vec<String> {
    let mut gaps = Vec::new();
    for name in FINANCIAL {
        let src = format!("crates/{name}/src");
        if !dir_exists(&src) {
            gaps.push(format!(
                "hygiene input directory {src} is missing, so the float scan would pass vacuously"
            ));
        } else if !files
            .iter()
            .any(|rel| float_scanned(rel) && in_crate_src(rel, &[name]))
        {
            gaps.push(format!(
                "{src} holds no scanned .rs file, so the float scan would pass vacuously"
            ));
        }
    }
    gaps
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

// Rule texts of the statement-PDF contract, one per kind of violation.
const PDF_NO_RENDERER: &str = "pdf module has no visible render_statement fn";
const PDF_INPUT: &str = "pdf entry point must take exactly one &[String] text-lines parameter";
const PDF_GENERICS: &str = "pdf entry point has a generic or where bound other than a lifetime";
const PDF_FLOAT: &str = "pdf entry point signature names a floating-point type";
const PDF_EXPORT: &str = "pdf module exports a floating-point item";
const PDF_FINANCIAL: &str = "pdf module names a money, rate or points type";
const PDF_STRING: &str = "pdf module rebinds the name String";

// The domain money, rate and points types and the decimal they wrap.
const FINANCIAL_TYPES: [&[u8]; 8] = [
    b"Money",
    b"MoneyExpr",
    b"PointsExpr",
    b"RewardPoints",
    b"InterestRate",
    b"RewardRate",
    b"RatedBalance",
    b"Decimal",
];

// Qualifiers that may stand between a visibility and the fn keyword. The ABI literal after
// extern is blanked in code.
const FN_QUALIFIERS: [&[u8]; 5] = [b"const", b"async", b"unsafe", b"safe", b"extern"];

// Words that, written just before String, give the name String a new meaning: an item
// keyword, or the rename of a use declaration.
const STRING_BINDERS: [&[u8]; 6] = [b"type", b"struct", b"enum", b"union", b"trait", b"as"];

// The statement-PDF contract, which is what admits the module's f32 page geometry.
fn pdf_contract(violations: &mut Vec<String>) {
    require_file(PDF_MODULE);
    let pdf = rust_files()
        .iter()
        .find(|file| file.rel == PDF_MODULE)
        .unwrap_or_else(|| panic!("{PDF_MODULE} was not among the scanned Rust files"));
    let hits = pdf_contract_hits(pdf)
        .into_iter()
        .map(|(at, rule)| (at, rule.to_owned()))
        .collect();
    report_in_order(violations, PDF_MODULE, &pdf.src, hits);
}

// Violations of the statement-PDF contract in the code spans of `pdf`, as (offset, rule) in
// source order. The entry points are the fns with a visibility and the fns of every trait
// declaration and trait impl, which have no private form. Each takes exactly one `&[String]`
// parameter, declares lifetimes as its only generics and where bounds, and names no
// floating-point type in its signature, and one of them is render_statement; their violations
// sit at the fn keyword. No other declaration reachable from outside the module names a
// floating-point type; those violations sit at the type. The module names no money, rate or
// points type and never rebinds String, so the one parameter is text.
fn pdf_contract_hits(pdf: &RustFile) -> Vec<(usize, &'static str)> {
    let code = pdf.code.as_slice();
    let mut hits = Vec::new();
    let mut has_renderer = false;
    for (item, fn_at) in entry_fns(code) {
        let name_at = skip_ws(code, fn_at + b"fn".len());
        let name = word_at(code, name_at);
        has_renderer |= name == b"render_statement";
        let mut open = skip_ws(code, name_at + name.len());
        if code.get(open) == Some(&b'<') {
            let close = close_angle(code, open);
            if !lifetimes_only(code.get(open + 1..close).unwrap_or_default()) {
                hits.push((fn_at, PDF_GENERICS));
            }
            open = skip_ws(code, (close + 1).min(code.len()));
        }
        if name.is_empty() || code.get(open) != Some(&b'(') {
            // No parameter list this reader can follow, so the input stays unverified.
            hits.push((fn_at, PDF_INPUT));
            continue;
        }
        let close = close_paren(code, open);
        let params = list_parts(&code[open + 1..close]);
        if !matches!(params.as_slice(), [param] if is_lines_param(param)) {
            hits.push((fn_at, PDF_INPUT));
        }
        let end = signature_end(code, close);
        let tail = code.get(close + 1..end).unwrap_or_default();
        let bounds = find_token(tail, b"where", b"")
            .first()
            .map(|&at| &tail[at + b"where".len()..]);
        if bounds.is_some_and(|bounds| !lifetimes_only(bounds)) {
            hits.push((fn_at, PDF_GENERICS));
        }
        if !float_hits(&code[item..end]).is_empty() {
            hits.push((fn_at, PDF_FLOAT));
        }
    }
    if !has_renderer {
        hits.push((0, PDF_NO_RENDERER));
    }
    hits.extend(
        exported_float_hits(code)
            .into_iter()
            .map(|at| (at, PDF_EXPORT)),
    );
    for tok in FINANCIAL_TYPES {
        hits.extend(
            find_token(code, tok, b"")
                .into_iter()
                .map(|at| (at, PDF_FINANCIAL)),
        );
    }
    hits.extend(
        find_token(code, b"String", b"")
            .into_iter()
            .filter(|&at| {
                // A raw identifier names String too.
                let bare = code[..at].strip_suffix(b"r#").map_or(at, <[u8]>::len);
                STRING_BINDERS.contains(&word_before(code, bare))
            })
            .map(|at| (at, PDF_STRING)),
    );
    hits.sort_unstable();
    hits.dedup();
    hits
}

// The entry points of `code`, as (item offset, fn keyword offset): the fns that carry a
// visibility, and the fns declared directly in the body of a trait declaration or trait impl,
// which are reachable wherever the trait is. A fn of both kinds is listed twice.
fn entry_fns(code: &[u8]) -> Vec<(usize, usize)> {
    let mut fns = visible_fns(code);
    for (_, end, reachable) in item_blocks(code) {
        if reachable && code.get(end) == Some(&b'{') {
            fns.extend(body_fns(code, end).into_iter().map(|(at, _)| (at, at)));
        }
    }
    fns
}

// The fns of `code` that carry a visibility, as (visibility offset, fn keyword offset).
fn visible_fns(code: &[u8]) -> Vec<(usize, usize)> {
    find_token(code, b"pub", b"")
        .into_iter()
        .filter_map(|at| fn_keyword(code, visibility_end(code, at)).map(|fn_at| (at, fn_at)))
        .collect()
}

// Index of the first word after the visibility whose word pub is at `at` in `code`. A
// visibility is the word pub with an optional parenthesized restriction.
fn visibility_end(code: &[u8], at: usize) -> usize {
    let next = skip_ws(code, at + b"pub".len());
    if code.get(next) == Some(&b'(') {
        skip_ws(code, (close_paren(code, next) + 1).min(code.len()))
    } else {
        next
    }
}

// Offset of the fn keyword when the words from `at` in `code` are fn qualifiers and then fn.
fn fn_keyword(code: &[u8], mut at: usize) -> Option<usize> {
    let mut word = word_at(code, at);
    while FN_QUALIFIERS.contains(&word) {
        at = skip_ws(code, at + word.len());
        word = word_at(code, at);
    }
    (word == b"fn").then_some(at)
}

// The trait declarations and impl blocks of `code`, as (keyword offset, header end, whether
// every item of the body is reachable from outside the module). The header runs from the
// keyword to the brace that opens the body, or to the semicolon of a bodiless item. The
// items of a trait declaration or trait impl have no private form; those of an inherent impl
// are private unless they carry a visibility. A raw identifier is no keyword.
fn item_blocks(code: &[u8]) -> Vec<(usize, usize, bool)> {
    let mut blocks = Vec::new();
    for (keyword, is_trait) in [(b"trait".as_slice(), true), (b"impl", false)] {
        for at in find_token(code, keyword, b"#") {
            let end = signature_end(code, at);
            blocks.push((at, end, is_trait || is_trait_impl(&code[at..end])));
        }
    }
    blocks
}

// Whether the impl header `header`, from the impl keyword to its body, implements a trait:
// it holds the word for other than as a higher-ranked binder. A binder is followed by
// generics and then no path separator, while a qualified self type, as in
// `for <T as Trait>::Out`, is followed by one.
fn is_trait_impl(header: &[u8]) -> bool {
    find_token(header, b"for", b"#").into_iter().any(|at| {
        let next = skip_ws(header, at + b"for".len());
        if header.get(next) != Some(&b'<') {
            return true;
        }
        let after = skip_ws(header, (close_angle(header, next) + 1).min(header.len()));
        header[after..].starts_with(b"::")
    })
}

// The fn items declared directly in the body whose brace opens at `open` in `code`, as (fn
// keyword offset, offset just past the item). A fn item is the keyword fn, outside every
// nested bracket, followed by a name; a fn pointer type has no name.
fn body_fns(code: &[u8], open: usize) -> Vec<(usize, usize)> {
    let close = close_delim(code, open);
    let mut fns = Vec::new();
    let (mut at, mut depth) = (open + 1, 0_usize);
    while at < close {
        match code[at] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            _ if depth == 0 && starts_fn_item(code, at) => {
                let mut end = signature_end(code, at);
                if code.get(end) == Some(&b'{') {
                    end = close_delim(code, end);
                }
                let fn_at = at;
                at = (end + 1).min(code.len());
                fns.push((fn_at, at));
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    fns
}

// Whether a fn item starts at `at` in `code`: the keyword fn as a whole word, not a raw
// identifier, followed by a name.
fn starts_fn_item(code: &[u8], at: usize) -> bool {
    word_at(code, at) == b"fn"
        && (at == 0 || !is_word_byte(code[at - 1], b"#"))
        && !word_at(code, skip_ws(code, at + b"fn".len())).is_empty()
}

// Offsets of the floating-point types named by the declarations of `code` that are reachable
// from outside the module, fn signatures aside, which the entry-point rules check: the
// non-fn items and fields that carry a visibility; every macro_rules definition; every trait
// declaration and trait impl, header and body, outside its fn items; and the header of an
// inherent impl whose body holds a visibility.
fn exported_float_hits(code: &[u8]) -> Vec<usize> {
    let mut hits: Vec<usize> = visible_decls(code)
        .into_iter()
        .chain(macro_definitions(code))
        .flat_map(|(start, end)| floats_outside(code, start, end, &[]))
        .collect();
    for (at, end, reachable) in item_blocks(code) {
        let has_body = code.get(end) == Some(&b'{');
        let close = if has_body {
            close_delim(code, end)
        } else {
            end
        };
        if reachable {
            let fns = if has_body {
                body_fns(code, end)
            } else {
                Vec::new()
            };
            hits.extend(floats_outside(code, at, close, &fns));
        } else if has_body && !find_token(&code[end..close], b"pub", b"").is_empty() {
            hits.extend(floats_outside(code, at, end, &[]));
        }
    }
    hits.sort_unstable();
    hits.dedup();
    hits
}

// Offsets of the floating-point types that `code` names in `start..end`, other than inside
// the (start, end) ranges of `skip`.
fn floats_outside(code: &[u8], start: usize, end: usize, skip: &[(usize, usize)]) -> Vec<usize> {
    float_hits(&code[start..end])
        .into_iter()
        .map(|at| start + at)
        .filter(|at| !skip.iter().any(|&(from, to)| (from..to).contains(at)))
        .collect()
}

// The declarations of `code` that carry a visibility and are not fns, as (visibility offset,
// end offset), cut to their visible text. A const, static, type alias, use or extern crate
// runs to its semicolon, and an enum, whose variant fields are public, to its closing brace.
// A struct or union keeps its header and the text after a tuple field list, since each field
// carries its own visibility; a field runs to its comma or the close of its list. A module or
// trait declares nothing of its own here: a module's items carry their own visibility, and a
// trait is one of the item blocks.
fn visible_decls(code: &[u8]) -> Vec<(usize, usize)> {
    let mut decls = Vec::new();
    for at in find_token(code, b"pub", b"") {
        let next = visibility_end(code, at);
        if fn_keyword(code, next).is_some() {
            continue;
        }
        match word_at(code, next) {
            b"mod" | b"trait" => {}
            b"const" | b"static" | b"type" | b"use" | b"extern" => {
                decls.push((at, decl_end(code, next, b";", false)));
            }
            b"enum" => {
                let mut end = decl_end(code, next, b"{;", true);
                if code.get(end) == Some(&b'{') {
                    end = (close_delim(code, end) + 1).min(code.len());
                }
                decls.push((at, end));
            }
            b"struct" | b"union" => {
                let open = decl_end(code, next, b"{(;", true);
                decls.push((at, open));
                if code.get(open) == Some(&b'(') {
                    let close = close_delim(code, open);
                    decls.push((close, decl_end(code, close + 1, b";", false)));
                }
            }
            _ => decls.push((at, decl_end(code, next, b",;{", true))),
        }
    }
    decls
}

// Index of the first byte at or after `from` in `code` that is one of `stops` outside every
// bracket, and outside every angle bracket when `angles` is set; or of the closer of the
// bracket that encloses `from`; or the end of the text. An opening bracket among `stops`
// ends the declaration before it nests.
fn decl_end(code: &[u8], from: usize, stops: &[u8], angles: bool) -> usize {
    let (mut nest, mut angle) = (0_usize, 0_usize);
    for (at, &byte) in code.iter().enumerate().skip(from) {
        if nest == 0 && angle == 0 && stops.contains(&byte) {
            return at;
        }
        match byte {
            b'(' | b'[' | b'{' => nest += 1,
            b')' | b']' | b'}' if nest == 0 => return at,
            b')' | b']' | b'}' => nest -= 1,
            b'<' if angles && nest == 0 => angle += 1,
            b'>' if angles && nest == 0 && (at == 0 || code[at - 1] != b'-') => {
                angle = angle.saturating_sub(1);
            }
            _ => {}
        }
    }
    code.len()
}

// The macro_rules definitions of `code`, as (keyword offset, end offset). A macro's body is
// written out at each of its call sites, which may lie in any module its textual scope
// reaches, so a type the body names leaves the module with it.
fn macro_definitions(code: &[u8]) -> Vec<(usize, usize)> {
    find_token(code, b"macro_rules", b"#")
        .into_iter()
        .filter_map(|at| {
            let bang = skip_ws(code, at + b"macro_rules".len());
            if code.get(bang) != Some(&b'!') {
                return None;
            }
            let name_at = skip_ws(code, bang + 1);
            let open = skip_ws(code, name_at + word_at(code, name_at).len());
            Some((at, (close_delim(code, open) + 1).min(code.len())))
        })
        .collect()
}

// Index of the angle bracket that closes the one at `open` in `code`, passing over the
// arrows of fn-trait bounds, or the end of the text.
fn close_angle(code: &[u8], open: usize) -> usize {
    let mut depth = 0_usize;
    for (at, &byte) in code.iter().enumerate().skip(open) {
        match byte {
            b'<' => depth += 1,
            b'>' if code[at - 1] != b'-' => {
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

// Index of the brace or semicolon that ends the item header whose rest follows `after` (the
// close of a fn's parameter list, or the offset of an item keyword), outside any bracket of
// the return type, generics or where clause, or the end of `code`.
fn signature_end(code: &[u8], after: usize) -> usize {
    let (mut nest, mut angle) = (0_usize, 0_usize);
    for (at, &byte) in code.iter().enumerate().skip(after + 1) {
        match byte {
            b'(' | b'[' => nest += 1,
            b')' | b']' => nest = nest.saturating_sub(1),
            b'<' if nest == 0 => angle += 1,
            b'>' if nest == 0 && code[at - 1] != b'-' => angle = angle.saturating_sub(1),
            b'{' | b';' if nest == 0 && angle == 0 => return at,
            _ => {}
        }
    }
    code.len()
}

// The comma-separated parts of a parameter, generic or where list, split outside brackets,
// each with its whitespace removed. A trailing comma adds no part.
fn list_parts(text: &[u8]) -> Vec<Vec<u8>> {
    let mut parts = Vec::new();
    let mut current = Vec::new();
    let mut depth = 0_usize;
    for (at, &byte) in text.iter().enumerate() {
        match byte {
            b',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            b'(' | b'[' | b'{' | b'<' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'>' if at == 0 || text[at - 1] != b'-' => depth = depth.saturating_sub(1),
            _ => {}
        }
        if !byte.is_ascii_whitespace() {
            current.push(byte);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

// Whether one parameter, whitespace removed, binds text lines: a pattern, a colon, then a
// shared slice of String, optionally under a named lifetime.
fn is_lines_param(param: &[u8]) -> bool {
    let Some(colon) = param.iter().position(|&byte| byte == b':') else {
        return false;
    };
    let Some(slice) = param[colon + 1..].strip_prefix(b"&") else {
        return false;
    };
    let slice = match slice.strip_prefix(b"'") {
        Some(lifetime) => match word_at(lifetime, 0).len() {
            0 => return false,
            len => &lifetime[len..],
        },
        None => slice,
    };
    slice == b"[String]"
}

// Whether every part of a generic or where list is a lifetime, the one kind that carries no
// input.
fn lifetimes_only(list: &[u8]) -> bool {
    list_parts(list)
        .iter()
        .all(|part| part.first() == Some(&b'\''))
}

// The identifier that ends where the whitespace before `at` begins, empty when none does.
fn word_before(text: &[u8], at: usize) -> &[u8] {
    let end = text[..at]
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |last| last + 1);
    let start = text[..end]
        .iter()
        .rposition(|&byte| !is_ident(byte))
        .map_or(0, |last| last + 1);
    &text[start..end]
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
    let mut violations = Vec::new();
    for file in rust_files() {
        let hits = sqlx_bypass_hits(file)
            .into_iter()
            .map(|(at, rule)| (at, rule.to_owned()))
            .collect();
        report_in_order(&mut violations, &file.rel, &file.src, hits);
    }
    let root_text = read_text(&require_file("Cargo.toml"));
    for name in CRATES {
        let (rel, text) = crate_manifest(name);
        match sqlx_renamed_deps(&text, &root_text) {
            Ok(lines) => {
                for line in lines {
                    report_line(
                        &mut violations,
                        &rel,
                        text.as_bytes(),
                        line,
                        SQLX_DEP_RENAME,
                    );
                }
            }
            Err(error) => report_unreadable(
                &mut violations,
                &rel,
                text.as_bytes(),
                "dependency declaration",
                &error,
            ),
        }
    }
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
    let scanned: Vec<&str> = rust_files().iter().map(|file| file.rel.as_str()).collect();
    let gaps = float_scan_gaps(&scanned, |dir| root().join(dir).is_dir());
    assert!(
        gaps.is_empty(),
        "{} financial crate(s) give the float scan nothing to read:\n{}",
        gaps.len(),
        gaps.join("\n")
    );
    let mut violations = scan_rust(
        |file| float_scanned(&file.rel),
        |file| float_hits(&file.code),
        "floating-point type in financial code",
    );
    pdf_contract(&mut violations);
    finish(&violations);
}

// ---------------------------------------------------------------------------------------
// Minimal TOML reader
// ---------------------------------------------------------------------------------------

// The std-only TOML reader, shared by source with crates/domain/tests/manifest_boundary.rs.
// It lives beside that test because the domain crate takes no dependency on this one.
#[path = "../../domain/tests/support/manifest_reader.rs"]
mod manifest_reader;

use manifest_reader::{Declaration, DepKind, ParseError, Value, declarations, entries};

// Reports a line the TOML reader rejects. The gate fails closed on it, since a form the
// reader cannot read could hide any violation.
fn report_unreadable(
    violations: &mut Vec<String>,
    rel: &str,
    src: &[u8],
    what: &str,
    error: &ParseError,
) {
    let rule = format!("unreadable {what} ({})", error.message);
    report_line(violations, rel, src, error.line, &rule);
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
    let read = entries(text).unwrap_or_else(|error| {
        report_unreadable(violations, rel, src, "TOML", &error);
        Vec::new()
    });
    let mut inherits = false;
    for entry in read
        .iter()
        .filter(|entry| has_prefix(&entry.path, &["lints"]))
    {
        match (&entry.path[1..], &entry.value) {
            ([], Value::Table) => {}
            ([field], Value::Other(flag)) if field == "workspace" && flag == "true" => {
                inherits = true;
            }
            ([_, ..], Value::Table) => {
                report_line(violations, rel, src, entry.line, "crate-level lint table");
            }
            _ => report_line(
                violations,
                rel,
                src,
                entry.line,
                "lints entry other than workspace = true",
            ),
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
    let read = entries(text).unwrap_or_else(|error| {
        report_unreadable(violations, rel, src, "TOML", &error);
        Vec::new()
    });
    let mut levels: Vec<(String, String, String)> = Vec::new();
    let lint_entries = read
        .iter()
        .filter(|entry| has_prefix(&entry.path, &["workspace", "lints"]));
    for entry in lint_entries {
        let (tool, lint, value) = match (&entry.path[2..], &entry.value) {
            // The lint tables, and an inline `{ level, priority }` table, hold the entries.
            ([] | [_] | [_, _], Value::Table) => continue,
            ([tool, lint], value) => (tool, lint, value),
            ([tool, lint, attribute], value) if attribute == "level" => (tool, lint, value),
            ([_, _, attribute], _) if attribute == "priority" => continue,
            _ => {
                report_line(
                    violations,
                    rel,
                    src,
                    entry.line,
                    "unrecognised workspace lint entry",
                );
                continue;
            }
        };
        let level = match value {
            Value::Str(level) => level.clone(),
            _ => String::new(),
        };
        if level != "deny" && level != "forbid" {
            report_line(
                violations,
                rel,
                src,
                entry.line,
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

// Long rustc and rustdoc options that relax or cap lint levels. Each takes its value as the
// next argument or after an equals sign.
const RELAXING_LONG: [&str; 4] = ["--cap-lints", "--allow", "--warn", "--force-warn"];

// Short options that relax lint levels, with their value attached or as the next argument.
const RELAXING_SHORT: [&str; 2] = ["-A", "-W"];

// Attributes an injected crate attribute may not be, since each relaxes a lint level.
const RELAXING_ATTRIBUTES: [&str; 3] = ["allow", "warn", "expect"];

// Whether the value of an unstable -Z option injects a crate attribute that relaxes a lint
// level, directly or nested in another attribute. Option names fold `_` to `-`, as rustc
// reads them.
fn injects_relaxing_attribute(option: &str) -> bool {
    let Some((name, attribute)) = option.split_once('=') else {
        return false;
    };
    name.replace('_', "-") == "crate-attr"
        && attribute
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .any(|word| RELAXING_ATTRIBUTES.contains(&word))
}

// The relaxing flag `flag` starts, if it starts one, with the number of arguments it
// spans: two when its value is the next argument `next`, else one.
fn relaxing_flag(flag: &str, next: Option<&str>) -> Option<(String, usize)> {
    let with_next = || {
        next.map_or_else(
            || (flag.to_owned(), 1),
            |value| (format!("{flag} {value}"), 2),
        )
    };
    if RELAXING_LONG.contains(&flag) || RELAXING_SHORT.contains(&flag) {
        return Some(with_next());
    }
    let long_assigned = RELAXING_LONG.iter().any(|long| {
        flag.strip_prefix(long)
            .is_some_and(|rest| rest.starts_with('='))
    });
    let short_attached = RELAXING_SHORT.iter().any(|short| flag.starts_with(short));
    if long_assigned || short_attached {
        return Some((flag.to_owned(), 1));
    }
    if flag == "-Z" {
        return next
            .filter(|option| injects_relaxing_attribute(option))
            .map(|_| with_next());
    }
    flag.strip_prefix("-Z")
        .filter(|option| injects_relaxing_attribute(option))
        .map(|_| (flag.to_owned(), 1))
}

// The rule an argument file breaks when its arguments cannot be checked, and why.
fn unverifiable(file: &str, why: &str) -> String {
    format!("rustflags argument file cannot be checked ({file}: {why})")
}

// The violations, as rules in argument order, of `args`, one compiler argument each, read
// as rustc and rustdoc read them. Each argument that starts with `@` names a file whose
// lines, a trailing carriage return stripped, replace it one argument per line, one level
// deep; `arg_files` returns that file's text, or None when it cannot be read as UTF-8. The
// expanded arguments are classified in order, so a switch whose value is the next argument
// is caught across a file boundary. A flag that relaxes or caps lint levels is a violation:
// --cap-lints, -A, -W, --allow, --warn and --force-warn in any value form, and a -Z
// crate-attr injection of an allow, warn or expect attribute, named with the argument files
// it came from. The deny and forbid forms, and the arguments of other options, are not.
// An argument file the gate cannot follow fails closed: one that cannot be read, one in the
// unstable shell form (shell words under -Z shell-argfiles, a plain file otherwise), and a
// line inside one that names another argument file, which rustc passes on unexpanded.
fn flag_rules(args: &[&str], arg_files: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    // Each rule with the index of the expanded argument it arises at.
    let mut rules: Vec<(usize, String)> = Vec::new();
    // Each expanded argument with the argument file it was read from, if any.
    let mut expanded: Vec<(String, Option<&str>)> = Vec::new();
    for &arg in args {
        let Some(path) = arg.strip_prefix('@') else {
            expanded.push((arg.to_owned(), None));
            continue;
        };
        if path.starts_with("shell:") {
            rules.push((expanded.len(), unverifiable(arg, "unstable shell form")));
            continue;
        }
        let Some(text) = arg_files(path) else {
            rules.push((expanded.len(), unverifiable(arg, "unreadable or not UTF-8")));
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if line.starts_with('@') {
                let why = format!("line {} is {line}, which rustc does not expand", index + 1);
                rules.push((expanded.len(), unverifiable(arg, &why)));
            }
            expanded.push((line.to_owned(), Some(arg)));
        }
    }
    let mut at = 0;
    while let Some((flag, _)) = expanded.get(at) {
        let next = expanded.get(at + 1).map(|(value, _)| value.as_str());
        let Some((text, spanned)) = relaxing_flag(flag, next) else {
            at += 1;
            continue;
        };
        let mut files: Vec<&str> = expanded[at..at + spanned]
            .iter()
            .filter_map(|&(_, file)| file)
            .collect();
        files.dedup();
        let source = if files.is_empty() {
            String::new()
        } else {
            format!(" from {}", files.join(" and "))
        };
        rules.push((at, format!("rustflags relax or cap lints ({text}{source})")));
        at += spanned;
    }
    // A stable sort, so a file problem stays ahead of a flag that starts where it arises.
    rules.sort_by_key(|&(at, _)| at);
    rules.into_iter().map(|(_, rule)| rule).collect()
}

// The text of the argument file a `@path` compiler argument names, or None when it cannot
// be read as UTF-8. Cargo runs rustc and rustdoc for workspace members from the workspace
// root, so a relative path resolves against the repository root; an absolute one is used
// as it is.
fn read_arg_file(path: &str) -> Option<String> {
    fs::read_to_string(root().join(path)).ok()
}

// Whether a Cargo configuration key names compiler or rustdoc flags: `rustflags` and
// `rustdocflags` under build, target or host, or a RUSTFLAGS-family variable under env.
fn names_flags(segment: &str) -> bool {
    let lower = segment.to_ascii_lowercase();
    lower.contains("rustflags") || lower.contains("rustdocflags")
}

// The flag values of one Cargo configuration file relax or cap no lint, directly or through
// an argument file `arg_files` supplies. A string holds whitespace-separated flags and an
// array one flag per element. An array element that is not a string, or a flag key holding
// a boolean, number or date, cannot be checked and is a violation.
fn config_flag_violations(
    violations: &mut Vec<String>,
    rel: &str,
    text: &str,
    arg_files: &dyn Fn(&str) -> Option<String>,
) {
    let src = text.as_bytes();
    let read = entries(text).unwrap_or_else(|error| {
        report_unreadable(violations, rel, src, "TOML", &error);
        Vec::new()
    });
    for entry in &read {
        if !entry.path.iter().any(|segment| names_flags(segment)) {
            continue;
        }
        let at_key = entry
            .path
            .last()
            .is_some_and(|segment| names_flags(segment));
        let mut flags = Vec::new();
        match &entry.value {
            Value::Str(value) => flags.extend(value.split_whitespace()),
            Value::Array(items) => {
                for item in items {
                    if let Value::Str(flag) = item {
                        flags.push(flag.as_str());
                    } else {
                        let rule = format!("rustflags element {item} is not a string");
                        report_line(violations, rel, src, entry.line, &rule);
                    }
                }
            }
            Value::Other(_) if at_key => report_line(
                violations,
                rel,
                src,
                entry.line,
                "rustflags value is neither a string nor an array",
            ),
            Value::Other(_) | Value::Table => continue,
        }
        for rule in flag_rules(&flags, arg_files) {
            report_line(violations, rel, src, entry.line, &rule);
        }
    }
}

// The Cargo configuration files a build of this repository reads: `.cargo/config.toml` and
// `.cargo/config` in the repository root and in every ancestor directory, then in the Cargo
// home, which is $CARGO_HOME or else $HOME/.cargo.
fn cargo_config_files() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = root().ancestors().map(|dir| dir.join(".cargo")).collect();
    let home = std::env::var_os("CARGO_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|home| !home.is_empty())
                .map(|home| PathBuf::from(home).join(".cargo"))
        });
    if let Some(home) = home.filter(|home| !dirs.contains(home)) {
        dirs.push(home);
    }
    dirs.iter()
        .flat_map(|dir| ["config.toml", "config"].map(|name| dir.join(name)))
        .filter(|path| path.is_file())
        .collect()
}

// No Cargo configuration file carries flags that relax or cap lints.
fn cargo_config_caps(violations: &mut Vec<String>) {
    for path in cargo_config_files() {
        let label = if path.starts_with(root()) {
            rel(&path)
        } else {
            path.display().to_string()
        };
        config_flag_violations(violations, &label, &read_text(&path), &read_arg_file);
    }
}

// The flags Cargo takes from the environment relax or cap no lint, directly or through an
// argument file `arg_files` supplies: every variable whose name ends in RUSTFLAGS or
// RUSTDOCFLAGS. The CARGO_ENCODED_ forms separate flags with the unit separator, the others
// with whitespace. A value that is not UTF-8 cannot be checked and is a violation.
fn env_flag_violations(
    vars: impl IntoIterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
    arg_files: &dyn Fn(&str) -> Option<String>,
) -> Vec<String> {
    let mut violations = Vec::new();
    for (name, value) in vars {
        let upper = name.as_encoded_bytes().to_ascii_uppercase();
        if !(upper.ends_with(b"RUSTFLAGS") || upper.ends_with(b"RUSTDOCFLAGS")) {
            continue;
        }
        let label = name.to_string_lossy();
        let Some(value) = value.to_str() else {
            violations.push(format!(
                "environment variable {label}: value is not UTF-8, so its flags cannot be checked"
            ));
            continue;
        };
        let flags: Vec<&str> = if upper.starts_with(b"CARGO_ENCODED_") {
            value
                .split('\x1f')
                .filter(|flag| !flag.is_empty())
                .collect()
        } else {
            value.split_whitespace().collect()
        };
        for rule in flag_rules(&flags, arg_files) {
            violations.push(format!("environment variable {label}: {rule}"));
        }
    }
    violations
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
    violations.extend(env_flag_violations(std::env::vars_os(), &read_arg_file));
    finish(&violations);
}

// Lower case with `_` folded to `-`, so a respelled crate name compares equal.
fn canonical(name: &str) -> String {
    name.to_ascii_lowercase().replace('_', "-")
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

// Asserts that the manifest reader found `key` in a table of `kind` of crate `name`, so the
// edge rules cannot pass on a manifest the reader misread.
fn require_dep(all: &[(&str, Vec<Declaration>)], name: &str, key: &str, kind: DepKind) {
    let found = all.iter().any(|(crate_name, declared)| {
        *crate_name == name
            && declared.iter().any(|declaration| {
                declaration.table.kind == kind
                    && declaration
                        .dependency
                        .as_ref()
                        .is_some_and(|dependency| dependency.key == key)
            })
    });
    assert!(
        found,
        "the manifest reader did not find `{key}` ({kind:?}) in crates/{name}/Cargo.toml"
    );
}

// Reports the crate-graph violations of the crate in directory `name`, whose manifest
// `text` is reported as `rel`; inherited dependencies resolve through the workspace
// manifest `root_text`. Every declaration of carddemo-audit is a violation, its dependency
// tables included, empty or not. Returns the declarations, none when the manifest cannot
// be resolved.
fn crate_edges(
    violations: &mut Vec<String>,
    name: &str,
    rel: &str,
    text: &str,
    root_text: &str,
) -> Vec<Declaration> {
    let src = text.as_bytes();
    let declared = match declarations(text, root_text) {
        Ok(declared) => declared,
        Err(error) => {
            report_unreadable(violations, rel, src, "dependency declaration", &error);
            return Vec::new();
        }
    };
    for declaration in &declared {
        let rule = match &declaration.dependency {
            Some(dependency) => {
                let package = canonical(&dependency.package);
                edge_rule(name, &package, declaration.table.kind)
                    .map(|rule| format!("{rule} ({package})"))
            }
            None if name == "audit" => {
                Some("carddemo-audit declares a dependency table".to_owned())
            }
            None => None,
        };
        if let Some(rule) = rule {
            report_line(violations, rel, src, declaration.line, &rule);
        }
    }
    declared
}

#[test]
fn forbidden_crate_edges() {
    let root_text = read_text(&require_file("Cargo.toml"));
    let mut violations = Vec::new();
    let mut all = Vec::new();
    for name in CRATES {
        let (rel, text) = crate_manifest(name);
        let declared = crate_edges(&mut violations, name, &rel, &text, &root_text);
        all.push((name, declared));
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
        let next = skip_spaces(&lower, at + b"double".len());
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
fn trivia_gap_self_check() {
    for positive in [
        "#[allow/**/(dead_code)]",
        "#[/*gap*/expect(dead_code)]",
        "#/*x*/![allow(y)]",
        "#[cfg_attr(test, allow/**/(x))]",
        "# /* a /* b */ c */ [allow(x)]",
        "#[allow // c\n(x)]",
        "#\u{B}[allow(x)]",
        "#[allow\u{85}(x)]",
        "#[cfg_attr(test, /* ] */ allow(x))]",
        "#[cfg_attr(feature = \"]\", expect(x))]",
        "#[r#allow(x)]",
        "#![r#expect(x)]",
        "# [ r#allow /* c */ (x) ]",
        "#[r#cfg_attr(test, allow(x))]",
        "#[cfg_attr(test, r#allow(x))]",
    ] {
        assert_eq!(
            suppression_hits(positive.as_bytes()),
            [0],
            "not detected: {positive:?}"
        );
    }
    for negative in [
        "#[allow / (x)]",
        "#[allow /* (x) */]",
        "#[allow /* a /* b */ (x) */]",
        "#[cfg_attr(test, derive /* allow */ (Debug))]",
        "#[r#allowed(x)]",
        "#[r#derive(Debug)]",
    ] {
        assert!(
            suppression_hits(negative.as_bytes()).is_empty(),
            "false positive: {negative:?}"
        );
    }

    for positive in [
        "sqlx/*gap*/::query(\"x\")",
        "sqlx:://c\nquery(\"x\")",
        "sqlx::query /* a /* b */ c */ (\"x\")",
        "sqlx\u{200E}::query_as::/**/<_, Row>(\"x\")",
    ] {
        assert_eq!(
            unchecked_sqlx_hits(positive.as_bytes()),
            [0],
            "{positive:?}"
        );
    }
    assert!(unchecked_sqlx_hits(b"sqlx::query /* ( */ !(\"x\")").is_empty());

    for (positive, name) in [
        ("Number/*gap*/(amount)", "Number"),
        ("fetch/*gap*/(url)", "fetch"),
        ("fetch // c\n(url)", "fetch"),
        ("fetch // c\r(url)", "fetch"),
        ("fetch /* a /* b */ (x)", "fetch"),
        ("fetch\u{FEFF}\u{2028}(x)", "fetch"),
        ("parseFloat /**/ /**/ (x)", "parseFloat"),
    ] {
        assert_eq!(
            js_calls(positive.as_bytes(), name.as_bytes()),
            [0],
            "{positive:?}"
        );
    }
    for (negative, name) in [
        ("Number.isInteger/*c*/(x)", "Number"),
        ("prefetch/*c*/(x)", "fetch"),
        ("fetch /* a /* b */ c */ (x)", "fetch"),
        ("fetch // (x)", "fetch"),
        ("fetch /* (x)", "fetch"),
    ] {
        assert!(
            js_calls(negative.as_bytes(), name.as_bytes()).is_empty(),
            "{negative:?}"
        );
    }
}

#[test]
fn sqlx_bypass_self_check() {
    let rules = |src: &str| -> Vec<(usize, &'static str)> {
        let file = RustFile::new("crates/x/src/c.rs".to_owned(), src.as_bytes().to_vec());
        sqlx_bypass_hits(&file)
            .into_iter()
            .map(|(at, rule)| (line_of(&file.src, at), rule))
            .collect()
    };
    for (positive, rule) in [
        ("sqlx::query_unchecked!(\"SELECT 1\")", SQLX_MACRO),
        ("sqlx::query_as_unchecked!(Row, \"SELECT 1\")", SQLX_MACRO),
        ("sqlx::query_file_scalar_unchecked!(\"q.sql\")", SQLX_MACRO),
        ("use sqlx::query_file_as_unchecked;", SQLX_MACRO),
        ("sqlx::r#query(\"SELECT 1\")", SQLX_CALL),
        ("let rows = names.iter().map(sqlx::query);", SQLX_VALUE),
        ("let f = sqlx :: query_as;", SQLX_VALUE),
        ("use sqlx::query;", SQLX_IMPORT),
        ("pub use ::sqlx::query_as as qa;", SQLX_IMPORT),
        ("use sqlx::{postgres::PgPool, query};", SQLX_IMPORT),
        (
            "use sqlx::{postgres::{PgPool, PgRow}, query_with as w};",
            SQLX_IMPORT,
        ),
        ("use {sqlx::query_scalar};", SQLX_IMPORT),
        ("use sqlx::r#query;", SQLX_IMPORT),
        ("use sqlx/* c */::query;", SQLX_IMPORT),
        ("use sqlx::*;", SQLX_GLOB),
        ("pub(crate) use sqlx::{Row, *};", SQLX_GLOB),
        ("use sqlx as db;", SQLX_ALIAS),
        ("use sqlx::{self as db};", SQLX_ALIAS),
        ("use sqlx::{self as db, Row};", SQLX_ALIAS),
        ("extern crate sqlx as db;", SQLX_ALIAS),
    ] {
        assert_eq!(rules(positive), [(1, rule)], "{positive}");
    }
    assert_eq!(
        rules("use sqlx::{\n    postgres::PgPool,\n    query,\n};"),
        [(3, SQLX_IMPORT)]
    );
    for negative in [
        "sqlx::query!(\"x\")",
        "sqlx::query_as!(Row, \"x\")",
        "sqlx::query_file!(\"q.sql\")",
        "sqlx::query_file_as!(Row, \"q.sql\")",
        "use sqlx::query::Query;",
        "use sqlx::postgres::PgPool;",
        "use sqlx::{self, Row};",
        "use sqlx::Executor;",
        "use sqlx;",
        "use sqlx::query as _;",
        "use sqlx::postgres::*;",
        "use sqlx::query::*;",
        "extern crate sqlx;",
        "let q: sqlx::query::Query<'_, Postgres, PgArguments> = x;",
        "fn f() -> impl Sized + use<> {}",
        "// use sqlx::query;\nlet s = \"use sqlx as db;\";",
        "my_query_unchecked(x)",
    ] {
        assert!(rules(negative).is_empty(), "false positive: {negative}");
    }

    let manifest = concat!(
        "[dependencies]\n",
        "db = { package = \"sqlx\", version = \"0.8\" }\n",
        "sqlx = { workspace = true }\n",
        "pool.workspace = true\n",
        "[dev-dependencies]\n",
        "SQLX = { package = \"sqlx\" }\n",
        "tokio = \"1\"\n",
    );
    let workspace = concat!(
        "[workspace.dependencies]\n",
        "sqlx = \"0.8\"\n",
        "pool = { package = \"sqlx\", version = \"0.8\" }\n",
    );
    let renamed = |manifest: &str| {
        sqlx_renamed_deps(manifest, workspace)
            .unwrap_or_else(|error| panic!("line {}: {}", error.line, error.message))
    };
    assert_eq!(renamed(manifest), [2, 4, 6]);
    assert!(renamed("[dependencies]\nsqlx = \"0.8\"\n").is_empty());
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
            "query!( /* ) */ \"SELECT * FROM TRANSACTIONS\");\n",
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

// Synthetic statement-PDF modules, each with the (line, rule) violations the contract finds.
const PDF_CASES: &[(&str, &[(usize, &str)])] = &[
    // The renderer's signature, and the same type with spaces, a named lifetime or a
    // trailing comma.
    (
        "pub(crate) fn render_statement(lines: &[String]) -> Result<Vec<u8>, RenderError> {}",
        &[],
    ),
    (
        "pub(crate) fn render_statement ( mut lines : & [ String ] ) {}",
        &[],
    ),
    (
        "pub(crate) fn render_statement<'a>(lines: &'a [String]) -> Vec<u8> {}",
        &[],
    ),
    (
        "pub(crate) fn render_statement(\n    lines: &[String],\n) {}",
        &[],
    ),
    // Private helpers, test fns and commented-out code are not entry points.
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "fn y(row: u16) -> f32 { 0.0 }\n",
            "#[cfg(test)]\nmod tests {\n    fn extract(pdf: &[u8]) -> usize { 0 }\n}\n",
            "// pub fn interest(balance: f64) -> f64\n",
        ),
        &[],
    ),
    // A numeric input, an unrelated entry point, or a numeric result.
    (
        "pub(crate) fn render_statement(lines: &[String], amount: f64) {}",
        &[(1, PDF_INPUT), (1, PDF_FLOAT)],
    ),
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "pub fn interest(balance: i64) -> i64 { balance }\n",
        ),
        &[(2, PDF_INPUT)],
    ),
    (
        concat!(
            "fn helper(lines: &[String]) {}\n",
            "pub fn render_statement(total: f32) {}\n",
        ),
        &[(2, PDF_INPUT), (2, PDF_FLOAT)],
    ),
    (
        "pub(crate) fn render_statement(lines: &[String]) -> f32 { 0.0 }",
        &[(1, PDF_FLOAT)],
    ),
    (
        "pub(crate) fn render_statement(lines: &[String], more: &[String]) {}",
        &[(1, PDF_INPUT)],
    ),
    (
        "pub(crate) fn render_statement(lines: &mut [String]) {}",
        &[(1, PDF_INPUT)],
    ),
    ("pub(crate) fn render_statement() {}", &[(1, PDF_INPUT)]),
    // Every visibility and qualifier form opens an entry point.
    (
        concat!(
            "pub(in crate::jobs) async fn render_statement(lines: &[String]) {}\n",
            "pub(super) const fn margin() -> u8 { 0 }\n",
            "pub ( self ) extern \"C\" fn pages(count: u32) {}\n",
        ),
        &[(2, PDF_INPUT), (3, PDF_INPUT)],
    ),
    // Inputs passed as generics or constrained through a where clause.
    (
        "pub(crate) fn render_statement<const CENTS: i64>(lines: &[String]) {}",
        &[(1, PDF_GENERICS)],
    ),
    (
        "pub fn render_statement<'a, T: Into<u64>>(lines: &'a [String]) where T: Copy {}",
        &[(1, PDF_GENERICS)],
    ),
    // No visible fn.
    (
        "fn render_statement(lines: &[String]) {}\nconst LEFT_MM: f32 = 25.0;",
        &[(1, PDF_NO_RENDERER)],
    ),
    // A financial type named anywhere, or String rebound to another type.
    (
        concat!(
            "use rust_decimal::Decimal;\n",
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "fn points(rate: RewardRate) -> PointsExpr { rate.points() }\n",
        ),
        &[(1, PDF_FINANCIAL), (3, PDF_FINANCIAL), (3, PDF_FINANCIAL)],
    ),
    (
        concat!(
            "type String = f64;\n",
            "use core::primitive::u64 as r#String;\n",
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
        ),
        &[(1, PDF_STRING), (2, PDF_STRING)],
    ),
    // Trait and trait impl fns have no private form, so each is an entry point.
    (
        concat!(
            "pub fn render_statement(_: &[String]) {}\n",
            "pub struct Interest;\n",
            "pub trait Accrue { fn interest(balance: f64) -> f64; }\n",
            "impl Accrue for Interest {\n",
            "    fn interest(balance: f64) -> f64 { balance * 1.01 }\n",
            "}\n",
        ),
        &[
            (3, PDF_INPUT),
            (3, PDF_FLOAT),
            (5, PDF_INPUT),
            (5, PDF_FLOAT),
        ],
    ),
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "trait Accrue { fn interest(balance: i64) -> i64; }\n",
            "impl<T: Copy> Accrue for Ledger<T> where T: Default {\n",
            "    fn interest(balance: i64) -> i64 { balance }\n",
            "}\n",
        ),
        &[(2, PDF_INPUT), (4, PDF_INPUT)],
    ),
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "pub trait Render { fn page<'a>(lines: &'a [String]) -> Vec<u8>; }\n",
            "impl Render for Page {\n",
            "    fn page<'a>(lines: &'a [String]) -> Vec<u8> {\n",
            "        fn y(row: u16) -> f32 { 0.0 }\n",
            "        Vec::new()\n",
            "    }\n",
            "}\n",
        ),
        &[],
    ),
    // A visible item, field, trait item or impl header naming a floating-point type exports
    // it, and so does a macro body.
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "pub const RATE: f32 = 1.01;\n",
            "pub struct Geometry { pub left: f32 }\n",
            "pub struct P(pub f64);\n",
            "pub type Mm = f32;\n",
        ),
        &[
            (2, PDF_EXPORT),
            (3, PDF_EXPORT),
            (4, PDF_EXPORT),
            (5, PDF_EXPORT),
        ],
    ),
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "pub(crate) static SCALE: [f64; 2] = [1.0, 2.0];\n",
            "pub enum Amount { Cents(i64), Units { value: f64 } }\n",
            "pub(super) struct Pair(pub (f32, u8), u8);\n",
            "pub struct Unit<T = f32>(T);\n",
            "pub use core::primitive::f64 as Ratio;\n",
        ),
        &[
            (2, PDF_EXPORT),
            (3, PDF_EXPORT),
            (4, PDF_EXPORT),
            (5, PDF_EXPORT),
            (6, PDF_EXPORT),
        ],
    ),
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) {}\n",
            "pub trait Geometry { const LEFT: f32; type Unit; }\n",
            "impl Geometry for Page { const LEFT: f32 = 25.0; type Unit = f64; }\n",
            "impl From<f64> for Page { fn from(mm: f64) -> Self { Page } }\n",
            "impl Tr for <u8 as Id>::Out { const C: f32 = 0.0; }\n",
            "impl Wrap<f32> { pub fn new(lines: &[String]) -> Self { Wrap(0.0) } }\n",
            "macro_rules! interest { ($balance:expr) => { $balance as f64 * 1.01 }; }\n",
        ),
        &[
            (2, PDF_EXPORT),
            (3, PDF_EXPORT),
            (3, PDF_EXPORT),
            (4, PDF_EXPORT),
            (4, PDF_INPUT),
            (4, PDF_FLOAT),
            (5, PDF_EXPORT),
            (6, PDF_EXPORT),
            (7, PDF_EXPORT),
        ],
    ),
    // Private items and fields, an inherent impl without a visible item, and an impl type or
    // higher-ranked bound, which makes no trait impl, export nothing.
    (
        concat!(
            "pub(crate) fn render_statement(lines: &[String]) -> impl Iterator<Item = u8> {\n",
            "    let y: f32 = 0.0;\n",
            "    std::iter::empty()\n",
            "}\n",
            "const LEFT_MM: f32 = 25.0;\n",
            "struct Layout { left: f32 }\n",
            "impl Layout { fn y(&self) -> f32 { 0.0 } }\n",
            "pub struct Page { left: f32, pub rows: u16 }\n",
            "impl Wrap<f32> { fn new() -> Self { Wrap(0.0) } }\n",
            "pub(crate) fn each(lines: &[String]) -> impl for<'a> Fn(&'a u8) -> u8 {\n",
            "    let s: f32 = 1.0;\n",
            "    |_| 0\n",
            "}\n",
            "impl<F> Holder<F> where F: for<'a> Fn(&'a f32) { fn call(&self) {} }\n",
        ),
        &[],
    ),
];

#[test]
fn pdf_contract_self_check() {
    for &(src, expected) in PDF_CASES {
        let file = RustFile::new(PDF_MODULE.to_owned(), src.as_bytes().to_vec());
        let hits: Vec<(usize, &str)> = pdf_contract_hits(&file)
            .into_iter()
            .map(|(at, rule)| (line_of(&file.src, at), rule))
            .collect();
        assert_eq!(hits, expected, "{src}");
    }

    // The renderer as it stands keeps the contract.
    let real = RustFile::new(PDF_MODULE.to_owned(), read_bytes(&require_file(PDF_MODULE)));
    let real_hits = pdf_contract_hits(&real);
    assert!(real_hits.is_empty(), "{PDF_MODULE}: {real_hits:?}");
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
        "Could not Update Widget...",
        "Kindly delete nested widgets beforehand:",
        "Pick or update label of widget to update",
        "KINDLY SELECT ONE WIDGET TO VIEW OR UPDATE",
        "Delete skipped with reason:",
        "{path}:{line}: rule: excerpt",
        "create the account table",
        "drop a b c d table",
    ] {
        assert_eq!(sql_shape(negative), None, "{negative}");
    }
}

// Line numbers of the transactions-table hits in `src`, scanned as a non-owner source file.
fn transactions_lines(src: &str) -> Vec<usize> {
    let file = RustFile::new("crates/x/src/q.rs".to_owned(), src.as_bytes().to_vec());
    transactions_hits(&file)
        .into_iter()
        .map(|at| line_of(&file.src, at))
        .collect()
}

// The (line, shape) pairs of the formatted-SQL hits in `src`.
fn format_lines(src: &str) -> Vec<(usize, &'static str)> {
    let file = RustFile::new("crates/x/src/f.rs".to_owned(), src.as_bytes().to_vec());
    format_sql_hits(&file)
        .into_iter()
        .map(|(at, shape)| (line_of(&file.src, at), shape))
        .collect()
}

#[test]
fn sql_macro_delimiters_self_check() {
    for (text, call) in [
        ("m!( (a) [b] {c} ) d", "( (a) [b] {c} )"),
        ("m ! [ [a] (b) ] ]", "[ [a] (b) ]"),
        ("m!{ {a} [b] } }", "{ {a} [b] }"),
        ("m!\n(\n)", "(\n)"),
    ] {
        let code = text.as_bytes();
        let Some(open) = macro_open(code, 0, 1) else {
            panic!("no invocation found: {text}");
        };
        assert_eq!(
            &code[open..=close_delim(code, open)],
            call.as_bytes(),
            "{text}"
        );
    }
    for negative in ["m (a)", "m != (a)", "m! x(a)", "m!"] {
        assert_eq!(macro_open(negative.as_bytes(), 0, 1), None, "{negative}");
    }
    assert_eq!(close_delim(b"m!( (a", 2), 6);
    assert_eq!(close_delim(b"x", 0), 1);
    assert_eq!(close_paren(b"f(a, (b)) c", 1), 8);

    for positive in [
        "sqlx::query! { \"SELECT * FROM transactions\" };",
        "sqlx::query![ \"SELECT * FROM transactions\" ];",
        "query_as! { Row, \"select x from transactions\" }",
        "query_scalar ! [\"SELECT count(*) FROM transactions\"]",
    ] {
        assert_eq!(transactions_lines(positive), [1], "{positive}");
    }
    for (positive, shape) in [
        (
            "let a = format![\"SELECT a FROM {table}\"];",
            "SELECT ... FROM",
        ),
        ("let b = format!{\"delete from {t}\"};", "DELETE FROM"),
        ("let c = format ! [ \"insert into {t}\" ];", "INSERT INTO"),
    ] {
        assert_eq!(format_lines(positive), [(1, shape)], "{positive}");
    }

    let calls = RustFile::new(
        "crates/persistence/src/c.rs".to_owned(),
        concat!(
            "sqlx::query! { \"SELECT 1\" };\n",
            "sqlx::query_scalar![\"SELECT 2\"];\n",
            "query_as!(Row, \"SELECT 3\");\n",
        )
        .as_bytes()
        .to_vec(),
    );
    let delims: Vec<(u8, u8)> = query_macro_calls(&calls)
        .into_iter()
        .map(|(open, close, _)| (calls.code[open], calls.code[close]))
        .collect();
    assert_eq!(delims, [(b'{', b'}'), (b'[', b']'), (b'(', b')')]);
}

// The decoded bytes and source offsets of the literals in `src`, joined in source order.
fn decoded(src: &str) -> (Vec<u8>, Vec<usize>) {
    let mut value = LitValue::default();
    for span in lex_rust(src.as_bytes())
        .iter()
        .filter(|span| span.kind == Kind::Str)
    {
        push_literal(src.as_bytes(), span, &mut value);
    }
    (value.bytes, value.offsets)
}

#[test]
fn sql_literal_decoding_self_check() {
    for (literal, value) in [
        (r#""a\n\r\t\\\0\'\"b""#, b"a\n\r\t\\\0'\"b".as_slice()),
        (r#""\x41\x7a""#, b"Az"),
        (r#""\u{74}\u{0074}\u{1_F600}""#, "tt\u{1F600}".as_bytes()),
        (r#"b"\xff\x00""#, b"\xff\x00"),
        (r#"c"\x41""#, b"A"),
        ("\"a \\\n    b\"", b"a b"),
        ("\"a\\\r\n\t b\"", b"ab"),
        (r#"r"a\n\u{74}""#, br"a\n\u{74}"),
        (r##"r#"a\"b"#"##, br#"a\"b"#),
        (r#"br"\x41""#, br"\x41"),
        (r#"cr"\t""#, br"\t"),
        (r"'\''", b"'"),
        (r"b'\x41'", b"A"),
        (
            r#""\q \x4 \u{zz} \u74 \u{110000}""#,
            br"\q \x4 \u{zz} \u74 \u{110000}",
        ),
        (r#""trans" + "actions""#, b"transactions"),
    ] {
        assert_eq!(decoded(literal).0, value, "{literal}");
    }
    assert_eq!(
        decoded("\"a\\x41\\\n  b\\u{e9}\""),
        (b"aAb\xc3\xa9".to_vec(), vec![1, 2, 10, 11, 11])
    );
    assert_eq!(decoded(r#"r"a\n""#), (br"a\n".to_vec(), vec![2, 3, 4]));

    let code = b"m!(Pair<A, B>, f(x, y), [1, 2], {a, b},)";
    let args: Vec<&[u8]> = macro_args(code, 2, code.len() - 1)
        .into_iter()
        .map(|(left, right)| &code[left + 1..right])
        .collect();
    assert_eq!(
        args,
        [
            b"Pair<A".as_slice(),
            b" B>",
            b" f(x, y)",
            b" [1, 2]",
            b" {a, b}",
            b""
        ]
    );
}

#[test]
fn query_sql_source_self_check() {
    for positive in [
        r#"query!("SELECT * FROM transac\u{0074}ions")"#,
        r#"query!("SELECT * FROM \x74ransactions WHERE x = 1")"#,
        r#"query!("SELECT * FROM trans" + "actions")"#,
        r#"query_as!(Row, "SELECT x FROM " + "transactions")"#,
        r#"query_as!(Pair<A, B>, "SELECT a, b FROM transactions", id)"#,
        r#"query_scalar!(/* "x", */ "SELECT 1 FROM public.Transactions", "x")"#,
        "query!(\"SELECT * FROM trans\\\n        actions\")",
        r#"query_as!(Pair<A, B>, "SELECT 1 FROM transactions")"#,
        r#"query_as!(crate::rows::Row<'static, fn() -> u8>, "SELECT 1 FROM transactions")"#,
        r#"query_as!(Map<fn() -> u8, i64>, "SELECT 1 FROM transactions")"#,
        r#"query_as!(Wrap<Pair<A, B>>, "SELECT 1 FROM transactions")"#,
        r#"query_as_unchecked!(Row::<{ 1 < 2 }>, "SELECT 1 FROM " + "transactions")"#,
        r#"query_unchecked!("SELECT 1 FROM transactions", id)"#,
        r#"query_scalar_unchecked!("SELECT count(*) FROM transactions")"#,
    ] {
        assert_eq!(transactions_lines(positive), [1], "{positive}");
    }
    for negative in [
        r#"query!("SELECT $1::text AS label", "transactions")"#,
        r#"query_as!(Row, "SELECT 1 FROM transaction_master", "transactions")"#,
        r#"query_as!(Pair<A, B>, "SELECT 1", f("transactions", 2))"#,
        r#"query!(r"SELECT * FROM transac\u{0074}ions")"#,
        r##"query!(r#"SELECT * FROM \x74ransactions"#)"##,
        r#"query!("SELECT * FROM transactions_archive")"#,
        r#"let s = "transactions"; query!("SELECT 1")"#,
        r#"query_as!(Row::<{ "transactions".len() }>, "SELECT 1 FROM transaction_master")"#,
        r#"query!("SELECT 1", Row::<{ "transactions" }>)"#,
        r#"query_as!(Row::<{ "transactions" }>)"#,
    ] {
        assert!(transactions_lines(negative).is_empty(), "{negative}");
    }

    let record_block = concat!(
        "let row = sqlx::query_as!(\n",
        "    Row::<{ \"not SQL\".len() }>,\n",
        "    \"SELECT 1 AS \\\"id!\\\" FROM transactions\"\n",
        ");\n",
    );
    assert_eq!(transactions_lines(record_block), [3]);

    let multi = concat!(
        "let rows = sqlx::query!(\n",
        "    \"SELECT t.id \"\n",
        "        + \"FROM \"\n",
        "        + \"transactions t\",\n",
        "    id\n",
        ");\n",
        "let other = query!(\"SELECT $1\", \"transactions\");\n",
        "let more = query_as![\n",
        "    Row,\n",
        "    \"SELECT * FROM trans\"\n",
        "        + \"actions\"\n",
        "];\n",
        "let cont = query!(\"SELECT * \\\n",
        "    FROM \\u{74}ransactions\");\n",
    );
    assert_eq!(transactions_lines(multi), [4, 10, 14]);
}

#[test]
fn format_string_self_check() {
    for (positive, shape) in [
        (
            r#"format!("SELECT a FRO\u{004d} {table}")"#,
            "SELECT ... FROM",
        ),
        (r#"format!("INSERT\x20INTO {t} VALUES (1)")"#, "INSERT INTO"),
        (
            r#"format!(concat!("DELETE ", "FROM {t}"), t = n)"#,
            "DELETE FROM",
        ),
        (
            "format!(\"UPDATE t \\\n    SET x = {v}\")",
            "UPDATE ... SET",
        ),
    ] {
        assert_eq!(format_lines(positive), [(1, shape)], "{positive}");
    }
    for negative in [
        r#"format!("{}", "select x from y")"#,
        r#"format!(r"SELECT a FRO\u{004d} t")"#,
        r#"format!("{label}", label = "delete from t")"#,
        r#"format!("{}: {}", "SELECT a", "FROM t")"#,
    ] {
        assert!(format_lines(negative).is_empty(), "{negative}");
    }
    let nested = concat!(
        "let a = format!(\n",
        "    concat!(\n",
        "        \"SELECT a \",\n",
        "        \"FROM {t}\"\n",
        "    ),\n",
        ");\n",
    );
    assert_eq!(format_lines(nested), [(3, "SELECT ... FROM")]);
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
fn float_scan_coverage_self_check() {
    // The crates whose COVERED files a case drops, the files it adds, the directories it
    // marks absent, and the gaps it expects.
    type Case = (
        &'static [&'static str],
        &'static [&'static str],
        &'static [&'static str],
        Vec<String>,
    );
    // One scanned file per financial crate, a nested one and the PDF module included.
    const COVERED: [&str; 7] = [
        "crates/domain/src/lib.rs",
        "crates/codec/src/lib.rs",
        "crates/persistence/src/lib.rs",
        "crates/api/src/routes/accounts.rs",
        "crates/batch/src/main.rs",
        "crates/batch/src/pdf.rs",
        "crates/messaging/src/lib.rs",
    ];
    let missing = |src: &str| {
        format!("hygiene input directory {src} is missing, so the float scan would pass vacuously")
    };
    let empty = |src: &str| {
        format!("{src} holds no scanned .rs file, so the float scan would pass vacuously")
    };
    let cases: [Case; 8] = [
        (&[], &[], &[], Vec::new()),
        (
            &["persistence"],
            &["crates/persistence/build.rs"],
            &["crates/persistence/src"],
            vec![missing("crates/persistence/src")],
        ),
        (
            &["api"],
            &["crates/api/tests/x.rs"],
            &["crates/api/src"],
            vec![missing("crates/api/src")],
        ),
        (
            &["batch"],
            &["crates/batch/src/pdf.rs"],
            &[],
            vec![empty("crates/batch/src")],
        ),
        (&["codec"], &[], &[], vec![empty("crates/codec/src")]),
        (
            &["codec"],
            &[],
            &["crates/codec/src"],
            vec![missing("crates/codec/src")],
        ),
        (
            &["messaging"],
            &[
                "crates/messaging/src/notes.md",
                "crates/messaging/build.rs",
                "crates/messaging/tests/x.rs",
                "crates/messaging/srcs/lib.rs",
                "crates/runtime/src/lib.rs",
                "crates/testkit/src/lib.rs",
            ],
            &[],
            vec![empty("crates/messaging/src")],
        ),
        (
            &["codec", "persistence", "api", "batch"],
            &[
                "crates/persistence/build.rs",
                "crates/api/tests/x.rs",
                "crates/batch/src/pdf.rs",
            ],
            &["crates/persistence/src", "crates/api/src"],
            vec![
                empty("crates/codec/src"),
                missing("crates/persistence/src"),
                missing("crates/api/src"),
                empty("crates/batch/src"),
            ],
        ),
    ];
    for (dropped, added, absent, expected) in cases {
        let mut files: Vec<&str> = COVERED
            .into_iter()
            .filter(|rel| {
                !rel.split('/')
                    .nth(1)
                    .is_some_and(|name| dropped.contains(&name))
            })
            .collect();
        files.extend(added);
        let gaps = float_scan_gaps(&files, |dir| !absent.contains(&dir));
        assert_eq!(gaps, expected, "files {files:?}, absent {absent:?}");
    }
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
    const WORKSPACE: &str = "[workspace.dependencies]\nsqlx = \"0.8\"\nchrono = \"0.4\"\n\
                             aliased = { package = \"printpdf\", version = \"0.7\" }\n";
    let declared = declarations(SAMPLE, WORKSPACE)
        .unwrap_or_else(|error| panic!("line {}: {}", error.line, error.message));
    // (line, kind, target, (key, canonical package)); a table declaration has no dependency.
    let resolved: Vec<_> = declared
        .iter()
        .map(|declaration| {
            let dependency = declaration
                .dependency
                .as_ref()
                .map(|dependency| (dependency.key.as_str(), canonical(&dependency.package)));
            let table = &declaration.table;
            (
                declaration.line,
                table.kind,
                table.target.as_deref(),
                dependency,
            )
        })
        .collect();
    let unix = Some("cfg(unix)");
    let expected = [
        (8, DepKind::Normal, None, None),
        (9, DepKind::Normal, None, Some(("sqlx", "sqlx"))),
        (10, DepKind::Normal, None, Some(("chrono", "chrono"))),
        (13, DepKind::Normal, None, Some(("renamed", "lapin"))),
        (14, DepKind::Normal, None, Some(("aliased", "printpdf"))),
        (16, DepKind::Dev, None, None),
        (
            17,
            DepKind::Dev,
            None,
            Some(("carddemo-testkit", "carddemo-testkit")),
        ),
        (19, DepKind::Build, unix, None),
        (20, DepKind::Build, unix, Some(("tokio", "tokio"))),
        (22, DepKind::Normal, None, Some(("foo", "serde-json"))),
    ];
    assert_eq!(resolved.len(), expected.len(), "{resolved:?}");
    for (got, want) in resolved.iter().zip(expected) {
        let dependency = got
            .3
            .as_ref()
            .map(|(key, package)| (*key, package.as_str()));
        assert_eq!((got.0, got.1, got.2, dependency), want);
    }

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

#[test]
fn toml_reader_self_check() {
    let read = |text: &str| {
        entries(text)
            .map(|read| {
                read.into_iter()
                    .map(|entry| (entry.line, entry.path.join("/"), entry.value))
                    .collect::<Vec<_>>()
            })
            .map_err(|error| error.line)
    };
    let text = |value: &str| Value::Str(value.to_owned());
    let other = |value: &str| Value::Other(value.to_owned());
    let cases = [
        (
            "a = \"t\\u006fkio\\U0001F600 \\\"q\\\" \\\\\"\n",
            Ok(vec![(1, "a", text("tokio\u{1F600} \"q\" \\"))]),
        ),
        (
            "a = 't\\u006fkio'\n",
            Ok(vec![(1, "a", text("t\\u006fkio"))]),
        ),
        (
            "\"t\\u006fkio\".version = '1'\n",
            Ok(vec![(1, "tokio/version", text("1"))]),
        ),
        (
            "[ target . 'cfg(unix)' . \"dev-dependencies\" ]\n",
            Ok(vec![(1, "target/cfg(unix)/dev-dependencies", Value::Table)]),
        ),
        (
            "[[bench]]\nname = \"x\" # comment\n",
            Ok(vec![
                (1, "bench", Value::Table),
                (2, "bench/name", text("x")),
            ]),
        ),
        (
            "a = \"\"\"\none \\\n    two\"\"\"\"\nb = 1\n",
            Ok(vec![(1, "a", text("one two\"")), (4, "b", other("1"))]),
        ),
        (
            "a = '''\nkeep \\n ''x'''''\n",
            Ok(vec![(1, "a", text("keep \\n ''x''"))]),
        ),
        (
            "a = [\n  \"x\", # note\n  1,\n]\nb = 2\n",
            Ok(vec![
                (1, "a", Value::Array(vec![text("x"), other("1")])),
                (5, "b", other("2")),
            ]),
        ),
        (
            "d = { e = {}, f.g = 'h', i = [{ j = 1 }] }\n",
            Ok(vec![
                (1, "d", Value::Table),
                (1, "d/e", Value::Table),
                (1, "d/f/g", text("h")),
                (1, "d/i", Value::Array(vec![Value::Table])),
            ]),
        ),
        (
            "t = 1979-05-27 07:32:00Z\nn = -1\n",
            Ok(vec![
                (1, "t", other("1979-05-27 07:32:00Z")),
                (2, "n", other("-1")),
            ]),
        ),
        ("a = \"\\q\"\n", Err(1)),
        ("a = \"\\uD800\"\n", Err(1)),
        ("a = \"\\u00\"\n", Err(1)),
        ("\na = \"open\n", Err(2)),
        ("a = 'x\ny'\n", Err(1)),
        ("a = \"x\" trailing\n", Err(1)),
        ("a = 1 2\n", Err(1)),
        ("[a\n", Err(1)),
        ("a = { b = 1, }\n", Err(1)),
        ("a = \"\"\"x\"\"\"\"\"\"\n", Err(1)),
        ("\n\nnot a statement\n", Err(3)),
        ("= 1\n", Err(1)),
    ];
    for (input, expected) in cases {
        let expected = expected.map(|items| {
            items
                .into_iter()
                .map(|(line, path, value)| (line, path.to_owned(), value))
                .collect::<Vec<_>>()
        });
        assert_eq!(read(input), expected, "{input:?}");
    }
}

// The 1-based number of the line of `text` that reads `wanted`.
fn line_number(text: &str, wanted: &str) -> usize {
    text.lines()
        .position(|line| line == wanted)
        .map_or_else(|| panic!("no line reads `{wanted}`"), |at| at + 1)
}

#[test]
fn crate_edges_reject_planted_declarations() {
    let root_text = read_text(&require_file("Cargo.toml"));
    let (codec_rel, codec) = crate_manifest("codec");
    let (audit_rel, audit) = crate_manifest("audit");
    let in_dependencies = |line: &str| {
        assert!(
            codec.contains("[dependencies]\n"),
            "{codec_rel} has no [dependencies] table, so the planted case would be vacuous"
        );
        codec.replacen("[dependencies]\n", &format!("[dependencies]\n{line}\n"), 1)
    };
    let tokio_alias = "wire = { package = \"tokio\", version = \"1\" }";
    let two_targets = format!(
        "{codec}\n[target.'cfg(unix)'.dependencies]\n{tokio_alias}\n\n\
         [target.'cfg(windows)'.dependencies]\nwire = {{ package = \"chrono\", version = \"0.4\" }}\n"
    );
    let codec_tokio = "forbidden edge from carddemo-codec (tokio)";
    let literal = "unreadable dependency declaration \
                   (`t\\u006fkio` is not a crate name the dependency checks can vouch for)";
    let audit_table = "carddemo-audit declares a dependency table";
    let escaped_package = "wire = { package = \"t\\u006fkio\", version = \"1\" }";
    let escaped_key = "\"t\\u006fkio\" = \"1\"";
    let literal_key = "'t\\u006fkio' = \"1\"";
    let literal_package = "wire = { package = 't\\u006fkio', version = \"1\" }";
    let mut cases = vec![
        // One alias under two platform tables stays two declarations.
        (
            "codec",
            &codec_rel,
            two_targets,
            Some((tokio_alias, codec_tokio)),
        ),
        // Basic strings and quoted keys are decoded; literal strings keep their backslash.
        (
            "codec",
            &codec_rel,
            in_dependencies(escaped_package),
            Some((escaped_package, codec_tokio)),
        ),
        (
            "codec",
            &codec_rel,
            in_dependencies(escaped_key),
            Some((escaped_key, codec_tokio)),
        ),
        (
            "codec",
            &codec_rel,
            in_dependencies(literal_key),
            Some((literal_key, literal)),
        ),
        (
            "codec",
            &codec_rel,
            in_dependencies(literal_package),
            Some((literal_package, literal)),
        ),
        ("codec", &codec_rel, codec.clone(), None),
        ("audit", &audit_rel, audit.clone(), None),
    ];
    // An empty inline dependency table is still a dependency table.
    for table in [
        "dependencies = {}",
        "dev-dependencies = {}",
        "build-dependencies = {}",
        "target.'cfg(unix)'.dependencies = {}",
    ] {
        cases.push((
            "audit",
            &audit_rel,
            format!("{table}\n{audit}"),
            Some((table, audit_table)),
        ));
    }
    for (name, rel, text, planted) in cases {
        let mut violations = Vec::new();
        crate_edges(&mut violations, name, rel, &text, &root_text);
        let expected: Vec<String> = planted
            .map(|(line, rule)| format!("{rel}:{}: {rule}: {line}", line_number(&text, line)))
            .into_iter()
            .collect();
        assert_eq!(violations, expected, "{text}");
    }
}

// The argument files of a synthetic file system, by the path written after `@`, so the
// flag self-checks read and write no file.
const ARG_FILES: [(&str, &str); 7] = [
    ("cap.flags", "--cap-lints\nallow\n"),
    ("ok.flags", "-D\nwarnings\n"),
    ("crlf.flags", "--cap-lints\r\nallow\r\n"),
    ("level.flags", "allow\n"),
    ("tail.flags", "-D\nwarnings\n-A\n"),
    ("warnings.flags", "warnings"),
    ("nested.flags", "-D\nwarnings\n@cap.flags\n"),
];

// The text of the synthetic argument file at `path`, or None when ARG_FILES has none.
fn arg_file(path: &str) -> Option<String> {
    ARG_FILES
        .iter()
        .find(|&&(name, _)| name == path)
        .map(|&(_, text)| text.to_owned())
}

// Compiler argument lists, with the rules flag_rules reports for them over ARG_FILES.
const FLAG_RULE_CASES: &[(&[&str], &[&str])] = &[
    (
        &["--cap-lints", "allow"],
        &["rustflags relax or cap lints (--cap-lints allow)"],
    ),
    (
        &["--cap-lints=warn"],
        &["rustflags relax or cap lints (--cap-lints=warn)"],
    ),
    (
        &["-D", "warnings", "--cap-lints"],
        &["rustflags relax or cap lints (--cap-lints)"],
    ),
    (
        &["-A", "warnings"],
        &["rustflags relax or cap lints (-A warnings)"],
    ),
    (
        &["-Awarnings"],
        &["rustflags relax or cap lints (-Awarnings)"],
    ),
    (
        &["-W", "unused"],
        &["rustflags relax or cap lints (-W unused)"],
    ),
    (&["-Wunused"], &["rustflags relax or cap lints (-Wunused)"]),
    (
        &["--allow", "dead_code"],
        &["rustflags relax or cap lints (--allow dead_code)"],
    ),
    (
        &["--allow=dead_code"],
        &["rustflags relax or cap lints (--allow=dead_code)"],
    ),
    (
        &["--warn", "missing_docs"],
        &["rustflags relax or cap lints (--warn missing_docs)"],
    ),
    (
        &["--warn=missing_docs"],
        &["rustflags relax or cap lints (--warn=missing_docs)"],
    ),
    (
        &["--force-warn", "missing_docs"],
        &["rustflags relax or cap lints (--force-warn missing_docs)"],
    ),
    (
        &["--force-warn=missing_docs"],
        &["rustflags relax or cap lints (--force-warn=missing_docs)"],
    ),
    (
        &["-Z", "crate-attr=allow(warnings)"],
        &["rustflags relax or cap lints (-Z crate-attr=allow(warnings))"],
    ),
    (
        &["-Zcrate-attr=expect(dead_code)"],
        &["rustflags relax or cap lints (-Zcrate-attr=expect(dead_code))"],
    ),
    (
        &["-Zcrate_attr=cfg_attr(all(), warn(missing_docs))"],
        &["rustflags relax or cap lints (-Zcrate_attr=cfg_attr(all(), warn(missing_docs)))"],
    ),
    // Argument files expand in place, one argument per line, and keep their source.
    (
        &["@cap.flags"],
        &["rustflags relax or cap lints (--cap-lints allow from @cap.flags)"],
    ),
    (
        &["-D", "warnings", "@cap.flags"],
        &["rustflags relax or cap lints (--cap-lints allow from @cap.flags)"],
    ),
    (
        &["@crlf.flags"],
        &["rustflags relax or cap lints (--cap-lints allow from @crlf.flags)"],
    ),
    (&["@ok.flags"], &[]),
    // A switch and its value may sit on either side of a file boundary.
    (
        &["--cap-lints", "@level.flags"],
        &["rustflags relax or cap lints (--cap-lints allow from @level.flags)"],
    ),
    (
        &["@tail.flags", "warnings"],
        &["rustflags relax or cap lints (-A warnings from @tail.flags)"],
    ),
    (
        &["@tail.flags", "@warnings.flags"],
        &["rustflags relax or cap lints (-A warnings from @tail.flags and @warnings.flags)"],
    ),
    // A file the gate cannot follow fails closed, one violation each, in argument order.
    (
        &["@missing.flags"],
        &["rustflags argument file cannot be checked (@missing.flags: unreadable or not UTF-8)"],
    ),
    (
        &["@"],
        &["rustflags argument file cannot be checked (@: unreadable or not UTF-8)"],
    ),
    (
        &["@nested.flags"],
        &[
            "rustflags argument file cannot be checked (@nested.flags: line 3 is @cap.flags, \
               which rustc does not expand)",
        ],
    ),
    (
        &["@shell:x"],
        &["rustflags argument file cannot be checked (@shell:x: unstable shell form)"],
    ),
    (
        &["-A", "warnings", "@missing.flags", "@cap.flags"],
        &[
            "rustflags relax or cap lints (-A warnings)",
            "rustflags argument file cannot be checked (@missing.flags: unreadable or not UTF-8)",
            "rustflags relax or cap lints (--cap-lints allow from @cap.flags)",
        ],
    ),
    (
        &[
            "-D",
            "warnings",
            "-Dwarnings",
            "-F",
            "missing_docs",
            "--deny",
            "missing_docs",
            "--forbid=missing_docs",
            "-C",
            "link-arg=-Wl,--as-needed",
            "-Clink-arg=-Wl,--as-needed",
            "--allow-features",
            "-Zcrate-attr=deny(warnings)",
            "-Z",
            "unstable-options",
        ],
        &[],
    ),
];

#[test]
fn relaxing_flag_parser_self_check() {
    for (args, expected) in FLAG_RULE_CASES {
        assert_eq!(flag_rules(args, &arg_file), expected.to_vec(), "{args:?}");
    }
}

#[test]
fn cargo_config_flags_self_check() {
    const REL: &str = ".cargo/config.toml";
    let cases: &[(&str, &[&str])] = &[
        (
            "[build]\nrustflags = [\"\\u002dA\", \"warnings\"]\n",
            &[
                ".cargo/config.toml:2: rustflags relax or cap lints (-A warnings): \
               rustflags = [\"\\u002dA\", \"warnings\"]",
            ],
        ),
        (
            "build.rustdocflags = '--cap-lints=allow'\n",
            &[
                ".cargo/config.toml:1: rustflags relax or cap lints (--cap-lints=allow): \
               build.rustdocflags = '--cap-lints=allow'",
            ],
        ),
        (
            "[env]\nRUSTFLAGS = { value = \"-Awarnings\", force = true }\n",
            &[
                ".cargo/config.toml:2: rustflags relax or cap lints (-Awarnings): \
               RUSTFLAGS = { value = \"-Awarnings\", force = true }",
            ],
        ),
        (
            "[build]\nrustflags = [1]\n",
            &[".cargo/config.toml:2: rustflags element 1 is not a string: rustflags = [1]"],
        ),
        (
            "[build]\nrustflags = true\n",
            &[
                ".cargo/config.toml:2: rustflags value is neither a string nor an array: \
               rustflags = true",
            ],
        ),
        (
            "[build]\nrustflags = \"\\q\"\n",
            &[
                ".cargo/config.toml:2: unreadable TOML (invalid escape sequence `\\q`): \
               rustflags = \"\\q\"",
            ],
        ),
        (
            "[build]\nrustflags = \"@cap.flags\"\n",
            &[
                ".cargo/config.toml:2: rustflags relax or cap lints (--cap-lints allow from \
               @cap.flags): rustflags = \"@cap.flags\"",
            ],
        ),
        (
            "[build]\nrustflags = [\"-D\", \"warnings\", \"@cap.flags\"]\n",
            &[
                ".cargo/config.toml:2: rustflags relax or cap lints (--cap-lints allow from \
               @cap.flags): rustflags = [\"-D\", \"warnings\", \"@cap.flags\"]",
            ],
        ),
        (
            "[target.'cfg(unix)']\nrustdocflags = [\"@missing.flags\"]\n",
            &[
                ".cargo/config.toml:2: rustflags argument file cannot be checked (@missing.flags: \
               unreadable or not UTF-8): rustdocflags = [\"@missing.flags\"]",
            ],
        ),
        ("[build]\nrustflags = \"-D warnings\"\n", &[]),
        ("[build]\nrustflags = [\"@ok.flags\"]\n", &[]),
        (
            "[target.x86_64-unknown-linux-gnu]\nrustflags = [\"-C\", \"link-arg=-Wl,--as-needed\"]\n",
            &[],
        ),
    ];
    for (text, expected) in cases {
        let mut violations = Vec::new();
        config_flag_violations(&mut violations, REL, text, &arg_file);
        assert_eq!(violations, expected.to_vec(), "{text}");
    }
}

#[test]
fn environment_flags_self_check() {
    use std::ffi::OsString;

    // The (name, value) pairs of a synthetic environment, and the violations they yield.
    type Case<'a> = (&'a [(&'a str, &'a str)], &'a [&'a str]);

    let cases: &[Case] = &[
        (&[("RUSTFLAGS", "-D warnings")], &[]),
        (
            &[("RUSTFLAGS", "--cap-lints allow")],
            &["environment variable RUSTFLAGS: rustflags relax or cap lints (--cap-lints allow)"],
        ),
        (
            &[("RUSTFLAGS", "--cap-lints=allow")],
            &["environment variable RUSTFLAGS: rustflags relax or cap lints (--cap-lints=allow)"],
        ),
        (&[("CARGO_ENCODED_RUSTFLAGS", "-D\u{1f}warnings")], &[]),
        (
            &[("CARGO_ENCODED_RUSTFLAGS", "-A\u{1f}warnings")],
            &[
                "environment variable CARGO_ENCODED_RUSTFLAGS: rustflags relax or cap lints \
               (-A warnings)",
            ],
        ),
        (
            &[(
                "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS",
                "-Wunused",
            )],
            &[
                "environment variable CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUSTFLAGS: \
               rustflags relax or cap lints (-Wunused)",
            ],
        ),
        (
            &[("RUSTDOCFLAGS", "--allow=missing_docs")],
            &[
                "environment variable RUSTDOCFLAGS: rustflags relax or cap lints \
               (--allow=missing_docs)",
            ],
        ),
        (
            &[("CARGO_ENCODED_RUSTDOCFLAGS", "-Zcrate-attr=allow(warnings)")],
            &[
                "environment variable CARGO_ENCODED_RUSTDOCFLAGS: rustflags relax or cap lints \
               (-Zcrate-attr=allow(warnings))",
            ],
        ),
        (
            &[("PATH", "-A warnings"), ("RUSTFLAGS_NOTE", "-A warnings")],
            &[],
        ),
        (
            &[("RUSTFLAGS", "@cap.flags")],
            &[
                "environment variable RUSTFLAGS: rustflags relax or cap lints (--cap-lints allow \
               from @cap.flags)",
            ],
        ),
        (
            &[(
                "CARGO_ENCODED_RUSTFLAGS",
                "-D\u{1f}warnings\u{1f}@cap.flags",
            )],
            &[
                "environment variable CARGO_ENCODED_RUSTFLAGS: rustflags relax or cap lints \
               (--cap-lints allow from @cap.flags)",
            ],
        ),
        (
            &[("RUSTDOCFLAGS", "@missing.flags")],
            &[
                "environment variable RUSTDOCFLAGS: rustflags argument file cannot be checked \
               (@missing.flags: unreadable or not UTF-8)",
            ],
        ),
        (&[("RUSTFLAGS", "@ok.flags")], &[]),
        (&[("CARGO_ENCODED_RUSTFLAGS", "@ok.flags")], &[]),
    ];
    for (vars, expected) in cases {
        let pairs = vars
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)));
        assert_eq!(
            env_flag_violations(pairs, &arg_file),
            expected.to_vec(),
            "{vars:?}"
        );
    }

    // A value that is not UTF-8 cannot be checked, so it fails closed.
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;

        let invalid = OsString::from_vec(vec![b'-', b'A', 0xff]);
        assert_eq!(
            env_flag_violations([(OsString::from("RUSTFLAGS"), invalid)], &arg_file),
            ["environment variable RUSTFLAGS: value is not UTF-8, so its flags cannot be checked"]
        );
    }
}
