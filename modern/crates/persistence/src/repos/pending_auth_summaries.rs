//! [`PendingAuthSummaryRepository`]: the statements of `pending_auth_summaries`, the
//! `CIPAUSMY` summary that is the IMS root segment `PAUTSUM0` of `DBPAUTP0`, one row per account
//! (migration 0004; `DBPAUTP0.dbd:28-30`). The primary key `acct_id` is the root key `ACCNTID`,
//! `PA-ACCT-ID PIC S9(11) COMP-3`, and the account's `pending_auth_details` rows are its children
//! through `pending_auth_details_summary_fk … ON DELETE CASCADE`.
//!
//! | Method | COBOL access it replaces | Rules |
//! |---|---|---|
//! | `find_by_id` | `GU PAUTSUM0 WHERE (ACCNTID = PA-ACCT-ID)`: COPAUS0C `GET-AUTH-SUMMARY` (`COPAUS0C.cbl:966-998`) and COPAUS1C `READ-AUTH-RECORD` (`COPAUS1C.cbl:436-443`); `None` is IMS `GE` | COPAUS0C-027, COPAUS0C-029, COPAUS0C-030, COPAUS1C-008 |
//! | `lock_or_create` | COPAUA0C `5500-READ-AUTH-SUMMRY` (`COPAUA0C.cbl:616-643`), with the `INITIALIZE … REPLACING NUMERIC DATA BY ZERO` summary of `8400-UPDATE-SUMMARY` for an account that has none (`COPAUA0C.cbl:801-806`) | COPAUA0C-025, COPAUA0C-026, COPAUA0C-092, COPAUA0C-125 |
//! | `update` | `8400-UPDATE-SUMMARY` `REPL` of a found summary or `ISRT` of a new one (`COPAUA0C.cbl:824-847`) | COPAUA0C-097, COPAUA0C-119 |
//! | `lock_next` | CBPAUP0C `2000-FIND-NEXT-AUTH-SUMMARY`, the unqualified `GN` that reads the next root in root-key order when the purge processes it (`CBPAUP0C.cbl:216-245`); `None` is IMS `GB` | CBPAUP0C-009, CBPAUP0C-023 |
//! | `find_all` | none: every summary at once in root-key order, for callers that inspect the whole table, such as tests; the purge reads with `lock_next` | — |
//! | `delete` | CBPAUP0C `6000-DELETE-AUTH-SUMMARY` `DLET`, which removes the dependent details with the root (`CBPAUP0C.cbl:328-350`) | CBPAUP0C-016, CBPAUP0C-026 |
//! | `insert` | none: a whole summary written at once, for callers that seed one | — |
//!
//! CP00 never calls `insert`. Its `ISRT` is the `lock_or_create` insert followed by `update`
//! in the same transaction (COPAUA0C-097). CBPAUP0C issues no `REPL`, so the purge has no
//! update here and a summary it keeps holds every stored count and amount: `approved_auth_amt`
//! still counts purged approvals (CBPAUP0C-017, COPAUA0C-049, E30).
//!
//! # The credit-limit lock (i10, AAP 0.3.4.2)
//!
//! `lock_or_create` is the one serialisation of CP00's authorization decisions. In one
//! transaction it inserts the caller's template with `ON CONFLICT (acct_id) DO NOTHING`, then
//! reads the row with `FOR UPDATE`. Whether the insert returned a row is whether this call
//! created the summary: the COBOL's `NFOUND-PAUT-SMRY-SEG` path against `FOUND-PAUT-SMRY-SEG`,
//! which the worker maps to the domain's created and existing summary states (COPAUA0C-025).
//!
//! - On a [`PgTx`] the row lock holds until the caller commits or rolls back. Every other
//!   decision for the account, in this `carddemo-mq` process or another, and for a redelivered
//!   message alike, waits for it and then decides against committed state. A concurrent first
//!   authorization waits on the uncommitted insert and then finds the committed row
//!   (COPAUA0C-026). The CP00 worker always calls it on its authorization transaction, which
//!   also covers the decision, the reply and the writes after the acknowledgement (E29).
//! - On a [`PgStore`] the call runs in a short transaction of its own, committed before it
//!   returns, so its lock ends with the call and serialises nothing after it.
//! - No process-local lock guards authorization state: a second process could not see one.
//! - A row that a concurrent purge deletes after the insert saw it, and before the lock is
//!   granted, is not returned by the locking read. The insert and the read then run once more,
//!   and a second miss is [`PersistenceError::NotFound`].
//!
//! # The purge's read (CBPAUP0C-009)
//!
//! `lock_next` reads, with `FOR UPDATE`, the summary after the last one the purge processed, as
//! the purge's `GN` reads each root when it reaches it and IMS program isolation holds that root
//! against other updaters until the purge's next sync point. On the purge's [`PgTx`] the lock
//! holds until the checkpoint commit, so a CP00 decision for an account the purge has read waits
//! for that commit, and the purge's delete test sees the count CP00 last committed, never a copy
//! taken earlier in the run.
//!
//! # Statements and redaction
//!
//! Each statement is written once, in a private function over a `PostgreSQL` connection: the
//! transaction's own for [`PgTx`], a pooled one for [`PgStore`]. Values pass through codec B,
//! [`PendingAuthSummaryRow`], unchanged. Errors are [`PersistenceError`] values, which carry no
//! key and no stored value, and nothing here logs (Cardholder Data Redaction).

