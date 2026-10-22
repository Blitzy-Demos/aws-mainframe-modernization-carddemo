//! i9 and the job-level share of g3b for POSTTRAN: `carddemo-batch posttran`, run as a
//! subprocess from `modern/` against a fixture-loaded `postgres:16` testcontainer, checked
//! against the expected results of `modern/audit/batch/POSTTRAN.md` and
//! `modern/audit/fixtures.md` (AAP 0.9.1).
//!
//! - `posttran_rejects_match_audit_byte_for_byte`: over `app/data/ASCII/dailytran.txt`, the
//!   reject generation `DALYREJS.G0001` equals the audited reject records byte for byte, record
//!   by record and in input order; the run exits with status 4 and logs the audited job-log
//!   lines (FIXPOST-902, FIXPOST-903).
//! - `posttran_posts_audited_effects`: the same run leaves the audited transactions, account
//!   fields and category balances (FIXPOST-001 to FIXPOST-300, FIXPOST-901).
//! - `second_run_writes_next_generation`: a second run into the same directory writes
//!   `DALYREJS.G0002` with the same records and keeps `DALYREJS.G0001` (CBTRN02C-028).
//! - The other tests drive the job-level rows no fixture record reaches, over scratch input that
//!   holds no fixture card number: input the job cannot take, an empty input, a writer claim held
//!   elsewhere, two runs at once and a duplicate transaction ID.
//!
//! Every expected value comes from the audit rows transcribed here and from the fixture bytes
//! they cite, never from `domain::batch::cbtrn02c` or the job. Codec A, which i8 proves, only
//! decodes the input and encodes the scratch records. The binary reads the system clock, which
//! a `FixedClock` cannot replace across the process boundary, so the processing timestamp is
//! compared by format only (AAP 0.5.6).
//!
//! The reject records and `transactions.card_num` hold full card numbers (Cardholder Data
//! Redaction). No assertion message shows a record or a card number: a reject mismatch names the
//! record, its input line and the first differing byte offset, and standard error is shown with
//! every fixture card number masked.
//!
//! A value that a halt or a halt candidate blocks is reported `BLOCKED` on standard output and
//! asserted as nothing (AAP 0.8.5).

use std::collections::{BTreeMap, HashMap};
use std::error::Error;
use std::fs;
use std::io;
use std::iter;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

use copybook::{FixedRecord, PicU, PicX, TsForm};
use domain::batch::loads::decode_fixture;
use domain::money::{Decimal, Money};
use domain::records::{
    AccountRecord, CardXrefRecord, DailyTransactionRecord, TranCatBalanceRecord, TransactionRecord,
};
use domain::sensitive::CardNumber;
use persistence::{
    AccountRepository, CardXrefRepository, Dataset, PgStore, TranCatBalanceRepository,
    TransactionRepository,
};
use test_support::fixtures::{
    fixture_card_numbers, fixture_dir, fixture_path, start_loaded_postgres,
};
use test_support::postgres::PostgresGuard;

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// The daily transaction fixture, read in place (FIXFRM-005).
const DAILYTRAN: &str = "dailytran.txt";

/// `DALYTRAN-RECORD`, 350 bytes (`CVTRA06Y.cpy:4-18`; FIXFRM-005).
const DALYTRAN_LEN: usize = 350;
/// A reject record: `DALYREJS` `LRECL=430` (`POSTTRAN.jcl:36`), the 350 input bytes and the
/// 80-byte validation trailer.
const REJECT_LEN: usize = 430;
/// `WS-VALIDATION-FAIL-REASON PIC 9(04)` (`CBTRN02C.cbl:180-182`).
const REASON_LEN: usize = 4;
/// `WS-VALIDATION-FAIL-REASON-DESC PIC X(76)` (`CBTRN02C.cbl:180-182`).
const DESC_LEN: usize = 76;
/// The offset of `DALYTRAN-AMT` in the record, after the 16-byte ID, the 2-byte type, the 4-byte
/// category, the 10-byte source and the 100-byte description (`CVTRA06Y.cpy:5-10`).
const AMT_OFFSET: usize = 132;
/// A body that ends 10 bytes into the trailing 20-byte `FILLER`, which CBTRN02C never reads as
/// a record (CBTRN02C-045).
const SHORT_BODY_LEN: usize = 340;

/// The first reject generation of an empty directory, then the next (CBTRN02C-028).
const FIRST_GENERATION: &str = "DALYREJS.G0001";
const SECOND_GENERATION: &str = "DALYREJS.G0002";

/// The exit status of a run that rejected a record (`CBTRN02C.cbl:229-230`; CBTRN02C-035).
const REJECTS_EXIT_STATUS: i32 = 4;
/// The exit status of every abend: the value X-POSTTRAN-2 proposes for the user abend `U0999`
/// (CBTRN02C-043).
const ABEND_EXIT_STATUS: i32 = 12;
/// Records posted over the fixture (FIXPOST-901).
const POSTED_COUNT: usize = 262;
/// Records rejected over the fixture (FIXPOST-902).
const REJECT_COUNT: usize = 38;
/// Rows of `tran_cat_balances` after the fixture run: the 50 loaded and the 50 created
/// (FIXPOST-901).
const CATEGORY_BALANCE_ROWS: usize = 100;
/// The `FILLER` of every category balance after the fixture run, loaded or created: 22 `0`
/// characters (FIXPOST-901; E05, X-FIXTURES-1).
const CATEGORY_FILLER: [u8; 22] = [b'0'; 22];

// The job-log lines of CBTRN02C, exactly as the program displays them (CBTRN02C-034).
const START_LINE: &str = "START OF EXECUTION OF PROGRAM CBTRN02C";
const END_LINE: &str = "END OF EXECUTION OF PROGRAM CBTRN02C";
/// The counts of the fixture run (FIXPOST-903).
const FIXTURE_COUNT_LINES: [&str; 2] = [
    "TRANSACTIONS PROCESSED :000000300",
    "TRANSACTIONS REJECTED  :000000038",
];
/// The counts of a run over an empty input (CBTRN02C-029).
const EMPTY_COUNT_LINES: [&str; 2] = [
    "TRANSACTIONS PROCESSED :000000000",
    "TRANSACTIONS REJECTED  :000000000",
];
/// The line shown before each category balance is created: this literal, the 17 key bytes,
/// which halt candidate X-POSTTRAN-H2 blocks, then `.. Creating.` (CBTRN02C-016).
const NOT_FOUND_PREFIX: &str = "TCATBAL record not found for key : ";
const NOT_FOUND_SUFFIX: &str = ".. Creating.";

// The failure lines of the paths the tests provoke (CBTRN02C-025, CBTRN02C-031, CBTRN02C-041,
// CBTRN02C-043).
const OPEN_DALYTRAN: &str = "ERROR OPENING DALYTRAN";
const OPEN_DALYREJS: &str = "ERROR OPENING DALY REJECTS FILE";
const OPEN_ACCTFILE: &str = "ERROR OPENING ACCOUNT MASTER FILE";
const WRITE_TRANFILE: &str = "ERROR WRITING TO TRANSACTION FILE";
/// `9910-DISPLAY-IO-STATUS`: this 20-character literal, then four status characters.
const STATUS_PREFIX: &str = "FILE STATUS IS: NNNN";
const STATUS_LEN: usize = 24;
/// The status line of a duplicate transaction key, status `22` (CBTRN02C-025, CBTRN02C-041).
const DUPLICATE_KEY_STATUS: &str = "FILE STATUS IS: NNNN0022";
const ABEND_LINE: &str = "ABENDING PROGRAM";

/// What precedes an event's message in the JSON lines of `runtime::telemetry`.
const MESSAGE_KEY: &str = r#""message":""#;

/// The card number of the scratch records: printable ASCII, all codec A requires of
/// `DALYTRAN-CARD-NUM` (FIXFRM-019), and no fixture card number.
const SCRATCH_CARD: [u8; 16] = *b"SCRATCH-CARD-001";
/// The fixture account the seeded cross-reference of [`SCRATCH_CARD`] names, with its own
/// customer, as every fixture cross-reference does (FIXFRM-003).
const SCRATCH_ACCOUNT: u64 = 1;
/// The type of the scratch records of the duplicate-ID test: `03`, whose category balance
/// (account 1, `03`, `0001`) the loads do not hold (FIXFRM-007), so the run creates it.
const SCRATCH_TYPE: [u8; 2] = *b"03";

