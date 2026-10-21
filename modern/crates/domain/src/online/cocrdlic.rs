//! COCRDLIC, the card list: the rules of endpoint 7, `GET /api/v1/cards` (transaction `CCLI`,
//! map `CCRDLIA`, route `/cards`; AAP 0.3.4.1).
//!
//! This module ports the `server` and `both` rows of `modern/audit/online/COCRDLIC.md`: the
//! filter edits (COCRDLIC-005 to COCRDLIC-010), the paging of `0000-MAIN`, `9000-READ-FORWARD`
//! and `9100-READ-BACKWARDS` (COCRDLIC-017 to COCRDLIC-030), the file-error text
//! (COCRDLIC-031, COCRDLIC-037), the listed fields (COCRDLIC-032) and the two message lines of
//! `1400-SETUP-MESSAGE` (COCRDLIC-033, COCRDLIC-034). Nothing here takes a user type: the
//! program never tests one, so every signed-on user lists every card (COCRDLIC-001). The
//! `client` rows belong to the screen, the selection edit of `2250-EDIT-ARRAY` among them
//! (COCRDLIC-012 to COCRDLIC-016): `S` and `U` only navigate and never reach the endpoint.
//!
//! # Flow
//!
//! `api::services::cocrdlic` calls [`edit_filters`] on the raw filter text, then [`plan`] with
//! the key and the paging state the screen echoes ([`plan_reread`] for the selection-error
//! re-read of COCRDLIC-014). The [`ListPlan`] names the one read to run, as [`Read`], and its row
//! limit; the service runs it against the index-served card page queries
//! (`queries/cards_page_forward.sql`, `queries/cards_page_backward.sql`) with the filters applied
//! in SQL, runs the unfiltered look-ahead that [`ListPlan::lookahead_after`] asks for, and hands
//! the rows to [`page`], which decides the page content, keys, page number and both messages.
//! No step reads more than `page_size + 1` rows (Bounded Pagination), and this module does no
//! I/O.
//!
//! # Paging state
//!
//! The COBOL keeps its paging state in the COMMAREA between interactions: the page number, the
//! first and last keys, the next-page indicator and the last-page flag (`WS-THIS-PROGCOMMAREA`,
//! `COCRDLIC.cbl:229-248`). Endpoint 7 is stateless, so [`CardPage::state`] returns that state
//! and the screen echoes it unchanged with its next request as a [`ListState`], with the key
//! pressed as the [`Direction`] (`None` for ENTER), as escalation candidate `X-COCRDLIC-2`
//! proposes. The echoed state carries no authority: the server re-reads the cards under the
//! filters sent. Keys are [`CardRef`] handles, never card numbers (Cardholder Data Redaction,
//! `E32`): no key, row text or message here holds a full card number (COCRDLIC-032).
//!
//! # Quirks kept
//!
//! The paging defects of `X-COCRDLIC-3` are the expected results of their rows and none is
//! corrected (Behaviour constraint): ENTER re-reads from the page's first key even after a
//! filter change (COCRDLIC-018); the look-ahead ignores the filters and becomes the last key
//! (COCRDLIC-019); `'NO MORE RECORDS TO SHOW'` appears on every page without a next page
//! (COCRDLIC-019, COCRDLIC-020); the one-digit page number wraps (COCRDLIC-030); F7 from page 2
//! shows `'NO PREVIOUS PAGES TO DISPLAY'` (COCRDLIC-028); a backward read that reaches the start
//! of the file reports the file-error text (COCRDLIC-029).
//!
//! # Halts
//!
//! - `HALTED(H4)`: the CICS `RESP` and `RESP2` fields of the file-error text have no
//!   counterpart; they are written as the spaces of their `VALUE SPACES`, which is no parity
//!   result and never carries a key value or database text (COCRDLIC-037, halt candidate
//!   `X-COCRDLIC-H1`, proposed to join H4).
//! - Halt candidate `X-COCRDLIC-H1`: a browse that finds no card at or after its key ends the
//!   COBOL task abnormally (COCRDLIC-038). The empty page that [`page`] gives for such a read,
//!   the empty page 1 of an empty card table included, is no parity result.
//! - Halt candidate `X-COCRDLIC-H2`: which filters that are neither blank nor all digits the
//!   COBOL rejects depends on compiler options outside the repository (COCRDLIC-006,
//!   COCRDLIC-008, COCRDLIC-009). [`edit_filters`] takes the program's evident path, the class
//!   test and its message, which is no parity result either.
//!
//! # Totality
//!
//! Every function takes any input, filter text of any length or encoding included, and returns
//! a value or a [`ListFailure`]; nothing indexes, slices, unwraps or panics (Total Parsers Over
//! Untrusted Input), and every page-number step is `u8` arithmetic on a digit below 10.

use core::fmt;

use copybook::{ErrorKind, PicU, PicX};

use crate::records::card::CardRecord;
use crate::sensitive::CardNumber;

/// The rows of one page when the request names no page size: `WS-MAX-SCREEN-LINES PIC S9(4)
/// COMP VALUE 7` (`COCRDLIC.cbl:177-178`), the seven list rows of `CCRDLIA` (COCRDLIC-017).
///
/// The screen never sends a page size, so every paging parity result is stated for this one;
/// `api` validates a requested size to 1–100 (Bounded Pagination).
pub const DEFAULT_PAGE_SIZE: usize = 7;

/// Width of `CC-ACCT-ID PIC X(11)` and `CC-ACCT-ID-N PIC 9(11)` (`CVCRD01Y`), the account filter.
const ACCT_FILTER_WIDTH: usize = 11;

/// Width of `CC-CARD-NUM PIC X(16)` and `CC-CARD-NUM-N PIC 9(16)` (`CVCRD01Y`), the card filter.
const CARD_FILTER_WIDTH: usize = 16;

/// The largest value of `WS-CA-SCREEN-NUM PIC 9(1)`.
const PAGE_NUMBER_MAX: u8 = 9;

/// The modulus a one-digit page number keeps: `ADD +1` drops the high-order digit.
const PAGE_NUMBER_MODULUS: u8 = 10;

/// `WS-INFORM-REC-ACTIONS`, the information line (`COCRDLIC.cbl:115-116`, COCRDLIC-033).
const INFO_RECORD_ACTIONS: &str = "TYPE S FOR DETAIL, U TO UPDATE ANY RECORD";

/// `WS-NO-RECORDS-FOUND` (`COCRDLIC.cbl:121-122`, COCRDLIC-021).
const NO_RECORDS_FOUND: &str = "NO RECORDS FOUND FOR THIS SEARCH CONDITION.";

/// The end-of-file message of both forward reads (`COCRDLIC.cbl:1219,1239`, COCRDLIC-019,
/// COCRDLIC-020).
const NO_MORE_RECORDS: &str = "NO MORE RECORDS TO SHOW";

/// F7 on page 1 (`COCRDLIC.cbl:903`, COCRDLIC-026, COCRDLIC-028).
const NO_PREVIOUS_PAGES: &str = "NO PREVIOUS PAGES TO DISPLAY";

/// F8 on the last page once it was shown (`COCRDLIC.cbl:908`, COCRDLIC-024).
const NO_MORE_PAGES: &str = "NO MORE PAGES TO DISPLAY";

/// `2210-EDIT-ACCOUNT` class-test failure (`COCRDLIC.cbl:1022`, COCRDLIC-006).
const ACCOUNT_FILTER_INVALID: &str = "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER";

/// `2220-EDIT-CARD` class-test failure (`COCRDLIC.cbl:1058`, COCRDLIC-008).
const CARD_FILTER_INVALID: &str = "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER";

/// `WS-FILE-ERROR-MESSAGE` (`COCRDLIC.cbl:153-171`) after a failed `READ` of `CARDDAT`, cut to
/// the 75 bytes of `WS-ERROR-MSG PIC X(75)` (COCRDLIC-031): its five trailing `FILLER` bytes are
/// dropped by the move.
///
/// `HALTED(H4)`: the `ERROR-RESP` and `ERROR-RESP2 PIC X(10)` fields, bytes 49-58 and 66-75,
/// would hold the CICS `RESP` and `RESP2` values, which have no counterpart; they keep the
/// spaces of their `VALUE SPACES` (COCRDLIC-037). The text is kept whole, trailing spaces
/// included, so that bytes 59-65 stay assertable.
const FILE_ERROR: &str = concat!(
    "File Error: ",    // FILLER PIC X(12) VALUE 'File Error:'
    "READ    ",        // ERROR-OPNAME PIC X(8) <- 'READ'
    " on ",            // FILLER PIC X(4)
    "CARDDAT  ",       // ERROR-FILE PIC X(9) <- LIT-CARD-FILE 'CARDDAT '
    " returned RESP ", // FILLER PIC X(15)
    "          ",      // ERROR-RESP PIC X(10): HALTED(H4)
    ",RESP2 ",         // FILLER PIC X(7)
    "          ",      // ERROR-RESP2 PIC X(10): HALTED(H4)
);

