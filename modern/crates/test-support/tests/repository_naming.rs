//! Repository Method Naming (AAP 0.5.4, 0.10): a repository method's name never repeats the
//! entity its trait names, so `AccountRepository` declares `find_by_id`, never a method whose
//! name holds `account`.
//!
//! The test reads every Rust file under `modern/crates/*/src` and `modern/crates/*/tests`, finds
//! each `trait <Entity>Repository` and each `impl <Entity>Repository for …` (test doubles
//! included), derives the entity's snake-case noun and fails when the name of a method declared
//! directly in that trait or implementation contains the noun.
//!
//! Run it from `modern/` with
//! `cargo test -p test-support --locked --test repository_naming -- --nocapture`. It only reads
//! files: no container, database or async runtime.
//!
//! The scan works on tokens, not on text. A small lexer drops comments, string literals, raw
//! strings and char literals, so a name in prose or in a fixture string never matches and a brace
//! inside one never unbalances a body. The lexer is total, because the scan also reads the
//! deliberately invalid sources under `crates/domain/tests/compile_fail/`: an unterminated
//! comment or string simply ends the token stream.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use test_support::modern_dir;

/// The suffix that marks a repository trait; the entity is the part of the name before it.
const SUFFIX: &str = "Repository";

/// One lexical token. Comments, string literals and char literals produce none.
#[derive(Debug)]
enum Kind {
    /// An identifier or keyword; a raw identifier `r#name` yields `name`.
    Ident(String),
    /// A lifetime or label such as `'a`, `'_` or `'static`.
    Lifetime,
    /// A number such as `0`, `1u8` or `0x7F`.
    Literal,
    /// `->`, a token of its own so that a `Fn() -> T` bound cannot unbalance a generic list.
    Arrow,
    /// `=>`.
    FatArrow,
    /// `::`.
    PathSep,
    /// Any other punctuation character.
    Punct(char),
}

/// A token and the 1-based line it starts on.
#[derive(Debug)]
struct Token {
    kind: Kind,
    line: usize,
}

/// Splits Rust source into [`Token`]s.
///
/// Every character is read through `get`, so input that ends inside a comment, string, raw
/// string or char literal ends the token stream instead of panicking, and every step consumes at
/// least one character, so the lexer always terminates.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    tokens: Vec<Token>,
}

impl Lexer {
    /// The tokens of `src`.
    fn tokenize(src: &str) -> Vec<Token> {
        let mut lexer = Self {
            chars: src.chars().collect(),
            pos: 0,
            line: 1,
            tokens: Vec::new(),
        };
        while let Some(c) = lexer.peek(0) {
            lexer.step(c);
        }
        lexer.tokens
    }

    /// Lexes whatever starts with `c`, the character at the current position.
    fn step(&mut self, c: char) {
        let next = self.peek(1);
        if c.is_whitespace() {
            self.bump();
        } else if c == '/' && next == Some('/') {
            self.skip_line_comment();
        } else if c == '/' && next == Some('*') {
            self.skip_block_comment();
        } else if c == '"' {
            self.bump();
            self.skip_string();
        } else if c == '\'' {
            self.lex_quote();
        } else if c.is_ascii_digit() {
            self.lex_number();
        } else if c == '_' || c.is_alphabetic() {
            self.lex_word(c, next);
        } else {
            self.lex_punct(c, next);
        }
    }

