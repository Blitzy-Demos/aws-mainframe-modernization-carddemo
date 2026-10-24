//! POSTTRAN, the daily transaction posting CBTRN02C, as pure rules (`app/cbl/CBTRN02C.cbl`;
//! `modern/audit/batch/POSTTRAN.md`; AAP 0.3.3.2, 0.3.4.3).
//!
//! `batch::jobs::posttran` decodes `dailytran.txt` with codec A, loads `accounts`, `card_xref`
//! and `tran_cat_balances` into [`Posttran::new`], and feeds the records to [`Posttran::process`]
//! in input order. Each call returns one [`RecordOutcome`]: a [`Posting`], whose three effects the
//! job applies in the order the program writes them, or a [`RejectRecord`], which the job writes
//! to the `DALYREJS` generation. This module does no I/O and reads no clock of its own; the job
//! passes the injected [`Clock`].
//!
//! | Item | COBOL | Rules |
//! |---|---|---|
//! | [`Posttran::process`] | main loop | CBTRN02C-001 to CBTRN02C-003, CBTRN02C-022, CBTRN02C-026, CBTRN02C-036 |
//! | [`validate`], [`RejectReason`], [`temp_balance`] | `1500-VALIDATE-TRAN`, `1500-A-LOOKUP-XREF`, `1500-B-LOOKUP-ACCT` | CBTRN02C-003 to CBTRN02C-011 |
//! | [`transaction_record`] | `2000-POST-TRANSACTION`, `Z-GET-DB2-FORMAT-TIMESTAMP` | CBTRN02C-012 to CBTRN02C-014 |
//! | [`TcatbalWrite`], [`new_category_balance`], [`add_to_category_balance`] | `2700-UPDATE-TCATBAL`, `2700-A-CREATE-TCATBAL-REC`, `2700-B-UPDATE-TCATBAL-REC` | CBTRN02C-016 to CBTRN02C-019 |
//! | [`update_account`] | `2800-UPDATE-ACCOUNT-REC` | CBTRN02C-020, CBTRN02C-021, CBTRN02C-023 |
//! | [`Posting`] | `2000-POST-TRANSACTION`, `2900-WRITE-TRANSACTION-FILE` | CBTRN02C-015, CBTRN02C-024 |
//! | [`reject_record`] | `2500-WRITE-REJECT-REC` | CBTRN02C-027 |
//! | [`Posttran::finish`], [`PosttranSummary`], [`START_LINE`], [`END_LINE`] | the end of the main procedure | CBTRN02C-029, CBTRN02C-034 to CBTRN02C-036 |
//!
//! # Defects reproduced
//!
//! - E09: the expiration test runs after the limit test and overwrites its reason, so a record
//!   that fails both is rejected with `0103` only (CBTRN02C-007, CBTRN02C-008).
//! - E10: `WS-TEMP-BAL PIC S9(09)V99` is narrower than its `S9(10)V99` operands, so the sum the
//!   limit test compares drops its high-order digits (CBTRN02C-006, ARITH-025).
//! - The cycle debit accumulates negative amounts, so subtracting it adds their magnitude to the
//!   next `WS-TEMP-BAL` (ARITH-033).
//! - A category balance created by `INITIALIZE` keeps the `FILLER` of the row most recently read
//!   in the run, not spaces (CBTRN02C-017, X-FIXTURES-1).
//! - The reason `0109` that a failed account rewrite moves has no effect (CBTRN02C-023); an
//!   in-memory account cannot vanish between its read and its rewrite, so the path never runs
//!   here, and the job treats an update that matches no row the same way.
//!
//! # Halts
//!
//! `HALTED(H2)`: the 20-byte `FILLER` of every posted `TRAN-RECORD` (CBTRN02C-014), and the
//! `FILLER` of a category balance created before any category-balance read of the run has
//! succeeded (CBTRN02C-018), hold storage nothing initialises. The replacement writes spaces
//! there, a stand-in that is not a parity result. The 17 key bytes of the not-found line are
//! the halt candidate X-POSTTRAN-H2; [`TcatbalWrite::sysout_line`] shows the key looked up,
//! which is not claimed as parity either.
//!
//! # Outside this module
//!
//! The opens, the closes, the failure paths, the reject generation and the exit status of an
//! abend are the job's (CBTRN02C-028, CBTRN02C-030 to CBTRN02C-033, CBTRN02C-038 to
//! CBTRN02C-045). So is the check that every input body is the full 350 bytes, which keeps a
//! padded record from reaching [`reject_record`] (CBTRN02C-045).
//!
//! # Rules governing this module
//!
//! - Money: amounts change only through `crate::money`, each store at its receiving picture:
//!   `WS-TEMP-BAL` and `TRAN-CAT-BAL` at `S9(09)V99`, the account fields at `S9(10)V99`
//!   (ARITH-023 to ARITH-033).
//! - Typed Dates and Timestamps: the expiration test compares the typed `ACCT-EXPIRAION-DATE`
//!   with the date of the typed `DALYTRAN-ORIG-TS` (CBTRN02C-010); `TRAN-PROC-TS` is a layout-`D`
//!   [`PicTimestamp`] from [`db2_timestamp`] (CBTRN02C-013, E33). Only the reject record's 350
//!   input bytes are codec-level bytes (CBTRN02C-027).
//! - Cardholder Data Redaction: postings and reject records carry the full card number for the
//!   `transactions` table and the local reject file only (CBTRN02C-037). Card numbers are held as
//!   [`CardNumber`] and reject records as [`RejectRecord`], whose `Debug` implementations redact
//!   them, and no error, job-log line or summary carries a card number or a record's bytes.
//! - Checked Integer Narrowing: the counters are 64-bit; only their job-log rendering keeps the
//!   low nine digits, as `PIC 9(09)` does (CBTRN02C-036).

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use copybook::{FixedRecord, PicTimestamp, PicU, PicX};

use crate::clock::{Clock, db2_timestamp};
use crate::money::Money;
use crate::records::{
    AccountRecord, CardXrefRecord, DailyTransactionRecord, RejectRecord, TranCatBalanceRecord,
    TransactionRecord,
};
use crate::sensitive::CardNumber;

/// `DISPLAY 'START OF EXECUTION OF PROGRAM CBTRN02C'`, the first job-log line of every run
/// (`CBTRN02C.cbl:194`; CBTRN02C-034).
pub const START_LINE: &str = "START OF EXECUTION OF PROGRAM CBTRN02C";

/// `DISPLAY 'END OF EXECUTION OF PROGRAM CBTRN02C'`, the last job-log line of a run that ends
/// normally (`CBTRN02C.cbl:232`; CBTRN02C-034).
pub const END_LINE: &str = "END OF EXECUTION OF PROGRAM CBTRN02C";

/// The 24-character label `DISPLAY` writes before `WS-TRANSACTION-COUNT` (`CBTRN02C.cbl:227`;
/// CBTRN02C-034).
pub const PROCESSED_LABEL: &str = "TRANSACTIONS PROCESSED :";

/// The 24-character label `DISPLAY` writes before `WS-REJECT-COUNT`, two spaces before the
/// colon (`CBTRN02C.cbl:228`; CBTRN02C-034).
pub const REJECTED_LABEL: &str = "TRANSACTIONS REJECTED  :";

/// The 35-character literal, ending in a space, that starts the line `2700-UPDATE-TCATBAL`
/// displays for a category balance it is about to create (`CBTRN02C.cbl:476`; CBTRN02C-016).
pub const TCATBAL_NOT_FOUND_PREFIX: &str = "TCATBAL record not found for key : ";

/// The literal that ends that line, after the 17 key bytes (`CBTRN02C.cbl:477`; CBTRN02C-016).
pub const TCATBAL_NOT_FOUND_SUFFIX: &str = ".. Creating.";

/// `MOVE 4 TO RETURN-CODE`, the return code of a run that rejected a record
/// (`CBTRN02C.cbl:229-231`; CBTRN02C-035).
pub const REJECTS_RETURN_CODE: u8 = 4;

/// One more than the largest `PIC 9(09)` count, the modulus its rendering keeps
/// (CBTRN02C-036).
const COUNT_MODULUS: u64 = 1_000_000_000;

