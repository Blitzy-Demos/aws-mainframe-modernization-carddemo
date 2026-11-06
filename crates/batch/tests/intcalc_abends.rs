//! INTCALC (`batch intcalc`) abends: the CBACT04C paths to `9999-ABEND-PROGRAM` that
//! `intcalc.rs` and `abend_write_persistence.rs` do not reach, each provoked through the
//! harness and checked against its section B row of the parity audit.
//!
//! - Before any record is processed: the five opens in their COBOL order (P-CBACT04C-177 to
//!   P-CBACT04C-181, with the daily rejects text of E-72, P-CBACT04C-194), a failed read of the
//!   category balances (P-CBACT04C-182) and a missing `DEFAULT` disclosure group
//!   (P-CBACT04C-005). One seeded database serves every case: each hides a table, corrupts a
//!   balance record, renames the `DEFAULT` group or puts a file where the batch directory
//!   belongs, runs INTCALC, undoes its fault, and checks that no file and no row changed.
//! - After the end of the file: the four KSDS closes in their COBOL order (P-CBACT04C-183 to
//!   P-CBACT04C-186). The tables are hidden by a deferred trigger when the unit of work of the
//!   last account break commits, after the last account's reads, so the loop runs to its end
//!   (P-CBACT04C-182) and every account rewrite persists.
//! - The `SYSTRAN(+1)` write (P-CBACT04C-009) fails under a file size limit, after the
//!   account rewrites of the accounts before the failing record, which persist.
//!
//! The close and write cases run POSTTRAN first, as the README cycle does, so the accounts
//! can be held to the audited INTCALC results of section D.3. Every abend's stderr is the
//! paragraph's text from its row, the `9910-DISPLAY-IO-STATUS` line and `ABENDING PROGRAM` of
//! P-CBACT04C-188 with the status as P-CBACT04C-189 formats it, and `ABEND` with the audited
//! `ABCODE`; the exit code is 16 (AAP 0.5.3.5).

pub mod common;

use std::collections::BTreeMap;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use carddemo_audit::{Document, KeyValue, PARITY_COMMON_COLUMNS};
use carddemo_runtime::config::keys;
use carddemo_testkit::fixtures;
use common::{Harness, RUN_TIMEOUT, TRANREPT_CLOCK, block_on, expected_value, parity, parity_cell};

/// The INTCALC command of the JCL: `PARM='2022071800'` [app/jcl/INTCALC.jcl:L22].
const INTCALC: [&str; 3] = ["intcalc", "--parm-date", "2022071800"];

/// The exit code of an abend (AAP 0.5.3.5).
const ABEND_EXIT: i32 = 16;

/// The audit row of `9999-ABEND-PROGRAM`: its `ABENDING PROGRAM` display after the
/// `FILE STATUS IS: NNNN` line, and its `ABCODE`.
const ABEND_ROW: &str = "P-CBACT04C-188";

/// The audit row of POSTTRAN's counts and exit code over the fixture day.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// The `###` heading of the INTCALC fixture expectations.
const INTCALC_SUBSECTION: &str = "D.3 INTCALC";

/// The `Expected result` column of every parity audit data table.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// The generation dataset INTCALC writes, `SYSTRAN(+1)` [app/jcl/INTCALC.jcl:L37-41].
const SYSTRAN: &str = "SYSTRAN";

/// The generation POSTTRAN leaves in the batch directory before INTCALC runs.
const POSTTRAN_REJECTS: &str = "DALYREJS.G0001V00";

/// The file status the job shows for a table that does not answer and for a failed
/// `SYSTRAN(+1)` file operation: 30, a permanent error with no further information.
const NO_FURTHER_INFORMATION: &str = "30";

/// The status of an `INVALID KEY` read: record not found.
const NOT_FOUND: &str = "23";

/// The message of the `DISPLAY TRAN-CAT-BAL-RECORD` event, one per balance record read
/// (P-CBACT04C-182).
const BALANCE_RECORD_EVENT: &str = r#""message":"TRAN-CAT-BAL-RECORD""#;

/// The message of the event the job logs, with the record's TRAN-ID, when a `SYSTRAN(+1)`
/// write fails.
const SYSTRAN_WRITE_FAILED_EVENT: &str = r#""message":"SYSTRAN(+1) write failed""#;

