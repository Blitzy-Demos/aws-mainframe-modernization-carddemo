//! COTRN00C, the transaction list: the rules of endpoint 11, `GET /api/v1/transactions`
//! (transaction `CT00`, map `COTRN0A`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COTRN00C.md`: the first entry
//! (COTRN00C-003, COTRN00C-004), the search edit (COTRN00C-011 to COTRN00C-013), the forward
//! page (COTRN00C-014 to COTRN00C-019), the PF8 and PF7 boundaries (COTRN00C-021,
//! COTRN00C-022), the backward page (COTRN00C-023 to COTRN00C-026), the failed read
//! (COTRN00C-027), the row fields (COTRN00C-028 to COTRN00C-034) and the one message a request
//! ends with (COTRN00C-035). The `client` rows send no request and belong to the screen: the
//! sign-on redirect, PF3, an unlabelled key, the selectors with the `S` transfer to COTRN01C,
//! the page-number picture, the cursor and the header (COTRN00C-001, COTRN00C-002,
//! COTRN00C-005 to COTRN00C-010, COTRN00C-036 to COTRN00C-039).
//!
//! # Flow
//!
//! `api::services::cotrn00c` passes the key, the raw search text and the echoed
//! [`TranListState`] to [`plan`]. A [`Plan::Answer`] is the response itself: a boundary message
//! that reads nothing. A [`Plan::Read`] holds a [`Browse`]: the service reads at most
//! [`Browse::limit`] records of `transactions` from [`Browse::start`] in [`Browse::direction`]
//! (`queries/transactions_page_forward.sql` ascending from the key, or
//! `queries/transactions_page_backward.sql` descending to it, both inclusive of the key, where
//! `STARTBR` positions), maps each record with [`row_from`] in the order read and hands the rows
//! to [`page`], which decides the rows shown, the keys, the page number, the next-page flag and
//! the message. A failed read is answered with [`lookup_failure`]; the state the screen keeps
//! after any failure is [`TranListState::after_failure`]. This module does no I/O.
//!
//! The program decides every page from its COMMAREA state and the AID, so a request carries
//! both: the [`ListKey`] and the state the previous response returned (X-COTRN00C-1). The state
//! grants nothing, because every request re-reads the table.
//!
//! # Records read
//!
//! [`page`] consumes the records in query order, as the program's browse reads them: the skip
//! read, which drops the first record wherever the program issues one (COTRN00C-004,
//! COTRN00C-015, COTRN00C-023); then up to `page_size` records for the rows; then one read-ahead
//! record. [`Browse::limit`] is that count, `page_size + 1` plus the skipped record (Bounded
//! Pagination), and records past it are never looked at. A `page_size` from 1 to
//! [`MAX_PAGE_SIZE`] replaces the screen's ten rows everywhere the program counts them: the rows
//! of a page, the row that sets the last key and the position of the read-ahead record.
//!
//! # Halts
//!
//! - `HALTED(X-COTRN00C-H1)` (COTRN00C-020): a forward browse that finds no record at or above
//!   its key ends the COBOL task with an `AEIP` abend, so the state is never saved. [`page`]
//!   answers with the last message the program sends before it: `You are at the top of the
//!   page...` with the rows kept, or, where the skip read follows and fails, `Unable to lookup
//!   transaction...` as a lookup failure; [`plan`] answers PF8 with a blank last key, whose
//!   high-values key no record reaches, the same way. Neither is a parity result.
//! - `HALTED(X-COTRN00C-H2)` (COTRN00C-040): after a search that is neither blank nor 16 digits
//!   the COBOL's final message depends on storage nothing assigned. The 400 `Tran ID must be
//!   Numeric ...` of [`edit_start_tran_id`] is the replacement's answer, not a parity result.
//! - `HALTED(X-COTRN00C-H3)` (COTRN00C-030, COTRN00C-033): a blank `TRAN-ORIG-TS` gives the row
//!   no display date, and a negative amount whose kept digits are all zero edits as
//!   `+00000000.00`, as [`EditedPicture::format`] stores it. Neither is a parity result.
//!
//! # Totality
//!
//! Every function takes any text, any key, any state and any rows, and returns a value or a
//! [`TranListFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over Untrusted
//! Input). Amounts are only formatted, never computed (R1).

use core::fmt;

use chrono::{Datelike, NaiveDate, NaiveDateTime};
use copybook::edited::EditedPicture;
use copybook::{PicU, PicX};

use crate::records::transaction::TransactionRecord;

/// Rows of a page when the request names no `page_size`: the ten list rows of `COTRN0A`, screen
/// lines 10 to 19 (COTRN00C-014; Bounded Pagination).
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// The largest `page_size` Bounded Pagination admits. `api` rejects a larger one with 422 before
/// this module runs; [`plan`] clamps any `page_size` into `1..=MAX_PAGE_SIZE`, so a page never
/// reads more than `MAX_PAGE_SIZE + 2` records.
pub const MAX_PAGE_SIZE: usize = 100;

/// Width of `TRNIDINI PIC X(16)`, the search field, and of `TRAN-ID PIC X(16)` (COTRN00C-012).
const TRAN_ID_WIDTH: usize = 16;

/// A search value that is neither blank nor 16 digits (COTRN00C-013).
pub const TRAN_ID_NOT_NUMERIC: &str = "Tran ID must be Numeric ...";

/// A browse command failed; in the replacement, the page query (COTRN00C-027).
pub const LOOKUP_FAILED: &str = "Unable to lookup transaction...";

/// PF7 with the page number not above 1 (COTRN00C-022).
pub const ALREADY_AT_TOP: &str = "You are already at the top of the page...";

/// PF8 with the next-page flag other than `Y` (COTRN00C-021).
pub const ALREADY_AT_BOTTOM: &str = "You are already at the bottom of the page...";

/// A backward page met the start of the file (COTRN00C-025, COTRN00C-026).
pub const REACHED_TOP: &str = "You have reached the top of the page...";

/// A forward page met the end of the file (COTRN00C-017 to COTRN00C-019).
pub const REACHED_BOTTOM: &str = "You have reached the bottom of the page...";

/// `STARTBR` found no record at or above the browse key: the message of the halted path of
/// COTRN00C-020, `HALTED(X-COTRN00C-H1)`.
pub const AT_TOP: &str = "You are at the top of the page...";

/// Page number 1, which `MOVE 1 TO CDEMO-CT00-PAGE-NUM` sets (COTRN00C-025).
const PAGE_ONE: PicU<8> = match PicU::new(1) {
    Ok(page) => page,
    Err(_) => PicU::ZERO,
};

/// Which way a page is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// `PROCESS-PAGE-FORWARD`, `READNEXT` up the key: ENTER, a first entry and PF8
    /// (COTRN00C-014, COTRN00C-015).
    Forward,
    /// `PROCESS-PAGE-BACKWARD`, `READPREV` down the key: PF7 above page 1 (COTRN00C-023).
    Backward,
}

/// The key of the transfer that brought the screen up on a first entry, which `EIBAID` still
/// holds when the program starts (COTRN00C-003, COTRN00C-004, X-COTRN00C-2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryKey {
    /// ENTER on the main menu's option: the page starts at the lowest record (COTRN00C-003).
    Enter,
    /// COTRN01C's F3 back to the list: as [`EntryKey::Enter`] (COTRN00C-003).
    Pf3,
    /// COTRN01C's F5 to the list: the skip read drops the lowest record (COTRN00C-004).
    Pf5,
}

/// The key a request is sent with: the `EIBAID` that `MAIN-PARA` dispatches on (X-COTRN00C-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListKey {
    /// ENTER on the list: the search edit, then a forward page from the search value
    /// (COTRN00C-011 to COTRN00C-014).
    Enter,
    /// PF7: the previous page (COTRN00C-022 to COTRN00C-026).
    Pf7,
    /// PF8: the next page (COTRN00C-015, COTRN00C-019, COTRN00C-021).
    Pf8,
    /// A first entry: `PROCESS-ENTER-KEY` over a cleared map, so the search is blank whatever the
    /// request holds (COTRN00C-003, COTRN00C-004).
    Entry(EntryKey),
}

