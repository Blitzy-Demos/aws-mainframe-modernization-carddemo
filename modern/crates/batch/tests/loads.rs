//! Integration tests of the nine load subcommands of `carddemo-batch`: binding g3b for
//! `modern/audit/batch/LOADS.md` and the load rows of `modern/audit/fixtures.md` (`FIXLOAD`,
//! `FIXUSR`).
//!
//! Each test runs the real binary as a subprocess against its own `postgres:16` testcontainer,
//! migrated by the `test-support` harness, and starts it in `modern/`, the working directory of
//! every binary, so the relative defaults `../app/data/ASCII` and `../app/jcl/DUSRSECJ.jcl`
//! resolve as they do for an operator. The fixtures are read in place. The inputs no fixture
//! holds (a subset, disjoint slices for two concurrent runs, a refused line, keys out of
//! sequence, broken decks) are scratch files in the test's temporary directory, passed with
//! `--input`, as LOADS.md ("How the rows are proved") prescribes. State is read back only
//! through the `persistence` repository traits (R4). The concurrent-load test also observes the
//! server's lock waits in its log, which `PostgresGuard::server_log` reads through Docker with
//! no SQL.
//!
//! Expected values are transcribed from the audit rows and the fixture bytes they cite, never
//! from the code under test, and each assertion carries the rule IDs it proves. Expected records
//! are the fixture lines decoded by codec A at the `copybook` level, never through the
//! `domain::batch::loads` path the binary decodes with, and a loaded row must also re-encode to
//! its line's bytes. LOADS.md names no halt, so no assertion here is blocked. This crate issues
//! no query (R4), so LOADS-020, the unchanged catalog, and the clauses of FIXLOAD-011 no
//! repository reads whole (`auth_fraud`, `pending_auth_details`, the whole of schema `db2`) are
//! proved in `crates/persistence/tests/schema.rs`, over the loads' statements. This file proves
//! FIXLOAD-011's `var/` clause and its empty tables that a repository reads whole
//! (`transactions`, `pending_auth_summaries`, `cssl_error_log`) against the real binary.
//!
//! Cardholder Data Redaction: the fixtures hold card numbers, CVVs, SSNs and passwords. Records
//! and input bytes are compared in memory and never printed; a failed comparison names the table
//! and the row index, or the file, and nothing else. The server log, which can quote statement
//! texts, is parsed in memory too, and a failure names only the wait it lacks.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fs;
use std::future::poll_fn;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use copybook::framing::decode_all;
use copybook::{FixedRecord, Framed, PicU, PicX};
use domain::online::cocrdlic::CardRef;
use domain::password;
use domain::records::{
    AccountRecord, CardRecord, CardXrefRecord, CustomerRecord, Db2TransactionType,
    DisclosureGroupRecord, TranCatBalanceRecord, TransactionCategoryRecord, TransactionTypeRecord,
    UserEntry,
};
use persistence::{
    AccountRepository, CardRepository, CardXrefRepository, CsslErrorLogRepository,
    CustomerRepository, Db2TransactionTypeRepository, DisclosureGroupRepository,
    PendingAuthSummaryRepository, PgStore, TranCatBalanceRepository, TransactionCategoryRepository,
    TransactionRepository, TransactionTypeRepository, UserRepository,
};
use test_support::fixtures::{
    dusrsecj_path, fixture_card_numbers, fixture_dir, fixture_path, fixture_ssn_forms,
    fixture_user_passwords,
};
use test_support::modern_dir;
use test_support::postgres::{PostgresGuard, start_postgres};
use tokio::sync::oneshot;
use tokio::time::{self, Instant};

type TestResult<T = ()> = Result<T, Box<dyn Error>>;

/// Runs `test` to completion on a multi-thread runtime with every driver enabled and the default
/// worker count, the one Tokio's test attribute builds for its `multi_thread` flavour, without the
/// lint attributes, `expect` and unchecked pin that attribute's expansion carries.
fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(test)
}

/// The nine load subcommands in `scripts/dev-up.sh` order, the refresh order of
/// `scripts/remote_refresh.sh:16-44` less TRANFILE (LOADS.md; LOADS-001 to LOADS-009).
const DEV_UP_ORDER: [&str; 9] = [
    "acctfile", "cardfile", "xreffile", "custfile", "discgrp", "tcatbalf", "trancatg", "trantype",
    "dusrsecj",
];

/// The eight `app/data/ASCII` fixtures the loads read, in the same order (LOADS-001 to
/// LOADS-008).
const FIXTURES: [&str; 8] = [
    "acctdata.txt",
    "carddata.txt",
    "cardxref.txt",
    "custdata.txt",
    "discgrp.txt",
    "tcatbal.txt",
    "trancatg.txt",
    "trantype.txt",
];

/// The ten in-stream users of `DUSRSECJ.jcl:35-44` in deck order, which is key order:
/// `SEC-USR-ID`, `SEC-USR-FNAME` and `SEC-USR-LNAME` without their space padding, and
/// `SEC-USR-TYPE` (FIXUSR-001 to FIXUSR-010, FIXLOAD-009).
const USERS: [(&str, &str, &str, &str); 10] = [
    ("ADMIN001", "MARGARET", "GOLD", "A"),
    ("ADMIN002", "RUSSELL", "RUSSELL", "A"),
    ("ADMIN003", "RAYMOND", "WHITMORE", "A"),
    ("ADMIN004", "EMMANUEL", "CASGRAIN", "A"),
    ("ADMIN005", "GRANVILLE", "LACHAPELLE", "A"),
    ("USER0001", "LAWRENCE", "THOMAS", "U"),
    ("USER0002", "AJITH", "KUMAR", "U"),
    ("USER0003", "LAURITZ", "ALME", "U"),
    ("USER0004", "AVERARDO", "MAZZI", "U"),
    ("USER0005", "LEE", "TING", "U"),
];

/// The Db2 data version of the transaction types, as migration 0006 seeds it from
/// `DB2LTTYP.ctl:17-23`, unpadded and in code order; `REVERAL` is the source's spelling (E23).
const DB2_SEED: [(&str, &str); 7] = [
    ("01", "PURCHASE"),
    ("02", "PAYMENT"),
    ("03", "CREDIT"),
    ("04", "AUTHORIZATION"),
    ("05", "REFUND"),
    ("06", "REVERAL"),
    ("07", "ADJUSTMENT"),
];

/// The row bound of every paged read here: the Bounded Pagination maximum, above the ten users,
/// seven Db2 types and one or two cards per account the tables hold.
const PAGE_LIMIT: i64 = 100;

/// A broker URL nothing listens on (port 9, discard): a load that needed `RabbitMQ` would fail
/// against it (LOADS-037).
const UNREACHABLE_BROKER: &str = "amqp://127.0.0.1:9/%2f";

/// The server settings of the two concurrent runs' sessions, as `options` parameters of their
/// `DATABASE_URL`: each lock wait of the session that lasts 100 ms is logged with the backends
/// holding the lock and the waiting statement, and its end is logged when the lock is acquired.
/// Both are superuser settings, which the image's bootstrap user `carddemo` may set.
const LOG_LOCK_WAITS: &str = "options[log_lock_waits]=on&options[deadlock_timeout]=100ms";

/// The two statements of the `accounts` replacement a concurrent run waits in, as the server log
/// quotes them: they tell which wait is which, and are no expectation of the audit.
const LOCK_ACCOUNTS: &str = "LOCK TABLE accounts IN SHARE ROW EXCLUSIVE MODE";
const DELETE_ACCOUNTS: &str = "DELETE FROM accounts";

/// Interval between two reads of the server log while a lock wait is awaited.
const LOG_POLL: Duration = Duration::from_millis(50);

/// Bound on awaiting a lock wait in the server log, far above the 100 ms after which the server
/// logs it, so only a run that never waits reaches it.
const LOG_BOUND: Duration = Duration::from_secs(60);

/// Bound on one read of `accounts` while both concurrent runs wait. A read never waits for their
/// locks; one that did would wait for the test's own transaction, so the bound turns it into a
/// failure.
const READ_BOUND: Duration = Duration::from_secs(10);

/// A private directory under the system temporary directory, removed with everything in it when
/// the guard drops. `var/` inside it is the `CARDDEMO_VAR_DIR` of every run.
struct Scratch {
    root: PathBuf,
}

