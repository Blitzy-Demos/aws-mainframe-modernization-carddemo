//! `CBTRN02C`, the program of the `POSTTRAN` job: posting the daily
//! transaction file [app/cbl/CBTRN02C.cbl] [app/jcl/POSTTRAN.jcl].
//!
//! For each `DALYTRAN` record in file order the main loop counts the record,
//! clears the reason, validates and then posts it, or counts and writes a
//! reject (P-CBTRN02C-006) [app/cbl/CBTRN02C.cbl:L202-219]:
//!
//! - [`validate`] is `1500-VALIDATE-TRAN`: the cross-reference read (reason
//!   100), the account read (101), the `WS-TEMP-BAL` limit check (102) and the
//!   expiry test (103), which replaces a 102 (E-15).
//! - [`post`] is `2000-POST-TRANSACTION`: the category balance update, the
//!   account update and the transaction write, in that order.
//! - [`reject_record`] is the 430-byte `REJECT-RECORD` that
//!   `2500-WRITE-REJECT-REC` writes to `DALYREJS(+1)`.
//! - [`PostingCounts`] holds the run counters and the `RETURN-CODE`.
//!
//! Every account read comes from the store, so the cycle totals of a record
//! posted earlier in the run take part in the checks of the next one, as the
//! COBOL rereads `ACCTFILE` for each record.
//!
//! # Writes and failures
//!
//! The three writes of a posted record each run under their own savepoint in
//! the adapter. Every one of them persists when a later one fails: the VSAM
//! files are `RECOVERY(NONE)` [app/csd/CARDDEMO.CSD:L1-96] (P-CBTRN02C-015,
//! P-CBTRN02C-018, P-CBTRN02C-019). A failed write abends the run with a
//! [`PostingAbend`]; `batch posttran` commits the record's completed writes,
//! prints the abend's `DISPLAY` lines and `ABEND 999`, and exits 16.
//!
//! The reward accrual that `batch posttran` adds to each posted record, in the
//! same database transaction, is not part of `CBTRN02C` and is not called here.

use std::cmp::Ordering;
use std::fmt;

use crate::clock::{Clock, db2_timestamp};
use crate::entities::{
    Account, CardXref, DailyTran, NewTransaction, TranCatBalance, TranCatBalanceKey, Transaction,
};
use crate::messages::{
    ABENDING_PROGRAM, ACCOUNT_RECORD_NOT_FOUND, ERROR_CLOSING_DAILY_REJECTS_FILE,
    ERROR_OPENING_DALY_REJECTS_FILE, ERROR_OPENING_DALYTRAN, ERROR_OPENING_TRANSACTION_FILE,
    ERROR_READING_DALYTRAN_FILE, ERROR_READING_TRANSACTION_BALANCE_FILE,
    ERROR_REWRITING_TRANSACTION_BALANCE_FILE, ERROR_WRITING_TO_REJECTS_FILE,
    ERROR_WRITING_TO_TRANSACTION_FILE, ERROR_WRITING_TRANSACTION_BALANCE_FILE,
    INVALID_CARD_NUMBER_FOUND, OVERLIMIT_TRANSACTION, TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
    UNEXPECTED_ERROR, file_status_message,
};
use crate::numeric::{Alnum, Money};
use crate::ports::{
    AccountReader, AccountWriter, PortError, TranCatBalanceStore, TransactionWriter, XrefReader,
};

/// `REJECT-TRAN-DATA PIC X(350)`, the `DALYTRAN` record as read —
/// P-CBTRN02C-001 [app/cbl/CBTRN02C.cbl:L177].
pub const REJECT_TRAN_DATA_LEN: usize = 350;

/// `WS-VALIDATION-FAIL-REASON PIC 9(04)` — P-CBTRN02C-002
/// [app/cbl/CBTRN02C.cbl:L181].
const REASON_DIGITS: usize = 4;

/// `WS-VALIDATION-FAIL-REASON-DESC PIC X(76)` — P-CBTRN02C-003
/// [app/cbl/CBTRN02C.cbl:L182].
const REASON_DESC_LEN: usize = 76;

/// `REJECT-RECORD`, the 350 input bytes and the 80-byte `VALIDATION-TRAILER`:
/// the `LRECL=430` of `DALYREJS` — P-CBTRN02C-012 [app/cbl/CBTRN02C.cbl:L176-182]
/// [app/jcl/POSTTRAN.jcl:L34-38].
pub const REJECT_RECORD_LEN: usize = REJECT_TRAN_DATA_LEN + REASON_DIGITS + REASON_DESC_LEN;

/// The values a `9(04)` field holds: a numeric `MOVE` keeps the low-order
/// four digits.
const REASON_MODULUS: u16 = 10_000;

/// Reject reason 100, the card number has no cross-reference — P-CBTRN02C-008.
pub const REASON_INVALID_CARD: u16 = 100;

/// Reject reason 101, the cross-referenced account is missing — P-CBTRN02C-009.
pub const REASON_ACCOUNT_NOT_FOUND: u16 = 101;

/// Reject reason 102, `WS-TEMP-BAL` exceeds the credit limit — P-CBTRN02C-010.
pub const REASON_OVERLIMIT: u16 = 102;

/// Reject reason 103, the transaction is dated after the account expiry —
/// P-CBTRN02C-011.
pub const REASON_AFTER_EXPIRATION: u16 = 103;

/// `DALYTRAN-ORIG-TS (1:10)`, the date part the expiry test compares —
/// P-CBTRN02C-011 [app/cbl/CBTRN02C.cbl:L414].
const ORIG_DATE_LEN: usize = 10;

/// `MOVE 4 TO RETURN-CODE` when any record was rejected — P-CBTRN02C-020
/// [app/cbl/CBTRN02C.cbl:L229-231].
const RETURN_CODE_REJECTS: u8 = 4;

/// The `RETURN-CODE` of a run without rejects.
const RETURN_CODE_CLEAN: u8 = 0;

/// The VSAM file status of an `INVALID KEY` read or rewrite: record not found.
const STATUS_NOT_FOUND: &str = "23";

/// The VSAM file status of a `WRITE` whose key exists: duplicate key.
const STATUS_DUPLICATE: &str = "22";

/// The status of `OPEN OUTPUT` on a loaded KSDS defined without `REUSE` —
/// P-CBTRN02C-005 (E-32).
const STATUS_NOT_REUSABLE: &str = "37";

const DALYTRAN_OPEN: &str = "0000-DALYTRAN-OPEN";
const TRANFILE_OPEN: &str = "0100-TRANFILE-OPEN";
const DALYREJS_OPEN: &str = "0300-DALYREJS-OPEN";
const DALYTRAN_GET_NEXT: &str = "1000-DALYTRAN-GET-NEXT";
const LOOKUP_XREF: &str = "1500-A-LOOKUP-XREF";
const LOOKUP_ACCT: &str = "1500-B-LOOKUP-ACCT";
const WRITE_REJECT_REC: &str = "2500-WRITE-REJECT-REC";
const UPDATE_TCATBAL: &str = "2700-UPDATE-TCATBAL";
const CREATE_TCATBAL_REC: &str = "2700-A-CREATE-TCATBAL-REC";
const UPDATE_TCATBAL_REC: &str = "2700-B-UPDATE-TCATBAL-REC";
const UPDATE_ACCOUNT_REC: &str = "2800-UPDATE-ACCOUNT-REC";
const WRITE_TRANSACTION_FILE: &str = "2900-WRITE-TRANSACTION-FILE";
const DALYREJS_CLOSE: &str = "9300-DALYREJS-CLOSE";

