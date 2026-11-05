//! `batch mnttrdb2`: the MNTTRDB2 job, `COBTUPDT` over an `INPFILE`, maintaining the Db2
//! master `CARDDEMO.TRANSACTION_TYPE` (table `transaction_types`) and nothing else.
//!
//! The fixture `crates/batch/tests/fixtures/mnttrdb2_input.txt` is test input in the
//! `WS-INPUT-REC` layout (P-COBTUPDT-001 to P-COBTUPDT-003): one record per branch of
//! `1003-TREAT-RECORD` and of each statement's `SQLCODE` test. Section D of the parity audit
//! holds no MNTTRDB2 rows, so every expected table is derived here from the reviewed rows,
//! without the production seed or maintenance code:
//!
//! - [`Masters::loaded`] parses the seeded Db2 masters from the CREADB21 load statements
//!   `DB2LTTYP` and `DB2LTCAT` and checks them against the audited seed rows (P-TRNTYPE-003 to
//!   P-TRNTYPE-005, P-TRNTYCAT-004, P-TRNTYCAT-005).
//! - [`Cobtupdt::audited`] reads the COBTUPDT rules from their cells: the field offsets and
//!   lengths, the record-type dispatch (P-COBTUPDT-006), the insert, update and delete with
//!   their failures, the E-30 codes -803 and -532 among them (P-COBTUPDT-007 to
//!   P-COBTUPDT-009), the right-trimmed description column (P-TRNTYPE-002), and the return
//!   codes `9999-ABEND` leaves (P-COBTUPDT-010). A cell that no longer states a clause the
//!   model relies on fails the test with its text.
//! - [`Cobtupdt::maintain`] applies those rules to records in file order from the seeded
//!   masters, a failed statement changing nothing and the return code staying to `STOP RUN`
//!   (P-COBTUPDT-012). It gives the final type master, each record's outcome, the counts the
//!   job reports and the exit code.
//!
//! [`RECORDS`] holds what COBTUPDT displays for each record, `9999-ABEND` displaying the
//! `WS-RETURN-MSG X(80)` it never clears. The test checks the fixture against [`RECORDS`], so
//! the input and the expectations cannot drift apart, each display against the outcome the
//! model derives, and the catalog texts against the code spans of the rows that quote them.
//! Two more runs reach the other ends of the job: the fixture records that do not fail, which
//! commit every change and end with the return code of a run without failure, and the whole
//! fixture with a `STOP RUN` commit the server refuses, which persists nothing and ends with
//! 16. Their job span and events are read as JSON members, one exact value token per key.

pub mod common;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::thread;

use carddemo_audit::{Document, PARITY_COMMON_COLUMNS, PARITY_LAYOUT_COLUMNS, PARITY_RULE_COLUMNS};
use carddemo_domain::messages::{
    ERROR_ACCESSING, ERROR_TYPE_NOT_VALID, IGNORING_COMMENTED_LINE, NO_RECORDS_FOUND,
    OPEN_FILE_NOT_OK, PROCESSING, RECORD_DELETED_SUCCESSFULLY, RECORD_INSERTED_SUCCESSFULLY,
    RECORD_UPDATED_SUCCESSFULLY, TRANSACTION_TYPE_TABLE_SQLCODE,
};
use carddemo_testkit::fixtures;
use common::{Harness, block_on, expected_span, expected_value, parity, parity_cell};

/// The audit rows this file proves: the `WS-INPUT-REC` layout, the COBTUPDT rules from the
/// job start to `STOP RUN`, the open failure of an `INPFILE` that cannot be read, the seeded
/// Db2 masters the rules start from, and the stored form of a type description.
const P_IDS: &[&str] = &[
    "P-COBTUPDT-001",
    "P-COBTUPDT-002",
    "P-COBTUPDT-003",
    "P-COBTUPDT-004",
    "P-COBTUPDT-005",
    "P-COBTUPDT-006",
    "P-COBTUPDT-007",
    "P-COBTUPDT-008",
    "P-COBTUPDT-009",
    "P-COBTUPDT-010",
    "P-COBTUPDT-011",
    "P-COBTUPDT-012",
    "P-TRNTYPE-002",
    "P-TRNTYPE-003",
    "P-TRNTYPE-004",
    "P-TRNTYPE-005",
    "P-TRNTYCAT-004",
    "P-TRNTYCAT-005",
];

/// The fixture `INPFILE`, relative to the repository root, the working directory of every
/// `batch` run of the harness.
const INPUT: &str = "crates/batch/tests/fixtures/mnttrdb2_input.txt";

/// An `INPFILE` that does not exist.
const MISSING_INPUT: &str = "crates/batch/tests/fixtures/mnttrdb2_input.absent";

/// The 53 bytes of `WS-INPUT-REC` each fixture record fills, space-padded: the end of its last
/// field, which [`Cobtupdt::audited`] checks against the layout rows.
const INPUT_REC_LEN: usize = 53;

/// The `WS-RETURN-MSG PIC X(80)` line `9999-ABEND` displays.
const RETURN_MSG_LEN: usize = 80;

/// The exit code of a setup failure, which changes nothing: an `INPFILE` that cannot be opened
/// (P-COBTUPDT-011, E-36), or a `STOP RUN` commit that fails and undoes every statement.
const SETUP_FAILURE_EXIT: i32 = 16;

/// The CREADB21 load statement of the Db2 type master, the `SYSIN` of its `DSNTEP4` step
/// [app/app-transaction-type-db2/jcl/CREADB21.jcl:L72], relative to the repository root.
const TYPE_LOAD: &str = "app/app-transaction-type-db2/ctl/DB2LTTYP.ctl";

/// The table [`TYPE_LOAD`] inserts into.
const TYPE_TABLE: &str = "CARDDEMO.TRANSACTION_TYPE";

/// The CREADB21 load statement of the Db2 category master
/// [app/app-transaction-type-db2/jcl/CREADB21.jcl:L84], relative to the repository root.
const CATEGORY_LOAD: &str = "app/app-transaction-type-db2/ctl/DB2LTCAT.ctl";

/// The table [`CATEGORY_LOAD`] inserts into.
const CATEGORY_TABLE: &str = "CARDDEMO.TRANSACTION_TYPE_CATEGORY";

/// The `Input condition` column of the parity audit data tables.
const INPUT_CONDITION: &str = PARITY_COMMON_COLUMNS[4];

/// The `Expected result` column of the parity audit data tables.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// The `Persists on later failure` column of a section B table.
const PERSISTS: &str = PARITY_RULE_COLUMNS[0];

/// The exception the trigger of [`REFUSE_AT_COMMIT`] raises at `COMMIT`.
const REFUSED_AT_COMMIT: &str = "MNTTRDB2 type change refused at commit";

