//! Repository of table `disclosure_groups` (migration 0001): the KSDS `DISCGRP`, one `CVTRA02Y`
//! `DIS-GROUP-RECORD` per `DIS-GROUP-KEY`, `KEYS(16 0)` and `RECORDSIZE(50 50)`
//! (`DISCGRP.jcl:40-41`). The key is the record's leading 10 + 2 + 4 bytes, so the primary key
//! is (`group_id`, `tran_type_cd`, `tran_cat_cd`), rows CVTRA02Y-001 to CVTRA02Y-003 of
//! `modern/audit/copybooks.md`.
//!
//! # COBOL accesses
//!
//! | Method | COBOL access | Audit rows |
//! |---|---|---|
//! | [`find_by_id`](DisclosureGroupRepository::find_by_id) | CBACT04C's random `READ DISCGRP-FILE INTO DIS-GROUP-RECORD` by `FD-DISCGRP-KEY`: in `1200-GET-INTEREST-RATE` under the account's group, and in `1200-A-GET-DEFAULT-INT-RATE` under group `DEFAULT` (`CBACT04C.cbl:415-460`) | CBACT04C-009, CBACT04C-010, CBACT04C-011, CBACT04C-033 |
//! | [`find_all`](DisclosureGroupRepository::find_all) | every record, for a job that serves those random reads from memory, as `intcalc` does; the interest job opens the file for input only and never writes it (`CBACT04C.cbl:272`) | CBACT04C-029 |
//! | [`insert`](DisclosureGroupRepository::insert) | one record written into the cluster, as REPRO writes each input record (`DISCGRP.jcl:61`) | LOADS-005, LOADS-017 |
//! | [`replace_all`](DisclosureGroupRepository::replace_all) | the `discgrp` load: DELETE CLUSTER, DEFINE CLUSTER, then REPRO of `discgrp.txt` into the empty cluster (`DISCGRP.jcl:22-62`) | LOADS-005, LOADS-014, LOADS-015, LOADS-018, LOADS-032 |
//!
//! # The `DEFAULT` group belongs to the caller
//!
//! `1200-GET-INTEREST-RATE` accepts the `INVALID KEY` status `23`, moves `'DEFAULT'` to
//! `FD-DIS-ACCT-GROUP-ID` and reads again in `1200-A-GET-DEFAULT-INT-RATE`
//! (`CBACT04C.cbl:415-441,443-460`). A method here mirrors one `READ`: a key no row holds is
//! `Ok(None)`, the status `23` of either read, and the repository never substitutes `DEFAULT`
//! for the group it was given. The fallback, its two log lines and the abend when the `DEFAULT`
//! key is missing too belong to `domain::batch::cbact04c`, driven by the `intcalc` job
//! (CBACT04C-010, CBACT04C-011). Every fixture account leaves `ACCT-GROUP-ID` blank, so every
//! fixture lookup takes that fallback (E07).
//!
//! Each key part is bound as the full text of its picture, never trimmed, and compared as
//! `CHAR`: the group as all 10 bytes, so `DEFAULT` with the three spaces the `MOVE` pads it with
//! is the key the second read uses, exactly as `discgrp.txt` and the `group_id` column hold it.
//!
//! # Bytes and the rate
//!
//! Records reach the table only through [`DisclosureGroupRow`] (codec B): the group, the type
//! code and the `FILLER` byte for byte, the 28 `0` characters of every fixture `FILLER`
//! included (LOADS-032, E05), and `DIS-INT-RATE` as the exact `NUMERIC(6,2)` value of its
//! `Rate<4>` (CVTRA02Y-004). This module performs no arithmetic on the rate.
//!
//! # Errors and redaction
//!
//! A failed statement is a [`PersistenceError`] carrying only an SQLSTATE and a constraint
//! name, never a key or a record value, and nothing here logs or traces. On a [`PgTx`] a failed
//! statement aborts the caller's transaction, so a caller that must go on runs the call inside
//! [`PgTx::savepoint`].

