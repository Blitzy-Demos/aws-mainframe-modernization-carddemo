//! Integration tests of `carddemo_persistence::seed`: the first seed, the
//! `--if-empty` guard, the refresh that replaces the README refresh sequence
//! `ACCTFILE` … `DUSRSECJ` [README.md:L213-232], the `TRANEXTR` copy of the Db2
//! masters over the processing copies, and the single database transaction of
//! every run (AAP 0.3.2, 0.3.6, 0.9.3 "Seed counts").
//!
//! Each test starts its own migrated `postgres:16.15` container through
//! `carddemo_testkit::TestStack`, so the Docker daemon must be running. The
//! sources are the repository's own: `app/data/ASCII`, `app/jcl/DUSRSECJ.jcl`
//! and `app/app-transaction-type-db2/ctl`. Expected rows come from decoding the
//! same fixture bytes through `carddemo-codec`, never from parsing them here,
//! and nothing is written under `app/`: the corrupted copy of the failure test
//! lives under `CARGO_TARGET_TMPDIR`.
//!
//! The testkit reaches `carddemo-persistence` through a dev-dependency cycle,
//! so only plain sqlx values cross from it: every `Db` here wraps a clone of
//! the stack's `PgPool`.

use std::fs;
use std::path::Path;

use carddemo_codec::layouts::tran_cat_balance::TRAN_CAT_BAL;
use carddemo_codec::{
    AccountLayout, CardLayout, CardXrefLayout, CodecErrorKind, CustomerLayout,
    DisclosureGroupLayout, Layout, TranCatBalanceLayout, decode_ascii_file, split_ascii,
};
use carddemo_domain::auth::{hash_stored_password, verify_as_entered, verify_signon};
use carddemo_domain::numeric::AccountId;
use carddemo_domain::ports::{
    AccountReader, CardReader, CustomerReader, DisclosureGroupReader, PortError,
    TranCatBalanceStore, XrefReader,
};
use carddemo_persistence::seed::{SeedMode, SeedReport, SeedSources, run};
use carddemo_persistence::{Db, PersistenceError, PgStore};
use carddemo_testkit::{TestStack, block_on, fixtures};
use chrono::{DateTime, TimeZone, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

/// The balance account 1 is edited to after a seed: 12345.67.
const EDITED_BALANCE_CENTS: i64 = 1_234_567;

/// Rows per table, read in one statement.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Counts {
    accounts: i64,
    cards: i64,
    card_xref: i64,
    customers: i64,
    disclosure_groups: i64,
    transaction_category_balances: i64,
    transaction_types: i64,
    trantype_file: i64,
    transaction_categories: i64,
    trancatg_file: i64,
    users: i64,
    reward_rules: i64,
    reward_accruals: i64,
    reward_balances: i64,
    transactions: i64,
}

/// A content fingerprint per table: the MD5 of every row in its text form,
/// sorted. Two snapshots are equal only when no table gained, lost or changed
/// a row.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snapshot {
    accounts: String,
    cards: String,
    card_xref: String,
    customers: String,
    disclosure_groups: String,
    transaction_category_balances: String,
    transaction_types: String,
    trantype_file: String,
    transaction_categories: String,
    trancatg_file: String,
    users: String,
    reward_rules: String,
    reward_accruals: String,
    reward_balances: String,
    transactions: String,
    pending_auth_summaries: String,
    pending_auth_details: String,
    auth_fraud: String,
}

/// The descriptions of type `01`, type `06` and category `01`/`0001` in both
/// versions of the reference data, `None` where the row is missing.
#[derive(Debug)]
struct Descriptions {
    master_type_01: Option<String>,
    master_type_06: Option<String>,
    master_category_0001: Option<String>,
    file_type_01: Option<String>,
    file_type_06: Option<String>,
    file_category_0001: Option<String>,
}

/// One `reward_rules` row as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RuleRow {
    rule_id: i64,
    transaction_type_cd: String,
    transaction_category_cd: i32,
    reward_rate: Decimal,
    min_transaction_amount: Option<Decimal>,
    max_transaction_amount: Option<Decimal>,
    active_from: DateTime<Utc>,
    active_to: Option<DateTime<Utc>>,
}

/// The seed sources at their places in the repository, read only.
fn sources() -> SeedSources {
    SeedSources::with_defaults(&fixtures::repo_root())
}

/// A fresh migrated database, and this crate's `Db` over its pool.
fn stack() -> (TestStack, Db) {
    let stack = block_on(TestStack::postgres());
    let db = Db::from_pool(stack.pool().clone());
    (stack, db)
}