/// The `FIXPOST-001` to `FIXPOST-300` rows of `fixtures.md`, one per `dailytran.txt` line in
/// input order, as [`parse_row`] reads them: `<line> <account> posted <create|update> <type>
/// <category> <cat_bal> <bal> <cr> <db>`, with the category balance and the account fields after
/// the posting, or `<line> <account> rejected <reason> <description>`. Amounts keep the audit's
/// decimal text, so each row reads against its audit row.
const FIXPOST_ROWS: [&str; 300] = [
    // audit: FIXPOST-001
    "1 7 posted update 01 0001 504.77 697.77 504.77 0.00",
    // audit: FIXPOST-002
    "2 20 posted create 03 0001 -919.00 -550.00 0.00 -919.00",
    // audit: FIXPOST-003
    "3 5 posted update 01 0001 67.88 412.88 67.88 0.00",
    // audit: FIXPOST-004
    "4 26 posted update 01 0001 281.77 327.77 281.77 0.00",
    // audit: FIXPOST-005
    "5 46 posted update 01 0001 454.66 850.66 454.66 0.00",
    // audit: FIXPOST-006
    "6 45 posted update 01 0001 849.99 1035.99 849.99 0.00",
    // audit: FIXPOST-007
    "7 13 posted create 03 0001 -56.77 -15.77 0.00 -56.77",
    // audit: FIXPOST-008
    "8 26 posted update 01 0001 655.43 701.43 655.43 0.00",
    // audit: FIXPOST-009
    "9 48 posted create 03 0001 -535.88 -309.88 0.00 -535.88",
    // audit: FIXPOST-010
    "10 9 posted update 01 0001 416.11 976.11 416.11 0.00",
    // audit: FIXPOST-011
    "11 10 posted update 01 0001 94.33 253.33 94.33 0.00",
    // audit: FIXPOST-012
    "12 32 posted update 01 0001 250.22 280.22 250.22 0.00",
    // audit: FIXPOST-013
    "13 41 posted update 01 0001 829.55 1204.55 829.55 0.00",
    // audit: FIXPOST-014
    "14 30 posted update 01 0001 29.44 31.44 29.44 0.00",
    // audit: FIXPOST-015
    "15 41 posted update 01 0001 1788.54 2163.54 1788.54 0.00",
    // audit: FIXPOST-016
    "16 37 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-017
    "17 22 posted create 03 0001 -945.66 -890.66 0.00 -945.66",
    // audit: FIXPOST-018
    "18 32 posted update 01 0001 899.55 929.55 899.55 0.00",
    // audit: FIXPOST-019
    "19 36 posted update 01 0001 502.66 612.66 502.66 0.00",
    // audit: FIXPOST-020
    "20 44 posted update 01 0001 303.11 566.11 303.11 0.00",
    // audit: FIXPOST-021
    "21 50 posted update 01 0001 183.88 675.88 183.88 0.00",
    // audit: FIXPOST-022
    "22 47 posted update 01 0001 779.33 811.33 779.33 0.00",
    // audit: FIXPOST-023
    "23 39 posted create 03 0001 -70.99 772.01 0.00 -70.99",
    // audit: FIXPOST-024
    "24 2 posted update 01 0001 355.11 513.11 355.11 0.00",
    // audit: FIXPOST-025
    "25 15 posted update 01 0001 660.11 1149.11 660.11 0.00",
    // audit: FIXPOST-026
    "26 8 posted update 01 0001 325.00 930.00 325.00 0.00",
    // audit: FIXPOST-027
    "27 3 posted update 01 0001 999.77 1146.77 999.77 0.00",
    // audit: FIXPOST-028
    "28 8 posted update 01 0001 807.44 1412.44 807.44 0.00",
    // audit: FIXPOST-029
    "29 20 posted update 01 0001 555.22 5.22 555.22 -919.00",
    // audit: FIXPOST-030
    "30 40 posted update 01 0001 356.22 399.22 356.22 0.00",
    // audit: FIXPOST-031
    "31 32 posted update 01 0001 1173.55 1203.55 1173.55 0.00",
    // audit: FIXPOST-032
    "32 17 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-033
    "33 45 posted update 01 0001 1690.54 1876.54 1690.54 0.00",
    // audit: FIXPOST-034
    "34 33 posted update 01 0001 769.55 1179.55 769.55 0.00",
    // audit: FIXPOST-035
    "35 28 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-036
    "36 22 posted update 01 0001 401.22 -489.44 401.22 -945.66",
    // audit: FIXPOST-037
    "37 27 posted update 01 0001 777.33 1061.33 777.33 0.00",
    // audit: FIXPOST-038
    "38 44 posted create 03 0001 -215.33 350.78 303.11 -215.33",
    // audit: FIXPOST-039
    "39 18 posted update 01 0001 46.66 190.66 46.66 0.00",
    // audit: FIXPOST-040
    "40 29 posted update 01 0001 570.66 909.66 570.66 0.00",
    // audit: FIXPOST-041
    "41 6 posted update 01 0001 843.66 1061.66 843.66 0.00",
    // audit: FIXPOST-042
    "42 49 posted update 01 0001 559.88 659.88 559.88 0.00",
    // audit: FIXPOST-043
    "43 30 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-044
    "44 39 posted update 01 0001 168.33 940.34 168.33 -70.99",
    // audit: FIXPOST-045
    "45 26 posted update 01 0001 1519.87 1565.87 1519.87 0.00",
    // audit: FIXPOST-046
    "46 25 posted update 01 0001 183.22 244.22 183.22 0.00",
    // audit: FIXPOST-047
    "47 28 posted update 01 0001 256.00 324.00 256.00 0.00",
    // audit: FIXPOST-048
    "48 14 posted update 01 0001 985.33 1000.33 985.33 0.00",
    // audit: FIXPOST-049
    "49 16 posted update 01 0001 19.99 752.99 19.99 0.00",
    // audit: FIXPOST-050
    "50 26 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-051
    "51 29 posted update 01 0001 1560.43 1899.43 1560.43 0.00",
    // audit: FIXPOST-052
    "52 27 posted update 01 0001 1357.10 1641.10 1357.10 0.00",
    // audit: FIXPOST-053
    "53 15 posted create 03 0001 -358.44 790.67 660.11 -358.44",
    // audit: FIXPOST-054
    "54 39 posted update 01 0001 1016.66 1788.67 1016.66 -70.99",
    // audit: FIXPOST-055
    "55 43 posted create 03 0001 -243.00 367.00 0.00 -243.00",
    // audit: FIXPOST-056
    "56 16 posted update 01 0001 79.32 812.32 79.32 0.00",
    // audit: FIXPOST-057
    "57 11 posted create 03 0001 -744.77 -532.77 0.00 -744.77",
    // audit: FIXPOST-058
    "58 39 posted update 01 0001 1402.43 2174.44 1402.43 -70.99",
    // audit: FIXPOST-059
    "59 13 posted update 01 0001 326.44 310.67 326.44 -56.77",
    // audit: FIXPOST-060
    "60 34 posted update 01 0001 299.33 552.33 299.33 0.00",
    // audit: FIXPOST-061
    "61 6 posted update 01 0001 1019.54 1237.54 1019.54 0.00",
    // audit: FIXPOST-062
    "62 47 posted update 01 0001 1258.55 1290.55 1258.55 0.00",
    // audit: FIXPOST-063
    "63 41 posted update 01 0001 1864.87 2239.87 1864.87 0.00",
    // audit: FIXPOST-064
    "64 28 posted update 01 0001 809.00 877.00 809.00 0.00",
    // audit: FIXPOST-065
    "65 8 posted create 03 0001 -835.11 577.33 807.44 -835.11",
    // audit: FIXPOST-066
    "66 30 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-067
    "67 31 posted update 01 0001 495.66 526.66 495.66 0.00",
    // audit: FIXPOST-068
    "68 9 posted update 01 0001 1267.44 1827.44 1267.44 0.00",
    // audit: FIXPOST-069
    "69 17 posted update 01 0001 343.77 376.77 343.77 0.00",
    // audit: FIXPOST-070
    "70 5 posted create 03 0001 -428.99 -16.11 67.88 -428.99",
    // audit: FIXPOST-071
    "71 11 posted update 01 0001 160.99 -371.78 160.99 -744.77",
    // audit: FIXPOST-072
    "72 30 posted create 03 0001 -930.33 -898.89 29.44 -930.33",
    // audit: FIXPOST-073
    "73 33 posted create 03 0001 -445.55 734.00 769.55 -445.55",
    // audit: FIXPOST-074
    "74 38 posted update 01 0001 816.77 1428.77 816.77 0.00",
    // audit: FIXPOST-075
    "75 22 posted update 01 0001 1249.99 359.33 1249.99 -945.66",
    // audit: FIXPOST-076
    "76 47 posted update 01 0001 1598.10 1630.10 1598.10 0.00",
    // audit: FIXPOST-077
    "77 42 posted update 01 0001 905.00 1207.00 905.00 0.00",
    // audit: FIXPOST-078
    "78 5 posted update 01 0001 414.87 330.88 414.87 -428.99",
    // audit: FIXPOST-079
    "79 28 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-080
    "80 38 posted update 01 0001 1074.32 1686.32 1074.32 0.00",
    // audit: FIXPOST-081
    "81 36 posted update 01 0001 998.99 1108.99 998.99 0.00",
    // audit: FIXPOST-082
    "82 48 posted update 01 0001 526.11 216.23 526.11 -535.88",
    // audit: FIXPOST-083
    "83 12 posted update 01 0001 210.33 386.33 210.33 0.00",
    // audit: FIXPOST-084
    "84 38 posted update 01 0001 1536.31 2148.31 1536.31 0.00",
    // audit: FIXPOST-085
    "85 10 posted update 01 0001 551.88 710.88 551.88 0.00",
    // audit: FIXPOST-086
    "86 48 posted update 01 0001 1290.33 980.45 1290.33 -535.88",
    // audit: FIXPOST-087
    "87 2 posted create 03 0001 -763.00 -249.89 355.11 -763.00",
    // audit: FIXPOST-088
    "88 21 posted update 01 0001 40.00 152.00 40.00 0.00",
    // audit: FIXPOST-089
    "89 32 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-090
    "90 42 posted update 01 0001 994.99 1296.99 994.99 0.00",
    // audit: FIXPOST-091
    "91 49 posted update 01 0001 609.43 709.43 609.43 0.00",
    // audit: FIXPOST-092
    "92 38 posted update 01 0001 2242.42 2854.42 2242.42 0.00",
    // audit: FIXPOST-093
    "93 43 posted update 01 0001 936.22 1303.22 936.22 -243.00",
    // audit: FIXPOST-094
    "94 47 posted create 03 0001 -585.44 1044.66 1598.10 -585.44",
    // audit: FIXPOST-095
    "95 45 posted update 01 0001 2407.54 2593.54 2407.54 0.00",
    // audit: FIXPOST-096
    "96 38 posted update 01 0001 2987.75 3599.75 2987.75 0.00",
    // audit: FIXPOST-097
    "97 8 posted update 01 0001 1536.21 1306.10 1536.21 -835.11",
    // audit: FIXPOST-098
    "98 23 posted update 01 0001 948.22 1052.22 948.22 0.00",
    // audit: FIXPOST-099
    "99 2 posted update 01 0001 925.10 320.10 925.10 -763.00",
    // audit: FIXPOST-100
    "100 25 posted update 01 0001 1042.66 1103.66 1042.66 0.00",
    // audit: FIXPOST-101
    "101 26 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-102
    "102 50 posted update 01 0001 197.88 689.88 197.88 0.00",
    // audit: FIXPOST-103
    "103 32 posted create 03 0001 -742.99 460.56 1173.55 -742.99",
    // audit: FIXPOST-104
    "104 14 posted update 01 0001 1976.21 1991.21 1976.21 0.00",
    // audit: FIXPOST-105
    "105 24 posted update 01 0001 997.88 1397.88 997.88 0.00",
    // audit: FIXPOST-106
    "106 29 posted create 03 0001 -852.33 1047.10 1560.43 -852.33",
    // audit: FIXPOST-107
    "107 42 posted update 01 0001 1547.76 1849.76 1547.76 0.00",
    // audit: FIXPOST-108
    "108 11 posted update 01 0001 606.99 74.22 606.99 -744.77",
    // audit: FIXPOST-109
    "109 20 posted update 01 0001 1520.77 970.77 1520.77 -919.00",
    // audit: FIXPOST-110
    "110 18 posted update 01 0001 1004.99 1148.99 1004.99 0.00",
    // audit: FIXPOST-111
    "111 3 posted update 01 0001 1114.88 1261.88 1114.88 0.00",
    // audit: FIXPOST-112
    "112 45 posted create 03 0001 -344.77 2248.77 2407.54 -344.77",
    // audit: FIXPOST-113
    "113 28 posted create 03 0001 -75.22 801.78 809.00 -75.22",
    // audit: FIXPOST-114
    "114 7 posted update 01 0001 933.10 1126.10 933.10 0.00",
    // audit: FIXPOST-115
    "115 46 posted update 01 0001 1339.65 1735.65 1339.65 0.00",
    // audit: FIXPOST-116
    "116 18 posted update 01 0001 1865.76 2009.76 1865.76 0.00",
    // audit: FIXPOST-117
    "117 34 posted update 01 0001 650.99 903.99 650.99 0.00",
    // audit: FIXPOST-118
    "118 6 posted create 03 0001 -396.22 841.32 1019.54 -396.22",
    // audit: FIXPOST-119
    "119 5 posted update 01 0001 669.64 585.65 669.64 -428.99",
    // audit: FIXPOST-120
    "120 44 posted update 01 0001 688.55 736.22 688.55 -215.33",
    // audit: FIXPOST-121
    "121 24 posted update 01 0001 1614.88 2014.88 1614.88 0.00",
    // audit: FIXPOST-122
    "122 41 posted create 03 0001 -756.66 1483.21 1864.87 -756.66",
    // audit: FIXPOST-123
    "123 19 posted update 01 0001 949.77 1429.77 949.77 0.00",
    // audit: FIXPOST-124
    "124 42 posted create 03 0001 -501.44 1348.32 1547.76 -501.44",
    // audit: FIXPOST-125
    "125 42 posted update 01 0001 1762.53 1563.09 1762.53 -501.44",
    // audit: FIXPOST-126
    "126 35 posted update 01 0001 0.99 166.99 0.99 0.00",
    // audit: FIXPOST-127
    "127 33 posted update 01 0001 1764.77 1729.22 1764.77 -445.55",
    // audit: FIXPOST-128
    "128 43 posted update 01 0001 1611.21 1978.21 1611.21 -243.00",
    // audit: FIXPOST-129
    "129 47 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-130
    "130 12 posted update 01 0001 755.99 931.99 755.99 0.00",
    // audit: FIXPOST-131
    "131 49 posted create 03 0001 -962.77 -253.34 609.43 -962.77",
    // audit: FIXPOST-132
    "132 9 posted update 01 0001 2118.66 2678.66 2118.66 0.00",
    // audit: FIXPOST-133
    "133 33 posted update 01 0001 2557.99 2522.44 2557.99 -445.55",
    // audit: FIXPOST-134
    "134 23 posted update 01 0001 1357.10 1461.10 1357.10 0.00",
    // audit: FIXPOST-135
    "135 19 posted update 01 0001 1079.10 1559.10 1079.10 0.00",
    // audit: FIXPOST-136
    "136 19 posted update 01 0001 1320.32 1800.32 1320.32 0.00",
    // audit: FIXPOST-137
    "137 4 posted update 01 0001 28.33 68.33 28.33 0.00",
    // audit: FIXPOST-138
    "138 4 posted update 01 0001 465.44 505.44 465.44 0.00",
    // audit: FIXPOST-139
    "139 12 posted update 01 0001 1343.10 1519.10 1343.10 0.00",
    // audit: FIXPOST-140
    "140 21 posted update 01 0001 214.66 326.66 214.66 0.00",
    // audit: FIXPOST-141
    "141 46 posted update 01 0001 2253.53 2649.53 2253.53 0.00",
    // audit: FIXPOST-142
    "142 50 posted update 01 0001 1165.32 1657.32 1165.32 0.00",
    // audit: FIXPOST-143
    "143 24 posted update 01 0001 1924.88 2324.88 1924.88 0.00",
    // audit: FIXPOST-144
    "144 34 posted update 01 0001 1471.10 1724.10 1471.10 0.00",
    // audit: FIXPOST-145
    "145 27 posted update 01 0001 1676.98 1960.98 1676.98 0.00",
    // audit: FIXPOST-146
    "146 21 posted update 01 0001 799.54 911.54 799.54 0.00",
    // audit: FIXPOST-147
    "147 24 posted update 01 0001 2086.87 2486.87 2086.87 0.00",
    // audit: FIXPOST-148
    "148 9 posted update 01 0001 2767.66 3327.66 2767.66 0.00",
    // audit: FIXPOST-149
    "149 16 posted update 01 0001 288.32 1021.32 288.32 0.00",
    // audit: FIXPOST-150
    "150 25 posted create 03 0001 -907.00 196.66 1042.66 -907.00",
    // audit: FIXPOST-151
    "151 10 posted update 01 0001 845.32 1004.32 845.32 0.00",
    // audit: FIXPOST-152
    "152 36 posted update 01 0001 1951.54 2061.54 1951.54 0.00",
    // audit: FIXPOST-153
    "153 44 posted update 01 0001 1343.54 1391.21 1343.54 -215.33",
    // audit: FIXPOST-154
    "154 7 posted create 03 0001 -579.88 546.22 933.10 -579.88",
    // audit: FIXPOST-155
    "155 7 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-156
    "156 1 posted update 01 0001 81.44 275.44 81.44 0.00",
    // audit: FIXPOST-157
    "157 2 posted update 01 0001 1580.21 975.21 1580.21 -763.00",
    // audit: FIXPOST-158
    "158 32 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-159
    "159 28 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-160
    "160 2 posted update 01 0001 1745.20 1140.20 1745.20 -763.00",
    // audit: FIXPOST-161
    "161 29 posted update 01 0001 2259.87 1746.54 2259.87 -852.33",
    // audit: FIXPOST-162
    "162 45 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-163
    "163 18 posted create 03 0001 -113.11 1896.65 1865.76 -113.11",
    // audit: FIXPOST-164
    "164 1 posted create 03 0001 -70.77 204.67 81.44 -70.77",
    // audit: FIXPOST-165
    "165 26 posted create 03 0001 -372.00 1193.87 1519.87 -372.00",
    // audit: FIXPOST-166
    "166 33 posted update 01 0001 2760.43 2724.88 2760.43 -445.55",
    // audit: FIXPOST-167
    "167 3 posted create 03 0001 -880.22 381.66 1114.88 -880.22",
    // audit: FIXPOST-168
    "168 35 posted update 01 0001 660.43 826.43 660.43 0.00",
    // audit: FIXPOST-169
    "169 13 posted update 01 0001 730.10 714.33 730.10 -56.77",
    // audit: FIXPOST-170
    "170 14 posted update 01 0001 2171.65 2186.65 2171.65 0.00",
    // audit: FIXPOST-171
    "171 10 posted create 03 0001 -537.44 466.88 845.32 -537.44",
    // audit: FIXPOST-172
    "172 15 posted update 01 0001 1510.33 1640.89 1510.33 -358.44",
    // audit: FIXPOST-173
    "173 48 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-174
    "174 1 posted update 01 0001 566.43 689.66 566.43 -70.77",
    // audit: FIXPOST-175
    "175 46 posted update 01 0001 2684.08 3080.08 2684.08 0.00",
    // audit: FIXPOST-176
    "176 48 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-177
    "177 17 posted create 03 0001 -998.33 -621.56 343.77 -998.33",
    // audit: FIXPOST-178
    "178 17 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-179
    "179 5 posted update 01 0001 831.41 747.42 831.41 -428.99",
    // audit: FIXPOST-180
    "180 40 posted update 01 0001 593.66 636.66 593.66 0.00",
    // audit: FIXPOST-181
    "181 45 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-182
    "182 39 posted update 01 0001 1827.54 2599.55 1827.54 -70.99",
    // audit: FIXPOST-183
    "183 11 posted update 01 0001 1228.21 695.44 1228.21 -744.77",
    // audit: FIXPOST-184
    "184 50 posted create 03 0001 -47.88 1609.44 1165.32 -47.88",
    // audit: FIXPOST-185
    "185 5 posted update 01 0001 1266.85 1182.86 1266.85 -428.99",
    // audit: FIXPOST-186
    "186 43 posted update 01 0001 2180.09 2547.09 2180.09 -243.00",
    // audit: FIXPOST-187
    "187 35 posted update 01 0001 699.98 865.98 699.98 0.00",
    // audit: FIXPOST-188
    "188 4 posted update 01 0001 1201.77 1241.77 1201.77 0.00",
    // audit: FIXPOST-189
    "189 29 posted update 01 0001 3003.98 2490.65 3003.98 -852.33",
    // audit: FIXPOST-190
    "190 30 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-191
    "191 29 posted update 01 0001 3170.97 2657.64 3170.97 -852.33",
    // audit: FIXPOST-192
    "192 8 posted update 01 0001 1632.20 1402.09 1632.20 -835.11",
    // audit: FIXPOST-193
    "193 22 posted update 01 0001 1848.32 957.66 1848.32 -945.66",
    // audit: FIXPOST-194
    "194 12 posted update 01 0001 2058.76 2234.76 2058.76 0.00",
    // audit: FIXPOST-195
    "195 19 posted create 03 0001 -71.66 1728.66 1320.32 -71.66",
    // audit: FIXPOST-196
    "196 37 posted create 03 0001 -132.88 -125.88 0.00 -132.88",
    // audit: FIXPOST-197
    "197 23 posted update 01 0001 1378.21 1482.21 1378.21 0.00",
    // audit: FIXPOST-198
    "198 35 posted update 01 0001 1339.20 1505.20 1339.20 0.00",
    // audit: FIXPOST-199
    "199 1 posted update 01 0001 1164.87 1288.10 1164.87 -70.77",
    // audit: FIXPOST-200
    "200 38 posted create 03 0001 -538.22 3061.53 2987.75 -538.22",
    // audit: FIXPOST-201
    "201 44 posted update 01 0001 1612.76 1660.43 1612.76 -215.33",
    // audit: FIXPOST-202
    "202 21 posted update 01 0001 888.65 1000.65 888.65 0.00",
    // audit: FIXPOST-203
    "203 33 posted update 01 0001 3389.98 3354.43 3389.98 -445.55",
    // audit: FIXPOST-204
    "204 3 posted update 01 0001 1669.10 935.88 1669.10 -880.22",
    // audit: FIXPOST-205
    "205 43 posted update 01 0001 2924.31 3291.31 2924.31 -243.00",
    // audit: FIXPOST-206
    "206 6 posted update 01 0001 1738.09 1559.87 1738.09 -396.22",
    // audit: FIXPOST-207
    "207 16 posted update 01 0001 921.87 1654.87 921.87 0.00",
    // audit: FIXPOST-208
    "208 18 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-209
    "209 16 posted update 01 0001 1346.87 2079.87 1346.87 0.00",
    // audit: FIXPOST-210
    "210 46 posted create 03 0001 -435.00 2645.08 2684.08 -435.00",
    // audit: FIXPOST-211
    "211 14 posted create 03 0001 -710.66 1475.99 2171.65 -710.66",
    // audit: FIXPOST-212
    "212 11 posted update 01 0001 1419.21 886.44 1419.21 -744.77",
    // audit: FIXPOST-213
    "213 34 posted update 01 0001 2168.54 2421.54 2168.54 0.00",
    // audit: FIXPOST-214
    "214 50 posted update 01 0001 1260.09 1704.21 1260.09 -47.88",
    // audit: FIXPOST-215
    "215 40 posted update 01 0001 675.10 718.10 675.10 0.00",
    // audit: FIXPOST-216
    "216 25 posted update 01 0001 1234.66 388.66 1234.66 -907.00",
    // audit: FIXPOST-217
    "217 19 posted update 01 0001 1650.31 2058.65 1650.31 -71.66",
    // audit: FIXPOST-218
    "218 24 posted update 01 0001 2163.87 2563.87 2163.87 0.00",
    // audit: FIXPOST-219
    "219 12 posted create 03 0001 -762.44 1472.32 2058.76 -762.44",
    // audit: FIXPOST-220
    "220 15 posted update 01 0001 2493.88 2624.44 2493.88 -358.44",
    // audit: FIXPOST-221
    "221 40 posted create 03 0001 -25.99 692.11 675.10 -25.99",
    // audit: FIXPOST-222
    "222 36 posted update 01 0001 2309.76 2419.76 2309.76 0.00",
    // audit: FIXPOST-223
    "223 21 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-224
    "224 17 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-225
    "225 14 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-226
    "226 35 posted create 03 0001 -499.99 1005.21 1339.20 -499.99",
    // audit: FIXPOST-227
    "227 30 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-228
    "228 43 posted update 01 0001 2938.19 3305.19 2938.19 -243.00",
    // audit: FIXPOST-229
    "229 9 posted update 01 0001 2907.66 3467.66 2907.66 0.00",
    // audit: FIXPOST-230
    "230 11 posted update 01 0001 2072.76 1539.99 2072.76 -744.77",
    // audit: FIXPOST-231
    "231 1 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-232
    "232 8 posted update 01 0001 2368.19 2138.08 2368.19 -835.11",
    // audit: FIXPOST-233
    "233 19 posted update 01 0001 2639.19 3047.53 2639.19 -71.66",
    // audit: FIXPOST-234
    "234 37 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-235
    "235 37 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-236
    "236 20 posted update 01 0001 1793.88 1243.88 1793.88 -919.00",
    // audit: FIXPOST-237
    "237 23 posted update 01 0001 1443.65 1547.65 1443.65 0.00",
    // audit: FIXPOST-238
    "238 25 posted update 01 0001 1544.10 698.10 1544.10 -907.00",
    // audit: FIXPOST-239
    "239 34 posted update 01 0001 2573.87 2826.87 2573.87 0.00",
    // audit: FIXPOST-240
    "240 1 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-241
    "241 49 posted update 01 0001 879.98 17.21 879.98 -962.77",
    // audit: FIXPOST-242
    "242 47 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-243
    "243 7 posted update 01 0001 1252.10 865.22 1252.10 -579.88",
    // audit: FIXPOST-244
    "244 3 posted update 01 0001 1896.32 1163.10 1896.32 -880.22",
    // audit: FIXPOST-245
    "245 31 posted update 01 0001 664.32 695.32 664.32 0.00",
    // audit: FIXPOST-246
    "246 15 posted update 01 0001 3297.87 3428.43 3297.87 -358.44",
    // audit: FIXPOST-247
    "247 27 posted update 01 0001 2652.09 2936.09 2652.09 0.00",
    // audit: FIXPOST-248
    "248 6 posted update 01 0001 1791.64 1613.42 1791.64 -396.22",
    // audit: FIXPOST-249
    "249 36 posted create 03 0001 -439.33 1980.43 2309.76 -439.33",
    // audit: FIXPOST-250
    "250 24 posted create 03 0001 -641.77 1922.10 2163.87 -641.77",
    // audit: FIXPOST-251
    "251 7 posted update 01 0001 1287.09 900.21 1287.09 -579.88",
    // audit: FIXPOST-252
    "252 18 posted update 01 0001 2661.76 2692.65 2661.76 -113.11",
    // audit: FIXPOST-253
    "253 17 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-254
    "254 2 posted update 01 0001 2339.97 1734.97 2339.97 -763.00",
    // audit: FIXPOST-255
    "255 31 posted create 03 0001 -362.22 333.10 664.32 -362.22",
    // audit: FIXPOST-256
    "256 21 posted create 03 0001 -41.77 958.88 888.65 -41.77",
    // audit: FIXPOST-257
    "257 50 posted update 01 0001 1501.75 1945.87 1501.75 -47.88",
    // audit: FIXPOST-258
    "258 34 posted create 03 0001 -457.55 2369.32 2573.87 -457.55",
    // audit: FIXPOST-259
    "259 13 posted update 01 0001 1432.32 1416.55 1432.32 -56.77",
    // audit: FIXPOST-260
    "260 4 posted update 01 0001 1483.32 1523.32 1483.32 0.00",
    // audit: FIXPOST-261
    "261 25 posted update 01 0001 1589.76 743.76 1589.76 -907.00",
    // audit: FIXPOST-262
    "262 16 posted create 03 0001 -270.99 1808.88 1346.87 -270.99",
    // audit: FIXPOST-263
    "263 10 posted update 01 0001 925.98 547.54 925.98 -537.44",
    // audit: FIXPOST-264
    "264 49 posted update 01 0001 1078.64 215.87 1078.64 -962.77",
    // audit: FIXPOST-265
    "265 4 posted create 03 0001 -322.99 1200.33 1483.32 -322.99",
    // audit: FIXPOST-266
    "266 22 posted update 01 0001 2617.20 1726.54 2617.20 -945.66",
    // audit: FIXPOST-267
    "267 36 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-268
    "268 44 posted update 01 0001 2012.20 2059.87 2012.20 -215.33",
    // audit: FIXPOST-269
    "269 12 posted update 01 0001 2357.09 1770.65 2357.09 -762.44",
    // audit: FIXPOST-270
    "270 40 posted update 01 0001 869.43 886.44 869.43 -25.99",
    // audit: FIXPOST-271
    "271 31 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-272
    "272 31 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-273
    "273 35 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-274
    "274 6 posted update 01 0001 2750.41 2572.19 2750.41 -396.22",
    // audit: FIXPOST-275
    "275 4 posted update 01 0001 1945.65 1662.66 1945.65 -322.99",
    // audit: FIXPOST-276
    "276 20 posted update 01 0001 1879.88 1329.88 1879.88 -919.00",
    // audit: FIXPOST-277
    "277 27 posted update 01 0001 3584.64 3868.64 3584.64 0.00",
    // audit: FIXPOST-278
    "278 46 posted update 01 0001 3577.41 3538.41 3577.41 -435.00",
    // audit: FIXPOST-279
    "279 13 posted update 01 0001 2191.54 2175.77 2191.54 -56.77",
    // audit: FIXPOST-280
    "280 31 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-281
    "281 27 posted create 03 0001 -372.99 3495.65 3584.64 -372.99",
    // audit: FIXPOST-282
    "282 13 posted update 01 0001 2875.87 2860.10 2875.87 -56.77",
    // audit: FIXPOST-283
    "283 14 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-284
    "284 9 posted create 03 0001 -260.11 3207.55 2907.66 -260.11",
    // audit: FIXPOST-285
    "285 48 posted update 01 0001 1512.77 1202.89 1512.77 -535.88",
    // audit: FIXPOST-286
    "286 22 posted update 01 0001 3328.86 2438.20 3328.86 -945.66",
    // audit: FIXPOST-287
    "287 23 posted update 01 0001 1881.42 1985.42 1881.42 0.00",
    // audit: FIXPOST-288
    "288 41 posted update 01 0001 2084.87 1703.21 2084.87 -756.66",
    // audit: FIXPOST-289
    "289 39 posted update 01 0001 2050.87 2822.88 2050.87 -70.99",
    // audit: FIXPOST-290
    "290 20 posted update 01 0001 2453.10 1903.10 2453.10 -919.00",
    // audit: FIXPOST-291
    "291 41 posted update 01 0001 2399.53 2017.87 2399.53 -756.66",
    // audit: FIXPOST-292
    "292 37 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-293
    "293 3 posted update 01 0001 2590.87 1857.65 2590.87 -880.22",
    // audit: FIXPOST-294
    "294 23 posted create 03 0001 -835.44 1149.98 1881.42 -835.44",
    // audit: FIXPOST-295
    "295 15 posted update 01 0001 4081.98 4212.54 4081.98 -358.44",
    // audit: FIXPOST-296
    "296 42 posted update 01 0001 2164.75 1965.31 2164.75 -501.44",
    // audit: FIXPOST-297
    "297 49 posted update 01 0001 1996.08 1133.31 1996.08 -962.77",
    // audit: FIXPOST-298
    "298 40 posted update 01 0001 1635.09 1652.10 1635.09 -25.99",
    // audit: FIXPOST-299
    "299 37 rejected 0102 OVERLIMIT TRANSACTION",
    // audit: FIXPOST-300
    "300 10 posted update 01 0001 1529.20 1150.76 1529.20 -537.44",
];

