//! `COBIL00C` (transaction CB00, `POST /payments`) through the public API of
//! `carddemo_domain::rules::bill_payment`: the edits and reads of `prepare`,
//! the record and the two writes of `pay`, and the E-19 order in which the
//! balance is rewritten whatever the transaction write answered.
//!
//! The rule runs against `MemoryStore`, an in-memory implementation of the four
//! ports it takes. Every expected kind, message and BMS field is the one of the
//! validation audit row a test cites (`V-COBIL00C-<nnn>`), and every expected
//! record value or arithmetic result the one of its parity audit row
//! (`P-COBIL00C-<nnn>`), both in `docs/parity/`. Message texts come from
//! `carddemo_domain::messages` only.

use std::collections::BTreeMap;
use std::future::{Future, ready};
use std::ops::Bound;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

use carddemo_domain::clock::FixedClock;
use carddemo_domain::entities::{Account, CardXref, NewTransaction, Transaction};
use carddemo_domain::error::{DomainError, ErrorKind};
use carddemo_domain::messages::{
    ACCOUNT_ID_NOT_FOUND, ACCT_ID_CAN_NOT_BE_EMPTY, CONFIRM_TO_MAKE_A_BILL_PAYMENT,
    INVALID_VALUE_VALID_VALUES_ARE_Y_N, PAYMENT_SUCCESSFUL, TRAN_ID_ALREADY_EXIST,
    UNABLE_TO_ADD_BILL_PAY_TRANSACTION, UNABLE_TO_LOOKUP_ACCOUNT, UNABLE_TO_LOOKUP_XREF_AIX_FILE,
    UNABLE_TO_UPDATE_ACCOUNT, YOU_HAVE_NOTHING_TO_PAY, YOUR_TRANSACTION_ID_IS,
    payment_successful_message,
};
use carddemo_domain::numeric::{
    AccountGroupId, AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money, TranCategoryCode,
    TranId, TranTypeCode, TransactionRowId,
};
use carddemo_domain::ports::{
    AccountReader, AccountWriter, PortError, PortResult, TransactionWriter, XrefReader,
};
use carddemo_domain::rules::Confirm;
use carddemo_domain::rules::bill_payment::{
    BillPaymentInput, BillPaymentStep, FIELD_ACTIDIN, FIELD_CONFIRM, PaymentDraft, PaymentSuccess,
    build_transaction, pay, prepare,
};
use chrono::{DateTime, NaiveDate, Utc};

/// The first `acctdata.txt` account, `00000000001` [app/data/ASCII/acctdata.txt:L1].
const ACCOUNT: u64 = 1;
/// Its 11-digit `ACTIDIN` entry.
const ACCOUNT_ENTRY: &str = "00000000001";
/// An 11-digit account number no fixture account has.
const UNKNOWN_ENTRY: &str = "99999999999";
/// The card the cross-reference maps account 1 to
/// [app/data/ASCII/cardxref.txt:L49].
const CARD: &str = "9680294154603697";
/// The TRAN-ID the caller allocated for the payment.
const TRAN_ID: &str = "0000000000000043";
/// 250.00, the balance a payment pays in full.
const BALANCE: i64 = 25_000;
/// The `WS-TIMESTAMP` of the fixed clock: whole seconds, the six fraction
/// digits zero (P-COBIL00C-026).
const PAYMENT_TS: &[u8; 26] = b"2022-07-18 09:30:15.000000";
/// Polls before `block_on` gives up. `MemoryStore` answers every call with a
/// ready future, so a rule future completes on its first poll.
const MAX_POLLS: usize = 8;

/// Drives `future` to completion on the current thread. The domain crate has
/// no async runtime, and the store never waits, so a waker that does nothing
/// suffices.
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

/// One port call, recorded in call order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Call {
    ReadAccount,
    ReadAccountForUpdate,
    RewriteAccount,
    ReadXrefByCard,
    ReadXrefByAccount,
    XrefsAfter,
    WriteTransaction,
}

