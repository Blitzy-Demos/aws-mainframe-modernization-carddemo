//! Repository of table `tran_cat_balances`: the TCATBALF KSDS of `TRAN-CAT-BAL-RECORD`
//! (copybook `CVTRA01Y`; CVTRA01Y-001 to CVTRA01Y-005), whose key `KEYS(17 0)` (`TCATBALF.jcl:40`)
//! is the primary key (`acct_id`, `tran_type_cd`, `tran_cat_cd`) of migration 0001.
//!
//! | Method | COBOL access | Rules |
//! |---|---|---|
//! | [`find_by_id`] | `READ TCATBAL-FILE` by `FD-TRAN-CAT-KEY` in `2700-UPDATE-TCATBAL` (`CBTRN02C.cbl:467-501`); `INVALID KEY`, status `23`, is `Ok(None)` | CBTRN02C-016 |
//! | [`insert`] | `WRITE FD-TRAN-CAT-BAL-RECORD` in `2700-A-CREATE-TCATBAL-REC` (`CBTRN02C.cbl:503-524`) | CBTRN02C-017 |
//! | [`update`] | `REWRITE FD-TRAN-CAT-BAL-RECORD` in `2700-B-UPDATE-TCATBAL-REC` (`CBTRN02C.cbl:526-543`) | CBTRN02C-019 |
//! | [`find_all`] | the sequential `READ TCATBAL-FILE` of `1000-TCATBALF-GET-NEXT` until status `10` (`CBACT04C.cbl:325-348`) | CBACT04C-003, CBACT04C-029 |
//! | [`replace_all`] | the TCATBALF job's `DELETE`, `DEFINE CLUSTER` and `REPRO` (`TCATBALF.jcl:22-62`) | LOADS-006, LOADS-014, LOADS-015, LOADS-018, LOADS-032 |
//!
//! A failed statement is the program's `ERROR READING`, `ERROR WRITING` or `ERROR REWRITING
//! TRANSACTION BALANCE FILE` path (CBTRN02C-038, CBACT04C-033); the caller ends the run on it.
//!
//! # Key order
//!
//! The KSDS returns records in the byte order of the 17-byte key in EBCDIC, where letters and the
//! space sort below digits, unlike ASCII. `tran_type_cd` is `PIC X(02)` text, so [`find_all`]
//! orders by `acct_id`, then by the code page 037 weights of the `tran_type_cd` bytes, then by
//! `tran_cat_cd`, never by a database collation (CBACT04C-003). The account and category are
//! digits, whose orders agree in both codes, and so is every type code the fixtures hold. The
//! statement reads in primary-key order, which the `"C"` collation of `tran_type_cd` makes the
//! ASCII byte order; a stable sort on the EBCDIC key then puts the rows in KSDS order, and keeps
//! the byte order between codes whose weights tie. The order fixes the order of an account's
//! SYSTRAN records and so their `TRAN-ID` suffixes in INTCALC.
//!
//! # Values
//!
//! Records cross codec B only through [`TranCatBalanceRow`], so the key, the balance at scale 2
//! and the 22 `FILLER` bytes are stored and read back verbatim, the `0` characters of every
//! `tcatbal.txt` record included (PIC-012, E05); the line framing of a fixture is never stored
//! (PIC-014). No arithmetic happens here: the `ADD DALYTRAN-AMT TO TRAN-CAT-BAL` of CBTRN02C, and
//! its truncation at `S9(09)V99`, is the caller's, applied to the record before [`insert`] or
//! [`update`] (ARITH-028, ARITH-029). The type code is taken by reference because `PicX` is not
//! `Copy`.
//!
//! No field of the record is protected, yet errors still carry no key or value, and this module
//! emits no tracing (Cardholder Data Redaction).
//!
//! [`find_by_id`]: TranCatBalanceRepository::find_by_id
//! [`insert`]: TranCatBalanceRepository::insert
//! [`update`]: TranCatBalanceRepository::update
//! [`find_all`]: TranCatBalanceRepository::find_all
//! [`replace_all`]: TranCatBalanceRepository::replace_all

