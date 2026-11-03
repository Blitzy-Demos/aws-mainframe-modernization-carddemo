//! Schema contract of `migrations/0001_reference_data.sql` to `0005_rewards.sql`
//! and of the migrator that embeds them (`carddemo_persistence::migrate`).
//!
//! Every test starts its own `postgres:16.15` container through
//! [`TestStack::postgres`], which applies the migrations, and reads the
//! `PostgreSQL` catalogs back: the applied versions, the tables and the view, the
//! foreign keys with their delete rules, the primary key of every table with its
//! columns in key order, the identity columns with their generation and sequence
//! options, the indexes that replace the VSAM and Db2 keys and alternate
//! indexes, the `C` collation of the text columns and index keys, and the column
//! types of the money, rate and points fields. Two tests write rows to prove the
//! constraints behave as the catalogs say: the `TRAN-ID` key that holds among
//! live rows only (E-09) and the reward invariants (AAP 0.3.5). The key and the
//! collation tests then change the schema and assert that their reads report
//! exactly the change: dropped and reordered primary keys, altered identities,
//! and a column and its indexes sharing a collation other than `C`, which the
//! printed index definitions do not show.
//!
//! The tests need a running Docker daemon (AAP 0.10.4). Every read and write is
//! a compile-time checked `sqlx` macro, so `.sqlx/` carries their metadata for
//! offline builds; the DDL the tests run, a second database and the schema
//! changes, goes through [`TestStack::exec_sql`].

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;

use carddemo_persistence::{Db, connect, migrate};
use carddemo_testkit::{TestStack, block_on};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::PgPool;

/// The embedded migrations as sqlx records them: the numeric prefix of each
/// file as its version and the rest of its name, underscores as spaces, as its
/// description.
const MIGRATIONS: [(i64, &str); 5] = [
    (1, "reference data"),
    (2, "master data"),
    (3, "transactions"),
    (4, "authorizations"),
    (5, "rewards"),
];

/// The empty database `migrations_apply_and_are_idempotent` creates beside the
/// stack's own, on the same server; the literal in its `CREATE DATABASE`
/// statement names the same database.
const FRESH_DATABASE: &str = "carddemo_first_apply";

/// The base tables the five migrations create, besides sqlx's own
/// `_sqlx_migrations`.
const BASE_TABLES: [&str; 18] = [
    // 0001: the Db2 masters, their VSAM processing copies, the disclosure groups.
    "transaction_types",
    "transaction_categories",
    "trantype_file",
    "trancatg_file",
    "disclosure_groups",
    // 0002: the VSAM master files and the user security file.
    "customers",
    "accounts",
    "cards",
    "card_xref",
    "users",
    // 0003: the transaction master and the category balances.
    "transactions",
    "transaction_category_balances",
    // 0004: the IMS pending authorizations and the Db2 fraud table.
    "pending_auth_summaries",
    "pending_auth_details",
    "auth_fraud",
    // 0005: the rewards.
    "reward_rules",
    "reward_balances",
    "reward_accruals",
];

/// The tables replacing VSAM files. VSAM enforces no relationship between
/// files, so none of them declares a foreign key.
const VSAM_TABLES: [&str; 10] = [
    "trantype_file",
    "trancatg_file",
    "disclosure_groups",
    "customers",
    "accounts",
    "cards",
    "card_xref",
    "users",
    "transactions",
    "transaction_category_balances",
];

/// The collation of every text column and text index key, as
/// [`column_collations`] and [`index_key_collations`] spell it: `C` sorts and
/// compares by byte value, the order of the COBOL keys and string comparisons,
/// where the database default (`en_US.utf8`) does not (the conventions of
/// `migrations/0001_reference_data.sql` and `0002_master_data.sql`).
const C_COLLATION: &str = r#"pg_catalog."C""#;

/// The database default collation, which a text column takes when its
/// definition names none.
const DEFAULT_COLLATION: &str = r#"pg_catalog."default""#;

/// The one text column not held to `C`, as (table, column): `password_hash`,
/// the argon2id PHC string stored in place of `SEC-USR-PWD`
/// (`migrations/0002_master_data.sql:L116, L243`). It holds no COBOL field
/// value, and no key or comparison orders it.
const NOT_C_COLUMNS: [(&str, &str); 1] = [("users", "password_hash")];

/// The `TRAN-ID` of the first `dailytran.txt` record, used by the tests that
/// write transaction rows.
const TRAN_ID: &str = "0000000006835800";

/// The account of the first `acctdata.txt` record.
const ACCOUNT_ID: i64 = 1;

/// One row of `_sqlx_migrations`. `installed_on` and `execution_time` change
/// whenever a version is applied again, so equal rows before and after a run
/// prove the run applied nothing.
#[derive(Debug, PartialEq, Eq)]
struct AppliedMigration {
    version: i64,
    description: String,
    installed_on: DateTime<Utc>,
    success: bool,
    checksum: Vec<u8>,
    execution_time: i64,
}

/// One foreign key of the `public` schema, its delete rule spelled as in SQL.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ForeignKey {
    name: String,
    table: String,
    columns: Vec<String>,
    referenced_table: String,
    referenced_columns: Vec<String>,
    on_delete: String,
}

impl ForeignKey {
    fn new(
        name: &str,
        (table, columns): (&str, &[&str]),
        (referenced_table, referenced_columns): (&str, &[&str]),
        on_delete: &str,
    ) -> Self {
        let owned = |names: &[&str]| names.iter().map(|&name| name.to_owned()).collect();
        Self {
            name: name.to_owned(),
            table: table.to_owned(),
            columns: owned(columns),
            referenced_table: referenced_table.to_owned(),
            referenced_columns: owned(referenced_columns),
            on_delete: on_delete.to_owned(),
        }
    }
}

/// The primary key of one table of the `public` schema, its columns in key
/// order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PrimaryKey {
    table: String,
    name: String,
    columns: Vec<String>,
}

impl PrimaryKey {
    fn new(table: &str, name: &str, columns: &[&str]) -> Self {
        Self {
            table: table.to_owned(),
            name: name.to_owned(),
            columns: columns.iter().map(|&column| column.to_owned()).collect(),
        }
    }
}

/// A primary key the schema must hold, with the COBOL key or the identity it
/// replaces.
struct ExpectedPrimaryKey {
    table: &'static str,
    name: &'static str,
    columns: &'static [&'static str],
    replaces: &'static str,
}

impl ExpectedPrimaryKey {
    fn key(&self) -> PrimaryKey {
        PrimaryKey::new(self.table, self.name, self.columns)
    }
}

/// One identity column of the `public` schema with the options of its
/// sequence; `generation` is `ALWAYS` or `BY DEFAULT`, as `GENERATED ... AS
/// IDENTITY` spells it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IdentityColumn {
    table: String,
    column: String,
    data_type: String,
    generation: String,
    start: i64,
    increment: i64,
    minimum: i64,
    maximum: i64,
    cycle: bool,
}

impl IdentityColumn {
    /// A `BIGINT GENERATED ALWAYS AS IDENTITY` column with the sequence options
    /// `PostgreSQL` gives an identity that names none: from 1 by 1 up to the
    /// `bigint` maximum, without cycling.
    fn always(table: &str, column: &str) -> Self {
        Self {
            table: table.to_owned(),
            column: column.to_owned(),
            data_type: "bigint".to_owned(),
            generation: "ALWAYS".to_owned(),
            start: 1,
            increment: 1,
            minimum: 1,
            maximum: 9_223_372_036_854_775_807,
            cycle: false,
        }
    }
}

/// An identity column the schema must hold, with the source that makes it one.
struct ExpectedIdentity {
    table: &'static str,
    column: &'static str,
    source: &'static str,
}

impl ExpectedIdentity {
    fn identity(&self) -> IdentityColumn {
        IdentityColumn::always(self.table, self.column)
    }
}

/// One index of the `public` schema as `pg_indexes` and `pg_index` describe it.
#[derive(Debug)]
struct IndexRow {
    name: String,
    table: String,
    definition: String,
    unique: bool,
    primary: bool,
    predicate: Option<String>,
}

impl IndexRow {
    /// The key list of the definition, `(...)` after `USING btree`, without any
    /// `WHERE` predicate.
    fn key_list(&self) -> &str {
        let (_, method_and_keys) = self
            .definition
            .split_once(" USING btree ")
            .unwrap_or_else(|| panic!("{} is not a btree index: {}", self.name, self.definition));
        method_and_keys
            .split_once(" WHERE ")
            .map_or(method_and_keys, |(keys, _)| keys)
    }

    /// The first key of [`Self::key_list`]: a column name, or the text of a key
    /// expression up to its first comma.
    fn leading_key(&self) -> &str {
        let keys = self.key_list();
        let keys = keys.strip_prefix('(').unwrap_or(keys);
        keys.split([',', ')']).next().unwrap_or(keys)
    }