    /// The character `offset` places after the current position, if the input has one.
    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos.saturating_add(offset)).copied()
    }

    /// Consumes the current character, counting the line it ends.
    fn bump(&mut self) -> Option<char> {
        let c = self.peek(0)?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
        }
        Some(c)
    }

    fn emit(&mut self, kind: Kind, line: usize) {
        self.tokens.push(Token { kind, line });
    }

    /// Skips `//` up to the end of the line, which covers `///` and `//!` doc comments.
    fn skip_line_comment(&mut self) {
        while self.peek(0).is_some_and(|c| c != '\n') {
            self.bump();
        }
    }

    /// Skips `/* … */`, which nests in Rust.
    fn skip_block_comment(&mut self) {
        self.bump();
        self.bump();
        let mut depth: usize = 1;
        while depth > 0 {
            match (self.peek(0), self.peek(1)) {
                (None, _) => return,
                (Some('/'), Some('*')) => {
                    self.bump();
                    self.bump();
                    depth += 1;
                }
                (Some('*'), Some('/')) => {
                    self.bump();
                    self.bump();
                    depth -= 1;
                }
                _ => {
                    self.bump();
                }
            }
        }
    }

    /// Skips the rest of a string literal whose opening quote is consumed, `\` escapes included.
    fn skip_string(&mut self) {
        while let Some(c) = self.bump() {
            match c {
                '\\' => {
                    self.bump();
                }
                '"' => return,
                _ => {}
            }
        }
    }

    /// At `r`, `br` or `cr`, `prefix` characters long: lexes a raw string when `#`s and a `"`
    /// follow the prefix, and reports whether it did. The string ends at a `"` followed by as
    /// many `#`s as it opened with.
    fn try_raw_string(&mut self, prefix: usize) -> bool {
        let mut hashes: usize = 0;
        while self.peek(prefix + hashes) == Some('#') {
            hashes += 1;
        }
        if self.peek(prefix + hashes) != Some('"') {
            return false;
        }
        for _ in 0..=prefix + hashes {
            self.bump();
        }
        while let Some(c) = self.bump() {
            if c == '"' && (0..hashes).all(|offset| self.peek(offset) == Some('#')) {
                for _ in 0..hashes {
                    self.bump();
                }
                break;
            }
        }
        true
    }

    /// At `'`: an escaped char literal (`'\''`, `'\u{7b}'`), a plain char literal (`'{'`) or a
    /// lifetime (`'a`). Char literals are skipped; a lifetime becomes a token.
    fn lex_quote(&mut self) {
        let line = self.line;
        if self.peek(1) == Some('\\') {
            // The quote, the backslash and the escaped character, then up to the closing quote.
            self.bump();
            self.bump();
            self.bump();
            while self.bump().is_some_and(|c| c != '\'') {}
        } else if self.peek(2) == Some('\'') {
            self.bump();
            self.bump();
            self.bump();
        } else {
            self.bump();
            while self.peek(0).is_some_and(is_ident_char) {
                self.bump();
            }
            self.emit(Kind::Lifetime, line);
        }
    }

    /// At a word start `c`, followed by `next`: a raw string, a byte or C string, a byte char
    /// literal, a raw identifier or a plain identifier.
    fn lex_word(&mut self, c: char, next: Option<char>) {
        let raw_prefix = match (c, next) {
            ('r', _) => Some(1),
            ('b' | 'c', Some('r')) => Some(2),
            _ => None,
        };
        if raw_prefix.is_some_and(|prefix| self.try_raw_string(prefix)) {
            return;
        }
        if matches!(c, 'b' | 'c') && next == Some('"') {
            self.bump();
            self.bump();
            self.skip_string();
        } else if c == 'b' && next == Some('\'') {
            self.bump();
            self.lex_quote();
        } else {
            if c == 'r' && next == Some('#') {
                // `r#` not followed by a raw string opens a raw identifier.
                self.bump();
                self.bump();
            }
            self.lex_ident();
        }
    }

    fn lex_ident(&mut self) {
        let line = self.line;
        let mut name = String::new();
        while let Some(c) = self.peek(0).filter(|&c| is_ident_char(c)) {
            name.push(c);
            self.bump();
        }
        if !name.is_empty() {
            self.emit(Kind::Ident(name), line);
        }
    }

    /// A number runs over alphanumerics and `_`, so a suffixed literal such as `1u8` is one
    /// token.
    fn lex_number(&mut self) {
        let line = self.line;
        while self.peek(0).is_some_and(is_ident_char) {
            self.bump();
        }
        self.emit(Kind::Literal, line);
    }

    fn lex_punct(&mut self, c: char, next: Option<char>) {
        let line = self.line;
        let pair = match (c, next) {
            ('-', Some('>')) => Some(Kind::Arrow),
            ('=', Some('>')) => Some(Kind::FatArrow),
            (':', Some(':')) => Some(Kind::PathSep),
            _ => None,
        };
        self.bump();
        if pair.is_some() {
            self.bump();
        }
        self.emit(pair.unwrap_or(Kind::Punct(c)), line);
    }
}

