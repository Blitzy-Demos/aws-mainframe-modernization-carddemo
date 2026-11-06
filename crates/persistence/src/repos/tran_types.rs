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
//! - Every text key is compared and ordered `COLLATE "C"`, by byte value,
//!   never by the locale order of the database's default collation.
//! - The cursors' start bound and `ORDER BY TR_TYPE` compare the key padded
//!   back to its two bytes, `rpad(tr_type, 2) COLLATE "C"` against
//!   `rpad(start, 2)`: the byte order of the Db2 `CHAR(2)` column and of the
//!   `Ord` of [`TranTypeCode`]. The right-trimmed column alone would sort `A`
//!   before `A` followed by a byte below the space, where the padded key
//!   compares the padding space of `A ` with that byte and sorts it after.
//!   The expression index `transaction_types_padded_tr_type` (migration 0001)
//!   holds exactly that padded key, so a page reads it in order, from the
//!   start bound or from either end, with no sort.
//! - The type filter (the `:WS-EDIT-TYPE-FLAG <> '1'` branch of the
//!   `COTRTLIC` cursors and count) and the start key select a static statement
//!   of their own: a given type code is a direct `tr_type` equality on the
//!   primary key `xtran_type`, and otherwise a given start key is a direct
//!   range bound on `transaction_types_padded_tr_type`. A `$n IS NULL OR …`
//!   predicate is folded away only in a custom plan, so under a generic
//!   prepared plan it would keep the key out of the index condition and a
//!   page would read every type before its start key. Within the one row a
//!   type code selects, the start bound stays such a test.
//! - The `:WS-EDIT-DESC-FLAG <> '1'` branch stays a typed
//!   `$n::text IS NULL OR …` predicate in every statement: no index serves the
//!   `LIKE` on `tr_description`, so it only tests the rows the key selects.
//! - The description filter is the complete pattern the rule built
//!   (P-COTRTLIC-020) and binds unchanged. `ESCAPE ''` turns off the backslash
//!   escape `PostgreSQL` applies by default, since the Db2 predicate names no
//!   escape character; `LIKE` stays case-sensitive, as in Db2.
//! - A key holding a NUL byte names no stored type, because `PostgreSQL` text
//!   cannot hold NUL. Binding one would fail on the server with an encoding
//!   error (SQLSTATE `22021`) rather than find no row, so the keyed reads, the
//!   update and the delete answer [`PortError::NotFound`] for it without a
//!   query, as the Db2 statements answer `SQLCODE` +100 and the `CBTRN03C`
//!   `READ`s `INVALID KEY`. The insert has no such answer: the column cannot
//!   store the code, and the statement's failure is reported as
//!   [`PortError::Unavailable`], as for any other value the column refuses.
//! - A type filter or description pattern holding a NUL byte matches no row,
//!   as Db2 finds none for a host variable holding `X'00'`: no stored code
//!   holds NUL, and under `ESCAPE ''` a NUL in the pattern is a literal
//!   character only text holding NUL could match. The count is 0 and the page
//!   empty, without a query.
//! - A start key holding a NUL byte bounds the cursors through a NUL-free key
//!   that selects exactly the same codes (`start_bound`): the characters
//!   before its first NUL, followed by U+0001 up to the key's two characters,
//!   so `"\0 "` binds as `"\x01\x01"` and `"0\0"` as `"0\x01"`. Every
//!   character of a padded stored code is at least U+0001: the column `CHECK`
//!   admits U+0001 to U+00FF, and the padding is a space. For a stored code
//!   `s` padded to two characters, if `s` differs from the key before the
//!   key's first NUL, it differs from the bound there by the same characters
//!   and compares with both alike. Otherwise its next character is above NUL,
//!   so `s > key`, and that character and the rest are each at least the
//!   bound's U+0001, so `s >= bound`. Hence `s >= key` exactly when
//!   `s >= bound`, and `s < key` exactly when `s < bound`, in the
//!   `rpad(…, 2) COLLATE "C"` order: the forward cursor stays inclusive and
//!   the backward cursor strict, through the same statements.
//!
//! # Writes
//!
//! Each insert, update and delete is one legacy write under its own savepoint
//! (`crate::tx::savepoint` and `crate::tx::settle`), and each read runs under
//! its own savepoint too, so a failed statement leaves the caller's
//! transaction usable for its error path and the Db2 `SYNCPOINT` or
//! `ROLLBACK` decision to the adapter. `crate::port_error_at`
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