/// A `tran_cat_balances` key: account, type and category.
type CategoryKey = (u64, [u8; 2], u64);

/// What a posted record leaves: its category balance key and value, whether the posting created
/// that row, and its account's balance, cycle credit and cycle debit.
struct Posting {
    created: bool,
    key: CategoryKey,
    cat_bal: Decimal,
    bal: Decimal,
    cr: Decimal,
    db: Decimal,
}

/// The outcome of one `FIXPOST` row.
enum Outcome {
    Posted(Posting),
    Rejected {
        reason: &'static str,
        desc: &'static str,
    },
}

/// One `FIXPOST-<line>` row: the `dailytran.txt` line, the account its card cross-references,
/// and the record's outcome.
struct PostRow {
    line: usize,
    acct_id: u64,
    outcome: Outcome,
}

/// One line of [`FIXPOST_ROWS`]. Amounts parse exactly, with no floating point.
fn parse_row(text: &'static str) -> TestResult<PostRow> {
    let malformed = || format!("malformed FIXPOST row `{text}`");
    let mut parts = text.splitn(4, ' ');
    let (Some(line), Some(acct_id), Some(kind), Some(rest)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(malformed().into());
    };
    let acct_id: u64 = acct_id.parse()?;
    let outcome = match kind {
        "rejected" => {
            let (reason, desc) = rest.split_once(' ').ok_or_else(malformed)?;
            Outcome::Rejected { reason, desc }
        }
        "posted" => {
            let fields: Vec<&str> = rest.split(' ').collect();
            let [write, type_cd, cat_cd, cat_bal, bal, cr, db] = fields.as_slice() else {
                return Err(malformed().into());
            };
            let created = match *write {
                "create" => true,
                "update" => false,
                _ => return Err(malformed().into()),
            };
            Outcome::Posted(Posting {
                created,
                key: (acct_id, type_cd.as_bytes().try_into()?, cat_cd.parse()?),
                cat_bal: Decimal::from_str_exact(cat_bal)?,
                bal: Decimal::from_str_exact(bal)?,
                cr: Decimal::from_str_exact(cr)?,
                db: Decimal::from_str_exact(db)?,
            })
        }
        _ => return Err(malformed().into()),
    };
    Ok(PostRow {
        line: line.parse()?,
        acct_id,
        outcome,
    })
}