/// One seed run on the shared test runtime.
fn run_seed(
    db: &Db,
    mode: SeedMode,
    sources: &SeedSources,
) -> Result<SeedReport, PersistenceError> {
    block_on(Box::pin(run(db, mode, sources)))
}

/// The report of a first seed: every fixture record and `CREADB21` row
/// inserted, the Db2 masters loaded.
fn first_seed_report() -> SeedReport {
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
    }
}

/// The table counts of the seeded state (AAP 0.9.3 "Seed counts"). The
/// transaction master stays empty: the `DALYTRAN.PS.INIT` record `TRANFILE`
/// loads holds NUL bytes no text column can store (E-07).
fn fixture_counts() -> Counts {
    Counts {
        accounts: 50,
        cards: 50,
        card_xref: 50,
        customers: 50,
        disclosure_groups: 51,
        transaction_category_balances: 50,
        transaction_types: 7,
        trantype_file: 7,
        transaction_categories: 18,
        trancatg_file: 18,
        users: 10,
        reward_rules: 2,
        reward_accruals: 0,
        reward_balances: 0,
        transactions: 0,
    }
}

/// The seed reward rules of AAP 0.3.6: retail purchase `01`/`0001` at 1 point
/// per dollar and travel `04`/`0003` at 3, both open-ended from
/// 2022-01-01T00:00:00Z with no amount bounds.
fn seed_rules() -> Vec<RuleRow> {
    let active_from = Utc
        .with_ymd_and_hms(2022, 1, 1, 0, 0, 0)
        .single()
        .expect("2022-01-01T00:00:00Z is one instant");
    [(1, "01", 1, 10_000), (2, "04", 3, 30_000)]
        .into_iter()
        .map(|(rule_id, type_cd, category_cd, rate_units)| RuleRow {
            rule_id,
            transaction_type_cd: type_cd.to_owned(),
            transaction_category_cd: category_cd,
            reward_rate: Decimal::new(rate_units, 4),
            min_transaction_amount: None,
            max_transaction_amount: None,
            active_from,
            active_to: None,
        })
        .collect()
}

/// An account ID of the fixture range.
fn account_id(value: u64) -> AccountId {
    AccountId::new(value).expect("an 11-digit account ID")
}

/// Every record of the ASCII fixture `name`, decoded with the layout `L` in
/// file order.
fn fixture<L: Layout>(name: &str) -> Vec<L::Record> {
    let path = fixtures::ascii_dir().join(name);
    let bytes =
        fs::read(&path).unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
    decode_ascii_file::<L>(&bytes)
        .unwrap_or_else(|err| panic!("cannot decode {}: {err}", path.display()))
        .into_iter()
        .map(|framed| framed.record)
        .collect()
}

/// Rows per table.
async fn counts(pool: &PgPool) -> Counts {
    sqlx::query_as!(
        Counts,
        r#"SELECT
               (SELECT count(*) FROM accounts) AS "accounts!",
               (SELECT count(*) FROM cards) AS "cards!",
               (SELECT count(*) FROM card_xref) AS "card_xref!",
               (SELECT count(*) FROM customers) AS "customers!",
               (SELECT count(*) FROM disclosure_groups) AS "disclosure_groups!",
               (SELECT count(*) FROM transaction_category_balances)
                   AS "transaction_category_balances!",
               (SELECT count(*) FROM transaction_types) AS "transaction_types!",
               (SELECT count(*) FROM trantype_file) AS "trantype_file!",
               (SELECT count(*) FROM transaction_categories) AS "transaction_categories!",
               (SELECT count(*) FROM trancatg_file) AS "trancatg_file!",
               (SELECT count(*) FROM users) AS "users!",
               (SELECT count(*) FROM reward_rules) AS "reward_rules!",
               (SELECT count(*) FROM reward_accruals) AS "reward_accruals!",
               (SELECT count(*) FROM reward_balances) AS "reward_balances!",
               (SELECT count(*) FROM transactions) AS "transactions!""#
    )
    .fetch_one(pool)
    .await
    .expect("table counts")
}

