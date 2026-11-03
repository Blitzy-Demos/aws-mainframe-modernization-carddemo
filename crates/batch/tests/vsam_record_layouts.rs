//! The VSAM record layouts of `docs/parity/parity-oracle-audit.md` held to the code: the
//! section A rows of `CVACT01Y`, `CVACT02Y`, `CVACT03Y`, `CVCUS01Y`, `CVTRA01Y` to `CVTRA06Y`
//! and `CSUSR01Y`, their seed counts and seeded records (section D) and their fixture
//! anomalies (section E).
//!
//! Every row is read from the audit at run time through the literal ID that cites it:
//!
//! - Layout rows against the `carddemo-codec` layouts: the field's name, offset, length and
//!   PIC class in `Layout::FIELDS`, the Rust type the decoded record holds it in, FILLER kept
//!   raw, every fixture record readable by its usage, and the table's lengths tiling the
//!   record.
//! - The same rows against the schema `batch seed` fills on an empty database: the column's
//!   type, its `CHECK` bounds evaluated on probe values, FILLER and the daily transaction
//!   record not stored, and each column holding its field of every source record.
//! - Seed counts and seeded records against that database, the card list and transaction
//!   view rules, and the output of `batch posttran` and `batch tranbkp`.
//! - The framing, content and init-record anomalies (E-04 to E-07, E-25, E-43) against the
//!   fixture bytes, the codec and the seeded state.
//!
//! `SEC-USR-PWD` has no probe here, because the code departs from its row: no `PasswordHash`
//! type holds it and its column is `TEXT`, not `VARCHAR`. Three rows name a type the domain
//! spells differently, and are held to the equivalence the domain documents: `ACCT-GROUP-ID`
//! and `DIS-ACCT-GROUP-ID` are held as `AccountGroupId`, the `X(10)` key whose item is the
//! `Alnum<10>` their rows name ([`key_item`]), and `CUST-SSN` as `CustSsn`, the type
//! `crates/domain/src/numeric/mod.rs` gives the row's `Ssn` ([`LABEL_ALIASES`]).
//!
//! The layout and anomaly tests read repository files only. The seed test needs Docker:
//! [`Harness::new`] starts a migrated `PostgreSQL` 16.15 container. Every statement it sends
//! is fixed text, run through `TestStack::exec_sql` and `TestStack::fetch_text_rows`; the
//! two `PL/pgSQL` functions it creates read each table and evaluate each `CHECK` on the
//! server.

pub mod common;

use std::any::type_name_of_val;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::str::FromStr;

use carddemo_audit::{
    CodeSpan, Document, KeyValue, Row, Table, cell_code_spans, citations, code_span, key_values,
};
use carddemo_codec::{
    AccountLayout, AccountRecord, AsciiFrame, CardLayout, CardRecord, CardXrefLayout,
    CardXrefRecord, Charset, CustomerLayout, CustomerRecord, DailyTranLayout, DailyTranRecord,
    DisclosureGroupLayout, DisclosureGroupRecord, FieldKind, FieldSpec, Layout, Terminator,
    TranCatBalanceLayout, TranCatBalanceRecord, TranCategoryFileRecord, TranCategoryLayout,
    TranTypeFileRecord, TranTypeLayout, TransactionLayout, TransactionRecord, UserSecurityLayout,
    UserSecurityRecord, decode_ascii_file, decode_ebcdic_file, encode_ascii_file, field,
    split_ascii,
};
use carddemo_domain::ErrorKind;
use carddemo_domain::auth::password::verify_as_entered;
use carddemo_domain::numeric::{AccountGroupId, Alnum, CustSsn};
use carddemo_domain::ports::Direction;
use carddemo_domain::rules::card_list::{self, CardListInput};
use carddemo_testkit::fixtures;
use common::{
    Harness, TRANREPT_CLOCK, block_on, ct01_view, expected_value, expected_values, fixture_lines,
    parity, parity_cell, parity_row, split_records,
};

/// The `Expected result` column of every parity audit table.
const EXPECTED: &str = "Expected result";

/// The `Copybook field or PIC clause` column: a field name, a space, its PIC clause.
const FIELD_CELL: &str = "Copybook field or PIC clause";

/// The ASCII trailing overpunch glyphs of a signed `DISPLAY` field: `{` and `A` to `I` for
/// +0 to +9, `}` and `J` to `R` for -0 to -9.
const OVERPUNCH: &[u8; 20] = b"{ABCDEFGHI}JKLMNOPQR";

/// The words of a FILLER row whose fixture bytes are all the character `0`.
const ZERO_FILLER: &str = "fixture bytes are '0' characters";

/// The `rust=` labels that `crates/domain/src/numeric/mod.rs` gives a type of another name,
/// each with that type and its digits: the row of `CUST-SSN 9(09)` labels it `Ssn`, which the
/// domain declares as `CustSsn`, a `9(09)` identifier of the same width.
const LABEL_ALIASES: [(&str, &str, usize); 1] = [("Ssn", "CustSsn", CustSsn::WIDTH)];

/// The seed count row of the user security file, whose `DUSRSECJ` citation locates the
/// `SYSUT1` records `batch seed` loads.
const USERS_SEED_COUNT: &str = "P-CSUSR01Y-007";

/// The seed count row of the transaction master, which names its table.
const TRANSACTIONS_SEED_COUNT: &str = "P-CVTRA05Y-015";

/// POSTTRAN's end-of-job counts and exit code over the fixture day.
const POSTTRAN_COUNTS: &str = "P-CBTRN02C-065";

/// The columns of the transaction master that no copybook field gives (E-09): the row
/// identity and the live-master flag, with their `information_schema` types.
const ROW_IDENTITY: [(&str, &str); 2] = [("transaction_id", "bigint"), ("in_master", "boolean")];

/// The processing timestamp POSTTRAN stamps from its clock, not from the daily record, so the
/// posted rows are compared with their daily records without it.
const CLOCK_COLUMNS: [&str; 1] = ["tran_proc_ts"];

/// Seeded records the audit names by key (section D.8): row, kind, table, key column.
const EXISTING: [(&str, &str, &str, &str); 4] = [
    (
        "P-CVACT01Y-015",
        "existing_account",
        "accounts",
        "account_id",
    ),
    ("P-CVACT02Y-009", "existing_card", "cards", "card_number"),
    (
        "P-CVCUS01Y-021",
        "existing_customer",
        "customers",
        "customer_id",
    ),
    (
        "P-CVTRA03Y-005",
        "processing_tran_type",
        "trantype_file",
        "tran_type",
    ),
];

/// Keys the audit names as absent from the seeded state (section D.8): row, kind, table, key
/// column.
const MISSING: [(&str, &str, &str, &str); 4] = [
    (
        "P-CVACT01Y-016",
        "missing_account",
        "accounts",
        "account_id",
    ),
    ("P-CVACT02Y-010", "missing_card", "cards", "card_number"),
    (
        "P-CVCUS01Y-022",
        "missing_customer",
        "customers",
        "customer_id",
    ),
    ("P-CSUSR01Y-010", "missing_user", "users", "user_id"),
];

/// Status rows of the seeded state (section D.8): row, kind, table, status column.
const STATUSES: [(&str, &str, &str, &str); 2] = [
    (
        "P-CVACT01Y-017",
        "account_status",
        "accounts",
        "acct_active_status",
    ),
    (
        "P-CVACT02Y-011",
        "card_status",
        "cards",
        "card_active_status",
    ),
];

/// The seeded users the audit names (section D.8): row and kind.
const USERS: [(&str, &str); 2] = [
    ("P-CSUSR01Y-008", "admin_user"),
    ("P-CSUSR01Y-009", "regular_user"),
];

/// Creates the two functions the seed test reads the database through, so that every
/// statement it sends is fixed text. `layout_cells` returns each column of each row of one
/// table as text; `jsonb` keeps the scale of a `NUMERIC`, so `194.00` stays `194.00`.
/// `layout_check_accepts` evaluates one `CHECK` expression on one probe value of its column,
/// with `NULL` passing as it does in a `CHECK`.
const FUNCTIONS_SQL: &str = "
CREATE FUNCTION layout_cells(relation text)
    RETURNS TABLE (row_no bigint, col text, val text) LANGUAGE plpgsql AS $$
BEGIN
    RETURN QUERY EXECUTE format(
        'SELECT r.n, e.key, e.value FROM (SELECT row_number() OVER () AS n, to_jsonb(t) AS j FROM %I AS t) AS r CROSS JOIN LATERAL jsonb_each_text(r.j) AS e',
        relation);
END
$$;
CREATE FUNCTION layout_check_accepts(expression text, col text, sql_type text, probe_value text)
    RETURNS boolean LANGUAGE plpgsql AS $$
DECLARE
    accepted boolean;
BEGIN
    EXECUTE format(
        'SELECT (%s) IS NOT FALSE FROM (SELECT CAST(%L AS %s) AS %I) AS probe',
        expression, probe_value, sql_type, col)
        INTO accepted;
    RETURN accepted;
END
$$;
";

/// Empties every table of the schema, leaving the migration bookkeeping table, whose name
/// starts with `_`, as it is: the empty database `batch seed` starts from.
const EMPTY_DATABASE_SQL: &str = "
DO $$
DECLARE
    relations text;
BEGIN
    SELECT string_agg(quote_ident(tablename), ', ') INTO relations
        FROM pg_tables WHERE schemaname = 'public' AND left(tablename, 1) <> '_';
    EXECUTE 'TRUNCATE TABLE ' || relations || ' RESTART IDENTITY CASCADE';
END
$$;
";

/// Every cell of every table of the schema but the migration bookkeeping: table, row number,
/// column, value.
const CELLS_SQL: &str = "SELECT p.tablename::text, c.row_no::text, c.col, c.val \
     FROM pg_tables AS p CROSS JOIN LATERAL layout_cells(p.tablename::text) AS c \
     WHERE p.schemaname = 'public' AND left(p.tablename::text, 1) <> '_'";

/// Every column of the schema with its type.
const COLUMNS_SQL: &str = "SELECT table_name, column_name, data_type, \
     character_maximum_length::text, numeric_precision::text, numeric_scale::text, is_identity \
     FROM information_schema.columns WHERE table_schema = 'public'";

/// Every integer column with a single-column `CHECK`, evaluated on -1 and on 10^k - 1 and
/// 10^k for k from 0 to 12: table, column, probe, whether all its checks accept it.
const INTEGER_PROBES_SQL: &str = "SELECT cl.relname::text, a.attname::text, v.probe::text, \
     bool_and(layout_check_accepts(pg_get_expr(c.conbin, c.conrelid), a.attname::text, \
     'bigint', v.probe::text))::text \
     FROM pg_constraint AS c \
     JOIN pg_class AS cl ON cl.oid = c.conrelid \
     JOIN pg_namespace AS n ON n.oid = cl.relnamespace \
     JOIN pg_attribute AS a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1] \
     CROSS JOIN (SELECT -1::bigint AS probe UNION \
     SELECT trunc(power(10::numeric, k))::bigint - d \
     FROM generate_series(0, 12) AS k CROSS JOIN (VALUES (0), (1)) AS s (d)) AS v \
     WHERE n.nspname = 'public' AND c.contype = 'c' AND cardinality(c.conkey) = 1 \
     AND a.atttypid IN ('integer'::regtype, 'bigint'::regtype) \
     GROUP BY cl.relname, a.attname, v.probe";

