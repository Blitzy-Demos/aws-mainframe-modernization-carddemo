//! `AccountRepository`: the statements on table `accounts` (migration
//! `0001_reference_and_master.sql`), the 300-byte `ACCOUNT-RECORD` of `CVACT01Y`
//! (CVACT01Y-001 to CVACT01Y-013), keyed by `acct_id` as the ACCTDAT KSDS is by `KEYS(11 0)`
//! (`ACCTFILE.jcl:40`; LOADS-001).
//!
//! # COBOL accesses
//!
//! | COBOL access | Method |
//! |---|---|
//! | COACTVWC and COACTUPC `9000-READ-ACCT`: the cross-reference through `CXACAIX`, then the account, then the customer (`COACTVWC.cbl:687-870`, `COACTUPC.cbl:3608-3797`; COACTVWC-033 to COACTVWC-047, COACTUPC-084 to COACTUPC-099); COPAUS0C `GATHER-ACCOUNT-DETAILS`, the same three reads (`COPAUS0C.cbl:750-812,865-964`; COPAUS0C-013 to COPAUS0C-019) | [`find_with_customer`](AccountRepository::find_with_customer) |
//! | Keyed `READ` of the account: CBTRN02C `1500-B-LOOKUP-ACCT` (`CBTRN02C.cbl:393-421`), CBACT04C `1100-GET-ACCT-DATA` (`CBACT04C.cbl:372-391`), COPAUA0C `5200-READ-ACCT-RECORD` (`COPAUA0C.cbl:520-564`), COACCT01 `4000-PROCESS-REQUEST-REPLY` (`COACCT01.cbl:390-460`) and CBSTM03B `4000-ACCTFILE-PROC` (`CBSTM03B.CBL:206-226`) | [`find_by_id`](AccountRepository::find_by_id) |
//! | `READ … UPDATE` of the account: COACTUPC `9600-WRITE-PROCESSING` (`COACTUPC.cbl:3894-3920`; COACTUPC-101), COBIL00C `READ-ACCTDAT-FILE` (`COBIL00C.cbl:343-376`) | [`lock_by_id`](AccountRepository::lock_by_id) |
//! | `REWRITE` of the account: COACTUPC `9600-WRITE-PROCESSING` (`COACTUPC.cbl:4066`; COACTUPC-103, COACTUPC-106), COBIL00C `UPDATE-ACCTDAT-FILE` (`COBIL00C.cbl:377-406`), CBTRN02C `2800-UPDATE-ACCOUNT-REC` (`CBTRN02C.cbl:545-559`), CBACT04C `1050-UPDATE-ACCOUNT` (`CBACT04C.cbl:350-370`) | [`update`](AccountRepository::update) |
//! | ACCTFILE `DELETE`, `DEFINE CLUSTER` and `REPRO` of `acctdata.txt`, batch `acctfile` (LOADS-001, LOADS-014, LOADS-015, LOADS-018, LOADS-025) | [`replace_all`](AccountRepository::replace_all) |
//! | Writing one account, for the tests and the fixture harness | [`insert`](AccountRepository::insert) |
//! | Reading the loaded table back, for the `acctfile` load and the tests | [`find_all`](AccountRepository::find_all) |
//!
//! The CICS response or file status each program branches on becomes the method's outcome:
//! `NORMAL` (status `00`) is `Ok(Some(_))`, `NOTFND` (`INVALID KEY`) is `Ok(None)`, or
//! `Ok(false)` for [`update`](AccountRepository::update), and every other response is a
//! [`PersistenceError`], the `WHEN OTHER` branch.
//!
//! # The account chain
//!
//! [`find_with_customer`](AccountRepository::find_with_customer) issues the three reads of
//! `9000-READ-ACCT` in program order and reports each as a [`ChainLink`], so that the services
//! of endpoints 4 and 5 hand every outcome to `domain::online::coactvwc` and
//! `domain::online::coactupc`, which decide what it means. It stops only after the
//! cross-reference read: the exits after the account and customer reads test condition names
//! whose `SET` statements are commented out (`COACTVWC.cbl:704,713,792,842`,
//! `COACTUPC.cbl:3627,3636,3719,3769`), so the customer is read whatever the account read
//! returned (COACTVWC-033, COACTUPC-084).
//!
//! COPAUS0C's `GATHER-ACCOUNT-DETAILS` runs differently in the COBOL: it performs all three
//! reads whatever each returns, the account keyed by the cross-reference's `XREF-ACCT-ID` and the
//! customer by its `XREF-CUST-ID` (`COPAUS0C.cbl:750-812,865-964`). The service of endpoint 21,
//! `api::services::copaus0c`, still reads all three through this chain (COPAUS0C-013 to
//! COPAUS0C-019). Endpoint 21 stops at the first failed read (X-COPAUS0C-1), so it never answers
//! with a read the chain skips after a failed cross-reference read, and the account key it passes
//! equals `XREF-ACCT-ID`, because the cross-reference was found by that account.
//!
//! # Statements
//!
//! Each statement is written once, in a private function over the executor, and [`PgStore`]
//! (the pool), [`PgTx`] (the caller's transaction) and [`PgSession`] (a batch job's session)
//! call it, so the offline query cache holds one entry per statement. The chain reuses the cross-reference and customer
//! statements of `repos::card_xref` and `repos::customers` rather than repeating them. Column
//! lists name all 13 columns in table order, which are the field names of [`AccountRow`].
//! Records reach SQL only through [`AccountRow::from_record`] and come back only through
//! [`AccountRow::into_record`] (codec B): amounts keep their scale, blank dates stay `NULL`, and
//! no byte is trimmed, padded or re-encoded.
//!
//! # Redaction
//!
//! No account field is cardholder data, but the chain carries a cross-reference, whose card
//! number is a `CardNumber`, and a customer, whose SSN is an `Ssn`: their `Debug` output is the
//! redacting one of `domain::records`, and this module never formats a record, row or key into
//! an error and emits no tracing (Cardholder Data Redaction).

