//! COTRTLIC, the transaction-type list (transaction `CTLI`, map `CTRTLIA` of mapset `COTRTLI`):
//! the rules of endpoint 24, `GET /api/v1/transaction-types` (the screen load and F7/F8), and
//! endpoint 25, `POST /api/v1/transaction-types/list-actions` (ENTER and F10), both behind the
//! admin gate, which is the verified token and not this module (COTRTLIC-001; AAP 0.3.4.1).
//! The list is the Db2 data version of the transaction types, `db2.transaction_type`
//! ([`Db2TransactionType`]), never the VSAM version (E23).
//!
//! This module ports the `server` rows of `modern/audit/online/COTRTLIC.md`: the first entry
//! (COTRTLIC-002, COTRTLIC-003), the page reads and their filters (COTRTLIC-004, COTRTLIC-005),
//! the refresh and the page moves (COTRTLIC-006 to COTRTLIC-013), the input received from the
//! screen (COTRTLIC-017, COTRTLIC-018), the edits in the program's order (COTRTLIC-019 to
//! COTRTLIC-034), the ENTER and F10 workflow (COTRTLIC-035 to COTRTLIC-043), the commits and
//! their outcomes (COTRTLIC-044 to COTRTLIC-052) and the Db2 message format (COTRTLIC-054,
//! COTRTLIC-055). The `client` rows, the key mapping, F2 to CTTU, F3 back and the attributes
//! (COTRTLIC-014 to COTRTLIC-016, COTRTLIC-056 to COTRTLIC-059), belong to the screen; the
//! attributes the next request depends on are kept in the state (COTRTLIC-018).
//!
//! # Flow
//!
//! One request runs as a pure state machine, [`Progress`]. [`load`] starts the screen load,
//! [`page`] F7 and F8, [`act`] ENTER and F10. Each step names the one store call the COBOL makes
//! at that point and takes its result back; `api::services::cotrtlic` only runs the call:
//!
//! - [`CountStep`]: `9100-CHECK-FILTERS`, the count under the filter predicates (COTRTLIC-031);
//! - [`VerifyStep`]: the re-read of the row a commit targets, so that the echoed state carries no
//!   authority (AAP 0.3.4.1);
//! - [`CommitStep`]: the keyed update or delete, `9200-UPDATE-RECORD` or `9300-DELETE-RECORD`;
//! - [`ReadStep`]: the page read, `8000-READ-FORWARD` or `8100-READ-BACKWARDS`, the last call of
//!   every request that reads;
//! - [`Progress::Done`]: the response, with no further call.
//!
//! Every branch on the state, the edits and the store results is here (Thin Request Handlers).
//!
//! # State
//!
//! [`TranTypeListState`] is the program's COMMAREA, `WS-THIS-PROGCOMMAREA`
//! (`COTRTLIC.cbl:377-418`), which the client echoes unchanged, plus what the previous response
//! left on the screen: the rows shown and which fields were enterable, because a 3270 returns a
//! protected field as displayed or not at all (COTRTLIC-018, escalation candidate X-COTRTLIC-9).
//! The rows are kept as the last read returned them, and both pending flags are kept, as they can
//! be set together (COTRTLIC-043). The state carries no authority: every request re-runs every
//! edit, and every commit re-reads its row first.
//!
//! # Halts
//!
//! - `HALTED(H3)`: the text of a Db2 message after ` SQLCODE:` is Db2's (COTRTLIC-055). A
//!   message here is the program's action text, trimmed, followed by ` SQLCODE:` and nothing
//!   else; it is the asserted part of COTRTLIC-054, and the halted remainder is left empty.
//! - Halt candidate X-COTRTLIC-H2: when the filter count fails, whether Db2 leaves the count
//!   unassigned decides the message. The count is read as zero here, the COBOL's flow for an
//!   unassigned host variable, which is the replacement's choice and no parity result
//!   (COTRTLIC-032).
//! - `9998-PRIMING-QUERY` is no step of its own: a database the service cannot reach is its
//!   store failure, answered with [`Db2Action::AccessFailure`] and no page or state
//!   (COTRTLIC-053).
//!
//! # Totality
//!
//! Every function takes any text, empty, longer than its field or not ASCII, and returns a value
//! or a typed error; nothing indexes, slices, unwraps or panics (Total Parsers Over Untrusted
//! Input). Map fields are read as a `MOVE` into their `PIC X(n)` fields: the first `n`
//! characters, padded with spaces.

use std::array;
use std::fmt;
use std::iter;

use copybook::PicX;

use crate::records::db2_transaction_type::Db2TransactionType;

/// Rows of a page when the caller sends no `page_size`: `WS-MAX-SCREEN-LINES`, 7 (COTRTLIC-004;
/// Bounded Pagination).
pub const DEFAULT_PAGE_SIZE: usize = 7;

/// Row positions of the map and of the COMMAREA: `EACH-ROWI OCCURS 7` and
/// `WS-CA-SCREEN-ROWS-OUT OCCURS 7` (COTRTLIC-017).
pub const SCREEN_ROWS: usize = 7;

/// Width of the type filter, `TRTYPEI PIC X(2)` and `WS-CA-TYPE-CD PIC X(02)` (COTRTLIC-017).
const TYPE_FILTER_WIDTH: usize = 2;

/// Width of the description filter and of a row description, `TRDESCI` and `TRTYPDI PIC X(50)`
/// (COTRTLIC-017).
const DESCRIPTION_WIDTH: usize = 50;

/// Width of a selection, `TRTSELI PIC X(1)` (COTRTLIC-017).
const SELECTION_WIDTH: usize = 1;

/// Width of `WS-RETURN-MSG PIC X(75)`, to which a Db2 message is cut (COTRTLIC-054).
const RETURN_MSG_WIDTH: usize = 75;

/// The page number modulus: `WS-CA-SCREEN-NUM PIC 9(1)` keeps one digit (COTRTLIC-013).
const PAGE_MODULUS: u8 = 10;

const NO_RECORDS_FOUND: &str = "No records found for this search condition.";
const NO_MORE_PAGES_FOR_SEARCH: &str = "No more pages for these search conditions";
const MORE_THAN_ONE_ACTION: &str = "Please select only 1 action";
const INVALID_ACTION_CODE: &str = "Action code selected is invalid";
const NO_CHANGE_DETECTED: &str = "No change detected with respect to database values.";
const DESCRIPTION_REQUIRED: &str = "Transaction Desc must be supplied.";
const DESCRIPTION_NOT_ALPHANUMERIC: &str = "Transaction Desc can have numbers or alphabets only.";
const TYPE_FILTER_NOT_NUMERIC: &str = "TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER";
const NO_RECORDS_FOR_FILTERS: &str = "No Records found for these filter conditions";
const NO_PREVIOUS_PAGES: &str = "No previous pages to display";
const NO_MORE_PAGES: &str = "No more pages to display";

/// The text `9999-FORMAT-DB2-MESSAGE` strings after the trimmed action (COTRTLIC-054).
const SQLCODE_LABEL: &str = " SQLCODE:";

/// The message of the map's error line: `WS-RETURN-MSG PIC X(75)`, moved to `ERRMSGO`
/// (COTRTLIC-033).
///
/// A request keeps one message, the last written: some are written whatever stands, others only
/// when none is set yet, in the order COTRTLIC-033 lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListMessage {
    /// `No records found for this search condition.`: page 1 read forward found no row
    /// (COTRTLIC-003).
    NoRecordsFound,
    /// `No more pages for these search conditions`: F8 read the last page (COTRTLIC-007,
    /// COTRTLIC-008).
    NoMorePagesForSearch,
    /// `Please select only 1 action` (COTRTLIC-022).
    MoreThanOneAction,
    /// `Action code selected is invalid` (COTRTLIC-021).
    InvalidActionCode,
    /// `No change detected with respect to database values.`, which is no input error
    /// (COTRTLIC-024).
    NoChangeDetected,
    /// `Transaction Desc must be supplied.` (COTRTLIC-025).
    DescriptionRequired,
    /// `Transaction Desc can have numbers or alphabets only.` (COTRTLIC-026).
    DescriptionNotAlphanumeric,
    /// `TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER` (COTRTLIC-028).
    TypeFilterNotNumeric,
    /// `No Records found for these filter conditions` (COTRTLIC-031).
    NoRecordsForFilters,
    /// `No previous pages to display`: F7 on, or onto, page 1 (COTRTLIC-010, COTRTLIC-011).
    NoPreviousPages,
    /// `No more pages to display`: F8 again on the last page (COTRTLIC-009).
    NoMorePages,
    /// A Db2 message (COTRTLIC-054).
    Db2(Db2Action),
}

impl ListMessage {
    /// The text of the error line.
    ///
    /// A Db2 message is its action text with its leading and trailing spaces removed, then
    /// ` SQLCODE:`, cut to the 75 characters of `WS-RETURN-MSG`; what Db2 would append after it
    /// is `HALTED(H3)` and is left empty (COTRTLIC-054, COTRTLIC-055).
    #[must_use]
    pub fn text(self) -> String {
        let literal = match self {
            Self::NoRecordsFound => NO_RECORDS_FOUND,
            Self::NoMorePagesForSearch => NO_MORE_PAGES_FOR_SEARCH,
            Self::MoreThanOneAction => MORE_THAN_ONE_ACTION,
            Self::InvalidActionCode => INVALID_ACTION_CODE,
            Self::NoChangeDetected => NO_CHANGE_DETECTED,
            Self::DescriptionRequired => DESCRIPTION_REQUIRED,
            Self::DescriptionNotAlphanumeric => DESCRIPTION_NOT_ALPHANUMERIC,
            Self::TypeFilterNotNumeric => TYPE_FILTER_NOT_NUMERIC,
            Self::NoRecordsForFilters => NO_RECORDS_FOR_FILTERS,
            Self::NoPreviousPages => NO_PREVIOUS_PAGES,
            Self::NoMorePages => NO_MORE_PAGES,
            Self::Db2(action) => {
                // HALTED(H3): the edited SQLCODE and the DSNTIAC text that follow the label are
                // Db2's and have no counterpart, so nothing follows it.
                let message = format!("{}{SQLCODE_LABEL}", action.text().trim_matches(' '));
                return message.chars().take(RETURN_MSG_WIDTH).collect();
            }
        };
        literal.to_owned()
    }
}

/// The action text `WS-DB2-CURRENT-ACTION` a Db2 message starts with (COTRTLIC-054).
///
/// The replacement reads a page with one statement, which fails as a whole, so a failed read is
/// the COBOL's failed cursor open (COTRTLIC-052); a fetch or close of its own cannot fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Db2Action {
    /// `C-TR-TYPE-FORWARD Open`: the forward page read failed (COTRTLIC-052).
    ForwardOpen,
    /// `C-TR-TYPE-BACKWARD Open`: the backward page read failed (COTRTLIC-052).
    BackwardOpen,
    /// `Error on fetch Cursor C-TR-TYPE-BACKWARD`: the backward read found fewer rows than a page
    /// (COTRTLIC-012).
    BackwardFetch,
    /// `Error reading TRANSACTION_TYPE table `: the filter count failed (COTRTLIC-032).
    CountFilters,
    /// `Record not found. Deleted by others ? `: the update found no row (COTRTLIC-046).
    UpdateNotFound,
    /// `Deadlock. Someone else updating ?` (COTRTLIC-047).
    UpdateDeadlock,
    /// `Update failed with` (COTRTLIC-048).
    UpdateFailed,
    /// `Please delete associated child records first:`: the type still has categories
    /// (COTRTLIC-050).
    DeleteRestricted,
    /// `Delete failed with message:` (COTRTLIC-051).
    DeleteFailed,
    /// `Db2 access failure. `: `9998-PRIMING-QUERY` found the database unreachable, which no
    /// step here meets; the service answers with this message, no page and no state, when it
    /// cannot reach the database (COTRTLIC-053).
    AccessFailure,
}

impl Db2Action {
    /// The literal moved to `WS-DB2-CURRENT-ACTION PIC X(72)`, spaces included.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::ForwardOpen => "C-TR-TYPE-FORWARD Open",
            Self::BackwardOpen => "C-TR-TYPE-BACKWARD Open",
            Self::BackwardFetch => "Error on fetch Cursor C-TR-TYPE-BACKWARD",
            Self::CountFilters => "Error reading TRANSACTION_TYPE table ",
            Self::UpdateNotFound => "Record not found. Deleted by others ? ",
            Self::UpdateDeadlock => "Deadlock. Someone else updating ?",
            Self::UpdateFailed => "Update failed with",
            Self::DeleteRestricted => "Please delete associated child records first:",
            Self::DeleteFailed => "Delete failed with message:",
            Self::AccessFailure => "Db2 access failure. ",
        }
    }
}

/// The message of the map's info line, `WS-INFO-MSG PIC X(45)` → `INFOMSGO`, which the screen
/// centres (COTRTLIC-058).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InfoMessage {
    /// `Type U to update, D to delete any record` (COTRTLIC-002, COTRTLIC-006).
    RecordActions,
    /// `Delete HIGHLIGHTED row ? Press F10 to confirm` (COTRTLIC-039).
    DeletePrompt,
    /// `Update HIGHLIGHTED row. Press F10 to save` (COTRTLIC-040).
    UpdatePrompt,
    /// `HIGHLIGHTED row deleted.Hit Enter to continue` (COTRTLIC-049).
    Deleted,
    /// `HIGHLIGHTED row was updated` (COTRTLIC-045).
    Updated,
}

impl InfoMessage {
    /// The literal of the 88-level on `WS-INFO-MSG` (`COTRTLIC.cbl:239-248`).
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::RecordActions => "Type U to update, D to delete any record",
            Self::DeletePrompt => "Delete HIGHLIGHTED row ? Press F10 to confirm",
            Self::UpdatePrompt => "Update HIGHLIGHTED row. Press F10 to save",
            Self::Deleted => "HIGHLIGHTED row deleted.Hit Enter to continue",
            Self::Updated => "HIGHLIGHTED row was updated",
        }
    }
}

/// The status class of a response, which the API turns into its HTTP status; ordered so that
/// the most severe event of a request decides (COTRTLIC.md, "Reading the table").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StatusClass {
    /// No error: 200, also with a non-error message such as COTRTLIC-024's.
    Ok,
    /// An input error or a delete restricted by categories: 400 (COTRTLIC-021 to COTRTLIC-031,
    /// COTRTLIC-050).
    Input,
    /// The row another user deleted: 404 (COTRTLIC-046, COTRTLIC-051).
    NotFound,
    /// A store failure: 500 (COTRTLIC-012, COTRTLIC-032, COTRTLIC-047, COTRTLIC-048,
    /// COTRTLIC-051, COTRTLIC-052).
    Store,
}

impl StatusClass {
    /// The HTTP status the audit assigns to the class.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::Ok => 200,
            Self::Input => 400,
            Self::NotFound => 404,
            Self::Store => 500,
        }
    }
}

/// Why an echoed state cannot be represented. No variant carries any part of the input.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListFailure {
    /// More than the seven row positions of the COMMAREA (COTRTLIC-017).
    TooManyRows,
    /// A row number outside 1 to 7, the range of `WS-CA-ROW-SELECTED` (COTRTLIC-023).
    RowOutOfRange,
    /// A page number outside 0 to 9, the range of `WS-CA-SCREEN-NUM PIC 9(1)` (COTRTLIC-013).
    PageOutOfRange,
    /// A filter or a shown description longer than its map field (COTRTLIC-017).
    FieldTooLong,
    /// A type code or key that is not one or two printable ASCII characters (COTRTLIC-004).
    InvalidKey,
    /// A row description longer than `TR_DESCRIPTION VARCHAR(50)` (COTRTLIC-004).
    InvalidRow,
}

impl fmt::Display for ListFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TooManyRows => "the list state holds more than seven rows",
            Self::RowOutOfRange => "a row number of the list state is outside 1 to 7",
            Self::PageOutOfRange => "the page number of the list state is outside 0 to 9",
            Self::FieldTooLong => "a field of the list state is longer than its map field",
            Self::InvalidKey => "a type code of the list state is not a valid key",
            Self::InvalidRow => "a row description of the list state is longer than 50 bytes",
        })
    }
}

impl std::error::Error for ListFailure {}

/// A row position of the map, 1 to 7 on the screen and 0 to 6 here (`WS-CA-ROW-SELECTED`,
/// `I-SELECTED`; COTRTLIC-020, COTRTLIC-023). Only [`RowIndex::new`] and
/// [`RowIndex::from_number`] build one, so a position is always in range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RowIndex(u8);

impl RowIndex {
    /// The seven positions, top to bottom.
    pub const ALL: [Self; SCREEN_ROWS] = [
        Self(0),
        Self(1),
        Self(2),
        Self(3),
        Self(4),
        Self(5),
        Self(6),
    ];

    /// The position `index`, counted from 0; `None` from 7 on.
    #[must_use]
    pub fn new(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The position of screen row `number`, counted from 1 as `WS-CA-ROW-SELECTED` counts it;
    /// `None` outside 1 to 7.
    #[must_use]
    pub fn from_number(number: i64) -> Option<Self> {
        number
            .checked_sub(1)
            .and_then(|index| usize::try_from(index).ok())
            .and_then(Self::new)
    }

    /// The position counted from 0.
    #[must_use]
    pub fn index(self) -> usize {
        usize::from(self.0)
    }

    /// The screen row number, 1 to 7.
    #[must_use]
    pub const fn number(self) -> u8 {
        self.0 + 1
    }
}

/// The rows a page holds: `page_size`, 7 by default (Bounded Pagination).
///
/// The state is the COMMAREA, whose rows are seven positions that the selections of the next
/// request address (`WS-CA-SCREEN-ROWS-OUT OCCURS 7`, COTRTLIC-017), so a page never holds more
/// than seven rows: a larger `page_size`, which the API admits up to 100, is served as seven.
/// The screen never sends `page_size`, and the audit makes no parity claim for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageSize(usize);

impl PageSize {
    /// [`DEFAULT_PAGE_SIZE`] rows.
    pub const DEFAULT: Self = Self(DEFAULT_PAGE_SIZE);

    /// A page of `requested` rows, at most [`SCREEN_ROWS`]; `None` for 0.
    #[must_use]
    pub fn new(requested: usize) -> Option<Self> {
        (requested > 0).then(|| Self(requested.min(SCREEN_ROWS)))
    }

    /// As [`PageSize::new`], from the `u32` the API resolves `page_size` to.
    #[must_use]
    pub fn from_u32(requested: u32) -> Option<Self> {
        usize::try_from(requested).ok().and_then(Self::new)
    }

    /// The rows a page holds, 1 to 7.
    #[must_use]
    pub const fn rows(self) -> usize {
        self.0
    }

    /// The `LIMIT` of a read: `rows`, plus the lookahead row of the forward read that sets the
    /// next-page indicator (COTRTLIC-004).
    fn limit(self, direction: ReadDirection) -> i64 {
        let rows = match direction {
            ReadDirection::Forward => self.0.saturating_add(1),
            ReadDirection::Backward => self.0,
        };
        // TRUNCATION: a page holds at most 7 rows, so the limit is at most 8 and always fits.
        i64::try_from(rows).unwrap_or(i64::MAX)
    }
}

impl Default for PageSize {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The page number, `WS-CA-SCREEN-NUM PIC 9(1)`: one digit, 0 to 9 (COTRTLIC-013, COTRTLIC-029).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageNumber(u8);

impl PageNumber {
    /// Page 1, `CA-FIRST-PAGE`.
    pub const FIRST: Self = Self(1);

