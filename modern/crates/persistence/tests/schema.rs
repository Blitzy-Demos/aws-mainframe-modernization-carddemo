//! The migrated `PostgreSQL` schema against the copybook column rules: the schema half of d9
//! (AAP 0.3.3.4, 0.3.3.5) and the schema proof of Typed Dates and Timestamps.
//!
//! The expected tables below are written from the copybooks and the Db2 DDL, through the rows of
//! `modern/audit/copybooks.md`, never from the migrations, so a migration that drifts from a
//! picture rule fails here with its table and column named:
//!
//! - `PIC X(n)` and every `FILLER` are `CHAR(n)`; the five `X(10)` dates are nullable `DATE` and
//!   the two `X(26)` timestamps nullable `TIMESTAMP(6)`, each with a nullable `CHAR(1)` layout
//!   code (E33), `NULL` standing for an all-space value;
//! - `PIC 9(n)` and `PIC S9(n)` are `BIGINT` with a range check of n nines, compared by its
//!   complete definition (Checked Integer Narrowing), `PIC S9(n)V9(m)` is `NUMERIC(n+m, m)`,
//!   `PIC S9(4) COMP` is `SMALLINT`;
//! - `users.password_hash` (`TEXT`) takes the place of `SEC-USR-PWD` (E25);
//! - Db2 columns keep their declared types, with `DECIMAL(n)` as a signed `BIGINT`.
//!
//! Keys and indexes follow the KSDS keys, the alternate indexes, the IMS hierarchy and the Db2 DDL.
//! Every check constraint, the timestamp layout checks of `transactions` included, is compared by
//! its complete definition, so a check joined by `OR` or carrying an extra predicate fails with its
//! table and name. A third test drives the constraints through the repository traits and checked
//! statements, both mismatched `NULL` states of each timestamp pair included.
//!
//! Every database comes migrated from the `test-support` harness, and every statement is a
//! compile-time-checked query macro (R4). Failure messages name schema objects, layout codes and
//! fixture files, never a stored value (Cardholder Data Redaction).

use std::collections::{BTreeMap, BTreeSet};

use copybook::{PicS, PicU, PicX};
use domain::batch::loads::decode_fixture;
use domain::records::{
    AccountRecord, Db2TransactionType, PendingAuthDetailRecord, PendingAuthSummaryRecord,
};
use domain::{CardNumber, Money};
use persistence::{
    AccountRepository, Db2TransactionTypeRepository, PendingAuthDetailRepository,
    PendingAuthSummaryRepository, PersistenceError, PgStore, TransactionTypeRepository,
};
use sqlx::PgPool;
use sqlx::postgres::PgQueryResult;
use test_support::fixtures::fixture_path;
use test_support::postgres::start_postgres;

use Ty::{
    Char, Date, Identity, Numeric, Serial, Signed, SmallInt, Text, Timestamp, Unsigned, Varchar,
};

/// A column type as a copybook picture or a Db2 declaration fixes it.
#[derive(Clone, Copy)]
enum Ty {
    /// `PIC X(n)`, `FILLER PIC X(n)` and Db2 `CHAR(n)`: `character(n)`.
    Char(i32),
    /// Db2 `VARCHAR(n)`: `character varying(n)`.
    Varchar(i32),
    /// `PIC S9(p − s)V9(s)`, DISPLAY or `COMP-3`, and Db2 `DECIMAL(p,s)`: `numeric(p,s)`.
    Numeric(i32, i32),
    /// `PIC 9(n)`: `bigint` checked to lie between 0 and n nines.
    Unsigned(usize),
    /// `PIC S9(n)` and Db2 `DECIMAL(n)`: `bigint` checked to lie between minus n nines and n
    /// nines.
    Signed(usize),
    /// `cards.card_ref`, no copybook field: `bigint` generated always as identity.
    Identity,
    /// `cssl_error_log.id`, no copybook field: `bigserial`.
    Serial,
    /// `PIC S9(4) COMP` and Db2 `SMALLINT`: `smallint`.
    SmallInt,
    /// `PIC X(10)` date and Db2 `DATE`: `date`.
    Date,
    /// `PIC X(26)` timestamp and Db2 `TIMESTAMP`: `timestamp(6) without time zone`.
    Timestamp,
    /// The argon2id hash in place of `SEC-USR-PWD`: `text`.
    Text,
}

/// One expected column: its name as the migrations spell it, its type and whether it admits
/// `NULL`.
#[derive(Clone, Copy)]
struct Col {
    name: &'static str,
    ty: Ty,
    nullable: bool,
}

/// A column that always holds a value: every copybook field but the blank-as-`NULL` dates and
/// timestamps, and every Db2 `NOT NULL` column.
const fn col(name: &'static str, ty: Ty) -> Col {
    Col {
        name,
        ty,
        nullable: false,
    }
}

/// A column that admits `NULL`: a date or timestamp whose all-space value is stored as `NULL`
/// (PIC-015, PIC-017), its layout code, or a Db2 column declared without `NOT NULL`.
const fn nullable(name: &'static str, ty: Ty) -> Col {
    Col {
        name,
        ty,
        nullable: true,
    }
}

/// One expected table and its complete column list.
struct Table {
    schema: &'static str,
    name: &'static str,
    columns: &'static [Col],
}

/// Every table of the schema. sqlx's own `_sqlx_migrations` is listed in [`OTHER_TABLES`].
const TABLES: [Table; 16] = [
    table("accounts", ACCOUNTS),
    table("customers", CUSTOMERS),
    table("cards", CARDS),
    table("card_xref", CARD_XREF),
    table("transaction_types", TRANSACTION_TYPES),
    table("transaction_categories", TRANSACTION_CATEGORIES),
    table("disclosure_groups", DISCLOSURE_GROUPS),
    table("tran_cat_balances", TRAN_CAT_BALANCES),
    table("transactions", TRANSACTIONS),
    table("users", USERS),
    table("pending_auth_summaries", PENDING_AUTH_SUMMARIES),
    table("pending_auth_details", PENDING_AUTH_DETAILS),
    table("auth_fraud", AUTH_FRAUD),
    table("cssl_error_log", CSSL_ERROR_LOG),
    Table {
        schema: "db2",
        name: "transaction_type",
        columns: DB2_TRANSACTION_TYPE,
    },
    Table {
        schema: "db2",
        name: "transaction_type_category",
        columns: DB2_TRANSACTION_TYPE_CATEGORY,
    },
];

/// The tables the schema holds besides [`TABLES`], whose columns are not this file's to check.
const OTHER_TABLES: [(&str, &str); 1] = [("public", "_sqlx_migrations")];

/// The schemas besides `PostgreSQL`'s own: `public`, and `db2` for the Db2 data version (E23).
const SCHEMAS: [&str; 2] = ["db2", "public"];

/// A table of schema `public`.
const fn table(name: &'static str, columns: &'static [Col]) -> Table {
    Table {
        schema: "public",
        name,
        columns,
    }
}

/// `ACCOUNT-RECORD` (`CVACT01Y`), 300 bytes.
const ACCOUNTS: &[Col] = &[
    // audit: CVACT01Y-001
    col("acct_id", Unsigned(11)),
    // audit: CVACT01Y-002
    col("acct_active_status", Char(1)),
    // audit: CVACT01Y-003
    col("acct_curr_bal", Numeric(12, 2)),
    // audit: CVACT01Y-004
    col("acct_credit_limit", Numeric(12, 2)),
    // audit: CVACT01Y-005
    col("acct_cash_credit_limit", Numeric(12, 2)),
    // audit: CVACT01Y-006
    nullable("acct_open_date", Date),
    // audit: CVACT01Y-007
    nullable("acct_expiraion_date", Date),
    // audit: CVACT01Y-008
    nullable("acct_reissue_date", Date),
    // audit: CVACT01Y-009
    col("acct_curr_cyc_credit", Numeric(12, 2)),
    // audit: CVACT01Y-010
    col("acct_curr_cyc_debit", Numeric(12, 2)),
    // audit: CVACT01Y-011
    col("acct_addr_zip", Char(10)),
    // audit: CVACT01Y-012
    col("acct_group_id", Char(10)),
    // audit: CVACT01Y-013
    col("filler", Char(178)),
];

