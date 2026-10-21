//! Binding i2 (AAP 0.9.1, 0.9.2 o3): interest, bill payment and category-balance results equal
//! the COBOL exactly, to the cent.
//!
//! - Expected values are transcribed from the derive-lane audits: the worked cases of
//!   `modern/audit/arithmetic.md` (`ARITH-001` to `ARITH-033`), the INTCALC rows that cite them
//!   (`CBACT04C-013`, `CBACT04C-014`, `CBACT04C-020` and `CBACT04C-026` of
//!   `modern/audit/batch/INTCALC.md`) and the per-record rows of
//!   `modern/audit/fixtures.md` (`FIXPOST-001` to `FIXPOST-300` and `FIXINT-001` to `FIXINT-050`,
//!   with their summaries 901 to 903). Each row carries its `// audit:` tag where it is asserted.
//! - Actual values come only from `domain::money` and the rule modules `domain::batch::cbact04c`
//!   (INTCALC), `domain::batch::cbtrn02c` (POSTTRAN) and `domain::online::cobil00c` (bill
//!   payment). Amounts compare as `Money` values, exactly: no tolerance and no floating point.
//!   The one text comparison is the edited balance of `ARITH-019`, whose expected result is text.
//! - The scenarios of `fixtures.md` run in memory, with no database: the masters are decoded in
//!   place from `app/data/ASCII/` by codec A, and the effects each rule returns are applied to
//!   in-memory tables in the order the rule returns them, as the batch jobs apply them.
//!   `POSTED` runs `Posttran` over the 300 `dailytran.txt` records, `INTEREST-RAW` runs
//!   `Intcalc` over the loaded category balances, and `INTEREST-POSTED` runs it over the tables
//!   `POSTED` leaves.
//! - Audited defects are asserted as the COBOL behaves, never corrected (Behaviour constraint):
//!   E07 (every rate lookup takes the `DEFAULT` group), E08 (the last account is never
//!   rewritten), E10 (`WS-TEMP-BAL` drops its high-order digit) and E11 (`TRAN-AMT` drops the
//!   balance's high-order digit).
//! - Fixture records hold card numbers, so no assertion prints a record: a failure names the
//!   rule ID and the fixture line (Cardholder Data Redaction). The timestamps the rules write
//!   from a `FixedClock` are checked by layout only (Typed Dates and Timestamps).

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::path::Path;

use chrono::{NaiveDate, NaiveDateTime};
use copybook::{FixedRecord, PicTimestamp, PicU, PicX, TsForm};
use domain::batch::cbact04c::{self, Intcalc, IntcalcEffect, IntcalcSummary, ParmDate};
use domain::batch::cbtrn02c::{
    self, Posting, Posttran, PosttranSummary, RecordOutcome, RejectReason, TcatbalWrite, Validation,
};
use domain::batch::loads::decode_fixture;
use domain::clock::FixedClock;
use domain::money::{Decimal, Money, MoneyError, Rate, monthly_interest};
use domain::online::cobil00c::{self, BillPayDecision, BillPayField, FailureClass, ReadIntent};
use domain::records::{
    AccountRecord, CardXrefRecord, DailyTransactionRecord, DisclosureGroupRecord,
    TranCatBalanceRecord, TransactionRecord,
};

type TestResult = Result<(), Box<dyn Error>>;

/// A category balance key as plain values: account ID, `TRANCAT-TYPE-CD` and `TRANCAT-CD`.
type CategoryKey = (u64, [u8; 2], u64);

/// `DIS-INT-RATE` of `DEFAULT` `01`/`0001` (`discgrp.txt:18`), in hundredths: the rate every
/// purchase category balance of the corpus takes (FIXFRM-006, FIXINT-902, E07).
const DEFAULT_PURCHASE_RATE: i64 = 1_500;

/// Records `posttran` reads in `POSTED` (FIXPOST-903).
const PROCESSED: u64 = 300;

/// Records `posttran` posts in `POSTED` (FIXPOST-901).
const POSTED: u64 = 262;

/// Records `posttran` rejects in `POSTED` (FIXPOST-902).
const REJECTED: u64 = 38;

/// The `posttran` return code of a run with rejects (FIXPOST-903).
const POSTTRAN_RETURN_CODE: u8 = 4;

/// Rows of `tran_cat_balances` after `POSTED`: the 50 loaded rows, of which 49 are updated, and
/// 50 created rows (FIXPOST-901).
const TCATBAL_ROWS_AFTER_POSTED: usize = 100;
const LOADED_ROWS_UPDATED: usize = 49;
const ROWS_CREATED: usize = 50;

/// The account whose every purchase is rejected, so its loaded `(37, 01, 0001)` row is never
/// updated (FIXPOST-901).
const ACCOUNT_WITHOUT_POSTED_PURCHASE: u64 = 37;

/// The last account in key order, which INTCALC never rewrites (E08, ARITH-017, FIXINT-903).
const LAST_ACCOUNT: u64 = 50;

/// `INTEREST-POSTED`: the totals the 49 rewritten accounts gain, and the sum of the 50 SYSTRAN
/// amounts, account 50's included (FIXINT-903).
const POSTED_REWRITTEN_TOTALS: &str = "1260.39";
const POSTED_SYSTRAN_AMOUNTS: &str = "1279.16";

/// How `posttran` writes the category balance of a posted record (CBTRN02C-016 to
/// CBTRN02C-019).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TcatbalKind {
    /// `2700-A-CREATE-TCATBAL-REC`: no row had the key.
    Create,
    /// `2700-B-UPDATE-TCATBAL-REC`: the row was found and rewritten.
    Update,
}

/// The tables a posted record leaves: its category balance and its account after the posting.
struct PostedResult {
    kind: TcatbalKind,
    type_cd: [u8; 2],
    cat_bal: Money<9>,
    bal: Money<10>,
    cr: Money<10>,
    db: Money<10>,
}

/// The outcome of one `FIXPOST-<line>` row.
enum PostExpected {
    Posted(PostedResult),
    /// Rejected, with the reason code of the reject trailer.
    Rejected(u64),
}

/// One `FIXPOST-<line>` row: the `dailytran.txt` line, the account its card cross-references,
/// `DALYTRAN-AMT`, `WS-TEMP-BAL` as stored, and the outcome.
struct PostRow {
    line: usize,
    acct_id: u64,
    amt: Money<9>,
    temp: Money<9>,
    expected: PostExpected,
}

/// The `INTEREST-RAW` result of one `FIXINT-<n>` row: account n's loaded `(n, 01, 0001)`
/// balance, its monthly interest, and the account balance before the run and after it (`None`
/// for the account the run never rewrites).
struct RawInterestRow {
    acct_id: u64,
    cat_bal: Money<9>,
    monthly: Money<9>,
    bal_before: Money<10>,
    bal_after: Option<Money<10>>,
}

/// The `INTEREST-POSTED` result of one `FIXINT-<n>` row: the `(n, 01, 0001)` and
/// `(n, 03, 0001)` balances `POSTED` leaves, the monthly interest of the first (the second has
/// rate 0.00), account n as `POSTED` leaves it, and its balance after the run (`None` for the
/// account the run never rewrites).
struct PostedInterestRow {
    acct_id: u64,
    purchase_bal: Money<9>,
    credit_bal: Money<9>,
    monthly: Money<9>,
    bal_before: Money<10>,
    cr_before: Money<10>,
    db_before: Money<10>,
    bal_after: Option<Money<10>>,
}

/// `text`, a decimal with two places as the audits write it, as an amount of picture
/// `S9(N)V99`. The parse is exact, and a value the picture cannot hold is an error.
fn amount<const N: u32>(text: &str) -> Result<Money<N>, Box<dyn Error>> {
    Ok(Money::new(Decimal::from_str_exact(text)?)?)
}

/// `cents` hundredths as an amount of picture `S9(N)V99`.
fn money<const N: u32>(cents: i64) -> Result<Money<N>, MoneyError> {
    Money::new(Decimal::new(cents, 2))
}

/// `hundredths` as a rate of picture `S9(04)V99`.
fn rate(hundredths: i64) -> Result<Rate<4>, MoneyError> {
    Rate::new(Decimal::new(hundredths, 2))
}

/// An account balance after a run: the amount, or `None` where the audit says `never`.
fn rewritten(text: &str) -> Result<Option<Money<10>>, Box<dyn Error>> {
    if text == "never" {
        Ok(None)
    } else {
        Ok(Some(amount(text)?))
    }
}

/// One line of [`POSTTRAN_ROWS`]:
/// `<line> <account> <amt> <temp> posted <create|update> <type> <cat_bal> <bal> <cr> <db>` or
/// `<line> <account> <amt> <temp> rejected <reason>`.
fn parse_post_row(text: &str) -> Result<PostRow, Box<dyn Error>> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let [line, acct_id, amt, temp, outcome @ ..] = fields.as_slice() else {
        return Err(format!("malformed FIXPOST row `{text}`").into());
    };
    let expected = match outcome {
        ["posted", kind, type_cd, cat_bal, bal, cr, db] => {
            let kind = match *kind {
                "create" => TcatbalKind::Create,
                "update" => TcatbalKind::Update,
                other => return Err(format!("unknown category-balance write `{other}`").into()),
            };
            PostExpected::Posted(PostedResult {
                kind,
                type_cd: type_cd.as_bytes().try_into()?,
                cat_bal: amount(cat_bal)?,
                bal: amount(bal)?,
                cr: amount(cr)?,
                db: amount(db)?,
            })
        }
        ["rejected", reason] => PostExpected::Rejected(reason.parse()?),
        _ => return Err(format!("malformed FIXPOST outcome in `{text}`").into()),
    };
    Ok(PostRow {
        line: line.parse()?,
        acct_id: acct_id.parse()?,
        amt: amount(amt)?,
        temp: amount(temp)?,
        expected,
    })
}

/// One line of [`INTEREST_RAW_ROWS`]: `<account> <cat_bal> <monthly> <bal_before> <bal_after>`.
fn parse_raw_interest_row(text: &str) -> Result<RawInterestRow, Box<dyn Error>> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let [acct_id, cat_bal, monthly, bal_before, bal_after] = fields.as_slice() else {
        return Err(format!("malformed FIXINT INTEREST-RAW row `{text}`").into());
    };
    Ok(RawInterestRow {
        acct_id: acct_id.parse()?,
        cat_bal: amount(cat_bal)?,
        monthly: amount(monthly)?,
        bal_before: amount(bal_before)?,
        bal_after: rewritten(bal_after)?,
    })
}

/// One line of [`INTEREST_POSTED_ROWS`]: `<account> <purchase_bal> <credit_bal> <monthly>
/// <bal_before> <cr_before> <db_before> <bal_after>`.
fn parse_posted_interest_row(text: &str) -> Result<PostedInterestRow, Box<dyn Error>> {
    let fields: Vec<&str> = text.split_whitespace().collect();
    let [acct_id, purchase, credit, monthly, bal, cr, db, bal_after] = fields.as_slice() else {
        return Err(format!("malformed FIXINT INTEREST-POSTED row `{text}`").into());
    };
    Ok(PostedInterestRow {
        acct_id: acct_id.parse()?,
        purchase_bal: amount(purchase)?,
        credit_bal: amount(credit)?,
        monthly: amount(monthly)?,
        bal_before: amount(bal)?,
        cr_before: amount(cr)?,
        db_before: amount(db)?,
        bal_after: rewritten(bal_after)?,
    })
}

/// Every record of fixture `name` in `app/data/ASCII/`, read in place and decoded by codec A,
/// in file order.
fn fixture<R: FixedRecord>(name: &str) -> Result<Vec<R>, Box<dyn Error>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../app/data/ASCII")
        .join(name);
    let bytes = fs::read(&path)?;
    Ok(decode_fixture::<R>(&bytes)?
        .into_iter()
        .map(|framed| framed.record)
        .collect())
}

/// `year-month-day hour:00:00`, a `FixedClock` instant of the scenarios of `fixtures.md`.
fn instant(year: i32, month: u32, day: u32, hour: u32) -> Result<NaiveDateTime, Box<dyn Error>> {
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|date| date.and_hms_opt(hour, 0, 0))
        .ok_or_else(|| format!("{year}-{month}-{day} {hour}:00 is not an instant").into())
}

/// The layout code of a written timestamp, `None` when the field is blank.
fn form(stamp: &PicTimestamp) -> Option<TsForm> {
    stamp.value().map(|value| value.form())
}

/// The key of `record` as plain values.
fn category_key(record: &TranCatBalanceRecord) -> CategoryKey {
    (
        record.acct_id.get(),
        *record.type_cd.as_bytes(),
        record.cd.get(),
    )
}