/// The content fingerprint of every table.
async fn snapshot(pool: &PgPool) -> Snapshot {
    sqlx::query_as!(
        Snapshot,
        r#"SELECT
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM accounts t) AS "accounts!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM cards t) AS "cards!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM card_xref t) AS "card_xref!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM customers t) AS "customers!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM disclosure_groups t) AS "disclosure_groups!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM transaction_category_balances t) AS "transaction_category_balances!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM transaction_types t) AS "transaction_types!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM trantype_file t) AS "trantype_file!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM transaction_categories t) AS "transaction_categories!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM trancatg_file t) AS "trancatg_file!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM users t) AS "users!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM reward_rules t) AS "reward_rules!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM reward_accruals t) AS "reward_accruals!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM reward_balances t) AS "reward_balances!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM transactions t) AS "transactions!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM pending_auth_summaries t) AS "pending_auth_summaries!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM pending_auth_details t) AS "pending_auth_details!",
               (SELECT md5(coalesce(string_agg(t::text, '|' ORDER BY t::text), ''))
                  FROM auth_fraud t) AS "auth_fraud!""#
    )
    .fetch_one(pool)
    .await
    .expect("table fingerprints")
}

/// The descriptions of type `01`, type `06` and category `01`/`0001` in the Db2
/// masters and in the processing copies.
async fn descriptions(pool: &PgPool) -> Descriptions {
    sqlx::query_as!(
        Descriptions,
        r#"SELECT
               (SELECT tr_description FROM transaction_types WHERE tr_type = '01')
                   AS "master_type_01?",
               (SELECT tr_description FROM transaction_types WHERE tr_type = '06')
                   AS "master_type_06?",
               (SELECT trc_cat_data FROM transaction_categories
                 WHERE trc_type_code = '01' AND trc_type_category = '0001')
                   AS "master_category_0001?",
               (SELECT tran_type_desc FROM trantype_file WHERE tran_type = '01')
                   AS "file_type_01?",
               (SELECT tran_type_desc FROM trantype_file WHERE tran_type = '06')
                   AS "file_type_06?",
               (SELECT tran_cat_type_desc FROM trancatg_file
                 WHERE tran_type_cd = '01' AND tran_cat_cd = 1)
                   AS "file_category_0001?""#
    )
    .fetch_one(pool)
    .await
    .expect("reference descriptions")
}

/// Every reward rule in `rule_id` order.
async fn reward_rules(pool: &PgPool) -> Vec<RuleRow> {
    sqlx::query_as!(
        RuleRow,
        r#"SELECT rule_id, transaction_type_cd, transaction_category_cd, reward_rate,
                  min_transaction_amount, max_transaction_amount, active_from, active_to
             FROM reward_rules
            ORDER BY rule_id"#
    )
    .fetch_all(pool)
    .await
    .expect("reward rules")
}

/// Inserts a rule as the reward-rule admin would, returning its identity.
async fn insert_rule(pool: &PgPool) -> i64 {
    let active_from = Utc
        .with_ymd_and_hms(2022, 7, 1, 0, 0, 0)
        .single()
        .expect("2022-07-01T00:00:00Z is one instant");
    sqlx::query_scalar!(
        r#"INSERT INTO reward_rules (
               transaction_type_cd, transaction_category_cd, reward_rate, active_from)
           VALUES ('02', 1, 2.5000, $1)
           RETURNING rule_id"#,
        active_from
    )
    .fetch_one(pool)
    .await
    .expect("rule insert")
}

/// Sets the current balance of account 1.
async fn set_account_one_balance(pool: &PgPool, cents: i64) {
    let balance = Decimal::new(cents, 2);
    sqlx::query!(
        "UPDATE accounts SET acct_curr_bal = $1 WHERE account_id = 1",
        balance
    )
    .execute(pool)
    .await
    .expect("account 1 balance update");
}

/// The current balance of account 1.
async fn account_one_balance(pool: &PgPool) -> Decimal {
    sqlx::query_scalar!("SELECT acct_curr_bal FROM accounts WHERE account_id = 1")
        .fetch_one(pool)
        .await
        .expect("account 1 balance")
}

/// Renames the Db2 master type `01`, as CTTU, CTLI or `MNTTRDB2` would.
async fn edit_type_01_master(pool: &PgPool) {
    sqlx::query!(
        "UPDATE transaction_types SET tr_description = 'EDITED PURCHASE' WHERE tr_type = '01'"
    )
    .execute(pool)
    .await
    .expect("type 01 master update");
}

