//! `CBTRN03C`: the daily transaction detail report of `batch tranrept` and of
//! the CR00 report `GET /reports/transactions`.
//!
//! The TRANREPT job [app/jcl/TRANREPT.jcl] [app/proc/TRANREPT.prc] backs up the
//! live master, keeps the records whose processing date lies in the range and
//! sorts them by card number (`STEP05R`, P-TRANREPT-002), then runs `CBTRN03C`
//! over that file with the same dates in `DATEPARM` (P-TRANREPT-003, E-34).
//! [`select_for_report`] is the `STEP05R` selection and sort, and
//! [`report_sort_key`] its rule for one record, which `batch tranrept` applies
//! as it streams the backup into its external sort; [`ReportRun`] is
//! the program: the caller feeds it the selected records in order through
//! [`ReportRun::process`] and ends it with [`ReportRun::finish`], writing every
//! returned line as one 133-byte `TRANREPT` record. Both callers share both
//! steps, so the report and `batch tranrept` cannot drift apart. The CR00
//! report reads the selection from the live master through
//! [`ReportSelection`], which yields the records of [`select_for_report`] in
//! bounded pages.
//!
//! The lookups read the VSAM processing copies `TRANTYPE` and `TRANCATG`
//! through [`TranTypeFileReader`], never the Db2 masters, so an admin edit of a
//! type reaches the report only after `batch seed --only tranextr` (AAP 0.3.2).
//! A run reads each type and category key the first time a record carries it
//! and keeps the record for the rest of that run only, so the next report reads
//! the copies again. Every line comes from the `CVTRA07Y` builders of
//! [`crate::format`].
//!
//! Behaviour the COBOL defines and the port keeps:
//!
//! - The date filter of the read loop ends the whole report at the first record
//!   outside the range, writing no totals: `NEXT SENTENCE` leaves the inline
//!   `PERFORM` (P-CBTRN03C-003). The `STEP05R` selection keeps such records out
//!   of the input, so a correct caller never meets this.
//! - At the end of the input the amount of the last record is added a second
//!   time to the page and account totals, and the account total of the last
//!   card is never written (P-CBTRN03C-009, P-CBTRN03C-013).
//! - The line counter counts the total and header lines too, so a page break
//!   falls where `MOD(WS-LINE-COUNTER, 20) = 0` happens to hold before a detail
//!   line, not every 20 details (P-CBTRN03C-006).

use std::collections::BTreeMap;
use std::fmt;

use crate::entities::{CardXref, TranCategoryRecord, TranTypeRecord};
use crate::format::{self, ReportDetail};
use crate::messages;
use crate::numeric::Money;
use crate::ports::{PortError, PortResult};

/// The transaction record, the keys and codes it carries, and the readers the
/// report and its selection read through, re-exported from
/// [`crate::entities`], [`crate::numeric`] and [`crate::ports`] for the API
/// and batch adapters.
pub use crate::entities::Transaction;
pub use crate::numeric::{
    Alnum, CardNumber, TranCategoryCode, TranId, TranTypeCode, TransactionRowId,
};
pub use crate::ports::{TranTypeFileReader, TransactionReader, XrefReader};

/// `WS-PAGE-SIZE PIC 9(03) COMP-3 VALUE 20` [app/cbl/CBTRN03C.cbl:L131-132]:
/// a page break is due when the line counter is a multiple of it
/// (P-CBTRN03C-006). The program never changes it.
pub const PAGE_SIZE: u32 = 20;

/// `10^9`: `WS-LINE-COUNTER PIC 9(09) COMP-3` [app/cbl/CBTRN03C.cbl:L129-130]
/// keeps the low-order nine digits of its `ADD 1`, which has no
/// `ON SIZE ERROR`.
const LINE_COUNTER_LIMIT: u32 = 1_000_000_000;

/// The record key a failed lookup displays after its literal: the `FD` key
/// field the program moved before the `READ`.
///
/// `Debug` masks a card number to its last four digits, as [`CardNumber`]
/// does; `Display` writes the full key, as the COBOL `DISPLAY` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookupKey {
    /// `FD-XREF-CARD-NUM X(16)` of `1500-A-LOOKUP-XREF`.
    Card(CardNumber),
    /// `FD-TRAN-TYPE X(02)` of `1500-B-LOOKUP-TRANTYPE`.
    TranType(TranTypeCode),
    /// `FD-TRAN-CAT-KEY` of `1500-C-LOOKUP-TRANCATG`: `FD-TRAN-TYPE-CD X(02)`
    /// followed by `FD-TRAN-CAT-CD 9(04)`.
    TranCategory(TranTypeCode, TranCategoryCode),
}

/// The key as the `DISPLAY` shows it: the full stored text of the `FD` field,
/// the category code as its four `DISPLAY` digits.
impl fmt::Display for LookupKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Card(card) => write!(formatter, "{card}"),
            Self::TranType(code) => write!(formatter, "{code}"),
            Self::TranCategory(code, category) => write!(formatter, "{code}{category}"),
        }
    }
}

/// A lookup that ended the report: CBTRN03C `1500-A-LOOKUP-XREF`,
/// `1500-B-LOOKUP-TRANTYPE` or `1500-C-LOOKUP-TRANCATG` met `INVALID KEY`,
/// displayed its literal and the key, moved 23 to `IO-STATUS` and called
/// `9999-ABEND-PROGRAM` — P-CBTRN03C-005, P-CBTRN03C-484, P-CBTRN03C-485.
///
/// The caller abends: `batch tranrept` exits 16 with `ABEND 999` and the
/// displays, dropping the uncataloged `TRANREPT(+1)` as
/// `DISP=(NEW,CATLG,DELETE)` does; the CR00 endpoint answers 500. A read that
/// fails for any other reason is reported the same way, with its cause.
///
/// The error's `Display` names the paragraph and the literal but not the key,
/// so a log line never carries a full card number; [`ReportAbend::display_line`]
/// is the complete COBOL `DISPLAY`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("CBTRN03C {paragraph}: {}", .display.trim_end_matches([' ', ':']))]
pub struct ReportAbend {
    /// The paragraph that failed, for example `1500-B-LOOKUP-TRANTYPE`.
    pub paragraph: &'static str,
    /// The literal it displays, one of [`messages::INVALID_CARD_NUMBER`],
    /// [`messages::INVALID_TRANSACTION_TYPE`] and
    /// [`messages::INVALID_TRAN_CATG_KEY`].
    pub display: &'static str,
    /// The key it displays after the literal.
    pub key: LookupKey,
    /// What the store answered: [`PortError::NotFound`] is the `INVALID KEY`
    /// path.
    #[source]
    pub cause: PortError,
}

impl ReportAbend {
    /// The `DISPLAY` of the failed lookup: the literal followed by the full
    /// key, for example `INVALID TRANSACTION TYPE : 09`. A card lookup shows
    /// the whole card number, as the COBOL job log does.
    #[must_use]
    pub fn display_line(&self) -> String {
        format!("{}{}", self.display, self.key)
    }
}

/// A record the `STEP05R` sort cannot place: its card number holds a byte
/// that is not a digit.
///
/// `SORT FIELDS=(TRAN-CARD-NUM,A)` sorts the card number as zoned decimal
/// (`TRAN-CARD-NUM,263,16,ZD`, P-TRANREPT-002). For card numbers of 16 digits
/// that order is the byte order; for any other byte no audit row pins where
/// DFSORT places the record, so the selection stops rather than guess it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("TRANREPT STEP05R: the card number of transaction {tran_id} holds a non-digit byte")]
pub struct UnpinnedSortKey {
    /// The TRAN-ID of the first such record in input order.
    pub tran_id: TranId,
}

/// Whether `TRAN-PROC-TS (1:10)` lies from `start` to `end` inclusive, both
/// compared as 10-byte character strings: the `STEP05R` `INCLUDE COND`
/// (P-TRANREPT-002) and the `CBTRN03C` read-loop test [app/cbl/CBTRN03C.cbl:L173-174]
/// (P-CBTRN03C-003).
fn in_date_range(start: &Alnum<10>, end: &Alnum<10>, tran: &Transaction) -> bool {
    let proc_date = Alnum::<10>::move_from_bytes(tran.tran_proc_ts.prefix(10));
    proc_date >= *start && proc_date <= *end
}

/// The place of a selected record in the `STEP05R` sort output: TRANREPT
/// `SORT FIELDS=(TRAN-CARD-NUM,A)` [app/jcl/TRANREPT.jcl:L37-55] —
/// P-TRANREPT-002, TRAN-ID ascending within a card (E-38).
///
/// The derived order compares the card number first, then the TRAN-ID, so
/// sorting the selected records by their keys, stably, gives the order of
/// [`select_for_report`]. [`report_sort_key`] gives a record its key.
///
/// `Debug` masks the card number to its last four digits, as [`CardNumber`]
/// does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReportSortKey {
    /// `TRAN-CARD-NUM`, the `SORT FIELDS` key (`TRAN-CARD-NUM,263,16,ZD`),
    /// whose 16 digits sort in byte order.
    pub card_number: CardNumber,
    /// `TRAN-ID`, the order within a card: the order of the backup the sort
    /// reads, as escalation E-38 proposes and the audit's report lines assume
    /// (P-CBTRN03C-015).
    pub tran_id: TranId,
}

