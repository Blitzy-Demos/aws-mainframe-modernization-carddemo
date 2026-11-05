//! The transaction master `TRANSACT`: `CVTRA05Y TRAN-RECORD`, 350 bytes
//! [app/cpy/CVTRA05Y.cpy:L4-18] (P-CVTRA05Y-001 to P-CVTRA05Y-014), on the
//! table `transactions` and its view `transaction_master` (E-09).
//!
//! # The live master
//!
//! `batch tranbkp` empties the KSDS, as the `DELETE` and `DEFINE` of `TRANBKP`
//! do [app/jcl/TRANBKP.jcl:L35-67], but a transaction that a reward accrual
//! references must stay as the target of its foreign key
//! (`migrations/0005_rewards.sql`). The table therefore keeps such rows, and
//! the live `TRANSACT` file is the subset flagged `in_master`, which the view
//! `transaction_master` holds (`migrations/0003_transactions.sql`):
//!
//! - every read of the [`TransactionReader`] port goes through the view, so a
//!   rule never sees a row outside the live master;
//! - the base table is named only where a statement changes it or locks it:
//!   the live insert of [`TransactionWriter`], the batch methods
//!   [`PgStore::empty_live_master`] and [`PgStore::load_combined_record`] that
//!   take rows out of the live master and bring them back, and the table lock
//!   of [`PgStore::exclude_master_writers`] that holds the master still while
//!   `batch tranbkp` backs it up and empties it. This is the one repository
//!   module that may name the table (`crates/testkit/tests/hygiene.rs`).
//!
//! # Key order
//!
//! `tran_id`, `tran_card_num` and `tran_proc_ts` are `VARCHAR COLLATE "C"`, so
//! every comparison and `ORDER BY` below follows the byte order of the stored
//! text: the order of the KSDS key and of the COBOL alphanumeric compare. No
//! statement repeats the collation, and a bound value takes the column's.
//!
//! Text is stored right-trimmed and bound right-trimmed. Equality of trimmed
//! values always equals COBOL equality of the space-padded fields, and so does
//! their order whenever no character below the space precedes the padding.
//! The trimmed columns are compared because that keeps the partial index
//! `transactions_master_tran_id` usable for the CT00 browse and agrees with
//! the `max(tran_id)` of [`crate::ids::next_tran_id`]. The orders could part
//! only on a control character inside a key, which no allocated TRAN-ID, no
//! fixture record and no CT00 start key (numeric, P-COTRN00C-005) holds.
//!
//! A TRAN-ID holding a NUL byte names no stored transaction, because
//! `PostgreSQL` text cannot hold NUL. Binding one would fail on the server
//! with an encoding error (SQLSTATE `22021`) rather than find no row, so
//! [`TransactionReader::read_transaction`] answers [`PortError::NotFound`]
//! for it without a query, as the COBOL `READ` answers `NOTFND`.
//!
//! The CR00 date selection, [`TransactionReader::transactions_by_proc_date`],
//! orders and pages by the padded keys instead: `rpad(tran_card_num, 16)` and
//! `rpad(tran_id, 16)`, the space-padded `X(16)` text, against its keyset bound
//! as the same padded text. In the UTF8 database `COLLATE "C"` compares UTF-8
//! bytes, whose order is the code-point order, and the column `CHECK` admits
//! only U+0001..U+00FF, each the one Latin-1 byte the key holds, so that order
//! is exactly the byte order of the `(CardNumber, TranId)` pair, control
//! characters included, which the report rule checks page by page. The partial
//! index `transactions_master_padded_card_tran_id` holds that order over the
//! live rows, so a page can be read in key order from its keyset bound.
//!
//! # Writes
//!
//! Each write, and each read, runs under its own savepoint through
//! `crate::tx::savepoint` and `crate::tx::settle`. A unique
//! violation on `transactions_master_tran_id`, a second live row with one
//! TRAN-ID, is the `DUPKEY`/`DUPREC` of the COBOL writes and maps to
//! [`PortError::Duplicate`]. TRAN-IDs are allocated by
//! [`crate::ids::next_tran_id`], never here.
//!
//! # Rows
//!
//! sqlx sees every view column as nullable, because `PostgreSQL` marks no view
//! column `NOT NULL`. Each select therefore declares its columns non-null with
//! an `AS "col!"` override, which the `NOT NULL` table columns behind the view
//! guarantee. A stored value that does not fit its PIC clause is
//! [`PersistenceError::Corrupt`] naming `transaction_master` and the column.

use carddemo_domain::entities::{NewTransaction, Transaction};
use carddemo_domain::numeric::{AccountId, Alnum, CardNumber, TranId, TransactionRowId};
use carddemo_domain::ports::{
    Browse, Direction, PortError, PortResult, TransactionReader, TransactionWriter,
};
use rust_decimal::Decimal;
use sqlx::PgConnection;

use super::{
    alnum, card_number, limit_i64, merchant_id, money, tran_category_code, tran_id, tran_type_code,
};
use crate::PersistenceError;
use crate::ids::TRAN_ID_LOCK_KEY;
use crate::tx::{PgStore, savepoint, settle};

/// The view every read selects from, as [`PersistenceError::Corrupt`] names it.
const MASTER_VIEW: &str = "transaction_master";

/// The outcome of loading one record of the combined generation into the live
/// master: [`PgStore::load_combined_record`], the `COMBTRAN` `STEP10` `REPRO`
/// without `REPLACE` [app/jcl/COMBTRAN.jcl:L41-48].
///
/// The method writes nothing on [`LoadOutcome::DuplicateLive`]; whether the
/// load goes on after it, and the condition code the run ends with, are the
/// batch job's decision from the `REPRO` duplicate-key rows (P-COMBTRAN-003,
/// P-COMBTRAN-006 to P-COMBTRAN-008).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    /// A live row already has the record's TRAN-ID: the `IDC3308I DUPLICATE
    /// RECORD` of `REPRO` (P-COMBTRAN-003). Nothing was written, and the live
    /// row keeps its content.
    DuplicateLive,
    /// A row outside the live master, equal to the record in TRAN-ID and in all
    /// twelve content columns, was made live again. Its identity is unchanged,
    /// so the reward accruals that reference it stay linked (E-09).
    Revived(TransactionRowId),
    /// No row matched, and the record was inserted as a new live row with this
    /// identity.
    Inserted(TransactionRowId),
}