    /// Page 0, the value the paging reset leaves until a read finds a row (COTRTLIC-029).
    const ZERO: Self = Self(0);

    /// The page `number`; `None` outside 0 to 9.
    #[must_use]
    pub fn new(number: i64) -> Option<Self> {
        u8::try_from(number)
            .ok()
            .filter(|digit| *digit < PAGE_MODULUS)
            .map(Self)
    }

    /// The page number, 0 to 9.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    const fn is_first(self) -> bool {
        self.0 == 1
    }

    /// `ADD +1 TO WS-CA-SCREEN-NUM`.
    const fn succ(self) -> Self {
        // TRUNCATION: the field is PIC 9(1) and the ADD has no ON SIZE ERROR, so only the units
        // digit is kept and page 9 plus 1 is 0 (COTRTLIC-013, X-COTRTLIC-5).
        Self((self.0 + 1) % PAGE_MODULUS)
    }

    /// `SUBTRACT 1 FROM WS-CA-SCREEN-NUM`.
    const fn pred(self) -> Self {
        // TRUNCATION: the field is unsigned, so the -1 that page 0 minus 1 gives is stored
        // without its sign, as 1; the result has one digit.
        Self(self.0.abs_diff(1) % PAGE_MODULUS)
    }
}

/// An alphanumeric map or COMMAREA field of `N` characters, `PIC X(N)`: always exactly `N`
/// characters, padded with spaces.
///
/// [`ScreenText::read`] is a `MOVE` of a received value (COTRTLIC-017): its first `N` characters,
/// padded with spaces. A NUL character stands for a low-value byte. Values compare as the padded
/// fields do in COBOL.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScreenText<const N: usize>(String);

impl<const N: usize> ScreenText<N> {
    /// `raw` moved into the field: its first `N` characters, padded with spaces.
    #[must_use]
    pub fn read(raw: &str) -> Self {
        let mut text: String = raw.chars().take(N).collect();
        let taken = text.chars().count();
        text.extend(iter::repeat_n(' ', N.saturating_sub(taken)));
        Self(text)
    }

    /// An echoed field: `raw` padded with spaces.
    ///
    /// # Errors
    ///
    /// [`ListFailure::FieldTooLong`] when `raw` has more than `N` characters, which no field the
    /// state returned can hold.
    pub fn echoed(raw: &str) -> Result<Self, ListFailure> {
        if raw.chars().count() > N {
            Err(ListFailure::FieldTooLong)
        } else {
            Ok(Self::read(raw))
        }
    }

    /// `N` spaces, the value `INITIALIZE` gives the field.
    #[must_use]
    pub fn blank() -> Self {
        Self(iter::repeat_n(' ', N).collect())
    }

    /// `N` low-values, the value of a field the 3270 did not send.
    fn low_values() -> Self {
        Self(iter::repeat_n('\0', N).collect())
    }

    /// The `N` characters, padding included.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The characters without their trailing spaces, as the screen shows them.
    #[must_use]
    pub fn shown(&self) -> &str {
        self.0.trim_end_matches(' ')
    }

    /// `FUNCTION TRIM`: without leading and trailing spaces; low-values are kept.
    fn trimmed(&self) -> &str {
        self.0.trim_matches(' ')
    }

    /// `EQUAL SPACES` or `EQUAL LOW-VALUES`.
    fn is_blank(&self) -> bool {
        self.0.chars().all(|c| c == ' ') || self.0.chars().all(|c| c == '\0')
    }
}

impl<const N: usize> Default for ScreenText<N> {
    fn default() -> Self {
        Self::blank()
    }
}

/// The pending flags of the COMMAREA, `WS-CA-DELETE-FLAG` and `WS-CA-UPDATE-FLAG`, which can be
/// set together (COTRTLIC-043, escalation candidate X-COTRTLIC-1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PendingAction {
    /// Neither flag is set.
    #[default]
    None,
    /// `CA-UPDATE-REQUESTED` (COTRTLIC-040).
    Update,
    /// `CA-DELETE-REQUESTED` (COTRTLIC-039).
    Delete,
    /// Both flags: a pending update and then a delete at the same row, or the reverse
    /// (COTRTLIC-043).
    UpdateAndDelete,
}

impl PendingAction {
    /// Whether either flag is set, the condition on which F10 can commit (COTRTLIC-036).
    #[must_use]
    pub const fn is_pending(self) -> bool {
        !matches!(self, Self::None)
    }

    /// `SET CA-UPDATE-REQUESTED TO TRUE`.
    const fn with_update(self) -> Self {
        match self {
            Self::None | Self::Update => Self::Update,
            Self::Delete | Self::UpdateAndDelete => Self::UpdateAndDelete,
        }
    }

    /// `SET CA-DELETE-REQUESTED TO TRUE`.
    const fn with_delete(self) -> Self {
        match self {
            Self::None | Self::Delete => Self::Delete,
            Self::Update | Self::UpdateAndDelete => Self::UpdateAndDelete,
        }
    }

    /// `SET CA-UPDATE-SUCCEEDED TO TRUE`: the update flag cleared, the delete flag kept.
    const fn without_update(self) -> Self {
        match self {
            Self::None | Self::Update => Self::None,
            Self::Delete | Self::UpdateAndDelete => Self::Delete,
        }
    }
}

/// `WS-CA-NEXT-PAGE-IND`: whether the forward read found a row after the page (COTRTLIC-004).
///
/// The paging reset leaves the indicator blank, which every test of the program treats as "not
/// exists" (COTRTLIC-029), so it is [`NextPage::NotExists`] here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum NextPage {
    /// `CA-NEXT-PAGE-EXISTS`.
    Exists,
    /// `CA-NEXT-PAGE-NOT-EXISTS`.
    #[default]
    NotExists,
}

/// `WS-CA-LAST-PAGE-DISPLAYED`: whether an F8 already ended on the last page (COTRTLIC-008,
/// COTRTLIC-009).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LastPage {
    /// `CA-LAST-PAGE-SHOWN`, 0, also the value the paging reset leaves (COTRTLIC-029).
    Shown,
    /// `CA-LAST-PAGE-NOT-SHOWN`, 9, which every key but F8 sets.
    #[default]
    NotShown,
}

/// Whether the previous response left the selectors enterable (COTRTLIC-018, COTRTLIC-056).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum SelectorAccess {
    /// Enterable at every row that holds a type.
    #[default]
    Open,
    /// Protected at every row, after a type filter in error or a filter matching no rows
    /// (`FLG-PROTECT-SELECT-ROWS-YES`; COTRTLIC-028, COTRTLIC-031).
    Protected,
}

/// A row as the previous response showed it: the code and description the 3270 returns for the
/// protected fields of that row (COTRTLIC-017, COTRTLIC-018).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScreenRow {
    code: PicX<2>,
    description: ScreenText<DESCRIPTION_WIDTH>,
}

impl ScreenRow {
    /// The code shown, `TRTTYPO`.
    #[must_use]
    pub const fn code(&self) -> &PicX<2> {
        &self.code
    }

    /// The description shown, `TRTYPDO`, without its trailing spaces.
    #[must_use]
    pub fn description(&self) -> &str {
        self.description.shown()
    }
}

/// One echoed row: its code and description, as the state returned them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct RowParts<'a> {
    /// The two-character type code.
    pub code: &'a str,
    /// The description.
    pub description: &'a str,
}

/// The parts of an echoed [`TranTypeListState`], in the shape its accessors return them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StateParts<'a> {
    /// `WS-CA-TYPE-CD`, at most 2 characters.
    pub type_filter: &'a str,
    /// `WS-CA-TYPE-DESC`, at most 50 characters.
    pub desc_filter: &'a str,
    /// The rows as read, at most 7 positions; `None` is an empty position.
    pub rows: &'a [Option<RowParts<'a>>],
    /// `WS-CA-ROW-SELECTED`, a screen row number 1 to 7; `None` for 0.
    pub selected_row: Option<i64>,
    /// `WS-CA-FIRST-TR-CODE`.
    pub first_key: &'a str,
    /// `WS-CA-LAST-TR-CODE`.
    pub last_key: &'a str,
    /// `WS-CA-SCREEN-NUM`, 0 to 9.
    pub page_no: i64,
    /// `WS-CA-NEXT-PAGE-IND`.
    pub next_page: NextPage,
    /// `WS-CA-LAST-PAGE-DISPLAYED`.
    pub last_page: LastPage,
    /// `WS-CA-DELETE-FLAG` and `WS-CA-UPDATE-FLAG`.
    pub pending: PendingAction,
    /// The rows the previous response showed, at most 7 positions.
    pub screen_rows: &'a [Option<RowParts<'a>>],
    /// Whether the previous response left the selectors enterable.
    pub selectors: SelectorAccess,
    /// The screen row number whose description the previous response left enterable.
    pub description_entry: Option<i64>,
}

/// The list state: COTRTLIC's COMMAREA `WS-THIS-PROGCOMMAREA` (`COTRTLIC.cbl:377-418`) and what
/// the previous response left on the screen (COTRTLIC-018, X-COTRTLIC-9). The server returns it
/// with every response and the client echoes it unchanged; it carries no authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranTypeListState {
    /// `WS-CA-TYPE-CD`: the type filter of the displayed page, as typed (COTRTLIC-029).
    type_filter: ScreenText<TYPE_FILTER_WIDTH>,
    /// `WS-CA-TYPE-DESC`: the description filter of the displayed page (COTRTLIC-030).
    desc_filter: ScreenText<DESCRIPTION_WIDTH>,
    /// `WS-CA-EACH-ROW-OUT`: the rows as the last read returned them; `None` is a position the
    /// read left at low-values (COTRTLIC-004).
    rows: [Option<Db2TransactionType>; SCREEN_ROWS],
    /// `WS-CA-ROW-SELECTED` (COTRTLIC-023).
    selected: Option<RowIndex>,
    /// `WS-CA-FIRST-TR-CODE` (COTRTLIC-004).
    first_key: PicX<2>,
    /// `WS-CA-LAST-TR-CODE` (COTRTLIC-004).
    last_key: PicX<2>,
    /// `WS-CA-SCREEN-NUM` (COTRTLIC-013).
    page_no: PageNumber,
    /// `WS-CA-NEXT-PAGE-IND` (COTRTLIC-004).
    next_page: NextPage,
    /// `WS-CA-LAST-PAGE-DISPLAYED` (COTRTLIC-008).
    last_page: LastPage,
    /// `WS-CA-DELETE-FLAG` and `WS-CA-UPDATE-FLAG` (COTRTLIC-043).
    pending: PendingAction,
    /// The rows the previous response showed (COTRTLIC-018).
    screen: [Option<ScreenRow>; SCREEN_ROWS],
    /// Whether the previous response left the selectors enterable (COTRTLIC-018).
    selectors: SelectorAccess,
    /// The row whose description the previous response left enterable (COTRTLIC-018).
    description_entry: Option<RowIndex>,
}

impl TranTypeListState {
    /// The state of a first entry before its read: filters and keys blank, page 1, the last-page
    /// indicator "not shown", nothing pending, no row selected and nothing on the screen
    /// (COTRTLIC-002).
    #[must_use]
    pub fn initial() -> Self {
        Self {
            type_filter: ScreenText::blank(),
            desc_filter: ScreenText::blank(),
            rows: array::from_fn(|_| None),
            selected: None,
            first_key: PicX::spaces(),
            last_key: PicX::spaces(),
            page_no: PageNumber::FIRST,
            next_page: NextPage::NotExists,
            last_page: LastPage::NotShown,
            pending: PendingAction::None,
            screen: array::from_fn(|_| None),
            selectors: SelectorAccess::Open,
            description_entry: None,
        }
    }

    /// The state an echo describes, checked field by field.
    ///
    /// # Errors
    ///
    /// - [`ListFailure::TooManyRows`] for more than seven row positions in `rows` or
    ///   `screen_rows`;
    /// - [`ListFailure::RowOutOfRange`] for a row number outside 1 to 7;
    /// - [`ListFailure::PageOutOfRange`] for a page number outside 0 to 9;
    /// - [`ListFailure::FieldTooLong`] for a filter or shown description longer than its field;
    /// - [`ListFailure::InvalidKey`] for a code or key that is not at most two printable ASCII
    ///   characters;
    /// - [`ListFailure::InvalidRow`] for a description longer than 50 bytes.
    pub fn from_parts(parts: &StateParts<'_>) -> Result<Self, ListFailure> {
        Ok(Self {
            type_filter: ScreenText::echoed(parts.type_filter)?,
            desc_filter: ScreenText::echoed(parts.desc_filter)?,
            rows: positions(parts.rows, read_row)?,
            selected: row_number(parts.selected_row)?,
            first_key: key(parts.first_key)?,
            last_key: key(parts.last_key)?,
            page_no: PageNumber::new(parts.page_no).ok_or(ListFailure::PageOutOfRange)?,
            next_page: parts.next_page,
            last_page: parts.last_page,
            pending: parts.pending,
            screen: positions(parts.screen_rows, screen_row)?,
            selectors: parts.selectors,
            description_entry: row_number(parts.description_entry)?,
        })
    }

    /// `WS-CA-TYPE-CD`, the type filter of the displayed page as typed, padded to 2 characters.
    #[must_use]
    pub fn type_filter(&self) -> &str {
        self.type_filter.as_str()
    }

    /// `WS-CA-TYPE-DESC`, the description filter of the displayed page, padded to 50 characters.
    #[must_use]
    pub fn desc_filter(&self) -> &str {
        self.desc_filter.as_str()
    }

    /// The rows as the last read returned them, `WS-CA-EACH-ROW-OUT`; `None` is an empty
    /// position.
    #[must_use]
    pub const fn rows(&self) -> &[Option<Db2TransactionType>; SCREEN_ROWS] {
        &self.rows
    }

    /// `WS-CA-ROW-SELECTED`: the row of the last accepted selection.
    #[must_use]
    pub const fn selected(&self) -> Option<RowIndex> {
        self.selected
    }

    /// `WS-CA-FIRST-TR-CODE`, from which the displayed page is re-read.
    #[must_use]
    pub const fn first_key(&self) -> &PicX<2> {
        &self.first_key
    }

    /// `WS-CA-LAST-TR-CODE`, from which F8 reads the next page.
    #[must_use]
    pub const fn last_key(&self) -> &PicX<2> {
        &self.last_key
    }

    /// `WS-CA-SCREEN-NUM`.
    #[must_use]
    pub const fn page_no(&self) -> PageNumber {
        self.page_no
    }

    /// `WS-CA-NEXT-PAGE-IND`.
    #[must_use]
    pub const fn next_page(&self) -> NextPage {
        self.next_page
    }

    /// `WS-CA-LAST-PAGE-DISPLAYED`.
    #[must_use]
    pub const fn last_page(&self) -> LastPage {
        self.last_page
    }

    /// The pending flags.
    #[must_use]
    pub const fn pending(&self) -> PendingAction {
        self.pending
    }

    /// The rows the previous response showed.
    #[must_use]
    pub const fn screen_rows(&self) -> &[Option<ScreenRow>; SCREEN_ROWS] {
        &self.screen
    }

    /// Whether the previous response left the selectors enterable.
    #[must_use]
    pub const fn selectors(&self) -> SelectorAccess {
        self.selectors
    }

    /// The row whose description the previous response left enterable.
    #[must_use]
    pub const fn description_entry(&self) -> Option<RowIndex> {
        self.description_entry
    }

    /// `INITIALIZE WS-CA-PAGING-VARIABLES`: keys blank, page 0, the next-page indicator blank
    /// and the last-page indicator 0 (COTRTLIC-029, COTRTLIC-030).
    fn reset_paging(&mut self) {
        self.first_key = PicX::spaces();
        self.last_key = PicX::spaces();
        self.page_no = PageNumber::ZERO;
        self.next_page = NextPage::NotExists;
        self.last_page = LastPage::Shown;
    }

    /// The COMMAREA after a successful delete: `INITIALIZE WS-THIS-PROGCOMMAREA`, which leaves
    /// every row at spaces rather than low-values, then `CA-FIRST-PAGE` and
    /// `CA-LAST-PAGE-NOT-SHOWN` (COTRTLIC-049). The screen is left as the response shows it.
    fn reset_after_delete(&mut self) {
        let blank_row = Db2TransactionType::new(PicX::spaces(), String::new()).ok();
        self.type_filter = ScreenText::blank();
        self.desc_filter = ScreenText::blank();
        self.rows = array::from_fn(|_| blank_row.clone());
        self.selected = None;
        self.first_key = PicX::spaces();
        self.last_key = PicX::spaces();
        self.page_no = PageNumber::FIRST;
        self.next_page = NextPage::NotExists;
        self.last_page = LastPage::NotShown;
        self.pending = PendingAction::None;
    }

    /// The description of the row at `index` as read, or low-values at an empty position.
    fn row_description(&self, index: RowIndex) -> ScreenText<DESCRIPTION_WIDTH> {
        self.rows
            .get(index.index())
            .and_then(Option::as_ref)
            .map_or_else(ScreenText::low_values, |row| {
                ScreenText::read(row.tr_description())
            })
    }
}

impl Default for TranTypeListState {
    fn default() -> Self {
        Self::initial()
    }
}

/// Up to seven echoed positions, each converted with `convert`.
fn positions<T>(
    parts: &[Option<RowParts<'_>>],
    convert: fn(&RowParts<'_>) -> Result<T, ListFailure>,
) -> Result<[Option<T>; SCREEN_ROWS], ListFailure> {
    if parts.len() > SCREEN_ROWS {
        return Err(ListFailure::TooManyRows);
    }
    let mut out: [Option<T>; SCREEN_ROWS] = array::from_fn(|_| None);
    for (slot, part) in out.iter_mut().zip(parts) {
        *slot = part.as_ref().map(convert).transpose()?;
    }
    Ok(out)
}

/// An echoed row as read.
fn read_row(part: &RowParts<'_>) -> Result<Db2TransactionType, ListFailure> {
    Db2TransactionType::new(key(part.code)?, part.description.to_owned())
        .map_err(|_| ListFailure::InvalidRow)
}

/// An echoed row as shown.
fn screen_row(part: &RowParts<'_>) -> Result<ScreenRow, ListFailure> {
    Ok(ScreenRow {
        code: key(part.code)?,
        description: ScreenText::echoed(part.description)?,
    })
}

/// An echoed code or key, padded with spaces.
fn key(text: &str) -> Result<PicX<2>, ListFailure> {
    PicX::from_text(text).map_err(|_| ListFailure::InvalidKey)
}

/// An echoed screen row number.
fn row_number(number: Option<i64>) -> Result<Option<RowIndex>, ListFailure> {
    number
        .map(|number| RowIndex::from_number(number).ok_or(ListFailure::RowOutOfRange))
        .transpose()
}

/// The key of an endpoint 25 request. Every key without a label on the map arrives as ENTER
/// (COTRTLIC-014); F10 commits only when [`act`] finds an action pending and nothing changed
/// (COTRTLIC-036).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ListAid {
    /// ENTER: edit, then refresh or prompt (COTRTLIC-006, COTRTLIC-039, COTRTLIC-040).
    Enter,
    /// F10: commit the pending update or delete (COTRTLIC-036).
    Pf10,
}

/// The page move of an endpoint 24 request: F8 pages forward, F7 backward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// F8, the next page (COTRTLIC-007 to COTRTLIC-009).
    Forward,
    /// F7, the previous page (COTRTLIC-010 to COTRTLIC-012).
    Backward,
}

