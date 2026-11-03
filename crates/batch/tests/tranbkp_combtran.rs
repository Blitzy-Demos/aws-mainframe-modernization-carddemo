//! TRANBKP and COMBTRAN against the live transaction master (E-09).
//!
//! Every case first runs the README cycle up to the backup [README.md:L225-227]: `posttran`
//! over the fixture day, then `intcalc`, both on the TRANREPT clock. The live master then
//! holds the posted records with their reward accruals (P-CBTRN02C-065), and `SYSTRAN(0)` the
//! interest records (P-CBACT04C-173). Then:
//!
//! - `tranbkp` backs up the live records in TRAN-ID order and empties the live master, which
//!   CT00, CT01 and the CT02 TRAN-ID allocation see as an empty file, while every reward
//!   accrual keeps its transaction row (P-TRANBKP-001, P-TRANBKP-003 to P-TRANBKP-005).
//! - The README case: `combtran` from the emptied master sorts the backup and the interest
//!   records into `TRANSACT.COMBINED(+1)` and loads all of them, reviving the accrued rows
//!   (P-COMBTRAN-001, P-COMBTRAN-002, P-COMBTRAN-004, P-COMBTRAN-005).
//! - The interleaved cases: CT02 rows that already hold TRAN-IDs of combined records keep
//!   their content, and the load follows the `REPRO` duplicate-key rows (P-COMBTRAN-003,
//!   P-COMBTRAN-006 to P-COMBTRAN-008).
//!
//! CT00, CT01 and CT02 run through the `common` helpers, which drive the domain rules and the
//! persistence adapters the API handlers drive. The expected values are read from
//! `docs/parity/parity-oracle-audit.md`.

pub mod common;

use std::collections::BTreeSet;
use std::fmt::Display;
use std::str::FromStr;

use carddemo_audit::Document;
use carddemo_codec::{Charset, Layout, TransactionLayout, TransactionRecord, encode_ascii_lf};
use carddemo_domain::entities::{NewTransaction, RewardAccrual, RewardBalance};
use carddemo_domain::error::{DomainError, ErrorKind};
use carddemo_domain::messages::{TRANSACTION_ID_NOT_FOUND, YOU_ARE_AT_THE_TOP_OF_THE_PAGE};
use carddemo_domain::numeric::{
    AccountId, Alnum, CardNumber, Money, TranCategoryCode, TranId, TranTypeCode,
};
use carddemo_domain::ports::{PortError, PortResult};
use carddemo_domain::rules::transaction_add;
use carddemo_persistence::Db;
use common::{
    Harness, LIVE_MASTER_COLUMNS, TRANREPT_CLOCK, block_on, ct00_first_page, ct01_view, ct02_add,
    ct02_input, expected_span, expected_value, parity, parity_cell, parity_row, split_records,
};
use rust_decimal::Decimal;

/// Every audit row this file checks: the setup counts, the TRANBKP and COMBTRAN rows of
/// sections B and D, the empty-file rows of CT00, CT01 and the CT02 allocation, and the CT02
/// allocation itself.
const P_IDS: &[&str] = &[
    "P-CBTRN02C-065",
    "P-CBACT04C-173",
    "P-TRANBKP-001",
    "P-TRANBKP-003",
    "P-TRANBKP-004",
    "P-TRANBKP-005",
    "P-COTRN00C-011",
    "P-COTRN01C-004",
    "P-COTRN02C-020",
    "P-COTRN02C-021",
    "P-COMBTRAN-001",
    "P-COMBTRAN-002",
    "P-COMBTRAN-003",
    "P-COMBTRAN-004",
    "P-COMBTRAN-005",
    "P-COMBTRAN-006",
    "P-COMBTRAN-007",
    "P-COMBTRAN-008",
];

/// POSTTRAN over the fixture day from the seeded state: `posted=262; exit=4`.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// The `SYSTRAN(+1)` INTCALC writes: `records=50`.
const SYSTRAN_RECORDS: &str = "P-CBACT04C-173";

/// CT00 on an empty live master: the empty page with its message.
const CT00_EMPTY_FILE: &str = "P-COTRN00C-011";

/// CT01 `READ` `NOTFND`.
const CT01_NOT_FOUND: &str = "P-COTRN01C-004";