/// Every `VARCHAR` column with a single-column `CHECK`, evaluated on `A` and on `A` followed
/// by a space: table, column, probe, whether all its checks accept it.
const VARCHAR_PROBES_SQL: &str = "SELECT cl.relname::text, a.attname::text, v.probe, \
     bool_and(layout_check_accepts(pg_get_expr(c.conbin, c.conrelid), a.attname::text, \
     'character varying', v.probe))::text \
     FROM pg_constraint AS c \
     JOIN pg_class AS cl ON cl.oid = c.conrelid \
     JOIN pg_namespace AS n ON n.oid = cl.relnamespace \
     JOIN pg_attribute AS a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1] \
     CROSS JOIN (VALUES ('A'), ('A ')) AS v (probe) \
     WHERE n.nspname = 'public' AND c.contype = 'c' AND cardinality(c.conkey) = 1 \
     AND a.atttypid = 'character varying'::regtype \
     GROUP BY cl.relname, a.attname, v.probe";

// ---------------------------------------------------------------------------------------------
// The probes: what each decoded record holds for each cited section A row.
// ---------------------------------------------------------------------------------------------

/// What a decoded record holds for one section A row.
enum Held<'r> {
    /// A typed field: the path of its Rust type, as `std::any::type_name` gives it.
    Typed(&'static str),
    /// An `X(n)` key: the path of the `Alnum<n>` item it holds, and that item's bytes.
    Key(&'static str, &'r [u8]),
    /// FILLER: the bytes the record keeps.
    Raw(&'r [u8]),
}

/// One section A row, by the literal ID that cites it, and what a decoded record holds for it.
struct Probe<'r> {
    id: &'static str,
    held: Held<'r>,
}

/// The probe of row `id` for the typed field `value`.
fn typed<T>(id: &'static str, value: &T) -> Probe<'static> {
    Probe {
        id,
        held: Held::Typed(type_name_of_val(value)),
    }
}

/// The probe of row `id` for the FILLER bytes `filler`.
fn raw<'r, const N: usize>(id: &'static str, filler: &'r Alnum<N>) -> Probe<'r> {
    Probe {
        id,
        held: Held::Raw(filler.as_bytes()),
    }
}

/// The probe of row `id` for the disclosure group key `key`: the item the key holds
/// ([`AccountGroupId::as_alnum`]), which is the `Alnum<10>` its row names, and that item's
/// bytes, which every record must keep as stored.
fn key_item<'r>(id: &'static str, key: &'r AccountGroupId) -> Probe<'r> {
    let item = key.as_alnum();
    Probe {
        id,
        held: Held::Key(type_name_of_val(item), item.as_bytes()),
    }
}

/// `CVACT01Y` `ACCOUNT-RECORD`. `ACCT-GROUP-ID` is held as the key `AccountGroupId`, probed
/// through the `Alnum<10>` item it holds.
fn account_probes(record: &AccountRecord) -> Vec<Probe<'_>> {
    let account = &record.account;
    vec![
        typed("P-CVACT01Y-001", &account.acct_id),
        typed("P-CVACT01Y-002", &account.acct_active_status),
        typed("P-CVACT01Y-003", &account.acct_curr_bal),
        typed("P-CVACT01Y-004", &account.acct_credit_limit),
        typed("P-CVACT01Y-005", &account.acct_cash_credit_limit),
        typed("P-CVACT01Y-006", &account.acct_open_date),
        typed("P-CVACT01Y-007", &account.acct_expiraion_date),
        typed("P-CVACT01Y-008", &account.acct_reissue_date),
        typed("P-CVACT01Y-009", &account.acct_curr_cyc_credit),
        typed("P-CVACT01Y-010", &account.acct_curr_cyc_debit),
        typed("P-CVACT01Y-011", &account.acct_addr_zip),
        key_item("P-CVACT01Y-012", &account.acct_group_id),
        raw("P-CVACT01Y-013", &record.filler),
    ]
}

/// `CVACT02Y` `CARD-RECORD`.
fn card_probes(record: &CardRecord) -> Vec<Probe<'_>> {
    let card = &record.card;
    vec![
        typed("P-CVACT02Y-001", &card.card_num),
        typed("P-CVACT02Y-002", &card.card_acct_id),
        typed("P-CVACT02Y-003", &card.card_cvv_cd),
        typed("P-CVACT02Y-004", &card.card_embossed_name),
        typed("P-CVACT02Y-005", &card.card_expiraion_date),
        typed("P-CVACT02Y-006", &card.card_active_status),
        raw("P-CVACT02Y-007", &record.filler),
    ]
}

/// `CVACT03Y` `CARD-XREF-RECORD`.
fn xref_probes(record: &CardXrefRecord) -> Vec<Probe<'_>> {
    let xref = &record.xref;
    vec![
        typed("P-CVACT03Y-001", &xref.xref_card_num),
        typed("P-CVACT03Y-002", &xref.xref_cust_id),
        typed("P-CVACT03Y-003", &xref.xref_acct_id),
        raw("P-CVACT03Y-004", &record.filler),
    ]
}

/// `CVCUS01Y` `CUSTOMER-RECORD`. `CUST-SSN` is held as `CustSsn`, the type
/// [`LABEL_ALIASES`] gives its row's `Ssn`.
fn customer_probes(record: &CustomerRecord) -> Vec<Probe<'_>> {
    let customer = &record.customer;
    vec![
        typed("P-CVCUS01Y-001", &customer.cust_id),
        typed("P-CVCUS01Y-002", &customer.cust_first_name),
        typed("P-CVCUS01Y-003", &customer.cust_middle_name),
        typed("P-CVCUS01Y-004", &customer.cust_last_name),
        typed("P-CVCUS01Y-005", &customer.cust_addr_line_1),
        typed("P-CVCUS01Y-006", &customer.cust_addr_line_2),
        typed("P-CVCUS01Y-007", &customer.cust_addr_line_3),
        typed("P-CVCUS01Y-008", &customer.cust_addr_state_cd),
        typed("P-CVCUS01Y-009", &customer.cust_addr_country_cd),
        typed("P-CVCUS01Y-010", &customer.cust_addr_zip),
        typed("P-CVCUS01Y-011", &customer.cust_phone_num_1),
        typed("P-CVCUS01Y-012", &customer.cust_phone_num_2),
        typed("P-CVCUS01Y-013", &customer.cust_ssn),
        typed("P-CVCUS01Y-014", &customer.cust_govt_issued_id),
        typed("P-CVCUS01Y-015", &customer.cust_dob_yyyy_mm_dd),
        typed("P-CVCUS01Y-016", &customer.cust_eft_account_id),
        typed("P-CVCUS01Y-017", &customer.cust_pri_card_holder_ind),
        typed("P-CVCUS01Y-018", &customer.cust_fico_credit_score),
        raw("P-CVCUS01Y-019", &record.filler),
    ]
}

/// `CVTRA01Y` `TRAN-CAT-BAL-RECORD`.
fn balance_probes(record: &TranCatBalanceRecord) -> Vec<Probe<'_>> {
    let balance = &record.balance;
    vec![
        typed("P-CVTRA01Y-001", &balance.key.trancat_acct_id),
        typed("P-CVTRA01Y-002", &balance.key.trancat_type_cd),
        typed("P-CVTRA01Y-003", &balance.key.trancat_cd),
        typed("P-CVTRA01Y-004", &balance.tran_cat_bal),
        raw("P-CVTRA01Y-005", &record.filler),
    ]
}

/// `CVTRA02Y` `DIS-GROUP-RECORD`. `DIS-ACCT-GROUP-ID` is held as the key `AccountGroupId`,
/// probed through the `Alnum<10>` item it holds.
fn group_probes(record: &DisclosureGroupRecord) -> Vec<Probe<'_>> {
    let group = &record.group;
    vec![
        key_item("P-CVTRA02Y-001", &group.key.dis_acct_group_id),
        typed("P-CVTRA02Y-002", &group.key.dis_tran_type_cd),
        typed("P-CVTRA02Y-003", &group.key.dis_tran_cat_cd),
        typed("P-CVTRA02Y-004", &group.dis_int_rate),
        raw("P-CVTRA02Y-005", &record.filler),
    ]
}

/// `CVTRA03Y` `TRAN-TYPE-RECORD`.
fn tran_type_probes(record: &TranTypeFileRecord) -> Vec<Probe<'_>> {
    let tran_type = &record.tran_type;
    vec![
        typed("P-CVTRA03Y-001", &tran_type.tran_type),
        typed("P-CVTRA03Y-002", &tran_type.tran_type_desc),
        raw("P-CVTRA03Y-003", &record.filler),
    ]
}

/// `CVTRA04Y` `TRAN-CAT-RECORD`.
fn tran_category_probes(record: &TranCategoryFileRecord) -> Vec<Probe<'_>> {
    let category = &record.tran_category;
    vec![
        typed("P-CVTRA04Y-001", &category.tran_type_cd),
        typed("P-CVTRA04Y-002", &category.tran_cat_cd),
        typed("P-CVTRA04Y-003", &category.tran_cat_type_desc),
        raw("P-CVTRA04Y-004", &record.filler),
    ]
}

/// `CVTRA05Y` `TRAN-RECORD`.
fn transaction_probes(record: &TransactionRecord) -> Vec<Probe<'_>> {
    let tran = &record.transaction;
    vec![
        typed("P-CVTRA05Y-001", &tran.tran_id),
        typed("P-CVTRA05Y-002", &tran.tran_type_cd),
        typed("P-CVTRA05Y-003", &tran.tran_cat_cd),
        typed("P-CVTRA05Y-004", &tran.tran_source),
        typed("P-CVTRA05Y-005", &tran.tran_desc),
        typed("P-CVTRA05Y-006", &tran.tran_amt),
        typed("P-CVTRA05Y-007", &tran.tran_merchant_id),
        typed("P-CVTRA05Y-008", &tran.tran_merchant_name),
        typed("P-CVTRA05Y-009", &tran.tran_merchant_city),
        typed("P-CVTRA05Y-010", &tran.tran_merchant_zip),
        typed("P-CVTRA05Y-011", &tran.tran_card_num),
        typed("P-CVTRA05Y-012", &tran.tran_orig_ts),
        typed("P-CVTRA05Y-013", &tran.tran_proc_ts),
        raw("P-CVTRA05Y-014", &record.filler),
    ]
}

/// `CVTRA06Y` `DALYTRAN-RECORD`.
fn daily_probes(record: &DailyTranRecord) -> Vec<Probe<'_>> {
    let daily = &record.daily_tran;
    vec![
        typed("P-CVTRA06Y-001", &daily.dalytran_id),
        typed("P-CVTRA06Y-002", &daily.dalytran_type_cd),
        typed("P-CVTRA06Y-003", &daily.dalytran_cat_cd),
        typed("P-CVTRA06Y-004", &daily.dalytran_source),
        typed("P-CVTRA06Y-005", &daily.dalytran_desc),
        typed("P-CVTRA06Y-006", &daily.dalytran_amt),
        typed("P-CVTRA06Y-007", &daily.dalytran_merchant_id),
        typed("P-CVTRA06Y-008", &daily.dalytran_merchant_name),
        typed("P-CVTRA06Y-009", &daily.dalytran_merchant_city),
        typed("P-CVTRA06Y-010", &daily.dalytran_merchant_zip),
        typed("P-CVTRA06Y-011", &daily.dalytran_card_num),
        typed("P-CVTRA06Y-012", &daily.dalytran_orig_ts),
        typed("P-CVTRA06Y-013", &daily.dalytran_proc_ts),
        raw("P-CVTRA06Y-014", &record.filler),
    ]
}

