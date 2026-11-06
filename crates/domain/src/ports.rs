//! Repository ports: the record access the ported rules perform, declared as
//! traits that the persistence adapters implement (AAP 0.5.3.3, R3).
//!
//! Each trait stands for one VSAM file, Db2 table or IMS database of the COBOL
//! oracle, and each method for one access verb a carried program issues on it:
//! CICS `READ`, `READ UPDATE`, `STARTBR`/`READNEXT`/`READPREV`, `WRITE`,
//! `REWRITE`, `DELETE`; Db2 `SELECT`, cursor `FETCH`, `INSERT`, `UPDATE`,
//! `DELETE`; DL/I `GU`, `GN`, `GNP`, `ISRT`, `REPL`, `DLET`. Section B of
//! `docs/parity/parity-oracle-audit.md` records these accesses per program; the
//! method docs name the verb and the programs that issue it.
//!
//! # Signature form
//!
//! Every method is declared as
//! `fn name(&mut self, arg: T) -> impl Future<Output = PortResult<R>> + Send + '_`:
//!
//! - Arguments are passed by value (owned records or `Copy` keys), so the future
//!   borrows only `self`.
//! - The explicit `Send` is the guarantee the adapters need on the multi-threaded
//!   tokio runtime of the API, batch and messaging binaries.
//! - The methods are not declared `async fn`: an `async fn` in a public trait
//!   raises the `async_fn_in_trait` lint, and the build allows no warning and no
//!   suppression. An adapter still writes each method as `async fn` in its
//!   `impl`, which the compiler checks against these bounds.
//!
//! # Dispatch
//!
//! Rules take ports through generics, for example
//! `fn update<A: AccountReader + AccountWriter>(store: &mut A, …)`. A trait whose
//! methods return `impl Future` is not object-safe, so no port is used as a trait
//! object. This module needs nothing beyond `std::future::Future`.
//!
//! The traits have no `Send` supertrait. A generic `async fn` rule is `Send`
//! wherever its concrete store is; a rule that declares its own returned future
//! `Send` also bounds the store `+ Send`, because that future holds the
//! `&mut` borrow of it.
//!
//! # Writes
//!
//! Every write method is one legacy write, and the adapter runs it under its own
//! savepoint. A failed write therefore leaves the writes before it in place,
//! which is what the COBOL does: every `CardDemo` VSAM file is `RECOVERY(NONE)`
//! [app/csd/CARDDEMO.CSD:L1-96], so `COACTUPC`'s account rewrite survives a
//! failed customer rewrite (P-COACTUPC-014) and `COBIL00C` rewrites the account
//! after a failed transaction write (E-19). The adapter's caller owns the
//! database transaction and commits it (AAP 0.6.4); no port method commits.
//!
//! # Results
//!
//! A read of a missing key is `Err(PortError::NotFound)`, the VSAM `NOTFND`,
//! SQLCODE +100 or IMS `GE` the rules branch on. A browse, a sweep or a lookup
//! that may legitimately find nothing returns an empty `Vec` or `Ok(None)`
//! instead, as its method documents.
//!
//! Any call, a plain read included, can lose a lock conflict with another unit
//! of work: a lock timeout, a deadlock or a serialization failure, the Db2
//! `SQLCODE` -911. It then fails with [`PortError::Contention`], which every
//! method's errors name. [`PortError::Unavailable`] is every failure a method
//! does not name otherwise.
//!
//! Two read shapes cover the sequential accesses:
//!
//! - an online browse (CICS `STARTBR` with `READNEXT`/`READPREV`, a Db2 cursor,
//!   an IMS `GNP` page) takes a [`Browse`]: a start key, a [`Direction`] and a row
//!   limit, and returns the rows in COBOL read order;
//! - a batch sweep (a sequential `READ NEXT` of a KSDS, an IMS `GN`) takes the
//!   last key already processed and a limit, and returns the rows after it in
//!   key order, so a job pages through the whole file.

use std::future::Future;

use crate::entities::{
    Account, AuthFraud, Card, CardXref, Customer, DisclosureGroup, DisclosureGroupKey,
    NewPendingAuthDetail, NewTransaction, PendingAuthDetail, PendingAuthSummary, TranCatBalance,
    TranCatBalanceKey, TranCategoryRecord, TranTypeMaster, TranTypeRecord, Transaction,
    UserProfile, UserSecurity,
};
use crate::numeric::{
    AccountId, Alnum, AuthId, CardNumber, CustomerId, TranCategoryCode, TranId, TranTypeCode,
    TransactionRowId, UserId,
};

/// The check a port writer applies to an instant before storing it: a stored
/// instant is a whole microsecond of UTC time. Re-exported from
/// [`crate::clock`] for the persistence adapters that implement these ports.
pub use crate::clock::is_whole_microsecond;

/// Why a port call failed, in the terms the COBOL programs test.
///
/// Each variant is the outcome a program branches on, whatever store answered:
/// a VSAM file status or CICS `RESP`, a Db2 `SQLCODE`, or an IMS status code.
/// Adapters map their database errors onto these variants, and the rules choose
/// the COBOL message and the error kind from them.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PortError {
    /// The key names no record: CICS `NOTFND` (VSAM file status 23), Db2
    /// `SQLCODE` +100, or IMS status `GE`. An `UPDATE` or `DELETE` that touches
    /// no row reports it too.
    #[error("record not found")]
    NotFound,
    /// A record with the key already exists: CICS `DUPREC` or `DUPKEY` (VSAM
    /// file status 22), Db2 `SQLCODE` -803, or IMS status `II`.
    #[error("duplicate record key")]
    Duplicate,
    /// A delete is blocked by dependent rows: Db2 `SQLCODE` -532, the
    /// `ON DELETE RESTRICT` of the categories that reference a transaction type
    /// (`COTRTLIC` [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1914],
    /// `COTRTUPC` [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1638]). A
    /// write whose row references a row that does not exist, such as an
    /// accrual of a missing transaction, reports it too, unless the method
    /// names another outcome for it, as [`AuthorizationStore::insert_detail`]
    /// does with [`PortError::NotFound`].
    #[error("delete restricted by dependent records")]
    Restricted,
    /// The row is locked by another unit of work: Db2 `SQLCODE` -911, deadlock
    /// or lock timeout (`COTRTLIC`
    /// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1870], `COTRTUPC`
    /// [app/app-transaction-type-db2/cbl/COTRTUPC.cbl:L1561]). A unit of work
    /// that loses a serialization conflict with another reports it too.
    #[error("record locked by another unit of work")]
    Contention,
    /// Any other I/O failure, the `WHEN OTHER` branch of the COBOL.
    ///
    /// The text is adapter detail for logs only. It never reaches a response
    /// body: a response carries the COBOL message the rule chooses, or the
    /// generic unexpected-error text.
    #[error("store unavailable: {0}")]
    Unavailable(String),
}

impl PortError {
    /// The Db2 `SQLCODE` this outcome stands for, as the Db2 programs format it
    /// into their messages: +100, -803, -532 or -911.
    ///
    /// `None` for [`PortError::Unavailable`], whose code the COBOL takes from the
    /// failed statement itself; the audit row of each message decides what the
    /// replacement shows there.
    #[must_use]
    pub const fn sqlcode(&self) -> Option<i32> {
        match self {
            Self::NotFound => Some(100),
            Self::Duplicate => Some(-803),
            Self::Restricted => Some(-532),
            Self::Contention => Some(-911),
            Self::Unavailable(_) => None,
        }
    }
}

/// The result of a port call.
pub type PortResult<T> = Result<T, PortError>;

/// The direction of an online browse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    /// Ascending key order: PF8, CICS `READNEXT`, a Db2 cursor ordered
    /// ascending, IMS `GNP` in twin order.
    #[default]
    Forward,
    /// Descending key order: PF7, CICS `READPREV`, a Db2 cursor ordered
    /// descending.
    Backward,
}