/// How often [`run_with_file_size_limit`] checks whether its child has exited.
const POLL: Duration = Duration::from_millis(20);

/// The account CBACT04C rewrites last: the second-to-last account of the category balance
/// file, at the break to the last one, which is never rewritten (E-14). The close cases'
/// triggers name it in their SQL; [`last_rewritten_account`] holds it to section D.3.
const LAST_REWRITTEN_ACCOUNT: u64 = 49;

/// `TCATBALF` missing, and back.
const HIDE_TCATBAL: &str =
    "ALTER TABLE transaction_category_balances RENAME TO transaction_category_balances_offline";
const SHOW_TCATBAL: &str =
    "ALTER TABLE transaction_category_balances_offline RENAME TO transaction_category_balances";
/// `XREFFILE` missing, and back.
const HIDE_XREF: &str = "ALTER TABLE card_xref RENAME TO card_xref_offline";
const SHOW_XREF: &str = "ALTER TABLE card_xref_offline RENAME TO card_xref";
/// `DISCGRP` missing, and back.
const HIDE_DISCGRP: &str = "ALTER TABLE disclosure_groups RENAME TO disclosure_groups_offline";
const SHOW_DISCGRP: &str = "ALTER TABLE disclosure_groups_offline RENAME TO disclosure_groups";
/// `ACCTFILE` missing, and back.
const HIDE_ACCOUNTS: &str = "ALTER TABLE accounts RENAME TO accounts_offline";
const SHOW_ACCOUNTS: &str = "ALTER TABLE accounts_offline RENAME TO accounts";

/// A balance record whose `TRANCAT-CD` does not fit its `9(04)`, which no read of the file can
/// decode, after the first record of account 1 in key order. The column check that keeps such
/// a value out is dropped first.
const CORRUPT_TCATBAL: &str = r"
ALTER TABLE transaction_category_balances
    DROP CONSTRAINT transaction_category_balances_trancat_cd_check;
INSERT INTO transaction_category_balances (trancat_acct_id, trancat_type_cd, trancat_cd, tran_cat_bal)
    VALUES (1, '01', 10000, 0.00);
";
const REPAIR_TCATBAL: &str = r"
DELETE FROM transaction_category_balances WHERE trancat_cd = 10000;
ALTER TABLE transaction_category_balances
    ADD CONSTRAINT transaction_category_balances_trancat_cd_check
    CHECK (trancat_cd >= 0 AND trancat_cd < 10000);
";

/// Every `DEFAULT` disclosure group moved to another group ID, so `1200-A-GET-DEFAULT-INT-RATE`
/// ends with status 23 for the first balance record, whose blank group ends with 23 too (E-06).
const HIDE_DEFAULT_GROUP: &str = "UPDATE disclosure_groups SET dis_acct_group_id = 'NODEFAULT' \
     WHERE dis_acct_group_id = 'DEFAULT'";
const SHOW_DEFAULT_GROUP: &str = "UPDATE disclosure_groups SET dis_acct_group_id = 'DEFAULT' \
     WHERE dis_acct_group_id = 'NODEFAULT'";

/// Every row of the four KSDS files CBACT04C reads, as text, in one order.
const FILES_SQL: &str = "SELECT 'account', a::text FROM accounts a \
     UNION ALL SELECT 'balance', b::text FROM transaction_category_balances b \
     UNION ALL SELECT 'group', g::text FROM disclosure_groups g \
     UNION ALL SELECT 'xref', x::text FROM card_xref x \
     ORDER BY 1, 2";

/// Every account's balance and cycle amounts, in key order.
const ACCOUNTS_SQL: &str = "SELECT account_id::text, acct_curr_bal::text, \
     acct_curr_cyc_credit::text, acct_curr_cyc_debit::text FROM accounts ORDER BY account_id";

/// The trigger that hides the tables of a close case once the last account break commits:
/// the rewrite of [`LAST_REWRITTEN_ACCOUNT`] marks a row, and the deferred trigger on the mark
/// runs the case's `hide_at_commit()` at the commit of that record's unit of work, after the
/// last account's account, cross-reference and disclosure group reads. The last account's
/// later balance records read only the disclosure groups the run already holds, so the next
/// statement on the hidden tables is the closes'.
const HIDE_AT_LAST_BREAK_COMMIT: &str = r"
CREATE TABLE last_break_mark (marked boolean NOT NULL);
CREATE FUNCTION mark_last_break() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO last_break_mark VALUES (true);
    RETURN NULL;
