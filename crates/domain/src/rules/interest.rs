//! `CBACT04C` (job `INTCALC`): the monthly interest on the transaction category
//! balances, one interest transaction per rated balance and one account update
//! per account.
//!
//! `batch intcalc` reads `TCATBALF` in key order (account, type code, category
//! code; `1000-TCATBALF-GET-NEXT`, P-CBACT04C-182) and hands each balance to
//! [`InterestRun::process_record`], which ports the body of the main loop
//! [app/cbl/CBACT04C.cbl:L188-218]:
//!
//! - on an account break it rewrites the previous account with its interest
//!   total (`1050-UPDATE-ACCOUNT`) and reads the new account and its card
//!   cross-reference (`1100-GET-ACCT-DATA`, `1110-GET-XREF-DATA`). The
//!   rewrite re-reads the previous account with a row lock and applies only
//!   the interest and the cycle resets to that record, so a bill payment,
//!   account update or posting committed since the account's first balance
//!   record keeps its changes (P-CBACT04C-010, P-CBACT04C-191,
//!   P-CBACT04C-192);
//! - it reads the rate of the account's disclosure group for the balance's type
//!   and category, falling back to the `DEFAULT` group on status 23
//!   (`1200-GET-INTEREST-RATE`, `1200-A-GET-DEFAULT-INT-RATE`). Every fixture
//!   `ACCT-GROUP-ID` is blank, so every fixture rate comes from `DEFAULT` (E-06);
//! - for a non-zero rate it computes the monthly interest, adds it to the
//!   account's total and returns the interest transaction
//!   (`1300-COMPUTE-INTEREST`, `1300-B-WRITE-TX`). `1400-COMPUTE-FEES` is an
//!   empty paragraph and has no counterpart.
//!
//! The interest transactions go to the new sequential generation `SYSTRAN(+1)`
//! (`TRANSACT DD DSN=AWS.M2.CARDDEMO.SYSTRAN(+1)` [app/jcl/INTCALC.jcl:L37-41]),
//! never to the live transaction master: the job writes each returned record
//! through the `CVTRA05Y` codec layout. The account rewrites go through
//! [`AccountWriter`]; the rule commits nothing. A rewrite done before a later
//! abend stays written in the COBOL, whose files are `RECOVERY(NONE)`
//! (P-CBACT04C-010).
//!
//! At end of file the COBOL loop ends before the `ELSE` that would update the
//! last account, so the last account never receives its interest (E-14,
//! P-CBACT04C-003, P-CBACT04C-174). [`InterestRun::finish`] reproduces that: it
//! reports the unposted total and writes nothing.
//!
//! Every open, read, write or close that ends with a status other than 00
//! abends with `CEE3ABD` code 999 after the paragraph's displays;
//! [`InterestAbend`] holds those displays for the job's stderr (exit 16,
//! P-CBACT04C-188). The job performs the opens and closes of the files
//! ([`InterestFile`]) and the `SYSTRAN(+1)` I/O around the rule.

use std::error::Error;
use std::fmt;

use crate::clock::{Clock, db2_timestamp};
use crate::entities::{CardXref, DisclosureGroupKey, NewTransaction, TranCatBalance};
use crate::messages::{
    ABENDING_PROGRAM, ACCOUNT_NOT_FOUND_CBACT04C, ERROR_CLOSING_ACCOUNT_FILE,
    ERROR_CLOSING_CROSS_REF_FILE, ERROR_CLOSING_DISCLOSURE_GROUP_FILE,
    ERROR_CLOSING_TRANSACTION_BALANCE_FILE, ERROR_OPENING_ACCOUNT_MASTER_FILE,
    ERROR_OPENING_CROSS_REF_FILE, ERROR_OPENING_DALY_REJECTS_FILE,
    ERROR_OPENING_TRANSACTION_CATEGORY_BALANCE, ERROR_RE_WRITING_ACCOUNT_FILE,
    ERROR_READING_ACCOUNT_FILE, ERROR_READING_DEFAULT_DISCLOSURE_GROUP,
    ERROR_READING_DISCLOSURE_GROUP_FILE, ERROR_READING_TRANSACTION_CATEGORY_FILE,
    ERROR_READING_XREF_FILE, ERROR_WRITING_TRANSACTION_RECORD, file_status_message,
};
use crate::numeric::{
    AccountGroupId, AccountId, Alnum, InterestRate, MerchantId, Money, TranCategoryCode, TranId,
    TranTypeCode,
};
use crate::ports::{AccountReader, AccountWriter, DisclosureGroupReader, PortError, XrefReader};

// The literals 1300-B-WRITE-TX moves into the interest record
// [app/cbl/CBACT04C.cbl:L482-494] (P-CBACT04C-008, P-CBACT04C-173). They are
// record field values, not messages, so they live here and not in
// `crate::messages`, whose catalog excludes record field values.

/// `MOVE '01' TO TRAN-TYPE-CD` [app/cbl/CBACT04C.cbl:L482].
const INTEREST_TRAN_TYPE_CD: TranTypeCode = TranTypeCode::from_alnum(Alnum::from_array(*b"01"));

/// `MOVE '05' TO TRAN-CAT-CD`, a `PIC 9(04)` receiver, so 0005
/// [app/cbl/CBACT04C.cbl:L483].
const INTEREST_TRAN_CAT_CD: TranCategoryCode = match TranCategoryCode::new(5) {
    Ok(code) => code,
    Err(_) => panic!("5 fits the four digits of TRAN-CAT-CD"),
};

/// `MOVE 'System' TO TRAN-SOURCE` [app/cbl/CBACT04C.cbl:L484].
const INTEREST_TRAN_SOURCE: &str = "System";

/// The first operand of `STRING 'Int. for a/c ', ACCT-ID DELIMITED BY SIZE
/// INTO TRAN-DESC` [app/cbl/CBACT04C.cbl:L485-489].
const INTEREST_DESC_PREFIX: &str = "Int. for a/c ";

/// `MOVE 0 TO TRAN-MERCHANT-ID` [app/cbl/CBACT04C.cbl:L491].
const NO_MERCHANT: MerchantId = match MerchantId::new(0) {
    Ok(id) => id,
    Err(_) => panic!("0 fits the nine digits of TRAN-MERCHANT-ID"),
};

/// `10^6`: `WS-TRANID-SUFFIX PIC 9(06)` [app/cbl/CBACT04C.cbl:L173] holds the
/// suffix modulo this value.
const TRANID_SUFFIX_MODULUS: u32 = 1_000_000;

/// The VSAM file status the COBOL tests after a port call that failed with
/// `cause`, as 9910-DISPLAY-IO-STATUS shows it (P-CBACT04C-189).
///
/// A missing key is status 23 (`INVALID KEY`) and a duplicate key 22. Every
/// other failure is a database outcome with no VSAM status of its own; it is
/// shown as 30, the permanent I/O error without further information, so the
/// abend path and its displays stay those of the COBOL.
fn vsam_status(cause: &PortError) -> [u8; 2] {
    match cause {
        PortError::NotFound => *b"23",
        PortError::Duplicate => *b"22",
        PortError::Restricted | PortError::Contention | PortError::Unavailable(_) => *b"30",
    }
}