const _: () = assert!(FILE_ERROR.len() == 75);
const _: () = assert!(INFO_RECORD_ACTIONS.len() <= 45);
const _: () = assert!(NO_RECORDS_FOUND.len() <= 75);
const _: () = assert!(ACCOUNT_FILTER_INVALID.len() <= 75);
const _: () = assert!(CARD_FILTER_INVALID.len() <= 75);

/// The opaque handle of one card: the `cards.card_ref` identity column (CVACT02Y-008), which
/// card list rows and page keys carry in place of the card number (Cardholder Data Redaction,
/// `E32`; COCRDLIC-032).
///
/// It names a card and nothing else: it orders no browse (the COBOL browses by card number) and
/// has no `Serialize`, because each response DTO maps it to its own field. `cocrdslc` and
/// `cocrdupc` take it from a list row to look the card up again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CardRef(i64);

impl CardRef {
    /// The handle `value`, as the database or a request carries it.
    #[must_use]
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// The handle's value, for SQL binds and response fields.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// The page number, `WS-CA-SCREEN-NUM PIC 9(1)` (`COCRDLIC.cbl:237-238`), shown in `PAGENO`.
///
/// One unsigned digit, 0 to 9, with no size-error phrase: a page move from 9 stores 0 and one
/// from 0 stores 1 (COCRDLIC-030). Page 0 is a value the screen can show and echo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageNumber(u8);

impl PageNumber {
    /// Page 1, `CA-FIRST-PAGE`.
    pub const FIRST: Self = Self(1);

    /// Page 0, which only the wrap of COCRDLIC-030 produces.
    const ZERO: Self = Self(0);

    /// The page number `value`, or `None` above 9.
    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if value <= PAGE_NUMBER_MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The digit.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Whether this is page 1, `CA-FIRST-PAGE`.
    const fn is_first(self) -> bool {
        self.0 == Self::FIRST.0
    }

    /// `ADD +1 TO WS-CA-SCREEN-NUM` (`COCRDLIC.cbl:492`): 9 + 1 stores 0, the high-order digit
    /// dropped (COCRDLIC-030). The digit is at most 9, so the sum is at most 10.
    const fn plus_one(self) -> Self {
        Self((self.0 + 1) % PAGE_NUMBER_MODULUS)
    }

    /// `SUBTRACT 1 FROM WS-CA-SCREEN-NUM` (`COCRDLIC.cbl:508`): 0 − 1 stores 1, the unsigned
    /// receiver dropping the sign (COCRDLIC-030).
    const fn minus_one(self) -> Self {
        Self(self.0.abs_diff(1))
    }
}

impl Default for PageNumber {
    /// [`PageNumber::FIRST`], the page of a fresh start (COCRDLIC-002).
    fn default() -> Self {
        Self::FIRST
    }
}

impl TryFrom<i64> for PageNumber {
    type Error = ErrorKind;

    /// The page number a request echoes.
    ///
    /// # Errors
    ///
    /// [`ErrorKind::OutOfRange`] outside 0 to 9.
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u8::try_from(value)
            .ok()
            .and_then(Self::new)
            .ok_or(ErrorKind::OutOfRange)
    }
}

impl From<PageNumber> for i64 {
    fn from(value: PageNumber) -> Self {
        Self::from(value.0)
    }
}

/// `WS-CA-NEXT-PAGE-IND PIC X(1)` (`COCRDLIC.cbl:242-244`): whether F8 moves to a next page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum NextPage {
    /// `CA-NEXT-PAGE-EXISTS`, `'Y'`.
    Exists,
    /// `CA-NEXT-PAGE-NOT-EXISTS`, low-values; also the fresh start, whose spaces F8 reads the
    /// same way.
    #[default]
    NotExists,
}

/// `WS-CA-LAST-PAGE-DISPLAYED PIC 9(1)` (`COCRDLIC.cbl:239-241`): whether F8 has already shown
/// the last page (COCRDLIC-024, COCRDLIC-025).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LastPage {
    /// `CA-LAST-PAGE-SHOWN`, 0.
    Shown,
    /// `CA-LAST-PAGE-NOT-SHOWN`, 9.
    #[default]
    NotShown,
}

/// The paging state the COBOL keeps in `WS-THIS-PROGCOMMAREA` (`COCRDLIC.cbl:229-248`), which
/// endpoint 7 returns with every page and the screen echoes with its next request
/// (`X-COCRDLIC-2`). Any combination is a state the program accepts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ListState {
    /// `WS-CA-SCREEN-NUM`.
    pub page_no: PageNumber,
    /// `WS-CA-FIRST-CARD-NUM`, as its card's handle: where ENTER and F7 read from. `None` is the
    /// key of spaces a fresh start browses from, below every card number.
    pub first_key: Option<CardRef>,
    /// `WS-CA-LAST-CARD-NUM`, as its card's handle: where F8 reads from.
    pub last_key: Option<CardRef>,
    /// `WS-CA-NEXT-PAGE-IND`.
    pub next_page: NextPage,
    /// `WS-CA-LAST-PAGE-DISPLAYED`.
    pub last_page: LastPage,
}

impl ListState {
    /// The state of a fresh start, from the menu or from either card screen (COCRDLIC-002):
    /// page 1, no first or last key, no next page and the last page not shown. The same value
    /// as [`Default`].
    #[must_use]
    pub const fn fresh() -> Self {
        Self {
            page_no: PageNumber::FIRST,
            first_key: None,
            last_key: None,
            next_page: NextPage::NotExists,
            last_page: LastPage::NotShown,
        }
    }
}

/// A paging key: F8 is [`Direction::Forward`], F7 [`Direction::Backward`]. A request without one
/// is ENTER, or a key the program processes as ENTER (COCRDLIC-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// F8 (or F20), page down.
    Forward,
    /// F7 (or F19), page up.
    Backward,
}

/// The one browse a request runs, by card number, the key order of the `CARDDAT` KSDS.
///
/// The service resolves `start` to its card number (a `None` start browses from the lowest
/// card number) and reads with the filters of [`CardFilters`] applied in SQL, at most
/// [`ListPlan::limit`] rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Read {
    /// `9000-READ-FORWARD`: `STARTBR GTEQ` at `start`, then `READNEXT`. The rows are the
    /// matching cards from `start` inclusive, in ascending card-number order
    /// (`queries/cards_page_forward.sql`; COCRDLIC-017, COCRDLIC-018, COCRDLIC-023).
    Forward {
        /// The browse key.
        start: Option<CardRef>,
    },
    /// `9100-READ-BACKWARDS`: `STARTBR GTEQ` at `start`, then `READPREV`. The rows are the
    /// matching cards from `start` inclusive, in descending card-number order
    /// (`queries/cards_page_backward.sql`; COCRDLIC-027, COCRDLIC-029). [`page`] drops the
    /// `start` card itself, which the first `READPREV` returns and the program discards
    /// unfiltered.
    Backward {
        /// The browse key, the displayed page's first key.
        start: Option<CardRef>,
    },
}

/// Whether the error line already holds a message when the read starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ErrorLine {
    /// `WS-ERROR-MSG-OFF`: the read may set `'NO MORE RECORDS TO SHOW'`.
    Clear,
    /// A selection message the screen holds (COCRDLIC-014): the read sets
    /// `'NO MORE RECORDS TO SHOW'` only while no message is set, so it leaves this one.
    Held,
}

/// What one request does before it reads: the key, the read, and the paging state once the key
/// has moved the page number and reset the last-page flag. Built only by [`plan`] and
/// [`plan_reread`], and consumed by [`page`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListPlan {
    key: Option<Direction>,
    read: Read,
    state: ListState,
    page_size: usize,
    error_line: ErrorLine,
}

impl ListPlan {
    /// The browse to run.
    #[must_use]
    pub const fn read(&self) -> Read {
        self.read
    }

    /// The rows of one page, at least 1.
    #[must_use]
    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// The most rows the browse needs: the page size forward, one more backward, where the
    /// first row may be the start card that [`page`] drops (Bounded Pagination).
    #[must_use]
    pub const fn limit(&self) -> usize {
        match self.read {
            Read::Forward { .. } => self.page_size,
            Read::Backward { .. } => self.page_size.saturating_add(1),
        }
    }

    /// The row after which the look-ahead `READNEXT` reads, when the forward read filled the
    /// page: the last row of a full page (COCRDLIC-019). `None` for a backward read or a page
    /// the read did not fill, which need no look-ahead.
    ///
    /// The look-ahead is the next card in card-number order after that row, read without the
    /// filters, matching or not; its handle is the `lookahead` of [`page`]. Taking instead the
    /// next row of the filtered read would differ whenever a filter excludes that card
    /// (`X-COCRDLIC-3`).
    #[must_use]
    pub fn lookahead_after<'r>(&self, rows: &'r [CardListRow]) -> Option<&'r CardListRow> {
        match self.read {
            Read::Forward { .. } => rows.get(self.page_size.checked_sub(1)?),
            Read::Backward { .. } => None,
        }
    }
}