/// One row of `transaction_master` as the selects below return it, before its
/// columns are held to their PIC clauses.
struct TransactionRow {
    transaction_id: i64,
    tran_id: String,
    tran_type_cd: String,
    tran_cat_cd: i32,
    tran_source: String,
    tran_desc: String,
    tran_amt: Decimal,
    tran_merchant_id: i32,
    tran_merchant_name: String,
    tran_merchant_city: String,
    tran_merchant_zip: String,
    tran_card_num: String,
    tran_orig_ts: String,
    tran_proc_ts: String,
}

/// The stored row read back into the copybook types of P-CVTRA05Y-001 to
/// P-CVTRA05Y-013: text re-padded to its `X(n)` width, the codes and the
/// amount held to their pictures. The identity enters unchecked, because the
/// database assigns it.
impl TryFrom<TransactionRow> for Transaction {
    type Error = PersistenceError;

    fn try_from(row: TransactionRow) -> Result<Self, Self::Error> {
        Ok(Self {
            transaction_id: TransactionRowId::from(row.transaction_id),
            tran_id: tran_id(&row.tran_id, MASTER_VIEW, "tran_id")?,
            tran_type_cd: tran_type_code(&row.tran_type_cd, MASTER_VIEW, "tran_type_cd")?,
            tran_cat_cd: tran_category_code(row.tran_cat_cd, MASTER_VIEW, "tran_cat_cd")?,
            tran_source: alnum::<10>(&row.tran_source, MASTER_VIEW, "tran_source")?,
            tran_desc: alnum::<100>(&row.tran_desc, MASTER_VIEW, "tran_desc")?,
            tran_amt: money::<9>(row.tran_amt, MASTER_VIEW, "tran_amt")?,
            tran_merchant_id: merchant_id(row.tran_merchant_id, MASTER_VIEW, "tran_merchant_id")?,
            tran_merchant_name: alnum::<50>(
                &row.tran_merchant_name,
                MASTER_VIEW,
                "tran_merchant_name",
            )?,
            tran_merchant_city: alnum::<50>(
                &row.tran_merchant_city,
                MASTER_VIEW,
                "tran_merchant_city",
            )?,
            tran_merchant_zip: alnum::<10>(
                &row.tran_merchant_zip,
                MASTER_VIEW,
                "tran_merchant_zip",
            )?,
            tran_card_num: card_number(&row.tran_card_num, MASTER_VIEW, "tran_card_num")?,
            tran_orig_ts: alnum::<26>(&row.tran_orig_ts, MASTER_VIEW, "tran_orig_ts")?,
            tran_proc_ts: alnum::<26>(&row.tran_proc_ts, MASTER_VIEW, "tran_proc_ts")?,
        })
    }
}

/// The rows of a select in the order the select returned them, each held to
/// its copybook types; the first misfit fails the whole read.
fn into_transactions(rows: Vec<TransactionRow>) -> PortResult<Vec<Transaction>> {
    rows.into_iter()
        .map(|row| Transaction::try_from(row).map_err(PortError::from))
        .collect()
}

/// The content of one record as its statements bind it: text right-trimmed,
/// the codes as their `INTEGER` columns and the amount as its `NUMERIC(11,2)`
/// column.
///
/// The fields drop the `tran_` prefix of the columns they bind.
struct RecordBinds {
    id: String,
    type_cd: String,
    cat_cd: i32,
    source: String,
    desc: String,
    amt: Decimal,
    merchant_id: i32,
    merchant_name: String,
    merchant_city: String,
    merchant_zip: String,
    card_num: String,
    orig_ts: String,
    proc_ts: String,
}

impl RecordBinds {
    /// The binds of `record`.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the `TRAN-MERCHANT-ID 9(09)` does not
    /// fit the `INTEGER` column. Every value below `10^9` fits, so this only
    /// guards the conversion instead of cutting a value.
    fn new(record: &NewTransaction) -> PortResult<Self> {
        let merchant_id = i32::try_from(record.tran_merchant_id.get()).map_err(|_| {
            PortError::Unavailable(
                "TRAN-MERCHANT-ID exceeds the INTEGER range of tran_merchant_id".to_owned(),
            )
        })?;
        Ok(Self {
            id: record.tran_id.trimmed().into_owned(),
            type_cd: record.tran_type_cd.trimmed().into_owned(),
            cat_cd: i32::from(record.tran_cat_cd.get()),
            source: record.tran_source.trimmed().into_owned(),
            desc: record.tran_desc.trimmed().into_owned(),
            amt: record.tran_amt.to_decimal(),
            merchant_id,
            merchant_name: record.tran_merchant_name.trimmed().into_owned(),
            merchant_city: record.tran_merchant_city.trimmed().into_owned(),
            merchant_zip: record.tran_merchant_zip.trimmed().into_owned(),
            card_num: record.tran_card_num.trimmed().into_owned(),
            orig_ts: record.tran_orig_ts.trimmed().into_owned(),
            proc_ts: record.tran_proc_ts.trimmed().into_owned(),
        })
    }
}

/// The start of a forward browse from `LOW-VALUES`: the empty text sorts before
/// every stored key, so the browse starts at the first record of the file.
const LOW_VALUES: &str = "";

/// One byte of COBOL `LOW-VALUES`, which `PostgreSQL` text cannot hold: a
/// TRAN-ID holding it names no stored transaction.
const LOW_VALUE: u8 = 0x00;

/// `TRAN-PROC-TS PIC X(26)` [app/cpy/CVTRA05Y.cpy:L17], the most characters
/// `tran_proc_ts` holds.
const PROC_TS_LEN: usize = 26;

/// `TRAN-PROC-DT`, the first 10 bytes of `TRAN-PROC-TS` that the TRANREPT
/// `INCLUDE COND` compares (P-TRANREPT-002).
const PROC_DATE_LEN: usize = 10;