use copybook::PicU;
use domain::records::{AccountRecord, CardXrefRecord, CustomerRecord};
use sqlx::{PgConnection, PgExecutor};

use super::card_xref::select_first_two_by_acct_id;
use super::customers;
use crate::error::PersistenceError;
use crate::rows::{AccountRow, pic_u_to_i64};
use crate::{PgSession, PgStore, PgTx, ProbePoint, RepositoryProbe, probe_verdict, probed};

/// Column of `ACCT-ID`, named by a key conversion error.
const ACCT_ID: &str = "acct_id";

/// The index over `XREF-ACCT-ID` that replaces the alternate index under `CXACAIX` (LOADS-023),
/// named by the `DUPKEY` outcome of the chain's cross-reference read.
const XREF_ACCT_ID_INDEX: &str = "card_xref_acct_id_idx";

/// Outcome of one link of a multi-file read, as the CICS `RESP` of its `READ` reports it.
///
/// The service converts each link into the lookup its domain rule takes
/// (`domain::online::coactvwc::Lookup` and the COACTUPC equivalent).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainLink<T> {
    /// `DFHRESP(NORMAL)`: the record was read.
    Found(T),
    /// `DFHRESP(NOTFND)`: no record has the key (COACTVWC-035, COACTVWC-040, COACTVWC-044;
    /// COACTUPC-085).
    NotFound,
    /// Any other response, the `WHEN OTHER` branch: the statement failed or the stored row does
    /// not fit its picture (COACTVWC-037, COACTVWC-042, COACTVWC-046; COACTUPC-087). For the
    /// cross-reference read it also stands for two or more rows sharing the account, which CICS
    /// answers with `DUPKEY` through the non-unique path `CXACAIX`; that link holds
    /// [`PersistenceError::Duplicate`] naming `card_xref_acct_id_idx` (COACTVWC-039,
    /// COACTUPC-089).
    Failed(PersistenceError),
    /// The program never issued this read, because an earlier link ended the chain.
    NotReached,
}

/// The three reads of `9000-READ-ACCT` (COACTVWC-033, COACTUPC-084), in program order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountCustomerChain {
    /// `9200-GETCARDXREF-BYACCT`: the cross-reference read by the account ID through
    /// `CXACAIX` (`COACTVWC.cbl:723-770`, `COACTUPC.cbl:3650-3698`). Only
    /// [`ChainLink::Found`], exactly one row, lets the chain go on (COACTVWC-034,
    /// COACTUPC-090).
    pub xref: ChainLink<CardXrefRecord>,
    /// `9300-GETACCTDATA-BYACCT`: the account read by the same key (`COACTVWC.cbl:774-820`;
    /// COACTVWC-040 to COACTVWC-043, COACTUPC-091 to COACTUPC-094);
    /// [`ChainLink::NotReached`] unless the cross-reference was found.
    pub account: ChainLink<AccountRecord>,
    /// `9400-GETCUSTDATA-BYCUST`: the customer read by the cross-reference's `XREF-CUST-ID`,
    /// whatever the account read returned (`COACTVWC.cbl:825-870`; COACTVWC-044 to
    /// COACTVWC-047, COACTUPC-095 to COACTUPC-098); [`ChainLink::NotReached`] unless the
    /// cross-reference was found.
    pub customer: ChainLink<CustomerRecord>,
}

impl AccountCustomerChain {
    /// The chain that ended at its cross-reference read: `xref` reports that read, and neither
    /// later read was issued (`IF FLG-ACCTFILTER-NOT-OK GO TO 9000-READ-ACCT-EXIT`,
    /// `COACTVWC.cbl:697-699`).
    fn ended_at_xref(xref: ChainLink<CardXrefRecord>) -> Self {
        Self {
            xref,
            account: ChainLink::NotReached,
            customer: ChainLink::NotReached,
        }
    }
}

