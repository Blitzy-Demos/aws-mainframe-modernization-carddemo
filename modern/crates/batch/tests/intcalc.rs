//! INTCALC (`CBACT04C`) through the real `carddemo-batch intcalc` binary: binding g3b for
//! `modern/audit/batch/INTCALC.md` (AAP 0.3.3.2, 0.3.4.3, 0.9.1).
//!
//! Each test starts a `postgres:16` container of its own through `test_support`, migrated by the
//! single schema executor and filled by the nine fixture loads (scenario `LOADED` of
//! `modern/audit/fixtures.md`), and runs the binary as a subprocess from `modern/`, its outputs
//! under a scratch directory of the test's own. `intcalc` rewrites balances, so no database sees
//! two interest runs that both reach the account file.
//!
//! - `intcalc_default_parm_date_matches_audit`: scenario `INTEREST-RAW` with the JCL's PARM.
//! - `intcalc_explicit_parm_date`: a refused PARM, then a PARM longer than 10 characters.
//! - `intcalc_after_posttran_matches_audit`: scenario `INTEREST-POSTED`, `posttran` first.
//! - `intcalc_refuses_a_held_account_claim`: another writer's claim on `accounts` (CBACT04C-039).
//!
//! Every expected value is transcribed from the `CBACT04C-` rows of `INTCALC.md`, the `FIXINT-`
//! rows of `fixtures.md` and the `ARITH-` rows of `arithmetic.md`. Nothing here computes
//! interest: the test calls neither `domain::batch::cbact04c` nor `monthly_interest`, and money is
//! compared as `Decimal` values parsed from the audit's decimal strings (R1).
//!
//! # Compared by format only, or not at all
//!
//! - Timestamps: the binary reads the system clock, so the `FixedClock` text of FIXINT-901
//!   cannot be reproduced from a subprocess. Both SYSTRAN timestamps are checked as layout `D`
//!   with the fraction cut to hundredths, and as the same value in both fields (CBACT04C-022;
//!   AAP 0.5.6).
//! - Bytes 25 to 100 of `TRAN-DESC` (CBACT04C-019) and the record's `FILLER` (CBACT04C-023) are
//!   `HALTED(H2)` through the candidates X-INTCALC-H1 and X-INTCALC-H2 (X-FIXTURES-H4): they are
//!   reported `BLOCKED` and never compared (AAP 0.8.5).
//! - The job log: CBACT04C-003 and X-INTCALC-4 give one tracing event per `DISPLAY` line, and
//!   CBACT04C-038 an `ABENDING PROGRAM` event. The job emits none of them, as its module
//!   documentation states, so no event text is asserted here; standard output is read for its
//!   path lines only.
//!
//! # Cardholder data
//!
//! Every SYSTRAN record carries a full card number (CBACT04C-021). The expected numbers are read
//! in place from `cardxref.txt` into `XrefCard`, which has no `Debug`, and a mismatch names the
//! record and the field only, never a byte of either side (Cardholder Data Redaction).

use std::error::Error;
use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use copybook::{FixedRecord, PicTimestamp, TsForm};
use domain::money::Decimal;
use domain::records::{
    AccountRecord, CardXrefRecord, DisclosureGroupRecord, TranCatBalanceRecord, TransactionRecord,
};
use persistence::{
    AccountRepository, CardXrefRepository, Dataset, DisclosureGroupRepository, PgStore,
    TranCatBalanceRepository, TransactionRepository,
};
use test_support::fixtures::{fixture_dir, fixture_path, start_loaded_postgres};
use test_support::postgres::PostgresGuard;

type TestResult = Result<(), Box<dyn Error>>;

/// `RECLN = 350` of `CVTRA05Y` and `LRECL=350` of the `TRANSACT` DD (`CVTRA05Y.cpy:2`,
/// `INTCALC.jcl:39`; CBACT04C-024).
const SYSTRAN_WIDTH: usize = 350;

/// Records of `SYSTRAN.G0001` in either interest scenario, one per (account, `01`, `0001`)
/// category balance (FIXINT-901).
const SYSTRAN_RECORDS: usize = 50;

/// The first generation a run writes into an empty `systran` directory (FIXINT-901).
const FIRST_SYSTRAN: &str = "SYSTRAN.G0001";

/// `PARM='2022071800'` of `INTCALC.jcl:22`, the default of `--parm-date` (CBACT04C-001).
const JCL_PARM: &[u8; 10] = b"2022071800";

/// The PARM of `intcalc_explicit_parm_date`, of which the first 10 characters are `PARM-DATE`
/// (CBACT04C-001).
const LONG_PARM: &str = "2022080100XYZ";

/// The first 10 characters of [`LONG_PARM`].
const LONG_PARM_DATE: &[u8; 10] = b"2022080100";

/// The shape of both SYSTRAN timestamps: layout `D`, `YYYY-MM-DD-HH.MM.SS.hh0000`, `9` standing
/// for any ASCII digit (CBACT04C-022).
const DB2_HUNDREDTHS: &[u8; 26] = b"9999-99-99-99.99.99.990000";

/// `TRAN-ORIG-TS`, record bytes 279 to 304 (CVTRA05Y-012).
const ORIG_TS: Range<usize> = 278..304;

/// `TRAN-PROC-TS`, record bytes 305 to 330 (CVTRA05Y-013).
const PROC_TS: Range<usize> = 304..330;

/// Zero, as the audit writes it.
const ZERO: &str = "0.00";

/// `posttran`'s exit status over `dailytran.txt`, which rejects records (FIXPOST-903): only a
/// precondition of `INTEREST-POSTED` here.
const POSTTRAN_REJECTED: i32 = 4;

/// The exit status of a job error that sets no return code, such as the abends of `CBACT04C`,
/// whose `9999-ABEND-PROGRAM` sets none (CBACT04C-038; `main.rs`, "Exit status").
const ABEND_STATUS: i32 = 1;

/// The exit status of a usage error, a refused `--parm-date` included (CBACT04C-001).
const USAGE_STATUS: i32 = 2;

