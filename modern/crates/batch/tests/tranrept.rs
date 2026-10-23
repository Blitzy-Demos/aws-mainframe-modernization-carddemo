//! `carddemo-batch tranrept`, the TRANREPT job of CBTRN03C, run as a subprocess against a
//! fixture-loaded `PostgreSQL` testcontainer (g3b), and the separation of the two
//! transaction-type data versions (E23, AAP 0.3.3.4).
//!
//! The expected report is the `FIXRPT-` rows of `modern/audit/fixtures.md` with the layout rules
//! of `modern/audit/batch/TRANREPT.md`; the E23 run is CBTRN03C-038 of that file and COBTUPDT-027
//! of `modern/audit/batch/MNTTRDB2.md`. Expected values are transcribed from those rows and from
//! the fixture bytes they cite, never computed through `domain::batch::cbtrn03c`.
//!
//! # Scenarios
//!
//! - `posttran` stamps `TRAN-PROC-TS` from the system clock, which a subprocess cannot be given,
//!   so a report over its output takes a window around today: the day before to the day after,
//!   read from `SystemClock`, the local clock the binary reads, so a run across midnight still
//!   selects every posted row. The report then holds exactly the records of the POSTED scenario,
//!   whose header shows the dates passed.
//! - The default window, `2022-01-01` to `2022-07-06`, selects nothing posted today: that empty
//!   selection is CBTRN03C-037, which halt candidate X-TRANREPT-H2 blocks. The default-window
//!   test therefore reproduces POSTED's `transactions` table as `fixtures.md` states it: the rows
//!   `posttran` wrote, with the processing timestamp the POSTED `FixedClock` instant gives them
//!   (FIXPOST-901), the one value the clock decides. `tranrept` reads no other table `posttran`
//!   writes.
//! - A reports directory that cannot be created stands for the failed open of `REPTFILE`
//!   (CBTRN03C-007), which ends the run on the abend path (CBTRN03C-035).
//! - `carddemo-batch` is a binary, so no repository double reaches it, and the other failures
//!   are provoked from outside the process. A [`Relay`] between the binary and the container
//!   closes the connection of the selection after its first row, so the next fetch fails inside
//!   the loop (CBTRN03C-010), or the connection of a lookup table at its row description
//!   (CBTRN03C-040 to CBTRN03C-042). A file-size limit that `/bin/sh` sets before it runs the
//!   binary makes a report write fail (CBTRN03C-033) or, set above the bytes the loop writes, the
//!   last write of the close (CBTRN03C-034).
//!
//! # Blocked values
//!
//! Halt candidates X-FIXTURES-H3 and X-TRANREPT-H1 leave the order of the details inside a card
//! block to the sort, so those values get no parity assertion: each block's details are compared
//! as a set at the block's record positions, and the page totals other than records 21, 201 and
//! 341, and the grand total, are checked for their record kind only. Positions, record kinds, the
//! detail texts, the 49 account totals and the three fixed page totals are asserted byte for
//! byte. The same order decides the last amount, so the items of the two end-of-file lines are
//! checked for their shape only (CBTRN03C-029).
//!
//! The four characters after `FILE STATUS IS: NNNN` on a database or file failure have no
//! counterpart (X-TRANREPT-H3), and the lookup failures other than a missing key are halted as a
//! whole (CBTRN03C-040 to CBTRN03C-042, X-TRANREPT-H5): both are reported `BLOCKED`, never
//! compared. The replacement reads each lookup table whole when it opens it, so a failed read of
//! one takes the open path of CBTRN03C-007; that outcome is printed with the `BLOCKED` report.
//!
//! # Job log
//!
//! Each `DISPLAY` of CBTRN03C is one tracing event of the job's module,
//! `carddemo_batch::jobs::tranrept`, whose message is the line, in program order
//! (`crates/batch/src/jobs/tranrept.rs`). The runs whose job log is checked log at `info`, and
//! [`job_log`] reads those messages from standard output. A run over POSTED shows the start line,
//! the `Reporting from` line with the dates passed, one 350-byte `TRAN-RECORD` line per selected
//! row in report order, the two end-of-file lines and the end line, with the path on the line
//! after it (CBTRN03C-006, CBTRN03C-008 to CBTRN03C-010, CBTRN03C-012, CBTRN03C-029,
//! CBTRN03C-032, CBTRN03C-034). A failure path ends with its line, the status line and
//! `ABENDING PROGRAM`, and shows no end line (CBTRN03C-007, CBTRN03C-010, CBTRN03C-033 to
//! CBTRN03C-036).
//!
//! # Cardholder data
//!
//! Report records hold no card number (CBTRN03C-023), but every failure message names record
//! numbers, block numbers, byte offsets, line positions or field names only, never record
//! bytes, and no job-log line is printed: the `TRAN-RECORD` line holds a card number in its
//! redacted form only (CBTRN03C-012). Standard error, which `carddemo-batch` keeps free of
//! protected values, is the only process output a message shows. Every `tranrept` run, normal or
//! failing, has its standard output and standard error searched whole for the 50 fixture card
//! numbers, which none may hold (CBTRN03C-039), the run whose transaction IDs are card numbers
//! included.

use std::collections::HashMap;
use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{Days, NaiveDate};
use copybook::{FixedRecord, PicTimestamp, PicX};
use domain::clock::{Clock, SystemClock};
use domain::records::TransactionRecord;
use domain::sensitive::CardNumber;
use persistence::{
    Db2TransactionTypeRepository, PgStore, TransactionRepository, TransactionTypeRepository,
};
use test_support::fixtures::{
    fixture_card_numbers, fixture_dir, fixture_path, start_loaded_postgres,
};
use test_support::postgres::PostgresGuard;

type TestResult = Result<(), Box<dyn Error>>;

/// Runs `test` to completion on a multi-thread runtime with every driver enabled and the default
/// worker count, the one Tokio's test attribute builds for its `multi_thread` flavour, without the
/// lint attributes, `expect` and unchecked pin that attribute's expansion carries.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

/// Bytes of a report record, `FD-REPTFILE-REC PIC X(133)`, each followed by LF (CBTRN03C-032).
const RECORD_LEN: usize = 133;

/// Records of the POSTED report: 262 details, 49 account totals, 15 page totals, the grand
/// total, 64 dash records and 15 header groups of 4 (FIXRPT-901).
const REPORT_RECORDS: usize = 451;

/// The default window of `tranrept`, the SORT constants of `TRANREPT.jcl:43-44` (CBTRN03C-001).
const DEFAULT_START: &str = "2022-01-01";
const DEFAULT_END: &str = "2022-07-06";

/// `TRAN-PROC-TS` of every row `posttran` writes under the POSTED `FixedClock` instant
/// `2022-07-01 12:00:00.000000`, in layout `D` (PIC-019, FIXPOST-901).
const POSTED_PROC_TS: &[u8; 26] = b"2022-07-01-12.00.00.000000";

/// Rows `posttran` writes over `dailytran.txt` (FIXPOST-901).
const POSTED_ROWS: usize = 262;

/// The first record of each header group, before the first detail and after each page total but
/// the last (FIXRPT-902).
const HEADER_GROUPS: [usize; 15] = [
    1, 23, 83, 103, 123, 163, 183, 203, 263, 283, 323, 343, 383, 403, 443,
];

/// The page totals by record number (FIXRPT-904, FIXRPT-905), each followed by a dash record.
/// The value is `None` where it follows the order inside a card block, which X-FIXTURES-H3
/// blocks; only records 21, 201 and 341 close pages that split no block.
const PAGE_TOTALS: [(usize, Option<&str>); 15] = [
    (21, Some("+      4,665.52")),
    (81, None),
    (101, None),
    (121, None),
    (161, None),
    (181, None),
    (201, Some("+      2,577.64")),
    (261, None),
    (281, None),
    (321, None),
    (341, Some("+      4,381.41")),
    (381, None),
    (401, None),
    (441, None),
    (449, None),
];

/// The grand total, the last record; its value repeats the last amount, which X-FIXTURES-H3
/// blocks (FIXRPT-905).
const GRAND_TOTAL_RECORD: usize = 451;

/// `REPORT-NAME-HEADER` up to the start date: `DALYREPT` in `X(38)`, `Daily Transaction Report`
/// in `X(41)` and `Date Range: ` (CBTRN03C-020, FIXRPT-902).
const NAME_HEADER: &str =
    "DALYREPT                              Daily Transaction Report                 Date Range: ";

/// `TRANSACTION-HEADER-1`, 112 bytes before its padding (CBTRN03C-020, FIXRPT-902).
const COLUMN_HEADER: &str = "Transaction ID   Account ID  Transaction Type   Tran Category                      Tran Source            Amount";

/// The label fields of the total records, each followed by dots up to column 97 and the edited
/// amount in columns 98-112 (CBTRN03C-025, CBTRN03C-027, CBTRN03C-031).
const PAGE_TOTAL_LABEL: &str = "Page Total ";
const ACCOUNT_TOTAL_LABEL: &str = "Account Total";
const GRAND_TOTAL_LABEL: &str = "Grand Total";

/// Columns 1-97 of a total record: its label and the dots after it.
const TOTAL_PREFIX_LEN: usize = 97;

/// Columns 98-112: an edited amount, `+ZZZ,ZZZ,ZZZ.ZZ` or `-ZZZ,ZZZ,ZZZ.ZZ` (CBTRN03C-024,
/// CBTRN03C-026).
const AMOUNT_LEN: usize = 15;

/// Columns 30-97 of a type `01` detail: type code and VSAM description (`trantype.txt:1`),
/// category and its description (`trancatg.txt:1`), source and the 4 spaces before the amount
/// (CBTRN03C-023, FIXRPT-903).
const PURCHASE_COLUMNS: &str =
    "01-Purchase        0001-Regular Sales Draft           POS TERM      ";

/// Columns 30-97 of a type `03` detail (`trantype.txt:3`, `trancatg.txt:9`; FIXRPT-903).
const CREDIT_COLUMNS: &str = "03-Credit          0001-Credit to Account             OPERATOR      ";

/// Columns 33-47 of every type `01` detail: the first 15 bytes of the VSAM description
/// `Purchase` (`trantype.txt:1`; CBTRN03C-038).
const VSAM_PURCHASE: &[u8; 15] = b"Purchase       ";

/// The `U` record of COBTUPDT-103: code `01`, a fictional description, space-padded to 50.
const RENAMED: &str = "AUDIT TEST PURCHASE RENAMED";

/// Bytes of a COBTUPDT input record: action, type code and description (COBTUPDT-002).
const MAINTENANCE_RECORD_LEN: usize = 53;

/// The Db2 seed description of type `01`, `DB2LTTYP.ctl:17` (migration 0006; E23).
const DB2_SEED_PURCHASE: &str = "PURCHASE";

const _: () = assert!(NAME_HEADER.len() == 91);
const _: () = assert!(COLUMN_HEADER.len() == 112);
const _: () = assert!(PURCHASE_COLUMNS.len() == 68 && CREDIT_COLUMNS.len() == 68);

// The job-log lines of CBTRN03C, transcribed from `modern/audit/batch/TRANREPT.md`.

