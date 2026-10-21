//! `TransactionRepository`: the reads and the insert of table `transactions` (migration 0002,
//! `CVTRA05Y` `TRAN-RECORD`, the KSDS `KEYS(16 0)` of `TRANFILE.jcl:53`; CVTRA05Y-001 to
//! CVTRA05Y-014), and [`next_id`](TransactionRepository::next_id), the single highest-plus-one
//! allocator of transaction IDs, shared by transaction add and bill payment (endpoints 13 and
//! 15; AAP 0.5.4).
//!
//! | Method | COBOL access | Statement | Served by |
//! |---|---|---|---|
//! | `find_by_id` | COTRN01C `READ-TRANSACT-FILE` by `TRAN-ID` (`COTRN01C.cbl:267-295`; COTRN01C-011, COTRN01C-012) | `WHERE tran_id = $1::bpchar`, no lock | `transactions_pkey` |
//! | `find_last` | COTRN02C `COPY-LAST-TRAN-DATA`: `STARTBR` at `HIGH-VALUES`, `READPREV`, `ENDBR` (`COTRN02C.cbl:471-480,642-706`; COTRN02C-053) | `ORDER BY tran_id DESC LIMIT 1` | `transactions_pkey`, backward |
//! | `next_id` | the same browse, then `MOVE TRAN-ID TO WS-TRAN-ID-N` and `ADD 1` (`COTRN02C.cbl:442-449,689`; `COBIL00C.cbl:212-219,488`; COTRN02C-043, COBIL00C-023) | `max(tran_id)` | `transactions_pkey` |
//! | `insert` | `WRITE-TRANSACT-FILE` of COTRN02C and COBIL00C (`COTRN02C.cbl:711-741`, `COBIL00C.cbl:510-539`; COTRN02C-050, COBIL00C-029) and CBTRN02C `2900-WRITE-TRANSACTION-FILE` (`CBTRN02C.cbl:562-579`; CBTRN02C-024, CBTRN02C-025) | `INSERT` of every column | — |
//! | `page_after` | COTRN00C `PROCESS-PAGE-FORWARD`: `STARTBR` (`GTEQ`), `READNEXT` (`COTRN00C.cbl:279-328,591-653`; COTRN00C-014) | `queries/transactions_page_forward.sql` | `transactions_pkey` |
//! | `page_before` | COTRN00C `PROCESS-PAGE-BACKWARD`: `STARTBR`, `READPREV` (`COTRN00C.cbl:333-375,591-619,658-687`; COTRN00C-023) | `queries/transactions_page_backward.sql` | `transactions_pkey`, backward |
//! | `find_all_in_card_order` | CREASTMT `STEP010` `SORT FIELDS=(263,16,CH,A,1,16,CH,A)` (`CREASTMT.JCL:53`; CBSTM03A-001) | `ORDER BY card_num, tran_id` | `transactions_card_num_idx` |
//! | `find_processed_between` | TRANREPT `INCLUDE COND` on `TRAN-PROC-DT`, then `SORT FIELDS=(TRAN-CARD-NUM,A)` (`TRANREPT.jcl:41-48`, `TRANREPT.prc:39-46`; CBTRN03C-004, CBTRN03C-005) | the `proc_ts` window, `ORDER BY card_num, tran_id` | `transactions_proc_ts_idx` |
//!
//! Every method returns domain values: rows pass through codec B
//! ([`TransactionRow::from_record`] and [`TransactionRow::into_record`]) and no byte is trimmed,
//! padded or normalised here. Each statement is written once, in a private function over any
//! executor, and both [`PgStore`] (pool, autocommit) and [`PgTx`] (the caller's transaction)
//! run it. Every column is `COLLATE "C"`, so key comparisons and orders are byte order, which
//! is the numeric order of the 16-digit IDs every in-scope writer stores (COTRN02C-043).
//!
//! # Redaction (Cardholder Data Redaction)
//!
//! The card number reaches its bind parameter only inside the [`TransactionRow`] that
//! `from_record` built through `CardNumber::expose`. No record or row is formatted into an
//! error, and a failed allocation names the column alone, never the stored value.

use chrono::NaiveDate;
use copybook::{PicU, PicX};
use domain::records::TransactionRecord;
use sqlx::PgExecutor;

