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
//!
//! # Construction
//!
//! The command is built with `clap`'s builder (`Command`, `Arg`, `ArgGroup`), and [`Cli`]
//! implements `CommandFactory`, `FromArgMatches` and `Parser` by hand. `clap`'s derive macros are
//! not used because the code they generate carries lint suppressions, which this workspace admits
//! nowhere, generated code included. The command keeps the shape the derive gives a parser: the
//! options of each subcommand form an `ArgGroup` named after the struct that holds them, each
//! option's id is its field name and its long name that name in kebab case, and an update
//! (`Parser::try_update_from`) replaces only the values its command line holds, defaults
//! included, or the whole job when it names another subcommand. An update needs no subcommand,
//! but the subcommand it names is built as for a parse, so `clap` refuses the update for what
//! it refuses a parse, `mnttrdb2` without `--input` included, and the job stays as it was.
//! Values leave the matches through `ArgMatches::try_remove_one`, so a reading that does not fit
//! its option is a typed `clap` error, never a panic.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::NaiveDate;
use clap::builder::ValueParser;
use clap::error::ErrorKind;
use clap::parser::MatchesError;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
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

/// The line that heads the help of `carddemo-batch`.
const CLI_ABOUT: &str =
    "Run one batch job of the COBOL replacement: each subcommand stands in for one JCL job";

/// Run one batch job of the COBOL replacement: each subcommand stands in for one JCL job
#[derive(Debug)]
pub(crate) struct Cli {
    /// The job to run.
    pub(crate) job: Job,
}

impl Cli {
    /// The command of `carddemo-batch`, with the 15 subcommands in the order of the `Job`
    /// variants. Built for an update (`for_update`), it requires no subcommand, so an update
    /// without one leaves the job as it is. The subcommands are the same in both commands, so
    /// an update that names one is held to the options its parse requires.
    fn build(for_update: bool) -> Command {
        // `Cli` holds no option of its own, so its group has no member.
        let command = Command::new("carddemo-batch").group(ArgGroup::new("Cli").multiple(true));
        JOB_COMMANDS
            .iter()
            .fold(command, |command, job| command.subcommand(job.command()))
            .subcommand_required(!for_update)
            .arg_required_else_help(!for_update)
            .about(CLI_ABOUT)
            .version(env!("CARGO_PKG_VERSION"))
            .disable_help_subcommand(true)
            .after_help(CLI_HELP)
    }
}

impl clap::CommandFactory for Cli {
    fn command() -> Command {
        Self::build(false)
    }

    fn command_for_update() -> Command {
        Self::build(true)
    }
}

impl clap::FromArgMatches for Cli {
    fn from_arg_matches(matches: &ArgMatches) -> Result<Self, clap::Error> {
        Self::from_arg_matches_mut(&mut matches.clone())
    }

    fn from_arg_matches_mut(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Job::read(matches).map(|job| Self { job })
    }

    fn update_from_arg_matches(&mut self, matches: &ArgMatches) -> Result<(), clap::Error> {
        self.update_from_arg_matches_mut(&mut matches.clone())
    }

    fn update_from_arg_matches_mut(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        self.job.update(matches)
    }
}

/// `Parser` provides every one of its methods, `parse`, `try_parse_from` and `try_update_from`
/// among them, over the `CommandFactory` and `FromArgMatches` implementations above.
impl clap::Parser for Cli {}