/// The rows of [`FIXPOST_ROWS`], checked to be lines 1 to 300 in order.
fn fixpost_rows() -> TestResult<Vec<PostRow>> {
    let rows = FIXPOST_ROWS
        .into_iter()
        .map(parse_row)
        .collect::<TestResult<Vec<_>>>()?;
    assert!(
        rows.iter().zip(1..).all(|(row, line)| row.line == line),
        "the FIXPOST rows are not lines 1 to 300 in input order"
    );
    Ok(rows)
}

/// A directory of its own under the system temporary directory for one test's outputs and
/// scratch input, removed with its contents when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> io::Result<Self> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "carddemo-posttran-i9-{name}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Writes `bytes` to the scratch file `name` and returns its path.
    fn write(&self, name: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        let path = self.0.join(name);
        fs::write(&path, bytes)?;
        Ok(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // A directory that cannot be removed has a name no later run reuses, so it harms nothing
        // but disk space; a panic here would hide the test's own outcome.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The reject generation `name` under the outputs directory `var_dir` (CBTRN02C-028).
fn generation(var_dir: &Path, name: &str) -> PathBuf {
    var_dir.join("dalyrejs").join(name)
}

/// Runs `carddemo-batch posttran --dalytran <dalytran>` from `modern/` against the database of
/// `guard`, with every output directory under `var_dir`, and returns its exit status and output.
///
/// The child inherits the test's environment, so the coverage profile variables reach it
/// (Coverage Floors), with the variables the job reads set for this run. `RUST_LOG` is `info`, so
/// the job-log events reach standard output (CBTRN02C-034), and span export is off, so that every
/// event is one JSON line.
async fn run_posttran(
    guard: &PostgresGuard,
    var_dir: &Path,
    dalytran: &Path,
) -> TestResult<Output> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command
        .arg("posttran")
        .arg("--dalytran")
        .arg(dalytran)
        .current_dir(test_support::modern_dir())
        .env("DATABASE_URL", guard.database_url())
        .env("CARDDEMO_VAR_DIR", var_dir)
        .env("CARDDEMO_STATEMENT_DIR", var_dir.join("statements"))
        .env("CARDDEMO_INTRDR_DIR", var_dir.join("jobs"))
        .env("CARDDEMO_FIXTURE_DIR", fixture_dir())
        .env("RUST_LOG", "info")
        .env_remove("CARDDEMO_OTEL_EXPORT");
    Ok(tokio::task::spawn_blocking(move || command.output()).await??)
}

/// The lines of standard output that are not JSON events: the paths the job wrote.
fn written_paths(output: &Output) -> Vec<PathBuf> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('{'))
        .map(PathBuf::from)
        .collect()
}