/// The first line of every run the window lets through (CBTRN03C-006).
const START_LINE: &str = "START OF EXECUTION OF PROGRAM CBTRN03C";
/// The last line of a normal end, before the path (CBTRN03C-032, CBTRN03C-034).
const END_LINE: &str = "END OF EXECUTION OF PROGRAM CBTRN03C";
/// The DATEPARM line: `Reporting from ` (15 characters), the start date, ` to ` (4) and the end
/// date (CBTRN03C-008).
const REPORTING_FROM: &str = "Reporting from ";
const REPORTING_TO: &str = " to ";
/// The DATEPARM line of the default window (CBTRN03C-008, FIXRPT-901).
const DEFAULT_REPORTING_LINE: &str = "Reporting from 2022-01-01 to 2022-07-06";
/// Bytes of the `TRAN-RECORD` line, the 350-byte `CVTRA05Y` record (CBTRN03C-012).
const TRAN_RECORD_LEN: usize = 350;
/// The bytes of the `TRAN-RECORD` line shown as `*` whatever the row holds, as 0-based ranges:
/// `TRAN-SOURCE` and `TRAN-DESC` (bytes 23-132), `TRAN-MERCHANT-NAME`, `TRAN-MERCHANT-CITY` and
/// `TRAN-MERCHANT-ZIP` (153-262) and `FILLER` (331-350) (CBTRN03C-012).
const MASKED_BYTES: [(usize, usize); 3] = [(22, 132), (152, 262), (330, 350)];
/// `TRAN-CARD-NUM`, bytes 263-278 of the `TRAN-RECORD` line, as a 0-based range: twelve `*`,
/// then the last four digits (CBTRN03C-012).
const CARD_BYTES: (usize, usize) = (262, 278);
const CARD_MASK_LEN: usize = 12;
/// `TRAN-ID`, bytes 1-16 of the `TRAN-RECORD` line and columns 1-16 of a detail record
/// (CBTRN03C-012, CBTRN03C-023).
const TRAN_ID_LEN: usize = 16;
/// `TRAN-PROC-TS`, bytes 305-330 of the `TRAN-RECORD` line, as a 0-based range (CVTRA05Y-013).
const PROC_TS_BYTES: (usize, usize) = (304, 330);
/// The fields of the `TRAN-RECORD` line, as 0-based ranges covering its 350 bytes, by which a
/// failure names what differs (CVTRA05Y-001 to CVTRA05Y-014).
const TRAN_RECORD_FIELDS: [(&str, usize, usize); 14] = [
    ("TRAN-ID", 0, 16),
    ("TRAN-TYPE-CD", 16, 18),
    ("TRAN-CAT-CD", 18, 22),
    ("TRAN-SOURCE", 22, 32),
    ("TRAN-DESC", 32, 132),
    ("TRAN-AMT", 132, 143),
    ("TRAN-MERCHANT-ID", 143, 152),
    ("TRAN-MERCHANT-NAME", 152, 202),
    ("TRAN-MERCHANT-CITY", 202, 252),
    ("TRAN-MERCHANT-ZIP", 252, 262),
    ("TRAN-CARD-NUM", 262, 278),
    ("TRAN-ORIG-TS", 278, 304),
    ("TRAN-PROC-TS", 304, 330),
    ("FILLER", 330, 350),
];
/// The shape of the `TRAN-PROC-TS` that `posttran` stamps from the clock: layout `D` with the
/// fraction cut to hundredths, `YYYY-MM-DD-HH.MM.SS.hh0000`, `9` standing for any ASCII digit
/// (CBTRN02C-013).
const CLOCK_PROC_TS_SHAPE: &[u8; 26] = b"9999-99-99-99.99.99.990000";
/// The end-of-file lines: `TRAN-AMT ` (9 characters) and `WS-PAGE-TOTAL` (13), each followed by
/// its item's 11 digits, the sign overpunched on the last (CBTRN03C-029).
const TRAN_AMT_LABEL: &str = "TRAN-AMT ";
const PAGE_TOTAL_ITEM_LABEL: &str = "WS-PAGE-TOTAL";
const AMOUNT_ITEM_LEN: usize = 11;
/// The failure lines of the paths the tests provoke (CBTRN03C-007, CBTRN03C-010, CBTRN03C-033,
/// CBTRN03C-034).
const OPEN_REPTFILE: &str = "ERROR OPENING REPTFILE";
const OPEN_CARDXREF: &str = "ERROR OPENING CROSS REF FILE";
const OPEN_TRANTYPE: &str = "ERROR OPENING TRANSACTION TYPE FILE";
const OPEN_TRANCATG: &str = "ERROR OPENING TRANSACTION CATG FILE";
const READ_TRANFILE: &str = "ERROR READING TRANSACTION FILE";
const WRITE_REPTFILE: &str = "ERROR WRITING REPTFILE";
const CLOSE_REPTFILE: &str = "ERROR CLOSING REPORT FILE";
/// `9910-DISPLAY-IO-STATUS`: this 20-character label, then four status characters
/// (CBTRN03C-036).
const STATUS_LABEL: &str = "FILE STATUS IS: NNNN";
const STATUS_LINE_LEN: usize = 24;
/// The line of `9999-ABEND-PROGRAM`, the last of every failure path (CBTRN03C-035).
const ABEND_LINE: &str = "ABENDING PROGRAM";

const _: () = assert!(REPORTING_FROM.len() == 15 && REPORTING_TO.len() == 4);
const _: () = assert!(TRAN_AMT_LABEL.len() == 9 && PAGE_TOTAL_ITEM_LABEL.len() == 13);
const _: () = assert!(STATUS_LABEL.len() == 20);
// The fields follow one another from byte 1 to byte 350, `TRAN-PROC-TS` among them.
const _: () = {
    let mut index = 0;
    let mut next = 0;
    while index < TRAN_RECORD_FIELDS.len() {
        let (_, start, end) = TRAN_RECORD_FIELDS[index];
        assert!(start == next && end > start);
        next = end;
        index += 1;
    }
    assert!(next == TRAN_RECORD_LEN);
    assert!(
        TRAN_RECORD_FIELDS[12].1 == PROC_TS_BYTES.0 && TRAN_RECORD_FIELDS[12].2 == PROC_TS_BYTES.1
    );
};

/// The target every job-log event carries in the JSON lines of `runtime::telemetry`: the job's
/// module. The events of `main`, such as `job finished`, carry another.
const JOB_LOG_TARGET: &str = r#""target":"carddemo_batch::jobs::tranrept""#;
/// What precedes an event's message in the JSON lines of `runtime::telemetry`.
const MESSAGE_KEY: &str = r#""message":""#;

/// A column of each statement `tranrept` runs that no other of them returns, by which the
/// [`Relay`] tells the statements apart in their row descriptions (migrations 0001 and 0002).
const TRANSACTIONS_COLUMN: &str = "proc_ts";
const CARD_XREF_COLUMN: &str = "cust_id";
const TRANSACTION_TYPES_COLUMN: &str = "tran_type_desc";
const TRANSACTION_CATEGORIES_COLUMN: &str = "tran_cat_type_desc";

/// The setting the event of a failed report open names in place of the directory's path.
const REPORTS_DIR_SETTING: &str = "reports under CARDDEMO_VAR_DIR";

/// The bytes a file-size limit lies strictly between. `/bin/sh` sets the limit in `ulimit -f`
/// blocks, whose size depends on the shell: 512 bytes under dash, POSIX and GNU Bash in POSIX
/// mode, 1,024 under GNU Bash outside it and under Apple's Bash 3.2, which macOS can run as
/// `/bin/sh`. [`run_size_limited`] therefore sets the fewest blocks of the size
/// [`ulimit_block_len`] measures whose bytes lie inside the window.
#[derive(Clone, Copy)]
struct SizeWindow {
    above: usize,
    below: usize,
}

/// The bytes of each write the job makes to the report over POSTED: the binary buffers 8,192
/// bytes before a write reaches the file, which hold 61 records of 134.
const WRITE_LEN: usize = 8_174;
/// The limit of a run whose report fails at its first write, inside the loop (CBTRN03C-033):
/// below that write. One block, 512 or 1,024 bytes, lies inside it.
const FIRST_WRITE_LIMIT: SizeWindow = SizeWindow {
    above: 0,
    below: WRITE_LEN,
};
/// The limit of a run whose report fails only at the last write, when the file is closed
/// (CBTRN03C-034): above the 7 writes the loop makes over POSTED (57,218 bytes) and below the
/// report's 60,434 bytes (451 records of 134). 112 blocks of 512 bytes or 56 of 1,024, 57,344
/// bytes either way, are the fewest inside it.
const CLOSE_LIMIT: SizeWindow = SizeWindow {
    above: 57_218,
    below: 60_434,
};
// 61 records of 134 fill the 8,192 bytes; the loop makes 7 such writes and leaves the close
// fewer bytes than one.
const _: () = assert!(
    WRITE_LEN == 8_192 / (RECORD_LEN + 1) * (RECORD_LEN + 1)
        && CLOSE_LIMIT.above == 7 * WRITE_LEN
        && CLOSE_LIMIT.below == REPORT_RECORDS * (RECORD_LEN + 1)
        && CLOSE_LIMIT.below - CLOSE_LIMIT.above < WRITE_LEN
);

/// The type of a POSTED detail, which fixes its columns 30-97.
#[derive(Clone, Copy)]
enum Kind {
    Purchase,
    Credit,
}

const P: Kind = Kind::Purchase;
const C: Kind = Kind::Credit;

impl Kind {
    fn columns(self) -> &'static str {
        match self {
            Kind::Purchase => PURCHASE_COLUMNS,
            Kind::Credit => CREDIT_COLUMNS,
        }
    }
}

/// One detail of a card block: its record number, the `dailytran.txt` line whose `DALYTRAN-ID`
/// is its transaction ID (`{T:n}`), its type and its edited amount, columns 98-112.
struct Detail {
    record: usize,
    tran_line: usize,
    kind: Kind,
    amount: &'static str,
}

const fn d(record: usize, tran_line: usize, kind: Kind, amount: &'static str) -> Detail {
    Detail {
        record,
        tran_line,
        kind,
        amount,
    }
}