/// What COBTUPDT displays for a record after its `PROCESSING` line.
#[derive(Clone, Copy, Debug)]
enum Shown {
    /// A paragraph's own `DISPLAY`, logged as an `INFO` event.
    Display(&'static str),
    /// The `WS-RETURN-MSG` that `9999-ABEND` displays, logged as a `WARN` event: the 80-byte
    /// receiver, given here without its trailing spaces.
    Abend(&'static str),
}

impl Shown {
    /// The `(level, message)` of the event the job logs for this display.
    fn event(self) -> (String, String) {
        match self {
            Self::Display(text) => ("INFO".to_owned(), text.to_owned()),
            Self::Abend(line) => ("WARN".to_owned(), format!("{line:<RETURN_MSG_LEN$}")),
        }
    }
}

/// The fixture records in file order, without their trailing spaces, each with what COBTUPDT
/// displays for it. A `STRING` into `WS-RETURN-MSG` overwrites only the bytes it writes, so a
/// shorter message keeps the tail of the one before it.
const RECORDS: [(&str, Shown); 10] = [
    (
        "* COBTUPDT INPFILE: COL 1 TYPE, 2-3 CODE, 4-53 DESC",
        Shown::Display(IGNORING_COMMENTED_LINE),
    ),
    (
        "A08BALANCE TRANSFER",
        Shown::Display(RECORD_INSERTED_SUCCESSFULLY),
    ),
    (
        "A09ANNUAL FEE",
        Shown::Display(RECORD_INSERTED_SUCCESSFULLY),
    ),
    // TR_TYPE 01 is seeded: the insert ends with -803, a unique violation (E-30).
    (
        "A01PURCHASE DUPLICATE KEY",
        Shown::Abend("Error accessing: TRANSACTION_TYPE table. SQLCODE: -803"),
    ),
    // Replaces the DB2LTTYP description REVERAL.
    ("U06REVERSAL", Shown::Display(RECORD_UPDATED_SUCCESSFULLY)),
    // +100: the 17 bytes of `No records found.` lie over the -803 message.
    (
        "U99UNKNOWN TRANSACTION TYPE",
        Shown::Abend("No records found.TRANSACTION_TYPE table. SQLCODE: -803"),
    ),
    // Type 09, added by the third record, has no category, so it is deleted.
    ("D09", Shown::Display(RECORD_DELETED_SUCCESSFULLY)),
    // The DB2LTCAT categories of type 01 restrict the delete: -532 (E-30).
    (
        "D01",
        Shown::Abend("Error accessing: TRANSACTION_TYPE table. SQLCODE: -532"),
    ),
    (
        "D99",
        Shown::Abend("No records found.TRANSACTION_TYPE table. SQLCODE: -532"),
    ),
    // The record type is compared as stored: a lower-case `u` is no valid type.
    (
        "u03CREDIT LOWER CASE RECORD TYPE",
        Shown::Abend("ERROR: TYPE NOT VALIDSACTION_TYPE table. SQLCODE: -532"),
    ),
];

/// The Db2 masters: the types by code and the categories in key order, each description in
/// the right-trimmed form its column stores.
#[derive(Clone, Debug)]
struct Masters {
    /// `TR_TYPE` to `TR_DESCRIPTION`.
    types: BTreeMap<String, String>,
    /// `TRC_TYPE_CODE`, `TRC_TYPE_CATEGORY` and `TRC_CAT_DATA` of each category.
    categories: Vec<[String; 3]>,
}

impl Masters {
    /// The masters the CREADB21 load statements [`TYPE_LOAD`] and [`CATEGORY_LOAD`] seed,
    /// checked against the audited seed rows: the row counts (P-TRNTYPE-003, P-TRNTYCAT-004),
    /// the existing type with its description and the range of codes (P-TRNTYPE-004), the
    /// missing type (P-TRNTYPE-005) and the existing category (P-TRNTYCAT-005).
    fn loaded(doc: &Document) -> Self {
        let type_rows = load_rows::<2>(TYPE_LOAD, TYPE_TABLE);
        let types: BTreeMap<String, String> = type_rows
            .iter()
            .map(|[code, description]| (code.clone(), description.clone()))
            .collect();
        assert_eq!(
            types.len(),
            type_rows.len(),
            "{TYPE_LOAD} loads a type code twice: {type_rows:?}"
        );
        let mut categories = load_rows::<3>(CATEGORY_LOAD, CATEGORY_TABLE);
        categories.sort();
        let keys: BTreeSet<(&str, &str)> = categories
            .iter()
            .map(|[code, category, _]| (code.as_str(), category.as_str()))
            .collect();
        assert_eq!(
            keys.len(),
            categories.len(),
            "{CATEGORY_LOAD} loads a category key twice: {categories:?}"
        );

        let rows = |id: &str, table: &str| -> usize {
            assert_eq!(
                expected_value(doc, id, "table"),
                table,
                "{id} counts another table"
            );
            let rows = expected_value(doc, id, "rows");
            rows.parse()
                .unwrap_or_else(|error| panic!("{id} rows={rows} is no count: {error}"))
        };
        assert_eq!(
            types.len(),
            rows("P-TRNTYPE-003", "transaction_types"),
            "the {TYPE_LOAD} rows: {types:?}"
        );
        assert_eq!(
            categories.len(),
            rows("P-TRNTYCAT-004", "transaction_categories"),
            "the {CATEGORY_LOAD} rows: {categories:?}"
        );
        let existing = quoted_value(doc, "P-TRNTYPE-004", "value");
        let description = quoted_value(doc, "P-TRNTYPE-004", "tr_description");
        assert_eq!(
            types.get(&existing),
            Some(&description),
            "P-TRNTYPE-004 in {TYPE_LOAD}: {types:?}"
        );
        let codes = code_range(&expected_value(doc, "P-TRNTYPE-004", "codes"));
        assert!(
            types.keys().eq(&codes),
            "P-TRNTYPE-004 codes {codes:?} in {TYPE_LOAD}: {types:?}"
        );
        let missing = quoted_value(doc, "P-TRNTYPE-005", "value");
        assert!(
            !types.contains_key(&missing),
            "P-TRNTYPE-005: {TYPE_LOAD} loads type {missing}"
        );
        let category = [
            expected_value(doc, "P-TRNTYCAT-005", "tran_type_cd"),
            expected_value(doc, "P-TRNTYCAT-005", "tran_cat_cd"),
            quoted_value(doc, "P-TRNTYCAT-005", "trc_cat_data"),
        ];
        assert!(
            categories.contains(&category),
            "P-TRNTYCAT-005 {category:?} in {CATEGORY_LOAD}: {categories:?}"
        );
        assert_eq!(
            categories.len().to_string(),
            expected_value(doc, "P-TRNTYCAT-005", "categories"),
            "P-TRNTYCAT-005 categories in {CATEGORY_LOAD}"
        );
        Self { types, categories }
    }

    /// The rows [`CATEGORIES_SQL`] reads of these categories.
    fn category_rows(&self) -> Vec<Vec<String>> {
        self.categories.iter().map(|row| row.to_vec()).collect()
    }
}

/// The rows of the CREADB21 load statement at `path`, relative to the repository root, after
/// checking that it inserts into `table`: the `N` string literals of each `SELECT` line, in
/// file order, the last one right-trimmed as the description columns store it.
fn load_rows<const N: usize>(path: &str, table: &str) -> Vec<[String; N]> {
    let file = fixtures::repo_root().join(path);
    let text = fs::read_to_string(&file)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", file.display()));
    assert_eq!(
        text.lines()
            .filter(|line| line.trim().strip_prefix("INSERT INTO ") == Some(table))
            .count(),
        1,
        "{path} holds no single INSERT INTO {table}"
    );
    text.lines()
        .filter_map(|line| line.trim_start().strip_prefix("SELECT "))
        .map(|select| {
            let values = select_literals(select, path);
            let mut row = <[String; N]>::try_from(values).unwrap_or_else(|values| {
                panic!(
                    "{path}: SELECT {select:?} lists {} literals, not {N}",
                    values.len()
                )
            });
            if let Some(last) = row.last_mut() {
                let kept = last.trim_end_matches(' ').len();
                last.truncate(kept);
            }
            row
        })
        .collect()
}

/// The SQL string literals `select`, the text after a `SELECT` of `path`, lists before its
/// `FROM`, each with a doubled quote `''` read as one quote.
fn select_literals(select: &str, path: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = select.trim_start();
    loop {
        let body = rest.strip_prefix('\'').unwrap_or_else(|| {
            panic!("{path}: no string literal at {rest:?} of SELECT {select:?}")
        });
        let mut value = String::new();
        let mut chars = body.char_indices().peekable();
        let end = loop {
            match chars.next() {
                Some((_, '\'')) if chars.peek().is_some_and(|&(_, next)| next == '\'') => {
                    chars.next();
                    value.push('\'');
                }
                Some((at, '\'')) => break at + 1,
                Some((_, ch)) => value.push(ch),
                None => panic!("{path}: an unterminated literal in SELECT {select:?}"),
            }
        };
        values.push(value);
        rest = body[end..].trim_start();
        match rest.strip_prefix(',') {
            Some(next) => rest = next.trim_start(),
            None => break,
        }
    }
    assert!(
        rest.starts_with("FROM "),
        "{path}: SELECT {select:?} lists more than string literals before its FROM"
    );
    values
}

/// The codes of the inclusive range `range`, written `<first>-<last>` such as `01-07`, each
/// zero-padded to the width of `<first>`.
fn code_range(range: &str) -> Vec<String> {
    let (first, last) = range
        .split_once('-')
        .unwrap_or_else(|| panic!("{range:?} is no code range"));
    let number = |code: &str| -> u32 {
        code.parse()
            .unwrap_or_else(|error| panic!("{code:?} of {range:?} is no code: {error}"))
    };
    let width = first.len();
    (number(first)..=number(last))
        .map(|code| format!("{code:0width$}"))
        .collect()
}

/// The text between the double quotes of the code span of the first `key=` pair of the
/// `Expected result` of row `id`, for example `01` for ``value=`"01"` ``.
fn quoted_value(doc: &Document, id: &str, key: &str) -> String {
    let span = expected_span(doc, id, key);
    span.text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id} {key}={} is no quoted text", span.text))
        .to_owned()
}

/// One field of `WS-INPUT-REC`: its byte offset and length, as its section A row records them.
#[derive(Clone, Copy, Debug)]
struct Field {
    /// The offset of the field's first byte.
    offset: usize,
    /// The field's length in bytes.
    len: usize,
}

impl Field {
    /// The field of the layout row `id`, read from its `Offset` and `Length` cells.
    fn audited(doc: &Document, id: &str) -> Self {
        let bytes = |column: &str| -> usize {
            let cell = parity_cell(doc, id, column);
            cell.parse()
                .unwrap_or_else(|error| panic!("{id} {column} {cell:?} is no byte count: {error}"))
        };
        Self {
            offset: bytes(PARITY_LAYOUT_COLUMNS[0]),
            len: bytes(PARITY_LAYOUT_COLUMNS[1]),
        }
    }

