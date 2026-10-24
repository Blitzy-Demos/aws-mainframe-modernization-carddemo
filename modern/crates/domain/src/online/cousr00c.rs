//! COUSR00C, the user list: the rules of endpoint 16, `GET /api/v1/users` \[A\] (transaction
//! `CU00`, map `COUSR0A`; AAP 0.3.4.1).
//!
//! This module ports the `server` rows of `modern/audit/online/COUSR00C.md` that a pure rule
//! decides: the paging state carried between interactions (COUSR00C-006), the read key of ENTER
//! (COUSR00C-012), the forward and backward pages with their messages (COUSR00C-013 to
//! COUSR00C-024), the two browses the audit halts (COUSR00C-025, COUSR00C-027), the store failure
//! (COUSR00C-026) and the four fields a row shows (COUSR00C-028). The admin gate of COUSR00C-001
//! is the `AdminUser` extractor of `api::auth::extract`: the search value and the paging state
//! are request data and grant nothing (AAP 0.5.1), and the list is never filtered by the caller.
//! The `client` rows, the transfers of F3 and of the `U` and `D` selections and the
//! invalid-selection message among them (COUSR00C-002 to COUSR00C-005, COUSR00C-007 to
//! COUSR00C-011, COUSR00C-029 to COUSR00C-033), send no request and belong to the screen.
//!
//! # Flow
//!
//! `api::services::cousr00c` calls [`plan`] with the request's [`Action`] and the
//! [`PagingState`] the request carries back. A [`Plan::Respond`] is the whole answer: F7 on the
//! first page and F8 with no next page read nothing (COUSR00C-020, COUSR00C-021). A
//! [`Plan::Read`] names one browse of the `users` table by primary key: the service reads at
//! most [`PageRead::fetch_limit`] users from [`PageRead::key`], ascending for
//! [`Direction::Forward`] and descending for [`Direction::Backward`], the key itself included,
//! and hands them unchanged, in the order read, to [`page`]. A failing read is
//! [`UserListFailure::LookupFailed`]. The service reads only the user ID, first name, last name
//! and user type, never the password hash (COUSR00C-028, Cardholder Data Redaction), and this
//! module does no I/O.
//!
//! # The browse
//!
//! COUSR00C reads `USRSEC` with `STARTBR` at the first user ID at or above the key (`GTEQ`, the
//! CICS default), then `READNEXT` or `READPREV`. Each step maps onto the rows read:
//!
//! - ENTER reads from the key with no discard (COUSR00C-013). F8 and F7 first discard the
//!   record `STARTBR` positioned at, which is the key's own user when it still exists
//!   (COUSR00C-017, COUSR00C-022). Forward, that record is the first row read, whatever its
//!   key. Backward, the rows read are the users at or below the key, so the discarded record is
//!   among them only when it is the key's own user; otherwise it is the key's successor, which a
//!   backward read never returns.
//! - Up to `page_size` users fill the page, then one more is read ahead: forward it decides the
//!   next-page indicator (COUSR00C-014), backward the page number (COUSR00C-023). A read that
//!   ends first is the end of the file, `ENDFILE`, with its message (COUSR00C-015,
//!   COUSR00C-018, COUSR00C-024).
//! - So a browse needs at most `page_size + 1` rows after the discarded record, and
//!   [`PageRead::fetch_limit`] adds that one record only to the F7 and F8 reads that discard it
//!   (Bounded Pagination). The default page is the map's ten rows, [`DEFAULT_PAGE_SIZE`]; the
//!   API validates `page_size` (1 to 100, 422 otherwise) before it reaches [`plan`].
//! - The first and last user IDs change only when the first and last row positions are filled,
//!   and otherwise keep the values the request carried (COUSR00C-016, X-COUSR00C-5).
//!
//! User IDs are compared as their 8 bytes, the order of the `COLLATE "C"` primary key
//! (X-COUSR00C-2).
//!
//! # Halts
//!
//! - `HALTED(X-COUSR00C-H1)`: a forward read that finds no user at or above its key is the
//!   `STARTBR` `NOTFND` of COUSR00C-025, after which the COBOL abends. [`page`] answers with the
//!   message the program sends first and keeps the screen's rows; that answer is no parity
//!   result. A backward read cannot tell that case from the one in which only the discarded
//!   successor lies at or above the key, and reads it as the latter.
//! - `HALTED(X-COUSR00C-H2)`: F8 with a next page and a blank last user ID starts the browse at
//!   `HIGH-VALUES` (COUSR00C-027). [`plan`] answers [`UserListFailure::LookupFailed`] without a
//!   read, the outcome of the `NOTFND` that IBM documents for that `READNEXT`; it is no parity
//!   result either.
//!
//! # Totality
//!
//! [`start_user_id`] takes any text; [`plan`] and [`page`] take any state, rows and page size
//! and return a value or a [`UserListFailure`]. Nothing indexes, slices, unwraps or panics
//! (Total Parsers Over Untrusted Input).

use core::fmt;

use copybook::{PicU, PicX};

use crate::records::{UserListEntry, UserSecurityRecord};

/// The rows of one page when the request names no `page_size`: the ten row groups of map
/// `COUSR0A`, filled by the loops to `WS-IDX > 10` and `WS-IDX >= 11` (COUSR00C-013).
pub const DEFAULT_PAGE_SIZE: usize = 10;

/// The read key below every user ID: COBOL `LOW-VALUES` in `SEC-USR-ID PIC X(08)`
/// (COUSR00C-012, COUSR00C-022).
///
/// Held as eight spaces: a stored user ID holds printable bytes only, and the space is the
/// lowest of them, so every user ID is at or above this key exactly as it is above
/// `LOW-VALUES`. The `CHAR(8)` comparison ignores trailing spaces, so the browse reads it as the
/// empty key.
pub const LOW_KEY: PicX<8> = PicX::spaces();

/// The `LOW-VALUES` byte.
const LOW_VALUE: u8 = 0;

/// Page 1, to which a backward page that reaches the first user resets the page number
/// (COUSR00C-023).
const FIRST_PAGE: PicU<8> = match PicU::new(1) {
    Ok(page) => page,
    Err(_) => PicU::ZERO,
};

/// `PROCESS-PF7-KEY` on page 1 or less (`COUSR00C.cbl:251`, COUSR00C-021).
const ALREADY_AT_TOP: &str = "You are already at the top of the page...";

/// `PROCESS-PF8-KEY` with no next page (`COUSR00C.cbl:273`, COUSR00C-020).
const ALREADY_AT_BOTTOM: &str = "You are already at the bottom of the page...";

/// `READNEXT` `ENDFILE` (`COUSR00C.cbl:637`, COUSR00C-014, COUSR00C-015, COUSR00C-018).
const REACHED_BOTTOM: &str = "You have reached the bottom of the page...";

/// `READPREV` `ENDFILE` (`COUSR00C.cbl:671`, COUSR00C-023, COUSR00C-024).
const REACHED_TOP: &str = "You have reached the top of the page...";

/// `STARTBR` `NOTFND` (`COUSR00C.cbl:603`, COUSR00C-025, `HALTED(X-COUSR00C-H1)`).
const AT_TOP: &str = "You are at the top of the page...";

/// A browse command answered other than as handled (`COUSR00C.cbl:610,644,678`,
/// COUSR00C-026).
const LOOKUP_FAILED: &str = "Unable to lookup User...";

