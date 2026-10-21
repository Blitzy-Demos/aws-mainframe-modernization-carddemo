//! [`CardRepository`]: table `cards`, the card master `CARDDAT` (`CVACT02Y` `CARD-RECORD`, rows
//! CVACT02Y-001 to CVACT02Y-008 of `modern/audit/copybooks.md`).
//!
//! The KSDS key, `KEYS(16 0)` (`CARDFILE.jcl:54`), is the primary key `cards_pkey` on
//! `card_num`. The alternate index on `CARD-ACCT-ID`, `KEYS(11 16)` (`CARDFILE.jcl:83-85`), is the
//! index `cards_acct_id_idx (acct_id, card_num)` of migration 0001 (LOADS-022). `card_ref`, unique
//! through `cards_card_ref_key`, is no copybook field: the database assigns it, and callers send it
//! outward in place of the card number (CVACT02Y-008, Cardholder Data Redaction, E32). Every read
//! therefore returns a [`CardEntry`], the handle beside the record.
//!
//! # The COBOL accesses it replaces
//!
//! | COBOL access | Method |
//! |---|---|
//! | COCRDLIC `9000-READ-FORWARD`: `STARTBR GTEQ`, then `READNEXT`, each record through `9500-FILTER-RECORDS` (`COCRDLIC.cbl:1123-1262,1382-1410`; COCRDLIC-010, COCRDLIC-017, COCRDLIC-018, COCRDLIC-019, COCRDLIC-023) | [`page_after`](CardRepository::page_after) |
//! | COCRDLIC `9100-READ-BACKWARDS`: `STARTBR GTEQ` at the page's first key, then `READPREV` (`COCRDLIC.cbl:1264-1375`; COCRDLIC-027, COCRDLIC-029) | [`page_before`](CardRepository::page_before) |
//! | COCRDSLC and COCRDUPC `9100-GETCARD-BYACCTCARD`: a `READ` of `CARDDAT` by the 16-byte card number alone (`COCRDSLC.cbl:736-775`, COCRDSLC-014, COCRDSLC-015; `COCRDUPC.cbl:1376-1415`, COCRDUPC-016, COCRDUPC-018) | [`find_by_id`](CardRepository::find_by_id) |
//! | The same read for the card a list row's handle names, in place of the number COCRDLIC passes (COCRDSLC-006, COCRDUPC-002, COCRDUPC-043) | [`find_by_ref`](CardRepository::find_by_ref) |
//! | COCRDUPC `9200-WRITE-PROCESSING`: `READ … UPDATE`, `9300-CHECK-CHANGE-IN-REC`, then `REWRITE` (`COCRDUPC.cbl:1420-1494`; COCRDUPC-036, COCRDUPC-038, COCRDUPC-040) | [`lock_by_id`](CardRepository::lock_by_id), then [`update`](CardRepository::update) |
//! | CARDFILE `DELETE`, `DEFINE CLUSTER` and `REPRO` (`CARDFILE.jcl:33-76`; LOADS-002, LOADS-014) | [`replace_all`](CardRepository::replace_all), over the statement of [`insert`](CardRepository::insert) |
//!
//! COCRDSLC `9150-GETCARD-BYACCT`, the only read of the account alternate index, is never
//! performed (`COCRDSLC.cbl:779-810`, COCRDSLC-030), so no method reads cards by account alone.
//! [`find_all`](CardRepository::find_all) reads the whole table for batch jobs and tests; it is
//! unbounded and serves no endpoint (Bounded Pagination binds the two browses).
//!
//! # Keys and binds
//!
//! - A card number reaches SQL only as a bind value that codec B builds from
//!   `CardNumber::expose()` (PIC-024). A service resolves a page key, which is a handle, with
//!   [`find_by_ref`](CardRepository::find_by_ref) and passes the [`CardNumber`] of the record it
//!   returns, so no caller handles the raw number.
//! - Every card-number parameter is cast to `bpchar`. `sqlx` declares a `String` parameter as
//!   `text`, and comparing the `CHAR(16)` column with `text` would compare by `text` rules and
//!   leave `cards_pkey` unused. Comparison keeps the column's `"C"` collation, the byte order of
//!   the KSDS, and no statement names a collation of its own, which would defeat the index.
//! - The two browses run the SQL of `queries/cards_page_forward.sql` and
//!   `queries/cards_page_backward.sql`, the texts the `explain` test plans.
//!
//! # Transactions
//!
//! On [`PgStore`] every method but [`replace_all`](CardRepository::replace_all) runs one
//! statement in autocommit, so the row lock of [`lock_by_id`](CardRepository::lock_by_id) ends with
//! the call; only on a [`PgTx`] does it hold until commit or rollback, as `READ … UPDATE` holds
//! the record until the `REWRITE`. On a [`PgTx`] a failed statement aborts the whole transaction,
//! so a caller that goes on after a refused insert or update runs it inside `PgTx::savepoint`.
//!
//! # Redaction
//!
//! No error, log line or panic message here formats a record or a key: a failure is a
//! [`PersistenceError`], which holds an SQLSTATE, a constraint name or a column name, never a
//! value (Cardholder Data Redaction).

