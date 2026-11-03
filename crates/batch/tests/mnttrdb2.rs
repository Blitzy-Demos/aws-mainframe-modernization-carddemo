//! `batch mnttrdb2`: the MNTTRDB2 job, `COBTUPDT` over an `INPFILE`, maintaining the Db2
//! master `CARDDEMO.TRANSACTION_TYPE` (table `transaction_types`) and nothing else.
//!
//! The fixture `crates/batch/tests/fixtures/mnttrdb2_input.txt` is test input in the
//! `WS-INPUT-REC` layout (P-COBTUPDT-001 to P-COBTUPDT-003): one record per branch of
//! `1003-TREAT-RECORD` and of each statement's `SQLCODE` test. Section D of the parity audit
//! holds no MNTTRDB2 rows, so each expected outcome in [`RECORDS`] applies the reviewed
//! COBTUPDT rules to the seeded masters (`DB2LTTYP`, `DB2LTCAT`) in file order: the record
//! type (P-COBTUPDT-006), the insert, update and delete with their displays and the E-30
//! codes -803 and -532 (P-COBTUPDT-007 to P-COBTUPDT-009), and `9999-ABEND`, whose
//! `WS-RETURN-MSG X(80)` is never cleared and whose return code 4 stays to `STOP RUN`
//! (P-COBTUPDT-010, P-COBTUPDT-012). The test checks the fixture against [`RECORDS`], so the
//! input and the expectations cannot drift apart, and the catalog texts against the code spans
//! of those rows.

pub mod common;

use std::fs;

use carddemo_audit::{Document, PARITY_COMMON_COLUMNS};
use carddemo_domain::messages::{
    ERROR_ACCESSING, ERROR_TYPE_NOT_VALID, IGNORING_COMMENTED_LINE, NO_RECORDS_FOUND,
    OPEN_FILE_NOT_OK, PROCESSING, RECORD_DELETED_SUCCESSFULLY, RECORD_INSERTED_SUCCESSFULLY,
    RECORD_UPDATED_SUCCESSFULLY, TRANSACTION_TYPE_TABLE_SQLCODE,
};
use carddemo_testkit::fixtures;
use common::{Harness, block_on, parity, parity_cell};

/// The audit rows this file proves: the `WS-INPUT-REC` layout, the COBTUPDT rules from the
/// job start to `STOP RUN`, and the open failure of an `INPFILE` that cannot be read.
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
];

/// The fixture `INPFILE`, relative to the repository root, the working directory of every
/// `batch` run of the harness.
const INPUT: &str = "crates/batch/tests/fixtures/mnttrdb2_input.txt";

/// An `INPFILE` that does not exist.
const MISSING_INPUT: &str = "crates/batch/tests/fixtures/mnttrdb2_input.absent";

/// The 53 bytes of `WS-INPUT-REC` each fixture record fills, space-padded.
const INPUT_REC_LEN: usize = 53;

/// `INPUT-REC-DESC X(50)` at offset 3 of `WS-INPUT-REC` (P-COBTUPDT-003).
const DESC_OFFSET: usize = 3;

/// The width of `INPUT-REC-DESC`.
const DESC_LEN: usize = 50;

/// The `WS-RETURN-MSG PIC X(80)` line `9999-ABEND` displays.
const RETURN_MSG_LEN: usize = 80;

/// `MOVE 4 TO RETURN-CODE` of `9999-ABEND`, which no later record resets (P-COBTUPDT-010).
const ABEND_RETURN_CODE: i32 = 4;

/// The exit code of an `INPFILE` that cannot be opened, before any change (P-COBTUPDT-011,
/// E-36).
const SETUP_FAILURE_EXIT: i32 = 16;

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

/// `transaction_types` as `DB2LTTYP` seeds it, the state every outcome above depends on.
const SEEDED_TYPES: [(&str, &str); 7] = [
    ("01", "PURCHASE"),
    ("02", "PAYMENT"),
    ("03", "CREDIT"),
    ("04", "AUTHORIZATION"),
    ("05", "REFUND"),
    ("06", "REVERAL"),
    ("07", "ADJUSTMENT"),
];

/// `transaction_types` after the run: 06 updated and 08 added; 09 added and deleted again; 01
/// kept by the failed insert and the restricted delete; 99 never present. Descriptions are
/// stored right-trimmed, the column's form of the 50 input bytes.
const MAINTAINED_TYPES: [(&str, &str); 8] = [
    ("01", "PURCHASE"),
    ("02", "PAYMENT"),
    ("03", "CREDIT"),
    ("04", "AUTHORIZATION"),
    ("05", "REFUND"),
    ("06", "REVERSAL"),
    ("07", "ADJUSTMENT"),
    ("08", "BALANCE TRANSFER"),
];

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

/// `(code, description)` pairs as the rows [`TYPES_SQL`] reads.
fn type_rows(types: &[(&str, &str)]) -> Vec<Vec<String>> {
    types
        .iter()
        .map(|&(code, description)| vec![code.to_owned(), description.to_owned()])
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
    let path = fixtures::repo_root().join(INPUT);
    let input = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let records: Vec<String> = RECORDS
        .iter()
        .map(|(record, _)| format!("{record:<INPUT_REC_LEN$}\n"))
        .collect();
    assert_eq!(
        input.split_inclusive('\n').collect::<Vec<_>>(),
        records,
        "the fixture holds the records RECORDS expects, each 53 bytes followed by LF"
    );

    let h = Harness::new();
    let [types, categories, processing_copy] = type_tables(&h);
    assert_eq!(
        types,
        type_rows(&SEEDED_TYPES),
        "the seeded Db2 type master"
    );
    assert!(
        categories.iter().any(|row| row[0] == "01"),
        "categories of type 01 restrict its delete: {categories:?}"
    );

    let out = h.run_expect(&["mnttrdb2", "--input", INPUT], None, ABEND_RETURN_CODE);

    let [types, after_categories, after_copy] = type_tables(&h);
    assert_eq!(
        types,
        type_rows(&MAINTAINED_TYPES),
        "the maintained Db2 type master"
    );
    // P-COBTUPDT-007: the stored description is the 50 input bytes of record 2, trimmed.
    let added = format!("{:<INPUT_REC_LEN$}", RECORDS[1].0);
    assert_eq!(
        format!("{:<DESC_LEN$}", MAINTAINED_TYPES[7].1),
        added[DESC_OFFSET..]
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
    for (index, &(record, shown)) in RECORDS.iter().enumerate() {
        let number = index + 1;
        let processing = format!("{PROCESSING}{record:<INPUT_REC_LEN$}");
        assert_eq!(
            record_events(&stdout, number),
            [("INFO".to_owned(), processing), shown.event()],
            "record {number} {record:?}\n--- stdout ---\n{stdout}"
        );
    }
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
