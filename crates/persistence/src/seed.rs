//! The fixture seed, its refresh and the `TRANEXTR` copy: the jobs that build
//! and reload the `CardDemo` files, as one database transaction per run.
//!
//! [`prepare`] and [`run`] replace these jobs. Every fixture is read through
//! the record codec of `carddemo-codec`, so the database is seeded from the
//! same bytes the codec round-trips (o4), framing included (CRLF, LF and the
//! 36-byte `cardxref.txt` records, E-04).
//!
//! | Job | Step replaced | Source → table | Audit rows |
//! |---|---|---|---|
//! | `ACCTFILE` | STEP15 `REPRO` [app/jcl/ACCTFILE.jcl:L54-61] | `acctdata.txt` → `accounts` | P-CVACT01Y-001..014 |
//! | `CARDFILE` | STEP15 `REPRO` [app/jcl/CARDFILE.jcl:L68-75] | `carddata.txt` → `cards` | P-CVACT02Y-001..008 |
//! | `XREFFILE` | STEP15 `REPRO` [app/jcl/XREFFILE.jcl:L57-64] | `cardxref.txt` → `card_xref` | P-CVACT03Y-001..005 |
//! | `CUSTFILE` | STEP15 `REPRO` [app/jcl/CUSTFILE.jcl:L64-71] | `custdata.txt` → `customers` | P-CVCUS01Y-001..020 |
//! | `DISCGRP` | STEP15 `REPRO` [app/jcl/DISCGRP.jcl:L54-61] | `discgrp.txt` → `disclosure_groups` | P-CVTRA02Y-001..006 |
//! | `TCATBALF` | STEP15 `REPRO` [app/jcl/TCATBALF.jcl:L54-61] | `tcatbal.txt` → `transaction_category_balances` | P-CVTRA01Y-001..006 |
//! | `TRANTYPE` | STEP15 `REPRO` [app/jcl/TRANTYPE.jcl:L54-61] | `trantype.txt` → `trantype_file` | P-CVTRA03Y-001..004 |
//! | `TRANCATG` | STEP15 `REPRO` [app/jcl/TRANCATG.jcl:L54-61] | `trancatg.txt` → `trancatg_file` | P-CVTRA04Y-001..005 |
//! | `DUSRSECJ` | STEP01 `IEBGENER`, STEP03 `REPRO` [app/jcl/DUSRSECJ.jcl:L32-45] [app/jcl/DUSRSECJ.jcl:L80-87] | `SYSUT1` in-stream rows → `users` | P-CSUSR01Y-001..007 |
//! | `CREADB21` | `LDTTYPE`, `LDTCCAT` [app/app-transaction-type-db2/jcl/CREADB21.jcl:L64-84] | `DB2LTTYP.ctl`, `DB2LTCAT.ctl` rows → `transaction_types`, `transaction_categories` | P-TRNTYPE-001..003, P-TRNTYCAT-001..004 |
//! | `TRANEXTR` | STEP30 to STEP50 [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L53-115] | `transaction_types`, `transaction_categories` → `trantype_file`, `trancatg_file` | — |
//!
//! # Modes
//!
//! - [`SeedMode::Full`] is the first seed and the refresh that replaces the
//!   README refresh sequence `ACCTFILE` … `DUSRSECJ` [README.md:L213-232]. It
//!   clears the reward tables, deletes and reloads every master file and
//!   processing copy, as each job's delete, define and `REPRO` steps do, and
//!   re-inserts the seed reward rules (AAP 0.3.6). The Db2 masters are loaded
//!   only while both are empty: the refresh sequence runs no `CREADB21`, so a
//!   master edited through the admin screens or `MNTTRDB2` survives a refresh.
//! - [`SeedMode::IfEmpty`] does nothing when `accounts` holds a row, and is
//!   [`SeedMode::Full`] otherwise. It judges the table once the run holds its
//!   `accounts` lock, so an account a concurrent unit of work committed before
//!   the lock was granted, a racing seed's included, makes it skip.
//! - [`SeedMode::OnlyTranextr`] is the `TRANEXTR` → `TRANTYPE`/`TRANCATG`
//!   chain: the Db2 masters are unloaded over the processing copies. Nothing
//!   else changes. The `.PS` backups `TRANEXTR` writes in STEP10 and STEP20
//!   are not reproduced, because seed jobs are replaced, not parity-tested.
//!
//! Each run is one database transaction, committed only at its end. Any
//! failure drops the transaction, which rolls it back, so the database is left
//! exactly as it was. A [`SeedMode::Full`] or [`SeedMode::IfEmpty`] run takes
//! `LOCK TABLE accounts IN EXCLUSIVE MODE` as its first statement and holds it
//! to its end: plain reads go on, and the run is serialized with every unit
//! of work that locks or writes an account row and with every other such run.
//!
//! # Two steps
//!
//! A run is split in two, because reading and decoding the sources is
//! blocking file I/O and hashing the ten `DUSRSECJ` passwords is memory-hard,
//! CPU-bound argon2id work, and neither ever yields:
//!
//! 1. [`prepare`], synchronous: reads and decodes every source the mode needs
//!    and hashes the passwords. It touches no database and answers with a
//!    [`PreparedSeed`].
//! 2. [`run`], asynchronous: the one database transaction that writes the
//!    prepared rows. It does no file I/O and no argon2 work.
//!
//! An asynchronous caller runs [`prepare`] on a blocking thread pool, such as
//! tokio's `spawn_blocking`, never inline on the executor, and before [`run`]
//! opens the transaction, so no connection is held while the sources are read
//! and hashed. This crate names no async runtime; the pool is the caller's.
//!
//! A source that cannot be read or decoded does not fail [`prepare`]: its
//! error is kept in the [`PreparedSeed`] and [`run`] returns it at the point
//! where it would have read that source, so every outcome is the one an
//! inline read gives. [`SeedMode::IfEmpty`] on a seeded database still writes
//! nothing and succeeds whatever the sources hold, the `DB2LT*.ctl` statements
//! fail a run only when both Db2 masters are empty, and a database error in an
//! earlier table's inserts comes before a later source's error.
//!
//! # What the seed never writes
//!
//! - The transaction master: `transactions` starts empty (P-CVTRA05Y-015,
//!   E-07), and no statement here names it. A refresh keeps its rows.
//! - `dailytran.txt`: it is `POSTTRAN` input, not seed data.
//! - Reward accruals and balances: the seed only clears them. Seeded accruals
//!   come from running `batch posttran` over `dailytran.txt` (AAP 0.3.6).
//! - The pending-authorization stores, which no refresh job touches.
//!
//! # Stored form
//!
//! The ASCII corpus seeds; the EBCDIC corpus serves only the codec round trip
//! (E-05). Text fields are bound right-trimmed, `FILLER` is not stored, and
//! the blank group IDs of all 50 accounts stay blank (E-06). Passwords are
//! stored as the argon2id hash of the 8 bytes `DUSRSECJ` holds, case
//! unchanged (E-20); no plaintext or hash is ever logged or returned.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use carddemo_codec::layouts::{
    AccountLayout, AccountRecord, CardLayout, CardRecord, CardXrefLayout, CardXrefRecord, Charset,
    CustomerLayout, CustomerRecord, DisclosureGroupLayout, DisclosureGroupRecord, Layout,
    TranCatBalanceLayout, TranCatBalanceRecord, TranCategoryFileRecord, TranCategoryLayout,
    TranTypeFileRecord, TranTypeLayout, UserSecurityLayout, decode_ascii_file,
};
use carddemo_domain::entities::user_security::hash_stored_password;
use carddemo_domain::entities::{NewRewardRule, TranCategoryMaster, TranTypeMaster};
use carddemo_domain::numeric::{
    Alnum, Money, NumericError, RewardRate, RuleId, TranCategoryCode, TranTypeCode,
};
use chrono::DateTime;
use rust_decimal::Decimal;
use sqlx::PgConnection;

use crate::tx::commit_transaction;
use crate::{Db, PersistenceError};

/// `ACCTDATA.PS`, the `ACCTFILE` input [app/jcl/ACCTFILE.jcl:L57].
const ACCTDATA: &str = "acctdata.txt";
/// `CARDDATA.PS`, the `CARDFILE` input [app/jcl/CARDFILE.jcl:L71].
const CARDDATA: &str = "carddata.txt";
/// `CARDXREF.PS`, the `XREFFILE` input [app/jcl/XREFFILE.jcl:L60].
const CARDXREF: &str = "cardxref.txt";
/// `CUSTDATA.PS`, the `CUSTFILE` input [app/jcl/CUSTFILE.jcl:L67].
const CUSTDATA: &str = "custdata.txt";
/// `DISCGRP.PS`, the `DISCGRP` input [app/jcl/DISCGRP.jcl:L57].
const DISCGRP: &str = "discgrp.txt";
/// `TCATBALF.PS`, the `TCATBALF` input [app/jcl/TCATBALF.jcl:L57].
const TCATBAL: &str = "tcatbal.txt";
/// `TRANTYPE.PS`, the `TRANTYPE` input [app/jcl/TRANTYPE.jcl:L57].
const TRANTYPE: &str = "trantype.txt";
/// `TRANCATG.PS`, the `TRANCATG` input [app/jcl/TRANCATG.jcl:L57].
const TRANCATG: &str = "trancatg.txt";
/// The `CREADB21` `LDTTYPE` statement, `&LBNM..CNTL(DB2LTTYP)`
/// [app/app-transaction-type-db2/jcl/CREADB21.jcl:L72].
const DB2LTTYP: &str = "DB2LTTYP.ctl";
/// The `CREADB21` `LDTCCAT` statement, `&LBNM..CNTL(DB2LTCAT)`
/// [app/app-transaction-type-db2/jcl/CREADB21.jcl:L84].
const DB2LTCAT: &str = "DB2LTCAT.ctl";

