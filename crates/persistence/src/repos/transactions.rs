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
//! # Writes
//!
//! Each write runs under its own savepoint through `crate::tx::savepoint` and
//! `crate::tx::settle`; reads run on the transaction directly. A unique
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
use carddemo_domain::numeric::{AccountId, Alnum, TranId, TransactionRowId};
use carddemo_domain::ports::{
    Browse, Direction, PortError, PortResult, TransactionReader, TransactionWriter,
};
use rust_decimal::Decimal;
use sqlx::PgConnection;

use super::{
    alnum, card_number, limit_i64, merchant_id, money, tran_category_code, tran_id, tran_type_code,
};
use crate::ids::TRAN_ID_LOCK_KEY;
use crate::tx::{PgStore, savepoint, settle};
use crate::{PersistenceError, port_error};

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

/// `STARTBR` at `start` with `READNEXT`: live rows with a TRAN-ID of at least
/// `start`, ascending, up to `limit`; with `account`, only rows whose card the
/// cross-reference maps to that account (E-17).
async fn select_forward(
    conn: &mut PgConnection,
    start: &str,
    account: Option<i64>,
    limit: i64,
) -> Result<Vec<TransactionRow>, sqlx::Error> {
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
  AND ($2::bigint IS NULL OR EXISTS (
      SELECT 1 FROM card_xref x
      WHERE x.xref_card_num = m.tran_card_num AND x.xref_acct_id = $2))
ORDER BY m.tran_id
LIMIT $3"#,
        start,
        account,
        limit
    )
    .fetch_all(conn)
    .await
}

/// `STARTBR` at `start` with `READPREV`: live rows with a TRAN-ID of at most
/// `start`, descending, up to `limit`, filtered by `account` as
/// [`select_forward`] filters.
async fn select_backward_from(
    conn: &mut PgConnection,
    start: &str,
    account: Option<i64>,
    limit: i64,
) -> Result<Vec<TransactionRow>, sqlx::Error> {
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
  AND ($2::bigint IS NULL OR EXISTS (
      SELECT 1 FROM card_xref x
      WHERE x.xref_card_num = m.tran_card_num AND x.xref_acct_id = $2))
ORDER BY m.tran_id DESC
LIMIT $3"#,
        start,
        account,
        limit
    )
    .fetch_all(conn)
    .await
}

/// `STARTBR` at `HIGH-VALUES` with `READPREV`: live rows from the highest
/// TRAN-ID down, up to `limit`, filtered by `account` as [`select_forward`]
/// filters.
async fn select_backward_from_end(
    conn: &mut PgConnection,
    account: Option<i64>,
    limit: i64,
) -> Result<Vec<TransactionRow>, sqlx::Error> {
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
WHERE $1::bigint IS NULL OR EXISTS (
      SELECT 1 FROM card_xref x
      WHERE x.xref_card_num = m.tran_card_num AND x.xref_acct_id = $1)
ORDER BY m.tran_id DESC
LIMIT $2"#,
        account,
        limit
    )
    .fetch_all(conn)
    .await
}

/// Inserts `record` as a live row and returns its new identity.
///
/// A live row with the same TRAN-ID fails the partial unique index
/// `transactions_master_tran_id` with SQLSTATE `23505`, which
/// [`crate::tx::settle`] reports as [`PortError::Duplicate`].
async fn insert_live(conn: &mut PgConnection, record: &RecordBinds) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar!(
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
}

/// Whether a live row holds the TRAN-ID `id`.
async fn is_live(conn: &mut PgConnection, id: &str) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar!(
        r#"SELECT EXISTS (SELECT 1 FROM transaction_master WHERE tran_id = $1) AS "live!""#,
        id
    )
    .fetch_one(conn)
    .await
}

