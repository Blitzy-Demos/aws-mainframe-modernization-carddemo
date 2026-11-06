//! POSTTRAN (`batch posttran`) abends before the main loop: the six CBTRN02C opens in their
//! COBOL order (P-CBTRN02C-004, P-CBTRN02C-221 to P-CBTRN02C-225), the first read of the daily
//! file (P-CBTRN02C-007), and the open of a transaction master that already holds records
//! (P-CBTRN02C-005, E-32 as P-CBTRN02C-219), each provoked through the harness and checked
//! against its parity audit row.
//!
//! Every case breaks one or two files, runs POSTTRAN, undoes what it broke, and checks the
//! abend and that nothing changed: no row of the files CBTRN02C reads or writes, no reward
//! accrual, and no generation in the batch directory, as `DISP=(NEW,CATLG,DELETE)` deletes an
//! uncataloged `DALYREJS(+1)`. A missing table is the open of a KSDS that fails; a missing
//! `--input` file the `DALYTRAN` open; a directory given as `--input`, which opens but cannot be
//! read, the `DALYTRAN` read; a file where the batch directory belongs the `DALYREJS(+1)` open.
//! With two files broken, the first in the COBOL order abends and the second is never reached.
//!
//! Every abend's stderr is the paragraph's text from its row, the `9910-DISPLAY-IO-STATUS` line
//! and `ABENDING PROGRAM` of `9999-ABEND-PROGRAM` (P-CBTRN02C-021), and `ABEND` with its
//! `CEE3ABD` code; the exit code is 16 (AAP 0.5.3.5).
//!
//! An `--input` whose records cannot all be read as `CVTRA06Y` records stops the run the same
//! way, before the main loop and changing nothing, as a setup failure (exit 16) naming the
//! record: an endless input without line ends, `/dev/zero`, at its first record, and a long file
//! whose last record does not decode at that record.

pub mod common;

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use carddemo_audit::{Document, PARITY_COMMON_COLUMNS};
use carddemo_codec::{CodecError, CodecErrorKind, DailyTranLayout, Layout, decode_ascii_file};
use common::{
    Harness, TRANREPT_CLOCK, block_on, expected_value, fixture_lines, parity, parity_cell,
};

/// The exit code of an abend (AAP 0.5.3.5).
const ABEND_EXIT: i32 = 16;

/// The audit row of `9999-ABEND-PROGRAM`: its `ABENDING PROGRAM` display after the
/// `FILE STATUS IS: NNNN` line, and its `CEE3ABD` code.
const ABEND_ROW: &str = "P-CBTRN02C-021";

/// The audit row of the six opens and their order.
const OPENS_ROW: &str = "P-CBTRN02C-004";

/// The audit row of the open of a loaded transaction master.
const LOADED_MASTER_ROW: &str = "P-CBTRN02C-005";

/// The section E row of the loaded master: escalation E-32 and its exit code.
const LOADED_MASTER_ESCALATION: &str = "P-CBTRN02C-219";

/// The audit row of POSTTRAN's counts and exit code over the fixture day.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// The `Expected result` column of every parity audit data table.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// The status `9910-DISPLAY-IO-STATUS` shows for a missing `--input` file: 35, file not present.
const NOT_PRESENT: &str = "35";

/// The status shown for a table that does not answer its open and for every other file-system
/// failure: 30, a permanent error with no further information.
const NO_FURTHER_INFORMATION: &str = "30";

/// The status of `OPEN OUTPUT` on a loaded KSDS defined without `REUSE` (E-32).
const NOT_REUSABLE: &str = "37";

/// The message of the end-of-job counts event of a run that read the whole file.
const COUNTS_EVENT: &str = r#""message":"end of job counts""#;

/// The `--input` a missing-file case names, under the statement directory.
const MISSING_INPUT: &str = "DALYTRAN.missing";

/// The prefix of the one stderr line of a setup failure.
const SETUP_FAILURE: &str = "SETUP FAILURE: ";

/// The copies of the fixture day the long-input case reads: 30 000 records.
const LONG_INPUT_DAYS: usize = 100;

