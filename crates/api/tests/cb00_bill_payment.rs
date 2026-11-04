//! CB00 bill payment (`COBIL00C`, map `COBIL00`) through `POST /payments`:
//! the entry edits and refusals, the confirmation prompt, the confirmed
//! payment and its arithmetic, the write persistence of E-19, and the 3270
//! mechanics the one POST replaces: the AID keys, the received map, the
//! session, the screen header, the edited balance and the emptied form.
//!
//! Each test runs on its own seeded database ([`TestApp`]): the 50 fixture
//! accounts and an empty live transaction master (E-07). Every refusal, fault
//! and prompt answers as one primary `V-COBIL00C` row of
//! `docs/parity/validation-audit.md`, read once its review passed: the status
//! and the exact text come from the row, and for a refusal also the error
//! code of the row's `kind=` (from a wire-code table of this file, not the
//! domain's error kinds) and the BMS field; the `carddemo_domain::messages`
//! constant a case names is asserted equal to that text. A `no-message` row
//! answers with no text, or, for the non-digit key of E-47, with the text of
//! the account `NOTFND` row its proposal names. The reviewed rows of
//! `docs/parity/parity-oracle-audit.md` give the success text, joined from the
//! quoted pieces of P-COBIL00C-016; the literals of the payment record,
//! P-COBIL00C-015; and which write persists when the other fails,
//! P-COBIL00C-016 to P-COBIL00C-019 and P-COBIL00C-024 (E-19). Every case
//! names the audit rows it proves in a string literal, which the coverage
//! gates read.
//!
//! A seeded balance or card is never copied into a case: it is decoded from
//! `acctdata.txt` and `cardxref.txt` at the reviewed section A offsets
//! ([`fixture_records`]), and the amount and balance a payment of it gives
//! follow from the audited `MOVE` and `COMPUTE` (P-COBIL00C-021,
//! P-COBIL00C-022). Balances a setup statement gives an account stay literal.
//!
//! Failures no request can provoke are injected with SQL through the stack:
//! triggers that fail or skip one of the two writes (E-19), and renames that
//! make one of the three reads fail. Each fault is removed before the next
//! case, and the fault tests run on stacks of their own.
//!
//! Docker must be running; the harness conventions are those of `common`.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use carddemo_audit::Document;
use carddemo_domain::format::{PIC_BILL_CURR_BAL, edit};
use carddemo_domain::messages;
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};

use common::{Resp, TestApp, assert_error, fixture_records};

/// The processing clock, the source of the payment timestamps.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// `TRAN-ORIG-TS` and `TRAN-PROC-TS` of a payment made at [`CLOCK`]: the
/// `ASKTIME`/`FORMATTIME` text `YYYY-MM-DD HH:MM:SS.000000` of
/// `GET-CURRENT-TIMESTAMP`.
const PAYMENT_TS: &str = "2022-07-01 12:00:00.000000";

/// The route under test.
const PAYMENTS: &str = "/payments";

/// The program whose validation rows this file proves.
const PROGRAM: &str = "COBIL00C";

/// The text of the cancelled payment: `CLEAR-CURRENT-SCREEN` sets none, and
/// the audit records the path as `no-message (E-49)`.
const NO_MESSAGE: &str = "";

/// The escalation of a `9(11)` key moved from an entry holding a non-digit,
/// whose halt proposal answers as the account `NOTFND` row.
const NON_DIGIT_KEY: &str = "E-47";

/// What the trigger of a row under [`NON_DIGIT_KEY`] records of its proposal.
const NOTFND_PROPOSAL: &str = "answers as the account NOTFND row";

/// The account `NOTFND` row of `READ-ACCTDAT-FILE`, whose answer the
/// [`NON_DIGIT_KEY`] proposal gives.
const ACCOUNT_NOTFND_ROW: &str = "V-COBIL00C-004";

/// What the trigger of [`ACCOUNT_NOTFND_ROW`] records of its read.
const ACCOUNT_NOTFND: &str = "no account master record (RESP NOTFND)";

/// The TRAN-ID of [`LIVE_ROW`], the highest live key before any payment.
const LIVE_TRAN_ID: &str = "0000000000000300";

/// The one live row the allocations count from, TRAN-ID `0000000000000300`,
/// so payments are allocated `…301` onwards. The seeded live master is empty
/// (E-07), and a payment against an empty master allocates nothing (E-27).
const LIVE_ROW: &str = "INSERT INTO transaction_master \
     (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, \
      tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip, \
      tran_card_num, tran_orig_ts, tran_proc_ts) \
     VALUES ('0000000000000300', '01', 1, 'POS TERM', 'CB00 base row', 10.00, \
      1, 'Base merchant', 'Base city', '12345', \
      '9680294154603697', '2022-06-01 00:00:00.000000', '2022-06-01 00:00:00.000000')";

/// Every account balance, in key order.
const BALANCES: &str =
    "SELECT account_id::text, acct_curr_bal::text FROM accounts ORDER BY account_id";

/// Every account column but the balance, in key order: what a payment's
/// `REWRITE` of the account record leaves as it was read.
const ACCOUNT_REST: &str = "SELECT account_id::text, acct_active_status, \
     acct_credit_limit::text, acct_cash_credit_limit::text, acct_open_date, \
     acct_expiraion_date, acct_reissue_date, acct_curr_cyc_credit::text, \
     acct_curr_cyc_debit::text, acct_addr_zip, acct_group_id \
     FROM accounts ORDER BY account_id";

/// The rows of the transaction table, live or not.
const TRANSACTION_COUNT: &str = "SELECT count(*)::text FROM transactions";

/// The live rows after the [`LIVE_ROW`] key, with every column a payment
/// writes.
const ADDED_ROWS: &str = "SELECT tran_id, tran_type_cd, tran_cat_cd::text, tran_source, \
     tran_desc, tran_amt::text, tran_merchant_id::text, tran_merchant_name, \
     tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts, tran_proc_ts \
     FROM transaction_master WHERE tran_id > '0000000000000300' ORDER BY tran_id";

/// The reward accruals: none, since the reward engine serves CT02 and
/// `POSTTRAN` only.
const ACCRUAL_COUNT: &str = "SELECT count(*)::text FROM reward_accruals";

/// The accounts on the edge of the balance guard: account 4 at zero, account
/// 5 negative and account 7 at one cent; and account 6 without its
/// cross-reference, the input class of E-29.
const REFUSAL_SETUP: &str = "UPDATE accounts SET acct_curr_bal = 0.00 WHERE account_id = 4; \
     UPDATE accounts SET acct_curr_bal = -5.00 WHERE account_id = 5; \
     UPDATE accounts SET acct_curr_bal = 0.01 WHERE account_id = 7; \
     DELETE FROM card_xref WHERE xref_acct_id = 6";

/// Account 3 with a balance whose integer part exceeds the nine digits of
/// `TRAN-AMT S9(09)V99`.
const OVERSIZED_BALANCE: &str =
    "UPDATE accounts SET acct_curr_bal = 1234567890.12 WHERE account_id = 3";

/// The trigger functions of the write faults: a duplicate key
/// (`unique_violation`, the `DUPKEY`/`DUPREC` of the `WRITE`), any other
/// failure, and a row the `UPDATE` skips (the `NOTFND` of the `REWRITE`).
const FAULT_FUNCTIONS: &str = "CREATE FUNCTION cb00_duplicate_key() RETURNS trigger \
     LANGUAGE plpgsql AS $$ BEGIN \
     RAISE EXCEPTION 'cb00 test: TRAN-ID taken' USING ERRCODE = 'unique_violation'; \
     END $$; \
     CREATE FUNCTION cb00_write_failure() RETURNS trigger \
     LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'cb00 test: write failed'; END $$; \
     CREATE FUNCTION cb00_row_skipped() RETURNS trigger \
     LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$";

/// The payment transaction's `WRITE` (write 1 of 2) answers a duplicate key.
const INSERT_DUPLICATE: &str = "CREATE TRIGGER cb00_insert_fault BEFORE INSERT ON transactions \
     FOR EACH ROW EXECUTE FUNCTION cb00_duplicate_key()";

/// The payment transaction's `WRITE` fails otherwise.
const INSERT_FAILURE: &str = "CREATE TRIGGER cb00_insert_fault BEFORE INSERT ON transactions \
     FOR EACH ROW EXECUTE FUNCTION cb00_write_failure()";

/// The account `REWRITE` (write 2 of 2) fails.
const REWRITE_FAILURE: &str = "CREATE TRIGGER cb00_rewrite_fault BEFORE UPDATE ON accounts \
     FOR EACH ROW EXECUTE FUNCTION cb00_write_failure()";

/// The account `REWRITE` finds no record: the trigger skips the row, so the
/// `UPDATE` changes none.
const REWRITE_NOT_FOUND: &str = "CREATE TRIGGER cb00_rewrite_fault BEFORE UPDATE ON accounts \
     FOR EACH ROW EXECUTE FUNCTION cb00_row_skipped()";

/// Removes the fault of [`INSERT_DUPLICATE`] or [`INSERT_FAILURE`].
const DROP_INSERT_FAULT: &str = "DROP TRIGGER cb00_insert_fault ON transactions";

/// Removes the fault of [`REWRITE_FAILURE`] or [`REWRITE_NOT_FOUND`].
const DROP_REWRITE_FAULT: &str = "DROP TRIGGER cb00_rewrite_fault ON accounts";