/// The `FIXPOST-001` to `FIXPOST-300` rows of `fixtures.md`, one per `dailytran.txt` line in
/// input order, as [`parse_post_row`] reads them. Amounts keep the audit's decimal text, so
/// each row reads against the audit table, and one row stays one line under rustfmt.
const POSTTRAN_ROWS: [&str; 300] = [
    // audit: FIXPOST-001
    "1 7 504.77 504.77 posted update 01 504.77 697.77 504.77 0.00",
    // audit: FIXPOST-002
    "2 20 -919.00 -919.00 posted create 03 -919.00 -550.00 0.00 -919.00",
    // audit: FIXPOST-003
    "3 5 67.88 67.88 posted update 01 67.88 412.88 67.88 0.00",
    // audit: FIXPOST-004
    "4 26 281.77 281.77 posted update 01 281.77 327.77 281.77 0.00",
    // audit: FIXPOST-005
    "5 46 454.66 454.66 posted update 01 454.66 850.66 454.66 0.00",
    // audit: FIXPOST-006
    "6 45 849.99 849.99 posted update 01 849.99 1035.99 849.99 0.00",
    // audit: FIXPOST-007
    "7 13 -56.77 -56.77 posted create 03 -56.77 -15.77 0.00 -56.77",
    // audit: FIXPOST-008
    "8 26 373.66 655.43 posted update 01 655.43 701.43 655.43 0.00",
    // audit: FIXPOST-009
    "9 48 -535.88 -535.88 posted create 03 -535.88 -309.88 0.00 -535.88",
    // audit: FIXPOST-010
    "10 9 416.11 416.11 posted update 01 416.11 976.11 416.11 0.00",
    // audit: FIXPOST-011
    "11 10 94.33 94.33 posted update 01 94.33 253.33 94.33 0.00",
    // audit: FIXPOST-012
    "12 32 250.22 250.22 posted update 01 250.22 280.22 250.22 0.00",
    // audit: FIXPOST-013
    "13 41 829.55 829.55 posted update 01 829.55 1204.55 829.55 0.00",
    // audit: FIXPOST-014
    "14 30 29.44 29.44 posted update 01 29.44 31.44 29.44 0.00",
    // audit: FIXPOST-015
    "15 41 958.99 1788.54 posted update 01 1788.54 2163.54 1788.54 0.00",
    // audit: FIXPOST-016
    "16 37 715.44 715.44 rejected 0102",
    // audit: FIXPOST-017
    "17 22 -945.66 -945.66 posted create 03 -945.66 -890.66 0.00 -945.66",
    // audit: FIXPOST-018
    "18 32 649.33 899.55 posted update 01 899.55 929.55 899.55 0.00",
    // audit: FIXPOST-019
    "19 36 502.66 502.66 posted update 01 502.66 612.66 502.66 0.00",
    // audit: FIXPOST-020
    "20 44 303.11 303.11 posted update 01 303.11 566.11 303.11 0.00",
    // audit: FIXPOST-021
    "21 50 183.88 183.88 posted update 01 183.88 675.88 183.88 0.00",
    // audit: FIXPOST-022
    "22 47 779.33 779.33 posted update 01 779.33 811.33 779.33 0.00",
    // audit: FIXPOST-023
    "23 39 -70.99 -70.99 posted create 03 -70.99 772.01 0.00 -70.99",
    // audit: FIXPOST-024
    "24 2 355.11 355.11 posted update 01 355.11 513.11 355.11 0.00",
    // audit: FIXPOST-025
    "25 15 660.11 660.11 posted update 01 660.11 1149.11 660.11 0.00",
    // audit: FIXPOST-026
    "26 8 325.00 325.00 posted update 01 325.00 930.00 325.00 0.00",
    // audit: FIXPOST-027
    "27 3 999.77 999.77 posted update 01 999.77 1146.77 999.77 0.00",
    // audit: FIXPOST-028
    "28 8 482.44 807.44 posted update 01 807.44 1412.44 807.44 0.00",
    // audit: FIXPOST-029
    "29 20 555.22 1474.22 posted update 01 555.22 5.22 555.22 -919.00",
    // audit: FIXPOST-030
    "30 40 356.22 356.22 posted update 01 356.22 399.22 356.22 0.00",
    // audit: FIXPOST-031
    "31 32 274.00 1173.55 posted update 01 1173.55 1203.55 1173.55 0.00",
    // audit: FIXPOST-032
    "32 17 633.00 633.00 rejected 0102",
    // audit: FIXPOST-033
    "33 45 840.55 1690.54 posted update 01 1690.54 1876.54 1690.54 0.00",
    // audit: FIXPOST-034
    "34 33 769.55 769.55 posted update 01 769.55 1179.55 769.55 0.00",
    // audit: FIXPOST-035
    "35 28 948.44 948.44 rejected 0102",
    // audit: FIXPOST-036
    "36 22 401.22 1346.88 posted update 01 401.22 -489.44 401.22 -945.66",
    // audit: FIXPOST-037
    "37 27 777.33 777.33 posted update 01 777.33 1061.33 777.33 0.00",
    // audit: FIXPOST-038
    "38 44 -215.33 87.78 posted create 03 -215.33 350.78 303.11 -215.33",
    // audit: FIXPOST-039
    "39 18 46.66 46.66 posted update 01 46.66 190.66 46.66 0.00",
    // audit: FIXPOST-040
    "40 29 570.66 570.66 posted update 01 570.66 909.66 570.66 0.00",
    // audit: FIXPOST-041
    "41 6 843.66 843.66 posted update 01 843.66 1061.66 843.66 0.00",
    // audit: FIXPOST-042
    "42 49 559.88 559.88 posted update 01 559.88 659.88 559.88 0.00",
    // audit: FIXPOST-043
    "43 30 203.44 232.88 rejected 0102",
    // audit: FIXPOST-044
    "44 39 168.33 239.32 posted update 01 168.33 940.34 168.33 -70.99",
    // audit: FIXPOST-045
    "45 26 864.44 1519.87 posted update 01 1519.87 1565.87 1519.87 0.00",
    // audit: FIXPOST-046
    "46 25 183.22 183.22 posted update 01 183.22 244.22 183.22 0.00",
    // audit: FIXPOST-047
    "47 28 256.00 256.00 posted update 01 256.00 324.00 256.00 0.00",
    // audit: FIXPOST-048
    "48 14 985.33 985.33 posted update 01 985.33 1000.33 985.33 0.00",
    // audit: FIXPOST-049
    "49 16 19.99 19.99 posted update 01 19.99 752.99 19.99 0.00",
    // audit: FIXPOST-050
    "50 26 907.55 2427.42 rejected 0102",
    // audit: FIXPOST-051
    "51 29 989.77 1560.43 posted update 01 1560.43 1899.43 1560.43 0.00",
    // audit: FIXPOST-052
    "52 27 579.77 1357.10 posted update 01 1357.10 1641.10 1357.10 0.00",
    // audit: FIXPOST-053
    "53 15 -358.44 301.67 posted create 03 -358.44 790.67 660.11 -358.44",
    // audit: FIXPOST-054
    "54 39 848.33 1087.65 posted update 01 1016.66 1788.67 1016.66 -70.99",
    // audit: FIXPOST-055
    "55 43 -243.00 -243.00 posted create 03 -243.00 367.00 0.00 -243.00",
    // audit: FIXPOST-056
    "56 16 59.33 79.32 posted update 01 79.32 812.32 79.32 0.00",
    // audit: FIXPOST-057
    "57 11 -744.77 -744.77 posted create 03 -744.77 -532.77 0.00 -744.77",
    // audit: FIXPOST-058
    "58 39 385.77 1473.42 posted update 01 1402.43 2174.44 1402.43 -70.99",
    // audit: FIXPOST-059
    "59 13 326.44 383.21 posted update 01 326.44 310.67 326.44 -56.77",
    // audit: FIXPOST-060
    "60 34 299.33 299.33 posted update 01 299.33 552.33 299.33 0.00",
    // audit: FIXPOST-061
    "61 6 175.88 1019.54 posted update 01 1019.54 1237.54 1019.54 0.00",
    // audit: FIXPOST-062
    "62 47 479.22 1258.55 posted update 01 1258.55 1290.55 1258.55 0.00",
    // audit: FIXPOST-063
    "63 41 76.33 1864.87 posted update 01 1864.87 2239.87 1864.87 0.00",
    // audit: FIXPOST-064
    "64 28 553.00 809.00 posted update 01 809.00 877.00 809.00 0.00",
    // audit: FIXPOST-065
    "65 8 -835.11 -27.67 posted create 03 -835.11 577.33 807.44 -835.11",
    // audit: FIXPOST-066
    "66 30 929.77 959.21 rejected 0102",
    // audit: FIXPOST-067
    "67 31 495.66 495.66 posted update 01 495.66 526.66 495.66 0.00",
    // audit: FIXPOST-068
    "68 9 851.33 1267.44 posted update 01 1267.44 1827.44 1267.44 0.00",
    // audit: FIXPOST-069
    "69 17 343.77 343.77 posted update 01 343.77 376.77 343.77 0.00",
    // audit: FIXPOST-070
    "70 5 -428.99 -361.11 posted create 03 -428.99 -16.11 67.88 -428.99",
    // audit: FIXPOST-071
    "71 11 160.99 905.76 posted update 01 160.99 -371.78 160.99 -744.77",
    // audit: FIXPOST-072
    "72 30 -930.33 -900.89 posted create 03 -930.33 -898.89 29.44 -930.33",
    // audit: FIXPOST-073
    "73 33 -445.55 324.00 posted create 03 -445.55 734.00 769.55 -445.55",
    // audit: FIXPOST-074
    "74 38 816.77 816.77 posted update 01 816.77 1428.77 816.77 0.00",
    // audit: FIXPOST-075
    "75 22 848.77 2195.65 posted update 01 1249.99 359.33 1249.99 -945.66",
    // audit: FIXPOST-076
    "76 47 339.55 1598.10 posted update 01 1598.10 1630.10 1598.10 0.00",
    // audit: FIXPOST-077
    "77 42 905.00 905.00 posted update 01 905.00 1207.00 905.00 0.00",
    // audit: FIXPOST-078
    "78 5 346.99 843.86 posted update 01 414.87 330.88 414.87 -428.99",
    // audit: FIXPOST-079
    "79 28 944.99 1753.99 rejected 0102",
    // audit: FIXPOST-080
    "80 38 257.55 1074.32 posted update 01 1074.32 1686.32 1074.32 0.00",
    // audit: FIXPOST-081
    "81 36 496.33 998.99 posted update 01 998.99 1108.99 998.99 0.00",
    // audit: FIXPOST-082
    "82 48 526.11 1061.99 posted update 01 526.11 216.23 526.11 -535.88",
    // audit: FIXPOST-083
    "83 12 210.33 210.33 posted update 01 210.33 386.33 210.33 0.00",
    // audit: FIXPOST-084
    "84 38 461.99 1536.31 posted update 01 1536.31 2148.31 1536.31 0.00",
    // audit: FIXPOST-085
    "85 10 457.55 551.88 posted update 01 551.88 710.88 551.88 0.00",
    // audit: FIXPOST-086
    "86 48 764.22 1826.21 posted update 01 1290.33 980.45 1290.33 -535.88",
    // audit: FIXPOST-087
    "87 2 -763.00 -407.89 posted create 03 -763.00 -249.89 355.11 -763.00",
    // audit: FIXPOST-088
    "88 21 40.00 40.00 posted update 01 40.00 152.00 40.00 0.00",
    // audit: FIXPOST-089
    "89 32 996.88 2170.43 rejected 0102",
    // audit: FIXPOST-090
    "90 42 89.99 994.99 posted update 01 994.99 1296.99 994.99 0.00",
    // audit: FIXPOST-091
    "91 49 49.55 609.43 posted update 01 609.43 709.43 609.43 0.00",
    // audit: FIXPOST-092
    "92 38 706.11 2242.42 posted update 01 2242.42 2854.42 2242.42 0.00",
    // audit: FIXPOST-093
    "93 43 936.22 1179.22 posted update 01 936.22 1303.22 936.22 -243.00",
    // audit: FIXPOST-094
    "94 47 -585.44 1012.66 posted create 03 -585.44 1044.66 1598.10 -585.44",
    // audit: FIXPOST-095
    "95 45 717.00 2407.54 posted update 01 2407.54 2593.54 2407.54 0.00",
    // audit: FIXPOST-096
    "96 38 745.33 2987.75 posted update 01 2987.75 3599.75 2987.75 0.00",
    // audit: FIXPOST-097
    "97 8 728.77 2371.32 posted update 01 1536.21 1306.10 1536.21 -835.11",
    // audit: FIXPOST-098
    "98 23 948.22 948.22 posted update 01 948.22 1052.22 948.22 0.00",
    // audit: FIXPOST-099
    "99 2 569.99 1688.10 posted update 01 925.10 320.10 925.10 -763.00",
    // audit: FIXPOST-100
    "100 25 859.44 1042.66 posted update 01 1042.66 1103.66 1042.66 0.00",
    // audit: FIXPOST-101
    "101 26 667.55 2187.42 rejected 0102",
    // audit: FIXPOST-102
    "102 50 14.00 197.88 posted update 01 197.88 689.88 197.88 0.00",
    // audit: FIXPOST-103
    "103 32 -742.99 430.56 posted create 03 -742.99 460.56 1173.55 -742.99",
    // audit: FIXPOST-104
    "104 14 990.88 1976.21 posted update 01 1976.21 1991.21 1976.21 0.00",
    // audit: FIXPOST-105
    "105 24 997.88 997.88 posted update 01 997.88 1397.88 997.88 0.00",
    // audit: FIXPOST-106
    "106 29 -852.33 708.10 posted create 03 -852.33 1047.10 1560.43 -852.33",
    // audit: FIXPOST-107
    "107 42 552.77 1547.76 posted update 01 1547.76 1849.76 1547.76 0.00",
    // audit: FIXPOST-108
    "108 11 446.00 1351.76 posted update 01 606.99 74.22 606.99 -744.77",
    // audit: FIXPOST-109
    "109 20 965.55 2439.77 posted update 01 1520.77 970.77 1520.77 -919.00",
    // audit: FIXPOST-110
    "110 18 958.33 1004.99 posted update 01 1004.99 1148.99 1004.99 0.00",
    // audit: FIXPOST-111
    "111 3 115.11 1114.88 posted update 01 1114.88 1261.88 1114.88 0.00",
    // audit: FIXPOST-112
    "112 45 -344.77 2062.77 posted create 03 -344.77 2248.77 2407.54 -344.77",
    // audit: FIXPOST-113
    "113 28 -75.22 733.78 posted create 03 -75.22 801.78 809.00 -75.22",
    // audit: FIXPOST-114
    "114 7 428.33 933.10 posted update 01 933.10 1126.10 933.10 0.00",
    // audit: FIXPOST-115
    "115 46 884.99 1339.65 posted update 01 1339.65 1735.65 1339.65 0.00",
    // audit: FIXPOST-116
    "116 18 860.77 1865.76 posted update 01 1865.76 2009.76 1865.76 0.00",
    // audit: FIXPOST-117
    "117 34 351.66 650.99 posted update 01 650.99 903.99 650.99 0.00",
    // audit: FIXPOST-118
    "118 6 -396.22 623.32 posted create 03 -396.22 841.32 1019.54 -396.22",
    // audit: FIXPOST-119
    "119 5 254.77 1098.63 posted update 01 669.64 585.65 669.64 -428.99",
    // audit: FIXPOST-120
    "120 44 385.44 903.88 posted update 01 688.55 736.22 688.55 -215.33",
    // audit: FIXPOST-121
    "121 24 617.00 1614.88 posted update 01 1614.88 2014.88 1614.88 0.00",
    // audit: FIXPOST-122
    "122 41 -756.66 1108.21 posted create 03 -756.66 1483.21 1864.87 -756.66",
    // audit: FIXPOST-123
    "123 19 949.77 949.77 posted update 01 949.77 1429.77 949.77 0.00",
    // audit: FIXPOST-124
    "124 42 -501.44 1046.32 posted create 03 -501.44 1348.32 1547.76 -501.44",
    // audit: FIXPOST-125
    "125 42 214.77 2263.97 posted update 01 1762.53 1563.09 1762.53 -501.44",
    // audit: FIXPOST-126
    "126 35 0.99 0.99 posted update 01 0.99 166.99 0.99 0.00",
    // audit: FIXPOST-127
    "127 33 995.22 2210.32 posted update 01 1764.77 1729.22 1764.77 -445.55",
    // audit: FIXPOST-128
    "128 43 674.99 1854.21 posted update 01 1611.21 1978.21 1611.21 -243.00",
    // audit: FIXPOST-129
    "129 47 648.11 2831.65 rejected 0102",
    // audit: FIXPOST-130
    "130 12 545.66 755.99 posted update 01 755.99 931.99 755.99 0.00",
    // audit: FIXPOST-131
    "131 49 -962.77 -353.34 posted create 03 -962.77 -253.34 609.43 -962.77",
    // audit: FIXPOST-132
    "132 9 851.22 2118.66 posted update 01 2118.66 2678.66 2118.66 0.00",
    // audit: FIXPOST-133
    "133 33 793.22 3003.54 posted update 01 2557.99 2522.44 2557.99 -445.55",
    // audit: FIXPOST-134
    "134 23 408.88 1357.10 posted update 01 1357.10 1461.10 1357.10 0.00",
    // audit: FIXPOST-135
    "135 19 129.33 1079.10 posted update 01 1079.10 1559.10 1079.10 0.00",
    // audit: FIXPOST-136
    "136 19 241.22 1320.32 posted update 01 1320.32 1800.32 1320.32 0.00",
    // audit: FIXPOST-137
    "137 4 28.33 28.33 posted update 01 28.33 68.33 28.33 0.00",
    // audit: FIXPOST-138
    "138 4 437.11 465.44 posted update 01 465.44 505.44 465.44 0.00",
    // audit: FIXPOST-139
    "139 12 587.11 1343.10 posted update 01 1343.10 1519.10 1343.10 0.00",
    // audit: FIXPOST-140
    "140 21 174.66 214.66 posted update 01 214.66 326.66 214.66 0.00",
    // audit: FIXPOST-141
    "141 46 913.88 2253.53 posted update 01 2253.53 2649.53 2253.53 0.00",
    // audit: FIXPOST-142
    "142 50 967.44 1165.32 posted update 01 1165.32 1657.32 1165.32 0.00",
    // audit: FIXPOST-143
    "143 24 310.00 1924.88 posted update 01 1924.88 2324.88 1924.88 0.00",
    // audit: FIXPOST-144
    "144 34 820.11 1471.10 posted update 01 1471.10 1724.10 1471.10 0.00",
    // audit: FIXPOST-145
    "145 27 319.88 1676.98 posted update 01 1676.98 1960.98 1676.98 0.00",
    // audit: FIXPOST-146
    "146 21 584.88 799.54 posted update 01 799.54 911.54 799.54 0.00",
    // audit: FIXPOST-147
    "147 24 161.99 2086.87 posted update 01 2086.87 2486.87 2086.87 0.00",
    // audit: FIXPOST-148
    "148 9 649.00 2767.66 posted update 01 2767.66 3327.66 2767.66 0.00",
    // audit: FIXPOST-149
    "149 16 209.00 288.32 posted update 01 288.32 1021.32 288.32 0.00",
    // audit: FIXPOST-150
    "150 25 -907.00 135.66 posted create 03 -907.00 196.66 1042.66 -907.00",
    // audit: FIXPOST-151
    "151 10 293.44 845.32 posted update 01 845.32 1004.32 845.32 0.00",
    // audit: FIXPOST-152
    "152 36 952.55 1951.54 posted update 01 1951.54 2061.54 1951.54 0.00",
    // audit: FIXPOST-153
    "153 44 654.99 1558.87 posted update 01 1343.54 1391.21 1343.54 -215.33",
    // audit: FIXPOST-154
    "154 7 -579.88 353.22 posted create 03 -579.88 546.22 933.10 -579.88",
    // audit: FIXPOST-155
    "155 7 955.11 2468.09 rejected 0102",
    // audit: FIXPOST-156
    "156 1 81.44 81.44 posted update 01 81.44 275.44 81.44 0.00",
    // audit: FIXPOST-157
    "157 2 655.11 2343.21 posted update 01 1580.21 975.21 1580.21 -763.00",
    // audit: FIXPOST-158
    "158 32 534.11 2450.65 rejected 0102",
    // audit: FIXPOST-159
    "159 28 670.00 1554.22 rejected 0102",
    // audit: FIXPOST-160
    "160 2 164.99 2508.20 posted update 01 1745.20 1140.20 1745.20 -763.00",
    // audit: FIXPOST-161
    "161 29 699.44 3112.20 posted update 01 2259.87 1746.54 2259.87 -852.33",
    // audit: FIXPOST-162
    "162 45 736.88 3489.19 rejected 0102",
    // audit: FIXPOST-163
    "163 18 -113.11 1752.65 posted create 03 -113.11 1896.65 1865.76 -113.11",
    // audit: FIXPOST-164
    "164 1 -70.77 10.67 posted create 03 -70.77 204.67 81.44 -70.77",
    // audit: FIXPOST-165
    "165 26 -372.00 1147.87 posted create 03 -372.00 1193.87 1519.87 -372.00",
    // audit: FIXPOST-166
    "166 33 202.44 3205.98 posted update 01 2760.43 2724.88 2760.43 -445.55",
    // audit: FIXPOST-167
    "167 3 -880.22 234.66 posted create 03 -880.22 381.66 1114.88 -880.22",
    // audit: FIXPOST-168
    "168 35 659.44 660.43 posted update 01 660.43 826.43 660.43 0.00",
    // audit: FIXPOST-169
    "169 13 403.66 786.87 posted update 01 730.10 714.33 730.10 -56.77",
    // audit: FIXPOST-170
    "170 14 195.44 2171.65 posted update 01 2171.65 2186.65 2171.65 0.00",
    // audit: FIXPOST-171
    "171 10 -537.44 307.88 posted create 03 -537.44 466.88 845.32 -537.44",
    // audit: FIXPOST-172
    "172 15 850.22 1868.77 posted update 01 1510.33 1640.89 1510.33 -358.44",
    // audit: FIXPOST-173
    "173 48 722.33 2548.54 rejected 0102",
    // audit: FIXPOST-174
    "174 1 484.99 637.20 posted update 01 566.43 689.66 566.43 -70.77",
    // audit: FIXPOST-175
    "175 46 430.55 2684.08 posted update 01 2684.08 3080.08 2684.08 0.00",
    // audit: FIXPOST-176
    "176 48 623.11 2449.32 rejected 0102",
    // audit: FIXPOST-177
    "177 17 -998.33 -654.56 posted create 03 -998.33 -621.56 343.77 -998.33",
    // audit: FIXPOST-178
    "178 17 352.33 1694.43 rejected 0102",
    // audit: FIXPOST-179
    "179 5 161.77 1260.40 posted update 01 831.41 747.42 831.41 -428.99",
    // audit: FIXPOST-180
    "180 40 237.44 593.66 posted update 01 593.66 636.66 593.66 0.00",
    // audit: FIXPOST-181
    "181 45 689.88 3442.19 rejected 0102",
    // audit: FIXPOST-182
    "182 39 425.11 1898.53 posted update 01 1827.54 2599.55 1827.54 -70.99",
    // audit: FIXPOST-183
    "183 11 621.22 1972.98 posted update 01 1228.21 695.44 1228.21 -744.77",
    // audit: FIXPOST-184
    "184 50 -47.88 1117.44 posted create 03 -47.88 1609.44 1165.32 -47.88",
    // audit: FIXPOST-185
    "185 5 435.44 1695.84 posted update 01 1266.85 1182.86 1266.85 -428.99",
    // audit: FIXPOST-186
    "186 43 568.88 2423.09 posted update 01 2180.09 2547.09 2180.09 -243.00",
    // audit: FIXPOST-187
    "187 35 39.55 699.98 posted update 01 699.98 865.98 699.98 0.00",
    // audit: FIXPOST-188
    "188 4 736.33 1201.77 posted update 01 1201.77 1241.77 1201.77 0.00",
    // audit: FIXPOST-189
    "189 29 744.11 3856.31 posted update 01 3003.98 2490.65 3003.98 -852.33",
    // audit: FIXPOST-190
    "190 30 900.22 1859.99 rejected 0102",
    // audit: FIXPOST-191
    "191 29 166.99 4023.30 posted update 01 3170.97 2657.64 3170.97 -852.33",
    // audit: FIXPOST-192
    "192 8 95.99 2467.31 posted update 01 1632.20 1402.09 1632.20 -835.11",
    // audit: FIXPOST-193
    "193 22 598.33 2793.98 posted update 01 1848.32 957.66 1848.32 -945.66",
    // audit: FIXPOST-194
    "194 12 715.66 2058.76 posted update 01 2058.76 2234.76 2058.76 0.00",
    // audit: FIXPOST-195
    "195 19 -71.66 1248.66 posted create 03 -71.66 1728.66 1320.32 -71.66",
    // audit: FIXPOST-196
    "196 37 -132.88 -132.88 posted create 03 -132.88 -125.88 0.00 -132.88",
    // audit: FIXPOST-197
    "197 23 21.11 1378.21 posted update 01 1378.21 1482.21 1378.21 0.00",
    // audit: FIXPOST-198
    "198 35 639.22 1339.20 posted update 01 1339.20 1505.20 1339.20 0.00",
    // audit: FIXPOST-199
    "199 1 598.44 1235.64 posted update 01 1164.87 1288.10 1164.87 -70.77",
    // audit: FIXPOST-200
    "200 38 -538.22 2449.53 posted create 03 -538.22 3061.53 2987.75 -538.22",
    // audit: FIXPOST-201
    "201 44 269.22 1828.09 posted update 01 1612.76 1660.43 1612.76 -215.33",
    // audit: FIXPOST-202
    "202 21 89.11 888.65 posted update 01 888.65 1000.65 888.65 0.00",
    // audit: FIXPOST-203
    "203 33 629.55 3835.53 posted update 01 3389.98 3354.43 3389.98 -445.55",
    // audit: FIXPOST-204
    "204 3 554.22 2549.32 posted update 01 1669.10 935.88 1669.10 -880.22",
    // audit: FIXPOST-205
    "205 43 744.22 3167.31 posted update 01 2924.31 3291.31 2924.31 -243.00",
    // audit: FIXPOST-206
    "206 6 718.55 2134.31 posted update 01 1738.09 1559.87 1738.09 -396.22",
    // audit: FIXPOST-207
    "207 16 633.55 921.87 posted update 01 921.87 1654.87 921.87 0.00",
    // audit: FIXPOST-208
    "208 18 952.77 2931.64 rejected 0102",
    // audit: FIXPOST-209
    "209 16 425.00 1346.87 posted update 01 1346.87 2079.87 1346.87 0.00",
    // audit: FIXPOST-210
    "210 46 -435.00 2249.08 posted create 03 -435.00 2645.08 2684.08 -435.00",
    // audit: FIXPOST-211
    "211 14 -710.66 1460.99 posted create 03 -710.66 1475.99 2171.65 -710.66",
    // audit: FIXPOST-212
    "212 11 191.00 2163.98 posted update 01 1419.21 886.44 1419.21 -744.77",
    // audit: FIXPOST-213
    "213 34 697.44 2168.54 posted update 01 2168.54 2421.54 2168.54 0.00",
    // audit: FIXPOST-214
    "214 50 94.77 1307.97 posted update 01 1260.09 1704.21 1260.09 -47.88",
    // audit: FIXPOST-215
    "215 40 81.44 675.10 posted update 01 675.10 718.10 675.10 0.00",
    // audit: FIXPOST-216
    "216 25 192.00 2141.66 posted update 01 1234.66 388.66 1234.66 -907.00",
    // audit: FIXPOST-217
    "217 19 329.99 1721.97 posted update 01 1650.31 2058.65 1650.31 -71.66",
    // audit: FIXPOST-218
    "218 24 77.00 2163.87 posted update 01 2163.87 2563.87 2163.87 0.00",
    // audit: FIXPOST-219
    "219 12 -762.44 1296.32 posted create 03 -762.44 1472.32 2058.76 -762.44",
    // audit: FIXPOST-220
    "220 15 983.55 2852.32 posted update 01 2493.88 2624.44 2493.88 -358.44",
    // audit: FIXPOST-221
    "221 40 -25.99 649.11 posted create 03 -25.99 692.11 675.10 -25.99",
    // audit: FIXPOST-222
    "222 36 358.22 2309.76 posted update 01 2309.76 2419.76 2309.76 0.00",
    // audit: FIXPOST-223
    "223 21 910.11 1798.76 rejected 0102",
    // audit: FIXPOST-224
    "224 17 643.44 1985.54 rejected 0102",
    // audit: FIXPOST-225
    "225 14 746.77 3629.08 rejected 0102",
    // audit: FIXPOST-226
    "226 35 -499.99 839.21 posted create 03 -499.99 1005.21 1339.20 -499.99",
    // audit: FIXPOST-227
    "227 30 805.77 1765.54 rejected 0102",
    // audit: FIXPOST-228
    "228 43 13.88 3181.19 posted update 01 2938.19 3305.19 2938.19 -243.00",
    // audit: FIXPOST-229
    "229 9 140.00 2907.66 posted update 01 2907.66 3467.66 2907.66 0.00",
    // audit: FIXPOST-230
    "230 11 653.55 2817.53 posted update 01 2072.76 1539.99 2072.76 -744.77",
    // audit: FIXPOST-231
    "231 1 906.44 2142.08 rejected 0102",
    // audit: FIXPOST-232
    "232 8 735.99 3203.30 posted update 01 2368.19 2138.08 2368.19 -835.11",
    // audit: FIXPOST-233
    "233 19 988.88 2710.85 posted update 01 2639.19 3047.53 2639.19 -71.66",
    // audit: FIXPOST-234
    "234 37 671.11 803.99 rejected 0102",
    // audit: FIXPOST-235
    "235 37 856.44 989.32 rejected 0102",
    // audit: FIXPOST-236
    "236 20 273.11 2712.88 posted update 01 1793.88 1243.88 1793.88 -919.00",
    // audit: FIXPOST-237
    "237 23 65.44 1443.65 posted update 01 1443.65 1547.65 1443.65 0.00",
    // audit: FIXPOST-238
    "238 25 309.44 2451.10 posted update 01 1544.10 698.10 1544.10 -907.00",
    // audit: FIXPOST-239
    "239 34 405.33 2573.87 posted update 01 2573.87 2826.87 2573.87 0.00",
    // audit: FIXPOST-240
    "240 1 985.22 2220.86 rejected 0102",
    // audit: FIXPOST-241
    "241 49 270.55 1842.75 posted update 01 879.98 17.21 879.98 -962.77",
    // audit: FIXPOST-242
    "242 47 489.66 2673.20 rejected 0102",
    // audit: FIXPOST-243
    "243 7 319.00 1831.98 posted update 01 1252.10 865.22 1252.10 -579.88",
    // audit: FIXPOST-244
    "244 3 227.22 2776.54 posted update 01 1896.32 1163.10 1896.32 -880.22",
    // audit: FIXPOST-245
    "245 31 168.66 664.32 posted update 01 664.32 695.32 664.32 0.00",
    // audit: FIXPOST-246
    "246 15 803.99 3656.31 posted update 01 3297.87 3428.43 3297.87 -358.44",
    // audit: FIXPOST-247
    "247 27 975.11 2652.09 posted update 01 2652.09 2936.09 2652.09 0.00",
    // audit: FIXPOST-248
    "248 6 53.55 2187.86 posted update 01 1791.64 1613.42 1791.64 -396.22",
    // audit: FIXPOST-249
    "249 36 -439.33 1870.43 posted create 03 -439.33 1980.43 2309.76 -439.33",
    // audit: FIXPOST-250
    "250 24 -641.77 1522.10 posted create 03 -641.77 1922.10 2163.87 -641.77",
    // audit: FIXPOST-251
    "251 7 34.99 1866.97 posted update 01 1287.09 900.21 1287.09 -579.88",
    // audit: FIXPOST-252
    "252 18 796.00 2774.87 posted update 01 2661.76 2692.65 2661.76 -113.11",
    // audit: FIXPOST-253
    "253 17 111.11 1453.21 rejected 0102",
    // audit: FIXPOST-254
    "254 2 594.77 3102.97 posted update 01 2339.97 1734.97 2339.97 -763.00",
    // audit: FIXPOST-255
    "255 31 -362.22 302.10 posted create 03 -362.22 333.10 664.32 -362.22",
    // audit: FIXPOST-256
    "256 21 -41.77 846.88 posted create 03 -41.77 958.88 888.65 -41.77",
    // audit: FIXPOST-257
    "257 50 241.66 1549.63 posted update 01 1501.75 1945.87 1501.75 -47.88",
    // audit: FIXPOST-258
    "258 34 -457.55 2116.32 posted create 03 -457.55 2369.32 2573.87 -457.55",
    // audit: FIXPOST-259
    "259 13 702.22 1489.09 posted update 01 1432.32 1416.55 1432.32 -56.77",
    // audit: FIXPOST-260
    "260 4 281.55 1483.32 posted update 01 1483.32 1523.32 1483.32 0.00",
    // audit: FIXPOST-261
    "261 25 45.66 2496.76 posted update 01 1589.76 743.76 1589.76 -907.00",
    // audit: FIXPOST-262
    "262 16 -270.99 1075.88 posted create 03 -270.99 1808.88 1346.87 -270.99",
    // audit: FIXPOST-263
    "263 10 80.66 1463.42 posted update 01 925.98 547.54 925.98 -537.44",
    // audit: FIXPOST-264
    "264 49 198.66 2041.41 posted update 01 1078.64 215.87 1078.64 -962.77",
    // audit: FIXPOST-265
    "265 4 -322.99 1160.33 posted create 03 -322.99 1200.33 1483.32 -322.99",
    // audit: FIXPOST-266
    "266 22 768.88 3562.86 posted update 01 2617.20 1726.54 2617.20 -945.66",
    // audit: FIXPOST-267
    "267 36 720.99 3470.08 rejected 0102",
    // audit: FIXPOST-268
    "268 44 399.44 2227.53 posted update 01 2012.20 2059.87 2012.20 -215.33",
    // audit: FIXPOST-269
    "269 12 298.33 3119.53 posted update 01 2357.09 1770.65 2357.09 -762.44",
    // audit: FIXPOST-270
    "270 40 194.33 895.42 posted update 01 869.43 886.44 869.43 -25.99",
    // audit: FIXPOST-271
    "271 31 818.00 1844.54 rejected 0102",
    // audit: FIXPOST-272
    "272 31 242.22 1268.76 rejected 0102",
    // audit: FIXPOST-273
    "273 35 973.11 2812.30 rejected 0102",
    // audit: FIXPOST-274
    "274 6 958.77 3146.63 posted update 01 2750.41 2572.19 2750.41 -396.22",
    // audit: FIXPOST-275
    "275 4 462.33 2268.64 posted update 01 1945.65 1662.66 1945.65 -322.99",
    // audit: FIXPOST-276
    "276 20 86.00 2798.88 posted update 01 1879.88 1329.88 1879.88 -919.00",
    // audit: FIXPOST-277
    "277 27 932.55 3584.64 posted update 01 3584.64 3868.64 3584.64 0.00",
    // audit: FIXPOST-278
    "278 46 893.33 4012.41 posted update 01 3577.41 3538.41 3577.41 -435.00",
    // audit: FIXPOST-279
    "279 13 759.22 2248.31 posted update 01 2191.54 2175.77 2191.54 -56.77",
    // audit: FIXPOST-280
    "280 31 130.11 1156.65 rejected 0102",
    // audit: FIXPOST-281
    "281 27 -372.99 3211.65 posted create 03 -372.99 3495.65 3584.64 -372.99",
    // audit: FIXPOST-282
    "282 13 684.33 2932.64 posted update 01 2875.87 2860.10 2875.87 -56.77",
    // audit: FIXPOST-283
    "283 14 920.11 3802.42 rejected 0102",
    // audit: FIXPOST-284
    "284 9 -260.11 2647.55 posted create 03 -260.11 3207.55 2907.66 -260.11",
    // audit: FIXPOST-285
    "285 48 222.44 2048.65 posted update 01 1512.77 1202.89 1512.77 -535.88",
    // audit: FIXPOST-286
    "286 22 711.66 4274.52 posted update 01 3328.86 2438.20 3328.86 -945.66",
    // audit: FIXPOST-287
    "287 23 437.77 1881.42 posted update 01 1881.42 1985.42 1881.42 0.00",
    // audit: FIXPOST-288
    "288 41 220.00 2841.53 posted update 01 2084.87 1703.21 2084.87 -756.66",
    // audit: FIXPOST-289
    "289 39 223.33 2121.86 posted update 01 2050.87 2822.88 2050.87 -70.99",
    // audit: FIXPOST-290
    "290 20 573.22 3372.10 posted update 01 2453.10 1903.10 2453.10 -919.00",
    // audit: FIXPOST-291
    "291 41 314.66 3156.19 posted update 01 2399.53 2017.87 2399.53 -756.66",
    // audit: FIXPOST-292
    "292 37 548.33 681.21 rejected 0102",
    // audit: FIXPOST-293
    "293 3 694.55 3471.09 posted update 01 2590.87 1857.65 2590.87 -880.22",
    // audit: FIXPOST-294
    "294 23 -835.44 1045.98 posted create 03 -835.44 1149.98 1881.42 -835.44",
    // audit: FIXPOST-295
    "295 15 784.11 4440.42 posted update 01 4081.98 4212.54 4081.98 -358.44",
    // audit: FIXPOST-296
    "296 42 402.22 2666.19 posted update 01 2164.75 1965.31 2164.75 -501.44",
    // audit: FIXPOST-297
    "297 49 917.44 2958.85 posted update 01 1996.08 1133.31 1996.08 -962.77",
    // audit: FIXPOST-298
    "298 40 765.66 1661.08 posted update 01 1635.09 1652.10 1635.09 -25.99",
    // audit: FIXPOST-299
    "299 37 635.99 768.87 rejected 0102",
    // audit: FIXPOST-300
    "300 10 603.22 2066.64 posted update 01 1529.20 1150.76 1529.20 -537.44",
];