/// The highest character a stored text column holds: the column `CHECK` of
/// `migrations/0003_transactions.sql` admits only U+0001..U+00FF.
const HIGHEST_STORED_CHAR: char = '\u{ff}';

/// The inclusive upper bound on the raw `tran_proc_ts` that keeps exactly the
/// rows whose processing date, `substr(tran_proc_ts, 1, 10)`, is at most `end`
/// under the right-trimmed `C` compare of
/// [`TransactionReader::transactions_by_proc_date`]: the trimmed `end` itself
/// when it is shorter than [`PROC_DATE_LEN`], and otherwise `end` followed by
/// [`HIGHEST_STORED_CHAR`] up to [`PROC_TS_LEN`] characters, the highest value
/// a timestamp of that date can hold. The method gives the proof.
///
/// Each byte of an `Alnum` is one character of its text, so the trimmed bytes
/// count the characters of the trimmed text.
fn proc_ts_upper_bound(end: &Alnum<PROC_DATE_LEN>) -> String {
    let mut bound = end.trimmed().into_owned();
    if end.trimmed_bytes().len() == PROC_DATE_LEN {
        bound.extend(std::iter::repeat_n(
            HIGHEST_STORED_CHAR,
            PROC_TS_LEN - PROC_DATE_LEN,
        ));
    }
    bound
}

/// `STARTBR` at `start` with `READNEXT`: live rows with a TRAN-ID of at least
/// `start`, ascending, up to `limit`, read on `conn` by the static statement of
/// the inputs present. Without `account` the statement reads only the view
/// `transaction_master`, as `COTRN00C` reads only `TRANSACT`; with it, the
/// statement keeps only rows whose card `card_xref` maps to that account
/// (E-17).
///
/// The filtered statement starts from the account's cards in `card_xref` and
/// reads each card through the partial index `transactions_master_card_tran_id`
/// from `(card, start)`, at most `limit` rows: the card's own rows from the
/// start key come first in that order, and the outer filter drops the rows of
/// the cards after it. The `LIMIT` inside the lateral keeps the planner from
/// pushing that filter into the read, where it would walk the TRAN-ID index
/// and skip the other cards' rows one by one. Each card is read once, because
/// `xref_card_num` is the key of `card_xref`, and the account's first `limit`
/// rows in TRAN-ID order are among the rows read, so the page equals a filter
/// of the whole master while its cost grows with `limit` per card, not with
/// the master.
async fn select_forward(
    conn: &mut PgConnection,
    start: &str,
    account: Option<i64>,
    limit: i64,
) -> Result<Vec<TransactionRow>, sqlx::Error> {
    match account {
        None => {
            sqlx::query_as!(
                TransactionRow,
                r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master m
WHERE m.tran_id >= $1
ORDER BY m.tran_id
LIMIT $2"#,
                start,
                limit
            )
            .fetch_all(conn)
            .await
        }
        Some(account) => {
            sqlx::query_as!(
                TransactionRow,
                r#"SELECT m.transaction_id AS "transaction_id!", m.tran_id AS "tran_id!",
       m.tran_type_cd AS "tran_type_cd!", m.tran_cat_cd AS "tran_cat_cd!",
       m.tran_source AS "tran_source!", m.tran_desc AS "tran_desc!", m.tran_amt AS "tran_amt!",
       m.tran_merchant_id AS "tran_merchant_id!", m.tran_merchant_name AS "tran_merchant_name!",
       m.tran_merchant_city AS "tran_merchant_city!", m.tran_merchant_zip AS "tran_merchant_zip!",
       m.tran_card_num AS "tran_card_num!", m.tran_orig_ts AS "tran_orig_ts!",
       m.tran_proc_ts AS "tran_proc_ts!"
FROM card_xref x
CROSS JOIN LATERAL (
    SELECT t.transaction_id, t.tran_id, t.tran_type_cd, t.tran_cat_cd, t.tran_source,
           t.tran_desc, t.tran_amt, t.tran_merchant_id, t.tran_merchant_name,
           t.tran_merchant_city, t.tran_merchant_zip, t.tran_card_num, t.tran_orig_ts,
           t.tran_proc_ts
    FROM transaction_master t
    WHERE (t.tran_card_num, t.tran_id) >= (x.xref_card_num, $1)
    ORDER BY t.tran_card_num, t.tran_id
    LIMIT $3) m
WHERE x.xref_acct_id = $2 AND m.tran_card_num = x.xref_card_num
ORDER BY m.tran_id
LIMIT $3"#,
                start,
                account,
                limit
            )
            .fetch_all(conn)
            .await
        }
    }
}

/// `STARTBR` at `start` with `READPREV`: live rows with a TRAN-ID of at most
/// `start`, descending, up to `limit`, read by the static statement of the
/// inputs present as [`select_forward`] reads: `transaction_master` alone
/// without `account`, and the `card_xref` filter with it. The filtered
/// statement reads each of the account's cards backward from `(card, start)`
/// through `transactions_master_card_tran_id`, at most `limit` rows, the
/// card's own rows first, for the reasons [`select_forward`] gives.
async fn select_backward_from(
    conn: &mut PgConnection,
    start: &str,
    account: Option<i64>,
    limit: i64,
) -> Result<Vec<TransactionRow>, sqlx::Error> {
    match account {
        None => {
            sqlx::query_as!(
                TransactionRow,
                r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master m
WHERE m.tran_id <= $1
ORDER BY m.tran_id DESC
LIMIT $2"#,
                start,
                limit
            )
            .fetch_all(conn)
            .await
        }
        Some(account) => {
            sqlx::query_as!(
                TransactionRow,
                r#"SELECT m.transaction_id AS "transaction_id!", m.tran_id AS "tran_id!",
       m.tran_type_cd AS "tran_type_cd!", m.tran_cat_cd AS "tran_cat_cd!",
       m.tran_source AS "tran_source!", m.tran_desc AS "tran_desc!", m.tran_amt AS "tran_amt!",
       m.tran_merchant_id AS "tran_merchant_id!", m.tran_merchant_name AS "tran_merchant_name!",
       m.tran_merchant_city AS "tran_merchant_city!", m.tran_merchant_zip AS "tran_merchant_zip!",
       m.tran_card_num AS "tran_card_num!", m.tran_orig_ts AS "tran_orig_ts!",
       m.tran_proc_ts AS "tran_proc_ts!"
FROM card_xref x
CROSS JOIN LATERAL (
    SELECT t.transaction_id, t.tran_id, t.tran_type_cd, t.tran_cat_cd, t.tran_source,
           t.tran_desc, t.tran_amt, t.tran_merchant_id, t.tran_merchant_name,
           t.tran_merchant_city, t.tran_merchant_zip, t.tran_card_num, t.tran_orig_ts,
           t.tran_proc_ts
    FROM transaction_master t
    WHERE (t.tran_card_num, t.tran_id) <= (x.xref_card_num, $1)
    ORDER BY t.tran_card_num DESC, t.tran_id DESC
    LIMIT $3) m
WHERE x.xref_acct_id = $2 AND m.tran_card_num = x.xref_card_num
ORDER BY m.tran_id DESC
LIMIT $3"#,
                start,
                account,
                limit
            )
            .fetch_all(conn)
            .await
        }
    }
}