impl Scratch {
    /// Creates `carddemo-batch-loads-<name>-<pid>-<nanos>` and its empty `var/`.
    fn create(name: &str) -> TestResult<Self> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let pid = std::process::id();
        let scratch = Self {
            root: std::env::temp_dir().join(format!("carddemo-batch-loads-{name}-{pid}-{nanos}")),
        };
        fs::create_dir_all(scratch.var_dir())?;
        Ok(scratch)
    }

    /// The `CARDDEMO_VAR_DIR` of the runs; the statement and job-deck directories lie inside it,
    /// so an empty `var/` shows that a run wrote no file and created no directory.
    fn var_dir(&self) -> PathBuf {
        self.root.join("var")
    }

    /// The path `name` inside the scratch directory, outside `var/`.
    fn path(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // Ignored: a directory left in the temporary directory affects no later run, and a drop
        // cannot report an error.
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The protected fixture values: the 50 card numbers, the 50 SSNs in both their plain and dashed
/// forms, and the 10 passwords. No standard-output or standard-error line of a run may hold one
/// (LOADS-039); they are searched for and never printed.
struct Needles(Vec<Vec<u8>>);

impl Needles {
    fn load() -> TestResult<Self> {
        let mut needles: Vec<Vec<u8>> = fixture_card_numbers()?
            .into_iter()
            .map(String::into_bytes)
            .collect();
        needles.extend(fixture_ssn_forms()?.into_iter().map(String::into_bytes));
        needles.extend(fixture_user_passwords()?);
        Ok(Self(needles))
    }

    /// Panics, naming the run and the stream only, when either stream of `output` holds one.
    fn assert_absent(&self, run: &str, output: &Output) {
        for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
            let leaked = self.0.iter().any(|needle| contains(bytes, needle));
            assert!(!leaked, "{run}: {stream} holds a protected fixture value");
        }
    }
}

/// Whether `needle`, which is not empty, occurs in `haystack`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

/// Runs `carddemo-batch` with `args` from `modern/` against the database of `guard`, its
/// directories inside `scratch`, the broker unreachable, span export off and every event at
/// `info` and above on standard output, so that LOADS-039 has events to search.
///
/// `fixture_dir` becomes `CARDDEMO_FIXTURE_DIR`; `None` removes the variable, so the binary's
/// default applies. The inherited environment is otherwise kept: the coverage run's
/// `LLVM_PROFILE_FILE` must reach the subprocess.
async fn run_batch(
    guard: &PostgresGuard,
    scratch: &Scratch,
    args: &[&str],
    fixture_dir: Option<&Path>,
) -> TestResult<Output> {
    run_batch_on(guard.database_url(), scratch, args, fixture_dir).await
}

/// [`run_batch`] against the database at `database_url`.
async fn run_batch_on(
    database_url: &str,
    scratch: &Scratch,
    args: &[&str],
    fixture_dir: Option<&Path>,
) -> TestResult<Output> {
    let var_dir = scratch.var_dir();
    let mut command = Command::new(env!("CARGO_BIN_EXE_carddemo-batch"));
    command
        .args(args)
        .current_dir(modern_dir())
        .env("DATABASE_URL", database_url)
        .env("CARDDEMO_AMQP_URL", UNREACHABLE_BROKER)
        .env("CARDDEMO_VAR_DIR", &var_dir)
        .env("CARDDEMO_STATEMENT_DIR", var_dir.join("statements"))
        .env("CARDDEMO_INTRDR_DIR", var_dir.join("jobs"))
        .env_remove("CARDDEMO_OTEL_EXPORT")
        .env("RUST_LOG", "info");
    if let Some(dir) = fixture_dir {
        command.env("CARDDEMO_FIXTURE_DIR", dir);
    } else {
        command.env_remove("CARDDEMO_FIXTURE_DIR");
    }
    Ok(test_support::run_child(command, test_support::CHILD_DEADLINE).await?)
}

/// The lines of standard output that name a written path: every line that is not empty and does
/// not begin with `{`, which begins each JSON tracing event.
fn written_paths(output: &Output) -> Vec<PathBuf> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('{'))
        .map(PathBuf::from)
        .collect()
}

/// Asserts that the run `run` exited with `expected`, showing its standard-error line, which
/// names no protected value, when it did not.
fn assert_exit(output: &Output, expected: i32, run: &str) {
    assert_eq!(
        output.status.code(),
        Some(expected),
        "{run}: stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Asserts that the run `run` failed: a non-zero exit with a standard-error line.
fn assert_failed(output: &Output, run: &str) {
    assert!(
        output.status.code().is_some_and(|code| code != 0),
        "{run}: exited {:?}",
        output.status.code()
    );
    assert!(!output.stderr.is_empty(), "{run}: no standard-error line");
}

/// `path` as a command-line argument.
fn path_arg(path: &Path) -> TestResult<&str> {
    Ok(path
        .to_str()
        .ok_or("a scratch or fixture path is not UTF-8")?)
}

/// Runs `jobs` in turn, with no arguments and the harness's fixture directory as
/// `CARDDEMO_FIXTURE_DIR`, and asserts of each run what every load that succeeds shows: exit 0
/// with no broker reachable (LOADS-037), no path line (LOADS-038) and no protected value on
/// either stream (LOADS-039).
async fn run_loads(
    guard: &PostgresGuard,
    scratch: &Scratch,
    jobs: &[&str],
    needles: &Needles,
) -> TestResult {
    let fixtures = fixture_dir();
    for &job in jobs {
        let output = run_batch(guard, scratch, &[job], Some(&fixtures)).await?;
        // audit: LOADS-037
        assert_exit(&output, 0, job);
        // audit: LOADS-038
        assert!(
            written_paths(&output).is_empty(),
            "{job}: printed a path line"
        );
        // audit: LOADS-039
        needles.assert_absent(job, &output);
    }
    Ok(())
}

/// The nine tables the loads write and the Db2 data version they must leave alone, as the
/// repository reads return them: key order, card-number order for `cards`.
struct Tables {
    accounts: Vec<AccountRecord>,
    cards: Vec<(CardRef, CardRecord)>,
    xrefs: Vec<CardXrefRecord>,
    customers: Vec<CustomerRecord>,
    groups: Vec<DisclosureGroupRecord>,
    balances: Vec<TranCatBalanceRecord>,
    categories: Vec<TransactionCategoryRecord>,
    types: Vec<TransactionTypeRecord>,
    users: Vec<UserEntry>,
    db2_types: Vec<Db2TransactionType>,
}

/// Reads every table of [`Tables`]. `users` has no unbounded read: its IDs come from one
/// COUSR00C page from the blank key, and each user is then read whole, filler and hash included.
async fn snapshot(store: &mut PgStore) -> TestResult<Tables> {
    let listed = UserRepository::page_after(store, &PicX::spaces(), PAGE_LIMIT).await?;
    let mut users = Vec::with_capacity(listed.len());
    for entry in &listed {
        let user = UserRepository::find_by_id(store, &entry.id)
            .await?
            .ok_or("a listed user cannot be read by its ID")?;
        users.push(user);
    }
    Ok(Tables {
        accounts: AccountRepository::find_all(store).await?,
        cards: CardRepository::find_all(store).await?,
        xrefs: CardXrefRepository::find_all(store).await?,
        customers: CustomerRepository::find_all(store).await?,
        groups: DisclosureGroupRepository::find_all(store).await?,
        balances: TranCatBalanceRepository::find_all(store).await?,
        categories: TransactionCategoryRepository::find_all(store).await?,
        types: TransactionTypeRepository::find_all(store).await?,
        users,
        db2_types: Db2TransactionTypeRepository::page_after(
            store,
            &PicX::spaces(),
            None,
            None,
            PAGE_LIMIT,
        )
        .await?,
    })
}

/// The index of the first row at which `expected` and `actual` differ, the shorter length when
/// one is a prefix of the other, or `None` when they are equal.
fn first_difference<T: PartialEq>(expected: &[T], actual: &[T]) -> Option<usize> {
    expected
        .iter()
        .zip(actual)
        .position(|(left, right)| left != right)
        .or_else(|| (expected.len() != actual.len()).then_some(expected.len().min(actual.len())))
}

/// The cards without their `card_ref`, which is no copybook field and which every `cardfile`
/// run assigns anew (LOADS-027).
fn card_records(tables: &Tables) -> Vec<&CardRecord> {
    tables.cards.iter().map(|(_, card)| card).collect()
}

/// The `card_ref` values of the cards.
fn card_refs(tables: &Tables) -> BTreeSet<CardRef> {
    tables.cards.iter().map(|(handle, _)| *handle).collect()
}

/// The copybook-derived columns of a user: all but `password_hash`, whose salt every
/// `dusrsecj` run draws anew (LOADS-033).
type UserColumns<'a> = (
    &'a PicX<8>,
    &'a PicX<20>,
    &'a PicX<20>,
    &'a PicX<1>,
    &'a PicX<23>,
);

fn user_columns(user: &UserEntry) -> UserColumns<'_> {
    (
        &user.id,
        &user.fname,
        &user.lname,
        &user.usr_type,
        &user.filler,
    )
}

