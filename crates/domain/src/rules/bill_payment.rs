//! `COBIL00C` (transaction CB00, map `COBIL00`): pay the account balance in
//! full with an online bill-payment transaction, `POST /payments`.
//!
//! The program runs as one `PROCESS-ENTER-KEY` [app/cbl/COBIL00C.cbl:L154-244]
//! that the replacement splits at the TRAN-ID allocation:
//!
//! 1. [`prepare`]: the account-ID and confirmation edits, the account read for
//!    update, the nothing-to-pay guard, the confirmation prompt and, for a
//!    confirmed payment, the `CXACAIX` cross-reference read
//!    [app/cbl/COBIL00C.cbl:L156-211].
//! 2. The caller allocates the TRAN-ID: `STARTBR` at `HIGH-VALUES`, `READPREV`,
//!    `ENDBR` and `ADD 1` [app/cbl/COBIL00C.cbl:L212-219] are the shared,
//!    serialized allocation of `crates/persistence/src/ids.rs`, which also
//!    answers the empty-master and browse failures (V-COBIL00C-011 to
//!    V-COBIL00C-013).
//! 3. [`pay`]: build the payment record ([`build_transaction`]), write it, then
//!    reduce and rewrite the balance [app/cbl/COBIL00C.cbl:L218-235].
//!
//! # The last message wins
//!
//! `SEND-BILLPAY-SCREEN` has no `RETURN` [app/cbl/COBIL00C.cbl:L289-301]
//! (P-COBIL00C-003): each failing paragraph sets `WS-ERR-FLG` and a message and
//! processing goes on, so the `IF NOT ERR-FLG-ON` guards decide what still runs
//! and the screen finally shows the last message set. Inside the confirmed
//! payment nothing is guarded: the balance is rewritten whatever the
//! transaction write answered (E-19), and a failed rewrite replaces the write's
//! message (P-COBIL00C-019). The adapter runs each of the two writes under its
//! own savepoint and commits both outcomes, as the `RECOVERY(NONE)` VSAM files
//! keep them [app/csd/CARDDEMO.CSD:L1-96] (P-COBIL00C-016, P-COBIL00C-024).
//!
//! Paths that set `WS-ERR-FLG` are a [`DomainError`]; the blank-confirmation
//! prompt sets only a message and is the [`BillPaymentStep::ConfirmPrompt`]
//! outcome, answered 200 with no write (V-COBIL00C-008).
//!
//! Bill payments accrue no reward points: the reward engine serves only CT02
//! and POSTTRAN.

use chrono::{DateTime, Utc};

use super::Confirm;
use crate::clock::{Clock, cics_timestamp};
use crate::entities::{Account, CardXref, NewTransaction};
use crate::error::DomainError;
use crate::messages::{
    ACCOUNT_ID_NOT_FOUND, ACCT_ID_CAN_NOT_BE_EMPTY, CONFIRM_TO_MAKE_A_BILL_PAYMENT,
    INVALID_VALUE_VALID_VALUES_ARE_Y_N, TRAN_ID_ALREADY_EXIST, UNABLE_TO_ADD_BILL_PAY_TRANSACTION,
    UNABLE_TO_LOOKUP_ACCOUNT, UNABLE_TO_LOOKUP_XREF_AIX_FILE, UNABLE_TO_UPDATE_ACCOUNT,
    YOU_HAVE_NOTHING_TO_PAY, payment_successful_message,
};
use crate::numeric::{AccountId, Alnum, MerchantId, Money, TranCategoryCode, TranId, TranTypeCode};
use crate::ports::{AccountReader, AccountWriter, PortError, TransactionWriter, XrefReader};

/// The BMS field `ACTIDIN` (`ACTIDINI X(11)` [app/cpy-bms/COBIL00.CPY:L60]):
/// the field every `COBIL00C` message but two puts the cursor on
/// (`MOVE -1 TO ACTIDINL`).
pub const FIELD_ACTIDIN: &str = "ACTIDIN";

/// The BMS field `CONFIRM` (`CONFIRMI X(1)`): the cursor field of the invalid
/// confirmation and of the confirmation prompt (`MOVE -1 TO CONFIRML`,
/// V-COBIL00C-003, V-COBIL00C-008).
pub const FIELD_CONFIRM: &str = "CONFIRM";

/// `MOVE '02' TO TRAN-TYPE-CD` — P-COBIL00C-015 [app/cbl/COBIL00C.cbl:L220].
const BILL_PAYMENT_TYPE_CD: &str = "02";

/// `MOVE 2 TO TRAN-CAT-CD`, the `9(04)` value 0002 — P-COBIL00C-015
/// [app/cbl/COBIL00C.cbl:L221].
const BILL_PAYMENT_CAT_CD: TranCategoryCode = match TranCategoryCode::new(2) {
    Ok(code) => code,
    // Evaluated at compile time: an out-of-range literal fails the build.
    Err(_) => panic!("2 fits TRAN-CAT-CD PIC 9(04)"),
};

/// `MOVE 'POS TERM' TO TRAN-SOURCE` — P-COBIL00C-015 [app/cbl/COBIL00C.cbl:L222].
const BILL_PAYMENT_SOURCE: &str = "POS TERM";

