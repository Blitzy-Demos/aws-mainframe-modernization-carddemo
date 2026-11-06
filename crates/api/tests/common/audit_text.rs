//! Expected STRING-built message texts, expanded from the reviewed templates of
//! `docs/parity/validation-audit.md`.
//!
//! A COBOL text built with `STRING … DELIMITED BY SIZE`, or by moving a group
//! of literal `FILLER`s and fields, is recorded in its validation row as a
//! template: the first code span of the `Message text` cell holds a
//! double-quoted string in which each variable part is `{<COBOL-FIELD>}`, and
//! the note after the span may state the fields' pictures and the receiving
//! field. A test expecting such a text builds it here, from the audit and from
//! the known key or status it injected, and never with the composers of
//! `carddemo_domain::messages`, so a defect in the production formatting
//! changes only one side of the comparison:
//!
//! - [`Template::load`] reads the template of one row that passed its review;
//! - each [`Operand`] is a COBOL `MOVE` of a known value into the field's
//!   [`Picture`], giving the exact bytes the field holds;
//! - [`Template::expand`] substitutes the operands and cuts the result at the
//!   width of the receiving field, after checking each picture and receiver
//!   width the note states.
//!
//! The cross-reference not-found text of V-COACTUPC-004 is
//! `Template::load(&audit, "V-COACTUPC-004").expand(&[account, resp, resp2], 75)`
//! with `Operand::alphanumeric("WS-CARD-RID-ACCT-ID-X", "99999999999", 11)`
//! [app/cbl/COACTUPC.cbl:L382-383] and the RESP fields
//! `Operand::unsigned_to_alphanumeric("ERROR-RESP", 13, 9, 10)` and
//! `("ERROR-RESP2", 80, 9, 10)` [app/cbl/COACTUPC.cbl:L401-406], cut at the
//! 75 bytes of `WS-RETURN-MSG` [app/cbl/COACTUPC.cbl:L479].
//!
//! Every template and operand is ASCII, so a cut at a byte count is exact.

use std::ops::Range;

use carddemo_audit::{Document, VALIDATION_AUDIT_PATH, code_span};

/// The validation-audit column whose first code span holds the template.
const MESSAGE_TEXT: &str = "Message text";

/// The review-log verdict every verdict on a loaded row must give.
const PASS: &str = "pass";

/// The picture of a template operand: how a `MOVE` lays a value out in the
/// field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    /// `PIC X(n)`: `n` bytes, the value left-justified and filled with spaces
    /// on the right, or cut at the right when it is longer.
    Alphanumeric(usize),
    /// `PIC 9(n)` DISPLAY: the low-order `n` digits of a non-negative value,
    /// filled with zeros on the left.
    Unsigned(usize),
    /// `PIC +9(n)`: `+` for a zero or positive value and `-` for a negative
    /// one, then the low-order `n` digits of the magnitude, zero-filled.
    SignedEdited(usize),
}

impl Picture {
    /// The `n` of the picture: its byte count, the sign of
    /// [`Picture::SignedEdited`] aside.
    fn size(self) -> usize {
        match self {
            Self::Alphanumeric(size) | Self::Unsigned(size) | Self::SignedEdited(size) => size,
        }
    }

    /// The picture as COBOL writes it, `X(8)`, `9(9)` or `+9(6)`, for failure
    /// texts.
    fn written(self) -> String {
        match self {
            Self::Alphanumeric(size) => format!("X({size})"),
            Self::Unsigned(size) => format!("9({size})"),
            Self::SignedEdited(size) => format!("+9({size})"),
        }
    }
}

/// One field of a template bound to the bytes it holds: the result of moving
/// a known value into the field's [`Picture`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Operand {
    /// The COBOL field name, as the template writes it between `{` and `}`.
    name: String,
    /// The picture the value was moved into.
    picture: Picture,
    /// The field's bytes after the move, always ASCII.
    bytes: String,
}