/// The 15 jobs. Each name is written out, never derived from the variant, so the digits of
/// `cbpaup0j` and `mnttrdb2` stay where the JCL member names have them.
#[derive(Debug)]
pub(crate) enum Job {
    /// ACCTFILE (IDCAMS REPRO): replace the accounts with acctdata.txt
    Acctfile(LoadArgs),
    /// CARDFILE (IDCAMS REPRO): replace the cards with carddata.txt
    Cardfile(LoadArgs),
    /// XREFFILE (IDCAMS REPRO): replace the card cross-references with cardxref.txt
    Xreffile(LoadArgs),
    /// CUSTFILE (IDCAMS REPRO): replace the customers with custdata.txt
    Custfile(LoadArgs),
    /// DISCGRP (IDCAMS REPRO): replace the disclosure groups with discgrp.txt
    Discgrp(LoadArgs),
    /// TCATBALF (IDCAMS REPRO): replace the transaction category balances with tcatbal.txt
    Tcatbalf(LoadArgs),
    /// TRANCATG (IDCAMS REPRO): replace the VSAM version of the transaction categories with
    /// trancatg.txt
    Trancatg(LoadArgs),
    /// TRANTYPE (IDCAMS REPRO): replace the VSAM version of the transaction types with
    /// trantype.txt
    Trantype(LoadArgs),
    /// DUSRSECJ (IEBGENER): replace the users with the in-stream records of the DUSRSECJ deck,
    /// passwords stored hashed
    Dusrsecj(DusrsecjArgs),
    /// POSTTRAN (CBTRN02C): post the daily transactions; rejects go to a new DALYREJS
    /// generation, and the exit code is 4 when any record is rejected
    Posttran(PosttranArgs),
    /// INTCALC (CBACT04C): compute the monthly interest into a new SYSTRAN generation and
    /// update the account balances
    Intcalc(IntcalcArgs),
    /// CREASTMT (CBSTM03A, CBSTM03B): write the account statements STATEMNT.PS, STATEMNT.HTML
    /// and STATEMNT.PDF
    Creastmt,
    /// TRANREPT (CBTRN03C): write the transaction detail report for a processing-date window
    /// to a new TRANREPT generation
    Tranrept(TranreptArgs),
    /// CBPAUP0J (CBPAUP0C): purge the expired pending authorizations
    Cbpaup0j(Cbpaup0jArgs),
    /// MNTTRDB2 (COBTUPDT): apply transaction type maintenance records to the Db2 version of
    /// the transaction types
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

    /// The job of the subcommand the matches hold.
    ///
    /// # Errors
    ///
    /// `ErrorKind::MissingSubcommand` when the matches hold no subcommand, and any error of
    /// [`Job::read_subcommand`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        let Some((name, mut subcommand)) = matches.remove_subcommand() else {
            return Err(clap::Error::raw(
                ErrorKind::MissingSubcommand,
                "a subcommand is required but one was not provided",
            ));
        };
        Self::read_subcommand(&name, &mut subcommand)
    }

    /// The job of subcommand `name`, from the subcommand's own matches.
    ///
    /// # Errors
    ///
    /// `ErrorKind::InvalidSubcommand` for a name no job has, and any error of reading the job's
    /// options.
    fn read_subcommand(name: &str, matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        match JOB_COMMANDS.iter().find(|job| job.name == name) {
            Some(job) => (job.read)(matches),
            None => Err(clap::Error::raw(
                ErrorKind::InvalidSubcommand,
                format!("the subcommand '{name}' wasn't recognized"),
            )),
        }
    }

    /// Updates the job from the matches of an update: a subcommand naming this job replaces the
    /// options its matches hold, a subcommand naming another job replaces the job, and matches
    /// without a subcommand leave the job as it is.
    ///
    /// # Errors
    ///
    /// Any error of reading the options or the other job. Another job that cannot be read never
    /// replaces this one.
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        let Some((name, mut subcommand)) = matches.remove_subcommand() else {
            return Ok(());
        };
        let matches = &mut subcommand;
        if name != self.name() {
            *self = Self::read_subcommand(&name, matches)?;
            return Ok(());
        }
        match self {
            Self::Acctfile(args)
            | Self::Cardfile(args)
            | Self::Xreffile(args)
            | Self::Custfile(args)
            | Self::Discgrp(args)
            | Self::Tcatbalf(args)
            | Self::Trancatg(args)
            | Self::Trantype(args) => args.update(matches),
            Self::Dusrsecj(args) => args.update(matches),
            Self::Posttran(args) => args.update(matches),
            Self::Intcalc(args) => args.update(matches),
            // `creastmt` takes no option, so its matches hold nothing to replace.
            Self::Creastmt => Ok(()),
            Self::Tranrept(args) => args.update(matches),
            Self::Cbpaup0j(args) => args.update(matches),
            Self::Mnttrdb2(args) => args.update(matches),
        }
    }
}

