//! The fixtures read in place, the fixture-loaded database the integration tests run against,
//! and the protected fixture values the redaction tests search for.
//!
//! # Fixtures read in place
//!
//! The fixtures are the eight ASCII master and reference files of `app/data/ASCII/` and the
//! in-stream `USRSEC` records of `app/jcl/DUSRSECJ.jcl`. They are read where they lie and never
//! copied (AAP 0.2.2). This module is where the integration tests of `persistence`, `api`,
//! `messaging` and `batch` find them; `copybook` and `domain`, below this crate in the crate
//! graph, resolve the same files from their own manifest directories (AAP 0.6.2).
//!
//! # The fixture-loaded database
//!
//! [`load_fixtures`] runs the nine loads of `carddemo-batch` with the semantics
//! `modern/audit/batch/LOADS.md` states for them, in the `scripts/dev-up.sh` order: `acctfile`,
//! `cardfile`, `xreffile`, `custfile`, `discgrp`, `tcatbalf`, `trancatg`, `trantype`,
//! `dusrsecj`, the refresh order of `scripts/remote_refresh.sh:16-44` less TRANFILE.
//!
//! - Codec A decodes every line of a load's input, and the record keys must rise strictly,
//!   before anything is written. A refused line fails the load; nothing is skipped or
//!   normalised.
//! - One transaction per table deletes every row and inserts every record through codec B, as
//!   IDCAMS `DELETE`, `DEFINE CLUSTER` and `REPRO` replace a cluster (AAP 0.3.3.4).
//! - `users` receives the argon2id hash of each `SEC-USR-PWD`, never the password (E25).
//! - The fixtures hold 50, 50, 50, 50, 51, 50, 18 and 7 records and the deck 10, the row counts
//!   each load must report (`modern/audit/fixtures.md`).
//!
//! No foreign key joins the loaded tables, so the loads succeed on an empty or a loaded
//! database, and a rerun leaves the same copybook-derived contents. Between two loads in one
//! process only `cards.card_ref`, which the database assigns anew to every inserted card,
//! differs: the harness hashes the deck once per process, and every load writes that same set
//! of salted argon2id hashes. A new process, such as another test binary, hashes the deck again
//! with fresh salts, as each separate `carddemo-batch dusrsecj` run does, because
//! `domain::batch::loads::hash_user` draws a new salt per call (LOADS-033, E25). Those runs
//! therefore store different `users.password_hash` strings that verify the same passwords.
//!
//! The loads write through the repository traits of `persistence` only: this module holds no
//! SQL text and applies no migration, which [`start_postgres`] has done before.
//!
//! # Redaction needles
//!
//! [`fixture_card_numbers`], [`fixture_ssns`], [`fixture_ssn_forms`] and
//! [`fixture_user_passwords`] return protected values in full: the 50 card numbers, the 50
//! SSNs and the 10 passwords of the fixtures. They exist only as search needles for the
//! redaction tests, which assert that no response, trace, error, broker message or log row
//! holds one (Cardholder Data Redaction). A caller never prints, traces or formats them into a
//! message, and no [`HarnessError`] of this module carries one: its contexts name the job or the
//! file, and its messages are the value-free `Display` texts of `LoadError`,
//! `PersistenceError` and `std::io::Error`.
//!
//! The needles are read through codec A rather than by byte offset. For reference, a card
//! number is bytes 0 to 15 of a `carddata.txt` line (`CARD-NUM`, `CVACT02Y`), an SSN bytes 279
//! to 287 of a `custdata.txt` line (`CUST-SSN`, `CVCUS01Y`) and a password bytes 48 to 55 of an
//! in-stream record (`SEC-USR-PWD`, `CSUSR01Y`), all 0-based.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use copybook::FixedRecord;
use persistence::domain::batch::loads::{
    HashedUser, LoadJob, check_key_sequence, decode_fixture, decode_usrsec, hash_user,
};
use persistence::domain::records::{
    AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DisclosureGroupRecord,
    TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord, UserSecurityRecord,
};
use persistence::{
    AccountRepository, CardRepository, CardXrefRepository, CustomerRepository,
    DisclosureGroupRepository, PersistenceError, PgStore, PgTx, TranCatBalanceRepository,
    TransactionCategoryRepository, TransactionTypeRepository, UserRepository,
};
use sqlx::PgPool;
use tokio::sync::OnceCell;
use tokio::task;

