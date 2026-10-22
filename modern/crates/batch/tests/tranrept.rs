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
//!
//! # Blocked values
//!
//! Halt candidates X-FIXTURES-H3 and X-TRANREPT-H1 leave the order of the details inside a card
//! block to the sort, so those values get no parity assertion: each block's details are compared
//! as a set at the block's record positions, and the page totals other than records 21, 201 and
//! 341, and the grand total, are checked for their record kind only. Positions, record kinds, the
//! detail texts, the 49 account totals and the three fixed page totals are asserted byte for
//! byte.
//!
//! # Job log
//!
//! `tranrept` emits no job-log line of CBTRN03C, as `crates/batch/src/jobs/tranrept.rs` states:
//! its standard output is the path it wrote. The job-log rules (CBTRN03C-006, CBTRN03C-008,
//! CBTRN03C-012, CBTRN03C-029 and the end line of CBTRN03C-034) are therefore not proved here.
//! Nor are the failures a subprocess cannot provoke without a repository double or a failing
//! device: a row that cannot be fetched (CBTRN03C-010), a failed write (CBTRN03C-033) and the
//! lookup failures other than a missing key (CBTRN03C-040 to CBTRN03C-042).
//!
//! # Cardholder data
//!
//! Report records hold no card number (CBTRN03C-023), but every failure message names record
//! numbers, block numbers or byte offsets only, never record bytes. Standard error, which
//! `carddemo-batch` keeps free of protected values, is the only process output a message shows.

use std::error::Error;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{Days, NaiveDate};
use copybook::{PicTimestamp, PicX};
use domain::clock::{Clock, SystemClock};
use persistence::{
    Db2TransactionTypeRepository, PgStore, TransactionRepository, TransactionTypeRepository,
};
use test_support::fixtures::{fixture_dir, fixture_path, start_loaded_postgres};
use test_support::postgres::PostgresGuard;

type TestResult = Result<(), Box<dyn Error>>;

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

/// Runs `carddemo-batch` with `args` from `modern/`, the working directory of every binary,
/// against `guard`'s database, with its output directories in `scratch` and the fixtures read in
/// place. The rest of the environment is inherited. Span export is off and only error events are
/// logged, so standard output carries the written paths.
async fn run_batch(
    guard: &PostgresGuard,
    scratch: &Scratch,
    args: &[&OsStr],
) -> Result<Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command
        .args(args)
        .current_dir(test_support::modern_dir())
        .env("DATABASE_URL", guard.database_url())
        .env("CARDDEMO_VAR_DIR", scratch.var_dir())
        .env("CARDDEMO_STATEMENT_DIR", scratch.path().join("statements"))
        .env("CARDDEMO_INTRDR_DIR", scratch.path().join("jobs"))
        .env("CARDDEMO_FIXTURE_DIR", fixture_dir())
        .env("RUST_LOG", "error")
        .env_remove("CARDDEMO_OTEL_EXPORT");
    Ok(tokio::task::spawn_blocking(move || command.output()).await??)
}

/// A run's standard error, which `carddemo-batch` keeps free of records and protected values.
fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
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
    let output = run_batch(guard, scratch, &args).await?;
    // audit: FIXPOST-903
    assert_eq!(
        output.status.code(),
        Some(4),
        "posttran: {}",
        stderr(&output)
    );
    Ok(())
}

/// Runs `tranrept` over `window`, or with its default dates when `None`, checks that it ended
/// normally and printed the path of generation `generation` alone, and returns that file.
async fn run_report(
    guard: &PostgresGuard,
    scratch: &Scratch,
    window: Option<(&str, &str)>,
    generation: u16,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut args = vec![OsStr::new("tranrept")];
    if let Some((start, end)) = window {
        args.extend([
            OsStr::new("--start-date"),
            OsStr::new(start),
            OsStr::new("--end-date"),
            OsStr::new(end),
        ]);
    }
    let output = run_batch(guard, scratch, &args).await?;
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
    Ok(fs::read(path)?)
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
/// with the passed dates in its name headers, leaves `transactions` as it was and adds no file
/// but its generation.
#[tokio::test(flavor = "multi_thread")]
async fn report_over_posted_transactions_matches_audit() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let scratch = Scratch::new()?;
    post_fixture_transactions(&guard, &scratch).await?;
    let mut store = PgStore::new(guard.pool().clone());
    let posted = TransactionRepository::find_all_in_card_order(&mut store).await?;
    let (start, end) = window_around_today()?;
    let report = run_report(&guard, &scratch, Some((start.as_str(), end.as_str())), 1).await?;
    // audit: CBTRN03C-001
    // audit: CBTRN03C-004
    assert_posted_report(&report, &start, &end)?;
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
}

