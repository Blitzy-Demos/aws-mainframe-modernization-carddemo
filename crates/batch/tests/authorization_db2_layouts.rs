//! Section A layouts and section D seeded state of the authorization records and the Db2
//! reference tables in `docs/parity/parity-oracle-audit.md`.
//!
//! The rows cover the IMS segments `CIPAUSMY` and `CIPAUDTY`, stored as
//! `pending_auth_summaries` and `pending_auth_details`; the CP00 request, reply and error-log
//! fields `CCPAURQY`, `CCPAURLY` and `CCPAUERY`; and the Db2 tables `TRNTYPE`, `TRNTYCAT` and
//! `AUTHFRDS`, stored as `transaction_types`, `transaction_categories` and `auth_fraud`
//! (`migrations/0001_reference_data.sql`, `migrations/0004_authorizations.sql`). Every row is
//! read at run time through `carddemo_audit`: the literal row ID in each list below is both
//! its citation and its lookup key, and every value an assertion expects is the audit's.
//!
//! Each row is held to the code that realises it:
//!
//! - `rust=`: the type of the field that holds the value, named and measured at compile time
//!   through [`SectionAType`]. Where `crates/domain/src/numeric/mod.rs` binds a row's label to
//!   a type of another name, [`LABEL_ALIASES`] gives that type.
//! - `sql=` and `column=`: the catalog of a migrated test database; probes of its `CHECK`
//!   constraints (digit ranges, the right-trimmed form, the occurrences of an array); and one
//!   record per table written and read back through the persistence adapters, which shows the
//!   column each field is stored in and the form it is stored in.
//! - `Offset`, `Length` and `Usage`: the widths of those types, the packed and binary sizes
//!   they give, and, for the IMS segments, the column order of the migration.
//! - The CP00 layouts: the `UNSTRING` receivers of `auth_decision::parse_request`, the reply
//!   text of `auth_decision::reply_body` and the fields of `auth_decision::ErrorLogEntry`.
//! - Fields the code keeps in another form than the `Alnum<n>` their row names: the Db2
//!   `TR_DESCRIPTION` as text of its own length, and the error-log stamps, literals and flags
//!   until the record is logged. Each is held to that item through a conversion without loss:
//!   every value fills the item and reads back from it unchanged, and a flag's byte is the
//!   `CCPAUERY` 88-level value it stands for.
//! - Section D: the state `batch seed` leaves, read through the same adapters and SQL.
//!
//! A row whose `rust=` or `sql=` the code does not follow is not cited here, so the coverage
//! gate keeps reporting it; its ID appears nowhere in this file.

pub mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::mem::size_of_val;

use carddemo_audit::{CodeSpan, Document, PARITY_COMMON_COLUMNS, PARITY_LAYOUT_COLUMNS, Table};
use carddemo_domain::clock;
use carddemo_domain::entities::{
    AuthFraud, FraudFlag, MatchStatus, NewPendingAuthDetail, PendingAuthDetail, PendingAuthSummary,
    TR_DESCRIPTION_MAX_LEN, TranCategoryMaster, TranTypeMaster,
};
use carddemo_domain::format::{self, PIC_CCPAU_AMT, PIC_CP00_APPROVED_AMT};
use carddemo_domain::numeric::{
    AccountId, Alnum, AuthCount, AuthId, CardNumber, CustomerId, Money, NumericCode,
    TranCategoryCode, TranTypeCode,
};
use carddemo_domain::ports::{AuthorizationStore, PortError, TranTypeMasterStore};
use carddemo_domain::rules::auth_decision::{
    self, AuthDecision, AuthRequest, AuthWritePlan, ErrorLevel, ErrorLogEntry, ErrorSource,
    SummaryWrite, WriteStep,
};
use carddemo_testkit::fixtures;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use common::{
    Harness, block_on, expected_span, expected_value, expected_values, parity, parity_row,
};

/// The `Copybook field or PIC clause` column of every parity audit data table.
const PIC_COLUMN: &str = PARITY_COMMON_COLUMNS[3];

/// The `Expected result` column of every parity audit data table.
const EXPECTED_COLUMN: &str = PARITY_COMMON_COLUMNS[5];

/// The section A `Offset` column: a 0-based byte offset, or `—` for a Db2 column.
const OFFSET_COLUMN: &str = PARITY_LAYOUT_COLUMNS[0];

/// The section A `Length` column.
const LENGTH_COLUMN: &str = PARITY_LAYOUT_COLUMNS[1];

/// The section A `Usage` column: a COBOL usage, or the Db2 type of a Db2 column.
const USAGE_COLUMN: &str = PARITY_LAYOUT_COLUMNS[2];

/// The section A labels that `crates/domain/src/numeric/mod.rs` binds to a type of another
/// name, each with that type. The widths agree, and the code holds the field in that type:
/// `PA-PROCESSING-CODE 9(06)` and `PA-POS-ENTRY-MODE 9(02)` in the [`NumericCode`] of their
/// width, and the `S9(04) COMP` counts in [`AuthCount`], which holds an `i16`.
const LABEL_ALIASES: [(&str, &str); 3] = [
    ("ProcessingCode", "NumericCode<6>"),
    ("PosEntryMode", "NumericCode<2>"),
    ("i16 (count)", "AuthCount"),
];

/// The string form of a Db2 `TIMESTAMP(6)` value, `YYYY-MM-DD-HH.MI.SS.NNNNNN`: its length is
/// the `Length` of a `TIMESTAMP` column.
const DB2_TIMESTAMP_FORMAT: &str = "%Y-%m-%d-%H.%M.%S%.6f";

/// The string form of a Db2 `DATE` value, `YYYY-MM-DD`, which is also the text `to_jsonb`
/// prints for an SQL `DATE`.
const DB2_DATE_FORMAT: &str = "%Y-%m-%d";

/// A `TIMESTAMP` as `to_jsonb` prints it before the fraction's trailing zeros are dropped.
const JSONB_TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6f";

/// A probe whose statement the database accepted.
const ACCEPTED: &str = "accepted";

/// SQLSTATE `check_violation`.
const CHECK_VIOLATION: &str = "23514";

/// SQLSTATE `string_data_right_truncation`: a value longer than its `VARCHAR(n)`.
const TOO_LONG: &str = "22001";

/// The account of the sample summary, detail and fraud report: fixture account 50. The
/// `*_STORED_SQL` reads name it as a literal.
const SAMPLE_ACCOUNT: u64 = 50;

/// The card of the sample detail and fraud report, as `FRAUD_STORED_SQL` names it.
const SAMPLE_CARD: &str = "4859452612877065";

/// The instant the sample detail is keyed by: 2022-07-01 12:34:56.789 UTC, day 182 of 2022,
/// so `YYDDD` is 22182 and `HHMMSSmmm` is 123456789.
const SAMPLE_INSTANT: &str = "2022-07-01T12:34:56.789Z";

/// `HHMMSSmmm` of [`SAMPLE_INSTANT`].
const SAMPLE_HHMMSSMMM: i64 = 123_456_789;

/// The stored row of the sample summary, one `(column, text)` pair per column.
const SUMMARY_STORED_SQL: &str = "SELECT j.key, j.value FROM pending_auth_summaries s \
     CROSS JOIN LATERAL jsonb_each_text(to_jsonb(s)) AS j WHERE s.pa_acct_id = 50";

/// The stored row of the sample detail, one `(column, text)` pair per column.
const DETAIL_STORED_SQL: &str = "SELECT j.key, j.value FROM pending_auth_details d \
     CROSS JOIN LATERAL jsonb_each_text(to_jsonb(d)) AS j WHERE d.pa_acct_id = 50";

/// The stored row of the sample fraud report, one `(column, text)` pair per column.
const FRAUD_STORED_SQL: &str = "SELECT j.key, j.value FROM auth_fraud f \
     CROSS JOIN LATERAL jsonb_each_text(to_jsonb(f)) AS j WHERE f.card_num = '4859452612877065'";

/// The seeded master of type `01`, one `(column, text)` pair per column.
const TYPE_STORED_SQL: &str = "SELECT j.key, j.value FROM transaction_types t \
     CROSS JOIN LATERAL jsonb_each_text(to_jsonb(t)) AS j WHERE t.tr_type = '01'";

/// The seeded master category `01`/`0001`, one `(column, text)` pair per column.
const CATEGORY_STORED_SQL: &str = "SELECT j.key, j.value FROM transaction_categories t \
     CROSS JOIN LATERAL jsonb_each_text(to_jsonb(t)) AS j \
     WHERE t.trc_type_code = '01' AND t.trc_type_category = '0001'";

/// Every column of every table: table, column, `format_type`, `NOT NULL`, identity kind,
/// declared array dimensions and type length, in column order.
const COLUMNS_SQL: &str = "SELECT c.relname::text, a.attname::text, \
     format_type(a.atttypid, a.atttypmod), a.attnotnull::text, a.attidentity::text, \
     a.attndims::text, t.typlen::text \
     FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid \
     JOIN pg_namespace n ON n.oid = c.relnamespace JOIN pg_type t ON t.oid = a.atttypid \
     WHERE n.nspname = 'public' AND c.relkind = 'r' AND a.attnum > 0 AND NOT a.attisdropped \
     ORDER BY c.relname, a.attnum";

/// Every primary key, unique and foreign key constraint: table, kind (`p`, `u`, `f`), its
/// columns in key order, the referenced table and columns, and the `ON DELETE` action.
const KEYS_SQL: &str = r"
SELECT c.conrelid::regclass::text, c.contype::text,
       (SELECT string_agg(a.attname::text, ',' ORDER BY k.n)
          FROM unnest(c.conkey) WITH ORDINALITY AS k (attnum, n)
          JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = k.attnum),
       c.confrelid::regclass::text,
       (SELECT string_agg(a.attname::text, ',' ORDER BY k.n)
          FROM unnest(c.confkey) WITH ORDINALITY AS k (attnum, n)
          JOIN pg_attribute a ON a.attrelid = c.confrelid AND a.attnum = k.attnum),
       c.confdeltype::text
  FROM pg_constraint c
  JOIN pg_namespace s ON s.oid = c.connamespace
 WHERE s.nspname = 'public' AND c.contype IN ('p', 'u', 'f')
";

/// The summary every probe detail is inserted under. Its account is neither the probe base
/// of `pending_auth_summaries` nor [`SAMPLE_ACCOUNT`].
const PROBE_PARENT_SQL: &str = "INSERT INTO pending_auth_summaries VALUES \
     (90000000001, 1, 'A', ARRAY['', '', '', '', ''], 0, 0, 0, 0, 0, 0, 0, 0)";

/// The probes of the `CHECK` constraints. Each inserts one row, a valid base row of its table
/// with one column replaced, inside a subtransaction that is always rolled back, and returns
/// `accepted` or the SQLSTATE of the failure. The kinds:
///
/// - `base`: the base row itself, accepted, so a failure of another probe of its table comes
///   from the replaced column alone;
/// - `trailing`: every `VARCHAR(n)` column holding `n - 1` letters and a trailing space;
/// - `range`: the values on both sides of the bounds of each digit range;
/// - `occurs`: `PA-ACCOUNT-STATUS` with 4, 5 and 6 occurrences, and with one too wide.
const PROBES_SQL: &str = r#"
CREATE OR REPLACE FUNCTION pg_temp.section_a_probe(target regclass, base jsonb, col text, val jsonb)
RETURNS text LANGUAGE plpgsql AS $$
DECLARE
    cols text;
BEGIN
    SELECT string_agg(quote_ident(attname), ', ' ORDER BY attnum) INTO cols
      FROM pg_attribute
     WHERE attrelid = target AND attnum > 0 AND NOT attisdropped AND attidentity = '';
    BEGIN
        EXECUTE 'INSERT INTO ' || target::text || ' (' || cols || ') SELECT ' || cols
             || ' FROM jsonb_populate_record(NULL::' || target::text || ', $1)'
          USING base || jsonb_build_object(col, val);
        RAISE SQLSTATE 'CDA01';
    EXCEPTION
        WHEN SQLSTATE 'CDA01' THEN RETURN 'accepted';
        WHEN OTHERS THEN RETURN SQLSTATE;
    END;