/// One subcommand: the job it selects, its help and its options.
struct JobCommand {
    /// The subcommand, as [`Job::name`] returns it for the job it selects.
    name: &'static str,
    /// The line the help lists for the subcommand.
    about: &'static str,
    /// The help printed after the subcommand's options.
    after_help: &'static str,
    /// Adds the subcommand's options to its command; `None` for a job that takes no option.
    options: Option<fn(Command) -> Command>,
    /// The job, from the matches of the subcommand.
    read: fn(&mut ArgMatches) -> Result<Job, clap::Error>,
}

impl JobCommand {
    /// The subcommand's command, the same for a parse and for an update.
    fn command(&self) -> Command {
        let command = Command::new(self.name);
        let command = match self.options {
            Some(options) => options(command),
            None => command,
        };
        command.about(self.about).after_help(self.after_help)
    }
}

/// The 15 subcommands, in the order of the `Job` variants, which is the order the help lists.
static JOB_COMMANDS: [JobCommand; 15] = [
    JobCommand {
        name: "acctfile",
        about: "ACCTFILE (IDCAMS REPRO): replace the accounts with acctdata.txt",
        after_help: ACCTFILE_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Acctfile),
    },
    JobCommand {
        name: "cardfile",
        about: "CARDFILE (IDCAMS REPRO): replace the cards with carddata.txt",
        after_help: CARDFILE_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Cardfile),
    },
    JobCommand {
        name: "xreffile",
        about: "XREFFILE (IDCAMS REPRO): replace the card cross-references with cardxref.txt",
        after_help: XREFFILE_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Xreffile),
    },
    JobCommand {
        name: "custfile",
        about: "CUSTFILE (IDCAMS REPRO): replace the customers with custdata.txt",
        after_help: CUSTFILE_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Custfile),
    },
    JobCommand {
        name: "discgrp",
        about: "DISCGRP (IDCAMS REPRO): replace the disclosure groups with discgrp.txt",
        after_help: DISCGRP_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Discgrp),
    },
    JobCommand {
        name: "tcatbalf",
        about: concat!(
            "TCATBALF (IDCAMS REPRO): replace the transaction category balances with ",
            "tcatbal.txt",
        ),
        after_help: TCATBALF_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Tcatbalf),
    },
    JobCommand {
        name: "trancatg",
        about: concat!(
            "TRANCATG (IDCAMS REPRO): replace the VSAM version of the transaction categories ",
            "with trancatg.txt",
        ),
        after_help: TRANCATG_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Trancatg),
    },
    JobCommand {
        name: "trantype",
        about: concat!(
            "TRANTYPE (IDCAMS REPRO): replace the VSAM version of the transaction types with ",
            "trantype.txt",
        ),
        after_help: TRANTYPE_HELP,
        options: Some(LoadArgs::options),
        read: |matches| LoadArgs::read(matches).map(Job::Trantype),
    },
    JobCommand {
        name: "dusrsecj",
        about: concat!(
            "DUSRSECJ (IEBGENER): replace the users with the in-stream records of the DUSRSECJ ",
            "deck, passwords stored hashed",
        ),
        after_help: DUSRSECJ_HELP,
        options: Some(DusrsecjArgs::options),
        read: |matches| DusrsecjArgs::read(matches).map(Job::Dusrsecj),
    },
    JobCommand {
        name: "posttran",
        about: concat!(
            "POSTTRAN (CBTRN02C): post the daily transactions; rejects go to a new DALYREJS ",
            "generation, and the exit code is 4 when any record is rejected",
        ),
        after_help: POSTTRAN_HELP,
        options: Some(PosttranArgs::options),
        read: |matches| PosttranArgs::read(matches).map(Job::Posttran),
    },
    JobCommand {
        name: "intcalc",
        about: concat!(
            "INTCALC (CBACT04C): compute the monthly interest into a new SYSTRAN generation and ",
            "update the account balances",
        ),
        after_help: INTCALC_HELP,
        options: Some(IntcalcArgs::options),
        read: |matches| IntcalcArgs::read(matches).map(Job::Intcalc),
    },
    JobCommand {
        name: "creastmt",
        about: concat!(
            "CREASTMT (CBSTM03A, CBSTM03B): write the account statements STATEMNT.PS, ",
            "STATEMNT.HTML and STATEMNT.PDF",
        ),
        after_help: CREASTMT_HELP,
        options: None,
        read: |_| Ok(Job::Creastmt),
    },
    JobCommand {
        name: "tranrept",
        about: concat!(
            "TRANREPT (CBTRN03C): write the transaction detail report for a processing-date ",
            "window to a new TRANREPT generation",
        ),
        after_help: TRANREPT_HELP,
        options: Some(TranreptArgs::options),
        read: |matches| TranreptArgs::read(matches).map(Job::Tranrept),
    },
    JobCommand {
        name: "cbpaup0j",
        about: "CBPAUP0J (CBPAUP0C): purge the expired pending authorizations",
        after_help: CBPAUP0J_HELP,
        options: Some(Cbpaup0jArgs::options),
        read: |matches| Cbpaup0jArgs::read(matches).map(Job::Cbpaup0j),
    },
    JobCommand {
        name: "mnttrdb2",
        about: concat!(
            "MNTTRDB2 (COBTUPDT): apply transaction type maintenance records to the Db2 version ",
            "of the transaction types",
        ),
        after_help: MNTTRDB2_HELP,
        options: Some(Mnttrdb2Args::options),
        read: |matches| Mnttrdb2Args::read(matches).map(Job::Mnttrdb2),
    },
];

