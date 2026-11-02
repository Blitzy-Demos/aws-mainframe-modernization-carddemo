//! `CBACT04C` (job `INTCALC`) through the public API of `carddemo-domain`: the
//! `1300-COMPUTE-INTEREST` arithmetic and the run rules of the main loop
//! [app/cbl/CBACT04C.cbl:L188-222].
//!
//! - **Arithmetic** (audit section C, P-CBACT04C-011 to P-CBACT04C-019):
//!   `WS-MONTHLY-INT = (TRAN-CAT-BAL * DIS-INT-RATE) / 1200` into
//!   `S9(09)V99`, without `ROUNDED` and without `ON SIZE ERROR`. The quotient is
//!   truncated toward zero to whole cents with its sign kept, the store keeps
//!   the low-order 9 integer digits, and a zero is stored positive (E-42). A
//!   property test checks the typed algebra against an independent
//!   `rust_decimal` division over every operand the two PIC clauses allow.
//! - **Run rules** (audit section B): the account break and its rewrite
//!   (`1050-UPDATE-ACCOUNT`), the `DEFAULT` disclosure group fallback that every
//!   fixture account takes (E-06), the zero rate that writes nothing, the
//!   `SYSTRAN` interest record (`1300-B-WRITE-TX`), the last account that is
//!   never rewritten (E-14) and the abend on a missing account or
//!   cross-reference.
//!
//! The rule is driven through an in-memory store with a runtime-free executor:
//! the domain crate has no async runtime, and its in-memory futures are ready
//! on the first poll.

use std::collections::BTreeMap;
use std::future::{self, Future};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use carddemo_domain::clock::FixedClock;
use carddemo_domain::entities::{
    Account, CardXref, DisclosureGroup, DisclosureGroupKey, NewTransaction, TranCatBalance,
    TranCatBalanceKey,
};
use carddemo_domain::messages::{
    ABENDING_PROGRAM, ERROR_READING_ACCOUNT_FILE, ERROR_READING_XREF_FILE,
};
use carddemo_domain::numeric::{
    AccountGroupId, AccountId, Alnum, CardNumber, CustomerId, InterestRate, MerchantId, Money,
    TranCategoryCode, TranId, TranTypeCode,
};
use carddemo_domain::ports::{
    AccountReader, AccountWriter, DisclosureGroupReader, PortError, PortResult, XrefReader,
};
use carddemo_domain::rules::interest::{
    InterestAbend, InterestRun, UnpostedInterest, monthly_interest,
};
use chrono::NaiveDate;
use proptest::test_runner::Config as ProptestConfig;
use proptest::{prop_assert_eq, proptest};
use rust_decimal::Decimal;

/// The polls [`block_on`] allows before it reports a future that never
/// completes; the in-memory store needs one per future.
const MAX_POLLS: usize = 16;

/// The card of fixture account 1 [app/data/ASCII/cardxref.txt:L49].
const CARD_1: &str = "9680294154603697";

/// The card of fixture account 2 [app/data/ASCII/cardxref.txt:L3].
const CARD_2: &str = "0923877193247330";

/// The `S9(09)V99` receiver of `WS-MONTHLY-INT` holds the value modulo `10^9`.
const MONTHLY_INT_MODULUS: i64 = 1_000_000_000;

/// The COBOL divisor of `1300-COMPUTE-INTEREST`.
const MONTHS_TIMES_PERCENT: i64 = 1_200;

/// Drives `future` to completion on the current thread with a waker that does
/// nothing: every future here is ready on its first poll, so a future still
/// pending after [`MAX_POLLS`] polls is a defect of the store, not a wait.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    for _ in 0..MAX_POLLS {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
    }
    panic!("future still pending after {MAX_POLLS} polls");
}

/// The three files `CBACT04C` reads or rewrites at random, in memory: the
/// account master, the cross-reference by account (`CXACAIX`) and the
/// disclosure groups, plus a log of every account rewrite and every
/// disclosure group key read, in call order.
#[derive(Default)]
struct MemoryStore {
    accounts: BTreeMap<AccountId, Account>,
    xrefs: BTreeMap<AccountId, CardXref>,
    groups: BTreeMap<DisclosureGroupKey, DisclosureGroup>,
    rewrites: Vec<Account>,
    group_reads: Vec<DisclosureGroupKey>,
}