/// `MOVE 'BILL PAYMENT - ONLINE' TO TRAN-DESC` — P-COBIL00C-015
/// [app/cbl/COBIL00C.cbl:L223].
const BILL_PAYMENT_DESC: &str = "BILL PAYMENT - ONLINE";

/// `MOVE 999999999 TO TRAN-MERCHANT-ID` — P-COBIL00C-015
/// [app/cbl/COBIL00C.cbl:L226].
const BILL_PAYMENT_MERCHANT_ID: MerchantId = match MerchantId::new(999_999_999) {
    Ok(id) => id,
    // Evaluated at compile time: an out-of-range literal fails the build.
    Err(_) => panic!("999999999 fits TRAN-MERCHANT-ID PIC 9(09)"),
};

/// `MOVE 'BILL PAYMENT' TO TRAN-MERCHANT-NAME` — P-COBIL00C-015
/// [app/cbl/COBIL00C.cbl:L227].
const BILL_PAYMENT_MERCHANT_NAME: &str = "BILL PAYMENT";

/// `MOVE 'N/A' TO TRAN-MERCHANT-CITY` and `MOVE 'N/A' TO TRAN-MERCHANT-ZIP` —
/// P-COBIL00C-015 [app/cbl/COBIL00C.cbl:L228-229].
const NOT_APPLICABLE: &str = "N/A";

/// The `COBIL0A` screen input of one ENTER: the account ID (`ACTIDINI X(11)`)
/// and the confirmation (`CONFIRMI X(1)`), as entered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillPaymentInput {
    /// `ACTIDIN`: the account to pay, as entered; `None` when not sent.
    pub account_id: Option<String>,
    /// `CONFIRM`: `Y` pays, `N` cancels, blank asks for confirmation.
    pub confirm: Confirm,
}

impl BillPaymentInput {
    /// `MOVE ACTIDINI OF COBIL0AI TO ACCT-ID XREF-ACCT-ID`
    /// [app/cbl/COBIL00C.cbl:L169-171]: the account key the reads use, `None`
    /// when the entry is not 11 digits.
    ///
    /// The entry is first moved into the `X(11)` screen field, left-justified
    /// and padded with spaces, so a shorter entry such as `1` holds spaces. A
    /// field holding a non-digit gives an unpinned `9(11)` key (E-47, halt),
    /// and the rule answers it as the account `NOTFND` row (V-COBIL00C-006);
    /// `None` stands for that case.
    #[must_use]
    pub fn account_key(&self) -> Option<AccountId> {
        let field = Alnum::<11>::move_from(self.account_id.as_deref().unwrap_or_default());
        if !field.is_numeric() {
            return None;
        }
        field.as_text().parse().ok()
    }
}

/// The records a confirmed payment works on: the account read for update and
/// its first cross-reference through `CXACAIX`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentDraft {
    /// `ACCOUNT-RECORD` read with `UPDATE` (P-COBIL00C-005, P-COBIL00C-009).
    pub account: Account,
    /// `CARD-XREF-RECORD` read by `XREF-ACCT-ID` (P-COBIL00C-011); its card
    /// number becomes `TRAN-CARD-NUM`.
    pub xref: CardXref,
}

/// The outcome of [`prepare`] when no edit failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillPaymentStep {
    /// Confirmation `N` or `n`: `CLEAR-CURRENT-SCREEN` and `WS-ERR-FLG` with no
    /// message, nothing read or written (P-COBIL00C-006, P-COBIL00C-030).
    ///
    /// The audited answer is V-COBIL00C-002: 400, kind invalid, no message
    /// (E-49), field `ACTIDIN` ([`FIELD_ACTIDIN`]). No COBOL text exists for
    /// it, so the rule returns this outcome and the API builds that answer.
    Cancelled,
    /// Blank confirmation on an account with a positive balance: the balance is
    /// shown and `message` asks for confirmation, without `WS-ERR-FLG` and
    /// without a write (P-COBIL00C-007, V-COBIL00C-008, field
    /// [`FIELD_CONFIRM`]); answered 200.
    ConfirmPrompt {
        /// `Confirm to make a bill payment...`.
        message: &'static str,
        /// `ACCT-CURR-BAL`, the balance the screen shows in `CURBAL`.
        current_balance: Money<10>,
    },
    /// Confirmation `Y` or `y`: the records are read and the payment goes on
    /// once the caller has allocated its TRAN-ID.
    Ready(PaymentDraft),
}

/// A payment whose transaction write and account rewrite both succeeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSuccess {
    /// The TRAN-ID of the payment transaction.
    pub tran_id: TranId,
    /// `Payment successful.  Your Transaction ID is <tran-id>.`
    /// (P-COBIL00C-016).
    pub message: String,
    /// `ACCT-CURR-BAL` after the payment, as rewritten (P-COBIL00C-022).
    pub new_balance: Money<10>,
    /// The account paid, for the response that echoes it.
    pub account_id: AccountId,
}

/// `ACTIDINI = SPACES OR LOW-VALUES` on the entered text: nothing entered, or
/// only spaces, or only NUL characters, as [`Confirm::parse`] reads its field.
fn is_blank_entry(text: Option<&str>) -> bool {
    text.is_none_or(|text| {
        text.bytes().all(|byte| byte == b' ') || text.bytes().all(|byte| byte == 0)
    })
}