fn is_ident_char(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// What a scan found: the repository traits and implementations whose bodies it checked, the
/// methods it checked in them, and every method whose name repeats its entity.
#[derive(Debug, Default)]
struct ScanReport {
    traits: usize,
    impls: usize,
    methods: usize,
    /// One message per offending method, and one per path that cannot be read.
    violations: Vec<String>,
    /// The offending methods, in source order.
    flagged: Vec<Flagged>,
}

impl ScanReport {
    fn absorb(&mut self, other: Self) {
        self.traits += other.traits;
        self.impls += other.impls;
        self.methods += other.methods;
        self.violations.extend(other.violations);
        self.flagged.extend(other.flagged);
    }
}

/// A method whose name repeats its entity: the trait or implementation it sits in, its name and
/// the line of its name.
#[derive(Debug)]
struct Flagged {
    label: String,
    method: String,
    line: usize,
}

/// A repository trait or implementation with a body: its label in messages, its entity (the
/// trait name less [`SUFFIX`]) and the index of the body's `{`.
struct Block<'t> {
    label: String,
    entity: &'t str,
    open: usize,
}

/// Scans one source file, shown in messages as `display`, and reports what it found.
fn scan_source(display: &str, src: &str) -> ScanReport {
    let tokens = Lexer::tokenize(src);
    let mut report = ScanReport::default();
    for (at, token) in tokens.iter().enumerate() {
        let Kind::Ident(word) = &token.kind else {
            continue;
        };
        let block = match word.as_str() {
            "trait" => trait_block(&tokens, at),
            "impl" => impl_block(&tokens, at),
            _ => None,
        };
        let Some(block) = block else {
            continue;
        };
        if word == "trait" {
            report.traits += 1;
        } else {
            report.impls += 1;
        }
        check_body(&tokens, &block, display, &mut report);
    }
    report
}

/// The identifier at `at` and its line, if the token there is one.
fn ident(tokens: &[Token], at: usize) -> Option<(&str, usize)> {
    let token = tokens.get(at)?;
    match &token.kind {
        Kind::Ident(name) => Some((name.as_str(), token.line)),
        _ => None,
    }
}

fn is_punct(tokens: &[Token], at: usize, c: char) -> bool {
    tokens
        .get(at)
        .is_some_and(|token| matches!(token.kind, Kind::Punct(p) if p == c))
}

fn is_path_sep(tokens: &[Token], at: usize) -> bool {
    tokens
        .get(at)
        .is_some_and(|token| matches!(token.kind, Kind::PathSep))
}

/// The entity of a repository trait name, `Account` for `AccountRepository`. A name that does
/// not end in [`SUFFIX`], or is exactly [`SUFFIX`], has none.
fn entity_of(name: &str) -> Option<&str> {
    name.strip_suffix(SUFFIX)
        .filter(|entity| !entity.is_empty())
}

