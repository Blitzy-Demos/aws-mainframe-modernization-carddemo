//! POSTTRAN over the fixture day: the `DALYREJS(+1)` reject set, byte for byte, and the
//! posting effects of the accepted records, against audit section D (AAP 0.9.3).
//!
//! `batch posttran` runs CBTRN02C over the 300 records of `app/data/ASCII/dailytran.txt` from
//! the seeded state, with the processing clock pinned. Every expected value is read at run time
//! from `docs/parity/parity-oracle-audit.md` through `carddemo_audit`, never written here:
//!
//! - D.1 POSTTRAN reject set: one row per reject in file order, then the end-of-job counts and
//!   the exit code;
//! - D.2 Posting effects: the balance and cycle totals of every account, the balance of every
//!   category key after the run, and the posted transaction master;
//! - F: the format and the pinned value of `TRAN-PROC-TS`;
//! - A: the byte ranges of the reject record and of the input and category balance fields the
//!   checks read;
//! - B: the reject reasons of `1500-VALIDATE-TRAN`, whose texts must equal the
//!   `carddemo_domain::messages` constants the binary writes.
//!
//! The audited reasons are asserted as they stand, defect-like behaviour included (E-15): 103
//! overwrites 102, so a 102 row must also have passed the expiration test, and a 109 rewrite
//! failure is never written, so the reject file holds the audited rows and nothing else.
//!
//! AAP 0.9.1 lets a parity test rely only on reviewed rows: every row this file reads must carry
//! `pass`, and only `pass`, in its review log, or the test fails naming it. The first test checks
//! the audit against itself and the fixtures without Docker; the second runs the binary.

pub mod common;

use std::array;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::iter;
use std::ops::Range;
use std::str::FromStr;

use carddemo_audit::{
    Document, KeyValue, PARITY_COMMON_COLUMNS, PARITY_LAYOUT_COLUMNS, REVIEW_LOG_COLUMNS, Row,
    Table,
};
use carddemo_domain::messages::{
    ACCOUNT_RECORD_NOT_FOUND, INVALID_CARD_NUMBER_FOUND, OVERLIMIT_TRANSACTION,
    TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
};
use carddemo_testkit::fixtures;
use common::{
    Harness, LIVE_MASTER_COLUMNS, TRANREPT_CLOCK, block_on, expected_span, fixture_lines, parity,
    parity_cell, split_records,
};
use rust_decimal::Decimal;

/// The condition code of a POSTTRAN run that rejected a record: CBTRN02C moves 4 to
/// `RETURN-CODE` [app/cbl/CBTRN02C.cbl:L229-231] (AAP 0.5.3.5). `carddemo-batch` has no library
/// target, so `crates/batch/src/exit.rs` cannot be imported.
const RECORDS_REJECTED: i32 = 4;

/// `DCB=(RECFM=F,LRECL=430)` of `DALYREJS(+1)` [app/jcl/POSTTRAN.jcl:L34-38].
const REJECT_LRECL: usize = 430;

/// The GDG base of the rejects, and the generation a first run creates (AAP 0.5.3.5).
const REJECTS: &str = "DALYREJS";
const FIRST_REJECTS: &str = "DALYREJS.G0001V00";

/// The `DALYTRAN DD` input [app/jcl/POSTTRAN.jcl:L30-31] and the seeded `TCATBALF` records,
/// under `app/data/ASCII`.
const DAILY_FILE: &str = "dailytran.txt";
const TCATBAL_FILE: &str = "tcatbal.txt";

/// The `###` subsections of audit section D that hold the POSTTRAN expectations.
const REJECT_SET: &str = "D.1 POSTTRAN reject set";
const POSTING_EFFECTS: &str = "D.2 Posting effects";

/// The derivation the posted-transactions row gives for its TRAN-IDs, which this file computes
/// from the fixture and the reject rows.
const POSTED_DERIVATION: &str = "every dailytran record not listed in D.1, in file order";

/// The account columns of the D.2 account rows, in the order of [`ACCOUNTS_SQL`].
const ACCOUNT_AMOUNTS: [&str; 3] = [
    "acct_curr_bal",
    "acct_curr_cyc_credit",
    "acct_curr_cyc_debit",
];

/// Every account after the run, columns as `migrations/0002_master_data.sql` names them.
const ACCOUNTS_SQL: &str = "SELECT account_id::text, acct_curr_bal::text, \
     acct_curr_cyc_credit::text, acct_curr_cyc_debit::text FROM accounts ORDER BY account_id";

/// The live transaction master's records per account, through the cross-reference
/// `1500-A-LOOKUP-XREF` reads by card number. Legacy reads name only the view (E-09).
const POSTED_PER_ACCOUNT_SQL: &str = "SELECT x.xref_acct_id::text, count(*)::text \
     FROM transaction_master t JOIN card_xref x ON x.xref_card_num = t.tran_card_num \
     GROUP BY x.xref_acct_id ORDER BY x.xref_acct_id";

/// Every category balance after the run, columns as `migrations/0003_transactions.sql` names
/// them.
const BALANCES_SQL: &str = "SELECT trancat_acct_id::text, trancat_type_cd, trancat_cd::text, \
     tran_cat_bal::text FROM transaction_category_balances";

const ID: &str = PARITY_COMMON_COLUMNS[0];
const INPUT_CONDITION: &str = PARITY_COMMON_COLUMNS[4];
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];
const OFFSET: &str = PARITY_LAYOUT_COLUMNS[0];
const LENGTH: &str = PARITY_LAYOUT_COLUMNS[1];
const ROW_ID: &str = REVIEW_LOG_COLUMNS[0];
const VERDICT: &str = REVIEW_LOG_COLUMNS[1];

/// The only review verdict a row this file reads may carry.
const PASS: &str = "pass";

/// Section A rows whose `Offset` and `Length` the checks use: the reject record
/// [app/cbl/CBTRN02C.cbl:L176-182], the `DALYTRAN-RECORD` fields compared with the audit and the
/// live master (CVTRA06Y), and the `TRAN-CAT-KEY` of the seeded balances (CVTRA01Y).
const REJECT_TRAN_DATA: &str = "P-CBTRN02C-001";
const REJECT_REASON: &str = "P-CBTRN02C-002";
const REJECT_DESCRIPTION: &str = "P-CBTRN02C-003";
const DALYTRAN_ID: &str = "P-CVTRA06Y-001";
const DALYTRAN_TYPE_CD: &str = "P-CVTRA06Y-002";
const DALYTRAN_CARD_NUM: &str = "P-CVTRA06Y-011";
const DALYTRAN_ORIG_TS: &str = "P-CVTRA06Y-012";
const TRAN_CAT_KEY: [&str; 3] = ["P-CVTRA01Y-001", "P-CVTRA01Y-002", "P-CVTRA01Y-003"];