use crate::HarnessError;
use crate::postgres::{PostgresGuard, start_postgres};

/// The context of every error of [`load_fixtures`]; the message names the job.
const LOAD_CONTEXT: &str = "load fixtures";

/// The context of every error of the redaction needles.
const NEEDLE_CONTEXT: &str = "read redaction needles";

/// The `USRSEC` records with their argon2id hashes, computed once per process by a single
/// initialiser on Tokio's blocking pool.
///
/// argon2id is slow in unoptimised and coverage builds, and every load of `users` needs the ten
/// hashes. Each hash is salted and verifies its 8 bytes in any database, so one set serves every
/// load of the process. Concurrent first loads wait for the one computation in flight rather
/// than repeat it, and a failed computation stores nothing, so the next load computes again.
static HASHED_USERS: OnceCell<Vec<HashedUser>> = OnceCell::const_new();

/// The directory of the ASCII fixtures, `app/data/ASCII` under the repository root, for
/// example `/src/carddemo/modern/crates/test-support/../../../app/data/ASCII` for a checkout at
/// `/src/carddemo`.
///
/// The path is not canonicalised, as [`crate::repo_root`] is not.
#[must_use]
pub fn fixture_dir() -> PathBuf {
    crate::repo_root().join("app/data/ASCII")
}

/// The fixture `name` inside [`fixture_dir`], such as `"acctdata.txt"`. The file is read in
/// place and never copied.
#[must_use]
pub fn fixture_path(name: &str) -> PathBuf {
    fixture_dir().join(name)
}

/// The `DUSRSECJ` job deck, `app/jcl/DUSRSECJ.jcl` under the repository root, whose in-stream
/// records are the only ASCII source of the `USRSEC` users (AAP 0.2.1).
#[must_use]
pub fn dusrsecj_path() -> PathBuf {
    crate::repo_root().join("app/jcl/DUSRSECJ.jcl")
}

/// Replaces the nine tables the load subcommands of `carddemo-batch` write with the fixtures,
/// one table per transaction, in the `scripts/dev-up.sh` order, and returns each subcommand
/// with the number of rows its load wrote, in that order.
///
/// The database must already hold the schema, as [`start_postgres`] leaves it. Each load reads
/// its input in place, decodes every record with codec A and checks that the keys rise before
/// it opens its transaction, then deletes every row of its table and inserts every record
/// through codec B, so the result is the same as running `acctfile`, `cardfile`, `xreffile`,
/// `custfile`, `discgrp`, `tcatbalf`, `trancatg`, `trantype` and `dusrsecj` in turn. The call
/// can be repeated on the same database and reports the same counts.
///
/// # Errors
///
/// Returns a [`HarnessError`] when a fixture cannot be read, when codec A refuses a line or a
/// key does not rise above the previous one, when a password cannot be hashed or the blocking
/// task hashing them does not complete, when a transaction cannot be opened, a delete or an
/// insert fails or the commit is refused, or when a load inserts a number of rows other than its
/// fixture's 50, 50, 50, 50, 51, 50, 18, 7 or 10, which rolls that load back. Each failure
/// leaves its table as it was and stops the run, so the tables loaded before it keep their new
/// contents.
pub async fn load_fixtures(pool: &PgPool) -> Result<Vec<(String, u64)>, HarnessError> {
    let store = PgStore::new(pool.clone());
    let mut loaded = Vec::with_capacity(LoadJob::ALL.len());
    for job in LoadJob::ALL {
        let rows = load(&store, job).await?;
        loaded.push((job.subcommand().to_owned(), rows));
    }
    Ok(loaded)
}

/// Starts a `postgres:16` container holding the schema through [`start_postgres`] and loads
/// the fixtures into it with [`load_fixtures`]: the schema-ready, fixture-loaded database the
/// `api` and `messaging` integration tests run against. Dropping the guard removes the
/// container.
///
/// # Errors
///
/// Returns the [`HarnessError`] of [`start_postgres`] or of [`load_fixtures`]. On a load error
/// the guard drops before the error returns, which removes the container.
pub async fn start_loaded_postgres() -> Result<PostgresGuard, HarnessError> {
    let guard = start_postgres().await?;
    load_fixtures(guard.pool()).await?;
    Ok(guard)
}