/// TRANREPT `STEP05R` [app/jcl/TRANREPT.jcl:L37-55] — P-TRANREPT-002, for one
/// `SORTIN` record: `Some` key when `INCLUDE COND` keeps `tran` (its
/// processing date lies from `start` to `end`), `None` when it leaves `tran`
/// out.
///
/// [`select_for_report`] applies it to every record and sorts by the keys;
/// `batch tranrept` applies it record by record as it streams the backup into
/// its external sort, so both selections share this one rule.
///
/// # Errors
///
/// [`UnpinnedSortKey`] naming `tran` when `INCLUDE COND` keeps it and its card
/// number holds a byte other than `0` to `9`, whose zoned-decimal place no
/// audit row pins. A record outside the range never stops the sort.
pub fn report_sort_key(
    start: &Alnum<10>,
    end: &Alnum<10>,
    tran: &Transaction,
) -> Result<Option<ReportSortKey>, UnpinnedSortKey> {
    if !in_date_range(start, end, tran) {
        return Ok(None);
    }
    if !tran.tran_card_num.as_alnum().is_numeric() {
        return Err(UnpinnedSortKey {
            tran_id: tran.tran_id,
        });
    }
    Ok(Some(ReportSortKey {
        card_number: tran.tran_card_num,
        tran_id: tran.tran_id,
    }))
}

/// TRANREPT `STEP05R` [app/jcl/TRANREPT.jcl:L37-55] — P-TRANREPT-002: the
/// records of `records` whose processing date lies from `start` to `end`,
/// sorted ascending by card number, TRAN-ID ascending within a card.
///
/// The sort has no `EQUALS`, so DFSORT leaves the order of one card's records
/// undefined; the order within a card is TRAN-ID order, the order of the
/// backup the sort reads, as escalation E-38 proposes and the audit's report
/// lines assume (P-CBTRN03C-015). Sorting on the TRAN-ID too makes the result
/// independent of the order the caller passes the records in. Each record's
/// place comes from [`report_sort_key`]; records with equal keys keep their
/// input order.
///
/// # Errors
///
/// [`UnpinnedSortKey`] when a selected record's card number holds a byte other
/// than `0` to `9`: the first such record in input order.
pub fn select_for_report(
    start: &Alnum<10>,
    end: &Alnum<10>,
    records: impl IntoIterator<Item = Transaction>,
) -> Result<Vec<Transaction>, UnpinnedSortKey> {
    let mut selected = Vec::new();
    for tran in records {
        if let Some(key) = report_sort_key(start, end, &tran)? {
            selected.push((key, tran));
        }
    }
    selected.sort_by_key(|&(key, _)| key);
    Ok(selected.into_iter().map(|(_, tran)| tran).collect())
}

/// TRANREPT `STEP05R` [app/jcl/TRANREPT.jcl:L37-55] — P-TRANREPT-002, read as
/// CBTRN03C `1000-TRANFILE-GET-NEXT` reads the sorted file — P-CBTRN03C-474:
/// [`select_for_report`] in bounded pages from a [`TransactionReader`], so the
/// caller holds one page of the selection at a time, never all of it.
///
/// Each [`ReportSelection::next_page`] asks the store for the records after the
/// last one it returned, in the `STEP05R` order the port declares (card number,
/// then TRAN-ID, E-38), checks that order, and keeps the records the exact
/// `CH` date compare of [`select_for_report`] keeps. Concatenated, the pages
/// are exactly the records [`select_for_report`] returns for the records the
/// store selects, in its order. Where [`select_for_report`] would stop on a
/// card number with a non-digit byte, [`ReportSelection::unpinned`] names the
/// same record once the pages are exhausted: the lowest such TRAN-ID, the
/// first in the TRAN-ID order of the backup the sort reads.
#[derive(Debug, Clone)]
pub struct ReportSelection {
    /// The first processing date selected, the `DATEPARM` start (E-34).
    start: Alnum<10>,
    /// The last processing date selected.
    end: Alnum<10>,
    /// The most records one read asks for, at least 1.
    page_rows: u16,
    /// The key of the last record the store returned; `None` before the first
    /// read.
    after: Option<(CardNumber, TranId)>,
    /// The store has returned its last page.
    exhausted: bool,
    /// The lowest TRAN-ID of the kept records whose card number holds a
    /// non-digit byte.
    unpinned: Option<TranId>,
}

impl ReportSelection {
    /// TRANREPT `STEP05R` — P-TRANREPT-002: the selection of the processing
    /// dates `start` to `end` inclusive, read `page_rows` records at a time; a
    /// `page_rows` of 0 reads one record at a time.
    #[must_use]
    pub fn new(start: Alnum<10>, end: Alnum<10>, page_rows: u16) -> Self {
        Self {
            start,
            end,
            page_rows: page_rows.max(1),
            after: None,
            exhausted: false,
            unpinned: None,
        }
    }

    /// TRANREPT `STEP05R` — P-TRANREPT-002; CBTRN03C `1000-TRANFILE-GET-NEXT`
    /// — P-CBTRN03C-474: the next page of the selection, or `None` once the
    /// store has none left.
    ///
    /// A page may be empty when every record the store returned fails the
    /// exact date compare: such records are dropped and never end the
    /// selection. A store page shorter than the page size is the last one.
    ///
    /// # Errors
    ///
    /// The store's [`PortError`]; and [`PortError::Unavailable`] when the
    /// store returns a record whose card number and TRAN-ID are not greater
    /// than those of the record before it. A store that broke that order would
    /// misorder the report, or return the same page forever. The selection is
    /// exhausted after either error.
    pub async fn next_page<S: TransactionReader>(
        &mut self,
        store: &mut S,
    ) -> PortResult<Option<Vec<Transaction>>> {
        if self.exhausted {
            return Ok(None);
        }
        let page = store
            .transactions_by_proc_date(self.start, self.end, self.after, self.page_rows)
            .await
            .inspect_err(|_| self.exhausted = true)?;
        self.exhausted = page.len() < usize::from(self.page_rows);
        if page.is_empty() {
            return Ok(None);
        }
        for tran in &page {
            let key = (tran.tran_card_num, tran.tran_id);
            if self.after.is_some_and(|after| key <= after) {
                self.exhausted = true;
                return Err(PortError::Unavailable(format!(
                    "TRANREPT STEP05R: the selection returned transaction {} out of card number \
                     and TRAN-ID order",
                    tran.tran_id
                )));
            }
            self.after = Some(key);
        }
        let kept: Vec<Transaction> = page
            .into_iter()
            .filter(|tran| in_date_range(&self.start, &self.end, tran))
            .collect();
        let unpinned = kept
            .iter()
            .filter(|tran| !tran.tran_card_num.as_alnum().is_numeric())
            .map(|tran| tran.tran_id)
            .chain(self.unpinned)
            .min();
        self.unpinned = unpinned;
        Ok(Some(kept))
    }

    /// TRANREPT `STEP05R` — P-TRANREPT-002: the record [`select_for_report`]
    /// stops on, a kept record whose card number holds a non-digit byte, with
    /// the lowest TRAN-ID of those the pages have returned so far. Complete
    /// once [`ReportSelection::next_page`] has returned `None`; the stop
    /// applies before any report line, as the sort ends before `CBTRN03C`
    /// starts.
    #[must_use]
    pub fn unpinned(&self) -> Option<UnpinnedSortKey> {
        self.unpinned.map(|tran_id| UnpinnedSortKey { tran_id })
    }
}

/// One run of CBTRN03C over the selected records: the `WS-REPORT-VARS`
/// [app/cbl/CBTRN03C.cbl:L127-137] and the record areas the program carries
/// from one record to the next.
///
/// Feed every record of the input in order to [`ReportRun::process`], then call
/// [`ReportRun::finish`]. The run is spent once a record falls outside the date
/// range (P-CBTRN03C-003) or a lookup fails (P-CBTRN03C-005): from then on
/// `process` and `finish` return no lines, as the COBOL writes none after
/// leaving its loop or abending.
///
/// The run also keeps the `TRANTYPE` and `TRANCATG` records it has read, so
/// each distinct key costs one read per run however many records carry it
/// (P-CBTRN03C-005). The memo belongs to the run and ends with it: a new run,
/// and so every report, reads the processing copies again.
#[derive(Debug, Clone)]
pub struct ReportRun {
    /// `WS-START-DATE X(10)`, from `DATEPARM` (P-CBTRN03C-002, E-34).
    start: Alnum<10>,
    /// `WS-END-DATE X(10)`, from `DATEPARM`.
    end: Alnum<10>,
    /// `WS-LINE-COUNTER PIC 9(09) COMP-3 VALUE 0`.
    line_counter: u32,
    /// `WS-FIRST-TIME PIC X VALUE 'Y'`: no header has been written yet.
    first_time: bool,
    /// `WS-CURR-CARD-NUM X(16) VALUE SPACES`; `None` until the first card
    /// break.
    curr_card: Option<CardNumber>,
    /// `CARD-XREF-RECORD` as `1500-A-LOOKUP-XREF` last read it.
    xref: Option<CardXref>,
    /// CBTRN03C `1500-B-LOOKUP-TRANTYPE` — P-CBTRN03C-005: the `TRANTYPE`
    /// records this run has read, by `FD-TRAN-TYPE`, one per distinct code
    /// and so bounded by the processing copy, not by the input. Filled and
    /// served by `ReportRun::tran_type`.
    tran_types: BTreeMap<TranTypeCode, TranTypeRecord>,
    /// CBTRN03C `1500-C-LOOKUP-TRANCATG` — P-CBTRN03C-005: the `TRANCATG`
    /// records this run has read, by `FD-TRAN-CAT-KEY`, one per distinct key.
    /// Filled and served by `ReportRun::tran_category`.
    tran_categories: BTreeMap<(TranTypeCode, TranCategoryCode), TranCategoryRecord>,
    /// `TRAN-AMT` of the last record read, which the end of the input adds
    /// again (P-CBTRN03C-013).
    last_amount: Option<Money<9>>,
    /// `WS-PAGE-TOTAL PIC S9(09)V99 VALUE 0`.
    page_total: Money<9>,
    /// `WS-ACCOUNT-TOTAL PIC S9(09)V99 VALUE 0`.
    account_total: Money<9>,
    /// `WS-GRAND-TOTAL PIC S9(09)V99 VALUE 0`.
    grand_total: Money<9>,
    /// The read loop has been left or the program has abended.
    stopped: bool,
}