END
$$;
CREATE TRIGGER mark_last_break AFTER UPDATE ON accounts
    FOR EACH ROW WHEN (OLD.account_id = 49) EXECUTE FUNCTION mark_last_break();
CREATE CONSTRAINT TRIGGER hide_at_commit AFTER INSERT ON last_break_mark
    DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hide_at_commit();
";

/// The `hide_at_commit()` of each close case: the tables it hides.
const HIDE_TCATBAL_AND_XREF_AT_COMMIT: &str = r"
CREATE FUNCTION hide_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'ALTER TABLE transaction_category_balances RENAME TO transaction_category_balances_offline';
    EXECUTE 'ALTER TABLE card_xref RENAME TO card_xref_offline';
    RETURN NULL;
END
$$;
";
const HIDE_XREF_AND_DISCGRP_AT_COMMIT: &str = r"
CREATE FUNCTION hide_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'ALTER TABLE card_xref RENAME TO card_xref_offline';
    EXECUTE 'ALTER TABLE disclosure_groups RENAME TO disclosure_groups_offline';
    RETURN NULL;
END
$$;
";
const HIDE_DISCGRP_AND_ACCOUNTS_AT_COMMIT: &str = r"
CREATE FUNCTION hide_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'ALTER TABLE disclosure_groups RENAME TO disclosure_groups_offline';
    EXECUTE 'ALTER TABLE accounts RENAME TO accounts_offline';
    RETURN NULL;
END
$$;
";
const HIDE_ACCOUNTS_AT_COMMIT: &str = r"
CREATE FUNCTION hide_at_commit() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE 'ALTER TABLE accounts RENAME TO accounts_offline';
    RETURN NULL;
END
$$;
";

/// What a case breaks before INTCALC runs, and undoes afterwards.
#[derive(Debug, Clone, Copy)]
enum Fault {
    /// `TCATBALF` is missing.
    HideTcatbal,
    /// `XREFFILE` is missing.
    HideXref,
    /// `DISCGRP` is missing.
    HideDiscgrp,
    /// `ACCTFILE` is missing.
    HideAccounts,
    /// A file stands where the batch directory belongs, so `SYSTRAN(+1)` cannot be allocated.
    BatchDirIsFile,
    /// `TCATBALF` holds a record no read can decode ([`CORRUPT_TCATBAL`]).
    CorruptTcatbal,
    /// `DISCGRP` has no `DEFAULT` group ([`HIDE_DEFAULT_GROUP`]).
    NoDefaultGroup,
}

impl Fault {
    /// Breaks what the fault names.
    fn apply(self, h: &Harness) {
        let dir = h.batch_dir();
        if let Some((break_sql, _)) = self.sql() {
            block_on(h.stack().exec_sql(break_sql));
        } else {
            fs::remove_dir_all(dir)
                .and_then(|()| fs::write(dir, b""))
                .unwrap_or_else(|error| panic!("cannot replace {}: {error}", dir.display()));
        }
    }

    /// Undoes [`Fault::apply`].
    fn undo(self, h: &Harness) {
        let dir = h.batch_dir();
        if let Some((_, repair_sql)) = self.sql() {
            block_on(h.stack().exec_sql(repair_sql));
        } else {
            fs::remove_file(dir)
                .and_then(|()| fs::create_dir(dir))
                .unwrap_or_else(|error| panic!("cannot restore {}: {error}", dir.display()));
        }
    }

    /// The statements that break and repair a database fault; `None` for the batch directory.
    fn sql(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::HideTcatbal => Some((HIDE_TCATBAL, SHOW_TCATBAL)),
            Self::HideXref => Some((HIDE_XREF, SHOW_XREF)),
            Self::HideDiscgrp => Some((HIDE_DISCGRP, SHOW_DISCGRP)),
            Self::HideAccounts => Some((HIDE_ACCOUNTS, SHOW_ACCOUNTS)),
            Self::CorruptTcatbal => Some((CORRUPT_TCATBAL, REPAIR_TCATBAL)),
            Self::NoDefaultGroup => Some((HIDE_DEFAULT_GROUP, SHOW_DEFAULT_GROUP)),
            Self::BatchDirIsFile => None,
        }
    }
}

