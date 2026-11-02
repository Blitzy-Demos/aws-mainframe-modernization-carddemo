//! `CBTRN02C` validation and posting, driven through the public API of
//! `carddemo_domain::rules::posting` over an in-memory store.
//!
//! The rules are those of the `POSTTRAN` job [app/cbl/CBTRN02C.cbl]: for each
//! daily record in file order, `1500-VALIDATE-TRAN` looks up the
//! cross-reference (reason 100), the account (101), checks `WS-TEMP-BAL`
//! against the credit limit (102) and the expiry date against the
//! origination date (103, which replaces a 102); an accepted record is posted
//! by `2000-POST-TRANSACTION`, whose cycle-total update the next record's
//! check reads.
//!
//! Every test names the reviewed rows of `docs/parity/parity-oracle-audit.md`
//! whose expected results it asserts (sections B, C and E). The reject
//! descriptions are the constants of `carddemo_domain::messages`, the reason
//! codes and amounts the values those rows record.

use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::future::{Future, ready};
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use carddemo_domain::clock::{Clock, FixedClock, db2_timestamp};
use carddemo_domain::entities::{
    Account, CardXref, DailyTran, NewTransaction, TranCatBalance, TranCatBalanceKey, Transaction,
};
use carddemo_domain::messages::{
    ACCOUNT_RECORD_NOT_FOUND, INVALID_CARD_NUMBER_FOUND, OVERLIMIT_TRANSACTION,
    TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
};
use carddemo_domain::numeric::{
    AccountGroupId, AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money, TranCategoryCode,
    TranId, TranTypeCode, TransactionRowId,
};
use carddemo_domain::ports::{
    AccountReader, AccountWriter, PortError, PortResult, TranCatBalanceStore, TransactionWriter,
    XrefReader,
};
use carddemo_domain::rules::posting::{
    Posted, PostingCounts, REJECT_RECORD_LEN, REJECT_TRAN_DATA_LEN, Validation, post,
    reject_record, validate,
};
use chrono::{TimeZone, Utc};

/// The daily transaction fixture, 300 records of 350 bytes, each followed by
/// a line feed [app/data/ASCII/dailytran.txt].
const DAILYTRAN: &[u8] = include_bytes!("../../../app/data/ASCII/dailytran.txt");

/// The card cross-referenced to fixture account 1 [app/data/ASCII/cardxref.txt:L49].
const CARD_1: &str = "9680294154603697";

/// A card number no cross-reference holds.
const UNKNOWN_CARD: &str = "0000000000000000";

/// `DALYTRAN-ORIG-TS` of every fixture daily record
/// [app/data/ASCII/dailytran.txt:L1].
const ORIG_TS: &str = "2022-06-10 19:27:53.000000";

/// `ACCT-EXPIRAION-DATE` of fixture account 1 [app/data/ASCII/acctdata.txt:L1].
const EXPIRY_1: &str = "2025-05-20";

/// The `TRAN-PROC-TS` the parity runs pin through `CARDDEMO_CLOCK_OVERRIDE`
/// (P-CBTRN02C-216, P-CBTRN02C-220).
const PINNED_PROC_TS: &str = "2022-07-01-12.00.00.000000";

/// Polls before [`block_on`] gives up on a future that never completes.
const MAX_POLLS: usize = 16;

/// Drives `future` to completion on the current thread: the domain crate has
/// no runtime, and the in-memory store never waits, so a waker that does
/// nothing suffices.
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

/// The files `CBTRN02C` reads and writes, in memory: `CCXREF` by card,
/// `ACCTDAT` by account, `TCATBALF` by key and the transaction master, with
/// the log of every port call made.
#[derive(Default)]
struct MemoryStore {
    xrefs: BTreeMap<CardNumber, CardXref>,
    accounts: BTreeMap<AccountId, Account>,
    tcatbals: BTreeMap<TranCatBalanceKey, TranCatBalance>,
    transactions: Vec<Transaction>,
    /// The port methods called, in call order.
    accesses: Vec<&'static str>,
    /// The outcome the next account rewrite returns instead of writing.
    account_rewrite_failure: Option<PortError>,
}

impl MemoryStore {
    /// The store holding `account` and the cross-reference of `CARD_1` to it.
    fn with_account(account: Account) -> Self {
        let mut store = Self::default();
        store.xrefs.insert(
            CardNumber::move_from(CARD_1),
            xref(CARD_1, account.acct_id.get()),
        );
        store.accounts.insert(account.acct_id, account);
        store
    }

