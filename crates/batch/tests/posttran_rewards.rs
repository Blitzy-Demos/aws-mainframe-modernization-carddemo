//! The reward accrual `batch posttran` adds to every posted record, and the halt when that
//! accrual fails (AAP 0.3.5, 0.5.3.5).
//!
//! The posted set is audit D: every `dailytran.txt` record that the D.1 reject rows do not
//! list, in file order (P-CBTRN02C-216), with the counts of P-CBTRN02C-065. Each record's
//! type, category and amount are read from the fixture with the `CVTRA06Y` codec, and its
//! account is the `XREF-ACCT-ID` of its card in `cardxref.txt`, the account POSTTRAN credits.
//!
//! Rewards have no COBOL oracle. The expected points follow the rewards rule: an amount above
//! zero accrues `ceil(amount × reward_rate)` under the highest-rated rule whose type and
//! category match and which is active at the processing instant. The seeded rules are retail
//! `01`/`0001` at 1.0000 and travel `04`/`0003` at 3.0000, both active from
//! 2022-01-01T00:00:00Z (AAP 0.3.6). The fixture day holds 250 `01`/`0001` records, all
//! positive, 50 `03`/`0001` records, all negative, and no type `04`: only the retail rule
//! accrues, once per posted `01`/`0001` record.
//!
//! POSTTRAN commits each posted record's CBTRN02C updates and its accrual in one unit of
//! work. When `RewardEngine::accrue` fails, that unit is rolled back whole, the records before
//! it stay committed, the run stops with exit 8 and the record number and TRAN-ID on stderr,
//! and `DALYREJS(+1)` is discarded uncataloged. A test-only trigger on `reward_accruals` makes
//! the accrual of one chosen record fail on the server. The halt runs twice, each on a fresh
//! database: over an empty batch directory, and over a full `DALYREJS` GDG of five generations
//! with known bytes, where cataloging `(+1)` would scratch the oldest. Both runs leave
//! `DALYREJS` as it was: the same generations holding the same bytes, and no new generation,
//! pending temporary file or allocation lock.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;

use carddemo_audit::Document;
use carddemo_codec::{CardXrefLayout, Charset, DailyTranLayout, Layout, pad};
use carddemo_domain::entities::{DailyTran, RewardAccrual, RewardBalance, RewardRule};
use carddemo_domain::numeric::{AccountId, CardNumber, RewardPoints, RuleId};
use chrono::{DateTime, Utc};
use common::{Harness, TRANREPT_CLOCK, block_on, expected_value, fixture_lines, parity};
use rust_decimal::Decimal;

/// The exit code of a run that rejected records: CBTRN02C moves 4 to `RETURN-CODE` when
/// `WS-REJECT-COUNT > 0` (P-CBTRN02C-065).
const RECORDS_REJECTED: i32 = 4;

/// The exit code of a reward-engine halt, `posttran` only (AAP 0.5.3.5).
const REWARD_HALT: i32 = 8;

/// The D.1 rows of the 38 records POSTTRAN rejects over the fixture day, each naming its
/// `record_no` and `tran_id`. Every other record is posted (P-CBTRN02C-216).
const REJECT_IDS: [&str; 38] = [
    "P-CBTRN02C-027",
    "P-CBTRN02C-028",
    "P-CBTRN02C-029",
    "P-CBTRN02C-030",
    "P-CBTRN02C-031",
    "P-CBTRN02C-032",
    "P-CBTRN02C-033",
    "P-CBTRN02C-034",
    "P-CBTRN02C-035",
    "P-CBTRN02C-036",
    "P-CBTRN02C-037",
    "P-CBTRN02C-038",
    "P-CBTRN02C-039",
    "P-CBTRN02C-040",
    "P-CBTRN02C-041",
    "P-CBTRN02C-042",
    "P-CBTRN02C-043",
    "P-CBTRN02C-044",
    "P-CBTRN02C-045",
    "P-CBTRN02C-046",
    "P-CBTRN02C-047",
    "P-CBTRN02C-048",
    "P-CBTRN02C-049",
    "P-CBTRN02C-050",
    "P-CBTRN02C-051",
    "P-CBTRN02C-052",
    "P-CBTRN02C-053",
    "P-CBTRN02C-054",
    "P-CBTRN02C-055",
    "P-CBTRN02C-056",
    "P-CBTRN02C-057",
    "P-CBTRN02C-058",
    "P-CBTRN02C-059",
    "P-CBTRN02C-060",
    "P-CBTRN02C-061",
    "P-CBTRN02C-062",
    "P-CBTRN02C-063",
    "P-CBTRN02C-064",
];