/// One card block of FIXRPT-001 to FIXRPT-050, in card order, block n being `cardxref.txt:n`:
/// the account of its cross-reference, its details in `tran_id` order, and its account-total
/// record with the edited total, absent for the last block (FIXRPT-905).
struct Block {
    account: &'static str,
    details: &'static [Detail],
    total: Option<(usize, &'static str)>,
}

/// FIXRPT-001 to FIXRPT-050.
static BLOCKS: [Block; 50] = [
    // audit: FIXRPT-001
    Block {
        account: "00000000050",
        details: &[
            d(5, 21, P, "         183.88"),
            d(6, 102, P, "          14.00"),
            d(7, 142, P, "         967.44"),
            d(8, 184, C, "-         47.88"),
            d(9, 214, P, "          94.77"),
            d(10, 257, P, "         241.66"),
        ],
        total: Some((11, "+      1,453.87")),
    },
    // audit: FIXRPT-002
    Block {
        account: "00000000027",
        details: &[
            d(13, 37, P, "         777.33"),
            d(14, 52, P, "         579.77"),
            d(15, 145, P, "         319.88"),
            d(16, 247, P, "         975.11"),
            d(17, 277, P, "         932.55"),
            d(18, 281, C, "-        372.99"),
        ],
        total: Some((19, "+      3,211.65")),
    },
    // audit: FIXRPT-003
    Block {
        account: "00000000002",
        details: &[
            d(27, 24, P, "         355.11"),
            d(28, 87, C, "-        763.00"),
            d(29, 99, P, "         569.99"),
            d(30, 157, P, "         655.11"),
            d(31, 160, P, "         164.99"),
            d(32, 254, P, "         594.77"),
        ],
        total: Some((33, "+      1,576.97")),
    },
    // audit: FIXRPT-004
    Block {
        account: "00000000020",
        details: &[
            d(35, 2, C, "-        919.00"),
            d(36, 29, P, "         555.22"),
            d(37, 109, P, "         965.55"),
            d(38, 236, P, "         273.11"),
            d(39, 276, P, "          86.00"),
            d(40, 290, P, "         573.22"),
        ],
        total: Some((41, "+      1,534.10")),
    },
    // audit: FIXRPT-005
    Block {
        account: "00000000012",
        details: &[
            d(43, 83, P, "         210.33"),
            d(44, 130, P, "         545.66"),
            d(45, 139, P, "         587.11"),
            d(46, 194, P, "         715.66"),
            d(47, 219, C, "-        762.44"),
            d(48, 269, P, "         298.33"),
        ],
        total: Some((49, "+      1,594.65")),
    },
    // audit: FIXRPT-006
    Block {
        account: "00000000044",
        details: &[
            d(51, 20, P, "         303.11"),
            d(52, 38, C, "-        215.33"),
            d(53, 120, P, "         385.44"),
            d(54, 153, P, "         654.99"),
            d(55, 201, P, "         269.22"),
            d(56, 268, P, "         399.44"),
        ],
        total: Some((57, "+      1,796.87")),
    },
    // audit: FIXRPT-007
    Block {
        account: "00000000037",
        details: &[d(59, 196, C, "-        132.88")],
        total: Some((60, "-        132.88")),
    },
    // audit: FIXRPT-008
    Block {
        account: "00000000035",
        details: &[
            d(62, 126, P, "            .99"),
            d(63, 168, P, "         659.44"),
            d(64, 187, P, "          39.55"),
            d(65, 198, P, "         639.22"),
            d(66, 226, C, "-        499.99"),
        ],
        total: Some((67, "+        839.21")),
    },
    // audit: FIXRPT-009
    Block {
        account: "00000000039",
        details: &[
            d(69, 23, C, "-         70.99"),
            d(70, 44, P, "         168.33"),
            d(71, 54, P, "         848.33"),
            d(72, 58, P, "         385.77"),
            d(73, 182, P, "         425.11"),
            d(74, 289, P, "         223.33"),
        ],
        total: Some((75, "+      1,979.88")),
    },
    // audit: FIXRPT-010
    Block {
        account: "00000000024",
        details: &[
            d(77, 105, P, "         997.88"),
            d(78, 121, P, "         617.00"),
            d(79, 143, P, "         310.00"),
            d(80, 147, P, "         161.99"),
            d(87, 218, P, "          77.00"),
            d(88, 250, C, "-        641.77"),
        ],
        total: Some((89, "+      1,522.10")),
    },
    // audit: FIXRPT-011
    Block {
        account: "00000000006",
        details: &[
            d(91, 41, P, "         843.66"),
            d(92, 61, P, "         175.88"),
            d(93, 118, C, "-        396.22"),
            d(94, 206, P, "         718.55"),
            d(95, 248, P, "          53.55"),
            d(96, 274, P, "         958.77"),
        ],
        total: Some((97, "+      2,354.19")),
    },
    // audit: FIXRPT-012
    Block {
        account: "00000000022",
        details: &[
            d(99, 17, C, "-        945.66"),
            d(100, 36, P, "         401.22"),
            d(107, 75, P, "         848.77"),
            d(108, 193, P, "         598.33"),
            d(109, 266, P, "         768.88"),
            d(110, 286, P, "         711.66"),
        ],
        total: Some((111, "+      2,383.20")),
    },
    // audit: FIXRPT-013
    Block {
        account: "00000000004",
        details: &[
            d(113, 137, P, "          28.33"),
            d(114, 138, P, "         437.11"),
            d(115, 188, P, "         736.33"),
            d(116, 260, P, "         281.55"),
            d(117, 265, C, "-        322.99"),
            d(118, 275, P, "         462.33"),
        ],
        total: Some((119, "+      1,622.66")),
    },
    // audit: FIXRPT-014
    Block {
        account: "00000000010",
        details: &[
            d(127, 11, P, "          94.33"),
            d(128, 85, P, "         457.55"),
            d(129, 151, P, "         293.44"),
            d(130, 171, C, "-        537.44"),
            d(131, 263, P, "          80.66"),
            d(132, 300, P, "         603.22"),
        ],
        total: Some((133, "+        991.76")),
    },
    // audit: FIXRPT-015
    Block {
        account: "00000000041",
        details: &[
            d(135, 13, P, "         829.55"),
            d(136, 15, P, "         958.99"),
            d(137, 63, P, "          76.33"),
            d(138, 122, C, "-        756.66"),
            d(139, 288, P, "         220.00"),
            d(140, 291, P, "         314.66"),
        ],
        total: Some((141, "+      1,642.87")),
    },
    // audit: FIXRPT-016
    Block {
        account: "00000000019",
        details: &[
            d(143, 123, P, "         949.77"),
            d(144, 135, P, "         129.33"),
            d(145, 136, P, "         241.22"),
            d(146, 195, C, "-         71.66"),
            d(147, 217, P, "         329.99"),
            d(148, 233, P, "         988.88"),
        ],
        total: Some((149, "+      2,567.53")),
    },
    // audit: FIXRPT-017
    Block {
        account: "00000000003",
        details: &[
            d(151, 27, P, "         999.77"),
            d(152, 111, P, "         115.11"),
            d(153, 167, C, "-        880.22"),
            d(154, 204, P, "         554.22"),
            d(155, 244, P, "         227.22"),
            d(156, 293, P, "         694.55"),
        ],
        total: Some((157, "+      1,710.65")),
    },
    // audit: FIXRPT-018
    Block {
        account: "00000000013",
        details: &[
            d(159, 7, C, "-         56.77"),
            d(160, 59, P, "         326.44"),
            d(167, 169, P, "         403.66"),
            d(168, 259, P, "         702.22"),
            d(169, 279, P, "         759.22"),
            d(170, 282, P, "         684.33"),
        ],
        total: Some((171, "+      2,819.10")),
    },
    // audit: FIXRPT-019
    Block {
        account: "00000000034",
        details: &[
            d(173, 60, P, "         299.33"),
            d(174, 117, P, "         351.66"),
            d(175, 144, P, "         820.11"),
            d(176, 213, P, "         697.44"),
            d(177, 239, P, "         405.33"),
            d(178, 258, C, "-        457.55"),
        ],
        total: Some((179, "+      2,116.32")),
    },
    // audit: FIXRPT-020
    Block {
        account: "00000000036",
        details: &[
            d(187, 19, P, "         502.66"),
            d(188, 81, P, "         496.33"),
            d(189, 152, P, "         952.55"),
            d(190, 222, P, "         358.22"),
            d(191, 249, C, "-        439.33"),
        ],
        total: Some((192, "+      1,870.43")),
    },
    // audit: FIXRPT-021
    Block {
        account: "00000000007",
        details: &[
            d(194, 1, P, "         504.77"),
            d(195, 114, P, "         428.33"),
            d(196, 154, C, "-        579.88"),
            d(197, 243, P, "         319.00"),
            d(198, 251, P, "          34.99"),
        ],
        total: Some((199, "+        707.21")),
    },
    // audit: FIXRPT-022
    Block {
        account: "00000000021",
        details: &[
            d(207, 88, P, "          40.00"),
            d(208, 140, P, "         174.66"),
            d(209, 146, P, "         584.88"),
            d(210, 202, P, "          89.11"),
            d(211, 256, C, "-         41.77"),
        ],
        total: Some((212, "+        846.88")),
    },
    // audit: FIXRPT-023
    Block {
        account: "00000000046",
        details: &[
            d(214, 5, P, "         454.66"),
            d(215, 115, P, "         884.99"),
            d(216, 141, P, "         913.88"),
            d(217, 175, P, "         430.55"),
            d(218, 210, C, "-        435.00"),
            d(219, 278, P, "         893.33"),
        ],
        total: Some((220, "+      3,142.41")),
    },
    // audit: FIXRPT-024
    Block {
        account: "00000000018",
        details: &[
            d(222, 39, P, "          46.66"),
            d(223, 110, P, "         958.33"),
            d(224, 116, P, "         860.77"),
            d(225, 163, C, "-        113.11"),
            d(226, 252, P, "         796.00"),
        ],
        total: Some((227, "+      2,548.65")),
    },
    // audit: FIXRPT-025
    Block {
        account: "00000000047",
        details: &[
            d(229, 22, P, "         779.33"),
            d(230, 62, P, "         479.22"),
            d(231, 76, P, "         339.55"),
            d(232, 94, C, "-        585.44"),
        ],
        total: Some((233, "+      1,012.66")),
    },
    // audit: FIXRPT-026
    Block {
        account: "00000000042",
        details: &[
            d(235, 77, P, "         905.00"),
            d(236, 90, P, "          89.99"),
            d(237, 107, P, "         552.77"),
            d(238, 124, C, "-        501.44"),
            d(239, 125, P, "         214.77"),
            d(240, 296, P, "         402.22"),
        ],
        total: Some((241, "+      1,663.31")),
    },
    // audit: FIXRPT-027
    Block {
        account: "00000000005",
        details: &[
            d(243, 3, P, "          67.88"),
            d(244, 70, C, "-        428.99"),
            d(245, 78, P, "         346.99"),
            d(246, 119, P, "         254.77"),
            d(247, 179, P, "         161.77"),
            d(248, 185, P, "         435.44"),
        ],
        total: Some((249, "+        837.86")),
    },
    // audit: FIXRPT-028
    Block {
        account: "00000000015",
        details: &[
            d(251, 25, P, "         660.11"),
            d(252, 53, C, "-        358.44"),
            d(253, 172, P, "         850.22"),
            d(254, 220, P, "         983.55"),
            d(255, 246, P, "         803.99"),
            d(256, 295, P, "         784.11"),
        ],
        total: Some((257, "+      3,723.54")),
    },
    // audit: FIXRPT-029
    Block {
        account: "00000000048",
        details: &[
            d(259, 9, C, "-        535.88"),
            d(260, 82, P, "         526.11"),
            d(267, 86, P, "         764.22"),
            d(268, 285, P, "         222.44"),
        ],
        total: Some((269, "+        976.89")),
    },
    // audit: FIXRPT-030
    Block {
        account: "00000000030",
        details: &[
            d(271, 14, P, "          29.44"),
            d(272, 72, C, "-        930.33"),
        ],
        total: Some((273, "-        900.89")),
    },
    // audit: FIXRPT-031
    Block {
        account: "00000000028",
        details: &[
            d(275, 47, P, "         256.00"),
            d(276, 64, P, "         553.00"),
            d(277, 113, C, "-         75.22"),
        ],
        total: Some((278, "+        733.78")),
    },
    // audit: FIXRPT-032
    Block {
        account: "00000000016",
        details: &[
            d(280, 49, P, "          19.99"),
            d(287, 56, P, "          59.33"),
            d(288, 149, P, "         209.00"),
            d(289, 207, P, "         633.55"),
            d(290, 209, P, "         425.00"),
            d(291, 262, C, "-        270.99"),
        ],
        total: Some((292, "+      1,075.88")),
    },
    // audit: FIXRPT-033
    Block {
        account: "00000000033",
        details: &[
            d(294, 34, P, "         769.55"),
            d(295, 73, C, "-        445.55"),
            d(296, 127, P, "         995.22"),
            d(297, 133, P, "         793.22"),
            d(298, 166, P, "         202.44"),
            d(299, 203, P, "         629.55"),
        ],
        total: Some((300, "+      2,944.43")),
    },
    // audit: FIXRPT-034
    Block {
        account: "00000000031",
        details: &[
            d(302, 67, P, "         495.66"),
            d(303, 245, P, "         168.66"),
            d(304, 255, C, "-        362.22"),
        ],
        total: Some((305, "+        302.10")),
    },
    // audit: FIXRPT-035
    Block {
        account: "00000000043",
        details: &[
            d(307, 55, C, "-        243.00"),
            d(308, 93, P, "         936.22"),
            d(309, 128, P, "         674.99"),
            d(310, 186, P, "         568.88"),
            d(311, 205, P, "         744.22"),
            d(312, 228, P, "          13.88"),
        ],
        total: Some((313, "+      2,695.19")),
    },
    // audit: FIXRPT-036
    Block {
        account: "00000000032",
        details: &[
            d(315, 12, P, "         250.22"),
            d(316, 18, P, "         649.33"),
            d(317, 31, P, "         274.00"),
            d(318, 103, C, "-        742.99"),
        ],
        total: Some((319, "+        430.56")),
    },
    // audit: FIXRPT-037
    Block {
        account: "00000000029",
        details: &[
            d(327, 40, P, "         570.66"),
            d(328, 51, P, "         989.77"),
            d(329, 106, C, "-        852.33"),
            d(330, 161, P, "         699.44"),
            d(331, 189, P, "         744.11"),
            d(332, 191, P, "         166.99"),
        ],
        total: Some((333, "+      2,318.64")),
    },
    // audit: FIXRPT-038
    Block {
        account: "00000000045",
        details: &[
            d(335, 6, P, "         849.99"),
            d(336, 33, P, "         840.55"),
            d(337, 95, P, "         717.00"),
            d(338, 112, C, "-        344.77"),
        ],
        total: Some((339, "+      2,062.77")),
    },
    // audit: FIXRPT-039
    Block {
        account: "00000000011",
        details: &[
            d(347, 57, C, "-        744.77"),
            d(348, 71, P, "         160.99"),
            d(349, 108, P, "         446.00"),
            d(350, 183, P, "         621.22"),
            d(351, 212, P, "         191.00"),
            d(352, 230, P, "         653.55"),
        ],
        total: Some((353, "+      1,327.99")),
    },
    // audit: FIXRPT-040
    Block {
        account: "00000000038",
        details: &[
            d(355, 74, P, "         816.77"),
            d(356, 80, P, "         257.55"),
            d(357, 84, P, "         461.99"),
            d(358, 92, P, "         706.11"),
            d(359, 96, P, "         745.33"),
            d(360, 200, C, "-        538.22"),
        ],
        total: Some((361, "+      2,449.53")),
    },
    // audit: FIXRPT-041
    Block {
        account: "00000000026",
        details: &[
            d(363, 4, P, "         281.77"),
            d(364, 8, P, "         373.66"),
            d(365, 45, P, "         864.44"),
            d(366, 165, C, "-        372.00"),
        ],
        total: Some((367, "+      1,147.87")),
    },
    // audit: FIXRPT-042
    Block {
        account: "00000000023",
        details: &[
            d(369, 98, P, "         948.22"),
            d(370, 134, P, "         408.88"),
            d(371, 197, P, "          21.11"),
            d(372, 237, P, "          65.44"),
            d(373, 287, P, "         437.77"),
            d(374, 294, C, "-        835.44"),
        ],
        total: Some((375, "+      1,045.98")),
    },
    // audit: FIXRPT-043
    Block {
        account: "00000000049",
        details: &[
            d(377, 42, P, "         559.88"),
            d(378, 91, P, "          49.55"),
            d(379, 131, C, "-        962.77"),
            d(380, 241, P, "         270.55"),
            d(387, 264, P, "         198.66"),
            d(388, 297, P, "         917.44"),
        ],
        total: Some((389, "+      1,033.31")),
    },
    // audit: FIXRPT-044
    Block {
        account: "00000000014",
        details: &[
            d(391, 48, P, "         985.33"),
            d(392, 104, P, "         990.88"),
            d(393, 170, P, "         195.44"),
            d(394, 211, C, "-        710.66"),
        ],
        total: Some((395, "+      1,460.99")),
    },
    // audit: FIXRPT-045
    Block {
        account: "00000000008",
        details: &[
            d(397, 26, P, "         325.00"),
            d(398, 28, P, "         482.44"),
            d(399, 65, C, "-        835.11"),
            d(400, 97, P, "         728.77"),
            d(407, 192, P, "          95.99"),
            d(408, 232, P, "         735.99"),
        ],
        total: Some((409, "+      1,533.08")),
    },
    // audit: FIXRPT-046
    Block {
        account: "00000000025",
        details: &[
            d(411, 46, P, "         183.22"),
            d(412, 100, P, "         859.44"),
            d(413, 150, C, "-        907.00"),
            d(414, 216, P, "         192.00"),
            d(415, 238, P, "         309.44"),
            d(416, 261, P, "          45.66"),
        ],
        total: Some((417, "+        682.76")),
    },
    // audit: FIXRPT-047
    Block {
        account: "00000000017",
        details: &[
            d(419, 69, P, "         343.77"),
            d(420, 177, C, "-        998.33"),
        ],
        total: Some((421, "-        654.56")),
    },
    // audit: FIXRPT-048
    Block {
        account: "00000000009",
        details: &[
            d(423, 10, P, "         416.11"),
            d(424, 68, P, "         851.33"),
            d(425, 132, P, "         851.22"),
            d(426, 148, P, "         649.00"),
            d(427, 229, P, "         140.00"),
            d(428, 284, C, "-        260.11"),
        ],
        total: Some((429, "+      2,647.55")),
    },
    // audit: FIXRPT-049
    Block {
        account: "00000000001",
        details: &[
            d(431, 156, P, "          81.44"),
            d(432, 164, C, "-         70.77"),
            d(433, 174, P, "         484.99"),
            d(434, 199, P, "         598.44"),
        ],
        total: Some((435, "+      1,094.10")),
    },
    // audit: FIXRPT-050
    Block {
        account: "00000000040",
        details: &[
            d(437, 30, P, "         356.22"),
            d(438, 180, P, "         237.44"),
            d(439, 215, P, "          81.44"),
            d(440, 221, C, "-         25.99"),
            d(447, 270, P, "         194.33"),
            d(448, 298, P, "         765.66"),
        ],
        total: None,
    },
];

/// Tells apart the scratch directories of the tests of one process.
static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// A directory of its own under the system temporary directory, removed with its content when
/// dropped: the output directories and the input file of one test's runs.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, Box<dyn Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "carddemo-batch-tranrept-test-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// `CARDDEMO_VAR_DIR` of every run, the root of `dalyrejs` and `reports`.
    fn var_dir(&self) -> PathBuf {
        self.0.join("var")
    }