use copybook::{PicU, PicX};
use domain::records::TranCatBalanceRecord;
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::{TranCatBalanceRow, pic_u_to_i64, pic_x_to_string};
use crate::{PgSession, PgStore, PgTx};

/// Column of `TRANCAT-ACCT-ID`, the first part of the key (CVTRA01Y-001).
const ACCT_ID: &str = "acct_id";
/// Column of `TRANCAT-TYPE-CD`, the second part of the key (CVTRA01Y-002).
const TRAN_TYPE_CD: &str = "tran_type_cd";
/// Column of `TRANCAT-CD`, the third part of the key (CVTRA01Y-003).
const TRAN_CAT_CD: &str = "tran_cat_cd";

/// Every access CBTRN02C, CBACT04C and the TCATBALF load make to the category balance file.
///
/// [`PgStore`] runs each method on a pooled connection in autocommit mode, except
/// [`replace_all`](Self::replace_all), which runs in a transaction of its own. [`PgSession`]
/// does the same on the session's one connection. [`PgTx`] runs every method inside the
/// caller's transaction, where a failed statement aborts the whole transaction: a caller that
/// must go on after one runs it inside [`PgTx::savepoint`].
pub trait TranCatBalanceRepository: Send {
    /// The balance whose `TRAN-CAT-KEY` is (`acct_id`, `type_cd`, `cat_cd`), or `None` when no
    /// row has that key: the `INVALID KEY` of the keyed read, status `23`, on which CBTRN02C
    /// creates the record (CBTRN02C-016).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when `type_cd` holds a NUL or non-ASCII byte, which no row
    /// can hold, or when the stored row does not fit the record's pictures;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the query fails
    /// (CBTRN02C-038).
    fn find_by_id(
        &mut self,
        acct_id: PicU<11>,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<TranCatBalanceRecord>, PersistenceError>> + Send;

    /// Every balance in the KSDS key order of the module documentation: `acct_id`, then
    /// `tran_type_cd` in EBCDIC order, then `tran_cat_cd`, all ascending, as CBACT04C reads the
    /// file from its first record to end of file (CBACT04C-003). Unbounded, for the batch jobs
    /// only: no collection endpoint reads this table.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a stored row does not fit the record's pictures;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the query fails
    /// (CBACT04C-033).
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TranCatBalanceRecord>, PersistenceError>> + Send;

    /// Writes `record` as a new row: the `WRITE` of `2700-A-CREATE-TCATBAL-REC`, whose record
    /// the caller has built with its key, balance and `FILLER` (CBTRN02C-017).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when a row already has the key, the KSDS `DUPREC`;
    /// [`PersistenceError::Decode`] when the type code or `FILLER` holds a NUL or non-ASCII
    /// byte; [`PersistenceError::Check`], [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] when the insert fails otherwise (CBTRN02C-038).
    fn insert(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Rewrites the row whose key is `record`'s with its balance and `FILLER`: the `REWRITE` of
    /// `2700-B-UPDATE-TCATBAL-REC` (CBTRN02C-019). `true` when exactly one row changed, `false`
    /// when no row has the key.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when the type code or `FILLER` holds a NUL or non-ASCII
    /// byte; [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] when the update
    /// fails (CBTRN02C-038).
    fn update(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Replaces the table's contents with `records`, inserted in slice order, and returns how
    /// many were inserted: the TCATBALF job's delete and redefinition of the cluster, then its
    /// `REPRO` (LOADS-006, LOADS-014). An empty table is no error (LOADS-015), and `FILLER`
    /// bytes are stored whatever they hold (LOADS-032). The delete and every insert form one
    /// transaction: [`PgStore`] and [`PgSession`] open and commit their own, [`PgTx`] uses the
    /// caller's.
    ///
    /// The delete is preceded by a `SHARE ROW EXCLUSIVE` lock on the table, held until the
    /// transaction ends, so a concurrent replacement or row write waits for it and then sees its
    /// committed rows: two loads at once leave the input of the one that commits last, never
    /// their union (LOADS-040). Reads do not wait.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a record's type code or `FILLER` holds a NUL or
    /// non-ASCII byte: every record is converted before the delete, so the call fails with
    /// nothing written, leaving the table and a [`PgTx`] caller's transaction as they were.
    /// [`PersistenceError::Duplicate`] when two records share a key, or any other error of the
    /// delete or an insert, as for [`insert`](Self::insert). On [`PgStore`] and [`PgSession`]
    /// the transaction then rolls back and the previous contents remain (LOADS-018); on [`PgTx`]
    /// the failed statement aborts the caller's transaction.
    fn replace_all(
        &mut self,
        records: &[TranCatBalanceRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

impl TranCatBalanceRepository for PgStore {
    fn find_by_id(
        &mut self,
        acct_id: PicU<11>,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<TranCatBalanceRecord>, PersistenceError>> + Send {
        select_by_id(self.pool(), acct_id, type_cd, cat_cd)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TranCatBalanceRecord>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_one(self.pool(), record)
    }

    fn update(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_one(self.pool(), record)
    }

    fn replace_all(
        &mut self,
        records: &[TranCatBalanceRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        // The transaction's future owns a pool handle and borrows nothing from `self`.
        let begin = self.begin();
        async move {
            // Dropped without a commit when a statement fails, which rolls the delete back.
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl TranCatBalanceRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        acct_id: PicU<11>,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<TranCatBalanceRecord>, PersistenceError>> + Send {
        select_by_id(self.conn(), acct_id, type_cd, cat_cd)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TranCatBalanceRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_one(self.conn(), record)
    }

    fn update(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_one(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[TranCatBalanceRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), records)
    }
}

impl TranCatBalanceRepository for PgSession {
    fn find_by_id(
        &mut self,
        acct_id: PicU<11>,
        type_cd: &PicX<2>,
        cat_cd: PicU<4>,
    ) -> impl Future<Output = Result<Option<TranCatBalanceRecord>, PersistenceError>> + Send {
        select_by_id(self.conn(), acct_id, type_cd, cat_cd)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<TranCatBalanceRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_one(self.conn(), record)
    }

    fn update(
        &mut self,
        record: &TranCatBalanceRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_one(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[TranCatBalanceRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // Dropped without a commit when a statement fails, which rolls the delete back.
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

/// The keyed read of `2700-UPDATE-TCATBAL` (CBTRN02C-016).
async fn select_by_id<'e, E>(
    executor: E,
    acct_id: PicU<11>,
    type_cd: &PicX<2>,
    cat_cd: PicU<4>,
) -> Result<Option<TranCatBalanceRecord>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let acct_id = pic_u_to_i64(ACCT_ID, acct_id)?;
    let type_cd = pic_x_to_string(TRAN_TYPE_CD, type_cd)?;
    let cat_cd = pic_u_to_i64(TRAN_CAT_CD, cat_cd)?;
    // The key text is cast to `bpchar`: `sqlx` binds a `String` as `text`, against which the
    // `CHAR(n)` column compares as `text`, its trailing spaces dropped, so a key that ends in
    // spaces would find no row and the primary key could not serve the statement.
    let row = sqlx::query_as!(
        TranCatBalanceRow,
        "SELECT acct_id, tran_type_cd, tran_cat_cd, tran_cat_bal, filler
           FROM tran_cat_balances
          WHERE acct_id = $1 AND tran_type_cd = $2::bpchar AND tran_cat_cd = $3",
        acct_id,
        type_cd,
        cat_cd
    )
    .fetch_optional(executor)
    .await?;
    row.map(TranCatBalanceRow::into_record).transpose()
}

/// The sequential read of `1000-TCATBALF-GET-NEXT`, in KSDS key order (CBACT04C-003).
async fn select_all<'e, E>(executor: E) -> Result<Vec<TranCatBalanceRecord>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as!(
        TranCatBalanceRow,
        "SELECT acct_id, tran_type_cd, tran_cat_cd, tran_cat_bal, filler
           FROM tran_cat_balances
          ORDER BY acct_id, tran_type_cd, tran_cat_cd"
    )
    .fetch_all(executor)
    .await?;
    let mut records = rows
        .into_iter()
        .map(TranCatBalanceRow::into_record)
        .collect::<Result<Vec<_>, _>>()?;
    // KSDS order rather than the column's byte collation: the type code sorts by its EBCDIC
    // weights (CBACT04C-003). Stable, so equal weights keep the statement's byte order.
    records.sort_by_key(ksds_key);
    Ok(records)
}

/// The `WRITE` of `2700-A-CREATE-TCATBAL-REC` (CBTRN02C-017): `record` converted by
/// [`TranCatBalanceRow::from_record`] and bound by [`insert_row`].
async fn insert_one<'e, E>(
    executor: E,
    record: &TranCatBalanceRecord,
) -> Result<(), PersistenceError>
where
    E: PgExecutor<'e>,
{
    let row = TranCatBalanceRow::from_record(record)?;
    insert_row(executor, &row).await
}

/// The one `INSERT` statement, which [`insert_one`] and [`replace_all_in`] share: every column
/// bound from `row`, already converted through codec B.
async fn insert_row<'e, E>(executor: E, row: &TranCatBalanceRow) -> Result<(), PersistenceError>
where
    E: PgExecutor<'e>,
{
    sqlx::query!(
        "INSERT INTO tran_cat_balances (acct_id, tran_type_cd, tran_cat_cd, tran_cat_bal, filler)
         VALUES ($1, $2, $3, $4, $5)",
        row.acct_id,
        row.tran_type_cd,
        row.tran_cat_cd,
        row.tran_cat_bal,
        row.filler
    )
    .execute(executor)
    .await?;
    Ok(())
}

/// The `REWRITE` of `2700-B-UPDATE-TCATBAL-REC` (CBTRN02C-019): the key selects the row, and the
/// balance and `FILLER` are written.
async fn update_one<'e, E>(
    executor: E,
    record: &TranCatBalanceRecord,
) -> Result<bool, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let row = TranCatBalanceRow::from_record(record)?;
    // The key text is cast to `bpchar`: `sqlx` binds a `String` as `text`, against which the
    // `CHAR(n)` column compares as `text`, its trailing spaces dropped, so a key that ends in
    // spaces would find no row and the primary key could not serve the statement.
    let result = sqlx::query!(
        "UPDATE tran_cat_balances
            SET tran_cat_bal = $4, filler = $5
          WHERE acct_id = $1 AND tran_type_cd = $2::bpchar AND tran_cat_cd = $3",
        row.acct_id,
        row.tran_type_cd,
        row.tran_cat_cd,
        row.tran_cat_bal,
        row.filler
    )
    .execute(executor)
    .await?;
    Ok(result.rows_affected() == 1)
}

/// The TCATBALF load on `conn`, which is inside a transaction: every row deleted, then every
/// record inserted in slice order (LOADS-006, LOADS-014).
///
/// Every record is converted through codec B before the delete, so a record that
/// [`TranCatBalanceRow::from_record`] cannot store fails the call before any statement is sent:
/// the table, and a caller's transaction, are left as they were (LOADS-018).
///
/// The first statement locks the table `SHARE ROW EXCLUSIVE` until the transaction ends, so
/// replacements of `tran_cat_balances` run one at a time and the last to commit leaves its
/// input (LOADS-040).
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[TranCatBalanceRecord],
) -> Result<u64, PersistenceError> {
    let rows = records
        .iter()
        .map(TranCatBalanceRow::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    // One replacement of the table at a time (LOADS-014, LOADS-040): this mode conflicts with
    // itself and with the ROW EXCLUSIVE lock of every row write, so a second load waits here
    // until this transaction ends, and its delete then removes the rows this one committed.
    // Readers take ACCESS SHARE and never wait.
    sqlx::query!("LOCK TABLE tran_cat_balances IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM tran_cat_balances")
        .execute(&mut *conn)
        .await?;
    let mut inserted: u64 = 0;
    for row in &rows {
        insert_row(&mut *conn, row).await?;
        inserted += 1;
    }
    Ok(inserted)
}

/// The KSDS order of `record`'s `TRAN-CAT-KEY`: account, the EBCDIC weights of the type code,
/// category (CBACT04C-003).
fn ksds_key(record: &TranCatBalanceRecord) -> (PicU<11>, [u8; 2], PicU<4>) {
    (
        record.acct_id,
        record.type_cd.as_bytes().map(ebcdic_weight),
        record.cd,
    )
}

/// The code page 037 code of a byte, its weight in the EBCDIC collating sequence of the KSDS.
///
/// Codec A admits printable ASCII only into a `PIC X` field (PIC-002), and each of those 95
/// characters has its own code. Any other byte a row could hold weighs `0xFF`, above every
/// printable character, so the order stays total; the stable sort of [`select_all`] orders such
/// ties by their bytes.
const fn ebcdic_weight(byte: u8) -> u8 {
    match byte {
        b' ' => 0x40,
        b'.' => 0x4B,
        b'<' => 0x4C,
        b'(' => 0x4D,
        b'+' => 0x4E,
        b'|' => 0x4F,
        b'&' => 0x50,
        b'!' => 0x5A,
        b'$' => 0x5B,
        b'*' => 0x5C,
        b')' => 0x5D,
        b';' => 0x5E,
        b'-' => 0x60,
        b'/' => 0x61,
        b',' => 0x6B,
        b'%' => 0x6C,
        b'_' => 0x6D,
        b'>' => 0x6E,
        b'?' => 0x6F,
        b'`' => 0x79,
        b':' => 0x7A,
        b'#' => 0x7B,
        b'@' => 0x7C,
        b'\'' => 0x7D,
        b'=' => 0x7E,
        b'"' => 0x7F,
        b'a'..=b'i' => 0x81 + (byte - b'a'),
        b'j'..=b'r' => 0x91 + (byte - b'j'),
        b'~' => 0xA1,
        b's'..=b'z' => 0xA2 + (byte - b's'),
        b'^' => 0xB0,
        b'[' => 0xBA,
        b']' => 0xBB,
        b'{' => 0xC0,
        b'A'..=b'I' => 0xC1 + (byte - b'A'),
        b'}' => 0xD0,
        b'J'..=b'R' => 0xD1 + (byte - b'J'),
        b'\\' => 0xE0,
        b'S'..=b'Z' => 0xE2 + (byte - b'S'),
        b'0'..=b'9' => 0xF0 + (byte - b'0'),
        _ => 0xFF,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use copybook::framing::decode_all;
    use copybook::{PicU, PicX};
    use domain::money::Money;
    use domain::records::TranCatBalanceRecord;
    use rust_decimal::Decimal;

    use super::{TranCatBalanceRepository, ebcdic_weight, ksds_key};
    use crate::{DecodeKind, PersistenceError, PgStore};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// The fixture of the TCATBALF load, read in place (AAP 0.2.2).
    const TCATBAL: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../app/data/ASCII/tcatbal.txt"
    );

    /// The error of a write whose key a row already has.
    fn duplicate() -> PersistenceError {
        PersistenceError::Duplicate {
            constraint: Some("tran_cat_balances_pkey".to_owned()),
        }
    }

    /// A record with key (`acct_id`, `type_cd`, `cd`), balance `bal` and `tcatbal.txt`'s
    /// `FILLER` of 22 `0` characters.
    fn record(
        acct_id: u64,
        type_cd: [u8; 2],
        cd: u64,
        bal: &str,
    ) -> Result<TranCatBalanceRecord, Box<dyn StdError>> {
        Ok(TranCatBalanceRecord {
            acct_id: PicU::new(acct_id)?,
            type_cd: PicX::from_array(type_cd),
            cd: PicU::new(cd)?,
            bal: Money::new(bal.parse::<Decimal>()?)?,
            filler: PicX::from_array([b'0'; 22]),
        })
    }

    /// Code page 037 places the space below every other printable character, then the
    /// specials, lower case, upper case and the digits last; every printable character has its
    /// own weight, and any other byte weighs most.
    #[test]
    fn bytes_weigh_their_code_page_037_codes() {
        let cases = [
            (b' ', 0x40),
            (b'-', 0x60),
            (b'a', 0x81),
            (b'r', 0x99),
            (b'z', 0xA9),
            (b'A', 0xC1),
            (b'J', 0xD1),
            (b'S', 0xE2),
            (b'Z', 0xE9),
            (b'0', 0xF0),
            (b'9', 0xF9),
            (0x00, 0xFF),
            (b'\t', 0xFF),
            (0x7F, 0xFF),
        ];
        for (byte, weight) in cases {
            assert_eq!(ebcdic_weight(byte), weight, "{byte:#04x}");
        }
        let mut weights: Vec<u8> = (0x20..=0x7E).map(ebcdic_weight).collect();
        weights.sort_unstable();
        weights.dedup();
        assert_eq!(weights.len(), 95);
        assert!(weights.iter().all(|&weight| weight < 0xFF));
    }

    /// The type codes of one account in KSDS order, which ASCII order would reverse in part
    /// (CBACT04C-003).
    fn ksds_ordered() -> Result<Vec<TranCatBalanceRecord>, Box<dyn StdError>> {
        Ok(vec![
            record(1, *b" 1", 1, "1.00")?,
            record(1, *b"a1", 1, "2.00")?,
            record(1, *b"A1", 9, "3.00")?,
            record(1, *b"0A", 5, "-4.00")?,
            record(1, *b"01", 1, "5.00")?,
            record(1, *b"01", 2, "-6.00")?,
            record(2, *b"01", 1, "7.00")?,
        ])
    }

    // audit: CBACT04C-003
    #[test]
    fn keys_order_by_account_then_ebcdic_type_code_then_category() -> TestResult {
        let expected = ksds_ordered()?;
        let mut sorted = expected.clone();
        sorted.reverse();
        sorted.sort_by_key(ksds_key);
        assert_eq!(sorted, expected);
        let mut ascii = expected.clone();
        ascii.sort_by(|a, b| {
            (a.acct_id, a.type_cd.as_bytes(), a.cd).cmp(&(b.acct_id, b.type_cd.as_bytes(), b.cd))
        });
        assert_ne!(ascii, expected);
        Ok(())
    }

    // audit: CBTRN02C-016
    // audit: CBTRN02C-017
    // audit: CBTRN02C-019
    #[tokio::test]
    async fn keyed_read_write_and_rewrite_follow_2700_update_tcatbal() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let first = record(50, *b"01", 1, "-12.34")?;
        let key = (first.acct_id, first.type_cd.clone(), first.cd);

        assert_eq!(store.find_by_id(key.0, &key.1, key.2).await?, None);
        store.insert(&first).await?;
        assert_eq!(
            store.find_by_id(key.0, &key.1, key.2).await?,
            Some(first.clone())
        );
        assert_eq!(store.insert(&first).await, Err(duplicate()));

        let mut rewritten = first.clone();
        rewritten.bal = Money::new("999999999.99".parse::<Decimal>()?)?;
        rewritten.filler = PicX::from_array(*b"FILLER BYTES KEPT 0000");
        assert!(store.update(&rewritten).await?);
        assert_eq!(
            store.find_by_id(key.0, &key.1, key.2).await?,
            Some(rewritten.clone())
        );
        let absent = record(50, *b"01", 2, "1.00")?;
        assert!(!store.update(&absent).await?);
        assert_eq!(
            store
                .find_by_id(absent.acct_id, &absent.type_cd, absent.cd)
                .await?,
            None
        );

        // Inside the caller's transaction: visible there, gone with its rollback.
        let mut tx = store.begin().await?;
        tx.insert(&absent).await?;
        let mut updated = absent.clone();
        updated.bal = Money::new("-999999999.99".parse::<Decimal>()?)?;
        assert!(tx.update(&updated).await?);
        assert_eq!(
            tx.find_by_id(absent.acct_id, &absent.type_cd, absent.cd)
                .await?,
            Some(updated.clone())
        );
        assert_eq!(tx.find_all().await?, vec![rewritten.clone(), updated]);
        tx.rollback().await?;
        assert_eq!(store.find_all().await?, vec![rewritten]);
        Ok(())
    }

    // audit: CBACT04C-003
    #[tokio::test]
    async fn find_all_reads_in_ksds_key_order() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let expected = ksds_ordered()?;
        for index in [3, 6, 0, 5, 2, 4, 1] {
            let record = expected.get(index).ok_or("no such record")?;
            store.insert(record).await?;
        }
        assert_eq!(store.find_all().await?, expected);
        Ok(())
    }

    // audit: LOADS-006
    // audit: LOADS-014
    // audit: LOADS-015
    // audit: LOADS-018
    // audit: LOADS-032
    #[tokio::test]
    async fn replace_all_loads_tcatbal_and_replaces_in_one_transaction() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let fixture: Vec<TranCatBalanceRecord> = decode_all(&std::fs::read(TCATBAL)?)?
            .into_iter()
            .map(|framed| framed.record)
            .collect();
        assert_eq!(fixture.len(), 50);

        // Freshly migrated, then loaded: the table holds the input set, FILLER kept.
        for _ in 0..2 {
            assert_eq!(store.replace_all(&fixture).await?, 50);
            assert_eq!(store.find_all().await?, fixture);
        }

        let replacement = vec![
            record(7, *b"02", 3, "0.01")?,
            record(8, *b"01", 1, "-0.01")?,
        ];
        assert_eq!(store.replace_all(&replacement).await?, 2);
        assert_eq!(store.find_all().await?, replacement);

        // A failing insert rolls the delete back with it.
        let twice = [fixture.clone(), fixture.clone()].concat();
        assert_eq!(store.replace_all(&twice).await, Err(duplicate()));
        assert_eq!(store.find_all().await?, replacement);

        // On a `PgTx` the caller's transaction decides.
        let head = fixture.get(..3).ok_or("fixture too short")?;
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(head).await?, 3);
        assert_eq!(tx.find_all().await?, head);
        assert_eq!(store.find_all().await?, replacement);
        tx.rollback().await?;
        assert_eq!(store.find_all().await?, replacement);

        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&[]).await?, 0);
        tx.commit().await?;
        assert_eq!(store.find_all().await?, Vec::new());
        Ok(())
    }

    /// A record codec B cannot store, after one it can, fails the replacement before the delete:
    /// through the pool nothing changes, and a caller's transaction is left usable and unchanged,
    /// so committing it after the error keeps the previous contents.
    // audit: LOADS-018
    #[tokio::test]
    async fn an_unstorable_record_fails_the_replacement_before_the_delete() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let original = vec![
            record(7, *b"02", 3, "0.01")?,
            record(8, *b"01", 1, "-0.01")?,
        ];
        assert_eq!(store.replace_all(&original).await?, 2);
        let input = [
            record(9, *b"01", 1, "1.00")?,
            record(9, [b'0', 0xFF], 2, "2.00")?,
        ];
        let invalid = Err(PersistenceError::decode(
            "tran_type_cd",
            DecodeKind::InvalidByte,
        ));

        assert_eq!(store.replace_all(&input).await, invalid);
        assert_eq!(
            store.find_all().await?,
            original,
            "a failed load through the pool leaves the previous contents"
        );

        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(&input).await, invalid);
        assert_eq!(
            tx.find_all().await?,
            original,
            "the transaction deleted nothing and is not aborted"
        );
        tx.commit().await?;
        assert_eq!(
            store.find_all().await?,
            original,
            "committing after the error keeps the previous contents"
        );
        Ok(())
    }
}