/// CBTRN02C 1500-VALIDATE-TRAN — P-CBTRN02C-006: the outcome for one daily
/// record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Validation {
    /// `WS-VALIDATION-FAIL-REASON` stayed 0: the record is posted with the
    /// cross-reference and the account the validation read.
    Accepted {
        /// The `CARD-XREF-RECORD` of `DALYTRAN-CARD-NUM`.
        xref: CardXref,
        /// The `ACCOUNT-RECORD` of `XREF-ACCT-ID`, as read.
        account: Account,
    },
    /// The record is counted as rejected and written to `DALYREJS`.
    Rejected {
        /// `WS-VALIDATION-FAIL-REASON`: 100, 101, 102 or 103.
        reason: u16,
        /// `WS-VALIDATION-FAIL-REASON-DESC`, before its `X(76)` padding.
        description: &'static str,
    },
}

/// CBTRN02C 2000-POST-TRANSACTION — P-CBTRN02C-013: a posted record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    /// The `TRAN-RECORD` written to the live master, with the row identity
    /// its reward accrual references.
    pub transaction: Transaction,
    /// The account `REWRITE` took its `INVALID KEY` path: reason 109 was set,
    /// but the record is neither counted nor written to `DALYREJS`, and the
    /// transaction was still written (P-CBTRN02C-018, P-CBTRN02C-218, E-15).
    /// The category balance update stands and the account is unchanged.
    pub account_rewrite_failed: bool,
}

/// `WS-COUNTERS`: `WS-TRANSACTION-COUNT` and `WS-REJECT-COUNT`, both
/// `PIC 9(09)` [app/cbl/CBTRN02C.cbl:L184-186] — P-CBTRN02C-006,
/// P-CBTRN02C-020.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PostingCounts {
    /// Records read from `DALYTRAN`.
    pub read: u64,
    /// Records rejected and written to `DALYREJS`.
    pub rejected: u64,
}

impl PostingCounts {
    /// CBTRN02C end of job — P-CBTRN02C-020: `RETURN-CODE` 4 when any record
    /// was rejected, else 0 [app/cbl/CBTRN02C.cbl:L229-231].
    #[must_use]
    pub fn return_code(&self) -> u8 {
        if self.rejected > 0 {
            RETURN_CODE_REJECTS
        } else {
            RETURN_CODE_CLEAN
        }
    }
}

/// CBTRN02C 9999-ABEND-PROGRAM — P-CBTRN02C-021: the `DISPLAY` lines of the
/// failing paragraph, then `CEE3ABD` with code 999
/// [app/cbl/CBTRN02C.cbl:L707-712].
///
/// Its `Display` is those lines, in the COBOL order, one per line: the
/// paragraph's text, the `9910-DISPLAY-IO-STATUS` line
/// `FILE STATUS IS: NNNN`+`IO-STATUS-04` [app/cbl/CBTRN02C.cbl:L714-727], and
/// `ABENDING PROGRAM`. `batch posttran` prints them, then `ABEND 999`, and
/// exits 16.
///
/// The status line is shown only for a status the store reports. A port
/// failure that has no VSAM file status (a lost connection, a lock timeout)
/// and the job-level file errors carry none, and no status is made up for
/// them, so their abend shows the text and `ABENDING PROGRAM` only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub struct PostingAbend {
    /// The paragraph that abended, for example `2900-WRITE-TRANSACTION-FILE`.
    pub paragraph: &'static str,
    /// The paragraph's `DISPLAY` text before `9910-DISPLAY-IO-STATUS`.
    pub display: &'static str,
    /// The two-character file status `9910-DISPLAY-IO-STATUS` formats: `23`
    /// for a missing key, `22` for a duplicate key, `37` for the non-empty
    /// transaction master; empty when the failure has no file status.
    pub file_status: String,
    /// The port failure behind the abend, for the job's log event. Its text is
    /// adapter detail and is never part of the displayed lines.
    #[source]
    pub cause: Option<PortError>,
}

impl PostingAbend {
    /// CBTRN02C 0000-DALYTRAN-OPEN — P-CBTRN02C-221: the daily transaction
    /// input cannot be opened [app/cbl/CBTRN02C.cbl:L236-252].
    #[must_use]
    pub fn dalytran_open() -> Self {
        Self::job_level(DALYTRAN_OPEN, ERROR_OPENING_DALYTRAN, "")
    }

    /// CBTRN02C 0100-TRANFILE-OPEN — P-CBTRN02C-005, P-CBTRN02C-219: `OPEN
    /// OUTPUT` of a transaction master that already holds records fails with
    /// status 37, so posting runs only against an empty live master (E-32)
    /// [app/cbl/CBTRN02C.cbl:L254-270].
    #[must_use]
    pub fn tranfile_open_not_empty() -> Self {
        Self::job_level(
            TRANFILE_OPEN,
            ERROR_OPENING_TRANSACTION_FILE,
            STATUS_NOT_REUSABLE,
        )
    }

    /// CBTRN02C 0300-DALYREJS-OPEN — P-CBTRN02C-223: the new `DALYREJS`
    /// generation cannot be opened [app/cbl/CBTRN02C.cbl:L291-307].
    #[must_use]
    pub fn dalyrejs_open() -> Self {
        Self::job_level(DALYREJS_OPEN, ERROR_OPENING_DALY_REJECTS_FILE, "")
    }

    /// CBTRN02C 1000-DALYTRAN-GET-NEXT — P-CBTRN02C-007: a read of the daily
    /// transaction input fails other than at its end
    /// [app/cbl/CBTRN02C.cbl:L345-369].
    #[must_use]
    pub fn dalytran_read() -> Self {
        Self::job_level(DALYTRAN_GET_NEXT, ERROR_READING_DALYTRAN_FILE, "")
    }

    /// CBTRN02C 2500-WRITE-REJECT-REC — P-CBTRN02C-012: the reject record
    /// cannot be written to `DALYREJS` [app/cbl/CBTRN02C.cbl:L446-465].
    #[must_use]
    pub fn reject_write() -> Self {
        Self::job_level(WRITE_REJECT_REC, ERROR_WRITING_TO_REJECTS_FILE, "")
    }

    /// CBTRN02C 9300-DALYREJS-CLOSE — P-CBTRN02C-020: the `DALYREJS`
    /// generation cannot be closed at the end of the run
    /// [app/cbl/CBTRN02C.cbl:L637-653].
    ///
    /// The paragraph moves `XREFFILE-STATUS`, not `DALYREJS-STATUS`, to
    /// `IO-STATUS` before `9910-DISPLAY-IO-STATUS` [app/cbl/CBTRN02C.cbl:L649].
    /// The audit row records no status for the path, so none is shown.
    #[must_use]
    pub fn dalyrejs_close() -> Self {
        Self::job_level(DALYREJS_CLOSE, ERROR_CLOSING_DAILY_REJECTS_FILE, "")
    }

    /// The `FILE STATUS IS: NNNN…` line of [`PostingAbend::file_status`], or
    /// `None` when the abend carries no two-character status.
    #[must_use]
    pub fn file_status_line(&self) -> Option<String> {
        <[u8; 2]>::try_from(self.file_status.as_bytes())
            .ok()
            .map(file_status_message)
    }

    /// An abend on a file the batch adapter handles itself, without a port.
    fn job_level(paragraph: &'static str, display: &'static str, file_status: &str) -> Self {
        Self {
            paragraph,
            display,
            file_status: file_status.to_owned(),
            cause: None,
        }
    }