/// An abend before any record is processed.
struct EarlyCase {
    /// The audit rows whose `Expected result` names the text the abend displays first.
    rows: &'static [&'static str],
    /// What the case breaks. With two faults, the first file in the COBOL order abends.
    faults: &'static [Fault],
    /// The two status bytes `9910-DISPLAY-IO-STATUS` shows.
    status: &'static str,
}

/// Each open alone, the opens in pairs, a failed read and a missing `DEFAULT` group.
const EARLY_CASES: [EarlyCase; 11] = [
    EarlyCase {
        rows: &["P-CBACT04C-177"],
        faults: &[Fault::HideTcatbal],
        status: NO_FURTHER_INFORMATION,
    },
    EarlyCase {
        rows: &["P-CBACT04C-178"],
        faults: &[Fault::HideXref],
        status: NO_FURTHER_INFORMATION,
    },
    EarlyCase {
        rows: &["P-CBACT04C-179", "P-CBACT04C-194"],
        faults: &[Fault::HideDiscgrp],
        status: NO_FURTHER_INFORMATION,
    },
    EarlyCase {
        rows: &["P-CBACT04C-180"],
        faults: &[Fault::HideAccounts],
        status: NO_FURTHER_INFORMATION,
    },
    EarlyCase {
        rows: &["P-CBACT04C-181"],
        faults: &[Fault::BatchDirIsFile],
        status: NO_FURTHER_INFORMATION,
    },
    // The order of the opens: TCATBALF, XREFFILE, DISCGRP, ACCTFILE, then TRANSACT.
    EarlyCase {
        rows: &["P-CBACT04C-177"],
        faults: &[Fault::HideTcatbal, Fault::HideXref],
        status: NO_FURTHER_INFORMATION,
    },
    EarlyCase {
        rows: &["P-CBACT04C-178"],
        faults: &[Fault::HideXref, Fault::HideDiscgrp],
        status: NO_FURTHER_INFORMATION,
    },
    EarlyCase {
        rows: &["P-CBACT04C-179", "P-CBACT04C-194"],
        faults: &[Fault::HideDiscgrp, Fault::HideAccounts],
        status: NO_FURTHER_INFORMATION,
    },
    EarlyCase {
        rows: &["P-CBACT04C-180"],
        faults: &[Fault::HideAccounts, Fault::BatchDirIsFile],
        status: NO_FURTHER_INFORMATION,
    },
    // The files open, then the first read of the category balances fails.
    EarlyCase {
        rows: &["P-CBACT04C-182"],
        faults: &[Fault::CorruptTcatbal],
        status: NO_FURTHER_INFORMATION,
    },
    // The first balance record's DEFAULT read ends with status 23.
    EarlyCase {
        rows: &["P-CBACT04C-005"],
        faults: &[Fault::NoDefaultGroup],
        status: NOT_FOUND,
    },
];

/// A close that fails after the end of the file.
struct CloseCase {
    /// The audit row of the close that abends: the first hidden file in the COBOL order.
    row: &'static str,
    /// The `hide_at_commit()` of the case, for [`HIDE_AT_LAST_BREAK_COMMIT`].
    hide_at_commit: &'static str,
    /// The tables it hides, restored after the run.
    hidden: &'static [Fault],
}

/// The closes in their COBOL order: each pair abends at its first file, and `ACCTFILE`, the
/// last KSDS closed, abends alone.
const CLOSE_CASES: [CloseCase; 4] = [
    CloseCase {
        row: "P-CBACT04C-183",
        hide_at_commit: HIDE_TCATBAL_AND_XREF_AT_COMMIT,
        hidden: &[Fault::HideTcatbal, Fault::HideXref],
    },
    CloseCase {
        row: "P-CBACT04C-184",
        hide_at_commit: HIDE_XREF_AND_DISCGRP_AT_COMMIT,
        hidden: &[Fault::HideXref, Fault::HideDiscgrp],
    },
    CloseCase {
        row: "P-CBACT04C-185",
        hide_at_commit: HIDE_DISCGRP_AND_ACCOUNTS_AT_COMMIT,
        hidden: &[Fault::HideDiscgrp, Fault::HideAccounts],
    },
    CloseCase {
        row: "P-CBACT04C-186",
        hide_at_commit: HIDE_ACCOUNTS_AT_COMMIT,
        hidden: &[Fault::HideAccounts],
    },
];

