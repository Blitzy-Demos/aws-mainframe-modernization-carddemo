//! [`Db2TransactionTypeRepository`]: `db2.transaction_type`, the Db2 data version of the
//! transaction types (migration `0006_db2_transaction_types.sql`, over `TRNTYPE.ddl:1-4`).
//!
//! # Data version (E23)
//!
//! Only the ports of COTRTLIC, COTRTUPC and COBTUPDT reach this table, and it is the only table
//! they name (COTRTUPC-002, COBTUPDT-026). It is not the VSAM data version in
//! `transaction_types` that `tranrept` reads: nothing here touches that table, and nothing copies
//! between the two, so a CTLI, CTTU or MNTTRDB2 change never reaches the report lookups.
//!
//! # Statements
//!
//! Each COBOL statement is one SQL text, written once in a private function generic over the
//! executor, so [`PgStore`] and [`PgTx`] run the same text.
//!
//! | Method | COBOL statement | Outcome |
//! |---|---|---|
//! | `find_by_id` | COTRTUPC `9100-GET-TRANSACTION-TYPE` `SELECT … WHERE TR_TYPE = :DCL-TR-TYPE` (`COTRTUPC.cbl:1475-1482`) | `None` for `SQLCODE` +100 (COTRTUPC-009, COTRTUPC-010) |
//! | `page_after` | COTRTLIC cursor `C-TR-TYPE-FORWARD`, `TR_TYPE >= :WS-START-KEY` (`COTRTLIC.cbl:338-352`), fetched by `8000-READ-FORWARD` (`COTRTLIC.cbl:1603-1724`) | rows in ascending code order (COTRTLIC-004) |
//! | `page_before` | COTRTLIC cursor `C-TR-TYPE-BACKWARD`, `TR_TYPE < :WS-START-KEY`, `ORDER BY TR_TYPE DESC` (`COTRTLIC.cbl:354-368`), fetched by `8100-READ-BACKWARDS` (`COTRTLIC.cbl:1727-1794`) | rows in descending code order, fewer than asked near the first code (COTRTLIC-011, COTRTLIC-012) |
//! | `count_matching` | COTRTLIC `9100-CHECK-FILTERS` `SELECT COUNT(1)` (`COTRTLIC.cbl:1801-1834`) | the count under the cursors' filter predicates, without a start key (COTRTLIC-031) |
//! | `insert` | COTRTUPC `9700-INSERT-RECORD` (`COTRTUPC.cbl:1597-1602`); COBTUPDT `10031-INSERT-DB` (`COBTUPDT.cbl:137-149`) | [`PersistenceError::Duplicate`] with constraint `xtran_type` for a code already present (COTRTUPC-027, COBTUPDT-011) |
//! | `update` | COTRTUPC `9600-WRITE-PROCESSING` (`COTRTUPC.cbl:1544-1548`); COTRTLIC `9200-UPDATE-RECORD` (`COTRTLIC.cbl:1837-1894`); COBTUPDT `10032-UPDATE-DB` (`COBTUPDT.cbl:171-176`) | `false` when no row has the code, `SQLCODE` +100, on which COTRTUPC inserts instead (COTRTUPC-026, COTRTLIC-046, COBTUPDT-013) |
//! | `delete` | COTRTUPC `9800-DELETE-PROCESSING` (`COTRTUPC.cbl:1627-1639`); COTRTLIC `9300-DELETE-RECORD` (`COTRTLIC.cbl:1896-1940`); COBTUPDT `10033-DELETE-DB` (`COBTUPDT.cbl:201-205`) | `false` for `SQLCODE` +100; [`PersistenceError::Restrict`] with constraint `trc_type_code` for `SQLCODE` -532 (COTRTUPC-033, COTRTLIC-050, COBTUPDT-017) |
//!
//! # Filters and paging
//!
//! A `None` filter stands for the COBOL edit flag that is not `'1'` (`WS-EDIT-TYPE-FLAG`,
//! `WS-EDIT-DESC-FLAG`), so its predicate drops out; a `Some` filter is bound verbatim. The
//! domain builds the description pattern exactly as COTRTLIC does, `%`, the trimmed filter text,
//! `%` (`COTRTLIC.cbl:1155-1163`), and the statement applies `TRIM` to it as the cursors do. The
//! match is case-sensitive, and `ESCAPE ''` keeps Db2's `LIKE` with no escape character, where
//! `PostgreSQL` would otherwise treat a backslash as one (COTRTLIC-005). `count_matching` writes
//! the cursors' two filter predicates character for character, so its count equals the rows an
//! unbounded cursor returns under the same filters.
//!
//! The pages read `queries/db2_transaction_types_page_forward.sql` and
//! `queries/db2_transaction_types_page_backward.sql` through `sqlx::query_file_as!`, the same
//! files the `explain` test plans on the `xtran_type` key (Bounded Pagination). `limit` is bound
//! as given: the service passes the page size plus one, the look-ahead fetch by which
//! `8000-READ-FORWARD` learns whether a next page exists (`COTRTLIC.cbl:1657-1676`), after the
//! API has bounded the page size to 1 to 100.
//!
//! # Stored bytes
//!
//! The code and the description are bound and read back verbatim through codec B in
//! [`crate::rows::db2_transaction_types`]. Trimming and padding the description belong to the
//! domain port that writes the row: COTRTUPC's `FUNCTION TRIM` padded to 50 bytes
//! (`COTRTUPC.cbl:1538-1542`, COTRTUPC-023), COTRTLIC's trimmed text (COTRTLIC-044) and
//! COBTUPDT's untrimmed 50 input bytes (COBTUPDT-010, COBTUPDT-012).
//!
//! # Errors and transactions
//!
//! - A Db2 `-911` (deadlock or timeout) arrives as [`PersistenceError::Database`] with SQLSTATE
//!   `40P01`, `55P03` or `40001`, like every other failure the programs answer with
//!   `WHEN OTHER` or `SQLCODE < 0`; no method treats it apart (COTRTLIC-047, COTRTUPC-029).
//! - The Db2 message texts that quote `SQLCODE` and `SQLERRM` are halt H3, and their port lives in
//!   the domain, not here (E19).
//! - On a [`PgTx`], a failed statement, a duplicate insert or a restricted delete included,
//!   aborts the whole transaction, whereas a failed Db2 statement leaves the unit of work open. A
//!   caller that must go on after such a failure, as COBTUPDT does for its next record
//!   (COBTUPDT-024), runs the statement inside a [`PgTx::savepoint`] and rolls back to it. On
//!   [`PgStore`] each statement commits on its own, so COTRTUPC's update followed by its insert
//!   (COTRTUPC-026) is two autocommitted statements unless the caller opens a transaction.
//! - Errors carry an SQLSTATE and a constraint name only, never a code or a description, and this
//!   module emits no tracing (Cardholder Data Redaction).
//!
//! Every key is taken by reference: `PicX<2>` is `Clone` but not `Copy`.

