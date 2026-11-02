//! The transaction type reference data, in the two versions the COBOL keeps
//! apart (AAP 0.3.2):
//!
//! - [`TranTypeMasterStore`] on the Db2 master `CARDDEMO.TRANSACTION_TYPE`
//!   [app/app-transaction-type-db2/ddl/TRNTYPE.ddl:L1-4] (P-TRNTYPE-001,
//!   P-TRNTYPE-002), table `transaction_types`. CTTU (`COTRTUPC`), CTLI
//!   (`COTRTLIC`) and `batch mnttrdb2` (`COBTUPDT`) are the programs that read
//!   and change it.
//! - [`TranTypeFileReader`] on the VSAM processing copies `TRANTYPE`
//!   (`CVTRA03Y`, P-CVTRA03Y-001, P-CVTRA03Y-002) and `TRANCATG` (`CVTRA04Y`,
//!   P-CVTRA04Y-001 to P-CVTRA04Y-003), tables `trantype_file` and
//!   `trancatg_file`. Only `CBTRN03C` reads them, for CR00 and `TRANREPT`
//!   (P-CBTRN03C-001).
//!
//! No statement of the master store names a processing copy, and no read of
//! the processing copies names a master. The two start different (`PURCHASE`
//! in the master, P-TRNTYPE-004; `Purchase` in the copy, P-CVTRA03Y-005), and
//! an edit of a master reaches the reports only after `batch seed --only
//! tranextr` copies the masters over the copies, as `TRANEXTR` does. The
//! master store never reads `transaction_categories`: no declared method needs
//! it, and the `ON DELETE RESTRICT` of the categories
//! [app/app-transaction-type-db2/ddl/TRNTYCAT.ddl:L6-7] is the database's own
//! check on a delete.
//!
//! # Keys and filters
//!
//! - A [`TranTypeCode`] binds as its right-trimmed text, the only form the
//!   `VARCHAR(2)` key columns admit (their `CHECK` in migration 0001). Db2
//!   compares `CHAR` values blank-padded, so the trimmed equality is the Db2
//!   equality of `TR_TYPE = :host-variable`.
//! - Every text key is compared and ordered `COLLATE "C"`, by byte value: the
//!   key order of the cursors' `ORDER BY TR_TYPE`, never the locale order of
//!   the database's default collation.
//! - An absent filter is a typed `$n::text IS NULL OR …` predicate, the
//!   `:WS-EDIT-TYPE-FLAG <> '1'` and `:WS-EDIT-DESC-FLAG <> '1'` branches of the
//!   `COTRTLIC` cursors and count.
//! - The description filter is the complete pattern the rule built
//!   (P-COTRTLIC-020) and binds unchanged. `ESCAPE ''` turns off the backslash
//!   escape `PostgreSQL` applies by default, since the Db2 predicate names no
//!   escape character; `LIKE` stays case-sensitive, as in Db2.
//!
//! # Writes
//!
//! Each insert, update and delete is one legacy write under its own savepoint
//! (`crate::tx::savepoint` and `crate::tx::settle`), so a failed statement
//! leaves the caller's transaction usable for its error path and the Db2
//! `SYNCPOINT` or `ROLLBACK` decision to the adapter. `crate::port_error`
//! maps the failure onto the outcome the Db2 programs branch on: SQLSTATE
//! `23505` is [`PortError::Duplicate`] (`SQLCODE` -803), `23503`
//! [`PortError::Restricted`] (-532), and `40001`, `40P01` and `55P03`
//! [`PortError::Contention`] (-911). An update or delete that touches no row is
//! [`PortError::NotFound`] (+100). The `SQLCODE` texts belong to the rules,
//! through [`PortError::sqlcode`].
//!
//! A description is stored exactly as the rule gives it. Where the COBOL
//! writes it blank-padded to 50 bytes (P-COTRTUPC-007, P-COTRTLIC-009,
//! P-COBTUPDT-007), the rules give its right-trimmed text, the only form the
//! column's `CHECK` admits. Any other form is refused by the database and
//! reported as [`PortError::Unavailable`]; this file never trims it.

use carddemo_domain::entities::{TranCategoryRecord, TranTypeMaster, TranTypeRecord};
use carddemo_domain::numeric::{TranCategoryCode, TranTypeCode};
use carddemo_domain::ports::{
    Browse, Direction, PortError, PortResult, TranTypeFileReader, TranTypeFilter,
    TranTypeMasterStore,
};

use super::{alnum, expect_affected, limit_i64, tran_category_code, tran_type_code};
use crate::tx::{PgStore, savepoint, settle};
use crate::{PersistenceError, port_error};

/// The Db2 master `CARDDEMO.TRANSACTION_TYPE`.
const MASTER: &str = "transaction_types";

