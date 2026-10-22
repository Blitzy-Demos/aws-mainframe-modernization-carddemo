//! [`UserRepository`]: table `users` (migration 0003), the USRSEC KSDS of `CSUSR01Y`
//! `SEC-USER-DATA`, keyed by `SEC-USR-ID` as `KEYS(8,0)` (`DUSRSECJ.jcl:65`; CSUSR01Y-001 to
//! CSUSR01Y-006). Column `password_hash` holds the argon2id PHC string in place of the
//! plaintext `SEC-USR-PWD` (E25).
//!
//! # The COBOL accesses it replaces
//!
//! | Method | COBOL access | Outcome | Rules |
//! |---|---|---|---|
//! | `find_by_id` | COSGN00C `READ-USER-SEC-FILE` (`COSGN00C.cbl:209-257`); the shared fetch of endpoint 18, COUSR02C and COUSR03C `READ-USER-SEC-FILE` on ENTER (`COUSR02C.cbl:320-356`, `COUSR03C.cbl:267-303`) | RESP 0 → `Some`; RESP 13 (`NOTFND`) → `None`; any other RESP → `Err` | COSGN00C-013, COSGN00C-019, COSGN00C-020, COUSR02C-011, COUSR02C-015, COUSR02C-016 |
//! | `lock_by_id` | COUSR02C and COUSR03C `READ-USER-SEC-FILE` with `UPDATE` on PF5, ahead of the rewrite or delete | as `find_by_id`, the row locked until the transaction ends | COUSR02C-024, COUSR03C-017 |
//! | `page_after` | COUSR00C `STARTBR-USER-SEC-FILE` (`GTEQ`) and `READNEXT-USER-SEC-FILE` (`COUSR00C.cbl:586-648`) | the users at or above the key, ascending | COUSR00C-012, COUSR00C-013, COUSR00C-026, COUSR00C-028 |
//! | `page_before` | COUSR00C `STARTBR-USER-SEC-FILE` and `READPREV-USER-SEC-FILE` (`COUSR00C.cbl:586-614,653-682`) | the users at or below the key, descending | COUSR00C-022, COUSR00C-026, COUSR00C-028 |
//! | `insert` | COUSR01C `WRITE-USER-SEC-FILE` (`COUSR01C.cbl:238-270`) | `DUPKEY`/`DUPREC` → [`PersistenceError::Duplicate`] on `users_pkey` | COUSR01C-016, COUSR01C-023, COUSR01C-024 |
//! | `update` | COUSR02C `UPDATE-USER-SEC-FILE`, `REWRITE` (`COUSR02C.cbl:358-390`) | `true` when the row was rewritten, `false` for `NOTFND` | COUSR02C-029, COUSR02C-031, COUSR02C-032 |
//! | `delete` | COUSR03C `DELETE-USER-SEC-FILE` (`COUSR03C.cbl:305-335`) | `true` when the row was removed, `false` for `NOTFND` | COUSR03C-019, COUSR03C-021, COUSR03C-022 |
//! | `replace_all` | DUSRSECJ: IEBGENER of the in-stream records, IDCAMS `DELETE`, `DEFINE CLUSTER` and `REPRO` (`DUSRSECJ.jcl:34-45,62-87`) | the table holds exactly the given users, in one transaction | LOADS-009, LOADS-012, LOADS-014, LOADS-015, LOADS-018, LOADS-033, LOADS-034 |
//!
//! The shared fetch of endpoint 18 reads with `find_by_id`: its CICS `UPDATE` lock ended with
//! the task that sent the screen, so nothing is held between requests. PF5 reads with
//! `lock_by_id` inside the transaction that then rewrites or deletes the row, which is how the
//! `READ … UPDATE` of `UPDATE-USER-INFO` and `DELETE-USER-INFO` holds the record until the
//! `REWRITE` or `DELETE`.
//!
//! # Keys
//!
//! - Every key and value is bound exactly as given: nothing here upper-cases (sign-on's
//!   `FUNCTION UPPER-CASE` is the domain's, E26), trims or pads. A key is the 8 bytes of
//!   `SEC-USR-ID`, leading spaces and case included, so `admin001` does not find `ADMIN001`
//!   (COUSR02C-011).
//! - `usr_id` is `CHAR(8) COLLATE "C"`, so the pages run in key byte order, the KSDS order. The
//!   `CHAR(n)` comparison ignores trailing spaces, so a key of eight spaces, the domain's
//!   stand-in for `LOW-VALUES`, compares as the empty key, below every stored user ID
//!   (COUSR00C-012).
//! - Every statement casts its key parameter to `bpchar`, as the page queries do. `sqlx` binds
//!   a `String` as `text`, and `usr_id = $1` against `text` compares `usr_id::text`, whose
//!   trailing spaces are dropped, with the padded key: a user ID shorter than 8 bytes, such as
//!   `AB CD` (COUSR02C-030), would find no row, and `users_pkey` could not serve the lookup.
//! - Codec B stores no NUL or non-ASCII byte (PIC-002), so no row holds a key that has one. The
//!   point accesses answer such a key as the `NOTFND` of a key nothing was written under:
//!   `find_by_id` and `lock_by_id` return `None`, `update` and `delete` return `false`, and no
//!   statement runs. A page has no position for it in the stored key order, and `insert`
//!   cannot store it, so both report [`PersistenceError::Decode`] on `usr_id`.
//!
//! # Pages (Bounded Pagination)
//!
//! `page_after` and `page_before` run `queries/users_page_forward.sql` and
//! `queries/users_page_backward.sql`, the files the `explain` test plans on `users_pkey`. Both
//! select the four columns `POPULATE-USER-DATA` shows and no credential column, so no page reads
//! a hash (COUSR00C-028). The start key is included, and `limit` is bound verbatim: the service
//! passes `domain::online::cousr00c::PageRead::fetch_limit`, from the page size the API
//! validated (1 to 100). For ENTER that is the page size plus one, the look-ahead row that tells
//! whether a further page exists (COUSR00C-014); for F7 and F8 it is the page size plus two,
//! because their browse first reads and discards the start record (COUSR00C-017, COUSR00C-022)
//! before the page and its look-ahead (COUSR00C-014, COUSR00C-023).
//!
//! # Transactions
//!
//! - Through [`PgStore`] each method runs one autocommitted statement, except `replace_all`,
//!   which opens and commits a transaction of its own. A `lock_by_id` through [`PgStore`]
//!   therefore releases its lock as it returns; the lock is meaningful only on a [`PgTx`].
//! - Through [`PgTx`] every statement runs inside the caller's transaction, and a lock taken by
//!   `lock_by_id` holds until the caller commits or rolls back. A failed statement, a duplicate
//!   `insert` included, aborts that transaction: a caller that must go on runs the statement
//!   inside [`PgTx::savepoint`].
//!
//! # Redaction (Cardholder Data Redaction)
//!
//! No password exists here, only the hash of [`UserEntry::password_hash`] and
//! [`HashedUser::password_hash`]: `replace_all` stores the hash of each [`HashedUser`] and never
//! reads its `record.pwd` (LOADS-033). The hash is bound and read back, never formatted, logged
//! or returned by a page, and this module emits no tracing. Errors carry only an SQLSTATE, a
//! constraint name or a column name ([`PersistenceError`]).