/// The statements on `accounts`, implemented by [`PgStore`], [`PgTx`] and [`PgSession`]; test
/// doubles in other crates implement the same trait.
///
/// On [`PgStore`] each call runs in autocommit, except [`replace_all`](Self::replace_all),
/// which opens a transaction of its own, and
/// [`find_with_customer`](Self::find_with_customer), which runs its reads on one pooled
/// connection. On [`PgTx`] every call runs inside the caller's transaction. On [`PgSession`]
/// every call runs on the session's one connection, in autocommit as on [`PgStore`].
pub trait AccountRepository: Send {
    /// The reads of `9000-READ-ACCT` for account `acct_id`, each reported as its own
    /// [`ChainLink`] (COACTVWC-033, COACTUPC-084):
    ///
    /// 1. The cross-reference through `CXACAIX`, at most the first two `card_xref` rows of the
    ///    account in card-number order: none is [`ChainLink::NotFound`], exactly one is
    ///    [`ChainLink::Found`], and a second row, however many more share the account, is the
    ///    `DUPKEY` failure of [`ChainLink::Failed`] (COACTVWC-039, COACTUPC-089). Unless it is
    ///    `Found`, the program leaves the paragraph, so `account` and `customer` are
    ///    [`ChainLink::NotReached`].
    /// 2. The account by `acct_id`.
    /// 3. The customer by the cross-reference's `cust_id`, whatever the account read returned.
    ///
    /// Infallible: every failure, of a statement, of a row's codec B conversion or, on
    /// [`PgStore`], of acquiring the connection (reported on `xref`), is carried in its link.
    /// On [`PgTx`] a failed statement aborts the transaction, so the reads after it report that
    /// abort; the read-only fetches of endpoints 4 and 5 run on [`PgStore`], where each
    /// statement stands on its own.
    fn find_with_customer(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = AccountCustomerChain> + Send;

    /// The account stored under `id`, or `None` when no row has that key (CICS `NOTFND`,
    /// `INVALID KEY`): the keyed `READ` of CBTRN02C, CBACT04C, COPAUA0C, COACCT01 and CBSTM03B,
    /// served by `accounts_pkey`.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Unavailable`] when the database cannot be reached,
    /// [`PersistenceError::Database`] for any other failure of the read, and
    /// [`PersistenceError::Decode`] when the stored row does not fit `CVACT01Y`
    /// ([`AccountRow::into_record`]).
    fn find_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send;

    /// The account stored under `id`, read with a row lock (`SELECT … FOR UPDATE`), or `None`
    /// when no row has that key: the `READ … UPDATE` of COACTUPC's `9600-WRITE-PROCESSING`
    /// (COACTUPC-101) and of COBIL00C's `READ-ACCTDAT-FILE`.
    ///
    /// On a [`PgTx`] the lock holds until the transaction commits or rolls back, so no other
    /// transaction can update or lock the row in between, as CICS holds the record for the
    /// task's `REWRITE`. On a [`PgStore`] the statement commits at once and the lock is
    /// released with it, so the lock means something only on a [`PgTx`]. A lock that another
    /// transaction holds makes the call wait, as a CICS `READ UPDATE` waits on the record
    /// another task holds.
    ///
    /// # Errors
    ///
    /// As [`find_by_id`](Self::find_by_id); [`PersistenceError::Database`] also reports a lock
    /// that cannot be had, such as SQLSTATE `55P03` when the session's `lock_timeout` expires or
    /// `40P01` for a deadlock. COACTUPC reports every outcome but `NORMAL` as
    /// `Could not lock account record for update` (COACTUPC-101).
    fn lock_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send;

    /// Every stored account in key order, as a KSDS browse returns them: for the `acctfile`
    /// load, the batch jobs and the tests only.
    ///
    /// The read is unbounded. No API collection calls it, so Bounded Pagination does not apply.
    ///
    /// # Errors
    ///
    /// As [`find_by_id`](Self::find_by_id), the first row that does not decode ending the read.
    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<AccountRecord>, PersistenceError>> + Send;

    /// Stores `record` as a new row, every field as codec B maps it.
    ///
    /// On a [`PgTx`], a failed insert aborts the whole transaction: a caller that must go on
    /// after an expected failure, such as a duplicate key, runs the insert inside a
    /// [`PgTx::savepoint`] and rolls back to it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Duplicate`] when a row already has the key (constraint
    /// `accounts_pkey`, VSAM duplicate key); [`PersistenceError::Decode`] for a `PIC X(n)` field
    /// holding a NUL or non-ASCII byte, which its `CHAR(n)` column cannot keep
    /// ([`AccountRow::from_record`]); [`PersistenceError::Unavailable`] and
    /// [`PersistenceError::Database`] as for [`find_by_id`](Self::find_by_id).
    fn insert(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send;

    /// Rewrites the row whose key is `record.id` with every other field of `record`, `FILLER`
    /// included: the account `REWRITE` of COACTUPC's `9600-WRITE-PROCESSING` (COACTUPC-103),
    /// COBIL00C's `UPDATE-ACCTDAT-FILE`, CBTRN02C's `2800-UPDATE-ACCOUNT-REC` and CBACT04C's
    /// `1050-UPDATE-ACCOUNT`.
    ///
    /// Returns `true` when the row was rewritten and `false` when no row has the key (CICS
    /// `NOTFND`, `INVALID KEY`). The key itself is never changed, as a `REWRITE` cannot change
    /// it.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] as for [`insert`](Self::insert);
    /// [`PersistenceError::Unavailable`] and [`PersistenceError::Database`] as for
    /// [`find_by_id`](Self::find_by_id). COACTUPC reports a failed rewrite as
    /// `Update of record failed` (COACTUPC-106).
    fn update(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send;

    /// Replaces the table's contents with `records`, inserted in slice order, and returns how
    /// many rows it inserted: the ACCTFILE job's `DELETE`, `DEFINE CLUSTER` and `REPRO` into the
    /// new, empty cluster (LOADS-001, LOADS-014).
    ///
    /// The delete and every insert form one unit: on a [`PgStore`] or a [`PgSession`] a
    /// transaction of their own, committed when the last insert succeeds; on a [`PgTx`] the
    /// caller's transaction. An empty table is no error (LOADS-015). No foreign key joins
    /// `accounts`, so the load may run in any order with the other loads (LOADS-025).
    ///
    /// The delete is preceded by a `SHARE ROW EXCLUSIVE` lock on the table, held until the
    /// transaction ends, so a concurrent replacement or row write waits for it and then sees its
    /// committed rows: two loads at once leave the input of the one that commits last, never
    /// their union (LOADS-040). Reads do not wait.
    ///
    /// # Errors
    ///
    /// [`PersistenceError::Decode`] when a record holds a byte codec B cannot store, as for
    /// [`insert`](Self::insert): every record is converted before the delete, so the call fails
    /// with nothing written, leaving the table and a [`PgTx`] caller's transaction as they were.
    /// Otherwise the first failure of the delete or of an insert, as [`insert`](Self::insert)
    /// reports it, a duplicate key in `records` included. On a [`PgStore`] or a [`PgSession`]
    /// the transaction is then rolled back and the previous contents remain (LOADS-018); on a
    /// [`PgTx`] the failed statement aborts the caller's transaction.
    fn replace_all(
        &mut self,
        records: &[AccountRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send;
}

/// A read's result as a link: a row is [`ChainLink::Found`], no row [`ChainLink::NotFound`],
/// any error [`ChainLink::Failed`].
fn link<T>(read: Result<Option<T>, PersistenceError>) -> ChainLink<T> {
    match read {
        Ok(Some(record)) => ChainLink::Found(record),
        Ok(None) => ChainLink::NotFound,
        Err(error) => ChainLink::Failed(error),
    }
}

/// The cross-reference link from the account's first `card_xref` rows in card-number order, at
/// most the two `select_first_two_by_acct_id` reads: a `READ` through `CXACAIX` answers `NORMAL`
/// only when one record has the alternate key, and `DUPKEY` when others share it, which a second
/// row already shows (COACTVWC-039, COACTUPC-089).
fn xref_link(rows: Vec<CardXrefRecord>) -> ChainLink<CardXrefRecord> {
    let mut rows = rows.into_iter();
    match (rows.next(), rows.next()) {
        (None, _) => ChainLink::NotFound,
        (Some(record), None) => ChainLink::Found(record),
        (Some(_), Some(_)) => ChainLink::Failed(PersistenceError::Duplicate {
            constraint: Some(XREF_ACCT_ID_INDEX.to_owned()),
        }),
    }
}

/// The three reads of `9000-READ-ACCT` in sequence on `conn`: the cross-reference by
/// `acct_id`, at most its first two rows, then, only when exactly one was found, the account by
/// `acct_id` and the customer by the cross-reference's customer ID, whatever the account read
/// returned.
///
/// `probe` is asked before the account read ([`ProbePoint::AccountRead`]) and before the
/// customer read ([`ProbePoint::CustomerRead`]), so a skipped read becomes the link its verdict
/// answers, through [`link`], and the reads after it still run.
async fn chain_in(
    conn: &mut PgConnection,
    probe: Option<&dyn RepositoryProbe>,
    acct_id: PicU<11>,
) -> AccountCustomerChain {
    // 9200-GETCARDXREF-BYACCT, then IF FLG-ACCTFILTER-NOT-OK GO TO 9000-READ-ACCT-EXIT.
    let xref = match select_first_two_by_acct_id(&mut *conn, acct_id).await {
        Ok(rows) => match xref_link(rows) {
            ChainLink::Found(record) => record,
            unfound => return AccountCustomerChain::ended_at_xref(unfound),
        },
        Err(error) => return AccountCustomerChain::ended_at_xref(ChainLink::Failed(error)),
    };
    // 9300-GETACCTDATA-BYACCT; its exit test is never true (COACTVWC-033, COACTUPC-084).
    let verdict = probe_verdict(probe, ProbePoint::AccountRead, &[&acct_id]);
    let account = link(probed(verdict, select_by_id(&mut *conn, acct_id)).await);
    // 9400-GETCUSTDATA-BYCUST, keyed by CDEMO-CUST-ID, which 9200 moved from XREF-CUST-ID.
    let verdict = probe_verdict(probe, ProbePoint::CustomerRead, &[&xref.cust_id]);
    let customer = link(probed(verdict, customers::select_by_id(&mut *conn, xref.cust_id)).await);
    AccountCustomerChain {
        xref: ChainLink::Found(xref),
        account,
        customer,
    }
}

/// `SELECT` of the row under `id`, decoded through codec B; `None` when no row has the key.
async fn select_by_id<'e, E: PgExecutor<'e>>(
    executor: E,
    id: PicU<11>,
) -> Result<Option<AccountRecord>, PersistenceError> {
    let key = pic_u_to_i64(ACCT_ID, id)?;
    let row = sqlx::query_as!(
        AccountRow,
        "SELECT acct_id, acct_active_status, acct_curr_bal, acct_credit_limit, \
         acct_cash_credit_limit, acct_open_date, acct_expiraion_date, acct_reissue_date, \
         acct_curr_cyc_credit, acct_curr_cyc_debit, acct_addr_zip, acct_group_id, filler \
         FROM accounts WHERE acct_id = $1",
        key
    )
    .fetch_optional(executor)
    .await?;
    row.map(AccountRow::into_record).transpose()
}

/// `SELECT … FOR UPDATE` of the row under `id`. The row lock lasts as long as the executor's
/// transaction: until the caller's [`PgTx`] commits or rolls back, or, on the pool, only as long
/// as the statement.
async fn select_for_update<'e, E: PgExecutor<'e>>(
    executor: E,
    id: PicU<11>,
) -> Result<Option<AccountRecord>, PersistenceError> {
    let key = pic_u_to_i64(ACCT_ID, id)?;
    let row = sqlx::query_as!(
        AccountRow,
        "SELECT acct_id, acct_active_status, acct_curr_bal, acct_credit_limit, \
         acct_cash_credit_limit, acct_open_date, acct_expiraion_date, acct_reissue_date, \
         acct_curr_cyc_credit, acct_curr_cyc_debit, acct_addr_zip, acct_group_id, filler \
         FROM accounts WHERE acct_id = $1 FOR UPDATE",
        key
    )
    .fetch_optional(executor)
    .await?;
    row.map(AccountRow::into_record).transpose()
}

/// `SELECT` of every row in key order, each decoded through codec B. Unbounded: batch jobs,
/// loads and tests only.
async fn select_all<'e, E: PgExecutor<'e>>(
    executor: E,
) -> Result<Vec<AccountRecord>, PersistenceError> {
    let rows = sqlx::query_as!(
        AccountRow,
        "SELECT acct_id, acct_active_status, acct_curr_bal, acct_credit_limit, \
         acct_cash_credit_limit, acct_open_date, acct_expiraion_date, acct_reissue_date, \
         acct_curr_cyc_credit, acct_curr_cyc_debit, acct_addr_zip, acct_group_id, filler \
         FROM accounts ORDER BY acct_id"
    )
    .fetch_all(executor)
    .await?;
    rows.into_iter().map(AccountRow::into_record).collect()
}

/// `INSERT` of `record` as one row, converted by [`AccountRow::from_record`] and bound by
/// [`insert_row`]; returns the number of rows inserted.
async fn insert_one<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &AccountRecord,
) -> Result<u64, PersistenceError> {
    let row = AccountRow::from_record(record)?;
    insert_row(executor, &row).await
}

/// The one `INSERT` statement, which [`insert_one`] and [`replace_all_in`] share: every column
/// bound from `row`, already converted through codec B; returns the number of rows inserted.
async fn insert_row<'e, E: PgExecutor<'e>>(
    executor: E,
    row: &AccountRow,
) -> Result<u64, PersistenceError> {
    let done = sqlx::query!(
        "INSERT INTO accounts (acct_id, acct_active_status, acct_curr_bal, acct_credit_limit, \
         acct_cash_credit_limit, acct_open_date, acct_expiraion_date, acct_reissue_date, \
         acct_curr_cyc_credit, acct_curr_cyc_debit, acct_addr_zip, acct_group_id, filler) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)",
        row.acct_id,
        row.acct_active_status,
        row.acct_curr_bal,
        row.acct_credit_limit,
        row.acct_cash_credit_limit,
        row.acct_open_date,
        row.acct_expiraion_date,
        row.acct_reissue_date,
        row.acct_curr_cyc_credit,
        row.acct_curr_cyc_debit,
        row.acct_addr_zip,
        row.acct_group_id,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected())
}

/// `UPDATE` of every non-key column of the row whose key is `record.id`, `filler` included;
/// `true` when exactly one row changed, `false` when no row has the key.
async fn update_one<'e, E: PgExecutor<'e>>(
    executor: E,
    record: &AccountRecord,
) -> Result<bool, PersistenceError> {
    let row = AccountRow::from_record(record)?;
    let done = sqlx::query!(
        "UPDATE accounts SET acct_active_status = $2, acct_curr_bal = $3, \
         acct_credit_limit = $4, acct_cash_credit_limit = $5, acct_open_date = $6, \
         acct_expiraion_date = $7, acct_reissue_date = $8, acct_curr_cyc_credit = $9, \
         acct_curr_cyc_debit = $10, acct_addr_zip = $11, acct_group_id = $12, filler = $13 \
         WHERE acct_id = $1",
        row.acct_id,
        row.acct_active_status,
        row.acct_curr_bal,
        row.acct_credit_limit,
        row.acct_cash_credit_limit,
        row.acct_open_date,
        row.acct_expiraion_date,
        row.acct_reissue_date,
        row.acct_curr_cyc_credit,
        row.acct_curr_cyc_debit,
        row.acct_addr_zip,
        row.acct_group_id,
        row.filler,
    )
    .execute(executor)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// `DELETE` of every row, then [`insert_row`] of each record in slice order, on one connection
/// that the caller holds inside a transaction; returns the number of rows inserted.
///
/// Every record is converted through codec B before the delete, so a record that
/// [`AccountRow::from_record`] cannot store fails the call before any statement is sent: the
/// table, and a caller's transaction, are left as they were (LOADS-018).
///
/// The first statement locks the table `SHARE ROW EXCLUSIVE` until the transaction ends, so
/// replacements of `accounts` run one at a time and the last to commit leaves its input
/// (LOADS-014, LOADS-040).
async fn replace_all_in(
    conn: &mut PgConnection,
    records: &[AccountRecord],
) -> Result<u64, PersistenceError> {
    let rows = records
        .iter()
        .map(AccountRow::from_record)
        .collect::<Result<Vec<_>, _>>()?;
    // One replacement of the table at a time (LOADS-014, LOADS-040): this mode conflicts with
    // itself and with the ROW EXCLUSIVE lock of every row write, so a second load waits here
    // until this transaction ends, and its delete then removes the rows this one committed.
    // Readers take ACCESS SHARE and never wait.
    sqlx::query!("LOCK TABLE accounts IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *conn)
        .await?;
    sqlx::query!("DELETE FROM accounts")
        .execute(&mut *conn)
        .await?;
    let mut inserted: u64 = 0;
    for row in &rows {
        inserted = inserted.saturating_add(insert_row(&mut *conn, row).await?);
    }
    Ok(inserted)
}

impl AccountRepository for PgStore {
    fn find_with_customer(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = AccountCustomerChain> + Send {
        let acquire = self.pool().acquire();
        let probe = self.probe();
        async move {
            match acquire.await {
                // The three reads share one connection, each statement in autocommit.
                Ok(mut conn) => chain_in(&mut conn, probe, acct_id).await,
                Err(error) => AccountCustomerChain::ended_at_xref(ChainLink::Failed(error.into())),
            }
        }
    }

    fn find_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AccountRead, &[&id]);
        probed(verdict, select_by_id(self.pool(), id))
    }

    fn lock_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AccountLock, &[&id]);
        probed(verdict, select_for_update(self.pool(), id))
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<AccountRecord>, PersistenceError>> + Send {
        select_all(self.pool())
    }

    fn insert(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let pool = self.pool();
        async move {
            insert_one(pool, record).await?;
            Ok(())
        }
    }

    fn update(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AccountUpdate, &[record]);
        probed(verdict, update_one(self.pool(), record))
    }

    fn replace_all(
        &mut self,
        records: &[AccountRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // Dropping the transaction on an error rolls it back, so the previous contents
            // remain (LOADS-018).
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

impl AccountRepository for PgTx<'_> {
    fn find_with_customer(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = AccountCustomerChain> + Send {
        let (conn, probe) = self.conn_and_probe();
        chain_in(conn, probe, acct_id)
    }

    fn find_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AccountRead, &[&id]);
        probed(verdict, select_by_id(self.conn(), id))
    }

    fn lock_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AccountLock, &[&id]);
        probed(verdict, select_for_update(self.conn(), id))
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<AccountRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let conn = self.conn();
        async move {
            insert_one(conn, record).await?;
            Ok(())
        }
    }

    fn update(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        let verdict = self.verdict(ProbePoint::AccountUpdate, &[record]);
        probed(verdict, update_one(self.conn(), record))
    }

    fn replace_all(
        &mut self,
        records: &[AccountRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        replace_all_in(self.conn(), records)
    }
}

// A session is never probed (see "Probes" in the crate documentation).
impl AccountRepository for PgSession {
    fn find_with_customer(
        &mut self,
        acct_id: PicU<11>,
    ) -> impl Future<Output = AccountCustomerChain> + Send {
        chain_in(self.conn(), None, acct_id)
    }

    fn find_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
        select_by_id(self.conn(), id)
    }

    fn lock_by_id(
        &mut self,
        id: PicU<11>,
    ) -> impl Future<Output = Result<Option<AccountRecord>, PersistenceError>> + Send {
        select_for_update(self.conn(), id)
    }

    fn find_all(
        &mut self,
    ) -> impl Future<Output = Result<Vec<AccountRecord>, PersistenceError>> + Send {
        select_all(self.conn())
    }

    fn insert(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<(), PersistenceError>> + Send {
        let conn = self.conn();
        async move {
            insert_one(conn, record).await?;
            Ok(())
        }
    }

    fn update(
        &mut self,
        record: &AccountRecord,
    ) -> impl Future<Output = Result<bool, PersistenceError>> + Send {
        update_one(self.conn(), record)
    }

    fn replace_all(
        &mut self,
        records: &[AccountRecord],
    ) -> impl Future<Output = Result<u64, PersistenceError>> + Send {
        let begin = self.begin();
        async move {
            // Dropping the transaction on an error rolls it back, so the previous contents
            // remain (LOADS-018).
            let mut tx = begin.await?;
            let inserted = replace_all_in(tx.conn(), records).await?;
            tx.commit().await?;
            Ok(inserted)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;
    use std::str::FromStr;

    use copybook::framing::decode_all;
    use copybook::{FixedRecord, PicU, PicX};
    use domain::records::{AccountRecord, CardXrefRecord, CustomerRecord};
    use domain::sensitive::CardNumber;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use test_support::postgres::{PostgresGuard, start_postgres};

    use super::{AccountCustomerChain, AccountRepository, ChainLink, link, xref_link};
    use crate::{DecodeKind, PersistenceError, PgStore};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// SQLSTATE `lock_not_available`, raised when `lock_timeout` expires.
    const LOCK_NOT_AVAILABLE: &str = "55P03";

    /// The fixture account, customer and cross-reference keys that `cardxref.txt` line 1 joins.
    const LINKED: u64 = 50;

    /// An account key that no fixture record holds.
    const ABSENT_ACCOUNT: u64 = 99_999_999_999;

    /// A customer key that no fixture record holds.
    const ABSENT_CUSTOMER: u64 = 999_999_999;

    /// The records of `app/data/ASCII/<file>`, read in place and decoded by codec A.
    fn fixture<R: FixedRecord>(file: &str) -> Result<Vec<R>, Box<dyn StdError>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../app/data/ASCII")
            .join(file);
        let bytes = std::fs::read(path)?;
        Ok(decode_all::<R>(&bytes)?
            .into_iter()
            .map(|framed| framed.record)
            .collect())
    }

    /// The 50 fixture accounts, customers and cross-references, loaded through their
    /// repositories; returns the accounts as loaded.
    async fn load_fixtures(store: &mut PgStore) -> Result<Vec<AccountRecord>, Box<dyn StdError>> {
        let accounts = fixture::<AccountRecord>("acctdata.txt")?;
        // Called by path: the three traits share method names, and only this one is in scope.
        crate::CardXrefRepository::replace_all(store, &fixture::<CardXrefRecord>("cardxref.txt")?)
            .await?;
        crate::CustomerRepository::replace_all(store, &fixture::<CustomerRecord>("custdata.txt")?)
            .await?;
        assert_eq!(store.replace_all(&accounts).await?, 50);
        Ok(accounts)
    }

    /// An account with invented values keyed `id`: a 300-byte line in the shape of
    /// `acctdata.txt` whose balance is `balance` (overpunched units digit included), decoded by
    /// codec A.
    fn account(id: u64, balance: &str) -> Result<AccountRecord, Box<dyn StdError>> {
        let mut line = format!(
            "{id:011}Y{balance}00000020200{{00000010200{{2014-11-202025-05-20          \
             00000000123J00000000456BA000000000"
        );
        line.push_str(&format!("{:<188}", "~filler kept~"));
        Ok(AccountRecord::decode(line.as_bytes())?)
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
            filler: PicX::from_array(*b"              "),
        })
    }

    /// The key `id` as an `ACCT-ID PIC 9(11)`.
    fn key(id: u64) -> Result<PicU<11>, Box<dyn StdError>> {
        Ok(PicU::new(id)?)
    }

    /// `records` in key order, the order of the primary key.
    fn by_id(records: &[AccountRecord]) -> Vec<AccountRecord> {
        let mut sorted = records.to_vec();
        sorted.sort_by_key(|record| record.id);
        sorted
    }

    /// A store over a second pool to `guard`'s database whose sessions wait at most 200 ms for
    /// a row lock, so a held lock shows as SQLSTATE `55P03` instead of a wait.
    async fn impatient_store(guard: &PostgresGuard) -> Result<PgStore, Box<dyn StdError>> {
        let options =
            PgConnectOptions::from_str(guard.database_url())?.options([("lock_timeout", "200ms")]);
        let pool = PgPoolOptions::new()
            .max_connections(2)
            .connect_with(options)
            .await?;
        Ok(PgStore::new(pool))
    }

    /// Whether `result` is the failure of a lock that another transaction holds.
    fn is_lock_timeout<T>(result: &Result<T, PersistenceError>) -> bool {
        matches!(
            result,
            Err(PersistenceError::Database { sqlstate: Some(code), .. }) if code == LOCK_NOT_AVAILABLE
        )
    }

    /// Whether `chain` ended at its cross-reference read with `xref`.
    fn ended_at_xref(chain: &AccountCustomerChain, xref: &ChainLink<CardXrefRecord>) -> bool {
        chain.xref == *xref
            && chain.account == ChainLink::NotReached
            && chain.customer == ChainLink::NotReached
    }

    // Records are compared with `assert!`: their `Debug` already redacts the card number and the
    // SSN, and a failure message then names the property rather than the values.

    /// Each read result maps to its link, and the cross-reference link counts the rows sharing
    /// the account: none, exactly one, or the `DUPKEY` of two or more.
    // audit: COACTVWC-039
    // audit: COACTUPC-089
    #[test]
    fn reads_map_to_links_and_shared_alternate_keys_are_dupkey() -> TestResult {
        assert_eq!(link(Ok(Some(7_u8))), ChainLink::Found(7));
        assert_eq!(link::<u8>(Ok(None)), ChainLink::NotFound);
        assert_eq!(
            link::<u8>(Err(PersistenceError::Unavailable)),
            ChainLink::Failed(PersistenceError::Unavailable)
        );

        let first = xref(*b"9100000000000001", 1, 1)?;
        let second = xref(*b"9100000000000002", 2, 1)?;
        assert!(xref_link(Vec::new()) == ChainLink::NotFound);
        assert!(xref_link(vec![first.clone()]) == ChainLink::Found(first.clone()));
        assert!(
            xref_link(vec![first, second])
                == ChainLink::Failed(PersistenceError::Duplicate {
                    constraint: Some("card_xref_acct_id_idx".to_owned()),
                })
        );
        Ok(())
    }

    /// The chain over the loaded fixtures: all three found; no cross-reference, so nothing else
    /// read; a cross-reference without its account, whose customer is still read; an account
    /// whose customer is missing; and two, then three cross-references sharing the account
    /// (`DUPKEY`). A transaction runs the same chain, and the chain's `Debug` redacts the card
    /// number and SSN.
    // audit: COACTVWC-033
    // audit: COACTVWC-034
    // audit: COACTVWC-035
    // audit: COACTVWC-039
    // audit: COACTVWC-040
    // audit: COACTVWC-044
    // audit: COACTUPC-084
    // audit: COACTUPC-085
    // audit: COACTUPC-089
    // audit: COACTUPC-090
    #[tokio::test]
    async fn the_chain_reads_the_cross_reference_then_account_and_customer() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let accounts = load_fixtures(&mut store).await?;

        // All found: cardxref.txt line 1 joins account, customer and card.
        let linked = key(LINKED)?;
        let chain = store.find_with_customer(linked).await;
        let ChainLink::Found(found_xref) = &chain.xref else {
            return Err("the fixture cross-reference of account 50 is found".into());
        };
        assert!(found_xref.acct_id == linked);
        assert!(
            chain.account == ChainLink::Found(store.find_by_id(linked).await?.ok_or("account")?)
        );
        let ChainLink::Found(found_customer) = &chain.customer else {
            return Err("the fixture customer of account 50 is found".into());
        };
        assert!(found_customer.id == found_xref.cust_id);
        assert!(
            accounts
                .iter()
                .any(|a| chain.account == ChainLink::Found(a.clone()))
        );

        // Cardholder Data Redaction: the chain prints neither the card number nor the SSN.
        let printed = format!("{chain:?}");
        let card = std::str::from_utf8(found_xref.card_num.expose().as_bytes())?;
        let ssn = format!("{:09}", found_customer.ssn.expose().get());
        assert!(!printed.contains(card), "the card number is redacted");
        assert!(!printed.contains(&ssn), "the SSN is redacted");

        // No cross-reference: NOTFND, and neither later read is issued.
        let chain = store.find_with_customer(key(ABSENT_ACCOUNT)?).await;
        assert!(ended_at_xref(&chain, &ChainLink::NotFound));

        // A cross-reference without its account: the customer is read all the same.
        let orphan = 12_345_678_901;
        crate::CardXrefRepository::insert(&mut store, &xref(*b"9200000000000001", LINKED, orphan)?)
            .await?;
        let chain = store.find_with_customer(key(orphan)?).await;
        assert!(matches!(chain.xref, ChainLink::Found(_)));
        assert!(chain.account == ChainLink::NotFound);
        assert!(matches!(&chain.customer, ChainLink::Found(c) if c.id == PicU::new(LINKED)?));

        // An account whose cross-reference names a missing customer.
        let lonely = account(12_345_678_902, "00000001940{")?;
        store.insert(&lonely).await?;
        let missing_customer = xref(*b"9200000000000002", ABSENT_CUSTOMER, 12_345_678_902)?;
        crate::CardXrefRepository::insert(&mut store, &missing_customer).await?;
        let chain = store.find_with_customer(lonely.id).await;
        assert!(matches!(chain.xref, ChainLink::Found(_)));
        assert!(chain.account == ChainLink::Found(lonely.clone()));
        assert!(chain.customer == ChainLink::NotFound);

        // Two, then three cross-references share account 50: DUPKEY each time, nothing else
        // read, though the chain's read takes only the first two rows.
        let dupkey = ChainLink::Failed(PersistenceError::Duplicate {
            constraint: Some("card_xref_acct_id_idx".to_owned()),
        });
        for card in [*b"9200000000000050", *b"9200000000000051"] {
            crate::CardXrefRepository::insert(&mut store, &xref(card, LINKED, LINKED)?).await?;
            assert!(ended_at_xref(
                &store.find_with_customer(linked).await,
                &dupkey
            ));
        }

        // Inside a transaction the chain gives the same links, three rows sharing account 50.
        let mut tx = store.begin().await?;
        assert!(ended_at_xref(&tx.find_with_customer(linked).await, &dupkey));
        assert!(
            tx.find_with_customer(lonely.id).await == store.find_with_customer(lonely.id).await
        );
        assert!(ended_at_xref(
            &tx.find_with_customer(key(ABSENT_ACCOUNT)?).await,
            &ChainLink::NotFound
        ));
        tx.rollback().await?;
        Ok(())
    }

    /// A store whose pool is closed cannot acquire the chain's connection: the failure is
    /// reported on the cross-reference link, and nothing else is read.
    #[tokio::test]
    async fn a_connection_that_cannot_be_had_fails_the_cross_reference_link() -> TestResult {
        let options = PgConnectOptions::new()
            .host("127.0.0.1")
            .port(9)
            .username("carddemo")
            .database("carddemo");
        let mut store = PgStore::new(PgPoolOptions::new().connect_lazy_with(options));
        store.pool().close().await;
        let chain = store.find_with_customer(key(LINKED)?).await;
        assert!(ended_at_xref(
            &chain,
            &ChainLink::Failed(PersistenceError::Unavailable)
        ));
        Ok(())
    }

    /// The keyed read returns the stored record or `None`, a second insert of a key is a
    /// duplicate, and the rewrite reports whether the key exists.
    #[tokio::test]
    async fn keyed_reads_inserts_and_rewrites_report_the_cics_outcomes() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let stored = account(42, "00000001940}")?;

        store.insert(&stored).await?;
        assert!(store.find_by_id(key(42)?).await? == Some(stored.clone()));
        assert!(store.find_by_id(key(43)?).await?.is_none());
        assert_eq!(
            store.insert(&stored).await,
            Err(PersistenceError::Duplicate {
                constraint: Some("accounts_pkey".to_owned())
            })
        );

        let rewritten = account(42, "00000099999I")?;
        assert!(store.update(&rewritten).await?);
        assert!(store.find_by_id(key(42)?).await? == Some(rewritten.clone()));
        assert!(!store.update(&account(43, "00000000000{")?).await?);
        assert!(store.find_by_id(key(43)?).await?.is_none());
        assert!(store.find_all().await? == vec![rewritten]);
        Ok(())
    }