    /// The offset just past the field.
    const fn end(self) -> usize {
        self.offset + self.len
    }

    /// The bytes of the field in `record`, a `WS-INPUT-REC` of ASCII text.
    fn of(self, record: &str) -> &str {
        record.get(self.offset..self.end()).unwrap_or_else(|| {
            panic!(
                "{record:?} holds no bytes {} to {}",
                self.offset,
                self.end()
            )
        })
    }
}

/// What `1003-TREAT-RECORD` performs for a record type, as P-COBTUPDT-006 pairs them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    /// `10031-INSERT-DB` (P-COBTUPDT-007).
    Insert,
    /// `10032-UPDATE-DB` (P-COBTUPDT-008).
    Update,
    /// `10033-DELETE-DB` (P-COBTUPDT-009).
    Delete,
    /// The comment display, and no statement.
    Comment,
    /// The invalid-type message and `9999-ABEND`, and no statement.
    Invalid,
}

/// The outcome class of one record the model treats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    /// A comment, ignored.
    Comment,
    /// A type inserted.
    Inserted,
    /// The description of a type updated.
    Updated,
    /// A type deleted.
    Deleted,
    /// `9999-ABEND` performed for an invalid record type or a failed statement, which changed
    /// nothing.
    Failed,
}

/// The counts the job reports for a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    /// Every record read, comments and failed records included: `read`.
    read: usize,
    /// Types inserted or updated: `written`.
    written: usize,
    /// Types deleted: `deleted`.
    deleted: usize,
    /// Records for which `9999-ABEND` was performed: `rejected`.
    failed: usize,
}

impl Counts {
    /// Counts one record with its `outcome`.
    fn add(&mut self, outcome: Outcome) {
        self.read += 1;
        match outcome {
            Outcome::Inserted | Outcome::Updated => self.written += 1,
            Outcome::Deleted => self.deleted += 1,
            Outcome::Failed => self.failed += 1,
            Outcome::Comment => {}
        }
    }

    /// The records whose statement changed the master: `posted`.
    const fn posted(self) -> usize {
        self.written + self.deleted
    }
}

