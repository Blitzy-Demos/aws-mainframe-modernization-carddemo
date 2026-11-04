//! `batch intcalc`: JCL `INTCALC` step `STEP15`, `EXEC PGM=CBACT04C,PARM='2022071800'`
//! [app/jcl/INTCALC.jcl:L22-41], the monthly interest on the transaction category balances.
//!
//! The program logic is `carddemo_domain::rules::interest`. This job supplies what the JCL and
//! the file I/O around the rule supply:
//!
//! | CBACT04C | Audit rows | Here |
//! |---|---|---|
//! | `PROCEDURE DIVISION USING EXTERNAL-PARMS`, `PARM-DATE X(10)` | P-CBACT04C-001 | `parm_date` |
//! | `0000-TCATBALF-OPEN` to `0300-ACCTFILE-OPEN` | P-CBACT04C-177 to P-CBACT04C-180, P-CBACT04C-194 | `open_files` |
//! | `0400-TRANFILE-OPEN`, `TRANSACT DD DSN=SYSTRAN(+1)` | P-CBACT04C-181 | `open_systran` |
//! | `1000-TCATBALF-GET-NEXT` | P-CBACT04C-182 | `read_page` |
//! | the loop body, `1050-UPDATE-ACCOUNT` to `1300-COMPUTE-INTEREST` | P-CBACT04C-002 to P-CBACT04C-010 | `InterestRun::process_record`, called by `IntcalcJob::process` |
//! | `1300-B-WRITE-TX` `WRITE FD-TRANFILE-REC` | P-CBACT04C-008, P-CBACT04C-009 | `IntcalcJob::write_systran` |
//! | end of file, the last account never updated (E-14) | P-CBACT04C-003, P-CBACT04C-174 | `InterestRun::finish`, called by `IntcalcJob::end_of_file` |
//! | `9000-TCATBALF-CLOSE` to `9300-ACCTFILE-CLOSE` | P-CBACT04C-183 to P-CBACT04C-186 | `close_files` in `IntcalcJob::end_of_file` |
//! | `9400-TRANFILE-CLOSE`, `DISP=(NEW,CATLG,DELETE)` | P-CBACT04C-187 | `PendingGeneration::catalog_async` in `IntcalcJob::end_of_file` |
//! | `9999-ABEND-PROGRAM`, `9910-DISPLAY-IO-STATUS` | P-CBACT04C-188, P-CBACT04C-189 | `abend_status`, `systran_abend` |
//!
//! # Files and units of work
//!
//! `TCATBALF`, `XREFFILE`, `DISCGRP` and `ACCTFILE` are tables of one database here. The job
//! layer opened the pool before this job runs, and a failure to reach it is a setup failure
//! (exit 16, as an abend). The `OPEN` and `CLOSE` of each of the four KSDS files become a check
//! that its table answers (`check_file`): one statement on the table in a unit of work of its
//! own that writes nothing, whose outcome is discarded. A table that does not answer, for
//! example one that is missing, is the open or close that ends with a status other than 00: the
//! paragraph's text, status 30 ([`NO_FURTHER_INFORMATION`]) and `ABEND 999`. The opens run in
//! the COBOL order before `SYSTRAN(+1)` is allocated, so a failed one reads and writes nothing;
//! the disclosure group open shows the daily rejects text, as written (E-72). The closes run in
//! the COBOL order after the end of the file, before `9400-TRANFILE-CLOSE`; a failed one leaves
//! the account rewrites committed and deletes the uncataloged `SYSTRAN(+1)`. A unit of work
//! that cannot begin or end is a setup failure.
//!
//! The category balances are browsed in key order a page at a time, each page in a read-only
//! unit of work. Every balance record is then processed in its own unit of work, committed once
//! the rule has rewritten the previous account (on an account break) and the record's interest
//! transaction is written to `SYSTRAN(+1)`. Every file CBACT04C rewrites is `RECOVERY(NONE)`
//! [app/csd/CARDDEMO.CSD:L1-96], so each `REWRITE` is durable when it completes and survives a
//! later abend (P-CBACT04C-010). A commit per record gives the account rewrites that durability,
//! and holds no account row lock beyond its record, so online bill payments and account updates
//! do not wait for the end of the run. Because the earlier records of an account are already
//! committed when its break comes, the rule re-reads the account with a row lock at the break
//! and adds the interest to that record, so a payment, account update or posting committed in
//! between keeps its changes; the lock lasts until the break record's commit.
//!
//! `DISCGRP` is reference data: CBACT04C opens it `INPUT`, `batch seed` loads it, and no online
//! program or other job writes it, so a key reads the same record, or the same status 23, for
//! the whole run. The job therefore keeps the outcome of each disclosure group key the run has
//! read and answers the repeated reads from it (`RecordStore`). The rule still issues the reads
//! of `1200-GET-INTEREST-RATE` and `1200-A-GET-DEFAULT-INT-RATE` for every balance record and
//! counts its `DEFAULT` fallbacks (P-CBACT04C-004, P-CBACT04C-005, P-CBACT04C-193); a read that
//! fails otherwise is never kept and abends as before.
//!
//! The allocation, every write and the catalog or discard of `SYSTRAN(+1)` run on Tokio's
//! blocking pool, inside the job span; a record's write still completes while its unit of work
//! is open, before that unit is committed.
//!
//! On an abend the record's unit of work is still committed, because the rule's account rewrite
//! ran under its own savepoint and persists: the failed read after it ran under its own savepoint
//! too, so it left the unit committable. The pending `SYSTRAN(+1)` is discarded
//! uncataloged, as `DISP=(NEW,CATLG,DELETE)` deletes it. The stderr lines are the `DISPLAY`
//! lines of the abending paragraph and `ABEND 999`; the adapter detail goes to the log only.
//!
//! No interest transaction enters the live transaction master: `SYSTRAN(0)` reaches it only
//! through `batch combtran`.

