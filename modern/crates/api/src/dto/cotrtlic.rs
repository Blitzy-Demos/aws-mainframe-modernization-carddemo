//! The contracts of CTLI, COTRTLIC's transaction-type list over the Db2 data version of the
//! types, `db2.transaction_type` (E23), behind the admin gate:
//!
//! - endpoint 24, `GET /api/v1/transaction-types`: [`TranTypeListQuery`] →
//!   [`TranTypeListResponse`], the screen load and F7/F8;
//! - endpoint 25, `POST /api/v1/transaction-types/list-actions`: [`TranTypeListActionRequest`]
//!   → [`TranTypeListActionResult`], ENTER and F10.
//!
//! F2 (to CTTU) and F3 call no endpoint. Both endpoints are Bounded Pagination collections: a
//! page holds `page_size` rows, 7 by default (`domain::online::cotrtlic::DEFAULT_PAGE_SIZE`),
//! and the seven positions of the map bound it.
//!
//! # State
//!
//! [`TranTypeListState`] is COTRTLIC's COMMAREA, `WS-THIS-PROGCOMMAREA` (`COTRTLIC.cbl:377-418`),
//! plus what the previous response left on the screen (X-COTRTLIC-9): the rows as the last read
//! returned them, both pending flags, the paging fields, the rows shown and the fields the next
//! request may change. Every response returns it twice: as `state`, which the screen echoes
//! unchanged with ENTER and F10, and as `page_key`, an opaque encoding of the same state that
//! the screen echoes with F7 and F8, because a query string cannot carry the nested state
//! (X-COTRTLIC-6). The state is request data and never authority: the admin gate is the
//! verified token, the domain re-runs every edit, and every commit re-reads its row first (AAP
//! 0.5.1, COTRTLIC-001).
//!
//! # Workflow the contract only carries
//!
//! Every rule below is `domain::online::cotrtlic`'s; this module only carries the data.
//!
//! - ENTER edits the selections against the state **as received**, the previous page's rows and
//!   selected row, and only then the description and type filters (COTRTLIC-019). A changed
//!   filter never clears the selections, which keep their row positions on page 1 of the new
//!   filter (E28, COTRTLIC-041).
//! - F10 commits only when an action is pending and the type filter, the description filter and
//!   the selected row are all unchanged; otherwise it is processed exactly as ENTER (COTRTLIC-036
//!   to COTRTLIC-038).
//!
//! # Status
//!
//! COTRTLIC sends the whole screen with every message, so an input error, a row another user
//! deleted or a store failure still returns the page and the state (`COTRTLIC.cbl:699-720`).
//! [`TranTypeListResponse`] and [`TranTypeListActionResult`] therefore carry the audited status
//! of their outcome and answer with it through `IntoResponse`, outside the body: return the
//! value itself, not `Json(value)`, which would answer 200 whatever the outcome.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use domain::copybook::PicX;
use domain::online::cotrtlic as rules;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::{IntoParams, ToSchema};
use validator::{Validate, ValidationError};

use super::{PageDirection, text};

/// The request field of a row error on a selection, `TRTSELn`.
const SELECTION_FIELD: &str = "selection";

/// The request field of a row error on a description, `TRTYPDn`.
const DESCRIPTION_FIELD: &str = "description";

/// `validator`'s code of a `length` rule, which `rows_fit` reports for a row too long for the
/// map, so that it gets the same text as a field too long for its map `LENGTH`.
const LENGTH_CODE: &str = "length";

/// The code of a `page_key` that is not one this API issued. `api::dto` has no text of its own
/// for it, so it answers with `FIELD_LENGTH_MESSAGE`, as for every such code.
const PAGE_KEY_CODE: &str = "page_key";

/// The key of an endpoint 25 request. Every key without a label on the map is sent as ENTER
/// (COTRTLIC-014); F10 is always sent as `PF10`, and the domain decides whether it commits
/// (COTRTLIC-036).
// The schema name `CotrtlicAid` keeps it apart from COTRTUPC's own `Aid` (Generated API Types
// Only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(as = CotrtlicAid)]
#[serde(rename_all = "UPPERCASE")]
pub enum Aid {
    /// `ENTER`: edit the request, then refresh the page or prompt for the confirmation.
    Enter,
    /// `PF10`: confirm the pending update or delete.
    Pf10,
}

impl From<Aid> for rules::ListAid {
    fn from(aid: Aid) -> Self {
        match aid {
            Aid::Enter => Self::Enter,
            Aid::Pf10 => Self::Pf10,
        }
    }
}

/// The pending flags of the state, `WS-CA-DELETE-FLAG` and `WS-CA-UPDATE-FLAG`, which can be
/// set together (COTRTLIC-043).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranTypeListPendingAction {
    /// `none`: neither flag is set.
    None,
    /// `update`: `CA-UPDATE-REQUESTED` (COTRTLIC-040).
    Update,
    /// `delete`: `CA-DELETE-REQUESTED` (COTRTLIC-039).
    Delete,
    /// `update_and_delete`: both flags, after a pending update and then a delete at the same
    /// row, or the reverse (COTRTLIC-043).
    UpdateAndDelete,
}

impl From<rules::PendingAction> for TranTypeListPendingAction {
    fn from(pending: rules::PendingAction) -> Self {
        match pending {
            rules::PendingAction::None => Self::None,
            rules::PendingAction::Update => Self::Update,
            rules::PendingAction::Delete => Self::Delete,
            rules::PendingAction::UpdateAndDelete => Self::UpdateAndDelete,
        }
    }
}

impl From<TranTypeListPendingAction> for rules::PendingAction {
    fn from(pending: TranTypeListPendingAction) -> Self {
        match pending {
            TranTypeListPendingAction::None => Self::None,
            TranTypeListPendingAction::Update => Self::Update,
            TranTypeListPendingAction::Delete => Self::Delete,
            TranTypeListPendingAction::UpdateAndDelete => Self::UpdateAndDelete,
        }
    }
}

/// Whether the previous response left the selectors enterable (COTRTLIC-018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranTypeListSelectors {
    /// `open`: enterable at every row that holds a type.
    Open,
    /// `protected`: protected at every row, after a type filter in error or a filter matching
    /// no rows (COTRTLIC-028, COTRTLIC-031).
    Protected,
}

impl From<rules::SelectorAccess> for TranTypeListSelectors {
    fn from(access: rules::SelectorAccess) -> Self {
        match access {
            rules::SelectorAccess::Open => Self::Open,
            rules::SelectorAccess::Protected => Self::Protected,
        }
    }
}

impl From<TranTypeListSelectors> for rules::SelectorAccess {
    fn from(access: TranTypeListSelectors) -> Self {
        match access {
            TranTypeListSelectors::Open => Self::Open,
            TranTypeListSelectors::Protected => Self::Protected,
        }
    }
}

/// One row of the state: a type code and its description, either as the last read returned
/// them (`WS-CA-EACH-ROW-OUT`, the description as stored) or as the previous response showed
/// them (`TRTTYPn`, `TRTYPDn`, without trailing spaces).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
#[schema(example = json!({"type_code": "01", "description": "PURCHASE"}))]
pub struct TranTypeListRow {
    /// The two-character type code, `WS-CA-ROW-TR-CODE-OUT PIC X(02)`; empty for a blank code.
    #[validate(length(max = 2))]
    #[schema(max_length = 2)]
    pub type_code: String,
    /// The description, `WS-CA-ROW-TR-DESC-OUT PIC X(50)`.
    #[validate(length(max = 50))]
    #[schema(max_length = 50)]
    pub description: String,
}

