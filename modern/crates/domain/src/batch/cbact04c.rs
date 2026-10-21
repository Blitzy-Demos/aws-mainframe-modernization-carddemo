//! INTCALC, the monthly interest calculator CBACT04C, as pure rules (`app/cbl/CBACT04C.cbl`;
//! `modern/audit/batch/INTCALC.md`; AAP 0.3.3.2, 0.3.4.3).
//!
//! `batch::jobs::intcalc` reads `tran_cat_balances` in key order, feeds each record to
//! [`Intcalc::process`], writes every [`IntcalcEffect::SystemTransaction`] to the SYSTRAN
//! generation file and persists every [`IntcalcEffect::AccountUpdate`]. This module does no I/O
//! and reads no clock of its own: the job loads `accounts`, `card_xref` and `disclosure_groups`
//! into [`Intcalc::new`] and passes the injected [`Clock`].
//!
//! | Item | COBOL | Rules |
//! |---|---|---|
//! | [`ParmDate`], [`DEFAULT_PARM_DATE`] | `EXTERNAL-PARMS`, `PARM='2022071800'` | CBACT04C-001 |
//! | [`Intcalc::process`] | main loop, `1100-GET-ACCT-DATA`, `1110-GET-XREF-DATA`, `1200-GET-INTEREST-RATE`, `1200-A-GET-DEFAULT-INT-RATE`, `1300-COMPUTE-INTEREST`, `1400-COMPUTE-FEES` | CBACT04C-003 to CBACT04C-014, CBACT04C-025, CBACT04C-028 |
//! | [`systran_record`], [`interest_transaction_id`] | `1300-B-WRITE-TX`, `Z-GET-DB2-FORMAT-TIMESTAMP` | CBACT04C-015 to CBACT04C-022 |
//! | [`update_account`] | `1050-UPDATE-ACCOUNT` | CBACT04C-026 |
//! | [`Intcalc::finish`], [`IntcalcSummary`] | the end of the loop | CBACT04C-027, CBACT04C-030 |
//! | [`IntcalcError`], the `DISPLAY` literals, [`file_status_line`] | the failure paragraphs, `9910-DISPLAY-IO-STATUS`, `9999-ABEND-PROGRAM` | CBACT04C-005, CBACT04C-007, CBACT04C-008, CBACT04C-011, CBACT04C-031 to CBACT04C-038 |
//!
//! # Defects reproduced
//!
//! - E07: every fixture account has a blank `ACCT-GROUP-ID`, so every lookup misses and reads
//!   the `DEFAULT` group (CBACT04C-010, FIXINT-902).
//! - E08: the loop's `ELSE PERFORM 1050-UPDATE-ACCOUNT` is never reached, so the last account in
//!   key order is never rewritten and its interest total is discarded. [`Intcalc::finish`]
//!   returns no account update; [`IntcalcSummary::unapplied_last_account`] names that account
//!   for the job log (CBACT04C-027, ARITH-017, FIXINT-903).
//! - The open failure of the disclosure group file shows `ERROR OPENING DALY REJECTS FILE`
//!   ([`OPEN_DISCGRP_ERROR`], CBACT04C-034).
//!
//! # Where the audit settles the run
//!
//! - `--parm-date` takes any value of at least 10 printable ASCII bytes and keeps its first 10,
//!   as the COBOL keeps the first 10 characters of a longer PARM (CBACT04C-001).
//! - The six-digit `TRAN-ID` suffix wraps from 999999 to 000000 with no error, as `ADD 1` without
//!   `ON SIZE ERROR` does (CBACT04C-016).
//! - An account with two or more cross-references ends the run with the duplicate-key lines and
//!   never takes one of its cards (CBACT04C-008); that outcome is not claimed as parity.
//! - At an account break the previous account is rewritten before the next one is read, so a
//!   lookup failure there leaves that rewrite standing (CBACT04C-005, CBACT04C-038).
//!   [`IntcalcError::rewritten_before_abend`] returns it, for the job to persist before it ends.
//!
//! # Halts
//!
//! `HALTED(H2)`: bytes 25 to 100 of `TRAN-DESC` and the 20-byte `FILLER` of every SYSTRAN record
//! hold storage nothing initialises (CBACT04C-019, CBACT04C-023). The replacement writes spaces
//! there, a stand-in that is not a parity result. `HALTED(H4)`: the status digits of a failure
//! the replacement does not reproduce (CBACT04C-037).
//!
//! # Rules governing this module
//!
//! - Money: amounts change only through `crate::money`. [`monthly_interest`] is the only path
//!   from the rate to money (ARITH-001), totals store at `S9(09)V99` (ARITH-010, ARITH-011) and
//!   the balance at `S9(10)V99` (ARITH-013).
//! - Typed Dates and Timestamps: both SYSTRAN timestamps are layout-`D` [`PicTimestamp`] values
//!   from [`db2_timestamp`] (PIC-019, E33). `PARM-DATE` is not a copybook date field: the
//!   program never reads it as a date, only as the first ten bytes of `TRAN-ID`, so it stays
//!   text.
//! - Cardholder Data Redaction: SYSTRAN records carry the full `XREF-CARD-NUM` for the local
//!   SYSTRAN file only (CBACT04C-021). Card numbers are held as [`CardNumber`], whose `Debug`
//!   redacts them, and no error, display line or summary carries one.
//! - Total Parsers Over Untrusted Input: [`ParmDate::parse`] returns a value or a typed error for
//!   any input.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use copybook::{PicTimestamp, PicU, PicX};

use crate::clock::{Clock, db2_timestamp};
use crate::money::{Money, MoneyExpr, Rate, monthly_interest};
use crate::records::{
    AccountRecord, CardXrefRecord, DisclosureGroupRecord, TranCatBalanceRecord, TransactionRecord,
};
use crate::sensitive::CardNumber;

/// Bytes of `PARM-DATE PIC X(10)` (`CBACT04C.cbl:178`; CBACT04C-001).
pub const PARM_DATE_LEN: usize = 10;

/// The JCL's `PARM='2022071800'` (`INTCALC.jcl:22`), the default of `--parm-date`
/// (CBACT04C-001).
pub const DEFAULT_PARM_DATE: &str = "2022071800";

/// `'DEFAULT'` moved into `FD-DIS-ACCT-GROUP-ID PIC X(10)`, padded with three spaces: the group
/// key of the retry after a disclosure lookup misses (`CBACT04C.cbl:437`; CBACT04C-010).
pub const DEFAULT_GROUP: PicX<10> = PicX::from_array(*b"DEFAULT   ");

/// `ABCODE`, the user abend code `9999-ABEND-PROGRAM` passes to `CEE3ABD`
/// (`CBACT04C.cbl:631-632`; CBACT04C-038).
pub const ABEND_CODE: u32 = 999;

/// The first line of every run (`CBACT04C.cbl:181`; CBACT04C-002).
pub const START_LINE: &str = "START OF EXECUTION OF PROGRAM CBACT04C";

/// The last line of a run that ends normally (`CBACT04C.cbl:230`; CBACT04C-030).
pub const END_LINE: &str = "END OF EXECUTION OF PROGRAM CBACT04C";

/// The `INVALID KEY` line of the account and cross-reference reads, 19 characters ending in a
/// space, followed by the 11-digit key (`CBACT04C.cbl:375,397`; CBACT04C-005, CBACT04C-007).
pub const ACCOUNT_NOT_FOUND_PREFIX: &str = "ACCOUNT NOT FOUND: ";

/// The first `INVALID KEY` line of a disclosure lookup that misses (`CBACT04C.cbl:418`;
/// CBACT04C-010).
pub const DISCLOSURE_MISSING_LINE: &str = "DISCLOSURE GROUP RECORD MISSING";

/// The second `INVALID KEY` line of a disclosure lookup that misses (`CBACT04C.cbl:419`;
/// CBACT04C-010).
pub const TRY_DEFAULT_LINE: &str = "TRY WITH DEFAULT GROUP CODE";

/// The literal that opens every status line, `NNNN` included (`CBACT04C.cbl:642,646`;
/// CBACT04C-036).
pub const FILE_STATUS_PREFIX: &str = "FILE STATUS IS: NNNN";