/// A request the program refuses with an error body, its one `fields` entry
/// carrying the top-level message.
struct Refusal {
    /// The primary validation row, whose status, error code, BMS field and
    /// text the answer carries.
    row: &'static str,
    /// The audit rows the case proves.
    rows: &'static str,
    /// `account_id` of the body; `None` leaves the field out.
    account_id: Option<&'static str>,
    /// `confirm` of the body; `None` leaves the field out.
    confirm: Option<&'static str>,
    /// The messages constant of the row's text, [`NO_MESSAGE`] for a path
    /// that sets none.
    message: &'static str,
}

/// The entry edits and the refusals of `PROCESS-ENTER-KEY` before any write,
/// in the COBOL order [app/cbl/COBIL00C.cbl:L154-219], after
/// [`REFUSAL_SETUP`]. None of them writes.
const REFUSALS: [Refusal; 19] = [
    Refusal {
        row: "V-COBIL00C-001",
        rows: "V-COBIL00C-001, P-COBIL00C-004",
        account_id: Some(""),
        confirm: Some("Y"),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
    },
    Refusal {
        row: "V-COBIL00C-001",
        rows: "V-COBIL00C-001, P-COBIL00C-004: a missing field is a blank one",
        account_id: None,
        confirm: Some(""),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
    },
    Refusal {
        row: "V-COBIL00C-001",
        rows: "V-COBIL00C-001, P-COBIL00C-004: LOW-VALUES are blank",
        account_id: Some("\u{0}\u{0}\u{0}"),
        confirm: Some("Y"),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
    },
    Refusal {
        row: "V-COBIL00C-001",
        rows: "V-COBIL00C-001: the account edit (order 1) precedes the confirmation edit",
        account_id: Some("   "),
        confirm: Some("X"),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
    },
    Refusal {
        row: "V-COBIL00C-002",
        rows: "V-COBIL00C-002, P-COBIL00C-006",
        account_id: Some("00000000001"),
        confirm: Some("N"),
        message: NO_MESSAGE,
    },
    Refusal {
        row: "V-COBIL00C-002",
        rows: "V-COBIL00C-002, P-COBIL00C-006: lower-case n",
        account_id: Some("00000000001"),
        confirm: Some("n"),
        message: NO_MESSAGE,
    },
    Refusal {
        row: "V-COBIL00C-002",
        rows: "V-COBIL00C-002, P-COBIL00C-006: N reads no account",
        account_id: Some("99999999999"),
        confirm: Some("N"),
        message: NO_MESSAGE,
    },
    Refusal {
        row: "V-COBIL00C-003",
        rows: "V-COBIL00C-003, P-COBIL00C-008",
        account_id: Some("00000000001"),
        confirm: Some("X"),
        message: messages::INVALID_VALUE_VALID_VALUES_ARE_Y_N,
    },
    Refusal {
        row: "V-COBIL00C-003",
        rows: "V-COBIL00C-003, P-COBIL00C-008: an invalid confirmation reads no account",
        account_id: Some("99999999999"),
        confirm: Some("?"),
        message: messages::INVALID_VALUE_VALID_VALUES_ARE_Y_N,
    },
    Refusal {
        row: "V-COBIL00C-004",
        rows: "V-COBIL00C-004, P-COBIL00C-009",
        account_id: Some("99999999999"),
        confirm: Some(""),
        message: messages::ACCOUNT_ID_NOT_FOUND,
    },
    Refusal {
        row: "V-COBIL00C-004",
        rows: "V-COBIL00C-004, P-COBIL00C-005, P-COBIL00C-009: confirmed",
        account_id: Some("99999999999"),
        confirm: Some("Y"),
        message: messages::ACCOUNT_ID_NOT_FOUND,
    },
    // A non-digit leaves the `9(11)` key unpinned (E-47, halt); the answer is
    // the escalation's proposal, the account `NOTFND` row without a read.
    Refusal {
        row: "V-COBIL00C-006",
        rows: "V-COBIL00C-006",
        account_id: Some("0000000000A"),
        confirm: Some(""),
        message: messages::ACCOUNT_ID_NOT_FOUND,
    },
    Refusal {
        row: "V-COBIL00C-007",
        rows: "V-COBIL00C-007, P-COBIL00C-010, P-COBIL00C-020: zero balance, blank confirm",
        account_id: Some("00000000004"),
        confirm: Some(""),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
    },
    Refusal {
        row: "V-COBIL00C-007",
        rows: "V-COBIL00C-007, P-COBIL00C-010, P-COBIL00C-020: zero balance, confirmed",
        account_id: Some("00000000004"),
        confirm: Some("Y"),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
    },
    Refusal {
        row: "V-COBIL00C-007",
        rows: "V-COBIL00C-007, P-COBIL00C-020: a negative balance compares algebraically",
        account_id: Some("00000000005"),
        confirm: Some(""),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
    },
    Refusal {
        row: "V-COBIL00C-007",
        rows: "V-COBIL00C-007, P-COBIL00C-020: negative balance, confirmed",
        account_id: Some("00000000005"),
        confirm: Some("Y"),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
    },
    // The COBOL goes on to write the payment with the card number left in
    // `CARD-XREF-RECORD` (E-29, halt); the answer is the escalation's
    // proposal: the `NOTFND` message and no write (P-COBIL00C-025).
    Refusal {
        row: "V-COBIL00C-009",
        rows: "V-COBIL00C-009, P-COBIL00C-011, P-COBIL00C-025",
        account_id: Some("00000000006"),
        confirm: Some("Y"),
        message: messages::ACCOUNT_ID_NOT_FOUND,
    },
    // The seeded live master is empty: no TRAN-ID can be allocated (E-27).
    Refusal {
        row: "V-COBIL00C-011",
        rows: "V-COBIL00C-011, P-COBIL00C-005, P-COBIL00C-013",
        account_id: Some("00000000001"),
        confirm: Some("Y"),
        message: messages::UNABLE_TO_LOOKUP_TRANSACTION,
    },
    Refusal {
        row: "V-COBIL00C-011",
        rows: "V-COBIL00C-011, P-COBIL00C-005, P-COBIL00C-013: lower-case y confirms",
        account_id: Some("00000000001"),
        confirm: Some("y"),
        message: messages::UNABLE_TO_LOOKUP_TRANSACTION,
    },
];

/// The `ACCT-CURR-BAL` an account holds when a case reads it.
#[derive(Debug, Clone, Copy)]
enum Balance {
    /// The balance of its `acctdata.txt` record, as seeded.
    Seeded,
    /// The decimal string a setup statement gives it.
    Set(&'static str),
}

impl Balance {
    /// The balance of the account `account_id` as the response's decimal
    /// string, a [`Balance::Seeded`] one decoded through `parity`.
    fn of(self, parity: &Document, account_id: &str) -> String {
        match self {
            Self::Seeded => seeded_balance(parity, account_id),
            Self::Set(balance) => balance.to_owned(),
        }
    }
}

/// A blank confirmation on an account with a positive balance: the status
/// and text of its message-only row, with the balance, no TRAN-ID and no
/// write.
struct Prompt {
    /// The primary validation row, whose status and text the answer carries.
    row: &'static str,
    /// The audit rows the case proves.
    rows: &'static str,
    /// `account_id` of the body.
    account_id: &'static str,
    /// `confirm` of the body; `None` leaves the field out.
    confirm: Option<&'static str>,
    /// `ACCT-CURR-BAL`, which the response carries as its decimal string.
    balance: Balance,
}

/// The prompts, after [`REFUSAL_SETUP`], which sets account 7's balance and
/// leaves the seeded balances of accounts 1 and 6.
const PROMPTS: [Prompt; 5] = [
    Prompt {
        row: "V-COBIL00C-008",
        rows: "V-COBIL00C-008, P-COBIL00C-007",
        account_id: "00000000001",
        confirm: Some(""),
        balance: Balance::Seeded,
    },
    Prompt {
        row: "V-COBIL00C-008",
        rows: "V-COBIL00C-008, P-COBIL00C-007: a missing confirmation is blank",
        account_id: "00000000001",
        confirm: None,
        balance: Balance::Seeded,
    },
    Prompt {
        row: "V-COBIL00C-008",
        rows: "V-COBIL00C-008, P-COBIL00C-007: LOW-VALUES are blank",
        account_id: "00000000001",
        confirm: Some("\u{0}"),
        balance: Balance::Seeded,
    },
    Prompt {
        row: "V-COBIL00C-008",
        rows: "V-COBIL00C-008, P-COBIL00C-020: one cent is more than zero",
        account_id: "00000000007",
        confirm: Some(" "),
        balance: Balance::Set("0.01"),
    },
    Prompt {
        row: "V-COBIL00C-008",
        rows: "V-COBIL00C-008, P-COBIL00C-011: only a confirmed payment reads the cross-reference",
        account_id: "00000000006",
        confirm: Some(""),
        balance: Balance::Seeded,
    },
];

/// A confirmed payment with one or both of its writes made to fail.
struct WriteFault {
    /// The primary validation row: that of the last write that fails, whose
    /// message replaces any earlier one (P-COBIL00C-019).
    row: &'static str,
    /// The audit rows the case proves.
    rows: &'static str,
    /// The account paid; each case pays another, so no earlier case masks it.
    account_id: &'static str,
    /// The statements that install the fault.
    faults: &'static [&'static str],
    /// The statements that remove it.
    repairs: &'static [&'static str],
    /// The messages constant of the row's text, the last one the program set.
    message: &'static str,
}

impl WriteFault {
    /// Whether the fault fails write 1 of 2, the transaction `WRITE`.
    fn fails_write(&self) -> bool {
        self.faults
            .iter()
            .any(|fault| [INSERT_DUPLICATE, INSERT_FAILURE].contains(fault))
    }