impl<'a> From<&'a TranTypeListRow> for rules::RowParts<'a> {
    fn from(row: &'a TranTypeListRow) -> Self {
        Self {
            code: &row.type_code,
            description: &row.description,
        }
    }
}

/// The list state: COTRTLIC's COMMAREA and what the previous response left on the screen.
///
/// The server returns it with every response of endpoints 24 and 25, and the screen sends it
/// back unchanged with every ENTER and F10. It carries no authority: the domain rebuilds it
/// through its total constructor, re-runs every edit and re-reads the row of every commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
pub struct TranTypeListState {
    /// `WS-CA-TYPE-CD PIC X(02)`: the type filter of the displayed page, as typed.
    #[validate(length(max = 2))]
    #[schema(max_length = 2)]
    pub type_filter: String,
    /// `WS-CA-TYPE-DESC PIC X(50)`: the description filter of the displayed page, as typed.
    #[validate(length(max = 50))]
    #[schema(max_length = 50)]
    pub desc_filter: String,
    /// The seven rows as the last read returned them, top to bottom; `null` is a position the
    /// read left empty (COTRTLIC-004, COTRTLIC-012).
    #[validate(length(max = 7), custom(function = "rows_fit"))]
    #[schema(max_items = 7)]
    pub rows: Vec<Option<TranTypeListRow>>,
    /// `WS-CA-ROW-SELECTED`: the screen row, 1 to 7, of the last accepted selection; absent for
    /// none (COTRTLIC-023).
    #[validate(range(min = 1, max = 7))]
    #[schema(minimum = 1, maximum = 7)]
    pub selected_row: Option<i64>,
    /// `WS-CA-FIRST-TR-CODE PIC X(02)`, from which the displayed page is re-read; absent when
    /// blank.
    #[validate(length(max = 2))]
    #[schema(max_length = 2)]
    pub first_key: Option<String>,
    /// `WS-CA-LAST-TR-CODE PIC X(02)`, from which F8 reads the next page; absent when blank.
    #[validate(length(max = 2))]
    #[schema(max_length = 2)]
    pub last_key: Option<String>,
    /// `WS-CA-SCREEN-NUM PIC 9(1)`: 1 to 9, or 0 after a filter change that no read has
    /// followed yet (COTRTLIC-013, COTRTLIC-029).
    #[validate(range(min = 0, max = 9))]
    #[schema(minimum = 0, maximum = 9)]
    pub page_no: i64,
    /// `WS-CA-NEXT-PAGE-IND`: whether the forward read found a row after the page.
    pub next_page_exists: bool,
    /// `WS-CA-LAST-PAGE-DISPLAYED`: whether an F8 already ended on the last page
    /// (COTRTLIC-008, COTRTLIC-009).
    pub last_page_shown: bool,
    /// The pending flags (COTRTLIC-043).
    pub pending_action: TranTypeListPendingAction,
    /// The seven rows the previous response showed, top to bottom; `null` is a position it
    /// left empty. The 3270 returns these protected fields as displayed (COTRTLIC-017,
    /// COTRTLIC-018).
    #[validate(length(max = 7), custom(function = "rows_fit"))]
    #[schema(max_items = 7)]
    pub screen_rows: Vec<Option<TranTypeListRow>>,
    /// Whether the previous response left the selectors enterable.
    pub selectors: TranTypeListSelectors,
    /// The screen row, 1 to 7, whose description the previous response left enterable: the row
    /// of a single `U` whose update has not completed (COTRTLIC-040).
    #[validate(range(min = 1, max = 7))]
    #[schema(minimum = 1, maximum = 7)]
    pub description_entry: Option<i64>,
}

/// The state as the domain holds it, text fields without their trailing spaces and keys absent
/// when blank. Encoding it back with `TryFrom` gives the same domain state.
impl From<&rules::TranTypeListState> for TranTypeListState {
    fn from(state: &rules::TranTypeListState) -> Self {
        Self {
            type_filter: without_padding(state.type_filter()),
            desc_filter: without_padding(state.desc_filter()),
            rows: state
                .rows()
                .iter()
                .map(|row| {
                    row.as_ref().map(|row| TranTypeListRow {
                        type_code: text(row.tr_type()),
                        description: row.tr_description().to_owned(),
                    })
                })
                .collect(),
            selected_row: state.selected().map(row_number),
            first_key: key(state.first_key()),
            last_key: key(state.last_key()),
            page_no: i64::from(state.page_no().get()),
            next_page_exists: state.next_page() == rules::NextPage::Exists,
            last_page_shown: state.last_page() == rules::LastPage::Shown,
            pending_action: state.pending().into(),
            screen_rows: state
                .screen_rows()
                .iter()
                .map(|row| {
                    row.as_ref().map(|row| TranTypeListRow {
                        type_code: text(row.code()),
                        description: row.description().to_owned(),
                    })
                })
                .collect(),
            selectors: state.selectors().into(),
            description_entry: state.description_entry().map(row_number),
        }
    }
}

impl From<rules::TranTypeListState> for TranTypeListState {
    fn from(state: rules::TranTypeListState) -> Self {
        Self::from(&state)
    }
}

/// The domain state an echoed state describes, through the domain's total constructor
/// `TranTypeListState::from_parts`. Row numbers stay `i64` here and reach the domain's row
/// index only through its checked conversion.
///
/// # Errors
///
/// The domain's [`rules::ListFailure`] for a part no COTRTLIC screen could hold: more than seven
/// rows, a row number outside 1 to 7, a page number outside 0 to 9, a field longer than its map
/// field, or a code that is not printable ASCII.
impl TryFrom<&TranTypeListState> for rules::TranTypeListState {
    type Error = rules::ListFailure;

    fn try_from(state: &TranTypeListState) -> Result<Self, Self::Error> {
        let rows = row_parts(&state.rows);
        let screen_rows = row_parts(&state.screen_rows);
        Self::from_parts(&rules::StateParts {
            type_filter: &state.type_filter,
            desc_filter: &state.desc_filter,
            rows: &rows,
            selected_row: state.selected_row,
            first_key: state.first_key.as_deref().unwrap_or_default(),
            last_key: state.last_key.as_deref().unwrap_or_default(),
            page_no: state.page_no,
            next_page: if state.next_page_exists {
                rules::NextPage::Exists
            } else {
                rules::NextPage::NotExists
            },
            last_page: if state.last_page_shown {
                rules::LastPage::Shown
            } else {
                rules::LastPage::NotShown
            },
            pending: state.pending_action.into(),
            screen_rows: &screen_rows,
            selectors: state.selectors.into(),
            description_entry: state.description_entry,
        })
    }
}

/// The echoed positions as the domain's row parts.
fn row_parts(rows: &[Option<TranTypeListRow>]) -> Vec<Option<rules::RowParts<'_>>> {
    rows.iter()
        .map(|row| row.as_ref().map(rules::RowParts::from))
        .collect()
}

/// Every row present fits its map fields: a `validator` custom rule over the positions, which
/// a `nested` rule cannot reach through `Option`.
fn rows_fit(rows: &[Option<TranTypeListRow>]) -> Result<(), ValidationError> {
    if rows.iter().flatten().all(|row| row.validate().is_ok()) {
        Ok(())
    } else {
        Err(ValidationError::new(LENGTH_CODE))
    }
}

/// A padded `PIC X(n)` text without its trailing spaces.
fn without_padding(field: &str) -> String {
    field.trim_end_matches(' ').to_owned()
}