use std::collections::HashMap;
use std::error::Error;

use carddemo_codec::layouts::{TransactionLayout, TransactionRecord};
use carddemo_domain::entities::{
    Account, CardXref, DisclosureGroup, DisclosureGroupKey, NewTransaction, TranCatBalance,
    TranCatBalanceKey,
};
use carddemo_domain::messages::{
    ABENDING_PROGRAM, END_OF_EXECUTION_OF_PROGRAM_CBACT04C, ERROR_CLOSING_TRANSACTION_FILE,
    ERROR_OPENING_TRANSACTION_FILE, file_status_message,
};
use carddemo_domain::numeric::{
    AccountGroupId, AccountId, Alnum, CardNumber, TranCategoryCode, TranTypeCode,
};
use carddemo_domain::ports::{
    AccountReader, AccountWriter, DisclosureGroupReader, PortError, PortResult,
    TranCatBalanceStore, XrefReader,
};
use carddemo_domain::rules::interest::{InterestAbend, InterestFile, InterestRun};
use carddemo_persistence::{Db, UnitOfWork};
use tracing::Span;

use super::{JobContext, setup_failure, span_count};
use crate::cli::IntcalcArgs;
use crate::exit::ExitStatus;
use crate::generations::{Dataset, PendingGeneration};

/// The category balances one browse read returns: a bound on the records the job holds at
/// once, not a COBOL value. `1000-TCATBALF-GET-NEXT` reads one record at a time.
const PAGE_SIZE: u16 = 500;

/// The file status the job shows for a failed open, write or close of `SYSTRAN(+1)`: 30, the
/// permanent error with no further information. A file-system error has no QSAM status of its
/// own, and `rules::interest` shows every database failure without one as 30 too.
const NO_FURTHER_INFORMATION: [u8; 2] = *b"30";

/// The account key the check of `ACCTFILE` reads (`check_file`): 0, an `ACCT-ID` no fixture
/// account holds. A record and status 23 both show that the table answers.
const CHECK_ACCOUNT: AccountId = match AccountId::new(0) {
    Ok(id) => id,
    Err(_) => panic!("0 fits the eleven digits of ACCT-ID"),
};

/// The key the check of `DISCGRP` reads (`check_file`): a blank group, a blank type and
/// category 0, a key no fixture disclosure group holds. A record and status 23 both show that
/// the table answers.
const CHECK_GROUP: DisclosureGroupKey = DisclosureGroupKey {
    dis_acct_group_id: AccountGroupId::from_alnum(Alnum::from_array(*b"          ")),
    dis_tran_type_cd: TranTypeCode::from_alnum(Alnum::from_array(*b"  ")),
    dis_tran_cat_cd: match TranCategoryCode::new(0) {
        Ok(code) => code,
        Err(_) => panic!("0 fits the four digits of DIS-TRAN-CAT-CD"),
    },
};