/// `CSUSR01Y` `SEC-USER-DATA`. `SEC-USR-PWD` has no probe: no `PasswordHash` type holds it
/// and its column is `TEXT`, not the `VARCHAR` its row names.
fn user_probes(record: &UserSecurityRecord) -> Vec<Probe<'_>> {
    vec![
        typed("P-CSUSR01Y-001", &record.sec_usr_id),
        typed("P-CSUSR01Y-002", &record.sec_usr_fname),
        typed("P-CSUSR01Y-003", &record.sec_usr_lname),
        typed("P-CSUSR01Y-005", &record.sec_usr_type),
        raw("P-CSUSR01Y-006", &record.filler),
    ]
}

// ---------------------------------------------------------------------------------------------
// The copybooks and their source records.
// ---------------------------------------------------------------------------------------------

/// One copybook layout with the records it is checked on.
struct Copybook {
    /// `Layout::NAME`, the audit key of its rows.
    name: &'static str,
    /// `Layout::LEN`.
    len: usize,
    /// `Layout::FIELDS`.
    fields: &'static [FieldSpec],
    /// The source records, each padded with spaces to `len` bytes as the codec pads them.
    records: Vec<Vec<u8>>,
    /// The probed rows, in copybook order.
    probes: Vec<Probed>,
    /// The section D.7 row that counts the table `batch seed` fills, and names it.
    seed_count: Option<&'static str>,
}

/// What every record shows for one probed row.
struct Probed {
    id: &'static str,
    /// The Rust type path of a typed field, or of the item an `X(n)` key holds; `None` for
    /// FILLER.
    rust: Option<&'static str>,
    /// The FILLER bytes, or the key item's bytes, of each record, in record order; empty for
    /// any other typed field.
    raw: Vec<Vec<u8>>,
}

/// Decodes every record of `records` with `L` and collects what `probes` shows of each.
///
/// Panics, naming the record, if a record does not decode.
fn copybook<L: Layout>(
    records: Vec<Vec<u8>>,
    probes: fn(&L::Record) -> Vec<Probe<'_>>,
    seed_count: Option<&'static str>,
) -> Copybook {
    let mut slots: Vec<Probed> = Vec::new();
    for (number, bytes) in (1..).zip(&records) {
        let record = L::decode(bytes, Charset::Ascii)
            .unwrap_or_else(|error| panic!("{} record {number} does not decode: {error}", L::NAME));
        let shown = probes(&record);
        if slots.is_empty() {
            slots = shown
                .iter()
                .map(|probe| Probed {
                    id: probe.id,
                    rust: None,
                    raw: Vec::new(),
                })
                .collect();
        }
        for (slot, probe) in slots.iter_mut().zip(shown) {
            match probe.held {
                Held::Typed(path) => slot.rust = Some(path),
                Held::Key(path, item) => {
                    slot.rust = Some(path);
                    slot.raw.push(item.to_vec());
                }
                Held::Raw(filler) => slot.raw.push(filler.to_vec()),
            }
        }
    }
    Copybook {
        name: L::NAME,
        len: L::LEN,
        fields: L::FIELDS,
        records,
        probes: slots,
        seed_count,
    }
}

/// The eleven VSAM copybooks, each with its source records: its ASCII fixture file, the
/// daily transactions for both 350-byte transaction layouts, and the `DUSRSECJ` `SYSUT1`
/// records for the user security file.
fn copybooks(doc: &Document) -> Vec<Copybook> {
    vec![
        copybook::<AccountLayout>(
            ascii_records("acctdata.txt", AccountLayout::LEN),
            account_probes,
            Some("P-CVACT01Y-014"),
        ),
        copybook::<CardLayout>(
            ascii_records("carddata.txt", CardLayout::LEN),
            card_probes,
            Some("P-CVACT02Y-008"),
        ),
        copybook::<CardXrefLayout>(
            ascii_records("cardxref.txt", CardXrefLayout::LEN),
            xref_probes,
            Some("P-CVACT03Y-005"),
        ),
        copybook::<CustomerLayout>(
            ascii_records("custdata.txt", CustomerLayout::LEN),
            customer_probes,
            Some("P-CVCUS01Y-020"),
        ),
        copybook::<TranCatBalanceLayout>(
            ascii_records("tcatbal.txt", TranCatBalanceLayout::LEN),
            balance_probes,
            Some("P-CVTRA01Y-006"),
        ),
        copybook::<DisclosureGroupLayout>(
            ascii_records("discgrp.txt", DisclosureGroupLayout::LEN),
            group_probes,
            Some("P-CVTRA02Y-006"),
        ),
        copybook::<TranTypeLayout>(
            ascii_records("trantype.txt", TranTypeLayout::LEN),
            tran_type_probes,
            Some("P-CVTRA03Y-004"),
        ),
        copybook::<TranCategoryLayout>(
            ascii_records("trancatg.txt", TranCategoryLayout::LEN),
            tran_category_probes,
            Some("P-CVTRA04Y-005"),
        ),
        copybook::<TransactionLayout>(
            ascii_records("dailytran.txt", TransactionLayout::LEN),
            transaction_probes,
            Some(TRANSACTIONS_SEED_COUNT),
        ),
        copybook::<DailyTranLayout>(
            ascii_records("dailytran.txt", DailyTranLayout::LEN),
            daily_probes,
            None,
        ),
        copybook::<UserSecurityLayout>(dusrsecj_records(doc), user_probes, Some(USERS_SEED_COUNT)),
    ]
}

/// The copybook named `name`.
fn book<'b>(books: &'b [Copybook], name: &str) -> &'b Copybook {
    books
        .iter()
        .find(|book| book.name == name)
        .unwrap_or_else(|| panic!("no copybook {name}"))
}

/// The records of the ASCII fixture `name`, each without its line end and padded with spaces
/// to `len` bytes.
fn ascii_records(name: &str, len: usize) -> Vec<Vec<u8>> {
    fixture_lines(name)
        .iter()
        .map(|line| padded(line.strip_suffix(b"\r").unwrap_or(line), len))
        .collect()
}

/// The `DUSRSECJ` `SYSUT1` records the user seed count row cites, each padded to the 80
/// bytes of `SEC-USER-DATA`.
fn dusrsecj_records(doc: &Document) -> Vec<Vec<u8>> {
    let cited = citations(parity_cell(doc, USERS_SEED_COUNT, "Paragraph"));
    let [records] = cited.as_slice() else {
        panic!("{USERS_SEED_COUNT} cites {cited:?}, not one line range")
    };
    let jcl = fixtures::dusrsecj_jcl();
    assert!(
        jcl.ends_with(&records.path),
        "{USERS_SEED_COUNT} cites {}, not {}",
        records.path,
        jcl.display()
    );
    let text = fs::read_to_string(&jcl)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", jcl.display()));
    text.lines()
        .skip(records.start - 1)
        .take(records.end + 1 - records.start)
        .map(|line| padded(line.as_bytes(), UserSecurityLayout::LEN))
        .collect()
}

/// `body` padded with spaces to `len` bytes.
fn padded(body: &[u8], len: usize) -> Vec<u8> {
    assert!(
        body.len() <= len,
        "a stored record of {} bytes exceeds its {len}-byte layout",
        body.len()
    );
    let mut record = body.to_vec();
    record.resize(len, b' ');
    record
}

// ---------------------------------------------------------------------------------------------
// Reading the audit rows.
// ---------------------------------------------------------------------------------------------

/// The SQL storage a section A row states.
#[derive(Debug, PartialEq, Eq)]
enum Sql {
    /// `INTEGER` or `BIGINT CHECK (0 <= v < 10^digits)`, with the `information_schema` type.
    Integer {
        data_type: &'static str,
        digits: u32,
    },
    /// `VARCHAR(length)`, `right-trimmed` or not.
    Varchar { length: String, trimmed: bool },
    /// `NUMERIC(precision,scale)`.
    Numeric { precision: String, scale: String },
    /// `not stored`.
    NotStored,
}

impl Sql {
    /// The storage the `sql=` value `text` of row `id` names.
    fn parse(id: &str, text: &str) -> Self {
        if text.starts_with("not stored") {
            return Self::NotStored;
        }
        for (prefix, data_type) in [
            ("BIGINT CHECK (0 <= v < 10^", "bigint"),
            ("INTEGER CHECK (0 <= v < 10^", "integer"),
        ] {
            if let Some(digits) = text.strip_prefix(prefix).and_then(|r| r.strip_suffix(')')) {
                return Self::Integer {
                    data_type,
                    digits: parsed(id, digits),
                };
            }
        }
        if let Some((length, tail)) = text
            .strip_prefix("VARCHAR(")
            .and_then(|rest| rest.split_once(')'))
        {
            let trimmed = match tail {
                "" => false,
                " right-trimmed" => true,
                _ => panic!("{id}: unrecognised sql={text}"),
            };
            return Self::Varchar {
                length: length.to_owned(),
                trimmed,
            };
        }
        let numeric = text
            .strip_prefix("NUMERIC(")
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|inner| inner.split_once(','));
        let Some((precision, scale)) = numeric else {
            panic!("{id}: unrecognised sql={text}")
        };
        Self::Numeric {
            precision: precision.to_owned(),
            scale: scale.to_owned(),
        }
    }
}

/// A section A row as the audit states it, with the codec field it names.
struct Audited {
    id: &'static str,
    /// The copybook field name, for example `ACCT-ID`.
    name: String,
    /// The PIC clause, for example `9(11)`.
    pic: String,
    /// The codec specification of the field of that name.
    spec: FieldSpec,
    /// The `rust=` value.
    rust: String,
    /// The `sql=` value.
    sql: Sql,
    /// The `column=` value.
    column: String,
    /// The whole `Expected result` cell.
    expected: String,
}

/// Row `id` of `book`.
///
/// Panics if the row belongs to another copybook, names a field `book` lacks, or lacks a
/// `rust=`, `sql=` or `column=` value.
fn audited(doc: &Document, book: &Copybook, id: &'static str) -> Audited {
    assert_eq!(
        parity_cell(doc, id, "Program"),
        book.name,
        "{id} is not a row of {}",
        book.name
    );
    let (name, pic) = field_and_pic(id, parity_cell(doc, id, FIELD_CELL));
    let spec = *book
        .fields
        .iter()
        .find(|spec| spec.name == name)
        .unwrap_or_else(|| panic!("{id}: {} has no field {name}", book.name));
    let expected = parity_cell(doc, id, EXPECTED).to_owned();
    let pairs = key_values(&expected);
    Audited {
        id,
        name: name.to_owned(),
        pic: pic.to_owned(),
        spec,
        rust: pair_value(&pairs, "rust", id),
        sql: Sql::parse(id, &pair_value(&pairs, "sql", id)),
        column: pair_value(&pairs, "column", id),
        expected,
    }
}