/// One page of an online browse: CICS `STARTBR` followed by up to `limit`
/// `READNEXT` or `READPREV` calls, or the equivalent Db2 cursor or IMS `GNP`
/// loop.
///
/// The rows come back in read order:
///
/// - Forward ([`Direction::Forward`]): keys greater than or equal to
///   `start_key`, ascending. `STARTBR` positions `GTEQ`, the CICS default the
///   browsing programs use (their explicit `GTEQ` is commented out,
///   P-COTRN00C-006, P-COUSR00C-009).
/// - Backward ([`Direction::Backward`]): keys less than or equal to
///   `start_key`, descending, as `READPREV` after a `STARTBR` on an existing key
///   returns that record first.
///
/// `None` starts at the beginning of the file for a forward browse (`LOW-VALUES`)
/// and at its end for a backward one (`HIGH-VALUES`). A blank COBOL key is
/// passed as `Some` key of spaces, never as `None`.
///
/// The start key is included because the COBOL includes it: on PF8 and PF7 the
/// programs restart at the last or first key shown and discard that record
/// themselves (P-COTRN00C-006, P-COTRN00C-007, P-COUSR00C-003), which the rules
/// reproduce. A method whose COBOL access bounds the start key differently says
/// so in its own documentation. `limit` is the most rows returned; a rule asks
/// for one row beyond its page to learn whether a next page exists, as the
/// programs' look-ahead read does.
///
/// `crates/api/src/pagination.rs` builds this from the request's `start_key`,
/// `direction` and `page_size`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browse<K> {
    /// The key the browse starts at, or `None` for the start or end of the file.
    pub start_key: Option<K>,
    /// Ascending (PF8) or descending (PF7).
    pub direction: Direction,
    /// The most rows returned.
    pub limit: u16,
}

/// The record filter of the `COCRDLIC` card list: `9500-FILTER-RECORDS`
/// [app/cbl/COCRDLIC.cbl:L1382-1409].
///
/// A card is listed when it matches every filter given. Records the filter
/// excludes are skipped, not counted against the browse limit.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardFilter {
    /// Only cards of this account (`CARD-ACCT-ID`), when given.
    pub account_id: Option<AccountId>,
    /// Only this card number (`CARD-NUM`), when given.
    pub card_number: Option<CardNumber>,
}

/// The filters of the `COTRTLIC` transaction type list, the conditions of its
/// cursors and of its `9100-CHECK-FILTERS` count
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L338-368]
/// [app/app-transaction-type-db2/cbl/COTRTLIC.cbl:L1803-1815] (P-COTRTLIC-003).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TranTypeFilter {
    /// `TR_TYPE = :WS-TYPE-CD-FILTER`, when given.
    pub type_code: Option<TranTypeCode>,
    /// `TR_DESCRIPTION LIKE :pattern`, when given.
    ///
    /// The value is the complete pattern the rule builds from the entered
    /// filter (`%`, the trimmed input, `%`, P-COTRTLIC-020). The adapter binds it
    /// unchanged: `%` and `_` are wildcards, there is no escape character, and
    /// the match is case-sensitive, as in Db2.
    pub description: Option<String>,
}

/// Reads of the account master: VSAM `ACCTDAT` (`CVACT01Y`), table `accounts`.
///
/// Used by `COACTVWC`, `COACTUPC`, `COBIL00C`, `CBTRN02C`, `CBACT04C`,
/// `CBSTM03A` (through `CBSTM03B`), `COPAUA0C`, `COPAUS0C` and `COACCT01`.
pub trait AccountReader {
    /// CICS `READ DATASET('ACCTDAT')` by `ACCT-ID`, or a random `READ` of the
    /// batch account file: `COACTVWC` `9300-GETACCTDATA-BYACCT` (P-COACTVWC-009),
    /// `CBTRN02C` `1500-B-LOOKUP-ACCT` (P-CBTRN02C-009), `CBACT04C`
    /// `1100-GET-ACCT-DATA` (P-CBACT04C-002), `CBSTM03A` `3000-ACCTFILE-GET`
    /// (P-CBSTM03A-1259), `COPAUA0C` `5200-READ-ACCT-RECORD` (P-COPAUA0C-007).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no account has the ID (`NOTFND`, status 23);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_;

    /// CICS `READ … UPDATE` of `ACCTDAT`, SQL `SELECT … FOR UPDATE`: the account
    /// locked until the caller's transaction ends, as `COACTUPC`
    /// `9600-WRITE-PROCESSING` (P-COACTUPC-010) reads it before its change check
    /// and rewrite, and `COBIL00C` `READ-ACCTDAT-FILE` (P-COBIL00C-005,
    /// P-COBIL00C-009) before the payment.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no account has the ID;
    /// [`PortError::Contention`] when the lock cannot be taken;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_account_for_update(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_;
}

/// Writes of the account master `ACCTDAT`, table `accounts`.
///
/// Used by `COACTUPC`, `COBIL00C`, `CBTRN02C` and `CBACT04C`.
pub trait AccountWriter {
    /// CICS or batch `REWRITE` of the account record, keyed by its `acct_id`:
    /// `COACTUPC` `9600-WRITE-PROCESSING` (P-COACTUPC-013), `COBIL00C`
    /// `UPDATE-ACCTDAT-FILE` (P-COBIL00C-019), `CBTRN02C`
    /// `2800-UPDATE-ACCOUNT-REC` (P-CBTRN02C-018), `CBACT04C`
    /// `1050-UPDATE-ACCOUNT` (P-CBACT04C-010). One legacy write, run under its
    /// own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no account has the ID (`REWRITE` `INVALID
    /// KEY`); [`PortError::Contention`] on a lock conflict with another unit of
    /// work; [`PortError::Unavailable`] on any other failure.
    fn rewrite_account(
        &mut self,
        account: Account,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;
}

/// Reads of the card master: VSAM `CARDDAT` (`CVACT02Y`) and its alternate index
/// `CARDAIX` on `CARD-ACCT-ID`, table `cards`.
///
/// Used by `COCRDLIC`, `COCRDSLC` and `COCRDUPC`.
pub trait CardReader {
    /// CICS `READ` of `CARDDAT` by card number alone: `COCRDSLC` and `COCRDUPC`
    /// `9100-GETCARD-BYACCTCARD` (P-COCRDUPC-023).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no card has the number;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_card(&mut self, card: CardNumber)
    -> impl Future<Output = PortResult<Card>> + Send + '_;

    /// CICS `READ … UPDATE` of `CARDDAT`, SQL `SELECT … FOR UPDATE`: `COCRDUPC`
    /// `9200-WRITE-PROCESSING` locks the card before its change check and
    /// rewrite (P-COCRDUPC-004).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no card has the number;
    /// [`PortError::Contention`] when the lock cannot be taken;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_card_for_update(
        &mut self,
        card: CardNumber,
    ) -> impl Future<Output = PortResult<Card>> + Send + '_;

    /// The `COCRDLIC` card list: `STARTBR` on `CARD-NUM` with `READNEXT`
    /// (`9000-READ-FORWARD`, P-COCRDLIC-003) or `READPREV`
    /// (`9100-READ-BACKWARDS`, P-COCRDLIC-013), keeping only the cards that pass
    /// `filter`.
    ///
    /// Rows follow the [`Browse`] contract in card-number order. Cards the
    /// filter excludes are skipped and do not count against `browse.limit`. The
    /// unfiltered look-ahead read `COCRDLIC` issues when a page is full is a
    /// second browse with [`CardFilter::default`].
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; an empty page is `Ok`
    /// with no rows.
    fn browse_cards(
        &mut self,
        browse: Browse<CardNumber>,
        filter: CardFilter,
    ) -> impl Future<Output = PortResult<Vec<Card>>> + Send + '_;

    /// The cards of one account through the `CARDAIX` alternate path
    /// (`KEYS(11 16) NONUNIQUEKEY`), in card-number order, the order of the
    /// alternate index's base keys: the first is the record a CICS `READ` on the
    /// path returns.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; an account with no
    /// card is `Ok` with no rows.
    fn cards_for_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Vec<Card>>> + Send + '_;
}