/// The line `9999-ABEND-PROGRAM` shows before the abend (`CBACT04C.cbl:629`; CBACT04C-038).
pub const ABEND_LINE: &str = "ABENDING PROGRAM";

/// The failed open of the category balance file (`CBACT04C.cbl:245`; CBACT04C-034); the
/// replacement shows it for a failed `PostgreSQL` connection.
pub const OPEN_TCATBAL_ERROR: &str = "ERROR OPENING TRANSACTION CATEGORY BALANCE";

/// The failed open of the cross-reference file, followed by its two status characters
/// (`CBACT04C.cbl:263`; CBACT04C-034).
pub const OPEN_XREF_ERROR: &str = "ERROR OPENING CROSS REF FILE";

/// The failed open of the disclosure group file, which names a rejects file this program does
/// not use; reproduced as written (`CBACT04C.cbl:281`; CBACT04C-034).
pub const OPEN_DISCGRP_ERROR: &str = "ERROR OPENING DALY REJECTS FILE";

/// The failed open of the account file (`CBACT04C.cbl:300`; CBACT04C-034).
pub const OPEN_ACCOUNT_ERROR: &str = "ERROR OPENING ACCOUNT MASTER FILE";

/// The failed open of the SYSTRAN file (`CBACT04C.cbl:318`; CBACT04C-034).
pub const OPEN_TRANSACT_ERROR: &str = "ERROR OPENING TRANSACTION FILE";

/// A failed read of the category balance file (`CBACT04C.cbl:342`; CBACT04C-033).
pub const READ_TCATBAL_ERROR: &str = "ERROR READING TRANSACTION CATEGORY FILE";

/// A failed account read (`CBACT04C.cbl:386`; CBACT04C-005, CBACT04C-033).
pub const READ_ACCOUNT_ERROR: &str = "ERROR READING ACCOUNT FILE";

/// A failed cross-reference read (`CBACT04C.cbl:408`; CBACT04C-007, CBACT04C-008,
/// CBACT04C-033).
pub const READ_XREF_ERROR: &str = "ERROR READING XREF FILE";

/// A failed disclosure read other than a missing record (`CBACT04C.cbl:431`; CBACT04C-033).
pub const READ_DISCGRP_ERROR: &str = "ERROR READING DISCLOSURE GROUP FILE";

/// A failed read of the `DEFAULT` disclosure record (`CBACT04C.cbl:455`; CBACT04C-011,
/// CBACT04C-033).
pub const READ_DEFAULT_DISCGRP_ERROR: &str = "ERROR READING DEFAULT DISCLOSURE GROUP";

/// A failed write of a SYSTRAN record (`CBACT04C.cbl:510`; CBACT04C-031).
pub const WRITE_TRANSACT_ERROR: &str = "ERROR WRITING TRANSACTION RECORD";

/// A failed account rewrite (`CBACT04C.cbl:365`; CBACT04C-032).
pub const REWRITE_ACCOUNT_ERROR: &str = "ERROR RE-WRITING ACCOUNT FILE";

/// A failed close of the category balance file (`CBACT04C.cbl:533`; CBACT04C-035).
pub const CLOSE_TCATBAL_ERROR: &str = "ERROR CLOSING TRANSACTION BALANCE FILE";

/// A failed close of the cross-reference file (`CBACT04C.cbl:552`; CBACT04C-035).
pub const CLOSE_XREF_ERROR: &str = "ERROR CLOSING CROSS REF FILE";

/// A failed close of the disclosure group file (`CBACT04C.cbl:570`; CBACT04C-035).
pub const CLOSE_DISCGRP_ERROR: &str = "ERROR CLOSING DISCLOSURE GROUP FILE";

/// A failed close of the account file (`CBACT04C.cbl:588`; CBACT04C-035).
pub const CLOSE_ACCOUNT_ERROR: &str = "ERROR CLOSING ACCOUNT FILE";

/// A failed close of the SYSTRAN file (`CBACT04C.cbl:606`; CBACT04C-035).
pub const CLOSE_TRANSACT_ERROR: &str = "ERROR CLOSING TRANSACTION FILE";

/// The file status of a record not found, shown as `FILE STATUS IS: NNNN0023` (CBACT04C-036).
const STATUS_NOT_FOUND: [u8; 2] = *b"23";

/// The file status of a duplicate alternate key, shown as `FILE STATUS IS: NNNN0002`
/// (CBACT04C-008, CBACT04C-036).
const STATUS_DUPLICATE_KEY: [u8; 2] = *b"02";

/// `MOVE '01' TO TRAN-TYPE-CD` (`CBACT04C.cbl:482`; CBACT04C-017).
const INTEREST_TYPE_CD: PicX<2> = PicX::from_array(*b"01");

/// `MOVE '05' TO TRAN-CAT-CD`: the alphanumeric literal moved into `PIC 9(04)` is the integer 5,
/// written `0005` (`CBACT04C.cbl:483`; CBACT04C-017).
const INTEREST_CAT_CD: PicU<4> = match PicU::new(5) {
    Ok(cd) => cd,
    Err(_) => panic!("5 fits PIC 9(04)"),
};

/// `MOVE 'System' TO TRAN-SOURCE`, padded with four spaces to `PIC X(10)` (`CBACT04C.cbl:484`;
/// CBACT04C-017).
const INTEREST_SOURCE: PicX<10> = PicX::from_array(*b"System    ");

/// The first `STRING` operand of `TRAN-DESC`, 13 characters ending in a space
/// (`CBACT04C.cbl:485`; CBACT04C-018).
const DESC_PREFIX: &[u8; 13] = b"Int. for a/c ";

/// Bytes the `STRING` into `TRAN-DESC` writes: the prefix and the 11 digits of `ACCT-ID`
/// (CBACT04C-018).
const DESC_TEXT_LEN: usize = 24;

const _: () = assert!(
    DESC_PREFIX.len() + 11 == DESC_TEXT_LEN && PicU::<11>::MAX == 99_999_999_999,
    "TRAN-DESC receives 13 prefix bytes and the 11 digits of ACCT-ID"
);

/// The disclosure group key: group, transaction type and category, `FD-DISCGRP-KEY`
/// (`CBACT04C.cbl:78-81`).
type DisclosureKey = (PicX<10>, PicX<2>, PicU<4>);

/// `PARM-DATE PIC X(10)`, the first ten bytes of every SYSTRAN `TRAN-ID` (CBACT04C-001,
/// CBACT04C-015).
///
/// The program never validates it or reads it as a date, so it is text, not a [`PicTimestamp`]
/// or date: `ABCDEFGHIJ` gives IDs that start `ABCDEFGHIJ`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParmDate(PicX<PARM_DATE_LEN>);

impl ParmDate {
    /// The JCL's PARM, [`DEFAULT_PARM_DATE`].
    pub const DEFAULT: Self = match DEFAULT_PARM_DATE.as_bytes().first_chunk::<PARM_DATE_LEN>() {
        Some(bytes) => Self(PicX::from_array(*bytes)),
        None => panic!("the INTCALC PARM has 10 characters"),
    };

    /// The PARM text `bytes`: at least 10 bytes, every one printable ASCII (`0x20` to `0x7E`),
    /// of which the first 10 are `PARM-DATE`, as the COBOL keeps the first 10 characters of a
    /// longer PARM (CBACT04C-001). Printable ASCII is what a `PIC X(16)` `TRAN-ID` holds
    /// (PIC-001, PIC-002).
    ///
    /// # Errors
    ///
    /// [`IntcalcError::ParmDateTooShort`] for fewer than 10 bytes, whose COBOL result no source
    /// fixes; [`IntcalcError::ParmDateNotPrintable`] when any byte is a control byte or outside
    /// ASCII. The value is never padded, trimmed or normalised.
    pub fn parse(bytes: &[u8]) -> Result<Self, IntcalcError> {
        let Some(first) = bytes.first_chunk::<PARM_DATE_LEN>() else {
            return Err(IntcalcError::ParmDateTooShort { len: bytes.len() });
        };
        if bytes.iter().all(|&byte| matches!(byte, b' '..=b'~')) {
            Ok(Self(PicX::from_array(*first)))
        } else {
            Err(IntcalcError::ParmDateNotPrintable)
        }
    }