/// COBIL00C PROCESS-ENTER-KEY — P-COBIL00C-004 to P-COBIL00C-011,
/// P-COBIL00C-020; V-COBIL00C-001 to V-COBIL00C-010.
///
/// The edits and reads before the TRAN-ID allocation, in the COBOL order
/// [app/cbl/COBIL00C.cbl:L156-211]:
///
/// 1. A blank account ID is rejected before anything else (V-COBIL00C-001).
/// 2. The confirmation is classified: `N` cancels without reading
///    ([`BillPaymentStep::Cancelled`], V-COBIL00C-002), any value but `Y`, `N`
///    and blank is rejected without reading (V-COBIL00C-003).
/// 3. `Y` and blank read the account for update (`READ-ACCTDAT-FILE`).
/// 4. A balance of zero or less has nothing to pay (V-COBIL00C-007).
/// 5. Blank asks for confirmation ([`BillPaymentStep::ConfirmPrompt`],
///    V-COBIL00C-008); `Y` reads the cross-reference (`READ-CXACAIX-FILE`) and
///    returns the [`PaymentDraft`].
///
/// Nothing is written here.
///
/// # Errors
///
/// The [`DomainError`] of the first edit or read that fails: empty account ID
/// (`ACTIDIN`), invalid confirmation (`CONFIRM`), account not found or not
/// readable, nothing to pay, cross-reference not found or not readable (all
/// `ACTIDIN`).
pub async fn prepare<S: AccountReader + XrefReader>(
    store: &mut S,
    input: BillPaymentInput,
) -> Result<BillPaymentStep, DomainError> {
    if is_blank_entry(input.account_id.as_deref()) {
        return Err(DomainError::empty(ACCT_ID_CAN_NOT_BE_EMPTY).with_field(FIELD_ACTIDIN));
    }
    let key = input.account_key();
    let confirmed = match input.confirm {
        Confirm::Yes => true,
        Confirm::Blank => false,
        Confirm::No => return Ok(BillPaymentStep::Cancelled),
        Confirm::Other(_) => {
            return Err(
                DomainError::invalid(INVALID_VALUE_VALID_VALUES_ARE_Y_N).with_field(FIELD_CONFIRM)
            );
        }
    };
    let account = read_account(store, key).await?;
    // `IF ACCT-CURR-BAL <= ZEROS AND ACTIDINI NOT = SPACES AND LOW-VALUES`
    // [app/cbl/COBIL00C.cbl:L197-205]: the account ID is known not to be blank.
    if account.acct_curr_bal <= Money::<10>::ZERO {
        return Err(DomainError::invalid(YOU_HAVE_NOTHING_TO_PAY).with_field(FIELD_ACTIDIN));
    }
    if !confirmed {
        return Ok(BillPaymentStep::ConfirmPrompt {
            message: CONFIRM_TO_MAKE_A_BILL_PAYMENT,
            current_balance: account.acct_curr_bal,
        });
    }
    let xref = read_xref(store, account.acct_id).await?;
    Ok(BillPaymentStep::Ready(PaymentDraft { account, xref }))
}

/// COBIL00C READ-ACCTDAT-FILE — P-COBIL00C-009; V-COBIL00C-004 to
/// V-COBIL00C-006.
///
/// `READ DATASET('ACCTDAT') … UPDATE` [app/cbl/COBIL00C.cbl:L343-372]:
/// `NOTFND` answers `Account ID NOT found...`, any other response
/// `Unable to lookup Account...`.
///
/// A key `None` comes from an entry holding a non-digit, whose `9(11)` key the
/// COBOL leaves unpinned (E-47, halt). The escalation's proposal answers it as
/// the `NOTFND` row without a read (V-COBIL00C-006), so no guessed key is ever
/// looked up.
///
/// # Errors
///
/// [`DomainError`] not found or unexpected, on field `ACTIDIN`.
async fn read_account<S: AccountReader>(
    store: &mut S,
    key: Option<AccountId>,
) -> Result<Account, DomainError> {
    let Some(id) = key else {
        return Err(DomainError::not_found(ACCOUNT_ID_NOT_FOUND).with_field(FIELD_ACTIDIN));
    };
    store
        .read_account_for_update(id)
        .await
        .map_err(|error| match error {
            PortError::NotFound => {
                DomainError::not_found(ACCOUNT_ID_NOT_FOUND).with_field(FIELD_ACTIDIN)
            }
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => {
                DomainError::unexpected(UNABLE_TO_LOOKUP_ACCOUNT).with_field(FIELD_ACTIDIN)
            }
        })
}

/// COBIL00C READ-CXACAIX-FILE — P-COBIL00C-011, P-COBIL00C-025;
/// V-COBIL00C-009, V-COBIL00C-010.
///
/// `READ DATASET('CXACAIX')` by `XREF-ACCT-ID` [app/cbl/COBIL00C.cbl:L408-436]:
/// `NOTFND` answers `Account ID NOT found...`, any other response
/// `Unable to lookup XREF AIX file...`.
///
/// The COBOL then still writes the payment, with whatever card number
/// `CARD-XREF-RECORD` holds, because the payment block passed its guard before
/// this read. That result is not pinned (E-29, halt), so the rule returns the
/// escalation's proposal: the message, and no write at all.
///
/// # Errors
///
/// [`DomainError`] not found or unexpected, on field `ACTIDIN`.
async fn read_xref<S: XrefReader>(store: &mut S, id: AccountId) -> Result<CardXref, DomainError> {
    store
        .read_xref_by_account(id)
        .await
        .map_err(|error| match error {
            PortError::NotFound => {
                DomainError::not_found(ACCOUNT_ID_NOT_FOUND).with_field(FIELD_ACTIDIN)
            }
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => {
                DomainError::unexpected(UNABLE_TO_LOOKUP_XREF_AIX_FILE).with_field(FIELD_ACTIDIN)
            }
        })
}