    /// Whether the fault fails write 2 of 2, the account `REWRITE`.
    fn fails_rewrite(&self) -> bool {
        self.faults
            .iter()
            .any(|fault| [REWRITE_FAILURE, REWRITE_NOT_FOUND].contains(fault))
    }
}

/// The `acct_curr_bal` of a paid account after its payment's writes.
#[derive(Debug, Clone, Copy)]
enum After {
    /// The `REWRITE` succeeded: [`Payment::after`].
    Paid,
    /// The `REWRITE` failed: [`Payment::balance`], as seeded.
    Unchanged,
}

impl After {
    /// The balance `payment` leaves.
    fn of(self, payment: &Payment) -> &str {
        match self {
            Self::Paid => &payment.after,
            Self::Unchanged => &payment.balance,
        }
    }
}

/// The E-19 cases in run order, after [`LIVE_ROW`], each paying a seeded
/// balance ([`Payment::of`]). What each leaves follows from the reviewed E-19
/// rows ([`e19_outcome`]): a failed `WRITE` still reduces and rewrites the
/// balance; a failed `REWRITE` keeps the written transaction; both files are
/// `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L1-96]. A TRAN-ID whose insert
/// failed is allocated again by the next case.
const WRITE_FAULTS: [WriteFault; 5] = [
    WriteFault {
        row: "V-COBIL00C-014",
        rows: "V-COBIL00C-014, P-COBIL00C-017, P-COBIL00C-018, P-COBIL00C-024",
        account_id: "00000000002",
        faults: &[INSERT_DUPLICATE],
        repairs: &[DROP_INSERT_FAULT],
        message: messages::TRAN_ID_ALREADY_EXIST,
    },
    WriteFault {
        row: "V-COBIL00C-015",
        rows: "V-COBIL00C-015, P-COBIL00C-017, P-COBIL00C-018, P-COBIL00C-024",
        account_id: "00000000005",
        faults: &[INSERT_FAILURE],
        repairs: &[DROP_INSERT_FAULT],
        message: messages::UNABLE_TO_ADD_BILL_PAY_TRANSACTION,
    },
    WriteFault {
        row: "V-COBIL00C-017",
        rows: "V-COBIL00C-017, P-COBIL00C-016, P-COBIL00C-019, P-COBIL00C-024",
        account_id: "00000000008",
        faults: &[REWRITE_FAILURE],
        repairs: &[DROP_REWRITE_FAULT],
        message: messages::UNABLE_TO_UPDATE_ACCOUNT,
    },
    WriteFault {
        row: "V-COBIL00C-016",
        rows: "V-COBIL00C-016, P-COBIL00C-016, P-COBIL00C-019, P-COBIL00C-024",
        account_id: "00000000009",
        faults: &[REWRITE_NOT_FOUND],
        repairs: &[DROP_REWRITE_FAULT],
        message: messages::ACCOUNT_ID_NOT_FOUND,
    },
    // `SEND-BILLPAY-SCREEN` does not end the task, so the rewrite's message
    // replaces the write's, and neither write persists.
    WriteFault {
        row: "V-COBIL00C-017",
        rows: "V-COBIL00C-017, P-COBIL00C-003, P-COBIL00C-019: both writes fail",
        account_id: "00000000010",
        faults: &[INSERT_FAILURE, REWRITE_FAILURE],
        repairs: &[DROP_INSERT_FAULT, DROP_REWRITE_FAULT],
        message: messages::UNABLE_TO_UPDATE_ACCOUNT,
    },
];

/// A confirmed payment on account 1 with one of its reads made to fail by
/// renaming the relation it reads.
struct ReadFault {
    /// The primary validation row, whose status, error code, BMS field and
    /// text the answer carries.
    row: &'static str,
    /// The audit rows the case proves.
    rows: &'static str,
    /// The rename that hides the relation.
    fault: &'static str,
    /// The rename that restores it.
    repair: &'static str,
    /// `confirm` of the body.
    confirm: &'static str,
    /// The messages constant of the row's text.
    message: &'static str,
}

/// The read failures, in the order of the reads.
const READ_FAULTS: [ReadFault; 5] = [
    ReadFault {
        row: "V-COBIL00C-005",
        rows: "V-COBIL00C-005, P-COBIL00C-009",
        fault: "ALTER TABLE accounts RENAME TO accounts_cb00_hidden",
        repair: "ALTER TABLE accounts_cb00_hidden RENAME TO accounts",
        confirm: "Y",
        message: messages::UNABLE_TO_LOOKUP_ACCOUNT,
    },
    ReadFault {
        row: "V-COBIL00C-005",
        rows: "V-COBIL00C-005, P-COBIL00C-007, P-COBIL00C-009: the prompt reads the account too",
        fault: "ALTER TABLE accounts RENAME TO accounts_cb00_hidden",
        repair: "ALTER TABLE accounts_cb00_hidden RENAME TO accounts",
        confirm: "",
        message: messages::UNABLE_TO_LOOKUP_ACCOUNT,
    },
    // The COBOL goes on with the payment after this read too (E-29, halt);
    // the escalation's proposal stops with the message and writes nothing.
    ReadFault {
        row: "V-COBIL00C-010",
        rows: "V-COBIL00C-010, P-COBIL00C-011, P-COBIL00C-025",
        fault: "ALTER TABLE card_xref RENAME TO card_xref_cb00_hidden",
        repair: "ALTER TABLE card_xref_cb00_hidden RENAME TO card_xref",
        confirm: "Y",
        message: messages::UNABLE_TO_LOOKUP_XREF_AIX_FILE,
    },
    // The replacement reads the highest live key in one statement, in place
    // of both the `STARTBR` and the `READPREV` of the browse, so a failure of
    // that read answers as each of their rows; the `READPREV` `ENDFILE` path
    // is unreachable after a successful `STARTBR`.
    ReadFault {
        row: "V-COBIL00C-012",
        rows: "V-COBIL00C-012, P-COBIL00C-014: the one read stands for the STARTBR",
        fault: "ALTER VIEW transaction_master RENAME TO transaction_master_cb00_hidden",
        repair: "ALTER VIEW transaction_master_cb00_hidden RENAME TO transaction_master",
        confirm: "Y",
        message: messages::UNABLE_TO_LOOKUP_TRANSACTION,
    },
    ReadFault {
        row: "V-COBIL00C-013",
        rows: "V-COBIL00C-013, P-COBIL00C-014: and for the READPREV",
        fault: "ALTER VIEW transaction_master RENAME TO transaction_master_cb00_hidden",
        repair: "ALTER VIEW transaction_master_cb00_hidden RENAME TO transaction_master",
        confirm: "Y",
        message: messages::UNABLE_TO_LOOKUP_TRANSACTION,
    },
];

/// What a request may have written: every account balance and the number of
/// transaction rows, live or not.
#[derive(Debug, PartialEq, Eq)]
struct Written {
    /// [`BALANCES`].
    balances: Vec<Vec<Option<String>>>,
    /// [`TRANSACTION_COUNT`].
    transactions: Vec<Vec<Option<String>>>,
}

/// The current [`Written`] state of `app`'s database.
async fn written(app: &TestApp) -> Written {
    Written {
        balances: app.stack().fetch_text_rows(BALANCES).await,
        transactions: app.stack().fetch_text_rows(TRANSACTION_COUNT).await,
    }
}

/// The `POST /payments` body with the fields given; `None` leaves a field
/// out.
fn request(account_id: Option<&str>, confirm: Option<&str>) -> Value {
    let mut body = Map::new();
    if let Some(account_id) = account_id {
        body.insert("account_id".to_owned(), Value::from(account_id));
    }
    if let Some(confirm) = confirm {
        body.insert("confirm".to_owned(), Value::from(confirm));
    }
    Value::Object(body)
}

/// The `code` of the error body for each `kind=` a validation row names: the
/// `ErrorBody::code` values `carddemo_api::error` publishes. The table is
/// written out here, not taken from the domain's error kinds, so the test
/// checks the published wire codes independently of the mapping it tests.
const KIND_CODES: [(&str, &str); 5] = [
    ("Invalid", "INVALID"),
    ("Empty", "EMPTY"),
    ("NotFound", "NOT_FOUND"),
    ("Duplicate", "DUPLICATE"),
    ("Unexpected", "UNEXPECTED"),
];

/// The `kind=` of a message-only row: no error flag, no error body.
const NO_KIND: &str = "none";

/// The error-body `code` of the audit kind `kind`.
fn kind_code(kind: &str) -> &'static str {
    let Some((_, code)) = KIND_CODES.iter().find(|(name, _)| *name == kind) else {
        panic!("no error code is published for the audit kind {kind:?}");
    };
    code
}

/// The HTTP status `text` names in the cell of audit row `id`.
fn parse_status(text: &str, id: &str) -> StatusCode {
    text.parse::<u16>()
        .ok()
        .and_then(|code| StatusCode::from_u16(code).ok())
        .unwrap_or_else(|| panic!("{id} assigns no HTTP status: {text:?}"))
}

/// The trimmed cell `name` of the data row `id` of `doc`, an audit.
fn cell<'d>(doc: &'d Document, id: &str, name: &str) -> &'d str {
    let Some((table, row)) = doc.find_row(id) else {
        panic!("the audit defines no {id} row");
    };
    table
        .cell(row, name)
        .unwrap_or_else(|| panic!("{id} has no `{name}` cell"))
        .trim()
}