/// Writes of the card master `CARDDAT`, table `cards`. Used by `COCRDUPC`.
pub trait CardWriter {
    /// CICS `REWRITE` of the card record, keyed by its `card_num`: `COCRDUPC`
    /// `9200-WRITE-PROCESSING` (P-COCRDUPC-006). One legacy write, run under its
    /// own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no card has the number;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn rewrite_card(&mut self, card: Card) -> impl Future<Output = PortResult<()>> + Send + '_;
}

/// Reads of the customer master: VSAM `CUSTDAT` (`CVCUS01Y`), table `customers`.
///
/// Used by `COACTVWC`, `COACTUPC`, `CBSTM03A` (through `CBSTM03B`), `COPAUA0C`
/// and `COPAUS0C`.
pub trait CustomerReader {
    /// CICS `READ DATASET('CUSTDAT')` by `CUST-ID`, or a random `READ` of the
    /// batch customer file: `COACTVWC` `9400-GETCUSTDATA-BYCUST`
    /// (P-COACTVWC-010), `CBSTM03A` `2000-CUSTFILE-GET` (P-CBSTM03A-1258),
    /// `COPAUA0C` `5300-READ-CUST-RECORD` (P-COPAUA0C-007).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no customer has the ID;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_customer(
        &mut self,
        id: CustomerId,
    ) -> impl Future<Output = PortResult<Customer>> + Send + '_;

    /// CICS `READ … UPDATE` of `CUSTDAT`, SQL `SELECT … FOR UPDATE`: `COACTUPC`
    /// `9600-WRITE-PROCESSING` locks the customer after the account
    /// (P-COACTUPC-011).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no customer has the ID;
    /// [`PortError::Contention`] when the lock cannot be taken;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_customer_for_update(
        &mut self,
        id: CustomerId,
    ) -> impl Future<Output = PortResult<Customer>> + Send + '_;
}

/// Writes of the customer master `CUSTDAT`, table `customers`. Used by
/// `COACTUPC`.
pub trait CustomerWriter {
    /// CICS `REWRITE` of the customer record, keyed by its `cust_id`: the second
    /// write of `COACTUPC` `9600-WRITE-PROCESSING` (P-COACTUPC-014). One legacy
    /// write under its own savepoint, so a failure here leaves the account
    /// rewrite before it in place, as `RECOVERY(NONE)` does.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no customer has the ID;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn rewrite_customer(
        &mut self,
        customer: Customer,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;
}

/// Reads of the card cross-reference: VSAM `CCXREF` (`CVACT03Y`) keyed by card
/// number and its alternate index `CXACAIX` on `XREF-ACCT-ID`, table
/// `card_xref`.
///
/// Used by `COACTVWC`, `COACTUPC`, `COTRN02C`, `COBIL00C`, `CBTRN02C`,
/// `CBACT04C`, `CBTRN03C`, `CBSTM03A`, `COPAUA0C` and `COPAUS0C`. The file is
/// read-only at run time.
pub trait XrefReader {
    /// `READ` of `CCXREF` by card number: `CBTRN02C` `1500-A-LOOKUP-XREF`
    /// (P-CBTRN02C-008), `CBTRN03C` `1500-A-LOOKUP-XREF` (P-CBTRN03C-005),
    /// `COTRN02C` `READ-CCXREF-FILE` (P-COTRN02C-009), `COPAUA0C`
    /// `5100-READ-XREF-RECORD` (P-COPAUA0C-006).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no cross-reference has the card number;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_xref_by_card(
        &mut self,
        card: CardNumber,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_;

    /// `READ` of `CXACAIX` by account (`KEYS(11,25) NONUNIQUEKEY`): the first
    /// cross-reference of the account in card-number order, the record a CICS
    /// `READ … RIDFLD` on the alternate path returns. `COACTVWC`
    /// `9200-GETCARDXREF-BYACCT` (P-COACTVWC-007) and its `COACTUPC`
    /// counterpart, `COBIL00C` `READ-CXACAIX-FILE` (P-COBIL00C-011), `COTRN02C`
    /// `READ-CXACAIX-FILE` (P-COTRN02C-008), `CBACT04C` `1110-GET-XREF-DATA`
    /// (P-CBACT04C-002), `COPAUS0C` `GETCARDXREF-BYACCT` (P-COPAUS0C-007).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when the account has no cross-reference;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_xref_by_account(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<CardXref>> + Send + '_;

    /// Sequential `READ` of `CCXREF` in card-number order: `CBSTM03B`
    /// `2000-XREFFILE-PROC` (P-CBSTM03B-003) driving `CBSTM03A`
    /// `1000-XREFFILE-GET-NEXT` (P-CBSTM03A-1257). Up to `limit` records whose
    /// card number is greater than `after`, from the first record when `after`
    /// is `None`.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; the end of the file is
    /// `Ok` with no rows.
    fn xrefs_after(
        &mut self,
        after: Option<CardNumber>,
        limit: u16,
    ) -> impl Future<Output = PortResult<Vec<CardXref>>> + Send + '_;
}

/// Reads of the live transaction master: VSAM `TRANSACT` (`CVTRA05Y`), the view
/// `transaction_master` over the table `transactions`.
///
/// Every read sees only the rows that form the live KSDS. Rows `batch tranbkp`
/// took out of the master stay as targets of their reward accruals and are
/// never returned here (E-09). Used by `COTRN00C`, `COTRN01C`, `COTRN02C`,
/// `COBIL00C`, `CREASTMT` (`CBSTM03A`) and `TRANREPT` (`CBTRN03C`).
pub trait TransactionReader {
    /// CICS `READ` of `TRANSACT` by TRAN-ID: `COTRN01C` `READ-TRANSACT-FILE`
    /// (P-COTRN01C-004). The COBOL reads with `UPDATE` and rewrites nothing, so
    /// a plain read gives the same result.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no live transaction has the ID;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_transaction(
        &mut self,
        id: TranId,
    ) -> impl Future<Output = PortResult<Transaction>> + Send + '_;

    /// The `COTRN00C` transaction list: `STARTBR` on TRAN-ID with `READNEXT`
    /// (`PROCESS-PAGE-FORWARD`, P-COTRN00C-006) or `READPREV`
    /// (`PROCESS-PAGE-BACKWARD`, P-COTRN00C-007), rows per the [`Browse`]
    /// contract in TRAN-ID byte order.
    ///
    /// `account_id` is the optional filter the REST resource adds (E-17): when
    /// given, only transactions whose card the cross-reference `CCXREF` maps to
    /// that account are returned, and the others do not count against
    /// `browse.limit`. `COTRN00C` itself browses every transaction, so parity is
    /// measured with `None`.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; an empty page, the
    /// `STARTBR` `NOTFND` of an empty file or of a start key above the highest
    /// key (P-COTRN00C-011), is `Ok` with no rows.
    fn browse_transactions(
        &mut self,
        browse: Browse<TranId>,
        account_id: Option<AccountId>,
    ) -> impl Future<Output = PortResult<Vec<Transaction>>> + Send + '_;

    /// The transaction with the highest TRAN-ID: `STARTBR` at `HIGH-VALUES`
    /// then one `READPREV`, as `COTRN02C` `ADD-TRANSACTION` (P-COTRN02C-020) and
    /// `COPY-LAST-TRAN-DATA` (PF5, P-COTRN02C-026) and `COBIL00C`
    /// `PROCESS-ENTER-KEY` (P-COBIL00C-012) read it.
    ///
    /// `None` when the live master is empty, the `STARTBR` `NOTFND` path of
    /// P-COTRN02C-021 and P-COBIL00C-013.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn highest_transaction(
        &mut self,
    ) -> impl Future<Output = PortResult<Option<Transaction>>> + Send + '_;

    /// The first `limit` live transactions in the `CREASTMT` statement order:
    /// `STEP010` `SORT FIELDS=(263,16,CH,A,1,16,CH,A)`, card number then
    /// TRAN-ID, both in byte order (P-CREASTMT-002), read sequentially by
    /// `CBSTM03B` `1000-TRNXFILE-PROC` (P-CBSTM03B-002). Fewer when the master
    /// holds fewer.
    ///
    /// `CBSTM03A` loads the file into tables of fixed size (P-CBSTM03A-004), so
    /// `CREASTMT` reads only the records that decide the load, the bound
    /// [`crate::rules::statement::TRNXFILE_READ_LIMIT`] names.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; an empty master is `Ok`
    /// with no rows.
    fn transactions_by_card(
        &mut self,
        limit: u16,
    ) -> impl Future<Output = PortResult<Vec<Transaction>>> + Send + '_;

    /// One page of the `TRANREPT` selection that serves the CR00 report: the
    /// live transactions whose processing date, the first 10 bytes of
    /// `TRAN-PROC-TS`, lies from `start` to `end` inclusive, compared as `CH`
    /// byte strings, the `STEP05R` `INCLUDE COND` (P-TRANREPT-002), read as
    /// `CBTRN03C` `1000-TRANFILE-GET-NEXT` reads the sorted file
    /// (P-CBTRN03C-474).
    ///
    /// Rows come back in the `STEP05R` order the report rule sorts by: card
    /// number, then TRAN-ID within a card (the order E-38 decides), each
    /// compared as its space-padded `X(16)` bytes, the order of the
    /// `(CardNumber, TranId)` pair. Only rows whose pair is greater than
    /// `after` are returned, from the first row when `after` is `None`, and at
    /// most `limit` of them; an empty page ends the selection. The rule
    /// (`transaction_report::ReportSelection`) checks the order and applies
    /// the exact date compare again, so a caller pages through the whole
    /// selection with the last pair of each page as the next `after`.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; the end of the
    /// selection is `Ok` with no rows.
    fn transactions_by_proc_date(
        &mut self,
        start: Alnum<10>,
        end: Alnum<10>,
        after: Option<(CardNumber, TranId)>,
        limit: u16,
    ) -> impl Future<Output = PortResult<Vec<Transaction>>> + Send + '_;
}