END
$$;
WITH bases (target, base) AS (VALUES
    ('pending_auth_summaries'::regclass, '{"pa_acct_id": 90000000002, "pa_cust_id": 1,
        "pa_auth_status": "A", "pa_account_status": ["", "", "", "", ""],
        "pa_credit_limit": 0, "pa_cash_limit": 0, "pa_credit_balance": 0,
        "pa_cash_balance": 0, "pa_approved_auth_cnt": 0, "pa_declined_auth_cnt": 0,
        "pa_approved_auth_amt": 0, "pa_declined_auth_amt": 0}'::jsonb),
    ('pending_auth_details'::regclass, '{"pa_acct_id": 90000000001,
        "pa_auth_date_9c": 77817, "pa_auth_time_9c": 876543210,
        "pa_auth_orig_date": "220701", "pa_auth_orig_time": "123456",
        "pa_card_num": "4859452612877065", "pa_auth_type": "SAL", "pa_card_expiry_date": "0225",
        "pa_message_type": "0100", "pa_message_source": "POS", "pa_auth_id_code": "123456",
        "pa_auth_resp_code": "00", "pa_auth_resp_reason": "0000", "pa_processing_code": 0,
        "pa_transaction_amt": 1, "pa_approved_amt": 1, "pa_merchant_catagory_code": "5411",
        "pa_acqr_country_code": "840", "pa_pos_entry_mode": 5, "pa_merchant_id": "1",
        "pa_merchant_name": "M", "pa_merchant_city": "C", "pa_merchant_state": "WA",
        "pa_merchant_zip": "98101", "pa_transaction_id": "T", "pa_match_status": "P",
        "pa_auth_fraud": "", "pa_fraud_rpt_date": ""}'::jsonb),
    ('auth_fraud'::regclass,
        '{"card_num": "0000000000000001", "auth_ts": "2022-07-01 12:34:56.789"}'::jsonb),
    ('transaction_types'::regclass, '{"tr_type": "ZZ", "tr_description": "PROBE"}'::jsonb),
    ('transaction_categories'::regclass,
        '{"trc_type_code": "01", "trc_type_category": "9999", "trc_cat_data": "PROBE"}'::jsonb)
), cases (kind, target, col, val) AS (
    SELECT 'base', target, '', 'null'::jsonb FROM bases
    UNION ALL
    SELECT 'trailing', a.attrelid::regclass, a.attname::text,
           to_jsonb(repeat('A', a.atttypmod - 5) || ' ')
      FROM pg_attribute a JOIN bases b ON a.attrelid = b.target::oid
     WHERE a.atttypid = 'varchar'::regtype AND a.attnum > 0 AND NOT a.attisdropped
    UNION ALL
    SELECT listed.kind, listed.target::regclass, listed.col, listed.val::jsonb FROM (VALUES
        ('range', 'pending_auth_summaries', 'pa_acct_id', '-1'),
        ('range', 'pending_auth_summaries', 'pa_acct_id', '0'),
        ('range', 'pending_auth_summaries', 'pa_acct_id', '99999999999'),
        ('range', 'pending_auth_summaries', 'pa_acct_id', '100000000000'),
        ('range', 'pending_auth_summaries', 'pa_cust_id', '-1'),
        ('range', 'pending_auth_summaries', 'pa_cust_id', '0'),
        ('range', 'pending_auth_summaries', 'pa_cust_id', '999999999'),
        ('range', 'pending_auth_summaries', 'pa_cust_id', '1000000000'),
        ('occurs', 'pending_auth_summaries', 'pa_account_status', '["01", "02", "03", "04"]'),
        ('occurs', 'pending_auth_summaries', 'pa_account_status', '["01", "02", "03", "04", "05"]'),
        ('occurs', 'pending_auth_summaries', 'pa_account_status',
            '["01", "02", "03", "04", "05", "06"]'),
        ('occurs', 'pending_auth_summaries', 'pa_account_status',
            '["001", "02", "03", "04", "05"]'),
        ('range', 'pending_auth_details', 'pa_auth_date_9c', '-100000'),
        ('range', 'pending_auth_details', 'pa_auth_date_9c', '-99999'),
        ('range', 'pending_auth_details', 'pa_auth_date_9c', '99999'),
        ('range', 'pending_auth_details', 'pa_auth_date_9c', '100000'),
        ('range', 'pending_auth_details', 'pa_auth_time_9c', '-1000000000'),
        ('range', 'pending_auth_details', 'pa_auth_time_9c', '-999999999'),
        ('range', 'pending_auth_details', 'pa_auth_time_9c', '999999999'),
        ('range', 'pending_auth_details', 'pa_auth_time_9c', '1000000000'),
        ('range', 'pending_auth_details', 'pa_processing_code', '-1'),
        ('range', 'pending_auth_details', 'pa_processing_code', '0'),
        ('range', 'pending_auth_details', 'pa_processing_code', '999999'),
        ('range', 'pending_auth_details', 'pa_processing_code', '1000000'),
        ('range', 'pending_auth_details', 'pa_pos_entry_mode', '-1'),
        ('range', 'pending_auth_details', 'pa_pos_entry_mode', '0'),
        ('range', 'pending_auth_details', 'pa_pos_entry_mode', '99'),
        ('range', 'pending_auth_details', 'pa_pos_entry_mode', '100'),
        ('range', 'auth_fraud', 'acct_id', '-1'),
        ('range', 'auth_fraud', 'acct_id', '0'),
        ('range', 'auth_fraud', 'acct_id', '99999999999'),
        ('range', 'auth_fraud', 'acct_id', '100000000000'),
        ('range', 'auth_fraud', 'cust_id', '-1'),
        ('range', 'auth_fraud', 'cust_id', '0'),
        ('range', 'auth_fraud', 'cust_id', '999999999'),
        ('range', 'auth_fraud', 'cust_id', '1000000000')
    ) AS listed (kind, target, col, val)
)
SELECT c.kind, c.target::text, c.col, c.val::text,
       pg_temp.section_a_probe(c.target, b.base, c.col, c.val)
  FROM cases c JOIN bases b ON b.target = c.target
 ORDER BY 1, 2, 3, 4
"#;

/// The row counts of the seeded Db2 masters and authorization stores.
const SEEDED_COUNTS_SQL: &str = "SELECT 'transaction_types', count(*)::text FROM transaction_types \
     UNION ALL SELECT 'transaction_categories', count(*)::text FROM transaction_categories \
     UNION ALL SELECT 'pending_auth_summaries', count(*)::text FROM pending_auth_summaries \
     UNION ALL SELECT 'pending_auth_details', count(*)::text FROM pending_auth_details \
     UNION ALL SELECT 'auth_fraud', count(*)::text FROM auth_fraud";

/// The seeded type codes, in byte order.
const SEEDED_TYPES_SQL: &str =
    "SELECT tr_type FROM transaction_types ORDER BY tr_type COLLATE \"C\"";

/// The seeded categories: type code, category and description.
const SEEDED_CATEGORIES_SQL: &str = "SELECT trc_type_code, trc_type_category, trc_cat_data \
     FROM transaction_categories ORDER BY 1, 2";

/// The section D kinds of `P-CIPAUSMY-014`, each with the tables whose rows it counts: the
/// IMS database `DBPAUTP0` is the summary and detail tables.
const SEEDED_STORES: [(&str, &[&str]); 2] = [
    (
        "pending_authorizations_after_seed",
        &["pending_auth_summaries", "pending_auth_details"],
    ),
    ("auth_fraud_after_seed", &["auth_fraud"]),
];

/// The rows of the `CCPAUERY` `ERROR-LOG-RECORD`, one per field in offset order.
const ERROR_LOG_ROWS: [&str; 11] = [
    "P-CCPAUERY-001",
    "P-CCPAUERY-002",
    "P-CCPAUERY-003",
    "P-CCPAUERY-004",
    "P-CCPAUERY-005",
    "P-CCPAUERY-006",
    "P-CCPAUERY-007",
    "P-CCPAUERY-008",
    "P-CCPAUERY-009",
    "P-CCPAUERY-010",
    "P-CCPAUERY-011",
];

/// The copybook whose 88-levels give the values of `ERR-LEVEL` and `ERR-SUBSYSTEM`.
const CCPAUERY_COPYBOOK: &str = "app/app-authorization-ims-db2-mq/cpy/CCPAUERY.cpy";

/// The program whose `SET ... TO TRUE` statements choose among those values.
const COPAUA0C_PROGRAM: &str = "app/app-authorization-ims-db2-mq/cbl/COPAUA0C.cbl";

/// Instants `9500-LOG-ERROR` may stamp a record with: the sample instant, both sides of the
/// century change and the last millisecond of a day.
const ERROR_LOG_INSTANTS: [&str; 4] = [
    SAMPLE_INSTANT,
    "1999-12-31T23:59:59Z",
    "2000-01-01T00:00:00Z",
    "2099-12-31T23:59:59.999Z",
];

/// The `CCPAURQY`, `CCPAURLY` and `CCPAUERY` rows whose fields no table stores
/// (`sql=not stored; column=—`).
const NOT_STORED: [&str; 25] = [
    "P-CCPAURQY-001",
    "P-CCPAURQY-002",
    "P-CCPAURQY-003",
    "P-CCPAURQY-004",
    "P-CCPAURQY-005",
    "P-CCPAURQY-006",
    "P-CCPAURQY-007",
    "P-CCPAURQY-009",
    "P-CCPAURQY-010",
    "P-CCPAURQY-011",
    "P-CCPAURQY-013",
    "P-CCPAURQY-014",
    "P-CCPAURQY-015",
    "P-CCPAURQY-016",
    "P-CCPAURQY-017",
    "P-CCPAURQY-018",
    "P-CCPAURLY-001",
    "P-CCPAURLY-002",
    "P-CCPAURLY-003",
    "P-CCPAURLY-004",
    "P-CCPAURLY-005",
    "P-CCPAURLY-006",
    "P-CCPAUERY-008",
    "P-CCPAUERY-009",
    "P-CCPAUERY-011",
];

// ---------------------------------------------------------------------------------------
// What a Rust type says about the field it holds
// ---------------------------------------------------------------------------------------

/// What a Rust field type says about the section A field it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RustField {
    /// The type as a `rust=` label names it, for example `Alnum<6>` or `Money<9>`.
    name: String,
    /// The digits of the PIC clause, or the characters of the `X(n)` item, the type holds;
    /// `None` for a plain integer, whose digits only its column's `CHECK` bounds.
    width: Option<usize>,
    /// The bytes of the binary (`COMP`) value the type holds, for a binary count.
    binary: Option<usize>,
    /// Whether the type holds SQL `NULL`: an `Option`.
    nullable: bool,
}

/// A Rust type that holds a section A field.
trait SectionAType {
    /// The type's `rust=` name.
    fn name() -> String;

    /// See [`RustField::width`].
    fn width() -> Option<usize>;

    /// See [`RustField::binary`].
    fn binary() -> Option<usize> {
        None
    }

    /// See [`RustField::nullable`].
    fn nullable() -> bool {
        false
    }

    /// The value as `jsonb_each_text(to_jsonb(row))` prints its column, `None` for `NULL`.
    fn stored(&self) -> Option<String>;
}

/// What the type of `value` says about the field it holds.
fn rust_of<T: SectionAType>(_value: &T) -> RustField {
    RustField {
        name: T::name(),
        width: T::width(),
        binary: T::binary(),
        nullable: T::nullable(),
    }
}

/// What the type a conversion returns says: for a flag, the `X(01)` item it converts to.
fn returned<A, R: SectionAType>(_convert: fn(A) -> R) -> RustField {
    RustField {
        name: R::name(),
        width: R::width(),
        binary: R::binary(),
        nullable: R::nullable(),
    }
}

impl<const N: usize> SectionAType for Alnum<N> {
    fn name() -> String {
        format!("Alnum<{N}>")
    }

    fn width() -> Option<usize> {
        Some(N)
    }

    fn stored(&self) -> Option<String> {
        Some(self.trimmed().into_owned())
    }
}

impl<T: SectionAType, const N: usize> SectionAType for [T; N] {
    fn name() -> String {
        format!("[{}; {N}]", T::name())
    }

    fn width() -> Option<usize> {
        T::width().map(|width| width * N)
    }

    /// The JSON array `to_jsonb` prints for an SQL array.
    fn stored(&self) -> Option<String> {
        let items: Vec<String> = self
            .iter()
            .map(|item| {
                let text = item.stored().unwrap_or_default();
                let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
                format!("\"{escaped}\"")
            })
            .collect();
        Some(format!("[{}]", items.join(", ")))
    }
}

impl SectionAType for CardNumber {
    fn name() -> String {
        "CardNumber".to_owned()
    }

    fn width() -> Option<usize> {
        Some(Self::move_from("").as_bytes().len())
    }

    fn stored(&self) -> Option<String> {
        Some(self.trimmed().into_owned())
    }
}

impl SectionAType for TranTypeCode {
    fn name() -> String {
        "TranTypeCode".to_owned()
    }

    fn width() -> Option<usize> {
        Some(Self::move_from("").as_bytes().len())
    }

    fn stored(&self) -> Option<String> {
        Some(self.trimmed().into_owned())
    }
}

impl SectionAType for AccountId {
    fn name() -> String {
        "AccountId".to_owned()
    }

    fn width() -> Option<usize> {
        Some(Self::WIDTH)
    }

    fn stored(&self) -> Option<String> {
        Some(self.get().to_string())
    }
}

impl SectionAType for CustomerId {
    fn name() -> String {
        "CustomerId".to_owned()
    }

    fn width() -> Option<usize> {
        Some(Self::WIDTH)
    }

    fn stored(&self) -> Option<String> {
        Some(self.get().to_string())
    }
}

impl<const N: u8> SectionAType for NumericCode<N> {
    fn name() -> String {
        format!("NumericCode<{N}>")
    }

    fn width() -> Option<usize> {
        Some(Self::WIDTH)
    }

    fn stored(&self) -> Option<String> {
        Some(self.get().to_string())
    }
}

impl<const D: u8> SectionAType for Money<D> {
    fn name() -> String {
        format!("Money<{D}>")
    }

    /// The `D` integer digits and the 2 of `V99`.
    fn width() -> Option<usize> {
        Some(usize::from(D) + 2)
    }

