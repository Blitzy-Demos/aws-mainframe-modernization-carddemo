//! The rows of `modern/audit/batch/*.md` and the fixture scenarios of `modern/audit/fixtures.md`
//! whose outcome a pure `domain::batch` function decides: binding g3b on the domain side and the
//! rule basis of i9 (AAP 0.9.1).
//!
//! One module per program. Each test carries an `// audit:` tag for every row it proves. Store
//! writes, generation file names and exit statuses belong to `crates/batch/tests`, and money
//! results to `financial_parity.rs`; neither is repeated here. A `HALTED(Hn)` path, or a value a
//! halt candidate blocks, may run for coverage but no value it produces is asserted (AAP 0.8.5).
//!
//! The fixtures and `DUSRSECJ.jcl` are read in place and never copied. Reject records, posted
//! transactions and statements hold full card numbers, so byte comparisons use `assert!` with a
//! message naming the rule ID and the input line, never `assert_eq!`, which would print them
//! (Cardholder Data Redaction).

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime};
use copybook::{FixedRecord, lines};
use domain::FixedClock;
use domain::batch::cbtrn02c::{Posting, Posttran, PosttranSummary, RecordOutcome};
use domain::batch::loads::decode_fixture;
use domain::money::{Decimal, Money};
use domain::records::{
    AccountRecord, CardXrefRecord, DailyTransactionRecord, TranCatBalanceRecord, TransactionRecord,
};

type TestResult = Result<(), Box<dyn Error>>;

type Outcome<T> = Result<T, Box<dyn Error>>;

/// A category-balance key as plain values: account ID, type code and category.
type TcatKey = (u64, [u8; 2], u64);

/// `modern/crates/domain`, against which the fixtures and the audit are found.
fn package_dir() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// A file of `app/data/ASCII/`, read in place.
fn ascii(file: &str) -> Outcome<Vec<u8>> {
    Ok(fs::read(
        package_dir().join("../../../app/data/ASCII").join(file),
    )?)
}

/// `app/jcl/DUSRSECJ.jcl`, read in place.
fn dusrsecj_jcl() -> Outcome<Vec<u8>> {
    let path: PathBuf = package_dir().join("../../../app/jcl/DUSRSECJ.jcl");
    Ok(fs::read(path)?)
}

/// A file of `modern/audit/`.
fn audit(file: &str) -> Outcome<String> {
    Ok(fs::read_to_string(
        package_dir().join("../../audit").join(file),
    )?)
}

/// The records of a fixture, decoded by codec A as the loads decode them.
fn fixture<R: FixedRecord>(file: &str) -> Outcome<Vec<R>> {
    Ok(decode_fixture::<R>(&ascii(file)?)?
        .into_iter()
        .map(|framed| framed.record)
        .collect())
}

/// An amount of `cents` hundredths.
fn money<const N: u32>(cents: i64) -> Outcome<Money<N>> {
    Ok(Money::new(Decimal::new(cents, 2))?)
}

/// The instant `date` at `time`.
fn instant(date: (i32, u32, u32), time: (u32, u32, u32)) -> Outcome<NaiveDateTime> {
    let (year, month, day) = date;
    let (hour, minute, second) = time;
    NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|day| day.and_hms_opt(hour, minute, second))
        .ok_or_else(|| "a valid instant".into())
}

/// `text` left-justified in `N` bytes and padded with spaces, as the audit quotes records with
/// their trailing spaces omitted.
fn padded<const N: usize>(text: &[u8]) -> [u8; N] {
    let mut record = [b' '; N];
    for (slot, &byte) in record.iter_mut().zip(text) {
        *slot = byte;
    }
    record
}

/// The plain key of a category balance.
fn tcat_key(record: &TranCatBalanceRecord) -> TcatKey {
    (
        record.acct_id.get(),
        *record.type_cd.as_bytes(),
        record.cd.get(),
    )
}

/// The `POSTED` scenario of `fixtures.md`: `posttran` over the 300 `dailytran.txt` records in
/// input order, against the masters `acctdata.txt`, `cardxref.txt` and `tcatbal.txt`, under the
/// `FixedClock` instant 2022-07-01 12:00:00.
struct Posted {
    /// The decoded input records, line n at index n - 1.
    daily: Vec<DailyTransactionRecord>,
    /// The input line bodies, terminators excluded.
    bodies: Vec<Vec<u8>>,
    /// One outcome per input record, in input order.
    outcomes: Vec<RecordOutcome>,
    /// The run's counts.
    summary: PosttranSummary,
}

impl Posted {
    fn run() -> Outcome<Self> {
        let mut run = Posttran::new(
            fixture::<AccountRecord>("acctdata.txt")?,
            fixture::<CardXrefRecord>("cardxref.txt")?,
            fixture::<TranCatBalanceRecord>("tcatbal.txt")?,
        );
        let input = ascii("dailytran.txt")?;
        let bodies = lines(&input).map(|line| line.body.to_vec()).collect();
        let daily: Vec<DailyTransactionRecord> = decode_fixture::<DailyTransactionRecord>(&input)?
            .into_iter()
            .map(|framed| framed.record)
            .collect();
        let clock = FixedClock::new(instant((2022, 7, 1), (12, 0, 0))?);
        let outcomes = daily
            .iter()
            .map(|record| run.process(record, &clock))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            daily,
            bodies,
            outcomes,
            summary: run.finish(),
        })
    }

    /// The postings, in input order.
    fn postings(&self) -> impl Iterator<Item = &Posting> {
        self.outcomes.iter().filter_map(|outcome| match outcome {
            RecordOutcome::Posted(posting) => Some(posting.as_ref()),
            RecordOutcome::Rejected(_) => None,
        })
    }

    /// The `transactions` rows the run inserts, in input order.
    fn transactions(&self) -> Vec<&TransactionRecord> {
        self.postings()
            .map(|posting| &posting.transaction)
            .collect()
    }

    /// The accounts as the run leaves them, by account ID.
    fn accounts(&self) -> Outcome<BTreeMap<u64, AccountRecord>> {
        let mut accounts: BTreeMap<u64, AccountRecord> = fixture::<AccountRecord>("acctdata.txt")?
            .into_iter()
            .map(|account| (account.id.get(), account))
            .collect();
        for posting in self.postings() {
            accounts.insert(posting.account.id.get(), posting.account.clone());
        }
        Ok(accounts)
    }

    /// The category balances as the run leaves them, in key order.
    fn tcatbals(&self) -> Outcome<BTreeMap<TcatKey, TranCatBalanceRecord>> {
        let mut tcatbals: BTreeMap<TcatKey, TranCatBalanceRecord> =
            fixture::<TranCatBalanceRecord>("tcatbal.txt")?
                .into_iter()
                .map(|record| (tcat_key(&record), record))
                .collect();
        for posting in self.postings() {
            let record = posting.tcatbal.record();
            tcatbals.insert(tcat_key(record), record.clone());
        }
        Ok(tcatbals)
    }

    /// `{T:n}` of the audit: the 16 bytes of `DALYTRAN-ID` on `dailytran.txt:n`.
    fn tran_id(&self, line: usize) -> Outcome<&[u8; 16]> {
        let record = line
            .checked_sub(1)
            .and_then(|index| self.daily.get(index))
            .ok_or("{T:n} names a dailytran.txt line")?;
        Ok(record.id.as_bytes())
    }

    /// An audit quote with every `{T:n}` replaced by the transaction ID it stands for.
    fn expand(&self, quote: &str) -> Outcome<Vec<u8>> {
        let mut out = Vec::with_capacity(quote.len());
        let mut rest = quote;
        while let Some((head, tail)) = rest.split_once("{T:") {
            out.extend_from_slice(head.as_bytes());
            let (number, after) = tail.split_once('}').ok_or("a closed {T:n}")?;
            out.extend_from_slice(self.tran_id(number.parse()?)?);
            rest = after;
        }
        out.extend_from_slice(rest.as_bytes());
        Ok(out)
    }
}

/// POSTTRAN, `domain::batch::cbtrn02c` (`modern/audit/batch/POSTTRAN.md`, FIXPOST rows).
mod posttran {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::NaiveDateTime;
    use copybook::{FixedRecord, PicDate, PicTimestamp, PicU, PicX, TsForm};
    use domain::batch::cbtrn02c::{
        END_LINE, PROCESSED_LABEL, Posttran, REJECTED_LABEL, REJECTS_RETURN_CODE, RecordOutcome,
        RejectReason, START_LINE, TCATBAL_NOT_FOUND_PREFIX, TCATBAL_NOT_FOUND_SUFFIX, TcatbalWrite,
        Validation, reject_record, validate,
    };
    use domain::records::{
        AccountRecord, CardXrefRecord, DailyTransactionRecord, TranCatBalanceRecord,
    };
    use domain::{CardNumber, Clock};

    use super::{Outcome, Posted, TcatKey, TestResult, audit, instant, money, padded, tcat_key};

    /// A card number on no fixture line, so no synthetic record holds a fixture card number.
    const CARD: [u8; 16] = *b"4000002000003000";

    /// `DALYTRAN-ORIG-TS` of every fixture record, layout `I` (FIXFRM-017).
    const RECEIVED: &[u8; 26] = b"2022-06-10 19:27:53.000000";

    /// A blank `DALYTRAN-ORIG-TS`.
    const BLANK_TS: &[u8; 26] = &[b' '; 26];

    /// The account every synthetic record posts to.
    const ACCT: u64 = 7;

    /// The lines FIXPOST-902 lists as rejected, in input order.
    const REJECTED_LINES: [usize; 38] = [
        16, 32, 35, 43, 50, 66, 79, 89, 101, 129, 155, 158, 159, 162, 173, 176, 178, 181, 190, 208,
        223, 224, 225, 227, 231, 234, 235, 240, 242, 253, 267, 271, 272, 273, 280, 283, 292, 299,
    ];

    fn date(text: &[u8; 10]) -> Outcome<PicDate> {
        Ok(PicDate::decode(text)?)
    }

    /// A daily transaction of type `01` for [`CARD`].
    fn daily(amt_cents: i64, orig_ts: &[u8; 26]) -> Outcome<DailyTransactionRecord> {
        Ok(DailyTransactionRecord {
            id: PicX::from_array(*b"T000000000000001"),
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            source: PicX::from_array(*b"POS TERM  "),
            desc: PicX::moved_from(b"Purchase at a test merchant"),
            amt: money(amt_cents)?,
            merchant_id: PicU::new(800_000_001)?,
            merchant_name: PicX::moved_from(b"Test Merchant"),
            merchant_city: PicX::moved_from(b"Test City"),
            merchant_zip: PicX::moved_from(b"12345"),
            card_num: CardNumber::new(PicX::from_array(CARD)),
            orig_ts: PicTimestamp::decode(orig_ts)?,
            proc_ts: PicTimestamp::BLANK,
            filler: PicX::spaces(),
        })
    }