/// The last-page flag after the key: any key but F8 sets it to not shown before the read
/// (`COCRDLIC.cbl:410-414`, COCRDLIC-025).
const fn reset_last_page(key: Option<Direction>, mut state: ListState) -> ListState {
    if !matches!(key, Some(Direction::Forward)) {
        state.last_page = LastPage::NotShown;
    }
    state
}

/// The read a request with no input error runs, as `0000-MAIN`'s `EVALUATE` decides it from the
/// key and the echoed paging state (COCRDLIC-018, COCRDLIC-023 to COCRDLIC-027, COCRDLIC-030):
///
/// - F8 while a next page exists: the page number goes up by one and the read runs forward from
///   the last key (COCRDLIC-023, COCRDLIC-030);
/// - F7 on a page other than 1: the page number goes down by one and the read runs backward from
///   the first key (COCRDLIC-027, COCRDLIC-030);
/// - F7 on page 1 (COCRDLIC-026), F8 with no next page (COCRDLIC-024) and ENTER
///   (COCRDLIC-018): the read runs forward from the first key and the page number is unchanged.
///
/// A fresh start is ENTER with [`ListState::fresh`], whose read starts below every card
/// (COCRDLIC-017). Any key but F8 also resets the last-page flag (COCRDLIC-025). A page size of
/// 0 is read as 1; `api` admits 1 to 100 and the screen sends none ([`DEFAULT_PAGE_SIZE`]).
#[must_use]
pub fn plan(key: Option<Direction>, state: ListState, page_size: usize) -> ListPlan {
    let mut state = reset_last_page(key, state);
    let read = match key {
        Some(Direction::Forward) if state.next_page == NextPage::Exists => {
            state.page_no = state.page_no.plus_one();
            Read::Forward {
                start: state.last_key,
            }
        }
        Some(Direction::Backward) if !state.page_no.is_first() => {
            state.page_no = state.page_no.minus_one();
            Read::Backward {
                start: state.first_key,
            }
        }
        _ => Read::Forward {
            start: state.first_key,
        },
    };
    ListPlan {
        key,
        read,
        state,
        page_size: page_size.max(1),
        error_line: ErrorLine::Clear,
    }
}

/// The re-read after a selection error that the screen found (COCRDLIC-014): whatever the key,
/// the read runs forward from the start of the card file under the filters, not from the
/// displayed page, and the page number is unchanged.
///
/// The key still decides the messages after the read (COCRDLIC-024, COCRDLIC-026), and any key
/// but F8 resets the last-page flag (COCRDLIC-025). The selection message the screen holds stays
/// on the error line: the read sets `'NO MORE RECORDS TO SHOW'` only while no message is set, so
/// [`CardPage::message`] is `None` unless a message that replaces it applies:
/// `'NO RECORDS FOUND FOR THIS SEARCH CONDITION.'`, `'NO PREVIOUS PAGES TO DISPLAY'` or
/// `'NO MORE PAGES TO DISPLAY'`.
#[must_use]
pub fn plan_reread(key: Option<Direction>, state: ListState, page_size: usize) -> ListPlan {
    ListPlan {
        key,
        read: Read::Forward { start: None },
        state: reset_last_page(key, state),
        page_size: page_size.max(1),
        error_line: ErrorLine::Held,
    }
}

/// One filter after its edit.
enum FilterEdit<T> {
    /// Not supplied: absent, all spaces, all low-values or all `0` digits.
    Blank,
    /// Supplied and all digits.
    Valid(T),
    /// Supplied and not all digits: the class test fails.
    Invalid,
}

/// Whether a moved filter field is `LOW-VALUES`, `SPACES` or all `0` digits: the three
/// not-supplied tests of `2210-EDIT-ACCOUNT` and `2220-EDIT-CARD` (`COCRDLIC.cbl:1007-1009,
/// 1042-1044`; COCRDLIC-005, COCRDLIC-007).
fn not_supplied(field: &[u8]) -> bool {
    [0_u8, b' ', b'0']
        .iter()
        .any(|&fill| field.iter().all(|&byte| byte == fill))
}

/// The not-supplied tests, then the `IS NUMERIC` class test of an alphanumeric field: all ASCII
/// digits.
fn numeric<const N: usize>(field: PicX<N>) -> FilterEdit<PicX<N>> {
    if not_supplied(field.as_bytes()) {
        FilterEdit::Blank
    } else if field.as_bytes().iter().all(u8::is_ascii_digit) {
        FilterEdit::Valid(field)
    } else {
        // HALTED(X-COCRDLIC-H2): the zero test runs on non-numeric data before this class test;
        // its result depends on compiler options, and this is the evident path.
        FilterEdit::Invalid
    }
}

/// `2210-EDIT-ACCOUNT` over the account filter text, moved into `CC-ACCT-ID PIC X(11)`.
fn edit_account(text: &str) -> FilterEdit<PicU<11>> {
    match numeric(PicX::<ACCT_FILTER_WIDTH>::moved_from(text.as_bytes())) {
        FilterEdit::Blank => FilterEdit::Blank,
        FilterEdit::Invalid => FilterEdit::Invalid,
        // Eleven ASCII digits always decode into `PIC 9(11)`; the arm is total regardless.
        FilterEdit::Valid(field) => {
            PicU::decode(field.as_bytes()).map_or(FilterEdit::Invalid, FilterEdit::Valid)
        }
    }
}

/// `2220-EDIT-CARD` over the card filter text, moved into `CC-CARD-NUM PIC X(16)`.
fn edit_card(text: &str) -> FilterEdit<CardNumber> {
    match numeric(PicX::<CARD_FILTER_WIDTH>::moved_from(text.as_bytes())) {
        FilterEdit::Blank => FilterEdit::Blank,
        FilterEdit::Invalid => FilterEdit::Invalid,
        FilterEdit::Valid(field) => FilterEdit::Valid(CardNumber::new(field)),
    }
}

/// The filters of one request after `2210-EDIT-ACCOUNT` and `2220-EDIT-CARD`: each is either
/// absent or valid. Built by [`edit_filters`]; [`Default`] is the value two blank filters give.
///
/// The card filter is request input, never egress: its `Debug` is the redacted form, and the
/// service passes it to the card page query as a bind (Cardholder Data Redaction).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CardFilters {
    account: Option<PicU<11>>,
    card: Option<CardNumber>,
}

impl CardFilters {
    /// The account filter, `FLG-ACCTFILTER-ISVALID`: never zero, which is no filter
    /// (COCRDLIC-005, COCRDLIC-010).
    #[must_use]
    pub const fn account(&self) -> Option<PicU<11>> {
        self.account
    }

    /// The card filter, `FLG-CARDFILTER-ISVALID`: sixteen digits, never all `0`, which is no
    /// filter (COCRDLIC-007, COCRDLIC-010).
    #[must_use]
    pub const fn card(&self) -> Option<&CardNumber> {
        self.card.as_ref()
    }

    /// `9500-FILTER-RECORDS` (`COCRDLIC.cbl:1382-1411`, COCRDLIC-010): whether `card` is listed.
    ///
    /// A card is excluded when the account filter is set and the card's account differs, and
    /// otherwise when the card filter is set and the card's number differs; both filters
    /// together require both to match. The card page queries apply the same rule in SQL, and
    /// this function lets tests cross-check them. It does not apply to the look-ahead
    /// (COCRDLIC-019).
    #[must_use]
    pub fn includes(&self, card: &CardRecord) -> bool {
        self.account.is_none_or(|account| card.acct_id == account)
            && self.card.as_ref().is_none_or(|number| card.num == *number)
    }
}

/// `2210-EDIT-ACCOUNT` then `2220-EDIT-CARD` over the raw filter text of a request
/// (COCRDLIC-005 to COCRDLIC-010).
///
/// Each text is read as a `MOVE` into its field: its first 11 or 16 bytes, padded with spaces. A
/// filter is not supplied when it is absent, all spaces, all low-values (`\0`) or all `0` digits
/// (COCRDLIC-005, COCRDLIC-007), and valid when it is all digits (COCRDLIC-010). Both edits run;
/// the card message is moved only while no message is set, so with both in error the account
/// message stands and both filters are marked (COCRDLIC-009).
///
/// `HALTED(X-COCRDLIC-H2)`: which other texts the COBOL rejects is not fixed by the repository;
/// every text that is neither blank nor all digits fails here, the program's evident path, which
/// is no parity result (COCRDLIC-006, COCRDLIC-008, COCRDLIC-009).
///
/// # Errors
///
/// A [`ListFailure`] of class [`FailureClass::Input`] (400) with the account message
/// `'ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER'` when the account filter is invalid,
/// otherwise with the card message `'CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER'`.
/// The failure holds no filter text.
pub fn edit_filters(acct: Option<&str>, card: Option<&str>) -> Result<CardFilters, ListFailure> {
    let account = edit_account(acct.unwrap_or_default());
    let card = edit_card(card.unwrap_or_default());
    let errors = match (&account, &card) {
        (FilterEdit::Invalid, FilterEdit::Invalid) => Some(FilterErrors::Both),
        (FilterEdit::Invalid, _) => Some(FilterErrors::Account),
        (_, FilterEdit::Invalid) => Some(FilterErrors::Card),
        _ => None,
    };
    if let Some(errors) = errors {
        return Err(ListFailure::filters(errors));
    }
    Ok(CardFilters {
        account: match account {
            FilterEdit::Valid(value) => Some(value),
            FilterEdit::Blank | FilterEdit::Invalid => None,
        },
        card: match card {
            FilterEdit::Valid(value) => Some(value),
            FilterEdit::Blank | FilterEdit::Invalid => None,
        },
    })
}