/// CBACT04C 0000-TCATBALF-OPEN to 0300-ACCTFILE-OPEN and 9000-TCATBALF-CLOSE
/// to 9300-ACCTFILE-CLOSE — P-CBACT04C-177 to P-CBACT04C-180, P-CBACT04C-183
/// to P-CBACT04C-186: a KSDS file `CBACT04C` opens before its main loop and
/// closes after it, in the order of [`InterestFile::ALL`]
/// [app/cbl/CBACT04C.cbl:L182-185] [app/cbl/CBACT04C.cbl:L224-227].
/// `batch intcalc` performs each open and
/// close and reports a failed one with [`InterestAbend::FileOpen`] or
/// [`InterestAbend::FileClose`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterestFile {
    /// `TCATBAL-FILE`, DD `TCATBALF`: the category balances.
    Tcatbal,
    /// `XREF-FILE`, DD `XREFFILE`: the card cross-reference.
    Xref,
    /// `DISCGRP-FILE`, DD `DISCGRP`: the disclosure groups.
    DisclosureGroup,
    /// `ACCOUNT-FILE`, DD `ACCTFILE`: the accounts.
    Account,
}

impl InterestFile {
    /// The four files in the order `CBACT04C` opens them (`0000-TCATBALF-OPEN`
    /// to `0300-ACCTFILE-OPEN`) and closes them (`9000-TCATBALF-CLOSE` to
    /// `9300-ACCTFILE-CLOSE`); `TRANSACT-FILE`, opened and closed last, is the
    /// job's `SYSTRAN(+1)`.
    pub const ALL: [Self; 4] = [
        Self::Tcatbal,
        Self::Xref,
        Self::DisclosureGroup,
        Self::Account,
    ];

    /// CBACT04C 0000-TCATBALF-OPEN to 0300-ACCTFILE-OPEN — P-CBACT04C-177 to
    /// P-CBACT04C-180: the text the failed open displays. The disclosure group
    /// open displays the daily rejects text, as written (E-72,
    /// P-CBACT04C-194) [app/cbl/CBACT04C.cbl:L281].
    const fn open_text(self) -> &'static str {
        match self {
            Self::Tcatbal => ERROR_OPENING_TRANSACTION_CATEGORY_BALANCE,
            Self::Xref => ERROR_OPENING_CROSS_REF_FILE,
            Self::DisclosureGroup => ERROR_OPENING_DALY_REJECTS_FILE,
            Self::Account => ERROR_OPENING_ACCOUNT_MASTER_FILE,
        }
    }

    /// CBACT04C 9000-TCATBALF-CLOSE to 9300-ACCTFILE-CLOSE — P-CBACT04C-183 to
    /// P-CBACT04C-186: the text the failed close displays.
    const fn close_text(self) -> &'static str {
        match self {
            Self::Tcatbal => ERROR_CLOSING_TRANSACTION_BALANCE_FILE,
            Self::Xref => ERROR_CLOSING_CROSS_REF_FILE,
            Self::DisclosureGroup => ERROR_CLOSING_DISCLOSURE_GROUP_FILE,
            Self::Account => ERROR_CLOSING_ACCOUNT_FILE,
        }
    }
}

/// A `CBACT04C` abend: an I/O statement ended with a status other than 00, and
/// the program displays its error text, `FILE STATUS IS: NNNN` with the status
/// and `ABENDING PROGRAM`, then calls `CEE3ABD` with code [`Self::ABCODE`]
/// (9999-ABEND-PROGRAM, P-CBACT04C-188). `batch intcalc` reports it with exit
/// code 16.
///
/// [`fmt::Display`] renders [`InterestAbend::lines`] one per line, the
/// `DISPLAY` lines of the failing paragraph from the failing statement to the
/// abend. [`Error::source`] gives the port failure, whose adapter detail
/// belongs in logs only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterestAbend {
    /// CBACT04C 0000-TCATBALF-OPEN, 0100-XREFFILE-OPEN, 0200-DISCGRP-OPEN and
    /// 0300-ACCTFILE-OPEN — P-CBACT04C-177 to P-CBACT04C-180, P-CBACT04C-194:
    /// the `OPEN` of `file` ended with a status other than 00, before any
    /// record is read. The job performs the open and reports it with this
    /// variant.
    FileOpen {
        /// The file whose open failed.
        file: InterestFile,
        /// Why the open failed.
        cause: PortError,
    },
    /// CBACT04C 9000-TCATBALF-CLOSE, 9100-XREFFILE-CLOSE, 9200-DISCGRP-CLOSE
    /// and 9300-ACCTFILE-CLOSE — P-CBACT04C-183 to P-CBACT04C-186: the `CLOSE`
    /// of `file` after the main loop ended with a status other than 00. The
    /// job performs the close and reports it with this variant.
    FileClose {
        /// The file whose close failed.
        file: InterestFile,
        /// Why the close failed.
        cause: PortError,
    },
    /// CBACT04C 1000-TCATBALF-GET-NEXT — P-CBACT04C-182: the sequential read
    /// of `TCATBALF` failed. The job performs this read and reports it with
    /// this variant.
    TcatbalRead {
        /// Why the read failed.
        cause: PortError,
    },
    /// CBACT04C 1100-GET-ACCT-DATA — P-CBACT04C-002: the account read failed.
    AccountRead {
        /// `FD-ACCT-ID`, the account the read used.
        acct_id: AccountId,
        /// Why the read failed; [`PortError::NotFound`] is the `INVALID KEY`
        /// that also displays `ACCOUNT NOT FOUND: ` and the account.
        cause: PortError,
    },
    /// CBACT04C 1110-GET-XREF-DATA — P-CBACT04C-002: the cross-reference read
    /// by account on the alternate index failed.
    XrefRead {
        /// `FD-XREF-ACCT-ID`, the account the read used.
        acct_id: AccountId,
        /// Why the read failed; [`PortError::NotFound`] is the `INVALID KEY`
        /// that also displays `ACCOUNT NOT FOUND: ` and the account.
        cause: PortError,
    },
    /// CBACT04C 1200-GET-INTEREST-RATE — P-CBACT04C-004: the disclosure group
    /// read ended with a status other than 00 and 23.
    DisclosureGroupRead {
        /// `FD-DISCGRP-KEY`: the account's group, the type and the category.
        key: DisclosureGroupKey,
        /// Why the read failed; never [`PortError::NotFound`], which reads the
        /// `DEFAULT` group instead.
        cause: PortError,
    },
    /// CBACT04C 1200-A-GET-DEFAULT-INT-RATE — P-CBACT04C-005: the read of the
    /// `DEFAULT` group ended with a status other than 00.
    DefaultDisclosureGroupRead {
        /// `FD-DISCGRP-KEY` with the group `DEFAULT`.
        key: DisclosureGroupKey,
        /// Why the read failed.
        cause: PortError,
    },
    /// CBACT04C 1050-UPDATE-ACCOUNT — P-CBACT04C-010: the account rewrite
    /// failed, or the locked re-read of the account that precedes it. The
    /// paragraph has no `INVALID KEY` clause, so a missing account shows only
    /// its status, 23.
    AccountRewrite {
        /// `ACCT-ID` of the account rewritten.
        acct_id: AccountId,
        /// Why the re-read or the rewrite failed.
        cause: PortError,
    },
    /// CBACT04C 1300-B-WRITE-TX — P-CBACT04C-009: the write of the interest
    /// record to `SYSTRAN(+1)` failed. The job performs this write and reports
    /// it with this variant.
    SystranWrite {
        /// The file status the job assigns to the failed write.
        status: [u8; 2],
    },
}

