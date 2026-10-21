//! `CustomerRepository`: the statements on table `customers` (migration
//! `0001_reference_and_master.sql`), the 500-byte `CUSTOMER-RECORD` of `CVCUS01Y`
//! (CVCUS01Y-001 to CVCUS01Y-019), keyed by `cust_id` as the CUSTDAT KSDS is by `KEYS(9 0)`
//! (`CUSTFILE.jcl:50`).
//!
//! # COBOL accesses
//!
//! | COBOL access | Method |
//! |---|---|
//! | COACTVWC `9400-GETCUSTDATA-BYCUST`, `READ` by the cross-reference's `CUST-ID` (`COACTVWC.cbl:825-870`; COACTVWC-044, COACTVWC-046) | [`find_by_id`](CustomerRepository::find_by_id) |
//! | COACTUPC `9400-GETCUSTDATA-BYCUST`, the fetch's `READ` (`COACTUPC.cbl:3752-3800`; COACTUPC-095, COACTUPC-097) | [`find_by_id`](CustomerRepository::find_by_id) |
//! | COPAUS0C `GETCUSTDATA-BYCUST` (`COPAUS0C.cbl:915-964`), COPAUA0C `5300-READ-CUST-RECORD` (`COPAUA0C.cbl:568-612`) and CBSTM03B `3000-CUSTFILE-PROC`, the keyed `READ CUST-FILE` (`CBSTM03B.CBL:181-200`) | [`find_by_id`](CustomerRepository::find_by_id) |
//! | COACTUPC `9600-WRITE-PROCESSING`, `READ … UPDATE` of the customer (`COACTUPC.cbl:3917-3942`; COACTUPC-102, COACTUPC-112) | [`lock_by_id`](CustomerRepository::lock_by_id) |
//! | COACTUPC `9600-WRITE-PROCESSING`, `REWRITE` of `CUST-UPDATE-RECORD` (`COACTUPC.cbl:4085-4103`; COACTUPC-104, COACTUPC-107) | [`update`](CustomerRepository::update) |
//! | CUSTFILE `DELETE`, `DEFINE CLUSTER` and `REPRO` of `custdata.txt` (`CUSTFILE.jcl:35-71`; LOADS-004, LOADS-014, LOADS-015, LOADS-018) | [`replace_all`](CustomerRepository::replace_all) |
//! | Reading the loaded table back, for the `custfile` load and the tests | [`find_all`](CustomerRepository::find_all) |
//!
//! The CICS response each program branches on becomes the method's outcome: `NORMAL` is
//! `Ok(Some(_))`, `NOTFND` is `Ok(None)` (or `Ok(false)` for [`update`](CustomerRepository::update)),
//! and every other response is a [`PersistenceError`], the `WHEN OTHER` branch.
//!
//! # Statements
//!
//! Each statement is written once, in a private function over the executor, and both
//! [`PgStore`] (the pool) and [`PgTx`] (the caller's transaction) call it, so the offline query
//! cache holds one entry per statement. Column lists name all 19 columns in table order, which
//! are the field names of [`CustomerRow`]. Records reach SQL only through
//! [`CustomerRow::from_record`] and come back only through [`CustomerRow::into_record`]
//! (codec B): this module never trims, pads or re-encodes a byte.
//!
//! # Redaction
//!
//! `cust_ssn` holds the full SSN, which only [`CustomerRow`] binds, through `Ssn::expose`
//! (PIC-024, E32). No record, row or key is ever formatted here, and the module emits no
//! tracing, so the SSN leaves only as a bind parameter (Cardholder Data Redaction).

use copybook::PicU;
use domain::records::CustomerRecord;
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::{CustomerRow, pic_u_to_i64};
use crate::{PgStore, PgTx};