/// `MOVE 100 TO WS-VALIDATION-FAIL-REASON` (`CBTRN02C.cbl:385`; CBTRN02C-004). Evaluated at
/// compile time, so a code that did not fit `PIC 9(04)` would fail the build.
const INVALID_CARD_CODE: PicU<4> = match PicU::new(100) {
    Ok(code) => code,
    Err(_) => panic!("100 fits PIC 9(04)"),
};

/// `MOVE 101 TO WS-VALIDATION-FAIL-REASON` (`CBTRN02C.cbl:397`; CBTRN02C-005).
const ACCOUNT_NOT_FOUND_CODE: PicU<4> = match PicU::new(101) {
    Ok(code) => code,
    Err(_) => panic!("101 fits PIC 9(04)"),
};

/// `MOVE 102 TO WS-VALIDATION-FAIL-REASON` (`CBTRN02C.cbl:410`; CBTRN02C-007).
const OVERLIMIT_CODE: PicU<4> = match PicU::new(102) {
    Ok(code) => code,
    Err(_) => panic!("102 fits PIC 9(04)"),
};

/// `MOVE 103 TO WS-VALIDATION-FAIL-REASON` (`CBTRN02C.cbl:417`; CBTRN02C-008).
const AFTER_EXPIRATION_CODE: PicU<4> = match PicU::new(103) {
    Ok(code) => code,
    Err(_) => panic!("103 fits PIC 9(04)"),
};

/// Why `1500-VALIDATE-TRAN` rejects a daily transaction: the `WS-VALIDATION-FAIL-REASON` and
/// `WS-VALIDATION-FAIL-REASON-DESC` a rejected record carries into its trailer
/// (`CBTRN02C.cbl:380-422`; CBTRN02C-004 to CBTRN02C-008, CBTRN02C-027).
///
/// The reason `0109` of a failed account rewrite is not a variant: it is moved after the reject
/// decision and never reaches a reject record (CBTRN02C-023).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RejectReason {
    /// `0100` `INVALID CARD NUMBER FOUND`: no cross-reference has the record's card number
    /// (CBTRN02C-004).
    InvalidCard,
    /// `0101` `ACCOUNT RECORD NOT FOUND`: no account has the cross-reference's account ID
    /// (CBTRN02C-005).
    AccountNotFound,
    /// `0102` `OVERLIMIT TRANSACTION`: the credit limit is below the stored `WS-TEMP-BAL`
    /// and the record is not after the account's expiration (CBTRN02C-007).
    Overlimit,
    /// `0103` `TRANSACTION RECEIVED AFTER ACCT EXPIRATION`: the expiration date is before the
    /// record's date, whatever the limit test found (CBTRN02C-008, E09).
    AfterExpiration,
}

impl RejectReason {
    /// The `PIC 9(04)` reason code, which the reject trailer writes as four digits with leading
    /// zeros: `0100` to `0103` (CBTRN02C-027).
    #[must_use]
    pub const fn code(self) -> PicU<4> {
        match self {
            Self::InvalidCard => INVALID_CARD_CODE,
            Self::AccountNotFound => ACCOUNT_NOT_FOUND_CODE,
            Self::Overlimit => OVERLIMIT_CODE,
            Self::AfterExpiration => AFTER_EXPIRATION_CODE,
        }
    }

    /// The literal the program moves to `WS-VALIDATION-FAIL-REASON-DESC`
    /// (`CBTRN02C.cbl:386,398,411,418`; CBTRN02C-004 to CBTRN02C-008).
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::InvalidCard => "INVALID CARD NUMBER FOUND",
            Self::AccountNotFound => "ACCOUNT RECORD NOT FOUND",
            Self::Overlimit => "OVERLIMIT TRANSACTION",
            Self::AfterExpiration => "TRANSACTION RECEIVED AFTER ACCT EXPIRATION",
        }
    }

    /// `WS-VALIDATION-FAIL-REASON-DESC PIC X(76)`: [`text`](Self::text) left-justified and
    /// padded with spaces, as the alphanumeric `MOVE` stores it (CBTRN02C-027).
    #[must_use]
    pub fn description(self) -> PicX<76> {
        PicX::moved_from(self.text().as_bytes())
    }
}

/// The result of `1500-VALIDATE-TRAN` for one record: posted when the reason is still 0
/// (CBTRN02C-002, CBTRN02C-009).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validation {
    /// The reason stayed 0: the record is posted.
    Valid,
    /// The last reason the paragraph moved.
    Rejected(RejectReason),
}

/// `1500-VALIDATE-TRAN` for `daily` (`CBTRN02C.cbl:370-422`; CBTRN02C-003 to CBTRN02C-011).
///
/// `xref` is the cross-reference whose `XREF-CARD-NUM` equals `DALYTRAN-CARD-NUM`, and `account`
/// the account whose `ACCT-ID` equals that cross-reference's `XREF-ACCT-ID`; `None` is a keyed
/// read that ends `INVALID KEY`. In program order:
///
/// 1. No cross-reference: [`RejectReason::InvalidCard`]; `account` is not consulted
///    (CBTRN02C-004).
/// 2. No account: [`RejectReason::AccountNotFound`] (CBTRN02C-005).
/// 3. [`temp_balance`] above the credit limit: [`RejectReason::Overlimit`]; the comparison is
///    exact and equality passes (CBTRN02C-006, CBTRN02C-007, ARITH-026).
/// 4. Then, independently, a record dated after the account's expiration:
///    [`RejectReason::AfterExpiration`], overwriting the reason of step 3 (CBTRN02C-008, E09);
///    equality passes (CBTRN02C-009, CBTRN02C-010).
///
/// Nothing else is validated: the account's active status, the card file, the amount and the
/// transaction type and category are not checked (CBTRN02C-011).
#[must_use]
pub fn validate(
    daily: &DailyTransactionRecord,
    xref: Option<&CardXrefRecord>,
    account: Option<&AccountRecord>,
) -> Validation {
    if xref.is_none() {
        return Validation::Rejected(RejectReason::InvalidCard);
    }
    let Some(account) = account else {
        return Validation::Rejected(RejectReason::AccountNotFound);
    };
    let mut reason = None;
    if account.credit_limit < temp_balance(account, daily) {
        reason = Some(RejectReason::Overlimit);
    }
    if !received_by_expiration(account, daily) {
        reason = Some(RejectReason::AfterExpiration);
    }
    reason.map_or(Validation::Valid, Validation::Rejected)
}

/// `COMPUTE WS-TEMP-BAL = ACCT-CURR-CYC-CREDIT - ACCT-CURR-CYC-DEBIT + DALYTRAN-AMT` into
/// `PIC S9(09)V99` (`CBTRN02C.cbl:187,403-405`; CBTRN02C-006).
///
/// The exact sum is stored by truncation with the high-order digits beyond nine dropped, so a
/// cycle credit of 1500000000.00 with an amount of 100.00 stores 500000100.00 (E10, ARITH-025).
/// The cycle debit holds the posted negative amounts, so subtracting it adds their magnitude
/// (ARITH-023, ARITH-024, ARITH-033).
#[must_use]
pub fn temp_balance(account: &AccountRecord, daily: &DailyTransactionRecord) -> Money<9> {
    (account.curr_cyc_credit - account.curr_cyc_debit + daily.amt).store::<9>()
}

/// `IF ACCT-EXPIRAION-DATE >= DALYTRAN-ORIG-TS (1:10)`: whether the record passes the
/// expiration test (`CBTRN02C.cbl:414`; CBTRN02C-010).
///
/// The first ten characters of a timestamp are its date in every layout, and for valid
/// `YYYY-MM-DD` texts character order is calendar order, so the typed values are compared. A
/// blank field is `None`, which sorts below every date as the space sorts below every digit: a
/// blank expiration date fails against a dated record, and a blank timestamp passes against any
/// expiration date, blank included.
fn received_by_expiration(account: &AccountRecord, daily: &DailyTransactionRecord) -> bool {
    let received = daily
        .orig_ts
        .value()
        .map(|timestamp| timestamp.value().date());
    account.expiraion_date.value() >= received
}