    /// The predicate text of the definition after `WHERE`, `None` for an index
    /// over every row.
    fn definition_predicate(&self) -> Option<&str> {
        self.definition
            .split_once(" WHERE ")
            .map(|(_, predicate)| predicate)
    }
}

/// An index the schema must hold, with the COBOL key it replaces.
struct ExpectedIndex {
    table: &'static str,
    name: &'static str,
    keys: &'static str,
    unique: bool,
    primary: bool,
    predicate: Option<&'static str>,
    replaces: &'static str,
}

/// The collation of one text column of a table or view of the `public` schema.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ColumnCollation {
    table: String,
    column: String,
    collation: String,
}

impl ColumnCollation {
    fn new(table: &str, column: &str, collation: &str) -> Self {
        Self {
            table: table.to_owned(),
            column: column.to_owned(),
            collation: collation.to_owned(),
        }
    }
}

/// One key of an index of the `public` schema: its 1-based position among the
/// index's keys, its text as `pg_get_indexdef` prints that key alone, and its
/// collation, `None` for a key whose type has none.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct KeyCollation {
    table: String,
    index: String,
    position: i32,
    key: String,
    collation: Option<String>,
}

impl KeyCollation {
    fn new(
        (table, index): (&str, &str),
        position: i32,
        key: &str,
        collation: Option<&str>,
    ) -> Self {
        Self {
            table: table.to_owned(),
            index: index.to_owned(),
            position,
            key: key.to_owned(),
            collation: collation.map(str::to_owned),
        }
    }
}

/// A padded-key expression of an index of [`EXPECTED_INDEXES`]: the text
/// column it pads, the index, and the key's position and text.
struct PaddedKey {
    table: &'static str,
    column: &'static str,
    index: &'static str,
    position: i32,
    key: &'static str,
}

impl PaddedKey {
    fn key_collation(&self, collation: &str) -> KeyCollation {
        KeyCollation::new(
            (self.table, self.index),
            self.position,
            self.key,
            Some(collation),
        )
    }
}

/// The type `information_schema.columns` reports for one column.
#[derive(Debug, PartialEq, Eq)]
struct ColumnType {
    data_type: String,
    precision: Option<i32>,
    scale: Option<i32>,
}

impl ColumnType {
    fn numeric(precision: i32, scale: i32) -> Self {
        Self {
            data_type: "numeric".to_owned(),
            precision: Some(precision),
            scale: Some(scale),
        }
    }

    /// `BIGINT`, which `information_schema` reports as binary precision 64,
    /// scale 0.
    fn bigint() -> Self {
        Self {
            data_type: "bigint".to_owned(),
            precision: Some(64),
            scale: Some(0),
        }
    }
}

// ---------------------------------------------------------------------------------------
// Catalog reads
// ---------------------------------------------------------------------------------------

/// Whether sqlx's bookkeeping table exists in the database of `pool`.
async fn migration_table_exists(pool: &PgPool) -> bool {
    sqlx::query_scalar!(r#"SELECT to_regclass('public._sqlx_migrations') IS NOT NULL AS "exists!""#)
        .fetch_one(pool)
        .await
        .expect("look up the _sqlx_migrations table")
}

/// Every version recorded in `_sqlx_migrations`, in version order.
async fn applied_migrations(pool: &PgPool) -> Vec<AppliedMigration> {
    sqlx::query_as!(
        AppliedMigration,
        "SELECT version, description, installed_on, success, checksum, execution_time
FROM _sqlx_migrations
ORDER BY version"
    )
    .fetch_all(pool)
    .await
    .expect("read _sqlx_migrations")
}

/// Every table and view of the `public` schema, with its `table_type`
/// (`BASE TABLE` or `VIEW`).
async fn relation_kinds(pool: &PgPool) -> BTreeMap<String, String> {
    sqlx::query!(
        r#"SELECT table_name::text AS "name!", table_type::text AS "kind!"
FROM information_schema.tables
WHERE table_schema = 'public'"#
    )
    .fetch_all(pool)
    .await
    .expect("read information_schema.tables")
    .into_iter()
    .map(|row| (row.name, row.kind))
    .collect()
}

/// Every foreign key of the `public` schema, its key columns in key order.
async fn foreign_keys(pool: &PgPool) -> BTreeSet<ForeignKey> {
    sqlx::query!(
        r#"SELECT
    con.conname::text AS "name!",
    src.relname::text AS "table_name!",
    ARRAY(
        SELECT att.attname::text
        FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
        JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = k.attnum
        ORDER BY k.ord
    ) AS "columns!",
    dst.relname::text AS "referenced_table!",
    ARRAY(
        SELECT att.attname::text
        FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord)
        JOIN pg_attribute att ON att.attrelid = con.confrelid AND att.attnum = k.attnum
        ORDER BY k.ord
    ) AS "referenced_columns!",
    CASE con.confdeltype
        WHEN 'a' THEN 'NO ACTION'
        WHEN 'r' THEN 'RESTRICT'
        WHEN 'c' THEN 'CASCADE'
        WHEN 'n' THEN 'SET NULL'
        WHEN 'd' THEN 'SET DEFAULT'
    END AS "on_delete!"
FROM pg_constraint con
JOIN pg_namespace nsp ON nsp.oid = con.connamespace
JOIN pg_class src ON src.oid = con.conrelid
JOIN pg_class dst ON dst.oid = con.confrelid
WHERE con.contype = 'f' AND nsp.nspname = 'public'"#
    )
    .fetch_all(pool)
    .await
    .expect("read the foreign keys from pg_constraint")
    .into_iter()
    .map(|row| ForeignKey {
        name: row.name,
        table: row.table_name,
        columns: row.columns,
        referenced_table: row.referenced_table,
        referenced_columns: row.referenced_columns,
        on_delete: row.on_delete,
    })
    .collect()
}

/// Every primary key of the `public` schema besides that of sqlx's own
/// `_sqlx_migrations`, its key columns in key order.
async fn primary_keys(pool: &PgPool) -> BTreeSet<PrimaryKey> {
    sqlx::query!(
        r#"SELECT
    cls.relname::text AS "table_name!",
    con.conname::text AS "name!",
    ARRAY(
        SELECT att.attname::text
        FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
        JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = k.attnum
        ORDER BY k.ord
    ) AS "columns!"
FROM pg_constraint con
JOIN pg_namespace nsp ON nsp.oid = con.connamespace
JOIN pg_class cls ON cls.oid = con.conrelid
WHERE con.contype = 'p' AND nsp.nspname = 'public' AND cls.relname <> '_sqlx_migrations'"#
    )
    .fetch_all(pool)
    .await
    .expect("read the primary keys from pg_constraint")
    .into_iter()
    .map(|row| PrimaryKey {
        table: row.table_name,
        name: row.name,
        columns: row.columns,
    })
    .collect()
}

/// Every identity column of the `public` schema with its generation and the
/// options of the sequence it owns, which `pg_depend` links to the column as an
/// internal dependency.
async fn identity_columns(pool: &PgPool) -> BTreeSet<IdentityColumn> {
    sqlx::query!(
        r#"SELECT
    cls.relname::text AS "table_name!",
    att.attname::text AS "column_name!",
    format_type(att.atttypid, att.atttypmod) AS "data_type!",
    CASE att.attidentity
        WHEN 'a' THEN 'ALWAYS'
        WHEN 'd' THEN 'BY DEFAULT'
    END AS "generation!",
    seq.seqstart AS "start!",
    seq.seqincrement AS "increment!",
    seq.seqmin AS "minimum!",
    seq.seqmax AS "maximum!",
    seq.seqcycle AS "cycle!"
FROM pg_attribute att
JOIN pg_class cls ON cls.oid = att.attrelid
JOIN pg_namespace nsp ON nsp.oid = cls.relnamespace
JOIN pg_depend dep
    ON dep.classid = 'pg_class'::regclass
    AND dep.refclassid = 'pg_class'::regclass
    AND dep.refobjid = cls.oid
    AND dep.refobjsubid = att.attnum
    AND dep.deptype = 'i'
JOIN pg_sequence seq ON seq.seqrelid = dep.objid
WHERE nsp.nspname = 'public' AND att.attidentity <> '' AND NOT att.attisdropped"#
    )
    .fetch_all(pool)
    .await
    .expect("read the identity columns from pg_attribute and pg_sequence")
    .into_iter()
    .map(|row| IdentityColumn {
        table: row.table_name,
        column: row.column_name,
        data_type: row.data_type,
        generation: row.generation,
        start: row.start,
        increment: row.increment,
        minimum: row.minimum,
        maximum: row.maximum,
        cycle: row.cycle,
    })
    .collect()
}