/// The VSAM processing copy `TRANTYPE`.
const TYPE_FILE: &str = "trantype_file";

/// The VSAM processing copy `TRANCATG`.
const CATEGORY_FILE: &str = "trancatg_file";

/// A `transaction_types` row as the master reads select it: `TR_TYPE` and
/// `TR_DESCRIPTION`, the columns `DCLTRANSACTION-TYPE` fetches into.
struct MasterRow {
    tr_type: String,
    tr_description: String,
}

impl MasterRow {
    /// The master the row holds.
    ///
    /// The code is rebuilt from its right-trimmed column (P-TRNTYPE-001) and
    /// the description is kept exactly as stored, through
    /// [`TranTypeMaster::try_new`] (P-TRNTYPE-002). A value that does not fit
    /// is [`PersistenceError::Corrupt`], never cut.
    fn into_master(self) -> Result<TranTypeMaster, PersistenceError> {
        let tr_type = tran_type_code(&self.tr_type, MASTER, "tr_type")?;
        TranTypeMaster::try_new(tr_type, self.tr_description).map_err(|err| {
            PersistenceError::Corrupt {
                table: MASTER,
                column: "tr_description",
                reason: err.to_string(),
            }
        })
    }
}

/// The masters of a cursor's rows, in the order fetched.
fn masters(rows: Vec<MasterRow>) -> PortResult<Vec<TranTypeMaster>> {
    rows.into_iter()
        .map(MasterRow::into_master)
        .collect::<Result<_, _>>()
        .map_err(PortError::from)
}