impl MemoryStore {
    fn with_account(mut self, account: Account) -> Self {
        self.accounts.insert(account.acct_id, account);
        self
    }

    fn with_xref(mut self, xref: CardXref) -> Self {
        self.xrefs.insert(xref.xref_acct_id, xref);
        self
    }

    fn with_group(mut self, group: DisclosureGroup) -> Self {
        self.groups.insert(group.key.clone(), group);
        self
    }
}

/// The access `CBACT04C` never issues, which no test here reaches.
fn not_issued(access: &str) -> PortError {
    PortError::Unavailable(format!("CBACT04C issues no {access}"))
}

impl AccountReader for MemoryStore {
    fn read_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_ {
        future::ready(self.accounts.get(&id).cloned().ok_or(PortError::NotFound))
    }

    /// The locked re-read `1050-UPDATE-ACCOUNT` makes before its rewrite: the
    /// stored record, as the plain read returns it.
    fn read_account_for_update(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_ {
        future::ready(self.accounts.get(&id).cloned().ok_or(PortError::NotFound))
    }
}

impl AccountWriter for MemoryStore {
    fn rewrite_account(
        &mut self,
        account: Account,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        let result = if let Some(stored) = self.accounts.get_mut(&account.acct_id) {
            stored.clone_from(&account);
            self.rewrites.push(account);
            Ok(())
        } else {
            Err(PortError::NotFound)
        };
        future::ready(result)
    }
}

impl XrefReader for MemoryStore {
    fn read_xref_by_card(
        &mut self,
        _card: CardNumber,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_ {
        future::ready(Err(not_issued("READ of XREFFILE by card")))
    }

    fn read_xref_by_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_ {
        future::ready(self.xrefs.get(&id).cloned().ok_or(PortError::NotFound))
    }

    fn xrefs_after(
        &mut self,
        _after: Option<CardNumber>,
        _limit: u16,
    ) -> impl Future<Output = PortResult<Vec<CardXref>>> + Send + '_ {
        future::ready(Err(not_issued("sequential READ of XREFFILE")))
    }
}

impl DisclosureGroupReader for MemoryStore {
    fn read_disclosure_group(
        &mut self,
        key: DisclosureGroupKey,
    ) -> impl Future<Output = PortResult<DisclosureGroup>> + Send + '_ {
        let result = self.groups.get(&key).cloned().ok_or(PortError::NotFound);
        self.group_reads.push(key);
        future::ready(result)
    }
}

fn account_id(value: u64) -> AccountId {
    AccountId::new(value).expect("an account number of at most 11 digits")
}

fn cents<const D: u8>(value: i64) -> Money<D> {
    Money::from_cents(value).expect("an amount within the field")
}

fn rate(hundredths: i32) -> InterestRate {
    InterestRate::from_hundredths(hundredths).expect("a rate within S9(04)V99")
}

fn category(code: u16) -> TranCategoryCode {
    TranCategoryCode::new(code).expect("a four-digit category")
}

/// An account with a blank `ACCT-GROUP-ID`, as every fixture account (E-06),
/// and the given balance and cycle amounts in cents.
fn account(id: u64, balance: i64, cycle_credit: i64, cycle_debit: i64) -> Account {
    Account {
        acct_id: account_id(id),
        acct_active_status: Alnum::move_from("Y"),
        acct_curr_bal: cents(balance),
        acct_credit_limit: cents(2_020_000),
        acct_cash_credit_limit: cents(1_020_000),
        acct_open_date: Alnum::move_from("2014-11-20"),
        acct_expiraion_date: Alnum::move_from("2025-05-20"),
        acct_reissue_date: Alnum::move_from("2025-05-20"),
        acct_curr_cyc_credit: cents(cycle_credit),
        acct_curr_cyc_debit: cents(cycle_debit),
        acct_addr_zip: Alnum::move_from("A000000000"),
        acct_group_id: AccountGroupId::move_from(""),
    }
}

fn xref(id: u64, card: &str) -> CardXref {
    CardXref {
        xref_card_num: CardNumber::move_from(card),
        xref_cust_id: CustomerId::new(1).expect("a nine-digit customer"),
        xref_acct_id: account_id(id),
    }
}

