//! Statement isolation of the store against a migrated, seeded `PostgreSQL`
//! 16.15: a read that fails on the server leaves the unit of work usable, and
//! the write made before it commits (`crate::tx`, AAP 0.6.4).
//!
//! A server-side error aborts the whole `PostgreSQL` transaction: every later
//! statement fails with `25P02`, and `COMMIT` ends as `ROLLBACK`. Every store
//! statement therefore runs under its own savepoint, reads included. Each case
//! here hides the table or view one read names, by a test-only rename, then
//! in one unit of work:
//!
//! 1. rewrites a row of another table, a legacy write that must persist
//!    (`RECOVERY(NONE)`, P-CBACT04C-010, P-CBPAUP0C-005);
//! 2. runs the read, which must fail with `PortError::Unavailable` naming the
//!    SQLSTATE `42P01` (undefined table);
//! 3. reads the written row back, a statement that must still succeed rather
//!    than fail with `25P02`, as `COACTVWC` still reads `CUSTDAT` after a
//!    failed `ACCTDAT` read (V-COACTVWC-006);
//! 4. commits, after which the write must be visible.
//!
//! The rename is undone before the next case. The test starts its own
//! container through `carddemo_testkit::TestStack`, so it needs a running
//! Docker daemon (AAP 0.10.4). Hiding and restoring a table, and reading the
//! committed row, go through the stack's text-typed SQL helpers, so the test
//! adds no compile-time query.

use carddemo_domain::entities::{DisclosureGroupKey, TranCatBalanceKey};
use carddemo_domain::numeric::{
    AccountGroupId, AccountId, Alnum, AuthId, CardNumber, CustomerId, TranCategoryCode, TranId,
    TranTypeCode, UserId,
};
use carddemo_domain::ports::{
    AccountReader, AccountWriter, AuthorizationStore, Browse, CardReader, CustomerReader,
    CustomerWriter, Direction, DisclosureGroupReader, PortError, PortResult, TranCatBalanceStore,
    TranTypeFileReader, TranTypeMasterStore, TransactionReader, UserStore, XrefReader,
};
use carddemo_domain::rewards::RewardStore;
use carddemo_persistence::seed::{self, SeedMode, SeedSources};
use carddemo_persistence::{Db, UnitOfWork};
use carddemo_testkit::{TestStack, block_on, fixtures};

/// The card the cross-reference maps to account 1 and customer 1
/// [app/data/ASCII/cardxref.txt:L49].
const ACCOUNT_1_CARD: &str = "9680294154603697";

/// The SQLSTATE of a statement naming a table or view that does not exist.
const UNDEFINED_TABLE: &str = "42P01";

/// The store read a case runs while its table is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    Account,
    Customer,
    XrefByAccount,
    XrefByCard,
    XrefsAfter,
    Card,
    CardsForAccount,
    DisclosureGroup,
    Tcatbal,
    TcatbalsAfter,
    SummariesAfter,
    Summary,
    DetailsForAccount,
    Detail,
    User,
    BrowseUsers,
    TranType,
    TranTypeFile,
    Transaction,
    BrowseTransactions,
    HighestTransaction,
    CandidateRules,
    RewardBalance,
    RewardHistory,
}

/// One case: the read, and the literal statements that hide and restore the
/// one table or view it names.
struct Case {
    read: Read,
    hide: &'static str,
    restore: &'static str,
}