impl ReportRun {
    /// CBTRN03C `0550-DATEPARM-READ` [app/cbl/CBTRN03C.cbl:L220-243] —
    /// P-CBTRN03C-002: a run over the date range `start` to `end`, which head
    /// the report and bound its date filter.
    ///
    /// The `DATEPARM` record is the pair of dates the job passes, the same
    /// dates as its sort selection (E-34), so the end-of-file and read-error
    /// paths of the paragraph have no counterpart.
    #[must_use]
    pub fn new(start: Alnum<10>, end: Alnum<10>) -> Self {
        Self {
            start,
            end,
            line_counter: 0,
            first_time: true,
            curr_card: None,
            xref: None,
            tran_types: BTreeMap::new(),
            tran_categories: BTreeMap::new(),
            last_amount: None,
            page_total: Money::ZERO,
            account_total: Money::ZERO,
            grand_total: Money::ZERO,
            stopped: false,
        }
    }

    /// CBTRN03C main loop [app/cbl/CBTRN03C.cbl:L170-196] — P-CBTRN03C-003,
    /// P-CBTRN03C-004, P-CBTRN03C-474: one record read by
    /// `1000-TRANFILE-GET-NEXT`, returning the report lines it writes, in
    /// order.
    ///
    /// A record whose `TRAN-PROC-TS (1:10)` lies outside the range ends the
    /// run: `NEXT SENTENCE` leaves the loop, so this and every later call write
    /// nothing and [`ReportRun::finish`] writes no totals (P-CBTRN03C-003).
    /// Otherwise a new card number writes the account totals of the previous
    /// card, unless no header has been written yet, and reads its
    /// cross-reference; the type and category are read from the processing
    /// copies, or taken from the run's memo when an earlier record of the run
    /// read the same key; and `1100-WRITE-TRANSACTION-REPORT` writes the detail
    /// line.
    ///
    /// A first record whose card number is all spaces, the `VALUE SPACES` of
    /// `WS-CURR-CARD-NUM`, is looked up like any other: the COBOL would detect
    /// no card change and show an account ID it never read, from
    /// working-storage without a `VALUE` (the class of E-25), so the port reads
    /// the cross-reference instead of guessing those bytes.
    ///
    /// # Errors
    ///
    /// [`ReportAbend`] when a lookup fails (P-CBTRN03C-005). The lines this
    /// record wrote before the failure are not returned: the abend deletes the
    /// report.
    pub async fn process<S>(
        &mut self,
        store: &mut S,
        tran: &Transaction,
    ) -> Result<Vec<String>, ReportAbend>
    where
        S: XrefReader + TranTypeFileReader,
    {
        if self.stopped {
            return Ok(Vec::new());
        }
        if !in_date_range(&self.start, &self.end, tran) {
            self.stopped = true;
            return Ok(Vec::new());
        }
        let written = self.report_record(store, tran).await;
        if written.is_err() {
            self.stopped = true;
        }
        written
    }

    /// CBTRN03C end of input [app/cbl/CBTRN03C.cbl:L197-204] —
    /// P-CBTRN03C-009, P-CBTRN03C-013, P-CBTRN03C-477: the lines written after
    /// the last record.
    ///
    /// `ADD TRAN-AMT TO WS-PAGE-TOTAL WS-ACCOUNT-TOTAL` adds the amount of the
    /// last record read a second time; then `1110-WRITE-PAGE-TOTALS` writes the
    /// page total and a `TRANSACTION-HEADER-2`, and `1110-WRITE-GRAND-TOTALS`
    /// the grand total, the last line of the report. The account total of the
    /// last card, which received the same second addition, is never written.
    ///
    /// An empty input reads no record, so `TRAN-AMT` holds working-storage
    /// without a `VALUE`. The port adds nothing and writes the page total and
    /// grand total of zero with the header line between them, the decision of
    /// escalation E-25. A spent run writes nothing.
    #[must_use]
    pub fn finish(mut self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.stopped {
            return lines;
        }
        if let Some(amount) = self.last_amount {
            self.page_total = (self.page_total + amount).store::<9>();
            self.account_total = (self.account_total + amount).store::<9>();
        }
        self.write_page_totals(&mut lines);
        self.write_grand_totals(&mut lines);
        lines
    }

    /// The in-range path of the main loop [app/cbl/CBTRN03C.cbl:L179-196] —
    /// P-CBTRN03C-004: the card break, the three lookups in the COBOL order
    /// (cross-reference, type, category) and `1100-WRITE-TRANSACTION-REPORT`.
    async fn report_record<S>(
        &mut self,
        store: &mut S,
        tran: &Transaction,
    ) -> Result<Vec<String>, ReportAbend>
    where
        S: XrefReader + TranTypeFileReader,
    {
        let mut lines = Vec::new();
        self.last_amount = Some(tran.tran_amt);
        let xref = self.card_break(store, tran, &mut lines).await?;
        let tran_type = self.tran_type(store, tran.tran_type_cd).await?;
        let category = self
            .tran_category(store, tran.tran_type_cd, tran.tran_cat_cd)
            .await?;
        self.write_transaction_report(tran, &xref, &tran_type, &category, &mut lines);
        Ok(lines)
    }

    /// CBTRN03C `1500-B-LOOKUP-TRANTYPE` [app/cbl/CBTRN03C.cbl:L494-502] —
    /// P-CBTRN03C-005: the `TRANTYPE` record of `code`, read from the
    /// processing copy the first time this run meets the code and taken from
    /// the run's `tran_types` every later time.
    ///
    /// The report is unchanged by the memo. CBTRN03C opens `TRANTYPE` `INPUT`
    /// [app/cbl/CBTRN03C.cbl:L432] and never writes it, so a repeated random
    /// `READ` of a code returns the record the first one did. The first record
    /// that carries a code still reads it at the point the COBOL does, so a
    /// missing code abends at its first occurrence with the same paragraph,
    /// display and key. Only a record that was read is kept: a failed read
    /// spends the run, which reads nothing more.
    async fn tran_type<S: TranTypeFileReader>(
        &mut self,
        store: &mut S,
        code: TranTypeCode,
    ) -> Result<TranTypeRecord, ReportAbend> {
        if let Some(record) = self.tran_types.get(&code) {
            return Ok(record.clone());
        }
        let record = lookup_tran_type(store, code).await?;
        self.tran_types.insert(code, record.clone());
        Ok(record)
    }

    /// CBTRN03C `1500-C-LOOKUP-TRANCATG` [app/cbl/CBTRN03C.cbl:L504-512] —
    /// P-CBTRN03C-005: the `TRANCATG` record of `code` and `category`, read
    /// from the processing copy the first time this run meets the key and taken
    /// from the run's `tran_categories` every later time.
    ///
    /// The report is unchanged for the reasons `ReportRun::tran_type` gives:
    /// `TRANCATG` is opened `INPUT` [app/cbl/CBTRN03C.cbl:L450], the first
    /// occurrence of a key reads it where the COBOL does, and a missing key
    /// abends there with the same paragraph, display and key.
    async fn tran_category<S: TranTypeFileReader>(
        &mut self,
        store: &mut S,
        code: TranTypeCode,
        category: TranCategoryCode,
    ) -> Result<TranCategoryRecord, ReportAbend> {
        if let Some(record) = self.tran_categories.get(&(code, category)) {
            return Ok(record.clone());
        }
        let record = lookup_tran_category(store, code, category).await?;
        self.tran_categories
            .insert((code, category), record.clone());
        Ok(record)
    }

