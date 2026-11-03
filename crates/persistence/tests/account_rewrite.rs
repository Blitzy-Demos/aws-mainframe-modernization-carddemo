//! The account `REWRITE` against a migrated, seeded `PostgreSQL` 16.15: the
//! `AccountWriter::rewrite_account` adapter of `PgStore`
//! (`crates/persistence/src/repos/accounts.rs`) stores each of the eleven
//! non-key `CVACT01Y` fields in its own column of the one row its key names,
//! and in no other row.
//!
//! The writer serves every account `REWRITE` of the carried programs:
//! `COACTUPC` `9600-WRITE-PROCESSING` [app/cbl/COACTUPC.cbl:L4060-4081],
//! `COBIL00C` `UPDATE-ACCTDAT-FILE` [app/cbl/COBIL00C.cbl:L377-403],
//! `CBTRN02C` `2800-UPDATE-ACCOUNT-REC` [app/cbl/CBTRN02C.cbl:L545-560] and
//! `CBACT04C` `1050-UPDATE-ACCOUNT` [app/cbl/CBACT04C.cbl:L350-370] —
//! P-COACTUPC-013, P-COACTUPC-014, P-COBIL00C-018, P-COBIL00C-019,
//! P-CBTRN02C-018, P-CBACT04C-010.
//!
//! The cases run in order on one seeded database. Each reads account 1 for
//! update in one unit of work, rewrites a record, then commits or rolls back:
//!
//! 1. a record whose eleven non-key fields all differ from the fixture row and
//!    from one another, committed: every column must hold its own field's
//!    text, so a bind given to another column, a column left out of the `SET`
//!    list, or an `UPDATE` that changes nothing fails;
//! 2. the fixture record, rolled back: the row keeps the first case's text;
//! 3. a record whose key names no row, committed: `PortError::NotFound`, the
//!    `REWRITE` `NOTFND`, and no row changes;
//! 4. the first case's record with a blank `ACCT-GROUP-ID`, committed: the
//!    column holds `''`, the right-trimmed form its column check admits
//!    (E-06).
//!
//! Every case reads the whole table in its `PostgreSQL` text form before and
//! after, and requires every row but account 1's to be unchanged byte for
//! byte, so a rewrite that reaches past its key fails too. The record
//! `AccountReader::read_account` returns, inside the unit of work and after it
//! ends, must be the one the case leaves stored.
//!
//! The test starts its own container through `carddemo_testkit::TestStack`, so
//! it needs a running Docker daemon (AAP 0.10.4), and loads it with
//! `seed::prepare` and `seed::run` in `SeedMode::Full`, the preparation on the
//! test thread before the async phase. The table is read through one
//! `sqlx::query!` that casts every column to `text`, so the snapshot's SQL is
//! checked against the migrated schema at compile time.

use carddemo_domain::entities::Account;
use carddemo_domain::numeric::{AccountGroupId, AccountId, Alnum, Money};
use carddemo_domain::ports::{AccountReader, AccountWriter, PortError, PortResult};
use carddemo_persistence::Db;
use carddemo_persistence::seed::{self, SeedMode, SeedSources};
use carddemo_testkit::{TestStack, block_on, fixtures};

/// The account every case reads for update, and every case but the third
/// rewrites.
const ACCOUNT: u64 = 1;

/// The highest `ACCT-ID PIC 9(11)`, which no fixture account has
/// [app/data/ASCII/acctdata.txt].
const ABSENT_ACCOUNT: u64 = 99_999_999_999;

/// The fixture accounts `batch seed` stores [app/data/ASCII/acctdata.txt].
const FIXTURE_ACCOUNTS: usize = 50;

/// One row of [`stored_accounts`], each column in its `PostgreSQL` text form.
type Row = [&'static str; 12];

/// Account 1 as `batch seed` stores the first `acctdata.txt` record
/// [app/data/ASCII/acctdata.txt:L1]: text right-trimmed, money as
/// `NUMERIC(12,2)`, the blank group ID as `''` (E-06).
const FIXTURE_ROW: Row = [
    "1",
    "Y",
    "194.00",
    "2020.00",
    "1020.00",
    "2014-11-20",
    "2025-05-20",
    "2025-05-20",
    "0.00",
    "0.00",
    "A000000000",
    "",
];

/// Account 1 once [`changed_account`] is rewritten: the `X(10)` ZIP code
/// stored without its trailing space, and the five `S9(10)V99` amounts at both
/// signs, at the picture's largest magnitude, its smallest and between.
const CHANGED_ROW: Row = [
    "1",
    "N",
    "-1234567890.12",
    "9999999999.99",
    "0.01",
    "2001-02-03",
    "2031-12-30",
    "2029-07-14",
    "345.60",
    "-78.09",
    "Z98765432",
    "GRP-R1",
];

/// Account 1 once [`blank_group_account`] is rewritten: [`CHANGED_ROW`] with
/// the blank group ID stored as `''`.
const BLANK_GROUP_ROW: Row = [
    "1",
    "N",
    "-1234567890.12",
    "9999999999.99",
    "0.01",
    "2001-02-03",
    "2031-12-30",
    "2029-07-14",
    "345.60",
    "-78.09",
    "Z98765432",
    "",
];

/// How a case ends its unit of work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Commit,
    Rollback,
}