/// The 50 card numbers of `carddata.txt`, in file order, each as its 16 digits.
///
/// Protected values (Cardholder Data Redaction): use them only as search needles in redaction
/// assertions, and never print, trace or format one into a message. `cardxref.txt` holds the
/// same 50 numbers.
///
/// # Errors
///
/// Returns a [`HarnessError`] when `carddata.txt` cannot be read, when codec A refuses a line
/// or a key does not rise, or when a card number is not UTF-8.
pub fn fixture_card_numbers() -> Result<Vec<String>, HarnessError> {
    decode_job::<CardRecord>(LoadJob::Cardfile)?
        .into_iter()
        .map(|card| {
            String::from_utf8(card.num.expose().as_bytes().to_vec()).map_err(|err| {
                HarnessError::new(NEEDLE_CONTEXT, &format_args!("carddata.txt: {err}"))
            })
        })
        .collect()
}

/// The 50 SSNs of `custdata.txt`, in file order, each as its 9 digits, leading zeros included.
///
/// Protected values (Cardholder Data Redaction): use them only as search needles in redaction
/// assertions, and never print, trace or format one into a message.
///
/// # Errors
///
/// Returns a [`HarnessError`] when `custdata.txt` cannot be read, or when codec A refuses a
/// line or a key does not rise.
pub fn fixture_ssns() -> Result<Vec<String>, HarnessError> {
    Ok(decode_job::<CustomerRecord>(LoadJob::Custfile)?
        .into_iter()
        .map(|customer| format!("{:09}", customer.ssn.expose().get()))
        .collect())
}

/// Every SSN of [`fixture_ssns`] in both forms a leak could take, the 9 digits and the
/// `NNN-NN-NNNN` form: 100 strings, each SSN's plain form followed by its dashed form.
///
/// Protected values (Cardholder Data Redaction): use them only as search needles in redaction
/// assertions, and never print, trace or format one into a message.
///
/// # Errors
///
/// Returns the [`HarnessError`] of [`fixture_ssns`], or one when an SSN is not 9 ASCII digits.
pub fn fixture_ssn_forms() -> Result<Vec<String>, HarnessError> {
    let mut forms = Vec::new();
    for ssn in fixture_ssns()? {
        let dashed = dashed_ssn(&ssn).ok_or_else(|| {
            HarnessError::new(
                NEEDLE_CONTEXT,
                &"custdata.txt: an SSN is not 9 ASCII digits",
            )
        })?;
        forms.push(ssn);
        forms.push(dashed);
    }
    Ok(forms)
}

/// The `SEC-USR-PWD` bytes of the 10 in-stream records of `DUSRSECJ.jcl`, in deck order, each
/// 8 bytes (`PASSWORD` for every fixture user).
///
/// Protected values (Cardholder Data Redaction): use them only as search needles in redaction
/// assertions, and never print, trace or format one into a message.
///
/// # Errors
///
/// Returns a [`HarnessError`] when `DUSRSECJ.jcl` cannot be read, holds no in-stream records,
/// or holds a record codec A refuses or a user ID that does not rise.
pub fn fixture_user_passwords() -> Result<Vec<Vec<u8>>, HarnessError> {
    Ok(decode_users()?
        .into_iter()
        .map(|user| user.pwd.expose().to_vec())
        .collect())
}