impl Operand {
    /// `MOVE value TO name` with `name` a `PIC X(width)`: `value`
    /// left-justified, filled with spaces on the right, or cut at the right
    /// when it is longer than `width`.
    ///
    /// `ERROR-OPNAME PIC X(8)` [app/cbl/COACTUPC.cbl:L392-393] holding `READ`
    /// is `"READ    "`.
    ///
    /// # Panics
    ///
    /// Panics, naming the field, when `width` is 0 or `value` is not ASCII: a
    /// `PIC X` field is a run of bytes, and a cut inside a multi-byte
    /// character has no text.
    #[must_use]
    pub fn alphanumeric(name: &str, value: &str, width: usize) -> Self {
        assert!(
            value.is_ascii(),
            "operand {name}: the value {value:?} is not ASCII, so its PIC X({width}) bytes \
             have no exact text"
        );
        Self::new(
            name,
            Picture::Alphanumeric(width),
            left_justified(value, width),
        )
    }

    /// `MOVE value TO name` with `value` a non-negative binary and `name` a
    /// `PIC 9(digits)` DISPLAY field: the low-order `digits` digits, filled
    /// with zeros on the left.
    ///
    /// `WS-RESP-CD-DIS PIC 9(09)`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS0C.cbl:L49] holding RESP
    /// 13 is `"000000013"`. Any `digits` is allowed, 20 and more included,
    /// where the field is wider than every `u64`.
    ///
    /// # Panics
    ///
    /// Panics, naming the field, when `digits` is 0.
    #[must_use]
    pub fn unsigned(name: &str, value: u64, digits: usize) -> Self {
        Self::new(
            name,
            Picture::Unsigned(digits),
            low_order_digits(value, digits),
        )
    }

    /// The `PIC 9(digits)` display text of `value`, as [`Operand::unsigned`]
    /// gives it, then moved to `name`, a `PIC X(width)`.
    ///
    /// `ERROR-RESP PIC X(10)` [app/cbl/COACTUPC.cbl:L401-402] receives the
    /// 9-digit display RESP, so RESP 13 is `"000000013 "`.
    ///
    /// # Panics
    ///
    /// Panics, naming the field, when `digits` or `width` is 0.
    #[must_use]
    pub fn unsigned_to_alphanumeric(name: &str, value: u64, digits: usize, width: usize) -> Self {
        let display = Self::unsigned(name, value, digits);
        Self::new(
            name,
            Picture::Alphanumeric(width),
            left_justified(&display.bytes, width),
        )
    }

    /// `MOVE value TO name` with `name` a `PIC +9(digits)` field: `+` for a
    /// zero or positive value and `-` for a negative one, then the low-order
    /// `digits` digits of the magnitude, filled with zeros on the left.
    ///
    /// `WS-SQLCODE PIC +9(06)`
    /// [app/app-authorization-ims-db2-mq/cbl/COPAUS2C.cbl:L55] holding 100
    /// is `"+000100"`, and holding -803 is `"-000803"`.
    ///
    /// # Panics
    ///
    /// Panics, naming the field, when `digits` is 0.
    #[must_use]
    pub fn signed_edited(name: &str, value: i64, digits: usize) -> Self {
        let sign = if value < 0 { '-' } else { '+' };
        let magnitude = low_order_digits(value.unsigned_abs(), digits);
        Self::new(
            name,
            Picture::SignedEdited(digits),
            format!("{sign}{magnitude}"),
        )
    }

    /// The bytes the field holds after the move.
    #[must_use]
    pub fn bytes(&self) -> &str {
        &self.bytes
    }

    /// The picture the value was moved into.
    #[must_use]
    pub fn picture(&self) -> Picture {
        self.picture
    }

    /// The operand `name` holding `bytes` under `picture`, once the picture is
    /// asserted to have a size.
    fn new(name: &str, picture: Picture, bytes: String) -> Self {
        assert!(
            picture.size() > 0,
            "operand {name}: PIC {} is not a COBOL picture",
            picture.written()
        );
        Self {
            name: name.to_owned(),
            picture,
            bytes,
        }
    }
}

/// The ASCII `value` in a `PIC X(width)`: space-filled or cut at the right.
fn left_justified(value: &str, width: usize) -> String {
    let mut field = format!("{value:<width$}");
    field.truncate(width);
    field
}