/// One rewrite: the record written, how its unit of work ends, the writer's
/// answer, and account 1's row before the case and after it.
struct Case {
    name: &'static str,
    record: fn() -> Account,
    end: End,
    outcome: PortResult<()>,
    before: Row,
    after: Row,
}

/// Every case, in the order they run: each starts from the row the one before
/// it leaves.
const CASES: [Case; 4] = [
    Case {
        name: "every non-key field changed",
        record: changed_account,
        end: End::Commit,
        outcome: Ok(()),
        before: FIXTURE_ROW,
        after: CHANGED_ROW,
    },
    Case {
        name: "a rewrite rolled back",
        record: fixture_account,
        end: End::Rollback,
        outcome: Ok(()),
        before: CHANGED_ROW,
        after: CHANGED_ROW,
    },
    Case {
        name: "a key that names no row",
        record: absent_account,
        end: End::Commit,
        outcome: Err(PortError::NotFound),
        before: CHANGED_ROW,
        after: CHANGED_ROW,
    },
    Case {
        name: "a blank group ID",
        record: blank_group_account,
        end: End::Commit,
        outcome: Ok(()),
        before: CHANGED_ROW,
        after: BLANK_GROUP_ROW,
    },
];

fn account(value: u64) -> AccountId {
    AccountId::new(value).expect("a 9(11) account number")
}

fn money(cents: i64) -> Money<10> {
    Money::from_cents(cents).expect("an S9(10)V99 amount")
}

/// The first `acctdata.txt` record as the codec decodes it, the record
/// [`FIXTURE_ROW`] stores.
fn fixture_account() -> Account {
    Account {
        acct_id: account(ACCOUNT),
        acct_active_status: Alnum::move_from("Y"),
        acct_curr_bal: money(19_400),
        acct_credit_limit: money(202_000),
        acct_cash_credit_limit: money(102_000),
        acct_open_date: Alnum::move_from("2014-11-20"),
        acct_expiraion_date: Alnum::move_from("2025-05-20"),
        acct_reissue_date: Alnum::move_from("2025-05-20"),
        acct_curr_cyc_credit: Money::ZERO,
        acct_curr_cyc_debit: Money::ZERO,
        acct_addr_zip: Alnum::move_from("A000000000"),
        acct_group_id: AccountGroupId::move_from(""),
    }
}

/// Account 1 with each of its eleven non-key fields changed, the record
/// [`CHANGED_ROW`] stores.
fn changed_account() -> Account {
    Account {
        acct_id: account(ACCOUNT),
        acct_active_status: Alnum::move_from("N"),
        acct_curr_bal: money(-123_456_789_012),
        acct_credit_limit: money(999_999_999_999),
        acct_cash_credit_limit: money(1),
        acct_open_date: Alnum::move_from("2001-02-03"),
        acct_expiraion_date: Alnum::move_from("2031-12-30"),
        acct_reissue_date: Alnum::move_from("2029-07-14"),
        acct_curr_cyc_credit: money(34_560),
        acct_curr_cyc_debit: money(-7_809),
        acct_addr_zip: Alnum::move_from("Z98765432"),
        acct_group_id: AccountGroupId::move_from("GRP-R1"),
    }
}

/// The fixture record under [`ABSENT_ACCOUNT`]'s key, so a rewrite that
/// reached account 1 instead would put the fixture text back there.
fn absent_account() -> Account {
    Account {
        acct_id: account(ABSENT_ACCOUNT),
        ..fixture_account()
    }
}

/// [`changed_account`] with the blank `ACCT-GROUP-ID` every fixture account
/// holds: ten spaces.
fn blank_group_account() -> Account {
    Account {
        acct_group_id: AccountGroupId::move_from(""),
        ..changed_account()
    }
}

/// `row` as [`stored_accounts`] returns it.
fn text_row(row: &Row) -> Vec<Option<String>> {
    row.iter().map(|text| Some((*text).to_owned())).collect()
}