/// Option `--<long>` with id `id`, the name of the field it fills: it takes one value, which the
/// help names `value_name` and `parser` turns into the field's type.
fn option(
    id: &'static str,
    long: &'static str,
    value_name: &'static str,
    parser: ValueParser,
) -> Arg {
    Arg::new(id)
        .long(long)
        .value_name(value_name)
        .value_parser(parser)
        .action(ArgAction::Set)
}

/// `command` with the options of one struct and the group `group`, the struct's name, that holds
/// them all.
fn with_options<const N: usize>(
    command: Command,
    group: &'static str,
    options: [Arg; N],
) -> Command {
    let members = options.each_ref().map(Arg::get_id);
    let command = command.group(ArgGroup::new(group).multiple(true).args(members));
    options.into_iter().fold(command, Command::arg)
}

/// The error of a reading that does not fit option `id`: an id the command does not define, or a
/// value of another type than the reading asks for. It names no value.
fn unreadable(id: &str, error: &MatchesError) -> clap::Error {
    clap::Error::raw(
        ErrorKind::ValueValidation,
        format!(
            "the value of {id} cannot be read: {}",
            error.to_string().trim_end()
        ),
    )
}

/// Whether the matches hold option `id`, from the command line or from its default.
///
/// # Errors
///
/// The error of [`unreadable`] when the command defines no option `id`.
fn holds(matches: &ArgMatches, id: &str) -> Result<bool, clap::Error> {
    matches
        .try_contains_id(id)
        .map_err(|error| unreadable(id, &error))
}