    /// Generation `generation` of `TRANREPT` in the reports directory, as the job builds its
    /// path from `CARDDEMO_VAR_DIR` (CBTRN03C-032).
    fn report(&self, generation: u16) -> PathBuf {
        self.var_dir()
            .join("reports")
            .join(format!("TRANREPT.G{generation:04}"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The events a run logs, its `RUST_LOG`.
#[derive(Clone, Copy)]
enum Log {
    /// Error events only, so standard output carries the written paths and little else.
    Errors,
    /// Every `INFO` event, so the job log reaches standard output.
    JobLog,
}

impl Log {
    fn directive(self) -> &'static str {
        match self {
            Log::Errors => "error",
            Log::JobLog => "info",
        }
    }
}

/// Runs `carddemo-batch` with `args` against `guard`'s database, as [`run_against`] does.
async fn run_batch(
    guard: &PostgresGuard,
    scratch: &Scratch,
    args: &[&OsStr],
    log: Log,
) -> Result<Output, Box<dyn Error>> {
    run_against(guard.database_url(), scratch, args, log).await
}

/// Runs `carddemo-batch` with `args` from `modern/`, the working directory of every binary,
/// against the database `database_url` names, with its output directories in `scratch`, the
/// fixtures read in place and `log` as `RUST_LOG`. The rest of the environment is inherited, so
/// the coverage profile variables reach the binary. Span export is off, so every event is one
/// JSON line and the other lines of standard output are the written paths.
async fn run_against(
    database_url: &str,
    scratch: &Scratch,
    args: &[&OsStr],
    log: Log,
) -> Result<Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command.args(args);
    configure(&mut command, database_url, scratch, log);
    Ok(test_support::run_child(command, test_support::CHILD_DEADLINE).await?)
}

/// `/bin/sh` set to run `program` once it has limited the size of every file `program` writes to
/// `blocks` of its `ulimit -f` blocks.
///
/// The shell ignores `SIGXFSZ` before it replaces itself with `program`, and an ignored signal
/// stays ignored across `exec`, so a write past the limit fails with `EFBIG` instead of killing
/// the process: the job meets it as a failed write. Standard output and standard error are
/// pipes, which the limit does not reach.
fn size_limited(blocks: usize, program: &str) -> Command {
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg(format!(
            "trap '' XFSZ; ulimit -f {blocks} && exec \"$0\" \"$@\""
        ))
        .arg(program);
    command
}

/// The bytes [`ulimit_block_len`] offers its probe file, more than one block of any shell.
const PROBE_LEN: usize = 4_096;

/// The bytes of one `ulimit -f` block of `/bin/sh`, measured rather than assumed
/// ([`SizeWindow`]). Under [`size_limited`] to one block, `dd` copies [`PROBE_LEN`] bytes into a
/// file in `scratch`, outside `CARDDEMO_VAR_DIR`, one byte per write, so the file stops at the
/// limit whether the system shortens the write that crosses it or refuses it whole, and its
/// length is the block. `dd` fails at the refused write, so its status is not read; a file that
/// holds none or all of the bytes has measured no block.
async fn ulimit_block_len(scratch: &Scratch) -> Result<usize, Box<dyn Error>> {
    let probe = scratch.path().join("ulimit-block");
    let mut target = std::ffi::OsString::from("of=");
    target.push(&probe);
    let mut command = size_limited(1, "dd");
    command
        .arg("if=/dev/zero")
        .arg(target)
        .arg("bs=1")
        .arg(format!("count={PROBE_LEN}"));
    let output = test_support::run_child(command, test_support::CHILD_DEADLINE).await?;
    let len = fs::metadata(&probe).map_or(Ok(0), |metadata| usize::try_from(metadata.len()))?;
    if len == 0 || len >= PROBE_LEN {
        return Err(format!(
            "`ulimit -f 1` in /bin/sh let dd write {len} of {PROBE_LEN} bytes: {}",
            String::from_utf8_lossy(&output.stderr).trim_end()
        )
        .into());
    }
    Ok(len)
}

/// The fewest `ulimit -f` blocks of `block_len` bytes whose bytes lie strictly inside `window`.
fn size_limit_blocks(window: SizeWindow, block_len: usize) -> Result<usize, Box<dyn Error>> {
    let blocks = window
        .above
        .checked_div(block_len)
        .and_then(|whole| whole.checked_add(1))
        .ok_or("a `ulimit -f` block of no bytes")?;
    let limit = blocks
        .checked_mul(block_len)
        .ok_or("a file-size limit past usize")?;
    if limit <= window.above || limit >= window.below {
        return Err(format!(
            "no whole number of {block_len}-byte `ulimit -f` blocks lies between {} and {} bytes",
            window.above, window.below
        )
        .into());
    }
    Ok(blocks)
}

/// Runs `carddemo-batch` with `args` against `guard`'s database, logging its job log, through
/// [`size_limited`] to the fewest blocks of `/bin/sh` whose bytes lie inside `window`, so the
/// limit lies inside it whichever block size the shell counts in.
async fn run_size_limited(
    guard: &PostgresGuard,
    scratch: &Scratch,
    args: &[&OsStr],
    window: SizeWindow,
) -> Result<Output, Box<dyn Error>> {
    let blocks = size_limit_blocks(window, ulimit_block_len(scratch).await?)?;
    let mut command = size_limited(blocks, env!("CARGO_BIN_EXE_carddemo-batch"));
    command.args(args);
    configure(&mut command, guard.database_url(), scratch, Log::JobLog);
    // Under `cargo llvm-cov` the binary is instrumented and writes its profile when it exits.
    // Under the limit that profile would be cut short and break the merged report, so this run
    // writes it to `/dev/null`, a device the limit does not reach, and adds no coverage.
    command.env("LLVM_PROFILE_FILE", "/dev/null");
    Ok(test_support::run_child(command, test_support::CHILD_DEADLINE).await?)
}

/// The environment of every run: `modern/` as working directory, the database `database_url`
/// names, the output directories in `scratch`, the fixtures read in place, `log` as `RUST_LOG`,
/// and span export off.
fn configure(command: &mut Command, database_url: &str, scratch: &Scratch, log: Log) {
    command
        .current_dir(test_support::modern_dir())
        .env("DATABASE_URL", database_url)
        .env("CARDDEMO_VAR_DIR", scratch.var_dir())
        .env("CARDDEMO_STATEMENT_DIR", scratch.path().join("statements"))
        .env("CARDDEMO_INTRDR_DIR", scratch.path().join("jobs"))
        .env("CARDDEMO_FIXTURE_DIR", fixture_dir())
        .env("RUST_LOG", log.directive())
        .env_remove("CARDDEMO_OTEL_EXPORT");
}

/// The job log of a run: the message of every event of the job's module on standard output, in
/// order, which are the program's lines and, on a failure path, the event naming the failed call
/// before them. The messages hold no quote or backslash, the `TRAN-RECORD` line included, whose
/// free-text fields show as `*`, so each ends at the first quote after its key.
fn job_log(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with('{') && line.contains(JOB_LOG_TARGET))
        .filter_map(|event| {
            let (_, rest) = event.split_once(MESSAGE_KEY)?;
            rest.split_once('"').map(|(message, _)| message.to_owned())
        })
        .collect()
}

/// Whether the line of standard output right after the job-log event `END OF EXECUTION OF
/// PROGRAM CBTRN03C` is `path` (CBTRN03C-032).
fn path_follows_end_line(output: &Output, path: &Path) -> bool {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    let end_message = format!("{MESSAGE_KEY}{END_LINE}\"");
    lines
        .iter()
        .position(|line| line.contains(JOB_LOG_TARGET) && line.contains(&end_message))
        .and_then(|end| lines.get(end + 1))
        .is_some_and(|line| Path::new(line) == path)
}

/// The `tranrept` arguments of the window `start` to `end`.
fn tranrept_args<'a>(start: &'a str, end: &'a str) -> [&'a OsStr; 5] {
    [
        OsStr::new("tranrept"),
        OsStr::new("--start-date"),
        OsStr::new(start),
        OsStr::new("--end-date"),
        OsStr::new(end),
    ]
}

/// Checks the job log of a run that abended on `failure_line` after the start line: no end line,
/// and its last three lines `failure_line`, a status line and `ABENDING PROGRAM`
/// (CBTRN03C-035, CBTRN03C-036). `HALTED(X-TRANREPT-H3)`: the status line's four characters
/// have no counterpart, so only its label and length are checked and the characters are reported
/// `BLOCKED` for `rule`. Failures name line positions only.
fn assert_abend_log(log: &[String], failure_line: &str, rule: &str) {
    // audit: CBTRN03C-006
    assert!(
        log.first().is_some_and(|line| line == START_LINE),
        "{rule}: the first job-log line is not the start line"
    );
    // audit: CBTRN03C-035
    assert!(
        !log.iter().any(|line| line == END_LINE),
        "{rule}: an abended run showed the end line"
    );
    let tail = log.len().checked_sub(3).and_then(|start| log.get(start..));
    let Some([failure, status, abend]) = tail else {
        panic!("{rule}: the job log has {} lines, fewer than 3", log.len());
    };
    assert!(
        failure == failure_line,
        "{rule}: the third line from the end is not the failure line"
    );
    // audit: CBTRN03C-036
    assert!(
        status.len() == STATUS_LINE_LEN && status.starts_with(STATUS_LABEL),
        "{rule}: the line after the failure line is not a status line"
    );
    assert!(
        abend == ABEND_LINE,
        "{rule}: the last job-log line is not ABENDING PROGRAM"
    );
    println!("BLOCKED {rule} (X-TRANREPT-H3): the four status characters of the failure");
}

/// The first line of a run's standard error, the job's one line. Only that line is compared after
/// a failure the test provokes, since an instrumented binary, such as one under a file-size limit,
/// may add lines of its own after it.
fn first_stderr_line(output: &Output) -> String {
    stderr(output).lines().next().unwrap_or_default().to_owned()
}

/// Where a [`Relay`] cuts the one connection whose row description names a column.
#[derive(Clone, Copy)]
enum Cut {
    /// After the first data row that follows the description, which is forwarded: the
    /// statement's first row reaches the binary, and its next fetch finds the connection closed.
    AfterFirstRow(&'static str),
    /// At the description itself, which is not forwarded: the statement fails before it answers.
    AtDescription(&'static str),
}

impl Cut {
    /// The column whose row description the cut waits for.
    fn column(self) -> &'static str {
        match self {
            Cut::AfterFirstRow(column) | Cut::AtDescription(column) => column,
        }
    }
}

/// The type bytes of the backend messages the relay looks at (protocol 3.0): `RowDescription`
/// and `DataRow`.
const ROW_DESCRIPTION: u8 = b'T';
const DATA_ROW: u8 = b'D';
/// Bytes of a message's length field, which counts itself.
const LENGTH_LEN: usize = 4;
/// Bytes after each field name of a `RowDescription`: the table OID (4), the column number (2),
/// the type OID (4), the type size (2), the type modifier (4) and the format code (2).
const FIELD_TRAILER_LEN: usize = 18;
/// How long the relay waits for the container to accept a connection.
const UPSTREAM_TIMEOUT: Duration = Duration::from_secs(10);

/// A TCP relay on `127.0.0.1` between `carddemo-batch` and the test's `PostgreSQL` container. It
/// forwards every connection unchanged except the first whose row description names the column
/// of its [`Cut`], which it closes where the cut says, as a lost connection ends.
///
/// The binary connects with `sslmode=disable`, so the container's side of every connection is a
/// sequence of typed messages from its first byte: a type byte, a big-endian `u32` length that
/// counts itself, then the body. The binary's side is copied as it is. Each connection is served
/// by two threads; dropping the relay wakes its accept loop, shuts every socket down and joins
/// every thread, so no relay outlives its test.
struct Relay {
    /// The port the relay listens on.
    port: u16,
    /// The `DATABASE_URL` that points the binary at the relay.
    database_url: String,
    /// What the relay's threads share.
    state: Arc<RelayState>,
    /// The accept loop.
    accept: Option<JoinHandle<()>>,
}

/// What the threads of a [`Relay`] share.
struct RelayState {
    /// The container's `host:port`.
    upstream: String,
    /// Where to cut.
    cut: Cut,
    /// Set when the relay is dropped: the accept loop ends at its next connection.
    stopping: AtomicBool,
    /// Set once a connection has been chosen for the cut, so that no other one is cut.
    cut_made: AtomicBool,
    /// A handle on every socket, shut down when the relay is dropped.
    sockets: Mutex<Vec<TcpStream>>,
    /// The threads of every connection, joined when the relay is dropped.
    threads: Mutex<Vec<JoinHandle<()>>>,
}

/// `mutex` locked, even if a relay thread panicked while it held the lock: a list of sockets or
/// threads stays valid whatever its holder did.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Relay {
    /// Starts a relay to the container that `database_url`, as `PostgresGuard::database_url`
    /// gives it, names, cutting as `cut` says.
    fn start(database_url: &str, cut: Cut) -> Result<Self, Box<dyn Error>> {
        let (scheme, rest) = database_url
            .split_once("://")
            .ok_or("the database URL has no scheme")?;
        let (credentials, rest) = rest
            .split_once('@')
            .ok_or("the database URL has no credentials")?;
        let (upstream, database) = rest
            .split_once('/')
            .ok_or("the database URL names no database")?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let port = listener.local_addr()?.port();
        let state = Arc::new(RelayState {
            upstream: upstream.to_owned(),
            cut,
            stopping: AtomicBool::new(false),
            cut_made: AtomicBool::new(false),
            sockets: Mutex::new(Vec::new()),
            threads: Mutex::new(Vec::new()),
        });
        let accepting = Arc::clone(&state);
        let accept = thread::spawn(move || accept_connections(&accepting, &listener));
        Ok(Self {
            port,
            database_url: format!(
                "{scheme}://{credentials}@127.0.0.1:{port}/{database}?sslmode=disable"
            ),
            state,
            accept: Some(accept),
        })
    }

