//! Writes that persist across an abend (AAP 0.6.4): a server-side read failure
//! after a completed legacy write must leave that write committed.
//!
//! Every file `CBACT04C` and `CBPAUP0C` write is `RECOVERY(NONE)` or committed
//! by the program's own `GOBACK`, so a write completed before an abend stays:
//!
//! - INTCALC (`batch intcalc`): at an account break `1050-UPDATE-ACCOUNT`
//!   rewrites the previous account before the next account's reads
//!   (P-CBACT04C-010). A failed cross-reference or disclosure group read then
//!   abends the step with `ABEND 999` (P-CBACT04C-188), and the rewrite stays.
//! - CBPAUP0J (`batch cbpaup0j`): the deletes made before a failed `GNP` stay,
//!   because `9999-ABEND` ends with `GOBACK` and the program's work so far is
//!   committed (P-CBPAUP0C-005, P-CBPAUP0C-009).
//!
//! `PostgreSQL` aborts the whole transaction on any server-side error, so the
//! store runs each statement, reads included, under its own savepoint. Each
//! case makes a read fail on the server after the write: a test-only trigger
//! on the written table renames the table the next read names, in the same
//! transaction, so the read fails with SQLSTATE `42P01`. The case then checks
//! the committed state, the exit code, the `DISPLAY` lines on stderr and the
//! failure event the persistence boundary logs on stdout.
//!
//! The INTCALC expectations are the audited ones after POSTTRAN
//! (P-CBTRN02C-065, P-CBACT04C-123), so each INTCALC case runs POSTTRAN first,
//! as the README cycle does.

pub mod common;

use carddemo_domain::messages::{
    ABENDING_PROGRAM, CBPAUP0C_ABENDING, ERROR_READING_DISCLOSURE_GROUP_FILE,
    ERROR_READING_XREF_FILE,
};
use common::{Harness, TRANREPT_CLOCK, block_on, expected_value, parity};

/// The INTCALC command of the JCL: `PARM='2022071800'` [app/jcl/INTCALC.jcl:L22].
const INTCALC: [&str; 3] = ["intcalc", "--parm-date", "2022071800"];

/// The CBPAUP0J command of the JCL `SYSIN` [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl]:
/// expiry 0 days, checkpoint frequencies 1 and 1, debug on.
const CBPAUP0J: [&str; 3] = ["cbpaup0j", "--sysin", "00,00001,00001,Y"];

/// The exit code of an abend (AAP 0.5.3.5).
const ABEND_EXIT: i32 = 16;

/// The audit row of POSTTRAN's end-of-job counts and exit code over the fixture day.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// The audit row of account 1 after INTCALC's account-break rewrite, after POSTTRAN.
const ACCOUNT_1_REWRITE: &str = "P-CBACT04C-123";

/// Every account row as one text column, in key order.
const ACCOUNTS_SQL: &str = "SELECT a::text FROM accounts a ORDER BY a.account_id";

/// Account 1's balance and cycle amounts.
const ACCOUNT_1_SQL: &str = "SELECT account_id, acct_curr_bal, acct_curr_cyc_credit, \
     acct_curr_cyc_debit FROM accounts WHERE account_id = 1";

/// One INTCALC case: the setup and trigger that hide a table once account 1 is
/// rewritten, the `DISPLAY` text of the paragraph whose read then fails, and the
/// repository file of that read.
struct IntcalcCase {
    trigger: &'static str,
    display: &'static str,
    repository: &'static str,
}

/// The reads after account 1's rewrite: account 2's cross-reference
/// (`1110-GET-XREF-DATA`, P-CBACT04C-002) and its disclosure group
/// (`1200-GET-INTEREST-RATE`, P-CBACT04C-004).
///
/// `batch intcalc` keeps every disclosure group it has read for the rest of the
/// run, and the fixture accounts share one blank group, so account 2's rates
/// would come from account 1's reads. The disclosure group case therefore gives
/// account 2 a group no earlier record names, which makes its first rate read
/// the first statement on `disclosure_groups` after account 1's rewrite.
const INTCALC_CASES: [IntcalcCase; 2] = [
    IntcalcCase {
        trigger: r"
UPDATE accounts SET acct_group_id = 'ISOLATED' WHERE account_id = 2;
CREATE FUNCTION hide_after_account_1() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'ALTER TABLE disclosure_groups RENAME TO disclosure_groups_offline';
    RETURN NULL;
END
$$;
CREATE TRIGGER hide_after_account_1 AFTER UPDATE ON accounts
    FOR EACH ROW WHEN (OLD.account_id = 1) EXECUTE FUNCTION hide_after_account_1();
",
        display: ERROR_READING_DISCLOSURE_GROUP_FILE,
        repository: "repos/discgrp.rs",
    },
    IntcalcCase {
        trigger: r"
CREATE FUNCTION hide_after_account_1() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'ALTER TABLE card_xref RENAME TO card_xref_offline';
    RETURN NULL;
END
$$;
CREATE TRIGGER hide_after_account_1 AFTER UPDATE ON accounts
    FOR EACH ROW WHEN (OLD.account_id = 1) EXECUTE FUNCTION hide_after_account_1();
",
        display: ERROR_READING_XREF_FILE,
        repository: "repos/xref.rs",
    },
];

