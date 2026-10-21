//! [`Db2TransactionTypeCategoryRepository`]: the repository of `db2.transaction_type_category`,
//! the Db2 data version of the transaction categories.
//!
//! Migration `0006_db2_transaction_types.sql` creates the table from `TRNTYCAT.ddl:1-7`
//! (`trc_type_code CHAR(2)`, `trc_type_category CHAR(4)`, `trc_cat_data VARCHAR(50)`, the
//! DCLGEN group of `DCLTRCAT.dcl`) and seeds it once per database with the 18 rows of
//! `DB2LTCAT.ctl:15-37`, as CREADB21 does.
//!
//! - Primary key `x_tran_type_catg` is (`trc_type_code`, `trc_type_category`), the unique index
//!   of `XTRNTYCAT.ddl:1-5`. It serves both reads: the full key for
//!   [`find_by_id`](Db2TransactionTypeCategoryRepository::find_by_id), its leading column for
//!   [`find_all_by_type`](Db2TransactionTypeCategoryRepository::find_all_by_type). The key
//!   columns are `COLLATE "C"` in the migration, so the categories of a type come back in byte
//!   order, as Db2 orders them.
//! - Foreign key `trc_type_code` references `db2.transaction_type (tr_type)` with
//!   `ON DELETE RESTRICT` (`TRNTYCAT.ddl:6-7`).
//!
//! # Role
//!
//! No statement of COTRTUPC, COTRTLIC or COBTUPDT names this table. It is reached only through
//! its foreign key, which refuses to delete a type that still has categories, as each of the
//! seven seeded types does: Db2 `SQLCODE` -532, `Please delete associated child records first:`
//! in COTRTUPC's `9800-DELETE-PROCESSING` (`COTRTUPC.cbl:1624-1649`; COTRTUPC-002,
//! COTRTUPC-033), COTRTLIC's `9300-DELETE-RECORD` (COTRTLIC-050) and COBTUPDT's
//! `10033-DELETE-DB` (COBTUPDT-017, COBTUPDT-026). That refusal arrives through the type
//! repository's delete as [`PersistenceError::Restrict`] naming `trc_type_code`. This repository
//! lets a caller observe that a refused delete left the categories unchanged, and set up the
//! categories of a new type, so that the restricted and the unrestricted delete (COTRTUPC-032)
//! can both be exercised.
//!
//! Nothing here empties or reloads the table: no fixture load reaches it, so the migration's
//! seed survives every reload (AAP 0.3.3.4).
//!
//! # Two data versions (E23)
//!
//! This repository reads and writes only the Db2 version. The VSAM version that `tranrept`
//! reads is `transaction_categories`, reached through the `TransactionCategoryRepository`. No
//! method copies between them: on the mainframe only the daily TRANEXTR did, and it is out of
//! scope, so a maintenance change here never reaches the report lookups.
//!
//! # Values and errors
//!
//! Keys and records convert only through codec B, [`Db2TransactionTypeCategoryRow`]: both keys
//! byte for byte, padding included, and the `VARCHAR(50)` text verbatim, never trimmed, padded or
//! upper-cased. A key byte that no `PostgreSQL` text column can hold, NUL or non-ASCII, fails
//! with [`PersistenceError::Decode`] before any statement runs. Errors carry no key or text
//! (Cardholder Data Redaction), and nothing here logs.
//!
//! The keys are taken by reference: `PicX` is not `Copy`, so clippy's
//! `trivially_copy_pass_by_ref` does not apply to them.

use copybook::PicX;
use domain::records::Db2TransactionCategory;
use sqlx::PgExecutor;