/// `CDEMO-CT00-NEXT-PAGE-FLG PIC X(01)`: whether PF8 pages forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NextPage {
    /// `Y`, `NEXT-PAGE-YES`: PF8 reads the page after the last key (COTRN00C-015).
    Yes,
    /// Any other value, `N` and the spaces a menu entry leaves included: PF8 answers
    /// [`ALREADY_AT_BOTTOM`] (COTRN00C-002, COTRN00C-021).
    #[default]
    No,
}

impl NextPage {
    /// The flag's byte: `Y` or `N`.
    #[must_use]
    pub const fn code(self) -> char {
        match self {
            Self::Yes => 'Y',
            Self::No => 'N',
        }
    }

    /// The flag a byte holds: [`NextPage::Yes`] for `Y` alone, as `IF NEXT-PAGE-YES` tests it,
    /// and [`NextPage::No`] for every other character (COTRN00C-021).
    #[must_use]
    pub const fn from_code(code: char) -> Self {
        match code {
            'Y' => Self::Yes,
            _ => Self::No,
        }
    }
}

/// The paging state `CDEMO-CT00-INFO` carries from task to task: the first key, the last key, the
/// page number and the next-page flag (COTRN00C-002, X-COTRN00C-1).
///
/// The response returns it and the screen echoes it with its next request. A blank key, spaces or
/// low-values in the COMMAREA, is `None`; [`TranListState::new`] reads a key of 16 spaces as
/// blank, as the program's `= SPACES OR LOW-VALUES` tests do. The default is the state a menu
/// entry starts from: blank keys, page 0 and a flag other than `Y`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct TranListState {
    first_key: Option<PicX<TRAN_ID_WIDTH>>,
    last_key: Option<PicX<TRAN_ID_WIDTH>>,
    page_no: PicU<8>,
    next_page: NextPage,
}

impl TranListState {
    /// The state holding these keys, `CDEMO-CT00-PAGE-NUM PIC 9(08)` and the next-page flag.
    #[must_use]
    pub fn new(
        first_key: Option<PicX<TRAN_ID_WIDTH>>,
        last_key: Option<PicX<TRAN_ID_WIDTH>>,
        page_no: PicU<8>,
        next_page: NextPage,
    ) -> Self {
        Self {
            first_key: non_blank(first_key),
            last_key: non_blank(last_key),
            page_no,
            next_page,
        }
    }

    /// `CDEMO-CT00-TRNID-FIRST`: the ID of row 1 of the last page that filled it, `None` when
    /// blank (COTRN00C-016, COTRN00C-023).
    #[must_use]
    pub const fn first_key(&self) -> Option<&PicX<TRAN_ID_WIDTH>> {
        self.first_key.as_ref()
    }

    /// `CDEMO-CT00-TRNID-LAST`: the ID of the last row of the last page that filled it, `None`
    /// when blank (COTRN00C-016, COTRN00C-018, COTRN00C-026).
    #[must_use]
    pub const fn last_key(&self) -> Option<&PicX<TRAN_ID_WIDTH>> {
        self.last_key.as_ref()
    }

    /// `CDEMO-CT00-PAGE-NUM PIC 9(08)`, which the screen shows as eight digits (COTRN00C-036).
    #[must_use]
    pub const fn page_no(&self) -> PicU<8> {
        self.page_no
    }

    /// `CDEMO-CT00-NEXT-PAGE-FLG`.
    #[must_use]
    pub const fn next_page(&self) -> NextPage {
        self.next_page
    }

    /// The state after a request sent with `key` failed, the input error of [`plan`] or a
    /// [`lookup_failure`]: page number 0 after ENTER and a first entry, which reset it before the
    /// read (line 224), the next-page flag `Y` after PF7, which sets it before the read (line
    /// 242), and otherwise this state unchanged, keys included (COTRN00C-013, COTRN00C-027).
    #[must_use]
    pub fn after_failure(&self, key: ListKey) -> Self {
        match key {
            ListKey::Enter | ListKey::Entry(_) => Self {
                page_no: PicU::ZERO,
                ..self.clone()
            },
            ListKey::Pf7 => Self {
                next_page: NextPage::Yes,
                ..self.clone()
            },
            ListKey::Pf8 => self.clone(),
        }
    }
}

/// `key`, or `None` when it is blank.
fn non_blank(key: Option<PicX<TRAN_ID_WIDTH>>) -> Option<PicX<TRAN_ID_WIDTH>> {
    key.filter(|id| !id.is_spaces())
}

/// The request field a failure names: where the program puts the cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranListField {
    /// `TRNIDIN`, the search field: `TranListQuery.start_tran_id` (COTRN00C-013).
    StartTranId,
}

/// The status class of a [`TranListFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// The search value failed its edit: 400 (COTRN00C-013).
    Input,
    /// The page could not be read: 500 (COTRN00C-027).
    Lookup,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400 for [`FailureClass::Input`], 500 for
    /// [`FailureClass::Lookup`].
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::Lookup => 500,
        }
    }
}

/// A request COTRN00C answers with an error instead of a page: the message, the field in error
/// and the status class.
///
/// The message is the literal moved to `WS-MESSAGE PIC X(80)`, without trailing spaces; every one
/// fits the 78-byte message row. A failure holds no request value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TranListFailure {
    message: &'static str,
    field: Option<TranListField>,
    class: FailureClass,
}

impl TranListFailure {
    /// The search value is neither blank nor 16 digits (COTRN00C-013).
    const fn input() -> Self {
        Self {
            message: TRAN_ID_NOT_NUMERIC,
            field: Some(TranListField::StartTranId),
            class: FailureClass::Input,
        }
    }

    /// A browse command failed (COTRN00C-027): no field, as the 500 `ScreenError` carries none.
    const fn lookup() -> Self {
        Self {
            message: LOOKUP_FAILED,
            field: None,
            class: FailureClass::Lookup,
        }
    }

    /// The message, as the response carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The field in error, `None` for a lookup failure.
    #[must_use]
    pub const fn field(&self) -> Option<TranListField> {
        self.field
    }

    /// The status class.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        self.class
    }
}

impl fmt::Display for TranListFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for TranListFailure {}

/// The answer to a failed page read: 500 with `Unable to lookup transaction...` and no field
/// (COTRN00C-027). The service returns it for any error of the page query, whatever page the
/// screen shows, and the screen keeps its rows; the state it keeps is
/// [`TranListState::after_failure`]. The query returns the whole page or fails, so the partial
/// page the COBOL leaves after a failure inside the browse has no counterpart (X-COTRN00C-7).
#[must_use]
pub const fn lookup_failure() -> TranListFailure {
    TranListFailure::lookup()
}

/// The search field `TRNIDINI PIC X(16)`, edited as `PROCESS-ENTER-KEY` does (COTRN00C-011 to
/// COTRN00C-013).
///
/// `raw` is read as the bytes the map field receives: its first 16 bytes, padded with spaces, as
/// BMS pads a short entry in a field without `NUM`. All spaces or all low-values (`\0`), the empty
/// text included, is blank: `None`, the low-values key from which the page starts at the lowest
/// `TRAN-ID` (COTRN00C-011). Sixteen ASCII digits are the `IS NUMERIC` value, the key the page
/// starts at or above (COTRN00C-012); a shorter entry of digits is padded with spaces and is not
/// numeric.
///
/// # Errors
///
/// A [`TranListFailure`] with [`TRAN_ID_NOT_NUMERIC`], field [`TranListField::StartTranId`] and
/// class [`FailureClass::Input`] (400) for any other value (COTRN00C-013); its final message is
/// `HALTED(X-COTRN00C-H2)` in the COBOL (COTRN00C-040).
pub fn edit_start_tran_id(raw: &str) -> Result<Option<PicX<TRAN_ID_WIDTH>>, TranListFailure> {
    let mut field = [b' '; TRAN_ID_WIDTH];
    // TRUNCATION: bytes past the 16 of the map field are dropped, as the field holds no more.
    for (slot, &byte) in field.iter_mut().zip(raw.as_bytes()) {
        *slot = byte;
    }
    if field.iter().all(|&byte| byte == b' ') || field.iter().all(|&byte| byte == 0) {
        Ok(None)
    } else if field.iter().all(u8::is_ascii_digit) {
        Ok(Some(PicX::from_array(field)))
    } else {
        Err(TranListFailure::input())
    }
}