/// Two pending-authorization summaries, of fixture accounts 1 and 2, each with expired
/// approved details (`PA-AUTH-RESP-CODE` `00`, dated 22180, two days before the clock's
/// day 22182). Summary 1's approved count equals its two details, so the purge deletes them
/// and then the summary (P-CBPAUP0C-005, P-CBPAUP0C-007, E-16); summary 2 is next. Columns
/// as `migrations/0004_authorizations.sql` defines them.
const PENDING_AUTHORIZATIONS: &str = r"
INSERT INTO pending_auth_summaries
    (pa_acct_id, pa_cust_id, pa_auth_status, pa_account_status, pa_credit_limit,
     pa_cash_limit, pa_credit_balance, pa_cash_balance, pa_approved_auth_cnt,
     pa_declined_auth_cnt, pa_approved_auth_amt, pa_declined_auth_amt)
VALUES (1, 1, 'A', ARRAY['', '', '', '', ''], 2020.00, 1020.00, 30.00, 0.00, 2, 0, 30.00, 0.00),
       (2, 2, 'A', ARRAY['', '', '', '', ''], 6100.00, 5200.00, 5.00, 0.00, 1, 0, 5.00, 0.00);
INSERT INTO pending_auth_details
    (pa_acct_id, pa_auth_date_9c, pa_auth_time_9c, pa_auth_orig_date, pa_auth_orig_time,
     pa_card_num, pa_auth_type, pa_card_expiry_date, pa_message_type, pa_message_source,
     pa_auth_id_code, pa_auth_resp_code, pa_auth_resp_reason, pa_processing_code,
     pa_transaction_amt, pa_approved_amt, pa_merchant_catagory_code, pa_acqr_country_code,
     pa_pos_entry_mode, pa_merchant_id, pa_merchant_name, pa_merchant_city,
     pa_merchant_state, pa_merchant_zip, pa_transaction_id, pa_match_status, pa_auth_fraud,
     pa_fraud_rpt_date)
VALUES (1, 77819, 879999999, '220629', '120000', '9680294154603697', 'PUR', '0525', '0100',
        'POS', 'A00001', '00', '0000', 0, 10.00, 10.00, '5411', '840', 1, '800000000',
        'Abshire-Lowe', 'Seattle', 'WA', '98101', 'T00000000000001', 'P', '', ''),
       (1, 77819, 879999998, '220629', '120001', '9680294154603697', 'PUR', '0525', '0100',
        'POS', 'A00002', '00', '0000', 0, 20.00, 20.00, '5411', '840', 1, '800000000',
        'Abshire-Lowe', 'Seattle', 'WA', '98101', 'T00000000000002', 'P', '', ''),
       (2, 77819, 879999999, '220629', '120000', '0923877193247330', 'PUR', '0525', '0100',
        'POS', 'A00003', '00', '0000', 0, 5.00, 5.00, '5411', '840', 1, '800000000',
        'Abshire-Lowe', 'Seattle', 'WA', '98101', 'T00000000000003', 'P', '', '');
";

/// The trigger that hides the details once a summary is deleted, so the next summary's `GNP`
/// fails on the server.
const HIDE_DETAILS_AFTER_SUMMARY_DELETE: &str = r"
CREATE FUNCTION hide_details() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'ALTER TABLE pending_auth_details RENAME TO pending_auth_details_offline';
    RETURN NULL;
END
$$;
CREATE TRIGGER hide_details AFTER DELETE ON pending_auth_summaries
    FOR EACH STATEMENT EXECUTE FUNCTION hide_details();
";

/// The text of every row of `sql`, a null as `NULL`.
fn text_rows(h: &Harness, sql: &str) -> Vec<Vec<String>> {
    block_on(h.stack().fetch_text_rows(sql))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| value.unwrap_or_else(|| "NULL".to_owned()))
                .collect()
        })
        .collect()
}