/// One row of the list: the four `SEC-USER-DATA` fields `POPULATE-USER-DATA` moves to the map,
/// as stored, padding kept (COUSR00C-028).
///
/// The type has no password field: `SEC-USR-PWD` is read with every COBOL record and never
/// moved to the map, and the replacement never reads the hash (CSUSR01Y-004, Cardholder Data
/// Redaction). User IDs and names are not protected values, so the derived `Debug` shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserListRow {
    /// `SEC-USR-ID PIC X(08)` → `USRIDnnI PIC X(8)` (CSUSR01Y-001).
    pub user_id: PicX<8>,
    /// `SEC-USR-FNAME PIC X(20)` → `FNAMEnnI PIC X(20)` (CSUSR01Y-002).
    pub first_name: PicX<20>,
    /// `SEC-USR-LNAME PIC X(20)` → `LNAMEnnI PIC X(20)` (CSUSR01Y-003).
    pub last_name: PicX<20>,
    /// `SEC-USR-TYPE PIC X(01)` → `UTYPEnnI PIC X(1)` (CSUSR01Y-005).
    pub user_type: PicX<1>,
}

/// The row of a user read from the store: the four columns the page query selects, moved as
/// stored (COUSR00C-028).
#[must_use]
pub const fn row_from(
    user_id: PicX<8>,
    first_name: PicX<20>,
    last_name: PicX<20>,
    user_type: PicX<1>,
) -> UserListRow {
    UserListRow {
        user_id,
        first_name,
        last_name,
        user_type,
    }
}

impl From<&UserSecurityRecord> for UserListRow {
    /// The row `POPULATE-USER-DATA` shows for a `SEC-USER-DATA` record: every field but
    /// `SEC-USR-PWD` and `SEC-USR-FILLER`, which never reach the map (COUSR00C-028).
    fn from(record: &UserSecurityRecord) -> Self {
        row_from(
            record.id.clone(),
            record.fname.clone(),
            record.lname.clone(),
            record.usr_type.clone(),
        )
    }
}

impl From<UserListEntry> for UserListRow {
    /// The list row the COUSR00C page rule shows for a user a page query read: its four fields,
    /// as stored (COUSR00C-028).
    fn from(entry: UserListEntry) -> Self {
        row_from(entry.id, entry.fname, entry.lname, entry.usr_type)
    }
}

/// The direction of a page key: F8 pages forward, F7 backward (COUSR00C-017, COUSR00C-022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// F8, `PROCESS-PF8-KEY`: the users after the last user ID shown, ascending.
    Forward,
    /// F7, `PROCESS-PF7-KEY`: the users before the first user ID shown, read descending.
    Backward,
}

/// `CDEMO-CU00-NEXT-PAGE-FLG PIC X(01)`: whether a user follows the page shown (COUSR00C-006,
/// COUSR00C-014).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NextPage {
    /// `'Y'`, 88-level `NEXT-PAGE-YES`.
    Yes,
    /// `'N'`, 88-level `NEXT-PAGE-NO`, and every other value of the flag, which F8 treats the
    /// same way.
    #[default]
    No,
}

impl NextPage {
    /// Whether the indicator is `'Y'`.
    #[must_use]
    pub const fn is_yes(self) -> bool {
        matches!(self, Self::Yes)
    }
}

/// `CDEMO-CU00-INFO`, the paging state COUSR00C keeps in the COMMAREA between interactions
/// (`COUSR00C.cbl:67-75`, COUSR00C-006).
///
/// Every answer returns it, and an F7 or F8 request carries it back as the answer left it. It
/// is request data and grants nothing (AAP 0.5.1). [`Default`] is the state of a first entry
/// from the admin menu: both user IDs blank, page 0 and no next page (COUSR00C-003).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PagingState {
    /// `CDEMO-CU00-USRID-FIRST PIC X(08)`: the user ID of the first row position when it was
    /// last filled, the key of F7.
    pub first_key: PicX<8>,
    /// `CDEMO-CU00-USRID-LAST PIC X(08)`: the user ID of the last row position when it was last
    /// filled, the key of F8.
    pub last_key: PicX<8>,
    /// `CDEMO-CU00-PAGE-NUM PIC 9(08)`, shown as eight digits with leading zeros
    /// (COUSR00C-029).
    pub page_no: PicU<8>,
    /// `CDEMO-CU00-NEXT-PAGE-FLG PIC X(01)`.
    pub next_page: NextPage,
}

/// The request a user list interaction makes of endpoint 16 (COUSR00C-012, COUSR00C-017,
/// COUSR00C-022).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// ENTER without a transfer, and the first entry of the screen, which COUSR00C processes as
    /// ENTER: list from `start`, the read key [`start_user_id`] makes of the search field.
    Enter {
        /// The read key: [`LOW_KEY`] for a blank search field.
        start: PicX<8>,
    },
    /// F8 ([`Direction::Forward`]) or F7 ([`Direction::Backward`]) from the paging state.
    Page(Direction),
}

/// Whether a browse discards the record `STARTBR` positioned at before it fills the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Discard {
    /// ENTER: the first record read is row 1 (COUSR00C-013).
    No,
    /// F8 and F7: one `READNEXT` or `READPREV` discards it (COUSR00C-017, COUSR00C-022).
    StartRecord,
}

/// One browse of the `users` table that [`plan`] asks the service to read, and that [`page`]
/// turns into the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRead {
    direction: Direction,
    key: PicX<8>,
    discard: Discard,
    page_size: usize,
    state: PagingState,
}

impl PageRead {
    /// The order of the read: ascending user IDs from the key for [`Direction::Forward`],
    /// descending from the key for [`Direction::Backward`].
    #[must_use]
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    /// The read key, itself included in the read: users at or above it forward, at or below it
    /// backward. [`LOW_KEY`] reads from the first user (COUSR00C-012, COUSR00C-022).
    #[must_use]
    pub const fn key(&self) -> &PicX<8> {
        &self.key
    }

    /// The rows a page shows (COUSR00C-013).
    #[must_use]
    pub const fn page_size(&self) -> usize {
        self.page_size
    }

    /// The most users the service reads: the page, the one read ahead, and for F8 and F7 the
    /// record the browse discards first (Bounded Pagination, COUSR00C-014, COUSR00C-017,
    /// COUSR00C-022, COUSR00C-023). [`page`] ignores rows beyond it.
    #[must_use]
    pub const fn fetch_limit(&self) -> usize {
        let page_and_read_ahead = self.page_size.saturating_add(1);
        match self.discard {
            Discard::No => page_and_read_ahead,
            Discard::StartRecord => page_and_read_ahead.saturating_add(1),
        }
    }
}

/// What an interaction needs after [`plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Read the users the [`PageRead`] names, then call [`page`].
    Read(PageRead),
    /// The answer, with no read of `users` (COUSR00C-020, COUSR00C-021).
    Respond(UserPage),
}

/// The rows an answer shows (COUSR00C-013, COUSR00C-024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PageRows {
    /// A page was read: the screen's `page_size` row positions, first to last, each a user or
    /// `None` for a blank row. Forward pages fill from the first position and backward pages
    /// from the last, so a short backward page leaves its first positions blank.
    Replace(Vec<Option<UserListRow>>),
    /// No page was read: the screen keeps the rows it shows, as the map is sent without
    /// `ERASE` (COUSR00C-020, COUSR00C-021).
    Keep,
}