    /// The abend of `paragraph` after the port failure `error`, displaying
    /// `display` and the file status the failure stands for.
    fn from_port(paragraph: &'static str, display: &'static str, error: PortError) -> Self {
        Self {
            paragraph,
            display,
            file_status: vsam_status(&error).to_owned(),
            cause: Some(error),
        }
    }

    /// A store failure other than `NOTFND` on the cross-reference read, the
    /// account read or the account rewrite (E-77).
    ///
    /// Those COBOL statements code only `INVALID KEY` and test no file
    /// status, so the COBOL would go on with the record area of the previous
    /// read. The replacement does not post against stale data: it stops the
    /// run here. No COBOL text exists for the path, so the display is the
    /// unexpected-error text.
    fn unaudited(paragraph: &'static str, error: PortError) -> Self {
        Self::from_port(paragraph, UNEXPECTED_ERROR, error)
    }
}

impl fmt::Display for PostingAbend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "{}", self.display)?;
        if let Some(status_line) = self.file_status_line() {
            writeln!(formatter, "{status_line}")?;
        }
        formatter.write_str(ABENDING_PROGRAM)
    }
}

/// The VSAM file status a port outcome stands for: `23` for `NOTFND`, `22` for
/// a duplicate key, and none for the failures that have no VSAM counterpart.
fn vsam_status(error: &PortError) -> &'static str {
    match error {
        PortError::NotFound => STATUS_NOT_FOUND,
        PortError::Duplicate => STATUS_DUPLICATE,
        PortError::Restricted | PortError::Contention | PortError::Unavailable(_) => "",
    }
}

/// CBTRN02C 1500-VALIDATE-TRAN — P-CBTRN02C-006, P-CBTRN02C-008,
/// P-CBTRN02C-009, P-CBTRN02C-010, P-CBTRN02C-011, P-CBTRN02C-217.
///
/// `1500-A-LOOKUP-XREF` reads the cross-reference of `DALYTRAN-CARD-NUM`; a
/// missing one rejects the record with reason 100 and skips the account read
/// [app/cbl/CBTRN02C.cbl:L370-391]. `1500-B-LOOKUP-ACCT` then reads the
/// account of `XREF-ACCT-ID`; a missing one is reason 101
/// [app/cbl/CBTRN02C.cbl:L393-399]. A found account then goes through the limit
/// check (102) and the expiry check (103).
///
/// # Errors
///
/// A [`PostingAbend`] of `1500-A-LOOKUP-XREF` or `1500-B-LOOKUP-ACCT` when the
/// read fails other than with [`PortError::NotFound`] (E-77).
pub async fn validate<S: XrefReader + AccountReader>(
    store: &mut S,
    tran: &DailyTran,
) -> Result<Validation, PostingAbend> {
    let xref = match store.read_xref_by_card(tran.dalytran_card_num).await {
        Ok(xref) => xref,
        Err(PortError::NotFound) => {
            return Ok(Validation::Rejected {
                reason: REASON_INVALID_CARD,
                description: INVALID_CARD_NUMBER_FOUND,
            });
        }
        Err(error) => return Err(PostingAbend::unaudited(LOOKUP_XREF, error)),
    };
    let account = match store.read_account(xref.xref_acct_id).await {
        Ok(account) => account,
        Err(PortError::NotFound) => {
            return Ok(Validation::Rejected {
                reason: REASON_ACCOUNT_NOT_FOUND,
                description: ACCOUNT_RECORD_NOT_FOUND,
            });
        }
        Err(error) => return Err(PostingAbend::unaudited(LOOKUP_ACCT, error)),
    };
    Ok(match account_check(&account, tran) {
        Some((reason, description)) => Validation::Rejected {
            reason,
            description,
        },
        None => Validation::Accepted { xref, account },
    })
}

/// CBTRN02C 1500-B-LOOKUP-ACCT, `NOT INVALID KEY` — P-CBTRN02C-010,
/// P-CBTRN02C-011, P-CBTRN02C-217 [app/cbl/CBTRN02C.cbl:L400-420].
///
/// The limit check sets 102 when `ACCT-CREDIT-LIMIT >= WS-TEMP-BAL` fails.
/// The expiry check runs whatever the limit check found and sets 103 when
/// `ACCT-EXPIRAION-DATE >= DALYTRAN-ORIG-TS (1:10)` fails, an alphanumeric
/// comparison; a 103 therefore replaces a 102 (E-15). `None` is reason 0.
fn account_check(account: &Account, tran: &DailyTran) -> Option<(u16, &'static str)> {
    let mut failure = None;
    if account.acct_credit_limit < ws_temp_bal(account, tran) {
        failure = Some((REASON_OVERLIMIT, OVERLIMIT_TRANSACTION));
    }
    let orig_date = tran.dalytran_orig_ts.prefix(ORIG_DATE_LEN);
    if account.acct_expiraion_date.cobol_cmp(orig_date) == Ordering::Less {
        failure = Some((
            REASON_AFTER_EXPIRATION,
            TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
        ));
    }
    failure
}

/// CBTRN02C 1500-B-LOOKUP-ACCT `COMPUTE WS-TEMP-BAL` — P-CBTRN02C-010,
/// P-CBTRN02C-022, P-CBTRN02C-023 [app/cbl/CBTRN02C.cbl:L403-405].
///
/// `ACCT-CURR-CYC-CREDIT - ACCT-CURR-CYC-DEBIT + DALYTRAN-AMT` into
/// `WS-TEMP-BAL S9(09)V99` [app/cbl/CBTRN02C.cbl:L187], narrower than its
/// `S9(10)V99` operands: the store keeps the low-order nine integer digits,
/// which can change the 102 decision. The cycle debit holds negative amounts
/// as negative values, so subtracting it adds their magnitude.
fn ws_temp_bal(account: &Account, tran: &DailyTran) -> Money<9> {
    (account.acct_curr_cyc_credit - account.acct_curr_cyc_debit + tran.dalytran_amt).store::<9>()
}

/// CBTRN02C 2000-POST-TRANSACTION — P-CBTRN02C-013, P-CBTRN02C-014,
/// P-CBTRN02C-220 [app/cbl/CBTRN02C.cbl:L424-444].
///
/// Moves the daily fields to `TRAN-RECORD`, stamps `TRAN-PROC-TS` with
/// `Z-GET-DB2-FORMAT-TIMESTAMP` of `clock`, then performs
/// `2700-UPDATE-TCATBAL`, `2800-UPDATE-ACCOUNT-REC` and
/// `2900-WRITE-TRANSACTION-FILE` in that order. `xref` and `account` are the
/// records [`validate`] accepted the daily record with.
///
/// # Errors
///
/// A [`PostingAbend`] when a category balance read, write or rewrite fails,
/// when the transaction write fails, or when the account rewrite fails other
/// than with [`PortError::NotFound`] (E-77). The writes completed before the
/// failure stay in place (P-CBTRN02C-015, P-CBTRN02C-018, P-CBTRN02C-019).
pub async fn post<S: TranCatBalanceStore + AccountWriter + TransactionWriter>(
    store: &mut S,
    tran: &DailyTran,
    xref: &CardXref,
    account: Account,
    clock: &impl Clock,
) -> Result<Posted, PostingAbend> {
    let record = tran.to_new_transaction(Alnum::move_from(&db2_timestamp(clock.now())));
    update_tcatbal(store, tran, xref).await?;
    let account_rewrite_failed = update_account(store, tran.dalytran_amt, account).await?;
    let transaction = write_transaction_file(store, record).await?;
    Ok(Posted {
        transaction,
        account_rewrite_failed,
    })
}