/// Runs `batch intcalc` with `args` on `db`, and returns how CBACT04C ended: exit 0 at the end
/// of the category balance file, 16 on an abend or a setup failure.
///
/// Once `SYSTRAN(+1)` is open, however the reading ends, `read` (`WS-RECORD-COUNT`, the balance
/// records handed to the rule) and `posted` (the accounts `1050-UPDATE-ACCOUNT` rewrote in a
/// committed unit of work, the summary's `accounts_updated` at the end of the file) are set in
/// the job's counts, whose `rejected` stays 0 because CBACT04C rejects no record, and `written`
/// (the interest records written to `SYSTRAN(+1)`) is recorded on the job span. A run that
/// ends before `SYSTRAN(+1)` is open has read nothing and leaves every count 0.
pub(super) async fn run(ctx: &JobContext, db: &Db, args: &IntcalcArgs) -> ExitStatus {
    let parm = match parm_date(args) {
        Ok(parm) => parm,
        Err(status) => return status,
    };
    if let Err(status) = open_files(db).await {
        return status;
    }
    let systran = match open_systran(ctx).await {
        Ok(pending) => pending,
        Err(status) => return status,
    };
    let mut job = IntcalcJob {
        ctx,
        db,
        interest: InterestRun::new(parm),
        groups: DisclosureGroups::new(),
        systran,
        read: 0,
        posted: 0,
        written: 0,
    };
    let outcome = job.read_file().await;
    job.record_counts();
    match outcome {
        Ok(()) => job.end_of_file().await,
        Err(status) => {
            // The abnormal end of the step: `DISP=(NEW,CATLG,DELETE)` deletes `SYSTRAN(+1)`.
            let IntcalcJob { systran, .. } = job;
            systran.discard_async().await;
            status
        }
    }
}

/// CBACT04C `PROCEDURE DIVISION USING EXTERNAL-PARMS` [app/cbl/CBACT04C.cbl:L175-180] —
/// P-CBACT04C-001: the JCL `PARM` as `PARM-DATE X(10)`, the first ten bytes of every interest
/// TRAN-ID.
///
/// # Errors
///
/// A setup failure when the text is not exactly ten bytes, which the command line already
/// rejects.
fn parm_date(args: &IntcalcArgs) -> Result<Alnum<10>, ExitStatus> {
    Alnum::from_bytes(args.parm_date.as_bytes())
        .map_err(|err| setup_failure("PARM-DATE does not fill X(10)", err))
}

/// CBACT04C 0000-TCATBALF-OPEN, 0100-XREFFILE-OPEN, 0200-DISCGRP-OPEN and 0300-ACCTFILE-OPEN
/// [app/cbl/CBACT04C.cbl:L182-185] [app/cbl/CBACT04C.cbl:L234-305] — P-CBACT04C-177 to
/// P-CBACT04C-180, P-CBACT04C-194: `OPEN INPUT TCATBAL-FILE`, `XREF-FILE` and `DISCGRP-FILE` and
/// `OPEN I-O ACCOUNT-FILE`, in that order, each a [`check_file`]. Nothing is read or written
/// before them.
///
/// # Errors
///
/// - The abend of the first open whose table does not answer
///   ([`InterestAbend::FileOpen`]); the later files are not opened.
/// - A setup failure when a check's unit of work cannot begin or end.
async fn open_files(db: &Db) -> Result<(), ExitStatus> {
    for file in InterestFile::ALL {
        if let Err(cause) = check_file(db, file).await? {
            return Err(abend_status(&InterestAbend::FileOpen { file, cause }));
        }
    }
    Ok(())
}

/// CBACT04C 9000-TCATBALF-CLOSE, 9100-XREFFILE-CLOSE, 9200-DISCGRP-CLOSE and
/// 9300-ACCTFILE-CLOSE [app/cbl/CBACT04C.cbl:L224-227] [app/cbl/CBACT04C.cbl:L522-593] —
/// P-CBACT04C-183 to P-CBACT04C-186: the `CLOSE` of each KSDS after the main loop, in that
/// order, each a [`check_file`].
///
/// # Errors
///
/// - The abend of the first close whose table does not answer
///   ([`InterestAbend::FileClose`]); the later files are not closed.
/// - A setup failure when a check's unit of work cannot begin or end.
async fn close_files(db: &Db) -> Result<(), ExitStatus> {
    for file in InterestFile::ALL {
        if let Err(cause) = check_file(db, file).await? {
            return Err(abend_status(&InterestAbend::FileClose { file, cause }));
        }
    }
    Ok(())
}

/// CBACT04C 0000-TCATBALF-OPEN to 0300-ACCTFILE-OPEN and 9000-TCATBALF-CLOSE to
/// 9300-ACCTFILE-CLOSE — P-CBACT04C-177 to P-CBACT04C-180, P-CBACT04C-183 to P-CBACT04C-186:
/// the `OPEN` or `CLOSE` of the KSDS `file`, whose table must answer: one statement on it, in a
/// unit of work of its own that writes nothing and is rolled back, through
/// [`table_answers`]. `Ok(Err(cause))` when the table does not answer.
///
/// # Errors
///
/// A setup failure when the unit of work cannot begin or end.
async fn check_file(db: &Db, file: InterestFile) -> Result<PortResult<()>, ExitStatus> {
    let mut uow = db
        .begin()
        .await
        .map_err(|err| setup_failure("file open or close check could not begin", err))?;
    let answered = table_answers(&mut uow.store(), file).await;
    if answered.is_ok() {
        uow.rollback()
            .await
            .map_err(|err| setup_failure("file open or close check could not end", err))?;
    }
    // A failed check wrote nothing; dropping its unit of work rolls it back.
    Ok(answered)
}