    /// The ten bytes of `PARM-DATE`.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; PARM_DATE_LEN] {
        self.0.as_bytes()
    }
}

impl Default for ParmDate {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// `STRING PARM-DATE, WS-TRANID-SUFFIX DELIMITED BY SIZE INTO TRAN-ID`: the ten PARM bytes, then
/// the suffix as six digits with leading zeros, filling `PIC X(16)` exactly; with the default
/// PARM and suffix 1, `2022071800000001` (`CBACT04C.cbl:476-480`; CBACT04C-015, FIXINT-901).
#[must_use]
pub fn interest_transaction_id(parm: &ParmDate, suffix: PicU<6>) -> PicX<16> {
    let mut id = Vec::with_capacity(PicX::<16>::WIDTH);
    id.extend_from_slice(parm.as_bytes());
    suffix.encode_into(&mut id);
    PicX::moved_from(&id)
}

/// `1050-UPDATE-ACCOUNT`: `ADD WS-TOTAL-INT TO ACCT-CURR-BAL`, stored at `S9(10)V99` with the
/// integer digits beyond ten dropped (ARITH-013), and both cycle fields set to zero (ARITH-014);
/// every other field keeps its value, as the `REWRITE` writes back the record read at the
/// account's break (`CBACT04C.cbl:350-356`; CBACT04C-026).
#[must_use]
pub fn update_account(account: AccountRecord, total: Money<9>) -> AccountRecord {
    AccountRecord {
        curr_bal: (MoneyExpr::from(account.curr_bal) + total).store::<10>(),
        curr_cyc_credit: Money::ZERO,
        curr_cyc_debit: Money::ZERO,
        ..account
    }
}

/// The SYSTRAN record `1300-B-WRITE-TX` builds for one category balance whose rate is not zero
/// (`CBACT04C.cbl:473-498`; CBACT04C-015 to CBACT04C-023, FIXINT-901).
///
/// - `TRAN-ID`: [`interest_transaction_id`] of `parm` and `suffix`, the suffix already advanced
///   for this record (CBACT04C-015).
/// - Constants: type `01`, category `0005`, source `System` and four spaces, merchant ID zero,
///   merchant name, city and ZIP spaces (CBACT04C-017).
/// - `TRAN-DESC`: `Int. for a/c ` and the 11 digits of `acct_id` (CBACT04C-018), then 76 spaces,
///   a `HALTED(H2)` stand-in (CBACT04C-019).
/// - `TRAN-AMT`: `monthly`, copied between equal pictures (CBACT04C-020, ARITH-012).
/// - `TRAN-CARD-NUM`: the account's cross-referenced card (CBACT04C-021).
/// - `TRAN-ORIG-TS` and `TRAN-PROC-TS`: one reading of `clock`, as `Z-GET-DB2-FORMAT-TIMESTAMP`
///   runs once per record, in layout `D` with the fraction cut to hundredths (CBACT04C-022,
///   PIC-019).
/// - `FILLER`: 20 spaces, a `HALTED(H2)` stand-in (CBACT04C-023).
#[must_use]
pub fn systran_record(
    parm: &ParmDate,
    suffix: PicU<6>,
    acct_id: PicU<11>,
    card_num: &CardNumber,
    monthly: Money<9>,
    clock: &dyn Clock,
) -> TransactionRecord {
    let stamp = PicTimestamp::new(Some(db2_timestamp(clock.now())));
    TransactionRecord {
        id: interest_transaction_id(parm, suffix),
        type_cd: INTEREST_TYPE_CD,
        cat_cd: INTEREST_CAT_CD,
        source: INTEREST_SOURCE,
        desc: interest_description(acct_id),
        amt: monthly,
        merchant_id: PicU::ZERO,
        merchant_name: PicX::spaces(),
        merchant_city: PicX::spaces(),
        merchant_zip: PicX::spaces(),
        card_num: card_num.clone(),
        orig_ts: stamp,
        proc_ts: stamp,
        // HALTED(H2): no statement writes this FILLER and nothing initialises TRAN-RECORD, so
        // its COBOL bytes are unfixed storage; spaces stand in (CBACT04C-023).
        filler: PicX::spaces(),
    }
}

/// `STRING 'Int. for a/c ', ACCT-ID DELIMITED BY SIZE INTO TRAN-DESC`: 24 bytes, then the rest
/// of `PIC X(100)` (`CBACT04C.cbl:485-489`; CBACT04C-018).
fn interest_description(acct_id: PicU<11>) -> PicX<100> {
    let mut desc = Vec::with_capacity(DESC_TEXT_LEN);
    desc.extend_from_slice(DESC_PREFIX);
    acct_id.encode_into(&mut desc);
    // HALTED(H2): `STRING` writes bytes 1-24 only and nothing initialises TRAN-RECORD, so bytes
    // 25-100 hold unfixed storage in the COBOL; the space padding of `moved_from` stands in
    // (CBACT04C-019).
    PicX::moved_from(&desc)
}

/// `ADD 1 TO WS-TRANID-SUFFIX` on `PIC 9(06)` (`CBACT04C.cbl:173,474`; CBACT04C-016).
fn next_suffix(suffix: PicU<6>) -> PicU<6> {
    // TRUNCATION: `ADD 1` without `ON SIZE ERROR` keeps the low six digits of 1000000, so the
    // suffix wraps from 999999 to 000000 and later records repeat IDs already written in the
    // run (CBACT04C-016). `succ` fails only at 999999, the one value that wraps.
    match suffix.succ() {
        Ok(next) => next,
        Err(_) => PicU::ZERO,
    }
}

/// `9910-DISPLAY-IO-STATUS`: [`FILE_STATUS_PREFIX`] followed by the four characters of
/// `IO-STATUS-04` for the two-character file status `status` (`CBACT04C.cbl:635-648`).
///
/// A numeric status that does not start with `9` shows `00` and the status, so a record not
/// found shows `FILE STATUS IS: NNNN0023` and a duplicate key `FILE STATUS IS: NNNN0002`
/// (CBACT04C-036). Any other status shows its first character and the binary value of its
/// second as three digits; those characters are `HALTED(H4)`, a VSAM or runtime status the
/// replacement does not reproduce, so they are not a parity result (CBACT04C-037).
#[must_use]
pub fn file_status_line(status: [u8; 2]) -> String {
    let [first, second] = status;
    let mut line = String::from(FILE_STATUS_PREFIX);
    if first.is_ascii_digit() && second.is_ascii_digit() && first != b'9' {
        line.push_str("00");
        line.push(char::from(first));
        line.push(char::from(second));
    } else {
        // HALTED(H4): `MOVE IO-STAT1 TO IO-STATUS-04(1:1)`, then the second status byte as the
        // low-order byte of `TWO-BYTES-BINARY`, moved into `IO-STATUS-0403 PIC 999`.
        line.push(char::from(first));
        line.push_str(&format!("{second:03}"));
    }
    line
}

/// One effect of a category balance record, in program order; the job applies them in the order
/// [`Intcalc::process`] returns them.
///
/// The derived `Debug` cannot disclose a card number: [`TransactionRecord`] holds it as a
/// [`CardNumber`], whose `Debug` redacts it (PIC-024).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntcalcEffect {
    /// `REWRITE FD-ACCTFILE-REC FROM ACCOUNT-RECORD` of the previous account at an account break,
    /// after [`update_account`] (CBACT04C-026). Replacement: update that `accounts` row.
    AccountUpdate(AccountRecord),
    /// `WRITE FD-TRANFILE-REC FROM TRAN-RECORD`: one 350-byte SYSTRAN record, built by
    /// [`systran_record`] (CBACT04C-024). Replacement: append its codec A bytes and LF to the
    /// SYSTRAN generation; never insert it into `transactions` (CBACT04C-029).
    SystemTransaction(TransactionRecord),
}