impl InterestAbend {
    /// `MOVE 999 TO ABCODE` [app/cbl/CBACT04C.cbl:L631]: the code passed to
    /// `CEE3ABD`, which the job reports as `ABEND 999`.
    pub const ABCODE: u32 = 999;

    /// The two status bytes 9910-DISPLAY-IO-STATUS displays for this abend.
    #[must_use]
    pub fn file_status(&self) -> [u8; 2] {
        match self {
            Self::FileOpen { cause, .. }
            | Self::FileClose { cause, .. }
            | Self::TcatbalRead { cause }
            | Self::AccountRead { cause, .. }
            | Self::XrefRead { cause, .. }
            | Self::DisclosureGroupRead { cause, .. }
            | Self::DefaultDisclosureGroupRead { cause, .. }
            | Self::AccountRewrite { cause, .. } => vsam_status(cause),
            Self::SystranWrite { status } => *status,
        }
    }

    /// The `DISPLAY` lines of the abend, in order: the `INVALID KEY` display
    /// of a missing account or cross-reference, the paragraph's error text,
    /// `FILE STATUS IS: NNNN` with the status, and `ABENDING PROGRAM`
    /// [app/cbl/CBACT04C.cbl:L372-413] [app/cbl/CBACT04C.cbl:L628-648].
    ///
    /// `0100-XREFFILE-OPEN` displays `XREFFILE-STATUS` right after its text,
    /// in the same `DISPLAY` (P-CBACT04C-178) [app/cbl/CBACT04C.cbl:L263], so
    /// that line ends with the two status bytes.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let invalid_key = match self {
            Self::AccountRead {
                acct_id,
                cause: PortError::NotFound,
            }
            | Self::XrefRead {
                acct_id,
                cause: PortError::NotFound,
            } => Some(format!("{ACCOUNT_NOT_FOUND_CBACT04C}{acct_id}")),
            _ => None,
        };
        let status = self.file_status();
        let mut error_line = self.error_text().to_owned();
        if let Self::FileOpen {
            file: InterestFile::Xref,
            ..
        } = self
        {
            error_line.extend(status.iter().copied().map(char::from));
        }
        invalid_key
            .into_iter()
            .chain([
                error_line,
                file_status_message(status),
                ABENDING_PROGRAM.to_owned(),
            ])
            .collect()
    }

    /// The error text the failing paragraph displays before its status.
    fn error_text(&self) -> &'static str {
        match self {
            Self::FileOpen { file, .. } => file.open_text(),
            Self::FileClose { file, .. } => file.close_text(),
            Self::TcatbalRead { .. } => ERROR_READING_TRANSACTION_CATEGORY_FILE,
            Self::AccountRead { .. } => ERROR_READING_ACCOUNT_FILE,
            Self::XrefRead { .. } => ERROR_READING_XREF_FILE,
            Self::DisclosureGroupRead { .. } => ERROR_READING_DISCLOSURE_GROUP_FILE,
            Self::DefaultDisclosureGroupRead { .. } => ERROR_READING_DEFAULT_DISCLOSURE_GROUP,
            Self::AccountRewrite { .. } => ERROR_RE_WRITING_ACCOUNT_FILE,
            Self::SystranWrite { .. } => ERROR_WRITING_TRANSACTION_RECORD,
        }
    }
}

/// The [`InterestAbend::lines`], one per line.
impl fmt::Display for InterestAbend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.lines().join("\n"))
    }
}

/// The source is the port failure; a failed `SYSTRAN` write has none.
impl Error for InterestAbend {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::FileOpen { cause, .. }
            | Self::FileClose { cause, .. }
            | Self::TcatbalRead { cause }
            | Self::AccountRead { cause, .. }
            | Self::XrefRead { cause, .. }
            | Self::DisclosureGroupRead { cause, .. }
            | Self::DefaultDisclosureGroupRead { cause, .. }
            | Self::AccountRewrite { cause, .. } => Some(cause),
            Self::SystranWrite { .. } => None,
        }
    }
}

/// The interest total `CBACT04C` computes for the last account of the run and
/// never adds to it (E-14, P-CBACT04C-174).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnpostedInterest {
    /// `ACCT-ID` of the last account, which keeps its balance and cycle
    /// amounts.
    pub acct_id: AccountId,
    /// Its `WS-TOTAL-INT`, whose interest records were still written.
    pub total_interest: Money<9>,
}

/// What an `INTCALC` run did, for the job's end-of-run telemetry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterestSummary {
    /// The balance records read: `WS-RECORD-COUNT` [app/cbl/CBACT04C.cbl:L192].
    pub records_read: u64,
    /// The interest records returned for `SYSTRAN(+1)`, one per balance with a
    /// non-zero rate.
    pub interest_records: u64,
    /// The accounts rewritten by `1050-UPDATE-ACCOUNT`.
    pub accounts_updated: u64,
    /// The rate reads that ended with status 23 and read the `DEFAULT` group,
    /// each of which the COBOL reports with `DISCLOSURE GROUP RECORD MISSING`
    /// [app/cbl/CBACT04C.cbl:L416-420].
    pub default_group_reads: u64,
    /// The last account and its unposted total, `None` when no record was
    /// read.
    pub unposted: Option<UnpostedInterest>,
}

/// The account whose balance records are being read: the working storage the
/// main loop keeps across records of one account (P-CBACT04C-002).
///
/// It keeps what the later records of the account use, never the whole
/// `ACCOUNT-RECORD`. Each record is committed on its own, so an online bill
/// payment, account update or posting can change the account before its
/// break; `1050-UPDATE-ACCOUNT` therefore re-reads the record rather than
/// rewriting a copy taken at the first record (P-CBACT04C-010).
#[derive(Debug, Clone, PartialEq, Eq)]
struct CurrentAccount {
    /// `WS-LAST-ACCT-NUM`: the `TRANCAT-ACCT-ID` of the record that started the
    /// account [app/cbl/CBACT04C.cbl:L201]. `1100-GET-ACCT-DATA` reads the
    /// account by this key, so it is also the `ACCT-ID` the interest
    /// description and the rewrite use.
    acct_num: AccountId,
    /// `ACCT-GROUP-ID` as `1100-GET-ACCT-DATA` read it: the group of every
    /// rate read of the account (`1200-GET-INTEREST-RATE`, P-CBACT04C-004).
    group_id: AccountGroupId,
    /// `CARD-XREF-RECORD` as `1110-GET-XREF-DATA` read it.
    xref: CardXref,
    /// `WS-TOTAL-INT PIC S9(09)V99`, set to 0 at the break (P-CBACT04C-190).
    total: Money<9>,
}

/// One `INTCALC` run: the state `CBACT04C` keeps across the balance records.
///
/// The job calls [`InterestRun::process_record`] for every `TCATBALF` record
/// in key order and [`InterestRun::finish`] at end of file. After an
/// [`InterestAbend`] the run is over, as the COBOL program is, and the job
/// stops calling it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterestRun {
    /// `PARM-DATE PIC X(10)`, the JCL `PARM` (`2022071800` in INTCALC,
    /// P-CBACT04C-001): the first ten bytes of every interest TRAN-ID.
    parm_date: Alnum<10>,
    /// `WS-TRANID-SUFFIX PIC 9(06)`, the last suffix issued.
    suffix: u32,
    /// The account being read: `None` before the first record, which plays
    /// the part of `WS-FIRST-TIME = 'Y'` and of `WS-LAST-ACCT-NUM` still
    /// holding spaces.
    current: Option<CurrentAccount>,
    /// `WS-RECORD-COUNT`.
    records_read: u64,
    /// Interest records returned.
    interest_records: u64,
    /// Accounts rewritten.
    accounts_updated: u64,
    /// Rate reads that fell back to `DEFAULT`.
    default_group_reads: u64,
}