/// Applies one token of an item header to `groups`, the delimiters open at that point, innermost
/// last, and reports whether the header is still balanced.
///
/// `(`, `[` and `{` open a group. `)`, `]` and `}` close the innermost open group of their kind
/// together with anything opened inside it; one with no such group open unbalances the header.
/// `<` opens a generic list only at the top level or directly inside another generic list, and
/// `>` closes one only while a generic list is innermost, so a comparison in a braced const
/// expression such as `{ 2 > 1 }` or `{ 1 < 2 }`, and a `<` or `>` inside parentheses or
/// brackets, leaves the nesting alone. `->` and `=>` are tokens of their own and never count.
fn nest(groups: &mut Vec<char>, kind: &Kind) -> bool {
    match *kind {
        Kind::Punct('<') if groups.last().is_none_or(|&top| top == '<') => groups.push('<'),
        Kind::Punct('>') if groups.last() == Some(&'<') => {
            groups.pop();
        }
        Kind::Punct(opener @ ('(' | '[' | '{')) => groups.push(opener),
        Kind::Punct(closer @ (')' | ']' | '}')) => {
            let opener = match closer {
                ')' => '(',
                ']' => '[',
                _ => '{',
            };
            let Some(at) = groups.iter().rposition(|&group| group == opener) else {
                return false;
            };
            groups.truncate(at);
        }
        _ => {}
    }
    true
}

/// From `from`, the index of the `{` that opens the item's body: the first `{` at the top level
/// of the header, outside every generic list, parenthesis, bracket and brace that [`nest`]
/// tracks, so neither a braced const default or argument (`<const N: usize = { 4 }>`,
/// `Store<{ 4 }>`) nor the `;` of an array type (`[Widget; 4]`) ends the header. `None` when a
/// top-level `;` comes first, as for a declaration without a body, when the header is
/// unbalanced, or when the tokens end.
fn body_open(tokens: &[Token], from: usize) -> Option<usize> {
    let mut groups = Vec::new();
    for (at, token) in tokens.iter().enumerate().skip(from) {
        if groups.is_empty() {
            match token.kind {
                Kind::Punct('{') => return Some(at),
                Kind::Punct(';') => return None,
                _ => {}
            }
        }
        if !nest(&mut groups, &token.kind) {
            return None;
        }
    }
    None
}

/// At a `<`, the index just past the `>` that closes it, nesting as [`nest`] does, so a braced
/// const argument holding a comparison (`<{ 2 > 1 }>`) or an array type (`<[u8; 4]>`) stays
/// inside the list. `None` when the token at `open` is not `<`, when the list is unbalanced, or
/// when the tokens end first.
fn skip_angles(tokens: &[Token], open: usize) -> Option<usize> {
    if !is_punct(tokens, open, '<') {
        return None;
    }
    let mut groups = Vec::new();
    for (at, token) in tokens.iter().enumerate().skip(open) {
        if !nest(&mut groups, &token.kind) {
            return None;
        }
        if groups.is_empty() {
            return Some(at + 1);
        }
    }
    None
}

/// At a `trait` keyword: the block of a repository trait, whatever visibility, generics,
/// supertraits or `where` clause it carries.
fn trait_block(tokens: &[Token], at: usize) -> Option<Block<'_>> {
    let (name, _) = ident(tokens, at + 1)?;
    let entity = entity_of(name)?;
    let open = body_open(tokens, at + 2)?;
    Some(Block {
        label: format!("trait {name}"),
        entity,
        open,
    })
}

/// At an `impl` keyword: the block of a repository trait implementation, shaped `impl`, optional
/// generic parameters, a path whose last segment is the trait, optional generic arguments, then
/// `for`. Anything else is none: an inherent impl, `impl Trait` in a signature, a negative impl.
fn impl_block(tokens: &[Token], at: usize) -> Option<Block<'_>> {
    let mut next = at + 1;
    if is_punct(tokens, next, '<') {
        next = skip_angles(tokens, next)?;
    }
    if is_path_sep(tokens, next) {
        next += 1;
    }
    let (mut name, _) = ident(tokens, next)?;
    next += 1;
    while is_path_sep(tokens, next) {
        (name, _) = ident(tokens, next + 1)?;
        next += 2;
    }
    if is_punct(tokens, next, '<') {
        next = skip_angles(tokens, next)?;
    }
    if !matches!(ident(tokens, next), Some(("for", _))) {
        return None;
    }
    let entity = entity_of(name)?;
    let open = body_open(tokens, next + 1)?;
    let target = tokens
        .get(next + 1..open)
        .unwrap_or_default()
        .iter()
        .find_map(|token| match &token.kind {
            Kind::Ident(word) if word != "mut" && word != "dyn" => Some(word.as_str()),
            _ => None,
        })
        .unwrap_or("_");
    Some(Block {
        label: format!("impl {name} for {target}"),
        entity,
        open,
    })
}