    /// The card break of the main loop [app/cbl/CBTRN03C.cbl:L181-188] —
    /// P-CBTRN03C-004: the cross-reference in effect for `tran`.
    ///
    /// When `WS-CURR-CARD-NUM` differs from `TRAN-CARD-NUM`, the account totals
    /// of the previous card are written unless `WS-FIRST-TIME` is still `Y`,
    /// the card is remembered and `1500-A-LOOKUP-XREF` reads its
    /// cross-reference. Otherwise the cross-reference read for the card stays.
    ///
    /// The run keeps only the current card's cross-reference, not a memo of
    /// every card: the `STEP05R` sort puts all records of a card next to each
    /// other, so one read per card break is already one read per distinct
    /// card, and a map of every card would grow with the input and save no
    /// read.
    async fn card_break<S>(
        &mut self,
        store: &mut S,
        tran: &Transaction,
        lines: &mut Vec<String>,
    ) -> Result<CardXref, ReportAbend>
    where
        S: XrefReader,
    {
        // `None` stands for the initial SPACES but differs from every card, an
        // all-space one included, so the first record always reads its
        // cross-reference rather than show an account ID the COBOL never read.
        let same_card = self.curr_card == Some(tran.tran_card_num);
        if let Some(xref) = self.xref.as_ref().filter(|_| same_card) {
            return Ok(xref.clone());
        }
        if !self.first_time {
            self.write_account_totals(lines);
        }
        self.curr_card = Some(tran.tran_card_num);
        let xref = lookup_xref(store, tran.tran_card_num).await?;
        self.xref = Some(xref.clone());
        Ok(xref)
    }

    /// CBTRN03C `1100-WRITE-TRANSACTION-REPORT` [app/cbl/CBTRN03C.cbl:L274-290]
    /// — P-CBTRN03C-006, P-CBTRN03C-010: the headers before the first detail,
    /// the page totals and headers whenever `MOD(WS-LINE-COUNTER, WS-PAGE-SIZE)
    /// = 0`, the amount added to the page and account totals, then the detail.
    fn write_transaction_report(
        &mut self,
        tran: &Transaction,
        xref: &CardXref,
        tran_type: &TranTypeRecord,
        category: &TranCategoryRecord,
        lines: &mut Vec<String>,
    ) {
        if self.first_time {
            self.first_time = false;
            self.write_headers(lines);
        }
        if self.line_counter % PAGE_SIZE == 0 {
            self.write_page_totals(lines);
            self.write_headers(lines);
        }
        self.page_total = (self.page_total + tran.tran_amt).store::<9>();
        self.account_total = (self.account_total + tran.tran_amt).store::<9>();
        self.write_detail(tran, xref, tran_type, category, lines);
    }

    /// CBTRN03C `1110-WRITE-PAGE-TOTALS` [app/cbl/CBTRN03C.cbl:L293-304] —
    /// P-CBTRN03C-008, P-CBTRN03C-011, P-CBTRN03C-486: the page total, which is
    /// then added to the grand total and reset, and a `TRANSACTION-HEADER-2`,
    /// each counted.
    fn write_page_totals(&mut self, lines: &mut Vec<String>) {
        lines.push(format::report_page_total(self.page_total));
        self.grand_total = (self.grand_total + self.page_total).store::<9>();
        self.page_total = Money::ZERO;
        self.count_line();
        lines.push(format::report_header_2());
        self.count_line();
    }

    /// CBTRN03C `1120-WRITE-ACCOUNT-TOTALS` [app/cbl/CBTRN03C.cbl:L306-316] —
    /// P-CBTRN03C-008, P-CBTRN03C-012: the account total, which is then reset,
    /// and a `TRANSACTION-HEADER-2`, each counted.
    fn write_account_totals(&mut self, lines: &mut Vec<String>) {
        lines.push(format::report_account_total(self.account_total));
        self.account_total = Money::ZERO;
        self.count_line();
        lines.push(format::report_header_2());
        self.count_line();
    }

    /// CBTRN03C `1110-WRITE-GRAND-TOTALS` [app/cbl/CBTRN03C.cbl:L318-322] —
    /// P-CBTRN03C-477: the grand total, not counted.
    fn write_grand_totals(&self, lines: &mut Vec<String>) {
        lines.push(format::report_grand_total(self.grand_total));
    }

    /// CBTRN03C `1120-WRITE-HEADERS` [app/cbl/CBTRN03C.cbl:L324-341] —
    /// P-CBTRN03C-475: `REPORT-NAME-HEADER` with the `DATEPARM` dates,
    /// `WS-BLANK-LINE`, `TRANSACTION-HEADER-1` and `TRANSACTION-HEADER-2`, each
    /// counted.
    fn write_headers(&mut self, lines: &mut Vec<String>) {
        let header = format::report_name_header(&self.start.as_text(), &self.end.as_text());
        for line in [
            header,
            format::report_blank_line(),
            format::report_header_1(),
            format::report_header_2(),
        ] {
            lines.push(line);
            self.count_line();
        }
    }

    /// CBTRN03C `1120-WRITE-DETAIL` [app/cbl/CBTRN03C.cbl:L361-374] —
    /// P-CBTRN03C-007, P-CBTRN03C-014: `TRANSACTION-DETAIL-REPORT` from the
    /// record, the account ID of its cross-reference and the descriptions of
    /// its type and category, counted.
    fn write_detail(
        &mut self,
        tran: &Transaction,
        xref: &CardXref,
        tran_type: &TranTypeRecord,
        category: &TranCategoryRecord,
        lines: &mut Vec<String>,
    ) {
        let tran_id = tran.tran_id.as_alnum().as_text();
        let type_cd = tran.tran_type_cd.as_alnum().as_text();
        let type_desc = tran_type.tran_type_desc.as_text();
        let cat_desc = category.tran_cat_type_desc.as_text();
        let source = tran.tran_source.as_text();
        lines.push(format::report_detail_line(&ReportDetail {
            tran_id: &tran_id,
            account_id: xref.xref_acct_id.get(),
            type_cd: &type_cd,
            type_desc: &type_desc,
            cat_cd: tran.tran_cat_cd.get(),
            cat_desc: &cat_desc,
            source: &source,
            amount: tran.tran_amt,
        }));
        self.count_line();
    }

    /// CBTRN03C `1110-WRITE-PAGE-TOTALS` [app/cbl/CBTRN03C.cbl:L299, L302],
    /// `1120-WRITE-ACCOUNT-TOTALS` [app/cbl/CBTRN03C.cbl:L311, L314],
    /// `1120-WRITE-HEADERS` [app/cbl/CBTRN03C.cbl:L327, L331, L335, L339] and
    /// `1120-WRITE-DETAIL` [app/cbl/CBTRN03C.cbl:L373] — P-CBTRN03C-006,
    /// P-CBTRN03C-008, P-CBTRN03C-475, P-CBTRN03C-007: the
    /// `ADD 1 TO WS-LINE-COUNTER` after every counted line, keeping the nine
    /// low-order digits of `WS-LINE-COUNTER PIC 9(09) COMP-3`
    /// [app/cbl/CBTRN03C.cbl:L129-130], as the statement has no `ON SIZE ERROR`.
    fn count_line(&mut self) {
        self.line_counter = (self.line_counter + 1) % LINE_COUNTER_LIMIT;
    }
}

/// CBTRN03C `1500-A-LOOKUP-XREF` [app/cbl/CBTRN03C.cbl:L484-492] —
/// P-CBTRN03C-005: the cross-reference of `card`.
async fn lookup_xref<S: XrefReader>(
    store: &mut S,
    card: CardNumber,
) -> Result<CardXref, ReportAbend> {
    store
        .read_xref_by_card(card)
        .await
        .map_err(|cause| ReportAbend {
            paragraph: "1500-A-LOOKUP-XREF",
            display: messages::INVALID_CARD_NUMBER,
            key: LookupKey::Card(card),
            cause,
        })
}

/// CBTRN03C `1500-B-LOOKUP-TRANTYPE` [app/cbl/CBTRN03C.cbl:L494-502] —
/// P-CBTRN03C-005: the `TRANTYPE` processing record of `code`.
async fn lookup_tran_type<S: TranTypeFileReader>(
    store: &mut S,
    code: TranTypeCode,
) -> Result<TranTypeRecord, ReportAbend> {
    store
        .read_tran_type_file(code)
        .await
        .map_err(|cause| ReportAbend {
            paragraph: "1500-B-LOOKUP-TRANTYPE",
            display: messages::INVALID_TRANSACTION_TYPE,
            key: LookupKey::TranType(code),
            cause,
        })
}

/// CBTRN03C `1500-C-LOOKUP-TRANCATG` [app/cbl/CBTRN03C.cbl:L504-512] —
/// P-CBTRN03C-005: the `TRANCATG` processing record of `code` and `category`.
async fn lookup_tran_category<S: TranTypeFileReader>(
    store: &mut S,
    code: TranTypeCode,
    category: TranCategoryCode,
) -> Result<TranCategoryRecord, ReportAbend> {
    store
        .read_tran_category_file(code, category)
        .await
        .map_err(|cause| ReportAbend {
            paragraph: "1500-C-LOOKUP-TRANCATG",
            display: messages::INVALID_TRAN_CATG_KEY,
            key: LookupKey::TranCategory(code, category),
            cause,
        })
}

#[cfg(test)]
mod tests {
    use super::{
        LookupKey, PAGE_SIZE, ReportAbend, ReportRun, ReportSortKey, UnpinnedSortKey,
        report_sort_key, select_for_report,
    };
    use crate::entities::{CardXref, TranCategoryRecord, TranTypeRecord, Transaction};
    use crate::entities::{TranCategoryMaster, TranTypeMaster};
    use crate::format::{self, REPORT_RECORD_LEN, ReportDetail};
    use crate::messages;
    use crate::numeric::{
        AccountId, Alnum, CardNumber, CustomerId, MerchantId, Money, TranCategoryCode, TranId,
        TranTypeCode, TransactionRowId,
    };
    use crate::ports::PortError;
    use crate::rules::test_support::{FakeStore, block_on};