    fn stored(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl SectionAType for AuthCount {
    fn name() -> String {
        "AuthCount".to_owned()
    }

    fn width() -> Option<usize> {
        Some(auth_count_digits())
    }

    /// The bytes of the `i16` the count holds.
    fn binary() -> Option<usize> {
        let held: i16 = Self::ZERO.get();
        Some(size_of_val(&held))
    }

    fn stored(&self) -> Option<String> {
        Some(self.get().to_string())
    }
}

impl SectionAType for TranCategoryCode {
    fn name() -> String {
        "TranCategoryCode".to_owned()
    }

    fn width() -> Option<usize> {
        Some(Self::WIDTH)
    }

    /// The zero-padded digits, the text its `VARCHAR` column stores.
    fn stored(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl SectionAType for NaiveDateTime {
    fn name() -> String {
        "NaiveDateTime".to_owned()
    }

    /// The characters of the value's Db2 `TIMESTAMP(6)` string form.
    fn width() -> Option<usize> {
        Some(
            Self::default()
                .format(DB2_TIMESTAMP_FORMAT)
                .to_string()
                .chars()
                .count(),
        )
    }

    /// The value as `to_jsonb` prints a `TIMESTAMP`: the fraction of a second without its
    /// trailing zeros, and without its point when nothing is left of it.
    fn stored(&self) -> Option<String> {
        let text = self.format(JSONB_TIMESTAMP_FORMAT).to_string();
        Some(text.trim_end_matches('0').trim_end_matches('.').to_owned())
    }
}

impl SectionAType for NaiveDate {
    fn name() -> String {
        "NaiveDate".to_owned()
    }

    /// The characters of the value's Db2 `DATE` string form.
    fn width() -> Option<usize> {
        Some(
            Self::default()
                .format(DB2_DATE_FORMAT)
                .to_string()
                .chars()
                .count(),
        )
    }

    fn stored(&self) -> Option<String> {
        Some(self.format(DB2_DATE_FORMAT).to_string())
    }
}

impl SectionAType for i32 {
    fn name() -> String {
        "i32".to_owned()
    }

    fn width() -> Option<usize> {
        None
    }

    fn stored(&self) -> Option<String> {
        Some(self.to_string())
    }
}

impl SectionAType for MatchStatus {
    fn name() -> String {
        returned(Self::to_alnum).name
    }

    fn width() -> Option<usize> {
        returned(Self::to_alnum).width
    }

    fn stored(&self) -> Option<String> {
        self.to_alnum().stored()
    }
}

impl SectionAType for FraudFlag {
    fn name() -> String {
        returned(Self::to_alnum).name
    }

    fn width() -> Option<usize> {
        returned(Self::to_alnum).width
    }

    fn stored(&self) -> Option<String> {
        self.to_alnum().stored()
    }
}

impl<T: SectionAType> SectionAType for Option<T> {
    fn name() -> String {
        T::name()
    }

    fn width() -> Option<usize> {
        T::width()
    }

    fn binary() -> Option<usize> {
        T::binary()
    }

    fn nullable() -> bool {
        true
    }

    fn stored(&self) -> Option<String> {
        self.as_ref().and_then(SectionAType::stored)
    }
}

/// The digits an [`AuthCount`] holds: the most nines it accepts, checked to reject one more.
fn auth_count_digits() -> usize {
    let mut digits = 0;
    let mut nines: i16 = 0;
    while let Some(next) = nines.checked_mul(10).and_then(|value| value.checked_add(9)) {
        if AuthCount::new(next).is_err() {
            break;
        }
        nines = next;
        digits += 1;
    }
    let beyond = nines
        .checked_add(1)
        .and_then(|value| AuthCount::new(value).ok());
    assert!(beyond.is_none(), "AuthCount accepts {nines} + 1");
    digits
}

/// The code type a `rust=` label names: its alias, or the label without a trailing
/// parenthesised note, so `i32 (inverted key part)` names `i32`.
fn code_type_name(label: &str) -> String {
    LABEL_ALIASES
        .iter()
        .find(|(alias, _)| *alias == label)
        .map_or_else(
            || label.split(" (").next().unwrap_or(label).to_owned(),
            |(_, name)| (*name).to_owned(),
        )
}

// ---------------------------------------------------------------------------------------
// Audit rows
// ---------------------------------------------------------------------------------------

/// The section A cells of one audit row.
#[derive(Debug)]
struct LayoutRow {
    id: &'static str,
    /// `Copybook field or PIC clause`.
    pic: String,
    /// The whole `Expected result` cell.
    expected: String,
    /// The `rust=` label, read up to `; sql=`, so `[Alnum<2>; 5]` stays whole.
    rust: String,
    sql: String,
    column: String,
    offset: Option<usize>,
    length: usize,
    usage: String,
}

/// The section A cells of row `id`.
fn layout_row(doc: &Document, id: &'static str) -> LayoutRow {
    let (table, row) = parity_row(doc, id);
    let cell = |column: &str| {
        table
            .cell(row, column)
            .unwrap_or_else(|| panic!("{id} has no {column:?} cell"))
            .to_owned()
    };
    let expected = cell(EXPECTED_COLUMN);
    let rust = expected
        .split_once("rust=")
        .and_then(|(_, rest)| rest.split_once("; sql="))
        .map_or_else(
            || panic!("{id}: no rust= before sql= in {expected:?}"),
            |(label, _)| label.trim().to_owned(),
        );
    LayoutRow {
        id,
        pic: cell(PIC_COLUMN),
        rust,
        sql: expected_value(doc, id, "sql"),
        column: expected_value(doc, id, "column"),
        offset: cell(OFFSET_COLUMN).parse().ok(),
        length: cell(LENGTH_COLUMN)
            .parse()
            .unwrap_or_else(|error| panic!("{id}: Length is not a byte count: {error}")),
        usage: cell(USAGE_COLUMN),
        expected,
    }
}

/// The record length a layout table's heading ends with, for example the 153 of
/// `CCPAURQY PENDING-AUTH-REQUEST (CP00 request fields, 153)`.
fn record_length(table: &Table) -> usize {
    let heading = table.subsection.as_deref().unwrap_or_default();
    heading
        .trim_end_matches(')')
        .rsplit_once(", ")
        .and_then(|(_, length)| length.parse().ok())
        .unwrap_or_else(|| panic!("the heading {heading:?} names no record length"))
}

/// The exact text an audit code span holds: its double-quoted string, unescaped.
fn exact(span: &CodeSpan) -> String {
    let quoted = span
        .text
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| panic!("the code span {:?} is not a quoted text", span.text));
    let mut text = String::new();
    let mut chars = quoted.chars();
    while let Some(character) = chars.next() {
        text.push(if character == '\\' {
            chars.next().unwrap_or('\\')
        } else {
            character
        });
    }
    text
}

/// The leading number of `text`, for example the 0 of `0 (the IMS unload is not loaded)`.
fn leading_number(text: &str) -> usize {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|error| panic!("{text:?} starts with no number: {error}"))
}

/// The number after `prefix` in `text`, for example the 6 of `BYTES=6`.
fn number_after(text: &str, prefix: &str) -> usize {
    let rest = text
        .split_once(prefix)
        .map_or_else(|| panic!("{text:?} holds no {prefix}"), |(_, rest)| rest);
    leading_number(rest)
}

/// The digits of the PIC clause in `pic`: the `n` of `9(n)`.
fn pic_digits(id: &str, pic: &str) -> u32 {
    let digits = number_after(pic, "9(");
    u32::try_from(digits).unwrap_or_else(|error| panic!("{id}: {digits} digits: {error}"))
}

/// `10^digits`.
fn pow10(digits: u32) -> i128 {
    10_i128.pow(digits)
}

/// The facts of a `sql=` value.
#[derive(Debug)]
struct SqlSpec {
    /// The type as `format_type` prints it, for example `character varying(2)[]`.
    pg_type: String,
    /// The `n` of `VARCHAR(n)`.
    text_width: Option<usize>,
    /// The occurrences of an array type, the `5` of `VARCHAR(2)[5]`.
    occurs: Option<usize>,
    /// The `n` of `CHECK (0 <= v < 10^n)`.
    check_digits: Option<u32>,
    right_trimmed: bool,
    not_null: bool,
}

/// The facts of the `sql=` value `sql` of row `id`.
fn sql_spec(id: &str, sql: &str) -> SqlSpec {
    let base = sql.split_whitespace().next().unwrap_or_default();
    let (scalar, occurs) = base.split_once('[').map_or((base, None), |(scalar, rest)| {
        (scalar, Some(leading_number(rest)))
    });
    let (name, argument) = scalar
        .split_once('(')
        .map_or((scalar, None), |(name, rest)| {
            (name, Some(rest.trim_end_matches(')')))
        });
    let pg_type = match (name, argument) {
        ("BIGINT", None) => "bigint".to_owned(),
        ("INTEGER", None) => "integer".to_owned(),
        ("SMALLINT", None) => "smallint".to_owned(),
        ("NUMERIC", Some(scale)) => format!("numeric({scale})"),
        ("VARCHAR", Some(width)) => format!("character varying({width})"),
        ("CHAR", Some(width)) => format!("character({width})"),
        ("TIMESTAMP", Some(precision)) => format!("timestamp({precision}) without time zone"),
        ("DATE", None) => "date".to_owned(),
        _ => panic!("{id}: sql={sql:?} names a type this test does not map"),
    };
    let text_width = (name == "VARCHAR").then(|| leading_number(argument.unwrap_or_default()));
    let check_digits = sql.split_once("< 10^").map(|(_, rest)| {
        u32::try_from(leading_number(rest)).unwrap_or_else(|error| panic!("{id}: {error}"))
    });
    SqlSpec {
        pg_type: if occurs.is_some() {
            pg_type + "[]"
        } else {
            pg_type
        },
        text_width,
        occurs,
        check_digits,
        right_trimmed: sql.contains("right-trimmed"),
        not_null: sql.contains("NOT NULL"),
    }
}

// ---------------------------------------------------------------------------------------
// The test database
// ---------------------------------------------------------------------------------------

/// One table column, as the catalog holds it.
#[derive(Debug)]
struct Column {
    table: String,
    name: String,
    /// The 1-based position among the table's columns.
    position: usize,
    pg_type: String,
    not_null: bool,
    /// `a` for an identity `GENERATED ALWAYS`, empty otherwise.
    identity: String,
    dims: usize,
    /// The bytes of a fixed-length type, `None` for a variable-length one.
    typlen: Option<usize>,
}

/// One primary key (`p`), unique (`u`) or foreign key (`f`) constraint.
#[derive(Debug)]
struct Key {
    table: String,
    kind: String,
    columns: Vec<String>,
    references: Option<(String, Vec<String>)>,
    /// `confdeltype`: `r` for `ON DELETE RESTRICT`.
    on_delete: String,
}

/// One probe of [`PROBES_SQL`].
#[derive(Debug)]
struct Probe {
    kind: String,
    table: String,
    column: String,
    value: String,
    outcome: String,
}

/// What the test database holds: its catalog, its keys, the probe outcomes and the stored
/// form of the sample rows by table and column.
struct Schema {
    columns: Vec<Column>,
    keys: Vec<Key>,
    probes: Vec<Probe>,
    stored: BTreeMap<&'static str, BTreeMap<String, Option<String>>>,
}

/// The text of cell `index` of `row`, which is never `NULL`.
fn text(row: &[Option<String>], index: usize, what: &str) -> String {
    row.get(index)
        .cloned()
        .flatten()
        .unwrap_or_else(|| panic!("the database gave no {what}"))
}

/// The comma-separated column list in cell `index` of `row`; none for `NULL`.
fn names(row: &[Option<String>], index: usize) -> Vec<String> {
    row.get(index)
        .and_then(Option::as_ref)
        .map_or_else(Vec::new, |list| {
            list.split(',').map(str::to_owned).collect()
        })
}

impl Schema {
    /// Reads the catalog, the keys and the stored sample rows, and runs the probes.
    fn read(h: &Harness) -> Self {
        let rows = |sql: &str| block_on(h.stack().fetch_text_rows(sql));
        let mut columns: Vec<Column> = Vec::new();
        for row in rows(COLUMNS_SQL) {
            let table = text(&row, 0, "table");
            let position = 1 + columns.iter().filter(|seen| seen.table == table).count();
            columns.push(Column {
                table,
                name: text(&row, 1, "column"),
                position,
                pg_type: text(&row, 2, "type"),
                not_null: text(&row, 3, "attnotnull") == "true",
                identity: text(&row, 4, "attidentity"),
                dims: text(&row, 5, "attndims").parse().unwrap_or_default(),
                typlen: text(&row, 6, "typlen").parse().ok(),
            });
        }
        let keys = rows(KEYS_SQL)
            .into_iter()
            .map(|row| Key {
                table: text(&row, 0, "table"),
                kind: text(&row, 1, "contype"),
                columns: names(&row, 2),
                references: row
                    .get(4)
                    .and_then(Option::as_ref)
                    .map(|_| (text(&row, 3, "confrelid"), names(&row, 4))),
                on_delete: text(&row, 5, "confdeltype"),
            })
            .collect();
        block_on(h.stack().exec_sql(PROBE_PARENT_SQL));
        let probes = rows(PROBES_SQL)
            .into_iter()
            .map(|row| Probe {
                kind: text(&row, 0, "kind"),
                table: text(&row, 1, "table"),
                column: text(&row, 2, "column"),
                value: text(&row, 3, "value"),
                outcome: text(&row, 4, "outcome"),
            })
            .collect();
        let stored = [
            ("pending_auth_summaries", SUMMARY_STORED_SQL),
            ("pending_auth_details", DETAIL_STORED_SQL),
            ("auth_fraud", FRAUD_STORED_SQL),
            ("transaction_types", TYPE_STORED_SQL),
            ("transaction_categories", CATEGORY_STORED_SQL),
        ]
        .into_iter()
        .map(|(table, sql)| {
            let pairs = rows(sql)
                .into_iter()
                .map(|row| (text(&row, 0, "key"), row.get(1).cloned().flatten()))
                .collect();
            (table, pairs)
        })
        .collect();
        Self {
            columns,
            keys,
            probes,
            stored,
        }
    }

    /// The column `name` of `table`.
    fn column(&self, table: &str, name: &str) -> &Column {
        self.columns
            .iter()
            .find(|column| column.table == table && column.name == name)
            .unwrap_or_else(|| panic!("{table} has no column {name}"))
    }

    /// The primary key columns of `table`, in key order.
    fn primary_key(&self, table: &str) -> &[String] {
        self.keys
            .iter()
            .find(|key| key.table == table && key.kind == "p")
            .map_or_else(|| panic!("{table} has no primary key"), |key| &key.columns)
    }

    /// The probes of `kind` on `table.column`.
    fn probes(&self, kind: &str, table: &str, column: &str) -> Vec<&Probe> {
        self.probes
            .iter()
            .filter(|probe| probe.kind == kind && probe.table == table && probe.column == column)
            .collect()
    }

    /// The stored text of `column` in the sample row of `table`.
    fn stored(&self, table: &str, column: &str) -> Option<String> {
        self.stored
            .get(table)
            .and_then(|row| row.get(column))
            .unwrap_or_else(|| panic!("the stored sample row of {table} has no {column}"))
            .clone()
    }
}

// ---------------------------------------------------------------------------------------
// Section A fields held to their rows
// ---------------------------------------------------------------------------------------

/// One cited field: its row, its column, its Rust type and the text its column must store.
struct Field {
    id: &'static str,
    column: &'static str,
    rust: RustField,
    stored: Option<String>,
}

/// The field of row `id`, stored in `column`, held in `value`.
fn field<T: SectionAType>(id: &'static str, column: &'static str, value: &T) -> Field {
    Field {
        id,
        column,
        rust: rust_of(value),
        stored: value.stored(),
    }
}

/// Whether a table holds an IMS segment, whose fields have byte offsets, or a Db2 table,
/// whose `Usage` is the Db2 type and whose nullability is the DDL's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Segment,
    Db2,
}

/// The probes of `table.column` hold the digit range `low..high`, and reach both of its
/// bounds from each side.
fn check_range(id: &str, probes: &[&Probe], low: i128, high: i128) {
    let values: Vec<i128> = probes
        .iter()
        .map(|probe| {
            probe
                .value
                .parse()
                .unwrap_or_else(|error| panic!("{id}: probe {:?}: {error}", probe.value))
        })
        .collect();
    for bound in [low - 1, low, high - 1, high] {
        assert!(values.contains(&bound), "{id}: no probe reaches {bound}");
    }
    for (probe, value) in probes.iter().zip(values) {
        let expected = if (low..high).contains(&value) {
            ACCEPTED
        } else {
            CHECK_VIOLATION
        };
        assert_eq!(
            probe.outcome, expected,
            "{id}: {}.{} = {value}",
            probe.table, probe.column
        );
    }
}

/// The probes of an array column hold exactly `occurs` elements of at most `width`
/// characters, and reach each bound.
fn check_occurs(id: &str, probes: &[&Probe], occurs: usize, width: usize) {
    let mut reached = [false; 4];
    for probe in probes {
        let elements: Vec<&str> = probe
            .value
            .trim_matches(|ch| ch == '[' || ch == ']')
            .split(", ")
            .map(|element| element.trim_matches('"'))
            .collect();
        let too_wide = elements
            .iter()
            .any(|element| element.chars().count() > width);
        let expected = if too_wide {
            reached[3] = true;
            TOO_LONG
        } else if elements.len() == occurs {
            reached[0] = true;
            ACCEPTED
        } else {
            reached[usize::from(elements.len() > occurs) + 1] = true;
            CHECK_VIOLATION
        };
        assert_eq!(
            probe.outcome, expected,
            "{id}: {} = {}",
            probe.column, probe.value
        );
    }
    assert_eq!(
        reached, [true; 4],
        "{id}: the probes must hold {occurs}, fewer, more, and one too wide element"
    );
}

/// The probes of `field` in `table` hold its row's `sql=`; returns the digits a plain
/// integer field's column holds, its PIC digits.
fn check_probes(schema: &Schema, table: &str, row: &LayoutRow, field: &Field) -> Option<usize> {
    let spec = sql_spec(row.id, &row.sql);
    if let Some(digits) = spec.check_digits {
        let probes = schema.probes("range", table, field.column);
        check_range(row.id, &probes, 0, pow10(digits));
    }
    if let Some(occurs) = spec.occurs {
        let width = spec
            .text_width
            .unwrap_or_else(|| panic!("{}: no width", row.id));
        let probes = schema.probes("occurs", table, field.column);
        check_occurs(row.id, &probes, occurs, width);
    }
    if spec.right_trimmed {
        let probes = schema.probes("trailing", table, field.column);
        assert!(!probes.is_empty(), "{}: no trailing-space probe", row.id);
        for probe in probes {
            assert_eq!(
                probe.outcome, CHECK_VIOLATION,
                "{}: {} = {} must be refused: the column holds the right-trimmed form",
                row.id, probe.column, probe.value
            );
        }
    }
    if field.rust.width.is_some() {
        return None;
    }
    // A plain integer: its column's CHECK holds the signed digits of the PIC clause.
    let digits = pic_digits(row.id, &row.pic);
    let limit = pow10(digits);
    let probes = schema.probes("range", table, field.column);
    check_range(row.id, &probes, 1 - limit, limit);
    Some(usize::try_from(digits).unwrap_or_else(|error| panic!("{}: {error}", row.id)))
}

/// The bytes the code gives `field` under the row's `Usage`; for a Db2 `TIMESTAMP` or `DATE`,
/// the characters of the value's string form.
fn code_length(row: &LayoutRow, field: &Field, column: &Column, digits: Option<usize>) -> usize {
    let width = field
        .rust
        .width
        .or(digits)
        .unwrap_or_else(|| panic!("{}: the code gives no width", row.id));
    let usage = row.usage.as_str();
    let text = usage == "DISPLAY" || usage.starts_with("CHAR(") || usage.starts_with("VARCHAR(");
    if text || usage == "TIMESTAMP" || usage == "DATE" {
        width
    } else if usage == "COMP-3" || usage.starts_with("DECIMAL(") {
        // A packed field holds two digits per byte and its sign in the last half-byte.
        width / 2 + 1
    } else if usage == "COMP" {
        field
            .rust
            .binary
            .unwrap_or_else(|| panic!("{}: {} holds no binary value", row.id, field.rust.name))
    } else if usage == "SMALLINT" {
        column
            .typlen
            .unwrap_or_else(|| panic!("{}: {} has no fixed length", row.id, column.name))
    } else {
        panic!("{}: Usage {usage:?} is not mapped by this test", row.id)
    }
}

/// The column, `rust=`, `sql=`, stored form and length of `field` hold its row.
fn check_field(schema: &Schema, table: &str, layout: Layout, row: &LayoutRow, field: &Field) {
    let id = row.id;
    assert_eq!(row.column, field.column, "{id}: column=");
    assert_eq!(
        code_type_name(&row.rust),
        field.rust.name,
        "{id}: rust={}",
        row.rust
    );
    let spec = sql_spec(id, &row.sql);
    let column = schema.column(table, field.column);
    assert_eq!(column.pg_type, spec.pg_type, "{id}: sql={}", row.sql);
    assert_eq!(
        column.dims,
        usize::from(spec.occurs.is_some()),
        "{id}: array dimensions"
    );
    if spec.not_null {
        assert!(
            column.not_null,
            "{id}: {table}.{} must be NOT NULL",
            column.name
        );
    }
    if layout == Layout::Db2 {
        let ddl_not_null = row.pic.contains("NOT NULL");
        assert_eq!(
            column.not_null, ddl_not_null,
            "{id}: NOT NULL as the DDL {}",
            row.pic
        );
        assert_eq!(
            field.rust.nullable, !ddl_not_null,
            "{id}: Option as the DDL nullability"
        );
    }
    assert_eq!(
        schema.stored(table, field.column),
        field.stored,
        "{id}: the adapters store {} in {table}.{}",
        field.rust.name,
        field.column
    );
}

/// Every field of `fields`, the cited fields of `table`, holds its row; the fields of a
/// segment lie at the offsets their widths give in the migration's column order.
fn check_fields(doc: &Document, schema: &Schema, table: &str, layout: Layout, fields: &[Field]) {
    let base = schema.probes("base", table, "");
    assert!(
        base.len() == 1 && base.iter().all(|probe| probe.outcome == ACCEPTED),
        "{table}: the probe base row must be accepted: {base:?}"
    );
    let mut placed: Vec<(usize, usize, LayoutRow)> = Vec::new();
    for field in fields {
        let row = layout_row(doc, field.id);
        check_field(schema, table, layout, &row, field);
        let digits = check_probes(schema, table, &row, field);
        let column = schema.column(table, field.column);
        let length = code_length(&row, field, column, digits);
        assert_eq!(length, row.length, "{}: Length under {}", row.id, row.usage);
        placed.push((column.position, length, row));
    }
    if layout == Layout::Segment {
        placed.sort_by_key(|(position, ..)| *position);
        let mut offset = 0;
        for (_, length, row) in &placed {
            assert_eq!(row.offset, Some(offset), "{}: Offset", row.id);
            offset += length;
        }
    }
}

// ---------------------------------------------------------------------------------------
// Sample records
// ---------------------------------------------------------------------------------------

/// [`SAMPLE_INSTANT`].
fn sample_instant() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(SAMPLE_INSTANT)
        .unwrap_or_else(|error| panic!("{SAMPLE_INSTANT} is RFC 3339: {error}"))
        .with_timezone(&Utc)
}