/// The `INTEREST-RAW` results of `FIXINT-001` to `FIXINT-050`, one per `tcatbal.txt` line, as
/// [`parse_raw_interest_row`] reads them.
const INTEREST_RAW_ROWS: [&str; 50] = [
    // audit: FIXINT-001
    "1 0.00 0.00 194.00 194.00",
    // audit: FIXINT-002
    "2 0.00 0.00 158.00 158.00",
    // audit: FIXINT-003
    "3 0.00 0.00 147.00 147.00",
    // audit: FIXINT-004
    "4 0.00 0.00 40.00 40.00",
    // audit: FIXINT-005
    "5 0.00 0.00 345.00 345.00",
    // audit: FIXINT-006
    "6 0.00 0.00 218.00 218.00",
    // audit: FIXINT-007
    "7 0.00 0.00 193.00 193.00",
    // audit: FIXINT-008
    "8 0.00 0.00 605.00 605.00",
    // audit: FIXINT-009
    "9 0.00 0.00 560.00 560.00",
    // audit: FIXINT-010
    "10 0.00 0.00 159.00 159.00",
    // audit: FIXINT-011
    "11 0.00 0.00 212.00 212.00",
    // audit: FIXINT-012
    "12 0.00 0.00 176.00 176.00",
    // audit: FIXINT-013
    "13 0.00 0.00 41.00 41.00",
    // audit: FIXINT-014
    "14 0.00 0.00 15.00 15.00",
    // audit: FIXINT-015
    "15 0.00 0.00 489.00 489.00",
    // audit: FIXINT-016
    "16 0.00 0.00 733.00 733.00",
    // audit: FIXINT-017
    "17 0.00 0.00 33.00 33.00",
    // audit: FIXINT-018
    "18 0.00 0.00 144.00 144.00",
    // audit: FIXINT-019
    "19 0.00 0.00 480.00 480.00",
    // audit: FIXINT-020
    "20 0.00 0.00 369.00 369.00",
    // audit: FIXINT-021
    "21 0.00 0.00 112.00 112.00",
    // audit: FIXINT-022
    "22 0.00 0.00 55.00 55.00",
    // audit: FIXINT-023
    "23 0.00 0.00 104.00 104.00",
    // audit: FIXINT-024
    "24 0.00 0.00 400.00 400.00",
    // audit: FIXINT-025
    "25 0.00 0.00 61.00 61.00",
    // audit: FIXINT-026
    "26 0.00 0.00 46.00 46.00",
    // audit: FIXINT-027
    "27 0.00 0.00 284.00 284.00",
    // audit: FIXINT-028
    "28 0.00 0.00 68.00 68.00",
    // audit: FIXINT-029
    "29 0.00 0.00 339.00 339.00",
    // audit: FIXINT-030
    "30 0.00 0.00 2.00 2.00",
    // audit: FIXINT-031
    "31 0.00 0.00 31.00 31.00",
    // audit: FIXINT-032
    "32 0.00 0.00 30.00 30.00",
    // audit: FIXINT-033
    "33 0.00 0.00 410.00 410.00",
    // audit: FIXINT-034
    "34 0.00 0.00 253.00 253.00",
    // audit: FIXINT-035
    "35 0.00 0.00 166.00 166.00",
    // audit: FIXINT-036
    "36 0.00 0.00 110.00 110.00",
    // audit: FIXINT-037
    "37 0.00 0.00 7.00 7.00",
    // audit: FIXINT-038
    "38 0.00 0.00 612.00 612.00",
    // audit: FIXINT-039
    "39 0.00 0.00 843.00 843.00",
    // audit: FIXINT-040
    "40 0.00 0.00 43.00 43.00",
    // audit: FIXINT-041
    "41 0.00 0.00 375.00 375.00",
    // audit: FIXINT-042
    "42 0.00 0.00 302.00 302.00",
    // audit: FIXINT-043
    "43 0.00 0.00 610.00 610.00",
    // audit: FIXINT-044
    "44 0.00 0.00 263.00 263.00",
    // audit: FIXINT-045
    "45 0.00 0.00 186.00 186.00",
    // audit: FIXINT-046
    "46 0.00 0.00 396.00 396.00",
    // audit: FIXINT-047
    "47 0.00 0.00 32.00 32.00",
    // audit: FIXINT-048
    "48 0.00 0.00 226.00 226.00",
    // audit: FIXINT-049
    "49 0.00 0.00 100.00 100.00",
    // audit: FIXINT-050
    "50 0.00 0.00 492.00 never",
];