/// The field name and the PIC clause of a `Copybook field or PIC clause` cell.
fn field_and_pic<'c>(id: &str, cell: &'c str) -> (&'c str, &'c str) {
    cell.split_once(' ')
        .unwrap_or_else(|| panic!("{id}: {cell:?} is not a field name and a PIC clause"))
}

/// The value of the first `key=` pair of `pairs`, the pairs of row `id`.
fn pair_value(pairs: &[KeyValue], key: &str, id: &str) -> String {
    pairs.iter().find(|pair| pair.key == key).map_or_else(
        || panic!("{id} expects no {key}="),
        |pair| pair.value.clone(),
    )
}

/// `text`, a value of row `id`, parsed.
fn parsed<T: FromStr>(id: &str, text: &str) -> T {
    text.parse()
        .unwrap_or_else(|_| panic!("{id}: {text:?} does not parse"))
}

/// The digits `text` starts with, for example `0` of `0 (E-07)`.
fn leading_digits(text: &str) -> &str {
    text.split(|ch: char| !ch.is_ascii_digit())
        .next()
        .unwrap_or_default()
}

/// The PIC class and byte width of the field `name` with the clause `pic`: `X(n)` is
/// alphanumeric (FILLER when the name is `FILLER` or ends `-FILLER`), `9(n)` unsigned and
/// `S9(n)V99` signed with the scale of its `V` digits.
fn pic_class(id: &str, name: &str, pic: &str) -> (FieldKind, usize) {
    let width = |inner: &str| -> usize { parsed(id, inner) };
    if let Some(inner) = pic
        .strip_prefix("X(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let filler = name == "FILLER" || name.ends_with("-FILLER");
        let kind = if filler {
            FieldKind::Filler
        } else {
            FieldKind::Alnum
        };
        return (kind, width(inner));
    }
    if let Some(inner) = pic
        .strip_prefix("9(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return (FieldKind::Unsigned, width(inner));
    }
    let signed = pic
        .strip_prefix("S9(")
        .and_then(|rest| rest.split_once(")V"));
    let Some((integer, fraction)) = signed else {
        panic!("{id}: unrecognised PIC {pic}")
    };
    assert!(
        !fraction.is_empty() && fraction.bytes().all(|byte| byte == b'9'),
        "{id}: unrecognised PIC {pic}"
    );
    let scale = u8::try_from(fraction.len())
        .unwrap_or_else(|_| panic!("{id}: PIC {pic} has too many decimals"));
    (FieldKind::Signed { scale }, width(integer) + fraction.len())
}

/// The `Input condition` the audit gives a field of PIC class `kind`.
const fn input_condition(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Alnum | FieldKind::Filler => "alphanumeric copy",
        FieldKind::Unsigned => "unsigned display digits 0-9",
        FieldKind::Signed { .. } => "signed: trailing overpunch",
    }
}

/// The last segment of a type path, for example `Alnum<10>` of
/// `carddemo_domain::numeric::alnum::Alnum<10>`.
fn type_tail(path: &str) -> &str {
    path.rsplit("::").next().unwrap_or(path)
}

/// The text of the exact-text span `span` of row `id`: its double-quoted string with `\"` and
/// `\\` unescaped, right-padded with spaces to its width when it has one.
fn exact(id: &str, span: &CodeSpan) -> String {
    let inner = span
        .text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{id}: {:?} is not a double-quoted exact text", span.text));
    let mut text = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escaped = chars.next();
            assert!(
                matches!(escaped, Some('"' | '\\')),
                "{id}: {:?} holds a stray backslash",
                span.text
            );
            text.extend(escaped);
        } else {
            text.push(ch);
        }
    }
    if let Some(width) = span.width {
        assert!(text.len() <= width, "{id}: {text:?} is wider than {width}");
        text.push_str(&" ".repeat(width - text.len()));
    }
    text
}

/// The stored form of the value `value` of row `id`: the exact text of its code span
/// right-trimmed, as every text column stores it, or the value as written.
fn stored_form(id: &str, value: &str) -> String {
    code_span(value).map_or_else(
        || value.to_owned(),
        |span| exact(id, &span).trim_end_matches(' ').to_owned(),
    )
}

/// Whether the stored text `stored` holds `expected`: equal digit strings compare as numbers,
/// so the audit's `00000000050` matches the integer column's `50`.
fn same_value(stored: &str, expected: &str) -> bool {
    let digits = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    if digits(stored) && digits(expected) {
        stored.trim_start_matches('0') == expected.trim_start_matches('0')
    } else {
        stored == expected
    }
}

/// `bytes` as ISO 8859-1 text, one character per byte.
fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&byte| char::from(byte)).collect()
}

// ---------------------------------------------------------------------------------------------
// Section A against the codec.
// ---------------------------------------------------------------------------------------------

/// Asserts the audit table of `book` against its layout: the same fields in the same order at
/// the same offsets and lengths, tiling the record length its heading names.
fn check_table(doc: &Document, book: &Copybook) {
    let first = book
        .probes
        .first()
        .unwrap_or_else(|| panic!("{} has no records to probe", book.name));
    let (table, _) = parity_row(doc, first.id);
    let audited: Vec<(String, usize, usize)> = table
        .rows
        .iter()
        .map(|row| table_field(table, row))
        .collect();
    let codec: Vec<(String, usize, usize)> = book
        .fields
        .iter()
        .map(|spec| (spec.name.to_owned(), spec.offset, spec.len))
        .collect();
    assert_eq!(
        audited, codec,
        "{}: audit table and Layout::FIELDS",
        book.name
    );
    let total: usize = audited.iter().map(|(_, _, length)| length).sum();
    assert_eq!(total, book.len, "{}: field lengths", book.name);
    assert!(
        field::fields_are_contiguous(book.fields, book.len),
        "{}: Layout::FIELDS do not tile the record",
        book.name
    );
    let heading = table.subsection.as_deref().unwrap_or_default();
    let length = format!(", {})", book.len);
    assert!(
        heading.starts_with(book.name) && heading.ends_with(&length),
        "{}: heading {heading:?} names another record or length",
        book.name
    );
}

/// The field name, offset and length of one row of a section A table.
fn table_field(table: &Table, row: &Row) -> (String, usize, usize) {
    let line = row.line;
    let cell = |column: &str| -> String {
        table
            .cell(row, column)
            .unwrap_or_else(|| panic!("audit line {line} has no {column:?} cell"))
            .to_owned()
    };
    let context = format!("audit line {line}");
    let field = cell(FIELD_CELL);
    let (name, _) = field_and_pic(&context, &field);
    (
        name.to_owned(),
        parsed(&context, &cell("Offset")),
        parsed(&context, &cell("Length")),
    )
}

/// Asserts one probed row against the codec field it names and every record of `book`.
fn check_layout_row(doc: &Document, book: &Copybook, probe: &Probed) {
    let row = audited(doc, book, probe.id);
    let id = row.id;
    let (kind, width) = pic_class(id, &row.name, &row.pic);
    let offset: usize = parsed(id, parity_cell(doc, id, "Offset"));
    let length: usize = parsed(id, parity_cell(doc, id, "Length"));
    assert_eq!(
        (row.spec.offset, row.spec.len, row.spec.kind),
        (offset, length, kind),
        "{id}: codec field {} (offset, length, PIC class)",
        row.name
    );
    assert_eq!(width, length, "{id}: PIC {} width", row.pic);
    assert_eq!(parity_cell(doc, id, "Usage"), "DISPLAY", "{id}: usage");
    assert_eq!(
        parity_cell(doc, id, "Input condition"),
        input_condition(kind),
        "{id}: input condition of a {kind:?} field"
    );
    if let Some(path) = probe.rust {
        let alias = LABEL_ALIASES.iter().find(|(label, ..)| *label == row.rust);
        if let Some((label, name, digits)) = alias {
            assert_eq!(
                (type_tail(path), *digits),
                (*name, length),
                "{id}: {} is held as {path}, the type and width the domain gives {label}",
                row.name
            );
        } else {
            assert_eq!(
                type_tail(path),
                row.rust,
                "{id}: {} is held as {path}",
                row.name
            );
        }
        if !probe.raw.is_empty() {
            check_key_item(book, &row, &probe.raw);
        }
    } else {
        assert_eq!(row.rust, "raw", "{id}: a FILLER row");
        check_raw(book, &row, &probe.raw);
    }
    check_stored_bytes(book, &row);
}

/// Asserts that the item of the `X(n)` key of `row` holds, in every record, exactly the bytes
/// stored at the field's offset: the key is its row's `Alnum<n>` without loss.
fn check_key_item(book: &Copybook, row: &Audited, items: &[Vec<u8>]) {
    assert_eq!(
        items.len(),
        book.records.len(),
        "{}: records probed",
        row.id
    );
    for (number, (item, stored)) in (1..).zip(items.iter().zip(&book.records)) {
        assert_eq!(
            item.as_slice(),
            &stored[row.spec.offset..row.spec.end()],
            "{}: record {number} key item differs from its stored bytes",
            row.id
        );
    }
}

/// Asserts that every record keeps the FILLER of `row` as its stored bytes, and that those
/// bytes are all `0` where the row says so.
fn check_raw(book: &Copybook, row: &Audited, fillers: &[Vec<u8>]) {
    assert_eq!(
        fillers.len(),
        book.records.len(),
        "{}: records probed",
        row.id
    );
    let zeros = row.expected.contains(ZERO_FILLER);
    for (number, (kept, stored)) in (1..).zip(fillers.iter().zip(&book.records)) {
        assert_eq!(
            kept.as_slice(),
            &stored[row.spec.offset..row.spec.end()],
            "{}: record {number} does not keep its FILLER raw",
            row.id
        );
        assert!(
            !zeros || kept.iter().all(|&byte| byte == b'0'),
            "{}: record {number} FILLER is not all '0'",
            row.id
        );
    }
}

/// Asserts that the field of `row` reads in every record by its usage: digits for `9(n)`,
/// digits and a trailing overpunch glyph for `S9(n)V99`.
fn check_stored_bytes(book: &Copybook, row: &Audited) {
    for (number, record) in (1..).zip(&book.records) {
        let bytes = &record[row.spec.offset..row.spec.end()];
        let readable = match row.spec.kind {
            FieldKind::Unsigned => bytes.iter().all(u8::is_ascii_digit),
            FieldKind::Signed { .. } => bytes.split_last().is_some_and(|(last, digits)| {
                digits.iter().all(u8::is_ascii_digit) && OVERPUNCH.contains(last)
            }),
            FieldKind::Alnum | FieldKind::Filler => true,
        };
        assert!(
            readable,
            "{}: record {number} holds {:?}, not a {} field",
            row.id,
            latin1(bytes),
            row.pic
        );
    }
}

