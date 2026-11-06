//! Command line of the `batch` binary: one subcommand per `CardDemo` JCL job that
//! carries business logic, with the job's own parameters as defaults.
//!
//! Every default is the value the JCL passes, so a bare `batch <subcommand>`
//! runs the job as the repository submits it:
//!
//! | Subcommand | JCL source of the defaults |
//! |---|---|
//! | `posttran --input` | `DALYTRAN DD` [app/jcl/POSTTRAN.jcl:L30-31], as its fixture copy |
//! | `intcalc --parm-date` | `PARM='2022071800'` [app/jcl/INTCALC.jcl:L22] (P-CBACT04C-001) |
//! | `tranrept --start-date`, `--end-date` | `SYMNAMES` `PARM-START-DATE`, `PARM-END-DATE` [app/jcl/TRANREPT.jcl:L43-44] [app/proc/TRANREPT.prc:L41-42] (P-TRANREPT-003, E-34) |
//! | `cbpaup0j --sysin` | `SYSIN` card [app/app-authorization-ims-db2-mq/jcl/CBPAUP0J.jcl:L36-37] (P-CBPAUP0C-001) |
//!
//! `mnttrdb2 --input` has no default: `MNTTRDB2` allocates `INPFILE` from a
//! dataset the repository does not hold [app/app-transaction-type-db2/jcl/MNTTRDB2.jcl:L27].
//!
//! This module only shapes and checks the text of the arguments. The jobs
//! parse their contents (the `SYSIN` card through
//! `carddemo_domain::rules::auth_purge::parse_sysin`, the `PARM` into an
//! `Alnum<10>`), and `carddemo_runtime::config::Settings` stays the only
//! reader of the environment, so no argument here has an `env` fallback.

use std::path::PathBuf;

use carddemo_persistence::seed::SeedMode;
use clap::{Args, Parser, Subcommand, ValueEnum};

/// Length of CBACT04C `PARM-DATE X(10)` [app/cbl/CBACT04C.cbl:L178].
const PARM_DATE_LEN: usize = 10;

/// Length of a `YYYY-MM-DD` date, the `TRAN-PROC-DT` text the `TRANREPT`
/// sort compares.
const ISO_DATE_LEN: usize = 10;

/// Byte offsets of the two `-` separators in a `YYYY-MM-DD` date.
const ISO_DATE_SEPARATORS: [usize; 2] = [4, 7];

/// The text `batch -h` and `batch --help` show after the subcommands: the settings the jobs
/// read with their defaults, where the output goes, the exit status legend and examples. Every
/// name and default equals `carddemo_runtime::config::{keys, defaults}`, which the tests check.
const ROOT_AFTER_HELP: &str = "\
Environment (KEY=default; .env.example lists every setting):
  CARDDEMO_DATABASE_URL=postgres://carddemo:carddemo@localhost:5432/carddemo
      The PostgreSQL database every subcommand connects to.
  CARDDEMO_BATCH_DIR=./var/batch/
      The directory of the generation files the jobs write and read.
  CARDDEMO_STATEMENT_DIR=./var/statements/
      The directory creastmt writes the statements to.
  CARDDEMO_FIXTURE_DIR=app/data/ASCII
      The ASCII fixture directory seed loads.
  CARDDEMO_CLOCK_OVERRIDE (unset: the host clock)
      An RFC 3339 instant, such as 2022-07-01T12:00:00Z, that pins the processing clock.
  RUST_LOG=info
      The filter of the JSON log lines.
  A relative directory resolves against the current directory. Every CARDDEMO_* setting is
  read and checked at start-up, the api and mq settings included (such as CARDDEMO_HTTP_ADDR
  and CARDDEMO_JWT_TTL_SECS). An unusable value, an invalid CARDDEMO_CLOCK_OVERRIDE or
  RUST_LOG included, is a setup failure: exit 16.

Output:
  stdout carries only JSON telemetry lines. stderr carries only the lines that report an
  abend, a reward-engine halt, a capacity stop, an equivalence halt or a setup failure (and
  one line should the telemetry shutdown fail).
  A generation is the file <DATASET>.G<nnnn>V00 under CARDDEMO_BATCH_DIR, such as
  DALYREJS.G0001V00: (+1) creates the next number, (0) reads the newest, and only the newest
  5 generations of each dataset are kept.

Exit status:
  0   Success.
  2   Usage error, reported on stderr; nothing runs.
  4   Records rejected: posttran rejects, mnttrdb2 failed records.
  8   posttran reward-engine halt.
  16  Abend: the DISPLAY lines of the COBOL abend path on stderr (with the file status where
      the COBOL displays one), then ABEND 999 where the COBOL passes that code (posttran,
      intcalc, tranrept). Also a creastmt capacity stop, a combtran equivalence halt and every
      setup failure.
  The IDCAMS REPRO condition code, with no other meaning: tranbkp and tranrept end with 12
  on an empty live transaction master, combtran with 8 or 12 after duplicate keys.

Examples:
  cargo run --bin batch -- migrate
  cargo run --bin batch -- seed --if-empty
  cargo run --bin batch -- posttran
  CARDDEMO_CLOCK_OVERRIDE=2022-07-01T12:00:00Z cargo run --bin batch -- posttran
  cargo run --bin batch -- <subcommand> --help";