/// A key field as sent: absent when blank.
fn key(field: &PicX<2>) -> Option<String> {
    let shown = text(field);
    (!shown.is_empty()).then_some(shown)
}

/// The screen row number, 1 to 7, of a row position.
fn row_number(index: rules::RowIndex) -> i64 {
    i64::from(index.number())
}

/// The query of endpoint 24: the screen load, and F7 or F8 under the filters on the screen.
///
/// With both `page_key` and `direction` it is F7 or F8 from the state `page_key` encodes,
/// with the filter edits run on `type_filter` and `desc_filter` as on ENTER, so a changed filter
/// moves to page 1 of it (COTRTLIC-007 to COTRTLIC-013, X-COTRTLIC-6). Without either it is the
/// screen load, a first entry that ignores every other member (COTRTLIC-002).
#[derive(
    Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, IntoParams, Validate,
)]
#[into_params(parameter_in = Query)]
pub struct TranTypeListQuery {
    /// The type filter on the screen when F7 or F8 is pressed, `TRTYPEI PIC X(2)`, as typed:
    /// the domain's type-filter edit owns its message.
    #[validate(length(max = 2))]
    #[param(max_length = 2)]
    #[schema(max_length = 2)]
    pub type_filter: Option<String>,
    /// The description filter on the screen, `TRDESCI PIC X(50)`, as typed.
    #[validate(length(max = 50))]
    #[param(max_length = 50)]
    #[schema(max_length = 50)]
    pub desc_filter: Option<String>,
    /// The `page_key` of the previous response, echoed unchanged; the screen never builds one.
    /// Absent or empty, the request is the screen load.
    #[validate(custom(function = "page_key_fits"))]
    pub page_key: Option<String>,
    /// `forward` for F8, `backward` for F7; absent, the request is the screen load.
    #[param(inline)]
    pub direction: Option<PageDirection>,
    /// Rows per page, 1 to 100, 7 when absent; a page never holds more than the seven
    /// positions of the map. Outside 1 to 100 the request is rejected with 422 (Bounded
    /// Pagination).
    #[validate(range(min = 1, max = 100))]
    #[param(minimum = 1, maximum = 100)]
    #[schema(minimum = 1, maximum = 100)]
    pub page_size: Option<i64>,
}

impl TranTypeListQuery {
    /// The domain state an F7 or F8 starts from: the one `page_key` encodes, or `None` when
    /// `page_key` is absent or empty.
    ///
    /// # Errors
    ///
    /// [`rules::ListFailure::InvalidKey`] for a `page_key` this API did not issue, which the
    /// `page_key` rule has already rejected for a validated query; otherwise the domain's
    /// failure for a state no COTRTLIC screen could hold.
    pub fn state(&self) -> Result<Option<rules::TranTypeListState>, rules::ListFailure> {
        let Some(page_key) = self.page_key.as_deref().filter(|key| !key.is_empty()) else {
            return Ok(None);
        };
        let state = decode_page_key(page_key).ok_or(rules::ListFailure::InvalidKey)?;
        rules::TranTypeListState::try_from(&state).map(Some)
    }
}

/// The `page_key` of a state: the lowercase hexadecimal of its JSON, URL-safe without escaping.
fn page_key(state: &TranTypeListState) -> String {
    // Serialising this struct cannot fail: its members are strings, integers, booleans, unit
    // enums and sequences of them, under string keys. The empty key the fallback would give is
    // rejected by no rule and read as a screen load.
    serde_json::to_vec(state)
        .map(|json| hex(&json))
        .unwrap_or_default()
}

/// The state a `page_key` encodes; `None` for any text this API does not issue.
fn decode_page_key(page_key: &str) -> Option<TranTypeListState> {
    unhex(page_key).and_then(|json| serde_json::from_slice(&json).ok())
}

/// The `page_key` rule: empty, or a key this API issued whose state passes its own rules.
fn page_key_fits(page_key: &str) -> Result<(), ValidationError> {
    if page_key.is_empty()
        || decode_page_key(page_key).is_some_and(|state| state.validate().is_ok())
    {
        Ok(())
    } else {
        Err(ValidationError::new(PAGE_KEY_CODE))
    }
}

/// `bytes` as lowercase hexadecimal, two digits per byte.
fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for &byte in bytes {
        for nibble in [byte >> 4, byte & 0x0F] {
            out.extend(char::from_digit(u32::from(nibble), 16));
        }
    }
    out
}

/// The bytes of hexadecimal `text`; `None` for an odd length or a character that is not a hex
/// digit. Total: it never indexes and never panics.
fn unhex(text: &str) -> Option<Vec<u8>> {
    let pairs = text.as_bytes().chunks_exact(2);
    if !pairs.remainder().is_empty() {
        return None;
    }
    pairs
        .map(|pair| match *pair {
            [high, low] => Some((nibble(high)? << 4) | nibble(low)?),
            _ => None,
        })
        .collect()
}

/// The value of one hex digit.
fn nibble(digit: u8) -> Option<u8> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
}

/// One row of a response as `2200-SETUP-ARRAY-ATTRIBS` and `2300-SCREEN-ARRAY-INIT` show it
/// (COTRTLIC-056).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranTypeListShownRow {
    /// The type code, `TRTTYPO`.
    pub type_code: String,
    /// The description, `TRTYPDO`, without trailing spaces: as read, or on the row of a single
    /// `U` the description received, `*` when blank (COTRTLIC-040, COTRTLIC-041).
    pub description: String,
    /// The selection, `TRTSELO`: as typed, empty when blank, after a commit or a page move.
    pub selection: String,
    /// Whether the row is the one a single `D` or `U` highlights, in the neutral colour.
    pub highlighted: bool,
    /// Whether the selector is enterable for the next request; a selector in error is
    /// enterable and has a row error.
    pub selector_enterable: bool,
    /// Whether the description is enterable for the next request: only on the row of a single
    /// `U` whose update has not completed. A description in error is enterable and has a row
    /// error.
    pub description_enterable: bool,
}

impl From<&rules::ShownRow> for TranTypeListShownRow {
    fn from(row: &rules::ShownRow) -> Self {
        Self {
            type_code: text(row.code()),
            description: row.description().to_owned(),
            selection: row
                .selection()
                .shown()
                .map(String::from)
                .unwrap_or_default(),
            highlighted: row.emphasis() == rules::Emphasis::Highlighted,
            selector_enterable: row.selector() != rules::FieldState::ReadOnly,
            description_enterable: row.description_field() != rules::FieldState::ReadOnly,
        }
    }
}

/// Whether a filter of the response adds a predicate, and whether it is in error, red with the
/// focus (COTRTLIC-028, COTRTLIC-031, COTRTLIC-057).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranTypeListFilterStatus {
    /// `blank`: no predicate; a type filter `00` is blank (COTRTLIC-027).
    Blank,
    /// `valid`: a predicate in force.
    Valid,
    /// `in_error`: flagged by its own edit or by the cross edit; the error line holds its
    /// message.
    InError,
}

impl From<rules::FilterStatus> for TranTypeListFilterStatus {
    fn from(status: rules::FilterStatus) -> Self {
        match status {
            rules::FilterStatus::Blank => Self::Blank,
            rules::FilterStatus::Valid => Self::Valid,
            rules::FilterStatus::InError => Self::InError,
        }
    }
}

/// A filter as `2400-SETUP-SCREEN-ATTRS` shows it (COTRTLIC-057).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranTypeListShownFilter {
    /// The text shown, without trailing spaces: as received while a selection is non-blank or
    /// the filter is valid or in error, empty otherwise and on the screen load.
    pub text: String,
    /// Blank, valid or in error.
    pub status: TranTypeListFilterStatus,
}

