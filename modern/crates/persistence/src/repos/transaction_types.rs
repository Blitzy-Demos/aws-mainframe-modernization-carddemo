//! [`TransactionTypeRepository`]: table `transaction_types` (migration 0001), one row per
//! `CVTRA03Y` `TRAN-TYPE-RECORD`, keyed by `tran_type`, the KSDS `KEYS(2 0)`
//! (`app/jcl/TRANTYPE.jcl:40`; CVTRA03Y-001 to CVTRA03Y-003).
//!
//! # Data version (E23)
//!
//! This table is the VSAM data version of the transaction types: the `trantype` load replaces
//! it from `trantype.txt` and only `tranrept` reads it, for the descriptions CBTRN03C prints.
//! The Db2 data version is `db2.transaction_type`, which only `Db2TransactionTypeRepository`
//! in `db2_transaction_types` reaches. No method's statement names schema `db2` (only a test reads
//! it, to show that a load leaves it unchanged), and no method copies between the versions: the
//! source synchronises them only through TRANEXTR, which is out of scope, so a maintenance change
//! through COTRTUPC or COBTUPDT never reaches a report lookup (CBTRN03C-038, LOADS-029).
//!
//! # COBOL accesses
//!
//! | COBOL access | Method |
//! |---|---|
//! | CBTRN03C `1500-B-LOOKUP-TRANTYPE`: random `READ TRANTYPE-FILE` by `FD-TRAN-TYPE`, `INVALID KEY` when the code is absent (`app/cbl/CBTRN03C.cbl:494-503`; CBTRN03C-015, CBTRN03C-016) | [`find_by_id`](TransactionTypeRepository::find_by_id): `Ok(None)` for `INVALID KEY`; any other failure is an error, never a missing row (CBTRN03C-041) |
//! | TRANTYPE STEP05, STEP10 and STEP15: IDCAMS `DELETE CLUSTER`, `DEFINE CLUSTER` and `REPRO INFILE(TRANTYPE) OUTFILE(TTYPVSAM)` from `TRANTYPE.PS` (`app/jcl/TRANTYPE.jcl:22-61`; LOADS-008) | [`replace_all`](TransactionTypeRepository::replace_all): one transaction deletes every row, then inserts every record in slice order (LOADS-014, LOADS-015, LOADS-018) |
//!
//! [`find_all`](TransactionTypeRepository::find_all) returns the whole table in key order for
//! batch callers; it is unbounded, as the table holds the seven fixture types and no endpoint
//! serves it. [`insert`](TransactionTypeRepository::insert) adds one row without replacing the
//! others; no in-scope COBOL program writes this file outside the REPRO load.
//!
//! Every record passes through codec B ([`TransactionTypeRow`]) byte for byte: the description
//! with its trailing spaces and the fixture's case, and the `FILLER` bytes, which hold `0`
//! characters in every fixture record (LOADS-032, E05).
//!
//! # Signatures
//!
//! `find_by_id` takes its key as `&PicX<2>`: `PicX` is `Clone` but not `Copy`, so the key is
//! borrowed like every other `PicX` key.

use std::future::Future;