/// The CT02 TRAN-ID allocation on an empty live master: `STARTBR` `NOTFND`, no add (E-27).
const CT02_EMPTY_FILE: &str = "P-COTRN02C-021";

/// `TRANSACT.COMBINED(+1)` after POSTTRAN, INTCALC and TRANBKP.
const COMBINED_CORPUS: &str = "P-COMBTRAN-004";

/// The README case: the `REPRO` into the live master TRANBKP emptied.
const README_LOAD: &str = "P-COMBTRAN-005";

/// The interleaved case that ends below the error limit: two combined records collide.
const INTERLEAVED_LOAD: &str = "P-COMBTRAN-008";

/// The interleaved cases that reach the `REPRO` error limit of 4 and stop.
const ERROR_LIMIT_LOADS: [&str; 2] = ["P-COMBTRAN-006", "P-COMBTRAN-007"];

/// The condition code of TRANBKP when the live master holds records: `REPRO` ends with 0
/// (P-TRANBKP-001), and so do the `DELETE`s and the `DEFINE` after it (P-TRANBKP-003,
/// P-TRANBKP-004).
const TRANBKP_EXIT: i32 = 0;

/// The INTCALC command of the JCL: `PARM='2022071800'` [app/jcl/INTCALC.jcl:L22].
const INTCALC: [&str; 3] = ["intcalc", "--parm-date", "2022071800"];

/// INTCALC ends normally over the posted corpus (`GOBACK`).
const INTCALC_EXIT: i32 = 0;

/// The `TRANSACT` datasets and `SYSTRAN` (AAP 0.5.3.5).
const BKUP: &str = "TRANSACT.BKUP";
const SYSTRAN: &str = "SYSTRAN";
const COMBINED: &str = "TRANSACT.COMBINED";

/// `CVTRA05Y` `TRAN-RECORD`: `RECORDSIZE(350 350)` [app/jcl/TRANBKP.jcl:L59].
const RECORD_LEN: usize = 350;

/// The sort key `TRAN-ID,1,16,CH` [app/jcl/COMBTRAN.jcl:L28].
const KEY_LEN: usize = 16;

/// More reward accruals than any fixture account earns from one fixture day, so a single
/// history page holds them all.
const HISTORY_LIMIT: u16 = 1_000;

/// Every fixture account, in key order.
const ACCOUNTS_SQL: &str = "SELECT account_id FROM accounts ORDER BY account_id";

/// The CT02 amount of each add that takes a combined record's TRAN-ID, in the `COTRN02`
/// screen format.
const COLLIDING_AMOUNT: &str = "+00000001.00";

/// The CT02 amount of the add whose TRAN-ID the allocation gives.
const FRESH_AMOUNT: &str = "+00000002.00";

/// The reward balance and the full reward history of one account.
#[derive(Debug, PartialEq, Eq)]
struct AccountRewards {
    account: AccountId,
    balance: Option<RewardBalance>,
    history: Vec<RewardAccrual>,
}

/// An interleaved `REPRO` row of section D (COMBTRAN STEP10), parsed from its input condition
/// and expected result.
#[derive(Debug)]
struct ReproCase {
    id: &'static str,
    /// The 1-based combined record numbers whose TRAN-IDs the live master already holds, each
    /// with that TRAN-ID, in record order.
    held: Vec<(usize, String)>,
    /// `loaded=`: how many combined records the load writes.
    loaded: usize,
    /// The record list of `loaded=<n> (records ...)`, when the row gives one.
    loaded_records: Option<Vec<usize>>,
    /// `rejected=`: the duplicate records met.
    rejected: usize,
    /// `stopped_at=`: the record at which the error limit stops the load, if it does.
    stopped_at: Option<usize>,
    /// `not_attempted=`: the records after the stop, 0 when the load runs to the end.
    not_attempted: usize,
    /// `exit=`: the `REPRO` condition code, the exit code of `combtran`.
    exit: i32,
}