    /// How many times the port method `access` was called.
    fn count(&self, access: &str) -> usize {
        self.accesses.iter().filter(|&&made| made == access).count()
    }

    /// The account `id` as stored.
    fn account(&self, id: u64) -> &Account {
        self.accounts
            .get(&account_id(id))
            .expect("the account is on file")
    }
}

/// The failure of a port method `CBTRN02C` never calls.
fn not_a_cbtrn02c_access(method: &str) -> PortError {
    PortError::Unavailable(format!("{method} is not an access CBTRN02C makes"))
}

impl XrefReader for MemoryStore {
    fn read_xref_by_card(
        &mut self,
        card: CardNumber,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_ {
        self.accesses.push("read_xref_by_card");
        ready(self.xrefs.get(&card).cloned().ok_or(PortError::NotFound))
    }

    fn read_xref_by_account(
        &mut self,
        _id: AccountId,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_ {
        ready(Err(not_a_cbtrn02c_access("read_xref_by_account")))
    }

    fn xrefs_after(
        &mut self,
        _after: Option<CardNumber>,
        _limit: u16,
    ) -> impl Future<Output = PortResult<Vec<CardXref>>> + Send + '_ {
        ready(Err(not_a_cbtrn02c_access("xrefs_after")))
    }
}

impl AccountReader for MemoryStore {
    fn read_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_ {
        self.accesses.push("read_account");
        ready(self.accounts.get(&id).cloned().ok_or(PortError::NotFound))
    }

    fn read_account_for_update(
        &mut self,
        _id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_ {
        ready(Err(not_a_cbtrn02c_access("read_account_for_update")))
    }
}

impl AccountWriter for MemoryStore {
    fn rewrite_account(
        &mut self,
        account: Account,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        self.accesses.push("rewrite_account");
        let id = account.acct_id;
        let outcome = match self.account_rewrite_failure.take() {
            Some(failure) => Err(failure),
            None => rewrite(&mut self.accounts, &id, account),
        };
        ready(outcome)
    }
}

impl TranCatBalanceStore for MemoryStore {
    fn read_tcatbal(
        &mut self,
        key: TranCatBalanceKey,
    ) -> impl Future<Output = PortResult<TranCatBalance>> + Send + '_ {
        self.accesses.push("read_tcatbal");
        ready(self.tcatbals.get(&key).cloned().ok_or(PortError::NotFound))
    }

    fn write_tcatbal(
        &mut self,
        rec: TranCatBalance,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        self.accesses.push("write_tcatbal");
        let outcome = match self.tcatbals.entry(rec.key.clone()) {
            Entry::Occupied(_) => Err(PortError::Duplicate),
            Entry::Vacant(slot) => {
                slot.insert(rec);
                Ok(())
            }
        };
        ready(outcome)
    }

    fn rewrite_tcatbal(
        &mut self,
        rec: TranCatBalance,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        self.accesses.push("rewrite_tcatbal");
        let key = rec.key.clone();
        ready(rewrite(&mut self.tcatbals, &key, rec))
    }

    fn tcatbals_after(
        &mut self,
        _after: Option<TranCatBalanceKey>,
        _limit: u16,
    ) -> impl Future<Output = PortResult<Vec<TranCatBalance>>> + Send + '_ {
        ready(Err(not_a_cbtrn02c_access("tcatbals_after")))
    }
}

impl TransactionWriter for MemoryStore {
    fn write_transaction(
        &mut self,
        tran: NewTransaction,
    ) -> impl Future<Output = PortResult<TransactionRowId>> + Send + '_ {
        self.accesses.push("write_transaction");
        let outcome = if self
            .transactions
            .iter()
            .any(|row| row.tran_id == tran.tran_id)
        {
            Err(PortError::Duplicate)
        } else {
            let identity = i64::try_from(self.transactions.len() + 1)
                .expect("a test store holds far fewer than i64::MAX rows");
            let row_id = TransactionRowId::from(identity);
            self.transactions.push(Transaction::from_new(row_id, tran));
            Ok(row_id)
        };
        ready(outcome)
    }
}

/// A `REWRITE`: the record replaces the stored one under `key`, which must
/// exist (`INVALID KEY`, status 23, otherwise).
fn rewrite<K: Ord, V>(records: &mut BTreeMap<K, V>, key: &K, record: V) -> PortResult<()> {
    let stored = records.get_mut(key).ok_or(PortError::NotFound)?;
    *stored = record;
    Ok(())
}