/// The account master, the cross-reference and the live transaction master in
/// memory, keyed as their VSAM files are, with an injectable failure per
/// access.
#[derive(Debug, Clone, Default)]
struct MemoryStore {
    accounts: BTreeMap<AccountId, Account>,
    /// `CCXREF` by card number; `CXACAIX` reads it in card-number order.
    xrefs: BTreeMap<CardNumber, CardXref>,
    transactions: BTreeMap<TranId, Transaction>,
    /// Every record `rewrite_account` was given, failed rewrites included.
    rewrite_attempts: Vec<Account>,
    calls: Vec<Call>,
    /// The answer of every account read instead of the stored record.
    account_read_failure: Option<PortError>,
    /// The answer of every cross-reference read instead of the stored record.
    xref_read_failure: Option<PortError>,
    /// The answer of `write_transaction` instead of the insert.
    write_failure: Option<PortError>,
    /// The answer of `rewrite_account` instead of the update.
    rewrite_failure: Option<PortError>,
}

impl MemoryStore {
    fn with_account(mut self, account: Account) -> Self {
        self.accounts.insert(account.acct_id, account);
        self
    }

    fn with_xref(mut self, xref: CardXref) -> Self {
        self.xrefs.insert(xref.xref_card_num, xref);
        self
    }

    /// A live transaction already holding `tran`'s TRAN-ID.
    fn with_transaction(mut self, tran: NewTransaction) -> Self {
        self.insert_transaction(tran);
        self
    }

    /// Stores `tran` as a new live row with the next row identity.
    fn insert_transaction(&mut self, tran: NewTransaction) -> TransactionRowId {
        let rows = i64::try_from(self.transactions.len()).expect("a small in-memory master");
        let row_id = TransactionRowId::from(rows + 1);
        self.transactions
            .insert(tran.tran_id, Transaction::from_new(row_id, tran));
        row_id
    }

    fn read_account_record(&self, id: AccountId) -> PortResult<Account> {
        match &self.account_read_failure {
            Some(error) => Err(error.clone()),
            None => self.accounts.get(&id).cloned().ok_or(PortError::NotFound),
        }
    }

    fn read_xref_where(&self, matches: impl Fn(&CardXref) -> bool) -> PortResult<CardXref> {
        match &self.xref_read_failure {
            Some(error) => Err(error.clone()),
            None => self
                .xrefs
                .values()
                .find(|xref| matches(xref))
                .cloned()
                .ok_or(PortError::NotFound),
        }
    }
}

impl AccountReader for MemoryStore {
    fn read_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_ {
        self.calls.push(Call::ReadAccount);
        ready(self.read_account_record(id))
    }

    fn read_account_for_update(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_ {
        self.calls.push(Call::ReadAccountForUpdate);
        ready(self.read_account_record(id))
    }
}

impl AccountWriter for MemoryStore {
    fn rewrite_account(
        &mut self,
        account: Account,
    ) -> impl Future<Output = PortResult<()>> + Send + '_ {
        self.calls.push(Call::RewriteAccount);
        self.rewrite_attempts.push(account.clone());
        let outcome = match self.rewrite_failure.clone() {
            Some(error) => Err(error),
            None => match self.accounts.get_mut(&account.acct_id) {
                Some(stored) => {
                    *stored = account;
                    Ok(())
                }
                None => Err(PortError::NotFound),
            },
        };
        ready(outcome)
    }
}

impl XrefReader for MemoryStore {
    fn read_xref_by_card(
        &mut self,
        card: CardNumber,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_ {
        self.calls.push(Call::ReadXrefByCard);
        ready(self.read_xref_where(|xref| xref.xref_card_num == card))
    }

    fn read_xref_by_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_ {
        self.calls.push(Call::ReadXrefByAccount);
        ready(self.read_xref_where(|xref| xref.xref_acct_id == id))
    }

    fn xrefs_after(
        &mut self,
        after: Option<CardNumber>,
        limit: u16,
    ) -> impl Future<Output = PortResult<Vec<CardXref>>> + Send + '_ {
        self.calls.push(Call::XrefsAfter);
        let outcome = if let Some(error) = &self.xref_read_failure {
            Err(error.clone())
        } else {
            let start = after.map_or(Bound::Unbounded, Bound::Excluded);
            Ok(self
                .xrefs
                .range((start, Bound::Unbounded))
                .map(|(_, xref)| xref.clone())
                .take(usize::from(limit))
                .collect())
        };
        ready(outcome)
    }
}