/// A D.3 account row: whether `1050-UPDATE-ACCOUNT` rewrites the account, and its balance and
/// cycle amounts after INTCALC.
#[derive(Debug)]
struct AuditedAccount {
    updated: bool,
    amounts: [String; 3],
}

/// The INTCALC expectations of section D.3 the abends are held to.
struct Expectations {
    /// The account rows, by `ACCT-ID`.
    accounts: BTreeMap<u64, AuditedAccount>,
    /// The account of each balance row, in key order: the order CBACT04C reads them in.
    balance_accounts: Vec<u64>,
    /// The account of each balance row with an interest record, by its `TRAN-ID`.
    interest_accounts: BTreeMap<String, u64>,
}

impl Expectations {
    /// Reads the balance and account rows of section D.3.
    fn from_audit(doc: &Document) -> Self {
        let mut expectations = Self {
            accounts: BTreeMap::new(),
            balance_accounts: Vec::new(),
            interest_accounts: BTreeMap::new(),
        };
        for (id, pairs) in d3_rows(doc) {
            let has = |key: &str| pairs.iter().any(|pair| pair.key == key);
            let get = |key: &str| value(&pairs, key, &id);
            if has("rate") {
                let account_id = number(get("account_id"), &id);
                expectations.balance_accounts.push(account_id);
                if has("tran_id") {
                    expectations
                        .interest_accounts
                        .insert(get("tran_id").to_owned(), account_id);
                }
            } else if has("total_interest") {
                let account_id = number(get("account_id"), &id);
                let row = AuditedAccount {
                    updated: get("updated").starts_with("yes"),
                    amounts: [
                        get("acct_curr_bal").to_owned(),
                        get("acct_curr_cyc_credit").to_owned(),
                        get("acct_curr_cyc_debit").to_owned(),
                    ],
                };
                assert!(
                    expectations.accounts.insert(account_id, row).is_none(),
                    "{id}: a second row for account {account_id}"
                );
            }
        }
        assert!(
            !expectations.accounts.is_empty() && !expectations.balance_accounts.is_empty(),
            "section D.3 holds no account or balance rows"
        );
        expectations
    }
}

/// Every data row of the D.3 subsection, with the `key=value` pairs of its `Expected result`.
fn d3_rows(doc: &Document) -> Vec<(String, Vec<KeyValue>)> {
    doc.tables()
        .iter()
        .filter(|table| {
            table.is_data_table() && table.subsection.as_deref() == Some(INTCALC_SUBSECTION)
        })
        .flat_map(|table| {
            table.rows.iter().map(move |row| {
                let id = table.cell(row, "ID").unwrap_or_default().to_owned();
                let expected = table
                    .cell(row, EXPECTED_RESULT)
                    .unwrap_or_else(|| panic!("{id} has no {EXPECTED_RESULT:?} cell"));
                (id, carddemo_audit::key_values(expected))
            })
        })
        .collect()
}

/// The value of the pair `key` of row `id`.
fn value<'p>(pairs: &'p [KeyValue], key: &str, id: &str) -> &'p str {
    pairs.iter().find(|pair| pair.key == key).map_or_else(
        || panic!("{id} expects no {key}="),
        |pair| pair.value.as_str(),
    )
}

/// `text` as an unsigned number, for example an 11-digit `ACCT-ID`.
fn number(text: &str, at: &str) -> u64 {
    text.parse()
        .unwrap_or_else(|error| panic!("{at}: {text:?} is not a number: {error}"))
}

/// The texts of the code spans of row `id`'s `Expected result`, in order, without the quotes
/// that delimit each COBOL literal.
fn audited_texts(doc: &Document, id: &str) -> Vec<String> {
    carddemo_audit::cell_code_spans(parity_cell(doc, id, EXPECTED_RESULT))
        .into_iter()
        .map(|span| {
            span.text
                .strip_prefix('"')
                .and_then(|text| text.strip_suffix('"'))
                .unwrap_or_else(|| panic!("{id}: {:?} is not a quoted literal", span.text))
                .to_owned()
        })
        .collect()
}