/// One list row of `COTRN0A` as `POPULATE-TRAN-DATA` fills it (COTRN00C-028 to COTRN00C-034).
///
/// It holds the ID, the typed `TRAN-ORIG-TS`, the eight-character display date, the screen's 26
/// bytes of the description and the edited amount. No field holds the card number, in any form
/// (COTRN00C-034, Cardholder Data Redaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranListRow {
    tran_id: PicX<TRAN_ID_WIDTH>,
    orig_ts: Option<NaiveDateTime>,
    display_date: Option<PicX<8>>,
    description: PicX<26>,
    amount: String,
}

impl TranListRow {
    /// `TRNIDnnI PIC X(16)`: the record's `TRAN-ID` as stored (COTRN00C-028).
    #[must_use]
    pub const fn tran_id(&self) -> &PicX<TRAN_ID_WIDTH> {
        &self.tran_id
    }

    /// `TRAN-ORIG-TS` as the typed timestamp, `None` when the field is blank (COTRN00C-029,
    /// Typed Dates and Timestamps).
    #[must_use]
    pub const fn orig_ts(&self) -> Option<NaiveDateTime> {
        self.orig_ts
    }

    /// The calendar date of `TRAN-ORIG-TS`, `None` when the field is blank (COTRN00C-029).
    #[must_use]
    pub fn date(&self) -> Option<NaiveDate> {
        self.orig_ts.map(|timestamp| timestamp.date())
    }

    /// `TDATEnnI PIC X(8)`: `WS-TRAN-DATE` as `MM/DD/YY`, the date the screen shows; `None` for a
    /// blank `TRAN-ORIG-TS`, whose COBOL display is `HALTED(X-COTRN00C-H3)` (COTRN00C-029,
    /// COTRN00C-030).
    #[must_use]
    pub const fn display_date(&self) -> Option<&PicX<8>> {
        self.display_date.as_ref()
    }

    /// `TDESCnnI PIC X(26)`: the first 26 bytes of `TRAN-DESC PIC X(100)`, trailing spaces
    /// included (COTRN00C-031).
    #[must_use]
    pub const fn description(&self) -> &PicX<26> {
        &self.description
    }

    /// `TAMTnnnI PIC X(12)`: `TRAN-AMT` edited by `WS-TRAN-AMT PIC +99999999.99`, twelve
    /// characters (COTRN00C-032).
    #[must_use]
    pub fn amount(&self) -> &str {
        &self.amount
    }
}

/// The list row of the record `t`, as `POPULATE-TRAN-DATA` moves its fields (COTRN00C-028 to
/// COTRN00C-034).
///
/// The amount passes through `WS-TRAN-AMT PIC +99999999.99`: sign, eight integer digits with
/// leading zeros, the point and two decimals, so an amount of 100,000,000.00 or more in magnitude
/// keeps only its eight low-order integer digits (COTRN00C-032, X-COTRN00C-6). The display date
/// takes the month, the day and the two low-order year digits of the timestamp's date: every
/// layout begins `YYYY-MM-DD` (PIC-018 to PIC-020), so the bytes `(6:2)`, `(9:2)` and `(3:2)` of
/// the text are exactly those (COTRN00C-029).
#[must_use]
pub fn row_from(t: &TransactionRecord) -> TranListRow {
    let orig_ts = t.orig_ts.value().map(|timestamp| timestamp.value());
    TranListRow {
        tran_id: t.id.clone(),
        orig_ts,
        display_date: orig_ts.map(|timestamp| screen_date(timestamp.date())),
        description: PicX::moved_from(t.desc.as_bytes()),
        amount: EditedPicture::Plus9x8Dot99.format(t.amt.as_decimal()),
    }
}

/// `WS-CURDATE-MM-DD-YY`: `MM/DD/YY`, each part two digits with a leading zero. A timestamp's
/// year lies in `0..=9999`, so its remainder by 100 is the two low-order digits of its four.
fn screen_date(date: NaiveDate) -> PicX<8> {
    let text = format!(
        "{:02}/{:02}/{:02}",
        date.month(),
        date.day(),
        date.year().rem_euclid(100)
    );
    PicX::moved_from(text.as_bytes())
}

/// Where a browse starts: the `RIDFLD` of `STARTBR`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BrowseKey {
    /// `LOW-VALUES`: from the lowest `TRAN-ID`, for a blank search (COTRN00C-011) or a blank
    /// first key (COTRN00C-023).
    Lowest,
    /// The search value (COTRN00C-012), the last key (COTRN00C-015) or the first key
    /// (COTRN00C-023).
    From(PicX<TRAN_ID_WIDTH>),
}

impl BrowseKey {
    /// The start the page query takes, a `CHAR(16)` value: the key itself, or 16 spaces for
    /// [`BrowseKey::Lowest`]. No text value holds the low-values bytes, and `CHAR` comparison
    /// ignores trailing spaces, so 16 spaces compare as the empty text, at or below every stored
    /// `tran_id`: the ascending query then starts at the lowest record, as `STARTBR` at
    /// low-values does.
    #[must_use]
    pub fn query_key(&self) -> PicX<TRAN_ID_WIDTH> {
        match self {
            Self::Lowest => PicX::spaces(),
            Self::From(key) => key.clone(),
        }
    }
}

/// Whether the browse issues the skip read that drops the record `STARTBR` positioned on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Skip {
    /// ENTER, and a first entry by ENTER or PF3: no skip read (COTRN00C-003, COTRN00C-014).
    Keep,
    /// PF8, PF7 and a first entry by PF5: `EIBAID` is none of ENTER, PF7 and PF3 on the forward
    /// page, or none of ENTER and PF8 on the backward one (COTRN00C-004, COTRN00C-015,
    /// COTRN00C-023).
    DropFirst,
}

/// A page read [`plan`] asks the service for: where it starts, which way it goes, how many
/// records it needs, and the state the read starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Browse {
    direction: Direction,
    start: BrowseKey,
    skip: Skip,
    page_size: usize,
    state: TranListState,
}

impl Browse {
    /// The direction of the read: ascending for [`Direction::Forward`], descending for
    /// [`Direction::Backward`].
    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    /// The key the read starts at, inclusive of a record with that key.
    #[must_use]
    pub const fn start(&self) -> &BrowseKey {
        &self.start
    }

    /// The rows of the page, `page_size` clamped into `1..=MAX_PAGE_SIZE`.
    #[must_use]
    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// The most records [`page`] consumes, and so the `LIMIT` of the page query: the skipped
    /// record when there is one, the page's rows and the read-ahead record (Bounded Pagination).
    #[must_use]
    pub const fn limit(&self) -> usize {
        let skipped = match self.skip {
            Skip::Keep => 0,
            Skip::DropFirst => 1,
        };
        self.page_size.saturating_add(1).saturating_add(skipped)
    }

    /// The state the read starts from: the echoed state with the page number reset to 0 after
    /// ENTER and a first entry, and the next-page flag set to `Y` after PF7.
    #[must_use]
    pub const fn state(&self) -> &TranListState {
        &self.state
    }
}

/// What a request needs, as [`plan`] decides it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Read a page and pass its records to [`page`].
    Read(Browse),
    /// The response, with nothing to read: PF7 at the top or PF8 at the bottom (COTRN00C-021,
    /// COTRN00C-022).
    Answer(TranPage),
}