/// The `TRAN-RECORD` that `2000-POST-TRANSACTION` builds from `daily`, with one reading of
/// `clock` (`CBTRN02C.cbl:424-438,692-705`; CBTRN02C-012 to CBTRN02C-014).
///
/// - Twelve fields move to their namesakes of the same picture, so their values are unchanged:
///   ID, type, category, source, description, amount (ARITH-027), merchant ID, name, city and
///   ZIP, card number, and `TRAN-ORIG-TS` with its layout code (CBTRN02C-012).
/// - `TRAN-PROC-TS` is `DB2-FORMAT-TS`: the clock's reading in layout `D`, its fraction cut to
///   hundredths, `YYYY-MM-DD-HH.MM.SS.hh0000`; `DALYTRAN-PROC-TS` is not moved (CBTRN02C-013,
///   E33).
/// - `FILLER`: 20 spaces, a `HALTED(H2)` stand-in (CBTRN02C-014).
#[must_use]
pub fn transaction_record(daily: &DailyTransactionRecord, clock: &dyn Clock) -> TransactionRecord {
    TransactionRecord {
        id: daily.id.clone(),
        type_cd: daily.type_cd.clone(),
        cat_cd: daily.cat_cd,
        source: daily.source.clone(),
        desc: daily.desc.clone(),
        amt: daily.amt,
        merchant_id: daily.merchant_id,
        merchant_name: daily.merchant_name.clone(),
        merchant_city: daily.merchant_city.clone(),
        merchant_zip: daily.merchant_zip.clone(),
        card_num: daily.card_num.clone(),
        orig_ts: daily.orig_ts,
        proc_ts: PicTimestamp::new(Some(db2_timestamp(clock.now()))),
        // HALTED(H2): no statement moves this FILLER and `TRAN-RECORD` has no VALUE, so every
        // posted record writes storage nothing initialises; spaces stand in (CBTRN02C-014).
        filler: PicX::spaces(),
    }
}

/// `2700-A-CREATE-TCATBAL-REC` for the key (`acct_id`, `DALYTRAN-TYPE-CD`, `DALYTRAN-CAT-CD`)
/// (`CBTRN02C.cbl:503-508`; CBTRN02C-017, CBTRN02C-018).
///
/// `INITIALIZE TRAN-CAT-BAL-RECORD` sets the account ID and category to zero, the type to spaces
/// and the balance to 0.00, and leaves `FILLER` as it was; the key fields are then moved in and
/// `ADD DALYTRAN-AMT TO TRAN-CAT-BAL` gives the balance `(0.00 + amt).store::<9>()` (ARITH-028).
///
/// `working_filler` is the `FILLER` the working record holds: that of the category balance most
/// recently read in the run, as read (CBTRN02C-017, X-FIXTURES-1). `None`, before any read has
/// succeeded, writes 22 spaces, a `HALTED(H2)` stand-in (CBTRN02C-018).
#[must_use]
pub fn new_category_balance(
    acct_id: PicU<11>,
    daily: &DailyTransactionRecord,
    working_filler: Option<&PicX<22>>,
) -> TranCatBalanceRecord {
    let initialized = TranCatBalanceRecord {
        acct_id,
        type_cd: daily.type_cd.clone(),
        cd: daily.cat_cd,
        bal: Money::ZERO,
        // HALTED(H2) when `working_filler` is `None`: no read has filled the working record, so
        // its FILLER is storage nothing initialises; spaces stand in (CBTRN02C-018).
        filler: working_filler.cloned().unwrap_or_else(PicX::spaces),
    };
    add_to_category_balance(initialized, daily.amt)
}

/// `ADD DALYTRAN-AMT TO TRAN-CAT-BAL` into `PIC S9(09)V99`: `(bal + amt).store::<9>()`, the
/// high-order digits beyond nine dropped (`CBTRN02C.cbl:508,527`; CBTRN02C-019, ARITH-028 to
/// ARITH-030). The key and `FILLER` are unchanged, as `2700-B-UPDATE-TCATBAL-REC` rewrites the
/// record as read.
#[must_use]
pub fn add_to_category_balance(
    record: TranCatBalanceRecord,
    amt: Money<9>,
) -> TranCatBalanceRecord {
    TranCatBalanceRecord {
        bal: (record.bal + amt).store::<9>(),
        ..record
    }
}

/// `2800-UPDATE-ACCOUNT-REC` for a posted amount `amt` (`CBTRN02C.cbl:545-559`; CBTRN02C-020,
/// CBTRN02C-021).
///
/// `ACCT-CURR-BAL` gains `amt`, `(bal + amt).store::<10>()` (ARITH-031). An amount of 0.00 or
/// more is added to the cycle credit (ARITH-032), a negative one to the cycle debit, which grows
/// more negative (ARITH-033), each stored at `S9(10)V99`; the other cycle field and every other
/// field keep the values read, as the record is rewritten whole. The `0109` reason of a rewrite
/// that ends `INVALID KEY` has no effect on any outcome (CBTRN02C-023).
#[must_use]
pub fn update_account(account: AccountRecord, amt: Money<9>) -> AccountRecord {
    let curr_bal = (account.curr_bal + amt).store::<10>();
    if amt >= Money::<9>::ZERO {
        AccountRecord {
            curr_bal,
            curr_cyc_credit: (account.curr_cyc_credit + amt).store::<10>(),
            ..account
        }
    } else {
        AccountRecord {
            curr_bal,
            curr_cyc_debit: (account.curr_cyc_debit + amt).store::<10>(),
            ..account
        }
    }
}

/// The 430-byte reject record `2500-WRITE-REJECT-REC` builds for `daily`
/// (`CBTRN02C.cbl:176-182,446-448`; `POSTTRAN.jcl:34-38`; CBTRN02C-027).
///
/// `MOVE DALYTRAN-RECORD TO REJECT-TRAN-DATA` copies the record's 350 bytes: codec A's encoding
/// of the decoded record, which equals the input line's body byte for byte for every 350-byte
/// body (i8). The trailer is the reason's four digits and its 76-byte description.
#[must_use]
pub fn reject_record(daily: &DailyTransactionRecord, reason: RejectReason) -> RejectRecord {
    // `encode` returns exactly the record's 350 bytes, so the alphanumeric MOVE copies them
    // without padding or truncation.
    let tran_data = PicX::moved_from(&daily.encode());
    RejectRecord::new(&tran_data, reason.code(), &reason.description())
}

/// What `2700-UPDATE-TCATBAL` writes to the category balance file for a posted record
/// (`CBTRN02C.cbl:467-542`; CBTRN02C-016 to CBTRN02C-019).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TcatbalWrite {
    /// The key was not found (`INVALID KEY`, status `23`): `WRITE` of the new record
    /// (CBTRN02C-017).
    Create(TranCatBalanceRecord),
    /// The key was found: `REWRITE` of the record read, its balance updated (CBTRN02C-019).
    Update(TranCatBalanceRecord),
}

impl TcatbalWrite {
    /// The record written or rewritten.
    #[must_use]
    pub const fn record(&self) -> &TranCatBalanceRecord {
        match self {
            Self::Create(record) | Self::Update(record) => record,
        }
    }

    /// The job-log line the program displays before creating a row, `None` for an update
    /// (`CBTRN02C.cbl:476-477`; CBTRN02C-016, CBTRN02C-034): [`TCATBAL_NOT_FOUND_PREFIX`], the 17
    /// key bytes (account ID, type, category) and [`TCATBAL_NOT_FOUND_SUFFIX`].
    ///
    /// The key bytes the COBOL shows come from a record area IBM leaves undefined after the
    /// failed read (halt candidate X-POSTTRAN-H2); the key looked up stands in, which is not
    /// claimed as parity. The key holds no card number (CBTRN02C-037).
    #[must_use]
    pub fn sysout_line(&self) -> Option<String> {
        let Self::Create(record) = self else {
            return None;
        };
        let mut key = Vec::with_capacity(17);
        record.acct_id.encode_into(&mut key);
        record.type_cd.encode_into(&mut key);
        record.cd.encode_into(&mut key);
        Some(format!(
            "{TCATBAL_NOT_FOUND_PREFIX}{}{TCATBAL_NOT_FOUND_SUFFIX}",
            String::from_utf8_lossy(&key)
        ))
    }
}