/// The section B rows of the reject reasons, each with the catalog text its `reason=` must
/// carry: `1500-A-LOOKUP-XREF` (100) and `1500-B-LOOKUP-ACCT` (101, 102, 103).
const REASON_ROWS: [(&str, &str); 4] = [
    ("P-CBTRN02C-008", INVALID_CARD_NUMBER_FOUND),
    ("P-CBTRN02C-009", ACCOUNT_RECORD_NOT_FOUND),
    ("P-CBTRN02C-010", OVERLIMIT_TRANSACTION),
    ("P-CBTRN02C-011", TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION),
];

/// The section F row of `TRAN-PROC-TS`: its `format_regex` and its `pinned` value.
const PROC_TS: &str = "P-CBTRN02C-220";

/// Every audit row this file checks; each is looked up by its ID, so the literals are both the
/// lookup keys and the citations `parity_audit_coverage.rs` counts.
///
/// - Section A: the layout rows above.
/// - Section B, CBTRN02C, the rules the run proves: the main loop (006), the reject reasons
///   (008 to 011), `2500-WRITE-REJECT-REC` (012), `2000-POST-TRANSACTION` (013),
///   `Z-GET-DB2-FORMAT-TIMESTAMP` (014), `2700-UPDATE-TCATBAL` with its create and update
///   (015 to 017), `2800-UPDATE-ACCOUNT-REC` (018), `2900-WRITE-TRANSACTION-FILE` (019) and the
///   `RETURN-CODE` (020).
/// - Section D: every row of D.1 (the 38 rejects, then the counts) and of D.2 (the 50
///   accounts, the 100 category balances, then the posted master).
/// - Section F: [`PROC_TS`].
const P_IDS: &[&str] = &[
    "P-CBTRN02C-001",
    "P-CBTRN02C-002",
    "P-CBTRN02C-003",
    "P-CVTRA06Y-001",
    "P-CVTRA06Y-002",
    "P-CVTRA06Y-011",
    "P-CVTRA06Y-012",
    "P-CVTRA01Y-001",
    "P-CVTRA01Y-002",
    "P-CVTRA01Y-003",
    "P-CBTRN02C-006",
    "P-CBTRN02C-008",
    "P-CBTRN02C-009",
    "P-CBTRN02C-010",
    "P-CBTRN02C-011",
    "P-CBTRN02C-012",
    "P-CBTRN02C-013",
    "P-CBTRN02C-014",
    "P-CBTRN02C-015",
    "P-CBTRN02C-016",
    "P-CBTRN02C-017",
    "P-CBTRN02C-018",
    "P-CBTRN02C-019",
    "P-CBTRN02C-020",
    "P-CBTRN02C-027",
    "P-CBTRN02C-028",
    "P-CBTRN02C-029",
    "P-CBTRN02C-030",
    "P-CBTRN02C-031",
    "P-CBTRN02C-032",
    "P-CBTRN02C-033",
    "P-CBTRN02C-034",
    "P-CBTRN02C-035",
    "P-CBTRN02C-036",
    "P-CBTRN02C-037",
    "P-CBTRN02C-038",
    "P-CBTRN02C-039",
    "P-CBTRN02C-040",
    "P-CBTRN02C-041",
    "P-CBTRN02C-042",
    "P-CBTRN02C-043",
    "P-CBTRN02C-044",
    "P-CBTRN02C-045",
    "P-CBTRN02C-046",
    "P-CBTRN02C-047",
    "P-CBTRN02C-048",
    "P-CBTRN02C-049",
    "P-CBTRN02C-050",
    "P-CBTRN02C-051",
    "P-CBTRN02C-052",
    "P-CBTRN02C-053",
    "P-CBTRN02C-054",
    "P-CBTRN02C-055",
    "P-CBTRN02C-056",
    "P-CBTRN02C-057",
    "P-CBTRN02C-058",
    "P-CBTRN02C-059",
    "P-CBTRN02C-060",
    "P-CBTRN02C-061",
    "P-CBTRN02C-062",
    "P-CBTRN02C-063",
    "P-CBTRN02C-064",
    "P-CBTRN02C-065",
    "P-CBTRN02C-066",
    "P-CBTRN02C-067",
    "P-CBTRN02C-068",
    "P-CBTRN02C-069",
    "P-CBTRN02C-070",
    "P-CBTRN02C-071",
    "P-CBTRN02C-072",
    "P-CBTRN02C-073",
    "P-CBTRN02C-074",
    "P-CBTRN02C-075",
    "P-CBTRN02C-076",
    "P-CBTRN02C-077",
    "P-CBTRN02C-078",
    "P-CBTRN02C-079",
    "P-CBTRN02C-080",
    "P-CBTRN02C-081",
    "P-CBTRN02C-082",
    "P-CBTRN02C-083",
    "P-CBTRN02C-084",
    "P-CBTRN02C-085",
    "P-CBTRN02C-086",
    "P-CBTRN02C-087",
    "P-CBTRN02C-088",
    "P-CBTRN02C-089",
    "P-CBTRN02C-090",
    "P-CBTRN02C-091",
    "P-CBTRN02C-092",
    "P-CBTRN02C-093",
    "P-CBTRN02C-094",
    "P-CBTRN02C-095",
    "P-CBTRN02C-096",
    "P-CBTRN02C-097",
    "P-CBTRN02C-098",
    "P-CBTRN02C-099",
    "P-CBTRN02C-100",
    "P-CBTRN02C-101",
    "P-CBTRN02C-102",
    "P-CBTRN02C-103",
    "P-CBTRN02C-104",
    "P-CBTRN02C-105",
    "P-CBTRN02C-106",
    "P-CBTRN02C-107",
    "P-CBTRN02C-108",
    "P-CBTRN02C-109",
    "P-CBTRN02C-110",
    "P-CBTRN02C-111",
    "P-CBTRN02C-112",
    "P-CBTRN02C-113",
    "P-CBTRN02C-114",
    "P-CBTRN02C-115",
    "P-CBTRN02C-116",
    "P-CBTRN02C-117",
    "P-CBTRN02C-118",
    "P-CBTRN02C-119",
    "P-CBTRN02C-120",
    "P-CBTRN02C-121",
    "P-CBTRN02C-122",
    "P-CBTRN02C-123",
    "P-CBTRN02C-124",
    "P-CBTRN02C-125",
    "P-CBTRN02C-126",
    "P-CBTRN02C-127",
    "P-CBTRN02C-128",
    "P-CBTRN02C-129",
    "P-CBTRN02C-130",
    "P-CBTRN02C-131",
    "P-CBTRN02C-132",
    "P-CBTRN02C-133",
    "P-CBTRN02C-134",
    "P-CBTRN02C-135",
    "P-CBTRN02C-136",
    "P-CBTRN02C-137",
    "P-CBTRN02C-138",
    "P-CBTRN02C-139",
    "P-CBTRN02C-140",
    "P-CBTRN02C-141",
    "P-CBTRN02C-142",
    "P-CBTRN02C-143",
    "P-CBTRN02C-144",
    "P-CBTRN02C-145",
    "P-CBTRN02C-146",
    "P-CBTRN02C-147",
    "P-CBTRN02C-148",
    "P-CBTRN02C-149",
    "P-CBTRN02C-150",
    "P-CBTRN02C-151",
    "P-CBTRN02C-152",
    "P-CBTRN02C-153",
    "P-CBTRN02C-154",
    "P-CBTRN02C-155",
    "P-CBTRN02C-156",
    "P-CBTRN02C-157",
    "P-CBTRN02C-158",
    "P-CBTRN02C-159",
    "P-CBTRN02C-160",
    "P-CBTRN02C-161",
    "P-CBTRN02C-162",
    "P-CBTRN02C-163",
    "P-CBTRN02C-164",
    "P-CBTRN02C-165",
    "P-CBTRN02C-166",
    "P-CBTRN02C-167",
    "P-CBTRN02C-168",
    "P-CBTRN02C-169",
    "P-CBTRN02C-170",
    "P-CBTRN02C-171",
    "P-CBTRN02C-172",
    "P-CBTRN02C-173",
    "P-CBTRN02C-174",
    "P-CBTRN02C-175",
    "P-CBTRN02C-176",
    "P-CBTRN02C-177",
    "P-CBTRN02C-178",
    "P-CBTRN02C-179",
    "P-CBTRN02C-180",
    "P-CBTRN02C-181",
    "P-CBTRN02C-182",
    "P-CBTRN02C-183",
    "P-CBTRN02C-184",
    "P-CBTRN02C-185",
    "P-CBTRN02C-186",
    "P-CBTRN02C-187",
    "P-CBTRN02C-188",
    "P-CBTRN02C-189",
    "P-CBTRN02C-190",
    "P-CBTRN02C-191",
    "P-CBTRN02C-192",
    "P-CBTRN02C-193",
    "P-CBTRN02C-194",
    "P-CBTRN02C-195",
    "P-CBTRN02C-196",
    "P-CBTRN02C-197",
    "P-CBTRN02C-198",
    "P-CBTRN02C-199",
    "P-CBTRN02C-200",
    "P-CBTRN02C-201",
    "P-CBTRN02C-202",
    "P-CBTRN02C-203",
    "P-CBTRN02C-204",
    "P-CBTRN02C-205",
    "P-CBTRN02C-206",
    "P-CBTRN02C-207",
    "P-CBTRN02C-208",
    "P-CBTRN02C-209",
    "P-CBTRN02C-210",
    "P-CBTRN02C-211",
    "P-CBTRN02C-212",
    "P-CBTRN02C-213",
    "P-CBTRN02C-214",
    "P-CBTRN02C-215",
    "P-CBTRN02C-216",
    "P-CBTRN02C-220",
];

