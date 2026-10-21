//! `intcalc`: batch job INTCALC, whose `STEP15` runs the monthly interest calculator `CBACT04C`
//! with `PARM='2022071800'` (`INTCALC.jcl:22`; AAP 0.3.3.2, 0.3.4.3). Every rule is
//! `domain::batch::cbact04c`'s; this module reads the four tables the program opens, drives
//! `Intcalc` over the category balances in key order and applies the effects it returns, after
//! the `CBACT04C-` rows of `modern/audit/batch/INTCALC.md` and, over the fixture corpus, the
//! `FIXINT-` rows of `modern/audit/fixtures.md`.
//!
//! # Flow
//!
//! 1. `--parm-date` arrives as the domain's typed `ParmDate`: the command line has already
//!    refused any value that is not 10 or more printable ASCII bytes, so a refused PARM reads no
//!    table and creates no file (CBACT04C-001).
//! 2. The opens (CBACT04C-002). `accounts`, `card_xref` and `disclosure_groups` are read whole
//!    into `Intcalc::new`, which serves the program's keyed reads from them, the
//!    cross-references grouped by account as the alternate index over `XREF-ACCT-ID` holds them
//!    (CBACT04C-004 to CBACT04C-011). The next `SYSTRAN` generation is then created under
//!    `Config::systran_dir`, before any category balance is read, as `0400-TRANFILE-OPEN`
//!    creates `SYSTRAN(+1)` (CBACT04C-024).
//! 3. Every `tran_cat_balances` row, in the KSDS key order its repository returns
//!    (CBACT04C-003), goes to `Intcalc::process`, and its effects are applied in the order
//!    returned: an account update rewrites that `accounts` row (CBACT04C-026), and a system
//!    transaction appends its 350 bytes and LF to the generation file (CBACT04C-024). The
//!    interest, the running total and the balance store are the domain's (`monthly_interest`
//!    and `store`, ARITH-001 to ARITH-017); this module does no arithmetic on money.
//! 4. At the end of the file `Intcalc::finish` runs and only its counters are kept. Its
//!    unapplied total is deliberately never persisted: the loop's
//!    `ELSE PERFORM 1050-UPDATE-ACCOUNT` is never reached, so the last account in key order
//!    keeps its balance and cycle fields while its SYSTRAN records are still written (E08;
//!    CBACT04C-027, ARITH-017, FIXINT-903).
//! 5. The generation file is flushed, synced and kept. Its path is the one path returned, and
//!    the run returns code 0, as `RETURN-CODE` is never set (CBACT04C-030).
//!
//! # Every rewrite commits when it is made
//!
//! A VSAM `REWRITE` has no unit of work to back out, so each account update is its own
//! autocommitted statement on the pool, never part of a job transaction, and the updates made
//! before an abend stand (CBACT04C-026, CBACT04C-038). When the first record of a new account
//! abends a lookup, the previous account's `1050-UPDATE-ACCOUNT` has already run, as it comes
//! before `1100-GET-ACCT-DATA`; the domain returns that update with its error, and the job
//! persists it before it ends (CBACT04C-005, CBACT04C-007, CBACT04C-008, CBACT04C-011). Should
//! that rewrite fail, its failure is the abend, as it is in the program.
//!
//! # Abends
//!
//! - [`JobError::Intcalc`]: an account, cross-reference or `DEFAULT` disclosure lookup the
//!   domain refuses (CBACT04C-005, CBACT04C-007, CBACT04C-008, CBACT04C-011).
//! - [`JobError::Abend`] with `ERROR RE-WRITING ACCOUNT FILE`: a rewrite that finds no row
//!   (CBACT04C-032).
//! - [`JobError::Persistence`]: a read or a rewrite that fails as a statement (CBACT04C-032,
//!   CBACT04C-033), and any query against a database whose schema has not been applied.
//! - [`JobError::Io`]: the generation cannot be allocated, created, written or synced
//!   (CBACT04C-031, CBACT04C-034, CBACT04C-035).
//!
//! `9999-ABEND-PROGRAM` sets no return code (CBACT04C-038), so no abend carries one. After any
//! error the unfinished generation file is removed by the guard of `OutputFile`, as
//! `DISP=(NEW,CATLG,DELETE)` deletes it at abend: no path is printed and the next run takes the
//! same generation number (CBACT04C-038).
//!
//! # SYSTRAN stays a file
//!
//! The SYSTRAN records go to the generation file only and are never inserted into
//! `transactions`: merging them into the transaction file is COMBTRAN's work, which is out of
//! scope (AAP 0.2.3). `tran_cat_balances`, `card_xref` and `disclosure_groups` are only read, so
//! `accounts` is the one table that changes (CBACT04C-029).
//!
//! # The `DEFAULT` group (E07)
//!
//! Every fixture account leaves `ACCT-GROUP-ID` blank, so every disclosure lookup misses and the
//! domain reads the `DEFAULT` group with the same type and category (CBACT04C-010,
//! FIXINT-902). This module never substitutes a group of its own.
//!
//! # Job log and cardholder data
//!
//! The program's `DISPLAY` lines, the record shown for every category balance read included
//! (CBACT04C-003), are not written: standard output carries only the path `main` prints from
//! [`JobOutcome::written`] (AAP 0.3.4.3), and the job emits no tracing of its own, as `tranrept`
//! does not. Each SYSTRAN record carries the full `XREF-CARD-NUM` (CBACT04C-021) in the local
//! generation file only, which is batch output and not egress; the job's errors are the fixed
//! texts of [`JobError`] and its counts are plain numbers, so nothing it emits carries a card
//! number (Cardholder Data Redaction).
//!
//! # Rules
//!
//! - Single Schema Executor: no schema work here; against a database whose schema has not been
//!   applied the job fails at its first read with [`JobError::Persistence`].
//! - R4: every statement is a repository call of `persistence`.
//! - Local-Only Runtime: the job writes `PostgreSQL` and the directory `Config::systran_dir`
//!   names, nothing else.
//! - Typed Dates and Timestamps: `ParmDate` is passed on as it was parsed, never rendered as
//!   text here, and both SYSTRAN timestamps come typed from the domain and the injected `Clock`
//!   (CBACT04C-022).