/// Decides what a request sent with `key` needs, from its raw search text and the state it echoes
/// (COTRN00C-003, COTRN00C-004, COTRN00C-011 to COTRN00C-015, COTRN00C-021 to COTRN00C-023).
///
/// - ENTER edits `start_tran_id` with [`edit_start_tran_id`], an absent one being blank, sets the
///   page number to 0 and reads forward from the search value without a skip read.
/// - A first entry reads as ENTER with a blank search; by PF5 the skip read drops the lowest
///   record.
/// - PF7 sets the next-page flag to `Y`. Above page 1 it reads backward from the first key,
///   low-values when blank, with a skip read; otherwise it answers [`ALREADY_AT_TOP`] and the
///   screen keeps its rows.
/// - PF8 with the flag `Y` reads forward from the last key with a skip read; with any other flag
///   it answers [`ALREADY_AT_BOTTOM`], the state and the rows kept.
///
/// `page_size` is clamped into `1..=MAX_PAGE_SIZE`.
///
/// # Errors
///
/// The input failure of [`edit_start_tran_id`] for an ENTER whose search value is neither blank
/// nor 16 digits (COTRN00C-013). [`lookup_failure`] for PF8 with the flag `Y` and a blank last
/// key: `STARTBR` at high-values finds no record and the skip `READNEXT` has no browse to read,
/// the halted path of COTRN00C-020.
pub fn plan(
    key: ListKey,
    start_tran_id: Option<&str>,
    state: TranListState,
    page_size: usize,
) -> Result<Plan, TranListFailure> {
    let page_size = page_size.clamp(1, MAX_PAGE_SIZE);
    match key {
        ListKey::Enter => {
            let search = start_tran_id.map_or(Ok(None), edit_start_tran_id)?;
            Ok(Plan::Read(first_page(search, Skip::Keep, state, page_size)))
        }
        ListKey::Entry(EntryKey::Enter | EntryKey::Pf3) => {
            Ok(Plan::Read(first_page(None, Skip::Keep, state, page_size)))
        }
        ListKey::Entry(EntryKey::Pf5) => Ok(Plan::Read(first_page(
            None,
            Skip::DropFirst,
            state,
            page_size,
        ))),
        ListKey::Pf7 => Ok(previous_page(state, page_size)),
        ListKey::Pf8 => following_page(state, page_size),
    }
}

/// The forward page of ENTER and of a first entry: `PROCESS-ENTER-KEY` moves 0 to the page number
/// before `PROCESS-PAGE-FORWARD` (line 224; COTRN00C-013, COTRN00C-014).
fn first_page(
    search: Option<PicX<TRAN_ID_WIDTH>>,
    skip: Skip,
    state: TranListState,
    page_size: usize,
) -> Browse {
    Browse {
        direction: Direction::Forward,
        start: search.map_or(BrowseKey::Lowest, BrowseKey::From),
        skip,
        page_size,
        state: TranListState {
            page_no: PicU::ZERO,
            ..state
        },
    }
}

/// `PROCESS-PF7-KEY`: the flag is set before the page test (X-COTRN00C-3), so even the answer at
/// the top leaves it `Y` (COTRN00C-022, COTRN00C-023).
fn previous_page(state: TranListState, page_size: usize) -> Plan {
    let state = TranListState {
        next_page: NextPage::Yes,
        ..state
    };
    if state.page_no.get() > 1 {
        Plan::Read(Browse {
            direction: Direction::Backward,
            start: state
                .first_key
                .clone()
                .map_or(BrowseKey::Lowest, BrowseKey::From),
            skip: Skip::DropFirst,
            page_size,
            state,
        })
    } else {
        Plan::Answer(TranPage::kept(state, ALREADY_AT_TOP))
    }
}

/// `PROCESS-PF8-KEY` (COTRN00C-015, COTRN00C-020, COTRN00C-021).
fn following_page(state: TranListState, page_size: usize) -> Result<Plan, TranListFailure> {
    match (state.next_page, state.last_key.clone()) {
        (NextPage::No, _) => Ok(Plan::Answer(TranPage::kept(state, ALREADY_AT_BOTTOM))),
        // HALTED(X-COTRN00C-H1): the blank last key browses from high-values, which no record
        // reaches; the program's last message is the failed skip read's, before the ENDBR abend.
        (NextPage::Yes, None) => Err(TranListFailure::lookup()),
        (NextPage::Yes, Some(last)) => Ok(Plan::Read(Browse {
            direction: Direction::Forward,
            start: BrowseKey::From(last),
            skip: Skip::DropFirst,
            page_size,
            state,
        })),
    }
}

/// The page `browse` reads, from the records the page query returned, each mapped by
/// [`row_from`], in query order: ascending from the key for [`Direction::Forward`], descending to
/// it for [`Direction::Backward`]. At most [`Browse::limit`] of them are consumed.
///
/// Forward (COTRN00C-014 to COTRN00C-019): after the skip read, if any, the records fill rows 1
/// up. A full page adds 1 to the page number and sets the first key from row 1 and the last key
/// from the last row; a read-ahead record then sets the next-page flag `Y` with no message, and
/// its absence sets `N` with [`REACHED_BOTTOM`]. A short page sets `N` with [`REACHED_BOTTOM`],
/// sets the first key and adds 1 to the page number when it holds a row, and keeps the last key
/// (X-COTRN00C-4). With no record at all the browse is the halted path of COTRN00C-020: without a
/// skip read the answer is [`AT_TOP`] with the flag `N` and the rows kept.
///
/// Backward (COTRN00C-023 to COTRN00C-026): the skip read drops the first key's record and the
/// records fill the rows from the last up, so the page shows them in ascending order; the last
/// row sets the last key and, on a full page, row 1 sets the first key. A full page with an
/// earlier record takes 1 from the page number with no message; without one it sets page 1 with
/// [`REACHED_TOP`]. A short page answers [`REACHED_TOP`] with the page number and first key kept.
///
/// # Errors
///
/// [`lookup_failure`] when a forward browse with a skip read finds no record at all: `STARTBR`
/// returns `NOTFND` and the skip `READNEXT` has no browse to read, the halted path of
/// COTRN00C-020.
pub fn page(browse: Browse, rows: Vec<TranListRow>) -> Result<TranPage, TranListFailure> {
    match browse.direction {
        Direction::Forward => forward(browse, rows),
        Direction::Backward => Ok(backward(browse, rows)),
    }
}

/// `PROCESS-PAGE-FORWARD` after a `STARTBR` (COTRN00C-014 to COTRN00C-020).
fn forward(browse: Browse, rows: Vec<TranListRow>) -> Result<TranPage, TranListFailure> {
    let Browse {
        skip,
        page_size,
        mut state,
        ..
    } = browse;
    if rows.is_empty() {
        // HALTED(X-COTRN00C-H1): STARTBR returns NOTFND and the task abends at ENDBR. The answer
        // is the last message the program sends and the flag it had set, no parity result.
        return match skip {
            Skip::Keep => {
                state.next_page = NextPage::No;
                Ok(TranPage::kept(state, AT_TOP))
            }
            Skip::DropFirst => Err(TranListFailure::lookup()),
        };
    }
    let mut records = rows.into_iter();
    if skip == Skip::DropFirst {
        records.next();
    }
    let mut slots: Vec<Option<TranListRow>> = vec![None; page_size];
    for (slot, record) in slots.iter_mut().zip(&mut records) {
        *slot = Some(record);
    }
    let filled = slots.iter().flatten().count();
    if let Some(Some(first)) = slots.first() {
        state.first_key = non_blank(Some(first.tran_id.clone()));
    }
    let message = if filled == page_size {
        if let Some(Some(last)) = slots.last() {
            state.last_key = non_blank(Some(last.tran_id.clone()));
        }
        state.page_no = plus_one(state.page_no);
        if records.next().is_some() {
            state.next_page = NextPage::Yes;
            None
        } else {
            state.next_page = NextPage::No;
            Some(REACHED_BOTTOM)
        }
    } else {
        state.next_page = NextPage::No;
        if filled > 0 {
            state.page_no = plus_one(state.page_no);
        }
        Some(REACHED_BOTTOM)
    };
    Ok(TranPage {
        rows: PageRows::Read(slots),
        state,
        message,
    })
}