/// Every index of the `public` schema, ordered by table and name.
async fn indexes(pool: &PgPool) -> Vec<IndexRow> {
    sqlx::query!(
        r#"SELECT
    idx.indexname::text AS "name!",
    idx.tablename::text AS "table_name!",
    idx.indexdef AS "definition!",
    ind.indisunique AS "is_unique!",
    ind.indisprimary AS "is_primary!",
    pg_get_expr(ind.indpred, ind.indrelid) AS "predicate?"
FROM pg_indexes idx
JOIN pg_namespace nsp ON nsp.nspname = idx.schemaname
JOIN pg_class cls ON cls.relname = idx.indexname AND cls.relnamespace = nsp.oid
JOIN pg_index ind ON ind.indexrelid = cls.oid
WHERE idx.schemaname = 'public'
ORDER BY 2, 1"#
    )
    .fetch_all(pool)
    .await
    .expect("read pg_indexes and pg_index")
    .into_iter()
    .map(|row| IndexRow {
        name: row.name,
        table: row.table_name,
        definition: row.definition,
        unique: row.is_unique,
        primary: row.is_primary,
        predicate: row.predicate,
    })
    .collect()
}

/// The reported type of every column of every table and view of the `public`
/// schema, keyed by (table, column). The `cardinal_number` precision columns are
/// cast to `int4` so they decode as `i32`.
async fn column_types(pool: &PgPool) -> BTreeMap<(String, String), ColumnType> {
    sqlx::query!(
        r#"SELECT
    table_name::text AS "table_name!",
    column_name::text AS "column_name!",
    data_type::text AS "data_type!",
    numeric_precision::int4 AS "numeric_precision?",
    numeric_scale::int4 AS "numeric_scale?"
FROM information_schema.columns
WHERE table_schema = 'public'"#
    )
    .fetch_all(pool)
    .await
    .expect("read information_schema.columns")
    .into_iter()
    .map(|row| {
        let column_type = ColumnType {
            data_type: row.data_type,
            precision: row.numeric_precision,
            scale: row.numeric_scale,
        };
        ((row.table_name, row.column_name), column_type)
    })
    .collect()
}

/// The `public` columns of a floating-point type, plain or as an array, as
/// `table.column (type)`.
async fn float_columns(pool: &PgPool) -> Vec<String> {
    sqlx::query!(
        r#"SELECT
    table_name::text AS "table_name!",
    column_name::text AS "column_name!",
    udt_name::text AS "udt_name!"
FROM information_schema.columns
WHERE table_schema = 'public'
  AND (data_type IN ('real', 'double precision') OR udt_name IN ('_float4', '_float8'))
ORDER BY 1, 2"#
    )
    .fetch_all(pool)
    .await
    .expect("read the floating-point columns from information_schema.columns")
    .into_iter()
    .map(|row| format!("{}.{} ({})", row.table_name, row.column_name, row.udt_name))
    .collect()
}

/// The collation `pg_attribute` records for every text column of the tables
/// and views of the `public` schema besides sqlx's own `_sqlx_migrations`. A
/// column whose type has no collation records collation 0, which no
/// `pg_collation` row joins.
async fn column_collations(pool: &PgPool) -> BTreeSet<ColumnCollation> {
    sqlx::query!(
        r#"SELECT
    cls.relname::text AS "table_name!",
    att.attname::text AS "column_name!",
    quote_ident(cnsp.nspname) || '.' || quote_ident(coll.collname) AS "collation!"
FROM pg_attribute att
JOIN pg_class cls ON cls.oid = att.attrelid
JOIN pg_namespace nsp ON nsp.oid = cls.relnamespace
JOIN pg_collation coll ON coll.oid = att.attcollation
JOIN pg_namespace cnsp ON cnsp.oid = coll.collnamespace
WHERE nsp.nspname = 'public'
  AND cls.relkind IN ('r', 'p', 'v', 'm')
  AND cls.relname <> '_sqlx_migrations'
  AND att.attnum > 0
  AND NOT att.attisdropped"#
    )
    .fetch_all(pool)
    .await
    .expect("read the column collations from pg_attribute")
    .into_iter()
    .map(|row| ColumnCollation {
        table: row.table_name,
        column: row.column_name,
        collation: row.collation,
    })
    .collect()
}

/// Every key of every index on the tables of the `public` schema besides
/// `_sqlx_migrations`, with the collation `pg_index.indcollation` records for
/// it; a key whose type has no collation records collation 0, read as `None`.
async fn index_key_collations(pool: &PgPool) -> BTreeSet<KeyCollation> {
    sqlx::query!(
        r#"SELECT
    tbl.relname::text AS "table_name!",
    idx.relname::text AS "index_name!",
    k.n AS "position!",
    pg_get_indexdef(ind.indexrelid, k.n, false) AS "key!",
    quote_ident(cnsp.nspname) || '.' || quote_ident(coll.collname) AS "collation?"
FROM pg_index ind
JOIN pg_class idx ON idx.oid = ind.indexrelid
JOIN pg_class tbl ON tbl.oid = ind.indrelid
JOIN pg_namespace nsp ON nsp.oid = tbl.relnamespace
CROSS JOIN LATERAL generate_series(1, ind.indnkeyatts::int4) AS k(n)
LEFT JOIN pg_collation coll ON coll.oid = ind.indcollation[k.n - 1]
LEFT JOIN pg_namespace cnsp ON cnsp.oid = coll.collnamespace
WHERE nsp.nspname = 'public' AND tbl.relname <> '_sqlx_migrations'"#
    )
    .fetch_all(pool)
    .await
    .expect("read the index key collations from pg_index")
    .into_iter()
    .map(|row| KeyCollation {
        table: row.table_name,
        index: row.index_name,
        position: row.position,
        key: row.key,
        collation: row.collation,
    })
    .collect()
}

// ---------------------------------------------------------------------------------------
// Row writes
// ---------------------------------------------------------------------------------------

/// Inserts the first `acctdata.txt` account under `account_id`, its text fields
/// stored right-trimmed as the seed stores them (the blank `ACCT-GROUP-ID` of
/// E-06 as the empty string).
async fn insert_account(pool: &PgPool, account_id: i64) {
    sqlx::query!(
        "INSERT INTO accounts (
    account_id, acct_active_status, acct_curr_bal, acct_credit_limit, acct_cash_credit_limit,
    acct_open_date, acct_expiraion_date, acct_reissue_date, acct_curr_cyc_credit,
    acct_curr_cyc_debit, acct_addr_zip, acct_group_id)
VALUES ($1, 'Y', 194.00, 2020.00, 1020.00, '2014-11-20', '2025-05-20', '2025-05-20', 0.00,
    0.00, 'A000000000', '')",
        account_id
    )
    .execute(pool)
    .await
    .expect("insert the first acctdata.txt account");
}

/// Inserts the first `dailytran.txt` record under `tran_id`, stamped with the
/// Db2-form processing timestamp `CBTRN02C` writes, leaving `in_master` to its
/// column default. Returns the row's `transaction_id`.
async fn insert_transaction(pool: &PgPool, tran_id: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        "INSERT INTO transactions (
    tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt, tran_merchant_id,
    tran_merchant_name, tran_merchant_city, tran_merchant_zip, tran_card_num, tran_orig_ts,
    tran_proc_ts)
VALUES ($1, '01', 1, 'POS TERM', 'Purchase at Abshire-Lowe', 50.47, 800000000,
    'Abshire-Lowe', 'North Enoshaven', '72112', '4859452612877065',
    '2022-06-10 19:27:53.000000', '2022-07-01-12.00.00.000000')
RETURNING transaction_id",
        tran_id
    )
    .fetch_one(pool)
    .await
}

/// Sets `in_master` of the transaction row `transaction_id`.
async fn set_live(pool: &PgPool, transaction_id: i64, live: bool) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE transactions SET in_master = $2 WHERE transaction_id = $1",
        transaction_id,
        live
    )
    .execute(pool)
    .await
    .map(drop)
}

/// The `in_master` flag of the transaction row `transaction_id`.
async fn is_live(pool: &PgPool, transaction_id: i64) -> bool {
    sqlx::query_scalar!(
        "SELECT in_master FROM transactions WHERE transaction_id = $1",
        transaction_id
    )
    .fetch_one(pool)
    .await
    .expect("read in_master of the transaction row")
}

/// The rows of the view and of the base table: (view rows, live rows, all rows).
async fn transaction_counts(pool: &PgPool) -> (i64, i64, i64) {
    let row = sqlx::query!(
        r#"SELECT
    (SELECT count(*) FROM transaction_master) AS "view!",
    (SELECT count(*) FROM transactions WHERE in_master) AS "live!",
    (SELECT count(*) FROM transactions) AS "all!""#
    )
    .fetch_one(pool)
    .await
    .expect("count the transaction rows");
    (row.view, row.live, row.all)
}

/// Inserts a reward balance of `account_id` with the three point totals.
async fn insert_balance(
    pool: &PgPool,
    account_id: i64,
    (earned, redeemed, available): (i64, i64, i64),
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "INSERT INTO reward_balances (
    account_id, total_points_earned, total_points_redeemed, available_points)
VALUES ($1, $2, $3, $4)",
        account_id,
        earned,
        redeemed,
        available
    )
    .execute(pool)
    .await
    .map(drop)
}