/// The seed reward rules of AAP 0.3.6: rule ID, transaction type code,
/// category code and `reward_rate` in units of 0.0001 points per dollar.
///
/// The rule ID is not inserted: it is the identity the restarted
/// `reward_rules.rule_id` sequence generates for the rule's position in this
/// list, which the load checks.
///
/// - Rule 1, retail purchase (1 pt/$): type `01` Purchase
///   [app/data/ASCII/trantype.txt:L1], category `0001` Regular Sales Draft
///   [app/data/ASCII/trancatg.txt:L1].
/// - Rule 2, travel (3 pts/$): type `04` Authorization
///   [app/data/ASCII/trantype.txt:L4], category `0003` Travel booking
///   authorization [app/data/ASCII/trancatg.txt:L14].
///
/// The request's dining, gas and grocery examples have no fixture category,
/// so they are not seeded and no category is invented for them (E-08).
const SEED_RULES: [(i64, &str, u16, i64); 2] = [(1, "01", 1, 10_000), (2, "04", 3, 30_000)];

/// `active_from` of every seed rule, 2022-01-01T00:00:00Z, in Unix seconds.
const SEED_RULES_ACTIVE_FROM: i64 = 1_640_995_200;

/// What a [`run`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedMode {
    /// `batch seed`: the first seed, or the refresh of a seeded database.
    Full,
    /// `batch seed --if-empty`: nothing when `accounts` holds a row, else
    /// [`SeedMode::Full`].
    IfEmpty,
    /// `batch seed --only tranextr`: the Db2 masters copied over the
    /// processing copies.
    OnlyTranextr,
}

/// Where [`prepare`] reads the sources of a run.
///
/// The caller decides every path; this crate reads no environment. `batch`
/// takes `fixture_dir` from `CARDDEMO_FIXTURE_DIR`.
#[derive(Debug, Clone)]
pub struct SeedSources {
    /// The directory of the ASCII fixture files, `app/data/ASCII` by default.
    pub fixture_dir: PathBuf,
    /// The `DUSRSECJ` job, whose `SYSUT1` in-stream rows are the users.
    pub dusrsecj_jcl: PathBuf,
    /// The directory of the `CREADB21` load statements `DB2LTTYP.ctl` and
    /// `DB2LTCAT.ctl`.
    pub db2_ctl_dir: PathBuf,
}

impl SeedSources {
    /// The sources at their places in the repository rooted at `repo_root`:
    /// `app/data/ASCII`, `app/jcl/DUSRSECJ.jcl` and
    /// `app/app-transaction-type-db2/ctl`.
    #[must_use]
    pub fn with_defaults(repo_root: &Path) -> Self {
        Self {
            fixture_dir: repo_root.join("app/data/ASCII"),
            dusrsecj_jcl: repo_root.join("app/jcl/DUSRSECJ.jcl"),
            db2_ctl_dir: repo_root.join("app/app-transaction-type-db2/ctl"),
        }
    }
}

/// What a [`run`] wrote: the rows inserted into each table, and how the run
/// went. Counts hold no row content.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedReport {
    /// Rows inserted into `accounts`.
    pub accounts: usize,
    /// Rows inserted into `cards`.
    pub cards: usize,
    /// Rows inserted into `card_xref`.
    pub card_xref: usize,
    /// Rows inserted into `customers`.
    pub customers: usize,
    /// Rows inserted into `disclosure_groups`.
    pub disclosure_groups: usize,
    /// Rows inserted into `transaction_category_balances`.
    pub tran_cat_balances: usize,
    /// Rows inserted into the processing copy `trantype_file`.
    pub trantype_file: usize,
    /// Rows inserted into the processing copy `trancatg_file`.
    pub trancatg_file: usize,
    /// Rows inserted into `users`.
    pub users: usize,
    /// Rows inserted into the Db2 master `transaction_types`.
    pub transaction_types: usize,
    /// Rows inserted into the Db2 master `transaction_categories`.
    pub transaction_categories: usize,
    /// Rows inserted into `reward_rules`.
    pub reward_rules: usize,
    /// The Db2 masters were empty and were loaded from the `CREADB21`
    /// statements.
    pub masters_loaded: bool,
    /// `accounts` held rows when the run had locked it, so the run was a
    /// refresh.
    pub refreshed: bool,
    /// [`SeedMode::IfEmpty`] found rows in `accounts` and wrote nothing.
    pub skipped: bool,
}

/// The sources of a run, read, decoded and hashed by [`prepare`] and not yet
/// written: what [`run`] writes in its one database transaction.
///
/// Every field is owned, so the prepared seed is `Send + 'static` and an
/// asynchronous caller can move it out of the blocking thread pool that
/// prepared it. `Debug` shows the mode and, per source, its row count or its
/// error text; no row, password or password hash reaches a log line.
#[must_use = "the seed is written only by `run`"]
pub struct PreparedSeed {
    /// What the mode needs.
    plan: Plan,
}

impl PreparedSeed {
    /// The mode [`prepare`] was given.
    fn mode(&self) -> SeedMode {
        match &self.plan {
            Plan::OnlyTranextr => SeedMode::OnlyTranextr,
            Plan::Load { if_empty: true, .. } => SeedMode::IfEmpty,
            Plan::Load {
                if_empty: false, ..
            } => SeedMode::Full,
        }
    }
}

impl fmt::Debug for PreparedSeed {
    /// The mode and, for a load, each source as `Ok(<rows>)` or
    /// `Err(<error text>)`, in the order [`run`] writes them.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("PreparedSeed");
        debug.field("mode", &self.mode());
        if let Plan::Load { sources, .. } = &self.plan {
            debug
                .field("accounts", &Outcome::of(&sources.accounts))
                .field("cards", &Outcome::of(&sources.cards))
                .field("card_xref", &Outcome::of(&sources.card_xref))
                .field("customers", &Outcome::of(&sources.customers))
                .field(
                    "disclosure_groups",
                    &Outcome::of(&sources.disclosure_groups),
                )
                .field(
                    "tran_cat_balances",
                    &Outcome::of(&sources.tran_cat_balances),
                )
                .field("trantype_file", &Outcome::of(&sources.trantype_file))
                .field("trancatg_file", &Outcome::of(&sources.trancatg_file))
                .field("users", &Outcome::of_users(&sources.users))
                .field(
                    "transaction_types",
                    &Outcome::of(&sources.transaction_types),
                )
                .field(
                    "transaction_categories",
                    &Outcome::of(&sources.transaction_categories),
                );
        }
        debug.finish()
    }
}

/// What a [`PreparedSeed`] holds for its mode.
enum Plan {
    /// [`SeedMode::OnlyTranextr`]: the copy reads only the database, so no
    /// source is read.
    OnlyTranextr,
    /// [`SeedMode::Full`] or, with `if_empty`, [`SeedMode::IfEmpty`]: every
    /// source is read.
    Load {
        /// The mode is [`SeedMode::IfEmpty`].
        if_empty: bool,
        /// The outcome of each source.
        sources: Box<PreparedSources>,
    },
}

/// The outcome of each source of a load: its rows, or the error reading,
/// decoding, parsing or hashing it gave.
struct PreparedSources {
    /// `acctdata.txt`.
    accounts: Result<Vec<AccountRecord>, PersistenceError>,
    /// `carddata.txt`.
    cards: Result<Vec<CardRecord>, PersistenceError>,
    /// `cardxref.txt`.
    card_xref: Result<Vec<CardXrefRecord>, PersistenceError>,
    /// `custdata.txt`.
    customers: Result<Vec<CustomerRecord>, PersistenceError>,
    /// `discgrp.txt`.
    disclosure_groups: Result<Vec<DisclosureGroupRecord>, PersistenceError>,
    /// `tcatbal.txt`.
    tran_cat_balances: Result<Vec<TranCatBalanceRecord>, PersistenceError>,
    /// `trantype.txt`.
    trantype_file: Result<Vec<TranTypeFileRecord>, PersistenceError>,
    /// `trancatg.txt`.
    trancatg_file: Result<Vec<TranCategoryFileRecord>, PersistenceError>,
    /// The `DUSRSECJ` `SYSUT1` rows.
    users: Result<PreparedUsers, PersistenceError>,
    /// `DB2LTTYP.ctl`.
    transaction_types: Result<Vec<TranTypeMaster>, PersistenceError>,
    /// `DB2LTCAT.ctl`.
    transaction_categories: Result<Vec<TranCategoryMaster>, PersistenceError>,
}

/// The `DUSRSECJ` users, prepared row by row in `SYSUT1` order as the load
/// inserts them: every row up to the first that does not decode or hash, and
/// that row's error, which the load returns after inserting the rows before
/// it.
struct PreparedUsers {
    /// The rows prepared, in order.
    rows: Vec<SeedUser>,
    /// The error of the first row that could not be prepared, if any; no row
    /// after it is decoded or hashed.
    failure: Option<PersistenceError>,
}

/// One `DUSRSECJ` user as its `users` row binds it, the password only as its
/// argon2id hash. `Debug` shows the user ID only.
struct SeedUser {
    /// `SEC-USR-ID`, right-trimmed.
    user_id: String,
    /// `SEC-USR-FNAME`, right-trimmed.
    first_name: String,
    /// `SEC-USR-LNAME`, right-trimmed.
    last_name: String,
    /// The argon2id PHC string of `SEC-USR-PWD` as stored.
    password_hash: String,
    /// `SEC-USR-TYPE`, right-trimmed.
    usr_type: String,
}

impl fmt::Debug for SeedUser {
    /// The user ID only, ended with `..` to show that the rest of the row,
    /// the password hash included, is left out.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SeedUser")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

/// A source's outcome as the `Debug` of a [`PreparedSeed`] shows it:
/// `Ok(<rows>)`, or `Err(<error text>)`. Error texts name files, lines,
/// records and fields, never a password or a hash.
struct Outcome<'a>(Result<usize, &'a PersistenceError>);

impl<'a> Outcome<'a> {
    /// The outcome of a source read into rows.
    fn of<T>(source: &'a Result<Vec<T>, PersistenceError>) -> Self {
        Self(source.as_ref().map(Vec::len))
    }

    /// The outcome of the users: their row count when every row was prepared,
    /// else the error that stopped them.
    fn of_users(users: &'a Result<PreparedUsers, PersistenceError>) -> Self {
        match users {
            Ok(PreparedUsers {
                failure: Some(err), ..
            })
            | Err(err) => Self(Err(err)),
            Ok(PreparedUsers {
                rows,
                failure: None,
            }) => Self(Ok(rows.len())),
        }
    }
}

impl fmt::Debug for Outcome<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Ok(rows) => formatter.debug_tuple("Ok").field(&rows).finish(),
            Err(err) => formatter
                .debug_tuple("Err")
                .field(&format_args!("{err}"))
                .finish(),
        }
    }
}