use std::borrow::Cow;
use std::future::poll_fn;
use std::iter;
use std::task::{Context, Poll};

use carddemo_domain::entities::{TranCategoryRecord, TranTypeMaster, TranTypeRecord};
use carddemo_domain::numeric::{TranCategoryCode, TranTypeCode};
use carddemo_domain::ports::{
    Browse, CursorError, CursorPhase, CursorResult, Direction, PortError, PortResult,
    TranTypeFileReader, TranTypeFilter, TranTypeMasterStore,
};

use super::{alnum, expect_affected, limit_i64, tran_category_code, tran_type_code};
use crate::PersistenceError;
use crate::tx::{PgStore, savepoint, settle};

/// The Db2 master `CARDDEMO.TRANSACTION_TYPE`.
const MASTER: &str = "transaction_types";

/// The VSAM processing copy `TRANTYPE`.
const TYPE_FILE: &str = "trantype_file";

/// The VSAM processing copy `TRANCATG`.
const CATEGORY_FILE: &str = "trancatg_file";

/// The COBOL figurative constant `LOW-VALUE`, which `PostgreSQL` text cannot
/// hold.
const LOW_VALUE: u8 = 0x00;

/// [`LOW_VALUE`] as a character of a code's text or of a description pattern.
const NUL: char = '\0';

/// The lowest character a stored code holds: the column `CHECK` admits
/// U+0001 to U+00FF.
const LOWEST_STORED: char = '\u{1}';

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

/// The one column of the `SELECT COUNT(1)` statements of
/// `count_tran_types` and `count_tran_types_before`.
struct CountRow {
    count: i64,
}

/// The masters of a cursor's rows, in the order fetched.
///
/// # Errors
///
/// A [`CursorPhase::Fetch`] failure at the first row that no
/// [`TranTypeMaster`] can carry, with the masters of the rows before it: the
/// `FETCH` into `DCLTRANSACTION-TYPE` of that row fails.
fn masters(rows: Vec<MasterRow>) -> CursorResult<TranTypeMaster> {
    let mut fetched = Vec::with_capacity(rows.len());
    for row in rows {
        match row.into_master() {
            Ok(master) => fetched.push(master),
            Err(err) => {
                return Err(CursorError {
                    phase: CursorPhase::Fetch,
                    fetched,
                    error: PortError::from(err),
                });
            }
        }
    }
    Ok(fetched)
}

/// The rows a cursor statement returned, in fetch order, and the error that
/// ended it, when it failed.
type Fetched = (Vec<MasterRow>, Option<sqlx::Error>);

/// Receives the rows of one cursor statement through `next_row`, the poll of
/// its row stream, one row at a time until the stream ends or fails, as the
/// program's `FETCH` loop receives them: a failure keeps the rows before it.
async fn fetch_each(
    mut next_row: impl FnMut(&mut Context<'_>) -> Poll<Option<Result<MasterRow, sqlx::Error>>>,
) -> Fetched {
    let mut rows = Vec::new();
    loop {
        match poll_fn(&mut next_row).await {
            Some(Ok(row)) => rows.push(row),
            Some(Err(err)) => return (rows, Some(err)),
            None => return (rows, None),
        }
    }
}

/// Whether `code` holds a NUL byte, and so names no stored type.
fn holds_nul(code: TranTypeCode) -> bool {
    code.as_bytes().contains(&LOW_VALUE)
}

/// The text a code parameter binds, the code right-trimmed, or `None` when
/// the code holds a NUL byte and so names no stored type.
fn key_text(code: &TranTypeCode) -> Option<Cow<'_, str>> {
    (!holds_nul(*code)).then(|| code.trimmed())
}