impl PageRows {
    /// The filled rows, first to last; none for [`PageRows::Keep`].
    pub fn filled(&self) -> impl Iterator<Item = &UserListRow> {
        let positions: &[Option<UserListRow>] = match self {
            Self::Replace(positions) => positions,
            Self::Keep => &[],
        };
        positions.iter().flatten()
    }
}

/// The message an answer carries: the last literal the interaction moved to `WS-MESSAGE`
/// (COUSR00C-014 to COUSR00C-024; X-COUSR00C-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListMessage {
    /// F7 on page 1 or less (COUSR00C-021).
    AlreadyAtTop,
    /// F8 with no next page (COUSR00C-020).
    AlreadyAtBottom,
    /// `READPREV` reached the first user (COUSR00C-023, COUSR00C-024).
    ReachedTop,
    /// `READNEXT` reached the last user (COUSR00C-014, COUSR00C-015, COUSR00C-018).
    ReachedBottom,
    /// `STARTBR` found no user at or above the key (COUSR00C-025, `HALTED(X-COUSR00C-H1)`).
    AtTop,
}

impl ListMessage {
    /// The COBOL literal, as the response carries it; each fits the 78-byte message row.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::AlreadyAtTop => ALREADY_AT_TOP,
            Self::AlreadyAtBottom => ALREADY_AT_BOTTOM,
            Self::ReachedTop => REACHED_TOP,
            Self::ReachedBottom => REACHED_BOTTOM,
            Self::AtTop => AT_TOP,
        }
    }
}

/// The answer to one interaction, status 200 whether or not it carries a message
/// (COUSR00C-006, COUSR00C-014 to COUSR00C-024).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserPage {
    /// The rows to show.
    pub rows: PageRows,
    /// The paging state after the interaction: first and last user IDs, page number and
    /// next-page indicator, which the next F7 or F8 request carries back (COUSR00C-006).
    pub state: PagingState,
    /// The message row, or `None` for none.
    pub message: Option<ListMessage>,
}

impl UserPage {
    /// The message literal, or `None`.
    #[must_use]
    pub fn message_text(&self) -> Option<&'static str> {
        self.message.map(ListMessage::text)
    }
}

/// The status class of a [`UserListFailure`], which the API turns into its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FailureClass {
    /// The store could not be read: 500 (COUSR00C-026).
    Store,
}

impl FailureClass {
    /// The HTTP status the audit assigns to the class: 500 for [`FailureClass::Store`].
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Store => 500,
        }
    }
}

/// An interaction that ends in a `ScreenError` instead of a page (COUSR00C-026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UserListFailure {
    /// A browse answered other than as handled; in the replacement, a failing read of `users`
    /// or a read that breaks the [`PageRead`] order. The response codes the COBOL displays go
    /// to the region log only, so the message carries none.
    LookupFailed,
}

impl UserListFailure {
    /// The literal moved to `WS-MESSAGE`: `Unable to lookup User...`.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::LookupFailed => LOOKUP_FAILED,
        }
    }

    /// The status class.
    #[must_use]
    pub const fn class(self) -> FailureClass {
        match self {
            Self::LookupFailed => FailureClass::Store,
        }
    }
}

impl fmt::Display for UserListFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for UserListFailure {}

/// The read key of ENTER from the raw search field `USRIDIN` (COUSR00C-012).
///
/// The text is moved into `USRIDINI PIC X(8)`: its first 8 bytes, space-padded. A field of
/// spaces or of `LOW-VALUES` reads from the first user, [`LOW_KEY`]; any other value is the key
/// as typed. There is no edit, no case translation and no existence check: the value only
/// positions the list, and it is data, never identity (AAP 0.5.1). Total for every text; the
/// request DTO admits at most the map's 8 characters (COUSR00-015).
#[must_use]
pub fn start_user_id(raw: &str) -> PicX<8> {
    let field = PicX::moved_from(raw.as_bytes());
    if is_blank(&field) { LOW_KEY } else { field }
}

/// Whether a key field is `SPACES` or `LOW-VALUES`, the test of `COUSR00C.cbl:218,239,262`.
fn is_blank(field: &PicX<8>) -> bool {
    field.is_spaces() || field.as_bytes().iter().all(|&byte| byte == LOW_VALUE)
}

/// What an interaction does before it reads `users`, from its [`Action`], the [`PagingState`]
/// the request carries and the page size the API validated.
///
/// - ENTER restarts the page number at 0 and reads forward from `start`, discarding nothing
///   (COUSR00C-012, COUSR00C-013).
/// - F7 sets the next-page indicator to `Y` whatever follows (X-COUSR00C-3). Above page 1 it
///   reads backward from the first user ID, [`LOW_KEY`] when that is blank, discarding the
///   record at the key (COUSR00C-022); on page 1 or less it answers
///   [`ListMessage::AlreadyAtTop`] and keeps the rows (COUSR00C-021).
/// - F8 with the indicator `N` answers [`ListMessage::AlreadyAtBottom`] with the state
///   unchanged (COUSR00C-020); with `Y` it reads forward from the last user ID, discarding the
///   record at the key (COUSR00C-017).
///
/// # Errors
///
/// [`UserListFailure::LookupFailed`] for F8 with the indicator `Y` and a blank last user ID,
/// the `HIGH-VALUES` browse of COUSR00C-027 (`HALTED(X-COUSR00C-H2)`): no parity result.
pub fn plan(action: Action, state: PagingState, page_size: usize) -> Result<Plan, UserListFailure> {
    match action {
        Action::Enter { start } => Ok(Plan::Read(PageRead {
            direction: Direction::Forward,
            key: start,
            discard: Discard::No,
            page_size,
            state: PagingState {
                page_no: PicU::ZERO,
                ..state
            },
        })),
        Action::Page(Direction::Backward) => Ok(plan_backward(state, page_size)),
        Action::Page(Direction::Forward) => plan_forward(state, page_size),
    }
}

/// `PROCESS-PF7-KEY` (`COUSR00C.cbl:237-255`; COUSR00C-021, COUSR00C-022).
fn plan_backward(state: PagingState, page_size: usize) -> Plan {
    let key = if is_blank(&state.first_key) {
        LOW_KEY
    } else {
        state.first_key.clone()
    };
    let state = PagingState {
        next_page: NextPage::Yes,
        ..state
    };
    if state.page_no > FIRST_PAGE {
        Plan::Read(PageRead {
            direction: Direction::Backward,
            key,
            discard: Discard::StartRecord,
            page_size,
            state,
        })
    } else {
        Plan::Respond(kept_rows(state, ListMessage::AlreadyAtTop))
    }
}

/// `PROCESS-PF8-KEY` (`COUSR00C.cbl:260-277`; COUSR00C-017, COUSR00C-020, COUSR00C-027).
fn plan_forward(state: PagingState, page_size: usize) -> Result<Plan, UserListFailure> {
    match state.next_page {
        NextPage::No => Ok(Plan::Respond(kept_rows(
            state,
            ListMessage::AlreadyAtBottom,
        ))),
        // HALTED(X-COUSR00C-H2): the browse would start at HIGH-VALUES, which no stored user ID
        // reaches; the replacement answers the NOTFND the audit cites, without a read.
        NextPage::Yes if is_blank(&state.last_key) => Err(UserListFailure::LookupFailed),
        NextPage::Yes => Ok(Plan::Read(PageRead {
            direction: Direction::Forward,
            key: state.last_key.clone(),
            discard: Discard::StartRecord,
            page_size,
            state,
        })),
    }
}