/// `CUSTOMER-RECORD` (`CVCUS01Y`), 500 bytes.
const CUSTOMERS: &[Col] = &[
    // audit: CVCUS01Y-001
    col("cust_id", Unsigned(9)),
    // audit: CVCUS01Y-002
    col("cust_first_name", Char(25)),
    // audit: CVCUS01Y-003
    col("cust_middle_name", Char(25)),
    // audit: CVCUS01Y-004
    col("cust_last_name", Char(25)),
    // audit: CVCUS01Y-005
    col("cust_addr_line_1", Char(50)),
    // audit: CVCUS01Y-006
    col("cust_addr_line_2", Char(50)),
    // audit: CVCUS01Y-007
    col("cust_addr_line_3", Char(50)),
    // audit: CVCUS01Y-008
    col("cust_addr_state_cd", Char(2)),
    // audit: CVCUS01Y-009
    col("cust_addr_country_cd", Char(3)),
    // audit: CVCUS01Y-010
    col("cust_addr_zip", Char(10)),
    // audit: CVCUS01Y-011
    col("cust_phone_num_1", Char(15)),
    // audit: CVCUS01Y-012
    col("cust_phone_num_2", Char(15)),
    // audit: CVCUS01Y-013
    col("cust_ssn", Unsigned(9)),
    // audit: CVCUS01Y-014
    col("cust_govt_issued_id", Char(20)),
    // audit: CVCUS01Y-015
    nullable("cust_dob_yyyy_mm_dd", Date),
    // audit: CVCUS01Y-016
    col("cust_eft_account_id", Char(10)),
    // audit: CVCUS01Y-017
    col("cust_pri_card_holder_ind", Char(1)),
    // audit: CVCUS01Y-018
    col("cust_fico_credit_score", Unsigned(3)),
    // audit: CVCUS01Y-019
    col("filler", Char(168)),
];

/// `CARD-RECORD` (`CVACT02Y`), 150 bytes, then the `card_ref` handle.
const CARDS: &[Col] = &[
    // audit: CVACT02Y-001
    col("card_num", Char(16)),
    // audit: CVACT02Y-002
    col("acct_id", Unsigned(11)),
    // audit: CVACT02Y-003
    col("card_cvv_cd", Unsigned(3)),
    // audit: CVACT02Y-004
    col("card_embossed_name", Char(50)),
    // audit: CVACT02Y-005
    nullable("card_expiraion_date", Date),
    // audit: CVACT02Y-006
    col("card_active_status", Char(1)),
    // audit: CVACT02Y-007
    col("filler", Char(59)),
    // audit: CVACT02Y-008
    col("card_ref", Identity),
];

/// `CARD-XREF-RECORD` (`CVACT03Y`), 50 bytes.
const CARD_XREF: &[Col] = &[
    // audit: CVACT03Y-001
    col("card_num", Char(16)),
    // audit: CVACT03Y-002
    col("cust_id", Unsigned(9)),
    // audit: CVACT03Y-003
    col("acct_id", Unsigned(11)),
    // audit: CVACT03Y-004
    col("filler", Char(14)),
];

/// `TRAN-TYPE-RECORD` (`CVTRA03Y`), 60 bytes: the VSAM data version (E23).
const TRANSACTION_TYPES: &[Col] = &[
    // audit: CVTRA03Y-001
    col("tran_type", Char(2)),
    // audit: CVTRA03Y-002
    col("tran_type_desc", Char(50)),
    // audit: CVTRA03Y-003
    col("filler", Char(8)),
];

/// `TRAN-CAT-RECORD` (`CVTRA04Y`), 60 bytes: the VSAM data version (E23).
const TRANSACTION_CATEGORIES: &[Col] = &[
    // audit: CVTRA04Y-001
    col("tran_type_cd", Char(2)),
    // audit: CVTRA04Y-002
    col("tran_cat_cd", Unsigned(4)),
    // audit: CVTRA04Y-003
    col("tran_cat_type_desc", Char(50)),
    // audit: CVTRA04Y-004
    col("filler", Char(4)),
];

/// `DIS-GROUP-RECORD` (`CVTRA02Y`), 50 bytes.
const DISCLOSURE_GROUPS: &[Col] = &[
    // audit: CVTRA02Y-001
    col("group_id", Char(10)),
    // audit: CVTRA02Y-002
    col("tran_type_cd", Char(2)),
    // audit: CVTRA02Y-003
    col("tran_cat_cd", Unsigned(4)),
    // audit: CVTRA02Y-004
    col("dis_int_rate", Numeric(6, 2)),
    // audit: CVTRA02Y-005
    col("filler", Char(28)),
];

/// `TRAN-CAT-BAL-RECORD` (`CVTRA01Y`), 50 bytes.
const TRAN_CAT_BALANCES: &[Col] = &[
    // audit: CVTRA01Y-001
    col("acct_id", Unsigned(11)),
    // audit: CVTRA01Y-002
    col("tran_type_cd", Char(2)),
    // audit: CVTRA01Y-003
    col("tran_cat_cd", Unsigned(4)),
    // audit: CVTRA01Y-004
    col("tran_cat_bal", Numeric(11, 2)),
    // audit: CVTRA01Y-005
    col("filler", Char(22)),
];

/// `TRAN-RECORD` (`CVTRA05Y`), 350 bytes; `dailytran.txt` (`CVTRA06Y`) round-trips through the
/// same row shape.
const TRANSACTIONS: &[Col] = &[
    // audit: CVTRA05Y-001
    col("tran_id", Char(16)),
    // audit: CVTRA05Y-002
    col("tran_type_cd", Char(2)),
    // audit: CVTRA05Y-003
    col("tran_cat_cd", Unsigned(4)),
    // audit: CVTRA05Y-004
    col("tran_source", Char(10)),
    // audit: CVTRA05Y-005
    col("tran_desc", Char(100)),
    // audit: CVTRA05Y-006
    col("tran_amt", Numeric(11, 2)),
    // audit: CVTRA05Y-007
    col("tran_merchant_id", Unsigned(9)),
    // audit: CVTRA05Y-008
    col("tran_merchant_name", Char(50)),
    // audit: CVTRA05Y-009
    col("tran_merchant_city", Char(50)),
    // audit: CVTRA05Y-010
    col("tran_merchant_zip", Char(10)),
    // audit: CVTRA05Y-011
    col("card_num", Char(16)),
    // audit: CVTRA05Y-012
    nullable("orig_ts", Timestamp),
    nullable("orig_ts_form", Char(1)),
    // audit: CVTRA05Y-013
    nullable("proc_ts", Timestamp),
    nullable("proc_ts_form", Char(1)),
    // audit: CVTRA05Y-014
    col("filler", Char(20)),
];

/// `SEC-USER-DATA` (`CSUSR01Y`), 80 bytes, with the password stored only as its hash.
const USERS: &[Col] = &[
    // audit: CSUSR01Y-001
    col("usr_id", Char(8)),
    // audit: CSUSR01Y-002
    col("usr_fname", Char(20)),
    // audit: CSUSR01Y-003
    col("usr_lname", Char(20)),
    // audit: CSUSR01Y-004
    col("password_hash", Text),
    // audit: CSUSR01Y-005
    col("usr_type", Char(1)),
    // audit: CSUSR01Y-006
    col("filler", Char(23)),
];

/// IMS root segment `PAUTSUM0` (`CIPAUSMY`), 100 bytes, `PA-ACCOUNT-STATUS OCCURS 5` as five
/// columns.
const PENDING_AUTH_SUMMARIES: &[Col] = &[
    // audit: CIPAUSMY-001
    col("acct_id", Signed(11)),
    // audit: CIPAUSMY-002
    col("cust_id", Unsigned(9)),
    // audit: CIPAUSMY-003
    col("auth_status", Char(1)),
    // audit: CIPAUSMY-004
    col("account_status_1", Char(2)),
    col("account_status_2", Char(2)),
    col("account_status_3", Char(2)),
    col("account_status_4", Char(2)),
    col("account_status_5", Char(2)),
    // audit: CIPAUSMY-005
    col("credit_limit", Numeric(11, 2)),
    // audit: CIPAUSMY-006
    col("cash_limit", Numeric(11, 2)),
    // audit: CIPAUSMY-007
    col("credit_balance", Numeric(11, 2)),
    // audit: CIPAUSMY-008
    col("cash_balance", Numeric(11, 2)),
    // audit: CIPAUSMY-009
    col("approved_auth_cnt", SmallInt),
    // audit: CIPAUSMY-010
    col("declined_auth_cnt", SmallInt),
    // audit: CIPAUSMY-011
    col("approved_auth_amt", Numeric(11, 2)),
    // audit: CIPAUSMY-012
    col("declined_auth_amt", Numeric(11, 2)),
    // audit: CIPAUSMY-013
    col("filler", Char(34)),
];

