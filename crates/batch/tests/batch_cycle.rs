//! The README batch sequence [README.md:L209-233]: every job with business logic run as its
//! `batch` subcommand, in README order, against one seeded database, and checked after each
//! step against the fixture expectations of `docs/parity/parity-oracle-audit.md` section D.
//!
//! | README job | Subcommand | Exit |
//! |---|---|---|
//! | ACCTFILE, CARDFILE, XREFFILE, CUSTFILE, TRANCATG, TRANTYPE, DISCGRP, TCATBALF, DUSRSECJ | `seed`, the refresh | 0 |
//! | TRANBKP [README.md:L218] | `tranbkp` | 12: the live master is empty |
//! | POSTTRAN [README.md:L225] | `posttran` | 4 |
//! | INTCALC [README.md:L226] | `intcalc --parm-date 2022071800` | 0 |
//! | TRANBKP [README.md:L227] | `tranbkp` | 0 |
//! | COMBTRAN [README.md:L228] | `combtran` | 0 |
//! | CREASTMT [README.md:L229] | `creastmt` | 0 |
//! | CBPAUP0J [README.md:L233] | `cbpaup0j`, the JCL `SYSIN` by default | 0 |
//!
//! CLOSEFIL, TRANIDX, OPENFIL and WAITSTEP have no equivalent, and the optional Db2 job
//! TRANEXTR is not run. The README runs the first TRANBKP between the CUSTFILE and TRANCATG
//! loads. The seed reloads every master file in one database transaction and leaves the
//! transaction master alone, the only file TRANBKP reads, so a TRANBKP after the seed meets the
//! state of the README order.
//!
//! Both TRANBKP positions hold (E-09). The sequence starts from the empty transaction master the
//! seed leaves (E-07), so the first TRANBKP catalogs an empty backup, and POSTTRAN meets the
//! seeded state section D starts from. The second TRANBKP backs up the posted records and
//! empties the master, COMBTRAN reloads it from that backup and the interest transactions, and
//! the reward accruals POSTTRAN made stay linked to their transactions throughout.
//!
//! Every run pins the processing clock to `TRANREPT_CLOCK`, the instant section D is derived
//! under. Bytes the COBOL never sets (`{UNDEF:n}`, E-25) are left out of every byte comparison,
//! and the INTCALC timestamps, which section F fixes by format only (P-CBACT04C-176), are checked
//! by that format.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::iter;
use std::process::Output;

use carddemo_audit::{CodeSpan, Document, KeyValue, PARITY_COMMON_COLUMNS, Row, Table};
use carddemo_codec::layouts::transaction::{
    FILLER, TRAN_AMT, TRAN_CARD_NUM, TRAN_CAT_CD, TRAN_DESC, TRAN_ID, TRAN_MERCHANT_CITY,
    TRAN_MERCHANT_ID, TRAN_MERCHANT_NAME, TRAN_MERCHANT_ZIP, TRAN_ORIG_TS, TRAN_PROC_TS,
    TRAN_SOURCE, TRAN_TYPE_CD,
};
use carddemo_codec::{FieldSpec, Layout, TransactionLayout, TransactionRecord, encode_ascii_lf};
use common::{
    Harness, LIVE_MASTER_COLUMNS, TRANREPT_CLOCK, block_on, expected_span, expected_value,
    expected_values, fixture_lines, parity, parity_cell, split_records,
};

/// TRANBKP `STEP05R` over a live master that holds records: every record backed up in TRAN-ID
/// order, condition code 0.
const BACKUP_FULL: &str = "P-TRANBKP-001";

/// TRANBKP `STEP05R` over an empty live master: the backup is cataloged empty and `REPRO` ends
/// with condition code 12.
const BACKUP_EMPTY: &str = "P-TRANBKP-002";

/// The disposition of a TRANBKP over an empty live master: exit 12 and an empty backup (E-37).
const BACKUP_EMPTY_DISPOSITION: &str = "P-TRANBKP-006";

/// POSTTRAN over the fixture day from the seeded state: records read, posted and rejected, and
/// the exit code.
const POSTTRAN_TOTALS: &str = "P-CBTRN02C-065";

/// The records POSTTRAN writes to the live master: their count, first and last TRAN-ID and
/// processing timestamp.
const POSTTRAN_POSTED: &str = "P-CBTRN02C-216";

/// The `TRAN-PROC-TS` POSTTRAN stamps under the pinned clock.
const POSTTRAN_PROC_TS: &str = "P-CBTRN02C-220";

/// The INTCALC `PARM` of the JCL.
const INTCALC_PARM_ROW: &str = "P-CBACT04C-001";

/// The last account of the category balance file, which INTCALC never rewrites (E-14).
const INTCALC_LAST_ACCOUNT: &str = "P-CBACT04C-172";

/// The SYSTRAN interest record: its template, record count and first and last TRAN-ID.
const INTCALC_SYSTRAN: &str = "P-CBACT04C-173";

/// The format of the SYSTRAN `TRAN-ORIG-TS` and `TRAN-PROC-TS`, which hold one value.
const INTCALC_TIMESTAMPS: &str = "P-CBACT04C-176";

/// The COMBTRAN sort output: the backup and the interest records in TRAN-ID order.
const COMBINED_FILE: &str = "P-COMBTRAN-004";

/// The COMBTRAN reload of a live master that TRANBKP emptied: every record loaded, exit 0, and
/// the live master equal to the combined file.
const COMBINED_RELOAD: &str = "P-COMBTRAN-005";

/// The CBSTM03A card and transaction tables over the combined live master.
const STATEMENT_CAPACITY: &str = "P-CBSTM03A-014";

/// The two statement files: names, record counts, widths and records per card.
const STATEMENT_FILES: &str = "P-CBSTM03A-015";

/// The normal end of CBSTM03A, condition code 0.
const STATEMENT_END: &str = "P-CBSTM03A-1264";

/// The CBPAUP0J `SYSIN` card.
const PURGE_SYSIN: &str = "P-CBPAUP0C-001";

/// The summary read loop of CBPAUP0C, which ends at its first `GN` over an empty database.
const PURGE_SUMMARIES: &str = "P-CBPAUP0C-002";

/// The section D rows, and the section B rows with no fixture row of their own, that this file
/// checks one by one. Whole D tables are read too: the reject set (D.1), the posting effects
/// (D.2), the interest records and account updates (D.3) and the statement lines (D.5), whose
/// rows `posttran_rejects.rs`, `intcalc.rs` and `creastmt.rs` cite.
const P_IDS: &[&str] = &[
    BACKUP_FULL,
    BACKUP_EMPTY,
    BACKUP_EMPTY_DISPOSITION,
    POSTTRAN_TOTALS,
    POSTTRAN_POSTED,
    POSTTRAN_PROC_TS,
    INTCALC_PARM_ROW,
    INTCALC_LAST_ACCOUNT,
    INTCALC_SYSTRAN,
    INTCALC_TIMESTAMPS,
    COMBINED_FILE,
    COMBINED_RELOAD,
    STATEMENT_CAPACITY,
    STATEMENT_FILES,
    STATEMENT_END,
    PURGE_SYSIN,
    PURGE_SUMMARIES,
];

/// The D.7 seed counts, each with the table it counts, in the column order of
/// [`SEED_COUNTS_SQL`]. The row of `transactions` (P-CVTRA05Y-015, no rows) is checked as the
/// empty view `transaction_master`, the only way legacy reads reach that table.
const SEED_COUNTS: [(&str, &str); 11] = [
    ("P-CVACT01Y-014", "accounts"),
    ("P-CVACT02Y-008", "cards"),
    ("P-CVACT03Y-005", "card_xref"),
    ("P-CVCUS01Y-020", "customers"),
    ("P-CVTRA02Y-006", "disclosure_groups"),
    ("P-CVTRA01Y-006", "transaction_category_balances"),
    ("P-CVTRA03Y-004", "trantype_file"),
    ("P-CVTRA04Y-005", "trancatg_file"),
    ("P-TRNTYPE-003", "transaction_types"),
    ("P-TRNTYCAT-004", "transaction_categories"),
    ("P-CSUSR01Y-007", "users"),
];

/// The row count of every [`SEED_COUNTS`] table, in that order, as one row.
const SEED_COUNTS_SQL: &str = "SELECT \
     (SELECT count(*) FROM accounts), \
     (SELECT count(*) FROM cards), \
     (SELECT count(*) FROM card_xref), \
     (SELECT count(*) FROM customers), \
     (SELECT count(*) FROM disclosure_groups), \
     (SELECT count(*) FROM transaction_category_balances), \
     (SELECT count(*) FROM trantype_file), \
     (SELECT count(*) FROM trancatg_file), \
     (SELECT count(*) FROM transaction_types), \
     (SELECT count(*) FROM transaction_categories), \
     (SELECT count(*) FROM users)";

