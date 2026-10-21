//! [`TransactionCategoryRepository`]: table `transaction_categories` (migration 0001), the VSAM
//! data version of the transaction categories, one `CVTRA04Y` `TRAN-CAT-RECORD` per row
//! (CVTRA04Y-001 to CVTRA04Y-004). Its primary key (`tran_type_cd`, `tran_cat_cd`) is
//! `TRAN-CAT-KEY`, the KSDS `KEYS(6 0)` of `TRANCATG.jcl:40`.
//!
//! | COBOL access | Method |
//! |---|---|
//! | CBTRN03C `1500-C-LOOKUP-TRANCATG`: random `READ TRANCATG-FILE INTO TRAN-CAT-RECORD` by `FD-TRAN-CAT-KEY`, the type code then the category code of the transaction (`CBTRN03C.cbl:45-49,77-82,191-195,504-512`; CBTRN03C-017) | [`find_by_id`](TransactionCategoryRepository::find_by_id): `INVALID KEY` is `Ok(None)` (CBTRN03C-018); any other failure is a typed error (CBTRN03C-042) |
//! | TRANCATG `STEP05` `DELETE`, `STEP10` `DEFINE CLUSTER` and `STEP15` `REPRO INFILE(TRANCATG) OUTFILE(TCATVSAM)` (`TRANCATG.jcl:22-61`; LOADS-007, LOADS-014, LOADS-015) | [`replace_all`](TransactionCategoryRepository::replace_all) |
//! | A sequential read of the whole cluster in key order | [`find_all`](TransactionCategoryRepository::find_all) |
//! | `REPRO` of one record into the loaded cluster | [`insert`](TransactionCategoryRepository::insert) |
//!
//! # One data version (E23)
//!
//! Only `tranrept` reads this table and only the `trancatg` load replaces it (LOADS-029). No
//! statement here names schema `db2`, so `db2.transaction_type_category`, the Db2 version that
//! COTRTLIC, COTRTUPC and COBTUPDT maintain, is never read or written, and nothing copies
//! between the two versions: a Db2 maintenance change never reaches a report lookup.
//!
//! # Bytes kept
//!
//! Records reach the table only through codec B ([`TransactionCategoryRow`]), which keeps every
//! byte of the three `PIC X` fields: the description's padding (PIC-001) and the `FILLER`, the
//! `0000` of every `trancatg.txt` record included (LOADS-032, PIC-012, E05). The CRLF ending each
//! fixture line is framing and is never stored (LOADS-031, PIC-014, E06). The key columns carry
//! the `C` collation, so key order is byte order, the KSDS's.
//!
//! # Transactions
//!
//! On [`PgStore`] every method but [`replace_all`](TransactionCategoryRepository::replace_all)
//! runs one autocommit statement; `replace_all` opens and commits a transaction of its own. On
//! [`PgTx`] every statement runs in the caller's transaction. Records are converted before the
//! first statement, so a record codec B cannot store fails before anything is written and leaves
//! a [`PgTx`] usable; a statement that fails aborts the transaction (LOADS-018), and a caller
//! that must go on after it runs the statement inside [`PgTx::savepoint`].
//!
//! # Redaction
//!
//! Errors are [`PersistenceError`] values, which carry an SQLSTATE, a constraint name or a column
//! name, never a key or a record. Nothing here is traced.