/// `PROCESS-PAGE-BACKWARD` after a `STARTBR` (COTRN00C-023 to COTRN00C-026). Only PF7 reads
/// backward, so the skip `READPREV` always runs and the next-page flag is already `Y`.
fn backward(browse: Browse, rows: Vec<TranListRow>) -> TranPage {
    let Browse {
        page_size,
        mut state,
        ..
    } = browse;
    let mut records = rows.into_iter();
    if records.next().is_none() {
        // No audited path gets here: the first key of a page above 1 is a stored ID, which the
        // inclusive query returns first. The skip READPREV meeting the start of the file sets
        // its message and clears no row, so the screen keeps its page.
        return TranPage::kept(state, REACHED_TOP);
    }
    let mut slots: Vec<Option<TranListRow>> = vec![None; page_size];
    for (slot, record) in slots.iter_mut().rev().zip(&mut records) {
        *slot = Some(record);
    }
    let filled = slots.iter().flatten().count();
    if let Some(Some(last)) = slots.last() {
        state.last_key = non_blank(Some(last.tran_id.clone()));
    }
    let message = if filled == page_size {
        if let Some(Some(first)) = slots.first() {
            state.first_key = non_blank(Some(first.tran_id.clone()));
        }
        let earlier = records.next().is_some();
        state.page_no = if earlier && state.page_no.get() > 1 {
            minus_one(state.page_no)
        } else {
            PAGE_ONE
        };
        if earlier { None } else { Some(REACHED_TOP) }
    } else {
        Some(REACHED_TOP)
    };
    TranPage {
        rows: PageRows::Read(slots),
        state,
        message,
    }
}

/// `COMPUTE CDEMO-CT00-PAGE-NUM = CDEMO-CT00-PAGE-NUM + 1` (COTRN00C-016 to COTRN00C-018).
fn plus_one(page: PicU<8>) -> PicU<8> {
    // TRUNCATION: the COMPUTE has no ON SIZE ERROR, so 99,999,999 + 1 keeps the eight low-order
    // digits of 100,000,000 in the PIC 9(08) receiver, page 0 (AAP 0.3.3.2).
    page.succ().unwrap_or(PicU::ZERO)
}

/// `SUBTRACT 1 FROM CDEMO-CT00-PAGE-NUM`, which the program runs only above page 1
/// (COTRN00C-024), so the result is never below 1; the fallback is never taken.
fn minus_one(page: PicU<8>) -> PicU<8> {
    PicU::new(page.get().saturating_sub(1)).unwrap_or(PAGE_ONE)
}

/// The rows a response shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRows {
    /// A page was read: exactly `page_size` rows, row 1 first, each `None` where the row is blank,
    /// spaces in every field (COTRN00C-014, COTRN00C-018, COTRN00C-019, COTRN00C-026).
    Read(Vec<Option<TranListRow>>),
    /// Nothing was read: the screen keeps the rows, page number, search value and selectors it
    /// showed (COTRN00C-021, COTRN00C-022).
    Kept,
}

/// The response to a request that did not fail: the rows, the state for the next request and the
/// message the program moved last, if any (COTRN00C-035). Every one is status 200.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranPage {
    rows: PageRows,
    state: TranListState,
    message: Option<&'static str>,
}

impl TranPage {
    /// An answer that reads nothing: the screen keeps its rows and shows `message`.
    fn kept(state: TranListState, message: &'static str) -> Self {
        Self {
            rows: PageRows::Kept,
            state,
            message: Some(message),
        }
    }

    /// The rows to show.
    #[must_use]
    pub const fn rows(&self) -> &PageRows {
        &self.rows
    }

    /// The state the screen echoes with its next request.
    #[must_use]
    pub const fn state(&self) -> &TranListState {
        &self.state
    }