use copybook::FixedRecord;
use domain::batch::cbact04c::{Intcalc, IntcalcEffect, REWRITE_ACCOUNT_ERROR};
use domain::clock::Clock;
use domain::records::{AccountRecord, TranCatBalanceRecord};
use persistence::{
    AccountRepository, CardXrefRepository, DisclosureGroupRepository, TranCatBalanceRepository,
};

use super::{JobContext, JobError, JobOutcome, OutputFile, count};
use crate::cli::IntcalcArgs;

/// The generation base of the SYSTRAN file, `AWS.M2.CARDDEMO.SYSTRAN(+1)` of the `TRANSACT` DD
/// (`INTCALC.jcl:37-41`; CBACT04C-024).
const SYSTRAN_BASE: &str = "SYSTRAN";

/// Runs INTCALC with the PARM of `args`: rewrites every account in key order but the last,
/// writes the next `SYSTRAN` generation under `Config::systran_dir` and returns its path,
/// return code 0, and the counts `records` (category balances read), `transactions` (SYSTRAN
/// records written) and `accounts` (accounts rewritten).
///
/// # Errors
///
/// - [`JobError::Persistence`] when a table cannot be read or a rewrite fails as a statement
///   (CBACT04C-032, CBACT04C-033).
/// - [`JobError::Intcalc`] for the lookup abends of `CBACT04C` (CBACT04C-005, CBACT04C-007,
///   CBACT04C-008, CBACT04C-011), after the account rewrite that preceded the failed lookup is
///   persisted.
/// - [`JobError::Abend`] with `ERROR RE-WRITING ACCOUNT FILE` when a rewrite finds no row
///   (CBACT04C-032).
/// - [`JobError::Io`] when the generation file cannot be allocated, created, written or synced
///   (CBACT04C-031, CBACT04C-034, CBACT04C-035).
///
/// Every rewrite committed before an error stands, and the generation file is removed
/// (CBACT04C-038).
pub(crate) async fn run(ctx: &JobContext<'_>, args: &IntcalcArgs) -> Result<JobOutcome, JobError> {
    // The store is a handle on the shared pool, cloned for the `&mut self` repository calls.
    // Each statement on it commits on its own, as each VSAM rewrite stands once made
    // (CBACT04C-026), so the job opens no transaction.
    let mut store = ctx.store.clone();
    let accounts = AccountRepository::find_all(&mut store).await?;
    let xrefs = CardXrefRepository::find_all(&mut store).await?;
    let groups = DisclosureGroupRepository::find_all(&mut store).await?;
    let mut intcalc = Intcalc::new(args.parm_date.clone(), accounts, xrefs, groups);
    let mut systran = OutputFile::next_generation(&ctx.config.systran_dir(), SYSTRAN_BASE)?;
    let balances = TranCatBalanceRepository::find_all(&mut store).await?;
    let rewritten =
        post_interest(&mut intcalc, &balances, ctx.clock, &mut store, &mut systran).await?;
    // E08: the summary's unapplied last account and total are discarded, never rewritten;
    // only its counters are reported (CBACT04C-027).
    let summary = intcalc.finish();
    let path = systran.finish()?;
    Ok(JobOutcome {
        written: vec![path],
        return_code: 0,
        counts: vec![
            ("records", summary.records_read()),
            ("transactions", summary.transactions_written()),
            ("accounts", count(rewritten)),
        ],
    })
}