    /// The `DATABASE_URL` that points the binary at the relay.
    fn database_url(&self) -> &str {
        &self.database_url
    }

    /// Whether the relay has cut a connection.
    fn cut_made(&self) -> bool {
        self.state.cut_made.load(Ordering::SeqCst)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.state.stopping.store(true, Ordering::SeqCst);
        // A connection of the relay's own wakes the accept loop, which sees `stopping` and ends.
        // Should none get through, the loop is left blocked rather than waited for.
        let woken = TcpStream::connect(("127.0.0.1", self.port)).is_ok();
        if let Some(accept) = self.accept.take().filter(|_| woken) {
            // A thread that panicked has nothing left to clean up, and a drop must not panic.
            let _ = accept.join();
        }
        for socket in lock(&self.state.sockets).drain(..) {
            // A socket the other side has closed already is no failure.
            let _ = socket.shutdown(Shutdown::Both);
        }
        let threads: Vec<JoinHandle<()>> = lock(&self.state.threads).drain(..).collect();
        for relaying in threads {
            let _ = relaying.join();
        }
    }
}

/// Serves every connection `listener` accepts until the relay stops.
fn accept_connections(state: &Arc<RelayState>, listener: &TcpListener) {
    for client in listener.incoming() {
        if state.stopping.load(Ordering::SeqCst) {
            break;
        }
        // A failed accept leaves the binary to connect again; the relay keeps listening.
        if let Ok(client) = client {
            serve(state, client);
        }
    }
}

/// Connects `client` to the container and starts the two threads that relay it. A connection
/// the container does not accept is closed at once, which the binary meets as a refused one.
fn serve(state: &Arc<RelayState>, client: TcpStream) {
    let server = state
        .upstream
        .to_socket_addrs()
        .ok()
        .and_then(|mut addresses| addresses.next())
        .and_then(|address| TcpStream::connect_timeout(&address, UPSTREAM_TIMEOUT).ok());
    let Some(server) = server else {
        let _ = client.shutdown(Shutdown::Both);
        return;
    };
    let handles = (
        client.try_clone(),
        client.try_clone(),
        server.try_clone(),
        server.try_clone(),
    );
    let (Ok(mut client_reader), Ok(client_kept), Ok(mut server_writer), Ok(server_kept)) = handles
    else {
        let _ = client.shutdown(Shutdown::Both);
        let _ = server.shutdown(Shutdown::Both);
        return;
    };
    lock(&state.sockets).extend([client_kept, server_kept]);
    let upward = thread::spawn(move || forward_client(&mut client_reader, &mut server_writer));
    let relaying = Arc::clone(state);
    let mut client_writer = client;
    let downward = thread::spawn(move || forward_server(&relaying, server, &mut client_writer));
    lock(&state.threads).extend([upward, downward]);
}

/// Copies what the binary sends to the container until either side ends, then ends the
/// container's side for writing, so the container sees the binary's end.
fn forward_client(client: &mut TcpStream, server: &mut TcpStream) {
    // An error ends the copy as the end of the stream does; a relay thread has no one to tell.
    let _ = io::copy(client, server);
    let _ = server.shutdown(Shutdown::Write);
}

/// Forwards the container's messages to the binary one at a time until the container ends or
/// the cut is made, then ends the binary's side for writing, so the binary reads the end of the
/// stream after the last message forwarded. A cut also closes the container's side.
fn forward_server(state: &RelayState, server: TcpStream, client: &mut TcpStream) {
    let mut reader = BufReader::new(server);
    // The connection was chosen for a cut after its next data row.
    let mut armed = false;
    let mut cut = false;
    while let Ok(message) = read_message(&mut reader) {
        let kind = message.first().copied();
        if !armed && kind == Some(ROW_DESCRIPTION) && describes(&message, state.cut.column()) {
            let chosen = state
                .cut_made
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok();
            match state.cut {
                Cut::AtDescription(_) if chosen => {
                    cut = true;
                    break;
                }
                Cut::AtDescription(_) => {}
                Cut::AfterFirstRow(_) => armed = chosen,
            }
        }
        if client.write_all(&message).is_err() {
            break;
        }
        if armed && kind == Some(DATA_ROW) {
            cut = true;
            break;
        }
    }
    if cut {
        let _ = reader.get_ref().shutdown(Shutdown::Both);
    }
    let _ = client.shutdown(Shutdown::Write);
}

/// The next backend message `reader` holds, whole: its type byte, its length and its body.
///
/// # Errors
///
/// The read's error, `UnexpectedEof` at the end of the stream included, or `InvalidData` for a
/// length shorter than its own field.
fn read_message(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut header = [0; 1 + LENGTH_LEN];
    reader.read_exact(&mut header)?;
    let [_, length @ ..] = header;
    let body_len = usize::try_from(u32::from_be_bytes(length))
        .ok()
        .and_then(|length| length.checked_sub(LENGTH_LEN))
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidData))?;
    let mut body = vec![0; body_len];
    reader.read_exact(&mut body)?;
    let mut message = header.to_vec();
    message.append(&mut body);
    Ok(message)
}

/// Whether the `RowDescription` `message` describes a field named `column`: after the header, a
/// big-endian `i16` field count, then each field's NUL-terminated name and its trailer.
fn describes(message: &[u8], column: &str) -> bool {
    let Some((count, mut fields)) = message
        .get(1 + LENGTH_LEN..)
        .and_then(|body| body.split_first_chunk::<2>())
    else {
        return false;
    };
    for _ in 0..i16::from_be_bytes(*count) {
        let Some(end) = fields.iter().position(|&byte| byte == 0) else {
            return false;
        };
        let (name, rest) = fields.split_at(end);
        if name == column.as_bytes() {
            return true;
        }
        let Some(rest) = rest.get(1 + FIELD_TRAILER_LEN..) else {
            return false;
        };
        fields = rest;
    }
    false
}

/// A run's standard error, which `carddemo-batch` keeps free of records and protected values.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Checks that no line of a run's standard output or standard error, every tracing event
/// included, holds one of the 50 fixture card numbers in full (CBTRN03C-039; Cardholder Data
/// Redaction). The numbers are search needles only: a failure names the stream and its line
/// numbers, never a line or a number.
fn assert_no_fixture_card_number(output: &Output) -> TestResult {
    let cards = fixture_card_numbers()?;
    let streams = [
        ("standard output", &output.stdout),
        ("standard error", &output.stderr),
    ];
    for (stream, bytes) in streams {
        let holding: Vec<usize> = bytes
            .split(|&byte| byte == b'\n')
            .zip(1_usize..)
            .filter(|(line, _)| {
                cards.iter().any(|card| {
                    line.windows(card.len())
                        .any(|window| window == card.as_bytes())
                })
            })
            .map(|(_, number)| number)
            .collect();
        // audit: CBTRN03C-039
        assert!(
            holding.is_empty(),
            "{stream} lines holding a full fixture card number: {holding:?}"
        );
    }
    Ok(())
}

/// The paths a run printed: its standard-output lines that are not JSON tracing events.
fn written_paths(output: &Output) -> Vec<PathBuf> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('{'))
        .map(PathBuf::from)
        .collect()
}

/// The records of `bytes`, each `len` bytes followed by LF, without their LF.
fn fixed_records(bytes: &[u8], len: usize) -> Result<Vec<&[u8]>, Box<dyn Error>> {
    let lines = bytes.chunks_exact(len + 1);
    let partial = lines.remainder().len();
    if partial != 0 {
        return Err(format!("the file ends in a partial record of {partial} bytes").into());
    }
    lines
        .zip(1_usize..)
        .map(|(line, number)| match line.split_at(len) {
            (record, b"\n") => Ok(record),
            _ => Err(format!("record {number} is not followed by LF").into()),
        })
        .collect()
}

/// Every entry under `CARDDEMO_VAR_DIR` that is not a directory, as its path relative to it with
/// `/` separators, in byte order.
fn var_files(scratch: &Scratch) -> Result<Vec<String>, Box<dyn Error>> {
    let root = scratch.var_dir();
    let mut pending = vec![root.clone()];
    let mut files = Vec::new();
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative: Vec<String> = path
                    .strip_prefix(&root)?
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy().into_owned())
                    .collect();
                files.push(relative.join("/"));
            }
        }
    }
    files.sort_unstable();
    Ok(files)
}

/// Runs `posttran` over `dailytran.txt`, read in place: the POSTED run of `fixtures.md` but for
/// its clock. Checks the audited exit status, 4, because 38 records are rejected.
async fn post_fixture_transactions(guard: &PostgresGuard, scratch: &Scratch) -> TestResult {
    let dalytran = fixture_path("dailytran.txt");
    let args = [
        OsStr::new("posttran"),
        OsStr::new("--dalytran"),
        dalytran.as_os_str(),
    ];
    let output = run_batch(guard, scratch, &args, Log::Errors).await?;
    // audit: FIXPOST-903
    assert_eq!(
        output.status.code(),
        Some(4),
        "posttran: {}",
        stderr(&output)
    );
    Ok(())
}

/// What a `tranrept` run that ended normally left: its generation file and its job log.
struct ReportRun {
    report: Vec<u8>,
    job_log: Vec<String>,
}

/// Runs `tranrept` over `window`, or with its default dates when `None`, logging its job log,
/// checks that nothing it printed holds a fixture card number in full, that it ended normally
/// and that it printed the path of generation `generation` alone, on the line after the end
/// line, and returns that file and the job log.
async fn run_report(
    guard: &PostgresGuard,
    scratch: &Scratch,
    window: Option<(&str, &str)>,
    generation: u16,
) -> Result<ReportRun, Box<dyn Error>> {
    let mut args = vec![OsStr::new("tranrept")];
    if let Some((start, end)) = window {
        args.extend([
            OsStr::new("--start-date"),
            OsStr::new(start),
            OsStr::new("--end-date"),
            OsStr::new(end),
        ]);
    }
    let output = run_batch(guard, scratch, &args, Log::JobLog).await?;
    // Checked first, as the exit-status message below quotes standard error.
    // audit: CBTRN03C-039
    assert_no_fixture_card_number(&output)?;
    // audit: CBTRN03C-034
    assert_eq!(
        output.status.code(),
        Some(0),
        "tranrept: {}",
        stderr(&output)
    );
    let path = scratch.report(generation);
    // audit: CBTRN03C-032
    assert_eq!(
        written_paths(&output),
        [path.clone()],
        "paths tranrept printed"
    );
    assert!(
        path_follows_end_line(&output, &path),
        "the path is not on the line after END OF EXECUTION OF PROGRAM CBTRN03C"
    );
    Ok(ReportRun {
        report: fs::read(path)?,
        job_log: job_log(&output),
    })
}

/// Writes `records`, each a 53-byte COBTUPDT record followed by LF (COBTUPDT-002,
/// COBTUPDT-003), to a file in `scratch` and returns its path.
fn mnttrdb2_input(scratch: &Scratch, records: &[String]) -> Result<PathBuf, Box<dyn Error>> {
    let mut bytes = Vec::new();
    for record in records {
        if record.len() != MAINTENANCE_RECORD_LEN {
            return Err(format!("a maintenance record of {} bytes", record.len()).into());
        }
        bytes.extend_from_slice(record.as_bytes());
        bytes.push(b'\n');
    }
    let path = scratch.path().join("mnttrdb2.txt");
    fs::write(&path, bytes)?;
    Ok(path)
}

/// The window `posttran`'s processing timestamps fall in: the day before today to the day
/// after, today read from the local clock the binary reads.
fn window_around_today() -> Result<(String, String), Box<dyn Error>> {
    let today = SystemClock.now().date();
    let start = today
        .checked_sub_days(Days::new(1))
        .ok_or("today has no day before it")?;
    let end = today
        .checked_add_days(Days::new(1))
        .ok_or("today has no day after it")?;
    Ok((iso_date(start), iso_date(end)))
}

/// `date` as an option value and as the report header shows it, `YYYY-MM-DD`.
fn iso_date(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// `DALYTRAN-ID`, the first 16 bytes, of each `dailytran.txt` line in file order: the entry at
/// index n - 1 is the `{T:n}` of the audit rows.
fn transaction_ids() -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let bytes = fs::read(fixture_path("dailytran.txt"))?;
    bytes
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .zip(1_usize..)
        .map(|(line, number)| {
            line.get(..16)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| format!("dailytran.txt:{number} is shorter than its ID").into())
        })
        .collect()
}