impl From<&rules::ShownFilter> for TranTypeListShownFilter {
    fn from(filter: &rules::ShownFilter) -> Self {
        Self {
            text: filter.text().to_owned(),
            status: filter.status().into(),
        }
    }
}

/// What an endpoint 25 request committed (`FLG-UPDATE-COMPLETED`, `FLG-DELETED-YES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranTypeListCommitted {
    /// `nothing`.
    Nothing,
    /// `updated`: the selected row's description (COTRTLIC-045).
    Updated,
    /// `deleted`: the selected row (COTRTLIC-049).
    Deleted,
}

impl From<rules::Committed> for TranTypeListCommitted {
    fn from(committed: rules::Committed) -> Self {
        match committed {
            rules::Committed::Nothing => Self::Nothing,
            rules::Committed::Updated => Self::Updated,
            rules::Committed::Deleted => Self::Deleted,
        }
    }
}

/// The response of endpoint 24: the page shown and the state the next request echoes.
///
/// Answer with the value itself: its `IntoResponse` sends the audited status of the outcome,
/// 200, 400 for a filter in error or 500 for a store failure, with this body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranTypeListResponse {
    /// The seven positions of the page, top to bottom; `null` is an empty position.
    #[schema(max_items = 7)]
    pub rows: Vec<Option<TranTypeListShownRow>>,
    /// The type filter shown.
    pub type_filter: TranTypeListShownFilter,
    /// The description filter shown.
    pub desc_filter: TranTypeListShownFilter,
    /// The page number shown, `PAGENOO`.
    pub page_no: i64,
    /// Whether a next page exists for the page shown.
    pub has_next_page: bool,
    /// The error line, `ERRMSG`: an error, or a paging message such as `No more pages to
    /// display`.
    pub message: Option<String>,
    /// The info line, `INFOMSG`, up to 45 characters, such as `Type U to update, D to delete
    /// any record`.
    pub info_message: Option<String>,
    /// The state, which the screen echoes with ENTER and F10.
    pub state: TranTypeListState,
    /// The state as the opaque key the screen echoes with F7 and F8.
    pub page_key: String,
    /// The audited status, which travels as the response status and is no part of the body.
    #[serde(skip, default = "status_not_carried")]
    #[schema(ignore)]
    status: StatusCode,
}

impl TranTypeListResponse {
    /// The audited status of the outcome: 200 for a value read back from a body, which does
    /// not carry it.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }
}

impl From<rules::TranTypeListPage> for TranTypeListResponse {
    fn from(page: rules::TranTypeListPage) -> Self {
        let state = TranTypeListState::from(page.state());
        Self {
            rows: shown_rows(&page),
            type_filter: page.type_filter().into(),
            desc_filter: page.desc_filter().into(),
            page_no: i64::from(page.page_no().get()),
            has_next_page: page.next_page() == rules::NextPage::Exists,
            message: page.message_text(),
            info_message: info_text(&page),
            page_key: page_key(&state),
            state,
            status: audited_status(&page),
        }
    }
}

/// The audited status, then the body as JSON without it.
impl IntoResponse for TranTypeListResponse {
    fn into_response(self) -> Response {
        let status = self.status;
        (status, Json(self)).into_response()
    }
}

/// One row of an endpoint 25 request: the selection and the description as typed.
///
/// The type code is no input: the 3270 returns the protected code field as displayed, so the
/// codes are the state's (COTRTLIC-017).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
pub struct TranTypeListRowInput {
    /// `TRTSELn PIC X(1)`: `U` update, `D` delete, blank for none. Codes are case-sensitive,
    /// and any other value is the domain's invalid action code (COTRTLIC-021).
    #[serde(default)]
    #[validate(length(max = 1))]
    #[schema(max_length = 1)]
    pub selection: String,
    /// `TRTYPDn PIC X(50)`: read only at the row the previous response left enterable
    /// (COTRTLIC-018).
    #[serde(default)]
    #[validate(length(max = 50))]
    #[schema(max_length = 50)]
    pub description: String,
}

impl<'a> From<&'a TranTypeListRowInput> for rules::RowInput<'a> {
    fn from(row: &'a TranTypeListRowInput) -> Self {
        Self {
            selection: &row.selection,
            description: &row.description,
        }
    }
}

/// The body of endpoint 25: the key, the map input and the echoed state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema, Validate)]
pub struct TranTypeListActionRequest {
    /// `ENTER` or `PF10`.
    pub aid: Aid,
    /// `TRTYPEI PIC X(2)`, as typed: the domain's type-filter edit owns its message
    /// (COTRTLIC-027, COTRTLIC-028).
    #[serde(default)]
    #[validate(length(max = 2))]
    #[schema(max_length = 2)]
    pub type_filter: String,
    /// `TRDESCI PIC X(50)`, as typed (COTRTLIC-030).
    #[serde(default)]
    #[validate(length(max = 50))]
    #[schema(max_length = 50)]
    pub desc_filter: String,
    /// The seven rows, top to bottom; missing rows are blank. More than seven do not
    /// deserialise, since the map has no field for them.
    // The seven-row bound is a deserialisation rule rather than a `length` rule because
    // `validator` panics when a `length` and a `nested` rule on one field both fail.
    #[serde(default, deserialize_with = "at_most_seven")]
    #[validate(nested)]
    #[schema(max_items = 7)]
    pub rows: Vec<TranTypeListRowInput>,
    /// The `state` of the previous response, echoed unchanged.
    #[validate(nested)]
    pub state: TranTypeListState,
    /// Rows per page, 1 to 100, 7 when absent; a page never holds more than the seven
    /// positions of the map. Outside 1 to 100 the request is rejected with 422 (Bounded
    /// Pagination).
    #[validate(range(min = 1, max = 100))]
    #[schema(minimum = 1, maximum = 100)]
    pub page_size: Option<i64>,
}

/// The rows of an endpoint 25 body, at most the seven positions of the map.
fn at_most_seven<'de, D>(deserializer: D) -> Result<Vec<TranTypeListRowInput>, D::Error>
where
    D: Deserializer<'de>,
{
    let rows = Vec::<TranTypeListRowInput>::deserialize(deserializer)?;
    if rows.len() > rules::SCREEN_ROWS {
        Err(D::Error::invalid_length(rows.len(), &"at most seven rows"))
    } else {
        Ok(rows)
    }
}

/// A field of one row that the response flags in error, red with the focus (COTRTLIC-021,
/// COTRTLIC-022, COTRTLIC-025, COTRTLIC-026).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({"row": 3, "field": "selection", "message": "Action code selected is invalid"}))]
pub struct TranTypeListRowError {
    /// The screen row, 1 to 7.
    #[schema(minimum = 1, maximum = 7)]
    pub row: i64,
    /// `selection` or `description`.
    pub field: String,
    /// The error line's message, the one message COTRTLIC sends with the flagged fields
    /// (COTRTLIC-033).
    pub message: String,
}