use copybook::{PicU, PicX};
use domain::records::TransactionCategoryRecord;
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::{TransactionCategoryRow, pic_u_to_i64, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column of `TRAN-TYPE-CD`, the first key part (CVTRA04Y-001).
const TRAN_TYPE_CD: &str = "tran_type_cd";
/// Column of `TRAN-CAT-CD`, the second key part (CVTRA04Y-002).
const TRAN_CAT_CD: &str = "tran_cat_cd";

/// The transaction categories of the VSAM data version, table `transaction_categories`.
///
/// Implemented by [`PgStore`] and [`PgTx`]; test doubles in other crates implement it too.
pub trait TransactionCategoryRepository: Send {
    /// The category stored under `TRAN-CAT-KEY` (`type_cd`, `cat_cd`), as CBTRN03C's random read
    /// returns it (`CBTRN03C.cbl:504-505`; CBTRN03C-017), or `None` when no row holds that key,
    /// the read's `INVALID KEY` (`CBTRN03C.cbl:506-511`; CBTRN03C-018).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming `tran_type_cd` when `type_cd` holds a NUL or non-ASCII
    /// byte, which no `CHAR(2)` key can store, or naming the column of a stored value that does
    /// not fit its picture; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`]
    /// when the query fails without answering (CBTRN03C-042).
    fn find_by_id(
        &mut self,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<TransactionCategoryRecord>, PersistenceError>> + Send;

    /// Every category, in key order: type code, then category code, both compared byte by byte.
    /// The read is unbounded, for batch jobs and tests; no API endpoint serves this table.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the column of the first stored value that does not fit
    /// its picture; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the
    /// query fails.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionCategoryRecord>, PersistenceError>> + Send;

    /// Adds `record` as one row, every byte of its `PIC X` fields kept.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] (constraint `transaction_categories_pkey`) when its key
    /// is already stored; [`PersistenceError::Decode`] when codec B cannot store a field, before
    /// any statement runs; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`]
    /// when the statement fails otherwise. On a [`PgTx`], a failed statement aborts the caller's
    /// transaction; run the insert inside [`PgTx::savepoint`] to go on after one.
    fn insert(
        &mut self,
        record: &TransactionCategoryRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Replaces the table's contents with `records`, the counterpart of TRANCATG's `DELETE`,
    /// `DEFINE CLUSTER` and `REPRO` into the new, empty cluster (`TRANCATG.jcl:22-61`): every row
    /// is deleted, then each record is inserted in slice order. Once committed, the table holds
    /// exactly `records` (LOADS-014); an empty table is no error (LOADS-015). Returns the number
    /// of rows inserted.
    ///
    /// The keys are not checked for ascending order, which the load does before calling
    /// (LOADS-017); the primary key refuses a duplicate. On [`PgStore`] the replacement runs in a
    /// transaction of its own; on [`PgTx`] it runs in the caller's and is undone with it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when codec B cannot store a field of any record, before any
    /// statement runs; [`PersistenceError::Duplicate`] when two records share a key;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when a statement or
    /// the commit fails. After any of them the previous contents remain once the transaction is
    /// rolled back (LOADS-018): [`PgStore`] rolls its own back, a [`PgTx`] caller rolls back
    /// its.
    fn replace_all(
        &mut self,
        records: &[TransactionCategoryRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

impl TransactionCategoryRepository for PgStore {
    fn find_by_id(
        &mut self,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<TransactionCategoryRecord>, PersistenceError>> + Send
    {
        select_by_id(self.pool(), type_cd, cat_cd)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionCategoryRecord>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &TransactionCategoryRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_record(self.pool(), record)
    }

    fn replace_all(
        &mut self,
        records: &[TransactionCategoryRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // Dropping `tx` on an error rolls the replacement back, leaving the previous rows.
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl TransactionCategoryRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<TransactionCategoryRecord>, PersistenceError>> + Send
    {
        select_by_id(self.conn(), type_cd, cat_cd)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionCategoryRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &TransactionCategoryRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_record(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[TransactionCategoryRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), records)
    }
}

/// The row keyed (`type_cd`, `cat_cd`), as its record.
async fn select_by_id<'e, E>(
    executor: E,
    type_cd: &PicX<2>,
    cat_cd: PicU<4>,
) -> Result<Option<TransactionCategoryRecord>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let tran_type_cd = pic_x_to_string(TRAN_TYPE_CD, type_cd)?;
    let tran_cat_cd = pic_u_to_i64(TRAN_CAT_CD, cat_cd)?;
    let row = sqlx::query_as!(
        TransactionCategoryRow,
        "SELECT tran_type_cd, tran_cat_cd, tran_cat_type_desc, filler \
         FROM transaction_categories \
         WHERE tran_type_cd = $1 AND tran_cat_cd = $2",
        tran_type_cd,
        tran_cat_cd,
    )
    .fetch_optional(executor)
    .await?;
    row.map(TransactionCategoryRow::into_record).transpose()
}

/// Every row in key order, as records.
async fn select_all<'e, E>(executor: E) -> Result<Vec<TransactionCategoryRecord>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as!(
        TransactionCategoryRow,
        "SELECT tran_type_cd, tran_cat_cd, tran_cat_type_desc, filler \
         FROM transaction_categories \
         ORDER BY tran_type_cd, tran_cat_cd",
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(TransactionCategoryRow::into_record)
        .collect()
}

/// The one insert statement: stores `row` and returns the number of rows it added.
async fn insert_one<'e, E>(
    executor: E,
    row: &TransactionCategoryRow,
) -> Result<u64, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let done = sqlx::query!(
        "INSERT INTO transaction_categories \
         (tran_type_cd, tran_cat_cd, tran_cat_type_desc, filler) \
         VALUES ($1, $2, $3, $4)",
        row.tran_type_cd,
        row.tran_cat_cd,
        row.tran_cat_type_desc,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// Converts `record` through codec B, then stores it.
async fn insert_record<'e, E>(
    executor: E,
    record: &TransactionCategoryRecord,
) -> Result<(), PersistenceError>
where
    E: PgExecutor<'e>,
{
    let row = TransactionCategoryRow::from_record(record)?;
    insert_one(executor, &row).await?;
    Ok(())
}

/// Deletes every row, then inserts `records` in slice order on `conn`, inside the transaction
/// `conn` holds; returns the number of rows inserted.
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[TransactionCategoryRecord],
) -> Result<u64, PersistenceError> {
    // Every record is converted first, so one that codec B cannot store fails the replacement
    // before the delete and leaves the caller's transaction usable.
    let rows = records
        .iter()
        .map(TransactionCategoryRow::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    sqlx::query!("DELETE FROM transaction_categories")
        .execute(&mut *conn)
        .await?;
    let mut inserted = 0;
    for row in &rows {
        inserted += insert_one(&mut *conn, row).await?;
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::{FixedRecord, PicU, PicX};
    use domain::records::TransactionCategoryRecord;

    use super::{TRAN_TYPE_CD, TransactionCategoryRepository};
    use crate::PgStore;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    // Four fixture bodies are quoted rather than the file read: this crate reaches fixture paths
    // only through `test_support`, and the whole-file load is covered by the batch `loads` test.

    /// `trancatg.txt` line 1: type `01`, category `0001` (FIXFRM-008).
    const LINE_1: &[u8] = b"010001Regular Sales Draft                               0000";
    /// `trancatg.txt` line 5: type `01`, category `0005`.
    const LINE_5: &[u8] = b"010005Interest Amount                                   0000";
    /// `trancatg.txt` line 9: type `03`, category `0001`.
    const LINE_9: &[u8] = b"030001Credit to Account                                 0000";
    /// `trancatg.txt` line 18, the last: type `07`, category `0001`.
    const LINE_18: &[u8] = b"070001Sales draft credit adjustment                     0000";

    /// The primary key of migration 0001's `transaction_categories`.
    const PKEY: &str = "transaction_categories_pkey";

    fn decode(body: &[u8]) -> Result<TransactionCategoryRecord, Box<dyn StdError>> {
        Ok(TransactionCategoryRecord::decode(body)?)
    }

    fn decode_all(bodies: &[&[u8]]) -> Result<Vec<TransactionCategoryRecord>, Box<dyn StdError>> {
        bodies.iter().map(|body| decode(body)).collect()
    }

    /// A record outside the fixtures, whose type code sorts by byte (`Z` before `a`) rather than
    /// by any linguistic collation, and whose description and `FILLER` hold bytes a trimming or
    /// blanking conversion would change.
    fn synthetic(
        type_cd: [u8; 2],
        cat_cd: u64,
    ) -> Result<TransactionCategoryRecord, Box<dyn StdError>> {
        Ok(TransactionCategoryRecord {
            type_cd: PicX::from_array(type_cd),
            cat_cd: PicU::new(cat_cd)?,
            type_desc: PicX::from_text("  leading, inner  and trailing spaces kept")?,
            filler: PicX::from_array(*b"0 9~"),
        })
    }

    fn duplicate() -> PersistenceError {
        PersistenceError::Duplicate {
            constraint: Some(PKEY.to_owned()),
        }
    }

    // audit: LOADS-014
    // audit: LOADS-015
    // audit: LOADS-018
    // audit: LOADS-032
    #[tokio::test]
    async fn store_replaces_reads_by_key_and_keeps_previous_rows_on_failure() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());

        // A freshly migrated table: nothing to delete is no error, every record is inserted,
        // and the rows come back in key order whatever the slice order was.
        assert_eq!(store.find_all().await?, Vec::new());
        assert_eq!(
            store
                .replace_all(&decode_all(&[LINE_18, LINE_9, LINE_5, LINE_1])?)
                .await?,
            4
        );
        let fixtures = decode_all(&[LINE_1, LINE_5, LINE_9, LINE_18])?;
        assert_eq!(store.find_all().await?, fixtures);

        // The random read needs both key parts; a missing pair is `INVALID KEY`, not an error.
        let three_one = decode(LINE_9)?;
        let found = store
            .find_by_id(&three_one.type_cd, three_one.cat_cd)
            .await?;
        assert_eq!(found, Some(three_one));
        let one = PicX::from_array(*b"01");
        assert_eq!(store.find_by_id(&one, PicU::new(9)?).await?, None);
        assert_eq!(
            store
                .find_by_id(&PicX::from_array(*b"09"), PicU::new(1)?)
                .await?,
            None
        );

        // On a loaded table the replacement holds exactly the new set, every byte kept, and a
        // rerun leaves identical contents.
        let lower = synthetic(*b"a1", 9_999)?;
        let upper = synthetic(*b"Z1", 0)?;
        let next = vec![lower.clone(), decode(LINE_1)?, upper.clone()];
        let ordered = vec![decode(LINE_1)?, upper.clone(), lower.clone()];
        for _ in 0..2 {
            assert_eq!(store.replace_all(&next).await?, 3);
            assert_eq!(store.find_all().await?, ordered);
        }
        assert_eq!(
            store.find_by_id(&lower.type_cd, lower.cat_cd).await?,
            Some(lower.clone())
        );
        assert_eq!(store.find_by_id(&one, PicU::new(5)?).await?, None);

        // One insert adds one row; a stored key is refused by the primary key.
        let added = decode(LINE_18)?;
        store.insert(&added).await?;
        assert_eq!(
            store.find_by_id(&added.type_cd, added.cat_cd).await?,
            Some(added.clone())
        );
        assert_eq!(store.insert(&added).await, Err(duplicate()));
        let loaded = vec![decode(LINE_1)?, decode(LINE_18)?, upper, lower];
        assert_eq!(store.find_all().await?, loaded);

        // A failed replacement leaves the previous contents: a duplicate key fails an insert
        // after the delete, and a record codec B cannot store fails before it.
        let twice = decode_all(&[LINE_9, LINE_9])?;
        assert_eq!(store.replace_all(&twice).await, Err(duplicate()));
        assert_eq!(store.find_all().await?, loaded);
        let unstorable = TransactionCategoryRecord {
            filler: PicX::from_array([b'0', 0, b'0', b'0']),
            ..decode(LINE_5)?
        };
        assert_eq!(
            store
                .replace_all(&[decode(LINE_9)?, unstorable.clone()])
                .await,
            Err(PersistenceError::decode("filler", DecodeKind::InvalidByte))
        );
        assert_eq!(
            store.insert(&unstorable).await,
            Err(PersistenceError::decode("filler", DecodeKind::InvalidByte))
        );
        assert_eq!(store.find_all().await?, loaded);

        // A key no `CHAR(2)` column can hold is a typed error, never normalised into a miss.
        assert_eq!(
            store
                .find_by_id(&PicX::from_array([0, b'1']), PicU::new(1)?)
                .await,
            Err(PersistenceError::decode(
                TRAN_TYPE_CD,
                DecodeKind::InvalidByte
            ))
        );
        Ok(())
    }

    // audit: LOADS-014
    // audit: LOADS-018
    #[tokio::test]
    async fn transaction_runs_every_method_in_the_callers_unit_of_work() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let before = decode_all(&[LINE_1])?;
        assert_eq!(store.replace_all(&before).await?, 1);

        // Uncommitted work is visible inside the transaction only, and a rollback undoes it.
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&decode_all(&[LINE_9, LINE_5])?).await?, 2);
        assert_eq!(tx.find_all().await?, decode_all(&[LINE_5, LINE_9])?);
        let five = decode(LINE_5)?;
        assert_eq!(
            tx.find_by_id(&five.type_cd, five.cat_cd).await?,
            Some(five.clone())
        );
        assert_eq!(store.find_all().await?, before);
        tx.rollback().await?;
        assert_eq!(store.find_all().await?, before);

        // A replacement and an insert in one transaction commit together.
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&[five.clone()]).await?, 1);
        tx.insert(&decode(LINE_18)?).await?;
        tx.commit().await?;
        assert_eq!(store.find_all().await?, decode_all(&[LINE_5, LINE_18])?);

        // A refused insert inside a savepoint is undone alone, and the transaction goes on.
        let mut tx = store.begin().await?;
        let mut nested = tx.savepoint().await?;
        assert_eq!(nested.insert(&five).await, Err(duplicate()));
        nested.rollback().await?;
        tx.insert(&decode(LINE_9)?).await?;
        tx.commit().await?;
        let after = decode_all(&[LINE_5, LINE_9, LINE_18])?;
        assert_eq!(store.find_all().await?, after);

        // A record codec B cannot store fails before any statement, so the transaction stays
        // usable; the missing key reads as `None` inside it too.
        let mut tx = store.begin().await?;
        let unstorable = TransactionCategoryRecord {
            type_cd: PicX::from_array([b'0', 0x80]),
            ..decode(LINE_1)?
        };
        assert_eq!(
            tx.replace_all(&[unstorable]).await,
            Err(PersistenceError::decode(
                TRAN_TYPE_CD,
                DecodeKind::InvalidByte
            ))
        );
        assert_eq!(tx.find_all().await?, after);
        assert_eq!(
            tx.find_by_id(&PicX::from_array(*b"01"), PicU::new(1)?)
                .await?,
            None
        );
        tx.commit().await?;
        assert_eq!(store.find_all().await?, after);
        Ok(())
    }
}
