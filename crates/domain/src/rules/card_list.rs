//! `COCRDLIC`, transaction `CCLI`: the credit card list behind
//! `GET /cards?account_id=&card_number=&start_key=&direction=&page_size=`.
//!
//! The module ports the filter edits `2200-EDIT-INPUTS`, `2210-EDIT-ACCOUNT`
//! and `2220-EDIT-CARD`, the browses `9000-READ-FORWARD` and
//! `9100-READ-BACKWARDS` with their record filter `9500-FILTER-RECORDS`, and
//! the page texts `1400-SETUP-MESSAGE` moves last
//! [app/cbl/COCRDLIC.cbl:L895-1411].
//!
//! # The request and the COMMAREA page state
//!
//! The COBOL keeps its page state in the COMMAREA: the first and last card of
//! the page shown, the screen number and the next-page flag (P-COCRDLIC-014).
//! A request carries none of it, so the state is read from the keys (E-54):
//!
//! - A forward request is ENTER or PF8. The browse starts at `start_key`,
//!   inclusively (`STARTBR GTEQ`); without one it starts at the lowest card
//!   number, as the blank first-card key of a first entry does
//!   (P-CVACT02Y-012). PF8 sends [`CardListPage::next_key`] back.
//! - A backward request is PF7 from the page whose first card is `start_key`,
//!   the [`CardListPage::prev_key`] of that page. Without one it is PF7 on the
//!   first entry.
//! - A page is the first page (`CA-FIRST-PAGE`) when no card that passes the
//!   filters lies before it.
//!
//! The filter edits run before either browse, in both directions, and a
//! failing filter skips the browse (P-COCRDLIC-021, P-COCRDLIC-023).
//!
//! The row selectors `S` and `U` have no request parameter: the list's row
//! actions open the card view and edit routes instead (E-54). Their edits in
//! `2250-EDIT-ARRAY` (V-COCRDLIC-003, V-COCRDLIC-004) are therefore not
//! carried, nor are the paragraphs no path reaches (E-67).

use std::fmt;

use crate::entities::Card;
use crate::error::DomainError;
use crate::messages::{
    ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT, CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT,
    NO_MORE_PAGES_TO_DISPLAY_COCRDLIC, NO_MORE_RECORDS_TO_SHOW,
    NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC, NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC,
    file_error_message,
};
use crate::numeric::{AccountId, CardNumber};
use crate::ports::{Browse, CardFilter, CardReader, Direction, PortError, PortResult};

/// `WS-MAX-SCREEN-LINES PIC S9(4) COMP VALUE 7` [app/cbl/COCRDLIC.cbl:L177-178]:
/// the card rows of the `COCRDLI` map (`CRDSEL1` to `CRDSEL7`), and the page
/// size of a request that names none — P-COCRDLIC-003.
pub const SCREEN_ROWS: u16 = 7;

/// The BMS field of the account filter. The cursor stays there whenever both
/// filters pass (`IF INPUT-OK MOVE -1 TO ACCTSIDL`), so every row other than
/// the card filter's names it — V-COCRDLIC-001, V-COCRDLIC-005 to
/// V-COCRDLIC-010.
const ACCOUNT_FIELD: &str = "ACCTSID";

/// The BMS field of the card filter — V-COCRDLIC-002.
const CARD_FIELD: &str = "CARDSID";

/// `CC-ACCT-ID PIC X(11)`, the receiver of the account filter
/// [app/cpy/CVCRD01Y.cpy:L34-36].
const ACCOUNT_FILTER_LEN: usize = 11;

/// `CC-CARD-NUM PIC X(16)`, the receiver of the card filter
/// [app/cpy/CVCRD01Y.cpy:L37-39].
const CARD_FILTER_LEN: usize = 16;

/// `LIT-CARD-FILE` [app/cbl/COCRDLIC.cbl:L213-214], the `ERROR-FILE` of every
/// failed card read.
const CARD_FILE: &str = "CARDDAT ";

/// The `ERROR-OPNAME` of every failed `READNEXT` and `READPREV`
/// [app/cbl/COCRDLIC.cbl:L1226] [app/cbl/COCRDLIC.cbl:L1250]
/// [app/cbl/COCRDLIC.cbl:L1312] [app/cbl/COCRDLIC.cbl:L1365].
const READ_OPERATION: &str = "READ";

/// The `RESP` and `RESP2` of a CICS `NOTFND`, which E-44 carries into the
/// file-error text for a read that finds no record.
const NOTFND_RESP: (u32, u32) = (13, 80);

/// The `RESP` and `RESP2` E-44 (open) proposes for a store failure, which has
/// no CICS counterpart: `IOERR`, RESP 17 and RESP2 0.
const IOERR_RESP: (u32, u32) = (17, 0);

/// The rows the look-ahead and first-page reads ask for: the key a read
/// starts at, which the browse includes, and the one card beyond it.
const KEY_AND_NEXT: u16 = 2;

/// The most cards a page holds. A browse limit is a `u16`, and the backward
/// read asks for [`KEY_AND_NEXT`] rows beyond the page.
const MAX_PAGE_ROWS: u16 = u16::MAX - KEY_AND_NEXT;

/// One `GET /cards` request: the filters as entered and the page asked for.
///
/// `Debug` shows the card filter masked as [`CardNumber`] masks it.
#[derive(Clone, PartialEq, Eq)]
pub struct CardListInput {
    /// The account filter as entered (`ACCTSIDI`, moved to `CC-ACCT-ID`).
    /// `None`, empty or blank text is no filter.
    pub account_filter: Option<String>,
    /// The card filter as entered (`CARDSIDI`, moved to `CC-CARD-NUM`).
    /// `None`, empty or blank text is no filter.
    pub card_filter: Option<String>,
    /// For a forward request the card the page starts at, for a backward one
    /// the first card of the page shown; `None` for a first entry.
    pub start_key: Option<CardNumber>,
    /// Forward (ENTER or PF8) or backward (PF7).
    pub direction: Direction,
    /// The most cards on the page, [`SCREEN_ROWS`] on the 3270 screen. Zero is
    /// read as one, and anything above `u16::MAX - 2` as that limit.
    pub page_size: u16,
}

