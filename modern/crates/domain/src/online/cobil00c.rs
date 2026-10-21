//! COBIL00C, bill payment: the rules of endpoint 15, `POST /api/v1/bill-payments` (transaction
//! `CB00`, map `COBIL0A`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COBIL00C.md` that a rule decides:
//! the account edit (COBIL00C-007, COBIL00C-008), the confirmation (COBIL00C-009 to
//! COBIL00C-012), the outcomes of the account read (COBIL00C-014, COBIL00C-015), the balance and
//! its test (COBIL00C-017 to COBIL00C-019), the payment record and the debit (COBIL00C-020 to
//! COBIL00C-027, COBIL00C-031) and the messages of the writes (COBIL00C-024, COBIL00C-028 to
//! COBIL00C-034), with the bill-payment arithmetic of `modern/audit/arithmetic.md` (ARITH-018 to
//! ARITH-022). The other `server` rows are carried out outside `domain`: the user gate
//! (COBIL00C-003) and the token check (COBIL00C-038) by `api::auth::extract::SignedOnUser`, the
//! read for update (COBIL00C-013) by the service's row lock, and the highest-plus-one ID
//! (COBIL00C-023) by `TransactionRepository::next_id`. The `client` rows, F3, F4 and every other
//! key among them (COBIL00C-004 to COBIL00C-006), send no request and belong to the screen.
//!
//! # Flow
//!
//! `api::services::cobil00c` calls [`edit`] with the two inputs. [`EnterAction::Clear`] answers
//! [`BillPayOutcome::Cleared`] at once; [`EnterAction::ReadAccount`] reads the account, the read
//! for update on the payment path, and answers a failed read with [`account_not_found`] or
//! [`account_lookup_failed`]. [`decide`] then judges the balance: it shows it, refuses it, or
//! orders the payment. For a payment the service reads the cross-reference by account, the
//! transaction with the highest ID and the next ID, calls [`payment`], inserts
//! [`PaymentEffects::transaction`], rewrites [`PaymentEffects::account`] whatever the insert's
//! result (E11), commits, and answers [`payment_outcome`] of the two results.
//!
//! # The last screen is the answer
//!
//! `SEND-BILLPAY-SCREEN` does not end the task: the program goes on under its error flag and
//! can send several screens, each erasing the last, so the answer is the last screen it sends
//! (COBIL00C-034). The payment block tests the flag once, before its first step, so a failed
//! cross-reference read does not stop the payment and its message never reaches the answer
//! (COBIL00C-021, COBIL00C-022, escalation candidate X-COBIL00C-1); this module therefore has no
//! failure for it. After the transaction write, the balance rewrite always runs, and its failure
//! message replaces the write's (COBIL00C-031, COBIL00C-033). A failure is a [`BillPayFailure`]:
//! the literal moved to `WS-MESSAGE`, the request field the cursor goes to and the status class.
//!
//! # Money
//!
//! Every amount passes through `crate::money`: the transaction amount is the balance stored into
//! `TRAN-AMT PIC S9(09)V99`, dropping its high-order digit from 1 000 000 000.00 up (ARITH-020,
//! ARITH-021, E11), and the new balance is the old balance less that amount stored into
//! `ACCT-CURR-BAL PIC S9(10)V99` (ARITH-022). A balance reaches a response only as the
//! 14-character text of `WS-CURR-BAL PIC +9999999999.99` (ARITH-019); the records the service
//! writes keep their typed amounts.
//!
//! # Halts
//!
//! - Halt candidate X-COBIL00C-H1: after `N`, an invalid confirmation or a failed account read,
//!   the program moves the balance of an unread record into the edited balance after its send
//!   (COBIL00C-016). The answer here is the screen it sent before that move, no parity result.
//! - `HALTED(X-COBIL00C-H2)`: with no cross-reference, or a failed read, the card number the
//!   payment writes is bytes no read filled; [`payment`] writes 16 spaces. With several, the
//!   service passes the lowest card number. Neither is a parity result.
//! - `HALTED(X-COBIL00C-H3)`: a failed browse of the transactions ends here in
//!   [`transaction_lookup_failed`], with nothing written; on an empty table the FILLER of the new
//!   record is what no read filled, and [`payment`] writes 20 spaces. Neither is a parity result.
//! - Halt candidate X-COBIL00C-H4: an account input whose first ten characters are digits and
//!   whose eleventh is not names no account here (404), no parity result.
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a value
//! or a [`BillPayFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input). No function reads time except through the injected [`Clock`].

use core::fmt;
use std::iter;

use copybook::edited::EditedPicture;
use copybook::{PicTimestamp, PicU, PicX};

use crate::clock::{Clock, iso_timestamp_zero_micros};
use crate::money::{Money, MoneyExpr};
use crate::records::account::AccountRecord;
use crate::records::card_xref::CardXrefRecord;
use crate::records::transaction::TransactionRecord;
use crate::sensitive::CardNumber;

/// Width of `ACTIDINI PIC X(11)` (`COBIL00.CPY:60`; COBIL00C-008).
const ACCT_ID_WIDTH: usize = 11;

/// Width of `CONFIRMI PIC X(1)` (COBIL00C-009 to COBIL00C-012).
const CONFIRM_WIDTH: usize = 1;

/// The account input is blank (COBIL00C-007).
const ACCT_ID_EMPTY: &str = "Acct ID can NOT be empty...";

/// The confirmation is none of `Y`, `y`, `N`, `n` and blank (COBIL00C-012).
const CONFIRM_INVALID: &str = "Invalid value. Valid values are (Y/N)...";

/// No account has the key, on the read or on the rewrite (COBIL00C-014, COBIL00C-032).
const ACCOUNT_NOT_FOUND: &str = "Account ID NOT found...";

/// The account read failed (COBIL00C-015).
const ACCOUNT_LOOKUP_FAILED: &str = "Unable to lookup Account...";

/// The balance is zero or negative (COBIL00C-018, ARITH-018).
const NOTHING_TO_PAY: &str = "You have nothing to pay...";

/// A blank confirmation over a positive balance (COBIL00C-019).
const CONFIRM_PROMPT: &str = "Confirm to make a bill payment...";

/// The browse for the highest transaction ID failed (COBIL00C-024, `HALTED(X-COBIL00C-H3)`).
const TRAN_LOOKUP_FAILED: &str = "Unable to lookup Transaction...";