/// The effects of one posted record, in the order `2000-POST-TRANSACTION` performs them: the
/// category balance (`2700`), the account (`2800`), then the transaction (`2900`)
/// (`CBTRN02C.cbl:440-442`; CBTRN02C-015).
///
/// The job applies them in that order, each as its own committed write that a later failure
/// does not undo, as VSAM keeps every completed write (CBTRN02C-015, X-POSTTRAN-1). The derived
/// `Debug` cannot disclose a card number: [`TransactionRecord`] holds it as a [`CardNumber`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Posting {
    /// The category balance created or updated (CBTRN02C-016 to CBTRN02C-019).
    pub tcatbal: TcatbalWrite,
    /// The account record as rewritten (CBTRN02C-020, CBTRN02C-021).
    pub account: AccountRecord,
    /// The transaction written under its `TRAN-ID` (CBTRN02C-012 to CBTRN02C-014,
    /// CBTRN02C-024).
    pub transaction: TransactionRecord,
}

/// The outcome of one daily transaction record (CBTRN02C-002).
///
/// The derived `Debug` cannot disclose a card number: [`Posting`] holds it as a [`CardNumber`],
/// and the `Debug` of [`RejectRecord`] omits its input bytes. Both records are boxed, as each
/// is hundreds of bytes and the two differ in size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordOutcome {
    /// Validation left the reason at 0 and the record was posted (CBTRN02C-009).
    Posted(Box<Posting>),
    /// The record was rejected; the job writes this record to the reject generation
    /// (CBTRN02C-026, CBTRN02C-027).
    Rejected(Box<RejectRecord>),
}

/// A run counter outgrew its 64-bit field. Neither counter can reach that in practice, as a run
/// of more than 18 quintillion records would be needed; the error keeps the increment checked
/// rather than wrapping. It carries no record bytes (CBTRN02C-036, CBTRN02C-037).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PosttranError {
    /// `ADD 1 TO WS-TRANSACTION-COUNT` overflowed.
    TransactionCountOverflow,
    /// `ADD 1 TO WS-REJECT-COUNT` overflowed.
    RejectCountOverflow,
}

impl fmt::Display for PosttranError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TransactionCountOverflow => "the transaction count exceeds its 64-bit counter",
            Self::RejectCountOverflow => "the reject count exceeds its 64-bit counter",
        })
    }
}

impl Error for PosttranError {}

/// The key of a category balance: `TRANCAT-ACCT-ID`, `TRANCAT-TYPE-CD`, `TRANCAT-CD`
/// (`CVTRA01Y.cpy:5-8`; CBTRN02C-016).
type CategoryKey = (PicU<11>, PicX<2>, PicU<4>);

/// One POSTTRAN run over the daily transaction records in input order (CBTRN02C-001).
///
/// It holds the files the program reads by key and the working storage the loop carries from
/// record to record. Every posting updates the held account and category balance, so a later
/// record of the run is validated and posted against the values an earlier one left, as the
/// VSAM files return them (CBTRN02C-022). The type has no `Debug`: it holds whole master files.
pub struct Posttran {
    /// The account file by `ACCT-ID` (CBTRN02C-005).
    accounts: BTreeMap<PicU<11>, AccountRecord>,
    /// The cross-reference file by `XREF-CARD-NUM`, compared bytewise (CBTRN02C-004).
    xrefs: BTreeMap<CardNumber, CardXrefRecord>,
    /// The category balance file by `FD-TRAN-CAT-KEY` (CBTRN02C-016).
    tcatbals: BTreeMap<CategoryKey, TranCatBalanceRecord>,
    /// The `FILLER` of `TRAN-CAT-BAL-RECORD`: that of the last category balance read, `None`
    /// before the first read succeeds (CBTRN02C-017, CBTRN02C-018).
    working_filler: Option<PicX<22>>,
    /// `WS-TRANSACTION-COUNT`, without its `PIC 9(09)` wrap (CBTRN02C-036).
    processed: u64,
    /// `WS-REJECT-COUNT`, without its `PIC 9(09)` wrap (CBTRN02C-036).
    rejected: u64,
}

impl Posttran {
    /// A run over the `accounts`, `xrefs` and `tcatbals` the job loads, with both counts at 0
    /// (CBTRN02C-036).
    ///
    /// Each file's key is unique; on a repeated key the first record supplied is kept, as a KSDS
    /// holds one record per key.
    #[must_use]
    pub fn new(
        accounts: impl IntoIterator<Item = AccountRecord>,
        xrefs: impl IntoIterator<Item = CardXrefRecord>,
        tcatbals: impl IntoIterator<Item = TranCatBalanceRecord>,
    ) -> Self {
        let mut by_id = BTreeMap::new();
        for account in accounts {
            by_id.entry(account.id).or_insert(account);
        }
        let mut by_card = BTreeMap::new();
        for xref in xrefs {
            by_card.entry(xref.card_num.clone()).or_insert(xref);
        }
        let mut by_key = BTreeMap::new();
        for tcatbal in tcatbals {
            let key = (tcatbal.acct_id, tcatbal.type_cd.clone(), tcatbal.cd);
            by_key.entry(key).or_insert(tcatbal);
        }
        Self {
            accounts: by_id,
            xrefs: by_card,
            tcatbals: by_key,
            working_filler: None,
            processed: 0,
            rejected: 0,
        }
    }

    /// One iteration of the main loop for `daily`, the next record in input order
    /// (`CBTRN02C.cbl:202-219`; CBTRN02C-002).
    ///
    /// The transaction count advances, then the record is validated against the current state
    /// ([`validate`]). A valid record is posted: its [`Posting`] is applied to the held account
    /// and category balance, and `clock` is read once for `TRAN-PROC-TS` (CBTRN02C-013,
    /// CBTRN02C-022). A rejected record advances the reject count and changes nothing else; the
    /// clock is not read (CBTRN02C-026).
    ///
    /// # Errors
    ///
    /// [`PosttranError::TransactionCountOverflow`] or [`PosttranError::RejectCountOverflow`]
    /// when a count would pass `u64::MAX`; the record is then neither posted nor rejected.
    pub fn process(
        &mut self,
        daily: &DailyTransactionRecord,
        clock: &dyn Clock,
    ) -> Result<RecordOutcome, PosttranError> {
        self.processed = self
            .processed
            .checked_add(1)
            .ok_or(PosttranError::TransactionCountOverflow)?;
        let outcome = match self.validated_account(daily) {
            Ok(account) => RecordOutcome::Posted(Box::new(self.post(daily, account, clock))),
            Err(reason) => {
                self.rejected = self
                    .rejected
                    .checked_add(1)
                    .ok_or(PosttranError::RejectCountOverflow)?;
                RecordOutcome::Rejected(Box::new(reject_record(daily, reason)))
            }
        };
        Ok(outcome)
    }

    /// The end of the daily transaction file: the counts the run reached (CBTRN02C-034 to
    /// CBTRN02C-036).
    #[must_use]
    pub fn finish(self) -> PosttranSummary {
        PosttranSummary {
            processed: self.processed,
            rejected: self.rejected,
        }
    }

    /// `1500-A-LOOKUP-XREF` and `1500-B-LOOKUP-ACCT` against the held files, then
    /// [`validate`]: the account read, or the reason the record is rejected (CBTRN02C-003 to
    /// CBTRN02C-009).
    fn validated_account(
        &self,
        daily: &DailyTransactionRecord,
    ) -> Result<AccountRecord, RejectReason> {
        let xref = self.xrefs.get(&daily.card_num);
        let account = xref.and_then(|xref| self.accounts.get(&xref.acct_id));
        match (validate(daily, xref, account), account) {
            (Validation::Valid, Some(account)) => Ok(account.clone()),
            (Validation::Rejected(reason), _) => Err(reason),
            // `validate` is valid only for a found account; without one its own reason applies.
            (Validation::Valid, None) => Err(RejectReason::AccountNotFound),
        }
    }

    /// `2000-POST-TRANSACTION` for `daily` against `account`, the account its cross-reference
    /// names; the held files take the posted values (CBTRN02C-012 to CBTRN02C-022).
    fn post(
        &mut self,
        daily: &DailyTransactionRecord,
        account: AccountRecord,
        clock: &dyn Clock,
    ) -> Posting {
        let transaction = transaction_record(daily, clock);
        let tcatbal = self.post_category_balance(account.id, daily);
        let account = update_account(account, daily.amt);
        self.accounts.insert(account.id, account.clone());
        Posting {
            tcatbal,
            account,
            transaction,
        }
    }