impl ReproCase {
    /// The interleaved row `id`, checked against `combined`, the combined file the row's
    /// record numbers count in.
    ///
    /// # Panics
    ///
    /// Panics, naming the row, if a cell does not parse, if a record number lies outside
    /// `combined` or names a record with another TRAN-ID, or if the row's counts contradict
    /// one another under the `REPRO` duplicate-key rule (P-COMBTRAN-003).
    fn from_audit(doc: &Document, id: &'static str, combined: &[String]) -> Self {
        let condition = parity_cell(doc, id, "Input condition");
        let (_, listed) = condition
            .split_once("combined records ")
            .unwrap_or_else(|| panic!("{id}: no held records in {condition:?}"));
        let held: Vec<(usize, String)> = listed
            .split(", ")
            .map(|pair| {
                let (number, tran_id) = pair
                    .split_once('=')
                    .unwrap_or_else(|| panic!("{id}: {pair:?} is not <record>=<TRAN-ID>"));
                (parsed(id, number), tran_id.trim().to_owned())
            })
            .collect();
        for (number, tran_id) in &held {
            let record = number
                .checked_sub(1)
                .and_then(|index| combined.get(index))
                .unwrap_or_else(|| panic!("{id}: record {number} is outside the combined file"));
            assert_eq!(key(record), tran_id, "{id}: combined record {number}");
        }
        let loaded = expected_value(doc, id, "loaded");
        let case = Self {
            id,
            held,
            loaded: leading_count(id, &loaded),
            loaded_records: loaded_records(id, &loaded),
            rejected: audited_count(doc, id, "rejected"),
            stopped_at: optional_value(doc, id, "stopped_at").map(|text| parsed(id, &text)),
            not_attempted: optional_value(doc, id, "not_attempted")
                .map_or(0, |text| parsed(id, &text)),
            exit: audited_exit(doc, id),
        };
        case.check_consistent(combined.len());
        case
    }

    /// The last combined record the load reaches: the stop record, or the last record.
    fn last_attempted(&self, records: usize) -> usize {
        self.stopped_at.unwrap_or(records)
    }

    /// The 1-based numbers of the combined records the load writes: every record it reaches
    /// whose TRAN-ID is not already live.
    fn loaded_numbers(&self, records: usize) -> Vec<usize> {
        let held: BTreeSet<usize> = self.held.iter().map(|(number, _)| *number).collect();
        (1..=self.last_attempted(records))
            .filter(|number| !held.contains(number))
            .collect()
    }

    /// P-COMBTRAN-003: the row's counts follow from its held records and its stop record.
    fn check_consistent(&self, records: usize) {
        let id = self.id;
        let last = self.last_attempted(records);
        let loaded = self.loaded_numbers(records);
        assert_eq!(loaded.len(), self.loaded, "{id}: loaded");
        if let Some(listed) = &self.loaded_records {
            assert_eq!(&loaded, listed, "{id}: loaded records");
        }
        let rejected = self
            .held
            .iter()
            .filter(|(number, _)| *number <= last)
            .count();
        assert_eq!(rejected, self.rejected, "{id}: rejected");
        assert_eq!(records - last, self.not_attempted, "{id}: not_attempted");
    }
}

/// `text` as a number of type `T`, or a panic naming the audit row `id`.
fn parsed<T: FromStr>(id: &str, text: &str) -> T
where
    T::Err: Display,
{
    text.trim()
        .parse()
        .unwrap_or_else(|error| panic!("{id}: {text:?} is not a number: {error}"))
}

/// The count that opens a value such as `296 (records 1, 3-4)`.
fn leading_count(id: &str, value: &str) -> usize {
    parsed(id, value.split_whitespace().next().unwrap_or_default())
}

/// The record list of a value such as `296 (records 1, 3-4, 6-8)`: single numbers and
/// inclusive ranges, `none` for no record. `None` when the value lists no records.
fn loaded_records(id: &str, value: &str) -> Option<Vec<usize>> {
    let (_, listed) = value.split_once("(records ")?;
    let items = listed
        .strip_suffix(')')
        .unwrap_or_else(|| panic!("{id}: unterminated record list in {value:?}"));
    if items.trim() == "none" {
        return Some(Vec::new());
    }
    let numbers = items
        .split(", ")
        .flat_map(|item| {
            let (low, high) = item.split_once('-').unwrap_or((item, item));
            parsed::<usize>(id, low)..=parsed(id, high)
        })
        .collect();
    Some(numbers)
}

/// The value of the `key=` pair of row `id`'s expected result, when it has one.
fn optional_value(doc: &Document, id: &str, key: &str) -> Option<String> {
    common::expected_values(doc, id)
        .into_iter()
        .find(|pair| pair.key == key)
        .map(|pair| pair.value)
}