/// IMS child segment `PAUTDTL1` (`CIPAUDTY`), 200 bytes, keyed under its parent's `ACCNTID`.
const PENDING_AUTH_DETAILS: &[Col] = &[
    // audit: CIPAUDTY-029
    col("acct_id", Signed(11)),
    // audit: CIPAUDTY-001
    col("auth_date_9c", Signed(5)),
    // audit: CIPAUDTY-002
    col("auth_time_9c", Signed(9)),
    // audit: CIPAUDTY-003
    col("auth_orig_date", Char(6)),
    // audit: CIPAUDTY-004
    col("auth_orig_time", Char(6)),
    // audit: CIPAUDTY-005
    col("card_num", Char(16)),
    // audit: CIPAUDTY-006
    col("auth_type", Char(4)),
    // audit: CIPAUDTY-007
    col("card_expiry_date", Char(4)),
    // audit: CIPAUDTY-008
    col("message_type", Char(6)),
    // audit: CIPAUDTY-009
    col("message_source", Char(6)),
    // audit: CIPAUDTY-010
    col("auth_id_code", Char(6)),
    // audit: CIPAUDTY-011
    col("auth_resp_code", Char(2)),
    // audit: CIPAUDTY-012
    col("auth_resp_reason", Char(4)),
    // audit: CIPAUDTY-013
    col("processing_code", Unsigned(6)),
    // audit: CIPAUDTY-014
    col("transaction_amt", Numeric(12, 2)),
    // audit: CIPAUDTY-015
    col("approved_amt", Numeric(12, 2)),
    // audit: CIPAUDTY-016
    col("merchant_catagory_code", Char(4)),
    // audit: CIPAUDTY-017
    col("acqr_country_code", Char(3)),
    // audit: CIPAUDTY-018
    col("pos_entry_mode", Unsigned(2)),
    // audit: CIPAUDTY-019
    col("merchant_id", Char(15)),
    // audit: CIPAUDTY-020
    col("merchant_name", Char(22)),
    // audit: CIPAUDTY-021
    col("merchant_city", Char(13)),
    // audit: CIPAUDTY-022
    col("merchant_state", Char(2)),
    // audit: CIPAUDTY-023
    col("merchant_zip", Char(9)),
    // audit: CIPAUDTY-024
    col("transaction_id", Char(15)),
    // audit: CIPAUDTY-025
    col("match_status", Char(1)),
    // audit: CIPAUDTY-026
    col("auth_fraud", Char(1)),
    // audit: CIPAUDTY-027
    col("fraud_rpt_date", Char(8)),
    // audit: CIPAUDTY-028
    col("filler", Char(17)),
];

/// Db2 table `CARDDEMO.AUTHFRDS` (`AUTHFRDS.ddl:1-28`): every column but the key may be `NULL`,
/// and `DECIMAL(11)` and `DECIMAL(9)` are signed `BIGINT` columns, so that the account and
/// customer keys are 64-bit as everywhere else.
const AUTH_FRAUD: &[Col] = &[
    col("card_num", Char(16)),
    col("auth_ts", Timestamp),
    nullable("auth_type", Char(4)),
    nullable("card_expiry_date", Char(4)),
    nullable("message_type", Char(6)),
    nullable("message_source", Char(6)),
    nullable("auth_id_code", Char(6)),
    nullable("auth_resp_code", Char(2)),
    nullable("auth_resp_reason", Char(4)),
    nullable("processing_code", Char(6)),
    nullable("transaction_amt", Numeric(12, 2)),
    nullable("approved_amt", Numeric(12, 2)),
    nullable("merchant_catagory_code", Char(4)),
    nullable("acqr_country_code", Char(3)),
    nullable("pos_entry_mode", SmallInt),
    nullable("merchant_id", Char(15)),
    nullable("merchant_name", Varchar(22)),
    nullable("merchant_city", Char(13)),
    nullable("merchant_state", Char(2)),
    nullable("merchant_zip", Char(9)),
    nullable("transaction_id", Char(15)),
    nullable("match_status", Char(1)),
    nullable("auth_fraud", Char(1)),
    nullable("fraud_rpt_date", Date),
    nullable("acct_id", Signed(11)),
    nullable("cust_id", Signed(9)),
];

/// `ERROR-LOG-RECORD` (`CCPAUERY`), 122 bytes, after the `id` that TDQ `CSSL` does not have.
const CSSL_ERROR_LOG: &[Col] = &[
    // audit: CCPAUERY-012
    col("id", Serial),
    // audit: CCPAUERY-001
    col("err_date", Char(6)),
    // audit: CCPAUERY-002
    col("err_time", Char(6)),
    // audit: CCPAUERY-003
    col("err_application", Char(8)),
    // audit: CCPAUERY-004
    col("err_program", Char(8)),
    // audit: CCPAUERY-005
    col("err_location", Char(4)),
    // audit: CCPAUERY-006
    col("err_level", Char(1)),
    // audit: CCPAUERY-007
    col("err_subsystem", Char(1)),
    // audit: CCPAUERY-008
    col("err_code_1", Char(9)),
    // audit: CCPAUERY-009
    col("err_code_2", Char(9)),
    // audit: CCPAUERY-010
    col("err_message", Char(50)),
    // audit: CCPAUERY-011
    col("err_event_key", Char(20)),
];

/// Db2 table `CARDDEMO.TRANSACTION_TYPE` (`TRNTYPE.ddl:1-4`), the Db2 data version (E23).
const DB2_TRANSACTION_TYPE: &[Col] = &[col("tr_type", Char(2)), col("tr_description", Varchar(50))];

/// Db2 table `CARDDEMO.TRANSACTION_TYPE_CATEGORY` (`TRNTYCAT.ddl:1-7`), the Db2 data version
/// (E23).
const DB2_TRANSACTION_TYPE_CATEGORY: &[Col] = &[
    col("trc_type_code", Char(2)),
    col("trc_type_category", Char(4)),
    col("trc_cat_data", Varchar(50)),
];

/// A key or index of one table: its name and its columns, in order, as one list.
#[derive(Clone, Copy)]
struct Key<'a> {
    schema: &'a str,
    table: &'a str,
    name: &'a str,
    columns: &'a str,
}

