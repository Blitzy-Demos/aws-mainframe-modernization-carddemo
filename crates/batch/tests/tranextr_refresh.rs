//! The two versions of the transaction reference data (AAP 0.3.2): an edit of a type in the
//! Db2 master reaches the TRANREPT report only after `batch seed --only tranextr`.
//!
//! CTTU (COTRTUPC) writes the Db2 master `TRANSACTION_TYPE` (P-COTRTUPC-007), while CBTRN03C
//! reads the VSAM processing copies `TRANTYPE` and `TRANCATG` (P-CBTRN03C-001) and moves
//! their descriptions into every detail line (P-CBTRN03C-004, P-CBTRN03C-007). The two
//! versions start different: the Db2 load holds `PURCHASE` for type `01` (P-TRNTYPE-004),
//! the fixture `Purchase` (P-CVTRA03Y-005). TRANEXTR unloads both masters, and the
//! `TRANTYPE` and `TRANCATG` jobs reload the processing copies from that unload
//! [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L62-122], so only then do the reports show
//! the masters' texts.
//!
//! The test posts the fixture day, edits type `01`, reports, copies the masters and reports
//! again. The CTTU edit runs through `common::cttu_upsert`, the COTRTUPC rules and adapters
//! the API handler drives, because `carddemo-batch` cannot depend on `carddemo-api`;
//! `crates/api/tests/cttu_tran_type_upsert.rs` covers the HTTP framing.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use carddemo_audit::Document;
use common::{
    Harness, TRANREPT_CLOCK, block_on, cttu_upsert, expected_span, expected_value, parity,
    split_records,
};

/// The audit rows this test exercises.
///
/// TRANEXTR has no rows of its own: it is a seed job, replaced rather than parity-tested
/// (AAP 0.3.2), whose result is defined as the masters themselves. Its effect is therefore
/// checked against the masters row for row, and the seeded state of both versions is taken
/// from the rows of the Db2 load and the fixture.
const P_IDS: &[&str] = &[
    "P-CBTRN03C-001",
    "P-CBTRN03C-004",
    "P-CBTRN03C-007",
    "P-CBTRN03C-015",
    "P-CBTRN03C-020",
    "P-COTRTUPC-007",
    "P-COTRTUPC-017",
    "P-TRNTYPE-004",
    "P-TRNTYCAT-005",
    "P-CVTRA03Y-005",
];

/// The type the CTTU edit changes: `Purchase`, the type of most fixture records.
const TYPE_CODE: &str = "01";

/// The new description of [`TYPE_CODE`]: 15 characters, the width of `TRAN-REPORT-TYPE-DESC
/// X(15)` [app/cpy/CVTRA07Y.cpy:L22], unlike both `Purchase` and `PURCHASE` in those 15, and
/// only letters and a space, which the COTRTUPC description edit accepts (P-COTRTUPC-017).
const NEW_DESC: &str = "RETAIL PURCHASE";

/// The key of category `0001` of [`TYPE_CODE`], as [`descriptions`] builds category keys.
const CATEGORY_KEY: &str = "010001";

/// The report dataset and its record width, `FD-REPTFILE-REC X(133)`.
const REPORT: &str = "TRANREPT";
const REPORT_WIDTH: usize = 133;

/// The TRANREPT job with the JCL dates, and the TRANEXTR copy.
const TRANREPT: [&str; 1] = ["tranrept"];
const TRANEXTR: [&str; 3] = ["seed", "--only", "tranextr"];

/// The exit code of a POSTTRAN run that rejects records (AAP 0.5.3.5); the fixture day has
/// rejects.
const POSTTRAN_REJECTED: i32 = 4;

/// The `TRANSACTION-DETAIL-REPORT` fields this test reads, as byte offsets of the 133-byte
/// line [app/cpy/CVTRA07Y.cpy:L15-31]: the TRAN-ID, the type code, which the `'-'` filler
/// follows, its description, the category code, which a `'-'` follows too, and its
/// description.
const TRANS_ID: Range<usize> = 0..16;
const TYPE_CD: Range<usize> = 29..31;
const TYPE_DESC: Range<usize> = 32..47;
const CAT_CD: Range<usize> = 48..52;
const CAT_DESC: Range<usize> = 53..82;

/// The processing copy `trantype_file` and the master `transaction_types`, code and
/// description, in key order; columns as `migrations/0001_reference_data.sql` defines them.
const PROCESSING_TYPES_SQL: &str =
    "SELECT tran_type, tran_type_desc FROM trantype_file ORDER BY tran_type COLLATE \"C\"";
const MASTER_TYPES_SQL: &str =
    "SELECT tr_type, tr_description FROM transaction_types ORDER BY tr_type COLLATE \"C\"";