/// The count that opens the `key=` value of row `id`, such as `262` for `posted=262`.
fn audited_count(doc: &Document, id: &str, key: &str) -> usize {
    leading_count(id, &expected_value(doc, id, key))
}

/// The `exit=` value of row `id`: the exit code of the job.
fn audited_exit(doc: &Document, id: &str) -> i32 {
    parsed(id, &expected_value(doc, id, "exit"))
}

/// The message of the code span of the `key=` pair of row `id`, without the double quotes
/// the audit writes around a COBOL literal.
fn audited_message(doc: &Document, id: &str, key: &str) -> String {
    let span = expected_span(doc, id, key);
    span.text
        .strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id}: {key}= holds no quoted literal: {:?}", span.text))
        .to_owned()
}

/// The TRAN-ID of a 350-byte `TRAN-RECORD`: its first 16 bytes.
fn key(record: &str) -> &str {
    &record[..KEY_LEN]
}

/// The text of the column `name` of a [`Harness::live_master_rows`] row.
fn field<'r>(row: &'r [Option<String>], name: &str) -> &'r str {
    let index = LIVE_MASTER_COLUMNS
        .iter()
        .position(|column| *column == name)
        .unwrap_or_else(|| panic!("the live master has no column {name}"));
    row.get(index)
        .and_then(Option::as_deref)
        .unwrap_or_else(|| panic!("live master column {name} is null in {row:?}"))
}

/// The column `name` of `row` read through its domain type's `FromStr`.
fn typed<T: FromStr>(row: &[Option<String>], name: &str) -> T
where
    T::Err: Display,
{
    parsed(name, field(row, name))
}

/// The `TRAN-RECORD` of a live master row, as the codec writes it: every `CVTRA05Y` field
/// moved from its column, FILLER spaces (E-25).
fn row_record(row: &[Option<String>]) -> TransactionRecord {
    let amount = Decimal::from_str(field(row, "tran_amt"))
        .ok()
        .and_then(|value| Money::from_decimal(value).ok())
        .unwrap_or_else(|| panic!("tran_amt is no S9(09)V99 amount in {row:?}"));
    TransactionRecord::new(NewTransaction {
        tran_id: TranId::move_from(field(row, "tran_id")),
        tran_type_cd: TranTypeCode::move_from(field(row, "tran_type_cd")),
        tran_cat_cd: typed::<TranCategoryCode>(row, "tran_cat_cd"),
        tran_source: Alnum::move_from(field(row, "tran_source")),
        tran_desc: Alnum::move_from(field(row, "tran_desc")),
        tran_amt: amount,
        tran_merchant_id: typed(row, "tran_merchant_id"),
        tran_merchant_name: Alnum::move_from(field(row, "tran_merchant_name")),
        tran_merchant_city: Alnum::move_from(field(row, "tran_merchant_city")),
        tran_merchant_zip: Alnum::move_from(field(row, "tran_merchant_zip")),
        tran_card_num: CardNumber::move_from(field(row, "tran_card_num")),
        tran_orig_ts: Alnum::move_from(field(row, "tran_orig_ts")),
        tran_proc_ts: Alnum::move_from(field(row, "tran_proc_ts")),
    })
}

/// Live master rows as the records of a `TRANSACT` generation file, without their LFs.
fn row_records(rows: &[Vec<Option<String>>]) -> Vec<String> {
    let records: Vec<TransactionRecord> = rows.iter().map(|row| row_record(row)).collect();
    split_records(&encode_ascii_lf::<TransactionLayout>(&records), RECORD_LEN)
}

/// The TRAN-IDs of `records`, for a readable first comparison of two record lists.
fn keys(records: &[String]) -> Vec<&str> {
    records.iter().map(|record| key(record)).collect()
}

/// The records of the `(0)` generation of `dataset`.
fn newest_records(h: &Harness, dataset: &str) -> Vec<String> {
    split_records(&h.newest(dataset), RECORD_LEN)
}

/// COMBTRAN STEP05R — P-COMBTRAN-001: `TRANSACT.BKUP(0)` then `SYSTRAN(0)`, the SORTIN
/// concatenation, sorted on `TRAN-ID,1,16,CH` ascending. The sort is stable, and the corpus
/// has no equal keys (P-COMBTRAN-004), so the undefined order of equal keys (E-38) never
/// matters here.
fn sorted_union(h: &Harness) -> Vec<String> {
    let mut records = newest_records(h, BKUP);
    records.extend(newest_records(h, SYSTRAN));
    records.sort_by(|left, right| key(left).cmp(key(right)));
    records
}