/// A key of a table of schema `public`.
const fn key(table: &'static str, name: &'static str, columns: &'static str) -> Key<'static> {
    Key {
        schema: "public",
        table,
        name,
        columns,
    }
}

/// Every primary key: the KSDS `KEYS(length offset)` of each load job, the IMS sequence fields
/// and the Db2 `PRIMARY KEY` clauses, under `PostgreSQL`'s default `<table>_pkey` name, and the
/// Db2 index names for the Db2 data version (`XTRNTYPE.ddl`, `XTRNTYCAT.ddl`).
const PRIMARY_KEYS: [Key<'static>; 16] = [
    key("accounts", "accounts_pkey", "acct_id"),
    key("customers", "customers_pkey", "cust_id"),
    key("cards", "cards_pkey", "card_num"),
    key("card_xref", "card_xref_pkey", "card_num"),
    key("transaction_types", "transaction_types_pkey", "tran_type"),
    key(
        "transaction_categories",
        "transaction_categories_pkey",
        "tran_type_cd, tran_cat_cd",
    ),
    key(
        "disclosure_groups",
        "disclosure_groups_pkey",
        "group_id, tran_type_cd, tran_cat_cd",
    ),
    key(
        "tran_cat_balances",
        "tran_cat_balances_pkey",
        "acct_id, tran_type_cd, tran_cat_cd",
    ),
    key("transactions", "transactions_pkey", "tran_id"),
    key("users", "users_pkey", "usr_id"),
    // `ACCNTID` (`DBPAUTP0.dbd:30`).
    key(
        "pending_auth_summaries",
        "pending_auth_summaries_pkey",
        "acct_id",
    ),
    // The parent's `ACCNTID`, then `PAUT9CTS` (`DBPAUTP0.dbd:37`).
    key(
        "pending_auth_details",
        "pending_auth_details_pkey",
        "acct_id, auth_date_9c, auth_time_9c",
    ),
    key("auth_fraud", "auth_fraud_pkey", "card_num, auth_ts"),
    key("cssl_error_log", "cssl_error_log_pkey", "id"),
    Key {
        schema: "db2",
        table: "transaction_type",
        name: "xtran_type",
        columns: "tr_type",
    },
    Key {
        schema: "db2",
        table: "transaction_type_category",
        name: "x_tran_type_catg",
        columns: "trc_type_code, trc_type_category",
    },
];

/// The one unique constraint that is not a primary key: the card handle.
// audit: CVACT02Y-008
// audit: LOADS-027
const UNIQUE_KEYS: [Key<'static>; 1] = [key("cards", "cards_card_ref_key", "card_ref")];

/// The indexes that serve no key constraint, each as `(unique, key)`, the key's columns written
/// as `pg_get_indexdef` renders them.
const SECONDARY_INDEXES: [(bool, Key<'static>); 5] = [
    // The AIX `KEYS(11 16) NONUNIQUEKEY` over `CARD-ACCT-ID`, in prime-key order within it.
    // audit: LOADS-022
    (
        false,
        key("cards", "cards_acct_id_idx", "acct_id, card_num"),
    ),
    // The AIX `KEYS(11,25) NONUNIQUEKEY` over `XREF-ACCT-ID`.
    // audit: LOADS-023
    (false, key("card_xref", "card_xref_acct_id_idx", "acct_id")),
    // The AIX `KEYS(26 304)` over `TRAN-PROC-TS`.
    // audit: CVTRA05Y-013
    (
        false,
        key("transactions", "transactions_proc_ts_idx", "proc_ts"),
    ),
    // The card-ordered reads that CREASTMT and TRANREPT obtain by SORT.
    // audit: CVTRA05Y-011
    (
        false,
        key(
            "transactions",
            "transactions_card_num_idx",
            "card_num, tran_id",
        ),
    ),
    // `XAUTHFRD.ddl:1-4`.
    (
        true,
        key("auth_fraud", "xauthfrd", "card_num, auth_ts DESC"),
    ),
];

/// One foreign key: the referencing key, the referenced table and columns, and the delete
/// action as `pg_get_constraintdef` names it.
struct ForeignKey {
    from: Key<'static>,
    references: &'static str,
    on_delete: &'static str,
}

/// Every foreign key. VSAM declares none, so none joins the tables the loads replace; IMS deletes
/// a child segment with its parent, and Db2 restricts deleting a referenced transaction type
/// (`TRNTYCAT.ddl:6-7`).
// audit: LOADS-025
// audit: CIPAUDTY-029
const FOREIGN_KEYS: [ForeignKey; 2] = [
    ForeignKey {
        from: key(
            "pending_auth_details",
            "pending_auth_details_summary_fk",
            "acct_id",
        ),
        references: "public.pending_auth_summaries (acct_id)",
        on_delete: "CASCADE",
    },
    ForeignKey {
        from: Key {
            schema: "db2",
            table: "transaction_type_category",
            name: "trc_type_code",
            columns: "trc_type_code",
        },
        references: "db2.transaction_type (tr_type)",
        on_delete: "RESTRICT",
    },
];

/// The check constraints of `transactions` besides its range checks, each as `(name, definition)`
/// in the words `pg_get_constraintdef` writes: each timestamp and its layout code are both `NULL`
/// for a blank value or both present (PIC-017), the code is one of `I`, `D` and `S` (PIC-018 to
/// PIC-020), and a layout-`S` value is a date at midnight (PIC-020, E33).
const TIMESTAMP_LAYOUT_CHECKS: [(&str, &str); 6] = [
    (
        "transactions_orig_ts_form_ck",
        "CHECK (((orig_ts IS NULL) = (orig_ts_form IS NULL)))",
    ),
    (
        "transactions_proc_ts_form_ck",
        "CHECK (((proc_ts IS NULL) = (proc_ts_form IS NULL)))",
    ),
    (
        "transactions_orig_ts_form_code_ck",
        "CHECK ((orig_ts_form = ANY (ARRAY['I'::bpchar, 'D'::bpchar, 'S'::bpchar])))",
    ),
    (
        "transactions_proc_ts_form_code_ck",
        "CHECK ((proc_ts_form = ANY (ARRAY['I'::bpchar, 'D'::bpchar, 'S'::bpchar])))",
    ),
    (
        "transactions_orig_ts_screen_date_ck",
        "CHECK (((orig_ts_form <> 'S'::bpchar) OR ((orig_ts)::time without time zone = \
         '00:00:00'::time without time zone)))",
    ),
    (
        "transactions_proc_ts_screen_date_ck",
        "CHECK (((proc_ts_form <> 'S'::bpchar) OR ((proc_ts)::time without time zone = \
         '00:00:00'::time without time zone)))",
    ),
];

/// One row of `information_schema.columns`, its domain-typed columns cast to base types.
struct CatalogColumn {
    schema: String,
    table: String,
    name: String,
    data_type: String,
    char_len: Option<i32>,
    precision: Option<i32>,
    scale: Option<i32>,
    dt_precision: Option<i32>,
    nullable: bool,
    identity: bool,
    generation: Option<String>,
    column_default: Option<String>,
}

/// One key, unique, foreign-key or check constraint of a table in `public` or `db2`.
struct CatalogConstraint {
    schema: String,
    table: String,
    name: String,
    /// `pg_constraint.contype`: `p`, `u`, `f` or `c`.
    kind: String,
    def: String,
    columns: Vec<String>,
    ref_schema: Option<String>,
    ref_table: Option<String>,
    ref_columns: Vec<String>,
    /// `pg_constraint.confdeltype`; a space for a constraint that is not a foreign key.
    on_delete: String,
}

/// One index of a table in `public` or `db2`, as `pg_indexes` reconstructs its definition.
struct CatalogIndex {
    schema: String,
    table: String,
    def: String,
}

/// Whether `schema.table` is one of [`TABLES`].
fn is_expected_table(schema: &str, table: &str) -> bool {
    TABLES
        .iter()
        .any(|expected| expected.schema == schema && expected.name == table)
}

/// Records every item of `expected` that `observed` lacks, and every item of `observed` that
/// `expected` does not list.
fn compare_sets(
    what: &str,
    expected: &BTreeSet<String>,
    observed: &BTreeSet<String>,
    failures: &mut Vec<String>,
) {
    failures.extend(
        expected
            .difference(observed)
            .map(|item| format!("{what} missing: {item}")),
    );
    failures.extend(
        observed
            .difference(expected)
            .map(|item| format!("{what} not expected: {item}")),
    );
}

/// The schemas, other than `PostgreSQL`'s own, are exactly [`SCHEMAS`], and they hold exactly
/// [`TABLES`] and [`OTHER_TABLES`], every one a base table.
async fn check_inventory(pool: &PgPool, failures: &mut Vec<String>) {
    let schemas: BTreeSet<String> = sqlx::query_scalar!(
        r#"SELECT nspname::text AS "name!"
             FROM pg_namespace
            WHERE NOT starts_with(nspname::text, 'pg_') AND nspname <> 'information_schema'"#
    )
    .fetch_all(pool)
    .await
    .expect("list the schemas")
    .into_iter()
    .collect();
    let expected_schemas = SCHEMAS.iter().map(|&name| name.to_owned()).collect();
    compare_sets("schema", &expected_schemas, &schemas, failures);

    let tables = sqlx::query!(
        r#"SELECT table_schema::text AS "schema!", table_name::text AS "name!",
                  table_type::text AS "kind!"
             FROM information_schema.tables
            WHERE table_schema IN ('public', 'db2')"#
    )
    .fetch_all(pool)
    .await
    .expect("list the tables of public and db2");
    let observed = tables
        .iter()
        .map(|row| format!("{}.{} ({})", row.schema, row.name, row.kind))
        .collect();
    let expected = TABLES
        .iter()
        .map(|table| (table.schema, table.name))
        .chain(OTHER_TABLES)
        .map(|(schema, name)| format!("{schema}.{name} (BASE TABLE)"))
        .collect();
    compare_sets("table", &expected, &observed, failures);
}

/// Every column of `public` and `db2`, grouped by table in ordinal order.
async fn fetch_columns(pool: &PgPool) -> BTreeMap<(String, String), Vec<CatalogColumn>> {
    let columns = sqlx::query_as!(
        CatalogColumn,
        r#"SELECT table_schema::text AS "schema!",
                  table_name::text AS "table!",
                  column_name::text AS "name!",
                  data_type::text AS "data_type!",
                  character_maximum_length::int4 AS "char_len?",
                  numeric_precision::int4 AS "precision?",
                  numeric_scale::int4 AS "scale?",
                  datetime_precision::int4 AS "dt_precision?",
                  (is_nullable = 'YES') AS "nullable!",
                  (is_identity = 'YES') AS "identity!",
                  identity_generation::text AS "generation?",
                  column_default::text AS "column_default?"
             FROM information_schema.columns
            WHERE table_schema IN ('public', 'db2')
            ORDER BY table_schema, table_name, ordinal_position"#
    )
    .fetch_all(pool)
    .await
    .expect("list the columns of public and db2");
    let mut by_table: BTreeMap<(String, String), Vec<CatalogColumn>> = BTreeMap::new();
    for column in columns {
        by_table
            .entry((column.schema.clone(), column.table.clone()))
            .or_default()
            .push(column);
    }
    by_table
}

/// The type `ty` stands for, in the words [`observed_type`] uses.
fn expected_type(ty: Ty) -> String {
    match ty {
        Char(width) => format!("character({width})"),
        Varchar(width) => format!("character varying({width})"),
        Numeric(precision, scale) => format!("numeric({precision},{scale})"),
        Unsigned(_) | Signed(_) => "bigint".to_owned(),
        Identity => "bigint generated ALWAYS as identity".to_owned(),
        Serial => "bigint default nextval".to_owned(),
        SmallInt => "smallint".to_owned(),
        Date => "date".to_owned(),
        Timestamp => "timestamp(6) without time zone".to_owned(),
        Text => "text".to_owned(),
    }
}

/// The catalog's description of `column`'s type: the base type with its length, precision and
/// scale, then any identity generation and any default. Defaults are schema expressions, not
/// stored values.
fn observed_type(column: &CatalogColumn) -> String {
    let data_type = column.data_type.as_str();
    let base = match (data_type, column.char_len, column.precision, column.scale) {
        ("character" | "character varying", Some(width), _, _) => format!("{data_type}({width})"),
        ("numeric", _, Some(precision), Some(scale)) => format!("numeric({precision},{scale})"),
        ("timestamp without time zone", ..) => match column.dt_precision {
            Some(precision) => format!("timestamp({precision}) without time zone"),
            None => data_type.to_owned(),
        },
        _ => data_type.to_owned(),
    };
    let identity = if column.identity {
        let generation = column.generation.as_deref().unwrap_or("?");
        format!(" generated {generation} as identity")
    } else {
        String::new()
    };
    let default = match column.column_default.as_deref() {
        Some(text) if text.starts_with("nextval(") => " default nextval".to_owned(),
        Some(text) => format!(" default {text}"),
        None => String::new(),
    };
    format!("{base}{identity}{default}")
}

/// `NULL` admitted or not, as a failure message says it.
const fn nullability(nullable: bool) -> &'static str {
    if nullable { "NULL" } else { "NOT NULL" }
}

/// Every column of `table`: the column names equal the expected set exactly, and each column has
/// its expected type and nullability.
fn compare_columns(table: &Table, observed: &[CatalogColumn], failures: &mut Vec<String>) {
    let qualified = format!("{}.{}", table.schema, table.name);
    for column in observed {
        if !table
            .columns
            .iter()
            .any(|expected| expected.name == column.name)
        {
            failures.push(format!(
                "{qualified}.{}: no copybook field or DDL column has this column",
                column.name
            ));
        }
    }
    for expected in table.columns {
        let Some(column) = observed.iter().find(|column| column.name == expected.name) else {
            failures.push(format!("{qualified}.{}: column missing", expected.name));
            continue;
        };
        let (want, found) = (expected_type(expected.ty), observed_type(column));
        if want != found {
            failures.push(format!(
                "{qualified}.{}: expected {want}, found {found}",
                expected.name
            ));
        }
        if expected.nullable != column.nullable {
            failures.push(format!(
                "{qualified}.{}: expected {}, found {}",
                expected.name,
                nullability(expected.nullable),
                nullability(column.nullable)
            ));
        }
    }
}

/// Every table of [`TABLES`] has exactly its copybook or DDL columns.
async fn check_columns(pool: &PgPool, failures: &mut Vec<String>) {
    let by_table = fetch_columns(pool).await;
    for table in &TABLES {
        let observed = by_table
            .get(&(table.schema.to_owned(), table.name.to_owned()))
            .map_or(&[][..], Vec::as_slice);
        compare_columns(table, observed, failures);
    }
}

/// No column of `public` or `db2`, sqlx's own table included, has a binary floating-point type:
/// money and rates stay exact decimals (R1).
async fn check_no_float_columns(pool: &PgPool, failures: &mut Vec<String>) {
    let floats = sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!"
             FROM information_schema.columns
            WHERE table_schema IN ('public', 'db2')
              AND data_type IN ('real', 'double precision')"#
    )
    .fetch_one(pool)
    .await
    .expect("count the floating-point columns");
    if floats != 0 {
        failures.push(format!(
            "{floats} column(s) of public or db2 are floating-point"
        ));
    }
}

/// Every constraint of the tables of `public` and `db2`, with its key columns in key order and,
/// for a foreign key, the referenced table and columns.
async fn fetch_constraints(pool: &PgPool) -> Vec<CatalogConstraint> {
    sqlx::query_as!(
        CatalogConstraint,
        r#"SELECT n.nspname::text AS "schema!",
                  t.relname::text AS "table!",
                  c.conname::text AS "name!",
                  c.contype::text AS "kind!",
                  pg_get_constraintdef(c.oid) AS "def!",
                  ARRAY(SELECT a.attname::text
                          FROM unnest(c.conkey) WITH ORDINALITY AS k (attnum, ord)
                          JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum
                         ORDER BY k.ord) AS "columns!",
                  rn.nspname::text AS "ref_schema?",
                  r.relname::text AS "ref_table?",
                  ARRAY(SELECT a.attname::text
                          FROM unnest(c.confkey) WITH ORDINALITY AS k (attnum, ord)
                          JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.attnum
                         ORDER BY k.ord) AS "ref_columns!",
                  c.confdeltype::text AS "on_delete!"
             FROM pg_constraint c
             JOIN pg_class t ON t.oid = c.conrelid
             JOIN pg_namespace n ON n.oid = t.relnamespace
             LEFT JOIN pg_class r ON r.oid = c.confrelid
             LEFT JOIN pg_namespace rn ON rn.oid = r.relnamespace
            WHERE n.nspname IN ('public', 'db2')
            ORDER BY 1, 2, 3"#
    )
    .fetch_all(pool)
    .await
    .expect("list the constraints of public and db2")
}

/// A key as the comparisons below write it.
fn key_text(kind: &str, key: &Key<'_>) -> String {
    format!(
        "{}.{} {kind} {} ({})",
        key.schema, key.table, key.name, key.columns
    )
}

/// The delete action a `confdeltype` code stands for.
fn delete_action(code: &str) -> &str {
    match code {
        "a" => "NO ACTION",
        "r" => "RESTRICT",
        "c" => "CASCADE",
        "n" => "SET NULL",
        "d" => "SET DEFAULT",
        other => other,
    }
}

/// The primary keys, unique constraints and foreign keys are exactly [`PRIMARY_KEYS`],
/// [`UNIQUE_KEYS`] and [`FOREIGN_KEYS`]. Equality of the foreign keys also proves that none joins
/// the tables the loads replace (LOADS-025).
fn check_keys(constraints: &[CatalogConstraint], failures: &mut Vec<String>) {
    let mut observed = BTreeSet::new();
    for constraint in constraints {
        if !is_expected_table(&constraint.schema, &constraint.table) {
            continue;
        }
        let columns = constraint.columns.join(", ");
        let key = Key {
            schema: &constraint.schema,
            table: &constraint.table,
            name: &constraint.name,
            columns: &columns,
        };
        let text = match constraint.kind.as_str() {
            "p" => key_text("PRIMARY KEY", &key),
            "u" => key_text("UNIQUE", &key),
            "f" => format!(
                "{} REFERENCES {}.{} ({}) ON DELETE {}",
                key_text("FOREIGN KEY", &key),
                constraint.ref_schema.as_deref().unwrap_or("?"),
                constraint.ref_table.as_deref().unwrap_or("?"),
                constraint.ref_columns.join(", "),
                delete_action(&constraint.on_delete)
            ),
            _ => continue,
        };
        observed.insert(text);
    }
    let expected = PRIMARY_KEYS
        .iter()
        .map(|key| key_text("PRIMARY KEY", key))
        .chain(UNIQUE_KEYS.iter().map(|key| key_text("UNIQUE", key)))
        .chain(FOREIGN_KEYS.iter().map(|fk| {
            format!(
                "{} REFERENCES {} ON DELETE {}",
                key_text("FOREIGN KEY", &fk.from),
                fk.references,
                fk.on_delete
            )
        }))
        .collect();
    compare_sets("key", &expected, &observed, failures);
}

/// The bounds a `9(n)` or `S9(n)` picture allows, as decimal literals: 0 or minus n nines, and
/// n nines.
fn picture_range(ty: Ty) -> Option<(String, String)> {
    match ty {
        Unsigned(digits) => Some(("0".to_owned(), "9".repeat(digits))),
        Signed(digits) => Some((format!("-{}", "9".repeat(digits)), "9".repeat(digits))),
        _ => None,
    }
}

/// The text after a cast to an integer type that `after` begins with, when `literal` is a whole
/// number: an optional minus sign and at least one digit.
fn after_integer_cast<'a>(literal: &str, after: &'a str) -> Option<&'a str> {
    let digits = literal.strip_prefix('-').unwrap_or(literal);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    ["::bigint", "::integer", "::smallint"]
        .iter()
        .find_map(|cast| after.strip_prefix(cast))
}