fn users_without_hash(tables: &Tables) -> Vec<UserColumns<'_>> {
    tables.users.iter().map(user_columns).collect()
}

/// Each table in which `actual` differs from `expected` in a copybook-derived column, with the
/// index of its first differing row; `cards.card_ref` and `users.password_hash` are not compared
/// (LOADS-026, FIXLOAD-010).
fn differences(expected: &Tables, actual: &Tables) -> Vec<(&'static str, usize)> {
    [
        (
            "accounts",
            first_difference(&expected.accounts, &actual.accounts),
        ),
        (
            "cards",
            first_difference(&card_records(expected), &card_records(actual)),
        ),
        (
            "card_xref",
            first_difference(&expected.xrefs, &actual.xrefs),
        ),
        (
            "customers",
            first_difference(&expected.customers, &actual.customers),
        ),
        (
            "disclosure_groups",
            first_difference(&expected.groups, &actual.groups),
        ),
        (
            "tran_cat_balances",
            first_difference(&expected.balances, &actual.balances),
        ),
        (
            "transaction_categories",
            first_difference(&expected.categories, &actual.categories),
        ),
        (
            "transaction_types",
            first_difference(&expected.types, &actual.types),
        ),
        (
            "users",
            first_difference(&users_without_hash(expected), &users_without_hash(actual)),
        ),
        (
            "db2.transaction_type",
            first_difference(&expected.db2_types, &actual.db2_types),
        ),
    ]
    .into_iter()
    .filter_map(|(table, row)| row.map(|row| (table, row)))
    .collect()
}

/// Asserts that `actual` holds the contents of `expected` (see [`differences`]), naming only the
/// tables and row indexes that differ.
fn assert_same_contents(expected: &Tables, actual: &Tables, run: &str) {
    let found = differences(expected, actual);
    assert!(
        found.is_empty(),
        "{run}: (table, first differing row) {found:?}"
    );
}

/// Asserts LOADS-027 of the `cardfile` run that `later` follows: each card has its own
/// `card_ref`, and none is a value an earlier run gave a card, so a handle issued before a
/// reload names no card after it.
fn assert_fresh_card_refs(earlier: &[&Tables], later: &Tables) {
    let refs = card_refs(later);
    assert_eq!(refs.len(), later.cards.len(), "a card_ref names two cards");
    for (run, tables) in earlier.iter().enumerate() {
        assert!(
            refs.is_disjoint(&card_refs(tables)),
            "a card_ref of earlier run {run} is issued again"
        );
    }
}

/// Whether every byte of a `FILLER` is the character `0`, as in `discgrp.txt`, `tcatbal.txt`,
/// `trancatg.txt` and `trantype.txt` (E05).
fn all_zero_characters(filler: &[u8]) -> bool {
    filler.iter().all(|&byte| byte == b'0')
}

/// The keys `1..=50`: the account IDs of `acctdata.txt:1-50` and the customer IDs of
/// `custdata.txt:1-50` (FIXLOAD-001, FIXLOAD-004).
fn one_to_fifty() -> Vec<u64> {
    (1..=50).collect()
}

/// The card numbers of `cards` or `card_xref` as bytes, never shown.
fn card_number_set<'a>(numbers: impl Iterator<Item = &'a domain::CardNumber>) -> BTreeSet<Vec<u8>> {
    numbers
        .map(|number| number.expose().as_bytes().to_vec())
        .collect()
}

/// Asserts the empty, migrated database the first load meets (LOADS-015): no row in any table a
/// load writes, and the Db2 data version as migration 0006 seeds it (E23).
fn assert_migrated_only(tables: &Tables) {
    let rows = [
        ("accounts", tables.accounts.len()),
        ("cards", tables.cards.len()),
        ("card_xref", tables.xrefs.len()),
        ("customers", tables.customers.len()),
        ("disclosure_groups", tables.groups.len()),
        ("tran_cat_balances", tables.balances.len()),
        ("transaction_categories", tables.categories.len()),
        ("transaction_types", tables.types.len()),
        ("users", tables.users.len()),
    ];
    for (table, count) in rows {
        assert_eq!(count, 0, "{table}: rows before the first load");
    }
    let seeded: Vec<(String, &str)> = tables
        .db2_types
        .iter()
        .map(|row| (row.tr_type().to_text(), row.tr_description()))
        .collect();
    let expected: Vec<(String, &str)> = DB2_SEED
        .into_iter()
        .map(|(code, description)| (code.to_owned(), description))
        .collect();
    assert_eq!(
        seeded, expected,
        "db2.transaction_type: migration 0006 seed"
    );
}

/// The account, card, cross-reference and customer tables after the fixture loads.
/// `card_numbers` holds the 16 bytes of every `CARD-NUM` of `carddata.txt`, which
/// `cardxref.txt` holds too.
fn assert_master_contents(tables: &Tables, card_numbers: &BTreeSet<Vec<u8>>) {
    let account_keys: Vec<u64> = tables
        .accounts
        .iter()
        .map(|account| account.id.get())
        .collect();
    // audit: LOADS-001
    // audit: FIXLOAD-001
    assert_eq!(account_keys, one_to_fifty(), "accounts: keys");

    // audit: LOADS-002
    // audit: FIXLOAD-002
    assert_eq!(tables.cards.len(), 50, "cards: rows");
    // audit: LOADS-002
    // audit: FIXLOAD-002
    assert!(
        card_number_set(tables.cards.iter().map(|(_, card)| &card.num)) == *card_numbers,
        "cards: keys differ from the card numbers of carddata.txt"
    );

    // audit: LOADS-003
    // audit: FIXLOAD-003
    assert_eq!(tables.xrefs.len(), 50, "card_xref: rows");
    // audit: LOADS-003
    // audit: FIXLOAD-003
    assert!(
        card_number_set(tables.xrefs.iter().map(|xref| &xref.card_num)) == *card_numbers,
        "card_xref: keys differ from the card numbers of carddata.txt"
    );
    // audit: LOADS-030
    // audit: FIXLOAD-003
    assert!(
        tables.xrefs.iter().all(|xref| xref.filler.is_spaces()),
        "card_xref: a filler is not 14 spaces"
    );

    let customer_keys: Vec<u64> = tables
        .customers
        .iter()
        .map(|customer| customer.id.get())
        .collect();
    // audit: LOADS-004
    // audit: FIXLOAD-004
    assert_eq!(customer_keys, one_to_fifty(), "customers: keys");
}

/// The disclosure group, category balance, category and type tables after the fixture loads,
/// the VSAM data version of the types and categories (LOADS-029).
fn assert_reference_contents(tables: &Tables) {
    // audit: LOADS-005
    // audit: FIXLOAD-005
    assert_eq!(tables.groups.len(), 51, "disclosure_groups: rows");
    // audit: LOADS-032
    // audit: FIXLOAD-005
    assert!(
        tables
            .groups
            .iter()
            .all(|group| all_zero_characters(group.filler.as_bytes())),
        "disclosure_groups: a filler is not 28 zero characters"
    );

    let balance_keys: Vec<(u64, String, u64)> = tables
        .balances
        .iter()
        .map(|balance| {
            (
                balance.acct_id.get(),
                balance.type_cd.to_text(),
                balance.cd.get(),
            )
        })
        .collect();
    let expected_keys: Vec<(u64, String, u64)> = one_to_fifty()
        .into_iter()
        .map(|account| (account, "01".to_owned(), 1))
        .collect();
    // audit: LOADS-006
    // audit: FIXLOAD-006
    assert_eq!(balance_keys, expected_keys, "tran_cat_balances: keys");
    // audit: FIXLOAD-006
    assert!(
        tables.balances.iter().all(|balance| balance.bal.is_zero()),
        "tran_cat_balances: a balance is not 0.00"
    );
    // audit: LOADS-032
    // audit: FIXLOAD-006
    assert!(
        tables
            .balances
            .iter()
            .all(|balance| all_zero_characters(balance.filler.as_bytes())),
        "tran_cat_balances: a filler is not 22 zero characters"
    );

    // audit: LOADS-007
    // audit: FIXLOAD-007
    assert_eq!(tables.categories.len(), 18, "transaction_categories: rows");
    // audit: LOADS-031
    // audit: LOADS-032
    // audit: FIXLOAD-007
    assert!(
        tables.categories.iter().all(|category| {
            all_zero_characters(category.filler.as_bytes())
                && !category.type_desc.as_bytes().contains(&b'\r')
        }),
        "transaction_categories: a filler is not 4 zero characters, or a column holds a CR"
    );

    let type_codes: Vec<String> = tables
        .types
        .iter()
        .map(|row| row.tran_type.to_text())
        .collect();
    // audit: LOADS-008
    // audit: FIXLOAD-008
    assert_eq!(
        type_codes,
        ["01", "02", "03", "04", "05", "06", "07"],
        "transaction_types: keys"
    );
    // audit: LOADS-031
    // audit: LOADS-032
    // audit: FIXLOAD-008
    assert!(
        tables.types.iter().all(|row| {
            all_zero_characters(row.filler.as_bytes()) && !row.desc.as_bytes().contains(&b'\r')
        }),
        "transaction_types: a filler is not 8 zero characters, or a column holds a CR"
    );
}