/// Every case, one read each.
const CASES: [Case; 24] = [
    Case {
        read: Read::Account,
        hide: "ALTER TABLE accounts RENAME TO accounts_hidden",
        restore: "ALTER TABLE accounts_hidden RENAME TO accounts",
    },
    Case {
        read: Read::Customer,
        hide: "ALTER TABLE customers RENAME TO customers_hidden",
        restore: "ALTER TABLE customers_hidden RENAME TO customers",
    },
    Case {
        read: Read::XrefByAccount,
        hide: "ALTER TABLE card_xref RENAME TO card_xref_hidden",
        restore: "ALTER TABLE card_xref_hidden RENAME TO card_xref",
    },
    Case {
        read: Read::XrefByCard,
        hide: "ALTER TABLE card_xref RENAME TO card_xref_hidden",
        restore: "ALTER TABLE card_xref_hidden RENAME TO card_xref",
    },
    Case {
        read: Read::XrefsAfter,
        hide: "ALTER TABLE card_xref RENAME TO card_xref_hidden",
        restore: "ALTER TABLE card_xref_hidden RENAME TO card_xref",
    },
    Case {
        read: Read::Card,
        hide: "ALTER TABLE cards RENAME TO cards_hidden",
        restore: "ALTER TABLE cards_hidden RENAME TO cards",
    },
    Case {
        read: Read::CardsForAccount,
        hide: "ALTER TABLE cards RENAME TO cards_hidden",
        restore: "ALTER TABLE cards_hidden RENAME TO cards",
    },
    Case {
        read: Read::DisclosureGroup,
        hide: "ALTER TABLE disclosure_groups RENAME TO disclosure_groups_hidden",
        restore: "ALTER TABLE disclosure_groups_hidden RENAME TO disclosure_groups",
    },
    Case {
        read: Read::Tcatbal,
        hide: "ALTER TABLE transaction_category_balances RENAME TO tcatbal_hidden",
        restore: "ALTER TABLE tcatbal_hidden RENAME TO transaction_category_balances",
    },
    Case {
        read: Read::TcatbalsAfter,
        hide: "ALTER TABLE transaction_category_balances RENAME TO tcatbal_hidden",
        restore: "ALTER TABLE tcatbal_hidden RENAME TO transaction_category_balances",
    },
    Case {
        read: Read::SummariesAfter,
        hide: "ALTER TABLE pending_auth_summaries RENAME TO summaries_hidden",
        restore: "ALTER TABLE summaries_hidden RENAME TO pending_auth_summaries",
    },
    Case {
        read: Read::Summary,
        hide: "ALTER TABLE pending_auth_summaries RENAME TO summaries_hidden",
        restore: "ALTER TABLE summaries_hidden RENAME TO pending_auth_summaries",
    },
    Case {
        read: Read::DetailsForAccount,
        hide: "ALTER TABLE pending_auth_details RENAME TO details_hidden",
        restore: "ALTER TABLE details_hidden RENAME TO pending_auth_details",
    },
    Case {
        read: Read::Detail,
        hide: "ALTER TABLE pending_auth_details RENAME TO details_hidden",
        restore: "ALTER TABLE details_hidden RENAME TO pending_auth_details",
    },
    Case {
        read: Read::User,
        hide: "ALTER TABLE users RENAME TO users_hidden",
        restore: "ALTER TABLE users_hidden RENAME TO users",
    },
    Case {
        read: Read::BrowseUsers,
        hide: "ALTER TABLE users RENAME TO users_hidden",
        restore: "ALTER TABLE users_hidden RENAME TO users",
    },
    Case {
        read: Read::TranType,
        hide: "ALTER TABLE transaction_types RENAME TO transaction_types_hidden",
        restore: "ALTER TABLE transaction_types_hidden RENAME TO transaction_types",
    },
    Case {
        read: Read::TranTypeFile,
        hide: "ALTER TABLE trantype_file RENAME TO trantype_file_hidden",
        restore: "ALTER TABLE trantype_file_hidden RENAME TO trantype_file",
    },
    Case {
        read: Read::Transaction,
        hide: "ALTER VIEW transaction_master RENAME TO transaction_master_hidden",
        restore: "ALTER VIEW transaction_master_hidden RENAME TO transaction_master",
    },
    Case {
        read: Read::BrowseTransactions,
        hide: "ALTER VIEW transaction_master RENAME TO transaction_master_hidden",
        restore: "ALTER VIEW transaction_master_hidden RENAME TO transaction_master",
    },
    Case {
        read: Read::HighestTransaction,
        hide: "ALTER VIEW transaction_master RENAME TO transaction_master_hidden",
        restore: "ALTER VIEW transaction_master_hidden RENAME TO transaction_master",
    },
    Case {
        read: Read::CandidateRules,
        hide: "ALTER TABLE reward_rules RENAME TO reward_rules_hidden",
        restore: "ALTER TABLE reward_rules_hidden RENAME TO reward_rules",
    },
    Case {
        read: Read::RewardBalance,
        hide: "ALTER TABLE reward_balances RENAME TO reward_balances_hidden",
        restore: "ALTER TABLE reward_balances_hidden RENAME TO reward_balances",
    },
    Case {
        read: Read::RewardHistory,
        hide: "ALTER TABLE reward_accruals RENAME TO reward_accruals_hidden",
        restore: "ALTER TABLE reward_accruals_hidden RENAME TO reward_accruals",
    },
];