/// Every cited section A row of the eleven VSAM copybooks names the codec field at its offset
/// and length, with its PIC class and Rust type, FILLER kept raw, and the table tiles the
/// record.
#[test]
fn every_vsam_layout_row_matches_its_codec_field() {
    let doc = parity();
    for book in copybooks(&doc) {
        check_table(&doc, &book);
        for probe in &book.probes {
            check_layout_row(&doc, &book, probe);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Section E against the fixture bytes.
// ---------------------------------------------------------------------------------------------

/// One E-04 framing row: the file, its records, their stored length, and how many of the first
/// records end CR LF (the rest end LF).
struct Framing {
    id: &'static str,
    file: &'static str,
    condition: &'static str,
    records: usize,
    stored_len: usize,
    crlf: usize,
}

/// Asserts `case` on its file: the frames as stored, then a decode with `L` that re-encodes
/// to the file's bytes. Returns the decoded records.
fn check_framing<L: Layout>(doc: &Document, case: &Framing) -> Vec<L::Record> {
    let id = case.id;
    assert_eq!(expected_value(doc, id, "escalation"), "E-04", "{id}");
    assert_eq!(
        parity_cell(doc, id, "Input condition"),
        case.condition,
        "{id}"
    );
    let path = fixtures::ascii_dir().join(case.file);
    let bytes =
        fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let frames: Vec<AsciiFrame> = split_ascii(&bytes)
        .into_iter()
        .map(|(frame, _)| frame)
        .collect();
    let expected: Vec<AsciiFrame> = (1..=case.records)
        .map(|number| AsciiFrame {
            stored_len: case.stored_len,
            terminator: if number <= case.crlf {
                Terminator::CrLf
            } else {
                Terminator::Lf
            },
        })
        .collect();
    assert_eq!(frames, expected, "{id}: frames of {}", case.file);
    let decoded = decode_ascii_file::<L>(&bytes)
        .unwrap_or_else(|error| panic!("{id}: {} does not decode: {error}", case.file));
    let encoded = encode_ascii_file::<L>(&decoded)
        .unwrap_or_else(|error| panic!("{id}: {} does not encode: {error}", case.file));
    assert!(
        encoded == bytes,
        "{id}: {} does not re-encode to its stored bytes",
        case.file
    );
    decoded.into_iter().map(|framed| framed.record).collect()
}

/// The E-04 rows: each record keeps its stored length and line end through the codec, and the
/// cross-reference records decode padded to 50 bytes.
#[test]
fn ascii_fixture_records_keep_their_stored_length_and_line_end() {
    let doc = parity();
    let xref = Framing {
        id: "P-CVACT03Y-007",
        file: "cardxref.txt",
        condition: "every cardxref.txt record",
        records: 50,
        stored_len: 36,
        crlf: 0,
    };
    let expected = parity_cell(&doc, xref.id, EXPECTED);
    assert!(
        expected.contains("36 bytes then LF") && expected.contains("pads to 50"),
        "{}: {expected}",
        xref.id
    );
    for record in check_framing::<CardXrefLayout>(&doc, &xref) {
        assert!(
            record.filler.as_bytes().iter().all(|&byte| byte == b' ')
                && xref.stored_len + record.filler.as_bytes().len() == CardXrefLayout::LEN,
            "{}: the absent FILLER does not decode as spaces up to 50 bytes",
            xref.id
        );
    }
    let _ = check_framing::<TranCategoryLayout>(
        &doc,
        &Framing {
            id: "P-CVTRA04Y-006",
            file: "trancatg.txt",
            condition: "all 18 records",
            records: 18,
            stored_len: TranCategoryLayout::LEN,
            crlf: 18,
        },
    );
    let _ = check_framing::<TranCatBalanceLayout>(
        &doc,
        &Framing {
            id: "P-CVTRA01Y-007",
            file: "tcatbal.txt",
            condition: "records 1-49 CR LF, record 50 LF",
            records: 50,
            stored_len: TranCatBalanceLayout::LEN,
            crlf: 49,
        },
    );
    let _ = check_framing::<TranTypeLayout>(
        &doc,
        &Framing {
            id: "P-CVTRA03Y-006",
            file: "trantype.txt",
            condition: "records 1-6 CR LF, record 7 LF",
            records: 7,
            stored_len: TranTypeLayout::LEN,
            crlf: 6,
        },
    );
}

/// The record number of an `Input condition` of the form `record <n>`.
fn record_number(id: &str, condition: &str) -> usize {
    let number = condition
        .strip_prefix("record ")
        .unwrap_or_else(|| panic!("{id}: {condition:?} names no record"));
    parsed(id, number)
}

/// The E-05 and E-06 account rows: record 49 holds its own ZIP in each corpus, the two EBCDIC
/// account files are one, and every group ID is blank with the ZIP at its PIC offset.
#[test]
fn account_zip_and_group_id_are_read_at_their_pic_offsets() {
    let doc = parity();
    let id = "P-CVACT01Y-018";
    assert_eq!(expected_value(&doc, id, "escalation"), "E-05", "{id}");
    let number = record_number(id, parity_cell(&doc, id, "Input condition"));
    let disposition = expected_value(&doc, id, "disposition");
    let spans = cell_code_spans(&disposition);
    let [ascii_zip, ebcdic_zip] = spans.as_slice() else {
        panic!("{id}: {disposition:?} holds no ASCII and EBCDIC ZIP")
    };
    let ascii = fs::read(fixtures::ascii_dir().join("acctdata.txt")).expect("acctdata.txt");
    let ascii = decode_ascii_file::<AccountLayout>(&ascii).expect("acctdata.txt decodes");
    let ebcdic_dir = fixtures::ebcdic_dir();
    let ebcdic = fs::read(ebcdic_dir.join("AWS.M2.CARDDEMO.ACCTDATA.PS")).expect("ACCTDATA.PS");
    let alias = fs::read(ebcdic_dir.join("AWS.M2.CARDDEMO.ACCDATA.PS")).expect("ACCDATA.PS");
    assert!(
        parity_cell(&doc, id, EXPECTED).contains("ACCDATA.PS is byte-identical to ACCTDATA.PS")
            && alias == ebcdic,
        "{id}: ACCDATA.PS and ACCTDATA.PS differ"
    );
    let ebcdic = decode_ebcdic_file::<AccountLayout>(&ebcdic).expect("ACCTDATA.PS decodes");
    let zips = (
        latin1(ascii[number - 1].record.account.acct_addr_zip.as_bytes()),
        latin1(ebcdic[number - 1].account.acct_addr_zip.as_bytes()),
    );
    assert_eq!(
        zips,
        (exact(id, ascii_zip), exact(id, ebcdic_zip)),
        "{id}: record {number} ZIP in ASCII and EBCDIC"
    );
    let id = "P-CVACT01Y-019";
    assert_eq!(expected_value(&doc, id, "escalation"), "E-06", "{id}");
    let disposition = expected_value(&doc, id, "disposition");
    let zip = code_span(&disposition).map_or_else(
        || panic!("{id}: {disposition:?} names no ZIP"),
        |span| exact(id, &span),
    );
    assert_eq!(ascii.len(), 50, "{id}: all 50 records");
    for (number, framed) in (1..).zip(&ascii) {
        let account = &framed.record.account;
        assert!(
            account
                .acct_group_id
                .as_bytes()
                .iter()
                .all(|&byte| byte == b' ')
                && latin1(account.acct_addr_zip.as_bytes()) == zip,
            "{id}: record {number} does not hold a blank group ID and the ZIP {zip:?}"
        );
    }
}

/// The E-07 and E-43 rows: the TRANFILE init record is 342 `X'00'` bytes, which no text
/// column can hold, then an EBCDIC count.
#[test]
fn tranfile_init_record_is_nul_bytes_then_an_ebcdic_count() {
    let doc = parity();
    let path = fixtures::ebcdic_dir().join("AWS.M2.CARDDEMO.DALYTRAN.PS.INIT");
    let bytes =
        fs::read(&path).unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
    let id = "P-CVTRA05Y-017";
    assert_eq!(expected_value(&doc, id, "escalation"), "E-07", "{id}");
    let declared = parity_cell(&doc, id, FIELD_CELL)
        .strip_prefix("TRAN-RECORD ")
        .and_then(|rest| rest.strip_suffix(" bytes"))
        .unwrap_or_else(|| panic!("{id}: no record length"));
    let declared: usize = parsed(id, declared);
    assert_eq!(
        (bytes.len(), declared),
        (TransactionLayout::LEN, TransactionLayout::LEN),
        "{id}: init record length"
    );
    let id = "P-CVTRA05Y-019";
    assert_eq!(expected_value(&doc, id, "escalation"), "E-43", "{id}");
    let condition = parity_cell(&doc, id, "Input condition");
    let (nuls, count) = condition
        .split_once(" X'00' bytes then EBCDIC ")
        .unwrap_or_else(|| panic!("{id}: {condition:?}"));
    let nuls: usize = parsed(id, nuls);
    assert!(
        bytes[..nuls].iter().all(|&byte| byte == 0),
        "{id}: the first {nuls} bytes are not X'00'"
    );
    assert_eq!(
        latin1(&Charset::Ebcdic.to_internal(&bytes[nuls..])),
        count,
        "{id}: the bytes after the X'00' run"
    );
}

// ---------------------------------------------------------------------------------------------
// The database after `batch seed`.
// ---------------------------------------------------------------------------------------------

/// One row of a table: its columns' text by column name.
type Cells = BTreeMap<String, String>;

/// Every row of every table.
struct Snapshot(BTreeMap<String, Vec<Cells>>);

impl Snapshot {
    /// Reads every row of every table through `layout_cells`.
    fn take(h: &Harness) -> Self {
        let mut rows: BTreeMap<(String, String), Cells> = BTreeMap::new();
        for cells in block_on(h.stack().fetch_text_rows(CELLS_SQL)) {
            let [table, row_no, column, value] = <[Option<String>; 4]>::try_from(cells)
                .unwrap_or_else(|cells| panic!("unexpected cell row {cells:?}"));
            let key = (required(table, "table"), required(row_no, "row number"));
            let row = rows.entry(key).or_default();
            if let Some(value) = value {
                row.insert(required(column, "column"), value);
            }
        }
        let mut tables: BTreeMap<String, Vec<Cells>> = BTreeMap::new();
        for ((table, _), row) in rows {
            tables.entry(table).or_default().push(row);
        }
        Self(tables)
    }

    /// The rows of `table`; none for an empty or unknown table.
    fn rows(&self, table: &str) -> &[Cells] {
        self.0.get(table).map(Vec::as_slice).unwrap_or_default()
    }

    /// The rows of `table` whose `column` holds `value`.
    fn find(&self, table: &str, column: &str, value: &str) -> Vec<&Cells> {
        self.rows(table)
            .iter()
            .filter(|row| row.get(column).is_some_and(|held| same_value(held, value)))
            .collect()
    }

    /// Whether no table holds a row.
    fn is_empty(&self) -> bool {
        self.0.values().all(Vec::is_empty)
    }
}

/// `value`, a column of a catalog read that is never null.
fn required(value: Option<String>, what: &str) -> String {
    value.unwrap_or_else(|| panic!("the catalog read returned no {what}"))
}

/// The text of `column` in `row`, for row `id`.
fn cell<'r>(row: &'r Cells, column: &str, id: &str) -> &'r str {
    row.get(column)
        .map_or_else(|| panic!("{id}: no {column} in {row:?}"), String::as_str)
}

/// The one row of `rows`, for row `id`.
fn only<'r>(rows: &[&'r Cells], id: &str) -> &'r Cells {
    let &[row] = rows else {
        panic!("{id}: {} rows found, expected one", rows.len())
    };
    row
}