/// The statements on `customers`, implemented by [`PgStore`] and [`PgTx`]; test doubles in
/// other crates implement the same trait.
///
/// On [`PgStore`] each call runs in autocommit, except
/// [`replace_all`](Self::replace_all), which opens a transaction of its own. On [`PgTx`] every
/// call runs inside the caller's transaction.
pub trait CustomerRepository: Send {
    /// The customer stored under `id`, or `None` when no row has that key (CICS `NOTFND`): the
    /// keyed `READ` of COACTVWC, COACTUPC, COPAUS0C, COPAUA0C and CBSTM03B (COACTVWC-044,
    /// COACTUPC-095).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the database cannot be reached,
    /// [`PersistenceError::Database`] for any other failure of the read, and
    /// [`PersistenceError::Decode`] when the stored row does not fit `CVCUS01Y`
    /// ([`CustomerRow::into_record`]). Each is the `WHEN OTHER` branch of the readers
    /// (COACTVWC-046, COACTUPC-097).
    fn find_by_id(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send;

    /// The customer stored under `id`, read with a row lock (`SELECT … FOR UPDATE`), or `None`
    /// when no row has that key: the `READ … UPDATE` of COACTUPC's `9600-WRITE-PROCESSING`
    /// (COACTUPC-102, COACTUPC-112).
    ///
    /// On a [`PgTx`] the lock holds until the transaction commits or rolls back, so no other
    /// transaction can update or lock the row in between. On a [`PgStore`] the statement commits
    /// at once and the lock is released with it, so the lock means something only on a
    /// [`PgTx`]. A lock that another transaction holds makes the call wait, as a CICS
    /// `READ UPDATE` waits on the record another task holds.
    ///
    /// # Errors
    ///
    /// As [`find_by_id`](Self::find_by_id); [`PersistenceError::Database`] also reports a lock
    /// that cannot be had, such as SQLSTATE `55P03` when the session's `lock_timeout` expires or
    /// `40P01` for a deadlock. COACTUPC reports every such outcome as
    /// `Could not lock customer record for update` (COACTUPC-102).
    fn lock_by_id(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send;

    /// Every stored customer in key order, as a KSDS browse returns them: for the `custfile`
    /// load and the tests only.
    ///
    /// The read is unbounded. No API collection calls it, so Bounded Pagination does not apply.
    ///
    /// # Errors
    ///
    /// As [`find_by_id`](Self::find_by_id), the first row that does not decode ending the read.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CustomerRecord>, PersistenceError>> + Send;

    /// Stores `record` as a new row, every field as codec B maps it.
    ///
    /// On a [`PgTx`], a failed insert aborts the whole transaction: a caller that must go on
    /// after an expected failure, such as a duplicate key, runs the insert inside a
    /// [`PgTx::savepoint`] and rolls back to it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when a row already has the key (constraint
    /// `customers_pkey`, CICS `DUPREC`); [`PersistenceError::Decode`] for a `PIC X(n)` field
    /// holding a NUL or non-ASCII byte, which its `CHAR(n)` column cannot keep
    /// ([`CustomerRow::from_record`]); [`PersistenceError::Unavailable`] and
    /// [`PersistenceError::Database`] as for [`find_by_id`](Self::find_by_id).
    fn insert(
        &mut self,
        record: &CustomerRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Rewrites the row whose key is `record.id` with every other field of `record`, `FILLER`
    /// included: the customer `REWRITE` of COACTUPC's `9600-WRITE-PROCESSING` (COACTUPC-104).
    ///
    /// Returns `true` when the row was rewritten and `false` when no row has the key (CICS
    /// `NOTFND`). The key itself is never changed, as CICS refuses a `REWRITE` that changes it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] as for [`insert`](Self::insert);
    /// [`PersistenceError::Unavailable`] and [`PersistenceError::Database`] as for
    /// [`find_by_id`](Self::find_by_id). COACTUPC reports a failed rewrite as
    /// `Update of record failed` (COACTUPC-107).
    fn update(
        &mut self,
        record: &CustomerRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Replaces the table's contents with `records`, inserted in slice order, and returns how
    /// many rows it inserted: the CUSTFILE job's `DELETE`, `DEFINE CLUSTER` and `REPRO` into the
    /// new, empty cluster (LOADS-004, LOADS-014).
    ///
    /// The delete and every insert form one unit: on a [`PgStore`] a transaction of their own,
    /// committed when the last insert succeeds; on a [`PgTx`] the caller's transaction. An empty
    /// table is no error (LOADS-015). No foreign key joins `customers`, so the load may run in
    /// any order with the other loads.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a record holds a byte codec B cannot store, as for
    /// [`insert`](Self::insert): every record is converted before the delete, so the call fails
    /// with nothing written, leaving the table and a [`PgTx`] caller's transaction as they were.
    /// Otherwise the first failure of the delete or of an insert, as [`insert`](Self::insert)
    /// reports it, a duplicate key in `records` included. On a [`PgStore`] the transaction is
    /// then rolled back and the previous contents remain (LOADS-018); on a [`PgTx`] the failed
    /// statement aborts the caller's transaction.
    fn replace_all(
        &mut self,
        records: &[CustomerRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

/// `SELECT` of the row under `id`, decoded through codec B; `None` when no row has the key.
///
/// Shared with `repos::accounts`, which reads the customer of an account with the same
/// statement.
pub(super) async fn select_by_id<'e, E: PgExecutor<'e>>(
    executor: E,
    id: PicU<9>,
) -> Result<Option<CustomerRecord>, PersistenceError> {
    let key = pic_u_to_i64("cust_id", id)?;
    let row = sqlx::query_as!(
        CustomerRow,
        "SELECT cust_id, cust_first_name, cust_middle_name, cust_last_name, cust_addr_line_1, \
         cust_addr_line_2, cust_addr_line_3, cust_addr_state_cd, cust_addr_country_cd, \
         cust_addr_zip, cust_phone_num_1, cust_phone_num_2, cust_ssn, cust_govt_issued_id, \
         cust_dob_yyyy_mm_dd, cust_eft_account_id, cust_pri_card_holder_ind, \
         cust_fico_credit_score, filler \
         FROM customers WHERE cust_id = $1",
        key
    )
    .fetch_optional(executor)
    .await?;
    row.map(CustomerRow::into_record).transpose()
}

/// `SELECT … FOR UPDATE` of the row under `id`. The row lock lasts as long as the executor's
/// transaction: until the caller's [`PgTx`] commits or rolls back, or, on the pool, only as long
/// as the statement.
async fn select_for_update<'e, E: PgExecutor<'e>>(
    executor: E,
    id: PicU<9>,
) -> Result<Option<CustomerRecord>, PersistenceError> {
    let key = pic_u_to_i64("cust_id", id)?;
    let row = sqlx::query_as!(
        CustomerRow,
        "SELECT cust_id, cust_first_name, cust_middle_name, cust_last_name, cust_addr_line_1, \
         cust_addr_line_2, cust_addr_line_3, cust_addr_state_cd, cust_addr_country_cd, \
         cust_addr_zip, cust_phone_num_1, cust_phone_num_2, cust_ssn, cust_govt_issued_id, \
         cust_dob_yyyy_mm_dd, cust_eft_account_id, cust_pri_card_holder_ind, \
         cust_fico_credit_score, filler \
         FROM customers WHERE cust_id = $1 FOR UPDATE",
        key
    )
    .fetch_optional(executor)
    .await?;
    row.map(CustomerRow::into_record).transpose()
}

/// `SELECT` of every row in key order, each decoded through codec B. Unbounded: batch loads and
/// tests only.
async fn select_all<'e, E: PgExecutor<'e>>(
    executor: E,
) -> Result<Vec<CustomerRecord>, PersistenceError> {
    let rows = sqlx::query_as!(
        CustomerRow,
        "SELECT cust_id, cust_first_name, cust_middle_name, cust_last_name, cust_addr_line_1, \
         cust_addr_line_2, cust_addr_line_3, cust_addr_state_cd, cust_addr_country_cd, \
         cust_addr_zip, cust_phone_num_1, cust_phone_num_2, cust_ssn, cust_govt_issued_id, \
         cust_dob_yyyy_mm_dd, cust_eft_account_id, cust_pri_card_holder_ind, \
         cust_fico_credit_score, filler \
         FROM customers ORDER BY cust_id"
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(CustomerRow::into_record).collect()
}

/// `INSERT` of `record` as one row, converted by [`CustomerRow::from_record`] and bound by
/// [`insert_row`]; returns the number of rows inserted.
async fn insert_one<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &CustomerRecord,
) -> Result<u64, PersistenceError> {
    let row = CustomerRow::from_record(record)?;
    insert_row(executor, &row).await
}

/// The one `INSERT` statement, which [`insert_one`] and [`replace_all_in`] share: every column
/// bound from `row`, already converted through codec B; returns the number of rows inserted.
async fn insert_row<'e, E: PgExecutor<'e>>(
    executor: E,
    row: &CustomerRow,
) -> Result<u64, PersistenceError> {
    let done = sqlx::query!(
        "INSERT INTO customers (cust_id, cust_first_name, cust_middle_name, cust_last_name, \
         cust_addr_line_1, cust_addr_line_2, cust_addr_line_3, cust_addr_state_cd, \
         cust_addr_country_cd, cust_addr_zip, cust_phone_num_1, cust_phone_num_2, cust_ssn, \
         cust_govt_issued_id, cust_dob_yyyy_mm_dd, cust_eft_account_id, \
         cust_pri_card_holder_ind, cust_fico_credit_score, filler) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, \
         $18, $19)",
        row.cust_id,
        row.cust_first_name,
        row.cust_middle_name,
        row.cust_last_name,
        row.cust_addr_line_1,
        row.cust_addr_line_2,
        row.cust_addr_line_3,
        row.cust_addr_state_cd,
        row.cust_addr_country_cd,
        row.cust_addr_zip,
        row.cust_phone_num_1,
        row.cust_phone_num_2,
        row.cust_ssn,
        row.cust_govt_issued_id,
        row.cust_dob_yyyy_mm_dd,
        row.cust_eft_account_id,
        row.cust_pri_card_holder_ind,
        row.cust_fico_credit_score,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// `UPDATE` of every non-key column of the row whose key is `record.id`, `filler` included;
/// `true` when exactly one row changed, `false` when no row has the key.
async fn update_one<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &CustomerRecord,
) -> Result<bool, PersistenceError> {
    let row = CustomerRow::from_record(record)?;
    let done = sqlx::query!(
        "UPDATE customers SET cust_first_name = $2, cust_middle_name = $3, \
         cust_last_name = $4, cust_addr_line_1 = $5, cust_addr_line_2 = $6, \
         cust_addr_line_3 = $7, cust_addr_state_cd = $8, cust_addr_country_cd = $9, \
         cust_addr_zip = $10, cust_phone_num_1 = $11, cust_phone_num_2 = $12, cust_ssn = $13, \
         cust_govt_issued_id = $14, cust_dob_yyyy_mm_dd = $15, cust_eft_account_id = $16, \
         cust_pri_card_holder_ind = $17, cust_fico_credit_score = $18, filler = $19 \
         WHERE cust_id = $1",
        row.cust_id,
        row.cust_first_name,
        row.cust_middle_name,
        row.cust_last_name,
        row.cust_addr_line_1,
        row.cust_addr_line_2,
        row.cust_addr_line_3,
        row.cust_addr_state_cd,
        row.cust_addr_country_cd,
        row.cust_addr_zip,
        row.cust_phone_num_1,
        row.cust_phone_num_2,
        row.cust_ssn,
        row.cust_govt_issued_id,
        row.cust_dob_yyyy_mm_dd,
        row.cust_eft_account_id,
        row.cust_pri_card_holder_ind,
        row.cust_fico_credit_score,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `DELETE` of every row, then [`insert_row`] of each record in slice order, on one connection
/// that the caller holds inside a transaction; returns the number of rows inserted.
///
/// Every record is converted through codec B before the delete, so a record that
/// [`CustomerRow::from_record`] cannot store fails the call before any statement is sent: the
/// table, and a caller's transaction, are left as they were (LOADS-018).
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[CustomerRecord],
) -> Result<u64, PersistenceError> {
    let rows = records
        .iter()
        .map(CustomerRow::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    sqlx::query!("DELETE FROM customers")
        .execute(&mut *conn)
        .await?;
    let mut inserted = 0;
    for row in &rows {
        inserted += insert_row(&mut *conn, row).await?;
    }
    Ok(inserted)
}

impl CustomerRepository for PgStore {
    fn find_by_id(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send {
        select_by_id(self.pool(), id)
    }

    fn lock_by_id(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send {
        select_for_update(self.pool(), id)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CustomerRecord>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &CustomerRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let pool = self.pool();
        async move {
            insert_one(pool, record).await?;
            Ok(())
        }
    }

    fn update(
        &mut self,
        record: &CustomerRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_one(self.pool(), record)
    }

    fn replace_all(
        &mut self,
        records: &[CustomerRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // Dropping the transaction on an error rolls it back, so the previous contents
            // remain (LOADS-018).
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl CustomerRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send {
        select_by_id(self.conn(), id)
    }

    fn lock_by_id(
        &mut self,
        id: PicU<9>,
    ) -> impl Future<Output = Result<Option<CustomerRecord>, PersistenceError>> + Send {
        select_for_update(self.conn(), id)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CustomerRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &CustomerRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let conn = self.conn();
        async move {
            insert_one(conn, record).await?;
            Ok(())
        }
    }

    fn update(
        &mut self,
        record: &CustomerRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_one(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[CustomerRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), records)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::str::FromStr;

    use copybook::{FixedRecord, PicU, PicX};
    use domain::records::CustomerRecord;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use test_support::postgres::{PostgresGuard, start_postgres};

    use super::CustomerRepository;
    use crate::{DecodeKind, PersistenceError, PgStore};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// An invented SSN, none of the 50 fixture values. No error may repeat it (Cardholder Data
    /// Redaction).
    const SSN_DIGITS: &str = "987123456";

    /// SQLSTATE `lock_not_available`, raised when `lock_timeout` expires.
    const LOCK_NOT_AVAILABLE: &str = "55P03";

    /// A customer with invented values, keyed `id` and named `first_name`: a 500-byte line in the
    /// shape of `custdata.txt`, each field left-justified and space-padded to its `CVCUS01Y`
    /// width, decoded by codec A.
    fn sample(id: u64, first_name: &str) -> Result<CustomerRecord, Box<dyn StdError>> {
        let key = format!("{id:09}");
        let fields: [(&str, usize); 19] = [
            (&key, 9),
            (first_name, 25),
            ("Middle", 25),
            ("Lastname", 25),
            ("1 Example Street", 50),
            ("Suite 9", 50),
            ("Sampletown", 50),
            ("CA", 2),
            ("USA", 3),
            ("94000-1234", 10),
            ("(555)010-0001", 15),
            ("(555)010-0002", 15),
            (SSN_DIGITS, 9),
            ("00000000000000004242", 20),
            ("1980-02-29", 10),
            ("0000004242", 10),
            ("N", 1),
            ("750", 3),
            ("~filler kept~", 168),
        ];
        let mut line = String::with_capacity(CustomerRecord::WIDTH);
        for (text, width) in fields {
            line.push_str(&format!("{text:<width$}"));
        }
        Ok(CustomerRecord::decode(line.as_bytes())?)
    }

    /// The key `id` as a `CUST-ID PIC 9(09)`.
    fn key(id: u64) -> Result<PicU<9>, Box<dyn StdError>> {
        Ok(PicU::new(id)?)
    }

    /// A store over a second pool to `guard`'s database whose sessions wait at most 200 ms for
    /// a row lock, so a held lock shows as SQLSTATE `55P03` instead of a wait.
    async fn impatient_store(guard: &PostgresGuard) -> Result<PgStore, Box<dyn StdError>> {
        let options =
            PgConnectOptions::from_str(guard.database_url())?.options([("lock_timeout", "200ms")]);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await?;
        Ok(PgStore::new(pool))
    }

    /// Whether `result` is the failure of a lock that another transaction holds.
    fn is_lock_timeout<T>(result: &Result<T, PersistenceError>) -> bool {
        matches!(
            result,
            Err(PersistenceError::Database { sqlstate: Some(code), .. }) if code == LOCK_NOT_AVAILABLE
        )
    }

    /// The keyed read returns the stored record or `None` (CICS `NORMAL` and `NOTFND`), a second
    /// insert of a key is a duplicate, and the rewrite reports whether the key exists.
    #[tokio::test]
    async fn keyed_reads_inserts_and_rewrites_report_the_cics_outcomes() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let stored = sample(42, "Alpha")?;

        store.insert(&stored).await?;
        assert!(store.find_by_id(key(42)?).await? == Some(stored.clone()));
        assert!(store.find_by_id(key(43)?).await?.is_none());

        let duplicate = store.insert(&stored).await;
        assert_eq!(
            duplicate,
            Err(PersistenceError::Duplicate {
                constraint: Some("customers_pkey".to_owned())
            })
        );
        if let Err(error) = duplicate {
            for text in [error.to_string(), format!("{error:?}")] {
                assert!(
                    !text.contains(SSN_DIGITS) && !text.contains("3456"),
                    "{text}"
                );
            }
        }

        let rewritten = sample(42, "Rewritten")?;
        assert!(store.update(&rewritten).await?);
        assert!(store.find_by_id(key(42)?).await? == Some(rewritten.clone()));
        assert!(!store.update(&sample(43, "Absent")?).await?);
        assert!(store.find_by_id(key(43)?).await?.is_none());
        assert!(store.find_all().await? == vec![rewritten]);
        Ok(())
    }

    /// On a transaction, `lock_by_id` holds the row lock until the commit, writes stay invisible
    /// to other sessions until then, a rollback undoes them, and a savepoint lets the
    /// transaction go on after a duplicate insert. On the pool the lock ends with the call.
    #[tokio::test]
    async fn transactions_hold_row_locks_and_scope_their_writes() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let mut impatient = impatient_store(&guard).await?;
        let original = sample(42, "Original")?;
        let changed = sample(42, "Changed")?;
        let second = sample(43, "Second")?;
        store.insert(&original).await?;

        let mut tx = store.begin().await?;
        assert!(tx.lock_by_id(key(42)?).await? == Some(original.clone()));
        assert!(tx.lock_by_id(key(44)?).await?.is_none());
        let mut waiting = impatient.begin().await?;
        assert!(is_lock_timeout(&waiting.lock_by_id(key(42)?).await));
        drop(waiting);
        assert!(is_lock_timeout(&impatient.update(&changed).await));

        assert!(tx.update(&changed).await?);
        tx.insert(&second).await?;
        assert!(tx.find_by_id(key(42)?).await? == Some(changed.clone()));
        assert!(tx.find_all().await? == vec![changed.clone(), second.clone()]);
        assert!(store.find_by_id(key(42)?).await? == Some(original));
        assert!(store.find_by_id(key(43)?).await?.is_none());
        tx.commit().await?;

        let mut after_commit = impatient.begin().await?;
        assert!(after_commit.lock_by_id(key(42)?).await? == Some(changed.clone()));
        after_commit.rollback().await?;
        assert!(store.find_all().await? == vec![changed.clone(), second.clone()]);

        assert!(store.lock_by_id(key(42)?).await? == Some(changed.clone()));
        let mut after_pool_lock = impatient.begin().await?;
        assert!(after_pool_lock.lock_by_id(key(42)?).await? == Some(changed.clone()));
        after_pool_lock.commit().await?;

        let mut undone = store.begin().await?;
        assert!(undone.update(&sample(42, "Undone")?).await?);
        undone.rollback().await?;
        assert!(store.find_by_id(key(42)?).await? == Some(changed));

        let mut goes_on = store.begin().await?;
        let mut savepoint = goes_on.savepoint().await?;
        assert!(
            savepoint
                .insert(&second)
                .await
                .is_err_and(|e| e.is_duplicate())
        );
        savepoint.rollback().await?;
        assert!(goes_on.find_by_id(key(43)?).await? == Some(second));
        goes_on.commit().await?;
        Ok(())
    }

    /// `replace_all` leaves exactly the input set in key order, gives the same contents when run
    /// again, accepts an empty table (LOADS-014, LOADS-015), keeps the previous contents when an
    /// insert fails (LOADS-018), and on a transaction runs inside the caller's unit of work.
    #[tokio::test]
    async fn replace_all_reloads_the_table_as_one_unit() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());

        assert_eq!(store.replace_all(&[]).await?, 0);
        assert!(store.find_all().await?.is_empty());
        store.insert(&sample(7, "Stale")?).await?;

        let input = [sample(43, "Second")?, sample(42, "First")?];
        let expected = vec![sample(42, "First")?, sample(43, "Second")?];
        assert_eq!(store.replace_all(&input).await?, 2);
        assert!(store.find_all().await? == expected);
        assert_eq!(store.replace_all(&input).await?, 2);
        assert!(store.find_all().await? == expected);

        let clashing = [sample(50, "Once")?, sample(50, "Twice")?];
        assert!(
            store
                .replace_all(&clashing)
                .await
                .is_err_and(|e| e.is_duplicate())
        );
        assert!(store.find_all().await? == expected);

        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&[sample(60, "Inside")?]).await?, 1);
        assert!(tx.find_all().await? == vec![sample(60, "Inside")?]);
        tx.rollback().await?;
        assert!(store.find_all().await? == expected);
        Ok(())
    }

    /// A record codec B cannot store, after one it can, fails the replacement before the delete
    /// (LOADS-018): through the pool nothing changes, and a caller's transaction is left usable
    /// and unchanged, so committing it after the error keeps the previous contents.
    #[tokio::test]
    async fn an_unstorable_record_fails_the_replacement_before_the_delete() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let original = [sample(42, "First")?, sample(43, "Second")?];
        assert_eq!(store.replace_all(&original).await?, 2);
        let unstorable = CustomerRecord {
            first_name: PicX::moved_from("Zoë".as_bytes()),
            ..sample(45, "Unstorable")?
        };
        let input = [sample(44, "Replacement")?, unstorable];
        let invalid = Err(PersistenceError::decode(
            "cust_first_name",
            DecodeKind::InvalidByte,
        ));

        assert_eq!(store.replace_all(&input).await, invalid);
        assert!(
            store.find_all().await? == original,
            "a failed load through the pool leaves the previous contents"
        );

        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&input).await, invalid);
        assert!(
            tx.find_all().await? == original,
            "the transaction deleted nothing and is not aborted"
        );
        tx.commit().await?;
        assert!(
            store.find_all().await? == original,
            "committing after the error keeps the previous contents"
        );
        Ok(())
    }
}