/// A check constraint's definition with every quoted whole number that carries a cast to an
/// integer type written as the bare number, and every other character kept as it stands.
/// `PostgreSQL` quotes and casts a bound that is negative or wider than `integer`, so
/// `BETWEEN -99999 AND 99999999999` comes back from `pg_get_constraintdef` with
/// `'-99999'::integer` and `'99999999999'::bigint`, which become `-99999` and `99999999999`.
fn normalised_check(def: &str) -> String {
    let mut normalised = String::with_capacity(def.len());
    let mut rest = def;
    while let Some((before, quoted)) = rest.split_once('\'') {
        normalised.push_str(before);
        let Some((literal, after)) = quoted.split_once('\'') else {
            normalised.push('\'');
            rest = quoted;
            break;
        };
        if let Some(tail) = after_integer_cast(literal, after) {
            normalised.push_str(literal);
            rest = tail;
        } else {
            normalised.push('\'');
            normalised.push_str(literal);
            normalised.push('\'');
            rest = after;
        }
    }
    normalised.push_str(rest);
    normalised
}

/// The check constraints of [`TABLES`] are exactly one range check per `9(n)` and `S9(n)`
/// column and [`TIMESTAMP_LAYOUT_CHECKS`], each compared by its complete definition. A range
/// check carries `PostgreSQL`'s default `<table>_<column>_check` name and the conjunction
/// `BETWEEN` expands to over the bounds its picture holds (PIC-003, PIC-023), so a check joined
/// by `OR`, one with an extra predicate or another bound, and a missing or unexpected check are
/// each reported with their table and name. A check admits `NULL`; whether a column does is
/// proven by [`compare_columns`].
fn check_constraints(constraints: &[CatalogConstraint], failures: &mut Vec<String>) {
    let observed = constraints
        .iter()
        .filter(|c| c.kind == "c" && is_expected_table(&c.schema, &c.table))
        .map(|c| {
            let def = normalised_check(&c.def);
            format!("{}.{} {} {def}", c.schema, c.table, c.name)
        })
        .collect();
    let ranges = TABLES.iter().flat_map(|table| {
        table.columns.iter().filter_map(move |column| {
            let (low, high) = picture_range(column.ty)?;
            let (schema, name, col) = (table.schema, table.name, column.name);
            Some(format!(
                "{schema}.{name} {name}_{col}_check \
                 CHECK ((({col} >= {low}) AND ({col} <= {high})))"
            ))
        })
    });
    let expected = ranges
        .chain(
            TIMESTAMP_LAYOUT_CHECKS
                .iter()
                .map(|(name, def)| format!("public.transactions {name} {def}")),
        )
        .collect();
    compare_sets("check", &expected, &observed, failures);
}