    /// On a transaction, `lock_by_id` holds the row lock until the commit, writes stay invisible
    /// to other sessions until then, a rollback undoes them, and a savepoint lets the
    /// transaction go on after a duplicate insert. On the pool the lock ends with the call.
    // audit: COACTUPC-101
    // audit: COACTUPC-103
    #[tokio::test]
    async fn transactions_hold_row_locks_and_scope_their_writes() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let mut impatient = impatient_store(&guard).await?;
        let original = account(42, "00000001940{")?;
        let changed = account(42, "00000000000{")?;
        let second = account(43, "00000000500{")?;
        store.insert(&original).await?;

        let mut tx = store.begin().await?;
        assert!(tx.lock_by_id(key(42)?).await? == Some(original.clone()));
        assert!(tx.lock_by_id(key(44)?).await?.is_none());
        let mut waiting = impatient.begin().await?;
        assert!(is_lock_timeout(&waiting.lock_by_id(key(42)?).await));
        drop(waiting);
        assert!(is_lock_timeout(&impatient.update(&changed).await));

        assert!(tx.update(&changed).await?);
        tx.insert(&second).await?;
        assert!(tx.find_by_id(key(42)?).await? == Some(changed.clone()));
        assert!(tx.find_all().await? == vec![changed.clone(), second.clone()]);
        assert!(store.find_by_id(key(42)?).await? == Some(original));
        assert!(store.find_by_id(key(43)?).await?.is_none());
        tx.commit().await?;