/// Each `dailytran.txt` line in file order, read in place, as the 350-byte transaction record it
/// is byte for byte: `CVTRA06Y` and `CVTRA05Y` share one layout.
fn daily_transactions() -> Result<Vec<TransactionRecord>, Box<dyn Error>> {
    let bytes = fs::read(fixture_path("dailytran.txt"))?;
    bytes
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| Ok(TransactionRecord::decode(line)?))
        .collect()
}

/// The `TRAN-RECORD` line of `row`, whose transaction ID is a card number, transcribed from
/// CBTRN03C-012: the row's 350 bytes with the six free-text fields as `*` and the card number as
/// twelve `*` and its last four digits; by Cardholder Data Redaction (CBTRN03C-039), the
/// transaction ID shows in that redacted form too.
fn id_redacted_tran_record_line(row: &TransactionRecord) -> Result<String, Box<dyn Error>> {
    let mut line = redacted_tran_record(row)?;
    line.get_mut(..CARD_MASK_LEN)
        .ok_or("a transaction record shorter than 350 bytes")?
        .fill(b'*');
    Ok(String::from_utf8(line)?)
}

/// The `TRAN-RECORD` line of `row`, transcribed from CBTRN03C-012: the row's 350 bytes with the
/// six free-text fields as `*` and the card number as twelve `*` and its last four digits.
fn redacted_tran_record(row: &TransactionRecord) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut line = row.encode();
    if line.len() != TRAN_RECORD_LEN {
        return Err(format!("a transaction record of {} bytes", line.len()).into());
    }
    for (start, end) in MASKED_BYTES
        .into_iter()
        .chain([(CARD_BYTES.0, CARD_BYTES.0 + CARD_MASK_LEN)])
    {
        line.get_mut(start..end)
            .ok_or("a transaction record shorter than 350 bytes")?
            .fill(b'*');
    }
    Ok(line)
}

/// The `TRAN-RECORD` line of each POSTED detail of [`BLOCKS`], by its transaction ID: the
/// detail's `dailytran.txt` record, read in place, in its redacted form. `posttran` moves every
/// field of the record but `DALYTRAN-PROC-TS` unchanged into the row it writes (CBTRN02C-012),
/// so every byte of the line but `TRAN-PROC-TS` is the record's (CBTRN03C-012).
fn expected_tran_record_lines() -> Result<HashMap<Vec<u8>, Vec<u8>>, Box<dyn Error>> {
    let daily = daily_transactions()?;
    let mut lines = HashMap::new();
    for detail in BLOCKS.iter().flat_map(|block| block.details) {
        let row = detail
            .tran_line
            .checked_sub(1)
            .and_then(|index| daily.get(index))
            .ok_or_else(|| format!("dailytran.txt has no line {}", detail.tran_line))?;
        let line = redacted_tran_record(row)?;
        let id = line
            .get(..TRAN_ID_LEN)
            .ok_or("a transaction record shorter than its ID")?
            .to_vec();
        if lines.insert(id, line).is_some() {
            return Err(format!("dailytran.txt:{} repeats a detail's ID", detail.tran_line).into());
        }
    }
    Ok(lines)
}

/// `text` padded on the right with spaces to a report record (CBTRN03C-032).
fn padded(text: &[u8]) -> Vec<u8> {
    let mut record = text.to_vec();
    record.resize(RECORD_LEN, b' ');
    record
}

/// A record of 133 dashes, `TRANSACTION-HEADER-2` (CBTRN03C-020, CBTRN03C-025, CBTRN03C-027).
fn dashes() -> Vec<u8> {
    vec![b'-'; RECORD_LEN]
}

/// Columns 1-97 of a total record: `label` and dots after it.
fn total_prefix(label: &str) -> Vec<u8> {
    let mut prefix = label.as_bytes().to_vec();
    prefix.resize(TOTAL_PREFIX_LEN, b'.');
    prefix
}

/// The total record `label` with the edited `amount` (CBTRN03C-025 to CBTRN03C-027).
fn total_record(label: &str, amount: &str) -> Vec<u8> {
    let mut text = total_prefix(label);
    text.extend_from_slice(amount.as_bytes());
    padded(&text)
}

/// Whether `record` is a total record `label` with an amount in the edited form: the check of a
/// total whose value X-FIXTURES-H3 blocks.
fn has_total_shape(record: &[u8], label: &str) -> bool {
    let prefix_end = TOTAL_PREFIX_LEN.min(record.len());
    let (prefix, rest) = record.split_at(prefix_end);
    let (amount, padding) = rest.split_at(AMOUNT_LEN.min(rest.len()));
    prefix == total_prefix(label).as_slice()
        && amount.len() == AMOUNT_LEN
        && amount.first().is_some_and(|sign| b"+- ".contains(sign))
        && amount
            .iter()
            .skip(1)
            .all(|byte| byte.is_ascii_digit() || b",. ".contains(byte))
        && padding.iter().all(|&byte| byte == b' ')
}

/// The four records of a header group whose name header shows `start` and `end`
/// (CBTRN03C-020, FIXRPT-902).
fn header_group(start: &str, end: &str) -> [Vec<u8>; 4] {
    [
        padded(format!("{NAME_HEADER}{start} to {end}").as_bytes()),
        padded(b""),
        padded(COLUMN_HEADER.as_bytes()),
        dashes(),
    ]
}

