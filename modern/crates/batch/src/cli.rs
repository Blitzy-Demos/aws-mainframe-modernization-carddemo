//! The command line of `carddemo-batch`: one subcommand per in-scope JCL job (AAP 0.2.3,
//! 0.3.4.3), parsed by `clap` before the binary reads any environment, so `--help`, `--version`
//! and usage errors need neither `.env` nor a database.
//!
//! Each subcommand is the JCL member name in lower case; its options and their defaults:
//!
//! - `acctfile`, `cardfile`, `xreffile`, `custfile`, `discgrp`, `tcatbalf`, `trancatg` and
//!   `trantype` (IDCAMS `REPRO`): `--input`, else the job's fixture file in
//!   `CARDDEMO_FIXTURE_DIR` (LOADS-010).
//! - `dusrsecj` (IEBGENER): `--input`, default `../app/jcl/DUSRSECJ.jcl` (LOADS-012).
//! - `posttran` (`CBTRN02C`): `--dalytran`, default `../app/data/ASCII/dailytran.txt`
//!   (CBTRN02C-001).
//! - `intcalc` (`CBACT04C`): `--parm-date`, default `2022071800`, the PARM of `INTCALC.jcl:22`
//!   (CBACT04C-001).
//! - `creastmt` (`CBSTM03A` with `CBSTM03B`): no options.
//! - `tranrept` (`CBTRN03C`): `--start-date` and `--end-date`, defaults `2022-01-01` and
//!   `2022-07-06` of `TRANREPT.jcl:43-44` (CBTRN03C-001, CBTRN03C-002).
//! - `cbpaup0j` (`CBPAUP0C`): `--sysin`, default `00,00001,00001,Y`, the in-stream card of
//!   `CBPAUP0J.jcl:36-37` (CBPAUP0C-002).
//! - `mnttrdb2` (`COBTUPDT`): `--input`, required, because the `INPFILE` dataset of
//!   `MNTTRDB2.jcl:27` is in no repository source (COBTUPDT-001).
//!
//! Relative paths resolve against `modern/`, the working directory of every binary. The schema is
//! applied only by the `carddemo-migrate` binary of the `runtime` crate, which is no subcommand
//! here (Single Schema Executor).
//!
//! # Parsing
//!
//! - Typed Dates and Timestamps: `--start-date` and `--end-date` become `NaiveDate` values by the
//!   `PIC X(10)` date rule (PIC-015, PIC-016), and `--parm-date` becomes the domain's `ParmDate`;
//!   neither travels on as text.
//! - Total Parsers Over Untrusted Input: command-line text is external input, and every value is
//!   decoded by a total parser of `domain` (`PicDate::decode`, `ParmDate::parse`,
//!   `PurgeParams::parse`), which returns a typed error for any text.
//! - Cardholder Data Redaction: `clap` repeats a rejected value in its usage error. A SYSIN value
//!   may hold a protected value (`CBPAUP0J.md`, CBPAUP0C-002), so `--sysin` is kept as typed and
//!   parsed by [`Cbpaup0jArgs::purge_params`], whose error names only a length or an offset. The
//!   other options carry no protected data.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use domain::batch::cbact04c::{DEFAULT_PARM_DATE, ParmDate};
use domain::batch::cbpaup0c::{DEFAULT_SYSIN, PurgeParams, SysinError};
use domain::batch::cbtrn03c::{DateParm, DateParmError};
use domain::batch::loads::LoadJob;
use domain::copybook::PicDate;

/// The DUSRSECJ deck, read in place from `modern/` (`DUSRSECJ.jcl:34-45`; LOADS-012).
const DUSRSECJ_DECK: &str = "../app/jcl/DUSRSECJ.jcl";

/// The daily transaction fixture, read in place from `modern/` (CBTRN02C-001).
const DAILY_TRANSACTIONS: &str = "../app/data/ASCII/dailytran.txt";

/// The usage error of a date option that names no date (CBTRN03C-002). It is fixed text;
/// `clap` adds the rejected value, a date, which is no protected value.
const DATE_ERROR: &str = "date must be YYYY-MM-DD naming a calendar date";

// The help printed after the options names the environment each job reads, with the defaults
// of `runtime::config::Config`, the only reader of it. `clap` prints this text exactly as
// written, so every line break is explicit. `concat!` takes only literals, so the text that
// several help texts share is written as macros that expand to string literals.

/// The `DATABASE_URL` entry of an environment list. The default is described, never printed,
/// because the URL carries the password.
macro_rules! database_url_entry {
    () => {
        concat!(
            "  DATABASE_URL            PostgreSQL connection URL, read by every job\n",
            "                          [default: the Compose database localhost:5432/carddemo]\n",
        )
    };
}

/// The `CARDDEMO_FIXTURE_DIR` entry of an environment list.
macro_rules! fixture_dir_entry {
    () => {
        concat!(
            "  CARDDEMO_FIXTURE_DIR    Fixture directory of the eight file loads, read when\n",
            "                          --input is absent [default: ../app/data/ASCII]\n",
        )
    };
}