impl InterestRun {
    /// A run whose interest TRAN-IDs start with `parm_date`, the `PARM` of the
    /// `INTCALC` step [app/jcl/INTCALC.jcl:L22]. The suffix starts at 0, so the
    /// first interest record is `<parm_date>000001` (P-CBACT04C-008).
    #[must_use]
    pub fn new(parm_date: Alnum<10>) -> Self {
        Self {
            parm_date,
            suffix: 0,
            current: None,
            records_read: 0,
            interest_records: 0,
            accounts_updated: 0,
            default_group_reads: 0,
        }
    }

    /// CBACT04C PROCEDURE DIVISION main loop [app/cbl/CBACT04C.cbl:L188-218] —
    /// P-CBACT04C-002, P-CBACT04C-006, P-CBACT04C-007, P-CBACT04C-022: one
    /// balance record of `TCATBALF`, read in key order.
    ///
    /// When the record's account differs from the previous record's, the
    /// previous account is re-read with a row lock and rewritten with its
    /// interest total added (skipped for the first record), so the changes
    /// other programs committed to it since its first record survive. The
    /// total is then set to zero and the new account and its cross-reference
    /// are read. The rate of the account's disclosure group
    /// for the record's type and category is then read, with the `DEFAULT`
    /// group as fallback. A zero rate computes nothing and returns `None`,
    /// leaving the total and the TRAN-ID suffix unchanged. Any other rate
    /// returns the interest record for `SYSTRAN(+1)`, stamped from `clock`.
    ///
    /// # Errors
    ///
    /// [`InterestAbend::AccountRewrite`], [`InterestAbend::AccountRead`],
    /// [`InterestAbend::XrefRead`], [`InterestAbend::DisclosureGroupRead`] or
    /// [`InterestAbend::DefaultDisclosureGroupRead`] when the port call of
    /// that paragraph fails. Writes made before the failure stay made.
    pub async fn process_record<S>(
        &mut self,
        store: &mut S,
        record: &TranCatBalance,
        clock: &impl Clock,
    ) -> Result<Option<NewTransaction>, InterestAbend>
    where
        S: AccountReader + AccountWriter + XrefReader + DisclosureGroupReader,
    {
        self.records_read += 1;
        let acct_num = record.key.trancat_acct_id;
        let current = match self.current.take() {
            Some(current) if current.acct_num == acct_num => self.current.insert(current),
            previous => {
                if let Some(previous) = previous {
                    update_account(store, &previous).await?;
                    self.accounts_updated += 1;
                }
                let opened = open_account(store, acct_num).await?;
                self.current.insert(opened)
            }
        };
        let (rate, defaulted) = rate_for(store, current.group_id, record).await?;
        if defaulted {
            self.default_group_reads += 1;
        }
        if rate.is_zero() {
            return Ok(None);
        }
        let monthly = monthly_interest(record.tran_cat_bal, rate);
        // CBACT04C 1300-COMPUTE-INTEREST — P-CBACT04C-020: ADD WS-MONTHLY-INT
        // TO WS-TOTAL-INT, exact, then stored into S9(09)V99.
        current.total = (current.total + monthly).store::<9>();
        // CBACT04C 1300-B-WRITE-TX — P-CBACT04C-008: ADD 1 TO WS-TRANID-SUFFIX.
        // The 9(06) receiver has no ON SIZE ERROR, so 999999 + 1 keeps its
        // low-order six digits, 000000, as every store without ON SIZE ERROR
        // truncates high-order digits.
        self.suffix = (self.suffix + 1) % TRANID_SUFFIX_MODULUS;
        self.interest_records += 1;
        Ok(Some(systran_record(
            self.parm_date,
            self.suffix,
            current,
            monthly,
            clock,
        )))
    }

    /// CBACT04C end of file [app/cbl/CBACT04C.cbl:L188-222] — P-CBACT04C-003,
    /// P-CBACT04C-182, E-14: the run's counts and the last account's unposted
    /// interest.
    ///
    /// The `PERFORM UNTIL END-OF-FILE = 'Y'` loop exits as soon as the read
    /// sets end of file, so the `ELSE` branch that would perform
    /// `1050-UPDATE-ACCOUNT` for the last account is never reached. The last
    /// account is therefore not rewritten: it keeps its balance and cycle
    /// amounts, while its interest records were already returned.
    #[must_use]
    pub fn finish(self) -> InterestSummary {
        InterestSummary {
            records_read: self.records_read,
            interest_records: self.interest_records,
            accounts_updated: self.accounts_updated,
            default_group_reads: self.default_group_reads,
            unposted: self.current.map(|current| UnpostedInterest {
                acct_id: current.acct_num,
                total_interest: current.total,
            }),
        }
    }
}

/// CBACT04C 1100-GET-ACCT-DATA and 1110-GET-XREF-DATA — P-CBACT04C-002,
/// P-CBACT04C-190: the account starting at an account break, with
/// `WS-TOTAL-INT` set to 0.
///
/// The account is read by `TRANCAT-ACCT-ID` and its cross-reference by the
/// same account on the alternate index `CXACAIX`
/// [app/cbl/CBACT04C.cbl:L200-205] [app/cbl/CBACT04C.cbl:L372-413]. Of the
/// account record only its group ID is kept, for the rate reads; the rewrite
/// at the next break reads the record again.
async fn open_account<S>(
    store: &mut S,
    acct_num: AccountId,
) -> Result<CurrentAccount, InterestAbend>
where
    S: AccountReader + XrefReader,
{
    let account =
        store
            .read_account(acct_num)
            .await
            .map_err(|cause| InterestAbend::AccountRead {
                acct_id: acct_num,
                cause,
            })?;
    let xref = store
        .read_xref_by_account(acct_num)
        .await
        .map_err(|cause| InterestAbend::XrefRead {
            acct_id: acct_num,
            cause,
        })?;
    Ok(CurrentAccount {
        acct_num,
        group_id: account.acct_group_id,
        xref,
        total: Money::ZERO,
    })
}

