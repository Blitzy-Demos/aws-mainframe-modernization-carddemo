//! Seeded fixture records decoded at the reviewed section A offsets of
//! `docs/parity/parity-oracle-audit.md`, and the section D values the audit
//! records for them.
//!
//! The fixture expectations of a parity test are read through
//! `carddemo_audit`, never copied into the test (AAP 0.9.3). A test that needs
//! a seeded value therefore takes it from a reviewed section D row with
//! [`section_d_value`], or decodes the fixture record that holds it with a
//! [`Layout`]: the section A rows of one copybook, each giving a field's
//! column (the `column=` of its `Expected result`), its 0-based `Offset`, its
//! `Length` and, from its `Input condition`, how the field is stored. FILLER
//! rows (`column=—`) are read and not decoded. Every row a layout or a value is
//! read from must have passed its review first.
//!
//! The records come from the ASCII fixtures under
//! [`carddemo_testkit::fixtures::ascii_dir`], which are only read:
//!
//! - an alphanumeric field is its bytes right-trimmed of spaces;
//! - an unsigned display field is its bytes as stored, zero padding kept;
//! - a signed field is the decimal string of its trailing overpunch with the
//!   implied decimals of its PIC, as the API writes a `Decimal`: `00000001940{`
//!   under `S9(10)V99` is `194.00`, and a negative value starts with `-`.
//!
//! The layouts read are `ACCOUNT-RECORD` of `acctdata.txt` (P-CVACT01Y-001…013)
//! and `CARD-XREF-RECORD` of `cardxref.txt` (P-CVACT03Y-001…004), whose records
//! are stored as their first 36 bytes (E-04). A cross-reference is looked up by
//! account or by card as exactly one record, because the seeded state gives
//! every account exactly one card and every card exactly one cross-reference
//! (P-CVACT03Y-006).
//!
//! Its callers are the API parity tests whose expectations rest on seeded
//! account and cross-reference records: the account-keyed adds of CT00, the
//! account-only add of CT02, the payments of CB00 and the account update of
//! CAUP.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use carddemo_audit::{Document, Row, Table, code_span, key_values};
use carddemo_testkit::fixtures;

/// The copybook of `ACCOUNT-RECORD`, the `ACCTDAT` layout (P-CVACT01Y-001).
const ACCOUNT_COPYBOOK: &str = "CVACT01Y";

/// The ASCII fixture of `ACCTDAT`.
const ACCOUNT_FILE: &str = "acctdata.txt";

/// The copybook of `CARD-XREF-RECORD`, the `CCXREF` layout (P-CVACT03Y-001).
const XREF_COPYBOOK: &str = "CVACT03Y";

/// The ASCII fixture of `CCXREF`.
const XREF_FILE: &str = "cardxref.txt";

/// The prefix of the `##` heading of section A, the record layouts.
const SECTION_A: &str = "A.";

/// The prefix of the `##` heading of section D, the fixture expectations.
const SECTION_D: &str = "D.";

/// The audit column holding a row's structured result.
const EXPECTED_RESULT: &str = "Expected result";

/// The `column=` of a section A row whose field is not stored: a FILLER.
const FILLER: &str = "—";

/// The section D row recording how many cross-references the seeded accounts
/// and cards have.
const XREF_CARDINALITY: &str = "P-CVACT03Y-006";

/// How a section A field is stored in its fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// Alphanumeric: decoded as its bytes right-trimmed of spaces.
    Text,
    /// Unsigned display digits: decoded as its bytes, zero padding kept.
    Digits,
    /// Signed zoned with a trailing overpunch and `scale` implied decimals:
    /// decoded as a decimal string.
    Signed { scale: usize },
}

/// One stored field of a section A record layout.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    /// The ID of the section A row recording the field.
    id: String,
    /// The column the field maps to, the key a [`Record`] keeps it under.
    column: String,
    /// The 0-based offset of the field in the record.
    offset: usize,
    /// The length of the field in bytes.
    length: usize,
    /// How it is stored.
    form: Form,
}