/// The `CARDDEMO_STATEMENT_DIR` entry of an environment list.
macro_rules! statement_dir_entry {
    () => {
        concat!(
            "  CARDDEMO_STATEMENT_DIR  Directory of the creastmt statements STATEMNT.PS,\n",
            "                          .HTML and .PDF; when unset, statements under\n",
            "                          CARDDEMO_VAR_DIR [default: ./var/statements]\n",
        )
    };
}

/// The example that ends the help of a subcommand: `$args` is the subcommand with any option
/// it needs, run from `modern/` with `.env` exported.
macro_rules! job_example {
    ($args:literal) => {
        concat!(
            "\nExample, from modern/:\n",
            "  set -a; . ./.env; set +a; carddemo-batch ",
            $args,
        )
    };
}

/// The help after the options of the file load `$job`.
macro_rules! load_help {
    ($job:literal) => {
        concat!(
            "Environment:\n",
            fixture_dir_entry!(),
            database_url_entry!(),
            job_example!($job),
        )
    };
}

/// The help after the commands of `carddemo-batch`: where to run, the environment of every
/// job and two invocations.
const CLI_HELP: &str = concat!(
    "Run from modern/ after carddemo-migrate has applied the schema. Every job prints each\n",
    "file it writes, one path per line.\n",
    "\n",
    "Environment:\n",
    database_url_entry!(),
    fixture_dir_entry!(),
    "  CARDDEMO_VAR_DIR        Root of the outputs: posttran writes DALYREJS.G<nnnn>\n",
    "                          in dalyrejs, intcalc SYSTRAN.G<nnnn> in systran and\n",
    "                          tranrept TRANREPT.G<nnnn> in reports [default: ./var]\n",
    statement_dir_entry!(),
    "  RUST_LOG                Tracing filter of the log events [default: info]\n",
    "  CARDDEMO_OTEL_EXPORT    OpenTelemetry span export: unset or empty means none,\n",
    "                          exactly stdout enables span export to standard output,\n",
    "                          and any other non-empty value is a configuration error\n",
    "                          [default: unset, no span export]\n",
    "\n",
    "No program loads .env: export it in the shell first with set -a; . ./.env; set +a.\n",
    "Relative paths resolve against modern/. modern/.env.example lists every variable.\n",
    "\n",
    "Examples, from modern/:\n",
    "  set -a; . ./.env; set +a; carddemo-batch acctfile\n",
    "  cargo run -p batch --bin carddemo-batch -- posttran",
);

/// The help after the options of `acctfile`.
const ACCTFILE_HELP: &str = load_help!("acctfile");

/// The help after the options of `cardfile`.
const CARDFILE_HELP: &str = load_help!("cardfile");

/// The help after the options of `xreffile`.
const XREFFILE_HELP: &str = load_help!("xreffile");

/// The help after the options of `custfile`.
const CUSTFILE_HELP: &str = load_help!("custfile");

/// The help after the options of `discgrp`.
const DISCGRP_HELP: &str = load_help!("discgrp");

/// The help after the options of `tcatbalf`.
const TCATBALF_HELP: &str = load_help!("tcatbalf");

/// The help after the options of `trancatg`.
const TRANCATG_HELP: &str = load_help!("trancatg");

/// The help after the options of `trantype`.
const TRANTYPE_HELP: &str = load_help!("trantype");

/// The help after the options of `dusrsecj`.
const DUSRSECJ_HELP: &str = concat!(
    "Environment:\n",
    database_url_entry!(),
    job_example!("dusrsecj"),
);

/// The help after the options of `posttran`.
const POSTTRAN_HELP: &str = concat!(
    "Environment:\n",
    database_url_entry!(),
    "  CARDDEMO_VAR_DIR        Root of the outputs; each run writes a new\n",
    "                          DALYREJS.G<nnnn> of rejects in dalyrejs under it,\n",
    "                          ./var/dalyrejs by default [default: ./var]\n",
    job_example!("posttran"),
);

/// The help after the options of `intcalc`.
const INTCALC_HELP: &str = concat!(
    "Environment:\n",
    database_url_entry!(),
    "  CARDDEMO_VAR_DIR        Root of the outputs; each run writes a new\n",
    "                          SYSTRAN.G<nnnn> in systran under it, ./var/systran\n",
    "                          by default [default: ./var]\n",
    job_example!("intcalc"),
);

/// The help after the options of `creastmt`.
const CREASTMT_HELP: &str = concat!(
    "Environment:\n",
    database_url_entry!(),
    statement_dir_entry!(),
    job_example!("creastmt"),
);

/// The help after the options of `tranrept`.
const TRANREPT_HELP: &str = concat!(
    "Environment:\n",
    database_url_entry!(),
    "  CARDDEMO_VAR_DIR        Root of the outputs; each run writes a new\n",
    "                          TRANREPT.G<nnnn> in reports under it, ./var/reports\n",
    "                          by default [default: ./var]\n",
    job_example!("tranrept"),
);

/// The help after the options of `cbpaup0j`.
const CBPAUP0J_HELP: &str = concat!(
    "Environment:\n",
    database_url_entry!(),
    job_example!("cbpaup0j"),
);