/// One listed card (COCRDLIC-032): the fields `9000-READ-FORWARD` and `9100-READ-BACKWARDS`
/// move into `WS-SCREEN-ROWS` (`COCRDLIC.cbl:1165-1171,1338-1344`), with the card's handle.
///
/// The COBOL shows the full card number; here it is a [`CardNumber`], whose `Debug`, `Display`
/// and `Serialize` emit only twelve `*` and its last four digits (Cardholder Data Redaction,
/// `E32`). No CVV, embossed name or expiry date is listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardListRow {
    /// The card's handle, which a selection carries to `/cards/view` or `/cards/update`.
    pub card_ref: CardRef,
    /// `WS-ROW-CARD-NUM` from `CARD-NUM PIC X(16)`; redacted wherever it is rendered.
    pub card_num: CardNumber,
    /// `WS-ROW-ACCTNO` from `CARD-ACCT-ID PIC 9(11)`, shown as its eleven digits.
    pub acct_id: PicU<11>,
    /// `WS-ROW-CARD-STATUS` from `CARD-ACTIVE-STATUS PIC X(01)`.
    pub status: PicX<1>,
}

impl CardListRow {
    /// The row of the card `record`, whose handle is `card_ref`.
    #[must_use]
    pub fn new(card_ref: CardRef, record: &CardRecord) -> Self {
        Self {
            card_ref,
            card_num: record.num.clone(),
            acct_id: record.acct_id,
            status: record.active_status.clone(),
        }
    }
}

impl From<(CardRef, CardRecord)> for CardListRow {
    /// The row of a card as the card repository returns it, handle first.
    fn from((card_ref, record): (CardRef, CardRecord)) -> Self {
        Self {
            card_ref,
            card_num: record.num,
            acct_id: record.acct_id,
            status: record.active_status,
        }
    }
}

/// The message a page puts on the error line, `WS-ERROR-MSG PIC X(75)` moved to `ERRMSGO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListMessage {
    /// `'NO RECORDS FOUND FOR THIS SEARCH CONDITION.'`: page 1 lists no card (COCRDLIC-021).
    NoRecordsFound,
    /// `'NO MORE RECORDS TO SHOW'`: a forward read found no next page (COCRDLIC-019,
    /// COCRDLIC-020, COCRDLIC-021).
    NoMoreRecords,
    /// `'NO PREVIOUS PAGES TO DISPLAY'`: F7 ended on page 1 (COCRDLIC-026, COCRDLIC-028).
    NoPreviousPages,
    /// `'NO MORE PAGES TO DISPLAY'`: F8 on a last page already shown (COCRDLIC-024).
    NoMorePages,
    /// The file-error text: a backward read reached the start of the card file (COCRDLIC-029,
    /// COCRDLIC-031); its code fields are `HALTED(H4)` (COCRDLIC-037).
    FileError,
}

impl ListMessage {
    /// The text as the program moves it; every one fits `WS-ERROR-MSG PIC X(75)`. The literals
    /// come without the field's padding; the file-error text is the field's whole 75 bytes.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::NoRecordsFound => NO_RECORDS_FOUND,
            Self::NoMoreRecords => NO_MORE_RECORDS,
            Self::NoPreviousPages => NO_PREVIOUS_PAGES,
            Self::NoMorePages => NO_MORE_PAGES,
            Self::FileError => FILE_ERROR,
        }
    }
}

/// Whether the information line `INFOMSGO PIC X(45)` is shown (COCRDLIC-033).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum InfoLine {
    /// `WS-INFORM-REC-ACTIONS`.
    RecordActions,
    /// `WS-NO-INFO-MESSAGE`, or suppressed by the no-records message.
    Empty,
}

/// One page of the card list: what `1000-SEND-MAP` sends after a read, as the response of
/// endpoint 7 carries it (COCRDLIC-017 to COCRDLIC-034). Built only by [`page`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardPage {
    rows: Vec<CardListRow>,
    blank_rows_above: usize,
    state: ListState,
    message: Option<ListMessage>,
    info: InfoLine,
}

impl CardPage {
    /// The listed cards in screen order, ascending by card number, at most the page size
    /// (COCRDLIC-032). They fill the rows from row [`CardPage::blank_rows_above`] + 1 down.
    #[must_use]
    pub fn rows(&self) -> &[CardListRow] {
        &self.rows
    }

    /// The listed cards, taking them out of the page; read the other fields first.
    #[must_use]
    pub fn into_rows(self) -> Vec<CardListRow> {
        self.rows
    }

    /// The empty rows above the first listed card: 0, except after a backward read that
    /// reached the start of the card file, which fills the rows from the last one upward
    /// (COCRDLIC-029).
    #[must_use]
    pub const fn blank_rows_above(&self) -> usize {
        self.blank_rows_above
    }

    /// The paging state the screen echoes with its next request (`X-COCRDLIC-2`).
    #[must_use]
    pub const fn state(&self) -> ListState {
        self.state
    }

    /// The page number `PAGENO` shows (COCRDLIC-022, COCRDLIC-030).
    #[must_use]
    pub const fn page_no(&self) -> PageNumber {
        self.state.page_no
    }

    /// The handle of the page's first key, where ENTER and F7 read from (COCRDLIC-022,
    /// COCRDLIC-027).
    #[must_use]
    pub const fn first_key(&self) -> Option<CardRef> {
        self.state.first_key
    }

    /// The handle of the page's last key, where F8 reads from: the look-ahead card when a next
    /// page exists (COCRDLIC-019, COCRDLIC-027).
    #[must_use]
    pub const fn last_key(&self) -> Option<CardRef> {
        self.state.last_key
    }

    /// Whether F8 moves to a next page.
    #[must_use]
    pub const fn next_page(&self) -> NextPage {
        self.state.next_page
    }

    /// Whether the last page has been shown by F8 (COCRDLIC-024, COCRDLIC-025).
    #[must_use]
    pub const fn last_page(&self) -> LastPage {
        self.state.last_page
    }

    /// The error line's message, as a kind (COCRDLIC-034).
    #[must_use]
    pub const fn message_kind(&self) -> Option<ListMessage> {
        self.message
    }

    /// The error line's text; `None` when the line is empty, or after [`plan_reread`] when the
    /// screen's own selection message stays (COCRDLIC-014, COCRDLIC-034).
    #[must_use]
    pub fn message(&self) -> Option<&'static str> {
        self.message.map(ListMessage::text)
    }

    /// The information line, `'TYPE S FOR DETAIL, U TO UPDATE ANY RECORD'` or `None`
    /// (COCRDLIC-033).
    #[must_use]
    pub const fn info_message(&self) -> Option<&'static str> {
        match self.info {
            InfoLine::RecordActions => Some(INFO_RECORD_ACTIONS),
            InfoLine::Empty => None,
        }
    }
}