/// Checks every method declared directly in `block`'s body: each identifier that follows `fn`
/// at depth 1. A `fn(` pointer type has no name and a nested `fn` sits deeper, so neither counts.
fn check_body(tokens: &[Token], block: &Block<'_>, display: &str, report: &mut ScanReport) {
    let noun = snake_case(block.entity);
    let mut depth: usize = 0;
    for (at, token) in tokens.iter().enumerate().skip(block.open) {
        match &token.kind {
            Kind::Punct('{') => depth += 1,
            Kind::Punct('}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return;
                }
            }
            Kind::Ident(word) if depth == 1 && word == "fn" => {
                let Some((method, line)) = ident(tokens, at + 1) else {
                    continue;
                };
                report.methods += 1;
                // A substring, not a whole word, so plurals and compounds such as
                // `find_accounts` fail too. A legitimate name that collides is renamed; the
                // check is not narrowed.
                if method.contains(noun.as_str()) {
                    let label = &block.label;
                    report.violations.push(format!(
                        "{display}:{line}: {label} method {method} repeats entity {noun}"
                    ));
                    report.flagged.push(Flagged {
                        label: label.clone(),
                        method: method.to_owned(),
                        line,
                    });
                }
            }
            _ => {}
        }
    }
}

/// The snake-case noun of a `CamelCase` entity: `_` before an uppercase letter that follows a
/// lowercase letter or a digit, and before the last uppercase letter of an acronym run that a
/// lowercase letter follows; then everything lowercased. Digits get no separator of their own.
fn snake_case(camel: &str) -> String {
    let chars: Vec<char> = camel.chars().collect();
    let mut snake = String::new();
    for (at, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev = at
                .checked_sub(1)
                .and_then(|before| chars.get(before))
                .copied();
            let next = chars.get(at + 1).copied();
            let after_word = prev.is_some_and(|p| p.is_lowercase() || p.is_ascii_digit());
            let ends_acronym =
                prev.is_some_and(char::is_uppercase) && next.is_some_and(char::is_lowercase);
            if after_word || ends_acronym {
                snake.push('_');
            }
        }
        snake.extend(c.to_lowercase());
    }
    snake
}

/// `path` relative to `modern`, for messages.
fn relative(path: &Path, modern: &Path) -> String {
    path.strip_prefix(modern)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn cannot_read(path: &Path, modern: &Path, err: &std::io::Error) -> String {
    let shown = relative(path, modern);
    format!("{shown}: cannot read: {err}")
}

/// The entries of `dir`, sorted so that the output is deterministic. A directory or entry that
/// cannot be read is recorded in `failures` and skipped.
fn list_dir(dir: &Path, modern: &Path, failures: &mut Vec<String>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    match fs::read_dir(dir) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(entry) => paths.push(entry.path()),
                    Err(err) => failures.push(cannot_read(dir, modern, &err)),
                }
            }
        }
        Err(err) => failures.push(cannot_read(dir, modern, &err)),
    }
    paths.sort();
    paths
}