/// Makes live again the row outside the live master that equals `record` in
/// TRAN-ID and in all twelve content columns, the lowest identity when several
/// do, and returns that identity; `None` when no such row exists.
///
/// Text compares right-trimmed, as stored, and `tran_amt` as `NUMERIC`, so
/// `1.5` equals `1.50`. The outer `NOT in_master` makes the update skip a row
/// a concurrent load revived first.
async fn revive(conn: &mut PgConnection, record: &RecordBinds) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar!(
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
    /// COBOL reads `UPDATE` and rewrites nothing, so a plain read gives the same
    /// result. No row is the `NOTFND` the rule turns into "Transaction ID NOT
    /// found...".
    async fn read_transaction(&mut self, id: TranId) -> PortResult<Transaction> {
        let key = id.trimmed();
        let row = sqlx::query_as!(
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
        .fetch_one(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?;
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
    /// (E-17): a row is kept when `card_xref` maps its card to the account, and
    /// the rows it drops do not count against the limit. An empty file or a
    /// start beyond the last key gives an empty page, the `STARTBR` `NOTFND`.
    async fn browse_transactions(
        &mut self,
        browse: Browse<TranId>,
        account_id: Option<AccountId>,
    ) -> PortResult<Vec<Transaction>> {
        let limit = limit_i64(browse.limit)?;
        let account = account_id.map(AccountId::to_i64);
        let start = browse.start_key.as_ref().map(TranId::trimmed);
        let conn = &mut **self.tx;
        let rows = match (browse.direction, start.as_deref()) {
            (Direction::Forward, start) => {
                select_forward(conn, start.unwrap_or(LOW_VALUES), account, limit).await
            }
            (Direction::Backward, Some(start)) => {
                select_backward_from(conn, start, account, limit).await
            }
            (Direction::Backward, None) => select_backward_from_end(conn, account, limit).await,
        }
        .map_err(|err| port_error(&err))?;
        into_transactions(rows)
    }

    /// COTRN02C ADD-TRANSACTION and COPY-LAST-TRAN-DATA — P-COTRN02C-020,
    /// P-COTRN02C-026 [app/cbl/COTRN02C.cbl:L442-451]
    /// [app/cbl/COTRN02C.cbl:L471-495]; COBIL00C PROCESS-ENTER-KEY —
    /// P-COBIL00C-012 [app/cbl/COBIL00C.cbl:L210-219]: `STARTBR` at
    /// `HIGH-VALUES` and one `READPREV`, the highest-keyed live row whatever its
    /// account. `None` on an empty live master, the `STARTBR` `NOTFND` of
    /// P-COTRN02C-021 and P-COBIL00C-013.
    async fn highest_transaction(&mut self) -> PortResult<Option<Transaction>> {
        let rows = select_backward_from_end(self.tx, None, 1)
            .await
            .map_err(|err| port_error(&err))?;
        Ok(into_transactions(rows)?.pop())
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
        let rows = sqlx::query_as!(
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
        .fetch_all(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?;
        into_transactions(rows)
    }

    /// TRANREPT STEP05R — P-TRANREPT-002 [app/proc/TRANREPT.prc:L35-53]: the
    /// `INCLUDE COND=(TRAN-PROC-DT,GE,start,AND,TRAN-PROC-DT,LE,end)` over
    /// `TRAN-PROC-DT`, bytes 305 to 314, the first 10 characters of
    /// `tran_proc_ts`, compared as `CH` text with both bounds inclusive. The
    /// selected rows are read by CBTRN03C 1000-TRANFILE-GET-NEXT —
    /// P-CBTRN03C-474 [app/cbl/CBTRN03C.cbl:L248-272], and the same selection
    /// serves CR00.
    async fn transactions_by_proc_date(
        &mut self,
        start: Alnum<10>,
        end: Alnum<10>,
    ) -> PortResult<Vec<Transaction>> {
        let (from, to) = (start.trimmed(), end.trimmed());
        // TRAN-ID order, the order of the TRANSACT.BKUP(+1) backup the sort reads
        // (P-TRANREPT-001), as the port declares. The STEP05R sort by card number,
        // and the order it leaves within one card (E-38), belong to the report
        // rule, so this read does not sort by card number.
        let rows = sqlx::query_as!(
            TransactionRow,
            r#"SELECT transaction_id AS "transaction_id!", tran_id AS "tran_id!",
       tran_type_cd AS "tran_type_cd!", tran_cat_cd AS "tran_cat_cd!",
       tran_source AS "tran_source!", tran_desc AS "tran_desc!", tran_amt AS "tran_amt!",
       tran_merchant_id AS "tran_merchant_id!", tran_merchant_name AS "tran_merchant_name!",
       tran_merchant_city AS "tran_merchant_city!", tran_merchant_zip AS "tran_merchant_zip!",
       tran_card_num AS "tran_card_num!", tran_orig_ts AS "tran_orig_ts!",
       tran_proc_ts AS "tran_proc_ts!"
FROM transaction_master
WHERE substr(tran_proc_ts, 1, 10) >= $1
  AND substr(tran_proc_ts, 1, 10) <= $2
ORDER BY tran_id"#,
            from.as_ref(),
            to.as_ref()
        )
        .fetch_all(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?;
        into_transactions(rows)
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
        let mut sp = savepoint(self.tx).await?;
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
        let rows = match after {
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
                .fetch_all(&mut **self.tx)
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
                .fetch_all(&mut **self.tx)
                .await
            }
        }
        .map_err(|err| port_error(&err))?;
        into_transactions(rows)
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
    /// committed master, the rows of the last writer included. The locks
    /// write nothing, so they run on the transaction itself, with no
    /// savepoint.
    ///
    /// # Errors
    ///
    /// The [`port_error`] mapping of a failed lock statement:
    /// [`PortError::Contention`] when the wait ends in a deadlock or a lock
    /// timeout, [`PortError::Unavailable`] on any other failure.
    pub async fn exclude_master_writers(&mut self) -> PortResult<()> {
        sqlx::query_scalar!(
            r#"SELECT 1 AS "locked!" FROM pg_advisory_xact_lock($1)"#,
            TRAN_ID_LOCK_KEY
        )
        .fetch_one(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?;
        sqlx::query!("LOCK TABLE transactions IN SHARE ROW EXCLUSIVE MODE")
            .execute(&mut **self.tx)
            .await
            .map_err(|err| port_error(&err))?;
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
        let mut sp = savepoint(self.tx).await?;
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
        let mut sp = savepoint(self.tx).await?;
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