/// What the model derives for a run over an input from the seeded masters.
#[derive(Debug)]
struct Maintained {
    /// The type master the run leaves, code to description.
    types: BTreeMap<String, String>,
    /// The outcome of each record, in file order.
    outcomes: Vec<Outcome>,
    /// The counts of the run.
    counts: Counts,
    /// The return code at `STOP RUN`, the job's exit code.
    return_code: i32,
}

/// The reviewed COBTUPDT rules the model applies, each read from its parity audit row.
#[derive(Debug)]
struct Cobtupdt {
    /// `INPUT-REC-TYPE` (P-COBTUPDT-001).
    rec_type: Field,
    /// `INPUT-REC-NUMBER`, the type code (P-COBTUPDT-002).
    code: Field,
    /// `INPUT-REC-DESC` (P-COBTUPDT-003).
    desc: Field,
    /// Each record type `1003-TREAT-RECORD` names, as stored, with its action.
    actions: Vec<(String, Action)>,
    /// The action for every other record type.
    other: Action,
    /// The display of each outcome that is no failure.
    displays: Vec<(Outcome, String)>,
    /// The return code at `STOP RUN` when a record failed.
    failed_return_code: i32,
    /// The return code at `STOP RUN` when no record failed.
    clean_return_code: i32,
}

impl Cobtupdt {
    /// The rules of the reviewed COBTUPDT rows and of the description column. Fails, quoting
    /// the cell, when a row no longer states a clause the model relies on.
    fn audited(doc: &Document) -> Self {
        let [rec_type, code, desc] = ["P-COBTUPDT-001", "P-COBTUPDT-002", "P-COBTUPDT-003"]
            .map(|id| Field::audited(doc, id));
        assert_eq!(
            (rec_type.end(), code.end(), desc.end()),
            (code.offset, desc.offset, INPUT_REC_LEN),
            "the WS-INPUT-REC fields follow each other up to the {INPUT_REC_LEN}-byte record"
        );
        let (actions, other, comment) = dispatch(doc, rec_type);
        let mut displays: Vec<(Outcome, String)> = comment
            .map(|text| (Outcome::Comment, text))
            .into_iter()
            .collect();
        let stored = format!("description stored as the {} input bytes", desc.len);
        displays.push((
            Outcome::Inserted,
            statement(
                doc,
                "P-COBTUPDT-007",
                &["SQLCODE 0", "< 0"],
                &[(1, "-803 duplicate"), (1, "and 9999-ABEND"), (1, &stored)],
            ),
        ));
        require(
            "P-COBTUPDT-007",
            parity_cell(doc, "P-COBTUPDT-007", PERSISTS),
            "a failed statement changes nothing",
        );
        displays.push((
            Outcome::Updated,
            statement(
                doc,
                "P-COBTUPDT-008",
                &["SQLCODE 0", "+100", "< 0"],
                &[(1, NO_RECORDS_FOUND), (2, "both with 9999-ABEND")],
            ),
        ));
        displays.push((
            Outcome::Deleted,
            statement(
                doc,
                "P-COBTUPDT-009",
                &["SQLCODE 0", "+100", "< 0"],
                &[
                    (1, NO_RECORDS_FOUND),
                    (2, "-532 when categories reference the type"),
                    (2, "and 9999-ABEND"),
                    (2, "only the Db2 master changes"),
                ],
            ),
        ));
        require(
            "P-TRNTYPE-002",
            &expected_value(doc, "P-TRNTYPE-002", "sql"),
            &format!("VARCHAR({}) right-trimmed", desc.len),
        );
        let (failed_return_code, clean_return_code) = return_codes(doc);
        Self {
            rec_type,
            code,
            desc,
            actions,
            other,
            displays,
            failed_return_code,
            clean_return_code,
        }
    }

    /// The action `1003-TREAT-RECORD` performs for the record type `rec_type`, compared as
    /// stored.
    fn action(&self, rec_type: &str) -> Action {
        self.actions
            .iter()
            .find(|(named, _)| named == rec_type)
            .map_or(self.other, |&(_, action)| action)
    }

    /// The display COBTUPDT shows after the `PROCESSING` line of a record with `outcome`, or
    /// `None` for a failure, which shows the `WS-RETURN-MSG` line of `9999-ABEND`.
    fn display(&self, outcome: Outcome) -> Option<&str> {
        if outcome == Outcome::Failed {
            return None;
        }
        let text = self
            .displays
            .iter()
            .find(|&&(shown, _)| shown == outcome)
            .map(|(_, text)| text.as_str());
        Some(text.unwrap_or_else(|| panic!("the audit gives no display for {outcome:?}")))
    }

    /// The model run over `records`, each a 53-byte `WS-INPUT-REC`, in file order, starting
    /// from `seed`.
    ///
    /// An insert of a code the master holds ends with -803 (P-COBTUPDT-007); an update or a
    /// delete of a code it does not hold ends with +100 (P-COBTUPDT-008, P-COBTUPDT-009); a
    /// delete of a type a category references ends with -532 (P-COBTUPDT-009). Each failure
    /// changes nothing. An inserted or updated description is the `INPUT-REC-DESC` bytes in the
    /// right-trimmed form of `TR_DESCRIPTION` (P-TRNTYPE-002). Only the type master changes, so
    /// the categories stay those of `seed`.
    fn maintain(&self, seed: &Masters, records: &[String]) -> Maintained {
        let mut types = seed.types.clone();
        let mut outcomes = Vec::with_capacity(records.len());
        let mut counts = Counts::default();
        for record in records {
            let code = self.code.of(record).to_owned();
            let description = || self.desc.of(record).trim_end_matches(' ').to_owned();
            let outcome = match self.action(self.rec_type.of(record)) {
                Action::Comment => Outcome::Comment,
                Action::Invalid => Outcome::Failed,
                Action::Insert => match types.entry(code) {
                    Entry::Vacant(vacant) => {
                        vacant.insert(description());
                        Outcome::Inserted
                    }
                    Entry::Occupied(_) => Outcome::Failed,
                },
                Action::Update => match types.get_mut(&code) {
                    Some(stored) => {
                        *stored = description();
                        Outcome::Updated
                    }
                    None => Outcome::Failed,
                },
                Action::Delete => {
                    let referenced = seed.categories.iter().any(|[owner, ..]| *owner == code);
                    if !referenced && types.remove(&code).is_some() {
                        Outcome::Deleted
                    } else {
                        Outcome::Failed
                    }
                }
            };
            counts.add(outcome);
            outcomes.push(outcome);
        }
        let return_code = if counts.failed > 0 {
            self.failed_return_code
        } else {
            self.clean_return_code
        };
        Maintained {
            types,
            outcomes,
            counts,
            return_code,
        }
    }
}