/// The answer a validation row gives.
#[derive(Debug)]
struct Audited {
    /// The HTTP status of its `Status` cell.
    status: StatusCode,
    /// The error code of its `kind=`; `None` for [`NO_KIND`].
    code: Option<&'static str>,
    /// Its `BMS field`, the `fields[].field` of an error body.
    field: String,
    /// The exact text the answer carries ([`audited_text`]).
    message: String,
}

/// The answer of the validation row `id` of `validation`, once its review
/// passed and its text is `message`, the messages constant a case names.
fn audited(validation: &Document, id: &str, message: &str) -> Audited {
    assert_reviewed(validation, &[id]);
    let text = audited_text(validation, id);
    assert_eq!(
        text, message,
        "{id}: the messages constant is the audited text"
    );
    let trigger = cell(validation, id, "Trigger condition");
    let Some(kind) = carddemo_audit::key_values(trigger)
        .into_iter()
        .rev()
        .find(|pair| pair.key == "kind")
    else {
        panic!("{id} names no kind: {trigger}");
    };
    Audited {
        status: parse_status(cell(validation, id, "Status"), id),
        code: (kind.value != NO_KIND).then(|| kind_code(&kind.value)),
        field: cell(validation, id, "BMS field").to_owned(),
        message: text,
    }
}

/// The text the answer of the validation row `id` carries: the first code
/// span of its `Message text`, unquoted, which a note may follow. A
/// `no-message` row sets none and answers with no text, except a row under
/// [`NON_DIGIT_KEY`], whose trigger records that its proposal answers as the
/// account `NOTFND` row, [`ACCOUNT_NOTFND_ROW`], and so carries that row's
/// text.
fn audited_text(validation: &Document, id: &str) -> String {
    let text = cell(validation, id, "Message text");
    if !carddemo_audit::is_no_message(text) {
        let Some(span) = carddemo_audit::code_span(text) else {
            panic!("{id} quotes no message text: {text}");
        };
        return unquote(&span.text, id);
    }
    let Some(escalation) = carddemo_audit::no_message_escalation(text) else {
        panic!("{id}: {text:?} links no escalation");
    };
    if escalation != NON_DIGIT_KEY {
        return String::new();
    }
    let trigger = cell(validation, id, "Trigger condition");
    assert!(
        trigger.contains(NOTFND_PROPOSAL),
        "{id}: the {escalation} proposal {NOTFND_PROPOSAL:?}: {trigger}"
    );
    assert_reviewed(validation, &[ACCOUNT_NOTFND_ROW]);
    let notfnd = cell(validation, ACCOUNT_NOTFND_ROW, "Trigger condition");
    assert!(
        notfnd.contains(ACCOUNT_NOTFND),
        "{ACCOUNT_NOTFND_ROW} is the account NOTFND row: {notfnd}"
    );
    audited_text(validation, ACCOUNT_NOTFND_ROW)
}

/// Asserts that `resp` is the error answer of the validation row `id` of
/// `validation`, whose text the messages constant `message` holds: the row's
/// status, the error code of its kind, `message`, and one `fields` entry, the
/// row's BMS field, carrying `message` too. A status, code or message that
/// differs names `rows`, the audit rows of the case.
fn assert_refused(resp: &Resp, validation: &Document, id: &str, rows: &str, message: &str) {
    let answer = audited(validation, id, message);
    let Some(code) = answer.code else {
        panic!("{rows}: {id} records no error kind, so it refuses nothing");
    };
    let (actual, body, raw) = resp;
    assert_eq!(
        (*actual, body["code"].as_str(), body["message"].as_str()),
        (answer.status, Some(code), Some(message)),
        "{rows}: status, code and message of {id}: {raw}"
    );
    assert_error(
        resp,
        answer.status,
        code,
        message,
        &[(answer.field.as_str(), message)],
    );
}

/// Asserts that `resp` is the confirmation prompt `expected`: the status of
/// its message-only row in `validation`, and exactly the account, its
/// balance, a seeded one decoded through `parity`, and the row's text, which
/// [`messages::CONFIRM_TO_MAKE_A_BILL_PAYMENT`] holds, with no TRAN-ID.
fn assert_prompt(resp: &Resp, expected: &Prompt, parity: &Document, validation: &Document) {
    let answer = audited(
        validation,
        expected.row,
        messages::CONFIRM_TO_MAKE_A_BILL_PAYMENT,
    );
    assert_eq!(
        answer.code, None,
        "{}: {} sets no error kind",
        expected.rows, expected.row
    );
    let (status, body, raw) = resp;
    let prompt = json!({
        "account_id": expected.account_id,
        "current_balance": expected.balance.of(parity, expected.account_id),
        "message": answer.message,
    });
    assert_eq!(
        (*status, body),
        (answer.status, &prompt),
        "{}: the prompt: {raw}",
        expected.rows
    );
}

/// The success text of P-COBIL00C-016: the quoted pieces `STRING` joins
/// around the TRAN-ID.
struct Success {
    /// The first piece, `"Payment successful. "`.
    head: String,
    /// The piece before the TRAN-ID, `" Your Transaction ID is "`.
    middle: String,
    /// The piece after the TRAN-ID, `"."`.
    tail: String,
}

impl Success {
    /// The success text of the payment given `tran_id`. The TRAN-ID holds no
    /// space, so `DELIMITED BY SPACE` takes all 16 digits.
    fn text(&self, tran_id: &str) -> String {
        [self.head.as_str(), &self.middle, tran_id, &self.tail].concat()
    }
}

/// The success text the reviewed P-COBIL00C-016 of `parity` records: the
/// three quoted pieces of its `message=`.
fn success_answer(parity: &Document) -> Success {
    let id = "P-COBIL00C-016";
    assert_reviewed(parity, &[id]);
    let result = expected_cell(parity, id);
    let Some(message) = carddemo_audit::key_values(result)
        .into_iter()
        .find(|pair| pair.key == "message")
    else {
        panic!("{id} gives no message=: {result}");
    };
    let pieces: Vec<String> = carddemo_audit::cell_code_spans(&message.value)
        .iter()
        .map(|span| unquote(&span.text, id))
        .collect();
    let Ok([head, middle, tail]) = <[String; 3]>::try_from(pieces) else {
        panic!("{id} builds its message from three quoted pieces: {result}");
    };
    Success { head, middle, tail }
}

/// Asserts that `resp` is the 201 answer of a payment on `account_id` under
/// `tran_id`: exactly the balance after it, the TRAN-ID and the text
/// `success` joins around it, which the catalog's
/// [`messages::payment_successful_message`] composes too.
fn assert_paid(
    resp: &Resp,
    success: &Success,
    rows: &str,
    account_id: &str,
    balance: &str,
    tran_id: &str,
) {
    let text = success.text(tran_id);
    assert_eq!(
        messages::payment_successful_message(tran_id),
        text,
        "P-COBIL00C-016: the catalog composes the audited success text of {tran_id}"
    );
    let (status, body, raw) = resp;
    let paid = json!({
        "account_id": account_id,
        "current_balance": balance,
        "tran_id": tran_id,
        "message": text,
    });
    assert_eq!(
        (*status, body),
        (StatusCode::CREATED, &paid),
        "{rows}: the payment: {raw}"
    );
}

/// The literals `PROCESS-ENTER-KEY` moves into the payment record, as the
/// reviewed P-COBIL00C-015 records them, each as [`ADDED_ROWS`] reads its
/// column.
struct RecordLiterals {
    /// `TRAN-TYPE-CD`.
    type_cd: String,
    /// `TRAN-CAT-CD`.
    cat_cd: String,
    /// `TRAN-SOURCE`.
    source: String,
    /// `TRAN-DESC`.
    desc: String,
    /// `TRAN-MERCHANT-ID`.
    merchant_id: String,
    /// `TRAN-MERCHANT-NAME`.
    merchant_name: String,
    /// `TRAN-MERCHANT-CITY`.
    merchant_city: String,
    /// `TRAN-MERCHANT-ZIP`.
    merchant_zip: String,
}

/// The [`RecordLiterals`] of the reviewed P-COBIL00C-015 of `parity`: the
/// comma-separated `fields=` of its expected result, each a label and its
/// value, a single-quoted code, a number or a quoted text. No literal holds a
/// comma.
fn record_literals(parity: &Document) -> RecordLiterals {
    let id = "P-COBIL00C-015";
    assert_reviewed(parity, &[id]);
    let result = expected_cell(parity, id);
    let Some(fields) = carddemo_audit::key_values(result)
        .into_iter()
        .find(|pair| pair.key == "fields")
    else {
        panic!("{id} gives no fields=: {result}");
    };
    let items: Vec<&str> = fields.value.split(", ").collect();
    let value = |label: &str| {
        let Some(value) = items.iter().find_map(|item| item.strip_prefix(label)) else {
            panic!("{id} records no {label:?}: {}", fields.value);
        };
        value.trim()
    };
    let number = |label: &str| {
        let text = value(label).split_whitespace().next().unwrap_or_default();
        let number: u64 = text
            .parse()
            .unwrap_or_else(|error| panic!("{id}: {label:?} {text:?} is no number: {error}"));
        number.to_string()
    };
    let quoted = |label: &str| {
        let Some(span) = carddemo_audit::code_span(value(label)) else {
            panic!("{id} quotes no text after {label:?}: {}", fields.value);
        };
        unquote(&span.text, id)
    };
    let code = value("TRAN-TYPE-CD ");
    let Some(type_cd) = code
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    else {
        panic!("{id}: TRAN-TYPE-CD {code:?} is no single-quoted code");
    };
    RecordLiterals {
        type_cd: type_cd.to_owned(),
        cat_cd: number("TRAN-CAT-CD "),
        source: quoted("TRAN-SOURCE "),
        desc: quoted("TRAN-DESC "),
        merchant_id: number("merchant id "),
        merchant_name: quoted("name "),
        merchant_city: quoted("city "),
        merchant_zip: quoted("zip "),
    }
}