/// The value of option `id`, taken out of the matches; `None` when they hold none.
///
/// # Errors
///
/// The error of [`unreadable`] when the option is undefined or its value is no `T`.
fn take<T: Any + Clone + Send + Sync>(
    matches: &mut ArgMatches,
    id: &str,
) -> Result<Option<T>, clap::Error> {
    matches
        .try_remove_one(id)
        .map_err(|error| unreadable(id, &error))
}

/// The value of option `id`, which every parse yields through the command line, the option's
/// default or its being required.
///
/// # Errors
///
/// `ErrorKind::MissingRequiredArgument` when the matches hold no value, and any error of
/// [`take`]. Matches of this command always hold one: `clap` refuses a command line without a
/// required option, an update's included, before any value is read.
fn take_required<T: Any + Clone + Send + Sync>(
    matches: &mut ArgMatches,
    id: &str,
) -> Result<T, clap::Error> {
    take(matches, id)?.ok_or_else(|| {
        clap::Error::raw(
            ErrorKind::MissingRequiredArgument,
            format!("the following required argument was not provided: {id}"),
        )
    })
}

/// Replaces `value` with the value of option `id` when the matches of an update hold the option.
///
/// # Errors
///
/// Any error of [`holds`] or [`take_required`]; `value` is then left as it was.
fn update_value<T: Any + Clone + Send + Sync>(
    value: &mut T,
    matches: &mut ArgMatches,
    id: &str,
) -> Result<(), clap::Error> {
    if holds(matches, id)? {
        *value = take_required(matches, id)?;
    }
    Ok(())
}

/// The input of a fixture load
#[derive(Debug, Clone)]
pub(crate) struct LoadArgs {
    // No `clap` default: the default lies in `CARDDEMO_FIXTURE_DIR`, which `Config` reads only
    // after parsing; `input_path` joins the two (LOADS-010).
    /// Fixture file to load [default: the job's file in `CARDDEMO_FIXTURE_DIR`, ../app/data/ASCII
    /// when unset]
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

    /// `command` with `--input`, the option of every fixture load.
    fn options(command: Command) -> Command {
        let input = option("input", "input", "PATH", ValueParser::path_buf()).help(concat!(
            "Fixture file to load [default: the job's file in `CARDDEMO_FIXTURE_DIR`, ",
            "../app/data/ASCII when unset]",
        ));
        with_options(command, "LoadArgs", [input])
    }

    /// The input, from the matches of a fixture load.
    ///
    /// # Errors
    ///
    /// Any error of [`take`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            input: take(matches, "input")?,
        })
    }

    /// Replaces the input when the matches of an update hold `--input`.
    ///
    /// # Errors
    ///
    /// Any error of [`holds`] or [`take`].
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        if holds(matches, "input")? {
            self.input = take(matches, "input")?;
        }
        Ok(())
    }
}

/// The input of `dusrsecj`
#[derive(Debug, Clone)]
pub(crate) struct DusrsecjArgs {
    /// JCL deck whose SYSUT1 in-stream records are the user security records
    pub(crate) input: PathBuf,
}

impl DusrsecjArgs {
    /// `command` with `--input`, the deck `dusrsecj` reads.
    fn options(command: Command) -> Command {
        let input = option("input", "input", "PATH", ValueParser::path_buf())
            .help("JCL deck whose SYSUT1 in-stream records are the user security records")
            .default_value(DUSRSECJ_DECK);
        with_options(command, "DusrsecjArgs", [input])
    }

    /// The input, from the matches of `dusrsecj`.
    ///
    /// # Errors
    ///
    /// Any error of [`take_required`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            input: take_required(matches, "input")?,
        })
    }

    /// Replaces the input when the matches of an update hold `--input`.
    ///
    /// # Errors
    ///
    /// Any error of [`update_value`].
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        update_value(&mut self.input, matches, "input")
    }
}