/// The low-order `digits` decimal digits of `value`, zero-filled on the left.
///
/// The digits are cut from the decimal text, never computed with `10^digits`,
/// so a field of 20 or more digits does not overflow.
fn low_order_digits(value: u64, digits: usize) -> String {
    let text = format!("{value:0digits$}");
    text[text.len() - digits..].to_owned()
}

/// One part of a template text.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    /// A literal run, never empty, by its byte range in the text.
    Literal(Range<usize>),
    /// A `{NAME}` operand, by the byte range of `NAME` in the text.
    Operand(Range<usize>),
}

/// The template of one reviewed validation row: its double-quoted message
/// text with the escapes resolved, split into literal runs and `{NAME}`
/// operands, and the note the row writes around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// The row ID, for failure texts.
    id: String,
    /// The whole `Message text` cell, as the audit parser gives it.
    cell: String,
    /// The template text, without its quotes and with `\"` and `\\` resolved.
    text: String,
    /// The cell outside its first code span.
    note: String,
    /// The parts of `text`, in order.
    parts: Vec<Part>,
}

impl Template {
    /// The template of the validation row `id` of `validation`, the parsed
    /// `docs/parity/validation-audit.md`.
    ///
    /// The row must be a data row whose review-log verdicts (`Row ID`,
    /// `Verdict`) are at least one and all `pass`. The first code span of its
    /// `Message text` cell must be a double-quoted string, in which `\"`
    /// stands for a double quote and `\\` for a backslash; the audit parser
    /// has already turned each `\|` of the cell into `|`. Each `{NAME}` of
    /// the text is an operand, `NAME` a COBOL field name of upper-case
    /// letters, digits and hyphens. The rest of the cell is the note that
    /// [`Template::expand`] checks its bindings against.
    ///
    /// # Panics
    ///
    /// Panics, naming the row, when the document defines no such data row,
    /// the row has no verdict or a verdict other than `pass`, the cell is
    /// missing or holds no double-quoted code span, the string holds a bare
    /// double quote or an escape other than `\"` and `\\`, or a `{` of the
    /// text has no `}` or encloses no COBOL field name.
    #[must_use]
    pub fn load(validation: &Document, id: &str) -> Self {
        let id = id.trim();
        let (table, row) = validation
            .find_row(id)
            .unwrap_or_else(|| panic!("{VALIDATION_AUDIT_PATH} defines no data row {id}"));
        let verdicts: Vec<&str> = validation
            .tables()
            .iter()
            .filter(|table| table.is_review_log())
            .flat_map(|table| table.rows.iter().map(move |row| (table, row)))
            .filter(|(table, row)| table.cell(row, "Row ID") == Some(id))
            .map(|(table, row)| table.cell(row, "Verdict").unwrap_or_default())
            .collect();
        assert!(
            !verdicts.is_empty() && verdicts.iter().all(|verdict| *verdict == PASS),
            "{VALIDATION_AUDIT_PATH}: row {id} has not passed its review: its review-log \
             verdicts are {verdicts:?}"
        );
        let cell = table.cell(row, MESSAGE_TEXT).unwrap_or_else(|| {
            panic!("{VALIDATION_AUDIT_PATH}: row {id} has no `{MESSAGE_TEXT}` cell")
        });
        let span = code_span(cell).unwrap_or_else(|| {
            panic!("{VALIDATION_AUDIT_PATH}: row {id} quotes no message text: {cell}")
        });
        let quoted = span
            .text
            .strip_prefix('"')
            .and_then(|text| text.strip_suffix('"'))
            .unwrap_or_else(|| {
                panic!(
                    "{VALIDATION_AUDIT_PATH}: row {id}: the message text {} is not a \
                     double-quoted string",
                    span.text
                )
            });
        let text = unescaped(id, quoted);
        let note = note_of(id, cell, &span.text);
        let parts = parts_of(id, &text);
        Self {
            id: id.to_owned(),
            cell: cell.to_owned(),
            text,
            note,
            parts,
        }
    }