/// The first step of a run: reads, decodes and hashes the sources a run in
/// `mode` writes, without a database.
///
/// - [`SeedMode::Full`] and [`SeedMode::IfEmpty`] read every fixture file of
///   `sources.fixture_dir` through its codec layout, the `DUSRSECJ` `SYSUT1`
///   rows, each decoded and its stored password hashed with
///   [`hash_stored_password`], and both `DB2LT*.ctl` statements.
///   [`SeedMode::IfEmpty`] prepares as much as [`SeedMode::Full`], because
///   whether `accounts` holds a row is decided only inside the run's
///   transaction.
/// - [`SeedMode::OnlyTranextr`] reads nothing: the copy reads only the Db2
///   masters.
///
/// The plaintext of each password lives only until it is hashed; the
/// prepared seed keeps the user ID, names, type and hash.
///
/// Synchronous: the file reads block and argon2id hashing is memory-hard and
/// CPU-bound, and neither yields. An asynchronous caller runs this on a
/// blocking thread pool, such as tokio's `spawn_blocking`, before [`run`],
/// never inline on the executor.
///
/// A source that cannot be read, decoded, parsed or hashed fails nothing
/// here: its error is kept, and [`run`] returns it where it reaches that
/// source.
pub fn prepare(mode: SeedMode, sources: &SeedSources) -> PreparedSeed {
    let plan = match mode {
        SeedMode::OnlyTranextr => Plan::OnlyTranextr,
        SeedMode::Full | SeedMode::IfEmpty => Plan::Load {
            if_empty: mode == SeedMode::IfEmpty,
            sources: Box::new(read_sources(sources)),
        },
    };
    PreparedSeed { plan }
}

/// Every source of a load, each read on its own, so that a failing source
/// leaves the outcomes of the others as they are.
fn read_sources(sources: &SeedSources) -> PreparedSources {
    let dir = &sources.fixture_dir;
    let ctl = &sources.db2_ctl_dir;
    PreparedSources {
        accounts: read_fixture::<AccountLayout>(&dir.join(ACCTDATA)),
        cards: read_fixture::<CardLayout>(&dir.join(CARDDATA)),
        card_xref: read_fixture::<CardXrefLayout>(&dir.join(CARDXREF)),
        customers: read_fixture::<CustomerLayout>(&dir.join(CUSTDATA)),
        disclosure_groups: read_fixture::<DisclosureGroupLayout>(&dir.join(DISCGRP)),
        tran_cat_balances: read_fixture::<TranCatBalanceLayout>(&dir.join(TCATBAL)),
        trantype_file: read_fixture::<TranTypeLayout>(&dir.join(TRANTYPE)),
        trancatg_file: read_fixture::<TranCategoryLayout>(&dir.join(TRANCATG)),
        users: prepare_users(&sources.dusrsecj_jcl),
        transaction_types: read_tran_type_masters(&ctl.join(DB2LTTYP)),
        transaction_categories: read_tran_category_masters(&ctl.join(DB2LTCAT)),
    }
}

/// The second step of a run: writes `prepared` as its mode says, seeding,
/// refreshing or copying the reference data in one database transaction. It
/// reads no file and does no argon2 work.
///
/// The transaction commits only when every step has succeeded. On an error
/// before the commit it is dropped, which rolls it back. A `COMMIT` the server
/// fails has already rolled it back, and the commit, through the
/// crate-internal `commit_transaction` of `crate::tx`, sends no `ROLLBACK`
/// after it. Either way the database is unchanged.
///
/// A [`SeedMode::Full`] or [`SeedMode::IfEmpty`] load first locks `accounts`
/// in `EXCLUSIVE` mode for the rest of the transaction, and only then reads
/// whether the table holds a row. That one read decides both whether an
/// `--if-empty` run skips and whether the run is a refresh, and it sees every
/// account committed before the lock was granted, a racing seed's included.
/// [`SeedMode::OnlyTranextr`] touches no account and takes no lock.
///
/// A source error [`prepare`] kept is returned where the load reaches that
/// source, after the statements before it and before any insert of its own,
/// so the outcome is the one an inline read gives:
///
/// - a [`SeedMode::IfEmpty`] run that finds rows in `accounts` writes nothing
///   and succeeds, whatever the sources held;
/// - a `DB2LT*.ctl` error fails the run only when both Db2 masters are empty;
/// - a failed statement of an earlier table, or of an earlier `DUSRSECJ` row,
///   is returned instead of a later source's error.
///
/// # Errors
///
/// - [`PersistenceError::Io`] when a source file could not be read.
/// - [`PersistenceError::Codec`] when a fixture record or a `DUSRSECJ` row
///   did not decode through its copybook layout.
/// - [`PersistenceError::Fixture`] when the `DUSRSECJ` in-stream data or a
///   `DB2LT*.ctl` statement did not parse.
/// - [`PersistenceError::Password`] when a password could not be hashed.
/// - [`PersistenceError::Corrupt`] when a value does not fit its column, a
///   Db2 category code has no four-digit processing-copy form, or a seed
///   reward rule is not generated under its fixed ID.
/// - [`PersistenceError::Sqlx`] when a statement or the commit fails, for
///   example a duplicate key in a fixture.
pub async fn run(db: &Db, prepared: PreparedSeed) -> Result<SeedReport, PersistenceError> {
    let mode = prepared.mode();
    let mut tx = db.pool().begin().await?;
    let report = match prepared.plan {
        Plan::OnlyTranextr => tranextr(&mut tx).await?,
        Plan::Load { if_empty, sources } => {
            lock_accounts(&mut tx).await?;
            let has_rows = has_accounts(&mut tx).await?;
            if if_empty && has_rows {
                SeedReport {
                    skipped: true,
                    ..SeedReport::default()
                }
            } else {
                full(&mut tx, *sources, has_rows).await?
            }
        }
    };
    if report.skipped {
        tx.rollback().await?;
    } else {
        commit_transaction(&mut tx).await?;
    }
    log_report(mode, &report);
    Ok(report)
}

/// One event with every count of the report.
fn log_report(mode: SeedMode, report: &SeedReport) {
    tracing::info!(
        mode = ?mode,
        accounts = report.accounts,
        cards = report.cards,
        card_xref = report.card_xref,
        customers = report.customers,
        disclosure_groups = report.disclosure_groups,
        tran_cat_balances = report.tran_cat_balances,
        trantype_file = report.trantype_file,
        trancatg_file = report.trancatg_file,
        users = report.users,
        transaction_types = report.transaction_types,
        transaction_categories = report.transaction_categories,
        reward_rules = report.reward_rules,
        masters_loaded = report.masters_loaded,
        refreshed = report.refreshed,
        skipped = report.skipped,
        "seed finished"
    );
}

/// [`SeedMode::Full`]: clear, reload every file, load the Db2 masters while
/// they are empty, re-insert the seed reward rules. Each source's error, if
/// [`prepare`] kept one, is returned just before that source's inserts.
///
/// It runs under the lock of [`lock_accounts`]; `refreshed` is whether
/// `accounts` held a row once that lock was granted.
async fn full(
    conn: &mut PgConnection,
    sources: PreparedSources,
    refreshed: bool,
) -> Result<SeedReport, PersistenceError> {
    let PreparedSources {
        accounts,
        cards,
        card_xref,
        customers,
        disclosure_groups,
        tran_cat_balances,
        trantype_file,
        trancatg_file,
        users,
        transaction_types,
        transaction_categories,
    } = sources;
    clear(conn).await?;
    let mut report = SeedReport {
        accounts: load_accounts(conn, &accounts?).await?,
        cards: load_cards(conn, &cards?).await?,
        card_xref: load_card_xref(conn, &card_xref?).await?,
        customers: load_customers(conn, &customers?).await?,
        disclosure_groups: load_disclosure_groups(conn, &disclosure_groups?).await?,
        tran_cat_balances: load_tran_cat_balances(conn, &tran_cat_balances?).await?,
        trantype_file: load_trantype_file(conn, &trantype_file?).await?,
        trancatg_file: load_trancatg_file(conn, &trancatg_file?).await?,
        users: load_users(conn, users?).await?,
        refreshed,
        ..SeedReport::default()
    };
    if masters_empty(conn).await? {
        report.transaction_types = load_tran_type_masters(conn, &transaction_types?).await?;
        report.transaction_categories =
            load_tran_category_masters(conn, &transaction_categories?).await?;
        report.masters_loaded = true;
    }
    report.reward_rules = load_reward_rules(conn).await?;
    Ok(report)
}

