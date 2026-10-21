//! Repository of table `card_xref`, the card cross-reference `CARD-XREF-RECORD` of copybook
//! `CVACT03Y` (migration 0001; CVACT03Y-001 to CVACT03Y-004).
//!
//! The KSDS `CARDXREF` is keyed by `XREF-CARD-NUM` (`KEYS(16 0)`, `XREFFILE.jcl:43`), which the
//! table keeps as its primary key `card_xref_pkey` (LOADS-003). Its alternate index on
//! `XREF-ACCT-ID` (`KEYS(11,25)` `NONUNIQUEKEY`, `XREFFILE.jcl:72-75`), read online through the
//! path `CXACAIX`, is replaced by the non-unique index `card_xref_acct_id_idx` on `acct_id`
//! (LOADS-023, LOADS-024).
//!
//! | COBOL access | Method |
//! |---|---|
//! | Keyed read by card number: CBTRN02C `1500-A-LOOKUP-XREF` (`CBTRN02C.cbl:380-391`), CBTRN03C `1500-A-LOOKUP-XREF` (`CBTRN03C.cbl:484-492`), COTRN02C `READ-CCXREF-FILE` (`COTRN02C.cbl:609-637`), COPAUA0C `5100-READ-XREF-RECORD` (`COPAUA0C.cbl:472-514`) | [`CardXrefRepository::find_by_id`] |
//! | Keyed read through the account path `CXACAIX`: COACTVWC and COACTUPC `9200-GETCARDXREF-BYACCT` (`COACTVWC.cbl:723-770`, `COACTUPC.cbl:3650-3698`), COBIL00C `READ-CXACAIX-FILE` (`COBIL00C.cbl:408-436`), COTRN02C `READ-CXACAIX-FILE` (`COTRN02C.cbl:576-604`), COPAUS0C `GETCARDXREF-BYACCT` (`COPAUS0C.cbl:812-862`), CBACT04C `1110-GET-XREF-DATA`, `READ … KEY IS FD-XREF-ACCT-ID` (`CBACT04C.cbl:393-398`) | [`CardXrefRepository::find_by_acct_id`] |
//! | Every record sharing one alternate key, which tells a `DUPKEY` read apart (COACTVWC-039) | [`CardXrefRepository::find_all_by_acct_id`] |
//! | Sequential read in key order: CBSTM03B `2000-XREFFILE-PROC` (`CBSTM03B.CBL:157-175`), driven by CREASTMT | [`CardXrefRepository::find_all`] |
//! | IDCAMS `DELETE`, `DEFINE CLUSTER` and `REPRO` of XREFFILE (`XREFFILE.jcl:25-29,39-52,64`), batch `xreffile` | [`CardXrefRepository::replace_all`] |
//!
//! # Alternate-index order
//!
//! A keyed `READ` through a non-unique alternate index returns the first base record that the
//! alternate-index record lists for the key. `BLDINDEX` (`XREFFILE.jcl:100-102`) lists the prime
//! keys of a shared alternate key in ascending order, and no in-scope program adds a
//! cross-reference afterwards, so the first record is the one with the lowest card number.
//! [`find_by_acct_id`](CardXrefRepository::find_by_acct_id) and `select_first_by_acct_id`,
//! which the account chain of `accounts.rs` shares, therefore order by `card_num` and keep the
//! first row. `card_num` is declared `COLLATE "C"` in the migration, so that order is bytewise,
//! and no statement here names a collation, which would keep the planner off
//! `card_xref_pkey` and `card_xref_acct_id_idx`.
//!
//! # Cardholder data
//!
//! `card_num` holds the full card number (PIC-024). A [`CardNumber`] reaches SQL only as a bind
//! value converted by codec B's `rows` helpers from [`CardNumber::expose`], records are built only
//! by [`CardXrefRow::into_record`], and no record, row or key is formatted into an error. Errors
//! are the value-free [`PersistenceError`] variants, and this module emits no tracing (Cardholder
//! Data Redaction).

use std::future::Future;