/// The main loop of `CBACT04C` over `balances`, in order (`CBACT04C.cbl:188-222`): each record
/// goes to `intcalc`, and its effects are applied as returned, an account update through
/// `accounts` and a SYSTRAN record appended to `systran` (CBACT04C-024, CBACT04C-026). Returns
/// the number of accounts rewritten.
///
/// # Errors
///
/// The first failure, as [`run`] reports it. A lookup abend carrying the previous account's
/// update has that update rewritten first (CBACT04C-005, CBACT04C-038).
async fn post_interest<A: AccountRepository>(
    intcalc: &mut Intcalc,
    balances: &[TranCatBalanceRecord],
    clock: &dyn Clock,
    accounts: &mut A,
    systran: &mut OutputFile,
) -> Result<usize, JobError> {
    let mut rewritten = 0;
    for balance in balances {
        let effects = match intcalc.process(balance, clock) {
            Ok(effects) => effects,
            Err(error) => {
                if let Some(account) = error.rewritten_before_abend() {
                    rewrite(accounts, account).await?;
                }
                return Err(JobError::Intcalc(error));
            }
        };
        for effect in effects {
            match effect {
                IntcalcEffect::AccountUpdate(account) => {
                    rewrite(accounts, &account).await?;
                    rewritten += 1;
                }
                IntcalcEffect::SystemTransaction(record) => {
                    systran.write_record(&record.encode())?;
                }
            }
        }
    }
    Ok(rewritten)
}