/// One row of an endpoint 25 request: the selection and the description as typed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct RowInput<'a> {
    /// `TRTSELI PIC X(1)`.
    pub selection: &'a str,
    /// `TRTYPDI PIC X(50)`.
    pub description: &'a str,
}

/// The map input of an endpoint 25 request (COTRTLIC-017). Rows after the seventh have no map
/// field and are not read; missing rows are blank.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ListInput<'a> {
    /// `TRTYPEI PIC X(2)`.
    pub type_filter: &'a str,
    /// `TRDESCI PIC X(50)`.
    pub desc_filter: &'a str,
    /// The seven rows, top to bottom.
    pub rows: &'a [RowInput<'a>],
}

/// The key COTRTLIC dispatches on, after `YYYY-STORE-PFKEY` and the F10 checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Enter,
    Pf7,
    Pf8,
    Pf10,
}

/// One selection as received, `WS-EDIT-SELECT PIC X(1)` (COTRTLIC-020).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Selection {
    /// A space or a low-value, `SELECT-BLANK`.
    Blank,
    /// `D`, `DELETE-REQUESTED-ON`.
    Delete,
    /// `U`, `UPDATE-REQUESTED-ON`.
    Update,
    /// Any other character, `d` and `u` included: codes are case-sensitive (COTRTLIC-021).
    Invalid(char),
}

impl Selection {
    /// `raw` moved into `PIC X(1)`.
    fn read(raw: &str) -> Self {
        ScreenText::<SELECTION_WIDTH>::read(raw)
            .as_str()
            .chars()
            .next()
            .map_or(Self::Blank, |c| match c {
                ' ' | '\0' => Self::Blank,
                'D' => Self::Delete,
                'U' => Self::Update,
                other => Self::Invalid(other),
            })
    }

    /// The character the screen shows, `None` for a blank selection.
    #[must_use]
    pub const fn shown(self) -> Option<char> {
        match self {
            Self::Blank => None,
            Self::Delete => Some('D'),
            Self::Update => Some('U'),
            Self::Invalid(c) => Some(c),
        }
    }
}

/// Whether a value differs from the state's (COTRTLIC-023, COTRTLIC-029, COTRTLIC-030).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Change {
    /// Equal to the state's.
    Unchanged,
    /// Different from the state's.
    Changed,
}

/// `WS-INPUT-FLAG`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum InputFlag {
    Ok,
    Error,
}

/// `WS-ROW-TRTSELECT-ERROR`: whether a row's selection is flagged in error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RowCheck {
    Clean,
    Flagged,
}

/// `WS-BAD-SELECTION-ACTION`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BadActions {
    No,
    Yes,
}

/// `WS-DATACHANGED-FLAG`: whether the last `U` row's description changed (COTRTLIC-024).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DataChange {
    NoChanges,
    Changed,
}

/// `WS-ARRAY-DESCRIPTION-FLGS`: the outcome of `1240-EDIT-ALPHANUM-REQD` on the last changed
/// `U` row description (COTRTLIC-025, COTRTLIC-026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum DescriptionCheck {
    Valid,
    NotOk,
    Blank,
}

/// `WS-RETURN-MSG`: one message, written over or only when none is set (COTRTLIC-033).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
struct ReturnMsg(Option<ListMessage>);

impl ReturnMsg {
    /// A `MOVE` or `SET` that writes whatever stands.
    const fn set(&mut self, message: ListMessage) {
        self.0 = Some(message);
    }

    /// A write guarded by `IF WS-RETURN-MSG-OFF`.
    const fn set_if_off(&mut self, message: ListMessage) {
        if self.0.is_none() {
            self.0 = Some(message);
        }
    }
}

/// The row a single `D` or `U` targets: its position and the code the screen showed there,
/// `WS-ROW-TR-CODE-IN(I-SELECTED)` (COTRTLIC-042).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Target {
    index: RowIndex,
    code: PicX<2>,
}

/// One row as `1100-RECEIVE-SCREEN` leaves it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Received {
    selection: Selection,
    /// `WS-ROW-TR-DESC-IN`: low-values for `*` or spaces, otherwise the text trimmed.
    description: ScreenText<DESCRIPTION_WIDTH>,
    /// `WS-ROW-TR-CODE-IN`: the code the previous response showed.
    code: PicX<2>,
}

/// The result of `1210-EDIT-ARRAY` with `1211-EDIT-ARRAY-DESC` (COTRTLIC-019 to COTRTLIC-026).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionEdit {
    received: [Received; SCREEN_ROWS],
    errors: [RowCheck; SCREEN_ROWS],
    actions: usize,
    deletes: usize,
    updates: usize,
    target: Option<Target>,
    change: Change,
    bad: BadActions,
    data_change: DataChange,
    check: DescriptionCheck,
    message: ReturnMsg,
    input: InputFlag,
}

impl SelectionEdit {
    /// The edit of a request with no selection at all: a first entry, which runs no edit.
    fn none() -> Self {
        Self {
            received: array::from_fn(|_| Received {
                selection: Selection::Blank,
                description: ScreenText::low_values(),
                code: PicX::spaces(),
            }),
            errors: [RowCheck::Clean; SCREEN_ROWS],
            actions: 0,
            deletes: 0,
            updates: 0,
            target: None,
            change: Change::Unchanged,
            bad: BadActions::No,
            data_change: DataChange::NoChanges,
            check: DescriptionCheck::Valid,
            message: ReturnMsg::default(),
            input: InputFlag::Ok,
        }
    }

    /// The selection received at `index`.
    #[must_use]
    pub fn selection(&self, index: RowIndex) -> Selection {
        self.received
            .get(index.index())
            .map_or(Selection::Blank, |row| row.selection)
    }

    /// `I-SELECTED`: the lowest row holding a `D` or `U` (COTRTLIC-020).
    #[must_use]
    pub fn selected(&self) -> Option<RowIndex> {
        self.target.as_ref().map(|target| target.index)
    }

    /// Whether the selected row differs from the state's (COTRTLIC-023).
    #[must_use]
    pub const fn change(&self) -> Change {
        self.change
    }

    /// The message the edit leaves, if any (COTRTLIC-021, COTRTLIC-022, COTRTLIC-024 to
    /// COTRTLIC-026).
    #[must_use]
    pub const fn message(&self) -> Option<ListMessage> {
        self.message.0
    }

    /// Whether the edit raised an input error (COTRTLIC-021, COTRTLIC-022, COTRTLIC-025,
    /// COTRTLIC-026); COTRTLIC-024's message is none.
    #[must_use]
    pub const fn is_input_error(&self) -> bool {
        matches!(self.input, InputFlag::Error)
    }

    /// `MOVE '1' TO WS-ROW-TRTSELECT-ERROR(I)` and `SET FLG-BAD-ACTIONS-SELECTED-YES`.
    fn flag(&mut self, index: RowIndex) {
        if let Some(check) = self.errors.get_mut(index.index()) {
            *check = RowCheck::Flagged;
        }
        self.bad = BadActions::Yes;
    }

    /// `1211-EDIT-ARRAY-DESC` of the `U` row description `received` against `baseline`, the
    /// description it is compared with (COTRTLIC-024 to COTRTLIC-026).
    fn edit_description(
        &mut self,
        received: &ScreenText<DESCRIPTION_WIDTH>,
        baseline: &ScreenText<DESCRIPTION_WIDTH>,
    ) {
        // Equal after upper-casing and trimming, with equal trimmed lengths: the ASCII case
        // fold keeps every length, so one byte-wise comparison decides both conditions.
        if received.trimmed().eq_ignore_ascii_case(baseline.trimmed()) {
            self.data_change = DataChange::NoChanges;
            self.message.set(ListMessage::NoChangeDetected);
            return;
        }
        self.data_change = DataChange::Changed;
        self.check = alphanumeric_required(received);
        match self.check {
            DescriptionCheck::Valid => {}
            DescriptionCheck::Blank => {
                self.input = InputFlag::Error;
                self.message.set_if_off(ListMessage::DescriptionRequired);
            }
            DescriptionCheck::NotOk => {
                self.input = InputFlag::Error;
                self.message
                    .set_if_off(ListMessage::DescriptionNotAlphanumeric);
            }
        }
    }

    /// The single valid action of the request without a bad one, as `2200` and `2300` test it:
    /// `WS-ONLY-1-VALID-ACTION AND FLG-BAD-ACTIONS-SELECTED-NO`.
    fn single_valid_action(&self) -> bool {
        self.deletes.saturating_add(self.updates) == 1 && self.bad == BadActions::No
    }

    /// `WS-ONLY-1-ACTION AND WS-ONLY-1-VALID-ACTION`, as `2500` tests it.
    fn only_one_action(&self) -> bool {
        self.actions == 1 && self.deletes.saturating_add(self.updates) == 1
    }

    /// `INITIALIZE WS-EDIT-SELECT-FLAGS` after a page move (COTRTLIC-007, COTRTLIC-011).
    fn clear_selections(&mut self) {
        for row in &mut self.received {
            row.selection = Selection::Blank;
        }
    }
}

/// `1240-EDIT-ALPHANUM-REQD` over 50 characters for `Transaction Desc` (COTRTLIC-025,
/// COTRTLIC-026): blank, or holding a character other than a letter, a digit or a space.
fn alphanumeric_required(field: &ScreenText<DESCRIPTION_WIDTH>) -> DescriptionCheck {
    if field.is_blank() {
        DescriptionCheck::Blank
    } else if field
        .as_str()
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ')
    {
        DescriptionCheck::Valid
    } else {
        DescriptionCheck::NotOk
    }
}

/// `1100-RECEIVE-SCREEN` over the rows (COTRTLIC-017, COTRTLIC-018).
///
/// A selection is read only where the previous response showed a row and left the selectors
/// enterable; elsewhere the 3270 sends nothing and it is a low-value. A description is read from
/// the request only at the row the previous response left enterable; elsewhere it is the
/// protected field returned as displayed. Then `*` or spaces read as low-values and any other
/// description is trimmed.
fn receive(rows: &[RowInput<'_>], state: &TranTypeListState) -> [Received; SCREEN_ROWS] {
    array::from_fn(|position| {
        let input = rows.get(position).copied().unwrap_or_default();
        let shown = state.screen.get(position).and_then(Option::as_ref);
        let selection = match (shown, state.selectors) {
            (Some(_), SelectorAccess::Open) => Selection::read(input.selection),
            _ => Selection::Blank,
        };
        let entered =
            RowIndex::new(position).is_some_and(|index| state.description_entry == Some(index));
        let field = if entered {
            ScreenText::read(input.description)
        } else {
            shown.map_or_else(ScreenText::low_values, |row| row.description.clone())
        };
        let asterisk = field
            .as_str()
            .strip_prefix('*')
            .is_some_and(|rest| rest.chars().all(|c| c == ' '));
        let description = if asterisk || field.as_str().chars().all(|c| c == ' ') {
            ScreenText::low_values()
        } else {
            ScreenText::read(field.trimmed())
        };
        Received {
            selection,
            description,
            code: shown.map_or_else(PicX::spaces, |row| row.code.clone()),
        }
    })
}

/// `1100-RECEIVE-SCREEN` and `1210-EDIT-ARRAY` (COTRTLIC-017 to COTRTLIC-026): the selections
/// and `U` row descriptions of `rows`, edited against the received `state`.
///
/// The selections are tallied and edited against the state as received, the previous page's
/// rows and selected row, never against the rows of a new filter. The branch of `1210` that
/// would clear them after a filter change is never taken: it tests the filter-changed flags,
/// which `0000-MAIN` has just initialised to spaces and which `1230` and `1220` set only after
/// it, so selections are never cleared here (E28, COTRTLIC-019).
#[must_use]
pub fn edit_selections(rows: &[RowInput<'_>], state: &TranTypeListState) -> SelectionEdit {
    let received = receive(rows, state);
    let count =
        |wanted: fn(Selection) -> bool| received.iter().filter(|row| wanted(row.selection)).count();
    let blanks = count(|selection| selection == Selection::Blank);
    let deletes = count(|selection| selection == Selection::Delete);
    let updates = count(|selection| selection == Selection::Update);
    let actions = SCREEN_ROWS.saturating_sub(blanks);
    let mut edit = SelectionEdit {
        actions,
        deletes,
        updates,
        ..SelectionEdit::none()
    };
    for (row, index) in received.iter().zip(RowIndex::ALL).rev() {
        match row.selection {
            Selection::Delete | Selection::Update => {
                edit.target = Some(Target {
                    index,
                    code: row.code.clone(),
                });
                if actions > 1 {
                    edit.flag(index);
                }
                if row.selection == Selection::Update {
                    edit.edit_description(&row.description, &state.row_description(index));
                }
            }
            Selection::Blank => {}
            Selection::Invalid(_) => {
                edit.input = InputFlag::Error;
                edit.flag(index);
                edit.message.set(ListMessage::InvalidActionCode);
            }
        }
    }
    edit.change = if edit.selected() == state.selected {
        Change::Unchanged
    } else {
        Change::Changed
    };
    if actions > 1 {
        edit.input = InputFlag::Error;
        edit.message.set(ListMessage::MoreThanOneAction);
    }
    edit.received = received;
    edit
}

/// The edited type filter, `WS-EDIT-TYPE-FLAG` (COTRTLIC-027, COTRTLIC-028).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeFilter {
    /// Empty, spaces, low-values or `00`: no predicate (`FLG-TYPEFILTER-BLANK`).
    Blank,
    /// Two digits: `tr_type` must equal them (`FLG-TYPEFILTER-ISVALID`).
    Valid(PicX<2>),
    /// Anything else (`FLG-TYPEFILTER-NOT-OK`).
    NotNumeric,
}

/// The result of `1220-EDIT-TYPECD` (COTRTLIC-027 to COTRTLIC-029).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeFilterEdit {
    input: ScreenText<TYPE_FILTER_WIDTH>,
    filter: TypeFilter,
    change: Change,
}

impl TypeFilterEdit {
    /// `WS-IN-TYPE-CD`, the filter as received, padded to 2 characters.
    #[must_use]
    pub fn input(&self) -> &str {
        self.input.as_str()
    }

    /// The edited filter.
    #[must_use]
    pub const fn filter(&self) -> &TypeFilter {
        &self.filter
    }

    /// Whether the filter differs from the state's (COTRTLIC-029).
    #[must_use]
    pub const fn change(&self) -> Change {
        self.change
    }
}

/// `1220-EDIT-TYPECD` of the received type filter `raw` against `state` (COTRTLIC-027 to
/// COTRTLIC-029).
///
/// Empty, spaces, low-values and `00` are blank, so `00` lists every row; two digits filter on
/// `tr_type`; anything else is `TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER`. The
/// filter is unchanged when it equals the state's, or is blank while the state's is blank or
/// `00`; a changed filter, even one in error, becomes the state's and resets its paging.
#[must_use]
pub fn edit_type_filter(raw: &str, state: &TranTypeListState) -> TypeFilterEdit {
    let input = ScreenText::<TYPE_FILTER_WIDTH>::read(raw);
    let filter = if input.is_blank() || input.as_str() == "00" {
        TypeFilter::Blank
    } else if input.as_str().bytes().all(|byte| byte.is_ascii_digit()) {
        TypeFilter::Valid(PicX::moved_from(input.as_str().as_bytes()))
    } else {
        TypeFilter::NotNumeric
    };
    let state_blank = state.type_filter.is_blank() || state.type_filter.as_str() == "00";
    let change = if input == state.type_filter || (filter == TypeFilter::Blank && state_blank) {
        Change::Unchanged
    } else {
        Change::Changed
    };
    TypeFilterEdit {
        input,
        filter,
        change,
    }
}

/// The edited description filter, `WS-EDIT-DESC-FLAG` (COTRTLIC-030).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DescFilter {
    /// Spaces or low-values: no predicate (`FLG-DESCFILTER-BLANK`).
    Blank,
    /// Any other text, with no character edit: `tr_description LIKE` the pattern, `%`, the text
    /// trimmed and `%`, with no escape character (`FLG-DESCFILTER-ISVALID`; COTRTLIC-005).
    Valid(String),
}

/// The result of `1230-EDIT-DESC` (COTRTLIC-030).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DescFilterEdit {
    input: ScreenText<DESCRIPTION_WIDTH>,
    filter: DescFilter,
    change: Change,
}

impl DescFilterEdit {
    /// `WS-IN-TYPE-DESC`, the filter as received, padded to 50 characters.
    #[must_use]
    pub fn input(&self) -> &str {
        self.input.as_str()
    }

    /// The edited filter.
    #[must_use]
    pub const fn filter(&self) -> &DescFilter {
        &self.filter
    }

    /// Whether the filter differs from the state's (COTRTLIC-030).
    #[must_use]
    pub const fn change(&self) -> Change {
        self.change
    }
}

/// `1230-EDIT-DESC` of the received description filter `raw` against `state` (COTRTLIC-005,
/// COTRTLIC-030).
///
/// Any non-blank text is valid and becomes the `LIKE` pattern `%`, the text trimmed, `%`; a `%`
/// or `_` the user types stays a wildcard. The filter is unchanged when it equals the state's in
/// all 50 characters, or is blank while the state's is blank; a changed filter becomes the
/// state's and resets its paging.
#[must_use]
pub fn edit_desc_filter(raw: &str, state: &TranTypeListState) -> DescFilterEdit {
    let input = ScreenText::<DESCRIPTION_WIDTH>::read(raw);
    let filter = if input.is_blank() {
        DescFilter::Blank
    } else {
        DescFilter::Valid(format!("%{}%", input.trimmed()))
    };
    let change = if input == state.desc_filter
        || (filter == DescFilter::Blank && state.desc_filter.is_blank())
    {
        Change::Unchanged
    } else {
        Change::Changed
    };
    DescFilterEdit {
        input,
        filter,
        change,
    }
}

/// A filter as the response reports it: blank, valid, or flagged in error by its own edit
/// (COTRTLIC-028) or by the cross edit (COTRTLIC-031).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterStatus {
    /// No predicate.
    Blank,
    /// A predicate in force.
    Valid,
    /// In error: red, with the focus (COTRTLIC-057).
    InError,
}

/// The predicates of a count or page read under the filters in force (COTRTLIC-005): `None`
/// for a filter that adds none.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FilterQuery {
    type_filter: Option<PicX<2>>,
    desc_pattern: Option<String>,
}

impl FilterQuery {
    /// `tr_type` must equal this code.
    #[must_use]
    pub const fn type_filter(&self) -> Option<&PicX<2>> {
        self.type_filter.as_ref()
    }

    /// `tr_description LIKE` this pattern, with no escape character.
    #[must_use]
    pub fn desc_pattern(&self) -> Option<&str> {
        self.desc_pattern.as_deref()
    }
}

/// The direction of a page read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReadDirection {
    /// `C-TR-TYPE-FORWARD`: `tr_type` at or above the start key, ascending (COTRTLIC-004).
    Forward,
    /// `C-TR-TYPE-BACKWARD`: `tr_type` below the start key, descending (COTRTLIC-011).
    Backward,
}

/// The page read a [`ReadStep`] asks for: `limit` rows at most, from `start` in `direction`,
/// under the filter predicates; served by the primary key `xtran_type` (Bounded Pagination).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReadQuery {
    direction: ReadDirection,
    start: PicX<2>,
    filters: FilterQuery,
    limit: i64,
}

impl ReadQuery {
    /// The cursor the read stands for.
    #[must_use]
    pub const fn direction(&self) -> ReadDirection {
        self.direction
    }

    /// `WS-START-KEY`: inclusive forward, exclusive backward.
    #[must_use]
    pub const fn start(&self) -> &PicX<2> {
        &self.start
    }