/// The D.1 row of POSTTRAN's end-of-job counts and exit code over the fixture day.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// The D.2 row of the posted set: its count and its first and last TRAN-ID.
const POSTED_SET: &str = "P-CBTRN02C-216";

/// The daily transaction fixture, POSTTRAN's default `--input`, DD `DALYTRAN`
/// [app/jcl/POSTTRAN.jcl:L30-31].
const DAILY_FILE: &str = "dailytran.txt";

/// The card cross-reference fixture, whose records are stored as 36 of their 50 bytes (E-04).
const XREF_FILE: &str = "cardxref.txt";

/// The reject dataset, `DALYREJS(+1)` [app/jcl/POSTTRAN.jcl:L34-38].
const DALYREJS: &str = "DALYREJS";

/// The width of a `DALYREJS` record, the CBTRN02C `REJECT-RECORD`: `LRECL=430`
/// [app/jcl/POSTTRAN.jcl:L34-38].
const REJECT_RECORD_LEN: usize = 430;

/// The generations of a full `DALYREJS` GDG, oldest first: `LIMIT(5) SCRATCH` keeps the newest
/// 5 [app/jcl/DALYREJS.jcl:L24-28], so cataloging `(+1)` over them would scratch `G0001V00`.
const FULL_DALYREJS: [u16; 5] = [1, 2, 3, 4, 5];

/// The page size of the reward history reads: above the 262 posted records, so one page holds
/// every accrual of an account.
const HISTORY_LIMIT: u16 = 300;

/// The retail seed rule's type and category codes: `01` Purchase, `0001` Regular Sales Draft.
const RETAIL: (&str, u16) = ("01", 1);

/// The travel seed rule's type and category codes: `04` Authorization, `0003` Travel booking
/// authorization.
const TRAVEL: (&str, u16) = ("04", 3);

/// The retail seed rule's `reward_rate`, 1.0000 points per dollar.
const RETAIL_RATE: Decimal = Decimal::from_parts(10_000, 0, 0, false, 4);

/// The travel seed rule's `reward_rate`, 3.0000 points per dollar.
const TRAVEL_RATE: Decimal = Decimal::from_parts(30_000, 0, 0, false, 4);

/// The TRAN-ID of the record whose accrual the injected failure stops: record 3, the first
/// posted `01`/`0001` record of account 5. The halt test derives the record from the audit
/// and the fixture and checks that it is this one.
///
/// A macro, so the trigger below is one constant: test SQL is never built with `format!`
/// (`crates/testkit/tests/hygiene.rs`).
macro_rules! halting_tran_id {
    () => {
        "0000000006292564"
    };
}

/// The TRAN-ID `halting_tran_id!` expands to.
const HALTING_TRAN_ID: &str = halting_tran_id!();