/// Makes, and commits, the changes a refresh must undo or keep after a seed:
/// account 1's balance, the Db2 master type `01`, an extra user `TEMP0001`, an
/// extra rule (identity 3), a live transaction on account 1's card, and a
/// reward balance and accrual of account 1 against that transaction and rule 1.
///
/// Returns the row identity of the transaction.
async fn edit_seeded_state(pool: &PgPool) -> i64 {
    set_account_one_balance(pool, EDITED_BALANCE_CENTS).await;
    edit_type_01_master(pool).await;
    let password_hash = hash_stored_password("TEMPPASS").expect("argon2id hash");
    sqlx::query!(
        r#"INSERT INTO users (user_id, sec_usr_fname, sec_usr_lname, password_hash, sec_usr_type)
           VALUES ('TEMP0001', 'TEMPORARY', 'USER', $1, 'U')"#,
        password_hash
    )
    .execute(pool)
    .await
    .expect("user insert");
    assert_eq!(
        insert_rule(pool).await,
        3,
        "the rule after the two seed rules"
    );
    let card = sqlx::query_scalar!(
        "SELECT xref_card_num FROM card_xref WHERE xref_acct_id = 1 ORDER BY xref_card_num LIMIT 1"
    )
    .fetch_one(pool)
    .await
    .expect("a card of account 1");
    let transaction_id = sqlx::query_scalar!(
        r#"INSERT INTO transactions (
               tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt,
               tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip,
               tran_card_num, tran_orig_ts, tran_proc_ts)
           VALUES ('0000000000000001', '01', 1, 'POS TERM', 'Refresh survivor', 10.00,
                   800000001, 'Merchant', 'City', '12345',
                   $1, '2022-07-01 12:00:00.000000', '2022-07-01-12.00.00.000000')
           RETURNING transaction_id"#,
        card
    )
    .fetch_one(pool)
    .await
    .expect("transaction insert");
    sqlx::query!(
        r#"INSERT INTO reward_balances (
               account_id, total_points_earned, total_points_redeemed, available_points)
           VALUES (1, 10, 0, 10)"#
    )
    .execute(pool)
    .await
    .expect("reward balance insert");
    let created_at = Utc
        .with_ymd_and_hms(2022, 7, 1, 12, 0, 0)
        .single()
        .expect("2022-07-01T12:00:00Z is one instant");
    sqlx::query!(
        r#"INSERT INTO reward_accruals (account_id, transaction_id, rule_id, points_earned, created_at)
           VALUES (1, $1, 1, 10, $2)"#,
        transaction_id,
        created_at
    )
    .execute(pool)
    .await
    .expect("reward accrual insert");
    transaction_id
}

/// `ACCTFILE`: every `acctdata.txt` record reads back through `AccountReader`.
async fn accounts_equal_fixture(store: &mut PgStore<'_, '_>) {
    let records = fixture::<AccountLayout>("acctdata.txt");
    assert_eq!(records.len(), 50);
    for record in records {
        let expected = record.account;
        let stored = store
            .read_account(expected.acct_id)
            .await
            .unwrap_or_else(|err| panic!("account {}: {err}", expected.acct_id));
        assert_eq!(stored, expected);
    }
}

/// `CARDFILE`: every `carddata.txt` record reads back through `CardReader`.
async fn cards_equal_fixture(store: &mut PgStore<'_, '_>) {
    let records = fixture::<CardLayout>("carddata.txt");
    assert_eq!(records.len(), 50);
    for record in records {
        let expected = record.card;
        let stored = store
            .read_card(expected.card_num)
            .await
            .unwrap_or_else(|err| panic!("card {:?}: {err}", expected.card_num));
        assert_eq!(stored, expected);
    }
}

/// `XREFFILE`: every `cardxref.txt` record, stored as 36 of its 50 bytes
/// (E-04), reads back through `XrefReader`.
async fn xrefs_equal_fixture(store: &mut PgStore<'_, '_>) {
    let records = fixture::<CardXrefLayout>("cardxref.txt");
    assert_eq!(records.len(), 50);
    for record in records {
        let expected = record.xref;
        let stored = store
            .read_xref_by_card(expected.xref_card_num)
            .await
            .unwrap_or_else(|err| panic!("cross-reference {:?}: {err}", expected.xref_card_num));
        assert_eq!(stored, expected);
    }
}

/// `CUSTFILE`: every `custdata.txt` record reads back through `CustomerReader`.
async fn customers_equal_fixture(store: &mut PgStore<'_, '_>) {
    let records = fixture::<CustomerLayout>("custdata.txt");
    assert_eq!(records.len(), 50);
    for record in records {
        let expected = record.customer;
        let stored = store
            .read_customer(expected.cust_id)
            .await
            .unwrap_or_else(|err| panic!("customer {}: {err}", expected.cust_id));
        assert_eq!(stored, expected);
    }
}

/// `TCATBALF`: every `tcatbal.txt` record, CRLF and LF framed alike (E-04),
/// reads back through `TranCatBalanceStore`.
async fn tran_cat_balances_equal_fixture(store: &mut PgStore<'_, '_>) {
    let records = fixture::<TranCatBalanceLayout>("tcatbal.txt");
    assert_eq!(records.len(), 50);
    for record in records {
        let expected = record.balance;
        let stored = store
            .read_tcatbal(expected.key.clone())
            .await
            .unwrap_or_else(|err| panic!("category balance {:?}: {err}", expected.key));
        assert_eq!(stored, expected);
    }
}