/// Asserts that `stored`, the rows of `table` in key order, are the records of `input`, the bytes
/// of the fixture the table was loaded from, as [`framed_records`] decodes them: one row per
/// line, each equal to its line's record in every copybook-derived field, in file order, which
/// is key order because every fixture's keys ascend (FIXFRM-020). Each row is then re-encoded
/// inside its line's framing and must give that line's bytes, terminator included, so every
/// stored field is tied to the fixture bytes themselves; the encoding also checks that a short
/// body's missing tail is blank FILLER (PIC-013). A failure names the table and the row or line
/// index only.
fn assert_rows_are_input<'a, R>(
    table: &str,
    stored: impl IntoIterator<Item = &'a R>,
    input: &[u8],
) -> TestResult
where
    R: FixedRecord + PartialEq + Clone + 'a,
{
    let stored: Vec<&R> = stored.into_iter().collect();
    let expected = framed_records::<R>(input)?;
    let records: Vec<&R> = expected.iter().map(|line| &line.record).collect();
    assert_eq!(
        stored.len(),
        records.len(),
        "{table}: rows against input lines"
    );
    assert_eq!(
        first_difference(&records, &stored),
        None,
        "{table}: first row that differs from its input line"
    );

    let mut encoded = Vec::with_capacity(stored.len());
    for (index, (&row, line)) in stored.iter().zip(&expected).enumerate() {
        let framed = Framed {
            record: row.clone(),
            framing: line.framing,
        };
        encoded.push(framed.try_encode().map_err(|err| {
            format!("{table}: row {index} does not re-encode inside its line framing: {err}")
        })?);
    }
    let encoded: Vec<&[u8]> = encoded.iter().map(Vec::as_slice).collect();
    let lines: Vec<&[u8]> = input.split_inclusive(|&byte| byte == b'\n').collect();
    assert_eq!(
        first_difference(&lines, &encoded),
        None,
        "{table}: first input line its row does not re-encode to"
    );
    Ok(())
}

/// Asserts that the eight fixture loads left each table holding exactly its fixture's records,
/// field by field and down to the bytes of each line, as [`assert_rows_are_input`] compares them,
/// with every fixture read in place. `cards.card_ref` is no copybook field and is left out
/// (LOADS-027); the users are compared by [`assert_users`], whose passwords are hashes.
fn assert_fixture_contents(tables: &Tables) -> TestResult {
    let fixture = |name: &str| fs::read(fixture_path(name));
    // audit: LOADS-001
    // audit: LOADS-032
    // audit: FIXLOAD-001
    assert_rows_are_input("accounts", &tables.accounts, &fixture("acctdata.txt")?)?;
    // audit: LOADS-002
    // audit: LOADS-032
    // audit: FIXLOAD-002
    assert_rows_are_input("cards", card_records(tables), &fixture("carddata.txt")?)?;
    // audit: LOADS-003
    // audit: LOADS-030
    // audit: LOADS-032
    // audit: FIXLOAD-003
    assert_rows_are_input("card_xref", &tables.xrefs, &fixture("cardxref.txt")?)?;
    // audit: LOADS-004
    // audit: LOADS-032
    // audit: FIXLOAD-004
    assert_rows_are_input("customers", &tables.customers, &fixture("custdata.txt")?)?;
    // audit: LOADS-005
    // audit: LOADS-032
    // audit: FIXLOAD-005
    assert_rows_are_input(
        "disclosure_groups",
        &tables.groups,
        &fixture("discgrp.txt")?,
    )?;
    // audit: LOADS-006
    // audit: LOADS-031
    // audit: LOADS-032
    // audit: FIXLOAD-006
    assert_rows_are_input(
        "tran_cat_balances",
        &tables.balances,
        &fixture("tcatbal.txt")?,
    )?;
    // audit: LOADS-007
    // audit: LOADS-031
    // audit: LOADS-032
    // audit: FIXLOAD-007
    assert_rows_are_input(
        "transaction_categories",
        &tables.categories,
        &fixture("trancatg.txt")?,
    )?;
    // audit: LOADS-008
    // audit: LOADS-031
    // audit: LOADS-032
    // audit: FIXLOAD-008
    assert_rows_are_input(
        "transaction_types",
        &tables.types,
        &fixture("trantype.txt")?,
    )?;
    Ok(())
}

/// The ten users after a `dusrsecj` run, against the deck's records in key order and
/// `passwords`, the 8 `SEC-USR-PWD` bytes of each record in deck order: every column but the
/// password as decoded and padded, and in its place an argon2id PHC string that verifies the
/// record's own 8 bytes and is not those bytes (E25).
fn assert_users(users: &[UserEntry], passwords: &[Vec<u8>]) -> TestResult {
    // audit: LOADS-009
    // audit: LOADS-012
    // audit: FIXLOAD-009
    assert_eq!(users.len(), USERS.len(), "users: rows");
    assert_eq!(
        passwords.len(),
        USERS.len(),
        "DUSRSECJ.jcl: in-stream records"
    );
    for ((user, (id, fname, lname, usr_type)), password) in users.iter().zip(USERS).zip(passwords) {
        let expected = (
            PicX::<8>::from_text(id)?,
            PicX::<20>::from_text(fname)?,
            PicX::<20>::from_text(lname)?,
            PicX::<1>::from_text(usr_type)?,
            PicX::<23>::spaces(),
        );
        // audit: LOADS-034
        // audit: FIXUSR-001
        // audit: FIXUSR-002
        // audit: FIXUSR-003
        // audit: FIXUSR-004
        // audit: FIXUSR-005
        // audit: FIXUSR-006
        // audit: FIXUSR-007
        // audit: FIXUSR-008
        // audit: FIXUSR-009
        // audit: FIXUSR-010
        assert!(
            user_columns(user)
                == (
                    &expected.0,
                    &expected.1,
                    &expected.2,
                    &expected.3,
                    &expected.4
                ),
            "users: {id} differs from its in-stream record"
        );
        let plaintext = <[u8; 8]>::try_from(password.as_slice())?;
        // audit: LOADS-033
        // audit: FIXLOAD-009
        assert!(
            user.password_hash.starts_with("$argon2id$")
                && user.password_hash.as_bytes() != plaintext.as_slice(),
            "users: {id} stores no argon2id PHC string in place of its password"
        );
        // audit: LOADS-033
        assert!(
            password::verify(&plaintext, &user.password_hash)?,
            "users: the hash of {id} does not verify its password"
        );
    }
    Ok(())
}

/// Asserts that the loads left the Db2 data version as `seeded` held it, while the VSAM version
/// holds the fixture's spelling (LOADS-028, LOADS-029, E23): type `01` is `PURCHASE` in
/// `db2.transaction_type` (`DB2LTTYP.ctl:17`) and `Purchase` in `transaction_types`
/// (`trantype.txt:1`).
async fn assert_db2_untouched(store: &mut PgStore, seeded: &Tables, loaded: &Tables) -> TestResult {
    // audit: LOADS-028
    // audit: LOADS-029
    assert_eq!(
        first_difference(&seeded.db2_types, &loaded.db2_types),
        None,
        "db2.transaction_type: first changed row"
    );
    let code = PicX::from_array(*b"01");
    let db2 = Db2TransactionTypeRepository::find_by_id(store, &code)
        .await?
        .ok_or("db2.transaction_type holds no type 01")?;
    let vsam = loaded
        .types
        .iter()
        .find(|row| row.tran_type == code)
        .ok_or("transaction_types holds no type 01")?;
    // audit: LOADS-029
    assert_eq!(
        db2.tr_description(),
        "PURCHASE",
        "db2.transaction_type: type 01"
    );
    // audit: LOADS-029
    // audit: FIXLOAD-008
    assert_eq!(
        vsam.desc.trim_end(),
        b"Purchase".as_slice(),
        "transaction_types: type 01"
    );
    Ok(())
}