#[test]
fn posttran_audit_rows_are_reviewed_complete_and_consistent_with_the_fixtures() {
    // Every check runs while the expectations are read, so no container is needed.
    Expectations::read(&parity());
}

#[test]
fn posttran_over_the_fixture_day_writes_the_audited_rejects_and_postings() {
    let expected = Expectations::read(&parity());
    let h = Harness::new();
    assert!(
        h.generations(REJECTS).is_empty(),
        "the seeded state holds no {REJECTS} generation"
    );
    let input_path = fixtures::ascii_dir().join(DAILY_FILE);
    let input = input_path
        .to_str()
        .unwrap_or_else(|| panic!("{} is not UTF-8", input_path.display()));

    // P-CBTRN02C-020: RETURN-CODE 4, because records were rejected.
    let _ = h.run_expect(
        &["posttran", "--input", input],
        Some(TRANREPT_CLOCK),
        RECORDS_REJECTED,
    );

    let generations: Vec<String> = h
        .generations(REJECTS)
        .iter()
        .filter_map(|path| Some(path.file_name()?.to_str()?.to_owned()))
        .collect();
    assert_eq!(
        generations,
        [FIRST_REJECTS],
        "one run catalogs one generation"
    );
    assert_reject_file(&h.newest(REJECTS), &expected);
    assert_accounts(&h, &expected.accounts);
    assert_balances(&h, &expected.balances);
    assert_live_master(&h, &expected);
}

/// What the audit expects of the run, read once and checked against the audit's own rules and
/// the fixture bytes as it is read.
struct Expectations {
    layout: Layout,
    /// The D.1 reject rows, in their listed (file) order.
    rejects: Vec<RejectRow>,
    counts: Counts,
    accounts: Vec<AccountRow>,
    balances: Vec<BalanceRow>,
    posted: Posted,
    format: TimestampFormat,
}

impl Expectations {
    /// Reads every row of [`P_IDS`] after checking that each has passed review and that the
    /// list cites exactly the POSTTRAN rows of section D.
    ///
    /// # Panics
    ///
    /// Panics, naming the row, on an unreviewed or missing row, a value that does not parse,
    /// or an expectation that contradicts the audited rules or the fixture bytes.
    fn read(doc: &Document) -> Self {
        assert_rows_reviewed_and_cited(doc);
        let layout = Layout::read(doc);
        let daily = fixture_lines(DAILY_FILE);
        let (rejects, counts) = read_reject_set(doc, &daily, &layout);
        let format = TimestampFormat::read(doc);
        let mut accounts = Vec::new();
        let mut balances = Vec::new();
        let mut posted = None;
        let seeded_keys = seeded_balance_keys(&layout);
        for values in subsection_values(doc, POSTING_EFFECTS) {
            if values.has("acct_curr_bal") {
                accounts.push(AccountRow::read(&values));
            } else if values.has("tran_cat_bal") {
                balances.push(BalanceRow::read(&values, &seeded_keys));
            } else if values.has("tran_ids") {
                assert!(
                    posted.is_none(),
                    "audit row {}: a second posted row",
                    values.id
                );
                posted = Some(Posted::read(
                    &values, &daily, &rejects, &counts, &layout, &format,
                ));
            } else {
                panic!(
                    "audit row {}: no account, balance or posted values",
                    values.id
                );
            }
        }
        let posted = posted.unwrap_or_else(|| panic!("{POSTING_EFFECTS} has no posted row"));
        let posted_to_accounts: usize = accounts.iter().map(|row| row.posted_records).sum();
        assert_eq!(
            posted_to_accounts, counts.posted,
            "the D.2 posted_records of the accounts must add up to the posted count"
        );
        Self {
            layout,
            rejects,
            counts,
            accounts,
            balances,
            posted,
            format,
        }
    }
}