/// COBIL00C PROCESS-ENTER-KEY, GET-CURRENT-TIMESTAMP — P-COBIL00C-015,
/// P-COBIL00C-021, P-COBIL00C-026.
///
/// The payment record [app/cbl/COBIL00C.cbl:L218-232]: `INITIALIZE
/// TRAN-RECORD`, then every field is moved, so nothing `INITIALIZE` set
/// survives. Type `02`, category 0002, source `POS TERM`, description
/// `BILL PAYMENT - ONLINE`, merchant 999999999 `BILL PAYMENT` in `N/A`, `N/A`,
/// the card of the cross-reference, and both timestamps the CICS
/// `ASKTIME`/`FORMATTIME` text of `now`, `YYYY-MM-DD HH:MM:SS.000000`
/// ([`cics_timestamp`]).
///
/// `MOVE ACCT-CURR-BAL TO TRAN-AMT` moves an `S9(10)V99` into an `S9(09)V99`:
/// the sign and the low-order nine integer digits are kept, so a balance of
/// 1234567890.12 pays 234567890.12 (P-COBIL00C-021).
#[must_use]
pub fn build_transaction(
    draft: &PaymentDraft,
    tran_id: TranId,
    now: DateTime<Utc>,
) -> NewTransaction {
    let timestamp = Alnum::move_from(&cics_timestamp(now));
    NewTransaction {
        tran_id,
        tran_type_cd: TranTypeCode::move_from(BILL_PAYMENT_TYPE_CD),
        tran_cat_cd: BILL_PAYMENT_CAT_CD,
        tran_source: Alnum::move_from(BILL_PAYMENT_SOURCE),
        tran_desc: Alnum::move_from(BILL_PAYMENT_DESC),
        tran_amt: draft.account.acct_curr_bal.store::<9>(),
        tran_merchant_id: BILL_PAYMENT_MERCHANT_ID,
        tran_merchant_name: Alnum::move_from(BILL_PAYMENT_MERCHANT_NAME),
        tran_merchant_city: Alnum::move_from(NOT_APPLICABLE),
        tran_merchant_zip: Alnum::move_from(NOT_APPLICABLE),
        tran_card_num: draft.xref.xref_card_num,
        tran_orig_ts: timestamp,
        tran_proc_ts: timestamp,
    }
}

/// COBIL00C PROCESS-ENTER-KEY — P-COBIL00C-015 to P-COBIL00C-019,
/// P-COBIL00C-022, P-COBIL00C-024 (E-19); V-COBIL00C-014 to V-COBIL00C-017.
///
/// The confirmed payment with its allocated `tran_id`
/// [app/cbl/COBIL00C.cbl:L218-235], in the COBOL order:
///
/// 1. Build the record ([`build_transaction`], clock read once) and write it
///    (`WRITE-TRANSACT-FILE`, write 1 of 2).
/// 2. Whatever the write answered, `COMPUTE ACCT-CURR-BAL = ACCT-CURR-BAL -
///    TRAN-AMT`, an `S9(10)V99` result stored with high-order truncation
///    (P-COBIL00C-022), and rewrite the account (`UPDATE-ACCTDAT-FILE`, write 2
///    of 2). A failed write never skips the rewrite (E-19).
/// 3. The last message set is the answer: a failed rewrite replaces any write
///    message; otherwise the write's own outcome stands.
///
/// Each write is one port call, which the adapter runs under its own
/// savepoint; the caller commits whatever this returns, so a write that
/// succeeded stays in place when the other fails.
///
/// # Errors
///
/// - The rewrite failed: `Account ID NOT found...` (not found,
///   V-COBIL00C-016) or `Unable to Update Account...` (unexpected,
///   V-COBIL00C-017), whatever the write answered.
/// - Otherwise the write failed and the reduced balance was rewritten:
///   `Tran ID already exist...` (duplicate, V-COBIL00C-014) or
///   `Unable to Add Bill pay Transaction...` (unexpected, V-COBIL00C-015).
///
/// Every message is on field `ACTIDIN`.
pub async fn pay<S: TransactionWriter + AccountWriter>(
    store: &mut S,
    draft: PaymentDraft,
    tran_id: TranId,
    clock: &impl Clock,
) -> Result<PaymentSuccess, DomainError> {
    let record = build_transaction(&draft, tran_id, clock.now());
    let tran_amt = record.tran_amt;
    let written = write_transaction(store, record).await;
    let new_balance = (draft.account.acct_curr_bal - tran_amt).store::<10>();
    let account_id = draft.account.acct_id;
    let account = Account {
        acct_curr_bal: new_balance,
        ..draft.account
    };
    rewrite_account(store, account).await?;
    let message = written?;
    Ok(PaymentSuccess {
        tran_id,
        message,
        new_balance,
        account_id,
    })
}