/// The lines of a child's output stream.
fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Whether stdout holds the persistence boundary's `warn` event for a statement of
/// `repository` that failed with SQLSTATE `42P01`.
fn logs_undefined_table(stdout: &[String], repository: &str) -> bool {
    let filepath_end = format!("{repository}\"");
    stdout.iter().any(|line| {
        line.contains(r#""level":"WARN""#)
            && line.contains(r#""message":"database statement failed""#)
            && line.contains(r#""sqlstate":"42P01""#)
            && line.contains(r#""code.filepath":""#)
            && line.contains(&filepath_end)
    })
}

/// One INTCALC case on a fresh database, after POSTTRAN.
fn run_intcalc_case(case: &IntcalcCase) {
    let doc = parity();
    let h = Harness::new();
    let posttran_exit: i32 = expected_value(&doc, POSTTRAN_COUNTS, "exit")
        .parse()
        .expect("P-CBTRN02C-065 gives a numeric exit code");
    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), posttran_exit);
    block_on(h.stack().exec_sql(case.trigger));
    let before = text_rows(&h, ACCOUNTS_SQL);

    let output = h.run_expect(&INTCALC, Some(TRANREPT_CLOCK), ABEND_EXIT);

    // P-CBACT04C-010: account 1's rewrite before the failed read persists.
    let audited: Vec<String> = [
        "account_id",
        "acct_curr_bal",
        "acct_curr_cyc_credit",
        "acct_curr_cyc_debit",
    ]
    .iter()
    .map(|key| expected_value(&doc, ACCOUNT_1_REWRITE, key))
    .collect();
    let account_1 = text_rows(&h, ACCOUNT_1_SQL);
    assert_eq!(account_1.len(), 1, "{account_1:?}");
    assert_eq!(
        account_1[0][0].parse::<u64>().ok(),
        audited[0].parse::<u64>().ok(),
        "the audited row is account 1"
    );
    assert_eq!(
        account_1[0][1..],
        audited[1..],
        "{}: account 1 keeps its interest and zeroed cycle amounts",
        case.display
    );
    let after = text_rows(&h, ACCOUNTS_SQL);
    assert_eq!(after.len(), before.len(), "{}", case.display);
    assert_eq!(
        after[1..],
        before[1..],
        "{}: accounts 2 to 50",
        case.display
    );
    assert!(
        h.generations("SYSTRAN").is_empty(),
        "{}: SYSTRAN(+1) must stay uncataloged",
        case.display
    );

    // P-CBACT04C-188: the paragraph's DISPLAY lines, then ABEND 999.
    let stderr = lines(&output.stderr);
    assert_eq!(
        stderr.first().map(String::as_str),
        Some(case.display),
        "{stderr:?}"
    );
    assert!(
        stderr.iter().any(|line| line == ABENDING_PROGRAM),
        "{stderr:?}"
    );
    assert_eq!(
        stderr.last().map(String::as_str),
        Some("ABEND 999"),
        "{stderr:?}"
    );
    assert!(
        logs_undefined_table(&lines(&output.stdout), case.repository),
        "{}: no 42P01 event from {} on stdout",
        case.display,
        case.repository
    );
}

#[test]
fn intcalc_keeps_the_account_rewrite_made_before_a_failed_read() {
    // P-CBACT04C-010, P-CBACT04C-188, P-CBACT04C-002, P-CBACT04C-004.
    for case in &INTCALC_CASES {
        run_intcalc_case(case);
    }
}

#[test]
fn cbpaup0j_keeps_the_deletes_made_before_a_failed_detail_read() {
    // P-CBPAUP0C-005, P-CBPAUP0C-009.
    let h = Harness::new();
    block_on(h.stack().exec_sql(PENDING_AUTHORIZATIONS));
    block_on(h.stack().exec_sql(HIDE_DETAILS_AFTER_SUMMARY_DELETE));

    let output = h.run_expect(&CBPAUP0J, Some(TRANREPT_CLOCK), ABEND_EXIT);

    assert_eq!(
        text_rows(
            &h,
            "SELECT pa_acct_id FROM pending_auth_summaries ORDER BY pa_acct_id"
        ),
        [["2"]],
        "summary 1 is deleted and summary 2 remains"
    );
    assert_eq!(
        text_rows(
            &h,
            "SELECT pa_acct_id, count(*) FROM pending_auth_details_offline \
             GROUP BY pa_acct_id ORDER BY pa_acct_id"
        ),
        [["2", "1"]],
        "summary 1's details are deleted and summary 2's detail remains"
    );
    assert_eq!(lines(&output.stderr), [CBPAUP0C_ABENDING]);
    let stdout = lines(&output.stdout);
    assert!(
        stdout.iter().any(|line| {
            line.contains(r#""message":"pending authorization purge abended""#)
                && line.contains(r#""summaries_read":2"#)
                && line.contains(r#""summaries_deleted":1"#)
                && line.contains(r#""details_read":2"#)
                && line.contains(r#""details_deleted":2"#)
        }),
        "the abend event does not carry the counts of the committed work: {stdout:#?}"
    );
    assert!(
        logs_undefined_table(&stdout, "repos/authorizations.rs"),
        "no 42P01 event from the detail read on stdout: {stdout:#?}"
    );
}