    /// The filter predicates (COTRTLIC-005).
    #[must_use]
    pub const fn filters(&self) -> &FilterQuery {
        &self.filters
    }

    /// The most rows to return: the page, plus one forward to detect the next page.
    #[must_use]
    pub const fn limit(&self) -> i64 {
        self.limit
    }
}

/// A store call that failed. It carries nothing: the text Db2 would add is `HALTED(H3)`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct StoreFailure;

/// The write a [`CommitStep`] asks the service to run inside one transaction (COTRTLIC-044,
/// COTRTLIC-049).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommitEffect {
    /// `UPDATE db2.transaction_type SET tr_description WHERE tr_type = code`: the received
    /// description with its leading and trailing spaces removed, in the case typed, padded with
    /// spaces to 50 bytes (COTRTLIC-044).
    Update {
        /// The code shown at the selected row.
        code: PicX<2>,
        /// The description to store, 50 bytes, which `Db2TransactionType::new` accepts.
        description: String,
    },
    /// `DELETE FROM db2.transaction_type WHERE tr_type = code` (COTRTLIC-049).
    Delete {
        /// The code shown at the selected row.
        code: PicX<2>,
    },
}

impl CommitEffect {
    /// The code the write targets.
    #[must_use]
    pub const fn code(&self) -> &PicX<2> {
        match self {
            Self::Update { code, .. } | Self::Delete { code } => code,
        }
    }
}

/// What the store reports for a [`CommitEffect`], the `SQLCODE` classes `9200` and `9300` test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CommitResult {
    /// `SQLCODE` 0: one row written and the transaction committed (`SYNCPOINT`).
    Done,
    /// `SQLCODE` +100: no row has the code (COTRTLIC-046, COTRTLIC-051).
    NotFound,
    /// `SQLCODE` −911, a deadlock or lock timeout (COTRTLIC-047).
    Deadlock,
    /// `SQLCODE` −532: categories still reference the type, `ON DELETE RESTRICT`
    /// (COTRTLIC-050).
    Restricted,
    /// Any other failure (COTRTLIC-048, COTRTLIC-051).
    Failed,
}

/// What a request committed (`FLG-UPDATE-COMPLETED`, `FLG-DELETED-YES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Committed {
    /// Nothing.
    Nothing,
    /// The selected row's description (COTRTLIC-045).
    Updated,
    /// The selected row (COTRTLIC-049).
    Deleted,
}

/// The emphasis of a shown row (COTRTLIC-056).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Emphasis {
    /// The default colour.
    Normal,
    /// The neutral colour of the row a single `D` or `U` highlights.
    Highlighted,
}

/// Whether a shown field is enterable for the next request (COTRTLIC-018, COTRTLIC-056).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldState {
    /// Protected.
    ReadOnly,
    /// Enterable.
    Enterable,
    /// Enterable, flagged in error: red, with the focus.
    Error,
}

/// One row of a response as `2200-SETUP-ARRAY-ATTRIBS` and `2300-SCREEN-ARRAY-INIT` show it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShownRow {
    code: PicX<2>,
    description: ScreenText<DESCRIPTION_WIDTH>,
    selection: Selection,
    emphasis: Emphasis,
    selector: FieldState,
    description_field: FieldState,
}

impl ShownRow {
    /// The code, `TRTTYPO`.
    #[must_use]
    pub const fn code(&self) -> &PicX<2> {
        &self.code
    }

    /// The description, `TRTYPDO`, without its trailing spaces: as read, or on the row of a
    /// single `U` the description received, `*` when blank (COTRTLIC-040, COTRTLIC-041).
    #[must_use]
    pub fn description(&self) -> &str {
        self.description.shown()
    }

    /// The selection, `TRTSELO`: as typed, blank after a commit or a page move.
    #[must_use]
    pub const fn selection(&self) -> Selection {
        self.selection
    }

    /// Whether the row is highlighted.
    #[must_use]
    pub const fn emphasis(&self) -> Emphasis {
        self.emphasis
    }

    /// The selector field: read-only after a filter error, in error after a selection error.
    #[must_use]
    pub const fn selector(&self) -> FieldState {
        self.selector
    }

    /// The description field: enterable only on the row of a single `U` whose update has not
    /// completed, in error when its edit failed (COTRTLIC-025, COTRTLIC-026).
    #[must_use]
    pub const fn description_field(&self) -> FieldState {
        self.description_field
    }
}

/// A filter as `2400-SETUP-SCREEN-ATTRS` shows it (COTRTLIC-027, COTRTLIC-057).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ShownFilter {
    text: String,
    status: FilterStatus,
}

impl ShownFilter {
    /// The text shown, without trailing spaces: as received while a selection is non-blank or
    /// the filter is valid or in error, empty otherwise.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the filter is blank, valid or in error.
    #[must_use]
    pub const fn status(&self) -> FilterStatus {
        self.status
    }
}

/// The response to a request: the screen COTRTLIC sends and the state it returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListActionOutcome {
    rows: [Option<ShownRow>; SCREEN_ROWS],
    type_filter: ShownFilter,
    desc_filter: ShownFilter,
    message: Option<ListMessage>,
    info: Option<InfoMessage>,
    status: StatusClass,
    committed: Committed,
    page_no: PageNumber,
    next_page: NextPage,
    state: TranTypeListState,
}

/// The response to an endpoint 24 request, which has the same shape as endpoint 25's.
pub type TranTypeListPage = ListActionOutcome;

impl ListActionOutcome {
    /// The seven row positions shown; `None` is an empty position.
    #[must_use]
    pub const fn rows(&self) -> &[Option<ShownRow>; SCREEN_ROWS] {
        &self.rows
    }

    /// The type filter shown.
    #[must_use]
    pub const fn type_filter(&self) -> &ShownFilter {
        &self.type_filter
    }

    /// The description filter shown.
    #[must_use]
    pub const fn desc_filter(&self) -> &ShownFilter {
        &self.desc_filter
    }

    /// The error-line message (COTRTLIC-033).
    #[must_use]
    pub const fn message(&self) -> Option<ListMessage> {
        self.message
    }

    /// The error-line text.
    #[must_use]
    pub fn message_text(&self) -> Option<String> {
        self.message.map(ListMessage::text)
    }

    /// The info-line message, absent beside `No records found for this search condition.`
    /// (COTRTLIC-003).
    #[must_use]
    pub const fn info(&self) -> Option<InfoMessage> {
        self.info
    }

    /// The status class.
    #[must_use]
    pub const fn status(&self) -> StatusClass {
        self.status
    }

    /// What the request committed.
    #[must_use]
    pub const fn committed(&self) -> Committed {
        self.committed
    }

    /// The page number shown, `PAGENOO`.
    #[must_use]
    pub const fn page_no(&self) -> PageNumber {
        self.page_no
    }

    /// Whether a next page exists for the page shown.
    #[must_use]
    pub const fn next_page(&self) -> NextPage {
        self.next_page
    }

    /// The state the client echoes with its next request.
    #[must_use]
    pub const fn state(&self) -> &TranTypeListState {
        &self.state
    }

    /// The state, by value.
    #[must_use]
    pub fn into_state(self) -> TranTypeListState {
        self.state
    }
}

/// The progress of one request: the next store call, or the response.
#[derive(Debug)]
pub enum Progress {
    /// Count the rows under the filters, then [`CountStep::finish`].
    Count(CountStep),
    /// Re-read the row a commit targets, then [`VerifyStep::finish`].
    Verify(VerifyStep),
    /// Run the write, then [`CommitStep::finish`].
    Commit(CommitStep),
    /// Read the page, then [`ReadStep::finish`], which gives the response.
    Read(ReadStep),
    /// The response.
    Done(Box<ListActionOutcome>),
}

impl Progress {
    fn done(outcome: ListActionOutcome) -> Self {
        Self::Done(Box::new(outcome))
    }
}

/// `9100-CHECK-FILTERS`: `COUNT(1)` of `db2.transaction_type` under [`CountStep::query`], with
/// no start key (COTRTLIC-031).
#[derive(Debug)]
pub struct CountStep {
    session: Box<Session>,
    query: FilterQuery,
}

impl CountStep {
    /// The predicates to count under.
    #[must_use]
    pub const fn query(&self) -> &FilterQuery {
        &self.query
    }

    /// `1290-CROSS-EDITS` with the count, then the request's dispatch (COTRTLIC-031,
    /// COTRTLIC-032).
    ///
    /// A count of zero is `No Records found for these filter conditions`: an input error, each
    /// valid filter flagged, every selector protected and no page read. A failed count is a store
    /// failure read as zero (halt candidate X-COTRTLIC-H2).
    #[must_use]
    pub fn finish(self, count: Result<i64, StoreFailure>) -> Progress {
        let mut session = *self.session;
        session.cross_edit(count);
        session.dispatch()
    }
}

/// The action a commit runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum CommitAction {
    Update,
    Delete,
}

/// The re-read of the row an F10 commit targets, by [`VerifyStep::code`], before any write: the
/// state is echoed and carries no authority (AAP 0.3.4.1).
#[derive(Debug)]
pub struct VerifyStep {
    session: Box<Session>,
    target: Target,
    action: CommitAction,
}

impl VerifyStep {
    /// The code shown at the selected row, which the commit targets (COTRTLIC-042).
    #[must_use]
    pub const fn code(&self) -> &PicX<2> {
        &self.target.code
    }

    /// The commit against `current`, the row as re-read.
    ///
    /// No row is the `SQLCODE` +100 outcome, with nothing written (COTRTLIC-046, COTRTLIC-051).
    /// For an update, the description edit is run again against the re-read description, so
    /// `No change detected with respect to database values.` compares with the database and an
    /// edit the state let through cannot reach it (COTRTLIC-024 to COTRTLIC-026); a failed edit
    /// is the input-error response, with nothing written (COTRTLIC-034).
    #[must_use]
    pub fn finish(self, current: Option<&Db2TransactionType>) -> Progress {
        let Self {
            session,
            target,
            action,
        } = self;
        let session = *session;
        match (action, current) {
            (CommitAction::Delete, None) => session.delete_outcome(CommitResult::NotFound),
            (CommitAction::Delete, Some(_)) => Progress::Commit(CommitStep {
                session: Box::new(session),
                effect: CommitEffect::Delete { code: target.code },
            }),
            (CommitAction::Update, None) => session.update_outcome(CommitResult::NotFound),
            (CommitAction::Update, Some(row)) => session.revalidate_update(target, row),
        }
    }
}

/// The write of an F10 commit, [`CommitStep::effect`], in the transaction of its re-read.
#[derive(Debug)]
pub struct CommitStep {
    session: Box<Session>,
    effect: CommitEffect,
}

impl CommitStep {
    /// The write to run.
    #[must_use]
    pub const fn effect(&self) -> &CommitEffect {
        &self.effect
    }

    /// The outcome of the write (COTRTLIC-045 to COTRTLIC-051).
    ///
    /// An update is followed by the page re-read from the state's first key, whatever its
    /// outcome; a delete is followed by no read, and a successful one resets the state as for a
    /// first entry (COTRTLIC-049).
    #[must_use]
    pub fn finish(self, result: CommitResult) -> Progress {
        let session = *self.session;
        match self.effect {
            CommitEffect::Update { .. } => session.update_outcome(result),
            CommitEffect::Delete { .. } => session.delete_outcome(result),
        }
    }
}

/// The page read of a request, [`ReadStep::query`], its last store call.
#[derive(Debug)]
pub struct ReadStep {
    session: Box<Session>,
    query: ReadQuery,
}

impl ReadStep {
    /// The read to run.
    #[must_use]
    pub const fn query(&self) -> &ReadQuery {
        &self.query
    }

    /// The response to the request, given the rows read: forward in ascending and backward in
    /// descending `tr_type` order, as the query returns them (COTRTLIC-004, COTRTLIC-011,
    /// COTRTLIC-012, COTRTLIC-052). Rows beyond the query's limit are not read.
    #[must_use]
    pub fn finish(self, rows: Result<Vec<Db2TransactionType>, StoreFailure>) -> ListActionOutcome {
        let mut session = *self.session;
        match self.query.direction {
            ReadDirection::Forward => session.read_forward(rows),
            ReadDirection::Backward => session.read_backward(rows),
        }
        if session.after_read == AfterRead::ClearSelections {
            session.edit.clear_selections();
        }
        session.send()
    }
}

/// `8000-READ-FORWARD` (COTRTLIC-002) for the screen load of endpoint 24: a first entry, which
/// ignores any state, reads page 1 from a blank key under no filter and shows
/// `Type U to update, D to delete any record` (COTRTLIC-002, COTRTLIC-003).
#[must_use]
pub fn load(page_size: PageSize) -> Progress {
    Session::first_entry(page_size).dispatch()
}

/// F7 or F8 of endpoint 24 under the filters `type_filter` and `desc_filter` as displayed, from
/// the echoed `state` (COTRTLIC-007 to COTRTLIC-013).
///
/// The request carries no selection, so every selection is blank; the filter edits run as on
/// ENTER (escalation candidate X-COTRTLIC-6). F8 reads the next page from the last key when one
/// exists and otherwise re-reads the page with `No more pages for these search conditions`,
/// then `No more pages to display`; F7 reads the previous page backward, and on page 1 re-reads
/// it with `No previous pages to display`.
#[must_use]
pub fn page(
    direction: Direction,
    type_filter: &str,
    desc_filter: &str,
    state: &TranTypeListState,
    page_size: PageSize,
) -> Progress {
    let key = match direction {
        Direction::Backward => Key::Pf7,
        Direction::Forward => Key::Pf8,
    };
    let input = ListInput {
        type_filter,
        desc_filter,
        rows: &[],
    };
    Session::edited(key, &input, state, page_size).after_edits()
}

/// ENTER or F10 of endpoint 25 with `input` over the echoed `state` (COTRTLIC-006,
/// COTRTLIC-017 to COTRTLIC-051).
///
/// The edits run in COTRTLIC's order, selections, then description filter, then type filter,
/// then the cross edit (COTRTLIC-019). An input error re-reads the page from the state's first
/// key, page 1 of a changed filter, unless a filter is itself in error (COTRTLIC-034). A single
/// valid `D` or `U` re-reads the page and becomes pending at its row position, with the prompt
/// only when the filters are unchanged (COTRTLIC-039 to COTRTLIC-041). F10 commits only when an
/// action is pending and the type filter, the description filter and the selected row are all
/// unchanged; otherwise it is processed exactly as ENTER (COTRTLIC-036 to COTRTLIC-038). A
/// commit targets the code shown at the selected row (COTRTLIC-042).
#[must_use]
pub fn act(
    aid: ListAid,
    input: &ListInput<'_>,
    state: &TranTypeListState,
    page_size: PageSize,
) -> Progress {
    let key = match aid {
        ListAid::Enter => Key::Enter,
        ListAid::Pf10 => Key::Pf10,
    };
    Session::edited(key, input, state, page_size).after_edits()
}

/// Whether the response is the first display, for which `2400` shows both filters empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Entry {
    First,
    Later,
}

/// Whether a page move clears the selections after its read, `INITIALIZE WS-EDIT-SELECT-FLAGS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum AfterRead {
    Keep,
    ClearSelections,
}

/// Whether the backward read found a full page (COTRTLIC-012).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum BackwardRead {
    Full,
    Short,
}

/// The dispatch of `0000-MAIN` once the edits and the F10 checks have run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Move {
    /// Re-read the page forward from the first key.
    Refresh,
    /// F8 with a next page.
    Forward,
    /// F7 off page 1.
    Backward,
    /// F10 with a single `D` or `U`.
    Commit(CommitAction),
}

/// One request of COTRTLIC in progress: the working storage of `0000-MAIN`.
#[derive(Debug, Clone)]
struct Session {
    key: Key,
    entry: Entry,
    state: TranTypeListState,
    edit: SelectionEdit,
    type_edit: TypeFilterEdit,
    desc_edit: DescFilterEdit,
    type_status: FilterStatus,
    desc_status: FilterStatus,
    /// `FLG-PROTECT-SELECT-ROWS` of this response.
    protect: SelectorAccess,
    message: ReturnMsg,
    input: InputFlag,
    severity: StatusClass,
    backward: BackwardRead,
    committed: Committed,
    page_size: PageSize,
    after_read: AfterRead,
}

impl Session {
    /// A first entry: `INITIALIZE WS-THIS-PROGCOMMAREA`, `CA-FIRST-PAGE`,
    /// `CA-LAST-PAGE-NOT-SHOWN` and ENTER, with no screen received and no edit (COTRTLIC-002).
    fn first_entry(page_size: PageSize) -> Self {
        let state = TranTypeListState::initial();
        let type_edit = edit_type_filter("", &state);
        let desc_edit = edit_desc_filter("", &state);
        Self {
            key: Key::Enter,
            entry: Entry::First,
            state,
            edit: SelectionEdit::none(),
            type_edit,
            desc_edit,
            type_status: FilterStatus::Blank,
            desc_status: FilterStatus::Blank,
            protect: SelectorAccess::Open,
            message: ReturnMsg::default(),
            input: InputFlag::Ok,
            severity: StatusClass::Ok,
            backward: BackwardRead::Full,
            committed: Committed::Nothing,
            page_size,
            after_read: AfterRead::Keep,
        }
    }

    /// A request that receives the screen: `1100-RECEIVE-SCREEN` and `1200-EDIT-INPUTS` up to the
    /// cross edit, in COTRTLIC's order (COTRTLIC-019).
    fn edited(
        key: Key,
        input: &ListInput<'_>,
        state: &TranTypeListState,
        page_size: PageSize,
    ) -> Self {
        let edit = edit_selections(input.rows, state);
        let desc_edit = edit_desc_filter(input.desc_filter, state);
        let type_edit = edit_type_filter(input.type_filter, state);
        let mut session = Self {
            key,
            entry: Entry::Later,
            state: state.clone(),
            message: edit.message,
            input: edit.input,
            edit,
            type_edit,
            desc_edit,
            type_status: FilterStatus::Blank,
            desc_status: FilterStatus::Blank,
            protect: SelectorAccess::Open,
            severity: StatusClass::Ok,
            backward: BackwardRead::Full,
            committed: Committed::Nothing,
            page_size,
            after_read: AfterRead::Keep,
        };
        session.apply_edits();
        session
    }

    /// The effects of the edits on the working storage and the state: the selected row
    /// (COTRTLIC-023), then the description filter and the type filter with their paging
    /// resets (COTRTLIC-028 to COTRTLIC-030).
    fn apply_edits(&mut self) {
        if self.edit.change == Change::Changed {
            self.state.selected = self.edit.selected();
        }
        self.desc_status = match self.desc_edit.filter {
            DescFilter::Blank => FilterStatus::Blank,
            DescFilter::Valid(_) => FilterStatus::Valid,
        };
        if self.desc_edit.change == Change::Changed {
            self.state.reset_paging();
            self.state.desc_filter.clone_from(&self.desc_edit.input);
        }
        self.type_status = match self.type_edit.filter {
            TypeFilter::Blank => FilterStatus::Blank,
            TypeFilter::Valid(_) => FilterStatus::Valid,
            TypeFilter::NotNumeric => {
                self.input = InputFlag::Error;
                self.protect = SelectorAccess::Protected;
                self.message.set(ListMessage::TypeFilterNotNumeric);
                FilterStatus::InError
            }
        };
        if self.type_edit.change == Change::Changed {
            self.state.reset_paging();
            self.state.type_filter.clone_from(&self.type_edit.input);
        }
    }