/// The transaction master missing, and back.
const HIDE_MASTER: &str = "ALTER VIEW transaction_master RENAME TO transaction_master_offline";
const SHOW_MASTER: &str = "ALTER VIEW transaction_master_offline RENAME TO transaction_master";
/// `XREFFILE` missing, and back.
const HIDE_XREF: &str = "ALTER TABLE card_xref RENAME TO card_xref_offline";
const SHOW_XREF: &str = "ALTER TABLE card_xref_offline RENAME TO card_xref";
/// `ACCTFILE` missing, and back.
const HIDE_ACCOUNTS: &str = "ALTER TABLE accounts RENAME TO accounts_offline";
const SHOW_ACCOUNTS: &str = "ALTER TABLE accounts_offline RENAME TO accounts";
/// `TCATBALF` missing, and back.
const HIDE_TCATBAL: &str =
    "ALTER TABLE transaction_category_balances RENAME TO transaction_category_balances_offline";
const SHOW_TCATBAL: &str =
    "ALTER TABLE transaction_category_balances_offline RENAME TO transaction_category_balances";

/// Every row of the files CBTRN02C reads or writes and of the reward tables its accrual
/// writes, as text, in one order.
const FILES_SQL: &str = "SELECT 'account', a::text FROM accounts a \
     UNION ALL SELECT 'balance', b::text FROM transaction_category_balances b \
     UNION ALL SELECT 'master', m::text FROM transaction_master m \
     UNION ALL SELECT 'xref', x::text FROM card_xref x \
     UNION ALL SELECT 'reward balance', r::text FROM reward_balances r \
     UNION ALL SELECT 'reward accrual', r::text FROM reward_accruals r \
     ORDER BY 1, 2";

/// What a case breaks before POSTTRAN runs, and undoes afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fault {
    /// The `--input` file does not exist.
    MissingInput,
    /// The `--input` is a directory: it opens, and its first read fails.
    InputIsDirectory,
    /// The transaction master is missing.
    HideMaster,
    /// `XREFFILE` is missing.
    HideXref,
    /// A file stands where the batch directory belongs, so `DALYREJS(+1)` cannot be allocated.
    BatchDirIsFile,
    /// `ACCTFILE` is missing.
    HideAccounts,
    /// `TCATBALF` is missing.
    HideTcatbal,
}

impl Fault {
    /// The statements that break and repair a database fault.
    fn sql(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::HideMaster => Some((HIDE_MASTER, SHOW_MASTER)),
            Self::HideXref => Some((HIDE_XREF, SHOW_XREF)),
            Self::HideAccounts => Some((HIDE_ACCOUNTS, SHOW_ACCOUNTS)),
            Self::HideTcatbal => Some((HIDE_TCATBAL, SHOW_TCATBAL)),
            Self::MissingInput | Self::InputIsDirectory | Self::BatchDirIsFile => None,
        }
    }

    /// Breaks what the fault names; an `--input` fault changes only the command line.
    fn apply(self, h: &Harness) {
        if let Some((break_sql, _)) = self.sql() {
            block_on(h.stack().exec_sql(break_sql));
        } else if self == Self::BatchDirIsFile {
            let dir = h.batch_dir();
            fs::rename(dir, aside(dir))
                .and_then(|()| fs::write(dir, b""))
                .unwrap_or_else(|error| panic!("cannot replace {}: {error}", dir.display()));
        }
    }

    /// Undoes [`Fault::apply`].
    fn undo(self, h: &Harness) {
        if let Some((_, repair_sql)) = self.sql() {
            block_on(h.stack().exec_sql(repair_sql));
        } else if self == Self::BatchDirIsFile {
            let dir = h.batch_dir();
            fs::remove_file(dir)
                .and_then(|()| fs::rename(aside(dir), dir))
                .unwrap_or_else(|error| panic!("cannot restore {}: {error}", dir.display()));
        }
    }
}

/// Where [`Fault::BatchDirIsFile`] keeps the batch directory while a file stands in its place.
fn aside(dir: &Path) -> PathBuf {
    dir.with_extension("aside")
}

/// The `--input` of a case: the fixture default of the JCL, or the path an `--input` fault
/// names.
fn input_of(h: &Harness, faults: &[Fault]) -> Option<PathBuf> {
    faults.iter().find_map(|fault| match fault {
        Fault::MissingInput => Some(h.statement_dir().join(MISSING_INPUT)),
        Fault::InputIsDirectory => Some(h.statement_dir().to_path_buf()),
        _ => None,
    })
}