/// The help after the options of `mnttrdb2`, whose example names the required `--input`.
const MNTTRDB2_HELP: &str = concat!(
    "Environment:\n",
    database_url_entry!(),
    job_example!("mnttrdb2 --input maintenance.txt"),
);

/// Run one batch job of the COBOL replacement: each subcommand stands in for one JCL job
#[derive(Debug, clap::Parser)]
#[command(
    name = "carddemo-batch",
    version,
    disable_help_subcommand = true,
    after_help = CLI_HELP
)]
pub(crate) struct Cli {
    /// The job to run.
    #[command(subcommand)]
    pub(crate) job: Job,
}

/// The 15 jobs. Each name is written out, never derived from the variant, so the digits of
/// `cbpaup0j` and `mnttrdb2` stay where the JCL member names have them.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum Job {
    /// ACCTFILE (IDCAMS REPRO): replace the accounts with acctdata.txt
    #[command(name = "acctfile", after_help = ACCTFILE_HELP)]
    Acctfile(LoadArgs),
    /// CARDFILE (IDCAMS REPRO): replace the cards with carddata.txt
    #[command(name = "cardfile", after_help = CARDFILE_HELP)]
    Cardfile(LoadArgs),
    /// XREFFILE (IDCAMS REPRO): replace the card cross-references with cardxref.txt
    #[command(name = "xreffile", after_help = XREFFILE_HELP)]
    Xreffile(LoadArgs),
    /// CUSTFILE (IDCAMS REPRO): replace the customers with custdata.txt
    #[command(name = "custfile", after_help = CUSTFILE_HELP)]
    Custfile(LoadArgs),
    /// DISCGRP (IDCAMS REPRO): replace the disclosure groups with discgrp.txt
    #[command(name = "discgrp", after_help = DISCGRP_HELP)]
    Discgrp(LoadArgs),
    /// TCATBALF (IDCAMS REPRO): replace the transaction category balances with tcatbal.txt
    #[command(name = "tcatbalf", after_help = TCATBALF_HELP)]
    Tcatbalf(LoadArgs),
    /// TRANCATG (IDCAMS REPRO): replace the VSAM version of the transaction categories with
    /// trancatg.txt
    #[command(name = "trancatg", after_help = TRANCATG_HELP)]
    Trancatg(LoadArgs),
    /// TRANTYPE (IDCAMS REPRO): replace the VSAM version of the transaction types with
    /// trantype.txt
    #[command(name = "trantype", after_help = TRANTYPE_HELP)]
    Trantype(LoadArgs),
    /// DUSRSECJ (IEBGENER): replace the users with the in-stream records of the DUSRSECJ deck,
    /// passwords stored hashed
    #[command(name = "dusrsecj", after_help = DUSRSECJ_HELP)]
    Dusrsecj(DusrsecjArgs),
    /// POSTTRAN (CBTRN02C): post the daily transactions; rejects go to a new DALYREJS
    /// generation, and the exit code is 4 when any record is rejected
    #[command(name = "posttran", after_help = POSTTRAN_HELP)]
    Posttran(PosttranArgs),
    /// INTCALC (CBACT04C): compute the monthly interest into a new SYSTRAN generation and
    /// update the account balances
    #[command(name = "intcalc", after_help = INTCALC_HELP)]
    Intcalc(IntcalcArgs),
    /// CREASTMT (CBSTM03A, CBSTM03B): write the account statements STATEMNT.PS, STATEMNT.HTML
    /// and STATEMNT.PDF
    #[command(name = "creastmt", after_help = CREASTMT_HELP)]
    Creastmt,
    /// TRANREPT (CBTRN03C): write the transaction detail report for a processing-date window
    /// to a new TRANREPT generation
    #[command(name = "tranrept", after_help = TRANREPT_HELP)]
    Tranrept(TranreptArgs),
    /// CBPAUP0J (CBPAUP0C): purge the expired pending authorizations
    #[command(name = "cbpaup0j", after_help = CBPAUP0J_HELP)]
    Cbpaup0j(Cbpaup0jArgs),
    /// MNTTRDB2 (COBTUPDT): apply transaction type maintenance records to the Db2 version of
    /// the transaction types
    #[command(name = "mnttrdb2", after_help = MNTTRDB2_HELP)]
    Mnttrdb2(Mnttrdb2Args),
}

impl Job {
    /// The subcommand that selected this job, for the job span, the log and the stderr prefix.
    /// The nine loads name their `LoadJob::subcommand`, which the unit tests check.
    #[must_use]
    pub(crate) const fn name(&self) -> &'static str {
        match self {
            Self::Acctfile(_) => "acctfile",
            Self::Cardfile(_) => "cardfile",
            Self::Xreffile(_) => "xreffile",
            Self::Custfile(_) => "custfile",
            Self::Discgrp(_) => "discgrp",
            Self::Tcatbalf(_) => "tcatbalf",
            Self::Trancatg(_) => "trancatg",
            Self::Trantype(_) => "trantype",
            Self::Dusrsecj(_) => "dusrsecj",
            Self::Posttran(_) => "posttran",
            Self::Intcalc(_) => "intcalc",
            Self::Creastmt => "creastmt",
            Self::Tranrept(_) => "tranrept",
            Self::Cbpaup0j(_) => "cbpaup0j",
            Self::Mnttrdb2(_) => "mnttrdb2",
        }
    }
}