/// `STARTBR` at `HIGH-VALUES` with `READPREV`: live rows from the highest
/// TRAN-ID down, up to `limit`, read by the static statement of the inputs
/// present as [`select_forward`] reads: `transaction_master` alone without
/// `account`, and the `card_xref` filter with it. The filtered statement reads
/// each of the account's cards backward from its highest TRAN-ID through
/// `transactions_master_card_tran_id`, at most `limit` rows, the card's own
/// rows first, for the reasons [`select_forward`] gives.
async fn select_backward_from_end(
    conn: &mut PgConnection,
    account: Option<i64>,
    limit: i64,
) -> Result<Vec<TransactionRow>, sqlx::Error> {
    match account {
        None => {
            sqlx::query_as!(
                TransactionRow,
                r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master m
ORDER BY m.tran_id DESC
LIMIT $1"#,
                limit
            )
            .fetch_all(conn)
            .await
        }
        Some(account) => {
            sqlx::query_as!(
                TransactionRow,
                r#"SELECT m.transaction_id AS "transaction_id!", m.tran_id AS "tran_id!",
       m.tran_type_cd AS "tran_type_cd!", m.tran_cat_cd AS "tran_cat_cd!",
       m.tran_source AS "tran_source!", m.tran_desc AS "tran_desc!", m.tran_amt AS "tran_amt!",
       m.tran_merchant_id AS "tran_merchant_id!", m.tran_merchant_name AS "tran_merchant_name!",
       m.tran_merchant_city AS "tran_merchant_city!", m.tran_merchant_zip AS "tran_merchant_zip!",
       m.tran_card_num AS "tran_card_num!", m.tran_orig_ts AS "tran_orig_ts!",
       m.tran_proc_ts AS "tran_proc_ts!"
FROM card_xref x
CROSS JOIN LATERAL (
    SELECT t.transaction_id, t.tran_id, t.tran_type_cd, t.tran_cat_cd, t.tran_source,
           t.tran_desc, t.tran_amt, t.tran_merchant_id, t.tran_merchant_name,
           t.tran_merchant_city, t.tran_merchant_zip, t.tran_card_num, t.tran_orig_ts,
           t.tran_proc_ts
    FROM transaction_master t
    WHERE t.tran_card_num <= x.xref_card_num
    ORDER BY t.tran_card_num DESC, t.tran_id DESC
    LIMIT $2) m
WHERE x.xref_acct_id = $1 AND m.tran_card_num = x.xref_card_num
ORDER BY m.tran_id DESC
LIMIT $2"#,
                account,
                limit
            )
            .fetch_all(conn)
            .await
        }
    }
}

/// Inserts `record` as a live row and returns its new identity.
///
/// A live row with the same TRAN-ID fails the partial unique index
/// `transactions_master_tran_id` with SQLSTATE `23505`, which
/// [`crate::tx::settle`] reports as [`PortError::Duplicate`].
async fn insert_live(conn: &mut PgConnection, record: &RecordBinds) -> Result<i64, sqlx::Error> {
    sqlx::query!(
        "INSERT INTO transactions (
    tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt,
    tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip,
    tran_card_num, tran_orig_ts, tran_proc_ts, in_master)
VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, TRUE)
RETURNING transaction_id",
        record.id,
        record.type_cd,
        record.cat_cd,
        record.source,
        record.desc,
        record.amt,
        record.merchant_id,
        record.merchant_name,
        record.merchant_city,
        record.merchant_zip,
        record.card_num,
        record.orig_ts,
        record.proc_ts
    )
    .fetch_one(conn)
    .await
    .map(|row| row.transaction_id)
}

/// Whether a live row holds the TRAN-ID `id`.
async fn is_live(conn: &mut PgConnection, id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query!(
        r#"SELECT EXISTS (SELECT 1 FROM transaction_master WHERE tran_id = $1) AS "live!""#,
        id
    )
    .fetch_one(conn)
    .await
    .map(|row| row.live)
}

/// Makes live again the row outside the live master that equals `record` in
/// TRAN-ID and in all twelve content columns, the lowest identity when several
/// do, and returns that identity; `None` when no such row exists.
///
/// Text compares right-trimmed, as stored, and `tran_amt` as `NUMERIC`, so
/// `1.5` equals `1.50`. The outer `NOT in_master` makes the update skip a row
/// a concurrent load revived first.
async fn revive(conn: &mut PgConnection, record: &RecordBinds) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query!(
        "UPDATE transactions SET in_master = TRUE
WHERE NOT in_master
  AND transaction_id = (
      SELECT transaction_id FROM transactions
      WHERE NOT in_master
        AND tran_id = $1 AND tran_type_cd = $2 AND tran_cat_cd = $3
        AND tran_source = $4 AND tran_desc = $5 AND tran_amt = $6
        AND tran_merchant_id = $7 AND tran_merchant_name = $8
        AND tran_merchant_city = $9 AND tran_merchant_zip = $10
        AND tran_card_num = $11 AND tran_orig_ts = $12 AND tran_proc_ts = $13
      ORDER BY transaction_id
      LIMIT 1)
RETURNING transaction_id",
        record.id,
        record.type_cd,
        record.cat_cd,
        record.source,
        record.desc,
        record.amt,
        record.merchant_id,
        record.merchant_name,
        record.merchant_city,
        record.merchant_zip,
        record.card_num,
        record.orig_ts,
        record.proc_ts
    )
    .fetch_optional(conn)
    .await
    .map(|row| row.map(|row| row.transaction_id))
}