/// A table one load replaces, with the repository whose `replace_all` replaces it.
///
/// Every repository trait names the method `replace_all`, so each implementation names its
/// trait to choose one.
trait Table: Sized + Sync {
    /// Deletes every row of the table and inserts `rows` in slice order inside `tx`,
    /// returning how many rows were inserted.
    fn replace(
        tx: &mut PgTx<'_>,
        rows: &[Self],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

/// Implements [`Table`] for each row type by delegating to its repository's `replace_all`.
macro_rules! tables {
    ($($row:ty => $repository:ident;)+) => {$(
        impl Table for $row {
            fn replace(
                tx: &mut PgTx<'_>,
                rows: &[Self],
            ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
                $repository::replace_all(tx, rows)
            }
        }
    )+};
}

tables! {
    AccountRecord => AccountRepository;
    CardRecord => CardRepository;
    CardXrefRecord => CardXrefRepository;
    CustomerRecord => CustomerRepository;
    DisclosureGroupRecord => DisclosureGroupRepository;
    TranCatBalanceRecord => TranCatBalanceRepository;
    TransactionCategoryRecord => TransactionCategoryRepository;
    TransactionTypeRecord => TransactionTypeRepository;
    HashedUser => UserRepository;
}

/// Runs the load `job`: decodes its input, then replaces its table.
async fn load(store: &PgStore, job: LoadJob) -> Result<u64, HarnessError> {
    match job {
        LoadJob::Acctfile => replace(store, job, &decode_job::<AccountRecord>(job)?).await,
        LoadJob::Cardfile => replace(store, job, &decode_job::<CardRecord>(job)?).await,
        LoadJob::Xreffile => replace(store, job, &decode_job::<CardXrefRecord>(job)?).await,
        LoadJob::Custfile => replace(store, job, &decode_job::<CustomerRecord>(job)?).await,
        LoadJob::Discgrp => replace(store, job, &decode_job::<DisclosureGroupRecord>(job)?).await,
        LoadJob::Tcatbalf => replace(store, job, &decode_job::<TranCatBalanceRecord>(job)?).await,
        LoadJob::Trancatg => {
            replace(store, job, &decode_job::<TransactionCategoryRecord>(job)?).await
        }
        LoadJob::Trantype => replace(store, job, &decode_job::<TransactionTypeRecord>(job)?).await,
        LoadJob::Dusrsecj => replace(store, job, hashed_users().await?).await,
    }
}

/// Replaces the table of `T` with `rows` in one transaction of its own, which commits only when
/// the load inserted the row count of its fixture. Dropping the transaction on an early return
/// rolls the replacement back, so a failure leaves the table as it was.
async fn replace<T: Table>(store: &PgStore, job: LoadJob, rows: &[T]) -> Result<u64, HarnessError> {
    let mut tx = store
        .begin()
        .await
        .map_err(|err| store_error(job, "begin", &err))?;
    let written = T::replace(&mut tx, rows)
        .await
        .map_err(|err| store_error(job, "replace", &err))?;
    let expected = expected_rows(job);
    if written != expected {
        return Err(HarnessError::new(
            LOAD_CONTEXT,
            &format_args!(
                "{} inserted {written} rows, expected {expected}",
                job.subcommand()
            ),
        ));
    }
    tx.commit()
        .await
        .map_err(|err| store_error(job, "commit", &err))?;
    Ok(written)
}

/// The records of the fixture file of `job`, in file order: codec A over every line, then the
/// key-sequence check of the job, then the framing dropped, since no column stores it.
fn decode_job<R: FixedRecord>(job: LoadJob) -> Result<Vec<R>, HarnessError> {
    let Some(file) = job.fixture_file() else {
        return Err(HarnessError::new(
            LOAD_CONTEXT,
            &format_args!("{} loads no fixture file", job.subcommand()),
        ));
    };
    let bytes = read(&fixture_path(file))?;
    let framed = decode_fixture::<R>(&bytes).map_err(|err| decode_error(job, file, &err))?;
    check_key_sequence(job, &framed).map_err(|err| decode_error(job, file, &err))?;
    Ok(framed.into_iter().map(|line| line.record).collect())
}

/// The in-stream `USRSEC` records of `DUSRSECJ.jcl`, in deck order, their user IDs checked to
/// rise, their framing dropped.
fn decode_users() -> Result<Vec<UserSecurityRecord>, HarnessError> {
    let deck = read(&dusrsecj_path())?;
    let framed = decode_usrsec(&deck)
        .map_err(|err| decode_error(LoadJob::Dusrsecj, "DUSRSECJ.jcl", &err))?;
    Ok(framed.into_iter().map(|line| line.record).collect())
}

/// The `USRSEC` records with their argon2id hashes, kept in [`HASHED_USERS`]: the first call of
/// the process computes them through [`hash_users`] on Tokio's blocking pool, calls made while
/// that computation runs wait for it, and every later call reuses the stored set.
async fn hashed_users() -> Result<&'static [HashedUser], HarnessError> {
    init_once(&HASHED_USERS, LoadJob::Dusrsecj, hash_users)
        .await
        .map(Vec::as_slice)
}

/// The `USRSEC` records of [`decode_users`] with their argon2id hashes. The ten hashes block for
/// as long as argon2id takes, so [`hashed_users`] runs this off the async workers.
fn hash_users() -> Result<Vec<HashedUser>, HarnessError> {
    decode_users()?
        .into_iter()
        .map(hash_user)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| decode_error(LoadJob::Dusrsecj, "DUSRSECJ.jcl", &err))
}