use copybook::PicU;
use domain::records::card_xref::CardXrefRecord;
use domain::sensitive::CardNumber;
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::CardXrefRow;
use crate::rows::{pic_u_to_i64, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column of `XREF-CARD-NUM`, named by a key conversion error.
const CARD_NUM: &str = "card_num";
/// Column of `XREF-ACCT-ID`, named by a key conversion error.
const ACCT_ID: &str = "acct_id";

/// The `card_xref` table, implemented by [`PgStore`] (each statement on a pooled connection,
/// committed on its own) and [`PgTx`] (inside the caller's transaction).
///
/// Every method takes and returns `domain` records and `copybook` keys and converts them through
/// codec B, [`CardXrefRow`], byte for byte: nothing is trimmed, padded or defaulted.
pub trait CardXrefRepository: Send {
    /// The cross-reference whose `XREF-CARD-NUM` is `card`, or `None` when no row has the key
    /// (CICS `NOTFND`, VSAM `INVALID KEY`).
    ///
    /// The keyed `READ` of CBTRN02C and CBTRN03C `1500-A-LOOKUP-XREF`, COTRN02C
    /// `READ-CCXREF-FILE` and COPAUA0C `5100-READ-XREF-RECORD`, served by `card_xref_pkey`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for column `card_num` when `card` holds a NUL or non-ASCII
    /// byte, which no row can hold, or for the first column of the stored row that does not fit
    /// its picture; [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// [`PersistenceError::Database`] for any other failure of the statement.
    fn find_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send;

    /// The first cross-reference of account `acct_id` in alternate-index order, the one with
    /// the lowest card number, or `None` when no row has the account (CICS `NOTFND`).
    ///
    /// The keyed `READ` through `CXACAIX` of COACTVWC and COACTUPC `9200-GETCARDXREF-BYACCT`,
    /// COBIL00C and COTRN02C `READ-CXACAIX-FILE` and COPAUS0C `GETCARDXREF-BYACCT`, and CBACT04C
    /// `1110-GET-XREF-DATA`, served by `card_xref_acct_id_idx`. When other rows share the
    /// account, CICS still returns this record but with the `DUPKEY` response, which this method
    /// does not report: a caller whose program branches on that response, as COACTVWC-039
    /// records, reads [`find_all_by_acct_id`](Self::find_all_by_acct_id) instead.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for the first column of the stored row that does not fit its
    /// picture; [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// [`PersistenceError::Database`] for any other failure of the statement.
    fn find_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send;

    /// Every cross-reference of account `acct_id`, in card-number order; empty when no row has
    /// the account.
    ///
    /// Unbounded: an account holds a handful of cards, and no caller passes a size of its own
    /// (Bounded Pagination binds the API collections, which page `cards`, not this table). Each
    /// fixture account has exactly one row (COACTVWC-039).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for the first column, in row order and then migration
    /// order, whose stored value does not fit its picture; [`PersistenceError::Unavailable`]
    /// when the database cannot be reached; [`PersistenceError::Database`] for any other failure
    /// of the statement.
    fn find_all_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send;

    /// Every cross-reference in key order, the sequential `READ` of CBSTM03B
    /// `2000-XREFFILE-PROC` that drives CREASTMT.
    ///
    /// Unbounded, for the batch jobs and tests only; no API endpoint reaches it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for the first column, in row order and then migration
    /// order, whose stored value does not fit its picture; [`PersistenceError::Unavailable`]
    /// when the database cannot be reached; [`PersistenceError::Database`] for any other failure
    /// of the statement.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send;

    /// Writes `record` as a new row, every field as given: the 14 `FILLER` bytes included, which
    /// codec A supplies as spaces for the 36-byte `cardxref.txt` bodies (E04).
    ///
    /// On a [`PgTx`], a failed insert aborts the whole transaction; a caller that must go on
    /// after an expected failure, such as a duplicate key, runs the insert inside a
    /// [`PgTx::savepoint`] and rolls back to it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] with constraint `card_xref_pkey` when a row already has
    /// the card number; [`PersistenceError::Decode`] for column `card_num` or `filler` when the
    /// field holds a NUL or non-ASCII byte; [`PersistenceError::Unavailable`] when the database
    /// cannot be reached; [`PersistenceError::Database`] for any other failure of the statement.
    fn insert(
        &mut self,
        record: &CardXrefRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Replaces the table's contents with `records`: deletes every row, then inserts each record
    /// in slice order, and returns the number of rows inserted. This is IDCAMS `DELETE`,
    /// `DEFINE CLUSTER` and `REPRO` into the new, empty cluster (LOADS-014); an empty table is
    /// not an error (LOADS-015).
    ///
    /// The delete and the inserts form one unit: [`PgStore`] runs them in a transaction of its
    /// own and commits it, [`PgTx`] inside the caller's transaction. No foreign key joins
    /// `card_xref` to another table, so the load succeeds whatever any other load did first
    /// (LOADS-025).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when two records share a card number,
    /// [`PersistenceError::Decode`] when a record holds a byte no row can store, and
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when a statement or
    /// the commit fails. On [`PgStore`] the transaction is then rolled back and the previous
    /// contents remain (LOADS-018); on [`PgTx`] the caller's transaction is aborted.
    fn replace_all(
        &mut self,
        records: &[CardXrefRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

impl CardXrefRepository for PgStore {
    fn find_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send {
        select_by_card(self.pool(), card)
    }

    fn find_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send {
        select_first_by_acct_id(self.pool(), acct_id)
    }

    fn find_all_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send {
        select_all_by_acct_id(self.pool(), acct_id)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &CardXrefRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_one(self.pool(), record)
    }

    fn replace_all(
        &mut self,
        records: &[CardXrefRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // Dropping `tx` on an error rolls the delete and every insert back together.
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl CardXrefRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send {
        select_by_card(self.conn(), card)
    }

    fn find_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Option<CardXrefRecord>, PersistenceError>> + Send {
        select_first_by_acct_id(self.conn(), acct_id)
    }

    fn find_all_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send {
        select_all_by_acct_id(self.conn(), acct_id)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &CardXrefRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_one(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[CardXrefRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), records)
    }
}

/// The cross-reference of `card`, bound through codec B from [`CardNumber::expose`].
async fn select_by_card<'e, E: PgExecutor<'e>>(
    executor: E,
    card: &CardNumber,
) -> Result<Option<CardXrefRecord>, PersistenceError> {
    let card_num = pic_x_to_string(CARD_NUM, card.expose())?;
    let row = sqlx::query_as!(
        CardXrefRow,
        "SELECT card_num, cust_id, acct_id, filler FROM card_xref WHERE card_num = $1",
        card_num
    )
    .fetch_optional(executor)
    .await?;
    row.map(CardXrefRow::into_record).transpose()
}

/// The first cross-reference of account `acct_id` in alternate-index order, the lowest card
/// number, or `None` when no row has the account. The one statement of the `CXACAIX` read:
/// [`CardXrefRepository::find_by_acct_id`] and the account chain of `accounts.rs` both run it.
///
/// # Errors
///
/// [`PersistenceError::Decode`] for the first column of the stored row that does not fit its
/// picture; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the
/// statement fails.
pub(super) async fn select_first_by_acct_id<'e, E: PgExecutor<'e>>(
    executor: E,
    acct_id: PicU<11>,
) -> Result<Option<CardXrefRecord>, PersistenceError> {
    let acct_id = pic_u_to_i64(ACCT_ID, acct_id)?;
    let row = sqlx::query_as!(
        CardXrefRow,
        "SELECT card_num, cust_id, acct_id, filler FROM card_xref WHERE acct_id = $1 \
         ORDER BY card_num LIMIT 1",
        acct_id
    )
    .fetch_optional(executor)
    .await?;
    row.map(CardXrefRow::into_record).transpose()
}

/// Every cross-reference of account `acct_id`, in card-number order.
async fn select_all_by_acct_id<'e, E: PgExecutor<'e>>(
    executor: E,
    acct_id: PicU<11>,
) -> Result<Vec<CardXrefRecord>, PersistenceError> {
    let acct_id = pic_u_to_i64(ACCT_ID, acct_id)?;
    let rows = sqlx::query_as!(
        CardXrefRow,
        "SELECT card_num, cust_id, acct_id, filler FROM card_xref WHERE acct_id = $1 \
         ORDER BY card_num",
        acct_id
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(CardXrefRow::into_record).collect()
}

/// Every cross-reference, in card-number order.
async fn select_all<'e, E: PgExecutor<'e>>(
    executor: E,
) -> Result<Vec<CardXrefRecord>, PersistenceError> {
    let rows = sqlx::query_as!(
        CardXrefRow,
        "SELECT card_num, cust_id, acct_id, filler FROM card_xref ORDER BY card_num"
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(CardXrefRow::into_record).collect()
}

/// Inserts the row of `record`, every column bound from codec B.
async fn insert_one<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &CardXrefRecord,
) -> Result<(), PersistenceError> {
    let row = CardXrefRow::from_record(record)?;
    sqlx::query!(
        "INSERT INTO card_xref (card_num, cust_id, acct_id, filler) VALUES ($1, $2, $3, $4)",
        row.card_num,
        row.cust_id,
        row.acct_id,
        row.filler
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// Deletes every row, then inserts `records` in slice order on `conn`, whose transaction the
/// caller owns; returns the number of records inserted.
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[CardXrefRecord],
) -> Result<u64, PersistenceError> {
    sqlx::query!("DELETE FROM card_xref")
        .execute(&mut *conn)
        .await?;
    let mut inserted: u64 = 0;
    for record in records {
        insert_one(&mut *conn, record).await?;
        inserted = inserted.saturating_add(1);
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use copybook::framing::decode_all;
    use copybook::{PicU, PicX};
    use domain::records::card_xref::CardXrefRecord;
    use domain::sensitive::CardNumber;
    use test_support::postgres::start_postgres;

    use super::{CardXrefRepository, select_first_by_acct_id};
    use crate::error::PersistenceError;
    use crate::{PgStore, PgTx};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The 14 spaces codec A supplies for the `FILLER` the fixture bodies omit.
    const BLANK_FILLER: [u8; 14] = *b"              ";

    /// An account key that no fixture record and no record built here holds.
    const ABSENT_ACCOUNT: u64 = 99_999_999_999;

    /// `app/data/ASCII/cardxref.txt`, read in place and decoded by codec A.
    fn fixture_records() -> Result<Vec<CardXrefRecord>, Box<dyn StdError>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/cardxref.txt");
        let bytes = std::fs::read(path)?;
        Ok(decode_all::<CardXrefRecord>(&bytes)?
            .into_iter()
            .map(|framed| framed.record)
            .collect())
    }

    /// A cross-reference with an invented card number, which no fixture line holds.
    fn xref(
        card: [u8; 16],
        cust_id: u64,
        acct_id: u64,
    ) -> Result<CardXrefRecord, Box<dyn StdError>> {
        Ok(CardXrefRecord {
            card_num: CardNumber::new(PicX::from_array(card)),
            cust_id: PicU::new(cust_id)?,
            acct_id: PicU::new(acct_id)?,
            filler: PicX::from_array(BLANK_FILLER),
        })
    }

    /// `records` in card-number order, the order of the primary key.
    fn by_card(records: &[CardXrefRecord]) -> Vec<CardXrefRecord> {
        let mut sorted = records.to_vec();
        sorted.sort_by(|a, b| a.card_num.cmp(&b.card_num));
        sorted
    }

    /// Whether `text` holds four consecutive digits, the shortest run that could be part of a
    /// card number; no error text has one.
    fn has_digit_run(text: &str) -> bool {
        text.as_bytes()
            .windows(4)
            .any(|run| run.iter().all(u8::is_ascii_digit))
    }

    /// The duplicate-key error of the primary key, its texts free of any digit run.
    fn assert_duplicate_key(error: Option<&PersistenceError>) {
        assert_eq!(
            error,
            Some(&PersistenceError::Duplicate {
                constraint: Some("card_xref_pkey".to_owned()),
            })
        );
        for text in error.iter().flat_map(|e| [e.to_string(), format!("{e:?}")]) {
            assert!(!has_digit_run(&text), "{text}");
        }
    }

    // Records are compared with `assert!`: their `Debug` already redacts the card number, and a
    // failure message then names the property rather than the values.

    // audit: LOADS-003
    // audit: LOADS-014
    // audit: LOADS-015
    // audit: LOADS-024
    #[tokio::test]
    async fn fixture_load_runs_twice_and_every_read_path_finds_each_record() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let records = fixture_records()?;
        assert_eq!(records.len(), 50);

        // Freshly migrated, then loaded: both runs leave exactly the input set.
        for _ in 0..2 {
            assert_eq!(store.replace_all(&records).await?, 50);
            assert!(
                store.find_all().await? == by_card(&records),
                "find_all returns every loaded record in card-number order"
            );
        }

        for record in &records {
            assert!(
                store.find_by_id(&record.card_num).await?.as_ref() == Some(record),
                "find_by_id returns the record of its card number"
            );
            assert!(
                store.find_by_acct_id(record.acct_id).await?.as_ref() == Some(record),
                "find_by_acct_id returns the one record of its account"
            );
            assert!(
                store.find_all_by_acct_id(record.acct_id).await? == [record.clone()],
                "each fixture account has exactly one cross-reference"
            );
        }

        let absent_card = CardNumber::new(PicX::from_array(*b"0000000000000000"));
        assert!(store.find_by_id(&absent_card).await?.is_none());
        let absent_account = PicU::new(ABSENT_ACCOUNT)?;
        assert!(store.find_by_acct_id(absent_account).await?.is_none());
        assert!(store.find_all_by_acct_id(absent_account).await?.is_empty());
        Ok(())
    }

    // audit: LOADS-023
    #[tokio::test]
    async fn an_account_with_several_cards_reads_the_lowest_card_number_first() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let shared = 12_345_678_901;
        // Inserted out of key order, so the order read back comes from the statements.
        let records = [
            xref(*b"7000000000000003", 3, shared)?,
            xref(*b"1000000000000009", 9, shared)?,
            xref(*b"5000000000000001", 1, shared)?,
            xref(*b"0900000000000000", 4, 98_765_432_109)?,
        ];
        for record in &records {
            store.insert(record).await?;
        }
        let shared = PicU::new(shared)?;
        let [highest, lowest, middle, other] = records;

        assert!(
            store.find_by_acct_id(shared).await? == Some(lowest.clone()),
            "the alternate-key read returns the lowest card number"
        );
        assert!(
            store.find_all_by_acct_id(shared).await?
                == [lowest.clone(), middle.clone(), highest.clone()],
            "every record of the account, in card-number order"
        );
        assert!(
            store.find_all().await?
                == [
                    other.clone(),
                    lowest.clone(),
                    middle.clone(),
                    highest.clone()
                ],
            "the whole table in card-number order"
        );
        assert!(
            store.find_by_id(&middle.card_num).await? == Some(middle.clone()),
            "the keyed read ignores the shared account"
        );

        // The same statements inside a transaction, and the one the account chain shares.
        let mut tx: PgTx<'static> = store.begin().await?;
        assert!(
            select_first_by_acct_id(tx.conn(), shared).await? == Some(lowest.clone()),
            "the shared statement keeps the lowest card number"
        );
        assert!(tx.find_by_acct_id(shared).await? == Some(lowest.clone()));
        assert!(tx.find_all_by_acct_id(shared).await?.len() == 3);
        assert!(tx.find_by_id(&other.card_num).await? == Some(other.clone()));
        assert!(tx.find_all().await?.len() == 4);
        tx.rollback().await?;
        Ok(())
    }

    #[tokio::test]
    async fn a_duplicate_card_number_is_typed_and_a_savepoint_keeps_the_transaction() -> TestResult
    {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let first = xref(*b"4000123412341234", 1, 1)?;
        let same_card = xref(*b"4000123412341234", 2, 2)?;
        let second = xref(*b"4000123412349999", 3, 3)?;

        store.insert(&first).await?;
        assert_duplicate_key(store.insert(&same_card).await.err().as_ref());

        let mut tx = store.begin().await?;
        let mut savepoint = tx.savepoint().await?;
        assert_duplicate_key(savepoint.insert(&same_card).await.err().as_ref());
        savepoint.rollback().await?;
        tx.insert(&second).await?;
        tx.commit().await?;

        assert!(
            store.find_all().await? == [first.clone(), second.clone()],
            "the first row stands and the insert after the savepoint committed"
        );
        Ok(())
    }

    // audit: LOADS-014
    // audit: LOADS-018
    #[tokio::test]
    async fn replace_all_keeps_only_its_input_and_fails_as_a_whole() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let a = xref(*b"6000000000000001", 1, 1)?;
        let b = xref(*b"6000000000000002", 2, 2)?;
        let c = xref(*b"6000000000000003", 3, 3)?;

        assert_eq!(store.replace_all(&[b.clone(), a.clone()]).await?, 2);
        assert_eq!(store.replace_all(&[c.clone()]).await?, 1);
        assert!(
            store.find_all().await? == [c.clone()],
            "rows the input lacks are gone"
        );
        assert_eq!(store.replace_all(&[a.clone(), b.clone()]).await?, 2);

        // A failing load deletes and inserts nothing: the previous contents remain.
        assert_duplicate_key(
            store
                .replace_all(&[c.clone(), c.clone()])
                .await
                .err()
                .as_ref(),
        );
        assert!(
            store.find_all().await? == [a.clone(), b.clone()],
            "a failed load leaves the previous contents"
        );

        // On a transaction the replacement is the caller's: rolled back, then committed.
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&[c.clone()]).await?, 1);
        assert!(tx.find_all().await? == [c.clone()]);
        tx.rollback().await?;
        assert!(
            store.find_all().await? == [a.clone(), b.clone()],
            "a rolled-back replacement leaves the previous contents"
        );
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&[c.clone(), a.clone()]).await?, 2);
        tx.commit().await?;
        assert!(
            store.find_all().await? == [a, c],
            "a committed replacement holds exactly its input"
        );

        assert_eq!(store.replace_all(&[]).await?, 0);
        assert!(store.find_all().await?.is_empty());
        Ok(())
    }
}
