//! Repository of table `card_xref`, the card cross-reference `CARD-XREF-RECORD` of copybook
//! `CVACT03Y` (migration 0001; CVACT03Y-001 to CVACT03Y-004).
//!
//! The KSDS `CARDXREF` is keyed by `XREF-CARD-NUM` (`KEYS(16 0)`, `XREFFILE.jcl:43`), which the
//! table keeps as its primary key `card_xref_pkey` (LOADS-003). Its alternate index on
//! `XREF-ACCT-ID` (`KEYS(11,25)` `NONUNIQUEKEY`, `XREFFILE.jcl:72-75`), read online through the
//! path `CXACAIX`, is replaced by the non-unique index `card_xref_acct_id_idx` on
//! `(acct_id, card_num)`, which keeps an account's rows in prime-key order (LOADS-023, LOADS-024).
//!
//! | COBOL access | Method |
//! |---|---|
//! | Keyed read by card number: CBTRN02C `1500-A-LOOKUP-XREF` (`CBTRN02C.cbl:380-391`), CBTRN03C `1500-A-LOOKUP-XREF` (`CBTRN03C.cbl:484-492`), COTRN02C `READ-CCXREF-FILE` (`COTRN02C.cbl:609-637`), COPAUA0C `5100-READ-XREF-RECORD` (`COPAUA0C.cbl:472-514`) | [`CardXrefRepository::find_by_id`] |
//! | Keyed read through the account path `CXACAIX`: COACTVWC and COACTUPC `9200-GETCARDXREF-BYACCT` (`COACTVWC.cbl:723-770`, `COACTUPC.cbl:3650-3698`), COBIL00C `READ-CXACAIX-FILE` (`COBIL00C.cbl:408-436`), COTRN02C `READ-CXACAIX-FILE` (`COTRN02C.cbl:576-604`), COPAUS0C `GETCARDXREF-BYACCT` (`COPAUS0C.cbl:812-862`), CBACT04C `1110-GET-XREF-DATA`, `READ … KEY IS FD-XREF-ACCT-ID` (`CBACT04C.cbl:393-398`) | [`CardXrefRepository::find_by_acct_id`] |
//! | The first two records sharing one alternate key, which tell a `NORMAL` read through `CXACAIX` from a `DUPKEY` one for the programs that branch on that response (COACTVWC-039, COACTUPC-089, COPAUS0C-015, COTRN02C-014) | [`CardXrefRepository::find_first_two_by_acct_id`] |
//! | Sequential read in key order: CBSTM03B `2000-XREFFILE-PROC` (`CBSTM03B.CBL:157-175`), driven by CREASTMT | [`CardXrefRepository::find_all`] |
//! | IDCAMS `DELETE`, `DEFINE CLUSTER` and `REPRO` of XREFFILE (`XREFFILE.jcl:25-29,39-52,64`), batch `xreffile` | [`CardXrefRepository::replace_all`] |
//!
//! # Alternate-index order
//!
//! A keyed `READ` through a non-unique alternate index returns the first base record that the
//! alternate-index record lists for the key. `BLDINDEX` (`XREFFILE.jcl:100-102`) lists the prime
//! keys of a shared alternate key in ascending order, and no in-scope program adds a
//! cross-reference afterwards, so the first record is the one with the lowest card number.
//! [`find_by_acct_id`](CardXrefRepository::find_by_acct_id) therefore orders by `card_num` and
//! keeps the first row. The account chain of `accounts.rs` and
//! [`find_first_two_by_acct_id`](CardXrefRepository::find_first_two_by_acct_id) read the first
//! two rows of the account through `select_first_two_by_acct_id` instead, because their programs
//! branch on whether other records share the key: one row is `NORMAL`, two are `DUPKEY`
//! (COACTVWC-039, COACTUPC-089). A second row already decides that response, so no read through
//! the account path loads every row of an account, however many cards it holds. `card_num` is
//! declared `COLLATE "C"` in the migration, so that order is bytewise,
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
use crate::{PgSession, PgStore, PgTx};