/// The payment record under `tran_id` for `amount` on the card `card`, as
/// [`ADDED_ROWS`] reads it: the `literals` of P-COBIL00C-015 and both
/// timestamps [`PAYMENT_TS`].
fn payment_row(
    literals: &RecordLiterals,
    tran_id: &str,
    amount: &str,
    card: &str,
) -> Vec<Option<String>> {
    [
        tran_id,
        &literals.type_cd,
        &literals.cat_cd,
        &literals.source,
        &literals.desc,
        amount,
        &literals.merchant_id,
        &literals.merchant_name,
        &literals.merchant_city,
        &literals.merchant_zip,
        card,
        PAYMENT_TS,
        PAYMENT_TS,
    ]
    .into_iter()
    .map(|value| Some(value.to_owned()))
    .collect()
}

/// Asserts that the reviewed parity row `id` of `parity` records `text` in
/// its cell `name`.
fn assert_records(parity: &Document, id: &str, name: &str, text: &str) {
    assert_reviewed(parity, &[id]);
    let recorded = cell(parity, id, name);
    assert!(
        recorded.contains(text),
        "{id}: its {name} records no {text:?}: {recorded}"
    );
}

/// What a confirmed payment leaves when `fault` fails one or both of its
/// writes, as the reviewed E-19 rows of `parity` record it: the balance the
/// account ends with, and whether the payment's transaction row persists.
///
/// A completed transaction `WRITE` persists when the `REWRITE` then fails
/// (P-COBIL00C-016, `RECOVERY(NONE)`); a failed `WRITE` still has the balance
/// reduced and rewritten (P-COBIL00C-017, P-COBIL00C-018, P-COBIL00C-024),
/// each write under its own savepoint (P-COBIL00C-024); and the `REWRITE`'s
/// message replaces any earlier one (P-COBIL00C-019), so the case's primary
/// row in `validation` is that of the last write that fails.
fn e19_outcome(parity: &Document, validation: &Document, fault: &WriteFault) -> (After, bool) {
    assert_reviewed(parity, &["P-COBIL00C-016"]);
    let persists = cell(parity, "P-COBIL00C-016", "Persists on later failure");
    assert!(
        persists.starts_with("persists") && persists.contains("RECOVERY(NONE)"),
        "P-COBIL00C-016: the transaction write persists on a later failure: {persists}"
    );
    assert_records(
        parity,
        "P-COBIL00C-017",
        "Expected result",
        "the balance update below still runs (E-19)",
    );
    assert_records(
        parity,
        "P-COBIL00C-018",
        "Input condition",
        "whatever its outcome",
    );
    assert_records(
        parity,
        "P-COBIL00C-019",
        "Expected result",
        "this message replaces any earlier one",
    );
    assert_records(
        parity,
        "P-COBIL00C-024",
        "Expected result",
        "the balance rewrite still runs; each write under its own savepoint",
    );
    let last = if fault.fails_rewrite() {
        "UPDATE-ACCTDAT-FILE"
    } else {
        "WRITE-TRANSACT-FILE"
    };
    let paragraph = cell(validation, fault.row, "Validation paragraph");
    assert!(
        paragraph.starts_with(last),
        "{}: {} is the row of the last failing write, {last}: {paragraph}",
        fault.rows,
        fault.row
    );
    let balance = if fault.fails_rewrite() {
        After::Unchanged
    } else {
        After::Paid
    };
    (balance, !fault.fails_write())
}

/// `WS-TRAN-ID-NUM + 1`: the TRAN-ID after `tran_id`, the highest key, in the
/// `9(16)` working field, zero-padded to 16 digits (P-COBIL00C-012).
fn next_tran_id(tran_id: &str) -> String {
    let number: u64 = tran_id
        .parse()
        .unwrap_or_else(|error| panic!("{tran_id:?} is no TRAN-ID: {error}"));
    format!("{:016}", number + 1)
}

/// Sets the balance of `account_id`, an 11-digit entry, in `balances`, a
/// [`BALANCES`] result.
///
/// # Panics
///
/// Panics if `balances` holds no row for the account.
fn set_balance(balances: &mut [Vec<Option<String>>], account_id: &str, balance: &str) {
    let key = account_id.trim_start_matches('0');
    let row = balances
        .iter_mut()
        .find(|row| row.first().and_then(Option::as_deref) == Some(key))
        .unwrap_or_else(|| panic!("account {account_id} is not among the balances"));
    row[1] = Some(balance.to_owned());
}

/// The reviewed section D row of account 1's seeded state.
const ACCOUNT_ONE_SEED: &str = "P-CVACT01Y-015";

/// The magnitude from which `MOVE ACCT-CURR-BAL TO TRAN-AMT` drops high-order
/// digits: 1,000,000,000.00, the first balance whose integer part exceeds the
/// nine digits of `TRAN-AMT S9(09)V99` (P-COBIL00C-021).
const TRAN_AMT_OVERFLOW: i64 = 1_000_000_000;

/// `ACCT-CURR-BAL` of the account `account_id` as seeded: the `acct_curr_bal`
/// of its `acctdata.txt` record, decoded at the reviewed `CVACT01Y` offsets.
fn seeded_balance(parity: &Document, account_id: &str) -> String {
    fixture_records::account(parity, account_id)
        .value("acct_curr_bal")
        .to_owned()
}

/// `XREF-CARD-NUM` of the account `account_id` as seeded, which
/// `READ-CXACAIX-FILE` reads by `XREF-ACCT-ID` (P-COBIL00C-011): the card of
/// its one `cardxref.txt` record, decoded at the reviewed `CVACT03Y` offsets
/// (P-CVACT03Y-001, P-CVACT03Y-003, P-CVACT03Y-006).
fn seeded_card(parity: &Document, account_id: &str) -> String {
    fixture_records::account_xref(parity, account_id)
        .value("xref_card_num")
        .to_owned()
}

/// The confirmed payment of a seeded balance, as the audited rules give it.
struct Payment {
    /// `ACCT-CURR-BAL` as seeded ([`seeded_balance`]).
    balance: String,
    /// `TRAN-AMT`, the balance moved to `S9(09)V99` (P-COBIL00C-021).
    amount: String,
    /// `ACCT-CURR-BAL - TRAN-AMT`, what the `REWRITE` stores (P-COBIL00C-022).
    after: String,
    /// The card of the payment record, the account's `XREF-CARD-NUM`
    /// ([`seeded_card`], P-COBIL00C-015).
    card: String,
    /// The literals of the payment record ([`record_literals`],
    /// P-COBIL00C-015).
    literals: RecordLiterals,
}

impl Payment {
    /// The payment of the seeded balance of the account `account_id`.
    ///
    /// # Panics
    ///
    /// Panics if the seeded balance is no decimal, is not above zero, which
    /// the guard of P-COBIL00C-020 refuses, or reaches [`TRAN_AMT_OVERFLOW`],
    /// which the `MOVE` of P-COBIL00C-021 truncates; and as
    /// [`fixture_records::account`], [`fixture_records::account_xref`] and
    /// [`record_literals`] do.
    fn of(parity: &Document, account_id: &str) -> Self {
        let balance = seeded_balance(parity, account_id);
        let seeded = Decimal::from_str(&balance).unwrap_or_else(|error| {
            panic!("account {account_id}: the seeded balance {balance:?} is no decimal: {error}")
        });
        assert!(
            seeded > Decimal::ZERO,
            "P-COBIL00C-020: account {account_id} has its seeded {balance} to pay"
        );
        assert!(
            seeded < Decimal::from(TRAN_AMT_OVERFLOW),
            "P-COBIL00C-021: the seeded {balance} of account {account_id} fits TRAN-AMT whole"
        );
        // Below the overflow, the `MOVE` keeps every digit of the balance.
        let amount = seeded;
        Self {
            amount: amount.to_string(),
            after: (seeded - amount).to_string(),
            card: seeded_card(parity, account_id),
            literals: record_literals(parity),
            balance,
        }
    }

    /// The payment record under `tran_id`, as [`ADDED_ROWS`] reads it.
    fn record(&self, tran_id: &str) -> Vec<Option<String>> {
        payment_row(&self.literals, tran_id, &self.amount, &self.card)
    }
}

/// Asserts that the reviewed section D row of account 1's seeded state
/// (P-CVACT01Y-015) records the account `account_id` with `balance`, the
/// balance decoded from its fixture record: the payment derived from the
/// fixture is the payment of the reviewed state.
fn assert_reviewed_seed(parity: &Document, account_id: &str, balance: &str) {
    assert_eq!(
        (
            fixture_records::section_d_value(parity, ACCOUNT_ONE_SEED, "value"),
            fixture_records::section_d_value(parity, ACCOUNT_ONE_SEED, "acct_curr_bal"),
        ),
        (account_id.to_owned(), balance.to_owned()),
        "P-CVACT01Y-015: the reviewed seeded state of account {account_id}"
    );
}