/// The `INTEREST-POSTED` results of `FIXINT-001` to `FIXINT-050`, one per account, as
/// [`parse_posted_interest_row`] reads them.
const INTEREST_POSTED_ROWS: [&str; 50] = [
    // audit: FIXINT-001
    "1 1164.87 -70.77 14.56 1288.10 1164.87 -70.77 1302.66",
    // audit: FIXINT-002
    "2 2339.97 -763.00 29.24 1734.97 2339.97 -763.00 1764.21",
    // audit: FIXINT-003
    "3 2590.87 -880.22 32.38 1857.65 2590.87 -880.22 1890.03",
    // audit: FIXINT-004
    "4 1945.65 -322.99 24.32 1662.66 1945.65 -322.99 1686.98",
    // audit: FIXINT-005
    "5 1266.85 -428.99 15.83 1182.86 1266.85 -428.99 1198.69",
    // audit: FIXINT-006
    "6 2750.41 -396.22 34.38 2572.19 2750.41 -396.22 2606.57",
    // audit: FIXINT-007
    "7 1287.09 -579.88 16.08 900.21 1287.09 -579.88 916.29",
    // audit: FIXINT-008
    "8 2368.19 -835.11 29.60 2138.08 2368.19 -835.11 2167.68",
    // audit: FIXINT-009
    "9 2907.66 -260.11 36.34 3207.55 2907.66 -260.11 3243.89",
    // audit: FIXINT-010
    "10 1529.20 -537.44 19.11 1150.76 1529.20 -537.44 1169.87",
    // audit: FIXINT-011
    "11 2072.76 -744.77 25.90 1539.99 2072.76 -744.77 1565.89",
    // audit: FIXINT-012
    "12 2357.09 -762.44 29.46 1770.65 2357.09 -762.44 1800.11",
    // audit: FIXINT-013
    "13 2875.87 -56.77 35.94 2860.10 2875.87 -56.77 2896.04",
    // audit: FIXINT-014
    "14 2171.65 -710.66 27.14 1475.99 2171.65 -710.66 1503.13",
    // audit: FIXINT-015
    "15 4081.98 -358.44 51.02 4212.54 4081.98 -358.44 4263.56",
    // audit: FIXINT-016
    "16 1346.87 -270.99 16.83 1808.88 1346.87 -270.99 1825.71",
    // audit: FIXINT-017
    "17 343.77 -998.33 4.29 -621.56 343.77 -998.33 -617.27",
    // audit: FIXINT-018
    "18 2661.76 -113.11 33.27 2692.65 2661.76 -113.11 2725.92",
    // audit: FIXINT-019
    "19 2639.19 -71.66 32.98 3047.53 2639.19 -71.66 3080.51",
    // audit: FIXINT-020
    "20 2453.10 -919.00 30.66 1903.10 2453.10 -919.00 1933.76",
    // audit: FIXINT-021
    "21 888.65 -41.77 11.10 958.88 888.65 -41.77 969.98",
    // audit: FIXINT-022
    "22 3328.86 -945.66 41.61 2438.20 3328.86 -945.66 2479.81",
    // audit: FIXINT-023
    "23 1881.42 -835.44 23.51 1149.98 1881.42 -835.44 1173.49",
    // audit: FIXINT-024
    "24 2163.87 -641.77 27.04 1922.10 2163.87 -641.77 1949.14",
    // audit: FIXINT-025
    "25 1589.76 -907.00 19.87 743.76 1589.76 -907.00 763.63",
    // audit: FIXINT-026
    "26 1519.87 -372.00 18.99 1193.87 1519.87 -372.00 1212.86",
    // audit: FIXINT-027
    "27 3584.64 -372.99 44.80 3495.65 3584.64 -372.99 3540.45",
    // audit: FIXINT-028
    "28 809.00 -75.22 10.11 801.78 809.00 -75.22 811.89",
    // audit: FIXINT-029
    "29 3170.97 -852.33 39.63 2657.64 3170.97 -852.33 2697.27",
    // audit: FIXINT-030
    "30 29.44 -930.33 0.36 -898.89 29.44 -930.33 -898.53",
    // audit: FIXINT-031
    "31 664.32 -362.22 8.30 333.10 664.32 -362.22 341.40",
    // audit: FIXINT-032
    "32 1173.55 -742.99 14.66 460.56 1173.55 -742.99 475.22",
    // audit: FIXINT-033
    "33 3389.98 -445.55 42.37 3354.43 3389.98 -445.55 3396.80",
    // audit: FIXINT-034
    "34 2573.87 -457.55 32.17 2369.32 2573.87 -457.55 2401.49",
    // audit: FIXINT-035
    "35 1339.20 -499.99 16.74 1005.21 1339.20 -499.99 1021.95",
    // audit: FIXINT-036
    "36 2309.76 -439.33 28.87 1980.43 2309.76 -439.33 2009.30",
    // audit: FIXINT-037
    "37 0.00 -132.88 0.00 -125.88 0.00 -132.88 -125.88",
    // audit: FIXINT-038
    "38 2987.75 -538.22 37.34 3061.53 2987.75 -538.22 3098.87",
    // audit: FIXINT-039
    "39 2050.87 -70.99 25.63 2822.88 2050.87 -70.99 2848.51",
    // audit: FIXINT-040
    "40 1635.09 -25.99 20.43 1652.10 1635.09 -25.99 1672.53",
    // audit: FIXINT-041
    "41 2399.53 -756.66 29.99 2017.87 2399.53 -756.66 2047.86",
    // audit: FIXINT-042
    "42 2164.75 -501.44 27.05 1965.31 2164.75 -501.44 1992.36",
    // audit: FIXINT-043
    "43 2938.19 -243.00 36.72 3305.19 2938.19 -243.00 3341.91",
    // audit: FIXINT-044
    "44 2012.20 -215.33 25.15 2059.87 2012.20 -215.33 2085.02",
    // audit: FIXINT-045
    "45 2407.54 -344.77 30.09 2248.77 2407.54 -344.77 2278.86",
    // audit: FIXINT-046
    "46 3577.41 -435.00 44.71 3538.41 3577.41 -435.00 3583.12",
    // audit: FIXINT-047
    "47 1598.10 -585.44 19.97 1044.66 1598.10 -585.44 1064.63",
    // audit: FIXINT-048
    "48 1512.77 -535.88 18.90 1202.89 1512.77 -535.88 1221.79",
    // audit: FIXINT-049
    "49 1996.08 -962.77 24.95 1133.31 1996.08 -962.77 1158.26",
    // audit: FIXINT-050
    "50 1501.75 -47.88 18.77 1945.87 1501.75 -47.88 never",
];