    /// The template text, without its quotes and with its escapes resolved:
    /// the message itself for a row without operands.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The literal runs of the template around its operands, in order,
    /// without empty runs.
    #[must_use]
    pub fn literals(&self) -> Vec<&str> {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Literal(range) => Some(&self.text[range.clone()]),
                Part::Operand(_) => None,
            })
            .collect()
    }

    /// The operand names of the template, in order, a name used twice listed
    /// twice.
    #[must_use]
    pub fn operands(&self) -> Vec<&str> {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Operand(range) => Some(&self.text[range.clone()]),
                Part::Literal(_) => None,
            })
            .collect()
    }

    /// The text the COBOL builds from this template into a `PIC X(receiver)`,
    /// by `STRING … DELIMITED BY SIZE` or a group `MOVE`: each `{NAME}`
    /// replaced by the bytes of the operand named `NAME`, and the result cut
    /// at `receiver` bytes. It is never padded, so a shorter text keeps its
    /// length. A template without operands expands, with an empty
    /// `operands`, to its literal text cut at `receiver`.
    ///
    /// Before substituting, the note of the row is held to the bindings. Only
    /// what it states is checked, and a note that states nothing passes:
    ///
    /// - a picture written after a bound name, as `NAME X(n)`, `NAME 9(n)` or
    ///   `NAME +9(n)`, each also with `PIC` before the picture, and after a
    ///   list `NAME and OTHER-NAME X(n)`, must equal that operand's
    ///   [`Picture`], `n` compared as a number, so `9(09)` is
    ///   [`Picture::Unsigned`] of 9;
    /// - a receiver written as `cut at N bytes`, `moved to X(N)`,
    ///   `moved to <FIELD> X(N)` or `into <FIELD> X(N)` must equal
    ///   `receiver`. A text moved through several receivers keeps the bytes of
    ///   the narrowest, so a note stating several widths is held to the
    ///   smallest.
    ///
    /// Names that are not bound, `FILLER` among them, are not checked.
    ///
    /// # Panics
    ///
    /// Panics, naming the row, the template and what differs, when an operand
    /// of the template has no binding, a binding is used by no operand or two
    /// bindings share a name, the template is not ASCII, `receiver` is 0, or
    /// the note states a picture or a receiver width that differs from the
    /// bindings or from `receiver`. Operands are ASCII by construction.
    #[must_use]
    pub fn expand(&self, operands: &[Operand], receiver: usize) -> String {
        if !self.text.is_ascii() {
            self.fail("the template is not ASCII, so a cut at a byte count is not exact");
        }
        if receiver == 0 {
            self.fail("a receiver of PIC X(0) is not a COBOL field");
        }
        for (index, operand) in operands.iter().enumerate() {
            if operands[..index]
                .iter()
                .any(|earlier| earlier.name == operand.name)
            {
                self.fail(&format!("{} is bound twice", operand.name));
            }
        }
        let names = self.operands();
        for operand in operands {
            if !names.contains(&operand.name.as_str()) {
                self.fail(&format!(
                    "the binding {} is used by no operand of the template",
                    operand.name
                ));
            }
            self.check_pictures(operand);
        }
        self.check_receiver(receiver);
        let mut text = String::with_capacity(self.text.len());
        for part in &self.parts {
            match part {
                Part::Literal(range) => text.push_str(&self.text[range.clone()]),
                Part::Operand(range) => {
                    let name = &self.text[range.clone()];
                    let operand = operands
                        .iter()
                        .find(|operand| operand.name == name)
                        .unwrap_or_else(|| {
                            self.fail(&format!("the operand {{{name}}} has no binding"))
                        });
                    text.push_str(&operand.bytes);
                }
            }
        }
        text.truncate(receiver);
        text
    }

    /// Panics unless every picture the note states for `operand` is its
    /// picture.
    fn check_pictures(&self, operand: &Operand) {
        for stated in stated_pictures(&self.note, &operand.name) {
            if stated != operand.picture {
                self.fail(&format!(
                    "{} is bound as PIC {}, but the note states PIC {}: {}",
                    operand.name,
                    operand.picture.written(),
                    stated.written(),
                    self.cell
                ));
            }
        }
    }

    /// Panics unless `receiver` is the narrowest receiver width the note
    /// states, when it states one.
    fn check_receiver(&self, receiver: usize) {
        if let Some(stated) = stated_receivers(&self.note)
            .into_iter()
            .min()
            .filter(|&stated| stated != receiver)
        {
            self.fail(&format!(
                "the receiver is PIC X({receiver}), but the note states X({stated}): {}",
                self.cell
            ));
        }
    }

    /// Panics with `detail`, naming the row and the template.
    fn fail(&self, detail: &str) -> ! {
        panic!(
            "{VALIDATION_AUDIT_PATH}: row {}, template {:?}: {detail}",
            self.id, self.text
        )
    }
}