/// Why the PARM is refused or the run abends. Every variant carries account IDs and keys at
/// most, never a card number (Cardholder Data Redaction).
///
/// The four abend variants are the lookups of an account break and its disclosure lookup. When
/// the record that failed was the first of a new account, the previous account had already been
/// rewritten, and `rewritten` holds that update: the COBOL keeps it, as VSAM has no unit of work
/// to back out (CBACT04C-005, CBACT04C-038).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IntcalcError {
    /// The PARM has fewer than 10 bytes; it carries its length (CBACT04C-001).
    ParmDateTooShort {
        /// Bytes received.
        len: usize,
    },
    /// The PARM holds a control byte or a byte outside ASCII (CBACT04C-001).
    ParmDateNotPrintable,
    /// No account has the category balance's account ID (CBACT04C-005).
    AccountNotFound {
        /// `FD-ACCT-ID`.
        acct_id: PicU<11>,
        /// The previous account's update, applied before the failed read.
        rewritten: Option<Box<AccountRecord>>,
    },
    /// No cross-reference carries the account ID (CBACT04C-007).
    XrefNotFound {
        /// `FD-XREF-ACCT-ID`.
        acct_id: PicU<11>,
        /// The previous account's update, applied before the failed read.
        rewritten: Option<Box<AccountRecord>>,
    },
    /// Two or more cross-references carry the account ID (CBACT04C-008); never one card chosen.
    XrefDuplicate {
        /// `FD-XREF-ACCT-ID`.
        acct_id: PicU<11>,
        /// The previous account's update, applied before the failed read.
        rewritten: Option<Box<AccountRecord>>,
    },
    /// Neither the account's group nor `DEFAULT` has a disclosure record for the category
    /// balance's type and category (CBACT04C-011).
    DefaultDisclosureMissing {
        /// The account of the category balance.
        acct_id: PicU<11>,
        /// `TRANCAT-TYPE-CD`.
        type_cd: PicX<2>,
        /// `TRANCAT-CD`.
        cat_cd: PicU<4>,
        /// The previous account's update, when this record began a new account.
        rewritten: Option<Box<AccountRecord>>,
    },
}

impl IntcalcError {
    /// The account update applied before the abend, which the job persists before it ends
    /// (CBACT04C-005, CBACT04C-038); `None` for a PARM error and for a failure inside an account
    /// already in hand.
    #[must_use]
    pub fn rewritten_before_abend(&self) -> Option<&AccountRecord> {
        match self {
            Self::ParmDateTooShort { .. } | Self::ParmDateNotPrintable => None,
            Self::AccountNotFound { rewritten, .. }
            | Self::XrefNotFound { rewritten, .. }
            | Self::XrefDuplicate { rewritten, .. }
            | Self::DefaultDisclosureMissing { rewritten, .. } => rewritten.as_deref(),
        }
    }

    /// The lines the COBOL displays for this failure, in order, ending with [`ABEND_LINE`]
    /// (CBACT04C-005, CBACT04C-007, CBACT04C-008, CBACT04C-011, CBACT04C-036, CBACT04C-038). A
    /// PARM error displays nothing: the job refuses the value before its first line
    /// (CBACT04C-001).
    #[must_use]
    pub fn sysout_lines(&self) -> Vec<String> {
        let not_found = |acct_id: PicU<11>, read_error: &str| {
            vec![
                format!("{ACCOUNT_NOT_FOUND_PREFIX}{}", digits(acct_id)),
                read_error.to_owned(),
                file_status_line(STATUS_NOT_FOUND),
                ABEND_LINE.to_owned(),
            ]
        };
        match self {
            Self::ParmDateTooShort { .. } | Self::ParmDateNotPrintable => Vec::new(),
            Self::AccountNotFound { acct_id, .. } => not_found(*acct_id, READ_ACCOUNT_ERROR),
            Self::XrefNotFound { acct_id, .. } => not_found(*acct_id, READ_XREF_ERROR),
            Self::XrefDuplicate { .. } => vec![
                READ_XREF_ERROR.to_owned(),
                file_status_line(STATUS_DUPLICATE_KEY),
                ABEND_LINE.to_owned(),
            ],
            Self::DefaultDisclosureMissing { .. } => vec![
                DISCLOSURE_MISSING_LINE.to_owned(),
                TRY_DEFAULT_LINE.to_owned(),
                READ_DEFAULT_DISCGRP_ERROR.to_owned(),
                file_status_line(STATUS_NOT_FOUND),
                ABEND_LINE.to_owned(),
            ],
        }
    }

    /// This error with `rewritten` set to the account update among `effects`, the effects of
    /// the record that failed.
    fn after(mut self, effects: Vec<IntcalcEffect>) -> Self {
        let update = effects.into_iter().find_map(|effect| match effect {
            IntcalcEffect::AccountUpdate(account) => Some(Box::new(account)),
            IntcalcEffect::SystemTransaction(_) => None,
        });
        match &mut self {
            Self::ParmDateTooShort { .. } | Self::ParmDateNotPrintable => {}
            Self::AccountNotFound { rewritten, .. }
            | Self::XrefNotFound { rewritten, .. }
            | Self::XrefDuplicate { rewritten, .. }
            | Self::DefaultDisclosureMissing { rewritten, .. } => *rewritten = update,
        }
        self
    }
}

impl fmt::Display for IntcalcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParmDateTooShort { len } => write!(
                f,
                "the PARM date has {len} bytes; it needs at least {PARM_DATE_LEN}"
            ),
            Self::ParmDateNotPrintable => {
                f.write_str("the PARM date holds a byte outside printable ASCII")
            }
            Self::AccountNotFound { acct_id, .. } => {
                write!(f, "account {} not found", digits(*acct_id))
            }
            Self::XrefNotFound { acct_id, .. } => {
                write!(
                    f,
                    "no card cross-reference for account {}",
                    digits(*acct_id)
                )
            }
            Self::XrefDuplicate { acct_id, .. } => write!(
                f,
                "more than one card cross-reference for account {}",
                digits(*acct_id)
            ),
            Self::DefaultDisclosureMissing {
                acct_id,
                type_cd,
                cat_cd,
                ..
            } => write!(
                f,
                "no DEFAULT disclosure group for type {} category {:04} of account {}",
                type_cd.as_bytes().escape_ascii(),
                cat_cd.get(),
                digits(*acct_id)
            ),
        }
    }
}

impl Error for IntcalcError {}

/// The 11 digits of an account ID, leading zeros kept, as `DISPLAY` shows a `PIC 9(11)` item.
fn digits(acct_id: PicU<11>) -> String {
    let mut text = Vec::with_capacity(11);
    acct_id.encode_into(&mut text);
    String::from_utf8_lossy(&text).into_owned()
}

/// The account in hand between breaks: `ACCOUNT-RECORD` and `CARD-XREF-RECORD` as read at its
/// break, and `WS-TOTAL-INT`.
struct CurrentAccount {
    /// `ACCOUNT-RECORD`, read by `1100-GET-ACCT-DATA` (CBACT04C-004).
    account: AccountRecord,
    /// `XREF-CARD-NUM`, read by `1110-GET-XREF-DATA` (CBACT04C-006).
    card_num: CardNumber,
    /// `WS-TOTAL-INT PIC S9(09)V99`, zero at the break (CBACT04C-014, ARITH-010).
    total: Money<9>,
}

/// One INTCALC run over the category balances in key order (CBACT04C-003).
///
/// It holds the job-supplied files as the program reads them by key, and the working storage
/// the loop carries from record to record. `WS-LAST-ACCT-NUM` (initially spaces) and
/// `WS-FIRST-TIME` (initially `Y`) are both the account in hand: none before the first break,
/// so the first record always breaks and rewrites nothing (CBACT04C-004).
pub struct Intcalc {
    /// `PARM-DATE`.
    parm: ParmDate,
    /// The account file by `ACCT-ID`; a rewritten account replaces its entry, as a later read of
    /// the same key would return the rewritten record.
    accounts: BTreeMap<PicU<11>, AccountRecord>,
    /// The cross-reference file's alternate index: every card of each `XREF-ACCT-ID`, in the
    /// order supplied (CBACT04C-006, CBACT04C-008).
    cards: BTreeMap<PicU<11>, Vec<CardNumber>>,
    /// The disclosure group file by `FD-DISCGRP-KEY`, holding `DIS-INT-RATE`.
    rates: BTreeMap<DisclosureKey, Rate<4>>,
    /// The account in hand, `None` before the first break.
    current: Option<CurrentAccount>,
    /// `WS-TRANID-SUFFIX PIC 9(06) VALUE 0`, counting SYSTRAN records over the run
    /// (CBACT04C-015).
    suffix: PicU<6>,
    /// Category balance records read (`WS-RECORD-COUNT`, CBACT04C-003).
    records_read: u64,
    /// SYSTRAN records built; unlike the suffix it never wraps.
    transactions_written: u64,
}

