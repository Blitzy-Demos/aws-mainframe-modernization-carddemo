//! TRANREPT [app/jcl/TRANREPT.jcl] when CBTRN03C abends: each failure the program can meet is
//! provoked on the test database or in the batch directory, and the run is held to the parity
//! audit row of the paragraph that fails.
//!
//! Every abend exits with 16 (AAP 0.5.3.5) and writes on stderr the paragraph's `DISPLAY`, then
//! `FILE STATUS IS: NNNN` followed by `IO-STATUS-04` (`9910-DISPLAY-IO-STATUS`, P-CBTRN03C-485),
//! then `ABENDING PROGRAM` and `ABEND 999` (`9999-ABEND-PROGRAM`, P-CBTRN03C-484). The REPROC and
//! SORT steps before CBTRN03C end normally, so `TRANSACT.BKUP(+1)` and `TRANSACT.DALY(+1)` stay
//! cataloged, while `TRANREPT(+1)`, `DISP=(NEW,CATLG,DELETE)` [app/jcl/TRANREPT.jcl:L76-80], is
//! never cataloged and its temporary file is deleted, even after lines were written to it. Only a
//! run whose closes all succeeded logs `END OF EXECUTION OF PROGRAM CBTRN03C` (P-CBTRN03C-483).
//!
//! The failures provoked:
//!
//! - The opens (P-CBTRN03C-469 to P-CBTRN03C-472): `TRANREPT(+1)` cannot be allocated, because
//!   `G9999V00` is its newest generation or its allocation lock is held, and the lookup tables
//!   `card_xref`, `trantype_file` and `trancatg_file` are renamed away. With several failing at
//!   once, the first open in program order reports, and a table renamed away abends at its open,
//!   before any record is read, never as a failed lookup.
//! - The lookups (P-CBTRN03C-005): a card, a type and a category key that no record holds, each
//!   displayed after its literal, with status 23.
//! - The close of `TRANREPT(+1)` (P-CBTRN03C-479): a directory holds its generation name, so the
//!   report cannot be cataloged.
//!
//! The other opens and closes have no failure the replacement can meet, so only their status-00
//! branch, `continue`, is proven, by the runs that end normally: `TRANFILE` is the
//! `TRANSACT.DALY(+1)` the job has just cataloged, read record by record (P-CBTRN03C-468,
//! P-CBTRN03C-478); `DATEPARM` is the job's `--start-date` and `--end-date`, which no file
//! holds (E-34; P-CBTRN03C-473, P-CBTRN03C-483); and the three lookup tables are read in one
//! unit of work, whose end is the database connection rather than a file statement
//! (P-CBTRN03C-480 to P-CBTRN03C-482).
//!
//! Every `DISPLAY` text is read from the code spans of the audit rows.

pub mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use carddemo_audit::{Document, PARITY_COMMON_COLUMNS, cell_code_spans};
use carddemo_domain::messages::file_status_message;
use common::{Harness, TRANREPT_CLOCK, block_on, expected_value, parity, parity_cell};

/// The exit code of an abend (AAP 0.5.3.5).
const ABEND_EXIT: i32 = 16;

/// The condition code of a TRANREPT whose steps all end normally over a live master that holds
/// records.
const SUCCESS: i32 = 0;

/// The header of the column holding each audit row's result.
const EXPECTED_RESULT: &str = PARITY_COMMON_COLUMNS[5];

/// The header of the column holding each audit row's input condition.
const INPUT_CONDITION: &str = PARITY_COMMON_COLUMNS[4];

/// `1500-A-LOOKUP-XREF`, `1500-B-LOOKUP-TRANTYPE` and `1500-C-LOOKUP-TRANCATG` on `INVALID KEY`.
const LOOKUPS: &str = "P-CBTRN03C-005";

/// `0000-TRANFILE-OPEN`, the first of the six opens.
const TRANFILE_OPEN: &str = "P-CBTRN03C-468";

/// `0100-REPTFILE-OPEN`.
const REPTFILE_OPEN: &str = "P-CBTRN03C-469";

/// `0200-CARDXREF-OPEN`.
const CARDXREF_OPEN: &str = "P-CBTRN03C-470";