/// Reads the cards and cross-references of every loaded account through the counterparts of
/// the alternate indexes and compares them with the whole tables (LOADS-022 to LOADS-024).
async fn assert_account_reads(store: &mut PgStore, tables: &Tables) -> TestResult {
    let accounts: BTreeSet<PicU<11>> = tables
        .cards
        .iter()
        .map(|(_, card)| card.acct_id)
        .chain(tables.xrefs.iter().map(|xref| xref.acct_id))
        .collect();
    for account in accounts {
        let cards: Vec<&(CardRef, CardRecord)> = tables
            .cards
            .iter()
            .filter(|(_, card)| card.acct_id == account)
            .collect();
        let read = CardRepository::page_after(store, None, Some(account), None, PAGE_LIMIT).await?;
        // audit: LOADS-022
        // audit: LOADS-024
        assert!(
            read.iter().eq(cards.iter().copied()),
            "cards of account {}: the read by acct_id differs from the table",
            account.get()
        );
        let xrefs: Vec<&CardXrefRecord> = tables
            .xrefs
            .iter()
            .filter(|xref| xref.acct_id == account)
            .collect();
        let read = CardXrefRepository::find_first_two_by_acct_id(store, account).await?;
        // audit: LOADS-023
        // audit: LOADS-024
        assert!(
            read.len() == xrefs.len().min(2) && read.iter().all(|xref| xrefs.contains(&xref)),
            "card_xref of account {}: the read by acct_id differs from the table",
            account.get()
        );
    }
    Ok(())
}

/// The bytes of the nine load inputs: the eight fixtures and the DUSRSECJ deck.
fn read_inputs() -> TestResult<Vec<(PathBuf, Vec<u8>)>> {
    let mut paths: Vec<PathBuf> = FIXTURES.into_iter().map(fixture_path).collect();
    paths.push(dusrsecj_path());
    let mut inputs = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = fs::read(&path)?;
        inputs.push((path, bytes));
    }
    Ok(inputs)
}

/// Asserts that every input of `inputs` still holds its bytes (LOADS-011), naming only the file.
fn assert_inputs_unchanged(inputs: &[(PathBuf, Vec<u8>)]) -> TestResult {
    for (path, before) in inputs {
        let after = fs::read(path)?;
        // audit: LOADS-011
        assert!(
            after == *before,
            "{} changed during the loads",
            path.display()
        );
    }
    Ok(())
}

/// Asserts that, after `run`, the tables no load writes and a repository reads whole hold no
/// row: `transactions`, `pending_auth_summaries` and `cssl_error_log` (FIXLOAD-011). A failure
/// names the run and the table only.
async fn assert_unloaded_tables_empty(store: &mut PgStore, run: &str) -> TestResult {
    // audit: FIXLOAD-011
    assert!(
        TransactionRepository::find_all_in_card_order(store)
            .await?
            .is_empty(),
        "{run}: transactions holds a row"
    );
    // audit: FIXLOAD-011
    assert!(
        PendingAuthSummaryRepository::find_all(store)
            .await?
            .is_empty(),
        "{run}: pending_auth_summaries holds a row"
    );
    // audit: FIXLOAD-011
    assert!(
        CsslErrorLogRepository::find_all(store).await?.is_empty(),
        "{run}: cssl_error_log holds a row"
    );
    Ok(())
}

/// Asserts that no run created anything under `CARDDEMO_VAR_DIR`, which holds the statement and
/// job-deck directories too.
fn assert_no_file_written(scratch: &Scratch) -> TestResult {
    let mut entries = fs::read_dir(scratch.var_dir())?;
    assert!(
        entries.next().is_none(),
        "a load created an entry under CARDDEMO_VAR_DIR"
    );
    Ok(())
}

/// The first `count` lines of `bytes`, terminators included.
fn first_lines(bytes: &[u8], count: usize) -> Vec<u8> {
    concat(bytes.split_inclusive(|&byte| byte == b'\n').take(count))
}

/// The bytes of `parts`, one after the other.
fn concat<'a>(parts: impl Iterator<Item = &'a [u8]>) -> Vec<u8> {
    parts.flatten().copied().collect()
}

/// The lines of the load input `bytes` decoded as `R`, each with its line framing, in input
/// order: the expected records of a load, taken from the input itself. The decoder is codec A at
/// the `copybook` level, `framing::decode_all`, whose field values the i8 round trip and its
/// decoded-corpus snapshot fix against the copybooks, and not the `domain::batch::loads` path the
/// binary decodes through.
fn framed_records<R: FixedRecord>(bytes: &[u8]) -> TestResult<Vec<Framed<R>>> {
    Ok(decode_all::<R>(bytes)?)
}

/// The records of [`framed_records`] without their line framing, in input order.
fn input_records<R: FixedRecord>(bytes: &[u8]) -> TestResult<Vec<R>> {
    Ok(framed_records::<R>(bytes)?
        .into_iter()
        .map(|line| line.record)
        .collect())
}

/// An `acctfile` input the load must refuse: the scratch file name, its bytes, the start of the
/// refused line, which the error must not show, and the line numbers the error must name.
struct Refusal<'a> {
    name: &'a str,
    bytes: Vec<u8>,
    refused_line: &'a [u8],
    named_lines: &'a [&'a str],
}

/// Runs `acctfile` over the input of `refusal` and asserts the refusal of LOADS-016 and
/// LOADS-017: a non-zero exit whose standard-error line names the file and each line of
/// `named_lines`, never the bytes of `refused_line`, and `accounts` still holding `previous`.
async fn assert_refused_accounts(
    guard: &PostgresGuard,
    scratch: &Scratch,
    store: &mut PgStore,
    refusal: &Refusal<'_>,
    previous: &[AccountRecord],
) -> TestResult {
    let name = refusal.name;
    let input = scratch.path(name);
    fs::write(&input, &refusal.bytes)?;
    let shown = path_arg(&input)?;
    let output = run_batch(guard, scratch, &["acctfile", "--input", shown], None).await?;
    assert_failed(&output, name);
    assert!(
        contains(&output.stderr, shown.as_bytes()),
        "{name}: the error does not name the file"
    );
    for line in refusal.named_lines {
        assert!(
            contains(&output.stderr, line.as_bytes()),
            "{name}: the error does not name {line}"
        );
    }
    assert!(
        !contains(&output.stderr, refusal.refused_line),
        "{name}: the error shows the bytes of the refused line"
    );
    let after = AccountRepository::find_all(store).await?;
    assert_eq!(
        first_difference(previous, &after),
        None,
        "{name}: accounts changed; first changed row"
    );
    Ok(())
}

/// The nine loads in `scripts/dev-up.sh` order on a freshly migrated database, then again on the
/// loaded one, then once more in reverse order: every run succeeds, the first leaves the fixture
/// contents, and the later runs leave the same copybook-derived contents.
///
/// The first load's rows are compared field by field with the fixture lines, decoded from the
/// files read in place, and re-encoded inside each line's framing to the lines' bytes
/// ([`assert_fixture_contents`]); its users with the deck's records ([`assert_users`]). The later
/// runs are compared with that proven first load.
#[test]
fn nine_loads_in_dev_up_order_then_rerun() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let scratch = Scratch::create("rerun")?;
        let needles = Needles::load()?;
        let passwords = fixture_user_passwords()?;
        let card_numbers: BTreeSet<Vec<u8>> = fixture_card_numbers()?
            .into_iter()
            .map(String::into_bytes)
            .collect();
        let inputs = read_inputs()?;
        let mut store = PgStore::new(guard.pool().clone());

        let seeded = snapshot(&mut store).await?;
        // audit: LOADS-015
        assert_migrated_only(&seeded);
        // audit: LOADS-015
        run_loads(&guard, &scratch, &DEV_UP_ORDER, &needles).await?;
        let first = snapshot(&mut store).await?;
        assert_fixture_contents(&first)?;
        assert_master_contents(&first, &card_numbers);
        assert_reference_contents(&first);
        assert_users(&first.users, &passwords)?;
        assert_account_reads(&mut store, &first).await?;
        assert_db2_untouched(&mut store, &seeded, &first).await?;
        assert_unloaded_tables_empty(&mut store, "dev-up.sh order").await?;

        // audit: LOADS-014
        run_loads(&guard, &scratch, &DEV_UP_ORDER, &needles).await?;
        let second = snapshot(&mut store).await?;
        // audit: LOADS-014
        // audit: LOADS-026
        // audit: LOADS-036
        // audit: FIXLOAD-010
        assert_same_contents(&first, &second, "rerun in dev-up.sh order");
        // audit: LOADS-027
        // audit: FIXLOAD-002
        assert_fresh_card_refs(&[&first], &second);
        // audit: LOADS-033
        // audit: FIXLOAD-009
        assert_users(&second.users, &passwords)?;
        assert_unloaded_tables_empty(&mut store, "rerun in dev-up.sh order").await?;

        let reverse: Vec<&str> = DEV_UP_ORDER.into_iter().rev().collect();
        // audit: LOADS-025
        run_loads(&guard, &scratch, &reverse, &needles).await?;
        let third = snapshot(&mut store).await?;
        // audit: LOADS-025
        // audit: FIXLOAD-010
        assert_same_contents(&first, &third, "reverse dev-up.sh order");
        // audit: LOADS-027
        assert_fresh_card_refs(&[&first, &second], &third);
        assert_users(&third.users, &passwords)?;
        assert_unloaded_tables_empty(&mut store, "reverse dev-up.sh order").await?;

        assert_inputs_unchanged(&inputs)?;
        // audit: LOADS-035
        // audit: LOADS-038
        // audit: FIXLOAD-011
        assert_no_file_written(&scratch)?;
        Ok(())
    })
}