/// The `TRAN-CARD-NUM` of a combined record, through the `CVTRA05Y` codec.
fn card_of(record: &str) -> String {
    TransactionLayout::decode(record.as_bytes(), Charset::Ascii)
        .unwrap_or_else(|error| panic!("cannot decode {:?}: {error}", key(record)))
        .transaction
        .tran_card_num
        .trimmed()
        .into_owned()
}

/// The first column of every row of `sql`.
fn text_column(h: &Harness, sql: &str) -> Vec<String> {
    block_on(h.stack().fetch_text_rows(sql))
        .into_iter()
        .map(|row| {
            row.into_iter()
                .next()
                .flatten()
                .unwrap_or_else(|| panic!("a null first column from {sql}"))
        })
        .collect()
}

/// The reward balance and history of every account, read through the reward adapter the
/// rewards endpoints use, in one unit of work that is rolled back.
fn reward_snapshot(h: &Harness) -> Vec<AccountRewards> {
    let accounts: Vec<AccountId> = text_column(h, ACCOUNTS_SQL)
        .iter()
        .map(|text| parsed("account_id", text))
        .collect();
    let db = h.db();
    block_on(async {
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("cannot begin the reward read: {error}"));
        let mut snapshot = Vec::with_capacity(accounts.len());
        for account in accounts {
            let balance = uow
                .store()
                .reward_balance(account)
                .await
                .unwrap_or_else(|error| panic!("reward balance of {account}: {error}"));
            let history = uow
                .store()
                .reward_history(account, None, HISTORY_LIMIT)
                .await
                .unwrap_or_else(|error| panic!("reward history of {account}: {error}"));
            assert!(
                history.len() <= usize::from(HISTORY_LIMIT),
                "account {account} has more than {HISTORY_LIMIT} accruals"
            );
            snapshot.push(AccountRewards {
                account,
                balance,
                history,
            });
        }
        if let Err(error) = uow.rollback().await {
            panic!("cannot roll back the reward read: {error}");
        }
        snapshot
    })
}

/// The README cycle up to the backup: POSTTRAN over the fixture day, then INTCALC, both on
/// the TRANREPT clock. Returns the reward balances and histories POSTTRAN accrued.
fn cycle_to_backup(h: &Harness, doc: &Document) -> Vec<AccountRewards> {
    let _ = h.run_expect(
        &["posttran"],
        Some(TRANREPT_CLOCK),
        audited_exit(doc, POSTTRAN_COUNTS),
    );
    let _ = h.run_expect(&INTCALC, Some(TRANREPT_CLOCK), INTCALC_EXIT);
    let rewards = reward_snapshot(h);
    assert!(
        rewards.iter().any(|account| !account.history.is_empty()),
        "POSTTRAN accrued no reward points, so the reward checks would hold vacuously"
    );
    rewards
}

/// Runs TRANBKP over the live master of `h` and checks its condition code.
fn tranbkp(h: &Harness) {
    let _ = h.run_expect(&["tranbkp"], Some(TRANREPT_CLOCK), TRANBKP_EXIT);
}

/// The CT02 TRAN-ID allocation, in a unit of work that is then rolled back, so nothing is
/// added and the advisory lock is released.
fn allocate_and_roll_back(db: &Db) -> PortResult<TranId> {
    block_on(async {
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("CT02: cannot begin a unit of work: {error}"));
        let allocated = uow.next_tran_id().await.map_err(PortError::from);
        if let Err(error) = uow.rollback().await {
            panic!("CT02: rollback failed: {error}");
        }
        allocated
    })
}

/// Checks that `error` is the not-found outcome of an empty live master, with the COBOL
/// literal the audit row `id` gives under `key=`.
fn assert_not_found(doc: &Document, error: &DomainError, id: &str, key: &str) {
    assert_eq!(error.kind, ErrorKind::NotFound, "{id}: {error:?}");
    assert_eq!(error.message, TRANSACTION_ID_NOT_FOUND, "{id}");
    assert_eq!(error.message, audited_message(doc, id, key), "{id}");
}