/// One column as `information_schema.columns` reports it.
struct Column {
    data_type: String,
    max_length: Option<String>,
    precision: Option<String>,
    scale: Option<String>,
    identity: String,
}

/// The schema's columns and the verdicts of their single-column `CHECK`s on probe values.
struct Catalog {
    columns: BTreeMap<(String, String), Column>,
    /// Per (table, column): each probe value and whether the column's checks accept it.
    accepts: BTreeMap<(String, String), BTreeMap<String, bool>>,
}

impl Catalog {
    /// Reads the columns and evaluates the checks.
    fn read(h: &Harness) -> Self {
        let mut columns = BTreeMap::new();
        for row in block_on(h.stack().fetch_text_rows(COLUMNS_SQL)) {
            let [
                table,
                column,
                data_type,
                max_length,
                precision,
                scale,
                identity,
            ] = <[Option<String>; 7]>::try_from(row)
                .unwrap_or_else(|row| panic!("unexpected column row {row:?}"));
            let key = (required(table, "table"), required(column, "column"));
            let column = Column {
                data_type: required(data_type, "data type"),
                max_length,
                precision,
                scale,
                identity: required(identity, "identity flag"),
            };
            columns.insert(key, column);
        }
        let mut accepts: BTreeMap<(String, String), BTreeMap<String, bool>> = BTreeMap::new();
        for sql in [INTEGER_PROBES_SQL, VARCHAR_PROBES_SQL] {
            for row in block_on(h.stack().fetch_text_rows(sql)) {
                let [table, column, probe, accepted] = <[Option<String>; 4]>::try_from(row)
                    .unwrap_or_else(|row| panic!("unexpected probe row {row:?}"));
                accepts
                    .entry((required(table, "table"), required(column, "column")))
                    .or_default()
                    .insert(
                        required(probe, "probe"),
                        required(accepted, "verdict") == "true",
                    );
            }
        }
        Self { columns, accepts }
    }

    /// The column `table`.`column`, for row `id`.
    fn column(&self, table: &str, column: &str, id: &str) -> &Column {
        self.columns
            .get(&(table.to_owned(), column.to_owned()))
            .unwrap_or_else(|| panic!("{id}: {table} has no column {column}"))
    }

    /// Whether any table, or `table` alone when given, has a column `column`.
    fn has_column(&self, table: Option<&str>, column: &str) -> bool {
        self.columns
            .keys()
            .any(|(held_by, name)| name == column && table.is_none_or(|table| table == held_by))
    }
}

/// The table `batch seed` fills from `book`, as its seed count row names it.
fn seeded_table(doc: &Document, book: &Copybook) -> Option<String> {
    book.seed_count.map(|id| expected_value(doc, id, "table"))
}

/// The section D.7 rows: each table `batch seed` fills holds the rows of its source, and every
/// `<column>=<value> in every row` pair holds in every row.
fn check_seed_counts(doc: &Document, books: &[Copybook], seeded: &Snapshot) {
    for book in books {
        let Some(id) = book.seed_count else {
            continue;
        };
        let table = expected_value(doc, id, "table");
        let rows: usize = parsed(id, leading_digits(&expected_value(doc, id, "rows")));
        let stored = seeded.rows(&table);
        assert_eq!(stored.len(), rows, "{id}: rows of {table} after batch seed");
        if book.name != TransactionLayout::NAME {
            assert_eq!(book.records.len(), rows, "{id}: records of the source");
        }
        for pair in expected_values(doc, id) {
            if pair.key == "table" || pair.key == "rows" {
                continue;
            }
            let value = pair.value.strip_suffix(" in every row").unwrap_or_else(|| {
                panic!("{id}: {}={} is not a per-row value", pair.key, pair.value)
            });
            for row in stored {
                assert_eq!(
                    cell(row, &pair.key, id),
                    value,
                    "{id}: {table}.{}",
                    pair.key
                );
            }
        }
    }
}

/// The E-07 and E-43 rows against the seeded state: the init record is not loaded, so the
/// transaction master starts empty.
fn check_init_record_not_loaded(doc: &Document, seeded: &Snapshot) {
    let table = expected_value(doc, TRANSACTIONS_SEED_COUNT, "table");
    for id in ["P-CVTRA05Y-017", "P-CVTRA05Y-019"] {
        assert!(
            parity_cell(doc, id, EXPECTED).contains("table starts empty"),
            "{id}: disposition"
        );
        assert!(
            seeded.rows(&table).is_empty(),
            "{id}: {table} holds rows after batch seed"
        );
    }
}

/// The transaction master's two columns that no copybook field gives: the `BIGINT` identity
/// and the live-master flag the TRAN-ID row names.
fn check_row_identity(doc: &Document, catalog: &Catalog) {
    let id = "P-CVTRA05Y-001";
    let expected = parity_cell(doc, id, EXPECTED);
    assert!(
        expected.contains("row identity transaction_id BIGINT is separate")
            && expected.contains("live rows in_master"),
        "{id}: {expected}"
    );
    let table = expected_value(doc, TRANSACTIONS_SEED_COUNT, "table");
    for (column, data_type) in ROW_IDENTITY {
        assert_eq!(
            catalog.column(&table, column, id).data_type,
            data_type,
            "{id}: {table}.{column}"
        );
    }
    assert_eq!(
        catalog.column(&table, ROW_IDENTITY[0].0, id).identity,
        "YES",
        "{id}: {table}.{} is no identity",
        ROW_IDENTITY[0].0
    );
}

/// Asserts the probed rows of `book` against the schema: each stored field's column, its type
/// and its checks; each FILLER and unstored field without a column; and the table holding
/// exactly the columns its stored rows and row identity give.
fn check_schema(doc: &Document, book: &Copybook, catalog: &Catalog) {
    let table = seeded_table(doc, book);
    for probe in &book.probes {
        let row = audited(doc, book, probe.id);
        let id = row.id;
        if row.sql == Sql::NotStored {
            let field_column = row.name.to_ascii_lowercase().replace('-', "_");
            assert_eq!(row.column, "—", "{id}: an unstored field names a column");
            assert!(
                !catalog.has_column(table.as_deref(), &field_column),
                "{id}: {field_column} is stored"
            );
            continue;
        }
        let table = table
            .as_deref()
            .unwrap_or_else(|| panic!("{id}: a stored field of a copybook without a table"));
        check_type(&row, catalog.column(table, &row.column, id));
        check_constraints(&row, table, catalog);
    }
    if let Some(table) = table {
        check_column_count(doc, book, catalog, &table);
    }
}

/// Asserts the `information_schema` type of the column of `row`.
fn check_type(row: &Audited, column: &Column) {
    let id = row.id;
    let actual = (
        column.data_type.as_str(),
        column.max_length.as_deref(),
        column.precision.as_deref(),
        column.scale.as_deref(),
    );
    match &row.sql {
        Sql::Integer { data_type, .. } => {
            assert_eq!(actual.0, *data_type, "{id}: {} type", row.column);
        }
        Sql::Varchar { length, .. } => assert_eq!(
            (actual.0, actual.1),
            ("character varying", Some(length.as_str())),
            "{id}: {} type",
            row.column
        ),
        Sql::Numeric { precision, scale } => assert_eq!(
            (actual.0, actual.2, actual.3),
            ("numeric", Some(precision.as_str()), Some(scale.as_str())),
            "{id}: {} type",
            row.column
        ),
        Sql::NotStored => panic!("{id}: an unstored field has no column type"),
    }
}

/// Asserts the single-column checks of the column of `row`: an integer accepts exactly the
/// probes from 0 to 10^digits - 1, and a right-trimmed text rejects a trailing space.
fn check_constraints(row: &Audited, table: &str, catalog: &Catalog) {
    let id = row.id;
    let probes = catalog.accepts.get(&(table.to_owned(), row.column.clone()));
    match &row.sql {
        Sql::Integer { digits, .. } => {
            let probes =
                probes.unwrap_or_else(|| panic!("{id}: {table}.{} has no CHECK", row.column));
            let bound = 10_i128.pow(*digits);
            for edge in [-1, 0, bound - 1, bound] {
                assert!(
                    probes.contains_key(&edge.to_string()),
                    "{id}: probe {edge} not evaluated"
                );
            }
            for (probe, accepted) in probes {
                let value: i128 = parsed(id, probe);
                assert_eq!(
                    *accepted,
                    (0..bound).contains(&value),
                    "{id}: CHECK of {table}.{} on {value}",
                    row.column
                );
            }
        }
        Sql::Varchar { trimmed: true, .. } => {
            let probes =
                probes.unwrap_or_else(|| panic!("{id}: {table}.{} has no CHECK", row.column));
            assert_eq!(
                (probes.get("A"), probes.get("A ")),
                (Some(&true), Some(&false)),
                "{id}: {table}.{} accepts a trailing space",
                row.column
            );
        }
        Sql::Varchar { trimmed: false, .. } | Sql::Numeric { .. } | Sql::NotStored => {}
    }
}

/// Asserts that `table` has one column per stored row of the audit table of `book`, plus the
/// row identity of the transaction master, so no FILLER or other field has a column.
fn check_column_count(doc: &Document, book: &Copybook, catalog: &Catalog, table: &str) {
    let (audit, _) = parity_row(doc, book.probes[0].id);
    let stored = audit
        .rows
        .iter()
        .filter(|row| {
            key_values(audit.cell(row, EXPECTED).unwrap_or_default())
                .iter()
                .any(|pair| pair.key == "sql" && !pair.value.starts_with("not stored"))
        })
        .count();
    let extra = if book.name == TransactionLayout::NAME {
        ROW_IDENTITY.len()
    } else {
        0
    };
    let columns = catalog
        .columns
        .keys()
        .filter(|(held_by, _)| held_by == table)
        .count();
    assert_eq!(columns, stored + extra, "{}: columns of {table}", book.name);
}

/// The text the column of `row` stores for `record`: an integer without leading zeros, a
/// text right-trimmed when its row says so, a signed amount as `NUMERIC` text.
fn field_text(book: &Copybook, record: &[u8], row: &Audited) -> String {
    let bytes = &record[row.spec.offset..row.spec.end()];
    match &row.sql {
        Sql::Integer { .. } => parsed::<u64>(row.id, &latin1(bytes)).to_string(),
        Sql::Varchar { trimmed, .. } => {
            let text = latin1(bytes);
            if *trimmed {
                text.trim_end_matches(' ').to_owned()
            } else {
                text
            }
        }
        Sql::Numeric { scale, .. } => {
            let signed = field::signed(book.name, record, &row.spec)
                .unwrap_or_else(|error| panic!("{}: {error}", row.id));
            let units = signed
                .value()
                .unwrap_or_else(|error| panic!("{}: {error}", row.id));
            decimal_text(units, parsed(row.id, scale))
        }
        Sql::NotStored => panic!("{}: an unstored field has no column", row.id),
    }
}