/// The distinct category balances the seed loads: `0.00` in every row (P-CVTRA01Y-006).
const SEEDED_TCATBALS_SQL: &str =
    "SELECT DISTINCT tran_cat_bal::text FROM transaction_category_balances";

/// Every account as its `ACCT-ID 9(11)`, `ACCT-CURR-BAL`, `ACCT-CURR-CYC-CREDIT` and
/// `ACCT-CURR-CYC-DEBIT`, in key order.
const ACCOUNTS_SQL: &str = "SELECT lpad(account_id::text, 11, '0'), acct_curr_bal, \
     acct_curr_cyc_credit, acct_curr_cyc_debit FROM accounts ORDER BY account_id";

/// Every category balance as its key, `TRANCAT-ACCT-ID 9(11)`, `TRANCAT-TYPE-CD X(02)` and
/// `TRANCAT-CD 9(04)`, and its `TRAN-CAT-BAL`, in key order.
const TCATBALS_SQL: &str = "SELECT lpad(trancat_acct_id::text, 11, '0'), trancat_type_cd, \
     lpad(trancat_cd::text, 4, '0'), tran_cat_bal FROM transaction_category_balances \
     ORDER BY trancat_acct_id, trancat_type_cd, trancat_cd";

/// Every reward accrual, its transaction row, and that row's TRAN-ID while the row is in the
/// live master, `NULL` while it is not, in accrual order.
const ACCRUAL_LINKS_SQL: &str = "SELECT a.accrual_id, a.transaction_id, m.tran_id \
     FROM reward_accruals a \
     LEFT JOIN transaction_master m ON m.transaction_id = a.transaction_id \
     ORDER BY a.accrual_id";

/// The pending authorization summaries and details, as one row of two counts.
const PENDING_AUTHORIZATIONS_SQL: &str = "SELECT \
     (SELECT count(*) FROM pending_auth_summaries), \
     (SELECT count(*) FROM pending_auth_details)";

/// `batch seed` without options: the refresh that stands for the README load jobs ACCTFILE to
/// DUSRSECJ, in one database transaction.
const SEED: [&str; 1] = ["seed"];

/// `batch tranbkp`, both README positions.
const TRANBKP: [&str; 1] = ["tranbkp"];

/// `batch posttran` over its JCL input, the daily transactions of the fixture corpus.
const POSTTRAN: [&str; 1] = ["posttran"];

/// The `PARM` of the INTCALC JCL [app/jcl/INTCALC.jcl:L22].
const INTCALC_PARM: &str = "2022071800";

/// `batch intcalc` with the JCL `PARM`.
const INTCALC: [&str; 3] = ["intcalc", "--parm-date", INTCALC_PARM];

/// `batch combtran`, over `TRANSACT.BKUP(0)` and `SYSTRAN(0)`.
const COMBTRAN: [&str; 1] = ["combtran"];

/// `batch creastmt`, over the live master COMBTRAN reloaded.
const CREASTMT: [&str; 1] = ["creastmt"];

/// `batch cbpaup0j` with its default `--sysin`, the card of the CBPAUP0J JCL.
const CBPAUP0J: [&str; 1] = ["cbpaup0j"];

/// The condition code of a step that ends normally: the seed, INTCALC (P-CBACT04C-187),
/// TRANBKP over records (P-TRANBKP-001), CREASTMT (P-CBSTM03A-1264) and CBPAUP0J, whose
/// `GOBACK` sets no return code over an empty database (P-CBPAUP0C-002).
const NORMAL_END: i32 = 0;

/// The REPRO condition code of a TRANBKP over an empty live master (P-TRANBKP-002,
/// P-TRANBKP-006, E-37).
///
/// The first TRANBKP of the sequence [README.md:L218] meets that case: the seed loads no
/// transaction (E-07), so its `REPRO` reads nothing. The audited code is therefore 12, not the 0
/// of a backup of records; a 0 there would need a record in the master, which no earlier job of
/// the sequence writes.
const EMPTY_BACKUP_EXIT: i32 = 12;

/// The POSTTRAN rejects, `DALYREJS(+1)`.
const DALYREJS: &str = "DALYREJS";

/// The INTCALC interest transactions, `SYSTRAN(+1)`.
const SYSTRAN: &str = "SYSTRAN";

/// The TRANBKP backup, `TRANSACT.BKUP(+1)`.
const BKUP: &str = "TRANSACT.BKUP";

/// The COMBTRAN sort output, `TRANSACT.COMBINED(+1)`.
const COMBINED: &str = "TRANSACT.COMBINED";

/// The POSTTRAN input, `DALYTRAN DD` [app/jcl/POSTTRAN.jcl:L30-31], as its fixture copy.
const DAILYTRAN: &str = "dailytran.txt";

/// `REJECT-RECORD X(430)`: the 350 input bytes, the 4-digit reason and the 76-byte description
/// (P-CBTRN02C-012) [app/cbl/CBTRN02C.cbl:L176-182].
const REJECT_RECORD_LEN: usize = 430;

/// `CVTRA05Y` `TRAN-RECORD`, 350 bytes: the record of every `TRANSACT` dataset and of `SYSTRAN`.
const TRAN_RECORD_LEN: usize = <TransactionLayout as Layout>::LEN;

/// More live transactions than the README sequence ever holds; a page this full fails the test
/// rather than leave records unread.
const LIVE_MASTER_READ_LIMIT: u16 = 1000;

/// The `Expected result` column of every parity audit data table.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// The trailing overpunch of a positive signed `DISPLAY` field, last digit 0 to 9 (section A:
/// `{ABCDEFGHI` for +0 to +9).
const POSITIVE_OVERPUNCH: &[u8; 10] = b"{ABCDEFGHI";

/// The trailing overpunch of a negative signed `DISPLAY` field (section A: `}JKLMNOPQR` for -0
/// to -9).
const NEGATIVE_OVERPUNCH: &[u8; 10] = b"}JKLMNOPQR";

/// The `format_regex` of P-CBACT04C-176, exactly as the audit writes it.
const DB2_TIMESTAMP_REGEX: &str =
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}-[0-9]{2}\.[0-9]{2}\.[0-9]{2}\.[0-9]{2}0000$";

/// The shape [`DB2_TIMESTAMP_REGEX`] accepts, byte by byte: `9` stands for any ASCII digit, every
/// other byte for itself.
const DB2_TIMESTAMP_SHAPE: &[u8; 26] = b"9999-99-99-99.99.99.990000";

#[test]
fn readme_batch_sequence_holds_at_both_tranbkp_positions() {
    let doc = parity();
    for id in P_IDS.iter().chain(SEED_COUNTS.iter().map(|(id, _)| id)) {
        assert!(
            doc.find_row(id).is_some(),
            "the parity audit has no row {id}"
        );
    }
    let h = Harness::new();

    seed(&h, &doc);
    back_up_empty_master(&h);
    let posted = post_daily_transactions(&h, &doc);
    compute_interest(&h, &doc);
    back_up_posted_master(&h, &doc, &posted);
    combine_transactions(&h, &doc, &posted);
    create_statements(&h, &doc);
    purge_expired_authorizations(&h, &doc);

    assert_eq!(
        generation_names(&h, BKUP),
        [generation(BKUP, 1), generation(BKUP, 2)],
        "the two TRANBKP runs cataloged one backup each, oldest first"
    );
}

/// What POSTTRAN left for the later steps: the daily records it posted, in TRAN-ID order, and
/// the reward accruals it made.
struct Posted {
    records: Vec<Vec<u8>>,
    accruals: Vec<AccrualLink>,
}

/// One reward accrual and the transaction row it references.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AccrualLink {
    accrual_id: String,
    transaction_id: String,
    /// The TRAN-ID of the row while it is in the live master, `None` while it is not.
    live_tran: Option<String>,
}

/// CLOSEFIL to DUSRSECJ: `batch seed`, the refresh of a database that already holds the seeded
/// state, reloads the master files and processing copies with the D.7 counts and leaves the
/// transaction master empty (E-07).
fn seed(h: &Harness, doc: &Document) {
    run(h, &SEED, NORMAL_END);
    let rows = text_rows(h, SEED_COUNTS_SQL);
    let [counts] = rows.as_slice() else {
        panic!("{SEED_COUNTS_SQL} gave {} rows", rows.len());
    };
    assert_eq!(
        counts.len(),
        SEED_COUNTS.len(),
        "one count per seeded table"
    );
    for ((id, table), got) in SEED_COUNTS.iter().zip(counts) {
        assert_eq!(
            expected_value(doc, id, "table"),
            *table,
            "{id} counts another table"
        );
        assert_eq!(
            *got,
            expected_value(doc, id, "rows"),
            "{id}: rows of {table} after batch seed"
        );
    }
    assert_eq!(
        text_rows(h, SEEDED_TCATBALS_SQL),
        [["0.00"]],
        "P-CVTRA01Y-006: every seeded category balance is zero"
    );
    assert!(
        h.live_master_rows().is_empty(),
        "the seed leaves the transaction master empty (E-07)"
    );
}