/// The stderr of an abend whose paragraph displays `text` and then status `status`:
/// `text`, the `9910-DISPLAY-IO-STATUS` line, `ABENDING PROGRAM` and `ABEND` with the code of
/// [`ABEND_ROW`].
///
/// A numeric status whose first digit is not 9 shows as `00` and its two digits
/// (P-CBACT04C-189).
fn abend_stderr(doc: &Document, text: String, status: &str) -> Vec<String> {
    assert!(
        status.len() == 2
            && status.bytes().all(|byte| byte.is_ascii_digit())
            && !status.starts_with('9'),
        "{status} is not a numeric status below 90"
    );
    let abend = audited_texts(doc, ABEND_ROW);
    assert_eq!(
        abend.len(),
        2,
        "{ABEND_ROW} names ABENDING PROGRAM and the status line"
    );
    let abcode = expected_value(doc, ABEND_ROW, "ABCODE");
    vec![
        text,
        format!("{}00{status}", abend[1]),
        abend[0].clone(),
        format!("ABEND {abcode}"),
    ]
}

/// The text the paragraph of row `id` displays first: the row's first literal, followed by the
/// file status where the row shows `+{XREFFILE-STATUS}` right after it (P-CBACT04C-178).
fn first_display(doc: &Document, id: &str, status: &str) -> String {
    let text = audited_texts(doc, id)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{id} names no text"));
    let with_status = format!("`\"{text}\"`+{{XREFFILE-STATUS}}");
    if parity_cell(doc, id, EXPECTED_RESULT).contains(&with_status) {
        format!("{text}{status}")
    } else {
        text
    }
}

/// The lines of a child's output stream.
fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The names in the batch directory, sorted; none when it does not exist.
fn batch_entries(dir: &Path) -> Vec<String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("cannot list {}: {error}", dir.display()),
    };
    let mut names: Vec<String> = entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort_unstable();
    names
}

/// The rows of `sql` as text, a null as `NULL`.
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

/// Every account's balance and cycle amounts, by `ACCT-ID`.
fn account_amounts(h: &Harness) -> BTreeMap<u64, [String; 3]> {
    text_rows(h, ACCOUNTS_SQL)
        .into_iter()
        .map(|row| match <[String; 4]>::try_from(row) {
            Ok([id, balance, credit, debit]) => (number(&id, "accounts"), [balance, credit, debit]),
            Err(row) => panic!("an account row of {} columns: {row:?}", row.len()),
        })
        .collect()
}

/// Runs POSTTRAN over the fixture day and checks its audited exit code.
fn posttran(h: &Harness, doc: &Document) {
    let exit: i32 = expected_value(doc, POSTTRAN_COUNTS, "exit")
        .parse()
        .expect("P-CBTRN02C-065 gives a numeric exit code");
    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), exit);
}

/// Checks that [`LAST_REWRITTEN_ACCOUNT`], the account the close cases' trigger names, is the
/// last account section D.3 rewrites, and that the account after it in key order, the last
/// one, is not rewritten (E-14).
fn assert_last_rewritten_account(expected: &Expectations) {
    let rewritten: Vec<u64> = expected
        .accounts
        .iter()
        .filter(|(_, row)| row.updated)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        rewritten.last(),
        Some(&LAST_REWRITTEN_ACCOUNT),
        "the close cases' trigger names the last rewritten account"
    );
    assert_eq!(
        expected.balance_accounts.last(),
        Some(&(LAST_REWRITTEN_ACCOUNT + 1)),
        "the last account of the file follows the last rewritten one"
    );
}

/// One case of [`EARLY_CASES`] on `h`, whose files and rows are `files` before the run.
fn run_early_case(h: &Harness, doc: &Document, case: &EarlyCase, files: &[Vec<String>]) {
    let displays: Vec<String> = case
        .rows
        .iter()
        .map(|id| first_display(doc, id, case.status))
        .collect();
    assert!(
        displays.windows(2).all(|pair| pair[0] == pair[1]),
        "{:?} name different texts: {displays:?}",
        case.rows
    );
    for fault in case.faults {
        fault.apply(h);
    }
    let output = h.run_expect(&INTCALC, Some(TRANREPT_CLOCK), ABEND_EXIT);
    for fault in case.faults.iter().rev() {
        fault.undo(h);
    }
    let label = format!("{:?} with {:?}", case.rows, case.faults);
    assert_eq!(
        lines(&output.stderr),
        abend_stderr(doc, displays[0].clone(), case.status),
        "{label}"
    );
    assert_eq!(
        text_rows(h, FILES_SQL),
        files,
        "{label}: no record is written before the abend"
    );
    assert!(
        batch_entries(h.batch_dir()).is_empty(),
        "{label}: SYSTRAN(+1) is not cataloged and leaves no file: {:?}",
        batch_entries(h.batch_dir())
    );
}

