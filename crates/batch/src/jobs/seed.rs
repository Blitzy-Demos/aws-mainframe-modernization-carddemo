//! `batch seed`: the jobs that build and reload the `CardDemo` files.
//!
//! | Options | Jobs replaced |
//! |---|---|
//! | none | The README refresh sequence: `ACCTFILE`, `CARDFILE`, `CUSTFILE`, `XREFFILE`, `DISCGRP`, `TCATBALF`, `TRANTYPE`, `TRANCATG` (IDCAMS `REPRO`) and `DUSRSECJ` (IEBGENER, `REPRO`), plus the `CREADB21` loads `LDTTYPE`/`LDTCCAT` [app/app-transaction-type-db2/jcl/CREADB21.jcl:L64-84] while the Db2 masters are empty |
//! | `--if-empty` | The same, on a first start only: nothing happens when the account master holds rows |
//! | `--only tranextr` | `TRANEXTR` STEP40/STEP50 (DSNTIAUL) [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L62-122] and the `TRANTYPE`/`TRANCATG` loads of its unload: the Db2 masters copied over the processing copies |
//!
//! The loading itself is `carddemo_persistence::seed`, which decodes every fixture through the
//! `carddemo-codec` layouts, so the database holds the bytes the codec round-trips (o4). It runs
//! in two steps: `seed::prepare` reads, decodes and hashes the sources, blocking file I/O and
//! argon2id work that this job runs on Tokio's blocking pool before any transaction is open;
//! `seed::run` then writes the prepared rows in one database transaction. This job only locates
//! the sources, runs the two steps and turns the outcome into an exit status. Seed jobs are
//! replaced, not parity-tested (AAP 0.3.2), so no audit row gates this module.

use std::path::Path;

use carddemo_persistence::seed::{self, SeedMode, SeedReport, SeedSources};
use carddemo_persistence::tx::Db;
use carddemo_runtime::config::Settings;
use tracing::Span;

use super::{JobContext, count_of, setup_failure, span_count};
use crate::cli::SeedArgs;
use crate::exit::ExitStatus;
use crate::generations::blocking;

/// Runs the seed `args` selects against `db` and records the rows it wrote in `written` on
/// the job span, and sets them in `read` and `posted` in the job's counts too, since every row
/// read from a source is loaded. `rejected` stays 0, since a source that does not decode fails
/// the run rather than leaving a row out, and a run that fails leaves every count 0 and writes
/// no row, unless its `COMMIT` failed without the server's answer, its connection lost: the run
/// may then have committed, so the database is checked before the seed is run again.
///
/// The sources resolve against the current directory, the repository root the runbook runs
/// every command from: the fixture directory is `CARDDEMO_FIXTURE_DIR` (default
/// `app/data/ASCII`), and the `DUSRSECJ` job and the `CREADB21` load statements stay at their
/// repository paths. `--if-empty` on a seeded database writes nothing and ends normally. Once
/// the sources are resolved, and before any is read, what the run's mode reads is recorded in
/// `input` on the job span ([`span_input`]); the mode itself is the span's `mode`.
///
/// The sources are prepared on Tokio's blocking pool, inside the job span, and only then is the
/// seed's transaction opened, so no worker thread and no connection is held while the files are
/// read and the passwords hashed. A source that cannot be read or decoded fails the run at the
/// point the load reaches it, so `--if-empty` on a seeded database still ends normally whatever
/// the sources hold.
///
/// A source that cannot be read or decoded, or a failed statement, is a setup failure
/// (exit 16): the seed jobs have no condition code the work reproduces. The seed's single
/// transaction is then rolled back, so the database is left as it was. A preparation the
/// runtime cancels because it is shutting down is a setup failure too, before any transaction.
pub(crate) async fn run(ctx: &JobContext, db: &Db, args: &SeedArgs) -> ExitStatus {
    let repo_root = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => return setup_failure("current directory unavailable", err),
    };
    let sources = seed_sources(&repo_root, &ctx.settings);
    let mode = args.mode();
    Span::current().record("input", span_input(mode, &repo_root, &sources));
    let prepared = match blocking(move || seed::prepare(mode, &sources)).await {
        Ok(prepared) => prepared,
        Err(err) => return setup_failure("seed preparation did not complete", err),
    };
    match seed::run(db, prepared).await {
        Ok(report) => {
            // `seed::run` already emits the per-table counts and the refreshed,
            // masters-loaded and skipped flags as one event inside this span, so the job adds
            // only the span's totals rather than a second event with the same counts.
            let rows = rows_written(&report);
            ctx.counts.set_read(count_of(rows));
            ctx.counts.set_posted(count_of(rows));
            Span::current().record("written", span_count(rows));
            ExitStatus::Success
        }
        Err(err) => setup_failure("seed failed", err),
    }
}

/// The seed sources of the repository at `repo_root`, with the fixture directory of
/// `settings`. A relative `CARDDEMO_FIXTURE_DIR` resolves against `repo_root`, an absolute one
/// stands as given.
fn seed_sources(repo_root: &Path, settings: &Settings) -> SeedSources {
    SeedSources {
        fixture_dir: repo_root.join(&settings.fixture_dir),
        ..SeedSources::with_defaults(repo_root)
    }
}

/// The `input` of a [`SeedMode::OnlyTranextr`] run, which reads no file: the Db2 masters it
/// copies, the tables that hold what `TRANEXTR` STEP40 and STEP50 unload from
/// `TRANSACTION_TYPE` and `TRANSACTION_TYPE_CATEGORY`
/// [app/app-transaction-type-db2/jcl/TRANEXTR.jcl:L65-84, L95-115].
const TRANEXTR_INPUT: &str = "transaction_types,transaction_categories";