/// Writes of the live transaction master `TRANSACT`, table `transactions`.
///
/// Used by `COTRN02C`, `COBIL00C` and `CBTRN02C`. The next TRAN-ID is allocated
/// by `crates/persistence/src/ids.rs`, which serializes the highest-key-plus-one
/// rule; this port only writes the record it is given.
pub trait TransactionWriter {
    /// CICS or batch `WRITE` of a new transaction into the live master:
    /// `COTRN02C` `WRITE-TRANSACT-FILE`, `COBIL00C` `WRITE-TRANSACT-FILE`
    /// (P-COBIL00C-016, P-COBIL00C-017), `CBTRN02C`
    /// `2900-WRITE-TRANSACTION-FILE` (P-CBTRN02C-019). One legacy write under
    /// its own savepoint; it returns the row identity the reward accrual of the
    /// transaction references.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when a live transaction already has the TRAN-ID
    /// (`DUPKEY`, `DUPREC`); [`PortError::Contention`] on a lock conflict with
    /// another unit of work; [`PortError::Unavailable`] on any other failure.
    fn write_transaction(
        &mut self,
        tran: NewTransaction,
    ) -> impl Future<Output = PortResult<TransactionRowId>> + Send + '_;
}

/// The transaction category balances: VSAM `TCATBALF` (`CVTRA01Y`), keyed by
/// account, type code and category code, table `transaction_category_balances`.
///
/// Used by `CBTRN02C` (read, write, rewrite) and `CBACT04C` (sequential read).
pub trait TranCatBalanceStore {
    /// Random `READ` of `TCATBALF` by key: `CBTRN02C` `2700-UPDATE-TCATBAL`
    /// (P-CBTRN02C-015), whose status 23 creates the record.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no balance has the key (status 23);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_tcatbal(
        &mut self,
        key: TranCatBalanceKey,
    ) -> impl Future<Output = PortResult<TranCatBalance>> + Send + '_;

    /// `WRITE` of a new balance record: `CBTRN02C` `2700-A-CREATE-TCATBAL-REC`
    /// (P-CBTRN02C-016). One legacy write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when a balance already has the key;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn write_tcatbal(
        &mut self,
        rec: TranCatBalance,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// `REWRITE` of an existing balance record: `CBTRN02C`
    /// `2700-B-UPDATE-TCATBAL-REC` (P-CBTRN02C-017). One legacy write under its
    /// own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no balance has the key;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn rewrite_tcatbal(
        &mut self,
        rec: TranCatBalance,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// Sequential `READ` of `TCATBALF` in key order: `CBACT04C`
    /// `1000-TCATBALF-GET-NEXT` (P-CBACT04C-182). Up to `limit` records whose
    /// key is greater than `after`, from the first record when `after` is
    /// `None`. Key order is the order of [`TranCatBalanceKey`]: account, then
    /// type code in byte order, then category.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; the end of the file is
    /// `Ok` with no rows.
    fn tcatbals_after(
        &mut self,
        after: Option<TranCatBalanceKey>,
        limit: u16,
    ) -> impl Future<Output = PortResult<Vec<TranCatBalance>>> + Send + '_;
}

/// Reads of the disclosure groups, the interest rates: VSAM `DISCGRP`
/// (`CVTRA02Y`), table `disclosure_groups`. Used by `CBACT04C`.
pub trait DisclosureGroupReader {
    /// Random `READ` of `DISCGRP` by group, type code and category code:
    /// `CBACT04C` `1200-GET-INTEREST-RATE` (P-CBACT04C-004) and
    /// `1200-A-GET-DEFAULT-INT-RATE` (P-CBACT04C-005). The fallback to the
    /// `DEFAULT` group on status 23 is rule logic.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no group has the key (status 23);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_disclosure_group(
        &mut self,
        key: DisclosureGroupKey,
    ) -> impl Future<Output = PortResult<DisclosureGroup>> + Send + '_;
}

/// The step of a Db2 cursor read at which it failed: the `OPEN`, a `FETCH`
/// or the `CLOSE` of the cursor a [`TranTypeMasterStore::browse_tran_types`]
/// call stands for, judged by how many rows the read had returned.
///
/// The adapter reads the cursor's rows as one statement whose rows arrive one
/// at a time, as the `FETCH` loop of the program receives them:
///
/// - the statement is planned, takes its locks and produces its first row
///   before it returns any, so a failure before the first row is the
///   `OPEN`'s, the step a Db2 cursor whose order needs a sort also evaluates
///   its rows in;
/// - a failure after some rows, fewer than the browse's limit, is the
///   `FETCH` of the row after them, the one that would answer `SQLCODE`
///   +100 when they are all the rows there are;
/// - a failure once every row the browse asked for has arrived, as the
///   statement completes, is the `CLOSE`'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPhase {
    /// `OPEN CURSOR`: no row was returned.
    Open,
    /// `FETCH`: the row after [`CursorError::fetched`] could not be fetched.
    Fetch,
    /// `CLOSE CURSOR`: every row the browse asked for was returned.
    Close,
}

/// A failed cursor read: the step it failed at, the rows it had returned
/// before the failure, in fetch order, and the outcome the program branches
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorError<T> {
    /// The cursor step that failed.
    pub phase: CursorPhase,
    /// The rows fetched before the failure, in fetch order; none for an
    /// `OPEN` failure.
    pub fetched: Vec<T>,
    /// Why the step failed.
    pub error: PortError,
}

impl<T> CursorError<T> {
    /// The failure `error` of a cursor read of at most `limit` rows that had
    /// returned `fetched` when it failed: [`CursorPhase::Open`] when it had
    /// returned none, [`CursorPhase::Close`] when it had returned all
    /// `limit`, and [`CursorPhase::Fetch`] in between.
    #[must_use]
    pub fn after(fetched: Vec<T>, limit: u16, error: PortError) -> Self {
        let phase = if fetched.is_empty() {
            CursorPhase::Open
        } else if fetched.len() < usize::from(limit) {
            CursorPhase::Fetch
        } else {
            CursorPhase::Close
        };
        Self {
            phase,
            fetched,
            error,
        }
    }
}