/// What the audit fixes for one report record.
enum Slot {
    /// A record fixed byte for byte.
    Exact(Vec<u8>),
    /// A detail of the block at this index of [`BLOCKS`], compared with the block's details as a
    /// set, because X-FIXTURES-H3 blocks their order.
    Detail(usize),
    /// A total record with this label, whose amount X-FIXTURES-H3 blocks.
    BlockedTotal(&'static str),
}

/// Puts `slot` at the 1-based `record` of `slots`, refusing a record outside the report or one
/// that already has an expectation.
fn place(slots: &mut [Option<Slot>], record: usize, slot: Slot) -> TestResult {
    let entry = record
        .checked_sub(1)
        .and_then(|index| slots.get_mut(index))
        .ok_or_else(|| format!("record {record} lies outside the report"))?;
    if entry.is_some() {
        return Err(format!("record {record} has two expectations").into());
    }
    *entry = Some(slot);
    Ok(())
}

/// The expectation of each of the 451 POSTED records whose header shows `start` and `end`, in
/// record order: header groups (FIXRPT-902), the card blocks with their account totals
/// (FIXRPT-001 to FIXRPT-050), the page totals (FIXRPT-904), a dash record after every total, and
/// the grand total (FIXRPT-905). Fails unless every record gets exactly one.
fn expected_layout(start: &str, end: &str) -> Result<Vec<Slot>, Box<dyn Error>> {
    let mut slots: Vec<Option<Slot>> = std::iter::repeat_with(|| None)
        .take(REPORT_RECORDS)
        .collect();
    for first in HEADER_GROUPS {
        for (record, text) in (first..).zip(header_group(start, end)) {
            place(&mut slots, record, Slot::Exact(text))?;
        }
    }
    for (index, block) in BLOCKS.iter().enumerate() {
        for detail in block.details {
            place(&mut slots, detail.record, Slot::Detail(index))?;
        }
        if let Some((record, amount)) = block.total {
            place(
                &mut slots,
                record,
                Slot::Exact(total_record(ACCOUNT_TOTAL_LABEL, amount)),
            )?;
            place(&mut slots, record + 1, Slot::Exact(dashes()))?;
        }
    }
    for (record, amount) in PAGE_TOTALS {
        let slot = amount.map_or(Slot::BlockedTotal(PAGE_TOTAL_LABEL), |amount| {
            Slot::Exact(total_record(PAGE_TOTAL_LABEL, amount))
        });
        place(&mut slots, record, slot)?;
        place(&mut slots, record + 1, Slot::Exact(dashes()))?;
    }
    place(
        &mut slots,
        GRAND_TOTAL_RECORD,
        Slot::BlockedTotal(GRAND_TOTAL_LABEL),
    )?;
    slots
        .into_iter()
        .zip(1_usize..)
        .map(|(slot, record)| {
            slot.ok_or_else(|| format!("record {record} has no expectation").into())
        })
        .collect()
}

/// The detail record of `detail` in `block`, its transaction ID taken from `ids`
/// (CBTRN03C-023, FIXRPT-903).
fn detail_record(
    block: &Block,
    detail: &Detail,
    ids: &[Vec<u8>],
) -> Result<Vec<u8>, Box<dyn Error>> {
    let id = detail
        .tran_line
        .checked_sub(1)
        .and_then(|index| ids.get(index))
        .ok_or_else(|| format!("dailytran.txt has no line {}", detail.tran_line))?;
    let mut text = id.clone();
    text.push(b' ');
    text.extend_from_slice(block.account.as_bytes());
    text.push(b' ');
    text.extend_from_slice(detail.kind.columns().as_bytes());
    text.extend_from_slice(detail.amount.as_bytes());
    Ok(padded(&text))
}

/// Checks `report` against the POSTED report of FIXRPT-001 to FIXRPT-905, its header showing
/// `start` and `end`. Records the audit fixes are compared byte for byte; each block's details
/// as a set at the block's positions; blocked totals by their record kind. Failures name record
/// and block numbers only.
fn assert_posted_report(report: &[u8], start: &str, end: &str) -> TestResult {
    let records = fixed_records(report, RECORD_LEN)?;
    // audit: FIXRPT-901
    assert_eq!(records.len(), REPORT_RECORDS, "report records");
    let layout = expected_layout(start, end)?;
    let mut details: Vec<Vec<&[u8]>> = BLOCKS.iter().map(|_| Vec::new()).collect();
    let mut differing = Vec::new();
    for ((record, slot), number) in records.iter().copied().zip(&layout).zip(1_usize..) {
        let matches = match slot {
            Slot::Exact(expected) => record == expected.as_slice(),
            Slot::Detail(block) => {
                details
                    .get_mut(*block)
                    .ok_or("a detail of no block")?
                    .push(record);
                true
            }
            Slot::BlockedTotal(label) => has_total_shape(record, label),
        };
        if !matches {
            differing.push(number);
        }
    }
    // BLOCKED(X-FIXTURES-H3, X-TRANREPT-H1): the page totals other than records 21, 201 and 341
    // and the grand total are checked for their kind only; their values get no parity assertion.
    // audit: FIXRPT-902
    // audit: FIXRPT-904
    // audit: FIXRPT-905
    // audit: CBTRN03C-019
    // audit: CBTRN03C-020
    // audit: CBTRN03C-021
    // audit: CBTRN03C-022
    // audit: CBTRN03C-025
    // audit: CBTRN03C-026
    // audit: CBTRN03C-027
    // audit: CBTRN03C-028
    // audit: CBTRN03C-030
    // audit: CBTRN03C-031
    assert!(
        differing.is_empty(),
        "records that differ from their audited bytes or kind: {differing:?}"
    );
    let ids = transaction_ids()?;
    for ((block, mut actual), number) in BLOCKS.iter().zip(details).zip(1_usize..) {
        let mut expected = block
            .details
            .iter()
            .map(|detail| detail_record(block, detail, &ids))
            .collect::<Result<Vec<_>, _>>()?;
        expected.sort_unstable();
        actual.sort_unstable();
        let positions: Vec<usize> = block.details.iter().map(|detail| detail.record).collect();
        // BLOCKED(X-FIXTURES-H3, X-TRANREPT-H1): the order of the details inside the block.
        // audit: FIXRPT-903
        // audit: CBTRN03C-005
        // audit: CBTRN03C-013
        // audit: CBTRN03C-015
        // audit: CBTRN03C-017
        // audit: CBTRN03C-023
        // audit: CBTRN03C-024
        assert!(
            actual
                .iter()
                .copied()
                .eq(expected.iter().map(Vec::as_slice)),
            "FIXRPT-{number:03}: the details at records {positions:?} are not the block's details"
        );
    }
    Ok(())
}

/// The sign-bearing last characters of a signed DISPLAY item: `{` and `A` to `I` for zero and
/// positive values, `}` and `J` to `R` for negative ones (PIC-005, CBTRN03C-029).
const OVERPUNCHED: &[u8; 20] = b"{ABCDEFGHI}JKLMNOPQR";

/// Whether `line` is a `TRAN-RECORD` line in its redacted form: 350 bytes, the six free-text
/// fields all `*` and the card number twelve `*` and four digits (CBTRN03C-012).
fn is_tran_record_line(line: &str) -> bool {
    let bytes = line.as_bytes();
    let masked = MASKED_BYTES.iter().all(|&(start, end)| {
        bytes
            .get(start..end)
            .is_some_and(|field| field.iter().all(|&byte| byte == b'*'))
    });
    let card = bytes.get(CARD_BYTES.0..CARD_BYTES.1).is_some_and(|card| {
        let (mask, last) = card.split_at(CARD_MASK_LEN.min(card.len()));
        mask.iter().all(|&byte| byte == b'*')
            && last.len() == 4
            && last.iter().all(u8::is_ascii_digit)
    });
    bytes.len() == TRAN_RECORD_LEN && masked && card
}

/// The `TRAN-PROC-TS` of the rows a report over POSTED reads, as its `TRAN-RECORD` lines show it.
#[derive(Clone, Copy)]
enum ProcTs {
    /// The stamp the test stored in every row, compared byte for byte.
    Stored(&'static [u8; 26]),
    /// The stamp `posttran` took from the system clock, which a subprocess cannot be given: a
    /// time field, compared by its format only, [`CLOCK_PROC_TS_SHAPE`].
    Clock,
}

impl ProcTs {
    /// Whether `shown`, bytes 305-330 of a `TRAN-RECORD` line, is this stamp.
    fn admits(self, shown: &[u8]) -> bool {
        match self {
            ProcTs::Stored(stamp) => shown == stamp.as_slice(),
            ProcTs::Clock => {
                shown.len() == CLOCK_PROC_TS_SHAPE.len()
                    && shown
                        .iter()
                        .zip(CLOCK_PROC_TS_SHAPE)
                        .all(|(&byte, &shape)| {
                            if shape == b'9' {
                                byte.is_ascii_digit()
                            } else {
                                byte == shape
                            }
                        })
            }
        }
    }
}

/// The fields in which `shown`, a `TRAN-RECORD` job-log line, departs from `expected`, its row's
/// redacted line: every field byte for byte but `TRAN-PROC-TS`, which `proc_ts` decides, and
/// `length` for a line of other than 350 bytes.
fn differing_fields(shown: &[u8], expected: &[u8], proc_ts: ProcTs) -> Vec<&'static str> {
    let mut fields: Vec<&'static str> = TRAN_RECORD_FIELDS
        .iter()
        .filter(|&&(_, start, end)| {
            let field = shown.get(start..end);
            let matches = if (start, end) == PROC_TS_BYTES {
                field.is_some_and(|stamp| proc_ts.admits(stamp))
            } else {
                field.is_some() && field == expected.get(start..end)
            };
            !matches
        })
        .map(|&(name, ..)| name)
        .collect();
    if shown.len() != TRAN_RECORD_LEN {
        fields.push("length");
    }
    fields
}

/// Whether `line` is the end-of-file line `label` followed by a signed DISPLAY item: ten digits,
/// then a digit with the sign overpunched (CBTRN03C-029).
fn is_amount_line(line: &str, label: &str) -> bool {
    line.strip_prefix(label).is_some_and(|item| {
        let (digits, last) = item
            .as_bytes()
            .split_at(item.len().min(AMOUNT_ITEM_LEN - 1));
        item.len() == AMOUNT_ITEM_LEN
            && digits.iter().all(u8::is_ascii_digit)
            && last.first().is_some_and(|sign| OVERPUNCHED.contains(sign))
    })
}

/// Checks the job log of a normal run over POSTED against the `report` it wrote, its window line
/// being `reporting_line`: the start line, the window line, one `TRAN-RECORD` line per detail in
/// report order, the two end-of-file lines and the end line, and nothing else (CBTRN03C-006,
/// CBTRN03C-008 to CBTRN03C-010, CBTRN03C-012, CBTRN03C-029, CBTRN03C-034). Each `TRAN-RECORD`
/// line is the redacted line of the detail whose transaction ID it shows, byte for byte, its
/// `TRAN-PROC-TS` being `proc_ts`. Failures name line positions and fields only.
fn assert_posted_job_log(
    log: &[String],
    reporting_line: &str,
    report: &[u8],
    proc_ts: ProcTs,
) -> TestResult {
    let records = fixed_records(report, RECORD_LEN)?;
    let mut detail_numbers: Vec<usize> = BLOCKS
        .iter()
        .flat_map(|block| block.details)
        .map(|detail| detail.record)
        .collect();
    detail_numbers.sort_unstable();
    let reported_ids = detail_numbers
        .iter()
        .map(|&number| {
            number
                .checked_sub(1)
                .and_then(|index| records.get(index))
                .and_then(|record| record.get(..TRAN_ID_LEN))
                .ok_or_else(|| format!("the report has no detail record {number}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    // audit: CBTRN03C-006
    assert!(
        log.first().is_some_and(|line| line == START_LINE),
        "the first job-log line is not the start line"
    );
    // The DATEPARM record always exists, so every run shows the window it read, and neither the
    // read's end of file nor its error path is taken.
    // audit: CBTRN03C-008
    // audit: CBTRN03C-009
    assert!(
        log.get(1).is_some_and(|line| line == reporting_line),
        "the second job-log line is not the window line"
    );
    let shown = log
        .get(2..2 + POSTED_ROWS)
        .ok_or("the job log ends before one TRAN-RECORD line per selected row")?;
    // Each line is matched by its transaction ID, as X-TRANREPT-H1 leaves the order inside a card
    // block to the sort; a line whose ID is no detail's, or one shown before, differs in its ID.
    let mut expected = expected_tran_record_lines()?;
    let mut differing: Vec<(usize, Vec<&str>)> = Vec::new();
    for (line, number) in shown.iter().map(String::as_bytes).zip(3_usize..) {
        let fields = line
            .get(..TRAN_ID_LEN)
            .and_then(|id| expected.remove(id))
            .map_or_else(
                || vec!["TRAN-ID"],
                |want| differing_fields(line, &want, proc_ts),
            );
        if !fields.is_empty() {
            differing.push((number, fields));
        }
    }
    // audit: CBTRN03C-012
    assert!(
        differing.is_empty(),
        "job-log lines that differ from their detail's redacted TRAN-RECORD line, by line and \
         field: {differing:?}"
    );
    let shown_ids: Vec<&[u8]> = shown
        .iter()
        .filter_map(|line| line.as_bytes().get(..TRAN_ID_LEN))
        .collect();
    // Status `00`: each selected row is read in turn, in report order, and shown before it is
    // reported; status `10` after the last one ends the loop with the end-of-file lines.
    // audit: CBTRN03C-010
    assert!(
        shown_ids == reported_ids,
        "the TRAN-RECORD lines do not show the reported transactions in report order"
    );
    let after = log.get(2 + POSTED_ROWS..).unwrap_or_default();
    // BLOCKED(X-TRANREPT-H1): the last amount follows the order inside the last card block, so
    // the two items are checked for their shape only; their values get no parity assertion.
    // audit: CBTRN03C-029
    assert!(
        matches!(after, [amount, total, _]
            if is_amount_line(amount, TRAN_AMT_LABEL)
                && is_amount_line(total, PAGE_TOTAL_ITEM_LABEL)),
        "the TRAN-AMT and WS-PAGE-TOTAL lines do not follow the TRAN-RECORD lines"
    );
    // audit: CBTRN03C-034
    assert!(
        after.last().is_some_and(|line| line == END_LINE),
        "the last job-log line is not the end line"
    );
    Ok(())
}

/// Checks that every type `01` detail of the POSTED `report` shows the VSAM description in
/// columns 33-47, and that no record holds a Db2 description of type `01`. Failures name record
/// numbers only.
fn assert_vsam_descriptions(report: &[u8]) -> TestResult {
    let records = fixed_records(report, RECORD_LEN)?;
    let mut purchases = 0;
    let mut without_vsam = Vec::new();
    for detail in BLOCKS.iter().flat_map(|block| block.details) {
        let record = detail
            .record
            .checked_sub(1)
            .and_then(|index| records.get(index))
            .ok_or_else(|| format!("the report has no record {}", detail.record))?;
        if record.get(29..31) == Some(b"01".as_slice()) {
            purchases += 1;
            if record.get(32..47) != Some(VSAM_PURCHASE.as_slice()) {
                without_vsam.push(detail.record);
            }
        }
    }
    let audited_purchases = BLOCKS
        .iter()
        .flat_map(|block| block.details)
        .filter(|detail| matches!(detail.kind, Kind::Purchase))
        .count();
    // audit: CBTRN03C-015
    // audit: CBTRN03C-038
    assert_eq!(purchases, audited_purchases, "type 01 details");
    assert!(
        without_vsam.is_empty(),
        "type 01 details without the VSAM description: {without_vsam:?}"
    );
    // Columns 33-47 would show the first 15 bytes of the Db2 description.
    let renamed_shown = RENAMED
        .as_bytes()
        .get(..15)
        .ok_or("a short Db2 description")?;
    let db2_texts = [renamed_shown, DB2_SEED_PURCHASE.as_bytes()];
    let holding_db2: Vec<usize> = records
        .iter()
        .zip(1_usize..)
        .filter(|(record, _)| {
            db2_texts
                .iter()
                .any(|text| record.windows(text.len()).any(|window| window == *text))
        })
        .map(|(_, number)| number)
        .collect();
    assert!(
        holding_db2.is_empty(),
        "records holding a Db2 description: {holding_db2:?}"
    );
    Ok(())
}

/// POSTED over a window around today: `tranrept` writes the report of FIXRPT-001 to FIXRPT-905
/// with the passed dates in its name headers, shows the job log of the run with those dates,
/// leaves `transactions` as it was and adds no file but its generation.
#[test]
fn report_over_posted_transactions_matches_audit() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let scratch = Scratch::new()?;
        post_fixture_transactions(&guard, &scratch).await?;
        let mut store = PgStore::new(guard.pool().clone());
        let posted = TransactionRepository::find_all_in_card_order(&mut store).await?;
        let (start, end) = window_around_today()?;
        let run = run_report(&guard, &scratch, Some((start.as_str(), end.as_str())), 1).await?;
        // audit: CBTRN03C-001
        // audit: CBTRN03C-004
        assert_posted_report(&run.report, &start, &end)?;
        let reporting_line = format!("{REPORTING_FROM}{start}{REPORTING_TO}{end}");
        assert_posted_job_log(&run.job_log, &reporting_line, &run.report, ProcTs::Clock)?;
        // audit: CBTRN03C-003
        assert!(
            TransactionRepository::find_all_in_card_order(&mut store).await? == posted,
            "tranrept changed `transactions`"
        );
        assert_eq!(
            var_files(&scratch)?,
            ["dalyrejs/DALYREJS.G0001", "reports/TRANREPT.G0001"],
            "files under CARDDEMO_VAR_DIR"
        );
        Ok(())
    })
}

/// The default window over POSTED as `fixtures.md` states it, the rows `posttran` wrote carrying
/// the POSTED processing timestamp: the name headers and the job log's window line show
/// `2022-01-01` and `2022-07-06`, and the report is FIXRPT-001 to FIXRPT-905.
#[test]
fn default_window() -> TestResult {
    block_on(async {
        // The rows are posted in a database of their own and copied, with the POSTED stamp, into
        // the one the report reads.
        let posted = {
            let source = start_loaded_postgres().await?;
            let scratch = Scratch::new()?;
            post_fixture_transactions(&source, &scratch).await?;
            // The default window over rows stamped today selects none of them.
            // BLOCKED(X-TRANREPT-H2): that empty selection has no single COBOL result, so the run's
            // outcome is reported, never asserted.
            // audit: CBTRN03C-037
            let empty =
                run_batch(&source, &scratch, &[OsStr::new("tranrept")], Log::Errors).await?;
            // audit: CBTRN03C-039
            assert_no_fixture_card_number(&empty)?;
            eprintln!(
                "CBTRN03C-037 BLOCKED (X-TRANREPT-H2): exit status {:?}, {} path(s) printed",
                empty.status.code(),
                written_paths(&empty).len()
            );
            TransactionRepository::find_all_in_card_order(&mut PgStore::new(source.pool().clone()))
                .await?
        };
        // audit: FIXPOST-901
        assert_eq!(posted.len(), POSTED_ROWS, "rows posttran wrote");
        let guard = start_loaded_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let stamp = PicTimestamp::decode(POSTED_PROC_TS)?;
        for mut row in posted {
            row.proc_ts = stamp;
            TransactionRepository::insert(&mut store, &row).await?;
        }
        let scratch = Scratch::new()?;
        let run = run_report(&guard, &scratch, None, 1).await?;
        // audit: CBTRN03C-001
        // audit: CBTRN03C-004
        // audit: FIXRPT-901
        assert_posted_report(&run.report, DEFAULT_START, DEFAULT_END)?;
        assert_posted_job_log(
            &run.job_log,
            DEFAULT_REPORTING_LINE,
            &run.report,
            ProcTs::Stored(POSTED_PROC_TS),
        )
    })
}

/// Transaction IDs that are card numbers: two `dailytran.txt` records of two cards that
/// `card_xref` holds are stored processed on the POSTED date, the first with its own card number
/// as its transaction ID and the second with a third fixture card's. A run over the default
/// window ends normally, nothing it prints holds a fixture card number in full, any event
/// included ([`run_report`]), and each row's `TRAN-RECORD` line shows its transaction ID as
/// twelve `*` and the ID's last four digits and every other byte as CBTRN03C-012 gives it.
/// Failures name no line and no number.
#[test]
fn card_numbers_in_transaction_ids_are_redacted() -> TestResult {
    block_on(async {
        let cards = fixture_card_numbers()?
            .into_iter()
            .map(|text| Ok((CardNumber::parse(&text)?, text)))
            .collect::<Result<Vec<(CardNumber, String)>, Box<dyn Error>>>()?;
        let card_of =
            |row: &TransactionRecord| cards.iter().position(|(card, _)| *card == row.card_num);
        let mut daily = daily_transactions()?.into_iter();
        let (own_row, own_card) = daily
            .find_map(|row| card_of(&row).map(|index| (row, index)))
            .ok_or("no dailytran.txt record of a card in card_xref")?;
        let (other_row, other_card) = daily
            .find_map(|row| {
                card_of(&row)
                    .filter(|&index| index != own_card)
                    .map(|index| (row, index))
            })
            .ok_or("no dailytran.txt record of a second card in card_xref")?;
        let third_card = (0..cards.len())
            .find(|&index| index != own_card && index != other_card)
            .ok_or("fewer than three fixture cards")?;
        let guard = start_loaded_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let stamp = PicTimestamp::decode(POSTED_PROC_TS)?;
        let mut expected = Vec::new();
        for (mut row, id_card) in [(own_row, own_card), (other_row, third_card)] {
            let (_, id) = cards.get(id_card).ok_or("no such fixture card")?;
            row.id = PicX::from_text(id)?;
            row.proc_ts = stamp;
            TransactionRepository::insert(&mut store, &row).await?;
            expected.push(id_redacted_tran_record_line(&row)?);
        }
        let scratch = Scratch::new()?;
        // audit: CBTRN03C-039
        let run = run_report(&guard, &scratch, None, 1).await?;
        let mut shown: Vec<&str> = run
            .job_log
            .iter()
            .filter(|line| is_tran_record_line(line))
            .map(String::as_str)
            .collect();
        shown.sort_unstable();
        expected.sort_unstable();
        // audit: CBTRN03C-012
        // audit: CBTRN03C-039
        assert!(
            shown == expected,
            "the TRAN-RECORD lines are not the two rows with their transaction IDs redacted"
        );
        Ok(())
    })
}

/// A reports directory that cannot be created, because `reports` is a file, fails the open of
/// `REPTFILE` and ends the run on the abend path: `ERROR OPENING REPTFILE`, the status line and
/// `ABENDING PROGRAM` after the start line, a non-zero exit status, no path printed, no
/// generation file, and `transactions` as it was.
#[test]
fn uncreatable_reports_directory_abends_the_run() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let scratch = Scratch::new()?;
        post_fixture_transactions(&guard, &scratch).await?;
        let mut store = PgStore::new(guard.pool().clone());
        let posted = TransactionRepository::find_all_in_card_order(&mut store).await?;
        fs::write(scratch.var_dir().join("reports"), b"")?;
        let (start, end) = window_around_today()?;
        let output = run_batch(&guard, &scratch, &tranrept_args(&start, &end), Log::JobLog).await?;
        // audit: CBTRN03C-039
        assert_no_fixture_card_number(&output)?;
        // The exit status is only required to be non-zero until the reviewer settles X-TRANREPT-4.
        // audit: CBTRN03C-007
        // audit: CBTRN03C-035
        assert!(
            output.status.code().is_some_and(|code| code != 0),
            "tranrept ended with {:?}",
            output.status.code()
        );
        // audit: CBTRN03C-007
        assert_abend_log(&job_log(&output), OPEN_REPTFILE, "CBTRN03C-007");
        // The event of the failed open names the setting of the directory, never its path.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let shown = scratch.var_dir().display().to_string();
        assert!(
            stdout.lines().any(|line| {
                line.contains(r#""operation":"OPEN REPTFILE""#)
                    && line.contains(&format!("({REPORTS_DIR_SETTING})"))
            }),
            "no event of the failed open names the reports setting"
        );
        assert!(
            !stdout.contains(&shown),
            "standard output shows the var directory"
        );
        assert!(written_paths(&output).is_empty(), "tranrept printed a path");
        assert_eq!(
            var_files(&scratch)?,
            ["dalyrejs/DALYREJS.G0001", "reports"],
            "files under CARDDEMO_VAR_DIR"
        );
        // audit: CBTRN03C-003
        assert!(
            TransactionRepository::find_all_in_card_order(&mut store).await? == posted,
            "tranrept changed `transactions`"
        );
        Ok(())
    })
}