use copybook::PicX;
use domain::batch::loads::HashedUser;
use domain::records::{UserEntry, UserListEntry};
use sqlx::{PgConnection, PgExecutor};

use crate::error::PersistenceError;
use crate::rows::{UserListRow, UserRow, pic_x_to_string};
use crate::{PgStore, PgTx};

/// The key column, `SEC-USR-ID PIC X(08)` (CSUSR01Y-001), as errors name it.
const KEY_COLUMN: &str = "usr_id";

/// The USRSEC file of `CSUSR01Y`, as table `users`: point reads, locked reads, key-ordered
/// pages, writes and the DUSRSECJ reload (module documentation).
///
/// Implemented by [`PgStore`] and [`PgTx`]; test doubles in other crates implement it too.
pub trait UserRepository: Send {
    /// The user stored under `id`: COSGN00C's sign-on read and the shared fetch of COUSR02C and
    /// COUSR03C. `None` is CICS `NOTFND`, RESP 13 (COSGN00C-019, COUSR02C-015), also for a key
    /// no stored row can hold.
    ///
    /// # Errors
    ///
    /// Any other read failure, COSGN00C's `WHEN OTHER` (COSGN00C-020, COUSR02C-016):
    /// [`PersistenceError::Unavailable`] when the database cannot be reached,
    /// [`PersistenceError::Database`] for a failed statement, and [`PersistenceError::Decode`]
    /// for a stored row that does not fit `CSUSR01Y`.
    fn find_by_id(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send;

    /// The user stored under `id`, read `FOR UPDATE`: CICS `READ … UPDATE` ahead of COUSR02C's
    /// `REWRITE` and COUSR03C's `DELETE` (COUSR02C-024, COUSR03C-017). On a [`PgTx`] the row
    /// stays locked until the transaction ends; on a [`PgStore`] the lock ends with the call.
    /// `None` as for [`find_by_id`](Self::find_by_id).
    ///
    /// # Errors
    ///
    /// As [`find_by_id`](Self::find_by_id); a lock wait the server ends, such as a deadlock
    /// (`40P01`) or a lock timeout (`55P03`), is [`PersistenceError::Database`] with that
    /// SQLSTATE.
    fn lock_by_id(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send;

    /// At most `limit` users whose ID is at or above `start`, ascending: COUSR00C's `STARTBR`
    /// (`GTEQ`) and `READNEXT` loop (COUSR00C-013). A blank `start` reads from the first user
    /// (COUSR00C-012). An empty page is the browse's `NOTFND` or `ENDFILE`.
    ///
    /// # Errors
    ///
    /// COUSR00C's `WHEN OTHER` (COUSR00C-026): [`PersistenceError::Decode`] on `usr_id` for a
    /// `start` holding a NUL or non-ASCII byte, or for a stored row that does not fit;
    /// [`PersistenceError::Unavailable`]; [`PersistenceError::Database`], a negative `limit`
    /// included.
    fn page_after(
        &mut self,
        start: &PicX<8>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send;

    /// At most `limit` users whose ID is at or below `start`, descending: COUSR00C's `STARTBR`
    /// and `READPREV` loop, whose first read returns the start record (COUSR00C-022).
    ///
    /// # Errors
    ///
    /// As [`page_after`](Self::page_after).
    fn page_before(
        &mut self,
        start: &PicX<8>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send;

    /// Stores `entry` as a new row, every column as given: COUSR01C's `WRITE` (COUSR01C-016).
    /// On a [`PgTx`] a failure aborts the transaction; a caller that must go on inserts inside
    /// [`PgTx::savepoint`].
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] on `users_pkey` when a row already holds the same 8
    /// `usr_id` bytes, CICS `DUPKEY`/`DUPREC` (COUSR01C-023). Any other failure is COUSR01C's
    /// `WHEN OTHER` (COUSR01C-024): [`PersistenceError::Decode`] for a field holding a NUL or
    /// non-ASCII byte, [`PersistenceError::Unavailable`], [`PersistenceError::Database`].
    fn insert(
        &mut self,
        entry: &UserEntry,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Rewrites the row of `entry.id` whole, `REWRITE` of the record read for update
    /// (COUSR02C-029): names, hash, type and `FILLER` as given. When no password was supplied
    /// the service passes the stored hash unchanged (E27). `false` when no row has the key,
    /// CICS `NOTFND` (COUSR02C-031).
    ///
    /// # Errors
    ///
    /// COUSR02C's `WHEN OTHER` (COUSR02C-032): [`PersistenceError::Decode`] for a non-key field
    /// holding a NUL or non-ASCII byte, [`PersistenceError::Unavailable`],
    /// [`PersistenceError::Database`].
    fn update(
        &mut self,
        entry: &UserEntry,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Removes the row of `id` (COUSR03C-019). `false` when no row has the key, CICS `NOTFND`
    /// (COUSR03C-021).
    ///
    /// # Errors
    ///
    /// COUSR03C's `WHEN OTHER` (COUSR03C-022): [`PersistenceError::Unavailable`],
    /// [`PersistenceError::Database`].
    fn delete(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Replaces the table's contents with `users`, the `dusrsecj` load (LOADS-009, LOADS-014):
    /// every row is deleted, then each user is inserted in slice order with its hash, never its
    /// password (LOADS-033). Returns the number of rows inserted. An empty table before the call
    /// is no error (LOADS-015). On a [`PgStore`] the replacement commits in a transaction of its
    /// own; on a [`PgTx`] it joins the caller's.
    ///
    /// Every user is converted through codec B before any statement runs, so a value the table
    /// cannot store leaves the contents untouched.
    ///
    /// The delete is preceded by a `SHARE ROW EXCLUSIVE` lock on the table, held until the
    /// transaction ends, so a concurrent replacement or row write waits for it and then sees its
    /// committed rows: two loads at once leave the input of the one that commits last, never
    /// their union (LOADS-040). Reads do not wait.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] for a field holding a NUL or non-ASCII byte, before any
    /// statement; [`PersistenceError::Duplicate`] when two users share a key;
    /// [`PersistenceError::Unavailable`]; [`PersistenceError::Database`]. On a [`PgStore`] a
    /// failed statement rolls the replacement back and the previous contents remain
    /// (LOADS-018).
    fn replace_all(
        &mut self,
        users: &[HashedUser],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

impl UserRepository for PgStore {
    fn find_by_id(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
        find_in(self.pool(), id)
    }

    fn lock_by_id(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
        lock_in(self.pool(), id)
    }

    fn page_after(
        &mut self,
        start: &PicX<8>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
        page_after_in(self.pool(), start, limit)
    }

    fn page_before(
        &mut self,
        start: &PicX<8>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
        page_before_in(self.pool(), start, limit)
    }

    fn insert(
        &mut self,
        entry: &UserEntry,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_in(self.pool(), entry)
    }

    fn update(
        &mut self,
        entry: &UserEntry,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_in(self.pool(), entry)
    }

    fn delete(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        delete_in(self.pool(), id)
    }

    fn replace_all(
        &mut self,
        users: &[HashedUser],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            let rows = rows_of(users)?;
            let mut tx = begin.await?;
            let inserted = replace_rows(tx.conn(), &rows).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl UserRepository for PgTx<'_> {
    fn find_by_id(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
        find_in(self.conn(), id)
    }

    fn lock_by_id(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<Option<UserEntry>, PersistenceError>> + Send {
        lock_in(self.conn(), id)
    }

    fn page_after(
        &mut self,
        start: &PicX<8>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
        page_after_in(self.conn(), start, limit)
    }

    fn page_before(
        &mut self,
        start: &PicX<8>,
        limit: i64,
    ) -> impl Future<Output = Result<Vec<UserListEntry>, PersistenceError>> + Send {
        page_before_in(self.conn(), start, limit)
    }

    fn insert(
        &mut self,
        entry: &UserEntry,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        insert_in(self.conn(), entry)
    }

    fn update(
        &mut self,
        entry: &UserEntry,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_in(self.conn(), entry)
    }

    fn delete(
        &mut self,
        id: &PicX<8>,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        delete_in(self.conn(), id)
    }

    fn replace_all(
        &mut self,
        users: &[HashedUser],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), users)
    }
}

/// The bind text of key `id`, or `None` when it holds a NUL or non-ASCII byte, which no stored
/// `usr_id` holds (PIC-002).
fn key_text(id: &PicX<8>) -> Option<String> {
    pic_x_to_string(KEY_COLUMN, id).ok()
}

/// Codec B of every user, in slice order, the first failure reported.
fn rows_of(users: &[HashedUser]) -> Result<Vec<UserRow>, PersistenceError> {
    users.iter().map(UserRow::from_hashed).collect()
}

/// The typed values of a page's rows, in read order.
fn entries(rows: Vec<UserListRow>) -> Result<Vec<UserListEntry>, PersistenceError> {
    rows.into_iter().map(UserListRow::into_entry).collect()
}

/// [`UserRepository::find_by_id`] on `executor`.
async fn find_in<'e, E: PgExecutor<'e>>(
    executor: E,
    id: &PicX<8>,
) -> Result<Option<UserEntry>, PersistenceError> {
    let Some(key) = key_text(id) else {
        return Ok(None);
    };
    select_by_id(executor, &key)
        .await?
        .map(UserRow::into_entry)
        .transpose()
}

/// [`UserRepository::lock_by_id`] on `executor`.
async fn lock_in<'e, E: PgExecutor<'e>>(
    executor: E,
    id: &PicX<8>,
) -> Result<Option<UserEntry>, PersistenceError> {
    let Some(key) = key_text(id) else {
        return Ok(None);
    };
    select_for_update(executor, &key)
        .await?
        .map(UserRow::into_entry)
        .transpose()
}

/// [`UserRepository::page_after`] on `executor`.
async fn page_after_in<'e, E: PgExecutor<'e>>(
    executor: E,
    start: &PicX<8>,
    limit: i64,
) -> Result<Vec<UserListEntry>, PersistenceError> {
    let start = pic_x_to_string(KEY_COLUMN, start)?;
    entries(select_page_forward(executor, &start, limit).await?)
}

/// [`UserRepository::page_before`] on `executor`.
async fn page_before_in<'e, E: PgExecutor<'e>>(
    executor: E,
    start: &PicX<8>,
    limit: i64,
) -> Result<Vec<UserListEntry>, PersistenceError> {
    let start = pic_x_to_string(KEY_COLUMN, start)?;
    entries(select_page_backward(executor, &start, limit).await?)
}

/// [`UserRepository::insert`] on `executor`.
async fn insert_in<'e, E: PgExecutor<'e>>(
    executor: E,
    entry: &UserEntry,
) -> Result<(), PersistenceError> {
    let row = UserRow::from_entry(entry)?;
    insert_row(executor, &row).await?;
    Ok(())
}

/// [`UserRepository::update`] on `executor`.
async fn update_in<'e, E: PgExecutor<'e>>(
    executor: E,
    entry: &UserEntry,
) -> Result<bool, PersistenceError> {
    if key_text(&entry.id).is_none() {
        return Ok(false);
    }
    let row = UserRow::from_entry(entry)?;
    Ok(update_row(executor, &row).await? == 1)
}

/// [`UserRepository::delete`] on `executor`.
async fn delete_in<'e, E: PgExecutor<'e>>(
    executor: E,
    id: &PicX<8>,
) -> Result<bool, PersistenceError> {
    let Some(key) = key_text(id) else {
        return Ok(false);
    };
    Ok(delete_row(executor, &key).await? == 1)
}

/// [`UserRepository::replace_all`] inside the transaction `conn` is in.
async fn replace_all_in(
    conn: &mut PgConnection,
    users: &[HashedUser],
) -> Result<u64, PersistenceError> {
    let rows = rows_of(users)?;
    replace_rows(conn, &rows).await
}

/// Deletes every row, then inserts `rows` in order, on one connection; returns the rows
/// inserted.
///
/// The first statement locks the table `SHARE ROW EXCLUSIVE` until the transaction ends, so
/// replacements of `users` run one at a time and the last to commit leaves its input
/// (LOADS-014, LOADS-040).
async fn replace_rows(conn: &mut PgConnection, rows: &[UserRow]) -> Result<u64, PersistenceError> {
    // One replacement of the table at a time (LOADS-014, LOADS-040): this mode conflicts with
    // itself and with the ROW EXCLUSIVE lock of every row write, so a second load waits here
    // until this transaction ends, and its delete then removes the rows this one committed.
    // Readers take ACCESS SHARE and never wait.
    sqlx::query!("LOCK TABLE users IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *conn)
        .await?;
    delete_every(&mut *conn).await?;
    let mut inserted: u64 = 0;
    for row in rows {
        inserted = inserted.saturating_add(insert_row(&mut *conn, row).await?);
    }
    Ok(inserted)
}

/// The row whose `usr_id` is `key`.
async fn select_by_id<'e, E: PgExecutor<'e>>(
    executor: E,
    key: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT usr_id, usr_fname, usr_lname, password_hash, usr_type, filler
         FROM users
         WHERE usr_id = $1::bpchar",
        key
    )
    .fetch_optional(executor)
    .await
}

/// The row whose `usr_id` is `key`, locked until the enclosing transaction ends.
async fn select_for_update<'e, E: PgExecutor<'e>>(
    executor: E,
    key: &str,
) -> Result<Option<UserRow>, sqlx::Error> {
    sqlx::query_as!(
        UserRow,
        "SELECT usr_id, usr_fname, usr_lname, password_hash, usr_type, filler
         FROM users
         WHERE usr_id = $1::bpchar
         FOR UPDATE",
        key
    )
    .fetch_optional(executor)
    .await
}

/// Up to `limit` list rows from `start` upward (`queries/users_page_forward.sql`).
async fn select_page_forward<'e, E: PgExecutor<'e>>(
    executor: E,
    start: &str,
    limit: i64,
) -> Result<Vec<UserListRow>, sqlx::Error> {
    sqlx::query_file_as!(UserListRow, "queries/users_page_forward.sql", start, limit)
        .fetch_all(executor)
        .await
}

/// Up to `limit` list rows from `start` downward (`queries/users_page_backward.sql`).
async fn select_page_backward<'e, E: PgExecutor<'e>>(
    executor: E,
    start: &str,
    limit: i64,
) -> Result<Vec<UserListRow>, sqlx::Error> {
    sqlx::query_file_as!(UserListRow, "queries/users_page_backward.sql", start, limit)
        .fetch_all(executor)
        .await
}

/// Inserts `row`, every column; returns the rows inserted.
async fn insert_row<'e, E: PgExecutor<'e>>(executor: E, row: &UserRow) -> Result<u64, sqlx::Error> {
    let done = sqlx::query!(
        "INSERT INTO users (usr_id, usr_fname, usr_lname, password_hash, usr_type, filler)
         VALUES ($1, $2, $3, $4, $5, $6)",
        row.usr_id,
        row.usr_fname,
        row.usr_lname,
        row.password_hash,
        row.usr_type,
        row.filler
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// Rewrites every non-key column of the row whose `usr_id` is `row.usr_id`; returns the rows
/// changed.
async fn update_row<'e, E: PgExecutor<'e>>(executor: E, row: &UserRow) -> Result<u64, sqlx::Error> {
    let done = sqlx::query!(
        "UPDATE users
         SET usr_fname = $2, usr_lname = $3, password_hash = $4, usr_type = $5, filler = $6
         WHERE usr_id = $1::bpchar",
        row.usr_id,
        row.usr_fname,
        row.usr_lname,
        row.password_hash,
        row.usr_type,
        row.filler
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// Deletes the row whose `usr_id` is `key`; returns the rows deleted.
async fn delete_row<'e, E: PgExecutor<'e>>(executor: E, key: &str) -> Result<u64, sqlx::Error> {
    let done = sqlx::query!("DELETE FROM users WHERE usr_id = $1::bpchar", key)
        .execute(executor)
        .await?;
    Ok(done.rows_affected())
}

/// Deletes every row of the table.
async fn delete_every<'e, E: PgExecutor<'e>>(executor: E) -> Result<(), sqlx::Error> {
    sqlx::query!("DELETE FROM users").execute(executor).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use copybook::PicX;
    use domain::batch::loads::{HashedUser, decode_usrsec};
    use domain::records::{UserEntry, UserListEntry};
    use test_support::postgres::{PostgresGuard, start_postgres};

    use super::UserRepository;
    use crate::error::{DecodeKind, PersistenceError};
    use crate::{PgStore, PgTx};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// PHC-shaped stand-ins for the strings `domain::password::hash` computes, one per load run:
    /// the repository stores whatever hash it is given, so no test calls argon2.
    const RUN_ONE: &str = "$argon2id$v=19$m=19456,t=2,p=1$cnVub25lc2FsdA$cnVub25lZmFrZWhhc2g";
    const RUN_TWO: &str = "$argon2id$v=19$m=19456,t=2,p=1$cnVudHdvc2FsdA$cnVudHdvZmFrZWhhc2g";

    /// The user IDs of the `DUSRSECJ.jcl` deck, in key byte order.
    const DECK_IDS: [&str; 10] = [
        "ADMIN001", "ADMIN002", "ADMIN003", "ADMIN004", "ADMIN005", "USER0001", "USER0002",
        "USER0003", "USER0004", "USER0005",
    ];

    /// A key holding `USER` and a UTF-8 `é`: no stored `usr_id` can hold it (PIC-002).
    const UNSTORABLE: PicX<8> = PicX::from_array(*b"USER\xC3\xA9  ");

    fn key(text: &str) -> Result<PicX<8>, Box<dyn StdError>> {
        Ok(PicX::from_text(text)?)
    }

    /// The user IDs of a page, trailing spaces dropped.
    fn ids(page: &[UserListEntry]) -> Vec<String> {
        page.iter().map(|entry| entry.id.to_text()).collect()
    }

    /// The ten in-stream records of `app/jcl/DUSRSECJ.jcl`, read in place and decoded as
    /// `dusrsecj` decodes them (LOADS-012), each with `hash` for its password.
    fn deck(hash: &str) -> Result<Vec<HashedUser>, Box<dyn StdError>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/jcl/DUSRSECJ.jcl");
        let jcl = std::fs::read(path)?;
        Ok(decode_usrsec(&jcl)?
            .into_iter()
            .map(|framed| HashedUser {
                record: framed.record,
                password_hash: hash.to_owned(),
            })
            .collect())
    }

    /// A user no fixture holds, with invented names.
    fn entry(id: &str, hash: &str) -> Result<UserEntry, Box<dyn StdError>> {
        Ok(UserEntry {
            id: key(id)?,
            fname: PicX::from_text("ADA")?,
            lname: PicX::from_text("LOVELACE")?,
            password_hash: hash.to_owned(),
            usr_type: PicX::from_text("U")?,
            filler: PicX::spaces(),
        })
    }

    /// A migrated database and a store over it.
    async fn store() -> Result<(PostgresGuard, PgStore), Box<dyn StdError>> {
        let guard = start_postgres().await?;
        let store = PgStore::new(guard.pool().clone());
        Ok((guard, store))
    }

    /// Every user of the table, in key order.
    async fn everyone(store: &mut PgStore) -> Result<Vec<String>, Box<dyn StdError>> {
        Ok(ids(&store.page_after(&PicX::spaces(), 100).await?))
    }

    /// Whether every deck user is stored as `users` gives it, with `hash`.
    async fn holds_deck(
        store: &mut PgStore,
        users: &[HashedUser],
        hash: &str,
    ) -> Result<bool, Box<dyn StdError>> {
        for user in users {
            let record = &user.record;
            let Some(stored) = store.find_by_id(&record.id).await? else {
                return Ok(false);
            };
            let same = stored.id == record.id
                && stored.fname == record.fname
                && stored.lname == record.lname
                && stored.password_hash == hash
                && stored.usr_type == record.usr_type
                && stored.filler == record.filler;
            if !same {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Makes a lock wait inside `tx` fail after 200 ms with SQLSTATE `55P03` instead of
    /// blocking, for the rest of the transaction.
    async fn lock_timeout(tx: &mut PgTx<'_>) -> TestResult {
        sqlx::query_scalar!("SELECT set_config('lock_timeout', '200ms', true)")
            .fetch_one(tx.conn())
            .await?;
        Ok(())
    }

    // audit: LOADS-009
    // audit: LOADS-012
    // audit: LOADS-014
    // audit: LOADS-015
    // audit: LOADS-018
    // audit: LOADS-033
    // audit: LOADS-034
    #[tokio::test]
    async fn replace_all_loads_the_deck_and_reloads_it_whole() -> TestResult {
        let (_guard, mut store) = store().await?;
        let first = deck(RUN_ONE)?;
        assert_eq!(first.len(), DECK_IDS.len());

        // The first run meets an empty table, which is no error.
        assert_eq!(store.replace_all(&first).await?, 10);
        assert_eq!(everyone(&mut store).await?, DECK_IDS);
        assert!(holds_deck(&mut store, &first, RUN_ONE).await?);

        // A rerun removes a row the deck lacks and stores the new hashes.
        store.insert(&entry("ZZTEMP01", RUN_ONE)?).await?;
        let second = deck(RUN_TWO)?;
        assert_eq!(store.replace_all(&second).await?, 10);
        assert_eq!(everyone(&mut store).await?, DECK_IDS);
        assert!(holds_deck(&mut store, &second, RUN_TWO).await?);

        // A failed replacement leaves the previous contents: a duplicate key rolls the
        // transaction back, and a value no column can store stops it before any statement.
        let mut duplicated = deck(RUN_ONE)?;
        duplicated.push(duplicated.first().cloned().ok_or("empty deck")?);
        assert_eq!(
            store.replace_all(&duplicated).await,
            Err(PersistenceError::Duplicate {
                constraint: Some("users_pkey".to_owned()),
            })
        );
        let mut unstorable = deck(RUN_ONE)?;
        unstorable
            .last_mut()
            .ok_or("empty deck")?
            .password_hash
            .push('\0');
        assert_eq!(
            store.replace_all(&unstorable).await,
            Err(PersistenceError::decode(
                "password_hash",
                DecodeKind::InvalidByte
            ))
        );
        assert!(holds_deck(&mut store, &second, RUN_TWO).await?);

        // On a transaction the replacement joins it: rolled back, nothing of it remains.
        let mut tx = store.begin().await?;
        let two = deck(RUN_ONE)?.get(..2).ok_or("short deck")?.to_vec();
        assert_eq!(tx.replace_all(&two).await?, 2);
        assert_eq!(
            ids(&tx.page_after(&PicX::spaces(), 100).await?),
            ["ADMIN001", "ADMIN002"]
        );
        tx.rollback().await?;
        assert!(holds_deck(&mut store, &second, RUN_TWO).await?);
        Ok(())
    }

    // audit: COUSR00C-012
    // audit: COUSR00C-013
    // audit: COUSR00C-022
    // audit: COUSR00C-028
    #[tokio::test]
    async fn pages_run_in_key_byte_order_from_an_inclusive_start_key() -> TestResult {
        let (_guard, mut store) = store().await?;
        store.replace_all(&deck(RUN_ONE)?).await?;
        let blank = PicX::spaces();

        // Forward from the blank key: every user, ascending, at most `limit` of them.
        assert_eq!(ids(&store.page_after(&blank, 11).await?), DECK_IDS);
        assert_eq!(
            ids(&store.page_after(&blank, 3).await?).as_slice(),
            DECK_IDS.get(..3).ok_or("short deck")?
        );
        // The start key is included; a key between two IDs starts at the next one.
        assert_eq!(
            ids(&store.page_after(&key("USER0004")?, 11).await?),
            ["USER0004", "USER0005"]
        );
        assert_eq!(
            ids(&store.page_after(&key("ADMIN00Z")?, 2).await?),
            ["USER0001", "USER0002"]
        );
        assert!(store.page_after(&key("ZZZZZZZZ")?, 11).await?.is_empty());

        // Backward: descending from the start key, itself included; nothing is below blank.
        assert_eq!(
            ids(&store.page_before(&key("USER0001")?, 3).await?),
            ["USER0001", "ADMIN005", "ADMIN004"]
        );
        assert!(store.page_before(&blank, 11).await?.is_empty());

        // A page carries the four listed fields as stored.
        assert_eq!(
            store.page_after(&key("USER0001")?, 1).await?,
            vec![UserListEntry {
                id: key("USER0001")?,
                fname: PicX::from_text("LAWRENCE")?,
                lname: PicX::from_text("THOMAS")?,
                usr_type: PicX::from_text("U")?,
            }]
        );

        // Byte order, not a locale's: a leading space sorts first and lower case last.
        store.insert(&entry("admin009", RUN_ONE)?).await?;
        store.insert(&entry(" LEADING", RUN_ONE)?).await?;
        let all = everyone(&mut store).await?;
        assert_eq!(all.first().map(String::as_str), Some(" LEADING"));
        assert_eq!(all.last().map(String::as_str), Some("admin009"));
        let mut tx = store.begin().await?;
        assert_eq!(
            ids(&tx.page_before(&key("ADMIN002")?, 11).await?),
            ["ADMIN002", "ADMIN001", " LEADING"]
        );
        tx.commit().await?;

        // A start key no stored ID can hold has no position: a typed error, never a page.
        let invalid = Err(PersistenceError::decode("usr_id", DecodeKind::InvalidByte));
        assert_eq!(store.page_after(&UNSTORABLE, 11).await, invalid);
        assert_eq!(store.page_before(&UNSTORABLE, 11).await, invalid);
        Ok(())
    }

    // audit: COSGN00C-013
    // audit: COSGN00C-019
    // audit: COUSR01C-016
    // audit: COUSR01C-023
    // audit: COUSR02C-011
    // audit: COUSR02C-029
    // audit: COUSR02C-031
    // audit: COUSR03C-019
    // audit: COUSR03C-021
    #[tokio::test]
    async fn writes_report_duplicate_and_missing_keys() -> TestResult {
        let (_guard, mut store) = store().await?;
        let added = entry("NEWUSR01", RUN_ONE)?;
        assert_eq!(store.find_by_id(&added.id).await?, None);
        store.insert(&added).await?;
        assert_eq!(store.find_by_id(&added.id).await?, Some(added.clone()));
        // Keys are bound as given: another case or a leading space is another key.
        assert_eq!(store.find_by_id(&key("newusr01")?).await?, None);
        assert_eq!(store.find_by_id(&key(" NEWUSR0")?).await?, None);

        // A second write of the key is `DUPKEY`, and the stored row is unchanged.
        let again = entry("NEWUSR01", RUN_TWO)?;
        let duplicate = Err(PersistenceError::Duplicate {
            constraint: Some("users_pkey".to_owned()),
        });
        assert_eq!(store.insert(&again).await, duplicate);
        assert_eq!(store.find_by_id(&added.id).await?, Some(added.clone()));
        // Inside a transaction the failed insert runs in a savepoint, and the rest goes on.
        let mut tx = store.begin().await?;
        let mut savepoint = tx.savepoint().await?;
        assert_eq!(savepoint.insert(&again).await, duplicate);
        savepoint.rollback().await?;
        assert_eq!(tx.find_by_id(&added.id).await?, Some(added.clone()));
        tx.commit().await?;

        // The rewrite stores every non-key column as given, `FILLER` byte for byte.
        let mut changed = added.clone();
        changed.fname = PicX::from_text("AUGUSTA")?;
        changed.lname = PicX::from_text("KING")?;
        changed.password_hash = RUN_TWO.to_owned();
        changed.usr_type = PicX::from_text("A")?;
        changed.filler = PicX::from_array(*b"kept byte for byte 0123");
        assert!(store.update(&changed).await?);
        assert_eq!(store.find_by_id(&changed.id).await?, Some(changed.clone()));
        assert!(!store.update(&entry("NOSUCH01", RUN_ONE)?).await?);
        assert_eq!(store.find_by_id(&key("NOSUCH01")?).await?, None);

        assert!(store.delete(&changed.id).await?);
        assert_eq!(store.find_by_id(&changed.id).await?, None);
        assert!(!store.delete(&changed.id).await?);

        // A key no stored row can hold is not found, and no statement runs.
        assert_eq!(store.find_by_id(&UNSTORABLE).await?, None);
        assert_eq!(store.lock_by_id(&UNSTORABLE).await?, None);
        assert!(!store.delete(&UNSTORABLE).await?);
        let mut unstorable_key = entry("NEWUSR02", RUN_ONE)?;
        unstorable_key.id = UNSTORABLE;
        assert!(!store.update(&unstorable_key).await?);
        let invalid_id = Err(PersistenceError::decode("usr_id", DecodeKind::InvalidByte));
        assert_eq!(store.insert(&unstorable_key).await, invalid_id);
        // A value no column can store is a typed error, and nothing is written.
        let mut nul_hash = entry("NEWUSR02", RUN_ONE)?;
        nul_hash.password_hash.push('\0');
        let invalid_hash = Err(PersistenceError::decode(
            "password_hash",
            DecodeKind::InvalidByte,
        ));
        assert_eq!(store.insert(&nul_hash).await, invalid_hash);
        assert_eq!(store.find_by_id(&nul_hash.id).await?, None);
        Ok(())
    }

    // audit: COUSR02C-011
    // audit: COUSR02C-030
    #[tokio::test]
    async fn a_key_shorter_than_its_field_is_found_by_every_point_access() -> TestResult {
        let (_guard, mut store) = store().await?;
        // COUSR01C stores a user ID as typed, so `AB CD` is kept padded to `AB CD   `.
        let short = entry("AB CD", RUN_ONE)?;
        store.insert(&short).await?;
        assert_eq!(store.find_by_id(&short.id).await?, Some(short.clone()));
        // The padding is the key's: the unpadded prefix and a leading space are other keys.
        assert_eq!(store.find_by_id(&key("AB")?).await?, None);
        assert_eq!(store.find_by_id(&key(" AB CD")?).await?, None);

        let mut tx = store.begin().await?;
        assert_eq!(tx.lock_by_id(&short.id).await?, Some(short.clone()));
        let mut changed = short.clone();
        changed.fname = PicX::from_text("RENAMED")?;
        assert!(tx.update(&changed).await?);
        tx.commit().await?;
        assert_eq!(store.find_by_id(&short.id).await?, Some(changed));

        assert!(store.delete(&short.id).await?);
        assert_eq!(store.find_by_id(&short.id).await?, None);
        Ok(())
    }

    // audit: COUSR02C-024
    // audit: COUSR03C-017
    #[tokio::test]
    async fn lock_by_id_holds_the_row_until_the_transaction_ends() -> TestResult {
        let (_guard, mut store) = store().await?;
        store.replace_all(&deck(RUN_ONE)?).await?;
        let id = key("USER0001")?;

        // Through the pool the lock ends with its statement.
        assert!(store.lock_by_id(&id).await?.is_some());
        assert!(store.lock_by_id(&id).await?.is_some());

        let mut holder = store.begin().await?;
        let held = holder.lock_by_id(&id).await?.ok_or("USER0001 not loaded")?;
        assert_eq!(held.id, id);
        // A plain read does not wait for the lock.
        assert_eq!(store.find_by_id(&id).await?, Some(held.clone()));
        // Another transaction cannot take it while the holder's transaction is open.
        let mut waiter = store.begin().await?;
        lock_timeout(&mut waiter).await?;
        assert_eq!(
            waiter.lock_by_id(&id).await,
            Err(PersistenceError::Database {
                sqlstate: Some("55P03".to_owned()),
                constraint: None,
            })
        );
        waiter.rollback().await?;

        // The holder rewrites the row and commits, which releases the lock.
        let mut changed = held.clone();
        changed.fname = PicX::from_text("RENAMED")?;
        assert!(holder.update(&changed).await?);
        holder.commit().await?;

        // A delete in a transaction that rolls back leaves the row.
        let mut undone = store.begin().await?;
        lock_timeout(&mut undone).await?;
        assert_eq!(undone.lock_by_id(&id).await?, Some(changed.clone()));
        assert!(undone.delete(&id).await?);
        assert_eq!(undone.lock_by_id(&id).await?, None);
        undone.rollback().await?;
        assert_eq!(store.find_by_id(&id).await?, Some(changed));

        // Committed, it removes the row.
        let mut done = store.begin().await?;
        assert!(done.lock_by_id(&id).await?.is_some());
        assert!(done.delete(&id).await?);
        assert!(!done.delete(&id).await?);
        done.commit().await?;
        assert_eq!(store.find_by_id(&id).await?, None);
        Ok(())
    }
}