impl TranTypeMasterStore for PgStore<'_, '_> {
    /// `COTRTUPC` `9100-GET-TRANSACTION-TYPE` — P-COTRTUPC-021
    /// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1469-1511]:
    /// `SELECT TR_TYPE, TR_DESCRIPTION … WHERE TR_TYPE = :DCL-TR-TYPE`. No row
    /// is `SQLCODE` +100, [`PortError::NotFound`]. A plain read, with no
    /// savepoint.
    async fn read_tran_type(&mut self, code: TranTypeCode) -> PortResult<TranTypeMaster> {
        let key = code.trimmed();
        let row = sqlx::query_as!(
            MasterRow,
            "SELECT tr_type, tr_description FROM transaction_types WHERE tr_type = $1",
            key.as_ref(),
        )
        .fetch_optional(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?
        .ok_or(PortError::NotFound)?;
        row.into_master().map_err(PortError::from)
    }

    /// The `COTRTLIC` cursors (P-COTRTLIC-003), up to `browse.limit` rows:
    ///
    /// - Forward: `C-TR-TYPE-FORWARD`, `TR_TYPE >= :WS-START-KEY … ORDER BY
    ///   TR_TYPE` [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L338-352],
    ///   opened by `8000-READ-FORWARD` (P-COTRTLIC-030).
    /// - Backward: `C-TR-TYPE-BACKWARD`, `TR_TYPE < :WS-START-KEY … ORDER BY
    ///   TR_TYPE DESC` [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L354-367],
    ///   opened by `8100-READ-BACKWARDS` (P-COTRTLIC-033). The bound is strict,
    ///   as the port documents for this method: the start key itself is never
    ///   fetched, so a blank start key fetches nothing.
    ///
    /// `None` as the start key drops the bound, reading from the first or the
    /// last type. The type and description filters are the cursors' `AND`
    /// conditions (P-COTRTLIC-020). The rows come back in fetch order; the
    /// backward page is the rule's to show ascending.
    async fn browse_tran_types(
        &mut self,
        browse: Browse<TranTypeCode>,
        filter: TranTypeFilter,
    ) -> PortResult<Vec<TranTypeMaster>> {
        let start = browse.start_key.as_ref().map(TranTypeCode::trimmed);
        let type_code = filter.type_code.as_ref().map(TranTypeCode::trimmed);
        let pattern = filter.description.as_deref();
        let limit = limit_i64(browse.limit)?;
        let rows = match browse.direction {
            Direction::Forward => {
                sqlx::query_as!(
                    MasterRow,
                    r#"SELECT tr_type, tr_description
                         FROM transaction_types
                        WHERE ($1::text IS NULL OR tr_type COLLATE "C" >= $1::text)
                          AND ($2::text IS NULL OR tr_type = $2::text)
                          AND ($3::text IS NULL OR tr_description LIKE $3::text ESCAPE '')
                        ORDER BY tr_type COLLATE "C"
                        LIMIT $4"#,
                    start.as_deref(),
                    type_code.as_deref(),
                    pattern,
                    limit,
                )
                .fetch_all(&mut **self.tx)
                .await
            }
            Direction::Backward => {
                sqlx::query_as!(
                    MasterRow,
                    r#"SELECT tr_type, tr_description
                         FROM transaction_types
                        WHERE ($1::text IS NULL OR tr_type COLLATE "C" < $1::text)
                          AND ($2::text IS NULL OR tr_type = $2::text)
                          AND ($3::text IS NULL OR tr_description LIKE $3::text ESCAPE '')
                        ORDER BY tr_type COLLATE "C" DESC
                        LIMIT $4"#,
                    start.as_deref(),
                    type_code.as_deref(),
                    pattern,
                    limit,
                )
                .fetch_all(&mut **self.tx)
                .await
            }
        };
        masters(rows.map_err(|err| port_error(&err))?)
    }

    /// `COTRTLIC` `9100-CHECK-FILTERS` — P-COTRTLIC-036
    /// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1801-1819]: `SELECT
    /// COUNT(1)` with the cursors' type and description conditions and no
    /// start key.
    async fn count_tran_types(&mut self, filter: TranTypeFilter) -> PortResult<u64> {
        let type_code = filter.type_code.as_ref().map(TranTypeCode::trimmed);
        let count = sqlx::query_scalar!(
            r#"SELECT COUNT(1) AS "count!"
                 FROM transaction_types
                WHERE ($1::text IS NULL OR tr_type = $1::text)
                  AND ($2::text IS NULL OR tr_description LIKE $2::text ESCAPE '')"#,
            type_code.as_deref(),
            filter.description.as_deref(),
        )
        .fetch_one(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?;
        u64::try_from(count)
            .map_err(|_| PortError::Unavailable("COUNT(1) returned a negative count".to_owned()))
    }

    /// `INSERT INTO CARDDEMO.TRANSACTION_TYPE (TR_TYPE, TR_DESCRIPTION)`:
    /// `COTRTUPC` `9700-INSERT-RECORD` — P-COTRTUPC-009
    /// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1596-1602], and
    /// `COBTUPDT` `10031-INSERT-DB` — P-COBTUPDT-007
    /// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L132-164]. The
    /// description is stored exactly as given. An existing code is `SQLCODE`
    /// -803, [`PortError::Duplicate`].
    async fn insert_tran_type(&mut self, rec: TranTypeMaster) -> PortResult<()> {
        let key = rec.tr_type.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "INSERT INTO transaction_types (tr_type, tr_description) VALUES ($1, $2)",
            key.as_ref(),
            rec.tr_description.as_str(),
        )
        .execute(&mut *sp)
        .await;
        settle(sp, result).await?;
        Ok(())
    }

    /// `UPDATE CARDDEMO.TRANSACTION_TYPE SET TR_DESCRIPTION … WHERE TR_TYPE`:
    /// `COTRTLIC` `9200-UPDATE-RECORD` — P-COTRTLIC-009, P-COTRTLIC-010
    /// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1841-1889]; `COTRTUPC`
    /// `9600-WRITE-PROCESSING` — P-COTRTUPC-007, P-COTRTUPC-008
    /// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1531-1578]; `COBTUPDT`
    /// `10032-UPDATE-DB` — P-COBTUPDT-008
    /// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L166-195]. The
    /// description is stored exactly as given. No row with the code is
    /// `SQLCODE` +100, [`PortError::NotFound`], which `COTRTUPC` answers with
    /// its insert.
    async fn update_tran_type(&mut self, rec: TranTypeMaster) -> PortResult<()> {
        let key = rec.tr_type.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "UPDATE transaction_types SET tr_description = $2 WHERE tr_type = $1",
            key.as_ref(),
            rec.tr_description.as_str(),
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }

    /// `DELETE FROM CARDDEMO.TRANSACTION_TYPE WHERE TR_TYPE`: `COTRTLIC`
    /// `9300-DELETE-RECORD` — P-COTRTLIC-007, P-COTRTLIC-008
    /// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1900-1935]; `COTRTUPC`
    /// `9800-DELETE-PROCESSING` — P-COTRTUPC-004, P-COTRTUPC-005
    /// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1624-1662]; `COBTUPDT`
    /// `10033-DELETE-DB` — P-COBTUPDT-009
    /// [app/app-transaction-type-db2/cbl/COBTUPDT.cbl:L196-226]. Categories
    /// that still reference the type are `SQLCODE` -532,
    /// [`PortError::Restricted`]; no row with the code is +100,
    /// [`PortError::NotFound`].
    async fn delete_tran_type(&mut self, code: TranTypeCode) -> PortResult<()> {
        let key = code.trimmed();
        let mut sp = savepoint(self.tx).await?;
        let result = sqlx::query!(
            "DELETE FROM transaction_types WHERE tr_type = $1",
            key.as_ref(),
        )
        .execute(&mut *sp)
        .await;
        let done = settle(sp, result).await?;
        expect_affected(done.rows_affected())
    }
}

impl TranTypeFileReader for PgStore<'_, '_> {
    /// `CBTRN03C` `1500-B-LOOKUP-TRANTYPE` — P-CBTRN03C-005
    /// [app/cbl/CBTRN03C.cbl:L494-502]: the random `READ TRANTYPE-FILE INTO
    /// TRAN-TYPE-RECORD` by `FD-TRAN-TYPE`. `INVALID KEY` is
    /// [`PortError::NotFound`]. The record is rebuilt with its `X(50)`
    /// description padded (P-CVTRA03Y-001, P-CVTRA03Y-002).
    async fn read_tran_type_file(&mut self, code: TranTypeCode) -> PortResult<TranTypeRecord> {
        let key = code.trimmed();
        let row = sqlx::query!(
            "SELECT tran_type, tran_type_desc FROM trantype_file WHERE tran_type = $1",
            key.as_ref(),
        )
        .fetch_optional(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?
        .ok_or(PortError::NotFound)?;
        Ok(TranTypeRecord {
            tran_type: tran_type_code(&row.tran_type, TYPE_FILE, "tran_type")?,
            tran_type_desc: alnum(&row.tran_type_desc, TYPE_FILE, "tran_type_desc")?,
        })
    }

    /// `CBTRN03C` `1500-C-LOOKUP-TRANCATG` — P-CBTRN03C-005
    /// [app/cbl/CBTRN03C.cbl:L504-512]: the random `READ TRANCATG-FILE INTO
    /// TRAN-CAT-RECORD` by `FD-TRAN-CAT-KEY`, the type code followed by the
    /// `9(04)` category. `INVALID KEY` is [`PortError::NotFound`]. The record
    /// is rebuilt with its `X(50)` description padded (P-CVTRA04Y-001 to
    /// P-CVTRA04Y-003).
    async fn read_tran_category_file(
        &mut self,
        code: TranTypeCode,
        category: TranCategoryCode,
    ) -> PortResult<TranCategoryRecord> {
        let key = code.trimmed();
        let row = sqlx::query!(
            "SELECT tran_type_cd, tran_cat_cd, tran_cat_type_desc
               FROM trancatg_file
              WHERE tran_type_cd = $1 AND tran_cat_cd = $2",
            key.as_ref(),
            i32::from(category.get()),
        )
        .fetch_optional(&mut **self.tx)
        .await
        .map_err(|err| port_error(&err))?
        .ok_or(PortError::NotFound)?;
        Ok(TranCategoryRecord {
            tran_type_cd: tran_type_code(&row.tran_type_cd, CATEGORY_FILE, "tran_type_cd")?,
            tran_cat_cd: tran_category_code(row.tran_cat_cd, CATEGORY_FILE, "tran_cat_cd")?,
            tran_cat_type_desc: alnum(
                &row.tran_cat_type_desc,
                CATEGORY_FILE,
                "tran_cat_type_desc",
            )?,
        })
    }
}

#[cfg(test)]
mod tests {
    use carddemo_domain::numeric::TranTypeCode;