/// The three steps of [`PgStore::load_combined_record`], in order, on the
/// savepoint's connection.
async fn load(conn: &mut PgConnection, record: &RecordBinds) -> Result<LoadOutcome, sqlx::Error> {
    if is_live(&mut *conn, &record.id).await? {
        return Ok(LoadOutcome::DuplicateLive);
    }
    if let Some(id) = revive(&mut *conn, record).await? {
        return Ok(LoadOutcome::Revived(TransactionRowId::from(id)));
    }
    let id = insert_live(conn, record).await?;
    Ok(LoadOutcome::Inserted(TransactionRowId::from(id)))
}

/// The two statements of [`PgStore::empty_live_master`] on the savepoint's
/// connection: the rows taken out of the live master, then the rows deleted.
async fn take_master_offline(conn: &mut PgConnection) -> Result<(u64, u64), sqlx::Error> {
    // The base table, not the view: the view's WITH LOCAL CHECK OPTION rejects
    // an update that takes a row out of it.
    let demoted = sqlx::query!("UPDATE transactions SET in_master = FALSE WHERE in_master")
        .execute(&mut *conn)
        .await?
        .rows_affected();
    let deleted = sqlx::query!(
        "DELETE FROM transactions t
WHERE NOT t.in_master
  AND NOT EXISTS (SELECT 1 FROM reward_accruals a WHERE a.transaction_id = t.transaction_id)"
    )
    .execute(conn)
    .await?
    .rows_affected();
    Ok((demoted, deleted))
}

