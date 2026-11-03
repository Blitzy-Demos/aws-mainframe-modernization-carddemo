//! CB00 bill payment (`COBIL00C`, map `COBIL00`) through `POST /payments`:
//! the entry edits and refusals, the confirmation prompt, the confirmed
//! payment and its arithmetic, the write persistence of E-19, and the 3270
//! mechanics the one POST replaces: the AID keys, the received map, the
//! session, the screen header, the edited balance and the emptied form.
//!
//! Each test runs on its own seeded database ([`TestApp`]): the 50 fixture
//! accounts and an empty live transaction master (E-07). Expected messages
//! come from `carddemo_domain::messages`; statuses, error codes and BMS fields
//! come from the `V-COBIL00C` rows of `docs/parity/validation-audit.md`, and
//! the writes that persist from the `P-COBIL00C` rows of
//! `docs/parity/parity-oracle-audit.md`. Every case names the audit rows it
//! proves in a string literal, which the coverage gates read.
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
use carddemo_domain::error::ErrorKind;
use carddemo_domain::format::{PIC_BILL_CURR_BAL, edit};
use carddemo_domain::messages;
use reqwest::StatusCode;
use rust_decimal::Decimal;
use serde_json::{Map, Value, json};

use common::{Resp, TestApp, assert_error};

/// The processing clock, the source of the payment timestamps.
const CLOCK: &str = "2022-07-01T12:00:00Z";

/// `TRAN-ORIG-TS` and `TRAN-PROC-TS` of a payment made at [`CLOCK`]: the
/// `ASKTIME`/`FORMATTIME` text `YYYY-MM-DD HH:MM:SS.000000` of
/// `GET-CURRENT-TIMESTAMP`.
const PAYMENT_TS: &str = "2022-07-01 12:00:00.000000";

/// The route under test.
const PAYMENTS: &str = "/payments";

/// The account-ID field of `COBIL0A` [app/cpy-bms/COBIL00.CPY], the BMS
/// field of every `V-COBIL00C` row but two.
const ACTIDIN: &str = "ACTIDIN";

/// The confirmation field of `COBIL0A` [app/cpy-bms/COBIL00.CPY].
const CONFIRM: &str = "CONFIRM";

/// The text of the cancelled payment: `CLEAR-CURRENT-SCREEN` sets none, and
/// the audit records the path as `no-message (E-49)`.
const NO_MESSAGE: &str = "";

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
    /// The audit rows the case proves.
    rows: &'static str,
    /// `account_id` of the body; `None` leaves the field out.
    account_id: Option<&'static str>,
    /// `confirm` of the body; `None` leaves the field out.
    confirm: Option<&'static str>,
    /// The audited status.
    status: StatusCode,
    /// The error code of the audited kind.
    code: &'static str,
    /// The audited message.
    message: &'static str,
    /// The audited BMS field.
    field: &'static str,
}