/// An answer that reads no page: the screen keeps its rows (COUSR00C-020, COUSR00C-021).
fn kept_rows(state: PagingState, message: ListMessage) -> UserPage {
    UserPage {
        rows: PageRows::Keep,
        state,
        message: Some(message),
    }
}

/// The answer to a [`PageRead`] from the users the service read: at most
/// [`PageRead::fetch_limit`] of them, from [`PageRead::key`] in [`PageRead::direction`], key
/// included, in the order read. Rows beyond the limit are ignored.
///
/// Forward (`PROCESS-PAGE-FORWARD`, COUSR00C-013 to COUSR00C-018): F8 discards the first row;
/// the page fills from the first position. A full page adds 1 to the page number and reads one
/// user ahead: indicator `Y` and no message when it exists, `N` and
/// [`ListMessage::ReachedBottom`] when it does not. A page that ends first is `N` with that
/// message, and the page number goes up only when a row was filled.
///
/// Backward (`PROCESS-PAGE-BACKWARD`, COUSR00C-022 to COUSR00C-024): the first row is
/// discarded when it is the key's own user; the page fills from the last position upward. A
/// full page reads one user ahead: the page number goes down by 1 when it exists above page 1,
/// and is 1 otherwise, with [`ListMessage::ReachedTop`] when no user is left. A page that ends
/// first carries that message and keeps its page number.
///
/// The first and last user IDs follow COUSR00C-016. A forward read with no row is the halted
/// `STARTBR` `NOTFND` (COUSR00C-025, `HALTED(X-COUSR00C-H1)`).
///
/// # Errors
///
/// [`UserListFailure::LookupFailed`] when the rows break the read's order: a row on the wrong
/// side of the key, or rows not strictly ascending forward or descending backward. The store
/// answered other than the browse allows, as COUSR00C-026 treats any unhandled response.
pub fn page(read: PageRead, rows: Vec<UserListRow>) -> Result<UserPage, UserListFailure> {
    let limit = read.fetch_limit();
    let read_rows = rows.get(..limit).unwrap_or(&rows);
    if !in_read_order(read.direction, &read.key, read_rows) {
        return Err(UserListFailure::LookupFailed);
    }
    let rows = rows.into_iter().take(limit);
    Ok(match read.direction {
        Direction::Forward => page_forward(read, rows),
        Direction::Backward => page_backward(read, rows),
    })
}

/// Whether `rows` are in the order a read from `key` in `direction` returns them: the first at
/// or beyond the key, each strictly beyond the one before.
fn in_read_order(direction: Direction, key: &PicX<8>, rows: &[UserListRow]) -> bool {
    let beyond = |before: &PicX<8>, after: &PicX<8>| match direction {
        Direction::Forward => before < after,
        Direction::Backward => before > after,
    };
    let from_key = rows
        .first()
        .is_none_or(|row| row.user_id == *key || beyond(key, &row.user_id));
    from_key
        && rows
            .iter()
            .zip(rows.iter().skip(1))
            .all(|(before, after)| beyond(&before.user_id, &after.user_id))
}

/// `PROCESS-PAGE-FORWARD` over the rows of a forward read (COUSR00C-013 to COUSR00C-018).
fn page_forward(read: PageRead, rows: impl Iterator<Item = UserListRow>) -> UserPage {
    let PageRead {
        discard,
        page_size,
        state,
        ..
    } = read;
    let mut rows = rows.peekable();
    if rows.peek().is_none() {
        return start_not_found(state);
    }
    if discard == Discard::StartRecord {
        // The READNEXT that F8 issues before the page discards the record at the key, or the
        // next one when the key's own user is gone (X-COUSR00C-5).
        rows.next();
    }
    let mut positions = vec![None; page_size];
    let end_of_file = fill(positions.iter_mut(), &mut rows);
    let PagingState {
        first_key,
        last_key,
        page_no,
        ..
    } = state;
    let (page_no, next_page, message) = if end_of_file {
        let page_no = if positions.first().is_some_and(Option::is_some) {
            next_page_no(page_no)
        } else {
            page_no
        };
        (page_no, NextPage::No, Some(ListMessage::ReachedBottom))
    } else if rows.next().is_some() {
        (next_page_no(page_no), NextPage::Yes, None)
    } else {
        (
            next_page_no(page_no),
            NextPage::No,
            Some(ListMessage::ReachedBottom),
        )
    };
    finish(positions, first_key, last_key, page_no, next_page, message)
}

/// `PROCESS-PAGE-BACKWARD` over the rows of a backward read (COUSR00C-022 to COUSR00C-024).
fn page_backward(read: PageRead, rows: impl Iterator<Item = UserListRow>) -> UserPage {
    let PageRead {
        key,
        page_size,
        state,
        ..
    } = read;
    let mut rows = rows.peekable();
    // The READPREV that F7 issues before the page discards the record STARTBR positioned at.
    // A backward read returns it only when it is the key's own user; when that user is gone it
    // is the key's successor, outside the read, and every row read belongs to the page.
    let _discarded = rows.next_if(|row| row.user_id == key);
    let mut positions = vec![None; page_size];
    let end_of_file = fill(positions.iter_mut().rev(), &mut rows);
    let PagingState {
        first_key,
        last_key,
        page_no,
        next_page,
    } = state;
    // NEXT-PAGE-YES always holds here: F7 sets it before every backward read.
    let (page_no, message) = if end_of_file {
        (page_no, Some(ListMessage::ReachedTop))
    } else if rows.next().is_none() {
        (FIRST_PAGE, Some(ListMessage::ReachedTop))
    } else {
        (previous_page_no(page_no), None)
    };
    finish(positions, first_key, last_key, page_no, next_page, message)
}

/// Fills `positions` in order from `rows`, as the read loops of `PROCESS-PAGE-FORWARD` and
/// `PROCESS-PAGE-BACKWARD` do, and returns whether the rows ended first: the `ENDFILE` that
/// stops the loop before every position is filled.
fn fill<'p>(
    positions: impl Iterator<Item = &'p mut Option<UserListRow>>,
    rows: &mut impl Iterator<Item = UserListRow>,
) -> bool {
    for position in positions {
        let Some(row) = rows.next() else {
            return true;
        };
        *position = Some(row);
    }
    false
}

/// The answer to a page read: the positions read, and the first and last user IDs of the first
/// and last positions where those are filled, else as carried (COUSR00C-016, X-COUSR00C-5).
fn finish(
    positions: Vec<Option<UserListRow>>,
    first_key: PicX<8>,
    last_key: PicX<8>,
    page_no: PicU<8>,
    next_page: NextPage,
    message: Option<ListMessage>,
) -> UserPage {
    let first_key = filled_key(positions.first(), first_key);
    let last_key = filled_key(positions.last(), last_key);
    UserPage {
        rows: PageRows::Replace(positions),
        state: PagingState {
            first_key,
            last_key,
            page_no,
            next_page,
        },
        message,
    }
}

/// The user ID of `position` when it is filled, else `carried`.
fn filled_key(position: Option<&Option<UserListRow>>, carried: PicX<8>) -> PicX<8> {
    position
        .and_then(Option::as_ref)
        .map_or(carried, |row| row.user_id.clone())
}