/// A trigger that fails the accrual insert of the halting record on the server. The record's
/// transaction row is already written in the same unit of work, so the live master view
/// resolves the accrual's row identity to its TRAN-ID.
const FAIL_HALTING_ACCRUAL: &str = concat!(
    r"
CREATE FUNCTION fail_halting_accrual() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF EXISTS (SELECT 1 FROM transaction_master m
               WHERE m.transaction_id = NEW.transaction_id AND m.tran_id = '",
    halting_tran_id!(),
    r"') THEN
        RAISE EXCEPTION 'injected reward failure';
    END IF;
    RETURN NEW;
END
$$;
CREATE TRIGGER fail_halting_accrual BEFORE INSERT ON reward_accruals
    FOR EACH ROW EXECUTE FUNCTION fail_halting_accrual();
"
);

/// The balances of every account, in key order: the fields `2800-UPDATE-ACCOUNT-REC` writes.
const ACCOUNT_BALANCES_SQL: &str = "SELECT account_id, acct_curr_bal, acct_curr_cyc_credit, \
     acct_curr_cyc_debit FROM accounts ORDER BY account_id";

/// Every category balance, in key order: the records `2700-UPDATE-TCATBAL` writes.
const CATEGORY_BALANCES_SQL: &str = "SELECT trancat_acct_id, trancat_type_cd, trancat_cd, \
     tran_cat_bal FROM transaction_category_balances \
     ORDER BY trancat_acct_id, trancat_type_cd, trancat_cd";

/// One record POSTTRAN posts.
#[derive(Debug)]
struct Posted {
    /// Its position in `dailytran.txt`, counted from 1.
    record_number: usize,
    /// Its `DALYTRAN-ID`, as the live master stores it.
    tran_id: String,
    /// The `XREF-ACCT-ID` of its card: the account POSTTRAN credits.
    account_id: AccountId,
    /// The retail rule's points for it, `ceil(amount × 1.0000)`: `Some` for a `01`/`0001`
    /// record with an amount above zero, the only records a seed rule matches on the fixture
    /// day; `None` for every other record.
    points: Option<RewardPoints>,
}

/// The fixture day as the audit posts it.
struct Corpus {
    /// The posted records, in file order.
    posted: Vec<Posted>,
    /// Every account of the card cross-reference.
    accounts: BTreeSet<AccountId>,
}

impl Corpus {
    /// The posted records of `doc`'s audit D over the fixture files, cross-checked with the
    /// audited counts (P-CBTRN02C-065) and the first and last posted TRAN-ID
    /// (P-CBTRN02C-216).
    fn load(doc: &Document) -> Self {
        let rejected = rejected_records(doc);
        let card_accounts = card_accounts();
        let lines = fixture_lines(DAILY_FILE);
        assert_eq!(audited_count(doc, POSTTRAN_COUNTS, "read"), lines.len());
        assert_eq!(
            audited_count(doc, POSTTRAN_COUNTS, "rejected"),
            rejected.len()
        );
        let mut posted = Vec::new();
        for (line, record_number) in lines.iter().zip(1_usize..) {
            let daily = DailyTranLayout::decode(line, Charset::Ascii)
                .unwrap_or_else(|error| panic!("{DAILY_FILE} record {record_number}: {error}"))
                .daily_tran;
            let tran_id = daily.dalytran_id.trimmed().into_owned();
            if let Some(reject) = rejected.get(&record_number) {
                assert_eq!(*reject, tran_id, "reject row of record {record_number}");
                continue;
            }
            let account_id = *card_accounts
                .get(&daily.dalytran_card_num)
                .unwrap_or_else(|| panic!("record {record_number}: card not in {XREF_FILE}"));
            posted.push(Posted {
                record_number,
                tran_id,
                account_id,
                points: retail_points(&daily),
            });
        }
        assert_eq!(audited_count(doc, POSTED_SET, "posted"), posted.len());
        assert_eq!(audited_count(doc, POSTTRAN_COUNTS, "posted"), posted.len());
        let ends = [posted.first(), posted.last()].map(|end| end.map(|p| p.tran_id.clone()));
        let audited_ends = ["first", "last"].map(|key| Some(expected_value(doc, POSTED_SET, key)));
        assert_eq!(ends, audited_ends, "first and last posted TRAN-ID");
        Self {
            posted,
            accounts: card_accounts.into_values().collect(),
        }
    }
}

/// The TRAN-ID of each rejected record by its record number, from the [`REJECT_IDS`] rows.
fn rejected_records(doc: &Document) -> BTreeMap<usize, String> {
    REJECT_IDS
        .iter()
        .map(|id| {
            let record_number = audited_count(doc, id, "record_no");
            (record_number, expected_value(doc, id, "tran_id"))
        })
        .collect()
}

/// The count under `key` in the `Expected result` of audit row `id`.
fn audited_count(doc: &Document, id: &str, key: &str) -> usize {
    let value = expected_value(doc, id, key);
    value
        .parse()
        .unwrap_or_else(|error| panic!("{id} {key}={value} is not a count: {error}"))
}

/// The `XREF-ACCT-ID` of every `XREF-CARD-NUM` of the card cross-reference fixture.
fn card_accounts() -> BTreeMap<CardNumber, AccountId> {
    fixture_lines(XREF_FILE)
        .iter()
        .zip(1_usize..)
        .map(|(line, number)| {
            let record = pad(CardXrefLayout::NAME, line, CardXrefLayout::LEN)
                .and_then(|padded| CardXrefLayout::decode(&padded, Charset::Ascii))
                .unwrap_or_else(|error| panic!("{XREF_FILE} record {number}: {error}"));
            (record.xref.xref_card_num, record.xref.xref_acct_id)
        })
        .collect()
}

/// The retail rule's points for `daily`: `ceil(DALYTRAN-AMT × 1.0000)` for a `01`/`0001`
/// record whose amount is above zero, else `None`.
fn retail_points(daily: &DailyTran) -> Option<RewardPoints> {
    let amount = daily.dalytran_amt.to_decimal();
    let retail = daily.dalytran_type_cd.trimmed() == RETAIL.0
        && daily.dalytran_cat_cd.get() == RETAIL.1
        && amount > Decimal::ZERO;
    retail.then(|| {
        let points = (amount * RETAIL_RATE).ceil();
        i64::try_from(points)
            .map_err(|error| error.to_string())
            .and_then(|whole| RewardPoints::try_from(whole).map_err(|error| error.to_string()))
            .unwrap_or_else(|error| panic!("{points} is not a points value: {error}"))
    })
}

/// What the reward store holds after a run, read through the persistence adapters.
struct Rewards {
    /// Every reward rule, in `rule_id` order.
    rules: Vec<RewardRule>,
    /// Per account, its reward history (newest first) and its balance row, if any.
    accounts: BTreeMap<AccountId, (Vec<RewardAccrual>, Option<RewardBalance>)>,
}

/// The rules, and the history and balance of every account of `accounts`, read through
/// `PgStore` in one unit of work that is rolled back.
fn read_rewards(h: &Harness, accounts: &BTreeSet<AccountId>) -> Rewards {
    let db = h.db();
    block_on(async {
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("cannot begin the reward read: {error}"));
        let rewards = {
            let mut store = uow.store();
            let rules = store
                .list_reward_rules()
                .await
                .unwrap_or_else(|error| panic!("reward rule read failed: {error}"));
            let mut read = BTreeMap::new();
            for &account in accounts {
                let history = store
                    .reward_history(account, None, HISTORY_LIMIT)
                    .await
                    .unwrap_or_else(|error| panic!("history of {account} failed: {error}"));
                let balance = store
                    .reward_balance(account)
                    .await
                    .unwrap_or_else(|error| panic!("balance of {account} failed: {error}"));
                read.insert(account, (history, balance));
            }
            Rewards {
                rules,
                accounts: read,
            }
        };
        uow.rollback()
            .await
            .unwrap_or_else(|error| panic!("reward read rollback failed: {error}"));
        rewards
    })
}