    /// `2700-UPDATE-TCATBAL` for the key (`XREF-ACCT-ID`, `DALYTRAN-TYPE-CD`,
    /// `DALYTRAN-CAT-CD`): a found record is read into the working record, whose `FILLER` it
    /// replaces, and updated; a missing one is created from the working record (CBTRN02C-016 to
    /// CBTRN02C-019). The held file takes the written record.
    fn post_category_balance(
        &mut self,
        acct_id: PicU<11>,
        daily: &DailyTransactionRecord,
    ) -> TcatbalWrite {
        let key = (acct_id, daily.type_cd.clone(), daily.cat_cd);
        let write = match self.tcatbals.get(&key) {
            Some(found) => {
                self.working_filler = Some(found.filler.clone());
                TcatbalWrite::Update(add_to_category_balance(found.clone(), daily.amt))
            }
            None => TcatbalWrite::Create(new_category_balance(
                acct_id,
                daily,
                self.working_filler.as_ref(),
            )),
        };
        self.tcatbals.insert(key, write.record().clone());
        write
    }
}

/// The counts of a run that reached the end of the daily transaction file (CBTRN02C-034 to
/// CBTRN02C-036).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PosttranSummary {
    /// Records read.
    processed: u64,
    /// Records rejected.
    rejected: u64,
}

impl PosttranSummary {
    /// Records read, `WS-TRANSACTION-COUNT` without its `PIC 9(09)` wrap (CBTRN02C-036).
    #[must_use]
    pub const fn processed(&self) -> u64 {
        self.processed
    }

    /// Records rejected, `WS-REJECT-COUNT` without its `PIC 9(09)` wrap (CBTRN02C-036).
    #[must_use]
    pub const fn rejected(&self) -> u64 {
        self.rejected
    }

    /// Records posted: those read and not rejected (CBTRN02C-002).
    #[must_use]
    pub const fn posted(&self) -> u64 {
        // Every rejected record was first counted as read, so the difference never underflows.
        self.processed.saturating_sub(self.rejected)
    }

    /// [`REJECTS_RETURN_CODE`] when any record was rejected, otherwise 0, the `RETURN-CODE` the
    /// program never sets (`CBTRN02C.cbl:229-234`; CBTRN02C-035).
    #[must_use]
    pub const fn return_code(&self) -> u8 {
        if self.rejected > 0 {
            REJECTS_RETURN_CODE
        } else {
            0
        }
    }

    /// The job-log lines written after the six files are closed, in order: the transaction
    /// count, the reject count and [`END_LINE`] (`CBTRN02C.cbl:227-232`; CBTRN02C-034). Each count
    /// follows its 24-character label as nine digits with leading zeros, so an empty run shows
    /// `TRANSACTIONS PROCESSED :000000000` (CBTRN02C-029).
    #[must_use]
    pub fn sysout_lines(&self) -> [String; 3] {
        [
            count_line(PROCESSED_LABEL, self.processed),
            count_line(REJECTED_LABEL, self.rejected),
            END_LINE.to_owned(),
        ]
    }
}