/// The `batch` command line.
#[derive(Debug, Parser)]
#[command(
    name = "batch",
    version,
    about = "CardDemo batch jobs (JCL replacement)",
    long_about = None,
    after_help = ROOT_AFTER_HELP
)]
pub(crate) struct Cli {
    /// The job to run.
    #[command(subcommand)]
    pub(crate) command: Command,
}

/// One JCL job each. Every name is written out, so the digits in `cbpaup0j`
/// and `mnttrdb2` never depend on clap's case conversion.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// CREADB21 (DSNTEP4): create the database schema by applying the migrations.
    ///
    /// Replaces the table and index definitions of CREADB21; its initial loads
    /// of the Db2 transaction type and category tables run in the seed subcommand.
    #[command(name = "migrate", after_help = MIGRATE_AFTER_HELP)]
    Migrate,

    /// ACCTFILE, CARDFILE, CUSTFILE, XREFFILE, DISCGRP, TCATBALF, TRANTYPE,
    /// TRANCATG, DUSRSECJ (IDCAMS, IEBGENER): load or refresh the master files;
    /// TRANEXTR (DSNTIAUL) with --only tranextr.
    ///
    /// Without options the run is the refresh: every master file and
    /// processing copy is reloaded from the fixture corpus in one database
    /// transaction.
    #[command(name = "seed", after_help = SEED_AFTER_HELP)]
    Seed(SeedArgs),

    /// POSTTRAN (CBTRN02C): post the daily transactions and write the rejects.
    #[command(name = "posttran", after_help = POSTTRAN_AFTER_HELP)]
    Posttran(PosttranArgs),

    /// INTCALC (CBACT04C): compute the monthly interest and write the system
    /// transactions.
    #[command(name = "intcalc", after_help = INTCALC_AFTER_HELP)]
    Intcalc(IntcalcArgs),

    /// TRANBKP (REPROC, IDCAMS DELETE/DEFINE): back up the transaction master,
    /// then empty it.
    #[command(name = "tranbkp", after_help = TRANBKP_AFTER_HELP)]
    Tranbkp,

    /// COMBTRAN (SORT, IDCAMS REPRO): combine the backup and the system
    /// transactions and reload the transaction master.
    #[command(name = "combtran", after_help = COMBTRAN_AFTER_HELP)]
    Combtran,

    /// CREASTMT (CBSTM03A, CBSTM03B): produce the account statements as text,
    /// HTML and PDF.
    #[command(name = "creastmt", after_help = CREASTMT_AFTER_HELP)]
    Creastmt,

    /// TRANREPT (REPROC, SORT, CBTRN03C): back up the transaction master and
    /// report the transactions processed within a date range.
    #[command(name = "tranrept", after_help = TRANREPT_AFTER_HELP)]
    Tranrept(TranreptArgs),

    /// CBPAUP0J (CBPAUP0C): purge the expired pending authorizations.
    #[command(name = "cbpaup0j", after_help = CBPAUP0J_AFTER_HELP)]
    Cbpaup0j(Cbpaup0jArgs),

    /// MNTTRDB2 (COBTUPDT): add, update and delete Db2 transaction types from
    /// an input file.
    #[command(name = "mnttrdb2", after_help = MNTTRDB2_AFTER_HELP)]
    Mnttrdb2(Mnttrdb2Args),
}

// The text each `batch <subcommand> -h` and `--help` shows after the options: what the job
// reads, what it writes and where, the exit codes it can end with, and an example.

/// After-help of `batch migrate`.
const MIGRATE_AFTER_HELP: &str = "\
Reads:
  The database at CARDDEMO_DATABASE_URL, and the migrations of migrations/ built into the
  binary.

Writes:
  The database schema: every migration the database has not applied, in version order. On a
  migrated database nothing changes. No rows are loaded; seed loads them.

Exit status:
  0   The schema is current, applied by this run or already.
  16  Setup failure: the database is unreachable or a migration failed. The versions applied
      before the failing one stay applied.

Example:
  cargo run --bin batch -- migrate";

/// After-help of `batch seed`.
const SEED_AFTER_HELP: &str = "\
Reads, a relative path against the current directory (the repository root):
  CARDDEMO_FIXTURE_DIR: acctdata.txt, carddata.txt, cardxref.txt, custdata.txt, discgrp.txt,
      tcatbal.txt, trantype.txt and trancatg.txt.
  app/jcl/DUSRSECJ.jcl: the 10 SYSUT1 user rows; each password is stored as an argon2id hash.
  app/app-transaction-type-db2/ctl/DB2LTTYP.ctl and DB2LTCAT.ctl: the Db2 master rows.
  The database at CARDDEMO_DATABASE_URL. --only tranextr reads no file.

Writes, in one database transaction that a failure rolls back whole:
  no option         Loads, or refreshes when rows exist: clears the reward accruals, balances
                    and rules, reloads every master file and processing copy, re-inserts the
                    seed reward rules, and loads the Db2 masters transaction_types and
                    transaction_categories only while both are empty. The transaction
                    master keeps its rows.
  --if-empty        Does nothing when accounts has rows; otherwise loads as above.
  --only tranextr   Copies the Db2 masters over the processing copies trantype_file and
                    trancatg_file. Nothing else changes.