/// A failure before the cursor returned any row, the statement never sent
/// included: its `OPEN`.
impl<T> From<PortError> for CursorError<T> {
    fn from(error: PortError) -> Self {
        Self {
            phase: CursorPhase::Open,
            fetched: Vec::new(),
            error,
        }
    }
}

/// The result of a cursor read: its rows in fetch order, or its failure.
pub type CursorResult<T> = Result<Vec<T>, CursorError<T>>;

/// The Db2 transaction type master `CARDDEMO.TRANSACTION_TYPE`, table
/// `transaction_types`.
///
/// Used by CTTU (`COTRTUPC`), CTLI (`COTRTLIC`) and `batch mnttrdb2`
/// (`COBTUPDT`), the only programs that change it. Reports read the VSAM
/// processing copy through [`TranTypeFileReader`] instead, which this master
/// reaches only through `batch seed --only tranextr` (AAP 0.3.2).
pub trait TranTypeMasterStore {
    /// `SELECT TR_TYPE, TR_DESCRIPTION … WHERE TR_TYPE = :code`: `COTRTUPC`
    /// `9100-GET-TRANSACTION-TYPE`.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no type has the code (`SQLCODE` +100);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_tran_type(
        &mut self,
        code: TranTypeCode,
    ) -> impl Future<Output = PortResult<TranTypeMaster>> + Send + '_;

    /// The `COTRTLIC` list cursors, filtered by `filter` (P-COTRTLIC-003):
    ///
    /// - Forward ([`Direction::Forward`]): `C-TR-TYPE-FORWARD`,
    ///   `TR_TYPE >= :start ORDER BY TR_TYPE` (`8000-READ-FORWARD`,
    ///   P-COTRTLIC-030);
    /// - Backward ([`Direction::Backward`]): `C-TR-TYPE-BACKWARD`,
    ///   `TR_TYPE < :start ORDER BY TR_TYPE DESC` (`8100-READ-BACKWARDS`,
    ///   P-COTRTLIC-033). The bound is strict, unlike the [`Browse`] default:
    ///   the cursor excludes the start key itself.
    ///
    /// `TR_TYPE` is `CHAR(2)`, so keys compare and sort as their two
    /// space-padded bytes, the `Ord` of [`TranTypeCode`]: `A\x01` comes before
    /// `A `. `None` as the start key reads from the first type forward or from
    /// the last type backward. The blank key `COTRTLIC` holds after a filter
    /// change is `Some` key of spaces, which a backward read answers with only
    /// the codes that sort below two spaces, those that begin with a byte
    /// below the space or with a space and then such a byte: no rows when no
    /// code holds one.
    ///
    /// # Errors
    ///
    /// A [`CursorError`] naming the step that failed, with the rows fetched
    /// before it (P-COTRTLIC-031, P-COTRTLIC-032, P-COTRTLIC-034,
    /// P-COTRTLIC-037 to P-COTRTLIC-040): [`PortError::Contention`] on a lock
    /// timeout or deadlock (`SQLCODE` -911), [`PortError::Unavailable`] on any
    /// other failure. A cursor that fetches nothing is `Ok` with no rows.
    fn browse_tran_types(
        &mut self,
        browse: Browse<TranTypeCode>,
        filter: TranTypeFilter,
    ) -> impl Future<Output = CursorResult<TranTypeMaster>> + Send + '_;

    /// `SELECT COUNT(1)` of the types that pass `filter`, with no start key:
    /// `COTRTLIC` `9100-CHECK-FILTERS` (P-COTRTLIC-036), whose zero count gives
    /// the no-records message and whose failure has a message of its own.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn count_tran_types(
        &mut self,
        filter: TranTypeFilter,
    ) -> impl Future<Output = PortResult<u64>> + Send + '_;

    /// `SELECT COUNT(1)` of the types that pass `filter` with the
    /// `C-TR-TYPE-BACKWARD` start condition `TR_TYPE < :start`, `key` the
    /// start: the number of types a page-back from `key` has before it
    /// (`COTRTLIC` `8100-READ-BACKWARDS`, P-COTRTLIC-033, P-COTRTLIC-034,
    /// P-COTRTLIC-039, V-COTRTLIC-009).
    ///
    /// `COTRTLIC` keeps the page number in its COMMAREA and decides from it
    /// whether a page-back reaches the first page; the request carries none
    /// (E-54), so the rules count instead. The bound is strict, and keys
    /// compare as their two space-padded bytes, the order and the NUL
    /// semantics of [`TranTypeMasterStore::browse_tran_types`] backward. The
    /// count names no description unless `filter` gives one, so a row whose
    /// description cannot be read fails it only under a description filter.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock timeout or deadlock (`SQLCODE`
    /// -911); [`PortError::Unavailable`] on any other failure.
    fn count_tran_types_before(
        &mut self,
        filter: TranTypeFilter,
        key: TranTypeCode,
    ) -> impl Future<Output = PortResult<u64>> + Send + '_;

    /// `INSERT INTO CARDDEMO.TRANSACTION_TYPE`: `COTRTUPC`
    /// `9700-INSERT-RECORD` and `COBTUPDT` `10031-INSERT-DB` (P-COBTUPDT-007).
    /// One legacy write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when a type has the code (`SQLCODE` -803);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn insert_tran_type(
        &mut self,
        rec: TranTypeMaster,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// `UPDATE CARDDEMO.TRANSACTION_TYPE SET TR_DESCRIPTION … WHERE TR_TYPE`:
    /// `COTRTLIC` `9200-UPDATE-RECORD` (P-COTRTLIC-009, P-COTRTLIC-010),
    /// `COTRTUPC` `9600-WRITE-PROCESSING` and `COBTUPDT` `10032-UPDATE-DB`
    /// (P-COBTUPDT-008). The description is stored exactly as given. One legacy
    /// write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no row has the code (`SQLCODE` +100);
    /// [`PortError::Contention`] on deadlock or lock timeout (`SQLCODE` -911);
    /// [`PortError::Unavailable`] on any other failure.
    fn update_tran_type(
        &mut self,
        rec: TranTypeMaster,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// `DELETE FROM CARDDEMO.TRANSACTION_TYPE WHERE TR_TYPE`: `COTRTLIC`
    /// `9300-DELETE-RECORD` (P-COTRTLIC-007, P-COTRTLIC-008), `COTRTUPC`
    /// `9800-DELETE-PROCESSING` and `COBTUPDT` `10033-DELETE-DB`
    /// (P-COBTUPDT-009). One legacy write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::Restricted`] when categories still reference the type
    /// (`SQLCODE` -532); [`PortError::NotFound`] when no row has the code
    /// (`SQLCODE` +100); [`PortError::Contention`] on deadlock or lock timeout;
    /// [`PortError::Unavailable`] on any other failure.
    fn delete_tran_type(
        &mut self,
        code: TranTypeCode,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;
}

/// Reads of the VSAM processing copies of the reference data: `TRANTYPE`
/// (`CVTRA03Y`, table `trantype_file`) and `TRANCATG` (`CVTRA04Y`, table
/// `trancatg_file`).
///
/// Only `CBTRN03C` reads them, for `batch tranrept` and the CR00 report; the
/// Db2 masters are [`TranTypeMasterStore`] (AAP 0.3.2).
pub trait TranTypeFileReader {
    /// Random `READ` of `TRANTYPE` by type code: `CBTRN03C`
    /// `1500-B-LOOKUP-TRANTYPE` (P-CBTRN03C-005).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no record has the code (status 23, the
    /// `INVALID TRANSACTION TYPE` abend); [`PortError::Contention`] on a lock
    /// conflict with another unit of work; [`PortError::Unavailable`] on any
    /// other failure.
    fn read_tran_type_file(
        &mut self,
        code: TranTypeCode,
    ) -> impl Future<Output = PortResult<TranTypeRecord>> + Send + '_;

    /// Random `READ` of `TRANCATG` by type code and category code: `CBTRN03C`
    /// `1500-C-LOOKUP-TRANCATG` (P-CBTRN03C-005).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no record has the key (status 23, the
    /// `INVALID TRAN CATG KEY` abend); [`PortError::Contention`] on a lock
    /// conflict with another unit of work; [`PortError::Unavailable`] on any
    /// other failure.
    fn read_tran_category_file(
        &mut self,
        code: TranTypeCode,
        category: TranCategoryCode,
    ) -> impl Future<Output = PortResult<TranCategoryRecord>> + Send + '_;
}

/// The user security file: VSAM `USRSEC` (`CSUSR01Y`), table `users`.
///
/// Used by CC00 (`COSGN00C`) and CU00 to CU03 (`COUSR00C` to `COUSR03C`). The
/// port stores the user record as given: hashing, verifying and upper-casing
/// belong to `crate::auth::password` and the rules (E-20).
///
/// Only [`UserStore::read_user`] (the sign-on) and
/// [`UserStore::read_user_for_update`] (the `COUSR02C` password compare and
/// rewrite) read the password hash. Every other read compares no credential
/// and returns the key alone or a [`UserProfile`], so no hash is read for it.
pub trait UserStore {
    /// CICS `READ` of `USRSEC` by `SEC-USR-ID`: `COSGN00C`
    /// `READ-USER-SEC-FILE` (P-COSGN00C-008), whose password compare needs the
    /// stored hash. The key is compared byte for byte, case included. One of
    /// the two reads that carry the hash, with
    /// [`UserStore::read_user_for_update`].
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no user has the ID (`RESP` 13);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_user(
        &mut self,
        id: UserId,
    ) -> impl Future<Output = PortResult<UserSecurity>> + Send + '_;

    /// CICS `READ` of `USRSEC` by `SEC-USR-ID` that keeps only the key: the
    /// `COSGN00C` `READ-USER-SEC-FILE` (P-COSGN00C-008) repeated for a
    /// verified bearer token, which asks only whether a user still has the
    /// ID. The key is compared byte for byte, case included, and the stored
    /// key is returned.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no user has the ID (`RESP` 13);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_user_key(&mut self, id: UserId)
    -> impl Future<Output = PortResult<UserId>> + Send + '_;

    /// CICS `READ … UPDATE` of `USRSEC`, SQL `SELECT … FOR UPDATE`: `COUSR02C`
    /// `READ-USER-SEC-FILE` before its comparison and rewrite (P-COUSR02C-002,
    /// P-COUSR02C-003), whose password compare needs the stored hash. One of
    /// the two reads that carry the hash, with [`UserStore::read_user`].
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no user has the ID;
    /// [`PortError::Contention`] when the lock cannot be taken;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_user_for_update(
        &mut self,
        id: UserId,
    ) -> impl Future<Output = PortResult<UserSecurity>> + Send + '_;

    /// CICS `READ … UPDATE` of `USRSEC` without the password, SQL
    /// `SELECT … FOR UPDATE` that leaves out the hash: the ENTER fetch of
    /// `COUSR02C` and `COUSR03C` (`READ-USER-SEC-FILE`, P-COUSR02C-002,
    /// P-COUSR02C-003, P-COUSR03C-003), which never shows the password (E-21),
    /// and `COUSR03C` `DELETE-USER-INFO` before the delete (P-COUSR03C-004).
    /// It takes the same row lock as [`UserStore::read_user_for_update`].
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no user has the ID;
    /// [`PortError::Contention`] when the lock cannot be taken;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_user_profile_for_update(
        &mut self,
        id: UserId,
    ) -> impl Future<Output = PortResult<UserProfile>> + Send + '_;

    /// The `COUSR00C` user list: `STARTBR` on `SEC-USR-ID` with `READNEXT`
    /// (`PROCESS-PAGE-FORWARD`, P-COUSR00C-003) or `READPREV`
    /// (`PROCESS-PAGE-BACKWARD`, P-COUSR00C-009), rows per the [`Browse`]
    /// contract in user-ID byte order. The list shows no password, so each
    /// row is a [`UserProfile`].
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; an empty page, the
    /// `STARTBR` `NOTFND` of P-COUSR00C-006, is `Ok` with no rows.
    fn browse_users(
        &mut self,
        browse: Browse<UserId>,
    ) -> impl Future<Output = PortResult<Vec<UserProfile>>> + Send + '_;

    /// CICS `WRITE` of a new user: `COUSR01C` `WRITE-USER-SEC-FILE`
    /// (P-COUSR01C-004, P-COUSR01C-005). One legacy write under its own
    /// savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when a user has the ID (`DUPKEY`, `DUPREC`);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn insert_user(
        &mut self,
        user: UserSecurity,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// CICS `REWRITE` of a user, keyed by its `sec_usr_id`: `COUSR02C`
    /// `UPDATE-USER-SEC-FILE` (P-COUSR02C-006). One legacy write under its own
    /// savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no user has the ID;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn update_user(
        &mut self,
        user: UserSecurity,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// CICS `DELETE` of a user: `COUSR03C` `DELETE-USER-SEC-FILE`
    /// (P-COUSR03C-005). One legacy write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no user has the ID;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn delete_user(&mut self, id: UserId) -> impl Future<Output = PortResult<()>> + Send + '_;
}