/// The seed rule with the type and category codes `codes`, checked to have `rate` and to be
/// active at `at`.
fn seed_rule(rules: &[RewardRule], codes: (&str, u16), rate: Decimal, at: DateTime<Utc>) -> RuleId {
    let rule = rules
        .iter()
        .find(|rule| {
            rule.transaction_type_cd.trimmed() == codes.0
                && rule.transaction_category_cd.get() == codes.1
        })
        .unwrap_or_else(|| panic!("no seed rule for {codes:?} in {rules:?}"));
    assert_eq!(rule.reward_rate.to_decimal(), rate, "rate of {rule:?}");
    assert!(rule.is_active_at(at), "{rule:?} is not active at {at}");
    rule.rule_id
}

/// The processing clock the runs are pinned to.
fn clock() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(TRANREPT_CLOCK)
        .unwrap_or_else(|error| panic!("{TRANREPT_CLOCK} is not RFC 3339: {error}"))
        .with_timezone(&Utc)
}

/// The TRAN-ID of every live transaction by its row identity.
fn live_tran_ids(h: &Harness) -> BTreeMap<i64, String> {
    h.live_master_rows()
        .into_iter()
        .map(|row| {
            let text = |index: usize| {
                row[index]
                    .clone()
                    .unwrap_or_else(|| panic!("live master row {row:?} holds a null"))
            };
            let identity = text(0);
            let row_id = identity
                .parse()
                .unwrap_or_else(|error| panic!("transaction_id {identity}: {error}"));
            (row_id, text(1))
        })
        .collect()
}