    /// Account [`ACCT`] with the given limit, cycle fields and expiration date.
    fn account(
        limit_cents: i64,
        credit_cents: i64,
        debit_cents: i64,
        expiraion_date: PicDate,
    ) -> Outcome<AccountRecord> {
        Ok(AccountRecord {
            id: PicU::new(ACCT)?,
            active_status: PicX::from_array(*b"Y"),
            curr_bal: money(19_300)?,
            credit_limit: money(limit_cents)?,
            cash_credit_limit: money(100_000)?,
            open_date: date(b"2014-11-20")?,
            expiraion_date,
            reissue_date: date(b"2024-05-21")?,
            curr_cyc_credit: money(credit_cents)?,
            curr_cyc_debit: money(debit_cents)?,
            addr_zip: PicX::from_array(*b"A000000000"),
            group_id: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// The cross-reference of [`CARD`] to [`ACCT`].
    fn xref() -> Outcome<CardXrefRecord> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(CARD)),
            cust_id: PicU::new(ACCT)?,
            acct_id: PicU::new(ACCT)?,
            filler: PicX::spaces(),
        })
    }

    // audit: CBTRN02C-004
    // audit: CBTRN02C-005
    // audit: CBTRN02C-007
    // audit: CBTRN02C-008
    // audit: CBTRN02C-027
    #[test]
    fn each_reason_carries_its_code_and_its_padded_description() -> TestResult {
        let cases = [
            (
                RejectReason::InvalidCard,
                b"0100",
                "INVALID CARD NUMBER FOUND",
            ),
            (
                RejectReason::AccountNotFound,
                b"0101",
                "ACCOUNT RECORD NOT FOUND",
            ),
            (RejectReason::Overlimit, b"0102", "OVERLIMIT TRANSACTION"),
            (
                RejectReason::AfterExpiration,
                b"0103",
                "TRANSACTION RECEIVED AFTER ACCT EXPIRATION",
            ),
        ];
        let record = daily(50_000, RECEIVED)?;
        for (reason, code, text) in cases {
            assert_eq!(reason.text(), text);
            assert_eq!(
                *reason.description().as_bytes(),
                padded::<76>(text.as_bytes()),
                "{text}"
            );
            let encoded = reject_record(&record, reason).encode();
            assert!(encoded.len() == 430, "{text}: 430-byte reject record");
            assert!(
                encoded.get(..350) == Some(record.encode().as_slice()),
                "{text}: the 350 input bytes"
            );
            assert!(
                encoded.get(350..354) == Some(code.as_slice()),
                "{text}: reason digits"
            );
            assert!(
                encoded.get(354..) == Some(padded::<76>(text.as_bytes()).as_slice()),
                "{text}: description"
            );
        }
        Ok(())
    }

    // audit: CBTRN02C-003
    // audit: CBTRN02C-004
    // audit: CBTRN02C-005
    // audit: CBTRN02C-007
    // audit: CBTRN02C-008
    // audit: CBTRN02C-009
    #[test]
    fn validation_runs_in_program_order_and_the_expiry_overwrites_the_limit() -> TestResult {
        let record = daily(50_000, RECEIVED)?;
        let xref = xref()?;
        let roomy = account(1_000_000, 0, 0, date(b"2025-12-31")?)?;
        let tight = account(10_000, 0, 0, date(b"2025-12-31")?)?;
        let expired = account(1_000_000, 0, 0, date(b"2022-06-09")?)?;
        let both = account(10_000, 0, 0, date(b"2022-06-09")?)?;
        let at_limit = account(50_000, 0, 0, date(b"2022-06-10")?)?;
        let rejected = Validation::Rejected;
        let cases: [(
            &str,
            Option<&CardXrefRecord>,
            Option<&AccountRecord>,
            Validation,
        ); 7] = [
            (
                "card not in xref",
                None,
                Some(&roomy),
                rejected(RejectReason::InvalidCard),
            ),
            (
                "account missing",
                Some(&xref),
                None,
                rejected(RejectReason::AccountNotFound),
            ),
            (
                "overlimit only",
                Some(&xref),
                Some(&tight),
                rejected(RejectReason::Overlimit),
            ),
            (
                "expired only",
                Some(&xref),
                Some(&expired),
                rejected(RejectReason::AfterExpiration),
            ),
            (
                "overlimit and expired (E09)",
                Some(&xref),
                Some(&both),
                rejected(RejectReason::AfterExpiration),
            ),
            (
                "limit and date equal",
                Some(&xref),
                Some(&at_limit),
                Validation::Valid,
            ),
            (
                "within limit and date",
                Some(&xref),
                Some(&roomy),
                Validation::Valid,
            ),
        ];
        for (case, xref, account, expected) in cases {
            assert_eq!(validate(&record, xref, account), expected, "{case}");
        }
        Ok(())
    }

    // audit: CBTRN02C-006
    #[test]
    fn the_limit_test_reads_the_temp_balance_cut_to_nine_integer_digits() -> TestResult {
        let record = daily(10_000, RECEIVED)?;
        let xref = xref()?;
        let expiry = date(b"2025-12-31")?;
        // A cycle credit of 1,500,000,000.00 plus 100.00 stores 500,000,100.00 (E10), which a
        // limit of 600,000,000.00 covers and one of 400,000,000.00 does not.
        let covers = account(60_000_000_000, 150_000_000_000, 0, expiry)?;
        let short = account(40_000_000_000, 150_000_000_000, 0, expiry)?;
        assert_eq!(
            validate(&record, Some(&xref), Some(&covers)),
            Validation::Valid
        );
        assert_eq!(
            validate(&record, Some(&xref), Some(&short)),
            Validation::Rejected(RejectReason::Overlimit)
        );
        // A negative cycle debit of -900.00 adds its magnitude: 0.00 + 900.00 + 100.00 = 1000.00.
        let debited = account(99_999, 0, -90_000, expiry)?;
        assert_eq!(
            validate(&record, Some(&xref), Some(&debited)),
            Validation::Rejected(RejectReason::Overlimit)
        );
        Ok(())
    }

    // audit: CBTRN02C-010
    // audit: CBTRN02C-011
    #[test]
    fn blank_dates_sort_below_every_date_and_nothing_else_is_validated() -> TestResult {
        let xref = xref()?;
        let dated = daily(50_000, RECEIVED)?;
        let undated = daily(50_000, BLANK_TS)?;
        let blank_expiry = account(1_000_000, 0, 0, PicDate::BLANK)?;
        let expiry = account(1_000_000, 0, 0, date(b"2022-06-10")?)?;
        assert_eq!(
            validate(&dated, Some(&xref), Some(&blank_expiry)),
            Validation::Rejected(RejectReason::AfterExpiration)
        );
        assert_eq!(
            validate(&undated, Some(&xref), Some(&expiry)),
            Validation::Valid
        );
        assert_eq!(
            validate(&undated, Some(&xref), Some(&blank_expiry)),
            Validation::Valid
        );
        // An inactive account, an unknown type and category and a negative amount still post.
        let inactive = AccountRecord {
            active_status: PicX::from_array(*b"N"),
            ..expiry
        };
        let unknown = DailyTransactionRecord {
            type_cd: PicX::from_array(*b"99"),
            cat_cd: PicU::new(9_999)?,
            amt: money(-1)?,
            ..dated
        };
        assert_eq!(
            validate(&unknown, Some(&xref), Some(&inactive)),
            Validation::Valid
        );
        Ok(())
    }

    /// A clock that counts its readings.
    struct CountingClock {
        at: NaiveDateTime,
        reads: AtomicUsize,
    }

    impl Clock for CountingClock {
        fn now(&self) -> NaiveDateTime {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.at
        }
    }

    // audit: CBTRN02C-002
    // audit: CBTRN02C-013
    // audit: CBTRN02C-022
    // audit: CBTRN02C-026
    // audit: CBTRN02C-035
    #[test]
    fn a_reject_reads_no_clock_and_a_later_record_sees_an_earlier_posting() -> TestResult {
        let clock = CountingClock {
            at: instant((2022, 7, 1), (12, 0, 0))?,
            reads: AtomicUsize::new(0),
        };
        let limit = account(60_000, 0, 0, date(b"2025-12-31")?)?;
        let mut run = Posttran::new([limit], [xref()?], Vec::<TranCatBalanceRecord>::new());
        let first = run.process(&daily(50_000, RECEIVED)?, &clock)?;
        assert!(
            matches!(first, RecordOutcome::Posted(_)),
            "500.00 within 600.00"
        );
        assert_eq!(
            clock.reads.load(Ordering::SeqCst),
            1,
            "one reading per posting"
        );
        // The first posting left a cycle credit of 500.00, so 500.00 more exceeds the limit.
        let second = run.process(&daily(50_000, RECEIVED)?, &clock)?;
        let RecordOutcome::Rejected(reject) = second else {
            return Err("the second record is rejected".into());
        };
        assert_eq!(reject.reason.get(), 102);
        assert_eq!(
            clock.reads.load(Ordering::SeqCst),
            1,
            "a reject reads no clock"
        );
        let summary = run.finish();
        assert_eq!((summary.processed(), summary.rejected()), (2, 1));
        assert_eq!(summary.return_code(), REJECTS_RETURN_CODE);
        Ok(())
    }

    // audit: CBTRN02C-029
    // audit: CBTRN02C-034
    // audit: CBTRN02C-035
    #[test]
    fn an_empty_run_counts_nothing_and_returns_zero() {
        let run = Posttran::new(
            Vec::<AccountRecord>::new(),
            Vec::<CardXrefRecord>::new(),
            Vec::<TranCatBalanceRecord>::new(),
        );
        let summary = run.finish();
        assert_eq!(summary.return_code(), 0);
        assert_eq!(
            summary.sysout_lines(),
            [
                "TRANSACTIONS PROCESSED :000000000".to_owned(),
                "TRANSACTIONS REJECTED  :000000000".to_owned(),
                "END OF EXECUTION OF PROGRAM CBTRN02C".to_owned(),
            ]
        );
        assert_eq!(START_LINE, "START OF EXECUTION OF PROGRAM CBTRN02C");
        assert_eq!(END_LINE, "END OF EXECUTION OF PROGRAM CBTRN02C");
    }

    /// What a `FIXPOST-nnn` row of `fixtures.md` expects for its `dailytran.txt` line.
    #[derive(Debug, PartialEq, Eq)]
    enum Expected {
        /// Posted, its category balance at `key` created or updated.
        Posted { key: TcatKey, created: bool },
        /// Rejected with the four reason digits and the description.
        Rejected { code: [u8; 4], description: String },
    }

    /// The `Expected result` cell of `FIXPOST-<line>`.
    fn parse_expected(line: usize, cell: &str) -> Outcome<Expected> {
        if let Some(rest) = cell.strip_prefix("rejected `") {
            let (code, rest) = rest.split_once('`').ok_or("a quoted reason")?;
            let description = rest.split('`').nth(1).ok_or("a quoted description")?;
            return Ok(Expected::Rejected {
                code: <[u8; 4]>::try_from(code.as_bytes())?,
                description: description.to_owned(),
            });
        }
        let posted = format!("posted `{{T:{line}}}`; tcatbal (");
        let rest = cell
            .strip_prefix(posted.as_str())
            .ok_or_else(|| format!("FIXPOST-{line:03}: neither posted nor rejected"))?;
        let (key, rest) = rest.split_once(") ").ok_or("a category-balance key")?;
        let mut parts = key.split(", ");
        let acct_id = parts.next().ok_or("an account ID")?.parse()?;
        let type_cd = <[u8; 2]>::try_from(parts.next().ok_or("a type code")?.as_bytes())?;
        let cat_cd = parts.next().ok_or("a category")?.parse()?;
        let created = match rest.split(' ').next() {
            Some("create") => true,
            Some("update") => false,
            _ => return Err(format!("FIXPOST-{line:03}: neither create nor update").into()),
        };
        Ok(Expected::Posted {
            key: (acct_id, type_cd, cat_cd),
            created,
        })
    }

    /// The rows `FIXPOST-001` to `FIXPOST-300` of `fixtures.md`, by line.
    fn fixpost_rows() -> Outcome<BTreeMap<usize, Expected>> {
        let text = audit("fixtures.md")?;
        let mut rows = BTreeMap::new();
        for row in text.lines() {
            let Some(rest) = row.strip_prefix("| FIXPOST-") else {
                continue;
            };
            let line: usize = rest.get(..3).ok_or("a three-digit row number")?.parse()?;
            if line > 300 {
                continue;
            }
            // Header: Rule ID | Program | Paragraph | Copybook field or PIC | Input condition |
            // Expected result | Source lines | Fixture record.
            let cell = row.split(" | ").nth(5).ok_or("an Expected result cell")?;
            rows.insert(line, parse_expected(line, cell)?);
        }
        Ok(rows)
    }

    // audit: CBTRN02C-001
    // audit: CBTRN02C-016
    // audit: CBTRN02C-017
    // audit: CBTRN02C-019
    // audit: CBTRN02C-027
    // audit: FIXPOST-001
    // audit: FIXPOST-002
    // audit: FIXPOST-003
    // audit: FIXPOST-004
    // audit: FIXPOST-005
    // audit: FIXPOST-006
    // audit: FIXPOST-007
    // audit: FIXPOST-008
    // audit: FIXPOST-009
    // audit: FIXPOST-010
    // audit: FIXPOST-011
    // audit: FIXPOST-012
    // audit: FIXPOST-013
    // audit: FIXPOST-014
    // audit: FIXPOST-015
    // audit: FIXPOST-016
    // audit: FIXPOST-017
    // audit: FIXPOST-018
    // audit: FIXPOST-019
    // audit: FIXPOST-020
    // audit: FIXPOST-021
    // audit: FIXPOST-022
    // audit: FIXPOST-023
    // audit: FIXPOST-024
    // audit: FIXPOST-025
    // audit: FIXPOST-026
    // audit: FIXPOST-027
    // audit: FIXPOST-028
    // audit: FIXPOST-029
    // audit: FIXPOST-030
    // audit: FIXPOST-031
    // audit: FIXPOST-032
    // audit: FIXPOST-033
    // audit: FIXPOST-034
    // audit: FIXPOST-035
    // audit: FIXPOST-036
    // audit: FIXPOST-037
    // audit: FIXPOST-038
    // audit: FIXPOST-039
    // audit: FIXPOST-040
    // audit: FIXPOST-041
    // audit: FIXPOST-042
    // audit: FIXPOST-043
    // audit: FIXPOST-044
    // audit: FIXPOST-045
    // audit: FIXPOST-046
    // audit: FIXPOST-047
    // audit: FIXPOST-048
    // audit: FIXPOST-049
    // audit: FIXPOST-050
    // audit: FIXPOST-051
    // audit: FIXPOST-052
    // audit: FIXPOST-053
    // audit: FIXPOST-054
    // audit: FIXPOST-055
    // audit: FIXPOST-056
    // audit: FIXPOST-057
    // audit: FIXPOST-058
    // audit: FIXPOST-059
    // audit: FIXPOST-060
    // audit: FIXPOST-061
    // audit: FIXPOST-062
    // audit: FIXPOST-063
    // audit: FIXPOST-064
    // audit: FIXPOST-065
    // audit: FIXPOST-066
    // audit: FIXPOST-067
    // audit: FIXPOST-068
    // audit: FIXPOST-069
    // audit: FIXPOST-070
    // audit: FIXPOST-071
    // audit: FIXPOST-072
    // audit: FIXPOST-073
    // audit: FIXPOST-074
    // audit: FIXPOST-075
    // audit: FIXPOST-076
    // audit: FIXPOST-077
    // audit: FIXPOST-078
    // audit: FIXPOST-079
    // audit: FIXPOST-080
    // audit: FIXPOST-081
    // audit: FIXPOST-082
    // audit: FIXPOST-083
    // audit: FIXPOST-084
    // audit: FIXPOST-085
    // audit: FIXPOST-086
    // audit: FIXPOST-087
    // audit: FIXPOST-088
    // audit: FIXPOST-089
    // audit: FIXPOST-090
    // audit: FIXPOST-091
    // audit: FIXPOST-092
    // audit: FIXPOST-093
    // audit: FIXPOST-094
    // audit: FIXPOST-095
    // audit: FIXPOST-096
    // audit: FIXPOST-097
    // audit: FIXPOST-098
    // audit: FIXPOST-099
    // audit: FIXPOST-100
    // audit: FIXPOST-101
    // audit: FIXPOST-102
    // audit: FIXPOST-103
    // audit: FIXPOST-104
    // audit: FIXPOST-105
    // audit: FIXPOST-106
    // audit: FIXPOST-107
    // audit: FIXPOST-108
    // audit: FIXPOST-109
    // audit: FIXPOST-110
    // audit: FIXPOST-111
    // audit: FIXPOST-112
    // audit: FIXPOST-113
    // audit: FIXPOST-114
    // audit: FIXPOST-115
    // audit: FIXPOST-116
    // audit: FIXPOST-117
    // audit: FIXPOST-118
    // audit: FIXPOST-119
    // audit: FIXPOST-120
    // audit: FIXPOST-121
    // audit: FIXPOST-122
    // audit: FIXPOST-123
    // audit: FIXPOST-124
    // audit: FIXPOST-125
    // audit: FIXPOST-126
    // audit: FIXPOST-127
    // audit: FIXPOST-128
    // audit: FIXPOST-129
    // audit: FIXPOST-130
    // audit: FIXPOST-131
    // audit: FIXPOST-132
    // audit: FIXPOST-133
    // audit: FIXPOST-134
    // audit: FIXPOST-135
    // audit: FIXPOST-136
    // audit: FIXPOST-137
    // audit: FIXPOST-138
    // audit: FIXPOST-139
    // audit: FIXPOST-140
    // audit: FIXPOST-141
    // audit: FIXPOST-142
    // audit: FIXPOST-143
    // audit: FIXPOST-144
    // audit: FIXPOST-145
    // audit: FIXPOST-146
    // audit: FIXPOST-147
    // audit: FIXPOST-148
    // audit: FIXPOST-149
    // audit: FIXPOST-150
    // audit: FIXPOST-151
    // audit: FIXPOST-152
    // audit: FIXPOST-153
    // audit: FIXPOST-154
    // audit: FIXPOST-155
    // audit: FIXPOST-156
    // audit: FIXPOST-157
    // audit: FIXPOST-158
    // audit: FIXPOST-159
    // audit: FIXPOST-160
    // audit: FIXPOST-161
    // audit: FIXPOST-162
    // audit: FIXPOST-163
    // audit: FIXPOST-164
    // audit: FIXPOST-165
    // audit: FIXPOST-166
    // audit: FIXPOST-167
    // audit: FIXPOST-168
    // audit: FIXPOST-169
    // audit: FIXPOST-170
    // audit: FIXPOST-171
    // audit: FIXPOST-172
    // audit: FIXPOST-173
    // audit: FIXPOST-174
    // audit: FIXPOST-175
    // audit: FIXPOST-176
    // audit: FIXPOST-177
    // audit: FIXPOST-178
    // audit: FIXPOST-179
    // audit: FIXPOST-180
    // audit: FIXPOST-181
    // audit: FIXPOST-182
    // audit: FIXPOST-183
    // audit: FIXPOST-184
    // audit: FIXPOST-185
    // audit: FIXPOST-186
    // audit: FIXPOST-187
    // audit: FIXPOST-188
    // audit: FIXPOST-189
    // audit: FIXPOST-190
    // audit: FIXPOST-191
    // audit: FIXPOST-192
    // audit: FIXPOST-193
    // audit: FIXPOST-194
    // audit: FIXPOST-195
    // audit: FIXPOST-196
    // audit: FIXPOST-197
    // audit: FIXPOST-198
    // audit: FIXPOST-199
    // audit: FIXPOST-200
    // audit: FIXPOST-201
    // audit: FIXPOST-202
    // audit: FIXPOST-203
    // audit: FIXPOST-204
    // audit: FIXPOST-205
    // audit: FIXPOST-206
    // audit: FIXPOST-207
    // audit: FIXPOST-208
    // audit: FIXPOST-209
    // audit: FIXPOST-210
    // audit: FIXPOST-211
    // audit: FIXPOST-212
    // audit: FIXPOST-213
    // audit: FIXPOST-214
    // audit: FIXPOST-215
    // audit: FIXPOST-216
    // audit: FIXPOST-217
    // audit: FIXPOST-218
    // audit: FIXPOST-219
    // audit: FIXPOST-220
    // audit: FIXPOST-221
    // audit: FIXPOST-222
    // audit: FIXPOST-223
    // audit: FIXPOST-224
    // audit: FIXPOST-225
    // audit: FIXPOST-226
    // audit: FIXPOST-227
    // audit: FIXPOST-228
    // audit: FIXPOST-229
    // audit: FIXPOST-230
    // audit: FIXPOST-231
    // audit: FIXPOST-232
    // audit: FIXPOST-233
    // audit: FIXPOST-234
    // audit: FIXPOST-235
    // audit: FIXPOST-236
    // audit: FIXPOST-237
    // audit: FIXPOST-238
    // audit: FIXPOST-239
    // audit: FIXPOST-240
    // audit: FIXPOST-241
    // audit: FIXPOST-242
    // audit: FIXPOST-243
    // audit: FIXPOST-244
    // audit: FIXPOST-245
    // audit: FIXPOST-246
    // audit: FIXPOST-247
    // audit: FIXPOST-248
    // audit: FIXPOST-249
    // audit: FIXPOST-250
    // audit: FIXPOST-251
    // audit: FIXPOST-252
    // audit: FIXPOST-253
    // audit: FIXPOST-254
    // audit: FIXPOST-255
    // audit: FIXPOST-256
    // audit: FIXPOST-257
    // audit: FIXPOST-258
    // audit: FIXPOST-259
    // audit: FIXPOST-260
    // audit: FIXPOST-261
    // audit: FIXPOST-262
    // audit: FIXPOST-263
    // audit: FIXPOST-264
    // audit: FIXPOST-265
    // audit: FIXPOST-266
    // audit: FIXPOST-267
    // audit: FIXPOST-268
    // audit: FIXPOST-269
    // audit: FIXPOST-270
    // audit: FIXPOST-271
    // audit: FIXPOST-272
    // audit: FIXPOST-273
    // audit: FIXPOST-274
    // audit: FIXPOST-275
    // audit: FIXPOST-276
    // audit: FIXPOST-277
    // audit: FIXPOST-278
    // audit: FIXPOST-279
    // audit: FIXPOST-280
    // audit: FIXPOST-281
    // audit: FIXPOST-282
    // audit: FIXPOST-283
    // audit: FIXPOST-284
    // audit: FIXPOST-285
    // audit: FIXPOST-286
    // audit: FIXPOST-287
    // audit: FIXPOST-288
    // audit: FIXPOST-289
    // audit: FIXPOST-290
    // audit: FIXPOST-291
    // audit: FIXPOST-292
    // audit: FIXPOST-293
    // audit: FIXPOST-294
    // audit: FIXPOST-295
    // audit: FIXPOST-296
    // audit: FIXPOST-297
    // audit: FIXPOST-298
    // audit: FIXPOST-299
    // audit: FIXPOST-300
    #[test]
    fn every_fixture_record_meets_its_fixpost_row() -> TestResult {
        let posted = Posted::run()?;
        let rows = fixpost_rows()?;
        assert_eq!(rows.len(), 300, "FIXPOST-001 to FIXPOST-300");
        assert_eq!(posted.outcomes.len(), 300);
        assert_eq!(posted.bodies.len(), 300);
        for (index, outcome) in posted.outcomes.iter().enumerate() {
            let line = index + 1;
            let expected = rows.get(&line).ok_or("a row for every line")?;
            match (outcome, expected) {
                (RecordOutcome::Posted(posting), Expected::Posted { key, created }) => {
                    let write = &posting.tcatbal;
                    assert_eq!(tcat_key(write.record()), *key, "FIXPOST-{line:03}: key");
                    assert_eq!(
                        matches!(write, TcatbalWrite::Create(_)),
                        *created,
                        "FIXPOST-{line:03}: create or update"
                    );
                }
                (RecordOutcome::Rejected(reject), Expected::Rejected { code, description }) => {
                    let mut record = posted.bodies.get(index).ok_or("a body per line")?.clone();
                    record.extend_from_slice(code);
                    record.extend_from_slice(&padded::<76>(description.as_bytes()));
                    let encoded = reject.encode();
                    assert!(encoded.len() == 430, "FIXPOST-{line:03}: 430 bytes");
                    assert!(encoded == record, "FIXPOST-{line:03}: reject record bytes");
                }
                _ => return Err(format!("FIXPOST-{line:03}: outcome differs").into()),
            }
        }
        Ok(())
    }

    // audit: FIXPOST-901
    // audit: FIXPOST-902
    // audit: FIXPOST-903
    // audit: CBTRN02C-016
    // audit: CBTRN02C-017
    // audit: CBTRN02C-034
    // audit: CBTRN02C-036
    #[test]
    fn the_fixture_run_rejects_38_posts_262_and_returns_four() -> TestResult {
        let posted = Posted::run()?;
        let summary = posted.summary;
        assert_eq!(
            (summary.processed(), summary.rejected(), summary.posted()),
            (300, 38, 262)
        );
        assert_eq!((summary.return_code(), REJECTS_RETURN_CODE), (4, 4));
        assert_eq!(
            summary.sysout_lines(),
            [
                format!("{PROCESSED_LABEL}000000300"),
                format!("{REJECTED_LABEL}000000038"),
                END_LINE.to_owned(),
            ]
        );
        let rejected: Vec<usize> = (1..)
            .zip(&posted.outcomes)
            .filter(|(_, outcome)| matches!(outcome, RecordOutcome::Rejected(_)))
            .map(|(line, _)| line)
            .collect();
        assert_eq!(rejected, REJECTED_LINES);
        assert_eq!(posted.transactions().len(), 262);
        assert_eq!(posted.tcatbals()?.len(), 100);
        let mut created = BTreeSet::new();
        let mut updated = BTreeSet::new();
        for posting in posted.postings() {
            let line = posting.tcatbal.sysout_line();
            match &posting.tcatbal {
                TcatbalWrite::Create(row) => {
                    // X-FIXTURES-1: the FILLER of the last balance read, 22 `0` characters.
                    assert!(row.filler.as_bytes() == &[b'0'; 22], "created FILLER");
                    let line = line.ok_or("a created row shows its not-found line")?;
                    assert!(line.starts_with(TCATBAL_NOT_FOUND_PREFIX));
                    assert!(line.ends_with(TCATBAL_NOT_FOUND_SUFFIX));
                    created.insert(tcat_key(row));
                }
                TcatbalWrite::Update(row) => {
                    assert_eq!(line, None, "an update shows no line");
                    updated.insert(tcat_key(row));
                }
            }
        }
        assert_eq!(created.len(), 50);
        assert!(created.iter().all(|(_, type_cd, _)| type_cd == b"03"));
        assert_eq!(updated.len(), 49);
        assert!(updated.iter().all(|(_, type_cd, _)| type_cd == b"01"));
        assert!(
            !updated.contains(&(37, *b"01", 1)),
            "account 37 posts no type 01 record"
        );
        Ok(())
    }

    // audit: CBTRN02C-012
    // audit: CBTRN02C-013
    // audit: CBTRN02C-024
    #[test]
    fn a_posted_transaction_moves_its_fields_and_stamps_layout_d() -> TestResult {
        let posted = Posted::run()?;
        for (index, outcome) in posted.outcomes.iter().enumerate() {
            let RecordOutcome::Posted(posting) = outcome else {
                continue;
            };
            let line = index + 1;
            let daily = posted.daily.get(index).ok_or("a record per line")?;
            let tran = &posting.transaction;
            let moved = tran.id == daily.id
                && tran.type_cd == daily.type_cd
                && tran.cat_cd == daily.cat_cd
                && tran.source == daily.source
                && tran.desc == daily.desc
                && tran.amt == daily.amt
                && tran.merchant_id == daily.merchant_id
                && tran.merchant_name == daily.merchant_name
                && tran.merchant_city == daily.merchant_city
                && tran.merchant_zip == daily.merchant_zip
                && tran.card_num == daily.card_num
                && tran.orig_ts == daily.orig_ts;
            assert!(moved, "FIXPOST-{line:03}: the twelve moved fields");
            let orig = tran.orig_ts.value().ok_or("TRAN-ORIG-TS is set")?;
            assert_eq!(
                orig.form(),
                TsForm::Iso,
                "FIXPOST-{line:03}: TRAN-ORIG-TS layout"
            );
            let proc = tran.proc_ts.value().ok_or("TRAN-PROC-TS is set")?;
            assert_eq!(
                proc.form(),
                TsForm::Db2,
                "FIXPOST-{line:03}: TRAN-PROC-TS layout"
            );
        }
        Ok(())
    }
}