/// One step of CBTRN02C before its main loop that a case breaks.
struct Step {
    /// The DD the step opens or reads, as the opens of [`OPENS_ROW`] name it.
    dd: &'static str,
    /// What breaks it.
    fault: Fault,
    /// The audit row whose `Expected result` names the text its abend displays first.
    row: &'static str,
    /// The two status bytes `9910-DISPLAY-IO-STATUS` shows.
    status: &'static str,
}

/// The six opens in their COBOL order (`0000-DALYTRAN-OPEN` to `0500-TCATBALF-OPEN`), then the
/// first `1000-DALYTRAN-GET-NEXT` read. The transaction master's open shows the text of the
/// opens row, which names it.
const STEPS: [Step; 7] = [
    Step {
        dd: "DALYTRAN",
        fault: Fault::MissingInput,
        row: "P-CBTRN02C-221",
        status: NOT_PRESENT,
    },
    Step {
        dd: "TRANSACT",
        fault: Fault::HideMaster,
        row: OPENS_ROW,
        status: NO_FURTHER_INFORMATION,
    },
    Step {
        dd: "XREF",
        fault: Fault::HideXref,
        row: "P-CBTRN02C-222",
        status: NO_FURTHER_INFORMATION,
    },
    Step {
        dd: "DALYREJS",
        fault: Fault::BatchDirIsFile,
        row: "P-CBTRN02C-223",
        status: NO_FURTHER_INFORMATION,
    },
    Step {
        dd: "ACCTFILE",
        fault: Fault::HideAccounts,
        row: "P-CBTRN02C-224",
        status: NO_FURTHER_INFORMATION,
    },
    Step {
        dd: "TCATBALF",
        fault: Fault::HideTcatbal,
        row: "P-CBTRN02C-225",
        status: NO_FURTHER_INFORMATION,
    },
    Step {
        dd: "DALYTRAN read",
        fault: Fault::InputIsDirectory,
        row: "P-CBTRN02C-007",
        status: NO_FURTHER_INFORMATION,
    },
];

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

/// The stderr of the abend whose paragraph row is `id` and whose status is `status`: the row's
/// first literal, the `9910-DISPLAY-IO-STATUS` line, `ABENDING PROGRAM` and `ABEND` with the
/// `CEE3ABD` code of [`ABEND_ROW`].
///
/// A numeric status whose first digit is not 9 shows as `00` and its two digits (P-CBTRN02C-021);
/// a row that gives the digits after its status literal, as P-CBTRN02C-005 gives `0037`, must
/// give these.
fn abend_stderr(doc: &Document, id: &str, status: &str) -> Vec<String> {
    assert!(
        status.len() == 2
            && status.bytes().all(|byte| byte.is_ascii_digit())
            && !status.starts_with('9'),
        "{status} is not a numeric status below 90"
    );
    let text = audited_texts(doc, id)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{id} names no text"));
    let abend = audited_texts(doc, ABEND_ROW);
    assert_eq!(
        abend.len(),
        2,
        "{ABEND_ROW} names ABENDING PROGRAM and the status line"
    );
    let io_status_04 = format!("00{status}");
    let status_literal = format!("`\"{}\"`", abend[1]);
    if let Some((_, after)) = parity_cell(doc, id, EXPECTED_RESULT).split_once(&status_literal) {
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if !digits.is_empty() {
            assert_eq!(digits, io_status_04, "{id}: the audited file status");
        }
    }
    vec![
        text,
        format!("{}{io_status_04}", abend[1]),
        abend[0].clone(),
        format!("ABEND {}", abend_code(doc)),
    ]
}

/// The `CEE3ABD` code of [`ABEND_ROW`], `999`.
fn abend_code(doc: &Document) -> String {
    let cell = parity_cell(doc, ABEND_ROW, EXPECTED_RESULT);
    let code: String = cell
        .split_once("CEE3ABD code ")
        .map(|(_, after)| after.chars().take_while(char::is_ascii_digit).collect())
        .unwrap_or_default();
    assert!(
        !code.is_empty(),
        "{ABEND_ROW} gives no CEE3ABD code: {cell}"
    );
    code
}