/// The masters of the `LOADED` scenario, decoded in place by codec A.
struct Masters {
    accounts: Vec<AccountRecord>,
    xrefs: Vec<CardXrefRecord>,
    tcatbals: Vec<TranCatBalanceRecord>,
    groups: Vec<DisclosureGroupRecord>,
}

impl Masters {
    fn load() -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            accounts: fixture("acctdata.txt")?,
            xrefs: fixture("cardxref.txt")?,
            tcatbals: fixture("tcatbal.txt")?,
            groups: fixture("discgrp.txt")?,
        })
    }

    /// The loaded account `acct_id`.
    fn account(&self, acct_id: u64) -> Result<&AccountRecord, Box<dyn Error>> {
        self.accounts
            .iter()
            .find(|account| account.id.get() == acct_id)
            .ok_or_else(|| format!("account {acct_id} is not in acctdata.txt").into())
    }

    /// The cross-reference of account `acct_id`; every fixture account has exactly one
    /// (FIXFRM-003).
    fn xref(&self, acct_id: u64) -> Result<&CardXrefRecord, Box<dyn Error>> {
        self.xrefs
            .iter()
            .find(|xref| xref.acct_id.get() == acct_id)
            .ok_or_else(|| format!("account {acct_id} is not in cardxref.txt").into())
    }

    /// The `DEFAULT` rate of type `type_cd`, category `0001`, as `discgrp.txt` holds it.
    fn default_rate(&self, type_cd: [u8; 2]) -> Result<Rate<4>, Box<dyn Error>> {
        self.groups
            .iter()
            .find(|group| {
                group.acct_group_id.as_bytes() == b"DEFAULT   "
                    && *group.tran_type_cd.as_bytes() == type_cd
                    && group.tran_cat_cd.get() == 1
            })
            .map(|group| group.int_rate)
            .ok_or_else(|| "discgrp.txt holds no DEFAULT rate for the type".into())
    }
}

/// The `POSTED` scenario: every outcome of `Posttran` in input order, and the tables after the
/// postings are applied in that order.
struct PostedRun {
    daily: Vec<DailyTransactionRecord>,
    outcomes: Vec<RecordOutcome>,
    accounts: BTreeMap<u64, AccountRecord>,
    tcatbals: BTreeMap<CategoryKey, TranCatBalanceRecord>,
    updated: BTreeSet<CategoryKey>,
    summary: PosttranSummary,
}

/// Runs `posttran` over `dailytran.txt` against `masters` with the `POSTED` instant
/// `2022-07-01 12:00:00`, applying each posting's category balance and account to the tables.
fn run_posttran(masters: &Masters) -> Result<PostedRun, Box<dyn Error>> {
    let daily: Vec<DailyTransactionRecord> = fixture("dailytran.txt")?;
    let clock = FixedClock::new(instant(2022, 7, 1, 12)?);
    let mut run = Posttran::new(
        masters.accounts.clone(),
        masters.xrefs.clone(),
        masters.tcatbals.clone(),
    );
    let mut accounts: BTreeMap<u64, AccountRecord> = masters
        .accounts
        .iter()
        .map(|account| (account.id.get(), account.clone()))
        .collect();
    let mut tcatbals: BTreeMap<CategoryKey, TranCatBalanceRecord> = masters
        .tcatbals
        .iter()
        .map(|record| (category_key(record), record.clone()))
        .collect();
    let mut updated = BTreeSet::new();
    let mut outcomes = Vec::with_capacity(daily.len());
    for record in &daily {
        let outcome = run.process(record, &clock)?;
        if let RecordOutcome::Posted(posting) = &outcome {
            let written = posting.tcatbal.record();
            if matches!(posting.tcatbal, TcatbalWrite::Update(_)) {
                updated.insert(category_key(written));
            }
            tcatbals.insert(category_key(written), written.clone());
            accounts.insert(posting.account.id.get(), posting.account.clone());
        }
        outcomes.push(outcome);
    }
    Ok(PostedRun {
        daily,
        outcomes,
        accounts,
        tcatbals,
        updated,
        summary: run.finish(),
    })
}

/// One effect `Intcalc::process` returns, as plain values: the account ID a rewrite updates, or
/// the amount of a SYSTRAN record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordEffect {
    Rewrite(u64),
    Systran(Money<9>),
}

/// An INTCALC run: each SYSTRAN record with the key of the category balance that wrote it, every
/// account update by account ID, the effects of each record read in the order its `process` call
/// returned them, and the summary.
struct InterestRun {
    systran: Vec<(CategoryKey, TransactionRecord)>,
    updates: BTreeMap<u64, AccountRecord>,
    effects: Vec<Vec<RecordEffect>>,
    summary: IntcalcSummary,
}

/// Runs `intcalc` with the default PARM `2022071800` and the interest instant
/// `2022-07-18 00:00:00` over `tcatbals` in the order given, against `accounts` and the loaded
/// cross-references and disclosure groups.
fn run_intcalc(
    accounts: Vec<AccountRecord>,
    tcatbals: &[TranCatBalanceRecord],
    masters: &Masters,
) -> Result<InterestRun, Box<dyn Error>> {
    let clock = FixedClock::new(instant(2022, 7, 18, 0)?);
    let mut run = Intcalc::new(
        ParmDate::DEFAULT,
        accounts,
        masters.xrefs.clone(),
        masters.groups.clone(),
    );
    let mut systran = Vec::new();
    let mut updates = BTreeMap::new();
    let mut effects = Vec::with_capacity(tcatbals.len());
    for tcatbal in tcatbals {
        let mut record_effects = Vec::new();
        for effect in run.process(tcatbal, &clock)? {
            match effect {
                IntcalcEffect::AccountUpdate(account) => {
                    let acct_id = account.id.get();
                    record_effects.push(RecordEffect::Rewrite(acct_id));
                    if updates.insert(acct_id, account).is_some() {
                        return Err(format!("account {acct_id} is rewritten twice").into());
                    }
                }
                IntcalcEffect::SystemTransaction(record) => {
                    record_effects.push(RecordEffect::Systran(record.amt));
                    systran.push((category_key(tcatbal), record));
                }
            }
        }
        effects.push(record_effects);
    }
    Ok(InterestRun {
        systran,
        updates,
        effects,
        summary: run.finish(),
    })
}

/// Whether both timestamps of `record` are written in `expected`'s layout.
fn stamped(record: &TransactionRecord, expected: TsForm) -> bool {
    form(&record.orig_ts) == Some(expected) && form(&record.proc_ts) == Some(expected)
}

/// Checks the account update a run made against the balance the audit expects after it, `None`
/// for the account the run never rewrites (E08, ARITH-017). A rewrite sets both cycle fields to
/// 0.00 (ARITH-014).
fn check_rewrite(
    id: &str,
    expected: Option<Money<10>>,
    update: Option<&AccountRecord>,
) -> TestResult {
    match (expected, update) {
        (Some(bal), Some(update)) => {
            assert_eq!(update.curr_bal, bal, "{id}: balance after the rewrite");
            assert!(
                update.curr_cyc_credit.is_zero() && update.curr_cyc_debit.is_zero(),
                "{id}: cycle fields after the rewrite"
            );
            Ok(())
        }
        (None, None) => Ok(()),
        (Some(_), None) => Err(format!("{id}: the account is never rewritten").into()),
        (None, Some(_)) => Err(format!("{id}: the last account is rewritten").into()),
    }
}

// ---------------------------------------------------------------------------------------------
// CBACT04C: interest (ARITH-001 to ARITH-017, FIXINT)
// ---------------------------------------------------------------------------------------------

/// `1300-COMPUTE-INTEREST` worked cases: (`TRAN-CAT-BAL`, `DIS-INT-RATE`, the
/// `WS-MONTHLY-INT` stored), in hundredths. ARITH-006 and ARITH-007 are the cases where rounding
/// would give one cent more. The CBACT04C-013 cases take the sign of the product and store 0.00
/// for a non-zero product whose kept digits are all zero; a zero stored from a negative product
/// has no case, as its sign has no single result (X-ARITH-7, X-INTCALC-H6).
const MONTHLY_INTEREST_CASES: [(i64, i64, i64); 13] = [
    // audit: ARITH-002
    (123_456, 1_500, 1_543),
    // audit: ARITH-003
    (-123_456, 1_500, -1_543),
    // audit: ARITH-004
    (240_000, 1_500, 3_000),
    // audit: ARITH-005
    (10_000, 1_000, 83),
    // audit: ARITH-006
    (20_000, 1_000, 166),
    // audit: ARITH-006
    (-20_000, 1_000, -166),
    // audit: ARITH-007
    (119_999, 100, 99),
    // audit: ARITH-008
    (99_999_999_999, 999_999, 33_332_499_991),
    // audit: ARITH-008
    (-99_999_999_999, 999_999, -33_332_499_991),
    // audit: CBACT04C-013
    (120_000, -1_200, -1_200),
    // audit: CBACT04C-013
    (-120_000, -1_200, 1_200),
    // audit: CBACT04C-013
    (79, 1_500, 0),
    // audit: CBACT04C-013
    (15_000_000_000, 800_000, 0),
];

// audit: ARITH-001
// audit: CBACT04C-013
#[test]
fn monthly_interest_truncates_toward_zero_and_drops_high_order_digits() -> TestResult {
    for (balance, rate_hundredths, expected) in MONTHLY_INTEREST_CASES {
        let interest = monthly_interest(money(balance)?, rate(rate_hundredths)?);
        assert_eq!(
            interest,
            money::<9>(expected)?,
            "balance {balance}, rate {rate_hundredths} (hundredths)"
        );
    }
    Ok(())
}

/// `ADD WS-MONTHLY-INT TO WS-TOTAL-INT` worked cases: (`WS-TOTAL-INT`, `WS-MONTHLY-INT`, the
/// total stored into `S9(09)V99`), in hundredths.
const TOTAL_INTEREST_CASES: [(i64, i64, i64); 4] = [
    // audit: ARITH-010
    (0, 1_543, 1_543),
    // audit: ARITH-010
    (1_543, 83, 1_626),
    // audit: ARITH-011
    (99_999_999_999, 1, 0),
    // audit: ARITH-011
    (-99_999_999_999, -2, -1),
];