/// The three `DELIMITED BY SIZE` literals around the new ID in the success message
/// (`COBIL00C.cbl:527-530`; COBIL00C-028).
const SUCCESS_HEAD: &str = "Payment successful. ";
const SUCCESS_MIDDLE: &str = " Your Transaction ID is ";
const SUCCESS_TAIL: &str = ".";

/// The new transaction's ID is already taken (COBIL00C-029).
const TRAN_ID_EXISTS: &str = "Tran ID already exist...";

/// The transaction write failed for another reason (COBIL00C-030).
const TRAN_WRITE_FAILED: &str = "Unable to Add Bill pay Transaction...";

/// The balance rewrite failed (COBIL00C-033).
const ACCOUNT_UPDATE_FAILED: &str = "Unable to Update Account...";

/// `MOVE '02' TO TRAN-TYPE-CD` (`COBIL00C.cbl:220`; COBIL00C-025).
const TRAN_TYPE_CD: PicX<2> = PicX::from_array(*b"02");

/// `MOVE 2 TO TRAN-CAT-CD`, stored as `0002` in `PIC 9(04)` (`COBIL00C.cbl:221`; COBIL00C-025).
/// The constant is evaluated at compile time, so a value that did not fit would fail the build.
const TRAN_CAT_CD: PicU<4> = {
    let Ok(code) = PicU::new(2) else {
        panic!("category 2 fits PIC 9(04)")
    };
    code
};

/// `MOVE 'POS TERM' TO TRAN-SOURCE` (`COBIL00C.cbl:222`; COBIL00C-025).
const TRAN_SOURCE: &[u8] = b"POS TERM";

/// `MOVE 'BILL PAYMENT - ONLINE' TO TRAN-DESC` (`COBIL00C.cbl:223`; COBIL00C-025).
const TRAN_DESC: &[u8] = b"BILL PAYMENT - ONLINE";

/// `MOVE 999999999 TO TRAN-MERCHANT-ID`, `PIC 9(09)` (`COBIL00C.cbl:226`; COBIL00C-025).
/// Evaluated at compile time, as [`TRAN_CAT_CD`].
const TRAN_MERCHANT_ID: PicU<9> = {
    let Ok(id) = PicU::new(999_999_999) else {
        panic!("999999999 fits PIC 9(09)")
    };
    id
};

/// `MOVE 'BILL PAYMENT' TO TRAN-MERCHANT-NAME` (`COBIL00C.cbl:227`; COBIL00C-025).
const TRAN_MERCHANT_NAME: &[u8] = b"BILL PAYMENT";

/// `MOVE 'N/A' TO TRAN-MERCHANT-CITY` and `TO TRAN-MERCHANT-ZIP` (`COBIL00C.cbl:228-229`;
/// COBIL00C-025).
const TRAN_MERCHANT_NOT_APPLICABLE: &[u8] = b"N/A";

/// The request field a failure or an outcome names: where the COBOL puts the cursor
/// (`MOVE -1 TO ACTIDINL` or `CONFIRML`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BillPayField {
    /// `ACTIDIN`, the account: every failure but the invalid confirmation, and the cleared and
    /// paid screens (COBIL00C-007, COBIL00C-010, COBIL00C-014, COBIL00C-015, COBIL00C-018,
    /// COBIL00C-024, COBIL00C-028 to COBIL00C-033).
    AcctId,
    /// `CONFIRM`: the invalid confirmation and the confirmation prompt (COBIL00C-012,
    /// COBIL00C-019).
    Confirm,
}

/// The status class of a [`BillPayFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// An edit of the request or of the balance failed: 400 (COBIL00C-007, COBIL00C-012,
    /// COBIL00C-018).
    Input,
    /// No account has the key: 404 (COBIL00C-014, COBIL00C-032).
    NotFound,
    /// A read or write of the store failed, the duplicate transaction ID included: 500
    /// (COBIL00C-015, COBIL00C-024, COBIL00C-029, COBIL00C-030, COBIL00C-033).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400, 404 or 500.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Store => 500,
        }
    }
}

/// A screen COBIL00C ends on instead of a payment or a balance: the message, the field in error
/// and the status class.
///
/// The message is the literal moved to `WS-MESSAGE PIC X(80)`, as the response carries it; every
/// one fits the 78-byte message row. None holds a request value, an amount or a card number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BillPayFailure {
    message: &'static str,
    field: BillPayField,
    class: FailureClass,
}