/// `0300-TRANTYPE-OPEN`.
const TRANTYPE_OPEN: &str = "P-CBTRN03C-471";

/// `0400-TRANCATG-OPEN`.
const TRANCATG_OPEN: &str = "P-CBTRN03C-472";

/// `0500-DATEPARM-OPEN`, the last of the six opens.
const DATEPARM_OPEN: &str = "P-CBTRN03C-473";

/// `9000-TRANFILE-CLOSE`, the first of the six closes.
const TRANFILE_CLOSE: &str = "P-CBTRN03C-478";

/// `9100-REPTFILE-CLOSE`.
const REPTFILE_CLOSE: &str = "P-CBTRN03C-479";

/// `9200-CARDXREF-CLOSE`.
const CARDXREF_CLOSE: &str = "P-CBTRN03C-480";

/// `9300-TRANTYPE-CLOSE`.
const TRANTYPE_CLOSE: &str = "P-CBTRN03C-481";

/// `9400-TRANCATG-CLOSE`.
const TRANCATG_CLOSE: &str = "P-CBTRN03C-482";

/// `9500-DATEPARM-CLOSE`, the last of the six closes, then `END OF EXECUTION`.
const DATEPARM_CLOSE: &str = "P-CBTRN03C-483";

/// `9999-ABEND-PROGRAM`.
const ABEND_PROGRAM: &str = "P-CBTRN03C-484";

/// `9910-DISPLAY-IO-STATUS`.
const DISPLAY_IO_STATUS: &str = "P-CBTRN03C-485";

/// Every open and close in program order. Each one's status-00 branch is `continue`, which
/// every normal end of the program takes.
const OPENS_AND_CLOSES: [&str; 12] = [
    TRANFILE_OPEN,
    REPTFILE_OPEN,
    CARDXREF_OPEN,
    TRANTYPE_OPEN,
    TRANCATG_OPEN,
    DATEPARM_OPEN,
    TRANFILE_CLOSE,
    REPTFILE_CLOSE,
    CARDXREF_CLOSE,
    TRANTYPE_CLOSE,
    TRANCATG_CLOSE,
    DATEPARM_CLOSE,
];

/// `IO-STATUS-04` of the lookups, which `MOVE 23 TO IO-STATUS` on `INVALID KEY`
/// (P-CBTRN03C-485).
const INVALID_KEY: &str = "0023";

/// `IO-STATUS-04` of a generation file or a lookup table that cannot be opened or closed: status
/// 30, the permanent error without further information, which the job shows for a file-system
/// or database failure, since neither has a file status of its own.
const PERMANENT_ERROR: &str = "0030";

/// The two in-range transactions of two fixture cards, so a normal end reports two detail
/// lines. Their processing date, the pinned POSTTRAN timestamp (P-CBTRN02C-220), lies inside the
/// default range `2022-01-01` to `2022-07-06`. The columns are those of the view
/// `transaction_master` of migration `0003`.
const LIVE_ROWS: &str = r"
INSERT INTO transaction_master
    (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, tran_merchant_id,
     tran_merchant_name, tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts,
     tran_proc_ts)
VALUES ('0000000000000001', '01', 1, 'POS TERM', 'Purchase at Example Store', 10.00, 800000001,
        'Example Store', 'Example City', '12345', '0500024453765740',
        '2022-06-10 19:27:53.000000', '2022-07-01-12.00.00.000000'),
       ('0000000000000002', '01', 1, 'POS TERM', 'Purchase at Example Store', 20.00, 800000001,
        'Example Store', 'Example City', '12345', '0683586198171516',
        '2022-06-10 19:27:53.000000', '2022-07-01-12.00.00.000000');
";

/// The TRAN-IDs of [`LIVE_ROWS`].
const LIVE_TRAN_IDS: [&str; 2] = ["0000000000000001", "0000000000000002"];

/// Empties the transaction master, which holds no row a reward accrual references.
const CLEAR_LIVE_MASTER: &str = "DELETE FROM transactions";

/// A card number `cardxref.txt` does not hold.
const UNKNOWN_CARD: &str = "9999999999999999";