/// Every `V-COBIL00C` row is the primary row of a case of this file, and each
/// case agrees with its row: the row's review passed, the messages constant
/// the case names is the row's text, a refusal's row is an error with a kind
/// and a prompt's row a success without one, and a write fault's row is that
/// of its last failing write. Needs no container.
#[test]
fn every_cobil00c_row_is_a_case_with_its_audited_answer() {
    let validation = common::validation_audit();
    let parity = common::parity_audit();
    let mut primary = BTreeSet::new();
    let refusals = REFUSALS
        .iter()
        .map(|case| (case.row, case.rows, case.message))
        .chain(
            WRITE_FAULTS
                .iter()
                .map(|case| (case.row, case.rows, case.message)),
        )
        .chain(
            READ_FAULTS
                .iter()
                .map(|case| (case.row, case.rows, case.message)),
        );
    for (row, rows, message) in refusals {
        let answer = audited(&validation, row, message);
        assert!(
            answer.code.is_some()
                && (answer.status.is_client_error() || answer.status.is_server_error()),
            "{rows}: {row} refuses with an error kind: {answer:?}"
        );
        primary.insert(row.to_owned());
    }
    for prompt in &PROMPTS {
        let answer = audited(
            &validation,
            prompt.row,
            messages::CONFIRM_TO_MAKE_A_BILL_PAYMENT,
        );
        assert!(
            answer.code.is_none() && answer.status.is_success(),
            "{}: {} answers without an error kind: {answer:?}",
            prompt.rows,
            prompt.row
        );
        primary.insert(prompt.row.to_owned());
    }
    for case in &WRITE_FAULTS {
        e19_outcome(&parity, &validation, case);
    }
    let defined: BTreeSet<String> = common::validation_ids(PROGRAM).into_iter().collect();
    assert_eq!(
        primary, defined,
        "the primary rows of this file's cases are the {PROGRAM} rows of the audit"
    );
}

/// `PROCESS-ENTER-KEY` up to the TRAN-ID allocation: every entry edit, read
/// refusal and confirmation prompt answers as audited, and none writes.
#[test]
fn entry_edits_refusals_and_prompts_write_nothing() {
    let parity = common::parity_audit();
    let validation = common::validation_audit();
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        app.stack().exec_sql(REFUSAL_SETUP).await;
        let before = written(&app).await;
        let token = app.user_token().await;
        for refusal in &REFUSALS {
            let body = request(refusal.account_id, refusal.confirm);
            let resp = app.post(PAYMENTS, Some(&token), &body).await;
            assert_refused(
                &resp,
                &validation,
                refusal.row,
                refusal.rows,
                refusal.message,
            );
        }
        for prompt in &PROMPTS {
            let body = request(Some(prompt.account_id), prompt.confirm);
            let resp = app.post(PAYMENTS, Some(&token), &body).await;
            assert_prompt(&resp, prompt, &parity, &validation);
        }
        assert_eq!(
            written(&app).await,
            before,
            "the refusals and the prompts wrote nothing"
        );
    });
}

/// A confirmed payment writes the bill-payment transaction for the whole
/// balance, moved to `TRAN-AMT` with high-order truncation, and rewrites the
/// balance reduced by it; the paid account then has nothing to pay. Account
/// 1 pays its seeded balance, the one P-CVACT01Y-015 records; account 3 pays
/// the oversized balance [`OVERSIZED_BALANCE`] sets, on its seeded card.
#[test]
fn a_confirmed_payment_pays_the_whole_balance() {
    let parity = common::parity_audit();
    let validation = common::validation_audit();
    let success = success_answer(&parity);
    let payment = Payment::of(&parity, "00000000001");
    assert_reviewed_seed(&parity, "00000000001", &payment.balance);
    let oversized_card = seeded_card(&parity, "00000000003");
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROW).await;
        stack.exec_sql(OVERSIZED_BALANCE).await;
        let mut balances = stack.fetch_text_rows(BALANCES).await;
        let rest = stack.fetch_text_rows(ACCOUNT_REST).await;
        let token = app.user_token().await;

        let body = request(Some("00000000001"), Some("Y"));
        let paid = app.post(PAYMENTS, Some(&token), &body).await;
        let rows =
            "P-COBIL00C-012, P-COBIL00C-016, P-COBIL00C-022: highest key + 1, whole balance paid";
        assert_paid(
            &paid,
            &success,
            rows,
            "00000000001",
            &payment.after,
            "0000000000000301",
        );
        let first = payment.record("0000000000000301");
        assert_eq!(
            stack.fetch_text_rows(ADDED_ROWS).await,
            [first.clone()],
            "P-COBIL00C-015, P-COBIL00C-026: the payment record"
        );

        let again = app.post(PAYMENTS, Some(&token), &body).await;
        assert_refused(
            &again,
            &validation,
            "V-COBIL00C-007",
            "V-COBIL00C-007, P-COBIL00C-010: the account just paid",
            messages::YOU_HAVE_NOTHING_TO_PAY,
        );

        // 1234567890.12 loses its high-order digit in the `S9(09)V99`
        // `TRAN-AMT`, and the `S9(10)V99` balance keeps the difference.
        let body = request(Some("00000000003"), Some("y"));
        let truncated = app.post(PAYMENTS, Some(&token), &body).await;
        let rows =
            "P-COBIL00C-005, P-COBIL00C-021, P-COBIL00C-022: 1234567890.12 pays 234567890.12";
        assert_paid(
            &truncated,
            &success,
            rows,
            "00000000003",
            "1000000000.00",
            "0000000000000302",
        );
        let second = payment_row(
            &payment.literals,
            "0000000000000302",
            "234567890.12",
            &oversized_card,
        );
        assert_eq!(
            stack.fetch_text_rows(ADDED_ROWS).await,
            [first, second],
            "P-COBIL00C-015, P-COBIL00C-021: the payment records"
        );

        set_balance(&mut balances, "00000000001", &payment.after);
        set_balance(&mut balances, "00000000003", "1000000000.00");
        assert_eq!(
            stack.fetch_text_rows(BALANCES).await,
            balances,
            "P-COBIL00C-022: only the paid balances changed"
        );
        assert_eq!(
            stack.fetch_text_rows(ACCOUNT_REST).await,
            rest,
            "the account rewrite changes the balance alone"
        );
        assert_eq!(
            stack.fetch_text_rows(TRANSACTION_COUNT).await,
            [[Some("3".to_owned())]],
            "the base row and the two payments"
        );
        assert_eq!(
            stack.fetch_text_rows(ACCRUAL_COUNT).await,
            [[Some("0".to_owned())]],
            "a bill payment accrues no reward points"
        );
    });
}

/// E-19: each write of a confirmed payment keeps its outcome when the other
/// fails, as the reviewed E-19 rows record it ([`e19_outcome`]), and the
/// answer is the last message the program set. A persisting payment row
/// takes the TRAN-ID after the highest persisted key (P-COBIL00C-012).
#[test]
fn a_failed_write_keeps_the_other_write() {
    let parity = common::parity_audit();
    let validation = common::validation_audit();
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROW).await;
        stack.exec_sql(FAULT_FUNCTIONS).await;
        let mut balances = stack.fetch_text_rows(BALANCES).await;
        let mut persisted = Vec::new();
        let mut highest = LIVE_TRAN_ID.to_owned();
        let token = app.user_token().await;
        for case in &WRITE_FAULTS {
            let payment = Payment::of(&parity, case.account_id);
            let (after, persists) = e19_outcome(&parity, &validation, case);
            for fault in case.faults {
                stack.exec_sql(fault).await;
            }
            let body = request(Some(case.account_id), Some("Y"));
            let resp = app.post(PAYMENTS, Some(&token), &body).await;
            for repair in case.repairs {
                stack.exec_sql(repair).await;
            }
            assert_refused(&resp, &validation, case.row, case.rows, case.message);
            set_balance(&mut balances, case.account_id, after.of(&payment));
            assert_eq!(
                stack.fetch_text_rows(BALANCES).await,
                balances,
                "{}: the balances",
                case.rows
            );
            if persists {
                let tran_id = next_tran_id(&highest);
                persisted.push(payment.record(&tran_id));
                highest = tran_id;
            }
            assert_eq!(
                stack.fetch_text_rows(ADDED_ROWS).await,
                persisted,
                "{}: the payment rows that persist",
                case.rows
            );
            let count = (persisted.len() + 1).to_string();
            assert_eq!(
                stack.fetch_text_rows(TRANSACTION_COUNT).await,
                [[Some(count)]],
                "{}: the transaction rows, live or not",
                case.rows
            );
        }
    });
}

/// A failed account, cross-reference or highest-key read stops the payment
/// with its 500 message and writes nothing; with the relations restored the
/// same payment goes through.
#[test]
fn a_failed_read_writes_nothing() {
    let parity = common::parity_audit();
    let validation = common::validation_audit();
    let success = success_answer(&parity);
    let payment = Payment::of(&parity, "00000000001");
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROW).await;
        let before = written(&app).await;
        let token = app.user_token().await;
        for case in &READ_FAULTS {
            stack.exec_sql(case.fault).await;
            let body = request(Some("00000000001"), Some(case.confirm));
            let resp = app.post(PAYMENTS, Some(&token), &body).await;
            stack.exec_sql(case.repair).await;
            assert_refused(&resp, &validation, case.row, case.rows, case.message);
        }
        assert_eq!(
            written(&app).await,
            before,
            "the failed reads wrote nothing"
        );
        let body = request(Some("00000000001"), Some("Y"));
        let paid = app.post(PAYMENTS, Some(&token), &body).await;
        let rows = "P-COBIL00C-012, P-COBIL00C-016: the restored relations serve the payment";
        assert_paid(
            &paid,
            &success,
            rows,
            "00000000001",
            &payment.after,
            "0000000000000301",
        );
    });
}