/// TRANBKP at its first README position [README.md:L218]: the live master is empty, so the
/// backup `TRANSACT.BKUP.G0001V00` is cataloged empty and `REPRO` ends with condition code 12
/// (P-TRANBKP-002, P-TRANBKP-006, E-37). The master stays empty.
fn back_up_empty_master(h: &Harness) {
    run(h, &TRANBKP, EMPTY_BACKUP_EXIT);
    assert_eq!(generation_names(h, BKUP), [generation(BKUP, 1)]);
    assert!(
        h.newest(BKUP).is_empty(),
        "{BACKUP_EMPTY}: the backup of an empty live master holds no byte"
    );
    assert!(
        h.live_master_rows().is_empty(),
        "the live master stays empty after a TRANBKP of nothing"
    );
}

/// POSTTRAN [README.md:L225] over the 300 daily records, from the state the first TRANBKP
/// left, which is the seeded state section D starts from. The rejects equal D.1, the account
/// and category balances D.2, and the live master holds the posted records (P-CBTRN02C-216),
/// each with a reward accrual link when it earned points.
fn post_daily_transactions(h: &Harness, doc: &Document) -> Posted {
    let daily = fixture_lines(DAILYTRAN);
    assert_eq!(
        daily.len().to_string(),
        expected_value(doc, POSTTRAN_TOTALS, "read"),
        "{POSTTRAN_TOTALS}: records of {DAILYTRAN}"
    );
    run(h, &POSTTRAN, exit_of(doc, POSTTRAN_TOTALS));

    let rejected = assert_rejects(h, doc, &daily);
    let d2 = audit_rows(d_table(doc, "D.2"));
    assert_accounts(h, &d2, "POSTTRAN");
    assert_tcatbals(h, &d2, "POSTTRAN");
    let records = assert_posted(h, doc, &daily, &rejected);

    let accruals = accrual_links(h);
    assert!(
        !accruals.is_empty(),
        "POSTTRAN accrues reward points under the seeded retail rule"
    );
    let posted_ids: BTreeSet<&str> = records.iter().map(|record| tran_id_of(record)).collect();
    for link in &accruals {
        let tran = link.live_tran.as_deref().unwrap_or_else(|| {
            panic!("accrual {} references no live transaction", link.accrual_id)
        });
        assert!(
            posted_ids.contains(tran),
            "accrual {} references {tran}, which POSTTRAN did not post",
            link.accrual_id
        );
    }
    Posted { records, accruals }
}

/// The `DALYREJS(+1)` POSTTRAN cataloged: one generation holding the D.1 rejects in input order,
/// each the daily record, its reason and its description, byte for byte. Returns the rejected
/// TRAN-IDs.
fn assert_rejects(h: &Harness, doc: &Document, daily: &[Vec<u8>]) -> BTreeSet<String> {
    assert_eq!(generation_names(h, DALYREJS), [generation(DALYREJS, 1)]);
    let records = split_records(&h.newest(DALYREJS), REJECT_RECORD_LEN);
    let expected: Vec<AuditRow<'_>> = audit_rows(d_table(doc, "D.1"))
        .into_iter()
        .filter(|row| row.get("record_no").is_some())
        .collect();
    assert_eq!(
        records.len(),
        expected.len(),
        "DALYREJS holds one record per D.1 reject row"
    );
    assert_eq!(
        records.len().to_string(),
        expected_value(doc, POSTTRAN_TOTALS, "rejected"),
        "{POSTTRAN_TOTALS}: rejected records"
    );
    let mut rejected = BTreeSet::new();
    for (record, row) in records.iter().zip(&expected) {
        let number = count(row.value("record_no"), row.id);
        let input = number
            .checked_sub(1)
            .and_then(|index| daily.get(index))
            .unwrap_or_else(|| panic!("{}: {DAILYTRAN} has no record {number}", row.id));
        let mut want = known(input);
        want.extend(known(row.value("reason").as_bytes()));
        want.extend(exact_text(&row.span("description"), row.id));
        assert_record(
            &want,
            record,
            &format!("{}: DALYREJS record of daily record {number}", row.id),
        );
        assert_eq!(field(record, &TRAN_ID), row.value("tran_id"), "{}", row.id);
        rejected.insert(row.value("tran_id").to_owned());
    }
    rejected
}

/// The live master after POSTTRAN: exactly the daily records not rejected, in TRAN-ID order,
/// each stamped with the pinned `TRAN-PROC-TS` (P-CBTRN02C-216, P-CBTRN02C-220). Returns those
/// daily records, in that order.
fn assert_posted(
    h: &Harness,
    doc: &Document,
    daily: &[Vec<u8>],
    rejected: &BTreeSet<String>,
) -> Vec<Vec<u8>> {
    let mut posted: Vec<Vec<u8>> = daily
        .iter()
        .filter(|record| !rejected.contains(tran_id_of(record)))
        .cloned()
        .collect();
    posted.sort_by(|left, right| tran_id_of(left).cmp(tran_id_of(right)));
    let want: Vec<&str> = posted.iter().map(|record| tran_id_of(record)).collect();

    let rows = h.live_master_rows();
    let tran_ids = live_column(&rows, "tran_id");
    assert_eq!(
        tran_ids, want,
        "{POSTTRAN_POSTED}: the live master holds every daily record not in D.1"
    );
    let posted_count = tran_ids.len().to_string();
    assert_eq!(posted_count, expected_value(doc, POSTTRAN_POSTED, "posted"));
    assert_eq!(posted_count, expected_value(doc, POSTTRAN_TOTALS, "posted"));
    assert_eq!(
        tran_ids.first().copied(),
        Some(expected_value(doc, POSTTRAN_POSTED, "first").as_str())
    );
    assert_eq!(
        tran_ids.last().copied(),
        Some(expected_value(doc, POSTTRAN_POSTED, "last").as_str())
    );
    let pinned = expected_value(doc, POSTTRAN_POSTED, "tran_proc_ts");
    assert_eq!(
        pinned,
        unquoted(&expected_span(doc, POSTTRAN_PROC_TS, "pinned")),
        "{POSTTRAN_POSTED} and {POSTTRAN_PROC_TS} pin one processing timestamp"
    );
    for (tran_id, proc_ts) in tran_ids.iter().zip(live_column(&rows, "tran_proc_ts")) {
        assert_eq!(
            proc_ts, pinned,
            "{POSTTRAN_PROC_TS}: TRAN-PROC-TS of {tran_id}"
        );
    }
    posted
}

/// INTCALC [README.md:L226] with the JCL `PARM` over the category balances POSTTRAN left: one
/// `SYSTRAN(+1)` generation holding the D.3 interest records, and every account rewritten with
/// its D.3 balance and zero cycle amounts except the last, which keeps its POSTTRAN values
/// (E-14). The category balances, which CBACT04C opens for input only (P-CBACT04C-001), stay
/// as POSTTRAN left them.
fn compute_interest(h: &Harness, doc: &Document) {
    assert_eq!(
        unquoted(&expected_span(doc, INTCALC_PARM_ROW, "parm")),
        INTCALC_PARM,
        "{INTCALC_PARM_ROW}: the JCL PARM"
    );
    assert_eq!(
        unquoted(&expected_span(doc, INTCALC_TIMESTAMPS, "format_regex")),
        DB2_TIMESTAMP_REGEX,
        "{INTCALC_TIMESTAMPS}: the format DB2_TIMESTAMP_SHAPE mirrors"
    );
    run(h, &INTCALC, NORMAL_END);

    assert_eq!(generation_names(h, SYSTRAN), [generation(SYSTRAN, 1)]);
    let records = split_records(&h.newest(SYSTRAN), TRAN_RECORD_LEN);
    let d3 = audit_rows(d_table(doc, "D.3"));
    let expected = systran_records(&d3);
    assert_eq!(
        records.len().to_string(),
        expected_value(doc, INTCALC_SYSTRAN, "records"),
        "{INTCALC_SYSTRAN}: interest records"
    );
    assert_eq!(
        records.len(),
        expected.len(),
        "one interest record per D.3 category row with a TRAN-ID"
    );
    for (record, (id, want)) in records.iter().zip(&expected) {
        assert_record(want, record, &format!("{id}: SYSTRAN record"));
        let orig_ts = field(record, &TRAN_ORIG_TS);
        assert_db2_timestamp(orig_ts, id);
        assert_eq!(
            orig_ts,
            field(record, &TRAN_PROC_TS),
            "{INTCALC_TIMESTAMPS}: {id} holds one timestamp in both fields"
        );
    }
    assert_eq!(
        records.first().map(|record| field(record, &TRAN_ID)),
        Some(expected_value(doc, INTCALC_SYSTRAN, "first").as_str())
    );
    assert_eq!(
        records.last().map(|record| field(record, &TRAN_ID)),
        Some(expected_value(doc, INTCALC_SYSTRAN, "last").as_str())
    );

    assert_accounts(h, &d3, "INTCALC");
    let d2 = audit_rows(d_table(doc, "D.2"));
    assert_last_account_unchanged(&d3, &d2);
    assert_tcatbals(h, &d2, "INTCALC");
}