use copybook::PicS;
use domain::records::PendingAuthSummaryRecord;
use sqlx::PgConnection;

use crate::error::PersistenceError;
use crate::rows::{PendingAuthSummaryRow, pic_s_to_i64};
use crate::{PgStore, PgTx, ProbePoint, probed};

/// How often `lock_or_create` runs its insert and locking read before it gives up: once, and
/// once more when a concurrent delete removed the row in between.
const LOCK_ATTEMPTS: usize = 2;

/// The statements of `pending_auth_summaries`, one row per account keyed by `acct_id`.
///
/// Implemented by [`PgStore`], on which each call commits on its own, and by [`PgTx`], on which
/// each call runs inside the caller's transaction. Test doubles in other crates implement it too.
pub trait PendingAuthSummaryRepository: Send {
    /// The summary of account `acct_id`, or `None` when the account has none: COBOL
    /// `GU PAUTSUM0 WHERE (ACCNTID = PA-ACCT-ID)`, `None` being IMS `GE` (COPAUS0C-027,
    /// COPAUS0C-029, COPAUS1C-008).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// [`PersistenceError::Database`] for any other failure of the read, the IMS status other
    /// than found and not found (COPAUS0C-030); [`PersistenceError::Decode`] when the stored row
    /// does not fit its `CIPAUSMY` pictures.
    fn find_by_id(
        &mut self,
        acct_id: PicS<11>,
    ) -> impl Future<Output = Result<Option<PendingAuthSummaryRecord>, PersistenceError>> + Send;