// ---------------------------------------------------------------------------
// The 3270 mechanics the one POST replaces (P-COBIL00C-001, P-COBIL00C-002,
// P-COBIL00C-023, P-COBIL00C-027 to P-COBIL00C-029, P-COBIL00C-031)
// ---------------------------------------------------------------------------

/// The 401 code of a request without a valid bearer token.
const UNAUTHENTICATED: &str = "UNAUTHENTICATED";

/// The keys of an `OpenAPI` path item that name operations.
const HTTP_METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// The schema of the `POST /payments` body in the published document.
const REQUEST_SCHEMA: &str = "PaymentRequest";

/// The schema of the 200 prompt and the 201 payment.
const RESPONSE_SCHEMA: &str = "PaymentResponse";

/// The members of the body: `ACTIDIN` and `CONFIRM`, the input fields of
/// `COBIL0AI` (P-COBIL00C-028).
const REQUEST_MEMBERS: [&str; 2] = ["account_id", "confirm"];

/// The members of a payment answer: no `COBIL0AO` header field
/// (P-COBIL00C-029). The prompt leaves out `tran_id`.
const RESPONSE_MEMBERS: [&str; 4] = ["account_id", "current_balance", "message", "tran_id"];

/// The members of every error body.
const ERROR_MEMBERS: [&str; 3] = ["code", "fields", "message"];

/// The CAVW route of the balance preview (P-COBIL00C-028).
const ACCOUNT_ROUTE: &str = "/accounts/{id}";

/// The AID keys other than ENTER, each sent in a body member the request
/// does not declare: PF3 (`RETURN-TO-PREV-SCREEN`), PF4
/// (`CLEAR-CURRENT-SCREEN`), and keys `WHEN OTHER` answers (P-COBIL00C-002).
const AID_KEYS: [&str; 4] = ["PF3", "PF4", "CLEAR", "PA1"];

/// Account 3 at the balance of the first P-COBIL00C-023 example, and account
/// 4 with nothing to pay.
const MECHANICS_SETUP: &str = "UPDATE accounts SET acct_curr_bal = 1940.00 WHERE account_id = 3; \
     UPDATE accounts SET acct_curr_bal = 0.00 WHERE account_id = 4";

/// The review verdict a row must carry before a test relies on it.
const PASS: &str = "pass";

/// The rows the published document proves: the AID keys, the session the
/// bearer token stands for, the received map and the header.
const SURFACE_ROWS: [&str; 4] = [
    "P-COBIL00C-002",
    "P-COBIL00C-027",
    "P-COBIL00C-028",
    "P-COBIL00C-029",
];

/// The rows the mechanics test proves through the running API.
const MECHANICS_ROWS: [&str; 7] = [
    "P-COBIL00C-001",
    "P-COBIL00C-002",
    "P-COBIL00C-023",
    "P-COBIL00C-027",
    "P-COBIL00C-028",
    "P-COBIL00C-029",
    "P-COBIL00C-031",
];

/// CB00 publishes one operation, the POST of ENTER: PF3, PF4 and every other
/// AID key have none, the body replaces the received map and carries no key,
/// `GET /accounts/{id}` serves the balance preview, the operation needs the
/// bearer token that stands for the COMMAREA, and its answers carry no 3270
/// header field.
#[test]
fn cb00_publishes_one_post_whose_body_replaces_the_map() {
    let doc = common::parity_audit();
    assert_reviewed(&doc, &SURFACE_ROWS);
    // WHEN OTHER moves CCDA-MSG-INVALID-KEY, the catalog literal in its X(50)
    // field; no request reaches it (E-56).
    let invalid_key = quoted(&doc, "P-COBIL00C-002");
    let [(text, Some(width))] = invalid_key.as_slice() else {
        panic!("P-COBIL00C-002 quotes no one text with its width: {invalid_key:?}");
    };
    let width = *width;
    let catalog = messages::CCDA_MSG_INVALID_KEY;
    assert_eq!(
        format!("{catalog:<width$}"),
        format!("{text:<width$}"),
        "P-COBIL00C-002: the WHEN OTHER text"
    );
    assert_replacement(&doc, "P-COBIL00C-027", "no token gives 401");
    assert_replacement(
        &doc,
        "P-COBIL00C-028",
        "GET /accounts/{id} for the balance preview and the JSON body of POST /payments",
    );
    assert_replacement(&doc, "P-COBIL00C-029", "no API field carries the header");

    let spec: Value = serde_json::from_str(&carddemo_api::spec_json())
        .unwrap_or_else(|error| panic!("the OpenAPI document is not JSON: {error}"));
    let item = &spec["paths"][PAYMENTS];
    assert_eq!(
        operations(item),
        BTreeSet::from(["post"]),
        "P-COBIL00C-002, P-COBIL00C-027: ENTER is the one POST; PF3, PF4 and the other AID \
         keys have no operation: {item}"
    );
    let post = &item["post"];
    assert!(
        post.get("parameters").is_none(),
        "P-COBIL00C-002: no parameter carries an AID key: {post}"
    );
    assert_eq!(
        post["security"],
        json!([{ "bearer_auth": [] }]),
        "P-COBIL00C-027: a request without a session is refused"
    );
    assert_eq!(
        post["requestBody"]["content"]["application/json"]["schema"]["$ref"],
        Value::from(format!("#/components/schemas/{REQUEST_SCHEMA}")),
        "P-COBIL00C-028: the JSON body replaces the received map"
    );
    let schemas = &spec["components"]["schemas"];
    assert_eq!(
        members(&schemas[REQUEST_SCHEMA]["properties"]),
        BTreeSet::from(REQUEST_MEMBERS),
        "P-COBIL00C-028, P-COBIL00C-002: the body holds the map's input fields and no key"
    );
    for status in ["200", "201"] {
        assert_eq!(
            post["responses"][status]["content"]["application/json"]["schema"]["$ref"],
            Value::from(format!("#/components/schemas/{RESPONSE_SCHEMA}")),
            "P-COBIL00C-029: the {status} answer"
        );
    }
    assert_eq!(
        members(&schemas[RESPONSE_SCHEMA]["properties"]),
        BTreeSet::from(RESPONSE_MEMBERS),
        "P-COBIL00C-029: no member carries the title, transaction, program, date or time \
         of the header"
    );
    assert!(
        operations(&spec["paths"][ACCOUNT_ROUTE]).contains("get"),
        "P-COBIL00C-028: GET {ACCOUNT_ROUTE} serves the balance preview"
    );
}

/// P-COBIL00C-023: `WS-CURR-BAL PIC +9999999999.99` edits each balance the
/// row names to the row's screen text, sign, ten digits with leading zeros,
/// point and two decimals.
#[test]
fn the_screen_balance_picture_edits_the_audited_examples() {
    let doc = common::parity_audit();
    assert_reviewed(&doc, &["P-COBIL00C-023"]);
    let examples = edited_examples(&doc, "P-COBIL00C-023");
    assert!(!examples.is_empty(), "P-COBIL00C-023 edits no example");
    for (value, text) in &examples {
        assert_eq!(
            edit(&PIC_BILL_CURR_BAL, *value),
            *text,
            "P-COBIL00C-023: {value} on the screen"
        );
        assert_eq!(
            text.len(),
            PIC_BILL_CURR_BAL.width(),
            "P-COBIL00C-023: the width of {value}"
        );
    }
}

/// The screen mechanics through the running API: a preselected account, a
/// MAPFAIL and the AID keys other than ENTER, a missing session, and the
/// emptied form; only the confirmed payment writes.
#[test]
fn cb00_screen_mechanics_answer_through_the_one_post() {
    let doc = common::parity_audit();
    assert_reviewed(&doc, &MECHANICS_ROWS);
    assert_replacement(
        &doc,
        "P-COBIL00C-031",
        "a successful payment is 201 with that text in message",
    );
    let examples = edited_examples(&doc, "P-COBIL00C-023");
    let validation = common::validation_audit();
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROW).await;
        stack.exec_sql(MECHANICS_SETUP).await;
        let token = app.user_token().await;
        let before = written(&app).await;
        preselected_account(&app, &token, &doc, &validation, &examples).await;
        received_map_and_keys(&app, &token, &validation).await;
        without_a_session(&app).await;
        assert_eq!(
            written(&app).await,
            before,
            "P-COBIL00C-001, P-COBIL00C-002, P-COBIL00C-027, P-COBIL00C-028: nothing written"
        );
        initialized_form(&app, &token, &doc, &validation).await;
    });
}