/// `quoted`, the inside of the double-quoted message text of row `id`, with
/// `\"` and `\\` resolved.
fn unescaped(id: &str, quoted: &str) -> String {
    let mut text = String::with_capacity(quoted.len());
    let mut chars = quoted.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                _ => panic!(
                    "{VALIDATION_AUDIT_PATH}: row {id}: the message text holds an escape other \
                     than \\\" or \\\\: {quoted}"
                ),
            },
            '"' => panic!(
                "{VALIDATION_AUDIT_PATH}: row {id}: the message text holds a double quote not \
                 written \\\": {quoted}"
            ),
            _ => text.push(ch),
        }
    }
    text
}

/// The `Message text` cell of row `id` outside its first code span, whose
/// text `code_span` gave as `span_text`.
///
/// The span is located as `code_span` reads it: a run of backticks opens it,
/// and the next run of exactly as many closes it. The two parts of the cell
/// are joined by a space, so no word of the note runs into another.
fn note_of(id: &str, cell: &str, span_text: &str) -> String {
    let bytes = cell.as_bytes();
    let run = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|&&byte| byte == b'`')
            .count()
    };
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'`' {
            at += 1;
            continue;
        }
        let ticks = run(at);
        let mut probe = at + ticks;
        while probe < bytes.len() {
            if bytes[probe] != b'`' {
                probe += 1;
                continue;
            }
            let closing = run(probe);
            if closing == ticks {
                let content = &cell[at + ticks..probe];
                let stripped = ticks >= 2
                    && content.starts_with(' ')
                    && content.ends_with(' ')
                    && content.bytes().any(|byte| byte != b' ');
                let text = if stripped {
                    &content[1..content.len() - 1]
                } else {
                    content
                };
                assert_eq!(
                    text, span_text,
                    "{VALIDATION_AUDIT_PATH}: row {id}: the code span located in the cell is not \
                     the one the audit parser reads: {cell}"
                );
                return format!("{} {}", &cell[..at], &cell[probe + closing..]);
            }
            probe += closing;
        }
        at += ticks;
    }
    panic!("{VALIDATION_AUDIT_PATH}: row {id}: no code span can be located in the cell: {cell}")
}

/// The parts of the template `text` of row `id`.
fn parts_of(id: &str, text: &str) -> Vec<Part> {
    let mut parts = Vec::new();
    let mut literal = 0;
    while let Some(offset) = text[literal..].find('{') {
        let open = literal + offset;
        let close = text[open..].find('}').map_or_else(
            || {
                panic!(
                    "{VALIDATION_AUDIT_PATH}: row {id}, template {text:?}: the `{{` at byte \
                     {open} has no `}}`"
                )
            },
            |found| open + found,
        );
        let name = &text[open + 1..close];
        assert!(
            cobol_name(name).is_some_and(|(found, rest)| found == name && rest.is_empty()),
            "{VALIDATION_AUDIT_PATH}: row {id}, template {text:?}: `{{{name}}}` at byte {open} \
             encloses no COBOL field name"
        );
        if open > literal {
            parts.push(Part::Literal(literal..open));
        }
        parts.push(Part::Operand(open + 1..close));
        literal = close + 1;
    }
    if literal < text.len() {
        parts.push(Part::Literal(literal..text.len()));
    }
    parts
}

/// Whether `ch` can be part of a COBOL field name as the audit writes one.
fn is_name_char(ch: char) -> bool {
    ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '-'
}