/// CBACT04C `OPEN` and `CLOSE` of the four KSDS files — P-CBACT04C-177 to P-CBACT04C-180,
/// P-CBACT04C-183 to P-CBACT04C-186: whether the table of `file` answers one statement of
/// `store`: for `TCATBALF` and `XREFFILE` a sequential read of no records, for `DISCGRP` and
/// `ACCTFILE` a keyed read of
/// [`CHECK_GROUP`] or [`CHECK_ACCOUNT`], where a record and status 23 both count as an answer.
/// The outcome is discarded.
///
/// # Errors
///
/// The port failure of the statement, other than [`PortError::NotFound`].
async fn table_answers<S>(store: &mut S, file: InterestFile) -> PortResult<()>
where
    S: TranCatBalanceStore + XrefReader + DisclosureGroupReader + AccountReader,
{
    let answered = match file {
        InterestFile::Tcatbal => store.tcatbals_after(None, 0).await.map(|_| ()),
        InterestFile::Xref => store.xrefs_after(None, 0).await.map(|_| ()),
        InterestFile::DisclosureGroup => store.read_disclosure_group(CHECK_GROUP).await.map(|_| ()),
        InterestFile::Account => store.read_account(CHECK_ACCOUNT).await.map(|_| ()),
    };
    match answered {
        Ok(()) | Err(PortError::NotFound) => Ok(()),
        Err(cause) => Err(cause),
    }
}

/// CBACT04C 0400-TRANFILE-OPEN [app/cbl/CBACT04C.cbl:L307-323] — P-CBACT04C-181:
/// `OPEN OUTPUT TRANSACT-FILE`, the new generation `SYSTRAN(+1)`, `RECFM=F,LRECL=350`
/// [app/jcl/INTCALC.jcl:L37-41], allocated on the blocking pool. Nothing has been read or
/// written yet.
///
/// # Errors
///
/// The 0400-TRANFILE-OPEN abend when the generation cannot be allocated; the allocation error
/// is logged.
async fn open_systran(ctx: &JobContext) -> Result<PendingGeneration, ExitStatus> {
    ctx.generations
        .begin_next_async(Dataset::SysTran)
        .await
        .map_err(|err| {
            tracing::error!(error = %err, "SYSTRAN(+1) allocation failed");
            systran_abend(ERROR_OPENING_TRANSACTION_FILE)
        })
}

/// CBACT04C 1000-TCATBALF-GET-NEXT [app/cbl/CBACT04C.cbl:L325-348] — P-CBACT04C-182: the next
/// records of `TCATBALF` in key order after `after`, from the first when `after` is `None`, in
/// a read-only unit of work. Fewer than [`PAGE_SIZE`] records is the end of the file (status
/// 10).
///
/// # Errors
///
/// - The 1000-TCATBALF-GET-NEXT abend when the read fails.
/// - A setup failure when the unit of work cannot begin or end.
async fn read_page(
    db: &Db,
    after: Option<TranCatBalanceKey>,
) -> Result<Vec<TranCatBalance>, ExitStatus> {
    let mut uow = db
        .begin()
        .await
        .map_err(|err| setup_failure("category balance read could not begin", err))?;
    match uow.store().tcatbals_after(after, PAGE_SIZE).await {
        Ok(records) => {
            uow.rollback()
                .await
                .map_err(|err| setup_failure("category balance read could not end", err))?;
            Ok(records)
        }
        // The unit of work wrote nothing; dropping it rolls it back.
        Err(cause) => Err(abend_status(&InterestAbend::TcatbalRead { cause })),
    }
}

/// The `DISCGRP` outcomes a run has read, by the key read: the record, or `None` for a key that
/// ended with status 23.
type DisclosureGroups = HashMap<DisclosureGroupKey, Option<DisclosureGroup>>;

/// The store the rule uses for one balance record: `store`, bound to the record's unit of work,
/// for every file, with the `DISCGRP` reads answered from the run's `groups` once a key is read.
///
/// `DISCGRP` does not change during the run (see the module documentation), so each key's
/// record or status 23 is read from the store once per run instead of once or twice per balance
/// record. The account and cross-reference accesses pass straight to `store`, so their locks,
/// savepoints and outcomes are those of the unit of work.
struct RecordStore<'g, S> {
    /// The record's unit of work.
    store: S,
    /// The `DISCGRP` outcomes of the run.
    groups: &'g mut DisclosureGroups,
}

/// CBACT04C 1100-GET-ACCT-DATA and the locked re-read of 1050-UPDATE-ACCOUNT — P-CBACT04C-002,
/// P-CBACT04C-010: the reads of the record's unit of work.
impl<S: AccountReader + Send> AccountReader for RecordStore<'_, S> {
    async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
        self.store.read_account(id).await
    }

    async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
        self.store.read_account_for_update(id).await
    }
}