/// The input of `posttran`
#[derive(Debug, Clone)]
pub(crate) struct PosttranArgs {
    // CBTRN02C-001: line n of the file is the n-th daily transaction.
    /// Daily transaction file, one 350-byte record per line
    pub(crate) dalytran: PathBuf,
}

impl PosttranArgs {
    /// `command` with `--dalytran`, the daily transaction file `posttran` posts.
    fn options(command: Command) -> Command {
        let dalytran = option("dalytran", "dalytran", "PATH", ValueParser::path_buf())
            .help("Daily transaction file, one 350-byte record per line")
            .default_value(DAILY_TRANSACTIONS);
        with_options(command, "PosttranArgs", [dalytran])
    }

    /// The input, from the matches of `posttran`.
    ///
    /// # Errors
    ///
    /// Any error of [`take_required`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            dalytran: take_required(matches, "dalytran")?,
        })
    }

    /// Replaces the input when the matches of an update hold `--dalytran`.
    ///
    /// # Errors
    ///
    /// Any error of [`update_value`].
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        update_value(&mut self.dalytran, matches, "dalytran")
    }
}

/// The PARM of `intcalc`, parsed into the domain's `ParmDate` while the command line is read.
#[derive(Debug, Clone)]
pub(crate) struct IntcalcArgs {
    // CBACT04C-001: a value of 10 or more printable ASCII bytes, of which the first 10 are
    // `PARM-DATE`; anything else is a usage error before any table is read.
    /// PARM of CBACT04C: 10 or more printable ASCII characters, the first 10 of which begin every
    /// interest transaction ID
    pub(crate) parm_date: ParmDate,
}

impl IntcalcArgs {
    /// `command` with `--parm-date`, decoded by [`parse_parm_date`] (CBACT04C-001).
    fn options(command: Command) -> Command {
        let parm_date = option(
            "parm_date",
            "parm-date",
            "PARM",
            ValueParser::new(parse_parm_date),
        )
        .help(concat!(
            "PARM of CBACT04C: 10 or more printable ASCII characters, the first 10 of which ",
            "begin every interest transaction ID",
        ))
        .default_value(DEFAULT_PARM_DATE);
        with_options(command, "IntcalcArgs", [parm_date])
    }

    /// The PARM, from the matches of `intcalc`.
    ///
    /// # Errors
    ///
    /// Any error of [`take_required`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            parm_date: take_required(matches, "parm_date")?,
        })
    }

    /// Replaces the PARM when the matches of an update hold `--parm-date`.
    ///
    /// # Errors
    ///
    /// Any error of [`update_value`].
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        update_value(&mut self.parm_date, matches, "parm_date")
    }
}

/// The reporting window of `tranrept`, both dates inclusive.
///
/// No order between the two is required: CBTRN03C checks none, and a start after the end selects
/// nothing (CBTRN03C-002).
#[derive(Debug, Clone)]
pub(crate) struct TranreptArgs {
    // CBTRN03C-001, CBTRN03C-002: the date rule of PIC-015; anything else is a usage error
    // before any table is read.
    /// First processing date reported, YYYY-MM-DD
    start_date: NaiveDate,
    // CBTRN03C-001, CBTRN03C-002, as `start_date`.
    /// Last processing date reported, YYYY-MM-DD
    end_date: NaiveDate,
}

/// The default of `--start-date`, the start of `DateParm::DEFAULT` as `YYYY-MM-DD`. `clap` takes
/// a default only as text that lives as long as the program, so the date is written out once.
static START_DATE_DEFAULT: OnceLock<String> = OnceLock::new();