/// CBACT04C 1050-UPDATE-ACCOUNT [app/cbl/CBACT04C.cbl:L350-370] —
/// P-CBACT04C-010, P-CBACT04C-021, P-CBACT04C-191, P-CBACT04C-192: the
/// account of `finished` rewritten with its interest.
///
/// The account is re-read with a row lock (`read_account_for_update`) and
/// the paragraph's three statements are applied to that record: `ADD
/// WS-TOTAL-INT TO ACCT-CURR-BAL`, exact and stored into `S9(10)V99` with
/// high-order digits truncated, and both cycle amounts set to zero, whatever
/// `POSTTRAN` accumulated in them. Every other field is rewritten as the
/// re-read found it. The balance records before the break were committed one
/// by one, so a bill payment, account update or posting committed since the
/// account's first record is in the re-read record and survives; the lock
/// holds off any other writer from the re-read until the caller commits the
/// rewrite. Without concurrent writers the re-read returns the record
/// `1100-GET-ACCT-DATA` read, as in the COBOL, where nothing else changes the
/// account during the run.
///
/// # Errors
///
/// [`InterestAbend::AccountRewrite`] when the re-read or the rewrite fails.
/// A missing account is status 23, as a `REWRITE` of a record that no longer
/// exists; a lock that cannot be taken is status 30.
async fn update_account<S>(store: &mut S, finished: &CurrentAccount) -> Result<(), InterestAbend>
where
    S: AccountReader + AccountWriter,
{
    let acct_id = finished.acct_num;
    let rewrite_failed = move |cause| InterestAbend::AccountRewrite { acct_id, cause };
    let mut account = store
        .read_account_for_update(acct_id)
        .await
        .map_err(rewrite_failed)?;
    account.acct_curr_bal = (account.acct_curr_bal + finished.total).store::<10>();
    account.acct_curr_cyc_credit = Money::ZERO;
    account.acct_curr_cyc_debit = Money::ZERO;
    store.rewrite_account(account).await.map_err(rewrite_failed)
}

/// CBACT04C 1200-GET-INTEREST-RATE and 1200-A-GET-DEFAULT-INT-RATE
/// [app/cbl/CBACT04C.cbl:L210-213] [app/cbl/CBACT04C.cbl:L415-460] —
/// P-CBACT04C-004, P-CBACT04C-005, P-CBACT04C-193, E-06: the annual rate for
/// `record`, and whether it came from the `DEFAULT` group fallback.
///
/// The key is the account's `ACCT-GROUP-ID`, `group_id`, with the record's
/// type and category. A missing key (status 23) reads the same type and
/// category under the group `DEFAULT`; the fixture accounts all reach that
/// read, because their group IDs are blank.
async fn rate_for<S>(
    store: &mut S,
    group_id: AccountGroupId,
    record: &TranCatBalance,
) -> Result<(InterestRate, bool), InterestAbend>
where
    S: DisclosureGroupReader,
{
    let key = DisclosureGroupKey {
        dis_acct_group_id: group_id,
        dis_tran_type_cd: record.key.trancat_type_cd,
        dis_tran_cat_cd: record.key.trancat_cd,
    };
    match store.read_disclosure_group(key.clone()).await {
        Ok(group) => Ok((group.dis_int_rate, false)),
        Err(PortError::NotFound) => {
            let key = DisclosureGroupKey::default_group(key.dis_tran_type_cd, key.dis_tran_cat_cd);
            store
                .read_disclosure_group(key.clone())
                .await
                .map(|group| (group.dis_int_rate, true))
                .map_err(|cause| InterestAbend::DefaultDisclosureGroupRead { key, cause })
        }
        Err(cause) => Err(InterestAbend::DisclosureGroupRead { key, cause }),
    }
}

/// CBACT04C 1300-COMPUTE-INTEREST [app/cbl/CBACT04C.cbl:L464-465] —
/// P-CBACT04C-012 to P-CBACT04C-019: `WS-MONTHLY-INT = (TRAN-CAT-BAL *
/// DIS-INT-RATE) / 1200`.
///
/// The product is exact, the quotient is truncated toward zero to whole cents
/// with its sign kept, and the result keeps the low-order nine integer digits
/// of `S9(09)V99`; nothing rounds. A negative value that truncates to zero is
/// stored as positive zero (P-CBACT04C-018, E-42). For example 1.00 at 15.00
/// gives 0.01, -1.00 at 15.00 gives -0.01, and 999999999.99 at 9999.99 gives
/// 333324999.91.
#[must_use]
pub fn monthly_interest(balance: Money<9>, rate: InterestRate) -> Money<9> {
    (balance * rate).monthly().store::<9>()
}