/// CBACT04C 1050-UPDATE-ACCOUNT — P-CBACT04C-010: the rewrite of the record's unit of work,
/// under its own savepoint.
impl<S: AccountWriter + Send> AccountWriter for RecordStore<'_, S> {
    async fn rewrite_account(&mut self, account: Account) -> PortResult<()> {
        self.store.rewrite_account(account).await
    }
}

/// CBACT04C 1110-GET-XREF-DATA — P-CBACT04C-002: the reads of the record's unit of work.
impl<S: XrefReader + Send> XrefReader for RecordStore<'_, S> {
    async fn read_xref_by_card(&mut self, card: CardNumber) -> PortResult<CardXref> {
        self.store.read_xref_by_card(card).await
    }

    async fn read_xref_by_account(&mut self, id: AccountId) -> PortResult<CardXref> {
        self.store.read_xref_by_account(id).await
    }

    async fn xrefs_after(
        &mut self,
        after: Option<CardNumber>,
        limit: u16,
    ) -> PortResult<Vec<CardXref>> {
        self.store.xrefs_after(after, limit).await
    }
}

/// CBACT04C 1200-GET-INTEREST-RATE and 1200-A-GET-DEFAULT-INT-RATE — P-CBACT04C-004,
/// P-CBACT04C-005, P-CBACT04C-193: the random `READ` of `DISCGRP` by `key`.
///
/// A key the run has read returns its kept outcome: the record, or [`PortError::NotFound`] for
/// status 23, on which the rule reads `DEFAULT` and counts the fallback as before. A key not yet
/// read goes to the store, and a record or status 23 is kept. Any other failure is returned as
/// the store gave it and not kept, so the rule abends with it and a later read of the key would
/// go to the store again.
impl<S: DisclosureGroupReader + Send> DisclosureGroupReader for RecordStore<'_, S> {
    async fn read_disclosure_group(
        &mut self,
        key: DisclosureGroupKey,
    ) -> PortResult<DisclosureGroup> {
        if let Some(kept) = self.groups.get(&key) {
            return kept.clone().ok_or(PortError::NotFound);
        }
        let outcome = self.store.read_disclosure_group(key.clone()).await;
        let kept = match &outcome {
            Ok(group) => Some(group.clone()),
            Err(PortError::NotFound) => None,
            Err(_) => return outcome,
        };
        self.groups.insert(key, kept);
        outcome
    }
}

/// One `batch intcalc` run: the CBACT04C state the rule keeps, the open `SYSTRAN(+1)` and the
/// counts the job reports.
struct IntcalcJob<'a> {
    /// The settings, the processing clock and the generation catalog of the run.
    ctx: &'a JobContext,
    /// The pool the units of work begin on.
    db: &'a Db,
    /// The CBACT04C working storage across balance records.
    interest: InterestRun,
    /// The `DISCGRP` outcomes the run has read, shared by every record's [`RecordStore`].
    groups: DisclosureGroups,
    /// `TRANSACT DD DSN=SYSTRAN(+1)`, open for output.
    systran: PendingGeneration,
    /// `WS-RECORD-COUNT`: the balance records read.
    read: u64,
    /// The accounts `1050-UPDATE-ACCOUNT` rewrote in a committed unit of work: the rule's
    /// count of rewrites as the last committed record left it, so a rewrite whose unit of work
    /// failed to commit is not counted.
    posted: u64,
    /// The interest records written to `SYSTRAN(+1)`.
    written: u64,
}