/// The default of `--end-date`, the end of `DateParm::DEFAULT`, written out as
/// [`START_DATE_DEFAULT`] is.
static END_DATE_DEFAULT: OnceLock<String> = OnceLock::new();

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

    /// `command` with `--start-date` and `--end-date`, each decoded by [`parse_iso_date`] and
    /// defaulting to its end of `DateParm::DEFAULT` (CBTRN03C-001).
    fn options(command: Command) -> Command {
        let date = |id, long, help| {
            option(id, long, "YYYY-MM-DD", ValueParser::new(parse_iso_date)).help(help)
        };
        let start_default =
            START_DATE_DEFAULT.get_or_init(|| DateParm::DEFAULT.start().to_string());
        let end_default = END_DATE_DEFAULT.get_or_init(|| DateParm::DEFAULT.end().to_string());
        let start_date = date(
            "start_date",
            "start-date",
            "First processing date reported, YYYY-MM-DD",
        )
        .default_value(start_default.as_str());
        let end_date = date(
            "end_date",
            "end-date",
            "Last processing date reported, YYYY-MM-DD",
        )
        .default_value(end_default.as_str());
        with_options(command, "TranreptArgs", [start_date, end_date])
    }

    /// The window, from the matches of `tranrept`.
    ///
    /// # Errors
    ///
    /// Any error of [`take_required`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            start_date: take_required(matches, "start_date")?,
            end_date: take_required(matches, "end_date")?,
        })
    }

    /// Replaces each date whose option the matches of an update hold.
    ///
    /// # Errors
    ///
    /// Any error of [`update_value`].
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        update_value(&mut self.start_date, matches, "start_date")?;
        update_value(&mut self.end_date, matches, "end_date")
    }
}

/// The SYSIN record of `cbpaup0j`, kept exactly as typed until the job parses it.
#[derive(Debug, Clone)]
pub(crate) struct Cbpaup0jArgs {
    // CBPAUP0C-002: read as text by `ValueParser::string()` and parsed by no option parser, so a
    // rejected record never reaches a `clap` message.
    /// SYSIN record of CBPAUP0C: expiry days, checkpoint frequency, display frequency and debug
    /// flag, at most 80 printable ASCII characters
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

    /// `command` with `--sysin`, kept as typed: `ValueParser::string()` turns no record into a
    /// usage error that would repeat it (CBPAUP0C-002).
    fn options(command: Command) -> Command {
        let sysin = option("sysin", "sysin", "RECORD", ValueParser::string())
            .help(concat!(
                "SYSIN record of CBPAUP0C: expiry days, checkpoint frequency, display frequency ",
                "and debug flag, at most 80 printable ASCII characters",
            ))
            .default_value(DEFAULT_SYSIN);
        with_options(command, "Cbpaup0jArgs", [sysin])
    }

    /// The SYSIN record, from the matches of `cbpaup0j`.
    ///
    /// # Errors
    ///
    /// Any error of [`take_required`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            sysin: take_required(matches, "sysin")?,
        })
    }

    /// Replaces the SYSIN record when the matches of an update hold `--sysin`.
    ///
    /// # Errors
    ///
    /// Any error of [`update_value`].
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        update_value(&mut self.sysin, matches, "sysin")
    }
}

/// The input of `mnttrdb2`
#[derive(Debug, Clone)]
pub(crate) struct Mnttrdb2Args {
    // COBTUPDT-001: required and without default, so its absence is a usage error before any
    // database access; COBTUPDT-002 and COBTUPDT-003 give the record layout.
    /// Maintenance records, one 53-byte record per line: action, type code, description; required,
    /// as no repository source holds the INPFILE dataset
    pub(crate) input: PathBuf,
}

impl Mnttrdb2Args {
    /// `command` with `--input`, required and without default (COBTUPDT-001).
    fn options(command: Command) -> Command {
        let input = option("input", "input", "PATH", ValueParser::path_buf())
            .help(concat!(
                "Maintenance records, one 53-byte record per line: action, type code, ",
                "description; required, as no repository source holds the INPFILE dataset",
            ))
            .required(true);
        with_options(command, "Mnttrdb2Args", [input])
    }