/// COBIL00C WRITE-TRANSACT-FILE — P-COBIL00C-016, P-COBIL00C-017;
/// V-COBIL00C-014, V-COBIL00C-015.
///
/// `WRITE DATASET('TRANSACT')` [app/cbl/COBIL00C.cbl:L510-547]. `NORMAL`
/// builds `Payment successful.  Your Transaction ID is <TRAN-ID>.`
/// ([`payment_successful_message`]); `DUPKEY` or `DUPREC` answers
/// `Tran ID already exist...`, any other response
/// `Unable to Add Bill pay Transaction...`.
///
/// # Errors
///
/// [`DomainError`] duplicate or unexpected, on field `ACTIDIN`.
async fn write_transaction<S: TransactionWriter>(
    store: &mut S,
    record: NewTransaction,
) -> Result<String, DomainError> {
    let tran_id = record.tran_id;
    match store.write_transaction(record).await {
        Ok(_) => Ok(payment_successful_message(&tran_id.as_alnum().as_text())),
        Err(PortError::Duplicate) => {
            Err(DomainError::duplicate(TRAN_ID_ALREADY_EXIST).with_field(FIELD_ACTIDIN))
        }
        Err(
            PortError::NotFound
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_),
        ) => {
            Err(DomainError::unexpected(UNABLE_TO_ADD_BILL_PAY_TRANSACTION)
                .with_field(FIELD_ACTIDIN))
        }
    }
}