        let mut after_commit = impatient.begin().await?;
        assert!(after_commit.lock_by_id(key(42)?).await? == Some(changed.clone()));
        after_commit.rollback().await?;

        assert!(store.lock_by_id(key(42)?).await? == Some(changed.clone()));
        let mut after_pool_lock = impatient.begin().await?;
        assert!(after_pool_lock.lock_by_id(key(42)?).await? == Some(changed.clone()));
        after_pool_lock.commit().await?;

        let mut undone = store.begin().await?;
        assert!(undone.update(&account(42, "00000077777{")?).await?);
        undone.rollback().await?;
        assert!(store.find_by_id(key(42)?).await? == Some(changed));

        let mut goes_on = store.begin().await?;
        let mut savepoint = goes_on.savepoint().await?;
        assert!(
            savepoint
                .insert(&second)
                .await
                .is_err_and(|e| e.is_duplicate())
        );
        savepoint.rollback().await?;
        assert!(goes_on.find_by_id(key(43)?).await? == Some(second));
        goes_on.commit().await?;
        Ok(())
    }

    /// `replace_all` loads the 50 fixture accounts with the same contents twice, accepts an
    /// empty table, keeps the previous contents when an insert fails, and on a transaction runs
    /// inside the caller's unit of work.
    // audit: LOADS-001
    // audit: LOADS-014
    // audit: LOADS-015
    // audit: LOADS-018
    #[tokio::test]
    async fn replace_all_reloads_the_table_as_one_unit() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let fixtures = fixture::<AccountRecord>("acctdata.txt")?;
        assert_eq!(fixtures.len(), 50);

        assert_eq!(store.replace_all(&[]).await?, 0);
        assert!(store.find_all().await?.is_empty());
        store.insert(&account(7, "00000000000{")?).await?;

        for _ in 0..2 {
            assert_eq!(store.replace_all(&fixtures).await?, 50);
            assert!(
                store.find_all().await? == by_id(&fixtures),
                "exactly the fixture accounts, in key order"
            );
        }
        for record in &fixtures {
            assert!(store.find_by_id(record.id).await?.as_ref() == Some(record));
        }

        let clashing = [account(60, "00000000000{")?, account(60, "00000000001A")?];
        assert!(
            store
                .replace_all(&clashing)
                .await
                .is_err_and(|e| e.is_duplicate())
        );
        assert!(store.find_all().await? == by_id(&fixtures));

        let inside = account(61, "00000000000{")?;
        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(std::slice::from_ref(&inside)).await?, 1);
        assert!(tx.find_all().await? == vec![inside.clone()]);
        tx.rollback().await?;
        assert!(store.find_all().await? == by_id(&fixtures));

        let mut tx = store.begin().await?;
        assert_eq!(tx.replace_all(std::slice::from_ref(&inside)).await?, 1);
        tx.commit().await?;
        assert!(store.find_all().await? == vec![inside]);
        Ok(())
    }

    /// A record codec B cannot store, after one it can, fails the replacement before the delete:
    /// through the pool nothing changes, and a caller's transaction is left usable and unchanged,
    /// so committing it after the error keeps the previous contents.
    // audit: LOADS-018
    #[tokio::test]
    async fn an_unstorable_record_fails_the_replacement_before_the_delete() -> TestResult {
        let guard = start_postgres().await?;
        let mut store = PgStore::new(guard.pool().clone());
        let original = [account(42, "00000001940{")?, account(43, "00000000500{")?];
        assert_eq!(store.replace_all(&original).await?, 2);
        let unstorable = AccountRecord {
            filler: PicX::moved_from(b"\0"),
            ..account(45, "00000000000{")?
        };
        let input = [account(44, "00000000000{")?, unstorable];
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
    }
}