impl TransactionWriter for MemoryStore {
    fn write_transaction(
        &mut self,
        tran: NewTransaction,
    ) -> impl Future<Output = PortResult<TransactionRowId>> + Send + '_ {
        self.calls.push(Call::WriteTransaction);
        let outcome = match self.write_failure.clone() {
            Some(error) => Err(error),
            None if self.transactions.contains_key(&tran.tran_id) => Err(PortError::Duplicate),
            None => Ok(self.insert_transaction(tran)),
        };
        ready(outcome)
    }
}

fn account_id() -> AccountId {
    AccountId::new(ACCOUNT).expect("an 11-digit account number")
}

fn money<const D: u8>(cents: i64) -> Money<D> {
    Money::from_cents(cents).expect("an amount the field holds")
}

/// Account 1 as `acctdata.txt` holds it, with the balance `balance_cents`.
fn account(balance_cents: i64) -> Account {
    Account {
        acct_id: account_id(),
        acct_active_status: Alnum::move_from("Y"),
        acct_curr_bal: money(balance_cents),
        acct_credit_limit: money(202_000),
        acct_cash_credit_limit: money(102_000),
        acct_open_date: Alnum::move_from("2014-11-20"),
        acct_expiraion_date: Alnum::move_from("2025-05-20"),
        acct_reissue_date: Alnum::move_from("2025-05-20"),
        acct_curr_cyc_credit: Money::ZERO,
        acct_curr_cyc_debit: Money::ZERO,
        acct_addr_zip: Alnum::move_from("A000000000"),
        acct_group_id: AccountGroupId::move_from(""),
    }
}

fn xref() -> CardXref {
    CardXref {
        xref_card_num: CardNumber::move_from(CARD),
        xref_cust_id: CustomerId::new(1).expect("a 9-digit customer number"),
        xref_acct_id: account_id(),
    }
}

/// Account 1 with `balance_cents` and its cross-reference.
fn store(balance_cents: i64) -> MemoryStore {
    MemoryStore::default()
        .with_account(account(balance_cents))
        .with_xref(xref())
}

fn input(account_entry: Option<&str>, confirm: Option<&str>) -> BillPaymentInput {
    BillPaymentInput {
        account_id: account_entry.map(str::to_owned),
        confirm: Confirm::parse(confirm),
    }
}

fn draft(balance_cents: i64) -> PaymentDraft {
    PaymentDraft {
        account: account(balance_cents),
        xref: xref(),
    }
}

fn tran_id() -> TranId {
    TranId::move_from(TRAN_ID)
}

/// 2022-07-18 09:30:15.654321 UTC: the fraction of the second must not reach
/// the timestamp text.
fn instant() -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(2022, 7, 18)
        .and_then(|date| date.and_hms_micro_opt(9, 30, 15, 654_321))
        .expect("a valid instant")
        .and_utc()
}

fn clock() -> FixedClock {
    FixedClock(instant())
}

/// The rejection an audit row records: its kind, its message, and the BMS
/// field the cursor goes to, which carries the same message.
fn rejection(kind: ErrorKind, message: &str, field: &'static str) -> DomainError {
    DomainError::new(kind, message).with_field(field)
}