impl Intcalc {
    /// A run with PARM `parm` over the `accounts`, `xrefs` and `disclosure_groups` the job loads.
    ///
    /// Each table's key is unique; on a repeated key the first record supplied is kept, as a
    /// KSDS holds one record per key. Cross-references are grouped by `XREF-ACCT-ID` and every
    /// card of an account is kept, so an account with more than one card fails at its break
    /// instead of taking either (CBACT04C-008).
    #[must_use]
    pub fn new(
        parm: ParmDate,
        accounts: impl IntoIterator<Item = AccountRecord>,
        xrefs: impl IntoIterator<Item = CardXrefRecord>,
        disclosure_groups: impl IntoIterator<Item = DisclosureGroupRecord>,
    ) -> Self {
        let mut by_id = BTreeMap::new();
        for account in accounts {
            by_id.entry(account.id).or_insert(account);
        }
        let mut cards: BTreeMap<PicU<11>, Vec<CardNumber>> = BTreeMap::new();
        for xref in xrefs {
            cards.entry(xref.acct_id).or_default().push(xref.card_num);
        }
        let mut rates = BTreeMap::new();
        for group in disclosure_groups {
            let key = (group.acct_group_id, group.tran_type_cd, group.tran_cat_cd);
            rates.entry(key).or_insert(group.int_rate);
        }
        Self {
            parm,
            accounts: by_id,
            cards,
            rates,
            current: None,
            suffix: PicU::ZERO,
            records_read: 0,
            transactions_written: 0,
        }
    }

    /// One iteration of the main loop for the category balance `tcatbal`, the next record in
    /// key order (`CBACT04C.cbl:188-218`), returning its effects in program order.
    ///
    /// 1. Account break, when `tcatbal`'s account differs from the account in hand: the account
    ///    in hand, if any, is rewritten with its total ([`IntcalcEffect::AccountUpdate`],
    ///    CBACT04C-026); the total restarts at zero, and the new account and its single
    ///    cross-reference are read (CBACT04C-004 to CBACT04C-008).
    /// 2. The rate of the account's group, type and category, or of `DEFAULT` with the same type
    ///    and category when that record is missing (CBACT04C-009, CBACT04C-010; E07).
    /// 3. A zero rate computes nothing and writes nothing (CBACT04C-012, ARITH-015).
    /// 4. Otherwise [`monthly_interest`] is added to the total, the suffix advances and one
    ///    [`IntcalcEffect::SystemTransaction`] is built from one reading of `clock`
    ///    (CBACT04C-013 to CBACT04C-022); `1400-COMPUTE-FEES` is a stub and computes no fee
    ///    (CBACT04C-025, ARITH-016).
    ///
    /// # Errors
    ///
    /// The abends of the lookups: [`IntcalcError::AccountNotFound`],
    /// [`IntcalcError::XrefNotFound`], [`IntcalcError::XrefDuplicate`] and
    /// [`IntcalcError::DefaultDisclosureMissing`]. The run ends there; an error raised by the
    /// first record of a new account carries the previous account's update, which the COBOL
    /// applied before the failure ([`IntcalcError::rewritten_before_abend`]).
    pub fn process(
        &mut self,
        tcatbal: &TranCatBalanceRecord,
        clock: &dyn Clock,
    ) -> Result<Vec<IntcalcEffect>, IntcalcError> {
        self.records_read = self.records_read.saturating_add(1);
        let mut effects = Vec::new();
        let current = match self.current.take() {
            Some(current) if current.account.id == tcatbal.acct_id => current,
            previous => {
                if let Some(previous) = previous {
                    effects.push(IntcalcEffect::AccountUpdate(self.rewrite(previous)));
                }
                match self.read_account(tcatbal.acct_id) {
                    Ok(opened) => opened,
                    Err(error) => return Err(error.after(effects)),
                }
            }
        };
        let rate = match interest_rate(&self.rates, &current.account, tcatbal) {
            Ok(rate) => rate,
            Err(error) => return Err(error.after(effects)),
        };
        let current = self.current.insert(current);
        if !rate.is_zero() {
            let monthly = monthly_interest(tcatbal.bal, rate);
            current.total = (MoneyExpr::from(current.total) + monthly).store::<9>();
            self.suffix = next_suffix(self.suffix);
            effects.push(IntcalcEffect::SystemTransaction(systran_record(
                &self.parm,
                self.suffix,
                current.account.id,
                &current.card_num,
                monthly,
                clock,
            )));
            self.transactions_written = self.transactions_written.saturating_add(1);
        }
        Ok(effects)
    }

    /// The end of the category balance file (CBACT04C-027, CBACT04C-030).
    ///
    /// The loop's `ELSE PERFORM 1050-UPDATE-ACCOUNT` is never reached, so the account in hand,
    /// the last in key order, is not rewritten and its total is discarded (E08, ARITH-017). The
    /// summary returns no account update; it names that account and its total for the job log.
    #[must_use]
    pub fn finish(self) -> IntcalcSummary {
        IntcalcSummary {
            records_read: self.records_read,
            transactions_written: self.transactions_written,
            unapplied: self
                .current
                .map(|current| (current.account.id, current.total)),
        }
    }

    /// `1050-UPDATE-ACCOUNT` for the account leaving hand: the rewritten record, which also
    /// replaces the account file's entry (CBACT04C-026).
    fn rewrite(&mut self, previous: CurrentAccount) -> AccountRecord {
        let updated = update_account(previous.account, previous.total);
        self.accounts.insert(updated.id, updated.clone());
        updated
    }

    /// `1100-GET-ACCT-DATA` and `1110-GET-XREF-DATA` at a break: the account and the card of its
    /// only cross-reference, with the total at zero (CBACT04C-004 to CBACT04C-008).
    fn read_account(&self, acct_id: PicU<11>) -> Result<CurrentAccount, IntcalcError> {
        let account =
            self.accounts
                .get(&acct_id)
                .cloned()
                .ok_or(IntcalcError::AccountNotFound {
                    acct_id,
                    rewritten: None,
                })?;
        let card_num = match self.cards.get(&acct_id).map(Vec::as_slice) {
            Some([card]) => card.clone(),
            None | Some([]) => {
                return Err(IntcalcError::XrefNotFound {
                    acct_id,
                    rewritten: None,
                });
            }
            Some(_) => {
                return Err(IntcalcError::XrefDuplicate {
                    acct_id,
                    rewritten: None,
                });
            }
        };
        Ok(CurrentAccount {
            account,
            card_num,
            total: Money::ZERO,
        })
    }
}

/// `1200-GET-INTEREST-RATE` and `1200-A-GET-DEFAULT-INT-RATE`: the `DIS-INT-RATE` keyed by the
/// account's `ACCT-GROUP-ID` (all ten bytes, a blank group included) and `tcatbal`'s type and
/// category, or, when no record has that key, the one keyed by [`DEFAULT_GROUP`] and the same
/// type and category (CBACT04C-009, CBACT04C-010; E07, FIXINT-902).
fn interest_rate(
    rates: &BTreeMap<DisclosureKey, Rate<4>>,
    account: &AccountRecord,
    tcatbal: &TranCatBalanceRecord,
) -> Result<Rate<4>, IntcalcError> {
    let own = (
        account.group_id.clone(),
        tcatbal.type_cd.clone(),
        tcatbal.cd,
    );
    if let Some(rate) = rates.get(&own) {
        return Ok(*rate);
    }
    let fallback = (DEFAULT_GROUP, tcatbal.type_cd.clone(), tcatbal.cd);
    rates
        .get(&fallback)
        .copied()
        .ok_or_else(|| IntcalcError::DefaultDisclosureMissing {
            acct_id: tcatbal.acct_id,
            type_cd: tcatbal.type_cd.clone(),
            cat_cd: tcatbal.cd,
            rewritten: None,
        })
}