/// Inserts an open-ended `01`/`0001` reward rule active from 2022-01-01, the
/// window of the seed rules, and returns its `rule_id`.
async fn insert_rule(
    pool: &PgPool,
    reward_rate: Decimal,
    (min_amount, max_amount): (Option<Decimal>, Option<Decimal>),
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        "INSERT INTO reward_rules (
    transaction_type_cd, transaction_category_cd, reward_rate, min_transaction_amount,
    max_transaction_amount, active_from, active_to)
VALUES ('01', 1, $1, $2, $3, '2022-01-01 00:00:00+00', NULL)
RETURNING rule_id",
        reward_rate,
        min_amount,
        max_amount
    )
    .fetch_one(pool)
    .await
}

/// Inserts an accrual of `points` for the transaction row `transaction_id`
/// under `rule_id`, stamped at the pinned processing clock of the batch tests.
async fn insert_accrual(
    pool: &PgPool,
    transaction_id: i64,
    rule_id: i64,
    points: i64,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
        "INSERT INTO reward_accruals (account_id, transaction_id, rule_id, points_earned, created_at)
VALUES ($1, $2, $3, $4, '2022-07-01 12:00:00+00')
RETURNING accrual_id",
        ACCOUNT_ID,
        transaction_id,
        rule_id,
        points
    )
    .fetch_one(pool)
    .await
}

/// Deletes the transaction row `transaction_id`.
async fn delete_transaction(pool: &PgPool, transaction_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "DELETE FROM transactions WHERE transaction_id = $1",
        transaction_id
    )
    .execute(pool)
    .await
    .map(drop)
}

// ---------------------------------------------------------------------------------------
// Assertions
// ---------------------------------------------------------------------------------------

/// Asserts that `result` failed in the database with SQLSTATE `sqlstate` on the
/// constraint or unique index `constraint`; `what` names the attempted write.
fn assert_violation<T>(
    result: Result<T, sqlx::Error>,
    sqlstate: &str,
    constraint: &str,
    what: &str,
) {
    let Err(error) = result else {
        panic!("{what} succeeded; expected SQLSTATE {sqlstate} on {constraint}");
    };
    let Some(database) = error.as_database_error() else {
        panic!("{what} failed outside the database: {error}");
    };
    assert_eq!(
        (database.code().as_deref(), database.constraint()),
        (Some(sqlstate), Some(constraint)),
        "{what}: {error}"
    );
}

/// The items of `expected` that `actual` lacks (missing) and the items of
/// `actual` that `expected` lacks (unexpected), each in ascending order.
fn set_difference<'a, T: Ord>(
    expected: &'a BTreeSet<T>,
    actual: &'a BTreeSet<T>,
) -> (Vec<&'a T>, Vec<&'a T>) {
    (
        expected.difference(actual).collect(),
        actual.difference(expected).collect(),
    )
}

/// Asserts that `actual` equals `expected`, listing what is missing and what is
/// unexpected; `what` names the compared items.
fn assert_same_set<T: Ord + Debug>(what: &str, expected: &BTreeSet<T>, actual: &BTreeSet<T>) {
    let (missing, unexpected) = set_difference(expected, actual);
    assert!(
        missing.is_empty() && unexpected.is_empty(),
        "{what} differ from the migrations\nmissing: {missing:#?}\nunexpected: {unexpected:#?}"
    );
}

/// Asserts that `changed`, read after a schema change, differs from `expected`
/// by exactly `missing` and `unexpected`; `what` names the compared items and
/// the change.
fn assert_differences<T: Ord + Debug>(
    what: &str,
    (expected, changed): (&BTreeSet<T>, &BTreeSet<T>),
    missing: &BTreeSet<T>,
    unexpected: &BTreeSet<T>,
) {
    let wanted: (Vec<&T>, Vec<&T>) = (missing.iter().collect(), unexpected.iter().collect());
    assert_eq!(
        set_difference(expected, changed),
        wanted,
        "{what}: (missing, unexpected)"
    );
}

/// The columns of `columns` sorted by a collation other than `C`, besides
/// those of [`NOT_C_COLUMNS`].
fn columns_not_in_c(columns: &BTreeSet<ColumnCollation>) -> BTreeSet<&ColumnCollation> {
    columns
        .iter()
        .filter(|column| {
            column.collation != C_COLLATION
                && !NOT_C_COLUMNS.contains(&(column.table.as_str(), column.column.as_str()))
        })
        .collect()
}

/// The keys of `keys` that have a collation other than `C`.
fn keys_not_in_c(keys: &BTreeSet<KeyCollation>) -> BTreeSet<&KeyCollation> {
    keys.iter()
        .filter(|key| {
            key.collation
                .as_deref()
                .is_some_and(|collation| collation != C_COLLATION)
        })
        .collect()
}

/// Asserts that `applied` records exactly the five embedded migrations, each
/// successful and with the checksum of the file the migrator embeds.
fn assert_matches_migrator(applied: &[AppliedMigration]) {
    let recorded: Vec<(i64, &str)> = applied
        .iter()
        .map(|migration| (migration.version, migration.description.as_str()))
        .collect();
    assert_eq!(
        recorded, MIGRATIONS,
        "versions recorded in _sqlx_migrations"
    );

    let embedded: Vec<(i64, &str)> = migrate::MIGRATOR
        .iter()
        .map(|migration| (migration.version, migration.description.as_ref()))
        .collect();
    assert_eq!(embedded, MIGRATIONS, "versions embedded by sqlx::migrate!");

    for (migration, embedded) in applied.iter().zip(migrate::MIGRATOR.iter()) {
        assert!(
            migration.success,
            "migration {} is recorded as failed",
            migration.version
        );
        assert_eq!(
            migration.checksum.as_slice(),
            embedded.checksum.as_ref(),
            "checksum of migration {}",
            migration.version
        );
    }
}

/// The URL of `database` on the server of `url`, which ends in `/<database>`.
fn sibling_database_url(url: &str, database: &str) -> String {
    let (server, _) = url
        .rsplit_once('/')
        .unwrap_or_else(|| panic!("the stack's database URL has no database segment: {url}"));
    format!("{server}/{database}")
}

// ---------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------

/// The migrator applies all five migrations to an empty database, the
/// `batch migrate` step that replaces the `CREADB21` DDL and the IDCAMS
/// `DEFINE` jobs, and a second run applies nothing: `migrate::run` and
/// `migrate::run_on` are idempotent on a migrated database, and the stack's
/// database, migrated by the testkit, holds the same versions and relations.
#[test]
fn migrations_apply_and_are_idempotent() {
    let stack = block_on(TestStack::postgres());
    block_on(stack.exec_sql("CREATE DATABASE carddemo_first_apply"));
    let fresh_url = sibling_database_url(stack.database_url(), FRESH_DATABASE);
    let fresh = block_on(connect(&fresh_url)).expect("connect to the empty database");
    assert!(
        !block_on(migration_table_exists(fresh.pool())),
        "the new database must start without _sqlx_migrations"
    );

    block_on(migrate::run(&fresh)).expect("apply the migrations to the empty database");
    let first_apply = block_on(applied_migrations(fresh.pool()));
    assert_matches_migrator(&first_apply);
    block_on(migrate::run_on(fresh.pool())).expect("re-run the migrations through run_on");
    block_on(migrate::run(&fresh)).expect("re-run the migrations through run");
    assert_eq!(
        block_on(applied_migrations(fresh.pool())),
        first_apply,
        "a second run must apply nothing"
    );
    let fresh_relations = block_on(relation_kinds(fresh.pool()));
    block_on(fresh.pool().close());

    let stack_db = Db::from_pool(stack.pool().clone());
    let testkit_apply = block_on(applied_migrations(stack.pool()));
    assert_matches_migrator(&testkit_apply);
    block_on(migrate::run(&stack_db)).expect("re-run the migrations on the stack through run");
    block_on(migrate::run_on(stack.pool()))
        .expect("re-run the migrations on the stack through run_on");
    assert_eq!(
        block_on(applied_migrations(stack.pool())),
        testkit_apply,
        "re-running on the testkit's migrated database must apply nothing"
    );
    assert_eq!(
        block_on(relation_kinds(stack.pool())),
        fresh_relations,
        "both databases must hold the same tables and views"
    );
}

/// The schema holds exactly the 18 base tables of the five migrations,
/// `_sqlx_migrations`, and the view `transaction_master`, the live `TRANSACT`
/// file of E-09 (`migrations/0003_transactions.sql`).
#[test]
fn tables_and_view_exist() {
    let stack = block_on(TestStack::postgres());
    let actual: BTreeSet<(String, String)> =
        block_on(relation_kinds(stack.pool())).into_iter().collect();
    let expected: BTreeSet<(String, String)> = BASE_TABLES
        .iter()
        .chain(["_sqlx_migrations"].iter())
        .map(|&table| (table.to_owned(), "BASE TABLE".to_owned()))
        .chain([("transaction_master".to_owned(), "VIEW".to_owned())])
        .collect();
    assert_same_set("tables and views", &expected, &actual);
}