use copybook::{PicU, PicX};
use domain::online::cocrdlic::CardRef;
use domain::records::card::CardRecord;
use domain::sensitive::CardNumber;
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::{CardBinds, CardRow, pic_u_to_i64, pic_x_to_string};
use crate::{PgStore, PgTx};

/// Column of `CARD-NUM`, which an error names when a card-number bind cannot be built
/// (CVACT02Y-001).
const CARD_NUM: &str = "card_num";

/// Column of `CARD-ACCT-ID`, which an error names when the account filter cannot be bound
/// (CVACT02Y-002).
const ACCT_ID: &str = "acct_id";

/// One card as every read returns it: the `card_ref` handle (CVACT02Y-008) and the record its
/// seven copybook columns hold (CVACT02Y-001 to CVACT02Y-007).
pub type CardEntry = (CardRef, CardRecord);

/// The card master `CARDDAT`, keyed by card number, as table `cards`.
///
/// Implemented by [`PgStore`] (each statement in autocommit; [`replace_all`] in a transaction
/// of its own) and by [`PgTx`] (every statement in the caller's transaction). Test doubles in
/// other crates implement it too.
///
/// [`replace_all`]: CardRepository::replace_all
pub trait CardRepository: Send {
    /// The card whose number is `card`: COCRDSLC and COCRDUPC `9100-GETCARD-BYACCTCARD`, a `READ`
    /// by the card number alone, the account input playing no part (`COCRDSLC.cbl:736-775`,
    /// COCRDSLC-014; `COCRDUPC.cbl:1376-1415`, COCRDUPC-016). `None` is the `NOTFND` of
    /// COCRDSLC-015 and COCRDUPC-018.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// [`PersistenceError::Database`] for any other failed statement (the file errors of
    /// COCRDSLC-016 and COCRDUPC-019); [`PersistenceError::Decode`] when `card` holds a NUL or
    /// non-ASCII byte, or the stored row does not fit `CVACT02Y`.
    fn find_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send;

    /// The card whose number is `card`, its row locked against every other lock and write until
    /// the transaction ends: the `READ … UPDATE` of COCRDUPC `9200-WRITE-PROCESSING`
    /// (`COCRDUPC.cbl:1427-1449`), after which `9300-CHECK-CHANGE-IN-REC` compares the record
    /// returned with the fetched snapshot and [`update`](Self::update) rewrites it
    /// (COCRDUPC-036 to COCRDUPC-038). `None` is the card no longer found, which COCRDUPC reports
    /// as `Could not lock record for update` (COCRDUPC-036).
    ///
    /// The lock holds only on a [`PgTx`]. On [`PgStore`] the statement runs in autocommit, so
    /// its lock ends with the call.
    ///
    /// # Errors
    ///
    /// As [`find_by_id`](Self::find_by_id); a lock wait the server ends, such as a deadlock
    /// (`40P01`) or a lock timeout (`55P03`), is a [`PersistenceError::Database`] whose SQLSTATE
    /// says so, which COCRDUPC also reports as `Could not lock record for update`.
    fn lock_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send;