/// The input of a fixture load
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct LoadArgs {
    // No `clap` default: the default lies in `CARDDEMO_FIXTURE_DIR`, which `Config` reads only
    // after parsing; `input_path` joins the two (LOADS-010).
    /// Fixture file to load [default: the job's file in `CARDDEMO_FIXTURE_DIR`, ../app/data/ASCII
    /// when unset]
    #[arg(long, value_name = "PATH")]
    input: Option<PathBuf>,
}

impl LoadArgs {
    /// The file `job` reads: `--input` when it is given, else `fixture_dir` joined with the job's
    /// fixture file, such as `acctdata.txt` for `LoadJob::Acctfile` (LOADS-010).
    /// `LoadJob::Dusrsecj` has no fixture file; its default is the DUSRSECJ deck that `dusrsecj`
    /// reads (LOADS-012).
    #[must_use]
    pub(crate) fn input_path(&self, fixture_dir: &Path, job: LoadJob) -> PathBuf {
        match (&self.input, job.fixture_file()) {
            (Some(input), _) => input.clone(),
            (None, Some(file)) => fixture_dir.join(file),
            (None, None) => PathBuf::from(DUSRSECJ_DECK),
        }
    }
}

/// The input of `dusrsecj`
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct DusrsecjArgs {
    /// JCL deck whose SYSUT1 in-stream records are the user security records
    #[arg(long, value_name = "PATH", default_value = DUSRSECJ_DECK)]
    pub(crate) input: PathBuf,
}

/// The input of `posttran`
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct PosttranArgs {
    // CBTRN02C-001: line n of the file is the n-th daily transaction.
    /// Daily transaction file, one 350-byte record per line
    #[arg(long, value_name = "PATH", default_value = DAILY_TRANSACTIONS)]
    pub(crate) dalytran: PathBuf,
}

/// The PARM of `intcalc`, parsed into the domain's `ParmDate` while the command line is read.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct IntcalcArgs {
    // CBACT04C-001: a value of 10 or more printable ASCII bytes, of which the first 10 are
    // `PARM-DATE`; anything else is a usage error before any table is read.
    /// PARM of CBACT04C: 10 or more printable ASCII characters, the first 10 of which begin every
    /// interest transaction ID
    #[arg(
        long,
        value_name = "PARM",
        default_value = DEFAULT_PARM_DATE,
        value_parser = parse_parm_date
    )]
    pub(crate) parm_date: ParmDate,
}

/// The reporting window of `tranrept`, both dates inclusive.
///
/// No order between the two is required: CBTRN03C checks none, and a start after the end selects
/// nothing (CBTRN03C-002).
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct TranreptArgs {
    // CBTRN03C-001, CBTRN03C-002: the date rule of PIC-015; anything else is a usage error
    // before any table is read.
    /// First processing date reported, YYYY-MM-DD
    #[arg(
        long,
        value_name = "YYYY-MM-DD",
        default_value_t = DateParm::DEFAULT.start(),
        value_parser = parse_iso_date
    )]
    start_date: NaiveDate,
    // CBTRN03C-001, CBTRN03C-002, as `start_date`.
    /// Last processing date reported, YYYY-MM-DD
    #[arg(
        long,
        value_name = "YYYY-MM-DD",
        default_value_t = DateParm::DEFAULT.end(),
        value_parser = parse_iso_date
    )]
    end_date: NaiveDate,
}

impl TranreptArgs {
    /// The DATEPARM window of CBTRN03C from the two dates (CBTRN03C-001).
    ///
    /// # Errors
    ///
    /// `DateParmError::Start` or `DateParmError::End` for a date whose year lies outside 0 to
    /// 9999, which a `YYYY-MM-DD` text cannot carry. The option parser accepts only such text, so
    /// a parsed command line never fails here.
    pub(crate) fn date_parm(&self) -> Result<DateParm, DateParmError> {
        DateParm::new(self.start_date, self.end_date)
    }
}

/// The SYSIN record of `cbpaup0j`, kept exactly as typed until the job parses it.
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Cbpaup0jArgs {
    // CBPAUP0C-002: no `value_parser`, so a rejected record never reaches a `clap` message.
    /// SYSIN record of CBPAUP0C: expiry days, checkpoint frequency, display frequency and debug
    /// flag, at most 80 printable ASCII characters
    #[arg(long, value_name = "RECORD", default_value = DEFAULT_SYSIN)]
    sysin: String,
}

impl Cbpaup0jArgs {
    /// The run parameters CBPAUP0C derives from the SYSIN record, through the domain's total
    /// parser (CBPAUP0C-002).
    ///
    /// # Errors
    ///
    /// `SysinError::TooLong` for a record longer than 80 bytes and `SysinError::NotPrintable` for
    /// one holding a byte outside printable ASCII. Neither repeats the record's text.
    pub(crate) fn purge_params(&self) -> Result<PurgeParams, SysinError> {
        PurgeParams::parse(self.sysin.as_bytes())
    }
}