fn group_key(group_id: &str, type_cd: &str, cat_cd: u16) -> DisclosureGroupKey {
    DisclosureGroupKey {
        dis_acct_group_id: AccountGroupId::move_from(group_id),
        dis_tran_type_cd: TranTypeCode::move_from(type_cd),
        dis_tran_cat_cd: category(cat_cd),
    }
}

fn group(group_id: &str, type_cd: &str, cat_cd: u16, hundredths: i32) -> DisclosureGroup {
    DisclosureGroup {
        key: group_key(group_id, type_cd, cat_cd),
        dis_int_rate: rate(hundredths),
    }
}

/// A `TCATBALF` record of `amount` cents.
fn balance(id: u64, type_cd: &str, cat_cd: u16, amount: i64) -> TranCatBalance {
    TranCatBalance {
        key: TranCatBalanceKey {
            trancat_acct_id: account_id(id),
            trancat_type_cd: TranTypeCode::move_from(type_cd),
            trancat_cd: category(cat_cd),
        },
        tran_cat_bal: cents(amount),
    }
}

/// The processing clock pinned to midnight UTC of the `INTCALC` date, whose
/// DB2-format timestamp is `2022-07-18-00.00.00.000000`.
fn clock() -> FixedClock {
    let instant = NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .expect("a valid instant");
    FixedClock(instant.and_utc())
}

/// The JCL `PARM='2022071800'` of `INTCALC` [app/jcl/INTCALC.jcl:L22]
/// (P-CBACT04C-001).
fn parm_date() -> Alnum<10> {
    Alnum::move_from("2022071800")
}

/// Every record through one run on the pinned clock.
fn process_all(
    run: &mut InterestRun,
    store: &mut MemoryStore,
    records: &[TranCatBalance],
) -> Result<Vec<Option<NewTransaction>>, InterestAbend> {
    let clock = clock();
    records
        .iter()
        .map(|record| block_on(run.process_record(store, record, &clock)))
        .collect()
}

/// The TRAN-IDs of the interest records returned, `None` where no record was.
fn tran_ids(outputs: &[Option<NewTransaction>]) -> Vec<Option<String>> {
    outputs
        .iter()
        .map(|output| output.as_ref().map(|tran| tran.tran_id.to_string()))
        .collect()
}