    /// `1290-CROSS-EDITS` performs `9100-CHECK-FILTERS` only when a filter is valid.
    fn after_edits(self) -> Progress {
        if self.type_status == FilterStatus::Valid || self.desc_status == FilterStatus::Valid {
            let query = self.filters();
            Progress::Count(CountStep {
                session: Box::new(self),
                query,
            })
        } else {
            self.dispatch()
        }
    }

    /// The predicates of the valid filters (COTRTLIC-005).
    fn filters(&self) -> FilterQuery {
        let type_filter = match (&self.type_edit.filter, self.type_status) {
            (TypeFilter::Valid(code), FilterStatus::Valid) => Some(code.clone()),
            _ => None,
        };
        let desc_pattern = match (&self.desc_edit.filter, self.desc_status) {
            (DescFilter::Valid(pattern), FilterStatus::Valid) => Some(pattern.clone()),
            _ => None,
        };
        FilterQuery {
            type_filter,
            desc_pattern,
        }
    }

    fn raise(&mut self, class: StatusClass) {
        self.severity = self.severity.max(class);
    }

    /// `1290-CROSS-EDITS` given the count of `9100-CHECK-FILTERS` (COTRTLIC-031, COTRTLIC-032).
    fn cross_edit(&mut self, count: Result<i64, StoreFailure>) {
        let matched = if let Ok(count) = count {
            count != 0
        } else {
            self.input = InputFlag::Error;
            self.raise(StatusClass::Store);
            self.message
                .set_if_off(ListMessage::Db2(Db2Action::CountFilters));
            // Halt candidate X-COTRTLIC-H2: the count stays at the zero `0000-MAIN` initialised
            // it to, as for a host variable Db2 leaves unassigned; no parity result.
            false
        };
        if !matched {
            self.input = InputFlag::Error;
            if self.type_status == FilterStatus::Valid {
                self.type_status = FilterStatus::InError;
            }
            if self.desc_status == FilterStatus::Valid {
                self.desc_status = FilterStatus::InError;
            }
            self.protect = SelectorAccess::Protected;
            self.message.set(ListMessage::NoRecordsForFilters);
        }
    }

    /// `0000-MAIN` after the edits: the key checks, then the `EVALUATE` (COTRTLIC-034 to
    /// COTRTLIC-038; `COTRTLIC.cbl:574-587,657-675,698-879`).
    fn dispatch(mut self) -> Progress {
        let unchanged = self.edit.change == Change::Unchanged
            && self.type_edit.change == Change::Unchanged
            && self.desc_edit.change == Change::Unchanged;
        if self.key == Key::Pf10 && !(self.state.pending.is_pending() && unchanged) {
            self.key = Key::Enter;
        }
        if self.key != Key::Pf8 {
            self.state.last_page = LastPage::NotShown;
        }
        if self.input == InputFlag::Error {
            return self.input_error();
        }
        let next = match self.key {
            Key::Pf8 if self.state.next_page == NextPage::Exists => Move::Forward,
            Key::Pf7 if !self.state.page_no.is_first() => Move::Backward,
            Key::Pf10 if self.edit.deletes > 0 => Move::Commit(CommitAction::Delete),
            Key::Pf10 if self.edit.updates > 0 => Move::Commit(CommitAction::Update),
            // ENTER with or without a selection, F7 on page 1, F8 on the last page and F10 with
            // nothing selected all re-read the displayed page.
            _ => Move::Refresh,
        };
        match next {
            Move::Refresh => self.refresh(),
            Move::Forward => {
                self.state.page_no = self.state.page_no.succ();
                self.after_read = AfterRead::ClearSelections;
                let start = self.state.last_key.clone();
                self.read(ReadDirection::Forward, start)
            }
            Move::Backward => {
                self.state.page_no = self.state.page_no.pred();
                self.after_read = AfterRead::ClearSelections;
                let start = self.state.first_key.clone();
                self.read(ReadDirection::Backward, start)
            }
            Move::Commit(action) => match self.edit.target.clone() {
                Some(target) => Progress::Verify(VerifyStep {
                    session: Box::new(self),
                    target,
                    action,
                }),
                None => self.refresh(),
            },
        }
    }

    /// `WHEN INPUT-ERROR`: the page re-read forward from the first key, unless a filter is in
    /// error (COTRTLIC-034).
    fn input_error(self) -> Progress {
        if self.type_status == FilterStatus::InError || self.desc_status == FilterStatus::InError {
            Progress::done(self.send())
        } else {
            self.refresh()
        }
    }

    /// The page re-read forward from `WS-CA-FIRST-TR-CODE`.
    fn refresh(self) -> Progress {
        let start = self.state.first_key.clone();
        self.read(ReadDirection::Forward, start)
    }

    fn read(self, direction: ReadDirection, start: PicX<2>) -> Progress {
        let query = ReadQuery {
            direction,
            start,
            filters: self.filters(),
            limit: self.page_size.limit(direction),
        };
        Progress::Read(ReadStep {
            session: Box::new(self),
            query,
        })
    }

    /// The update of `target` once its row is re-read as `current` (COTRTLIC-042, COTRTLIC-044).
    fn revalidate_update(mut self, target: Target, current: &Db2TransactionType) -> Progress {
        let received = self
            .edit
            .received
            .get(target.index.index())
            .map_or_else(ScreenText::low_values, |row| row.description.clone());
        // No input error reached the commit, so the only message standing is this row's
        // `No change detected` of the first edit, which the edit against the database replaces.
        self.edit.message = ReturnMsg::default();
        self.edit
            .edit_description(&received, &ScreenText::read(current.tr_description()));
        self.message = self.edit.message;
        self.input = self.edit.input;
        if self.input == InputFlag::Error {
            return self.input_error();
        }
        // `TRIM` into the host variable, padded to the 50 bytes `TR_DESCRIPTION` is always
        // written with (COTRTLIC-044). The text is at most 50 bytes: a changed description
        // passed the ASCII edit, and an unchanged one has the bytes of the row re-read.
        let trimmed = received.trimmed();
        let mut description = trimmed.to_owned();
        description.extend(iter::repeat_n(
            ' ',
            DESCRIPTION_WIDTH.saturating_sub(trimmed.len()),
        ));
        Progress::Commit(CommitStep {
            session: Box::new(self),
            effect: CommitEffect::Update {
                code: target.code,
                description,
            },
        })
    }

    /// `9200-UPDATE-RECORD`'s outcome, then the re-read forward from the first key
    /// (COTRTLIC-045 to COTRTLIC-048).
    fn update_outcome(mut self, result: CommitResult) -> Progress {
        let failure = match result {
            CommitResult::Done => None,
            CommitResult::NotFound => Some((Db2Action::UpdateNotFound, StatusClass::NotFound)),
            CommitResult::Deadlock => {
                self.input = InputFlag::Error;
                Some((Db2Action::UpdateDeadlock, StatusClass::Store))
            }
            CommitResult::Restricted | CommitResult::Failed => {
                Some((Db2Action::UpdateFailed, StatusClass::Store))
            }
        };
        match failure {
            None => {
                self.state.pending = self.state.pending.without_update();
                self.committed = Committed::Updated;
            }
            Some((action, class)) => {
                self.state.pending = self.state.pending.with_update();
                self.message.set_if_off(ListMessage::Db2(action));
                self.raise(class);
            }
        }
        self.refresh()
    }

    /// `9300-DELETE-RECORD`'s outcome, with no re-read (COTRTLIC-049 to COTRTLIC-051).
    fn delete_outcome(mut self, result: CommitResult) -> Progress {
        let failure = match result {
            CommitResult::Done => None,
            CommitResult::Restricted => {
                self.state.pending = self.state.pending.with_delete();
                Some((Db2Action::DeleteRestricted, StatusClass::Input))
            }
            CommitResult::NotFound => Some((Db2Action::DeleteFailed, StatusClass::NotFound)),
            CommitResult::Deadlock | CommitResult::Failed => {
                Some((Db2Action::DeleteFailed, StatusClass::Store))
            }
        };
        match failure {
            None => self.committed = Committed::Deleted,
            Some((action, class)) => {
                self.message.set_if_off(ListMessage::Db2(action));
                self.raise(class);
            }
        }
        Progress::done(self.send())
    }
}

impl Session {
    /// `8000-READ-FORWARD` over the rows the query returned (COTRTLIC-003, COTRTLIC-004,
    /// COTRTLIC-007, COTRTLIC-008, COTRTLIC-052).
    fn read_forward(&mut self, rows: Result<Vec<Db2TransactionType>, StoreFailure>) {
        self.state.rows = array::from_fn(|_| None);
        let Ok(rows) = rows else {
            self.message
                .set_if_off(ListMessage::Db2(Db2Action::ForwardOpen));
            self.raise(StatusClass::Store);
            return;
        };
        self.state.next_page = NextPage::Exists;
        let size = self.page_size.rows();
        let mut fetched = rows.into_iter();
        let mut filled = 0;
        for slot in self.state.rows.iter_mut().take(size) {
            let Some(row) = fetched.next() else {
                break;
            };
            if filled == 0 {
                self.state.first_key.clone_from(row.tr_type());
                if self.state.page_no == PageNumber::ZERO {
                    self.state.page_no = PageNumber::FIRST;
                }
            }
            self.state.last_key.clone_from(row.tr_type());
            *slot = Some(row);
            filled += 1;
        }
        let full = filled == size;
        // A full page keeps the last row's code as the last key unless the lookahead row exists,
        // whose code it then is, the first code of the next page. A short page ends on a fetch
        // that `INITIALIZE DCLTRANSACTION-TYPE` left at spaces, which becomes the last key.
        if let Some(next) = fetched.next().filter(|_| full) {
            self.state.last_key.clone_from(next.tr_type());
            return;
        }
        self.state.next_page = NextPage::NotExists;
        if !full {
            self.state.last_key = PicX::spaces();
        }
        if self.key == Key::Pf8 {
            self.message.set_if_off(ListMessage::NoMorePagesForSearch);
        }
        if !full && filled == 0 && self.state.page_no.is_first() {
            self.message.set(ListMessage::NoRecordsFound);
        }
    }

    /// `8100-READ-BACKWARDS` over the rows the query returned, highest code first: placed from
    /// the bottom row up, so the page reads ascending (COTRTLIC-011, COTRTLIC-012,
    /// COTRTLIC-052).
    fn read_backward(&mut self, rows: Result<Vec<Db2TransactionType>, StoreFailure>) {
        self.state.rows = array::from_fn(|_| None);
        self.state.last_key.clone_from(&self.state.first_key);
        self.state.next_page = NextPage::Exists;
        let Ok(rows) = rows else {
            self.message
                .set_if_off(ListMessage::Db2(Db2Action::BackwardOpen));
            self.raise(StatusClass::Store);
            return;
        };
        let size = self.page_size.rows();
        let mut filled = 0;
        let mut top = None;
        for (slot, row) in self.state.rows.iter_mut().take(size).rev().zip(rows) {
            top = Some(row.tr_type().clone());
            *slot = Some(row);
            filled += 1;
        }
        match top {
            Some(code) if filled == size => self.state.first_key = code,
            _ => {
                // The fetch past the start of the table is handled as a Db2 error
                // (X-COTRTLIC-3): the rows found stay at the bottom and the first key is kept.
                self.message
                    .set_if_off(ListMessage::Db2(Db2Action::BackwardFetch));
                self.backward = BackwardRead::Short;
            }
        }
    }

    /// `2000-SEND-MAP`: the response and the state returned with it.
    fn send(mut self) -> ListActionOutcome {
        let (rows, description_entry) = self.shown_rows();
        let type_filter = shown_filter(
            self.entry,
            self.edit.actions,
            self.type_status,
            &self.type_edit.input,
        );
        let desc_filter = shown_filter(
            self.entry,
            self.edit.actions,
            self.desc_status,
            &self.desc_edit.input,
        );
        let info = self.setup_message();
        if self.backward == BackwardRead::Short
            && self.message.0 == Some(ListMessage::Db2(Db2Action::BackwardFetch))
        {
            self.raise(StatusClass::Store);
        }
        if self.input == InputFlag::Error {
            self.raise(StatusClass::Input);
        }
        let page_no = self.state.page_no;
        let next_page = self.state.next_page;
        for (memo, row) in self.state.screen.iter_mut().zip(&rows) {
            *memo = row.as_ref().map(|row| ScreenRow {
                code: row.code.clone(),
                description: row.description.clone(),
            });
        }
        self.state.selectors = self.protect;
        self.state.description_entry = description_entry;
        if self.committed == Committed::Deleted {
            self.state.reset_after_delete();
        }
        ListActionOutcome {
            rows,
            type_filter,
            desc_filter,
            message: self.message.0,
            info,
            status: self.severity,
            committed: self.committed,
            page_no,
            next_page,
            state: self.state,
        }
    }

    /// `2200-SETUP-ARRAY-ATTRIBS` and `2300-SCREEN-ARRAY-INIT`: the rows shown, the pending flags
    /// a single valid action sets at a row that holds a type, and the row whose description the
    /// response leaves enterable (COTRTLIC-039 to COTRTLIC-041, COTRTLIC-043, COTRTLIC-045,
    /// COTRTLIC-049).
    fn shown_rows(&mut self) -> ([Option<ShownRow>; SCREEN_ROWS], Option<RowIndex>) {
        let single = self.edit.single_valid_action();
        let mut pending = self.state.pending;
        let mut entry = None;
        let mut shown: [Option<ShownRow>; SCREEN_ROWS] = array::from_fn(|_| None);
        let positions = self.state.rows.iter().zip(&self.edit.received);
        let marks = self.edit.errors.iter().zip(RowIndex::ALL);
        for ((slot, (row, received)), (check, index)) in shown.iter_mut().zip(positions).zip(marks)
        {
            let Some(row) = row else {
                continue;
            };
            let delete = received.selection == Selection::Delete && single;
            let update = received.selection == Selection::Update && single;
            let mut out = ShownRow {
                code: row.tr_type().clone(),
                description: ScreenText::read(row.tr_description()),
                selection: received.selection,
                emphasis: Emphasis::Normal,
                selector: FieldState::ReadOnly,
                description_field: FieldState::ReadOnly,
            };
            if self.protect == SelectorAccess::Open {
                out.selector = match check {
                    RowCheck::Flagged => FieldState::Error,
                    RowCheck::Clean => FieldState::Enterable,
                };
                if delete || update {
                    out.emphasis = Emphasis::Highlighted;
                }
                if update && self.committed != Committed::Updated {
                    out.description_field = match self.edit.check {
                        DescriptionCheck::Valid => FieldState::Enterable,
                        DescriptionCheck::NotOk | DescriptionCheck::Blank => FieldState::Error,
                    };
                    entry = Some(index);
                }
            }
            if delete {
                if self.committed == Committed::Deleted {
                    out.selection = Selection::Blank;
                } else {
                    pending = pending.with_delete();
                }
            }
            if update {
                if self.committed == Committed::Updated {
                    out.selection = Selection::Blank;
                } else {
                    pending = pending.with_update();
                }
                if self.edit.data_change == DataChange::Changed {
                    out.description = if self.edit.check == DescriptionCheck::Blank {
                        ScreenText::read("*")
                    } else {
                        received.description.clone()
                    };
                }
            }
            *slot = Some(out);
        }
        self.state.pending = pending;
        (shown, entry)
    }

    /// `2500-SETUP-MESSAGE`: the info line, the paging messages and the last-page indicator
    /// (COTRTLIC-002, COTRTLIC-007 to COTRTLIC-011, COTRTLIC-039, COTRTLIC-040, COTRTLIC-045,
    /// COTRTLIC-049).
    fn setup_message(&mut self) -> Option<InfoMessage> {
        let unchanged = self.type_edit.change == Change::Unchanged
            && self.desc_edit.change == Change::Unchanged;
        let enter = self.key == Key::Enter && self.edit.only_one_action();
        let info = if self.committed == Committed::Deleted {
            Some(InfoMessage::Deleted)
        } else if self.committed == Committed::Updated {
            Some(InfoMessage::Updated)
        } else if self.type_status == FilterStatus::InError
            || self.desc_status == FilterStatus::InError
        {
            None
        } else if enter && self.edit.deletes > 0 {
            unchanged.then_some(InfoMessage::DeletePrompt)
        } else if enter && self.edit.updates > 0 {
            unchanged.then_some(InfoMessage::UpdatePrompt)
        } else if self.key == Key::Pf7 && self.state.page_no.is_first() {
            self.message.set(ListMessage::NoPreviousPages);
            None
        } else if self.key == Key::Pf8
            && self.state.next_page == NextPage::NotExists
            && self.state.last_page == LastPage::Shown
        {
            self.message.set(ListMessage::NoMorePages);
            None
        } else {
            if self.key == Key::Pf8 && self.state.next_page == NextPage::NotExists {
                self.state.last_page = LastPage::Shown;
            }
            Some(InfoMessage::RecordActions)
        };
        info.filter(|_| self.message.0 != Some(ListMessage::NoRecordsFound))
    }
}

/// `2400-SETUP-SCREEN-ATTRS` for one filter: empty on the first display; otherwise the value
/// received while a selection is non-blank or the filter is valid or in error, and empty for a
/// blank filter, `00` included (COTRTLIC-027, COTRTLIC-057).
fn shown_filter<const N: usize>(
    entry: Entry,
    actions: usize,
    status: FilterStatus,
    input: &ScreenText<N>,
) -> ShownFilter {
    let text = if entry == Entry::Later && (actions > 0 || status != FilterStatus::Blank) {
        input.shown().to_owned()
    } else {
        String::new()
    };
    ShownFilter { text, status }
}

#[cfg(test)]
mod tests {
    use copybook::PicX;

    use super::{
        Change, CommitEffect, CommitResult, Committed, Db2Action, DescFilter, Direction, Emphasis,
        FieldState, FilterQuery, FilterStatus, InfoMessage, LastPage, ListActionOutcome, ListAid,
        ListFailure, ListInput, ListMessage, NextPage, PageNumber, PageSize, PendingAction,
        Progress, ReadDirection, ReadQuery, RowIndex, RowInput, RowParts, Selection,
        SelectorAccess, ShownRow, StateParts, StatusClass, StoreFailure, TranTypeListState,
        TypeFilter, act, edit_desc_filter, edit_selections, edit_type_filter, load, page,
    };
    use crate::records::db2_transaction_type::Db2TransactionType;

    type TestResult = Result<(), String>;

    /// The seed of `DB2LTTYP.ctl:17-23`.
    const SEED: [(&str, &str); 7] = [
        ("01", "PURCHASE"),
        ("02", "PAYMENT"),
        ("03", "CREDIT"),
        ("04", "AUTHORIZATION"),
        ("05", "REFUND"),
        ("06", "REVERAL"),
        ("07", "ADJUSTMENT"),
    ];

    fn code(text: &str) -> Result<PicX<2>, String> {
        PicX::from_text(text).map_err(|error| format!("{error:?}"))
    }

    fn row(code_text: &str, description: &str) -> Result<Db2TransactionType, String> {
        Db2TransactionType::new(code(code_text)?, description.to_owned())
            .map_err(|error| format!("{error:?}"))
    }