/// A fixture card's record, then a record of [`UNKNOWN_CARD`], which sorts after it: the first
/// card writes its header block and detail line before the second card's lookup fails.
const UNKNOWN_CARD_ROWS: &str = r"
INSERT INTO transaction_master
    (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, tran_merchant_id,
     tran_merchant_name, tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts,
     tran_proc_ts)
VALUES ('0000000000000003', '01', 1, 'POS TERM', 'Purchase at Example Store', 10.00, 800000001,
        'Example Store', 'Example City', '12345', '0500024453765740',
        '2022-06-10 19:27:53.000000', '2022-07-01-12.00.00.000000'),
       ('0000000000000004', '01', 1, 'POS TERM', 'Purchase at Example Store', 20.00, 800000001,
        'Example Store', 'Example City', '12345', '9999999999999999',
        '2022-06-10 19:27:53.000000', '2022-07-01-12.00.00.000000');
";

/// A record of type `09`, which `trantype.txt` does not hold.
const UNKNOWN_TYPE_ROWS: &str = r"
INSERT INTO transaction_master
    (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, tran_merchant_id,
     tran_merchant_name, tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts,
     tran_proc_ts)
VALUES ('0000000000000005', '09', 1, 'POS TERM', 'Purchase at Example Store', 10.00, 800000001,
        'Example Store', 'Example City', '12345', '0500024453765740',
        '2022-06-10 19:27:53.000000', '2022-07-01-12.00.00.000000');
";

/// A record of type `01` and category `9999`, a key `trancatg.txt` does not hold.
const UNKNOWN_CATEGORY_ROWS: &str = r"
INSERT INTO transaction_master
    (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, tran_merchant_id,
     tran_merchant_name, tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts,
     tran_proc_ts)
VALUES ('0000000000000006', '01', 9999, 'POS TERM', 'Purchase at Example Store', 10.00,
        800000001, 'Example Store', 'Example City', '12345', '0500024453765740',
        '2022-06-10 19:27:53.000000', '2022-07-01-12.00.00.000000');
";

/// Renames the three lookup tables away, so no statement can read them.
const HIDE_LOOKUP_TABLES: &str = "
ALTER TABLE card_xref RENAME TO card_xref_offline;
ALTER TABLE trantype_file RENAME TO trantype_file_offline;
ALTER TABLE trancatg_file RENAME TO trancatg_file_offline;
";

/// Gives `card_xref` its name back.
const RESTORE_CARD_XREF: &str = "ALTER TABLE card_xref_offline RENAME TO card_xref";

/// Gives `trantype_file` its name back.
const RESTORE_TRANTYPE_FILE: &str = "ALTER TABLE trantype_file_offline RENAME TO trantype_file";

/// Gives `trancatg_file` its name back.
const RESTORE_TRANCATG_FILE: &str = "ALTER TABLE trancatg_file_offline RENAME TO trancatg_file";

/// The audited texts of the CBTRN03C abends.
struct Audit {
    doc: Document,
    /// `FILE STATUS IS: NNNN`, the literal `9910-DISPLAY-IO-STATUS` puts before `IO-STATUS-04`.
    file_status: String,
    /// `ABENDING PROGRAM`, the display of `9999-ABEND-PROGRAM`.
    abending: String,
    /// `ABEND <ABCODE>`, the line the replacement writes for `CALL 'CEE3ABD' USING ABCODE`.
    abend: String,
    /// `END OF EXECUTION OF PROGRAM CBTRN03C`, displayed after the last close.
    end_of_execution: String,
}

impl Audit {
    /// Reads the texts from P-CBTRN03C-483, P-CBTRN03C-484 and P-CBTRN03C-485, and checks that
    /// every open and close row has the status-00 branch `continue`.
    fn load() -> Self {
        let doc = parity();
        let io_status = texts(&doc, DISPLAY_IO_STATUS);
        let abend = texts(&doc, ABEND_PROGRAM);
        assert_eq!(
            abend.get(1),
            io_status.first(),
            "{ABEND_PROGRAM} displays ABENDING PROGRAM after the {DISPLAY_IO_STATUS} line"
        );
        let abcode = expected_value(&doc, ABEND_PROGRAM, "ABCODE");
        for id in OPENS_AND_CLOSES {
            assert!(
                parity_cell(&doc, id, INPUT_CONDITION).contains("status 00 / other"),
                "{id} branches on status 00 or other"
            );
            assert!(
                parity_cell(&doc, id, EXPECTED_RESULT).starts_with("continue / "),
                "{id}: status 00 continues"
            );
        }
        let end_of_execution = texts(&doc, DATEPARM_CLOSE)
            .pop()
            .unwrap_or_else(|| panic!("{DATEPARM_CLOSE} names no display after the last close"));
        Self {
            file_status: first(io_status, DISPLAY_IO_STATUS),
            abending: first(abend, ABEND_PROGRAM),
            abend: format!("ABEND {abcode}"),
            end_of_execution,
            doc,
        }
    }