/// The pending authorizations and their fraud reports: the IMS database
/// `DBPAUTP0`, root segment `PAUTSUM0` (`CIPAUSMY`, table
/// `pending_auth_summaries`) and child segment `PAUTDTL1` (`CIPAUDTY`, table
/// `pending_auth_details`), plus the Db2 table `CARDDEMO.AUTHFRDS` (table
/// `auth_fraud`).
///
/// Used by CP00 (`COPAUA0C`), CPVS (`COPAUS0C`), CPVD (`COPAUS1C` and
/// `COPAUS2C`) and `batch cbpaup0j` (`CBPAUP0C`). The details of one summary
/// are IMS twins in the order of their sequence field `PAUT9CTS`: ascending
/// (`pa_auth_date_9c`, `pa_auth_time_9c`), which lists the newest
/// authorization first because both parts are stored complemented.
pub trait AuthorizationStore {
    /// DL/I `GU PAUTSUM0 WHERE (ACCNTID = :id)`: `COPAUA0C`
    /// `5500-READ-AUTH-SUMMRY` (P-COPAUA0C-008), `COPAUS0C` `GET-AUTH-SUMMARY`
    /// (P-COPAUS0C-009). `COPAUS1C` reads the same summary through
    /// [`Self::read_summary_of_detail`].
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when the account has no summary (status `GE`);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_summary(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<PendingAuthSummary>> + Send + '_;

    /// DL/I `GU PAUTSUM0 WHERE (ACCNTID = :id)` with the update intent of the
    /// PSB `PSBPAUTB` (`PROCOPT=AP`), SQL `SELECT … FOR UPDATE`: `COPAUA0C`
    /// `5500-READ-AUTH-SUMMRY` (P-COPAUA0C-008), and the summary the
    /// `8000-WRITE-AUTH-TO-DB` unit of work replaces or inserts under
    /// (`8400-UPDATE-SUMMARY`, P-COPAUA0C-013).
    ///
    /// IMS program isolation orders `COPAUA0C` and the `CBPAUP0C` BMP, both
    /// scheduled on `PSBPAUTB`, one after the other. The call therefore waits
    /// for a unit of work that holds the summary, a running purge sweep
    /// included ([`Self::summaries_after`]), and then reads what that unit
    /// left: the summary it committed, or no summary when it deleted it. The
    /// lock is held until the caller's transaction commits or rolls back, so
    /// a sweep that reaches the summary meanwhile waits for that end in turn.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when the account has no summary (status `GE`);
    /// [`PortError::Contention`] when the wait ends in a lock timeout or a
    /// deadlock; [`PortError::Unavailable`] on any other failure.
    fn read_summary_for_update(
        &mut self,
        id: AccountId,
    ) -> impl Future<Output = PortResult<PendingAuthSummary>> + Send + '_;

    /// DL/I `GU PAUTSUM0 WHERE (ACCNTID = :account)` for the account of the
    /// detail `id`: `COPAUS1C` `READ-AUTH-RECORD`, the summary read ENTER, PF5
    /// and PF8 issue before the detail read (P-COPAUS1C-003, P-COPAUS1C-004).
    ///
    /// The COBOL qualifies the `GU` by the `CDEMO-ACCT-ID` its COMMAREA
    /// carries. The REST request carries only the authorization identity
    /// (P-COPAUS1C-012), so the account is resolved here, server-side, as the
    /// parent of that detail.
    ///
    /// A failure leaves the unit of work usable: `READ-AUTH-RECORD` still
    /// issues the detail read after a summary `GU` that fails with a status
    /// other than `GE` or `GB` (P-COPAUS1C-014), and that read's outcome
    /// decides which error text the caller answers (V-COPAUS1C-003,
    /// V-COPAUS1C-006).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no detail has the identity or its account
    /// has no summary (status `GE`); [`PortError::Contention`] on a lock
    /// conflict with another unit of work; [`PortError::Unavailable`] on any
    /// other failure.
    fn read_summary_of_detail(
        &mut self,
        id: AuthId,
    ) -> impl Future<Output = PortResult<PendingAuthSummary>> + Send + '_;

    /// DL/I `ISRT PAUTSUM0`: `COPAUA0C` `8400-UPDATE-SUMMARY` when no summary
    /// was found (P-COPAUA0C-013). One legacy write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when the account already has a summary (status
    /// `II`); [`PortError::Contention`] on a lock conflict with another unit of
    /// work; [`PortError::Unavailable`] on any other failure.
    fn insert_summary(
        &mut self,
        summary: PendingAuthSummary,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// DL/I `REPL PAUTSUM0`, keyed by its `pa_acct_id`: `COPAUA0C`
    /// `8400-UPDATE-SUMMARY` when the summary was found (P-COPAUA0C-013). One
    /// legacy write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when the account has no summary;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn update_summary(
        &mut self,
        summary: PendingAuthSummary,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// DL/I `DLET PAUTSUM0`: `CBPAUP0C` `6000-DELETE-AUTH-SUMMARY`
    /// (P-CBPAUP0C-007). As an IMS delete of a parent, it also deletes every
    /// detail still under the summary (P-CBPAUP0C-006). One legacy write under
    /// its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when the account has no summary;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn delete_summary(&mut self, id: AccountId)
    -> impl Future<Output = PortResult<()>> + Send + '_;

    /// DL/I `GN PAUTSUM0`, the purge sweep of `CBPAUP0C`
    /// `2000-FIND-NEXT-AUTH-SUMMARY` (P-CBPAUP0C-002): up to `limit` summaries
    /// whose account is greater than `after`, in `ACCNTID` order, from the first
    /// summary when `after` is `None`.
    ///
    /// The sweep holds each summary it returns until the purge's unit of work
    /// commits or rolls back, as the BMP's update-intent `GN` holds it under
    /// IMS program isolation. A summary another unit of work holds, through
    /// [`Self::read_summary_for_update`], is waited for and returned as that
    /// unit left it; one it deleted is not returned. A pending authorization
    /// is therefore written either before the sweep reads its summary or after
    /// the purge ends, never under a summary the purge has already judged.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] when the wait for a held summary ends in a
    /// lock timeout or a deadlock; [`PortError::Unavailable`] on any other
    /// failure. The end of the database (status `GB`) is `Ok` with no rows.
    fn summaries_after(
        &mut self,
        after: Option<AccountId>,
        limit: u16,
    ) -> impl Future<Output = PortResult<Vec<PendingAuthSummary>>> + Send + '_;

    /// DL/I `ISRT PAUTDTL1` under the summary of `detail.pa_acct_id`:
    /// `COPAUA0C` `8500-INSERT-AUTH` (P-COPAUA0C-014). One legacy write under
    /// its own savepoint; it returns the identity the REST resource uses.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when the summary already has a detail with the
    /// same `PAUT9CTS` key (status `II`); [`PortError::NotFound`] when the
    /// account has no summary to insert under (status `GE`);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn insert_detail(
        &mut self,
        detail: NewPendingAuthDetail,
    ) -> impl Future<Output = PortResult<AuthId>> + Send + '_;

    /// The detail with the identity `id`: the record `COPAUS1C`
    /// `READ-AUTH-RECORD` reaches with `GNP PAUTDTL1 WHERE (PAUT9CTS = :key)`
    /// under its summary (P-COPAUS1C-003).
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no detail has the identity (status `GE`);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_detail(
        &mut self,
        id: AuthId,
    ) -> impl Future<Output = PortResult<PendingAuthDetail>> + Send + '_;

    /// [`Self::read_detail`] under a row lock, SQL `SELECT … FOR UPDATE`: the
    /// detail `COPAUS1C` `MARK-AUTH-FRAUD` reads through `READ-AUTH-RECORD`
    /// before it toggles the fraud flag and replaces the detail (PF5,
    /// P-COPAUS1C-006 to P-COPAUS1C-008).
    ///
    /// The lock is held until the caller's transaction commits or rolls back,
    /// so a second fraud mark of the same authorization waits and then reads
    /// the flag the first one committed. `COPAUS1C` takes no lock of its own;
    /// this one keeps two overlapping marks from both deriving their action
    /// from the same flag, which would lose a toggle. The reads of the view
    /// and of PF8 stay unlocked, through [`Self::read_detail`].
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no detail has the identity (status `GE`);
    /// [`PortError::Contention`] when the lock cannot be taken;
    /// [`PortError::Unavailable`] on any other failure.
    fn read_detail_for_update(
        &mut self,
        id: AuthId,
    ) -> impl Future<Output = PortResult<PendingAuthDetail>> + Send + '_;

    /// The details of one account in twin order: the `GNP PAUTDTL1` loop of
    /// `COPAUS0C` `PROCESS-PAGE-FORWARD` (P-COPAUS0C-010) and of `CBPAUP0C`
    /// `3000-FIND-NEXT-AUTH-DTL` (P-CBPAUP0C-003).
    ///
    /// The [`Browse`] start key names a detail of the account, and the rows
    /// start at its `PAUT9CTS` key inclusive, as `COPAUS0C`
    /// `REPOSITION-AUTHORIZATIONS` positions with
    /// `GNP … WHERE (PAUT9CTS = :key)` before reading on. Forward is twin order
    /// (newest first); backward is the reverse. A start key that names no
    /// detail of the account finds nothing, as that `GNP` ends with `GE`.
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; no detail, or the end
    /// of the twins, is `Ok` with no rows.
    fn details_for_account(
        &mut self,
        id: AccountId,
        browse: Browse<AuthId>,
    ) -> impl Future<Output = PortResult<Vec<PendingAuthDetail>>> + Send + '_;

    /// The twin next to the `PAUT9CTS` key (`auth_date_9c`, `auth_time_9c`)
    /// under the summary of `account`: `COPAUS1C` `READ-NEXT-AUTH-RECORD`, the
    /// unqualified `GNP PAUTDTL1` from the detail `READ-AUTH-RECORD` positioned
    /// on (PF8, P-COPAUS1C-014).
    ///
    /// The account and the key are those of the detail already read, so the
    /// step needs no read of that detail or of its parent.
    /// [`Direction::Forward`] is the first twin with a greater key, the next
    /// older authorization; [`Direction::Backward`] the last twin with a
    /// smaller key, the step back the web detail page adds (`COPAUS1C` has no
    /// PF7).
    ///
    /// `None` when no twin follows in that direction (status `GE` or `GB`,
    /// `"Already at the last Authorization..."`, P-COPAUS1C-009).
    ///
    /// # Errors
    ///
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure; the end of the twins
    /// is `Ok(None)`.
    fn adjacent_detail(
        &mut self,
        account: AccountId,
        auth_date_9c: i32,
        auth_time_9c: i32,
        direction: Direction,
    ) -> impl Future<Output = PortResult<Option<PendingAuthDetail>>> + Send + '_;

    /// DL/I `REPL PAUTDTL1`, keyed by its `auth_id`: `COPAUS1C`
    /// `UPDATE-AUTH-DETAILS` after fraud marking (P-COPAUS1C-008). One legacy
    /// write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no detail has the identity;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn update_detail(
        &mut self,
        detail: PendingAuthDetail,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// DL/I `DLET PAUTDTL1`: `CBPAUP0C` `5000-DELETE-AUTH-DTL` (P-CBPAUP0C-005).
    /// One legacy write under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no detail has the identity;
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn delete_detail(&mut self, id: AuthId) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// `INSERT INTO CARDDEMO.AUTHFRDS`: `COPAUS2C` `MAIN-PARA`
    /// (P-COPAUS2C-003). One legacy write under its own savepoint; the rule
    /// turns a duplicate into the `FRAUD-UPDATE` path.
    ///
    /// # Errors
    ///
    /// [`PortError::Duplicate`] when a report with the same `CARD_NUM` and
    /// `AUTH_TS` exists (`SQLCODE` -803); [`PortError::Contention`] on a lock
    /// conflict with another unit of work; [`PortError::Unavailable`] on any
    /// other failure.
    fn insert_fraud(
        &mut self,
        fraud: AuthFraud,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;

    /// `UPDATE CARDDEMO.AUTHFRDS SET AUTH_FRAUD, FRAUD_RPT_DATE` of the report
    /// keyed by `fraud.card_num` and `fraud.auth_ts`: `COPAUS2C`
    /// `FRAUD-UPDATE` (P-COPAUS2C-004). Both values are taken from `fraud`; the
    /// rule fills the report date from the processing clock. One legacy write
    /// under its own savepoint.
    ///
    /// # Errors
    ///
    /// [`PortError::NotFound`] when no report has the key (`SQLCODE` +100);
    /// [`PortError::Contention`] on a lock conflict with another unit of work;
    /// [`PortError::Unavailable`] on any other failure.
    fn update_fraud(
        &mut self,
        fraud: AuthFraud,
    ) -> impl Future<Output = PortResult<()>> + Send + '_;
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::pin::{Pin, pin};
    use std::task::{Context, Poll, Waker};

    use super::{AccountReader, CardFilter, Direction, PortError, PortResult, TranTypeFilter};
    use crate::entities::Account;
    use crate::numeric::{AccountGroupId, AccountId, Alnum, Money};

    /// Polls before [`block_on`] gives up on a future that never completes.
    const MAX_POLLS: usize = 16;

    /// Drives `future` to completion on the current thread, with no runtime:
    /// the domain crate has none. The in-memory store never waits on I/O, so a
    /// waker that does nothing suffices and a pending future is polled again.
    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        let mut context = Context::from_waker(Waker::noop());
        for _ in 0..MAX_POLLS {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
        }
        panic!("future still pending after {MAX_POLLS} polls");
    }

    /// Pending on its first poll and ready on the next, as an adapter's future
    /// is pending while its query runs, so each port call crosses an await
    /// point holding `&mut self`.
    struct YieldOnce {
        yielded: bool,
    }

    impl Future for YieldOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                Poll::Ready(())
            } else {
                self.yielded = true;
                context.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// An account master in memory, implemented with `async fn` as the
    /// persistence adapters implement the port.
    #[derive(Default)]
    struct MemoryAccounts {
        accounts: BTreeMap<AccountId, Account>,
        locked: Vec<AccountId>,
    }

    impl AccountReader for MemoryAccounts {
        async fn read_account(&mut self, id: AccountId) -> PortResult<Account> {
            YieldOnce { yielded: false }.await;
            self.accounts.get(&id).cloned().ok_or(PortError::NotFound)
        }

        async fn read_account_for_update(&mut self, id: AccountId) -> PortResult<Account> {
            YieldOnce { yielded: false }.await;
            let account = self.accounts.get(&id).cloned().ok_or(PortError::NotFound)?;
            self.locked.push(id);
            Ok(account)
        }
    }

    /// Compiles only because the trait declares its futures `Send`: the store
    /// type is a generic parameter here, so nothing about it is known beyond
    /// the port.
    fn locking_read<A: AccountReader>(
        store: &mut A,
        id: AccountId,
    ) -> impl Future<Output = PortResult<Account>> + Send + '_ {
        store.read_account_for_update(id)
    }