/// Every row of [`P_IDS`] exists and carries only `pass` verdicts (AAP 0.9.1); every row a
/// lookup names is in [`P_IDS`]; and [`P_IDS`] holds exactly the rows of D.1 and D.2.
fn assert_rows_reviewed_and_cited(doc: &Document) {
    let mut verdicts: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            if let (Some(id), Some(verdict)) = (table.cell(row, ROW_ID), table.cell(row, VERDICT)) {
                verdicts.entry(id).or_default().push(verdict);
            }
        }
    }
    for &id in P_IDS {
        assert!(
            doc.find_row(id).is_some(),
            "the parity audit has no row {id}"
        );
        let given = verdicts.get(id).map(Vec::as_slice).unwrap_or_default();
        assert!(
            !given.is_empty() && given.iter().all(|verdict| *verdict == PASS),
            "audit row {id} has not passed review (verdicts {given:?}); AAP 0.9.1 forbids a \
             parity test to rely on it"
        );
    }
    let looked_up = [REJECT_TRAN_DATA, REJECT_REASON, REJECT_DESCRIPTION]
        .into_iter()
        .chain([
            DALYTRAN_ID,
            DALYTRAN_TYPE_CD,
            DALYTRAN_CARD_NUM,
            DALYTRAN_ORIG_TS,
        ])
        .chain(TRAN_CAT_KEY)
        .chain(REASON_ROWS.map(|(id, _)| id))
        .chain(iter::once(PROC_TS));
    for id in looked_up {
        assert!(P_IDS.contains(&id), "{id} is read but missing from P_IDS");
    }
    let audited: BTreeSet<&str> = [REJECT_SET, POSTING_EFFECTS]
        .into_iter()
        .flat_map(|subsection| {
            let table = subsection_table(doc, subsection);
            table.rows.iter().filter_map(|row| table.cell(row, ID))
        })
        .collect();
    let cited: BTreeSet<&str> = P_IDS
        .iter()
        .copied()
        .filter(|id| {
            doc.find_row(id).is_some_and(|(table, _)| {
                table
                    .section
                    .as_deref()
                    .is_some_and(|section| section.starts_with("D."))
            })
        })
        .collect();
    assert_eq!(
        cited, audited,
        "P_IDS must cite exactly the rows of {REJECT_SET} and {POSTING_EFFECTS}"
    );
}

/// The section A byte ranges the checks use.
struct Layout {
    /// `REJECT-TRAN-DATA X(350)`, the daily record as read.
    data: Range<usize>,
    /// `WS-VALIDATION-FAIL-REASON 9(04)`.
    reason: Range<usize>,
    /// `WS-VALIDATION-FAIL-REASON-DESC X(76)`.
    description: Range<usize>,
    /// `DALYTRAN-ID X(16)`.
    tran_id: Range<usize>,
    /// `DALYTRAN-TYPE-CD X(02)`.
    type_cd: Range<usize>,
    /// `DALYTRAN-CARD-NUM X(16)`.
    card: Range<usize>,
    /// `DALYTRAN-ORIG-TS X(26)`.
    orig_ts: Range<usize>,
    /// `TRANCAT-ACCT-ID 9(11)`, `TRANCAT-TYPE-CD X(02)` and `TRANCAT-CD 9(04)`.
    balance_key: [Range<usize>; 3],
}

impl Layout {
    fn read(doc: &Document) -> Self {
        let layout = Self {
            data: field_range(doc, REJECT_TRAN_DATA),
            reason: field_range(doc, REJECT_REASON),
            description: field_range(doc, REJECT_DESCRIPTION),
            tran_id: field_range(doc, DALYTRAN_ID),
            type_cd: field_range(doc, DALYTRAN_TYPE_CD),
            card: field_range(doc, DALYTRAN_CARD_NUM),
            orig_ts: field_range(doc, DALYTRAN_ORIG_TS),
            balance_key: TRAN_CAT_KEY.map(|id| field_range(doc, id)),
        };
        assert_eq!(
            [
                layout.data.start,
                layout.data.end,
                layout.reason.end,
                layout.description.end
            ],
            [
                0,
                layout.reason.start,
                layout.description.start,
                REJECT_LRECL
            ],
            "{REJECT_TRAN_DATA} to {REJECT_DESCRIPTION} must tile the {REJECT_LRECL}-byte record"
        );
        layout
    }

    /// The reject record `2500-WRITE-REJECT-REC` writes for `data` (P-CBTRN02C-012): the
    /// daily record, the 4-digit reason and the description padded with spaces to its field.
    fn reject_record(&self, data: &[u8], reason: &str, description: &str, id: &str) -> Vec<u8> {
        assert!(
            reason.len() == self.reason.len() && reason.bytes().all(|byte| byte.is_ascii_digit()),
            "audit row {id}: reason={reason} is not {} digits",
            self.reason.len()
        );
        assert!(
            description.len() <= self.description.len(),
            "audit row {id}: {description:?} is longer than its {}-byte field",
            self.description.len()
        );
        let mut record = Vec::with_capacity(REJECT_LRECL);
        record.extend_from_slice(data);
        record.extend_from_slice(reason.as_bytes());
        record.extend_from_slice(description.as_bytes());
        record.resize(REJECT_LRECL, b' ');
        record
    }
}

/// The `key=value` pairs of a section D row's `Input condition` and `Expected result`, which
/// hold distinct keys, with the keys of its subsection's `Keys:` line.
struct Values {
    id: String,
    keys: Vec<String>,
    pairs: Vec<KeyValue>,
}