    /// Locks the summary of `template.acct_id`, creating it from `template` when the account has
    /// none, and returns the locked row with `true` when this call created it.
    ///
    /// `INSERT … ON CONFLICT (acct_id) DO NOTHING RETURNING acct_id` writes every field of
    /// `template`; the caller builds it with zero limits, balances, amounts and counts and the
    /// cross-reference's account and customer IDs, as `INITIALIZE PENDING-AUTH-SUMMARY REPLACING
    /// NUMERIC DATA BY ZERO` does (`COPAUA0C.cbl:801-806`; COPAUA0C-092). `SELECT … FOR UPDATE`
    /// then locks and reads the row, so the record returned is the stored one, whoever created
    /// it, and never `template` alone (COPAUA0C-025). On a [`PgTx`] the lock holds until that
    /// transaction ends and serialises every decision for the account (COPAUA0C-026); on a
    /// [`PgStore`] it ends when the call returns.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when `template` holds a byte no `CHAR(n)` column stores or
    /// the stored row does not fit its pictures; [`PersistenceError::NotFound`] when concurrent
    /// deletes removed the row after both inserts; [`PersistenceError::Unavailable`] and
    /// [`PersistenceError::Database`] for a failed statement, lock wait or commit, such as a
    /// deadlock (`40P01`). Each is CP00's `I002` case (COPAUA0C-125). A failed statement aborts
    /// a [`PgTx`].
    fn lock_or_create(
        &mut self,
        template: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(PendingAuthSummaryRecord, bool), PersistenceError>> + Send;

    /// The summary with the lowest `acct_id` above `after`, or the lowest of all when `after` is
    /// `None`, read and locked with `SELECT … ORDER BY acct_id LIMIT 1 FOR UPDATE`: CBPAUP0C's
    /// unqualified `GN`, which returns the next root in root-key order when the purge reaches it
    /// (CBPAUP0C-009). `None` is the end of the database, IMS `GB`.
    ///
    /// The primary-key index serves both forms. Under `READ COMMITTED`, a row another
    /// transaction has locked, such as CP00's [`Self::lock_or_create`], is waited for and read
    /// again once that transaction ends, so the record returned holds what it committed; a row it
    /// deleted is skipped and the next one returned. On a [`PgTx`] the lock holds until that
    /// transaction commits or rolls back, as IMS program isolation holds a root read by `GN`
    /// until the sync point; on a [`PgStore`] the read runs on a pooled connection in autocommit
    /// mode, so the lock ends with the call.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] and [`PersistenceError::Database`] for a failed read or
    /// lock wait, such as a deadlock (`40P01`), CBPAUP0C's `AUTH SUMMARY READ FAILED` case
    /// (CBPAUP0C-023); [`PersistenceError::Decode`] when the stored row does not fit its
    /// pictures. A failed statement aborts a [`PgTx`].
    fn lock_next(
        &mut self,
        after: Option<PicS<11>>,
    ) -> impl Future<Output = Result<Option<PendingAuthSummaryRecord>, PersistenceError>> + Send;

    /// Every summary in ascending `acct_id`, the root-key order of CBPAUP0C's unqualified `GN`,
    /// read at once and without a lock. The read is unbounded and serves callers that inspect the
    /// whole table, such as tests; the purge reads one summary at a time with
    /// [`Self::lock_next`] instead (CBPAUP0C-009).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] and [`PersistenceError::Database`] for a failed read;
    /// [`PersistenceError::Decode`] when a stored row does not fit its pictures.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<PendingAuthSummaryRecord>, PersistenceError>> + Send;

    /// Inserts `record` as a new summary, every field as given.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when the account already has a summary (constraint
    /// `pending_auth_summaries_pkey`); [`PersistenceError::Decode`] when `record` holds a byte
    /// no `CHAR(n)` column stores; [`PersistenceError::Unavailable`] and [`PersistenceError::Database`] for any
    /// other failure. On a [`PgTx`] a failed insert aborts the transaction unless it runs inside
    /// a [`PgTx::savepoint`].
    fn insert(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Rewrites every field of the summary of `record.acct_id` other than the key, and returns
    /// whether exactly one row changed; `false` means the account has no summary. This is CP00's
    /// summary write after the acknowledgement, `REPL` or `ISRT` of `8400-UPDATE-SUMMARY`; the
    /// order of that write is the worker's (COPAUA0C-097, E29).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when `record` holds a byte no `CHAR(n)` column stores;
    /// [`PersistenceError::Unavailable`] and [`PersistenceError::Database`] for any other
    /// failure. CP00 logs each as `I003` (COPAUA0C-119).
    fn update(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Deletes the summary of account `acct_id` and, through `ON DELETE CASCADE`, every detail
    /// under it, as IMS deletes a root's dependents with it; returns whether exactly one summary
    /// was deleted (CBPAUP0C-016).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] and [`PersistenceError::Database`] for a failed delete,
    /// CBPAUP0C's `AUTH SUMMARY DELETE FAILED` case (CBPAUP0C-026).
    fn delete(
        &mut self,
        acct_id: PicS<11>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;
}

impl PendingAuthSummaryRepository for PgStore {
    fn find_by_id(
        &mut self,
        acct_id: PicS<11>,
    ) -> impl Future<Output = Result<Option<PendingAuthSummaryRecord>, PersistenceError>> + Send
    {
        let verdict = self.verdict(ProbePoint::AuthSummaryRead, &[&acct_id]);
        let pool = self.pool().clone();
        probed(verdict, async move {
            let mut conn = pool.acquire().await?;
            find_by_id_in(&mut conn, acct_id).await
        })
    }

    /// Runs the insert and the locking read in a transaction of its own and commits it, so the
    /// row lock is released before the call returns.
    fn lock_or_create(
        &mut self,
        template: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(PendingAuthSummaryRecord, bool), PersistenceError>> + Send
    {
        let begin = self.begin();
        async move {
            // Dropped uncommitted on an error, which rolls the transaction back.
            let mut tx = begin.await?;
            let locked = lock_or_create_in(tx.conn(), template).await?;
            tx.commit().await?;
            Ok(locked)
        }
    }

    /// Runs the locking read on a pooled connection in autocommit mode: it still waits for a
    /// row another transaction holds, but its own lock is released before the call returns.
    fn lock_next(
        &mut self,
        after: Option<PicS<11>>,
    ) -> impl Future<Output = Result<Option<PendingAuthSummaryRecord>, PersistenceError>> + Send
    {
        let pool = self.pool().clone();
        async move {
            let mut conn = pool.acquire().await?;
            lock_next_in(&mut conn, after).await
        }
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<PendingAuthSummaryRecord>, PersistenceError>> + Send {
        let pool = self.pool().clone();
        async move {
            let mut conn = pool.acquire().await?;
            find_all_in(&mut conn).await
        }
    }

    fn insert(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let pool = self.pool().clone();
        async move {
            let mut conn = pool.acquire().await?;
            insert_in(&mut conn, record).await
        }
    }

    fn update(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let pool = self.pool().clone();
        async move {
            let mut conn = pool.acquire().await?;
            update_in(&mut conn, record).await
        }
    }

    fn delete(
        &mut self,
        acct_id: PicS<11>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let pool = self.pool().clone();
        async move {
            let mut conn = pool.acquire().await?;
            delete_in(&mut conn, acct_id).await
        }
    }
}

impl PendingAuthSummaryRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        acct_id: PicS<11>,
    ) -> impl Future<Output = Result<Option<PendingAuthSummaryRecord>, PersistenceError>> + Send
    {
        let verdict = self.verdict(ProbePoint::AuthSummaryRead, &[&acct_id]);
        probed(verdict, find_by_id_in(self.conn(), acct_id))
    }

    /// Runs the insert and the locking read inside this transaction: the row lock holds until
    /// [`PgTx::commit`] or [`PgTx::rollback`], which is the i10 serialisation CP00 relies on.
    fn lock_or_create(
        &mut self,
        template: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(PendingAuthSummaryRecord, bool), PersistenceError>> + Send
    {
        lock_or_create_in(self.conn(), template)
    }

    /// Runs the locking read inside this transaction: the row lock holds until
    /// [`PgTx::commit`] or [`PgTx::rollback`], also when the read ran in a
    /// [`PgTx::savepoint`] that was then released.
    fn lock_next(
        &mut self,
        after: Option<PicS<11>>,
    ) -> impl Future<Output = Result<Option<PendingAuthSummaryRecord>, PersistenceError>> + Send
    {
        lock_next_in(self.conn(), after)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<PendingAuthSummaryRecord>, PersistenceError>> + Send {
        find_all_in(self.conn())
    }

    fn insert(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_in(self.conn(), record)
    }

    fn update(
        &mut self,
        record: &PendingAuthSummaryRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_in(self.conn(), record)
    }

    fn delete(
        &mut self,
        acct_id: PicS<11>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        delete_in(self.conn(), acct_id)
    }
}

/// `find_by_id` on `conn`: the row of `acct_id`, decoded through codec B.
async fn find_by_id_in(
    conn: &mut PgConnection,
    acct_id: PicS<11>,
) -> Result<Option<PendingAuthSummaryRecord>, PersistenceError> {
    select_by_id(conn, pic_s_to_i64(acct_id))
        .await?
        .map(PendingAuthSummaryRow::into_record)
        .transpose()
}

/// `lock_or_create` on `conn`: insert when absent, then lock, with one more attempt when a
/// concurrent delete removed the row between the two statements.
async fn lock_or_create_in(
    conn: &mut PgConnection,
    template: &PendingAuthSummaryRecord,
) -> Result<(PendingAuthSummaryRecord, bool), PersistenceError> {
    let row = PendingAuthSummaryRow::from_record(template)?;
    for _ in 0..LOCK_ATTEMPTS {
        let created = insert_if_absent(&mut *conn, &row).await?;
        // READ COMMITTED: once the lock is granted the row is re-read, and a row deleted by the
        // transaction that held the lock is skipped, so `None` here means it is gone.
        if let Some(locked) = select_for_update(&mut *conn, row.acct_id).await? {
            return Ok((locked.into_record()?, created));
        }
    }
    Err(PersistenceError::NotFound)
}

/// `lock_next` on `conn`: the first row above `after`, or the first of all, locked and decoded
/// through codec B.
async fn lock_next_in(
    conn: &mut PgConnection,
    after: Option<PicS<11>>,
) -> Result<Option<PendingAuthSummaryRecord>, PersistenceError> {
    let row = match after {
        None => select_first_for_update(conn).await?,
        Some(key) => select_next_for_update(conn, pic_s_to_i64(key)).await?,
    };
    row.map(PendingAuthSummaryRow::into_record).transpose()
}

/// `find_all` on `conn`: every row in key order, decoded through codec B.
async fn find_all_in(
    conn: &mut PgConnection,
) -> Result<Vec<PendingAuthSummaryRecord>, PersistenceError> {
    select_all(conn)
        .await?
        .into_iter()
        .map(PendingAuthSummaryRow::into_record)
        .collect()
}

/// `insert` on `conn`.
async fn insert_in(
    conn: &mut PgConnection,
    record: &PendingAuthSummaryRecord,
) -> Result<(), PersistenceError> {
    insert_row(conn, &PendingAuthSummaryRow::from_record(record)?).await
}

/// `update` on `conn`.
async fn update_in(
    conn: &mut PgConnection,
    record: &PendingAuthSummaryRecord,
) -> Result<bool, PersistenceError> {
    update_row(conn, &PendingAuthSummaryRow::from_record(record)?).await
}

/// `delete` on `conn`.
async fn delete_in(conn: &mut PgConnection, acct_id: PicS<11>) -> Result<bool, PersistenceError> {
    delete_row(conn, pic_s_to_i64(acct_id)).await
}

/// `SELECT` of the row whose key is `acct_id`.
async fn select_by_id(
    conn: &mut PgConnection,
    acct_id: i64,
) -> Result<Option<PendingAuthSummaryRow>, PersistenceError> {
    let row = sqlx::query_as!(
        PendingAuthSummaryRow,
        "SELECT acct_id, cust_id, auth_status, account_status_1, account_status_2, \
         account_status_3, account_status_4, account_status_5, credit_limit, cash_limit, \
         credit_balance, cash_balance, approved_auth_cnt, declined_auth_cnt, \
         approved_auth_amt, declined_auth_amt, filler \
         FROM pending_auth_summaries WHERE acct_id = $1",
        acct_id
    )
    .fetch_optional(conn)
    .await?;
    Ok(row)
}

/// `SELECT … FOR UPDATE` of the row whose key is `acct_id`: waits for any transaction that
/// holds its lock, then locks it until this transaction ends.
async fn select_for_update(
    conn: &mut PgConnection,
    acct_id: i64,
) -> Result<Option<PendingAuthSummaryRow>, PersistenceError> {
    let row = sqlx::query_as!(
        PendingAuthSummaryRow,
        "SELECT acct_id, cust_id, auth_status, account_status_1, account_status_2, \
         account_status_3, account_status_4, account_status_5, credit_limit, cash_limit, \
         credit_balance, cash_balance, approved_auth_cnt, declined_auth_cnt, \
         approved_auth_amt, declined_auth_amt, filler \
         FROM pending_auth_summaries WHERE acct_id = $1 FOR UPDATE",
        acct_id
    )
    .fetch_optional(conn)
    .await?;
    Ok(row)
}

/// `SELECT … FOR UPDATE` of the row with the lowest key: waits for any transaction that holds
/// its lock, skips it when that transaction deleted it, then locks the row returned until this
/// transaction ends.
async fn select_first_for_update(
    conn: &mut PgConnection,
) -> Result<Option<PendingAuthSummaryRow>, PersistenceError> {
    let row = sqlx::query_as!(
        PendingAuthSummaryRow,
        "SELECT acct_id, cust_id, auth_status, account_status_1, account_status_2, \
         account_status_3, account_status_4, account_status_5, credit_limit, cash_limit, \
         credit_balance, cash_balance, approved_auth_cnt, declined_auth_cnt, \
         approved_auth_amt, declined_auth_amt, filler \
         FROM pending_auth_summaries ORDER BY acct_id LIMIT 1 FOR UPDATE"
    )
    .fetch_optional(conn)
    .await?;
    Ok(row)
}

/// `SELECT … FOR UPDATE` of the row with the lowest key above `after`, waiting and skipping as
/// [`select_first_for_update`] does.
async fn select_next_for_update(
    conn: &mut PgConnection,
    after: i64,
) -> Result<Option<PendingAuthSummaryRow>, PersistenceError> {
    let row = sqlx::query_as!(
        PendingAuthSummaryRow,
        "SELECT acct_id, cust_id, auth_status, account_status_1, account_status_2, \
         account_status_3, account_status_4, account_status_5, credit_limit, cash_limit, \
         credit_balance, cash_balance, approved_auth_cnt, declined_auth_cnt, \
         approved_auth_amt, declined_auth_amt, filler \
         FROM pending_auth_summaries WHERE acct_id > $1 ORDER BY acct_id LIMIT 1 FOR UPDATE",
        after
    )
    .fetch_optional(conn)
    .await?;
    Ok(row)
}

/// `SELECT` of every row in ascending key order.
async fn select_all(
    conn: &mut PgConnection,
) -> Result<Vec<PendingAuthSummaryRow>, PersistenceError> {
    let rows = sqlx::query_as!(
        PendingAuthSummaryRow,
        "SELECT acct_id, cust_id, auth_status, account_status_1, account_status_2, \
         account_status_3, account_status_4, account_status_5, credit_limit, cash_limit, \
         credit_balance, cash_balance, approved_auth_cnt, declined_auth_cnt, \
         approved_auth_amt, declined_auth_amt, filler \
         FROM pending_auth_summaries ORDER BY acct_id"
    )
    .fetch_all(conn)
    .await?;
    Ok(rows)
}

/// `INSERT … ON CONFLICT (acct_id) DO NOTHING` of `row`; whether it inserted, that is whether
/// the account had no summary. A conflicting insert that another transaction has not committed
/// yet makes this statement wait for that transaction's end.
async fn insert_if_absent(
    conn: &mut PgConnection,
    row: &PendingAuthSummaryRow,
) -> Result<bool, PersistenceError> {
    let inserted = sqlx::query_scalar!(
        "INSERT INTO pending_auth_summaries (acct_id, cust_id, auth_status, account_status_1, \
         account_status_2, account_status_3, account_status_4, account_status_5, credit_limit, \
         cash_limit, credit_balance, cash_balance, approved_auth_cnt, declined_auth_cnt, \
         approved_auth_amt, declined_auth_amt, filler) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17) \
         ON CONFLICT (acct_id) DO NOTHING \
         RETURNING acct_id",
        row.acct_id,
        row.cust_id,
        row.auth_status,
        row.account_status_1,
        row.account_status_2,
        row.account_status_3,
        row.account_status_4,
        row.account_status_5,
        row.credit_limit,
        row.cash_limit,
        row.credit_balance,
        row.cash_balance,
        row.approved_auth_cnt,
        row.declined_auth_cnt,
        row.approved_auth_amt,
        row.declined_auth_amt,
        row.filler
    )
    .fetch_optional(conn)
    .await?;
    Ok(inserted.is_some())
}

/// `INSERT` of `row`; a key already present is [`PersistenceError::Duplicate`].
async fn insert_row(
    conn: &mut PgConnection,
    row: &PendingAuthSummaryRow,
) -> Result<(), PersistenceError> {
    sqlx::query!(
        "INSERT INTO pending_auth_summaries (acct_id, cust_id, auth_status, account_status_1, \
         account_status_2, account_status_3, account_status_4, account_status_5, credit_limit, \
         cash_limit, credit_balance, cash_balance, approved_auth_cnt, declined_auth_cnt, \
         approved_auth_amt, declined_auth_amt, filler) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
        row.acct_id,
        row.cust_id,
        row.auth_status,
        row.account_status_1,
        row.account_status_2,
        row.account_status_3,
        row.account_status_4,
        row.account_status_5,
        row.credit_limit,
        row.cash_limit,
        row.credit_balance,
        row.cash_balance,
        row.approved_auth_cnt,
        row.declined_auth_cnt,
        row.approved_auth_amt,
        row.declined_auth_amt,
        row.filler
    )
    .execute(conn)
    .await?;
    Ok(())
}

/// `UPDATE` of every non-key column of the row whose key is `row.acct_id`; whether exactly one
/// row changed.
async fn update_row(
    conn: &mut PgConnection,
    row: &PendingAuthSummaryRow,
) -> Result<bool, PersistenceError> {
    let result = sqlx::query!(
        "UPDATE pending_auth_summaries SET cust_id = $2, auth_status = $3, \
         account_status_1 = $4, account_status_2 = $5, account_status_3 = $6, \
         account_status_4 = $7, account_status_5 = $8, credit_limit = $9, cash_limit = $10, \
         credit_balance = $11, cash_balance = $12, approved_auth_cnt = $13, \
         declined_auth_cnt = $14, approved_auth_amt = $15, declined_auth_amt = $16, \
         filler = $17 \
         WHERE acct_id = $1",
        row.acct_id,
        row.cust_id,
        row.auth_status,
        row.account_status_1,
        row.account_status_2,
        row.account_status_3,
        row.account_status_4,
        row.account_status_5,
        row.credit_limit,
        row.cash_limit,
        row.credit_balance,
        row.cash_balance,
        row.approved_auth_cnt,
        row.declined_auth_cnt,
        row.approved_auth_amt,
        row.declined_auth_amt,
        row.filler
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// `DELETE` of the row whose key is `acct_id`, its details with it through the cascade; whether
/// exactly one summary was deleted.
async fn delete_row(conn: &mut PgConnection, acct_id: i64) -> Result<bool, PersistenceError> {
    let result = sqlx::query!(
        "DELETE FROM pending_auth_summaries WHERE acct_id = $1",
        acct_id
    )
    .execute(conn)
    .await?;
    Ok(result.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::time::Duration;

    use copybook::{Count, PicS, PicU, PicX};
    use domain::money::Money;
    use domain::records::PendingAuthSummaryRecord;
    use rust_decimal::Decimal;
    use sqlx::PgPool;
    use tokio::task::JoinHandle;
    use tokio::time;

    use super::PendingAuthSummaryRepository;
    use crate::{PersistenceError, PgStore};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// What a `lock_or_create` running in a task of its own returns.
    type Locked = Result<(PendingAuthSummaryRecord, bool), PersistenceError>;

    /// What a `lock_next` running in a task of its own returns.
    type Next = Result<Option<PendingAuthSummaryRecord>, PersistenceError>;

    /// How long a call that must wait for a row lock is watched before the test treats it as
    /// waiting.
    const LOCK_WAIT: Duration = Duration::from_millis(500);

    /// The amount of `cents` hundredths.
    fn money(cents: i64) -> Result<Money<9>, Box<dyn StdError>> {
        Ok(Money::new(Decimal::new(cents, 2))?)
    }

    /// The zeroed summary CP00 builds for account `acct` and customer `cust`
    /// (COPAUA0C-092).
    fn template(acct: i64, cust: u64) -> Result<PendingAuthSummaryRecord, Box<dyn StdError>> {
        Ok(PendingAuthSummaryRecord::initialized(
            PicS::new(acct)?,
            PicU::new(cust)?,
        ))
    }

    /// A summary whose fields all differ from the template's: distinct status occurrences, a
    /// negative balance and amount, the largest amount and count, and `FILLER` text.
    fn sample(acct: i64) -> Result<PendingAuthSummaryRecord, Box<dyn StdError>> {
        let mut record = template(acct, 50)?;
        record.auth_status = PicX::from_array(*b"A");
        record.account_status = [
            PicX::from_array(*b"A1"),
            PicX::from_array(*b"B2"),
            PicX::from_array(*b"C3"),
            PicX::from_array(*b"D4"),
            PicX::from_array(*b"E5"),
        ];
        record.credit_limit = money(500_000)?;
        record.cash_limit = money(150_000)?;
        record.credit_balance = money(-123_456)?;
        record.approved_auth_cnt = Count::new(2);
        record.approved_auth_amt = money(99_999_999_999)?;
        record.declined_auth_cnt = Count::new(Count::MAX);
        record.declined_auth_amt = money(-1)?;
        record.filler = PicX::from_array(*b"FILLER KEPT AS STORED, NOT BLANKED");
        Ok(record)
    }

    /// `record` after one approval of 100.00, as CP00's summary write stores it.
    fn charged(
        record: &PendingAuthSummaryRecord,
    ) -> Result<PendingAuthSummaryRecord, Box<dyn StdError>> {
        let mut record = record.clone();
        record.approved_auth_cnt = Count::new(1);
        record.approved_auth_amt = money(10_000)?;
        record.credit_balance = money(10_000)?;
        Ok(record)
    }

    /// Runs `lock_or_create(template)` in a transaction of its own on a spawned task, the way a
    /// second worker process decides for the same account, and commits it.
    fn lock_elsewhere(store: &PgStore, template: &PendingAuthSummaryRecord) -> JoinHandle<Locked> {
        let begin = store.begin();
        let template = template.clone();
        tokio::spawn(async move {
            let mut tx = begin.await?;
            let locked = tx.lock_or_create(&template).await?;
            tx.commit().await?;
            Ok(locked)
        })
    }

    /// Runs `lock_next(after)` in a transaction of its own on a spawned task, the way the purge
    /// reads its next summary beside CP00, and commits it.
    fn next_elsewhere(store: &PgStore, after: Option<PicS<11>>) -> JoinHandle<Next> {
        let begin = store.begin();
        tokio::spawn(async move {
            let mut tx = begin.await?;
            let next = tx.lock_next(after).await?;
            tx.commit().await?;
            Ok(next)
        })
    }

    /// The keys `lock_next` returns on `repo`, one call at a time from no key to `None`.
    async fn walk(
        repo: &mut impl PendingAuthSummaryRepository,
    ) -> Result<Vec<i64>, PersistenceError> {
        let mut keys = Vec::new();
        let mut after = None;
        while let Some(summary) = repo.lock_next(after).await? {
            keys.push(summary.acct_id.get());
            after = Some(summary.acct_id);
        }
        Ok(keys)
    }

    /// Inserts one detail under the summary of `acct_id`: blank texts, zero numbers.
    async fn insert_detail(pool: &PgPool, acct_id: i64) -> Result<(), sqlx::Error> {
        sqlx::query!(
            "INSERT INTO pending_auth_details (acct_id, auth_date_9c, auth_time_9c, \
             auth_orig_date, auth_orig_time, card_num, auth_type, card_expiry_date, \
             message_type, message_source, auth_id_code, auth_resp_code, auth_resp_reason, \
             processing_code, transaction_amt, approved_amt, merchant_catagory_code, \
             acqr_country_code, pos_entry_mode, merchant_id, merchant_name, merchant_city, \
             merchant_state, merchant_zip, transaction_id, match_status, auth_fraud, \
             fraud_rpt_date, filler) \
             VALUES ($1, 0, 0, '', '', '', '', '', '', '', '', '', '', 0, 0, 0, '', '', 0, '', \
             '', '', '', '', '', '', '', '', '')",
            acct_id
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// How many details the account `acct_id` has.
    async fn details(pool: &PgPool, acct_id: i64) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!" FROM pending_auth_details WHERE acct_id = $1"#,
            acct_id
        )
        .fetch_one(pool)
        .await
    }

    /// The keys `find_all` returns, in its order.
    async fn keys(store: &mut PgStore) -> Result<Vec<i64>, PersistenceError> {
        let all = store.find_all().await?;
        Ok(all.iter().map(|summary| summary.acct_id.get()).collect())
    }

    /// The first call creates the summary from the template and the next ones lock the stored
    /// row, whatever template they pass; the store form commits on its own.
    // audit: COPAUA0C-025
    // audit: COPAUA0C-092
    // audit: COPAUA0C-097
    #[tokio::test]
    async fn lock_or_create_creates_once_then_locks_the_stored_row() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let new = template(50, 50)?;

        let mut tx = store.begin().await?;
        assert_eq!(tx.lock_or_create(&new).await?, (new.clone(), true));
        assert_eq!(tx.find_by_id(new.acct_id).await?, Some(new.clone()));
        let written = charged(&new)?;
        assert!(tx.update(&written).await?);
        tx.commit().await?;

        let mut tx = store.begin().await?;
        let other_customer = template(50, 77)?;
        assert_eq!(
            tx.lock_or_create(&other_customer).await?,
            (written.clone(), false)
        );
        tx.rollback().await?;
        assert_eq!(store.find_by_id(new.acct_id).await?, Some(written));

        let fresh = template(51, 51)?;
        assert_eq!(store.lock_or_create(&fresh).await?, (fresh.clone(), true));
        assert_eq!(store.lock_or_create(&fresh).await?, (fresh.clone(), false));
        assert_eq!(store.find_by_id(fresh.acct_id).await?, Some(fresh));
        Ok(())
    }

    /// A second decision for the account waits for the transaction holding the lock, a first
    /// authorization included, and then sees what it committed; a summary deleted while its
    /// lock is awaited is created afresh.
    // audit: COPAUA0C-026
    // audit: COPAUA0C-049
    // audit: CBPAUP0C-016
    #[tokio::test]
    async fn a_second_decision_waits_for_the_lock_and_reads_committed_state() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        let new = template(60, 60)?;

        let mut first = store.begin().await?;
        let (row, created) = first.lock_or_create(&new).await?;
        assert!(created);
        let mut second = lock_elsewhere(&store, &new);
        assert!(
            time::timeout(LOCK_WAIT, &mut second).await.is_err(),
            "the second decision runs only after the first commits"
        );
        let written = charged(&row)?;
        assert!(first.update(&written).await?);
        first.commit().await?;
        assert_eq!(second.await??, (written, false));

        let mut purge = store.begin().await?;
        assert!(!purge.lock_or_create(&new).await?.1);
        let mut waiting = lock_elsewhere(&store, &new);
        assert!(
            time::timeout(LOCK_WAIT, &mut waiting).await.is_err(),
            "the decision waits while the purge holds the lock"
        );
        assert!(purge.delete(new.acct_id).await?);
        purge.commit().await?;
        assert_eq!(waiting.await??, (new, true));
        Ok(())
    }

    /// The purge's read returns the summaries one at a time in key order, then `None`. It waits
    /// for a decision that holds the next row's lock and returns what that decision committed,
    /// skips a row the lock holder deleted, and on a transaction holds its own lock, through a
    /// released savepoint, until the commit.
    // audit: CBPAUP0C-009
    #[tokio::test]
    async fn lock_next_reads_each_summary_as_last_committed_in_key_order() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        assert_eq!(store.lock_next(None).await?, None);
        for acct in [30, 10, 20] {
            store.insert(&template(acct, 1)?).await?;
        }
        assert_eq!(walk(&mut store).await?, [10, 20, 30]);
        let between = Some(PicS::new(15)?);
        assert_eq!(store.lock_next(between).await?, Some(template(20, 1)?));
        let mut tx = store.begin().await?;
        assert_eq!(walk(&mut tx).await?, [10, 20, 30]);
        tx.rollback().await?;

        let after_first = Some(PicS::new(10)?);
        let mut cp00 = store.begin().await?;
        let (row, created) = cp00.lock_or_create(&template(20, 1)?).await?;
        assert!(!created);
        let mut next = next_elsewhere(&store, after_first);
        assert!(
            time::timeout(LOCK_WAIT, &mut next).await.is_err(),
            "the read waits for the decision that holds the row"
        );
        let written = charged(&row)?;
        assert!(cp00.update(&written).await?);
        cp00.commit().await?;
        assert_eq!(next.await??, Some(written));

        let mut holder = store.begin().await?;
        assert!(!holder.lock_or_create(&template(20, 1)?).await?.1);
        let mut skipping = next_elsewhere(&store, after_first);
        assert!(
            time::timeout(LOCK_WAIT, &mut skipping).await.is_err(),
            "the read waits for the transaction that deletes the row"
        );
        assert!(holder.delete(PicS::new(20)?).await?);
        holder.commit().await?;
        assert_eq!(skipping.await??, Some(template(30, 1)?));

        let mut purge = store.begin().await?;
        let mut point = purge.savepoint().await?;
        assert_eq!(point.lock_next(None).await?, Some(template(10, 1)?));
        point.commit().await?;
        let mut decision = lock_elsewhere(&store, &template(10, 1)?);
        assert!(
            time::timeout(LOCK_WAIT, &mut decision).await.is_err(),
            "a decision waits for the commit of the transaction that read the row"
        );
        purge.commit().await?;
        assert_eq!(decision.await??, (template(10, 1)?, false));
        Ok(())
    }

    /// Reads, inserts, updates and deletes on both executors, the cascade to the details and
    /// the key order of `find_all`.
    // audit: COPAUS0C-027
    // audit: COPAUS0C-029
    // audit: COPAUA0C-097
    // audit: CBPAUP0C-016
    #[tokio::test]
    async fn statements_report_their_outcome_and_deletes_cascade() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let summary = sample(70)?;
        let key = summary.acct_id;

        assert_eq!(store.find_by_id(key).await?, None);
        assert!(!store.update(&summary).await?);
        assert!(!store.delete(key).await?);
        store.insert(&summary).await?;
        assert_eq!(store.find_by_id(key).await?, Some(summary.clone()));
        assert_eq!(
            store.insert(&summary).await,
            Err(PersistenceError::Duplicate {
                constraint: Some("pending_auth_summaries_pkey".to_owned()),
            })
        );

        let mut changed = template(70, 71)?;
        changed.declined_auth_cnt = Count::new(3);
        assert!(store.update(&changed).await?);
        assert_eq!(store.find_by_id(key).await?, Some(changed));

        insert_detail(guard.pool(), 70).await?;
        assert_eq!(details(guard.pool(), 70).await?, 1);
        assert!(store.delete(key).await?);
        assert_eq!(store.find_by_id(key).await?, None);
        assert_eq!(details(guard.pool(), 70).await?, 0);

        for acct in [30, -5, 7] {
            store.insert(&template(acct, 1)?).await?;
        }
        assert_eq!(keys(&mut store).await?, [-5, 7, 30]);

        let mut tx = store.begin().await?;
        assert_eq!(tx.find_all().await?.len(), 3);
        // A failed insert inside a savepoint leaves the enclosing transaction usable.
        let mut savepoint = tx.savepoint().await?;
        assert!(matches!(
            savepoint.insert(&template(7, 1)?).await,
            Err(PersistenceError::Duplicate { .. })
        ));
        savepoint.rollback().await?;
        tx.insert(&template(8, 1)?).await?;
        let moved = template(30, 2)?;
        assert!(tx.update(&moved).await?);
        assert!(tx.delete(PicS::new(-5)?).await?);
        assert_eq!(tx.find_by_id(PicS::new(30)?).await?, Some(moved));
        tx.rollback().await?;
        assert_eq!(keys(&mut store).await?, [-5, 7, 30]);
        assert_eq!(
            store.find_by_id(PicS::new(30)?).await?,
            Some(template(30, 1)?)
        );
        Ok(())
    }
}