/// The indexes of [`TABLES`] are exactly those of the primary keys, the unique constraints and
/// [`SECONDARY_INDEXES`], each a B-tree over its listed columns in order.
async fn check_indexes(pool: &PgPool, failures: &mut Vec<String>) {
    let indexes = sqlx::query_as!(
        CatalogIndex,
        r#"SELECT schemaname::text AS "schema!", tablename::text AS "table!",
                  indexdef AS "def!"
             FROM pg_indexes
            WHERE schemaname IN ('public', 'db2')"#
    )
    .fetch_all(pool)
    .await
    .expect("list the indexes of public and db2");
    let observed = indexes
        .into_iter()
        .filter(|index| is_expected_table(&index.schema, &index.table))
        .map(|index| index.def)
        .collect();
    let index_def = |unique: bool, key: &Key<'_>| {
        let kind = if unique { "UNIQUE INDEX" } else { "INDEX" };
        format!(
            "CREATE {kind} {} ON {}.{} USING btree ({})",
            key.name, key.schema, key.table, key.columns
        )
    };
    let expected = PRIMARY_KEYS
        .iter()
        .chain(&UNIQUE_KEYS)
        .map(|key| index_def(true, key))
        .chain(
            SECONDARY_INDEXES
                .iter()
                .map(|(unique, key)| index_def(*unique, key)),
        )
        .collect();
    compare_sets("index", &expected, &observed, failures);
}

/// Asserts that `result` is a check violation, of the constraint `constraint` when one is named.
/// A refused statement shows only the error's SQLSTATE class and constraint name.
fn assert_check(result: Result<PgQueryResult, sqlx::Error>, what: &str, constraint: Option<&str>) {
    match result.map_err(PersistenceError::from) {
        Err(PersistenceError::Check { constraint: found }) => {
            if let Some(name) = constraint {
                assert_eq!(
                    found.as_deref(),
                    Some(name),
                    "{what}: another check refused it"
                );
            }
        }
        Err(other) => panic!("{what}: expected a check violation, found {other}"),
        Ok(_) => panic!("{what}: accepted, though a check constraint must refuse it"),
    }
}

/// Asserts that `result` changed exactly one row.
fn assert_one_row(result: Result<PgQueryResult, sqlx::Error>, what: &str) {
    match result {
        Ok(done) => assert_eq!(done.rows_affected(), 1, "{what}: expected one row"),
        Err(err) => panic!("{what}: refused: {}", PersistenceError::from(err)),
    }
}

/// `PA-ACCT-ID` and `PA-CUST-ID` of the summary the behaviour test stores: account and customer
/// 50, the keys `cardxref.txt` line 1 joins.
const SUMMARY_ACCOUNT: i64 = 50;
const SUMMARY_CUSTOMER: u64 = 50;

/// An account no summary is stored under.
const ORPHAN_ACCOUNT: i64 = 51;

/// `YYDDD` and `HHMMSSmmm` of the detail the behaviour test stores, inverted into its key as
/// COPAUA0C does (CIPAUDTY-001, CIPAUDTY-002).
const DETAIL_YYDDD: u64 = 22_200;
const DETAIL_HHMMSSMMM: u64 = 123_456_789;

/// `XREF-CARD-NUM PIC X(16)` at offset 0 of `CARD-XREF-RECORD` (CVACT03Y-001).
const XREF_CARD_NUM_LEN: usize = 16;

/// The first record of `acctdata.txt`, read in place and decoded by codec A.
fn first_fixture_account() -> AccountRecord {
    let bytes = std::fs::read(fixture_path("acctdata.txt")).expect("read acctdata.txt in place");
    decode_fixture::<AccountRecord>(&bytes)
        .expect("codec A decodes acctdata.txt")
        .into_iter()
        .next()
        .expect("acctdata.txt holds line 1")
        .record
}

/// The card number of `cardxref.txt` line 1, read in place. It is held only as a [`CardNumber`],
/// whose `Debug` and `Display` redact it.
fn first_fixture_card() -> CardNumber {
    let bytes = std::fs::read(fixture_path("cardxref.txt")).expect("read cardxref.txt in place");
    let field = bytes
        .get(..XREF_CARD_NUM_LEN)
        .expect("cardxref.txt line 1 holds XREF-CARD-NUM");
    CardNumber::new(PicX::decode(field).expect("XREF-CARD-NUM is 16 printable bytes"))
}