/// The schema holds exactly five foreign keys, with the delete rules of their
/// sources: the Db2 `ON DELETE RESTRICT` of the categories on their type
/// (`app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L9-10`), the cascade of the
/// IMS child segment `PAUTDTL1` with its root `PAUTSUM0`, and the three reward
/// keys of AAP 0.3.5, `reward_accruals.transaction_id` with `ON DELETE
/// RESTRICT` (E-09). No table replacing a VSAM file declares one, the
/// processing copies `trantype_file` and `trancatg_file` included.
#[test]
fn foreign_keys_and_delete_rules() {
    let stack = block_on(TestStack::postgres());
    let actual = block_on(foreign_keys(stack.pool()));
    let expected = BTreeSet::from([
        ForeignKey::new(
            "transaction_categories_trc_type_code_fkey",
            ("transaction_categories", &["trc_type_code"]),
            ("transaction_types", &["tr_type"]),
            "RESTRICT",
        ),
        ForeignKey::new(
            "pending_auth_details_pa_acct_id_fkey",
            ("pending_auth_details", &["pa_acct_id"]),
            ("pending_auth_summaries", &["pa_acct_id"]),
            "CASCADE",
        ),
        ForeignKey::new(
            "reward_balances_account_id_fkey",
            ("reward_balances", &["account_id"]),
            ("accounts", &["account_id"]),
            "NO ACTION",
        ),
        ForeignKey::new(
            "reward_accruals_transaction_id_fkey",
            ("reward_accruals", &["transaction_id"]),
            ("transactions", &["transaction_id"]),
            "RESTRICT",
        ),
        ForeignKey::new(
            "reward_accruals_rule_id_fkey",
            ("reward_accruals", &["rule_id"]),
            ("reward_rules", &["rule_id"]),
            "NO ACTION",
        ),
    ]);
    let from_vsam: Vec<&ForeignKey> = actual
        .iter()
        .filter(|key| VSAM_TABLES.contains(&key.table.as_str()))
        .collect();
    assert!(
        from_vsam.is_empty(),
        "tables replacing VSAM files declare foreign keys: {from_vsam:#?}"
    );
    let to_copies: Vec<&ForeignKey> = actual
        .iter()
        .filter(|key| ["trantype_file", "trancatg_file"].contains(&key.referenced_table.as_str()))
        .collect();
    assert!(
        to_copies.is_empty(),
        "foreign keys reference the VSAM processing copies: {to_copies:#?}"
    );
    assert_same_set("foreign keys", &expected, &actual);
}

/// The primary key of every base table, in [`BASE_TABLES`] order, with the
/// COBOL, Db2 or IMS key it replaces or the identity the AAP gives the table.
const EXPECTED_PRIMARY_KEYS: [ExpectedPrimaryKey; 18] = [
    ExpectedPrimaryKey {
        table: "transaction_types",
        name: "xtran_type",
        columns: &["tr_type"],
        replaces: "TRANSACTION_TYPE PRIMARY KEY(TR_TYPE) (TRNTYPE.ddl:L4)",
    },
    ExpectedPrimaryKey {
        table: "transaction_categories",
        name: "x_tran_type_catg",
        columns: &["trc_type_code", "trc_type_category"],
        replaces: "TRANSACTION_TYPE_CATEGORY PRIMARY KEY(TRC_TYPE_CODE,TRC_TYPE_CATEGORY) \
                   (TRNTYCAT.ddl:L5)",
    },
    ExpectedPrimaryKey {
        table: "trantype_file",
        name: "trantype_file_pkey",
        columns: &["tran_type"],
        replaces: "TRANTYPE KSDS KEYS(2 0), TRAN-TYPE (TRANTYPE.jcl:L40)",
    },
    ExpectedPrimaryKey {
        table: "trancatg_file",
        name: "trancatg_file_pkey",
        columns: &["tran_type_cd", "tran_cat_cd"],
        replaces: "TRANCATG KSDS KEYS(6 0), TRAN-CAT-KEY (TRANCATG.jcl:L40)",
    },
    ExpectedPrimaryKey {
        table: "disclosure_groups",
        name: "disclosure_groups_pkey",
        columns: &["dis_acct_group_id", "dis_tran_type_cd", "dis_tran_cat_cd"],
        replaces: "DISCGRP KSDS KEYS(16 0), DIS-GROUP-KEY (DISCGRP.jcl:L40)",
    },
    ExpectedPrimaryKey {
        table: "customers",
        name: "customers_pkey",
        columns: &["customer_id"],
        replaces: "CUSTDAT KSDS KEYS(9 0), CUST-ID (CUSTFILE.jcl:L50)",
    },
    ExpectedPrimaryKey {
        table: "accounts",
        name: "accounts_pkey",
        columns: &["account_id"],
        replaces: "ACCTDAT KSDS KEYS(11 0), ACCT-ID (ACCTFILE.jcl:L40)",
    },
    ExpectedPrimaryKey {
        table: "cards",
        name: "cards_pkey",
        columns: &["card_number"],
        replaces: "CARDDAT KSDS KEYS(16 0), CARD-NUM (CARDFILE.jcl:L54)",
    },
    ExpectedPrimaryKey {
        table: "card_xref",
        name: "card_xref_pkey",
        columns: &["xref_card_num"],
        replaces: "CCXREF KSDS KEYS(16 0), XREF-CARD-NUM (XREFFILE.jcl:L43)",
    },
    ExpectedPrimaryKey {
        table: "users",
        name: "users_pkey",
        columns: &["user_id"],
        replaces: "USRSEC KSDS KEYS(8,0), SEC-USR-ID (DUSRSECJ.jcl:L65)",
    },
    ExpectedPrimaryKey {
        table: "transactions",
        name: "transactions_pkey",
        columns: &["transaction_id"],
        replaces: "the row identity of E-09, the reward accrual foreign key target (AAP 0.5.3.4)",
    },
    ExpectedPrimaryKey {
        table: "transaction_category_balances",
        name: "transaction_category_balances_pkey",
        columns: &["trancat_acct_id", "trancat_type_cd", "trancat_cd"],
        replaces: "TCATBALF KSDS KEYS(17 0), TRAN-CAT-KEY (TCATBALF.jcl:L40)",
    },
    ExpectedPrimaryKey {
        table: "pending_auth_summaries",
        name: "pending_auth_summaries_pkey",
        columns: &["pa_acct_id"],
        replaces: "PAUTSUM0 root sequence field ACCNTID, SEQ,U (DBPAUTP0.dbd:L30)",
    },
    ExpectedPrimaryKey {
        table: "pending_auth_details",
        name: "pending_auth_details_pkey",
        columns: &["auth_id"],
        replaces: "the identity REST key of /authorizations/{id} (AAP 0.5.3.4)",
    },
    ExpectedPrimaryKey {
        table: "auth_fraud",
        name: "auth_fraud_pkey",
        columns: &["card_num", "auth_ts"],
        replaces: "AUTHFRDS PRIMARY KEY(CARD_NUM,AUTH_TS) (AUTHFRDS.ddl:L28)",
    },
    ExpectedPrimaryKey {
        table: "reward_rules",
        name: "reward_rules_pkey",
        columns: &["rule_id"],
        replaces: "the rule identity, the lowest of which wins a tie of equal rates (AAP 0.3.5)",
    },
    ExpectedPrimaryKey {
        table: "reward_balances",
        name: "reward_balances_pkey",
        columns: &["account_id"],
        replaces: "the one reward balance of an account (AAP 0.3.5)",
    },
    ExpectedPrimaryKey {
        table: "reward_accruals",
        name: "reward_accruals_pkey",
        columns: &["accrual_id"],
        replaces: "the accrual identity, the reward history page key (AAP 0.3.5)",
    },
];

/// The four identity columns, each `BIGINT GENERATED ALWAYS AS IDENTITY` with
/// the default sequence options, so no writer supplies a value and every value
/// is new.
const EXPECTED_IDENTITY_COLUMNS: [ExpectedIdentity; 4] = [
    ExpectedIdentity {
        table: "transactions",
        column: "transaction_id",
        source: "is the row identity no writer supplies; COMBTRAN revives a row by \
                 updating it, never by inserting its old identity \
                 (migrations/0003_transactions.sql:L126-129, AAP 0.5.3.4)",
    },
    ExpectedIdentity {
        table: "pending_auth_details",
        column: "auth_id",
        source: "is the generated REST key that is never reused \
                 (migrations/0004_authorizations.sql:L156, L246, AAP 0.5.3.4)",
    },
    ExpectedIdentity {
        table: "reward_rules",
        column: "rule_id",
        source: "is an identity no writer supplies, which the seed refresh's RESTART \
                 IDENTITY restarts at 1 (migrations/0005_rewards.sql:L45-47, AAP 0.3.5)",
    },
    ExpectedIdentity {
        table: "reward_accruals",
        column: "accrual_id",
        source: "is an identity no writer supplies (migrations/0005_rewards.sql:L45, \
                 AAP 0.3.5)",
    },
];