/// The processing copy `trancatg_file` and the master `transaction_categories`, type code,
/// four-digit category code and description, in key order. The copy keeps the category as the
/// `9(04)` number, so it is shown in the master's `CHAR(4)` form.
const PROCESSING_CATEGORIES_SQL: &str = "SELECT tran_type_cd, lpad(tran_cat_cd::text, 4, '0'), \
     tran_cat_type_desc FROM trancatg_file ORDER BY tran_type_cd COLLATE \"C\", tran_cat_cd";
const MASTER_CATEGORIES_SQL: &str = "SELECT trc_type_code, trc_type_category, trc_cat_data \
     FROM transaction_categories ORDER BY trc_type_code COLLATE \"C\", trc_type_category \
     COLLATE \"C\"";

/// Rows in their `PostgreSQL` text form.
type Rows = Vec<Vec<Option<String>>>;

#[test]
fn master_edit_reaches_report_only_after_tranextr() {
    let doc = parity();
    for id in P_IDS {
        assert!(
            doc.find_row(id).is_some(),
            "the parity audit has no row {id}"
        );
    }
    let h = Harness::new();
    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), POSTTRAN_REJECTED);

    // The seeded state: the processing copy and the Db2 master differ in case.
    let types_before = rows(&h, PROCESSING_TYPES_SQL);
    let categories_before = rows(&h, PROCESSING_CATEGORIES_SQL);
    let old_desc = description_of(&types_before, TYPE_CODE);
    assert_eq!(old_desc, audited(&doc, "P-CVTRA03Y-005", "tran_type_desc"));
    assert_eq!(
        description_of(&rows(&h, MASTER_TYPES_SQL), TYPE_CODE),
        audited(&doc, "P-TRNTYPE-004", "tr_description"),
    );

    // CTTU updates the existing type in the master, and only there.
    let saved = cttu_upsert(&h.db(), TYPE_CODE, NEW_DESC);
    assert!(
        !saved.inserted,
        "CTTU inserted type {TYPE_CODE}, which the Db2 load holds, instead of updating it"
    );
    assert_eq!(saved.master.tr_type.to_string(), TYPE_CODE);
    assert_eq!(saved.master.tr_description, NEW_DESC);
    assert_eq!(saved.message, audited(&doc, "P-COTRTUPC-007", "info"));
    assert_eq!(
        description_of(&rows(&h, MASTER_TYPES_SQL), TYPE_CODE),
        NEW_DESC
    );
    assert_eq!(
        rows(&h, PROCESSING_TYPES_SQL),
        types_before,
        "CTTU changed the processing copy `trantype_file`"
    );

    // The first report still shows the descriptions of the processing copies.
    let before = report(&h, &doc);
    assert_eq!(
        before[4],
        moved(&audited(&doc, "P-CBTRN03C-020", "text"), REPORT_WIDTH),
        "report line 5 before TRANEXTR"
    );
    assert_descriptions(&before, &types_before, &categories_before, "report 1");
    assert_eq!(
        type_descs(&before, TYPE_CODE),
        BTreeSet::from([moved(&old_desc, TYPE_DESC.len())])
    );
    assert!(
        before.iter().all(|line| !line.contains(NEW_DESC)),
        "the report before TRANEXTR shows the master's {NEW_DESC:?}"
    );

    // TRANEXTR copies both masters over the processing copies.
    let _ = h.run_expect(&TRANEXTR, Some(TRANREPT_CLOCK), 0);
    let types_after = rows(&h, PROCESSING_TYPES_SQL);
    let categories_after = rows(&h, PROCESSING_CATEGORIES_SQL);
    assert_eq!(
        types_after,
        rows(&h, MASTER_TYPES_SQL),
        "`trantype_file` after TRANEXTR"
    );
    assert_eq!(
        categories_after,
        rows(&h, MASTER_CATEGORIES_SQL),
        "`trancatg_file` after TRANEXTR"
    );
    assert_eq!(
        description_of(&categories_after, CATEGORY_KEY),
        audited(&doc, "P-TRNTYCAT-005", "trc_cat_data"),
    );

    // The second report shows the masters' texts, the edit included, and nothing else changes.
    let after = report(&h, &doc);
    assert_descriptions(&after, &types_after, &categories_after, "report 2");
    assert_eq!(
        type_descs(&after, TYPE_CODE),
        BTreeSet::from([NEW_DESC.to_owned()])
    );
    for (number, (old, new)) in (1..).zip(before.iter().zip(&after)) {
        assert_eq!(
            masked(old),
            masked(new),
            "report line {number} changed outside the descriptions"
        );
    }

    let generations = h.generations(REPORT);
    let names: Vec<_> = generations
        .iter()
        .filter_map(|path| path.file_name())
        .collect();
    assert_eq!(names, ["TRANREPT.G0001V00", "TRANREPT.G0002V00"]);
}