/// A summary key as `PA-ACCT-ID PIC S9(11) COMP-3`.
fn summary_key(acct_id: i64) -> PicS<11> {
    PicS::new(acct_id).expect("an 11-digit account key")
}

/// An approved detail for `card_num` under the fixed key of [`DETAIL_YYDDD`] and
/// [`DETAIL_HHMMSSMMM`], every other text field blank and every amount zero.
fn pending_detail(card_num: CardNumber) -> PendingAuthDetailRecord {
    PendingAuthDetailRecord {
        auth_date_9c: PicS::complement_from(DETAIL_YYDDD).expect("an inverted YYDDD key"),
        auth_time_9c: PicS::complement_from(DETAIL_HHMMSSMMM).expect("an inverted time key"),
        orig_date: PicX::spaces(),
        orig_time: PicX::spaces(),
        card_num,
        auth_type: PicX::spaces(),
        card_expiry_date: PicX::spaces(),
        message_type: PicX::spaces(),
        message_source: PicX::spaces(),
        auth_id_code: PicX::spaces(),
        auth_resp_code: PicX::from_array(*PendingAuthDetailRecord::AUTH_APPROVED),
        auth_resp_reason: PicX::spaces(),
        processing_code: PicU::default(),
        transaction_amt: Money::ZERO,
        approved_amt: Money::ZERO,
        merchant_catagory_code: PicX::spaces(),
        acqr_country_code: PicX::spaces(),
        pos_entry_mode: PicU::default(),
        merchant_id: PicX::spaces(),
        merchant_name: PicX::spaces(),
        merchant_city: PicX::spaces(),
        merchant_state: PicX::spaces(),
        merchant_zip: PicX::spaces(),
        transaction_id: PicX::spaces(),
        match_status: PicX::from_array(*PendingAuthDetailRecord::MATCH_PENDING),
        auth_fraud: PicX::spaces(),
        fraud_rpt_date: PicX::spaces(),
        filler: PicX::spaces(),
    }
}

/// A Db2 transaction type code, `TR_TYPE CHAR(2)`.
fn type_code(code: &str) -> PicX<2> {
    PicX::from_text(code).expect("a two-character type code")
}

/// `accounts.acct_id`, `PIC 9(11)`, holds 0 and eleven nines and refuses -1 and 10^11.
async fn account_key_range_is_enforced(pool: &PgPool, store: &mut PgStore) {
    AccountRepository::insert(store, &first_fixture_account())
        .await
        .expect("insert the account of acctdata.txt line 1");
    assert_check(
        sqlx::query!("UPDATE accounts SET acct_id = 100000000000")
            .execute(pool)
            .await,
        "accounts.acct_id = 10^11",
        None,
    );
    assert_check(
        sqlx::query!("UPDATE accounts SET acct_id = -1")
            .execute(pool)
            .await,
        "accounts.acct_id = -1",
        None,
    );
    assert_one_row(
        sqlx::query!("UPDATE accounts SET acct_id = 99999999999")
            .execute(pool)
            .await,
        "accounts.acct_id = eleven nines",
    );
    assert_one_row(
        sqlx::query!("UPDATE accounts SET acct_id = 0")
            .execute(pool)
            .await,
        "accounts.acct_id = 0",
    );
    let zero = PicU::new(0).expect("account key 0");
    let stored = AccountRepository::find_by_id(store, zero)
        .await
        .expect("read account 0");
    assert!(
        stored.is_some(),
        "accounts: the row moved to key 0 reads back"
    );
}

/// `pending_auth_summaries.acct_id`, `PIC S9(11) COMP-3`, holds minus eleven nines and refuses
/// ±10^11.
async fn summary_key_range_is_enforced(pool: &PgPool, store: &mut PgStore) {
    let summary = PendingAuthSummaryRecord::initialized(
        summary_key(SUMMARY_ACCOUNT),
        PicU::new(SUMMARY_CUSTOMER).expect("a 9-digit customer key"),
    );
    PendingAuthSummaryRepository::insert(store, &summary)
        .await
        .expect("insert the summary of account 50");
    assert_check(
        sqlx::query!("UPDATE pending_auth_summaries SET acct_id = 100000000000")
            .execute(pool)
            .await,
        "pending_auth_summaries.acct_id = 10^11",
        None,
    );
    assert_check(
        sqlx::query!("UPDATE pending_auth_summaries SET acct_id = -100000000000")
            .execute(pool)
            .await,
        "pending_auth_summaries.acct_id = -10^11",
        None,
    );
    assert_one_row(
        sqlx::query!("UPDATE pending_auth_summaries SET acct_id = -99999999999")
            .execute(pool)
            .await,
        "pending_auth_summaries.acct_id = minus eleven nines",
    );
    assert_one_row(
        sqlx::query!("UPDATE pending_auth_summaries SET acct_id = 50")
            .execute(pool)
            .await,
        "pending_auth_summaries.acct_id back to 50",
    );
}

/// A detail needs its summary, and deleting the summary deletes its details, as IMS deletes a
/// child segment with its parent.
async fn detail_hierarchy_is_enforced(store: &mut PgStore) {
    let detail = pending_detail(first_fixture_card());
    let (date_9c, time_9c) = (detail.auth_date_9c, detail.auth_time_9c);
    match PendingAuthDetailRepository::insert(store, summary_key(ORPHAN_ACCOUNT), &detail).await {
        Err(err) => {
            assert!(
                err.is_restrict(),
                "a detail without its summary: found {err}"
            );
            assert_eq!(
                err.constraint(),
                Some("pending_auth_details_summary_fk"),
                "a detail without its summary: refused by another constraint"
            );
        }
        Ok(()) => panic!("a detail without its summary was stored"),
    }

    let parent = summary_key(SUMMARY_ACCOUNT);
    PendingAuthDetailRepository::insert(store, parent, &detail)
        .await
        .expect("insert a detail under the summary of account 50");
    let stored = PendingAuthDetailRepository::find_by_id(store, parent, date_9c, time_9c)
        .await
        .expect("read the detail back");
    assert!(
        stored.is_some(),
        "pending_auth_details: the detail reads back"
    );

    let deleted = PendingAuthSummaryRepository::delete(store, parent)
        .await
        .expect("delete the summary of account 50");
    assert!(deleted, "pending_auth_summaries: the summary is deleted");
    let orphan = PendingAuthDetailRepository::find_by_id(store, parent, date_9c, time_9c)
        .await
        .expect("read the detail after its summary was deleted");
    assert!(
        orphan.is_none(),
        "pending_auth_details_summary_fk: ON DELETE CASCADE left the detail behind"
    );
}

/// For `orig_ts` and for `proc_ts`, a timestamp without its layout code and a code without its
/// timestamp are each refused by that pair's check, on the row
/// [`timestamp_layout_codes_are_enforced`] stores: `orig_ts` in layout `I`, `proc_ts` blank. A
/// code without its timestamp is `I`, which the code and screen-date checks admit, so only the
/// pair's check can refuse it.
async fn one_sided_timestamp_pairs_are_refused(pool: &PgPool) {
    assert_check(
        sqlx::query!("UPDATE transactions SET orig_ts_form = NULL")
            .execute(pool)
            .await,
        "orig_ts without its layout code",
        Some("transactions_orig_ts_form_ck"),
    );
    assert_check(
        sqlx::query!("UPDATE transactions SET orig_ts = NULL")
            .execute(pool)
            .await,
        "orig_ts_form without its timestamp",
        Some("transactions_orig_ts_form_ck"),
    );
    assert_check(
        sqlx::query!("UPDATE transactions SET proc_ts = TIMESTAMP '2022-06-10 19:27:53'")
            .execute(pool)
            .await,
        "proc_ts without its layout code",
        Some("transactions_proc_ts_form_ck"),
    );
    assert_check(
        sqlx::query!("UPDATE transactions SET proc_ts_form = 'I'")
            .execute(pool)
            .await,
        "proc_ts_form without its timestamp",
        Some("transactions_proc_ts_form_ck"),
    );
}