    use super::{MASTER, MasterRow};
    use crate::PersistenceError;

    fn row(tr_type: &str, tr_description: &str) -> MasterRow {
        MasterRow {
            tr_type: tr_type.to_owned(),
            tr_description: tr_description.to_owned(),
        }
    }

    #[test]
    fn a_master_row_keeps_its_description_as_stored() {
        // P-TRNTYPE-004: the seeded master 01 holds the Db2 load's upper case.
        let cases = [
            ("01", "PURCHASE", "01"),
            ("7", "ADJUSTMENT", "7 "),
            ("06", "", "06"),
            ("05", "  REFUND", "05"),
        ];
        for (tr_type, tr_description, code) in cases {
            let master = row(tr_type, tr_description)
                .into_master()
                .unwrap_or_else(|err| panic!("{tr_type:?} rejected: {err}"));
            assert_eq!(master.tr_type, TranTypeCode::move_from(code), "{tr_type:?}");
            assert_eq!(master.tr_description, tr_description, "{tr_type:?}");
        }
    }

    #[test]
    fn a_stored_value_that_does_not_fit_is_corrupt() {
        let cases = [
            (row("011", "PURCHASE"), "tr_type"),
            (row("01", &"D".repeat(51)), "tr_description"),
            (row("01", "PAYMENT \u{20ac}"), "tr_description"),
        ];
        for (stored, expected) in cases {
            match stored.into_master() {
                Err(PersistenceError::Corrupt { table, column, .. }) => {
                    assert_eq!((table, column), (MASTER, expected));
                }
                other => panic!("expected a Corrupt {expected} error, got {other:?}"),
            }
        }
    }
}