/// `DISCGRP`: every `discgrp.txt` record reads back through
/// `DisclosureGroupReader`, the ASCII rate of record 34 included (E-05).
async fn disclosure_groups_equal_fixture(store: &mut PgStore<'_, '_>) {
    let records = fixture::<DisclosureGroupLayout>("discgrp.txt");
    assert_eq!(records.len(), 51);
    for record in records {
        let expected = record.group;
        let stored = store
            .read_disclosure_group(expected.key.clone())
            .await
            .unwrap_or_else(|err| panic!("disclosure group {:?}: {err}", expected.key));
        assert_eq!(stored, expected);
    }
}

/// Account 1 reads back exactly as `acctdata.txt` record 1 decodes.
async fn account_one_equals_fixture(db: &Db) {
    let expected = fixture::<AccountLayout>("acctdata.txt")
        .into_iter()
        .next()
        .expect("acctdata.txt record 1")
        .account;
    assert_eq!(expected.acct_id, account_id(1));
    let mut uow = db.begin().await.expect("unit of work");
    let stored = uow.store().read_account(expected.acct_id).await;
    uow.rollback().await.expect("rollback");
    assert_eq!(stored, Ok(expected));
}

/// The seed sources with the fixture directory replaced by a copy under this
/// test's directory in `CARGO_TARGET_TMPDIR`, whose `tcatbal.txt` has a plain
/// digit in the sign position of its last record's `TRAN-CAT-BAL`. The load
/// therefore fails at the sixth table, after five tables have been reloaded
/// inside the run's transaction.
fn corrupt_sources(test: &str) -> SeedSources {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("seed-corrupt-{test}"));
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .unwrap_or_else(|err| panic!("cannot clear {}: {err}", dir.display()));
    }
    fs::create_dir_all(&dir).unwrap_or_else(|err| panic!("cannot create {}: {err}", dir.display()));
    let source_dir = fixtures::ascii_dir();
    let entries = fs::read_dir(&source_dir)
        .unwrap_or_else(|err| panic!("cannot list {}: {err}", source_dir.display()));
    for entry in entries {
        let path = entry.expect("a fixture directory entry").path();
        if path.is_file() {
            let target = dir.join(path.file_name().expect("a fixture file name"));
            fs::copy(&path, &target)
                .unwrap_or_else(|err| panic!("cannot copy {}: {err}", path.display()));
        }
    }
    let tcatbal = dir.join("tcatbal.txt");
    let bytes =
        fs::read(&tcatbal).unwrap_or_else(|err| panic!("cannot read {}: {err}", tcatbal.display()));
    fs::write(&tcatbal, with_plain_last_sign(&bytes))
        .unwrap_or_else(|err| panic!("cannot write {}: {err}", tcatbal.display()));
    SeedSources {
        fixture_dir: dir,
        ..sources()
    }
}

/// `tcatbal.txt` with the trailing overpunch of the last record's
/// `TRAN-CAT-BAL` replaced by the plain digit `0`. Records are found through
/// the codec's framing and the sign byte through the layout's field, and every
/// other byte, terminators included, is kept.
fn with_plain_last_sign(bytes: &[u8]) -> Vec<u8> {
    let records = split_ascii(bytes);
    let sign_at = TRAN_CAT_BAL.offset + TRAN_CAT_BAL.len - 1;
    let mut out = Vec::with_capacity(bytes.len());
    for (number, (frame, body)) in (1..).zip(&records) {
        let start = out.len();
        out.extend_from_slice(body);
        if number == records.len() {
            assert_eq!(out[start + sign_at], b'{', "the last balance ends in +0");
            out[start + sign_at] = b'0';
        }
        out.extend_from_slice(frame.terminator.as_bytes());
    }
    assert_eq!(out.len(), bytes.len());
    out
}

/// `err` is the codec error of the corrupted sign byte: `tcatbal.txt` record
/// 50, field `TRAN-CAT-BAL`.
fn assert_last_tcatbal_sign_rejected(err: &PersistenceError, sources: &SeedSources) {
    let PersistenceError::Codec { path, source } = err else {
        panic!("expected the codec error of tcatbal.txt, got {err}");
    };
    assert_eq!(path, &sources.fixture_dir.join("tcatbal.txt"));
    assert_eq!(source.record, Some(50));
    assert_eq!(source.field, Some(TRAN_CAT_BAL.name));
    assert_eq!(
        source.kind,
        CodecErrorKind::PlainDigitInSigned { byte: b'0' }
    );
}