/// Checks the reward store against the accruals `posted` earns: exactly one accrual, under
/// the retail rule, at the processing clock, for each record with points, crediting its
/// account with those points; no accrual for any other TRAN-ID and none under the travel rule;
/// and per account of `accounts` a balance earning the sum of its records' points, with
/// nothing redeemed and `available = earned − redeemed`.
fn assert_accruals(h: &Harness, posted: &[Posted], accounts: &BTreeSet<AccountId>) {
    let expected: BTreeMap<&str, (AccountId, RewardPoints)> = posted
        .iter()
        .filter_map(|record| {
            let points = record.points?;
            Some((record.tran_id.as_str(), (record.account_id, points)))
        })
        .collect();
    assert!(
        !expected.is_empty(),
        "no posted record accrues, so the check is vacuous"
    );
    let tran_ids = live_tran_ids(h);
    let rewards = read_rewards(h, accounts);
    let at = clock();
    let retail = seed_rule(&rewards.rules, RETAIL, RETAIL_RATE, at);
    let travel = seed_rule(&rewards.rules, TRAVEL, TRAVEL_RATE, at);
    let mut actual = BTreeMap::new();
    for (account, (history, _)) in &rewards.accounts {
        assert!(
            history.len() <= usize::from(HISTORY_LIMIT),
            "{account}: history truncated"
        );
        for accrual in history {
            let tran_id = tran_ids
                .get(&accrual.transaction_id.get())
                .unwrap_or_else(|| panic!("{accrual:?} names no live transaction"));
            assert_eq!(accrual.account_id, *account, "{tran_id}: {accrual:?}");
            assert_ne!(
                accrual.rule_id, travel,
                "{tran_id} accrued under the travel rule"
            );
            assert_eq!(accrual.rule_id, retail, "{tran_id}: {accrual:?}");
            assert_eq!(accrual.created_at, at, "{tran_id}: accrual time");
            let earlier = actual.insert(tran_id.as_str(), (*account, accrual.points_earned));
            assert!(earlier.is_none(), "{tran_id} has more than one accrual");
        }
    }
    let missing: Vec<&&str> = expected
        .keys()
        .filter(|id| !actual.contains_key(*id))
        .collect();
    let extra: Vec<&&str> = actual
        .keys()
        .filter(|id| !expected.contains_key(*id))
        .collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "accruals missing for {missing:?}; accruals not expected for {extra:?}"
    );
    assert_eq!(actual, expected, "account and points per TRAN-ID");
    assert_balances(&rewards, &expected);
}

/// Checks every balance of `rewards` against the points `expected` credits its account.
fn assert_balances(rewards: &Rewards, expected: &BTreeMap<&str, (AccountId, RewardPoints)>) {
    let mut earned: BTreeMap<AccountId, RewardPoints> = BTreeMap::new();
    for &(account, points) in expected.values() {
        let total = earned.entry(account).or_insert(RewardPoints::ZERO);
        *total = total
            .checked_add(points)
            .unwrap_or_else(|_| panic!("the points of {account} overflow"));
    }
    for (account, (_, balance)) in &rewards.accounts {
        let want = earned.remove(account).unwrap_or(RewardPoints::ZERO);
        let Some(balance) = balance else {
            assert_eq!(
                want,
                RewardPoints::ZERO,
                "{account} accrued but has no balance"
            );
            continue;
        };
        assert_eq!(balance.account_id, *account);
        assert_eq!(
            balance.total_points_earned, want,
            "{account}: points earned"
        );
        assert_eq!(
            balance.total_points_redeemed,
            RewardPoints::ZERO,
            "{account}"
        );
        let available = balance
            .total_points_earned
            .checked_sub(balance.total_points_redeemed);
        assert_eq!(
            Ok(balance.available_points),
            available,
            "{account}: available"
        );
    }
    assert!(
        earned.is_empty(),
        "accounts outside the read accrued: {earned:?}"
    );
}