/// The SYSTRAN records D.3 expects, in category key order, each with the ID of its category
/// row: the template of P-CBACT04C-173 filled with the row's TRAN-ID and monthly interest, the
/// account, and the card of the account row. The timestamps and the bytes the COBOL never sets
/// are left out.
fn systran_records(d3: &[AuditRow<'_>]) -> Vec<(String, Pattern)> {
    let template = d3
        .iter()
        .find(|row| row.id == INTCALC_SYSTRAN)
        .unwrap_or_else(|| panic!("D.3 has no row {INTCALC_SYSTRAN}"));
    assert_eq!(template.value("kind"), "systran_template");
    assert_eq!(template.value("tran_amt"), "WS-MONTHLY-INT");
    assert_eq!(template.value("tran_card_num"), "XREF-CARD-NUM");
    assert_eq!(
        template.value("tran_merchant_name, tran_merchant_city, tran_merchant_zip"),
        "spaces"
    );
    assert!(
        template
            .value("tran_orig_ts")
            .starts_with("tran_proc_ts=Z-GET-DB2-FORMAT-TIMESTAMP"),
        "{INTCALC_SYSTRAN}: both timestamps come from Z-GET-DB2-FORMAT-TIMESTAMP"
    );
    let width = TRAN_CAT_CD.len;
    let category = template.value("tran_cat_cd");
    let category = format!("{category:0>width$}");
    let source = exact_text(&template.span("tran_source"), template.id);
    let filler = render(template.value("filler"), FILLER.len, template.id, &no_field);
    let description = template.span("tran_desc");
    let cards: BTreeMap<&str, &str> = d3
        .iter()
        .filter_map(|row| Some((row.get("account_id")?, row.get("card_number")?)))
        .collect();

    let mut records = Vec::new();
    for row in d3
        .iter()
        .filter(|row| row.get("monthly_interest").is_some())
    {
        let interest = row.value("monthly_interest");
        let Some(tran_id) = row.get("tran_id") else {
            assert!(
                interest.starts_with("none"),
                "{}: an interest of {interest} without a TRAN-ID",
                row.id
            );
            continue;
        };
        let account = row.value("account_id");
        let card = cards
            .get(account)
            .unwrap_or_else(|| panic!("D.3 gives no card for account {account}"));
        let mut want = spaces(TRAN_RECORD_LEN);
        put(&mut want, &TRAN_ID, &known(tran_id.as_bytes()));
        put(
            &mut want,
            &TRAN_TYPE_CD,
            &known(template.value("tran_type_cd").as_bytes()),
        );
        put(&mut want, &TRAN_CAT_CD, &known(category.as_bytes()));
        put(&mut want, &TRAN_SOURCE, &source);
        let account_text = |name: &str| {
            assert_eq!(
                name, "ACCT-ID",
                "{INTCALC_SYSTRAN}: tran_desc names {{{name}}}"
            );
            known(account.as_bytes())
        };
        put(
            &mut want,
            &TRAN_DESC,
            &render(
                &unquoted(&description),
                TRAN_DESC.len,
                template.id,
                &account_text,
            ),
        );
        put(
            &mut want,
            &TRAN_AMT,
            &overpunched(interest, TRAN_AMT.len, row.id),
        );
        put(
            &mut want,
            &TRAN_MERCHANT_ID,
            &known(template.value("tran_merchant_id").as_bytes()),
        );
        for blank in [&TRAN_MERCHANT_NAME, &TRAN_MERCHANT_CITY, &TRAN_MERCHANT_ZIP] {
            put(&mut want, blank, &spaces(blank.len));
        }
        put(&mut want, &TRAN_CARD_NUM, &known(card.as_bytes()));
        put(&mut want, &TRAN_ORIG_TS, &unknown(TRAN_ORIG_TS.len));
        put(&mut want, &TRAN_PROC_TS, &unknown(TRAN_PROC_TS.len));
        put(&mut want, &FILLER, &filler);
        records.push((row.id.to_owned(), want));
    }
    records
}

/// E-14: the only D.3 account row INTCALC does not rewrite is P-CBACT04C-172, and its balance
/// and cycle amounts are those POSTTRAN left (D.2).
fn assert_last_account_unchanged(d3: &[AuditRow<'_>], d2: &[AuditRow<'_>]) {
    let unchanged: Vec<&AuditRow<'_>> = d3
        .iter()
        .filter(|row| {
            row.get("updated")
                .is_some_and(|updated| updated.starts_with("no"))
        })
        .collect();
    let ids: Vec<&str> = unchanged.iter().map(|row| row.id).collect();
    assert_eq!(
        ids,
        [INTCALC_LAST_ACCOUNT],
        "E-14: one account is not rewritten"
    );
    for row in unchanged {
        let account = row.value("account_id");
        let posted = d2
            .iter()
            .find(|candidate| {
                candidate.get("acct_curr_bal").is_some()
                    && candidate.get("account_id") == Some(account)
            })
            .unwrap_or_else(|| panic!("D.2 has no row for account {account}"));
        for key in [
            "acct_curr_bal",
            "acct_curr_cyc_credit",
            "acct_curr_cyc_debit",
        ] {
            assert_eq!(
                row.value(key),
                posted.value(key),
                "{} and {}: {key} of the last account (E-14)",
                row.id,
                posted.id
            );
        }
    }
}

/// TRANBKP at its second README position [README.md:L227]: `TRANSACT.BKUP.G0002V00` holds the
/// posted records in TRAN-ID order, each the daily record with its pinned processing timestamp
/// (P-TRANBKP-001, P-CBTRN02C-216), and the live master is empty. The accrued rows leave the
/// master but stay the targets of their accruals (E-09).
fn back_up_posted_master(h: &Harness, doc: &Document, posted: &Posted) {
    run(h, &TRANBKP, NORMAL_END);
    assert_eq!(
        generation_names(h, BKUP),
        [generation(BKUP, 1), generation(BKUP, 2)]
    );
    let records = split_records(&h.newest(BKUP), TRAN_RECORD_LEN);
    assert_eq!(
        records.len(),
        posted.records.len(),
        "{BACKUP_FULL}: the backup holds every posted record"
    );
    let proc_ts = expected_value(doc, POSTTRAN_POSTED, "tran_proc_ts");
    for (record, daily) in records.iter().zip(&posted.records) {
        let mut want = known(daily);
        put(&mut want, &TRAN_PROC_TS, &known(proc_ts.as_bytes()));
        put(&mut want, &FILLER, &unknown(FILLER.len));
        assert_record(
            &want,
            record,
            &format!("{BACKUP_FULL}: TRANSACT.BKUP record {}", tran_id_of(daily)),
        );
    }
    assert!(
        h.live_master_rows().is_empty(),
        "TRANBKP empties the live master"
    );
    let retired: Vec<AccrualLink> = posted
        .accruals
        .iter()
        .map(|link| AccrualLink {
            live_tran: None,
            ..link.clone()
        })
        .collect();
    assert_eq!(
        accrual_links(h),
        retired,
        "E-09: every accrual keeps its transaction row, which has left the live master"
    );
}

/// COMBTRAN [README.md:L228] over the second backup and the interest records:
/// `TRANSACT.COMBINED.G0001V00` holds both inputs sorted by TRAN-ID (P-COMBTRAN-004), the live
/// master that TRANBKP emptied is reloaded with exactly those records, byte for byte as the
/// codec writes them (P-COMBTRAN-005), and every accrual is linked to a live row again (E-09).
fn combine_transactions(h: &Harness, doc: &Document, posted: &Posted) {
    let backup = h.newest(BKUP);
    let systran = h.newest(SYSTRAN);
    run(h, &COMBTRAN, exit_of(doc, COMBINED_RELOAD));
    assert_eq!(generation_names(h, COMBINED), [generation(COMBINED, 1)]);

    let mut inputs = split_records(&backup, TRAN_RECORD_LEN);
    inputs.extend(split_records(&systran, TRAN_RECORD_LEN));
    inputs.sort_by(|left, right| field(left, &TRAN_ID).cmp(field(right, &TRAN_ID)));
    assert!(
        expected_value(doc, COMBINED_FILE, "equal_keys").starts_with("none"),
        "{COMBINED_FILE}: the corpus has no equal keys, so their undefined order (E-38) is moot"
    );
    assert!(
        inputs
            .windows(2)
            .all(|pair| field(&pair[0], &TRAN_ID) != field(&pair[1], &TRAN_ID)),
        "{COMBINED_FILE}: no two input records share a TRAN-ID"
    );
    let combined = h.newest(COMBINED);
    assert_same_records(&combined, &inputs, "TRANSACT.COMBINED");
    assert_eq!(
        inputs.len().to_string(),
        expected_value(doc, COMBINED_FILE, "records"),
        "{COMBINED_FILE}: combined records"
    );
    assert_eq!(
        inputs.first().map(|record| field(record, &TRAN_ID)),
        Some(expected_value(doc, COMBINED_FILE, "first").as_str())
    );
    assert_eq!(
        inputs.last().map(|record| field(record, &TRAN_ID)),
        Some(expected_value(doc, COMBINED_FILE, "last").as_str())
    );

    let live = live_master_file(h);
    assert_eq!(
        split_records(&live, TRAN_RECORD_LEN).len().to_string(),
        expected_value(doc, COMBINED_RELOAD, "loaded"),
        "{COMBINED_RELOAD}: records loaded"
    );
    assert_same_records(&live, &inputs, "the live master after COMBTRAN");
    assert_eq!(
        accrual_links(h),
        posted.accruals,
        "E-09: COMBTRAN makes every accrued row live again"
    );
}

/// The live transaction master in TRAN-ID order, each row encoded with the codec
/// `TransactionLayout` and followed by LF: the bytes a file of its records would hold. The
/// rows are read through the store, so only `transaction_master` is read.
fn live_master_file(h: &Harness) -> Vec<u8> {
    let db = h.db();
    let rows = block_on(async {
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("cannot begin a unit of work: {error}"));
        let rows = uow
            .store()
            .live_master_after(None, LIVE_MASTER_READ_LIMIT)
            .await;
        if let Err(error) = uow.rollback().await {
            panic!("cannot roll back the live master read: {error}");
        }
        rows.unwrap_or_else(|error| panic!("cannot read the live master: {error}"))
    });
    assert!(
        rows.len() < usize::from(LIVE_MASTER_READ_LIMIT),
        "the live master read stopped at its limit of {LIVE_MASTER_READ_LIMIT} rows"
    );
    let records: Vec<TransactionRecord> = rows
        .iter()
        .map(|row| TransactionRecord::new(row.to_new()))
        .collect();
    encode_ascii_lf::<TransactionLayout>(&records)
}