/// Runs TRANREPT under the pinned clock and returns the lines of the newest `TRANREPT`
/// generation, checked against the audited width and line count (P-CBTRN03C-015). The
/// descriptions change no line count.
fn report(h: &Harness, doc: &Document) -> Vec<String> {
    let _ = h.run_expect(&TRANREPT, Some(TRANREPT_CLOCK), 0);
    let id = "P-CBTRN03C-015";
    assert_eq!(expected_value(doc, id, "width"), REPORT_WIDTH.to_string());
    let lines = split_records(&h.newest(REPORT), REPORT_WIDTH);
    assert_eq!(
        lines.len().to_string(),
        expected_value(doc, id, "lines"),
        "TRANREPT line count"
    );
    lines
}

/// Checks that every detail line of `report`, named `label` in a failure, shows the
/// descriptions `types` and `categories` hold for its codes, as `1120-WRITE-DETAIL` moves the
/// `TRAN-TYPE-DESC` and `TRAN-CAT-TYPE-DESC` the lookups read into the line (P-CBTRN03C-004,
/// P-CBTRN03C-007).
fn assert_descriptions(report: &[String], types: &Rows, categories: &Rows, label: &str) {
    let (types, categories) = (descriptions(types), descriptions(categories));
    let lookup = |table: &BTreeMap<String, String>, key: &str, width: usize| {
        let text = table
            .get(key)
            .unwrap_or_else(|| panic!("{label}: no reference row has the key {key:?}"));
        moved(text, width)
    };
    let mut details = 0_usize;
    for line in report.iter().filter(|line| is_detail(line)) {
        let type_cd = field(line, TYPE_CD);
        let category = format!("{type_cd}{}", field(line, CAT_CD));
        let type_desc = lookup(&types, type_cd, TYPE_DESC.len());
        let cat_desc = lookup(&categories, &category, CAT_DESC.len());
        assert_eq!(field(line, TYPE_DESC), type_desc, "{label}: {line:?}");
        assert_eq!(field(line, CAT_DESC), cat_desc, "{label}: {line:?}");
        details += 1;
    }
    assert!(details > 0, "{label} holds no detail line");
}

/// The distinct type descriptions the detail lines of `report` show for the type `code`.
fn type_descs(report: &[String], code: &str) -> BTreeSet<String> {
    report
        .iter()
        .filter(|line| is_detail(line) && field(line, TYPE_CD) == code)
        .map(|line| field(line, TYPE_DESC).to_owned())
        .collect()
}

/// `line` with the type and category descriptions of a detail line blanked, so two reports
/// compare equal when they differ only there.
fn masked(line: &str) -> String {
    let mut bytes = line.as_bytes().to_vec();
    if is_detail(line) {
        bytes[TYPE_DESC].fill(b' ');
        bytes[CAT_DESC].fill(b' ');
    }
    String::from_utf8(bytes).unwrap_or_else(|error| panic!("report line {line:?}: {error}"))
}

/// Whether `line` is a `TRANSACTION-DETAIL-REPORT` line: a TRAN-ID of 16 digits, and the
/// `'-'` fillers after the type and the category codes. No header, separator or total line
/// has them.
fn is_detail(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes[TRANS_ID].iter().all(u8::is_ascii_digit)
        && bytes[TYPE_CD.end] == b'-'
        && bytes[CAT_CD.end] == b'-'
}

/// The text of the detail field at `range` of the 133-byte `line`.
fn field(line: &str, range: Range<usize>) -> &str {
    line.get(range)
        .unwrap_or_else(|| panic!("report line {line:?} is not ASCII at a CVTRA07Y field"))
}

/// `text` as a COBOL `MOVE` stores it in an `X(width)` field: cut on the right, or padded
/// there with spaces.
fn moved(text: &str, width: usize) -> String {
    format!("{text:<width$.width$}")
}

/// The rows `sql` reads from the test database.
fn rows(h: &Harness, sql: &str) -> Rows {
    block_on(h.stack().fetch_text_rows(sql))
}

/// The last column of every reference row, keyed by its other columns run together: the
/// description of a type code (`01`), or of a type and category code ([`CATEGORY_KEY`]).
fn descriptions(rows: &Rows) -> BTreeMap<String, String> {
    rows.iter()
        .map(|row| {
            let text = |cell: Option<&str>| {
                cell.unwrap_or_else(|| panic!("a null in reference row {row:?}"))
                    .to_owned()
            };
            let (description, key) = row
                .split_last()
                .unwrap_or_else(|| panic!("a reference row without columns"));
            let key: String = key.iter().map(|cell| text(cell.as_deref())).collect();
            (key, text(description.as_deref()))
        })
        .collect()
}

/// The description of the reference row whose key is `key`, as [`descriptions`] keys it.
fn description_of(rows: &Rows, key: &str) -> String {
    descriptions(rows)
        .remove(key)
        .unwrap_or_else(|| panic!("no reference row has the key {key:?}"))
}

/// The text of the quoted code span of `key=` in row `id`, without its quotes: the audited
/// value as the database stores it, before any padding to the span's width.
fn audited(doc: &Document, id: &str, key: &str) -> String {
    let span = expected_span(doc, id, key);
    span.text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("parity audit row {id}: {key}= holds no quoted text"))
        .to_owned()
}