impl Values {
    fn of(table: &Table, row: &Row) -> Self {
        let id = table.cell(row, ID).unwrap_or_default().to_owned();
        let mut pairs = Vec::new();
        for column in [INPUT_CONDITION, EXPECTED_RESULT] {
            let cell = table
                .cell(row, column)
                .unwrap_or_else(|| panic!("audit row {id} has no {column:?} cell"));
            pairs.extend(carddemo_audit::key_values(cell));
        }
        Self {
            id,
            keys: table.key_names(),
            pairs,
        }
    }

    fn has(&self, key: &str) -> bool {
        self.pairs.iter().any(|pair| pair.key == key)
    }

    /// The value of `key`, backticks kept, which the subsection's `Keys:` line must name.
    fn get(&self, key: &str) -> &str {
        assert!(
            self.keys.iter().any(|named| named == key),
            "audit row {}: {key} is not on the Keys: line of its subsection",
            self.id
        );
        self.pairs.iter().find(|pair| pair.key == key).map_or_else(
            || panic!("audit row {}: no {key}=", self.id),
            |pair| pair.value.as_str(),
        )
    }

    fn number<T: FromStr>(&self, key: &str) -> T
    where
        T::Err: Display,
    {
        parse(self.get(key), &format!("audit row {}: {key}", self.id))
    }

    fn money(&self, key: &str) -> Decimal {
        money(self.get(key), &format!("audit row {}: {key}", self.id))
    }
}

/// One D.1 reject row and the record it expects in `DALYREJS(+1)`.
struct RejectRow {
    id: String,
    /// The 1-based record of [`DAILY_FILE`] it rejects.
    record_no: usize,
    tran_id: String,
    /// The [`REJECT_LRECL`] bytes, without the LF.
    record: Vec<u8>,
}

impl RejectRow {
    /// Reads `values` and checks them against the record they name: its TRAN-ID, card number
    /// and origination date, and the audited rule behind the reason.
    fn read(
        values: &Values,
        daily: &[Vec<u8>],
        layout: &Layout,
        reasons: &BTreeMap<String, &str>,
    ) -> Self {
        let id = values.id.as_str();
        let record_no: usize = values.number("record_no");
        let data = record_no
            .checked_sub(1)
            .and_then(|index| daily.get(index))
            .unwrap_or_else(|| panic!("audit row {id}: {DAILY_FILE} has no record {record_no}"));
        assert_eq!(
            data.len(),
            layout.data.len(),
            "{DAILY_FILE} record {record_no}"
        );
        let tran_id = values.get("tran_id");
        let what = format!("audit row {id} against {DAILY_FILE} record {record_no}");
        assert_eq!(field(data, &layout.tran_id), tran_id, "{what}: TRAN-ID");
        assert_eq!(
            field(data, &layout.card),
            values.get("card"),
            "{what}: card"
        );
        assert_eq!(
            field(data, &layout.orig_ts).get(..10),
            Some(values.get("orig_date")),
            "{what}: DALYTRAN-ORIG-TS(1:10)"
        );
        assert_eq!(
            values.get("bytes"),
            format!(
                "dailytran record {record_no} ({}) + reason + description, {REJECT_LRECL} bytes",
                data.len()
            ),
            "{what}: bytes"
        );
        let reason = values.get("reason");
        let (description, width) = exact_text(values.get("description"), id);
        assert_eq!(
            width,
            Some(layout.description.len()),
            "{what}: description width"
        );
        assert_eq!(
            reasons.get(reason).copied(),
            Some(description.as_str()),
            "{what}: reason {reason} must carry its 1500-VALIDATE-TRAN text"
        );
        assert_reason_rule(values, &description);
        Self {
            id: id.to_owned(),
            record_no,
            tran_id: tran_id.to_owned(),
            record: layout.reject_record(data, reason, &description, id),
        }
    }
}

/// The `1500-B-LOOKUP-ACCT` tests behind a reject's text (P-CBTRN02C-010, P-CBTRN02C-011): a
/// 102 failed the limit check and passed the expiration test, which would otherwise have
/// overwritten it with 103 (E-15); a 103 failed the expiration test, an alphanumeric compare.
fn assert_reason_rule(values: &Values, description: &str) {
    let id = values.id.as_str();
    let overlimit = description == OVERLIMIT_TRANSACTION;
    if overlimit {
        let temp_bal = values.money("ws_temp_bal");
        let limit = values.money("acct_credit_limit");
        assert!(
            temp_bal > limit,
            "audit row {id}: a 102 needs WS-TEMP-BAL {temp_bal} above ACCT-CREDIT-LIMIT {limit}"
        );
    }
    if overlimit || description == TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION {
        let expiration = values.get("acct_expiraion_date");
        let orig_date = values.get("orig_date");
        assert_eq!(
            expiration >= orig_date,
            overlimit,
            "audit row {id}: ACCT-EXPIRAION-DATE {expiration} against DALYTRAN-ORIG-TS(1:10) \
             {orig_date}"
        );
    }
}

/// The end-of-job counts of the D.1 count row (P-CBTRN02C-065).
struct Counts {
    read: usize,
    posted: usize,
    rejected: usize,
}

/// The D.1 rows: the rejects in their listed order, then the counts, checked against the
/// fixture's record count, the reject rows and [`RECORDS_REJECTED`].
fn read_reject_set(doc: &Document, daily: &[Vec<u8>], layout: &Layout) -> (Vec<RejectRow>, Counts) {
    let reasons = reason_texts(doc);
    let mut rejects = Vec::new();
    let mut counts = None;
    for values in subsection_values(doc, REJECT_SET) {
        if values.has("record_no") {
            rejects.push(RejectRow::read(&values, daily, layout, &reasons));
        } else if values.has("read") {
            assert!(
                counts.is_none(),
                "audit row {}: a second count row",
                values.id
            );
            let exit: i32 = values.number("exit");
            assert_eq!(exit, RECORDS_REJECTED, "audit row {}: exit", values.id);
            counts = Some(Counts {
                read: values.number("read"),
                posted: values.number("posted"),
                rejected: values.number("rejected"),
            });
        } else {
            panic!(
                "audit row {}: neither a reject nor the count row",
                values.id
            );
        }
    }
    let counts = counts.unwrap_or_else(|| panic!("{REJECT_SET} has no count row"));
    assert_eq!(
        counts.read,
        daily.len(),
        "the count row reads every {DAILY_FILE} record"
    );
    assert_eq!(
        counts.rejected,
        rejects.len(),
        "the count row counts the reject rows"
    );
    assert_eq!(
        counts.posted + counts.rejected,
        counts.read,
        "posted + rejected = read"
    );
    assert!(
        rejects
            .windows(2)
            .all(|pair| pair[0].record_no < pair[1].record_no),
        "the reject rows are listed in file order"
    );
    (rejects, counts)
}