/// E23: a COBTUPDT update of type `01` in the Db2 data version leaves the VSAM version untouched,
/// every type `01` detail still shows `Purchase`, and the report written after the update equals
/// the one written before it byte for byte.
#[test]
fn db2_maintenance_never_reaches_report_lookups() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let scratch = Scratch::new()?;
        post_fixture_transactions(&guard, &scratch).await?;
        let (start, end) = window_around_today()?;
        let window = Some((start.as_str(), end.as_str()));
        let before = run_report(&guard, &scratch, window, 1).await?.report;
        let code = PicX::<2>::from_text("01")?;
        let mut store = PgStore::new(guard.pool().clone());
        let vsam = TransactionTypeRepository::find_by_id(&mut store, &code)
            .await?
            .ok_or("no VSAM type 01")?;
        let seeded = Db2TransactionTypeRepository::find_by_id(&mut store, &code)
            .await?
            .ok_or("no Db2 type 01")?;
        // audit: COBTUPDT-103
        assert_eq!(
            seeded.tr_description(),
            DB2_SEED_PURCHASE,
            "Db2 seed of type 01"
        );

        let input = mnttrdb2_input(&scratch, &[format!("U01{RENAMED:<50}")])?;
        let args = [
            OsStr::new("mnttrdb2"),
            OsStr::new("--input"),
            input.as_os_str(),
        ];
        let output = run_batch(&guard, &scratch, &args, Log::Errors).await?;
        // audit: COBTUPDT-027
        assert_eq!(
            output.status.code(),
            Some(0),
            "mnttrdb2: {}",
            stderr(&output)
        );
        assert!(written_paths(&output).is_empty(), "mnttrdb2 printed a path");
        let updated = Db2TransactionTypeRepository::find_by_id(&mut store, &code)
            .await?
            .ok_or("no Db2 type 01 after the update")?;
        // audit: COBTUPDT-012
        assert_eq!(
            updated.tr_description(),
            format!("{RENAMED:<50}"),
            "Db2 type 01 after the update"
        );
        // audit: COBTUPDT-026
        assert_eq!(vsam.desc, PicX::from_text("Purchase")?, "VSAM type 01");
        assert!(
            TransactionTypeRepository::find_by_id(&mut store, &code).await? == Some(vsam),
            "mnttrdb2 changed the VSAM type 01"
        );

        let after = run_report(&guard, &scratch, window, 2).await?.report;
        assert_vsam_descriptions(&after)?;
        // audit: CBTRN03C-038
        assert!(
            after == before,
            "the report written after the update differs from the one before it"
        );
        Ok(())
    })
}

/// A connection lost inside the read loop: the [`Relay`] closes the connection of the selection
/// after its first row, so the job reads, shows and reports that row and fails to fetch the next.
/// The run ends on the abend path of `1000-TRANFILE-GET-NEXT`: `ERROR READING TRANSACTION FILE`,
/// the status line and `ABENDING PROGRAM`, a non-zero exit status, no path printed, no generation
/// file and `transactions` as it was.
#[test]
fn a_selection_lost_after_its_first_row_abends_the_read() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let scratch = Scratch::new()?;
        post_fixture_transactions(&guard, &scratch).await?;
        let mut store = PgStore::new(guard.pool().clone());
        let posted = TransactionRepository::find_all_in_card_order(&mut store).await?;
        let (start, end) = window_around_today()?;
        let relay = Relay::start(
            guard.database_url(),
            Cut::AfterFirstRow(TRANSACTIONS_COLUMN),
        )?;
        let args = tranrept_args(&start, &end);
        let output = run_against(relay.database_url(), &scratch, &args, Log::JobLog).await?;
        let cut = relay.cut_made();
        drop(relay);
        assert!(cut, "the relay never met the selection's row description");
        // audit: CBTRN03C-039
        assert_no_fixture_card_number(&output)?;
        // The exit status is only required to be non-zero until the reviewer settles X-TRANREPT-4.
        // audit: CBTRN03C-035
        assert!(
            output.status.code().is_some_and(|code| code != 0),
            "tranrept ended with {:?}",
            output.status.code()
        );
        assert_eq!(
            first_stderr_line(&output),
            format!("carddemo-batch tranrept: ABEND: {READ_TRANFILE}"),
            "standard error"
        );
        let log = job_log(&output);
        // Status `00` for the first row, read when the selection opened and shown before it was
        // reported; the fetch of the second fails.
        // audit: CBTRN03C-010
        assert_eq!(
            log.iter().filter(|line| is_tran_record_line(line)).count(),
            1,
            "TRAN-RECORD lines before the failed fetch"
        );
        // audit: CBTRN03C-010
        assert_abend_log(&log, READ_TRANFILE, "CBTRN03C-010");
        // audit: CBTRN03C-035
        assert!(written_paths(&output).is_empty(), "tranrept printed a path");
        assert_eq!(
            var_files(&scratch)?,
            ["dalyrejs/DALYREJS.G0001"],
            "files under CARDDEMO_VAR_DIR"
        );
        // audit: CBTRN03C-003
        assert!(
            TransactionRepository::find_all_in_card_order(&mut store).await? == posted,
            "tranrept changed `transactions`"
        );
        Ok(())
    })
}

/// One run of [`report_writes_past_a_size_limit_abend_the_run`]: the bytes its file-size limit
/// lies between, the failure line the write it stops shows, and the rule that line proves.
struct LimitCase {
    limit: SizeWindow,
    failure_line: &'static str,
    rule: &'static str,
}

/// A report file that cannot grow past a file-size limit: below the first buffer the job writes,
/// a write inside the loop fails (CBTRN03C-033); above every buffer the loop writes, the last
/// write, when the file is closed, fails (CBTRN03C-034). Each run ends on the abend path: its
/// failure line, the status line and `ABENDING PROGRAM`, a non-zero exit status, no path printed
/// and no generation file.
#[test]
fn report_writes_past_a_size_limit_abend_the_run() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let posting = Scratch::new()?;
        post_fixture_transactions(&guard, &posting).await?;
        let (start, end) = window_around_today()?;
        let cases = [
            LimitCase {
                limit: FIRST_WRITE_LIMIT,
                failure_line: WRITE_REPTFILE,
                rule: "CBTRN03C-033",
            },
            LimitCase {
                limit: CLOSE_LIMIT,
                failure_line: CLOSE_REPTFILE,
                rule: "CBTRN03C-034",
            },
        ];
        for case in cases {
            let rule = case.rule;
            let scratch = Scratch::new()?;
            let args = tranrept_args(&start, &end);
            let output = run_size_limited(&guard, &scratch, &args, case.limit).await?;
            // audit: CBTRN03C-039
            assert_no_fixture_card_number(&output)?;
            // audit: CBTRN03C-035
            assert!(
                output.status.code().is_some_and(|code| code != 0),
                "{rule}: tranrept ended with {:?}",
                output.status.code()
            );
            assert_eq!(
                first_stderr_line(&output),
                format!("carddemo-batch tranrept: ABEND: {}", case.failure_line),
                "{rule}: standard error"
            );
            // audit: CBTRN03C-033
            // audit: CBTRN03C-034
            assert_abend_log(&job_log(&output), case.failure_line, rule);
            // audit: CBTRN03C-035
            assert!(
                written_paths(&output).is_empty(),
                "{rule}: tranrept printed a path"
            );
            assert!(
                var_files(&scratch)?.is_empty(),
                "{rule}: a file remains under CARDDEMO_VAR_DIR"
            );
        }
        Ok(())
    })
}

/// The lookup reads that fail other than by a missing key, `HALTED(X-TRANREPT-H5)`
/// (CBTRN03C-040 to CBTRN03C-042): the COBOL falls through to `END-READ` and goes on with the
/// last record read, a result no source fixes for a `PostgreSQL` failure. For each lookup table
/// the [`Relay`] closes the table's connection at its row description, and the run's outcome is
/// printed as the rule's `BLOCKED` report, never asserted. The replacement reads each table
/// whole when it opens it, before the first record, so the failure takes the open path of
/// CBTRN03C-007 (X-TRANREPT-10).
#[test]
fn lookup_reads_that_fail_are_reported_blocked() -> TestResult {
    block_on(async {
        let guard = start_loaded_postgres().await?;
        let posting = Scratch::new()?;
        post_fixture_transactions(&guard, &posting).await?;
        let (start, end) = window_around_today()?;
        let cases = [
            ("CBTRN03C-040", "card_xref", CARD_XREF_COLUMN, OPEN_CARDXREF),
            (
                "CBTRN03C-041",
                "transaction_types",
                TRANSACTION_TYPES_COLUMN,
                OPEN_TRANTYPE,
            ),
            (
                "CBTRN03C-042",
                "transaction_categories",
                TRANSACTION_CATEGORIES_COLUMN,
                OPEN_TRANCATG,
            ),
        ];
        for (rule, table, column, open_line) in cases {
            let scratch = Scratch::new()?;
            let relay = Relay::start(guard.database_url(), Cut::AtDescription(column))?;
            let args = tranrept_args(&start, &end);
            let output = run_against(relay.database_url(), &scratch, &args, Log::JobLog).await?;
            let cut = relay.cut_made();
            drop(relay);
            // Only the relay's own working and the run's redaction, which no halt covers, are
            // checked; the run's outcome is reported.
            assert!(
                cut,
                "{rule}: the relay never met the row description of {table}"
            );
            // audit: CBTRN03C-039
            assert_no_fixture_card_number(&output)?;
            let log = job_log(&output);
            // BLOCKED(X-TRANREPT-H5): no parity assertion.
            // audit: CBTRN03C-040
            // audit: CBTRN03C-041
            // audit: CBTRN03C-042
            println!(
                "BLOCKED {rule} (X-TRANREPT-H5): a failed read of {table}, no parity asserted; \
                 observed: exit status {:?}, {} path(s) printed, {} file(s) under \
                 CARDDEMO_VAR_DIR, open line shown: {}, ABENDING PROGRAM last: {}",
                output.status.code(),
                written_paths(&output).len(),
                var_files(&scratch).map_or(0, |files| files.len()),
                log.iter().any(|line| line == open_line),
                log.last().is_some_and(|line| line == ABEND_LINE)
            );
        }
        Ok(())
    })
}