/// INTCALC, `domain::batch::cbact04c` (`modern/audit/batch/INTCALC.md`, FIXINT rows).
mod intcalc {
    use std::collections::BTreeMap;

    use copybook::{PicDate, PicU, PicX, TsForm};
    use domain::batch::cbact04c::{
        DEFAULT_GROUP, DEFAULT_PARM_DATE, Intcalc, IntcalcEffect, IntcalcError, ParmDate,
        file_status_line, interest_transaction_id,
    };
    use domain::money::{Decimal, Money, Rate};
    use domain::records::{
        AccountRecord, CardXrefRecord, DisclosureGroupRecord, TranCatBalanceRecord,
        TransactionRecord,
    };
    use domain::{CardNumber, FixedClock};

    use super::{Outcome, Posted, TestResult, fixture, instant, money};

    /// The `FixedClock` of the interest scenarios of `fixtures.md`.
    fn interest_clock() -> Outcome<FixedClock> {
        Ok(FixedClock::new(instant((2022, 7, 18), (0, 0, 0))?))
    }

    /// The card number of account `acct_id`, on no fixture line.
    fn card(acct_id: u64) -> CardNumber {
        CardNumber::new(PicX::from_array(super::padded::<16>(
            format!("{:016}", 4_000_000_000_000_000_u64 + acct_id).as_bytes(),
        )))
    }

    fn account(acct_id: u64, group: &[u8; 10]) -> Outcome<AccountRecord> {
        Ok(AccountRecord {
            id: PicU::new(acct_id)?,
            active_status: PicX::from_array(*b"Y"),
            curr_bal: money(10_000)?,
            credit_limit: money(500_000)?,
            cash_credit_limit: money(100_000)?,
            open_date: PicDate::BLANK,
            expiraion_date: PicDate::BLANK,
            reissue_date: PicDate::BLANK,
            curr_cyc_credit: money(1_234)?,
            curr_cyc_debit: money(-567)?,
            addr_zip: PicX::from_array(*b"A000000000"),
            group_id: PicX::from_array(*group),
            filler: PicX::spaces(),
        })
    }

    fn xref(acct_id: u64, card_num: CardNumber) -> Outcome<CardXrefRecord> {
        Ok(CardXrefRecord {
            card_num,
            cust_id: PicU::new(acct_id)?,
            acct_id: PicU::new(acct_id)?,
            filler: PicX::spaces(),
        })
    }

    /// The disclosure record of `group` for type `01`, category `0001`.
    fn group(group: &[u8; 10], rate_cents: i64) -> Outcome<DisclosureGroupRecord> {
        Ok(DisclosureGroupRecord {
            acct_group_id: PicX::from_array(*group),
            tran_type_cd: PicX::from_array(*b"01"),
            tran_cat_cd: PicU::new(1)?,
            int_rate: Rate::new(Decimal::new(rate_cents, 2))?,
            filler: PicX::spaces(),
        })
    }

    /// The category balance (`acct_id`, `01`, `0001`).
    fn balance(acct_id: u64, bal_cents: i64) -> Outcome<TranCatBalanceRecord> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(acct_id)?,
            type_cd: PicX::from_array(*b"01"),
            cd: PicU::new(1)?,
            bal: money(bal_cents)?,
            filler: PicX::spaces(),
        })
    }

    /// The fixture cards by account, from `cardxref.txt`.
    fn fixture_cards() -> Outcome<BTreeMap<u64, CardNumber>> {
        Ok(fixture::<CardXrefRecord>("cardxref.txt")?
            .into_iter()
            .map(|xref| (xref.acct_id.get(), xref.card_num))
            .collect())
    }

    /// SYSTRAN record `n` of FIXINT-901, for account `acct_id` and its card `card`.
    fn assert_systran(record: &TransactionRecord, n: u64, acct_id: u64, card: &CardNumber) {
        let id = format!("2022071800{n:06}");
        assert!(
            record.id.as_bytes() == id.as_bytes(),
            "FIXINT-901: TRAN-ID of record {n}"
        );
        assert_eq!(
            record.type_cd.as_bytes(),
            b"01",
            "FIXINT-901: type of record {n}"
        );
        assert_eq!(record.cat_cd.get(), 5, "FIXINT-901: category of record {n}");
        assert_eq!(
            record.source.as_bytes(),
            b"System    ",
            "FIXINT-901: source {n}"
        );
        // Bytes 25-100 of TRAN-DESC and the FILLER are X-FIXTURES-H4 (HALTED(H2)): not asserted.
        let desc = format!("Int. for a/c {acct_id:011}");
        assert!(
            record.desc.as_bytes().get(..24) == Some(desc.as_bytes()),
            "FIXINT-901: TRAN-DESC bytes 1-24 of record {n}"
        );
        assert_eq!(
            record.merchant_id.get(),
            0,
            "FIXINT-901: merchant ID of record {n}"
        );
        assert!(
            record.merchant_name.is_spaces()
                && record.merchant_city.is_spaces()
                && record.merchant_zip.is_spaces(),
            "FIXINT-901: merchant name, city and ZIP of record {n}"
        );
        assert!(record.card_num == *card, "FIXINT-901: card of record {n}");
        for stamp in [record.orig_ts, record.proc_ts] {
            let form = stamp.value().map(|timestamp| timestamp.form());
            assert_eq!(
                form,
                Some(TsForm::Db2),
                "FIXINT-901: timestamp layout of record {n}"
            );
        }
    }

    // audit: CBACT04C-001
    #[test]
    fn the_parm_date_defaults_to_the_jcl_value_and_keeps_its_first_ten_bytes() -> TestResult {
        assert_eq!(DEFAULT_PARM_DATE, "2022071800");
        assert_eq!(ParmDate::DEFAULT.as_bytes(), b"2022071800");
        assert_eq!(ParmDate::default(), ParmDate::DEFAULT);
        assert_eq!(ParmDate::parse(b"2022071800")?, ParmDate::DEFAULT);
        assert_eq!(
            ParmDate::parse(b"2022071800 MORE")?.as_bytes(),
            b"2022071800"
        );
        assert_eq!(ParmDate::parse(b"ABCDEFGHIJ")?.as_bytes(), b"ABCDEFGHIJ");
        let short = ParmDate::parse(b"202207");
        assert_eq!(short, Err(IntcalcError::ParmDateTooShort { len: 6 }));
        let control = ParmDate::parse(b"2022071800\t");
        assert_eq!(control, Err(IntcalcError::ParmDateNotPrintable));
        for refused in [short, control] {
            let lines = refused.err().map(|error| error.sysout_lines());
            assert_eq!(lines, Some(Vec::new()), "a refused PARM displays nothing");
        }
        Ok(())
    }

    // audit: CBACT04C-015
    #[test]
    fn a_systran_id_is_the_parm_date_then_six_suffix_digits() -> TestResult {
        let first = interest_transaction_id(&ParmDate::DEFAULT, PicU::new(1)?);
        assert_eq!(first.as_bytes(), b"2022071800000001");
        let parm = ParmDate::parse(b"ABCDEFGHIJ")?;
        let last = interest_transaction_id(&parm, PicU::new(999_999)?);
        assert_eq!(last.as_bytes(), b"ABCDEFGHIJ999999");
        Ok(())
    }

    // audit: CBACT04C-016
    #[test]
    fn a_run_past_999999_records_wraps_the_suffix_and_repeats_earlier_ids() -> TestResult {
        // The blank group falls back to DEFAULT's 15.00 and every balance names the same
        // account, so each read writes one SYSTRAN record, never a rewrite: record n carries
        // suffix n modulo 10^6.
        let mut run = Intcalc::new(
            ParmDate::DEFAULT,
            [account(1, b"          ")?],
            [xref(1, card(1))?],
            [group(DEFAULT_GROUP.as_bytes(), 1_500)?],
        );
        let tcatbal = balance(1, 100)?;
        let clock = interest_clock()?;
        // Record 1 and records 999999 to 1000002 are kept; every other ID is checked and dropped.
        let mut kept = Vec::with_capacity(5);
        for n in 1_u64..=1_000_002 {
            let effects = run.process(&tcatbal, &clock)?;
            let [IntcalcEffect::SystemTransaction(record)] = effects.as_slice() else {
                return Err(format!("CBACT04C-016: record {n} is one SYSTRAN record").into());
            };
            let expected = format!("2022071800{:06}", n % 1_000_000);
            assert!(
                record.id.as_bytes() == expected.as_bytes(),
                "CBACT04C-016: TRAN-ID of record {n}"
            );
            if n == 1 || n >= 999_999 {
                kept.push(*record.id.as_bytes());
            }
        }
        let [first, last_unwrapped, wrapped, repeated, after] = kept.as_slice() else {
            return Err("CBACT04C-016: records 1 and 999999 to 1000002 kept".into());
        };
        assert!(
            last_unwrapped.ends_with(b"999999"),
            "CBACT04C-016: record 999999 ends 999999"
        );
        assert!(
            wrapped.ends_with(b"000000"),
            "CBACT04C-016: record 1000000 wraps to 000000"
        );
        assert!(
            repeated.ends_with(b"000001") && repeated == first,
            "CBACT04C-016: record 1000001 repeats the TRAN-ID of record 1"
        );
        assert!(
            after.ends_with(b"000002"),
            "CBACT04C-016: record 1000002 ends 000002"
        );
        // Only the suffix wraps: the written count goes on past 999999.
        let summary = run.finish();
        assert_eq!(
            (summary.records_read(), summary.transactions_written()),
            (1_000_002, 1_000_002)
        );
        Ok(())
    }

    // audit: FIXINT-901
    // audit: FIXINT-902
    // audit: FIXINT-903
    // audit: CBACT04C-003
    // audit: CBACT04C-004
    // audit: CBACT04C-006
    // audit: CBACT04C-010
    // audit: CBACT04C-017
    // audit: CBACT04C-018
    // audit: CBACT04C-021
    // audit: CBACT04C-022
    // audit: CBACT04C-024
    // audit: CBACT04C-026
    // audit: CBACT04C-027
    #[test]
    fn the_raw_run_writes_a_record_per_balance_and_never_rewrites_the_last_account() -> TestResult {
        let accounts = fixture::<AccountRecord>("acctdata.txt")?;
        let mut run = Intcalc::new(
            ParmDate::DEFAULT,
            accounts.clone(),
            fixture::<CardXrefRecord>("cardxref.txt")?,
            fixture::<DisclosureGroupRecord>("discgrp.txt")?,
        );
        let cards = fixture_cards()?;
        let clock = interest_clock()?;
        let mut rewritten = Vec::new();
        for (n, tcatbal) in (1_u64..).zip(fixture::<TranCatBalanceRecord>("tcatbal.txt")?) {
            let acct_id = tcatbal.acct_id.get();
            assert_eq!(
                acct_id, n,
                "FIXINT-{n:03}: tcatbal.txt line n holds account n"
            );
            let effects = run.process(&tcatbal, &clock)?;
            // Every account has a blank group, so its own lookup misses and DEFAULT 01/0001
            // (15.00, not zero) applies: each balance writes one SYSTRAN record (E07).
            let record = match effects.as_slice() {
                [IntcalcEffect::SystemTransaction(record)] if n == 1 => record,
                [
                    IntcalcEffect::AccountUpdate(account),
                    IntcalcEffect::SystemTransaction(record),
                ] => {
                    rewritten.push(account.clone());
                    record
                }
                _ => return Err(format!("FIXINT-{n:03}: effects in program order").into()),
            };
            let card = cards.get(&acct_id).ok_or("a card per account")?;
            assert_systran(record, n, acct_id, card);
        }
        let ids: Vec<u64> = rewritten.iter().map(|account| account.id.get()).collect();
        assert_eq!(
            ids,
            (1..=49).collect::<Vec<u64>>(),
            "FIXINT-903: accounts 1 to 49"
        );
        for account in &rewritten {
            let loaded = accounts
                .iter()
                .find(|loaded| loaded.id == account.id)
                .ok_or("a loaded account")?;
            // Every total is 0.00 and every cycle field 0.00 as loaded, so nothing changes.
            assert!(
                account == loaded,
                "FIXINT-903: raw rewrite keeps the record"
            );
        }
        let summary = run.finish();
        assert_eq!(
            (summary.records_read(), summary.transactions_written()),
            (50, 50)
        );
        assert_eq!(summary.unapplied_last_account().map(PicU::get), Some(50));
        Ok(())
    }

    // audit: FIXINT-901
    // audit: FIXINT-902
    // audit: CBACT04C-012
    // audit: CBACT04C-025
    // audit: CBACT04C-026
    #[test]
    fn the_posted_run_skips_every_zero_rate_balance() -> TestResult {
        let posted = Posted::run()?;
        let mut run = Intcalc::new(
            ParmDate::DEFAULT,
            posted.accounts()?.into_values(),
            fixture::<CardXrefRecord>("cardxref.txt")?,
            fixture::<DisclosureGroupRecord>("discgrp.txt")?,
        );
        let clock = interest_clock()?;
        let (mut written, mut rewrites) = (0, 0);
        for ((acct_id, type_cd, _), tcatbal) in posted.tcatbals()? {
            let effects = run.process(&tcatbal, &clock)?;
            if type_cd == *b"03" {
                // DEFAULT 03/0001 has rate 0.00: nothing is computed or written.
                assert!(
                    effects.is_empty(),
                    "FIXINT-{acct_id:03}: type 03 writes nothing"
                );
                continue;
            }
            // 1400-COMPUTE-FEES adds no effect: at most the break's rewrite and one record.
            for effect in &effects {
                match effect {
                    IntcalcEffect::AccountUpdate(account) => {
                        assert_eq!(
                            account.curr_cyc_credit,
                            Money::<10>::ZERO,
                            "cycle credit zeroed"
                        );
                        assert_eq!(
                            account.curr_cyc_debit,
                            Money::<10>::ZERO,
                            "cycle debit zeroed"
                        );
                        rewrites += 1;
                    }
                    IntcalcEffect::SystemTransaction(_) => written += 1,
                }
            }
            assert!(effects.len() <= 2, "FIXINT-{acct_id:03}: no fee effect");
        }
        assert_eq!((written, rewrites), (50, 49));
        let summary = run.finish();
        assert_eq!(
            (summary.records_read(), summary.transactions_written()),
            (100, 50)
        );
        assert_eq!(summary.unapplied_last_account().map(PicU::get), Some(50));
        Ok(())
    }

    // audit: CBACT04C-009
    // audit: CBACT04C-010
    // audit: CBACT04C-012
    // audit: CBACT04C-015
    // audit: CBACT04C-028
    #[test]
    fn an_account_group_is_read_before_the_default_group() -> TestResult {
        let groups = [
            group(b"GOLD      ", 0)?,
            group(DEFAULT_GROUP.as_bytes(), 1_500)?,
        ];
        let accounts = [
            account(1, b"GOLD      ")?,
            account(2, b"          ")?,
            account(3, b"          ")?,
        ];
        let xrefs = [xref(1, card(1))?, xref(2, card(2))?, xref(3, card(3))?];
        let mut run = Intcalc::new(ParmDate::DEFAULT, accounts, xrefs, groups);
        let clock = interest_clock()?;
        // GOLD's own 0.00 applies, so DEFAULT's 15.00 is never read for account 1.
        assert!(run.process(&balance(1, 100_000)?, &clock)?.is_empty());
        // No record keys the blank group: DEFAULT applies, and the run's first record takes
        // suffix 000001 although it comes from the second balance.
        let effects = run.process(&balance(2, 100_000)?, &clock)?;
        let [
            IntcalcEffect::AccountUpdate(first),
            IntcalcEffect::SystemTransaction(record),
        ] = effects.as_slice()
        else {
            return Err("account 1 rewritten, then one record for account 2".into());
        };
        assert_eq!(first.id.get(), 1);
        assert_eq!(record.id.as_bytes(), b"2022071800000001");
        // Account 3 has no balance, so it is never read or rewritten (CBACT04C-028).
        let summary = run.finish();
        assert_eq!(
            (summary.records_read(), summary.transactions_written()),
            (2, 1)
        );
        assert_eq!(summary.unapplied_last_account().map(PicU::get), Some(2));
        let empty = Intcalc::new(
            ParmDate::DEFAULT,
            Vec::<AccountRecord>::new(),
            Vec::<CardXrefRecord>::new(),
            Vec::<DisclosureGroupRecord>::new(),
        );
        assert_eq!(empty.finish().unapplied_last_account(), None);
        Ok(())
    }

    // audit: CBACT04C-005
    // audit: CBACT04C-007
    // audit: CBACT04C-011
    // audit: CBACT04C-036
    // audit: CBACT04C-038
    #[test]
    fn a_failed_lookup_abends_with_its_lines_and_keeps_the_earlier_rewrite() -> TestResult {
        let clock = interest_clock()?;
        let default = || group(DEFAULT_GROUP.as_bytes(), 1_500);
        let mut no_account = Intcalc::new(
            ParmDate::DEFAULT,
            Vec::<AccountRecord>::new(),
            Vec::<CardXrefRecord>::new(),
            [default()?],
        );
        let error = no_account.process(&balance(9, 100)?, &clock).err();
        let error = error.ok_or("account 9 is missing")?;
        let expected = [
            "ACCOUNT NOT FOUND: 00000000009",
            "ERROR READING ACCOUNT FILE",
            "FILE STATUS IS: NNNN0023",
            "ABENDING PROGRAM",
        ];
        assert_eq!(error.sysout_lines(), expected);
        assert_eq!(error.rewritten_before_abend(), None);
        let mut no_xref = Intcalc::new(
            ParmDate::DEFAULT,
            [account(9, b"          ")?],
            Vec::<CardXrefRecord>::new(),
            [default()?],
        );
        let error = no_xref.process(&balance(9, 100)?, &clock).err();
        let expected = [
            "ACCOUNT NOT FOUND: 00000000009",
            "ERROR READING XREF FILE",
            "FILE STATUS IS: NNNN0023",
            "ABENDING PROGRAM",
        ];
        assert_eq!(
            error.ok_or("no card for account 9")?.sysout_lines(),
            expected
        );
        let mut no_default = Intcalc::new(
            ParmDate::DEFAULT,
            [account(9, b"          ")?],
            [xref(9, card(9))?],
            Vec::<DisclosureGroupRecord>::new(),
        );
        let error = no_default.process(&balance(9, 100)?, &clock).err();
        let expected = [
            "DISCLOSURE GROUP RECORD MISSING",
            "TRY WITH DEFAULT GROUP CODE",
            "ERROR READING DEFAULT DISCLOSURE GROUP",
            "FILE STATUS IS: NNNN0023",
            "ABENDING PROGRAM",
        ];
        assert_eq!(error.ok_or("no DEFAULT record")?.sysout_lines(), expected);
        // Account 1's break rewrite was issued before account 2's read failed, and stands.
        let mut later = Intcalc::new(
            ParmDate::DEFAULT,
            [account(1, b"          ")?],
            [xref(1, card(1))?],
            [default()?],
        );
        later.process(&balance(1, 100)?, &clock)?;
        let error = later.process(&balance(2, 100)?, &clock).err();
        let rewritten = error
            .as_ref()
            .and_then(IntcalcError::rewritten_before_abend);
        assert_eq!(rewritten.map(|account| account.id.get()), Some(1));
        assert_eq!(file_status_line(*b"23"), "FILE STATUS IS: NNNN0023");
        assert_eq!(file_status_line(*b"02"), "FILE STATUS IS: NNNN0002");
        Ok(())
    }

    #[test]
    fn halted_statuses_and_duplicate_cross_references_run_without_a_parity_claim() -> TestResult {
        // HALTED(H4) (CBACT04C-037) and X-INTCALC-H5 (CBACT04C-008): exercised, not asserted.
        let _ = file_status_line(*b"9A");
        let _ = file_status_line(*b"3 ");
        let mut run = Intcalc::new(
            ParmDate::DEFAULT,
            [account(1, b"          ")?],
            [xref(1, card(1))?, xref(1, card(2))?],
            [group(DEFAULT_GROUP.as_bytes(), 1_500)?],
        );
        let _ = run.process(&balance(1, 100)?, &interest_clock()?);
        Ok(())
    }
}