/// The text of each reject reason, by its 4-digit code: each [`REASON_ROWS`] row's `reason=`
/// and the exact text after it, which must equal its catalog constant.
fn reason_texts(doc: &Document) -> BTreeMap<String, &'static str> {
    let mut reasons = BTreeMap::new();
    for (id, message) in REASON_ROWS {
        let cell = parity_cell(doc, id, EXPECTED_RESULT);
        let reason = cell
            .split_once("reason=")
            .and_then(|(_, rest)| rest.get(..4))
            .filter(|code| code.bytes().all(|byte| byte.is_ascii_digit()))
            .unwrap_or_else(|| panic!("audit row {id}: no 4-digit reason= in {cell:?}"));
        let (text, _) = exact_text(cell, id);
        assert_eq!(text, message, "audit row {id}: the text of reason {reason}");
        assert!(
            reasons.insert(reason.to_owned(), message).is_none(),
            "audit row {id}: reason {reason} is audited twice"
        );
    }
    reasons
}

/// One D.2 account row: `2800-UPDATE-ACCOUNT-REC` after every posted record (P-CBTRN02C-018).
struct AccountRow {
    id: String,
    account_id: u64,
    /// The records posted to the account.
    posted_records: usize,
    /// The [`ACCOUNT_AMOUNTS`], in that order.
    amounts: [Decimal; 3],
}

impl AccountRow {
    fn read(values: &Values) -> Self {
        Self {
            id: values.id.clone(),
            account_id: values.number("account_id"),
            posted_records: values.number("posted_records"),
            amounts: ACCOUNT_AMOUNTS.map(|key| values.money(key)),
        }
    }
}

/// A `TRAN-CAT-KEY`: account, type code and category code.
type BalanceKey = (u64, String, u16);

/// One D.2 category balance row: `2700-UPDATE-TCATBAL` after every posted record, the key
/// read and rewritten (`created=no`, P-CBTRN02C-017) or created on status 23 (`created=yes`,
/// P-CBTRN02C-016).
struct BalanceRow {
    id: String,
    key: BalanceKey,
    balance: Decimal,
}

impl BalanceRow {
    /// Reads `values` and checks `created` against the keys of the seeded [`TCATBAL_FILE`].
    fn read(values: &Values, seeded_keys: &BTreeSet<BalanceKey>) -> Self {
        let id = values.id.as_str();
        let key = (
            values.number("account_id"),
            values.get("tran_type_cd").to_owned(),
            values.number("tran_cat_cd"),
        );
        let created = match values.get("created") {
            "yes" => true,
            "no" => false,
            other => panic!("audit row {id}: created={other} is neither yes nor no"),
        };
        assert_eq!(
            !seeded_keys.contains(&key),
            created,
            "audit row {id}: created={created} against the keys of {TCATBAL_FILE}"
        );
        Self {
            id: id.to_owned(),
            key,
            balance: values.money("tran_cat_bal"),
        }
    }
}

/// The `TRAN-CAT-KEY` of every seeded category balance record.
fn seeded_balance_keys(layout: &Layout) -> BTreeSet<BalanceKey> {
    let [account, type_cd, category] = &layout.balance_key;
    fixture_lines(TCATBAL_FILE)
        .iter()
        .map(|record| {
            (
                parse(field(record, account), "TRANCAT-ACCT-ID"),
                field(record, type_cd).trim_end_matches(' ').to_owned(),
                parse(field(record, category), "TRANCAT-CD"),
            )
        })
        .collect()
}

/// The D.2 posted row: the live transaction master `2900-WRITE-TRANSACTION-FILE` leaves
/// (P-CBTRN02C-019), every record moved by `2000-POST-TRANSACTION` (P-CBTRN02C-013) and
/// stamped by `Z-GET-DB2-FORMAT-TIMESTAMP` (P-CBTRN02C-014).
struct Posted {
    id: String,
    /// The posted TRAN-IDs in key order, the order of the live master.
    tran_ids: Vec<String>,
    /// The daily record of each posted TRAN-ID.
    records: BTreeMap<String, Vec<u8>>,
    /// The `TRAN-PROC-TS` of every posted record under the pinned clock.
    proc_ts: String,
}

impl Posted {
    /// Reads `values`, derives the posted records from the fixture and the reject rows, and
    /// checks the row's count, first and last TRAN-ID and timestamp against them.
    fn read(
        values: &Values,
        daily: &[Vec<u8>],
        rejects: &[RejectRow],
        counts: &Counts,
        layout: &Layout,
        format: &TimestampFormat,
    ) -> Self {
        let id = values.id.as_str();
        assert_eq!(
            values.get("tran_ids"),
            POSTED_DERIVATION,
            "audit row {id}: tran_ids"
        );
        let rejected: BTreeSet<usize> = rejects.iter().map(|reject| reject.record_no).collect();
        let mut file_order = Vec::new();
        let mut records = BTreeMap::new();
        for (index, record) in daily.iter().enumerate() {
            if !rejected.contains(&(index + 1)) {
                let tran_id = field(record, &layout.tran_id).to_owned();
                file_order.push(tran_id.clone());
                assert!(
                    records.insert(tran_id, record.clone()).is_none(),
                    "{DAILY_FILE} record {} repeats a posted TRAN-ID",
                    index + 1
                );
            }
        }
        let posted: usize = values.number("posted");
        assert_eq!(posted, file_order.len(), "audit row {id}: posted");
        assert_eq!(
            posted, counts.posted,
            "audit row {id}: posted against the count row"
        );
        assert_eq!(
            file_order.first().map(String::as_str),
            Some(values.get("first"))
        );
        assert_eq!(
            file_order.last().map(String::as_str),
            Some(values.get("last"))
        );
        let proc_ts = values.get("tran_proc_ts");
        assert_eq!(
            proc_ts, format.pinned,
            "audit row {id}: tran_proc_ts against {PROC_TS}"
        );
        Self {
            id: id.to_owned(),
            tran_ids: records.keys().cloned().collect(),
            records,
            proc_ts: proc_ts.to_owned(),
        }
    }
}