use copybook::PicX;
use domain::records::TransactionTypeRecord;
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::{TransactionTypeRow, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column `tran_type CHAR(2)`, the primary key, as an error names it when a lookup key cannot be
/// bound.
const TRAN_TYPE: &str = "tran_type";

/// The VSAM data version of the transaction types, table `transaction_types` (E23).
///
/// Implemented by [`PgStore`], which runs each call on a pooled connection and gives
/// [`replace_all`](Self::replace_all) a transaction of its own, and by [`PgTx`], which runs every
/// call inside the caller's transaction.
pub trait TransactionTypeRepository: Send {
    /// The type whose code is `code`, the random read of CBTRN03C `1500-B-LOOKUP-TRANTYPE`
    /// (`app/cbl/CBTRN03C.cbl:494-503`; CBTRN03C-015); `None` when no row holds the code, the
    /// program's `INVALID KEY` (CBTRN03C-016).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming `tran_type` when `code` holds a NUL or non-ASCII byte,
    /// which no row can hold, or naming the column of a stored value that does not fit its
    /// picture; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the read
    /// fails, which a caller must never take for a missing row (CBTRN03C-041).
    fn find_by_id(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<Option<TransactionTypeRecord>, PersistenceError>> + Send;

    /// Every type, in ascending key order, the order of the KSDS: the `C` collation of
    /// `tran_type` compares bytes. Unbounded, for batch callers only.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] naming the column of the first stored value, in key order,
    /// that does not fit its picture; [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] when the read fails.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionTypeRecord>, PersistenceError>> + Send;

    /// Adds `record` as one row, every byte verbatim, `FILLER` included (LOADS-032, E05).
    ///
    /// On a [`PgTx`], a failed insert aborts the whole transaction, so a caller that must go on
    /// after a duplicate runs the insert inside a [`PgTx::savepoint`] and rolls back to it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when a row already holds the code (constraint
    /// `transaction_types_pkey`); [`PersistenceError::Decode`] when a field holds a NUL or
    /// non-ASCII byte, before anything is written; [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] when the insert fails otherwise.
    fn insert(
        &mut self,
        record: &TransactionTypeRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Replaces the whole table with `records`, the counterpart of the TRANTYPE job's
    /// `DELETE CLUSTER`, `DEFINE CLUSTER` and `REPRO` (`app/jcl/TRANTYPE.jcl:22-61`; LOADS-008):
    /// every row is deleted, then every record is inserted in slice order, and the number of rows
    /// inserted is returned. Once the transaction commits, the table holds exactly `records`
    /// (LOADS-014); on an empty table the delete removes nothing (LOADS-015). No other table is
    /// read or written, schema `db2` included (LOADS-029), and no foreign key joins this table to
    /// another, so the load runs in any order (LOADS-025).
    ///
    /// On a [`PgStore`] the replacement runs in a transaction of its own, which commits before
    /// the call returns and rolls back on any error, leaving the previous contents (LOADS-018).
    /// On a [`PgTx`] it runs in the caller's transaction, which a failed statement aborts.
    ///
    /// The delete is preceded by a `SHARE ROW EXCLUSIVE` lock on the table, held until the
    /// transaction ends, so a concurrent replacement or row write waits for it and then sees its
    /// committed rows: two loads at once leave the input of the one that commits last, never
    /// their union (LOADS-040). Reads do not wait.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a record holds a NUL or non-ASCII byte; every record is
    /// converted before the delete, so nothing is written. [`PersistenceError::Duplicate`] when
    /// two records share a code (constraint `transaction_types_pkey`); the load itself refuses
    /// such input before calling, so this is a backstop (LOADS-018).
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the transaction,
    /// the delete, an insert or the commit fails.
    fn replace_all(
        &mut self,
        records: &[TransactionTypeRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

// Each method starts its work before any `.await`, then returns the future: the statement's own
// future where nothing is left to do, or an `async` block that awaits it. A body that is only an
// `async` block is `clippy::manual_async_fn`, and the `async fn` form that lint asks for would
// leave `Send` out of the signature, where every caller relies on it.
impl TransactionTypeRepository for PgStore {
    fn find_by_id(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<Option<TransactionTypeRecord>, PersistenceError>> + Send {
        select_by_id(self.pool(), code)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionTypeRecord>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &TransactionTypeRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let row = TransactionTypeRow::from_record(record);
        let pool = self.pool();
        async move { insert_one(pool, &row?).await.map(drop) }
    }

    fn replace_all(
        &mut self,
        records: &[TransactionTypeRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // Dropping the transaction on an error rolls it back, leaving the previous contents.
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl TransactionTypeRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<Option<TransactionTypeRecord>, PersistenceError>> + Send {
        select_by_id(self.conn(), code)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TransactionTypeRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &TransactionTypeRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let row = TransactionTypeRow::from_record(record);
        let conn = self.conn();
        async move { insert_one(conn, &row?).await.map(drop) }
    }

    fn replace_all(
        &mut self,
        records: &[TransactionTypeRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), records)
    }
}

/// The row whose `tran_type` equals `code`, as its record.
async fn select_by_id<'e, E>(
    executor: E,
    code: &PicX<2>,
) -> Result<Option<TransactionTypeRecord>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let key = pic_x_to_string(TRAN_TYPE, code)?;
    // The key text is cast to `bpchar`: `sqlx` binds a `String` as `text`, against which the
    // `CHAR(n)` column compares as `text`, its trailing spaces dropped, so a key that ends in
    // spaces would find no row and the primary key could not serve the statement.
    let row = sqlx::query_as!(
        TransactionTypeRow,
        "SELECT tran_type, tran_type_desc, filler FROM transaction_types \
         WHERE tran_type = $1::bpchar",
        key,
    )
    .fetch_optional(executor)
    .await?;
    row.map(TransactionTypeRow::into_record).transpose()
}

/// Every row, in ascending `tran_type` order, as records.
async fn select_all<'e, E>(executor: E) -> Result<Vec<TransactionTypeRecord>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as!(
        TransactionTypeRow,
        "SELECT tran_type, tran_type_desc, filler FROM transaction_types ORDER BY tran_type",
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(TransactionTypeRow::into_record)
        .collect()
}

/// Inserts `row`, returning the number of rows the server reports inserted: one.
async fn insert_one<'e, E>(executor: E, row: &TransactionTypeRow) -> Result<u64, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let done = sqlx::query!(
        "INSERT INTO transaction_types (tran_type, tran_type_desc, filler) VALUES ($1, $2, $3)",
        row.tran_type,
        row.tran_type_desc,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// Deletes every row on `conn`, then inserts `records` in slice order, returning the number of
/// rows inserted. Every record is converted first, so a record codec B cannot store fails the
/// call before the delete, and even a caller's transaction is left as it was.
///
/// The first statement locks the table `SHARE ROW EXCLUSIVE` until the transaction ends, so
/// replacements of `transaction_types` run one at a time and the last to commit leaves its
/// input (LOADS-014, LOADS-040).
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[TransactionTypeRecord],
) -> Result<u64, PersistenceError> {
    let rows = records
        .iter()
        .map(TransactionTypeRow::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    // One replacement of the table at a time (LOADS-014, LOADS-040): this mode conflicts with
    // itself and with the ROW EXCLUSIVE lock of every row write, so a second load waits here
    // until this transaction ends, and its delete then removes the rows this one committed.
    // Readers take ACCESS SHARE and never wait.
    sqlx::query!("LOCK TABLE transaction_types IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM transaction_types")
        .execute(&mut *conn)
        .await?;
    let mut inserted: u64 = 0;
    for row in &rows {
        inserted = inserted.saturating_add(insert_one(&mut *conn, row).await?);
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use copybook::framing::{decode_all, encode_all};
    use copybook::{Framed, PicX};
    use domain::records::TransactionTypeRecord;
    use sqlx::PgPool;

    use super::TransactionTypeRepository;
    use crate::error::{DecodeKind, PersistenceError};
    use crate::{PgStore, PgTx};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The primary key every duplicate names.
    const PKEY: &str = "transaction_types_pkey";

    /// SQLSTATE `in_failed_sql_transaction`: a statement after a failed one in the same
    /// transaction.
    const IN_FAILED_SQL_TRANSACTION: &str = "25P02";

    /// The bytes of `trantype.txt` and its lines as codec A decodes them, framing included.
    type Fixture = (Vec<u8>, Vec<Framed<TransactionTypeRecord>>);

    /// `trantype.txt`, read in place, decoded by codec A with its line framing.
    fn trantype_fixture() -> Result<Fixture, Box<dyn StdError>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/trantype.txt");
        let bytes = std::fs::read(path)?;
        let lines = decode_all::<TransactionTypeRecord>(&bytes)?;
        Ok((bytes, lines))
    }

    /// The fixture's seven records, in file order, which is key order.
    fn fixture_records() -> Result<Vec<TransactionTypeRecord>, Box<dyn StdError>> {
        let (_, lines) = trantype_fixture()?;
        Ok(lines.into_iter().map(|line| line.record).collect())
    }

    /// `base` under another code.
    fn with_code(base: &TransactionTypeRecord, code: [u8; 2]) -> TransactionTypeRecord {
        TransactionTypeRecord {
            tran_type: PicX::from_array(code),
            ..base.clone()
        }
    }

    fn duplicate() -> PersistenceError {
        PersistenceError::Duplicate {
            constraint: Some(PKEY.to_owned()),
        }
    }

    /// The Db2 data version, as migration 0006 seeds it, to show that nothing here touches it.
    async fn db2_version(pool: &PgPool) -> Result<Vec<(String, String)>, sqlx::Error> {
        let rows = sqlx::query!(
            "SELECT tr_type, tr_description FROM db2.transaction_type ORDER BY tr_type"
        )
        .fetch_all(pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| (row.tr_type, row.tr_description))
            .collect())
    }

    /// The load on an empty and on a loaded table, the lookups and the ordered read, through the
    /// pool: a rerun leaves identical contents, every byte returns to the fixture's, and the Db2
    /// data version keeps migration 0006's seed.
    // audit: LOADS-008
    // audit: LOADS-014
    // audit: LOADS-015
    // audit: LOADS-029
    // audit: LOADS-032
    // audit: CBTRN03C-015
    // audit: CBTRN03C-016
    // audit: FIXLOAD-008
    #[tokio::test]
    async fn store_replaces_reads_and_reloads_the_vsam_version() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let db2_seed = db2_version(guard.pool()).await?;
        assert!(
            !db2_seed.is_empty(),
            "migration 0006 seeds the Db2 data version"
        );

        let (bytes, lines) = trantype_fixture()?;
        let records: Vec<TransactionTypeRecord> =
            lines.iter().map(|line| line.record.clone()).collect();
        let first = records.first().ok_or("trantype.txt holds no record")?;
        assert_eq!(records.len(), 7);

        assert!(store.find_all().await?.is_empty());
        assert_eq!(store.find_by_id(&first.tran_type).await?, None);

        assert_eq!(store.replace_all(&records).await?, 7);
        let loaded = store.find_all().await?;
        assert_eq!(loaded, records);
        let restored: Vec<Framed<TransactionTypeRecord>> = loaded
            .iter()
            .zip(&lines)
            .map(|(record, line)| Framed {
                record: record.clone(),
                framing: line.framing,
            })
            .collect();
        assert_eq!(encode_all(&restored), bytes, "every byte, FILLER included");

        for record in &records {
            assert_eq!(
                store.find_by_id(&record.tran_type).await?.as_ref(),
                Some(record)
            );
        }
        for absent in [*b"99", *b"00", *b"  ", *b"1 "] {
            assert_eq!(store.find_by_id(&PicX::from_array(absent)).await?, None);
        }

        assert_eq!(store.replace_all(&records).await?, 7);
        assert_eq!(
            store.find_all().await?,
            loaded,
            "a rerun leaves identical contents"
        );

        let subset = records.get(1..3).ok_or("fewer than three records")?;
        assert_eq!(store.replace_all(subset).await?, 2);
        assert_eq!(
            store.find_all().await?,
            subset,
            "no row the input lacks remains"
        );
        assert_eq!(store.find_by_id(&first.tran_type).await?, None);

        assert_eq!(store.replace_all(&[]).await?, 0);
        assert!(store.find_all().await?.is_empty());

        assert_eq!(db2_version(guard.pool()).await?, db2_seed, "E23");
        Ok(())
    }

    /// A duplicate code, on its own or inside a replacement, and a record codec B cannot store
    /// leave the previous contents in place.
    // audit: LOADS-018
    #[tokio::test]
    async fn failed_writes_through_the_store_leave_the_previous_contents() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let records = fixture_records()?;
        let first = records.first().ok_or("trantype.txt holds no record")?;
        store.replace_all(&records).await?;

        assert_eq!(store.insert(first).await, Err(duplicate()));
        let added = with_code(first, *b"08");
        store.insert(&added).await?;
        assert_eq!(
            store.find_by_id(&added.tran_type).await?,
            Some(added.clone())
        );
        let before = store.find_all().await?;
        assert_eq!(before.len(), 8);

        let twice = [first.clone(), first.clone()];
        assert_eq!(store.replace_all(&twice).await, Err(duplicate()));
        assert_eq!(store.find_all().await?, before);

        let unstorable = TransactionTypeRecord {
            filler: PicX::from_array(*b"0000000\0"),
            ..added.clone()
        };
        let invalid = Err(PersistenceError::decode("filler", DecodeKind::InvalidByte));
        assert_eq!(store.insert(&unstorable).await, invalid);
        assert_eq!(
            store.replace_all(&[first.clone(), unstorable]).await,
            Err(PersistenceError::decode("filler", DecodeKind::InvalidByte))
        );
        assert_eq!(
            store.find_by_id(&PicX::from_array(*b"0\0")).await,
            Err(PersistenceError::decode(
                "tran_type",
                DecodeKind::InvalidByte
            ))
        );
        assert_eq!(store.find_all().await?, before);
        Ok(())
    }

    /// Through a caller's transaction: its own writes are visible to it alone until it commits,
    /// a rollback restores the previous contents, a failed insert aborts the transaction, and a
    /// savepoint lets it go on.
    // audit: LOADS-014
    // audit: LOADS-018
    #[tokio::test]
    async fn transaction_executor_runs_inside_the_callers_transaction() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let records = fixture_records()?;
        let first = records.first().ok_or("trantype.txt holds no record")?;
        let subset = records.get(..2).ok_or("fewer than two records")?;
        store.replace_all(&records).await?;

        let mut tx: PgTx<'static> = store.begin().await?;
        assert_eq!(tx.replace_all(subset).await?, 2);
        assert_eq!(tx.find_all().await?, subset);
        assert_eq!(
            store.find_all().await?,
            records,
            "uncommitted work stays invisible"
        );
        tx.rollback().await?;
        assert_eq!(store.find_all().await?, records);

        let mut tx = store.begin().await?;
        assert_eq!(tx.insert(first).await, Err(duplicate()));
        let aborted = tx.find_by_id(&first.tran_type).await;
        assert_eq!(
            aborted.as_ref().map_err(PersistenceError::sqlstate),
            Err(Some(IN_FAILED_SQL_TRANSACTION))
        );
        tx.rollback().await?;

        let mut tx = store.begin().await?;
        let mut savepoint = tx.savepoint().await?;
        assert_eq!(savepoint.insert(first).await, Err(duplicate()));
        savepoint.rollback().await?;
        let added = with_code(first, *b"09");
        tx.insert(&added).await?;
        assert_eq!(tx.find_by_id(&added.tran_type).await?, Some(added.clone()));
        assert_eq!(store.find_by_id(&added.tran_type).await?, None);
        tx.commit().await?;
        assert_eq!(store.find_by_id(&added.tran_type).await?, Some(added));

        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(subset).await?, 2);
        tx.commit().await?;
        assert_eq!(store.find_all().await?, subset);
        Ok(())
    }
}