/// CBTRN02C 2700-UPDATE-TCATBAL — P-CBTRN02C-015
/// [app/cbl/CBTRN02C.cbl:L467-501].
///
/// Reads the balance of the cross-referenced account, the transaction type and
/// the category: a found record is updated (`2700-B`), a missing one, status
/// 23, is created (`2700-A`), and any other failure abends.
async fn update_tcatbal<S: TranCatBalanceStore>(
    store: &mut S,
    tran: &DailyTran,
    xref: &CardXref,
) -> Result<(), PostingAbend> {
    let key = TranCatBalanceKey {
        trancat_acct_id: xref.xref_acct_id,
        trancat_type_cd: tran.dalytran_type_cd,
        trancat_cd: tran.dalytran_cat_cd,
    };
    match store.read_tcatbal(key.clone()).await {
        Ok(existing) => update_tcatbal_rec(store, existing, tran.dalytran_amt).await,
        Err(PortError::NotFound) => create_tcatbal_rec(store, key, tran.dalytran_amt).await,
        Err(error) => Err(PostingAbend::from_port(
            UPDATE_TCATBAL,
            ERROR_READING_TRANSACTION_BALANCE_FILE,
            error,
        )),
    }
}

/// CBTRN02C 2700-A-CREATE-TCATBAL-REC — P-CBTRN02C-016, P-CBTRN02C-024
/// [app/cbl/CBTRN02C.cbl:L503-524].
///
/// `INITIALIZE` zeroes the balance, `ADD DALYTRAN-AMT TO TRAN-CAT-BAL` stores
/// into `S9(09)V99`, and the record is written under `key`.
async fn create_tcatbal_rec<S: TranCatBalanceStore>(
    store: &mut S,
    key: TranCatBalanceKey,
    amount: Money<9>,
) -> Result<(), PostingAbend> {
    let record = TranCatBalance {
        key,
        tran_cat_bal: (Money::<9>::ZERO + amount).store::<9>(),
    };
    store.write_tcatbal(record).await.map_err(|error| {
        PostingAbend::from_port(
            CREATE_TCATBAL_REC,
            ERROR_WRITING_TRANSACTION_BALANCE_FILE,
            error,
        )
    })
}

/// CBTRN02C 2700-B-UPDATE-TCATBAL-REC — P-CBTRN02C-017, P-CBTRN02C-025
/// [app/cbl/CBTRN02C.cbl:L526-542].
///
/// `ADD DALYTRAN-AMT TO TRAN-CAT-BAL` stores into `S9(09)V99`, keeping the
/// low-order nine integer digits, and the record is rewritten.
async fn update_tcatbal_rec<S: TranCatBalanceStore>(
    store: &mut S,
    mut record: TranCatBalance,
    amount: Money<9>,
) -> Result<(), PostingAbend> {
    record.tran_cat_bal = (record.tran_cat_bal + amount).store::<9>();
    store.rewrite_tcatbal(record).await.map_err(|error| {
        PostingAbend::from_port(
            UPDATE_TCATBAL_REC,
            ERROR_REWRITING_TRANSACTION_BALANCE_FILE,
            error,
        )
    })
}

/// CBTRN02C 2800-UPDATE-ACCOUNT-REC — P-CBTRN02C-018, P-CBTRN02C-026,
/// P-CBTRN02C-226, P-CBTRN02C-227, P-CBTRN02C-218
/// [app/cbl/CBTRN02C.cbl:L545-560].
///
/// Adds `amount` to `ACCT-CURR-BAL`, and to `ACCT-CURR-CYC-CREDIT` when it is
/// zero or more or to `ACCT-CURR-CYC-DEBIT` when it is negative, each stored
/// into `S9(10)V99`, then rewrites the account. Returns whether the rewrite
/// took its `INVALID KEY` path: the COBOL moves reason 109 there, which
/// nothing reads afterwards, so posting goes on (E-15).
async fn update_account<S: AccountWriter>(
    store: &mut S,
    amount: Money<9>,
    mut account: Account,
) -> Result<bool, PostingAbend> {
    account.acct_curr_bal = (account.acct_curr_bal + amount).store::<10>();
    if amount.is_negative() {
        account.acct_curr_cyc_debit = (account.acct_curr_cyc_debit + amount).store::<10>();
    } else {
        account.acct_curr_cyc_credit = (account.acct_curr_cyc_credit + amount).store::<10>();
    }
    match store.rewrite_account(account).await {
        Ok(()) => Ok(false),
        Err(PortError::NotFound) => Ok(true),
        Err(error) => Err(PostingAbend::unaudited(UPDATE_ACCOUNT_REC, error)),
    }
}

/// CBTRN02C 2900-WRITE-TRANSACTION-FILE — P-CBTRN02C-019, P-CBTRN02C-216
/// [app/cbl/CBTRN02C.cbl:L562-579].
///
/// Writes `record` to the live transaction master; any failure, a duplicate
/// TRAN-ID (status 22) included, abends.
async fn write_transaction_file<S: TransactionWriter>(
    store: &mut S,
    record: NewTransaction,
) -> Result<Transaction, PostingAbend> {
    match store.write_transaction(record.clone()).await {
        Ok(row_id) => Ok(Transaction::from_new(row_id, record)),
        Err(error) => Err(PostingAbend::from_port(
            WRITE_TRANSACTION_FILE,
            ERROR_WRITING_TO_TRANSACTION_FILE,
            error,
        )),
    }
}