/// One statement file of P-CBSTM03A-015, as the D.5 rows name it (`STMTFILE`, `HTMLFILE`):
/// its dataset name under `CARDDEMO_STATEMENT_DIR`, record count and width, and the line
/// count of one card's statement as an expression of the card's transaction count `n`.
struct StatementFile {
    file: String,
    name: String,
    records: usize,
    width: usize,
    per_card: String,
}

/// The statement of one card of D.5: its card number, its transaction count `n` (its
/// `card_summary` row), and the expected lines of each [`StatementFile`], in their order,
/// each `None` until an audit row gives it.
struct Statement {
    card: String,
    transactions: usize,
    lines: Vec<Vec<Option<Pattern>>>,
}

/// The source of a statement line that copies part of another line: `NAME is bytes A-B of FILE
/// line N of the same card` in the `field=` of a D.5 template row.
struct FieldRef {
    name: String,
    from: usize,
    to: usize,
    file: String,
    line: String,
}

impl FieldRef {
    /// The reference `description` states, for the row `id`.
    fn parse(description: &str, id: &str) -> Self {
        let words: Vec<&str> = description.split_whitespace().collect();
        let [
            name,
            "is",
            "bytes",
            range,
            "of",
            file,
            "line",
            line,
            "of",
            "the",
            "same",
            "card",
        ] = words.as_slice()
        else {
            panic!(
                "{id}: field={description:?} is not `NAME is bytes A-B of FILE line N of the \
                 same card`"
            );
        };
        let (from, to) = range
            .split_once('-')
            .unwrap_or_else(|| panic!("{id}: {range:?} is not a byte range A-B"));
        let (from, to) = (count(from, id), count(to, id));
        assert!(
            (1..=to).contains(&from),
            "{id}: bytes {range} are not a 1-based range"
        );
        Self {
            name: (*name).to_owned(),
            from,
            to,
            file: (*file).to_owned(),
            line: (*line).to_owned(),
        }
    }
}

/// CREASTMT [README.md:L229] over the live master COMBTRAN reloaded: `STATEMNT.PS` and
/// `STATEMNT.HTML` under the statement directory hold, card after card, exactly the lines D.5
/// gives, at 80 and 100 bytes each followed by LF (P-CBSTM03A-015), and the corpus stays
/// within the CBSTM03A tables (P-CBSTM03A-014).
fn create_statements(h: &Harness, doc: &Document) {
    run(h, &CREASTMT, NORMAL_END);
    let files = statement_files(doc);
    let statements = expected_statements(&audit_rows(d_table(doc, "D.5")), &files);
    assert_statement_capacity(doc, &statements, h.live_master_rows().len());
    for (index, file) in files.iter().enumerate() {
        let path = h.statement_dir().join(&file.name);
        let bytes = fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let records = split_records(&bytes, file.width);
        let expected: Vec<(&str, usize, &Pattern)> = statements
            .iter()
            .flat_map(|statement| {
                statement.lines[index]
                    .iter()
                    .enumerate()
                    .filter_map(move |(at, line)| {
                        Some((statement.card.as_str(), at + 1, line.as_ref()?))
                    })
            })
            .collect();
        assert_eq!(
            expected.len(),
            file.records,
            "{STATEMENT_FILES}: the D.5 card statements add up to the {} records",
            file.name
        );
        assert_eq!(
            records.len(),
            file.records,
            "{STATEMENT_FILES}: records of {}",
            file.name
        );
        for (record, (card, line, want)) in records.iter().zip(&expected) {
            assert_record(
                want,
                record,
                &format!("{}: card {card} line {line}", file.name),
            );
        }
    }
}

/// The two statement files of P-CBSTM03A-015, in the order the row lists them: each `file=`
/// starts a file whose `name`, `records`, `width` and `terminator` follow it, and
/// `per_card_records` gives each file's lines per card after its file name.
fn statement_files(doc: &Document) -> Vec<StatementFile> {
    let pairs = expected_values(doc, STATEMENT_FILES);
    let mut files: Vec<StatementFile> = Vec::new();
    let mut per_card = None;
    for pair in &pairs {
        let value = pair.value.as_str();
        match (pair.key.as_str(), files.last_mut()) {
            ("file", _) => files.push(StatementFile {
                file: value.to_owned(),
                name: String::new(),
                records: 0,
                width: 0,
                per_card: String::new(),
            }),
            ("name", Some(file)) => value.clone_into(&mut file.name),
            ("records", Some(file)) => file.records = count(value, STATEMENT_FILES),
            ("width", Some(file)) => file.width = count(value, STATEMENT_FILES),
            ("terminator", Some(file)) => {
                assert_eq!(value, "LF", "{STATEMENT_FILES}: {} terminator", file.file);
            }
            ("per_card_records", _) => per_card = Some(value),
            _ => {}
        }
    }
    let per_card =
        per_card.unwrap_or_else(|| panic!("{STATEMENT_FILES} gives no per_card_records"));
    for file in &mut files {
        assert!(
            !file.name.is_empty() && file.records > 0 && file.width > 0,
            "{STATEMENT_FILES}: {} lacks a name, record count or width",
            file.file
        );
        file.per_card = per_card
            .split(',')
            .find_map(|part| part.trim().strip_prefix(file.file.as_str()).map(str::trim))
            .unwrap_or_else(|| panic!("{STATEMENT_FILES}: no records per card of {}", file.file))
            .to_owned();
    }
    assert_eq!(files.len(), 2, "{STATEMENT_FILES}: STMTFILE and HTMLFILE");
    files
}