impl fmt::Debug for CardListInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let masked = self.card_filter.as_deref().map(CardNumber::move_from);
        formatter
            .debug_struct("CardListInput")
            .field("account_filter", &self.account_filter)
            .field("card_filter", &masked)
            .field("start_key", &self.start_key)
            .field("direction", &self.direction)
            .field("page_size", &self.page_size)
            .finish()
    }
}

/// One page of the card list, as `1000-SEND-MAP` shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardListPage {
    /// The cards shown, in ascending card number, the `CARDDAT` key order:
    /// the rows `ACCTNOnO`, `CRDNUMnO` and `CRDSTSnO` take their account,
    /// number and active status from (P-COCRDLIC-017).
    pub items: Vec<Card>,
    /// The `start_key` of the forward request for the next page, PF8: the
    /// card the look-ahead read found after a full page. `None` when the
    /// browse reached the end of the file first (`CA-NEXT-PAGE-NOT-EXISTS`).
    pub next_key: Option<CardNumber>,
    /// The `start_key` of the backward request for the previous page, PF7:
    /// the first card shown, or the start key of a page that shows none.
    pub prev_key: Option<CardNumber>,
    /// The message line (`ERRMSGO`), when the page shows one. None of these
    /// texts sets the error flag, so the page is a success.
    pub message: Option<&'static str>,
}

/// A filter as `2210-EDIT-ACCOUNT` and `2220-EDIT-CARD` classify the text
/// their `X(n)` receiver holds after the alphanumeric `MOVE` of the entry.
enum FilterText<'a> {
    /// Not supplied: the receiver is `LOW-VALUES`, `SPACES` or, read through
    /// its `9(n)` redefinition, zero.
    Blank,
    /// `n` digits that are not all zero: the filter applies.
    Number(&'a str),
    /// Anything else: the `IS NOT NUMERIC` branch.
    NotNumeric,
}

/// The class of the filter `raw` for a receiver of `width` bytes.
///
/// The `MOVE` pads the entry with spaces, so only text of exactly `width`
/// digits is numeric, and only text of exactly `width` NUL bytes is
/// `LOW-VALUES`. The all-zero test of the `9(n)` redefinition is unpinned for
/// text holding a non-digit, an input class E-47 holds at `halt`; such text is
/// classified as E-47 decides, the all-zero test false, so the `NUMERIC` test
/// rejects it (V-COCRDLIC-001 records that outcome for `"0000000005A"`).
///
/// Text longer than the receiver has no 3270 counterpart, since the map field
/// holds `width` characters and the API rejects longer values before the
/// rule. It is rejected rather than cut, so no entered digit is dropped
/// unseen.
fn classify(raw: Option<&str>, width: usize) -> FilterText<'_> {
    let text = raw.unwrap_or_default();
    let bytes = text.as_bytes();
    if bytes.len() > width {
        return FilterText::NotNumeric;
    }
    let full = bytes.len() == width;
    if bytes.iter().all(|&byte| byte == b' ') || (full && bytes.iter().all(|&byte| byte == 0)) {
        FilterText::Blank
    } else if full && bytes.iter().all(u8::is_ascii_digit) {
        if bytes.iter().all(|&byte| byte == b'0') {
            FilterText::Blank
        } else {
            FilterText::Number(text)
        }
    } else {
        FilterText::NotNumeric
    }
}

/// The account filter's rejection: V-COCRDLIC-001.
fn account_filter_error() -> DomainError {
    DomainError::invalid(ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT).with_field(ACCOUNT_FIELD)
}

/// The card filter's rejection: V-COCRDLIC-002.
fn card_filter_error() -> DomainError {
    DomainError::invalid(CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT).with_field(CARD_FIELD)
}

/// COCRDLIC 2210-EDIT-ACCOUNT — P-COCRDLIC-005, P-COCRDLIC-022, V-COCRDLIC-001.
///
/// The account the list is filtered on, or `None` for no filter: an absent,
/// empty or blank entry, or one of eleven zeros
/// [app/cbl/COCRDLIC.cbl:L1007-1013]. Eleven digits that are not all zero
/// filter on that account [app/cbl/COCRDLIC.cbl:L1026-1029]; `9500-FILTER-RECORDS`
/// then keeps a card when its `CARD-ACCT-ID` equals them.
///
/// # Errors
///
/// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with
/// 'ACCOUNT FILTER,IF SUPPLIED MUST BE A 11 DIGIT NUMBER' on `ACCTSID` for any
/// other entry [app/cbl/COCRDLIC.cbl:L1017-1025].
pub fn edit_account_filter(raw: Option<&str>) -> Result<Option<AccountId>, DomainError> {
    match classify(raw, ACCOUNT_FILTER_LEN) {
        FilterText::Blank => Ok(None),
        // Eleven digits always fit `9(11)`; the mapping only keeps the
        // conversion free of a panic.
        FilterText::Number(digits) => digits.parse().map(Some).map_err(|_| account_filter_error()),
        FilterText::NotNumeric => Err(account_filter_error()),
    }
}

/// COCRDLIC 2220-EDIT-CARD — P-COCRDLIC-006, P-COCRDLIC-022, V-COCRDLIC-002.
///
/// The card number the list is filtered on, or `None` for no filter: an
/// absent, empty or blank entry, or one of sixteen zeros
/// [app/cbl/COCRDLIC.cbl:L1042-1048]. Sixteen digits that are not all zero
/// filter on that number [app/cbl/COCRDLIC.cbl:L1063-1066]. `9500-FILTER-RECORDS`
/// compares the `X(16)` `CARD-NUM` with the unsigned `9(16)` `CC-CARD-NUM-N`,
/// which COBOL compares as its sixteen digit characters, so the filter is the
/// key holding those digits.
///
/// # Errors
///
/// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with
/// 'CARD ID FILTER,IF SUPPLIED MUST BE A 16 DIGIT NUMBER' on `CARDSID` for any
/// other entry [app/cbl/COCRDLIC.cbl:L1052-1062].
pub fn edit_card_filter(raw: Option<&str>) -> Result<Option<CardNumber>, DomainError> {
    match classify(raw, CARD_FILTER_LEN) {
        FilterText::Blank => Ok(None),
        FilterText::Number(digits) => Ok(Some(CardNumber::move_from(digits))),
        FilterText::NotNumeric => Err(card_filter_error()),
    }
}