use crate::error::PersistenceError;
use crate::rows::{Db2TransactionTypeCategoryRow, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column of `DCL-TRC-TYPE-CODE PIC X(2)`, named in a key's conversion error.
const TRC_TYPE_CODE: &str = "trc_type_code";

/// Column of `DCL-TRC-TYPE-CATEGORY PIC X(4)`, named in a key's conversion error.
const TRC_TYPE_CATEGORY: &str = "trc_type_category";

/// Reads and inserts of `db2.transaction_type_category`, the Db2 version of the transaction
/// categories (E23), implemented by [`PgStore`] and [`PgTx`].
///
/// On a [`PgTx`] a failed statement aborts the caller's transaction, so a caller that goes on
/// after a failure, such as a duplicate or restricted insert, runs the call inside
/// [`PgTx::savepoint`] and rolls back to it.
pub trait Db2TransactionTypeCategoryRepository: Send {
    /// The category with key (`type_code`, `category`), or `None` when no row holds that key.
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Decode`] for a NUL or non-ASCII byte in either key, which no row can
    ///   hold, or for a stored value that does not fit its column's picture;
    /// - [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// - [`PersistenceError::Database`] for any other database error.
    fn find_by_id(
        &mut self,
        type_code: &PicX<2>,
        category: &PicX<4>,
    ) -> impl Future<Output = Result<Option<Db2TransactionCategory>, PersistenceError>> + Send;

    /// Every category of `type_code`, in ascending byte order of `trc_type_category`; empty when
    /// the type has none.
    ///
    /// The result is not paginated: it is not served to any collection endpoint, and the seed
    /// gives each type at most five categories.
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Decode`] for a NUL or non-ASCII byte in `type_code`, or for the
    ///   first stored row, in that order, whose value does not fit its column's picture;
    /// - [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// - [`PersistenceError::Database`] for any other database error.
    fn find_all_by_type(
        &mut self,
        type_code: &PicX<2>,
    ) -> impl Future<Output = Result<Vec<Db2TransactionCategory>, PersistenceError>> + Send;

    /// Inserts `row`: both keys and the text exactly as the record holds them.
    ///
    /// # Errors
    ///
    /// - [`PersistenceError::Duplicate`] naming `x_tran_type_catg` when a row already holds the
    ///   key;
    /// - [`PersistenceError::Restrict`] naming `trc_type_code` when `db2.transaction_type` holds
    ///   no type with the record's type code;
    /// - [`PersistenceError::Decode`] for a NUL or non-ASCII byte in any field, which no text
    ///   column stores byte for byte;
    /// - [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// - [`PersistenceError::Database`] for any other database error.
    ///
    /// On a [`PgTx`], each of the database errors aborts the transaction unless the call runs
    /// inside a [`PgTx::savepoint`].
    fn insert(
        &mut self,
        row: &Db2TransactionCategory,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;
}

impl Db2TransactionTypeCategoryRepository for PgStore {
    fn find_by_id(
        &mut self,
        type_code: &PicX<2>,
        category: &PicX<4>,
    ) -> impl Future<Output = Result<Option<Db2TransactionCategory>, PersistenceError>> + Send {
        select_by_id(self.pool(), type_code, category)
    }

    fn find_all_by_type(
        &mut self,
        type_code: &PicX<2>,
    ) -> impl Future<Output = Result<Vec<Db2TransactionCategory>, PersistenceError>> + Send {
        select_all_by_type(self.pool(), type_code)
    }

    fn insert(
        &mut self,
        row: &Db2TransactionCategory,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_one(self.pool(), row)
    }
}

impl Db2TransactionTypeCategoryRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        type_code: &PicX<2>,
        category: &PicX<4>,
    ) -> impl Future<Output = Result<Option<Db2TransactionCategory>, PersistenceError>> + Send {
        select_by_id(self.conn(), type_code, category)
    }

    fn find_all_by_type(
        &mut self,
        type_code: &PicX<2>,
    ) -> impl Future<Output = Result<Vec<Db2TransactionCategory>, PersistenceError>> + Send {
        select_all_by_type(self.conn(), type_code)
    }

    fn insert(
        &mut self,
        row: &Db2TransactionCategory,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_one(self.conn(), row)
    }
}

// `sqlx` binds a `String` as `text`. Compared with a `text` value, a `CHAR(n)` column is
// converted to `text`, which drops its trailing spaces: a padded key such as `A   ` would never
// match, and `x_tran_type_catg` would only be scanned in full under a filter. Every key
// parameter is therefore cast to `bpchar`, which keeps the comparison in `CHAR(n)` semantics,
// equal bytes for the equal-width keys a `PicX` holds, and an index condition.