fn money9(cents: i64) -> Money<9> {
    Money::from_cents(cents).expect("the amount fits S9(09)V99")
}

fn money10(cents: i64) -> Money<10> {
    Money::from_cents(cents).expect("the amount fits S9(10)V99")
}

fn account_id(id: u64) -> AccountId {
    AccountId::new(id).expect("the ID fits 9(11)")
}

/// A daily record of `amount_cents` on `card`, originated at `orig_ts`, with
/// the other fields of dailytran record 1, a type `01` category `0001`
/// purchase [app/data/ASCII/dailytran.txt:L1].
fn daily(card: &str, amount_cents: i64, orig_ts: &str) -> DailyTran {
    DailyTran {
        dalytran_id: TranId::move_from("0000000000683580"),
        dalytran_type_cd: TranTypeCode::move_from("01"),
        dalytran_cat_cd: TranCategoryCode::new(1).expect("the code fits 9(04)"),
        dalytran_source: Alnum::move_from("POS TERM"),
        dalytran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
        dalytran_amt: money9(amount_cents),
        dalytran_merchant_id: MerchantId::new(800_000_000).expect("the ID fits 9(09)"),
        dalytran_merchant_name: Alnum::move_from("Abshire-Lowe"),
        dalytran_merchant_city: Alnum::move_from("North Enoshaven"),
        dalytran_merchant_zip: Alnum::move_from("72112"),
        dalytran_card_num: CardNumber::move_from(card),
        dalytran_orig_ts: Alnum::move_from(orig_ts),
        dalytran_proc_ts: Alnum::spaces(),
    }
}

/// Account `id` with the given credit limit, cycle totals (in cents) and
/// expiry date, and the other fields of fixture account 1
/// [app/data/ASCII/acctdata.txt:L1].
fn account(id: u64, limit: i64, cyc_credit: i64, cyc_debit: i64, expiry: &str) -> Account {
    Account {
        acct_id: account_id(id),
        acct_active_status: Alnum::move_from("Y"),
        acct_curr_bal: money10(19_400),
        acct_credit_limit: money10(limit),
        acct_cash_credit_limit: money10(102_000),
        acct_open_date: Alnum::move_from("2014-11-20"),
        acct_expiraion_date: Alnum::move_from(expiry),
        acct_reissue_date: Alnum::move_from("2025-05-20"),
        acct_curr_cyc_credit: money10(cyc_credit),
        acct_curr_cyc_debit: money10(cyc_debit),
        acct_addr_zip: Alnum::move_from("A000000000"),
        acct_group_id: AccountGroupId::move_from(""),
    }
}

/// The cross-reference of `card` to `account`, under customer 1
/// [app/data/ASCII/cardxref.txt:L49].
fn xref(card: &str, account: u64) -> CardXref {
    CardXref {
        xref_card_num: CardNumber::move_from(card),
        xref_cust_id: CustomerId::new(1).expect("the ID fits 9(09)"),
        xref_acct_id: account_id(account),
    }
}

/// The clock `CARDDEMO_CLOCK_OVERRIDE` pins for the parity runs.
fn pinned_clock() -> FixedClock {
    FixedClock(
        Utc.with_ymd_and_hms(2022, 7, 1, 12, 0, 0)
            .single()
            .expect("a valid instant"),
    )
}

/// `1500-VALIDATE-TRAN` of `tran` against `store`, which never abends here.
fn validated(store: &mut MemoryStore, tran: &DailyTran) -> Validation {
    block_on(validate(store, tran)).expect("no store failure, so no abend")
}

/// The outcome of a record that passes every check, with the records read.
fn accepted(account: &Account) -> Validation {
    Validation::Accepted {
        xref: xref(CARD_1, account.acct_id.get()),
        account: account.clone(),
    }
}

/// P-CBTRN02C-006, P-CBTRN02C-008: `1500-A-LOOKUP-XREF` `INVALID KEY` sets
/// reason 100 `INVALID CARD NUMBER FOUND`, and `1500-VALIDATE-TRAN` then
/// skips `1500-B-LOOKUP-ACCT` [app/cbl/CBTRN02C.cbl:L370-391].
#[test]
fn an_unknown_card_is_reason_100_and_skips_the_account_read() {
    let mut store = MemoryStore::with_account(account(1, 202_000, 0, 0, EXPIRY_1));

    let validation = validated(&mut store, &daily(UNKNOWN_CARD, 5_047, ORIG_TS));

    assert_eq!(
        validation,
        Validation::Rejected {
            reason: 100,
            description: INVALID_CARD_NUMBER_FOUND,
        }
    );
    assert_eq!(store.count("read_account"), 0);
    assert_eq!(store.accesses, ["read_xref_by_card"]);
}