/// Locks `accounts` with `LOCK TABLE accounts IN EXCLUSIVE MODE` until the
/// run ends: the first statement of every load, before [`has_accounts`]
/// judges the table.
///
/// The mode conflicts with the table lock every account row lock takes
/// (`SELECT ... FOR KEY SHARE` and `FOR UPDATE`, `UPDATE`, `DELETE`, and the
/// foreign key check of a `reward_balances` insert), with the one an `INSERT`
/// takes, and with itself, and not with plain reads. The statement therefore
/// waits for every unit of work that holds an account row lock or has
/// inserted an account and not yet ended, and for a load already running. A
/// unit that asks for any of these locks later waits for the run to end.
///
/// - `--if-empty` is judged once the lock is granted. Under `READ COMMITTED`
///   the read that follows takes its own snapshot, so it sees every account
///   committed while the run waited, and a [`SeedMode::IfEmpty`] run skips
///   instead of deleting them. Read before the lock, the table could look
///   empty to a run that then deletes the rows committed in between. Two
///   concurrent loads run one after the other, and a second `--if-empty` one
///   finds the first one's rows. The lock is requested once, in its final
///   mode: a weaker lock upgraded later would deadlock two runs that both
///   held it. A run that skips holds it only until it rolls back.
/// - It keeps the lock order [`crate::ids`] documents: account locks before
///   the reward tables. Holding no reward table yet, the run lets a unit that
///   already holds an account row finish, such as a CT02 add whose accrual is
///   still to read `reward_rules` and write the accrual and the balance. The
///   lock is held until the run ends, so the `DELETE` of `accounts` in
///   [`clear`] never waits for a row lock. Truncating the reward tables before
///   this lock would deadlock with that CT02: the run would hold them while
///   waiting at the `DELETE` for the account row CT02 holds, and CT02 would
///   wait for `reward_rules`.
async fn lock_accounts(conn: &mut PgConnection) -> Result<(), PersistenceError> {
    sqlx::query!("LOCK TABLE accounts IN EXCLUSIVE MODE")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Whether `accounts` holds a row: the `--if-empty` test, and what makes a
/// [`SeedMode::Full`] run a refresh. A load reads it under the lock of
/// [`lock_accounts`].
async fn has_accounts(conn: &mut PgConnection) -> Result<bool, PersistenceError> {
    let has_rows = sqlx::query_scalar!(r#"SELECT EXISTS (SELECT 1 FROM accounts) AS "has_rows!""#)
        .fetch_one(&mut *conn)
        .await?;
    Ok(has_rows)
}

/// Whether both Db2 masters are empty, the state `CREADB21` loads into.
async fn masters_empty(conn: &mut PgConnection) -> Result<bool, PersistenceError> {
    let empty = sqlx::query_scalar!(
        r#"SELECT NOT EXISTS (SELECT 1 FROM transaction_types)
              AND NOT EXISTS (SELECT 1 FROM transaction_categories) AS "empty!""#
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(empty)
}

/// Empties the reward tables and every table the seed reloads, children
/// before their parents. It runs under the lock of [`lock_accounts`], which
/// the run took first and holds until it ends, so no other unit of work holds
/// an account row lock meanwhile.
///
/// The reward tables are emptied first: `reward_balances` references the
/// accounts about to be reloaded, and seeded accruals come only from posting
/// the fixtures (AAP 0.3.2, 0.3.6). One `TRUNCATE … RESTART IDENTITY` empties
/// them and restarts the `rule_id` and `accrual_id` identities at 1, so the
/// seed rules are generated as rules 1 and 2. The restart is part of the run's
/// transaction: if the run or its commit fails, it rolls back with the rows,
/// together with every identity drawn from the restarted sequences, and the
/// surviving rows keep sequences that continue after them. `setval` would not
/// roll back, leaving those rows behind sequences that issue their identities
/// again. The `TRUNCATE` also holds `ACCESS EXCLUSIVE` on the three tables
/// until the run ends, so a concurrent rule or accrual writer waits instead of
/// drawing an identity from a restarted sequence. A CT02 or `POSTTRAN` accrual
/// takes its account row lock first, so during a run it waits at that lock,
/// before it reaches them. The other tables are the delete and define steps
/// (STEP05, STEP10) of each load job; none references another.
async fn clear(conn: &mut PgConnection) -> Result<(), PersistenceError> {
    sqlx::query!("TRUNCATE reward_accruals, reward_balances, reward_rules RESTART IDENTITY")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM card_xref")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM cards")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM accounts")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM customers")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM transaction_category_balances")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM disclosure_groups")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM trantype_file")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM trancatg_file")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM users")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// The bytes of a source file.
