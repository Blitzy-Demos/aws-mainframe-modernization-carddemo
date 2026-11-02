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

/// The `batch` command line.
#[derive(Debug, Parser)]
#[command(
    name = "batch",
    version,
    about = "CardDemo batch jobs (JCL replacement)",
    long_about = None
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
    #[command(name = "migrate")]
    Migrate,

    /// ACCTFILE, CARDFILE, CUSTFILE, XREFFILE, DISCGRP, TCATBALF, TRANTYPE,
    /// TRANCATG, DUSRSECJ (IDCAMS, IEBGENER): load or refresh the master files;
    /// TRANEXTR (DSNTIAUL) with --only tranextr.
    ///
    /// Without options the run is the refresh: every master file and
    /// processing copy is reloaded from the fixture corpus in one database
    /// transaction.
    #[command(name = "seed")]
    Seed(SeedArgs),

    /// POSTTRAN (CBTRN02C): post the daily transactions and write the rejects.
    #[command(name = "posttran")]
    Posttran(PosttranArgs),

    /// INTCALC (CBACT04C): compute the monthly interest and write the system
    /// transactions.
    #[command(name = "intcalc")]
    Intcalc(IntcalcArgs),

    /// TRANBKP (REPROC, IDCAMS DELETE/DEFINE): back up the transaction master,
    /// then empty it.
    #[command(name = "tranbkp")]
    Tranbkp,

    /// COMBTRAN (SORT, IDCAMS REPRO): combine the backup and the system
    /// transactions and reload the transaction master.
    #[command(name = "combtran")]
    Combtran,

    /// CREASTMT (CBSTM03A, CBSTM03B): produce the account statements as text,
    /// HTML and PDF.
    #[command(name = "creastmt")]
    Creastmt,

    /// TRANREPT (REPROC, SORT, CBTRN03C): back up the transaction master and
    /// report the transactions processed within a date range.
    #[command(name = "tranrept")]
    Tranrept(TranreptArgs),

    /// CBPAUP0J (CBPAUP0C): purge the expired pending authorizations.
    #[command(name = "cbpaup0j")]
    Cbpaup0j(Cbpaup0jArgs),

    /// MNTTRDB2 (COBTUPDT): add, update and delete Db2 transaction types from
    /// an input file.
    #[command(name = "mnttrdb2")]
    Mnttrdb2(Mnttrdb2Args),
}

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

    #[test]
    fn help_names_every_subcommand_and_default() {
        let mut command = Cli::command();
        command.build();
        let root_help = command.render_long_help().to_string();
        let cases: [(&str, &[&str]); 10] = [
            ("migrate", &["CREADB21 (DSNTEP4)"]),
            (
                "seed",
                &["DUSRSECJ", "TRANEXTR (DSNTIAUL)", "--if-empty", "--only"],
            ),
            (
                "posttran",
                &[
                    "POSTTRAN (CBTRN02C)",
                    "[default: app/data/ASCII/dailytran.txt]",
                ],
            ),
            ("intcalc", &["INTCALC (CBACT04C)", "[default: 2022071800]"]),
            ("tranbkp", &["TRANBKP (REPROC, IDCAMS DELETE/DEFINE)"]),
            ("combtran", &["COMBTRAN (SORT, IDCAMS REPRO)"]),
            ("creastmt", &["CREASTMT (CBSTM03A, CBSTM03B)"]),
            (
                "tranrept",
                &[
                    "TRANREPT (REPROC, SORT, CBTRN03C)",
                    "[default: 2022-01-01]",
                    "[default: 2022-07-06]",
                ],
            ),
            (
                "cbpaup0j",
                &["CBPAUP0J (CBPAUP0C)", "[default: 00,00001,00001,Y]"],
            ),
            ("mnttrdb2", &["MNTTRDB2 (COBTUPDT)", "--input <FILE>"]),
        ];
        for (name, expected) in cases {
            assert!(
                root_help.contains(name),
                "root help lacks {name}:\n{root_help}"
            );
            let sub_help = command
                .find_subcommand_mut(name)
                .unwrap_or_else(|| panic!("{name} is a subcommand"))
                .render_long_help()
                .to_string();
            for text in expected {
                assert!(
                    sub_help.contains(text),
                    "{name} help lacks {text:?}:\n{sub_help}"
                );
            }
        }
    }
}