/// `DISPLAY label count` for a `PIC 9(09)` count: the label, then nine digits with leading zeros.
fn count_line(label: &str, count: u64) -> String {
    // TRUNCATION: `ADD 1` without `ON SIZE ERROR` keeps the low nine digits of a `PIC 9(09)`
    // count past 999,999,999, so the 64-bit count renders modulo 10^9 (CBTRN02C-036).
    let digits = count % COUNT_MODULUS;
    format!("{label}{digits:09}")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error as StdError;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::framing::{decode_all, lines};
    use copybook::{FixedRecord, PicDate, PicTimestamp, PicU, PicX, TsForm};

    use super::{
        AccountRecord, CardXrefRecord, DailyTransactionRecord, END_LINE, PROCESSED_LABEL, Posting,
        Posttran, PosttranSummary, REJECTS_RETURN_CODE, RecordOutcome, RejectReason, START_LINE,
        TCATBAL_NOT_FOUND_PREFIX, TCATBAL_NOT_FOUND_SUFFIX, TcatbalWrite, TranCatBalanceRecord,
        Validation, add_to_category_balance, count_line, new_category_balance, reject_record,
        temp_balance, transaction_record, update_account, validate,
    };
    use crate::clock::{Clock, FixedClock};
    use crate::money::{Decimal, Money};
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A card number on no fixture line, so no synthetic record holds a fixture card number.
    const CARD: [u8; 16] = *b"4000002000003000";

    /// `DALYTRAN-ORIG-TS` of every fixture record, layout `I` (FIXFRM-017).
    const RECEIVED: &[u8; 26] = b"2022-06-10 19:27:53.000000";

    /// The account every synthetic record posts to.
    const ACCT: u64 = 7;

    /// The 38 `dailytran.txt` lines FIXPOST-902 lists as rejected, in input order.
    const REJECTED_LINES: [usize; 38] = [
        16, 32, 35, 43, 50, 66, 79, 89, 101, 129, 155, 158, 159, 162, 173, 176, 178, 181, 190, 208,
        223, 224, 225, 227, 231, 234, 235, 240, 242, 253, 267, 271, 272, 273, 280, 283, 292, 299,
    ];

    /// Offset and end of `TRAN-PROC-TS` in `TRAN-RECORD` (CVTRA05Y-013).
    const PROC_TS: core::ops::Range<usize> = 304..330;

    fn money<const N: u32>(cents: i64) -> Result<Money<N>, Box<dyn StdError>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    fn date(text: &[u8; 10]) -> Result<PicDate, Box<dyn StdError>> {
        Ok(PicDate::decode(text)?)
    }

    /// The `FixedClock` instant of the `POSTED` scenario of `fixtures.md`.
    fn posting_instant() -> Result<NaiveDateTime, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(2022, 7, 1)
            .and_then(|day| day.and_hms_opt(12, 0, 0))
            .ok_or_else(|| "2022-07-01 12:00:00 is a valid instant".into())
    }

    fn daily(
        amt_cents: i64,
        orig_ts: &[u8; 26],
    ) -> Result<DailyTransactionRecord, Box<dyn StdError>> {
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

    fn with_type(mut record: DailyTransactionRecord, type_cd: [u8; 2]) -> DailyTransactionRecord {
        record.type_cd = PicX::from_array(type_cd);
        record
    }

    fn account(
        limit_cents: i64,
        credit_cents: i64,
        debit_cents: i64,
        expiraion_date: PicDate,
    ) -> Result<AccountRecord, Box<dyn StdError>> {
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

    /// An account with room for every synthetic amount and an expiration after every record.
    fn open_account() -> Result<AccountRecord, Box<dyn StdError>> {
        account(1_000_000, 0, 0, date(b"2025-12-31")?)
    }

    fn xref() -> Result<CardXrefRecord, Box<dyn StdError>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(CARD)),
            cust_id: PicU::new(ACCT)?,
            acct_id: PicU::new(ACCT)?,
            filler: PicX::spaces(),
        })
    }

    fn tcatbal(
        type_cd: [u8; 2],
        bal_cents: i64,
        filler: [u8; 22],
    ) -> Result<TranCatBalanceRecord, Box<dyn StdError>> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(ACCT)?,
            type_cd: PicX::from_array(type_cd),
            cd: PicU::new(1)?,
            bal: money(bal_cents)?,
            filler: PicX::from_array(filler),
        })
    }

    fn posted(outcome: RecordOutcome) -> Result<Posting, Box<dyn StdError>> {
        match outcome {
            RecordOutcome::Posted(posting) => Ok(*posting),
            RecordOutcome::Rejected(reject) => {
                Err(format!("rejected with reason {:?}", reject.reason).into())
            }
        }
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

    // audit: CBTRN02C-004
    // audit: CBTRN02C-005
    // audit: CBTRN02C-007
    // audit: CBTRN02C-008
    // audit: CBTRN02C-027
    #[test]
    fn each_reason_carries_its_code_and_its_space_padded_literal() -> TestResult {
        let cases = [
            (RejectReason::InvalidCard, 100, "INVALID CARD NUMBER FOUND"),
            (
                RejectReason::AccountNotFound,
                101,
                "ACCOUNT RECORD NOT FOUND",
            ),
            (RejectReason::Overlimit, 102, "OVERLIMIT TRANSACTION"),
            (
                RejectReason::AfterExpiration,
                103,
                "TRANSACTION RECEIVED AFTER ACCT EXPIRATION",
            ),
        ];
        for (reason, code, text) in cases {
            assert_eq!(reason.code().get(), code, "{text}");
            assert_eq!(reason.text(), text);
            let description = reason.description();
            let (head, tail) = description
                .as_bytes()
                .split_at_checked(text.len())
                .ok_or("every literal fits PIC X(76)")?;
            assert_eq!(head, text.as_bytes());
            assert!(tail.iter().all(|&byte| byte == b' '), "{text}");
        }
        let overlimit = RejectReason::Overlimit.description();
        assert_eq!(overlimit.trim_end().len(), 21);
        assert_eq!(overlimit.as_bytes().len() - overlimit.trim_end().len(), 55);
        Ok(())
    }

    // audit: CBTRN02C-003
    // audit: CBTRN02C-004
    // audit: CBTRN02C-005
    // audit: CBTRN02C-007
    // audit: CBTRN02C-008
    // audit: CBTRN02C-009
    // audit: ARITH-026
    #[test]
    fn validation_runs_in_program_order_and_the_expiry_overwrites_the_limit() -> TestResult {
        // Credit 1000.00, debit -250.00 and amount 100.00 store a temp of 1350.00 (ARITH-023).
        let record = daily(10_000, RECEIVED)?;
        let after = date(b"2023-01-06")?;
        let before = date(b"2022-06-09")?;
        let within = account(135_000, 100_000, -25_000, after)?;
        let over = account(134_999, 100_000, -25_000, after)?;
        let expired = account(135_000, 100_000, -25_000, before)?;
        let both = account(134_999, 100_000, -25_000, before)?;
        let on_expiry = account(135_000, 100_000, -25_000, date(b"2022-06-10")?)?;
        let xref = xref()?;
        let rejected = Validation::Rejected;
        let cases = [
            (
                "no cross-reference, account not consulted",
                None,
                Some(&within),
                rejected(RejectReason::InvalidCard),
            ),
            (
                "no account",
                Some(&xref),
                None,
                rejected(RejectReason::AccountNotFound),
            ),
            (
                "limit equal to temp",
                Some(&xref),
                Some(&within),
                Validation::Valid,
            ),
            (
                "limit below temp",
                Some(&xref),
                Some(&over),
                rejected(RejectReason::Overlimit),
            ),
            (
                "expired only",
                Some(&xref),
                Some(&expired),
                rejected(RejectReason::AfterExpiration),
            ),
            (
                "both, 0102 overwritten (E09)",
                Some(&xref),
                Some(&both),
                rejected(RejectReason::AfterExpiration),
            ),
            (
                "received on the expiration date",
                Some(&xref),
                Some(&on_expiry),
                Validation::Valid,
            ),
        ];
        for (case, xref, account, expected) in cases {
            assert_eq!(validate(&record, xref, account), expected, "{case}");
        }
        Ok(())
    }

    // audit: CBTRN02C-010
    #[test]
    fn the_expiry_test_orders_blank_values_below_every_date() -> TestResult {
        let iso = daily(100, RECEIVED)?;
        let db2 = daily(100, b"2022-06-10-19.27.53.000000")?;
        let screen = daily(100, b"2022-06-10                ")?;
        let blank = daily(100, &[b' '; 26])?;
        let blank_exp = account(1_000_000, 0, 0, PicDate::BLANK)?;
        let same_day = account(1_000_000, 0, 0, date(b"2022-06-10")?)?;
        let day_before = account(1_000_000, 0, 0, date(b"2022-06-09")?)?;
        let xref = xref()?;
        let after = Validation::Rejected(RejectReason::AfterExpiration);
        let cases = [
            ("blank expiration, dated record", &iso, &blank_exp, after),
            (
                "blank timestamp, dated expiration",
                &blank,
                &day_before,
                Validation::Valid,
            ),
            ("both blank", &blank, &blank_exp, Validation::Valid),
            (
                "layout D on the expiration date",
                &db2,
                &same_day,
                Validation::Valid,
            ),
            (
                "layout D after the expiration date",
                &db2,
                &day_before,
                after,
            ),
            (
                "layout S on the expiration date",
                &screen,
                &same_day,
                Validation::Valid,
            ),
            (
                "layout S after the expiration date",
                &screen,
                &day_before,
                after,
            ),
        ];
        for (case, record, account, expected) in cases {
            assert_eq!(
                validate(record, Some(&xref), Some(account)),
                expected,
                "{case}"
            );
        }
        Ok(())
    }

    // audit: CBTRN02C-006
    // audit: ARITH-023
    // audit: ARITH-024
    // audit: ARITH-025
    #[test]
    fn the_temp_balance_stores_into_nine_integer_digits() -> TestResult {
        let exp = date(b"2025-12-31")?;
        let cases = [
            ("ARITH-023", 100_000, -25_000, 10_000, 135_000),
            ("ARITH-024", 50_000, -20_000, -5_000, 65_000),
            // E10: 1500000100.00 loses its high-order digit.
            ("ARITH-025", 150_000_000_000, 0, 10_000, 50_000_010_000),
        ];
        for (row, credit, debit, amt, expected) in cases {
            let account = account(60_000_000_000, credit, debit, exp)?;
            let record = daily(amt, RECEIVED)?;
            assert_eq!(
                temp_balance(&account, &record),
                money::<9>(expected)?,
                "{row}"
            );
        }
        // The truncated temp passes the 600000000.00 limit although the exact sum exceeds it.
        let account = account(60_000_000_000, 150_000_000_000, 0, exp)?;
        let record = daily(10_000, RECEIVED)?;
        assert_eq!(
            validate(&record, Some(&xref()?), Some(&account)),
            Validation::Valid
        );
        Ok(())
    }

    // audit: CBTRN02C-012
    // audit: CBTRN02C-013
    // audit: ARITH-027
    #[test]
    fn a_posted_transaction_moves_twelve_fields_and_stamps_the_db2_layout() -> TestResult {
        let record = daily(-1_234, RECEIVED)?;
        let at = NaiveDate::from_ymd_opt(2022, 7, 1)
            .and_then(|day| day.and_hms_micro_opt(12, 34, 56, 789_123))
            .ok_or("a valid instant")?;
        let transaction = transaction_record(&record, &FixedClock::new(at));
        assert_eq!(transaction.id, record.id);
        assert_eq!(transaction.type_cd, record.type_cd);
        assert_eq!(transaction.cat_cd, record.cat_cd);
        assert_eq!(transaction.source, record.source);
        assert_eq!(transaction.desc, record.desc);
        assert_eq!(transaction.amt, money::<9>(-1_234)?);
        assert_eq!(transaction.merchant_id, record.merchant_id);
        assert_eq!(transaction.merchant_name, record.merchant_name);
        assert_eq!(transaction.merchant_city, record.merchant_city);
        assert_eq!(transaction.merchant_zip, record.merchant_zip);
        assert!(transaction.card_num == record.card_num, "card number moved");
        assert_eq!(transaction.orig_ts, record.orig_ts);
        assert_eq!(
            transaction.orig_ts.value().map(|ts| ts.form()),
            Some(TsForm::Iso)
        );
        assert_eq!(
            transaction.proc_ts.value().map(|ts| ts.form()),
            Some(TsForm::Db2)
        );
        let bytes = transaction.encode();
        assert_eq!(
            bytes.get(PROC_TS),
            Some(b"2022-07-01-12.34.56.780000".as_slice())
        );
        Ok(())
    }

    // audit: CBTRN02C-016
    // audit: CBTRN02C-017
    // audit: CBTRN02C-019
    // audit: ARITH-028
    // audit: ARITH-029
    #[test]
    fn a_created_balance_takes_the_filler_of_the_last_balance_read() -> TestResult {
        let zeros = [b'0'; 22];
        let mut run = Posttran::new(
            [open_account()?],
            [xref()?],
            [tcatbal(*b"01", 10_000, zeros)?],
        );
        let clock = FixedClock::new(posting_instant()?);

        let first = posted(run.process(&daily(2_550, RECEIVED)?, &clock)?)?;
        assert_eq!(
            first.tcatbal,
            TcatbalWrite::Update(tcatbal(*b"01", 12_550, zeros)?)
        );
        assert_eq!(first.tcatbal.sysout_line(), None);

        // X-FIXTURES-1: INITIALIZE leaves FILLER, which holds the row read for `first`.
        let second = posted(run.process(&with_type(daily(-1_234, RECEIVED)?, *b"03"), &clock)?)?;
        assert_eq!(
            second.tcatbal,
            TcatbalWrite::Create(tcatbal(*b"03", -1_234, zeros)?)
        );

        // The created row is found by the next record of its key and updated.
        let third = posted(run.process(&with_type(daily(5_000, RECEIVED)?, *b"03"), &clock)?)?;
        assert_eq!(
            third.tcatbal,
            TcatbalWrite::Update(tcatbal(*b"03", 3_766, zeros)?)
        );

        // ARITH-029: a negative amount below the balance.
        let fourth = posted(run.process(&daily(-13_000, RECEIVED)?, &clock)?)?;
        assert_eq!(
            fourth.tcatbal,
            TcatbalWrite::Update(tcatbal(*b"01", -450, zeros)?)
        );
        Ok(())
    }

    // audit: CBTRN02C-017
    // audit: ARITH-028
    #[test]
    fn a_created_balance_holds_its_key_and_the_amount() -> TestResult {
        let record = with_type(daily(5_000, RECEIVED)?, *b"03");
        let filler = PicX::from_array([b'0'; 22]);
        let created = new_category_balance(PicU::new(ACCT)?, &record, Some(&filler));
        assert_eq!(created, tcatbal(*b"03", 5_000, [b'0'; 22])?);

        // A run whose first access creates its row: the keys and balance are asserted; the
        // FILLER is the HALTED(H2) stand-in of CBTRN02C-018 and is not.
        let mut run = Posttran::new([open_account()?], [xref()?], Vec::new());
        let clock = FixedClock::new(posting_instant()?);
        let posting = posted(run.process(&daily(-1_234, RECEIVED)?, &clock)?)?;
        let TcatbalWrite::Create(row) = posting.tcatbal else {
            return Err("the first access creates its row".into());
        };
        assert_eq!(row.acct_id.get(), ACCT);
        assert_eq!(row.type_cd.as_bytes(), b"01");
        assert_eq!(row.cd.get(), 1);
        assert_eq!(row.bal, money::<9>(-1_234)?);
        Ok(())
    }

    // audit: CBTRN02C-019
    // audit: ARITH-030
    #[test]
    fn a_category_balance_drops_the_digits_beyond_nine() -> TestResult {
        let top = tcatbal(*b"01", 99_999_999_999, [b' '; 22])?;
        let wrapped = add_to_category_balance(top, money(1)?);
        assert_eq!(wrapped.bal, money::<9>(0)?);
        let bottom = tcatbal(*b"01", -99_999_999_999, [b' '; 22])?;
        let wrapped = add_to_category_balance(bottom, money(-100)?);
        assert_eq!(wrapped.bal, money::<9>(-99)?);
        assert_eq!(wrapped.filler, PicX::spaces());
        Ok(())
    }

    // audit: CBTRN02C-020
    // audit: CBTRN02C-021
    // audit: ARITH-031
    // audit: ARITH-032
    // audit: ARITH-033
    #[test]
    fn the_account_takes_the_amount_into_its_balance_and_one_cycle_field() -> TestResult {
        let exp = date(b"2024-12-13")?;
        let base = AccountRecord {
            curr_bal: money(84_300)?,
            ..account(206_500, 100_000, -25_000, exp)?
        };
        let credit = update_account(base.clone(), money(10_000)?);
        assert_eq!(credit.curr_bal, money::<10>(94_300)?);
        assert_eq!(credit.curr_cyc_credit, money::<10>(110_000)?);
        assert_eq!(credit.curr_cyc_debit, money::<10>(-25_000)?);

        let zero = update_account(base.clone(), money(0)?);
        assert_eq!(
            zero, base,
            "0.00 takes the credit branch and changes nothing"
        );

        let debit = update_account(base.clone(), money(-5_000)?);
        assert_eq!(debit.curr_bal, money::<10>(79_300)?);
        assert_eq!(debit.curr_cyc_credit, money::<10>(100_000)?);
        assert_eq!(debit.curr_cyc_debit, money::<10>(-30_000)?);
        assert_eq!(
            AccountRecord {
                curr_bal: base.curr_bal,
                curr_cyc_debit: base.curr_cyc_debit,
                ..debit
            },
            base,
            "every other field keeps the value read"
        );

        let top = AccountRecord {
            curr_bal: money(999_999_999_999)?,
            ..base
        };
        assert_eq!(update_account(top, money(1)?).curr_bal, money::<10>(0)?);
        Ok(())
    }

    // audit: CBTRN02C-022
    // audit: CBTRN02C-026
    #[test]
    fn later_records_see_earlier_postings_and_a_reject_changes_nothing() -> TestResult {
        let exp = date(b"2025-12-31")?;
        let mut run = Posttran::new([account(100_000, 0, 0, exp)?], [xref()?], Vec::new());
        let clock = CountingClock {
            at: posting_instant()?,
            reads: AtomicUsize::new(0),
        };

        let first = posted(run.process(&daily(60_000, RECEIVED)?, &clock)?)?;
        assert_eq!(first.account.curr_cyc_credit, money::<10>(60_000)?);
        assert_eq!(
            clock.reads.load(Ordering::SeqCst),
            1,
            "one reading per posted record"
        );

        // 600.00 + 500.00 exceeds the 1000.00 limit only because the first posting is held.
        let RecordOutcome::Rejected(reject) = run.process(&daily(50_000, RECEIVED)?, &clock)?
        else {
            return Err("the second record is over the limit".into());
        };
        assert_eq!(reject.reason, RejectReason::Overlimit.code());
        assert_eq!(
            clock.reads.load(Ordering::SeqCst),
            1,
            "a reject reads no clock"
        );

        let third = posted(run.process(&daily(40_000, RECEIVED)?, &clock)?)?;
        assert_eq!(third.account.curr_cyc_credit, money::<10>(100_000)?);
        assert_eq!(third.account.curr_bal, money::<10>(119_300)?);
        assert_eq!(
            third.tcatbal,
            TcatbalWrite::Update(TranCatBalanceRecord {
                bal: money(100_000)?,
                ..first.tcatbal.record().clone()
            })
        );
        assert_eq!(clock.reads.load(Ordering::SeqCst), 2);
        assert_eq!(run.finish().rejected(), 1);
        Ok(())
    }

    // audit: CBTRN02C-027
    #[test]
    fn a_reject_record_is_the_input_bytes_then_the_reason_and_its_description() -> TestResult {
        let record = daily(10_000, RECEIVED)?;
        let bytes = reject_record(&record, RejectReason::Overlimit).encode();
        assert_eq!(bytes.len(), 430);
        let input = record.encode();
        assert!(
            bytes.get(..350) == Some(input.as_slice()),
            "bytes 1-350 are the record"
        );
        assert_eq!(bytes.get(350..354), Some(b"0102".as_slice()));
        let mut description = b"OVERLIMIT TRANSACTION".to_vec();
        description.resize(76, b' ');
        assert_eq!(bytes.get(354..), Some(description.as_slice()));
        Ok(())
    }

    // audit: CBTRN02C-016
    // audit: CBTRN02C-034
    #[test]
    fn a_created_balance_shows_the_not_found_line_and_an_update_none() -> TestResult {
        let created = TcatbalWrite::Create(tcatbal(*b"03", 100, [b' '; 22])?);
        let line = created.sysout_line().ok_or("a created row has a line")?;
        // The 17 key bytes between the literals are the halt candidate X-POSTTRAN-H2.
        assert!(line.starts_with(TCATBAL_NOT_FOUND_PREFIX));
        assert!(line.ends_with(TCATBAL_NOT_FOUND_SUFFIX));
        assert_eq!(TCATBAL_NOT_FOUND_PREFIX.len(), 35);
        assert_eq!(line.len(), 35 + 17 + TCATBAL_NOT_FOUND_SUFFIX.len());
        let updated = TcatbalWrite::Update(tcatbal(*b"01", 100, [b' '; 22])?);
        assert_eq!(updated.sysout_line(), None);
        Ok(())
    }

    // audit: CBTRN02C-029
    // audit: CBTRN02C-034
    // audit: CBTRN02C-035
    // audit: CBTRN02C-036
    #[test]
    fn the_summary_counts_records_and_sets_the_return_code() -> TestResult {
        assert_eq!(START_LINE, "START OF EXECUTION OF PROGRAM CBTRN02C");
        let empty = Posttran::new(
            Vec::<AccountRecord>::new(),
            Vec::<CardXrefRecord>::new(),
            Vec::<TranCatBalanceRecord>::new(),
        )
        .finish();
        assert_eq!(
            (empty.processed(), empty.rejected(), empty.posted()),
            (0, 0, 0)
        );
        assert_eq!(empty.return_code(), 0);
        assert_eq!(
            empty.sysout_lines(),
            [
                "TRANSACTIONS PROCESSED :000000000".to_owned(),
                "TRANSACTIONS REJECTED  :000000000".to_owned(),
                END_LINE.to_owned(),
            ]
        );

        // A card with no cross-reference is rejected; the next record is posted.
        let mut run = Posttran::new([open_account()?], [xref()?], Vec::new());
        let clock = FixedClock::new(posting_instant()?);
        let mut stranger = daily(100, RECEIVED)?;
        stranger.card_num = CardNumber::new(PicX::from_array(*b"4000002000003001"));
        let RecordOutcome::Rejected(reject) = run.process(&stranger, &clock)? else {
            return Err("an unknown card is rejected".into());
        };
        assert_eq!(reject.reason, RejectReason::InvalidCard.code());
        posted(run.process(&daily(100, RECEIVED)?, &clock)?)?;
        let summary: PosttranSummary = run.finish();
        assert_eq!(
            (summary.processed(), summary.rejected(), summary.posted()),
            (2, 1, 1)
        );
        assert_eq!(summary.return_code(), REJECTS_RETURN_CODE);
        assert_eq!(
            summary.sysout_lines(),
            [
                "TRANSACTIONS PROCESSED :000000002".to_owned(),
                "TRANSACTIONS REJECTED  :000000001".to_owned(),
                END_LINE.to_owned(),
            ]
        );
        // A count past 999,999,999 shows its low nine digits.
        assert_eq!(
            count_line(PROCESSED_LABEL, 1_000_000_123),
            "TRANSACTIONS PROCESSED :000000123"
        );
        Ok(())
    }

    fn fixture_path(file: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../app/data/ASCII")
            .join(file)
    }

    fn fixture<R: FixedRecord>(file: &str) -> Result<Vec<R>, Box<dyn StdError>> {
        let records = decode_all::<R>(&fs::read(fixture_path(file))?)?;
        Ok(records.into_iter().map(|framed| framed.record).collect())
    }

    /// The `POSTED` scenario of `fixtures.md`: every outcome in input order, with each input
    /// line's body.
    struct PostedScenario {
        outcomes: Vec<RecordOutcome>,
        bodies: Vec<Vec<u8>>,
        summary: PosttranSummary,
    }

    fn posted_scenario() -> Result<PostedScenario, Box<dyn StdError>> {
        let mut run = Posttran::new(
            fixture::<AccountRecord>("acctdata.txt")?,
            fixture::<CardXrefRecord>("cardxref.txt")?,
            fixture::<TranCatBalanceRecord>("tcatbal.txt")?,
        );
        let input = fs::read(fixture_path("dailytran.txt"))?;
        let bodies = lines(&input).map(|line| line.body.to_vec()).collect();
        let clock = FixedClock::new(posting_instant()?);
        let outcomes = decode_all::<DailyTransactionRecord>(&input)?
            .iter()
            .map(|framed| run.process(&framed.record, &clock))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PostedScenario {
            outcomes,
            bodies,
            summary: run.finish(),
        })
    }

    // audit: FIXPOST-902
    // audit: FIXPOST-903
    // audit: CBTRN02C-027
    #[test]
    fn the_fixture_run_rejects_the_audited_lines_as_overlimit() -> TestResult {
        let scenario = posted_scenario()?;
        assert_eq!(scenario.outcomes.len(), 300);
        assert_eq!(scenario.bodies.len(), 300);
        let mut trailer = b"0102OVERLIMIT TRANSACTION".to_vec();
        trailer.resize(80, b' ');
        let mut rejected_lines = Vec::new();
        for (index, (outcome, body)) in scenario.outcomes.iter().zip(&scenario.bodies).enumerate() {
            let line = index + 1;
            if let RecordOutcome::Rejected(reject) = outcome {
                rejected_lines.push(line);
                let bytes = reject.encode();
                assert!(
                    bytes.get(..350) == Some(body.as_slice()),
                    "FIXPOST-{line:03}: input bytes"
                );
                assert!(
                    bytes.get(350..) == Some(trailer.as_slice()),
                    "FIXPOST-{line:03}: trailer"
                );
            }
        }
        assert_eq!(rejected_lines, REJECTED_LINES);
        let summary = scenario.summary;
        assert_eq!((summary.processed(), summary.rejected()), (300, 38));
        assert_eq!(summary.return_code(), 4);
        assert_eq!(
            summary.sysout_lines(),
            [
                "TRANSACTIONS PROCESSED :000000300".to_owned(),
                "TRANSACTIONS REJECTED  :000000038".to_owned(),
                END_LINE.to_owned(),
            ]
        );
        Ok(())
    }

    /// A category balance key as plain values, for set membership.
    fn key(record: &TranCatBalanceRecord) -> (u64, [u8; 2], u64) {
        (
            record.acct_id.get(),
            *record.type_cd.as_bytes(),
            record.cd.get(),
        )
    }

    // audit: FIXPOST-001
    // audit: FIXPOST-002
    // audit: FIXPOST-901
    #[test]
    fn the_fixture_run_posts_the_audited_balances() -> TestResult {
        let scenario = posted_scenario()?;
        let postings: Vec<&Posting> = scenario
            .outcomes
            .iter()
            .filter_map(|outcome| match outcome {
                RecordOutcome::Posted(posting) => Some(posting.as_ref()),
                RecordOutcome::Rejected(_) => None,
            })
            .collect();
        assert_eq!(postings.len(), 262);
        assert_eq!(scenario.summary.posted(), 262);

        let first = postings.first().ok_or("line 1 is posted")?;
        let TcatbalWrite::Update(row) = &first.tcatbal else {
            return Err("FIXPOST-001 updates its row".into());
        };
        assert_eq!((key(row), row.bal), ((7, *b"01", 1), money::<9>(50_477)?));
        assert_eq!(first.account.curr_bal, money::<10>(69_777)?);
        assert_eq!(first.account.curr_cyc_credit, money::<10>(50_477)?);
        assert_eq!(first.account.curr_cyc_debit, money::<10>(0)?);

        let second = postings.get(1).ok_or("line 2 is posted")?;
        let TcatbalWrite::Create(row) = &second.tcatbal else {
            return Err("FIXPOST-002 creates its row".into());
        };
        assert_eq!((key(row), row.bal), ((20, *b"03", 1), money::<9>(-91_900)?));
        assert_eq!(second.account.curr_bal, money::<10>(-55_000)?);
        assert_eq!(second.account.curr_cyc_credit, money::<10>(0)?);
        assert_eq!(second.account.curr_cyc_debit, money::<10>(-91_900)?);

        let mut created = BTreeSet::new();
        let mut updated = BTreeSet::new();
        for posting in &postings {
            match &posting.tcatbal {
                TcatbalWrite::Create(row) => {
                    assert!(row.filler.as_bytes() == &[b'0'; 22], "X-FIXTURES-1 filler");
                    assert!(created.insert(key(row)), "a key is created once");
                }
                TcatbalWrite::Update(row) => {
                    updated.insert(key(row));
                }
            }
            let proc_ts = posting.transaction.encode();
            assert_eq!(
                proc_ts.get(PROC_TS),
                Some(b"2022-07-01-12.00.00.000000".as_slice())
            );
        }
        assert_eq!(created.len(), 50);
        assert!(created.iter().all(|(_, type_cd, _)| type_cd == b"03"));
        let loaded_updates = updated
            .iter()
            .filter(|(_, type_cd, _)| type_cd == b"01")
            .count();
        assert_eq!(loaded_updates, 49);
        assert!(
            !updated.contains(&(37, *b"01", 1)),
            "account 37 posts no type 01 record"
        );
        Ok(())
    }
}