/// The record-type dispatch of `1003-TREAT-RECORD` (P-COBTUPDT-006): each quoted type of the
/// `Input condition`, as stored, with the action its `Expected result` alternative names, the
/// action of `other`, and the comment display.
///
/// An alternative `insert`, `update` or `delete` names its statement, one naming `9999-ABEND`
/// is a failure, and one that is a single quoted display is the comment.
fn dispatch(doc: &Document, rec_type: Field) -> (Vec<(String, Action)>, Action, Option<String>) {
    let id = "P-COBTUPDT-006";
    let conditions = parity_cell(doc, id, INPUT_CONDITION);
    let results = parity_cell(doc, id, EXPECTED_RESULT);
    let types = alternatives(conditions);
    let actions = alternatives(results);
    assert_eq!(
        types.len(),
        actions.len(),
        "{id} pairs each record type with one result: {conditions} | {results}"
    );
    let mut named = Vec::new();
    let mut other = None;
    let mut comment = None;
    for (condition, result) in types.into_iter().zip(actions) {
        let action = match result {
            "insert" => Action::Insert,
            "update" => Action::Update,
            "delete" => Action::Delete,
            _ if result.contains("9999-ABEND") => Action::Invalid,
            _ => {
                comment = Some(lone_display(id, result));
                Action::Comment
            }
        };
        if condition == "other" {
            assert!(
                other.replace(action).is_none(),
                "{id} names `other` twice: {conditions}"
            );
        } else {
            let literal = condition
                .strip_prefix('\'')
                .and_then(|quoted| quoted.strip_suffix('\''))
                .filter(|literal| literal.len() == rec_type.len)
                .unwrap_or_else(|| {
                    panic!(
                        "{id}: {condition:?} is no quoted {}-byte record type: {conditions}",
                        rec_type.len
                    )
                });
            named.push((literal.to_owned(), action));
        }
    }
    let other =
        other.unwrap_or_else(|| panic!("{id} gives no action for `other` types: {conditions}"));
    (named, other, comment)
}

/// The success display of the statement row `id`, its first `Expected result` alternative
/// ``display `"…"` ``, after checking that its `Input condition` lists exactly the `SQLCODE`
/// alternatives `sqlcodes`, and that each `(index, clause)` of `failures` is in the result
/// alternative `index`.
fn statement(doc: &Document, id: &str, sqlcodes: &[&str], failures: &[(usize, &str)]) -> String {
    let conditions = parity_cell(doc, id, INPUT_CONDITION);
    assert_eq!(
        alternatives(conditions),
        sqlcodes,
        "{id} tests other SQLCODE values than the model: {conditions}"
    );
    let results = parity_cell(doc, id, EXPECTED_RESULT);
    let listed = alternatives(results);
    assert_eq!(
        listed.len(),
        sqlcodes.len(),
        "{id} pairs each SQLCODE with one result: {results}"
    );
    for &(index, clause) in failures {
        require(id, listed[index], clause);
    }
    let success = listed[0]
        .strip_prefix("display ")
        .unwrap_or_else(|| panic!("{id} shows no display for SQLCODE 0: {results}"));
    lone_display(id, success)
}

/// The return codes `STOP RUN` ends with (P-COBTUPDT-010): the one `9999-ABEND` leaves when a
/// record failed and the one of a run without failure, read from the clause
/// `final return code <n> when any record failed, else <m>`.
fn return_codes(doc: &Document) -> (i32, i32) {
    let id = "P-COBTUPDT-010";
    let cell = parity_cell(doc, id, EXPECTED_RESULT);
    require(id, cell, "processing continues with the next record");
    cell.split_once("final return code ")
        .and_then(|(_, rest)| rest.split_once(" when any record failed, else "))
        .and_then(|(failed, rest)| {
            let clean: String = rest.chars().take_while(char::is_ascii_digit).collect();
            Some((failed.parse().ok()?, clean.parse().ok()?))
        })
        .unwrap_or_else(|| {
            panic!(
                "{id} no longer states `final return code <n> when any record failed, else <m>`: \
                 {cell}"
            )
        })
}

/// The alternatives of `cell`, separated by ` / ` outside code spans, each trimmed.
fn alternatives(cell: &str) -> Vec<&str> {
    const SEPARATOR: &[u8] = b" / ";
    let bytes = cell.as_bytes();
    let mut parts = Vec::new();
    let mut in_span = false;
    let mut start = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'`' {
            in_span = !in_span;
            at += 1;
        } else if !in_span && bytes[at..].starts_with(SEPARATOR) {
            // The separator is ASCII, so both slice bounds fall between characters.
            parts.push(cell[start..at].trim());
            at += SEPARATOR.len();
            start = at;
        } else {
            at += 1;
        }
    }
    parts.push(cell[start..].trim());
    parts
}

/// The text of `alternative` of row `id` when it is one code span holding a double-quoted
/// text and nothing else, such as `` `"IGNORING COMMENTED LINE"` ``, without its quotes.
fn lone_display(id: &str, alternative: &str) -> String {
    carddemo_audit::code_span(alternative)
        .filter(|span| {
            alternative
                .strip_prefix('`')
                .and_then(|inner| inner.strip_suffix('`'))
                == Some(span.text.as_str())
        })
        .and_then(|span| Some(span.text.strip_prefix('"')?.strip_suffix('"')?.to_owned()))
        .unwrap_or_else(|| panic!("{id}: {alternative:?} is no single quoted display"))
}

/// Fails, quoting the cell, unless `cell` of row `id` states `clause`.
fn require(id: &str, cell: &str, clause: &str) {
    assert!(
        cell.contains(clause),
        "{id} no longer states {clause:?}, which the model relies on: {cell}"
    );
}

/// Each catalog text the job displays, with the COBTUPDT row whose code spans quote it.
const AUDITED_TEXTS: [(&str, &str); 9] = [
    ("P-COBTUPDT-005", PROCESSING),
    ("P-COBTUPDT-006", IGNORING_COMMENTED_LINE),
    ("P-COBTUPDT-006", ERROR_TYPE_NOT_VALID),
    ("P-COBTUPDT-007", RECORD_INSERTED_SUCCESSFULLY),
    ("P-COBTUPDT-007", ERROR_ACCESSING),
    ("P-COBTUPDT-007", TRANSACTION_TYPE_TABLE_SQLCODE),
    ("P-COBTUPDT-008", RECORD_UPDATED_SUCCESSFULLY),
    ("P-COBTUPDT-008", NO_RECORDS_FOUND),
    ("P-COBTUPDT-009", RECORD_DELETED_SUCCESSFULLY),
];

/// The Db2 master of the types, in key order.
const TYPES_SQL: &str = "SELECT tr_type, tr_description FROM transaction_types ORDER BY tr_type";