/// P-CBTRN02C-009: the cross-referenced account missing from `ACCTDAT` is
/// reason 101 `ACCOUNT RECORD NOT FOUND` [app/cbl/CBTRN02C.cbl:L393-399].
#[test]
fn a_cross_referenced_account_that_is_missing_is_reason_101() {
    let mut store = MemoryStore::default();
    store
        .xrefs
        .insert(CardNumber::move_from(CARD_1), xref(CARD_1, 1));

    let validation = validated(&mut store, &daily(CARD_1, 5_047, ORIG_TS));

    assert_eq!(
        validation,
        Validation::Rejected {
            reason: 101,
            description: ACCOUNT_RECORD_NOT_FOUND,
        }
    );
    assert_eq!(store.accesses, ["read_xref_by_card", "read_account"]);
}

/// P-CBTRN02C-010, P-CBTRN02C-011, P-CBTRN02C-217 (E-15): the limit check
/// sets 102 `OVERLIMIT TRANSACTION` when `ACCT-CREDIT-LIMIT >= WS-TEMP-BAL`
/// fails; the expiry check runs after it whatever it found and sets 103
/// `TRANSACTION RECEIVED AFTER ACCT EXPIRATION`, so a record both over the
/// limit and expired keeps 103 [app/cbl/CBTRN02C.cbl:L403-420].
#[test]
fn the_limit_and_expiry_checks_reject_with_the_audited_reason() {
    let cases = [
        (
            "over the limit: 90.00 + 20.00 above 100.00",
            account(1, 10_000, 9_000, 0, EXPIRY_1),
            2_000,
            102,
            OVERLIMIT_TRANSACTION,
        ),
        (
            "within the limit, expired the day before",
            account(1, 10_000, 0, 0, "2022-06-09"),
            2_000,
            103,
            TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
        ),
        (
            "over the limit and expired: 103 replaces 102",
            account(1, 10_000, 9_000, 0, "2022-06-09"),
            2_000,
            103,
            TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
        ),
    ];
    for (name, account, cents, reason, description) in cases {
        let mut store = MemoryStore::with_account(account);

        let validation = validated(&mut store, &daily(CARD_1, cents, ORIG_TS));

        assert_eq!(
            validation,
            Validation::Rejected {
                reason,
                description
            },
            "{name}"
        );
    }
}

/// P-CBTRN02C-010, P-CBTRN02C-011: both checks are `>=`, so a `WS-TEMP-BAL`
/// equal to the limit and an expiry date equal to `DALYTRAN-ORIG-TS (1:10)`
/// pass.
#[test]
fn the_limit_and_the_expiry_date_themselves_are_accepted() {
    let cases = [
        (
            "WS-TEMP-BAL 40.00 + 60.00 equals the limit 100.00",
            account(1, 10_000, 4_000, 0, EXPIRY_1),
        ),
        (
            "expiry equals the origination date",
            account(1, 202_000, 0, 0, "2022-06-10"),
        ),
    ];
    for (name, account) in cases {
        let mut store = MemoryStore::with_account(account.clone());

        let validation = validated(&mut store, &daily(CARD_1, 6_000, ORIG_TS));

        assert_eq!(validation, accepted(&account), "{name}");
    }
}