/// The proptest configuration: the default case count, and no failure
/// persistence, so a failing run writes no `proptest-regressions/` directory
/// into the checkout. The shrunk failing case is still printed.
fn proptest_config() -> ProptestConfig {
    ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// CBACT04C 1300-COMPUTE-INTEREST — P-CBACT04C-012 to P-CBACT04C-019,
/// P-CBACT04C-175 (E-42): `(TRAN-CAT-BAL * DIS-INT-RATE) / 1200` truncated
/// toward zero to whole cents, then stored into `WS-MONTHLY-INT S9(09)V99`
/// with its sign and its low-order 9 integer digits, a zero stored positive.
///
/// The exact quotient is also stored into a wider `S9(11)V99` receiver, which
/// drops no digit, so the high-order truncation of the maximal operands
/// (P-CBACT04C-019) shows against the value it came from. The ported
/// [`monthly_interest`] of the rule gives the same stored value.
#[test]
fn monthly_interest_truncates_toward_zero_and_keeps_nine_integer_digits() {
    // (TRAN-CAT-BAL cents, DIS-INT-RATE hundredths, quotient in S9(11)V99,
    // WS-MONTHLY-INT in S9(09)V99, audit row)
    let cases = [
        (1, 100, "0.00", "0.00", "P-CBACT04C-013"),
        (100, 1_500, "0.01", "0.01", "P-CBACT04C-014"),
        (120_000, 100, "1.00", "1.00", "P-CBACT04C-015"),
        (119_999, 100, "0.99", "0.99", "P-CBACT04C-016"),
        (-100, 1_500, "-0.01", "-0.01", "P-CBACT04C-017"),
        (-1, 100, "0.00", "0.00", "P-CBACT04C-018"),
        (
            99_999_999_999,
            999_999,
            "8333324999.91",
            "333324999.91",
            "P-CBACT04C-019",
        ),
        (
            -99_999_999_999,
            999_999,
            "-8333324999.91",
            "-333324999.91",
            "P-CBACT04C-019",
        ),
    ];
    for (balance_cents, hundredths, quotient, stored, row) in cases {
        let product = cents::<9>(balance_cents) * rate(hundredths);
        let wide = product.monthly().store::<11>();
        let monthly = product.monthly().store::<9>();
        assert_eq!(wide.to_string(), quotient, "{row}: S9(11)V99 receiver");
        assert_eq!(monthly.to_string(), stored, "{row}: S9(09)V99 receiver");
        assert_eq!(
            monthly.is_negative(),
            stored.starts_with('-'),
            "{row}: a zero is stored positive"
        );
        assert_eq!(
            monthly_interest(cents(balance_cents), rate(hundredths)),
            monthly,
            "{row}: the ported rule"
        );
    }
}

proptest! {
    #![proptest_config(proptest_config())]

    /// CBACT04C 1300-COMPUTE-INTEREST — P-CBACT04C-012: over every
    /// `S9(09)V99` balance and `S9(04)V99` rate, the monthly interest equals
    /// the exact decimal quotient truncated to cents, and its `S9(09)V99`
    /// store keeps that value's sign and low-order 9 integer digits.
    ///
    /// The reference division rounds only in its 28th significant digit, far
    /// below the cent: the quotient in cents is the product in `0.0001` units
    /// divided by 120 000, whose remainder leaves a fraction of at most
    /// 119 999 / 120 000 of a cent, so no rounding carries into the cent and
    /// the truncated reference is exact.
    #[test]
    fn monthly_is_the_truncated_decimal_quotient(
        balance_cents in -99_999_999_999_i64..=99_999_999_999,
        hundredths in -999_999_i32..=999_999,
    ) {
        let balance = cents::<9>(balance_cents);
        let annual_rate = rate(hundredths);
        let reference = (balance.to_decimal() * annual_rate.to_decimal()
            / Decimal::from(MONTHS_TIMES_PERCENT))
        .trunc_with_scale(2);
        let monthly = (balance * annual_rate).monthly();
        prop_assert_eq!(monthly.store::<11>().to_decimal(), reference);
        let low_order = Money::<9>::from_decimal(reference % Decimal::from(MONTHLY_INT_MODULUS))
            .expect("a remainder below 10^9 at scale 2");
        prop_assert_eq!(monthly.store::<9>(), low_order);
    }
}

/// CBACT04C 1200-GET-INTEREST-RATE, 1300-COMPUTE-INTEREST and 1300-B-WRITE-TX
/// — P-CBACT04C-004, P-CBACT04C-193, P-CBACT04C-007, P-CBACT04C-008,
/// P-CBACT04C-173, P-CBACT04C-176, E-06: a blank `ACCT-GROUP-ID` finds no
/// disclosure group, so the `DEFAULT` group of the same type and category
/// gives the rate ([app/data/ASCII/discgrp.txt:L18], 15.00), and the interest
/// on 1000.00 is returned as the first `SYSTRAN` record of the run.
///
/// The description is `Int. for a/c ` and the 11 digits of `ACCT-ID`, bytes 25
/// to 100 spaces (E-25); both timestamps are the DB2-format timestamp of the
/// processing clock.
#[test]
fn a_blank_group_reads_the_default_rate_and_returns_the_systran_record() {
    let mut store = MemoryStore::default()
        .with_account(account(1, 0, 0, 0))
        .with_xref(xref(1, CARD_1))
        .with_group(group("DEFAULT", "01", 1, 1_500));
    let mut run = InterestRun::new(parm_date());
    let outputs =
        process_all(&mut run, &mut store, &[balance(1, "01", 1, 100_000)]).expect("no abend");

    let timestamp = "2022-07-18-00.00.00.000000";
    let expected = NewTransaction {
        tran_id: TranId::move_from("2022071800000001"),
        tran_type_cd: TranTypeCode::move_from("01"),
        tran_cat_cd: category(5),
        tran_source: Alnum::move_from("System"),
        tran_desc: Alnum::move_from("Int. for a/c 00000000001"),
        tran_amt: cents(1_250),
        tran_merchant_id: MerchantId::new(0).expect("a nine-digit merchant"),
        tran_merchant_name: Alnum::spaces(),
        tran_merchant_city: Alnum::spaces(),
        tran_merchant_zip: Alnum::spaces(),
        tran_card_num: CardNumber::move_from(CARD_1),
        tran_orig_ts: Alnum::move_from(timestamp),
        tran_proc_ts: Alnum::move_from(timestamp),
    };
    assert_eq!(outputs, [Some(expected)]);
    let tran = outputs[0].as_ref().expect("an interest record");
    assert_eq!(tran.tran_cat_cd.to_string(), "0005");
    assert_eq!(tran.tran_source.as_bytes(), b"System    ");
    let (written, unset) = tran.tran_desc.as_bytes().split_at(24);
    assert_eq!(written, b"Int. for a/c 00000000001");
    assert!(unset.iter().all(|byte| *byte == b' '));
    assert_eq!(tran.tran_orig_ts.as_bytes(), timestamp.as_bytes());

    // The blank group ends status 23; the DEFAULT read keeps the record's type
    // and category.
    assert_eq!(
        store.group_reads,
        [group_key("", "01", 1), group_key("DEFAULT", "01", 1)]
    );
    assert!(store.group_reads[1].is_default_group());
    assert_eq!(run.finish().default_group_reads, 1);
}

/// CBACT04C 1200-GET-INTEREST-RATE — P-CBACT04C-004: an account whose own
/// group has a record for the balance's type and category takes that record's
/// rate, and the `DEFAULT` group is never read.
#[test]
fn an_existing_group_rate_wins_without_a_default_read() {
    let mut own = account(1, 0, 0, 0);
    own.acct_group_id = AccountGroupId::move_from("A000000000");
    let mut store = MemoryStore::default()
        .with_account(own)
        .with_xref(xref(1, CARD_1))
        .with_group(group("A000000000", "01", 1, 3_000))
        .with_group(group("DEFAULT", "01", 1, 1_500));
    let mut run = InterestRun::new(parm_date());
    let outputs =
        process_all(&mut run, &mut store, &[balance(1, "01", 1, 100_000)]).expect("no abend");

    // 1000.00 at the group's 30.00 is 25.00, not the DEFAULT 12.50.
    let amounts: Vec<Option<i64>> = outputs
        .iter()
        .map(|output| output.as_ref().map(|tran| tran.tran_amt.cents()))
        .collect();
    assert_eq!(amounts, [Some(2_500)]);
    assert_eq!(store.group_reads, [group_key("A000000000", "01", 1)]);
    assert_eq!(run.finish().default_group_reads, 0);
}

/// CBACT04C PROCEDURE DIVISION `IF DIS-INT-RATE NOT = 0` — P-CBACT04C-006,
/// P-CBACT04C-022: a zero rate (`DEFAULT` 02/0001,
/// [app/data/ASCII/discgrp.txt:L22]) computes nothing, returns no record and
/// leaves the TRAN-ID suffix alone, so the next rated balance (`DEFAULT`
/// 04/0001 at 15.00, [app/data/ASCII/discgrp.txt:L28]) gets suffix 000001.
#[test]
fn a_zero_rate_returns_nothing_and_keeps_the_suffix() {
    let mut store = MemoryStore::default()
        .with_account(account(1, 0, 0, 0))
        .with_xref(xref(1, CARD_1))
        .with_group(group("DEFAULT", "02", 1, 0))
        .with_group(group("DEFAULT", "04", 1, 1_500));
    let records = [balance(1, "02", 1, 50_000), balance(1, "04", 1, 100_000)];
    let mut run = InterestRun::new(parm_date());
    let outputs = process_all(&mut run, &mut store, &records).expect("no abend");

    assert_eq!(
        tran_ids(&outputs),
        [None, Some("2022071800000001".to_owned())]
    );
    let summary = run.finish();
    assert_eq!((summary.records_read, summary.interest_records), (2, 1));
    // WS-TOTAL-INT holds only the rated balance's 12.50.
    assert_eq!(
        summary.unposted,
        Some(UnpostedInterest {
            acct_id: account_id(1),
            total_interest: cents(1_250),
        })
    );
}

/// CBACT04C PROCEDURE DIVISION and 1050-UPDATE-ACCOUNT — P-CBACT04C-002,
/// P-CBACT04C-010, P-CBACT04C-020, P-CBACT04C-021, P-CBACT04C-190,
/// P-CBACT04C-191, P-CBACT04C-192, P-CBACT04C-003, P-CBACT04C-174, E-14.
///
/// Account 1 earns 12.50 and 0.01 over two balances and is rewritten only when
/// account 2's first balance breaks the account: its balance plus 12.51, both
/// cycle amounts zero. `WS-TOTAL-INT` restarts at zero for account 2, whose
/// card the new cross-reference read supplies. At end of file the loop exits
/// before the `ELSE` that would rewrite account 2, so the last account keeps
/// its balance and cycle amounts. The TRAN-ID suffix runs across accounts.
#[test]
fn an_account_break_rewrites_the_previous_account_and_never_the_last() {
    let mut store = MemoryStore::default()
        .with_account(account(1, 100_000, 25_000, -4_000))
        .with_account(account(2, 200_000, 30_000, -5_000))
        .with_xref(xref(1, CARD_1))
        .with_xref(xref(2, CARD_2))
        .with_group(group("DEFAULT", "01", 1, 1_500))
        .with_group(group("DEFAULT", "04", 1, 1_500));
    let records = [
        balance(1, "01", 1, 100_000),
        balance(1, "04", 1, 100),
        balance(2, "01", 1, 50_000),
    ];
    let mut run = InterestRun::new(parm_date());

    let first = process_all(&mut run, &mut store, &records[..2]).expect("account 1 posts");
    assert!(store.rewrites.is_empty(), "no rewrite before the break");
    let second = process_all(&mut run, &mut store, &records[2..]).expect("account 2 posts");
    assert_eq!(store.rewrites, [account(1, 101_251, 0, 0)]);

    let outputs: Vec<Option<NewTransaction>> = first.into_iter().chain(second).collect();
    assert_eq!(
        tran_ids(&outputs),
        [
            Some("2022071800000001".to_owned()),
            Some("2022071800000002".to_owned()),
            Some("2022071800000003".to_owned()),
        ]
    );
    let amounts: Vec<Option<i64>> = outputs
        .iter()
        .map(|output| output.as_ref().map(|tran| tran.tran_amt.cents()))
        .collect();
    assert_eq!(amounts, [Some(1_250), Some(1), Some(625)]);
    let last = outputs[2].as_ref().expect("an interest record");
    assert_eq!(last.tran_card_num, CardNumber::move_from(CARD_2));

    let summary = run.finish();
    assert_eq!(
        store.rewrites.len(),
        1,
        "the last account is never rewritten"
    );
    assert_eq!(
        store.accounts[&account_id(2)],
        account(2, 200_000, 30_000, -5_000)
    );
    assert_eq!(summary.accounts_updated, 1);
    assert_eq!(
        summary.unposted,
        Some(UnpostedInterest {
            acct_id: account_id(2),
            total_interest: cents(625),
        })
    );
}

/// CBACT04C 1100-GET-ACCT-DATA, 1110-GET-XREF-DATA and 9999-ABEND-PROGRAM —
/// P-CBACT04C-002, P-CBACT04C-188, P-CBACT04C-189: an account or
/// cross-reference read that ends `INVALID KEY` displays `ACCOUNT NOT FOUND: `
/// and the account, the paragraph's error text and status 23, then abends.
/// The record is not skipped: no rate is read and nothing is written.
#[test]
fn a_missing_account_or_cross_reference_abends() {
    let cases = [
        (
            MemoryStore::default().with_xref(xref(1, CARD_1)),
            InterestAbend::AccountRead {
                acct_id: account_id(1),
                cause: PortError::NotFound,
            },
            ERROR_READING_ACCOUNT_FILE,
        ),
        (
            MemoryStore::default().with_account(account(1, 0, 0, 0)),
            InterestAbend::XrefRead {
                acct_id: account_id(1),
                cause: PortError::NotFound,
            },
            ERROR_READING_XREF_FILE,
        ),
    ];
    for (store, expected, text) in cases {
        let mut store = store.with_group(group("DEFAULT", "01", 1, 1_500));
        let mut run = InterestRun::new(parm_date());
        let abend = process_all(&mut run, &mut store, &[balance(1, "01", 1, 100_000)])
            .expect_err("an abend, not a skipped record");
        assert_eq!(abend, expected);
        assert_eq!(
            abend.lines(),
            [
                "ACCOUNT NOT FOUND: 00000000001",
                text,
                "FILE STATUS IS: NNNN0023",
                ABENDING_PROGRAM,
            ]
        );
        assert!(store.group_reads.is_empty(), "{text}: no rate read");
        assert!(store.rewrites.is_empty(), "{text}: nothing rewritten");
    }
}