    /// Account 50 in the audit's report lines (P-CBTRN03C-020 to -026).
    const CARD_A: &str = "4111000000000050";
    /// Account 27 (P-CBTRN03C-028 to -034).
    const CARD_B: &str = "4111000000000027";
    /// The TRANREPT default range [app/jcl/TRANREPT.jcl] (P-TRANREPT-002).
    const START: &str = "2022-01-01";
    const END: &str = "2022-07-06";
    /// The processing date POSTTRAN stamps with the clock pinned to
    /// 2022-07-01T12:00:00Z (audit section D.6).
    const IN_RANGE: &str = "2022-07-01";

    fn date(text: &str) -> Alnum<10> {
        Alnum::move_from(text)
    }

    fn run() -> ReportRun {
        ReportRun::new(date(START), date(END))
    }

    fn category(value: u16) -> TranCategoryCode {
        TranCategoryCode::new(value).expect("a four-digit category")
    }

    fn money(cents: i64) -> Money<9> {
        Money::from_cents(cents).expect("an amount of S9(09)V99")
    }

    fn xref(card: &str, account: u64) -> CardXref {
        CardXref {
            xref_card_num: CardNumber::move_from(card),
            xref_cust_id: CustomerId::new(1).expect("a customer number"),
            xref_acct_id: AccountId::new(account).expect("an account number"),
        }
    }

    /// The processing copies as `batch seed` loads them from the fixtures
    /// [app/data/ASCII/trantype.txt] [app/data/ASCII/trancatg.txt], and the
    /// cross-references of the two cards.
    fn store() -> FakeStore {
        let type_record = |code: &str, desc: &str| TranTypeRecord {
            tran_type: TranTypeCode::move_from(code),
            tran_type_desc: Alnum::move_from(desc),
        };
        let category_record = |code: &str, cat: u16, desc: &str| TranCategoryRecord {
            tran_type_cd: TranTypeCode::move_from(code),
            tran_cat_cd: category(cat),
            tran_cat_type_desc: Alnum::move_from(desc),
        };
        FakeStore::default()
            .with_xref(xref(CARD_A, 50))
            .with_xref(xref(CARD_B, 27))
            .with_tran_type_record(type_record("01", "Purchase"))
            .with_tran_type_record(type_record("03", "Credit"))
            .with_tran_category_record(category_record("01", 1, "Regular Sales Draft"))
            .with_tran_category_record(category_record("03", 1, "Credit to Account"))
    }

    fn tran(id: &str, card: &str, cents: i64) -> Transaction {
        tran_on(id, card, cents, IN_RANGE)
    }

    fn tran_on(id: &str, card: &str, cents: i64, proc_date: &str) -> Transaction {
        Transaction {
            transaction_id: TransactionRowId::from(1),
            tran_id: TranId::move_from(id),
            tran_type_cd: TranTypeCode::move_from("01"),
            tran_cat_cd: category(1),
            tran_source: Alnum::move_from("POS TERM"),
            tran_desc: Alnum::move_from("Purchase at Abshire-Lowe"),
            tran_amt: money(cents),
            tran_merchant_id: MerchantId::new(800_000_000).expect("a merchant number"),
            tran_merchant_name: Alnum::move_from("Abshire-Lowe"),
            tran_merchant_city: Alnum::move_from("North Enoshaven"),
            tran_merchant_zip: Alnum::move_from("72112"),
            tran_card_num: CardNumber::move_from(card),
            tran_orig_ts: Alnum::move_from("2022-06-10 19:27:53.000000"),
            tran_proc_ts: Alnum::move_from(&format!("{proc_date} 12:00:00.000000")),
        }
    }

    /// The detail line the `CVTRA07Y` builder writes for a purchase.
    fn detail(tran: &Transaction, account: u64) -> String {
        format::report_detail_line(&ReportDetail {
            tran_id: &tran.tran_id.trimmed(),
            account_id: account,
            type_cd: "01",
            type_desc: "Purchase",
            cat_cd: 1,
            cat_desc: "Regular Sales Draft",
            source: "POS TERM",
            amount: tran.tran_amt,
        })
    }

    /// The four lines of `1120-WRITE-HEADERS` for the default range.
    fn headers() -> Vec<String> {
        vec![
            format::report_name_header(START, END),
            format::report_blank_line(),
            format::report_header_1(),
            format::report_header_2(),
        ]
    }

    /// The lines of a whole run over `trans`: every `process`, then `finish`.
    fn report(store: &mut FakeStore, trans: &[Transaction]) -> Result<Vec<String>, ReportAbend> {
        let mut run = run();
        let mut lines = Vec::new();
        for tran in trans {
            lines.extend(block_on(run.process(store, tran))?);
        }
        lines.extend(run.finish());
        Ok(lines)
    }

    /// Section D of the parity audit, which holds the expected report lines.
    const AUDIT: &str = include_str!("../../../../docs/parity/parity-oracle-audit.md");

    /// The D.6 report lines numbered 1 to `last`, each right-trimmed in the
    /// audit and padded here to the 133-byte record (P-CBTRN03C-015).
    fn audit_lines(last: usize) -> Vec<String> {
        let mut numbered: Vec<(usize, String)> = AUDIT
            .lines()
            .filter(|row| row.starts_with("| P-CBTRN03C-"))
            .filter_map(|row| {
                let line_no = row.split("line_no=").nth(1)?.split(';').next()?;
                let text = row
                    .split("text=`\"")
                    .nth(1)?
                    .split("\"` width=133")
                    .next()?;
                Some((
                    line_no.parse().ok()?,
                    format::move_alnum(text, REPORT_RECORD_LEN),
                ))
            })
            .filter(|(line_no, _)| *line_no <= last)
            .collect();
        numbered.sort_by_key(|(line_no, _)| *line_no);
        let numbers: Vec<usize> = numbered.iter().map(|(line_no, _)| *line_no).collect();
        assert_eq!(
            numbers,
            (1..=last).collect::<Vec<_>>(),
            "D.6 lines 1 to {last}"
        );
        numbered.into_iter().map(|(_, text)| text).collect()
    }

    /// The record and cross-reference behind an audit detail line, read back
    /// from the `CVTRA07Y` detail columns; the card number is the account ID
    /// in 16 digits, since the report does not show it.
    fn record_of(line: &str) -> (Transaction, CardXref) {
        let field = |from: usize, to: usize| line.get(from..to).expect("a detail column");
        let account: u64 = field(17, 28).parse().expect("an 11-digit account");
        let card = format!("{account:016}");
        let edited = field(97, 112);
        let digits: String = edited.chars().filter(char::is_ascii_digit).collect();
        let magnitude: i64 = digits.parse().expect("an edited amount");
        let cents = if edited.contains('-') {
            -magnitude
        } else {
            magnitude
        };
        let record = Transaction {
            tran_type_cd: TranTypeCode::move_from(field(29, 31)),
            tran_cat_cd: field(48, 52).parse().expect("a four-digit category"),
            tran_source: Alnum::move_from(field(83, 93)),
            ..tran(field(0, 16), &card, cents)
        };
        (record, xref(&card, account))
    }

    #[test]
    fn the_first_page_matches_the_audit_report_byte_for_byte() {
        // P-CBTRN03C-016 to P-CBTRN03C-042: the header block, the details of
        // accounts 50 and 27 with their account totals, the page total of
        // line 21 and the next page's header block before line 27.
        let expected = audit_lines(27);
        let mut store = store();
        let mut records = Vec::new();
        let is_detail = |line: &&String| {
            line.get(..16)
                .is_some_and(|tran_id| tran_id.bytes().all(|byte| byte.is_ascii_digit()))
        };
        for line in expected.iter().filter(is_detail) {
            let (record, card_xref) = record_of(line);
            store = store.with_xref(card_xref);
            records.push(record);
        }
        assert_eq!(
            records.len(),
            13,
            "twelve details of page 1 and one of page 2"
        );

        let mut run = run();
        let mut lines = Vec::new();
        for record in &records {
            lines.extend(block_on(run.process(&mut store, record)).expect("a detail"));
        }
        assert_eq!(lines, expected);
        assert!(lines.iter().all(|line| line.len() == REPORT_RECORD_LEN));
    }

    #[test]
    fn two_cards_write_one_account_total_and_count_the_last_amount_twice() {
        // P-CBTRN03C-004, -008, -009, -013: the account total of card A at the
        // card break, none for the last card; the last amount (10.25) is added
        // to the page total again at the end, which the grand total takes.
        let trans = [
            tran("0000000000000001", CARD_A, 10_000),
            tran("0000000000000002", CARD_A, 5_000),
            tran("0000000000000003", CARD_B, 2_550),
            tran("0000000000000004", CARD_B, 1_025),
        ];
        let lines = report(&mut store(), &trans).expect("a report");

        let mut expected = headers();
        expected.push(detail(&trans[0], 50));
        expected.push(detail(&trans[1], 50));
        expected.push(format::report_account_total(money(15_000)));
        expected.push(format::report_header_2());
        expected.push(detail(&trans[2], 27));
        expected.push(detail(&trans[3], 27));
        expected.push(format::report_page_total(money(19_600)));
        expected.push(format::report_header_2());
        expected.push(format::report_grand_total(money(19_600)));
        assert_eq!(lines, expected);

        let account_totals = lines
            .iter()
            .filter(|line| line.starts_with("Account Total"))
            .count();
        assert_eq!(account_totals, 1, "the last card's total is never written");
    }