/// P-COMBTRAN-004, with P-CBTRN02C-065 and P-CBACT04C-173: the SORTIN inputs and the
/// combined file the audit derives from the corpus.
fn assert_combined_corpus(doc: &Document, h: &Harness, combined: &[String]) {
    assert_eq!(
        newest_records(h, BKUP).len(),
        audited_count(doc, POSTTRAN_COUNTS, "posted"),
        "TRANSACT.BKUP(0) holds the posted records"
    );
    assert_eq!(
        newest_records(h, SYSTRAN).len(),
        audited_count(doc, SYSTRAN_RECORDS, "records"),
        "SYSTRAN(0) holds the interest records"
    );
    assert_eq!(
        combined.len(),
        audited_count(doc, COMBINED_CORPUS, "records")
    );
    let first = combined.first().map(|record| key(record));
    let last = combined.last().map(|record| key(record));
    assert_eq!(
        first,
        Some(expected_value(doc, COMBINED_CORPUS, "first").as_str())
    );
    assert_eq!(
        last,
        Some(expected_value(doc, COMBINED_CORPUS, "last").as_str())
    );
    assert!(
        combined
            .windows(2)
            .all(|pair| key(&pair[0]) < key(&pair[1])),
        "equal_keys=none: the combined TRAN-IDs are strictly ascending"
    );
}

/// The live rows whose TRAN-IDs are in `tran_ids`, in TRAN-ID order, with every column.
fn live_rows_of(h: &Harness, tran_ids: &BTreeSet<String>) -> Vec<Vec<Option<String>>> {
    h.live_master_rows()
        .into_iter()
        .filter(|row| tran_ids.contains(field(row, "tran_id")))
        .collect()
}

/// CT02 adds that take the TRAN-IDs of the combined records `case` holds, each with the
/// record's card and an amount and description that differ from the record. Returns the
/// TRAN-IDs written.
///
/// The add names its TRAN-ID instead of allocating one: right after TRANBKP the allocation
/// meets an empty live master and adds nothing (P-COTRN02C-021, E-27), and an allocated
/// TRAN-ID is the highest live key plus one, so it cannot be steered onto the combined
/// records the audit row names. These adds are the rows the master holds before any
/// allocation. `ReproCase::from_audit` has checked every record number against `combined`.
fn add_colliding_rows(db: &Db, case: &ReproCase, combined: &[String]) -> BTreeSet<String> {
    case.held
        .iter()
        .map(|(number, tran_id)| {
            let description = format!("Interleaved CT02 add on combined record {number}");
            let input = ct02_input(
                &card_of(&combined[number - 1]),
                COLLIDING_AMOUNT,
                &description,
            );
            let written = ct02_add(db, input, Some(TranId::move_from(tran_id)), TRANREPT_CLOCK);
            assert_eq!(written.trimmed(), tran_id.as_str(), "{}", case.id);
            written.trimmed().into_owned()
        })
        .collect()
}

/// The CT02 add whose TRAN-ID the allocation gives — P-COTRN02C-020: the highest live key
/// plus one, the live master holding only the `held` rows at that point. Returns its
/// TRAN-ID, which no combined record has.
fn add_fresh_row(db: &Db, held: &BTreeSet<String>, combined: &[String]) -> String {
    let highest = held
        .last()
        .unwrap_or_else(|| panic!("the fresh add needs a live row before it (E-27)"));
    let expected = TranId::move_from(highest)
        .next()
        .unwrap_or_else(|error| panic!("{highest} has no successor: {error}"));
    let input = ct02_input(
        &card_of(&combined[0]),
        FRESH_AMOUNT,
        "Fresh CT02 add after TRANBKP",
    );
    let written = ct02_add(db, input, None, TRANREPT_CLOCK);
    assert_eq!(
        written, expected,
        "P-COTRN02C-020: the highest live key plus one"
    );
    let fresh = written.trimmed().into_owned();
    assert!(
        combined.iter().all(|record| key(record) != fresh),
        "the fresh TRAN-ID {fresh} is in TRANSACT.BKUP(0) or SYSTRAN(0)"
    );
    fresh
}