/// The JSON events on standard output, in order.
fn events(output: &Output) -> Vec<String> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| line.starts_with('{'))
        .map(str::to_owned)
        .collect()
}

/// The message of every event, in order. The messages the tests look for hold no quote or
/// backslash, so each ends at the first quote after its key.
fn messages(output: &Output) -> Vec<String> {
    events(output)
        .iter()
        .filter_map(|event| {
            let (_, rest) = event.split_once(MESSAGE_KEY)?;
            rest.split_once('"').map(|(message, _)| message.to_owned())
        })
        .collect()
}

/// Whether the JSON `event` holds field `name` with the JSON value `value`.
fn has_field(event: &str, name: &str, value: &str) -> bool {
    let field = format!("\"{name}\":{value}");
    event
        .match_indices(&field)
        .any(|(at, _)| matches!(event.as_bytes().get(at + field.len()), Some(b',' | b'}')))
}

/// Asserts that one event of `output` holds every field of `fields`, each a name and its JSON
/// value; on failure the events are shown with every fixture card number masked.
fn assert_event(output: &Output, fields: &[(&str, &str)], needles: &Needles) {
    let events = events(output);
    assert!(
        events.iter().any(|event| {
            fields
                .iter()
                .all(|(name, value)| has_field(event, name, value))
        }),
        "no event holds {fields:?}; events: {}",
        needles.mask(events.join("\n").as_bytes())
    );
}

/// The bodies of the lines of `bytes`, each without its LF. Every `dailytran.txt` line is a
/// 350-byte body ending LF alone, the last included (FIXFRM-005), which this checks, so no CR is
/// stripped.
fn dailytran_lines(bytes: &[u8]) -> TestResult<Vec<&[u8]>> {
    let mut lines: Vec<&[u8]> = bytes.split(|&byte| byte == b'\n').collect();
    if lines.pop().is_none_or(|tail| !tail.is_empty()) {
        return Err("dailytran.txt does not end with LF".into());
    }
    if let Some(index) = lines.iter().position(|line| line.len() != DALYTRAN_LEN) {
        return Err(format!("dailytran.txt line {} is not a 350-byte body", index + 1).into());
    }
    Ok(lines)
}

/// `dailytran.txt` line 1, decoded by codec A: the template of the scratch records.
fn first_daily_record() -> TestResult<DailyTransactionRecord> {
    decode_fixture::<DailyTransactionRecord>(&fs::read(fixture_path(DAILYTRAN))?)?
        .into_iter()
        .next()
        .map(|framed| framed.record)
        .ok_or_else(|| "dailytran.txt holds no record".into())
}

/// `template` with [`SCRATCH_CARD`] as its card number, `type_cd` as its type and `cents`
/// hundredths as its amount, encoded by codec A to its 350 bytes: scratch input that holds no
/// fixture card number.
fn scratch_record(
    template: &DailyTransactionRecord,
    type_cd: [u8; 2],
    cents: i64,
) -> TestResult<Vec<u8>> {
    let mut record = template.clone();
    record.card_num = CardNumber::new(PicX::from_array(SCRATCH_CARD));
    record.type_cd = PicX::from_array(type_cd);
    record.amt = Money::new(Decimal::new(cents, 2))?;
    Ok(record.encode())
}

/// The cross-reference of [`SCRATCH_CARD`] to account and customer [`SCRATCH_ACCOUNT`].
fn scratch_xref() -> TestResult<CardXrefRecord> {
    Ok(CardXrefRecord {
        card_num: CardNumber::new(PicX::from_array(SCRATCH_CARD)),
        cust_id: PicU::new(SCRATCH_ACCOUNT)?,
        acct_id: PicU::new(SCRATCH_ACCOUNT)?,
        filler: PicX::spaces(),
    })
}

/// `records` as the lines of an input file, each followed by LF.
fn lines_of(records: &[&[u8]]) -> Vec<u8> {
    records
        .iter()
        .flat_map(|record| record.iter().copied().chain([b'\n']))
        .collect()
}

/// `bytes` as records of `width` bytes, each followed by LF (CBTRN02C-028), without their LF.
fn fixed_records(bytes: &[u8], width: usize) -> TestResult<Vec<&[u8]>> {
    let framed = width + 1;
    if bytes.len() % framed != 0 {
        return Err(format!(
            "{} bytes are no whole number of {framed}-byte lines",
            bytes.len()
        )
        .into());
    }
    bytes
        .chunks_exact(framed)
        .enumerate()
        .map(|(index, chunk)| match chunk.split_last() {
            Some((&b'\n', record)) => Ok(record),
            _ => Err(format!("record {index} is not followed by LF").into()),
        })
        .collect()
}