    /// A rule-shaped caller: generic over the port, never a trait object.
    async fn balance_of<A: AccountReader>(store: &mut A, id: AccountId) -> PortResult<Money<10>> {
        store
            .read_account(id)
            .await
            .map(|account| account.acct_curr_bal)
    }

    /// Requires `future` to be `Send`, as the multi-threaded runtime of the
    /// adapters does.
    fn require_send<F: Future + Send>(future: F) -> F {
        future
    }

    fn account_id(value: u64) -> AccountId {
        AccountId::new(value).expect("an account number of at most 11 digits")
    }

    fn account(id: u64, balance_cents: i64) -> Account {
        Account {
            acct_id: account_id(id),
            acct_active_status: Alnum::move_from("Y"),
            acct_curr_bal: Money::from_cents(balance_cents).expect("a balance within S9(10)V99"),
            acct_credit_limit: Money::ZERO,
            acct_cash_credit_limit: Money::ZERO,
            acct_open_date: Alnum::move_from("2014-11-20"),
            acct_expiraion_date: Alnum::move_from("2025-05-20"),
            acct_reissue_date: Alnum::move_from("2025-05-20"),
            acct_curr_cyc_credit: Money::ZERO,
            acct_curr_cyc_debit: Money::ZERO,
            acct_addr_zip: Alnum::move_from("A000000000"),
            acct_group_id: AccountGroupId::move_from(""),
        }
    }