impl BillPayFailure {
    /// The failure with this message, field and class.
    const fn new(message: &'static str, field: BillPayField, class: FailureClass) -> Self {
        Self {
            message,
            field,
            class,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field the cursor goes to.
    #[must_use]
    pub const fn field(&self) -> BillPayField {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for BillPayFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for BillPayFailure {}

/// The failure of a blank account input: `Acct ID can NOT be empty...` on `acct_id`, status
/// class [`FailureClass::Input`] (COBIL00C-007).
const fn account_empty() -> BillPayFailure {
    BillPayFailure::new(ACCT_ID_EMPTY, BillPayField::AcctId, FailureClass::Input)
}

/// The failure of an unknown confirmation: `Invalid value. Valid values are (Y/N)...` on
/// `confirm`, status class [`FailureClass::Input`] (COBIL00C-012).
const fn confirm_invalid() -> BillPayFailure {
    BillPayFailure::new(CONFIRM_INVALID, BillPayField::Confirm, FailureClass::Input)
}

/// The failure of a zero or negative balance: `You have nothing to pay...` on `acct_id`, status
/// class [`FailureClass::Input`] (COBIL00C-018, ARITH-018).
const fn nothing_to_pay() -> BillPayFailure {
    BillPayFailure::new(NOTHING_TO_PAY, BillPayField::AcctId, FailureClass::Input)
}

/// The failure of an account read that finds no account: `Account ID NOT found...` on
/// `acct_id`, status class [`FailureClass::NotFound`] (COBIL00C-014).
///
/// [`edit`] returns it for an input that names no stored key, which no read can find
/// (COBIL00C-008); the service returns it when the read finds no row. Nothing is written.
#[must_use]
pub const fn account_not_found() -> BillPayFailure {
    BillPayFailure::new(
        ACCOUNT_NOT_FOUND,
        BillPayField::AcctId,
        FailureClass::NotFound,
    )
}

/// The failure of an account read that fails for any reason other than not found:
/// `Unable to lookup Account...` on `acct_id`, status class [`FailureClass::Store`]
/// (COBIL00C-015). Nothing is written.
#[must_use]
pub const fn account_lookup_failed() -> BillPayFailure {
    BillPayFailure::new(
        ACCOUNT_LOOKUP_FAILED,
        BillPayField::AcctId,
        FailureClass::Store,
    )
}

/// The failure of the browse for the highest transaction ID: `Unable to lookup Transaction...`
/// on `acct_id`, status class [`FailureClass::Store`] (COBIL00C-024).
///
/// `HALTED(X-COBIL00C-H3)`: the COBOL sends this message and runs on with no active browse or
/// with the key the failed command left; the service instead returns this failure and writes
/// nothing, the replacement's choice and no parity result. An empty table is no failure: the
/// new ID is then `0000000000000001`.
#[must_use]
pub const fn transaction_lookup_failed() -> BillPayFailure {
    BillPayFailure::new(
        TRAN_LOOKUP_FAILED,
        BillPayField::AcctId,
        FailureClass::Store,
    )
}

/// The two inputs of `COBIL0A`, each the raw text of its map field (`BillPaymentRequest` of
/// endpoint 15). An absent field is the empty text.
///
/// The balance the terminal returns is never an input (COBIL00C-037).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct BillPayInput<'a> {
    /// `ACTIDIN`, `PIC X(11)`, read by [`edit_account`].
    pub acct_id: &'a str,
    /// `CONFIRM`, `PIC X(1)`, read by [`confirm`].
    pub confirm: &'a str,
}

/// The confirmation as `PROCESS-ENTER-KEY` evaluates `CONFIRMI` (`COBIL00C.cbl:173-191`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Confirm {
    /// `Y` or `y`: pay (COBIL00C-009).
    Yes,
    /// `N` or `n`: clear the screen (COBIL00C-010).
    No,
    /// Spaces or low-values: show the balance (COBIL00C-011).
    Blank,
    /// Any other value (COBIL00C-012).
    Invalid,
}

/// The confirmation `raw` names, read as a `MOVE` into `CONFIRMI PIC X(1)`: its first
/// character, a space when it is empty (COBIL00C-009 to COBIL00C-012).
///
/// The request DTO admits at most one character (`COBIL00-020` of `modern/audit/screens.md`).
#[must_use]
pub fn confirm(raw: &str) -> Confirm {
    let field = fit(raw, CONFIRM_WIDTH);
    if is_blank(&field) {
        return Confirm::Blank;
    }
    match field.as_str() {
        "Y" | "y" => Confirm::Yes,
        "N" | "n" => Confirm::No,
        _ => Confirm::Invalid,
    }
}

/// What the account input names once it is not blank (COBIL00C-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountKey {
    /// Eleven digits: the key `ACCT-ID PIC 9(11)` and `XREF-ACCT-ID PIC 9(11)` read by value,
    /// leading zeros included.
    Id(PicU<11>),
    /// Any other text, which no stored key equals, so a read of it ends not found
    /// (COBIL00C-014). A value whose first ten characters are digits and whose eleventh is not,
    /// a ten-digit entry among them, is halt candidate X-COBIL00C-H4, here as unmatched as any.
    Unmatched,
}

/// Edits the account input `raw`, read as a `MOVE` into `ACTIDINI PIC X(11)`: its first eleven
/// characters, padded with spaces (COBIL00C-007, COBIL00C-008).
///
/// The request DTO admits at most eleven characters (`COBIL00-013` of
/// `modern/audit/screens.md`). The input is blank when every character is a space, or every one
/// is `\0`, the COBOL's `SPACES` and `LOW-VALUES`.
///
/// An input that names no stored key is [`AccountKey::Unmatched`], not a failure: the program
/// judges the confirmation before it reads the account, so `N` still clears and an unknown
/// confirmation still fails with such an input (COBIL00C-010, COBIL00C-012). [`edit`] turns it
/// into the read's not-found failure only on the paths that read.
///
/// # Errors
///
/// `Acct ID can NOT be empty...` on `acct_id`, status class [`FailureClass::Input`], when the
/// input is blank (COBIL00C-007).
pub fn edit_account(raw: &str) -> Result<AccountKey, BillPayFailure> {
    let field = fit(raw, ACCT_ID_WIDTH);
    if is_blank(&field) {
        return Err(account_empty());
    }
    Ok(PicU::<11>::decode(field.as_bytes()).map_or(AccountKey::Unmatched, AccountKey::Id))
}

/// Why the account is read: the confirmation that leads `PROCESS-ENTER-KEY` to
/// `READ-ACCTDAT-FILE` (`COBIL00C.cbl:174-184`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadIntent {
    /// `Y` or `y`: the read for update that precedes the payment (COBIL00C-009, COBIL00C-013).
    Pay,
    /// A blank confirmation: the read whose balance is shown (COBIL00C-011).
    ShowBalance,
}

/// What an ENTER asks the service to do once the inputs pass [`edit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnterAction {
    /// `N` or `n`: answer [`BillPayOutcome::Cleared`]; the account is neither read nor
    /// validated, and nothing is written (COBIL00C-010).
    Clear,
    /// Read account `id`, for update when `intent` is [`ReadIntent::Pay`], then [`decide`]
    /// (COBIL00C-009, COBIL00C-011, COBIL00C-013).
    ReadAccount {
        /// The account key.
        id: PicU<11>,
        /// Why the account is read.
        intent: ReadIntent,
    },
}

/// The edits of `PROCESS-ENTER-KEY` before any read, in the program's order: the blank account
/// (COBIL00C-007), then the confirmation (COBIL00C-009 to COBIL00C-012), then the key
/// (COBIL00C-008, COBIL00C-014).
///
/// So `N` clears and an unknown confirmation fails whatever the account input holds, once it is
/// not blank: the program reads no account on those paths. An input that names no stored key
/// fails as the read of it would.
///
/// # Errors
///
/// - `Acct ID can NOT be empty...` on `acct_id`, status class [`FailureClass::Input`], for a
///   blank account, whatever the confirmation (COBIL00C-007);
/// - `Invalid value. Valid values are (Y/N)...` on `confirm`, status class
///   [`FailureClass::Input`], for any confirmation other than `Y`, `y`, `N`, `n` and blank
///   (COBIL00C-012);
/// - [`account_not_found`] for a `Y`, `y` or blank confirmation and an account input other than
///   eleven digits (COBIL00C-008, COBIL00C-014).
pub fn edit(input: &BillPayInput<'_>) -> Result<EnterAction, BillPayFailure> {
    let key = edit_account(input.acct_id)?;
    let intent = match confirm(input.confirm) {
        Confirm::No => return Ok(EnterAction::Clear),
        Confirm::Invalid => return Err(confirm_invalid()),
        Confirm::Yes => ReadIntent::Pay,
        Confirm::Blank => ReadIntent::ShowBalance,
    };
    match key {
        AccountKey::Id(id) => Ok(EnterAction::ReadAccount { id, intent }),
        AccountKey::Unmatched => Err(account_not_found()),
    }
}