/// `ACCTFILE`, `CARDFILE`, `XREFFILE`, `CUSTFILE`, `DISCGRP`, `TCATBALF`,
/// `TRANTYPE`, `TRANCATG` and `DUSRSECJ`, the `CREADB21` loads and the seed
/// reward rules on an empty database: the seed counts of AAP 0.9.3, with the
/// transaction master and the reward history still empty (E-07, AAP 0.3.6).
#[test]
fn full_seed_loads_fixture_counts() {
    let (_stack, db) = stack();
    let report = run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    assert_eq!(report, first_seed_report());
    assert_eq!(block_on(counts(db.pool())), fixture_counts());
}

/// `ACCTFILE`, `CARDFILE`, `XREFFILE`, `CUSTFILE`, `TCATBALF` and `DISCGRP`
/// STEP15 `REPRO`: every stored row reads back through its port exactly as the
/// codec decodes its fixture record, blank group IDs included (E-06).
#[test]
fn seeded_rows_equal_codec_decode() {
    let (_stack, db) = stack();
    run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    block_on(Box::pin(async {
        let mut uow = db.begin().await.expect("unit of work");
        let mut store = uow.store();
        accounts_equal_fixture(&mut store).await;
        cards_equal_fixture(&mut store).await;
        xrefs_equal_fixture(&mut store).await;
        customers_equal_fixture(&mut store).await;
        tran_cat_balances_equal_fixture(&mut store).await;
        disclosure_groups_equal_fixture(&mut store).await;
        let first = store
            .read_account(account_id(1))
            .await
            .expect("account 00000000001");
        assert_eq!(first.acct_curr_bal.to_decimal(), Decimal::new(19_400, 2));
        assert!(first.acct_group_id.trimmed().is_empty());
        assert_eq!(
            store.read_account(account_id(99_999_999_999)).await,
            Err(PortError::NotFound)
        );
        uow.rollback().await.expect("rollback");
    }));
}

/// `CREADB21` `LDTTYPE` and `LDTCCAT` against `TRANTYPE` and `TRANCATG`
/// STEP15: the Db2 masters hold the upper-case `DB2LT*.ctl` literals,
/// `REVERAL` reproduced as written, while the processing copies hold the
/// fixtures' mixed case, right-trimmed (AAP 0.3.2).
#[test]
fn reference_data_versions_differ() {
    let (_stack, db) = stack();
    run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    let found = block_on(descriptions(db.pool()));
    assert_eq!(found.master_type_01.as_deref(), Some("PURCHASE"));
    assert_eq!(found.master_type_06.as_deref(), Some("REVERAL"));
    assert_eq!(
        found.master_category_0001.as_deref(),
        Some("REGULAR SALES DRAFT")
    );
    assert_eq!(found.file_type_01.as_deref(), Some("Purchase"));
    assert_eq!(found.file_type_06.as_deref(), Some("Reversal"));
    assert_eq!(
        found.file_category_0001.as_deref(),
        Some("Regular Sales Draft")
    );
}

/// `DUSRSECJ` STEP01 `SYSUT1` → `users`: the ten in-stream users with their
/// types, each password stored only as an argon2id hash of the 8 bytes as
/// stored. Sign-on upper-cases what is typed, so `password` signs on; the
/// stored case is kept, so only `PASSWORD` verifies as entered (E-20).
#[test]
fn users_are_hashed_and_typed() {
    let (_stack, db) = stack();
    run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    let (users, password_columns) = block_on(async {
        let users =
            sqlx::query!("SELECT user_id, sec_usr_type, password_hash FROM users ORDER BY user_id")
                .fetch_all(db.pool())
                .await
                .expect("users");
        let columns = sqlx::query_scalar!(
            r#"SELECT column_name::text AS "column_name!"
                 FROM information_schema.columns
                WHERE table_schema = current_schema()
                  AND table_name = 'users'
                  AND (column_name::text ILIKE '%pw%' OR column_name::text ILIKE '%password%')
                ORDER BY 1"#
        )
        .fetch_all(db.pool())
        .await
        .expect("users columns");
        (users, columns)
    });
    assert_eq!(password_columns, ["password_hash"], "no plaintext column");
    let ids: Vec<&str> = users.iter().map(|user| user.user_id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005", "USER0001", "USER0002",
            "USER0003", "USER0004", "USER0005"
        ]
    );
    for (user_type, expected) in [("A", 5), ("U", 5)] {
        let found = users.iter().filter(|user| user.sec_usr_type == user_type);
        assert_eq!(found.count(), expected, "users of type {user_type}");
    }
    assert!(
        users
            .iter()
            .all(|user| user.password_hash.starts_with("$argon2id$"))
    );
    for (user_id, user_type) in [("ADMIN001", "A"), ("USER0001", "U")] {
        let user = users
            .iter()
            .find(|user| user.user_id == user_id)
            .unwrap_or_else(|| panic!("{user_id} is seeded"));
        assert_eq!(user.sec_usr_type, user_type);
        let hash = user.password_hash.as_str();
        let verify = |result: Result<bool, _>| result.expect("a parseable argon2id hash");
        assert!(
            verify(verify_signon("password", hash)),
            "{user_id} lower case"
        );
        assert!(
            verify(verify_signon("PASSWORD", hash)),
            "{user_id} upper case"
        );
        assert!(
            verify(verify_as_entered("PASSWORD", hash)),
            "{user_id} as stored"
        );
        assert!(
            !verify(verify_as_entered("password", hash)),
            "{user_id} case kept"
        );
    }
}