/// CREASTMT, `domain::batch::cbstm03a` (`modern/audit/batch/CREASTMT.md`, FIXSTMT rows).
mod creastmt {
    use std::collections::BTreeMap;

    use copybook::{PicTimestamp, PicU, PicX};
    use domain::CardNumber;
    use domain::batch::cbstm03a::{
        HTML_RECORD_LEN, STMT_RECORD_LEN, Statement, StatementError, TransactionTable, sort_key,
        statement, xref_sort_key,
    };
    use domain::records::{AccountRecord, CardXrefRecord, CustomerRecord, TransactionRecord};

    use super::{Outcome, Posted, TestResult, fixture, money, padded};

    const DASHES: &str =
        "--------------------------------------------------------------------------------";

    /// The 25 `STATEMNT.PS` records of FIXSTMT-001, quoted as the audit gives them.
    const FIRST_TEXT: [&str; 25] = [
        "*******************************START OF STATEMENT*******************************",
        "Aniya Alba Von",
        "1588 Nienow Cape",
        "Suite 187",
        "New OR USA 04257",
        DASHES,
        "                                 Basic Details",
        DASHES,
        "Account ID         :00000000050",
        "Current Balance    :000001945.87",
        "FICO Score         :623",
        DASHES,
        "                              TRANSACTION SUMMARY",
        DASHES,
        "Tran ID         Tran Details                                         Tran Amount",
        DASHES,
        "{T:21} Purchase at Blick-Rippin                         $      183.88",
        "{T:102} Purchase at Reichel Group                        $       14.00",
        "{T:142} Purchase at Adams-Watsica                        $      967.44",
        "{T:184} Return item at DuBuque, Wuckert and Mraz         $       47.88-",
        "{T:214} Purchase at Williamson Group                     $       94.77",
        "{T:257} Purchase at Abbott-Gerlach                       $      241.66",
        DASHES,
        "Total EXP:                                                        $     1453.87",
        "********************************END OF STATEMENT********************************",
    ];

    /// Indexes into [`FIRST_TEXT`] of the six `ST-LINE14` records.
    const FIRST_TRANSACTIONS: core::ops::Range<usize> = 16..22;

    const LTRS: &str = "<tr>";
    const LTRE: &str = "</tr>";
    const LTDE: &str = "</td>";
    const L10: &str = r#"<td colspan="3" style="padding:0px 5px;background-color:#1d1d96b3;">"#;
    const L22_35: &str = r#"<td colspan="3" style="padding:0px 5px;background-color:#f2f2f2;">"#;
    const L30_42: &str =
        r#"<td colspan="3" style="padding:0px 5px;background-color:#33FFD1; text-align:center;">"#;

    /// The 22 records of 5100-WRITE-HTML-HEADER for statement 1 (FIXSTMT-001, FIXSTMT-903).
    const FIRST_HTML_HEADER: [&str; 22] = [
        "<!DOCTYPE html>",
        r#"<html lang="en">"#,
        "<head>",
        r#"<meta charset="utf-8">"#,
        "<title>HTML Table Layout</title>",
        "</head>",
        r#"<body style="margin:0px;">"#,
        r#"<table  align="center" frame="box" style="width:70%; font:12px Segoe UI,sans-serif;">"#,
        LTRS,
        L10,
        "<h3>Statement for Account Number: 00000000050         </h3>",
        LTDE,
        LTRE,
        LTRS,
        r#"<td colspan="3" style="padding:0px 5px;background-color:#FFAF33;">"#,
        r#"<p style="font-size:16px">Bank of XYZ</p>"#,
        "<p>410 Terry Ave N</p>",
        "<p>Seattle WA 99999</p>",
        LTDE,
        LTRE,
        LTRS,
        L22_35,
    ];

    /// The 34 records of 5200-WRITE-HTML-NMADBS for statement 1 (FIXSTMT-001, FIXSTMT-903).
    const FIRST_HTML_DETAILS: [&str; 34] = [
        r#"<p style="font-size:16px">Aniya Alba Von  </p>"#,
        "<p>1588 Nienow Cape  </p>",
        "<p>Suite 187  </p>",
        "<p>New OR USA 04257  </p>",
        LTDE,
        LTRE,
        LTRS,
        L30_42,
        r#"<p style="font-size:16px">Basic Details</p>"#,
        LTDE,
        LTRE,
        LTRS,
        L22_35,
        "<p>Account ID         : 00000000050         </p>",
        "<p>Current Balance    : 000001945.87 </p>",
        "<p>FICO Score         : 623                 </p>",
        LTDE,
        LTRE,
        LTRS,
        L30_42,
        r#"<p style="font-size:16px">Transaction Summary</p>"#,
        LTDE,
        LTRE,
        LTRS,
        r#"<td style="width:25%; padding:0px 5px; background-color:#33FF5E; text-align:left;">"#,
        r#"<p style="font-size:16px">Tran ID</p>"#,
        LTDE,
        r#"<td style="width:55%; padding:0px 5px; background-color:#33FF5E; text-align:left;">"#,
        r#"<p style="font-size:16px">Tran Details</p>"#,
        LTDE,
        r#"<td style="width:20%; padding:0px 5px; background-color:#33FF5E; text-align:right;">"#,
        r#"<p style="font-size:16px">Amount</p>"#,
        LTDE,
        LTRE,
    ];

    /// The cells around one transaction's three fields (CBSTM03A-037, CBSTM03A-045).
    const L58: &str =
        r#"<td style="width:25%; padding:0px 5px; background-color:#f2f2f2; text-align:left;">"#;
    const L61: &str =
        r#"<td style="width:55%; padding:0px 5px; background-color:#f2f2f2; text-align:left;">"#;
    const L64: &str =
        r#"<td style="width:20%; padding:0px 5px; background-color:#f2f2f2; text-align:right;">"#;

    /// The 8 records that close every statement's document (CBSTM03A-038, CBSTM03A-046).
    const HTML_TRAILER: [&str; 8] = [
        LTRS,
        L10,
        "<h3>End of Statement</h3>",
        LTDE,
        LTRE,
        "</table>",
        "</body>",
        "</html>",
    ];

    /// A transaction of card `4000000000000000` + `card` with transaction ID `id`.
    fn transaction(card: u64, id: u64) -> Outcome<TransactionRecord> {
        let digits = |value: u64| PicX::from_array(padded::<16>(format!("{value:016}").as_bytes()));
        Ok(TransactionRecord {
            id: digits(id),
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            source: PicX::from_array(*b"POS TERM  "),
            desc: PicX::moved_from(b"Purchase at a test merchant"),
            amt: money(1_000)?,
            merchant_id: PicU::new(1)?,
            merchant_name: PicX::spaces(),
            merchant_city: PicX::spaces(),
            merchant_zip: PicX::spaces(),
            card_num: CardNumber::new(digits(4_000_000_000_000_000 + card)),
            orig_ts: PicTimestamp::BLANK,
            proc_ts: PicTimestamp::BLANK,
            filler: PicX::spaces(),
        })
    }

    /// The `POSTED` statements, one per cross-reference record in `xref_sort_key` order.
    fn posted_statements(posted: &Posted) -> Outcome<Vec<(CardXrefRecord, Statement)>> {
        let table = TransactionTable::build(posted.transactions())?;
        let customers: BTreeMap<u64, CustomerRecord> = fixture::<CustomerRecord>("custdata.txt")?
            .into_iter()
            .map(|customer| (customer.id.get(), customer))
            .collect();
        let accounts: BTreeMap<u64, AccountRecord> = posted.accounts()?;
        let mut xrefs = fixture::<CardXrefRecord>("cardxref.txt")?;
        xrefs.sort_by_key(xref_sort_key);
        xrefs
            .into_iter()
            .map(|xref| {
                let customer = customers
                    .get(&xref.cust_id.get())
                    .ok_or("a customer per cross-reference")?;
                let account = accounts
                    .get(&xref.acct_id.get())
                    .ok_or("an account per cross-reference")?;
                let built = statement(&xref, customer, account, &table);
                Ok((xref, built))
            })
            .collect()
    }

    /// The `HTML` records of statement 1, from FIXSTMT-001 and FIXSTMT-903: each transaction's
    /// three cells take the fields of its quoted `ST-LINE14`.
    fn first_html(posted: &Posted) -> Outcome<Vec<[u8; HTML_RECORD_LEN]>> {
        let mut records: Vec<Vec<u8>> = FIRST_HTML_HEADER
            .iter()
            .chain(&FIRST_HTML_DETAILS)
            .map(|quote| quote.as_bytes().to_vec())
            .collect();
        for quote in FIRST_TEXT.get(FIRST_TRANSACTIONS).ok_or("six ST-LINE14")? {
            let line = padded::<STMT_RECORD_LEN>(&posted.expand(quote)?);
            let cell = |range: core::ops::Range<usize>| -> Outcome<Vec<u8>> {
                let field = line.get(range).ok_or("an ST-LINE14 field")?;
                Ok([b"<p>".as_slice(), field, b"</p>"].concat())
            };
            records.extend([LTRS.into(), L58.into()]);
            records.push(cell(0..16)?);
            records.extend([LTDE.into(), L61.into()]);
            records.push(cell(17..66)?);
            records.extend([LTDE.into(), L64.into()]);
            records.push(cell(67..80)?);
            records.extend([LTDE.into(), LTRE.into()]);
        }
        records.extend(HTML_TRAILER.iter().map(|quote| quote.as_bytes().to_vec()));
        Ok(records
            .iter()
            .map(|record| padded::<HTML_RECORD_LEN>(record))
            .collect())
    }

    // audit: CBSTM03A-031
    // audit: CBSTM03A-047
    #[test]
    fn text_records_are_80_bytes_and_html_records_100() {
        assert_eq!((STMT_RECORD_LEN, HTML_RECORD_LEN), (80, 100));
    }

    // audit: CBSTM03A-001
    // audit: CBSTM03A-009
    // audit: CBSTM03B-002
    // audit: CBSTM03B-003
    #[test]
    fn transactions_sort_by_card_number_then_transaction_id() -> TestResult {
        let mut records = [
            transaction(2, 1)?,
            transaction(1, 9)?,
            transaction(1, 3)?,
            transaction(2, 0)?,
        ];
        records.sort_by_key(sort_key);
        let expected = [
            transaction(1, 3)?,
            transaction(1, 9)?,
            transaction(2, 0)?,
            transaction(2, 1)?,
        ];
        assert!(records == expected, "card number, then transaction ID");
        let xref = |card: u64| -> Outcome<CardXrefRecord> {
            Ok(CardXrefRecord {
                card_num: transaction(card, 0)?.card_num,
                cust_id: PicU::new(card)?,
                acct_id: PicU::new(card)?,
                filler: PicX::spaces(),
            })
        };
        let mut xrefs = [xref(3)?, xref(1)?, xref(2)?];
        xrefs.sort_by_key(xref_sort_key);
        let accounts: Vec<u64> = xrefs.iter().map(|xref| xref.acct_id.get()).collect();
        assert_eq!(accounts, [1, 2, 3], "cross-references in card-number order");
        Ok(())
    }