/// The section F row of `TRAN-PROC-TS` (P-CBTRN02C-220).
struct TimestampFormat {
    /// Its `format_regex`.
    pattern: Vec<Position>,
    /// Its `pinned` value under [`TRANREPT_CLOCK`].
    pinned: String,
}

impl TimestampFormat {
    fn read(doc: &Document) -> Self {
        let regex = unquote(&expected_span(doc, PROC_TS, "format_regex").text, PROC_TS);
        let pinned = unquote(&expected_span(doc, PROC_TS, "pinned").text, PROC_TS);
        let pattern = fixed_pattern(&regex, PROC_TS);
        assert!(
            matches(&pattern, &pinned),
            "audit row {PROC_TS}: pinned={pinned:?} does not match format_regex={regex:?}"
        );
        Self { pattern, pinned }
    }
}

/// One character position of a fixed-length pattern.
#[derive(Clone, Copy)]
enum Position {
    Digit,
    Char(char),
}

/// The positions of `regex`, a section F `format_regex`. The timestamp formats use only `^…$`
/// anchors, the class `[0-9]`, `\` escapes, literal characters and `{n}` repeats of the
/// preceding position, so a regex that needs more fails here, naming the row, rather than
/// being read wrongly.
fn fixed_pattern(regex: &str, id: &str) -> Vec<Position> {
    let body = regex
        .strip_prefix('^')
        .and_then(|rest| rest.strip_suffix('$'))
        .unwrap_or_else(|| panic!("audit row {id}: {regex:?} is not anchored with ^ and $"));
    let mut positions = Vec::new();
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '[' => {
                let class: String = chars.by_ref().take_while(|&next| next != ']').collect();
                assert_eq!(class, "0-9", "audit row {id}: class [{class}] in {regex:?}");
                positions.push(Position::Digit);
            }
            '{' => {
                let count: String = chars.by_ref().take_while(|&next| next != '}').collect();
                let count: usize = parse(&count, &format!("audit row {id}: a repeat in {regex:?}"));
                assert!(count > 0, "audit row {id}: a repeat of 0 in {regex:?}");
                let last = positions
                    .last()
                    .copied()
                    .unwrap_or_else(|| panic!("audit row {id}: {regex:?} repeats nothing"));
                positions.extend(iter::repeat_n(last, count - 1));
            }
            '\\' => {
                let escaped = chars.next();
                positions.push(Position::Char(escaped.unwrap_or_else(|| {
                    panic!("audit row {id}: {regex:?} ends with a lone backslash")
                })));
            }
            '.' | '*' | '+' | '?' | '(' | ')' | '|' | '^' | '$' | ']' | '}' => {
                panic!("audit row {id}: {regex:?} uses {ch:?}, which a fixed pattern cannot hold")
            }
            literal => positions.push(Position::Char(literal)),
        }
    }
    positions
}

/// Whether `text` has exactly the positions of `pattern`.
fn matches(pattern: &[Position], text: &str) -> bool {
    text.chars().count() == pattern.len()
        && text
            .chars()
            .zip(pattern)
            .all(|(ch, position)| match position {
                Position::Digit => ch.is_ascii_digit(),
                Position::Char(expected) => ch == *expected,
            })
}

/// `DALYREJS(+1)` equals the D.1 reject records, in order, each followed by LF
/// (P-CBTRN02C-012). A difference is reported as the first differing record and both of its
/// versions.
fn assert_reject_file(bytes: &[u8], expected: &Expectations) {
    let frame = REJECT_LRECL + 1;
    let written: Vec<&[u8]> = bytes.chunks(frame).collect();
    let audited: Vec<Vec<u8>> = expected
        .rejects
        .iter()
        .map(|reject| [reject.record.as_slice(), b"\n"].concat())
        .collect();
    let first_difference = (0..written.len().max(audited.len()))
        .find(|&index| written.get(index).copied() != audited.get(index).map(Vec::as_slice));
    if let Some(index) = first_difference {
        let row = expected.rejects.get(index).map_or_else(
            || "no audit row".to_owned(),
            |reject| format!("{}, {DAILY_FILE} record {}", reject.id, reject.record_no),
        );
        panic!(
            "{REJECTS} record {} differs from the audit ({row}); {} records written, {} audited\n\
             audited: {}\nwritten: {}",
            index + 1,
            written.len(),
            audited.len(),
            shown(audited.get(index).map(Vec::as_slice)),
            shown(written.get(index).copied()),
        );
    }
    let records = split_records(bytes, REJECT_LRECL);
    assert_eq!(
        records.len(),
        expected.counts.rejected,
        "{REJECTS} holds the audited count"
    );
    let written_ids: BTreeSet<&str> = records
        .iter()
        .map(|record| field(record.as_bytes(), &expected.layout.tran_id))
        .collect();
    let audited_ids: BTreeSet<&str> = expected
        .rejects
        .iter()
        .map(|reject| reject.tran_id.as_str())
        .collect();
    assert_eq!(written_ids, audited_ids, "the TRAN-IDs of {REJECTS}");
}

/// Every account holds its D.2 balance and cycle totals, and the live master holds as many
/// records of the account as the audit posted to it.
fn assert_accounts(h: &Harness, expected: &[AccountRow]) {
    let stored: BTreeMap<u64, [Decimal; 3]> = text_rows(h, ACCOUNTS_SQL)
        .iter()
        .map(|row| {
            let amounts = array::from_fn(|index| money(&row[index + 1], ACCOUNT_AMOUNTS[index]));
            (parse(&row[0], "accounts.account_id"), amounts)
        })
        .collect();
    let posted: BTreeMap<u64, usize> = text_rows(h, POSTED_PER_ACCOUNT_SQL)
        .iter()
        .map(|row| (parse(&row[0], "xref_acct_id"), parse(&row[1], "count")))
        .collect();
    let audited: BTreeSet<u64> = expected.iter().map(|row| row.account_id).collect();
    assert_eq!(
        stored.keys().copied().collect::<BTreeSet<u64>>(),
        audited,
        "the account master holds exactly the audited accounts"
    );
    for row in expected {
        assert_eq!(
            stored.get(&row.account_id),
            Some(&row.amounts),
            "{}: account {} {ACCOUNT_AMOUNTS:?}",
            row.id,
            row.account_id
        );
        assert_eq!(
            posted.get(&row.account_id).copied().unwrap_or_default(),
            row.posted_records,
            "{}: records posted to account {}",
            row.id,
            row.account_id
        );
    }
}