/// The value of `cell`, which the first caller computes with `compute` for the load `job`.
///
/// `compute` blocks, so it runs on Tokio's blocking pool: on a current-thread runtime the
/// caller's worker is the only thread that drives tasks and timers. Callers arriving while one
/// computation runs wait for it rather than start their own. An error, or a `compute` that
/// panics or is cancelled, leaves `cell` empty, so the next call computes again. The error of a
/// task that did not complete names the job and the outcome only, never the panic message.
async fn init_once<T: Send + Sync + 'static>(
    cell: &'static OnceCell<T>,
    job: LoadJob,
    compute: fn() -> Result<T, HarnessError>,
) -> Result<&'static T, HarnessError> {
    cell.get_or_try_init(|| async move {
        task::spawn_blocking(compute).await.map_err(|err| {
            let outcome = if err.is_panic() {
                "panicked"
            } else {
                "was cancelled"
            };
            HarnessError::new(
                LOAD_CONTEXT,
                &format_args!("{} blocking task {outcome}", job.subcommand()),
            )
        })?
    })
    .await
}

/// The row count the fixture of `job` holds (`modern/audit/fixtures.md`).
const fn expected_rows(job: LoadJob) -> u64 {
    match job {
        LoadJob::Acctfile
        | LoadJob::Cardfile
        | LoadJob::Xreffile
        | LoadJob::Custfile
        | LoadJob::Tcatbalf => 50,
        LoadJob::Discgrp => 51,
        LoadJob::Trancatg => 18,
        LoadJob::Trantype => 7,
        LoadJob::Dusrsecj => 10,
    }
}

/// The whole file at `path`. The error names the file, never its contents.
fn read(path: &Path) -> Result<Vec<u8>, HarnessError> {
    fs::read(path).map_err(|err| {
        let name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        HarnessError::new("read fixture", &format_args!("{name}: {err}"))
    })
}

/// `ssn`, 9 ASCII digits, as `NNN-NN-NNNN`, or `None` for any other text.
fn dashed_ssn(ssn: &str) -> Option<String> {
    if ssn.len() != 9 || !ssn.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some(format!(
        "{}-{}-{}",
        ssn.get(0..3)?,
        ssn.get(3..5)?,
        ssn.get(5..9)?
    ))
}

/// A decode, key or hash failure of `job` over `file`; `err` is a value-free `LoadError`.
fn decode_error(job: LoadJob, file: &str, err: &dyn fmt::Display) -> HarnessError {
    HarnessError::new(
        LOAD_CONTEXT,
        &format_args!("{} over {file}: {err}", job.subcommand()),
    )
}