/// A screen COBIL00C ends on with status 200.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BillPayOutcome {
    /// `N` or `n`: no balance, an empty message and no transaction ID; the screen empties the
    /// account and the confirmation and focuses `acct_id` (COBIL00C-010).
    Cleared,
    /// A blank confirmation over a positive balance: the balance, the prompt
    /// `Confirm to make a bill payment...` and no transaction ID; the screen keeps the account
    /// and focuses `confirm`. Nothing is written (COBIL00C-017, COBIL00C-019).
    BalanceShown {
        /// The account shown, the key that was read.
        acct_id: PicU<11>,
        /// The balance as `WS-CURR-BAL PIC +9999999999.99` edits it (ARITH-019).
        balance: String,
    },
    /// The payment was written and the balance rewritten: the success message with the new ID
    /// and no balance; the screen empties every field and focuses `acct_id` (COBIL00C-028).
    Paid {
        /// The new transaction's ID.
        tran_id: PicX<16>,
    },
}

impl BillPayOutcome {
    /// The message the screen shows: `None` for the empty message of [`Self::Cleared`]
    /// (COBIL00C-010), the prompt of [`Self::BalanceShown`] (COBIL00C-019) and the
    /// [`success_message`] of [`Self::Paid`] (COBIL00C-028).
    #[must_use]
    pub fn message(&self) -> Option<String> {
        match self {
            Self::Cleared => None,
            Self::BalanceShown { .. } => Some(CONFIRM_PROMPT.to_owned()),
            Self::Paid { tran_id } => Some(success_message(tran_id)),
        }
    }

    /// The edited balance, shown only by [`Self::BalanceShown`] (COBIL00C-017).
    #[must_use]
    pub fn balance(&self) -> Option<&str> {
        match self {
            Self::BalanceShown { balance, .. } => Some(balance),
            Self::Cleared | Self::Paid { .. } => None,
        }
    }

    /// The account the screen keeps, `None` where it is emptied (COBIL00C-010, COBIL00C-019,
    /// COBIL00C-028).
    #[must_use]
    pub const fn acct_id(&self) -> Option<PicU<11>> {
        match self {
            Self::BalanceShown { acct_id, .. } => Some(*acct_id),
            Self::Cleared | Self::Paid { .. } => None,
        }
    }

    /// The new transaction's ID, only after a payment (COBIL00C-028).
    #[must_use]
    pub const fn tran_id(&self) -> Option<&PicX<16>> {
        match self {
            Self::Paid { tran_id } => Some(tran_id),
            Self::Cleared | Self::BalanceShown { .. } => None,
        }
    }

    /// The field the cursor goes to: `confirm` for the prompt, `acct_id` otherwise
    /// (COBIL00C-010, COBIL00C-019, COBIL00C-028).
    #[must_use]
    pub const fn field(&self) -> BillPayField {
        match self {
            Self::BalanceShown { .. } => BillPayField::Confirm,
            Self::Cleared | Self::Paid { .. } => BillPayField::AcctId,
        }
    }
}

/// What [`decide`] orders once the account is read and its balance is positive.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BillPayDecision {
    /// A blank confirmation: answer this [`BillPayOutcome::BalanceShown`]; nothing is written
    /// (COBIL00C-019).
    ShowBalance(BillPayOutcome),
    /// `Y` or `y`: make the [`payment`] (COBIL00C-020 onward).
    Pay,
}

/// `ACCT-CURR-BAL` as `MOVE ACCT-CURR-BAL TO WS-CURR-BAL` edits it into
/// `PIC +9999999999.99`: 14 characters, the sign, all ten integer digits with leading zeros, the
/// point and two decimals, so `843.00` is `+0000000843.00` and `-25.00` is `-0000000025.00`
/// (`COBIL00C.cbl:56,193-194`; ARITH-019, COBIL00C-017).
#[must_use]
pub fn balance_display(balance: Money<10>) -> String {
    EditedPicture::Plus9x10Dot99.format(balance.as_decimal())
}

/// Judges the balance of the account read for `intent` (`COBIL00C.cbl:193-242`).
///
/// `IF ACCT-CURR-BAL <= ZEROS` compares the exact balance with zero (ARITH-018). The other half
/// of that condition, an account input that is not blank, always holds here, because [`edit`]
/// has refused a blank one. A positive balance is shown with the prompt for a blank
/// confirmation (COBIL00C-017, COBIL00C-019) and paid for `Y` (COBIL00C-020 onward).
///
/// # Errors
///
/// `You have nothing to pay...` on `acct_id`, status class [`FailureClass::Input`], when the
/// balance is zero or negative, whatever `intent`; nothing is written (COBIL00C-018). The COBOL
/// screen also shows the balance, which the error does not carry (X-COBIL00C-3).
pub fn decide(
    intent: ReadIntent,
    account: &AccountRecord,
) -> Result<BillPayDecision, BillPayFailure> {
    if account.curr_bal <= Money::<10>::ZERO {
        return Err(nothing_to_pay());
    }
    Ok(match intent {
        ReadIntent::ShowBalance => BillPayDecision::ShowBalance(BillPayOutcome::BalanceShown {
            acct_id: account.id,
            balance: balance_display(account.curr_bal),
        }),
        ReadIntent::Pay => BillPayDecision::Pay,
    })
}

/// The two records a payment writes, in the order the service writes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentEffects {
    /// The `TRAN-RECORD` to insert first (`WRITE-TRANSACT-FILE`; COBIL00C-025 to COBIL00C-027).
    pub transaction: TransactionRecord,
    /// The account to rewrite next, whatever the insert's result: the record read with only its
    /// balance changed (`UPDATE-ACCTDAT-FILE`; COBIL00C-031, E11).
    pub account: AccountRecord,
}