/// The entry edits and the refusals of `PROCESS-ENTER-KEY` before any write,
/// in the COBOL order [app/cbl/COBIL00C.cbl:L154-219], after
/// [`REFUSAL_SETUP`]. None of them writes.
const REFUSALS: [Refusal; 19] = [
    Refusal {
        rows: "V-COBIL00C-001, P-COBIL00C-004",
        account_id: Some(""),
        confirm: Some("Y"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Empty.code(),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-001, P-COBIL00C-004: a missing field is a blank one",
        account_id: None,
        confirm: Some(""),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Empty.code(),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-001, P-COBIL00C-004: LOW-VALUES are blank",
        account_id: Some("\u{0}\u{0}\u{0}"),
        confirm: Some("Y"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Empty.code(),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-001: the account edit (order 1) precedes the confirmation edit",
        account_id: Some("   "),
        confirm: Some("X"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Empty.code(),
        message: messages::ACCT_ID_CAN_NOT_BE_EMPTY,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-002, P-COBIL00C-006",
        account_id: Some("00000000001"),
        confirm: Some("N"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: NO_MESSAGE,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-002, P-COBIL00C-006: lower-case n",
        account_id: Some("00000000001"),
        confirm: Some("n"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: NO_MESSAGE,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-002, P-COBIL00C-006: N reads no account",
        account_id: Some("99999999999"),
        confirm: Some("N"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: NO_MESSAGE,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-003, P-COBIL00C-008",
        account_id: Some("00000000001"),
        confirm: Some("X"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: messages::INVALID_VALUE_VALID_VALUES_ARE_Y_N,
        field: CONFIRM,
    },
    Refusal {
        rows: "V-COBIL00C-003, P-COBIL00C-008: an invalid confirmation reads no account",
        account_id: Some("99999999999"),
        confirm: Some("?"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: messages::INVALID_VALUE_VALID_VALUES_ARE_Y_N,
        field: CONFIRM,
    },
    Refusal {
        rows: "V-COBIL00C-004, P-COBIL00C-009",
        account_id: Some("99999999999"),
        confirm: Some(""),
        status: StatusCode::NOT_FOUND,
        code: ErrorKind::NotFound.code(),
        message: messages::ACCOUNT_ID_NOT_FOUND,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-004, P-COBIL00C-005, P-COBIL00C-009: confirmed",
        account_id: Some("99999999999"),
        confirm: Some("Y"),
        status: StatusCode::NOT_FOUND,
        code: ErrorKind::NotFound.code(),
        message: messages::ACCOUNT_ID_NOT_FOUND,
        field: ACTIDIN,
    },
    // A non-digit leaves the `9(11)` key unpinned (E-47, halt); the answer is
    // the escalation's proposal, the account `NOTFND` row without a read.
    Refusal {
        rows: "V-COBIL00C-006",
        account_id: Some("0000000000A"),
        confirm: Some(""),
        status: StatusCode::NOT_FOUND,
        code: ErrorKind::NotFound.code(),
        message: messages::ACCOUNT_ID_NOT_FOUND,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-007, P-COBIL00C-010, P-COBIL00C-020: zero balance, blank confirm",
        account_id: Some("00000000004"),
        confirm: Some(""),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-007, P-COBIL00C-010, P-COBIL00C-020: zero balance, confirmed",
        account_id: Some("00000000004"),
        confirm: Some("Y"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-007, P-COBIL00C-020: a negative balance compares algebraically",
        account_id: Some("00000000005"),
        confirm: Some(""),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-007, P-COBIL00C-020: negative balance, confirmed",
        account_id: Some("00000000005"),
        confirm: Some("Y"),
        status: StatusCode::BAD_REQUEST,
        code: ErrorKind::Invalid.code(),
        message: messages::YOU_HAVE_NOTHING_TO_PAY,
        field: ACTIDIN,
    },
    // The COBOL goes on to write the payment with the card number left in
    // `CARD-XREF-RECORD` (E-29, halt); the answer is the escalation's
    // proposal: the `NOTFND` message and no write (P-COBIL00C-025).
    Refusal {
        rows: "V-COBIL00C-009, P-COBIL00C-011, P-COBIL00C-025",
        account_id: Some("00000000006"),
        confirm: Some("Y"),
        status: StatusCode::NOT_FOUND,
        code: ErrorKind::NotFound.code(),
        message: messages::ACCOUNT_ID_NOT_FOUND,
        field: ACTIDIN,
    },
    // The seeded live master is empty: no TRAN-ID can be allocated (E-27).
    Refusal {
        rows: "V-COBIL00C-011, P-COBIL00C-005, P-COBIL00C-013",
        account_id: Some("00000000001"),
        confirm: Some("Y"),
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: ErrorKind::Unexpected.code(),
        message: messages::UNABLE_TO_LOOKUP_TRANSACTION,
        field: ACTIDIN,
    },
    Refusal {
        rows: "V-COBIL00C-011, P-COBIL00C-005, P-COBIL00C-013: lower-case y confirms",
        account_id: Some("00000000001"),
        confirm: Some("y"),
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: ErrorKind::Unexpected.code(),
        message: messages::UNABLE_TO_LOOKUP_TRANSACTION,
        field: ACTIDIN,
    },
];

/// A blank confirmation on an account with a positive balance: 200 with the
/// balance and `Confirm to make a bill payment...`, no TRAN-ID, no write.
struct Prompt {
    /// The audit rows the case proves.
    rows: &'static str,
    /// `account_id` of the body.
    account_id: &'static str,
    /// `confirm` of the body; `None` leaves the field out.
    confirm: Option<&'static str>,
    /// `ACCT-CURR-BAL` as the response's decimal string.
    balance: &'static str,
}

/// The prompts, after [`REFUSAL_SETUP`]; balances from `acctdata.txt`.
const PROMPTS: [Prompt; 5] = [
    Prompt {
        rows: "V-COBIL00C-008, P-COBIL00C-007",
        account_id: "00000000001",
        confirm: Some(""),
        balance: "194.00",
    },
    Prompt {
        rows: "V-COBIL00C-008, P-COBIL00C-007: a missing confirmation is blank",
        account_id: "00000000001",
        confirm: None,
        balance: "194.00",
    },
    Prompt {
        rows: "V-COBIL00C-008, P-COBIL00C-007: LOW-VALUES are blank",
        account_id: "00000000001",
        confirm: Some("\u{0}"),
        balance: "194.00",
    },
    Prompt {
        rows: "V-COBIL00C-008, P-COBIL00C-020: one cent is more than zero",
        account_id: "00000000007",
        confirm: Some(" "),
        balance: "0.01",
    },
    Prompt {
        rows: "V-COBIL00C-008, P-COBIL00C-011: only a confirmed payment reads the cross-reference",
        account_id: "00000000006",
        confirm: Some(""),
        balance: "218.00",
    },
];

/// A confirmed payment with one or both of its writes made to fail.
struct WriteFault {
    /// The audit rows the case proves.
    rows: &'static str,
    /// The account paid; each case pays another, so no earlier case masks it.
    account_id: &'static str,
    /// The statements that install the fault.
    faults: &'static [&'static str],
    /// The statements that remove it.
    repairs: &'static [&'static str],
    /// The audited status.
    status: StatusCode,
    /// The error code of the audited kind.
    code: &'static str,
    /// The audited message, the last one the program set.
    message: &'static str,
    /// `acct_curr_bal` afterwards.
    balance: &'static str,
    /// The payment row that persists, as TRAN-ID, amount and card number;
    /// `None` when the transaction write failed.
    persisted: Option<(&'static str, &'static str, &'static str)>,
}

/// The E-19 cases in run order, after [`LIVE_ROW`]. A failed `WRITE` still
/// reduces and rewrites the balance; a failed `REWRITE` keeps the written
/// transaction; both files are `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L1-96].
/// A TRAN-ID whose insert failed is allocated again by the next case.
const WRITE_FAULTS: [WriteFault; 5] = [
    WriteFault {
        rows: "V-COBIL00C-014, P-COBIL00C-017, P-COBIL00C-018, P-COBIL00C-024",
        account_id: "00000000002",
        faults: &[INSERT_DUPLICATE],
        repairs: &[DROP_INSERT_FAULT],
        status: StatusCode::CONFLICT,
        code: ErrorKind::Duplicate.code(),
        message: messages::TRAN_ID_ALREADY_EXIST,
        balance: "0.00",
        persisted: None,
    },
    WriteFault {
        rows: "V-COBIL00C-015, P-COBIL00C-017, P-COBIL00C-018, P-COBIL00C-024",
        account_id: "00000000005",
        faults: &[INSERT_FAILURE],
        repairs: &[DROP_INSERT_FAULT],
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: ErrorKind::Unexpected.code(),
        message: messages::UNABLE_TO_ADD_BILL_PAY_TRANSACTION,
        balance: "0.00",
        persisted: None,
    },
    WriteFault {
        rows: "V-COBIL00C-017, P-COBIL00C-016, P-COBIL00C-019, P-COBIL00C-024",
        account_id: "00000000008",
        faults: &[REWRITE_FAILURE],
        repairs: &[DROP_REWRITE_FAULT],
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: ErrorKind::Unexpected.code(),
        message: messages::UNABLE_TO_UPDATE_ACCOUNT,
        balance: "605.00",
        persisted: Some(("0000000000000301", "605.00", "8931369351894783")),
    },
    WriteFault {
        rows: "V-COBIL00C-016, P-COBIL00C-016, P-COBIL00C-019, P-COBIL00C-024",
        account_id: "00000000009",
        faults: &[REWRITE_NOT_FOUND],
        repairs: &[DROP_REWRITE_FAULT],
        status: StatusCode::NOT_FOUND,
        code: ErrorKind::NotFound.code(),
        message: messages::ACCOUNT_ID_NOT_FOUND,
        balance: "560.00",
        persisted: Some(("0000000000000302", "560.00", "9501733721429893")),
    },
    // `SEND-BILLPAY-SCREEN` does not end the task, so the rewrite's message
    // replaces the write's, and neither write persists.
    WriteFault {
        rows: "V-COBIL00C-017, P-COBIL00C-003, P-COBIL00C-019: both writes fail",
        account_id: "00000000010",
        faults: &[INSERT_FAILURE, REWRITE_FAILURE],
        repairs: &[DROP_INSERT_FAULT, DROP_REWRITE_FAULT],
        status: StatusCode::INTERNAL_SERVER_ERROR,
        code: ErrorKind::Unexpected.code(),
        message: messages::UNABLE_TO_UPDATE_ACCOUNT,
        balance: "159.00",
        persisted: None,
    },
];

/// A confirmed payment on account 1 with one of its reads made to fail by
/// renaming the relation it reads.
struct ReadFault {
    /// The audit rows the case proves.
    rows: &'static str,
    /// The rename that hides the relation.
    fault: &'static str,
    /// The rename that restores it.
    repair: &'static str,
    /// `confirm` of the body.
    confirm: &'static str,
    /// The audited message, a 500 on `ACTIDIN`.
    message: &'static str,
}

/// The read failures, in the order of the reads.
const READ_FAULTS: [ReadFault; 4] = [
    ReadFault {
        rows: "V-COBIL00C-005, P-COBIL00C-009",
        fault: "ALTER TABLE accounts RENAME TO accounts_cb00_hidden",
        repair: "ALTER TABLE accounts_cb00_hidden RENAME TO accounts",
        confirm: "Y",
        message: messages::UNABLE_TO_LOOKUP_ACCOUNT,
    },
    ReadFault {
        rows: "V-COBIL00C-005, P-COBIL00C-007, P-COBIL00C-009: the prompt reads the account too",
        fault: "ALTER TABLE accounts RENAME TO accounts_cb00_hidden",
        repair: "ALTER TABLE accounts_cb00_hidden RENAME TO accounts",
        confirm: "",
        message: messages::UNABLE_TO_LOOKUP_ACCOUNT,
    },
    // The COBOL goes on with the payment after this read too (E-29, halt);
    // the escalation's proposal stops with the message and writes nothing.
    ReadFault {
        rows: "V-COBIL00C-010, P-COBIL00C-011, P-COBIL00C-025",
        fault: "ALTER TABLE card_xref RENAME TO card_xref_cb00_hidden",
        repair: "ALTER TABLE card_xref_cb00_hidden RENAME TO card_xref",
        confirm: "Y",
        message: messages::UNABLE_TO_LOOKUP_XREF_AIX_FILE,
    },
    // The replacement reads the highest live key in one statement, in place
    // of both the `STARTBR` and the `READPREV` of the browse; the `READPREV`
    // `ENDFILE` path is unreachable after a successful `STARTBR`.
    ReadFault {
        rows: "V-COBIL00C-012, V-COBIL00C-013, P-COBIL00C-014",
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

/// Asserts that `resp` is the error answer `status` with `code` and
/// `message`, its one `fields` entry `field` carrying `message` too. A status,
/// code or message that differs names `rows`, the audit rows of the case.
fn assert_refused(
    resp: &Resp,
    rows: &str,
    status: StatusCode,
    code: &str,
    message: &str,
    field: &str,
) {
    let (actual, body, raw) = resp;
    assert_eq!(
        (*actual, body["code"].as_str(), body["message"].as_str()),
        (status, Some(code), Some(message)),
        "{rows}: status, code and message: {raw}"
    );
    assert_error(resp, status, code, message, &[(field, message)]);
}

/// Asserts that `resp` is the 200 confirmation prompt `expected`: exactly the
/// account, its balance and `Confirm to make a bill payment...`, with no
/// TRAN-ID.
fn assert_prompt(resp: &Resp, expected: &Prompt) {
    let (status, body, raw) = resp;
    let prompt = json!({
        "account_id": expected.account_id,
        "current_balance": expected.balance,
        "message": messages::CONFIRM_TO_MAKE_A_BILL_PAYMENT,
    });
    assert_eq!(
        (*status, body),
        (StatusCode::OK, &prompt),
        "{}: the prompt: {raw}",
        expected.rows
    );
}

/// Asserts that `resp` is the 201 answer of a payment on `account_id` under
/// `tran_id`: exactly the balance after it, the TRAN-ID and the composed
/// `Payment successful.  Your Transaction ID is <tran-id>.` text.
fn assert_paid(resp: &Resp, rows: &str, account_id: &str, balance: &str, tran_id: &str) {
    let (status, body, raw) = resp;
    let paid = json!({
        "account_id": account_id,
        "current_balance": balance,
        "tran_id": tran_id,
        "message": messages::payment_successful_message(tran_id),
    });
    assert_eq!(
        (*status, body),
        (StatusCode::CREATED, &paid),
        "{rows}: the payment: {raw}"
    );
}

/// The payment record under `tran_id` for `amount` on the card `card`, as
/// [`ADDED_ROWS`] reads it: type `02`, category 0002, source `POS TERM`,
/// description `BILL PAYMENT - ONLINE`, merchant 999999999 `BILL PAYMENT`
/// in `N/A`, `N/A`, and both timestamps [`PAYMENT_TS`].
fn payment_row(tran_id: &str, amount: &str, card: &str) -> Vec<Option<String>> {
    [
        tran_id,
        "02",
        "2",
        "POS TERM",
        "BILL PAYMENT - ONLINE",
        amount,
        "999999999",
        "BILL PAYMENT",
        "N/A",
        "N/A",
        card,
        PAYMENT_TS,
        PAYMENT_TS,
    ]
    .into_iter()
    .map(|value| Some(value.to_owned()))
    .collect()
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

/// `PROCESS-ENTER-KEY` up to the TRAN-ID allocation: every entry edit, read
/// refusal and confirmation prompt answers as audited, and none writes.
#[test]
fn entry_edits_refusals_and_prompts_write_nothing() {
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
                refusal.rows,
                refusal.status,
                refusal.code,
                refusal.message,
                refusal.field,
            );
        }
        for prompt in &PROMPTS {
            let body = request(Some(prompt.account_id), prompt.confirm);
            let resp = app.post(PAYMENTS, Some(&token), &body).await;
            assert_prompt(&resp, prompt);
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
/// balance reduced by it; the paid account then has nothing to pay.
#[test]
fn a_confirmed_payment_pays_the_whole_balance() {
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
        let rows = "P-COBIL00C-012, P-COBIL00C-016, P-COBIL00C-022: highest key + 1, 194.00 paid";
        assert_paid(&paid, rows, "00000000001", "0.00", "0000000000000301");
        let first = payment_row("0000000000000301", "194.00", "9680294154603697");
        assert_eq!(
            stack.fetch_text_rows(ADDED_ROWS).await,
            [first.clone()],
            "P-COBIL00C-015, P-COBIL00C-026: the payment record"
        );

        let again = app.post(PAYMENTS, Some(&token), &body).await;
        assert_refused(
            &again,
            "V-COBIL00C-007, P-COBIL00C-010: the account just paid",
            StatusCode::BAD_REQUEST,
            ErrorKind::Invalid.code(),
            messages::YOU_HAVE_NOTHING_TO_PAY,
            ACTIDIN,
        );

        // 1234567890.12 loses its high-order digit in the `S9(09)V99`
        // `TRAN-AMT`, and the `S9(10)V99` balance keeps the difference.
        let body = request(Some("00000000003"), Some("y"));
        let truncated = app.post(PAYMENTS, Some(&token), &body).await;
        let rows =
            "P-COBIL00C-005, P-COBIL00C-021, P-COBIL00C-022: 1234567890.12 pays 234567890.12";
        assert_paid(
            &truncated,
            rows,
            "00000000003",
            "1000000000.00",
            "0000000000000302",
        );
        let second = payment_row("0000000000000302", "234567890.12", "3999169246375885");
        assert_eq!(
            stack.fetch_text_rows(ADDED_ROWS).await,
            [first, second],
            "P-COBIL00C-015, P-COBIL00C-021: the payment records"
        );

        set_balance(&mut balances, "00000000001", "0.00");
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
/// fails, and the answer is the last message the program set.
#[test]
fn a_failed_write_keeps_the_other_write() {
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROW).await;
        stack.exec_sql(FAULT_FUNCTIONS).await;
        let mut balances = stack.fetch_text_rows(BALANCES).await;
        let mut persisted = Vec::new();
        let token = app.user_token().await;
        for case in &WRITE_FAULTS {
            for fault in case.faults {
                stack.exec_sql(fault).await;
            }
            let body = request(Some(case.account_id), Some("Y"));
            let resp = app.post(PAYMENTS, Some(&token), &body).await;
            for repair in case.repairs {
                stack.exec_sql(repair).await;
            }
            assert_refused(
                &resp,
                case.rows,
                case.status,
                case.code,
                case.message,
                ACTIDIN,
            );
            set_balance(&mut balances, case.account_id, case.balance);
            assert_eq!(
                stack.fetch_text_rows(BALANCES).await,
                balances,
                "{}: the balances",
                case.rows
            );
            if let Some((tran_id, amount, card)) = case.persisted {
                persisted.push(payment_row(tran_id, amount, card));
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
            assert_refused(
                &resp,
                case.rows,
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorKind::Unexpected.code(),
                case.message,
                ACTIDIN,
            );
        }
        assert_eq!(
            written(&app).await,
            before,
            "the failed reads wrote nothing"
        );
        let body = request(Some("00000000001"), Some("Y"));
        let paid = app.post(PAYMENTS, Some(&token), &body).await;
        let rows = "P-COBIL00C-012, P-COBIL00C-016: the restored relations serve the payment";
        assert_paid(&paid, rows, "00000000001", "0.00", "0000000000000301");
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
    let app = TestApp::start_with_clock(CLOCK);
    carddemo_testkit::block_on(async {
        let stack = app.stack();
        stack.exec_sql(LIVE_ROW).await;
        stack.exec_sql(MECHANICS_SETUP).await;
        let token = app.user_token().await;
        let before = written(&app).await;
        preselected_account(&app, &token, &examples).await;
        received_map_and_keys(&app, &token).await;
        without_a_session(&app).await;
        assert_eq!(
            written(&app).await,
            before,
            "P-COBIL00C-001, P-COBIL00C-002, P-COBIL00C-027, P-COBIL00C-028: nothing written"
        );
        initialized_form(&app, &token).await;
    });
}

/// P-COBIL00C-001: a first entry with a preselected account copies it to
/// `ACTIDINI` and runs `PROCESS-ENTER-KEY` with `CONFIRMI` still
/// `LOW-VALUES`, so the body holding the account alone answers the prompt,
/// with the balance the `GET /accounts/{id}` preview shows, or the
/// nothing-to-pay refusal. P-COBIL00C-023: the prompt carries `ACCT-CURR-BAL`
/// exactly, and `WS-CURR-BAL` edits it to the audited screen text.
async fn preselected_account(app: &TestApp, token: &str, examples: &[(Decimal, String)]) {
    for (account_id, balance) in [("00000000001", "194.00"), ("00000000003", "1940.00")] {
        let resp = app
            .post(PAYMENTS, Some(token), &request(Some(account_id), None))
            .await;
        let prompt = Prompt {
            rows: "P-COBIL00C-001: the preselected account alone",
            account_id,
            confirm: None,
            balance,
        };
        assert_prompt(&resp, &prompt);
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
        "P-COBIL00C-001: a preselected account with nothing to pay",
        StatusCode::BAD_REQUEST,
        ErrorKind::Invalid.code(),
        messages::YOU_HAVE_NOTHING_TO_PAY,
        ACTIDIN,
    );
}

/// P-COBIL00C-028: an empty body, the received map of a MAPFAIL, still runs
/// `PROCESS-ENTER-KEY`, whose account edit answers. P-COBIL00C-002: a body
/// member naming an AID key is no input, so every key answers as ENTER does
/// with the same body, never with the invalid-key text, and PF4 clears
/// nothing on the server. P-COBIL00C-029: the error body carries no header
/// field.
async fn received_map_and_keys(app: &TestApp, token: &str) {
    let empty = app.post(PAYMENTS, Some(token), &json!({})).await;
    assert_refused(
        &empty,
        "P-COBIL00C-028: an empty body",
        StatusCode::BAD_REQUEST,
        ErrorKind::Empty.code(),
        messages::ACCT_ID_CAN_NOT_BE_EMPTY,
        ACTIDIN,
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
/// same account answered before. P-COBIL00C-029: the payment carries no
/// header field.
async fn initialized_form(app: &TestApp, token: &str) {
    for account_id in ["00000000001", "99999999999"] {
        let resp = app
            .post(PAYMENTS, Some(token), &request(Some(account_id), Some("N")))
            .await;
        assert_refused(
            &resp,
            "P-COBIL00C-031: N clears the form",
            StatusCode::BAD_REQUEST,
            ErrorKind::Invalid.code(),
            NO_MESSAGE,
            ACTIDIN,
        );
    }
    let body = request(Some("00000000001"), Some(""));
    let prompt = app.post(PAYMENTS, Some(token), &body).await;
    assert_eq!(
        prompt.1["message"],
        Value::from(messages::CONFIRM_TO_MAKE_A_BILL_PAYMENT),
        "P-COBIL00C-031: the prompt before the payment: {}",
        prompt.2
    );
    let body = request(Some("00000000001"), Some("Y"));
    let paid = app.post(PAYMENTS, Some(token), &body).await;
    assert_paid(
        &paid,
        "P-COBIL00C-031: the success text alone",
        "00000000001",
        "0.00",
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