/// The seed reward rules (AAP 0.3.6, E-08): retail `01`/`0001` at 1.0000 and
/// travel `04`/`0003` at 3.0000, with their fixed identities 1 and 2.
#[test]
fn seed_reward_rules() {
    let (_stack, db) = stack();
    run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    let rules = block_on(reward_rules(db.pool()));
    assert_eq!(rules, seed_rules());
    let rates: Vec<String> = rules
        .iter()
        .map(|rule| rule.reward_rate.to_string())
        .collect();
    assert_eq!(rates, ["1.0000", "3.0000"], "NUMERIC(9,4) keeps scale 4");
}

/// `batch seed --if-empty`: on an empty database it is the first seed; on a
/// seeded one it writes nothing, so an edit made since survives.
#[test]
fn if_empty_on_seeded_db_is_a_noop() {
    let (_stack, db) = stack();
    let first = run_seed(&db, SeedMode::IfEmpty, &sources()).expect("first seed");
    assert_eq!(first, first_seed_report());
    block_on(set_account_one_balance(db.pool(), EDITED_BALANCE_CENTS));
    let before = block_on(snapshot(db.pool()));
    let report = run_seed(&db, SeedMode::IfEmpty, &sources()).expect("if-empty run");
    assert_eq!(
        report,
        SeedReport {
            skipped: true,
            ..SeedReport::default()
        }
    );
    assert_eq!(block_on(snapshot(db.pool())), before);
    assert_eq!(
        block_on(account_one_balance(db.pool())),
        Decimal::new(EDITED_BALANCE_CENTS, 2)
    );
}

/// The README refresh sequence `ACCTFILE` … `DUSRSECJ` [README.md:L213-232] as
/// a second `batch seed` (AAP 0.3.2): the master files, the processing copies,
/// the users and the seed rules are reloaded, rule identities restart after
/// rule 2, and the reward history is cleared. The Db2 masters keep their edit,
/// because the sequence runs no `CREADB21`, and the transaction master keeps
/// its rows, because the seed never names it.
#[test]
fn full_refresh_restores_fixtures_and_keeps_masters() {
    let (_stack, db) = stack();
    run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    let pool = db.pool();
    let transaction_id = block_on(edit_seeded_state(pool));
    let before = block_on(snapshot(pool));
    let report = run_seed(&db, SeedMode::Full, &sources()).expect("refresh");
    assert_eq!(
        report,
        SeedReport {
            transaction_types: 0,
            transaction_categories: 0,
            masters_loaded: false,
            refreshed: true,
            ..first_seed_report()
        }
    );
    block_on(Box::pin(async {
        assert_eq!(
            counts(pool).await,
            Counts {
                transactions: 1,
                ..fixture_counts()
            }
        );
        account_one_equals_fixture(&db).await;
        let temp_user = sqlx::query_scalar!(
            r#"SELECT EXISTS (SELECT 1 FROM users WHERE user_id = 'TEMP0001') AS "found!""#
        )
        .fetch_one(pool)
        .await
        .expect("user lookup");
        assert!(!temp_user, "TEMP0001 is not a DUSRSECJ user");
        assert_eq!(reward_rules(pool).await, seed_rules());
        let found = descriptions(pool).await;
        assert_eq!(found.master_type_01.as_deref(), Some("EDITED PURCHASE"));
        assert_eq!(found.file_type_01.as_deref(), Some("Purchase"));
        let after = snapshot(pool).await;
        assert_eq!(after.transaction_types, before.transaction_types);
        assert_eq!(after.transaction_categories, before.transaction_categories);
        assert_eq!(
            after.transactions, before.transactions,
            "row and content kept"
        );
        let live = sqlx::query_scalar!(
            "SELECT in_master FROM transactions WHERE transaction_id = $1",
            transaction_id
        )
        .fetch_optional(pool)
        .await
        .expect("transaction lookup");
        assert_eq!(live, Some(true), "transaction {transaction_id} stays live");
        assert_eq!(
            insert_rule(pool).await,
            3,
            "rule identities restart after 2"
        );
    }));
}