/// `--input` names the file a fixture load reads, whatever `CARDDEMO_FIXTURE_DIR` holds; without
/// it, a fixture missing from that directory fails the load before the table is touched; and a
/// load writes only its own table.
#[test]
fn input_override_wins_over_fixture_dir() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let scratch = Scratch::create("input")?;
        let empty = scratch.path("no-fixtures");
        fs::create_dir(&empty)?;
        let mut store = PgStore::new(guard.pool().clone());
        let before = snapshot(&mut store).await?;

        let missing = run_batch(&guard, &scratch, &["acctfile"], Some(&empty)).await?;
        let expected_path = empty.join("acctdata.txt");
        // audit: LOADS-010
        // audit: LOADS-019
        assert_failed(&missing, "acctfile without its fixture");
        // audit: LOADS-019
        assert!(
            contains(&missing.stderr, path_arg(&expected_path)?.as_bytes()),
            "acctfile without its fixture: the error does not name the path"
        );
        // audit: LOADS-019
        assert!(
            AccountRepository::find_all(&mut store).await?.is_empty(),
            "accounts: a failed load wrote rows"
        );

        let input = fs::canonicalize(fixture_path("acctdata.txt"))?;
        let args = ["acctfile", "--input", path_arg(&input)?];
        let loaded = run_batch(&guard, &scratch, &args, Some(&empty)).await?;
        // audit: LOADS-010
        assert_exit(&loaded, 0, "acctfile --input");
        let after = snapshot(&mut store).await?;
        // audit: LOADS-010
        // audit: FIXLOAD-001
        assert_eq!(after.accounts.len(), 50, "accounts: rows");
        let changed: Vec<&str> = differences(&before, &after)
            .into_iter()
            .map(|(table, _)| table)
            .collect();
        // audit: LOADS-028
        assert_eq!(changed, ["accounts"], "tables acctfile changed");
        // audit: LOADS-038
        assert_no_file_written(&scratch)?;
        Ok(())
    })
}

/// With `CARDDEMO_FIXTURE_DIR` unset and no `--input`, a fixture load reads
/// `../app/data/ASCII/<file>.txt` and `dusrsecj` reads `../app/jcl/DUSRSECJ.jcl`, both relative
/// to `modern/`; a deck that is broken or missing then fails and leaves the users as they were.
#[test]
fn default_fixture_dir_resolves_from_modern() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let scratch = Scratch::create("defaults")?;
        let mut store = PgStore::new(guard.pool().clone());

        let types = run_batch(&guard, &scratch, &["trantype"], None).await?;
        // audit: LOADS-010
        assert_exit(&types, 0, "trantype");
        // audit: LOADS-010
        // audit: FIXLOAD-008
        assert_eq!(
            TransactionTypeRepository::find_all(&mut store).await?.len(),
            7,
            "transaction_types: rows"
        );

        let users = run_batch(&guard, &scratch, &["dusrsecj"], None).await?;
        // audit: LOADS-012
        assert_exit(&users, 0, "dusrsecj");
        let loaded = snapshot(&mut store).await?;
        // audit: LOADS-012
        // audit: FIXLOAD-009
        assert_eq!(loaded.users.len(), 10, "users: rows");

        let decks = [
            (
                "no-sysut1.jcl",
                "//DUSRSECJ JOB\n//STEP01  EXEC PGM=IEBGENER\n".to_owned(),
            ),
            ("no-delimiter.jcl", "//SYSUT1   DD *\n".to_owned()),
            (
                "long-line.jcl",
                format!("//SYSUT1   DD *\n{}\n/*\n", "X".repeat(81)),
            ),
        ];
        for (name, text) in decks {
            let deck = scratch.path(name);
            fs::write(&deck, text)?;
            let output = run_batch(
                &guard,
                &scratch,
                &["dusrsecj", "--input", path_arg(&deck)?],
                None,
            )
            .await?;
            // audit: LOADS-013
            assert_failed(&output, name);
            let after = snapshot(&mut store).await?;
            // audit: LOADS-013
            assert_eq!(
                first_difference(&loaded.users, &after.users),
                None,
                "{name}: users changed"
            );
        }

        let missing = scratch.path("missing.jcl");
        let output = run_batch(
            &guard,
            &scratch,
            &["dusrsecj", "--input", path_arg(&missing)?],
            None,
        )
        .await?;
        // audit: LOADS-019
        assert_failed(&output, "dusrsecj over a missing deck");
        // audit: LOADS-019
        assert!(
            contains(&output.stderr, path_arg(&missing)?.as_bytes()),
            "dusrsecj over a missing deck: the error does not name the path"
        );
        let after = snapshot(&mut store).await?;
        // audit: LOADS-019
        assert_eq!(
            first_difference(&loaded.users, &after.users),
            None,
            "missing deck: users changed"
        );
        Ok(())
    })
}

/// Scratch copies of `acctdata.txt` passed with `--input`: a subset replaces the loaded table
/// with exactly its own records, and an input codec A refuses, or whose keys do not rise, fails
/// before any write and leaves the previous rows.
#[test]
fn a_refused_input_leaves_the_previous_contents() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let scratch = Scratch::create("refused")?;
        let mut store = PgStore::new(guard.pool().clone());
        let fixture = fs::read(fixture_path("acctdata.txt"))?;
        let loaded = run_batch(&guard, &scratch, &["acctfile"], Some(&fixture_dir())).await?;
        assert_exit(&loaded, 0, "acctfile");

        let subset = scratch.path("acctdata-first-10.txt");
        fs::write(&subset, first_lines(&fixture, 10))?;
        let replaced = run_batch(
            &guard,
            &scratch,
            &["acctfile", "--input", path_arg(&subset)?],
            None,
        )
        .await?;
        // audit: LOADS-014
        assert_exit(&replaced, 0, "acctfile over the first 10 records");
        let previous = AccountRepository::find_all(&mut store).await?;
        let keys: Vec<u64> = previous.iter().map(|account| account.id.get()).collect();
        // audit: LOADS-014
        assert_eq!(
            keys,
            (1..=10).collect::<Vec<u64>>(),
            "accounts: keys after the subset"
        );

        let lines: Vec<&[u8]> = fixture.split_inclusive(|&byte| byte == b'\n').collect();
        let [line_1, line_2, rest @ ..] = lines.as_slice() else {
            return Err("acctdata.txt holds fewer than two lines".into());
        };
        let start_1 = line_1
            .get(..20)
            .ok_or("line 1 of acctdata.txt is shorter than 20 bytes")?;
        let start_2 = line_2
            .get(..20)
            .ok_or("line 2 of acctdata.txt is shorter than 20 bytes")?;
        // Line 2 cut to its first 20 bytes ends inside `ACCT-CURR-BAL` (offset 12, 12 bytes), not
        // in the trailing FILLER, so codec A refuses it (PIC-013).
        let short = Refusal {
            name: "acctdata-short-line.txt",
            bytes: concat(
                [*line_1, start_2, b"\n".as_slice()]
                    .into_iter()
                    .chain(rest.iter().copied()),
            ),
            refused_line: start_2,
            named_lines: &["line 2"],
        };
        // The first two records swapped: the key on line 2 is below the key on line 1.
        let swapped = Refusal {
            name: "acctdata-swapped.txt",
            bytes: concat([*line_2, *line_1].into_iter().chain(rest.iter().copied())),
            refused_line: start_1,
            named_lines: &["line 2", "line 1"],
        };

        // audit: LOADS-016
        assert_refused_accounts(&guard, &scratch, &mut store, &short, &previous).await?;
        // audit: LOADS-017
        assert_refused_accounts(&guard, &scratch, &mut store, &swapped, &previous).await?;

        // Every transaction of this session is read-only, so `PostgreSQL` refuses the statement
        // that opens the replacement, the table lock before the delete, with SQLSTATE 25006
        // (`read_only_sql_transaction`), whatever the input holds.
        let read_only = format!(
            "{}?options[default_transaction_read_only]=on",
            guard.database_url()
        );
        let whole = fs::canonicalize(fixture_path("acctdata.txt"))?;
        let args = ["acctfile", "--input", path_arg(&whole)?];
        let output = run_batch_on(&read_only, &scratch, &args, None).await?;
        // audit: LOADS-018
        assert_failed(&output, "acctfile in a read-only session");
        // audit: LOADS-018
        assert!(
            contains(&output.stderr, b"25006"),
            "acctfile in a read-only session: the error does not carry the SQLSTATE"
        );
        // audit: LOADS-018
        assert_eq!(
            first_difference(&previous, &AccountRepository::find_all(&mut store).await?),
            None,
            "acctfile in a read-only session: accounts changed; first changed row"
        );
        Ok(())
    })
}