    /// The input, from the matches of `mnttrdb2`. `clap` refuses `mnttrdb2` without `--input`,
    /// in a parse and in an update alike, while it matches the command line, so the matches of
    /// this command always hold the input.
    ///
    /// # Errors
    ///
    /// Any error of [`take_required`].
    fn read(matches: &mut ArgMatches) -> Result<Self, clap::Error> {
        Ok(Self {
            input: take_required(matches, "input")?,
        })
    }

    /// Replaces the input when the matches of an update hold `--input`.
    ///
    /// # Errors
    ///
    /// Any error of [`update_value`].
    fn update(&mut self, matches: &mut ArgMatches) -> Result<(), clap::Error> {
        update_value(&mut self.input, matches, "input")
    }
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

    // An update replaces the values its command line holds, defaults included, or the whole job
    // when it names another subcommand; a refused update leaves the job as it was. The
    // subcommand an update names requires what its parse requires.
    #[test]
    fn an_update_replaces_the_values_or_the_job_it_names() -> TestResult {
        let end_date = |job: &Job| match job {
            Job::Tranrept(args) => Ok(args.end_date.to_string()),
            other => Err(format!("updated to {other:?}")),
        };
        let mut cli = Cli::try_parse_from([BIN, "intcalc"])?;
        cli.try_update_from([BIN, "intcalc", "--parm-date", "2023010199XYZ"])?;
        match &cli.job {
            Job::Intcalc(args) => assert_eq!(args.parm_date.as_bytes(), b"2023010199"),
            other => return Err(format!("updated to {other:?}").into()),
        }
        cli.try_update_from([BIN, "intcalc"])?;
        match &cli.job {
            Job::Intcalc(args) => assert_eq!(args.parm_date, ParmDate::DEFAULT),
            other => return Err(format!("updated to {other:?}").into()),
        }
        cli.try_update_from([BIN, "tranrept", "--end-date", "2022-02-28"])?;
        match &cli.job {
            Job::Tranrept(args) => assert_eq!(args.start_date, DateParm::DEFAULT.start()),
            other => return Err(format!("updated to {other:?}").into()),
        }
        assert_eq!(end_date(&cli.job)?, "2022-02-28");
        cli.try_update_from([BIN])?;
        assert_eq!(end_date(&cli.job)?, "2022-02-28");

        let refused: [(&[&str], ErrorKind); 3] = [
            (&["intcalc", "--parm-date", "1"], ErrorKind::ValueValidation),
            (&["mnttrdb2"], ErrorKind::MissingRequiredArgument),
            (&["help"], ErrorKind::InvalidSubcommand),
        ];
        for (args, kind) in refused {
            let err = cli
                .try_update_from(std::iter::once(BIN).chain(args.iter().copied()))
                .err();
            assert_eq!(err.map(|e| e.kind()), Some(kind), "{args:?}");
            assert_eq!(end_date(&cli.job)?, "2022-02-28", "{args:?}");
        }

        // An update naming `mnttrdb2` without `--input` is refused as its parse is, with the
        // subcommand's usage, whether the job is `mnttrdb2` already or another one.
        let jobs: [&[&str]; 2] = [&["mnttrdb2", "--input", "maint.txt"], &["intcalc"]];
        for args in jobs {
            let mut cli = Cli::try_parse_from(std::iter::once(BIN).chain(args.iter().copied()))?;
            let job = format!("{:?}", cli.job);
            let err = cli
                .try_update_from([BIN, "mnttrdb2"])
                .err()
                .ok_or_else(|| format!("{args:?}: update accepted"))?;
            assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument, "{args:?}");
            assert_eq!(err.exit_code(), 2, "{args:?}");
            let rendered = err.to_string();
            assert!(
                rendered.contains("Usage: carddemo-batch mnttrdb2 --input <PATH>"),
                "{args:?}:\n{rendered}"
            );
            assert_eq!(format!("{:?}", cli.job), job, "{args:?}");
        }
        Ok(())
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