/// The counts of a run that reached the end of the category balance file, and the interest it
/// computed but never applied (CBACT04C-027, CBACT04C-030).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntcalcSummary {
    /// Category balance records read.
    records_read: u64,
    /// SYSTRAN records built.
    transactions_written: u64,
    /// The last account in key order and its total, never rewritten (E08).
    unapplied: Option<(PicU<11>, Money<9>)>,
}

impl IntcalcSummary {
    /// Category balance records read, `WS-RECORD-COUNT` without its `PIC 9(09)` wrap
    /// (CBACT04C-003).
    #[must_use]
    pub const fn records_read(&self) -> u64 {
        self.records_read
    }

    /// SYSTRAN records built over the run (CBACT04C-024).
    #[must_use]
    pub const fn transactions_written(&self) -> u64 {
        self.transactions_written
    }

    /// The last account in key order, which the run never rewrites (E08, CBACT04C-027); `None`
    /// when no record was read.
    #[must_use]
    pub fn unapplied_last_account(&self) -> Option<PicU<11>> {
        self.unapplied.map(|(acct_id, _)| acct_id)
    }

    /// The interest total of that account, computed and discarded (E08, ARITH-017).
    #[must_use]
    pub fn unapplied_total(&self) -> Option<Money<9>> {
        self.unapplied.map(|(_, total)| total)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::fs;
    use std::path::Path;

    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::framing::decode_all;
    use copybook::{FixedRecord, PicDate, PicU, PicX, TsForm};

    use super::{
        ABEND_LINE, AccountRecord, CardXrefRecord, DEFAULT_GROUP, DEFAULT_PARM_DATE,
        DISCLOSURE_MISSING_LINE, DisclosureGroupRecord, FILE_STATUS_PREFIX, Intcalc, IntcalcEffect,
        IntcalcError, ParmDate, READ_ACCOUNT_ERROR, READ_DEFAULT_DISCGRP_ERROR, READ_XREF_ERROR,
        TRY_DEFAULT_LINE, TranCatBalanceRecord, TransactionRecord, file_status_line,
        interest_transaction_id, next_suffix, systran_record, update_account,
    };
    use crate::clock::FixedClock;
    use crate::money::{Decimal, Money, Rate};
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A blank `ACCT-GROUP-ID`, as every fixture account holds (E07).
    const BLANK_GROUP: &[u8; 10] = b"          ";

    /// The `FixedClock` instant of the interest scenarios of `fixtures.md`.
    fn interest_instant() -> Result<NaiveDateTime, Box<dyn StdError>> {
        NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_opt(0, 0, 0))
            .ok_or_else(|| "2022-07-18 00:00:00 is a valid instant".into())
    }

    fn money<const N: u32>(cents: i64) -> Result<Money<N>, Box<dyn StdError>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    fn account(
        id: u64,
        group: &[u8; 10],
        bal_cents: i64,
    ) -> Result<AccountRecord, Box<dyn StdError>> {
        Ok(AccountRecord {
            id: PicU::new(id)?,
            active_status: PicX::from_array(*b"Y"),
            curr_bal: money(bal_cents)?,
            credit_limit: money(500_000)?,
            cash_credit_limit: money(100_000)?,
            open_date: PicDate::BLANK,
            expiraion_date: PicDate::BLANK,
            reissue_date: PicDate::BLANK,
            curr_cyc_credit: money(12_345)?,
            curr_cyc_debit: money(-6_789)?,
            addr_zip: PicX::from_array(*b"A000000000"),
            group_id: PicX::from_array(*group),
            filler: PicX::spaces(),
        })
    }

    fn card(last: u8) -> CardNumber {
        let mut digits = *b"4000000000000000";
        if let Some(digit) = digits.last_mut() {
            *digit = last;
        }
        CardNumber::new(PicX::from_array(digits))
    }

    fn xref(acct_id: u64, card_num: CardNumber) -> Result<CardXrefRecord, Box<dyn StdError>> {
        Ok(CardXrefRecord {
            card_num,
            cust_id: PicU::new(acct_id)?,
            acct_id: PicU::new(acct_id)?,
            filler: PicX::spaces(),
        })
    }

    fn rate(
        group: &[u8; 10],
        type_cd: [u8; 2],
        cat_cd: u64,
        cents: i64,
    ) -> Result<DisclosureGroupRecord, Box<dyn StdError>> {
        Ok(DisclosureGroupRecord {
            acct_group_id: PicX::from_array(*group),
            tran_type_cd: PicX::from_array(type_cd),
            tran_cat_cd: PicU::new(cat_cd)?,
            int_rate: Rate::new(Decimal::new(cents, 2))?,
            filler: PicX::spaces(),
        })
    }