    /// The message, `None` when the program moved none (COTRN00C-016, COTRN00C-024).
    #[must_use]
    pub const fn message(&self) -> Option<&'static str> {
        self.message
    }

    /// The rows, the state and the message, by value.
    #[must_use]
    pub fn into_parts(self) -> (PageRows, TranListState, Option<&'static str>) {
        (self.rows, self.state, self.message)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveDateTime};
    use copybook::{CobolTimestamp, PicTimestamp, PicU, PicX, TsForm};

    use super::{
        Browse, BrowseKey, Direction, EntryKey, FailureClass, ListKey, MAX_PAGE_SIZE, NextPage,
        PageRows, Plan, TranListFailure, TranListField, TranListRow, TranListState, TranPage,
        edit_start_tran_id, lookup_failure, page, plan, row_from,
    };
    use crate::money::{Decimal, Money};
    use crate::records::transaction::TransactionRecord;
    use crate::sensitive::CardNumber;

    type TestResult = Result<(), String>;

    // The audited literals, written out apart from the module's own constants.
    const NOT_NUMERIC: &str = "Tran ID must be Numeric ...";
    const LOOKUP: &str = "Unable to lookup transaction...";
    const ALREADY_TOP: &str = "You are already at the top of the page...";
    const ALREADY_BOTTOM: &str = "You are already at the bottom of the page...";
    const REACHED_TOP: &str = "You have reached the top of the page...";
    const REACHED_BOTTOM: &str = "You have reached the bottom of the page...";
    const AT_TOP: &str = "You are at the top of the page...";

    /// What a page sets in the state, with its message.
    #[derive(Debug, PartialEq, Eq)]
    struct Seen {
        first: Option<u64>,
        last: Option<u64>,
        page_no: u64,
        next: NextPage,
        message: Option<&'static str>,
    }

    fn shown<E: core::fmt::Debug>(error: E) -> String {
        format!("{error:?}")
    }

    /// The transaction ID `n`, sixteen digits with leading zeros.
    fn id(n: u64) -> Result<PicX<16>, String> {
        PicX::from_text(&format!("{n:016}")).map_err(shown)
    }

    fn number(key: &PicX<16>) -> Result<u64, String> {
        key.to_text().parse().map_err(shown)
    }

    fn timestamp(text: &[u8]) -> Result<PicTimestamp, String> {
        PicTimestamp::decode(text).map_err(shown)
    }

    /// The instant of `date` at `time` and `micros` microseconds.
    fn at(
        (year, month, day): (i32, u32, u32),
        (hour, minute, second): (u32, u32, u32),
        micros: u32,
    ) -> Result<NaiveDateTime, String> {
        NaiveDate::from_ymd_opt(year, month, day)
            .and_then(|date| date.and_hms_micro_opt(hour, minute, second, micros))
            .ok_or_else(|| "no such instant".to_owned())
    }

    /// A transaction `n` of `cents` hundredths, its description `desc` and its origin `orig_ts`.
    fn record(
        n: u64,
        cents: i64,
        desc: &str,
        orig_ts: PicTimestamp,
    ) -> Result<TransactionRecord, String> {
        Ok(TransactionRecord {
            id: id(n)?,
            type_cd: PicX::from_text("01").map_err(shown)?,
            cat_cd: PicU::new(1).map_err(shown)?,
            source: PicX::from_text("POS TERM").map_err(shown)?,
            desc: PicX::from_text(desc).map_err(shown)?,
            amt: Money::new(Decimal::new(cents, 2)).map_err(shown)?,
            merchant_id: PicU::ZERO,
            merchant_name: PicX::spaces(),
            merchant_city: PicX::spaces(),
            merchant_zip: PicX::spaces(),
            card_num: CardNumber::new(PicX::spaces()),
            orig_ts,
            proc_ts: PicTimestamp::BLANK,
            filler: PicX::spaces(),
        })
    }

    /// The rows of the transactions `ids`, in the order given.
    fn rows(ids: impl IntoIterator<Item = u64>) -> Result<Vec<TranListRow>, String> {
        ids.into_iter()
            .map(|n| Ok(row_from(&record(n, 100, "Purchase", PicTimestamp::BLANK)?)))
            .collect()
    }

    fn state(
        first: Option<u64>,
        last: Option<u64>,
        page_no: u64,
        next: NextPage,
    ) -> Result<TranListState, String> {
        Ok(TranListState::new(
            first.map(id).transpose()?,
            last.map(id).transpose()?,
            PicU::new(page_no).map_err(shown)?,
            next,
        ))
    }

    fn read(planned: Result<Plan, TranListFailure>) -> Result<Browse, String> {
        match planned {
            Ok(Plan::Read(browse)) => Ok(browse),
            other => Err(format!("expected a read, got {other:?}")),
        }
    }

    fn answered(planned: Result<Plan, TranListFailure>) -> Result<TranPage, String> {
        match planned {
            Ok(Plan::Answer(answer)) => Ok(answer),
            other => Err(format!("expected an answer, got {other:?}")),
        }
    }

    fn seen(answer: &TranPage) -> Result<Seen, String> {
        let state = answer.state();
        Ok(Seen {
            first: state.first_key().map(number).transpose()?,
            last: state.last_key().map(number).transpose()?,
            page_no: state.page_no().get(),
            next: state.next_page(),
            message: answer.message(),
        })
    }

    /// The IDs on the page's rows, row 1 first, `None` for a blank row.
    fn ids_on(answer: &TranPage) -> Result<Vec<Option<u64>>, String> {
        match answer.rows() {
            PageRows::Read(slots) => slots
                .iter()
                .map(|slot| slot.as_ref().map(|row| number(row.tran_id())).transpose())
                .collect(),
            PageRows::Kept => Err("the rows were kept".to_owned()),
        }
    }

    /// `n` blank rows after the IDs `ids`.
    fn then_blank(ids: impl IntoIterator<Item = u64>, n: usize) -> Vec<Option<u64>> {
        ids.into_iter()
            .map(Some)
            .chain(std::iter::repeat_n(None, n))
            .collect()
    }

    /// The page an ENTER with a blank search reads over `records`.
    fn enter_page(records: Vec<TranListRow>, before: TranListState) -> Result<TranPage, String> {
        let browse = read(plan(ListKey::Enter, None, before, 10))?;
        page(browse, records).map_err(shown)
    }

    // COTRN00C-011 and COTRN00C-003: a blank search, and a first entry by ENTER or PF3, start at
    // the lowest record with no skip read and page number 0.
    #[test]
    fn a_blank_search_and_a_first_entry_start_at_the_lowest_record() -> TestResult {
        for raw in [
            "",
            " ",
            "                ",
            "\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0",
        ] {
            assert_eq!(edit_start_tran_id(raw), Ok(None), "{raw:?}");
        }
        let before = state(Some(7), Some(9), 4, NextPage::Yes)?;
        for (key, search) in [
            (ListKey::Enter, None),
            (ListKey::Enter, Some("   ")),
            (ListKey::Entry(EntryKey::Enter), Some("ABC")),
            (ListKey::Entry(EntryKey::Pf3), Some("0000000000000005")),
        ] {
            let browse = read(plan(key, search, before.clone(), 10))?;
            assert_eq!(browse.direction(), Direction::Forward, "{key:?}");
            assert_eq!(browse.start(), &BrowseKey::Lowest, "{key:?}");
            assert_eq!(browse.start().query_key(), PicX::spaces(), "{key:?}");
            assert_eq!(browse.limit(), 11, "{key:?}: no skip read");
            assert_eq!(browse.page_size(), 10, "{key:?}");
            assert_eq!(
                browse.state(),
                &before.after_failure(ListKey::Enter),
                "{key:?}"
            );
            assert_eq!(browse.state().page_no(), PicU::ZERO, "{key:?}");
        }
        Ok(())
    }

    // COTRN00C-012: sixteen ASCII digits start the page at or above them; bytes past the
    // sixteen of the map field are dropped.
    #[test]
    fn sixteen_digits_start_the_page_at_or_above_them() -> TestResult {
        assert_eq!(edit_start_tran_id("0000000000000123"), Ok(Some(id(123)?)));
        assert_eq!(edit_start_tran_id("00000000000001239"), Ok(Some(id(123)?)));
        let browse = read(plan(
            ListKey::Enter,
            Some("0000000000000042"),
            TranListState::default(),
            10,
        ))?;
        assert_eq!(browse.start(), &BrowseKey::From(id(42)?));
        assert_eq!(browse.start().query_key(), id(42)?);
        Ok(())
    }

    // COTRN00C-013 (and the replacement's 400 of COTRN00C-040): any other search value is the
    // input error on the search field, and the state keeps its keys with page number 0.
    #[test]
    fn a_search_that_is_not_sixteen_digits_is_rejected() -> TestResult {
        for raw in [
            "123",
            "ABC",
            "000000000000012A",
            "00000000 0000123",
            "\0",
            "-000000000000001",
            "\u{ff11}\u{ff12}",
        ] {
            let failure = edit_start_tran_id(raw)
                .err()
                .ok_or(format!("{raw:?} passed"))?;
            assert_eq!(failure.message(), NOT_NUMERIC, "{raw:?}");
            assert_eq!(failure.to_string(), NOT_NUMERIC, "{raw:?}");
            assert_eq!(failure.field(), Some(TranListField::StartTranId), "{raw:?}");
            assert_eq!(failure.class(), FailureClass::Input, "{raw:?}");
            assert_eq!(failure.class().http_status(), 400, "{raw:?}");
        }
        let before = state(Some(11), Some(20), 2, NextPage::Yes)?;
        let failure = plan(ListKey::Enter, Some("12"), before.clone(), 10)
            .err()
            .ok_or("a short search passed")?;
        assert_eq!(failure.message(), NOT_NUMERIC);
        assert_eq!(
            before.after_failure(ListKey::Enter),
            state(Some(11), Some(20), 0, NextPage::Yes)?
        );
        Ok(())
    }

    // COTRN00C-014, COTRN00C-016: a full page with a read-ahead record.
    #[test]
    fn a_full_forward_page_with_a_record_after_it() -> TestResult {
        let shown_page = enter_page(rows(1..=11)?, state(None, Some(99), 0, NextPage::No)?)?;
        assert_eq!(ids_on(&shown_page)?, then_blank(1..=10, 0));
        let expected = Seen {
            first: Some(1),
            last: Some(10),
            page_no: 1,
            next: NextPage::Yes,
            message: None,
        };
        assert_eq!(seen(&shown_page)?, expected);
        Ok(())
    }

    // COTRN00C-017: a full page whose read-ahead meets the end of the file.
    #[test]
    fn a_full_forward_page_at_the_end_of_the_file() -> TestResult {
        let shown_page = enter_page(rows(1..=10)?, TranListState::default())?;
        assert_eq!(ids_on(&shown_page)?, then_blank(1..=10, 0));
        let expected = Seen {
            first: Some(1),
            last: Some(10),
            page_no: 1,
            next: NextPage::No,
            message: Some(REACHED_BOTTOM),
        };
        assert_eq!(seen(&shown_page)?, expected);
        Ok(())
    }

    // COTRN00C-018: a short forward page keeps the last key of the state (X-COTRN00C-4).
    #[test]
    fn a_short_forward_page_keeps_the_last_key() -> TestResult {
        let shown_page = enter_page(rows(1..=3)?, state(Some(50), Some(99), 6, NextPage::Yes)?)?;
        assert_eq!(ids_on(&shown_page)?, then_blank(1..=3, 7));
        let expected = Seen {
            first: Some(1),
            last: Some(99),
            page_no: 1,
            next: NextPage::No,
            message: Some(REACHED_BOTTOM),
        };
        assert_eq!(seen(&shown_page)?, expected);
        Ok(())
    }

    // COTRN00C-015: PF8 browses from the last key, drops it, and continues the page number.
    #[test]
    fn pf8_reads_the_page_after_the_last_key() -> TestResult {
        let before = state(Some(1), Some(10), 1, NextPage::Yes)?;
        let browse = read(plan(ListKey::Pf8, None, before.clone(), 10))?;
        assert_eq!(browse.direction(), Direction::Forward);
        assert_eq!(browse.start(), &BrowseKey::From(id(10)?));
        assert_eq!(browse.limit(), 12, "the skip read adds one record");
        assert_eq!(browse.state(), &before);
        let shown_page = page(browse, rows(10..=21)?).map_err(shown)?;
        assert_eq!(ids_on(&shown_page)?, then_blank(11..=20, 0));
        let expected = Seen {
            first: Some(11),
            last: Some(20),
            page_no: 2,
            next: NextPage::Yes,
            message: None,
        };
        assert_eq!(seen(&shown_page)?, expected);
        Ok(())
    }

    // COTRN00C-019: PF8 with no record after the last key shows ten blank rows.
    #[test]
    fn pf8_after_the_last_record_shows_blank_rows() -> TestResult {
        let before = state(Some(1), Some(10), 3, NextPage::Yes)?;
        let browse = read(plan(ListKey::Pf8, None, before, 10))?;
        let shown_page = page(browse, rows([10])?).map_err(shown)?;
        assert_eq!(ids_on(&shown_page)?, then_blank([], 10));
        let expected = Seen {
            first: Some(1),
            last: Some(10),
            page_no: 3,
            next: NextPage::No,
            message: Some(REACHED_BOTTOM),
        };
        assert_eq!(seen(&shown_page)?, expected);
        Ok(())
    }

    // COTRN00C-021: PF8 with a flag other than `Y` reads nothing and keeps the state.
    #[test]
    fn pf8_without_a_next_page_answers_at_the_bottom() -> TestResult {
        let before = state(Some(1), Some(7), 1, NextPage::No)?;
        let answer = answered(plan(ListKey::Pf8, None, before.clone(), 10))?;
        assert_eq!(answer.rows(), &PageRows::Kept);
        assert_eq!(answer.message(), Some(ALREADY_BOTTOM));
        assert_eq!(answer.state(), &before);
        Ok(())
    }

    // COTRN00C-022: PF7 at page 1 or 0 reads nothing and still sets the flag `Y`
    // (X-COTRN00C-3).
    #[test]
    fn pf7_at_the_top_answers_and_sets_the_flag() -> TestResult {
        for page_no in [0, 1] {
            let before = state(Some(1), Some(7), page_no, NextPage::No)?;
            let answer = answered(plan(ListKey::Pf7, None, before, 10))?;
            assert_eq!(answer.rows(), &PageRows::Kept, "page {page_no}");
            let expected = Seen {
                first: Some(1),
                last: Some(7),
                page_no,
                next: NextPage::Yes,
                message: Some(ALREADY_TOP),
            };
            assert_eq!(seen(&answer)?, expected, "page {page_no}");
        }
        Ok(())
    }

    // COTRN00C-023, COTRN00C-024: PF7 above page 1 reads the ten records before the first key,
    // shown in ascending order, and takes 1 from the page number.
    #[test]
    fn pf7_reads_the_page_before_the_first_key() -> TestResult {
        let before = state(Some(21), Some(30), 3, NextPage::No)?;
        let browse = read(plan(ListKey::Pf7, None, before, 10))?;
        assert_eq!(browse.direction(), Direction::Backward);
        assert_eq!(browse.start(), &BrowseKey::From(id(21)?));
        assert_eq!(browse.limit(), 12);
        assert_eq!(browse.state().next_page(), NextPage::Yes);
        let shown_page = page(browse, rows((10..=21).rev())?).map_err(shown)?;
        assert_eq!(ids_on(&shown_page)?, then_blank(11..=20, 0));
        let expected = Seen {
            first: Some(11),
            last: Some(20),
            page_no: 2,
            next: NextPage::Yes,
            message: None,
        };
        assert_eq!(seen(&shown_page)?, expected);
        Ok(())
    }

    // COTRN00C-025: a full backward page with no earlier record sets page 1.
    #[test]
    fn a_full_backward_page_at_the_start_of_the_file() -> TestResult {
        for page_no in [2, 5] {
            let before = state(Some(11), Some(20), page_no, NextPage::Yes)?;
            let browse = read(plan(ListKey::Pf7, None, before, 10))?;
            let shown_page = page(browse, rows((1..=11).rev())?).map_err(shown)?;
            assert_eq!(
                ids_on(&shown_page)?,
                then_blank(1..=10, 0),
                "page {page_no}"
            );
            let expected = Seen {
                first: Some(1),
                last: Some(10),
                page_no: 1,
                next: NextPage::Yes,
                message: Some(REACHED_TOP),
            };
            assert_eq!(seen(&shown_page)?, expected, "page {page_no}");
        }
        Ok(())
    }

    // COTRN00C-026: a short backward page fills the last rows, sets the last key and keeps the
    // first key and the page number; with no record before the first key every row is blank.
    #[test]
    fn a_short_backward_page_fills_the_last_rows() -> TestResult {
        let before = state(Some(4), Some(13), 3, NextPage::Yes)?;
        let browse = read(plan(ListKey::Pf7, None, before.clone(), 10))?;
        let shown_page = page(browse.clone(), rows((1..=4).rev())?).map_err(shown)?;
        let expected_rows: Vec<Option<u64>> = std::iter::repeat_n(None, 7)
            .chain((1..=3).map(Some))
            .collect();
        assert_eq!(ids_on(&shown_page)?, expected_rows);
        let expected = Seen {
            first: Some(4),
            last: Some(3),
            page_no: 3,
            next: NextPage::Yes,
            message: Some(REACHED_TOP),
        };
        assert_eq!(seen(&shown_page)?, expected);

        let none_before = page(browse, rows([4])?).map_err(shown)?;
        assert_eq!(ids_on(&none_before)?, then_blank([], 10));
        let expected = Seen {
            first: Some(4),
            last: Some(13),
            page_no: 3,
            next: NextPage::Yes,
            message: Some(REACHED_TOP),
        };
        assert_eq!(seen(&none_before)?, expected);
        Ok(())
    }

    // A backward read that returns nothing, which no audited path reaches (a blank first key
    // above page 1): the screen keeps its page with the start-of-file message.
    #[test]
    fn a_backward_read_of_no_record_keeps_the_screen() -> TestResult {
        let before = state(None, Some(20), 2, NextPage::No)?;
        let browse = read(plan(ListKey::Pf7, None, before, 10))?;
        assert_eq!(browse.start(), &BrowseKey::Lowest);
        let answer = page(browse, Vec::new()).map_err(shown)?;
        assert_eq!(answer.rows(), &PageRows::Kept);
        assert_eq!(answer.message(), Some(REACHED_TOP));
        assert_eq!(answer.state(), &state(None, Some(20), 2, NextPage::Yes)?);
        Ok(())
    }

    // COTRN00C-004: a first entry by COTRN01C's F5 skips the lowest record; with one
    // transaction every row is blank and the page number stays 0.
    #[test]
    fn a_first_entry_by_pf5_skips_the_lowest_record() -> TestResult {
        let before = state(Some(31), Some(40), 4, NextPage::Yes)?;
        let browse = read(plan(
            ListKey::Entry(EntryKey::Pf5),
            None,
            before.clone(),
            10,
        ))?;
        assert_eq!(browse.start(), &BrowseKey::Lowest);
        assert_eq!(browse.limit(), 12);
        let skipped = page(browse.clone(), rows(1..=12)?).map_err(shown)?;
        assert_eq!(ids_on(&skipped)?, then_blank(2..=11, 0));
        assert_eq!(seen(&skipped)?.page_no, 1);

        let alone = page(browse, rows([1])?).map_err(shown)?;
        assert_eq!(ids_on(&alone)?, then_blank([], 10));
        let expected = Seen {
            first: Some(31),
            last: Some(40),
            page_no: 0,
            next: NextPage::No,
            message: Some(REACHED_BOTTOM),
        };
        assert_eq!(seen(&alone)?, expected);
        Ok(())
    }

    // COTRN00C-020, HALTED(X-COTRN00C-H1): no parity value is asserted. These check the
    // replacement's documented answers only: the last message the program sends.
    #[test]
    fn a_forward_browse_that_finds_no_record_takes_the_replacements_answer() -> TestResult {
        let before = state(Some(1), Some(10), 3, NextPage::Yes)?;
        let answer = enter_page(Vec::new(), before.clone())?;
        assert_eq!(answer.rows(), &PageRows::Kept);
        assert_eq!(answer.message(), Some(AT_TOP));
        assert_eq!(answer.state(), &state(Some(1), Some(10), 0, NextPage::No)?);

        let entry = read(plan(
            ListKey::Entry(EntryKey::Pf5),
            None,
            before.clone(),
            10,
        ))?;
        assert_eq!(page(entry, Vec::new()), Err(lookup_failure()));

        let blank_last = state(Some(1), None, 3, NextPage::Yes)?;
        assert_eq!(
            plan(ListKey::Pf8, None, blank_last, 10).err(),
            Some(lookup_failure())
        );
        Ok(())
    }

    // COTRN00C-027: a failed read is the 500 lookup failure with no field; the state the
    // screen keeps resets the page number after ENTER and sets the flag after PF7.
    #[test]
    fn a_failed_read_is_the_lookup_failure() -> TestResult {
        let failure = lookup_failure();
        assert_eq!(failure.message(), LOOKUP);
        assert_eq!(failure.to_string(), LOOKUP);
        assert_eq!(failure.field(), None);
        assert_eq!(failure.class(), FailureClass::Lookup);
        assert_eq!(failure.class().http_status(), 500);
        let before = state(Some(11), Some(20), 2, NextPage::No)?;
        let reset = state(Some(11), Some(20), 0, NextPage::No)?;
        assert_eq!(before.after_failure(ListKey::Enter), reset);
        assert_eq!(before.after_failure(ListKey::Entry(EntryKey::Pf5)), reset);
        assert_eq!(
            before.after_failure(ListKey::Pf7),
            state(Some(11), Some(20), 2, NextPage::Yes)?
        );
        assert_eq!(before.after_failure(ListKey::Pf8), before);
        Ok(())
    }

    // COTRN00C-028, COTRN00C-031, COTRN00C-032, COTRN00C-034: the ID as stored, 26 bytes of
    // the description and the amount through `+99999999.99` (X-COTRN00C-6 drops the ninth
    // integer digit); no field holds the card number.
    #[test]
    fn a_row_holds_the_id_the_screen_description_and_the_edited_amount() -> TestResult {
        let desc = "Purchase at Abshire-Lowe and a description longer than the screen";
        for (cents, amount) in [
            (5_047, "+00000050.47"),
            (-1_234, "-00000012.34"),
            (0, "+00000000.00"),
            (12_345_678_901, "+23456789.01"),
            (-99_999_999_999, "-99999999.99"),
        ] {
            let row = row_from(&record(683_580, cents, desc, PicTimestamp::BLANK)?);
            assert_eq!(row.tran_id(), &id(683_580)?);
            assert_eq!(row.description().as_bytes(), b"Purchase at Abshire-Lowe a");
            assert_eq!(row.amount(), amount, "{cents}");
            assert_eq!(row.amount().len(), 12, "{cents}");
        }
        let short = row_from(&record(1, 1, "Fee", PicTimestamp::BLANK)?);
        assert_eq!(
            short.description().as_bytes(),
            b"Fee                       "
        );
        Ok(())
    }

    // COTRN00C-029: the typed origin timestamp, its date and the `MM/DD/YY` display date, for
    // each layout an in-scope writer produces (PIC-018 to PIC-020); COTRN00C-030,
    // HALTED(X-COTRN00C-H3): a blank timestamp gives no date, which asserts no parity value.
    #[test]
    fn a_row_carries_the_typed_origin_and_its_display_date() -> TestResult {
        let cases = [
            (
                b"2022-06-10 19:27:53.000000",
                at((2022, 6, 10), (19, 27, 53), 0)?,
                TsForm::Iso,
                b"06/10/22",
            ),
            (
                b"2005-07-18-10.15.42.370000",
                at((2005, 7, 18), (10, 15, 42), 370_000)?,
                TsForm::Db2,
                b"07/18/05",
            ),
            (
                b"1999-12-31                ",
                at((1999, 12, 31), (0, 0, 0), 0)?,
                TsForm::ScreenDate,
                b"12/31/99",
            ),
        ];
        for (text, expected, form, display) in cases {
            let orig_ts = timestamp(text)?;
            let typed = CobolTimestamp::new(expected, form).map_err(shown)?;
            assert_eq!(orig_ts.value(), Some(typed));
            let row = row_from(&record(1, 1, "Fee", orig_ts)?);
            assert_eq!(row.orig_ts(), Some(expected));
            assert_eq!(row.date(), Some(expected.date()));
            assert_eq!(row.display_date(), Some(&PicX::from_array(*display)));
        }
        let blank = row_from(&record(1, 1, "Fee", PicTimestamp::BLANK)?);
        assert_eq!(blank.orig_ts(), None);
        assert_eq!(blank.date(), None);
        assert_eq!(blank.display_date(), None);
        Ok(())
    }

    // Bounded Pagination: a page size replaces the ten rows everywhere the program counts them,
    // and any size is clamped into 1..=MAX_PAGE_SIZE.
    #[test]
    fn the_page_size_sets_the_rows_the_last_key_and_the_read_ahead() -> TestResult {
        let browse = read(plan(ListKey::Enter, None, TranListState::default(), 3))?;
        assert_eq!(browse.limit(), 4);
        let shown_page = page(browse, rows(1..=4)?).map_err(shown)?;
        assert_eq!(ids_on(&shown_page)?, then_blank(1..=3, 0));
        assert_eq!(seen(&shown_page)?.last, Some(3));
        assert_eq!(seen(&shown_page)?.next, NextPage::Yes);
        for (asked, size) in [
            (0, 1),
            (1, 1),
            (MAX_PAGE_SIZE, MAX_PAGE_SIZE),
            (1_000, MAX_PAGE_SIZE),
        ] {
            let browse = read(plan(
                ListKey::Pf8,
                None,
                state(None, Some(1), 1, NextPage::Yes)?,
                asked,
            ))?;
            assert_eq!(browse.page_size(), size, "{asked}");
            assert_eq!(browse.limit(), size + 2, "{asked}");
        }
        Ok(())
    }

    // Bounded Pagination: records past the limit are never looked at.
    #[test]
    fn records_past_the_limit_change_nothing() -> TestResult {
        let at_limit = enter_page(rows(1..=11)?, TranListState::default())?;
        let past_limit = enter_page(rows(1..=40)?, TranListState::default())?;
        assert_eq!(at_limit, past_limit);
        Ok(())
    }

    // COTRN00C-016: the page number is PIC 9(08), and the COMPUTE that passes 99,999,999 keeps
    // its eight low-order digits.
    #[test]
    fn the_page_number_wraps_past_eight_digits() -> TestResult {
        let before = state(Some(1), Some(10), 99_999_999, NextPage::Yes)?;
        let browse = read(plan(ListKey::Pf8, None, before, 10))?;
        let shown_page = page(browse, rows(10..=21)?).map_err(shown)?;
        assert_eq!(seen(&shown_page)?.page_no, 0);
        Ok(())
    }

    // COTRN00C-002, COTRN00C-021: the flag is `Y` or not, a blank key is `None`, and a menu
    // entry starts from blank keys, page 0 and a flag other than `Y`.
    #[test]
    fn the_state_reads_the_commarea_values() {
        for (code, flag) in [
            ('Y', NextPage::Yes),
            ('N', NextPage::No),
            (' ', NextPage::No),
            ('y', NextPage::No),
        ] {
            assert_eq!(NextPage::from_code(code), flag, "{code:?}");
        }
        assert_eq!(NextPage::Yes.code(), 'Y');
        assert_eq!(NextPage::No.code(), 'N');
        let blank = TranListState::new(
            Some(PicX::spaces()),
            Some(PicX::spaces()),
            PicU::ZERO,
            NextPage::No,
        );
        assert_eq!(blank, TranListState::default());
        assert_eq!(blank.first_key(), None);
        assert_eq!(blank.last_key(), None);
        assert_eq!(blank.page_no(), PicU::ZERO);
        assert_eq!(blank.next_page(), NextPage::No);
    }

    // COTRN00C-035: the response carries the one message the request ends with, by value.
    #[test]
    fn a_page_comes_apart_into_rows_state_and_message() -> TestResult {
        let shown_page = enter_page(rows(1..=2)?, TranListState::default())?;
        let expected_state = shown_page.state().clone();
        let (shown_rows, shown_state, message) = shown_page.into_parts();
        assert!(matches!(shown_rows, PageRows::Read(ref slots) if slots.len() == 10));
        assert_eq!(shown_state, expected_state);
        assert_eq!(message, Some(REACHED_BOTTOM));
        Ok(())
    }
}