/// CBTRN02C 2500-WRITE-REJECT-REC — P-CBTRN02C-001, P-CBTRN02C-002,
/// P-CBTRN02C-003, P-CBTRN02C-012 [app/cbl/CBTRN02C.cbl:L446-451].
///
/// The 430-byte `REJECT-RECORD`: the 350 bytes of the daily record as read
/// (`raw`), `reason` as the four `9(04)` digits, zero-padded and keeping the
/// low-order four, and `description` moved into `X(76)`, padded with spaces or
/// cut on the right.
#[must_use]
pub fn reject_record(
    raw: &[u8; REJECT_TRAN_DATA_LEN],
    reason: u16,
    description: &str,
) -> [u8; REJECT_RECORD_LEN] {
    let reason_field = Alnum::<REASON_DIGITS>::move_from(&format!(
        "{:0width$}",
        reason % REASON_MODULUS,
        width = REASON_DIGITS
    ));
    let description_field = Alnum::<REASON_DESC_LEN>::move_from(description);
    let mut record = [b' '; REJECT_RECORD_LEN];
    let (data, trailer) = record.split_at_mut(REJECT_TRAN_DATA_LEN);
    data.copy_from_slice(raw);
    let (reason_bytes, description_bytes) = trailer.split_at_mut(REASON_DIGITS);
    reason_bytes.copy_from_slice(reason_field.as_bytes());
    description_bytes.copy_from_slice(description_field.as_bytes());
    record
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use chrono::{TimeZone, Utc};

    use super::{
        Posted, PostingAbend, PostingCounts, REASON_ACCOUNT_NOT_FOUND, REASON_AFTER_EXPIRATION,
        REASON_INVALID_CARD, REASON_OVERLIMIT, REJECT_RECORD_LEN, REJECT_TRAN_DATA_LEN, Validation,
        post, reject_record, validate, ws_temp_bal,
    };
    use crate::clock::FixedClock;
    use crate::entities::{
        Account, CardXref, DailyTran, NewTransaction, TranCatBalance, TranCatBalanceKey,
        Transaction,
    };
    use crate::messages::{
        ABENDING_PROGRAM, ACCOUNT_RECORD_NOT_FOUND, ERROR_CLOSING_DAILY_REJECTS_FILE,
        ERROR_OPENING_DALY_REJECTS_FILE, ERROR_OPENING_DALYTRAN, ERROR_OPENING_TRANSACTION_FILE,
        ERROR_READING_DALYTRAN_FILE, ERROR_READING_TRANSACTION_BALANCE_FILE,
        ERROR_REWRITING_TRANSACTION_BALANCE_FILE, ERROR_WRITING_TO_REJECTS_FILE,
        ERROR_WRITING_TO_TRANSACTION_FILE, ERROR_WRITING_TRANSACTION_BALANCE_FILE,
        INVALID_CARD_NUMBER_FOUND, OVERLIMIT_TRANSACTION,
        TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION, UNEXPECTED_ERROR,
    };
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money,
        TranCategoryCode, TranId, TranTypeCode, TransactionRowId,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// The card cross-referenced to fixture account 1 [app/data/ASCII/cardxref.txt:L49].
    const CARD_1: &str = "9680294154603697";

    /// A card number no cross-reference holds.
    const UNKNOWN_CARD: &str = "0000000000000000";

    /// The origination timestamp of every fixture daily record.
    const ORIG_TS: &str = "2022-06-10 19:27:53.000000";

    /// The processing timestamp the parity tests pin (P-CBTRN02C-216,
    /// P-CBTRN02C-220).
    const PINNED_PROC_TS: &str = "2022-07-01-12.00.00.000000";

    fn money9(cents: i64) -> Money<9> {
        Money::from_cents(cents).expect("the amount fits S9(09)V99")
    }

    fn money10(cents: i64) -> Money<10> {
        Money::from_cents(cents).expect("the amount fits S9(10)V99")
    }

    fn account_id(id: u64) -> AccountId {
        AccountId::new(id).expect("the ID fits 9(11)")
    }

    fn category(code: u16) -> TranCategoryCode {
        TranCategoryCode::new(code).expect("the code fits 9(04)")
    }

    /// The clock `CARDDEMO_CLOCK_OVERRIDE` pins for the parity runs.
    fn pinned_clock() -> FixedClock {
        FixedClock(
            Utc.with_ymd_and_hms(2022, 7, 1, 12, 0, 0)
                .single()
                .expect("a valid instant"),
        )
    }

    /// Fixture account 1 as seeded [app/data/ASCII/acctdata.txt:L1].
    fn account_1() -> Account {
        Account {
            acct_id: account_id(1),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: money10(19_400),
            acct_credit_limit: money10(202_000),
            acct_cash_credit_limit: money10(102_000),
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    /// Account 1 with the given limit, cycle totals and expiry date.
    fn account_with(limit: i64, credit: i64, debit: i64, expiry: &str) -> Account {
        Account {
            acct_credit_limit: money10(limit),
            acct_curr_cyc_credit: money10(credit),
            acct_curr_cyc_debit: money10(debit),
            acct_expiraion_date: Alnum::move_from(expiry),
            ..account_1()
        }
    }

    /// The cross-reference of `CARD_1` to `account` [app/data/ASCII/cardxref.txt:L49].
    fn xref_to(account: u64) -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(CARD_1),
            xref_cust_id: CustomerId::new(1).expect("the ID fits 9(09)"),
            xref_acct_id: account_id(account),
        }
    }

    /// A daily record of category 0001 on `card`, shaped as the fixture records.
    fn daily(tran_id: &str, type_cd: &str, cents: i64, card: &str) -> DailyTran {
        DailyTran {
            dalytran_id: TranId::move_from(tran_id),
            dalytran_type_cd: TranTypeCode::move_from(type_cd),
            dalytran_cat_cd: category(1),
            dalytran_source: Alnum::move_from("POS TERM"),
            dalytran_desc: Alnum::move_from("Purchase at Casper Group"),
            dalytran_amt: money9(cents),
            dalytran_merchant_id: MerchantId::new(800_000_000).expect("the ID fits 9(09)"),
            dalytran_merchant_name: Alnum::move_from("Casper Group"),
            dalytran_merchant_city: Alnum::move_from("Lake Wilma"),
            dalytran_merchant_zip: Alnum::move_from("12345"),
            dalytran_card_num: CardNumber::move_from(card),
            dalytran_orig_ts: Alnum::move_from(ORIG_TS),
            dalytran_proc_ts: Alnum::spaces(),
        }
    }

    fn tcatbal_key(account: u64, type_cd: &str) -> TranCatBalanceKey {
        TranCatBalanceKey {
            trancat_acct_id: account_id(account),
            trancat_type_cd: TranTypeCode::move_from(type_cd),
            trancat_cd: category(1),
        }
    }

    fn tcatbal(account: u64, type_cd: &str, cents: i64) -> TranCatBalance {
        TranCatBalance {
            key: tcatbal_key(account, type_cd),
            tran_cat_bal: money9(cents),
        }
    }

    /// The seeded state of account 1: account, cross-reference and its
    /// `01`/`0001` category balance [app/data/ASCII/tcatbal.txt:L1].
    fn seeded_account_1() -> FakeStore {
        FakeStore::default()
            .with_account(account_1())
            .with_xref(xref_to(1))
            .with_tcatbal(tcatbal(1, "01", 0))
    }

    /// The master record `daily` posts as, stamped with the pinned clock.
    fn posted_record(daily: &DailyTran) -> NewTransaction {
        daily.to_new_transaction(Alnum::move_from(PINNED_PROC_TS))
    }

    /// Validates `daily` and posts it when accepted, as the main loop does.
    fn validate_and_post(
        store: &mut FakeStore,
        daily: &DailyTran,
    ) -> Result<Option<Posted>, PostingAbend> {
        match block_on(validate(store, daily))? {
            Validation::Accepted { xref, account } => {
                block_on(post(store, daily, &xref, account, &pinned_clock())).map(Some)
            }
            Validation::Rejected { .. } => Ok(None),
        }
    }

    fn balance_of(store: &FakeStore, account: u64, type_cd: &str) -> Option<Money<9>> {
        store
            .tcatbals
            .get(&tcatbal_key(account, type_cd))
            .map(|record| record.tran_cat_bal)
    }

    fn stored_account(store: &FakeStore) -> &Account {
        store
            .accounts
            .get(&account_id(1))
            .expect("account 1 is on file")
    }

    /// P-CBTRN02C-013 to P-CBTRN02C-015, P-CBTRN02C-017 to P-CBTRN02C-019,
    /// P-CBTRN02C-025, P-CBTRN02C-026, P-CBTRN02C-226, P-CBTRN02C-220: dailytran
    /// record 156, a purchase of 81.44 [app/data/ASCII/dailytran.txt:L156].
    #[test]
    fn an_accepted_purchase_updates_its_balance_the_account_and_the_master() {
        let mut store = seeded_account_1();
        let purchase = daily("0000000503557384", "01", 8_144, CARD_1);

        let validation = block_on(validate(&mut store, &purchase));
        assert_eq!(
            validation,
            Ok(Validation::Accepted {
                xref: xref_to(1),
                account: account_1(),
            })
        );
        let posted = block_on(post(
            &mut store,
            &purchase,
            &xref_to(1),
            account_1(),
            &pinned_clock(),
        ));

        let expected = Transaction::from_new(TransactionRowId::from(1), posted_record(&purchase));
        assert_eq!(
            posted,
            Ok(Posted {
                transaction: expected.clone(),
                account_rewrite_failed: false,
            })
        );
        assert_eq!(expected.tran_proc_ts.as_bytes(), PINNED_PROC_TS.as_bytes());
        assert_eq!(
            store.transactions.get(&purchase.dalytran_id),
            Some(&expected)
        );
        assert_eq!(balance_of(&store, 1, "01"), Some(money9(8_144)));
        let account = stored_account(&store);
        assert_eq!(account.acct_curr_bal, money10(27_544));
        assert_eq!(account.acct_curr_cyc_credit, money10(8_144));
        assert_eq!(account.acct_curr_cyc_debit, Money::<10>::ZERO);
        assert_eq!(
            store.calls,
            [
                "read_xref_by_card",
                "read_account",
                "read_tcatbal",
                "rewrite_tcatbal",
                "rewrite_account",
                "write_transaction",
            ]
        );
    }

    /// P-CBTRN02C-016, P-CBTRN02C-024, P-CBTRN02C-117, P-CBTRN02C-227: dailytran
    /// record 164, a return of -70.77 whose `03`/`0001` balance is missing
    /// [app/data/ASCII/dailytran.txt:L164].
    #[test]
    fn a_return_creates_its_missing_balance_and_adds_to_the_cycle_debit() {
        let mut store = seeded_account_1();
        let refund = daily("0000000532120892", "03", -7_077, CARD_1);

        let posted = validate_and_post(&mut store, &refund).expect("no abend");

        assert!(posted.is_some_and(|posted| !posted.account_rewrite_failed));
        assert_eq!(balance_of(&store, 1, "03"), Some(money9(-7_077)));
        assert_eq!(balance_of(&store, 1, "01"), Some(Money::ZERO));
        let account = stored_account(&store);
        assert_eq!(account.acct_curr_bal, money10(12_323));
        assert_eq!(account.acct_curr_cyc_credit, Money::<10>::ZERO);
        assert_eq!(account.acct_curr_cyc_debit, money10(-7_077));
        assert!(store.calls.contains(&"write_tcatbal"));
        assert!(!store.calls.contains(&"rewrite_tcatbal"));
    }

    /// P-CBTRN02C-008 to P-CBTRN02C-011, P-CBTRN02C-217: each check, alone and
    /// combined, with its audited reason and description.
    #[test]
    fn rejections_carry_the_audited_reason_and_description() {
        let in_limit = account_with(10_000, 0, 0, "2025-05-20");
        let expired = account_with(10_000, 0, 0, "2022-06-09");
        let cases = [
            (
                "card not in the cross-reference",
                Some(in_limit.clone()),
                UNKNOWN_CARD,
                100,
                REASON_INVALID_CARD,
                INVALID_CARD_NUMBER_FOUND,
            ),
            (
                "account not on file",
                None,
                CARD_1,
                100,
                REASON_ACCOUNT_NOT_FOUND,
                ACCOUNT_RECORD_NOT_FOUND,
            ),
            (
                "over the limit by one cent",
                Some(in_limit),
                CARD_1,
                10_001,
                REASON_OVERLIMIT,
                OVERLIMIT_TRANSACTION,
            ),
            (
                "received after the expiry date",
                Some(expired.clone()),
                CARD_1,
                100,
                REASON_AFTER_EXPIRATION,
                TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
            ),
            (
                "over the limit and expired: 103 replaces 102",
                Some(expired),
                CARD_1,
                10_001,
                REASON_AFTER_EXPIRATION,
                TRANSACTION_RECEIVED_AFTER_ACCT_EXPIRATION,
            ),
        ];
        for (name, account, card, cents, reason, description) in cases {
            let mut store = FakeStore::default().with_xref(xref_to(1));
            if let Some(account) = account {
                store = store.with_account(account);
            }
            let validation = block_on(validate(&mut store, &daily("1", "01", cents, card)));
            assert_eq!(
                validation,
                Ok(Validation::Rejected {
                    reason,
                    description
                }),
                "{name}"
            );
            assert!(
                !store
                    .calls
                    .iter()
                    .any(|call| call.starts_with("write") || call.starts_with("rewrite")),
                "{name}: a rejected record writes nothing"
            );
        }

        let mut store = FakeStore::default().with_xref(xref_to(1));
        block_on(validate(&mut store, &daily("1", "01", 100, UNKNOWN_CARD))).expect("a rejection");
        assert_eq!(
            store.calls,
            ["read_xref_by_card"],
            "reason 100 skips the account read"
        );
    }

    /// P-CBTRN02C-010, P-CBTRN02C-011: both checks pass on equality, the limit
    /// test being `>=` and the expiry test an alphanumeric `>=` of the date
    /// against `DALYTRAN-ORIG-TS (1:10)`.
    #[test]
    fn the_limit_and_the_expiry_date_themselves_are_accepted() {
        let account = account_with(10_000, 0, 0, "2022-06-10");
        let mut store = FakeStore::default()
            .with_xref(xref_to(1))
            .with_account(account.clone());

        let validation = block_on(validate(&mut store, &daily("1", "01", 10_000, CARD_1)));

        assert_eq!(
            validation,
            Ok(Validation::Accepted {
                xref: xref_to(1),
                account,
            })
        );
    }

    /// P-CBTRN02C-022: the narrower `WS-TEMP-BAL` keeps the low-order nine
    /// integer digits, so a cycle credit of 1500000000.00 checks as
    /// 500000000.00. P-CBTRN02C-023: subtracting a negative cycle debit adds
    /// its magnitude.
    #[test]
    fn ws_temp_bal_is_stored_truncated_and_decides_the_limit_check() {
        let cases = [
            (
                "audited example",
                60_000_000_000,
                150_000_000_000,
                0,
                0,
                true,
            ),
            (
                "limit below the kept digits",
                49_999_999_999,
                150_000_000_000,
                0,
                0,
                false,
            ),
            (
                "cycle debit adds its magnitude",
                14_000,
                10_000,
                -3_000,
                1_000,
                true,
            ),
            (
                "one cent under that sum",
                13_999,
                10_000,
                -3_000,
                1_000,
                false,
            ),
        ];
        for (name, limit, credit, debit, amount, accepted) in cases {
            let account = account_with(limit, credit, debit, "2025-05-20");
            let tran = daily("1", "01", amount, CARD_1);
            let mut store = FakeStore::default()
                .with_xref(xref_to(1))
                .with_account(account.clone());

            let validation = block_on(validate(&mut store, &tran)).expect("no abend");

            let expected = if accepted {
                Validation::Accepted {
                    xref: xref_to(1),
                    account: account.clone(),
                }
            } else {
                Validation::Rejected {
                    reason: REASON_OVERLIMIT,
                    description: OVERLIMIT_TRANSACTION,
                }
            };
            assert_eq!(validation, expected, "{name}");
        }
        let audited = account_with(60_000_000_000, 150_000_000_000, 0, "2025-05-20");
        assert_eq!(
            ws_temp_bal(&audited, &daily("1", "01", 0, CARD_1)),
            money9(50_000_000_000)
        );
        let debited = account_with(14_000, 10_000, -3_000, "2025-05-20");
        assert_eq!(
            ws_temp_bal(&debited, &daily("1", "01", 1_000, CARD_1)),
            money9(14_000)
        );
    }

    /// P-CBTRN02C-024 to P-CBTRN02C-026: the balance stores keep the low-order
    /// digits of their receivers, nine for `TRAN-CAT-BAL` and ten for the
    /// account fields.
    #[test]
    fn balance_stores_truncate_high_order_digits() {
        let account = Account {
            acct_curr_bal: money10(999_999_999_999),
            acct_curr_cyc_credit: money10(999_999_999_999),
            acct_credit_limit: money10(999_999_999_999),
            ..account_1()
        };
        let mut store = FakeStore::default()
            .with_xref(xref_to(1))
            .with_account(account.clone())
            .with_tcatbal(tcatbal(1, "01", 99_999_999_999));
        let tran = daily("1", "01", 1, CARD_1);

        block_on(post(
            &mut store,
            &tran,
            &xref_to(1),
            account,
            &pinned_clock(),
        ))
        .expect("no abend");

        assert_eq!(balance_of(&store, 1, "01"), Some(Money::ZERO));
        let stored = stored_account(&store);
        assert_eq!(stored.acct_curr_bal, Money::<10>::ZERO);
        assert_eq!(stored.acct_curr_cyc_credit, Money::<10>::ZERO);
    }

    /// P-CBTRN02C-018, P-CBTRN02C-218 (E-15): a rewrite `INVALID KEY` sets
    /// reason 109, which is neither counted nor rejected, and the transaction
    /// is still written after the category balance update.
    #[test]
    fn an_account_rewrite_invalid_key_still_writes_the_transaction() {
        let mut store = seeded_account_1();
        store.fail_next("rewrite_account", PortError::NotFound);
        let purchase = daily("0000000503557384", "01", 8_144, CARD_1);

        let posted = validate_and_post(&mut store, &purchase).expect("no abend");

        assert!(posted.is_some_and(|posted| posted.account_rewrite_failed));
        assert!(store.transactions.contains_key(&purchase.dalytran_id));
        assert_eq!(balance_of(&store, 1, "01"), Some(money9(8_144)));
        assert_eq!(stored_account(&store), &account_1());
    }

    /// P-CBTRN02C-017, P-CBTRN02C-021: a failed balance rewrite abends; the
    /// earlier record's writes stand, and nothing of the failing record
    /// follows the rewrite.
    #[test]
    fn a_failed_balance_rewrite_abends_after_the_earlier_writes() {
        let mut store = seeded_account_1();
        let first = daily("0000000503557384", "01", 8_144, CARD_1);
        let second = daily("0000000555363230", "01", 48_499, CARD_1);
        validate_and_post(&mut store, &first).expect("the first record posts");
        store.fail_next("rewrite_tcatbal", PortError::Unavailable("lost".to_owned()));

        let abend = validate_and_post(&mut store, &second).expect_err("an abend");

        assert_eq!(abend.paragraph, "2700-B-UPDATE-TCATBAL-REC");
        assert_eq!(abend.display, ERROR_REWRITING_TRANSACTION_BALANCE_FILE);
        assert_eq!(abend.file_status, "");
        assert_eq!(
            abend.to_string(),
            [ERROR_REWRITING_TRANSACTION_BALANCE_FILE, ABENDING_PROGRAM].join("\n")
        );
        assert_eq!(store.calls.last(), Some(&"rewrite_tcatbal"));
        assert_eq!(balance_of(&store, 1, "01"), Some(money9(8_144)));
        assert_eq!(stored_account(&store).acct_curr_cyc_credit, money10(8_144));
        assert_eq!(store.transactions.len(), 1);
    }

    /// P-CBTRN02C-019: a duplicate TRAN-ID abends with status 22, and the
    /// category balance and account updates of that record stay written.
    #[test]
    fn a_duplicate_tran_id_abends_after_the_balance_and_account_writes() {
        let purchase = daily("0000000503557384", "01", 8_144, CARD_1);
        let mut store = seeded_account_1().with_transaction(posted_record(&purchase));

        let abend = validate_and_post(&mut store, &purchase).expect_err("an abend");

        assert_eq!(abend.paragraph, "2900-WRITE-TRANSACTION-FILE");
        assert_eq!(abend.file_status, "22");
        assert_eq!(abend.cause, Some(PortError::Duplicate));
        assert_eq!(
            abend.to_string(),
            [
                ERROR_WRITING_TO_TRANSACTION_FILE,
                "FILE STATUS IS: NNNN0022",
                ABENDING_PROGRAM,
            ]
            .join("\n")
        );
        assert_eq!(balance_of(&store, 1, "01"), Some(money9(8_144)));
        assert_eq!(stored_account(&store).acct_curr_bal, money10(27_544));
    }

    /// P-CBTRN02C-015, P-CBTRN02C-016: a balance read failing other than with
    /// status 23, and a failed balance create, abend with their texts.
    #[test]
    fn balance_read_and_create_failures_abend_with_their_texts() {
        let cases = [
            (
                "read_tcatbal",
                PortError::Unavailable("lost".to_owned()),
                "2700-UPDATE-TCATBAL",
                ERROR_READING_TRANSACTION_BALANCE_FILE,
                "",
            ),
            (
                "write_tcatbal",
                PortError::Duplicate,
                "2700-A-CREATE-TCATBAL-REC",
                ERROR_WRITING_TRANSACTION_BALANCE_FILE,
                "22",
            ),
        ];
        for (method, error, paragraph, display, file_status) in cases {
            let mut store = FakeStore::default()
                .with_account(account_1())
                .with_xref(xref_to(1));
            store.fail_next(method, error.clone());

            let abend = validate_and_post(&mut store, &daily("1", "01", 100, CARD_1))
                .expect_err("an abend");

            assert_eq!(
                abend,
                PostingAbend {
                    paragraph,
                    display,
                    file_status: file_status.to_owned(),
                    cause: Some(error),
                },
                "{method}"
            );
            assert_eq!(store.calls.last(), Some(&method), "{method}");
            assert!(store.tcatbals.is_empty(), "{method}");
            assert!(store.transactions.is_empty(), "{method}");
        }
    }

    /// E-77: store failures other than `NOTFND` on the cross-reference read,
    /// the account read and the account rewrite, which the COBOL does not
    /// test, stop the run with the unexpected-error text instead of posting
    /// against stale data.
    #[test]
    fn store_failures_the_cobol_does_not_test_stop_the_run() {
        let cases = [
            (
                "read_xref_by_card",
                PortError::Contention,
                "1500-A-LOOKUP-XREF",
            ),
            (
                "read_account",
                PortError::Unavailable("lost".to_owned()),
                "1500-B-LOOKUP-ACCT",
            ),
            (
                "rewrite_account",
                PortError::Unavailable("lost".to_owned()),
                "2800-UPDATE-ACCOUNT-REC",
            ),
        ];
        for (method, error, paragraph) in cases {
            let mut store = seeded_account_1();
            store.fail_next(method, error.clone());

            let abend = validate_and_post(&mut store, &daily("1", "01", 100, CARD_1))
                .expect_err("an abend");

            assert_eq!(abend.paragraph, paragraph, "{method}");
            assert_eq!(abend.display, UNEXPECTED_ERROR, "{method}");
            assert_eq!(abend.file_status_line(), None, "{method}");
            assert_eq!(abend.cause, Some(error), "{method}");
            assert!(store.transactions.is_empty(), "{method}");
            assert_eq!(stored_account(&store), &account_1(), "{method}");
        }
    }

    /// Section D for account 1: dailytran records 156, 164, 174, 199, 231 and
    /// 240 in file order from the seeded state. Records 231 and 240 are over
    /// the limit because the earlier postings raised the cycle credit
    /// (P-CBTRN02C-051, P-CBTRN02C-054); the final account and balances are
    /// P-CBTRN02C-066, P-CBTRN02C-116 and P-CBTRN02C-117.
    #[test]
    fn the_fixture_records_of_account_1_post_as_audited() {
        let records = [
            daily("0000000503557384", "01", 8_144, CARD_1),
            daily("0000000532120892", "03", -7_077, CARD_1),
            daily("0000000555363230", "01", 48_499, CARD_1),
            daily("0000000624335286", "01", 59_844, CARD_1),
            daily("0000000755736377", "01", 90_644, CARD_1),
            daily("0000000776200014", "01", 98_522, CARD_1),
        ];
        let mut store = seeded_account_1();
        let mut counts = PostingCounts::default();
        let mut rejects = Vec::new();
        for record in &records {
            counts.read += 1;
            let temp_bal = ws_temp_bal(stored_account(&store), record);
            match block_on(validate(&mut store, record)).expect("no abend") {
                Validation::Accepted { xref, account } => {
                    block_on(post(&mut store, record, &xref, account, &pinned_clock()))
                        .expect("no abend");
                }
                Validation::Rejected {
                    reason,
                    description,
                } => {
                    counts.rejected += 1;
                    rejects.push((record.dalytran_id, reason, description, temp_bal));
                }
            }
        }

        assert_eq!(
            rejects,
            [
                (
                    TranId::move_from("0000000755736377"),
                    REASON_OVERLIMIT,
                    OVERLIMIT_TRANSACTION,
                    money9(214_208),
                ),
                (
                    TranId::move_from("0000000776200014"),
                    REASON_OVERLIMIT,
                    OVERLIMIT_TRANSACTION,
                    money9(222_086),
                ),
            ]
        );
        assert_eq!(
            (counts.read, counts.rejected, counts.return_code()),
            (6, 2, 4)
        );
        let account = stored_account(&store);
        assert_eq!(account.acct_curr_bal, money10(128_810));
        assert_eq!(account.acct_curr_cyc_credit, money10(116_487));
        assert_eq!(account.acct_curr_cyc_debit, money10(-7_077));
        assert_eq!(balance_of(&store, 1, "01"), Some(money9(116_487)));
        assert_eq!(balance_of(&store, 1, "03"), Some(money9(-7_077)));
        let posted: Vec<TranId> = store.transactions.keys().copied().collect();
        let expected: Vec<TranId> = [0, 1, 2, 3]
            .iter()
            .map(|&index| records[index].dalytran_id)
            .collect();
        assert_eq!(posted, expected);
    }

    /// P-CBTRN02C-001 to P-CBTRN02C-003, P-CBTRN02C-012: the input bytes, the
    /// four reason digits and the 76-byte description, 430 bytes in all.
    #[test]
    fn a_reject_record_is_the_input_bytes_then_the_validation_trailer() {
        let mut raw = [b' '; REJECT_TRAN_DATA_LEN];
        for (byte, digit) in raw.iter_mut().zip(b"0123456789".iter().cycle()) {
            *byte = *digit;
        }
        let long_description = "X".repeat(80);
        let cases = [
            (
                REASON_OVERLIMIT,
                OVERLIMIT_TRANSACTION,
                "0102",
                OVERLIMIT_TRANSACTION,
            ),
            (7, "", "0007", ""),
            (
                12_345,
                long_description.as_str(),
                "2345",
                &long_description[..76],
            ),
        ];
        for (reason, description, digits, kept) in cases {
            let record = reject_record(&raw, reason, description);

            assert_eq!(record.len(), REJECT_RECORD_LEN);
            let (data, trailer) = record.split_at(REJECT_TRAN_DATA_LEN);
            assert_eq!(data, raw.as_slice(), "{reason}");
            let (reason_field, description_field) = trailer.split_at(4);
            assert_eq!(reason_field, digits.as_bytes(), "{reason}");
            let padded = format!("{kept:<76}");
            assert_eq!(description_field, padded.as_bytes(), "{reason}");
        }
    }

    /// P-CBTRN02C-020: `RETURN-CODE` 4 once any record is rejected.
    #[test]
    fn the_return_code_is_four_when_any_record_was_rejected() {
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

    /// P-CBTRN02C-005, P-CBTRN02C-007, P-CBTRN02C-012, P-CBTRN02C-020,
    /// P-CBTRN02C-021, P-CBTRN02C-221, P-CBTRN02C-223: the job-level abends
    /// display their text, the status line when the status is known, then
    /// `ABENDING PROGRAM`.
    #[test]
    fn job_level_abends_display_their_lines_in_the_cobol_order() {
        let cases = [
            (
                PostingAbend::dalytran_open(),
                "0000-DALYTRAN-OPEN",
                vec![ERROR_OPENING_DALYTRAN, ABENDING_PROGRAM],
            ),
            (
                PostingAbend::tranfile_open_not_empty(),
                "0100-TRANFILE-OPEN",
                vec![
                    ERROR_OPENING_TRANSACTION_FILE,
                    "FILE STATUS IS: NNNN0037",
                    ABENDING_PROGRAM,
                ],
            ),
            (
                PostingAbend::dalyrejs_open(),
                "0300-DALYREJS-OPEN",
                vec![ERROR_OPENING_DALY_REJECTS_FILE, ABENDING_PROGRAM],
            ),
            (
                PostingAbend::dalytran_read(),
                "1000-DALYTRAN-GET-NEXT",
                vec![ERROR_READING_DALYTRAN_FILE, ABENDING_PROGRAM],
            ),
            (
                PostingAbend::reject_write(),
                "2500-WRITE-REJECT-REC",
                vec![ERROR_WRITING_TO_REJECTS_FILE, ABENDING_PROGRAM],
            ),
            (
                PostingAbend::dalyrejs_close(),
                "9300-DALYREJS-CLOSE",
                vec![ERROR_CLOSING_DAILY_REJECTS_FILE, ABENDING_PROGRAM],
            ),
        ];
        for (abend, paragraph, lines) in cases {
            assert_eq!(abend.paragraph, paragraph);
            assert_eq!(abend.to_string(), lines.join("\n"), "{paragraph}");
            assert!(abend.source().is_none(), "{paragraph}");
            assert!(abend.cause.is_none(), "{paragraph}");
        }
    }

    /// Over a `Send` store both rule futures are `Send`, as the batch adapter's
    /// multi-threaded runtime needs.
    #[test]
    fn the_rule_futures_are_send() {
        fn sendable<F: Future + Send>(future: F) -> F {
            future
        }
        let mut store = seeded_account_1();
        let purchase = daily("0000000503557384", "01", 8_144, CARD_1);
        let clock = pinned_clock();

        let validation = block_on(sendable(validate(&mut store, &purchase)));
        let posted = block_on(sendable(post(
            &mut store,
            &purchase,
            &xref_to(1),
            account_1(),
            &clock,
        )));

        assert!(matches!(validation, Ok(Validation::Accepted { .. })));
        assert!(posted.is_ok());
    }

    /// The port failure behind an abend is its error source, kept out of the
    /// displayed lines.
    #[test]
    fn a_port_abend_exposes_its_cause_as_the_source_only() {
        let mut store = seeded_account_1();
        store.fail_next(
            "read_tcatbal",
            PortError::Unavailable("connection reset by peer".to_owned()),
        );

        let abend =
            validate_and_post(&mut store, &daily("1", "01", 100, CARD_1)).expect_err("an abend");

        let source = abend.source().map(ToString::to_string);
        assert_eq!(
            source.as_deref(),
            Some("store unavailable: connection reset by peer")
        );
        assert!(!abend.to_string().contains("connection reset"));
    }
}