    #[test]
    fn a_page_breaks_when_the_line_counter_reaches_a_multiple_of_the_page_size() {
        // P-CBTRN03C-006, -011: headers count 4, so the 16th detail brings the
        // counter to 20 and the 17th is preceded by the page total, a header 2
        // and a new header block.
        assert_eq!(PAGE_SIZE, 20);
        let mut store = store();
        let mut run = run();
        let trans: Vec<Transaction> = (1..=17)
            .map(|n| tran(&format!("{n:016}"), CARD_A, 100))
            .collect();
        let mut per_record = Vec::new();
        for tran in &trans {
            per_record.push(block_on(run.process(&mut store, tran)).expect("a detail"));
        }

        let mut first = headers();
        first.push(detail(&trans[0], 50));
        assert_eq!(per_record[0], first);
        assert!(per_record[1..16].iter().all(|lines| lines.len() == 1));

        let mut seventeenth = vec![
            format::report_page_total(money(1_600)),
            format::report_header_2(),
        ];
        seventeenth.extend(headers());
        seventeenth.push(detail(&trans[16], 50));
        assert_eq!(per_record[16], seventeenth);

        // The page holds 1.00 plus the last amount again; the grand total adds
        // the first page's 16.00.
        assert_eq!(
            run.finish(),
            [
                format::report_page_total(money(200)),
                format::report_header_2(),
                format::report_grand_total(money(1_800)),
            ]
        );
    }

    #[test]
    fn total_lines_count_so_a_card_break_can_step_over_the_page_size() {
        // P-CBTRN03C-006, -008: 15 details leave the counter at 19; the account
        // totals of the card break take it to 21, so no page break comes until
        // the counter reaches 40 before a detail.
        let mut store = store();
        let mut run = run();
        for n in 1..=15 {
            let lines = block_on(run.process(&mut store, &tran(&format!("{n:016}"), CARD_A, 100)))
                .expect("a detail");
            assert_eq!(lines.len(), if n == 1 { 5 } else { 1 });
        }
        let first_b = tran("0000000000000016", CARD_B, 100);
        assert_eq!(
            block_on(run.process(&mut store, &first_b)).expect("a detail"),
            [
                format::report_account_total(money(1_500)),
                format::report_header_2(),
                detail(&first_b, 27),
            ]
        );
        for n in 17..=34 {
            let lines = block_on(run.process(&mut store, &tran(&format!("{n:016}"), CARD_B, 100)))
                .expect("a detail");
            assert_eq!(lines.len(), 1, "record {n} is a detail only");
        }
        let breaking = block_on(run.process(&mut store, &tran("0000000000000035", CARD_B, 100)))
            .expect("a detail");
        assert_eq!(breaking[0], format::report_page_total(money(3_400)));
        assert_eq!(breaking.len(), 7);
    }

    #[test]
    fn the_first_record_outside_the_range_ends_the_report_without_totals() {
        // P-CBTRN03C-003: NEXT SENTENCE leaves the read loop; nothing after it
        // is read or written. The range bounds are inclusive.
        for (proc_date, in_range) in [
            ("2021-12-31", false),
            (START, true),
            (END, true),
            ("2022-07-07", false),
            ("          ", false),
        ] {
            let mut store = store();
            let mut run = run();
            let lines = block_on(run.process(
                &mut store,
                &tran_on("0000000000000001", CARD_A, 100, proc_date),
            ))
            .expect("no lookup fails");
            assert_eq!(lines.is_empty(), !in_range, "{proc_date:?}");
            if in_range {
                continue;
            }
            let later = block_on(run.process(&mut store, &tran("0000000000000002", CARD_A, 100)))
                .expect("no lookup is made");
            assert!(later.is_empty(), "{proc_date:?}");
            assert!(
                store.calls.is_empty(),
                "{proc_date:?}: no record is looked up"
            );
            assert!(run.finish().is_empty(), "{proc_date:?}: no totals");
        }
    }

    #[test]
    fn a_failed_lookup_abends_with_its_paragraph_display_and_key() {
        // P-CBTRN03C-005: INVALID KEY on each of the three lookups.
        let unknown_type = Transaction {
            tran_type_cd: TranTypeCode::move_from("09"),
            ..tran("0000000000000001", CARD_A, 100)
        };
        let unknown_category = Transaction {
            tran_cat_cd: category(99),
            ..tran("0000000000000001", CARD_A, 100)
        };
        let unknown_card = "4111999999999999";
        let cases = [
            (
                tran("0000000000000001", unknown_card, 100),
                "1500-A-LOOKUP-XREF",
                messages::INVALID_CARD_NUMBER,
                LookupKey::Card(CardNumber::move_from(unknown_card)),
                "INVALID CARD NUMBER : 4111999999999999",
            ),
            (
                unknown_type,
                "1500-B-LOOKUP-TRANTYPE",
                messages::INVALID_TRANSACTION_TYPE,
                LookupKey::TranType(TranTypeCode::move_from("09")),
                "INVALID TRANSACTION TYPE : 09",
            ),
            (
                unknown_category,
                "1500-C-LOOKUP-TRANCATG",
                messages::INVALID_TRAN_CATG_KEY,
                LookupKey::TranCategory(TranTypeCode::move_from("01"), category(99)),
                "INVALID TRAN CATG KEY : 010099",
            ),
        ];
        for (record, paragraph, display, key, display_line) in cases {
            let mut run = run();
            let abend = block_on(run.process(&mut store(), &record)).expect_err("an abend");
            assert_eq!(
                abend,
                ReportAbend {
                    paragraph,
                    display,
                    key,
                    cause: PortError::NotFound,
                }
            );
            assert_eq!(abend.display_line(), display_line);
            assert!(!abend.to_string().contains("4111"), "{abend}");
            assert!(
                run.finish().is_empty(),
                "{paragraph}: the abend writes no totals"
            );
        }
    }

    #[test]
    fn a_failed_read_abends_with_its_cause_and_masks_the_card_in_debug() {
        let mut store = store();
        store.fail_next(
            "read_tran_type_file",
            PortError::Unavailable("connection reset".to_owned()),
        );
        let mut run = run();
        let abend = block_on(run.process(&mut store, &tran("0000000000000001", CARD_A, 100)))
            .expect_err("an abend");
        assert_eq!(abend.paragraph, "1500-B-LOOKUP-TRANTYPE");
        assert_eq!(
            abend.cause,
            PortError::Unavailable("connection reset".to_owned())
        );
        assert_eq!(
            abend.to_string(),
            "CBTRN03C 1500-B-LOOKUP-TRANTYPE: INVALID TRANSACTION TYPE"
        );
        let card = LookupKey::Card(CardNumber::move_from(CARD_A));
        assert!(!format!("{card:?}").contains(CARD_A));
        assert_eq!(card.to_string(), CARD_A);
    }

    /// A purchase record of `id` changed to type `type_cd` and category `cat`.
    fn tran_of(id: &str, card: &str, cents: i64, type_cd: &str, cat: u16) -> Transaction {
        Transaction {
            tran_type_cd: TranTypeCode::move_from(type_cd),
            tran_cat_cd: category(cat),
            ..tran(id, card, cents)
        }
    }

    /// The detail line of `tran` with the given account and descriptions.
    fn detail_of(tran: &Transaction, account: u64, type_desc: &str, cat_desc: &str) -> String {
        format::report_detail_line(&ReportDetail {
            tran_id: &tran.tran_id.trimmed(),
            account_id: account,
            type_cd: &tran.tran_type_cd.to_string(),
            type_desc,
            cat_cd: tran.tran_cat_cd.get(),
            cat_desc,
            source: "POS TERM",
            amount: tran.tran_amt,
        })
    }

    #[test]
    fn each_distinct_type_category_and_card_is_read_once_per_run() {
        // P-CBTRN03C-004, -005: three cards carrying two types and three
        // (type, category) keys, most of them repeated. The first record with
        // a key reads it where the COBOL does; every later one takes the
        // record the run has kept, and the lines are those of a read per
        // record.
        let card_c = "4111000000000013";
        let mut store = store()
            .with_xref(xref(card_c, 13))
            .with_tran_category_record(TranCategoryRecord {
                tran_type_cd: TranTypeCode::move_from("01"),
                tran_cat_cd: category(2),
                tran_cat_type_desc: Alnum::move_from("Regular Cash Advance"),
            });
        let trans = [
            tran_of("0000000000000001", CARD_A, 100, "01", 1),
            tran_of("0000000000000002", CARD_A, 200, "03", 1),
            tran_of("0000000000000003", CARD_A, 300, "01", 2),
            tran_of("0000000000000004", CARD_B, 400, "01", 1),
            tran_of("0000000000000005", CARD_B, 500, "03", 1),
            tran_of("0000000000000006", card_c, 600, "01", 2),
            tran_of("0000000000000007", card_c, 700, "01", 1),
        ];
        let lines = report(&mut store, &trans).expect("a report");

        let mut expected = headers();
        expected.extend([
            detail_of(&trans[0], 50, "Purchase", "Regular Sales Draft"),
            detail_of(&trans[1], 50, "Credit", "Credit to Account"),
            detail_of(&trans[2], 50, "Purchase", "Regular Cash Advance"),
            format::report_account_total(money(600)),
            format::report_header_2(),
            detail_of(&trans[3], 27, "Purchase", "Regular Sales Draft"),
            detail_of(&trans[4], 27, "Credit", "Credit to Account"),
            format::report_account_total(money(900)),
            format::report_header_2(),
            detail_of(&trans[5], 13, "Purchase", "Regular Cash Advance"),
            detail_of(&trans[6], 13, "Purchase", "Regular Sales Draft"),
            format::report_page_total(money(3_500)),
            format::report_header_2(),
            format::report_grand_total(money(3_500)),
        ]);
        assert_eq!(lines, expected);
        assert_eq!(
            store.calls,
            [
                // Record 1: card A, type 01, key 01/0001.
                "read_xref_by_card",
                "read_tran_type_file",
                "read_tran_category_file",
                // Record 2: type 03, key 03/0001.
                "read_tran_type_file",
                "read_tran_category_file",
                // Record 3: key 01/0002.
                "read_tran_category_file",
                // Records 4 and 6: the card breaks of B and C.
                "read_xref_by_card",
                "read_xref_by_card",
            ]
        );
    }