/// Column of `XREF-CARD-NUM`, named by a key conversion error.
const CARD_NUM: &str = "card_num";
/// Column of `XREF-ACCT-ID`, named by a key conversion error.
const ACCT_ID: &str = "acct_id";

/// The `card_xref` table, implemented by [`PgStore`] (each statement on a pooled connection,
/// committed on its own), [`PgTx`] (inside the caller's transaction) and [`PgSession`] (each
/// statement on the session's one connection, committed on its own).
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
    /// `1110-GET-XREF-DATA`, served by `card_xref_acct_id_idx`, whose `card_num` order yields
    /// that row first, so the read stops at it without a sort. When other rows share the
    /// account, CICS still returns this record but with the `DUPKEY` response, which this method
    /// does not report: a caller whose program branches on that response, as COACTVWC-039
    /// records, reads [`find_first_two_by_acct_id`](Self::find_first_two_by_acct_id) instead.
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

    /// The first two cross-references of account `acct_id` in card-number order, never more:
    /// none when no row has the account (CICS `NOTFND`), one when the record is the only one
    /// with the alternate key (`NORMAL`), and two when other records share it (`DUPKEY`). The
    /// first row is the record [`find_by_acct_id`](Self::find_by_acct_id) returns.
    ///
    /// The `CXACAIX` read of the programs that branch on its response (COACTVWC-039,
    /// COACTUPC-089, COPAUS0C-015, COTRN02C-014), served by `card_xref_acct_id_idx`, whose
    /// `card_num` order delivers the rows sorted, so the scan reads no row past the second. The
    /// statement's `LIMIT 2` bounds the read whatever the account holds: a second row already
    /// decides `DUPKEY`, so the rows after it would change no outcome, and an account with many
    /// cards returns, allocates and decodes no more rows than one with two. Each fixture account
    /// has exactly one row (COACTVWC-039).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for the first column, in row order and then migration
    /// order, whose stored value does not fit its picture; [`PersistenceError::Unavailable`]
    /// when the database cannot be reached; [`PersistenceError::Database`] for any other failure
    /// of the statement.
    fn find_first_two_by_acct_id(
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
    /// The delete and the inserts form one unit: [`PgStore`] and [`PgSession`] run them in a
    /// transaction of their own and commit it, [`PgTx`] inside the caller's transaction. No
    /// foreign key joins `card_xref` to another table, so the load succeeds whatever any other
    /// load did first (LOADS-025).
    ///
    /// The delete is preceded by a `SHARE ROW EXCLUSIVE` lock on the table, held until the
    /// transaction ends, so a concurrent replacement or row write waits for it and then sees its
    /// committed rows: two loads at once leave the input of the one that commits last, never
    /// their union (LOADS-040). Reads do not wait.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a record holds a byte no row can store: every record is
    /// converted before the delete, so the call fails with nothing written, leaving the table
    /// and a [`PgTx`] caller's transaction as they were. [`PersistenceError::Duplicate`] when two
    /// records share a card number, and [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] when a statement or the commit fails. On [`PgStore`] and
    /// [`PgSession`] the transaction is then rolled back and the previous contents remain
    /// (LOADS-018); on [`PgTx`] the failed statement aborts the caller's transaction.
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

    fn find_first_two_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send {
        select_first_two_by_acct_id(self.pool(), acct_id)
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

    fn find_first_two_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send {
        select_first_two_by_acct_id(self.conn(), acct_id)
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

impl CardXrefRepository for PgSession {
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

    fn find_first_two_by_acct_id(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = Result<Vec<CardXrefRecord>, PersistenceError>> + Send {
        select_first_two_by_acct_id(self.conn(), acct_id)
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

/// The cross-reference of `card`, bound through codec B from [`CardNumber::expose`].
async fn select_by_card<'e, E: PgExecutor<'e>>(
    executor: E,
    card: &CardNumber,
) -> Result<Option<CardXrefRecord>, PersistenceError> {
    let card_num = pic_x_to_string(CARD_NUM, card.expose())?;
    // The key text is cast to `bpchar`: `sqlx` binds a `String` as `text`, against which the
    // `CHAR(n)` column compares as `text`, its trailing spaces dropped, so a key that ends in
    // spaces would find no row and the primary key could not serve the statement.
    let row = sqlx::query_as!(
        CardXrefRow,
        "SELECT card_num, cust_id, acct_id, filler FROM card_xref WHERE card_num = $1::bpchar",
        card_num
    )
    .fetch_optional(executor)
    .await?;
    row.map(CardXrefRow::into_record).transpose()
}

/// The first cross-reference of account `acct_id` in alternate-index order, the lowest card
/// number, or `None` when no row has the account. The one statement of the `CXACAIX` read that
/// [`CardXrefRepository::find_by_acct_id`] runs.
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

/// The first two cross-references of account `acct_id`, in card-number order, never more: the
/// one statement of [`CardXrefRepository::find_first_two_by_acct_id`], shared with the account
/// chain of `accounts.rs`. Both tell a `CXACAIX` read that finds one record (`NORMAL`) from one
/// whose key other records share (`DUPKEY`) by the number of rows (COACTVWC-039, COACTUPC-089),
/// which a second row already decides; the literal `LIMIT 2` keeps the read at two rows however
/// many cards the account holds.
///
/// # Errors
///
/// [`PersistenceError::Decode`] for the first column, in row order and then migration order,
/// whose stored value does not fit its picture; [`PersistenceError::Unavailable`] or
/// [`PersistenceError::Database`] when the statement fails.
pub(super) async fn select_first_two_by_acct_id<'e, E: PgExecutor<'e>>(
    executor: E,
    acct_id: PicU<11>,
) -> Result<Vec<CardXrefRecord>, PersistenceError> {
    let acct_id = pic_u_to_i64(ACCT_ID, acct_id)?;
    let rows = sqlx::query_as!(
        CardXrefRow,
        "SELECT card_num, cust_id, acct_id, filler FROM card_xref WHERE acct_id = $1 \
         ORDER BY card_num LIMIT 2",
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

/// Inserts the row of `record`, converted by [`CardXrefRow::from_record`] and bound by
/// [`insert_row`].
async fn insert_one<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &CardXrefRecord,
) -> Result<(), PersistenceError> {
    let row = CardXrefRow::from_record(record)?;
    insert_row(executor, &row).await
}

/// The one `INSERT` statement, which [`insert_one`] and [`replace_all_in`] share: every column
/// bound from `row`, already converted through codec B.
async fn insert_row<'e, E: PgExecutor<'e>>(
    executor: E,
    row: &CardXrefRow,
) -> Result<(), PersistenceError> {
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
///
/// Every record is converted through codec B before the delete, so a record that
/// [`CardXrefRow::from_record`] cannot store fails the call before any statement is sent: the
/// table, and a caller's transaction, are left as they were (LOADS-018).
///
/// The first statement locks the table `SHARE ROW EXCLUSIVE` until the transaction ends, so
/// replacements of `card_xref` run one at a time and the last to commit leaves its input
/// (LOADS-014, LOADS-040).
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[CardXrefRecord],
) -> Result<u64, PersistenceError> {
    let rows = records
        .iter()
        .map(CardXrefRow::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    // One replacement of the table at a time (LOADS-014, LOADS-040): this mode conflicts with
    // itself and with the ROW EXCLUSIVE lock of every row write, so a second load waits here
    // until this transaction ends, and its delete then removes the rows this one committed.
    // Readers take ACCESS SHARE and never wait.
    sqlx::query!("LOCK TABLE card_xref IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM card_xref")
        .execute(&mut *conn)
        .await?;
    let mut inserted: u64 = 0;
    for row in &rows {
        insert_row(&mut *conn, row).await?;
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

    use super::{CardXrefRepository, select_first_by_acct_id, select_first_two_by_acct_id};
    use crate::error::{DecodeKind, PersistenceError};
    use crate::{PgStore, PgTx};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Runs `test` to completion on a current-thread Tokio runtime built here, with no lint
    /// attribute.
    fn block_on(test: impl Future<Output = TestResult>) -> TestResult {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(test)
    }

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
    #[test]
    fn fixture_load_runs_twice_and_every_read_path_finds_each_record() -> TestResult {
        block_on(async {
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
                    store.find_first_two_by_acct_id(record.acct_id).await? == [record.clone()],
                    "each fixture account has exactly one cross-reference"
                );
            }

            let absent_card = CardNumber::new(PicX::from_array(*b"0000000000000000"));
            assert!(store.find_by_id(&absent_card).await?.is_none());
            let absent_account = PicU::new(ABSENT_ACCOUNT)?;
            assert!(store.find_by_acct_id(absent_account).await?.is_none());
            assert!(
                store
                    .find_first_two_by_acct_id(absent_account)
                    .await?
                    .is_empty()
            );
            Ok(())
        })
    }

    // audit: LOADS-023
    #[test]
    fn an_account_with_several_cards_reads_the_lowest_card_number_first() -> TestResult {
        block_on(async {
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
                store.find_first_two_by_acct_id(shared).await? == [lowest.clone(), middle.clone()],
                "the probe returns the two lowest card numbers of the account, in order, never more"
            );
            assert!(
                store.find_first_two_by_acct_id(other.acct_id).await? == [other.clone()],
                "an account with one record returns that record alone"
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

            // The same statements inside a transaction, the one the account chain shares included.
            let mut tx: PgTx<'static> = store.begin().await?;
            assert!(
                select_first_by_acct_id(tx.conn(), shared).await? == Some(lowest.clone()),
                "the keyed statement keeps the lowest card number"
            );
            assert!(
                select_first_two_by_acct_id(tx.conn(), shared).await?
                    == [lowest.clone(), middle.clone()],
                "the statement the account chain shares returns the two lowest card numbers"
            );
            assert!(tx.find_by_acct_id(shared).await? == Some(lowest.clone()));
            assert!(
                tx.find_first_two_by_acct_id(shared).await? == [lowest.clone(), middle.clone()]
            );
            assert!(tx.find_first_two_by_acct_id(other.acct_id).await? == [other.clone()]);
            let absent_account = PicU::new(ABSENT_ACCOUNT)?;
            assert!(
                tx.find_first_two_by_acct_id(absent_account)
                    .await?
                    .is_empty()
            );
            assert!(tx.find_by_id(&other.card_num).await? == Some(other.clone()));
            assert!(tx.find_all().await?.len() == 4);
            tx.rollback().await?;
            Ok(())
        })
    }

    #[test]
    fn a_duplicate_card_number_is_typed_and_a_savepoint_keeps_the_transaction() -> TestResult {
        block_on(async {
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
        })
    }

    // audit: LOADS-014
    // audit: LOADS-018
    #[test]
    fn replace_all_keeps_only_its_input_and_fails_as_a_whole() -> TestResult {
        block_on(async {
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
        })
    }

    /// A record codec B cannot store, after one it can, fails the replacement before the delete:
    /// through the pool nothing changes, and a caller's transaction is left usable and unchanged,
    /// so committing it after the error keeps the previous contents.
    // audit: LOADS-018
    #[test]
    fn an_unstorable_record_fails_the_replacement_before_the_delete() -> TestResult {
        block_on(async {
            let guard = start_postgres().await?;
            let mut store = PgStore::new(guard.pool().clone());
            let original = [
                xref(*b"6100000000000001", 1, 1)?,
                xref(*b"6100000000000002", 2, 2)?,
            ];
            assert_eq!(store.replace_all(&original).await?, 2);
            let unstorable = CardXrefRecord {
                filler: PicX::from_array(*b"             \0"),
                ..xref(*b"6100000000000004", 4, 4)?
            };
            let input = [xref(*b"6100000000000003", 3, 3)?, unstorable];
            let invalid = Err(PersistenceError::decode("filler", DecodeKind::InvalidByte));

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
        })
    }
}