/// COCRDLIC 2200-EDIT-INPUTS — P-COCRDLIC-019, P-COCRDLIC-021, P-COCRDLIC-023,
/// V-COCRDLIC-001, V-COCRDLIC-002.
///
/// Both edits always run, account first. `2220-EDIT-CARD` moves its text only
/// `IF WS-ERROR-MSG-OFF` [app/cbl/COCRDLIC.cbl:L1056-1060], so when both
/// filters fail the account text is the message (Order 1). The card filter's
/// field is still listed after the account's, with its own text, because
/// `1300-SETUP-SCREEN-ATTRS` marks each failing filter red (P-COCRDLIC-019).
fn edit_inputs(account: Option<&str>, card: Option<&str>) -> Result<CardFilter, DomainError> {
    match (edit_account_filter(account), edit_card_filter(card)) {
        (Ok(account_id), Ok(card_number)) => Ok(CardFilter {
            account_id,
            card_number,
        }),
        (Err(error), Ok(_)) | (Ok(_), Err(error)) => Err(error),
        (Err(account), Err(card)) => Err(account.with_field_message(CARD_FIELD, card.message)),
    }
}

/// COCRDLIC 0000-MAIN, 9000-READ-FORWARD, 9100-READ-BACKWARDS and
/// 1400-SETUP-MESSAGE — P-COCRDLIC-003, P-COCRDLIC-004, P-COCRDLIC-011,
/// P-COCRDLIC-012, P-COCRDLIC-013, P-COCRDLIC-021, V-COCRDLIC-001,
/// V-COCRDLIC-002, V-COCRDLIC-005 to V-COCRDLIC-010.
///
/// Edits both filters, then reads one page of `CARDDAT` in card-number order:
/// forward from `start_key` inclusively for ENTER and PF8, or backward from
/// the first card of the page shown for PF7. Cards the filters exclude are
/// read past and not counted. The page carries the keys of its neighbours and
/// the text the message line shows; an empty page is a page, not an error.
///
/// # Errors
///
/// - [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) with the
///   account or card filter text when a filter is neither blank nor a number
///   of its width, before any read (V-COCRDLIC-001, V-COCRDLIC-002).
/// - [`ErrorKind::Unexpected`](crate::error::ErrorKind::Unexpected) with the
///   `WS-FILE-ERROR-MESSAGE` text on `ACCTSID` when a card read fails
///   (V-COCRDLIC-008 forward, V-COCRDLIC-010 backward).
pub async fn list<S: CardReader>(
    store: &mut S,
    input: CardListInput,
) -> Result<CardListPage, DomainError> {
    let CardListInput {
        account_filter,
        card_filter,
        start_key,
        direction,
        page_size,
    } = input;
    let filter = edit_inputs(account_filter.as_deref(), card_filter.as_deref())?;
    let rows = page_size.clamp(1, MAX_PAGE_ROWS);
    match (direction, start_key) {
        (Direction::Forward, start) => read_forward(store, start, &filter, rows).await,
        (Direction::Backward, Some(first)) => read_backward(store, first, &filter, rows).await,
        // A first entry leaves the first-card key blank and the screen on the
        // first page, so PF7 re-reads from the lowest card number.
        (Direction::Backward, None) => Ok(first_page_again(store, None, &filter, rows).await),
    }
}

/// The cards one forward read shows, and the key the next page starts at.
struct ForwardRead {
    /// Up to a page of the cards that pass the filters, in key order.
    items: Vec<Card>,
    /// The look-ahead card after a full page, if the file holds one.
    next_key: Option<CardNumber>,
}

/// COCRDLIC 9000-READ-FORWARD and 9500-FILTER-RECORDS — P-COCRDLIC-003,
/// P-COCRDLIC-004, F-COCRDLI-004.
///
/// `STARTBR GTEQ` at `start` and `READNEXT` until `rows` cards have passed
/// the filters or the file ends [app/cbl/COCRDLIC.cbl:L1129-1195]. A full page
/// is followed by the look-ahead read.
async fn fetch_forward<S: CardReader>(
    store: &mut S,
    start: Option<CardNumber>,
    filter: &CardFilter,
    rows: u16,
) -> PortResult<ForwardRead> {
    let browse = Browse {
        start_key: start,
        direction: Direction::Forward,
        limit: rows,
    };
    let mut items = store.browse_cards(browse, filter.clone()).await?;
    items.truncate(usize::from(rows));
    let last_of_full_page = items
        .last()
        .filter(|_| items.len() == usize::from(rows))
        .map(|card| card.card_num);
    let next_key = match last_of_full_page {
        Some(last) => look_ahead(store, last).await?,
        None => None,
    };
    Ok(ForwardRead { items, next_key })
}

/// COCRDLIC 9000-READ-FORWARD look-ahead — P-COCRDLIC-003, F-COCRDLI-004,
/// V-COCRDLIC-007.
///
/// Once the page is full, one more `READNEXT` that `9500-FILTER-RECORDS` does
/// not see: a card after `last` sets `CA-NEXT-PAGE-EXISTS` and becomes the
/// next page's start key, whether or not it passes the filters; the end of
/// the file leaves none [app/cbl/COCRDLIC.cbl:L1191-1221].
async fn look_ahead<S: CardReader>(
    store: &mut S,
    last: CardNumber,
) -> PortResult<Option<CardNumber>> {
    let browse = Browse {
        start_key: Some(last),
        direction: Direction::Forward,
        limit: KEY_AND_NEXT,
    };
    let cards = store.browse_cards(browse, CardFilter::default()).await?;
    Ok(cards
        .into_iter()
        .map(|card| card.card_num)
        .find(|number| *number > last))
}

/// `CA-FIRST-PAGE` read from the keys (E-54): whether no card that passes
/// `filter` lies before `start`. A page with no start key is the first.
async fn is_first_page<S: CardReader>(
    store: &mut S,
    start: Option<CardNumber>,
    filter: &CardFilter,
) -> PortResult<bool> {
    let Some(start) = start else {
        return Ok(true);
    };
    let browse = Browse {
        start_key: Some(start),
        direction: Direction::Backward,
        limit: KEY_AND_NEXT,
    };
    let cards = store.browse_cards(browse, filter.clone()).await?;
    Ok(cards.iter().all(|card| card.card_num >= start))
}