/// P-COBIL00C-001: a first entry with a preselected account copies it to
/// `ACTIDINI` and runs `PROCESS-ENTER-KEY` with `CONFIRMI` still
/// `LOW-VALUES`, so the body holding the account alone answers the prompt,
/// with the balance the `GET /accounts/{id}` preview shows, or the
/// nothing-to-pay refusal, each as its row of `validation` answers: account
/// 1 at its seeded balance, decoded through `parity`, and account 3 at the
/// balance [`MECHANICS_SETUP`] sets. P-COBIL00C-023: the prompt carries
/// `ACCT-CURR-BAL` exactly, and `WS-CURR-BAL` edits it to the audited screen
/// text.
async fn preselected_account(
    app: &TestApp,
    token: &str,
    parity: &Document,
    validation: &Document,
    examples: &[(Decimal, String)],
) {
    for (account_id, balance) in [
        ("00000000001", Balance::Seeded),
        ("00000000003", Balance::Set("1940.00")),
    ] {
        let resp = app
            .post(PAYMENTS, Some(token), &request(Some(account_id), None))
            .await;
        let prompt = Prompt {
            row: "V-COBIL00C-008",
            rows: "P-COBIL00C-001: the preselected account alone",
            account_id,
            confirm: None,
            balance,
        };
        assert_prompt(&resp, &prompt, parity, validation);
        let view = app
            .get(&format!("/accounts/{account_id}"), Some(token))
            .await;
        assert_eq!(
            (view.0, &view.1["acct_curr_bal"]),
            (StatusCode::OK, &resp.1["current_balance"]),
            "P-COBIL00C-001: the preview of account {account_id}: {}",
            view.2
        );
    }
    let Some((value, text)) = examples.first() else {
        panic!("P-COBIL00C-023 edits no example");
    };
    assert_eq!(
        Decimal::from_str("1940.00").ok(),
        Some(*value),
        "P-COBIL00C-023: MECHANICS_SETUP gives account 3 the first example's balance"
    );
    assert_eq!(
        edit(&PIC_BILL_CURR_BAL, *value),
        *text,
        "P-COBIL00C-023: the screen text of account 3's prompt balance"
    );
    let resp = app
        .post(PAYMENTS, Some(token), &request(Some("00000000004"), None))
        .await;
    assert_refused(
        &resp,
        validation,
        "V-COBIL00C-007",
        "P-COBIL00C-001: a preselected account with nothing to pay",
        messages::YOU_HAVE_NOTHING_TO_PAY,
    );
}

/// P-COBIL00C-028: an empty body, the received map of a MAPFAIL, still runs
/// `PROCESS-ENTER-KEY`, whose account edit answers as its row of
/// `validation`. P-COBIL00C-002: a body
/// member naming an AID key is no input, so every key answers as ENTER does
/// with the same body, never with the invalid-key text, and PF4 clears
/// nothing on the server. P-COBIL00C-029: the error body carries no header
/// field.
async fn received_map_and_keys(app: &TestApp, token: &str, validation: &Document) {
    let empty = app.post(PAYMENTS, Some(token), &json!({})).await;
    assert_refused(
        &empty,
        validation,
        "V-COBIL00C-001",
        "P-COBIL00C-028: an empty body",
        messages::ACCT_ID_CAN_NOT_BE_EMPTY,
    );
    assert_eq!(
        members(&empty.1),
        BTreeSet::from(ERROR_MEMBERS),
        "P-COBIL00C-029: the error body: {}",
        empty.2
    );
    let prompt = request(Some("00000000001"), None);
    let shown = app.post(PAYMENTS, Some(token), &prompt).await;
    for (body, enter) in [(json!({}), &empty), (prompt, &shown)] {
        for key in AID_KEYS {
            let mut keyed = body.clone();
            keyed["aid"] = Value::from(key);
            let resp = app.post(PAYMENTS, Some(token), &keyed).await;
            assert_eq!(
                (resp.0, &resp.1),
                (enter.0, &enter.1),
                "P-COBIL00C-002: {key} with {body} answers as ENTER: {}",
                resp.2
            );
        }
    }
}

/// P-COBIL00C-027: `EIBCALEN = 0` sends the terminal to the sign-on, so a
/// confirmed payment without a valid bearer token answers 401 with the
/// authentication text (P-COMEN01C-001), reading and writing nothing.
async fn without_a_session(app: &TestApp) {
    let body = request(Some("00000000001"), Some("Y"));
    for token in [None, Some("not-a-bearer-token")] {
        let resp = app.post(PAYMENTS, token, &body).await;
        assert_error(
            &resp,
            StatusCode::UNAUTHORIZED,
            UNAUTHENTICATED,
            messages::AUTHENTICATION_REQUIRED,
            &[],
        );
    }
}

/// P-COBIL00C-031: `INITIALIZE-ALL-FIELDS` empties the form with the cursor
/// on `ACTIDIN`. On `N` (`CLEAR-CURRENT-SCREEN`) the answer sets no message
/// and reads no account; after the payment's write it runs before the success
/// text is built, so the 201 message holds that text alone, not the prompt the
/// same account answered before; it pays account 1's seeded balance, decoded
/// through `parity`. The refusal and the prompt answer as their rows of
/// `validation`. P-COBIL00C-029: the payment carries no header field.
async fn initialized_form(app: &TestApp, token: &str, parity: &Document, validation: &Document) {
    let payment = Payment::of(parity, "00000000001");
    let success = success_answer(parity);
    for account_id in ["00000000001", "99999999999"] {
        let resp = app
            .post(PAYMENTS, Some(token), &request(Some(account_id), Some("N")))
            .await;
        assert_refused(
            &resp,
            validation,
            "V-COBIL00C-002",
            "P-COBIL00C-031: N clears the form",
            NO_MESSAGE,
        );
    }
    let body = request(Some("00000000001"), Some(""));
    let prompt = app.post(PAYMENTS, Some(token), &body).await;
    let shown = audited(
        validation,
        "V-COBIL00C-008",
        messages::CONFIRM_TO_MAKE_A_BILL_PAYMENT,
    );
    assert_eq!(
        (prompt.0, &prompt.1["message"]),
        (shown.status, &Value::from(shown.message)),
        "P-COBIL00C-031: the prompt before the payment: {}",
        prompt.2
    );
    let body = request(Some("00000000001"), Some("Y"));
    let paid = app.post(PAYMENTS, Some(token), &body).await;
    assert_paid(
        &paid,
        &success,
        "P-COBIL00C-031: the success text alone",
        "00000000001",
        &payment.after,
        "0000000000000301",
    );
    assert_eq!(
        members(&paid.1),
        BTreeSet::from(RESPONSE_MEMBERS),
        "P-COBIL00C-029: the payment: {}",
        paid.2
    );
}

/// The operations `item`, a published path item, declares.
fn operations(item: &Value) -> BTreeSet<&str> {
    item.as_object()
        .unwrap_or_else(|| panic!("the path is not published: {item}"))
        .keys()
        .map(String::as_str)
        .filter(|key| HTTP_METHODS.contains(key))
        .collect()
}

/// The member names of the JSON object `value`.
fn members(value: &Value) -> BTreeSet<&str> {
    value
        .as_object()
        .unwrap_or_else(|| panic!("{value} is not a JSON object"))
        .keys()
        .map(String::as_str)
        .collect()
}

/// Asserts that the last review verdict of each of `ids` is `pass`: no test
/// relies on a row its COBOL-informed review has not passed.
fn assert_reviewed(doc: &Document, ids: &[&str]) {
    let mut verdicts = BTreeMap::new();
    for table in doc.tables().iter().filter(|table| table.is_review_log()) {
        for row in &table.rows {
            if let (Some(id), Some(verdict)) =
                (table.cell(row, "Row ID"), table.cell(row, "Verdict"))
            {
                verdicts.insert(id, verdict);
            }
        }
    }
    for id in ids {
        assert_eq!(
            verdicts.get(id).copied(),
            Some(PASS),
            "audit row {id} has no passing review"
        );
    }
}

/// The `Expected result` cell of the parity row `id`.
fn expected_cell<'d>(doc: &'d Document, id: &str) -> &'d str {
    let (table, row) = doc
        .find_row(id)
        .unwrap_or_else(|| panic!("the audit has no row {id}"));
    table
        .cell(row, "Expected result")
        .unwrap_or_else(|| panic!("{id} has no Expected result cell"))
}

/// Asserts that the `replacement=` of row `id`, which runs to the end of its
/// cell across `;`, names `replacement`: the REST counterpart a test proves is
/// the one the reviewed row records.
fn assert_replacement(doc: &Document, id: &str, replacement: &str) {
    let Some((_, named)) = expected_cell(doc, id).split_once("replacement=") else {
        panic!("{id} names no replacement=");
    };
    assert!(
        named.contains(replacement),
        "{id}: replacement={named} does not name {replacement:?}"
    );
}

/// Every code span of the expected result of row `id`, unquoted, with the
/// width the audit pads it to.
fn quoted(doc: &Document, id: &str) -> Vec<(String, Option<usize>)> {
    carddemo_audit::cell_code_spans(expected_cell(doc, id))
        .into_iter()
        .map(|span| (unquote(&span.text, id), span.width))
        .collect()
}

/// The `<value> -> <text>` examples of row `id`: each balance before an
/// arrow, and the quoted text after it, padded to its width.
///
/// # Panics
///
/// Panics, naming the row, if a balance is no decimal or a text is not
/// quoted with its width.
fn edited_examples(doc: &Document, id: &str) -> Vec<(Decimal, String)> {
    let segments: Vec<&str> = expected_cell(doc, id).split("->").collect();
    segments
        .windows(2)
        .map(|pair| {
            let value = pair[0]
                .split_whitespace()
                .last()
                .and_then(|value| Decimal::from_str(value).ok())
                .unwrap_or_else(|| panic!("{id}: no balance before -> in {:?}", pair[0]));
            let span = carddemo_audit::code_span(pair[1])
                .unwrap_or_else(|| panic!("{id}: no quoted text after {value} ->"));
            let width = span
                .width
                .unwrap_or_else(|| panic!("{id}: the text of {value} has no width"));
            let text = unquote(&span.text, id);
            (value, format!("{text:<width$}"))
        })
        .collect()
}

/// The value of the double-quoted audit string `content`, whose only escapes
/// are `\"` and `\\`.
fn unquote(content: &str, id: &str) -> String {
    let inner = content
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id}: {content:?} is not a double-quoted exact text"));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(escaped @ ('"' | '\\')) => text.push(escaped),
                _ => panic!("{id}: {content:?} holds a backslash escaping no quote or backslash"),
            },
            '"' => panic!("{id}: {content:?} holds an unescaped quote"),
            _ => text.push(ch),
        }
    }
    text
}