/// The Db2 master of the categories, in key order.
const CATEGORIES_SQL: &str = "SELECT trc_type_code, trc_type_category, trc_cat_data \
     FROM transaction_categories ORDER BY trc_type_code, trc_type_category";

/// The `TRANTYPE` processing copy the reports read, in key order.
const TRANTYPE_FILE_SQL: &str =
    "SELECT tran_type, tran_type_desc FROM trantype_file ORDER BY tran_type";

/// The rows `sql` reads, each column as its text. Every column read here is `NOT NULL`.
fn rows(h: &Harness, sql: &str) -> Vec<Vec<String>> {
    block_on(h.stack().fetch_text_rows(sql))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| value.unwrap_or_else(|| panic!("a null in a NOT NULL column: {sql}")))
                .collect()
        })
        .collect()
}

/// A type master, code to description, as the rows [`TYPES_SQL`] reads.
fn type_rows(types: &BTreeMap<String, String>) -> Vec<Vec<String>> {
    types
        .iter()
        .map(|(code, description)| vec![code.clone(), description.clone()])
        .collect()
}

/// The three tables of transaction type data: the type and category masters and the
/// processing copy.
fn type_tables(h: &Harness) -> [Vec<Vec<String>>; 3] {
    [TYPES_SQL, CATEGORIES_SQL, TRANTYPE_FILE_SQL].map(|sql| rows(h, sql))
}

/// The texts the code spans of the `Expected result` cell of row `id` quote, without their
/// double quotes.
fn quoted_texts(doc: &Document, id: &str) -> Vec<String> {
    carddemo_audit::cell_code_spans(parity_cell(doc, id, PARITY_COMMON_COLUMNS[5]))
        .into_iter()
        .filter_map(|span| {
            let text = span.text.strip_prefix('"')?.strip_suffix('"')?;
            Some(text.to_owned())
        })
        .collect()
}

/// The string value of `key` in the JSON line `line`. The messages of this input hold no
/// character that JSON escapes, so the value runs to the next double quote.
fn json_text(line: &str, key: &str) -> String {
    let opener = format!(r#""{key}":""#);
    let (_, rest) = line
        .split_once(&opener)
        .unwrap_or_else(|| panic!("no {key} in {line}"));
    let (value, _) = rest
        .split_once('"')
        .unwrap_or_else(|| panic!("an unterminated {key} in {line}"));
    value.to_owned()
}

/// The `(level, message)` of every stdout event of record `number`, in order. Each event the
/// job logs for a record carries the fields `message`, `record` and `tran_type`.
fn record_events(stdout: &str, number: usize) -> Vec<(String, String)> {
    let field = format!(r#""record":{number},"#);
    stdout
        .lines()
        .filter(|line| line.contains(&field))
        .map(|line| (json_text(line, "level"), json_text(line, "message")))
        .collect()
}

/// Checks that `stdout` shows each record of [`RECORDS`], under its 1-based number, with its
/// `PROCESSING` display followed by what COBTUPDT displays for it, and no other event.
fn assert_record_events(stdout: &str) {
    for (index, &(record, shown)) in RECORDS.iter().enumerate() {
        let number = index + 1;
        let processing = format!("{PROCESSING}{record:<INPUT_REC_LEN$}");
        assert_eq!(
            record_events(stdout, number),
            [("INFO".to_owned(), processing), shown.event()],
            "record {number} {record:?}\n--- stdout ---\n{stdout}"
        );
    }
}

/// The fixture records, each the 53 bytes before its LF, after checking that the fixture holds
/// exactly [`RECORDS`], each space-padded to 53 bytes and followed by LF.
fn fixture_records() -> Vec<String> {
    let path = fixtures::repo_root().join(INPUT);
    let input = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let records: Vec<String> = RECORDS
        .iter()
        .map(|(record, _)| format!("{record:<INPUT_REC_LEN$}\n"))
        .collect();
    let lines: Vec<&str> = input.split_inclusive('\n').collect();
    assert_eq!(
        lines, records,
        "the fixture holds the records RECORDS expects, each 53 bytes followed by LF"
    );
    lines
        .iter()
        .map(|line| line.strip_suffix('\n').unwrap_or(line).to_owned())
        .collect()
}

/// A constraint trigger on `transaction_types`, deferred to `COMMIT`, that raises
/// [`REFUSED_AT_COMMIT`] for every row a statement inserted, updated or deleted, in the form of
/// the deferred triggers of `crates/persistence/tests/commit_failure.rs`. Every statement of a
/// run ends as it would without it, and only the `STOP RUN` commit fails.
const REFUSE_AT_COMMIT: &str = "\
    CREATE FUNCTION refuse_type_change_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$ \
    BEGIN RAISE EXCEPTION 'MNTTRDB2 type change refused at commit'; END $$; \
    CREATE CONSTRAINT TRIGGER refuse_type_change_at_commit \
    AFTER INSERT OR UPDATE OR DELETE ON transaction_types DEFERRABLE INITIALLY DEFERRED \
    FOR EACH ROW EXECUTE FUNCTION refuse_type_change_at_commit()";

/// A fresh directory of one test for its input file, from `fixtures::temp_dir`. Dropping it
/// removes the directory and its contents after a passing test and keeps them after a panic for
/// inspection, as [`Harness`] treats its output directories.
struct InputDir(PathBuf);

impl InputDir {
    /// Creates the directory.
    fn new() -> Self {
        Self(fixtures::temp_dir("mnttrdb2-input"))
    }

    /// Writes `records` to the file `name` of the directory, each followed by LF, and returns
    /// its absolute path.
    fn write(&self, name: &str, records: &[String]) -> PathBuf {
        let path = std::path::absolute(self.0.join(name)).unwrap_or_else(|error| {
            panic!(
                "{} has no absolute form: {error}",
                self.0.join(name).display()
            )
        });
        let mut text = String::new();
        for record in records {
            text.push_str(record);
            text.push('\n');
        }
        fs::write(&path, text)
            .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
        path
    }
}

impl Drop for InputDir {
    /// Removes the directory after a passing test and keeps it after a panic. Removal is best
    /// effort: a directory that is already gone, or one that cannot be removed, changes no test
    /// outcome.
    fn drop(&mut self) {
        if thread::panicking() {
            return;
        }
        if let Err(error) = fs::remove_dir_all(&self.0) {
            if error.kind() != ErrorKind::NotFound {
                eprintln!("cannot remove test directory {}: {error}", self.0.display());
            }
        }
    }
}

/// One JSON object of the job's stdout, read member by member.
struct JsonObject<'a> {
    /// The object's text, for failure messages.
    text: &'a str,
    /// Each member's key token, quotes included, with its raw value token, in order.
    members: Vec<(&'a str, &'a str)>,
}

impl<'a> JsonObject<'a> {
    /// The members of the JSON object `text`, which nothing but whitespace may surround. A
    /// value token is a string with its quotes and escapes, a number or literal as written, or a
    /// nested object or array whole. Strings are scanned escape by escape, so a quote, brace or
    /// comma inside one never ends a member.
    fn parse(text: &'a str) -> Self {
        let bytes = text.as_bytes();
        let mut at = skip_space(bytes, 0);
        assert_eq!(bytes.get(at), Some(&b'{'), "no JSON object: {text}");
        at = skip_space(bytes, at + 1);
        let mut members = Vec::new();
        if bytes.get(at) == Some(&b'}') {
            at += 1;
        } else {
            loop {
                assert_eq!(bytes.get(at), Some(&b'"'), "no key at byte {at}: {text}");
                let key_end = json_string_end(text, at);
                let colon = skip_space(bytes, key_end);
                assert_eq!(
                    bytes.get(colon),
                    Some(&b':'),
                    "no colon at byte {colon}: {text}"
                );
                let value = skip_space(bytes, colon + 1);
                let value_end = json_value_end(text, value);
                members.push((&text[at..key_end], &text[value..value_end]));
                at = skip_space(bytes, value_end);
                match bytes.get(at) {
                    Some(b',') => at = skip_space(bytes, at + 1),
                    Some(b'}') => {
                        at += 1;
                        break;
                    }
                    _ => panic!("no comma or closing brace at byte {at}: {text}"),
                }
            }
        }
        assert!(
            text[at..].trim().is_empty(),
            "text after the JSON object at byte {at}: {text}"
        );
        Self { text, members }
    }

    /// The raw value token of the member `key`, or `None` when the object has none. Fails when
    /// the object holds the key more than once.
    fn get(&self, key: &str) -> Option<&'a str> {
        let quoted = format!("\"{key}\"");
        let mut values = self
            .members
            .iter()
            .filter(|&&(name, _)| name == quoted)
            .map(|&(_, value)| value);
        let value = values.next();
        assert!(
            values.next().is_none(),
            "member {key} repeated in {}",
            self.text
        );
        value
    }

    /// The object value of the member `key`, or `None` when the object has no such member.
    fn object(&self, key: &str) -> Option<Self> {
        self.get(key).map(Self::parse)
    }

    /// Checks that each `(key, token)` of `expected` is exactly one member of the object, with
    /// exactly that raw value token: an integer unquoted, a string with its quotes.
    fn assert_tokens(&self, expected: &[(&str, String)], stdout: &str) {
        for (key, token) in expected {
            assert_eq!(
                self.get(key),
                Some(token.as_str()),
                "member {key} of {}\n--- stdout ---\n{stdout}",
                self.text
            );
        }
    }
}