/// The answer when no user lies at or above a forward read's key: `STARTBR` `NOTFND`, which
/// moves [`ListMessage::AtTop`] and sends the map as it stands before the COBOL abends at
/// `ENDBR` (COUSR00C-025).
///
/// `HALTED(X-COUSR00C-H1)`: the rows are kept and the state is the one the read started from;
/// no parity result.
fn start_not_found(state: PagingState) -> UserPage {
    kept_rows(state, ListMessage::AtTop)
}

/// `COMPUTE CDEMO-CU00-PAGE-NUM = CDEMO-CU00-PAGE-NUM + 1` (COUSR00C-014, COUSR00C-015).
fn next_page_no(page_no: PicU<8>) -> PicU<8> {
    // TRUNCATION: the sum is stored into PIC 9(08) with no ON SIZE ERROR, so the 100000000 that
    // follows 99999999 keeps its low eight digits, 00000000.
    page_no.succ().unwrap_or(PicU::ZERO)
}

/// After a full backward page with a user before it: `SUBTRACT 1 FROM CDEMO-CU00-PAGE-NUM`
/// above page 1, else `MOVE 1` (COUSR00C-023). A backward read starts only above page 1, so
/// the result is at least 1 either way.
fn previous_page_no(page_no: PicU<8>) -> PicU<8> {
    page_no
        .get()
        .checked_sub(1)
        .and_then(|previous| PicU::new(previous).ok())
        .filter(|previous| *previous >= FIRST_PAGE)
        .unwrap_or(FIRST_PAGE)
}

#[cfg(test)]
mod tests {
    use copybook::{PicU, PicX};

    use super::{
        Action, DEFAULT_PAGE_SIZE, Direction, FailureClass, LOW_KEY, ListMessage, NextPage,
        PageRows, PagingState, Plan, UserListFailure, UserListRow, UserPage, page, plan, row_from,
        start_user_id,
    };
    use crate::records::{UserListEntry, UserSecurityRecord};
    use crate::sensitive::Password;

    type TestResult = Result<(), String>;

    /// The ten in-stream users of `DUSRSECJ.jcl:35-44` (FIXUSR-001 to FIXUSR-010), in key
    /// order: user ID, first name, last name, type.
    const FIXTURE: [(&str, &str, &str, &str); 10] = [
        ("ADMIN001", "MARGARET", "GOLD", "A"),
        ("ADMIN002", "RUSSELL", "RUSSELL", "A"),
        ("ADMIN003", "RAYMOND", "WHITMORE", "A"),
        ("ADMIN004", "EMMANUEL", "CASGRAIN", "A"),
        ("ADMIN005", "GRANVILLE", "LACHAPELLE", "A"),
        ("USER0001", "LAWRENCE", "THOMAS", "U"),
        ("USER0002", "AJITH", "KUMAR", "U"),
        ("USER0003", "LAURITZ", "ALME", "U"),
        ("USER0004", "AVERARDO", "MAZZI", "U"),
        ("USER0005", "LEE", "TING", "U"),
    ];

    fn key(text: &str) -> PicX<8> {
        PicX::moved_from(text.as_bytes())
    }

    fn page_no(value: u64) -> Result<PicU<8>, String> {
        PicU::new(value).map_err(|error| format!("{error:?}"))
    }

    fn row(id: &str, first: &str, last: &str, user_type: &str) -> UserListRow {
        row_from(
            key(id),
            PicX::moved_from(first.as_bytes()),
            PicX::moved_from(last.as_bytes()),
            PicX::moved_from(user_type.as_bytes()),
        )
    }

    fn fixture() -> Vec<UserListRow> {
        FIXTURE
            .iter()
            .map(|&(id, first, last, user_type)| row(id, first, last, user_type))
            .collect()
    }

    /// `count` users `U0000001`, `U0000002`, …, in key order.
    fn numbered(count: u32) -> Vec<UserListRow> {
        (1..=count)
            .map(|n| row(&format!("U{n:07}"), "FIRST", "LAST", "U"))
            .collect()
    }

    /// The rows the page queries return from `users`: at or above the key ascending, or at or
    /// below it descending, at most `limit`.
    fn read(
        users: &[UserListRow],
        direction: Direction,
        at: &PicX<8>,
        limit: usize,
    ) -> Vec<UserListRow> {
        let matching: Vec<&UserListRow> = match direction {
            Direction::Forward => users.iter().filter(|u| u.user_id >= *at).collect(),
            Direction::Backward => users.iter().rev().filter(|u| u.user_id <= *at).collect(),
        };
        matching.into_iter().take(limit).cloned().collect()
    }

    /// One interaction against `users`, as the service runs it.
    fn run(
        action: Action,
        state: PagingState,
        users: &[UserListRow],
    ) -> Result<UserPage, UserListFailure> {
        match plan(action, state, DEFAULT_PAGE_SIZE)? {
            Plan::Respond(answer) => Ok(answer),
            Plan::Read(browse) => {
                let rows = read(
                    users,
                    browse.direction(),
                    browse.key(),
                    browse.fetch_limit(),
                );
                page(browse, rows)
            }
        }
    }

    fn enter(search: &str) -> Action {
        Action::Enter {
            start: start_user_id(search),
        }
    }

    /// The user ID of every row position, `""` for a blank row; none when the rows are kept.
    fn positions(answer: &UserPage) -> Vec<String> {
        let PageRows::Replace(rows) = &answer.rows else {
            return Vec::new();
        };
        rows.iter()
            .map(|position| {
                position
                    .as_ref()
                    .map(|r| r.user_id.to_text())
                    .unwrap_or_default()
            })
            .collect()
    }