fn read_bytes(path: &Path) -> Result<Vec<u8>, PersistenceError> {
    std::fs::read(path).map_err(|source| PersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// The text of a JCL or control file.
fn read_text(path: &Path) -> Result<String, PersistenceError> {
    std::fs::read_to_string(path).map_err(|source| PersistenceError::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Every record of the ASCII fixture file `path`, decoded with the layout `L`
/// in file order. The frames (stored length and terminator) are the codec's
/// concern and are not stored.
fn read_fixture<L: Layout>(path: &Path) -> Result<Vec<L::Record>, PersistenceError> {
    let bytes = read_bytes(path)?;
    let framed = decode_ascii_file::<L>(&bytes).map_err(|source| PersistenceError::Codec {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(framed.into_iter().map(|record| record.record).collect())
}

/// An `X(n)` item as its right-trimmed `VARCHAR(n)` column holds it.
fn text<const N: usize>(item: &Alnum<N>) -> String {
    item.trimmed().into_owned()
}

/// A `9(09)` value as its `INTEGER` column binds it. Every nine-digit value
/// fits, so the error arm only guards the conversion against a wider domain
/// type; it never cuts a value.
fn integer(table: &'static str, column: &'static str, value: u32) -> Result<i32, PersistenceError> {
    i32::try_from(value).map_err(|err| PersistenceError::Corrupt {
        table,
        column,
        reason: err.to_string(),
    })
}

/// `ACCTFILE` STEP15 `REPRO INFILE(ACCTDATA) OUTFILE(ACCTVSAM)`
/// [app/jcl/ACCTFILE.jcl:L54-61]: `acctdata.txt` → `accounts`
/// (P-CVACT01Y-014).
///
/// Columns P-CVACT01Y-001 to P-CVACT01Y-012; the `FILLER X(178)`
/// P-CVACT01Y-013 is not stored. `ACCT-GROUP-ID` is blank in every fixture
/// record and is stored empty (E-06).
async fn load_accounts(
    conn: &mut PgConnection,
    records: &[AccountRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let account = &record.account;
        sqlx::query!(
            r#"INSERT INTO accounts (
                   account_id, acct_active_status, acct_curr_bal, acct_credit_limit,
                   acct_cash_credit_limit, acct_open_date, acct_expiraion_date,
                   acct_reissue_date, acct_curr_cyc_credit, acct_curr_cyc_debit,
                   acct_addr_zip, acct_group_id)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
            account.acct_id.to_i64(),
            text(&account.acct_active_status),
            account.acct_curr_bal.to_decimal(),
            account.acct_credit_limit.to_decimal(),
            account.acct_cash_credit_limit.to_decimal(),
            text(&account.acct_open_date),
            text(&account.acct_expiraion_date),
            text(&account.acct_reissue_date),
            account.acct_curr_cyc_credit.to_decimal(),
            account.acct_curr_cyc_debit.to_decimal(),
            text(&account.acct_addr_zip),
            account.acct_group_id.trimmed().into_owned()
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `CARDFILE` STEP15 `REPRO INFILE(CARDDATA) OUTFILE(CARDVSAM)`
/// [app/jcl/CARDFILE.jcl:L68-75]: `carddata.txt` → `cards` (P-CVACT02Y-008).
///
/// Columns P-CVACT02Y-001 to P-CVACT02Y-006; the `FILLER X(59)`
/// P-CVACT02Y-007 is not stored. The alternate index `CARDAIX`
/// [app/jcl/CARDFILE.jcl:L80-85] is the `cardaix` index of the table.
async fn load_cards(
    conn: &mut PgConnection,
    records: &[CardRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let card = &record.card;
        sqlx::query!(
            r#"INSERT INTO cards (
                   card_number, card_acct_id, card_cvv_cd, card_embossed_name,
                   card_expiraion_date, card_active_status)
               VALUES ($1, $2, $3, $4, $5, $6)"#,
            card.card_num.trimmed().into_owned(),
            card.card_acct_id.to_i64(),
            i32::from(card.card_cvv_cd.get()),
            text(&card.card_embossed_name),
            text(&card.card_expiraion_date),
            text(&card.card_active_status)
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `XREFFILE` STEP15 `REPRO INFILE(XREFDATA) OUTFILE(XREFVSAM)`
/// [app/jcl/XREFFILE.jcl:L57-64]: `cardxref.txt` → `card_xref`
/// (P-CVACT03Y-005).
///
/// Columns P-CVACT03Y-001 to P-CVACT03Y-003; the `FILLER X(14)`
/// P-CVACT03Y-004, which the fixture does not even store (E-04), is not
/// stored. The alternate index `CXACAIX` [app/jcl/XREFFILE.jcl:L69-74] is the
/// `cxacaix` index of the table.
async fn load_card_xref(
    conn: &mut PgConnection,
    records: &[CardXrefRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let xref = &record.xref;
        sqlx::query!(
            r#"INSERT INTO card_xref (xref_card_num, xref_cust_id, xref_acct_id)
               VALUES ($1, $2, $3)"#,
            xref.xref_card_num.trimmed().into_owned(),
            integer("card_xref", "xref_cust_id", xref.xref_cust_id.get())?,
            xref.xref_acct_id.to_i64()
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `CUSTFILE` STEP15 `REPRO INFILE(CUSTDATA) OUTFILE(CUSTVSAM)`
/// [app/jcl/CUSTFILE.jcl:L64-71]: `custdata.txt` → `customers`
/// (P-CVCUS01Y-020).
///
/// Columns P-CVCUS01Y-001 to P-CVCUS01Y-018; the `FILLER X(168)`
/// P-CVCUS01Y-019 is not stored.
async fn load_customers(
    conn: &mut PgConnection,
    records: &[CustomerRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let customer = &record.customer;
        sqlx::query!(
            r#"INSERT INTO customers (
                   customer_id, cust_first_name, cust_middle_name, cust_last_name,
                   cust_addr_line_1, cust_addr_line_2, cust_addr_line_3,
                   cust_addr_state_cd, cust_addr_country_cd, cust_addr_zip,
                   cust_phone_num_1, cust_phone_num_2, cust_ssn, cust_govt_issued_id,
                   cust_dob_yyyy_mm_dd, cust_eft_account_id, cust_pri_card_holder_ind,
                   cust_fico_credit_score)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                       $15, $16, $17, $18)"#,
            integer("customers", "customer_id", customer.cust_id.get())?,
            text(&customer.cust_first_name),
            text(&customer.cust_middle_name),
            text(&customer.cust_last_name),
            text(&customer.cust_addr_line_1),
            text(&customer.cust_addr_line_2),
            text(&customer.cust_addr_line_3),
            text(&customer.cust_addr_state_cd),
            text(&customer.cust_addr_country_cd),
            text(&customer.cust_addr_zip),
            text(&customer.cust_phone_num_1),
            text(&customer.cust_phone_num_2),
            integer("customers", "cust_ssn", customer.cust_ssn.get())?,
            text(&customer.cust_govt_issued_id),
            text(&customer.cust_dob_yyyy_mm_dd),
            text(&customer.cust_eft_account_id),
            text(&customer.cust_pri_card_holder_ind),
            i32::from(customer.cust_fico_credit_score.get())
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `DISCGRP` STEP15 `REPRO INFILE(DISCGRP) OUTFILE(DISCVSAM)`
/// [app/jcl/DISCGRP.jcl:L54-61]: `discgrp.txt` → `disclosure_groups`
/// (P-CVTRA02Y-006).
///
/// Columns P-CVTRA02Y-001 to P-CVTRA02Y-004; the `FILLER X(28)` of `0`
/// characters, P-CVTRA02Y-005, is not stored. The ASCII rate of record 34 is
/// the one seeded (E-05).
async fn load_disclosure_groups(
    conn: &mut PgConnection,
    records: &[DisclosureGroupRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let group = &record.group;
        sqlx::query!(
            r#"INSERT INTO disclosure_groups (
                   dis_acct_group_id, dis_tran_type_cd, dis_tran_cat_cd, dis_int_rate)
               VALUES ($1, $2, $3, $4)"#,
            group.key.dis_acct_group_id.trimmed().into_owned(),
            group.key.dis_tran_type_cd.trimmed().into_owned(),
            i32::from(group.key.dis_tran_cat_cd.get()),
            group.dis_int_rate.to_decimal()
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `TCATBALF` STEP15 `REPRO INFILE(TCATBAL) OUTFILE(TCATBALV)`
/// [app/jcl/TCATBALF.jcl:L54-61]: `tcatbal.txt` →
/// `transaction_category_balances` (P-CVTRA01Y-006).
///
/// Columns P-CVTRA01Y-001 to P-CVTRA01Y-004; the `FILLER X(22)` of `0`
/// characters, P-CVTRA01Y-005, is not stored.
async fn load_tran_cat_balances(
    conn: &mut PgConnection,
    records: &[TranCatBalanceRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let balance = &record.balance;
        sqlx::query!(
            r#"INSERT INTO transaction_category_balances (
                   trancat_acct_id, trancat_type_cd, trancat_cd, tran_cat_bal)
               VALUES ($1, $2, $3, $4)"#,
            balance.key.trancat_acct_id.to_i64(),
            balance.key.trancat_type_cd.trimmed().into_owned(),
            i32::from(balance.key.trancat_cd.get()),
            balance.tran_cat_bal.to_decimal()
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `TRANTYPE` STEP15 `REPRO INFILE(TRANTYPE) OUTFILE(TTYPVSAM)`
/// [app/jcl/TRANTYPE.jcl:L54-61]: `trantype.txt` → the processing copy
/// `trantype_file` (P-CVTRA03Y-004), in the fixture's mixed case.
///
/// Columns P-CVTRA03Y-001 and P-CVTRA03Y-002; the `FILLER X(08)`
/// P-CVTRA03Y-003 is not stored.
async fn load_trantype_file(
    conn: &mut PgConnection,
    records: &[TranTypeFileRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let tran_type = &record.tran_type;
        sqlx::query!(
            r#"INSERT INTO trantype_file (tran_type, tran_type_desc) VALUES ($1, $2)"#,
            tran_type.tran_type.trimmed().into_owned(),
            text(&tran_type.tran_type_desc)
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `TRANCATG` STEP15 `REPRO INFILE(TRANCATG) OUTFILE(TCATVSAM)`
/// [app/jcl/TRANCATG.jcl:L54-61]: `trancatg.txt` → the processing copy
/// `trancatg_file` (P-CVTRA04Y-005), in the fixture's mixed case.
///
/// Columns P-CVTRA04Y-001 to P-CVTRA04Y-003; the `FILLER X(04)`
/// P-CVTRA04Y-004 is not stored.
async fn load_trancatg_file(
    conn: &mut PgConnection,
    records: &[TranCategoryFileRecord],
) -> Result<usize, PersistenceError> {
    for record in records {
        let category = &record.tran_category;
        sqlx::query!(
            r#"INSERT INTO trancatg_file (tran_type_cd, tran_cat_cd, tran_cat_type_desc)
               VALUES ($1, $2, $3)"#,
            category.tran_type_cd.trimmed().into_owned(),
            i32::from(category.tran_cat_cd.get()),
            text(&category.tran_cat_type_desc)
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(records.len())
}

/// `DUSRSECJ` STEP01 `IEBGENER` [app/jcl/DUSRSECJ.jcl:L32-45]: the ten
/// `SYSUT1` in-stream rows of the job at `path`, each prepared as its `users`
/// row (P-CSUSR01Y-007), in order.
///
/// Each row is one `CSUSR01Y` record, decoded with its layout: columns
/// P-CSUSR01Y-001 to P-CSUSR01Y-003 and P-CSUSR01Y-005, the user type taken
/// from the row's own type byte. `SEC-USR-PWD` (P-CSUSR01Y-004) is kept only
/// as the argon2id hash of its 8 bytes as stored, case unchanged (E-20); the
/// decoded record holding the plaintext is dropped as soon as it is hashed.
/// `SEC-USR-FILLER` (P-CSUSR01Y-006) is not stored.
///
/// The rows are prepared one by one, as the load inserts them: preparation
/// stops at the first row that does not decode or hash, and
/// [`PreparedUsers::failure`] holds that row's error.
///
/// # Errors
///
/// [`PersistenceError::Io`] when the job cannot be read, and the
/// [`parse_sysut1`] error when its in-stream data does not parse.
fn prepare_users(path: &Path) -> Result<PreparedUsers, PersistenceError> {
    let jcl = read_text(path)?;
    let rows = parse_sysut1(&jcl, path)?;
    let mut prepared = Vec::with_capacity(rows.len());
    for (record_number, (_line, row)) in (1..).zip(&rows) {
        match prepare_user(path, record_number, row) {
            Ok(user) => prepared.push(user),
            Err(failure) => {
                return Ok(PreparedUsers {
                    rows: prepared,
                    failure: Some(failure),
                });
            }
        }
    }
    Ok(PreparedUsers {
        rows: prepared,
        failure: None,
    })
}

/// The `users` row of the `SYSUT1` row `row`, record `record_number` of the
/// job at `path`: decoded through `CSUSR01Y`, its password hashed with
/// [`hash_stored_password`].
///
/// # Errors
///
/// - [`PersistenceError::Codec`] at the record when the row does not decode.
/// - [`PersistenceError::Password`] when the password cannot be hashed.
fn prepare_user(
    path: &Path,
    record_number: usize,
    row: &[u8],
) -> Result<SeedUser, PersistenceError> {
    let user = UserSecurityLayout::decode(row, Charset::Ascii).map_err(|source| {
        PersistenceError::Codec {
            path: path.to_path_buf(),
            source: source.at_record(record_number),
        }
    })?;
    let password_hash = hash_stored_password(&user.sec_usr_pwd.as_text())?;
    Ok(SeedUser {
        user_id: user.sec_usr_id.trimmed().into_owned(),
        first_name: text(&user.sec_usr_fname),
        last_name: text(&user.sec_usr_lname),
        password_hash,
        usr_type: text(&user.sec_usr_type),
    })
}

/// `DUSRSECJ` STEP03 `REPRO INFILE(IN) OUTFILE(OUT)`
/// [app/jcl/DUSRSECJ.jcl:L80-87]: the prepared `SYSUT1` rows → `users`
/// (P-CSUSR01Y-007), in order, each with the hash [`prepare_users`] computed.
///
/// # Errors
///
/// The error of a failed insert; else, after the rows before it are
/// inserted, the error of the first row [`prepare_users`] could not prepare.
async fn load_users(
    conn: &mut PgConnection,
    users: PreparedUsers,
) -> Result<usize, PersistenceError> {
    let PreparedUsers { rows, failure } = users;
    for user in &rows {
        sqlx::query!(
            r#"INSERT INTO users (user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type)
               VALUES ($1, $2, $3, $4, $5)"#,
            user.user_id.as_str(),
            user.first_name.as_str(),
            user.last_name.as_str(),
            user.password_hash.as_str(),
            user.usr_type.as_str()
        )
        .execute(&mut *conn)
        .await?;
    }
    match failure {
        Some(err) => Err(err),
        None => Ok(rows.len()),
    }
}

/// The `SYSUT1` in-stream records of the `DUSRSECJ` job text `text`, each with
/// its 1-based line number and padded with spaces to the 80-byte `USRSEC`
/// record (`LRECL=80` [app/jcl/DUSRSECJ.jcl:L48]).
///
/// The data starts after the first `//SYSUT1 DD *` statement
/// [app/jcl/DUSRSECJ.jcl:L34] and ends, as JCL ends `DD *` data, at the first
/// line that starts with `/*` or `//` [app/jcl/DUSRSECJ.jcl:L45]. A trailing
/// carriage return is not part of a row.
///
/// # Errors
///
/// [`PersistenceError::Fixture`] at the offending line when there is no
/// `//SYSUT1 DD *` statement, when a row is longer than 80 bytes, when the
/// data holds no row, or when the file ends before its delimiter.
fn parse_sysut1(text: &str, path: &Path) -> Result<Vec<(usize, Vec<u8>)>, PersistenceError> {
    let fixture = |line: usize, reason: String| PersistenceError::Fixture {
        path: path.to_path_buf(),
        line,
        reason,
    };
    let last_line = text.lines().count().max(1);
    let mut lines = (1..).zip(
        text.lines()
            .map(|line| line.strip_suffix('\r').unwrap_or(line)),
    );
    if !lines.by_ref().any(|(_, line)| is_sysut1_dd_star(line)) {
        return Err(fixture(last_line, "no //SYSUT1 DD * statement".to_owned()));
    }
    let mut rows = Vec::new();
    for (number, line) in lines {
        if line.starts_with("/*") || line.starts_with("//") {
            if rows.is_empty() {
                return Err(fixture(
                    number,
                    "SYSUT1 in-stream data holds no row".to_owned(),
                ));
            }
            return Ok(rows);
        }
        let bytes = line.as_bytes();
        if bytes.len() > UserSecurityLayout::LEN {
            return Err(fixture(
                number,
                format!(
                    "row of {} bytes is longer than the {}-byte USRSEC record",
                    bytes.len(),
                    UserSecurityLayout::LEN
                ),
            ));
        }
        let mut row = bytes.to_vec();
        row.resize(UserSecurityLayout::LEN, b' ');
        rows.push((number, row));
    }
    Err(fixture(
        last_line,
        "SYSUT1 in-stream data has no /* delimiter".to_owned(),
    ))
}

/// Whether `line` is the statement `//SYSUT1 DD *`, whatever its spacing.
fn is_sysut1_dd_star(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    tokens.next() == Some("//SYSUT1") && tokens.next() == Some("DD") && tokens.next() == Some("*")
}

/// `CREADB21` `LDTTYPE` [app/app-transaction-type-db2/jcl/CREADB21.jcl:L64-72]:
/// the `DB2LTTYP.ctl` rows → the Db2 master `transaction_types`
/// (P-TRNTYPE-003), upper case as loaded (`PURCHASE`), the `REVERAL` of type
/// `06` included as written. Columns P-TRNTYPE-001 and P-TRNTYPE-002.
async fn load_tran_type_masters(
    conn: &mut PgConnection,
    masters: &[TranTypeMaster],
) -> Result<usize, PersistenceError> {
    for master in masters {
        sqlx::query!(
            r#"INSERT INTO transaction_types (tr_type, tr_description) VALUES ($1, $2)"#,
            master.tr_type.trimmed().into_owned(),
            master.tr_description.as_str()
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(masters.len())
}

/// `CREADB21` `LDTCCAT` [app/app-transaction-type-db2/jcl/CREADB21.jcl:L77-84]:
/// the `DB2LTCAT.ctl` rows → the Db2 master `transaction_categories`
/// (P-TRNTYCAT-004). Columns P-TRNTYCAT-001 to P-TRNTYCAT-003; the category
/// keeps its `CHAR(4)` text, the zero-padded form of its code.
async fn load_tran_category_masters(
    conn: &mut PgConnection,
    masters: &[TranCategoryMaster],
) -> Result<usize, PersistenceError> {
    for master in masters {
        sqlx::query!(
            r#"INSERT INTO transaction_categories (trc_type_code, trc_type_category, trc_cat_data)
               VALUES ($1, $2, $3)"#,
            master.trc_type_code.trimmed().into_owned(),
            master.trc_type_category.to_string(),
            text(&master.trc_cat_data)
        )
        .execute(&mut *conn)
        .await?;
    }
    Ok(masters.len())
}

/// The masters of the `DB2LTTYP.ctl` statement at `path`.
///
/// # Errors
///
/// [`PersistenceError::Io`] when the file cannot be read, and the
/// [`parse_tran_type_masters`] error when it does not parse.
fn read_tran_type_masters(path: &Path) -> Result<Vec<TranTypeMaster>, PersistenceError> {
    parse_tran_type_masters(&read_text(path)?, path)
}

/// The masters of the `DB2LTCAT.ctl` statement at `path`.
///
/// # Errors
///
/// [`PersistenceError::Io`] when the file cannot be read, and the
/// [`parse_tran_category_masters`] error when it does not parse.
fn read_tran_category_masters(path: &Path) -> Result<Vec<TranCategoryMaster>, PersistenceError> {
    parse_tran_category_masters(&read_text(path)?, path)
}

/// The masters of `DB2LTTYP.ctl`, one per `SELECT '<type>','<description>'`
/// line [app/app-transaction-type-db2/ctl/DB2LTTYP.ctl:L17-23].
///
/// The description is stored without trailing spaces, the right-trimmed form
/// the `transaction_types` column holds (the `X(n)` storage rule); the
/// `DB2LTTYP` literals have none.
///
/// # Errors
///
/// [`PersistenceError::Fixture`] at the line of a statement that does not hold
/// exactly two literals, a type code longer than `CHAR(2)` or a description
/// longer than `VARCHAR(50)`, or when the file holds no row.
fn parse_tran_type_masters(
    text: &str,
    path: &Path,
) -> Result<Vec<TranTypeMaster>, PersistenceError> {
    literal_rows::<2>(text, path)?
        .into_iter()
        .map(|(line, [code, description])| {
            let tr_type = TranTypeCode::from_trimmed(&code)
                .map_err(|err| bad_literal(path, line, "type code", &err))?;
            let description = description.trim_end_matches(' ').to_owned();
            TranTypeMaster::try_new(tr_type, description)
                .map_err(|err| bad_literal(path, line, "description", &err))
        })
        .collect()
}

/// The masters of `DB2LTCAT.ctl`, one per
/// `SELECT '<type>','<category>','<description>'` line
/// [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:L20-37].
///
/// # Errors
///
/// [`PersistenceError::Fixture`] at the line of a statement that does not hold
/// exactly three literals, a type code longer than `CHAR(2)`, a category that
/// is not the four digits of a `9(04)` code, or a description longer than
/// `VARCHAR(50)`, or when the file holds no row.
fn parse_tran_category_masters(
    text: &str,
    path: &Path,
) -> Result<Vec<TranCategoryMaster>, PersistenceError> {
    literal_rows::<3>(text, path)?
        .into_iter()
        .map(|(line, [code, category, description])| {
            let trc_type_code = TranTypeCode::from_trimmed(&code)
                .map_err(|err| bad_literal(path, line, "type code", &err))?;
            let trc_type_category = four_digit_category(&category)
                .map_err(|err| bad_literal(path, line, "category", &err))?;
            TranCategoryMaster::try_new(trc_type_code, trc_type_category, &description)
                .map_err(|err| bad_literal(path, line, "description", &err))
        })
        .collect()
}

/// The category code of a `CHAR(4)` category literal. Only four digits are
/// accepted: a shorter literal would be a blank-padded `CHAR(4)` value with no
/// zero-padded code form, which `TRANEXTR` could not copy into the `9(04)`
/// `TRAN-CAT-CD` (P-CVTRA04Y-002).
fn four_digit_category(literal: &str) -> Result<TranCategoryCode, NumericError> {
    if literal.len() == TranCategoryCode::WIDTH {
        TranCategoryCode::from_str(literal)
    } else {
        Err(NumericError::WrongLength {
            expected: TranCategoryCode::WIDTH,
            actual: literal.len(),
        })
    }
}

/// The `Fixture` error of a literal that does not fit its column.
fn bad_literal(path: &Path, line: usize, what: &str, err: &NumericError) -> PersistenceError {
    PersistenceError::Fixture {
        path: path.to_path_buf(),
        line,
        reason: format!("{what} literal does not fit its column: {err}"),
    }
}

/// The lines of a `DB2LT*.ctl` statement that hold quoted literals, each with
/// its 1-based line number and exactly `N` literals. Lines without a literal,
/// such as the comment header and `WITH DMY AS (...)`
/// [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:L19], are skipped.
///
/// # Errors
///
/// [`PersistenceError::Fixture`] at a line whose literal count is not `N` or
/// whose literal is not closed, or at the last line when no line holds one.
fn literal_rows<const N: usize>(
    text: &str,
    path: &Path,
) -> Result<Vec<(usize, [String; N])>, PersistenceError> {
    let fixture = |line: usize, reason: String| PersistenceError::Fixture {
        path: path.to_path_buf(),
        line,
        reason,
    };
    let mut rows = Vec::new();
    for (number, line) in (1..).zip(text.lines()) {
        let literals =
            quoted_literals(line).map_err(|reason| fixture(number, reason.to_owned()))?;
        if literals.is_empty() {
            continue;
        }
        let count = literals.len();
        let row = <[String; N]>::try_from(literals).map_err(|_| {
            fixture(
                number,
                format!("expected {N} quoted literals, found {count}"),
            )
        })?;
        rows.push((number, row));
    }
    if rows.is_empty() {
        return Err(fixture(
            text.lines().count().max(1),
            "no line holds a quoted literal".to_owned(),
        ));
    }
    Ok(rows)
}

/// The single-quoted SQL literals of one line, in order, with `''` read as one
/// quote. Text inside a `/* ... */` comment, or after `--`, is skipped up to
/// the end of the comment or of the line.
///
/// # Errors
///
/// The reason text when a literal is not closed on its line.
fn quoted_literals(line: &str) -> Result<Vec<String>, &'static str> {
    let mut literals = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            '\'' => {
                let mut literal = String::new();
                loop {
                    match chars.next() {
                        Some('\'') if chars.peek() == Some(&'\'') => {
                            chars.next();
                            literal.push('\'');
                        }
                        Some('\'') => break,
                        Some(other) => literal.push(other),
                        None => return Err("a quoted literal is not closed on its line"),
                    }
                }
                literals.push(literal);
            }
            '-' if chars.peek() == Some(&'-') => break,
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = None;
                for inner in chars.by_ref() {
                    if previous == Some('*') && inner == '/' {
                        break;
                    }
                    previous = Some(inner);
                }
            }
            _ => {}
        }
    }
    Ok(literals)
}

/// [`SeedMode::OnlyTranextr`]: `TRANEXTR` and the load jobs it feeds.
///
/// STEP30 deletes `TRANTYPE.PS` and `TRANCATG.PS`
/// [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L53-61]; STEP40 unloads
/// `TR_TYPE` and `TR_DESCRIPTION` `ORDER BY TR_TYPE`
/// [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L65-84]; STEP50 unloads the
/// categories `ORDER BY TRC_TYPE_CODE, TRC_TYPE_CATEGORY`
/// [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L95-115]; the `TRANTYPE` and
/// `TRANCATG` jobs then reload the VSAM processing copies from those files.
/// Each unloaded description is `CAST(... AS CHAR(50))`, so the copy holds it
/// right-trimmed, as every `X(n)` column does; the `'0'` filler the unload
/// appends is the `FILLER` the tables do not store (P-CVTRA03Y-003,
/// P-CVTRA04Y-004).
///
/// The categories are checked first: the `CHAR(4)` text becomes the `9(04)`
/// `TRAN-CAT-CD` (P-CVTRA04Y-002), which only four digits express. Every
/// loaded category is four digits, and no carried program changes one.
async fn tranextr(conn: &mut PgConnection) -> Result<SeedReport, PersistenceError> {
    let unconvertible = sqlx::query_scalar!(
        r#"SELECT EXISTS (
               SELECT 1 FROM transaction_categories
                WHERE trc_type_category !~ '^[0-9]{4}$') AS "found!""#
    )
    .fetch_one(&mut *conn)
    .await?;
    if unconvertible {
        return Err(PersistenceError::Corrupt {
            table: "transaction_categories",
            column: "trc_type_category",
            reason: "a category is not the four digits of a 9(04) code".to_owned(),
        });
    }
    sqlx::query!("DELETE FROM trantype_file")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM trancatg_file")
        .execute(&mut *conn)
        .await?;
    let types = sqlx::query!(
        r#"INSERT INTO trantype_file (tran_type, tran_type_desc)
           SELECT tr_type, rtrim(tr_description)
             FROM transaction_types
            ORDER BY tr_type COLLATE "C""#
    )
    .execute(&mut *conn)
    .await?;
    let categories = sqlx::query!(
        r#"INSERT INTO trancatg_file (tran_type_cd, tran_cat_cd, tran_cat_type_desc)
           SELECT trc_type_code, trc_type_category::integer, rtrim(trc_cat_data)
             FROM transaction_categories
            ORDER BY trc_type_code COLLATE "C", trc_type_category COLLATE "C""#
    )
    .execute(&mut *conn)
    .await?;
    Ok(SeedReport {
        trantype_file: row_count(types.rows_affected()),
        trancatg_file: row_count(categories.rows_affected()),
        ..SeedReport::default()
    })
}

/// A statement's row count as a report count. A count above `usize::MAX`
/// cannot occur on the 64-bit targets the crate builds for; it saturates
/// rather than wraps.
fn row_count(rows: u64) -> usize {
    usize::try_from(rows).unwrap_or(usize::MAX)
}

/// The seed reward rules of [`SEED_RULES`] with their identities, checked
/// through the domain constructors.
///
/// # Errors
///
/// [`PersistenceError::Corrupt`] naming the column whose seed value its domain
/// type rejects.
fn seed_rules() -> Result<Vec<(RuleId, NewRewardRule)>, PersistenceError> {
    let corrupt = |column: &'static str, reason: String| PersistenceError::Corrupt {
        table: "reward_rules",
        column,
        reason,
    };
    let active_from = DateTime::from_timestamp(SEED_RULES_ACTIVE_FROM, 0).ok_or_else(|| {
        corrupt(
            "active_from",
            "the seed instant is outside the supported range".to_owned(),
        )
    })?;
    SEED_RULES
        .iter()
        .map(|&(rule_id, type_cd, category_cd, rate_units)| {
            let transaction_category_cd = TranCategoryCode::new(category_cd)
                .map_err(|err| corrupt("transaction_category_cd", err.to_string()))?;
            let reward_rate = RewardRate::from_decimal(Decimal::new(rate_units, 4))
                .map_err(|err| corrupt("reward_rate", err.to_string()))?;
            let rule = NewRewardRule {
                transaction_type_cd: TranTypeCode::move_from(type_cd),
                transaction_category_cd,
                reward_rate,
                min_transaction_amount: None,
                max_transaction_amount: None,
                active_from,
                active_to: None,
            };
            Ok((RuleId::from(rule_id), rule))
        })
        .collect()
}

/// Inserts the seed reward rules in [`SEED_RULES`] order into the table
/// [`clear`] has emptied and whose identities it has restarted, so that a
/// refresh restores the seeded state exactly: the rules are generated as rules
/// 1 and 2, the next rule an admin creates is rule 3, and the first accrual
/// `batch posttran` writes is accrual 1 (AAP 0.3.6). No identity is supplied
/// or set here, so every identity change rolls back with the run.
///
/// # Errors
///
/// - [`PersistenceError::Corrupt`] when the identity generated for a rule is
///   not its [`SEED_RULES`] ID.
/// - [`PersistenceError::Sqlx`] when an insert fails.
async fn load_reward_rules(conn: &mut PgConnection) -> Result<usize, PersistenceError> {
    let rules = seed_rules()?;
    for (rule_id, rule) in &rules {
        let generated = sqlx::query_scalar!(
            r#"INSERT INTO reward_rules (
                   transaction_type_cd, transaction_category_cd, reward_rate,
                   min_transaction_amount, max_transaction_amount, active_from, active_to)
               VALUES ($1, $2, $3, $4, $5, $6, $7)
               RETURNING rule_id"#,
            rule.transaction_type_cd.trimmed().into_owned(),
            i32::from(rule.transaction_category_cd.get()),
            rule.reward_rate.to_decimal(),
            rule.min_transaction_amount.map(Money::to_decimal),
            rule.max_transaction_amount.map(Money::to_decimal),
            rule.active_from,
            rule.active_to
        )
        .fetch_one(&mut *conn)
        .await?;
        if generated != rule_id.get() {
            return Err(PersistenceError::Corrupt {
                table: "reward_rules",
                column: "rule_id",
                reason: format!("seed rule {rule_id} was generated as rule {generated}"),
            });
        }
    }
    Ok(rules.len())
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use carddemo_codec::{Charset, CodecErrorKind, Layout, UserSecurityLayout, UserSecurityRecord};
    use carddemo_domain::auth::verify_as_entered;
    use carddemo_domain::numeric::{TranCategoryCode, TranTypeCode};
    use chrono::{TimeZone, Utc};
    use rust_decimal::Decimal;

    use super::{
        PersistenceError, PreparedSeed, SeedMode, SeedSources, parse_sysut1,
        parse_tran_category_masters, parse_tran_type_masters, prepare, prepare_user, prepare_users,
        quoted_literals, seed_rules,
    };

    /// A prepared seed moves from the blocking thread pool that prepared it to
    /// the task that writes it, so it must be `Send + 'static`.
    const _: () = {
        const fn send_and_static<T: Send + 'static>() {}
        send_and_static::<PreparedSeed>();
    };

    const DUSRSECJ: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../app/jcl/DUSRSECJ.jcl");
    const DB2LTTYP: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/app-transaction-type-db2/ctl/DB2LTTYP.ctl"
    );
    const DB2LTCAT: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../app/app-transaction-type-db2/ctl/DB2LTCAT.ctl"
    );

    fn source(path: &str) -> String {
        std::fs::read_to_string(path).unwrap_or_else(|err| panic!("cannot read {path}: {err}"))
    }

    /// The repository root, two levels above this crate.
    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn decode_user(row: &[u8]) -> UserSecurityRecord {
        UserSecurityLayout::decode(row, Charset::Ascii).expect("a CSUSR01Y record")
    }

    /// The line of a `Fixture` error; any other result fails the test.
    fn fixture_line<T: std::fmt::Debug>(result: Result<T, PersistenceError>) -> usize {
        match result {
            Err(PersistenceError::Fixture { line, .. }) => line,
            other => panic!("expected a Fixture error, got {other:?}"),
        }
    }

    fn code(text: &str) -> TranTypeCode {
        TranTypeCode::move_from(text)
    }

    fn category(value: u16) -> TranCategoryCode {
        TranCategoryCode::new(value).expect("a 9(04) category code")
    }

    #[test]
    fn defaults_point_into_the_repository() {
        let sources = SeedSources::with_defaults(Path::new("/repo"));
        assert_eq!(sources.fixture_dir, PathBuf::from("/repo/app/data/ASCII"));
        assert_eq!(
            sources.dusrsecj_jcl,
            PathBuf::from("/repo/app/jcl/DUSRSECJ.jcl")
        );
        assert_eq!(
            sources.db2_ctl_dir,
            PathBuf::from("/repo/app/app-transaction-type-db2/ctl")
        );
    }

    /// DUSRSECJ STEP01 SYSUT1 [app/jcl/DUSRSECJ.jcl:L35-44] (P-CSUSR01Y-007..009).
    #[test]
    fn dusrsecj_holds_ten_users_of_both_types() {
        let rows = parse_sysut1(&source(DUSRSECJ), Path::new(DUSRSECJ)).expect("SYSUT1 rows");
        let lines: Vec<usize> = rows.iter().map(|(line, _)| *line).collect();
        assert_eq!(lines, (35..=44).collect::<Vec<_>>());
        assert!(
            rows.iter()
                .all(|(_, row)| row.len() == UserSecurityLayout::LEN)
        );

        let users: Vec<UserSecurityRecord> = rows.iter().map(|(_, row)| decode_user(row)).collect();
        let first = &users[0];
        assert_eq!(first.sec_usr_id.trimmed(), "ADMIN001");
        assert_eq!(first.sec_usr_fname.trimmed(), "MARGARET");
        assert_eq!(first.sec_usr_lname.trimmed(), "GOLD");
        assert_eq!(first.sec_usr_pwd.as_text(), "PASSWORD");
        assert_eq!(first.sec_usr_type.as_text(), "A");
        let last = &users[9];
        assert_eq!(last.sec_usr_id.trimmed(), "USER0005");
        assert_eq!(last.sec_usr_type.as_text(), "U");
        let admins = users
            .iter()
            .filter(|user| user.sec_usr_type.as_text() == "A");
        assert_eq!(admins.count(), 5);
        assert!(
            users
                .iter()
                .all(|user| user.sec_usr_pwd.as_text() == "PASSWORD")
        );
        assert!(users.iter().all(|user| user.filler.is_blank()));
    }

    #[test]
    fn sysut1_rows_end_at_either_delimiter_and_lose_their_carriage_return() {
        for text in [
            "//STEP01 EXEC PGM=IEBGENER\r\n//SYSUT1   DD *\r\nADMIN001X\r\n/*\r\n",
            "//SYSUT1 DD *\nADMIN001X\n//SYSUT2   DD DSN=X\nIGNORED\n",
        ] {
            let rows = parse_sysut1(text, Path::new("t.jcl")).expect("one row");
            assert_eq!(rows.len(), 1, "{text:?}");
            let (line, row) = &rows[0];
            assert_eq!(*line, if text.starts_with("//STEP01") { 3 } else { 2 });
            assert_eq!(&row[..9], b"ADMIN001X");
            assert!(row[9..].iter().all(|&byte| byte == b' '));
        }
    }

    #[test]
    fn malformed_sysut1_data_names_its_line() {
        let long_row = format!("//SYSUT1 DD *\n{}\n/*\n", "X".repeat(81));
        let cases = [
            ("//STEP01 EXEC PGM=IEBGENER\n//SYSUT2 DD *\n", 2),
            ("//SYSUT1 DD *\n/*\n", 2),
            ("//SYSUT1 DD *\nADMIN001\nUSER0001\n", 3),
            (long_row.as_str(), 2),
            ("", 1),
        ];
        for (text, line) in cases {
            assert_eq!(
                fixture_line(parse_sysut1(text, Path::new("t.jcl"))),
                line,
                "{text:?}"
            );
        }
        let exact = format!("//SYSUT1 DD *\n{}\n/*\n", "X".repeat(80));
        assert!(parse_sysut1(&exact, Path::new("t.jcl")).is_ok());
    }

    #[test]
    fn quoted_literals_follow_sql_quoting_and_comments() {
        let cases: [(&str, &[&str]); 7] = [
            (
                " SELECT '01','PURCHASE'      FROM SYSIBM.SYSDUMMY1 UNION ALL",
                &["01", "PURCHASE"],
            ),
            (" WITH DMY AS (SELECT * FROM SYSIBM.SYSDUMMY1)", &[]),
            ("/* \"AS IS\" BASIS, it's quoted */", &[]),
            ("/* 'a' */ 'b' /* 'c' */ 'd'", &["b", "d"]),
            ("'it''s' -- 'comment'", &["it's"]),
            ("'' 'x'", &["", "x"]),
            ("'a-- b' '/* c */'", &["a-- b", "/* c */"]),
        ];
        for (line, expected) in cases {
            assert_eq!(
                quoted_literals(line),
                Ok(expected.iter().map(|s| (*s).to_owned()).collect()),
                "{line}"
            );
        }
        assert!(quoted_literals(" SELECT '01','PURCHASE FROM X").is_err());
    }

    /// CREADB21 LDTTYPE: DB2LTTYP [app/app-transaction-type-db2/ctl/DB2LTTYP.ctl:L17-23]
    /// (P-TRNTYPE-003, P-TRNTYPE-004).
    #[test]
    fn db2lttyp_loads_seven_upper_case_types() {
        let masters =
            parse_tran_type_masters(&source(DB2LTTYP), Path::new(DB2LTTYP)).expect("types");
        let rows: Vec<(String, &str)> = masters
            .iter()
            .map(|master| {
                (
                    master.tr_type.trimmed().into_owned(),
                    master.tr_description.as_str(),
                )
            })
            .collect();
        let expected = [
            ("01", "PURCHASE"),
            ("02", "PAYMENT"),
            ("03", "CREDIT"),
            ("04", "AUTHORIZATION"),
            ("05", "REFUND"),
            ("06", "REVERAL"),
            ("07", "ADJUSTMENT"),
        ];
        let expected: Vec<(String, &str)> = expected
            .iter()
            .map(|&(code, text)| (code.to_owned(), text))
            .collect();
        assert_eq!(rows, expected);
    }

    /// CREADB21 LDTCCAT: DB2LTCAT [app/app-transaction-type-db2/ctl/DB2LTCAT.ctl:L20-37]
    /// (P-TRNTYCAT-004, P-TRNTYCAT-005).
    #[test]
    fn db2ltcat_loads_eighteen_categories() {
        let masters = parse_tran_category_masters(&source(DB2LTCAT), Path::new(DB2LTCAT))
            .expect("categories");
        assert_eq!(masters.len(), 18);
        let first = &masters[0];
        assert_eq!(
            (
                first.trc_type_code,
                first.trc_type_category,
                first.trc_cat_data.trimmed()
            ),
            (code("01"), category(1), "REGULAR SALES DRAFT".into())
        );
        assert_eq!(first.trc_type_category.to_string(), "0001");
        let travel = &masters[13];
        assert_eq!(
            (
                travel.trc_type_code,
                travel.trc_type_category,
                travel.trc_cat_data.trimmed()
            ),
            (
                code("04"),
                category(3),
                "TRAVEL BOOKING AUTHORIZATION".into()
            )
        );
        let last = &masters[17];
        assert_eq!(
            (
                last.trc_type_code,
                last.trc_type_category,
                last.trc_cat_data.trimmed()
            ),
            (
                code("07"),
                category(1),
                "SALES DRAFT CREDIT ADJUSTMENT".into()
            )
        );
    }

    #[test]
    fn malformed_ctl_statements_name_their_line() {
        let types = [
            ("/* header */\n SELECT '01' FROM X UNION ALL\n", 2),
            (" SELECT '01','A','B' FROM X\n", 1),
            (" SELECT '012','PURCHASE' FROM X\n", 1),
            (" SELECT '01','PURCHASE FROM X\n", 1),
            ("/* header */\n COMMIT;\n", 2),
        ];
        for (text, line) in types {
            let result = parse_tran_type_masters(text, Path::new("t.ctl"));
            assert_eq!(fixture_line(result), line, "{text:?}");
        }
        let long = format!("'01','{}'\n", "D".repeat(51));
        assert_eq!(
            fixture_line(parse_tran_type_masters(&long, Path::new("t.ctl"))),
            1
        );
        let categories = [
            (" SELECT '01','0001' FROM DMY\n", 1),
            ("\n SELECT '01','1','CASH' FROM DMY\n", 2),
            (" SELECT '01','00A1','CASH' FROM DMY\n", 1),
            (" SELECT '01','00001','CASH' FROM DMY\n", 1),
        ];
        for (text, line) in categories {
            let result = parse_tran_category_masters(text, Path::new("t.ctl"));
            assert_eq!(fixture_line(result), line, "{text:?}");
        }
    }

    /// AAP 0.3.6: retail (01/0001, 1 pt/$) and travel (04/0003, 3 pts/$); E-08.
    #[test]
    fn seed_rules_are_retail_and_travel() {
        let rules = seed_rules().expect("valid seed rules");
        let from = Utc
            .with_ymd_and_hms(2022, 1, 1, 0, 0, 0)
            .single()
            .expect("an instant");
        let summary: Vec<(i64, TranTypeCode, TranCategoryCode, Decimal)> = rules
            .iter()
            .map(|(id, rule)| {
                assert_eq!(rule.active_from, from);
                assert_eq!(rule.active_to, None);
                assert_eq!(rule.min_transaction_amount, None);
                assert_eq!(rule.max_transaction_amount, None);
                (
                    id.get(),
                    rule.transaction_type_cd,
                    rule.transaction_category_cd,
                    rule.reward_rate.to_decimal(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (1, code("01"), category(1), Decimal::new(10_000, 4)),
                (2, code("04"), category(3), Decimal::new(30_000, 4)),
            ]
        );
        assert_eq!(summary[0].3.to_string(), "1.0000");
    }

    /// `Debug` of a prepared seed: the mode and, per source, its row count or
    /// the error text reading it gave, never a password or a hash.
    /// `OnlyTranextr` reads no source, so a missing one does not show.
    #[test]
    fn debug_of_a_prepared_seed_shows_the_mode_and_each_source_outcome() {
        let defaults = SeedSources::with_defaults(&repo_root());
        let missing = repo_root().join("app/data/no-such-seed-source");
        assert!(!missing.exists(), "{} must not exist", missing.display());
        let unreadable = SeedSources {
            fixture_dir: missing.clone(),
            dusrsecj_jcl: missing.join("DUSRSECJ.jcl"),
            db2_ctl_dir: missing.clone(),
        };
        let error_of = |name: &str| {
            let path = missing.join(name);
            let err = std::fs::read(&path).expect_err("a missing source");
            format!("Err(cannot read {}: {err})", path.display())
        };
        let full = "PreparedSeed { mode: Full, accounts: Ok(50), cards: Ok(50), \
                    card_xref: Ok(50), customers: Ok(50), disclosure_groups: Ok(51), \
                    tran_cat_balances: Ok(50), trantype_file: Ok(7), trancatg_file: Ok(18), \
                    users: Ok(10), transaction_types: Ok(7), transaction_categories: Ok(18) }";
        let if_empty = format!(
            "PreparedSeed {{ mode: IfEmpty, accounts: {}, cards: {}, card_xref: {}, \
             customers: {}, disclosure_groups: {}, tran_cat_balances: {}, \
             trantype_file: {}, trancatg_file: {}, users: {}, transaction_types: {}, \
             transaction_categories: {} }}",
            error_of("acctdata.txt"),
            error_of("carddata.txt"),
            error_of("cardxref.txt"),
            error_of("custdata.txt"),
            error_of("discgrp.txt"),
            error_of("tcatbal.txt"),
            error_of("trantype.txt"),
            error_of("trancatg.txt"),
            error_of("DUSRSECJ.jcl"),
            error_of("DB2LTTYP.ctl"),
            error_of("DB2LTCAT.ctl"),
        );
        let cases = [
            (SeedMode::Full, &defaults, full.to_owned()),
            (SeedMode::IfEmpty, &unreadable, if_empty),
            (
                SeedMode::OnlyTranextr,
                &unreadable,
                "PreparedSeed { mode: OnlyTranextr }".to_owned(),
            ),
        ];
        for (mode, sources, expected) in cases {
            let rendered = format!("{:?}", prepare(mode, sources));
            assert_eq!(rendered, expected, "{mode:?}");
            for hidden in ["PASSWORD", "argon2"] {
                assert!(!rendered.contains(hidden), "{hidden} in {rendered}");
            }
        }
    }

    /// DUSRSECJ SYSUT1 (P-CSUSR01Y-004, E-20): each prepared user keeps its
    /// stored password only as an argon2id hash of the 8 bytes as stored, and
    /// `Debug` of a user shows its ID only.
    #[test]
    fn prepared_users_keep_only_the_hash_and_debug_shows_the_user_id() {
        let users = prepare_users(Path::new(DUSRSECJ)).expect("the DUSRSECJ users");
        assert!(users.failure.is_none());
        let ids: Vec<&str> = users
            .rows
            .iter()
            .map(|user| user.user_id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005", "USER0001", "USER0002",
                "USER0003", "USER0004", "USER0005"
            ]
        );
        for user in &users.rows {
            assert!(
                user.password_hash
                    .starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
                "{user:?}"
            );
            let rendered = format!("{user:?}");
            assert_eq!(
                rendered,
                format!("SeedUser {{ user_id: {:?}, .. }}", user.user_id)
            );
            assert!(!rendered.contains(&user.password_hash));
        }
        let first = &users.rows[0];
        assert_eq!(
            (first.first_name.as_str(), first.last_name.as_str()),
            ("MARGARET", "GOLD")
        );
        assert_eq!(first.usr_type, "A");
        assert!(
            verify_as_entered("PASSWORD", &first.password_hash).expect("a parseable argon2id hash")
        );
    }

    #[test]
    fn a_user_row_that_does_not_decode_names_its_record() {
        let err = prepare_user(Path::new("t.jcl"), 4, &[b' '; 79]).expect_err("a 79-byte row");
        let PersistenceError::Codec { path, source } = err else {
            panic!("expected a Codec error, got {err:?}");
        };
        assert_eq!(path, Path::new("t.jcl"));
        assert_eq!(source.record, Some(4));
        assert_eq!(
            source.kind,
            CodecErrorKind::Length {
                expected: UserSecurityLayout::LEN,
                actual: 79
            }
        );
    }
}