/// The money field holding `cents` hundredths.
fn money<const D: u8>(cents: i64) -> Money<D> {
    Money::from_cents(cents).unwrap_or_else(|error| panic!("{cents} cents fit Money<{D}>: {error}"))
}

/// [`SAMPLE_ACCOUNT`] as an account number.
fn sample_account() -> AccountId {
    AccountId::new(SAMPLE_ACCOUNT).unwrap_or_else(|error| panic!("account 50: {error}"))
}

/// [`SAMPLE_ACCOUNT`] as a customer number.
fn sample_customer() -> CustomerId {
    CustomerId::new(50).unwrap_or_else(|error| panic!("customer 50: {error}"))
}

/// A summary of [`SAMPLE_ACCOUNT`] whose values tell its fields apart; the last status
/// occurrence is padded, so its stored form is right-trimmed.
fn sample_summary() -> PendingAuthSummary {
    PendingAuthSummary {
        pa_acct_id: sample_account(),
        pa_cust_id: sample_customer(),
        pa_auth_status: Alnum::move_from("A"),
        pa_account_status: ["01", "02", "03", "04", "1"].map(Alnum::move_from),
        pa_credit_limit: money(202_000),
        pa_cash_limit: money(102_000),
        pa_credit_balance: money(-19_450),
        pa_cash_balance: Money::ZERO,
        pa_approved_auth_cnt: AuthCount::new(2).unwrap_or_else(|error| panic!("2: {error}")),
        pa_declined_auth_cnt: AuthCount::new(-1).unwrap_or_else(|error| panic!("-1: {error}")),
        pa_approved_auth_amt: money(99_999_999_999),
        pa_declined_auth_amt: money(-75_001),
    }
}

/// A detail under [`SAMPLE_ACCOUNT`] keyed by `now`, whose values tell its fields apart.
fn sample_detail(now: DateTime<Utc>) -> NewPendingAuthDetail {
    let (pa_auth_date_9c, time) = clock::auth_key_complement(now);
    NewPendingAuthDetail {
        pa_acct_id: sample_account(),
        pa_auth_date_9c,
        pa_auth_time_9c: i32::try_from(time).unwrap_or_else(|error| panic!("{time}: {error}")),
        pa_auth_orig_date: Alnum::move_from("220701"),
        pa_auth_orig_time: Alnum::move_from("123456"),
        pa_card_num: CardNumber::move_from(SAMPLE_CARD),
        pa_auth_type: Alnum::move_from("SAL"),
        pa_card_expiry_date: Alnum::move_from("0225"),
        pa_message_type: Alnum::move_from("0100"),
        pa_message_source: Alnum::move_from("POS"),
        pa_auth_id_code: Alnum::move_from("654321"),
        pa_auth_resp_code: Alnum::move_from("00"),
        pa_auth_resp_reason: Alnum::move_from("0000"),
        pa_processing_code: NumericCode::new(123_456)
            .unwrap_or_else(|error| panic!("123456: {error}")),
        pa_transaction_amt: money(-12_345),
        pa_approved_amt: money(999_999_999_999),
        pa_merchant_catagory_code: Alnum::move_from("5411"),
        pa_acqr_country_code: Alnum::move_from("840"),
        pa_pos_entry_mode: NumericCode::new(5).unwrap_or_else(|error| panic!("5: {error}")),
        pa_merchant_id: Alnum::move_from("000000000000001"),
        pa_merchant_name: Alnum::move_from("CORNER GROCERY"),
        pa_merchant_city: Alnum::move_from("SEATTLE"),
        pa_merchant_state: Alnum::move_from("WA"),
        pa_merchant_zip: Alnum::move_from("98101"),
        pa_transaction_id: Alnum::move_from("TX0000000000001"),
        pa_match_status: MatchStatus::Pending,
        pa_auth_fraud: FraudFlag::Blank,
        pa_fraud_rpt_date: Alnum::spaces(),
    }
}