/// The records of a payment of `account` under the new transaction ID `new_id`
/// (`COBIL00C.cbl:212-235`; COBIL00C-020, COBIL00C-025 to COBIL00C-027, COBIL00C-031).
///
/// - `prior` is the transaction with the highest ID, the record `READPREV` left in
///   `TRAN-RECORD` (`TransactionRepository::find_last`, read with the new ID): `INITIALIZE`
///   skips FILLER, so the new record keeps its 20 FILLER bytes (COBIL00C-025, PIC-012). On an
///   empty table that FILLER is what no read filled, `HALTED(X-COBIL00C-H3)`; 20 spaces are
///   written, no parity result.
/// - `xref` is the cross-reference read by account through `CXACAIX`; its card number is the
///   transaction's (COBIL00C-020). With none, or after a failed read, the card number is
///   `HALTED(X-COBIL00C-H2)` and 16 spaces are written; with several, the service passes the one
///   with the lowest card number. Neither is a parity result (COBIL00C-021, COBIL00C-022).
/// - The amount is the whole balance stored into `TRAN-AMT PIC S9(09)V99`, its high-order digit
///   dropped from 1 000 000 000.00 up, so a balance of 1 000 000 000.00 pays 0.00 (ARITH-020,
///   ARITH-021, E11, COBIL00C-026).
/// - Both timestamps are one reading of `clock`, layout `I`, `YYYY-MM-DD HH:MM:SS.000000`
///   (COBIL00C-027, PIC-018, E33).
/// - The account keeps every field it was read with but the balance, the old balance less the
///   amount (ARITH-022, COBIL00C-031).
#[must_use]
pub fn payment(
    new_id: PicX<16>,
    prior: Option<&TransactionRecord>,
    account: &AccountRecord,
    xref: Option<&CardXrefRecord>,
    clock: &dyn Clock,
) -> PaymentEffects {
    // ARITH-020, ARITH-021 (E11): `MOVE ACCT-CURR-BAL TO TRAN-AMT` stores the S9(10)V99
    // balance into S9(09)V99, dropping its high-order digit.
    let tran_amt: Money<9> = MoneyExpr::from(account.curr_bal).store::<9>();
    // ARITH-022: `COMPUTE ACCT-CURR-BAL = ACCT-CURR-BAL - TRAN-AMT`, stored into S9(10)V99.
    let new_balance: Money<10> = (account.curr_bal - tran_amt).store::<10>();
    // One `GET-CURRENT-TIMESTAMP`, moved to both fields (COBIL00C-027).
    let stamp = PicTimestamp::new(Some(iso_timestamp_zero_micros(clock.now())));
    // HALTED(X-COBIL00C-H2) when `xref` is `None`: 16 spaces stand in for unfilled bytes.
    let card_num = xref.map_or_else(
        || CardNumber::new(PicX::spaces()),
        |found| found.card_num.clone(),
    );
    // HALTED(X-COBIL00C-H3) when `prior` is `None`: 20 spaces stand in for unfilled bytes.
    let filler = prior.map_or_else(PicX::spaces, |last| last.filler.clone());
    let transaction = TransactionRecord {
        id: new_id,
        type_cd: TRAN_TYPE_CD,
        cat_cd: TRAN_CAT_CD,
        source: PicX::moved_from(TRAN_SOURCE),
        desc: PicX::moved_from(TRAN_DESC),
        amt: tran_amt,
        merchant_id: TRAN_MERCHANT_ID,
        merchant_name: PicX::moved_from(TRAN_MERCHANT_NAME),
        merchant_city: PicX::moved_from(TRAN_MERCHANT_NOT_APPLICABLE),
        merchant_zip: PicX::moved_from(TRAN_MERCHANT_NOT_APPLICABLE),
        card_num,
        orig_ts: stamp,
        proc_ts: stamp,
        filler,
    };
    let account = AccountRecord {
        curr_bal: new_balance,
        ..account.clone()
    };
    PaymentEffects {
        transaction,
        account,
    }
}

/// The message of a written payment: `Payment successful. `, ` Your Transaction ID is `, the
/// ID up to its first space and `.`, as the `STRING` of `WRITE-TRANSACT-FILE` builds it; a
/// 16-digit ID gives 61 characters, with two spaces after `successful.`
/// (`COBIL00C.cbl:527-531`; COBIL00C-028).
///
/// `INITIALIZE-ALL-FIELDS` and `MOVE SPACES` empty `WS-MESSAGE PIC X(80)` first, so nothing
/// precedes the text, and at most 61 characters always fit it.
#[must_use]
pub fn success_message(tran_id: &PicX<16>) -> String {
    let id = String::from_utf8_lossy(delimited_by_space(tran_id.as_bytes()));
    format!("{SUCCESS_HEAD}{SUCCESS_MIDDLE}{id}{SUCCESS_TAIL}")
}

/// The result of inserting [`PaymentEffects::transaction`] (`WRITE-TRANSACT-FILE`,
/// `COBIL00C.cbl:510-547`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranWrite {
    /// The row was inserted (COBIL00C-028).
    Written,
    /// A row with the new ID already exists, `DUPREC` (COBIL00C-029).
    Duplicate,
    /// The insert failed for any other reason (COBIL00C-030).
    Failed,
}

/// The result of rewriting [`PaymentEffects::account`] (`UPDATE-ACCTDAT-FILE`,
/// `COBIL00C.cbl:377-403`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountRewrite {
    /// The balance was stored (COBIL00C-031).
    Rewritten,
    /// The update matched no row, which the row lock of the read prevents (COBIL00C-032,
    /// X-COBIL00C-2).
    NotFound,
    /// The update failed for any other reason (COBIL00C-033).
    Failed,
}

/// The answer to a payment once both writes ran, by the program's rule that the last message
/// set wins (`COBIL00C.cbl:233-242`; COBIL00C-028 to COBIL00C-034).
///
/// The rewrite runs after the insert whatever its result, so a failed rewrite's message replaces
/// the insert's; a rewritten balance leaves the insert's own message. The service commits both
/// writes whatever they gave, as the task's normal `RETURN` does (COBIL00C-034).
///
/// # Errors
///
/// - `Account ID NOT found...`, status class [`FailureClass::NotFound`], when the rewrite matched
///   no row (COBIL00C-032);
/// - `Unable to Update Account...`, status class [`FailureClass::Store`], when the rewrite failed
///   (COBIL00C-033);
/// - otherwise, after a rewritten balance, `Tran ID already exist...` for a duplicate ID
///   (COBIL00C-029) and `Unable to Add Bill pay Transaction...` for another failed insert
///   (COBIL00C-030), both status class [`FailureClass::Store`]: the account is debited with no
///   transaction recording it (E11).
///
/// Every failure names `acct_id`.
pub fn payment_outcome(
    new_id: &PicX<16>,
    write: TranWrite,
    rewrite: AccountRewrite,
) -> Result<BillPayOutcome, BillPayFailure> {
    let store = |message| BillPayFailure::new(message, BillPayField::AcctId, FailureClass::Store);
    match (write, rewrite) {
        // The rewrite's not-found literal is the read's (COBIL00C-032).
        (_, AccountRewrite::NotFound) => Err(account_not_found()),
        (_, AccountRewrite::Failed) => Err(store(ACCOUNT_UPDATE_FAILED)),
        (TranWrite::Duplicate, AccountRewrite::Rewritten) => Err(store(TRAN_ID_EXISTS)),
        (TranWrite::Failed, AccountRewrite::Rewritten) => Err(store(TRAN_WRITE_FAILED)),
        (TranWrite::Written, AccountRewrite::Rewritten) => Ok(BillPayOutcome::Paid {
            tran_id: new_id.clone(),
        }),
    }
}