fn account(value: u64) -> AccountId {
    AccountId::new(value).expect("a 9(11) account number")
}

fn customer(value: u32) -> CustomerId {
    CustomerId::new(value).expect("a 9(09) customer ID")
}

fn category(value: u16) -> TranCategoryCode {
    TranCategoryCode::new(value).expect("a 9(04) category code")
}

fn card() -> CardNumber {
    CardNumber::from_trimmed(ACCOUNT_1_CARD).expect("a 16-digit card number")
}

/// A forward browse from the start of the file, ten rows at most.
fn first_page<K>() -> Browse<K> {
    Browse {
        start_key: None,
        direction: Direction::Forward,
        limit: 10,
    }
}

/// Runs `read` in `uow`, its result reduced to whether it succeeded.
async fn run_read(uow: &mut UnitOfWork, read: Read) -> PortResult<()> {
    let mut store = uow.store();
    match read {
        Read::Account => store.read_account(account(1)).await.map(|_| ()),
        Read::Customer => store.read_customer(customer(1)).await.map(|_| ()),
        Read::XrefByAccount => store.read_xref_by_account(account(1)).await.map(|_| ()),
        Read::XrefByCard => store.read_xref_by_card(card()).await.map(|_| ()),
        Read::XrefsAfter => store.xrefs_after(None, 10).await.map(|_| ()),
        Read::Card => store.read_card(card()).await.map(|_| ()),
        Read::CardsForAccount => store.cards_for_account(account(1)).await.map(|_| ()),
        Read::DisclosureGroup => {
            let key = DisclosureGroupKey::default_group(TranTypeCode::move_from("01"), category(1));
            store.read_disclosure_group(key).await.map(|_| ())
        }
        Read::Tcatbal => {
            let key = TranCatBalanceKey {
                trancat_acct_id: account(1),
                trancat_type_cd: TranTypeCode::move_from("01"),
                trancat_cd: category(1),
            };
            store.read_tcatbal(key).await.map(|_| ())
        }
        Read::TcatbalsAfter => store.tcatbals_after(None, 10).await.map(|_| ()),
        Read::SummariesAfter => store.summaries_after(None, 10).await.map(|_| ()),
        Read::Summary => store.read_summary(account(1)).await.map(|_| ()),
        Read::DetailsForAccount => store
            .details_for_account(account(1), first_page())
            .await
            .map(|_| ()),
        Read::Detail => store.read_detail(AuthId::from(1)).await.map(|_| ()),
        _ => other_read(uow, read).await,
    }
}

/// [`run_read`] for the user, reference-data, transaction and reward reads.
async fn other_read(uow: &mut UnitOfWork, read: Read) -> PortResult<()> {
    let mut store = uow.store();
    match read {
        Read::User => store
            .read_user(UserId::move_from("ADMIN001"))
            .await
            .map(|_| ()),
        Read::BrowseUsers => store.browse_users(first_page()).await.map(|_| ()),
        Read::TranType => store
            .read_tran_type(TranTypeCode::move_from("01"))
            .await
            .map(|_| ()),
        Read::TranTypeFile => store
            .read_tran_type_file(TranTypeCode::move_from("01"))
            .await
            .map(|_| ()),
        Read::Transaction => {
            let tran_id = TranId::from_trimmed("0000000000000001").expect("a 16-digit TRAN-ID");
            store.read_transaction(tran_id).await.map(|_| ())
        }
        Read::BrowseTransactions => store
            .browse_transactions(first_page(), None)
            .await
            .map(|_| ()),
        Read::HighestTransaction => store.highest_transaction().await.map(|_| ()),
        Read::RewardBalance => store.reward_balance(account(1)).await.map(|_| ()),
        Read::RewardHistory => store.reward_history(account(1), None, 10).await.map(|_| ()),
        Read::CandidateRules => uow
            .rewards()
            .candidate_rules(TranTypeCode::move_from("01"), category(1))
            .await
            .map(|_| ()),
        _ => panic!("{read:?} is a read of run_read"),
    }
}