/// Appends every `.rs` file under `dir` to `files`, skipping any directory named `target`.
fn collect_sources(
    dir: &Path,
    modern: &Path,
    files: &mut Vec<PathBuf>,
    failures: &mut Vec<String>,
) {
    for path in list_dir(dir, modern, failures) {
        if path.is_dir() {
            if path.file_name() != Some(OsStr::new("target")) {
                collect_sources(&path, modern, files, failures);
            }
        } else if path.extension() == Some(OsStr::new("rs")) {
            files.push(path);
        }
    }
}

#[test]
fn repository_methods_never_repeat_their_entity() {
    let modern = modern_dir();
    let mut report = ScanReport::default();
    let mut files = Vec::new();
    for crate_dir in list_dir(&modern.join("crates"), &modern, &mut report.violations) {
        for part in ["src", "tests"] {
            let dir = crate_dir.join(part);
            if dir.is_dir() {
                collect_sources(&dir, &modern, &mut files, &mut report.violations);
            }
        }
    }
    for path in &files {
        let shown = relative(path, &modern);
        match fs::read_to_string(path) {
            Ok(src) => report.absorb(scan_source(&shown, &src)),
            Err(err) => report
                .violations
                .push(format!("{shown}: cannot read: {err}")),
        }
    }
    println!(
        "repository_naming: {traits} repository traits, {impls} impls, {methods} methods checked \
         in {count} files",
        traits = report.traits,
        impls = report.impls,
        methods = report.methods,
        count = files.len(),
    );
    for violation in &report.violations {
        println!("{violation}");
    }
    assert!(
        report.violations.is_empty(),
        "{n} repository method name(s) repeat their entity",
        n = report.violations.len()
    );
    assert!(
        report.traits > 0,
        "no repository trait found under crates/: the scanner is broken"
    );
}

/// Source for [`scanner_self_check`]: matches, near misses and the lexical traps the lexer must
/// step over. Being a raw string, it is invisible to the workspace scan. The fourth matching
/// implementation, on `Generic`, carries a leading `::`, a `Fn() -> u8` bound, trait generic
/// arguments, a `where` clause, a raw byte string and a byte char holding `}`, and a raw
/// identifier method name. The six items after it put delimiters in their headers that must not
/// end the header: braced const defaults, one holding a comparison; a braced const target
/// argument; braced const trait arguments holding `>` and `<`; an array target type; and array
/// types in a generic default and in `where` clauses. Each declares one compliant method and one
/// that repeats its entity, so the method count proves the real body is read and the flagged
/// list proves its offender is caught.
const FIXTURE: &str = r##"
pub trait WidgetRepository: Send {
    fn find_widget_by_id(&self);
    fn find_by_id(&self);
    fn page_after(&self);
    fn next_id(&self);
    fn replace_all(&self);
}

impl WidgetRepository for FakeStore {
    fn widget_count(&self) -> usize { 0 }
    fn find_by_id(&self) { let _c = '{'; let _q = '\''; }
    fn page_after(&self) {}
    fn next_id(&self) {}
    fn replace_all(&self) { fn helper_widget() {} }
}

// trait CommentRepository { fn find_comment(); }
/* outer /* nested */
impl CommentRepository for X { fn comment_count() {} }
*/
const NOTE: &str = "trait GadgetRepository {
    fn gadget_id(); }";
const RAW: &str = r#"impl GadgetRepository for Y { fn gadget_all() {} }"#;
trait Repository { fn repository_id(); }
trait Clock { fn clock_now(); }
fn takes(repo: &impl WidgetRepository) -> impl Iterator<Item = u8> { todo!() }
impl WidgetRepository { fn widget_inherent() {} }

impl<'a> repos::WidgetRepository for PgTx<'a> {
    fn find_widgets(&self) {}
}

impl WidgetRepository for PgTx<'_> { fn find_by_id(&self) {} }