/// P-COBIL00C-015, P-COBIL00C-026: the payment row holds the `COBIL00C`
/// constants, `TRAN-AMT` of `amount_cents`, the cross-reference card and the
/// clock's `WS-TIMESTAMP` in both timestamps.
fn assert_payment_record(row: &Transaction, amount_cents: i64) {
    assert_eq!(row.tran_id, tran_id());
    assert_eq!(row.tran_type_cd, TranTypeCode::move_from("02"));
    assert_eq!(
        row.tran_cat_cd,
        TranCategoryCode::new(2).expect("a 4-digit category")
    );
    assert_eq!(row.tran_source.as_bytes(), b"POS TERM  ");
    assert_eq!(row.tran_desc.trimmed(), "BILL PAYMENT - ONLINE");
    assert_eq!(row.tran_amt, money::<9>(amount_cents));
    assert_eq!(
        row.tran_merchant_id,
        MerchantId::new(999_999_999).expect("a 9-digit merchant")
    );
    assert_eq!(row.tran_merchant_name.trimmed(), "BILL PAYMENT");
    assert_eq!(row.tran_merchant_city.trimmed(), "N/A");
    assert_eq!(row.tran_merchant_zip.as_bytes(), b"N/A       ");
    assert_eq!(row.tran_card_num, CardNumber::move_from(CARD));
    assert_eq!(row.tran_orig_ts.as_bytes(), PAYMENT_TS);
    assert_eq!(row.tran_proc_ts.as_bytes(), PAYMENT_TS);
}

// --- prepare: the edits and reads before the TRAN-ID allocation

/// V-COBIL00C-001, P-COBIL00C-004: a blank or low-values account ID is
/// rejected first, whatever the confirmation holds, and nothing is read.
#[test]
fn a_blank_account_id_is_rejected_before_anything_else() {
    let expected = rejection(ErrorKind::Empty, ACCT_ID_CAN_NOT_BE_EMPTY, FIELD_ACTIDIN);
    let cases = [
        (None, Some("Y")),
        (Some(""), Some("N")),
        (Some("           "), Some("X")),
        (Some("\0\0\0\0\0\0\0\0\0\0\0"), None),
    ];
    for (account_entry, confirm) in cases {
        let mut store = store(BALANCE);
        let outcome = block_on(prepare(&mut store, input(account_entry, confirm)));
        assert_eq!(
            outcome,
            Err(expected.clone()),
            "{account_entry:?} {confirm:?}"
        );
        assert!(store.calls.is_empty(), "{account_entry:?} {confirm:?}");
    }
}

/// V-COBIL00C-002, P-COBIL00C-006, P-COBIL00C-030: `N` or `n` clears the
/// screen with no message and reads nothing, even for an unknown account.
#[test]
fn confirm_n_cancels_without_reading() {
    for (account_entry, confirm) in [
        (ACCOUNT_ENTRY, "N"),
        (ACCOUNT_ENTRY, "n"),
        (UNKNOWN_ENTRY, "N"),
    ] {
        let mut store = store(BALANCE);
        let outcome = block_on(prepare(
            &mut store,
            input(Some(account_entry), Some(confirm)),
        ));
        assert_eq!(
            outcome,
            Ok(BillPaymentStep::Cancelled),
            "{account_entry} {confirm}"
        );
        assert!(store.calls.is_empty(), "{account_entry} {confirm}");
    }
}

/// V-COBIL00C-003, P-COBIL00C-008: a confirmation other than `Y`, `N` or blank
/// is rejected on `CONFIRM` before the account is read.
#[test]
fn an_invalid_confirmation_is_rejected_without_reading() {
    let expected = rejection(
        ErrorKind::Invalid,
        INVALID_VALUE_VALID_VALUES_ARE_Y_N,
        FIELD_CONFIRM,
    );
    for confirm in ["X", "?", "YY", "Y "] {
        let mut store = store(BALANCE);
        let outcome = block_on(prepare(
            &mut store,
            input(Some(ACCOUNT_ENTRY), Some(confirm)),
        ));
        assert_eq!(outcome, Err(expected.clone()), "{confirm:?}");
        assert!(store.calls.is_empty(), "{confirm:?}");
    }
}