/// COBIL00C UPDATE-ACCTDAT-FILE — P-COBIL00C-019; V-COBIL00C-016,
/// V-COBIL00C-017.
///
/// `REWRITE DATASET('ACCTDAT')` [app/cbl/COBIL00C.cbl:L377-403]: `NOTFND`
/// answers `Account ID NOT found...`, any other response
/// `Unable to Update Account...`.
///
/// # Errors
///
/// [`DomainError`] not found or unexpected, on field `ACTIDIN`.
async fn rewrite_account<S: AccountWriter>(
    store: &mut S,
    account: Account,
) -> Result<(), DomainError> {
    store
        .rewrite_account(account)
        .await
        .map_err(|error| match error {
            PortError::NotFound => {
                DomainError::not_found(ACCOUNT_ID_NOT_FOUND).with_field(FIELD_ACTIDIN)
            }
            PortError::Duplicate
            | PortError::Restricted
            | PortError::Contention
            | PortError::Unavailable(_) => {
                DomainError::unexpected(UNABLE_TO_UPDATE_ACCOUNT).with_field(FIELD_ACTIDIN)
            }
        })
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Timelike, Utc};

    use super::{
        BillPaymentInput, BillPaymentStep, FIELD_ACTIDIN, FIELD_CONFIRM, PaymentDraft,
        PaymentSuccess, build_transaction, pay, prepare,
    };
    use crate::clock::FixedClock;
    use crate::entities::{Account, CardXref};
    use crate::error::DomainError;
    use crate::messages::{
        ACCOUNT_ID_NOT_FOUND, ACCT_ID_CAN_NOT_BE_EMPTY, CONFIRM_TO_MAKE_A_BILL_PAYMENT,
        INVALID_VALUE_VALID_VALUES_ARE_Y_N, TRAN_ID_ALREADY_EXIST,
        UNABLE_TO_ADD_BILL_PAY_TRANSACTION, UNABLE_TO_LOOKUP_ACCOUNT,
        UNABLE_TO_LOOKUP_XREF_AIX_FILE, UNABLE_TO_UPDATE_ACCOUNT, YOU_HAVE_NOTHING_TO_PAY,
    };
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money,
        TranCategoryCode, TranId, TranTypeCode,
    };
    use crate::ports::PortError;
    use crate::rules::Confirm;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The first `acctdata.txt` account (P-CVACT01Y-015).
    const ACCOUNT: u64 = 1;
    /// Its 11-digit entry on the screen.
    const ACCOUNT_ENTRY: &str = "00000000001";
    /// Its balance in the fixture, 194.00.
    const FIXTURE_BALANCE: i64 = 19_400;
    /// The fixture card of account 1 [app/data/ASCII/cardxref.txt:L49].
    const CARD: &str = "9680294154603697";
    /// The TRAN-ID the caller allocated.
    const TRAN_ID: &str = "0000000000000043";

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an 11-digit account number")
    }

    fn money<const D: u8>(cents: i64) -> Money<D> {
        Money::from_cents(cents).expect("an amount the field holds")
    }

    fn account(balance_cents: i64) -> Account {
        Account {
            acct_id: account_id(ACCOUNT),
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
            xref_acct_id: account_id(ACCOUNT),
        }
    }

    /// Account 1 with `balance_cents` and its cross-reference.
    fn store_with_balance(balance_cents: i64) -> FakeStore {
        FakeStore::default()
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

    /// 2022-07-18 09:30:15.123456 UTC: the sub-second part must not reach the
    /// timestamp text.
    fn instant() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2022, 7, 18, 9, 30, 15)
            .single()
            .and_then(|at| at.with_nanosecond(123_456_000))
            .expect("a valid instant")
    }

    fn clock() -> FixedClock {
        FixedClock(instant())
    }

    fn on_actidin(error: DomainError) -> DomainError {
        error.with_field(FIELD_ACTIDIN)
    }

    /// V-COBIL00C-001, V-COBIL00C-003: the edits that fail before any file is
    /// read, the blank account ID first whatever the confirmation holds.
    #[test]
    fn edits_reject_before_any_read() {
        let empty = on_actidin(DomainError::empty(ACCT_ID_CAN_NOT_BE_EMPTY));
        let invalid =
            DomainError::invalid(INVALID_VALUE_VALID_VALUES_ARE_Y_N).with_field(FIELD_CONFIRM);
        let cases = [
            (None, Some("Y"), &empty),
            (Some(""), None, &empty),
            (Some("           "), Some("N"), &empty),
            (Some("\0\0\0\0\0\0\0\0\0\0\0"), Some("X"), &empty),
            (Some(ACCOUNT_ENTRY), Some("X"), &invalid),
            (Some("99999999999"), Some("?"), &invalid),
            (Some("0000000000A"), Some("1"), &invalid),
        ];
        for (account_entry, confirm, expected) in cases {
            let mut store = store_with_balance(FIXTURE_BALANCE);
            let outcome = block_on(prepare(&mut store, input(account_entry, confirm)));
            assert_eq!(
                outcome.as_ref(),
                Err(expected),
                "{account_entry:?} {confirm:?}"
            );
            assert!(store.calls.is_empty(), "{account_entry:?} {confirm:?}");
        }
    }

    /// V-COBIL00C-002, P-COBIL00C-006: `N` clears the screen with no message
    /// and reads nothing, even for an unknown or non-numeric account.
    #[test]
    fn confirm_n_cancels_without_reading() {
        for (account_entry, confirm) in [
            (ACCOUNT_ENTRY, "N"),
            (ACCOUNT_ENTRY, "n"),
            ("99999999999", "N"),
            ("0000000000A", "n"),
        ] {
            let mut store = store_with_balance(FIXTURE_BALANCE);
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

    /// V-COBIL00C-008, P-COBIL00C-007: a blank confirmation shows the balance
    /// and asks for confirmation, with nothing written.
    #[test]
    fn blank_confirm_prompts_with_the_balance_and_writes_nothing() {
        for confirm in [None, Some(""), Some(" "), Some("\0")] {
            let mut store = store_with_balance(FIXTURE_BALANCE);
            let before = store.clone();
            let outcome = block_on(prepare(&mut store, input(Some(ACCOUNT_ENTRY), confirm)));
            assert_eq!(
                outcome,
                Ok(BillPaymentStep::ConfirmPrompt {
                    message: CONFIRM_TO_MAKE_A_BILL_PAYMENT,
                    current_balance: money(FIXTURE_BALANCE),
                }),
                "{confirm:?}"
            );
            assert_eq!(store.calls, ["read_account_for_update"], "{confirm:?}");
            assert_eq!(store.accounts, before.accounts);
            assert_eq!(store.transactions, before.transactions);
        }
    }

    /// V-COBIL00C-007, P-COBIL00C-010, P-COBIL00C-020: a balance of zero or
    /// less has nothing to pay, confirmed or not, and nothing is written.
    #[test]
    fn a_balance_of_zero_or_less_has_nothing_to_pay() {
        let expected = on_actidin(DomainError::invalid(YOU_HAVE_NOTHING_TO_PAY));
        for (balance, confirm) in [(0, "Y"), (0, ""), (-500, "y"), (-1, "")] {
            let mut store = store_with_balance(balance);
            let outcome = block_on(prepare(
                &mut store,
                input(Some(ACCOUNT_ENTRY), Some(confirm)),
            ));
            assert_eq!(outcome.as_ref(), Err(&expected), "{balance} {confirm:?}");
            assert_eq!(
                store.calls,
                ["read_account_for_update"],
                "{balance} {confirm:?}"
            );
        }
    }

    /// V-COBIL00C-004 to V-COBIL00C-006, P-COBIL00C-009: the account read
    /// fails, and an entry that is not 11 digits answers as not found without
    /// a read (E-47).
    #[test]
    fn account_read_failures_answer_with_their_messages() {
        let not_found = on_actidin(DomainError::not_found(ACCOUNT_ID_NOT_FOUND));
        let unreadable = on_actidin(DomainError::unexpected(UNABLE_TO_LOOKUP_ACCOUNT));
        let read = vec!["read_account_for_update"];
        let cases = [
            ("99999999999", "Y", None, &not_found, read.clone()),
            ("99999999999", "", None, &not_found, read.clone()),
            ("0000000000A", "Y", None, &not_found, vec![]),
            ("1", "", None, &not_found, vec![]),
            (" 0000000001", "Y", None, &not_found, vec![]),
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
            let mut store = store_with_balance(FIXTURE_BALANCE);
            if let Some(error) = failure {
                store.fail_next("read_account_for_update", error);
            }
            let outcome = block_on(prepare(
                &mut store,
                input(Some(account_entry), Some(confirm)),
            ));
            assert_eq!(
                outcome.as_ref(),
                Err(expected),
                "{account_entry:?} {confirm:?}"
            );
            assert_eq!(store.calls, calls, "{account_entry:?} {confirm:?}");
        }
    }

    /// V-COBIL00C-009, V-COBIL00C-010, P-COBIL00C-011: a confirmed payment
    /// whose cross-reference read fails stops there, with nothing written
    /// (E-29).
    #[test]
    fn cross_reference_failures_stop_the_payment() {
        let read_both = ["read_account_for_update", "read_xref_by_account"];

        let mut orphan = FakeStore::default().with_account(account(FIXTURE_BALANCE));
        let outcome = block_on(prepare(&mut orphan, input(Some(ACCOUNT_ENTRY), Some("Y"))));
        assert_eq!(
            outcome,
            Err(on_actidin(DomainError::not_found(ACCOUNT_ID_NOT_FOUND)))
        );
        assert_eq!(orphan.calls, read_both);

        let mut store = store_with_balance(FIXTURE_BALANCE);
        store.fail_next(
            "read_xref_by_account",
            PortError::Unavailable("index unavailable".to_owned()),
        );
        let outcome = block_on(prepare(&mut store, input(Some(ACCOUNT_ENTRY), Some("y"))));
        assert_eq!(
            outcome,
            Err(on_actidin(DomainError::unexpected(
                UNABLE_TO_LOOKUP_XREF_AIX_FILE
            )))
        );
        assert_eq!(store.calls, read_both);
    }

    /// P-COBIL00C-005, P-COBIL00C-011: `Y` reads the account for update and
    /// its cross-reference, and returns both for the payment.
    #[test]
    fn a_confirmed_payment_is_ready_with_its_records() {
        let mut store = store_with_balance(FIXTURE_BALANCE);
        let outcome = block_on(prepare(&mut store, input(Some(ACCOUNT_ENTRY), Some("Y"))));
        assert_eq!(outcome, Ok(BillPaymentStep::Ready(draft(FIXTURE_BALANCE))));
        assert_eq!(
            store.calls,
            ["read_account_for_update", "read_xref_by_account"]
        );
    }

    /// The `MOVE ACTIDINI TO ACCT-ID` key: only 11 digits give one.
    #[test]
    fn account_key_needs_eleven_digits() {
        let cases = [
            (Some(ACCOUNT_ENTRY), Some(ACCOUNT)),
            (Some("00000000000"), Some(0)),
            (Some("99999999999"), Some(99_999_999_999)),
            (Some("1"), None),
            (Some("0000000000A"), None),
            (Some("-0000000001"), None),
            (Some(""), None),
            (None, None),
        ];
        for (account_entry, expected) in cases {
            assert_eq!(
                input(account_entry, None).account_key(),
                expected.map(account_id),
                "{account_entry:?}"
            );
        }
    }

    /// P-COBIL00C-015, P-COBIL00C-026: every field of the payment record.
    #[test]
    fn the_payment_record_holds_the_cobil00c_values() {
        let record = build_transaction(&draft(FIXTURE_BALANCE), tran_id(), instant());
        assert_eq!(record.tran_id, tran_id());
        assert_eq!(record.tran_type_cd, TranTypeCode::move_from("02"));
        assert_eq!(
            record.tran_cat_cd,
            TranCategoryCode::new(2).expect("a 4-digit category")
        );
        assert_eq!(record.tran_cat_cd.to_string(), "0002");
        assert_eq!(record.tran_source.as_bytes(), b"POS TERM  ");
        assert_eq!(record.tran_desc.trimmed(), "BILL PAYMENT - ONLINE");
        assert_eq!(record.tran_amt, money::<9>(FIXTURE_BALANCE));
        assert_eq!(
            record.tran_merchant_id,
            MerchantId::new(999_999_999).expect("a 9-digit merchant")
        );
        assert_eq!(record.tran_merchant_name.trimmed(), "BILL PAYMENT");
        assert_eq!(record.tran_merchant_city.trimmed(), "N/A");
        assert_eq!(record.tran_merchant_zip.as_bytes(), b"N/A       ");
        assert_eq!(record.tran_card_num, CardNumber::move_from(CARD));
        assert_eq!(
            record.tran_orig_ts.as_bytes(),
            b"2022-07-18 09:30:15.000000"
        );
        assert_eq!(record.tran_proc_ts, record.tran_orig_ts);
    }

    /// P-COBIL00C-016, P-COBIL00C-018, P-COBIL00C-022: the payment is written,
    /// the balance drops to zero and the success text names the TRAN-ID.
    #[test]
    fn a_successful_payment_writes_then_rewrites() {
        let mut store = store_with_balance(FIXTURE_BALANCE);
        let outcome = block_on(pay(&mut store, draft(FIXTURE_BALANCE), tran_id(), &clock()));
        assert_eq!(
            outcome,
            Ok(PaymentSuccess {
                tran_id: tran_id(),
                message: "Payment successful.  Your Transaction ID is 0000000000000043.".to_owned(),
                new_balance: Money::ZERO,
                account_id: account_id(ACCOUNT),
            })
        );
        assert_eq!(store.calls, ["write_transaction", "rewrite_account"]);
        let written = store.transactions.get(&tran_id()).expect("the payment row");
        assert_eq!(
            written.to_new(),
            build_transaction(&draft(FIXTURE_BALANCE), tran_id(), instant())
        );
        assert_eq!(written.tran_amt, money::<9>(FIXTURE_BALANCE));
        assert_eq!(store.accounts[&account_id(ACCOUNT)], account(0));
    }

    /// V-COBIL00C-014, V-COBIL00C-015, P-COBIL00C-017, P-COBIL00C-024 (E-19):
    /// a failed transaction write still reduces and rewrites the balance, and
    /// the write's message is the answer.
    #[test]
    fn a_failed_write_still_rewrites_the_balance() {
        let duplicate = on_actidin(DomainError::duplicate(TRAN_ID_ALREADY_EXIST));
        let unwritable = on_actidin(DomainError::unexpected(UNABLE_TO_ADD_BILL_PAY_TRANSACTION));
        let cases = [
            (PortError::Duplicate, &duplicate),
            (PortError::Unavailable("disk full".to_owned()), &unwritable),
            (PortError::Contention, &unwritable),
        ];
        for (failure, expected) in cases {
            let mut store = store_with_balance(FIXTURE_BALANCE);
            store.fail_next("write_transaction", failure.clone());
            let outcome = block_on(pay(&mut store, draft(FIXTURE_BALANCE), tran_id(), &clock()));
            assert_eq!(outcome.as_ref(), Err(expected), "{failure:?}");
            assert_eq!(store.calls, ["write_transaction", "rewrite_account"]);
            assert!(store.transactions.is_empty(), "{failure:?}");
            assert_eq!(
                store.accounts[&account_id(ACCOUNT)],
                account(0),
                "{failure:?}"
            );
        }
    }

    /// V-COBIL00C-014 (E-19): a TRAN-ID a live row already holds keeps that
    /// row unchanged while the balance is still paid off.
    #[test]
    fn a_duplicate_tran_id_keeps_the_live_row_and_pays_the_balance() {
        let existing = build_transaction(&draft(5_000), tran_id(), instant());
        let mut store = store_with_balance(FIXTURE_BALANCE).with_transaction(existing.clone());
        let outcome = block_on(pay(&mut store, draft(FIXTURE_BALANCE), tran_id(), &clock()));
        assert_eq!(
            outcome,
            Err(on_actidin(DomainError::duplicate(TRAN_ID_ALREADY_EXIST)))
        );
        assert_eq!(store.transactions[&tran_id()].to_new(), existing);
        assert_eq!(store.accounts[&account_id(ACCOUNT)], account(0));
    }

    /// V-COBIL00C-016, V-COBIL00C-017, P-COBIL00C-019: a failed rewrite
    /// replaces any earlier message, and a written transaction stays written.
    #[test]
    fn a_failed_rewrite_replaces_the_write_outcome() {
        let not_found = on_actidin(DomainError::not_found(ACCOUNT_ID_NOT_FOUND));
        let unwritable = on_actidin(DomainError::unexpected(UNABLE_TO_UPDATE_ACCOUNT));
        let cases = [
            (None, PortError::NotFound, &not_found, true),
            (
                None,
                PortError::Unavailable("lost".to_owned()),
                &unwritable,
                true,
            ),
            (
                Some(PortError::Duplicate),
                PortError::Contention,
                &unwritable,
                false,
            ),
            (
                Some(PortError::Contention),
                PortError::NotFound,
                &not_found,
                false,
            ),
        ];
        for (write_failure, rewrite_failure, expected, kept) in cases {
            let mut store = store_with_balance(FIXTURE_BALANCE);
            if let Some(error) = write_failure.clone() {
                store.fail_next("write_transaction", error);
            }
            store.fail_next("rewrite_account", rewrite_failure.clone());
            let outcome = block_on(pay(&mut store, draft(FIXTURE_BALANCE), tran_id(), &clock()));
            let case = format!("{write_failure:?} {rewrite_failure:?}");
            assert_eq!(outcome.as_ref(), Err(expected), "{case}");
            assert_eq!(store.transactions.contains_key(&tran_id()), kept, "{case}");
            assert_eq!(
                store.accounts[&account_id(ACCOUNT)],
                account(FIXTURE_BALANCE),
                "{case}"
            );
        }
    }

    /// P-COBIL00C-021, P-COBIL00C-022: a balance of 1,000,000,000.00 or more
    /// loses its high-order digit in the `S9(09)V99` amount, and the balance
    /// keeps what the amount did not pay.
    #[test]
    fn a_ten_digit_balance_pays_only_its_low_order_nine_digits() {
        let cases = [
            (123_456_789_012, 23_456_789_012, 100_000_000_000),
            (999_999_999_999, 99_999_999_999, 900_000_000_000),
            (100_000_000_000, 0, 100_000_000_000),
            (99_999_999_999, 99_999_999_999, 0),
        ];
        for (balance, amount, left) in cases {
            let record = build_transaction(&draft(balance), tran_id(), instant());
            assert_eq!(record.tran_amt, money::<9>(amount), "{balance}");

            let mut store = store_with_balance(balance);
            let outcome = block_on(pay(&mut store, draft(balance), tran_id(), &clock()));
            let paid = outcome.expect("the payment succeeds");
            assert_eq!(paid.new_balance, money::<10>(left), "{balance}");
            assert_eq!(
                store.accounts[&account_id(ACCOUNT)].acct_curr_bal,
                money::<10>(left)
            );
        }
    }

    /// The rule futures are `Send` when the store is, as the API's
    /// multi-threaded runtime needs.
    #[test]
    fn the_rule_futures_are_send() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = store_with_balance(FIXTURE_BALANCE);
        assert_send(&prepare(&mut store, input(Some(ACCOUNT_ENTRY), Some("Y"))));
        assert_send(&pay(
            &mut store,
            draft(FIXTURE_BALANCE),
            tran_id(),
            &clock(),
        ));
    }
}