/// The halting record: the first posted `01`/`0001` record with points that is the first
/// posted record of its account and not the first posted record of the file, with its index
/// in `posted`. Its account then still holds its seeded balances when it halts, and an
/// earlier record has committed an accrual.
fn halting_record(posted: &[Posted]) -> (usize, &Posted) {
    posted
        .iter()
        .enumerate()
        .skip(1)
        .find(|&(index, record)| {
            record.points.is_some()
                && posted[..index]
                    .iter()
                    .all(|earlier| earlier.account_id != record.account_id)
        })
        .unwrap_or_else(|| panic!("no posted record qualifies as the halting record"))
}

/// The balances of `account` and its category balance records, as `PostgreSQL` text.
fn account_state(h: &Harness, account: AccountId) -> Vec<Vec<Option<String>>> {
    let key = Some(account.get().to_string());
    let rows = block_on(h.stack().fetch_text_rows(ACCOUNT_BALANCES_SQL))
        .into_iter()
        .chain(block_on(h.stack().fetch_text_rows(CATEGORY_BALANCES_SQL)));
    rows.filter(|row| row.first() == Some(&key)).collect()
}

/// Catalogs the generations `numbers` of `DALYREJS` in the batch directory of `h`, as earlier
/// runs leave them, and returns each file name with the bytes written. Generation `n` holds `n`
/// records of [`REJECT_RECORD_LEN`] bytes, each its generation's name and record number padded
/// with spaces and followed by `LF`, so no two generations hold the same bytes or length.
/// POSTTRAN never reads `DALYREJS`, so the records need not be rejects.
fn catalog_rejects(h: &Harness, numbers: &[u16]) -> BTreeMap<String, Vec<u8>> {
    numbers
        .iter()
        .map(|&number| {
            let name = format!("{DALYREJS}.G{number:04}V00");
            let mut bytes = Vec::new();
            for record in 1..=number {
                let mut line = format!("{name} RECORD {record}").into_bytes();
                line.resize(REJECT_RECORD_LEN, b' ');
                line.push(b'\n');
                bytes.extend(line);
            }
            let path = h.batch_dir().join(&name);
            fs::write(&path, &bytes)
                .unwrap_or_else(|error| panic!("cannot write {}: {error}", path.display()));
            (name, bytes)
        })
        .collect()
}

/// The names of the entries of the batch directory that belong to `DALYREJS`: cataloged
/// generations, a pending generation's hidden temporary file and the allocation lock.
fn dalyrejs_entries(h: &Harness) -> BTreeSet<String> {
    let entries = match fs::read_dir(h.batch_dir()) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return BTreeSet::new(),
        Err(error) => panic!("cannot list {}: {error}", h.batch_dir().display()),
    };
    entries
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("cannot read the batch directory: {error}"))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.contains(DALYREJS))
        .collect()
}