/// The default window over POSTED as `fixtures.md` states it, the rows `posttran` wrote carrying
/// the POSTED processing timestamp: the name headers show `2022-01-01` and `2022-07-06`, and the
/// report is FIXRPT-001 to FIXRPT-905.
#[tokio::test(flavor = "multi_thread")]
async fn default_window() -> TestResult {
    // The rows are posted in a database of their own and copied, with the POSTED stamp, into the
    // one the report reads.
    let posted = {
        let source = start_loaded_postgres().await?;
        let scratch = Scratch::new()?;
        post_fixture_transactions(&source, &scratch).await?;
        // The default window over rows stamped today selects none of them.
        // BLOCKED(X-TRANREPT-H2): that empty selection has no single COBOL result, so the run's
        // outcome is reported, never asserted.
        // audit: CBTRN03C-037
        let empty = run_batch(&source, &scratch, &[OsStr::new("tranrept")]).await?;
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
    let report = run_report(&guard, &scratch, None, 1).await?;
    // audit: CBTRN03C-001
    // audit: CBTRN03C-004
    // audit: FIXRPT-901
    assert_posted_report(&report, DEFAULT_START, DEFAULT_END)
}

/// A reports directory that cannot be created, because `reports` is a file, fails the open of
/// `REPTFILE` and ends the run on the abend path: a non-zero exit status, no path printed, no
/// generation file, and `transactions` as it was.
#[tokio::test(flavor = "multi_thread")]
async fn uncreatable_reports_directory_abends_the_run() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let scratch = Scratch::new()?;
    post_fixture_transactions(&guard, &scratch).await?;
    let mut store = PgStore::new(guard.pool().clone());
    let posted = TransactionRepository::find_all_in_card_order(&mut store).await?;
    fs::write(scratch.var_dir().join("reports"), b"")?;
    let (start, end) = window_around_today()?;
    let args = [
        OsStr::new("tranrept"),
        OsStr::new("--start-date"),
        OsStr::new(&start),
        OsStr::new("--end-date"),
        OsStr::new(&end),
    ];
    let output = run_batch(&guard, &scratch, &args).await?;
    // The exit status is only required to be non-zero until the reviewer settles X-TRANREPT-4.
    // audit: CBTRN03C-007
    // audit: CBTRN03C-035
    assert!(
        output.status.code().is_some_and(|code| code != 0),
        "tranrept ended with {:?}",
        output.status.code()
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
}

/// E23: a COBTUPDT update of type `01` in the Db2 data version leaves the VSAM version untouched,
/// every type `01` detail still shows `Purchase`, and the report written after the update equals
/// the one written before it byte for byte.
#[tokio::test(flavor = "multi_thread")]
async fn db2_maintenance_never_reaches_report_lookups() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let scratch = Scratch::new()?;
    post_fixture_transactions(&guard, &scratch).await?;
    let (start, end) = window_around_today()?;
    let window = Some((start.as_str(), end.as_str()));
    let before = run_report(&guard, &scratch, window, 1).await?;
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
    let output = run_batch(&guard, &scratch, &args).await?;
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

    let after = run_report(&guard, &scratch, window, 2).await?;
    assert_vsam_descriptions(&after)?;
    // audit: CBTRN03C-038
    assert!(
        after == before,
        "the report written after the update differs from the one before it"
    );
    Ok(())
}