/// The reads of the live master, all from the view `transaction_master`.
impl TransactionReader for PgStore<'_, '_> {
    /// COTRN01C READ-TRANSACT-FILE — P-COTRN01C-004
    /// [app/cbl/COTRN01C.cbl:L267-296]: the live row with the TRAN-ID. The
    /// COBOL reads `UPDATE` and rewrites nothing, so a read without a lock
    /// gives the same result. No row is the `NOTFND` the rule turns into
    /// "Transaction ID NOT found...", and so is a TRAN-ID holding a NUL byte,
    /// without a query.
    async fn read_transaction(&mut self, id: TranId) -> PortResult<Transaction> {
        if id.as_bytes().contains(&LOW_VALUE) {
            return Err(PortError::NotFound);
        }
        let key = id.trimmed();
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            TransactionRow,
            r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master
WHERE tran_id = $1"#,
            key.as_ref()
        )
        .fetch_one(&mut *sp)
        .await;
        let row = settle(sp, result).await?;
        Transaction::try_from(row).map_err(PortError::from)
    }

    /// COTRN00C STARTBR-TRANSACT-FILE, READNEXT-TRANSACT-FILE,
    /// READPREV-TRANSACT-FILE — P-COTRN00C-006, P-COTRN00C-007, P-COTRN00C-011
    /// [app/cbl/COTRN00C.cbl:L591-687]: one page per the [`Browse`] contract,
    /// in TRAN-ID byte order. `STARTBR` positions `GTEQ`, the CICS default
    /// (the explicit option is commented out): a forward page holds the keys at
    /// or above the start key, ascending, and a backward page the keys at or
    /// below it, descending, the start key included either way. A start of
    /// `None` is `LOW-VALUES` forward and `HIGH-VALUES` backward.
    ///
    /// `account_id` is the filter the REST resource adds and `COTRN00C` lacks
    /// (E-17): a row is kept when `card_xref` maps its card to the account. The
    /// filter is part of the statement, so the rows it drops do not count
    /// against the limit. An empty file or a start beyond the last key gives an
    /// empty page, the `STARTBR` `NOTFND`.
    ///
    /// Each of the three starts (forward, backward from a key, backward from
    /// the end) has two static statements, chosen by whether `account_id` is
    /// present (`select_forward`, `select_backward_from`,
    /// `select_backward_from_end`). Without it the statement reads only the
    /// view `transaction_master`, as `COTRN00C` reads only `TRANSACT`, so the
    /// unfiltered browse, the one parity is measured on, never waits on a lock
    /// of `card_xref` nor fails with it. With it the statement starts from the
    /// account's cards in `card_xref` and reads each card's rows in TRAN-ID
    /// order from the start key, so its cost follows the page size rather than
    /// the size of the master. A single statement guarding the `card_xref`
    /// read with `$n IS NULL OR` would name `card_xref` whatever the binding,
    /// and `PostgreSQL` resolves and locks every relation a statement names
    /// before it runs.
    async fn browse_transactions(
        &mut self,
        browse: Browse<TranId>,
        account_id: Option<AccountId>,
    ) -> PortResult<Vec<Transaction>> {
        let limit = limit_i64(browse.limit)?;
        let account = account_id.map(AccountId::to_i64);
        let start = browse.start_key.as_ref().map(TranId::trimmed);
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let conn = &mut *sp;
        let result = match (browse.direction, start.as_deref()) {
            (Direction::Forward, start) => {
                select_forward(conn, start.unwrap_or(LOW_VALUES), account, limit).await
            }
            (Direction::Backward, Some(start)) => {
                select_backward_from(conn, start, account, limit).await
            }
            (Direction::Backward, None) => select_backward_from_end(conn, account, limit).await,
        };
        into_transactions(settle(sp, result).await?)
    }

    /// COTRN02C ADD-TRANSACTION and COPY-LAST-TRAN-DATA — P-COTRN02C-020,
    /// P-COTRN02C-026 [app/cbl/COTRN02C.cbl:L442-451]
    /// [app/cbl/COTRN02C.cbl:L471-495]; COBIL00C PROCESS-ENTER-KEY —
    /// P-COBIL00C-012 [app/cbl/COBIL00C.cbl:L210-219]: `STARTBR` at
    /// `HIGH-VALUES` and one `READPREV`, the highest-keyed live row whatever its
    /// account. `None` on an empty live master, the `STARTBR` `NOTFND` of
    /// P-COTRN02C-021 and P-COBIL00C-013.
    async fn highest_transaction(&mut self) -> PortResult<Option<Transaction>> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = select_backward_from_end(&mut sp, None, 1).await;
        Ok(into_transactions(settle(sp, result).await?)?.pop())
    }

    /// CREASTMT STEP010 — P-CREASTMT-002 [app/jcl/CREASTMT.JCL:L44-55]:
    /// `SORT FIELDS=(263,16,CH,A,1,16,CH,A)`, the live rows by card number
    /// then TRAN-ID, both in byte order, as CBSTM03B 1000-TRNXFILE-PROC —
    /// P-CBSTM03B-002 [app/cbl/CBSTM03B.CBL:L133-155] reads the sorted file.
    /// The keys are unique, so the order is total and the first `limit` rows
    /// are the first `limit` records of the sorted file; the database stops
    /// there instead of returning the whole master. `batch creastmt` passes
    /// the bound of the CBSTM03A 8500-READTRNX-READ table load — P-CBSTM03A-004,
    /// `statement::TRNXFILE_READ_LIMIT`, past which no record changes the load.
    async fn transactions_by_card(&mut self, limit: u16) -> PortResult<Vec<Transaction>> {
        let limit = limit_i64(limit)?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            TransactionRow,
            r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master
ORDER BY tran_card_num, tran_id
LIMIT $1"#,
            limit
        )
        .fetch_all(&mut *sp)
        .await;
        into_transactions(settle(sp, result).await?)
    }

    /// TRANREPT STEP05R — P-TRANREPT-002 [app/proc/TRANREPT.prc:L35-53]: one
    /// page of the `INCLUDE COND=(TRAN-PROC-DT,GE,start,AND,TRAN-PROC-DT,LE,end)`
    /// over `TRAN-PROC-DT`, bytes 305 to 314, the first 10 characters of
    /// `tran_proc_ts`, compared as `CH` text with both bounds inclusive, sorted
    /// by card number then TRAN-ID. The rows are read as CBTRN03C
    /// 1000-TRANFILE-GET-NEXT — P-CBTRN03C-474 [app/cbl/CBTRN03C.cbl:L248-272]
    /// reads the sorted file; the selection serves CR00.
    ///
    /// Up to `limit` rows whose padded card number and TRAN-ID follow `after`,
    /// the keyset of the page before, in the padded-key order of the module
    /// documentation, which is the `(CardNumber, TranId)` order the port
    /// declares. The date range is bounded on the raw column alone, by bounds
    /// that keep exactly the rows of the date compare, so the planner sees the
    /// range's real selectivity. Each page is then read either in key order
    /// from its keyset bound through the partial index
    /// `transactions_master_padded_card_tran_id`, stopping after `limit` rows,
    /// or as the rows of the range through `transactions_master_proc_ts`,
    /// whichever the range makes cheaper.
    async fn transactions_by_proc_date(
        &mut self,
        start: Alnum<10>,
        end: Alnum<10>,
        after: Option<(CardNumber, TranId)>,
        limit: u16,
    ) -> PortResult<Vec<Transaction>> {
        let limit = limit_i64(limit)?;
        // The INCLUDE COND compares p = substr(tran_proc_ts, 1, 10) with the
        // trimmed bounds. These bounds on the raw column keep exactly its rows,
        // given |start| <= 10 and |end| <= 10 (a trimmed Alnum<10>), the C
        // collation of the column, which in the UTF8 database is code-point
        // order, and its CHECK, which admits at most 26 characters, each in
        // U+0001..U+00FF. A value is never below its own prefix p.
        // - tran_proc_ts >= start holds exactly when p >= start. A prefix below
        //   start either differs from it at a character the value shares, or is
        //   a proper prefix of start, shorter than 10 characters and so the
        //   whole value.
        // - With |end| < 10, tran_proc_ts <= end holds exactly when p <= end. A
        //   prefix of 10 characters that is at most end is neither end itself,
        //   which is shorter, nor an extension of it, which sorts above it, so
        //   it is below end at a character the value shares; a shorter prefix
        //   is the whole value.
        // - With |end| = 10, tran_proc_ts <= end followed by 16 U+00FF holds
        //   exactly when p <= end (proc_ts_upper_bound). A value whose prefix is
        //   end carries at most 16 more characters, none above U+00FF; a prefix
        //   below end keeps the value below it; and a prefix above end differs
        //   from it within its 10 characters, which puts the value above the
        //   bound.
        let (from, to) = (start.trimmed(), proc_ts_upper_bound(&end));
        // The keyset as the space-padded X(16) text of both keys, the form
        // rpad(…, 16) gives the stored keys below; NULL reads from the first row.
        let (card, id) = after.map_or((None, None), |(card, id)| {
            (
                Some(card.as_alnum().as_text().into_owned()),
                Some(id.as_alnum().as_text().into_owned()),
            )
        });
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let conn = &mut *sp;
        let result = async {
            // sqlx runs the select as a cached prepared statement, which
            // PostgreSQL may switch to a generic plan after five executions. A
            // generic plan is made without the bound values, so it can use
            // neither the keyset comparison behind `$3 IS NULL OR` as an index
            // condition nor the selectivity of the range, which decides between
            // the two indexes. force_custom_plan has every page planned with
            // its own bounds. SET LOCAL lasts until the caller's transaction
            // ends: for CR00 that is the read-only report snapshot, which is
            // always rolled back, so the setting never outlives the report on
            // the pooled connection.
            sqlx::query!("SET LOCAL plan_cache_mode = force_custom_plan")
                .execute(&mut *conn)
                .await?;
            sqlx::query_as!(
                TransactionRow,
                r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master
WHERE tran_proc_ts >= $1
  AND tran_proc_ts <= $2
  AND ($3::text IS NULL OR (rpad(tran_card_num, 16), rpad(tran_id, 16)) > ($3, $4))
ORDER BY rpad(tran_card_num, 16), rpad(tran_id, 16)
LIMIT $5"#,
                from.as_ref(),
                to.as_str(),
                card.as_deref(),
                id.as_deref(),
                limit
            )
            .fetch_all(&mut *conn)
            .await
        }
        .await;
        into_transactions(settle(sp, result).await?)
    }
}