use crate::error::{DecodeKind, PersistenceError};
use crate::rows::{TransactionRow, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column `tran_id`, `TRAN-ID PIC X(16)` (CVTRA05Y-001): the key every keyed statement binds
/// and the column a failed allocation names.
const TRAN_ID: &str = "tran_id";

/// The digits of `WS-TRAN-ID-N PIC 9(16)` (`COTRN02C.cbl:57`) and `WS-TRAN-ID-NUM PIC 9(16)`
/// (`COBIL00C.cbl:57`), which the allocation moves back into `TRAN-ID`.
const ID_DIGITS: usize = 16;

/// Table `transactions`: the transaction master the online programs, `posttran`, `creastmt`
/// and `tranrept` read and write (AAP 0.5.4).
///
/// Implemented by [`PgStore`], where each call runs and commits on its own pooled connection,
/// and by [`PgTx`], where each call runs inside the caller's open transaction. Test doubles in
/// other crates implement it too.
pub trait TransactionRepository: Send {
    /// The transaction whose `TRAN-ID` is `id`, or `None` when no row has it: COTRN01C's
    /// `READ` by key, whose `NOTFND` the caller answers with `Transaction ID NOT found...`
    /// (`COTRN01C.cbl:269-288`; COTRN01C-012).
    ///
    /// The COBOL reads with `UPDATE` but rewrites nothing, so the replacement reads without a
    /// lock (COTRN01C-011).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] with [`DecodeKind::InvalidByte`] when `id` holds a NUL or
    /// non-ASCII byte, which no `CHAR(16)` value can match; [`PersistenceError::Decode`] when
    /// the stored row does not fit its pictures; [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] when the read fails, which the caller answers with
    /// `Unable to lookup Transaction...` (COTRN01C-013).
    fn find_by_id(
        &mut self,
        id: &PicX<16>,
    ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send;

    /// The transaction with the highest `TRAN-ID` in the whole table, whatever its card, or
    /// `None` on an empty table: the `STARTBR` at `HIGH-VALUES`, `READPREV` and `ENDBR` that
    /// COTRN02C's PF5 copies from (`COTRN02C.cbl:471-480,642-706`; COTRN02C-053).
    ///
    /// On an empty table the COBOL outcome is halt candidate X-COTRN02C-H1; the caller answers
    /// `None` with `Transaction ID NOT found...` and claims no parity.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when the stored row does not fit its pictures;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read fails,
    /// which the caller answers with `Unable to lookup Transaction...` (COTRN02C-044).
    fn find_last(
        &mut self,
    ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send;

    /// The next transaction ID: the highest stored `TRAN-ID` read as `PIC 9(16)`, plus 1,
    /// moved back as 16 digits with leading zeros (`COTRN02C.cbl:442-450`,
    /// `COBIL00C.cbl:212-219`; COTRN02C-043, COBIL00C-023).
    ///
    /// - `ADD 1` has no `ON SIZE ERROR`, so sixteen `9`s are followed by sixteen `0`s: the
    ///   high-order digit is dropped by the COBOL store rule (AAP 0.3.3.2).
    /// - On an empty table, `READPREV` at end of file moves `ZEROS` to `TRAN-ID`
    ///   (`COTRN02C.cbl:689`, `COBIL00C.cbl:488`), so the ID is `0000000000000001`. Whether
    ///   CICS ends that browse at `STARTBR` instead is halt candidate X-COTRN02C-H1 and
    ///   X-COBIL00C-H3: the replacement takes the end-of-file branch and claims no parity
    ///   (COTRN02C-044, COBIL00C-024). A caller that must tell the empty table apart asks
    ///   [`find_last`](Self::find_last); this method stays the pure allocator.
    /// - Allocation takes no lock, exactly as the CICS browse takes none: two concurrent adds
    ///   or payments can compute the same ID, and the second [`insert`](Self::insert) fails with
    ///   [`PersistenceError::Duplicate`], which the caller answers with `Tran ID already
    ///   exist...` (COTRN02C-050, COBIL00C-029). On a [`PgTx`] the read sees the transaction's
    ///   own uncommitted inserts.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] with [`DecodeKind::NotNumeric`] on column `tran_id` when
    /// the highest stored ID is not 16 ASCII digits. No in-scope writer stores such an ID, so
    /// it is an anomaly to escalate, never a value to normalise (R7); the error carries the
    /// column name only. [`PersistenceError::Unavailable`] or [`PersistenceError::Database`]
    /// when the read fails (COTRN02C-044, COBIL00C-024).
    fn next_id(&mut self) -> impl Future<Output = Result<PicX<16>, PersistenceError>> + Send;

    /// Inserts `record` under its `TRAN-ID`, every column bound from codec B: the typed
    /// timestamps with their layout codes, the card number through the row (`WRITE` of
    /// `COTRN02C.cbl:711-741` and `COBIL00C.cbl:510-539`; `CBTRN02C.cbl:562-579`, CBTRN02C-024).
    /// The insert maintains `transactions_proc_ts_idx` and `transactions_card_num_idx`, as
    /// `UPGRADE` keeps the alternate index current.
    ///
    /// On a [`PgTx`] a failed insert aborts the whole transaction, so a caller that must go on
    /// after it runs the insert inside [`PgTx::savepoint`] and rolls back to the savepoint.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] (constraint `transactions_pkey`) when a row already has
    /// the ID: CICS `DUPKEY`/`DUPREC`, `Tran ID already exist...` (COTRN02C-050, COBIL00C-029),
    /// and file status `22` for `posttran` (CBTRN02C-025). [`PersistenceError::Check`] when a
    /// range or layout check refuses a value; [`PersistenceError::Decode`] with
    /// [`DecodeKind::InvalidByte`] when a `PIC X(n)` field holds a NUL or non-ASCII byte, before
    /// any statement runs; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`]
    /// when the write fails.
    fn insert(
        &mut self,
        record: &TransactionRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Up to `limit` transactions whose `TRAN-ID` is at or above `start`, ascending: the
    /// `STARTBR` (`GTEQ`) and `READNEXT` loop of COTRN00C's forward page
    /// (`COTRN00C.cbl:279-328,591-653`; COTRN00C-014).
    ///
    /// The start is inclusive, as the record `STARTBR` positions on is the first `READNEXT`
    /// returns. Blank `start` ranks below every ID, because `CHAR(16)` ignores trailing
    /// spaces, and is the low-values key of a first page (COTRN00C-011). `limit` is bound
    /// verbatim: the caller passes the rows it reads, the page and the read-ahead, after the
    /// API bounded `page_size` to 1..=100 (Bounded Pagination).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] with [`DecodeKind::InvalidByte`] when `start` holds a NUL or
    /// non-ASCII byte, such as `HIGH-VALUES`; [`PersistenceError::Decode`] when a stored row
    /// does not fit its pictures; [`PersistenceError::Database`] for a negative `limit`;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read fails
    /// (COTRN00C-027).
    fn page_after(
        &mut self,
        start: &PicX<16>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send;

    /// Up to `limit` transactions whose `TRAN-ID` is at or below `start`, descending: the
    /// `STARTBR` and `READPREV` loop of COTRN00C's backward page
    /// (`COTRN00C.cbl:333-375,591-619,658-687`; COTRN00C-023).
    ///
    /// The start is inclusive, as a `READPREV` straight after `STARTBR` returns the record it
    /// positioned on; the caller drops it where the program's skip `READPREV` does and fills
    /// the rows bottom up. `limit` is bound verbatim, as for
    /// [`page_after`](Self::page_after).
    ///
    /// # Errors
    ///
    /// As [`page_after`](Self::page_after).
    fn page_before(
        &mut self,
        start: &PicX<16>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send;

    /// Every transaction, by card number, then by transaction ID: the order CREASTMT's
    /// `STEP010` sorts `TRANSACT` into for CBSTM03A (`CREASTMT.JCL:53`; CBSTM03A-001).
    ///
    /// The order is byte order. The program's EBCDIC order (X-CREASTMT-3) is the caller's to
    /// apply; for the digit-only keys every in-scope writer stores, the two agree. The read is
    /// unbounded and serves the batch job only.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a stored row does not fit its pictures;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read fails
    /// (CBSTM03A-012).
    fn find_all_in_card_order(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send;

    /// The transactions processed on `start` through `end`, both days included, by card
    /// number, then by transaction ID: TRANREPT's `INCLUDE COND=(TRAN-PROC-DT,GE,…,AND,
    /// TRAN-PROC-DT,LE,…)` and `SORT FIELDS=(TRAN-CARD-NUM,A)` (`TRANREPT.jcl:41-48`,
    /// `TRANREPT.prc:39-46`; CBTRN03C-004, CBTRN03C-005).
    ///
    /// - The `INCLUDE` compares `TRAN-PROC-TS(1:10)` as characters. Every stored layout, `I`,
    ///   `D` and `S`, begins with the `YYYY-MM-DD` date (PIC-018 to PIC-020), so comparing the
    ///   typed date of `proc_ts` gives the text comparison's result. A layout added later must
    ///   be checked against this argument before it is admitted.
    /// - A blank `TRAN-PROC-TS` is a `NULL` `proc_ts` (PIC-017). Its spaces compare below every
    ///   date, so it is never selected, and no `NULL` row is returned.
    /// - The window is written as `proc_ts` at or after `start` and before the day after `end`,
    ///   so that the comparison stays on `transactions_proc_ts_idx`.
    /// - Rows of one card come in transaction-ID order, the order of the KSDS unload. The
    ///   `SORT` has no `EQUALS`, so the COBOL fixes no such order: halt candidate X-TRANREPT-H1,
    ///   whose proposed resolution this order is, and which claims no parity.
    /// - A start date after the end date selects nothing.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a stored row does not fit its pictures;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read fails
    /// (CBTRN03C-010).
    fn find_processed_between(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send;
}

impl TransactionRepository for PgStore {
    fn find_by_id(
        &mut self,
        id: &PicX<16>,
    ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send {
        select_by_id(self.pool(), id)
    }

    fn find_last(
        &mut self,
    ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send {
        select_last(self.pool())
    }

    fn next_id(&mut self) -> impl Future<Output = Result<PicX<16>, PersistenceError>> + Send {
        allocate_id(self.pool())
    }

    fn insert(
        &mut self,
        record: &TransactionRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_row(self.pool(), record)
    }

    fn page_after(
        &mut self,
        start: &PicX<16>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_page_forward(self.pool(), start, limit)
    }

    fn page_before(
        &mut self,
        start: &PicX<16>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_page_backward(self.pool(), start, limit)
    }

    fn find_all_in_card_order(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_in_card_order(self.pool())
    }

    fn find_processed_between(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_processed_between(self.pool(), start, end)
    }
}

impl TransactionRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        id: &PicX<16>,
    ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send {
        select_by_id(self.conn(), id)
    }

    fn find_last(
        &mut self,
    ) -> impl Future<Output = Result<Option<TransactionRecord>, PersistenceError>> + Send {
        select_last(self.conn())
    }

    fn next_id(&mut self) -> impl Future<Output = Result<PicX<16>, PersistenceError>> + Send {
        allocate_id(self.conn())
    }

    fn insert(
        &mut self,
        record: &TransactionRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_row(self.conn(), record)
    }

    fn page_after(
        &mut self,
        start: &PicX<16>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_page_forward(self.conn(), start, limit)
    }

    fn page_before(
        &mut self,
        start: &PicX<16>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_page_backward(self.conn(), start, limit)
    }

    fn find_all_in_card_order(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_in_card_order(self.conn())
    }

    fn find_processed_between(
        &mut self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> impl Future<Output = Result<Vec<TransactionRecord>, PersistenceError>> + Send {
        select_processed_between(self.conn(), start, end)
    }
}

/// The records of `rows`, in their order, each through codec B.
fn into_records(rows: Vec<TransactionRow>) -> Result<Vec<TransactionRecord>, PersistenceError> {
    rows.into_iter().map(TransactionRow::into_record).collect()
}

/// `find_by_id`: the row keyed `id`, if any.
async fn select_by_id<'e, E: PgExecutor<'e>>(
    executor: E,
    id: &PicX<16>,
) -> Result<Option<TransactionRecord>, PersistenceError> {
    let key = pic_x_to_string(TRAN_ID, id)?;
    // The key text is cast to `bpchar`: `sqlx` binds a `String` as `text`, against which the
    // `CHAR(n)` column compares as `text`, its trailing spaces dropped, so a key that ends in
    // spaces would find no row and the primary key could not serve the statement.
    let row = sqlx::query_as!(
        TransactionRow,
        r#"SELECT tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt,
                  tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip,
                  card_num, orig_ts, orig_ts_form, proc_ts, proc_ts_form, filler
           FROM transactions
           WHERE tran_id = $1::bpchar"#,
        key
    )
    .fetch_optional(executor)
    .await?;
    row.map(TransactionRow::into_record).transpose()
}

/// `find_last`: the row with the highest `tran_id`, if any.
async fn select_last<'e, E: PgExecutor<'e>>(
    executor: E,
) -> Result<Option<TransactionRecord>, PersistenceError> {
    let row = sqlx::query_as!(
        TransactionRow,
        r#"SELECT tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt,
                  tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip,
                  card_num, orig_ts, orig_ts_form, proc_ts, proc_ts_form, filler
           FROM transactions
           ORDER BY tran_id DESC
           LIMIT 1"#
    )
    .fetch_optional(executor)
    .await?;
    row.map(TransactionRow::into_record).transpose()
}

/// `next_id`: the highest `tran_id` plus 1, as `ADD-TRANSACTION` and `PROCESS-ENTER-KEY`
/// compute it (COTRN02C-043, COBIL00C-023).
async fn allocate_id<'e, E: PgExecutor<'e>>(executor: E) -> Result<PicX<16>, PersistenceError> {
    let highest = sqlx::query_scalar!(r#"SELECT max(tran_id) AS "max_id?" FROM transactions"#)
        .fetch_one(executor)
        .await?;
    let current = match highest {
        // `MOVE TRAN-ID TO WS-TRAN-ID-N`: the stored 16 bytes read as `PIC 9(16)`. Anything but
        // 16 digits is refused, never read as a number (R7).
        Some(text) => PicU::<16>::decode(text.as_bytes())
            .map_err(|_| PersistenceError::decode(TRAN_ID, DecodeKind::NotNumeric))?,
        // HALTED(X-COTRN02C-H1, X-COBIL00C-H3): the empty table takes the `READPREV` end-of-file
        // branch, `MOVE ZEROS TO TRAN-ID` (`COTRN02C.cbl:689`, `COBIL00C.cbl:488`); no parity
        // is claimed for it (COTRN02C-044, COBIL00C-024).
        None => PicU::ZERO,
    };
    // TRUNCATION: COBOL-mandated. `ADD 1 TO WS-TRAN-ID-N` stores into `PIC 9(16)` with no
    // `ON SIZE ERROR`, so the carry out of sixteen `9`s is dropped and the ID wraps to sixteen
    // `0`s (COTRN02C-043, COBIL00C-023; AAP 0.3.3.2). `succ` fails only at that maximum, so the
    // fallback is exactly the wrapped value; every other ID is incremented by `succ` itself.
    let next = current.succ().unwrap_or(PicU::ZERO);
    render_id(next)
}

/// `MOVE WS-TRAN-ID-N TO TRAN-ID`: the 16 digits, leading zeros kept, as the `PIC X(16)` key.
fn render_id(id: PicU<16>) -> Result<PicX<16>, PersistenceError> {
    let mut digits = Vec::with_capacity(ID_DIGITS);
    id.encode_into(&mut digits);
    PicX::decode(&digits).map_err(|kind| PersistenceError::decode(TRAN_ID, DecodeKind::from(kind)))
}

/// `insert`: one row holding every column of `record`.
async fn insert_row<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &TransactionRecord,
) -> Result<(), PersistenceError> {
    let row = TransactionRow::from_record(record)?;
    sqlx::query!(
        r#"INSERT INTO transactions (
               tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt,
               tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip,
               card_num, orig_ts, orig_ts_form, proc_ts, proc_ts_form, filler)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)"#,
        row.tran_id,
        row.tran_type_cd,
        row.tran_cat_cd,
        row.tran_source,
        row.tran_desc,
        row.tran_amt,
        row.tran_merchant_id,
        row.tran_merchant_name,
        row.tran_merchant_city,
        row.tran_merchant_zip,
        row.card_num,
        row.orig_ts,
        row.orig_ts_form,
        row.proc_ts,
        row.proc_ts_form,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// `page_after`: the forward page query file, shared with the `explain` test.
async fn select_page_forward<'e, E: PgExecutor<'e>>(
    executor: E,
    start: &PicX<16>,
    limit: i64,
) -> Result<Vec<TransactionRecord>, PersistenceError> {
    let key = pic_x_to_string(TRAN_ID, start)?;
    let rows = sqlx::query_file_as!(
        TransactionRow,
        "queries/transactions_page_forward.sql",
        key,
        limit
    )
    .fetch_all(executor)
    .await?;
    into_records(rows)
}

/// `page_before`: the backward page query file, shared with the `explain` test.
async fn select_page_backward<'e, E: PgExecutor<'e>>(
    executor: E,
    start: &PicX<16>,
    limit: i64,
) -> Result<Vec<TransactionRecord>, PersistenceError> {
    let key = pic_x_to_string(TRAN_ID, start)?;
    let rows = sqlx::query_file_as!(
        TransactionRow,
        "queries/transactions_page_backward.sql",
        key,
        limit
    )
    .fetch_all(executor)
    .await?;
    into_records(rows)
}

/// `find_all_in_card_order`: every row, by card number, then transaction ID.
async fn select_in_card_order<'e, E: PgExecutor<'e>>(
    executor: E,
) -> Result<Vec<TransactionRecord>, PersistenceError> {
    let rows = sqlx::query_as!(
        TransactionRow,
        r#"SELECT tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt,
                  tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip,
                  card_num, orig_ts, orig_ts_form, proc_ts, proc_ts_form, filler
           FROM transactions
           ORDER BY card_num, tran_id"#
    )
    .fetch_all(executor)
    .await?;
    into_records(rows)
}

/// `find_processed_between`: the rows whose processing date lies in `start..=end`, by card
/// number, then transaction ID.
async fn select_processed_between<'e, E: PgExecutor<'e>>(
    executor: E,
    start: NaiveDate,
    end: NaiveDate,
) -> Result<Vec<TransactionRecord>, PersistenceError> {
    let rows = sqlx::query_as!(
        TransactionRow,
        r#"SELECT tran_id, tran_type_cd, tran_cat_cd, tran_source, tran_desc, tran_amt,
                  tran_merchant_id, tran_merchant_name, tran_merchant_city, tran_merchant_zip,
                  card_num, orig_ts, orig_ts_form, proc_ts, proc_ts_form, filler
           FROM transactions
           WHERE proc_ts >= $1::date AND proc_ts < ($2::date + 1)
           ORDER BY card_num, tran_id"#,
        start,
        end
    )
    .fetch_all(executor)
    .await?;
    into_records(rows)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use chrono::NaiveDate;
    use copybook::{FixedRecord, PicX};
    use domain::records::TransactionRecord;

    use super::TransactionRepository;
    use crate::PgStore;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Card numbers in no fixture file, every `cardxref.txt` number starting with `0`: no error
    /// may repeat them.
    const CARD_A: &[u8; 16] = b"9900000000000001";
    const CARD_B: &[u8; 16] = b"9900000000000002";

    /// A blank `TRAN-PROC-TS`, a `NULL` `proc_ts` (PIC-017).
    const BLANK_TS: &[u8; 26] = &[b' '; 26];

    /// `text` followed by spaces to `N` bytes.
    fn padded<const N: usize>(text: &[u8]) -> Vec<u8> {
        let mut field = text.to_vec();
        field.resize(N, b' ');
        field
    }

    /// A 350-byte `TRAN-RECORD` with this ID, card number and processing timestamp, its other
    /// fields shaped as `dailytran.txt` line 1, decoded by codec A.
    fn record(
        id: &[u8; 16],
        card: &[u8; 16],
        proc_ts: &[u8; 26],
    ) -> Result<TransactionRecord, Box<dyn StdError>> {
        let fields: [&[u8]; 14] = [
            id,
            b"01",
            b"0001",
            &padded::<10>(b"POS TERM"),
            &padded::<100>(b"Purchase at Abshire-Lowe"),
            b"0000005047G",
            b"800000000",
            &padded::<50>(b"Abshire-Lowe"),
            &padded::<50>(b"North Enoshaven"),
            &padded::<10>(b"72112"),
            card,
            b"2022-06-10 19:27:53.000000",
            proc_ts,
            &[b' '; 20],
        ];
        Ok(TransactionRecord::decode(&fields.concat())?)
    }

    /// The `PIC X(16)` key holding `text`.
    fn key(text: &[u8; 16]) -> PicX<16> {
        PicX::from_array(*text)
    }

    /// The IDs of `records`, in their order.
    fn ids(records: &[TransactionRecord]) -> Vec<[u8; 16]> {
        records.iter().map(|record| *record.id.as_bytes()).collect()
    }

    /// The calendar day `(year, month, day)`.
    fn day(year: i32, month: u32, day: u32) -> Result<NaiveDate, &'static str> {
        NaiveDate::from_ymd_opt(year, month, day).ok_or("no such day")
    }

    // audit: COTRN02C-043
    // audit: COTRN02C-044
    // audit: COTRN02C-050
    // audit: COTRN02C-053
    // audit: COBIL00C-023
    // audit: COBIL00C-024
    // audit: COBIL00C-029
    // audit: COTRN01C-011
    // audit: COTRN01C-012
    // audit: CBTRN02C-024
    #[tokio::test]
    async fn store_allocates_reads_back_and_refuses_a_duplicate_id() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());

        // The empty table takes the end-of-file branch: ID 1, and nothing to read.
        assert_eq!(store.next_id().await?, key(b"0000000000000001"));
        assert!(store.find_last().await?.is_none());
        assert!(store.find_by_id(&key(b"0000000000000001")).await?.is_none());

        let five = record(b"0000000000000005", CARD_B, BLANK_TS)?;
        let ten = record(b"0000000000000010", CARD_A, b"2022-07-18-10.15.42.370000")?;
        let seven = record(b"0000000000000007", CARD_A, b"2022-06-10 19:27:53.000000")?;
        for inserted in [&five, &ten, &seven] {
            store.insert(inserted).await?;
        }

        // The highest ID plus one, whatever the insertion order; each row reads back equal.
        assert_eq!(store.next_id().await?, key(b"0000000000000011"));
        assert_eq!(store.find_last().await?, Some(ten.clone()));
        for stored in [&five, &ten, &seven] {
            assert_eq!(store.find_by_id(&stored.id).await?.as_ref(), Some(stored));
        }
        assert!(store.find_by_id(&key(b"0000000000000006")).await?.is_none());

        // A second row with a taken ID is `DUPREC`; the stored row is unchanged, and the error
        // names the constraint alone, never the card number.
        let refused = store
            .insert(&record(b"0000000000000010", CARD_B, BLANK_TS)?)
            .await;
        let duplicate = PersistenceError::Duplicate {
            constraint: Some("transactions_pkey".to_owned()),
        };
        assert_eq!(refused, Err(duplicate.clone()));
        assert!(!format!("{duplicate}{duplicate:?}").contains("9900000000000002"));
        assert_eq!(store.find_by_id(&ten.id).await?, Some(ten));
        Ok(())
    }

    // audit: COTRN00C-011
    // audit: COTRN00C-014
    // audit: COTRN00C-023
    // audit: CBSTM03A-001
    // audit: CBTRN03C-004
    // audit: CBTRN03C-005
    // audit: PIC-017
    // audit: PIC-018
    // audit: PIC-019
    // audit: PIC-020
    #[tokio::test]
    async fn pages_card_order_and_processing_window_follow_the_keys() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let rows: [(&[u8; 16], &[u8; 16], &[u8; 26]); 6] = [
            // The last instant before the window: excluded.
            (b"0000000000000001", CARD_B, b"2021-12-31 23:59:59.999999"),
            // The window's first instant, layout `I`.
            (b"0000000000000002", CARD_A, b"2022-01-01 00:00:00.000000"),
            // A screen date inside the window, layout `S`.
            (b"0000000000000003", CARD_B, b"2022-03-15                "),
            // A blank processing timestamp: never selected.
            (b"0000000000000004", CARD_A, BLANK_TS),
            // The window's last day, layout `D`.
            (b"0000000000000005", CARD_B, b"2022-07-06-23.59.59.990000"),
            // The first instant after the window: excluded.
            (b"0000000000000006", CARD_A, b"2022-07-07 00:00:00.000000"),
        ];
        for (id, card, proc_ts) in rows {
            store.insert(&record(id, card, proc_ts)?).await?;
        }
        let [one, two, three, four, five, six] = rows.map(|(id, _, _)| *id);
        let blank = PicX::<16>::spaces();

        // Forward pages start at the key, inclusive; a blank key ranks below every ID.
        assert_eq!(ids(&store.page_after(&blank, 3).await?), [one, two, three]);
        assert_eq!(
            ids(&store.page_after(&key(&four), 11).await?),
            [four, five, six]
        );
        assert!(
            store
                .page_after(&key(b"0000000000000007"), 11)
                .await?
                .is_empty()
        );
        assert!(store.page_after(&blank, 0).await?.is_empty());
        // Backward pages start at the key, inclusive, and run downwards.
        assert_eq!(
            ids(&store.page_before(&key(&four), 2).await?),
            [four, three]
        );
        let all_down = [six, five, four, three, two, one];
        assert_eq!(
            ids(&store.page_before(&key(b"0000000000000009"), 11).await?),
            all_down
        );
        assert!(store.page_before(&blank, 11).await?.is_empty());

        // CREASTMT: card number, then transaction ID.
        let card_order = [two, four, six, one, three, five];
        assert_eq!(ids(&store.find_all_in_card_order().await?), card_order);

        // TRANREPT: both boundary days included, the blank timestamp and the days outside the
        // window excluded, in card-number order.
        let (start, end) = (day(2022, 1, 1)?, day(2022, 7, 6)?);
        let window = store.find_processed_between(start, end).await?;
        assert_eq!(ids(&window), [two, three, five]);
        assert_eq!(ids(&store.find_processed_between(end, end).await?), [five]);
        assert!(store.find_processed_between(end, start).await?.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn keys_no_statement_can_bind_and_negative_limits_are_refused() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let high_values = PicX::from_array([0xFF; 16]);
        let invalid = PersistenceError::decode("tran_id", DecodeKind::InvalidByte);
        assert_eq!(
            store.page_after(&high_values, 1).await,
            Err(invalid.clone())
        );
        assert_eq!(
            store.page_before(&high_values, 1).await,
            Err(invalid.clone())
        );
        assert_eq!(store.find_by_id(&high_values).await, Err(invalid));
        let refused = store.page_after(&PicX::spaces(), -1).await;
        assert_eq!(
            refused.as_ref().err().and_then(PersistenceError::sqlstate),
            Some("2201W")
        );
        Ok(())
    }

    // audit: COTRN02C-043
    // audit: COBIL00C-023
    #[tokio::test]
    async fn tx_allocation_wraps_and_refuses_a_highest_id_that_is_not_digits() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let mut tx = store.begin().await?;
        tx.insert(&record(b"9999999999999999", CARD_A, BLANK_TS)?)
            .await?;

        // `ADD 1` to sixteen 9s stores sixteen 0s. The transaction sees its own insert; the
        // pool does not, until the commit.
        assert_eq!(tx.next_id().await?, key(b"0000000000000000"));
        assert_eq!(store.next_id().await?, key(b"0000000000000001"));

        // A highest ID with a byte other than a digit is refused, naming the column alone.
        let not_numeric = PersistenceError::decode("tran_id", DecodeKind::NotNumeric);
        for odd in [b"999999999999999:", b"ABCDEFGHIJKLMNOP"] {
            let mut savepoint = tx.savepoint().await?;
            savepoint.insert(&record(odd, CARD_A, BLANK_TS)?).await?;
            assert_eq!(
                savepoint.find_last().await?.map(|last| last.id),
                Some(key(odd))
            );
            assert_eq!(savepoint.next_id().await, Err(not_numeric.clone()));
            savepoint.rollback().await?;
        }
        assert_eq!(tx.next_id().await?, key(b"0000000000000000"));
        tx.commit().await?;
        assert_eq!(store.next_id().await?, key(b"0000000000000000"));
        Ok(())
    }

    // audit: CBTRN02C-025
    // audit: COTRN02C-050
    #[tokio::test]
    async fn tx_reads_its_own_rows_and_survives_a_failed_insert_only_in_a_savepoint() -> TestResult
    {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let first = record(b"0000000000000041", CARD_B, b"2022-05-01 08:00:00.000000")?;
        let second = record(b"0000000000000042", CARD_A, BLANK_TS)?;

        // Inside a savepoint the failed insert is undone and the transaction goes on.
        let mut tx = store.begin().await?;
        tx.insert(&first).await?;
        tx.insert(&second).await?;
        let mut savepoint = tx.savepoint().await?;
        let clash = record(b"0000000000000041", CARD_A, BLANK_TS)?;
        let refused = savepoint.insert(&clash).await;
        assert!(refused.as_ref().is_err_and(PersistenceError::is_duplicate));
        savepoint.rollback().await?;
        assert_eq!(tx.next_id().await?, key(b"0000000000000043"));
        assert_eq!(tx.find_by_id(&first.id).await?.as_ref(), Some(&first));
        assert_eq!(tx.find_last().await?.as_ref(), Some(&second));
        let both_down = [*second.id.as_bytes(), *first.id.as_bytes()];
        assert_eq!(ids(&tx.page_before(&second.id, 11).await?), both_down);
        assert_eq!(
            ids(&tx.page_after(&first.id, 1).await?),
            [*first.id.as_bytes()]
        );
        let card_order = [*second.id.as_bytes(), *first.id.as_bytes()];
        assert_eq!(ids(&tx.find_all_in_card_order().await?), card_order);
        let may = day(2022, 5, 1)?;
        let window = tx.find_processed_between(may, may).await?;
        assert_eq!(window, [first.clone()]);
        tx.commit().await?;

        // Without a savepoint the failed insert aborts the transaction: the next statement is
        // refused (SQLSTATE 25P02), and the rollback leaves the table as it was.
        let mut tx = store.begin().await?;
        let refused = tx.insert(&clash).await;
        assert!(refused.as_ref().is_err_and(PersistenceError::is_duplicate));
        let aborted = tx.next_id().await;
        assert_eq!(
            aborted.as_ref().err().and_then(PersistenceError::sqlstate),
            Some("25P02")
        );
        tx.rollback().await?;
        assert_eq!(store.find_by_id(&first.id).await?, Some(first));
        assert_eq!(store.next_id().await?, key(b"0000000000000043"));
        Ok(())
    }
}