/// Every `accounts` row in key order, so account 1's row comes first: its
/// twelve columns, the key first, each cast to its `PostgreSQL` text form, so
/// a `NUMERIC(12,2)` amount keeps its two decimals and a null reads as `None`.
/// The order names the table's key column, because the bare name would sort
/// by the `text` alias it shares.
async fn stored_accounts(db: &Db) -> Vec<Vec<Option<String>>> {
    sqlx::query!(
        "SELECT account_id::text AS account_id, \
                acct_active_status::text AS acct_active_status, \
                acct_curr_bal::text AS acct_curr_bal, \
                acct_credit_limit::text AS acct_credit_limit, \
                acct_cash_credit_limit::text AS acct_cash_credit_limit, \
                acct_open_date::text AS acct_open_date, \
                acct_expiraion_date::text AS acct_expiraion_date, \
                acct_reissue_date::text AS acct_reissue_date, \
                acct_curr_cyc_credit::text AS acct_curr_cyc_credit, \
                acct_curr_cyc_debit::text AS acct_curr_cyc_debit, \
                acct_addr_zip::text AS acct_addr_zip, \
                acct_group_id::text AS acct_group_id \
         FROM accounts ORDER BY accounts.account_id"
    )
    .fetch_all(db.pool())
    .await
    .expect("the account rows read")
    .into_iter()
    .map(|row| {
        vec![
            row.account_id,
            row.acct_active_status,
            row.acct_curr_bal,
            row.acct_credit_limit,
            row.acct_cash_credit_limit,
            row.acct_open_date,
            row.acct_expiraion_date,
            row.acct_reissue_date,
            row.acct_curr_cyc_credit,
            row.acct_curr_cyc_debit,
            row.acct_addr_zip,
            row.acct_group_id,
        ]
    })
    .collect()
}

/// Holds the first case to values that tell every column apart: each non-key
/// text differs from the fixture's in its column, so a column the rewrite
/// leaves out keeps a different value, and from every other non-key text, so
/// a bind given to another column shows there.
fn assert_each_column_tells(changed: &Row, fixture: &Row) {
    for (column, text) in changed.iter().enumerate().skip(1) {
        assert_ne!(
            *text, fixture[column],
            "column {column} keeps its fixture text"
        );
        assert!(
            !changed[column + 1..].contains(text),
            "column {column} repeats a later column's text"
        );
    }
}

/// Runs one case: account 1's row before it, the locking read, the rewrite,
/// the end of the unit of work, then every stored row and account 1's record
/// read back.
async fn run_case(db: &Db, case: &Case) {
    let name = case.name;
    let before = stored_accounts(db).await;
    assert_eq!(
        before.len(),
        FIXTURE_ACCOUNTS,
        "{name}: the stored accounts"
    );
    assert_eq!(
        before.first(),
        Some(&text_row(&case.before)),
        "{name}: account 1 before the rewrite"
    );
    let mut uow = db.begin().await.expect("a unit of work");
    let stored = uow
        .store()
        .read_account_for_update(account(ACCOUNT))
        .await
        .unwrap_or_else(|error| panic!("{name}: account 1 does not read for update: {error}"));
    let record = (case.record)();
    let outcome = uow.store().rewrite_account(record.clone()).await;
    assert_eq!(outcome, case.outcome, "{name}: the rewrite's answer");
    let written = if outcome.is_ok() {
        record
    } else {
        stored.clone()
    };
    assert_eq!(
        uow.store().read_account(account(ACCOUNT)).await,
        Ok(written.clone()),
        "{name}: account 1 inside the unit of work"
    );
    let kept = match case.end {
        End::Commit => {
            uow.commit().await.expect("the unit of work commits");
            written
        }
        End::Rollback => {
            uow.rollback().await.expect("the unit of work rolls back");
            stored
        }
    };
    let mut expected = before;
    expected[0] = text_row(&case.after);
    assert_eq!(
        stored_accounts(db).await,
        expected,
        "{name}: after the {:?}, account 1 holds the case's text and every other row its own",
        case.end
    );
    let mut reader = db.begin().await.expect("a unit of work");
    assert_eq!(
        reader.store().read_account(account(ACCOUNT)).await,
        Ok(kept),
        "{name}: account 1 read after the unit of work"
    );
    reader
        .rollback()
        .await
        .expect("the reading unit of work rolls back");
}

#[test]
fn a_rewrite_replaces_the_eleven_columns_of_its_own_row_alone_once_committed() {
    assert_each_column_tells(&CHANGED_ROW, &FIXTURE_ROW);
    // The seed is prepared on the test thread, outside every runtime, as an adapter prepares it
    // on its blocking pool, so the argon2id hashing never runs inside the async phase.
    let prepared = seed::prepare(
        SeedMode::Full,
        &SeedSources::with_defaults(&fixtures::repo_root()),
    );
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        let report = seed::run(&db, prepared)
            .await
            .expect("the fixture seed loads");
        assert_eq!(report.accounts, FIXTURE_ACCOUNTS, "the seeded accounts");
        for case in &CASES {
            run_case(&db, case).await;
        }
    });
}