/// The input of `mnttrdb2`
#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Mnttrdb2Args {
    // COBTUPDT-001: required and without default, so its absence is a usage error before any
    // database access; COBTUPDT-002 and COBTUPDT-003 give the record layout.
    /// Maintenance records, one 53-byte record per line: action, type code, description; required,
    /// as no repository source holds the INPFILE dataset
    #[arg(long, value_name = "PATH")]
    pub(crate) input: PathBuf,
}

/// `--parm-date` through the domain's total parser (CBACT04C-001). Its error text names only the
/// length or the class of the rejected value.
fn parse_parm_date(raw: &str) -> Result<ParmDate, String> {
    ParmDate::parse(raw.as_bytes()).map_err(|error| error.to_string())
}

/// A date option by the `PIC X(10)` date rule: exactly 10 characters `YYYY-MM-DD` with
/// zero-padded parts, naming a valid calendar date (PIC-015). Blank, partly blank or unpadded
/// text, another layout and an impossible date are refused (PIC-016, CBTRN03C-002). `chrono`'s
/// own text parser is not used because it accepts unpadded parts such as `2022-1-01`.
fn parse_iso_date(raw: &str) -> Result<NaiveDate, String> {
    PicDate::decode(raw.as_bytes())
        .ok()
        .and_then(PicDate::value)
        .ok_or_else(|| DATE_ERROR.to_owned())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::path::{Path, PathBuf};

    use clap::error::ErrorKind;
    use clap::{CommandFactory, Parser};
    use domain::batch::cbact04c::{DEFAULT_PARM_DATE, ParmDate};
    use domain::batch::cbpaup0c::{DEFAULT_SYSIN, PurgeParams, SysinError};
    use domain::batch::cbtrn03c::DateParm;
    use domain::batch::loads::LoadJob;
    use runtime::config::{Config, OtelExport};

    use super::{Cli, DAILY_TRANSACTIONS, DUSRSECJ_DECK, Job, parse_iso_date};

    type TestResult = Result<(), Box<dyn Error>>;

    const BIN: &str = "carddemo-batch";

    /// The two forms of an example line in the help, both run from `modern/`: the binary with
    /// `.env` exported, and `cargo run`.
    const EXPORTED: &str = "set -a; . ./.env; set +a; carddemo-batch ";
    const CARGO_RUN: &str = "cargo run -p batch --bin carddemo-batch -- ";

    /// The 15 in-scope jobs (AAP 0.2.3).
    const NAMES: [&str; 15] = [
        "acctfile", "cardfile", "xreffile", "custfile", "discgrp", "tcatbalf", "trancatg",
        "trantype", "dusrsecj", "posttran", "intcalc", "creastmt", "tranrept", "cbpaup0j",
        "mnttrdb2",
    ];

    fn parse(args: &[&str]) -> Result<Job, clap::Error> {
        Cli::try_parse_from(std::iter::once(BIN).chain(args.iter().copied())).map(|cli| cli.job)
    }

    /// The jobs the example lines of `help` run, each as its form followed by the subcommand
    /// its arguments select. An example whose arguments do not parse is an error.
    fn example_jobs(help: &str) -> Result<BTreeSet<String>, clap::Error> {
        let mut jobs = BTreeSet::new();
        for line in help.lines().map(str::trim) {
            for form in [EXPORTED, CARGO_RUN] {
                if let Some(args) = line.strip_prefix(form) {
                    let args: Vec<&str> = args.split_whitespace().collect();
                    jobs.insert(format!("{form}{}", parse(&args)?.name()));
                }
            }
        }
        Ok(jobs)
    }

    /// Checks `help`, as the program (`name` `None`) or subcommand `name` prints it: it names
    /// every needle, shows no connection URL and no variable the batch does not read, and ends
    /// with its examples, which are exactly `examples` and parse.
    fn check_help(
        name: Option<&str>,
        help: &str,
        needles: &[&str],
        examples: &BTreeSet<String>,
    ) -> TestResult {
        for needle in needles {
            assert!(
                help.contains(needle),
                "{name:?} help names {needle:?}:\n{help}"
            );
        }
        assert!(!help.contains("carddemo:carddemo@"), "{name:?}:\n{help}");
        assert!(
            !help.contains(&Config::default().database_url),
            "{name:?}:\n{help}"
        );
        assert!(!help.contains("CARDDEMO_INTRDR_DIR"), "{name:?}:\n{help}");
        assert_eq!(&example_jobs(help)?, examples, "{name:?}:\n{help}");
        let last = help
            .trim_end()
            .lines()
            .last()
            .unwrap_or_default()
            .trim_start();
        let ends_with_example = [EXPORTED, CARGO_RUN]
            .iter()
            .any(|form| last.starts_with(form));
        assert!(
            ends_with_example,
            "{name:?} help ends with an example:\n{help}"
        );
        Ok(())
    }

    #[test]
    fn command_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn exactly_the_fifteen_jobs_are_subcommands() {
        let command = Cli::command();
        let names: BTreeSet<&str> = command
            .get_subcommands()
            .map(clap::Command::get_name)
            .collect();
        assert_eq!(names, NAMES.into_iter().collect::<BTreeSet<_>>());
        assert!(!names.contains("help"));
        assert!(!names.contains("carddemo-migrate"));
        assert!(!names.iter().any(|name| name.contains("migrat")));
    }

    #[test]
    fn help_is_neither_a_subcommand_nor_needed_to_list_the_jobs() {
        let help = Cli::command().render_help().to_string();
        for name in NAMES {
            assert!(help.contains(name), "--help lists {name}");
        }
        let err = parse(&["help"]).err();
        assert_eq!(err.map(|e| e.kind()), Some(ErrorKind::InvalidSubcommand));
    }

    // Each help text names the variables its jobs read, with the defaults of `Config` and the
    // output directories `Config` derives, shows no connection URL, and ends with examples that
    // parse to the subcommand they illustrate.
    #[test]
    fn help_names_the_environment_with_the_code_defaults_and_ends_with_an_example() -> TestResult {
        let config = Config::default();
        let stated = |path: &Path| format!("[default: {}]", path.display());
        let (fixture_dir, var_dir) = (stated(&config.fixture_dir), stated(&config.var_dir));
        let statement_dir = stated(&config.statement_dir);
        let input = format!("{} when unset]", config.fixture_dir.display());
        let dirs = [
            config.dalyrejs_dir(),
            config.systran_dir(),
            config.reports_dir(),
        ];
        let [dalyrejs, systran, reports] = dirs.each_ref().map(|dir| dir.display().to_string());
        let mut subdirs = Vec::new();
        for dir in &dirs {
            subdirs.push(format!(
                "in {}",
                dir.strip_prefix(&config.var_dir)?.display()
            ));
        }
        let subdirs: Vec<&str> = subdirs.iter().map(String::as_str).collect();

        let database: &[&str] = &["DATABASE_URL", "localhost:5432/carddemo"];
        let fixtures: &[&str] = &["CARDDEMO_FIXTURE_DIR", fixture_dir.as_str()];
        let var_root: &[&str] = &["CARDDEMO_VAR_DIR", var_dir.as_str()];
        let statements: &[&str] = &[
            "CARDDEMO_STATEMENT_DIR",
            statement_dir.as_str(),
            "STATEMNT.PS",
        ];
        let logging: &[&str] = &[
            "RUST_LOG",
            "[default: info]",
            "CARDDEMO_OTEL_EXPORT",
            "unset or empty",
        ];
        let exported: &[&str] = &[
            "set -a; . ./.env; set +a",
            ".env.example",
            "carddemo-migrate",
        ];
        let top = [
            database, fixtures, var_root, statements, &subdirs, logging, exported,
        ]
        .concat();
        let load = [fixtures, database, &[input.as_str()]].concat();
        let posttran = [database, var_root, &["DALYREJS.G", dalyrejs.as_str()]].concat();
        let intcalc = [database, var_root, &["SYSTRAN.G", systran.as_str()]].concat();
        let creastmt = [database, statements].concat();
        let tranrept = [database, var_root, &["TRANREPT.G", reports.as_str()]].concat();
        let cases: [(Option<&str>, &[&str]); 16] = [
            (None, &top),
            (Some("acctfile"), &load),
            (Some("cardfile"), &load),
            (Some("xreffile"), &load),
            (Some("custfile"), &load),
            (Some("discgrp"), &load),
            (Some("tcatbalf"), &load),
            (Some("trancatg"), &load),
            (Some("trantype"), &load),
            (Some("dusrsecj"), database),
            (Some("posttran"), &posttran),
            (Some("intcalc"), &intcalc),
            (Some("creastmt"), &creastmt),
            (Some("tranrept"), &tranrept),
            (Some("cbpaup0j"), database),
            (Some("mnttrdb2"), database),
        ];

        for (name, needles) in cases {
            let mut cli = Cli::command();
            let (command, examples) = match name {
                Some(name) => (
                    cli.find_subcommand_mut(name)
                        .ok_or_else(|| format!("no subcommand {name}"))?,
                    BTreeSet::from([format!("{EXPORTED}{name}")]),
                ),
                None => (
                    &mut cli,
                    BTreeSet::from([
                        format!("{EXPORTED}acctfile"),
                        format!("{CARGO_RUN}posttran"),
                    ]),
                ),
            };
            // What `-h` and `--help` print.
            for help in [command.render_help(), command.render_long_help()] {
                check_help(name, &help.to_string(), needles, &examples)?;
            }
        }
        Ok(())
    }

    // The `CARDDEMO_OTEL_EXPORT` entry of the program's help states what `Config` makes of an
    // empty value, of exactly `stdout` and of any other value.
    #[test]
    fn help_states_how_config_reads_the_span_export() {
        let help = Cli::command().render_help().to_string();
        let read = |value: &str| {
            Config::from_lookup(|var| (var == "CARDDEMO_OTEL_EXPORT").then(|| value.to_owned()))
                .map(|config| config.otel_export)
                .ok()
        };
        let cases = [
            ("", "unset or empty means none", Some(OtelExport::Off)),
            (
                "stdout",
                "exactly stdout enables span export",
                Some(OtelExport::Stdout),
            ),
            (
                "otlp",
                "any other non-empty value is a configuration error",
                None,
            ),
        ];
        for (value, stated, read_as) in cases {
            assert!(help.contains(stated), "--help states {stated:?}:\n{help}");
            assert_eq!(read(value), read_as, "CARDDEMO_OTEL_EXPORT={value:?}");
        }
    }

    #[test]
    fn no_subcommand_is_a_usage_error() {
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn every_subcommand_parses_and_names_itself() -> TestResult {
        for name in NAMES {
            let args: &[&str] = if name == "mnttrdb2" {
                &[name, "--input", "maintenance.txt"]
            } else {
                &[name]
            };
            assert_eq!(parse(args)?.name(), name);
        }
        Ok(())
    }

    #[test]
    fn every_subcommand_names_its_jcl_job() {
        let command = Cli::command();
        for sub in command.get_subcommands() {
            let about = sub.get_about().map(ToString::to_string).unwrap_or_default();
            let job = sub.get_name().to_uppercase();
            assert!(about.starts_with(&job), "{} about: {about}", sub.get_name());
        }
    }

    #[test]
    fn load_names_are_the_domain_subcommands() -> TestResult {
        for job in LoadJob::ALL {
            assert!(NAMES.contains(&job.subcommand()));
            assert_eq!(parse(&[job.subcommand()])?.name(), job.subcommand());
        }
        Ok(())
    }

    #[test]
    fn mnttrdb2_requires_input() -> TestResult {
        let err = parse(&["mnttrdb2"]).err();
        assert_eq!(
            err.map(|e| e.kind()),
            Some(ErrorKind::MissingRequiredArgument)
        );
        match parse(&["mnttrdb2", "--input", "maint.txt"])? {
            Job::Mnttrdb2(args) => assert_eq!(args.input, PathBuf::from("maint.txt")),
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: CBACT04C-001
    #[test]
    fn intcalc_parm_date_defaults_to_the_jcl_parm() -> TestResult {
        match parse(&["intcalc"])? {
            Job::Intcalc(args) => {
                assert_eq!(
                    args.parm_date,
                    ParmDate::parse(DEFAULT_PARM_DATE.as_bytes())?
                );
                assert_eq!(args.parm_date, ParmDate::DEFAULT);
                assert_eq!(args.parm_date.as_bytes(), b"2022071800");
            }
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: CBACT04C-001
    #[test]
    fn intcalc_keeps_the_first_ten_parm_characters() -> TestResult {
        match parse(&["intcalc", "--parm-date", "2023010199XYZ"])? {
            Job::Intcalc(args) => assert_eq!(args.parm_date.as_bytes(), b"2023010199"),
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: CBACT04C-001
    #[test]
    fn intcalc_rejects_a_parm_the_domain_rejects() {
        for parm in ["20220718", "", "2022071800\t", "2022-07-18é"] {
            assert!(ParmDate::parse(parm.as_bytes()).is_err(), "{parm:?}");
            let err = parse(&["intcalc", "--parm-date", parm]).err();
            assert_eq!(
                err.map(|e| e.kind()),
                Some(ErrorKind::ValueValidation),
                "{parm:?}"
            );
        }
    }

    // audit: CBTRN03C-001
    #[test]
    fn tranrept_defaults_to_the_jcl_window() -> TestResult {
        match parse(&["tranrept"])? {
            Job::Tranrept(args) => {
                assert_eq!(args.date_parm()?, DateParm::DEFAULT);
                assert_eq!(args.start_date.to_string(), "2022-01-01");
                assert_eq!(args.end_date.to_string(), "2022-07-06");
            }
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: CBTRN03C-002
    #[test]
    fn tranrept_takes_any_order_of_two_valid_dates() -> TestResult {
        let args = [
            "tranrept",
            "--start-date",
            "2022-03-01",
            "--end-date",
            "2022-02-28",
        ];
        match parse(&args)? {
            Job::Tranrept(args) => {
                assert_eq!(
                    args.date_parm()?,
                    DateParm::parse("2022-03-01", "2022-02-28")?
                );
            }
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: CBTRN03C-002
    #[test]
    fn tranrept_rejects_text_that_names_no_date() {
        let texts = [
            "2022-1-1",
            "2022-02-30",
            "2022-13-01",
            "01/01/2022",
            "2022-01-1 ",
            " 2022-1-01",
            "-001-01-01",
            "+002-01-01",
            "          ",
            "2022-01-  ",
            "",
            "2022-01-010",
        ];
        for text in texts {
            for option in ["--start-date", "--end-date"] {
                // The `=` form hands clap a value that starts with `-` as a value, not a flag.
                let arg = format!("{option}={text}");
                let err = parse(&["tranrept", &arg]).err();
                assert_eq!(
                    err.map(|e| e.kind()),
                    Some(ErrorKind::ValueValidation),
                    "{arg:?}"
                );
            }
        }
    }

    // audit: PIC-015
    #[test]
    fn date_options_accept_exactly_what_the_domain_window_accepts() {
        let texts = [
            "2022-01-01",
            "0000-01-01",
            "9999-12-31",
            "2024-02-29",
            "2023-02-29",
            "2022-1-01",
            " 2022-1-1 ",
            "2022/01/01",
            "          ",
            "+202-01-01",
            "2022-01-01T",
        ];
        for text in texts {
            let window = DateParm::parse(text, text).ok().map(|parm| parm.start());
            assert_eq!(parse_iso_date(text).ok(), window, "{text:?}");
        }
    }

    // audit: CBPAUP0C-002
    #[test]
    fn cbpaup0j_sysin_defaults_to_the_instream_card() -> TestResult {
        match parse(&["cbpaup0j"])? {
            Job::Cbpaup0j(args) => {
                assert_eq!(args.sysin, "00,00001,00001,Y");
                assert_eq!(args.sysin, DEFAULT_SYSIN);
                assert_eq!(
                    args.purge_params()?,
                    PurgeParams::parse(DEFAULT_SYSIN.as_bytes())?
                );
            }
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: CBPAUP0C-002
    #[test]
    fn cbpaup0j_sysin_is_parsed_by_the_job_not_by_clap() -> TestResult {
        let long = "9".repeat(81);
        let cases = [
            ("", Ok(())),
            ("05", Ok(())),
            (long.as_str(), Err(SysinError::TooLong { len: 81 })),
            ("00,\t", Err(SysinError::NotPrintable { offset: 3 })),
        ];
        for (sysin, expected) in cases {
            match parse(&["cbpaup0j", "--sysin", sysin])? {
                Job::Cbpaup0j(args) => {
                    assert_eq!(args.sysin, sysin);
                    assert_eq!(args.purge_params().map(|_| ()), expected, "{sysin:?}");
                }
                other => return Err(format!("parsed {other:?}").into()),
            }
        }
        Ok(())
    }

    // audit: LOADS-012
    #[test]
    fn dusrsecj_reads_the_deck_in_place_by_default() -> TestResult {
        match parse(&["dusrsecj"])? {
            Job::Dusrsecj(args) => assert_eq!(args.input, PathBuf::from("../app/jcl/DUSRSECJ.jcl")),
            other => return Err(format!("parsed {other:?}").into()),
        }
        match parse(&["dusrsecj", "--input", "deck.jcl"])? {
            Job::Dusrsecj(args) => assert_eq!(args.input, PathBuf::from("deck.jcl")),
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: CBTRN02C-001
    #[test]
    fn posttran_reads_the_daily_fixture_by_default() -> TestResult {
        match parse(&["posttran"])? {
            Job::Posttran(args) => {
                assert_eq!(
                    args.dalytran,
                    PathBuf::from("../app/data/ASCII/dailytran.txt")
                );
                assert_eq!(args.dalytran, PathBuf::from(DAILY_TRANSACTIONS));
            }
            other => return Err(format!("parsed {other:?}").into()),
        }
        match parse(&["posttran", "--dalytran", "daily.txt"])? {
            Job::Posttran(args) => assert_eq!(args.dalytran, PathBuf::from("daily.txt")),
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: LOADS-010
    #[test]
    fn load_input_defaults_to_the_fixture_in_the_fixture_dir() -> TestResult {
        let dir = Path::new("fixtures");
        let cases = [
            ("acctfile", LoadJob::Acctfile, "fixtures/acctdata.txt"),
            ("tcatbalf", LoadJob::Tcatbalf, "fixtures/tcatbal.txt"),
            ("xreffile", LoadJob::Xreffile, "fixtures/cardxref.txt"),
        ];
        for (name, job, expected) in cases {
            match parse(&[name])? {
                Job::Acctfile(args) | Job::Tcatbalf(args) | Job::Xreffile(args) => {
                    assert_eq!(args.input_path(dir, job), PathBuf::from(expected));
                }
                other => return Err(format!("parsed {other:?}").into()),
            }
        }
        Ok(())
    }

    // audit: LOADS-010
    #[test]
    fn explicit_load_input_wins() -> TestResult {
        match parse(&["cardfile", "--input", "other/cards.txt"])? {
            Job::Cardfile(args) => {
                let path = args.input_path(Path::new("fixtures"), LoadJob::Cardfile);
                assert_eq!(path, PathBuf::from("other/cards.txt"));
            }
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }

    // audit: LOADS-012
    #[test]
    fn a_load_without_fixture_file_defaults_to_the_deck() -> TestResult {
        match parse(&["trantype"])? {
            Job::Trantype(args) => {
                let path = args.input_path(Path::new("fixtures"), LoadJob::Dusrsecj);
                assert_eq!(path, PathBuf::from(DUSRSECJ_DECK));
            }
            other => return Err(format!("parsed {other:?}").into()),
        }
        Ok(())
    }
}
