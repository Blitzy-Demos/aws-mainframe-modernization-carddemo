//! The repository of table `pending_auth_details` (migration 0004): the pending-authorization
//! detail `CIPAUDTY`, IMS child segment `PAUTDTL1` under the summary root `PAUTSUM0` of the HIDAM
//! database `DBPAUTP0` (`DBPAUTP0.dbd:28-37`).
//!
//! # Key and order
//!
//! A detail is addressed by its parent's root key `ACCNTID`, column `acct_id`, followed by the
//! unique sequence field `PAUT9CTS`, columns `auth_date_9c` and `auth_time_9c`, the primary key
//! of the table. The account lies outside the 200-byte segment (CIPAUDTY-029), so every method
//! takes it beside the record and no read returns it: the caller already holds it. The two
//! sequence parts are stored as the record holds them, already inverted (`99999 − YYDDD` and
//! `999999999 − HHMMSSmmm`, `COPAUA0C.cbl:871-875`, COPAUA0C-098), so ascending key order is the
//! IMS twin order under one summary, newest first (COPAUS0C-031, CBPAUP0C-010). Every read that
//! returns several details returns them in that order, through the primary key.
//!
//! # The DL/I calls each method replaces
//!
//! | COBOL access | Method |
//! |---|---|
//! | COPAUA0C `8500-INSERT-AUTH`, `ISRT` of `PAUTDTL1` under `ACCNTID` (`COPAUA0C.cbl:854-935`) | [`insert`](PendingAuthDetailRepository::insert) |
//! | COPAUS0C `GET-AUTHORIZATIONS`, `GNP`, and `REPOSITION-AUTHORIZATIONS`, `GNP WHERE (PAUT9CTS = key)` (`COPAUS0C.cbl:458-520`) | [`page_after`](PendingAuthDetailRepository::page_after) |
//! | COPAUS1C `READ-AUTH-RECORD`, `GNP WHERE (PAUT9CTS = key)` (`COPAUS1C.cbl:465-469`), on ENTER and F8 | [`find_by_id`](PendingAuthDetailRepository::find_by_id) |
//! | The same read under `MARK-AUTH-FRAUD` on F5, ahead of the `REPL`, held as the get-hold call a `REPL` requires (`COPAUS1C.cbl:230-265`; X-COPAUS1C-1) | [`lock_by_id`](PendingAuthDetailRepository::lock_by_id) |
//! | COPAUS1C `READ-NEXT-AUTH-RECORD`, unqualified `GNP` (`COPAUS1C.cbl:493-498`) | [`page_after`](PendingAuthDetailRepository::page_after) with a limit of 2 from the current key |
//! | COPAUS1C `UPDATE-AUTH-DETAILS`, `REPL` (`COPAUS1C.cbl:520-528`) | [`update`](PendingAuthDetailRepository::update) |
//! | CBPAUP0C `3000-FIND-NEXT-AUTH-DTL`, `GNP` until `GE` (`CBPAUP0C.cbl:248-273`) | [`find_all_by_acct_id`](PendingAuthDetailRepository::find_all_by_acct_id) |
//! | CBPAUP0C `5000-DELETE-AUTH-DTL`, `DLET` (`CBPAUP0C.cbl:303-324`) | [`delete`](PendingAuthDetailRepository::delete) |
//!
//! The repository decides nothing the programs decide. Which key a page starts from, whether a
//! repositioned page keeps or drops its first row, whether a detail has expired and which flag a
//! fraud toggle writes are decisions of the services, workers and jobs that call it, after the
//! audit rows of COPAUS0C, COPAUS1C, COPAUA0C and CBPAUP0C.
//!
//! # Outcomes
//!
//! - A missing detail is `Ok(None)`, and an update or delete that changes no row is `Ok(false)`:
//!   the IMS `GE` status the programs branch on.
//! - A second detail with the same key under one account is [`PersistenceError::Duplicate`], the
//!   IMS `II` status of CP00's `I004` path (COPAUA0C-099). A detail whose account has no summary
//!   is [`PersistenceError::Restrict`] through foreign key `pending_auth_details_summary_fk`,
//!   where an IMS `ISRT` qualified by a missing parent fails with `GE`. CP00 never meets it,
//!   because it inserts the detail after the summary in the same transaction.
//! - Deleting a summary deletes its details (`ON DELETE CASCADE`), as IMS deletes the children
//!   of a deleted root; no method here does it.
//! - Every other failure converts through `From<sqlx::Error>` and keeps only an SQLSTATE and a
//!   constraint name.
//!
//! # Redaction (Cardholder Data Redaction)
//!
//! `PA-CARD-NUM` is stored in full. It reaches a bind parameter only through
//! [`PendingAuthDetailRow`], which takes it through `CardNumber::expose`, and it leaves a read
//! only inside the record's `CardNumber`, whose `Debug` shows the redacted form. Nothing here
//! formats a record, a row or a key into an error, a log or a span.

use copybook::PicS;
use domain::records::PendingAuthDetailRecord;
use sqlx::PgExecutor;

use crate::error::PersistenceError;
use crate::rows::{PendingAuthDetailRow, pic_s_to_i64};
use crate::{PgStore, PgTx, ProbePoint, probed};