Exit status:
  0   Done, an --if-empty run that found rows included.
  16  Setup failure: a source that cannot be read or decoded, or a failed statement.

Examples:
  cargo run --bin batch -- seed --if-empty
  cargo run --bin batch -- seed
  cargo run --bin batch -- seed --only tranextr";

/// After-help of `batch posttran`.
const POSTTRAN_AFTER_HELP: &str = "\
Reads:
  --input: the DALYTRAN daily transactions, 350-byte CVTRA06Y records.
  The card cross-reference, accounts, category balances and reward rules at
  CARDDEMO_DATABASE_URL. The live transaction master must be empty (run tranbkp first);
  otherwise the run abends with exit 16.
  CARDDEMO_CLOCK_OVERRIDE pins TRAN-PROC-TS and the accrual time of every posted record.

Writes:
  DALYREJS(+1): DALYREJS.G<nnnn>V00 under CARDDEMO_BATCH_DIR, one 430-byte record per reject
      (the 350-byte input record and an 80-byte reason trailer), each followed by LF. It is
      cataloged only when the run reads every record, and is empty when none is rejected.
  Each posted record, in its own database transaction: the transaction into the live master,
      the account and category balance updates, and its reward accrual and balance.

Exit status:
  0   No record rejected.
  4   Records rejected.
  8   Reward-engine halt: the record number and TRAN-ID on stderr. That record is not
      posted; the records before it stay posted.
  16  Abend: the DISPLAY lines and ABEND 999 on stderr. Or a setup failure, such as an input
      record that does not decode.

Examples:
  cargo run --bin batch -- posttran
  cargo run --bin batch -- posttran --input /data/daily.txt
  CARDDEMO_CLOCK_OVERRIDE=2022-07-01T12:00:00Z cargo run --bin batch -- posttran";

/// After-help of `batch intcalc`.
const INTCALC_AFTER_HELP: &str = "\
Reads:
  --parm-date: the PARM, which prefixes the TRAN-ID of every interest transaction.
  The category balances, card cross-reference, disclosure groups and accounts at
  CARDDEMO_DATABASE_URL.
  CARDDEMO_CLOCK_OVERRIDE pins the timestamps of the interest transactions.

Writes:
  SYSTRAN(+1): SYSTRAN.G<nnnn>V00 under CARDDEMO_BATCH_DIR, the interest transactions as
      350-byte CVTRA05Y records, each followed by LF. combtran loads them into the live
      transaction master.
  The account updates: the interest added to the current balance and the cycle credit and
      debit set to zero, for every account but the last, which CBACT04C never rewrites.

Exit status:
  0   Done.
  16  Abend: the DISPLAY lines and ABEND 999 on stderr. Or a setup failure. SYSTRAN(+1) is
      then not cataloged.

Example:
  cargo run --bin batch -- intcalc --parm-date 2022071800";

/// After-help of `batch tranbkp`.
const TRANBKP_AFTER_HELP: &str = "\
Reads:
  The live transaction master at CARDDEMO_DATABASE_URL.

Writes:
  TRANSACT.BKUP(+1): TRANSACT.BKUP.G<nnnn>V00 under CARDDEMO_BATCH_DIR, every live
      transaction in TRAN-ID order as a 350-byte CVTRA05Y record, each followed by LF.
  Then empties the live transaction master. Transactions that reward accruals reference stay
      stored outside it, so the reward history is kept.

Exit status:
  0   The live master held transactions.
  12  IDCAMS REPRO condition code: the live master was empty. The empty backup is cataloged.
  16  Setup failure. No transaction leaves the live master without its backup.

Example:
  cargo run --bin batch -- tranbkp";

/// After-help of `batch combtran`.
const COMBTRAN_AFTER_HELP: &str = "\
Reads:
  TRANSACT.BKUP(0) and SYSTRAN(0): the newest TRANSACT.BKUP.G<nnnn>V00 and SYSTRAN.G<nnnn>V00
      under CARDDEMO_BATCH_DIR.
  The live transaction master at CARDDEMO_DATABASE_URL.

Writes:
  TRANSACT.COMBINED(+1): TRANSACT.COMBINED.G<nnnn>V00 under CARDDEMO_BATCH_DIR, both inputs
      sorted by TRAN-ID, 350-byte records, each followed by LF.
  The reload of the live transaction master from it, in one database transaction. A TRAN-ID
      already live is not loaded and keeps its row (REPRO without REPLACE).

Exit status:
  0   Every record loaded.
  8   IDCAMS REPRO condition code after one to three duplicate keys (not a reward halt).
  12  IDCAMS REPRO condition code: REPRO stopped at its error limit of 4 duplicate keys.
  16  Equivalence halt: two different records with one TRAN-ID; nothing is written. Or a
      setup failure, such as a missing (0) generation.

Example:
  cargo run --bin batch -- combtran";