/// The text a cursor's start bound binds: the start key right-trimmed, or,
/// when it holds a NUL byte, the NUL-free key that selects exactly the same
/// stored codes, as the module documentation proves. That key is the
/// characters of the trimmed key before its first NUL, one per byte, followed
/// by [`LOWEST_STORED`] up to the key's width.
fn start_bound(start: &TranTypeCode) -> Cow<'_, str> {
    key_text(start).unwrap_or_else(|| {
        let width = start.as_bytes().len();
        Cow::Owned(
            start
                .trimmed()
                .chars()
                .take_while(|&character| character != NUL)
                .chain(iter::repeat(LOWEST_STORED))
                .take(width)
                .collect(),
        )
    })
}

/// Whether `filter` holds a NUL byte in its type code or its description
/// pattern, and so matches no stored row (the module documentation).
fn matches_nothing(filter: &TranTypeFilter) -> bool {
    filter.type_code.is_some_and(holds_nul)
        || filter
            .description
            .as_deref()
            .is_some_and(|pattern| pattern.contains(NUL))
}

impl TranTypeMasterStore for PgStore<'_, '_> {
    /// `COTRTUPC` `9100-GET-TRANSACTION-TYPE` — P-COTRTUPC-021
    /// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1469-1511]:
    /// `SELECT TR_TYPE, TR_DESCRIPTION … WHERE TR_TYPE = :DCL-TR-TYPE`. No row
    /// is `SQLCODE` +100, [`PortError::NotFound`], and so is a code holding a
    /// NUL byte, without a query. The read runs under its own savepoint, as
    /// every statement of this store does.
    async fn read_tran_type(&mut self, code: TranTypeCode) -> PortResult<TranTypeMaster> {
        let Some(key) = key_text(&code) else {
            return Err(PortError::NotFound);
        };
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query_as!(
            MasterRow,
            "SELECT tr_type, tr_description FROM transaction_types WHERE tr_type = $1",
            key.as_ref(),
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
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
    ///   fetched, so a blank start key, two spaces, fetches only the codes
    ///   that sort below it, those that begin with a byte below the space or
    ///   with a space and then such a byte.
    ///
    /// Keys compare and sort as the two padded bytes of `TR_TYPE CHAR(2)`, the
    /// order of [`TranTypeCode`], so `A\x01` comes before `A ` (`A` stored);
    /// the module documentation explains how. `None` as the start key drops
    /// the bound, reading from the first or the last type. The type and
    /// description filters are the cursors' `AND` conditions (P-COTRTLIC-020).
    /// The rows come back in fetch order; the backward page is the rule's to
    /// show ascending.
    ///
    /// Each direction has three static statements, chosen by which keys are
    /// present (`types_forward`, `types_backward`): a type filter reads the one
    /// row `tr_type = code` through `xtran_type`, with the start bound left as
    /// a `$n IS NULL OR …` test of that row; otherwise a start key bounds the
    /// range of the padded-key index `transaction_types_padded_tr_type`
    /// directly; otherwise the scan reads that index from the first or last
    /// key. The description filter is a `$n IS NULL OR …` test in all three,
    /// as the module documentation explains.
    ///
    /// A type filter or description pattern holding a NUL byte matches no
    /// row: the page is empty, without a query. A start key holding a NUL byte
    /// binds as the NUL-free key that bounds the cursor at exactly the same
    /// codes (`start_bound`).
    ///
    /// The rows are received one at a time, as the program's `FETCH` loop
    /// receives them, so a failure names the cursor step it stopped
    /// ([`CursorError::after`]) and keeps the rows fetched before it: before
    /// any row, the `OPEN` (P-COTRTLIC-037, P-COTRTLIC-039), a savepoint that
    /// cannot be opened included; after some rows, the `FETCH` of the
    /// next one (P-COTRTLIC-031, P-COTRTLIC-032, P-COTRTLIC-034); after every
    /// row asked for, as the statement completes, the `CLOSE`
    /// (P-COTRTLIC-038, P-COTRTLIC-040). A row the master holds that no
    /// [`TranTypeMaster`] can carry fails the `FETCH` of that row.
    async fn browse_tran_types(
        &mut self,
        browse: Browse<TranTypeCode>,
        filter: TranTypeFilter,
    ) -> CursorResult<TranTypeMaster> {
        if matches_nothing(&filter) {
            return Ok(Vec::new());
        }
        let start = browse.start_key.as_ref().map(start_bound);
        let type_code = filter.type_code.as_ref().map(TranTypeCode::trimmed);
        let pattern = filter.description.as_deref();
        let limit = limit_i64(browse.limit)?;
        let (type_code, start) = (type_code.as_deref(), start.as_deref());
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let (rows, failure) = match browse.direction {
            Direction::Forward => types_forward(&mut sp, type_code, start, pattern, limit).await,
            Direction::Backward => types_backward(&mut sp, type_code, start, pattern, limit).await,
        };
        let settled = settle(sp, failure.map_or(Ok(()), Err)).await;
        let fetched = masters(rows)?;
        match settled {
            Ok(()) => Ok(fetched),
            Err(error) => Err(CursorError::after(fetched, browse.limit, error)),
        }
    }

    /// `COTRTLIC` `9100-CHECK-FILTERS` — P-COTRTLIC-036
    /// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1801-1819]: `SELECT
    /// COUNT(1)` with the cursors' type and description conditions and no
    /// start key.
    ///
    /// A type filter has its own static statement, so `tr_type = code` reads
    /// `xtran_type` under a generic prepared plan too; without one every type
    /// is counted. The description filter is a `$n IS NULL OR …` test in both.
    /// A type filter or description pattern holding a NUL byte matches no row:
    /// the count is 0, without a query.
    async fn count_tran_types(&mut self, filter: TranTypeFilter) -> PortResult<u64> {
        if matches_nothing(&filter) {
            return Ok(0);
        }
        let type_code = filter.type_code.as_ref().map(TranTypeCode::trimmed);
        let pattern = filter.description.as_deref();
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = match type_code.as_deref() {
            Some(code) => {
                sqlx::query_as!(
                    CountRow,
                    r#"SELECT COUNT(1) AS "count!"
                         FROM transaction_types
                        WHERE tr_type = $1::text
                          AND ($2::text IS NULL OR tr_description LIKE $2::text ESCAPE '')"#,
                    code,
                    pattern,
                )
                .fetch_one(&mut *sp)
                .await
            }
            None => {
                sqlx::query_as!(
                    CountRow,
                    r#"SELECT COUNT(1) AS "count!"
                         FROM transaction_types
                        WHERE ($1::text IS NULL OR tr_description LIKE $1::text ESCAPE '')"#,
                    pattern,
                )
                .fetch_one(&mut *sp)
                .await
            }
        };
        let count = settle(sp, result).await?.count;
        u64::try_from(count)
            .map_err(|_| PortError::Unavailable("COUNT(1) returned a negative count".to_owned()))
    }

    /// `COTRTLIC` `8100-READ-BACKWARDS` — P-COTRTLIC-033, P-COTRTLIC-034,
    /// P-COTRTLIC-039, V-COTRTLIC-009
    /// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L354-367]: `SELECT
    /// COUNT(1)` with the `C-TR-TYPE-BACKWARD` conditions, `TR_TYPE <
    /// :WS-START-KEY` and the type and description filters, `key` the start.
    /// It stands in for the page number the COMMAREA keeps (E-54): the rules
    /// count the types before a page-back to tell whether it reaches the
    /// first page.
    ///
    /// The bound is the backward cursor's: strict, on the code padded to its
    /// two bytes `COLLATE "C"`, with a key holding a NUL byte bound through
    /// its NUL-free twin (`start_bound`), so the count is the number of rows
    /// the backward cursor fetches from `key` with no limit. A type filter
    /// has its own static statement, as in `count_tran_types`; the
    /// description filter is a `$n IS NULL OR …` test, so without it no
    /// description is read. A type filter or description pattern holding a
    /// NUL byte matches no row: the count is 0, without a query.
    async fn count_tran_types_before(
        &mut self,
        filter: TranTypeFilter,
        key: TranTypeCode,
    ) -> PortResult<u64> {
        if matches_nothing(&filter) {
            return Ok(0);
        }
        let start = start_bound(&key);
        let type_code = filter.type_code.as_ref().map(TranTypeCode::trimmed);
        let pattern = filter.description.as_deref();
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = match type_code.as_deref() {
            Some(code) => {
                sqlx::query_as!(
                    CountRow,
                    r#"SELECT COUNT(1) AS "count!"
                         FROM transaction_types
                        WHERE tr_type = $1::text
                          AND rpad(tr_type, 2) COLLATE "C" < rpad($2::text, 2)
                          AND ($3::text IS NULL OR tr_description LIKE $3::text ESCAPE '')"#,
                    code,
                    start.as_ref(),
                    pattern,
                )
                .fetch_one(&mut *sp)
                .await
            }
            None => {
                sqlx::query_as!(
                    CountRow,
                    r#"SELECT COUNT(1) AS "count!"
                         FROM transaction_types
                        WHERE rpad(tr_type, 2) COLLATE "C" < rpad($1::text, 2)
                          AND ($2::text IS NULL OR tr_description LIKE $2::text ESCAPE '')"#,
                    start.as_ref(),
                    pattern,
                )
                .fetch_one(&mut *sp)
                .await
            }
        };
        let count = settle(sp, result).await?.count;
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
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
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
    /// its insert; a code holding a NUL byte is too, without a query.
    async fn update_tran_type(&mut self, rec: TranTypeMaster) -> PortResult<()> {
        let Some(key) = key_text(&rec.tr_type) else {
            return Err(PortError::NotFound);
        };
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
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
    /// [`PortError::NotFound`], and so is a code holding a NUL byte, without a
    /// query.
    async fn delete_tran_type(&mut self, code: TranTypeCode) -> PortResult<()> {
        let Some(key) = key_text(&code) else {
            return Err(PortError::NotFound);
        };
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
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

/// The rows of a [`Direction::Forward`]
/// [`TranTypeMasterStore::browse_tran_types`] page: `COTRTLIC`
/// `C-TR-TYPE-FORWARD` (P-COTRTLIC-003)
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L338-352], opened by
/// `8000-READ-FORWARD` (P-COTRTLIC-030), with the filters of P-COTRTLIC-020.
/// Up to `limit` types in ascending byte order of the code padded to its
/// `X(2)` bytes, from `start` inclusive, or from the first type, read on
/// `conn` by the static statement of the keys present and received one row at
/// a time ([`fetch_each`]). `type_code` and `start` are right-trimmed codes,
/// `start` made NUL-free by `start_bound`; each bound pads `start` back with
/// `rpad`.
async fn types_forward(
    conn: &mut sqlx::PgConnection,
    type_code: Option<&str>,
    start: Option<&str>,
    pattern: Option<&str>,
    limit: i64,
) -> Fetched {
    let mut rows = match (type_code, start) {
        (Some(code), start) => sqlx::query_as!(
            MasterRow,
            r#"SELECT tr_type, tr_description
                     FROM transaction_types
                    WHERE tr_type = $1::text
                      AND ($2::text IS NULL OR rpad(tr_type, 2) COLLATE "C" >= rpad($2::text, 2))
                      AND ($3::text IS NULL OR tr_description LIKE $3::text ESCAPE '')
                    ORDER BY rpad(tr_type, 2) COLLATE "C"
                    LIMIT $4"#,
            code,
            start,
            pattern,
            limit,
        )
        .fetch(&mut *conn),
        (None, Some(start)) => sqlx::query_as!(
            MasterRow,
            r#"SELECT tr_type, tr_description
                     FROM transaction_types
                    WHERE rpad(tr_type, 2) COLLATE "C" >= rpad($1::text, 2)
                      AND ($2::text IS NULL OR tr_description LIKE $2::text ESCAPE '')
                    ORDER BY rpad(tr_type, 2) COLLATE "C"
                    LIMIT $3"#,
            start,
            pattern,
            limit,
        )
        .fetch(&mut *conn),
        (None, None) => sqlx::query_as!(
            MasterRow,
            r#"SELECT tr_type, tr_description
                     FROM transaction_types
                    WHERE ($1::text IS NULL OR tr_description LIKE $1::text ESCAPE '')
                    ORDER BY rpad(tr_type, 2) COLLATE "C"
                    LIMIT $2"#,
            pattern,
            limit,
        )
        .fetch(&mut *conn),
    };
    fetch_each(|cx| rows.as_mut().poll_next(cx)).await
}

/// The rows of a [`Direction::Backward`]
/// [`TranTypeMasterStore::browse_tran_types`] page: `COTRTLIC`
/// `C-TR-TYPE-BACKWARD` (P-COTRTLIC-003)
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L354-367], opened by
/// `8100-READ-BACKWARDS` (P-COTRTLIC-033), with the filters of
/// P-COTRTLIC-020. Up to `limit` types in descending byte order of the code
/// padded to its `X(2)` bytes, strictly below `start`, or from the last type,
/// read on `conn` by the static statement of the keys present and received
/// one row at a time ([`fetch_each`]). `type_code` and `start` are
/// right-trimmed codes, `start` made NUL-free by `start_bound`; each bound
/// pads `start` back with `rpad`.
async fn types_backward(
    conn: &mut sqlx::PgConnection,
    type_code: Option<&str>,
    start: Option<&str>,
    pattern: Option<&str>,
    limit: i64,
) -> Fetched {
    let mut rows = match (type_code, start) {
        (Some(code), start) => sqlx::query_as!(
            MasterRow,
            r#"SELECT tr_type, tr_description
                     FROM transaction_types
                    WHERE tr_type = $1::text
                      AND ($2::text IS NULL OR rpad(tr_type, 2) COLLATE "C" < rpad($2::text, 2))
                      AND ($3::text IS NULL OR tr_description LIKE $3::text ESCAPE '')
                    ORDER BY rpad(tr_type, 2) COLLATE "C" DESC
                    LIMIT $4"#,
            code,
            start,
            pattern,
            limit,
        )
        .fetch(&mut *conn),
        (None, Some(start)) => sqlx::query_as!(
            MasterRow,
            r#"SELECT tr_type, tr_description
                     FROM transaction_types
                    WHERE rpad(tr_type, 2) COLLATE "C" < rpad($1::text, 2)
                      AND ($2::text IS NULL OR tr_description LIKE $2::text ESCAPE '')
                    ORDER BY rpad(tr_type, 2) COLLATE "C" DESC
                    LIMIT $3"#,
            start,
            pattern,
            limit,
        )
        .fetch(&mut *conn),
        (None, None) => sqlx::query_as!(
            MasterRow,
            r#"SELECT tr_type, tr_description
                     FROM transaction_types
                    WHERE ($1::text IS NULL OR tr_description LIKE $1::text ESCAPE '')
                    ORDER BY rpad(tr_type, 2) COLLATE "C" DESC
                    LIMIT $2"#,
            pattern,
            limit,
        )
        .fetch(&mut *conn),
    };
    fetch_each(|cx| rows.as_mut().poll_next(cx)).await
}

impl TranTypeFileReader for PgStore<'_, '_> {
    /// `CBTRN03C` `1500-B-LOOKUP-TRANTYPE` — P-CBTRN03C-005
    /// [app/cbl/CBTRN03C.cbl:L494-502]: the random `READ TRANTYPE-FILE INTO
    /// TRAN-TYPE-RECORD` by `FD-TRAN-TYPE`. `INVALID KEY` is
    /// [`PortError::NotFound`], and so is a code holding a NUL byte, without a
    /// query. The record is rebuilt with its `X(50)` description padded
    /// (P-CVTRA03Y-001, P-CVTRA03Y-002).
    async fn read_tran_type_file(&mut self, code: TranTypeCode) -> PortResult<TranTypeRecord> {
        let Some(key) = key_text(&code) else {
            return Err(PortError::NotFound);
        };
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query!(
            "SELECT tran_type, tran_type_desc FROM trantype_file WHERE tran_type = $1",
            key.as_ref(),
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
        Ok(TranTypeRecord {
            tran_type: tran_type_code(&row.tran_type, TYPE_FILE, "tran_type")?,
            tran_type_desc: alnum(&row.tran_type_desc, TYPE_FILE, "tran_type_desc")?,
        })
    }

    /// `CBTRN03C` `1500-C-LOOKUP-TRANCATG` — P-CBTRN03C-005
    /// [app/cbl/CBTRN03C.cbl:L504-512]: the random `READ TRANCATG-FILE INTO
    /// TRAN-CAT-RECORD` by `FD-TRAN-CAT-KEY`, the type code followed by the
    /// `9(04)` category. `INVALID KEY` is [`PortError::NotFound`], and so is a
    /// type code holding a NUL byte, without a query. The record is rebuilt
    /// with its `X(50)` description padded (P-CVTRA04Y-001 to P-CVTRA04Y-003).
    async fn read_tran_category_file(
        &mut self,
        code: TranTypeCode,
        category: TranCategoryCode,
    ) -> PortResult<TranCategoryRecord> {
        let Some(key) = key_text(&code) else {
            return Err(PortError::NotFound);
        };
        let mut sp = savepoint(self.tx, &mut self.ledger).await?;
        let result = sqlx::query!(
            "SELECT tran_type_cd, tran_cat_cd, tran_cat_type_desc
               FROM trancatg_file
              WHERE tran_type_cd = $1 AND tran_cat_cd = $2",
            key.as_ref(),
            i32::from(category.get()),
        )
        .fetch_optional(&mut *sp)
        .await;
        let row = settle(sp, result).await?.ok_or(PortError::NotFound)?;
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
    use carddemo_domain::numeric::{Alnum, TranTypeCode};
    use carddemo_domain::ports::TranTypeFilter;

    use super::{MASTER, MasterRow, key_text, matches_nothing, start_bound};
    use crate::PersistenceError;

    /// The code of the two `X(2)` bytes `bytes`.
    fn code(bytes: [u8; 2]) -> TranTypeCode {
        TranTypeCode::from_alnum(Alnum::from_array(bytes))
    }

    #[test]
    fn a_nul_key_binds_nothing_and_a_nul_start_key_its_nul_free_bound() {
        // (code, the key a keyed statement binds, the start bound a cursor binds)
        let cases = [
            (*b"01", Some("01"), "01"),
            (*b"7 ", Some("7"), "7"),
            (*b"  ", Some(""), ""),
            (*b"A\x01", Some("A\u{1}"), "A\u{1}"),
            (*b"\xE9 ", Some("\u{e9}"), "\u{e9}"),
            (*b"\0 ", None, "\u{1}\u{1}"),
            (*b"\0\0", None, "\u{1}\u{1}"),
            (*b"\0A", None, "\u{1}\u{1}"),
            (*b"0\0", None, "0\u{1}"),
            (*b"A\0", None, "A\u{1}"),
            (*b" \0", None, " \u{1}"),
            (*b"\xE9\0", None, "\u{e9}\u{1}"),
        ];
        for (bytes, key, bound) in cases {
            let tran_type = code(bytes);
            assert_eq!(key_text(&tran_type).as_deref(), key, "{bytes:?}");
            assert_eq!(start_bound(&tran_type), bound, "{bytes:?}");
        }
    }

    #[test]
    fn a_filter_holding_nul_matches_nothing() {
        let cases = [
            (None, None, false),
            (Some(*b"01"), None, false),
            (Some(*b"  "), Some("%PAY%"), false),
            (None, Some("%\u{1}%"), false),
            (Some(*b"0\0"), None, true),
            (Some(*b"\0\0"), None, true),
            (None, Some("%\0%"), true),
            (None, Some("%A\0B%"), true),
            (Some(*b"02"), Some("%PAY\0%"), true),
        ];
        for (type_code, description, expected) in cases {
            let filter = TranTypeFilter {
                type_code: type_code.map(code),
                description: description.map(str::to_owned),
            };
            assert_eq!(matches_nothing(&filter), expected, "{filter:?}");
        }
    }

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