impl<T: Fn() -> u8> ::persistence::WidgetRepository<T> for Generic<T> where T: Send {
    fn find_by_id(&self) { let _s = br#"}"#; let _b = b'}'; }
    fn r#type(&self) {}
}

pub trait GizmoRepository<const N: usize = { 4 }, const B: bool = { 2 > 1 }> {
    fn gizmo_total(&self);
    fn find_by_id(&self);
}

impl WidgetRepository for Store<{ 4 }> {
    fn widget_total(&self) {}
    fn page_after(&self) {}
}

impl WidgetRepository<{ 2 > 1 }, { 1 < 2 }> for Store {
    fn first_widget(&self) {}
    fn next_id(&self) {}
}

impl WidgetRepository for [Widget; 4] {
    fn widgets_after(&self) {}
    fn find_by_id(&self) {}
}

trait SprocketRepository<T = [u8; 4]> where [T; 2]: Sized {
    fn sprocket_ids(&self);
    fn page_after(&self);
}

impl<T> GizmoRepository for Wrapper<T> where [T; 2]: Sized {
    fn page_gizmos(&self) {}
    fn next_id(&self) {}
}
"##;

#[test]
fn scanner_self_check() {
    let report = scan_source("fixture.rs", FIXTURE);
    assert_eq!(report.traits, 3, "repository traits in the fixture");
    assert_eq!(report.impls, 8, "repository implementations in the fixture");
    assert_eq!(
        report.methods, 26,
        "methods declared directly in the eleven bodies"
    );

    let flagged: Vec<(&str, &str, usize)> = report
        .flagged
        .iter()
        .map(|hit| (hit.label.as_str(), hit.method.as_str(), hit.line))
        .collect();
    assert_eq!(
        flagged,
        [
            ("trait WidgetRepository", "find_widget_by_id", 3),
            ("impl WidgetRepository for FakeStore", "widget_count", 11),
            ("impl WidgetRepository for PgTx", "find_widgets", 31),
            ("trait GizmoRepository", "gizmo_total", 42),
            ("impl WidgetRepository for Store", "widget_total", 47),
            ("impl WidgetRepository for Store", "first_widget", 52),
            ("impl WidgetRepository for Widget", "widgets_after", 57),
            ("trait SprocketRepository", "sprocket_ids", 62),
            ("impl GizmoRepository for Wrapper", "page_gizmos", 67),
        ],
        "flagged methods with their lines"
    );
    assert_eq!(report.violations.len(), 9);
    assert!(
        report.violations.contains(
            &"fixture.rs:31: impl WidgetRepository for PgTx method find_widgets repeats entity \
              widget"
                .to_owned()
        ),
        "violation message: {violations:?}",
        violations = report.violations
    );
    for clean in [
        "find_by_id",
        "page_after",
        "next_id",
        "replace_all",
        "helper_widget",
        "widget_inherent",
        "gadget_id",
        "gadget_all",
        "find_comment",
        "comment_count",
        "repository_id",
        "clock_now",
    ] {
        assert!(
            report.flagged.iter().all(|hit| hit.method != clean),
            "{clean} must not be flagged"
        );
    }

    // Totality: every prefix of the fixture, which ends inside comments, strings, raw strings,
    // char literals, generic lists and bodies, scans without a panic.
    for (cut, _) in FIXTURE.char_indices() {
        let partial = scan_source("cut.rs", FIXTURE.get(..cut).unwrap_or_default());
        assert!(
            partial.traits <= 3 && partial.impls <= 8,
            "prefix of {cut} characters"
        );
    }
}

#[test]
fn snake_case_nouns() {
    for (camel, snake) in [
        ("PendingAuthSummary", "pending_auth_summary"),
        ("CardXref", "card_xref"),
        (
            "Db2TransactionTypeCategory",
            "db2_transaction_type_category",
        ),
        ("CsslErrorLog", "cssl_error_log"),
        ("TranCatBalance", "tran_cat_balance"),
        ("HTTPServer", "http_server"),
        ("Card", "card"),
        ("User", "user"),
    ] {
        assert_eq!(snake_case(camel), snake, "snake case of {camel}");
    }
}