    /// The card whose `card_ref` is `handle`: the read that endpoints 8 to 10 make for a card
    /// list row, in place of the read by the number COCRDLIC passes (COCRDSLC-006,
    /// COCRDUPC-002, COCRDUPC-043). `None` is a handle that names no card, as one issued before a
    /// reload does (LOADS-027), which the endpoints treat as their `NOTFND`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// [`PersistenceError::Database`] for any other failed statement;
    /// [`PersistenceError::Decode`] when the stored row does not fit `CVACT02Y`.
    fn find_by_ref(
        &mut self,
        handle: CardRef,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send;

    /// Up to `limit` cards from `start` upward, inclusive, in ascending card-number order: COCRDLIC
    /// `9000-READ-FORWARD`, `STARTBR GTEQ` then `READNEXT` (`COCRDLIC.cbl:1123-1262`;
    /// COCRDLIC-017, COCRDLIC-018, COCRDLIC-023), with `9500-FILTER-RECORDS` applied in SQL
    /// (`COCRDLIC.cbl:1382-1410`, COCRDLIC-010).
    ///
    /// - `start`: the browse key. `None`, the fresh start (COCRDLIC-017), binds the 16 spaces the
    ///   program browses from; `bpchar` comparison ignores trailing spaces, so that key ranks below
    ///   every card number and the browse starts at the lowest one.
    /// - `acct`, `card`: the account and card filters, each `None` when off. A card passes only if
    ///   it matches every filter given. The look-ahead read of COCRDLIC-019 ignores the filters, so
    ///   the service issues it with both `None`.
    /// - `limit`: the row bound, bound verbatim. The service passes `page_size + 1`, the page and
    ///   its look-ahead row, from a `page_size` the API has validated to 1–100 (Bounded
    ///   Pagination), and decides from the rows which page, keys and messages result.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// [`PersistenceError::Database`] for any other failed statement, a negative `limit`
    /// included (`2201W`); [`PersistenceError::Decode`] when a key holds a NUL or non-ASCII byte,
    /// or a stored row does not fit `CVACT02Y`.
    fn page_after(
        &mut self,
        start: Option<&CardNumber>,
        acct: Option<PicU<11>>,
        card: Option<&CardNumber>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send;

    /// Up to `limit` cards from `start` downward, inclusive, in descending card-number order:
    /// COCRDLIC `9100-READ-BACKWARDS`, `STARTBR GTEQ` at the displayed page's first key then
    /// `READPREV`, whose first record is that card itself (`COCRDLIC.cbl:1264-1375`;
    /// COCRDLIC-027, COCRDLIC-029), with the filters of [`page_after`](Self::page_after) applied
    /// in SQL (COCRDLIC-010).
    ///
    /// The rows come back in query order, highest card number first. Dropping the `start` card,
    /// which the program discards unfiltered, and reversing the rows for display are the
    /// service's, by the rules of `domain::online::cocrdlic`. `start` `None` binds 16 spaces, at
    /// or below which no card number ranks, so it returns no row. `acct`, `card` and `limit` are
    /// as for [`page_after`](Self::page_after).
    ///
    /// # Errors
    ///
    /// As [`page_after`](Self::page_after).
    fn page_before(
        &mut self,
        start: Option<&CardNumber>,
        acct: Option<PicU<11>>,
        card: Option<&CardNumber>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send;

    /// Every card, in ascending card-number order. Unbounded: for batch jobs and tests, never for
    /// an endpoint (Bounded Pagination).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the database cannot be reached;
    /// [`PersistenceError::Database`] for any other failed statement;
    /// [`PersistenceError::Decode`] when a stored row does not fit `CVACT02Y`.
    fn find_all(&mut self)
    -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send;

    /// Writes `record` as a new card. The database assigns its `card_ref`, which no statement
    /// binds (CVACT02Y-008).
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] with constraint `cards_pkey` when a card already has the
    /// number; [`PersistenceError::Check`] when a range check refuses a value;
    /// [`PersistenceError::Decode`] when a field holds a NUL or non-ASCII byte, which no
    /// `CHAR(n)` column stores byte for byte (PIC-002); [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] as for every statement.
    fn insert(
        &mut self,
        record: &CardRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Rewrites the card whose number is `record.num` with every other field of `record`, its
    /// `FILLER` included: the `REWRITE` of COCRDUPC `9200-WRITE-PROCESSING`
    /// (`COCRDUPC.cbl:1477-1492`, COCRDUPC-038). `card_ref` is left as it is, so the handle the
    /// screen holds still names the card. `false` is no card with that number, no row changed.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Check`] when a range check refuses a value;
    /// [`PersistenceError::Decode`] when a field holds a NUL or non-ASCII byte;
    /// [`PersistenceError::Unavailable`] or [`PersistenceError::Database`] as for every
    /// statement. Each is the `Update of record failed` of COCRDUPC-040.
    fn update(
        &mut self,
        record: &CardRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Replaces the table's contents with `records`, the counterpart of CARDFILE's `DELETE`,
    /// `DEFINE CLUSTER` and `REPRO` (`CARDFILE.jcl:33-76`; LOADS-002, LOADS-014): every row is
    /// deleted, then every record is inserted, in slice order, and the count inserted is
    /// returned. An empty table is no error (LOADS-015).
    ///
    /// Every card gets a new `card_ref`, none equal to a value the table held before, so a handle
    /// issued before a reload names no card after it; only `card_ref` differs between two loads
    /// of the same records (LOADS-026, LOADS-027).
    ///
    /// On [`PgStore`] the delete and the inserts run in one transaction of their own, which
    /// commits only when every insert succeeded. On a [`PgTx`] they run in the caller's.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a record cannot be stored, before anything is deleted;
    /// [`PersistenceError::Duplicate`] when two records share a card number;
    /// [`PersistenceError::Check`], [`PersistenceError::Unavailable`] or
    /// [`PersistenceError::Database`] as for every statement. On [`PgStore`] any of them leaves
    /// the previous contents (LOADS-018).
    fn replace_all(
        &mut self,
        records: &[CardRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

impl CardRepository for PgStore {
    fn find_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send {
        select_by_num(self.pool(), card)
    }

    fn lock_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send {
        lock_by_num(self.pool(), card)
    }

    fn find_by_ref(
        &mut self,
        handle: CardRef,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send {
        select_by_ref(self.pool(), handle)
    }

    fn page_after(
        &mut self,
        start: Option<&CardNumber>,
        acct: Option<PicU<11>>,
        card: Option<&CardNumber>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send {
        select_after(self.pool(), start, acct, card, limit)
    }

    fn page_before(
        &mut self,
        start: Option<&CardNumber>,
        acct: Option<PicU<11>>,
        card: Option<&CardNumber>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send {
        select_before(self.pool(), start, acct, card, limit)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &CardRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_record(self.pool(), record)
    }

    fn update(
        &mut self,
        record: &CardRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_record(self.pool(), record)
    }

    fn replace_all(
        &mut self,
        records: &[CardRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // An early return drops the transaction, which rolls it back: the previous contents
            // stay (LOADS-018).
            let mut tx = begin.await?;
            let inserted = replace_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl CardRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send {
        select_by_num(self.conn(), card)
    }

    fn lock_by_id(
        &mut self,
        card: &CardNumber,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send {
        lock_by_num(self.conn(), card)
    }

    fn find_by_ref(
        &mut self,
        handle: CardRef,
    ) -> impl Future<Output = Result<Option<CardEntry>, PersistenceError>> + Send {
        select_by_ref(self.conn(), handle)
    }

    fn page_after(
        &mut self,
        start: Option<&CardNumber>,
        acct: Option<PicU<11>>,
        card: Option<&CardNumber>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send {
        select_after(self.conn(), start, acct, card, limit)
    }

    fn page_before(
        &mut self,
        start: Option<&CardNumber>,
        acct: Option<PicU<11>>,
        card: Option<&CardNumber>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send {
        select_before(self.conn(), start, acct, card, limit)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<CardEntry>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &CardRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_record(self.conn(), record)
    }

    fn update(
        &mut self,
        record: &CardRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_record(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[CardRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_in(self.conn(), records)
    }
}

/// The bind values of one browse: the start key, then the two filters, `None` when off.
struct PageBinds {
    /// `$1`, the browse key as its 16 bytes.
    start: String,
    /// `$2`, the account filter.
    acct: Option<i64>,
    /// `$3`, the card filter as its 16 bytes.
    card: Option<String>,
}

impl PageBinds {
    /// The binds of a browse from `start`, or from 16 spaces when `None` (COCRDLIC-017), under
    /// the filters `acct` and `card`.
    fn new(
        start: Option<&CardNumber>,
        acct: Option<PicU<11>>,
        card: Option<&CardNumber>,
    ) -> Result<Self, PersistenceError> {
        let spaces = CardNumber::new(PicX::spaces());
        Ok(Self {
            start: card_text(start.unwrap_or(&spaces))?,
            acct: acct.map(|id| pic_u_to_i64(ACCT_ID, id)).transpose()?,
            card: card.map(card_text).transpose()?,
        })
    }
}

/// The text a card-number parameter binds: the number's 16 bytes, verbatim (PIC-001, PIC-024).
fn card_text(card: &CardNumber) -> Result<String, PersistenceError> {
    pic_x_to_string(CARD_NUM, card.expose())
}

/// Codec B's records of `rows`, in their order.
fn into_entries(rows: Vec<CardRow>) -> Result<Vec<CardEntry>, PersistenceError> {
    rows.into_iter().map(CardRow::into_record).collect()
}

/// The `READ` of `9100-GETCARD-BYACCTCARD` (`COCRDSLC.cbl:742-750`, `COCRDUPC.cbl:1382-1390`).
async fn select_by_num<'e, E>(
    executor: E,
    card: &CardNumber,
) -> Result<Option<CardEntry>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let key = card_text(card)?;
    let row = sqlx::query_as!(
        CardRow,
        r#"SELECT card_num, acct_id, card_cvv_cd, card_embossed_name, card_expiraion_date,
               card_active_status, filler, card_ref
           FROM cards
           WHERE card_num = $1::bpchar"#,
        key
    )
    .fetch_optional(executor)
    .await?;
    row.map(CardRow::into_record).transpose()
}

/// The `READ … UPDATE` of `9200-WRITE-PROCESSING` (`COCRDUPC.cbl:1427-1436`).
async fn lock_by_num<'e, E>(
    executor: E,
    card: &CardNumber,
) -> Result<Option<CardEntry>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let key = card_text(card)?;
    let row = sqlx::query_as!(
        CardRow,
        r#"SELECT card_num, acct_id, card_cvv_cd, card_embossed_name, card_expiraion_date,
               card_active_status, filler, card_ref
           FROM cards
           WHERE card_num = $1::bpchar
           FOR UPDATE"#,
        key
    )
    .fetch_optional(executor)
    .await?;
    row.map(CardRow::into_record).transpose()
}

/// The card a list row's handle names (COCRDSLC-006), through `cards_card_ref_key`.
async fn select_by_ref<'e, E>(
    executor: E,
    handle: CardRef,
) -> Result<Option<CardEntry>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let row = sqlx::query_as!(
        CardRow,
        r#"SELECT card_num, acct_id, card_cvv_cd, card_embossed_name, card_expiraion_date,
               card_active_status, filler, card_ref
           FROM cards
           WHERE card_ref = $1::bigint"#,
        handle.get()
    )
    .fetch_optional(executor)
    .await?;
    row.map(CardRow::into_record).transpose()
}

/// `9000-READ-FORWARD` (`queries/cards_page_forward.sql`).
async fn select_after<'e, E>(
    executor: E,
    start: Option<&CardNumber>,
    acct: Option<PicU<11>>,
    card: Option<&CardNumber>,
    limit: i64,
) -> Result<Vec<CardEntry>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let binds = PageBinds::new(start, acct, card)?;
    let rows = sqlx::query_file_as!(
        CardRow,
        "queries/cards_page_forward.sql",
        binds.start,
        binds.acct,
        binds.card,
        limit
    )
    .fetch_all(executor)
    .await?;
    into_entries(rows)
}

/// `9100-READ-BACKWARDS` (`queries/cards_page_backward.sql`).
async fn select_before<'e, E>(
    executor: E,
    start: Option<&CardNumber>,
    acct: Option<PicU<11>>,
    card: Option<&CardNumber>,
    limit: i64,
) -> Result<Vec<CardEntry>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let binds = PageBinds::new(start, acct, card)?;
    let rows = sqlx::query_file_as!(
        CardRow,
        "queries/cards_page_backward.sql",
        binds.start,
        binds.acct,
        binds.card,
        limit
    )
    .fetch_all(executor)
    .await?;
    into_entries(rows)
}

/// Every card, in key order.
async fn select_all<'e, E>(executor: E) -> Result<Vec<CardEntry>, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let rows = sqlx::query_as!(
        CardRow,
        r#"SELECT card_num, acct_id, card_cvv_cd, card_embossed_name, card_expiraion_date,
               card_active_status, filler, card_ref
           FROM cards
           ORDER BY card_num"#
    )
    .fetch_all(executor)
    .await?;
    into_entries(rows)
}

/// Inserts `record` through codec B.
async fn insert_record<'e, E>(executor: E, record: &CardRecord) -> Result<(), PersistenceError>
where
    E: PgExecutor<'e>,
{
    let binds = CardBinds::from_record(record)?;
    insert_binds(executor, &binds).await.map(drop)
}

/// The one insert statement, which [`insert_record`] and [`replace_in`] share; `card_ref` is
/// `GENERATED ALWAYS` and never bound. Returns the rows inserted.
async fn insert_binds<'e, E>(executor: E, binds: &CardBinds) -> Result<u64, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let done = sqlx::query!(
        r#"INSERT INTO cards (card_num, acct_id, card_cvv_cd, card_embossed_name,
               card_expiraion_date, card_active_status, filler)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
        binds.card_num,
        binds.acct_id,
        binds.card_cvv_cd,
        binds.card_embossed_name,
        binds.card_expiraion_date,
        binds.card_active_status,
        binds.filler
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// The `REWRITE` of `9200-WRITE-PROCESSING` (`COCRDUPC.cbl:1477-1483`): every column but the
/// key and `card_ref`.
async fn update_record<'e, E>(executor: E, record: &CardRecord) -> Result<bool, PersistenceError>
where
    E: PgExecutor<'e>,
{
    let binds = CardBinds::from_record(record)?;
    let done = sqlx::query!(
        r#"UPDATE cards
           SET acct_id = $2, card_cvv_cd = $3, card_embossed_name = $4,
               card_expiraion_date = $5, card_active_status = $6, filler = $7
           WHERE card_num = $1::bpchar"#,
        binds.card_num,
        binds.acct_id,
        binds.card_cvv_cd,
        binds.card_embossed_name,
        binds.card_expiraion_date,
        binds.card_active_status,
        binds.filler
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// Deletes then reloads every row on `conn`, inside the transaction the caller holds.
///
/// Every record is converted before the delete, so a record codec B cannot store fails the load
/// with nothing changed. The delete keeps the identity sequence where it stands, so each insert
/// takes a `card_ref` the table never held before (LOADS-027).
// A `TRUNCATE … RESTART IDENTITY` would give the same records the same handles on every reload;
// the delete is kept because LOADS-027 requires a handle from before a reload to name no card
// after it, and a recycled one could name a different card.
async fn replace_in(
    conn: &mut PgConnection,
    records: &[CardRecord],
) -> Result<u64, PersistenceError> {
    let binds = records
        .iter()
        .map(CardBinds::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    sqlx::query!("DELETE FROM cards")
        .execute(&mut *conn)
        .await?;
    let mut inserted = 0;
    for row in &binds {
        inserted += insert_binds(&mut *conn, row).await?;
    }
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::slice;
    use std::time::Duration;

    use copybook::{FixedRecord, PicX};
    use domain::online::cocrdlic::CardRef;
    use domain::records::card::CardRecord;
    use domain::sensitive::CardNumber;

    use super::{CardEntry, CardRepository};
    use crate::{PersistenceError, PgStore};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// Card numbers in ascending order. None is a fixture card number: no test here quotes
    /// `carddata.txt`.
    const NUMBERS: [&str; 5] = [
        "9100000000000011",
        "9100000000000022",
        "9100000000000033",
        "9100000000000044",
        "9100000000000055",
    ];
    /// The accounts of [`NUMBERS`], in the same order: two cards on 901, two on 902, one on 903.
    const ACCOUNTS: [u64; 5] = [901, 902, 901, 903, 902];
    /// A card number no test stores.
    const UNKNOWN: &[u8; 16] = b"9100000000000099";

    /// The record of a 150-byte `carddata.txt`-shaped line (CVACT02Y-001 to CVACT02Y-007).
    fn card(
        num: &str,
        acct: u64,
        name: &str,
        status: &str,
    ) -> Result<CardRecord, Box<dyn StdError>> {
        let line = format!(
            "{num}{acct:011}123{name:<50}2030-12-31{status:<1}{:<59}",
            ""
        );
        Ok(CardRecord::decode(line.as_bytes())?)
    }

    /// The five cards of [`NUMBERS`] and [`ACCOUNTS`], all active.
    fn sample() -> Result<[CardRecord; 5], Box<dyn StdError>> {
        let mut cards = Vec::with_capacity(NUMBERS.len());
        for (num, acct) in NUMBERS.into_iter().zip(ACCOUNTS) {
            cards.push(card(num, acct, "SAMPLE HOLDER", "Y")?);
        }
        cards.try_into().map_err(|_| "five sample cards".into())
    }

    /// The records of `entries`, without their handles.
    fn records(entries: &[CardEntry]) -> Vec<CardRecord> {
        entries.iter().map(|(_, record)| record.clone()).collect()
    }

    /// `entries` as an array of N, or an error naming the count expected.
    fn exactly<const N: usize>(
        entries: Vec<CardEntry>,
    ) -> Result<[CardEntry; N], Box<dyn StdError>> {
        entries.try_into().map_err(|rows: Vec<CardEntry>| {
            format!("expected {N} cards, read {}", rows.len()).into()
        })
    }

    /// Clones of `rows`, as a read is expected to return them.
    fn want(rows: &[&CardEntry]) -> Vec<CardEntry> {
        rows.iter().map(|&entry| entry.clone()).collect()
    }

    /// The reads by number and by handle find a card or report `None`.
    async fn check_lookups(
        repo: &mut impl CardRepository,
        cards: &[CardRecord; 5],
        entries: &[CardEntry; 5],
    ) -> TestResult {
        let [_, c2, _, _, c5] = cards;
        let [_, e2, _, _, e5] = entries;
        assert_eq!(repo.find_by_id(&c2.num).await?, Some(e2.clone()));
        assert_eq!(repo.find_by_id(&c5.num).await?, Some(e5.clone()));
        let unknown = CardNumber::new(PicX::from_array(*UNKNOWN));
        assert_eq!(repo.find_by_id(&unknown).await?, None);
        assert_eq!(repo.find_by_ref(e5.0).await?, Some(e5.clone()));
        let highest = entries.iter().map(|(handle, _)| handle.get()).max();
        let unused = CardRef::new(highest.unwrap_or(0) + 1);
        assert_eq!(repo.find_by_ref(unused).await?, None);
        Ok(())
    }

    /// The forward browse: from the lowest key or a key inclusive, ascending, bounded, filtered.
    async fn check_forward(
        repo: &mut impl CardRepository,
        cards: &[CardRecord; 5],
        entries: &[CardEntry; 5],
    ) -> TestResult {
        let [c1, c2, c3, c4, _] = cards;
        let [e1, e2, e3, e4, e5] = entries;
        let page = repo.page_after(None, None, None, 3).await?;
        assert_eq!(page, want(&[e1, e2, e3]));
        let page = repo.page_after(Some(&c3.num), None, None, 100).await?;
        assert_eq!(page, want(&[e3, e4, e5]));
        let page = repo.page_after(None, Some(c1.acct_id), None, 100).await?;
        assert_eq!(page, want(&[e1, e3]));
        let page = repo.page_after(Some(&c2.num), Some(c1.acct_id), None, 100);
        assert_eq!(page.await?, want(&[e3]));
        let page = repo.page_after(None, None, Some(&c4.num), 100).await?;
        assert_eq!(page, want(&[e4]));
        let page = repo.page_after(None, Some(c2.acct_id), Some(&c4.num), 100);
        assert_eq!(page.await?, Vec::new());
        let unknown = CardNumber::new(PicX::from_array(*UNKNOWN));
        let page = repo.page_after(Some(&unknown), None, None, 100).await?;
        assert_eq!(page, Vec::new());
        Ok(())
    }

    /// The backward browse: from a key inclusive, descending, bounded, filtered; no card ranks at
    /// or below the spaces of a missing start key.
    async fn check_backward(
        repo: &mut impl CardRepository,
        cards: &[CardRecord; 5],
        entries: &[CardEntry; 5],
    ) -> TestResult {
        let [c1, c2, _, c4, c5] = cards;
        let [e1, e2, e3, e4, e5] = entries;
        let page = repo.page_before(Some(&c4.num), None, None, 100).await?;
        assert_eq!(page, want(&[e4, e3, e2, e1]));
        let page = repo.page_before(Some(&c4.num), None, None, 2).await?;
        assert_eq!(page, want(&[e4, e3]));
        let page = repo.page_before(Some(&c5.num), Some(c2.acct_id), None, 100);
        assert_eq!(page.await?, want(&[e5, e2]));
        let page = repo.page_before(Some(&c5.num), None, Some(&c1.num), 100);
        assert_eq!(page.await?, want(&[e1]));
        assert_eq!(repo.page_before(None, None, None, 100).await?, Vec::new());
        let unknown = CardNumber::new(PicX::from_array(*UNKNOWN));
        let page = repo.page_before(Some(&unknown), None, None, 1).await?;
        assert_eq!(page, want(&[e5]));
        Ok(())
    }

    // audit: COCRDLIC-010
    // audit: COCRDLIC-017
    // audit: COCRDLIC-027
    // audit: COCRDSLC-006
    // audit: COCRDSLC-014
    // audit: COCRDSLC-015
    /// Both browses are inclusive at their start key, ordered by card number, bounded by `limit`
    /// and filtered in SQL, and the reads by number and by handle find a card or report `None`,
    /// on the pool and inside a transaction alike; a duplicate number is refused by `cards_pkey`
    /// and a negative bound by the server.
    #[tokio::test]
    async fn browses_and_lookups_follow_the_key_order_filters_and_handles() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let cards = sample()?;
        for record in &cards {
            store.insert(record).await?;
        }
        let [_, _, c3, _, _] = &cards;
        assert_eq!(
            store.insert(c3).await,
            Err(PersistenceError::Duplicate {
                constraint: Some("cards_pkey".to_owned())
            })
        );

        let all = store.find_all().await?;
        assert_eq!(records(&all), cards.to_vec());
        let entries = exactly::<5>(all)?;
        let handles = entries.clone().map(|(handle, _)| handle);
        assert!(
            handles
                .iter()
                .enumerate()
                .all(|(i, h)| !handles.iter().skip(i + 1).any(|o| o == h))
        );

        check_lookups(&mut store, &cards, &entries).await?;
        check_forward(&mut store, &cards, &entries).await?;
        check_backward(&mut store, &cards, &entries).await?;
        assert!(matches!(
            store.page_after(None, None, None, -1).await,
            Err(PersistenceError::Database { .. })
        ));

        let mut tx = store.begin().await?;
        check_lookups(&mut tx, &cards, &entries).await?;
        check_forward(&mut tx, &cards, &entries).await?;
        check_backward(&mut tx, &cards, &entries).await?;
        assert_eq!(tx.find_all().await?, entries.to_vec());
        tx.commit().await?;
        Ok(())
    }

    // audit: COCRDUPC-036
    // audit: COCRDUPC-038
    /// A reload replaces the rows in one transaction and gives every card a new handle; an update
    /// rewrites every column but the key and the handle; a row lock taken on a transaction holds
    /// off a second one until the first commits.
    #[tokio::test]
    async fn reloads_updates_and_row_locks_keep_their_contracts() -> TestResult {
        let guard = test_support::postgres::start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let [c1, c2, c3, _, c5] = sample()?;
        let [n1, n2, ..] = NUMBERS;
        let load = [c1.clone(), c2.clone(), c3.clone()];

        // Two loads of the same records: same contents, all-new handles (LOADS-026, LOADS-027).
        assert_eq!(store.replace_all(&load).await?, 3);
        let first = store.find_all().await?;
        assert_eq!(store.replace_all(&load).await?, 3);
        let second = store.find_all().await?;
        assert_eq!(records(&first), load.to_vec());
        assert_eq!(records(&second), load.to_vec());
        assert!(
            second
                .iter()
                .all(|(new, _)| first.iter().all(|(old, _)| old != new))
        );
        for (old, _) in &first {
            assert_eq!(store.find_by_ref(*old).await?, None);
        }

        // A failing load leaves the previous contents (LOADS-018).
        assert!(matches!(
            store.replace_all(&[c5.clone(), c5.clone()]).await,
            Err(PersistenceError::Duplicate { .. })
        ));
        assert_eq!(store.find_all().await?, second);

        // A load inside the caller's transaction, rolled back, leaves them too; an empty load
        // empties the table.
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(slice::from_ref(&c5)).await?, 1);
        assert_eq!(records(&tx.find_all().await?), vec![c5.clone()]);
        assert_eq!(tx.replace_all(&[]).await?, 0);
        assert_eq!(tx.find_all().await?, Vec::new());
        tx.rollback().await?;
        assert_eq!(store.find_all().await?, second);

        // The rewrite keeps the key and the handle and changes every other column.
        let [s1, s2, _] = exactly::<3>(second)?;
        let renamed = card(n2, 777, "RENAMED HOLDER", "N")?;
        assert!(store.update(&renamed).await?);
        assert_eq!(
            store.find_by_id(&c2.num).await?,
            Some((s2.0, renamed.clone()))
        );
        assert!(!store.update(&c5).await?);
        assert_eq!(store.find_by_id(&c5.num).await?, None);

        // `READ … UPDATE`: the second lock waits for the first transaction, then reads its write.
        let mut holder = store.begin().await?;
        assert_eq!(holder.lock_by_id(&c1.num).await?, Some(s1.clone()));
        let waiter_store = store.clone();
        let key = c1.num.clone();
        let waiter = tokio::spawn(async move {
            let mut tx = waiter_store.begin().await?;
            let locked = tx.lock_by_id(&key).await?;
            tx.rollback().await?;
            Ok::<_, PersistenceError>(locked)
        });
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(!waiter.is_finished());
        let blocked = card(n1, 901, "SAMPLE HOLDER", "N")?;
        assert!(holder.update(&blocked).await?);
        holder.commit().await?;
        assert_eq!(waiter.await??, Some((s1.0, blocked)));

        // Without a transaction the lock ends with the call, so the next lock does not wait.
        assert!(store.lock_by_id(&c1.num).await?.is_some());
        assert!(store.lock_by_id(&c1.num).await?.is_some());
        Ok(())
    }
}