/// The section A record layout of one copybook, with the ASCII fixture it
/// decodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The copybook, for example `CVACT01Y`.
    copybook: String,
    /// The fixture file name under `app/data/ASCII`, for example
    /// `acctdata.txt`.
    file: String,
    /// The stored fields, in record order.
    fields: Vec<Field>,
}

impl Layout {
    /// The section A layout of `copybook`, decoding the ASCII fixture `file`.
    ///
    /// The layout is every row of the section A data tables whose `###`
    /// heading starts with the word `copybook`, for example
    /// `CVACT01Y ACCOUNT-RECORD (ACCTDAT, 300)`. Every row, FILLER included,
    /// must hold at least one review verdict and only `pass` verdicts.
    ///
    /// # Panics
    ///
    /// Panics if section A records no layout of `copybook` or no stored field
    /// in it, if a row of it has no review or a review that did not pass, if a
    /// row records no `column=`, a non-numeric `Offset` or `Length`, an
    /// `Input condition` no stored form is known for or a PIC whose implied
    /// decimals cannot be read, or if two stored fields share a column.
    #[must_use]
    pub fn load(parity: &Document, copybook: &str, file: &str) -> Self {
        let tables: Vec<&Table> = parity
            .section_tables(SECTION_A)
            .filter(|table| table.is_data_table() && is_headed_by(table, copybook))
            .collect();
        assert!(
            !tables.is_empty(),
            "section A of the parity audit records no {copybook} layout"
        );
        let mut fields = Vec::new();
        for table in tables {
            for row in &table.rows {
                assert_reviewed(parity, table.cell(row, "ID").unwrap_or_default());
                fields.extend(layout_field(table, row));
            }
        }
        assert!(!fields.is_empty(), "the {copybook} layout stores no field");
        let mut columns = BTreeSet::new();
        for field in &fields {
            assert!(
                columns.insert(field.column.as_str()),
                "the {copybook} layout stores `{}` twice ({})",
                field.column,
                field.id
            );
        }
        Self {
            copybook: copybook.to_owned(),
            file: file.to_owned(),
            fields,
        }
    }

    /// The `CVACT01Y` layout of `ACCOUNT-RECORD`, decoding `acctdata.txt`
    /// (P-CVACT01Y-001…013).
    ///
    /// # Panics
    ///
    /// Panics as [`Layout::load`] does.
    #[must_use]
    pub fn account(parity: &Document) -> Self {
        Self::load(parity, ACCOUNT_COPYBOOK, ACCOUNT_FILE)
    }

    /// The `CVACT03Y` layout of `CARD-XREF-RECORD`, decoding `cardxref.txt`
    /// (P-CVACT03Y-001…004).
    ///
    /// # Panics
    ///
    /// Panics as [`Layout::load`] does.
    #[must_use]
    pub fn card_xref(parity: &Document) -> Self {
        Self::load(parity, XREF_COPYBOOK, XREF_FILE)
    }