/// One lock wait of a backend, as the server logs it under `log_lock_waits`.
///
/// It has no `Debug`: its statement is server log text, which a failure never shows.
struct LockWait {
    /// The waiting backend.
    pid: u32,
    /// `false` for the report that the backend still waits, `true` for the report that it
    /// acquired the lock after waiting.
    acquired: bool,
    /// The lock as the server names it: the mode, then the object, as in
    /// `ShareLock on transaction 745` or `ShareRowExclusiveLock on relation 16390 of database
    /// 16384`.
    lock: String,
    /// The backends holding a lock on the object, from the report's `DETAIL`, whether or not
    /// their mode conflicts with the one awaited; empty when the report has none.
    holders: Vec<u32>,
    /// The statement that waits, from the report's `STATEMENT`; `None` until that line is in the
    /// log.
    statement: Option<String>,
}

/// The lock waits of the server log `log`, in log order.
///
/// Under the image's `log_line_prefix`, `%m [%p] `, every line reads
/// `<date> <time> UTC [<pid>] <severity>:  <text>`. A wait is reported as
/// `LOG:  process <pid> still waiting for <lock> after <ms> ms`, and its end as
/// `LOG:  process <pid> acquired <lock> after <ms> ms`. The lines of the same pid that follow,
/// among which other backends' lines may fall, complete the report:
/// `DETAIL:  Process holding the lock: <pid>. Wait queue: <pids>.` (`Processes holding the lock:
/// <pid>, <pid>. …` for several holders), an optional `CONTEXT:  …`, then `STATEMENT:  <sql>`.
/// Any other message of that pid ends the report, and a line in any other shape, such as the
/// image's entrypoint output, is no report.
fn lock_waits(log: &[u8]) -> Vec<LockWait> {
    let text = String::from_utf8_lossy(log);
    let mut waits: Vec<LockWait> = Vec::new();
    // The report each backend's next `DETAIL` or `STATEMENT` line completes, by index in `waits`.
    let mut open: BTreeMap<u32, usize> = BTreeMap::new();
    for line in text.lines() {
        let Some((pid, severity, message)) = log_line(line) else {
            continue;
        };
        match severity {
            "DETAIL" => {
                let report = open.get(&pid).and_then(|&index| waits.get_mut(index));
                if let (Some(wait), Some(holders)) = (report, lock_holders(message)) {
                    wait.holders = holders;
                }
            }
            "STATEMENT" => {
                if let Some(wait) = open.remove(&pid).and_then(|index| waits.get_mut(index)) {
                    wait.statement = Some(message.to_owned());
                }
            }
            "CONTEXT" | "HINT" => {}
            _ => {
                open.remove(&pid);
                if let Some((acquired, lock)) = lock_report(pid, message) {
                    open.insert(pid, waits.len());
                    waits.push(LockWait {
                        pid,
                        acquired,
                        lock,
                        holders: Vec::new(),
                        statement: None,
                    });
                }
            }
        }
    }
    waits
}

/// The pid, severity and text of one server log line (see [`lock_waits`]), or `None` for a line
/// in another shape.
fn log_line(line: &str) -> Option<(u32, &str, &str)> {
    let (_, rest) = line.split_once(" [")?;
    let (pid, rest) = rest.split_once("] ")?;
    let (severity, text) = rest.split_once(":  ")?;
    Some((pid.parse().ok()?, severity, text))
}

/// Whether the `LOG` text `text` of backend `pid` reports a lock it still waits for (`false`) or
/// acquired after waiting (`true`), with that lock; `None` for any other message.
fn lock_report(pid: u32, text: &str) -> Option<(bool, String)> {
    let report = text.strip_prefix(&format!("process {pid} "))?;
    let (acquired, rest) = match report.strip_prefix("still waiting for ") {
        Some(rest) => (false, rest),
        None => (true, report.strip_prefix("acquired ")?),
    };
    let (lock, _) = rest.rsplit_once(" after ")?;
    Some((acquired, lock.to_owned()))
}

/// The holder pids of a lock wait's `DETAIL` text, or `None` when it names none.
fn lock_holders(text: &str) -> Option<Vec<u32>> {
    let listed = text
        .strip_prefix("Process holding the lock: ")
        .or_else(|| text.strip_prefix("Processes holding the lock: "))?;
    let (holders, _) = listed.split_once(". Wait queue: ")?;
    holders.split(", ").map(|pid| pid.parse().ok()).collect()
}

/// Reads the server log of `guard` every [`LOG_POLL`] until a lock wait satisfies `wanted`, and
/// returns that backend's pid.
///
/// When the run watched has ended, `ended` is set, and the call fails at once rather than at
/// [`LOG_BOUND`]: a run that ends before its wait is seen never waited. `awaited` names the wait
/// in the error, which never shows the log.
async fn await_lock_wait(
    guard: &PostgresGuard,
    awaited: &str,
    ended: Option<&AtomicBool>,
    wanted: impl Fn(&LockWait) -> bool,
) -> TestResult<u32> {
    let deadline = Instant::now() + LOG_BOUND;
    loop {
        // The flag is read before the log, so a run it shows ended had ended before the log was
        // read; a run that waits cannot end while the lock it waits for is held.
        let run_ended = ended.is_some_and(|flag| flag.load(Ordering::SeqCst));
        let log = guard.server_log().await?;
        if let Some(wait) = lock_waits(&log).iter().find(|&wait| wanted(wait)) {
            return Ok(wait.pid);
        }
        if run_ended {
            return Err(
                format!("{awaited}: the run ended before the server logged the wait").into(),
            );
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{awaited}: not in the server log within {} ms",
                LOG_BOUND.as_millis()
            )
            .into());
        }
        time::sleep(LOG_POLL).await;
    }
}

/// The scratch file `name` holding lines `range` of `acctdata.txt` (0-based, end excluded), and
/// the records those bytes decode to.
fn account_slice(
    scratch: &Scratch,
    fixture: &[u8],
    name: &str,
    range: Range<usize>,
) -> TestResult<(PathBuf, Vec<AccountRecord>)> {
    let lines: Vec<&[u8]> = fixture.split_inclusive(|&byte| byte == b'\n').collect();
    let slice = lines
        .get(range)
        .ok_or("acctdata.txt holds fewer lines than a slice needs")?;
    let bytes = concat(slice.iter().copied());
    let path = scratch.path(name);
    fs::write(&path, &bytes)?;
    Ok((path, input_records(&bytes)?))
}

/// The keys of `accounts`.
fn account_keys(accounts: &[AccountRecord]) -> BTreeSet<u64> {
    accounts.iter().map(|account| account.id.get()).collect()
}

/// The two waits of [`concurrent_runs_of_one_load_leave_one_input_set`], observed while the test
/// holds the lock of the first prior row: run A, already started, takes the table lock and waits
/// in its delete for the test's transaction; run B, started through `start_b` once that wait is
/// logged, waits for the table lock with A among its holders (the test's backend is listed too,
/// but its ROW SHARE does not conflict with SHARE ROW EXCLUSIVE); and while both wait, a read of
/// `accounts` returns at once and sees `prior`, the rows committed before either run. Returns the
/// pids of A and B.
///
/// An error drops `start_b` unsent when B has not been started, so B never runs.
async fn observe_overlap(
    guard: &PostgresGuard,
    reader: &mut PgStore,
    prior: &[AccountRecord],
    start_b: oneshot::Sender<()>,
    ended: [&AtomicBool; 2],
) -> TestResult<(u32, u32)> {
    let [a_ended, b_ended] = ended;
    let pid_a = await_lock_wait(
        guard,
        "run A waiting in its delete for the test's row lock",
        Some(a_ended),
        |wait| {
            !wait.acquired
                && wait.lock.starts_with("ShareLock on transaction ")
                && wait.statement.as_deref() == Some(DELETE_ACCOUNTS)
        },
    )
    .await?;
    start_b
        .send(())
        .map_err(|()| "run B is gone before its start")?;
    let pid_b = await_lock_wait(
        guard,
        "run B waiting for the table lock run A holds",
        Some(b_ended),
        |wait| {
            !wait.acquired
                && wait.pid != pid_a
                && wait.lock.starts_with("ShareRowExclusiveLock on relation ")
                && wait.holders.contains(&pid_a)
                && wait.statement.as_deref() == Some(LOCK_ACCOUNTS)
        },
    )
    .await?;

    let read = time::timeout(READ_BOUND, AccountRepository::find_all(reader))
        .await
        .map_err(|_| {
            format!(
                "accounts: a read while both runs wait did not return within {} ms",
                READ_BOUND.as_millis()
            )
        })??;
    // audit: LOADS-014
    // audit: LOADS-040
    assert_eq!(
        first_difference(prior, &read),
        None,
        "accounts while both runs wait: first row that differs from lines 1-10"
    );
    // audit: LOADS-040
    assert!(
        !a_ended.load(Ordering::SeqCst) && !b_ended.load(Ordering::SeqCst),
        "a run ended while the test held its row lock"
    );
    Ok((pid_a, pid_b))
}