/// V-COBIL00C-004 to V-COBIL00C-006, P-COBIL00C-009: the account read for
/// update answers `NOTFND` or fails, and an entry holding a non-digit answers
/// as not found without a read (E-47).
#[test]
fn account_read_failures_answer_with_their_messages() {
    let not_found = rejection(ErrorKind::NotFound, ACCOUNT_ID_NOT_FOUND, FIELD_ACTIDIN);
    let unreadable = rejection(
        ErrorKind::Unexpected,
        UNABLE_TO_LOOKUP_ACCOUNT,
        FIELD_ACTIDIN,
    );
    let read = vec![Call::ReadAccountForUpdate];
    let cases = [
        (UNKNOWN_ENTRY, "Y", None, &not_found, read.clone()),
        (UNKNOWN_ENTRY, "", None, &not_found, read.clone()),
        ("0000000000A", "y", None, &not_found, Vec::new()),
        (
            ACCOUNT_ENTRY,
            "Y",
            Some(PortError::Unavailable("connection reset".to_owned())),
            &unreadable,
            read.clone(),
        ),
        (
            ACCOUNT_ENTRY,
            "",
            Some(PortError::Contention),
            &unreadable,
            read,
        ),
    ];
    for (account_entry, confirm, failure, expected, calls) in cases {
        let mut store = store(BALANCE);
        store.account_read_failure = failure;
        let outcome = block_on(prepare(
            &mut store,
            input(Some(account_entry), Some(confirm)),
        ));
        assert_eq!(
            outcome.as_ref(),
            Err(expected),
            "{account_entry} {confirm:?}"
        );
        assert_eq!(store.calls, calls, "{account_entry} {confirm:?}");
    }
}

/// V-COBIL00C-007, P-COBIL00C-010, P-COBIL00C-020: a balance of zero or less
/// has nothing to pay, confirmed or not, and nothing is written.
#[test]
fn a_balance_of_zero_or_less_has_nothing_to_pay() {
    let expected = rejection(ErrorKind::Invalid, YOU_HAVE_NOTHING_TO_PAY, FIELD_ACTIDIN);
    for (balance, confirm) in [(0, "Y"), (0, ""), (-500, "y"), (-500, "")] {
        let mut store = store(balance);
        let outcome = block_on(prepare(
            &mut store,
            input(Some(ACCOUNT_ENTRY), Some(confirm)),
        ));
        assert_eq!(outcome, Err(expected.clone()), "{balance} {confirm:?}");
        assert_eq!(
            store.calls,
            [Call::ReadAccountForUpdate],
            "{balance} {confirm:?}"
        );
    }
}

/// V-COBIL00C-008, P-COBIL00C-007: a blank confirmation on a positive balance
/// shows the balance and asks for confirmation, a 200 answer with no write.
#[test]
fn a_blank_confirmation_prompts_with_the_balance_and_writes_nothing() {
    for confirm in [None, Some(""), Some(" "), Some("\0")] {
        let mut store = store(BALANCE);
        let outcome = block_on(prepare(&mut store, input(Some(ACCOUNT_ENTRY), confirm)));
        assert_eq!(
            outcome,
            Ok(BillPaymentStep::ConfirmPrompt {
                message: CONFIRM_TO_MAKE_A_BILL_PAYMENT,
                current_balance: money(BALANCE),
            }),
            "{confirm:?}"
        );
        assert_eq!(store.calls, [Call::ReadAccountForUpdate], "{confirm:?}");
        assert!(store.rewrite_attempts.is_empty(), "{confirm:?}");
        assert!(store.transactions.is_empty(), "{confirm:?}");
    }
}

/// V-COBIL00C-009, V-COBIL00C-010, P-COBIL00C-011, P-COBIL00C-025: a confirmed
/// payment whose `CXACAIX` read answers `NOTFND` or fails stops there, with
/// nothing written (E-29).
#[test]
fn a_confirmed_payment_without_its_cross_reference_stops() {
    let cases = [
        (
            None,
            rejection(ErrorKind::NotFound, ACCOUNT_ID_NOT_FOUND, FIELD_ACTIDIN),
        ),
        (
            Some(PortError::Unavailable("index unavailable".to_owned())),
            rejection(
                ErrorKind::Unexpected,
                UNABLE_TO_LOOKUP_XREF_AIX_FILE,
                FIELD_ACTIDIN,
            ),
        ),
    ];
    for (failure, expected) in cases {
        let mut store = MemoryStore::default().with_account(account(BALANCE));
        if failure.is_some() {
            store = store.with_xref(xref());
        }
        store.xref_read_failure = failure;
        let outcome = block_on(prepare(&mut store, input(Some(ACCOUNT_ENTRY), Some("Y"))));
        assert_eq!(outcome, Err(expected.clone()), "{}", expected.message);
        assert_eq!(
            store.calls,
            [Call::ReadAccountForUpdate, Call::ReadXrefByAccount],
            "{}",
            expected.message
        );
    }
}