/// Runs `batch posttran` over the fixture day on a fresh harness whose `DALYREJS` GDG holds the
/// generations `prior` ([`catalog_rejects`]), with the accrual of [`HALTING_TRAN_ID`] failing on
/// the server, and checks the halt: exit 8 with the record number and TRAN-ID on stderr; the
/// records before the halting one posted with their accruals; the halting record and its
/// account rolled back; and `DALYREJS` unchanged: the same generations holding the same bytes,
/// and no other `DALYREJS` entry, so no new generation, pending temporary file or allocation
/// lock.
fn assert_reward_halt(prior: &[u16]) {
    let corpus = Corpus::load(&parity());
    let (index, halting) = halting_record(&corpus.posted);
    assert_eq!(
        halting.tran_id, HALTING_TRAN_ID,
        "the trigger names the halting record"
    );
    let h = Harness::new();
    let written = catalog_rejects(&h, prior);
    let names: BTreeSet<String> = written.keys().cloned().collect();
    let cataloged: Vec<PathBuf> = names.iter().map(|name| h.batch_dir().join(name)).collect();
    let rejects_before = h.generations(DALYREJS);
    assert_eq!(
        rejects_before, cataloged,
        "DALYREJS generations before the run"
    );
    assert_eq!(
        dalyrejs_entries(&h),
        names,
        "DALYREJS entries before the run"
    );
    let before = account_state(&h, halting.account_id);
    let category_key = [RETAIL.0.to_owned(), RETAIL.1.to_string()].map(Some);
    assert!(
        before
            .iter()
            .any(|row| row.get(1..3) == Some(&category_key[..])),
        "{} has no 01/0001 category balance: {before:?}",
        halting.account_id
    );
    block_on(h.stack().exec_sql(FAIL_HALTING_ACCRUAL));

    let out = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), REWARD_HALT);

    let stderr = String::from_utf8_lossy(&out.stderr);
    let lines: Vec<&str> = stderr.lines().collect();
    // The rendering of `ExitStatus::RewardHalt`, followed by the engine's error.
    let report = format!(
        "REWARD ENGINE HALT: record {} TRAN-ID {}: ",
        halting.record_number, halting.tran_id
    );
    assert!(
        lines.len() == 1 && lines[0].starts_with(&report),
        "stderr must be one line starting {report:?}: {lines:?}"
    );
    let live: BTreeSet<String> = live_tran_ids(&h).into_values().collect();
    assert!(
        !live.contains(HALTING_TRAN_ID),
        "the halting record stays unposted"
    );
    let committed: BTreeSet<String> = corpus.posted[..index]
        .iter()
        .map(|p| p.tran_id.clone())
        .collect();
    assert_eq!(
        live, committed,
        "exactly the records before the halt stay posted"
    );
    assert_eq!(account_state(&h, halting.account_id), before, "rolled back");
    assert_accruals(&h, &corpus.posted[..index], &corpus.accounts);
    assert_eq!(
        h.generations(DALYREJS),
        rejects_before,
        "no DALYREJS generation added or scratched"
    );
    for (name, bytes) in &written {
        let path = h.batch_dir().join(name);
        let now = fs::read(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        assert!(
            now == *bytes,
            "{name} changed: {} bytes written, {} bytes now",
            bytes.len(),
            now.len()
        );
    }
    assert_eq!(
        dalyrejs_entries(&h),
        names,
        "no new generation, pending temporary file or allocation lock of DALYREJS remains"
    );
}

#[test]
fn posting_accrues_retail_points_per_posted_record() {
    // P-CBTRN02C-065, P-CBTRN02C-216 and the D.1 rows of REJECT_IDS.
    let corpus = Corpus::load(&parity());
    let h = Harness::new();

    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), RECORDS_REJECTED);

    let live: BTreeSet<String> = live_tran_ids(&h).into_values().collect();
    let posted: BTreeSet<String> = corpus.posted.iter().map(|p| p.tran_id.clone()).collect();
    assert_eq!(
        live, posted,
        "the live master holds exactly the audited posted set"
    );
    assert_accruals(&h, &corpus.posted, &corpus.accounts);
}

#[test]
fn reward_failure_halts_posting_with_exit_8() {
    // P-CBTRN02C-065, P-CBTRN02C-216 and the D.1 rows of REJECT_IDS, over an empty batch
    // directory.
    assert_reward_halt(&[]);
}

#[test]
fn reward_halt_leaves_every_cataloged_dalyrejs_generation_unchanged() {
    // P-CBTRN02C-065, P-CBTRN02C-216 and the D.1 rows of REJECT_IDS, over a full DALYREJS GDG.
    assert_reward_halt(&FULL_DALYREJS);
}