/// Whether `ch` next to a name makes it part of a longer word: a letter of
/// either case, a digit, `-` or `_`.
fn joins_name(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'
}

/// The COBOL field name `text` starts with and the text after it: the
/// longest run of upper-case letters, digits and hyphens, holding at least
/// one letter.
fn cobol_name(text: &str) -> Option<(&str, &str)> {
    let end = text
        .find(|ch: char| !is_name_char(ch))
        .unwrap_or(text.len());
    let (name, rest) = text.split_at(end);
    name.bytes()
        .any(|byte| byte.is_ascii_uppercase())
        .then_some((name, rest))
}

/// The picture `text` starts with, `X(n)`, `9(n)` or `+9(n)`, each also with
/// `PIC` before it, and the text after it.
fn picture(text: &str) -> Option<(Picture, &str)> {
    let text = match text.strip_prefix("PIC") {
        Some(rest) if rest.starts_with([' ', '\t']) => rest.trim_start(),
        _ => text,
    };
    let (kind, rest): (fn(usize) -> Picture, &str) = if let Some(rest) = text.strip_prefix("+9(") {
        (Picture::SignedEdited, rest)
    } else if let Some(rest) = text.strip_prefix("9(") {
        (Picture::Unsigned, rest)
    } else if let Some(rest) = text.strip_prefix("X(") {
        (Picture::Alphanumeric, rest)
    } else {
        return None;
    };
    let (digits, rest) = rest.split_once(')')?;
    Some((kind(number(digits)?), rest))
}

/// `digits` as a number when it is one or more ASCII digits that fit `usize`.
fn number(digits: &str) -> Option<usize> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Whether `at` starts a word of `note`: no letter or digit before it.
fn starts_word(note: &str, at: usize) -> bool {
    !note[..at].ends_with(|ch: char| ch.is_ascii_alphanumeric())
}

/// The pictures `note` states for the field `name`: each written after an
/// occurrence of the whole name, directly or after a list
/// `and OTHER-NAME …`, as [`Template::expand`] describes.
fn stated_pictures(note: &str, name: &str) -> Vec<Picture> {
    let mut pictures = Vec::new();
    for (at, _) in note.match_indices(name) {
        let end = at + name.len();
        let whole = !note[..at].ends_with(joins_name) && !note[end..].starts_with(joins_name);
        if !whole {
            continue;
        }
        let mut rest = &note[end..];
        while let Some(listed) = rest
            .trim_start()
            .strip_prefix("and ")
            .and_then(|list| cobol_name(list.trim_start()))
        {
            rest = listed.1;
        }
        if let Some((stated, _)) = picture(rest.trim_start()) {
            pictures.push(stated);
        }
    }
    pictures
}

/// The receiver widths `note` states, as [`Template::expand`] describes.
fn stated_receivers(note: &str) -> Vec<usize> {
    let mut widths = Vec::new();
    for (at, phrase) in note.match_indices("cut at ") {
        let rest = &note[at + phrase.len()..];
        let digits = rest
            .find(|ch: char| !ch.is_ascii_digit())
            .unwrap_or(rest.len());
        if starts_word(note, at) && rest[digits..].trim_start().starts_with("bytes") {
            widths.extend(number(&rest[..digits]));
        }
    }
    for (at, phrase) in note.match_indices("moved to ") {
        if !starts_word(note, at) {
            continue;
        }
        let rest = note[at + phrase.len()..].trim_start();
        widths.extend(alphanumeric_width(rest).or_else(|| {
            cobol_name(rest).and_then(|(_, after)| alphanumeric_width(after.trim_start()))
        }));
    }
    for (at, phrase) in note.match_indices("into ") {
        if !starts_word(note, at) {
            continue;
        }
        let rest = note[at + phrase.len()..].trim_start();
        widths
            .extend(cobol_name(rest).and_then(|(_, after)| alphanumeric_width(after.trim_start())));
    }
    widths
}

/// The `n` of the `X(n)` picture `text` starts with.
fn alphanumeric_width(text: &str) -> Option<usize> {
    match picture(text) {
        Some((Picture::Alphanumeric(width), _)) => Some(width),
        _ => None,
    }
}