use copybook::{PicU, PicX};
use domain::records::DisclosureGroupRecord;
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::{DisclosureGroupRow, pic_u_to_i64, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column of `DIS-ACCT-GROUP-ID PIC X(10)`, the first key part (CVTRA02Y-001).
const GROUP_ID: &str = "group_id";

/// Column of `DIS-TRAN-TYPE-CD PIC X(02)`, the second key part (CVTRA02Y-002).
const TRAN_TYPE_CD: &str = "tran_type_cd";

/// Column of `DIS-TRAN-CAT-CD PIC 9(04)`, the third key part (CVTRA02Y-003).
const TRAN_CAT_CD: &str = "tran_cat_cd";

/// Access to table `disclosure_groups`, the interest rate of each (account group, transaction
/// type, transaction category) key.
///
/// Implemented by [`PgStore`], where each call commits on its own and
/// [`replace_all`](Self::replace_all) opens a transaction of its own, and by [`PgTx`], where
/// every call runs inside the caller's transaction.
pub trait DisclosureGroupRepository: Send {
    /// The record whose key is (`group_id`, `type_cd`, `cat_cd`), or `None` when no row has that
    /// key: CBACT04C's keyed `READ DISCGRP-FILE` and its `INVALID KEY` status `23`
    /// (`CBACT04C.cbl:415-460`, CBACT04C-009).
    ///
    /// The key is compared as given, all 10 group bytes included. A missing key is never
    /// retried under group `DEFAULT`: that fallback is the caller's (CBACT04C-010, E07).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a key part holds a NUL or non-ASCII byte, which no
    /// stored key can hold, or when the stored row does not fit its pictures;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the query fails,
    /// the counterpart of a read status other than `00` and `23` (CBACT04C-033).
    fn find_by_id(
        &mut self,
        group_id: &PicX<10>,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<DisclosureGroupRecord>, PersistenceError>> + Send;

    /// Every record, in ascending key order: group, then type code, then category, the text
    /// parts compared byte by byte. Unbounded, as only batch jobs read the whole table: `intcalc`
    /// serves CBACT04C's keyed reads from it (CBACT04C-029).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the query fails;
    /// [`PersistenceError::Decode`] when a stored row does not fit its pictures.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<DisclosureGroupRecord>, PersistenceError>> + Send;

    /// Writes `record` as one new row, every byte of it kept (LOADS-005, LOADS-032).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when a row already has the record's key, as VSAM refuses a
    /// duplicate key (LOADS-017); [`PersistenceError::Decode`] when a field holds a NUL or
    /// non-ASCII byte, which no `CHAR` column stores byte for byte; [`PersistenceError::Check`]
    /// for a category outside `0 ..= 9999`; [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] when the statement fails otherwise.
    fn insert(
        &mut self,
        record: &DisclosureGroupRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Replaces the table's contents with `records`, the `discgrp` load: every row is deleted,
    /// then every record is inserted in slice order, and the number of rows inserted is returned
    /// (LOADS-005, LOADS-014). An empty table is not an error (LOADS-015). On [`PgStore`] the
    /// delete and the inserts run in a transaction of their own, committed only when all of them
    /// succeed; on [`PgTx`] they run in the caller's transaction.
    ///
    /// Every record is converted before any statement runs, so a record the table cannot store
    /// leaves the contents untouched.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a record holds a NUL or non-ASCII byte;
    /// [`PersistenceError::Duplicate`] when two records share a key; [`PersistenceError::Check`],
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when a statement or
    /// the commit fails. On [`PgStore`] the transaction then rolls back and the previous contents
    /// remain (LOADS-018).
    fn replace_all(
        &mut self,
        records: &[DisclosureGroupRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

impl DisclosureGroupRepository for PgStore {
    fn find_by_id(
        &mut self,
        group_id: &PicX<10>,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<DisclosureGroupRecord>, PersistenceError>> + Send {
        select_by_id(self.pool(), group_id, type_cd, cat_cd)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<DisclosureGroupRecord>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &DisclosureGroupRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_record(self.pool(), record)
    }

    fn replace_all(
        &mut self,
        records: &[DisclosureGroupRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl DisclosureGroupRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        group_id: &PicX<10>,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<DisclosureGroupRecord>, PersistenceError>> + Send {
        select_by_id(self.conn(), group_id, type_cd, cat_cd)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<DisclosureGroupRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &DisclosureGroupRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_record(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[DisclosureGroupRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), records)
    }
}

/// The keyed read of [`DisclosureGroupRepository::find_by_id`], each key part bound as the
/// full text or value of its column.
///
/// The two text parts are cast to `bpchar`, never compared as `text`: a `text` operand casts the
/// column to `text`, which drops its trailing spaces, so the padded `DEFAULT` key would match no
/// row, and the primary key could not serve the read.
async fn select_by_id<'e, E: PgExecutor<'e>>(
    executor: E,
    group_id: &PicX<10>,
    type_cd: &PicX<2>,
    cat_cd: PicU<4>,
) -> Result<Option<DisclosureGroupRecord>, PersistenceError> {
    let group_id = pic_x_to_string(GROUP_ID, group_id)?;
    let type_cd = pic_x_to_string(TRAN_TYPE_CD, type_cd)?;
    let cat_cd = pic_u_to_i64(TRAN_CAT_CD, cat_cd)?;
    let row = sqlx::query_as!(
        DisclosureGroupRow,
        "SELECT group_id, tran_type_cd, tran_cat_cd, dis_int_rate, filler \
         FROM disclosure_groups \
         WHERE group_id = $1::bpchar AND tran_type_cd = $2::bpchar AND tran_cat_cd = $3",
        group_id,
        type_cd,
        cat_cd,
    )
    .fetch_optional(executor)
    .await?;
    row.map(DisclosureGroupRow::into_record).transpose()
}

/// The whole-table read of [`DisclosureGroupRepository::find_all`], in primary-key order.
async fn select_all<'e, E: PgExecutor<'e>>(
    executor: E,
) -> Result<Vec<DisclosureGroupRecord>, PersistenceError> {
    let rows = sqlx::query_as!(
        DisclosureGroupRow,
        "SELECT group_id, tran_type_cd, tran_cat_cd, dis_int_rate, filler \
         FROM disclosure_groups \
         ORDER BY group_id, tran_type_cd, tran_cat_cd",
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(DisclosureGroupRow::into_record)
        .collect()
}

/// [`DisclosureGroupRepository::insert`]: `record` converted through codec B, then written.
async fn insert_record<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &DisclosureGroupRecord,
) -> Result<(), PersistenceError> {
    let row = DisclosureGroupRow::from_record(record)?;
    insert_one(executor, &row).await?;
    Ok(())
}

/// The single-row insert, returning the number of rows it wrote.
async fn insert_one<'e, E: PgExecutor<'e>>(
    executor: E,
    row: &DisclosureGroupRow,
) -> Result<u64, PersistenceError> {
    let done = sqlx::query!(
        "INSERT INTO disclosure_groups \
         (group_id, tran_type_cd, tran_cat_cd, dis_int_rate, filler) \
         VALUES ($1, $2, $3, $4, $5)",
        row.group_id,
        row.tran_type_cd,
        row.tran_cat_cd,
        row.dis_int_rate,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// [`DisclosureGroupRepository::replace_all`] on one connection, inside the transaction the
/// caller holds on it: convert every record, delete every row, then insert the rows in slice
/// order and count them.
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[DisclosureGroupRecord],
) -> Result<u64, PersistenceError> {
    let rows = records
        .iter()
        .map(DisclosureGroupRow::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    sqlx::query!("DELETE FROM disclosure_groups")
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
    use std::fs;

    use copybook::{PicU, PicX};
    use domain::records::DisclosureGroupRecord;
    use rust_decimal::Decimal;

    use super::DisclosureGroupRepository;
    use crate::PgStore;
    use crate::error::{DecodeKind, PersistenceError};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Records in `discgrp.txt`: 17 for each of the groups `A000000000`, `DEFAULT` and `ZEROAPR`.
    const FIXTURE_RECORDS: usize = 51;

    /// The constraint behind the primary key of migration 0001.
    const PRIMARY_KEY: &str = "disclosure_groups_pkey";

    /// The 51 records of `discgrp.txt`, read in place and decoded by codec A in file order.
    fn fixture() -> Result<Vec<DisclosureGroupRecord>, Box<dyn StdError>> {
        let path = test_support::repo_root().join("app/data/ASCII/discgrp.txt");
        let lines = copybook::framing::decode_all::<DisclosureGroupRecord>(&fs::read(path)?)?;
        Ok(lines.into_iter().map(|line| line.record).collect())
    }

    /// `records` in ascending key order: group, type code, then category.
    fn in_key_order(records: &[DisclosureGroupRecord]) -> Vec<DisclosureGroupRecord> {
        let mut sorted = records.to_vec();
        sorted.sort_by(|a, b| {
            a.acct_group_id
                .cmp(&b.acct_group_id)
                .then_with(|| a.tran_type_cd.cmp(&b.tran_type_cd))
                .then_with(|| a.tran_cat_cd.cmp(&b.tran_cat_cd))
        });
        sorted
    }

    /// The group `MOVE 'DEFAULT' TO FD-DIS-ACCT-GROUP-ID` leaves: the seven letters and three
    /// spaces (`CBACT04C.cbl:437`).
    fn default_group() -> PicX<10> {
        PicX::moved_from(b"DEFAULT")
    }

    /// A record whose group no fixture line holds, for writes the fixture must not contain.
    fn seeded(template: &DisclosureGroupRecord) -> DisclosureGroupRecord {
        DisclosureGroupRecord {
            acct_group_id: PicX::moved_from(b"SEEDED"),
            ..template.clone()
        }
    }

    /// The `discgrp` load on a freshly migrated, empty table, the keyed reads of CBACT04C over
    /// every loaded key, the `DEFAULT` keys the fallback reads, misses, a duplicate insert and a
    /// rerun of the load over a table holding a row the input lacks (CBACT04C-009, CBACT04C-010,
    /// LOADS-005, LOADS-014, LOADS-015, LOADS-017, LOADS-032).
    #[tokio::test]
    async fn the_load_replaces_the_table_and_every_key_reads_back() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let records = fixture()?;
        assert_eq!(records.len(), FIXTURE_RECORDS);
        let expected = in_key_order(&records);

        assert!(store.find_all().await?.is_empty());
        assert_eq!(store.replace_all(&records).await?, 51);
        let loaded = store.find_all().await?;
        assert_eq!(loaded, expected);
        assert!(
            loaded
                .iter()
                .all(|record| record.filler.as_bytes() == &[b'0'; 28])
        );

        for record in &records {
            let found = store
                .find_by_id(
                    &record.acct_group_id,
                    &record.tran_type_cd,
                    record.tran_cat_cd,
                )
                .await?;
            assert_eq!(found.as_ref(), Some(record));
        }

        // The two `DEFAULT` keys every fixture lookup falls back to: `discgrp.txt:18` and `:25`.
        // TRUNCATION: none survives. The conversion is checked: `DIS-TRAN-CAT-CD PIC 9(04)` holds
        // 1 exactly, and `?` refuses a value above 9999 instead of wrapping.
        let category = PicU::<4>::try_from(1_u64)?;
        let purchase = store
            .find_by_id(&default_group(), &PicX::from_array(*b"01"), category)
            .await?
            .ok_or("DEFAULT/01/0001 is missing")?;
        assert_eq!(purchase.int_rate.as_decimal(), Decimal::new(1_500, 2));
        let zero_rate = store
            .find_by_id(&default_group(), &PicX::from_array(*b"03"), category)
            .await?
            .ok_or("DEFAULT/03/0001 is missing")?;
        assert!(zero_rate.int_rate.is_zero());

        // The blank `ACCT-GROUP-ID` of every fixture account names no row: `Ok(None)`, never a
        // `DEFAULT` row in its place (E07). Neither does an unknown type or category.
        let misses = [
            (PicX::spaces(), *b"01", 1_u64),
            (PicX::moved_from(b"DEFAULT"), *b"99", 1),
            (PicX::moved_from(b"A000000000"), *b"01", 9_999),
        ];
        for (group, type_cd, cat_cd) in misses {
            // TRUNCATION: none survives. The conversion is checked: `DIS-TRAN-CAT-CD PIC 9(04)`
            // holds 1 and 9999 exactly, and `?` refuses a value above 9999 instead of wrapping.
            let cat_cd = PicU::<4>::try_from(cat_cd)?;
            let found = store
                .find_by_id(&group, &PicX::from_array(type_cd), cat_cd)
                .await?;
            assert_eq!(found, None);
        }

        let extra = seeded(&purchase);
        store.insert(&extra).await?;
        assert_eq!(
            store
                .find_by_id(&extra.acct_group_id, &extra.tran_type_cd, extra.tran_cat_cd)
                .await?
                .as_ref(),
            Some(&extra)
        );
        let duplicate = store.insert(&purchase).await;
        assert_eq!(
            duplicate,
            Err(PersistenceError::Duplicate {
                constraint: Some(PRIMARY_KEY.to_owned()),
            })
        );

        assert_eq!(store.replace_all(&records).await?, 51);
        assert_eq!(store.find_all().await?, expected);
        assert_eq!(
            store
                .find_by_id(&extra.acct_group_id, &extra.tran_type_cd, extra.tran_cat_cd)
                .await?,
            None
        );
        Ok(())
    }

    /// On a [`crate::PgTx`] every call runs in the caller's transaction: a replacement is seen
    /// inside it, and rollback or commit decides what remains. A record no column can store
    /// leaves the table untouched, and a key no column can hold is refused before any query
    /// (LOADS-014, LOADS-018).
    #[tokio::test]
    async fn a_transaction_owns_its_replacement_and_unstorable_input_changes_nothing() -> TestResult
    {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let records = fixture()?;
        assert_eq!(store.replace_all(&records).await?, 51);
        // TRUNCATION: none survives. The conversion is checked: `DIS-TRAN-CAT-CD PIC 9(04)` holds
        // 1 exactly, and `?` refuses a value above 9999 instead of wrapping.
        let category = PicU::<4>::try_from(1_u64)?;
        let type_cd = PicX::from_array(*b"01");

        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&[]).await?, 0);
        assert!(tx.find_all().await?.is_empty());
        assert_eq!(
            tx.find_by_id(&default_group(), &type_cd, category).await?,
            None
        );
        tx.rollback().await?;
        assert_eq!(store.find_all().await?, in_key_order(&records));

        let kept: Vec<DisclosureGroupRecord> = records.iter().take(2).cloned().collect();
        let extra = records.first().map(seeded).ok_or("discgrp.txt is empty")?;
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&kept).await?, 2);
        tx.insert(&extra).await?;
        assert_eq!(
            tx.find_by_id(&extra.acct_group_id, &extra.tran_type_cd, extra.tran_cat_cd)
                .await?
                .as_ref(),
            Some(&extra)
        );
        tx.commit().await?;
        let mut committed = kept.clone();
        committed.push(extra.clone());
        assert_eq!(store.find_all().await?, in_key_order(&committed));

        let unstorable = DisclosureGroupRecord {
            acct_group_id: PicX::from_array(*b"A00000000\xC3"),
            ..extra
        };
        let invalid_group = Err(PersistenceError::decode(
            "group_id",
            DecodeKind::InvalidByte,
        ));
        assert_eq!(
            store.replace_all(&[unstorable.clone()]).await,
            invalid_group.clone().map(|()| 0)
        );
        assert_eq!(store.insert(&unstorable).await, invalid_group.clone());
        assert_eq!(
            store
                .find_by_id(
                    &unstorable.acct_group_id,
                    &unstorable.tran_type_cd,
                    unstorable.tran_cat_cd,
                )
                .await,
            invalid_group.map(|()| None)
        );
        assert_eq!(store.find_all().await?, in_key_order(&committed));
        Ok(())
    }
}