    /// Every record of the fixture, in file order, whose `column` decodes to
    /// `key`.
    ///
    /// Only the `column` field of each record is decoded to compare it with
    /// `key`; each record that matches is then decoded whole.
    ///
    /// # Panics
    ///
    /// Panics if the layout stores no `column`, if the fixture cannot be read,
    /// if a decoded record does not hold a field at its offset and length, or
    /// if a signed field of it is no zoned decimal with a trailing overpunch.
    #[must_use]
    pub fn records(&self, column: &str, key: &str) -> Vec<Record> {
        let field = self.field(column);
        let path = fixtures::ascii_dir().join(&self.file);
        let text = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{} cannot be read: {error}", path.display()));
        text.lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line))
            .filter(|&(number, line)| self.decode(number, line, field) == key)
            .map(|(number, line)| Record {
                file: self.file.clone(),
                line: number,
                values: self
                    .fields
                    .iter()
                    .map(|stored| (stored.column.clone(), self.decode(number, line, stored)))
                    .collect(),
            })
            .collect()
    }

    /// The one record of the fixture whose `column` decodes to `key`.
    ///
    /// # Panics
    ///
    /// Panics, naming the fixture, `column`, `key` and the records found,
    /// unless exactly one record matches, and as [`Layout::records`] does.
    #[must_use]
    pub fn record(&self, column: &str, key: &str) -> Record {
        let mut records = self.records(column, key);
        assert_eq!(
            records.len(),
            1,
            "{} must hold exactly one record whose `{column}` is {key:?}, found lines {:?}",
            self.file,
            records.iter().map(|record| record.line).collect::<Vec<_>>()
        );
        records.remove(0)
    }

    /// The stored field of the layout under `column`.
    fn field(&self, column: &str) -> &Field {
        self.fields
            .iter()
            .find(|field| field.column == column)
            .unwrap_or_else(|| {
                panic!(
                    "the {} layout of {} stores no `{column}`",
                    self.copybook, self.file
                )
            })
    }

    /// The value `field` holds in `line`, the record on 1-based line `number`
    /// of the fixture.
    fn decode(&self, number: usize, line: &str, field: &Field) -> String {
        let end = field.offset + field.length;
        let bytes = line.get(field.offset..end).unwrap_or_else(|| {
            panic!(
                "{} record {number} holds no `{}` at bytes {}..{end} ({}): {line:?}",
                self.file, field.column, field.offset, field.id
            )
        });
        match field.form {
            Form::Text => bytes.trim_end_matches(' ').to_owned(),
            Form::Digits => bytes.to_owned(),
            Form::Signed { scale } => overpunch(bytes, scale).unwrap_or_else(|| {
                panic!(
                    "{} record {number} holds {bytes:?} in `{}` ({}), no zoned decimal with \
                     {scale} implied decimals and a trailing overpunch",
                    self.file, field.column, field.id
                )
            }),
        }
    }
}

/// One fixture record decoded at its layout's offsets: the value of every
/// stored field, by column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The fixture file name the record is read from.
    file: String,
    /// The 1-based line of the record in that file.
    line: usize,
    /// The decoded value of every stored field, by column.
    values: BTreeMap<String, String>,
}

impl Record {
    /// The decoded value of the stored field `column`, for example
    /// `xref_card_num`.
    ///
    /// # Panics
    ///
    /// Panics, naming `column` and the record, if its layout stores no
    /// `column`.
    #[must_use]
    pub fn value(&self, column: &str) -> &str {
        self.values.get(column).map_or_else(
            || {
                panic!(
                    "{} record {} stores no `{column}`; its columns are {:?}",
                    self.file,
                    self.line,
                    self.values.keys().collect::<Vec<_>>()
                )
            },
            String::as_str,
        )
    }
}

/// The `ACCTDAT` record of the account `account_id`, the 11 digits
/// `ACCT-ID 9(11)` stores (P-CVACT01Y-001).
///
/// # Panics
///
/// Panics unless `acctdata.txt` holds exactly one record of `account_id`, and
/// as [`Layout::account`] and [`Layout::records`] do.
#[must_use]
pub fn account(parity: &Document, account_id: &str) -> Record {
    Layout::account(parity).record("account_id", account_id)
}

/// The one cross-reference of the account `account_id`: the `CCXREF` record
/// whose `XREF-ACCT-ID 9(11)` is those 11 digits (P-CVACT03Y-003), giving the
/// account's card as `xref_card_num` (P-CVACT03Y-001) and its customer as
/// `xref_cust_id` (P-CVACT03Y-002).
///
/// The seeded state gives every account exactly one card (P-CVACT03Y-006).
///
/// # Panics
///
/// Panics if P-CVACT03Y-006 has not passed its review or no longer records
/// that every account has exactly one card, unless `cardxref.txt` holds
/// exactly one record of `account_id`, and as [`Layout::card_xref`] and
/// [`Layout::records`] do.
#[must_use]
pub fn account_xref(parity: &Document, account_id: &str) -> Record {
    assert_cardinality(parity, "every account exactly one card");
    Layout::card_xref(parity).record("xref_acct_id", account_id)
}