/// A fraud report of [`SAMPLE_CARD`] holding every column, as `COPAUS2C` inserts one.
fn sample_fraud() -> AuthFraud {
    let auth_ts: NaiveDateTime = NaiveDate::from_ymd_opt(2022, 7, 1)
        .and_then(|date| date.and_hms_micro_opt(12, 34, 56, 789_000))
        .unwrap_or_else(|| panic!("2022-07-01 12:34:56.789 is a timestamp"));
    AuthFraud {
        card_num: CardNumber::move_from(SAMPLE_CARD),
        auth_ts,
        auth_type: Some(Alnum::move_from("SAL")),
        card_expiry_date: Some(Alnum::move_from("0225")),
        message_type: Some(Alnum::move_from("0100")),
        message_source: Some(Alnum::move_from("POS")),
        auth_id_code: Some(Alnum::move_from("654321")),
        auth_resp_code: Some(Alnum::move_from("00")),
        auth_resp_reason: Some(Alnum::move_from("0000")),
        processing_code: Some(Alnum::move_from("123456")),
        transaction_amt: Some(money(-12_345)),
        approved_amt: Some(money(999_999_999_999)),
        merchant_catagory_code: Some(Alnum::move_from("5411")),
        acqr_country_code: Some(Alnum::move_from("840")),
        pos_entry_mode: Some(NumericCode::new(5).unwrap_or_else(|error| panic!("5: {error}"))),
        merchant_id: Some(Alnum::move_from("000000000000001")),
        merchant_name: Some(Alnum::move_from("CORNER GROCERY")),
        merchant_city: Some(Alnum::move_from("SEATTLE")),
        merchant_state: Some(Alnum::move_from("WA")),
        merchant_zip: Some(Alnum::move_from("98101")),
        transaction_id: Some(Alnum::move_from("TX0000000000001")),
        match_status: Some(Alnum::move_from("P")),
        auth_fraud: Some(Alnum::move_from("F")),
        fraud_rpt_date: NaiveDate::from_ymd_opt(2022, 7, 2),
        acct_id: Some(sample_account()),
        cust_id: Some(sample_customer()),
    }
}

/// The seeded master of type `01`, as `DB2LTTYP` loads it.
fn sample_type() -> TranTypeMaster {
    TranTypeMaster {
        tr_type: TranTypeCode::move_from("01"),
        tr_description: "PURCHASE".to_owned(),
    }
}

/// The seeded master category `01`/`0001`, as `DB2LTCAT` loads it.
fn sample_category() -> TranCategoryMaster {
    let category = TranCategoryCode::new(1).unwrap_or_else(|error| panic!("0001: {error}"));
    TranCategoryMaster::try_new(
        TranTypeCode::move_from("01"),
        category,
        "REGULAR SALES DRAFT",
    )
    .unwrap_or_else(|error| panic!("the category 01/0001: {error}"))
}

/// Writes the sample summary, detail and fraud report through the persistence adapters,
/// commits, reads each back with the seeded type `01`, and returns the detail's identity.
fn store_samples(
    h: &Harness,
    summary: &PendingAuthSummary,
    detail: &NewPendingAuthDetail,
    fraud: &AuthFraud,
) -> AuthId {
    block_on(async {
        let db = h.db();
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin: {error}"));
        let auth_id = {
            let mut store = uow.store();
            if let Err(error) = store.insert_summary(summary.clone()).await {
                panic!("insert_summary: {error}");
            }
            let auth_id = store
                .insert_detail(detail.clone())
                .await
                .unwrap_or_else(|error| panic!("insert_detail: {error}"));
            if let Err(error) = store.insert_fraud(fraud.clone()).await {
                panic!("insert_fraud: {error}");
            }
            auth_id
        };
        if let Err(error) = uow.commit().await {
            panic!("commit: {error}");
        }
        let mut uow = db
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin: {error}"));
        {
            let mut store = uow.store();
            let read = store.read_summary(summary.pa_acct_id).await;
            assert_eq!(
                read.as_ref().ok(),
                Some(summary),
                "summary read back: {read:?}"
            );
            let read = store.read_detail(auth_id).await;
            let read = read.as_ref().map(PendingAuthDetail::to_new);
            assert_eq!(
                read.as_ref().ok(),
                Some(detail),
                "detail read back: {read:?}"
            );
            let read = store.read_fraud(fraud.card_num, fraud.auth_ts).await;
            assert_eq!(
                read.as_ref().ok(),
                Some(fraud),
                "fraud report read back: {read:?}"
            );
            let read = store.read_tran_type(TranTypeCode::move_from("01")).await;
            assert_eq!(read.ok(), Some(sample_type()), "seeded type 01 read back");
        }
        if let Err(error) = uow.rollback().await {
            panic!("rollback: {error}");
        }
        auth_id
    })
}

// ---------------------------------------------------------------------------------------
// The tables
// ---------------------------------------------------------------------------------------

/// `CIPAUSMY` `PENDING-AUTH-SUMMARY` as `pending_auth_summaries`, and its IMS key `ACCNTID`.
fn check_summaries(doc: &Document, schema: &Schema, s: &PendingAuthSummary) {
    let fields = [
        field("P-CIPAUSMY-001", "pa_acct_id", &s.pa_acct_id),
        field("P-CIPAUSMY-002", "pa_cust_id", &s.pa_cust_id),
        field("P-CIPAUSMY-003", "pa_auth_status", &s.pa_auth_status),
        field("P-CIPAUSMY-004", "pa_account_status", &s.pa_account_status),
        field("P-CIPAUSMY-005", "pa_credit_limit", &s.pa_credit_limit),
        field("P-CIPAUSMY-006", "pa_cash_limit", &s.pa_cash_limit),
        field("P-CIPAUSMY-007", "pa_credit_balance", &s.pa_credit_balance),
        field("P-CIPAUSMY-008", "pa_cash_balance", &s.pa_cash_balance),
        field(
            "P-CIPAUSMY-009",
            "pa_approved_auth_cnt",
            &s.pa_approved_auth_cnt,
        ),
        field(
            "P-CIPAUSMY-010",
            "pa_declined_auth_cnt",
            &s.pa_declined_auth_cnt,
        ),
        field(
            "P-CIPAUSMY-011",
            "pa_approved_auth_amt",
            &s.pa_approved_auth_amt,
        ),
        field(
            "P-CIPAUSMY-012",
            "pa_declined_auth_amt",
            &s.pa_declined_auth_amt,
        ),
    ];
    check_fields(
        doc,
        schema,
        "pending_auth_summaries",
        Layout::Segment,
        &fields,
    );
    check_segment_filler(
        doc,
        schema,
        "pending_auth_summaries",
        "P-CIPAUSMY-013",
        &fields,
        &[],
    );
    // ims_key=ACCNTID SEQ,U START=1 BYTES=6 TYPE=P: the unique sequence field is the key.
    let row = layout_row(doc, "P-CIPAUSMY-001");
    let key = expected_value(doc, row.id, "ims_key");
    assert!(key.contains("SEQ,U"), "{}: ims_key={key}", row.id);
    assert_eq!(
        schema.primary_key("pending_auth_summaries"),
        [row.column.clone()]
    );
    assert_eq!(
        row.offset.map(|offset| offset + 1),
        Some(number_after(&key, "START="))
    );
    assert_eq!(
        row.length,
        number_after(&key, "BYTES="),
        "{}: BYTES=",
        row.id
    );
    assert_eq!(
        key.contains("TYPE=P"),
        row.usage == "COMP-3",
        "{}: TYPE=P is packed",
        row.id
    );
}

/// `CIPAUDTY` `PENDING-AUTH-DETAILS` as `pending_auth_details`.
fn check_details(doc: &Document, schema: &Schema, d: &NewPendingAuthDetail) {
    let fields = [
        field("P-CIPAUDTY-001", "pa_auth_date_9c", &d.pa_auth_date_9c),
        field("P-CIPAUDTY-002", "pa_auth_time_9c", &d.pa_auth_time_9c),
        field("P-CIPAUDTY-003", "pa_auth_orig_date", &d.pa_auth_orig_date),
        field("P-CIPAUDTY-004", "pa_auth_orig_time", &d.pa_auth_orig_time),
        field("P-CIPAUDTY-005", "pa_card_num", &d.pa_card_num),
        field("P-CIPAUDTY-006", "pa_auth_type", &d.pa_auth_type),
        field(
            "P-CIPAUDTY-007",
            "pa_card_expiry_date",
            &d.pa_card_expiry_date,
        ),
        field("P-CIPAUDTY-008", "pa_message_type", &d.pa_message_type),
        field("P-CIPAUDTY-009", "pa_message_source", &d.pa_message_source),
        field("P-CIPAUDTY-010", "pa_auth_id_code", &d.pa_auth_id_code),
        field("P-CIPAUDTY-011", "pa_auth_resp_code", &d.pa_auth_resp_code),
        field(
            "P-CIPAUDTY-012",
            "pa_auth_resp_reason",
            &d.pa_auth_resp_reason,
        ),
        field(
            "P-CIPAUDTY-013",
            "pa_processing_code",
            &d.pa_processing_code,
        ),
        field(
            "P-CIPAUDTY-014",
            "pa_transaction_amt",
            &d.pa_transaction_amt,
        ),
        field("P-CIPAUDTY-015", "pa_approved_amt", &d.pa_approved_amt),
        field(
            "P-CIPAUDTY-016",
            "pa_merchant_catagory_code",
            &d.pa_merchant_catagory_code,
        ),
        field(
            "P-CIPAUDTY-017",
            "pa_acqr_country_code",
            &d.pa_acqr_country_code,
        ),
        field("P-CIPAUDTY-018", "pa_pos_entry_mode", &d.pa_pos_entry_mode),
        field("P-CIPAUDTY-019", "pa_merchant_id", &d.pa_merchant_id),
        field("P-CIPAUDTY-020", "pa_merchant_name", &d.pa_merchant_name),
        field("P-CIPAUDTY-021", "pa_merchant_city", &d.pa_merchant_city),
        field("P-CIPAUDTY-022", "pa_merchant_state", &d.pa_merchant_state),
        field("P-CIPAUDTY-023", "pa_merchant_zip", &d.pa_merchant_zip),
        field("P-CIPAUDTY-024", "pa_transaction_id", &d.pa_transaction_id),
        field("P-CIPAUDTY-025", "pa_match_status", &d.pa_match_status),
        field("P-CIPAUDTY-026", "pa_auth_fraud", &d.pa_auth_fraud),
        field("P-CIPAUDTY-027", "pa_fraud_rpt_date", &d.pa_fraud_rpt_date),
    ];
    check_fields(
        doc,
        schema,
        "pending_auth_details",
        Layout::Segment,
        &fields,
    );
    // Beside the segment's fields: the REST key auth_id and the parent summary's key.
    check_segment_filler(
        doc,
        schema,
        "pending_auth_details",
        "P-CIPAUDTY-028",
        &fields,
        &["auth_id", "pa_acct_id"],
    );
    // The REST key of P-CIPAUDTY-005: the identity auth_id, alongside the IMS key columns.
    assert!(
        layout_row(doc, "P-CIPAUDTY-005")
            .expected
            .contains("auth_id is the REST key"),
        "P-CIPAUDTY-005 names auth_id as the REST key"
    );
    assert_eq!(schema.primary_key("pending_auth_details"), ["auth_id"]);
    assert_eq!(
        schema.column("pending_auth_details", "auth_id").identity,
        "a"
    );
}

/// The `FILLER` row `id` that ends the IMS segment stored as `table`, whose cited fields are
/// `fields`. No Rust field models it, since no codec layout encodes the segment, and no column
/// stores it: it spans the bytes from the end of the stored fields to the end of the segment,
/// and `table` holds exactly the columns of `fields` and the `others` the segment does not
/// define.
fn check_segment_filler(
    doc: &Document,
    schema: &Schema,
    table: &str,
    id: &'static str,
    fields: &[Field],
    others: &[&str],
) {
    let row = layout_row(doc, id);
    assert!(
        row.pic.starts_with("FILLER "),
        "{id}: {} is no FILLER",
        row.pic
    );
    assert_eq!(
        (
            code_type_name(&row.rust).as_str(),
            row.sql.as_str(),
            row.column.as_str()
        ),
        ("—", "not stored", "—"),
        "{id}: rust=, sql= and column="
    );
    let end: usize = fields
        .iter()
        .map(|field| layout_row(doc, field.id).length)
        .sum();
    let (segment, _) = parity_row(doc, id);
    assert_eq!(row.offset, Some(end), "{id}: FILLER follows the fields");
    assert_eq!(
        end + row.length,
        record_length(segment),
        "{id}: FILLER ends the segment"
    );
    let mut expected: Vec<&str> = fields
        .iter()
        .map(|field| field.column)
        .chain(others.iter().copied())
        .collect();
    let mut columns: Vec<&str> = schema
        .columns
        .iter()
        .filter(|column| column.table == table)
        .map(|column| column.name.as_str())
        .collect();
    expected.sort_unstable();
    columns.sort_unstable();
    assert_eq!(
        columns, expected,
        "{id}: {table} stores the cited fields and no FILLER"
    );
}