/// COMBTRAN STEP10 — P-COMBTRAN-003 and the row of `case`: every CT02 row stays live with
/// the content `ct02_before` recorded, no combined record replaced one, and the other live
/// rows are exactly the combined records the row says the load wrote, so the duplicates and
/// every record after the stop record stay out.
fn assert_interleaved_load(
    h: &Harness,
    case: &ReproCase,
    combined: &[String],
    ct02_before: &[Vec<Option<String>>],
) {
    let id = case.id;
    let ct02_ids: BTreeSet<&str> = ct02_before
        .iter()
        .map(|row| field(row, "tran_id"))
        .collect();
    let (ct02_after, loaded): (Vec<_>, Vec<_>) = h
        .live_master_rows()
        .into_iter()
        .partition(|row| ct02_ids.contains(field(row, "tran_id")));
    assert_eq!(
        ct02_after, ct02_before,
        "{id}: every CT02 row stays live with its content"
    );
    let ct02_records = row_records(&ct02_after);
    for (number, tran_id) in &case.held {
        let kept = ct02_records
            .iter()
            .find(|record| key(record) == tran_id)
            .unwrap_or_else(|| panic!("{id}: no live CT02 row holds {tran_id}"));
        assert_ne!(
            kept,
            &combined[number - 1],
            "{id}: combined record {number} must differ from the CT02 row it collides with"
        );
    }
    let expected: Vec<String> = case
        .loaded_numbers(combined.len())
        .into_iter()
        .map(|number| combined[number - 1].clone())
        .collect();
    let loaded = row_records(&loaded);
    assert_eq!(keys(&loaded), keys(&expected), "{id}: the TRAN-IDs loaded");
    assert_eq!(loaded, expected, "{id}: the records loaded");
}

/// One interleaved case on a fresh database: the cycle to the backup, TRANBKP, the CT02 rows
/// that take the TRAN-IDs of the combined records the audit row `id` names, with a further
/// CT02 add through the allocation when `fresh_add` is set, then COMBTRAN with the row's
/// condition code.
fn run_interleaved(doc: &Document, id: &'static str, fresh_add: bool) {
    let h = Harness::new();
    let _ = cycle_to_backup(&h, doc);
    tranbkp(&h);
    let combined = sorted_union(&h);
    assert_combined_corpus(doc, &h, &combined);
    let case = ReproCase::from_audit(doc, id, &combined);
    let db = h.db();

    // The colliding rows go first: an allocation on the master TRANBKP emptied adds nothing
    // (E-27), so the fresh add allocates once they are live.
    let mut ct02_ids = add_colliding_rows(&db, &case, &combined);
    if fresh_add {
        let fresh = add_fresh_row(&db, &ct02_ids, &combined);
        ct02_ids.insert(fresh);
    }
    let ct02_before = live_rows_of(&h, &ct02_ids);
    assert_eq!(
        ct02_before.len(),
        ct02_ids.len(),
        "{id}: every CT02 add is live"
    );

    let _ = h.run_expect(&["combtran"], Some(TRANREPT_CLOCK), case.exit);

    assert_eq!(
        h.generations(COMBINED).len(),
        1,
        "{id}: TRANSACT.COMBINED(+1)"
    );
    assert_eq!(
        newest_records(&h, COMBINED),
        combined,
        "{id}: P-COMBTRAN-001"
    );
    assert_interleaved_load(&h, &case, &combined, &ct02_before);
}

#[test]
fn every_cited_audit_row_exists() {
    let doc = parity();
    for id in P_IDS {
        let _ = parity_row(&doc, id);
    }
}