/// `text` moved into a `PIC X(width)` field: its first `width` characters, padded with spaces.
fn fit(text: &str, width: usize) -> String {
    text.chars().chain(iter::repeat(' ')).take(width).collect()
}

/// `= SPACES OR LOW-VALUES`: every character a space, or every one `\0`.
fn is_blank(field: &str) -> bool {
    field.chars().all(|c| c == ' ') || field.chars().all(|c| c == '\0')
}

/// A `STRING` source `DELIMITED BY SPACE`: the bytes before its first space.
fn delimited_by_space(bytes: &[u8]) -> &[u8] {
    bytes.split(|&byte| byte == b' ').next().unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;
    use copybook::{FixedRecord, PicDate, PicU, PicX, TsForm};

    use super::{
        AccountKey, AccountRewrite, BillPayDecision, BillPayFailure, BillPayField, BillPayInput,
        BillPayOutcome, Confirm, EnterAction, FailureClass, ReadIntent, TranWrite,
        account_lookup_failed, account_not_found, balance_display, confirm, decide, edit,
        edit_account, payment, payment_outcome, success_message, transaction_lookup_failed,
    };
    use crate::clock::FixedClock;
    use crate::money::{Decimal, Money};
    use crate::records::account::AccountRecord;
    use crate::records::card_xref::CardXrefRecord;
    use crate::records::transaction::TransactionRecord;
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A failure as the response shows it: message, field and HTTP status.
    type Shown = (&'static str, BillPayField, u16);

    // The audited literals, written out apart from the module's own constants.
    const EMPTY: &str = "Acct ID can NOT be empty...";
    const INVALID: &str = "Invalid value. Valid values are (Y/N)...";
    const NOT_FOUND: &str = "Account ID NOT found...";
    const LOOKUP: &str = "Unable to lookup Account...";
    const NOTHING: &str = "You have nothing to pay...";
    const PROMPT: &str = "Confirm to make a bill payment...";
    const TRAN_LOOKUP: &str = "Unable to lookup Transaction...";
    const EXISTS: &str = "Tran ID already exist...";
    const ADD_FAILED: &str = "Unable to Add Bill pay Transaction...";
    const UPDATE_FAILED: &str = "Unable to Update Account...";

    /// A synthetic card number, none of the fixture's.
    const CARD: [u8; 16] = *b"9999000011112222";

    fn shown(failure: &BillPayFailure) -> Shown {
        (
            failure.message(),
            failure.field(),
            failure.class().http_status(),
        )
    }

    /// An account like `acctdata.txt` line 1 with balance `cents` / 100.
    fn account(cents: i64) -> Result<AccountRecord, Box<dyn std::error::Error>> {
        Ok(AccountRecord {
            id: PicU::new(1)?,
            active_status: PicX::from_array(*b"Y"),
            curr_bal: Money::new(Decimal::new(cents, 2))?,
            credit_limit: Money::new(Decimal::new(2_020_000, 2))?,
            cash_credit_limit: Money::new(Decimal::new(1_020_000, 2))?,
            open_date: PicDate::new(NaiveDate::from_ymd_opt(2014, 11, 20))?,
            expiraion_date: PicDate::new(NaiveDate::from_ymd_opt(2025, 5, 20))?,
            reissue_date: PicDate::new(NaiveDate::from_ymd_opt(2025, 5, 20))?,
            curr_cyc_credit: Money::ZERO,
            curr_cyc_debit: Money::new(Decimal::new(-2_500, 2))?,
            addr_zip: PicX::from_array(*b"A000000000"),
            group_id: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    fn xref() -> Result<CardXrefRecord, Box<dyn std::error::Error>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(CARD)),
            cust_id: PicU::new(7)?,
            acct_id: PicU::new(1)?,
            filler: PicX::spaces(),
        })
    }

    fn clock() -> Result<FixedClock, Box<dyn std::error::Error>> {
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_milli_opt(14, 5, 9, 321))
            .ok_or("valid instant")?;
        Ok(FixedClock::new(at))
    }

    fn new_id() -> PicX<16> {
        PicX::from_array(*b"0000000000000301")
    }

    #[test]
    fn confirm_reads_the_first_character_of_the_field() {
        // audit: COBIL00C-009, COBIL00C-010, COBIL00C-011, COBIL00C-012
        let cases = [
            ("Y", Confirm::Yes),
            ("y", Confirm::Yes),
            ("N", Confirm::No),
            ("n", Confirm::No),
            ("", Confirm::Blank),
            (" ", Confirm::Blank),
            ("\0", Confirm::Blank),
            ("X", Confirm::Invalid),
            ("1", Confirm::Invalid),
            ("\u{e9}", Confirm::Invalid),
            ("YN", Confirm::Yes),
        ];
        for (raw, expected) in cases {
            assert_eq!(confirm(raw), expected, "confirm {raw:?}");
        }
    }

    #[test]
    fn edit_account_reads_eleven_digits_by_value() -> TestResult {
        // audit: COBIL00C-007
        for raw in ["", " ", "           ", "\0\0\0\0\0\0\0\0\0\0\0"] {
            let failure = edit_account(raw).err().ok_or("blank refused")?;
            assert_eq!(
                shown(&failure),
                (EMPTY, BillPayField::AcctId, 400),
                "{raw:?}"
            );
        }
        // audit: COBIL00C-008
        let keys = [
            ("00000000001", AccountKey::Id(PicU::new(1)?)),
            ("99999999999", AccountKey::Id(PicU::new(99_999_999_999)?)),
            ("1", AccountKey::Unmatched),
            ("A0000000001", AccountKey::Unmatched),
            (" 0000000001", AccountKey::Unmatched),
            ("0000000001", AccountKey::Unmatched),
            ("0000000001X", AccountKey::Unmatched),
            ("0000000000\u{e9}", AccountKey::Unmatched),
            ("\0\0\0", AccountKey::Unmatched),
        ];
        for (raw, expected) in keys {
            assert_eq!(edit_account(raw)?, expected, "{raw:?}");
        }
        Ok(())
    }

    #[test]
    fn edit_follows_the_program_order() -> TestResult {
        let pay = |intent| -> Result<Result<EnterAction, Shown>, Box<dyn std::error::Error>> {
            Ok(Ok(EnterAction::ReadAccount {
                id: PicU::new(1)?,
                intent,
            }))
        };
        let cases = [
            // audit: COBIL00C-007
            ("", "N", Err((EMPTY, BillPayField::AcctId, 400))),
            ("", "X", Err((EMPTY, BillPayField::AcctId, 400))),
            // audit: COBIL00C-010
            ("abc", "N", Ok(EnterAction::Clear)),
            ("00000000001", "n", Ok(EnterAction::Clear)),
            // audit: COBIL00C-012
            ("abc", "X", Err((INVALID, BillPayField::Confirm, 400))),
            (
                "00000000001",
                "?",
                Err((INVALID, BillPayField::Confirm, 400)),
            ),
            // audit: COBIL00C-008, COBIL00C-014
            ("abc", "Y", Err((NOT_FOUND, BillPayField::AcctId, 404))),
            (
                "0000000001",
                "",
                Err((NOT_FOUND, BillPayField::AcctId, 404)),
            ),
            // audit: COBIL00C-009
            ("00000000001", "Y", pay(ReadIntent::Pay)?),
            ("00000000001", "y", pay(ReadIntent::Pay)?),
            // audit: COBIL00C-011
            ("00000000001", "", pay(ReadIntent::ShowBalance)?),
            ("00000000001", " ", pay(ReadIntent::ShowBalance)?),
        ];
        for (acct_id, confirm, expected) in cases {
            let got = edit(&BillPayInput { acct_id, confirm }).map_err(|failure| shown(&failure));
            assert_eq!(got, expected, "acct {acct_id:?}, confirm {confirm:?}");
        }
        Ok(())
    }

    #[test]
    fn failures_carry_the_audited_literals_fields_and_statuses() {
        let cases = [
            // audit: COBIL00C-014, COBIL00C-032
            (account_not_found(), (NOT_FOUND, BillPayField::AcctId, 404)),
            // audit: COBIL00C-015
            (account_lookup_failed(), (LOOKUP, BillPayField::AcctId, 500)),
            // audit: COBIL00C-024 (BLOCKED, HALTED(X-COBIL00C-H3): the replacement's answer)
            (
                transaction_lookup_failed(),
                (TRAN_LOOKUP, BillPayField::AcctId, 500),
            ),
        ];
        for (failure, expected) in cases {
            assert_eq!(shown(&failure), expected);
            assert_eq!(failure.to_string(), expected.0);
        }
        assert_eq!(FailureClass::Input.http_status(), 400);
        assert_eq!(FailureClass::NotFound.http_status(), 404);
        assert_eq!(FailureClass::Store.http_status(), 500);
    }

    #[test]
    fn decide_refuses_a_balance_of_zero_or_less() -> TestResult {
        // audit: COBIL00C-018, ARITH-018
        for cents in [0, -2_500, -1] {
            for intent in [ReadIntent::Pay, ReadIntent::ShowBalance] {
                let failure = decide(intent, &account(cents)?).err().ok_or("refused")?;
                assert_eq!(
                    shown(&failure),
                    (NOTHING, BillPayField::AcctId, 400),
                    "{cents}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn decide_shows_a_positive_balance_or_orders_the_payment() -> TestResult {
        // audit: COBIL00C-017, COBIL00C-019
        let BillPayDecision::ShowBalance(shown_balance) =
            decide(ReadIntent::ShowBalance, &account(84_300)?)?
        else {
            return Err("a blank confirmation shows the balance".into());
        };
        assert_eq!(
            shown_balance,
            BillPayOutcome::BalanceShown {
                acct_id: PicU::new(1)?,
                balance: "+0000000843.00".to_owned(),
            }
        );
        assert_eq!(shown_balance.message().as_deref(), Some(PROMPT));
        assert_eq!(shown_balance.balance(), Some("+0000000843.00"));
        assert_eq!(shown_balance.acct_id(), Some(PicU::new(1)?));
        assert_eq!(shown_balance.tran_id(), None);
        assert_eq!(shown_balance.field(), BillPayField::Confirm);
        // audit: COBIL00C-009
        assert_eq!(decide(ReadIntent::Pay, &account(1)?)?, BillPayDecision::Pay);
        Ok(())
    }

    #[test]
    fn balance_display_edits_plus_9_10_dot_99() -> TestResult {
        // audit: ARITH-019, COBIL00C-017
        let cases = [
            (84_300, "+0000000843.00"),
            (0, "+0000000000.00"),
            (-2_500, "-0000000025.00"),
            (123_456_789_012, "+1234567890.12"),
        ];
        for (cents, expected) in cases {
            let text = balance_display(Money::new(Decimal::new(cents, 2))?);
            assert_eq!(text, expected);
            assert_eq!(text.len(), 14);
        }
        Ok(())
    }

    #[test]
    fn payment_amount_and_new_balance_follow_the_store_rule() -> TestResult {
        // (balance, TRAN-AMT, new balance), in cents.
        let cases = [
            // audit: ARITH-020, ARITH-022, COBIL00C-026, COBIL00C-031
            (84_300, 84_300, 0),
            // audit: ARITH-021, ARITH-022, COBIL00C-026, COBIL00C-031
            (123_456_789_012, 23_456_789_012, 100_000_000_000),
            (999_999_999_999, 99_999_999_999, 900_000_000_000),
            (100_000_000_000, 0, 100_000_000_000),
        ];
        let (xref, clock) = (xref()?, clock()?);
        for (balance, amount, after) in cases {
            let read = account(balance)?;
            let effects = payment(new_id(), None, &read, Some(&xref), &clock);
            assert_eq!(
                effects.transaction.amt,
                Money::<9>::new(Decimal::new(amount, 2))?
            );
            assert_eq!(
                effects.account.curr_bal,
                Money::<10>::new(Decimal::new(after, 2))?
            );
            // Every other column keeps the value read (COBIL00C-031).
            let unchanged = AccountRecord {
                curr_bal: read.curr_bal,
                ..effects.account.clone()
            };
            assert_eq!(unchanged, read, "balance {balance}");
        }
        Ok(())
    }

    /// Whether `text` is laid out as `YYYY-MM-DD HH:MM:SS.000000`, layout `I` with zero micros.
    fn is_iso_zero_micros(text: &[u8]) -> bool {
        let layout = b"9999-99-99 99:99:99.000000";
        text.len() == layout.len()
            && text.iter().zip(layout).all(|(&byte, &shape)| match shape {
                b'9' => byte.is_ascii_digit(),
                _ => byte == shape,
            })
    }

    #[test]
    fn payment_record_holds_the_audited_fields() -> TestResult {
        // audit: COBIL00C-020, COBIL00C-025, COBIL00C-027
        let (xref, clock) = (xref()?, clock()?);
        let effects = payment(new_id(), None, &account(84_300)?, Some(&xref), &clock);
        let record = &effects.transaction;
        assert_eq!(record.id, new_id());
        assert!(
            record.card_num.expose().as_bytes() == &CARD,
            "card from the xref"
        );
        let bytes = record.encode();
        let expected: [(usize, usize, &[u8]); 9] = [
            (16, 18, b"02"),
            (18, 22, b"0002"),
            (22, 32, b"POS TERM  "),
            (32, 53, b"BILL PAYMENT - ONLINE"),
            (143, 152, b"999999999"),
            (152, 164, b"BILL PAYMENT"),
            (202, 205, b"N/A"),
            (252, 262, b"N/A       "),
            (330, 350, b"                    "),
        ];
        for (start, end, value) in expected {
            assert_eq!(bytes.get(start..end), Some(value), "bytes {start}..{end}");
        }
        let padding = [(53, 132), (164, 202), (205, 252)];
        for (start, end) in padding {
            let field = bytes.get(start..end).ok_or("in record")?;
            assert!(
                field.iter().all(|&byte| byte == b' '),
                "spaces {start}..{end}"
            );
        }
        // Time fields are compared by format only (AAP 0.5.6).
        for (start, end) in [(278, 304), (304, 330)] {
            let text = bytes.get(start..end).ok_or("in record")?;
            assert!(is_iso_zero_micros(text), "timestamp {start}..{end}");
        }
        assert_eq!(record.orig_ts, record.proc_ts);
        let stamp = record.orig_ts.value().ok_or("not blank")?;
        assert_eq!(stamp.form(), TsForm::Iso);
        Ok(())
    }

    #[test]
    fn payment_keeps_the_filler_of_the_highest_transaction() -> TestResult {
        // audit: COBIL00C-025
        let (xref, clock) = (xref()?, clock()?);
        let read = account(84_300)?;
        let mut prior: TransactionRecord =
            payment(new_id(), None, &read, Some(&xref), &clock).transaction;
        prior.filler = PicX::from_array(*b"ABCDEFGHIJKLMNOPQRST");
        let effects = payment(new_id(), Some(&prior), &read, Some(&xref), &clock);
        assert_eq!(effects.transaction.filler, prior.filler);
        Ok(())
    }

    #[test]
    fn payment_without_cross_reference_or_prior_writes_spaces() -> TestResult {
        // BLOCKED: HALTED(X-COBIL00C-H2) and HALTED(X-COBIL00C-H3). Only the replacement's
        // stand-ins are asserted, no parity value.
        let effects = payment(new_id(), None, &account(84_300)?, None, &clock()?);
        assert!(
            effects.transaction.card_num.expose().is_spaces(),
            "16 spaces"
        );
        assert!(effects.transaction.filler.is_spaces(), "20 spaces");
        Ok(())
    }

    #[test]
    fn success_message_is_the_string_of_write_transact_file() {
        // audit: COBIL00C-028
        let message = success_message(&new_id());
        assert_eq!(
            message,
            "Payment successful.  Your Transaction ID is 0000000000000301."
        );
        assert_eq!(message.len(), 61);
        let spaced = PicX::from_array(*b"12345678 9012345");
        assert_eq!(
            success_message(&spaced),
            "Payment successful.  Your Transaction ID is 12345678."
        );
    }

    #[test]
    fn payment_outcome_lets_the_last_message_win() {
        let paid = Ok(BillPayOutcome::Paid { tran_id: new_id() });
        let cases = [
            // audit: COBIL00C-028, COBIL00C-031, COBIL00C-034
            (TranWrite::Written, AccountRewrite::Rewritten, paid),
            // audit: COBIL00C-029, COBIL00C-031
            (
                TranWrite::Duplicate,
                AccountRewrite::Rewritten,
                Err((EXISTS, BillPayField::AcctId, 500)),
            ),
            // audit: COBIL00C-030, COBIL00C-031
            (
                TranWrite::Failed,
                AccountRewrite::Rewritten,
                Err((ADD_FAILED, BillPayField::AcctId, 500)),
            ),
            // audit: COBIL00C-032
            (
                TranWrite::Written,
                AccountRewrite::NotFound,
                Err((NOT_FOUND, BillPayField::AcctId, 404)),
            ),
            (
                TranWrite::Duplicate,
                AccountRewrite::NotFound,
                Err((NOT_FOUND, BillPayField::AcctId, 404)),
            ),
            // audit: COBIL00C-033
            (
                TranWrite::Written,
                AccountRewrite::Failed,
                Err((UPDATE_FAILED, BillPayField::AcctId, 500)),
            ),
            (
                TranWrite::Failed,
                AccountRewrite::Failed,
                Err((UPDATE_FAILED, BillPayField::AcctId, 500)),
            ),
        ];
        for (write, rewrite, expected) in cases {
            let got = payment_outcome(&new_id(), write, rewrite).map_err(|failure| shown(&failure));
            assert_eq!(got, expected, "{write:?}, {rewrite:?}");
        }
    }

    #[test]
    fn cleared_and_paid_outcomes_empty_the_screen() {
        // audit: COBIL00C-010
        let cleared = BillPayOutcome::Cleared;
        assert_eq!(cleared.message(), None);
        assert_eq!(cleared.balance(), None);
        assert_eq!(cleared.acct_id(), None);
        assert_eq!(cleared.tran_id(), None);
        assert_eq!(cleared.field(), BillPayField::AcctId);
        // audit: COBIL00C-028
        let paid = BillPayOutcome::Paid { tran_id: new_id() };
        assert_eq!(paid.message(), Some(success_message(&new_id())));
        assert_eq!(paid.balance(), None);
        assert_eq!(paid.acct_id(), None);
        assert_eq!(paid.tran_id(), Some(&new_id()));
        assert_eq!(paid.field(), BillPayField::AcctId);
    }
}