/// The flags of `P-CIPAUDTY-025` and `P-CIPAUDTY-026` hold exactly what their `X(01)` item
/// holds: every byte converts to the flag and back unchanged, and every flag to its byte.
fn check_flags_are_alnum_1() {
    for byte in u8::MIN..=u8::MAX {
        let item = Alnum::from_array([byte]);
        assert_eq!(
            MatchStatus::from_alnum(item).to_alnum(),
            item,
            "P-CIPAUDTY-025: {byte}"
        );
        assert_eq!(
            FraudFlag::from_alnum(item).to_alnum(),
            item,
            "P-CIPAUDTY-026: {byte}"
        );
        let status = MatchStatus::from_alnum(item);
        assert_eq!(MatchStatus::from_alnum(status.to_alnum()), status);
        let flag = FraudFlag::from_alnum(item);
        assert_eq!(FraudFlag::from_alnum(flag.to_alnum()), flag);
    }
}

/// The IMS key `PAUT9CTS` of `P-CIPAUDTY-001` and `P-CIPAUDTY-002`: unique under the parent,
/// its two parts at bytes 1-3 and 4-8, each the complement the row names.
fn check_detail_key(doc: &Document, schema: &Schema, now: DateTime<Utc>) {
    let date = layout_row(doc, "P-CIPAUDTY-001");
    let time = layout_row(doc, "P-CIPAUDTY-002");
    let key = expected_value(doc, date.id, "ims_key");
    assert!(key.contains("SEQ,U"), "{}: ims_key={key}", date.id);
    let unique = vec![
        "pa_acct_id".to_owned(),
        date.column.clone(),
        time.column.clone(),
    ];
    assert!(
        schema
            .keys
            .iter()
            .any(|k| k.table == "pending_auth_details" && k.kind == "u" && k.columns == unique),
        "{}: PAUT9CTS is unique under its parent {unique:?}",
        date.id
    );
    assert_eq!(
        date.offset.map(|offset| offset + 1),
        Some(number_after(&key, "START="))
    );
    assert_eq!(
        date.length + time.length,
        number_after(&key, "BYTES="),
        "{}: BYTES=",
        date.id
    );
    let parts = expected_value(doc, time.id, "ims_key");
    let (first, last) = parts
        .split_once("bytes ")
        .and_then(|(_, range)| range.split_once('-'))
        .map_or_else(
            || panic!("{}: ims_key={parts} names no byte range", time.id),
            |(first, last)| (leading_number(first), leading_number(last)),
        );
    assert_eq!(
        time.offset.map(|offset| offset + 1),
        Some(first),
        "{}: first byte",
        time.id
    );
    assert_eq!(
        time.offset.map(|offset| offset + time.length),
        Some(last),
        "{}",
        time.id
    );
    let (date_part, time_part) = clock::auth_key_complement(now);
    let yyddd = i64::from(clock::yyddd(now));
    assert_eq!(yyddd, 22_182, "YYDDD of {SAMPLE_INSTANT}");
    let date_base = number_after(&date.expected, "value ");
    let date_base = i64::try_from(date_base).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        date.expected.contains(" - YYDDD"),
        "{}: value - YYDDD",
        date.id
    );
    assert_eq!(
        i64::from(date_part),
        date_base - yyddd,
        "{}: the key's date",
        date.id
    );
    let time_base = number_after(&time.expected, "value ");
    let time_base = i64::try_from(time_base).unwrap_or_else(|error| panic!("{error}"));
    assert!(
        time.expected.contains(" - HHMMSSmmm"),
        "{}: value - HHMMSSmmm",
        time.id
    );
    assert_eq!(
        time_part,
        time_base - SAMPLE_HHMMSSMMM,
        "{}: the key's time",
        time.id
    );
}

/// `AUTHFRDS` as `auth_fraud`, keyed by (`CARD_NUM`, `AUTH_TS`).
fn check_fraud(doc: &Document, schema: &Schema, f: &AuthFraud) {
    let fields = [
        field("P-AUTHFRDS-001", "card_num", &f.card_num),
        field("P-AUTHFRDS-002", "auth_ts", &f.auth_ts),
        field("P-AUTHFRDS-003", "auth_type", &f.auth_type),
        field("P-AUTHFRDS-004", "card_expiry_date", &f.card_expiry_date),
        field("P-AUTHFRDS-005", "message_type", &f.message_type),
        field("P-AUTHFRDS-006", "message_source", &f.message_source),
        field("P-AUTHFRDS-007", "auth_id_code", &f.auth_id_code),
        field("P-AUTHFRDS-008", "auth_resp_code", &f.auth_resp_code),
        field("P-AUTHFRDS-009", "auth_resp_reason", &f.auth_resp_reason),
        field("P-AUTHFRDS-010", "processing_code", &f.processing_code),
        field("P-AUTHFRDS-011", "transaction_amt", &f.transaction_amt),
        field("P-AUTHFRDS-012", "approved_amt", &f.approved_amt),
        field(
            "P-AUTHFRDS-013",
            "merchant_catagory_code",
            &f.merchant_catagory_code,
        ),
        field("P-AUTHFRDS-014", "acqr_country_code", &f.acqr_country_code),
        field("P-AUTHFRDS-015", "pos_entry_mode", &f.pos_entry_mode),
        field("P-AUTHFRDS-016", "merchant_id", &f.merchant_id),
        field("P-AUTHFRDS-017", "merchant_name", &f.merchant_name),
        field("P-AUTHFRDS-018", "merchant_city", &f.merchant_city),
        field("P-AUTHFRDS-019", "merchant_state", &f.merchant_state),
        field("P-AUTHFRDS-020", "merchant_zip", &f.merchant_zip),
        field("P-AUTHFRDS-021", "transaction_id", &f.transaction_id),
        field("P-AUTHFRDS-022", "match_status", &f.match_status),
        field("P-AUTHFRDS-023", "auth_fraud", &f.auth_fraud),
        field("P-AUTHFRDS-024", "fraud_rpt_date", &f.fraud_rpt_date),
        field("P-AUTHFRDS-025", "acct_id", &f.acct_id),
        field("P-AUTHFRDS-026", "cust_id", &f.cust_id),
    ];
    check_fields(doc, schema, "auth_fraud", Layout::Db2, &fields);
    assert_eq!(
        schema.primary_key("auth_fraud"),
        listed_primary_key(doc, "P-AUTHFRDS-001")
    );
}

/// The primary key row `id` names, `primary key (A, B)`, lower-cased; a bare `primary key`
/// is the row's own column.
fn listed_primary_key(doc: &Document, id: &'static str) -> Vec<String> {
    let row = layout_row(doc, id);
    let listed = row.expected.split_once("primary key").map_or_else(
        || panic!("{id} names no primary key"),
        |(_, rest)| rest.trim_start(),
    );
    listed
        .strip_prefix('(')
        .and_then(|rest| rest.split_once(')'))
        .map_or_else(
            || vec![row.column.clone()],
            |(columns, _)| {
                columns
                    .split(", ")
                    .map(str::to_lowercase)
                    .collect::<Vec<String>>()
            },
        )
}

/// The Db2 masters `TRNTYPE` and `TRNTYCAT` as `transaction_types` and
/// `transaction_categories`: their columns, keys and the foreign key between them.
fn check_masters(doc: &Document, schema: &Schema) {
    let master = sample_type();
    let category = sample_category();
    let types = [
        field("P-TRNTYPE-001", "tr_type", &master.tr_type),
        description_item(&master),
    ];
    check_fields(doc, schema, "transaction_types", Layout::Db2, &types);
    let categories = [
        field("P-TRNTYCAT-001", "trc_type_code", &category.trc_type_code),
        field(
            "P-TRNTYCAT-002",
            "trc_type_category",
            &category.trc_type_category,
        ),
        field("P-TRNTYCAT-003", "trc_cat_data", &category.trc_cat_data),
    ];
    check_fields(
        doc,
        schema,
        "transaction_categories",
        Layout::Db2,
        &categories,
    );
    for (id, table) in [
        ("P-TRNTYPE-001", "transaction_types"),
        ("P-TRNTYCAT-001", "transaction_categories"),
    ] {
        assert_eq!(
            schema.primary_key(table),
            listed_primary_key(doc, id),
            "{id}"
        );
    }
    // foreign key to TRANSACTION_TYPE(TR_TYPE) ON DELETE RESTRICT
    let id = "P-TRNTYCAT-001";
    let row = layout_row(doc, id);
    let target = row
        .expected
        .split_once("foreign key to ")
        .map_or_else(|| panic!("{id} names no foreign key"), |(_, rest)| rest);
    let (db2_table, rest) = target
        .split_once('(')
        .unwrap_or_else(|| panic!("{id}: {target}"));
    let (db2_column, rest) = rest
        .split_once(')')
        .unwrap_or_else(|| panic!("{id}: {rest}"));
    assert!(
        rest.trim_start().starts_with("ON DELETE RESTRICT"),
        "{id}: {rest}"
    );
    let table = stored_table(doc, db2_table);
    assert!(
        schema
            .keys
            .iter()
            .any(|key| key.table == "transaction_categories"
                && key.kind == "f"
                && key.columns == [row.column.clone()]
                && key.references == Some((table.clone(), vec![db2_column.to_lowercase()]))
                && key.on_delete == "r"),
        "{id}: {} references {table}({db2_column}) ON DELETE RESTRICT",
        row.column
    );
}

/// The field of `TR_DESCRIPTION` (P-TRNTYPE-002) as the `Alnum<50>` item its row names.
///
/// A master holds the description as text of its own length. That text converts into the
/// item and back without loss: `TranTypeMaster::try_new` admits a description exactly when
/// the item can hold it (at most 50 characters, each one ISO 8859-1 byte) and keeps it as
/// given, and the right-trimmed text the column stores is the item's own trimmed text.
fn description_item(master: &TranTypeMaster) -> Field {
    let id = "P-TRNTYPE-002";
    let item = Alnum::<TR_DESCRIPTION_MAX_LEN>::from_trimmed(&master.tr_description)
        .unwrap_or_else(|error| panic!("{id}: {:?}: {error}", master.tr_description));
    assert_eq!(
        item.trimmed(),
        master.tr_description,
        "{id}: the stored description is the item's trimmed text"
    );
    let width = TR_DESCRIPTION_MAX_LEN;
    for text in [
        "A".repeat(width),
        "\u{e9}".repeat(width),
        "A".repeat(width + 1),
        "\u{20ac}".to_owned(),
        format!("{:<width$}", "REVERSAL"),
    ] {
        let as_item = Alnum::<TR_DESCRIPTION_MAX_LEN>::from_trimmed(&text);
        let as_master = TranTypeMaster::try_new(master.tr_type, text.clone());
        assert_eq!(
            as_master.as_ref().err(),
            as_item.as_ref().err(),
            "{id}: {text:?} is admitted as the item admits it"
        );
        if let (Ok(kept), Ok(held)) = (&as_master, &as_item) {
            assert_eq!(
                held.trimmed(),
                kept.tr_description.trim_end_matches(' '),
                "{id}: {text:?} reads back from its stored form"
            );
        }
    }
    field(id, "tr_description", &item)
}

/// The table that stores the Db2 table `db2_table`, as the heading of its section A table
/// names it: `TRNTYPE Db2 table CARDDEMO.TRANSACTION_TYPE (transaction_types)`.
fn stored_table(doc: &Document, db2_table: &str) -> String {
    let marker = format!("CARDDEMO.{db2_table} (");
    doc.tables()
        .iter()
        .filter_map(|table| table.subsection.as_deref())
        .find_map(|heading| heading.split_once(&marker))
        .and_then(|(_, rest)| rest.split_once(')'))
        .map_or_else(
            || panic!("no section A heading names CARDDEMO.{db2_table}"),
            |(name, _)| name.to_owned(),
        )
}

/// The CP00 fields no table stores: no column anywhere bears the field's name.
fn check_not_stored(doc: &Document, schema: &Schema) {
    for id in NOT_STORED {
        let row = layout_row(doc, id);
        assert_eq!(row.sql, "not stored", "{id}: sql=");
        assert_eq!(row.column, "—", "{id}: column=");
        let name = row
            .pic
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_lowercase()
            .replace('-', "_");
        let stored: Vec<String> = schema
            .columns
            .iter()
            .filter(|column| column.name == name)
            .map(|column| format!("{}.{}", column.table, column.name))
            .collect();
        assert!(stored.is_empty(), "{id}: {name} is stored in {stored:?}");
    }
}