impl IntcalcJob<'_> {
    /// CBACT04C `PERFORM UNTIL END-OF-FILE = 'Y'` [app/cbl/CBACT04C.cbl:L188-222] —
    /// P-CBACT04C-002, P-CBACT04C-182: every balance record of `TCATBALF`, in key order, then
    /// the end of the file.
    ///
    /// # Errors
    ///
    /// The status of the first abend or setup failure; the records before it stay committed.
    async fn read_file(&mut self) -> Result<(), ExitStatus> {
        let mut after = None;
        loop {
            let page = read_page(self.db, after).await?;
            for record in &page {
                self.process(record).await?;
            }
            match page.last() {
                Some(last) if page.len() == usize::from(PAGE_SIZE) => {
                    after = Some(last.key.clone());
                }
                _ => return Ok(()),
            }
        }
    }

    /// CBACT04C loop body [app/cbl/CBACT04C.cbl:L191-217] — P-CBACT04C-002, P-CBACT04C-006,
    /// P-CBACT04C-007, P-CBACT04C-010, P-CBACT04C-182: `ADD 1 TO WS-RECORD-COUNT`,
    /// `DISPLAY TRAN-CAT-BAL-RECORD`, then the rule on one balance record in its own unit of
    /// work, through a [`RecordStore`] that answers the run's repeated `DISCGRP` reads, and the
    /// interest record it returns written to `SYSTRAN(+1)`. A zero rate returns none and writes
    /// nothing.
    ///
    /// # Errors
    ///
    /// - The abend of the failing paragraph, after the unit of work is committed: the account
    ///   rewrite the rule made before the failure persists (P-CBACT04C-010).
    /// - A setup failure when the unit of work cannot begin or commit.
    ///
    /// Each committed unit of work brings [`IntcalcJob::posted`] up to the rule's count of
    /// account rewrites.
    async fn process(&mut self, record: &TranCatBalance) -> Result<(), ExitStatus> {
        self.read += 1;
        display_record(record);
        let mut uow = self
            .db
            .begin()
            .await
            .map_err(|err| setup_failure("balance record unit of work could not begin", err))?;
        let processed = self
            .interest
            .process_record(
                &mut RecordStore {
                    store: uow.store(),
                    groups: &mut self.groups,
                },
                record,
                &self.ctx.clock,
            )
            .await;
        let written = match processed {
            Ok(Some(tran)) => self.write_systran(tran).await,
            Ok(None) => Ok(()),
            Err(abend) => Err(abend),
        };
        match written {
            Ok(()) => {
                uow.commit().await.map_err(|err| {
                    setup_failure("balance record unit of work commit failed", err)
                })?;
                self.posted = self.accounts_updated();
                Ok(())
            }
            Err(abend) => Err(self.commit_then_abend(uow, &abend).await),
        }
    }

    /// CBACT04C 9999-ABEND-PROGRAM after a failed paragraph of the loop — P-CBACT04C-010,
    /// P-CBACT04C-188: commits the record's unit of work, whose account rewrite persists under
    /// `RECOVERY(NONE)` and is then counted in [`IntcalcJob::posted`], then reports `abend`.
    async fn commit_then_abend(&mut self, uow: UnitOfWork, abend: &InterestAbend) -> ExitStatus {
        match uow.commit().await {
            Ok(()) => self.posted = self.accounts_updated(),
            Err(err) => {
                tracing::error!(
                    error = %err,
                    "account rewrite before the abend could not be committed"
                );
            }
        }
        abend_status(abend)
    }

    /// The accounts `1050-UPDATE-ACCOUNT` has rewritten so far, committed or not: the
    /// `accounts_updated` that [`InterestRun::finish`] would report now. The rule keeps its
    /// counts to itself until the end of the file, so they are read from a copy of the run,
    /// which leaves the run itself going on.
    fn accounts_updated(&self) -> u64 {
        self.interest.clone().finish().accounts_updated
    }

    /// CBACT04C 1300-B-WRITE-TX `WRITE FD-TRANFILE-REC FROM TRAN-RECORD`
    /// [app/cbl/CBACT04C.cbl:L500-515] — P-CBACT04C-008, P-CBACT04C-009, P-CBACT04C-173: the
    /// interest record as a 350-byte `CVTRA05Y` record, its FILLER spaces (E-25), encoded and
    /// written on the blocking pool.
    ///
    /// # Errors
    ///
    /// The 1300-B-WRITE-TX abend, status 30, when the generation cannot be written; the write
    /// error is logged.
    async fn write_systran(&mut self, tran: NewTransaction) -> Result<(), InterestAbend> {
        let record = TransactionRecord::new(tran);
        let tran_id = record.transaction.tran_id;
        match self
            .systran
            .append_records_async::<TransactionLayout>(vec![record])
            .await
        {
            Ok(()) => {
                self.written += 1;
                Ok(())
            }
            Err(err) => {
                tracing::error!(
                    error = %err,
                    tran_id = %tran_id,
                    "SYSTRAN(+1) write failed"
                );
                Err(InterestAbend::SystranWrite {
                    status: NO_FURTHER_INFORMATION,
                })
            }
        }
    }

    /// Sets `read` and `posted` in the job's counts and records `written` on the job span.
    /// `rejected` stays 0: CBACT04C has no reject file and turns no balance record away.
    fn record_counts(&self) {
        let counts = &self.ctx.counts;
        counts.set_read(self.read);
        counts.set_posted(self.posted);
        Span::current().record("written", span_count(self.written));
    }

    /// CBACT04C end of file and closes [app/cbl/CBACT04C.cbl:L188-230] — P-CBACT04C-003,
    /// P-CBACT04C-174, P-CBACT04C-183 to P-CBACT04C-187, E-14: the run ends with the last
    /// account not updated, the four KSDS files are closed ([`close_files`]), and
    /// `9400-TRANFILE-CLOSE` catalogs `SYSTRAN(+1)` on the blocking pool.
    ///
    /// The loop leaves before the `ELSE` that would perform `1050-UPDATE-ACCOUNT`, so the last
    /// account keeps its balance and cycle amounts while its interest records are written; the
    /// rule reports its total and nothing updates it. Each record's unit of work is already
    /// committed, so a failed close leaves every account rewrite in place.
    ///
    /// # Errors
    ///
    /// - The abend of the first KSDS close that fails, or a setup failure of its check;
    ///   `SYSTRAN(+1)` is then deleted uncataloged, as `DISP=(NEW,CATLG,DELETE)` deletes it.
    /// - The 9400-TRANFILE-CLOSE abend when the generation cannot be cataloged; the catalog
    ///   error is logged.
    ///
    /// The account updates stay committed in both cases.
    async fn end_of_file(self) -> ExitStatus {
        let summary = self.interest.finish();
        if let Some(unposted) = summary.unposted {
            tracing::info!(
                acct_id = %unposted.acct_id,
                total_interest = %unposted.total_interest,
                "last account of the category balance file not updated (E-14)"
            );
        }
        if let Err(status) = close_files(self.db).await {
            self.systran.discard_async().await;
            return status;
        }
        match self.systran.catalog_async().await {
            Ok(generation) => {
                tracing::info!(
                    generation = %generation,
                    records_read = summary.records_read,
                    interest_records = summary.interest_records,
                    accounts_updated = summary.accounts_updated,
                    default_group_reads = summary.default_group_reads,
                    "{END_OF_EXECUTION_OF_PROGRAM_CBACT04C}"
                );
                ExitStatus::Success
            }
            Err(err) => {
                tracing::error!(error = %err, "SYSTRAN(+1) catalog failed");
                systran_abend(ERROR_CLOSING_TRANSACTION_FILE)
            }
        }
    }
}