/// The first byte of `bytes` at or after `at` that is no JSON whitespace, or the length.
fn skip_space(bytes: &[u8], at: usize) -> usize {
    bytes[at..]
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map_or(bytes.len(), |skipped| at + skipped)
}

/// The byte just past the JSON string that opens at byte `start` of `text`, each backslash
/// escape skipped whole.
fn json_string_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut at = start + 1;
    while let Some(&byte) = bytes.get(at) {
        match byte {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    panic!("an unterminated JSON string at byte {start}: {text}")
}

/// The byte just past the JSON value that starts at byte `start` of `text`: a string, a nested
/// object or array counted bracket by bracket outside strings, or a scalar up to the next
/// delimiter or whitespace.
fn json_value_end(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    match bytes.get(start) {
        Some(b'"') => json_string_end(text, start),
        Some(b'{' | b'[') => {
            let mut depth = 0_usize;
            let mut at = start;
            while let Some(&byte) = bytes.get(at) {
                at = match byte {
                    b'"' => json_string_end(text, at),
                    b'{' | b'[' => {
                        depth += 1;
                        at + 1
                    }
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return at + 1;
                        }
                        at + 1
                    }
                    _ => at + 1,
                };
            }
            panic!("an unterminated JSON value at byte {start}: {text}")
        }
        Some(_) => {
            let end = bytes[start..]
                .iter()
                .position(|&byte| matches!(byte, b',' | b'}' | b']') || byte.is_ascii_whitespace())
                .map_or(bytes.len(), |len| start + len);
            assert!(end > start, "an empty JSON value at byte {start}: {text}");
            end
        }
        None => panic!("no JSON value at byte {start}: {text}"),
    }
}

/// The `attributes` of the one `job` span the job writes to `stdout`: the JSON line whose
/// `type` is `"span"` and whose `name` is `"job"`.
fn job_span(stdout: &str) -> JsonObject<'_> {
    let spans: Vec<JsonObject<'_>> = stdout
        .lines()
        .map(JsonObject::parse)
        .filter(|line| {
            line.get("type") == Some(r#""span""#) && line.get("name") == Some(r#""job""#)
        })
        .map(|span| {
            span.object("attributes")
                .unwrap_or_else(|| panic!("a job span without attributes: {}", span.text))
        })
        .collect();
    let [attributes] = <[JsonObject<'_>; 1]>::try_from(spans).unwrap_or_else(|spans| {
        panic!("{} job spans, not 1\n--- stdout ---\n{stdout}", spans.len())
    });
    attributes
}

/// The `fields` of the one `WARN` event of `stdout` that carries `undone_written`: the event
/// the job logs when its `STOP RUN` commit fails.
fn commit_failure_fields(stdout: &str) -> JsonObject<'_> {
    let events: Vec<JsonObject<'_>> = stdout
        .lines()
        .map(JsonObject::parse)
        .filter(|line| line.get("level") == Some(r#""WARN""#))
        .filter_map(|event| event.object("fields"))
        .filter(|fields| fields.get("undone_written").is_some())
        .collect();
    let [fields] = <[JsonObject<'_>; 1]>::try_from(events).unwrap_or_else(|events| {
        panic!(
            "{} WARN events carry undone_written, not 1\n--- stdout ---\n{stdout}",
            events.len()
        )
    });
    fields
}