/// The reject record of the input body `line`: its 350 bytes verbatim, the four-digit `reason`,
/// then `desc` padded with spaces to 76 bytes, 430 bytes in all (CBTRN02C-027).
fn expected_reject(line: &[u8], reason: &str, desc: &str) -> TestResult<Vec<u8>> {
    if line.len() != DALYTRAN_LEN || reason.len() != REASON_LEN || desc.len() > DESC_LEN {
        return Err(format!("no reject record has reason `{reason}` and `{desc}`").into());
    }
    let mut record = Vec::with_capacity(REJECT_LEN);
    record.extend_from_slice(line);
    record.extend_from_slice(reason.as_bytes());
    record.extend_from_slice(desc.as_bytes());
    record.resize(REJECT_LEN, b' ');
    Ok(record)
}

/// The first offset at which `actual` and `expected` differ, the shorter one's length when one
/// is a prefix of the other, or `None` when they are equal.
fn first_difference(actual: &[u8], expected: &[u8]) -> Option<usize> {
    actual
        .iter()
        .zip(expected)
        .position(|(got, want)| got != want)
        .or_else(|| (actual.len() != expected.len()).then_some(actual.len().min(expected.len())))
}

/// Whether `text` is 26 bytes of layout `D`, `YYYY-MM-DD-HH.MM.SS.ffffff`: `-` at offsets 4, 7
/// and 10, `.` at 13, 16 and 19, and a digit everywhere else (PIC-019).
fn is_db2_timestamp(text: &[u8]) -> bool {
    text.len() == 26
        && text.iter().enumerate().all(|(offset, &byte)| match offset {
            4 | 7 | 10 => byte == b'-',
            13 | 16 | 19 => byte == b'.',
            _ => byte.is_ascii_digit(),
        })
}

/// Reports a value that a halt or a halt candidate blocks; nothing is asserted for it
/// (AAP 0.8.5).
fn blocked(rule: &str, halt: &str, what: &str) {
    println!("BLOCKED {rule} ({halt}): {what}");
}

/// The 50 fixture card numbers, as search needles only: never printed or formatted into a
/// message (Cardholder Data Redaction).
struct Needles(Vec<String>);

impl Needles {
    fn load() -> TestResult<Self> {
        Ok(Self(fixture_card_numbers()?))
    }

    /// `bytes` as text with every fixture card number replaced by `<card number>`.
    fn mask(&self, bytes: &[u8]) -> String {
        let mut text = String::from_utf8_lossy(bytes).into_owned();
        for needle in &self.0 {
            text = text.replace(needle.as_str(), "<card number>");
        }
        text
    }

    /// The exit status and masked standard error of a run, for an assertion message.
    fn report(&self, output: &Output) -> String {
        format!(
            "exit status {:?}; standard error: {}",
            output.status.code(),
            self.mask(&output.stderr)
        )
    }

    /// Asserts that neither output stream of a run holds a fixture card number (CBTRN02C-037).
    fn assert_absent(&self, output: &Output) {
        for (stream, bytes) in [
            ("standard output", &output.stdout),
            ("standard error", &output.stderr),
        ] {
            let text = String::from_utf8_lossy(bytes);
            let found = self
                .0
                .iter()
                .filter(|needle| text.contains(needle.as_str()))
                .count();
            assert_eq!(found, 0, "{found} fixture card numbers appear on {stream}");
        }
    }
}

/// The three tables `posttran` writes, as a run leaves them.
#[derive(PartialEq)]
struct Tables {
    transactions: Vec<TransactionRecord>,
    accounts: Vec<AccountRecord>,
    balances: Vec<TranCatBalanceRecord>,
}

impl Tables {
    async fn read(store: &mut PgStore) -> TestResult<Self> {
        Ok(Self {
            transactions: TransactionRepository::find_all_in_card_order(store).await?,
            accounts: AccountRepository::find_all(store).await?,
            balances: TranCatBalanceRepository::find_all(store).await?,
        })
    }
}

/// The audited reject records over `lines`, the `dailytran.txt` bodies: each rejected `FIXPOST`
/// row's line and its 430 bytes, in input order.
fn expected_rejects(rows: &[PostRow], lines: &[&[u8]]) -> TestResult<Vec<(usize, Vec<u8>)>> {
    let expected = rows
        .iter()
        .filter_map(|row| match row.outcome {
            Outcome::Rejected { reason, desc } => Some((row.line, reason, desc)),
            Outcome::Posted(_) => None,
        })
        .map(|(line, reason, desc)| -> TestResult<(usize, Vec<u8>)> {
            let body = line
                .checked_sub(1)
                .and_then(|index| lines.get(index))
                .ok_or_else(|| format!("dailytran.txt has no line {line}"))?;
            Ok((line, expected_reject(body, reason, desc)?))
        })
        .collect::<TestResult<Vec<_>>>()?;
    // audit: FIXPOST-902
    assert_eq!(expected.len(), REJECT_COUNT, "rejected FIXPOST rows");
    Ok(expected)
}

/// Asserts that `written`, a reject generation, holds exactly the `expected` records, each
/// followed by LF, record by record and in input order. A mismatch names the record, its input
/// line and the first differing byte offset, never a byte (Cardholder Data Redaction).
fn assert_rejects(written: &[u8], expected: &[(usize, Vec<u8>)]) -> TestResult {
    let records = fixed_records(written, REJECT_LEN)?;
    assert_eq!(records.len(), expected.len(), "reject records written");
    for (index, (record, (line, want))) in records.iter().zip(expected).enumerate() {
        let offset = first_difference(record, want);
        assert!(
            offset.is_none(),
            "reject record {index} (dailytran.txt line {line}) differs from its audited bytes \
             at byte offset {offset:?}"
        );
    }
    Ok(())
}

/// Whether `message` is one of the job-log lines of a run that ends normally (CBTRN02C-034).
fn is_job_log(message: &str) -> bool {
    message == START_LINE
        || message == END_LINE
        || message.starts_with("TRANSACTIONS ")
        || message.starts_with(NOT_FOUND_PREFIX)
}

/// Asserts the job log of a run that ended normally: the start line, one not-found line before
/// each of the `created` category balances, then `counts` and the end line. Of each not-found
/// line its two literals are asserted, and the 17 key bytes between them are reported blocked.
fn assert_job_log(output: &Output, created: usize, counts: [&str; 2]) {
    let log: Vec<String> = messages(output)
        .into_iter()
        .filter(|message| is_job_log(message))
        .collect();
    let shapes: Vec<&str> = log
        .iter()
        .map(|message| {
            if message.starts_with(NOT_FOUND_PREFIX) && message.ends_with(NOT_FOUND_SUFFIX) {
                NOT_FOUND_PREFIX
            } else {
                message.as_str()
            }
        })
        .collect();
    let mut expected = vec![START_LINE];
    expected.extend(iter::repeat_n(NOT_FOUND_PREFIX, created));
    expected.extend(counts);
    expected.push(END_LINE);
    // audit: CBTRN02C-016
    // audit: CBTRN02C-034
    assert_eq!(shapes, expected, "job log");
    if created > 0 {
        blocked(
            "CBTRN02C-016",
            "X-POSTTRAN-H2",
            "the 17 key bytes of each not-found line",
        );
    }
}

/// Asserts that `output` is a run that abended: exit status 12 (CBTRN02C-043); `line`, when
/// given, shown before a status line and `ABENDING PROGRAM` (CBTRN02C-041), or else
/// `ABENDING PROGRAM` alone; no path printed and no reject generation kept under `var_dir`
/// (CBTRN02C-044); and no fixture card number shown (CBTRN02C-037). Returns the status line
/// shown after `line`, whose four status characters only the caller can judge (CBTRN02C-042).
fn assert_abended(
    output: &Output,
    var_dir: &Path,
    line: Option<&str>,
    needles: &Needles,
) -> Option<String> {
    // audit: CBTRN02C-043
    assert_eq!(
        output.status.code(),
        Some(ABEND_EXIT_STATUS),
        "{}",
        needles.report(output)
    );
    let messages = messages(output);
    let abend = messages.iter().position(|message| message == ABEND_LINE);
    assert!(abend.is_some(), "the run shows no `{ABEND_LINE}` line");
    let status = line.zip(abend).map(|(line, abend)| {
        let shown = abend
            .checked_sub(2)
            .and_then(|first| messages.get(first..abend));
        let Some([failure, status]) = shown else {
            panic!("no failure and status line precede `{ABEND_LINE}`");
        };
        // audit: CBTRN02C-041
        assert_eq!(failure, line, "the failure line");
        assert!(
            status.starts_with(STATUS_PREFIX) && status.len() == STATUS_LEN,
            "`{status}` is not a status line"
        );
        status.clone()
    });
    // audit: CBTRN02C-044
    assert!(
        written_paths(output).is_empty(),
        "an abended run printed a path"
    );
    assert!(
        !generation(var_dir, FIRST_GENERATION).exists(),
        "an abended run kept its reject generation"
    );
    // audit: CBTRN02C-037
    needles.assert_absent(output);
    status
}

/// Reports the four status characters of a failure path other than a duplicate key, which no
/// `PostgreSQL` or file-system error carries (CBTRN02C-042).
fn status_blocked(status: Option<&str>) {
    if let Some(status) = status {
        // audit: CBTRN02C-042
        blocked(
            "CBTRN02C-042",
            "H4",
            &format!("the four status characters of `{status}`"),
        );
    }
}

/// Asserts that `posted` is `input` moved field by field (CBTRN02C-012), with a processing
/// timestamp in layout `D` whose fraction holds hundredths only (CBTRN02C-013).
fn assert_posted(posted: &TransactionRecord, input: &DailyTransactionRecord, line: usize) {
    let mut expected = TransactionRecord::from(input.clone());
    // The system clock's instant, compared by format below.
    expected.proc_ts = posted.proc_ts;
    // Not claimed as parity: halt candidate X-FIXTURES-H1, reported by the caller.
    expected.filler = posted.filler.clone();
    // audit: CBTRN02C-012
    // audit: CBTRN02C-024
    assert!(
        *posted == expected,
        "the transaction of input line {line} differs from its input record"
    );
    let mut text = Vec::new();
    posted.proc_ts.encode_into(&mut text);
    // audit: CBTRN02C-013
    assert_eq!(
        posted.proc_ts.value().map(|stamp| stamp.form()),
        Some(TsForm::Db2),
        "input line {line}: the layout of TRAN-PROC-TS"
    );
    assert!(
        is_db2_timestamp(&text) && text.ends_with(b"0000"),
        "input line {line}: TRAN-PROC-TS `{}` is not YYYY-MM-DD-HH.MM.SS.hh0000",
        String::from_utf8_lossy(&text)
    );
}