/// A store failure of `job` at `step`; `err` is a value-free `PersistenceError`.
fn store_error(job: LoadJob, step: &str, err: &dyn fmt::Display) -> HarnessError {
    HarnessError::new(
        LOAD_CONTEXT,
        &format_args!("{} {step}: {err}", job.subcommand()),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::error::Error;
    use std::future;
    use std::path::Path;
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Poll;
    use std::thread::{self, ThreadId};

    use copybook::lines;
    use persistence::domain::batch::loads::{LoadJob, usrsec_instream_lines};
    use persistence::domain::password;
    use persistence::domain::records::{
        AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, DisclosureGroupRecord,
        TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord,
    };
    use tokio::sync::OnceCell;

    use super::{
        LOAD_CONTEXT, dashed_ssn, decode_job, decode_users, dusrsecj_path, expected_rows,
        fixture_card_numbers, fixture_dir, fixture_path, fixture_ssn_forms, fixture_ssns,
        fixture_user_passwords, hashed_users, init_once, load_fixtures, read,
        start_loaded_postgres,
    };
    use crate::HarnessError;
    use crate::tests::block_on;

    type TestResult = Result<(), Box<dyn Error>>;

    /// The subcommands and row counts every run of the nine loads reports, in load order.
    const LOADED: [(&str, u64); 9] = [
        ("acctfile", 50),
        ("cardfile", 50),
        ("xreffile", 50),
        ("custfile", 50),
        ("discgrp", 51),
        ("tcatbalf", 50),
        ("trancatg", 18),
        ("trantype", 7),
        ("dusrsecj", 10),
    ];

    /// The bytes `start..end` of every line of the fixture `file`, read by offset as the
    /// verification reference of the module documentation.
    fn columns(file: &str, start: usize, end: usize) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
        let bytes = read(&fixture_path(file))?;
        lines(&bytes)
            .map(|line| {
                line.body
                    .get(start..end)
                    .map(<[u8]>::to_vec)
                    .ok_or_else(|| format!("{file}: a line is shorter than {end} bytes").into())
            })
            .collect()
    }

    /// The 1-based position of the first element at which `left` and `right` differ, or `None`
    /// when they are equal. A slice that ends first differs at its length plus one.
    ///
    /// The needle tests report this position instead of comparing with `assert_eq!`, whose
    /// failure prints both operands and so the protected values.
    fn first_difference<T: PartialEq>(left: &[T], right: &[T]) -> Option<usize> {
        (1_usize..)
            .zip(left.iter().zip(right))
            .find_map(|(position, (one, other))| (one != other).then_some(position))
            .or_else(|| (left.len() != right.len()).then(|| left.len().min(right.len()) + 1))
    }

    /// The paths name the fixtures where they lie, under the repository root.
    #[test]
    fn paths_name_the_fixtures_in_place() {
        assert!(fixture_dir().ends_with("app/data/ASCII"));
        assert_eq!(
            fixture_path("acctdata.txt"),
            fixture_dir().join("acctdata.txt")
        );
        for job in LoadJob::ALL {
            let path = job.fixture_file().map_or_else(dusrsecj_path, fixture_path);
            assert!(path.is_file(), "{}", path.display());
        }
    }

    /// Every fixture decodes to the row count its load must report, in the order of
    /// [`LoadJob::ALL`].
    #[test]
    fn every_fixture_holds_the_rows_its_load_reports() -> TestResult {
        let decoded = [
            decode_job::<AccountRecord>(LoadJob::Acctfile)?.len(),
            decode_job::<CardRecord>(LoadJob::Cardfile)?.len(),
            decode_job::<CardXrefRecord>(LoadJob::Xreffile)?.len(),
            decode_job::<CustomerRecord>(LoadJob::Custfile)?.len(),
            decode_job::<DisclosureGroupRecord>(LoadJob::Discgrp)?.len(),
            decode_job::<TranCatBalanceRecord>(LoadJob::Tcatbalf)?.len(),
            decode_job::<TransactionCategoryRecord>(LoadJob::Trancatg)?.len(),
            decode_job::<TransactionTypeRecord>(LoadJob::Trantype)?.len(),
            decode_users()?.len(),
        ];
        for ((job, count), (subcommand, rows)) in LoadJob::ALL.into_iter().zip(decoded).zip(LOADED)
        {
            assert_eq!(job.subcommand(), subcommand);
            assert_eq!(expected_rows(job), rows, "{subcommand}");
            assert_eq!(u64::try_from(count)?, rows, "{subcommand}");
        }
        Ok(())
    }

    /// A job without a fixture file and an unreadable file fail with the job or the file name
    /// and the reason, nothing more.
    #[test]
    fn failures_name_the_job_or_the_file_only() {
        let no_file = decode_job::<AccountRecord>(LoadJob::Dusrsecj).map(|rows| rows.len());
        assert_eq!(
            no_file.map_err(|err| err.to_string()),
            Err("load fixtures: dusrsecj loads no fixture file".to_owned())
        );

        let missing = read(Path::new("/nonexistent/carddemo-fixture.txt")).map(|bytes| bytes.len());
        let Err(err) = missing else {
            panic!("a missing file was read");
        };
        let text = err.to_string();
        assert!(
            text.starts_with("read fixture: carddemo-fixture.txt: "),
            "{text}"
        );
        assert!(!text.contains("/nonexistent"), "{text}");
    }

    /// The card numbers are the 16-digit `CARD-NUM` of every `carddata.txt` line, the same 50
    /// values as the `XREF-CARD-NUM` of `cardxref.txt`. A failure names a line or a count,
    /// never a card number.
    #[test]
    fn card_numbers_are_the_card_num_field_of_every_card() -> TestResult {
        let numbers = fixture_card_numbers()?;
        assert_eq!(numbers.len(), 50);
        assert!(
            numbers.first().map(String::as_str) == Some("0500024453765740"),
            "carddata.txt line 1: CARD-NUM is not the audited first card number"
        );
        assert!(
            numbers
                .iter()
                .all(|number| number.len() == 16 && number.bytes().all(|b| b.is_ascii_digit()))
        );

        let by_offset = columns("carddata.txt", 0, 16)?;
        let as_bytes: Vec<Vec<u8>> = numbers
            .iter()
            .map(|number| number.clone().into_bytes())
            .collect();
        let mismatch = first_difference(&as_bytes, &by_offset);
        assert!(
            mismatch.is_none(),
            "carddata.txt line {}: codec A's CARD-NUM differs from bytes 0..16",
            mismatch.unwrap_or_default()
        );

        let cards: BTreeSet<Vec<u8>> = by_offset.into_iter().collect();
        let xrefs: BTreeSet<Vec<u8>> = columns("cardxref.txt", 0, 16)?.into_iter().collect();
        assert_eq!(cards.len(), 50);
        let unmatched = cards.symmetric_difference(&xrefs).count();
        assert!(
            unmatched == 0,
            "cardxref.txt: {unmatched} card numbers are not in both carddata.txt and cardxref.txt"
        );
        Ok(())
    }

    /// The SSNs are the 9-digit `CUST-SSN` of every `custdata.txt` line, and each appears plain
    /// and dashed among the forms. A failure names a line, never an SSN.
    #[test]
    fn ssns_are_the_cust_ssn_field_in_both_forms() -> TestResult {
        let ssns = fixture_ssns()?;
        assert_eq!(ssns.len(), 50);
        assert!(
            ssns.first().map(String::as_str) == Some("020973888"),
            "custdata.txt line 1: CUST-SSN is not the audited first SSN"
        );
        let as_bytes: Vec<Vec<u8>> = ssns.iter().map(|ssn| ssn.clone().into_bytes()).collect();
        let mismatch = first_difference(&as_bytes, &columns("custdata.txt", 279, 288)?);
        assert!(
            mismatch.is_none(),
            "custdata.txt line {}: codec A's CUST-SSN differs from bytes 279..288",
            mismatch.unwrap_or_default()
        );

        let forms = fixture_ssn_forms()?;
        assert_eq!(forms.len(), 100);
        assert!(
            forms.get(..2) == Some(["020973888".to_owned(), "020-97-3888".to_owned()].as_slice()),
            "custdata.txt line 1: the forms are not the audited first SSN, plain then dashed"
        );
        for (line, (pair, ssn)) in (1_usize..).zip(forms.chunks(2).zip(&ssns)) {
            assert!(
                pair.first() == Some(ssn),
                "custdata.txt line {line}: the plain form is not CUST-SSN"
            );
            assert!(
                pair.get(1).cloned() == dashed_ssn(ssn),
                "custdata.txt line {line}: the dashed form does not follow the plain form"
            );
        }
        Ok(())
    }

    /// Only 9 ASCII digits take the dashed form.
    #[test]
    fn dashed_ssn_accepts_nine_digits_only() {
        assert_eq!(dashed_ssn("123456789").as_deref(), Some("123-45-6789"));
        for text in [
            "",
            "12345678",
            "1234567890",
            "12345678a",
            "123-45-678",
            "１２３456789",
        ] {
            assert_eq!(dashed_ssn(text), None, "{text}");
        }
    }

    /// The passwords are the 8-byte `SEC-USR-PWD` of the ten in-stream records. A failure names
    /// the deck or a record, never a password.
    #[test]
    fn passwords_are_the_sec_usr_pwd_field_of_every_user() -> TestResult {
        let passwords = fixture_user_passwords()?;
        assert_eq!(passwords.len(), 10);
        assert!(
            passwords.iter().all(|pwd| pwd.as_slice() == b"PASSWORD"),
            "DUSRSECJ.jcl: a SEC-USR-PWD is not the audited fixture password"
        );

        let deck = read(&dusrsecj_path())?;
        let by_offset = usrsec_instream_lines(&deck)?
            .into_iter()
            .map(|line| {
                line.body
                    .get(48..56)
                    .map(<[u8]>::to_vec)
                    .ok_or("a short card")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mismatch = first_difference(&passwords, &by_offset);
        assert!(
            mismatch.is_none(),
            "DUSRSECJ.jcl in-stream record {}: codec A's SEC-USR-PWD differs from bytes 48..56",
            mismatch.unwrap_or_default()
        );
        Ok(())
    }

    /// The users are hashed once per process, and every stored hash verifies its record's
    /// password.
    #[test]
    fn hashed_users_are_computed_once_and_verify() -> TestResult {
        block_on(async {
            let first = hashed_users().await?;
            let again = hashed_users().await?;
            assert!(ptr::eq(first, again));
            assert_eq!(first.len(), 10);
            for user in first {
                assert!(user.password_hash.starts_with("$argon2id$"));
                assert!(password::verify(
                    user.record.pwd.expose(),
                    &user.password_hash
                )?);
            }
            Ok(())
        })
    }

    /// A failed or panicking computation leaves the cell empty and the next call computes
    /// again; concurrent first callers then share one computation, which runs on the blocking
    /// pool rather than on the thread of this current-thread runtime.
    #[test]
    fn init_once_computes_once_off_the_caller_thread() -> TestResult {
        /// A computation for [`init_once`], counted in `CALLS`.
        type Compute = fn() -> Result<ThreadId, HarnessError>;
        static CELL: OnceCell<ThreadId> = OnceCell::const_new();
        static CALLS: AtomicUsize = AtomicUsize::new(0);

        let refuses: Compute = || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Err(HarnessError::new(LOAD_CONTEXT, &"refused"))
        };
        let panics: Compute = || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            panic!("deliberate panic of the init_once test");
        };
        let computes: Compute = || {
            CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(thread::current().id())
        };

        block_on(async {
            for (compute, expected) in [
                (refuses, "load fixtures: refused"),
                (panics, "load fixtures: dusrsecj blocking task panicked"),
            ] {
                let failed = init_once(&CELL, LoadJob::Dusrsecj, compute).await.copied();
                assert_eq!(
                    failed.map_err(|err| err.to_string()),
                    Err(expected.to_owned())
                );
                assert!(CELL.get().is_none(), "{expected}");
            }
            assert_eq!(CALLS.load(Ordering::SeqCst), 2);

            // Four first callers in this one task: each poll polls every caller that has no
            // value yet, so all four wait on the cell at the same time.
            let mut callers =
                [(); 4].map(|()| Box::pin(init_once(&CELL, LoadJob::Dusrsecj, computes)));
            let mut values = [(); 4].map(|()| None);
            future::poll_fn(|cx| {
                for (caller, value) in callers.iter_mut().zip(&mut values) {
                    if value.is_none() {
                        if let Poll::Ready(outcome) = caller.as_mut().poll(cx) {
                            *value = Some(outcome);
                        }
                    }
                }
                if values.iter().all(Option::is_some) {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
            let [Some(first), Some(second), Some(third), Some(fourth)] = values else {
                panic!("the join ended before every caller had its value");
            };
            let first = first?;
            for other in [second?, third?, fourth?] {
                assert!(ptr::eq(first, other));
            }
            assert_eq!(CALLS.load(Ordering::SeqCst), 3);
            assert_ne!(*first, thread::current().id());
            Ok(())
        })
    }

    /// The loaded database reports the nine counts, and a second run over the same database
    /// reports them again.
    #[test]
    fn the_loads_fill_the_nine_tables_and_rerun() -> Result<(), Box<dyn Error>> {
        block_on(async {
            let guard = start_loaded_postgres().await?;
            let expected: Vec<(String, u64)> = LOADED
                .iter()
                .map(|&(subcommand, rows)| (subcommand.to_owned(), rows))
                .collect();
            assert_eq!(load_fixtures(guard.pool()).await?, expected);
            assert_eq!(load_fixtures(guard.pool()).await?, expected);
            Ok(())
        })
    }
}