    fn expected(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|&id| id.to_owned()).collect()
    }

    // audit: COUSR00C-012
    #[test]
    fn the_search_field_is_moved_as_typed_and_blank_reads_from_the_start() {
        for blank in ["", " ", "        ", "\0\0\0\0\0\0\0\0", "          "] {
            assert_eq!(start_user_id(blank), LOW_KEY, "{blank:?}");
        }
        assert_eq!(start_user_id("USER0001"), key("USER0001"));
        assert_eq!(start_user_id("user").as_bytes(), b"user    ");
        assert_eq!(start_user_id(" ADM").as_bytes(), b" ADM    ");
        assert_eq!(start_user_id("ADMIN0019").as_bytes(), b"ADMIN001");
        assert_eq!(start_user_id("\0A").as_bytes(), b"\0A      ");
        assert_eq!(start_user_id("ÉÉÉÉÉ").as_bytes().len(), 8);
        assert_eq!(LOW_KEY.as_bytes(), b"        ");
    }

    // audit: COUSR00C-006, COUSR00C-012, COUSR00C-013, COUSR00C-014, COUSR00C-016
    // audit: COUSR00C-028
    #[test]
    fn enter_with_a_blank_search_lists_the_ten_fixture_users() -> TestResult {
        let Plan::Read(browse) = plan(enter(""), PagingState::default(), DEFAULT_PAGE_SIZE)
            .map_err(|failure| failure.to_string())?
        else {
            return Err("ENTER reads a page".to_owned());
        };
        assert_eq!(browse.direction(), Direction::Forward);
        assert_eq!(browse.key(), &LOW_KEY);
        assert_eq!(browse.page_size(), 10);
        assert_eq!(browse.fetch_limit(), 11);

        let answer =
            run(enter(""), PagingState::default(), &fixture()).map_err(|f| f.to_string())?;
        let ids: Vec<&str> = FIXTURE.iter().map(|&(id, ..)| id).collect();
        assert_eq!(positions(&answer), expected(&ids));
        assert_eq!(answer.rows.filled().cloned().collect::<Vec<_>>(), fixture());
        assert_eq!(answer.state.page_no, page_no(1)?);
        assert_eq!(answer.state.next_page, NextPage::No);
        assert_eq!(answer.state.first_key, key("ADMIN001"));
        assert_eq!(answer.state.last_key, key("USER0005"));
        assert_eq!(answer.message, Some(ListMessage::ReachedBottom));
        assert_eq!(
            answer.message_text(),
            Some("You have reached the bottom of the page...")
        );
        let first = answer.rows.filled().next().ok_or("a first row")?;
        assert_eq!(first.first_name.as_bytes(), b"MARGARET            ");
        assert_eq!(first.last_name.as_bytes(), b"GOLD                ");
        assert_eq!(first.user_type.as_bytes(), b"A");
        Ok(())
    }

    // audit: COUSR00C-012, COUSR00C-013, COUSR00C-015, COUSR00C-016
    #[test]
    fn enter_with_a_search_value_restarts_paging_from_that_key() -> TestResult {
        let users = fixture();
        let first = run(enter(""), PagingState::default(), &users).map_err(|f| f.to_string())?;
        let carried = PagingState {
            page_no: page_no(7)?,
            next_page: NextPage::Yes,
            ..first.state
        };
        let answer = run(enter("USER0001"), carried, &users).map_err(|f| f.to_string())?;
        assert_eq!(
            positions(&answer),
            expected(&[
                "USER0001", "USER0002", "USER0003", "USER0004", "USER0005", "", "", "", "", ""
            ])
        );
        assert_eq!(answer.state.page_no, page_no(1)?);
        assert_eq!(answer.state.next_page, NextPage::No);
        assert_eq!(answer.state.first_key, key("USER0001"));
        assert_eq!(answer.state.last_key, key("USER0005"));
        assert_eq!(answer.message, Some(ListMessage::ReachedBottom));

        // A key between user IDs positions at the next one; no existence check.
        let between =
            run(enter("ADMIN00A"), PagingState::default(), &users).map_err(|f| f.to_string())?;
        assert_eq!(
            between
                .rows
                .filled()
                .next()
                .map(|r| r.user_id.to_text())
                .as_deref(),
            Some("USER0001")
        );
        Ok(())
    }

    // audit: COUSR00C-014, COUSR00C-015, COUSR00C-016, COUSR00C-017
    #[test]
    fn f8_pages_forward_past_the_last_user_shown() -> TestResult {
        let users = numbered(25);
        let one = run(enter(""), PagingState::default(), &users).map_err(|f| f.to_string())?;
        assert_eq!(one.state.page_no, page_no(1)?);
        assert_eq!(one.state.next_page, NextPage::Yes);
        assert_eq!(one.message, None);
        assert_eq!(one.state.last_key, key("U0000010"));

        let Plan::Read(browse) = plan(Action::Page(Direction::Forward), one.state.clone(), 10)
            .map_err(|f| f.to_string())?
        else {
            return Err("F8 with a next page reads".to_owned());
        };
        assert_eq!(browse.key(), &key("U0000010"));
        assert_eq!(browse.fetch_limit(), 12);

        let two =
            run(Action::Page(Direction::Forward), one.state, &users).map_err(|f| f.to_string())?;
        let ids: Vec<String> = (11..=20).map(|n| format!("U{n:07}")).collect();
        assert_eq!(positions(&two), ids);
        assert_eq!(two.state.page_no, page_no(2)?);
        assert_eq!(two.state.next_page, NextPage::Yes);
        assert_eq!(two.state.first_key, key("U0000011"));
        assert_eq!(two.state.last_key, key("U0000020"));
        assert_eq!(two.message, None);

        let three =
            run(Action::Page(Direction::Forward), two.state, &users).map_err(|f| f.to_string())?;
        assert_eq!(
            positions(&three),
            expected(&[
                "U0000021", "U0000022", "U0000023", "U0000024", "U0000025", "", "", "", "", ""
            ])
        );
        assert_eq!(three.state.page_no, page_no(3)?);
        assert_eq!(three.state.next_page, NextPage::No);
        assert_eq!(three.state.first_key, key("U0000021"));
        assert_eq!(three.state.last_key, key("U0000020"));
        assert_eq!(three.message, Some(ListMessage::ReachedBottom));
        Ok(())
    }

    // audit: COUSR00C-014
    #[test]
    fn a_full_last_page_reports_the_bottom_after_the_read_ahead() -> TestResult {
        let users = numbered(20);
        let one = run(enter(""), PagingState::default(), &users).map_err(|f| f.to_string())?;
        let two =
            run(Action::Page(Direction::Forward), one.state, &users).map_err(|f| f.to_string())?;
        assert_eq!(two.rows.filled().count(), 10);
        assert_eq!(two.state.page_no, page_no(2)?);
        assert_eq!(two.state.next_page, NextPage::No);
        assert_eq!(two.message, Some(ListMessage::ReachedBottom));
        Ok(())
    }

    // audit: COUSR00C-017
    #[test]
    fn f8_discards_the_next_user_when_the_last_one_shown_is_gone() -> TestResult {
        let mut users = numbered(15);
        let one = run(enter(""), PagingState::default(), &users).map_err(|f| f.to_string())?;
        users.retain(|user| user.user_id != key("U0000010"));
        let two =
            run(Action::Page(Direction::Forward), one.state, &users).map_err(|f| f.to_string())?;
        assert_eq!(
            positions(&two),
            expected(&[
                "U0000012", "U0000013", "U0000014", "U0000015", "", "", "", "", "", ""
            ])
        );
        Ok(())
    }

    // audit: COUSR00C-018, COUSR00C-019, COUSR00C-021
    #[test]
    fn enter_f7_f8_on_the_fixture_shows_ten_blank_rows() -> TestResult {
        let users = fixture();
        let mut state = run(enter(""), PagingState::default(), &users)
            .map_err(|f| f.to_string())?
            .state;
        for _ in 0..2 {
            let top =
                run(Action::Page(Direction::Backward), state, &users).map_err(|f| f.to_string())?;
            assert_eq!(top.rows, PageRows::Keep);
            assert_eq!(top.message, Some(ListMessage::AlreadyAtTop));
            assert_eq!(
                top.message_text(),
                Some("You are already at the top of the page...")
            );
            assert_eq!(top.state.next_page, NextPage::Yes);
            assert_eq!(top.state.page_no, page_no(1)?);

            let bottom = run(Action::Page(Direction::Forward), top.state, &users)
                .map_err(|f| f.to_string())?;
            assert_eq!(positions(&bottom), expected(&[""; 10]));
            assert_eq!(bottom.state.page_no, page_no(1)?);
            assert_eq!(bottom.state.next_page, NextPage::No);
            assert_eq!(bottom.state.first_key, key("ADMIN001"));
            assert_eq!(bottom.state.last_key, key("USER0005"));
            assert_eq!(bottom.message, Some(ListMessage::ReachedBottom));
            state = bottom.state;
        }
        Ok(())
    }

    // audit: COUSR00C-020
    #[test]
    fn f8_without_a_next_page_reads_nothing() -> TestResult {
        let state = PagingState {
            first_key: key("ADMIN001"),
            last_key: key("USER0005"),
            page_no: page_no(1)?,
            next_page: NextPage::No,
        };
        let Plan::Respond(answer) =
            plan(Action::Page(Direction::Forward), state.clone(), 10).map_err(|f| f.to_string())?
        else {
            return Err("F8 with no next page reads nothing".to_owned());
        };
        assert_eq!(answer.rows, PageRows::Keep);
        assert_eq!(answer.rows.filled().count(), 0);
        assert_eq!(answer.state, state);
        assert_eq!(answer.message, Some(ListMessage::AlreadyAtBottom));
        assert_eq!(
            answer.message_text(),
            Some("You are already at the bottom of the page...")
        );
        Ok(())
    }

    // audit: COUSR00C-021
    #[test]
    fn f7_on_page_one_or_less_sets_the_next_page_indicator_and_reads_nothing() -> TestResult {
        for number in [0, 1] {
            let state = PagingState {
                first_key: key("ADMIN001"),
                last_key: LOW_KEY,
                page_no: page_no(number)?,
                next_page: NextPage::No,
            };
            let Plan::Respond(answer) = plan(Action::Page(Direction::Backward), state.clone(), 10)
                .map_err(|f| f.to_string())?
            else {
                return Err("F7 on page 1 reads nothing".to_owned());
            };
            assert_eq!(answer.rows, PageRows::Keep);
            assert_eq!(answer.message, Some(ListMessage::AlreadyAtTop));
            assert_eq!(
                answer.state,
                PagingState {
                    next_page: NextPage::Yes,
                    ..state
                }
            );
        }
        Ok(())
    }

    /// The paging state of page `number` whose first user ID is `first`.
    fn on_page(number: u64, first: &str, last: &str) -> Result<PagingState, String> {
        Ok(PagingState {
            first_key: key(first),
            last_key: key(last),
            page_no: page_no(number)?,
            next_page: NextPage::No,
        })
    }

    // audit: COUSR00C-016, COUSR00C-022, COUSR00C-023
    #[test]
    fn f7_pages_backward_before_the_first_user_shown() -> TestResult {
        let users = numbered(25);
        let state = on_page(3, "U0000021", "U0000020")?;
        let Plan::Read(browse) = plan(Action::Page(Direction::Backward), state.clone(), 10)
            .map_err(|f| f.to_string())?
        else {
            return Err("F7 above page 1 reads".to_owned());
        };
        assert_eq!(browse.direction(), Direction::Backward);
        assert_eq!(browse.key(), &key("U0000021"));
        assert_eq!(browse.fetch_limit(), 12);

        let two =
            run(Action::Page(Direction::Backward), state, &users).map_err(|f| f.to_string())?;
        let ids: Vec<String> = (11..=20).map(|n| format!("U{n:07}")).collect();
        assert_eq!(positions(&two), ids);
        assert_eq!(two.state.page_no, page_no(2)?);
        assert_eq!(two.state.next_page, NextPage::Yes);
        assert_eq!(two.state.first_key, key("U0000011"));
        assert_eq!(two.state.last_key, key("U0000020"));
        assert_eq!(two.message, None);

        let one =
            run(Action::Page(Direction::Backward), two.state, &users).map_err(|f| f.to_string())?;
        let ids: Vec<String> = (1..=10).map(|n| format!("U{n:07}")).collect();
        assert_eq!(positions(&one), ids);
        assert_eq!(one.state.page_no, page_no(1)?);
        assert_eq!(one.state.first_key, key("U0000001"));
        assert_eq!(one.message, Some(ListMessage::ReachedTop));
        assert_eq!(
            one.message_text(),
            Some("You have reached the top of the page...")
        );
        Ok(())
    }

    // audit: COUSR00C-023
    #[test]
    fn a_full_backward_page_from_page_two_goes_to_page_one_while_users_remain_before() -> TestResult
    {
        // Users added before the list's start since page 1 was shown: the page number follows
        // the count of pages paged, not the users left, so no top message is moved.
        let users = numbered(25);
        let Plan::Read(browse) = plan(
            Action::Page(Direction::Backward),
            on_page(2, "U0000021", "")?,
            10,
        )
        .map_err(|f| f.to_string())?
        else {
            return Err("F7 above page 1 reads".to_owned());
        };
        let rows = read(
            &users,
            browse.direction(),
            browse.key(),
            browse.fetch_limit(),
        );
        let answer = page(browse, rows).map_err(|f| f.to_string())?;
        assert_eq!(answer.state.page_no, page_no(1)?);
        assert_eq!(answer.message, None);
        Ok(())
    }

    // audit: COUSR00C-016, COUSR00C-024
    #[test]
    fn a_short_backward_page_fills_from_the_last_row_and_keeps_its_number() -> TestResult {
        let users = numbered(25);
        let answer = run(
            Action::Page(Direction::Backward),
            on_page(4, "U0000006", "U0000030")?,
            &users,
        )
        .map_err(|f| f.to_string())?;
        assert_eq!(
            positions(&answer),
            expected(&[
                "", "", "", "", "", "U0000001", "U0000002", "U0000003", "U0000004", "U0000005"
            ])
        );
        assert_eq!(answer.state.page_no, page_no(4)?);
        assert_eq!(answer.state.first_key, key("U0000006"));
        assert_eq!(answer.state.last_key, key("U0000005"));
        assert_eq!(answer.state.next_page, NextPage::Yes);
        assert_eq!(answer.message, Some(ListMessage::ReachedTop));
        Ok(())
    }

    // audit: COUSR00C-022
    #[test]
    fn f7_keeps_the_user_before_a_first_key_that_is_gone() -> TestResult {
        let mut users = numbered(25);
        users.retain(|user| user.user_id != key("U0000021"));
        let answer = run(
            Action::Page(Direction::Backward),
            on_page(3, "U0000021", "U0000030")?,
            &users,
        )
        .map_err(|f| f.to_string())?;
        let ids: Vec<String> = (11..=20).map(|n| format!("U{n:07}")).collect();
        assert_eq!(positions(&answer), ids);
        assert_eq!(answer.state.page_no, page_no(2)?);
        Ok(())
    }

    // audit: COUSR00C-022, COUSR00C-024
    #[test]
    fn f7_with_a_blank_first_key_reads_from_low_values() -> TestResult {
        let users = fixture();
        for blank in ["", "\0\0\0\0\0\0\0\0"] {
            let state = on_page(2, blank, "USER0005")?;
            let Plan::Read(browse) = plan(Action::Page(Direction::Backward), state.clone(), 10)
                .map_err(|f| f.to_string())?
            else {
                return Err("F7 above page 1 reads".to_owned());
            };
            assert_eq!(browse.key(), &LOW_KEY);
            let answer =
                run(Action::Page(Direction::Backward), state, &users).map_err(|f| f.to_string())?;
            assert_eq!(positions(&answer), expected(&[""; 10]));
            assert_eq!(answer.state.page_no, page_no(2)?);
            assert_eq!(answer.message, Some(ListMessage::ReachedTop));
        }
        Ok(())
    }

    // audit: COUSR00C-026
    #[test]
    fn a_failed_lookup_is_a_store_failure_with_the_cobol_text() {
        let failure = UserListFailure::LookupFailed;
        assert_eq!(failure.message(), "Unable to lookup User...");
        assert_eq!(failure.to_string(), "Unable to lookup User...");
        assert_eq!(failure.class(), FailureClass::Store);
        assert_eq!(failure.class().http_status(), 500);
        let boxed: Box<dyn std::error::Error> = Box::new(failure);
        assert_eq!(boxed.to_string(), "Unable to lookup User...");
    }

    // audit: COUSR00C-026
    #[test]
    fn rows_out_of_read_order_are_a_failed_lookup() -> TestResult {
        let users = numbered(5);
        let forward = |state: PagingState| plan(enter("U0000003"), state, 10);
        let backward = |state: PagingState| plan(Action::Page(Direction::Backward), state, 10);
        let cases = [
            (
                forward(PagingState::default()),
                read(&users, Direction::Backward, &key("U0000005"), 5),
            ),
            (
                forward(PagingState::default()),
                read(&users, Direction::Forward, &LOW_KEY, 5),
            ),
            (
                backward(on_page(2, "U0000003", "")?),
                read(&users, Direction::Forward, &key("U0000003"), 5),
            ),
            (
                backward(on_page(2, "U0000003", "")?),
                read(&users, Direction::Backward, &key("U0000005"), 5),
            ),
        ];
        for (planned, rows) in cases {
            let Ok(Plan::Read(browse)) = planned else {
                return Err("each case reads".to_owned());
            };
            assert_eq!(page(browse, rows), Err(UserListFailure::LookupFailed));
        }
        Ok(())
    }

    // audit: COUSR00C-013
    #[test]
    fn rows_beyond_the_fetch_limit_are_ignored() -> TestResult {
        let users = numbered(30);
        let Plan::Read(browse) =
            plan(enter(""), PagingState::default(), 10).map_err(|f| f.to_string())?
        else {
            return Err("ENTER reads a page".to_owned());
        };
        let answer = page(browse, users.clone()).map_err(|f| f.to_string())?;
        assert_eq!(answer.rows.filled().count(), 10);
        assert_eq!(answer.state.next_page, NextPage::Yes);

        // Twelve rows past the limit, the twelfth out of order, are still a page.
        let mut rows: Vec<UserListRow> = users.into_iter().take(11).collect();
        rows.push(row("A0000000", "FIRST", "LAST", "U"));
        let Plan::Read(browse) =
            plan(enter(""), PagingState::default(), 10).map_err(|f| f.to_string())?
        else {
            return Err("ENTER reads a page".to_owned());
        };
        assert!(page(browse, rows).is_ok());
        Ok(())
    }

    /// The halted browses run for coverage only; no value they produce is asserted.
    #[test]
    fn halted_browses_answer_without_panicking() -> TestResult {
        // X-COUSR00C-H1: no user at or above the key, on ENTER and on F8.
        let users = fixture();
        let gone = PagingState {
            next_page: NextPage::Yes,
            ..on_page(1, "ADMIN001", "USER0005")?
        };
        let halted = [
            run(enter("ZZZZZZZZ"), PagingState::default(), &users),
            run(enter(""), PagingState::default(), &[]),
            run(Action::Page(Direction::Forward), gone, &[]),
        ];
        let texts: Vec<Option<&str>> = halted
            .iter()
            .map(|answer| answer.as_ref().ok().and_then(UserPage::message_text))
            .collect();
        assert_eq!(texts.len(), halted.len());
        // X-COUSR00C-H2: F8 with a next page and a blank last user ID.
        let blank_last = PagingState {
            next_page: NextPage::Yes,
            ..on_page(1, "ADMIN001", "")?
        };
        let _h2 = plan(Action::Page(Direction::Forward), blank_last, 10);
        Ok(())
    }

    // audit: COUSR00C-028
    #[test]
    fn a_row_carries_the_four_shown_fields_and_never_the_password() {
        let record = UserSecurityRecord {
            id: key("USER0003"),
            fname: PicX::moved_from(b"LAURITZ"),
            lname: PicX::moved_from(b"ALME"),
            pwd: Password::new(PicX::moved_from(b"FAKE-PW1")),
            usr_type: PicX::moved_from(b"U"),
            filler: PicX::moved_from(b"FILLER BYTES"),
        };
        let shown = UserListRow::from(&record);
        assert_eq!(shown, row("USER0003", "LAURITZ", "ALME", "U"));
        let debug = format!("{shown:?}");
        assert!(!debug.contains("FAKE-PW1"));
        assert!(!debug.contains("FILLER BYTES"));
        assert!(debug.contains("USER0003"));
    }

    // audit: COUSR00C-028
    #[test]
    fn a_page_entry_becomes_the_row_of_its_four_fields_as_stored() {
        let entry = UserListEntry {
            id: key("USER0001"),
            fname: PicX::moved_from(b"LAWRENCE"),
            lname: PicX::moved_from(b"THOMAS"),
            usr_type: PicX::moved_from(b"U"),
        };
        assert_eq!(
            UserListRow::from(entry),
            row("USER0001", "LAWRENCE", "THOMAS", "U")
        );
    }

    // audit: COUSR00C-006, COUSR00C-014, COUSR00C-020, COUSR00C-021, COUSR00C-023
    #[test]
    fn messages_and_indicators_are_the_cobol_values() {
        for (message, text) in [
            (
                ListMessage::AlreadyAtTop,
                "You are already at the top of the page...",
            ),
            (
                ListMessage::AlreadyAtBottom,
                "You are already at the bottom of the page...",
            ),
            (
                ListMessage::ReachedTop,
                "You have reached the top of the page...",
            ),
            (
                ListMessage::ReachedBottom,
                "You have reached the bottom of the page...",
            ),
        ] {
            assert_eq!(message.text(), text);
            assert!(text.len() <= 78);
        }
        assert!(NextPage::Yes.is_yes());
        assert!(!NextPage::No.is_yes());
        assert_eq!(NextPage::default(), NextPage::No);
        let initial = PagingState::default();
        assert!(initial.first_key.is_spaces() && initial.last_key.is_spaces());
        assert_eq!(initial.page_no, PicU::ZERO);
        assert_eq!(DEFAULT_PAGE_SIZE, 10);
    }

    // audit: COUSR00C-014
    #[test]
    fn the_page_number_keeps_eight_digits() -> TestResult {
        let users = numbered(25);
        let state = PagingState {
            next_page: NextPage::Yes,
            ..on_page(99_999_999, "U0000001", "U0000010")?
        };
        let answer =
            run(Action::Page(Direction::Forward), state, &users).map_err(|f| f.to_string())?;
        assert_eq!(answer.state.page_no, PicU::ZERO);
        Ok(())
    }

    #[test]
    fn any_page_size_is_total() -> TestResult {
        let users = numbered(3);
        for size in [0, 1, 2, 3, 4, 100] {
            let Plan::Read(browse) =
                plan(enter(""), PagingState::default(), size).map_err(|f| f.to_string())?
            else {
                return Err("ENTER reads a page".to_owned());
            };
            let limit = browse.fetch_limit();
            let rows = read(&users, Direction::Forward, &LOW_KEY, limit);
            let answer = page(browse, rows).map_err(|f| f.to_string())?;
            let PageRows::Replace(positions) = &answer.rows else {
                return Err("a page read replaces the rows".to_owned());
            };
            assert_eq!(positions.len(), size);
            assert_eq!(answer.rows.filled().count(), size.min(3));
        }
        Ok(())
    }
}