/// CBACT04C `DISPLAY TRAN-CAT-BAL-RECORD` [app/cbl/CBACT04C.cbl:L193] — P-CBACT04C-182: the
/// balance record just read, as an event with its `CVTRA01Y` fields.
fn display_record(record: &TranCatBalance) {
    tracing::info!(
        trancat_acct_id = %record.key.trancat_acct_id,
        trancat_type_cd = %record.key.trancat_type_cd,
        trancat_cd = %record.key.trancat_cd,
        tran_cat_bal = %record.tran_cat_bal,
        "TRAN-CAT-BAL-RECORD"
    );
}

/// CBACT04C 9999-ABEND-PROGRAM [app/cbl/CBACT04C.cbl:L628-632] — P-CBACT04C-188,
/// P-CBACT04C-189: the `DISPLAY` lines of `abend` and `CEE3ABD` with `ABCODE` 999, exit 16.
/// The port failure behind it is logged, never written to stderr.
fn abend_status(abend: &InterestAbend) -> ExitStatus {
    tracing::error!(
        display = %abend,
        cause = ?abend.source(),
        abcode = InterestAbend::ABCODE,
        "{ABENDING_PROGRAM}"
    );
    ExitStatus::abend_from(abend, Some(InterestAbend::ABCODE))
}