/// The page a request shows, from the rows its [`ListPlan`] read (COCRDLIC-017 to
/// COCRDLIC-034).
///
/// `rows` are the read's rows exactly as the card page query returns them, at most
/// [`ListPlan::limit`]: forward, the matching cards from the start key inclusive in ascending
/// order; backward, the matching cards from the start key inclusive in descending order. Rows
/// beyond the page are not listed. `lookahead` is the handle of the card after the row that
/// [`ListPlan::lookahead_after`] names, read without the filters, and `None` when no card
/// follows it or no look-ahead was asked for.
///
/// Forward (`9000-READ-FORWARD`): the first listed card becomes the first key and a page number
/// of 0 becomes 1 (COCRDLIC-022). A full page with a look-ahead has a next page and the
/// look-ahead as its last key (COCRDLIC-019); a full page without one, or a short page, has
/// none and `'NO MORE RECORDS TO SHOW'` (COCRDLIC-019, COCRDLIC-020), replaced by
/// `'NO RECORDS FOUND FOR THIS SEARCH CONDITION.'` when page 1 lists no card (COCRDLIC-021).
///
/// Backward (`9100-READ-BACKWARDS`): the last key becomes the displayed page's first key, the
/// start card is dropped and the cards before it fill the page from its last row upward; a next
/// page exists. A full page makes its row 1 the first key (COCRDLIC-027); a short one leaves the
/// first key, leaves rows empty above and shows the file-error text (COCRDLIC-029).
///
/// Then `1400-SETUP-MESSAGE` (COCRDLIC-033, COCRDLIC-034): F7 ending on page 1 shows
/// `'NO PREVIOUS PAGES TO DISPLAY'` (COCRDLIC-026, COCRDLIC-028); F8 with no next page shows
/// `'NO MORE PAGES TO DISPLAY'` once the last page was shown (COCRDLIC-024) and otherwise marks
/// it shown (COCRDLIC-025). Each of those two replaces the read's message and empties the
/// information line; otherwise the line shows `'TYPE S FOR DETAIL, U TO UPDATE ANY RECORD'`
/// unless the error line holds the no-records message.
///
/// It takes the whole [`ListPlan`] rather than only a direction and a page number, because the
/// keys, the last-page flag and the messages depend on the key and on the echoed state together
/// (COCRDLIC-024 to COCRDLIC-026).
#[must_use]
pub fn page(plan: &ListPlan, rows: Vec<CardListRow>, lookahead: Option<CardRef>) -> CardPage {
    let read = match plan.read {
        Read::Forward { .. } => read_forward(plan, rows, lookahead),
        Read::Backward { start } => read_backward(plan, start, rows),
    };
    setup_message(plan.key, read)
}

/// `9000-READ-FORWARD` over the rows of a forward read (`COCRDLIC.cbl:1123-1263`).
fn read_forward(plan: &ListPlan, rows: Vec<CardListRow>, lookahead: Option<CardRef>) -> CardPage {
    let mut state = plan.state;
    let listed: Vec<CardListRow> = rows.into_iter().take(plan.page_size).collect();
    let no_more_records = match plan.error_line {
        ErrorLine::Clear => Some(ListMessage::NoMoreRecords),
        ErrorLine::Held => None,
    };
    if let Some(first) = listed.first() {
        state.first_key = Some(first.card_ref);
        if state.page_no == PageNumber::ZERO {
            state.page_no = PageNumber::FIRST;
        }
    }
    let last_listed = listed.last().map(|row| row.card_ref);
    let message = if listed.len() == plan.page_size {
        // The page's last row is filled: the look-ahead `READNEXT` (COCRDLIC-019).
        if let Some(next) = lookahead {
            state.next_page = NextPage::Exists;
            state.last_key = Some(next);
            None
        } else {
            state.next_page = NextPage::NotExists;
            state.last_key = last_listed;
            no_more_records
        }
    } else {
        // `ENDFILE` before the page is full (COCRDLIC-020, COCRDLIC-021). The COBOL's last key is
        // the last card read, which may be one the filters excluded; no later key uses it while
        // no next page exists, so the last listed card stands in for it.
        state.next_page = NextPage::NotExists;
        state.last_key = last_listed.or(state.last_key);
        if listed.is_empty() && state.page_no.is_first() {
            Some(ListMessage::NoRecordsFound)
        } else {
            no_more_records
        }
    };
    CardPage {
        rows: listed,
        blank_rows_above: 0,
        state,
        message,
        info: InfoLine::Empty,
    }
}

/// `9100-READ-BACKWARDS` over the rows of a backward read (`COCRDLIC.cbl:1264-1380`).
fn read_backward(plan: &ListPlan, start: Option<CardRef>, rows: Vec<CardListRow>) -> CardPage {
    let mut state = plan.state;
    state.last_key = state.first_key;
    state.next_page = NextPage::Exists;
    // The first `READPREV` returns the start card itself, which the program discards unfiltered;
    // the query has already left it out when a filter excludes it.
    let mut found: Vec<CardListRow> = rows
        .into_iter()
        .filter(|row| Some(row.card_ref) != start)
        .take(plan.page_size)
        .collect();
    let message = if found.len() == plan.page_size {
        state.first_key = found.last().map(|row| row.card_ref);
        None
    } else {
        // The start of the file: `ENDFILE` falls into the error branch (COCRDLIC-029).
        Some(ListMessage::FileError)
    };
    let blank_rows_above = plan.page_size.saturating_sub(found.len());
    found.reverse();
    CardPage {
        rows: found,
        blank_rows_above,
        state,
        message,
        info: InfoLine::Empty,
    }
}

/// `1400-SETUP-MESSAGE` with no filter in error (`COCRDLIC.cbl:895-930`).
fn setup_message(key: Option<Direction>, mut page: CardPage) -> CardPage {
    let last_page_reached = page.state.next_page == NextPage::NotExists;
    match key {
        Some(Direction::Backward) if page.state.page_no.is_first() => {
            page.message = Some(ListMessage::NoPreviousPages);
        }
        Some(Direction::Forward)
            if last_page_reached && page.state.last_page == LastPage::Shown =>
        {
            page.message = Some(ListMessage::NoMorePages);
        }
        Some(Direction::Forward) if last_page_reached => {
            page.state.last_page = LastPage::Shown;
            page.info = InfoLine::RecordActions;
        }
        _ => page.info = InfoLine::RecordActions,
    }
    if page.message == Some(ListMessage::NoRecordsFound) {
        page.info = InfoLine::Empty;
    }
    page
}

/// The request field a filter failure names, where the cursor goes (COCRDLIC-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListField {
    /// `ACCTSID`, request field `acct_id`.
    Account,
    /// `CARDSID`, request field `card_num`.
    Card,
}

/// The filters a failed edit marks in error, each shown in red (COCRDLIC-009, COCRDLIC-011).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterErrors {
    /// `FLG-ACCTFILTER-NOT-OK` only.
    Account,
    /// `FLG-CARDFILTER-NOT-OK` only.
    Card,
    /// Both flags.
    Both,
}

/// The status class of a [`ListFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// A filter edit failed: 400 (COCRDLIC-006, COCRDLIC-008, COCRDLIC-009).
    Input,
    /// Reading the cards failed: 500 (COCRDLIC-031).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 400 for [`FailureClass::Input`], 500 for
    /// [`FailureClass::Store`].
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Input => 400,
            Self::Store => 500,
        }
    }
}

/// An answer to endpoint 7 that is no page: a filter edit failure, or a failed card read.
///
/// The message is the program's literal; it holds no filter text and no card number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListFailure {
    message: &'static str,
    filters: Option<FilterErrors>,
}

impl ListFailure {
    /// A filter edit failure: the account message when the account filter is in error,
    /// otherwise the card message (COCRDLIC-006, COCRDLIC-008, COCRDLIC-009).
    const fn filters(errors: FilterErrors) -> Self {
        let message = match errors {
            FilterErrors::Account | FilterErrors::Both => ACCOUNT_FILTER_INVALID,
            FilterErrors::Card => CARD_FILTER_INVALID,
        };
        Self {
            message,
            filters: Some(errors),
        }
    }

    /// A card read that failed in the store, as the service reports a repository error while
    /// browsing (COCRDLIC-031): 500 with the file-error text, whose code fields are
    /// `HALTED(H4)` (COCRDLIC-037). No row read before the failure is returned.
    #[must_use]
    pub const fn file_error() -> Self {
        Self {
            message: FILE_ERROR,
            filters: None,
        }
    }

    /// The message, as the response's `ScreenError` carries it.
    #[must_use]
    pub const fn message(&self) -> &'static str {
        self.message
    }

    /// The status class: [`FailureClass::Input`] for a filter edit, [`FailureClass::Store`]
    /// for a failed read.
    #[must_use]
    pub const fn class(&self) -> FailureClass {
        match self.filters {
            Some(_) => FailureClass::Input,
            None => FailureClass::Store,
        }
    }

    /// The field the message names and the cursor goes to: the account filter when it is in
    /// error, otherwise the card filter (COCRDLIC-011); `None` for a failed read.
    #[must_use]
    pub const fn field(&self) -> Option<ListField> {
        match self.filters {
            Some(FilterErrors::Account | FilterErrors::Both) => Some(ListField::Account),
            Some(FilterErrors::Card) => Some(ListField::Card),
            None => None,
        }
    }

    /// The filters marked in error; `None` for a failed read.
    #[must_use]
    pub const fn filter_errors(&self) -> Option<FilterErrors> {
        self.filters
    }
}