    // audit: CBSTM03A-007
    // audit: CBSTM03A-008
    #[test]
    fn the_table_needs_a_transaction_and_reports_the_cobol_bounds() -> TestResult {
        let empty = TransactionTable::build(Vec::<&TransactionRecord>::new()).err();
        assert_eq!(empty, Some(StatementError::NoTransactions));
        let mut full = Vec::new();
        for card in 0..51 {
            for id in 0..10 {
                full.push(transaction(card, id)?);
            }
        }
        let table = TransactionTable::build(&full)?;
        assert_eq!(table.card_count(), 51);
        assert!(
            table.within_cobol_bounds(),
            "51 cards of 10 fit WS-TRNX-TABLE"
        );
        let one_card_more = TransactionTable::build(full.iter().chain([&transaction(51, 0)?]))?;
        assert_eq!(one_card_more.card_count(), 52);
        assert!(
            !one_card_more.within_cobol_bounds(),
            "a 52nd card leaves the table"
        );
        let one_more = TransactionTable::build(full.iter().chain([&transaction(0, 10)?]))?;
        assert_eq!(one_more.card_count(), 51);
        assert!(
            !one_more.within_cobol_bounds(),
            "an 11th transaction leaves the table"
        );
        Ok(())
    }

    // audit: FIXSTMT-901
    // audit: CBSTM03A-008
    // audit: CBSTM03A-009
    // audit: CBSTM03A-013
    // audit: CBSTM03A-031
    // audit: CBSTM03A-047
    #[test]
    fn the_posted_run_writes_50_statements_of_1212_and_6082_records() -> TestResult {
        let posted = Posted::run()?;
        let transactions = posted.transactions();
        let table = TransactionTable::build(transactions.iter().copied())?;
        assert_eq!(table.card_count(), 50);
        assert!(table.within_cobol_bounds());
        let statements = posted_statements(&posted)?;
        let file_order = fixture::<CardXrefRecord>("cardxref.txt")?;
        assert!(
            statements
                .iter()
                .map(|(xref, _)| xref)
                .eq(file_order.iter()),
            "FIXSTMT-901: statements in cardxref.txt line order"
        );
        let (mut text, mut html) = (0, 0);
        for (n, (xref, built)) in (1..).zip(&statements) {
            let count = transactions
                .iter()
                .filter(|tran| tran.card_num == xref.card_num)
                .count();
            assert!(
                (1..=6).contains(&count),
                "FIXSTMT-{n:03}: 1 to 6 transactions"
            );
            assert_eq!(
                built.text_lines.len(),
                19 + count,
                "FIXSTMT-{n:03}: text records"
            );
            assert_eq!(
                built.html_records.len(),
                64 + 11 * count,
                "FIXSTMT-{n:03}: HTML"
            );
            text += built.text_lines.len();
            html += built.html_records.len();
        }
        assert_eq!((statements.len(), text, html), (50, 1_212, 6_082));
        Ok(())
    }

    // audit: FIXSTMT-001
    // audit: FIXSTMT-902
    // audit: CBSTM03A-014
    // audit: CBSTM03A-015
    // audit: CBSTM03A-016
    // audit: CBSTM03A-017
    // audit: CBSTM03A-018
    // audit: CBSTM03A-019
    // audit: CBSTM03A-020
    // audit: CBSTM03A-021
    // audit: CBSTM03A-022
    // audit: CBSTM03A-023
    // audit: CBSTM03A-024
    // audit: CBSTM03A-025
    // audit: CBSTM03A-026
    // audit: CBSTM03A-027
    // audit: CBSTM03A-028
    // audit: CBSTM03A-029
    // audit: CBSTM03A-030
    #[test]
    fn the_first_statement_has_its_audited_text_records() -> TestResult {
        let posted = Posted::run()?;
        let statements = posted_statements(&posted)?;
        let (_, first) = statements.first().ok_or("a first statement")?;
        assert_eq!(first.text_lines.len(), FIRST_TEXT.len());
        for (n, (actual, quote)) in (1..).zip(first.text_lines.iter().zip(FIRST_TEXT)) {
            let expected = padded::<STMT_RECORD_LEN>(&posted.expand(quote)?);
            assert!(*actual == expected, "FIXSTMT-001: STATEMNT.PS record {n}");
        }
        Ok(())
    }

    // audit: FIXSTMT-001
    // audit: FIXSTMT-903
    // audit: CBSTM03A-032
    // audit: CBSTM03A-033
    // audit: CBSTM03A-034
    // audit: CBSTM03A-035
    // audit: CBSTM03A-036
    // audit: CBSTM03A-037
    // audit: CBSTM03A-038
    // audit: CBSTM03A-039
    // audit: CBSTM03A-040
    // audit: CBSTM03A-041
    // audit: CBSTM03A-042
    // audit: CBSTM03A-043
    // audit: CBSTM03A-044
    // audit: CBSTM03A-045
    // audit: CBSTM03A-046
    #[test]
    fn the_first_statement_has_its_audited_html_records() -> TestResult {
        let posted = Posted::run()?;
        let statements = posted_statements(&posted)?;
        let (_, first) = statements.first().ok_or("a first statement")?;
        let expected = first_html(&posted)?;
        assert_eq!(first.html_records.len(), 130);
        assert_eq!(expected.len(), 130);
        for (n, (actual, wanted)) in (1..).zip(first.html_records.iter().zip(&expected)) {
            assert!(actual == wanted, "FIXSTMT-001: STATEMNT.HTML record {n}");
        }
        Ok(())
    }
}

/// TRANREPT, `domain::batch::cbtrn03c` (`modern/audit/batch/TRANREPT.md`, FIXRPT rows).
mod tranrept {
    use chrono::NaiveDateTime;
    use copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};
    use domain::CardNumber;
    use domain::batch::cbtrn03c::{
        DATEPARM_RECORD_LEN, DateParm, DateParmError, Flow, REPORT_RECORD_LEN, ReportError,
        ReportRecord, Tranrept, selects, sort_key,
    };
    use domain::records::{
        CardXrefRecord, TransactionCategoryRecord, TransactionRecord, TransactionTypeRecord,
    };

    use super::{Outcome, Posted, TestResult, fixture, instant, money, padded};

    /// The first record of every header group (FIXRPT-902).
    const HEADER_GROUPS: [usize; 15] = [
        1, 23, 83, 103, 123, 163, 183, 203, 263, 283, 323, 343, 383, 403, 443,
    ];

    /// The page-total records, the last one at end of file (FIXRPT-904, FIXRPT-905).
    const PAGE_TOTALS: [usize; 15] = [
        21, 81, 101, 121, 161, 181, 201, 261, 281, 321, 341, 381, 401, 441, 449,
    ];

    /// The details of card block 1, FIXRPT-001, quoted as the audit gives them.
    const FIRST_BLOCK: [&str; 6] = [
        "{T:21} 00000000050 01-Purchase        0001-Regular Sales Draft           POS TERM               183.88",
        "{T:102} 00000000050 01-Purchase        0001-Regular Sales Draft           POS TERM                14.00",
        "{T:142} 00000000050 01-Purchase        0001-Regular Sales Draft           POS TERM               967.44",
        "{T:184} 00000000050 03-Credit          0001-Credit to Account             OPERATOR      -         47.88",
        "{T:214} 00000000050 01-Purchase        0001-Regular Sales Draft           POS TERM                94.77",
        "{T:257} 00000000050 01-Purchase        0001-Regular Sales Draft           POS TERM               241.66",
    ];

    /// `TRANSACTION-HEADER-1`, its trailing spaces omitted (FIXRPT-902).
    const HEADER_1: &str = "Transaction ID   Account ID  Transaction Type   Tran Category                      Tran Source            Amount";

    /// What a report record is.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Kind {
        Header,
        Dashes,
        Detail,
        AccountTotal,
        PageTotal,
        GrandTotal,
    }

    /// The four records of a header group over the default window (FIXRPT-902).
    fn header_group() -> [ReportRecord; 4] {
        let name = format!(
            "DALYREPT{}Daily Transaction Report{}Date Range: 2022-01-01 to 2022-07-06",
            " ".repeat(30),
            " ".repeat(17)
        );
        [
            padded(name.as_bytes()),
            [b' '; REPORT_RECORD_LEN],
            padded(HEADER_1.as_bytes()),
            [b'-'; REPORT_RECORD_LEN],
        ]
    }

    /// A total record: its label, the dots up to column 97 and the edited amount.
    fn total(label: &str, amount: &str) -> ReportRecord {
        let dots = ".".repeat(97 - label.len());
        padded(format!("{label}{dots}{amount}").as_bytes())
    }

    /// The kind of a record outside the header groups.
    fn kind(record: &ReportRecord) -> Kind {
        if record.iter().all(|&byte| byte == b'-') {
            Kind::Dashes
        } else if record.starts_with(b"Page Total ") {
            Kind::PageTotal
        } else if record.starts_with(b"Account Total") {
            Kind::AccountTotal
        } else if record.starts_with(b"Grand Total") {
            Kind::GrandTotal
        } else if record.starts_with(b"DALYREPT")
            || record.starts_with(b"Transaction ID")
            || record.iter().all(|&byte| byte == b' ')
        {
            Kind::Header
        } else {
            Kind::Detail
        }
    }

    /// The report of `tranrept` over `POSTED` with the default window, and its end-of-file
    /// lines.
    fn posted_report(posted: &Posted) -> Outcome<(Vec<ReportRecord>, Option<[String; 2]>)> {
        let mut run = Tranrept::new(
            DateParm::DEFAULT,
            fixture::<CardXrefRecord>("cardxref.txt")?,
            fixture::<TransactionTypeRecord>("trantype.txt")?,
            fixture::<TransactionCategoryRecord>("trancatg.txt")?,
        );
        let mut selected: Vec<&TransactionRecord> = posted
            .transactions()
            .into_iter()
            .filter(|tran| selects(tran, &DateParm::DEFAULT))
            .collect();
        assert_eq!(
            selected.len(),
            262,
            "FIXRPT-901: every posted row is selected"
        );
        selected.sort_by_key(|tran| sort_key(tran));
        for tran in selected {
            assert_eq!(run.process(tran)?, Flow::Continue);
        }
        let display = run.end_of_file_display();
        Ok((run.finish()?, display))
    }

    /// A transaction of card `4000000000000000` + `card`, processed at `proc_ts`.
    fn reported(
        card: u64,
        id: u64,
        amt_cents: i64,
        proc_ts: Option<(NaiveDateTime, TsForm)>,
    ) -> Outcome<TransactionRecord> {
        let digits = |value: u64| PicX::from_array(padded::<16>(format!("{value:016}").as_bytes()));
        let stamp = match proc_ts {
            Some((value, form)) => PicTimestamp::new(Some(CobolTimestamp::new(value, form)?)),
            None => PicTimestamp::BLANK,
        };
        Ok(TransactionRecord {
            id: digits(id),
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            source: PicX::from_array(*b"POS TERM  "),
            desc: PicX::moved_from(b"Purchase at a test merchant"),
            amt: money(amt_cents)?,
            merchant_id: PicU::new(1)?,
            merchant_name: PicX::spaces(),
            merchant_city: PicX::spaces(),
            merchant_zip: PicX::spaces(),
            card_num: card_number(card),
            orig_ts: stamp,
            proc_ts: stamp,
            filler: PicX::spaces(),
        })
    }

    fn card_number(card: u64) -> CardNumber {
        CardNumber::new(PicX::from_array(padded::<16>(
            format!("{:016}", 4_000_000_000_000_000_u64 + card).as_bytes(),
        )))
    }

    /// A run whose lookups know card 1, type `01` and category `01`/`0001`.
    fn small_run(parm: DateParm) -> Outcome<Tranrept> {
        let xref = CardXrefRecord {
            card_num: card_number(1),
            cust_id: PicU::new(1)?,
            acct_id: PicU::new(1)?,
            filler: PicX::spaces(),
        };
        let tran_type = TransactionTypeRecord {
            tran_type: PicX::from_array(*b"01"),
            desc: PicX::moved_from(b"Purchase"),
            filler: PicX::spaces(),
        };
        let category = TransactionCategoryRecord {
            type_cd: PicX::from_array(*b"01"),
            cat_cd: PicU::new(1)?,
            type_desc: PicX::moved_from(b"Regular Sales Draft"),
            filler: PicX::spaces(),
        };
        Ok(Tranrept::new(parm, [xref], [tran_type], [category]))
    }

    /// 2022-07-01 12:00:00 in layout `D`, as `posttran` stamps it.
    fn in_window() -> Outcome<Option<(NaiveDateTime, TsForm)>> {
        Ok(Some((instant((2022, 7, 1), (12, 0, 0))?, TsForm::Db2)))
    }

    // audit: CBTRN03C-001
    // audit: CBTRN03C-008
    #[test]
    fn the_default_window_builds_its_dateparm_record() -> TestResult {
        let parm = DateParm::DEFAULT;
        assert_eq!(parm.start(), instant((2022, 1, 1), (0, 0, 0))?.date());
        assert_eq!(parm.end(), instant((2022, 7, 6), (0, 0, 0))?.date());
        assert_eq!(DateParm::default(), parm);
        assert_eq!(DateParm::parse("2022-01-01", "2022-07-06")?, parm);
        assert_eq!(DATEPARM_RECORD_LEN, 80);
        assert!(parm.record() == padded::<80>(b"2022-01-01 2022-07-06"));
        assert_eq!(
            parm.reporting_line(),
            "Reporting from 2022-01-01 to 2022-07-06"
        );
        let later = DateParm::new(parm.end(), parm.start())?;
        assert!(later.record() == padded::<80>(b"2022-07-06 2022-01-01"));
        Ok(())
    }

    // audit: CBTRN03C-002
    #[test]
    fn option_text_that_names_no_date_is_refused() {
        let cases = [
            ("2022-1-1", "2022-07-06", DateParmError::Start),
            ("2022-01-01", "01/01/2022", DateParmError::End),
            ("2022-02-30", "2022-07-06", DateParmError::Start),
            ("          ", "2022-07-06", DateParmError::Start),
            ("2022-01-0 ", "2022-07-06", DateParmError::Start),
            ("2022-01-01", "2022-07-066", DateParmError::End),
            ("bad", "bad", DateParmError::Start),
        ];
        for (start, end, error) in cases {
            assert_eq!(
                DateParm::parse(start, end),
                Err(error),
                "{start:?} to {end:?}"
            );
        }
        assert!(DateParm::parse("2022-07-06", "2022-01-01").is_ok());
    }

    // audit: CBTRN03C-004
    #[test]
    fn the_window_selects_on_the_processing_date_both_ends_included() -> TestResult {
        let parm = DateParm::DEFAULT;
        let cases = [
            (
                "before the start",
                Some(((2021, 12, 31), TsForm::Db2)),
                false,
            ),
            ("the start", Some(((2022, 1, 1), TsForm::Db2)), true),
            ("the end, layout I", Some(((2022, 7, 6), TsForm::Iso)), true),
            (
                "the end, layout S",
                Some(((2022, 7, 6), TsForm::ScreenDate)),
                true,
            ),
            ("after the end", Some(((2022, 7, 7), TsForm::Db2)), false),
            ("blank", None, false),
        ];
        for (case, stamp, selected) in cases {
            let stamp = match stamp {
                Some((date, TsForm::ScreenDate)) => {
                    Some((instant(date, (0, 0, 0))?, TsForm::ScreenDate))
                }
                Some((date, form)) => Some((instant(date, (23, 59, 59))?, form)),
                None => None,
            };
            let tran = reported(1, 1, 100, stamp)?;
            assert_eq!(selects(&tran, &parm), selected, "{case}");
        }
        Ok(())
    }

    // audit: CBTRN03C-005
    #[test]
    fn selected_rows_sort_by_card_number_then_transaction_id() -> TestResult {
        let stamp = in_window()?;
        let mut rows = [
            reported(2, 1, 100, stamp)?,
            reported(1, 9, 100, stamp)?,
            reported(1, 3, 100, stamp)?,
        ];
        rows.sort_by_key(sort_key);
        let expected = [
            reported(1, 3, 100, stamp)?,
            reported(1, 9, 100, stamp)?,
            reported(2, 1, 100, stamp)?,
        ];
        assert!(rows == expected, "card number, then transaction ID");
        Ok(())
    }

    // audit: FIXRPT-901
    // audit: FIXRPT-902
    // audit: FIXRPT-904
    // audit: FIXRPT-905
    // audit: CBTRN03C-019
    // audit: CBTRN03C-020
    // audit: CBTRN03C-021
    // audit: CBTRN03C-025
    // audit: CBTRN03C-027
    // audit: CBTRN03C-031
    // audit: CBTRN03C-032
    #[test]
    fn the_posted_report_holds_its_audited_record_kinds_and_positions() -> TestResult {
        let posted = Posted::run()?;
        let (records, display) = posted_report(&posted)?;
        assert_eq!(records.len(), 451, "FIXRPT-901");
        assert!(display.is_some(), "the end-of-file branch is taken");
        let group = header_group();
        let mut kinds = Vec::with_capacity(records.len());
        for (position, record) in (1..).zip(&records) {
            let starts_group = HEADER_GROUPS
                .iter()
                .any(|&start| (start..start + 4).contains(&position));
            if starts_group {
                let first = HEADER_GROUPS
                    .iter()
                    .rev()
                    .find(|&&start| start <= position)
                    .ok_or("a group start")?;
                let wanted = group.get(position - first).ok_or("four records a group")?;
                assert!(record == wanted, "FIXRPT-902: record {position}");
            } else {
                kinds.push((position, kind(record)));
            }
        }
        let count = |wanted: Kind| kinds.iter().filter(|(_, kind)| *kind == wanted).count();
        assert_eq!(count(Kind::Detail), 262, "FIXRPT-901: details");
        assert_eq!(count(Kind::AccountTotal), 49, "FIXRPT-901: account totals");
        assert_eq!(count(Kind::PageTotal), 15, "FIXRPT-901: page totals");
        assert_eq!(count(Kind::GrandTotal), 1, "FIXRPT-901: grand total");
        assert_eq!(count(Kind::Dashes), 64, "FIXRPT-901: dash lines");
        assert_eq!(count(Kind::Header), 0, "no header outside a group");
        let pages: Vec<usize> = kinds
            .iter()
            .filter(|(_, kind)| *kind == Kind::PageTotal)
            .map(|(position, _)| *position)
            .collect();
        assert_eq!(pages, PAGE_TOTALS, "FIXRPT-904: page-total positions");
        for (position, record) in (1..).zip(&records) {
            let follows_total = matches!(kind(record), Kind::PageTotal | Kind::AccountTotal);
            if follows_total {
                let next = records.get(position).ok_or("a record after each total")?;
                assert_eq!(kind(next), Kind::Dashes, "dashes after record {position}");
            }
        }
        // The totals at 21, 201 and 341 close pages that split no card block (FIXRPT-904).
        for (position, amount) in [
            (21, "+      4,665.52"),
            (201, "+      2,577.64"),
            (341, "+      4,381.41"),
        ] {
            let record = records.get(position - 1).ok_or("a page total")?;
            assert!(
                *record == total("Page Total ", amount),
                "FIXRPT-904: {position}"
            );
        }
        // FIXRPT-905: the last detail at 448, then the page total, dashes and the grand total,
        // with no account total for the last card block.
        let tail: Vec<Kind> = records
            .get(447..)
            .ok_or("451 records")?
            .iter()
            .map(kind)
            .collect();
        assert_eq!(
            tail,
            [
                Kind::Detail,
                Kind::PageTotal,
                Kind::Dashes,
                Kind::GrandTotal
            ]
        );
        Ok(())
    }

    // audit: FIXRPT-001
    // audit: FIXRPT-903
    // audit: CBTRN03C-013
    // audit: CBTRN03C-015
    // audit: CBTRN03C-017
    // audit: CBTRN03C-023
    // audit: CBTRN03C-024
    // audit: CBTRN03C-026
    #[test]
    fn the_first_card_block_and_every_description_come_from_the_vsam_version() -> TestResult {
        let posted = Posted::run()?;
        let (records, _) = posted_report(&posted)?;
        // The order inside a card block is X-FIXTURES-H3, so block 1 is compared as a set.
        let mut block = records.get(4..10).ok_or("records 5 to 10")?.to_vec();
        let mut expected = FIRST_BLOCK
            .iter()
            .map(|quote| Ok(padded::<REPORT_RECORD_LEN>(&posted.expand(quote)?)))
            .collect::<Outcome<Vec<ReportRecord>>>()?;
        block.sort_unstable();
        expected.sort_unstable();
        assert!(block == expected, "FIXRPT-001: the six details of block 1");
        let account_total = records.get(10).ok_or("record 11")?;
        assert!(
            *account_total == total("Account Total", "+      1,453.87"),
            "FIXRPT-001: record 11"
        );
        assert_eq!(
            records.get(11).map(kind),
            Some(Kind::Dashes),
            "FIXRPT-001: record 12"
        );
        // E23: the descriptions are those of trantype.txt and trancatg.txt, never the Db2 seed.
        let sales = padded::<34>(b"0001-Regular Sales Draft");
        let to_account = padded::<34>(b"0001-Credit to Account");
        let purchase = (
            Some(b"01-Purchase       ".as_slice()),
            Some(sales.as_slice()),
        );
        let credit = (
            Some(b"03-Credit         ".as_slice()),
            Some(to_account.as_slice()),
        );
        for (position, record) in (1..).zip(&records) {
            if kind(record) != Kind::Detail || HEADER_GROUPS.contains(&position) {
                continue;
            }
            let lookups = (record.get(29..47), record.get(48..82));
            assert!(
                lookups == purchase || lookups == credit,
                "FIXRPT-903: descriptions of record {position}"
            );
        }
        Ok(())
    }

    // audit: CBTRN03C-022
    // audit: CBTRN03C-029
    // audit: CBTRN03C-030
    // audit: CBTRN03C-031
    #[test]
    fn the_last_amount_is_counted_twice_and_the_last_card_gets_no_total() -> TestResult {
        let mut run = small_run(DateParm::DEFAULT)?;
        let stamp = in_window()?;
        assert_eq!(run.process(&reported(1, 1, 1_000, stamp)?)?, Flow::Continue);
        assert_eq!(run.process(&reported(1, 2, 500, stamp)?)?, Flow::Continue);
        let display = run.end_of_file_display().ok_or("the end-of-file lines")?;
        assert_eq!(
            display,
            [
                "TRAN-AMT 0000000050{".to_owned(),
                "WS-PAGE-TOTAL0000000150{".to_owned(),
            ]
        );
        let records = run.finish()?;
        let kinds: Vec<Kind> = records.iter().skip(4).map(kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Detail,
                Kind::Detail,
                Kind::PageTotal,
                Kind::Dashes,
                Kind::GrandTotal,
            ]
        );
        // 10.00 + 5.00, then 5.00 again: 20.00 on the page and in the grand total.
        let page = records.get(6).ok_or("the page total")?;
        assert!(*page == total("Page Total ", "+         20.00"));
        let grand = records.get(8).ok_or("the grand total")?;
        assert!(*grand == total("Grand Total", "+         20.00"));
        Ok(())
    }

    // audit: CBTRN03C-014
    // audit: CBTRN03C-016
    // audit: CBTRN03C-018
    // audit: CBTRN03C-035
    // audit: CBTRN03C-036
    #[test]
    fn a_failed_lookup_ends_the_run_with_its_lines() -> TestResult {
        let stamp = in_window()?;
        let unknown_type = TransactionRecord {
            type_cd: PicX::from_array(*b"09"),
            ..reported(1, 1, 100, stamp)?
        };
        let unknown_category = TransactionRecord {
            cat_cd: PicU::new(9)?,
            ..reported(1, 1, 100, stamp)?
        };
        let cases = [
            (
                reported(2, 1, 100, stamp)?,
                "INVALID CARD NUMBER : ************0002",
            ),
            (unknown_type, "INVALID TRANSACTION TYPE : 09"),
            (unknown_category, "INVALID TRAN CATG KEY : 010009"),
        ];
        for (tran, line) in cases {
            let mut run = small_run(DateParm::DEFAULT)?;
            let error = run.process(&tran).err().ok_or(line)?;
            let lines = error.job_log_lines().ok_or(line)?;
            assert_eq!(
                lines,
                [line.to_owned(), "FILE STATUS IS: NNNN0023".to_owned()]
            );
            assert_eq!(run.process(&tran).err(), Some(error), "the run stays ended");
            assert_eq!(run.end_of_file_display(), None);
            assert_eq!(run.finish().err(), Some(error));
        }
        assert_eq!(ReportError::InvalidCardNumber.job_log_lines(), None);
        Ok(())
    }

    // audit: CBTRN03C-011
    #[test]
    fn a_record_outside_the_window_leaves_the_loop_without_totals() -> TestResult {
        let mut run = small_run(DateParm::DEFAULT)?;
        let inside = in_window()?;
        let outside = Some((instant((2023, 1, 1), (0, 0, 0))?, TsForm::Db2));
        assert_eq!(run.process(&reported(1, 1, 100, inside)?)?, Flow::Continue);
        assert_eq!(run.process(&reported(1, 2, 100, outside)?)?, Flow::Stop);
        assert_eq!(run.process(&reported(1, 3, 100, inside)?)?, Flow::Stop);
        assert_eq!(run.end_of_file_display(), None);
        let records = run.finish()?;
        let kinds: Vec<Kind> = records.iter().map(kind).collect();
        assert_eq!(
            kinds,
            [
                Kind::Header,
                Kind::Header,
                Kind::Header,
                Kind::Dashes,
                Kind::Detail,
            ],
            "the header group and the one detail, no total"
        );
        Ok(())
    }
}