/// Each timestamp of `transactions` and its layout code are both `NULL` or both present: for
/// `orig_ts` and for `proc_ts`, both one-sided pairs are refused
/// ([`one_sided_timestamp_pairs_are_refused`]), and a pair with both present and a pair with both
/// `NULL` are each stored. The code is one of `I`, `D` and `S`, and a layout-`S` value is a date
/// at midnight.
async fn timestamp_layout_codes_are_enforced(pool: &PgPool) {
    assert_one_row(
        sqlx::query!(
            "INSERT INTO transactions (tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, \
             tran_amt, tran_merchant_id, tran_merchant_name, tran_merchant_city, \
             tran_merchant_zip, card_num, orig_ts, orig_ts_form, proc_ts, proc_ts_form, filler) \
             VALUES ('0000000000000001', '01', 1, 'SCHEMA', 'schema test', 0.00, 0, ' ', ' ', \
             ' ', '0000000000000000', TIMESTAMP '2022-06-10 19:27:53.123456', 'I', NULL, \
             NULL, ' ')"
        )
        .execute(pool)
        .await,
        "insert a transaction with a layout-I original timestamp",
    );
    one_sided_timestamp_pairs_are_refused(pool).await;
    assert_check(
        sqlx::query!("UPDATE transactions SET orig_ts_form = 'X'")
            .execute(pool)
            .await,
        "orig_ts_form X",
        Some("transactions_orig_ts_form_code_ck"),
    );
    assert_check(
        sqlx::query!(
            "UPDATE transactions SET proc_ts = TIMESTAMP '2022-06-10 00:00:00', proc_ts_form = 'X'"
        )
        .execute(pool)
        .await,
        "proc_ts_form X",
        Some("transactions_proc_ts_form_code_ck"),
    );
    assert_check(
        sqlx::query!("UPDATE transactions SET orig_ts_form = 'S'")
            .execute(pool)
            .await,
        "layout S on an orig_ts with a time of day",
        Some("transactions_orig_ts_screen_date_ck"),
    );
    assert_check(
        sqlx::query!(
            "UPDATE transactions SET proc_ts = TIMESTAMP '2022-06-10 19:27:53', proc_ts_form = 'S'"
        )
        .execute(pool)
        .await,
        "layout S on a proc_ts with a time of day",
        Some("transactions_proc_ts_screen_date_ck"),
    );
    assert_one_row(
        sqlx::query!("UPDATE transactions SET orig_ts_form = 'D'")
            .execute(pool)
            .await,
        "layout D",
    );
    assert_one_row(
        sqlx::query!(
            "UPDATE transactions SET proc_ts = TIMESTAMP '2022-06-10 00:00:00', proc_ts_form = 'S'"
        )
        .execute(pool)
        .await,
        "layout S at midnight",
    );
    assert_one_row(
        sqlx::query!("UPDATE transactions SET orig_ts = NULL, orig_ts_form = NULL")
            .execute(pool)
            .await,
        "a blank orig_ts as two NULLs",
    );
    assert_one_row(
        sqlx::query!("UPDATE transactions SET proc_ts = NULL, proc_ts_form = NULL")
            .execute(pool)
            .await,
        "a blank proc_ts as two NULLs",
    );
}

/// Migration 0006 seeds the Db2 data version as CREADB21 does, with its own spellings, leaves the
/// VSAM data version empty, and keeps `ON DELETE RESTRICT` on a type that has categories (E23).
async fn db2_version_is_seeded_and_restricted(pool: &PgPool, store: &mut PgStore) {
    let types = sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM db2.transaction_type"#)
        .fetch_one(pool)
        .await
        .expect("count db2.transaction_type");
    assert_eq!(types, 7, "db2.transaction_type: the DB2LTTYP.ctl seed rows");
    let categories =
        sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM db2.transaction_type_category"#)
            .fetch_one(pool)
            .await
            .expect("count db2.transaction_type_category");
    assert_eq!(
        categories, 18,
        "db2.transaction_type_category: the DB2LTCAT.ctl seed rows"
    );
    for (code, description) in [("01", "PURCHASE"), ("06", "REVERAL")] {
        let row = Db2TransactionTypeRepository::find_by_id(store, &type_code(code))
            .await
            .expect("read a seeded Db2 transaction type");
        assert_eq!(
            row.as_ref().map(Db2TransactionType::tr_description),
            Some(description),
            "db2.transaction_type {code}: the Db2 seed spelling"
        );
    }

    let vsam_types = TransactionTypeRepository::find_all(store)
        .await
        .expect("read transaction_types");
    assert!(
        vsam_types.is_empty(),
        "transaction_types: no row is copied from db2"
    );
    let vsam_categories =
        sqlx::query_scalar!(r#"SELECT count(*) AS "count!" FROM transaction_categories"#)
            .fetch_one(pool)
            .await
            .expect("count transaction_categories");
    assert_eq!(
        vsam_categories, 0,
        "transaction_categories: no row is copied from db2"
    );

    match Db2TransactionTypeRepository::delete(store, &type_code("01")).await {
        Err(err) => {
            assert!(err.is_restrict(), "deleting type 01: found {err}");
            assert_eq!(err.constraint(), Some("trc_type_code"), "deleting type 01");
        }
        Ok(deleted) => panic!("deleting type 01, which has categories, returned Ok({deleted})"),
    }
    let kept = Db2TransactionTypeRepository::find_by_id(store, &type_code("01"))
        .await
        .expect("read type 01 after the refused delete");
    assert!(
        kept.is_some(),
        "db2.transaction_type 01 survives the refused delete"
    );

    let unreferenced = Db2TransactionType::new(type_code("99"), "SCHEMA TEST".to_owned())
        .expect("a Db2 transaction type");
    Db2TransactionTypeRepository::insert(store, &unreferenced)
        .await
        .expect("insert type 99, which has no category");
    let deleted = Db2TransactionTypeRepository::delete(store, &type_code("99"))
        .await
        .expect("delete type 99");
    assert!(
        deleted,
        "RESTRICT refuses only a type that still has categories"
    );
}

/// Every table, column type, nullability and identity follows the copybook and DDL rules.
// audit: PIC-001
// audit: PIC-011
// audit: PIC-012
// audit: PIC-015
// audit: PIC-017
// audit: PIC-023
// audit: PIC-024
#[tokio::test]
async fn columns_follow_the_copybook_rules() {
    let guard = start_postgres()
        .await
        .expect("start a migrated postgres:16 container");
    let pool = guard.pool();
    let mut failures = Vec::new();
    check_inventory(pool, &mut failures).await;
    check_columns(pool, &mut failures).await;
    check_no_float_columns(pool, &mut failures).await;
    assert!(
        failures.is_empty(),
        "the schema departs from the copybook column rules:\n{}",
        failures.join("\n")
    );
}

/// Keys, indexes and foreign keys follow the KSDS keys, the alternate indexes, the IMS hierarchy
/// and the Db2 DDL, and every check constraint equals, in its complete definition, the range of
/// its integral picture or the timestamp layout rule it states.
// audit: PIC-003
// audit: PIC-017
// audit: PIC-018
// audit: PIC-019
// audit: PIC-020
#[tokio::test]
async fn keys_indexes_and_checks_follow_the_sources() {
    let guard = start_postgres()
        .await
        .expect("start a migrated postgres:16 container");
    let pool = guard.pool();
    let constraints = fetch_constraints(pool).await;
    let mut failures = Vec::new();
    check_keys(&constraints, &mut failures);
    check_constraints(&constraints, &mut failures);
    check_indexes(pool, &mut failures).await;
    assert!(
        failures.is_empty(),
        "the keys and constraints depart from their sources:\n{}",
        failures.join("\n")
    );
}

/// The declared constraints refuse what their pictures and sources refuse, through the
/// repository traits where one serves the statement.
// audit: PIC-003
// audit: PIC-017
// audit: PIC-018
// audit: PIC-019
// audit: PIC-020
// audit: CIPAUDTY-029
// audit: CVTRA03Y-002
#[tokio::test]
async fn constraints_behave_as_declared() {
    let guard = start_postgres()
        .await
        .expect("start a migrated postgres:16 container");
    let pool = guard.pool();
    let mut store = PgStore::new(pool.clone());
    account_key_range_is_enforced(pool, &mut store).await;
    summary_key_range_is_enforced(pool, &mut store).await;
    detail_hierarchy_is_enforced(&mut store).await;
    timestamp_layout_codes_are_enforced(pool).await;
    db2_version_is_seeded_and_restricted(pool, &mut store).await;
}