#[test]
fn maintenance_file_applies_audited_changes() {
    let doc = parity();
    for id in P_IDS {
        assert!(
            doc.find_row(id).is_some(),
            "the parity audit has no row {id}"
        );
    }
    for (id, text) in AUDITED_TEXTS {
        let quoted = quoted_texts(&doc, id);
        assert!(
            quoted.iter().any(|q| q == text),
            "{id} quotes no {text:?}: {quoted:?}"
        );
    }
    let rules = Cobtupdt::audited(&doc);
    let seed = Masters::loaded(&doc);
    let expected = rules.maintain(&seed, &fixture_records());
    for (index, (&(record, shown), &outcome)) in RECORDS.iter().zip(&expected.outcomes).enumerate()
    {
        let agrees = match shown {
            Shown::Display(text) => rules.display(outcome) == Some(text),
            Shown::Abend(_) => outcome == Outcome::Failed,
        };
        assert!(
            agrees,
            "record {} {record:?} shows {shown:?}, but the audited rules derive {outcome:?}",
            index + 1
        );
    }

    let h = Harness::new();
    let [types, categories, processing_copy] = type_tables(&h);
    assert_eq!(types, type_rows(&seed.types), "the seeded Db2 type master");
    assert_eq!(
        categories,
        seed.category_rows(),
        "the seeded Db2 category master"
    );

    let out = h.run_expect(&["mnttrdb2", "--input", INPUT], None, expected.return_code);

    let [types, after_categories, after_copy] = type_tables(&h);
    assert_eq!(
        types,
        type_rows(&expected.types),
        "the maintained Db2 type master"
    );
    assert_eq!(
        after_categories, categories,
        "the category master is unchanged"
    );
    assert_eq!(
        after_copy, processing_copy,
        "the TRANTYPE processing copy is unchanged"
    );
    assert_record_events(&String::from_utf8_lossy(&out.stdout));
}

#[test]
fn an_input_that_cannot_be_opened_ends_with_16_before_any_change() {
    // P-COBTUPDT-011, E-36: under JCL a missing INPFILE fails allocation and the step never
    // runs, so the job stops before its unit of work.
    let h = Harness::new();
    let before = type_tables(&h);

    let out = h.run_expect(
        &["mnttrdb2", "--input", MISSING_INPUT],
        None,
        SETUP_FAILURE_EXIT,
    );

    assert_eq!(type_tables(&h), before, "no transaction type data changed");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let setup_failure = format!("SETUP FAILURE: INPFILE {MISSING_INPUT}: ");
    assert!(stderr.starts_with(&setup_failure), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let open_failure = format!(r#""message":"{OPEN_FILE_NOT_OK}""#);
    assert!(
        stdout
            .lines()
            .any(|line| line.contains(r#""level":"ERROR""#) && line.contains(&open_failure)),
        "no {OPEN_FILE_NOT_OK:?} event\n--- stdout ---\n{stdout}"
    );
    assert!(!stdout.contains(PROCESSING), "a record was read\n{stdout}");
}

#[test]
fn a_file_without_failures_commits_every_change_and_ends_with_0() {
    // P-COBTUPDT-010, P-COBTUPDT-012: with no 9999-ABEND the return code STOP RUN ends with is
    // the one of a run without failure, and the commit keeps every statement.
    let doc = parity();
    let rules = Cobtupdt::audited(&doc);
    let seed = Masters::loaded(&doc);
    let fixture = fixture_records();
    let mixed = rules.maintain(&seed, &fixture);
    let clean: Vec<String> = fixture
        .iter()
        .zip(&mixed.outcomes)
        .filter(|&(_, &outcome)| outcome != Outcome::Failed)
        .map(|(record, _)| record.clone())
        .collect();
    let expected = rules.maintain(&seed, &clean);
    assert_eq!(
        expected.counts.failed, 0,
        "the records that succeed in the fixture succeed alone: {:?}",
        expected.outcomes
    );
    let dir = InputDir::new();
    let path = dir.write("INPFILE.txt", &clean);
    let input = path
        .to_str()
        .unwrap_or_else(|| panic!("{} is no UTF-8 argument", path.display()));

    let h = Harness::new();
    let [types, categories, processing_copy] = type_tables(&h);
    assert_eq!(types, type_rows(&seed.types), "the seeded Db2 type master");

    let out = h.run_expect(&["mnttrdb2", "--input", input], None, expected.return_code);

    let [after_types, after_categories, after_copy] = type_tables(&h);
    assert_eq!(
        after_types,
        type_rows(&expected.types),
        "every change of the file is committed"
    );
    assert_eq!(
        after_categories, categories,
        "the category master is unchanged"
    );
    assert_eq!(
        after_copy, processing_copy,
        "the TRANTYPE processing copy is unchanged"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let counts = expected.counts;
    job_span(&stdout).assert_tokens(
        &[
            ("job", r#""mnttrdb2""#.to_owned()),
            ("read", counts.read.to_string()),
            ("posted", counts.posted().to_string()),
            ("rejected", counts.failed.to_string()),
            ("written", counts.written.to_string()),
            ("deleted", counts.deleted.to_string()),
            ("exit_code", expected.return_code.to_string()),
        ],
        &stdout,
    );
}

#[test]
fn a_refused_stop_run_commit_ends_with_16_and_persists_nothing() {
    // P-COBTUPDT-012: every statement of the run succeeds or fails as it would, and the server
    // then refuses the STOP RUN commit, which undoes them all.
    let doc = parity();
    let rules = Cobtupdt::audited(&doc);
    let seed = Masters::loaded(&doc);
    let undone = rules.maintain(&seed, &fixture_records()).counts;

    assert!(
        REFUSE_AT_COMMIT.contains(REFUSED_AT_COMMIT),
        "the trigger raises another text than {REFUSED_AT_COMMIT:?}"
    );
    let h = Harness::new();
    block_on(h.stack().exec_sql(REFUSE_AT_COMMIT));
    let before = type_tables(&h);
    assert_eq!(
        before[0],
        type_rows(&seed.types),
        "the seeded Db2 type master"
    );

    let out = h.run_expect(&["mnttrdb2", "--input", INPUT], None, SETUP_FAILURE_EXIT);

    assert_eq!(
        type_tables(&h),
        before,
        "the refused commit persists no transaction type data"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    assert!(
        matches!(
            lines.as_slice(),
            [line] if line.starts_with("SETUP FAILURE: ") && line.contains(REFUSED_AT_COMMIT)
        ),
        "stderr is not one SETUP FAILURE line naming {REFUSED_AT_COMMIT:?}:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    job_span(&stdout).assert_tokens(
        &[
            ("job", r#""mnttrdb2""#.to_owned()),
            ("read", undone.read.to_string()),
            ("rejected", undone.failed.to_string()),
            ("posted", "0".to_owned()),
            ("written", "0".to_owned()),
            ("deleted", "0".to_owned()),
            ("exit_code", SETUP_FAILURE_EXIT.to_string()),
        ],
        &stdout,
    );
    commit_failure_fields(&stdout).assert_tokens(
        &[
            ("undone_written", undone.written.to_string()),
            ("undone_deleted", undone.deleted.to_string()),
        ],
        &stdout,
    );
    assert_record_events(&stdout);
}