#[test]
fn the_interest_total_stores_into_s9_09_v99() -> TestResult {
    for (total, monthly, expected) in TOTAL_INTEREST_CASES {
        let stored = (money::<9>(total)? + money::<9>(monthly)?).store::<9>();
        assert_eq!(
            stored,
            money::<9>(expected)?,
            "total {total} + monthly {monthly} (hundredths)"
        );
    }
    Ok(())
}

/// `1050-UPDATE-ACCOUNT` worked cases: (`ACCT-CURR-BAL`, `WS-TOTAL-INT`, the balance stored into
/// `S9(10)V99`), in hundredths.
const INTEREST_BALANCE_CASES: [(i64, i64, i64); 3] = [
    // audit: ARITH-013
    (84_300, 1_543, 85_843),
    // audit: ARITH-013
    (84_300, 0, 84_300),
    // audit: ARITH-013
    (999_999_999_999, 1, 0),
];

// audit: ARITH-014
// audit: CBACT04C-026
#[test]
fn the_interest_rewrite_adds_the_total_and_zeroes_both_cycle_fields() -> TestResult {
    let masters = Masters::load()?;
    let loaded = masters.account(1)?;
    for (bal, total, expected) in INTEREST_BALANCE_CASES {
        let account = AccountRecord {
            curr_bal: money(bal)?,
            curr_cyc_credit: money(100_000)?,
            curr_cyc_debit: money(-25_000)?,
            ..loaded.clone()
        };
        let rewritten = cbact04c::update_account(account.clone(), money(total)?);
        let expected = AccountRecord {
            curr_bal: money(expected)?,
            curr_cyc_credit: Money::ZERO,
            curr_cyc_debit: Money::ZERO,
            ..account
        };
        assert_eq!(
            rewritten, expected,
            "balance {bal} + total {total} (hundredths)"
        );
    }
    Ok(())
}

// audit: ARITH-002
// audit: ARITH-004
// audit: ARITH-012
// audit: ARITH-013
// audit: ARITH-015
// audit: ARITH-016
// audit: ARITH-017
// audit: CBACT04C-012
// audit: CBACT04C-027
#[test]
fn intcalc_skips_a_zero_rate_and_applies_each_total_at_the_next_break() -> TestResult {
    let masters = Masters::load()?;
    let [first, second, ..] = masters.tcatbals.as_slice() else {
        return Err("tcatbal.txt holds fewer than two records".into());
    };
    assert_eq!(masters.default_rate(*b"01")?, rate(DEFAULT_PURCHASE_RATE)?);
    assert!(masters.default_rate(*b"02")?.is_zero(), "DEFAULT 02/0001");
    // ARITH-002 and ARITH-004 at DEFAULT 01/0001, and ARITH-015 at DEFAULT 02/0001, rate 0.00.
    let records = [
        TranCatBalanceRecord {
            bal: money(123_456)?,
            ..first.clone()
        },
        TranCatBalanceRecord {
            type_cd: PicX::from_array(*b"02"),
            bal: money(50_000)?,
            ..first.clone()
        },
        TranCatBalanceRecord {
            bal: money(240_000)?,
            ..second.clone()
        },
    ];
    // ARITH-013's worked balance, with cycle fields the rewrite zeroes.
    let mut accounts = masters.accounts.clone();
    for account in &mut accounts {
        if account.id == first.acct_id {
            account.curr_bal = money(84_300)?;
            account.curr_cyc_credit = money(100_000)?;
            account.curr_cyc_debit = money(-25_000)?;
        }
    }
    let before = accounts
        .iter()
        .find(|account| account.id == first.acct_id)
        .cloned()
        .ok_or("the first account is not in acctdata.txt")?;
    let run = run_intcalc(accounts, &records, &masters)?;
    // ARITH-016: `1400-COMPUTE-FEES` computes no fee. A record at a non-zero rate returns its
    // SYSTRAN record of the monthly interest alone, the zero-rate record returns nothing, and the
    // break adds only the rewrite of the previous account.
    assert_eq!(
        run.effects,
        [
            vec![RecordEffect::Systran(money(1_543)?)],
            vec![],
            vec![
                RecordEffect::Rewrite(first.acct_id.get()),
                RecordEffect::Systran(money(3_000)?),
            ],
        ],
        "ARITH-016: no fee effect"
    );
    // ARITH-016: the rewrite moves the balance by the interest total alone and zeroes the cycle
    // fields; no other field changes.
    let rewritten = AccountRecord {
        curr_bal: money(85_843)?,
        curr_cyc_credit: Money::ZERO,
        curr_cyc_debit: Money::ZERO,
        ..before
    };
    assert_eq!(
        run.updates.get(&first.acct_id.get()),
        Some(&rewritten),
        "ARITH-016: no fee field changes"
    );
    let amounts: Vec<(CategoryKey, Money<9>)> = run
        .systran
        .iter()
        .map(|(key, record)| (*key, record.amt))
        .collect();
    let expected = [
        (category_key(first), money::<9>(1_543)?),
        (category_key(second), money::<9>(3_000)?),
    ];
    assert_eq!(amounts, expected, "no SYSTRAN record for the 0.00 rate");
    // The zero-rate record leaves the total at 15.43: 843.00 + 15.43.
    let updated: Vec<(u64, Money<10>)> = run
        .updates
        .values()
        .map(|account| (account.id.get(), account.curr_bal))
        .collect();
    assert_eq!(updated, [(first.acct_id.get(), money::<10>(85_843)?)]);
    check_rewrite(
        "account break",
        Some(money(85_843)?),
        run.updates.get(&first.acct_id.get()),
    )?;
    assert_eq!(run.summary.records_read(), 3);
    assert_eq!(run.summary.transactions_written(), 2);
    assert_eq!(run.summary.unapplied_last_account(), Some(second.acct_id));
    assert_eq!(run.summary.unapplied_total(), Some(money(3_000)?));
    Ok(())
}

// audit: ARITH-010
// audit: CBACT04C-014
#[test]
fn intcalc_sums_each_accounts_interest_and_restarts_the_total_at_its_break() -> TestResult {
    let masters = Masters::load()?;
    let [first, second, third, ..] = masters.tcatbals.as_slice() else {
        return Err("tcatbal.txt holds fewer than three records".into());
    };
    for type_cd in [*b"01", *b"04", *b"05"] {
        let type_text = String::from_utf8_lossy(&type_cd);
        assert_eq!(
            masters.default_rate(type_cd)?,
            rate(DEFAULT_PURCHASE_RATE)?,
            "DEFAULT {type_text}/0001"
        );
    }
    // At 15.00 the monthly interest of 1234.56 is 15.43 (ARITH-002), of -1234.56 is -15.43
    // (ARITH-003) and of 2400.00 is 30.00 (ARITH-004).
    let balance = |account: &TranCatBalanceRecord, type_cd: [u8; 2], cents: i64| {
        Ok::<_, MoneyError>(TranCatBalanceRecord {
            type_cd: PicX::from_array(type_cd),
            bal: money(cents)?,
            ..account.clone()
        })
    };
    let records = [
        balance(first, *b"01", 123_456)?,
        balance(first, *b"04", 240_000)?,
        balance(second, *b"01", 240_000)?,
        balance(second, *b"05", -123_456)?,
        balance(third, *b"01", 123_456)?,
    ];
    // ARITH-013's worked balance for the two accounts the run rewrites.
    let mut accounts = masters.accounts.clone();
    for account in &mut accounts {
        if account.id == first.acct_id || account.id == second.acct_id {
            account.curr_bal = money(84_300)?;
        }
    }
    let run = run_intcalc(accounts, &records, &masters)?;
    let amounts: Vec<Money<9>> = run.systran.iter().map(|(_, record)| record.amt).collect();
    assert_eq!(
        amounts,
        [
            money::<9>(1_543)?,
            money(3_000)?,
            money(3_000)?,
            money(-1_543)?,
            money(1_543)?,
        ],
        "one SYSTRAN record per non-zero rate"
    );
    // The first account's total is 15.43 + 30.00 = 45.43. The break restarts it at 0.00, so the
    // second account's is 30.00 + (-15.43) = 14.57, not 59.86, and the third's is 15.43, not
    // 75.43.
    let updated: Vec<(u64, Money<10>)> = run
        .updates
        .values()
        .map(|account| (account.id.get(), account.curr_bal))
        .collect();
    assert_eq!(
        updated,
        [
            (first.acct_id.get(), money::<10>(88_843)?),
            (second.acct_id.get(), money(85_757)?),
        ],
        "843.00 plus each account's own total"
    );
    assert_eq!(run.summary.unapplied_last_account(), Some(third.acct_id));
    assert_eq!(run.summary.unapplied_total(), Some(money(1_543)?));
    Ok(())
}