/// Drives `a`, `b` and `c` concurrently on the current task, as Tokio's join macro does, and
/// returns their outputs once all three have finished. Every wake polls each future that has not
/// finished, so one that is pending never holds back the others, and a finished one is never
/// polled again. Each future is pinned in a box of its own, so no unchecked pin is needed, and
/// none has to be `'static` or `Send`, as spawning it would require.
async fn join3<A: Future, B: Future, C: Future>(
    a: A,
    b: B,
    c: C,
) -> (A::Output, B::Output, C::Output) {
    let (mut a, mut b, mut c) = (Some(Box::pin(a)), Some(Box::pin(b)), Some(Box::pin(c)));
    let (mut a_output, mut b_output, mut c_output) = (None, None, None);
    poll_fn(|cx| {
        poll_unfinished(&mut a, &mut a_output, cx);
        poll_unfinished(&mut b, &mut b_output, cx);
        poll_unfinished(&mut c, &mut c_output, cx);
        match (a_output.take(), b_output.take(), c_output.take()) {
            (Some(a), Some(b), Some(c)) => Poll::Ready((a, b, c)),
            unfinished => {
                (a_output, b_output, c_output) = unfinished;
                Poll::Pending
            }
        }
    })
    .await
}

/// Polls `future` with `cx` unless it has finished. Once it finishes, its output is kept in
/// `output` and the future is dropped, so it is never polled again.
fn poll_unfinished<F: Future>(
    future: &mut Option<Pin<Box<F>>>,
    output: &mut Option<F::Output>,
    cx: &mut Context<'_>,
) {
    if let Some(pinned) = future {
        if let Poll::Ready(value) = pinned.as_mut().poll(cx) {
            *output = Some(value);
            *future = None;
        }
    }
}

/// Two runs of `acctfile` whose transactions overlap, over disjoint inputs: lines 11-30 of
/// `acctdata.txt` (run A) and lines 31-50 (run B), on a table holding lines 1-10.
///
/// The test holds the row lock of account 1, so run A takes the table lock and its delete waits
/// for the test's transaction; run B starts only once the server logs that wait, and waits for
/// the table lock A holds. Both waits are read from the server log, which `log_lock_waits` makes
/// report each wait with its holders and statement; no sleep orders the runs. While both wait, a
/// reader still sees lines 1-10. The test then rolls its transaction back: A commits first, B
/// acquires the table lock A held and commits last. Both exit 0, and the table holds exactly the
/// records of lines 31-50: no row of lines 1-10 or of run A, so never a union of inputs.
#[test]
fn concurrent_runs_of_one_load_leave_one_input_set() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let scratch = Scratch::create("concurrent")?;
        let mut store = PgStore::new(guard.pool().clone());
        let mut reader = PgStore::new(guard.pool().clone());
        let fixture = fs::read(fixture_path("acctdata.txt"))?;
        let (prior_path, prior) = account_slice(&scratch, &fixture, "acctdata-1-10.txt", 0..10)?;
        let (a_path, a_records) = account_slice(&scratch, &fixture, "acctdata-11-30.txt", 10..30)?;
        let (b_path, b_records) = account_slice(&scratch, &fixture, "acctdata-31-50.txt", 30..50)?;
        let (prior_keys, a_keys, b_keys) = (
            account_keys(&prior),
            account_keys(&a_records),
            account_keys(&b_records),
        );
        assert!(
            prior_keys.is_disjoint(&a_keys)
                && prior_keys.is_disjoint(&b_keys)
                && a_keys.is_disjoint(&b_keys),
            "acctdata.txt: two of lines 1-10, 11-30 and 31-50 share a key"
        );

        let loaded = run_batch(
            &guard,
            &scratch,
            &["acctfile", "--input", path_arg(&prior_path)?],
            None,
        )
        .await?;
        // audit: LOADS-014
        assert_exit(&loaded, 0, "acctfile over lines 1-10");
        // audit: LOADS-014
        assert_eq!(
            first_difference(&prior, &AccountRepository::find_all(&mut store).await?),
            None,
            "accounts after lines 1-10: first row that differs"
        );

        // `SELECT … FOR UPDATE` takes ROW SHARE on the table, which the load's SHARE ROW EXCLUSIVE
        // does not conflict with, and the row lock, which the load's delete waits for.
        let mut held = store.begin().await?;
        let first_key = prior.first().ok_or("lines 1-10 hold no record")?.id;
        AccountRepository::lock_by_id(&mut held, first_key)
            .await?
            .ok_or("accounts: the first row of lines 1-10 is not stored")?;

        let logged = format!("{}?{LOG_LOCK_WAITS}", guard.database_url());
        let a_args = ["acctfile", "--input", path_arg(&a_path)?];
        let b_args = ["acctfile", "--input", path_arg(&b_path)?];
        let (a_ended, b_ended) = (AtomicBool::new(false), AtomicBool::new(false));
        let (start_b, b_started) = oneshot::channel::<()>();

        let run_a = async {
            let output = run_batch_on(&logged, &scratch, &a_args, None).await;
            a_ended.store(true, Ordering::SeqCst);
            output
        };
        let run_b = async {
            // An observer that fails before B is due drops the sender, and B never runs.
            if b_started.await.is_err() {
                return Ok(None);
            }
            let output = run_batch_on(&logged, &scratch, &b_args, None).await;
            b_ended.store(true, Ordering::SeqCst);
            output.map(Some)
        };
        let observer = async {
            let observed =
                observe_overlap(&guard, &mut reader, &prior, start_b, [&a_ended, &b_ended]).await;
            // Whatever was observed, ending the test's transaction lets A, then B, go on, so no run
            // is left waiting.
            held.rollback().await?;
            observed
        };
        let (a_output, b_output, overlap) = join3(run_a, run_b, observer).await;

        // audit: LOADS-040
        assert_exit(&a_output?, 0, "acctfile over lines 11-30");
        let b_output: Option<Output> = b_output?;
        if let Some(output) = &b_output {
            // audit: LOADS-040
            assert_exit(output, 0, "acctfile over lines 31-50");
        }
        let (_, pid_b) = overlap?;
        if b_output.is_none() {
            return Err("acctfile over lines 31-50 never started".into());
        }
        // audit: LOADS-040
        await_lock_wait(
            &guard,
            "run B acquiring the table lock after run A ended",
            None,
            |wait| {
                wait.acquired
                    && wait.pid == pid_b
                    && wait.lock.starts_with("ShareRowExclusiveLock on relation ")
            },
        )
        .await?;

        let after = AccountRepository::find_all(&mut store).await?;
        // audit: LOADS-014
        // audit: LOADS-040
        assert_eq!(
            after.len(),
            b_records.len(),
            "accounts after both runs: rows against lines 31-50"
        );
        // audit: LOADS-014
        // audit: LOADS-040
        assert_eq!(
            first_difference(&b_records, &after),
            None,
            "accounts after both runs: first row that differs from lines 31-50"
        );
        Ok(())
    })
}

/// Against a database `carddemo-migrate` has not migrated, the maintenance database `postgres`
/// of the same server, every load fails at its first statement and creates no schema, so the
/// same load fails again on a second run.
#[test]
fn a_load_never_migrates() -> TestResult {
    block_on(async {
        let guard = start_postgres().await?;
        let scratch = Scratch::create("unmigrated")?;
        let (server, _) = guard
            .database_url()
            .rsplit_once('/')
            .ok_or("the database URL names no database")?;
        let unmigrated = format!("{server}/postgres");
        let fixtures = fixture_dir();
        for run in ["first", "second"] {
            for job in DEV_UP_ORDER {
                let output = run_batch_on(&unmigrated, &scratch, &[job], Some(&fixtures)).await?;
                // audit: LOADS-021
                assert_failed(&output, &format!("{run} {job} on an unmigrated database"));
            }
        }
        // audit: LOADS-021
        assert_no_file_written(&scratch)?;
        Ok(())
    })
}