/// Every cited row of the authorization stores and the Db2 masters holds the migrated
/// schema, its constraints and the form the persistence adapters store each field in.
#[test]
fn authorization_and_db2_tables_hold_their_section_a_rows() {
    let doc = parity();
    let h = Harness::new();
    let now = sample_instant();
    let summary = sample_summary();
    let detail = sample_detail(now);
    let fraud = sample_fraud();
    let auth_id = store_samples(&h, &summary, &detail, &fraud);
    assert!(
        auth_id > AuthId::from(0),
        "the detail identity is generated: {auth_id:?}"
    );
    let schema = Schema::read(&h);
    check_summaries(&doc, &schema, &summary);
    check_details(&doc, &schema, &detail);
    check_flags_are_alnum_1();
    check_detail_key(&doc, &schema, now);
    check_fraud(&doc, &schema, &fraud);
    check_masters(&doc, &schema);
    check_not_stored(&doc, &schema);
}

// ---------------------------------------------------------------------------------------
// Section D: the seeded state
// ---------------------------------------------------------------------------------------

/// The keyed read of the Db2 type `code` through the persistence adapter of
/// `TranTypeMasterStore`, in a unit of work that is rolled back.
fn read_type(h: &Harness, code: &str) -> Result<TranTypeMaster, PortError> {
    block_on(async {
        let mut uow = h
            .db()
            .begin()
            .await
            .unwrap_or_else(|error| panic!("begin: {error}"));
        let read = uow
            .store()
            .read_tran_type(TranTypeCode::move_from(code))
            .await;
        if let Err(error) = uow.rollback().await {
            panic!("rollback: {error}");
        }
        read
    })
}

/// The codes of a range `01-07`, each zero-padded to the width of the first.
fn code_range(id: &str, range: &str) -> Vec<String> {
    let (first, last) = range
        .split_once('-')
        .unwrap_or_else(|| panic!("{id}: codes={range} is not a range"));
    let width = first.len();
    (leading_number(first)..=leading_number(last))
        .map(|code| format!("{code:0width$}"))
        .collect()
}

/// The state `batch seed` leaves: the 7 Db2 types and 18 categories with their audited
/// examples, and empty pending-authorization and fraud stores.
#[test]
fn seeded_masters_and_authorization_stores_hold_their_section_d_rows() {
    let doc = parity();
    let h = Harness::new();
    let rows = |sql: &str| block_on(h.stack().fetch_text_rows(sql));
    let counts: BTreeMap<String, usize> = rows(SEEDED_COUNTS_SQL)
        .into_iter()
        .map(|row| {
            let table = text(&row, 0, "table");
            (table, leading_number(&text(&row, 1, "count")))
        })
        .collect();
    for id in ["P-TRNTYPE-003", "P-TRNTYCAT-004"] {
        let table = expected_value(&doc, id, "table");
        let expected = leading_number(&expected_value(&doc, id, "rows"));
        assert_eq!(counts.get(&table), Some(&expected), "{id}: rows of {table}");
    }
    let mut kind = None;
    let mut seen = 0;
    for pair in expected_values(&doc, "P-CIPAUSMY-014") {
        match pair.key.as_str() {
            "kind" => kind = Some(pair.value),
            "rows" => {
                let name = kind.take().unwrap_or_else(|| panic!("rows= before kind="));
                let (_, tables) = SEEDED_STORES
                    .iter()
                    .find(|(store, _)| *store == name)
                    .unwrap_or_else(|| panic!("P-CIPAUSMY-014: unknown kind {name}"));
                for table in *tables {
                    let expected = leading_number(&pair.value);
                    assert_eq!(
                        counts.get(*table),
                        Some(&expected),
                        "P-CIPAUSMY-014: {table}"
                    );
                }
                seen += 1;
            }
            _ => {}
        }
    }
    assert_eq!(
        seen,
        SEEDED_STORES.len(),
        "P-CIPAUSMY-014 counts every store"
    );
    let codes: Vec<String> = rows(SEEDED_TYPES_SQL)
        .into_iter()
        .map(|row| text(&row, 0, "tr_type"))
        .collect();
    let id = "P-TRNTYPE-004";
    let code = exact(&expected_span(&doc, id, "value"));
    let master = read_type(&h, &code).unwrap_or_else(|error| panic!("{id}: {code}: {error}"));
    assert_eq!(master.tr_type, TranTypeCode::move_from(&code), "{id}");
    assert_eq!(
        master.tr_description,
        exact(&expected_span(&doc, id, "tr_description"))
    );
    assert_eq!(
        codes,
        code_range(id, &expected_value(&doc, id, "codes")),
        "{id}: codes="
    );
    let id = "P-TRNTYPE-005";
    let missing = exact(&expected_span(&doc, id, "value"));
    assert!(!codes.contains(&missing), "{id}: {missing} is not seeded");
    let read = read_type(&h, &missing);
    assert!(matches!(read, Err(PortError::NotFound)), "{id}: {read:?}");
    let id = "P-TRNTYCAT-005";
    let categories = rows(SEEDED_CATEGORIES_SQL);
    let wanted = [
        Some(expected_value(&doc, id, "tran_type_cd")),
        Some(expected_value(&doc, id, "tran_cat_cd")),
        Some(exact(&expected_span(&doc, id, "trc_cat_data"))),
    ];
    assert!(
        categories.iter().any(|row| *row == wanted),
        "{id}: {wanted:?}"
    );
    let expected = leading_number(&expected_value(&doc, id, "categories"));
    assert_eq!(categories.len(), expected, "{id}: categories=");
}

// ---------------------------------------------------------------------------------------
// The CP00 request, reply and error-log layouts
// ---------------------------------------------------------------------------------------

/// One `UNSTRING` receiver of the request: its row when this file cites it, its type, the
/// bytes it spans in `CCPAURQY` and the bytes it holds.
struct Receiver {
    id: Option<&'static str>,
    rust: RustField,
    bytes: usize,
    held: Vec<u8>,
}

/// An alphanumeric receiver.
fn alnum<const N: usize>(id: Option<&'static str>, item: &Alnum<N>) -> Receiver {
    Receiver {
        id,
        rust: rust_of(item),
        bytes: N,
        held: item.as_bytes().to_vec(),
    }
}

/// The 18 receivers of `req` in `UNSTRING` order; the amount receiver is
/// `PA-RQ-TRANSACTION-AMT`, the `FUNCTION NUMVAL` of its text edited `+9(10).99`.
fn receivers(req: &AuthRequest) -> Vec<Receiver> {
    let amount = auth_decision::parse_amount(req).unwrap_or_else(|error| panic!("{error}"));
    let card = req.pa_rq_card_num;
    vec![
        alnum(Some("P-CCPAURQY-001"), &req.pa_rq_auth_date),
        alnum(Some("P-CCPAURQY-002"), &req.pa_rq_auth_time),
        Receiver {
            rust: rust_of(&card),
            ..alnum(Some("P-CCPAURQY-003"), card.as_alnum())
        },
        alnum(Some("P-CCPAURQY-004"), &req.pa_rq_auth_type),
        alnum(Some("P-CCPAURQY-005"), &req.pa_rq_card_expiry_date),
        alnum(Some("P-CCPAURQY-006"), &req.pa_rq_message_type),
        alnum(Some("P-CCPAURQY-007"), &req.pa_rq_message_source),
        alnum(Some("P-CCPAURQY-008"), &req.pa_rq_processing_code),
        Receiver {
            id: Some("P-CCPAURQY-009"),
            rust: rust_of(&amount),
            bytes: PIC_CCPAU_AMT.width(),
            held: format::edit_money(&PIC_CCPAU_AMT, amount).into_bytes(),
        },
        alnum(Some("P-CCPAURQY-010"), &req.pa_rq_merchant_catagory_code),
        alnum(Some("P-CCPAURQY-011"), &req.pa_rq_acqr_country_code),
        alnum(Some("P-CCPAURQY-012"), &req.pa_rq_pos_entry_mode),
        alnum(Some("P-CCPAURQY-013"), &req.pa_rq_merchant_id),
        alnum(Some("P-CCPAURQY-014"), &req.pa_rq_merchant_name),
        alnum(Some("P-CCPAURQY-015"), &req.pa_rq_merchant_city),
        alnum(Some("P-CCPAURQY-016"), &req.pa_rq_merchant_state),
        alnum(Some("P-CCPAURQY-017"), &req.pa_rq_merchant_zip),
        alnum(Some("P-CCPAURQY-018"), &req.pa_rq_transaction_id),
    ]
}

/// The request text of 18 comma-separated fields, field `k` being `repeat` copies of the
/// `k`-th letter, the amount field at `amount_at` excepted, which is `amount`.
fn request_text(amount_at: usize, amount: &str, repeat: usize) -> Vec<u8> {
    let fields: Vec<String> = (b'A'..=b'R')
        .enumerate()
        .map(|(position, letter)| {
            if position == amount_at {
                amount.to_owned()
            } else {
                char::from(letter).to_string().repeat(repeat)
            }
        })
        .collect();
    fields.join(",").into_bytes()
}

/// The receiver of `row` after the `UNSTRING` moves the one character `letter` into it: an
/// `X(n)` receiver holds it left-justified and space-padded; a `9(n)` receiver, the numeric
/// move, holds it right-justified behind zeros, the non-digit kept as it is.
fn moved(row: &LayoutRow, letter: u8) -> Vec<u8> {
    let numeric = row.pic.contains(" 9(");
    let mut held = vec![if numeric { b'0' } else { b' ' }; row.length];
    let at = if numeric { row.length - 1 } else { 0 };
    held[at] = letter;
    held
}

/// `COPAUA0C` `2100-EXTRACT-REQUEST-MSG` fills each `CCPAURQY` field in the order of its
/// offset: each receiver is its field's width, an `X(n)` one cut on the right and padded with
/// spaces, a `9(n)` one keeping its bytes as text behind zeros ([`moved`]), the amount is
/// `+9(10).99` of a `Money<10>`, and the widths add up to the record.
#[test]
fn cp00_request_receivers_hold_their_section_a_rows() {
    let doc = parity();
    let amount_row = layout_row(&doc, "P-CCPAURQY-009");
    let (table, _) = parity_row(&doc, amount_row.id);
    let amount_at = table
        .rows
        .iter()
        .filter_map(|row| table.cell(row, OFFSET_COLUMN)?.parse::<usize>().ok())
        .filter(|offset| Some(*offset) < amount_row.offset)
        .count();
    // 25 characters exceed every receiver, the widest being 22, and keep the 18 fields within
    // the 500 bytes of `W01-GET-BUFFER` that `parse_request` reads.
    let wide_text = request_text(amount_at, "100.29", 25);
    assert!(
        wide_text.len() <= auth_decision::REQUEST_BUFFER_LEN,
        "the request fits"
    );
    let wide_request = auth_decision::parse_request(&wide_text);
    let narrow_request = auth_decision::parse_request(&request_text(amount_at, "100.29", 1));
    let wide = receivers(&wide_request);
    let narrow = receivers(&narrow_request);
    let mut offset = 0;
    for ((position, long), short) in wide.into_iter().enumerate().zip(narrow) {
        let start = offset;
        offset += long.bytes;
        let Some(id) = long.id else { continue };
        let row = layout_row(&doc, id);
        assert_eq!(
            code_type_name(&row.rust),
            long.rust.name,
            "{id}: rust={}",
            row.rust
        );
        assert_eq!(long.bytes, row.length, "{id}: Length");
        assert_eq!(
            row.offset,
            Some(start),
            "{id}: Offset, UNSTRING receiver {position}"
        );
        assert_eq!(row.usage, "DISPLAY", "{id}: Usage");
        if position == amount_at {
            assert!(
                row.pic.ends_with(&PIC_CCPAU_AMT.to_string()),
                "{id}: {}",
                row.pic
            );
            assert_eq!(
                long.held, b"+0000000100.29",
                "{id}: NUMVAL of 100.29, edited"
            );
            continue;
        }
        let letter = b'A' + u8::try_from(position).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            long.held,
            vec![letter; row.length],
            "{id}: cut at its width"
        );
        assert_eq!(
            short.held,
            moved(&row, letter),
            "{id}: one character moved into its receiver"
        );
        let width = long.rust.width.unwrap_or_default();
        assert_eq!(width, row.length, "{id}: {} width", long.rust.name);
    }
    assert_eq!(
        offset,
        record_length(table),
        "the CCPAURQY receivers fill the record"
    );
}

/// The request of the reply tests: card, transaction ID of 13 of its 15 characters and
/// authorization time, the amount `100.29`.
const REPLY_REQUEST: &[u8] = b"220701,123456,4859452612877065,SAL,0225,0100,POS,123456,100.29,\
5411,840,05,000000000000001,CORNER GROCERY,SEATTLE,WA,98101,TXN0000000001";