    fn store_with(accounts: &[Account]) -> MemoryAccounts {
        MemoryAccounts {
            accounts: accounts
                .iter()
                .map(|account| (account.acct_id, account.clone()))
                .collect(),
            locked: Vec::new(),
        }
    }

    #[test]
    fn async_fn_adapter_serves_reads_through_the_declared_signatures() {
        let stored = account(1, 19_400);
        let mut store = store_with(&[stored.clone(), account(2, -5)]);

        let balance = block_on(require_send(balance_of(&mut store, account_id(1))));
        let expected = Money::from_cents(19_400).expect("a balance within S9(10)V99");
        assert_eq!(balance, Ok(expected));

        let locked = block_on(require_send(locking_read(&mut store, account_id(1))));
        assert_eq!(locked, Ok(stored));
        assert_eq!(store.locked, vec![account_id(1)]);
    }

    #[test]
    fn missing_key_is_not_found_on_every_read() {
        let mut store = MemoryAccounts::default();
        let missing = account_id(99_999_999_999);

        assert_eq!(
            block_on(store.read_account(missing)),
            Err(PortError::NotFound)
        );
        assert_eq!(
            block_on(store.read_account_for_update(missing)),
            Err(PortError::NotFound)
        );
        assert!(store.locked.is_empty(), "a failed read takes no lock");
    }

    #[test]
    fn each_outcome_reports_its_db2_sqlcode() {
        let cases = [
            (PortError::NotFound, Some(100)),
            (PortError::Duplicate, Some(-803)),
            (PortError::Restricted, Some(-532)),
            (PortError::Contention, Some(-911)),
            (PortError::Unavailable("connection reset".to_owned()), None),
        ];
        for (error, sqlcode) in cases {
            assert_eq!(error.sqlcode(), sqlcode, "{error:?}");
        }
    }

    #[test]
    fn unavailable_keeps_the_adapter_detail_for_logs() {
        let error = PortError::Unavailable("pool timed out".to_owned());
        assert_eq!(error.to_string(), "store unavailable: pool timed out");
    }

    #[test]
    fn browse_vocabulary_defaults_to_an_unfiltered_forward_read() {
        assert_eq!(Direction::default(), Direction::Forward);
        assert_eq!(
            CardFilter::default(),
            CardFilter {
                account_id: None,
                card_number: None,
            }
        );
        assert_eq!(
            TranTypeFilter::default(),
            TranTypeFilter {
                type_code: None,
                description: None,
            }
        );
    }
}