/// COCRDLIC 9000-READ-FORWARD with 1400-SETUP-MESSAGE, for ENTER and PF8 —
/// P-COCRDLIC-003, P-COCRDLIC-011, P-COCRDLIC-012, V-COCRDLIC-005 to
/// V-COCRDLIC-008.
async fn read_forward<S: CardReader>(
    store: &mut S,
    start: Option<CardNumber>,
    filter: &CardFilter,
    rows: u16,
) -> Result<CardListPage, DomainError> {
    let read = fetch_forward(store, start, filter, rows)
        .await
        .map_err(|error| read_failure(&error))?;
    let message = match (read.next_key, read.items.is_empty()) {
        (Some(_), _) => None,
        // The end of the file after at least one card, in the page loop or
        // in the look-ahead [app/cbl/COCRDLIC.cbl:L1215-1221]
        // [app/cbl/COCRDLIC.cbl:L1233-1240] — V-COCRDLIC-007.
        (None, false) => Some(NO_MORE_RECORDS_TO_SHOW),
        (None, true) => Some(empty_page_text(store, start, filter).await?),
    };
    Ok(page(read.items, read.next_key, start, message))
}

/// The text of a forward page that shows no card.
///
/// On the first page `SET WS-NO-RECORDS-FOUND` replaces the end-of-file text
/// [app/cbl/COCRDLIC.cbl:L1241-1245] (V-COCRDLIC-006). Past the first page a
/// forward request that finds no card starts beyond the last card that
/// passes the filters: PF8 after the last page, whose text
/// `1400-SETUP-MESSAGE` moves last [app/cbl/COCRDLIC.cbl:L905-909]
/// (V-COCRDLIC-005, E-54).
async fn empty_page_text<S: CardReader>(
    store: &mut S,
    start: Option<CardNumber>,
    filter: &CardFilter,
) -> Result<&'static str, DomainError> {
    let first_page = is_first_page(store, start, filter)
        .await
        .map_err(|error| read_failure(&error))?;
    Ok(if first_page {
        NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC
    } else {
        NO_MORE_PAGES_TO_DISPLAY_COCRDLIC
    })
}

/// COCRDLIC 9100-READ-BACKWARDS with 1400-SETUP-MESSAGE, for PF7 —
/// P-COCRDLIC-013, V-COCRDLIC-009, V-COCRDLIC-010.
///
/// `STARTBR GTEQ` at the first card of the page shown and `READPREV`: the
/// first read returns that card and only steps the counter
/// [app/cbl/COCRDLIC.cbl:L1294-1307], the next ones fill the rows from the
/// bottom with the cards that pass the filters [app/cbl/COCRDLIC.cbl:L1320-1359].
/// So the page holds the cards below `first`, shown in ascending order.
///
/// `0000-MAIN` steps the screen number back before the read
/// [app/cbl/COCRDLIC.cbl:L508], so a page with no card before it is the first
/// page again and `1400-SETUP-MESSAGE` shows 'NO PREVIOUS PAGES TO DISPLAY'
/// on it [app/cbl/COCRDLIC.cbl:L901-904]. With no card at all before `first`
/// the request is PF7 on the first page.
async fn read_backward<S: CardReader>(
    store: &mut S,
    first: CardNumber,
    filter: &CardFilter,
    rows: u16,
) -> Result<CardListPage, DomainError> {
    let browse = Browse {
        start_key: Some(first),
        direction: Direction::Backward,
        limit: rows.saturating_add(KEY_AND_NEXT),
    };
    let cards = store
        .browse_cards(browse, filter.clone())
        .await
        .map_err(|error| read_failure(&error))?;
    let mut before: Vec<Card> = cards
        .into_iter()
        .filter(|card| card.card_num < first)
        .collect();
    if before.is_empty() {
        return Ok(first_page_again(store, Some(first), filter, rows).await);
    }
    let reaches_first_page = before.len() <= usize::from(rows);
    before.truncate(usize::from(rows));
    before.reverse();
    let message = reaches_first_page.then_some(NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC);
    // 9100 moves the old first-card key to the last-card key and sets
    // CA-NEXT-PAGE-EXISTS [app/cbl/COCRDLIC.cbl:L1268]
    // [app/cbl/COCRDLIC.cbl:L1287], so PF8 starts again at `first`.
    Ok(page(before, Some(first), None, message))
}

/// COCRDLIC 0000-MAIN `WHEN CCARD-AID-PFK07 AND CA-FIRST-PAGE` with
/// 1400-SETUP-MESSAGE — P-COCRDLIC-011, V-COCRDLIC-009, F-COCRDLI-003.
///
/// PF7 on the first page re-reads it forward from its first card, `first`
/// [app/cbl/COCRDLIC.cbl:L439-454], and `1400-SETUP-MESSAGE` then moves
/// 'NO PREVIOUS PAGES TO DISPLAY' over whatever text the read left
/// [app/cbl/COCRDLIC.cbl:L901-904].
///
/// That includes the file-error text of a failed `READNEXT`: V-COCRDLIC-009
/// (Order 5) outranks V-COCRDLIC-008 (Order 6), whose trigger is a forward
/// request. A failed re-read therefore still answers with the
/// no-previous-pages text, and with no card, because a port browse returns
/// whole or not at all; the rows a CICS browse had filled before failing have
/// no counterpart.
async fn first_page_again<S: CardReader>(
    store: &mut S,
    first: Option<CardNumber>,
    filter: &CardFilter,
    rows: u16,
) -> CardListPage {
    let message = Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC);
    match fetch_forward(store, first, filter, rows).await {
        Ok(read) => page(read.items, read.next_key, first, message),
        Err(_replaced_by_the_page_text) => page(Vec::new(), None, first, message),
    }
}