/// The standard-error line of a run that meets another writer's claim on `accounts`: the
/// account file's open text of CBACT04C-034 and CBACT04C-039.
const HELD_CLAIM_STDERR: &str =
    "carddemo-batch intcalc: ABEND: ERROR OPENING ACCOUNT MASTER FILE\n";

/// `INTEREST-POSTED`: the 49 totals added to accounts `00000000001` to `00000000049`
/// (FIXINT-903).
const POSTED_TOTALS_ADDED: &str = "1260.39";

/// `INTEREST-POSTED`: the sum of the 50 SYSTRAN amounts, account 50's 18.77 included
/// (FIXINT-903).
const POSTED_SYSTRAN_SUM: &str = "1279.16";

/// `tran_cat_balances` rows after `posttran`: 50 loaded with type `01`, 50 created with type
/// `03` (FIXPOST-901), a precondition of the rate-zero rows.
const POSTED_CATEGORY_BALANCES: usize = 100;

/// The type code of the category balances `posttran` creates, whose `DEFAULT` rate is 0.00
/// (`discgrp.txt:25`; CBACT04C-010, CBACT04C-012).
const ZERO_RATE_TYPE: &[u8; 2] = b"03";

/// One per-account row of `fixtures.md`: `FIXINT-<n>` for account n, `tcatbal.txt` line n and
/// SYSTRAN record n.
struct Fixint {
    /// The `cardxref.txt` line k whose card `{C:k}` the account's SYSTRAN record carries.
    xref_line: usize,
    /// `INTEREST-RAW`: `ACCT-CURR-BAL` before and after the run, whose total is 0.00.
    raw_bal: &'static str,
    /// `INTEREST-POSTED`: the monthly interest of (n, `01`, `0001`), SYSTRAN record n's amount.
    interest: &'static str,
    /// `INTEREST-POSTED`: `ACCT-CURR-BAL`, `ACCT-CURR-CYC-CREDIT` and `ACCT-CURR-CYC-DEBIT` as
    /// `POSTED` leaves them.
    posted: [&'static str; 3],
    /// `INTEREST-POSTED`: `ACCT-CURR-BAL` after the rewrite, or `None` for the last account in
    /// key order, which is never rewritten (E08).
    rewritten_bal: Option<&'static str>,
}

impl Fixint {
    /// The row `text` of [`FIXINT_ROWS`].
    fn parse(text: &'static str) -> Result<Self, String> {
        let columns: Vec<&'static str> = text.split_whitespace().collect();
        let [xref_line, raw_bal, interest, bal, cr, db, rewritten] = columns.as_slice() else {
            return Err(format!("FIXINT row {text:?}: not seven columns"));
        };
        Ok(Self {
            xref_line: xref_line
                .parse()
                .map_err(|_| format!("FIXINT row {text:?}: no cardxref.txt line number"))?,
            raw_bal,
            interest,
            posted: [bal, cr, db],
            rewritten_bal: (*rewritten != NEVER_REWRITTEN).then_some(*rewritten),
        })
    }
}

/// The last column of the row of the account that is never rewritten (E08).
const NEVER_REWRITTEN: &str = "never";

/// FIXINT-001 to FIXINT-050 as `fixtures.md` writes them, row n-1 for account n, its columns
/// separated by spaces: the `cardxref.txt` line k of `{C:k}`; the `INTEREST-RAW` balance; then,
/// for `INTEREST-POSTED`, the interest, the balance, cycle credit and cycle debit `POSTED`
/// leaves, and the balance after the rewrite, or `never` (see [`Fixint`]). The RAW columns are
/// asserted by `intcalc_default_parm_date_matches_audit`, `intcalc_explicit_parm_date` and
/// `intcalc_refuses_a_held_account_claim`, the POSTED columns by
/// `intcalc_after_posttran_matches_audit`.
const FIXINT_ROWS: [&str; SYSTRAN_RECORDS] = [
    // audit: FIXINT-001
    "49 194.00 14.56 1288.10 1164.87 -70.77 1302.66",
    // audit: FIXINT-002
    "3 158.00 29.24 1734.97 2339.97 -763.00 1764.21",
    // audit: FIXINT-003
    "17 147.00 32.38 1857.65 2590.87 -880.22 1890.03",
    // audit: FIXINT-004
    "13 40.00 24.32 1662.66 1945.65 -322.99 1686.98",
    // audit: FIXINT-005
    "27 345.00 15.83 1182.86 1266.85 -428.99 1198.69",
    // audit: FIXINT-006
    "11 218.00 34.38 2572.19 2750.41 -396.22 2606.57",
    // audit: FIXINT-007
    "21 193.00 16.08 900.21 1287.09 -579.88 916.29",
    // audit: FIXINT-008
    "45 605.00 29.60 2138.08 2368.19 -835.11 2167.68",
    // audit: FIXINT-009
    "48 560.00 36.34 3207.55 2907.66 -260.11 3243.89",
    // audit: FIXINT-010
    "14 159.00 19.11 1150.76 1529.20 -537.44 1169.87",
    // audit: FIXINT-011
    "39 212.00 25.90 1539.99 2072.76 -744.77 1565.89",
    // audit: FIXINT-012
    "5 176.00 29.46 1770.65 2357.09 -762.44 1800.11",
    // audit: FIXINT-013
    "18 41.00 35.94 2860.10 2875.87 -56.77 2896.04",
    // audit: FIXINT-014
    "44 15.00 27.14 1475.99 2171.65 -710.66 1503.13",
    // audit: FIXINT-015
    "28 489.00 51.02 4212.54 4081.98 -358.44 4263.56",
    // audit: FIXINT-016
    "32 733.00 16.83 1808.88 1346.87 -270.99 1825.71",
    // audit: FIXINT-017
    "47 33.00 4.29 -621.56 343.77 -998.33 -617.27",
    // audit: FIXINT-018
    "24 144.00 33.27 2692.65 2661.76 -113.11 2725.92",
    // audit: FIXINT-019
    "16 480.00 32.98 3047.53 2639.19 -71.66 3080.51",
    // audit: FIXINT-020
    "4 369.00 30.66 1903.10 2453.10 -919.00 1933.76",
    // audit: FIXINT-021
    "22 112.00 11.10 958.88 888.65 -41.77 969.98",
    // audit: FIXINT-022
    "12 55.00 41.61 2438.20 3328.86 -945.66 2479.81",
    // audit: FIXINT-023
    "42 104.00 23.51 1149.98 1881.42 -835.44 1173.49",
    // audit: FIXINT-024
    "10 400.00 27.04 1922.10 2163.87 -641.77 1949.14",
    // audit: FIXINT-025
    "46 61.00 19.87 743.76 1589.76 -907.00 763.63",
    // audit: FIXINT-026
    "41 46.00 18.99 1193.87 1519.87 -372.00 1212.86",
    // audit: FIXINT-027
    "2 284.00 44.80 3495.65 3584.64 -372.99 3540.45",
    // audit: FIXINT-028
    "31 68.00 10.11 801.78 809.00 -75.22 811.89",
    // audit: FIXINT-029
    "37 339.00 39.63 2657.64 3170.97 -852.33 2697.27",
    // audit: FIXINT-030
    "30 2.00 0.36 -898.89 29.44 -930.33 -898.53",
    // audit: FIXINT-031
    "34 31.00 8.30 333.10 664.32 -362.22 341.40",
    // audit: FIXINT-032
    "36 30.00 14.66 460.56 1173.55 -742.99 475.22",
    // audit: FIXINT-033
    "33 410.00 42.37 3354.43 3389.98 -445.55 3396.80",
    // audit: FIXINT-034
    "19 253.00 32.17 2369.32 2573.87 -457.55 2401.49",
    // audit: FIXINT-035
    "8 166.00 16.74 1005.21 1339.20 -499.99 1021.95",
    // audit: FIXINT-036
    "20 110.00 28.87 1980.43 2309.76 -439.33 2009.30",
    // audit: FIXINT-037
    "7 7.00 0.00 -125.88 0.00 -132.88 -125.88",
    // audit: FIXINT-038
    "40 612.00 37.34 3061.53 2987.75 -538.22 3098.87",
    // audit: FIXINT-039
    "9 843.00 25.63 2822.88 2050.87 -70.99 2848.51",
    // audit: FIXINT-040
    "50 43.00 20.43 1652.10 1635.09 -25.99 1672.53",
    // audit: FIXINT-041
    "15 375.00 29.99 2017.87 2399.53 -756.66 2047.86",
    // audit: FIXINT-042
    "26 302.00 27.05 1965.31 2164.75 -501.44 1992.36",
    // audit: FIXINT-043
    "35 610.00 36.72 3305.19 2938.19 -243.00 3341.91",
    // audit: FIXINT-044
    "6 263.00 25.15 2059.87 2012.20 -215.33 2085.02",
    // audit: FIXINT-045
    "38 186.00 30.09 2248.77 2407.54 -344.77 2278.86",
    // audit: FIXINT-046
    "23 396.00 44.71 3538.41 3577.41 -435.00 3583.12",
    // audit: FIXINT-047
    "25 32.00 19.97 1044.66 1598.10 -585.44 1064.63",
    // audit: FIXINT-048
    "29 226.00 18.90 1202.89 1512.77 -535.88 1221.79",
    // audit: FIXINT-049
    "43 100.00 24.95 1133.31 1996.08 -962.77 1158.26",
    // audit: FIXINT-050
    "1 492.00 18.77 1945.87 1501.75 -47.88 never",
];

/// The rows of [`FIXINT_ROWS`].
fn fixint() -> Result<Vec<Fixint>, String> {
    FIXINT_ROWS.into_iter().map(Fixint::parse).collect()
}

/// Tells apart the scratch directories of the tests of one process.
static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

/// A directory of the test's own under the system temporary directory: the `CARDDEMO_VAR_DIR`
/// of its runs, removed with its content when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Result<Self, Box<dyn Error>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "carddemo-batch-tests-intcalc-{test}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// `systran` under the directory, where `intcalc` writes its generations (CBACT04C-024).
    fn systran_dir(&self) -> PathBuf {
        self.0.join("systran")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Ignored: a directory left behind affects no later test, as each names its own.
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The 16-byte `XREF-CARD-NUM` of one `cardxref.txt` line (CVACT03Y-001), read in place. It has
/// no `Debug` and no `Display`, so no message can show it (Cardholder Data Redaction).
struct XrefCard([u8; 16]);

/// The tables `intcalc` never writes, read whole: `tran_cat_balances`, `card_xref`,
/// `disclosure_groups` and `transactions` (CBACT04C-029). No field is printed: the
/// cross-references and transactions hold card numbers.
#[derive(PartialEq)]
struct Inputs {
    balances: Vec<TranCatBalanceRecord>,
    xrefs: Vec<CardXrefRecord>,
    groups: Vec<DisclosureGroupRecord>,
    transactions: Vec<TransactionRecord>,
}

/// One field of a SYSTRAN record: its copybook name, its record offsets and its expected bytes.
type ByteField = (&'static str, Range<usize>, Vec<u8>);

/// What SYSTRAN record n holds by the audit: CBACT04C-015, CBACT04C-017, CBACT04C-018,
/// CBACT04C-020 and CBACT04C-021, over the fixture records of FIXINT-901.
struct ExpectedRecord<'a> {
    /// The record's place in the file, its account ID and its `TRAN-ID` suffix.
    n: usize,
    /// `PARM-DATE`, the first 10 bytes of `TRAN-ID`.
    parm: &'a [u8; 10],
    /// The monthly interest as the audit writes it.
    amount: &'a str,
    /// `XREF-CARD-NUM` of the account's cross-reference.
    card: &'a XrefCard,
}

impl ExpectedRecord<'_> {
    /// `PARM-DATE`, then the suffix n as six digits with leading zeros (CBACT04C-015).
    fn id(&self) -> Vec<u8> {
        let mut id = self.parm.to_vec();
        id.extend_from_slice(format!("{:06}", self.n).as_bytes());
        id
    }

    /// `Int. for a/c `, then the account ID as 11 digits: bytes 1 to 24 of `TRAN-DESC`
    /// (CBACT04C-018).
    fn desc_head(&self) -> Vec<u8> {
        format!("Int. for a/c {:011}", self.n).into_bytes()
    }

    /// The record's bytes by field, outside the two timestamps, which are compared by format,
    /// and outside the spans of CBACT04C-019 and CBACT04C-023, which `HALTED(H2)` blocks.
    fn byte_fields(&self) -> Result<Vec<ByteField>, String> {
        Ok(vec![
            ("TRAN-ID", 0..16, self.id()),
            ("TRAN-TYPE-CD", 16..18, b"01".to_vec()),
            ("TRAN-CAT-CD", 18..22, b"0005".to_vec()),
            ("TRAN-SOURCE", 22..32, b"System    ".to_vec()),
            ("TRAN-DESC bytes 1-24", 32..56, self.desc_head()),
            ("TRAN-AMT", 132..143, zoned_amount(self.amount)?),
            ("TRAN-MERCHANT-ID", 143..152, b"000000000".to_vec()),
            ("TRAN-MERCHANT-NAME", 152..202, vec![b' '; 50]),
            ("TRAN-MERCHANT-CITY", 202..252, vec![b' '; 50]),
            ("TRAN-MERCHANT-ZIP", 252..262, vec![b' '; 10]),
            ("TRAN-CARD-NUM", 262..278, self.card.0.to_vec()),
        ])
    }
}

/// Runs `carddemo-batch` with `args` against the database of `guard`, from `modern/`, with every
/// output directory under `scratch`, and returns what the process wrote and its status.
///
/// The child inherits the test's environment, of which only the variables the run reads are set
/// here: span export is off, so standard output holds JSON events and path lines only, and
/// `RUST_LOG=error` keeps the events to errors (`main.rs`, "Standard output").
async fn run_batch(
    guard: &PostgresGuard,
    scratch: &Scratch,
    args: &[&str],
) -> Result<Output, Box<dyn Error>> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command
        .args(args)
        .current_dir(test_support::modern_dir())
        .env("DATABASE_URL", guard.database_url())
        .env("CARDDEMO_VAR_DIR", scratch.path())
        .env("CARDDEMO_STATEMENT_DIR", scratch.path().join("statements"))
        .env("CARDDEMO_INTRDR_DIR", scratch.path().join("jobs"))
        .env("CARDDEMO_FIXTURE_DIR", fixture_dir())
        .env("RUST_LOG", "error")
        .env_remove("CARDDEMO_OTEL_EXPORT");
    Ok(tokio::task::spawn_blocking(move || command.output()).await??)
}