/// The response of endpoint 25: the page as re-read, or the state's rows where nothing was
/// read, with the row errors, the messages, the commit and the new state.
///
/// Answer with the value itself: its `IntoResponse` sends the audited status of the outcome,
/// 200, 400, 404 or 500 (COTRTLIC.md, "Reading the table"), with this body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct TranTypeListActionResult {
    /// The seven positions shown, top to bottom; `null` is an empty position.
    #[schema(max_items = 7)]
    pub rows: Vec<Option<TranTypeListShownRow>>,
    /// The row fields in error, top to bottom.
    pub row_errors: Vec<TranTypeListRowError>,
    /// The type filter shown.
    pub type_filter: TranTypeListShownFilter,
    /// The description filter shown.
    pub desc_filter: TranTypeListShownFilter,
    /// The page number shown, `PAGENOO`.
    pub page_no: i64,
    /// Whether a next page exists for the page shown.
    pub has_next_page: bool,
    /// The error line, `ERRMSG`: an error, `No change detected with respect to database
    /// values.` or the fixed start of a Db2 message (COTRTLIC-054; the rest is `HALTED(H3)`).
    pub message: Option<String>,
    /// The info line, `INFOMSG`: the confirmation prompt, the commit text or `Type U to
    /// update, D to delete any record`.
    pub info_message: Option<String>,
    /// What the request committed.
    pub committed: TranTypeListCommitted,
    /// The new state, the pending action set when ENTER validated one; the screen echoes it
    /// with ENTER and F10.
    pub state: TranTypeListState,
    /// The new state as the opaque key the screen echoes with F7 and F8.
    pub page_key: String,
    /// The audited status, which travels as the response status and is no part of the body.
    #[serde(skip, default = "status_not_carried")]
    #[schema(ignore)]
    status: StatusCode,
}

impl TranTypeListActionResult {
    /// The audited status of the outcome: 200 for a value read back from a body, which does
    /// not carry it.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.status
    }
}

impl From<rules::ListActionOutcome> for TranTypeListActionResult {
    fn from(outcome: rules::ListActionOutcome) -> Self {
        let state = TranTypeListState::from(outcome.state());
        Self {
            rows: shown_rows(&outcome),
            row_errors: row_errors(&outcome),
            type_filter: outcome.type_filter().into(),
            desc_filter: outcome.desc_filter().into(),
            page_no: i64::from(outcome.page_no().get()),
            has_next_page: outcome.next_page() == rules::NextPage::Exists,
            message: outcome.message_text(),
            info_message: info_text(&outcome),
            committed: outcome.committed().into(),
            page_key: page_key(&state),
            state,
            status: audited_status(&outcome),
        }
    }
}

/// The audited status, then the body as JSON without it.
impl IntoResponse for TranTypeListActionResult {
    fn into_response(self) -> Response {
        let status = self.status;
        (status, Json(self)).into_response()
    }
}

/// The seven positions an outcome shows.
fn shown_rows(outcome: &rules::ListActionOutcome) -> Vec<Option<TranTypeListShownRow>> {
    outcome
        .rows()
        .iter()
        .map(|row| row.as_ref().map(TranTypeListShownRow::from))
        .collect()
}

/// The selectors and descriptions an outcome flags in error, each with the error line.
fn row_errors(outcome: &rules::ListActionOutcome) -> Vec<TranTypeListRowError> {
    let message = outcome.message_text().unwrap_or_default();
    let mut errors = Vec::new();
    for (row, index) in outcome.rows().iter().zip(rules::RowIndex::ALL) {
        let Some(row) = row else {
            continue;
        };
        let fields = [
            (row.selector(), SELECTION_FIELD),
            (row.description_field(), DESCRIPTION_FIELD),
        ];
        for (state, field) in fields {
            if state == rules::FieldState::Error {
                errors.push(TranTypeListRowError {
                    row: row_number(index),
                    field: field.to_owned(),
                    message: message.clone(),
                });
            }
        }
    }
    errors
}

/// The info line's text.
fn info_text(outcome: &rules::ListActionOutcome) -> Option<String> {
    outcome.info().map(|info| info.text().to_owned())
}