/// The page showing `items`; its previous-page key is the first card shown,
/// or `start` when it shows none.
fn page(
    items: Vec<Card>,
    next_key: Option<CardNumber>,
    start: Option<CardNumber>,
    message: Option<&'static str>,
) -> CardListPage {
    let prev_key = items.first().map(|card| card.card_num).or(start);
    CardListPage {
        items,
        next_key,
        prev_key,
        message,
    }
}

/// The text a failed card read leaves — V-COCRDLIC-008, V-COCRDLIC-010.
///
/// `WS-FILE-ERROR-MESSAGE` with `ERROR-OPNAME` 'READ' and `ERROR-FILE`
/// 'CARDDAT ' [app/cbl/COCRDLIC.cbl:L1246-1254]
/// [app/cbl/COCRDLIC.cbl:L1361-1369], with the cursor left on `ACCTSID`.
/// The database produces no CICS `RESP`, so the slots carry the codes of E-44
/// (open): a read that finds no record is `NOTFND`, any other failure the
/// proposed `IOERR`.
fn read_failure(error: &PortError) -> DomainError {
    let (resp, resp2) = match error {
        PortError::NotFound => NOTFND_RESP,
        PortError::Duplicate
        | PortError::Restricted
        | PortError::Contention
        | PortError::Unavailable(_) => IOERR_RESP,
    };
    DomainError::unexpected(file_error_message(READ_OPERATION, CARD_FILE, resp, resp2))
        .with_field(ACCOUNT_FIELD)
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::ops::Range;

    use super::{
        CardListInput, CardListPage, SCREEN_ROWS, edit_account_filter, edit_card_filter, list,
    };
    use crate::entities::Card;
    use crate::error::{DomainError, ErrorKind, FieldError};
    use crate::messages::{
        ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT, CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT,
        NO_MORE_PAGES_TO_DISPLAY_COCRDLIC, NO_MORE_RECORDS_TO_SHOW,
        NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC, NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC,
        file_error_message,
    };
    use crate::numeric::{AccountId, Alnum, CardNumber};
    use crate::ports::{Direction, PortError};
    use crate::rules::test_support::{FakeStore, block_on};

    type TestResult = Result<(), Box<dyn Error>>;

    /// An account filter, a card filter, the cards they keep and the text
    /// shown.
    type FilterCase<'a> = (Option<&'a str>, Option<&'a str>, &'a [&'a str], &'a str);

    /// The 50 `CARD-RECORD`s of the fixture corpus, in key order.
    const CARDDATA: &str = include_str!("../../../../app/data/ASCII/carddata.txt");

    /// Rows 1 to 7 of the unfiltered first page — P-CVACT02Y-012.
    const FIRST_PAGE: [&str; 7] = [
        "0500024453765740",
        "0683586198171516",
        "0923877193247330",
        "0927987108636232",
        "0982496213629795",
        "1014086565224350",
        "1142167692878931",
    ];

    /// The `CARD-ACCT-ID` of rows 1 to 7 of the unfiltered first page —
    /// P-CVACT02Y-012.
    const FIRST_PAGE_ACCOUNTS: [u64; 7] = [50, 27, 2, 20, 12, 44, 37];

    /// The one card of account `00000000050` — P-CVACT02Y-009.
    const ACCOUNT_50: &str = "00000000050";
    const ACCOUNT_50_CARD: &str = "0500024453765740";

    /// A database failure as the adapter reports it.
    fn store_failure() -> PortError {
        PortError::Unavailable("connection reset".to_owned())
    }

    /// One `carddata.txt` record decoded by the `CVACT02Y` offsets.
    fn fixture_card(line: &str) -> Result<Card, Box<dyn Error>> {
        let field = |range: Range<usize>| {
            line.get(range)
                .ok_or("carddata.txt record shorter than CARD-RECORD")
        };
        Ok(Card {
            card_num: CardNumber::from_trimmed(field(0..16)?)?,
            card_acct_id: field(16..27)?.parse()?,
            card_cvv_cd: field(27..30)?.parse()?,
            card_embossed_name: Alnum::move_from(field(30..80)?),
            card_expiraion_date: Alnum::move_from(field(80..90)?),
            card_active_status: Alnum::move_from(field(90..91)?),
        })
    }

    /// `CARDDAT` as the seed loads it from the fixture corpus.
    fn fixture_store() -> Result<FakeStore, Box<dyn Error>> {
        CARDDATA
            .lines()
            .try_fold(FakeStore::default(), |store, line| {
                Ok(store.with_card(fixture_card(line)?))
            })
    }

    /// Every fixture card number in key order.
    fn all_numbers() -> Vec<String> {
        let mut numbers: Vec<String> = CARDDATA
            .lines()
            .filter_map(|line| line.get(0..16))
            .map(str::to_owned)
            .collect();
        numbers.sort();
        numbers
    }

    fn key(text: &str) -> CardNumber {
        CardNumber::move_from(text)
    }

    /// An unfiltered request of [`SCREEN_ROWS`] cards.
    fn request(direction: Direction, start: Option<&str>) -> CardListInput {
        CardListInput {
            account_filter: None,
            card_filter: None,
            start_key: start.map(CardNumber::move_from),
            direction,
            page_size: SCREEN_ROWS,
        }
    }

    /// A first-entry forward request with the filters as entered.
    fn filtered(account: Option<&str>, card: Option<&str>) -> CardListInput {
        CardListInput {
            account_filter: account.map(str::to_owned),
            card_filter: card.map(str::to_owned),
            ..request(Direction::Forward, None)
        }
    }

    fn numbers(page: &CardListPage) -> Vec<String> {
        page.items
            .iter()
            .map(|card| card.card_num.trimmed().into_owned())
            .collect()
    }

    fn run(store: &mut FakeStore, input: CardListInput) -> Result<CardListPage, DomainError> {
        block_on(list(store, input))
    }

    /// P-COCRDLIC-003, P-COCRDLIC-004, P-CVACT02Y-012: with no filter every
    /// card is kept, the first page holds the seven lowest card numbers, and
    /// the unfiltered look-ahead read gives the eighth as the next page's key.
    #[test]
    fn the_first_unfiltered_page_shows_the_seven_lowest_cards() -> TestResult {
        let mut store = fixture_store()?;
        let all = all_numbers();
        let page = run(&mut store, request(Direction::Forward, None))?;
        assert_eq!(numbers(&page), FIRST_PAGE);
        let accounts: Vec<u64> = page
            .items
            .iter()
            .map(|card| card.card_acct_id.get())
            .collect();
        assert_eq!(accounts, FIRST_PAGE_ACCOUNTS);
        assert!(
            page.items
                .iter()
                .all(|card| card.card_active_status.trimmed() == "Y")
        );
        assert_eq!(page.next_key, Some(key(&all[7])));
        assert_eq!(page.prev_key, Some(key(FIRST_PAGE[0])));
        assert_eq!(page.message, None);
        // The page read, then the look-ahead read.
        assert_eq!(store.calls, ["browse_cards", "browse_cards"]);
        Ok(())
    }

    /// The rule's future is `Send` over a `Send` store, as the API's
    /// multi-threaded runtime requires of a handler.
    #[test]
    fn the_list_future_is_send() -> TestResult {
        fn assert_send<T: Send>(future: T) -> T {
            future
        }
        let mut store = fixture_store()?;
        let page = block_on(assert_send(list(
            &mut store,
            request(Direction::Forward, None),
        )))?;
        assert_eq!(numbers(&page), FIRST_PAGE);
        Ok(())
    }

    /// F-COCRDLI-004, P-COCRDLIC-003: PF8 sends the look-ahead card back as
    /// the start key, and the forward read includes it.
    #[test]
    fn pf8_starts_the_next_page_at_the_look_ahead_card() -> TestResult {
        let mut store = fixture_store()?;
        let all = all_numbers();
        let page = run(&mut store, request(Direction::Forward, Some(&all[7])))?;
        assert_eq!(numbers(&page), all[7..14]);
        assert_eq!(page.next_key, Some(key(&all[14])));
        assert_eq!(page.prev_key, Some(key(&all[7])));
        assert_eq!(page.message, None);
        Ok(())
    }

    /// P-COCRDLIC-003, V-COCRDLIC-007: following `next_key` shows every card
    /// once; the 50 cards fill seven pages and one card, and only the last
    /// page shows the end-of-file text.
    #[test]
    fn paging_forward_shows_every_card_once() -> TestResult {
        let mut store = fixture_store()?;
        let mut shown = Vec::new();
        let mut pages = Vec::new();
        let mut start = None;
        for _ in 0..10 {
            let page = run(
                &mut store,
                CardListInput {
                    start_key: start,
                    ..request(Direction::Forward, None)
                },
            )?;
            shown.extend(numbers(&page));
            pages.push((page.items.len(), page.message));
            start = page.next_key;
            if start.is_none() {
                break;
            }
        }
        assert_eq!(shown, all_numbers());
        let mut expected = vec![(7, None); 7];
        expected.push((1, Some(NO_MORE_RECORDS_TO_SHOW)));
        assert_eq!(pages, expected);
        Ok(())
    }

    /// V-COCRDLIC-007: a full page whose look-ahead read reaches the end of
    /// the file has no next page and shows the end-of-file text.
    #[test]
    fn a_full_last_page_whose_look_ahead_ends_the_file_has_no_next_page() -> TestResult {
        let mut store = fixture_store()?;
        let all = all_numbers();
        let input = CardListInput {
            page_size: 10,
            ..request(Direction::Forward, Some(&all[40]))
        };
        let page = run(&mut store, input)?;
        assert_eq!(numbers(&page), all[40..]);
        assert_eq!(page.next_key, None);
        assert_eq!(page.message, Some(NO_MORE_RECORDS_TO_SHOW));
        assert_eq!(store.calls, ["browse_cards", "browse_cards"]);
        Ok(())
    }

    /// P-COCRDLIC-013, V-COCRDLIC-009: PF7 shows the cards before the page's
    /// first card in ascending order, PF8 from there starts again at that
    /// card, and the page reached shows the no-previous-pages text when it is
    /// the first page.
    #[test]
    fn pf7_shows_the_cards_before_the_page() -> TestResult {
        let all = all_numbers();
        let cases: [(usize, Range<usize>, Option<&str>); 2] = [
            (14, 7..14, None),
            (7, 0..7, Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC)),
        ];
        for (first, shown, message) in cases {
            let mut store = fixture_store()?;
            let page = run(&mut store, request(Direction::Backward, Some(&all[first])))?;
            assert_eq!(numbers(&page), all[shown.clone()]);
            assert_eq!(page.next_key, Some(key(&all[first])));
            assert_eq!(page.prev_key, Some(key(&all[shown.start])));
            assert_eq!(page.message, message);
        }
        Ok(())
    }

    /// V-COCRDLIC-009, P-COCRDLIC-011, F-COCRDLI-003: PF7 with no card before
    /// its start key re-reads the first page forward and shows the
    /// no-previous-pages text; without a start key it is PF7 on a first
    /// entry, which reads from the lowest card number and pages back nothing.
    #[test]
    fn pf7_on_the_first_page_shows_it_again() -> TestResult {
        let all = all_numbers();
        for start in [Some(FIRST_PAGE[0]), Some("0000000000000000"), None] {
            let mut store = fixture_store()?;
            let page = run(&mut store, request(Direction::Backward, start))?;
            assert_eq!(numbers(&page), FIRST_PAGE);
            assert_eq!(page.next_key, Some(key(&all[7])));
            assert_eq!(page.prev_key, Some(key(FIRST_PAGE[0])));
            assert_eq!(page.message, Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC));
        }

        let mut store = fixture_store()?;
        run(&mut store, request(Direction::Backward, None))?;
        assert_eq!(store.calls, ["browse_cards", "browse_cards"]);

        let mut store = fixture_store()?;
        let input = CardListInput {
            account_filter: Some(ACCOUNT_50.to_owned()),
            ..request(Direction::Backward, Some(ACCOUNT_50_CARD))
        };
        let page = run(&mut store, input)?;
        assert_eq!(numbers(&page), [ACCOUNT_50_CARD]);
        assert_eq!(page.next_key, None);
        assert_eq!(page.message, Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC));
        Ok(())
    }

    /// P-COCRDLIC-004, V-COCRDLIC-006, V-COCRDLIC-007: `9500-FILTER-RECORDS`
    /// keeps the cards that match every filter given; a first page that keeps
    /// none shows the no-records text instead of the end-of-file text.
    #[test]
    fn the_filters_keep_only_matching_cards() -> TestResult {
        let cases: [FilterCase<'_>; 5] = [
            (
                Some(ACCOUNT_50),
                None,
                &[ACCOUNT_50_CARD],
                NO_MORE_RECORDS_TO_SHOW,
            ),
            (
                None,
                Some(FIRST_PAGE[1]),
                &[FIRST_PAGE[1]],
                NO_MORE_RECORDS_TO_SHOW,
            ),
            (
                Some("00000000027"),
                Some(FIRST_PAGE[1]),
                &[FIRST_PAGE[1]],
                NO_MORE_RECORDS_TO_SHOW,
            ),
            (
                Some(ACCOUNT_50),
                Some(FIRST_PAGE[1]),
                &[],
                NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC,
            ),
            // The missing account of P-CVACT01Y-016.
            (
                Some("99999999999"),
                None,
                &[],
                NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC,
            ),
        ];
        for (account, card, shown, message) in cases {
            let mut store = fixture_store()?;
            let page = run(&mut store, filtered(account, card))?;
            assert_eq!(numbers(&page), shown);
            assert_eq!(page.next_key, None);
            assert_eq!(page.message, Some(message));
        }
        Ok(())
    }

    /// P-COCRDLIC-022, P-COCRDLIC-005, P-COCRDLIC-006: an absent, empty,
    /// blank, `LOW-VALUES` or all-zero filter is no filter.
    #[test]
    fn blank_and_zero_filters_filter_nothing() -> TestResult {
        let account_nuls = "\0".repeat(11);
        let card_nuls = "\0".repeat(16);
        let accounts = [
            None,
            Some(""),
            Some("   "),
            Some("           "),
            Some(account_nuls.as_str()),
            Some("00000000000"),
        ];
        let cards = [
            None,
            Some(""),
            Some(" "),
            Some(card_nuls.as_str()),
            Some("0000000000000000"),
        ];
        for raw in accounts {
            assert_eq!(edit_account_filter(raw), Ok(None), "{raw:?}");
        }
        for raw in cards {
            assert_eq!(edit_card_filter(raw), Ok(None), "{raw:?}");
        }
        let mut store = fixture_store()?;
        let page = run(&mut store, filtered(Some("00000000000"), Some("   ")))?;
        assert_eq!(numbers(&page), FIRST_PAGE);
        Ok(())
    }

    /// P-COCRDLIC-004, P-COCRDLIC-005, P-COCRDLIC-006: a filter of its full
    /// width in digits, not all zero, is the key the cards are matched on.
    #[test]
    fn numeric_filters_are_the_keys_matched_on() -> TestResult {
        assert_eq!(
            edit_account_filter(Some(ACCOUNT_50)),
            Ok(Some(AccountId::new(50)?))
        );
        assert_eq!(
            edit_card_filter(Some(ACCOUNT_50_CARD)),
            Ok(Some(key(ACCOUNT_50_CARD)))
        );
        Ok(())
    }

    /// V-COCRDLIC-001, V-COCRDLIC-002, P-COCRDLIC-021, E-47: a filter that is
    /// neither blank nor all digits of its width is rejected on its field,
    /// before any read and in either direction.
    #[test]
    fn a_filter_that_is_not_a_number_of_its_width_is_rejected() -> TestResult {
        let account =
            DomainError::invalid(ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT).with_field("ACCTSID");
        let card =
            DomainError::invalid(CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT).with_field("CARDSID");
        let cases = [
            // The example of V-COCRDLIC-001.
            (Some("0000000005A"), None, &account),
            (Some("50"), None, &account),
            (Some("000000000050"), None, &account),
            (Some(" 0000000005"), None, &account),
            (Some("-0000000005"), None, &account),
            (Some("0000000000 "), None, &account),
            // The example of V-COCRDLIC-002.
            (None, Some("050002445376574X"), &card),
            (None, Some("123"), &card),
            (None, Some("05000244537657401"), &card),
            (None, Some("000000000000000 "), &card),
        ];
        for (account_filter, card_filter, expected) in cases {
            for direction in [Direction::Forward, Direction::Backward] {
                let mut store = fixture_store()?;
                let input = CardListInput {
                    account_filter: account_filter.map(str::to_owned),
                    card_filter: card_filter.map(str::to_owned),
                    ..request(direction, Some(FIRST_PAGE[1]))
                };
                let Err(error) = run(&mut store, input) else {
                    return Err(format!("{account_filter:?} {card_filter:?} was accepted").into());
                };
                assert_eq!(&error, expected);
                assert_eq!(error.kind, ErrorKind::Invalid);
                assert!(store.calls.is_empty(), "{:?}", store.calls);
            }
        }
        Ok(())
    }

    /// V-COCRDLIC-001 (Order 1), P-COCRDLIC-019, P-COCRDLIC-023: with both
    /// filters bad the account text is the message, and both fields are
    /// marked, each with its own text.
    #[test]
    fn two_bad_filters_keep_the_account_text() -> TestResult {
        let mut store = fixture_store()?;
        let Err(error) = run(
            &mut store,
            filtered(Some("0000000005A"), Some("050002445376574X")),
        ) else {
            return Err("two bad filters were accepted".into());
        };
        assert_eq!(error.kind, ErrorKind::Invalid);
        assert_eq!(error.message, ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT);
        assert_eq!(
            error.fields,
            [
                FieldError {
                    field: "ACCTSID",
                    message: ACCOUNT_FILTER_IF_SUPPLIED_MUST_BE_11_DIGIT.to_owned(),
                },
                FieldError {
                    field: "CARDSID",
                    message: CARD_ID_FILTER_IF_SUPPLIED_MUST_BE_16_DIGIT.to_owned(),
                },
            ]
        );
        assert!(store.calls.is_empty());
        Ok(())
    }

    /// F-COCRDLI-004, V-COCRDLIC-005: the look-ahead read does not see the
    /// filters, so a full page's next key can be a card they exclude, and the
    /// page from it shows no card and the no-more-pages text.
    #[test]
    fn an_unfiltered_look_ahead_can_open_an_empty_page() -> TestResult {
        let mut store = fixture_store()?;
        let one_row = CardListInput {
            page_size: 1,
            ..filtered(Some(ACCOUNT_50), None)
        };
        let page = run(&mut store, one_row.clone())?;
        assert_eq!(numbers(&page), [ACCOUNT_50_CARD]);
        assert_eq!(page.next_key, Some(key(FIRST_PAGE[1])));
        assert_eq!(page.message, None);

        let next = run(
            &mut store,
            CardListInput {
                start_key: page.next_key,
                ..one_row
            },
        )?;
        assert!(next.items.is_empty());
        assert_eq!(next.next_key, None);
        assert_eq!(next.prev_key, Some(key(FIRST_PAGE[1])));
        assert_eq!(next.message, Some(NO_MORE_PAGES_TO_DISPLAY_COCRDLIC));
        Ok(())
    }

    /// V-COCRDLIC-005, V-COCRDLIC-006: a forward page that shows no card
    /// shows the no-more-pages text when a card passing the filters lies
    /// before its start key, and the no-records text when none does.
    #[test]
    fn an_empty_forward_page_is_told_apart_by_the_cards_before_it() -> TestResult {
        let cases = [
            // The example of V-COCRDLIC-005: past the last card of the account.
            (
                Some(ACCOUNT_50),
                "0500024453765741",
                NO_MORE_PAGES_TO_DISPLAY_COCRDLIC,
            ),
            (
                Some("99999999999"),
                "0000000000000000",
                NO_RECORDS_FOUND_FOR_THIS_SEARCH_CONDITION_COCRDLIC,
            ),
            // Past the last card of the file (P-CVACT02Y-010).
            (None, "9999999999999999", NO_MORE_PAGES_TO_DISPLAY_COCRDLIC),
        ];
        for (account, start, message) in cases {
            let mut store = fixture_store()?;
            let input = CardListInput {
                start_key: Some(key(start)),
                ..filtered(account, None)
            };
            let page = run(&mut store, input)?;
            assert!(page.items.is_empty(), "{start}");
            assert_eq!(page.next_key, None);
            assert_eq!(page.prev_key, Some(key(start)));
            assert_eq!(page.message, Some(message), "{start}");
        }
        Ok(())
    }

    /// V-COCRDLIC-008, V-COCRDLIC-010, E-44: a failed card read in either
    /// direction answers the file-error text on `ACCTSID`.
    #[test]
    fn a_failed_card_read_answers_the_file_error_text() -> TestResult {
        let cases = [(store_failure(), 17, 0), (PortError::NotFound, 13, 80)];
        for (failure, resp, resp2) in cases {
            let expected =
                DomainError::unexpected(file_error_message("READ", "CARDDAT ", resp, resp2))
                    .with_field("ACCTSID");
            for (direction, start) in [
                (Direction::Forward, None),
                (Direction::Backward, Some(FIRST_PAGE[1])),
            ] {
                let mut store = fixture_store()?;
                store.fail_next("browse_cards", failure.clone());
                let Err(error) = run(&mut store, request(direction, start)) else {
                    return Err(format!("{direction:?} read failure was answered").into());
                };
                assert_eq!(error, expected);
                assert_eq!(error.kind, ErrorKind::Unexpected);
            }
        }
        Ok(())
    }

    /// V-COCRDLIC-009 over V-COCRDLIC-008: the no-previous-pages text
    /// replaces the text of a failed re-read of the first page.
    #[test]
    fn a_failed_reread_of_the_first_page_keeps_the_no_previous_pages_text() -> TestResult {
        let mut store = fixture_store()?;
        store.fail_next("browse_cards", store_failure());
        let page = run(&mut store, request(Direction::Backward, None))?;
        assert_eq!(
            page,
            CardListPage {
                items: Vec::new(),
                next_key: None,
                prev_key: None,
                message: Some(NO_PREVIOUS_PAGES_TO_DISPLAY_COCRDLIC),
            }
        );
        Ok(())
    }

    /// P-COCRDLIC-003: a page holds `page_size` cards, [`SCREEN_ROWS`] on the
    /// 3270 screen; zero is read as one, and a page larger than the file
    /// shows every card with the end-of-file text.
    #[test]
    fn the_page_size_bounds_the_page() -> TestResult {
        let all = all_numbers();
        let cases = [
            (0, 1, Some(key(&all[1])), None),
            (1, 1, Some(key(&all[1])), None),
            (SCREEN_ROWS, 7, Some(key(&all[7])), None),
            (100, 50, None, Some(NO_MORE_RECORDS_TO_SHOW)),
        ];
        for (page_size, shown, next_key, message) in cases {
            let mut store = fixture_store()?;
            let input = CardListInput {
                page_size,
                ..request(Direction::Forward, None)
            };
            let page = run(&mut store, input)?;
            assert_eq!(numbers(&page), all[..shown], "{page_size}");
            assert_eq!(page.next_key, next_key, "{page_size}");
            assert_eq!(page.message, message, "{page_size}");
        }
        Ok(())
    }

    /// `Debug` masks the card filter as entered, whatever its length, and the
    /// start key to their last four characters, and shows the account filter
    /// as entered.
    #[test]
    fn debug_masks_the_card_filter_and_the_start_key() {
        let cases = [
            // The fixture card number, the receiver's full width.
            (ACCOUNT_50_CARD, "************5740"),
            // Longer than the receiver: masked as the `MOVE` cuts it.
            ("05000244537657401", "************5740"),
            // Shorter than the receiver.
            ("12345678", "****5678"),
        ];
        for (card_filter, masked) in cases {
            let input = CardListInput {
                start_key: Some(key(FIRST_PAGE[1])),
                ..filtered(Some(ACCOUNT_50), Some(card_filter))
            };
            let shown = format!("{input:?}");
            assert!(shown.contains(masked), "{shown}");
            assert!(shown.contains("************1516"), "{shown}");
            assert!(shown.contains(ACCOUNT_50), "{shown}");
            for full in [card_filter, ACCOUNT_50_CARD, FIRST_PAGE[1]] {
                assert!(!shown.contains(full), "{shown}");
            }
        }
    }
}