/// P-CBTRN02C-010, P-CBTRN02C-022, P-CBTRN02C-023: `COMPUTE WS-TEMP-BAL =
/// ACCT-CURR-CYC-CREDIT - ACCT-CURR-CYC-DEBIT + DALYTRAN-AMT` stores into
/// `WS-TEMP-BAL PIC S9(09)V99` [app/cbl/CBTRN02C.cbl:L187]
/// [app/cbl/CBTRN02C.cbl:L403-405], narrower than its `S9(10)V99` operands.
///
/// Without `ON SIZE ERROR` the store keeps the low-order nine integer digits
/// (AAP 0.9.2): a cycle credit of 1,000,000,100.00 plus 50.00 is exactly
/// 1,000,000,150.00 but checks as 150.00. That is the COBOL result, reproduced
/// rather than repaired, so a limit of 200.00 accepts the record and only a
/// limit below 150.00 rejects it. Subtracting the cycle debit, which holds
/// negative amounts as negative values, adds their magnitude.
#[test]
fn ws_temp_bal_keeps_the_low_order_nine_integer_digits() {
    let cases = [
        (
            "1000000150.00 stored as 150.00, under a limit of 200.00",
            20_000,
            100_000_010_000,
            0,
            5_000,
            true,
        ),
        (
            "1000000150.00 stored as 150.00, over a limit of 100.00",
            10_000,
            100_000_010_000,
            0,
            5_000,
            false,
        ),
        (
            "P-CBTRN02C-022: 1500000000.00 stored as 500000000.00",
            60_000_000_000,
            150_000_000_000,
            0,
            0,
            true,
        ),
        (
            "P-CBTRN02C-023: 100.00 - (-30.00) + 10.00 is 140.00",
            14_000,
            10_000,
            -3_000,
            1_000,
            true,
        ),
        (
            "P-CBTRN02C-023: one cent under 140.00",
            13_999,
            10_000,
            -3_000,
            1_000,
            false,
        ),
    ];
    for (name, limit, credit, debit, cents, passes) in cases {
        let account = account(1, limit, credit, debit, EXPIRY_1);
        let mut store = MemoryStore::with_account(account.clone());

        let validation = validated(&mut store, &daily(CARD_1, cents, ORIG_TS));

        let expected = if passes {
            accepted(&account)
        } else {
            Validation::Rejected {
                reason: 102,
                description: OVERLIMIT_TRANSACTION,
            }
        };
        assert_eq!(validation, expected, "{name}");
    }
}

/// P-CBTRN02C-011: `IF ACCT-EXPIRAION-DATE >= DALYTRAN-ORIG-TS (1:10)` is an
/// alphanumeric comparison of two `X(10)` items, byte by byte, not a date
/// comparison [app/cbl/CBTRN02C.cbl:L414]. A blank expiry sorts below every
/// date; dates in another layout compare by their characters.
#[test]
fn the_expiry_check_compares_text_not_dates() {
    let cases = [
        ("a blank expiry date", "", ORIG_TS, false),
        (
            "a later date whose text sorts lower",
            "12/31/2025",
            ORIG_TS,
            false,
        ),
        (
            "an earlier date whose text sorts higher",
            "2022-06-09",
            "06/10/2022 19:27:53.000000",
            true,
        ),
    ];
    for (name, expiry, orig_ts, passes) in cases {
        let account = account(1, 202_000, 0, 0, expiry);
        let mut store = MemoryStore::with_account(account.clone());

        let validation = validated(&mut store, &daily(CARD_1, 5_047, orig_ts));

        let expected = if passes {
            accepted(&account)
        } else {
            Validation::Rejected {
                reason: 103,
                description: TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
            }
        };
        assert_eq!(validation, expected, "{name}");
    }
}

/// The `TCATBALF` key of account `account`, type `01`, category `0001`: the
/// key `2700-UPDATE-TCATBAL` builds for a [`daily`] record.
fn tcatbal_key(account: u64) -> TranCatBalanceKey {
    TranCatBalanceKey {
        trancat_acct_id: account_id(account),
        trancat_type_cd: TranTypeCode::move_from("01"),
        trancat_cd: TranCategoryCode::new(1).expect("the code fits 9(04)"),
    }
}

/// Validates `tran` and posts it with the records the validation read,
/// stamped by the pinned clock.
fn posted(store: &mut MemoryStore, tran: &DailyTran) -> Posted {
    let Validation::Accepted { xref, account } = validated(store, tran) else {
        panic!("the record passes 1500-VALIDATE-TRAN");
    };
    block_on(post(store, tran, &xref, account, &pinned_clock()))
        .expect("no store failure, so no abend")
}