/// CBPAUP0J, `domain::batch::cbpaup0c` (`modern/audit/batch/CBPAUP0J.md`).
mod cbpaup0j {
    use chrono::NaiveDateTime;
    use copybook::{Count, FixedRecord, PicS, PicU, PicX};
    use domain::FixedClock;
    use domain::batch::cbpaup0c::{
        ABEND_RETURN_CODE, CheckpointAction, DEFAULT_SYSIN, DebugFlag, DetailAction,
        NORMAL_RETURN_CODE, PRM_INFO_LEN, Purge, PurgeFailure, PurgeHalt, PurgeParams,
        SYSIN_RECORD_LEN, SummaryAction, SummaryDisposition, SysinError,
    };
    use domain::clock::{inverted_auth_keys, yyddd};
    use domain::records::{PendingAuthDetailRecord, PendingAuthSummaryRecord};

    use super::{Outcome, TestResult, instant, money};

    /// 2025-02-09 at noon, `YYDDD` 25040, the worked example of CBPAUP0C-012.
    fn today() -> Outcome<NaiveDateTime> {
        instant((2025, 2, 9), (12, 0, 0))
    }

    fn purge(sysin: &str) -> Outcome<Purge> {
        Ok(Purge::new(
            PurgeParams::parse(sysin.as_bytes())?,
            &FixedClock::new(today()?),
        ))
    }

    /// A summary of account `acct_id` with the given counts.
    fn summary(acct_id: i64, approved: i16, declined: i16) -> Outcome<PendingAuthSummaryRecord> {
        let mut record = PendingAuthSummaryRecord::initialized(PicS::new(acct_id)?, PicU::new(9)?);
        record.approved_auth_cnt = Count::new(approved);
        record.declined_auth_cnt = Count::new(declined);
        record.approved_auth_amt = money(30_000)?;
        record.declined_auth_amt = money(25_000)?;
        Ok(record)
    }

    /// The 200-byte `PAUTDTL1` segment of a detail whose keys, counts and amounts are zero and
    /// whose text fields, the card number among them, are blank (CIPAUDTY-001 to CIPAUDTY-028).
    fn blank_segment() -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PendingAuthDetailRecord::WIDTH);
        bytes.extend([0x00, 0x00, 0x0C]);
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x0C]);
        bytes.extend([b' '; 60]);
        bytes.extend(*b"000000");
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C]);
        bytes.extend([0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C]);
        bytes.extend([b' '; 7]);
        bytes.extend(*b"00");
        bytes.extend([b' '; 103]);
        bytes
    }

    /// A detail authorized at `authorized`, keyed as CP00 keys it, with response `resp`.
    fn detail(authorized: NaiveDateTime, resp: [u8; 2]) -> Outcome<PendingAuthDetailRecord> {
        let (auth_date_9c, auth_time_9c) = inverted_auth_keys(authorized);
        Ok(PendingAuthDetailRecord {
            auth_date_9c,
            auth_time_9c,
            auth_resp_code: PicX::from_array(resp),
            transaction_amt: money(12_500)?,
            approved_amt: money(12_500)?,
            ..PendingAuthDetailRecord::decode(&blank_segment())?
        })
    }

    // audit: CBPAUP0C-002
    // audit: CBPAUP0C-004
    // audit: CBPAUP0C-005
    // audit: CBPAUP0C-006
    // audit: CBPAUP0C-007
    #[test]
    fn the_default_card_and_the_defaults_of_a_blank_one() -> TestResult {
        assert_eq!(DEFAULT_SYSIN, "00,00001,00001,Y");
        assert_eq!((SYSIN_RECORD_LEN, PRM_INFO_LEN), (80, 17));
        let card = PurgeParams::parse(DEFAULT_SYSIN.as_bytes())?;
        assert_eq!(card.raw(), b"00,00001,00001,Y ");
        assert_eq!(card.expiry_days(), 0);
        assert_eq!(
            (card.chkp_freq(), card.chkp_dis_freq()),
            (*b"00001", *b"00001")
        );
        assert_eq!(card.debug(), DebugFlag::On);
        let blank = PurgeParams::parse(b"")?;
        assert_eq!(blank.raw(), &[b' '; 17]);
        assert_eq!(blank.expiry_days(), 5);
        assert_eq!(
            (blank.chkp_freq(), blank.chkp_dis_freq()),
            (*b"5    ", *b"10   ")
        );
        assert_eq!(blank.debug(), DebugFlag::Off);
        let kept = PurgeParams::parse(b" 5,00000,0    ,y")?;
        assert_eq!(kept.expiry_days(), 5, "' 5' is not NUMERIC");
        assert_eq!(kept.chkp_freq(), *b"00000", "00000 is not the literal 0");
        assert_eq!(kept.chkp_dis_freq(), *b"10   ", "0 is replaced");
        assert_eq!(kept.debug(), DebugFlag::Off, "only Y turns debug on");
        let long = [b' '; 81];
        assert_eq!(
            PurgeParams::parse(&long),
            Err(SysinError::TooLong { len: 81 })
        );
        assert_eq!(
            PurgeParams::parse(b"00,0000\n"),
            Err(SysinError::NotPrintable { offset: 7 })
        );
        Ok(())
    }

    // audit: CBPAUP0C-001
    // audit: CBPAUP0C-003
    #[test]
    fn a_run_reads_today_once_and_opens_with_five_lines() -> TestResult {
        let run = purge(DEFAULT_SYSIN)?;
        assert_eq!(run.today(), 25_040);
        assert_eq!(run.today(), yyddd(today()?));
        assert_eq!(
            run.sysout(),
            [
                "STARTING PROGRAM CBPAUP0C::",
                "*-------------------------------------*",
                "CBPAUP0C PARM RECEIVED :00,00001,00001,Y ",
                "TODAYS DATE            :25040",
                " ",
            ]
        );
        assert_eq!(run.params().expiry_days(), 0);
        Ok(())
    }

    // audit: CBPAUP0C-010
    // audit: CBPAUP0C-011
    // audit: CBPAUP0C-012
    // audit: CBPAUP0C-013
    // audit: CBPAUP0C-014
    // audit: CBPAUP0C-015
    #[test]
    fn a_detail_expires_by_its_yyddd_difference() -> TestResult {
        // T 25040 and A 25035 give 5: deleted with expiry 5, kept with expiry 6.
        let five_days = detail(instant((2025, 2, 4), (8, 30, 0))?, *b"00")?;
        let four_days = detail(instant((2025, 2, 5), (8, 30, 0))?, *b"00")?;
        let mut five = purge("05,00001,00001,N")?;
        let mut pass = five.begin_summary(&summary(11, 2, 1)?);
        assert_eq!(pass.on_detail(&five_days)?, DetailAction::Delete);
        assert_eq!(
            pass.working().approved_auth_cnt.get(),
            1,
            "approved count less one"
        );
        assert_eq!(pass.on_detail(&four_days)?, DetailAction::Keep);
        assert_eq!(
            pass.working().approved_auth_cnt.get(),
            1,
            "a kept detail changes nothing"
        );
        let declined = detail(instant((2025, 2, 1), (8, 30, 0))?, *b"05")?;
        assert_eq!(pass.on_detail(&declined)?, DetailAction::Delete);
        assert_eq!(
            pass.working().approved_auth_cnt.get(),
            1,
            "a decline leaves approvals"
        );
        assert_eq!(
            pass.working().declined_auth_cnt.get(),
            0,
            "declined count less one"
        );
        let mut six = purge("06,00001,00001,N")?;
        let mut pass = six.begin_summary(&summary(11, 2, 0)?);
        assert_eq!(pass.on_detail(&five_days)?, DetailAction::Keep);
        // With expiry 0, today's own authorization already qualifies.
        let mut zero = purge(DEFAULT_SYSIN)?;
        let mut pass_zero = zero.begin_summary(&summary(12, 1, 0)?);
        assert_eq!(
            pass_zero.on_detail(&detail(today()?, *b"00")?)?,
            DetailAction::Delete
        );
        let tomorrow = detail(instant((2025, 2, 10), (8, 0, 0))?, *b"00")?;
        assert_eq!(pass_zero.on_detail(&tomorrow)?, DetailAction::Keep);
        Ok(())
    }

    // audit: CBPAUP0C-016
    // audit: CBPAUP0C-017
    #[test]
    fn the_delete_test_reads_only_the_approved_count_and_nothing_is_written_back() -> TestResult {
        let mut run = purge("05,00001,00001,N")?;
        let old = detail(instant((2025, 1, 1), (9, 0, 0))?, *b"00")?;
        let keep = |summary| SummaryAction {
            delete_summary: summary,
            checkpoint: CheckpointAction::NotDue,
        };
        // Approved 0 and declined 3: deleted, as the condition names the approved count twice.
        let none_approved = run.begin_summary(&summary(21, 0, 3)?).end_summary();
        assert_eq!(none_approved, keep(SummaryDisposition::Delete));
        // Approved 1, its approval expired: the working count reaches 0.
        let mut pass = run.begin_summary(&summary(22, 1, 0)?);
        assert_eq!(pass.on_detail(&old)?, DetailAction::Delete);
        assert_eq!(pass.end_summary(), keep(SummaryDisposition::Delete));
        // Approved 2, one expired: kept, and the action carries no update of the summary.
        let stored = summary(23, 2, 0)?;
        let mut pass = run.begin_summary(&stored);
        assert_eq!(pass.on_detail(&old)?, DetailAction::Delete);
        assert_eq!(pass.end_summary(), keep(SummaryDisposition::Keep));
        assert_eq!(
            stored,
            summary(23, 2, 0)?,
            "the stored summary is unchanged"
        );
        let report = run.finish();
        assert_eq!(
            (report.summaries_read(), report.summaries_deleted()),
            (3, 2)
        );
        assert_eq!((report.details_read(), report.details_deleted()), (2, 2));
        Ok(())
    }

    // audit: CBPAUP0C-009
    // audit: CBPAUP0C-018
    // audit: CBPAUP0C-019
    // audit: CBPAUP0C-020
    #[test]
    fn checkpoints_compare_the_count_with_the_card_as_text() -> TestResult {
        // `00001`: 00000999 is below `00001   `, 00001000 above it, so the 1000th summary
        // takes the first checkpoint.
        let mut run = purge(DEFAULT_SYSIN)?;
        let stored = summary(1, 1, 0)?;
        for count in 1..=1_000 {
            let action = run.begin_summary(&stored).end_summary();
            let due = if count == 1_000 {
                CheckpointAction::Take
            } else {
                CheckpointAction::NotDue
            };
            assert_eq!(action.checkpoint, due, "summary {count}");
        }
        run.checkpoint_taken();
        assert!(
            !run.sysout()
                .iter()
                .any(|line| line.starts_with("CHKP SUCCESS")),
            "00000001 is below the display frequency 00001"
        );
        let report = run.finish();
        assert_eq!(report.checkpoints(), 2, "the 1000th summary and the end");
        // `00000`: every summary is above `00000   `.
        let mut every = purge("00,00000,00001,N")?;
        let action = every.begin_summary(&stored).end_summary();
        assert_eq!(action.checkpoint, CheckpointAction::Take);
        // A display frequency of `00000` shows the CHKP SUCCESS line at the first checkpoint.
        let mut shown = purge("00,00001,00000,N")?;
        let _ = shown.begin_summary(&summary(77, 1, 0)?).end_summary();
        shown.checkpoint_taken();
        assert_eq!(
            shown.sysout().last().map(String::as_str),
            Some("CHKP SUCCESS: AUTH COUNT - 00000001, APP ID - 00000000077")
        );
        Ok(())
    }

    // audit: CBPAUP0C-008
    // audit: CBPAUP0C-021
    // audit: CBPAUP0C-022
    #[test]
    fn a_normal_end_shows_the_debug_lines_and_the_totals() -> TestResult {
        let mut run = purge(DEFAULT_SYSIN)?;
        let mut first = run.begin_summary(&summary(11, 1, 0)?);
        assert_eq!(
            first.on_detail(&detail(today()?, *b"00")?)?,
            DetailAction::Delete
        );
        let _ = first.end_summary();
        let mut second = run.begin_summary(&summary(12, 2, 0)?);
        let tomorrow = detail(instant((2025, 2, 10), (8, 0, 0))?, *b"00")?;
        assert_eq!(second.on_detail(&tomorrow)?, DetailAction::Keep);
        let _ = second.end_summary();
        let report = run.finish();
        assert_eq!(report.return_code(), NORMAL_RETURN_CODE);
        assert_eq!(NORMAL_RETURN_CODE, 0);
        let lines = report
            .sysout()
            .get(5..)
            .ok_or("the lines after the opening five")?;
        assert_eq!(
            lines,
            [
                "DEBUG: AUTH SMRY READ : 00000000",
                "DEBUG: AUTH DTL READ : 00000000",
                "DEBUG: AUTH DTL DLET : 00000000011",
                "DEBUG: AUTH DTL READ : 00000001",
                "DEBUG: AUTH SMRY DLET : 00000000011",
                "DEBUG: AUTH SMRY READ : 00000001",
                "DEBUG: AUTH DTL READ : 00000001",
                "DEBUG: AUTH DTL READ : 00000002",
                "DEBUG: AUTH SMRY READ : 00000002",
                " ",
                "*-------------------------------------*",
                "# TOTAL SUMMARY READ  :00000002",
                "# SUMMARY REC DELETED :00000001",
                "# TOTAL DETAILS READ  :00000002",
                "# DETAILS REC DELETED :00000001",
                "*-------------------------------------*",
                " ",
            ]
        );
        Ok(())
    }

    // audit: CBPAUP0C-023
    // audit: CBPAUP0C-024
    // audit: CBPAUP0C-025
    // audit: CBPAUP0C-026
    // audit: CBPAUP0C-027
    // audit: CBPAUP0C-028
    #[test]
    fn a_failed_command_abends_with_return_code_16() -> TestResult {
        assert_eq!(ABEND_RETURN_CODE, 16);
        // The two status characters after each failure label are HALTED(H4): only the label is
        // compared on those lines.
        let cases: [(PurgeFailure, &str, &[&str]); 5] = [
            (
                PurgeFailure::SummaryRead,
                "AUTH SUMMARY READ FAILED  :",
                &["SUMMARY READ BEFORE ABEND :00000001"],
            ),
            (
                PurgeFailure::DetailRead,
                "AUTH DETAIL READ FAILED  :",
                &[
                    "SUMMARY AUTH APP ID      :00000000011",
                    "DETAIL READ BEFORE ABEND :00000000",
                ],
            ),
            (
                PurgeFailure::DetailDelete,
                "AUTH DETAIL DELETE FAILED :",
                &["AUTH APP ID               :00000000011"],
            ),
            (
                PurgeFailure::SummaryDelete,
                "AUTH SUMMARY DELETE FAILED :",
                &["AUTH APP ID                :00000000011"],
            ),
            (PurgeFailure::Checkpoint, "CHKP FAILED: DIBSTAT - ", &[]),
        ];
        for (failure, label, after) in cases {
            let mut run = purge("05,00001,00001,N")?;
            let _ = run.begin_summary(&summary(11, 1, 0)?).end_summary();
            let report = run.abend(failure);
            assert_eq!(report.return_code(), ABEND_RETURN_CODE, "{label}");
            let lines = report.sysout().get(5..).ok_or("the failure lines")?;
            let (status, rest) = lines.split_first().ok_or(label)?;
            assert!(status.starts_with(label), "{label}");
            let (abending, middle) = rest.split_last().ok_or(label)?;
            assert_eq!(middle, after, "{label}");
            assert_eq!(abending, "CBPAUP0C ABENDING ...");
        }
        Ok(())
    }

    #[test]
    fn halted_differences_and_statuses_run_without_a_parity_claim() -> TestResult {
        // X-CBPAUP0J-H1 (a YYDDD difference beyond 9999) and HALTED(H4) (the DIBSTAT of a
        // failed final checkpoint): exercised, nothing they produce is asserted.
        let mut run = purge("05,00001,00001,Y")?;
        let mut pass = run.begin_summary(&summary(31, 1, 0)?);
        let ancient = detail(instant((2013, 2, 9), (8, 0, 0))?, *b"00")?;
        let _: Result<DetailAction, PurgeHalt> = pass.on_detail(&ancient);
        let _ = pass.end_summary();
        let _ = run.abend(PurgeFailure::FinalCheckpoint);
        Ok(())
    }
}