/// The pending-authorization details, one row per `PAUTDTL1` segment, keyed by the summary's
/// account and the inverted date and time of the authorization.
///
/// Implemented by [`PgStore`], where each call runs on its own pooled connection and commits at
/// once, and by [`PgTx`], where each call runs inside the caller's transaction or savepoint.
/// Test doubles in other crates implement the same methods.
pub trait PendingAuthDetailRepository: Send {
    /// The detail of account `acct` whose sequence key is (`date_9c`, `time_9c`), the qualified
    /// `GNP` of COPAUS1C `READ-AUTH-RECORD` (`COPAUS1C.cbl:465-469`, COPAUS1C-008). `None` when
    /// the account holds no detail with that key, the `GE` status of COPAUS1C-009.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when the stored row does not fit `CIPAUDTY`;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read fails.
    fn find_by_id(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<Option<PendingAuthDetailRecord>, PersistenceError>> + Send;

    /// The detail of account `acct` whose sequence key is (`date_9c`, `time_9c`), read with a
    /// row lock (`SELECT … FOR UPDATE`): the read of COPAUS1C `MARK-AUTH-FRAUD` that its `REPL`
    /// follows, held as the get-hold call IMS requires before a `REPL` would hold the segment
    /// (`COPAUS1C.cbl:230-265`; COPAUS1C-028, X-COPAUS1C-1). `None` as for
    /// [`find_by_id`](Self::find_by_id).
    ///
    /// On a [`PgTx`] the row stays locked until the transaction commits or rolls back, so two
    /// fraud toggles of one detail run one after the other. A lock that another transaction
    /// holds makes the call wait; once that transaction commits, the call returns the row as it
    /// committed it, so the flag a toggle reads is the one the toggle before it stored. On a
    /// [`PgStore`] the statement commits at once and the lock ends with the call.
    ///
    /// # Errors
    ///
    /// As [`find_by_id`](Self::find_by_id); a lock wait the server ends, such as a lock timeout
    /// (`55P03`) or a deadlock (`40P01`), is [`PersistenceError::Database`] with that SQLSTATE,
    /// the detail-read failure endpoint 23 answers with COPAUS1C-011's text.
    fn lock_by_id(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<Option<PendingAuthDetailRecord>, PersistenceError>> + Send;

    /// Up to `limit` details of account `acct`, in ascending key order from the key
    /// (`start_date_9c`, `start_time_9c`) on, that key included: the twin-order `GNP` of COPAUS0C
    /// `GET-AUTHORIZATIONS` and, after its `GNP WHERE (PAUT9CTS = key)`, of
    /// `REPOSITION-AUTHORIZATIONS` (`COPAUS0C.cbl:458-520`; COPAUS0C-031, COPAUS0C-041,
    /// COPAUS0C-047), and the next-detail `GNP` of COPAUS1C `READ-NEXT-AUTH-RECORD` when called
    /// from the current key with a limit of 2 (`COPAUS1C.cbl:493-498`, COPAUS1C-024).
    ///
    /// The statement is `queries/pending_auth_details_page_forward.sql`, served by the primary
    /// key (Bounded Pagination). The caller chooses every bound:
    ///
    /// - a first page starts from the smallest keys the pictures allow, −99999 and −999999999;
    /// - `limit` is bound as given, so a page that must tell whether another follows asks for one
    ///   row more than it shows, and a page repositioned after a key one more again, for the row
    ///   at that key (`copaus0c::PageRequest::fetch_limit`, COPAUS0C-041); the API validates the
    ///   page size before it gets here;
    /// - whether a repositioned page keeps its first row, and whether that row must equal the key
    ///   it started from, are decisions of the service after the COPAUS0C and COPAUS1C audits.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Database`] for a negative `limit`, which `PostgreSQL` refuses;
    /// [`PersistenceError::Decode`] when a stored row does not fit `CIPAUDTY`;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read fails.
    fn page_after(
        &mut self,
        acct: PicS<11>,
        start_date_9c: PicS<5>,
        start_time_9c: PicS<9>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<PendingAuthDetailRecord>, PersistenceError>> + Send;

    /// Every detail of account `acct`, in ascending key order: the `GNP` loop under one summary
    /// of CBPAUP0C `3000-FIND-NEXT-AUTH-DTL`, which reads until `GE` (`CBPAUP0C.cbl:248-273`,
    /// CBPAUP0C-010). Unbounded, so it serves the `cbpaup0j` purge only, never an endpoint.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a stored row does not fit `CIPAUDTY`;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read fails,
    /// the read failure of CBPAUP0C-024.
    fn find_all_by_acct_id(
        &mut self,
        acct: PicS<11>,
    ) -> impl Future<Output = Result<Vec<PendingAuthDetailRecord>, PersistenceError>> + Send;

    /// Stores `record` under the summary of account `acct`: the `ISRT` of COPAUA0C
    /// `8500-INSERT-AUTH` (`COPAUA0C.cbl:854-935`; COPAUA0C-098 to COPAUA0C-103).
    ///
    /// CP00 calls it on a [`PgTx::savepoint`] opened inside the transaction that holds the
    /// summary lock, after the reply is confirmed and the request acknowledged (AAP 0.3.4.2), for
    /// the reason
    /// [Reply before write (E29)](crate::repos::pending_auth_summaries#reply-before-write-e29)
    /// gives. When the insert fails, a failed statement having aborted the savepoint alone, the
    /// worker rolls back to the savepoint, logs `I004` with `IMS INSERT DETL FAILED` and commits
    /// the summary update without its detail, as the COBOL's unit of work commits at the normal
    /// end of its task (`COPAUA0C.cbl:925-931,1008-1009,1016-1022`, COPAUA0C-120).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when the account already holds a detail with the same key,
    /// as two authorizations in one millisecond produce (`COPAUA0C.cbl:871-875`, COPAUA0C-099);
    /// [`PersistenceError::Restrict`] when the account has no summary; [`PersistenceError::Decode`]
    /// when a field holds a byte no column can store; [`PersistenceError::Check`] when a key is
    /// outside its picture; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`]
    /// when the write fails.
    fn insert(
        &mut self,
        acct: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Replaces every field of the stored detail of account `acct` that has `record`'s own key
    /// with `record`'s: the `REPL` of COPAUS1C `UPDATE-AUTH-DETAILS` (`COPAUS1C.cbl:520-528`,
    /// COPAUS1C-032). The key itself never changes, as `REPL` cannot change a sequence field.
    /// `true` when exactly one row was replaced; `false` when the account holds no detail with
    /// that key.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a field holds a byte no column can store;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the write fails,
    /// the replace failure of COPAUS1C-033.
    fn update(
        &mut self,
        acct: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Deletes the detail of account `acct` whose sequence key is (`date_9c`, `time_9c`): the
    /// `DLET` of CBPAUP0C `5000-DELETE-AUTH-DTL` (`CBPAUP0C.cbl:303-324`, CBPAUP0C-015). `true`
    /// when exactly one row was deleted; `false` when the account holds no detail with that key.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the delete fails,
    /// the delete failure of CBPAUP0C-025.
    fn delete(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;
}

impl PendingAuthDetailRepository for PgStore {
    fn find_by_id(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<Option<PendingAuthDetailRecord>, PersistenceError>> + Send
    {
        let verdict = self.verdict(ProbePoint::AuthDetailRead, &[&acct, &date_9c, &time_9c]);
        probed(verdict, select_by_key(self.pool(), acct, date_9c, time_9c))
    }

    fn lock_by_id(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<Option<PendingAuthDetailRecord>, PersistenceError>> + Send
    {
        let verdict = self.verdict(ProbePoint::AuthDetailLock, &[&acct, &date_9c, &time_9c]);
        probed(
            verdict,
            select_for_update(self.pool(), acct, date_9c, time_9c),
        )
    }

    fn page_after(
        &mut self,
        acct: PicS<11>,
        start_date_9c: PicS<5>,
        start_time_9c: PicS<9>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<PendingAuthDetailRecord>, PersistenceError>> + Send {
        let verdict = self.verdict(
            ProbePoint::AuthDetailPage,
            &[&acct, &start_date_9c, &start_time_9c, &limit],
        );
        probed(
            verdict,
            select_page_forward(self.pool(), acct, start_date_9c, start_time_9c, limit),
        )
    }

    fn find_all_by_acct_id(
        &mut self,
        acct: PicS<11>,
    ) -> impl Future<Output = Result<Vec<PendingAuthDetailRecord>, PersistenceError>> + Send {
        select_all_of_account(self.pool(), acct)
    }

    fn insert(
        &mut self,
        acct: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_row(self.pool(), acct, record)
    }

    fn update(
        &mut self,
        acct: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AuthDetailUpdate, &[&acct, record]);
        probed(verdict, update_row(self.pool(), acct, record))
    }

    fn delete(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        delete_row(self.pool(), acct, date_9c, time_9c)
    }
}

impl PendingAuthDetailRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<Option<PendingAuthDetailRecord>, PersistenceError>> + Send
    {
        let verdict = self.verdict(ProbePoint::AuthDetailRead, &[&acct, &date_9c, &time_9c]);
        probed(verdict, select_by_key(self.conn(), acct, date_9c, time_9c))
    }

    fn lock_by_id(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<Option<PendingAuthDetailRecord>, PersistenceError>> + Send
    {
        let verdict = self.verdict(ProbePoint::AuthDetailLock, &[&acct, &date_9c, &time_9c]);
        probed(
            verdict,
            select_for_update(self.conn(), acct, date_9c, time_9c),
        )
    }

    fn page_after(
        &mut self,
        acct: PicS<11>,
        start_date_9c: PicS<5>,
        start_time_9c: PicS<9>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<PendingAuthDetailRecord>, PersistenceError>> + Send {
        let verdict = self.verdict(
            ProbePoint::AuthDetailPage,
            &[&acct, &start_date_9c, &start_time_9c, &limit],
        );
        probed(
            verdict,
            select_page_forward(self.conn(), acct, start_date_9c, start_time_9c, limit),
        )
    }

    fn find_all_by_acct_id(
        &mut self,
        acct: PicS<11>,
    ) -> impl Future<Output = Result<Vec<PendingAuthDetailRecord>, PersistenceError>> + Send {
        select_all_of_account(self.conn(), acct)
    }

    fn insert(
        &mut self,
        acct: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_row(self.conn(), acct, record)
    }

    fn update(
        &mut self,
        acct: PicS<11>,
        record: &PendingAuthDetailRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AuthDetailUpdate, &[&acct, record]);
        probed(verdict, update_row(self.conn(), acct, record))
    }

    fn delete(
        &mut self,
        acct: PicS<11>,
        date_9c: PicS<5>,
        time_9c: PicS<9>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        delete_row(self.conn(), acct, date_9c, time_9c)
    }
}

/// The record a read row stores. The row's account is the one the statement selected on, which
/// the caller already holds, so it is dropped.
fn into_detail(row: PendingAuthDetailRow) -> Result<PendingAuthDetailRecord, PersistenceError> {
    row.into_record().map(|(_, record)| record)
}

/// Every row of a multi-row read as its record, the first row that does not fit `CIPAUDTY`
/// failing the whole read.
fn into_details(
    rows: Vec<PendingAuthDetailRow>,
) -> Result<Vec<PendingAuthDetailRecord>, PersistenceError> {
    rows.into_iter().map(into_detail).collect()
}

/// [`PendingAuthDetailRepository::find_by_id`]: one row by its full primary key.
async fn select_by_key<'e, E: PgExecutor<'e>>(
    executor: E,
    acct: PicS<11>,
    date_9c: PicS<5>,
    time_9c: PicS<9>,
) -> Result<Option<PendingAuthDetailRecord>, PersistenceError> {
    let row = sqlx::query_as!(
        PendingAuthDetailRow,
        "SELECT acct_id, auth_date_9c, auth_time_9c, auth_orig_date, auth_orig_time, card_num, \
             auth_type, card_expiry_date, message_type, message_source, auth_id_code, \
             auth_resp_code, auth_resp_reason, processing_code, transaction_amt, approved_amt, \
             merchant_catagory_code, acqr_country_code, pos_entry_mode, merchant_id, \
             merchant_name, merchant_city, merchant_state, merchant_zip, transaction_id, \
             match_status, auth_fraud, fraud_rpt_date, filler \
         FROM pending_auth_details \
         WHERE acct_id = $1 AND auth_date_9c = $2 AND auth_time_9c = $3",
        pic_s_to_i64(acct),
        pic_s_to_i64(date_9c),
        pic_s_to_i64(time_9c),
    )
    .fetch_optional(executor)
    .await?;
    row.map(into_detail).transpose()
}

/// [`PendingAuthDetailRepository::lock_by_id`]: one row by its full primary key, `FOR UPDATE`.
/// The row lock lasts as long as the executor's transaction: until the caller's [`PgTx`]
/// commits or rolls back, or, on the pool, only as long as the statement.
async fn select_for_update<'e, E: PgExecutor<'e>>(
    executor: E,
    acct: PicS<11>,
    date_9c: PicS<5>,
    time_9c: PicS<9>,
) -> Result<Option<PendingAuthDetailRecord>, PersistenceError> {
    let row = sqlx::query_as!(
        PendingAuthDetailRow,
        "SELECT acct_id, auth_date_9c, auth_time_9c, auth_orig_date, auth_orig_time, card_num, \
             auth_type, card_expiry_date, message_type, message_source, auth_id_code, \
             auth_resp_code, auth_resp_reason, processing_code, transaction_amt, approved_amt, \
             merchant_catagory_code, acqr_country_code, pos_entry_mode, merchant_id, \
             merchant_name, merchant_city, merchant_state, merchant_zip, transaction_id, \
             match_status, auth_fraud, fraud_rpt_date, filler \
         FROM pending_auth_details \
         WHERE acct_id = $1 AND auth_date_9c = $2 AND auth_time_9c = $3 \
         FOR UPDATE",
        pic_s_to_i64(acct),
        pic_s_to_i64(date_9c),
        pic_s_to_i64(time_9c),
    )
    .fetch_optional(executor)
    .await?;
    row.map(into_detail).transpose()
}

/// [`PendingAuthDetailRepository::page_after`]: the paginated read, its text the committed query
/// file that the `explain` test plans.
async fn select_page_forward<'e, E: PgExecutor<'e>>(
    executor: E,
    acct: PicS<11>,
    start_date_9c: PicS<5>,
    start_time_9c: PicS<9>,
    limit: i64,
) -> Result<Vec<PendingAuthDetailRecord>, PersistenceError> {
    let rows = sqlx::query_file_as!(
        PendingAuthDetailRow,
        "queries/pending_auth_details_page_forward.sql",
        pic_s_to_i64(acct),
        pic_s_to_i64(start_date_9c),
        pic_s_to_i64(start_time_9c),
        limit,
    )
    .fetch_all(executor)
    .await?;
    into_details(rows)
}

/// [`PendingAuthDetailRepository::find_all_by_acct_id`]: every row of one account, a range scan
/// of the primary key's leading column.
async fn select_all_of_account<'e, E: PgExecutor<'e>>(
    executor: E,
    acct: PicS<11>,
) -> Result<Vec<PendingAuthDetailRecord>, PersistenceError> {
    let rows = sqlx::query_as!(
        PendingAuthDetailRow,
        "SELECT acct_id, auth_date_9c, auth_time_9c, auth_orig_date, auth_orig_time, card_num, \
             auth_type, card_expiry_date, message_type, message_source, auth_id_code, \
             auth_resp_code, auth_resp_reason, processing_code, transaction_amt, approved_amt, \
             merchant_catagory_code, acqr_country_code, pos_entry_mode, merchant_id, \
             merchant_name, merchant_city, merchant_state, merchant_zip, transaction_id, \
             match_status, auth_fraud, fraud_rpt_date, filler \
         FROM pending_auth_details \
         WHERE acct_id = $1 \
         ORDER BY auth_date_9c ASC, auth_time_9c ASC",
        pic_s_to_i64(acct),
    )
    .fetch_all(executor)
    .await?;
    into_details(rows)
}

/// [`PendingAuthDetailRepository::insert`]: one row, every column, built by codec B.
async fn insert_row<'e, E: PgExecutor<'e>>(
    executor: E,
    acct: PicS<11>,
    record: &PendingAuthDetailRecord,
) -> Result<(), PersistenceError> {
    let row = PendingAuthDetailRow::from_record(acct, record)?;
    sqlx::query!(
        "INSERT INTO pending_auth_details (acct_id, auth_date_9c, auth_time_9c, auth_orig_date, \
             auth_orig_time, card_num, auth_type, card_expiry_date, message_type, message_source, \
             auth_id_code, auth_resp_code, auth_resp_reason, processing_code, transaction_amt, \
             approved_amt, merchant_catagory_code, acqr_country_code, pos_entry_mode, \
             merchant_id, merchant_name, merchant_city, merchant_state, merchant_zip, \
             transaction_id, match_status, auth_fraud, fraud_rpt_date, filler) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
             $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29)",
        row.acct_id,
        row.auth_date_9c,
        row.auth_time_9c,
        row.auth_orig_date,
        row.auth_orig_time,
        row.card_num,
        row.auth_type,
        row.card_expiry_date,
        row.message_type,
        row.message_source,
        row.auth_id_code,
        row.auth_resp_code,
        row.auth_resp_reason,
        row.processing_code,
        row.transaction_amt,
        row.approved_amt,
        row.merchant_catagory_code,
        row.acqr_country_code,
        row.pos_entry_mode,
        row.merchant_id,
        row.merchant_name,
        row.merchant_city,
        row.merchant_state,
        row.merchant_zip,
        row.transaction_id,
        row.match_status,
        row.auth_fraud,
        row.fraud_rpt_date,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// [`PendingAuthDetailRepository::update`]: every non-key column of the row that has the
/// record's own key.
async fn update_row<'e, E: PgExecutor<'e>>(
    executor: E,
    acct: PicS<11>,
    record: &PendingAuthDetailRecord,
) -> Result<bool, PersistenceError> {
    let row = PendingAuthDetailRow::from_record(acct, record)?;
    let done = sqlx::query!(
        "UPDATE pending_auth_details \
         SET auth_orig_date = $4, auth_orig_time = $5, card_num = $6, auth_type = $7, \
             card_expiry_date = $8, message_type = $9, message_source = $10, \
             auth_id_code = $11, auth_resp_code = $12, auth_resp_reason = $13, \
             processing_code = $14, transaction_amt = $15, approved_amt = $16, \
             merchant_catagory_code = $17, acqr_country_code = $18, pos_entry_mode = $19, \
             merchant_id = $20, merchant_name = $21, merchant_city = $22, merchant_state = $23, \
             merchant_zip = $24, transaction_id = $25, match_status = $26, auth_fraud = $27, \
             fraud_rpt_date = $28, filler = $29 \
         WHERE acct_id = $1 AND auth_date_9c = $2 AND auth_time_9c = $3",
        row.acct_id,
        row.auth_date_9c,
        row.auth_time_9c,
        row.auth_orig_date,
        row.auth_orig_time,
        row.card_num,
        row.auth_type,
        row.card_expiry_date,
        row.message_type,
        row.message_source,
        row.auth_id_code,
        row.auth_resp_code,
        row.auth_resp_reason,
        row.processing_code,
        row.transaction_amt,
        row.approved_amt,
        row.merchant_catagory_code,
        row.acqr_country_code,
        row.pos_entry_mode,
        row.merchant_id,
        row.merchant_name,
        row.merchant_city,
        row.merchant_state,
        row.merchant_zip,
        row.transaction_id,
        row.match_status,
        row.auth_fraud,
        row.fraud_rpt_date,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// [`PendingAuthDetailRepository::delete`]: one row by its full primary key.
async fn delete_row<'e, E: PgExecutor<'e>>(
    executor: E,
    acct: PicS<11>,
    date_9c: PicS<5>,
    time_9c: PicS<9>,
) -> Result<bool, PersistenceError> {
    let done = sqlx::query!(
        "DELETE FROM pending_auth_details \
         WHERE acct_id = $1 AND auth_date_9c = $2 AND auth_time_9c = $3",
        pic_s_to_i64(acct),
        pic_s_to_i64(date_9c),
        pic_s_to_i64(time_9c),
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::pin::pin;
    use std::time::Duration;

    use copybook::{PicS, PicU, PicX};
    use domain::money::Money;
    use domain::records::PendingAuthDetailRecord;
    use domain::sensitive::CardNumber;
    use rust_decimal::Decimal;
    use sqlx::PgPool;
    use tokio::time::timeout;

    use super::PendingAuthDetailRepository;
    use crate::{PersistenceError, PgStore};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread runtime, the one `#[tokio::test]` builds,
    /// without the lint attributes that macro's expansion carries.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

    /// How long a locked read is given to show that it waits: far longer than a read of one row
    /// by its key takes on an idle server.
    const LOCK_WAIT: Duration = Duration::from_millis(300);

    /// Not a fixture card number: no error may repeat it, nor its last four digits.
    const CARD: &str = "9876543210987654";
    const LAST_FOUR: &str = "7654";

    /// The account the tests store details under, and a second one beside it.
    const ACCT: i64 = 50;
    const OTHER_ACCT: i64 = 51;

    /// The name `PostgreSQL` gives the primary key of `pending_auth_details`.
    const PRIMARY_KEY: &str = "pending_auth_details_pkey";

    /// The detail-to-summary foreign key of migration 0004.
    const SUMMARY_FK: &str = "pending_auth_details_summary_fk";

    /// An approval at day `yyddd`, time `hhmmssmmm`, keyed as CP00 keys it: both parts inverted
    /// (COPAUA0C-098).
    fn detail(yyddd: u64, hhmmssmmm: u64) -> Result<PendingAuthDetailRecord, Box<dyn StdError>> {
        Ok(PendingAuthDetailRecord {
            auth_date_9c: PicS::complement_from(yyddd)?,
            auth_time_9c: PicS::complement_from(hhmmssmmm)?,
            orig_date: PicX::from_text("250101")?,
            orig_time: PicX::from_text("123456")?,
            card_num: CardNumber::parse(CARD)?,
            auth_type: PicX::from_text("AUTH")?,
            card_expiry_date: PicX::from_text("1227")?,
            message_type: PicX::from_text("0100")?,
            message_source: PicX::from_text("POS")?,
            auth_id_code: PicX::from_text("123456")?,
            auth_resp_code: PicX::from_text("00")?,
            auth_resp_reason: PicX::from_text("0000")?,
            processing_code: PicU::new(3_000)?,
            transaction_amt: Money::new(Decimal::new(12_345, 2))?,
            approved_amt: Money::new(Decimal::new(12_345, 2))?,
            merchant_catagory_code: PicX::from_text("5411")?,
            acqr_country_code: PicX::from_text("840")?,
            pos_entry_mode: PicU::new(5)?,
            merchant_id: PicX::from_text("MERCH0000000001")?,
            merchant_name: PicX::from_text("CORNER GROCERY")?,
            merchant_city: PicX::from_text("SEATTLE")?,
            merchant_state: PicX::from_text("WA")?,
            merchant_zip: PicX::from_text("98101")?,
            transaction_id: PicX::from_text("TXN000000000001")?,
            match_status: PicX::from_text("P")?,
            auth_fraud: PicX::from_text(" ")?,
            fraud_rpt_date: PicX::spaces(),
            filler: PicX::spaces(),
        })
    }

    /// The summary root a detail needs, with zero amounts and counts as CP00 creates it
    /// (COPAUA0C-092).
    async fn seed_summary(pool: &PgPool, acct: i64) -> TestResult {
        sqlx::query!(
            "INSERT INTO pending_auth_summaries (acct_id, cust_id, auth_status, account_status_1, \
                 account_status_2, account_status_3, account_status_4, account_status_5, \
                 credit_limit, cash_limit, credit_balance, cash_balance, approved_auth_cnt, \
                 declined_auth_cnt, approved_auth_amt, declined_auth_amt, filler) \
             VALUES ($1, 50, ' ', ' ', ' ', ' ', ' ', ' ', 0, 0, 0, 0, 0, 0, 0, 0, ' ')",
            acct,
        )
        .execute(pool)
        .await?;
        Ok(())
    }

    /// Cardholder Data Redaction: an error names neither the card number nor its last digits.
    fn assert_redacted(error: &PersistenceError) {
        for text in [error.to_string(), format!("{error:?}")] {
            assert!(!text.contains(CARD) && !text.contains(LAST_FOUR), "{text}");
        }
    }

    // audit: COPAUS0C-031
    // audit: COPAUS0C-047
    // audit: COPAUS1C-008
    // audit: COPAUS1C-024
    // audit: CBPAUP0C-010
    #[test]
    fn reads_follow_the_twin_order_from_an_inclusive_start() -> TestResult {
        block_on(async {
            let guard = test_support::postgres::start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            let (acct, other) = (PicS::<11>::new(ACCT)?, PicS::<11>::new(OTHER_ACCT)?);
            for id in [ACCT, OTHER_ACCT] {
                seed_summary(guard.pool(), id).await?;
            }

            // Two authorizations on one day and one the day before; newest first is twin order.
            let oldest = detail(25_001, 235_959_999)?;
            let middle = detail(25_002, 93_015_123)?;
            let newest = detail(25_002, 101_500_000)?;
            for record in [&middle, &oldest, &newest] {
                store.insert(acct, record).await?;
            }
            // The same key under another account is another segment.
            store.insert(other, &middle).await?;

            let (first_date, first_time) =
                (PicS::<5>::new(-99_999)?, PicS::<9>::new(-999_999_999)?);
            let everything = vec![newest.clone(), middle.clone(), oldest.clone()];
            assert_eq!(
                store.page_after(acct, first_date, first_time, 10).await?,
                everything
            );
            assert_eq!(store.find_all_by_acct_id(acct).await?, everything);
            assert_eq!(
                store.find_all_by_acct_id(other).await?,
                vec![middle.clone()]
            );

            // The start key is included; the limit is bound as given.
            let (date, time) = (middle.auth_date_9c, middle.auth_time_9c);
            let from_middle = vec![middle.clone(), oldest.clone()];
            assert_eq!(store.page_after(acct, date, time, 5).await?, from_middle);
            assert_eq!(
                store.page_after(acct, date, time, 1).await?,
                vec![middle.clone()]
            );
            assert!(store.page_after(acct, date, time, 0).await?.is_empty());
            // From the current detail with a limit of 2: the detail and the next one, as F8 reads.
            let (newest_date, newest_time) = (newest.auth_date_9c, newest.auth_time_9c);
            assert_eq!(
                store.page_after(acct, newest_date, newest_time, 2).await?,
                vec![newest.clone(), middle.clone()]
            );
            // Past the oldest key nothing is left.
            let past = PicS::<9>::new(oldest.auth_time_9c.get() + 1)?;
            assert!(
                store
                    .page_after(acct, oldest.auth_date_9c, past, 5)
                    .await?
                    .is_empty()
            );

            assert_eq!(
                store.find_by_id(acct, date, time).await?,
                Some(middle.clone())
            );
            assert_eq!(store.find_by_id(acct, date, past).await?, None);
            let missing = PicS::<11>::new(52)?;
            assert_eq!(store.find_by_id(missing, date, time).await?, None);
            assert!(store.find_all_by_acct_id(missing).await?.is_empty());

            // A negative limit is the server's error, never a silent empty page.
            let refused = store.page_after(acct, date, time, -1).await;
            let error = refused.err().ok_or("a negative limit was accepted")?;
            assert_eq!(error.sqlstate(), Some("2201W"), "{error:?}");
            Ok(())
        })
    }

    // audit: COPAUA0C-099
    // audit: COPAUA0C-120
    #[test]
    fn a_failed_insert_inside_a_savepoint_leaves_the_transaction_usable() -> TestResult {
        block_on(async {
            let guard = test_support::postgres::start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            let acct = PicS::<11>::new(ACCT)?;
            seed_summary(guard.pool(), ACCT).await?;

            let first = detail(25_040, 120_000_000)?;
            // A second authorization in the same millisecond: the same key, other fields.
            let mut second = first.clone();
            second.auth_resp_code = PicX::from_text("05")?;
            second.match_status = PicX::from_text("D")?;

            let mut tx = store.begin().await?;
            tx.insert(acct, &first).await?;
            let mut savepoint = tx.savepoint().await?;
            let duplicate = savepoint.insert(acct, &second).await;
            let error = duplicate.err().ok_or("a duplicate key was stored")?;
            assert_eq!(
                error,
                PersistenceError::Duplicate {
                    constraint: Some(PRIMARY_KEY.to_owned())
                }
            );
            assert_redacted(&error);
            savepoint.rollback().await?;

            // The enclosing transaction goes on, and commits the first detail alone.
            let (date, time) = (first.auth_date_9c, first.auth_time_9c);
            assert_eq!(tx.find_by_id(acct, date, time).await?, Some(first.clone()));
            assert_eq!(tx.find_all_by_acct_id(acct).await?, vec![first.clone()]);
            assert_eq!(
                tx.page_after(acct, date, time, 5).await?,
                vec![first.clone()]
            );
            tx.commit().await?;
            assert_eq!(store.find_all_by_acct_id(acct).await?, vec![first.clone()]);

            // On the pool, the same duplicate fails on its own statement and stores nothing.
            let again = store.insert(acct, &second).await;
            assert!(again.as_ref().is_err_and(PersistenceError::is_duplicate));
            assert_eq!(store.find_by_id(acct, date, time).await?, Some(first));

            // A detail whose account has no summary is refused by the foreign key.
            let orphan = PicS::<11>::new(OTHER_ACCT)?;
            let refused = store.insert(orphan, &second).await;
            let error = refused
                .err()
                .ok_or("a detail without its summary was stored")?;
            assert_eq!(
                error,
                PersistenceError::Restrict {
                    constraint: Some(SUMMARY_FK.to_owned())
                }
            );
            assert_redacted(&error);
            assert!(store.find_all_by_acct_id(orphan).await?.is_empty());
            Ok(())
        })
    }

    // audit: COPAUS1C-032
    // audit: CBPAUP0C-015
    #[test]
    fn update_and_delete_report_whether_exactly_one_row_changed() -> TestResult {
        block_on(async {
            let guard = test_support::postgres::start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            let (acct, other) = (PicS::<11>::new(ACCT)?, PicS::<11>::new(OTHER_ACCT)?);
            for id in [ACCT, OTHER_ACCT] {
                seed_summary(guard.pool(), id).await?;
            }
            let stored = detail(25_100, 80_000_500)?;
            let neighbour = detail(25_100, 80_000_501)?;
            store.insert(acct, &stored).await?;
            store.insert(acct, &neighbour).await?;
            let (date, time) = (stored.auth_date_9c, stored.auth_time_9c);

            // Fraud reported: every non-key field is replaced, the key kept.
            let mut reported = stored.clone();
            reported.auth_fraud = PicX::from_text("F")?;
            reported.fraud_rpt_date = PicX::from_text("07/18/22")?;
            reported.filler = PicX::from_array(*b"~FILLER{}  bytes!");
            assert!(store.update(acct, &reported).await?);
            assert_eq!(
                store.find_by_id(acct, date, time).await?,
                Some(reported.clone())
            );
            assert_eq!(
                store
                    .find_by_id(acct, neighbour.auth_date_9c, neighbour.auth_time_9c)
                    .await?,
                Some(neighbour.clone())
            );

            // No row under that key: nothing replaced, nothing stored.
            assert!(!store.update(other, &reported).await?);
            assert!(store.find_all_by_acct_id(other).await?.is_empty());
            let mut unknown = reported.clone();
            unknown.auth_time_9c = PicS::new(time.get() - 2)?;
            assert!(!store.update(acct, &unknown).await?);

            // A replace inside a transaction that rolls back leaves the stored detail as it was.
            let mut tx = store.begin().await?;
            assert!(tx.update(acct, &stored).await?);
            assert_eq!(tx.find_by_id(acct, date, time).await?, Some(stored.clone()));
            tx.rollback().await?;
            assert_eq!(store.find_by_id(acct, date, time).await?, Some(reported));

            // A delete inside a transaction holds only once committed.
            let mut tx = store.begin().await?;
            assert!(tx.delete(acct, date, time).await?);
            assert!(!tx.delete(acct, date, time).await?);
            tx.commit().await?;
            assert_eq!(store.find_by_id(acct, date, time).await?, None);
            assert!(!store.delete(acct, date, time).await?);
            assert!(
                !store
                    .delete(other, neighbour.auth_date_9c, neighbour.auth_time_9c)
                    .await?
            );
            assert_eq!(store.find_all_by_acct_id(acct).await?, vec![neighbour]);
            Ok(())
        })
    }

    /// `lock_by_id` on a transaction holds the detail until the transaction ends: a second
    /// transaction's `lock_by_id` waits for it, a plain read does not, and the waiter, once the
    /// holder commits, reads the flag the holder stored, as the next F5 on the detail must. On
    /// the pool the lock ends with the call, and a key no detail holds is `None`.
    // audit: COPAUS1C-028
    #[test]
    fn lock_by_id_holds_the_detail_until_the_transaction_ends() -> TestResult {
        block_on(async {
            let guard = test_support::postgres::start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            let acct = PicS::<11>::new(ACCT)?;
            seed_summary(guard.pool(), ACCT).await?;
            let stored = detail(25_200, 101_010_101)?;
            store.insert(acct, &stored).await?;
            let (date, time) = (stored.auth_date_9c, stored.auth_time_9c);
            let absent = PicS::<9>::new(time.get() + 1)?;

            // Through the pool the lock ends with its statement, so a second one does not wait.
            assert_eq!(
                store.lock_by_id(acct, date, time).await?,
                Some(stored.clone())
            );
            assert_eq!(
                store.lock_by_id(acct, date, time).await?,
                Some(stored.clone())
            );
            assert_eq!(store.lock_by_id(acct, date, absent).await?, None);

            let mut holder = store.begin().await?;
            assert_eq!(holder.lock_by_id(acct, date, absent).await?, None);
            let mut reported = holder
                .lock_by_id(acct, date, time)
                .await?
                .ok_or("the detail is stored")?;
            assert_eq!(reported, stored);
            // A plain read does not wait for the lock.
            assert_eq!(
                store.find_by_id(acct, date, time).await?,
                Some(stored.clone())
            );

            // Another transaction's locked read waits while the holder's transaction is open.
            let mut waiter = store.begin().await?;
            let seen = {
                let mut waiting = pin!(waiter.lock_by_id(acct, date, time));
                assert!(
                    timeout(LOCK_WAIT, waiting.as_mut()).await.is_err(),
                    "a second lock of the detail was granted while the first was held"
                );
                // The holder flags the detail and commits, which releases the lock: the waiter then
                // reads the detail as the holder committed it, not as it was when the wait began.
                reported.auth_fraud = PicX::from_text("F")?;
                reported.fraud_rpt_date = PicX::from_text("07/18/22")?;
                assert!(holder.update(acct, &reported).await?);
                holder.commit().await?;
                waiting.await?
            };
            assert_eq!(seen, Some(reported.clone()));
            waiter.rollback().await?;
            assert_eq!(store.find_by_id(acct, date, time).await?, Some(reported));
            Ok(())
        })
    }
}