/// The write of a new record into the live master.
impl TransactionWriter for PgStore<'_, '_> {
    /// COTRN02C WRITE-TRANSACT-FILE — P-COTRN02C-024, P-COTRN02C-025
    /// [app/cbl/COTRN02C.cbl:L711-749]; COBIL00C WRITE-TRANSACT-FILE —
    /// P-COBIL00C-016, P-COBIL00C-017 [app/cbl/COBIL00C.cbl:L510-547];
    /// CBTRN02C 2900-WRITE-TRANSACTION-FILE — P-CBTRN02C-019
    /// [app/cbl/CBTRN02C.cbl:L562-578]: one live row under its own savepoint,
    /// so a failed write leaves the writes before it in place (E-19). A live
    /// row with the TRAN-ID already is [`PortError::Duplicate`], the
    /// `DUPKEY`/`DUPREC` [app/cbl/COTRN02C.cbl:L735-736]
    /// [app/cbl/COBIL00C.cbl:L533-534]. Returns the identity the reward accrual
    /// of the transaction references.
    async fn write_transaction(&mut self, tran: NewTransaction) -> PortResult<TransactionRowId> {
        let binds = RecordBinds::new(&tran)?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = insert_live(&mut sp, &binds).await;
        settle(sp, result).await.map(TransactionRowId::from)
    }
}