/// Every base table has the primary key of [`EXPECTED_PRIMARY_KEYS`], with its
/// columns in key order, and the four columns of [`EXPECTED_IDENTITY_COLUMNS`]
/// are the only identities, `GENERATED ALWAYS` from 1 by 1 without cycling.
/// No foreign key references `customers`, `cards` or `users`, so nothing else
/// would notice their keys missing. Dropping those three keys, reordering a
/// composite one, relaxing an identity to `BY DEFAULT` and dropping another
/// identity are then each reported exactly.
#[test]
fn primary_keys_and_identity_columns() {
    let stack = block_on(TestStack::postgres());
    let keyed: BTreeSet<&str> = EXPECTED_PRIMARY_KEYS.iter().map(|key| key.table).collect();
    assert_eq!(
        keyed,
        BTreeSet::from(BASE_TABLES),
        "tables with an expected primary key"
    );

    let keys = block_on(primary_keys(stack.pool()));
    for expected in &EXPECTED_PRIMARY_KEYS {
        let on_table: Vec<&PrimaryKey> = keys
            .iter()
            .filter(|key| key.table == expected.table)
            .collect();
        assert_eq!(
            on_table,
            [&expected.key()],
            "primary key of {}, which replaces {}",
            expected.table,
            expected.replaces
        );
    }
    let expected_keys: BTreeSet<PrimaryKey> = EXPECTED_PRIMARY_KEYS
        .iter()
        .map(ExpectedPrimaryKey::key)
        .collect();
    assert_same_set("primary keys", &expected_keys, &keys);

    let identities = block_on(identity_columns(stack.pool()));
    for expected in &EXPECTED_IDENTITY_COLUMNS {
        let on_column: Vec<&IdentityColumn> = identities
            .iter()
            .filter(|identity| {
                identity.table == expected.table && identity.column == expected.column
            })
            .collect();
        assert_eq!(
            on_column,
            [&expected.identity()],
            "identity {}.{}, which {}",
            expected.table,
            expected.column,
            expected.source
        );
    }
    let expected_identities: BTreeSet<IdentityColumn> = EXPECTED_IDENTITY_COLUMNS
        .iter()
        .map(ExpectedIdentity::identity)
        .collect();
    assert_same_set("identity columns", &expected_identities, &identities);

    check_key_and_identity_changes(&stack, &expected_keys, &expected_identities);
}

/// Drops the primary keys of `customers`, `cards` and `users`, none of which a
/// foreign key references, recreates `disclosure_groups_pkey` over its columns
/// in another order, makes `transactions.transaction_id` `GENERATED BY DEFAULT`
/// and drops the identity of `reward_rules.rule_id`; the catalog reads must
/// report exactly these changes against `expected_keys` and
/// `expected_identities`.
fn check_key_and_identity_changes(
    stack: &TestStack,
    expected_keys: &BTreeSet<PrimaryKey>,
    expected_identities: &BTreeSet<IdentityColumn>,
) {
    block_on(stack.exec_sql(
        "ALTER TABLE customers DROP CONSTRAINT customers_pkey;
ALTER TABLE cards DROP CONSTRAINT cards_pkey;
ALTER TABLE users DROP CONSTRAINT users_pkey;
ALTER TABLE disclosure_groups
    DROP CONSTRAINT disclosure_groups_pkey,
    ADD CONSTRAINT disclosure_groups_pkey
        PRIMARY KEY (dis_tran_type_cd, dis_acct_group_id, dis_tran_cat_cd);
ALTER TABLE transactions ALTER COLUMN transaction_id SET GENERATED BY DEFAULT;
ALTER TABLE reward_rules ALTER COLUMN rule_id DROP IDENTITY;",
    ));

    let removed: BTreeSet<PrimaryKey> = EXPECTED_PRIMARY_KEYS
        .iter()
        .filter(|key| ["customers", "cards", "users", "disclosure_groups"].contains(&key.table))
        .map(ExpectedPrimaryKey::key)
        .collect();
    let reordered = PrimaryKey::new(
        "disclosure_groups",
        "disclosure_groups_pkey",
        &["dis_tran_type_cd", "dis_acct_group_id", "dis_tran_cat_cd"],
    );
    assert_differences(
        "primary keys after dropping customers_pkey, cards_pkey and users_pkey and \
         reordering disclosure_groups_pkey",
        (expected_keys, &block_on(primary_keys(stack.pool()))),
        &removed,
        &BTreeSet::from([reordered]),
    );

    let by_default = IdentityColumn {
        generation: "BY DEFAULT".to_owned(),
        ..IdentityColumn::always("transactions", "transaction_id")
    };
    assert_differences(
        "identity columns after making transactions.transaction_id GENERATED BY DEFAULT \
         and dropping the identity of reward_rules.rule_id",
        (
            expected_identities,
            &block_on(identity_columns(stack.pool())),
        ),
        &BTreeSet::from([
            IdentityColumn::always("transactions", "transaction_id"),
            IdentityColumn::always("reward_rules", "rule_id"),
        ]),
        &BTreeSet::from([by_default]),
    );
}

/// The indexes that replace the keys and alternate indexes of the COBOL
/// files and tables, checked on their key lists, uniqueness and predicates.
const EXPECTED_INDEXES: [ExpectedIndex; 18] = [
    ExpectedIndex {
        table: "transaction_types",
        name: "xtran_type",
        keys: "(tr_type)",
        unique: true,
        primary: true,
        predicate: None,
        replaces: "PRIMARY KEY(TR_TYPE), index XTRAN_TYPE (TRNTYPE.ddl:L4)",
    },
    ExpectedIndex {
        table: "transaction_types",
        name: "transaction_types_padded_tr_type",
        keys: "(rpad((tr_type)::text, 2))",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "the ORDER BY TR_TYPE of the COTRTLIC cursors C-TR-TYPE-FORWARD and \
                   C-TR-TYPE-BACKWARD, TR_TYPE CHAR(2) padded to 2 (COTRTLIC.cbl:L338-367)",
    },
    ExpectedIndex {
        table: "transaction_categories",
        name: "x_tran_type_catg",
        keys: "(trc_type_code, trc_type_category)",
        unique: true,
        primary: true,
        predicate: None,
        replaces: "PRIMARY KEY(TRC_TYPE_CODE,TRC_TYPE_CATEGORY), index X_TRAN_TYPE_CATG (TRNTYCAT.ddl:L8)",
    },
    ExpectedIndex {
        table: "cards",
        name: "cardaix",
        keys: "(card_acct_id)",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "CARDAIX, AIX KEYS(11 16) NONUNIQUEKEY (CARDFILE.jcl:L83-86)",
    },
    ExpectedIndex {
        table: "card_xref",
        name: "cxacaix",
        keys: "(xref_acct_id)",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "CXACAIX, AIX KEYS(11,25) NONUNIQUEKEY (XREFFILE.jcl:L72-75)",
    },
    // The definitions of the three padded-key indexes (transaction_types_padded_tr_type
    // above, users_padded_user_id and transaction_category_balances_padded_key) print no
    // COLLATE. pg_indexes prints a key's COLLATE only where it differs from the collation
    // its expression takes from the column, so these key lists cannot tell "C" from any
    // other collation the column and the index share. text_columns_and_keys_sort_by_c_collation
    // reads the column and index-key collations from the catalogs and pins "C".
    ExpectedIndex {
        table: "users",
        name: "users_padded_user_id",
        keys: "(rpad((user_id)::text, 8))",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "USRSEC KSDS KEYS(8,0) browse order, SEC-USR-ID padded to 8 (DUSRSECJ.jcl:L64-66)",
    },
    ExpectedIndex {
        table: "transactions",
        name: "transactions_pkey",
        keys: "(transaction_id)",
        unique: true,
        primary: true,
        predicate: None,
        replaces: "the row identity of E-09, the reward accrual foreign key target",
    },
    ExpectedIndex {
        table: "transactions",
        name: "transactions_master_tran_id",
        keys: "(tran_id)",
        unique: true,
        primary: false,
        predicate: Some("in_master"),
        replaces: "TRANSACT KSDS KEYS(16 0) over the live rows (TRANFILE.jcl:L53, E-09)",
    },
    ExpectedIndex {
        table: "transactions",
        name: "transactions_master_proc_ts",
        keys: "(tran_proc_ts)",
        unique: false,
        primary: false,
        predicate: Some("in_master"),
        replaces: "TRANSACT AIX KEYS(26 304) NONUNIQUEKEY (TRANFILE.jcl:L82-86, STEP20)",
    },
    ExpectedIndex {
        table: "transactions",
        name: "transactions_retained_tran_id",
        keys: "(tran_id, transaction_id)",
        unique: false,
        primary: false,
        predicate: Some("(NOT in_master)"),
        replaces: "the COMBTRAN revive lookup of a non-live row by TRAN-ID and content (E-09)",
    },
    ExpectedIndex {
        table: "transaction_category_balances",
        name: "transaction_category_balances_padded_key",
        keys: "(trancat_acct_id, rpad((trancat_type_cd)::text, 2), trancat_cd)",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "TCATBALF KSDS KEYS(17 0) sequential order, type code padded to X(02) \
                   (TCATBALF.jcl:L36-41)",
    },
    ExpectedIndex {
        table: "pending_auth_details",
        name: "pautdtl1_key",
        keys: "(pa_acct_id, pa_auth_date_9c, pa_auth_time_9c)",
        unique: true,
        primary: false,
        predicate: None,
        replaces: "PAUTDTL1 unique sequence field PAUT9CTS under its PAUTSUM0 root",
    },
    ExpectedIndex {
        table: "auth_fraud",
        name: "auth_fraud_pkey",
        keys: "(card_num, auth_ts)",
        unique: true,
        primary: true,
        predicate: None,
        replaces: "AUTHFRDS PRIMARY KEY(CARD_NUM,AUTH_TS) (AUTHFRDS.ddl:L28)",
    },
    ExpectedIndex {
        table: "auth_fraud",
        name: "xauthfrd",
        keys: "(card_num, auth_ts DESC)",
        unique: true,
        primary: false,
        predicate: None,
        replaces: "UNIQUE INDEX XAUTHFRD (CARD_NUM ASC, AUTH_TS DESC) (XAUTHFRD.ddl:L1-4)",
    },
    ExpectedIndex {
        table: "reward_rules",
        name: "reward_rules_match_idx",
        keys: "(transaction_type_cd, transaction_category_cd, active_from, active_to)",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "the candidate-rule read of the RewardEngine (AAP 0.3.5)",
    },
    ExpectedIndex {
        table: "reward_accruals",
        name: "reward_accruals_transaction_id_key",
        keys: "(transaction_id)",
        unique: true,
        primary: false,
        predicate: None,
        replaces: "one accrual per qualifying transaction (AAP 0.3.5)",
    },
    ExpectedIndex {
        table: "reward_accruals",
        name: "reward_accruals_account_created_idx",
        keys: "(account_id, created_at)",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "the reward history of one account (AAP 0.3.5)",
    },
    ExpectedIndex {
        table: "reward_accruals",
        name: "reward_accruals_account_created_accrual_idx",
        keys: "(account_id, created_at, accrual_id)",
        unique: false,
        primary: false,
        predicate: None,
        replaces: "the newest-first reward history page and its (created_at, accrual_id) cursor",
    },
];