/// The statements D.5 describes, card by card in the order of its `card_summary` rows, with
/// every line of every file given by exactly one `template` or `variable` row.
///
/// A template row holds for every card; one whose `line_no` names `i` holds once per
/// transaction `i` of the card. A line that copies part of another line (`field=`) is built
/// after every line that copies nothing.
fn expected_statements(rows: &[AuditRow<'_>], files: &[StatementFile]) -> Vec<Statement> {
    let mut statements: Vec<Statement> = rows
        .iter()
        .filter(|row| row.get("kind") == Some("card_summary"))
        .map(|row| {
            let transactions = count(row.value("n"), row.id);
            Statement {
                card: row.value("card_number").to_owned(),
                transactions,
                lines: files
                    .iter()
                    .map(|file| vec![None; LineExpr::eval(&file.per_card, transactions, None)])
                    .collect(),
            }
        })
        .collect();
    for row in rows {
        if let Some(kind) = row.get("kind") {
            assert!(
                matches!(kind, "card_summary" | "template" | "variable"),
                "{}: unknown D.5 row kind {kind}",
                row.id
            );
        }
    }
    let (copies, plain): (Vec<&AuditRow<'_>>, Vec<&AuditRow<'_>>) = rows
        .iter()
        .filter(|row| matches!(row.get("kind"), Some("template" | "variable")))
        .partition(|row| row.get("field").is_some());
    for row in plain.into_iter().chain(copies) {
        place_statement_row(&mut statements, files, row);
    }
    for statement in &statements {
        for (file, lines) in files.iter().zip(&statement.lines) {
            let gap = lines.iter().position(Option::is_none);
            assert!(
                gap.is_none(),
                "D.5 gives no line {} of the {} statement of card {}",
                gap.map_or(0, |at| at + 1),
                file.file,
                statement.card
            );
        }
    }
    statements
}

/// Places the line or lines the D.5 `template` or `variable` row `row` gives.
fn place_statement_row(statements: &mut [Statement], files: &[StatementFile], row: &AuditRow<'_>) {
    let file_name = row.value("file");
    let file = files
        .iter()
        .position(|file| file.file == file_name)
        .unwrap_or_else(|| panic!("{}: unknown statement file {file_name}", row.id));
    let width = files[file].width;
    let span = row.span("text");
    assert_eq!(
        span.width,
        Some(width),
        "{}: a {file_name} line is {width} bytes",
        row.id
    );
    let text = unquoted(&span);
    let line_no = row.value("line_no");
    if row.value("kind") == "variable" {
        let card = row.value("card_number");
        let statement = statements
            .iter_mut()
            .find(|statement| statement.card == card)
            .unwrap_or_else(|| panic!("{}: card {card} has no card_summary row", row.id));
        let line = LineExpr::eval(line_no, statement.transactions, None);
        let want = render(&text, width, row.id, &no_field);
        place(&mut statement.lines[file], line, want, row.id, card);
        return;
    }
    let copied = row
        .get("field")
        .map(|description| FieldRef::parse(description, row.id));
    for statement in statements.iter_mut() {
        let transactions: Vec<Option<usize>> = if line_no.contains('i') {
            (1..=statement.transactions).map(Some).collect()
        } else {
            vec![None]
        };
        for i in transactions {
            let line = LineExpr::eval(line_no, statement.transactions, i);
            let source =
                |name: &str| copied_field(copied.as_ref(), statement, files, i, name, row.id);
            let want = render(&text, width, row.id, &source);
            place(
                &mut statement.lines[file],
                line,
                want,
                row.id,
                &statement.card,
            );
        }
    }
}

/// The bytes `{name}` stands for in a line of `statement`: bytes A to B of the line `field`
/// names, with `i` the transaction of the line being built.
fn copied_field(
    field: Option<&FieldRef>,
    statement: &Statement,
    files: &[StatementFile],
    i: Option<usize>,
    name: &str,
    id: &str,
) -> Pattern {
    let field = field
        .filter(|field| field.name == name)
        .unwrap_or_else(|| panic!("{id}: the text names {{{name}}}, which no field= defines"));
    let file = files
        .iter()
        .position(|file| file.file == field.file)
        .unwrap_or_else(|| panic!("{id}: unknown statement file {}", field.file));
    let line = LineExpr::eval(&field.line, statement.transactions, i);
    let source = line
        .checked_sub(1)
        .and_then(|at| statement.lines[file].get(at))
        .and_then(Option::as_ref)
        .unwrap_or_else(|| {
            panic!(
                "{id}: {} line {line} of card {} is not given",
                field.file, statement.card
            )
        });
    source
        .get(field.from - 1..field.to)
        .unwrap_or_else(|| {
            panic!(
                "{id}: {} has no bytes {}-{}",
                field.file, field.from, field.to
            )
        })
        .to_vec()
}

/// Puts `want` at the 1-based `line` of one file of the statement of `card`, which no other row
/// may give.
fn place(lines: &mut [Option<Pattern>], line: usize, want: Pattern, id: &str, card: &str) {
    let total = lines.len();
    let slot = line
        .checked_sub(1)
        .and_then(|at| lines.get_mut(at))
        .unwrap_or_else(|| {
            panic!("{id}: line {line} lies outside the {total} lines of the statement of {card}")
        });
    assert!(
        slot.is_none(),
        "{id}: line {line} of card {card} is given twice"
    );
    *slot = Some(want);
}

/// P-CBSTM03A-014: as many statements as D.5 has cards, their transactions adding up to the
/// live master COMBTRAN reloaded, none beyond the CBSTM03A tables, cards in ascending order.
fn assert_statement_capacity(doc: &Document, statements: &[Statement], live: usize) {
    let expected = |key: &str| {
        count(
            &expected_value(doc, STATEMENT_CAPACITY, key),
            STATEMENT_CAPACITY,
        )
    };
    assert_eq!(
        statements.len(),
        expected("cards"),
        "{STATEMENT_CAPACITY}: cards"
    );
    let transactions: usize = statements
        .iter()
        .map(|statement| statement.transactions)
        .sum();
    assert_eq!(
        transactions,
        expected("transactions"),
        "{STATEMENT_CAPACITY}: transactions"
    );
    assert_eq!(
        transactions, live,
        "the statements list every transaction of the live master"
    );
    let most = statements
        .iter()
        .map(|statement| statement.transactions)
        .max()
        .unwrap_or(0);
    assert_eq!(
        most,
        expected("max_transactions_per_card"),
        "{STATEMENT_CAPACITY}: transactions of the busiest card"
    );
    assert!(
        expected_value(doc, STATEMENT_CAPACITY, "within_capacity").starts_with("yes"),
        "{STATEMENT_CAPACITY}: the corpus fits the 51-card and 10-transaction tables"
    );
    assert!(
        statements
            .windows(2)
            .all(|pair| pair[0].card < pair[1].card),
        "D.5 lists the cards in card-number order"
    );
}

/// CBPAUP0J [README.md:L233] with the JCL `SYSIN`: the corpus holds no pending authorization,
/// so the first summary read ends the loop (P-CBPAUP0C-002), nothing is read or deleted, and
/// the run ends normally.
fn purge_expired_authorizations(h: &Harness, doc: &Document) {
    assert_eq!(
        text_rows(h, PENDING_AUTHORIZATIONS_SQL),
        [["0", "0"]],
        "the sequence writes no pending authorization"
    );
    let card = parity_cell(doc, PURGE_SYSIN, "Input condition")
        .strip_prefix("SYSIN '")
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or_else(|| panic!("{PURGE_SYSIN} names no SYSIN card"));
    let output = run(h, &CBPAUP0J, NORMAL_END);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sysin = format!("\"sysin\":\"{card}");
    assert!(
        stdout
            .lines()
            .any(|line| line.contains(r#""message":"PARM RECEIVED""#) && line.contains(&sysin)),
        "{PURGE_SYSIN}: the run reads the JCL SYSIN card {card:?}\n{stdout}"
    );
    let zeros: Vec<String> = [
        "summaries_read",
        "summaries_deleted",
        "details_read",
        "details_deleted",
    ]
    .iter()
    .map(|key| format!("\"{key}\":0"))
    .collect();
    assert!(
        stdout.lines().any(|line| {
            line.contains(r#""message":"expired pending authorizations purged""#)
                && zeros.iter().all(|zero| line.contains(zero.as_str()))
        }),
        "{PURGE_SUMMARIES}: nothing to read or delete\n{stdout}"
    );
    assert_eq!(
        text_rows(h, PENDING_AUTHORIZATIONS_SQL),
        [["0", "0"]],
        "CBPAUP0J leaves no pending authorization"
    );
}

/// Runs `batch <args>` under the pinned clock and checks its exit `code`. A job that ends
/// without an abend, a halt or a setup failure writes nothing to stderr, whatever its condition
/// code.
fn run(h: &Harness, args: &[&str], code: i32) -> Output {
    let output = h.run_expect(args, Some(TRANREPT_CLOCK), code);
    assert!(
        output.stderr.is_empty(),
        "batch {args:?} wrote to stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// The `exit=` of the audit row `id` as an exit code.
fn exit_of(doc: &Document, id: &str) -> i32 {
    let exit = expected_value(doc, id, "exit");
    exit.parse()
        .unwrap_or_else(|error| panic!("{id}: exit={exit} is not an exit code: {error}"))
}

/// The count `text` of the audit row `id`.
fn count(text: &str, id: &str) -> usize {
    text.parse()
        .unwrap_or_else(|error| panic!("{id}: {text:?} is not a count: {error}"))
}

/// The rows of `sql`, every column in its `PostgreSQL` text form; a null fails the test.
fn text_rows(h: &Harness, sql: &str) -> Vec<Vec<String>> {
    block_on(h.stack().fetch_text_rows(sql))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| value.unwrap_or_else(|| panic!("a row of {sql} holds a null")))
                .collect()
        })
        .collect()
}

/// The file names of the cataloged generations of `dataset`, oldest first.
fn generation_names(h: &Harness, dataset: &str) -> Vec<String> {
    h.generations(dataset)
        .iter()
        .map(|path| {
            path.file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_else(|| panic!("{} has no UTF-8 file name", path.display()))
                .to_owned()
        })
        .collect()
}

/// The file of the absolute generation `number` of `dataset`, `<DATASET>.G<nnnn>V00`.
fn generation(dataset: &str, number: u16) -> String {
    format!("{dataset}.G{number:04}V00")
}

/// The column `name` of every row `Harness::live_master_rows` gave, in row order.
fn live_column<'r>(rows: &'r [Vec<Option<String>>], name: &str) -> Vec<&'r str> {
    let at = LIVE_MASTER_COLUMNS
        .iter()
        .position(|column| *column == name)
        .unwrap_or_else(|| panic!("transaction_master has no column {name}"));
    rows.iter()
        .map(|row| {
            row.get(at)
                .and_then(Option::as_deref)
                .unwrap_or_else(|| panic!("a live transaction has no {name}"))
        })
        .collect()
}

/// The TRAN-ID of a daily transaction record: `DALYTRAN-ID X(16)` lies at the offset and width
/// of `TRAN-ID`, as every `CVTRA06Y` field lies where its `CVTRA05Y` field does.
fn tran_id_of(record: &[u8]) -> &str {
    record
        .get(TRAN_ID.offset..TRAN_ID.end())
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .expect("a daily transaction record holds a TRAN-ID")
}

/// The bytes of the field `spec` in the record `record`.
fn field<'r>(record: &'r str, spec: &FieldSpec) -> &'r str {
    record
        .get(spec.offset..spec.end())
        .unwrap_or_else(|| panic!("a record of {} bytes holds no {}", record.len(), spec.name))
}