/// CBACT04C 1300-B-WRITE-TX and Z-GET-DB2-FORMAT-TIMESTAMP
/// [app/cbl/CBACT04C.cbl:L473-498] [app/cbl/CBACT04C.cbl:L613-626] —
/// P-CBACT04C-008, P-CBACT04C-173, P-CBACT04C-176: the interest record of
/// `amount` for the account `current`, with TRAN-ID suffix `suffix`.
///
/// TRAN-ID is `PARM-DATE` followed by the six suffix digits; the type is 01,
/// the category 0005 and the source `System`. The description is
/// `Int. for a/c ` and the eleven digits of `ACCT-ID`, 24 bytes. `STRING`
/// leaves bytes 25 to 100 of `TRAN-DESC` as the working storage held them,
/// which the COBOL never initializes; they are written as spaces and are
/// excluded from parity (E-25). The merchant is 0 with blank name, city and
/// ZIP, the card is the account's `XREF-CARD-NUM`, and both timestamps are
/// the one DB2-format timestamp of the processing clock.
fn systran_record(
    parm_date: Alnum<10>,
    suffix: u32,
    current: &CurrentAccount,
    amount: Money<9>,
    clock: &impl Clock,
) -> NewTransaction {
    let suffix_digits = format!("{suffix:06}");
    let tran_id_bytes = [parm_date.as_bytes().as_slice(), suffix_digits.as_bytes()].concat();
    let timestamp = Alnum::move_from(&db2_timestamp(clock.now()));
    NewTransaction {
        tran_id: TranId::from_alnum(Alnum::move_from_bytes(&tran_id_bytes)),
        tran_type_cd: INTEREST_TRAN_TYPE_CD,
        tran_cat_cd: INTEREST_TRAN_CAT_CD,
        tran_source: Alnum::move_from(INTEREST_TRAN_SOURCE),
        tran_desc: Alnum::move_from(&format!("{INTEREST_DESC_PREFIX}{}", current.acct_num)),
        tran_amt: amount,
        tran_merchant_id: NO_MERCHANT,
        tran_merchant_name: Alnum::spaces(),
        tran_merchant_city: Alnum::spaces(),
        tran_merchant_zip: Alnum::spaces(),
        tran_card_num: current.xref.xref_card_num,
        tran_orig_ts: timestamp,
        tran_proc_ts: timestamp,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::future::Future;

    use chrono::NaiveDate;

    use super::{
        InterestAbend, InterestFile, InterestRun, InterestSummary, UnpostedInterest,
        monthly_interest,
    };
    use crate::clock::FixedClock;
    use crate::entities::{
        Account, CardXref, DisclosureGroup, DisclosureGroupKey, NewTransaction, TranCatBalance,
        TranCatBalanceKey,
    };
    use crate::messages::{
        ABENDING_PROGRAM, ERROR_CLOSING_ACCOUNT_FILE, ERROR_CLOSING_CROSS_REF_FILE,
        ERROR_CLOSING_DISCLOSURE_GROUP_FILE, ERROR_CLOSING_TRANSACTION_BALANCE_FILE,
        ERROR_OPENING_ACCOUNT_MASTER_FILE, ERROR_OPENING_DALY_REJECTS_FILE,
        ERROR_OPENING_TRANSACTION_CATEGORY_BALANCE, ERROR_RE_WRITING_ACCOUNT_FILE,
        ERROR_READING_ACCOUNT_FILE, ERROR_READING_DEFAULT_DISCLOSURE_GROUP,
        ERROR_READING_DISCLOSURE_GROUP_FILE, ERROR_READING_TRANSACTION_CATEGORY_FILE,
        ERROR_READING_XREF_FILE, ERROR_WRITING_TRANSACTION_RECORD,
    };
    use crate::numeric::{
        AccountGroupId, AccountId, Alnum, CardNumber, CustomerId, InterestRate, Money,
        TranCategoryCode, TranTypeCode,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    // The cards of fixture accounts 1, 2 and 50 [app/data/ASCII/cardxref.txt]
    // (P-CBACT04C-123, P-CBACT04C-124, P-CBACT04C-172).
    const CARD_1: &str = "9680294154603697";
    const CARD_2: &str = "0923877193247330";
    const CARD_50: &str = "0500024453765740";

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn cents<const D: u8>(value: i64) -> Money<D> {
        Money::from_cents(value).expect("an amount within the field")
    }

    fn rate(hundredths: i32) -> InterestRate {
        InterestRate::from_hundredths(hundredths).expect("a rate within S9(04)V99")
    }

    fn category(value: u16) -> TranCategoryCode {
        TranCategoryCode::new(value).expect("a four-digit category")
    }

    /// An account with a blank group ID, as every fixture account (E-06).
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

    fn xref(card: &str, id: u64) -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(card),
            xref_cust_id: CustomerId::new(1).expect("a nine-digit customer"),
            xref_acct_id: account_id(id),
        }
    }

    fn group(group_id: &str, type_cd: &str, cat_cd: u16, hundredths: i32) -> DisclosureGroup {
        DisclosureGroup {
            key: DisclosureGroupKey {
                dis_acct_group_id: AccountGroupId::move_from(group_id),
                dis_tran_type_cd: TranTypeCode::move_from(type_cd),
                dis_tran_cat_cd: category(cat_cd),
            },
            dis_int_rate: rate(hundredths),
        }
    }

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

    /// 10:15:30.123456 on the INTCALC date, whose DB2-format timestamp is
    /// `2022-07-18-10.15.30.120000`.
    fn clock() -> FixedClock {
        let instant = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|date| date.and_hms_micro_opt(10, 15, 30, 123_456))
            .expect("a valid instant");
        FixedClock(instant.and_utc())
    }

    /// The JCL PARM of INTCALC [app/jcl/INTCALC.jcl:L22].
    fn parm_date() -> Alnum<10> {
        Alnum::move_from("2022071800")
    }

    /// The DEFAULT rows of the fixture disclosure groups for 01/0001 (15.00)
    /// and 03/0001 (0.00) [app/data/ASCII/discgrp.txt:L18]
    /// [app/data/ASCII/discgrp.txt:L25], and the `A000000000` 01/0001 row
    /// [app/data/ASCII/discgrp.txt:L1] that blank group IDs never reach.
    fn fixture_groups(store: FakeStore) -> FakeStore {
        store
            .with_disclosure_group(group("DEFAULT", "01", 1, 1_500))
            .with_disclosure_group(group("DEFAULT", "03", 1, 0))
            .with_disclosure_group(group("A000000000", "01", 1, 1_500))
    }

    /// Accounts 1, 2 and 50 as POSTTRAN leaves them, with their category
    /// balances in key order (P-CBACT04C-023 to P-CBACT04C-026,
    /// P-CBACT04C-123, P-CBACT04C-124, P-CBACT04C-172).
    fn fixture_slice() -> (FakeStore, Vec<TranCatBalance>) {
        let store = fixture_groups(FakeStore::default())
            .with_account(account(1, 128_810, 116_487, -7_077))
            .with_account(account(2, 173_497, 233_997, -76_300))
            .with_account(account(50, 194_587, 150_175, -4_788))
            .with_xref(xref(CARD_1, 1))
            .with_xref(xref(CARD_2, 2))
            .with_xref(xref(CARD_50, 50));
        let records = vec![
            balance(1, "01", 1, 116_487),
            balance(1, "03", 1, -7_077),
            balance(2, "01", 1, 233_997),
            balance(2, "03", 1, -76_300),
            balance(50, "01", 1, 150_175),
            balance(50, "03", 1, -4_788),
        ];
        (store, records)
    }

    /// Every record through one run, then its summary.
    fn run_all(
        store: &mut FakeStore,
        records: &[TranCatBalance],
    ) -> (Vec<Option<NewTransaction>>, InterestSummary) {
        let clock = clock();
        let mut run = InterestRun::new(parm_date());
        let outputs = records
            .iter()
            .map(|record| {
                block_on(run.process_record(store, record, &clock)).expect("no abend on the slice")
            })
            .collect();
        (outputs, run.finish())
    }

    /// The first record of account 1 through a fresh run.
    fn first_record(store: &mut FakeStore) -> Result<Option<NewTransaction>, InterestAbend> {
        let mut run = InterestRun::new(parm_date());
        block_on(run.process_record(store, &balance(1, "01", 1, 116_487), &clock()))
    }

    fn require_send<F: Future + Send>(future: F) -> F {
        future
    }

    #[test]
    fn monthly_interest_truncates_toward_zero_and_drops_high_order_digits() {
        // (TRAN-CAT-BAL cents, DIS-INT-RATE hundredths, WS-MONTHLY-INT cents, row)
        let cases = [
            (1, 100, 0, "P-CBACT04C-013"),
            (100, 1_500, 1, "P-CBACT04C-014"),
            (120_000, 100, 100, "P-CBACT04C-015"),
            (119_999, 100, 99, "P-CBACT04C-016"),
            (-100, 1_500, -1, "P-CBACT04C-017"),
            (-1, 100, 0, "P-CBACT04C-018"),
            (99_999_999_999, 999_999, 33_332_499_991, "P-CBACT04C-019"),
            (116_487, 1_500, 1_456, "P-CBACT04C-023"),
            (233_997, 1_500, 2_924, "P-CBACT04C-025"),
        ];
        for (balance_cents, hundredths, expected, row) in cases {
            let monthly = monthly_interest(cents(balance_cents), rate(hundredths));
            assert_eq!(monthly.cents(), expected, "{row}");
            assert!(
                !monthly.is_negative() || expected < 0,
                "{row}: zero is positive"
            );
        }
    }

    #[test]
    fn fixture_slice_posts_every_account_but_the_last() {
        let (mut store, records) = fixture_slice();
        let (outputs, summary) = run_all(&mut store, &records);

        // A zero rate (every 03/0001 balance) returns no record and leaves the
        // suffix alone (P-CBACT04C-006, P-CBACT04C-022, P-CBACT04C-024).
        let issued: Vec<Option<(String, i64)>> = outputs
            .iter()
            .map(|output| {
                output
                    .as_ref()
                    .map(|tran| (tran.tran_id.to_string(), tran.tran_amt.cents()))
            })
            .collect();
        assert_eq!(
            issued,
            [
                Some(("2022071800000001".to_owned(), 1_456)),
                None,
                Some(("2022071800000002".to_owned(), 2_924)),
                None,
                Some(("2022071800000003".to_owned(), 1_877)),
                None,
            ]
        );

        // Accounts 1 and 2 are rewritten at the next break with their total
        // and zero cycle amounts (P-CBACT04C-123, P-CBACT04C-124).
        for (id, expected_balance) in [(1, 130_266), (2, 176_421)] {
            let stored = &store.accounts[&account_id(id)];
            assert_eq!(stored.acct_curr_bal.cents(), expected_balance, "{id}");
            assert!(stored.acct_curr_cyc_credit.is_zero(), "{id}");
            assert!(stored.acct_curr_cyc_debit.is_zero(), "{id}");
        }
        // The last account keeps its balance and cycle amounts (E-14,
        // P-CBACT04C-172).
        assert_eq!(
            store.accounts[&account_id(50)],
            account(50, 194_587, 150_175, -4_788)
        );
        assert_eq!(
            store
                .calls
                .iter()
                .filter(|call| **call == "rewrite_account")
                .count(),
            2
        );
        assert_eq!(
            summary,
            InterestSummary {
                records_read: 6,
                interest_records: 3,
                accounts_updated: 2,
                default_group_reads: 6,
                unposted: Some(UnpostedInterest {
                    acct_id: account_id(50),
                    total_interest: cents(1_877),
                }),
            }
        );
    }

    #[test]
    fn interest_record_carries_the_systran_fields() {
        let (mut store, records) = fixture_slice();
        let (outputs, _) = run_all(&mut store, &records[..1]);
        let tran = outputs
            .into_iter()
            .next()
            .flatten()
            .expect("an interest record");

        // P-CBACT04C-008, P-CBACT04C-173: TRAN-ID is PARM-DATE and the suffix.
        assert_eq!(tran.tran_id.as_bytes(), b"2022071800000001");
        assert_eq!(tran.tran_type_cd.as_bytes(), b"01");
        assert_eq!(tran.tran_cat_cd.to_string(), "0005");
        assert_eq!(tran.tran_source.as_bytes(), b"System    ");
        let description = tran.tran_desc.as_bytes();
        assert_eq!(&description[..24], b"Int. for a/c 00000000001");
        // Bytes 25 to 100 are never set by the COBOL and written as spaces
        // (E-25).
        assert!(description[24..].iter().all(|byte| *byte == b' '));
        assert_eq!(tran.tran_amt.cents(), 1_456);
        assert_eq!(tran.tran_merchant_id.to_string(), "000000000");
        assert!(tran.tran_merchant_name.is_blank());
        assert!(tran.tran_merchant_city.is_blank());
        assert!(tran.tran_merchant_zip.is_blank());
        assert_eq!(tran.tran_card_num, CardNumber::move_from(CARD_1));
        // P-CBACT04C-176: both timestamps hold one DB2-format timestamp.
        assert_eq!(tran.tran_orig_ts.as_bytes(), b"2022-07-18-10.15.30.120000");
        assert_eq!(tran.tran_proc_ts, tran.tran_orig_ts);
    }

    #[test]
    fn rate_comes_from_the_account_group_or_else_from_default() {
        // (ACCT-GROUP-ID, disclosure group reads, DEFAULT fallbacks, rate used)
        let cases = [("", 2, 1, 1_456), ("A000000000", 1, 0, 2_912)];
        for (group_id, reads, fallbacks, expected) in cases {
            let mut own = account(1, 0, 0, 0);
            own.acct_group_id = AccountGroupId::move_from(group_id);
            let mut store = FakeStore::default()
                .with_disclosure_group(group("DEFAULT", "01", 1, 1_500))
                .with_disclosure_group(group("A000000000", "01", 1, 3_000))
                .with_account(own)
                .with_xref(xref(CARD_1, 1));
            let (outputs, summary) = run_all(&mut store, &[balance(1, "01", 1, 116_487)]);
            let amount = outputs[0].as_ref().map(|tran| tran.tran_amt.cents());
            assert_eq!(amount, Some(expected), "{group_id:?}");
            assert_eq!(summary.default_group_reads, fallbacks, "{group_id:?}");
            let group_reads = store
                .calls
                .iter()
                .filter(|call| **call == "read_disclosure_group")
                .count();
            assert_eq!(group_reads, reads, "{group_id:?}");
        }
    }

    #[test]
    fn totals_and_balances_keep_their_low_order_digits() {
        // Four maximal categories: each monthly interest is 333324999.91 and
        // the S9(09)V99 total of the four keeps 333299999.64 (P-CBACT04C-019,
        // P-CBACT04C-020). An S9(10)V99 balance of 9999999999.99 plus 1.00
        // keeps 0.99 (P-CBACT04C-021).
        let cases = [
            (0, vec![99_999_999_999; 4], 999_999, 33_329_999_964),
            (999_999_999_999, vec![120_000], 100, 99),
        ];
        for (opening, balances, hundredths, expected) in cases {
            let mut store = FakeStore::default()
                .with_account(account(1, opening, 0, 0))
                .with_account(account(2, 0, 0, 0))
                .with_xref(xref(CARD_1, 1))
                .with_xref(xref(CARD_2, 2));
            let mut records = Vec::new();
            for (code, amount) in (1..).zip(&balances) {
                store = store.with_disclosure_group(group("DEFAULT", "01", code, hundredths));
                records.push(balance(1, "01", code, *amount));
            }
            store = store.with_disclosure_group(group("DEFAULT", "01", 1, hundredths));
            records.push(balance(2, "01", 1, 0));
            run_all(&mut store, &records);
            assert_eq!(
                store.accounts[&account_id(1)].acct_curr_bal.cents(),
                expected
            );
        }
    }

    #[test]
    fn tran_id_suffix_wraps_at_six_digits() {
        let (mut store, records) = fixture_slice();
        let mut run = InterestRun::new(parm_date());
        run.suffix = 999_999;
        let tran = block_on(run.process_record(&mut store, &records[0], &clock()))
            .expect("no abend")
            .expect("an interest record");
        assert_eq!(tran.tran_id.as_bytes(), b"2022071800000000");
    }

    #[test]
    fn a_failed_port_call_abends_with_the_cobol_displays() {
        let unavailable = || PortError::Unavailable("connection reset".to_owned());
        let cases = [
            (
                "read_account",
                PortError::NotFound,
                vec![
                    "ACCOUNT NOT FOUND: 00000000001",
                    ERROR_READING_ACCOUNT_FILE,
                    "FILE STATUS IS: NNNN0023",
                ],
            ),
            (
                "read_account",
                unavailable(),
                vec![ERROR_READING_ACCOUNT_FILE, "FILE STATUS IS: NNNN0030"],
            ),
            (
                "read_xref_by_account",
                PortError::NotFound,
                vec![
                    "ACCOUNT NOT FOUND: 00000000001",
                    ERROR_READING_XREF_FILE,
                    "FILE STATUS IS: NNNN0023",
                ],
            ),
            (
                "read_disclosure_group",
                unavailable(),
                vec![
                    ERROR_READING_DISCLOSURE_GROUP_FILE,
                    "FILE STATUS IS: NNNN0030",
                ],
            ),
        ];
        for (method, cause, expected) in cases {
            let (mut store, _) = fixture_slice();
            store.fail_next(method, cause.clone());
            let abend = first_record(&mut store).expect_err("an abend");
            let mut lines = expected;
            lines.push(ABENDING_PROGRAM);
            assert_eq!(abend.lines(), lines, "{method} {cause:?}");
            assert_eq!(abend.to_string(), lines.join("\n"), "{method}");
            let source = abend
                .source()
                .and_then(|source| source.downcast_ref::<PortError>());
            assert_eq!(source, Some(&cause), "{method}");
        }
    }

    #[test]
    fn a_missing_default_group_abends() {
        // P-CBACT04C-005: the account's group and DEFAULT both end status 23.
        let (mut store, _) = fixture_slice();
        store.disclosure_groups.clear();
        let abend = first_record(&mut store).expect_err("an abend");
        assert!(
            matches!(&abend, InterestAbend::DefaultDisclosureGroupRead { key, cause: PortError::NotFound } if key.is_default_group()),
            "{abend:?}"
        );
        assert_eq!(
            abend.lines(),
            [
                ERROR_READING_DEFAULT_DISCLOSURE_GROUP,
                "FILE STATUS IS: NNNN0023",
                ABENDING_PROGRAM,
            ]
        );
    }

    #[test]
    fn a_change_committed_before_the_break_survives_the_rewrite() {
        // P-CBACT04C-010, P-CBACT04C-191, P-CBACT04C-192: a payment and an
        // address change committed to account 1 after its first balance record
        // are in the record 1050-UPDATE-ACCOUNT re-reads, which receives the
        // interest and the cycle resets and nothing else.
        let (mut store, records) = fixture_slice();
        let clock = clock();
        let mut run = InterestRun::new(parm_date());
        for record in &records[..2] {
            block_on(run.process_record(&mut store, record, &clock)).expect("account 1 posts");
        }
        let mut changed = account(1, -10_000, 116_487, -7_077);
        changed.acct_addr_zip = Alnum::move_from("Z8888");
        store.accounts.insert(account_id(1), changed.clone());
        store.calls.clear();

        block_on(run.process_record(&mut store, &records[2], &clock)).expect("account 2 posts");

        // -100.00 plus the 14.56 of account 1 (P-CBACT04C-023).
        let mut expected = changed;
        expected.acct_curr_bal = cents(-8_544);
        expected.acct_curr_cyc_credit = Money::ZERO;
        expected.acct_curr_cyc_debit = Money::ZERO;
        assert_eq!(store.accounts[&account_id(1)], expected);
        // Account 1 is re-read and rewritten before account 2 and its rate
        // are read (P-CBACT04C-002).
        assert_eq!(
            store.calls,
            [
                "read_account_for_update",
                "rewrite_account",
                "read_account",
                "read_xref_by_account",
                "read_disclosure_group",
                "read_disclosure_group",
            ]
        );
    }

    #[test]
    fn a_failed_rewrite_abends_before_the_next_account_is_read() {
        // P-CBACT04C-010: REWRITE has no INVALID KEY clause, so a missing
        // account shows only its status. The locked re-read before the
        // rewrite fails as the rewrite does: a record gone since its first
        // balance record is status 23, a lock that cannot be taken 30.
        // (failing method, cause, status display)
        let cases = [
            (
                "rewrite_account",
                PortError::NotFound,
                "FILE STATUS IS: NNNN0023",
            ),
            (
                "read_account_for_update",
                PortError::NotFound,
                "FILE STATUS IS: NNNN0023",
            ),
            (
                "read_account_for_update",
                PortError::Contention,
                "FILE STATUS IS: NNNN0030",
            ),
        ];
        for (method, cause, status) in cases {
            let (mut store, records) = fixture_slice();
            store.fail_next(method, cause.clone());
            let clock = clock();
            let mut run = InterestRun::new(parm_date());
            for record in &records[..2] {
                block_on(run.process_record(&mut store, record, &clock)).expect("account 1 posts");
            }
            let abend = block_on(run.process_record(&mut store, &records[2], &clock))
                .expect_err("the rewrite of account 1 abends");
            assert_eq!(
                abend,
                InterestAbend::AccountRewrite {
                    acct_id: account_id(1),
                    cause: cause.clone(),
                },
                "{method}"
            );
            assert_eq!(
                abend.lines(),
                [ERROR_RE_WRITING_ACCOUNT_FILE, status, ABENDING_PROGRAM],
                "{method}"
            );
            let source = abend
                .source()
                .and_then(|source| source.downcast_ref::<PortError>());
            assert_eq!(source, Some(&cause), "{method}");
            // Account 2 is never read, and account 1 keeps its record.
            assert_eq!(store.calls.last(), Some(&method), "{method}");
            assert_eq!(
                store.accounts[&account_id(1)],
                account(1, 128_810, 116_487, -7_077),
                "{method}"
            );
        }
    }

    #[test]
    fn job_side_abends_render_their_paragraph_texts() {
        let unavailable = || PortError::Unavailable("relation does not exist".to_owned());
        let open = |file| InterestAbend::FileOpen {
            file,
            cause: unavailable(),
        };
        let close = |file| InterestAbend::FileClose {
            file,
            cause: unavailable(),
        };
        let status_30 = "FILE STATUS IS: NNNN0030";
        let cases = [
            (
                InterestAbend::TcatbalRead {
                    cause: PortError::Unavailable("timeout".to_owned()),
                },
                ERROR_READING_TRANSACTION_CATEGORY_FILE,
                status_30,
                true,
            ),
            (
                InterestAbend::SystranWrite { status: *b"34" },
                ERROR_WRITING_TRANSACTION_RECORD,
                "FILE STATUS IS: NNNN0034",
                false,
            ),
            // The opens, in their COBOL order: the cross-reference text carries
            // XREFFILE-STATUS, and the disclosure group text is the daily
            // rejects one (E-72).
            (
                open(InterestFile::Tcatbal),
                ERROR_OPENING_TRANSACTION_CATEGORY_BALANCE,
                status_30,
                true,
            ),
            (
                open(InterestFile::Xref),
                "ERROR OPENING CROSS REF FILE30",
                status_30,
                true,
            ),
            (
                open(InterestFile::DisclosureGroup),
                ERROR_OPENING_DALY_REJECTS_FILE,
                status_30,
                true,
            ),
            (
                open(InterestFile::Account),
                ERROR_OPENING_ACCOUNT_MASTER_FILE,
                status_30,
                true,
            ),
            (
                close(InterestFile::Tcatbal),
                ERROR_CLOSING_TRANSACTION_BALANCE_FILE,
                status_30,
                true,
            ),
            (
                close(InterestFile::Xref),
                ERROR_CLOSING_CROSS_REF_FILE,
                status_30,
                true,
            ),
            (
                close(InterestFile::DisclosureGroup),
                ERROR_CLOSING_DISCLOSURE_GROUP_FILE,
                status_30,
                true,
            ),
            (
                close(InterestFile::Account),
                ERROR_CLOSING_ACCOUNT_FILE,
                status_30,
                true,
            ),
        ];
        for (abend, text, status, has_source) in cases {
            assert_eq!(abend.lines(), [text, status, ABENDING_PROGRAM]);
            assert_eq!(abend.source().is_some(), has_source, "{abend:?}");
        }
        assert_eq!(InterestAbend::ABCODE, 999);
    }

    #[test]
    fn an_empty_run_reports_nothing_unposted() {
        let summary = InterestRun::new(parm_date()).finish();
        assert_eq!(
            summary,
            InterestSummary {
                records_read: 0,
                interest_records: 0,
                accounts_updated: 0,
                default_group_reads: 0,
                unposted: None,
            }
        );
    }

    #[test]
    fn process_record_is_send_for_a_send_store() {
        let (mut store, records) = fixture_slice();
        let clock = clock();
        let mut run = InterestRun::new(parm_date());
        let output = block_on(require_send(run.process_record(
            &mut store,
            &records[0],
            &clock,
        )));
        assert!(matches!(output, Ok(Some(_))));
    }
}