#[test]
fn failed_opens_reads_and_default_group_abend_before_any_record_is_processed() {
    // P-CBACT04C-177 to P-CBACT04C-182, P-CBACT04C-194 and P-CBACT04C-005, in one seeded
    // database: each case leaves it as it found it.
    let doc = parity();
    let h = Harness::new();
    let files = text_rows(&h, FILES_SQL);
    for case in &EARLY_CASES {
        run_early_case(&h, &doc, case, &files);
    }
}

/// One case of [`CLOSE_CASES`] on a fresh database after POSTTRAN.
fn run_close_case(doc: &Document, expected: &Expectations, case: &CloseCase) {
    let h = Harness::new();
    posttran(&h, doc);
    block_on(h.stack().exec_sql(case.hide_at_commit));
    block_on(h.stack().exec_sql(HIDE_AT_LAST_BREAK_COMMIT));

    let output = h.run_expect(&INTCALC, Some(TRANREPT_CLOCK), ABEND_EXIT);
    for fault in case.hidden.iter().rev() {
        fault.undo(&h);
    }

    let text = first_display(doc, case.row, NO_FURTHER_INFORMATION);
    assert_eq!(
        lines(&output.stderr),
        abend_stderr(doc, text, NO_FURTHER_INFORMATION),
        "{}",
        case.row
    );
    // P-CBACT04C-182: every balance record was read, counted and displayed in key order, and
    // the end of the file ended the loop without updating the last account (E-14).
    let read: Vec<u64> = lines(&output.stdout)
        .iter()
        .filter(|line| line.contains(BALANCE_RECORD_EVENT))
        .map(|line| number(json_string(line, "trancat_acct_id"), case.row))
        .collect();
    assert_eq!(read, expected.balance_accounts, "{}", case.row);
    // P-CBACT04C-010: every rewrite stays, the last account keeps its amounts.
    let audited: BTreeMap<u64, [String; 3]> = expected
        .accounts
        .iter()
        .map(|(id, row)| (*id, row.amounts.clone()))
        .collect();
    assert_eq!(account_amounts(&h), audited, "{}", case.row);
    assert!(h.generations(SYSTRAN).is_empty(), "{}", case.row);
    assert_eq!(
        batch_entries(h.batch_dir()),
        [POSTTRAN_REJECTS],
        "{}: SYSTRAN(+1) leaves no file",
        case.row
    );
}

#[test]
fn failed_closes_abend_after_the_loop_and_keep_every_account_rewrite() {
    // P-CBACT04C-183 to P-CBACT04C-186, P-CBACT04C-182.
    let doc = parity();
    let expected = Expectations::from_audit(&doc);
    assert_last_rewritten_account(&expected);
    for case in &CLOSE_CASES {
        run_close_case(&doc, &expected, case);
    }
}

/// The JSON string value of `key` in the event line `line`.
fn json_string<'l>(line: &'l str, key: &str) -> &'l str {
    let head = format!("\"{key}\":\"");
    line.split_once(&head)
        .and_then(|(_, rest)| rest.split_once('"'))
        .map_or_else(|| panic!("no {key} string in {line}"), |(value, _)| value)
}