/// Every reward accrual with its transaction row and that row's live TRAN-ID.
fn accrual_links(h: &Harness) -> Vec<AccrualLink> {
    block_on(h.stack().fetch_text_rows(ACCRUAL_LINKS_SQL))
        .into_iter()
        .map(|row| {
            let [accrual_id, transaction_id, live_tran] = <[Option<String>; 3]>::try_from(row)
                .unwrap_or_else(|row| panic!("{ACCRUAL_LINKS_SQL} gave {} columns", row.len()));
            AccrualLink {
                accrual_id: accrual_id.expect("an accrual has an identity"),
                transaction_id: transaction_id.expect("an accrual references a transaction row"),
                live_tran,
            }
        })
        .collect()
}

/// The accounts after `step` equal the rows of `rows` that give an account's balance and cycle
/// amounts: one row per account, each `ACCT-CURR-BAL`, `ACCT-CURR-CYC-CREDIT` and
/// `ACCT-CURR-CYC-DEBIT` as the audit writes it.
fn assert_accounts(h: &Harness, rows: &[AuditRow<'_>], step: &str) {
    let actual: BTreeMap<String, [String; 3]> = text_rows(h, ACCOUNTS_SQL)
        .into_iter()
        .map(|row| {
            let [account, balance, credit, debit] = <[String; 4]>::try_from(row)
                .unwrap_or_else(|row| panic!("{ACCOUNTS_SQL} gave {} columns", row.len()));
            (account, [balance, credit, debit])
        })
        .collect();
    let expected: Vec<&AuditRow<'_>> = rows
        .iter()
        .filter(|row| row.get("acct_curr_bal").is_some())
        .collect();
    assert_eq!(
        actual.len(),
        expected.len(),
        "accounts after {step}: the audit gives one row per account"
    );
    for row in expected {
        let account = row.value("account_id");
        let got = actual
            .get(account)
            .unwrap_or_else(|| panic!("{}: no account {account} after {step}", row.id));
        let want = [
            "acct_curr_bal",
            "acct_curr_cyc_credit",
            "acct_curr_cyc_debit",
        ]
        .map(|key| row.value(key));
        assert_eq!(
            got.each_ref().map(String::as_str),
            want,
            "{}: balance, cycle credit and cycle debit of account {account} after {step}",
            row.id
        );
    }
}

/// The category balances after `step` equal the D.2 category rows of `rows`, one per key.
fn assert_tcatbals(h: &Harness, rows: &[AuditRow<'_>], step: &str) {
    let actual: BTreeMap<[String; 3], String> = text_rows(h, TCATBALS_SQL)
        .into_iter()
        .map(|row| {
            let [account, type_cd, category, balance] = <[String; 4]>::try_from(row)
                .unwrap_or_else(|row| panic!("{TCATBALS_SQL} gave {} columns", row.len()));
            ([account, type_cd, category], balance)
        })
        .collect();
    let expected: Vec<&AuditRow<'_>> = rows
        .iter()
        .filter(|row| row.get("tran_cat_bal").is_some() && row.get("created").is_some())
        .collect();
    assert_eq!(
        actual.len(),
        expected.len(),
        "category balances after {step}: the audit gives one row per key"
    );
    for row in expected {
        let key =
            ["account_id", "tran_type_cd", "tran_cat_cd"].map(|key| row.value(key).to_owned());
        let got = actual
            .get(&key)
            .unwrap_or_else(|| panic!("{}: no category balance {key:?} after {step}", row.id));
        assert_eq!(
            got,
            row.value("tran_cat_bal"),
            "{}: category balance {key:?} after {step}",
            row.id
        );
    }
}

/// The data table of the section D subsection numbered `number`, such as `D.2`.
fn d_table<'d>(doc: &'d Document, number: &str) -> &'d Table {
    let heading = format!("{number} ");
    let mut tables = doc.section_tables("D.").filter(|table| {
        table.is_data_table()
            && table
                .subsection
                .as_deref()
                .is_some_and(|subsection| subsection.starts_with(&heading))
    });
    let table = tables
        .next()
        .unwrap_or_else(|| panic!("section D has no table {number}"));
    assert!(
        tables.next().is_none(),
        "section D has more than one table {number}"
    );
    table
}

/// One data row of an audit table: its ID and the `key=value` pairs of its `Expected result`.
struct AuditRow<'d> {
    id: &'d str,
    values: Vec<KeyValue>,
}

impl AuditRow<'_> {
    /// The value of the first `key=` pair, if any.
    fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|pair| pair.key == key)
            .map(|pair| pair.value.as_str())
    }

    /// The value of the first `key=` pair, which the row must give.
    fn value(&self, key: &str) -> &str {
        self.get(key)
            .unwrap_or_else(|| panic!("parity audit row {} expects no {key}=", self.id))
    }

    /// The exact text of the first `key=` pair, which the row must give.
    fn span(&self, key: &str) -> CodeSpan {
        carddemo_audit::code_span(self.value(key))
            .unwrap_or_else(|| panic!("parity audit row {}: {key}= holds no exact text", self.id))
    }
}

/// Every row of the data table `table`.
fn audit_rows(table: &Table) -> Vec<AuditRow<'_>> {
    table
        .rows
        .iter()
        .map(|row| AuditRow {
            id: cell(table, row, "ID"),
            values: carddemo_audit::key_values(cell(table, row, EXPECTED_RESULT)),
        })
        .collect()
}

/// The cell of `row` under `column`, which the row must reach.
fn cell<'d>(table: &'d Table, row: &'d Row, column: &str) -> &'d str {
    table.cell(row, column).unwrap_or_else(|| {
        panic!(
            "the parity audit row at line {} has no {column} cell",
            row.line
        )
    })
}

/// The expected bytes of one fixed-width record, `None` where a byte is left out of the
/// comparison: a byte the COBOL never sets (`{UNDEF:n}`, E-25) or a clock-dependent field.
type Pattern = Vec<Option<u8>>;

/// Every byte of `bytes`, each compared.
fn known(bytes: &[u8]) -> Pattern {
    bytes.iter().copied().map(Some).collect()
}

/// `len` spaces, each compared.
fn spaces(len: usize) -> Pattern {
    vec![Some(b' '); len]
}