/// The row with the full primary key, through index `x_tran_type_catg`.
async fn select_by_id<'e, E>(
    executor: E,
    type_code: &PicX<2>,
    category: &PicX<4>,
) -> Result<Option<Db2TransactionCategory>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let type_code = pic_x_to_string(TRC_TYPE_CODE, type_code)?;
    let category = pic_x_to_string(TRC_TYPE_CATEGORY, category)?;
    let row = sqlx::query_as!(
        Db2TransactionTypeCategoryRow,
        r#"SELECT trc_type_code, trc_type_category, trc_cat_data
             FROM db2.transaction_type_category
            WHERE trc_type_code = $1::bpchar
              AND trc_type_category = $2::bpchar"#,
        type_code,
        category,
    )
    .fetch_optional(executor)
    .await?;
    row.map(Db2TransactionTypeCategoryRow::into_record)
        .transpose()
}

/// Every row of one type, in key order, through the leading column of `x_tran_type_catg`.
async fn select_all_by_type<'e, E>(
    executor: E,
    type_code: &PicX<2>,
) -> Result<Vec<Db2TransactionCategory>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let type_code = pic_x_to_string(TRC_TYPE_CODE, type_code)?;
    let rows = sqlx::query_as!(
        Db2TransactionTypeCategoryRow,
        r#"SELECT trc_type_code, trc_type_category, trc_cat_data
             FROM db2.transaction_type_category
            WHERE trc_type_code = $1::bpchar
            ORDER BY trc_type_category"#,
        type_code,
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter()
        .map(Db2TransactionTypeCategoryRow::into_record)
        .collect()
}