/// The HTTP status the domain's status class assigns; never another success status.
fn audited_status(outcome: &rules::ListActionOutcome) -> StatusCode {
    StatusCode::from_u16(outcome.status().http_status())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

/// The status of a response read back from a body, which does not carry it.
const fn status_not_carried() -> StatusCode {
    StatusCode::OK
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use axum::extract::Query;
    use axum::http::{StatusCode, Uri};
    use axum::response::IntoResponse;
    use domain::copybook::PicX;
    use domain::online::cotrtlic as rules;
    use domain::records::db2_transaction_type::Db2TransactionType;
    use utoipa::openapi::path::ParameterIn;
    use utoipa::{IntoParams, ToSchema};
    use validator::Validate;

    use super::{
        Aid, TranTypeListActionRequest, TranTypeListActionResult, TranTypeListCommitted,
        TranTypeListFilterStatus, TranTypeListPendingAction, TranTypeListQuery,
        TranTypeListResponse, TranTypeListRow, TranTypeListRowError, TranTypeListRowInput,
        TranTypeListSelectors, TranTypeListState, hex, unhex,
    };
    use crate::dto::{PAGE_SIZE_FIELD, PageDirection, resolve_page_size, validation_error};

    type TestResult = Result<(), Box<dyn StdError>>;

    /// A change to a valid body that breaks one of its rules.
    type BreakRule = fn(&mut TranTypeListActionRequest);

    /// The Db2 seed of migration 0006 (`DB2LTTYP.ctl:15-23`).
    const SEED: [(&str, &str); 7] = [
        ("01", "PURCHASE"),
        ("02", "PAYMENT"),
        ("03", "CREDIT"),
        ("04", "AUTHORIZATION"),
        ("05", "REFUND"),
        ("06", "REVERAL"),
        ("07", "ADJUSTMENT"),
    ];

    /// `db2.transaction_type` in code order, answering the domain's store steps.
    struct Table(Vec<Db2TransactionType>);

    impl Table {
        fn seed() -> Result<Self, Box<dyn StdError>> {
            let mut rows = Vec::new();
            for (code, description) in SEED {
                rows.push(Db2TransactionType::new(
                    PicX::from_text(code)?,
                    description.to_owned(),
                )?);
            }
            Ok(Self(rows))
        }

        fn matches(row: &Db2TransactionType, filters: &rules::FilterQuery) -> bool {
            filters
                .type_filter()
                .is_none_or(|code| row.tr_type() == code)
                && filters
                    .desc_pattern()
                    .is_none_or(|pattern| row.tr_description().contains(pattern.trim_matches('%')))
        }

        fn count(&self, filters: &rules::FilterQuery) -> i64 {
            let matching = self.0.iter().filter(|row| Self::matches(row, filters));
            i64::try_from(matching.count()).unwrap_or(i64::MAX)
        }

        fn read(&self, query: &rules::ReadQuery) -> Vec<Db2TransactionType> {
            let limit = usize::try_from(query.limit()).unwrap_or(0);
            let filtered = self
                .0
                .iter()
                .filter(|row| Self::matches(row, query.filters()));
            match query.direction() {
                rules::ReadDirection::Forward => filtered
                    .filter(|row| row.tr_type() >= query.start())
                    .take(limit)
                    .cloned()
                    .collect(),
                rules::ReadDirection::Backward => filtered
                    .rev()
                    .filter(|row| row.tr_type() < query.start())
                    .take(limit)
                    .cloned()
                    .collect(),
            }
        }

        fn find(&self, code: &PicX<2>) -> Option<Db2TransactionType> {
            self.0.iter().find(|row| row.tr_type() == code).cloned()
        }

        fn commit(&mut self, effect: &rules::CommitEffect) -> rules::CommitResult {
            let Some(position) = self.0.iter().position(|row| row.tr_type() == effect.code())
            else {
                return rules::CommitResult::NotFound;
            };
            match effect {
                rules::CommitEffect::Update { code, description } => {
                    match Db2TransactionType::new(code.clone(), description.clone()) {
                        Ok(row) => {
                            self.0.splice(position..=position, [row]);
                            rules::CommitResult::Done
                        }
                        Err(_) => rules::CommitResult::Failed,
                    }
                }
                rules::CommitEffect::Delete { .. } => {
                    self.0.remove(position);
                    rules::CommitResult::Done
                }
            }
        }

        fn description(&self, code: &str) -> Option<String> {
            let code = PicX::from_text(code).ok()?;
            self.find(&code)
                .map(|row| row.tr_description().trim_end_matches(' ').to_owned())
        }
    }

    /// Every store step of `progress` answered from `table`, as `api::services::cotrtlic` runs
    /// them.
    fn run(table: &mut Table, mut progress: rules::Progress) -> rules::ListActionOutcome {
        loop {
            progress = match progress {
                rules::Progress::Count(step) => {
                    let count = table.count(step.query());
                    step.finish(Ok(count))
                }
                rules::Progress::Verify(step) => {
                    let current = table.find(step.code());
                    step.finish(current.as_ref())
                }
                rules::Progress::Commit(step) => {
                    let result = table.commit(step.effect());
                    step.finish(result)
                }
                rules::Progress::Read(step) => {
                    let rows = table.read(step.query());
                    return step.finish(Ok(rows));
                }
                rules::Progress::Done(outcome) => return *outcome,
            };
        }
    }

    fn page_size(requested: Option<i64>) -> Result<rules::PageSize, Box<dyn StdError>> {
        let default = u32::try_from(rules::DEFAULT_PAGE_SIZE)?;
        Ok(rules::PageSize::from_u32(resolve_page_size(requested, default)).ok_or("page size")?)
    }

    fn load(table: &mut Table) -> Result<TranTypeListResponse, Box<dyn StdError>> {
        Ok(run(table, rules::load(page_size(None)?)).into())
    }

    /// An endpoint 25 body over `state`: the filters, then `(row number, selection,
    /// description)` for the rows that are not blank.
    fn request(
        aid: Aid,
        state: &TranTypeListState,
        filters: (&str, &str),
        entries: &[(usize, &str, &str)],
    ) -> TranTypeListActionRequest {
        let rows = (1..=7)
            .map(|number| {
                entries
                    .iter()
                    .find(|(wanted, ..)| *wanted == number)
                    .map_or_else(
                        TranTypeListRowInput::default,
                        |&(_, selection, description)| TranTypeListRowInput {
                            selection: selection.to_owned(),
                            description: description.to_owned(),
                        },
                    )
            })
            .collect();
        TranTypeListActionRequest {
            aid,
            type_filter: filters.0.to_owned(),
            desc_filter: filters.1.to_owned(),
            rows,
            state: state.clone(),
            page_size: None,
        }
    }

    /// Endpoint 25 as the service runs it: validated, the state rebuilt, the domain run.
    fn act(
        table: &mut Table,
        body: &TranTypeListActionRequest,
    ) -> Result<TranTypeListActionResult, Box<dyn StdError>> {
        body.validate()?;
        let state = rules::TranTypeListState::try_from(&body.state)?;
        let rows: Vec<rules::RowInput<'_>> = body.rows.iter().map(rules::RowInput::from).collect();
        let input = rules::ListInput {
            type_filter: &body.type_filter,
            desc_filter: &body.desc_filter,
            rows: &rows,
        };
        let progress = rules::act(body.aid.into(), &input, &state, page_size(body.page_size)?);
        Ok(run(table, progress).into())
    }

    fn codes<T>(rows: &[Option<T>], code: fn(&T) -> &str) -> Vec<String> {
        rows.iter()
            .map(|row| row.as_ref().map(code).unwrap_or_default().to_owned())
            .collect()
    }

    #[test]
    fn the_wire_forms_and_schema_names_are_the_contract() -> TestResult {
        for (aid, wire) in [(Aid::Enter, "\"ENTER\""), (Aid::Pf10, "\"PF10\"")] {
            assert_eq!(serde_json::to_string(&aid)?, wire);
            assert_eq!(serde_json::from_str::<Aid>(wire)?, aid);
            assert_eq!(
                rules::ListAid::from(aid),
                match aid {
                    Aid::Enter => rules::ListAid::Enter,
                    Aid::Pf10 => rules::ListAid::Pf10,
                }
            );
        }
        assert!(serde_json::from_str::<Aid>("\"enter\"").is_err());
        assert!(serde_json::from_str::<Aid>("\"PF5\"").is_err());
        for (pending, wire, domain) in [
            (
                TranTypeListPendingAction::None,
                "\"none\"",
                rules::PendingAction::None,
            ),
            (
                TranTypeListPendingAction::Update,
                "\"update\"",
                rules::PendingAction::Update,
            ),
            (
                TranTypeListPendingAction::Delete,
                "\"delete\"",
                rules::PendingAction::Delete,
            ),
            (
                TranTypeListPendingAction::UpdateAndDelete,
                "\"update_and_delete\"",
                rules::PendingAction::UpdateAndDelete,
            ),
        ] {
            assert_eq!(serde_json::to_string(&pending)?, wire);
            assert_eq!(rules::PendingAction::from(pending), domain);
            assert_eq!(TranTypeListPendingAction::from(domain), pending);
        }
        for (selectors, wire, domain) in [
            (
                TranTypeListSelectors::Open,
                "\"open\"",
                rules::SelectorAccess::Open,
            ),
            (
                TranTypeListSelectors::Protected,
                "\"protected\"",
                rules::SelectorAccess::Protected,
            ),
        ] {
            assert_eq!(serde_json::to_string(&selectors)?, wire);
            assert_eq!(rules::SelectorAccess::from(selectors), domain);
            assert_eq!(TranTypeListSelectors::from(domain), selectors);
        }
        assert_eq!(
            serde_json::to_string(&TranTypeListFilterStatus::InError)?,
            "\"in_error\""
        );
        assert_eq!(
            serde_json::to_string(&TranTypeListCommitted::Deleted)?,
            "\"deleted\""
        );
        let names = [
            Aid::name(),
            TranTypeListState::name(),
            TranTypeListRow::name(),
            TranTypeListRowInput::name(),
            TranTypeListRowError::name(),
            TranTypeListPendingAction::name(),
            TranTypeListActionRequest::name(),
            TranTypeListActionResult::name(),
            TranTypeListResponse::name(),
        ];
        assert_eq!(
            names.map(std::borrow::Cow::into_owned),
            [
                "CotrtlicAid",
                "TranTypeListState",
                "TranTypeListRow",
                "TranTypeListRowInput",
                "TranTypeListRowError",
                "TranTypeListPendingAction",
                "TranTypeListActionRequest",
                "TranTypeListActionResult",
                "TranTypeListResponse",
            ]
            .map(str::to_owned)
        );
        Ok(())
    }

    #[test]
    fn the_screen_load_carries_its_state_and_a_page_key_that_round_trip() -> TestResult {
        let mut table = Table::seed()?;
        let page = load(&mut table)?;
        assert_eq!(page.status(), StatusCode::OK);
        assert_eq!(
            codes(&page.rows, |row| row.type_code.as_str()),
            SEED.map(|(code, _)| code.to_owned())
        );
        assert_eq!((page.page_no, page.has_next_page), (1, false));
        assert_eq!(page.message, None);
        assert_eq!(
            page.info_message.as_deref(),
            Some("Type U to update, D to delete any record")
        );
        assert_eq!(page.type_filter.status, TranTypeListFilterStatus::Blank);
        assert_eq!(
            page.state.rows.first(),
            Some(&Some(TranTypeListRow {
                type_code: "01".to_owned(),
                description: "PURCHASE".to_owned(),
            }))
        );
        assert_eq!(page.state.first_key.as_deref(), Some("01"));
        assert_eq!(page.state.pending_action, TranTypeListPendingAction::None);

        let domain_state = rules::TranTypeListState::try_from(&page.state)?;
        assert_eq!(TranTypeListState::from(&domain_state), page.state);
        let query = TranTypeListQuery {
            page_key: Some(page.page_key.clone()),
            direction: Some(PageDirection::Forward),
            ..TranTypeListQuery::default()
        };
        query.validate()?;
        assert_eq!(query.state()?, Some(domain_state));

        // The status is no part of the body; a body read back answers 200.
        let body = serde_json::to_value(&page)?;
        assert!(body.get("status").is_none());
        assert_eq!(serde_json::from_value::<TranTypeListResponse>(body)?, page);
        assert_eq!(page.into_response().status(), StatusCode::OK);
        Ok(())
    }

    #[test]
    fn an_echoed_state_keeps_empty_positions_both_flags_and_page_zero() -> TestResult {
        let padded = format!("{:<50}", "REFUND");
        let read = [
            None,
            Some(rules::RowParts {
                code: "05",
                description: &padded,
            }),
        ];
        let shown = [
            None,
            Some(rules::RowParts {
                code: "05",
                description: "NEW TEXT",
            }),
        ];
        let domain_state = rules::TranTypeListState::from_parts(&rules::StateParts {
            type_filter: "A",
            desc_filter: "  FUND",
            rows: &read,
            selected_row: Some(2),
            first_key: "05",
            last_key: "",
            page_no: 0,
            next_page: rules::NextPage::NotExists,
            last_page: rules::LastPage::Shown,
            pending: rules::PendingAction::UpdateAndDelete,
            screen_rows: &shown,
            selectors: rules::SelectorAccess::Protected,
            description_entry: Some(2),
        })?;
        let state = TranTypeListState::from(domain_state.clone());
        assert_eq!(state.rows.len(), 7);
        assert_eq!(state.rows.first(), Some(&None));
        assert_eq!(
            (state.type_filter.as_str(), state.desc_filter.as_str()),
            ("A", "  FUND")
        );
        assert_eq!(
            (state.first_key.as_deref(), state.last_key.as_deref()),
            (Some("05"), None)
        );
        assert_eq!((state.page_no, state.last_page_shown), (0, true));
        assert_eq!(state.selectors, TranTypeListSelectors::Protected);
        state.validate()?;

        let json = serde_json::to_string(&state)?;
        let echoed: TranTypeListState = serde_json::from_str(&json)?;
        assert_eq!(rules::TranTypeListState::try_from(&echoed)?, domain_state);
        Ok(())
    }

    // audit: COTRTLIC-021
    // audit: COTRTLIC-039
    // audit: COTRTLIC-049
    #[test]
    fn enter_and_f10_carry_the_row_errors_the_prompt_and_the_delete() -> TestResult {
        let mut table = Table::seed()?;
        let state = load(&mut table)?.state;

        let invalid = act(
            &mut table,
            &request(Aid::Enter, &state, ("", ""), &[(3, "X", "")]),
        )?;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        let message = "Action code selected is invalid";
        assert_eq!(invalid.message.as_deref(), Some(message));
        assert_eq!(
            invalid.row_errors,
            [TranTypeListRowError {
                row: 3,
                field: "selection".to_owned(),
                message: message.to_owned(),
            }]
        );
        assert_eq!(
            invalid
                .rows
                .get(2)
                .and_then(Option::as_ref)
                .map(|row| row.selection.as_str()),
            Some("X")
        );
        assert_eq!(invalid.into_response().status(), StatusCode::BAD_REQUEST);

        let prompted = act(
            &mut table,
            &request(Aid::Enter, &state, ("", ""), &[(2, "D", "")]),
        )?;
        assert_eq!(prompted.status(), StatusCode::OK);
        assert_eq!(
            prompted.info_message.as_deref(),
            Some("Delete HIGHLIGHTED row ? Press F10 to confirm")
        );
        assert!(prompted.row_errors.is_empty());
        assert_eq!(
            (prompted.state.pending_action, prompted.state.selected_row),
            (TranTypeListPendingAction::Delete, Some(2))
        );
        let row = prompted
            .rows
            .get(1)
            .and_then(Option::as_ref)
            .ok_or("row 2")?;
        assert!(row.highlighted && row.selector_enterable && !row.description_enterable);
        assert_eq!(prompted.committed, TranTypeListCommitted::Nothing);

        let deleted = act(
            &mut table,
            &request(Aid::Pf10, &prompted.state, ("", ""), &[(2, "D", "")]),
        )?;
        assert_eq!(deleted.committed, TranTypeListCommitted::Deleted);
        assert_eq!(
            deleted.info_message.as_deref(),
            Some("HIGHLIGHTED row deleted.Hit Enter to continue")
        );
        assert_eq!(table.description("02"), None);
        assert_eq!(
            (deleted.state.pending_action, deleted.state.page_no),
            (TranTypeListPendingAction::None, 1)
        );
        // The reset state's blank rows still echo.
        rules::TranTypeListState::try_from(&deleted.state)?;
        Ok(())
    }

    // audit: COTRTLIC-024
    // audit: COTRTLIC-040
    // audit: COTRTLIC-045
    #[test]
    fn an_update_opens_its_description_and_f10_writes_it() -> TestResult {
        let mut table = Table::seed()?;
        let state = load(&mut table)?.state;

        let opened = act(
            &mut table,
            &request(Aid::Enter, &state, ("", ""), &[(1, "U", "")]),
        )?;
        assert_eq!(
            opened.message.as_deref(),
            Some("No change detected with respect to database values.")
        );
        assert_eq!(
            opened.info_message.as_deref(),
            Some("Update HIGHLIGHTED row. Press F10 to save")
        );
        assert_eq!(opened.state.description_entry, Some(1));
        let row = opened
            .rows
            .first()
            .and_then(Option::as_ref)
            .ok_or("row 1")?;
        assert!(row.description_enterable);

        let edited = act(
            &mut table,
            &request(
                Aid::Enter,
                &opened.state,
                ("", ""),
                &[(1, "U", "Bought goods")],
            ),
        )?;
        assert_eq!(edited.status(), StatusCode::OK);
        let row = edited
            .rows
            .first()
            .and_then(Option::as_ref)
            .ok_or("row 1")?;
        assert_eq!(row.description, "Bought goods");

        let saved = act(
            &mut table,
            &request(
                Aid::Pf10,
                &edited.state,
                ("", ""),
                &[(1, "U", "Bought goods")],
            ),
        )?;
        assert_eq!(saved.committed, TranTypeListCommitted::Updated);
        assert_eq!(
            saved.info_message.as_deref(),
            Some("HIGHLIGHTED row was updated")
        );
        assert_eq!(table.description("01").as_deref(), Some("Bought goods"));

        let blank = act(
            &mut table,
            &request(Aid::Enter, &opened.state, ("", ""), &[(1, "U", "   ")]),
        )?;
        assert_eq!(blank.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            blank.row_errors,
            [TranTypeListRowError {
                row: 1,
                field: "description".to_owned(),
                message: "Transaction Desc must be supplied.".to_owned(),
            }]
        );
        Ok(())
    }

    // audit: COTRTLIC-028
    // audit: COTRTLIC-031
    #[test]
    fn a_filter_in_error_is_reported_on_the_filter_shown() -> TestResult {
        let mut table = Table::seed()?;
        let state = load(&mut table)?.state;

        let not_numeric = act(&mut table, &request(Aid::Enter, &state, ("A1", ""), &[]))?;
        assert_eq!(not_numeric.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            not_numeric.type_filter.status,
            TranTypeListFilterStatus::InError
        );
        assert_eq!(not_numeric.type_filter.text, "A1");
        assert_eq!(
            not_numeric.state.selectors,
            TranTypeListSelectors::Protected
        );
        assert_eq!(not_numeric.state.page_no, 0);

        let no_rows = act(
            &mut table,
            &request(Aid::Enter, &state, ("", "NOTHING"), &[]),
        )?;
        assert_eq!(
            no_rows.message.as_deref(),
            Some("No Records found for these filter conditions")
        );
        assert_eq!(
            no_rows.desc_filter.status,
            TranTypeListFilterStatus::InError
        );

        let filtered = act(&mut table, &request(Aid::Enter, &state, ("03", ""), &[]))?;
        assert_eq!(
            codes(&filtered.rows, |row| row.type_code.as_str()),
            ["03", "", "", "", "", "", ""].map(str::to_owned)
        );
        assert_eq!(filtered.type_filter.status, TranTypeListFilterStatus::Valid);
        Ok(())
    }

    #[test]
    fn request_rules_name_the_field_or_answer_422() -> TestResult {
        let mut table = Table::seed()?;
        let state = load(&mut table)?.state;
        let base = request(Aid::Enter, &state, ("", ""), &[]);
        assert!(base.validate().is_ok());

        let cases: [(BreakRule, &str); 7] = [
            (|body| body.type_filter = "123".to_owned(), "type_filter"),
            (|body| body.desc_filter = "D".repeat(51), "desc_filter"),
            (
                |body| {
                    if let Some(row) = body.rows.get_mut(2) {
                        row.selection = "DU".to_owned();
                    }
                },
                "rows[2].selection",
            ),
            (|body| body.state.page_no = 10, "state.page_no"),
            (
                |body| body.state.selected_row = Some(8),
                "state.selected_row",
            ),
            (
                |body| {
                    body.state.screen_rows = vec![Some(TranTypeListRow {
                        type_code: "01".to_owned(),
                        description: "X".repeat(51),
                    })];
                },
                "state.screen_rows",
            ),
            (|body| body.state.rows = vec![None; 8], "state.rows"),
        ];
        for (break_rule, field) in cases {
            let mut body = base.clone();
            break_rule(&mut body);
            let errors = body.validate().err().ok_or(field)?;
            let error = validation_error(&errors);
            assert_eq!(
                (error.status(), error.field()),
                (StatusCode::BAD_REQUEST, Some(field))
            );
        }

        for size in [0, 101, -1] {
            let mut body = base.clone();
            body.page_size = Some(size);
            body.type_filter = "123".to_owned();
            let errors = body.validate().err().ok_or("page_size accepted")?;
            let error = validation_error(&errors);
            assert_eq!(
                (error.status(), error.field()),
                (StatusCode::UNPROCESSABLE_ENTITY, Some(PAGE_SIZE_FIELD))
            );
        }
        Ok(())
    }

    #[test]
    fn rows_beyond_the_seven_positions_do_not_deserialise() -> TestResult {
        let mut table = Table::seed()?;
        let state = serde_json::to_value(load(&mut table)?.state)?;
        let row = serde_json::json!({"selection": "", "description": ""});
        let body = |count: usize| {
            serde_json::json!({
                "aid": "ENTER",
                "rows": vec![row.clone(); count],
                "state": state.clone(),
            })
        };
        assert!(serde_json::from_value::<TranTypeListActionRequest>(body(8)).is_err());
        let seven: TranTypeListActionRequest = serde_json::from_value(body(7))?;
        assert_eq!((seven.rows.len(), seven.type_filter.as_str()), (7, ""));
        let sparse: TranTypeListActionRequest = serde_json::from_value(serde_json::json!({
            "aid": "PF10",
            "rows": [{"selection": "D"}],
            "state": state,
        }))?;
        assert_eq!(
            sparse.rows,
            [TranTypeListRowInput {
                selection: "D".to_owned(),
                description: String::new(),
            }]
        );
        assert_eq!(sparse.page_size, None);
        Ok(())
    }

    #[test]
    fn page_keys_are_issued_here_or_rejected() -> TestResult {
        assert_eq!(hex(&[0x00, 0x7B, 0xFF]), "007bff");
        assert_eq!(unhex("007bff"), Some(vec![0x00, 0x7B, 0xFF]));
        assert_eq!(unhex("007BFF"), Some(vec![0x00, 0x7B, 0xFF]));
        assert_eq!(unhex(""), Some(Vec::new()));
        for broken in ["0", "0g", "zz", "+f", "\u{e9}e"] {
            assert_eq!(unhex(broken), None, "{broken:?}");
        }

        let mut table = Table::seed()?;
        let issued = load(&mut table)?.page_key;
        for (page_key, valid) in [
            (issued.clone(), true),
            (String::new(), true),
            ("7b7d".to_owned(), false),
            (hex(b"{\"type_filter\":"), false),
            (format!("{issued}0"), false),
        ] {
            let query = TranTypeListQuery {
                page_key: Some(page_key.clone()),
                ..TranTypeListQuery::default()
            };
            match query.validate() {
                Ok(()) => assert!(valid, "{page_key}"),
                Err(errors) => {
                    assert!(!valid, "{page_key}");
                    let error = validation_error(&errors);
                    assert_eq!(
                        (error.status(), error.field()),
                        (StatusCode::BAD_REQUEST, Some("page_key"))
                    );
                    assert_eq!(query.state(), Err(rules::ListFailure::InvalidKey));
                }
            }
        }
        let empty = TranTypeListQuery {
            page_key: Some(String::new()),
            ..TranTypeListQuery::default()
        };
        assert_eq!(empty.state(), Ok(None));
        assert_eq!(TranTypeListQuery::default().state(), Ok(None));
        Ok(())
    }

    #[test]
    fn the_query_is_a_query_string() -> TestResult {
        let names: Vec<(String, bool)> = TranTypeListQuery::into_params(|| None)
            .into_iter()
            .map(|param| (param.name, param.parameter_in == ParameterIn::Query))
            .collect();
        assert_eq!(
            names,
            [
                "type_filter",
                "desc_filter",
                "page_key",
                "direction",
                "page_size"
            ]
            .map(|name| (name.to_owned(), true))
        );

        let uri: Uri =
            "/api/v1/transaction-types?type_filter=03&direction=backward&page_size=7".parse()?;
        let Query(query) = Query::<TranTypeListQuery>::try_from_uri(&uri)?;
        assert_eq!(
            query,
            TranTypeListQuery {
                type_filter: Some("03".to_owned()),
                direction: Some(PageDirection::Backward),
                page_size: Some(7),
                ..TranTypeListQuery::default()
            }
        );
        let too_long: Uri = "/api/v1/transaction-types?type_filter=123".parse()?;
        let Query(query) = Query::<TranTypeListQuery>::try_from_uri(&too_long)?;
        let errors = query.validate().err().ok_or("over-long filter accepted")?;
        assert_eq!(validation_error(&errors).field(), Some("type_filter"));
        Ok(())
    }
}