/// Runs `batch <args>` with the environment [`Harness::run`] gives the binary, under a file
/// size limit of 512 bytes with `SIGXFSZ` ignored, so a write that would grow a file beyond
/// 512 bytes fails with `EFBIG`: a write that ends with a status other than 00. `/bin/sh` sets
/// both and `exec`s the binary. The run is bounded by [`RUN_TIMEOUT`]; a child still running
/// then is killed and reaped, and the run panics.
///
/// An instrumented binary (`cargo llvm-cov`) writes its profile at exit to the file
/// `LLVM_PROFILE_FILE` names, and the size limit would fail that write and add an
/// `LLVM Profile Error` line to stderr. The child's profile therefore goes to `/dev/null`, a
/// device the size limit does not apply to, so its stderr holds only the job's lines. A binary
/// built without instrumentation ignores the variable.
fn run_with_file_size_limit(h: &Harness, args: &[&str], clock: &str) -> Output {
    let program = env!("CARGO_BIN_EXE_batch");
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(r#"trap '' XFSZ; ulimit -f 1; exec "$0" "$@""#)
        .arg(program)
        .args(args)
        .current_dir(fixtures::repo_root())
        .env(keys::DATABASE_URL, h.stack().database_url())
        .env(keys::BATCH_DIR, h.batch_dir())
        .env(keys::STATEMENT_DIR, h.statement_dir())
        .env(keys::FIXTURE_DIR, fixtures::ascii_dir())
        .env(keys::CLOCK_OVERRIDE, clock)
        .env("LLVM_PROFILE_FILE", "/dev/null")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("cannot run {program} {args:?} through sh: {error}"));
    let stdout = drain(child.stdout.take(), "stdout");
    let stderr = drain(child.stderr.take(), "stderr");
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < RUN_TIMEOUT => thread::sleep(POLL),
            outcome => {
                let killed = child.kill();
                let reaped = child.wait();
                panic!(
                    "batch {args:?} under a file size limit did not exit within {RUN_TIMEOUT:?} \
                     ({outcome:?}); kill {killed:?}, reap {reaped:?}"
                );
            }
        }
    };
    Output {
        status,
        stdout: joined(stdout, "stdout"),
        stderr: joined(stderr, "stderr"),
    }
}

/// Reads the child's stream `name` to its end on a thread of its own.
fn drain<R: Read + Send + 'static>(pipe: Option<R>, name: &'static str) -> JoinHandle<Vec<u8>> {
    let mut pipe = pipe.unwrap_or_else(|| panic!("the child has no {name} pipe"));
    thread::spawn(move || {
        let mut bytes = Vec::new();
        pipe.read_to_end(&mut bytes)
            .unwrap_or_else(|error| panic!("cannot read the child's {name}: {error}"));
        bytes
    })
}

/// The bytes [`drain`] read from the child's stream `name`.
fn joined(reader: JoinHandle<Vec<u8>>, name: &str) -> Vec<u8> {
    reader
        .join()
        .unwrap_or_else(|_| panic!("the {name} reader panicked"))
}

#[test]
fn a_failed_systran_write_abends_and_keeps_the_rewrites_before_it() {
    // P-CBACT04C-009: 1300-B-WRITE-TX fails on the file size limit. The rewrites of the
    // accounts before the failing record's account persist (P-CBACT04C-010), and
    // DISP=(NEW,CATLG,DELETE) deletes SYSTRAN(+1).
    let doc = parity();
    let expected = Expectations::from_audit(&doc);
    let h = Harness::new();
    posttran(&h, &doc);
    let before = account_amounts(&h);

    let output = run_with_file_size_limit(&h, &INTCALC, TRANREPT_CLOCK);

    assert_eq!(output.status.code(), Some(ABEND_EXIT), "{output:?}");
    let text = first_display(&doc, "P-CBACT04C-009", NO_FURTHER_INFORMATION);
    assert_eq!(
        lines(&output.stderr),
        abend_stderr(&doc, text, NO_FURTHER_INFORMATION)
    );
    let stdout = lines(&output.stdout);
    let failed = stdout
        .iter()
        .find(|line| line.contains(SYSTRAN_WRITE_FAILED_EVENT))
        .map_or_else(
            || panic!("no SYSTRAN write failure event: {stdout:#?}"),
            |line| json_string(line, "tran_id"),
        );
    let failing_account = *expected
        .interest_accounts
        .get(failed)
        .unwrap_or_else(|| panic!("section D.3 has no interest record {failed}"));
    assert!(
        failing_account > 1,
        "the write fails after some account is rewritten, not at account {failing_account}"
    );
    let after = account_amounts(&h);
    for (id, row) in &expected.accounts {
        let kept = if *id < failing_account {
            &row.amounts
        } else {
            &before[id]
        };
        assert_eq!(
            &after[id], kept,
            "account {id}, the write failing at {failed}"
        );
    }
    assert!(h.generations(SYSTRAN).is_empty());
    assert_eq!(batch_entries(h.batch_dir()), [POSTTRAN_REJECTS]);
}