#[test]
fn tranbkp_empties_live_master_and_keeps_rewards() {
    let doc = parity();
    let h = Harness::new();
    let rewards = cycle_to_backup(&h, &doc);
    let live = h.live_master_rows();
    assert_eq!(
        live.len(),
        audited_count(&doc, POSTTRAN_COUNTS, "posted"),
        "the live master holds the posted records"
    );
    assert!(h.generations(BKUP).is_empty(), "no backup before TRANBKP");

    tranbkp(&h);

    // P-TRANBKP-001: one new generation, every live record in TRAN-ID order.
    assert_eq!(h.generations(BKUP).len(), 1, "TRANSACT.BKUP(+1)");
    let backup = newest_records(&h, BKUP);
    let expected = row_records(&live);
    assert_eq!(keys(&backup), keys(&expected), "the TRAN-IDs backed up");
    assert_eq!(backup, expected, "the records backed up");

    // P-TRANBKP-003, P-TRANBKP-004: the deleted and redefined cluster is an empty master.
    assert!(h.live_master_rows().is_empty(), "the live master is empty");
    let db = h.db();

    // P-COTRN00C-011: CT00 answers the empty page with the STARTBR NOTFND text.
    let page = ct00_first_page(&db);
    assert!(page.items.is_empty(), "CT00 lists {:?}", page.items);
    assert_eq!((page.next_key, page.prev_key), (None, None));
    assert_eq!(page.message, Some(YOU_ARE_AT_THE_TOP_OF_THE_PAGE));
    assert_eq!(
        YOU_ARE_AT_THE_TOP_OF_THE_PAGE,
        audited_message(&doc, CT00_EMPTY_FILE, "message")
    );

    // P-COTRN01C-004: a backed-up TRAN-ID is not found.
    let backed_up = key(&backup[0]);
    let viewed = ct01_view(&db, backed_up);
    let Err(not_found) = viewed else {
        panic!("CT01 shows {backed_up} after TRANBKP: {viewed:?}");
    };
    assert_not_found(&doc, &not_found, CT01_NOT_FOUND, "error");

    // P-COTRN02C-021: the CT02 allocation meets the empty master and adds nothing (E-27).
    let allocated = allocate_and_roll_back(&db);
    let Err(empty_file) = transaction_add::allocated_tran_id(allocated) else {
        panic!("CT02 allocates a TRAN-ID on the empty live master");
    };
    assert_not_found(&doc, &empty_file, CT02_EMPTY_FILE, "error");
    assert!(
        h.live_master_rows().is_empty(),
        "the allocation added nothing"
    );

    // P-TRANBKP-005: the accrued rows stay as the targets of unchanged accruals.
    assert_eq!(
        reward_snapshot(&h),
        rewards,
        "reward balances and histories"
    );
}

#[test]
fn combtran_from_empty_master_restores_combined_file() {
    let doc = parity();
    let h = Harness::new();
    let rewards = cycle_to_backup(&h, &doc);
    tranbkp(&h);
    let expected = sorted_union(&h);
    assert_combined_corpus(&doc, &h, &expected);

    let _ = h.run_expect(
        &["combtran"],
        Some(TRANREPT_CLOCK),
        audited_exit(&doc, README_LOAD),
    );

    // P-COMBTRAN-001, P-COMBTRAN-004: the sorted SORTIN concatenation.
    assert_eq!(h.generations(COMBINED).len(), 1, "TRANSACT.COMBINED(+1)");
    let combined = newest_records(&h, COMBINED);
    assert_eq!(keys(&combined), keys(&expected), "the combined TRAN-IDs");
    assert_eq!(combined, expected, "the combined records");

    // P-COMBTRAN-002, P-COMBTRAN-005: every record loaded, none rejected, the live master
    // the combined file record for record.
    let live = h.live_master_rows();
    assert_eq!(live.len(), audited_count(&doc, README_LOAD, "loaded"));
    assert_eq!(audited_count(&doc, README_LOAD, "rejected"), 0);
    let reloaded = row_records(&live);
    assert_eq!(keys(&reloaded), keys(&combined), "the TRAN-IDs reloaded");
    assert_eq!(reloaded, combined, "the live master is the combined file");

    // E-09: the revived rows keep their accruals, which point at live rows again.
    let live_rows: BTreeSet<&str> = live
        .iter()
        .map(|row| field(row, "transaction_id"))
        .collect();
    for accrual in rewards.iter().flat_map(|account| &account.history) {
        let row = accrual.transaction_id.to_string();
        assert!(
            live_rows.contains(row.as_str()),
            "accrual {} references transaction row {row}, which is not live",
            accrual.accrual_id
        );
    }
    assert_eq!(
        reward_snapshot(&h),
        rewards,
        "reward balances and histories"
    );
}

#[test]
fn interleaved_ct02_rows_survive_combtran() {
    // P-COMBTRAN-008: CT02 rows on two backed-up TRAN-IDs and one fresh TRAN-ID; REPRO
    // rejects the two duplicates, loads every other record and ends with condition code 8.
    run_interleaved(&parity(), INTERLEAVED_LOAD, true);
}

#[test]
fn repro_stops_at_its_error_limit() {
    // P-COMBTRAN-006, P-COMBTRAN-007: the fourth duplicate stops REPRO with condition code
    // 12; the records after it are never attempted.
    let doc = parity();
    for id in ERROR_LIMIT_LOADS {
        run_interleaved(&doc, id, false);
    }
}