/// MNTTRDB2, `domain::batch::cobtupdt` (`modern/audit/batch/MNTTRDB2.md`).
mod mnttrdb2 {
    use copybook::{ErrorKind, PicX};
    use domain::batch::cobtupdt::{
        INPUT_RECORD_LEN, Maintenance, MaintenanceAction, OpenStatus, SqlOutcome, decode_input,
        parse_input,
    };

    use super::{Outcome, TestResult, padded};

    /// The derived records of COBTUPDT-101 to COBTUPDT-110: action and code, then description.
    const RECORDS: [(&str, &str); 10] = [
        ("*  ", "AUDIT TEST INPUT FOR MNTTRDB2"),
        ("A08", "AUDIT TEST TYPE EIGHT"),
        ("U01", "AUDIT TEST PURCHASE RENAMED"),
        ("U99", "AUDIT TEST ABSENT TYPE"),
        ("D08", ""),
        ("D99", ""),
        ("X01", "AUDIT TEST NOT APPLIED"),
        ("U98", "AUDIT TEST ABSENT TYPE TWO"),
        ("A01", "AUDIT TEST DUPLICATE KEY"),
        ("D02", ""),
    ];

    /// The outcome each record's statement meets on a freshly migrated database, as the rows
    /// state it; `None` where no statement runs.
    const OUTCOMES: [Option<SqlOutcome>; 10] = [
        None,
        Some(SqlOutcome::Ok),
        Some(SqlOutcome::Ok),
        Some(SqlOutcome::NotFound),
        Some(SqlOutcome::Ok),
        Some(SqlOutcome::NotFound),
        None,
        Some(SqlOutcome::NotFound),
        Some(SqlOutcome::Failed),
        Some(SqlOutcome::Failed),
    ];

    /// The fixed part of the SQL error text, COBTUPDT-019: columns 1-49.
    const SQL_ERROR: &str = "Error accessing: TRANSACTION_TYPE table. SQLCODE:";

    /// The 53 bytes of a record: action and code, then the description padded to 50.
    fn body(head: &str, description: &str) -> [u8; INPUT_RECORD_LEN] {
        let mut text = head.as_bytes().to_vec();
        text.extend_from_slice(&padded::<50>(description.as_bytes()));
        padded(&text)
    }

    fn bodies() -> Vec<[u8; INPUT_RECORD_LEN]> {
        RECORDS
            .iter()
            .map(|(head, description)| body(head, description))
            .collect()
    }

    /// `WS-RETURN-MSG` as displayed: all 80 bytes.
    fn message(text: &str) -> String {
        format!("{text:<80}")
    }

    /// The `PROCESSING` line of a record (COBTUPDT-006).
    fn processing(body: &[u8; INPUT_RECORD_LEN]) -> String {
        format!("PROCESSING   {}", String::from_utf8_lossy(body))
    }

    /// The job log of COBTUPDT-111 after `OPEN FILE OK`; `None` marks a line whose columns
    /// 50-54 hold the `HALTED(H3)` SQLCODE.
    fn expected_log(bodies: &[[u8; INPUT_RECORD_LEN]]) -> Outcome<Vec<Option<String>>> {
        let line = |text: &str| Some(text.to_owned());
        let body = |index: usize| bodies.get(index).ok_or("ten records");
        Ok(vec![
            Some(processing(body(0)?)),
            line("IGNORING COMMENTED LINE"),
            Some(processing(body(1)?)),
            line("ADDING RECORD"),
            line("RECORD INSERTED SUCCESSFULLY"),
            Some(processing(body(2)?)),
            line("UPDATING RECORD"),
            line("RECORD UPDATED SUCCESSFULLY"),
            Some(processing(body(3)?)),
            line("UPDATING RECORD"),
            Some(message("No records found.")),
            Some(processing(body(4)?)),
            line("DELETING RECORD"),
            line("RECORD DELETED SUCCESSFULLY"),
            Some(processing(body(5)?)),
            line("DELETING RECORD"),
            Some(message("No records found.")),
            Some(processing(body(6)?)),
            Some(message("ERROR: TYPE NOT VALID")),
            Some(processing(body(7)?)),
            line("UPDATING RECORD"),
            Some(message("No records found.ALID")),
            Some(processing(body(8)?)),
            line("ADDING RECORD"),
            None,
            Some(processing(body(9)?)),
            line("DELETING RECORD"),
            None,
        ])
    }

    // audit: COBTUPDT-002
    // audit: COBTUPDT-003
    #[test]
    fn an_input_record_is_53_printable_bytes_split_into_its_three_fields() -> TestResult {
        assert_eq!(INPUT_RECORD_LEN, 53);
        let bodies = bodies();
        let added = bodies.get(1).ok_or("record 2")?;
        let record = parse_input(added)?;
        assert_eq!(record.kind_byte(), b'A');
        assert_eq!(record.code(), PicX::from_array(*b"08"));
        assert_eq!(
            record.description(),
            PicX::from_array(padded(b"AUDIT TEST TYPE EIGHT"))
        );
        assert_eq!(record.raw(), added);
        // LF lines, one CRLF line and a last line without LF.
        let mut file = Vec::new();
        for (index, body) in bodies.iter().enumerate() {
            file.extend_from_slice(body);
            match index {
                3 => file.extend_from_slice(b"\r\n"),
                9 => {}
                _ => file.push(b'\n'),
            }
        }
        let decoded = decode_input(&file)?;
        assert_eq!(decoded.len(), 10);
        for (decoded, body) in decoded.iter().zip(&bodies) {
            assert_eq!(decoded.raw(), body);
        }
        let short = parse_input(added.get(..52).ok_or("52 bytes")?).err();
        let short = short.ok_or("a 52-byte body is refused")?;
        assert_eq!(short.line, None);
        assert!(matches!(short.cause.kind, ErrorKind::WidthMismatch { .. }));
        let mut tabbed = *added;
        if let Some(byte) = tabbed.get_mut(10) {
            *byte = b'\t';
        }
        let tab = parse_input(&tabbed)
            .err()
            .ok_or("a control byte is refused")?;
        assert_eq!(tab.cause.kind, ErrorKind::NonPrintable);
        let second = [added.as_slice(), b"\n", &tabbed, b"\n"].concat();
        let error = decode_input(&second).err().ok_or("line 2 is refused")?;
        assert_eq!(error.line, Some(2));
        Ok(())
    }

    // audit: COBTUPDT-004
    // audit: COBTUPDT-006
    // audit: COBTUPDT-007
    // audit: COBTUPDT-008
    // audit: COBTUPDT-009
    // audit: COBTUPDT-010
    // audit: COBTUPDT-012
    // audit: COBTUPDT-013
    // audit: COBTUPDT-015
    // audit: COBTUPDT-016
    // audit: COBTUPDT-019
    // audit: COBTUPDT-021
    // audit: COBTUPDT-022
    // audit: COBTUPDT-023
    // audit: COBTUPDT-101
    // audit: COBTUPDT-102
    // audit: COBTUPDT-103
    // audit: COBTUPDT-104
    // audit: COBTUPDT-105
    // audit: COBTUPDT-106
    // audit: COBTUPDT-107
    // audit: COBTUPDT-108
    // audit: COBTUPDT-109
    // audit: COBTUPDT-110
    // audit: COBTUPDT-111
    #[test]
    fn the_audited_input_shows_29_lines_and_returns_four() -> TestResult {
        let bodies = bodies();
        let mut run = Maintenance::new();
        let mut log = Maintenance::open_lines(OpenStatus::Ok);
        let mut actions = Vec::new();
        for (body, outcome) in bodies.iter().zip(OUTCOMES) {
            let (shown, action) = run.plan(&parse_input(body)?);
            log.extend(shown);
            if let Some(outcome) = outcome {
                log.extend(run.complete(&action, outcome));
            }
            actions.push(action);
        }
        assert_eq!(log.len(), 29, "COBTUPDT-111: 29 lines");
        assert_eq!(log.first().map(String::as_str), Some("OPEN FILE OK"));
        let expected = expected_log(&bodies)?;
        for (n, (line, wanted)) in (2..).zip(log.iter().skip(1).zip(&expected)) {
            if let Some(wanted) = wanted {
                assert_eq!(line, wanted, "job-log line {n}");
                continue;
            }
            // Columns 50-54 are the HALTED(H3) SQLCODE: only the rest is compared.
            let bytes = line.as_bytes();
            assert_eq!(bytes.len(), 80, "job-log line {n}");
            assert!(bytes.get(..49) == Some(SQL_ERROR.as_bytes()), "line {n}");
            assert!(bytes.get(54..) == Some([b' '; 26].as_slice()), "line {n}");
        }
        assert_eq!(run.return_code(), 4);
        assert_eq!(actions.first(), Some(&MaintenanceAction::Ignore));
        assert_eq!(actions.get(6), Some(&MaintenanceAction::InvalidType));
        assert_eq!(
            actions.get(4),
            Some(&MaintenanceAction::Delete(PicX::from_array(*b"08")))
        );
        let Some(MaintenanceAction::Insert(row)) = actions.get(1) else {
            return Err("record 2 inserts".into());
        };
        assert_eq!(row.tr_type().as_bytes(), b"08");
        assert_eq!(
            row.tr_description(),
            format!("{:<50}", "AUDIT TEST TYPE EIGHT")
        );
        let Some(MaintenanceAction::Update(row)) = actions.get(2) else {
            return Err("record 3 updates".into());
        };
        assert_eq!(row.tr_type().as_bytes(), b"01");
        assert_eq!(row.tr_description().len(), 50, "all 50 description bytes");
        Ok(())
    }

    // audit: COBTUPDT-025
    // audit: COBTUPDT-112
    // audit: COBTUPDT-113
    #[test]
    fn a_clean_run_and_an_empty_one_return_zero() -> TestResult {
        let bodies = bodies();
        let mut run = Maintenance::new();
        let mut log = Maintenance::open_lines(OpenStatus::Ok);
        for index in [0, 1, 2, 4] {
            let body = bodies.get(index).ok_or("records 1, 2, 3 and 5")?;
            let (shown, action) = run.plan(&parse_input(body)?);
            log.extend(shown);
            log.extend(run.complete(&action, SqlOutcome::Ok));
        }
        assert_eq!(log.len(), 12, "COBTUPDT-112: 12 lines");
        assert_eq!(run.return_code(), 0);
        assert!(decode_input(b"")?.is_empty(), "COBTUPDT-113: no record");
        let empty = Maintenance::new();
        assert_eq!(Maintenance::open_lines(OpenStatus::Ok), ["OPEN FILE OK"]);
        assert_eq!(empty.return_code(), 0);
        assert_eq!(Maintenance::default(), empty);
        Ok(())
    }