/// `len` bytes left out of the comparison.
fn unknown(len: usize) -> Pattern {
    vec![None; len]
}

/// Puts `bytes` into the field `spec` of the record `want`.
fn put(want: &mut [Option<u8>], spec: &FieldSpec, bytes: &[Option<u8>]) {
    assert_eq!(
        bytes.len(),
        spec.len,
        "{} holds {} bytes",
        spec.name,
        spec.len
    );
    want[spec.offset..spec.end()].copy_from_slice(bytes);
}

/// The text of the audit exact text `span`: its double-quoted string with the `\"` and `\\`
/// escapes resolved (Conventions of the audit).
fn unquoted(span: &CodeSpan) -> String {
    let quoted = span.text.as_str();
    let inner = quoted
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{quoted:?} is not a double-quoted exact text"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                other => panic!("{quoted:?} holds the unknown escape {other:?}"),
            }
        } else {
            text.push(ch);
        }
    }
    text
}

/// The record `text` of the audit row `id` describes, `width` bytes: each `{UNDEF:k}` stands
/// for k bytes left out of the comparison, every other `{NAME}` for the bytes `field` gives it,
/// and the rest is right-padded with spaces (Conventions: `width=<n>`).
fn render(text: &str, width: usize, id: &str, field: &dyn Fn(&str) -> Pattern) -> Pattern {
    let mut want = Pattern::with_capacity(width);
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        want.extend(known(&rest.as_bytes()[..open]));
        let (name, after) = rest[open + 1..]
            .split_once('}')
            .unwrap_or_else(|| panic!("{id}: {text:?} leaves a placeholder open"));
        if let Some(undefined) = name.strip_prefix("UNDEF:") {
            want.extend(iter::repeat_n(None, count(undefined, id)));
        } else {
            want.extend(field(name));
        }
        rest = after;
    }
    want.extend(known(rest.as_bytes()));
    assert!(
        want.len() <= width,
        "{id}: {text:?} is longer than its {width} bytes"
    );
    want.resize(width, Some(b' '));
    want
}

/// The exact text `span` of the row `id` at its `width=`, naming no field.
fn exact_text(span: &CodeSpan, id: &str) -> Pattern {
    let width = span
        .width
        .unwrap_or_else(|| panic!("{id}: the exact text {} has no width=", span.text));
    render(&unquoted(span), width, id, &no_field)
}

/// The field of a text that may name none.
fn no_field(name: &str) -> Pattern {
    panic!("the audit text names {{{name}}}, which stands for no value here")
}

/// The `S9(n)V99` `DISPLAY` bytes of the decimal `amount` of the row `id`, such as `-47.88`:
/// `digits` digits, the sign overpunched on the last one.
fn overpunched(amount: &str, digits: usize, id: &str) -> Pattern {
    let (negative, magnitude) = amount
        .strip_prefix('-')
        .map_or((false, amount), |rest| (true, rest));
    let (units, cents) = magnitude
        .split_once('.')
        .unwrap_or_else(|| panic!("{id}: {amount} is not a decimal"));
    let number = format!("{units}{cents}");
    assert!(
        !units.is_empty() && cents.len() == 2 && number.bytes().all(|byte| byte.is_ascii_digit()),
        "{id}: {amount} is not an amount with two decimals"
    );
    let significant = number.trim_start_matches('0');
    assert!(
        significant.len() <= digits,
        "{id}: {amount} has more than {digits} digits"
    );
    let mut text = format!("{significant:0>digits$}").into_bytes();
    let last = text.last_mut().expect("an amount field holds digits");
    let digit = usize::from(*last - b'0');
    *last = if negative {
        NEGATIVE_OVERPUNCH[digit]
    } else {
        POSITIVE_OVERPUNCH[digit]
    };
    known(&text)
}

/// Whether `actual` equals `want` at every byte `want` compares.
fn fits(want: &[Option<u8>], actual: &[u8]) -> bool {
    want.len() == actual.len()
        && want
            .iter()
            .zip(actual)
            .all(|(byte, got)| byte.is_none_or(|byte| byte == *got))
}

/// `want` as text, `?` at each byte left out of the comparison.
fn shown(want: &[Option<u8>]) -> String {
    want.iter()
        .map(|byte| byte.map_or('?', char::from))
        .collect()
}

/// Fails with `what` and both records unless `actual` fits `want`.
fn assert_record(want: &[Option<u8>], actual: &str, what: &str) {
    assert!(
        fits(want, actual.as_bytes()),
        "{what}\nexpected {:?} (? is not compared)\n     got {actual:?}",
        shown(want)
    );
}

/// `bytes` holds exactly the `expected` records, in order, each followed by LF.
fn assert_same_records(bytes: &[u8], expected: &[String], what: &str) {
    let records = split_records(bytes, TRAN_RECORD_LEN);
    assert_eq!(records.len(), expected.len(), "{what}: records");
    for (number, (record, want)) in records.iter().zip(expected).enumerate() {
        assert_eq!(record, want, "{what}: record {}", number + 1);
    }
}

/// `value`, a SYSTRAN timestamp of the row `id`, is of the format P-CBACT04C-176 gives,
/// [`DB2_TIMESTAMP_SHAPE`].
fn assert_db2_timestamp(value: &str, id: &str) {
    let shaped = value.len() == DB2_TIMESTAMP_SHAPE.len()
        && value
            .bytes()
            .zip(DB2_TIMESTAMP_SHAPE)
            .all(|(byte, &shape)| {
                if shape == b'9' {
                    byte.is_ascii_digit()
                } else {
                    byte == shape
                }
            });
    assert!(
        shaped,
        "{id}: timestamp {value:?} does not match {DB2_TIMESTAMP_REGEX}"
    );
}

/// A D.5 line expression, such as `16+n+3`, `56+11*(i-1)+2` or the `64+11n` of
/// P-CBSTM03A-015: integers, `n` (the card's transaction count), `i` (the transaction, 1 to
/// `n`), `+`, `-`, `*` and parentheses; a name or parenthesis written right after a factor
/// multiplies it.
struct LineExpr<'e> {
    text: &'e str,
    bytes: &'e [u8],
    at: usize,
    n: i64,
    i: Option<i64>,
}

impl LineExpr<'_> {
    /// The value of `text` for a card of `n` transactions and, when `text` names `i`, its
    /// transaction `i`.
    fn eval(text: &str, n: usize, i: Option<usize>) -> usize {
        let wide = |value: usize| i64::try_from(value).expect("a statement count fits i64");
        let mut expr = LineExpr {
            text,
            bytes: text.as_bytes(),
            at: 0,
            n: wide(n),
            i: i.map(wide),
        };
        let value = expr.sum();
        assert!(
            expr.peek().is_none(),
            "line expression {text:?} has trailing text"
        );
        usize::try_from(value).unwrap_or_else(|_| panic!("line expression {text:?} gives {value}"))
    }

    /// The next byte that is not a space.
    fn peek(&mut self) -> Option<u8> {
        while self.bytes.get(self.at) == Some(&b' ') {
            self.at += 1;
        }
        self.bytes.get(self.at).copied()
    }

    /// Terms joined by `+` and `-`.
    fn sum(&mut self) -> i64 {
        let mut value = self.product();
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.at += 1;
                    value += self.product();
                }
                Some(b'-') => {
                    self.at += 1;
                    value -= self.product();
                }
                _ => return value,
            }
        }
    }

    /// Factors joined by `*` or written side by side.
    fn product(&mut self) -> i64 {
        let mut value = self.factor();
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.at += 1;
                    value *= self.factor();
                }
                Some(b'n' | b'i' | b'(') => value *= self.factor(),
                _ => return value,
            }
        }
    }

    /// A number, `n`, `i` or a parenthesized sum.
    fn factor(&mut self) -> i64 {
        let text = self.text;
        match self.peek() {
            Some(b'(') => {
                self.at += 1;
                let value = self.sum();
                assert_eq!(
                    self.peek(),
                    Some(b')'),
                    "line expression {text:?} leaves a ( open"
                );
                self.at += 1;
                value
            }
            Some(b'n') => {
                self.at += 1;
                self.n
            }
            Some(b'i') => {
                self.at += 1;
                self.i.unwrap_or_else(|| {
                    panic!("line expression {text:?} names i outside a transaction line")
                })
            }
            Some(byte) if byte.is_ascii_digit() => {
                let start = self.at;
                while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
                    self.at += 1;
                }
                text[start..self.at]
                    .parse()
                    .unwrap_or_else(|error| panic!("line expression {text:?}: {error}"))
            }
            _ => panic!(
                "line expression {text:?} lacks a number, n, i or ( at byte {}",
                self.at
            ),
        }
    }
}