/// `units` of the last of `scale` decimals as `NUMERIC` text, for example `-12.34`.
fn decimal_text(units: i64, scale: u32) -> String {
    let divisor = 10_u64.pow(scale);
    let magnitude = units.unsigned_abs();
    let sign = if units < 0 { "-" } else { "" };
    let (whole, fraction) = (magnitude / divisor, magnitude % divisor);
    let width = usize::try_from(scale).expect("a scale fits usize");
    format!("{sign}{whole}.{fraction:0width$}")
}

/// Asserts that the stored columns of `book`, `skip` aside, hold the fields of `records`, row
/// for record, as two equal sorted lists.
fn check_content(
    doc: &Document,
    book: &Copybook,
    rows: &[&Cells],
    records: &[&[u8]],
    skip: &[&str],
) {
    let stored: Vec<Audited> = book
        .probes
        .iter()
        .map(|probe| audited(doc, book, probe.id))
        .filter(|row| row.sql != Sql::NotStored && !skip.contains(&row.column.as_str()))
        .collect();
    let mut expected: Vec<Vec<String>> = records
        .iter()
        .map(|record| {
            stored
                .iter()
                .map(|row| field_text(book, record, row))
                .collect()
        })
        .collect();
    let mut actual: Vec<Vec<String>> = rows
        .iter()
        .map(|cells| {
            stored
                .iter()
                .map(|row| cell(cells, &row.column, row.id).to_owned())
                .collect()
        })
        .collect();
    expected.sort();
    actual.sort();
    let columns: Vec<&str> = stored.iter().map(|row| row.column.as_str()).collect();
    assert!(!expected.is_empty(), "{}: no records to compare", book.name);
    assert_eq!(
        expected.len(),
        actual.len(),
        "{}: rows of {columns:?}",
        book.name
    );
    if let Some((want, got)) = expected.iter().zip(&actual).find(|(want, got)| want != got) {
        panic!(
            "{}: columns {columns:?} hold {got:?}, not the fields {want:?}",
            book.name
        );
    }
}

/// The section D.8 rows that name a seeded record or an absent key.
fn check_named_records(doc: &Document, seeded: &Snapshot) {
    for (id, kind, table, key) in EXISTING {
        let pairs = expected_values(doc, id);
        assert_eq!(pair_value(&pairs, "kind", id), kind, "{id}");
        let value = stored_form(id, &pair_value(&pairs, "value", id));
        let found = seeded.find(table, key, &value);
        let row = only(&found, id);
        for pair in pairs
            .iter()
            .filter(|pair| pair.key != "kind" && pair.key != "value")
        {
            let expected = stored_form(id, &pair.value);
            let held = cell(row, &pair.key, id);
            assert!(
                same_value(held, &expected),
                "{id}: {table}.{} holds {held:?}, not {expected:?}",
                pair.key
            );
        }
    }
    for (id, kind, table, key) in MISSING {
        assert_eq!(expected_value(doc, id, "kind"), kind, "{id}");
        let value = stored_form(id, &expected_value(doc, id, "value"));
        assert!(
            seeded.find(table, key, &value).is_empty(),
            "{id}: {table} holds {key} {value}"
        );
    }
}

/// The processing copy's type description differs in case from the Db2 master's until
/// TRANEXTR copies the master over it.
fn check_master_case(doc: &Document, seeded: &Snapshot) {
    let id = "P-CVTRA03Y-005";
    let description = expected_value(doc, id, "tran_type_desc");
    assert!(
        description.contains("differs in case from the Db2 master until TRANEXTR runs"),
        "{id}: {description}"
    );
    let code = stored_form(id, &expected_value(doc, id, "value"));
    let processing = cell(
        only(&seeded.find("trantype_file", "tran_type", &code), id),
        "tran_type_desc",
        id,
    );
    let master = cell(
        only(&seeded.find("transaction_types", "tr_type", &code), id),
        "tr_description",
        id,
    );
    assert!(
        processing != master && processing.eq_ignore_ascii_case(master),
        "{id}: processing {processing:?} and master {master:?}"
    );
}

/// Asserts that every account of the seeded state has exactly one card, for row `id`.
fn check_one_card_per_account(id: &str, seeded: &Snapshot) {
    for account in seeded.rows("accounts") {
        let account_id = cell(account, "account_id", id);
        assert_eq!(
            seeded.find("cards", "card_acct_id", account_id).len(),
            1,
            "{id}: cards of account {account_id}"
        );
    }
}

/// The section D.8 status rows: every account and card active, and one card per account.
fn check_statuses(doc: &Document, seeded: &Snapshot) {
    for (id, kind, table, column) in STATUSES {
        assert_eq!(expected_value(doc, id, "kind"), kind, "{id}");
        let value = expected_value(doc, id, "value");
        let words: Vec<&str> = value.split_whitespace().collect();
        let [status, "for", "all", count, _] = words.as_slice() else {
            panic!("{id}: {value:?} is not <status> for all <n> <records>")
        };
        let rows = seeded.rows(table);
        assert_eq!(
            rows.len(),
            parsed::<usize>(id, count),
            "{id}: rows of {table}"
        );
        for row in rows {
            assert_eq!(cell(row, column, id), *status, "{id}: {table}.{column}");
        }
        let inactive = expected_values(doc, id)
            .into_iter()
            .find(|pair| pair.key.starts_with("inactive_"))
            .unwrap_or_else(|| panic!("{id} names no inactive records"));
        assert_eq!(inactive.value, "none", "{id}: {}", inactive.key);
    }
    let id = "P-CVACT02Y-011";
    assert_eq!(
        expected_value(doc, id, "cards_per_account"),
        "1 for every account",
        "{id}"
    );
    check_one_card_per_account(id, seeded);
}

/// The section D.8 cross-reference row: the first card's customer and account, one
/// cross-reference per card and one card per account.
fn check_cross_reference(doc: &Document, seeded: &Snapshot) {
    let id = "P-CVACT03Y-006";
    assert_eq!(expected_value(doc, id, "kind"), "existing_xref", "{id}");
    let card = expected_value(doc, id, "card_number");
    let found = seeded.find("card_xref", "xref_card_num", &card);
    let xref = only(&found, id);
    for (column, key) in [
        ("xref_cust_id", "customer_id"),
        ("xref_acct_id", "account_id"),
    ] {
        let expected = expected_value(doc, id, key);
        assert!(
            same_value(cell(xref, column, id), &expected),
            "{id}: {column} of card {card} is not {expected}"
        );
    }
    assert!(
        parity_cell(doc, id, EXPECTED).contains(
            "every card has exactly one cross-reference and every account exactly one card"
        ),
        "{id}: cardinality"
    );
    for card in seeded.rows("cards") {
        let number = cell(card, "card_number", id);
        assert_eq!(
            seeded.find("card_xref", "xref_card_num", number).len(),
            1,
            "{id}: cross-references of card {number}"
        );
    }
    check_one_card_per_account(id, seeded);
}

/// The users and the password every one of them signs on with, from the `seeded_users`
/// value of row `id`, for example `ADMIN001-ADMIN005 (A), USER0001-USER0005 (U), every
/// password "PASSWORD"`.
fn seeded_users(id: &str, value: &str) -> (BTreeMap<String, String>, String) {
    let mut users = BTreeMap::new();
    let mut password = None;
    for part in value.split(", ") {
        if part.starts_with("every password") {
            password = code_span(part).map(|span| exact(id, &span));
            continue;
        }
        let (range, kind) = part
            .split_once(" (")
            .and_then(|(range, kind)| Some((range, kind.strip_suffix(')')?)))
            .unwrap_or_else(|| panic!("{id}: {part:?} is not <first>-<last> (<type>)"));
        let (first, last) = range
            .split_once('-')
            .unwrap_or_else(|| panic!("{id}: {range:?} is not a range"));
        let prefix = first.trim_end_matches(|ch: char| ch.is_ascii_digit());
        let from = &first[prefix.len()..];
        let to = last
            .strip_prefix(prefix)
            .unwrap_or_else(|| panic!("{id}: {range:?} changes its prefix"));
        let width = from.len();
        for number in parsed::<u32>(id, from)..=parsed::<u32>(id, to) {
            users.insert(format!("{prefix}{number:0width$}"), kind.to_owned());
        }
    }
    (
        users,
        password.unwrap_or_else(|| panic!("{id}: {value:?} names no password")),
    )
}

/// Whether the stored argon2id PHC string `hash` verifies `password` as stored, for row `id`.
fn verifies(id: &str, password: &str, hash: &str) -> bool {
    verify_as_entered(password, hash).unwrap_or_else(|error| panic!("{id}: {error}"))
}

/// The section D.8 user rows: the two named users with their type, names and stored password,
/// and the ten seeded users, each signing on with the stored password.
fn check_users(doc: &Document, seeded: &Snapshot) {
    for (id, kind) in USERS {
        let pairs = expected_values(doc, id);
        assert_eq!(pair_value(&pairs, "kind", id), kind, "{id}");
        let user_id = stored_form(id, &pair_value(&pairs, "value", id));
        let found = seeded.find("users", "user_id", &user_id);
        let user = only(&found, id);
        for (column, key) in [
            ("sec_usr_type", "usr_type"),
            ("sec_usr_fname", "first_name"),
            ("sec_usr_lname", "last_name"),
        ] {
            let expected = stored_form(id, &pair_value(&pairs, key, id));
            assert_eq!(
                cell(user, column, id),
                expected,
                "{id}: {column} of {user_id}"
            );
        }
        let stored = pair_value(&pairs, "password_as_stored", id);
        let password = code_span(&stored).map_or_else(
            || panic!("{id}: {stored:?} is no exact text"),
            |span| exact(id, &span),
        );
        assert!(
            verifies(id, &password, cell(user, "password_hash", id)),
            "{id}: {user_id} does not verify {password:?}"
        );
    }
    let id = "P-CSUSR01Y-010";
    let (expected, password) = seeded_users(id, &expected_value(doc, id, "seeded_users"));
    let users = seeded.rows("users");
    let actual: BTreeMap<String, String> = users
        .iter()
        .map(|user| {
            (
                cell(user, "user_id", id).to_owned(),
                cell(user, "sec_usr_type", id).to_owned(),
            )
        })
        .collect();
    assert_eq!(actual, expected, "{id}: seeded users and their types");
    for user in users {
        assert!(
            verifies(id, &password, cell(user, "password_hash", id)),
            "{id}: {} does not verify {password:?}",
            cell(user, "user_id", id)
        );
    }
}

/// The values of a list such as `a,b,c (rows 1 to 7)`.
fn listed(value: &str) -> Vec<String> {
    value
        .split_once(" (rows")
        .map_or(value, |(list, _)| list)
        .split(',')
        .map(str::to_owned)
        .collect()
}