/// After-help of `batch creastmt`.
const CREASTMT_AFTER_HELP: &str = "\
Reads:
  The live transaction master, sorted by card number then TRAN-ID, the card cross-reference,
  the customers and the accounts at CARDDEMO_DATABASE_URL.

Writes, under CARDDEMO_STATEMENT_DIR, replacing the previous statements:
  STATEMNT.PS       The text statements, 80-byte records, each followed by LF.
  STATEMNT.HTML     The HTML statements, 100-byte records, each followed by LF.
  STATEMNT.PS.PDF   STATEMNT.PS as PDF, one text line per record.
  The run holds the lock file .STATEMNT.lock there; a run that finds it exits 16 and
  changes nothing.

Exit status:
  0   Statements written.
  16  Abend: a read failed or the transaction master is empty; the DISPLAY lines on stderr,
      with no ABEND code. Or a capacity stop: more than 51 cards or 10 transactions per
      card, CAPACITY STOP on stderr. Or a setup failure.

Example:
  cargo run --bin batch -- creastmt";

/// After-help of `batch tranrept`.
const TRANREPT_AFTER_HELP: &str = "\
Reads:
  --start-date, --end-date: the range of processing dates reported.
  The live transaction master, the card cross-reference and the processing copies
  trantype_file and trancatg_file at CARDDEMO_DATABASE_URL; never the Db2 masters, so an
  edited transaction type is reported only after seed --only tranextr.

Writes, under CARDDEMO_BATCH_DIR, each record followed by LF:
  TRANSACT.BKUP(+1)   TRANSACT.BKUP.G<nnnn>V00, the live master, 350-byte records.
  TRANSACT.DALY(+1)   TRANSACT.DALY.G<nnnn>V00, the records processed within the range,
                      sorted by card number, 350-byte records.
  TRANREPT(+1)        TRANREPT.G<nnnn>V00, the transaction report, 133-byte lines.

Exit status:
  0   Done.
  12  IDCAMS REPRO condition code: the live master was empty. Every step still runs.
  16  Abend: the CBTRN03C DISPLAY lines and ABEND 999 on stderr. Or a SORT stop on a card
      number it cannot place, or a setup failure.

Example:
  cargo run --bin batch -- tranrept --start-date 2022-01-01 --end-date 2022-07-06";

/// After-help of `batch cbpaup0j`.
const CBPAUP0J_AFTER_HELP: &str = "\
Reads:
  --sysin: the SYSIN card, with the expiry days, checkpoint frequency, checkpoint display
      frequency and debug flag.
  The pending authorization summaries and details at CARDDEMO_DATABASE_URL.
  CARDDEMO_CLOCK_OVERRIDE pins the purge day (ACCEPT CURRENT-YYDDD FROM DAY).

Writes:
  The deletion of the expired pending authorizations, in one database transaction.

Exit status:
  0   Done.
  16  Abend: the CBPAUP0C DISPLAY text on stderr, with no ABEND code; the deletions made
      before it are committed. Or a setup failure.

Example:
  cargo run --bin batch -- cbpaup0j --sysin 00,00001,00001,Y";

/// After-help of `batch mnttrdb2`.
const MNTTRDB2_AFTER_HELP: &str = "\
Reads:
  --input: 53-byte records, each a type A (add), U (update), D (delete) or * (comment), a
      2-byte type code and a 50-byte description.
  The Db2 master transaction_types at CARDDEMO_DATABASE_URL.

Writes:
  Changes to the Db2 master transaction_types only, in one database transaction. The
  processing copy trantype_file the reports read follows only after seed --only tranextr.

Exit status:
  0   No record failed.
  4   Records failed: each is logged and changes nothing; the others are committed.
  16  Setup failure: the input cannot be read or framed (nothing changes), or the commit
      failed (every change is undone).

Example:
  cargo run --bin batch -- mnttrdb2 --input trantype.upd";

/// Arguments of `batch seed`.
#[derive(Debug, Args)]
pub(crate) struct SeedArgs {
    /// Do nothing when the account master already holds rows (first start).
    #[arg(long, conflicts_with = "only")]
    pub(crate) if_empty: bool,

    /// Run only the named job instead of the refresh.
    #[arg(long, value_enum)]
    pub(crate) only: Option<SeedOnly>,
}

impl SeedArgs {
    /// The seed run these options select. `--only tranextr` is the TRANEXTR
    /// chain, `--if-empty` the first-start seed, and no option the refresh
    /// that replaces the README refresh sequence (AAP 0.3.2). clap rejects the
    /// two options together, so their order here decides nothing.
    pub(crate) fn mode(&self) -> SeedMode {
        match (self.only, self.if_empty) {
            (Some(SeedOnly::Tranextr), _) => SeedMode::OnlyTranextr,
            (None, true) => SeedMode::IfEmpty,
            (None, false) => SeedMode::Full,
        }
    }
}

/// The single job `batch seed --only` can run.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum SeedOnly {
    /// TRANEXTR (DSNTIAUL): copy the Db2 transaction type and category masters
    /// over the TRANTYPE and TRANCATG processing copies the reports read.
    #[value(name = "tranextr")]
    Tranextr,
}