/// What a run of the `CBTRN02C` main loop produced.
struct Run {
    counts: PostingCounts,
    /// `DALYTRAN-ID`, reason and description of each reject, in file order.
    rejects: Vec<(TranId, u16, &'static str)>,
    posted: Vec<Posted>,
}

/// The main loop over `records` in file order (P-CBTRN02C-006)
/// [app/cbl/CBTRN02C.cbl:L202-219]: each record is counted and validated,
/// then posted, or counted as rejected.
fn run(store: &mut MemoryStore, records: &[DailyTran]) -> Run {
    let mut outcome = Run {
        counts: PostingCounts::default(),
        rejects: Vec::new(),
        posted: Vec::new(),
    };
    for tran in records {
        outcome.counts.read += 1;
        match validated(store, tran) {
            Validation::Accepted { xref, account } => outcome.posted.push(
                block_on(post(store, tran, &xref, account, &pinned_clock()))
                    .expect("no store failure, so no abend"),
            ),
            Validation::Rejected {
                reason,
                description,
            } => {
                outcome.counts.rejected += 1;
                outcome
                    .rejects
                    .push((tran.dalytran_id, reason, description));
            }
        }
    }
    outcome
}

/// P-CBTRN02C-015, P-CBTRN02C-016, P-CBTRN02C-017, P-CBTRN02C-024,
/// P-CBTRN02C-025: `2700-UPDATE-TCATBAL` reads the balance of the
/// cross-referenced account, type and category. A found record takes
/// `2700-B`, `ADD DALYTRAN-AMT TO TRAN-CAT-BAL` and `REWRITE`; a missing one
/// (status 23) takes `2700-A`, `INITIALIZE`, the same `ADD` and `WRITE`. The
/// receiver is `S9(09)V99` without `ON SIZE ERROR`, so a sum of ten integer
/// digits keeps its low-order nine [app/cbl/CBTRN02C.cbl:L467-542].
#[test]
fn the_category_balance_is_rewritten_when_found_and_created_when_missing() {
    let cases = [
        ("found: 100.00 + 25.50", Some(10_000), 2_550, 12_550, (0, 1)),
        ("missing: 0 + 25.50", None, 2_550, 2_550, (1, 0)),
        (
            "found: 999999999.00 + 2.00 keeps 000000001.00",
            Some(99_999_999_900),
            200,
            100,
            (0, 1),
        ),
    ];
    for (name, existing, cents, balance, (writes, rewrites)) in cases {
        let mut store = MemoryStore::with_account(account(1, 202_000, 0, 0, EXPIRY_1));
        if let Some(existing) = existing {
            store.tcatbals.insert(
                tcatbal_key(1),
                TranCatBalance {
                    key: tcatbal_key(1),
                    tran_cat_bal: money9(existing),
                },
            );
        }

        posted(&mut store, &daily(CARD_1, cents, ORIG_TS));

        let stored = store.tcatbals.get(&tcatbal_key(1));
        assert_eq!(
            stored.map(|record| record.tran_cat_bal),
            Some(money9(balance)),
            "{name}"
        );
        assert_eq!(store.tcatbals.len(), 1, "{name}");
        assert_eq!(
            (store.count("write_tcatbal"), store.count("rewrite_tcatbal")),
            (writes, rewrites),
            "{name}"
        );
    }
}

/// P-CBTRN02C-018, P-CBTRN02C-026, P-CBTRN02C-226, P-CBTRN02C-227:
/// `2800-UPDATE-ACCOUNT-REC` adds the amount to `ACCT-CURR-BAL`, and to
/// `ACCT-CURR-CYC-CREDIT` when it is zero or more or to `ACCT-CURR-CYC-DEBIT`
/// when it is negative, so the debit accumulates negative values; then it
/// rewrites the account [app/cbl/CBTRN02C.cbl:L545-559]. A zero amount takes
/// the credit branch, where adding it leaves every field as it was.
#[test]
fn the_account_takes_the_amount_into_its_balance_and_one_cycle_total() {
    let cases = [
        ("credit 25.50", 2_550, 21_950, 7_550, -2_000),
        ("debit -10.00", -1_000, 18_400, 5_000, -3_000),
        ("zero counts as a credit", 0, 19_400, 5_000, -2_000),
    ];
    for (name, cents, balance, cyc_credit, cyc_debit) in cases {
        let mut store = MemoryStore::with_account(account(1, 202_000, 5_000, -2_000, EXPIRY_1));

        let outcome = posted(&mut store, &daily(CARD_1, cents, ORIG_TS));

        assert!(!outcome.account_rewrite_failed, "{name}");
        let stored = store.account(1);
        assert_eq!(stored.acct_curr_bal, money10(balance), "{name}");
        assert_eq!(stored.acct_curr_cyc_credit, money10(cyc_credit), "{name}");
        assert_eq!(stored.acct_curr_cyc_debit, money10(cyc_debit), "{name}");
        assert_eq!(store.count("rewrite_account"), 1, "{name}");
    }
}

/// P-CBTRN02C-013, P-CBTRN02C-014, P-CBTRN02C-216, P-CBTRN02C-220:
/// `2000-POST-TRANSACTION` moves `DALYTRAN-ID` through `DALYTRAN-ORIG-TS` one
/// by one into `TRAN-RECORD`, stamps `TRAN-PROC-TS` with
/// `Z-GET-DB2-FORMAT-TIMESTAMP` of the processing clock, then performs 2700,
/// 2800 and 2900 in that order [app/cbl/CBTRN02C.cbl:L424-444].
#[test]
fn the_written_transaction_moves_the_daily_fields_and_stamps_the_clock() {
    let tran = daily(CARD_1, 5_047, ORIG_TS);
    let mut store = MemoryStore::with_account(account(1, 202_000, 0, 0, EXPIRY_1));

    let outcome = posted(&mut store, &tran);

    let proc_ts = Alnum::move_from(&db2_timestamp(pinned_clock().now()));
    assert_eq!(proc_ts.as_bytes(), PINNED_PROC_TS.as_bytes());
    let expected = Transaction {
        transaction_id: TransactionRowId::from(1),
        tran_id: tran.dalytran_id,
        tran_type_cd: tran.dalytran_type_cd,
        tran_cat_cd: tran.dalytran_cat_cd,
        tran_source: tran.dalytran_source,
        tran_desc: tran.dalytran_desc,
        tran_amt: tran.dalytran_amt,
        tran_merchant_id: tran.dalytran_merchant_id,
        tran_merchant_name: tran.dalytran_merchant_name,
        tran_merchant_city: tran.dalytran_merchant_city,
        tran_merchant_zip: tran.dalytran_merchant_zip,
        tran_card_num: tran.dalytran_card_num,
        tran_orig_ts: tran.dalytran_orig_ts,
        tran_proc_ts: proc_ts,
    };
    assert_eq!(
        outcome,
        Posted {
            transaction: expected.clone(),
            account_rewrite_failed: false,
        }
    );
    assert_eq!(store.transactions, [expected]);
    assert_eq!(
        store.accesses,
        [
            "read_xref_by_card",
            "read_account",
            "read_tcatbal",
            "write_tcatbal",
            "rewrite_account",
            "write_transaction",
        ]
    );
}

/// P-CBTRN02C-018, P-CBTRN02C-218 (E-15): the account `REWRITE` `INVALID KEY`
/// moves reason 109 `ACCOUNT RECORD NOT FOUND` into a field nothing reads
/// afterwards [app/cbl/CBTRN02C.cbl:L554-559]. The record is neither counted
/// as rejected nor written to `DALYREJS`, the category balance update stands,
/// the account stays as it was, and the transaction is still written.
#[test]
fn an_account_rewrite_invalid_key_is_never_rejected_and_still_writes_the_transaction() {
    let before = account(1, 202_000, 0, 0, EXPIRY_1);
    let mut store = MemoryStore::with_account(before.clone());
    store.account_rewrite_failure = Some(PortError::NotFound);
    let tran = daily(CARD_1, 5_047, ORIG_TS);

    let outcome = run(&mut store, std::slice::from_ref(&tran));

    assert_eq!(
        outcome.counts,
        PostingCounts {
            read: 1,
            rejected: 0
        }
    );
    assert_eq!(outcome.counts.return_code(), 0);
    assert!(outcome.rejects.is_empty());
    let [only] = outcome.posted.as_slice() else {
        panic!("the record is posted");
    };
    assert!(only.account_rewrite_failed);
    assert_eq!(
        only.transaction.to_new(),
        tran.to_new_transaction(Alnum::move_from(PINNED_PROC_TS))
    );
    assert_eq!(store.transactions, [only.transaction.clone()]);
    assert_eq!(store.account(1), &before);
    assert_eq!(
        store
            .tcatbals
            .get(&tcatbal_key(1))
            .map(|record| record.tran_cat_bal),
        Some(money9(5_047))
    );
    assert_eq!(
        store.accesses.last_chunk(),
        Some(&["rewrite_account", "write_transaction"])
    );
}

/// P-CBTRN02C-006, P-CBTRN02C-010, P-CBTRN02C-018 (AAP 0.9.3): every record
/// rereads its account, so the cycle credit an accepted record posts takes
/// part in the next record's `WS-TEMP-BAL`, and a rejected record posts
/// nothing. Under a limit of 100.00, 60.00 is accepted, 50.00 then checks as
/// 110.00 and is rejected, and 40.00 checks as exactly 100.00 and is accepted.
#[test]
fn an_accepted_record_raises_the_cycle_credit_the_next_record_checks() {
    let record = |id: &str, cents| DailyTran {
        dalytran_id: TranId::move_from(id),
        ..daily(CARD_1, cents, ORIG_TS)
    };
    let records = [
        record("0000000000000001", 6_000),
        record("0000000000000002", 5_000),
        record("0000000000000003", 4_000),
    ];
    let mut store = MemoryStore::with_account(account(1, 10_000, 0, 0, EXPIRY_1));

    let outcome = run(&mut store, &records);

    assert_eq!(
        outcome.rejects,
        [(
            TranId::move_from("0000000000000002"),
            102,
            OVERLIMIT_TRANSACTION
        )]
    );
    assert_eq!(
        outcome.counts,
        PostingCounts {
            read: 3,
            rejected: 1
        }
    );
    assert_eq!(outcome.counts.return_code(), 4);
    let stored = store.account(1);
    assert_eq!(stored.acct_curr_cyc_credit, money10(10_000));
    assert_eq!(stored.acct_curr_bal, money10(29_400));
    let written: Vec<TranId> = store.transactions.iter().map(|row| row.tran_id).collect();
    assert_eq!(written, [records[0].dalytran_id, records[2].dalytran_id]);
    assert_eq!(store.count("read_account"), 3);
}

/// Record `number`, counted from 1, of the daily transaction fixture: its
/// 350 bytes without the line feed.
fn fixture_record(number: usize) -> [u8; REJECT_TRAN_DATA_LEN] {
    let line = DAILYTRAN
        .split(|&byte| byte == b'\n')
        .nth(number - 1)
        .expect("the fixture holds the record");
    <[u8; REJECT_TRAN_DATA_LEN]>::try_from(line).expect("every fixture record is 350 bytes")
}

/// P-CBTRN02C-001, P-CBTRN02C-002, P-CBTRN02C-003, P-CBTRN02C-012,
/// P-CBTRN02C-027: `2500-WRITE-REJECT-REC` writes the 430-byte
/// `REJECT-RECORD`, the 350 bytes of the daily record as read and the
/// `VALIDATION-TRAILER`: the reason as four `9(04)` digits and the
/// description in `X(76)`, space-padded [app/cbl/CBTRN02C.cbl:L176-182]
/// [app/cbl/CBTRN02C.cbl:L446-451]. Dailytran record 16 is the first fixture
/// reject, reason 0102 [app/data/ASCII/dailytran.txt:L16].
#[test]
fn a_reject_record_is_the_input_bytes_then_the_reason_and_its_description() {
    let raw = fixture_record(16);
    assert!(raw.starts_with(b"0000000040455859"));
    let cases = [
        (100, INVALID_CARD_NUMBER_FOUND, "0100"),
        (101, ACCOUNT_RECORD_NOT_FOUND, "0101"),
        (102, OVERLIMIT_TRANSACTION, "0102"),
        (103, TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION, "0103"),
    ];
    for (reason, description, digits) in cases {
        let record = reject_record(&raw, reason, description);

        assert_eq!(record.len(), 430, "{reason}");
        assert_eq!(record.len(), REJECT_RECORD_LEN, "{reason}");
        let (data, trailer) = record.split_at(REJECT_TRAN_DATA_LEN);
        assert_eq!(data, raw.as_slice(), "{reason}");
        let (reason_field, description_field) = trailer.split_at(4);
        assert_eq!(reason_field, digits.as_bytes(), "{reason}");
        assert_eq!(
            description_field,
            format!("{description:<76}").as_bytes(),
            "{reason}"
        );
    }
}

/// P-CBTRN02C-020: `MOVE 4 TO RETURN-CODE` when `WS-REJECT-COUNT > 0`, else the
/// run ends with 0 [app/cbl/CBTRN02C.cbl:L229-231]. The fixture run reads 300
/// records and posts 262 (P-CBTRN02C-216), rejecting 38.
#[test]
fn the_return_code_is_four_once_any_record_is_rejected() {
    let cases = [(0, 0, 0), (300, 0, 0), (300, 38, 4), (1, 1, 4)];
    for (read, rejected, code) in cases {
        assert_eq!(
            PostingCounts { read, rejected }.return_code(),
            code,
            "{read} read, {rejected} rejected"
        );
    }
    assert_eq!(PostingCounts::default().return_code(), 0);
}