    /// The stderr lines of an abend after `display`, with `IO-STATUS-04` `io_status_04`.
    fn lines(&self, display: String, io_status_04: &str) -> Vec<String> {
        vec![
            display,
            format!("{}{io_status_04}", self.file_status),
            self.abending.clone(),
            self.abend.clone(),
        ]
    }

    /// The stderr lines of the failure branch of the open or close row `id`, with `IO-STATUS-04`
    /// `io_status_04`: the row's `DISPLAY`, then the two displays it names after it.
    fn file_abend(&self, id: &str, io_status_04: &str) -> Vec<String> {
        let texts = texts(&self.doc, id);
        assert_eq!(
            texts.get(1..3),
            Some([self.file_status.clone(), self.abending.clone()].as_slice()),
            "{id} displays the file status and ABENDING PROGRAM after its text"
        );
        self.lines(first(texts, id), io_status_04)
    }
}

/// The double-quoted texts of the code spans of row `id`'s expected result, in order, without
/// their quotes.
fn texts(doc: &Document, id: &str) -> Vec<String> {
    cell_code_spans(parity_cell(doc, id, EXPECTED_RESULT))
        .into_iter()
        .map(|span| {
            span.text
                .strip_prefix('"')
                .and_then(|text| text.strip_suffix('"'))
                .unwrap_or_else(|| panic!("{id}: {:?} is not a double-quoted text", span.text))
                .to_owned()
        })
        .collect()
}

/// The first of the `texts` of row `id`.
fn first(texts: Vec<String>, id: &str) -> String {
    texts
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("{id} names no display"))
}

/// The lines of a child's output stream.
fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Whether the JSON lines of `stdout` hold an event whose message is `message`.
fn logs(stdout: &[u8], message: &str) -> bool {
    let field = format!("\"message\":\"{message}\"");
    lines(stdout).iter().any(|line| line.contains(&field))
}

/// The names of the entries of `dir` that start with `prefix`.
fn entries_named(dir: &Path, prefix: &str) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("cannot list {}: {error}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| {
                    panic!("cannot read an entry of {}: {error}", dir.display())
                })
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.starts_with(prefix))
        .collect()
}

/// A seeded harness whose live master holds rows, and the number of `tranrept` runs made on it.
/// Every run catalogs the next generation of `TRANSACT.BKUP` and of `TRANSACT.DALY` before
/// CBTRN03C starts, whatever the program does after.
struct Job {
    h: Harness,
    runs: usize,
}

impl Job {
    /// A seeded harness whose live master holds `rows`.
    fn new(rows: &str) -> Self {
        let h = Harness::new();
        block_on(h.stack().exec_sql(rows));
        Self { h, runs: 0 }
    }

    /// Replaces the live master with `rows`.
    fn replace_rows(&self, rows: &str) {
        block_on(self.h.stack().exec_sql(CLEAR_LIVE_MASTER));
        block_on(self.h.stack().exec_sql(rows));
    }

    /// Runs `batch tranrept` with its JCL default dates, checks that it exits with `code`, and
    /// that the REPROC and SORT steps before CBTRN03C cataloged their `(+1)` generations.
    fn run(&mut self, code: i32, case: &str) -> Output {
        let output = self.h.run_expect(&["tranrept"], Some(TRANREPT_CLOCK), code);
        self.runs += 1;
        for dataset in ["TRANSACT.BKUP", "TRANSACT.DALY"] {
            let generation = format!("{dataset}.G{:04}V00", self.runs);
            assert_eq!(
                self.h.generations(dataset).pop(),
                Some(self.h.batch_dir().join(&generation)),
                "{case}: {dataset}(+1) of the step before CBTRN03C must stay cataloged"
            );
        }
        output
    }