    /// A failure the fake store injects.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    enum Fault {
        #[default]
        None,
        Count,
        Read,
        Commit(CommitResult),
    }

    /// An in-memory `db2.transaction_type`: the calls `api::services::cotrtlic` makes.
    #[derive(Debug, Default)]
    struct Table {
        rows: Vec<Db2TransactionType>,
        parents: Vec<PicX<2>>,
        fault: Fault,
    }

    impl Table {
        /// The seed, every type but `07` with categories.
        fn seed() -> Result<Self, String> {
            let rows = SEED
                .iter()
                .map(|(code, description)| row(code, description))
                .collect::<Result<Vec<_>, _>>()?;
            let parents = rows
                .iter()
                .take(6)
                .map(|row| row.tr_type().clone())
                .collect();
            Ok(Self {
                rows,
                parents,
                fault: Fault::None,
            })
        }

        /// Types `01` to `count`, described `TYPE n`, none with categories.
        fn numbered(count: u8) -> Result<Self, String> {
            let rows = (1..=count)
                .map(|n| row(&format!("{n:02}"), &format!("TYPE {n}")))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Self {
                rows,
                ..Self::default()
            })
        }

        fn matches(row: &Db2TransactionType, filters: &FilterQuery) -> bool {
            let chars: Vec<char> = row.tr_description().chars().collect();
            filters
                .type_filter()
                .is_none_or(|code| row.tr_type() == code)
                && filters.desc_pattern().is_none_or(|pattern| {
                    let pattern: Vec<char> = pattern.trim_matches(' ').chars().collect();
                    like(&chars, &pattern)
                })
        }

        fn count(&self, filters: &FilterQuery) -> Result<i64, StoreFailure> {
            if self.fault == Fault::Count {
                return Err(StoreFailure);
            }
            let count = self
                .rows
                .iter()
                .filter(|row| Self::matches(row, filters))
                .count();
            Ok(i64::try_from(count).unwrap_or(i64::MAX))
        }

        fn read(&self, query: &ReadQuery) -> Result<Vec<Db2TransactionType>, StoreFailure> {
            if self.fault == Fault::Read {
                return Err(StoreFailure);
            }
            let limit = usize::try_from(query.limit()).unwrap_or(0);
            let mut rows: Vec<Db2TransactionType> = self
                .rows
                .iter()
                .filter(|row| Self::matches(row, query.filters()))
                .filter(|row| match query.direction() {
                    ReadDirection::Forward => row.tr_type() >= query.start(),
                    ReadDirection::Backward => row.tr_type() < query.start(),
                })
                .cloned()
                .collect();
            rows.sort_by(|a, b| a.tr_type().cmp(b.tr_type()));
            if query.direction() == ReadDirection::Backward {
                rows.reverse();
            }
            rows.truncate(limit);
            Ok(rows)
        }

        fn find(&self, code: &PicX<2>) -> Option<Db2TransactionType> {
            self.rows.iter().find(|row| row.tr_type() == code).cloned()
        }

        fn commit(&mut self, effect: &CommitEffect) -> CommitResult {
            if let Fault::Commit(result) = self.fault {
                return result;
            }
            let Some(position) = self
                .rows
                .iter()
                .position(|row| row.tr_type() == effect.code())
            else {
                return CommitResult::NotFound;
            };
            match effect {
                CommitEffect::Update { code, description } => {
                    match Db2TransactionType::new(code.clone(), description.clone()) {
                        Ok(updated) => {
                            self.rows.push(updated);
                            self.rows.swap_remove(position);
                            CommitResult::Done
                        }
                        Err(_) => CommitResult::Failed,
                    }
                }
                CommitEffect::Delete { code } if self.parents.contains(code) => {
                    CommitResult::Restricted
                }
                CommitEffect::Delete { .. } => {
                    self.rows.remove(position);
                    CommitResult::Done
                }
            }
        }

        fn remove(&mut self, code_text: &str) -> TestResult {
            let code = code(code_text)?;
            self.rows.retain(|row| row.tr_type() != &code);
            Ok(())
        }

        fn description(&self, code_text: &str) -> Result<String, String> {
            self.find(&code(code_text)?)
                .map(|row| row.tr_description().to_owned())
                .ok_or_else(|| format!("no type {code_text}"))
        }
    }

    /// SQL `LIKE` with `%` and `_` and no escape character.
    fn like(text: &[char], pattern: &[char]) -> bool {
        match pattern.split_first() {
            None => text.is_empty(),
            Some(('%', rest)) => {
                (0..=text.len()).any(|skip| text.get(skip..).is_some_and(|tail| like(tail, rest)))
            }
            Some(('_', rest)) => text.split_first().is_some_and(|(_, tail)| like(tail, rest)),
            Some((wanted, rest)) => text
                .split_first()
                .is_some_and(|(head, tail)| head == wanted && like(tail, rest)),
        }
    }

    /// The service's loop: each store call the progress names, then the response.
    fn run(table: &mut Table, mut progress: Progress) -> ListActionOutcome {
        loop {
            progress = match progress {
                Progress::Count(step) => {
                    let count = table.count(step.query());
                    step.finish(count)
                }
                Progress::Verify(step) => {
                    let current = table.find(step.code());
                    step.finish(current.as_ref())
                }
                Progress::Commit(step) => {
                    let result = table.commit(step.effect());
                    step.finish(result)
                }
                Progress::Read(step) => {
                    let rows = table.read(step.query());
                    return step.finish(rows);
                }
                Progress::Done(outcome) => return *outcome,
            };
        }
    }

    fn loaded(table: &mut Table) -> ListActionOutcome {
        run(table, load(PageSize::DEFAULT))
    }

    /// Seven rows, blank but for `(row number, selection, description)`.
    fn rows_with(entries: &[(usize, &'static str, &'static str)]) -> Vec<RowInput<'static>> {
        (1..=7)
            .map(|number| {
                entries
                    .iter()
                    .find(|(wanted, ..)| *wanted == number)
                    .map_or_else(RowInput::default, |(_, selection, description)| RowInput {
                        selection,
                        description,
                    })
            })
            .collect()
    }

    fn request(
        table: &mut Table,
        aid: ListAid,
        state: &TranTypeListState,
        filters: (&str, &str),
        rows: &[RowInput<'_>],
    ) -> ListActionOutcome {
        let input = ListInput {
            type_filter: filters.0,
            desc_filter: filters.1,
            rows,
        };
        run(table, act(aid, &input, state, PageSize::DEFAULT))
    }

    fn enter(
        table: &mut Table,
        state: &TranTypeListState,
        filters: (&str, &str),
        rows: &[RowInput<'_>],
    ) -> ListActionOutcome {
        request(table, ListAid::Enter, state, filters, rows)
    }

    fn f10(
        table: &mut Table,
        state: &TranTypeListState,
        filters: (&str, &str),
        rows: &[RowInput<'_>],
    ) -> ListActionOutcome {
        request(table, ListAid::Pf10, state, filters, rows)
    }

    fn paged(
        table: &mut Table,
        direction: Direction,
        state: &TranTypeListState,
    ) -> ListActionOutcome {
        let progress = page(
            direction,
            state.type_filter(),
            state.desc_filter(),
            state,
            PageSize::DEFAULT,
        );
        run(table, progress)
    }

    /// The codes shown, `""` at an empty position.
    fn codes(outcome: &ListActionOutcome) -> Vec<String> {
        outcome
            .rows()
            .iter()
            .map(|row| {
                row.as_ref()
                    .map_or_else(String::new, |row| row.code().to_text())
            })
            .collect()
    }

    fn shown(outcome: &ListActionOutcome, number: i64) -> Result<&ShownRow, String> {
        RowIndex::from_number(number)
            .and_then(|index| outcome.rows().get(index.index()))
            .and_then(Option::as_ref)
            .ok_or_else(|| format!("no row {number}"))
    }

    fn row_number(number: i64) -> Result<RowIndex, String> {
        RowIndex::from_number(number).ok_or_else(|| format!("no row number {number}"))
    }

    fn texts(codes: &[&str]) -> Vec<String> {
        codes.iter().map(|code| (*code).to_owned()).collect()
    }

    fn db2_message(outcome: &ListActionOutcome) -> String {
        outcome.message_text().unwrap_or_default()
    }

    // audit: COTRTLIC-002
    // audit: COTRTLIC-004
    #[test]
    fn load_reads_page_one_from_a_blank_key_with_no_filter() -> TestResult {
        let progress = load(PageSize::DEFAULT);
        let Progress::Read(step) = &progress else {
            return Err("a first entry reads".to_owned());
        };
        assert_eq!(step.query().direction(), ReadDirection::Forward);
        assert!(step.query().start().is_spaces());
        assert_eq!(step.query().limit(), 8);
        assert_eq!(step.query().filters().type_filter(), None);
        assert_eq!(step.query().filters().desc_pattern(), None);

        let mut table = Table::numbered(9)?;
        let outcome = run(&mut table, progress);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!(outcome.page_no(), PageNumber::FIRST);
        assert_eq!(outcome.next_page(), NextPage::Exists);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        assert_eq!(outcome.message(), None);
        assert_eq!(outcome.status(), StatusClass::Ok);
        assert_eq!(outcome.type_filter().text(), "");
        assert_eq!(outcome.desc_filter().status(), FilterStatus::Blank);
        let state = outcome.state();
        assert_eq!(state.first_key(), &code("01")?);
        assert_eq!(state.last_key(), &code("08")?);
        assert_eq!(state.last_page(), LastPage::NotShown);
        assert_eq!(state.pending(), PendingAction::None);
        assert_eq!(state.selectors(), SelectorAccess::Open);
        assert!(state.screen_rows().iter().all(Option::is_some));

        // Exactly seven rows: no next page, and the seventh code is the last key.
        let outcome = loaded(&mut Table::numbered(7)?);
        assert_eq!(outcome.next_page(), NextPage::NotExists);
        assert_eq!(outcome.state().last_key(), &code("07")?);
        // Fewer: the last key is spaces.
        let outcome = loaded(&mut Table::numbered(2)?);
        assert_eq!(codes(&outcome), texts(&["01", "02", "", "", "", "", ""]));
        assert!(outcome.state().last_key().is_spaces());
        Ok(())
    }

    // audit: COTRTLIC-003
    #[test]
    fn page_one_with_no_row_shows_no_records_found_and_no_info() {
        let outcome = loaded(&mut Table::default());
        assert_eq!(outcome.message(), Some(ListMessage::NoRecordsFound));
        assert_eq!(
            outcome.message_text().as_deref(),
            Some("No records found for this search condition.")
        );
        assert_eq!(outcome.info(), None);
        assert_eq!(outcome.status(), StatusClass::Ok);
        assert!(outcome.rows().iter().all(Option::is_none));
        assert_eq!(outcome.next_page(), NextPage::NotExists);
    }

    // audit: COTRTLIC-006
    #[test]
    fn enter_with_unchanged_filters_and_no_selection_refreshes_the_page() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        table.remove("03")?;
        let outcome = enter(&mut table, &state, ("", ""), &[]);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "04", "05", "06", "07", ""])
        );
        assert_eq!(outcome.page_no(), PageNumber::FIRST);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(outcome.status(), StatusClass::Ok);
        Ok(())
    }

    // audit: COTRTLIC-029
    // audit: COTRTLIC-035
    // audit: COTRTLIC-036
    // audit: COTRTLIC-037
    #[test]
    fn a_changed_type_filter_with_blank_selections_reads_page_one_of_it() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        for aid in [ListAid::Enter, ListAid::Pf10] {
            let outcome = request(&mut table, aid, &state, ("03", ""), &[]);
            assert_eq!(codes(&outcome), texts(&["03", "", "", "", "", "", ""]));
            assert_eq!(outcome.page_no(), PageNumber::FIRST);
            assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
            assert_eq!(outcome.message(), None);
            assert_eq!(outcome.committed(), Committed::Nothing);
            assert_eq!(outcome.type_filter().text(), "03");
            assert_eq!(outcome.type_filter().status(), FilterStatus::Valid);
            assert_eq!(outcome.state().type_filter(), "03");
            assert_eq!(outcome.state().first_key(), &code("03")?);
        }
        assert_eq!(table.rows.len(), 7);
        Ok(())
    }

    // audit: COTRTLIC-030
    // audit: COTRTLIC-035
    // audit: COTRTLIC-036
    // audit: COTRTLIC-037
    #[test]
    fn a_changed_description_filter_with_blank_selections_reads_page_one_of_it() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        for aid in [ListAid::Enter, ListAid::Pf10] {
            let outcome = request(&mut table, aid, &state, ("", "R"), &[]);
            assert_eq!(
                codes(&outcome),
                texts(&["01", "03", "04", "05", "06", "", ""])
            );
            assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
            assert_eq!(outcome.desc_filter().text(), "R");
            assert_eq!(outcome.desc_filter().status(), FilterStatus::Valid);
            assert_eq!(outcome.state().desc_filter().trim_end(), "R");
            assert_eq!(outcome.committed(), Committed::Nothing);
        }
        // The pattern is the trimmed filter between `%`, case-sensitive.
        let edit = edit_desc_filter("  pay  ", &state);
        assert_eq!(edit.change(), Change::Changed);
        assert_eq!(edit.input().len(), 50);
        let outcome = enter(&mut table, &state, ("", "  PAY  "), &[]);
        assert_eq!(codes(&outcome), texts(&["02", "", "", "", "", "", ""]));
        Ok(())
    }

    // audit: COTRTLIC-019
    // audit: COTRTLIC-029
    // audit: COTRTLIC-041
    // audit: COTRTLIC-042
    // audit: COTRTLIC-049
    #[test]
    fn a_delete_kept_across_a_changed_type_filter_is_committed_on_the_new_row() -> TestResult {
        let mut table = Table::seed()?;
        table.parents.retain(|parent| parent.to_text() != "05");
        let state = loaded(&mut table).into_state();

        // `D` on row 1 (type 01) with type filter 05: kept at row 1, now type 05.
        let outcome = enter(&mut table, &state, ("05", ""), &rows_with(&[(1, "D", "")]));
        assert_eq!(codes(&outcome), texts(&["05", "", "", "", "", "", ""]));
        assert_eq!(shown(&outcome, 1)?.selection(), Selection::Delete);
        assert_eq!(shown(&outcome, 1)?.emphasis(), Emphasis::Highlighted);
        assert_eq!(outcome.info(), None);
        assert_eq!(outcome.message(), None);
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        assert_eq!(outcome.state().selected(), Some(row_number(1)?));
        let state = outcome.into_state();

        let outcome = f10(&mut table, &state, ("05", ""), &rows_with(&[(1, "D", "")]));
        assert_eq!(outcome.committed(), Committed::Deleted);
        assert_eq!(outcome.info(), Some(InfoMessage::Deleted));
        assert_eq!(outcome.status(), StatusClass::Ok);
        assert!(table.find(&code("05")?).is_none());
        assert!(table.find(&code("01")?).is_some());
        // No re-read: the deleted row is still listed, its selection blank.
        assert_eq!(codes(&outcome), texts(&["05", "", "", "", "", "", ""]));
        assert_eq!(shown(&outcome, 1)?.selection(), Selection::Blank);
        let state = outcome.state();
        assert_eq!(state.pending(), PendingAction::None);
        assert_eq!(state.page_no(), PageNumber::FIRST);
        assert_eq!(state.type_filter(), "  ");
        assert_eq!(state.selected(), None);
        assert!(
            state
                .rows()
                .iter()
                .flatten()
                .all(|row| row.tr_type().is_spaces())
        );
        assert!(state.screen_rows().first().is_some_and(Option::is_some));
        Ok(())
    }

    // audit: COTRTLIC-018
    // audit: COTRTLIC-030
    // audit: COTRTLIC-040
    // audit: COTRTLIC-041
    // audit: COTRTLIC-042
    // audit: COTRTLIC-044
    // audit: COTRTLIC-045
    #[test]
    fn an_edited_description_moves_to_the_new_row_and_f10_updates_that_row() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let outcome = enter(&mut table, &state, ("", ""), &rows_with(&[(2, "U", "")]));
        assert_eq!(outcome.info(), Some(InfoMessage::UpdatePrompt));
        assert_eq!(outcome.state().description_entry(), Some(row_number(2)?));
        assert_eq!(
            shown(&outcome, 2)?.description_field(),
            FieldState::Enterable
        );
        let state = outcome.into_state();

        let edited = rows_with(&[(2, "U", "RENAMED CREDIT")]);
        let outcome = enter(&mut table, &state, ("", "R"), &edited);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "03", "04", "05", "06", "", ""])
        );
        assert_eq!(shown(&outcome, 2)?.description(), "RENAMED CREDIT");
        assert_eq!(outcome.info(), None);
        assert_eq!(outcome.message(), None);
        assert_eq!(outcome.state().pending(), PendingAction::Update);
        let state = outcome.into_state();

        let outcome = f10(&mut table, &state, ("", "R"), &edited);
        assert_eq!(outcome.committed(), Committed::Updated);
        assert_eq!(outcome.info(), Some(InfoMessage::Updated));
        assert_eq!(outcome.message(), None);
        assert_eq!(
            table.description("03")?,
            format!("{:<50}", "RENAMED CREDIT")
        );
        assert_eq!(table.description("02")?, "PAYMENT");
        assert_eq!(shown(&outcome, 2)?.selection(), Selection::Blank);
        assert_eq!(shown(&outcome, 2)?.description(), "RENAMED CREDIT");
        assert_eq!(
            shown(&outcome, 2)?.description_field(),
            FieldState::ReadOnly
        );
        assert_eq!(outcome.state().pending(), PendingAction::None);
        assert_eq!(outcome.state().description_entry(), None);
        Ok(())
    }

    // audit: COTRTLIC-038
    #[test]
    fn f10_with_a_pending_action_and_a_changed_filter_commits_nothing() -> TestResult {
        let mut table = Table::seed()?;
        table.parents.clear();
        let state = loaded(&mut table).into_state();
        let delete = rows_with(&[(3, "D", "")]);
        let state = enter(&mut table, &state, ("", ""), &delete).into_state();
        assert_eq!(state.pending(), PendingAction::Delete);

        let outcome = f10(&mut table, &state, ("", "R"), &delete);
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(table.rows.len(), 7);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "03", "04", "05", "06", "", ""])
        );
        assert_eq!(shown(&outcome, 3)?.selection(), Selection::Delete);
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        assert_eq!(outcome.info(), None);
        assert_eq!(outcome.status(), StatusClass::Ok);

        // A position the new page leaves empty is not returned and records nothing.
        let state = loaded(&mut table).into_state();
        let outcome = enter(&mut table, &state, ("02", ""), &rows_with(&[(4, "D", "")]));
        assert_eq!(codes(&outcome), texts(&["02", "", "", "", "", "", ""]));
        assert_eq!(outcome.state().pending(), PendingAction::None);
        Ok(())
    }

    // audit: COTRTLIC-021
    // audit: COTRTLIC-034
    #[test]
    fn an_invalid_code_with_a_changed_filter_re_reads_page_one_of_the_new_filter() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let outcome = enter(&mut table, &state, ("", "R"), &rows_with(&[(2, "X", "")]));
        assert_eq!(outcome.message(), Some(ListMessage::InvalidActionCode));
        assert_eq!(outcome.status(), StatusClass::Input);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "03", "04", "05", "06", "", ""])
        );
        assert_eq!(shown(&outcome, 2)?.selection(), Selection::Invalid('X'));
        assert_eq!(shown(&outcome, 2)?.selector(), FieldState::Error);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        assert_eq!(outcome.state().pending(), PendingAction::None);
        assert_eq!(outcome.state().page_no(), PageNumber::FIRST);
        Ok(())
    }

    // audit: COTRTLIC-020
    // audit: COTRTLIC-021
    #[test]
    fn an_invalid_action_code_is_an_input_error_and_records_nothing() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        for code in ["d", "u", "X", "*"] {
            let outcome = enter(&mut table, &state, ("", ""), &rows_with(&[(1, code, "")]));
            assert_eq!(
                outcome.message_text().as_deref(),
                Some("Action code selected is invalid")
            );
            assert_eq!(outcome.status().http_status(), 400);
            assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
            assert_eq!(outcome.state().pending(), PendingAction::None);
            assert_eq!(codes(&outcome).len(), 7);
        }
        Ok(())
    }

    // audit: COTRTLIC-020
    // audit: COTRTLIC-022
    // audit: COTRTLIC-023
    #[test]
    fn two_selected_rows_are_both_flagged_and_record_nothing() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let rows = rows_with(&[(1, "D", ""), (3, "U", "")]);
        let edit = edit_selections(&rows, &state);
        assert_eq!(edit.selected(), Some(row_number(1)?));
        assert_eq!(edit.selection(row_number(3)?), Selection::Update);
        assert_eq!(edit.change(), Change::Changed);
        assert!(edit.is_input_error());
        assert_eq!(edit.message(), Some(ListMessage::MoreThanOneAction));

        let outcome = enter(&mut table, &state, ("", ""), &rows);
        assert_eq!(
            outcome.message_text().as_deref(),
            Some("Please select only 1 action")
        );
        assert_eq!(outcome.status(), StatusClass::Input);
        assert_eq!(shown(&outcome, 1)?.selector(), FieldState::Error);
        assert_eq!(shown(&outcome, 3)?.selector(), FieldState::Error);
        assert_eq!(shown(&outcome, 2)?.selector(), FieldState::Enterable);
        assert_eq!(shown(&outcome, 1)?.emphasis(), Emphasis::Normal);
        assert_eq!(outcome.state().pending(), PendingAction::None);
        assert_eq!(outcome.state().selected(), Some(row_number(1)?));
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        Ok(())
    }

    // audit: COTRTLIC-024
    // audit: COTRTLIC-040
    // audit: COTRTLIC-045
    #[test]
    fn an_unchanged_description_prompts_and_f10_still_rewrites_the_row() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        // The description is protected on the first `U` request: the edit is not received.
        let first = rows_with(&[(1, "U", "SOMETHING ELSE")]);
        let outcome = enter(&mut table, &state, ("", ""), &first);
        assert_eq!(
            outcome.message_text().as_deref(),
            Some("No change detected with respect to database values.")
        );
        assert_eq!(outcome.status(), StatusClass::Ok);
        assert_eq!(outcome.info(), Some(InfoMessage::UpdatePrompt));
        assert_eq!(
            outcome.info().map(InfoMessage::text),
            Some("Update HIGHLIGHTED row. Press F10 to save")
        );
        assert_eq!(shown(&outcome, 1)?.description(), "PURCHASE");
        assert_eq!(shown(&outcome, 1)?.emphasis(), Emphasis::Highlighted);
        assert_eq!(outcome.state().pending(), PendingAction::Update);
        let state = outcome.into_state();

        let outcome = f10(
            &mut table,
            &state,
            ("", ""),
            &rows_with(&[(1, "U", "purchase")]),
        );
        assert_eq!(outcome.committed(), Committed::Updated);
        assert_eq!(outcome.message(), Some(ListMessage::NoChangeDetected));
        assert_eq!(outcome.info(), Some(InfoMessage::Updated));
        assert_eq!(outcome.status(), StatusClass::Ok);
        assert_eq!(table.description("01")?, format!("{:<50}", "purchase"));
        assert_eq!(shown(&outcome, 1)?.description(), "purchase");
        Ok(())
    }

    // audit: COTRTLIC-024
    // audit: COTRTLIC-044
    #[test]
    fn an_update_pads_the_description_to_fifty_bytes() -> TestResult {
        let mut table = Table {
            rows: vec![row("07", "CAFÉ")?],
            ..Table::default()
        };
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(1, "U", "")])).into_state();
        let outcome = f10(
            &mut table,
            &state,
            ("", ""),
            &rows_with(&[(1, "U", " CAFÉ ")]),
        );
        assert_eq!(outcome.committed(), Committed::Updated);
        assert_eq!(outcome.message(), Some(ListMessage::NoChangeDetected));
        let stored = table.description("07")?;
        assert_eq!(stored.len(), 50);
        assert_eq!(stored.trim_end(), "CAFÉ");
        Ok(())
    }

    // audit: COTRTLIC-025
    // audit: COTRTLIC-026
    // audit: COTRTLIC-034
    // audit: COTRTLIC-040
    #[test]
    fn a_failed_description_edit_keeps_the_update_pending_and_commits_nothing() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(1, "U", "")])).into_state();

        for blank in ["", "   ", "*"] {
            let outcome = enter(&mut table, &state, ("", ""), &rows_with(&[(1, "U", blank)]));
            assert_eq!(
                outcome.message_text().as_deref(),
                Some("Transaction Desc must be supplied.")
            );
            assert_eq!(outcome.status(), StatusClass::Input);
            assert_eq!(shown(&outcome, 1)?.description(), "*");
            assert_eq!(shown(&outcome, 1)?.description_field(), FieldState::Error);
            assert_eq!(outcome.info(), Some(InfoMessage::UpdatePrompt));
            assert_eq!(outcome.state().pending(), PendingAction::Update);
        }

        let bad = rows_with(&[(1, "U", "BAD;DESC")]);
        let outcome = enter(&mut table, &state, ("", ""), &bad);
        assert_eq!(
            outcome.message_text().as_deref(),
            Some("Transaction Desc can have numbers or alphabets only.")
        );
        assert_eq!(shown(&outcome, 1)?.description(), "BAD;DESC");
        assert_eq!(shown(&outcome, 1)?.description_field(), FieldState::Error);

        let outcome = f10(&mut table, &state, ("", ""), &bad);
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(outcome.status(), StatusClass::Input);
        assert_eq!(table.description("01")?, "PURCHASE");
        Ok(())
    }

    // audit: COTRTLIC-036
    // audit: COTRTLIC-039
    // audit: COTRTLIC-049
    #[test]
    fn a_confirmed_delete_removes_the_row_and_resets_the_state() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let delete = rows_with(&[(7, "D", "")]);
        let outcome = enter(&mut table, &state, ("", ""), &delete);
        assert_eq!(outcome.info(), Some(InfoMessage::DeletePrompt));
        assert_eq!(
            outcome.info().map(InfoMessage::text),
            Some("Delete HIGHLIGHTED row ? Press F10 to confirm")
        );
        assert_eq!(shown(&outcome, 7)?.emphasis(), Emphasis::Highlighted);
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        assert_eq!(table.rows.len(), 7);
        let state = outcome.into_state();

        let outcome = f10(&mut table, &state, ("", ""), &delete);
        assert_eq!(outcome.committed(), Committed::Deleted);
        assert_eq!(
            outcome.info().map(InfoMessage::text),
            Some("HIGHLIGHTED row deleted.Hit Enter to continue")
        );
        assert!(table.find(&code("07")?).is_none());
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!(shown(&outcome, 7)?.selection(), Selection::Blank);
        let state = outcome.into_state();

        // The next ENTER reads page 1 under the filters then on the screen.
        let outcome = enter(&mut table, &state, ("", ""), &[]);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "03", "04", "05", "06", ""])
        );
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        Ok(())
    }

    // audit: COTRTLIC-037
    #[test]
    fn f10_with_nothing_pending_is_processed_as_enter() -> TestResult {
        let mut table = Table::seed()?;
        table.parents.clear();
        let state = loaded(&mut table).into_state();
        let outcome = f10(&mut table, &state, ("", ""), &rows_with(&[(2, "D", "")]));
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(outcome.info(), Some(InfoMessage::DeletePrompt));
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        assert_eq!(table.rows.len(), 7);
        Ok(())
    }

    // audit: COTRTLIC-050
    #[test]
    fn a_delete_restricted_by_categories_keeps_the_row_and_the_pending_delete() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let delete = rows_with(&[(1, "D", "")]);
        let state = enter(&mut table, &state, ("", ""), &delete).into_state();
        let outcome = f10(&mut table, &state, ("", ""), &delete);
        assert!(
            db2_message(&outcome)
                .starts_with("Please delete associated child records first: SQLCODE:")
        );
        assert_eq!(outcome.status().http_status(), 400);
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        assert_eq!(shown(&outcome, 1)?.selection(), Selection::Delete);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        assert_eq!(table.rows.len(), 7);
        // A repeated F10 tries the delete again.
        let again = f10(&mut table, outcome.state(), ("", ""), &delete);
        assert_eq!(again.status(), StatusClass::Input);
        Ok(())
    }

    // audit: COTRTLIC-046
    // audit: COTRTLIC-047
    // audit: COTRTLIC-048
    // audit: COTRTLIC-054
    #[test]
    fn a_failed_update_keeps_it_pending_and_re_reads_the_page() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(3, "U", "")])).into_state();
        let edited = rows_with(&[(3, "U", "CREDITS")]);

        let cases = [
            (
                Fault::Commit(CommitResult::Deadlock),
                "Deadlock. Someone else updating ? SQLCODE:",
            ),
            (
                Fault::Commit(CommitResult::Failed),
                "Update failed with SQLCODE:",
            ),
            (
                Fault::Commit(CommitResult::Restricted),
                "Update failed with SQLCODE:",
            ),
        ];
        for (fault, prefix) in cases {
            table.fault = fault;
            let outcome = f10(&mut table, &state, ("", ""), &edited);
            assert!(db2_message(&outcome).starts_with(prefix));
            assert_eq!(outcome.status().http_status(), 500);
            assert_eq!(outcome.committed(), Committed::Nothing);
            assert_eq!(outcome.state().pending(), PendingAction::Update);
        }
        table.fault = Fault::None;

        // Another user deleted the row: nothing written, 404, the edit kept at its position.
        table.remove("03")?;
        let outcome = f10(&mut table, &state, ("", ""), &edited);
        assert!(
            db2_message(&outcome).starts_with("Record not found. Deleted by others ? SQLCODE:")
        );
        assert_eq!(outcome.status().http_status(), 404);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "04", "05", "06", "07", ""])
        );
        assert_eq!(shown(&outcome, 3)?.description(), "CREDITS");
        assert_eq!(outcome.state().pending(), PendingAction::Update);

        // An earlier message stands: an unchanged description reports no change instead.
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(2, "U", "")])).into_state();
        table.fault = Fault::Commit(CommitResult::NotFound);
        let outcome = f10(
            &mut table,
            &state,
            ("", ""),
            &rows_with(&[(2, "U", "PAYMENT")]),
        );
        assert_eq!(outcome.message(), Some(ListMessage::NoChangeDetected));
        assert_eq!(outcome.status(), StatusClass::NotFound);
        Ok(())
    }

    // audit: COTRTLIC-051
    #[test]
    fn a_failed_delete_keeps_it_pending_with_no_re_read() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let delete = rows_with(&[(7, "D", "")]);
        let state = enter(&mut table, &state, ("", ""), &delete).into_state();

        table.fault = Fault::Commit(CommitResult::Failed);
        let outcome = f10(&mut table, &state, ("", ""), &delete);
        assert!(db2_message(&outcome).starts_with("Delete failed with message: SQLCODE:"));
        assert_eq!(outcome.status(), StatusClass::Store);
        table.fault = Fault::Commit(CommitResult::Deadlock);
        let outcome = f10(&mut table, &state, ("", ""), &delete);
        assert_eq!(outcome.status(), StatusClass::Store);
        table.fault = Fault::None;

        table.remove("07")?;
        let outcome = f10(&mut table, &state, ("", ""), &delete);
        assert!(db2_message(&outcome).starts_with("Delete failed with message: SQLCODE:"));
        assert_eq!(outcome.status().http_status(), 404);
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        assert_eq!(outcome.committed(), Committed::Nothing);
        Ok(())
    }

    // audit: COTRTLIC-043
    #[test]
    fn both_pending_flags_can_be_set_and_an_update_clears_only_its_own() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(2, "U", "")])).into_state();
        assert_eq!(state.pending(), PendingAction::Update);
        // ENTER without a selection keeps it.
        let state = enter(&mut table, &state, ("", ""), &[]).into_state();
        assert_eq!(state.pending(), PendingAction::Update);
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(2, "D", "")])).into_state();
        assert_eq!(state.pending(), PendingAction::UpdateAndDelete);
        assert!(state.pending().is_pending());

        // F10 commits the action now typed at the remembered row, here the update.
        let outcome = f10(&mut table, &state, ("", ""), &rows_with(&[(2, "U", "")]));
        assert_eq!(outcome.committed(), Committed::Updated);
        assert_eq!(table.description("02")?, format!("{:<50}", "PAYMENT"));
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        Ok(())
    }

    // audit: COTRTLIC-043
    // audit: COTRTLIC-049
    #[test]
    fn a_pending_delete_then_update_sets_both_flags_and_the_delete_clears_them() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(7, "D", "")])).into_state();
        assert_eq!(state.pending(), PendingAction::Delete);
        // The description is not enterable yet, so it is the one shown: no change, no error.
        let outcome = enter(&mut table, &state, ("", ""), &rows_with(&[(7, "U", "")]));
        assert_eq!(outcome.message(), Some(ListMessage::NoChangeDetected));
        let state = outcome.into_state();
        assert_eq!(state.pending(), PendingAction::UpdateAndDelete);

        let outcome = f10(&mut table, &state, ("", ""), &rows_with(&[(7, "D", "")]));
        assert_eq!(outcome.committed(), Committed::Deleted);
        assert!(table.find(&code("07")?).is_none());
        assert_eq!(outcome.state().pending(), PendingAction::None);
        Ok(())
    }

    // audit: COTRTLIC-027
    // audit: COTRTLIC-028
    // audit: COTRTLIC-029
    // audit: COTRTLIC-030
    #[test]
    fn the_filter_edits_report_their_input_filter_and_change() -> TestResult {
        let state = TranTypeListState::initial();
        let edit = edit_type_filter("05", &state);
        assert_eq!(edit.input(), "05");
        assert_eq!(edit.filter(), &TypeFilter::Valid(code("05")?));
        assert_eq!(edit.change(), Change::Changed);
        let edit = edit_type_filter("00", &state);
        assert_eq!(edit.filter(), &TypeFilter::Blank);
        assert_eq!(edit.change(), Change::Unchanged);
        let edit = edit_type_filter("5", &state);
        assert_eq!(edit.input(), "5 ");
        assert_eq!(edit.filter(), &TypeFilter::NotNumeric);
        assert_eq!(edit.change(), Change::Changed);

        let edit = edit_desc_filter(" PAY ", &state);
        assert_eq!(edit.input(), format!("{:<50}", " PAY "));
        assert_eq!(edit.filter(), &DescFilter::Valid("%PAY%".to_owned()));
        assert_eq!(edit.change(), Change::Changed);
        let edit = edit_desc_filter("", &state);
        assert_eq!(edit.filter(), &DescFilter::Blank);
        assert_eq!(edit.change(), Change::Unchanged);
        Ok(())
    }

    // audit: COTRTLIC-007
    // audit: COTRTLIC-008
    // audit: COTRTLIC-009
    // audit: COTRTLIC-010
    // audit: COTRTLIC-011
    #[test]
    fn f7_and_f8_move_between_pages_with_the_paging_messages() -> TestResult {
        let mut table = Table::numbered(15)?;
        let state = loaded(&mut table).into_state();

        let outcome = paged(&mut table, Direction::Forward, &state);
        assert_eq!(
            codes(&outcome),
            texts(&["08", "09", "10", "11", "12", "13", "14"])
        );
        assert_eq!(outcome.page_no().get(), 2);
        assert_eq!(outcome.state().last_key(), &code("15")?);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        assert_eq!(outcome.message(), None);

        let outcome = paged(&mut table, Direction::Forward, outcome.state());
        assert_eq!(codes(&outcome), texts(&["15", "", "", "", "", "", ""]));
        assert_eq!(outcome.page_no().get(), 3);
        assert_eq!(outcome.message(), Some(ListMessage::NoMorePagesForSearch));
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        assert_eq!(outcome.state().last_page(), LastPage::Shown);
        let last = outcome.into_state();

        let outcome = paged(&mut table, Direction::Forward, &last);
        assert_eq!(codes(&outcome), texts(&["15", "", "", "", "", "", ""]));
        assert_eq!(
            outcome.message_text().as_deref(),
            Some("No more pages to display")
        );
        assert_eq!(outcome.info(), None);

        // After any other key the indicator is reset, so F8 shows the first message again.
        let other = enter(&mut table, &last, ("", ""), &[]).into_state();
        assert_eq!(other.last_page(), LastPage::NotShown);
        let outcome = paged(&mut table, Direction::Forward, &other);
        assert_eq!(outcome.message(), Some(ListMessage::NoMorePagesForSearch));

        let outcome = paged(&mut table, Direction::Backward, &last);
        assert_eq!(
            codes(&outcome),
            texts(&["08", "09", "10", "11", "12", "13", "14"])
        );
        assert_eq!(outcome.page_no().get(), 2);
        assert_eq!(outcome.state().first_key(), &code("08")?);
        assert_eq!(outcome.state().last_key(), &code("15")?);
        assert_eq!(outcome.next_page(), NextPage::Exists);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));

        let outcome = paged(&mut table, Direction::Backward, outcome.state());
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!(outcome.page_no(), PageNumber::FIRST);
        assert_eq!(
            outcome.message_text().as_deref(),
            Some("No previous pages to display")
        );
        assert_eq!(outcome.info(), None);

        let outcome = paged(&mut table, Direction::Backward, outcome.state());
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!(outcome.message(), Some(ListMessage::NoPreviousPages));
        assert_eq!(outcome.status(), StatusClass::Ok);
        Ok(())
    }

    /// A state at page `page_no` whose first key is `first`, with no rows.
    fn at_page(
        page_no: i64,
        first: &str,
        next_page: NextPage,
    ) -> Result<TranTypeListState, String> {
        TranTypeListState::from_parts(&StateParts {
            first_key: first,
            last_key: first,
            page_no,
            next_page,
            ..StateParts::default()
        })
        .map_err(|failure| failure.to_string())
    }

    // audit: COTRTLIC-012
    #[test]
    fn a_short_backward_read_is_a_db2_error_except_on_page_one() -> TestResult {
        let mut table = Table::numbered(9)?;
        let state = at_page(3, "03", NextPage::Exists)?;
        let outcome = paged(&mut table, Direction::Backward, &state);
        assert_eq!(codes(&outcome), texts(&["", "", "", "", "", "01", "02"]));
        assert!(
            db2_message(&outcome).starts_with("Error on fetch Cursor C-TR-TYPE-BACKWARD SQLCODE:")
        );
        assert_eq!(outcome.status(), StatusClass::Store);
        assert_eq!(outcome.page_no().get(), 2);
        assert_eq!(outcome.state().first_key(), &code("03")?);
        assert_eq!(outcome.state().last_key(), &code("03")?);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));

        let state = at_page(2, "03", NextPage::Exists)?;
        let outcome = paged(&mut table, Direction::Backward, &state);
        assert_eq!(outcome.message(), Some(ListMessage::NoPreviousPages));
        assert_eq!(outcome.status(), StatusClass::Ok);
        assert_eq!(outcome.info(), None);
        Ok(())
    }

    // audit: COTRTLIC-013
    #[test]
    fn the_page_number_keeps_one_digit() -> TestResult {
        let mut table = Table::numbered(15)?;
        let state = at_page(9, "08", NextPage::Exists)?;
        let outcome = paged(&mut table, Direction::Forward, &state);
        assert_eq!(outcome.page_no(), PageNumber::FIRST);
        assert_eq!(codes(&outcome).first().map(String::as_str), Some("08"));
        // Page 0, left by a paging reset, minus one is 1.
        let state = at_page(0, "  ", NextPage::NotExists)?;
        let outcome = paged(&mut table, Direction::Backward, &state);
        assert_eq!(outcome.page_no(), PageNumber::FIRST);
        assert_eq!(outcome.message(), Some(ListMessage::NoPreviousPages));
        assert!(outcome.rows().iter().all(Option::is_none));
        Ok(())
    }

    // audit: COTRTLIC-027
    // audit: COTRTLIC-028
    // audit: COTRTLIC-029
    #[test]
    fn the_type_filter_edit_and_its_change_detection() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();

        // `00` is blank: every row, and an unchanged filter against a blank state.
        let edit = edit_type_filter("00", &state);
        assert_eq!(edit.change(), Change::Unchanged);
        assert_eq!(edit.input(), "00");
        let outcome = enter(&mut table, &state, ("00", ""), &[]);
        assert_eq!(codes(&outcome).len(), 7);
        assert_eq!(outcome.type_filter().text(), "");
        assert_eq!(outcome.type_filter().status(), FilterStatus::Blank);
        // As typed while a selection is non-blank.
        let outcome = enter(&mut table, &state, ("00", ""), &rows_with(&[(1, "D", "")]));
        assert_eq!(outcome.type_filter().text(), "00");
        // `00` after a real filter is a change, and blank after `00` is none.
        let state = enter(&mut table, &state, ("05", ""), &[]).into_state();
        let state = enter(&mut table, &state, ("00", ""), &[]).into_state();
        assert_eq!(state.type_filter(), "00");
        assert_eq!(edit_type_filter("", &state).change(), Change::Unchanged);

        for invalid in ["A1", " 1", "1", "1 "] {
            let progress = act(
                ListAid::Enter,
                &ListInput {
                    type_filter: invalid,
                    ..ListInput::default()
                },
                &state,
                PageSize::DEFAULT,
            );
            let Progress::Done(outcome) = progress else {
                return Err(format!("no read for type filter {invalid:?}"));
            };
            assert_eq!(
                outcome.message_text().as_deref(),
                Some("TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER")
            );
            assert_eq!(outcome.status(), StatusClass::Input);
            assert_eq!(outcome.type_filter().status(), FilterStatus::InError);
            assert_eq!(outcome.type_filter().text(), invalid.trim_end());
            assert_eq!(outcome.info(), None);
            assert_eq!(codes(&outcome).len(), 7);
            assert!(
                outcome
                    .rows()
                    .iter()
                    .flatten()
                    .all(|row| row.selector() == FieldState::ReadOnly)
            );
            assert_eq!(outcome.state().selectors(), SelectorAccess::Protected);
            assert_eq!(outcome.state().page_no().get(), 0);
            assert_eq!(outcome.state().type_filter().trim_end(), invalid.trim_end());
        }
        Ok(())
    }

    // audit: COTRTLIC-018
    // audit: COTRTLIC-031
    #[test]
    fn a_filter_matching_no_rows_protects_the_selectors() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let outcome = enter(&mut table, &state, ("99", ""), &rows_with(&[(2, "D", "")]));
        assert_eq!(
            outcome.message_text().as_deref(),
            Some("No Records found for these filter conditions")
        );
        assert_eq!(outcome.status(), StatusClass::Input);
        assert_eq!(outcome.type_filter().status(), FilterStatus::InError);
        assert_eq!(outcome.info(), None);
        // No read: the state's rows, under the new filter text, with page number 0.
        assert_eq!(
            codes(&outcome),
            texts(&["01", "02", "03", "04", "05", "06", "07"])
        );
        assert_eq!(shown(&outcome, 2)?.selector(), FieldState::ReadOnly);
        assert_eq!(outcome.state().pending(), PendingAction::Delete);
        assert_eq!(outcome.state().page_no().get(), 0);
        assert_eq!(outcome.state().type_filter(), "99");
        let state = outcome.into_state();

        // The protected selectors are not received, so the F10 is an ENTER.
        let outcome = f10(&mut table, &state, ("", ""), &rows_with(&[(2, "D", "")]));
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(outcome.state().selected(), None);
        assert_eq!(codes(&outcome).len(), 7);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));

        // A description filter matching nothing is flagged too.
        let state = loaded(&mut table).into_state();
        let outcome = enter(&mut table, &state, ("", "ZZZ"), &[]);
        assert_eq!(outcome.desc_filter().status(), FilterStatus::InError);
        assert_eq!(outcome.desc_filter().text(), "ZZZ");
        Ok(())
    }

    // audit: COTRTLIC-017
    // audit: COTRTLIC-018
    #[test]
    fn the_screen_returns_only_what_the_previous_response_left_enterable() -> TestResult {
        let mut table = Table::numbered(3)?;
        let state = loaded(&mut table).into_state();
        // A selection at a position that shows no row is never sent.
        let outcome = enter(&mut table, &state, ("", ""), &rows_with(&[(5, "D", "")]));
        assert_eq!(outcome.state().pending(), PendingAction::None);
        assert_eq!(outcome.info(), Some(InfoMessage::RecordActions));
        // A selection is one character, and rows past the seventh have no field.
        let mut long = rows_with(&[(1, "DX", "")]);
        long.extend(rows_with(&[(1, "U", "")]));
        let edit = edit_selections(&long, &state);
        assert_eq!(edit.selection(row_number(1)?), Selection::Delete);
        assert_eq!(edit.selected(), Some(row_number(1)?));
        assert_eq!(Selection::Delete.shown(), Some('D'));
        assert_eq!(Selection::Invalid('x').shown(), Some('x'));
        assert_eq!(Selection::Blank.shown(), None);
        assert_eq!(Selection::Update.shown(), Some('U'));
        Ok(())
    }

    // audit: COTRTLIC-052
    #[test]
    fn a_failed_page_read_is_a_store_failure() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        table.fault = Fault::Read;
        let outcome = enter(&mut table, &state, ("", ""), &[]);
        assert!(db2_message(&outcome).starts_with("C-TR-TYPE-FORWARD Open SQLCODE:"));
        assert_eq!(outcome.status().http_status(), 500);
        assert!(outcome.rows().iter().all(Option::is_none));

        let state = at_page(3, "08", NextPage::Exists)?;
        let outcome = paged(&mut table, Direction::Backward, &state);
        assert!(db2_message(&outcome).starts_with("C-TR-TYPE-BACKWARD Open SQLCODE:"));
        assert_eq!(outcome.status(), StatusClass::Store);
        // On page 1, `No previous pages to display` replaces it; the failure still stands.
        let state = at_page(2, "08", NextPage::Exists)?;
        let outcome = paged(&mut table, Direction::Backward, &state);
        assert_eq!(outcome.message(), Some(ListMessage::NoPreviousPages));
        assert_eq!(outcome.status(), StatusClass::Store);
        Ok(())
    }

    // audit: COTRTLIC-032
    #[test]
    fn a_failed_filter_count_is_a_store_failure_that_commits_nothing() -> TestResult {
        let mut table = Table::seed()?;
        table.parents.clear();
        let state = loaded(&mut table).into_state();
        let delete = rows_with(&[(1, "D", "")]);
        let state = enter(&mut table, &state, ("", ""), &delete).into_state();
        table.fault = Fault::Count;
        let outcome = f10(&mut table, &state, ("", "PUR"), &delete);
        assert_eq!(outcome.status(), StatusClass::Store);
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(table.rows.len(), 7);
        Ok(())
    }

    #[test]
    fn an_update_is_edited_again_against_the_row_as_re_read() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(2, "U", "")])).into_state();
        // Another user renamed the row: the edit compares with the database.
        table.rows.retain(|row| row.tr_type().to_text() != "02");
        table.rows.push(row("02", "RENAMED")?);
        let outcome = f10(
            &mut table,
            &state,
            ("", ""),
            &rows_with(&[(2, "U", "renamed")]),
        );
        assert_eq!(outcome.committed(), Committed::Updated);
        assert_eq!(outcome.message(), Some(ListMessage::NoChangeDetected));

        // A state whose rows were altered cannot pass a description the edit rejects.
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", ""), &rows_with(&[(1, "U", "")])).into_state();
        let parts = parts_of(&state);
        let mut rows = parts.rows.clone();
        if let Some(first) = rows.first_mut() {
            *first = Some(("01".to_owned(), "A;B".to_owned()));
        }
        let altered = from_owned(&parts, &rows)?;
        let outcome = f10(
            &mut table,
            &altered,
            ("", ""),
            &rows_with(&[(1, "U", "A;B")]),
        );
        assert_eq!(outcome.committed(), Committed::Nothing);
        assert_eq!(
            outcome.message(),
            Some(ListMessage::DescriptionNotAlphanumeric)
        );
        assert_eq!(outcome.status(), StatusClass::Input);
        assert_eq!(table.description("01")?, "PURCHASE");
        Ok(())
    }

    /// The owned parts of a state, as a DTO carries them.
    #[derive(Debug, Clone)]
    struct OwnedParts {
        type_filter: String,
        desc_filter: String,
        rows: Vec<Option<(String, String)>>,
        selected_row: Option<i64>,
        first_key: String,
        last_key: String,
        page_no: i64,
        next_page: NextPage,
        last_page: LastPage,
        pending: PendingAction,
        screen_rows: Vec<Option<(String, String)>>,
        selectors: SelectorAccess,
        description_entry: Option<i64>,
    }

    fn parts_of(state: &TranTypeListState) -> OwnedParts {
        OwnedParts {
            type_filter: state.type_filter().to_owned(),
            desc_filter: state.desc_filter().to_owned(),
            rows: state
                .rows()
                .iter()
                .map(|row| {
                    row.as_ref()
                        .map(|row| (row.tr_type().to_text(), row.tr_description().to_owned()))
                })
                .collect(),
            selected_row: state.selected().map(|index| i64::from(index.number())),
            first_key: state.first_key().to_text(),
            last_key: state.last_key().to_text(),
            page_no: i64::from(state.page_no().get()),
            next_page: state.next_page(),
            last_page: state.last_page(),
            pending: state.pending(),
            screen_rows: state
                .screen_rows()
                .iter()
                .map(|row| {
                    row.as_ref()
                        .map(|row| (row.code().to_text(), row.description().to_owned()))
                })
                .collect(),
            selectors: state.selectors(),
            description_entry: state
                .description_entry()
                .map(|index| i64::from(index.number())),
        }
    }

    fn borrowed(rows: &[Option<(String, String)>]) -> Vec<Option<RowParts<'_>>> {
        rows.iter()
            .map(|row| {
                row.as_ref()
                    .map(|(code, description)| RowParts { code, description })
            })
            .collect()
    }

    fn from_owned(
        parts: &OwnedParts,
        rows: &[Option<(String, String)>],
    ) -> Result<TranTypeListState, String> {
        let rows = borrowed(rows);
        let screen_rows = borrowed(&parts.screen_rows);
        TranTypeListState::from_parts(&StateParts {
            type_filter: &parts.type_filter,
            desc_filter: &parts.desc_filter,
            rows: &rows,
            selected_row: parts.selected_row,
            first_key: &parts.first_key,
            last_key: &parts.last_key,
            page_no: parts.page_no,
            next_page: parts.next_page,
            last_page: parts.last_page,
            pending: parts.pending,
            screen_rows: &screen_rows,
            selectors: parts.selectors,
            description_entry: parts.description_entry,
        })
        .map_err(|failure| failure.to_string())
    }

    // audit: COTRTLIC-017
    // audit: COTRTLIC-023
    #[test]
    fn an_echoed_state_round_trips_and_out_of_range_parts_are_typed_errors() -> TestResult {
        let mut table = Table::seed()?;
        let state = loaded(&mut table).into_state();
        let state = enter(&mut table, &state, ("", "E"), &rows_with(&[(2, "U", "")])).into_state();
        let parts = parts_of(&state);
        assert_eq!(from_owned(&parts, &parts.rows)?, state);
        assert_eq!(TranTypeListState::default(), TranTypeListState::initial());

        let base = StateParts::default();
        let eight = [None; 8];
        let x51 = "X".repeat(51);
        let long_row = [Some(RowParts {
            code: "01",
            description: &x51,
        })];
        let cases: [(StateParts<'_>, ListFailure); 8] = [
            (
                StateParts {
                    rows: &eight,
                    ..base
                },
                ListFailure::TooManyRows,
            ),
            (
                StateParts {
                    screen_rows: &eight,
                    ..base
                },
                ListFailure::TooManyRows,
            ),
            (
                StateParts {
                    selected_row: Some(8),
                    ..base
                },
                ListFailure::RowOutOfRange,
            ),
            (
                StateParts {
                    description_entry: Some(0),
                    ..base
                },
                ListFailure::RowOutOfRange,
            ),
            (
                StateParts {
                    page_no: 10,
                    ..base
                },
                ListFailure::PageOutOfRange,
            ),
            (
                StateParts {
                    type_filter: "ABC",
                    ..base
                },
                ListFailure::FieldTooLong,
            ),
            (
                StateParts {
                    first_key: "ABC",
                    ..base
                },
                ListFailure::InvalidKey,
            ),
            (
                StateParts {
                    rows: &long_row,
                    ..base
                },
                ListFailure::InvalidRow,
            ),
        ];
        for (parts, failure) in cases {
            assert_eq!(TranTypeListState::from_parts(&parts), Err(failure));
            assert!(!failure.to_string().contains("ABC"));
        }
        let y51 = "Y".repeat(51);
        let long_shown = [Some(RowParts {
            code: "01",
            description: &y51,
        })];
        assert_eq!(
            TranTypeListState::from_parts(&StateParts {
                screen_rows: &long_shown,
                ..base
            }),
            Err(ListFailure::FieldTooLong)
        );
        assert_eq!(
            TranTypeListState::from_parts(&StateParts {
                last_key: "\u{7f}",
                ..base
            }),
            Err(ListFailure::InvalidKey)
        );
        for failure in [ListFailure::PageOutOfRange, ListFailure::InvalidRow] {
            assert!(!failure.to_string().is_empty());
        }
        Ok(())
    }

    // audit: COTRTLIC-054
    #[test]
    fn the_messages_carry_the_program_literals() {
        let literals = [
            (
                ListMessage::NoRecordsFound,
                "No records found for this search condition.",
            ),
            (
                ListMessage::NoMorePagesForSearch,
                "No more pages for these search conditions",
            ),
            (
                ListMessage::MoreThanOneAction,
                "Please select only 1 action",
            ),
            (
                ListMessage::InvalidActionCode,
                "Action code selected is invalid",
            ),
            (
                ListMessage::NoChangeDetected,
                "No change detected with respect to database values.",
            ),
            (
                ListMessage::DescriptionRequired,
                "Transaction Desc must be supplied.",
            ),
            (
                ListMessage::DescriptionNotAlphanumeric,
                "Transaction Desc can have numbers or alphabets only.",
            ),
            (
                ListMessage::TypeFilterNotNumeric,
                "TYPE CODE FILTER,IF SUPPLIED MUST BE A 2 DIGIT NUMBER",
            ),
            (
                ListMessage::NoRecordsForFilters,
                "No Records found for these filter conditions",
            ),
            (ListMessage::NoPreviousPages, "No previous pages to display"),
            (ListMessage::NoMorePages, "No more pages to display"),
        ];
        for (message, text) in literals {
            assert_eq!(message.text(), text);
        }
        let actions = [
            (Db2Action::ForwardOpen, "C-TR-TYPE-FORWARD Open SQLCODE:"),
            (Db2Action::BackwardOpen, "C-TR-TYPE-BACKWARD Open SQLCODE:"),
            (
                Db2Action::BackwardFetch,
                "Error on fetch Cursor C-TR-TYPE-BACKWARD SQLCODE:",
            ),
            (
                Db2Action::CountFilters,
                "Error reading TRANSACTION_TYPE table SQLCODE:",
            ),
            (
                Db2Action::UpdateNotFound,
                "Record not found. Deleted by others ? SQLCODE:",
            ),
            (
                Db2Action::UpdateDeadlock,
                "Deadlock. Someone else updating ? SQLCODE:",
            ),
            (Db2Action::UpdateFailed, "Update failed with SQLCODE:"),
            (
                Db2Action::DeleteRestricted,
                "Please delete associated child records first: SQLCODE:",
            ),
            (
                Db2Action::DeleteFailed,
                "Delete failed with message: SQLCODE:",
            ),
            (Db2Action::AccessFailure, "Db2 access failure. SQLCODE:"),
        ];
        for (action, prefix) in actions {
            assert!(ListMessage::Db2(action).text().starts_with(prefix));
            assert!(ListMessage::Db2(action).text().chars().count() <= 75);
        }
        assert_eq!(
            InfoMessage::RecordActions.text(),
            "Type U to update, D to delete any record"
        );
        assert_eq!(InfoMessage::Updated.text(), "HIGHLIGHTED row was updated");
        let statuses = [
            (StatusClass::Ok, 200),
            (StatusClass::Input, 400),
            (StatusClass::NotFound, 404),
            (StatusClass::Store, 500),
        ];
        for (class, status) in statuses {
            assert_eq!(class.http_status(), status);
        }
    }

    #[test]
    fn page_sizes_bound_the_reads_to_the_seven_positions() -> TestResult {
        assert_eq!(PageSize::new(0), None);
        assert_eq!(PageSize::new(3).map(PageSize::rows), Some(3));
        assert_eq!(PageSize::new(100).map(PageSize::rows), Some(7));
        assert_eq!(PageSize::from_u32(5).map(PageSize::rows), Some(5));
        assert_eq!(PageSize::from_u32(0), None);
        assert_eq!(PageSize::default(), PageSize::DEFAULT);
        let three = PageSize::new(3).ok_or("page size 3")?;

        let mut table = Table::numbered(9)?;
        let progress = load(three);
        if let Progress::Read(step) = &progress {
            assert_eq!(step.query().limit(), 4);
        }
        let outcome = run(&mut table, progress);
        assert_eq!(codes(&outcome), texts(&["01", "02", "03", "", "", "", ""]));
        assert_eq!(outcome.state().last_key(), &code("04")?);
        assert_eq!(outcome.next_page(), NextPage::Exists);

        let state = outcome.into_state();
        let progress = page(Direction::Forward, "", "", &state, three);
        let outcome = run(&mut table, progress);
        assert_eq!(codes(&outcome), texts(&["04", "05", "06", "", "", "", ""]));
        let progress = page(Direction::Backward, "", "", outcome.state(), three);
        if let Progress::Read(step) = &progress {
            assert_eq!(step.query().limit(), 3);
            assert_eq!(step.query().direction(), ReadDirection::Backward);
        }
        let outcome = run(&mut table, progress);
        assert_eq!(codes(&outcome), texts(&["01", "02", "03", "", "", "", ""]));
        Ok(())
    }

    #[test]
    fn row_indexes_and_page_numbers_stay_in_range() {
        assert_eq!(RowIndex::new(7), None);
        assert_eq!(RowIndex::new(6).map(RowIndex::number), Some(7));
        assert_eq!(RowIndex::from_number(0), None);
        assert_eq!(RowIndex::from_number(8), None);
        assert_eq!(RowIndex::from_number(i64::MIN), None);
        assert_eq!(RowIndex::from_number(1).map(RowIndex::index), Some(0));
        assert_eq!(RowIndex::ALL.len(), 7);
        assert_eq!(PageNumber::new(10), None);
        assert_eq!(PageNumber::new(-1), None);
        assert_eq!(PageNumber::new(9).map(PageNumber::get), Some(9));
        assert_eq!(PageNumber::FIRST.get(), 1);
    }
}