/// Fails unless the process of `run` exited with `status`. The message quotes its standard
/// error, which never holds a record or a card number (`main.rs`, "Standard error").
fn expect_status(output: &Output, status: i32, run: &str) -> TestResult {
    if output.status.code() == Some(status) {
        Ok(())
    } else {
        Err(format!(
            "{run}: exit status {:?}, expected {status}; standard error: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        )
        .into())
    }
}

/// `Ok` when `holds`, else the error `what`.
fn ensure(holds: bool, what: &str) -> TestResult {
    if holds { Ok(()) } else { Err(what.into()) }
}

/// The path lines of a run's standard output: every line that does not begin with `{`, the
/// first character of each JSON tracing event (`main.rs`, "Standard output").
fn written_paths(stdout: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter(|line| !line.starts_with('{'))
        .map(PathBuf::from)
        .collect()
}

/// The names of the entries of `dir`, sorted; none when `dir` does not exist.
fn file_names(dir: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut names = entries
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

/// The one path a successful run printed, which must be `SYSTRAN.G0001` under the scratch
/// directory's `systran`, the only file there (CBACT04C-024, CBACT04C-030; FIXINT-901).
fn first_generation(output: &Output, scratch: &Scratch) -> Result<PathBuf, Box<dyn Error>> {
    let generation = scratch.systran_dir().join(FIRST_SYSTRAN);
    ensure(
        written_paths(&output.stdout) == [generation.clone()],
        "intcalc: standard output does not name exactly the first SYSTRAN generation",
    )?;
    ensure(
        file_names(&scratch.systran_dir())? == [FIRST_SYSTRAN],
        "intcalc: the systran directory holds a file other than the first generation",
    )?;
    Ok(generation)
}

/// The `width`-byte records of `bytes`, each of which must be followed by LF (AAP 0.3.4.3;
/// CBACT04C-024).
fn fixed_records(bytes: &[u8], width: usize) -> Result<Vec<&[u8]>, String> {
    let lines = bytes.chunks_exact(width + 1);
    let rest = lines.remainder();
    if !rest.is_empty() {
        return Err(format!(
            "SYSTRAN: {} bytes after the last whole record",
            rest.len()
        ));
    }
    lines
        .enumerate()
        .map(|(index, line)| match line.split_last() {
            Some((b'\n', record)) => Ok(record),
            _ => Err(format!("SYSTRAN record {}: not followed by LF", index + 1)),
        })
        .collect()
}

/// Whether `field` is 26 bytes of layout `D` with the fraction cut to hundredths, the text
/// `Z-GET-DB2-FORMAT-TIMESTAMP` builds (CBACT04C-022).
fn is_db2_timestamp(field: &[u8]) -> bool {
    field.len() == DB2_HUNDREDTHS.len()
        && field.iter().zip(DB2_HUNDREDTHS).all(|(&byte, &shape)| {
            if shape == b'9' {
                byte.is_ascii_digit()
            } else {
                byte == shape
            }
        })
}

/// The 11 bytes of `TRAN-AMT PIC S9(09)V99` for the audit's amount `text`, such as `14.56`: ten
/// digits, then the last digit with the sign overpunched, `{` and `A` to `I` for 0 to 9 when the
/// value is zero or positive, `}` and `J` to `R` when it is negative (PIC-005, PIC-006).
fn zoned_amount(text: &str) -> Result<Vec<u8>, String> {
    let refused = || format!("{text:?} is no S9(09)V99 amount");
    let (negative, magnitude) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (int, cents) = magnitude.split_once('.').ok_or_else(refused)?;
    let digits_only = int
        .bytes()
        .chain(cents.bytes())
        .all(|byte| byte.is_ascii_digit());
    if int.is_empty() || int.len() > 9 || cents.len() != 2 || !digits_only {
        return Err(refused());
    }
    let mut bytes = format!("{int:0>9}{cents}").into_bytes();
    let overpunch: &[u8; 10] = if negative {
        b"}JKLMNOPQR"
    } else {
        b"{ABCDEFGHI"
    };
    let sign = bytes
        .pop()
        .and_then(|last| last.checked_sub(b'0'))
        .and_then(|digit| overpunch.get(usize::from(digit)))
        .copied()
        .ok_or_else(refused)?;
    bytes.push(sign);
    Ok(bytes)
}

/// Whether a decoded SYSTRAN timestamp carries layout `D` (PIC-019; CBACT04C-022).
fn is_db2_form(timestamp: &PicTimestamp) -> bool {
    timestamp
        .value()
        .is_some_and(|value| value.form() == TsForm::Db2)
}

/// The card of every `cardxref.txt` line, entry k-1 for line k (FIXFRM-003: 50 lines, each
/// ending LF, the card number in bytes 1 to 16).
fn xref_cards() -> Result<Vec<XrefCard>, Box<dyn Error>> {
    let bytes = fs::read(fixture_path("cardxref.txt"))?;
    bytes
        .split(|&byte| byte == b'\n')
        .filter(|line| !line.is_empty())
        .enumerate()
        .map(|(index, line)| {
            line.get(..16)
                .and_then(|card| <[u8; 16]>::try_from(card).ok())
                .map(XrefCard)
                .ok_or_else(|| {
                    format!("cardxref.txt line {}: shorter than a card", index + 1).into()
                })
        })
        .collect()
}

/// The byte spans of SYSTRAN record `expected.n` that differ from the audit, by field name only:
/// every byte outside the timestamps and the halted spans, then both timestamps as layout `D`
/// text with hundredths, the same 26 bytes in both fields (CBACT04C-022).
fn byte_failures(bytes: &[u8], expected: &ExpectedRecord<'_>) -> Result<Vec<String>, String> {
    let n = expected.n;
    let mut failures: Vec<String> = expected
        .byte_fields()?
        .into_iter()
        .filter(|(_, at, want)| bytes.get(at.clone()) != Some(want.as_slice()))
        .map(|(field, _, _)| format!("SYSTRAN record {n}: {field} bytes"))
        .collect();
    let orig = bytes.get(ORIG_TS);
    let proc = bytes.get(PROC_TS);
    for (field, text) in [("TRAN-ORIG-TS", orig), ("TRAN-PROC-TS", proc)] {
        if !text.is_some_and(is_db2_timestamp) {
            failures.push(format!(
                "SYSTRAN record {n}: {field} is no layout D hundredths text"
            ));
        }
    }
    if orig != proc {
        failures.push(format!(
            "SYSTRAN record {n}: TRAN-ORIG-TS and TRAN-PROC-TS differ"
        ));
    }
    Ok(failures)
}

/// The fields of decoded SYSTRAN record `expected.n` that differ from the audit, by field name
/// only. `TRAN-AMT` is compared as a `Decimal` parsed from the audit's text (CBACT04C-020).
fn typed_failures(
    record: &TransactionRecord,
    expected: &ExpectedRecord<'_>,
) -> Result<Vec<String>, Box<dyn Error>> {
    let amount: Decimal = expected.amount.parse()?;
    let desc_head = expected.desc_head();
    let checks = [
        (
            "TRAN-ID",
            record.id.as_bytes().as_slice() == expected.id().as_slice(),
        ),
        ("TRAN-TYPE-CD", record.type_cd.as_bytes() == b"01"),
        ("TRAN-CAT-CD", record.cat_cd.get() == 5),
        ("TRAN-SOURCE", record.source.as_bytes() == b"System    "),
        (
            "TRAN-DESC bytes 1-24",
            record.desc.as_bytes().get(..24) == Some(desc_head.as_slice()),
        ),
        ("TRAN-AMT", record.amt.as_decimal() == amount),
        ("TRAN-MERCHANT-ID", record.merchant_id.get() == 0),
        ("TRAN-MERCHANT-NAME", record.merchant_name.is_spaces()),
        ("TRAN-MERCHANT-CITY", record.merchant_city.is_spaces()),
        ("TRAN-MERCHANT-ZIP", record.merchant_zip.is_spaces()),
        (
            "TRAN-CARD-NUM",
            record.card_num.expose().as_bytes() == &expected.card.0,
        ),
        ("TRAN-ORIG-TS form", is_db2_form(&record.orig_ts)),
        ("TRAN-PROC-TS form", is_db2_form(&record.proc_ts)),
        ("timestamps", record.orig_ts == record.proc_ts),
    ];
    Ok(checks
        .into_iter()
        .filter(|&(_, holds)| !holds)
        .map(|(field, _)| format!("SYSTRAN record {}: decoded {field}", expected.n))
        .collect())
}

/// Checks the SYSTRAN generation at `path` record by record: 50 records of 350 bytes, each
/// followed by LF, record n for account n with the `TRAN-ID` of `parm` and suffix n, the card of
/// the account's cross-reference and the amount `amount(row)` of its FIXINT row (FIXINT-901).
/// Returns the decoded records; every difference is reported at once, by record and field.
fn check_systran(
    path: &Path,
    parm: &[u8; 10],
    amount: fn(&Fixint) -> &'static str,
) -> Result<Vec<TransactionRecord>, Box<dyn Error>> {
    let bytes = fs::read(path)?;
    let lines = fixed_records(&bytes, SYSTRAN_WIDTH)?;
    ensure(
        lines.len() == SYSTRAN_RECORDS,
        &format!(
            "SYSTRAN: {} records, expected {SYSTRAN_RECORDS}",
            lines.len()
        ),
    )?;
    let rows = fixint()?;
    let cards = xref_cards()?;
    let mut failures = Vec::new();
    let mut records = Vec::with_capacity(SYSTRAN_RECORDS);
    for (index, (line, row)) in lines.iter().zip(&rows).enumerate() {
        let card = row
            .xref_line
            .checked_sub(1)
            .and_then(|at| cards.get(at))
            .ok_or("FIXINT: a cross-reference line outside cardxref.txt")?;
        let expected = ExpectedRecord {
            n: index + 1,
            parm,
            amount: amount(row),
            card,
        };
        failures.extend(byte_failures(line, &expected)?);
        match TransactionRecord::decode(line) {
            Ok(record) => {
                failures.extend(typed_failures(&record, &expected)?);
                records.push(record);
            }
            Err(error) => failures.push(format!("SYSTRAN record {}: {error}", index + 1)),
        }
    }
    if failures.is_empty() {
        Ok(records)
    } else {
        Err(failures.join("\n").into())
    }
}

/// Reports the two `HALTED(H2)` spans of every SYSTRAN record, which no test compares
/// (AAP 0.8.5).
fn report_blocked_spans() {
    println!("BLOCKED CBACT04C-019 (H2, X-INTCALC-H1): TRAN-DESC bytes 25-100 of each record");
    println!("BLOCKED CBACT04C-023 (H2, X-INTCALC-H2): FILLER of each SYSTRAN record");
}

/// The fields of `account`, account `n`, among `ACCT-CURR-BAL`, `ACCT-CURR-CYC-CREDIT` and
/// `ACCT-CURR-CYC-DEBIT` that differ from `expected`, in that order, each with both values; no
/// account field is cardholder data.
fn balance_failures(
    stage: &str,
    n: usize,
    account: &AccountRecord,
    expected: [&str; 3],
) -> Result<Vec<String>, Box<dyn Error>> {
    let found = [
        account.curr_bal,
        account.curr_cyc_credit,
        account.curr_cyc_debit,
    ];
    let names = [
        "ACCT-CURR-BAL",
        "ACCT-CURR-CYC-CREDIT",
        "ACCT-CURR-CYC-DEBIT",
    ];
    let mut failures = Vec::new();
    for ((name, got), text) in names.into_iter().zip(found).zip(expected) {
        let want: Decimal = text.parse()?;
        if got.as_decimal() != want {
            failures.push(format!(
                "{stage}: account {n}: {name} {} found, {want} expected",
                got.as_decimal()
            ));
        }
    }
    Ok(failures)
}

/// Checks every account after a run, in key order: account n holds the `[bal, cr, db]` that
/// `expected(row)` gives for its FIXINT row, and every other field as `before` held it, as the
/// rewrite of `1050-UPDATE-ACCOUNT` changes those three fields only (CBACT04C-026).
fn check_accounts(
    before: &[AccountRecord],
    after: &[AccountRecord],
    expected: impl Fn(&Fixint) -> [&'static str; 3],
) -> TestResult {
    ensure(
        before.len() == SYSTRAN_RECORDS && after.len() == SYSTRAN_RECORDS,
        "accounts: the table does not hold the 50 fixture accounts",
    )?;
    let rows = fixint()?;
    let mut failures = Vec::new();
    for (index, ((old, new), row)) in before.iter().zip(after).zip(&rows).enumerate() {
        let n = index + 1;
        if new.id.get() != u64::try_from(n)? || old.id != new.id {
            failures.push(format!("accounts: key order differs at account {n}"));
            continue;
        }
        let [bal, cr, db] = expected(row);
        failures.extend(balance_failures("after intcalc", n, new, [bal, cr, db])?);
        let mut kept = new.clone();
        kept.curr_bal = old.curr_bal;
        kept.curr_cyc_credit = old.curr_cyc_credit;
        kept.curr_cyc_debit = old.curr_cyc_debit;
        if kept != *old {
            failures.push(format!(
                "after intcalc: account {n}: a field beside the balance and cycle fields changed"
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n").into())
    }
}

/// E08: the last account in key order, `00000000050`, is never rewritten, so every field keeps
/// the value it had before the run (CBACT04C-027, ARITH-017).
fn check_last_account_kept(before: &[AccountRecord], after: &[AccountRecord]) -> TestResult {
    ensure(
        before.last().is_some() && before.last() == after.last(),
        "E08: the last account in key order was rewritten",
    )
}

/// The tables `intcalc` only reads (CBACT04C-029), read whole through `store`.
async fn read_inputs(store: &mut PgStore) -> Result<Inputs, Box<dyn Error>> {
    Ok(Inputs {
        balances: TranCatBalanceRepository::find_all(store).await?,
        xrefs: CardXrefRepository::find_all(store).await?,
        groups: DisclosureGroupRepository::find_all(store).await?,
        transactions: TransactionRepository::find_all_in_card_order(store).await?,
    })
}

/// CBACT04C-029: no table but `accounts` changes; in particular no SYSTRAN record reaches
/// `transactions`, as only COMBTRAN, which is out of scope, merges them (AAP 0.2.3).
async fn check_inputs_kept(store: &mut PgStore, before: &Inputs) -> TestResult {
    let after = read_inputs(store).await?;
    ensure(
        after.balances == before.balances,
        "CBACT04C-029: tran_cat_balances changed",
    )?;
    ensure(
        after.xrefs == before.xrefs,
        "CBACT04C-029: card_xref changed",
    )?;
    ensure(
        after.groups == before.groups,
        "CBACT04C-029: disclosure_groups changed",
    )?;
    ensure(
        after.transactions == before.transactions,
        "CBACT04C-029: transactions changed",
    )
}

/// `INTEREST-RAW`: the loaded fixtures, then `intcalc` with no option. Every category balance is
/// 0.00, so every interest is 0.00 and every account keeps its loaded balance and cycle fields.
#[tokio::test(flavor = "multi_thread")]
async fn intcalc_default_parm_date_matches_audit() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let scratch = Scratch::new("raw")?;
    let accounts_before = AccountRepository::find_all(&mut store).await?;
    let inputs_before = read_inputs(&mut store).await?;

    // No option: `PARM-DATE` is the JCL's PARM. The opens succeed, the run reaches its end and
    // its return code is 0.
    // audit: CBACT04C-001
    // audit: CBACT04C-002
    // audit: CBACT04C-030
    let output = run_batch(&guard, &scratch, &["intcalc"]).await?;
    expect_status(&output, 0, "intcalc")?;
    // audit: CBACT04C-024
    // audit: FIXINT-901
    let systran = first_generation(&output, &scratch)?;

    // One record per category balance, in key order, each for the account of its break and
    // carrying that account's one card; the amount is the 0.00 interest of ARITH-009.
    // audit: CBACT04C-004
    // audit: CBACT04C-006
    // audit: CBACT04C-013
    // audit: CBACT04C-015
    // audit: CBACT04C-017
    // audit: CBACT04C-018
    // audit: CBACT04C-020
    // audit: CBACT04C-021
    // audit: CBACT04C-022
    // audit: ARITH-009
    // audit: ARITH-012
    // audit: CBACT04C-019
    // audit: CBACT04C-023
    report_blocked_spans();
    check_systran(&systran, JCL_PARM, |_| ZERO)?;

    // Accounts 1 to 49 are rewritten with a total of 0.00 and zeroed cycle fields, which the
    // load left at 0.00; account 50, the last, is never rewritten (E08).
    // audit: CBACT04C-014
    // audit: CBACT04C-026
    // audit: CBACT04C-027
    // audit: ARITH-010
    // audit: ARITH-013
    // audit: ARITH-014
    // audit: ARITH-017
    // audit: FIXINT-903
    let accounts_after = AccountRepository::find_all(&mut store).await?;
    check_accounts(&accounts_before, &accounts_after, |row| {
        [row.raw_bal, ZERO, ZERO]
    })?;
    check_last_account_kept(&accounts_before, &accounts_after)?;

    // audit: CBACT04C-029
    check_inputs_kept(&mut store, &inputs_before).await?;
    ensure(
        TransactionRepository::find_last(&mut store)
            .await?
            .is_none(),
        "CBACT04C-029: a SYSTRAN record reached transactions",
    )
}

/// `--parm-date`: a value of fewer than 10 bytes is refused before any table is read or file
/// created; a longer one contributes its first 10 characters to every `TRAN-ID`, and changes
/// nothing else of the run (CBACT04C-001, CBACT04C-015).
#[tokio::test(flavor = "multi_thread")]
async fn intcalc_explicit_parm_date() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let scratch = Scratch::new("parm")?;
    let accounts_before = AccountRepository::find_all(&mut store).await?;

    // The replacement's typed refusal of a short PARM, which is no parity result: a COBOL PARM
    // of fewer than 10 characters has none (X-INTCALC-H4).
    // audit: CBACT04C-001
    let refused = run_batch(&guard, &scratch, &["intcalc", "--parm-date", "20220801"]).await?;
    expect_status(
        &refused,
        USAGE_STATUS,
        "intcalc --parm-date of 8 characters",
    )?;
    ensure(
        written_paths(&refused.stdout).is_empty(),
        "a refused PARM printed a path",
    )?;
    ensure(
        file_names(scratch.path())?.is_empty(),
        "a refused PARM created a directory or file",
    )?;
    ensure(
        AccountRepository::find_all(&mut store).await? == accounts_before,
        "a refused PARM changed an account",
    )?;

    // A PARM of 13 characters: its first 10 are `PARM-DATE`, followed in each `TRAN-ID` by the
    // record's suffix; the record count and every other byte are those of the JCL's PARM.
    // audit: CBACT04C-001
    // audit: CBACT04C-015
    // audit: FIXINT-901
    let output = run_batch(&guard, &scratch, &["intcalc", "--parm-date", LONG_PARM]).await?;
    expect_status(&output, 0, "intcalc --parm-date of 13 characters")?;
    let systran = first_generation(&output, &scratch)?;
    report_blocked_spans();
    let records = check_systran(&systran, LONG_PARM_DATE, |_| ZERO)?;
    ensure(
        records.len() == SYSTRAN_RECORDS,
        "the PARM changed the number of SYSTRAN records",
    )
}

/// The state `posttran` leaves that `INTEREST-POSTED` starts from, checked before `intcalc`
/// runs: every account's balance and cycle fields as FIXINT-001 to FIXINT-050 give them, every
/// `ACCT-GROUP-ID` blank (FIXFRM-014, E07), and the 100 category balances of FIXPOST-901, 50 of
/// them of type `03`.
fn check_posted_start(accounts: &[AccountRecord], inputs: &Inputs) -> TestResult {
    ensure(
        accounts.len() == SYSTRAN_RECORDS,
        "POSTED: the table does not hold the 50 fixture accounts",
    )?;
    let rows = fixint()?;
    let mut failures = Vec::new();
    for (index, (account, row)) in accounts.iter().zip(&rows).enumerate() {
        failures.extend(balance_failures("POSTED", index + 1, account, row.posted)?);
    }
    ensure(failures.is_empty(), &failures.join("\n"))?;
    ensure(
        accounts.iter().all(|account| account.group_id.is_spaces()),
        "POSTED: an account carries an ACCT-GROUP-ID",
    )?;
    let zero_rate = inputs
        .balances
        .iter()
        .filter(|balance| balance.type_cd.as_bytes() == ZERO_RATE_TYPE)
        .count();
    ensure(
        inputs.balances.len() == POSTED_CATEGORY_BALANCES && zero_rate == SYSTRAN_RECORDS,
        "POSTED: tran_cat_balances does not hold the 100 category balances of FIXPOST-901",
    )
}

/// `INTEREST-POSTED`: `posttran` over `dailytran.txt`, then `intcalc` with no option.
///
/// Each (account, `01`, `0001`) balance earns interest at the `DEFAULT` rate 15.00, as every
/// account's group is blank; each (account, `03`, `0001`) balance meets the `DEFAULT` rate 0.00
/// and writes no record. Accounts 1 to 49 gain their interest and lose their cycle amounts;
/// account 50 keeps every value `posttran` left (E08).
#[tokio::test(flavor = "multi_thread")]
async fn intcalc_after_posttran_matches_audit() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let scratch = Scratch::new("posted")?;
    // `posttran`'s own results are FIXPOST's, asserted by `posttran_i9.rs`; its status 4 is
    // only the precondition that it ran to its end here.
    let posted = run_batch(&guard, &scratch, &["posttran"]).await?;
    expect_status(&posted, POSTTRAN_REJECTED, "posttran")?;
    let accounts_before = AccountRepository::find_all(&mut store).await?;
    let inputs_before = read_inputs(&mut store).await?;
    check_posted_start(&accounts_before, &inputs_before)?;

    let output = run_batch(&guard, &scratch, &["intcalc"]).await?;
    expect_status(&output, 0, "intcalc")?;
    let systran = first_generation(&output, &scratch)?;

    // 100 category balances read, 50 records written: the rate-0.00 balances are skipped, and
    // each amount is its balance times the `DEFAULT` rate 15.00, divided by 1200 and truncated,
    // with no fee added.
    // audit: CBACT04C-010
    // audit: CBACT04C-012
    // audit: CBACT04C-013
    // audit: CBACT04C-020
    // audit: CBACT04C-024
    // audit: CBACT04C-025
    // audit: ARITH-001
    // audit: ARITH-012
    // audit: ARITH-015
    // audit: ARITH-016
    // audit: FIXINT-901
    // audit: FIXINT-902
    // audit: CBACT04C-019
    // audit: CBACT04C-023
    report_blocked_spans();
    let records = check_systran(&systran, JCL_PARM, |row| row.interest)?;
    // audit: FIXINT-903
    let systran_sum: Decimal = records.iter().map(|record| record.amt.as_decimal()).sum();
    ensure(
        systran_sum == POSTED_SYSTRAN_SUM.parse::<Decimal>()?,
        &format!("FIXINT-903: the SYSTRAN amounts sum to {systran_sum}, not {POSTED_SYSTRAN_SUM}"),
    )?;

    // audit: CBACT04C-014
    // audit: CBACT04C-026
    // audit: CBACT04C-027
    // audit: ARITH-010
    // audit: ARITH-013
    // audit: ARITH-014
    // audit: ARITH-017
    // audit: FIXINT-903
    let accounts_after = AccountRepository::find_all(&mut store).await?;
    check_accounts(&accounts_before, &accounts_after, |row| {
        row.rewritten_bal
            .map_or(row.posted, |bal| [bal, ZERO, ZERO])
    })?;
    check_last_account_kept(&accounts_before, &accounts_after)?;
    let totals_added: Decimal = accounts_before
        .iter()
        .zip(&accounts_after)
        .map(|(old, new)| new.curr_bal.as_decimal() - old.curr_bal.as_decimal())
        .sum();
    ensure(
        totals_added == POSTED_TOTALS_ADDED.parse::<Decimal>()?,
        &format!("FIXINT-903: the balances gain {totals_added}, not {POSTED_TOTALS_ADDED}"),
    )?;

    // audit: CBACT04C-029
    check_inputs_kept(&mut store, &inputs_before).await
}

/// `0300-ACCTFILE-OPEN` against another writer: while the test's own session holds the claim on
/// `accounts`, as a running `posttran` or `intcalc` does, `intcalc` abends at the account file's
/// open, before it reads a table or creates a generation. Once the claim is given up, the next
/// run is the only writer and takes the generation the refused run never allocated.
#[tokio::test(flavor = "multi_thread")]
async fn intcalc_refuses_a_held_account_claim() -> TestResult {
    let guard = start_loaded_postgres().await?;
    let mut store = PgStore::new(guard.pool().clone());
    let scratch = Scratch::new("claim")?;
    let accounts_before = AccountRepository::find_all(&mut store).await?;
    let mut holder = store.session().await?;
    ensure(
        holder.claim_writer(Dataset::Accounts).await?,
        "the test's own claim on accounts was refused",
    )?;

    // audit: CBACT04C-039
    let refused = run_batch(&guard, &scratch, &["intcalc"]).await?;
    expect_status(&refused, ABEND_STATUS, "intcalc against a held claim")?;
    ensure(
        String::from_utf8_lossy(&refused.stderr) == HELD_CLAIM_STDERR,
        "intcalc against a held claim: standard error is not the account file's open text",
    )?;
    ensure(
        written_paths(&refused.stdout).is_empty(),
        "intcalc against a held claim printed a path",
    )?;
    ensure(
        file_names(&scratch.systran_dir())?.is_empty(),
        "intcalc against a held claim created a SYSTRAN generation",
    )?;
    ensure(
        AccountRepository::find_all(&mut store).await? == accounts_before,
        "intcalc against a held claim changed an account",
    )?;
    // The status characters VSAM would show after the open's text are no parity result.
    // audit: CBACT04C-037
    println!("BLOCKED CBACT04C-037 (H4, X-INTCALC-H3): the status of the refused open");

    holder.close().await?;
    let output = run_batch(&guard, &scratch, &["intcalc"]).await?;
    expect_status(&output, 0, "intcalc after the claim was given up")?;
    let systran = first_generation(&output, &scratch)?;
    report_blocked_spans();
    check_systran(&systran, JCL_PARM, |_| ZERO)?;
    Ok(())
}