    /// The cataloged `TRANREPT` generations: the regular files of that name. A directory that
    /// holds a generation's name is none.
    fn reports(&self) -> Vec<PathBuf> {
        self.h
            .generations("TRANREPT")
            .into_iter()
            .filter(|path| path.is_file())
            .collect()
    }

    /// One run that abends: exit 16, no `TRANREPT(+1)` cataloged, its temporary file deleted, no
    /// `END OF EXECUTION`. Returns the stderr lines.
    fn abend(&mut self, audit: &Audit, case: &str) -> Vec<String> {
        let reports = self.reports();
        let output = self.run(ABEND_EXIT, case);
        assert_eq!(
            self.reports(),
            reports,
            "{case}: DISP=(NEW,CATLG,DELETE) catalogs no TRANREPT(+1) on an abend"
        );
        assert_eq!(
            entries_named(self.h.batch_dir(), ".TRANREPT.G"),
            Vec::<String>::new(),
            "{case}: the uncataloged TRANREPT(+1) is deleted"
        );
        assert!(
            !logs(&output.stdout, &audit.end_of_execution),
            "{case}: an abend ends before {DATEPARM_CLOSE}"
        );
        lines(&output.stderr)
    }

    /// One run that ends normally: exit 0, nothing on stderr, `TRANREPT(+1)` cataloged with a
    /// detail line for each of `tran_ids`, and `END OF EXECUTION OF PROGRAM CBTRN03C` logged,
    /// the status-00 branch of every open and close.
    fn ends_normally(&mut self, audit: &Audit, tran_ids: &[&str], case: &str) {
        let mut reports = self.reports();
        let output = self.run(SUCCESS, case);
        assert_eq!(lines(&output.stderr), Vec::<String>::new(), "{case}");
        let report = self
            .reports()
            .pop()
            .unwrap_or_else(|| panic!("{case}: no TRANREPT(+1) is cataloged"));
        assert!(!reports.contains(&report), "{case}: TRANREPT(+1) is new");
        reports.push(report.clone());
        assert_eq!(self.reports(), reports, "{case}");
        let text = fs::read_to_string(&report)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", report.display()));
        for tran_id in tran_ids {
            let details = text
                .lines()
                .filter(|line| line.starts_with(tran_id))
                .count();
            assert_eq!(details, 1, "{case}: the detail line of {tran_id}");
        }
        assert!(
            logs(&output.stdout, &audit.end_of_execution),
            "{case}: {DATEPARM_CLOSE} displays {:?} after its close",
            audit.end_of_execution
        );
    }
}

/// Runs `sql` on the harness database.
fn exec(job: &Job, sql: &str) {
    block_on(job.h.stack().exec_sql(sql));
}

#[test]
fn each_open_that_fails_abends_before_any_record_is_read_in_program_order() {
    let audit = Audit::load();
    let mut job = Job::new(LIVE_ROWS);
    let newest = job.h.batch_dir().join("TRANREPT.G9999V00");

    // TRANREPT(+1) cannot be allocated and no lookup table can be read: REPTFILE, opened before
    // the three, reports.
    fs::write(&newest, b"")
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", newest.display()));
    exec(&job, HIDE_LOOKUP_TABLES);
    let stderr = job.abend(&audit, "TRANREPT.G9999V00 newest, lookup tables renamed");
    assert_eq!(stderr, audit.file_abend(REPTFILE_OPEN, PERMANENT_ERROR));
    fs::remove_file(&newest)
        .unwrap_or_else(|error| panic!("cannot remove {}: {error}", newest.display()));

    // Each table renamed away abends at its own open, CARDXREF before TRANTYPE before TRANCATG,
    // never as a lookup of the first record.
    let opens = [
        (CARDXREF_OPEN, RESTORE_CARD_XREF, "card_xref"),
        (TRANTYPE_OPEN, RESTORE_TRANTYPE_FILE, "trantype_file"),
        (TRANCATG_OPEN, RESTORE_TRANCATG_FILE, "trancatg_file"),
    ];
    for (id, restore, table) in opens {
        let stderr = job.abend(&audit, &format!("{table} and the tables after it renamed"));
        assert_eq!(stderr, audit.file_abend(id, PERMANENT_ERROR), "{table}");
        exec(&job, restore);
    }

    job.ends_normally(&audit, &LIVE_TRAN_IDS, "every file can be opened");
    a_held_allocation_lock_fails_the_reptfile_open(&mut job, &audit);
}