/// The section D.8 card page row: the first page of the unfiltered card list, as the CCLI
/// rules read it from the seeded cards.
fn check_card_page(doc: &Document, h: &Harness) {
    let id = "P-CVACT02Y-012";
    assert_eq!(
        expected_value(doc, id, "kind"),
        "unfiltered_card_page",
        "{id}"
    );
    let db = h.db();
    let page = block_on(async {
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("{id}: cannot begin a unit of work: {error}"));
        let input = CardListInput {
            account_filter: None,
            card_filter: None,
            start_key: None,
            direction: Direction::Forward,
            page_size: card_list::SCREEN_ROWS,
        };
        let page = card_list::list(&mut uow.store(), input).await;
        if let Err(error) = uow.rollback().await {
            panic!("{id}: rollback failed: {error}");
        }
        page
    })
    .unwrap_or_else(|error| panic!("{id}: the first card page failed: {error}"));
    let rows: usize = parsed(id, &expected_value(doc, id, "rows"));
    let numbers: Vec<String> = page
        .items
        .iter()
        .map(|card| card.card_num.trimmed().into_owned())
        .collect();
    let accounts: Vec<String> = page
        .items
        .iter()
        .map(|card| card.card_acct_id.to_string())
        .collect();
    assert_eq!(page.items.len(), rows, "{id}: cards on the page");
    assert_eq!(
        numbers,
        listed(&expected_value(doc, id, "card_number")),
        "{id}"
    );
    assert_eq!(
        accounts,
        listed(&expected_value(doc, id, "card_acct_id")),
        "{id}"
    );
    let status = expected_value(doc, id, "card_active_status");
    let active = status
        .strip_suffix(&format!(" for all {rows}"))
        .unwrap_or_else(|| panic!("{id}: {status:?}"));
    for card in &page.items {
        assert_eq!(
            card.card_active_status.trimmed(),
            active,
            "{id}: card status"
        );
    }
}

/// The E-05 and E-06 rows against the seeded state: the ASCII ZIP of record 49 is the one
/// seeded, every group ID is blank with the ZIP at its offset, and no disclosure group has a
/// blank ID while `DEFAULT` covers every category balance, so INTCALC falls back to `DEFAULT`.
fn check_group_ids(doc: &Document, books: &[Copybook], seeded: &Snapshot) {
    let id = "P-CVACT01Y-018";
    let number = record_number(id, parity_cell(doc, id, "Input condition"));
    let spans = cell_code_spans(&expected_value(doc, id, "disposition"));
    let ascii_zip = spans
        .first()
        .map_or_else(|| panic!("{id}: no ASCII ZIP"), |span| exact(id, span));
    let accounts = book(books, AccountLayout::NAME);
    let acct_id = accounts.fields.first().expect("ACCT-ID leads the record");
    let record = latin1(&accounts.records[number - 1][acct_id.offset..acct_id.end()]);
    let found = seeded.find("accounts", "account_id", &record);
    assert_eq!(
        cell(only(&found, id), "acct_addr_zip", id),
        ascii_zip,
        "{id}: seeded ZIP"
    );
    let id = "P-CVACT01Y-019";
    let disposition = expected_value(doc, id, "disposition");
    let zip =
        code_span(&disposition).map_or_else(|| panic!("{id}: no ZIP"), |span| exact(id, &span));
    let fallback = disposition
        .strip_prefix("read by PIC offsets: ")
        .and_then(|rest| rest.rsplit_once("falls back to "))
        .map_or_else(
            || panic!("{id}: {disposition:?} names no fallback group"),
            |(_, group)| group,
        );
    for account in seeded.rows("accounts") {
        assert_eq!(cell(account, "acct_group_id", id), "", "{id}: group ID");
        assert_eq!(cell(account, "acct_addr_zip", id), zip, "{id}: ZIP");
    }
    let groups = seeded.rows("disclosure_groups");
    assert!(
        groups
            .iter()
            .all(|group| !cell(group, "dis_acct_group_id", id).is_empty()),
        "{id}: a disclosure group matches the blank group ID"
    );
    for balance in seeded.rows("transaction_category_balances") {
        let key = (
            cell(balance, "trancat_type_cd", id),
            cell(balance, "trancat_cd", id),
        );
        assert!(
            groups.iter().any(|group| {
                cell(group, "dis_acct_group_id", id) == fallback
                    && (
                        cell(group, "dis_tran_type_cd", id),
                        cell(group, "dis_tran_cat_cd", id),
                    ) == key
            }),
            "{id}: no {fallback} rate for type and category {key:?}"
        );
    }
}

/// The kinds of row `id`, each with the pairs that follow it up to the next `kind=`.
fn kinds(doc: &Document, id: &str) -> BTreeMap<String, Vec<KeyValue>> {
    let mut kinds: BTreeMap<String, Vec<KeyValue>> = BTreeMap::new();
    let mut current = None;
    for pair in expected_values(doc, id) {
        if pair.key == "kind" {
            current = Some(pair.value.clone());
            kinds.entry(pair.value).or_default();
        } else if let Some(kind) = &current {
            kinds.entry(kind.clone()).or_default().push(pair);
        } else {
            panic!("{id}: {}={} precedes every kind", pair.key, pair.value);
        }
    }
    kinds
}

/// The first `key=` value of the pairs of `kind` in row `id`.
fn kind_value(kinds: &BTreeMap<String, Vec<KeyValue>>, kind: &str, key: &str, id: &str) -> String {
    let pairs = kinds
        .get(kind)
        .unwrap_or_else(|| panic!("{id} has no kind={kind}"));
    pair_value(pairs, key, id)
}

/// Runs POSTTRAN over the fixture day, then asserts the posted rows against their daily
/// records and the transaction view of the TRAN-ID row. Returns the number of posted rows.
fn check_posted_transactions(doc: &Document, books: &[Copybook], h: &Harness) -> usize {
    let exit: i32 = parsed(
        POSTTRAN_COUNTS,
        &expected_value(doc, POSTTRAN_COUNTS, "exit"),
    );
    let posted: usize = parsed(
        POSTTRAN_COUNTS,
        &expected_value(doc, POSTTRAN_COUNTS, "posted"),
    );
    let _ = h.run_expect(&["posttran"], Some(TRANREPT_CLOCK), exit);
    let after = Snapshot::take(h);
    let transactions = book(books, TransactionLayout::NAME);
    let table = expected_value(doc, TRANSACTIONS_SEED_COUNT, "table");
    let live: Vec<&Cells> = after
        .rows(&table)
        .iter()
        .filter(|row| row.get(ROW_IDENTITY[1].0).map(String::as_str) == Some("true"))
        .collect();
    assert_eq!(
        live.len(),
        posted,
        "{POSTTRAN_COUNTS}: live rows of {table}"
    );
    let tran_ids: BTreeSet<&str> = live
        .iter()
        .map(|row| cell(row, "tran_id", TRANSACTIONS_SEED_COUNT))
        .collect();
    let key = transactions
        .fields
        .first()
        .expect("TRAN-ID leads the record");
    let records: Vec<&[u8]> = transactions
        .records
        .iter()
        .map(Vec::as_slice)
        .filter(|record| tran_ids.contains(latin1(&record[key.offset..key.end()]).as_str()))
        .collect();
    check_content(doc, transactions, &live, &records, &CLOCK_COLUMNS);
    let id = "P-CVTRA05Y-016";
    let kinds = kinds(doc, id);
    let existing = stored_form(
        id,
        &kind_value(&kinds, "existing_tran_after_posttran", "value", id),
    );
    let viewed = ct01_view(&h.db(), &existing)
        .unwrap_or_else(|error| panic!("{id}: CT01 of {existing}: {error}"));
    assert_eq!(viewed.tran_id.trimmed(), existing, "{id}: CT01 TRAN-ID");
    let missing = stored_form(id, &kind_value(&kinds, "missing_tran", "value", id));
    let error = ct01_view(&h.db(), &missing).expect_err("CT01 finds the missing TRAN-ID");
    assert_eq!(error.kind, ErrorKind::NotFound, "{id}: CT01 of {missing}");
    posted
}

/// The TRAN-ID row's state after seeding: no transaction before POSTTRAN runs.
fn check_transactions_after_seed(doc: &Document, seeded: &Snapshot) {
    let id = "P-CVTRA05Y-016";
    let kinds = kinds(doc, id);
    let rows: usize = parsed(
        id,
        leading_digits(&kind_value(&kinds, "transactions_after_seed", "rows", id)),
    );
    let table = expected_value(doc, TRANSACTIONS_SEED_COUNT, "table");
    assert_eq!(
        seeded.rows(&table).len(),
        rows,
        "{id}: {table} after batch seed"
    );
}

/// The E-25 row: the `TRAN-RECORD`s the replacement writes, here the TRANBKP backup of the
/// posted master, hold spaces in the `FILLER` the COBOL never sets.
fn check_backup_filler(doc: &Document, h: &Harness, posted: usize) {
    let id = "P-CVTRA05Y-018";
    assert_eq!(expected_value(doc, id, "escalation"), "E-25", "{id}");
    let expected = parity_cell(doc, id, EXPECTED);
    assert!(
        expected.ends_with("replacement writes spaces"),
        "{id}: {expected}"
    );
    // A TRANBKP REPRO of a live master that holds records ends with condition code 0.
    let _ = h.run_expect(&["tranbkp"], Some(TRANREPT_CLOCK), 0);
    let records = split_records(&h.newest("TRANSACT.BKUP"), TransactionLayout::LEN);
    assert_eq!(records.len(), posted, "{id}: backed-up records");
    let filler = TransactionLayout::FIELDS
        .iter()
        .find(|spec| spec.kind == FieldKind::Filler)
        .expect("TRAN-RECORD has a FILLER");
    for (number, record) in (1..).zip(&records) {
        assert!(
            record.as_bytes()[filler.offset..filler.end()]
                .iter()
                .all(|&byte| byte == b' '),
            "{id}: record {number} FILLER is not spaces"
        );
    }
}

/// The section A, D and E rows against the database `batch seed` fills on an empty database,
/// then against POSTTRAN and TRANBKP run on it.
#[test]
fn batch_seed_stores_every_audited_field_and_record() {
    let doc = parity();
    let books = copybooks(&doc);
    let h = Harness::new();
    block_on(h.stack().exec_sql(FUNCTIONS_SQL));
    block_on(h.stack().exec_sql(EMPTY_DATABASE_SQL));
    assert!(
        Snapshot::take(&h).is_empty(),
        "rows are left before batch seed"
    );
    let _ = h.run_expect(&["seed"], None, 0);
    let seeded = Snapshot::take(&h);
    let catalog = Catalog::read(&h);
    check_seed_counts(&doc, &books, &seeded);
    check_init_record_not_loaded(&doc, &seeded);
    check_transactions_after_seed(&doc, &seeded);
    check_row_identity(&doc, &catalog);
    for book in &books {
        check_schema(&doc, book, &catalog);
        let table = seeded_table(&doc, book).filter(|_| book.name != TransactionLayout::NAME);
        if let Some(table) = table {
            let rows: Vec<&Cells> = seeded.rows(&table).iter().collect();
            let records: Vec<&[u8]> = book.records.iter().map(Vec::as_slice).collect();
            check_content(&doc, book, &rows, &records, &[]);
        }
    }
    check_named_records(&doc, &seeded);
    check_master_case(&doc, &seeded);
    check_statuses(&doc, &seeded);
    check_cross_reference(&doc, &seeded);
    check_users(&doc, &seeded);
    check_card_page(&doc, &h);
    check_group_ids(&doc, &books, &seeded);
    let posted = check_posted_transactions(&doc, &books, &h);
    check_backup_filler(&doc, &h, posted);
}