/// CBACT04C 9999-ABEND-PROGRAM after 0400-TRANFILE-OPEN or 9400-TRANFILE-CLOSE
/// [app/cbl/CBACT04C.cbl:L307-323] [app/cbl/CBACT04C.cbl:L595-611] — P-CBACT04C-181,
/// P-CBACT04C-187 to P-CBACT04C-189: `text`, 9910-DISPLAY-IO-STATUS with status 30 and
/// `ABENDING PROGRAM`, then `CEE3ABD` with `ABCODE` 999, exit 16.
fn systran_abend(text: &'static str) -> ExitStatus {
    let lines = vec![
        text.to_owned(),
        file_status_message(NO_FURTHER_INFORMATION),
        ABENDING_PROGRAM.to_owned(),
    ];
    tracing::error!(
        display = %lines.join("\n"),
        abcode = InterestAbend::ABCODE,
        "{ABENDING_PROGRAM}"
    );
    ExitStatus::Abend {
        lines,
        abcode: Some(InterestAbend::ABCODE),
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::entities::{DisclosureGroup, DisclosureGroupKey};
    use carddemo_domain::messages::{
        ABENDING_PROGRAM, ERROR_CLOSING_TRANSACTION_FILE, ERROR_OPENING_TRANSACTION_FILE,
        ERROR_WRITING_TRANSACTION_RECORD,
    };
    use carddemo_domain::numeric::{AccountGroupId, InterestRate, TranCategoryCode, TranTypeCode};
    use carddemo_domain::ports::{DisclosureGroupReader, PortError, PortResult};
    use carddemo_domain::rules::interest::InterestAbend;

    use super::{
        DisclosureGroups, NO_FURTHER_INFORMATION, RecordStore, abend_status, parm_date,
        systran_abend,
    };
    use crate::cli::IntcalcArgs;
    use crate::exit::{ABEND, ExitStatus};

    fn args(parm: &str) -> IntcalcArgs {
        IntcalcArgs {
            parm_date: parm.to_owned(),
        }
    }

    /// `DISCGRP` in memory, counting the reads that reach it; the first read fails with `failure`
    /// when one is given.
    struct CountedGroups {
        groups: Vec<DisclosureGroup>,
        failure: Option<PortError>,
        reads: usize,
    }

    impl DisclosureGroupReader for CountedGroups {
        async fn read_disclosure_group(
            &mut self,
            key: DisclosureGroupKey,
        ) -> PortResult<DisclosureGroup> {
            self.reads += 1;
            if let Some(cause) = self.failure.take() {
                return Err(cause);
            }
            self.groups
                .iter()
                .find(|group| group.key == key)
                .cloned()
                .ok_or(PortError::NotFound)
        }
    }

    #[tokio::test]
    async fn discgrp_reads_keep_records_and_status_23_for_the_run_but_not_failures() {
        // The DEFAULT 01/0001 rate of every fixture 01/0001 balance (P-CBACT04C-023) and the
        // blank-group key of the same balance, which ends with status 23 (E-06).
        let type_01 = TranTypeCode::move_from("01");
        let category_1 = TranCategoryCode::new(1).expect("a four-digit category");
        let default_01 = DisclosureGroup {
            key: DisclosureGroupKey::default_group(type_01, category_1),
            dis_int_rate: InterestRate::from_hundredths(1_500).expect("a rate within S9(04)V99"),
        };
        let blank_01 = DisclosureGroupKey {
            dis_acct_group_id: AccountGroupId::move_from(""),
            dis_tran_type_cd: type_01,
            dis_tran_cat_cd: category_1,
        };
        let unavailable = PortError::Unavailable("connection reset".to_owned());
        // (key, failure of the first store read, outcomes of two records' reads, store reads)
        let cases = [
            (
                default_01.key.clone(),
                None,
                [Ok(default_01.clone()), Ok(default_01.clone())],
                1,
            ),
            (
                blank_01,
                None,
                [Err(PortError::NotFound), Err(PortError::NotFound)],
                1,
            ),
            (
                default_01.key.clone(),
                Some(unavailable.clone()),
                [Err(unavailable), Ok(default_01.clone())],
                2,
            ),
        ];
        for (key, failure, expected, reads) in cases {
            let mut store = CountedGroups {
                groups: vec![default_01.clone()],
                failure,
                reads: 0,
            };
            let mut groups = DisclosureGroups::new();
            let mut outcomes = Vec::new();
            // One RecordStore per balance record, sharing the run's outcomes.
            for _ in 0..2 {
                let mut record_store = RecordStore {
                    store,
                    groups: &mut groups,
                };
                outcomes.push(record_store.read_disclosure_group(key.clone()).await);
                store = record_store.store;
            }
            assert_eq!(outcomes, expected, "{key:?}");
            assert_eq!(store.reads, reads, "{key:?}");
        }
    }

    #[test]
    fn parm_date_is_the_jcl_parm_as_x10() {
        // `PARM-DATE X(10)` copies the bytes it is given, a record delimiter included.
        for (parm, expected) in [
            ("2022071800", *b"2022071800"),
            ("2022\n71800", *b"2022\n71800"),
        ] {
            assert_eq!(
                parm_date(&args(parm)).map(|parm| *parm.as_bytes()),
                Ok(expected),
                "{parm:?}"
            );
        }
    }

    #[test]
    fn parm_date_of_another_length_is_a_setup_failure() {
        for parm in ["202207180", "20220718000"] {
            let status = parm_date(&args(parm)).err();
            assert!(
                matches!(status, Some(ExitStatus::SetupFailure(_))),
                "{parm}: {status:?}"
            );
        }
    }

    #[test]
    fn systran_failures_abend_999_with_status_30() {
        let cases = [
            (
                systran_abend(ERROR_OPENING_TRANSACTION_FILE),
                ERROR_OPENING_TRANSACTION_FILE,
            ),
            (
                systran_abend(ERROR_CLOSING_TRANSACTION_FILE),
                ERROR_CLOSING_TRANSACTION_FILE,
            ),
            (
                abend_status(&InterestAbend::SystranWrite {
                    status: NO_FURTHER_INFORMATION,
                }),
                ERROR_WRITING_TRANSACTION_RECORD,
            ),
        ];
        for (status, text) in cases {
            assert_eq!(status.code(), ABEND, "{text}");
            assert_eq!(
                status.render(),
                [
                    text,
                    "FILE STATUS IS: NNNN0030",
                    ABENDING_PROGRAM,
                    "ABEND 999"
                ],
                "{text}"
            );
        }
    }
}