/// P-COBIL00C-005, P-COBIL00C-011: `Y` or `y` reads the account for update and
/// its cross-reference, and hands both to the payment.
#[test]
fn a_confirmed_payment_is_ready_with_its_records() {
    for confirm in ["Y", "y"] {
        let mut store = store(BALANCE);
        let outcome = block_on(prepare(
            &mut store,
            input(Some(ACCOUNT_ENTRY), Some(confirm)),
        ));
        assert_eq!(
            outcome,
            Ok(BillPaymentStep::Ready(draft(BALANCE))),
            "{confirm}"
        );
        assert_eq!(
            store.calls,
            [Call::ReadAccountForUpdate, Call::ReadXrefByAccount],
            "{confirm}"
        );
    }
}

// --- pay: the payment record, the transaction write and the account rewrite

/// P-COBIL00C-015, P-COBIL00C-016, P-COBIL00C-018, P-COBIL00C-022,
/// P-COBIL00C-026: the draft `prepare` returns is paid in full: the payment
/// row is written, then the balance drops to zero, and the success text names
/// the TRAN-ID the caller allocated.
#[test]
fn a_confirmed_payment_writes_the_record_then_pays_off_the_balance() {
    let mut store = store(BALANCE);
    let step = block_on(prepare(&mut store, input(Some(ACCOUNT_ENTRY), Some("Y"))));
    let Ok(BillPaymentStep::Ready(ready_draft)) = step else {
        panic!("a confirmed payment is ready, not {step:?}");
    };
    let outcome = block_on(pay(&mut store, ready_draft, tran_id(), &clock()));

    let message = payment_successful_message(TRAN_ID);
    assert!(
        message.starts_with(&[PAYMENT_SUCCESSFUL, YOUR_TRANSACTION_ID_IS, TRAN_ID].concat()),
        "{message}"
    );
    assert_eq!(
        outcome,
        Ok(PaymentSuccess {
            tran_id: tran_id(),
            message,
            new_balance: Money::ZERO,
            account_id: account_id(),
        })
    );
    assert_eq!(
        store.calls,
        [
            Call::ReadAccountForUpdate,
            Call::ReadXrefByAccount,
            Call::WriteTransaction,
            Call::RewriteAccount,
        ]
    );
    assert_payment_record(&store.transactions[&tran_id()], BALANCE);
    assert_eq!(store.accounts[&account_id()], account(0));
}

/// P-COBIL00C-021, P-COBIL00C-022: `MOVE ACCT-CURR-BAL TO TRAN-AMT` keeps the
/// low-order nine integer digits, so a balance of 1,000,000,000.00 or more
/// pays only those, and the rewritten balance keeps the rest.
#[test]
fn a_ten_digit_balance_pays_only_its_low_order_nine_digits() {
    let cases = [
        (123_456_789_012, 23_456_789_012, 100_000_000_000),
        (100_000_000_000, 0, 100_000_000_000),
        (99_999_999_999, 99_999_999_999, 0),
    ];
    for (balance, amount, left) in cases {
        let mut store = store(balance);
        let outcome = block_on(pay(&mut store, draft(balance), tran_id(), &clock()));
        let paid = outcome.expect("the payment succeeds");
        assert_eq!(paid.new_balance, money::<10>(left), "{balance}");
        assert_payment_record(&store.transactions[&tran_id()], amount);
        assert_eq!(
            store.accounts[&account_id()].acct_curr_bal,
            money::<10>(left),
            "{balance}"
        );
    }
}