    // audit: COBTUPDT-004
    // audit: COBTUPDT-018
    #[test]
    fn statements_with_no_matching_when_display_nothing() -> TestResult {
        assert_eq!(
            Maintenance::open_lines(OpenStatus::NotOk),
            ["OPEN FILE NOT OK"]
        );
        let mut run = Maintenance::new();
        let (_, insert) = run.plan(&parse_input(&body("A08", "AUDIT TEST TYPE EIGHT"))?);
        assert!(run.complete(&insert, SqlOutcome::NotFound).is_empty());
        assert!(
            run.complete(&MaintenanceAction::Ignore, SqlOutcome::Failed)
                .is_empty()
        );
        assert!(
            run.complete(&MaintenanceAction::InvalidType, SqlOutcome::Ok)
                .is_empty()
        );
        assert_eq!(run.return_code(), 0, "no WHEN, no return code");
        Ok(())
    }

    // audit: COBTUPDT-002
    // audit: COBTUPDT-007
    // audit: COBTUPDT-009
    #[test]
    fn only_upper_case_actions_dispatch_and_any_two_code_bytes_reach_the_sql() -> TestResult {
        for head in ["a08", "u01", "d01", " 01", "101"] {
            let mut run = Maintenance::new();
            let (shown, action) = run.plan(&parse_input(&body(head, "AUDIT TEST"))?);
            assert_eq!(
                action,
                MaintenanceAction::InvalidType,
                "action byte of `{head}`"
            );
            assert_eq!(
                shown.get(1),
                Some(&message("ERROR: TYPE NOT VALID")),
                "`{head}`"
            );
            assert_eq!(shown.len(), 2, "no action line for `{head}`");
            assert_eq!(run.return_code(), 4, "`{head}`");
        }
        let mut run = Maintenance::new();
        let (shown, action) = run.plan(&parse_input(&body("AZ!", "AUDIT TEST"))?);
        assert_eq!(shown.get(1).map(String::as_str), Some("ADDING RECORD"));
        let MaintenanceAction::Insert(row) = action else {
            return Err("an `A` record inserts whatever its code".into());
        };
        assert_eq!(row.tr_type().as_bytes(), b"Z!");
        let (_, action) = run.plan(&parse_input(&body("D  ", ""))?);
        assert_eq!(action, MaintenanceAction::Delete(PicX::from_array(*b"  ")));
        Ok(())
    }

    // audit: COBTUPDT-021
    // audit: COBTUPDT-022
    #[test]
    fn a_shorter_message_after_the_sql_error_text_keeps_its_tail() -> TestResult {
        let mut run = Maintenance::new();
        let (_, insert) = run.plan(&parse_input(&body("A01", "AUDIT TEST"))?);
        let failed = run.complete(&insert, SqlOutcome::Failed);
        let (_, update) = run.plan(&parse_input(&body("U99", "AUDIT TEST"))?);
        let missing = run.complete(&update, SqlOutcome::NotFound);
        let (invalid, _) = run.plan(&parse_input(&body("X01", "AUDIT TEST"))?);
        let overlays = [
            (failed.first(), SQL_ERROR),
            (
                missing.first(),
                "No records found.TRANSACTION_TYPE table. SQLCODE:",
            ),
            (
                invalid.get(1),
                "ERROR: TYPE NOT VALIDSACTION_TYPE table. SQLCODE:",
            ),
        ];
        for (n, (line, head)) in (1..).zip(overlays) {
            let bytes = line.ok_or("an error line")?.as_bytes();
            assert_eq!(bytes.len(), 80, "message {n}: all 80 bytes");
            // Columns 50-54 are the HALTED(H3) SQLCODE, left uncompared.
            assert_eq!(bytes.get(..49), Some(head.as_bytes()), "message {n}");
            assert_eq!(bytes.get(54..), Some([b' '; 26].as_slice()), "message {n}");
        }
        assert_eq!(failed.len(), 1);
        assert_eq!(missing.len(), 1);
        assert_eq!(run.return_code(), 4);
        Ok(())
    }
}

/// The nine loads, `domain::batch::loads` (`modern/audit/batch/LOADS.md`, FIXLOAD and FIXUSR).
mod loads {
    use copybook::FixedRecord;
    use copybook::framing::encode_all;
    use domain::batch::loads::{
        LoadErrorKind, LoadJob, check_key_sequence, decode_fixture, decode_usrsec, hash_user,
        usrsec_instream_lines,
    };
    use domain::password::verify;
    use domain::records::{
        AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DisclosureGroupRecord,
        TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord,
    };

    use super::{Outcome, TestResult, ascii, dusrsecj_jcl, padded};

    /// The ten in-stream users of FIXUSR-001 to FIXUSR-010: ID, first and last name, type.
    const USERS: [(&str, &str, &str, &str); 10] = [
        ("ADMIN001", "MARGARET", "GOLD", "A"),
        ("ADMIN002", "RUSSELL", "RUSSELL", "A"),
        ("ADMIN003", "RAYMOND", "WHITMORE", "A"),
        ("ADMIN004", "EMMANUEL", "CASGRAIN", "A"),
        ("ADMIN005", "GRANVILLE", "LACHAPELLE", "A"),
        ("USER0001", "LAWRENCE", "THOMAS", "U"),
        ("USER0002", "AJITH", "KUMAR", "U"),
        ("USER0003", "LAURITZ", "ALME", "U"),
        ("USER0004", "AVERARDO", "MAZZI", "U"),
        ("USER0005", "LEE", "TING", "U"),
    ];

    /// Decodes `bytes` as `job` loads them, checks the key sequence and the lossless re-encoding,
    /// and returns the record count.
    fn load<R: FixedRecord>(job: LoadJob, bytes: &[u8]) -> Outcome<usize> {
        let records = decode_fixture::<R>(bytes)?;
        check_key_sequence(job, &records)?;
        assert!(
            encode_all(&records) == bytes,
            "{}: re-encodes byte for byte",
            job.subcommand()
        );
        Ok(records.len())
    }

    /// The record count of a fixture job's input.
    fn fixture_count(job: LoadJob) -> Outcome<usize> {
        let bytes = ascii(job.fixture_file().ok_or("a fixture job")?)?;
        match job {
            LoadJob::Acctfile => load::<AccountRecord>(job, &bytes),
            LoadJob::Cardfile => load::<CardRecord>(job, &bytes),
            LoadJob::Xreffile => load::<CardXrefRecord>(job, &bytes),
            LoadJob::Custfile => load::<CustomerRecord>(job, &bytes),
            LoadJob::Discgrp => load::<DisclosureGroupRecord>(job, &bytes),
            LoadJob::Tcatbalf => load::<TranCatBalanceRecord>(job, &bytes),
            LoadJob::Trancatg => load::<TransactionCategoryRecord>(job, &bytes),
            LoadJob::Trantype => load::<TransactionTypeRecord>(job, &bytes),
            LoadJob::Dusrsecj => Err("dusrsecj reads DUSRSECJ.jcl".into()),
        }
    }

    /// A `trantype.txt`-shaped line for `code`.
    fn type_line(code: [u8; 2]) -> Vec<u8> {
        let mut line = code.to_vec();
        line.extend_from_slice(&padded::<50>(b"Purchase"));
        line.extend_from_slice(b"00000000\n");
        line
    }

    // audit: LOADS-001
    // audit: LOADS-002
    // audit: LOADS-003
    // audit: LOADS-004
    // audit: LOADS-005
    // audit: LOADS-006
    // audit: LOADS-007
    // audit: LOADS-008
    // audit: LOADS-009
    // audit: LOADS-010
    // audit: LOADS-029
    #[test]
    fn the_jobs_follow_the_dev_up_order_with_their_inputs_widths_keys_and_tables() {
        let expected = [
            ("acctfile", Some("acctdata.txt"), 300, 11, "accounts"),
            ("cardfile", Some("carddata.txt"), 150, 16, "cards"),
            ("xreffile", Some("cardxref.txt"), 50, 16, "card_xref"),
            ("custfile", Some("custdata.txt"), 500, 9, "customers"),
            ("discgrp", Some("discgrp.txt"), 50, 16, "disclosure_groups"),
            ("tcatbalf", Some("tcatbal.txt"), 50, 17, "tran_cat_balances"),
            (
                "trancatg",
                Some("trancatg.txt"),
                60,
                6,
                "transaction_categories",
            ),
            ("trantype", Some("trantype.txt"), 60, 2, "transaction_types"),
            ("dusrsecj", None, 80, 8, "users"),
        ];
        for (job, (subcommand, file, width, key, table)) in LoadJob::ALL.into_iter().zip(expected) {
            assert_eq!(job.subcommand(), subcommand);
            assert_eq!(job.fixture_file(), file, "{subcommand}");
            assert_eq!(job.record_width(), width, "{subcommand}");
            assert_eq!(job.key_len(), key, "{subcommand}");
            assert_eq!(job.target_table(), table, "{subcommand}");
        }
    }

    // audit: FIXLOAD-001
    // audit: FIXLOAD-002
    // audit: FIXLOAD-003
    // audit: FIXLOAD-004
    // audit: FIXLOAD-005
    // audit: FIXLOAD-006
    // audit: FIXLOAD-007
    // audit: FIXLOAD-008
    // audit: LOADS-017
    // audit: LOADS-030
    // audit: LOADS-031
    // audit: LOADS-032
    #[test]
    fn every_fixture_decodes_to_its_audited_count_in_key_order() -> TestResult {
        let counts = [50, 50, 50, 50, 51, 50, 18, 7];
        let jobs = LoadJob::ALL
            .into_iter()
            .filter(|job| job.fixture_file().is_some());
        let mut checked = 0;
        for (job, count) in jobs.zip(counts) {
            assert_eq!(fixture_count(job)?, count, "{}", job.subcommand());
            checked += 1;
        }
        assert_eq!(checked, 8);
        assert!(fixture_count(LoadJob::Dusrsecj).is_err());
        Ok(())
    }

    // audit: FIXLOAD-009
    // audit: FIXUSR-001
    // audit: FIXUSR-002
    // audit: FIXUSR-003
    // audit: FIXUSR-004
    // audit: FIXUSR-005
    // audit: FIXUSR-006
    // audit: FIXUSR-007
    // audit: FIXUSR-008
    // audit: FIXUSR-009
    // audit: FIXUSR-010
    // audit: LOADS-012
    // audit: LOADS-034
    #[test]
    fn the_user_deck_is_lines_35_to_44_padded_to_80_bytes() -> TestResult {
        let jcl = dusrsecj_jcl()?;
        let file_lines: Vec<&[u8]> = jcl.split(|&byte| byte == b'\n').collect();
        let opening = file_lines.get(33).ok_or("DUSRSECJ.jcl:34")?;
        assert!(opening.starts_with(b"//SYSUT1   DD *"), "DUSRSECJ.jcl:34");
        let deck = usrsec_instream_lines(&jcl)?;
        assert_eq!(deck.len(), 10);
        for (number, line) in (35..).zip(&deck) {
            let in_file = file_lines.get(number - 1).ok_or("the deck line")?;
            assert!(line.body == *in_file, "DUSRSECJ.jcl:{number}");
        }
        let users = decode_usrsec(&jcl)?;
        assert_eq!(users.len(), 10);
        for ((framed, line), (id, first, last, kind)) in users.iter().zip(&deck).zip(USERS) {
            let record = &framed.record;
            assert_eq!(*record.id.as_bytes(), padded::<8>(id.as_bytes()));
            assert_eq!(
                *record.fname.as_bytes(),
                padded::<20>(first.as_bytes()),
                "{id}"
            );
            assert_eq!(
                *record.lname.as_bytes(),
                padded::<20>(last.as_bytes()),
                "{id}"
            );
            assert_eq!(record.usr_type.as_bytes(), kind.as_bytes(), "{id}");
            assert!(record.filler.is_spaces(), "{id}: SEC-USR-FILLER is spaces");
            let image = padded::<80>(line.body);
            assert!(
                record.encode() == image,
                "FIXUSR: the 80-byte image of {id}"
            );
            let original = [line.body, line.framing.terminator.as_bytes()].concat();
            assert!(framed.encode() == original, "FIXUSR: the deck line of {id}");
        }
        Ok(())
    }

    // audit: LOADS-033
    #[test]
    fn a_loaded_password_is_kept_only_as_its_argon2id_hash() -> TestResult {
        let users = decode_usrsec(&dusrsecj_jcl()?)?;
        let first = users.into_iter().next().ok_or("ADMIN001")?.record;
        let password = *first.pwd.expose();
        // argon2id is slow in debug builds, so one user is hashed.
        let hashed = hash_user(first)?;
        assert!(hashed.password_hash.starts_with("$argon2id$"));
        assert!(
            !hashed.password_hash.contains("PASSWORD"),
            "E25: no plaintext"
        );
        assert!(
            verify(&password, &hashed.password_hash)?,
            "the hash verifies its 8 bytes"
        );
        let shown = format!("{hashed:?}");
        assert!(
            !shown.contains(&hashed.password_hash),
            "Debug hides the hash"
        );
        assert!(!shown.contains("PASSWORD"), "Debug hides the password");
        Ok(())
    }

    // audit: LOADS-013
    #[test]
    fn a_deck_without_its_delimiters_or_with_a_long_card_is_refused() -> TestResult {
        let none = usrsec_instream_lines(b"//DUSRSECJ JOB\n//STEP01 EXEC PGM=IEBGENER\n").err();
        let none = none.ok_or("no SYSUT1 statement")?;
        assert_eq!(
            (none.line(), none.kind()),
            (None, LoadErrorKind::InStreamStartMissing)
        );
        let open = usrsec_instream_lines(b"//DUSRSECJ JOB\n//SYSUT1   DD *\nADMIN001\n").err();
        let open = open.ok_or("no delimiter")?;
        assert_eq!(
            (open.line(), open.kind()),
            (Some(2), LoadErrorKind::InStreamEndMissing)
        );
        let long = [b"//SYSUT1   DD *\n".as_slice(), &[b'A'; 81], b"\n/*\n"].concat();
        let long = decode_usrsec(&long).err().ok_or("an 81-byte card")?;
        assert_eq!(long.line(), Some(2));
        assert!(matches!(long.kind(), LoadErrorKind::Record(_)));
        Ok(())
    }

    // audit: LOADS-016
    // audit: LOADS-017
    #[test]
    fn a_bad_line_or_an_out_of_order_key_fails_the_load() -> TestResult {
        let good = [type_line(*b"01"), type_line(*b"02")].concat();
        assert_eq!(load::<TransactionTypeRecord>(LoadJob::Trantype, &good)?, 2);
        let mut long = type_line(*b"02");
        long.insert(5, b'X');
        let bad = [type_line(*b"01"), long].concat();
        let error = decode_fixture::<TransactionTypeRecord>(&bad).err();
        let error = error.ok_or("a 61-byte body")?;
        assert_eq!(error.line(), Some(2));
        assert!(matches!(error.kind(), LoadErrorKind::Record(_)));
        for keys in [[*b"01", *b"01"], [*b"02", *b"01"]] {
            let bytes = [type_line(keys[0]), type_line(keys[1])].concat();
            let records = decode_fixture::<TransactionTypeRecord>(&bytes)?;
            let error = check_key_sequence(LoadJob::Trantype, &records).err();
            let error = error.ok_or("a key not above the previous one")?;
            assert_eq!(
                (error.line(), error.kind()),
                (Some(2), LoadErrorKind::KeySequence { previous_line: 1 })
            );
        }
        Ok(())
    }

    // audit: LOADS-030
    // audit: LOADS-031
    // audit: LOADS-032
    #[test]
    fn framing_and_filler_survive_and_nothing_else_is_accepted() -> TestResult {
        let xrefs = decode_fixture::<CardXrefRecord>(&ascii("cardxref.txt")?)?;
        for (line, framed) in (1..).zip(&xrefs) {
            assert_eq!(framed.framing.body_len, 36, "cardxref.txt:{line}");
            assert!(
                framed.record.filler.is_spaces(),
                "cardxref.txt:{line}: 14 spaces"
            );
        }
        let first = ascii("cardxref.txt")?;
        let body = first.get(..36).ok_or("cardxref.txt:1")?;
        // A body ending inside XREF-ACCT-ID, or past the 50 bytes of the copybook, is refused.
        for cut in [
            body.get(..30).ok_or("30 bytes")?.to_vec(),
            [body, &[b' '; 15]].concat(),
        ] {
            let line = [cut.as_slice(), b"\n"].concat();
            let error = decode_fixture::<CardXrefRecord>(&line).err();
            let error = error.ok_or("a body that ends inside a field or past 50 bytes")?;
            assert_eq!(error.line(), Some(1));
            assert!(matches!(error.kind(), LoadErrorKind::Record(_)));
        }
        let types = decode_fixture::<TransactionTypeRecord>(&ascii("trantype.txt")?)?;
        let fillers: Vec<[u8; 8]> = types
            .iter()
            .map(|framed| *framed.record.filler.as_bytes())
            .collect();
        assert!(
            fillers.iter().all(|filler| filler == b"00000000"),
            "E05: `0` FILLER kept"
        );
        let mut carriage = type_line(*b"01");
        if let Some(byte) = carriage.get_mut(9) {
            *byte = b'\r';
        }
        let error = decode_fixture::<TransactionTypeRecord>(&carriage).err();
        let error = error.ok_or("a CR inside a body")?;
        assert_eq!(error.line(), Some(1));
        assert!(matches!(error.kind(), LoadErrorKind::Record(_)));
        let crlf = [type_line(*b"01").get(..60).ok_or("60 bytes")?, b"\r\n"].concat();
        let decoded = decode_fixture::<TransactionTypeRecord>(&crlf)?;
        let record = decoded.first().ok_or("one CRLF line")?;
        assert!(
            record.record.encode() == crlf.get(..60).ok_or("the body")?,
            "CR is framing"
        );
        Ok(())
    }
}