/// The marker a case writes, unique per case so each assertion sees its own.
fn marker(read: Read) -> String {
    format!("{read:?}").chars().take(10).collect()
}

/// Whether `read` names `accounts`, so that the case writes `customers`.
fn reads_accounts(read: Read) -> bool {
    read == Read::Account
}

/// The legacy write of a case, on a table its read does not name: account 1's
/// `ACCT-GROUP-ID`, or customer 1's address line 3 when the read hides
/// `accounts`. Both start from the stored record, read in the same unit.
async fn write_marker(uow: &mut UnitOfWork, read: Read) -> PortResult<()> {
    let mut store = uow.store();
    if reads_accounts(read) {
        let mut record = store.read_customer(customer(1)).await?;
        record.cust_addr_line_3 = Alnum::move_from(&marker(read));
        store.rewrite_customer(record).await
    } else {
        let mut record = store.read_account(account(1)).await?;
        record.acct_group_id = AccountGroupId::move_from(&marker(read));
        store.rewrite_account(record).await
    }
}

/// Reads the written row back inside the unit: a statement after the failed
/// read, which must not fail with `25P02`.
async fn read_back(uow: &mut UnitOfWork, read: Read) -> PortResult<String> {
    let mut store = uow.store();
    if reads_accounts(read) {
        let record = store.read_customer(customer(1)).await?;
        Ok(record.cust_addr_line_3.trimmed().into_owned())
    } else {
        let record = store.read_account(account(1)).await?;
        Ok(record.acct_group_id.trimmed().into_owned())
    }
}

/// The committed value of the written column, read outside the unit.
async fn committed_marker(stack: &TestStack, read: Read) -> Option<String> {
    let sql = if reads_accounts(read) {
        "SELECT cust_addr_line_3 FROM customers WHERE customer_id = 1"
    } else {
        "SELECT acct_group_id FROM accounts WHERE account_id = 1"
    };
    stack
        .fetch_text_rows(sql)
        .await
        .into_iter()
        .next()
        .and_then(|row| row.into_iter().next())
        .flatten()
}

/// Runs one case: hide, write, failing read, read back, commit, restore.
async fn run_case(stack: &TestStack, db: &Db, case: &Case) {
    let read = case.read;
    stack.exec_sql(case.hide).await;
    let mut uow = db.begin().await.expect("a unit of work");
    write_marker(&mut uow, read)
        .await
        .unwrap_or_else(|error| panic!("{read:?}: the write before the read failed: {error}"));
    match run_read(&mut uow, read).await {
        Err(PortError::Unavailable(text)) => assert!(
            text.contains(UNDEFINED_TABLE),
            "{read:?}: the failure does not name {UNDEFINED_TABLE}: {text}"
        ),
        other => panic!("{read:?}: expected Unavailable with {UNDEFINED_TABLE}, got {other:?}"),
    }
    let seen = read_back(&mut uow, read).await.unwrap_or_else(|error| {
        panic!("{read:?}: a statement after the failed read failed: {error}")
    });
    assert_eq!(seen, marker(read), "{read:?}: the write is not in the unit");
    uow.commit().await.expect("the unit of work commits");
    stack.exec_sql(case.restore).await;
    assert_eq!(
        committed_marker(stack, read).await,
        Some(marker(read)),
        "{read:?}: the write before the failed read did not persist"
    );
}

#[test]
fn a_failed_read_leaves_the_unit_of_work_usable_and_its_earlier_write_committed() {
    // The seed is prepared on the test thread, outside every runtime, as an adapter prepares it
    // on its blocking pool, so the argon2id hashing never runs inside the async phase.
    let prepared = seed::prepare(
        SeedMode::Full,
        &SeedSources::with_defaults(&fixtures::repo_root()),
    );
    block_on(async {
        let stack = TestStack::postgres().await;
        let db = Db::from_pool(stack.pool().clone());
        seed::run(&db, prepared)
            .await
            .expect("the fixture seed loads");
        for case in &CASES {
            run_case(&stack, &db, case).await;
        }
    });
}