/// The batch steps that empty and reload the live master (E-09). The ports do
/// not declare them: only `batch tranbkp`, `batch tranrept` and
/// `batch combtran` call them.
impl PgStore<'_, '_> {
    /// TRANBKP STEP05R — P-TRANBKP-001 [app/jcl/TRANBKP.jcl:L23-33]
    /// [app/proc/REPROC.prc:L21-28], and the backup step of TRANREPT —
    /// P-TRANREPT-001 [app/jcl/TRANREPT.jcl:L23-33]: `REPRO` of the live
    /// records, in TRAN-ID order, into `TRANSACT.BKUP(+1)`, one page at a
    /// time: the live rows after the TRAN-ID `after`, from the first when
    /// `after` is `None`, up to `limit`.
    ///
    /// The bound is exclusive, so the TRAN-ID of the last row of one page
    /// gives the next page, and a page shorter than `limit` is the end of the
    /// file. The caller holds one page however large the master grows.
    ///
    /// The pages form one consistent read only through the caller's unit of
    /// work: TRANBKP reads them after [`PgStore::exclude_master_writers`],
    /// which keeps every writer of the master waiting until it has emptied
    /// the master and committed, and TRANREPT reads them in the single
    /// snapshot of `Db::begin_snapshot`.
    ///
    /// An empty live master gives an empty first page. The condition code 12
    /// the `REPRO` ends with there (P-TRANBKP-002, E-37) is the batch job's to
    /// report.
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when the read fails or a stored value does
    /// not fit its PIC clause.
    pub async fn live_master_after(
        &mut self,
        after: Option<TranId>,
        limit: u16,
    ) -> PortResult<Vec<Transaction>> {
        let limit = limit_i64(limit)?;
        // Two statements rather than `None` bound as "": an all-space TRAN-ID
        // is stored as "", which `tran_id > ''` would leave out of the first
        // page.
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = match after {
            None => {
                sqlx::query_as!(
                    TransactionRow,
                    r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master
ORDER BY tran_id
LIMIT $1"#,
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
            Some(last) => {
                let key = last.trimmed();
                sqlx::query_as!(
                    TransactionRow,
                    r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master
WHERE tran_id > $1
ORDER BY tran_id
LIMIT $2"#,
                    key.as_ref(),
                    limit
                )
                .fetch_all(&mut *sp)
                .await
            }
        };
        into_transactions(settle(sp, result).await?)
    }

    /// TRANBKP STEP05R to STEP10 — P-TRANBKP-001, P-TRANBKP-003,
    /// P-TRANBKP-005 [app/jcl/TRANBKP.jcl:L23-67]: holds the live master
    /// still for the rest of this unit of work, so the `REPRO` backup and the
    /// `DELETE` and `DEFINE` after it see the same records, and no record
    /// leaves the master without being in its backup. On the mainframe the
    /// batch sequence gives that: `CLOSEFIL` closes the CICS files before it
    /// and `OPENFIL` reopens them after it [README.md:L213-231], so no online
    /// transaction writes `TRANSACT` while the job runs.
    ///
    /// Two locks, both held until the unit of work commits or rolls back:
    ///
    /// 1. The advisory lock [`TRAN_ID_LOCK_KEY`] of the CT02 and CB00 TRAN-ID
    ///    allocation ([`crate::ids::next_tran_id`]), taken with the same
    ///    statement. An add that allocated first is waited for; an add that
    ///    allocates later waits, then reads the master this unit left. CT02
    ///    and CB00 allocate before they insert, the same order as these two
    ///    locks, so the two cannot deadlock.
    /// 2. `LOCK TABLE transactions IN SHARE ROW EXCLUSIVE MODE`. The mode
    ///    conflicts with itself and with the `ROW EXCLUSIVE` lock every
    ///    `INSERT`, `UPDATE` and `DELETE` takes, and not with a plain
    ///    `SELECT`. It waits until the writers in flight (POSTTRAN, COMBTRAN,
    ///    CT02, CB00) end and keeps every later one waiting, while reads of
    ///    the master go on.
    ///
    /// The unit of work runs at `READ COMMITTED`, the `PostgreSQL` default,
    /// where each statement takes its own snapshot. Every later statement
    /// therefore starts after both locks are granted and sees one stable
    /// committed master, the rows of the last writer included. Each lock
    /// statement runs under its own savepoint, as every store statement does;
    /// releasing the savepoint hands its lock to the unit of work, so both
    /// locks stay held until the unit ends.
    ///
    /// # Errors
    ///
    /// The mapped error of a failed lock statement:
    /// [`PortError::Contention`] when the wait ends in a deadlock or a lock
    /// timeout, [`PortError::Unavailable`] on any other failure.
    pub async fn exclude_master_writers(&mut self) -> PortResult<()> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query!(
            r#"SELECT 1 AS "locked!" FROM pg_advisory_xact_lock($1)"#,
            TRAN_ID_LOCK_KEY
        )
        .fetch_one(&mut *sp)
        .await;
        settle(sp, result).await?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query!("LOCK TABLE transactions IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut *sp)
            .await;
        settle(sp, result).await?;
        Ok(())
    }

    /// TRANBKP STEP05 and STEP10 — P-TRANBKP-003, P-TRANBKP-004,
    /// P-TRANBKP-005 [app/jcl/TRANBKP.jcl:L35-67]: the `DELETE` and `DEFINE`
    /// that leave `TRANSACT` empty, as the E-09 representation performs them.
    ///
    /// Under one savepoint, every live row leaves the live master
    /// (`in_master = false`), then every row outside it that no reward accrual
    /// references is deleted. Accrued rows stay as the targets of their
    /// accruals, so the reward history survives the emptied master. Returns
    /// `(demoted, deleted)`: the rows taken out of the live master and the
    /// rows deleted, which include unreferenced rows an earlier run left.
    ///
    /// TRANBKP calls it in the unit of work that took
    /// [`PgStore::exclude_master_writers`] and read the backup through
    /// [`PgStore::live_master_after`], so the rows it takes out of the live
    /// master are exactly the rows backed up.
    ///
    /// # Errors
    ///
    /// [`PortError::Restricted`] when an accrual written concurrently
    /// references a row being deleted; [`PortError::Unavailable`] on any other
    /// failure. Either way the savepoint is rolled back and the live master is
    /// unchanged.
    pub async fn empty_live_master(&mut self) -> PortResult<(u64, u64)> {
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = take_master_offline(&mut sp).await;
        settle(sp, result).await
    }

    /// COMBTRAN STEP10 — P-COMBTRAN-002, P-COMBTRAN-003
    /// [app/jcl/COMBTRAN.jcl:L41-48]: `REPRO INFILE(TRANSACT)
    /// OUTFILE(TRANVSAM)` of one record of `TRANSACT.COMBINED(+1)`, without
    /// `REPLACE`.
    ///
    /// Under one savepoint, in order:
    ///
    /// 1. A live row with the record's TRAN-ID exists:
    ///    [`LoadOutcome::DuplicateLive`], nothing is written and the live row
    ///    keeps its content (`IDC3308I DUPLICATE RECORD`).
    /// 2. A row outside the live master equals the record in TRAN-ID and all
    ///    twelve content columns: the row with the lowest identity is made
    ///    live again, [`LoadOutcome::Revived`], so its accruals stay linked.
    /// 3. Otherwise the record is inserted as a new live row,
    ///    [`LoadOutcome::Inserted`].
    ///
    /// The batch job, not this method, decides from the outcomes whether the
    /// load goes on and with which condition code it ends (P-COMBTRAN-005 to
    /// P-COMBTRAN-008).
    ///
    /// # Errors
    ///
    /// [`PortError::Unavailable`] when a statement fails, or when the merchant
    /// ID does not fit its column; the savepoint is then rolled back and
    /// nothing of the record is written.
    pub async fn load_combined_record(&mut self, rec: NewTransaction) -> PortResult<LoadOutcome> {
        let binds = RecordBinds::new(&rec)?;
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = load(&mut sp, &binds).await;
        match settle(sp, result).await {
            // A live row that a concurrent writer committed after the check of
            // step 1 fails the revive or the insert on the partial unique index.
            // That is the same duplicate, and the savepoint undid the statement.
            Err(PortError::Duplicate) => Ok(LoadOutcome::DuplicateLive),
            outcome => outcome,
        }
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::numeric::Alnum;

    use super::{PROC_TS_LEN, proc_ts_upper_bound};

    /// The highest 26-character timestamp of the 10-character date `date`: the
    /// date, then 16 U+00FF.
    fn highest_of(date: &str) -> String {
        [date, &"\u{ff}".repeat(16)].concat()
    }

    /// An end of 10 characters, a Latin-1 byte and a control byte counting as
    /// one character each, takes the 16 highest characters; a shorter end,
    /// blank included, is its own bound, and trailing spaces never count.
    #[test]
    fn the_upper_bound_extends_only_an_end_of_ten_characters() {
        let cases = [
            (Alnum::move_from("2022-07-06"), highest_of("2022-07-06")),
            (
                Alnum::move_from_bytes(b"2022-07-0\xe9"),
                highest_of("2022-07-0\u{e9}"),
            ),
            (
                Alnum::move_from_bytes(b"2022-07-0\x01"),
                highest_of("2022-07-0\u{1}"),
            ),
            (Alnum::move_from("2022 07 06"), highest_of("2022 07 06")),
            (Alnum::move_from("2022-07-0"), "2022-07-0".to_owned()),
            (Alnum::move_from("2022-07"), "2022-07".to_owned()),
            (Alnum::move_from("2022-07   "), "2022-07".to_owned()),
            (Alnum::move_from(""), String::new()),
        ];
        for (end, expected) in cases {
            let bound = proc_ts_upper_bound(&end);
            assert_eq!(bound, expected, "end {:?}", end.as_text());
            assert!(
                bound.chars().count() <= PROC_TS_LEN,
                "end {:?}: the bound fits X(26)",
                end.as_text()
            );
        }
    }
}