/// Arguments of `batch posttran`.
#[derive(Debug, Args)]
pub(crate) struct PosttranArgs {
    /// The DALYTRAN input: 350-byte daily transaction records (CVTRA06Y).
    /// The default is the fixture copy of the dataset POSTTRAN reads; a
    /// relative path resolves against the current directory.
    #[arg(
        long,
        value_name = "FILE",
        default_value = "app/data/ASCII/dailytran.txt"
    )]
    pub(crate) input: PathBuf,
}

/// Arguments of `batch intcalc`.
#[derive(Debug, Args)]
pub(crate) struct IntcalcArgs {
    /// The JCL PARM that CBACT04C receives as PARM-DATE X(10): exactly 10
    /// bytes. It prefixes the TRAN-ID of every interest transaction.
    #[arg(
        long,
        value_name = "PARM",
        default_value = "2022071800",
        value_parser = parse_parm_date
    )]
    pub(crate) parm_date: String,
}

/// Arguments of `batch tranrept`.
#[derive(Debug, Args)]
pub(crate) struct TranreptArgs {
    /// First processing date reported, PARM-START-DATE. Compared as text, as
    /// the SORT INCLUDE compares it, and written to the report header.
    #[arg(
        long,
        value_name = "YYYY-MM-DD",
        default_value = "2022-01-01",
        value_parser = parse_iso_date
    )]
    pub(crate) start_date: String,

    /// Last processing date reported, PARM-END-DATE. Compared as text, as the
    /// SORT INCLUDE compares it, and written to the report header.
    #[arg(
        long,
        value_name = "YYYY-MM-DD",
        default_value = "2022-07-06",
        value_parser = parse_iso_date
    )]
    pub(crate) end_date: String,
}

/// Arguments of `batch cbpaup0j`.
#[derive(Debug, Args)]
pub(crate) struct Cbpaup0jArgs {
    /// The SYSIN card CBPAUP0C accepts into PRM-INFO: expiry days,
    /// checkpoint frequency, checkpoint display frequency and debug flag,
    /// separated by commas.
    #[arg(long, value_name = "CARD", default_value = "00,00001,00001,Y")]
    pub(crate) sysin: String,
}

/// Arguments of `batch mnttrdb2`.
#[derive(Debug, Args)]
pub(crate) struct Mnttrdb2Args {
    /// The INPFILE input: 53-byte records of a type X(1) (A add, U update,
    /// D delete, * comment), a type code X(2) and a description X(50), the
    /// COBTUPDT WS-INPUT-REC layout.
    #[arg(long, value_name = "FILE")]
    pub(crate) input: PathBuf,
}

/// CBACT04C `PROCEDURE DIVISION USING EXTERNAL-PARMS` — P-CBACT04C-001.
///
/// Accepts the `PARM` text only when it fills `PARM-DATE X(10)` exactly
/// [app/cbl/CBACT04C.cbl:L175-180]. A shorter `PARM` leaves part of
/// `PARM-DATE` undefined, an input class whose result cannot be pinned (AAP
/// 0.9.6), and a longer one does not fit the field, so both are usage errors.
fn parse_parm_date(text: &str) -> Result<String, String> {
    if text.len() == PARM_DATE_LEN {
        Ok(text.to_owned())
    } else {
        Err(format!(
            "expected exactly {PARM_DATE_LEN} bytes (CBACT04C PARM-DATE X({PARM_DATE_LEN})), got {}",
            text.len()
        ))
    }
}