/// The lines of a child's output stream.
fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// The rows of [`FILES_SQL`] as text, a null as `NULL`.
fn files(h: &Harness) -> Vec<Vec<String>> {
    block_on(h.stack().fetch_text_rows(FILES_SQL))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|value| value.unwrap_or_else(|| "NULL".to_owned()))
                .collect()
        })
        .collect()
}

/// Every file of the batch directory with its bytes, by name; none when it does not exist.
fn batch_contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Vec::new(),
        Err(error) => panic!("cannot list {}: {error}", dir.display()),
    };
    let mut contents: Vec<(String, Vec<u8>)> = entries
        .map(|entry| {
            let path = entry
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", dir.display()))
                .path();
            let bytes = fs::read(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, bytes)
        })
        .collect();
    contents.sort_unstable();
    contents
}

/// Breaks `faults`, runs POSTTRAN, undoes them, and checks that it exits with `exit` and the
/// abend of row `id` with status `status`, and that no row and no batch file changed.
fn run_case(h: &Harness, doc: &Document, faults: &[Fault], (id, status): (&str, &str), exit: i32) {
    let before = (files(h), batch_contents(h.batch_dir()));
    let input = input_of(h, faults);
    let mut args = vec!["posttran"];
    if let Some(path) = &input {
        args.push("--input");
        args.push(
            path.to_str()
                .expect("the test directories have UTF-8 paths"),
        );
    }
    for fault in faults {
        fault.apply(h);
    }
    let output = h.run_expect(&args, Some(TRANREPT_CLOCK), exit);
    for fault in faults.iter().rev() {
        fault.undo(h);
    }
    let label = format!("{id} with {faults:?}");
    assert_eq!(
        lines(&output.stderr),
        abend_stderr(doc, id, status),
        "{label}"
    );
    assert_eq!(
        (files(h), batch_contents(h.batch_dir())),
        before,
        "{label}: no file is read or written before the abend"
    );
}

/// Checks that [`STEPS`] opens the files in the order of the opens of [`OPENS_ROW`]: the DD
/// that begins each comma-separated open, commas inside parentheses excepted.
fn assert_open_order(doc: &Document) {
    let opens = expected_value(doc, OPENS_ROW, "opens");
    let mut audited = Vec::new();
    let mut open = String::new();
    let mut depth = 0_u32;
    for ch in opens.chars().chain([',']) {
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                if let Some(dd) = open.split_whitespace().next() {
                    audited.push(dd.to_owned());
                }
                open.clear();
                continue;
            }
            _ => {}
        }
        open.push(ch);
    }
    let steps: Vec<&str> = STEPS[..STEPS.len() - 1]
        .iter()
        .map(|step| step.dd)
        .collect();
    assert_eq!(steps, audited, "{OPENS_ROW}: {opens}");
}

#[test]
fn failed_opens_and_first_read_abend_in_cobol_order_and_change_nothing() {
    // P-CBTRN02C-004, P-CBTRN02C-007, P-CBTRN02C-021, P-CBTRN02C-221 to P-CBTRN02C-225, on
    // one seeded database with an empty live master: each case leaves it as it found it.
    let doc = parity();
    assert_open_order(&doc);
    let h = Harness::new();
    for (at, step) in STEPS.iter().enumerate() {
        run_case(&h, &doc, &[step.fault], (step.row, step.status), ABEND_EXIT);
        // With the next step broken too, this step still abends first.
        if let Some(next) = STEPS.get(at + 1) {
            let faults = [step.fault, next.fault];
            run_case(&h, &doc, &faults, (step.row, step.status), ABEND_EXIT);
        }
    }
}