/// V-COBIL00C-014, V-COBIL00C-015, P-COBIL00C-017, P-COBIL00C-018,
/// P-COBIL00C-024 (E-19): a transaction write that answers `DUPKEY`, `DUPREC`
/// or any other failure still reduces and rewrites the balance, and the
/// write's message is the answer. A live row already holding the TRAN-ID is
/// left as it was.
#[test]
fn a_failed_transaction_write_still_rewrites_the_balance() {
    let duplicate = rejection(ErrorKind::Duplicate, TRAN_ID_ALREADY_EXIST, FIELD_ACTIDIN);
    let unwritable = rejection(
        ErrorKind::Unexpected,
        UNABLE_TO_ADD_BILL_PAY_TRANSACTION,
        FIELD_ACTIDIN,
    );
    let live_row = build_transaction(&draft(5_000), tran_id(), instant());
    let cases = [
        (Some(live_row.clone()), None, &duplicate),
        (None, Some(PortError::Duplicate), &duplicate),
        (
            None,
            Some(PortError::Unavailable("disk full".to_owned())),
            &unwritable,
        ),
    ];
    for (existing, failure, expected) in cases {
        let case = format!("{existing:?} {failure:?}");
        let mut store = store(BALANCE);
        if let Some(row) = existing.clone() {
            store = store.with_transaction(row);
        }
        store.write_failure = failure;
        let outcome = block_on(pay(&mut store, draft(BALANCE), tran_id(), &clock()));
        assert_eq!(outcome.as_ref(), Err(expected), "{case}");
        assert_eq!(
            store.calls,
            [Call::WriteTransaction, Call::RewriteAccount],
            "{case}"
        );
        assert_eq!(store.rewrite_attempts, [account(0)], "{case}");
        assert_eq!(store.accounts[&account_id()], account(0), "{case}");
        let stored: Vec<NewTransaction> = store
            .transactions
            .values()
            .map(Transaction::to_new)
            .collect();
        assert_eq!(stored, existing.into_iter().collect::<Vec<_>>(), "{case}");
    }
}

/// V-COBIL00C-016, V-COBIL00C-017, P-COBIL00C-003, P-COBIL00C-016,
/// P-COBIL00C-019 (E-19): a failed rewrite of the reduced balance replaces any
/// earlier message, the last one sent, and a transaction already written stays
/// written.
#[test]
fn a_failed_rewrite_replaces_the_write_outcome() {
    let not_found = rejection(ErrorKind::NotFound, ACCOUNT_ID_NOT_FOUND, FIELD_ACTIDIN);
    let unwritable = rejection(
        ErrorKind::Unexpected,
        UNABLE_TO_UPDATE_ACCOUNT,
        FIELD_ACTIDIN,
    );
    let lost = || PortError::Unavailable("connection lost".to_owned());
    let cases = [
        (None, lost(), &unwritable, true),
        (None, PortError::NotFound, &not_found, true),
        (Some(PortError::Duplicate), lost(), &unwritable, false),
        (Some(lost()), PortError::NotFound, &not_found, false),
    ];
    for (write_failure, rewrite_failure, expected, written) in cases {
        let case = format!("{write_failure:?} {rewrite_failure:?}");
        let mut store = store(BALANCE);
        store.write_failure = write_failure;
        store.rewrite_failure = Some(rewrite_failure);
        let outcome = block_on(pay(&mut store, draft(BALANCE), tran_id(), &clock()));
        assert_eq!(outcome.as_ref(), Err(expected), "{case}");
        assert_eq!(
            store.calls,
            [Call::WriteTransaction, Call::RewriteAccount],
            "{case}"
        );
        assert_eq!(store.rewrite_attempts, [account(0)], "{case}");
        assert_eq!(store.accounts[&account_id()], account(BALANCE), "{case}");
        assert_eq!(
            store.transactions.contains_key(&tran_id()),
            written,
            "{case}"
        );
        if written {
            assert_payment_record(&store.transactions[&tran_id()], BALANCE);
        }
    }
}