/// `TRANEXTR` STEP30 to STEP50 and the `TRANTYPE` and `TRANCATG` reloads it
/// feeds (`batch seed --only tranextr`): the processing copies become the Db2
/// masters, an edited master description and the `REVERAL` literal included,
/// and no other table changes.
#[test]
fn only_tranextr_copies_masters_to_processing_copies() {
    let (_stack, db) = stack();
    run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    let pool = db.pool();
    block_on(edit_type_01_master(pool));
    let before = block_on(snapshot(pool));
    let report = run_seed(&db, SeedMode::OnlyTranextr, &sources()).expect("tranextr");
    assert_eq!(
        report,
        SeedReport {
            trantype_file: 7,
            trancatg_file: 18,
            ..SeedReport::default()
        }
    );
    block_on(Box::pin(async {
        let type_mismatches = sqlx::query_scalar!(
            r#"SELECT count(*) AS "mismatches!"
                 FROM transaction_types m
                 FULL JOIN trantype_file f ON f.tran_type = m.tr_type
                WHERE m.tr_type IS NULL OR f.tran_type IS NULL
                   OR f.tran_type_desc <> m.tr_description"#
        )
        .fetch_one(pool)
        .await
        .expect("type comparison");
        assert_eq!(type_mismatches, 0);
        let category_mismatches = sqlx::query_scalar!(
            r#"SELECT count(*) AS "mismatches!"
                 FROM transaction_categories m
                 FULL JOIN trancatg_file f
                   ON f.tran_type_cd = m.trc_type_code
                  AND f.tran_cat_cd = m.trc_type_category::integer
                WHERE m.trc_type_code IS NULL OR f.tran_type_cd IS NULL
                   OR f.tran_cat_type_desc <> m.trc_cat_data"#
        )
        .fetch_one(pool)
        .await
        .expect("category comparison");
        assert_eq!(category_mismatches, 0);
        assert_eq!(counts(pool).await, fixture_counts());
        let found = descriptions(pool).await;
        assert_eq!(found.file_type_01.as_deref(), Some("EDITED PURCHASE"));
        assert_eq!(found.file_type_06.as_deref(), Some("REVERAL"));
        assert_eq!(
            found.file_category_0001.as_deref(),
            Some("REGULAR SALES DRAFT")
        );
        let after = snapshot(pool).await;
        assert_ne!(after.trantype_file, before.trantype_file);
        assert_ne!(after.trancatg_file, before.trancatg_file);
        let unchanged = Snapshot {
            trantype_file: before.trantype_file.clone(),
            trancatg_file: before.trancatg_file.clone(),
            ..after
        };
        assert_eq!(unchanged, before, "only the processing copies change");
    }));
}

/// One database transaction per run: a `batch seed` whose `TCATBALF` input
/// fails to decode, after five tables were already reloaded inside the run,
/// leaves an empty database empty and a seeded, edited one exactly as it was.
#[test]
fn failed_seed_changes_nothing() {
    let (_stack, db) = stack();
    let pool = db.pool();
    let corrupt = corrupt_sources("failed_seed_changes_nothing");
    let err = run_seed(&db, SeedMode::Full, &corrupt).expect_err("corrupt tcatbal.txt");
    assert_last_tcatbal_sign_rejected(&err, &corrupt);
    assert_eq!(block_on(counts(pool)), Counts::default());

    run_seed(&db, SeedMode::Full, &sources()).expect("first seed");
    block_on(set_account_one_balance(pool, EDITED_BALANCE_CENTS));
    let before = block_on(snapshot(pool));
    let err = run_seed(&db, SeedMode::Full, &corrupt).expect_err("corrupt tcatbal.txt");
    assert_last_tcatbal_sign_rejected(&err, &corrupt);
    assert_eq!(block_on(snapshot(pool)), before);
    assert_eq!(
        block_on(account_one_balance(pool)),
        Decimal::new(EDITED_BALANCE_CENTS, 2)
    );
}