    #[test]
    fn a_new_run_reads_the_processing_copies_again() {
        // AAP 0.3.2: `batch seed --only tranextr` copies an edited master over
        // TRANTYPE between two reports; the second report shows the new
        // description, because nothing a run kept outlives it.
        let trans = [
            tran("0000000000000001", CARD_A, 100),
            tran("0000000000000002", CARD_A, 200),
        ];
        let mut store = store();
        let first = report(&mut store, &trans).expect("a report");
        assert!(first.contains(&detail(&trans[1], 50)));

        store = store.with_tran_type_record(TranTypeRecord {
            tran_type: TranTypeCode::move_from("01"),
            tran_type_desc: Alnum::move_from("PURCHASE"),
        });
        store.calls.clear();
        let second = report(&mut store, &trans).expect("a report");
        assert!(second.contains(&detail_of(&trans[1], 50, "PURCHASE", "Regular Sales Draft")));
        assert_eq!(
            store.calls,
            [
                "read_xref_by_card",
                "read_tran_type_file",
                "read_tran_category_file",
            ]
        );
    }

    #[test]
    fn a_key_first_met_after_kept_keys_abends_at_its_first_occurrence() {
        // P-CBTRN03C-005: records 1 to 3 leave types 01 and 03 and keys
        // 01/0001 and 03/0001 kept. A key none of them carried is read at its
        // first occurrence, from the processing copy only (AAP 0.3.2), and
        // abends there; the spent run reads and writes nothing more.
        let type_master = TranTypeMaster {
            tr_type: TranTypeCode::move_from("02"),
            tr_description: "PAYMENT".to_owned(),
        };
        let category_master = TranCategoryMaster {
            trc_type_code: TranTypeCode::move_from("01"),
            trc_type_category: category(2),
            trc_cat_data: Alnum::move_from("REGULAR CASH ADVANCE"),
        };
        let category_key = LookupKey::TranCategory(TranTypeCode::move_from("01"), category(2));
        let cases = [
            // In neither version.
            (
                store(),
                ("01", 2),
                "1500-C-LOOKUP-TRANCATG",
                messages::INVALID_TRAN_CATG_KEY,
                category_key,
                "read_tran_category_file",
            ),
            // Only in the Db2 category master.
            (
                store().with_tran_category(category_master),
                ("01", 2),
                "1500-C-LOOKUP-TRANCATG",
                messages::INVALID_TRAN_CATG_KEY,
                category_key,
                "read_tran_category_file",
            ),
            // Only in the Db2 type master.
            (
                store().with_tran_type(type_master),
                ("02", 1),
                "1500-B-LOOKUP-TRANTYPE",
                messages::INVALID_TRANSACTION_TYPE,
                LookupKey::TranType(TranTypeCode::move_from("02")),
                "read_tran_type_file",
            ),
        ];
        for (mut store, (type_cd, cat), paragraph, display, key, failed_read) in cases {
            let mut run = run();
            for record in [
                tran("0000000000000001", CARD_A, 100),
                tran_of("0000000000000002", CARD_A, 100, "03", 1),
                tran("0000000000000003", CARD_A, 100),
            ] {
                block_on(run.process(&mut store, &record)).expect("a detail");
            }
            store.calls.clear();

            let first = tran_of("0000000000000004", CARD_B, 100, type_cd, cat);
            let abend = block_on(run.process(&mut store, &first)).expect_err("an abend");
            assert_eq!(
                abend,
                ReportAbend {
                    paragraph,
                    display,
                    key,
                    cause: PortError::NotFound,
                }
            );
            assert_eq!(
                store.calls,
                ["read_xref_by_card", failed_read],
                "{paragraph}: only the card break and the new key are read"
            );

            store.calls.clear();
            let later = block_on(run.process(&mut store, &tran("0000000000000005", CARD_B, 100)))
                .expect("a spent run");
            assert!(later.is_empty(), "{paragraph}: no lines after the abend");
            assert!(store.calls.is_empty(), "{paragraph}: no reads after it");
            assert!(run.finish().is_empty(), "{paragraph}: no totals");
        }
    }

    #[test]
    fn an_empty_input_writes_zero_page_and_grand_totals() {
        // E-25 decides the empty input; P-CBTRN03C-012: a zero total edits to
        // 15 spaces under +ZZZ,ZZZ,ZZZ.ZZ.
        let lines = run().finish();
        assert_eq!(
            lines,
            [
                format::report_page_total(Money::ZERO),
                format::report_header_2(),
                format::report_grand_total(Money::ZERO),
            ]
        );
        let page_total = format!("Page Total {}", ".".repeat(86));
        assert_eq!(lines[0], format::move_alnum(&page_total, REPORT_RECORD_LEN));
    }

    #[test]
    fn totals_keep_the_low_order_nine_integer_digits() {
        // P-CBTRN03C-010, -011, -013: every total is S9(09)V99 without ON SIZE
        // ERROR, so 999,999,999.99 twice is stored as 999,999,999.98, and the
        // last amount added again as 999,999,999.97.
        let top = 99_999_999_999;
        let trans = [
            tran("0000000000000001", CARD_A, top),
            tran("0000000000000002", CARD_A, top),
        ];
        let lines = report(&mut store(), &trans).expect("a report");
        let tail = &lines[lines.len() - 3..];
        assert_eq!(
            tail,
            [
                format::report_page_total(money(99_999_999_997)),
                format::report_header_2(),
                format::report_grand_total(money(99_999_999_997)),
            ]
        );
    }

    #[test]
    fn the_selection_keeps_the_range_and_sorts_by_card_then_tran_id() {
        // P-TRANREPT-002 with the E-38 order within a card (P-CBTRN03C-015).
        let records = vec![
            tran("0000000000000001", CARD_A, 100),
            tran("0000000000000002", CARD_B, 100),
            tran_on("0000000000000003", CARD_A, 100, "2022-07-07"),
            tran("0000000000000004", CARD_B, 100),
            tran_on("0000000000000005", "41110000000000AB", 100, "2021-12-31"),
            tran("0000000000000000", CARD_A, 100),
        ];
        let selected = select_for_report(&date(START), &date(END), records).expect("all digits");
        let order: Vec<String> = selected
            .iter()
            .map(|tran| format!("{} {}", tran.tran_card_num, tran.tran_id))
            .collect();
        assert_eq!(
            order,
            [
                format!("{CARD_B} 0000000000000002"),
                format!("{CARD_B} 0000000000000004"),
                format!("{CARD_A} 0000000000000000"),
                format!("{CARD_A} 0000000000000001"),
            ]
        );
    }

    #[test]
    fn the_selection_stops_on_a_card_number_with_a_non_digit_byte() {
        let records = vec![
            tran("0000000000000001", CARD_A, 100),
            tran("0000000000000002", "411100000000005 ", 100),
        ];
        assert_eq!(
            select_for_report(&date(START), &date(END), records),
            Err(UnpinnedSortKey {
                tran_id: TranId::move_from("0000000000000002"),
            })
        );
    }

    #[test]
    fn the_sort_key_of_one_record_keeps_the_range_and_stops_only_inside_it() {
        // P-TRANREPT-002: `INCLUDE COND` on the 10-byte `CH` date with both
        // ends inclusive, and a zoned-decimal key pinned only for digits.
        let id = "0000000000000007";
        let unpinned = "41110000000000AB";
        let key = |card: &str| {
            Ok(Some(ReportSortKey {
                card_number: CardNumber::move_from(card),
                tran_id: TranId::move_from(id),
            }))
        };
        let stop = Err(UnpinnedSortKey {
            tran_id: TranId::move_from(id),
        });
        let cases = [
            (START, CARD_A, key(CARD_A)),
            (END, CARD_B, key(CARD_B)),
            ("2021-12-31", CARD_A, Ok(None)),
            ("2022-07-07", CARD_A, Ok(None)),
            (IN_RANGE, unpinned, stop),
            ("2022-07-07", unpinned, Ok(None)),
        ];
        for (proc_date, card, expected) in cases {
            let tran = tran_on(id, card, 100, proc_date);
            assert_eq!(
                report_sort_key(&date(START), &date(END), &tran),
                expected,
                "{proc_date} {card}"
            );
        }
    }