/// The `input` the job span records for a run in `mode` on `sources`, its parts comma-joined.
///
/// [`SeedMode::Full`] and [`SeedMode::IfEmpty`] read the fixture directory, the `DUSRSECJ` job
/// and the `CREADB21` load statement directory, whether or not `--if-empty` then writes, so the
/// input is those three paths, each relative to `repo_root` when it lies under it and as given
/// otherwise: `app/data/ASCII,app/jcl/DUSRSECJ.jcl,app/app-transaction-type-db2/ctl` by default.
/// [`SeedMode::OnlyTranextr`] reads only the database, so its input is [`TRANEXTR_INPUT`]. Only
/// paths are recorded, never what a source holds.
fn span_input(mode: SeedMode, repo_root: &Path, sources: &SeedSources) -> String {
    match mode {
        SeedMode::OnlyTranextr => TRANEXTR_INPUT.to_owned(),
        SeedMode::Full | SeedMode::IfEmpty => format!(
            "{},{},{}",
            shown(repo_root, &sources.fixture_dir).display(),
            shown(repo_root, &sources.dusrsecj_jcl).display(),
            shown(repo_root, &sources.db2_ctl_dir).display(),
        ),
    }
}

/// `path` as [`span_input`] shows it: relative to `repo_root` when it lies under it, `.` when it
/// is `repo_root` itself (such as `CARDDEMO_FIXTURE_DIR=.`), and as given otherwise.
fn shown<'a>(repo_root: &Path, path: &'a Path) -> &'a Path {
    match path.strip_prefix(repo_root) {
        Ok(relative) if relative.as_os_str().is_empty() => Path::new("."),
        Ok(relative) => relative,
        Err(_) => path,
    }
}

/// The rows `report` says were inserted, over every table. The destructuring names every
/// field, so a count added to [`SeedReport`] fails to compile here until it is summed.
fn rows_written(report: &SeedReport) -> usize {
    let SeedReport {
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
        reward_rules,
        masters_loaded: _,
        refreshed: _,
        skipped: _,
    } = report;
    [
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
        reward_rules,
    ]
    .into_iter()
    .sum()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use carddemo_persistence::seed::{SeedMode, SeedReport, SeedSources};
    use carddemo_runtime::config::Settings;

    use super::{rows_written, seed_sources, span_input};

    #[test]
    fn fixture_dir_comes_from_the_settings_and_the_rest_from_the_repository() {
        let root = Path::new("/repo");
        let cases = [
            (PathBuf::from("app/data/ASCII"), root.join("app/data/ASCII")),
            (PathBuf::from("/data/ascii"), PathBuf::from("/data/ascii")),
        ];
        for (configured, expected) in cases {
            let settings = Settings {
                fixture_dir: configured.clone(),
                ..Settings::default()
            };
            let sources = seed_sources(root, &settings);
            let defaults = SeedSources::with_defaults(root);
            assert_eq!(sources.fixture_dir, expected, "{}", configured.display());
            assert_eq!(sources.dusrsecj_jcl, defaults.dusrsecj_jcl);
            assert_eq!(sources.db2_ctl_dir, defaults.db2_ctl_dir);
        }
    }

    #[test]
    fn the_span_input_names_what_the_mode_reads_relative_to_the_repository() {
        let root = Path::new("/repo");
        let jcl_and_ctl = "app/jcl/DUSRSECJ.jcl,app/app-transaction-type-db2/ctl";
        let default_input = format!("app/data/ASCII,{jcl_and_ctl}");
        let tranextr = "transaction_types,transaction_categories";
        // (mode, CARDDEMO_FIXTURE_DIR, input)
        let cases = [
            (SeedMode::Full, "app/data/ASCII", default_input.clone()),
            (SeedMode::IfEmpty, "app/data/ASCII", default_input),
            (
                SeedMode::OnlyTranextr,
                "app/data/ASCII",
                tranextr.to_owned(),
            ),
            (
                SeedMode::Full,
                "/data/ascii",
                format!("/data/ascii,{jcl_and_ctl}"),
            ),
            (
                SeedMode::IfEmpty,
                "../shared/ascii",
                format!("../shared/ascii,{jcl_and_ctl}"),
            ),
            (SeedMode::Full, ".", format!(".,{jcl_and_ctl}")),
            (SeedMode::OnlyTranextr, "/data/ascii", tranextr.to_owned()),
        ];
        for (mode, fixture_dir, expected) in cases {
            let settings = Settings {
                fixture_dir: PathBuf::from(fixture_dir),
                ..Settings::default()
            };
            let sources = seed_sources(root, &settings);
            assert_eq!(
                span_input(mode, root, &sources),
                expected,
                "{mode:?} with {fixture_dir}"
            );
        }
    }

    #[test]
    fn rows_written_sums_every_count_and_no_flag() {
        let cases = [
            (SeedReport::default(), 0),
            (
                SeedReport {
                    skipped: true,
                    ..SeedReport::default()
                },
                0,
            ),
            (
                SeedReport {
                    trantype_file: 7,
                    trancatg_file: 18,
                    ..SeedReport::default()
                },
                25,
            ),
            (
                SeedReport {
                    accounts: 50,
                    cards: 50,
                    card_xref: 50,
                    customers: 50,
                    disclosure_groups: 51,
                    tran_cat_balances: 50,
                    trantype_file: 7,
                    trancatg_file: 18,
                    users: 10,
                    transaction_types: 7,
                    transaction_categories: 18,
                    reward_rules: 2,
                    masters_loaded: true,
                    refreshed: false,
                    skipped: false,
                },
                363,
            ),
        ];
        for (report, expected) in cases {
            assert_eq!(rows_written(&report), expected, "{report:?}");
        }
    }
}