/// `REWRITE FD-ACCTFILE-REC FROM ACCOUNT-RECORD` of `1050-UPDATE-ACCOUNT`, committed as it is
/// made (`CBACT04C.cbl:356-369`; CBACT04C-026).
///
/// # Errors
///
/// [`JobError::Abend`] with the program's `ERROR RE-WRITING ACCOUNT FILE` when no row has the
/// account's key, the status `23` of a `REWRITE` that finds no record, and
/// [`JobError::Persistence`] when the statement fails (CBACT04C-032).
async fn rewrite<A: AccountRepository>(
    accounts: &mut A,
    account: &AccountRecord,
) -> Result<(), JobError> {
    if AccountRepository::update(accounts, account).await? {
        Ok(())
    } else {
        Err(JobError::Abend {
            code: None,
            reason: REWRITE_ACCOUNT_ERROR,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::collections::btree_map::Entry;
    use std::error::Error;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use chrono::{NaiveDate, Timelike};
    use copybook::{FixedRecord, PicDate, PicU, PicX, TsForm};
    use domain::batch::cbact04c::{
        DEFAULT_GROUP, Intcalc, IntcalcError, ParmDate, REWRITE_ACCOUNT_ERROR,
    };
    use domain::clock::FixedClock;
    use domain::money::{Decimal, Money, Rate};
    use domain::records::{
        AccountRecord, CardXrefRecord, DisclosureGroupRecord, TranCatBalanceRecord,
        TransactionRecord,
    };
    use domain::sensitive::CardNumber;
    use persistence::{AccountCustomerChain, AccountRepository, ChainLink, PersistenceError};

    use super::post_interest;
    use crate::jobs::{JobError, OutputFile};

    type TestResult = Result<(), Box<dyn Error>>;

    /// Width of a `CVTRA05Y` SYSTRAN record (CBACT04C-024).
    const SYSTRAN_WIDTH: usize = 350;

    /// The shape of both SYSTRAN timestamps: layout `D`, `YYYY-MM-DD-HH.MM.SS.ffffff`, with
    /// the fraction cut to hundredths, `9` standing for any ASCII digit (CBACT04C-022).
    const DB2_HUNDREDTHS: &[u8; 26] = b"9999-99-99-99.99.99.990000";

    /// Nanoseconds in one hundredth of a second, the finest unit of a SYSTRAN timestamp
    /// (CBACT04C-022).
    const NANOS_PER_HUNDREDTH: u32 = 10_000_000;

    /// Tells apart the scratch directories of the tests of one process.
    static SCRATCH_SEQUENCE: AtomicU32 = AtomicU32::new(0);

    /// A directory of its own under the system temporary directory, removed with its content
    /// when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Result<Self, Box<dyn Error>> {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
            let sequence = SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "carddemo-batch-intcalc-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// An in-memory `accounts` table. `update` rewrites a row it holds and answers `false` for
    /// a key it does not, as `PgStore` does; with `failing` set every update fails as a
    /// statement. `rewrites` lists the keys rewritten, in order.
    struct Ledger {
        rows: BTreeMap<PicU<11>, AccountRecord>,
        rewrites: Vec<PicU<11>>,
        failing: bool,
    }

    impl Ledger {
        fn holding(accounts: &[AccountRecord], failing: bool) -> Self {
            Self {
                rows: accounts
                    .iter()
                    .map(|account| (account.id, account.clone()))
                    .collect(),
                rewrites: Vec::new(),
                failing,
            }
        }
    }

    impl AccountRepository for Ledger {
        fn find_with_customer(
            &mut self,
            _acct_id: PicU<11>,
        ) -> impl Future<Output = AccountCustomerChain> + Send {
            std::future::ready(AccountCustomerChain {
                xref: ChainLink::NotFound,
                account: ChainLink::NotReached,
                customer: ChainLink::NotReached,
            })
        }

        fn find_by_id(
            &mut self,
            id: PicU<11>,
        ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
            std::future::ready(Ok(self.rows.get(&id).cloned()))
        }

        fn lock_by_id(
            &mut self,
            id: PicU<11>,
        ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
            std::future::ready(Ok(self.rows.get(&id).cloned()))
        }

        fn find_all(
            &mut self,
        ) -> impl Future<Output = Result<Vec<AccountRecord>, PersistenceError>> + Send {
            std::future::ready(Ok(self.rows.values().cloned().collect()))
        }

        fn insert(
            &mut self,
            record: &AccountRecord,
        ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
            let outcome = match self.rows.entry(record.id) {
                Entry::Vacant(row) => {
                    row.insert(record.clone());
                    Ok(())
                }
                Entry::Occupied(_) => Err(PersistenceError::Duplicate {
                    constraint: Some("accounts_pkey".to_owned()),
                }),
            };
            std::future::ready(outcome)
        }

        fn update(
            &mut self,
            record: &AccountRecord,
        ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
            let outcome = if self.failing {
                Err(PersistenceError::Unavailable)
            } else if let Some(row) = self.rows.get_mut(&record.id) {
                *row = record.clone();
                self.rewrites.push(record.id);
                Ok(true)
            } else {
                Ok(false)
            };
            std::future::ready(outcome)
        }

        fn replace_all(
            &mut self,
            records: &[AccountRecord],
        ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
            self.rows = records
                .iter()
                .map(|record| (record.id, record.clone()))
                .collect();
            std::future::ready(Ok(crate::jobs::count(self.rows.len())))
        }
    }

    fn money<const N: u32>(cents: i64) -> Result<Money<N>, Box<dyn Error>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    /// Account `id` with balance `bal_cents`, non-zero cycle fields and the blank group of every
    /// fixture account (E07).
    fn account(id: u64, bal_cents: i64) -> Result<AccountRecord, Box<dyn Error>> {
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
            group_id: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// A synthetic card number for account `id`, in no fixture file.
    fn card(id: u8) -> CardNumber {
        let mut digits = *b"4000000000000000";
        if let Some(last) = digits.last_mut() {
            *last = b'0'.saturating_add(id);
        }
        CardNumber::new(PicX::from_array(digits))
    }

    fn xref(id: u8) -> Result<CardXrefRecord, Box<dyn Error>> {
        Ok(CardXrefRecord {
            card_num: card(id),
            cust_id: PicU::new(u64::from(id))?,
            acct_id: PicU::new(u64::from(id))?,
            filler: PicX::spaces(),
        })
    }

    /// Category balance (`acct_id`, `01`, `0001`) of `bal_cents`.
    fn balance(acct_id: u64, bal_cents: i64) -> Result<TranCatBalanceRecord, Box<dyn Error>> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(acct_id)?,
            type_cd: PicX::from_array(*b"01"),
            cd: PicU::new(1)?,
            bal: money(bal_cents)?,
            filler: PicX::spaces(),
        })
    }

    /// A run over accounts 1 and 2 and their cards, with only the `DEFAULT` rate of 15.00 for
    /// type `01`, category `0001`, as `discgrp.txt:18` holds it (CBACT04C-010).
    fn intcalc() -> Result<Intcalc, Box<dyn Error>> {
        let default_rate = DisclosureGroupRecord {
            acct_group_id: DEFAULT_GROUP,
            tran_type_cd: PicX::from_array(*b"01"),
            tran_cat_cd: PicU::new(1)?,
            int_rate: Rate::new(Decimal::new(1_500, 2))?,
            filler: PicX::spaces(),
        };
        Ok(Intcalc::new(
            ParmDate::DEFAULT,
            [account(1, 10_000)?, account(2, 20_000)?],
            [xref(1)?, xref(2)?],
            [default_rate],
        ))
    }

    fn clock() -> Result<FixedClock, Box<dyn Error>> {
        let at = NaiveDate::from_ymd_opt(2022, 7, 18)
            .and_then(|day| day.and_hms_opt(0, 0, 0))
            .ok_or("2022-07-18 00:00:00 is a valid instant")?;
        Ok(FixedClock::new(at))
    }

    /// The raw SYSTRAN records of `bytes`, the content of a finished generation, after checking
    /// that each is 350 bytes followed by LF (CBACT04C-024).
    fn systran_records(bytes: &[u8]) -> Result<Vec<&[u8]>, Box<dyn Error>> {
        let lines = bytes.chunks_exact(SYSTRAN_WIDTH + 1);
        if !lines.remainder().is_empty() {
            return Err("the generation ends in a partial record".into());
        }
        lines
            .map(|line| match line.split_at(SYSTRAN_WIDTH) {
                (raw, b"\n") => Ok(raw),
                _ => Err("a SYSTRAN record is not followed by LF".into()),
            })
            .collect()
    }

    /// Whether `text` has the shape of [`DB2_HUNDREDTHS`]. Time fields are compared by format
    /// only, never by the clock's value (AAP 0.5.6).
    fn is_db2_hundredths(text: &[u8]) -> bool {
        text.len() == DB2_HUNDREDTHS.len()
            && text
                .iter()
                .zip(DB2_HUNDREDTHS)
                .all(|(&byte, &shape)| match shape {
                    b'9' => byte.is_ascii_digit(),
                    _ => byte == shape,
                })
    }

    /// Account 1 is rewritten at the break of account 2 with its interest added and both cycle
    /// fields zeroed; account 2, the last in key order, keeps every field (E08), while the
    /// records of both are written in key order with consecutive suffixes.
    // audit: CBACT04C-024
    // audit: CBACT04C-026
    // audit: CBACT04C-027
    // audit: FIXINT-903
    #[tokio::test]
    async fn every_account_but_the_last_is_rewritten() -> TestResult {
        let scratch = Scratch::new()?;
        let before = [account(1, 10_000)?, account(2, 20_000)?];
        let mut ledger = Ledger::holding(&before, false);
        let mut run = intcalc()?;
        let mut systran = OutputFile::next_generation(scratch.path(), "SYSTRAN")?;
        let balances = [balance(1, 120_000)?, balance(2, 240_000)?];
        let rewritten =
            post_interest(&mut run, &balances, &clock()?, &mut ledger, &mut systran).await?;
        let summary = run.finish();
        let path = systran.finish()?;

        assert_eq!(rewritten, 1);
        assert_eq!(ledger.rewrites, [PicU::new(1)?]);
        let first = ledger.rows.get(&PicU::new(1)?).ok_or("account 1 is held")?;
        // 100.00 and the 15.00 of (1200.00 * 15.00) / 1200 (ARITH-001, ARITH-013).
        assert_eq!(first.curr_bal, money::<10>(11_500)?);
        assert_eq!(first.curr_cyc_credit, Money::<10>::ZERO);
        assert_eq!(first.curr_cyc_debit, Money::<10>::ZERO);
        assert_eq!(ledger.rows.get(&PicU::new(2)?), before.get(1));
        assert_eq!(summary.records_read(), 2);
        assert_eq!(summary.transactions_written(), 2);
        assert_eq!(summary.unapplied_last_account(), Some(PicU::new(2)?));

        assert_eq!(path, scratch.path().join("SYSTRAN.G0001"));
        let bytes = fs::read(&path)?;
        let records = systran_records(&bytes)?;
        let expected = [
            ("2022071800000001", 1, 1_500),
            ("2022071800000002", 2, 3_000),
        ];
        assert_eq!(records.len(), expected.len());
        for (raw, (id, owner, cents)) in records.into_iter().zip(expected) {
            let record = TransactionRecord::decode(raw)?;
            assert_eq!(record.id, PicX::from_text(id)?, "{id}");
            assert_eq!(record.amt, money::<9>(cents)?, "{id}");
            assert!(record.card_num == card(owner), "{id}: card number");
            // Time fields are compared by format only (AAP 0.5.6): one clock reading, in layout
            // `D` and cut to hundredths, fills both fields (CBACT04C-022).
            let orig_text = raw.get(278..304).ok_or("TRAN-ORIG-TS is in the record")?;
            let proc_text = raw.get(304..330).ok_or("TRAN-PROC-TS is in the record")?;
            assert!(is_db2_hundredths(orig_text), "{id}: TRAN-ORIG-TS");
            assert!(is_db2_hundredths(proc_text), "{id}: TRAN-PROC-TS");
            assert_eq!(orig_text, proc_text, "{id}: one reading for both fields");
            assert_eq!(
                record.orig_ts, record.proc_ts,
                "{id}: one reading for both fields"
            );
            let stamp = record.orig_ts.value().ok_or("TRAN-ORIG-TS is not blank")?;
            assert_eq!(stamp.form(), TsForm::Db2, "{id}: layout D");
            assert_eq!(
                stamp.value().nanosecond() % NANOS_PER_HUNDREDTH,
                0,
                "{id}: hundredths"
            );
        }
        Ok(())
    }

    /// How an abending run must end.
    #[derive(Clone, Copy, Debug)]
    enum Ends {
        /// `ERROR RE-WRITING ACCOUNT FILE`: the rewrite found no row.
        RewriteNotFound,
        /// The rewrite failed as a statement.
        StoreFailure,
        /// The account lookup at a break found no record.
        AccountNotFound,
    }

    /// One abending run: the accounts the table holds, whether every update fails, the accounts
    /// of the two category balances, the expected end and the rewrites that stand.
    struct Case {
        held: &'static [u64],
        failing: bool,
        keys: [u64; 2],
        ends: Ends,
        kept: &'static [u64],
    }

    /// Each abend ends the loop with its error, keeps the rewrites committed before it, the one
    /// made at the failing break included, and leaves no generation file once the output is
    /// dropped. A rewrite that precedes a failed lookup and fails itself is the abend.
    // audit: CBACT04C-005
    // audit: CBACT04C-032
    // audit: CBACT04C-038
    #[tokio::test]
    async fn abends_keep_the_rewrites_made_before_them() -> TestResult {
        let cases = [
            Case {
                held: &[2],
                failing: false,
                keys: [1, 2],
                ends: Ends::RewriteNotFound,
                kept: &[],
            },
            Case {
                held: &[1, 2],
                failing: true,
                keys: [1, 2],
                ends: Ends::StoreFailure,
                kept: &[],
            },
            Case {
                held: &[1, 2],
                failing: false,
                keys: [1, 3],
                ends: Ends::AccountNotFound,
                kept: &[1],
            },
            Case {
                held: &[2],
                failing: false,
                keys: [1, 3],
                ends: Ends::RewriteNotFound,
                kept: &[],
            },
        ];
        for Case {
            held,
            failing,
            keys,
            ends,
            kept,
        } in cases
        {
            let scratch = Scratch::new()?;
            let accounts = held
                .iter()
                .map(|&id| account(id, 10_000))
                .collect::<Result<Vec<_>, _>>()?;
            let mut ledger = Ledger::holding(&accounts, failing);
            let mut run = intcalc()?;
            let mut systran = OutputFile::next_generation(scratch.path(), "SYSTRAN")?;
            let balances = keys
                .into_iter()
                .map(|id| balance(id, 120_000))
                .collect::<Result<Vec<_>, _>>()?;
            let error = post_interest(&mut run, &balances, &clock()?, &mut ledger, &mut systran)
                .await
                .err()
                .ok_or("the run abends")?;
            let matched = match ends {
                Ends::RewriteNotFound => matches!(
                    &error,
                    JobError::Abend { code: None, reason } if *reason == REWRITE_ACCOUNT_ERROR
                ),
                Ends::StoreFailure => {
                    matches!(&error, JobError::Persistence(PersistenceError::Unavailable))
                }
                Ends::AccountNotFound => matches!(
                    &error,
                    JobError::Intcalc(IntcalcError::AccountNotFound { .. })
                ),
            };
            assert!(matched, "{ends:?} ended with {error}");
            let kept = kept
                .iter()
                .map(|&id| PicU::new(id))
                .collect::<Result<Vec<PicU<11>>, _>>()?;
            assert_eq!(ledger.rewrites, kept, "{ends:?}");
            drop(systran);
            assert_eq!(fs::read_dir(scratch.path())?.count(), 0, "{ends:?}");
        }
        Ok(())
    }
}