/// The one cross-reference of the card `card_number`: the `CCXREF` record
/// whose `XREF-CARD-NUM X(16)` is `card_number` (P-CVACT03Y-001), giving the
/// card's account as `xref_acct_id` (P-CVACT03Y-003) and its customer as
/// `xref_cust_id` (P-CVACT03Y-002).
///
/// The seeded state gives every card exactly one cross-reference
/// (P-CVACT03Y-006).
///
/// # Panics
///
/// Panics if P-CVACT03Y-006 has not passed its review or no longer records
/// that every card has exactly one cross-reference, unless `cardxref.txt`
/// holds exactly one record of `card_number`, and as [`Layout::card_xref`]
/// and [`Layout::records`] do.
#[must_use]
pub fn card_xref(parity: &Document, card_number: &str) -> Record {
    assert_cardinality(parity, "every card has exactly one cross-reference");
    Layout::card_xref(parity).record("xref_card_num", card_number)
}

/// The value the `key=` pair of the `Expected result` of the section D row
/// `id` records, once that row has passed its review.
///
/// An exact text, a code span holding a double-quoted string such as
/// `` `"00000000001"` ``, is returned as the string it quotes, its `\"` and
/// `\\` escapes resolved; any other value is returned as written. For
/// example, `section_d_value(parity, "P-CVACT01Y-015", "acct_curr_bal")` is
/// `194.00`.
///
/// # Panics
///
/// Panics if the parity audit has no data row `id`, if the row lies outside
/// section D or has not passed its review, or unless its `Expected result`
/// records `key=` exactly once.
#[must_use]
pub fn section_d_value(parity: &Document, id: &str, key: &str) -> String {
    let result = expected_result(parity, id, SECTION_D);
    let values: Vec<String> = key_values(result)
        .into_iter()
        .filter(|pair| pair.key == key)
        .map(|pair| pair.value)
        .collect();
    let [value] = values.as_slice() else {
        panic!(
            "{id} must record {key}= exactly once, found {}: {result}",
            values.len()
        );
    };
    exact_text(value)
}

/// Whether the `###` heading of `table` starts with the word `copybook`.
fn is_headed_by(table: &Table, copybook: &str) -> bool {
    table
        .subsection
        .as_deref()
        .and_then(|subsection| subsection.split_whitespace().next())
        == Some(copybook)
}

/// The field the section A row `row` of `table` records, or `None` for a
/// FILLER, which is not stored.
fn layout_field(table: &Table, row: &Row) -> Option<Field> {
    let id = table.cell(row, "ID").unwrap_or_default();
    let cell = |name: &str| {
        table
            .cell(row, name)
            .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
    };
    let column = key_values(cell(EXPECTED_RESULT))
        .into_iter()
        .find(|pair| pair.key == "column")
        .unwrap_or_else(|| panic!("{id} records no column="))
        .value;
    if column == FILLER {
        return None;
    }
    let number = |name: &str| {
        cell(name)
            .parse::<usize>()
            .unwrap_or_else(|_| panic!("{id} has no numeric `{name}`"))
    };
    let condition = cell("Input condition");
    let form = if condition.starts_with("alphanumeric") {
        Form::Text
    } else if condition.starts_with("unsigned display") {
        Form::Digits
    } else if condition.starts_with("signed") {
        Form::Signed {
            scale: scale(id, cell("Copybook field or PIC clause")),
        }
    } else {
        panic!("{id}: no stored form is known for `{condition}`")
    };
    Some(Field {
        id: id.to_owned(),
        column,
        offset: number("Offset"),
        length: number("Length"),
        form,
    })
}

/// The implied decimals of the PIC clause ending `pic_cell`, the cell of the
/// section A row `id`: 2 for `ACCT-CURR-BAL S9(10)V99`, 0 without a `V`.
fn scale(id: &str, pic_cell: &str) -> usize {
    let pic = pic_cell.split_whitespace().last().unwrap_or_default();
    pic.split_once('V').map_or(0, |(_, decimals)| {
        match decimals
            .strip_prefix("9(")
            .and_then(|rest| rest.strip_suffix(')'))
        {
            Some(count) => count
                .parse()
                .unwrap_or_else(|_| panic!("{id}: `{pic}` has no decimal count")),
            None if decimals.bytes().all(|byte| byte == b'9') => decimals.len(),
            None => panic!("{id}: `{pic}` has unreadable decimals"),
        }
    })
}