use copybook::PicX;
use domain::records::Db2TransactionType;
use sqlx::PgExecutor;

use crate::error::PersistenceError;
use crate::rows::{Db2TransactionTypeRow, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column of `DCL-TR-TYPE`, the primary key `xtran_type`, named in a key's conversion error.
const TR_TYPE: &str = "tr_type";

/// Reads and writes of `db2.transaction_type`, one method per COBOL statement of the module
/// table, implemented by [`PgStore`] (each statement autocommitted) and [`PgTx`] (the caller's
/// transaction).
pub trait Db2TransactionTypeRepository: Send {
    /// The row whose code is `code`, COTRTUPC's key read (`COTRTUPC.cbl:1475-1482`,
    /// COTRTUPC-009); `None` when no row has it (`SQLCODE` +100, COTRTUPC-010).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for column `tr_type` when `code` holds a NUL or non-ASCII
    /// byte, or when the stored row does not fit its record; [`PersistenceError::Database`] or
    /// [`PersistenceError::Unavailable`] when the read fails.
    fn find_by_id(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<Option<Db2TransactionType>, PersistenceError>> + Send;

    /// At most `limit` rows whose code is at or above `start`, in ascending code order, under the
    /// filters: cursor `C-TR-TYPE-FORWARD` (`COTRTLIC.cbl:338-352`, COTRTLIC-004, COTRTLIC-005).
    ///
    /// `type_filter` is `None` when `WS-EDIT-TYPE-FLAG` is not `'1'`; `desc_pattern` is `None`
    /// when `WS-EDIT-DESC-FLAG` is not `'1'`, otherwise the `LIKE` pattern the domain built.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for column `tr_type` when `start` or `type_filter` holds a NUL
    /// or non-ASCII byte, or when a stored row does not fit its record;
    /// [`PersistenceError::Database`] when the server rejects the read, such as a negative
    /// `limit` or a NUL in `desc_pattern`; [`PersistenceError::Unavailable`] when the database
    /// cannot be reached.
    fn page_after(
        &mut self,
        start: &PicX<2>,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<Db2TransactionType>, PersistenceError>> + Send;

    /// At most `limit` rows whose code is below `start`, in descending code order, under the
    /// filters: cursor `C-TR-TYPE-BACKWARD` (`COTRTLIC.cbl:354-368`, COTRTLIC-011). Fewer rows
    /// than `limit` come back when fewer lie below `start` (COTRTLIC-012). The filters are those
    /// of [`page_after`](Self::page_after).
    ///
    /// # Errors
    ///
    /// As [`page_after`](Self::page_after).
    fn page_before(
        &mut self,
        start: &PicX<2>,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<Db2TransactionType>, PersistenceError>> + Send;

    /// How many rows the filters match, with no start key: `9100-CHECK-FILTERS`
    /// (`COTRTLIC.cbl:1801-1834`, COTRTLIC-031). The filters are those of
    /// [`page_after`](Self::page_after), and the predicates are the cursors' own.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for column `tr_type` when `type_filter` holds a NUL or
    /// non-ASCII byte; [`PersistenceError::Database`] when the server rejects the count, such as
    /// a NUL in `desc_pattern`; [`PersistenceError::Unavailable`] when the database cannot be
    /// reached.
    fn count_matching(
        &mut self,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send;

    /// Inserts `row`, code and description verbatim: COTRTUPC's insert after an update that found
    /// no row (`COTRTUPC.cbl:1597-1602`, COTRTUPC-026) and COBTUPDT's `A` record
    /// (`COBTUPDT.cbl:137-149`, COBTUPDT-010).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] with constraint `xtran_type` when a row already has the
    /// code (COTRTUPC-027, COBTUPDT-011); [`PersistenceError::Decode`] when the code or the
    /// description holds a NUL or non-ASCII byte; [`PersistenceError::Database`] or
    /// [`PersistenceError::Unavailable`] when the insert fails otherwise.
    fn insert(
        &mut self,
        row: &Db2TransactionType,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Sets the description of the row whose code is `row`'s to `row`'s description, verbatim:
    /// COTRTUPC's save (`COTRTUPC.cbl:1544-1548`, COTRTUPC-024), COTRTLIC's F10 update
    /// (`COTRTLIC.cbl:1837-1856`, COTRTLIC-044) and COBTUPDT's `U` record
    /// (`COBTUPDT.cbl:171-176`, COBTUPDT-012). `true` when one row changed, `false` when no row
    /// has the code (`SQLCODE` +100: COTRTUPC-026, COTRTLIC-046, COBTUPDT-013).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when the code or the description holds a NUL or non-ASCII
    /// byte; [`PersistenceError::Database`] when the update fails, a deadlock or lock timeout
    /// (`40P01`, `55P03`) included (COTRTLIC-047, COTRTUPC-029); [`PersistenceError::Unavailable`]
    /// when the database cannot be reached.
    fn update(
        &mut self,
        row: &Db2TransactionType,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Deletes the row whose code is `code`: COTRTUPC's second F4 (`COTRTUPC.cbl:1627-1639`,
    /// COTRTUPC-032), COTRTLIC's F10 delete (`COTRTLIC.cbl:1896-1940`, COTRTLIC-049) and
    /// COBTUPDT's `D` record (`COBTUPDT.cbl:201-205`, COBTUPDT-015). `true` when the row was
    /// deleted, `false` when no row has the code (`SQLCODE` +100: COTRTUPC-034, COTRTLIC-051,
    /// COBTUPDT-016).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Restrict`] with constraint `trc_type_code` when
    /// `db2.transaction_type_category` still holds a category of the code, the `ON DELETE
    /// RESTRICT` of `TRNTYCAT.ddl:6-7` and Db2 `SQLCODE` -532 (COTRTUPC-033, COTRTLIC-050,
    /// COBTUPDT-017); on a [`PgTx`] that failure aborts the transaction, so a caller that goes on
    /// runs the delete inside a [`PgTx::savepoint`]. [`PersistenceError::Decode`] for column
    /// `tr_type` when `code` holds a NUL or non-ASCII byte; [`PersistenceError::Database`] or
    /// [`PersistenceError::Unavailable`] when the delete fails otherwise.
    fn delete(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;
}

impl Db2TransactionTypeRepository for PgStore {
    fn find_by_id(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<Option<Db2TransactionType>, PersistenceError>> + Send {
        select_by_code(self.pool(), code)
    }

    fn page_after(
        &mut self,
        start: &PicX<2>,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<Db2TransactionType>, PersistenceError>> + Send {
        select_forward(self.pool(), start, type_filter, desc_pattern, limit)
    }

    fn page_before(
        &mut self,
        start: &PicX<2>,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<Db2TransactionType>, PersistenceError>> + Send {
        select_backward(self.pool(), start, type_filter, desc_pattern, limit)
    }

    fn count_matching(
        &mut self,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send {
        select_count(self.pool(), type_filter, desc_pattern)
    }

    fn insert(
        &mut self,
        row: &Db2TransactionType,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_row(self.pool(), row)
    }

    fn update(
        &mut self,
        row: &Db2TransactionType,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_description(self.pool(), row)
    }

    fn delete(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        delete_by_code(self.pool(), code)
    }
}

impl Db2TransactionTypeRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<Option<Db2TransactionType>, PersistenceError>> + Send {
        select_by_code(self.conn(), code)
    }

    fn page_after(
        &mut self,
        start: &PicX<2>,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<Db2TransactionType>, PersistenceError>> + Send {
        select_forward(self.conn(), start, type_filter, desc_pattern, limit)
    }

    fn page_before(
        &mut self,
        start: &PicX<2>,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<Db2TransactionType>, PersistenceError>> + Send {
        select_backward(self.conn(), start, type_filter, desc_pattern, limit)
    }

    fn count_matching(
        &mut self,
        type_filter: Option<&PicX<2>>,
        desc_pattern: Option<&str>,
    ) -> impl Future<Output = Result<i64, PersistenceError>> + Send {
        select_count(self.conn(), type_filter, desc_pattern)
    }

    fn insert(
        &mut self,
        row: &Db2TransactionType,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_row(self.conn(), row)
    }

    fn update(
        &mut self,
        row: &Db2TransactionType,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_description(self.conn(), row)
    }

    fn delete(
        &mut self,
        code: &PicX<2>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        delete_by_code(self.conn(), code)
    }
}

/// The bind value of a type code, through codec B (PIC-001, PIC-002).
fn code_text(code: &PicX<2>) -> Result<String, PersistenceError> {
    pic_x_to_string(TR_TYPE, code)
}

/// The bind value of an optional type filter: `None` keeps the predicate off.
fn filter_text(type_filter: Option<&PicX<2>>) -> Result<Option<String>, PersistenceError> {
    type_filter.map(code_text).transpose()
}

/// Rows of a page, in the order the query returned them, each through codec B.
fn into_records(
    rows: Vec<Db2TransactionTypeRow>,
) -> Result<Vec<Db2TransactionType>, PersistenceError> {
    rows.into_iter()
        .map(Db2TransactionTypeRow::into_record)
        .collect()
}

/// COTRTUPC `9100-GET-TRANSACTION-TYPE` (`COTRTUPC.cbl:1475-1482`).
async fn select_by_code<'e, E>(
    executor: E,
    code: &PicX<2>,
) -> Result<Option<Db2TransactionType>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let code = code_text(code)?;
    let row = sqlx::query_as!(
        Db2TransactionTypeRow,
        "SELECT tr_type, tr_description FROM db2.transaction_type WHERE tr_type = $1",
        code
    )
    .fetch_optional(executor)
    .await?;
    row.map(Db2TransactionTypeRow::into_record).transpose()
}

/// Cursor `C-TR-TYPE-FORWARD` (`COTRTLIC.cbl:338-352`).
async fn select_forward<'e, E>(
    executor: E,
    start: &PicX<2>,
    type_filter: Option<&PicX<2>>,
    desc_pattern: Option<&str>,
    limit: i64,
) -> Result<Vec<Db2TransactionType>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let start = code_text(start)?;
    let type_filter = filter_text(type_filter)?;
    let rows = sqlx::query_file_as!(
        Db2TransactionTypeRow,
        "queries/db2_transaction_types_page_forward.sql",
        start,
        type_filter,
        desc_pattern,
        limit
    )
    .fetch_all(executor)
    .await?;
    into_records(rows)
}

/// Cursor `C-TR-TYPE-BACKWARD` (`COTRTLIC.cbl:354-368`).
async fn select_backward<'e, E>(
    executor: E,
    start: &PicX<2>,
    type_filter: Option<&PicX<2>>,
    desc_pattern: Option<&str>,
    limit: i64,
) -> Result<Vec<Db2TransactionType>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let start = code_text(start)?;
    let type_filter = filter_text(type_filter)?;
    let rows = sqlx::query_file_as!(
        Db2TransactionTypeRow,
        "queries/db2_transaction_types_page_backward.sql",
        start,
        type_filter,
        desc_pattern,
        limit
    )
    .fetch_all(executor)
    .await?;
    into_records(rows)
}

/// COTRTLIC `9100-CHECK-FILTERS` (`COTRTLIC.cbl:1801-1834`). The two predicates are those of the
/// page queries, character for character, renumbered from `$2` and `$3` to `$1` and `$2`.
async fn select_count<'e, E>(
    executor: E,
    type_filter: Option<&PicX<2>>,
    desc_pattern: Option<&str>,
) -> Result<i64, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let type_filter = filter_text(type_filter)?;
    let matching = sqlx::query_scalar!(
        r#"SELECT count(1) AS "matching!"
FROM db2.transaction_type
WHERE ($1::bpchar IS NULL OR tr_type = $1::bpchar)
    AND ($2::varchar IS NULL OR tr_description LIKE TRIM($2::varchar) ESCAPE '')"#,
        type_filter,
        desc_pattern
    )
    .fetch_one(executor)
    .await?;
    Ok(matching)
}

/// COTRTUPC `9700-INSERT-RECORD` (`COTRTUPC.cbl:1597-1602`) and COBTUPDT `10031-INSERT-DB`
/// (`COBTUPDT.cbl:137-149`).
async fn insert_row<'e, E>(executor: E, record: &Db2TransactionType) -> Result<(), PersistenceError>
where
    E: PgExecutor<'e>,
{
    let row = Db2TransactionTypeRow::from_record(record)?;
    sqlx::query!(
        "INSERT INTO db2.transaction_type (tr_type, tr_description) VALUES ($1, $2)",
        row.tr_type,
        row.tr_description
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// COTRTUPC `9600-WRITE-PROCESSING` (`COTRTUPC.cbl:1544-1548`), COTRTLIC `9200-UPDATE-RECORD`
/// (`COTRTLIC.cbl:1846-1850`) and COBTUPDT `10032-UPDATE-DB` (`COBTUPDT.cbl:171-176`).
async fn update_description<'e, E>(
    executor: E,
    record: &Db2TransactionType,
) -> Result<bool, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let row = Db2TransactionTypeRow::from_record(record)?;
    let done = sqlx::query!(
        "UPDATE db2.transaction_type SET tr_description = $1 WHERE tr_type = $2",
        row.tr_description,
        row.tr_type
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// COTRTUPC `9800-DELETE-PROCESSING` (`COTRTUPC.cbl:1627-1630`), COTRTLIC `9300-DELETE-RECORD`
/// (`COTRTLIC.cbl:1900-1903`) and COBTUPDT `10033-DELETE-DB` (`COBTUPDT.cbl:201-205`).
async fn delete_by_code<'e, E>(executor: E, code: &PicX<2>) -> Result<bool, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let code = code_text(code)?;
    let done = sqlx::query!("DELETE FROM db2.transaction_type WHERE tr_type = $1", code)
        .execute(executor)
        .await?;
    Ok(done.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::PicX;
    use domain::records::Db2TransactionType;

    use super::Db2TransactionTypeRepository;
    use crate::{PersistenceError, PgStore};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// More rows than the table holds, for reads that must return every match.
    const UNBOUNDED: i64 = 100;

    /// The seed of migration 0006 (`DB2LTTYP.ctl:15-23`), in code order.
    const SEED: [(&str, &str); 7] = [
        ("01", "PURCHASE"),
        ("02", "PAYMENT"),
        ("03", "CREDIT"),
        ("04", "AUTHORIZATION"),
        ("05", "REFUND"),
        ("06", "REVERAL"),
        ("07", "ADJUSTMENT"),
    ];

    fn code(text: &str) -> Result<PicX<2>, Box<dyn StdError>> {
        Ok(PicX::<2>::from_text(text)?)
    }

    fn record(text: &str, description: &str) -> Result<Db2TransactionType, Box<dyn StdError>> {
        Ok(Db2TransactionType::new(
            code(text)?,
            description.to_owned(),
        )?)
    }

    /// The error of a delete that a category of the type refuses (`SQLCODE` -532).
    fn restricted() -> PersistenceError {
        PersistenceError::Restrict {
            constraint: Some("trc_type_code".to_owned()),
        }
    }

    /// The error of an insert whose code a row already has (`SQLCODE` -803).
    fn duplicate() -> PersistenceError {
        PersistenceError::Duplicate {
            constraint: Some("xtran_type".to_owned()),
        }
    }

    /// The codes of `rows`, in their order.
    fn codes(rows: &[Db2TransactionType]) -> Vec<String> {
        rows.iter().map(|row| row.tr_type().to_text()).collect()
    }

    /// A store over a freshly migrated container, which holds the seed and nothing else.
    async fn seeded() -> Result<(test_support::postgres::PostgresGuard, PgStore), Box<dyn StdError>>
    {
        let guard = test_support::postgres::start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        Ok((guard, store))
    }

    // audit: COTRTUPC-009
    // audit: COTRTUPC-010
    // audit: COTRTLIC-004
    // audit: COTRTLIC-005
    // audit: COTRTLIC-011
    // audit: COTRTLIC-012
    // audit: COTRTLIC-031
    #[tokio::test]
    async fn reads_follow_the_key_read_the_cursors_and_the_count() -> TestResult {
        let (_guard, mut store) = seeded().await?;

        for (text, description) in SEED {
            assert_eq!(
                store.find_by_id(&code(text)?).await?,
                Some(record(text, description)?)
            );
        }
        assert_eq!(store.find_by_id(&code("99")?).await?, None);
        assert_eq!(store.find_by_id(&PicX::spaces()).await?, None);

        let all = store
            .page_after(&PicX::spaces(), None, None, UNBOUNDED)
            .await?;
        let seed = SEED
            .iter()
            .map(|(text, description)| record(text, description))
            .collect::<Result<Vec<_>, _>>()?;
        assert_eq!(all, seed);

        // (forward, start, limit, codes): the start key is inclusive forward and exclusive
        // backward, and `limit` is bound as given.
        let paging: [(bool, &str, i64, &[&str]); 6] = [
            (true, "03", 3, &["03", "04", "05"]),
            (true, "03", 1, &["03"]),
            (false, "03", 8, &["02", "01"]),
            (false, "06", 3, &["05", "04", "03"]),
            (false, "01", 8, &[]),
            (true, "08", 8, &[]),
        ];
        for (forward, start, limit, expected) in paging {
            let start = code(start)?;
            let page = if forward {
                store.page_after(&start, None, None, limit).await?
            } else {
                store.page_before(&start, None, None, limit).await?
            };
            assert_eq!(codes(&page), expected, "forward {forward}, limit {limit}");
        }

        let type_06 = code("06")?;
        let type_03 = code("03")?;
        // (type filter, description pattern, codes the filters match). Db2's LIKE is
        // case-sensitive, `TRIM` drops the spaces around the pattern, and with `ESCAPE ''` a
        // backslash is an ordinary character, so `PURCHAS\E` matches nothing.
        let filters: [(Option<&PicX<2>>, Option<&str>, &[&str]); 9] = [
            (None, None, &["01", "02", "03", "04", "05", "06", "07"]),
            (Some(&type_06), None, &["06"]),
            (None, Some("%RE%"), &["03", "05", "06"]),
            (None, Some("  %RE%  "), &["03", "05", "06"]),
            (None, Some("%re%"), &[]),
            (None, Some("PAY_ENT"), &["02"]),
            (None, Some("PURCHAS\\E"), &[]),
            (Some(&type_03), Some("%RE%"), &["03"]),
            (Some(&type_06), Some("%PAY%"), &[]),
        ];
        for (type_filter, desc_pattern, expected) in filters {
            let forward = store
                .page_after(&PicX::spaces(), type_filter, desc_pattern, UNBOUNDED)
                .await?;
            assert_eq!(codes(&forward), expected);

            let mut backward = store
                .page_before(&code("99")?, type_filter, desc_pattern, UNBOUNDED)
                .await?;
            backward.reverse();
            assert_eq!(backward, forward);

            let count = store.count_matching(type_filter, desc_pattern).await?;
            assert_eq!(usize::try_from(count)?, forward.len());
        }
        Ok(())
    }

    // audit: COTRTUPC-023
    // audit: COTRTUPC-024
    // audit: COTRTUPC-026
    // audit: COTRTUPC-027
    // audit: COTRTUPC-032
    // audit: COTRTUPC-033
    // audit: COTRTUPC-034
    // audit: COTRTLIC-044
    // audit: COTRTLIC-046
    // audit: COTRTLIC-049
    // audit: COTRTLIC-050
    // audit: COTRTLIC-051
    #[tokio::test]
    async fn writes_report_changed_rows_duplicates_and_restricted_deletes() -> TestResult {
        let (_guard, mut store) = seeded().await?;

        // An update stores the description verbatim, padding included.
        let renamed = record("01", &format!("{:<50}", "PURCHASE RENAMED"))?;
        assert!(store.update(&renamed).await?);
        assert_eq!(store.find_by_id(&code("01")?).await?, Some(renamed));

        // SQLCODE +100: no row has the code, so nothing changes and COTRTUPC inserts instead.
        let added = record("42", &format!("{:<50}", "NEW TYPE"))?;
        assert!(!store.update(&added).await?);
        assert_eq!(store.find_by_id(&code("42")?).await?, None);
        store.insert(&added).await?;
        assert_eq!(store.find_by_id(&code("42")?).await?, Some(added.clone()));

        let other = record("42", "OTHER")?;
        assert_eq!(store.insert(&other).await, Err(duplicate()));
        assert_eq!(store.find_by_id(&code("42")?).await?, Some(added));

        // A type without categories is deleted once; a second delete finds no row.
        assert!(store.delete(&code("42")?).await?);
        assert_eq!(store.find_by_id(&code("42")?).await?, None);
        assert!(!store.delete(&code("42")?).await?);

        // Every seeded type has categories, so ON DELETE RESTRICT refuses the delete (-532).
        for (text, description) in SEED.iter().skip(1) {
            assert_eq!(store.delete(&code(text)?).await, Err(restricted()));
            assert_eq!(
                store.find_by_id(&code(text)?).await?,
                Some(record(text, description)?)
            );
        }
        assert_eq!(store.count_matching(None, None).await?, 7);
        Ok(())
    }

    /// The replacement of COBTUPDT's single unit of work: a failed statement aborts a
    /// `PostgreSQL` transaction, so the caller runs each statement in a savepoint and rolls back
    /// to it on failure, keeping the earlier statements' changes.
    // audit: COBTUPDT-010
    // audit: COBTUPDT-011
    // audit: COBTUPDT-012
    // audit: COBTUPDT-013
    // audit: COBTUPDT-015
    // audit: COBTUPDT-016
    // audit: COBTUPDT-017
    // audit: COBTUPDT-024
    #[tokio::test]
    async fn a_transaction_goes_on_after_a_failed_statement_only_through_a_savepoint() -> TestResult
    {
        let (_guard, mut store) = seeded().await?;
        let updated = record("02", &format!("{:<50}", "PAYMENT UPDATED BY MNTTRDB2"))?;
        let added = record("43", &format!("{:<50}", "ADDED BY MNTTRDB2"))?;

        let mut tx = store.begin().await?;
        tx.insert(&added).await?;
        let mut statement = tx.savepoint().await?;
        assert_eq!(statement.delete(&code("03")?).await, Err(restricted()));
        statement.rollback().await?;
        let mut statement = tx.savepoint().await?;
        assert_eq!(statement.insert(&added).await, Err(duplicate()));
        statement.rollback().await?;
        assert!(tx.update(&updated).await?);
        assert!(!tx.update(&record("44", "MISSING")?).await?);
        assert!(!tx.delete(&code("44")?).await?);
        tx.commit().await?;

        assert_eq!(store.find_by_id(&code("02")?).await?, Some(updated));
        assert_eq!(store.find_by_id(&code("43")?).await?, Some(added));
        assert_eq!(
            store.find_by_id(&code("03")?).await?,
            Some(record("03", "CREDIT")?)
        );

        // Without a savepoint the restricted delete leaves the transaction aborted: the next
        // statement fails with `in_failed_sql_transaction`, and nothing commits.
        let mut tx = store.begin().await?;
        assert!(tx.delete(&code("43")?).await?);
        assert_eq!(tx.delete(&code("04")?).await, Err(restricted()));
        assert_eq!(
            tx.find_by_id(&code("05")?).await.map(drop),
            Err(PersistenceError::Database {
                sqlstate: Some("25P02".to_owned()),
                constraint: None
            })
        );
        tx.rollback().await?;
        assert!(store.find_by_id(&code("43")?).await?.is_some());
        Ok(())
    }
}