// audit: ARITH-011
// audit: CBACT04C-014
#[test]
fn intcalc_stores_each_accounts_total_into_s9_09_v99_and_keeps_its_sign() -> TestResult {
    let masters = Masters::load()?;
    let [first, second, third, ..] = masters.tcatbals.as_slice() else {
        return Err("tcatbal.txt holds fewer than three records".into());
    };
    // `DEFAULT` `01`/`0001` to `01`/`0004` at 9999.99, the widest `DIS-INT-RATE`. Every fixture
    // account's group is blank, so each record takes its category's `DEFAULT` rate (E07).
    let groups = (1..=4)
        .map(|cat| -> Result<DisclosureGroupRecord, Box<dyn Error>> {
            Ok(DisclosureGroupRecord {
                acct_group_id: PicX::from_array(*b"DEFAULT   "),
                tran_type_cd: PicX::from_array(*b"01"),
                tran_cat_cd: PicU::new(cat)?,
                int_rate: rate(999_999)?,
                filler: PicX::spaces(),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let balance = |account: &TranCatBalanceRecord, cat: u64, cents: i64| {
        Ok::<_, Box<dyn Error>>(TranCatBalanceRecord {
            type_cd: PicX::from_array(*b"01"),
            cd: PicU::new(cat)?,
            bal: money(cents)?,
            ..account.clone()
        })
    };
    // Four categories of 999999999.99 for the first account and of -999999999.99 for the
    // second, then a 0.00 balance of the third, whose record is the second account's break.
    let records = [
        balance(first, 1, 99_999_999_999)?,
        balance(first, 2, 99_999_999_999)?,
        balance(first, 3, 99_999_999_999)?,
        balance(first, 4, 99_999_999_999)?,
        balance(second, 1, -99_999_999_999)?,
        balance(second, 2, -99_999_999_999)?,
        balance(second, 3, -99_999_999_999)?,
        balance(second, 4, -99_999_999_999)?,
        balance(third, 1, 0)?,
    ];
    // ARITH-013's worked balance for the two accounts the run rewrites.
    let mut accounts = masters.accounts.clone();
    for account in &mut accounts {
        if account.id == first.acct_id || account.id == second.acct_id {
            account.curr_bal = money(84_300)?;
        }
    }
    let clock = FixedClock::new(instant(2022, 7, 18, 0)?);
    let mut run = Intcalc::new(ParmDate::DEFAULT, accounts, masters.xrefs.clone(), groups);
    let mut effects = Vec::with_capacity(records.len());
    let mut rewritten = Vec::new();
    for record in &records {
        let mut record_effects = Vec::new();
        for effect in run.process(record, &clock)? {
            match effect {
                IntcalcEffect::AccountUpdate(account) => {
                    record_effects.push(RecordEffect::Rewrite(account.id.get()));
                    rewritten.push((account.id.get(), account.curr_bal));
                }
                IntcalcEffect::SystemTransaction(transaction) => {
                    record_effects.push(RecordEffect::Systran(transaction.amt));
                }
            }
        }
        effects.push(record_effects);
    }
    let summary = run.finish();
    // Each monthly interest, and the SYSTRAN amount it writes, is 333324999.91 or -333324999.91
    // (ARITH-008); the 0.00 balance gives 0.00.
    let gain = RecordEffect::Systran(money(33_332_499_991)?);
    let loss = RecordEffect::Systran(money(-33_332_499_991)?);
    assert_eq!(
        effects,
        [
            vec![gain],
            vec![gain],
            vec![gain],
            vec![gain],
            vec![RecordEffect::Rewrite(first.acct_id.get()), loss],
            vec![loss],
            vec![loss],
            vec![loss],
            vec![
                RecordEffect::Rewrite(second.acct_id.get()),
                RecordEffect::Systran(Money::ZERO),
            ],
        ],
        "one SYSTRAN record per category balance, each break rewriting the account before it"
    );
    // The first total runs 333324999.91, 666649999.82, 999974999.73, then 1333299999.64, stored
    // 333299999.64: the high-order digit dropped. The second account's mirror is stored
    // -333299999.64, the sign kept. The unstored totals would give 1333300842.64 and
    // -1333299156.64, which fit `S9(10)V99` too, so the balances show the total's store.
    assert_eq!(
        rewritten,
        [
            (first.acct_id.get(), money::<10>(33_330_084_264)?),
            (second.acct_id.get(), money(-33_329_915_664)?),
        ],
        "843.00 plus each account's total stored into S9(09)V99"
    );
    assert_eq!(summary.unapplied_last_account(), Some(third.acct_id));
    assert_eq!(summary.unapplied_total(), Some(Money::ZERO));
    Ok(())
}

// audit: ARITH-009
// audit: ARITH-012
// audit: ARITH-014
// audit: ARITH-017
// audit: FIXINT-901
// audit: FIXINT-902
// audit: FIXINT-903
// audit: CBACT04C-010
// audit: CBACT04C-020
// audit: CBACT04C-022
#[test]
fn intcalc_over_the_loaded_balances_matches_interest_raw() -> TestResult {
    let masters = Masters::load()?;
    let purchase_rate = masters.default_rate(*b"01")?;
    assert_eq!(
        purchase_rate,
        rate(DEFAULT_PURCHASE_RATE)?,
        "DEFAULT 01/0001"
    );
    assert!(
        masters
            .accounts
            .iter()
            .all(|account| account.group_id.is_spaces()),
        "E07: every ACCT-GROUP-ID is blank, so every lookup takes DEFAULT"
    );
    let rows = INTEREST_RAW_ROWS
        .into_iter()
        .map(parse_raw_interest_row)
        .collect::<Result<Vec<_>, _>>()?;
    let run = run_intcalc(masters.accounts.clone(), &masters.tcatbals, &masters)?;
    assert_eq!(
        rows.len(),
        masters.tcatbals.len(),
        "one FIXINT row per record"
    );
    assert_eq!(
        run.systran.len(),
        rows.len(),
        "one SYSTRAN record per record"
    );
    let records = rows.iter().zip(&masters.tcatbals).zip(&run.systran);
    for ((row, tcatbal), (key, systran)) in records {
        let id = format!("FIXINT-{:03}", row.acct_id);
        assert_eq!(category_key(tcatbal), (row.acct_id, *b"01", 1), "{id}: key");
        assert_eq!(*key, category_key(tcatbal), "{id}: SYSTRAN record order");
        assert_eq!(tcatbal.bal, row.cat_bal, "{id}: loaded category balance");
        assert_eq!(
            monthly_interest(tcatbal.bal, purchase_rate),
            row.monthly,
            "{id}: monthly interest at the DEFAULT rate"
        );
        assert_eq!(systran.amt, row.monthly, "{id}: SYSTRAN amount");
        assert!(stamped(systran, TsForm::Db2), "{id}: SYSTRAN timestamps");
        let loaded = masters.account(row.acct_id)?;
        assert_eq!(loaded.curr_bal, row.bal_before, "{id}: loaded balance");
        check_rewrite(&id, row.bal_after, run.updates.get(&row.acct_id))?;
    }
    assert_eq!(run.summary.records_read(), u64::try_from(rows.len())?);
    assert_eq!(
        run.summary.transactions_written(),
        u64::try_from(rows.len())?
    );
    assert_eq!(
        run.summary.unapplied_last_account().map(PicU::get),
        Some(LAST_ACCOUNT),
        "E08: the last account is never rewritten"
    );
    assert_eq!(run.summary.unapplied_total(), Some(Money::ZERO));
    Ok(())
}

/// Checks that the tables `POSTED` leaves hold the inputs a `FIXINT` row of `INTEREST-POSTED`
/// starts from: both category balances of the account and the account itself.
fn check_posted_inputs(id: &str, row: &PostedInterestRow, posted: &PostedRun) -> TestResult {
    let balance = |type_cd: [u8; 2]| {
        posted
            .tcatbals
            .get(&(row.acct_id, type_cd, 1))
            .map(|record| record.bal)
            .ok_or_else(|| {
                let type_cd = String::from_utf8_lossy(&type_cd);
                format!("{id}: no {type_cd}/0001 balance after POSTED")
            })
    };
    assert_eq!(balance(*b"01")?, row.purchase_bal, "{id}: 01/0001 balance");
    assert_eq!(balance(*b"03")?, row.credit_bal, "{id}: 03/0001 balance");
    let account = posted
        .accounts
        .get(&row.acct_id)
        .ok_or_else(|| format!("{id}: no account after POSTED"))?;
    assert_eq!(account.curr_bal, row.bal_before, "{id}: balance");
    assert_eq!(account.curr_cyc_credit, row.cr_before, "{id}: cycle credit");
    assert_eq!(account.curr_cyc_debit, row.db_before, "{id}: cycle debit");
    Ok(())
}

// audit: ARITH-001
// audit: ARITH-010
// audit: ARITH-012
// audit: ARITH-013
// audit: ARITH-014
// audit: ARITH-015
// audit: ARITH-017
// audit: FIXINT-903
// audit: CBACT04C-012
// audit: CBACT04C-020
#[test]
fn intcalc_over_the_posted_tables_matches_interest_posted() -> TestResult {
    let masters = Masters::load()?;
    let posted = run_posttran(&masters)?;
    let rows = INTEREST_POSTED_ROWS
        .into_iter()
        .map(parse_posted_interest_row)
        .collect::<Result<Vec<_>, _>>()?;
    let purchase_rate = masters.default_rate(*b"01")?;
    assert!(masters.default_rate(*b"03")?.is_zero(), "DEFAULT 03/0001");
    // The category balances in key order, as `intcalc` reads `tran_cat_balances`.
    let tcatbals: Vec<TranCatBalanceRecord> = posted.tcatbals.values().cloned().collect();
    let accounts: Vec<AccountRecord> = posted.accounts.values().cloned().collect();
    let run = run_intcalc(accounts, &tcatbals, &masters)?;
    assert_eq!(
        run.systran.len(),
        rows.len(),
        "one SYSTRAN record per account: the 03/0001 balances have rate 0.00"
    );
    let mut rewritten_totals = Money::<12>::ZERO;
    let mut systran_amounts = Money::<12>::ZERO;
    for (row, (key, systran)) in rows.iter().zip(&run.systran) {
        let id = format!("FIXINT-{:03}", row.acct_id);
        check_posted_inputs(&id, row, &posted)?;
        assert_eq!(*key, (row.acct_id, *b"01", 1), "{id}: SYSTRAN record key");
        assert_eq!(
            monthly_interest(row.purchase_bal, purchase_rate),
            row.monthly,
            "{id}: monthly interest at the DEFAULT rate"
        );
        assert_eq!(systran.amt, row.monthly, "{id}: SYSTRAN amount");
        assert!(stamped(systran, TsForm::Db2), "{id}: SYSTRAN timestamps");
        let update = run.updates.get(&row.acct_id);
        check_rewrite(&id, row.bal_after, update)?;
        if let Some(update) = update {
            rewritten_totals = (rewritten_totals + update.curr_bal - row.bal_before).store::<12>();
        }
        systran_amounts = (systran_amounts + systran.amt).store::<12>();
    }
    assert_eq!(rewritten_totals, amount::<12>(POSTED_REWRITTEN_TOTALS)?);
    assert_eq!(systran_amounts, amount::<12>(POSTED_SYSTRAN_AMOUNTS)?);
    let summary = run.summary;
    assert_eq!(summary.records_read(), u64::try_from(tcatbals.len())?);
    assert_eq!(summary.transactions_written(), u64::try_from(rows.len())?);
    assert_eq!(
        summary.unapplied_last_account().map(PicU::get),
        Some(LAST_ACCOUNT)
    );
    let last_monthly = rows
        .last()
        .map(|row| row.monthly)
        .ok_or("INTEREST_POSTED_ROWS is empty")?;
    assert_eq!(
        summary.unapplied_total(),
        Some(last_monthly),
        "E08: discarded"
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// CBTRN02C: posting (ARITH-023 to ARITH-033, FIXPOST)
// ---------------------------------------------------------------------------------------------

/// Checks one posting against its `FIXPOST` row: the category balance written, the account after
/// the posting, and the transaction, whose `TRAN-AMT` is `DALYTRAN-AMT` unchanged and whose
/// `TRAN-PROC-TS` is the clock's reading in layout `D`.
fn check_posting(
    id: &str,
    acct_id: u64,
    expected: &PostedResult,
    posting: &Posting,
    daily: &DailyTransactionRecord,
) {
    let kind = match posting.tcatbal {
        TcatbalWrite::Create(_) => TcatbalKind::Create,
        TcatbalWrite::Update(_) => TcatbalKind::Update,
    };
    assert_eq!(kind, expected.kind, "{id}: category balance write");
    let written = posting.tcatbal.record();
    assert_eq!(
        category_key(written),
        (acct_id, expected.type_cd, 1),
        "{id}: category balance key"
    );
    assert_eq!(written.bal, expected.cat_bal, "{id}: TRAN-CAT-BAL");
    let account = &posting.account;
    assert_eq!(account.id.get(), acct_id, "{id}: account");
    assert_eq!(account.curr_bal, expected.bal, "{id}: ACCT-CURR-BAL");
    assert_eq!(
        account.curr_cyc_credit, expected.cr,
        "{id}: ACCT-CURR-CYC-CREDIT"
    );
    assert_eq!(
        account.curr_cyc_debit, expected.db,
        "{id}: ACCT-CURR-CYC-DEBIT"
    );
    let transaction = &posting.transaction;
    assert_eq!(transaction.amt, daily.amt, "{id}: TRAN-AMT");
    assert_eq!(
        form(&transaction.orig_ts),
        Some(TsForm::Iso),
        "{id}: TRAN-ORIG-TS"
    );
    assert_eq!(
        form(&transaction.proc_ts),
        Some(TsForm::Db2),
        "{id}: TRAN-PROC-TS"
    );
}

// audit: ARITH-023
// audit: ARITH-026
// audit: ARITH-027
// audit: ARITH-028
// audit: ARITH-029
// audit: ARITH-031
// audit: ARITH-032
// audit: ARITH-033
// audit: CBTRN02C-004
// audit: CBTRN02C-007
// audit: CBTRN02C-013
// audit: CBTRN02C-022
#[test]
fn posttran_over_dailytran_matches_every_fixpost_row() -> TestResult {
    let masters = Masters::load()?;
    let run = run_posttran(&masters)?;
    let rows = POSTTRAN_ROWS
        .into_iter()
        .map(parse_post_row)
        .collect::<Result<Vec<_>, _>>()?;
    assert_eq!(rows.len(), run.daily.len(), "one FIXPOST row per record");
    assert_eq!(
        run.outcomes.len(),
        run.daily.len(),
        "one outcome per record"
    );
    // The accounts as each record finds them: every posting is visible to the next record.
    let mut accounts: BTreeMap<u64, AccountRecord> = masters
        .accounts
        .iter()
        .map(|account| (account.id.get(), account.clone()))
        .collect();
    let records = rows.iter().enumerate().zip(&run.daily).zip(&run.outcomes);
    for (((index, row), daily), outcome) in records {
        let id = format!("FIXPOST-{:03}", row.line);
        assert_eq!(row.line, index + 1, "{id}: rows follow the input order");
        assert_eq!(daily.amt, row.amt, "{id}: DALYTRAN-AMT");
        let xref = masters
            .xrefs
            .iter()
            .find(|xref| xref.card_num == daily.card_num)
            .ok_or_else(|| format!("{id}: the card is not cross-referenced"))?;
        assert_eq!(
            xref.acct_id.get(),
            row.acct_id,
            "{id}: cross-referenced account"
        );
        let before = accounts
            .get(&row.acct_id)
            .ok_or_else(|| format!("{id}: the account is not loaded"))?;
        assert_eq!(
            cbtrn02c::temp_balance(before, daily),
            row.temp,
            "{id}: WS-TEMP-BAL"
        );
        match (&row.expected, outcome) {
            (PostExpected::Posted(expected), RecordOutcome::Posted(posting)) => {
                check_posting(&id, row.acct_id, expected, posting, daily);
                accounts.insert(row.acct_id, posting.account.clone());
            }
            (PostExpected::Rejected(reason), RecordOutcome::Rejected(reject)) => {
                assert_eq!(reject.reason.get(), *reason, "{id}: reject reason");
            }
            (PostExpected::Posted(_), RecordOutcome::Rejected(_)) => {
                return Err(format!("{id}: rejected, where the audit posts it").into());
            }
            (PostExpected::Rejected(_), RecordOutcome::Posted(_)) => {
                return Err(format!("{id}: posted, where the audit rejects it").into());
            }
        }
    }
    Ok(())
}

// audit: FIXPOST-901
// audit: FIXPOST-902
// audit: FIXPOST-903
// audit: CBTRN02C-035
#[test]
fn posttran_counts_and_category_balances_match_the_fixpost_summaries() -> TestResult {
    let masters = Masters::load()?;
    let run = run_posttran(&masters)?;
    assert_eq!(run.summary.processed(), PROCESSED, "records read");
    assert_eq!(run.summary.posted(), POSTED, "records posted");
    assert_eq!(run.summary.rejected(), REJECTED, "records rejected");
    assert_eq!(
        run.summary.return_code(),
        POSTTRAN_RETURN_CODE,
        "return code"
    );
    let rows = POSTTRAN_ROWS
        .into_iter()
        .map(parse_post_row)
        .collect::<Result<Vec<_>, _>>()?;
    let overlimit = RejectReason::Overlimit.code().get();
    let reasons: Vec<u64> = rows
        .iter()
        .filter_map(|row| match row.expected {
            PostExpected::Rejected(reason) => Some(reason),
            PostExpected::Posted(_) => None,
        })
        .collect();
    assert_eq!(u64::try_from(reasons.len())?, REJECTED, "rejected rows");
    assert!(
        reasons.iter().all(|reason| *reason == overlimit),
        "every reject is 0102"
    );
    assert_eq!(
        run.tcatbals.len(),
        TCATBAL_ROWS_AFTER_POSTED,
        "category balances"
    );
    let loaded: BTreeSet<CategoryKey> = masters.tcatbals.iter().map(category_key).collect();
    assert!(
        run.updated.is_subset(&loaded),
        "only loaded rows are updated"
    );
    assert_eq!(
        run.updated.len(),
        LOADED_ROWS_UPDATED,
        "loaded rows updated"
    );
    let never_updated: Vec<CategoryKey> = loaded.difference(&run.updated).copied().collect();
    assert_eq!(
        never_updated,
        [(ACCOUNT_WITHOUT_POSTED_PURCHASE, *b"01", 1)],
        "the loaded row no posting reaches"
    );
    let created: Vec<CategoryKey> = run
        .tcatbals
        .keys()
        .filter(|key| !loaded.contains(*key))
        .copied()
        .collect();
    let one_per_account: Vec<CategoryKey> = (1..=u64::try_from(ROWS_CREATED)?)
        .map(|acct_id| (acct_id, *b"03", 1))
        .collect();
    assert_eq!(
        created, one_per_account,
        "one 03/0001 row created per account"
    );
    Ok(())
}

/// `COMPUTE WS-TEMP-BAL` worked cases: (`ACCT-CURR-CYC-CREDIT`, `ACCT-CURR-CYC-DEBIT`,
/// `DALYTRAN-AMT`, `WS-TEMP-BAL` as stored into `S9(09)V99`), in hundredths.
const TEMP_BALANCE_CASES: [(i64, i64, i64, i64); 4] = [
    // audit: ARITH-023
    (100_000, -25_000, 10_000, 135_000),
    // audit: ARITH-024
    (50_000, -20_000, -5_000, 65_000),
    // audit: ARITH-025
    (150_000_000_000, 0, 10_000, 50_000_010_000),
    // audit: ARITH-033
    (110_000, -30_000, 1_000, 141_000),
];

/// The record a limit test passes: the reason stays 0.
const VALID: Validation = Validation::Valid;

/// The record a limit test rejects with reason `0102`.
const OVERLIMIT: Validation = Validation::Rejected(RejectReason::Overlimit);

/// `IF ACCT-CREDIT-LIMIT >= WS-TEMP-BAL` worked cases: (`ACCT-CREDIT-LIMIT`,
/// `ACCT-CURR-CYC-CREDIT`, `ACCT-CURR-CYC-DEBIT`, `DALYTRAN-AMT`, the validation), in
/// hundredths.
const LIMIT_CASES: [(i64, i64, i64, i64, Validation); 3] = [
    // audit: ARITH-026
    (135_000, 100_000, -25_000, 10_000, VALID),
    // audit: ARITH-026
    (134_999, 100_000, -25_000, 10_000, OVERLIMIT),
    // audit: ARITH-025
    (60_000_000_000, 150_000_000_000, 0, 10_000, VALID),
];

/// A loaded account, its cross-reference and the first daily record, whose dates pass the
/// expiration test, as the base of the synthetic posting cases.
fn posting_base(
    masters: &Masters,
) -> Result<(AccountRecord, CardXrefRecord, DailyTransactionRecord), Box<dyn Error>> {
    let daily: Vec<DailyTransactionRecord> = fixture("dailytran.txt")?;
    let first = daily
        .into_iter()
        .next()
        .ok_or("dailytran.txt holds no record")?;
    Ok((masters.account(1)?.clone(), masters.xref(1)?.clone(), first))
}

// audit: CBTRN02C-006
// audit: CBTRN02C-009
#[test]
fn ws_temp_bal_drops_its_high_order_digit_before_the_limit_test() -> TestResult {
    let masters = Masters::load()?;
    let (account, xref, daily) = posting_base(&masters)?;
    for (cr, db, amt, expected) in TEMP_BALANCE_CASES {
        let account = AccountRecord {
            curr_cyc_credit: money(cr)?,
            curr_cyc_debit: money(db)?,
            ..account.clone()
        };
        let daily = DailyTransactionRecord {
            amt: money(amt)?,
            ..daily.clone()
        };
        assert_eq!(
            cbtrn02c::temp_balance(&account, &daily),
            money::<9>(expected)?,
            "credit {cr}, debit {db}, amount {amt} (hundredths)"
        );
    }
    for (limit, cr, db, amt, expected) in LIMIT_CASES {
        let account = AccountRecord {
            credit_limit: money(limit)?,
            curr_cyc_credit: money(cr)?,
            curr_cyc_debit: money(db)?,
            ..account.clone()
        };
        let daily = DailyTransactionRecord {
            amt: money(amt)?,
            ..daily.clone()
        };
        assert_eq!(
            cbtrn02c::validate(&daily, Some(&xref), Some(&account)),
            expected,
            "limit {limit}, credit {cr}, debit {db}, amount {amt} (hundredths)"
        );
    }
    Ok(())
}

/// `2700-A-CREATE-TCATBAL-REC` worked cases: (`DALYTRAN-AMT`, the `TRAN-CAT-BAL` of the new row),
/// in hundredths.
const CREATED_BALANCE_CASES: [(i64, i64); 2] = [
    // audit: ARITH-028
    (5_000, 5_000),
    // audit: ARITH-028
    (-1_234, -1_234),
];

/// `2700-B-UPDATE-TCATBAL-REC` worked cases: (`TRAN-CAT-BAL`, `DALYTRAN-AMT`, the balance stored
/// into `S9(09)V99`), in hundredths.
const UPDATED_BALANCE_CASES: [(i64, i64, i64); 4] = [
    // audit: ARITH-029
    (10_000, 2_550, 12_550),
    // audit: ARITH-029
    (10_000, -13_000, -3_000),
    // audit: ARITH-030
    (99_999_999_999, 1, 0),
    // audit: ARITH-030
    (-99_999_999_999, -100, -99),
];

// audit: CBTRN02C-017
// audit: CBTRN02C-019
#[test]
fn category_balances_store_into_s9_09_v99() -> TestResult {
    let masters = Masters::load()?;
    let (account, _, daily) = posting_base(&masters)?;
    for (amt, expected) in CREATED_BALANCE_CASES {
        let daily = DailyTransactionRecord {
            amt: money(amt)?,
            ..daily.clone()
        };
        let created = cbtrn02c::new_category_balance(account.id, &daily, None);
        assert_eq!(
            created.bal,
            money::<9>(expected)?,
            "amount {amt} (hundredths)"
        );
    }
    let loaded = masters
        .tcatbals
        .first()
        .ok_or("tcatbal.txt holds no record")?;
    for (bal, amt, expected) in UPDATED_BALANCE_CASES {
        let record = TranCatBalanceRecord {
            bal: money(bal)?,
            ..loaded.clone()
        };
        let updated = cbtrn02c::add_to_category_balance(record, money(amt)?);
        assert_eq!(
            updated.bal,
            money::<9>(expected)?,
            "balance {bal} + amount {amt} (hundredths)"
        );
    }
    Ok(())
}

/// `ADD DALYTRAN-AMT TO ACCT-CURR-BAL` worked cases: (`ACCT-CURR-BAL`, `DALYTRAN-AMT`, the
/// balance stored into `S9(10)V99`), in hundredths.
const POSTED_BALANCE_CASES: [(i64, i64, i64); 3] = [
    // audit: ARITH-031
    (84_300, 10_000, 94_300),
    // audit: ARITH-031
    (0, -5_000, -5_000),
    // audit: ARITH-031
    (999_999_999_999, 1, 0),
];

/// `ADD DALYTRAN-AMT TO ACCT-CURR-CYC-CREDIT` worked cases, for an amount of 0.00 or more:
/// (credit, amount, stored credit), in hundredths; the cycle debit is unchanged.
const CYCLE_CREDIT_CASES: [(i64, i64, i64); 2] = [
    // audit: ARITH-032
    (100_000, 10_000, 110_000),
    // audit: ARITH-032
    (100_000, 0, 100_000),
];

/// `ADD DALYTRAN-AMT TO ACCT-CURR-CYC-DEBIT` worked cases, for an amount below 0.00:
/// (debit, amount, stored debit), in hundredths; the cycle credit is unchanged.
const CYCLE_DEBIT_CASES: [(i64, i64, i64); 2] = [
    // audit: ARITH-033
    (0, -5_000, -5_000),
    // audit: ARITH-033
    (-25_000, -5_000, -30_000),
];

// audit: CBTRN02C-020
// audit: CBTRN02C-021
#[test]
fn posting_updates_the_balance_and_one_cycle_field_into_s9_10_v99() -> TestResult {
    let masters = Masters::load()?;
    let loaded = masters.account(1)?;
    for (bal, amt, expected) in POSTED_BALANCE_CASES {
        let account = AccountRecord {
            curr_bal: money(bal)?,
            ..loaded.clone()
        };
        let posted = cbtrn02c::update_account(account, money(amt)?);
        assert_eq!(
            posted.curr_bal,
            money::<10>(expected)?,
            "balance {bal} + amount {amt} (hundredths)"
        );
    }
    for (cr, amt, expected) in CYCLE_CREDIT_CASES {
        let account = AccountRecord {
            curr_cyc_credit: money(cr)?,
            curr_cyc_debit: money(-25_000)?,
            ..loaded.clone()
        };
        let posted = cbtrn02c::update_account(account.clone(), money(amt)?);
        let credit = money::<10>(expected)?;
        assert_eq!(posted.curr_cyc_credit, credit, "credit {cr} + amount {amt}");
        assert_eq!(
            posted.curr_cyc_debit, account.curr_cyc_debit,
            "debit unchanged"
        );
    }
    for (db, amt, expected) in CYCLE_DEBIT_CASES {
        let account = AccountRecord {
            curr_cyc_credit: money(100_000)?,
            curr_cyc_debit: money(db)?,
            ..loaded.clone()
        };
        let posted = cbtrn02c::update_account(account.clone(), money(amt)?);
        let debit = money::<10>(expected)?;
        assert_eq!(posted.curr_cyc_debit, debit, "debit {db} + amount {amt}");
        assert_eq!(
            posted.curr_cyc_credit, account.curr_cyc_credit,
            "credit unchanged"
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// COBIL00C: bill payment (ARITH-018 to ARITH-022)
// ---------------------------------------------------------------------------------------------

/// The new transaction ID every payment here is written under; the amounts do not depend on it.
const NEW_TRAN_ID: [u8; 16] = *b"0000000000000301";

/// The bill-payment message of a balance of zero or less (COBIL00C-018).
const NOTHING_TO_PAY: &str = "You have nothing to pay...";

/// The `FixedClock` instant of the payments, the `POSTED` instant of `fixtures.md`.
fn payment_clock() -> Result<FixedClock, Box<dyn Error>> {
    Ok(FixedClock::new(instant(2022, 7, 1, 12)?))
}

/// The upper bound of the balances ARITH-020 pays in full: 1000000000.00.
fn whole_balance_bound() -> Result<Money<10>, MoneyError> {
    money(100_000_000_000)
}

// audit: ARITH-020
// audit: ARITH-022
// audit: COBIL00C-020
// audit: COBIL00C-026
// audit: COBIL00C-027
// audit: COBIL00C-031
#[test]
fn every_fixture_balance_is_paid_in_full() -> TestResult {
    let masters = Masters::load()?;
    let clock = payment_clock()?;
    let bound = whole_balance_bound()?;
    assert_eq!(masters.accounts.len(), 50, "acctdata.txt accounts");
    for account in &masters.accounts {
        let acct_id = account.id.get();
        assert!(
            account.curr_bal.is_positive() && account.curr_bal < bound,
            "account {acct_id}: every fixture balance lies above 0.00 and below 1000000000.00"
        );
        assert_eq!(
            cobil00c::decide(ReadIntent::Pay, account),
            Ok(BillPayDecision::Pay),
            "account {acct_id}: confirmation Y"
        );
        let xref = masters.xref(acct_id)?;
        let new_id = PicX::from_array(NEW_TRAN_ID);
        let effects = cobil00c::payment(new_id, None, account, Some(xref), &clock);
        let transaction = &effects.transaction;
        assert_eq!(
            transaction.amt, account.curr_bal,
            "account {acct_id}: TRAN-AMT"
        );
        assert!(
            transaction.card_num == xref.card_num,
            "account {acct_id}: the cross-referenced card"
        );
        assert!(
            stamped(transaction, TsForm::Iso),
            "account {acct_id}: timestamps"
        );
        let paid = AccountRecord {
            curr_bal: Money::ZERO,
            ..account.clone()
        };
        assert_eq!(
            effects.account, paid,
            "account {acct_id}: only the balance changes"
        );
    }
    Ok(())
}

/// `MOVE ACCT-CURR-BAL TO TRAN-AMT` and `COMPUTE ACCT-CURR-BAL = ACCT-CURR-BAL - TRAN-AMT`
/// worked cases: (`ACCT-CURR-BAL`, `TRAN-AMT`, the balance after the payment), in hundredths.
const PAYMENT_CASES: [(i64, i64, i64); 4] = [
    // audit: ARITH-020
    (84_300, 84_300, 0),
    // audit: ARITH-021
    (123_456_789_012, 23_456_789_012, 100_000_000_000),
    // audit: ARITH-021
    (999_999_999_999, 99_999_999_999, 900_000_000_000),
    // audit: ARITH-021
    (100_000_000_000, 0, 100_000_000_000),
];

// audit: ARITH-022
// audit: COBIL00C-026
#[test]
fn a_payment_drops_the_high_order_digit_of_the_balance() -> TestResult {
    let masters = Masters::load()?;
    let clock = payment_clock()?;
    let loaded = masters.account(1)?;
    let xref = masters.xref(1)?;
    for (bal, tran_amt, after) in PAYMENT_CASES {
        let account = AccountRecord {
            curr_bal: money(bal)?,
            ..loaded.clone()
        };
        assert_eq!(
            cobil00c::decide(ReadIntent::Pay, &account),
            Ok(BillPayDecision::Pay),
            "balance {bal}"
        );
        let new_id = PicX::from_array(NEW_TRAN_ID);
        let effects = cobil00c::payment(new_id, None, &account, Some(xref), &clock);
        assert_eq!(
            effects.transaction.amt,
            money::<9>(tran_amt)?,
            "balance {bal}: TRAN-AMT (hundredths)"
        );
        let debited = AccountRecord {
            curr_bal: money(after)?,
            ..account
        };
        assert_eq!(effects.account, debited, "balance {bal}: balance after");
    }
    Ok(())
}

/// Balances `PROCESS-ENTER-KEY` refuses to pay or show, in hundredths.
const NOTHING_TO_PAY_BALANCES: [i64; 2] = [0, -2_500];

// audit: ARITH-018
// audit: COBIL00C-018
#[test]
fn a_balance_of_zero_or_less_has_nothing_to_pay() -> TestResult {
    let masters = Masters::load()?;
    let loaded = masters.account(1)?;
    for bal in NOTHING_TO_PAY_BALANCES {
        let account = AccountRecord {
            curr_bal: money(bal)?,
            ..loaded.clone()
        };
        for intent in [ReadIntent::Pay, ReadIntent::ShowBalance] {
            let Err(failure) = cobil00c::decide(intent, &account) else {
                return Err(format!("balance {bal}: {intent:?} is not refused").into());
            };
            assert_eq!(
                failure.message(),
                NOTHING_TO_PAY,
                "balance {bal}: {intent:?}"
            );
            assert_eq!(failure.field(), BillPayField::AcctId, "balance {bal}");
            assert_eq!(failure.class(), FailureClass::Input, "balance {bal}");
            assert_eq!(failure.class().http_status(), 400, "balance {bal}");
        }
    }
    Ok(())
}

/// `MOVE ACCT-CURR-BAL TO WS-CURR-BAL` worked cases: (`ACCT-CURR-BAL` in hundredths, the
/// `PIC +9999999999.99` text).
const BALANCE_DISPLAY_CASES: [(i64, &str); 4] = [
    // audit: ARITH-019
    (84_300, "+0000000843.00"),
    // audit: ARITH-019
    (0, "+0000000000.00"),
    // audit: ARITH-019
    (-2_500, "-0000000025.00"),
    // audit: ARITH-019
    (123_456_789_012, "+1234567890.12"),
];

// audit: COBIL00C-017
// audit: COBIL00C-019
#[test]
fn the_shown_balance_keeps_every_digit_and_its_sign() -> TestResult {
    for (bal, shown) in BALANCE_DISPLAY_CASES {
        assert_eq!(
            cobil00c::balance_display(money(bal)?),
            shown,
            "balance {bal}"
        );
    }
    let masters = Masters::load()?;
    let account = AccountRecord {
        curr_bal: money(84_300)?,
        ..masters.account(1)?.clone()
    };
    let Ok(BillPayDecision::ShowBalance(outcome)) =
        cobil00c::decide(ReadIntent::ShowBalance, &account)
    else {
        return Err("a positive balance with a blank confirmation is not shown".into());
    };
    assert_eq!(outcome.balance(), Some("+0000000843.00"));
    Ok(())
}