/// The category balance file holds exactly the D.2 keys, each with its audited balance.
fn assert_balances(h: &Harness, expected: &[BalanceRow]) {
    let stored: BTreeMap<BalanceKey, Decimal> = text_rows(h, BALANCES_SQL)
        .iter()
        .map(|row| {
            let key = (
                parse(&row[0], "trancat_acct_id"),
                row[1].clone(),
                parse(&row[2], "trancat_cd"),
            );
            (key, money(&row[3], "tran_cat_bal"))
        })
        .collect();
    let audited: BTreeSet<&BalanceKey> = expected.iter().map(|row| &row.key).collect();
    assert_eq!(
        stored.keys().collect::<BTreeSet<_>>(),
        audited,
        "the category balance file holds exactly the audited keys"
    );
    for row in expected {
        assert_eq!(
            stored.get(&row.key),
            Some(&row.balance),
            "{}: tran_cat_bal of {:?}",
            row.id,
            row.key
        );
    }
}

/// The live master, read through `transaction_master` only (E-09), holds the posted
/// TRAN-IDs in key order, each with the daily record's type, card and origination timestamp
/// and the audited `TRAN-PROC-TS`.
fn assert_live_master(h: &Harness, expected: &Expectations) {
    let posted = &expected.posted;
    let layout = &expected.layout;
    let rows = h.live_master_rows();
    let tran_ids: Vec<&str> = rows.iter().map(|row| cell(row, "tran_id")).collect();
    assert_eq!(
        tran_ids, posted.tran_ids,
        "{}: the posted TRAN-IDs",
        posted.id
    );
    assert_eq!(
        rows.len(),
        expected.counts.read - expected.counts.rejected,
        "the live master holds every record read less the rejects"
    );
    for row in &rows {
        let tran_id = cell(row, "tran_id");
        let record = &posted.records[tran_id];
        for (column, range) in [
            ("tran_type_cd", &layout.type_cd),
            ("tran_card_num", &layout.card),
            ("tran_orig_ts", &layout.orig_ts),
        ] {
            assert_eq!(
                cell(row, column),
                field(record, range).trim_end_matches(' '),
                "TRAN-ID {tran_id}: {column} moved from the daily record"
            );
        }
        let proc_ts = cell(row, "tran_proc_ts");
        assert!(
            matches(&expected.format.pattern, proc_ts),
            "{PROC_TS}: TRAN-PROC-TS {proc_ts:?} of {tran_id} does not have the audited format"
        );
        assert_eq!(
            proc_ts, posted.proc_ts,
            "{}: TRAN-PROC-TS of {tran_id}",
            posted.id
        );
    }
}

/// The table of the `###` `subsection` of section D.
fn subsection_table<'d>(doc: &'d Document, subsection: &str) -> &'d Table {
    let mut tables = doc
        .section_tables("D.")
        .filter(|table| table.is_data_table() && table.subsection.as_deref() == Some(subsection));
    let table = tables
        .next()
        .unwrap_or_else(|| panic!("audit section D has no table under ### {subsection}"));
    assert!(
        tables.next().is_none(),
        "### {subsection} holds more than one table"
    );
    table
}

/// The [`Values`] of every row of `subsection`, in their listed order.
fn subsection_values(doc: &Document, subsection: &str) -> Vec<Values> {
    let table = subsection_table(doc, subsection);
    table
        .rows
        .iter()
        .map(|row| Values::of(table, row))
        .collect()
}

/// The bytes the section A row `id` places a field at: its `Offset` and `Length`.
fn field_range(doc: &Document, id: &str) -> Range<usize> {
    let offset: usize = parse(
        parity_cell(doc, id, OFFSET),
        &format!("audit row {id}: Offset"),
    );
    let length: usize = parse(
        parity_cell(doc, id, LENGTH),
        &format!("audit row {id}: Length"),
    );
    offset..offset + length
}

/// The text of the field at `range` of `record`.
fn field<'r>(record: &'r [u8], range: &Range<usize>) -> &'r str {
    record
        .get(range.clone())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .unwrap_or_else(|| {
            panic!(
                "bytes {range:?} of a {}-byte record are no text",
                record.len()
            )
        })
}

/// The exact text of the first code span of `value` and its `width=<n>`, if any.
fn exact_text(value: &str, id: &str) -> (String, Option<usize>) {
    let span = carddemo_audit::code_span(value)
        .unwrap_or_else(|| panic!("audit row {id}: {value:?} holds no exact text"));
    (unquote(&span.text, id), span.width)
}

/// The value of the double-quoted string `content`, whose only escapes are `\"` and `\\`.
fn unquote(content: &str, id: &str) -> String {
    let inner = content
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("audit row {id}: {content:?} is not a double-quoted text"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                _ => panic!("audit row {id}: {content:?} holds a backslash that escapes nothing"),
            },
            '"' => panic!("audit row {id}: {content:?} holds an unescaped quote"),
            _ => text.push(ch),
        }
    }
    text
}

/// `text` as an exact decimal with the 2 places of every amount the checks compare.
fn money(text: &str, what: &str) -> Decimal {
    let value = Decimal::from_str_exact(text)
        .unwrap_or_else(|error| panic!("{what}: {text:?} is not a decimal: {error}"));
    assert_eq!(
        value.scale(),
        2,
        "{what}: {text:?} must have 2 decimal places"
    );
    value
}

fn parse<T: FromStr>(text: &str, what: &str) -> T
where
    T::Err: Display,
{
    text.parse()
        .unwrap_or_else(|error| panic!("{what}: {text:?} does not parse: {error}"))
}

/// The rows of `sql`, every column as text; the columns read are all `NOT NULL`.
fn text_rows(h: &Harness, sql: &str) -> Vec<Vec<String>> {
    block_on(h.stack().fetch_text_rows(sql))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| value.unwrap_or_else(|| panic!("a NULL column in {sql}")))
                .collect()
        })
        .collect()
}

/// The `column` of a [`Harness::live_master_rows`] row.
fn cell<'r>(row: &'r [Option<String>], column: &str) -> &'r str {
    LIVE_MASTER_COLUMNS
        .iter()
        .position(|name| *name == column)
        .and_then(|index| row.get(index)?.as_deref())
        .unwrap_or_else(|| panic!("the live master row has no {column} value"))
}

/// A reject frame for a failure message, its spaces visible.
fn shown(frame: Option<&[u8]>) -> String {
    frame.map_or_else(
        || "<none>".to_owned(),
        |bytes| format!("{:?}", String::from_utf8_lossy(bytes)),
    )
}