impl fmt::Display for ListFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for ListFailure {}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;
    use std::path::Path;

    use copybook::ErrorKind;
    use copybook::framing::decode_all;

    use super::{
        CardFilters, CardListRow, CardPage, CardRef, DEFAULT_PAGE_SIZE, Direction, FailureClass,
        FilterErrors, LastPage, ListFailure, ListField, ListMessage, ListPlan, ListState, NextPage,
        PageNumber, Read, edit_filters, page, plan, plan_reread,
    };
    use crate::records::card::CardRecord;

    type TestResult = Result<(), Box<dyn StdError>>;

    /// One card with its handle, as the card repository returns it.
    type Card = (CardRef, CardRecord);

    /// The 50 cards of `carddata.txt`, read in place, in card-number order, each with the handle
    /// the `cardfile` load gives it: its line number.
    fn fixture() -> Result<Vec<Card>, Box<dyn StdError>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../app/data/ASCII/carddata.txt");
        let framed = decode_all::<CardRecord>(&std::fs::read(path)?)?;
        Ok(framed
            .into_iter()
            .zip(1_i64..)
            .map(|(line, handle)| (CardRef::new(handle), line.record))
            .collect())
    }

    /// The card whose handle is `handle`.
    fn card(cards: &[Card], handle: i64) -> Result<&CardRecord, Box<dyn StdError>> {
        cards
            .iter()
            .find(|(card_ref, _)| card_ref.get() == handle)
            .map(|(_, record)| record)
            .ok_or_else(|| format!("no fixture card {handle}").into())
    }

    /// The account filter text that selects `record`: its eleven digits.
    fn account_text(record: &CardRecord) -> String {
        format!("{:011}", record.acct_id.get())
    }

    /// The card filter text that selects `record`: its sixteen digits.
    fn card_text(record: &CardRecord) -> Result<String, Box<dyn StdError>> {
        Ok(String::from_utf8(record.num.expose().as_bytes().to_vec())?)
    }

    /// Runs `plan` over `cards` as the service and the card page queries do: the filtered browse
    /// of [`ListPlan::read`] within [`ListPlan::limit`], then the unfiltered look-ahead.
    fn run(plan: &ListPlan, filters: &CardFilters, cards: &[Card]) -> CardPage {
        let at = |start: Option<CardRef>| {
            start.and_then(|key| cards.iter().position(|(card_ref, _)| *card_ref == key))
        };
        let row = |(card_ref, record): &Card| CardListRow::new(*card_ref, record);
        let rows: Vec<CardListRow> = match plan.read() {
            Read::Forward { start } => cards
                .iter()
                .skip(at(start).unwrap_or(0))
                .filter(|(_, record)| filters.includes(record))
                .take(plan.limit())
                .map(row)
                .collect(),
            Read::Backward { start } => cards
                .iter()
                .take(at(start).map_or(0, |index| index + 1))
                .rev()
                .filter(|(_, record)| filters.includes(record))
                .take(plan.limit())
                .map(row)
                .collect(),
        };
        let lookahead = plan.lookahead_after(&rows).and_then(|after| {
            cards
                .iter()
                .skip_while(|(card_ref, _)| *card_ref != after.card_ref)
                .nth(1)
                .map(|(card_ref, _)| *card_ref)
        });
        page(plan, rows, lookahead)
    }

    /// The handles a page lists, in screen order.
    fn handles(page: &CardPage) -> Vec<i64> {
        page.rows().iter().map(|row| row.card_ref.get()).collect()
    }

    /// The handles `first` to `last`.
    fn span(first: i64, last: i64) -> Vec<i64> {
        (first..=last).collect()
    }

    /// A paging state with the given page number and keys, a next page and the flag not shown.
    fn state(page_no: u8, first: i64, last: i64) -> Result<ListState, Box<dyn StdError>> {
        Ok(ListState {
            page_no: PageNumber::new(page_no).ok_or("page number 0-9")?,
            first_key: Some(CardRef::new(first)),
            last_key: Some(CardRef::new(last)),
            next_page: NextPage::Exists,
            last_page: LastPage::NotShown,
        })
    }

    const INFO: Option<&str> = Some("TYPE S FOR DETAIL, U TO UPDATE ANY RECORD");

    // audit: COCRDLIC-017
    #[test]
    fn the_default_page_is_the_seven_screen_rows() {
        assert_eq!(DEFAULT_PAGE_SIZE, 7);
        assert_eq!(ListState::fresh(), ListState::default());
        assert_eq!(ListState::fresh().page_no, PageNumber::FIRST);
    }

    // audit: COCRDLIC-005
    // audit: COCRDLIC-007
    #[test]
    fn absent_blank_and_zero_filters_are_no_filter() -> TestResult {
        let blanks = [
            (None, None),
            (Some(""), Some("")),
            (Some("           "), Some("                ")),
            (
                Some("\0\0\0\0\0\0\0\0\0\0\0"),
                Some("\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
            ),
            (Some("00000000000"), Some("0000000000000000")),
        ];
        for (acct, card) in blanks {
            let filters = edit_filters(acct, card)?;
            assert_eq!(filters, CardFilters::default());
            assert_eq!(filters.account(), None);
            assert!(filters.card().is_none());
        }
        Ok(())
    }

    // audit: COCRDLIC-010
    #[test]
    fn valid_filters_list_only_the_cards_matching_both() -> TestResult {
        let cards = fixture()?;
        let (one, two) = (card(&cards, 1)?, card(&cards, 2)?);
        let by_account = edit_filters(Some(&account_text(one)), None)?;
        assert_eq!(by_account.account(), Some(one.acct_id));
        assert!(by_account.includes(one) && !by_account.includes(two));
        let by_card = edit_filters(None, Some(&card_text(two)?))?;
        assert!(by_card.card() == Some(&two.num), "COCRDLIC-010");
        assert!(by_card.includes(two) && !by_card.includes(one));
        let both = edit_filters(Some(&account_text(one)), Some(&card_text(one)?))?;
        assert!(both.includes(one) && !both.includes(two));
        let crossed = edit_filters(Some(&account_text(one)), Some(&card_text(two)?))?;
        assert!(!crossed.includes(one) && !crossed.includes(two));

        // One card per account: each filter lists one card, on page 1 with no next page.
        for filters in [&by_account, &by_card, &both] {
            let listed = run(&plan(None, ListState::fresh(), 7), filters, &cards);
            assert_eq!(listed.rows().len(), 1);
            assert_eq!(listed.next_page(), NextPage::NotExists);
            assert_eq!(listed.message_kind(), Some(ListMessage::NoMoreRecords));
            assert_eq!(listed.info_message(), INFO);
        }
        Ok(())
    }

    // The filter-error rows are HALTED(X-COCRDLIC-H2): these are the replacement's answers on
    // the program's evident path, not parity results, so the test carries no tag.
    #[test]
    fn filters_that_are_not_all_digits_answer_400_with_the_account_message_first() -> TestResult {
        let cases = [
            (Some("50"), None, FilterErrors::Account, ListField::Account),
            (
                Some("0000000005A"),
                Some(""),
                FilterErrors::Account,
                ListField::Account,
            ),
            (None, Some("1234"), FilterErrors::Card, ListField::Card),
            (
                Some("x"),
                Some("1234"),
                FilterErrors::Both,
                ListField::Account,
            ),
        ];
        for (acct, card, errors, field) in cases {
            let failure = edit_filters(acct, card)
                .err()
                .ok_or("a filter that is not all digits fails its edit")?;
            let expected = match errors {
                FilterErrors::Card => "CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER",
                FilterErrors::Account | FilterErrors::Both => {
                    "ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER"
                }
            };
            assert_eq!(failure.message(), expected);
            assert_eq!(failure.to_string(), expected);
            assert_eq!(failure.filter_errors(), Some(errors));
            assert_eq!(failure.field(), Some(field));
            assert_eq!(failure.class(), FailureClass::Input);
            assert_eq!(failure.class().http_status(), 400);
            assert!(!failure.message().contains("1234"));
        }
        Ok(())
    }

    // audit: COCRDLIC-017
    // audit: COCRDLIC-019
    // audit: COCRDLIC-022
    // audit: COCRDLIC-033
    #[test]
    fn a_fresh_start_lists_seven_cards_and_keeps_the_look_ahead_as_its_last_key() -> TestResult {
        let cards = fixture()?;
        let fresh = plan(None, ListState::fresh(), DEFAULT_PAGE_SIZE);
        assert_eq!(fresh.read(), Read::Forward { start: None });
        assert_eq!(fresh.limit(), 7);
        let first = run(&fresh, &CardFilters::default(), &cards);
        assert_eq!(handles(&first), span(1, 7));
        assert_eq!(first.blank_rows_above(), 0);
        assert_eq!(first.page_no(), PageNumber::FIRST);
        assert_eq!(first.first_key(), Some(CardRef::new(1)));
        // The look-ahead card, not the seventh row, is the last key (X-COCRDLIC-3).
        assert_eq!(first.last_key(), Some(CardRef::new(8)));
        assert_eq!(first.next_page(), NextPage::Exists);
        assert_eq!(first.last_page(), LastPage::NotShown);
        assert_eq!(first.message(), None);
        assert_eq!(first.info_message(), INFO);
        Ok(())
    }

    // audit: COCRDLIC-018
    // audit: COCRDLIC-021
    // audit: COCRDLIC-023
    #[test]
    fn f8_pages_from_the_last_key_and_enter_rereads_from_the_first_key() -> TestResult {
        let cards = fixture()?;
        let none = CardFilters::default();
        let first = run(&plan(None, ListState::fresh(), 7), &none, &cards);
        let forward = plan(Some(Direction::Forward), first.state(), 7);
        assert_eq!(
            forward.read(),
            Read::Forward {
                start: Some(CardRef::new(8))
            }
        );
        let second = run(&forward, &none, &cards);
        assert_eq!(handles(&second), span(8, 14));
        assert_eq!(second.page_no().get(), 2);
        assert_eq!(second.first_key(), Some(CardRef::new(8)));
        assert_eq!(second.last_key(), Some(CardRef::new(15)));

        let enter = plan(None, second.state(), 7);
        assert_eq!(
            enter.read(),
            Read::Forward {
                start: Some(CardRef::new(8))
            }
        );
        let again = run(&enter, &none, &cards);
        assert_eq!(handles(&again), span(8, 14));
        assert_eq!(again.page_no().get(), 2);

        // A filter typed on page 2 still reads from its first key: an earlier card is not listed,
        // and the empty later page says no more records rather than no records found.
        let earlier = edit_filters(Some(&account_text(card(&cards, 3)?)), None)?;
        let filtered = run(&plan(None, second.state(), 7), &earlier, &cards);
        assert!(filtered.rows().is_empty());
        assert_eq!(filtered.page_no().get(), 2);
        assert_eq!(filtered.first_key(), Some(CardRef::new(8)));
        assert_eq!(filtered.message(), Some("NO MORE RECORDS TO SHOW"));
        assert_eq!(filtered.info_message(), INFO);
        Ok(())
    }

    // audit: COCRDLIC-019
    // audit: COCRDLIC-020
    // audit: COCRDLIC-024
    // audit: COCRDLIC-025
    #[test]
    fn f8_reaches_the_last_page_and_then_says_no_more_pages() -> TestResult {
        let cards = fixture()?;
        let none = CardFilters::default();
        let mut shown = run(&plan(None, ListState::fresh(), 7), &none, &cards);
        for _ in 2..=7 {
            shown = run(
                &plan(Some(Direction::Forward), shown.state(), 7),
                &none,
                &cards,
            );
        }
        assert_eq!(handles(&shown), span(43, 49));
        assert_eq!(shown.next_page(), NextPage::Exists);
        assert_eq!(shown.last_key(), Some(CardRef::new(50)));

        let last = run(
            &plan(Some(Direction::Forward), shown.state(), 7),
            &none,
            &cards,
        );
        assert_eq!(handles(&last), vec![50]);
        assert_eq!(last.page_no().get(), 8);
        assert_eq!(last.next_page(), NextPage::NotExists);
        assert_eq!(last.message(), Some("NO MORE RECORDS TO SHOW"));
        assert_eq!(last.info_message(), INFO);
        assert_eq!(last.last_page(), LastPage::Shown);

        let again_plan = plan(Some(Direction::Forward), last.state(), 7);
        assert_eq!(
            again_plan.read(),
            Read::Forward {
                start: Some(CardRef::new(50))
            }
        );
        let again = run(&again_plan, &none, &cards);
        assert_eq!(handles(&again), vec![50]);
        assert_eq!(again.page_no().get(), 8);
        assert_eq!(again.message(), Some("NO MORE PAGES TO DISPLAY"));
        assert_eq!(again.info_message(), None);

        // Any other key resets the flag, so the read's own message comes back.
        let enter = run(&plan(None, again.state(), 7), &none, &cards);
        assert_eq!(enter.last_page(), LastPage::NotShown);
        assert_eq!(enter.message(), Some("NO MORE RECORDS TO SHOW"));
        assert_eq!(enter.info_message(), INFO);
        Ok(())
    }

    // audit: COCRDLIC-019
    #[test]
    fn a_full_page_with_no_card_after_it_has_no_next_page() -> TestResult {
        let cards = fixture()?;
        let seven: Vec<Card> = cards.into_iter().take(7).collect();
        let full = run(
            &plan(None, ListState::fresh(), 7),
            &CardFilters::default(),
            &seven,
        );
        assert_eq!(handles(&full), span(1, 7));
        assert_eq!(full.next_page(), NextPage::NotExists);
        assert_eq!(full.last_key(), Some(CardRef::new(7)));
        assert_eq!(full.message(), Some("NO MORE RECORDS TO SHOW"));
        Ok(())
    }

    // audit: COCRDLIC-019
    #[test]
    fn the_look_ahead_ignores_the_filters() -> TestResult {
        let cards = fixture()?;
        let filters = edit_filters(Some(&account_text(card(&cards, 7)?)), None)?;
        let listed = run(&plan(None, ListState::fresh(), 1), &filters, &cards);
        assert_eq!(handles(&listed), vec![7]);
        // Card 8 belongs to another account, yet it makes a next page and the last key.
        assert_eq!(listed.next_page(), NextPage::Exists);
        assert_eq!(listed.last_key(), Some(CardRef::new(8)));
        let next = run(
            &plan(Some(Direction::Forward), listed.state(), 1),
            &filters,
            &cards,
        );
        assert!(next.rows().is_empty());
        assert_eq!(next.page_no().get(), 2);
        assert_eq!(next.message(), Some("NO MORE RECORDS TO SHOW"));
        Ok(())
    }

    // audit: COCRDLIC-021
    // audit: COCRDLIC-022
    // audit: COCRDLIC-034
    #[test]
    fn page_one_with_no_matching_card_says_no_records_found() -> TestResult {
        let cards = fixture()?;
        let crossed = edit_filters(
            Some(&account_text(card(&cards, 1)?)),
            Some(&card_text(card(&cards, 2)?)?),
        )?;
        let on_page_one = ListState {
            first_key: Some(CardRef::new(1)),
            ..ListState::fresh()
        };
        let empty = run(&plan(None, on_page_one, 7), &crossed, &cards);
        assert!(empty.rows().is_empty());
        assert_eq!(empty.page_no(), PageNumber::FIRST);
        assert_eq!(empty.first_key(), Some(CardRef::new(1)));
        assert_eq!(empty.next_page(), NextPage::NotExists);
        assert_eq!(
            empty.message(),
            Some("NO RECORDS FOUND FOR THIS SEARCH CONDITION.")
        );
        assert_eq!(empty.info_message(), None);

        // F7 on page 1 replaces even the no-records message.
        let back = run(
            &plan(Some(Direction::Backward), on_page_one, 7),
            &crossed,
            &cards,
        );
        assert_eq!(back.message(), Some("NO PREVIOUS PAGES TO DISPLAY"));
        assert_eq!(back.info_message(), None);
        Ok(())
    }

    // audit: COCRDLIC-026
    #[test]
    fn f7_on_page_one_rereads_it_and_says_no_previous_pages() -> TestResult {
        let cards = fixture()?;
        let none = CardFilters::default();
        let first = run(&plan(None, ListState::fresh(), 7), &none, &cards);
        let back = plan(Some(Direction::Backward), first.state(), 7);
        assert_eq!(
            back.read(),
            Read::Forward {
                start: Some(CardRef::new(1))
            }
        );
        let again = run(&back, &none, &cards);
        assert_eq!(handles(&again), span(1, 7));
        assert_eq!(again.page_no(), PageNumber::FIRST);
        assert_eq!(again.message(), Some("NO PREVIOUS PAGES TO DISPLAY"));
        assert_eq!(again.info_message(), None);
        Ok(())
    }

    // audit: COCRDLIC-027
    // audit: COCRDLIC-028
    #[test]
    fn f7_pages_back_from_the_first_key_and_page_one_says_no_previous_pages() -> TestResult {
        let cards = fixture()?;
        let none = CardFilters::default();
        let third = state(3, 15, 22)?;
        let back = plan(Some(Direction::Backward), third, 7);
        assert_eq!(
            back.read(),
            Read::Backward {
                start: Some(CardRef::new(15))
            }
        );
        assert_eq!(back.limit(), 8);
        let second = run(&back, &none, &cards);
        assert_eq!(handles(&second), span(8, 14));
        assert_eq!(second.blank_rows_above(), 0);
        assert_eq!(second.page_no().get(), 2);
        assert_eq!(second.first_key(), Some(CardRef::new(8)));
        assert_eq!(second.last_key(), Some(CardRef::new(15)));
        assert_eq!(second.next_page(), NextPage::Exists);
        assert_eq!(second.message(), None);
        assert_eq!(second.info_message(), INFO);

        let first = run(
            &plan(Some(Direction::Backward), second.state(), 7),
            &none,
            &cards,
        );
        assert_eq!(handles(&first), span(1, 7));
        assert_eq!(first.page_no(), PageNumber::FIRST);
        assert_eq!(first.first_key(), Some(CardRef::new(1)));
        assert_eq!(first.last_key(), Some(CardRef::new(8)));
        assert_eq!(first.message(), Some("NO PREVIOUS PAGES TO DISPLAY"));
        assert_eq!(first.info_message(), None);
        Ok(())
    }

    // audit: COCRDLIC-029
    // audit: COCRDLIC-031
    #[test]
    fn a_backward_read_that_reaches_the_first_card_shows_the_file_error_text() -> TestResult {
        let cards = fixture()?;
        let short = run(
            &plan(Some(Direction::Backward), state(3, 4, 11)?, 7),
            &CardFilters::default(),
            &cards,
        );
        assert_eq!(handles(&short), span(1, 3));
        assert_eq!(short.blank_rows_above(), 4);
        assert_eq!(short.page_no().get(), 2);
        assert_eq!(short.first_key(), Some(CardRef::new(4)));
        assert_eq!(short.last_key(), Some(CardRef::new(4)));
        assert_eq!(short.next_page(), NextPage::Exists);
        assert_eq!(short.message_kind(), Some(ListMessage::FileError));
        assert_eq!(short.info_message(), INFO);
        let text = short.message().ok_or("file-error text")?;
        assert_eq!(text.len(), 75);
        assert_eq!(
            text.get(..48),
            Some("File Error: READ     on CARDDAT   returned RESP ")
        );
        assert_eq!(text.get(58..65), Some(",RESP2 "));
        Ok(())
    }

    // audit: COCRDLIC-031
    #[test]
    fn a_failed_card_read_is_a_500_with_the_file_error_text() {
        let failure = ListFailure::file_error();
        assert_eq!(failure.class(), FailureClass::Store);
        assert_eq!(failure.class().http_status(), 500);
        assert_eq!(failure.field(), None);
        assert_eq!(failure.filter_errors(), None);
        assert_eq!(failure.message(), ListMessage::FileError.text());
        assert_eq!(
            failure.message().get(..48),
            Some("File Error: READ     on CARDDAT   returned RESP ")
        );
        assert_eq!(failure.message().get(58..65), Some(",RESP2 "));
    }

    // audit: COCRDLIC-022
    // audit: COCRDLIC-030
    #[test]
    fn the_one_digit_page_number_wraps() -> TestResult {
        let cards = fixture()?;
        let none = CardFilters::default();
        let ninth = plan(Some(Direction::Forward), state(9, 1, 8)?, 7);
        assert_eq!(
            ninth.read(),
            Read::Forward {
                start: Some(CardRef::new(8))
            }
        );
        assert_eq!(run(&ninth, &none, &cards).page_no(), PageNumber::FIRST);

        let no_match = edit_filters(Some(&account_text(card(&cards, 1)?)), None)?;
        let empty = run(&ninth, &no_match, &cards);
        assert!(empty.rows().is_empty());
        assert_eq!(empty.page_no().get(), 0);
        assert_eq!(empty.message(), Some("NO MORE RECORDS TO SHOW"));

        let from_zero = plan(Some(Direction::Backward), empty.state(), 7);
        assert_eq!(
            from_zero.read(),
            Read::Backward {
                start: Some(CardRef::new(1))
            }
        );
        let back = run(&from_zero, &none, &cards);
        assert_eq!(back.page_no(), PageNumber::FIRST);
        assert_eq!(back.message(), Some("NO PREVIOUS PAGES TO DISPLAY"));
        Ok(())
    }

    #[test]
    fn page_numbers_convert_only_within_one_digit() {
        for value in 0..=9_i64 {
            assert_eq!(PageNumber::try_from(value).map(i64::from), Ok(value));
        }
        for value in [-1_i64, 10, i64::MAX, i64::MIN] {
            assert_eq!(PageNumber::try_from(value), Err(ErrorKind::OutOfRange));
        }
        assert_eq!(PageNumber::new(10), None);
        assert_eq!(PageNumber::default(), PageNumber::FIRST);
    }

    // audit: COCRDLIC-032
    #[test]
    fn rows_carry_the_handle_and_only_the_redacted_card_number() -> TestResult {
        let cards = fixture()?;
        for (card_ref, record) in &cards {
            let row = CardListRow::new(*card_ref, record);
            assert!(
                row == CardListRow::from((*card_ref, record.clone())),
                "COCRDLIC-032"
            );
            assert_eq!(row.card_ref, *card_ref);
            assert_eq!(row.acct_id, record.acct_id);
            assert_eq!(row.status.as_bytes(), b"Y");
            let number = card_text(record)?;
            let redacted = format!("************{}", number.get(12..).ok_or("16 digits")?);
            assert_eq!(row.card_num.redacted(), redacted);
            for text in [format!("{row:?}"), format!("{}", row.card_num)] {
                assert!(
                    !text.contains(&number),
                    "COCRDLIC-032: a full card number leaked"
                );
                assert!(text.contains(&redacted));
            }
        }
        Ok(())
    }

    // audit: COCRDLIC-033
    // audit: COCRDLIC-034
    #[test]
    fn the_messages_are_the_program_literals() {
        let texts = [
            (
                ListMessage::NoRecordsFound,
                "NO RECORDS FOUND FOR THIS SEARCH CONDITION.",
            ),
            (ListMessage::NoMoreRecords, "NO MORE RECORDS TO SHOW"),
            (ListMessage::NoPreviousPages, "NO PREVIOUS PAGES TO DISPLAY"),
            (ListMessage::NoMorePages, "NO MORE PAGES TO DISPLAY"),
        ];
        for (message, text) in texts {
            assert_eq!(message.text(), text);
        }
        assert_eq!(ListMessage::FileError.text().len(), 75);
        assert!(INFO.is_some_and(|info| info.len() <= 45));
    }

    // audit: COCRDLIC-014
    #[test]
    fn the_selection_error_reread_starts_at_the_first_card_and_keeps_the_page() -> TestResult {
        let cards = fixture()?;
        let none = CardFilters::default();
        let reread = plan_reread(None, state(3, 15, 22)?, 7);
        assert_eq!(reread.read(), Read::Forward { start: None });
        let page = run(&reread, &none, &cards);
        assert_eq!(handles(&page), span(1, 7));
        assert_eq!(page.page_no().get(), 3);
        assert_eq!(page.last_key(), Some(CardRef::new(8)));
        assert_eq!(page.message(), None);
        assert_eq!(page.info_message(), INFO);

        // The screen's selection message stays where the read would say no more records.
        let one = edit_filters(Some(&account_text(card(&cards, 9)?)), None)?;
        let short = run(&plan_reread(None, state(3, 15, 22)?, 7), &one, &cards);
        assert_eq!(handles(&short), vec![9]);
        assert_eq!(short.next_page(), NextPage::NotExists);
        assert_eq!(short.message(), None);

        let crossed = edit_filters(
            Some(&account_text(card(&cards, 1)?)),
            Some(&card_text(card(&cards, 2)?)?),
        )?;
        let first = ListState::fresh();
        let empty = run(&plan_reread(None, first, 7), &crossed, &cards);
        assert_eq!(empty.message_kind(), Some(ListMessage::NoRecordsFound));
        let back = run(
            &plan_reread(Some(Direction::Backward), first, 7),
            &none,
            &cards,
        );
        assert_eq!(back.message_kind(), Some(ListMessage::NoPreviousPages));
        let shown = ListState {
            last_page: LastPage::Shown,
            ..ListState::fresh()
        };
        let forward = run(
            &plan_reread(Some(Direction::Forward), shown, 7),
            &one,
            &cards,
        );
        assert_eq!(forward.message_kind(), Some(ListMessage::NoMorePages));
        Ok(())
    }

    #[test]
    fn plans_read_no_more_than_one_page_and_one_row() -> TestResult {
        let cards = fixture()?;
        let none = CardFilters::default();
        let zero = plan(None, ListState::fresh(), 0);
        assert_eq!((zero.page_size(), zero.limit()), (1, 1));
        let back = plan(Some(Direction::Backward), state(2, 8, 15)?, 100);
        assert_eq!((back.page_size(), back.limit()), (100, 101));
        assert!(back.lookahead_after(&[]).is_none());

        let three = plan(None, ListState::fresh(), 3);
        let rows: Vec<CardListRow> = cards
            .iter()
            .take(3)
            .map(|(card_ref, record)| CardListRow::new(*card_ref, record))
            .collect();
        let after = three.lookahead_after(&rows).map(|row| row.card_ref);
        assert_eq!(after, Some(CardRef::new(3)));
        assert!(
            three
                .lookahead_after(rows.get(..2).ok_or("two rows")?)
                .is_none()
        );
        let small = run(&three, &none, &cards);
        assert_eq!(handles(&small), span(1, 3));
        assert_eq!(small.last_key(), Some(CardRef::new(4)));

        // Rows beyond the page are not listed.
        let extra: Vec<CardListRow> = cards
            .iter()
            .map(|(card_ref, record)| CardListRow::new(*card_ref, record))
            .collect();
        assert_eq!(handles(&page(&three, extra, None)), span(1, 3));

        let all = run(&plan(None, ListState::fresh(), 100), &none, &cards);
        assert_eq!(handles(&all), span(1, 50));
        assert_eq!(all.next_page(), NextPage::NotExists);
        assert_eq!(all.into_rows().len(), 50);
        Ok(())
    }
}