/// One new row, all three columns.
async fn insert_one<'e, E>(
    executor: E,
    record: &Db2TransactionCategory,
) -> Result<(), PersistenceError>
where
    E: PgExecutor<'e>,
{
    let row = Db2TransactionTypeCategoryRow::from_record(record)?;
    sqlx::query!(
        r#"INSERT INTO db2.transaction_type_category
                  (trc_type_code, trc_type_category, trc_cat_data)
           VALUES ($1, $2, $3)"#,
        row.trc_type_code,
        row.trc_type_category,
        row.trc_cat_data,
    )
    .execute(executor)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::PicX;
    use domain::records::Db2TransactionCategory;

    use super::{Db2TransactionTypeCategoryRepository, TRC_TYPE_CATEGORY, TRC_TYPE_CODE};
    use crate::error::{DecodeKind, PersistenceError};
    use crate::{PgStore, PgTx};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The 18 rows migration 0006 seeds from `DB2LTCAT.ctl:20-37`, in key order: type code,
    /// category, text.
    const SEED: [(&str, &str, &str); 18] = [
        ("01", "0001", "REGULAR SALES DRAFT"),
        ("01", "0002", "REGULAR CASH ADVANCE"),
        ("01", "0003", "CONVENIENCE CHECK DEBIT"),
        ("01", "0004", "ATM CASH ADVANCE"),
        ("01", "0005", "INTEREST AMOUNT"),
        ("02", "0001", "CASH PAYMENT"),
        ("02", "0002", "ELECTRONIC PAYMENT"),
        ("02", "0003", "CHECK PAYMENT"),
        ("03", "0001", "CREDIT TO ACCOUNT"),
        ("03", "0002", "CREDIT TO PURCHASE BALANCE"),
        ("03", "0003", "CREDIT TO CASH BALANCE"),
        ("04", "0001", "ZERO DOLLAR AUTHORIZATION"),
        ("04", "0002", "ONLINE PURCHASE AUTHORIZATION"),
        ("04", "0003", "TRAVEL BOOKING AUTHORIZATION"),
        ("05", "0001", "REFUND CREDIT"),
        ("06", "0001", "FRAUD REVERSAL"),
        ("06", "0002", "NON FRAUD REVERSAL"),
        ("07", "0001", "SALES DRAFT CREDIT ADJUSTMENT"),
    ];

    /// The seven seeded type codes of `DB2LTTYP.ctl:17-23`.
    const SEEDED_TYPES: [&str; 7] = ["01", "02", "03", "04", "05", "06", "07"];

    fn code(text: &str) -> Result<PicX<2>, Box<dyn StdError>> {
        Ok(PicX::from_text(text)?)
    }

    fn category(text: &str) -> Result<PicX<4>, Box<dyn StdError>> {
        Ok(PicX::from_text(text)?)
    }

    fn record(
        type_code: &str,
        type_category: &str,
        text: &str,
    ) -> Result<Db2TransactionCategory, Box<dyn StdError>> {
        Ok(Db2TransactionCategory::new(
            code(type_code)?,
            category(type_category)?,
            text.to_owned(),
        )?)
    }

    /// The seeded rows of one type, in key order.
    fn seeded(type_code: &str) -> Result<Vec<Db2TransactionCategory>, Box<dyn StdError>> {
        SEED.iter()
            .filter(|(seed_code, _, _)| *seed_code == type_code)
            .map(|(seed_code, seed_category, seed_text)| {
                record(seed_code, seed_category, seed_text)
            })
            .collect()
    }

    /// Every seeded type's categories, read through `repo`.
    async fn every_seeded_type(
        repo: &mut impl Db2TransactionTypeCategoryRepository,
    ) -> Result<Vec<Db2TransactionCategory>, Box<dyn StdError>> {
        let mut all = Vec::new();
        for type_code in SEEDED_TYPES {
            all.extend(repo.find_all_by_type(&code(type_code)?).await?);
        }
        Ok(all)
    }

    fn duplicate() -> PersistenceError {
        PersistenceError::Duplicate {
            constraint: Some("x_tran_type_catg".to_owned()),
        }
    }

    fn restrict() -> PersistenceError {
        PersistenceError::Restrict {
            constraint: Some("trc_type_code".to_owned()),
        }
    }

    /// The migration's seed reads back by key and by type, every byte as the CTL member wrote
    /// it, and a key nothing holds reads as `None` or as no rows.
    #[tokio::test]
    async fn seeded_rows_read_back_by_key_and_by_type() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());

        for (type_code, type_category, text) in SEED {
            let found = store
                .find_by_id(&code(type_code)?, &category(type_category)?)
                .await?;
            assert_eq!(found, Some(record(type_code, type_category, text)?));
        }
        for type_code in SEEDED_TYPES {
            assert_eq!(
                store.find_all_by_type(&code(type_code)?).await?,
                seeded(type_code)?,
                "type {type_code}"
            );
        }
        let expected: Vec<_> = SEED
            .iter()
            .map(|(seed_code, seed_category, seed_text)| {
                record(seed_code, seed_category, seed_text)
            })
            .collect::<Result<_, _>>()?;
        assert_eq!(every_seeded_type(&mut store).await?, expected);

        // Keys that nothing holds, a padded type code among them: `1 ` is not `01`.
        for (type_code, type_category) in [("01", "0006"), ("08", "0001"), ("1 ", "0001")] {
            let found = store
                .find_by_id(&code(type_code)?, &category(type_category)?)
                .await?;
            assert_eq!(found, None, "{type_code}/{type_category}");
        }
        for type_code in ["08", "  ", "1 "] {
            assert!(store.find_all_by_type(&code(type_code)?).await?.is_empty());
        }
        Ok(())
    }

    /// An insert keeps its bytes verbatim; a duplicate key is `Duplicate`, an unknown type is
    /// `Restrict` through foreign key `trc_type_code`, and neither changes the table.
    #[tokio::test]
    async fn inserts_keep_bytes_and_report_duplicate_and_restrict() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());

        let added = [
            record("07", "0002", "  Spaced text, mixed Case  ")?,
            record("07", "A   ", "")?,
            record("07", "0003", &"X".repeat(50))?,
        ];
        for row in &added {
            store.insert(row).await?;
            let found = store
                .find_by_id(row.type_code(), row.type_category())
                .await?;
            assert_eq!(found.as_ref(), Some(row));
        }
        // Byte order: the digits sort before `A`.
        let [spaced, letter, longest] = added;
        let mut expected = seeded("07")?;
        expected.extend([spaced.clone(), longest, letter]);
        assert_eq!(store.find_all_by_type(&code("07")?).await?, expected);

        let before = every_seeded_type(&mut store).await?;
        for (row, error) in [
            (record("01", "0001", "ANOTHER TEXT")?, duplicate()),
            (spaced, duplicate()),
            (record("99", "0001", "NO SUCH TYPE")?, restrict()),
            (record("1 ", "0001", "PADDED TYPE")?, restrict()),
        ] {
            assert_eq!(store.insert(&row).await, Err(error));
        }
        assert_eq!(every_seeded_type(&mut store).await?, before);
        assert!(store.find_all_by_type(&code("99")?).await?.is_empty());
        Ok(())
    }

    /// Inside a transaction the insert is visible to that transaction alone until it commits; a
    /// failed insert inside a savepoint rolls back to it and leaves the transaction usable.
    #[tokio::test]
    async fn transactions_see_their_inserts_and_recover_through_savepoints() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let first = record("05", "0002", "REFUND DEBIT")?;
        let second = record("05", "0003", "REFUND REVERSAL")?;

        let mut tx = store.begin().await?;
        tx.insert(&first).await?;
        assert_eq!(
            tx.find_by_id(first.type_code(), first.type_category())
                .await?,
            Some(first.clone())
        );
        assert_eq!(
            store
                .find_by_id(first.type_code(), first.type_category())
                .await?,
            None
        );
        tx.rollback().await?;
        assert_eq!(store.find_all_by_type(&code("05")?).await?, seeded("05")?);

        let mut tx = store.begin().await?;
        tx.insert(&first).await?;
        let mut savepoint: PgTx<'_> = tx.savepoint().await?;
        assert_eq!(savepoint.insert(&first).await, Err(duplicate()));
        savepoint.rollback().await?;
        let mut savepoint = tx.savepoint().await?;
        assert_eq!(
            savepoint
                .insert(&record("98", "0001", "NO SUCH TYPE")?)
                .await,
            Err(restrict())
        );
        savepoint.rollback().await?;
        tx.insert(&second).await?;
        assert_eq!(
            tx.find_all_by_type(&code("05")?).await?.len(),
            seeded("05")?.len() + 2
        );
        tx.commit().await?;

        let mut expected = seeded("05")?;
        expected.extend([first, second]);
        assert_eq!(store.find_all_by_type(&code("05")?).await?, expected);
        Ok(())
    }

    /// A key byte no text column can hold fails before any statement runs, naming its column and
    /// no value. The pool is closed, so any statement would fail as `Unavailable` instead.
    #[tokio::test]
    async fn keys_no_column_can_hold_fail_before_the_query() -> TestResult {
        use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

        let options = PgConnectOptions::new().host("127.0.0.1").port(9);
        let mut store = PgStore::new(PgPoolOptions::new().connect_lazy_with(options));
        store.pool().close().await;
        let invalid = PersistenceError::decode;

        let nul_code = PicX::<2>::from_array(*b"0\0");
        let nul_category = PicX::<4>::from_array([b'0', 0, b'0', b'1']);
        assert_eq!(
            store.find_by_id(&nul_code, &category("0001")?).await,
            Err(invalid(TRC_TYPE_CODE, DecodeKind::InvalidByte))
        );
        assert_eq!(
            store.find_by_id(&code("01")?, &nul_category).await,
            Err(invalid(TRC_TYPE_CATEGORY, DecodeKind::InvalidByte))
        );
        assert_eq!(
            store
                .find_all_by_type(&PicX::from_array([b'0', 0xC9]))
                .await,
            Err(invalid(TRC_TYPE_CODE, DecodeKind::InvalidByte))
        );
        let row = Db2TransactionCategory::new(nul_code, category("0001")?, "TEXT".to_owned())?;
        assert_eq!(
            store.insert(&row).await,
            Err(invalid(TRC_TYPE_CODE, DecodeKind::InvalidByte))
        );
        // With valid keys the statement runs and meets the closed pool.
        assert_eq!(
            store.find_by_id(&code("01")?, &category("0001")?).await,
            Err(PersistenceError::Unavailable)
        );
        Ok(())
    }
}