/// The decimal text of the signed zoned field `bytes`, whose last byte holds
/// the sign as an ASCII overpunch (`{` and `A` to `I` for +0 to +9, `}` and
/// `J` to `R` for -0 to -9), with `scale` implied decimals, or `None` when
/// `bytes` is no such field.
///
/// `00000001940{` with scale 2 is `194.00`; a zero is written without a sign.
fn overpunch(bytes: &str, scale: usize) -> Option<String> {
    let (&last, head) = bytes.as_bytes().split_last()?;
    let (digit, negative) = match last {
        b'{' => (b'0', false),
        b'A'..=b'I' => (b'1' + (last - b'A'), false),
        b'}' => (b'0', true),
        b'J'..=b'R' => (b'1' + (last - b'J'), true),
        _ => return None,
    };
    if head.len() < scale || !head.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut digits = std::str::from_utf8(head).ok()?.to_owned();
    digits.push(char::from(digit));
    let (whole, fraction) = digits.split_at(digits.len() - scale);
    let whole = whole.trim_start_matches('0');
    let whole = if whole.is_empty() { "0" } else { whole };
    let sign = if negative && digits.bytes().any(|byte| byte != b'0') {
        "-"
    } else {
        ""
    };
    Some(if scale == 0 {
        format!("{sign}{whole}")
    } else {
        format!("{sign}{whole}.{fraction}")
    })
}

/// The string the audit value `value` stands for: the double-quoted string of
/// its first code span with its `\"` and `\\` escapes resolved, as in
/// `` `"00000000001"` ``, else the code span's text, else `value` as written,
/// as in `194.00`.
fn exact_text(value: &str) -> String {
    let Some(span) = code_span(value) else {
        return value.to_owned();
    };
    let Some(quoted) = span
        .text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
    else {
        return span.text;
    };
    let mut text = String::with_capacity(quoted.len());
    let mut chars = quoted.chars().peekable();
    while let Some(ch) = chars.next() {
        let escaped = if ch == '\\' {
            chars.next_if(|next| matches!(next, '"' | '\\'))
        } else {
            None
        };
        text.push(escaped.unwrap_or(ch));
    }
    text
}

/// The `Expected result` of the data row `id` of the parity audit, once the
/// row is found under a `##` heading starting with `section` and has passed
/// its review.
fn expected_result<'a>(parity: &'a Document, id: &str, section: &str) -> &'a str {
    let (table, row) = parity
        .find_row(id)
        .unwrap_or_else(|| panic!("the parity audit has no {id} row"));
    assert!(
        table
            .section
            .as_deref()
            .is_some_and(|heading| heading.starts_with(section)),
        "{id} lies under {:?}, not in section {section}",
        table.section
    );
    assert_reviewed(parity, id);
    table
        .cell(row, EXPECTED_RESULT)
        .unwrap_or_else(|| panic!("{id} has no `{EXPECTED_RESULT}` cell"))
}

/// Asserts that the section D row P-CVACT03Y-006 has passed its review and
/// records `statement` of the seeded cross-references.
fn assert_cardinality(parity: &Document, statement: &str) {
    let result = expected_result(parity, XREF_CARDINALITY, SECTION_D);
    assert!(
        result.contains(statement),
        "{XREF_CARDINALITY} must record that {statement}: {result}"
    );
}

/// Asserts that a review log of `parity` holds a verdict on the row `id` and
/// that every verdict on it is `pass`.
fn assert_reviewed(parity: &Document, id: &str) {
    let verdicts: Vec<&str> = parity
        .tables()
        .iter()
        .filter(|table| table.is_review_log())
        .flat_map(|table| {
            table
                .rows
                .iter()
                .filter(|row| table.cell(row, "Row ID") == Some(id))
                .map(|row| table.cell(row, "Verdict").unwrap_or_default())
        })
        .collect();
    assert!(
        !verdicts.is_empty() && verdicts.iter().all(|verdict| *verdict == "pass"),
        "{id} must have passed its review, found {verdicts:?}"
    );
}