#[test]
fn inputs_that_do_not_read_as_records_fail_before_any_posting_and_change_nothing() {
    // P-CBTRN02C-007: every record of DALYTRAN is read, one line at a time, and decoded before
    // the first one is processed. /dev/zero, which never ends and holds no line end, fails at
    // its first record once the 352 bytes of a record and a CR LF line end hold no LF; a long
    // file whose last record does not decode fails at that record. Neither posts a record,
    // accrues a reward or adds a generation.
    let h = Harness::new();
    let day = fixture_lines("dailytran.txt");
    let mut long_input = Vec::new();
    for _ in 0..LONG_INPUT_DAYS {
        for line in &day {
            long_input.extend_from_slice(line);
            long_input.push(b'\n');
        }
    }
    // DALYTRAN-AMT of the last record starts at byte 132 of its 350; a letter there is not a
    // zoned digit.
    let last_amount = long_input.len() - (DailyTranLayout::LEN + 1) + 132;
    long_input[last_amount] = b'X';
    let last_undecodable = decode_ascii_file::<DailyTranLayout>(&long_input)
        .expect_err("the last record does not decode");
    assert_eq!(
        last_undecodable.record,
        Some(LONG_INPUT_DAYS * day.len()),
        "{last_undecodable}"
    );
    let long_path = h.statement_dir().join("DALYTRAN.last-undecodable");
    fs::write(&long_path, &long_input)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", long_path.display()));
    let endless = CodecError::new(
        DailyTranLayout::NAME,
        CodecErrorKind::RecordTooLong {
            stored_len: DailyTranLayout::LEN + 2,
        },
    )
    .at_record(1);
    let cases = [
        ("/dev/zero", PathBuf::from("/dev/zero"), endless),
        ("a long file", long_path.clone(), last_undecodable),
    ];
    for (case, input, refused) in cases {
        let before = (files(&h), batch_contents(h.batch_dir()));
        let path = input
            .to_str()
            .expect("the test directories have UTF-8 paths");
        let output = h.run_expect(
            &["posttran", "--input", path],
            Some(TRANREPT_CLOCK),
            ABEND_EXIT,
        );
        assert_eq!(
            lines(&output.stderr),
            [format!("{SETUP_FAILURE}{refused}")],
            "{case}"
        );
        assert_eq!(
            (files(&h), batch_contents(h.batch_dir())),
            before,
            "{case}: no record is posted and no generation is added"
        );
    }
    fs::remove_file(&long_path)
        .unwrap_or_else(|error| panic!("cannot remove {}: {error}", long_path.display()));
}

/// The exit code of the loaded-master escalation row: `reproduced: exit <code>`.
fn loaded_master_exit(doc: &Document) -> i32 {
    assert_eq!(
        expected_value(doc, LOADED_MASTER_ESCALATION, "escalation"),
        "E-32"
    );
    let disposition = expected_value(doc, LOADED_MASTER_ESCALATION, "disposition");
    disposition
        .strip_prefix("reproduced: exit ")
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("{LOADED_MASTER_ESCALATION}: {disposition}"))
}

#[test]
fn a_loaded_live_master_abends_the_transaction_master_open_and_changes_nothing() {
    // P-CBTRN02C-005, P-CBTRN02C-219 (E-32): POSTTRAN over the fixture day fills the live
    // master, so every later POSTTRAN abends at 0100-TRANFILE-OPEN with status 37.
    let doc = parity();
    let exit = loaded_master_exit(&doc);
    assert_eq!(exit, ABEND_EXIT, "E-32 is an abend");
    let h = Harness::new();
    let posted_exit: i32 = expected_value(&doc, POSTTRAN_COUNTS, "exit")
        .parse()
        .expect("P-CBTRN02C-065 gives a numeric exit code");
    let posted = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), posted_exit);
    // P-CBTRN02C-007: status 00 read every record and status 10 ended the file.
    let read = expected_value(&doc, POSTTRAN_COUNTS, "read");
    let processed = format!(r#""processed":{read}"#);
    assert!(
        lines(&posted.stdout)
            .iter()
            .any(|line| line.contains(COUNTS_EVENT) && line.contains(&processed)),
        "the run reads all {read} records to the end of the file"
    );
    let loaded = (LOADED_MASTER_ROW, NOT_REUSABLE);
    run_case(&h, &doc, &[], loaded, exit);
    // 0000-DALYTRAN-OPEN comes first, 0200-XREFFILE-OPEN after the transaction master.
    let dalytran = &STEPS[0];
    run_case(
        &h,
        &doc,
        &[dalytran.fault],
        (dalytran.row, dalytran.status),
        exit,
    );
    run_case(&h, &doc, &[Fault::HideXref], loaded, exit);
}