/// Every index of [`EXPECTED_INDEXES`] exists on its table with its key list,
/// uniqueness and predicate. The `TRAN-ID` key holds among live rows only: of
/// the indexes on `transactions` led by `tran_id`, the partial live-row key
/// `transactions_master_tran_id` is the only unique one, and
/// `transactions_retained_tran_id`, which serves the COMBTRAN revive lookup
/// over the non-live rows, is not unique. A `TRAN-ID` held by non-live rows may
/// therefore be issued again, and several non-live rows may share it (E-09).
/// The key lists print no collation a key shares with its column, so
/// `text_columns_and_keys_sort_by_c_collation` checks the collations.
#[test]
fn indexes_exist_with_expected_shape() {
    let stack = block_on(TestStack::postgres());
    let actual = block_on(indexes(stack.pool()));
    for expected in &EXPECTED_INDEXES {
        let on_table: Vec<&IndexRow> = actual
            .iter()
            .filter(|index| index.table == expected.table)
            .collect();
        let Some(index) = on_table.iter().find(|index| index.name == expected.name) else {
            panic!(
                "{}: no index {} on {}, which replaces {}; the table has {on_table:#?}",
                expected.table, expected.name, expected.table, expected.replaces
            );
        };
        let shape = (
            index.key_list(),
            index.unique,
            index.primary,
            index.definition_predicate(),
            index.predicate.as_deref(),
        );
        let wanted = (
            expected.keys,
            expected.unique,
            expected.primary,
            expected.predicate,
            expected.predicate,
        );
        assert_eq!(
            shape, wanted,
            "{} (keys, unique, primary, definition predicate, catalog predicate), which \
             replaces {}: {}",
            expected.name, expected.replaces, index.definition
        );
    }
    let tran_id_led: Vec<(&str, bool, Option<&str>)> = actual
        .iter()
        .filter(|index| index.table == "transactions" && index.leading_key() == "tran_id")
        .map(|index| {
            (
                index.name.as_str(),
                index.unique,
                index.predicate.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        tran_id_led,
        [
            ("transactions_master_tran_id", true, Some("in_master")),
            (
                "transactions_retained_tran_id",
                false,
                Some("(NOT in_master)")
            ),
        ],
        "tran_id must be unique among live rows only: the partial live-row key is the one \
         unique index led by tran_id, and the retained-row index is not unique"
    );
}

/// The padded-key expressions of `transaction_types_padded_tr_type`,
/// `users_padded_user_id` and `transaction_category_balances_padded_key`, whose
/// byte order is the COBOL key order of their [`EXPECTED_INDEXES`] entries.
const PADDED_KEYS: [PaddedKey; 3] = [
    PaddedKey {
        table: "transaction_types",
        column: "tr_type",
        index: "transaction_types_padded_tr_type",
        position: 1,
        key: "rpad((tr_type)::text, 2)",
    },
    PaddedKey {
        table: "users",
        column: "user_id",
        index: "users_padded_user_id",
        position: 1,
        key: "rpad((user_id)::text, 8)",
    },
    PaddedKey {
        table: "transaction_category_balances",
        column: "trancat_type_cd",
        index: "transaction_category_balances_padded_key",
        position: 2,
        key: "rpad((trancat_type_cd)::text, 2)",
    },
];

/// Every text column of the tables and the view, and every text key of their
/// indexes, sorts by `C`, the byte order of the COBOL keys and string
/// comparisons (the conventions of `migrations/0001_reference_data.sql` and
/// `0002_master_data.sql`); only the columns of [`NOT_C_COLUMNS`] are exempt.
/// The columns and keys of [`PADDED_KEYS`] and the view's `tran_id` are among
/// those read, so the check is not vacuous. The collations come from
/// `pg_attribute` and `pg_index`: once `users.user_id` and its padded index
/// share the default collation, the key list `indexes_exist_with_expected_shape`
/// compares is unchanged, while these reads report the column and both of its
/// keys.
#[test]
fn text_columns_and_keys_sort_by_c_collation() {
    let stack = block_on(TestStack::postgres());
    let columns = block_on(column_collations(stack.pool()));
    let keys = block_on(index_key_collations(stack.pool()));
    for padded in &PADDED_KEYS {
        assert!(
            columns.contains(&ColumnCollation::new(
                padded.table,
                padded.column,
                C_COLLATION
            )),
            "{}.{}, padded by {}, must sort by C; the columns are {columns:#?}",
            padded.table,
            padded.column,
            padded.index
        );
        assert!(
            keys.contains(&padded.key_collation(C_COLLATION)),
            "key {} of {} must be {} sorting by C; the keys are {keys:#?}",
            padded.position,
            padded.index,
            padded.key
        );
    }
    assert!(
        columns.contains(&ColumnCollation::new(
            "transaction_master",
            "tran_id",
            C_COLLATION
        )),
        "the view's tran_id must sort by C; the columns are {columns:#?}"
    );
    let columns_off_c = columns_not_in_c(&columns);
    assert!(
        columns_off_c.is_empty(),
        "text columns sorted by a collation other than C: {columns_off_c:#?}"
    );
    let keys_off_c = keys_not_in_c(&keys);
    assert!(
        keys_off_c.is_empty(),
        "index keys sorted by a collation other than C: {keys_off_c:#?}"
    );

    check_shared_default_collation(&stack);
}

/// Gives `users.user_id` the default collation, which rebuilds `users_pkey`
/// under it, and recreates `users_padded_user_id` without `COLLATE`, so the
/// expression takes the column's collation. The key list of that index stays
/// the one [`EXPECTED_INDEXES`] pins, while the collation reads report exactly
/// the column and its two keys.
fn check_shared_default_collation(stack: &TestStack) {
    block_on(stack.exec_sql(
        r#"DROP INDEX users_padded_user_id;
ALTER TABLE users ALTER COLUMN user_id TYPE VARCHAR(8) COLLATE "default";
CREATE INDEX users_padded_user_id ON users ((rpad(user_id, 8)));"#,
    ));

    let expected = EXPECTED_INDEXES
        .iter()
        .find(|index| index.name == "users_padded_user_id")
        .expect("users_padded_user_id is an expected index");
    let rebuilt = block_on(indexes(stack.pool()));
    let index = rebuilt
        .iter()
        .find(|index| index.name == expected.name)
        .expect("users_padded_user_id exists after its rebuild");
    assert_eq!(
        index.key_list(),
        expected.keys,
        "{} under the default collation: {}",
        expected.name,
        index.definition
    );

    let padded = PADDED_KEYS
        .iter()
        .find(|key| key.index == expected.name)
        .expect("users_padded_user_id has a padded key");
    let columns = block_on(column_collations(stack.pool()));
    assert_eq!(
        columns_not_in_c(&columns),
        BTreeSet::from([&ColumnCollation::new("users", "user_id", DEFAULT_COLLATION)]),
        "text columns sorted by a collation other than C once users.user_id has the default"
    );
    let keys = block_on(index_key_collations(stack.pool()));
    assert_eq!(
        keys_not_in_c(&keys),
        BTreeSet::from([
            &KeyCollation::new(
                ("users", "users_pkey"),
                1,
                "user_id",
                Some(DEFAULT_COLLATION)
            ),
            &padded.key_collation(DEFAULT_COLLATION),
        ]),
        "index keys sorted by a collation other than C once users.user_id has the default"
    );
}

/// E-09 live master: the partial unique index `transactions_master_tran_id`
/// is the KSDS key `KEYS(16 0)` of `TRANSACT` (`app/jcl/TRANFILE.jcl:L53`) over
/// the rows with `in_master`. A second live row with a live `TRAN-ID` fails
/// with SQLSTATE `23505`, the `DUPKEY`/`DUPREC` of the COBOL writes; once the
/// first row is no longer live, as `batch tranbkp` leaves it, a new live row
/// takes the same `TRAN-ID`, and reviving the old row then fails. The view
/// `transaction_master` returns exactly the live rows.
#[test]
fn live_tran_id_is_unique_only_among_live_rows() {
    let stack = block_on(TestStack::postgres());
    let pool = stack.pool();

    let first = block_on(insert_transaction(pool, TRAN_ID)).expect("insert the first live row");
    assert!(
        block_on(is_live(pool, first)),
        "in_master must default to TRUE"
    );
    assert_violation(
        block_on(insert_transaction(pool, TRAN_ID)),
        "23505",
        "transactions_master_tran_id",
        "a second live row with the same TRAN-ID",
    );

    block_on(set_live(pool, first, false)).expect("take the first row out of the live master");
    let second = block_on(insert_transaction(pool, TRAN_ID))
        .expect("a live row may take the TRAN-ID of a non-live row");
    assert_ne!(first, second, "every row gets its own transaction_id");
    assert_violation(
        block_on(set_live(pool, first, true)),
        "23505",
        "transactions_master_tran_id",
        "reviving a non-live row whose TRAN-ID is live again",
    );

    let in_view = block_on(
        sqlx::query_scalar!(
            r#"SELECT transaction_id AS "transaction_id!" FROM transaction_master WHERE tran_id = $1"#,
            TRAN_ID
        )
        .fetch_all(pool),
    )
    .expect("read the TRAN-ID through transaction_master");
    assert_eq!(in_view, [second], "the view holds the live row only");
    assert_eq!(
        block_on(transaction_counts(pool)),
        (1, 1, 2),
        "(view rows, live rows, all rows)"
    );
}

/// The reward invariants of AAP 0.3.5 hold in the database for every writer:
/// `available_points = total_points_earned - total_points_redeemed` and its
/// non-negative bound, a balance only for an existing account, a non-negative
/// `reward_rate`, `min_transaction_amount <= max_transaction_amount`, one
/// accrual per transaction, and `ON DELETE RESTRICT` keeping an accrued
/// transaction.
#[test]
fn reward_constraints_reject_invalid_rows() {
    let stack = block_on(TestStack::postgres());
    let pool = stack.pool();
    block_on(insert_account(pool, ACCOUNT_ID));
    let transaction_id =
        block_on(insert_transaction(pool, TRAN_ID)).expect("insert the accrued transaction");

    check_balance_constraints(pool);
    let rule_id = check_rule_constraints(pool);

    // ceil(50.47 x 1.0000) points for the first dailytran.txt record.
    let points = 51;
    block_on(insert_accrual(pool, transaction_id, rule_id, points))
        .expect("insert the transaction's accrual");
    assert_violation(
        block_on(insert_accrual(pool, transaction_id, rule_id, points)),
        "23505",
        "reward_accruals_transaction_id_key",
        "a second accrual for the same transaction",
    );
    assert_violation(
        block_on(delete_transaction(pool, transaction_id)),
        "23503",
        "reward_accruals_transaction_id_fkey",
        "deleting an accrued transaction",
    );
    assert_eq!(
        block_on(transaction_counts(pool)),
        (1, 1, 1),
        "the accrued transaction must survive the refused delete"
    );
}

/// The `reward_balances` checks and foreign key, on the account of
/// `ACCOUNT_ID`.
fn check_balance_constraints(pool: &PgPool) {
    assert_violation(
        block_on(insert_balance(pool, ACCOUNT_ID, (10, 0, 9))),
        "23514",
        "reward_balances_available_points_check",
        "a balance whose available points are not earned minus redeemed",
    );
    assert_violation(
        block_on(insert_balance(pool, ACCOUNT_ID, (0, 1, -1))),
        "23514",
        "reward_balances_available_points_nonnegative_check",
        "a balance with negative available points",
    );
    assert_violation(
        block_on(insert_balance(pool, ACCOUNT_ID + 1, (0, 0, 0))),
        "23503",
        "reward_balances_account_id_fkey",
        "a balance of an account that does not exist",
    );
    block_on(insert_balance(pool, ACCOUNT_ID, (10, 0, 10))).expect("insert a consistent balance");
}

/// The `reward_rules` checks; returns the `rule_id` of the valid rule it
/// inserts, whose bounds are equal, as the inclusive range allows.
fn check_rule_constraints(pool: &PgPool) -> i64 {
    assert_violation(
        block_on(insert_rule(pool, Decimal::new(-1, 4), (None, None))),
        "23514",
        "reward_rules_reward_rate_check",
        "a rule with reward_rate -0.0001",
    );
    let fifty = Decimal::new(5_000, 2);
    let hundred = Decimal::new(10_000, 2);
    assert_violation(
        block_on(insert_rule(
            pool,
            Decimal::ONE,
            (Some(hundred), Some(fifty)),
        )),
        "23514",
        "reward_rules_amount_range_check",
        "a rule with min_transaction_amount above max_transaction_amount",
    );
    block_on(insert_rule(
        pool,
        Decimal::new(10_000, 4),
        (Some(fifty), Some(fifty)),
    ))
    .expect("insert a valid rule")
}

/// No column of the schema has a floating-point type (AAP 0.10.3), and the
/// money, rate and points columns carry the precision and scale of their PIC
/// clauses or of the reward model (AAP 0.5.3.1): `S9(10)V99` as
/// `numeric(12,2)` (`app/cpy/CVACT01Y.cpy:L7-9, L13-14`), `S9(09)V99` as
/// `numeric(11,2)` (`app/cpy/CVTRA05Y.cpy:L10`, `app/cpy/CVTRA01Y.cpy:L9`),
/// `S9(04)V99` as `numeric(6,2)` (`app/cpy/CVTRA02Y.cpy:L9`), the reward rate
/// as `numeric(9,4)` and reward points as `bigint`.
#[test]
fn column_types_are_exact_decimals() {
    let stack = block_on(TestStack::postgres());
    let floats = block_on(float_columns(stack.pool()));
    assert!(floats.is_empty(), "floating-point columns: {floats:#?}");

    let money_12_2 = [
        "acct_curr_bal",
        "acct_credit_limit",
        "acct_cash_credit_limit",
        "acct_curr_cyc_credit",
        "acct_curr_cyc_debit",
    ]
    .map(|column| (("accounts", column), ColumnType::numeric(12, 2)));
    let points = [
        ("reward_balances", "total_points_earned"),
        ("reward_balances", "total_points_redeemed"),
        ("reward_balances", "available_points"),
        ("reward_accruals", "points_earned"),
    ]
    .map(|key| (key, ColumnType::bigint()));
    let others = [
        (("transactions", "tran_amt"), ColumnType::numeric(11, 2)),
        (
            ("transaction_category_balances", "tran_cat_bal"),
            ColumnType::numeric(11, 2),
        ),
        (
            ("disclosure_groups", "dis_int_rate"),
            ColumnType::numeric(6, 2),
        ),
        (("reward_rules", "reward_rate"), ColumnType::numeric(9, 4)),
        (
            ("reward_rules", "min_transaction_amount"),
            ColumnType::numeric(11, 2),
        ),
        (
            ("reward_rules", "max_transaction_amount"),
            ColumnType::numeric(11, 2),
        ),
    ];

    let actual = block_on(column_types(stack.pool()));
    for ((table, column), expected) in money_12_2.into_iter().chain(points).chain(others) {
        let key = (table.to_owned(), column.to_owned());
        assert_eq!(
            actual.get(&key),
            Some(&expected),
            "type of {table}.{column}"
        );
    }
}