/// `COPAUA0C` `6000-MAKE-DECISION` strings the `CCPAURLY` fields in offset order, each at
/// its width and followed by a comma, into the 64-byte reply.
#[test]
fn cp00_reply_fields_hold_their_section_a_rows() {
    let doc = parity();
    let req = auth_decision::parse_request(REPLY_REQUEST);
    let amount = auth_decision::parse_amount(&req).unwrap_or_else(|error| panic!("{error}"));
    let decision = AuthDecision {
        approved: true,
        resp_code: Alnum::move_from("00"),
        reason: Alnum::move_from("0000"),
        approved_amount: amount,
    };
    let reply: Vec<char> = auth_decision::reply_body(&req, &decision).chars().collect();
    let edited = format::edit_money(&PIC_CP00_APPROVED_AMT, amount);
    let card = req.pa_rq_card_num;
    let fields = [
        (
            "P-CCPAURLY-001",
            rust_of(&card),
            card.as_bytes().len(),
            card.to_string(),
        ),
        (
            "P-CCPAURLY-002",
            rust_of(&req.pa_rq_transaction_id),
            req.pa_rq_transaction_id.as_bytes().len(),
            req.pa_rq_transaction_id.to_string(),
        ),
        (
            "P-CCPAURLY-003",
            rust_of(&req.pa_rq_auth_time),
            req.pa_rq_auth_time.as_bytes().len(),
            req.pa_rq_auth_time.to_string(),
        ),
        (
            "P-CCPAURLY-004",
            rust_of(&decision.resp_code),
            decision.resp_code.as_bytes().len(),
            decision.resp_code.to_string(),
        ),
        (
            "P-CCPAURLY-005",
            rust_of(&decision.reason),
            decision.reason.as_bytes().len(),
            decision.reason.to_string(),
        ),
        (
            "P-CCPAURLY-006",
            rust_of(&amount),
            PIC_CP00_APPROVED_AMT.width(),
            edited,
        ),
    ];
    let mut offset = 0;
    for (position, (id, rust, width, shown)) in fields.iter().enumerate() {
        let row = layout_row(&doc, id);
        assert_eq!(
            code_type_name(&row.rust),
            rust.name,
            "{id}: rust={}",
            row.rust
        );
        assert_eq!(*width, row.length, "{id}: Length");
        assert_eq!(row.offset, Some(offset), "{id}: Offset");
        assert_eq!(row.usage, "DISPLAY", "{id}: Usage");
        let start = offset + position;
        let slot: String = reply[start..start + width].iter().collect();
        assert_eq!(
            slot,
            *shown,
            "{id}: reply characters {start} to {}",
            start + width
        );
        assert_eq!(reply[start + width], ',', "{id}: followed by its comma");
        offset += width;
    }
    let (table, _) = parity_row(&doc, "P-CCPAURLY-001");
    assert_eq!(
        offset,
        record_length(table),
        "the CCPAURLY fields fill the record"
    );
    assert_eq!(format::CP00_REPLY_STRING_LEN, offset + fields.len());
    assert_eq!(reply.len(), format::CP00_REPLY_LEN);
    let amount_row = layout_row(&doc, "P-CCPAURLY-006");
    assert!(
        amount_row.pic.ends_with(&PIC_CCPAU_AMT.to_string()),
        "{}",
        amount_row.pic
    );
    assert_eq!(
        PIC_CCPAU_AMT.width(),
        amount_row.length,
        "P-CCPAURLY-006: +9(10).99"
    );
    assert_eq!(format::edit_money(&PIC_CCPAU_AMT, amount), "+0000000100.29");
}

/// `COPAUA0C` `9500-LOG-ERROR` records: the codes are nine display digits and the event key
/// is the card moved into twenty characters; `ERR-CODE-2` follows `ERR-CODE-1`.
#[test]
fn cp00_error_log_fields_hold_their_section_a_rows() {
    let doc = parity();
    let card = CardNumber::move_from(SAMPLE_CARD);
    let read = auth_decision::xref_read_failure_entry(&card, &PortError::NotFound);
    let put = auth_decision::reply_failure_entry(2, 2009);
    let code_1 = layout_row(&doc, "P-CCPAUERY-008");
    let code_2 = layout_row(&doc, "P-CCPAUERY-009");
    let event_key = layout_row(&doc, "P-CCPAUERY-011");
    for (row, rust, shown) in [
        (&code_1, rust_of(&put.code_1), put.code_1.to_string()),
        (&code_2, rust_of(&put.code_2), put.code_2.to_string()),
        (
            &event_key,
            rust_of(&read.event_key),
            read.event_key.to_string(),
        ),
    ] {
        let id = row.id;
        assert_eq!(
            code_type_name(&row.rust),
            rust.name,
            "{id}: rust={}",
            row.rust
        );
        assert_eq!(rust.width, Some(row.length), "{id}: Length");
        assert_eq!(row.usage, "DISPLAY", "{id}: Usage");
        assert_eq!(shown.chars().count(), row.length, "{id}: {shown:?}");
    }
    let width = code_1.length;
    assert_eq!(
        put.code_1.to_string(),
        format!("{:0width$}", 2),
        "P-CCPAUERY-008"
    );
    assert_eq!(
        put.code_2.to_string(),
        format!("{:0width$}", 2009),
        "P-CCPAUERY-009"
    );
    assert!(
        read.code_1.is_numeric() && read.code_2.is_numeric(),
        "{read:?}"
    );
    let width = event_key.length;
    assert_eq!(
        read.event_key.to_string(),
        format!("{SAMPLE_CARD:<width$}"),
        "P-CCPAUERY-011"
    );
    assert_eq!(
        code_2.offset,
        code_1.offset.map(|offset| offset + code_1.length),
        "P-CCPAUERY-009 follows P-CCPAUERY-008"
    );
}

/// The text of the repository file `path`, one character per byte where it is not UTF-8.
fn repository_text(path: &str) -> String {
    let full = fixtures::repo_root().join(path);
    let bytes = fs::read(&full).unwrap_or_else(|error| panic!("{}: {error}", full.display()));
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The 88-level condition names of the `05` item `item` of [`CCPAUERY_COPYBOOK`], each with
/// the one byte of its `VALUE 'x'`.
fn conditions_of(item: &str) -> BTreeMap<String, u8> {
    let mut conditions = BTreeMap::new();
    let mut inside = false;
    for line in repository_text(CCPAUERY_COPYBOOK).lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["05", name, ..] => inside = *name == item,
            ["88", name, "VALUE", value, ..] if inside => {
                let quoted = value.trim_end_matches('.').trim_matches('\'');
                let [byte] = quoted.as_bytes() else {
                    panic!("{CCPAUERY_COPYBOOK}: {name} has no one-byte VALUE: {line}")
                };
                conditions.insert((*name).to_owned(), *byte);
            }
            _ => {}
        }
    }
    assert!(
        !conditions.is_empty(),
        "{CCPAUERY_COPYBOOK} declares no 88-level under {item}"
    );
    conditions
}

/// The condition names [`COPAUA0C_PROGRAM`] sets with `SET <name> TO TRUE`, comment lines
/// (an `*` in the indicator column 7) left out.
fn conditions_set_by_copaua0c() -> BTreeSet<String> {
    repository_text(COPAUA0C_PROGRAM)
        .lines()
        .filter(|line| line.chars().nth(6) != Some('*'))
        .filter_map(|line| {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["SET", name, "TO", truth, ..] if truth.trim_end_matches('.') == "TRUE" => {
                    Some((*name).to_owned())
                }
                _ => None,
            }
        })
        .collect()
}

/// The `ERR-LEVEL` 88-level each [`ErrorLevel`] stands for.
const fn level_condition(level: ErrorLevel) -> &'static str {
    match level {
        ErrorLevel::Warning => "ERR-WARNING",
        ErrorLevel::Critical => "ERR-CRITICAL",
    }
}

/// The `ERR-SUBSYSTEM` 88-level each [`ErrorSource`] stands for.
const fn source_condition(source: ErrorSource) -> &'static str {
    match source {
        ErrorSource::App => "ERR-APP",
        ErrorSource::Cics => "ERR-CICS",
        ErrorSource::Ims => "ERR-IMS",
        ErrorSource::Mq => "ERR-MQ",
    }
}

/// Asserts that `value`, a value the code gives the field of `row`, fills the `Alnum<N>` item
/// the row names without loss: `N` is the row's length, `value` has at most `N` characters
/// (exactly `N` when `exact`), each one ISO 8859-1 byte, and the item's trimmed text, the text
/// the log line carries, is `value`.
fn check_item<const N: usize>(row: &LayoutRow, value: &str, exact: bool) {
    let id = row.id;
    let item = Alnum::<N>::from_trimmed(value)
        .unwrap_or_else(|error| panic!("{id}: {value:?} does not fit Alnum<{N}>: {error}"));
    assert_eq!(
        code_type_name(&row.rust),
        rust_of(&item).name,
        "{id}: rust={}",
        row.rust
    );
    assert_eq!(N, row.length, "{id}: Length");
    assert_eq!(item.trimmed(), value, "{id}: {value:?} through its item");
    if exact {
        assert_eq!(value.chars().count(), N, "{id}: {value:?} fills the item");
    }
}

/// Asserts that the `X(01)` flag of `row`, the `05` item `item`, holds the copybook's 88-level
/// values: each `(condition, byte)` of `held`, one per variant of the code's flag type, is the
/// `Alnum<1>` item of that condition's `VALUE`, and the conditions are exactly those of `item`
/// that `COPAUA0C` sets.
fn check_flag(row: &LayoutRow, item: &str, held: &[(&'static str, u8)]) {
    let id = row.id;
    let conditions = conditions_of(item);
    for &(condition, byte) in held {
        check_item::<1>(row, &char::from(byte).to_string(), true);
        assert_eq!(
            conditions.get(condition),
            Some(&byte),
            "{id}: {condition} is VALUE {:?}",
            char::from(byte)
        );
    }
    let set: BTreeSet<String> = conditions_set_by_copaua0c()
        .into_iter()
        .filter(|name| conditions.contains_key(name))
        .collect();
    let modelled: BTreeSet<String> = held.iter().map(|(name, _)| (*name).to_owned()).collect();
    assert_eq!(
        modelled, set,
        "{id}: the conditions of {item} COPAUA0C sets"
    );
}

/// The `9500-LOG-ERROR` records the rules build without a store: the cross-reference read
/// failure (`C001`), the request get failure and the oversize request (`M003`), the reply put
/// failure (`M004`), and the summary and detail write failures (`I003`, `I004`).
fn error_log_entries() -> Vec<ErrorLogEntry> {
    let card = CardNumber::move_from(SAMPLE_CARD);
    let failure = PortError::Unavailable("store unavailable".to_owned());
    let plan = AuthWritePlan {
        summary: SummaryWrite::Insert(sample_summary()),
        detail: sample_detail(sample_instant()),
    };
    let oversize = vec![b'A'; auth_decision::REQUEST_BUFFER_LEN + 1];
    vec![
        auth_decision::xref_read_failure_entry(&card, &failure),
        auth_decision::request_read_failure_entry(2, 2033),
        auth_decision::oversize_request_entry(&oversize)
            .unwrap_or_else(|| panic!("a request past the buffer is logged")),
        auth_decision::reply_failure_entry(2, 2009),
        auth_decision::write_failure_entry(WriteStep::Summary, &failure, &plan),
        auth_decision::write_failure_entry(WriteStep::Detail, &failure, &plan),
    ]
}

/// `COPAUA0C` `9500-LOG-ERROR` records, the fields the code keeps as text or flags until the
/// record is logged: each value it gives `ERR-DATE`, `ERR-TIME`, `ERR-APPLICATION`,
/// `ERR-PROGRAM`, `ERR-LOCATION` and `ERR-MESSAGE` fills the `Alnum<n>` item its row names
/// without loss, `ERR-LEVEL` and `ERR-SUBSYSTEM` hold the `CCPAUERY` 88-level values `COPAUA0C`
/// sets, and the eleven fields tile the 122-byte record.
#[test]
fn cp00_error_log_values_fill_their_section_a_items() {
    let doc = parity();
    let rows: Vec<LayoutRow> = ERROR_LOG_ROWS
        .iter()
        .map(|id| layout_row(&doc, id))
        .collect();
    let mut offset = 0;
    for row in &rows {
        assert_eq!(row.offset, Some(offset), "{}: Offset", row.id);
        assert_eq!(row.usage, "DISPLAY", "{}: Usage", row.id);
        offset += row.length;
    }
    let (table, _) = parity_row(&doc, ERROR_LOG_ROWS[0]);
    assert_eq!(offset, record_length(table), "the CCPAUERY fields fill it");
    let [
        date,
        time,
        application,
        program,
        location,
        level,
        subsystem,
        _,
        _,
        message,
        _,
    ] = rows.as_slice()
    else {
        panic!("{} CCPAUERY rows, not 11", rows.len())
    };
    for text in ERROR_LOG_INSTANTS {
        let now = DateTime::parse_from_rfc3339(text)
            .unwrap_or_else(|error| panic!("{text} is RFC 3339: {error}"))
            .with_timezone(&Utc);
        check_item::<6>(date, &clock::yymmdd(now), true);
        check_item::<6>(time, &clock::hhmmss_compact(now), true);
    }
    check_item::<8>(application, auth_decision::ERR_APPLICATION, false);
    check_item::<8>(program, auth_decision::ERR_PROGRAM, false);
    for entry in error_log_entries() {
        check_item::<4>(location, entry.location, true);
        check_item::<50>(message, entry.message, false);
    }
    let levels = [ErrorLevel::Warning, ErrorLevel::Critical];
    check_flag(
        level,
        "ERR-LEVEL",
        &levels.map(|v| (level_condition(v), v.code())),
    );
    let sources = [
        ErrorSource::App,
        ErrorSource::Cics,
        ErrorSource::Ims,
        ErrorSource::Mq,
    ];
    check_flag(
        subsystem,
        "ERR-SUBSYSTEM",
        &sources.map(|v| (source_condition(v), v.code())),
    );
}