    fn tcatbal(
        acct_id: u64,
        type_cd: [u8; 2],
        cat_cd: u64,
        cents: i64,
    ) -> Result<TranCatBalanceRecord, Box<dyn StdError>> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(acct_id)?,
            type_cd: PicX::from_array(type_cd),
            cd: PicU::new(cat_cd)?,
            bal: money(cents)?,
            filler: PicX::spaces(),
        })
    }

    /// The account updates and the SYSTRAN records among `effects`, each in order.
    fn split(effects: Vec<IntcalcEffect>) -> (Vec<AccountRecord>, Vec<TransactionRecord>) {
        let mut updates = Vec::new();
        let mut records = Vec::new();
        for effect in effects {
            match effect {
                IntcalcEffect::AccountUpdate(account) => updates.push(account),
                IntcalcEffect::SystemTransaction(record) => records.push(record),
            }
        }
        (updates, records)
    }

    // audit: CBACT04C-001
    #[test]
    fn parm_date_keeps_the_first_ten_of_ten_or_more_printable_bytes() -> TestResult {
        let cases: [(&[u8], &[u8; 10]); 4] = [
            (b"2022071800", b"2022071800"),
            (b"ABCDEFGHIJ", b"ABCDEFGHIJ"),
            (b"2022071800 AND MORE", b"2022071800"),
            (b" ~~~~~~~~~", b" ~~~~~~~~~"),
        ];
        for (input, kept) in cases {
            assert_eq!(ParmDate::parse(input)?.as_bytes(), kept);
        }
        assert_eq!(
            ParmDate::parse(DEFAULT_PARM_DATE.as_bytes())?,
            ParmDate::default()
        );
        assert_eq!(ParmDate::DEFAULT.as_bytes(), b"2022071800");
        Ok(())
    }

    // audit: CBACT04C-001
    #[test]
    fn parm_date_refuses_short_or_unprintable_values() {
        for len in 0..10 {
            let input = vec![b'2'; len];
            let refused = ParmDate::parse(&input);
            assert_eq!(refused, Err(IntcalcError::ParmDateTooShort { len }));
            assert!(
                refused.is_err_and(|error| error.rewritten_before_abend().is_none()
                    && error.sysout_lines().is_empty()
                    && error.to_string().contains(&format!("{len} bytes")))
            );
        }
        let unprintable: [&[u8]; 5] = [
            b"2022071800\t",
            b"20220718\x7f0",
            b"2022071800\x00",
            "2022071800é".as_bytes(),
            b"\n2022071800",
        ];
        for input in unprintable {
            let refused = ParmDate::parse(input);
            assert_eq!(refused, Err(IntcalcError::ParmDateNotPrintable));
            assert!(refused.is_err_and(|error| error.sysout_lines().is_empty()
                && error.rewritten_before_abend().is_none()
                && error.to_string().contains("printable ASCII")));
        }
    }

    // audit: CBACT04C-015
    // audit: FIXINT-901
    #[test]
    fn transaction_id_is_the_parm_then_the_suffix_in_six_digits() -> TestResult {
        let cases: [(&[u8], u64, &[u8; 16]); 3] = [
            (b"2022071800", 1, b"2022071800000001"),
            (b"2022071800", 999_999, b"2022071800999999"),
            (b"ABCDEFGHIJ", 42, b"ABCDEFGHIJ000042"),
        ];
        for (parm, suffix, id) in cases {
            let parm = ParmDate::parse(parm)?;
            assert_eq!(
                interest_transaction_id(&parm, PicU::new(suffix)?).as_bytes(),
                id
            );
        }
        Ok(())
    }

    // audit: CBACT04C-016
    #[test]
    fn the_suffix_wraps_from_999999_to_zero() -> TestResult {
        let cases: [(u64, u64); 3] = [(0, 1), (999_998, 999_999), (999_999, 0)];
        for (from, to) in cases {
            assert_eq!(next_suffix(PicU::new(from)?), PicU::new(to)?);
        }
        Ok(())
    }

    // audit: CBACT04C-026
    // audit: ARITH-013
    // audit: ARITH-014
    #[test]
    fn update_account_adds_the_total_and_zeroes_both_cycle_fields() -> TestResult {
        let cases: [(i64, i64, i64); 4] = [
            (84_300, 1_543, 85_843),
            (84_300, 0, 84_300),
            (999_999_999_999, 1, 0),
            (-62_156, 429, -61_727),
        ];
        for (bal, total, expected) in cases {
            let before = account(7, BLANK_GROUP, bal)?;
            let after = update_account(before.clone(), money(total)?);
            assert_eq!(after.curr_bal, money::<10>(expected)?);
            assert_eq!(after.curr_cyc_credit, Money::<10>::ZERO);
            assert_eq!(after.curr_cyc_debit, Money::<10>::ZERO);
            let restored = AccountRecord {
                curr_bal: before.curr_bal,
                curr_cyc_credit: before.curr_cyc_credit,
                curr_cyc_debit: before.curr_cyc_debit,
                ..after
            };
            assert_eq!(restored, before);
        }
        Ok(())
    }

    // audit: CBACT04C-036
    #[test]
    fn a_numeric_status_line_shows_two_zeros_and_the_status() {
        let cases: [([u8; 2], &str); 3] = [
            (*b"23", "FILE STATUS IS: NNNN0023"),
            (*b"02", "FILE STATUS IS: NNNN0002"),
            (*b"35", "FILE STATUS IS: NNNN0035"),
        ];
        for (status, line) in cases {
            assert_eq!(file_status_line(status), line);
        }
    }

    // HALTED(H4): the characters after the prefix carry a runtime status the replacement does
    // not reproduce; only the run and the prefix are checked, so this test carries no tag.
    #[test]
    fn other_statuses_keep_the_status_line_prefix() {
        for status in [*b"9A", *b"3x", [b'9', 0x05]] {
            let line = file_status_line(status);
            assert!(line.starts_with(FILE_STATUS_PREFIX));
        }
    }

    // audit: CBACT04C-017
    // audit: CBACT04C-018
    // audit: CBACT04C-020
    // audit: CBACT04C-021
    // audit: CBACT04C-022
    #[test]
    fn the_systran_record_carries_the_audited_fields() -> TestResult {
        let clock = FixedClock::new(interest_instant()?);
        let card_num = card(b'7');
        let record = systran_record(
            &ParmDate::DEFAULT,
            PicU::new(1)?,
            PicU::new(1)?,
            &card_num,
            money(1_543)?,
            &clock,
        );
        let bytes = record.encode();
        assert_eq!(bytes.len(), 350);
        let stamp: &[u8] = b"2022-07-18-00.00.00.000000";
        let fields: [(usize, usize, &[u8]); 9] = [
            (0, 16, b"2022071800000001"),
            (16, 18, b"01"),
            (18, 22, b"0005"),
            (22, 32, b"System    "),
            (32, 56, b"Int. for a/c 00000000001"),
            (132, 143, b"0000000154C"),
            (143, 152, b"000000000"),
            (278, 304, stamp),
            (304, 330, stamp),
        ];
        for (start, end, expected) in fields {
            assert_eq!(bytes.get(start..end), Some(expected), "offset {start}");
        }
        let merchant = bytes.get(152..262).ok_or("merchant fields")?;
        assert!(merchant.iter().all(|&byte| byte == b' '));
        assert!(bytes.get(262..278) == Some(card_num.expose().as_bytes().as_slice()));
        assert!(record.card_num == card_num);
        assert_eq!(record.amt, money::<9>(1_543)?);
        assert_eq!(record.orig_ts, record.proc_ts);
        assert_eq!(
            record.orig_ts.value().map(|stamp| stamp.form()),
            Some(TsForm::Db2)
        );
        Ok(())
    }

    // audit: CBACT04C-022
    #[test]
    fn the_timestamp_keeps_hundredths_of_one_clock_reading() -> TestResult {
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_micro_opt(12, 34, 56, 789_123))
            .ok_or("a valid instant")?;
        let record = systran_record(
            &ParmDate::DEFAULT,
            PicU::new(2)?,
            PicU::new(3)?,
            &card(b'1'),
            Money::ZERO,
            &FixedClock::new(at),
        );
        let bytes = record.encode();
        let expected: &[u8] = b"2022-07-18-12.34.56.780000";
        assert_eq!(bytes.get(278..304), Some(expected));
        assert_eq!(bytes.get(304..330), Some(expected));
        Ok(())
    }

    fn fixture<R: FixedRecord>(file: &str) -> Result<Vec<R>, Box<dyn StdError>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../app/data/ASCII")
            .join(file);
        let records = decode_all::<R>(&fs::read(path)?)?;
        Ok(records.into_iter().map(|framed| framed.record).collect())
    }

    // audit: CBACT04C-004
    // audit: CBACT04C-010
    // audit: CBACT04C-027
    // audit: FIXINT-901
    // audit: FIXINT-902
    // audit: FIXINT-903
    #[test]
    fn the_raw_fixture_run_rewrites_every_account_but_the_last() -> TestResult {
        let accounts: Vec<AccountRecord> = fixture("acctdata.txt")?;
        let xrefs: Vec<CardXrefRecord> = fixture("cardxref.txt")?;
        let balances: Vec<TranCatBalanceRecord> = fixture("tcatbal.txt")?;
        let groups: Vec<DisclosureGroupRecord> = fixture("discgrp.txt")?;
        assert!(accounts.iter().all(|account| account.group_id.is_spaces()));
        let clock = FixedClock::new(interest_instant()?);
        let mut run = Intcalc::new(ParmDate::DEFAULT, accounts.clone(), xrefs.clone(), groups);
        for (n, balance) in (1_u64..).zip(&balances) {
            let (updates, records) = split(run.process(balance, &clock)?);
            let previous = accounts
                .iter()
                .find(|account| account.id.get() + 1 == n)
                .map(|account| update_account(account.clone(), Money::ZERO));
            assert_eq!(updates.first(), previous.as_ref(), "FIXINT-903, record {n}");
            assert_eq!(updates.len(), usize::from(n > 1));
            let [record] = records.as_slice() else {
                return Err(format!("record {n} writes one SYSTRAN record").into());
            };
            assert_eq!(
                record.id,
                interest_transaction_id(&ParmDate::DEFAULT, PicU::new(n)?)
            );
            assert!(record.amt.is_zero(), "ARITH-009, record {n}");
            let card_num = xrefs
                .iter()
                .find(|xref| xref.acct_id == balance.acct_id)
                .map(|xref| &xref.card_num);
            assert!(card_num == Some(&record.card_num), "FIXINT-901, record {n}");
            let desc = format!("Int. for a/c {n:011}");
            assert!(record.desc.as_bytes().starts_with(desc.as_bytes()));
        }
        let summary = run.finish();
        assert_eq!(summary.records_read(), 50);
        assert_eq!(summary.transactions_written(), 50);
        assert_eq!(summary.unapplied_last_account(), Some(PicU::new(50)?));
        assert_eq!(summary.unapplied_total(), Some(Money::ZERO));
        Ok(())
    }

    // audit: CBACT04C-009
    // audit: CBACT04C-010
    // audit: CBACT04C-012
    // audit: CBACT04C-013
    // audit: CBACT04C-014
    // audit: CBACT04C-015
    // audit: CBACT04C-026
    // audit: ARITH-002
    // audit: ARITH-003
    // audit: ARITH-005
    // audit: ARITH-006
    // audit: ARITH-010
    // audit: ARITH-015
    #[test]
    fn own_group_rates_default_fallback_and_zero_rates_drive_the_totals() -> TestResult {
        let group = b"GRP1      ";
        let first = account(1, group, 100_000)?;
        let mut run = Intcalc::new(
            ParmDate::DEFAULT,
            [first.clone(), account(2, BLANK_GROUP, 0)?],
            [xref(1, card(b'1'))?, xref(2, card(b'2'))?],
            [
                rate(group, *b"01", 1, 1_500)?,
                rate(group, *b"01", 2, 1_000)?,
                rate(DEFAULT_GROUP.as_bytes(), *b"02", 1, 0)?,
                rate(DEFAULT_GROUP.as_bytes(), *b"03", 1, 1_500)?,
                rate(DEFAULT_GROUP.as_bytes(), *b"01", 1, 1_000)?,
            ],
        );
        let clock = FixedClock::new(interest_instant()?);
        // (balance, expected monthly interest in cents, or None for a zero rate)
        let cases: [(TranCatBalanceRecord, Option<i64>); 5] = [
            (tcatbal(1, *b"01", 1, 123_456)?, Some(1_543)),
            (tcatbal(1, *b"01", 2, 10_000)?, Some(83)),
            (tcatbal(1, *b"02", 1, 50_000)?, None),
            (tcatbal(1, *b"03", 1, -123_456)?, Some(-1_543)),
            (tcatbal(2, *b"01", 1, -20_000)?, Some(-166)),
        ];
        let mut suffix = 0;
        let mut updates = Vec::new();
        for (balance, monthly) in &cases {
            let (rewritten, records) = split(run.process(balance, &clock)?);
            updates.extend(rewritten);
            match (monthly, records.as_slice()) {
                (None, []) => {}
                (Some(cents), [record]) => {
                    suffix += 1;
                    assert_eq!(record.amt, money::<9>(*cents)?);
                    assert_eq!(
                        record.id,
                        interest_transaction_id(&ParmDate::DEFAULT, PicU::new(suffix)?)
                    );
                }
                _ => return Err("one SYSTRAN record per non-zero rate, none otherwise".into()),
            }
        }
        assert_eq!(updates, [update_account(first, money(83)?)]);
        let summary = run.finish();
        assert_eq!(summary.records_read(), 5);
        assert_eq!(summary.transactions_written(), 4);
        assert_eq!(summary.unapplied_last_account(), Some(PicU::new(2)?));
        assert_eq!(summary.unapplied_total(), Some(money(-166)?));
        Ok(())
    }

    // audit: CBACT04C-027
    #[test]
    fn a_run_with_no_records_leaves_nothing_unapplied() {
        let summary = Intcalc::new(ParmDate::DEFAULT, [], [], []).finish();
        assert_eq!(summary.records_read(), 0);
        assert_eq!(summary.transactions_written(), 0);
        assert_eq!(summary.unapplied_last_account(), None);
        assert_eq!(summary.unapplied_total(), None);
    }

    /// A run over accounts 1 to 6 with one card each, except account 3, which has none, and
    /// account 4, which has two; only `DEFAULT` `01`/`0001` has a rate.
    fn failing_run() -> Result<Intcalc, Box<dyn StdError>> {
        let accounts = (1..=6)
            .map(|id| account(id, BLANK_GROUP, 1_000))
            .collect::<Result<Vec<_>, _>>()?;
        let xrefs = [
            xref(1, card(b'1'))?,
            xref(2, card(b'2'))?,
            xref(4, card(b'4'))?,
            xref(4, card(b'5'))?,
            xref(5, card(b'6'))?,
            xref(6, card(b'7'))?,
        ];
        let groups = [rate(DEFAULT_GROUP.as_bytes(), *b"01", 1, 1_200)?];
        Ok(Intcalc::new(ParmDate::DEFAULT, accounts, xrefs, groups))
    }

    /// The error the second of `balances` raises after the first succeeds.
    fn second_fails(
        balances: [TranCatBalanceRecord; 2],
    ) -> Result<IntcalcError, Box<dyn StdError>> {
        let clock = FixedClock::new(interest_instant()?);
        let mut run = failing_run()?;
        let [first, second] = balances;
        run.process(&first, &clock)?;
        match run.process(&second, &clock) {
            Ok(_) => Err("the second record abends".into()),
            Err(error) => Ok(error),
        }
    }

    // audit: CBACT04C-005
    // audit: CBACT04C-038
    #[test]
    fn a_missing_account_abends_after_rewriting_the_previous_one() -> TestResult {
        let error = second_fails([tcatbal(1, *b"01", 1, 10_000)?, tcatbal(9, *b"01", 1, 0)?])?;
        // 100.00 at the DEFAULT rate 12.00 is 1.00 of monthly interest.
        let rewritten = update_account(account(1, BLANK_GROUP, 1_000)?, money(100)?);
        assert_eq!(
            error,
            IntcalcError::AccountNotFound {
                acct_id: PicU::new(9)?,
                rewritten: Some(Box::new(rewritten.clone())),
            }
        );
        assert_eq!(error.rewritten_before_abend(), Some(&rewritten));
        assert_eq!(error.to_string(), "account 00000000009 not found");
        assert_eq!(
            error.sysout_lines(),
            [
                "ACCOUNT NOT FOUND: 00000000009",
                READ_ACCOUNT_ERROR,
                "FILE STATUS IS: NNNN0023",
                ABEND_LINE,
            ]
        );
        Ok(())
    }

    // audit: CBACT04C-007
    // audit: CBACT04C-008
    // audit: CBACT04C-036
    #[test]
    fn a_missing_or_repeated_cross_reference_abends() -> TestResult {
        let rewritten = Some(Box::new(update_account(
            account(2, BLANK_GROUP, 1_000)?,
            Money::ZERO,
        )));
        let cases: [(u64, IntcalcError, Vec<String>); 2] = [
            (
                3,
                IntcalcError::XrefNotFound {
                    acct_id: PicU::new(3)?,
                    rewritten: rewritten.clone(),
                },
                vec![
                    "ACCOUNT NOT FOUND: 00000000003".to_owned(),
                    READ_XREF_ERROR.to_owned(),
                    "FILE STATUS IS: NNNN0023".to_owned(),
                    ABEND_LINE.to_owned(),
                ],
            ),
            (
                4,
                IntcalcError::XrefDuplicate {
                    acct_id: PicU::new(4)?,
                    rewritten,
                },
                vec![
                    READ_XREF_ERROR.to_owned(),
                    "FILE STATUS IS: NNNN0002".to_owned(),
                    ABEND_LINE.to_owned(),
                ],
            ),
        ];
        for (acct_id, expected, lines) in cases {
            let error = second_fails([tcatbal(2, *b"01", 1, 0)?, tcatbal(acct_id, *b"01", 1, 0)?])?;
            assert_eq!(error.sysout_lines(), lines);
            assert_eq!(error, expected);
            assert!(!error.to_string().contains("400000000000000"));
        }
        Ok(())
    }

    // audit: CBACT04C-011
    #[test]
    fn a_missing_default_disclosure_record_abends() -> TestResult {
        let rewritten = update_account(account(5, BLANK_GROUP, 1_000)?, Money::ZERO);
        let cases: [(u64, Option<Box<AccountRecord>>); 2] =
            [(5, None), (6, Some(Box::new(rewritten)))];
        for (acct_id, rewritten) in cases {
            let error = second_fails([tcatbal(5, *b"01", 1, 0)?, tcatbal(acct_id, *b"02", 1, 0)?])?;
            assert_eq!(
                error.sysout_lines(),
                [
                    DISCLOSURE_MISSING_LINE,
                    TRY_DEFAULT_LINE,
                    READ_DEFAULT_DISCGRP_ERROR,
                    "FILE STATUS IS: NNNN0023",
                    ABEND_LINE,
                ]
            );
            assert_eq!(
                error,
                IntcalcError::DefaultDisclosureMissing {
                    acct_id: PicU::new(acct_id)?,
                    type_cd: PicX::from_array(*b"02"),
                    cat_cd: PicU::new(1)?,
                    rewritten,
                }
            );
            assert!(error.to_string().contains("type 02 category 0001"));
        }
        Ok(())
    }
}