/// A held `TRANREPT` allocation lock fails the REPTFILE open: the lock's text, then the audited
/// displays. The job neither waits for the lock nor takes it over.
fn a_held_allocation_lock_fails_the_reptfile_open(job: &mut Job, audit: &Audit) {
    let lock = job.h.batch_dir().join(".TRANREPT.lock");
    fs::write(&lock, b"0\n")
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", lock.display()));
    let stderr = job.abend(audit, "TRANREPT allocation lock held");
    assert!(
        stderr
            .first()
            .is_some_and(|line| line.contains(&lock.display().to_string())),
        "the first line names the lock file: {stderr:?}"
    );
    assert_eq!(
        stderr.get(1..),
        Some(audit.file_abend(REPTFILE_OPEN, PERMANENT_ERROR).as_slice())
    );
    assert_eq!(
        fs::read(&lock).ok(),
        Some(b"0\n".to_vec()),
        "the held lock is left as it was"
    );
    fs::remove_file(&lock)
        .unwrap_or_else(|error| panic!("cannot remove {}: {error}", lock.display()));
}

#[test]
fn a_lookup_that_meets_invalid_key_abends_with_its_key_and_status_23() {
    let audit = Audit::load();
    let mut job = Job::new(LIVE_ROWS);
    let literals = texts(&audit.doc, LOOKUPS);
    assert_eq!(
        literals.len(),
        3,
        "{LOOKUPS} names the three lookup displays"
    );
    let cases = [
        (UNKNOWN_CARD_ROWS, format!("{}{UNKNOWN_CARD}", literals[0])),
        (UNKNOWN_TYPE_ROWS, format!("{}09", literals[1])),
        (UNKNOWN_CATEGORY_ROWS, format!("{}019999", literals[2])),
    ];
    for (rows, display) in cases {
        job.replace_rows(rows);
        let stderr = job.abend(&audit, &display);
        assert_eq!(stderr, audit.lines(display, INVALID_KEY));
    }

    // REPTFILE close: a directory holds the name of TRANREPT(+1), so the report written to its
    // temporary file cannot be cataloged.
    job.replace_rows(LIVE_ROWS);
    assert_eq!(
        job.reports(),
        Vec::<PathBuf>::new(),
        "no report is cataloged yet"
    );
    let taken = job.h.batch_dir().join("TRANREPT.G0001V00");
    fs::create_dir(&taken)
        .unwrap_or_else(|error| panic!("cannot create {}: {error}", taken.display()));
    let stderr = job.abend(&audit, "TRANREPT.G0001V00 is a directory");
    assert_eq!(stderr, audit.file_abend(REPTFILE_CLOSE, PERMANENT_ERROR));
    assert_eq!(
        entries_named(&taken, ""),
        Vec::<String>::new(),
        "the directory is left as it was"
    );
    fs::remove_dir(&taken)
        .unwrap_or_else(|error| panic!("cannot remove {}: {error}", taken.display()));
    job.ends_normally(&audit, &LIVE_TRAN_IDS, "TRANREPT.G0001V00 free");
}

#[test]
fn the_file_status_display_follows_9910_display_io_status() {
    let audit = Audit::load();
    // A numeric status whose first byte is not 9 shows 00 and its two digits, so the lookups
    // show 0023; a status whose first byte is 9, or that is not numeric, shows its first byte
    // and the binary value of its second as three digits.
    let cases: [([u8; 2], &str); 5] = [
        (*b"23", INVALID_KEY),
        (*b"30", PERMANENT_ERROR),
        (*b"00", "0000"),
        ([b'9', 0x05], "9005"),
        ([b'3', 0x05], "3005"),
    ];
    for (status, io_status_04) in cases {
        assert_eq!(
            file_status_message(status),
            format!("{}{io_status_04}", audit.file_status),
            "status {status:?}"
        );
    }
}