/// Asserts that `transactions` holds exactly the records the `FIXPOST` rows post, each its
/// input record of `daily`, and none they reject (FIXPOST-901).
async fn assert_transactions(
    store: &mut PgStore,
    rows: &[PostRow],
    daily: &[DailyTransactionRecord],
) -> TestResult {
    let stored = TransactionRepository::find_all_in_card_order(store).await?;
    let by_id: HashMap<&[u8; 16], &TransactionRecord> = stored
        .iter()
        .map(|record| (record.id.as_bytes(), record))
        .collect();
    let posted_rows = rows
        .iter()
        .filter(|row| matches!(row.outcome, Outcome::Posted(_)))
        .count();
    // audit: FIXPOST-901
    assert_eq!(posted_rows, POSTED_COUNT, "posted FIXPOST rows");
    assert_eq!(stored.len(), POSTED_COUNT, "transactions rows");
    assert_eq!(by_id.len(), stored.len(), "transaction IDs are not unique");
    for row in rows {
        let input = row
            .line
            .checked_sub(1)
            .and_then(|index| daily.get(index))
            .ok_or_else(|| format!("dailytran.txt has no line {}", row.line))?;
        let posted = by_id.get(input.id.as_bytes());
        match row.outcome {
            Outcome::Posted(_) => {
                let posted =
                    posted.ok_or_else(|| format!("input line {} was not posted", row.line))?;
                assert_posted(posted, input, row.line);
            }
            // audit: CBTRN02C-026
            Outcome::Rejected { .. } => {
                assert!(
                    posted.is_none(),
                    "rejected input line {} was posted",
                    row.line
                );
            }
        }
    }
    // audit: CBTRN02C-014
    blocked(
        "CBTRN02C-014",
        "H2",
        "transactions.filler of every posted record (X-FIXTURES-H1)",
    );
    Ok(())
}

/// Asserts every account a posting reaches against the balance and cycle fields of the last
/// `FIXPOST` row that posts to it, earlier postings being visible to later records
/// (CBTRN02C-022).
async fn assert_accounts(store: &mut PgStore, rows: &[PostRow]) -> TestResult {
    let mut expected: BTreeMap<u64, &Posting> = BTreeMap::new();
    for row in rows {
        if let Outcome::Posted(posting) = &row.outcome {
            expected.insert(row.acct_id, posting);
        }
    }
    let accounts = AccountRepository::find_all(store).await?;
    let by_id: HashMap<u64, &AccountRecord> = accounts
        .iter()
        .map(|account| (account.id.get(), account))
        .collect();
    for (acct_id, posting) in &expected {
        let account = by_id
            .get(acct_id)
            .ok_or_else(|| format!("account {acct_id} is missing"))?;
        // audit: CBTRN02C-020
        // audit: CBTRN02C-022
        assert_eq!(
            account.curr_bal.as_decimal(),
            posting.bal,
            "account {acct_id}: ACCT-CURR-BAL"
        );
        // audit: CBTRN02C-021
        assert_eq!(
            account.curr_cyc_credit.as_decimal(),
            posting.cr,
            "account {acct_id}: ACCT-CURR-CYC-CREDIT"
        );
        assert_eq!(
            account.curr_cyc_debit.as_decimal(),
            posting.db,
            "account {acct_id}: ACCT-CURR-CYC-DEBIT"
        );
    }
    Ok(())
}

/// The key of a stored category balance.
fn category_key(balance: &TranCatBalanceRecord) -> CategoryKey {
    (
        balance.acct_id.get(),
        *balance.type_cd.as_bytes(),
        balance.cd.get(),
    )
}

/// Asserts the category balances: every key a posting reaches holds the value of its last
/// `FIXPOST` row, created or updated; every other key keeps its loaded 0.00; and every row's
/// `FILLER` is the 22 `0` characters of the loaded rows (FIXPOST-901).
async fn assert_category_balances(store: &mut PgStore, rows: &[PostRow]) -> TestResult {
    let mut expected: BTreeMap<CategoryKey, &Posting> = BTreeMap::new();
    for row in rows {
        if let Outcome::Posted(posting) = &row.outcome {
            expected.insert(posting.key, posting);
        }
    }
    let balances = TranCatBalanceRepository::find_all(store).await?;
    // audit: FIXPOST-901
    assert_eq!(
        balances.len(),
        CATEGORY_BALANCE_ROWS,
        "tran_cat_balances rows"
    );
    let by_key: BTreeMap<CategoryKey, &TranCatBalanceRecord> = balances
        .iter()
        .map(|balance| (category_key(balance), balance))
        .collect();
    for (key, posting) in &expected {
        let balance = by_key
            .get(key)
            .ok_or_else(|| format!("no category balance {key:?}"))?;
        let write = if posting.created {
            "created"
        } else {
            "updated"
        };
        // audit: CBTRN02C-017
        // audit: CBTRN02C-019
        assert_eq!(
            balance.bal.as_decimal(),
            posting.cat_bal,
            "{write} category balance {key:?}"
        );
    }
    for (key, balance) in &by_key {
        // audit: CBTRN02C-026
        assert!(
            expected.contains_key(key) || balance.bal.as_decimal().is_zero(),
            "category balance {key:?} changed with no posting"
        );
        // audit: CBTRN02C-017
        // audit: CBTRN02C-019
        assert_eq!(
            balance.filler.as_bytes(),
            &CATEGORY_FILLER,
            "category balance {key:?}: FILLER"
        );
    }
    Ok(())
}

/// Asserts that both scratch postings of the duplicate-ID test stand: account
/// [`SCRATCH_ACCOUNT`] and its type-`03` category balance, created by the first record, hold
/// both credits, and every other account and category balance is as it was. The amounts are a
/// few dollars, so no receiving picture drops a digit.
fn assert_both_postings_stand(before: &Tables, after: &Tables, added: Decimal) -> TestResult {
    let mut accounts = before.accounts.clone();
    let account = accounts
        .iter_mut()
        .find(|account| account.id.get() == SCRATCH_ACCOUNT)
        .ok_or("the scratch account is not loaded")?;
    account.curr_bal = Money::new(account.curr_bal.as_decimal() + added)?;
    account.curr_cyc_debit = Money::new(account.curr_cyc_debit.as_decimal() + added)?;
    // audit: CBTRN02C-015
    // audit: CBTRN02C-020
    // audit: CBTRN02C-021
    assert!(
        after.accounts == accounts,
        "the accounts do not hold both credits of the scratch account alone"
    );
    let key: CategoryKey = (SCRATCH_ACCOUNT, SCRATCH_TYPE, 1);
    assert!(
        before
            .balances
            .iter()
            .all(|balance| category_key(balance) != key),
        "the loads hold the scratch category balance"
    );
    let (created, others): (Vec<_>, Vec<_>) = after
        .balances
        .iter()
        .partition(|balance| category_key(balance) == key);
    let [created] = created.as_slice() else {
        panic!("{} scratch category balances, not one", created.len());
    };
    // audit: CBTRN02C-015
    // audit: CBTRN02C-017
    // audit: CBTRN02C-019
    assert_eq!(
        created.bal.as_decimal(),
        added,
        "the created category balance"
    );
    assert!(
        others.into_iter().eq(before.balances.iter()),
        "a category balance other than the scratch one changed"
    );
    // audit: CBTRN02C-018
    blocked(
        "CBTRN02C-018",
        "H2",
        "the FILLER of a category balance created before any read of the run succeeded",
    );
    Ok(())
}

/// i9: over `dailytran.txt`, the reject generation equals the audited reject records byte for
/// byte, record by record and in input order; the run exits with status 4, prints the
/// generation's path, logs the audited job-log lines, leaves its input unchanged and shows no
/// card number.
#[tokio::test(flavor = "multi_thread")]
async fn posttran_rejects_match_audit_byte_for_byte() -> TestResult {
    let rows = fixpost_rows()?;
    let needles = Needles::load()?;
    let dalytran = fixture_path(DAILYTRAN);
    let input = fs::read(&dalytran)?;
    let expected = expected_rejects(&rows, &dailytran_lines(&input)?)?;
    let guard = start_loaded_postgres().await?;
    let scratch = Scratch::new("rejects")?;
    let output = run_posttran(&guard, scratch.path(), &dalytran).await?;
    let status = if expected.is_empty() {
        0
    } else {
        REJECTS_EXIT_STATUS
    };
    // audit: FIXPOST-903
    // audit: CBTRN02C-035
    assert_eq!(
        output.status.code(),
        Some(status),
        "{}",
        needles.report(&output)
    );
    let rejects = generation(scratch.path(), FIRST_GENERATION);
    // audit: CBTRN02C-028
    assert_eq!(written_paths(&output), [rejects.clone()]);
    // audit: FIXPOST-902
    // audit: CBTRN02C-007
    // audit: CBTRN02C-027
    assert_rejects(&fs::read(&rejects)?, &expected)?;
    let created = rows
        .iter()
        .filter(|row| matches!(&row.outcome, Outcome::Posted(posting) if posting.created))
        .count();
    // audit: FIXPOST-903
    assert_job_log(&output, created, FIXTURE_COUNT_LINES);
    // audit: CBTRN02C-033
    assert!(
        fs::read(&dalytran)? == input,
        "posttran changed its input file"
    );
    // audit: CBTRN02C-037
    needles.assert_absent(&output);
    Ok(())
}