/// `TRANREPT` `SYMNAMES` `PARM-START-DATE`/`PARM-END-DATE` — P-TRANREPT-003.
///
/// Accepts a `YYYY-MM-DD` shape: ten ASCII bytes, digits except for `-` at
/// offsets 4 and 7. No calendar check is made: the sort's `INCLUDE COND`
/// compares `TRAN-PROC-DT` with the date as `CH` text
/// [app/jcl/TRANREPT.jcl:L47-48], so `2022-13-45` is a valid bound there.
fn parse_iso_date(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let shaped = bytes.len() == ISO_DATE_LEN
        && bytes.iter().enumerate().all(|(offset, &byte)| {
            if ISO_DATE_SEPARATORS.contains(&offset) {
                byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        });
    if shaped {
        Ok(text.to_owned())
    } else {
        Err(String::from(
            "expected a YYYY-MM-DD date: 10 characters, digits with '-' as the 5th and 8th",
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use carddemo_persistence::seed::SeedMode;
    use carddemo_runtime::config::{defaults, keys};
    use clap::error::ErrorKind;
    use clap::{CommandFactory, Parser};

    use super::{Cli, Command};

    /// `batch` followed by `args`, parsed.
    fn try_parse(args: &[&str]) -> Result<Command, clap::Error> {
        Cli::try_parse_from(std::iter::once("batch").chain(args.iter().copied()))
            .map(|cli| cli.command)
    }

    fn parse(args: &[&str]) -> Command {
        try_parse(args).unwrap_or_else(|e| panic!("{args:?} must parse: {e}"))
    }

    /// The kind of the error `args` must raise; clap exits 2 for each.
    fn rejection(args: &[&str]) -> ErrorKind {
        let error = try_parse(args).expect_err(&format!("{args:?} must be rejected"));
        assert_eq!(error.exit_code(), 2, "{args:?}");
        error.kind()
    }

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    /// Whether a parsed command holds the values a case expects.
    type Expectation = fn(&Command) -> bool;

    #[test]
    fn every_subcommand_without_options_takes_the_jcl_defaults() {
        let cases: [(&str, Expectation); 9] = [
            ("migrate", |c| matches!(c, Command::Migrate)),
            (
                "seed",
                |c| matches!(c, Command::Seed(a) if a.mode() == SeedMode::Full),
            ),
            (
                "posttran",
                |c| matches!(c, Command::Posttran(a) if a.input == Path::new("app/data/ASCII/dailytran.txt")),
            ),
            (
                "intcalc",
                |c| matches!(c, Command::Intcalc(a) if a.parm_date == "2022071800"),
            ),
            ("tranbkp", |c| matches!(c, Command::Tranbkp)),
            ("combtran", |c| matches!(c, Command::Combtran)),
            ("creastmt", |c| matches!(c, Command::Creastmt)),
            ("tranrept", |c| {
                matches!(c, Command::Tranrept(a)
                    if a.start_date == "2022-01-01" && a.end_date == "2022-07-06")
            }),
            (
                "cbpaup0j",
                |c| matches!(c, Command::Cbpaup0j(a) if a.sysin == "00,00001,00001,Y"),
            ),
        ];
        for (name, holds_defaults) in cases {
            let command = parse(&[name]);
            assert!(holds_defaults(&command), "{name}: {command:?}");
        }
    }

    #[test]
    fn mnttrdb2_requires_its_input_file() {
        assert_eq!(rejection(&["mnttrdb2"]), ErrorKind::MissingRequiredArgument);
        let Command::Mnttrdb2(args) = parse(&["mnttrdb2", "--input", "trantype.upd"]) else {
            panic!("mnttrdb2 must parse as Command::Mnttrdb2");
        };
        assert_eq!(args.input, Path::new("trantype.upd"));
    }

    #[test]
    fn explicit_values_replace_the_defaults() {
        let Command::Posttran(posttran) = parse(&["posttran", "--input", "/data/daily.txt"]) else {
            panic!("posttran must parse as Command::Posttran");
        };
        assert_eq!(posttran.input, Path::new("/data/daily.txt"));

        let Command::Cbpaup0j(purge) = parse(&["cbpaup0j", "--sysin", "05,00010,00020,N"]) else {
            panic!("cbpaup0j must parse as Command::Cbpaup0j");
        };
        assert_eq!(purge.sysin, "05,00010,00020,N");
    }

    #[test]
    fn seed_options_select_the_mode() {
        let cases: [(&[&str], SeedMode); 3] = [
            (&["seed"], SeedMode::Full),
            (&["seed", "--if-empty"], SeedMode::IfEmpty),
            (&["seed", "--only", "tranextr"], SeedMode::OnlyTranextr),
        ];
        for (args, mode) in cases {
            let Command::Seed(seed) = parse(args) else {
                panic!("{args:?} must parse as Command::Seed");
            };
            assert_eq!(seed.mode(), mode, "{args:?}");
        }
    }

    #[test]
    fn seed_rejects_conflicting_or_unknown_options() {
        let cases: [(&[&str], ErrorKind); 2] = [
            (
                &["seed", "--if-empty", "--only", "tranextr"],
                ErrorKind::ArgumentConflict,
            ),
            (&["seed", "--only", "acctfile"], ErrorKind::InvalidValue),
        ];
        for (args, kind) in cases {
            assert_eq!(rejection(args), kind, "{args:?}");
        }
    }

    #[test]
    fn intcalc_parm_date_must_fill_parm_date_exactly() {
        for value in ["202207180", "20220718000", ""] {
            assert_eq!(
                rejection(&["intcalc", "--parm-date", value]),
                ErrorKind::ValueValidation,
                "{value:?}"
            );
        }
        let Command::Intcalc(args) = parse(&["intcalc", "--parm-date", "2023010100"]) else {
            panic!("intcalc must parse as Command::Intcalc");
        };
        assert_eq!(args.parm_date, "2023010100");
    }

    #[test]
    fn tranrept_dates_must_have_the_iso_shape() {
        // "2022-01-\u{660}" is 10 bytes whose last character, ARABIC-INDIC DIGIT
        // ZERO, is numeric but not an ASCII digit.
        for option in ["--start-date", "--end-date"] {
            for value in [
                "2022/01/01",
                "2022-1-01",
                "2022-01-011",
                "2022-0a-01",
                "2022-01-\u{660}",
            ] {
                assert_eq!(
                    rejection(&["tranrept", option, value]),
                    ErrorKind::ValueValidation,
                    "{option} {value:?}"
                );
            }
        }
        let Command::Tranrept(args) = parse(&[
            "tranrept",
            "--start-date",
            "2022-13-45",
            "--end-date",
            "2022-00-00",
        ]) else {
            panic!("tranrept must parse as Command::Tranrept");
        };
        assert_eq!(args.start_date, "2022-13-45");
        assert_eq!(args.end_date, "2022-00-00");
    }

    #[test]
    fn unknown_or_missing_subcommands_are_rejected() {
        assert_eq!(rejection(&["cbtrn01c"]), ErrorKind::InvalidSubcommand);
        assert_eq!(rejection(&["CBPAUP0J"]), ErrorKind::InvalidSubcommand);
        assert!(try_parse(&[]).is_err());
    }

    /// The short (`-h`) and the long (`--help`) help of `command`, in that order.
    fn both_helps(command: &mut clap::Command) -> [String; 2] {
        [
            command.render_help().to_string(),
            command.render_long_help().to_string(),
        ]
    }

    /// The settings the root help lists, each with its default; `None` has no default.
    const ROOT_SETTINGS: [(&str, Option<&str>); 6] = [
        (keys::DATABASE_URL, Some(defaults::DATABASE_URL)),
        (keys::BATCH_DIR, Some(defaults::BATCH_DIR)),
        (keys::STATEMENT_DIR, Some(defaults::STATEMENT_DIR)),
        (keys::FIXTURE_DIR, Some(defaults::FIXTURE_DIR)),
        (keys::CLOCK_OVERRIDE, None),
        (keys::RUST_LOG, Some(defaults::RUST_LOG)),
    ];

    /// The output, exit status and example lines the root help holds.
    const ROOT_TEXTS: [&str; 19] = [
        ".env.example lists every setting",
        "Every CARDDEMO_* setting is\n  read and checked at start-up",
        "is a setup failure: exit 16",
        "stdout carries only JSON telemetry lines",
        "<DATASET>.G<nnnn>V00 under CARDDEMO_BATCH_DIR",
        "(+1) creates the next number, (0) reads the newest",
        "only the newest\n  5 generations",
        "\n  0   Success.\n",
        "\n  2   Usage error",
        "\n  4   Records rejected: posttran rejects, mnttrdb2 failed records.\n",
        "\n  8   posttran reward-engine halt.\n",
        "\n  16  Abend",
        "ABEND 999",
        "tranbkp and tranrept end with 12",
        "combtran with 8 or 12 after duplicate keys",
        "\n  cargo run --bin batch -- migrate\n",
        "\n  cargo run --bin batch -- seed --if-empty\n",
        "\n  CARDDEMO_CLOCK_OVERRIDE=2022-07-01T12:00:00Z cargo run --bin batch -- posttran\n",
        "\n  cargo run --bin batch -- <subcommand> --help",
    ];

    #[test]
    fn root_help_names_the_settings_outputs_exit_codes_and_examples() {
        let mut command = Cli::command();
        command.build();
        for help in both_helps(&mut command) {
            for (key, default) in ROOT_SETTINGS {
                let entry = default.map_or_else(
                    || format!("\n  {key} (unset: the host clock)\n"),
                    |value| format!("\n  {key}={value}\n"),
                );
                assert!(help.contains(&entry), "root help lacks {entry:?}:\n{help}");
            }
            for text in ROOT_TEXTS {
                assert!(help.contains(text), "root help lacks {text:?}:\n{help}");
            }
        }
    }

    /// The settings a subcommand help names only when its job reads them. Every job opens
    /// the database, so every subcommand help names `CARDDEMO_DATABASE_URL` as well.
    const JOB_SETTINGS: [&str; 4] = [
        keys::BATCH_DIR,
        keys::STATEMENT_DIR,
        keys::FIXTURE_DIR,
        keys::CLOCK_OVERRIDE,
    ];

    /// Per subcommand: the [`JOB_SETTINGS`] its job reads, and the texts its help holds: the
    /// JCL job, the defaults, the inputs and outputs, the exit codes and an example.
    const SUBCOMMAND_HELP: [(&str, &[&str], &[&str]); 10] = [
        (
            "migrate",
            &[],
            &[
                "CREADB21 (DSNTEP4)",
                "the migrations of migrations/",
                "every migration the database has not applied",
                "\n  0   The schema is current",
                "\n  16  Setup failure",
                "\n  cargo run --bin batch -- migrate",
            ],
        ),
        (
            "seed",
            &[keys::FIXTURE_DIR],
            &[
                "DUSRSECJ",
                "TRANEXTR (DSNTIAUL)",
                "--if-empty",
                "--only",
                "a relative path against the current directory",
                "CARDDEMO_FIXTURE_DIR: acctdata.txt",
                "app/jcl/DUSRSECJ.jcl",
                "app/app-transaction-type-db2/ctl/DB2LTTYP.ctl and DB2LTCAT.ctl",
                "refreshes when rows exist",
                "Does nothing when accounts has rows",
                "Copies the Db2 masters over the processing copies",
                "\n  0   Done",
                "\n  16  Setup failure",
                "\n  cargo run --bin batch -- seed --if-empty\n",
                "\n  cargo run --bin batch -- seed --only tranextr",
            ],
        ),
        (
            "posttran",
            &[keys::BATCH_DIR, keys::CLOCK_OVERRIDE],
            &[
                "POSTTRAN (CBTRN02C)",
                "[default: app/data/ASCII/dailytran.txt]",
                "DALYREJS(+1): DALYREJS.G<nnnn>V00 under CARDDEMO_BATCH_DIR",
                "one 430-byte record per reject",
                "reward accrual",
                "TRAN-PROC-TS",
                "\n  0   No record rejected",
                "\n  4   Records rejected",
                "\n  8   Reward-engine halt",
                "\n  16  Abend",
                "ABEND 999",
                "\n  cargo run --bin batch -- posttran\n",
            ],
        ),
        (
            "intcalc",
            &[keys::BATCH_DIR, keys::CLOCK_OVERRIDE],
            &[
                "INTCALC (CBACT04C)",
                "[default: 2022071800]",
                "SYSTRAN(+1): SYSTRAN.G<nnnn>V00 under CARDDEMO_BATCH_DIR",
                "The account updates",
                "\n  0   Done",
                "\n  16  Abend",
                "ABEND 999",
                "\n  cargo run --bin batch -- intcalc --parm-date 2022071800",
            ],
        ),
        (
            "tranbkp",
            &[keys::BATCH_DIR],
            &[
                "TRANBKP (REPROC, IDCAMS DELETE/DEFINE)",
                "TRANSACT.BKUP(+1): TRANSACT.BKUP.G<nnnn>V00 under CARDDEMO_BATCH_DIR",
                "Then empties the live transaction master",
                "\n  0   ",
                "\n  12  IDCAMS REPRO condition code",
                "\n  16  Setup failure",
                "\n  cargo run --bin batch -- tranbkp",
            ],
        ),
        (
            "combtran",
            &[keys::BATCH_DIR],
            &[
                "COMBTRAN (SORT, IDCAMS REPRO)",
                "TRANSACT.BKUP(0) and SYSTRAN(0)",
                "TRANSACT.COMBINED(+1): TRANSACT.COMBINED.G<nnnn>V00 under CARDDEMO_BATCH_DIR",
                "sorted by TRAN-ID",
                "The reload of the live transaction master",
                "\n  0   ",
                "\n  8   IDCAMS REPRO condition code",
                "\n  12  IDCAMS REPRO condition code",
                "\n  16  Equivalence halt",
                "\n  cargo run --bin batch -- combtran",
            ],
        ),
        (
            "creastmt",
            &[keys::STATEMENT_DIR],
            &[
                "CREASTMT (CBSTM03A, CBSTM03B)",
                "under CARDDEMO_STATEMENT_DIR",
                "\n  STATEMNT.PS  ",
                "\n  STATEMNT.HTML  ",
                "\n  STATEMNT.PS.PDF  ",
                "\n  0   ",
                "\n  16  Abend",
                "capacity stop",
                "\n  cargo run --bin batch -- creastmt",
            ],
        ),
        (
            "tranrept",
            &[keys::BATCH_DIR],
            &[
                "TRANREPT (REPROC, SORT, CBTRN03C)",
                "[default: 2022-01-01]",
                "[default: 2022-07-06]",
                "under CARDDEMO_BATCH_DIR",
                "\n  TRANSACT.BKUP(+1)  ",
                "\n  TRANSACT.DALY(+1)  ",
                "\n  TRANREPT(+1)  ",
                "133-byte lines",
                "\n  0   ",
                "\n  12  IDCAMS REPRO condition code",
                "\n  16  Abend",
                "ABEND 999",
                "\n  cargo run --bin batch -- tranrept --start-date 2022-01-01 --end-date 2022-07-06",
            ],
        ),
        (
            "cbpaup0j",
            &[keys::CLOCK_OVERRIDE],
            &[
                "CBPAUP0J (CBPAUP0C)",
                "[default: 00,00001,00001,Y]",
                "deletion of the expired pending authorizations",
                "the purge day",
                "\n  0   ",
                "\n  16  Abend",
                "\n  cargo run --bin batch -- cbpaup0j --sysin 00,00001,00001,Y",
            ],
        ),
        (
            "mnttrdb2",
            &[],
            &[
                "MNTTRDB2 (COBTUPDT)",
                "--input <FILE>",
                "the Db2 master transaction_types only",
                "\n  0   ",
                "\n  4   Records failed",
                "\n  16  Setup failure",
                "\n  cargo run --bin batch -- mnttrdb2 --input trantype.upd",
            ],
        ),
    ];

    #[test]
    fn help_names_every_subcommand_and_default() {
        let mut command = Cli::command();
        command.build();
        let root_help = command.render_long_help().to_string();
        for (name, settings, expected) in SUBCOMMAND_HELP {
            assert!(
                root_help.contains(name),
                "root help lacks {name}:\n{root_help}"
            );
            let subcommand = command
                .find_subcommand_mut(name)
                .unwrap_or_else(|| panic!("{name} is a subcommand"));
            for sub_help in both_helps(subcommand) {
                let sections = ["Reads", "\nWrites", "\nExit status:\n", "\nExample"];
                for text in sections.iter().chain([&keys::DATABASE_URL]).chain(expected) {
                    assert!(
                        sub_help.contains(text),
                        "{name} help lacks {text:?}:\n{sub_help}"
                    );
                }
                for key in JOB_SETTINGS {
                    assert_eq!(
                        sub_help.contains(key),
                        settings.contains(&key),
                        "{name} help and the settings its job reads disagree on {key}:\n{sub_help}"
                    );
                }
            }
        }
    }
}