    use super::ReportSelection;
    use crate::ports::{Browse, PortResult, TransactionReader};

    /// How [`Adapter`] bends the date selection of the fake store it wraps.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Bend {
        /// Every call passes through unchanged.
        Exact,
        /// The date range is ignored, as by an adapter whose SQL keeps records
        /// the `CH` compare rejects.
        Widened,
        /// Each page comes back in reverse order.
        Reversed,
        /// `after` is ignored, so every read returns the first page again.
        Restarts,
    }

    /// The fake store as a selection adapter that may break the port contract.
    struct Adapter {
        store: FakeStore,
        bend: Bend,
    }

    impl Adapter {
        /// The fake store holding `records`, the cross-references and the
        /// processing copies of [`store`].
        fn new(records: &[Transaction], bend: Bend) -> Self {
            let store = records
                .iter()
                .fold(store(), |store, tran| store.with_transaction(tran.to_new()));
            Self { store, bend }
        }

        /// The number of date selection reads made.
        fn reads(&self) -> usize {
            self.store
                .calls
                .iter()
                .filter(|call| **call == "transactions_by_proc_date")
                .count()
        }
    }

    impl TransactionReader for Adapter {
        async fn read_transaction(&mut self, id: TranId) -> PortResult<Transaction> {
            self.store.read_transaction(id).await
        }

        async fn browse_transactions(
            &mut self,
            browse: Browse<TranId>,
            account_id: Option<AccountId>,
        ) -> PortResult<Vec<Transaction>> {
            self.store.browse_transactions(browse, account_id).await
        }

        async fn highest_transaction(&mut self) -> PortResult<Option<Transaction>> {
            self.store.highest_transaction().await
        }

        async fn transactions_by_card(&mut self, limit: u16) -> PortResult<Vec<Transaction>> {
            self.store.transactions_by_card(limit).await
        }

        async fn transactions_by_proc_date(
            &mut self,
            start: Alnum<10>,
            end: Alnum<10>,
            after: Option<(CardNumber, TranId)>,
            limit: u16,
        ) -> PortResult<Vec<Transaction>> {
            let (start, end) = if self.bend == Bend::Widened {
                (Alnum::move_from(""), Alnum::move_from_bytes(&[0xFF; 10]))
            } else {
                (start, end)
            };
            let after = after.filter(|_| self.bend != Bend::Restarts);
            let mut page = self
                .store
                .transactions_by_proc_date(start, end, after, limit)
                .await?;
            if self.bend == Bend::Reversed {
                page.reverse();
            }
            Ok(page)
        }
    }

    /// Every page of a selection of the default range over `store`, `rows`
    /// records per read, concatenated.
    fn all_pages(
        store: &mut Adapter,
        rows: u16,
    ) -> (PortResult<Vec<Transaction>>, ReportSelection) {
        let mut selection = ReportSelection::new(date(START), date(END), rows);
        let mut selected = Vec::new();
        loop {
            match block_on(selection.next_page(store)) {
                Ok(Some(page)) => selected.extend(page),
                Ok(None) => return (Ok(selected), selection),
                Err(error) => return (Err(error), selection),
            }
        }
    }

    /// The `STEP05R` sort key of each record: card number, then TRAN-ID.
    fn keys(records: &[Transaction]) -> Vec<(CardNumber, TranId)> {
        records
            .iter()
            .map(|tran| (tran.tran_card_num, tran.tran_id))
            .collect()
    }

    /// Records of three cards in no order, with the range bounds and records
    /// either side of the range.
    fn mixed_records() -> Vec<Transaction> {
        vec![
            tran("0000000000000007", CARD_A, 100),
            tran("0000000000000002", CARD_B, 200),
            tran_on("0000000000000003", CARD_A, 300, "2022-07-07"),
            tran_on("0000000000000004", CARD_B, 400, END),
            tran_on("0000000000000005", CARD_B, 500, "2021-12-31"),
            tran("0000000000000001", CARD_A, 600),
            tran_on("0000000000000008", "4111000000000099", 700, START),
            tran("0000000000000006", "4111000000000099", 800),
            tran_on("0000000000000009", "4111000000000001", 900, "          "),
        ]
    }

    #[test]
    fn the_selection_pages_concatenate_to_the_selection_in_its_order() {
        let records = mixed_records();
        let expected =
            select_for_report(&date(START), &date(END), records.clone()).expect("all digits");
        assert_eq!(expected.len(), 6);
        for bend in [Bend::Exact, Bend::Widened] {
            for rows in [1, 2, 3, 6, 100] {
                let mut store = Adapter::new(&records, bend);
                let (pages, selection) = all_pages(&mut store, rows);
                let pages = pages.expect("a well-behaved store");
                assert_eq!(keys(&pages), keys(&expected), "{bend:?}, {rows} per read");
                assert_eq!(selection.unpinned(), None, "{bend:?}, {rows} per read");
                // One read per full page, and the short or empty read that ends
                // the selection.
                let returned = if bend == Bend::Widened {
                    records.len()
                } else {
                    expected.len()
                };
                let reads = returned / usize::from(rows) + 1;
                assert_eq!(store.reads(), reads, "{bend:?}, {rows} per read");
            }
        }
    }

    #[test]
    fn an_unpinned_card_on_a_later_page_stops_with_the_lowest_tran_id() {
        // P-TRANREPT-002: the non-digit card numbers sort after the digit ones,
        // so they come on the last pages; the out-of-range one never counts.
        let mut records = mixed_records();
        records.extend([
            tran("0000000000000012", "41110000000000AB", 100),
            tran("0000000000000011", "41110000000000AC", 100),
            tran("0000000000000013", "41110000000000AB", 100),
            tran_on("0000000000000010", "41110000000000AA", 100, "2021-12-31"),
        ]);
        let mut in_tran_id_order = records.clone();
        in_tran_id_order.sort_by_key(|tran| tran.tran_id);
        let expected = select_for_report(&date(START), &date(END), in_tran_id_order).err();
        assert_eq!(
            expected,
            Some(UnpinnedSortKey {
                tran_id: TranId::move_from("0000000000000011"),
            })
        );
        for bend in [Bend::Exact, Bend::Widened] {
            for rows in [1, 2, 3] {
                let mut store = Adapter::new(&records, bend);
                let (pages, selection) = all_pages(&mut store, rows);
                assert!(pages.is_ok(), "{bend:?}, {rows} per read");
                assert_eq!(selection.unpinned(), expected, "{bend:?}, {rows} per read");
            }
        }
    }

    #[test]
    fn an_empty_selection_has_no_page() {
        let outside = [
            tran_on("0000000000000001", CARD_A, 100, "2021-12-31"),
            tran_on("0000000000000002", CARD_B, 100, "2022-07-07"),
        ];
        for records in [&[][..], &outside[..]] {
            let mut store = Adapter::new(records, Bend::Exact);
            let mut selection = ReportSelection::new(date(START), date(END), 10);
            assert_eq!(block_on(selection.next_page(&mut store)), Ok(None));
            assert_eq!(block_on(selection.next_page(&mut store)), Ok(None));
            assert_eq!(store.reads(), 1, "{} records", records.len());
        }
    }

    #[test]
    fn a_store_failure_or_a_broken_order_ends_the_selection_with_an_error() {
        let failure = PortError::Unavailable("connection reset".to_owned());
        for pages_before in [0, 2] {
            let mut store = Adapter::new(&mixed_records(), Bend::Exact);
            let mut selection = ReportSelection::new(date(START), date(END), 1);
            for _ in 0..pages_before {
                assert!(matches!(
                    block_on(selection.next_page(&mut store)),
                    Ok(Some(_))
                ));
            }
            store
                .store
                .fail_next("transactions_by_proc_date", failure.clone());
            let result = block_on(selection.next_page(&mut store));
            assert_eq!(result, Err(failure.clone()), "after {pages_before} pages");
            assert_eq!(block_on(selection.next_page(&mut store)), Ok(None));
        }
        for (bend, rows) in [(Bend::Reversed, 3), (Bend::Restarts, 1)] {
            let mut store = Adapter::new(&mixed_records(), bend);
            let (pages, mut selection) = all_pages(&mut store, rows);
            match pages {
                Err(PortError::Unavailable(text)) => assert!(
                    text.contains("out of card number and TRAN-ID order"),
                    "{bend:?}: {text}"
                ),
                other => panic!("{bend:?}: a contract breach, not {other:?}"),
            }
            assert_eq!(block_on(selection.next_page(&mut store)), Ok(None));
        }
    }

    #[test]
    fn the_page_future_is_send_for_a_send_store() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = Adapter::new(&mixed_records(), Bend::Exact);
        let mut selection = ReportSelection::new(date(START), date(END), 0);
        let future = selection.next_page(&mut store);
        assert_send(&future);
        let page = block_on(future).expect("a page").expect("not the end");
        assert_eq!(keys(&page).len(), 1, "a page size of 0 reads one record");
    }

    #[test]
    fn the_record_future_is_send_for_a_send_store() {
        fn assert_send<T: Send>(_: &T) {}
        let mut store = store();
        let mut run = run();
        let record = tran("0000000000000001", CARD_A, 100);
        let future = run.process(&mut store, &record);
        assert_send(&future);
        assert_eq!(block_on(future).expect("a detail").len(), 5);
    }
}