/// The table effects of the same run: the 262 posted transactions, the account fields and the
/// category balances that the `FIXPOST` rows state.
#[tokio::test(flavor = "multi_thread")]
async fn posttran_posts_audited_effects() -> TestResult {
    let rows = fixpost_rows()?;
    let needles = Needles::load()?;
    let dalytran = fixture_path(DAILYTRAN);
    // Codec A, which i8 proves, decodes the input records the postings must equal.
    let daily: Vec<DailyTransactionRecord> =
        decode_fixture::<DailyTransactionRecord>(&fs::read(&dalytran)?)?
            .into_iter()
            .map(|framed| framed.record)
            .collect();
    let guard = start_loaded_postgres().await?;
    let scratch = Scratch::new("effects")?;
    let output = run_posttran(&guard, scratch.path(), &dalytran).await?;
    // audit: FIXPOST-903
    assert_eq!(
        output.status.code(),
        Some(REJECTS_EXIT_STATUS),
        "{}",
        needles.report(&output)
    );
    let mut store = PgStore::new(guard.pool().clone());
    assert_transactions(&mut store, &rows, &daily).await?;
    assert_accounts(&mut store, &rows).await?;
    assert_category_balances(&mut store, &rows).await
}

/// GDG `(+1)`: a second run, over a freshly loaded database but into the directory that holds
/// the first run's `DALYREJS.G0001`, writes `DALYREJS.G0002` with the same records and exits
/// with the same status, and `DALYREJS.G0001` is kept.
#[tokio::test(flavor = "multi_thread")]
async fn second_run_writes_next_generation() -> TestResult {
    let needles = Needles::load()?;
    // Each run needs a database of its own: a rerun meets the rows the first run posted.
    let (first_db, second_db) = tokio::try_join!(start_loaded_postgres(), start_loaded_postgres())?;
    let scratch = Scratch::new("generations")?;
    let dalytran = fixture_path(DAILYTRAN);
    let first = run_posttran(&first_db, scratch.path(), &dalytran).await?;
    let second = run_posttran(&second_db, scratch.path(), &dalytran).await?;
    // audit: CBTRN02C-035
    assert_eq!(
        first.status.code(),
        Some(REJECTS_EXIT_STATUS),
        "{}",
        needles.report(&first)
    );
    assert_eq!(
        second.status.code(),
        first.status.code(),
        "{}",
        needles.report(&second)
    );
    let older = generation(scratch.path(), FIRST_GENERATION);
    let newer = generation(scratch.path(), SECOND_GENERATION);
    // audit: CBTRN02C-028
    assert_eq!(written_paths(&first), [older.clone()]);
    assert_eq!(written_paths(&second), [newer.clone()]);
    let offset = first_difference(&fs::read(&newer)?, &fs::read(&older)?);
    assert!(
        offset.is_none(),
        "DALYREJS.G0002 differs from DALYREJS.G0001 at byte offset {offset:?}"
    );
    Ok(())
}

/// Input the job cannot take ends it before any effect: an input it cannot open and a reject
/// directory it cannot create (CBTRN02C-031), a line codec A refuses (CBTRN02C-032) and a body
/// short of the 350-byte record (CBTRN02C-045). An empty input then runs to its end, into the
/// directory the refused runs left without a generation, and writes an empty `DALYREJS.G0001`
/// (CBTRN02C-029).
#[tokio::test(flavor = "multi_thread")]
async fn posttran_abends_before_any_effect_on_input_it_cannot_take() -> TestResult {
    let needles = Needles::load()?;
    let record = scratch_record(&first_daily_record()?, *b"01", 1_000)?;
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let loaded = Tables::read(&mut store).await?;
    let scratch = Scratch::new("refused")?;
    let var_dir = scratch.path().join("var");

    let absent = scratch.path().join("absent.txt");
    let refused = run_posttran(&guard, &var_dir, &absent).await?;
    // audit: CBTRN02C-031
    status_blocked(assert_abended(&refused, &var_dir, Some(OPEN_DALYTRAN), &needles).as_deref());

    let mut malformed = record.clone();
    let amount = malformed
        .get_mut(AMT_OFFSET)
        .ok_or("the scratch record has no amount")?;
    *amount = b'X';
    let input = scratch.write("malformed.txt", &lines_of(&[&record, &malformed]))?;
    let refused = run_posttran(&guard, &var_dir, &input).await?;
    // audit: CBTRN02C-032
    assert_abended(&refused, &var_dir, None, &needles);
    assert_event(
        &refused,
        &[("file", r#""DALYTRAN""#), ("line", "2")],
        &needles,
    );

    let short = record
        .get(..SHORT_BODY_LEN)
        .ok_or("the scratch record is short")?;
    let input = scratch.write("short.txt", &lines_of(&[short]))?;
    let refused = run_posttran(&guard, &var_dir, &input).await?;
    // audit: CBTRN02C-045
    assert_abended(&refused, &var_dir, None, &needles);
    // `DailyTransactionRecord` declares no trailing filler codec A may supply (FIXFRM-011), so
    // codec A itself refuses the body and the event is that of CBTRN02C-032, its rule text
    // giving the body length against the record's; either way the event names the line.
    assert_event(
        &refused,
        &[("file", r#""DALYTRAN""#), ("line", "1")],
        &needles,
    );

    // A file where the reject directory must be created.
    let unwritable = scratch.path().join("unwritable");
    fs::create_dir_all(&unwritable)?;
    fs::write(unwritable.join("dalyrejs"), b"")?;
    let refused = run_posttran(&guard, &unwritable, &fixture_path(DAILYTRAN)).await?;
    // audit: CBTRN02C-031
    status_blocked(assert_abended(&refused, &unwritable, Some(OPEN_DALYREJS), &needles).as_deref());
    assert!(
        Tables::read(&mut store).await? == loaded,
        "a refused run changed a table"
    );

    let empty = run_posttran(&guard, &var_dir, &scratch.write("empty.txt", b"")?).await?;
    // audit: CBTRN02C-029
    assert_eq!(empty.status.code(), Some(0), "{}", needles.report(&empty));
    let rejects = generation(&var_dir, FIRST_GENERATION);
    // audit: CBTRN02C-028
    assert_eq!(written_paths(&empty), [rejects.clone()]);
    assert!(
        fs::read(&rejects)?.is_empty(),
        "the generation of an empty input holds records"
    );
    assert_job_log(&empty, 0, EMPTY_COUNT_LINES);
    assert!(
        Tables::read(&mut store).await? == loaded,
        "an empty input changed a table"
    );
    Ok(())
}

/// One writer at a time: a run that meets the claim on `accounts` another session holds abends
/// at that open with no table changed and no generation kept, and of two runs started together
/// exactly one posts.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_writer_is_refused() -> TestResult {
    let needles = Needles::load()?;
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let loaded = Tables::read(&mut store).await?;
    let scratch = Scratch::new("writers")?;
    let dalytran = fixture_path(DAILYTRAN);

    let held = scratch.path().join("held");
    let mut session = store.session().await?;
    assert!(
        session.claim_writer(Dataset::Accounts).await?,
        "the test could not claim accounts"
    );
    let refused = run_posttran(&guard, &held, &dalytran).await;
    session.close().await?;
    let refused = refused?;
    // audit: CBTRN02C-046
    status_blocked(assert_abended(&refused, &held, Some(OPEN_ACCTFILE), &needles).as_deref());
    assert!(
        Tables::read(&mut store).await? == loaded,
        "the refused run changed a table"
    );

    let (left, right) = (scratch.path().join("left"), scratch.path().join("right"));
    let (one, other) = tokio::try_join!(
        run_posttran(&guard, &left, &dalytran),
        run_posttran(&guard, &right, &dalytran)
    )?;
    let mut runs = [(one, left), (other, right)];
    runs.sort_by_key(|(output, _)| output.status.code() != Some(REJECTS_EXIT_STATUS));
    let [(winner, winner_dir), (loser, loser_dir)] = &runs;
    // audit: CBTRN02C-046
    assert_eq!(
        winner.status.code(),
        Some(REJECTS_EXIT_STATUS),
        "{}",
        needles.report(winner)
    );
    assert_eq!(
        written_paths(winner),
        [generation(winner_dir, FIRST_GENERATION)]
    );
    assert_abended(loser, loser_dir, None, &needles);
    // audit: CBTRN02C-030
    blocked(
        "CBTRN02C-030",
        "X-POSTTRAN-H1",
        "which refusal the second run meets: the first run's claim, or the rows it posted",
    );
    // audit: FIXPOST-901
    assert_eq!(
        TransactionRepository::find_all_in_card_order(&mut store)
            .await?
            .len(),
        POSTED_COUNT,
        "transactions rows after two runs"
    );
    Ok(())
}

/// A record whose `DALYTRAN-ID` an earlier record of the run posted abends at its transaction
/// write with the duplicate-key status, after its category balance and account writes, which
/// stand with the earlier record's effects; the reject generation is removed.
#[tokio::test(flavor = "multi_thread")]
async fn a_duplicate_transaction_id_abends_after_its_balances_are_written() -> TestResult {
    let needles = Needles::load()?;
    let template = first_daily_record()?;
    // Two credits under one ID: the first creates the category balance, the second updates it.
    let (first_cents, second_cents) = (-1_000, -2_000);
    let first = scratch_record(&template, SCRATCH_TYPE, first_cents)?;
    let second = scratch_record(&template, SCRATCH_TYPE, second_cents)?;
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    CardXrefRepository::insert(&mut store, &scratch_xref()?).await?;
    let before = Tables::read(&mut store).await?;
    let scratch = Scratch::new("duplicate")?;
    let input = scratch.write("duplicate.txt", &lines_of(&[&first, &second]))?;
    let output = run_posttran(&guard, scratch.path(), &input).await?;
    let status = assert_abended(&output, scratch.path(), Some(WRITE_TRANFILE), &needles);
    // audit: CBTRN02C-025
    // audit: CBTRN02C-041
    assert_eq!(
        status.as_deref(),
        Some(DUPLICATE_KEY_STATUS),
        "the status line"
    );
    let after = Tables::read(&mut store).await?;
    // audit: CBTRN02C-025
    let [transaction] = after.transactions.as_slice() else {
        panic!(
            "{} transactions, not the first scratch record alone",
            after.transactions.len()
        );
    };
    assert_posted(transaction, &DailyTransactionRecord::decode(&first)?, 1);
    assert_both_postings_stand(&before, &after, Decimal::new(first_cents + second_cents, 2))
}
